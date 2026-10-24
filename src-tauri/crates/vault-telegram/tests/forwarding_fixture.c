/* Test-only pinned tdjson send lifecycle. All calls occur on the receive worker. */
#include <stdio.h>
#include <string.h>
#include <time.h>
static char queue[32][2048], result[2048];
static unsigned head, tail;
static const char *audio = "\"content\":{\"@type\":\"messageAudio\",\"caption\":{\"text\":\"\"},\"audio\":{\"audio\":{\"id\":1,\"size\":42},\"file_name\":\"song.mp3\",\"mime_type\":\"audio/mpeg\"}}";
int td_create_client_id(void) { head=tail=0; return 1; }
const char *td_execute(const char *request) { (void)request; return "{\"@type\":\"ok\"}"; }
void td_send(int id, const char *request) {
    (void)id;
    if (strstr(request,"\"close\"")) {
        snprintf(queue[tail++%32],2048,"{\"@client_id\":1,\"@type\":\"updateAuthorizationState\",\"authorization_state\":{\"@type\":\"authorizationStateClosed\"}}");
        return;
    }
    const char *extra=strstr(request,"\"@extra\":\"");
    if (!extra) return;
    extra+=10;
    char correlation[64]={0};
    size_t length=strcspn(extra,"\"");
    if (length>=sizeof(correlation)) return;
    memcpy(correlation,extra,length);
    if (strstr(request,"\"getMessage\"")) {
        int mid=strstr(request,"2097152") ? 2097152 : 1048576;
        snprintf(queue[tail++%32],2048,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"message\",\"id\":%d,\"chat_id\":-100,\"edit_date\":0,%s}",correlation,mid,audio);
    } else if (strstr(request,"\"getMessageProperties\"")) {
        snprintf(queue[tail++%32],2048,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"messageProperties\",\"can_be_forwarded\":true,\"can_be_copied\":%s}",correlation,strstr(request,"2097152") ? "false" : "true");
    } else if (strstr(request,"\"getChat\"")) {
        int chat=strstr(request,"-201") ? -201 : -200;
        snprintf(queue[tail++%32],2048,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"chat\",\"id\":%d,\"type\":{\"@type\":\"chatTypePrivate\"}}",correlation,chat);
    } else if (strstr(request,"\"sendMessage\"")) {
        int copying = strstr(request,"\"inputMessageForwarded\"") != 0;
        int valid_copy = copying && strstr(request,"\"send_copy\":true") && strstr(request,"\"replace_caption\":true") && strstr(request,"\"new_caption\":{") && strstr(request,"\"from_chat_id\":-100") && strstr(request,"\"message_id\":1048576");
        int valid_audio = !copying && strstr(request,"\"inputMessageAudio\"") && strstr(request,"\"inputAudio\"") && strstr(request,"\"inputFileLocal\"") && strstr(request,"\"title\":\"Track title\"") && strstr(request,"\"performer\":\"Artist\"") && strstr(request,"\"duration\":123") && strstr(request,"\"text\":\"media marker\"");
        if (!valid_audio && !valid_copy) {
            snprintf(queue[tail++%32],2048,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"error\",\"code\":400}",correlation);
            return;
        }
        snprintf(queue[tail++%32],2048,"{\"@client_id\":1,\"@type\":\"updateMessageSendSucceeded\",\"old_message_id\":-8,\"message\":{\"@type\":\"message\",\"chat_id\":-200,\"id\":10485760,\"edit_date\":0,%s}}",audio);
        snprintf(queue[tail++%32],2048,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"message\",\"id\":-8,\"chat_id\":-200,\"sending_state\":{\"@type\":\"messageSendingStatePending\"}}",correlation);
    } else if (strstr(request,"\"forwardMessages\"")) {
        int chat=strstr(request,"-201") ? -201 : -200;
        if (!strstr(request,"\"send_copy\":true") || !strstr(request,"\"remove_caption\":true") || strstr(request,"\"protect_content\":true")) {
            snprintf(queue[tail++%32],2048,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"error\",\"code\":400}",correlation);
            return;
        }
        /* Queue success BEFORE the request reply to catch late subscriptions. */
        if (chat == -201)
            snprintf(queue[tail++%32],2048,"{\"@client_id\":1,\"@type\":\"updateMessageSendFailed\",\"old_message_id\":-7,\"message\":{\"chat_id\":%d},\"error\":{\"code\":403}}",chat);
        else
            snprintf(queue[tail++%32],2048,"{\"@client_id\":1,\"@type\":\"updateMessageSendSucceeded\",\"old_message_id\":-7,\"message\":{\"@type\":\"message\",\"chat_id\":%d,\"id\":10485760,\"edit_date\":0,%s}}",chat,audio);
        snprintf(queue[tail++%32],2048,"{\"@client_id\":1,\"@extra\":\"%s\",\"@type\":\"messages\",\"messages\":[{\"@type\":\"message\",\"id\":-7,\"chat_id\":%d,\"sending_state\":{\"@type\":\"messageSendingStatePending\"}}]}",correlation,chat);
    }
}
const char *td_receive(double timeout) {
    if(head!=tail) { strcpy(result,queue[head++%32]); return result; }
    struct timespec ts={0,(long)(timeout*1000000000)};
    nanosleep(&ts,0);
    return 0;
}
