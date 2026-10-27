/* Test-only TDLib ABI double. Session files live only in Rust TempDirs. */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
static char queue[512][4096], result[4096], state[80], session_path[2048];
static unsigned head, tail;
static int delay, next_method, sms, resend_calls, phone_calls, code_calls, qr_calls, qr_generation, me_calls;
static char control_path[2048], pending_extra[80];

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
        snprintf(out,size,"\"@type\":\"authorizationStateWaitOtherDeviceConfirmation\",\"link\":\"tg://login?token=PRIVATE_QR_TOKEN_%d\"",qr_generation);
    else snprintf(out,size,"\"@type\":\"authorizationState%s\"",state);
}
static void update(void) {
    char auth[2048]; auth_json(auth,sizeof(auth));
    snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@type\":\"updateAuthorizationState\",\"authorization_state\":{%s}}",auth);
}
static void connection(int ready) {
    snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@type\":\"updateConnectionState\",\"state\":{\"@type\":\"connectionState%s\"}}",ready ? "Ready" : "WaitingForNetwork");
}
static void error(const char *extra, int code, const char *message) {
    snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@type\":\"error\",\"@extra\":\"%s\",\"code\":%d,\"message\":\"%s\"}",extra,code,message);
}
int td_create_client_id(void) {
    head=tail=0; delay=0; next_method=1; sms=0;
    resend_calls=phone_calls=code_calls=qr_calls=qr_generation=me_calls=0; session_path[0]=control_path[0]=pending_extra[0]=0;
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
        if(strcmp(state,"Ready")) { error(extra,401,"GET_ME_BEFORE_READY"); return; }
        me_calls++;
        snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"user\",\"id\":123,\"first_name\":\"Test\",\"last_name\":\"Account\"}",extra); return;
    }
    if(!strcmp(method,"testStats")) {
        snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"testStats\",\"resend_calls\":%d,\"phone_calls\":%d,\"code_calls\":%d,\"qr_calls\":%d,\"me_calls\":%d}",extra,resend_calls,phone_calls,code_calls,qr_calls,me_calls); return;
    }
    int changed=1;
    if(!strcmp(method,"setTdlibParameters")) {
        char directory[1800]; field(request,"database_directory",directory,sizeof(directory));
        snprintf(session_path,sizeof(session_path),"%s/fixture-session",directory);
        snprintf(control_path,sizeof(control_path),"%s/fixture-control",directory);
        connection(1);
        strcpy(state,"WaitPhoneNumber");
        FILE *f=fopen(session_path,"r");
        if(f) { int ignored=fscanf(f,"%79s %d %d %d",state,&delay,&next_method,&sms); (void)ignored; fclose(f); changed=0; }
    } else if(!strcmp(method,"testSetState")) {
        field(request,"state",state,sizeof(state)); delay=number(request,"delay"); next_method=number(request,"next");
    } else if(!strcmp(method,"testConnection")) {
        connection(number(request,"ready")); changed=0;
    } else if(!strcmp(method,"testRotateQr")) {
        qr_generation++;
    } else if(!strcmp(method,"testReleaseAuth")) {
        snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"ok\"}",pending_extra);
        pending_extra[0]=0; changed=0;
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
    else if(!strcmp(method,"requestQrCodeAuthentication")) {
        qr_calls++; strcpy(state,"WaitOtherDeviceConfirmation");
    }
    else if(!strcmp(method,"logOut") || !strcmp(method,"destroy")) { error(extra,400,"TEST_FORBIDS_SESSION_DELETION"); return; }
    else changed=0;
    save();
    if(changed) update();
    if(strstr(request,"INTERRUPTED_CODE_PRIVATE")) { strcpy(pending_extra,extra); return; } /* Accepted, but its reply is lost. */
    snprintf(queue[tail++%512],4096,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"ok\"}",extra);
}
const char *td_receive(double timeout) {
    if(control_path[0]) {
        FILE *f=fopen(control_path,"r");
        if(f) {
            char command[80]={0}; int ignored=fscanf(f,"%79s",command); (void)ignored; fclose(f);
            remove(control_path);
            if(!strcmp(command,"offline")) connection(0);
            else if(!strcmp(command,"online")) connection(1);
            else if(!strcmp(command,"rotate")) { qr_generation++; update(); }
            else if(command[0]) { strcpy(state,command); save(); update(); }
        }
    }
    if(head!=tail) { strcpy(result,queue[head++%512]); return result; }
    struct timespec ts={0,(long)(timeout*1000000000)}; nanosleep(&ts,0); return 0;
}
