/* Test-only tdjson ABI fixture. No sockets, credentials, or Telegram account. */
#include <stdio.h>
#include <string.h>
#include <time.h>
static char queue[512][2048];
static char result[2048];
static unsigned head, tail;
static int initialized, me_calls;
int td_create_client_id(void) { head=tail=0; initialized=0; me_calls=0; return 1; }
const char *td_execute(const char *request) { (void)request; return "{\"@type\":\"ok\"}"; }
static void auth_update(const char *state) {
    snprintf(queue[tail++%512],2048,"{\"@client_id\":1,\"@type\":\"updateAuthorizationState\",\"authorization_state\":{\"@type\":\"%s\"}}",state);
}
void td_send(int id,const char *request) {
    (void)id;
    if(strstr(request,"\"close\"")) { auth_update("authorizationStateClosed"); return; }
    const char *extra=strstr(request,"\"@extra\":\"");
    if(!extra) { auth_update("authorizationStateWaitTdlibParameters"); return; }
    extra+=10;
    char correlation[64]={0};
    size_t length=strcspn(extra,"\"");
    if(length>=sizeof(correlation)) return;
    memcpy(correlation,extra,length);
    if(strstr(request,"getOption"))
        snprintf(queue[tail++%512],2048,"{\"@client_id\":1,\"@type\":\"optionValueString\",\"@extra\":\"%s\",\"value\":\"1.8.69\"}",correlation);
    else if(strstr(request,"getAuthorizationState")) {
        snprintf(queue[tail++%512],2048,"{\"@client_id\":1,\"@type\":\"%s\",\"@extra\":\"%s\"}",initialized ? "authorizationStateWaitPhoneNumber" : "authorizationStateWaitTdlibParameters",correlation);
        /* Ready arrives asynchronously, not as the response to connect_native. */
        if(initialized) auth_update("authorizationStateReady");
    } else if(strstr(request,"getMe")) {
        /* Ready must activate even if getMe initially hits a transient failure. */
        if (me_calls++ == 0)
            snprintf(queue[tail++%512],2048,"{\"@client_id\":1,\"@type\":\"error\",\"@extra\":\"%s\",\"code\":500,\"message\":\"fixture transient failure\"}",correlation);
        else
            snprintf(queue[tail++%512],2048,"{\"@client_id\":1,\"@type\":\"user\",\"@extra\":\"%s\",\"id\":123}",correlation);
    }
    else {
        snprintf(queue[tail++%512],2048,"{\"@client_id\":1,\"@type\":\"ok\",\"@extra\":\"%s\"}",correlation);
        if(strstr(request,"setTdlibParameters")) {
            initialized=1;
            auth_update("authorizationStateWaitPhoneNumber");
        }
    }
}
const char *td_receive(double timeout) {
    if(head!=tail) { strcpy(result,queue[head++%512]); return result; }
    struct timespec ts={0,(long)(timeout*1000000000)};
    nanosleep(&ts,0);
    return 0;
}
