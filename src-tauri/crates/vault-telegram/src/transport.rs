use crate::{TdClient, TelegramError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{future::Future, path::PathBuf, time::Duration};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use vault_domain::{ChatId, MessageId, UserId};
use vault_protocol::Record;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteFile {
    pub file_id: i32,
    pub size: i64,
    pub name: String,
    pub mime_type: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteMessage {
    pub chat_id: ChatId,
    pub id: MessageId,
    pub text: String,
    pub file: Option<RemoteFile>,
    /// Channel-origin, unscheduled, not forwarded/edited. Required for service records.
    pub trusted_control: bool,
}
impl RemoteMessage {
    pub fn record(&self) -> Result<Option<Record>, TelegramError> {
        let record =
            vault_protocol::classify_text(&self.text).map_err(|_| TelegramError::Protocol)?;
        if record.is_some() && !self.trusted_control {
            return Err(TelegramError::InvalidVault);
        }
        Ok(record)
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelInfo {
    pub chat_id: ChatId,
    pub owner_id: UserId,
    pub title: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Update {
    New(RemoteMessage),
    Changed {
        chat_id: ChatId,
        message_id: MessageId,
    },
    Deleted {
        chat_id: ChatId,
        message_ids: Vec<MessageId>,
    },
    Disconnected,
    Connected,
    AuthRequired,
}

/// Transport methods return only confirmed, permanent messages. A timeout on a
/// send has unknown outcome: the caller must reconcile, never blindly resend.
pub trait Telegram: Send + Sync {
    fn subscribe(&self) -> broadcast::Receiver<Update>;
    fn verify_channel(
        &self,
        chat: ChatId,
        owner: UserId,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<ChannelInfo, TelegramError>> + Send;
    /// Strictly older than `before`; empty means history exhausted, not a short page.
    fn history(
        &self,
        chat: ChatId,
        before: Option<MessageId>,
        limit: u8,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<Vec<RemoteMessage>, TelegramError>> + Send;
    fn message(
        &self,
        chat: ChatId,
        id: MessageId,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<Option<RemoteMessage>, TelegramError>> + Send;
    fn send_text(
        &self,
        chat: ChatId,
        text: String,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<RemoteMessage, TelegramError>> + Send;
    fn send_document(
        &self,
        chat: ChatId,
        path: PathBuf,
        caption: String,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<RemoteMessage, TelegramError>> + Send;
    fn download(
        &self,
        file: RemoteFile,
        offset: i64,
        limit: i64,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<DownloadedRange, TelegramError>> + Send;
    /// Forward one user-selected media message to one user-selected chat only.
    fn forward_media(
        &self,
        source_chat: ChatId,
        message_id: MessageId,
        vault_id: uuid::Uuid,
        recipient_chat: ChatId,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<RemoteMessage, TelegramError>> + Send;
    fn delete_messages(
        &self,
        chat: ChatId,
        ids: Vec<MessageId>,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<(), TelegramError>> + Send;
}
#[derive(Debug, Clone)]
pub struct DownloadedRange {
    pub path: PathBuf,
    pub offset: i64,
    pub available: i64,
    pub complete: bool,
}

/// Owns an update forwarder. Every clone shares one forwarder and stops it on drop.
#[derive(Clone)]
pub struct TdTransport {
    client: TdClient,
    updates: broadcast::Sender<Update>,
    _pump: std::sync::Arc<Pump>,
}
struct Pump(CancellationToken);
impl Drop for Pump {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
impl TdTransport {
    pub fn new(client: TdClient) -> Self {
        let mut input = client.subscribe();
        let (updates, _) = broadcast::channel(2048);
        let out = updates.clone();
        let token = CancellationToken::new();
        let cancel = token.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _=cancel.cancelled()=>break,
                    value=input.recv()=>match value {
                        Ok(value)=>match parse_update(&value) {
                            Ok(Some(update))=>{let _=out.send(update);},Ok(None)=>{},
                            Err(_)=>{let _=out.send(Update::Disconnected);}
                        },
                        Err(broadcast::error::RecvError::Lagged(_))=>{let _=out.send(Update::Disconnected);},
                        Err(broadcast::error::RecvError::Closed)=>{let _=out.send(Update::AuthRequired);break;}
                    }
                }
            }
        });
        Self {
            client,
            updates,
            _pump: std::sync::Arc::new(Pump(token)),
        }
    }
    pub fn client(&self) -> &TdClient {
        &self.client
    }
    async fn send_content(
        &self,
        chat: ChatId,
        content: Value,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, TelegramError> {
        let updates = self.client.subscribe();
        let response=self.client.request(json!({"@type":"sendMessage","chat_id":chat.get(),"topic_id":null,"reply_to":null,"options":null,"reply_markup":null,"input_message_content":content}),Duration::from_secs(120),cancel).await?;
        self.confirm_send(chat, response, updates, cancel).await
    }
    pub async fn confirm_send(
        &self,
        chat: ChatId,
        response: Value,
        mut updates: broadcast::Receiver<Value>,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, TelegramError> {
        if response["sending_state"].is_null() {
            return parse_message(&response, chat);
        }
        let temporary = integer(&response["id"])?;
        let wait = async {
            loop {
                let update = updates
                    .recv()
                    .await
                    .map_err(|_| TelegramError::UpdatesLost)?;
                if integer(&update["old_message_id"]).ok() != Some(temporary)
                    || integer(&update["message"]["chat_id"]).ok() != Some(chat.get())
                {
                    continue;
                }
                match update["@type"].as_str() {
                    Some("updateMessageSendSucceeded") => {
                        return parse_message(&update["message"], chat);
                    }
                    Some("updateMessageSendFailed") => {
                        return Err(TelegramError::Request {
                            code: integer(&update["error"]["code"])?,
                        });
                    }
                    _ => {}
                }
            }
        };
        tokio::select! {_=cancel.cancelled()=>Err(TelegramError::Cancelled),result=tokio::time::timeout(Duration::from_secs(300),wait)=>result.map_err(|_|TelegramError::Timeout)?}
    }
}
impl Telegram for TdTransport {
    fn subscribe(&self) -> broadcast::Receiver<Update> {
        self.updates.subscribe()
    }
    async fn verify_channel(
        &self,
        chat: ChatId,
        owner: UserId,
        cancel: &CancellationToken,
    ) -> Result<ChannelInfo, TelegramError> {
        let me = self.client.read(json!({"@type":"getMe"}), cancel).await?;
        if integer(&me["id"])? != owner.get() {
            return Err(TelegramError::AuthRequired);
        }
        let info = self
            .client
            .read(json!({"@type":"getChat","chat_id":chat.get()}), cancel)
            .await?;
        if info["type"]["@type"] != "chatTypeSupergroup"
            || info["type"]["is_channel"] != true
            || integer(&info["message_auto_delete_time"])? != 0
        {
            return Err(TelegramError::UnsafeChannel);
        }
        let group = integer(&info["type"]["supergroup_id"])?;
        let basic = self
            .client
            .read(
                json!({"@type":"getSupergroup","supergroup_id":group}),
                cancel,
            )
            .await?;
        let full = self
            .client
            .read(
                json!({"@type":"getSupergroupFullInfo","supergroup_id":group}),
                cancel,
            )
            .await?;
        validate_channel(&info, &basic, &full, chat, owner)
    }
    async fn history(
        &self,
        chat: ChatId,
        before: Option<MessageId>,
        limit: u8,
        cancel: &CancellationToken,
    ) -> Result<Vec<RemoteMessage>, TelegramError> {
        if limit == 0 || limit > 100 {
            return Err(TelegramError::InvalidInput);
        }
        // TDLib encodes server IDs with 20 low zero bits. Subtract one SERVER id,
        // not one raw integer (which would create an invalid scheduled/local id).
        let from = match before {
            None => 0,
            Some(id) => {
                validate_server_id(id.get())?;
                if id.get() == 1 << 20 {
                    return Ok(Vec::new());
                }
                id.get() - (1 << 20)
            }
        };
        let result=self.client.read(json!({"@type":"getChatHistory","chat_id":chat.get(),"from_message_id":from,"offset":0,"limit":limit,"only_local":false}),cancel).await?;
        let messages = result["messages"]
            .as_array()
            .ok_or(TelegramError::InvalidResponse)?;
        let mut parsed = Vec::new();
        for value in messages {
            if !value["sending_state"].is_null() {
                return Err(TelegramError::IncompleteHistory);
            }
            let message = parse_message(value, chat)?;
            if before.is_some_and(|id| message.id >= id) {
                return Err(TelegramError::IncompleteHistory);
            }
            parsed.push(message);
        }
        Ok(parsed)
    }
    async fn message(
        &self,
        chat: ChatId,
        id: MessageId,
        cancel: &CancellationToken,
    ) -> Result<Option<RemoteMessage>, TelegramError> {
        match self
            .client
            .read(
                json!({"@type":"getMessage","chat_id":chat.get(),"message_id":id.get()}),
                cancel,
            )
            .await
        {
            Ok(value) => {
                let message = parse_message(&value, chat)?;
                if message.id != id {
                    return Err(TelegramError::InvalidResponse);
                }
                Ok(Some(message))
            }
            Err(TelegramError::Request { code: 404 }) => Ok(None),
            Err(error) => Err(error),
        }
    }
    async fn send_text(
        &self,
        chat: ChatId,
        text: String,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, TelegramError> {
        if text.encode_utf16().count() > 4096 {
            return Err(TelegramError::InvalidInput);
        }
        self.send_content(chat,json!({"@type":"inputMessageText","text":{"@type":"formattedText","text":text,"entities":[]},"link_preview_options":{"@type":"linkPreviewOptions","is_disabled":true},"clear_draft":false}),cancel).await
    }
    async fn send_document(
        &self,
        chat: ChatId,
        path: PathBuf,
        caption: String,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, TelegramError> {
        if !path.is_absolute() || !path.is_file() || caption.encode_utf16().count() > 1024 {
            return Err(TelegramError::InvalidInput);
        }
        self.send_content(chat,json!({"@type":"inputMessageDocument","document":{"@type":"inputDocument","document":{"@type":"inputFileLocal","path":path.to_str().ok_or(TelegramError::InvalidInput)?},"thumbnail":null,"disable_content_type_detection":true},"caption":{"@type":"formattedText","text":caption,"entities":[]}}),cancel).await
    }
    async fn download(
        &self,
        file: RemoteFile,
        offset: i64,
        limit: i64,
        cancel: &CancellationToken,
    ) -> Result<DownloadedRange, TelegramError> {
        if file.file_id <= 0 || offset < 0 || limit < 0 || offset.checked_add(limit).is_none() {
            return Err(TelegramError::InvalidInput);
        }
        let response=self.client.request(json!({"@type":"downloadFile","file_id":file.file_id,"priority":16,"offset":offset,"limit":limit,"synchronous":true}),Duration::from_secs(300),cancel).await;
        let value = match response {
            Ok(value) => value,
            Err(error) => {
                // Cancellation stops transport work as well; no shared library mutation.
                let _=self.client.call(json!({"@type":"cancelDownloadFile","file_id":file.file_id,"only_if_pending":false})).await;
                return Err(error);
            }
        };
        if integer(&value["id"])? != i64::from(file.file_id) {
            return Err(TelegramError::InvalidResponse);
        }
        let local = &value["local"];
        let complete = local["is_downloading_completed"] == true;
        let start = integer(&local["download_offset"])?;
        let prefix = integer(&local["downloaded_prefix_size"])?;
        if start > offset || start.checked_add(prefix).is_none_or(|end| end < offset) {
            return Err(TelegramError::InvalidResponse);
        }
        let available = if complete {
            integer(&value["size"])? - offset
        } else {
            start + prefix - offset
        };
        if available < 0
            || (limit == 0 && !complete)
            || (limit > 0 && available < limit && !complete)
        {
            return Err(TelegramError::Timeout);
        }
        Ok(DownloadedRange {
            path: PathBuf::from(
                local["path"]
                    .as_str()
                    .ok_or(TelegramError::InvalidResponse)?,
            ),
            offset,
            available,
            complete,
        })
    }
    async fn forward_media(
        &self,
        source_chat: ChatId,
        message_id: MessageId,
        vault_id: uuid::Uuid,
        recipient_chat: ChatId,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, TelegramError> {
        if source_chat == recipient_chat {
            return Err(TelegramError::InvalidInput);
        }
        let source = self
            .message(source_chat, message_id, cancel)
            .await?
            .ok_or(TelegramError::Request { code: 404 })?;
        let record = source.record()?;
        if source.file.is_none()
            || record.is_some_and(
                |record| !matches!(record, Record::Media(media) if media.vault_id == vault_id),
            )
        {
            return Err(TelegramError::Unshareable);
        }
        let properties = self
            .client
            .read(
                json!({"@type":"getMessageProperties","chat_id":source_chat.get(),"message_id":message_id.get()}),
                cancel,
            )
            .await?;
        if properties["can_be_forwarded"] != true {
            return Err(TelegramError::Unshareable);
        }
        let recipient = self
            .client
            .read(
                json!({"@type":"getChat","chat_id":recipient_chat.get()}),
                cancel,
            )
            .await?;
        if integer(&recipient["id"])? != recipient_chat.get()
            || !matches!(
                recipient["type"]["@type"].as_str(),
                Some("chatTypePrivate" | "chatTypeBasicGroup" | "chatTypeSupergroup")
            )
        {
            return Err(TelegramError::InvalidInput);
        }
        let response = self
            .client
            .request(
                json!({"@type":"forwardMessages","chat_id":recipient_chat.get(),"from_chat_id":source_chat.get(),"message_ids":[message_id.get()],"options":{"@type":"messageSendOptions","disable_notification":false,"from_background":false,"protect_content":true},"send_copy":true,"remove_caption":true}),
                Duration::from_secs(60),
                cancel,
            )
            .await?;
        let messages = response["messages"]
            .as_array()
            .filter(|messages| messages.len() == 1)
            .ok_or(TelegramError::InvalidResponse)?;
        let forwarded = parse_message(&messages[0], recipient_chat)?;
        if forwarded.file.is_none() {
            return Err(TelegramError::InvalidResponse);
        }
        Ok(forwarded)
    }
    async fn delete_messages(
        &self,
        chat: ChatId,
        ids: Vec<MessageId>,
        cancel: &CancellationToken,
    ) -> Result<(), TelegramError> {
        if ids.is_empty() || ids.len() > 100 {
            return Err(TelegramError::InvalidInput);
        }
        for id in &ids {
            let properties=self.client.read(json!({"@type":"getMessageProperties","chat_id":chat.get(),"message_id":id.get()}),cancel).await?;
            if properties["can_be_deleted_for_all_users"] != true {
                return Err(TelegramError::UnsafeChannel);
            }
        }
        self.client.request(json!({"@type":"deleteMessages","chat_id":chat.get(),"message_ids":ids.into_iter().map(MessageId::get).collect::<Vec<_>>(),"revoke":true}),Duration::from_secs(30),cancel).await?;
        Ok(())
    }
}

pub fn integer(value: &Value) -> Result<i64, TelegramError> {
    if let Some(n) = value.as_i64() {
        return Ok(n);
    }
    if let Some(s) = value.as_str() {
        let n: i64 = s.parse().map_err(|_| TelegramError::InvalidResponse)?;
        if n.to_string() == s {
            return Ok(n);
        }
    }
    Err(TelegramError::InvalidResponse)
}
fn validate_server_id(n: i64) -> Result<MessageId, TelegramError> {
    if n <= 0 || n > ((i64::from(i32::MAX)) << 20) || n & ((1 << 20) - 1) != 0 {
        return Err(TelegramError::InvalidResponse);
    }
    MessageId::new(n).map_err(|_| TelegramError::InvalidResponse)
}
pub fn parse_message(value: &Value, chat: ChatId) -> Result<RemoteMessage, TelegramError> {
    if value["@type"] != "message"
        || integer(&value["chat_id"])? != chat.get()
        || !value["sending_state"].is_null()
        || !value["scheduling_state"].is_null()
    {
        return Err(TelegramError::InvalidResponse);
    }
    let id = validate_server_id(integer(&value["id"])?)?;
    let content = &value["content"];
    let (text, file) = match content["@type"].as_str() {
        Some("messageText") => (
            content["text"]["text"]
                .as_str()
                .ok_or(TelegramError::InvalidResponse)?
                .to_owned(),
            None,
        ),
        Some("messageDocument" | "messageAudio") => {
            let (object, key) = if content["@type"] == "messageAudio" {
                (&content["audio"], "audio")
            } else {
                (&content["document"], "document")
            };
            let file = &object[key];
            (
                content["caption"]["text"]
                    .as_str()
                    .ok_or(TelegramError::InvalidResponse)?
                    .to_owned(),
                Some(RemoteFile {
                    file_id: i32::try_from(integer(&file["id"])?)
                        .map_err(|_| TelegramError::InvalidResponse)?,
                    size: integer(&file["size"])?,
                    name: object["file_name"].as_str().unwrap_or("").into(),
                    mime_type: object["mime_type"].as_str().unwrap_or("").into(),
                }),
            )
        }
        Some(_) => (String::new(), None),
        None => return Err(TelegramError::InvalidResponse),
    };
    let trusted_control = value["is_channel_post"] == true
        && value["forward_info"].is_null()
        && integer(&value["edit_date"])? == 0
        && value["sender_id"]["@type"] == "messageSenderChat"
        && integer(&value["sender_id"]["chat_id"]).ok() == Some(chat.get());
    Ok(RemoteMessage {
        chat_id: chat,
        id,
        text,
        file,
        trusted_control,
    })
}
pub fn validate_channel(
    chat: &Value,
    group: &Value,
    full: &Value,
    chat_id: ChatId,
    owner_id: UserId,
) -> Result<ChannelInfo, TelegramError> {
    if integer(&chat["id"])? != chat_id.get()
        || chat["type"]["@type"] != "chatTypeSupergroup"
        || chat["type"]["is_channel"] != true
        || integer(&chat["message_auto_delete_time"])? != 0
        || group["is_channel"] != true
        || group["status"]["@type"] != "chatMemberStatusCreator"
        || group["status"]["is_member"] != true
        || integer(&full["member_count"])? != 1
        || integer(&full["administrator_count"])? != 1
        || integer(&full["linked_chat_id"])? != 0
        || group["has_location"] != false
        || group["has_linked_chat"] != false
        || group["usernames"]["active_usernames"]
            .as_array()
            .is_some_and(|names| !names.is_empty())
    {
        return Err(TelegramError::UnsafeChannel);
    }
    Ok(ChannelInfo {
        chat_id,
        owner_id,
        title: chat["title"]
            .as_str()
            .ok_or(TelegramError::InvalidResponse)?
            .into(),
    })
}
fn parse_update(value: &Value) -> Result<Option<Update>, TelegramError> {
    let chat = |v: &Value| ChatId::new(integer(v)?).map_err(|_| TelegramError::InvalidResponse);
    Ok(match value["@type"].as_str() {
        Some("updateNewMessage") => {
            if !value["message"]["sending_state"].is_null() {
                None
            } else {
                let id = chat(&value["message"]["chat_id"])?;
                Some(Update::New(parse_message(&value["message"], id)?))
            }
        }
        Some("updateMessageContent" | "updateMessageEdited") => Some(Update::Changed {
            chat_id: chat(&value["chat_id"])?,
            message_id: validate_server_id(integer(&value["message_id"])?)?,
        }),
        Some("updateDeleteMessages") if value["is_permanent"] == true => Some(Update::Deleted {
            chat_id: chat(&value["chat_id"])?,
            message_ids: value["message_ids"]
                .as_array()
                .ok_or(TelegramError::InvalidResponse)?
                .iter()
                .map(|v| validate_server_id(integer(v)?))
                .collect::<Result<_, _>>()?,
        }),
        Some("updateConnectionState") if value["state"]["@type"] != "connectionStateReady" => {
            Some(Update::Disconnected)
        }
        Some("updateConnectionState") => Some(Update::Connected),
        Some("updateAuthorizationState")
            if value["authorization_state"]["@type"] != "authorizationStateReady" =>
        {
            Some(Update::AuthRequired)
        }
        Some("updateAuthorizationState") => Some(Update::Connected),
        _ => None,
    })
}
