/* Test-only TDLib ABI double. Session files live only in Rust TempDirs. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
static char queue[512][4096], result[4096], state[80], session_path[2048];
static unsigned head, tail;
static int delay, next_method, sms, resend_calls, phone_calls, code_calls;

static void field(const char *request, const char *name, char *out, size_t size) {
    char key[128];
    snprintf(key,sizeof(key),"\"%s\":\"",name);
    const char *p=strstr(request,key);
    if (!p) { out[0]=0; return; }
    p+=strlen(key);
    size_t n=strcspn(p,"\"");
    if(n>=size) n=size-1;
    memcpy(out,p,n); out[n]=0;
}
static int number(const char *request, const char *name) {
    char key[128]; snprintf(key,sizeof(key),"\"%s\":",name);
    const char *p=strstr(request,key);
    return p ? atoi(p+strlen(key)) : 0;
}
static void save(void) {
    if (!session_path[0]) return;
    FILE *f=fopen(session_path,"w");
    if(f) { fprintf(f,"%s %d %d %d",state,delay,next_method,sms); fclose(f); }
}
static void auth_json(char *out, size_t size) {
    if(!strcmp(state,"WaitCode"))
        snprintf(out,size,"\"@type\":\"authorizationStateWaitCode\",\"code_info\":{\"phone_number\":\"PRIVATE_PHONE\",\"type\":{\"@type\":\"authenticationCodeType%s\",\"length\":5,\"receipt\":\"PRIVATE_RECEIPT\"},\"timeout\":%d,\"next_type\":%s}",
            sms ? "Sms" : "TelegramMessage",delay,next_method ? "{\"@type\":\"authenticationCodeTypeSms\",\"length\":5}" : "null");
    else if(!strcmp(state,"WaitEmailCode"))
        snprintf(out,size,"\"@type\":\"authorizationStateWaitEmailCode\",\"code_info\":{\"email_address_pattern\":\"PRIVATE_EMAIL\",\"length\":6}");
    else if(!strcmp(state,"WaitOtherDeviceConfirmation"))
        snprintf(out,size,"\"@type\":\"authorizationStateWaitOtherDeviceConfirmation\",\"link\":\"tg://login?token=PRIVATE_QR_TOKEN\"");
    else snprintf(out,size,"\"@type\":\"authorizationState%s\"",state);
}
static void update(void) {
    char auth[2048]; auth_json(auth,sizeof(auth));
    snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@type\":\"updateAuthorizationState\",\"authorization_state\":{%s}}",auth);
}
static void error(const char *extra, int code, const char *message) {
    snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@type\":\"error\",\"@extra\":\"%s\",\"code\":%d,\"message\":\"%s\"}",extra,code,message);
}
int td_create_client_id(void) {
    head=tail=0; delay=0; next_method=1; sms=0;
    resend_calls=phone_calls=code_calls=0; session_path[0]=0;
    strcpy(state,"WaitTdlibParameters"); return 1;
}
const char *td_execute(const char *request) { (void)request; return "{\"@type\":\"ok\"}"; }
void td_send(int id, const char *request) {
    (void)id;
    char method[128], extra[80]; field(request,"@type",method,sizeof(method)); field(request,"@extra",extra,sizeof(extra));
    if(!strcmp(method,"close")) { strcpy(state,"Closed"); update(); return; }
    if(!extra[0]) { update(); return; }
    if(!strcmp(method,"getAuthorizationState")) {
        char auth[2048]; auth_json(auth,sizeof(auth));
        snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@extra\":\"%s\",%s}",extra,auth); return;
    }
    if(!strcmp(method,"getOption")) {
        snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"optionValueString\",\"value\":\"1.8.69\"}",extra); return;
    }
    if(!strcmp(method,"getMe")) {
        snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"user\",\"id\":123}",extra); return;
    }
    if(!strcmp(method,"testStats")) {
        snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"testStats\",\"resend_calls\":%d,\"phone_calls\":%d,\"code_calls\":%d}",extra,resend_calls,phone_calls,code_calls); return;
    }
    int changed=1;
    if(!strcmp(method,"setTdlibParameters")) {
        char directory[1800]; field(request,"database_directory",directory,sizeof(directory));
        snprintf(session_path,sizeof(session_path),"%s/fixture-session",directory);
        strcpy(state,"WaitPhoneNumber");
        FILE *f=fopen(session_path,"r");
        if(f) { int ignored=fscanf(f,"%79s %d %d %d",state,&delay,&next_method,&sms); (void)ignored; fclose(f); changed=0; }
    } else if(!strcmp(method,"testSetState")) {
        field(request,"state",state,sizeof(state)); delay=number(request,"delay"); next_method=number(request,"next");
    } else if(!strcmp(method,"testSetTimeout")) {
        delay=number(request,"delay"); changed=0; /* TDLib need not emit a countdown update. */
    } else if(!strcmp(method,"setAuthenticationPhoneNumber")) {
        phone_calls++; strcpy(state,"WaitCode"); delay=30; next_method=1;
    } else if(!strcmp(method,"resendAuthenticationCode")) {
        resend_calls++;
        if(!strstr(request,"resendCodeReasonUserRequest")) { error(extra,400,"MISSING_REASON"); return; }
        if(!strcmp(state,"WaitCode") && (!next_method || delay>0)) { error(extra,400,"RESEND_FORBIDDEN"); return; }
        sms=1; delay=30;
    } else if(!strcmp(method,"checkAuthenticationCode") || !strcmp(method,"checkAuthenticationEmailCode")) {
        code_calls++;
        if(strstr(request,"WRONG_CODE_PRIVATE")) { error(extra,400,"PHONE_CODE_INVALID"); return; }
        if(strstr(request,"EXPIRED_CODE_PRIVATE")) { error(extra,400,"PHONE_CODE_EXPIRED"); return; }
        if(strstr(request,"FLOOD_CODE_PRIVATE")) { error(extra,429,"Too Many Requests: retry after 77"); return; }
        if(strstr(request,"NETWORK_CODE_PRIVATE")) { error(extra,400,"NETWORK_ERROR"); return; }
        if(strstr(request,"REJECT_CODE_PRIVATE")) { error(extra,403,"PRIVATE_PHONE PRIVATE_SESSION_DATA"); return; }
        strcpy(state,!strcmp(method,"checkAuthenticationEmailCode") ? "WaitCode" : "WaitPassword");
    } else if(!strcmp(method,"checkAuthenticationPassword")) {
        if(strstr(request,"WRONG_PASSWORD_PRIVATE")) { error(extra,400,"PASSWORD_HASH_INVALID"); return; }
        strcpy(state,"Ready");
    } else if(!strcmp(method,"setAuthenticationEmailAddress")) strcpy(state,"WaitEmailCode");
    else if(!strcmp(method,"requestQrCodeAuthentication")) strcpy(state,"WaitOtherDeviceConfirmation");
    else if(!strcmp(method,"logOut") || !strcmp(method,"destroy")) { error(extra,400,"TEST_FORBIDS_SESSION_DELETION"); return; }
    else changed=0;
    save();
    if(changed) update();
    if(strstr(request,"INTERRUPTED_CODE_PRIVATE")) return; /* Accepted, but its reply is lost. */
    snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"ok\"}",extra);
}
const char *td_receive(double timeout) {
    if(head!=tail) { strcpy(result,queue[head++%512]); return result; }
    struct timespec ts={0,(long)(timeout*1000000000)}; nanosleep(&ts,0); return 0;
}
