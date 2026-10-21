/* Test-only TDLib ABI double; never built or loaded by production code. */
#include <stdio.h>
#include <string.h>
#include <time.h>
static char queue[512][1024];
static char result[1024];
static unsigned head, tail;
int td_create_client_id(void) { head=tail=0; return 1; }
const char *td_execute(const char *request) { (void)request; return "{\"@type\":\"ok\"}"; }
void td_send(int id,const char *request) {
    (void)id;
    if(strstr(request,"neverReply")) return;
    if(strstr(request,"\"close\"")) {
        snprintf(queue[tail++%512],1024,"{\"@client_id\":1,\"@type\":\"updateAuthorizationState\",\"authorization_state\":{\"@type\":\"authorizationStateClosed\"}}");
        return;
    }
    const char *extra=strstr(request,"\"@extra\":\"");
    if(!extra) return;
    extra+=10;
    char correlation[64]={0};
    size_t length=strcspn(extra,"\"");
    if(length>=sizeof(correlation)) return;
    memcpy(correlation,extra,length);
    if(strstr(request,"rateLimited"))
        snprintf(queue[tail++%512],1024,"{\"@client_id\":1,\"@type\":\"error\",\"@extra\":\"%s\",\"code\":429,\"message\":\"Too Many Requests: retry after 23\"}",correlation);
    else if(strstr(request,"getOption"))
        snprintf(queue[tail++%512],1024,"{\"@client_id\":1,\"@type\":\"optionValueString\",\"@extra\":\"%s\",\"value\":\"1.8.69\"}",correlation);
    else
        snprintf(queue[tail++%512],1024,"{\"@client_id\":1,\"@type\":\"ok\",\"@extra\":\"%s\"}",correlation);
}
const char *td_receive(double timeout) {
    if(head!=tail) { strcpy(result,queue[head++%512]); return result; }
    struct timespec ts={0,(long)(timeout*1000000000)};
    nanosleep(&ts,0);
    return 0;
}
