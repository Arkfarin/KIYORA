use crate::{TdClient, TelegramError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use std::{future::Future, path::PathBuf, time::Duration};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use vault_domain::{ChatId, MessageId, UserId};
use vault_protocol::Record;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RemoteFile {
    pub file_id: i32,
    pub size: i64,
    pub name: String,
    pub mime_type: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AudioInfo {
    pub title: String,
    pub performer: String,
    pub duration_ms: u64,
    pub file: RemoteFile,
    pub thumbnail: Option<RemoteFile>,
}
/// Bounded Telegram display fields. The full metadata remains in TrackRegister.
#[derive(Debug, Clone)]
pub struct AudioUpload {
    pub path: PathBuf,
    pub title: String,
    pub performer: String,
    pub duration_seconds: u32,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageContent {
    Text,
    Audio,
    Document,
    #[default]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteMessage {
    /// Preserve the actual TDLib content kind; a caption alone is not evidence
    /// that a message is a disposable control record. Old serialized hints are
    /// unclassified until fetched again.
    #[serde(default)]
    pub content: MessageContent,
    #[serde(default)]
    pub sent_at_unix: Option<u64>,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ChannelInfo {
    #[schemars(with = "String")]
    pub chat_id: ChatId,
    #[schemars(with = "String")]
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
    /// Current account's upload limit. Download/copy limits are independent.
    fn upload_limit(
        &self,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<u64, TelegramError>> + Send;
    /// Positive search results are hints only. A miss never authorizes creation
    /// or proves absence; discovery uses a bounded history fallback.
    fn init_hint(
        &self,
        _chat: ChatId,
        _cancel: &CancellationToken,
    ) -> impl Future<Output = Result<Option<RemoteMessage>, TelegramError>> + Send {
        async { Ok(None) }
    }
    fn pinned_message(
        &self,
        _chat: ChatId,
        _cancel: &CancellationToken,
    ) -> impl Future<Output = Result<Option<RemoteMessage>, TelegramError>> + Send {
        async { Ok(None) }
    }
    fn audio_info(
        &self,
        _chat: ChatId,
        _id: MessageId,
        _cancel: &CancellationToken,
    ) -> impl Future<Output = Result<Option<AudioInfo>, TelegramError>> + Send {
        async { Ok(None) }
    }
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
    fn send_audio(
        &self,
        chat: ChatId,
        audio: AudioUpload,
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
    /// Check media type and copy permission before downloading incoming content.
    fn incoming_media(
        &self,
        chat: ChatId,
        id: MessageId,
        cancel: &CancellationToken,
    ) -> impl Future<Output = Result<RemoteMessage, TelegramError>> + Send;
    /// Copy selected incoming media into the verified vault, replacing its caption.
    /// Implementations must enforce Telegram's copy restriction, even on retries.
    fn copy_media(
        &self,
        source_chat: ChatId,
        message_id: MessageId,
        target_chat: ChatId,
        caption: String,
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
    /// Keep alive until the caller has finished reading the file. Maintenance
    /// must not remove TDLib's local file while a consumer is using this range.
    pub storage_lease: Option<Arc<tokio::sync::OwnedRwLockReadGuard<()>>>,
    pub path: PathBuf,
    /// Absolute offset represented by byte zero of `path`.
    pub file_offset: i64,
    pub offset: i64,
    pub available: i64,
    pub complete: bool,
}

/// Owns an update forwarder. Every clone shares one forwarder and stops it on drop.
#[derive(Clone)]
pub struct TdTransport {
    pub(crate) client: TdClient,
    pub(crate) discovery_gate: Arc<tokio::sync::Mutex<()>>,
    updates: broadcast::Sender<Update>,
    _pump: std::sync::Arc<Pump>,
    downloads: Arc<DownloadLocks>,
    pub(crate) storage: Arc<tokio::sync::RwLock<()>>,
}
#[derive(Default)]
struct DownloadLocks(Mutex<HashMap<i32, Weak<tokio::sync::Mutex<()>>>>);
impl DownloadLocks {
    async fn acquire(
        &self,
        file_id: i32,
        cancel: &CancellationToken,
    ) -> Result<tokio::sync::OwnedMutexGuard<()>, TelegramError> {
        let lock = {
            let mut files = self.0.lock().map_err(|_| TelegramError::Closed)?;
            files.retain(|_, lock| lock.strong_count() != 0);
            if let Some(lock) = files.get(&file_id).and_then(Weak::upgrade) {
                lock
            } else {
                let lock = Arc::new(tokio::sync::Mutex::new(()));
                files.insert(file_id, Arc::downgrade(&lock));
                lock
            }
        };
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(TelegramError::Cancelled),
            guard = lock.lock_owned() => Ok(guard),
        }
    }
}
struct Pump(CancellationToken);
impl Drop for Pump {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
impl TdTransport {
    async fn validate_upload(
        &self,
        path: &std::path::Path,
        cancel: &CancellationToken,
    ) -> Result<(), TelegramError> {
        if !path.is_absolute() {
            return Err(TelegramError::InvalidInput);
        }
        let metadata = tokio::fs::symlink_metadata(path)
            .await
            .map_err(|_| TelegramError::LocalFile)?;
        if !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > self.upload_limit(cancel).await?
        {
            return Err(TelegramError::InvalidInput);
        }
        Ok(())
    }
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
            discovery_gate: Arc::default(),
            updates,
            _pump: std::sync::Arc::new(Pump(token)),
            downloads: Arc::new(DownloadLocks::default()),
            storage: Arc::new(tokio::sync::RwLock::new(())),
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
    async fn upload_limit(&self, cancel: &CancellationToken) -> Result<u64, TelegramError> {
        let option = self
            .client
            .read(json!({"@type":"getOption","name":"is_premium"}), cancel)
            .await?;
        if option["@type"] != "optionValueBoolean" {
            return Err(TelegramError::InvalidResponse);
        }
        let premium = option["value"]
            .as_bool()
            .ok_or(TelegramError::InvalidResponse)?;
        // Pinned TDLib 1.8.69 PartsManager: 4000/8000 parts of 512 KiB.
        // FileLoaderUtils independently caps local uploads at 4000 MiB.
        Ok(if premium { 4000 } else { 2000 } * 1024 * 1024)
    }
    async fn init_hint(
        &self,
        chat: ChatId,
        cancel: &CancellationToken,
    ) -> Result<Option<RemoteMessage>, TelegramError> {
        let result=self.client.read(json!({"@type":"searchChatMessages","chat_id":chat.get(),"topic_id":null,
            "query":"VAULT_INIT","sender_id":null,"from_message_id":0,"offset":0,"limit":20,"filter":null}),cancel).await?;
        let values = result["messages"]
            .as_array()
            .ok_or(TelegramError::InvalidResponse)?;
        let mut candidate = None;
        for value in values {
            let message = parse_message(value, chat)?;
            if matches!(message.record()?, Some(Record::Init(_))) {
                if candidate.is_some() {
                    return Err(TelegramError::InvalidVault);
                }
                candidate = Some(message);
            }
        }
        // Incomplete search results are never treated as an authoritative marker.
        if integer(&result["next_from_message_id"])? != 0 {
            return Ok(None);
        }
        if let Some(hint) = candidate {
            let actual = self.message(chat, hint.id, cancel).await?;
            if actual.as_ref() != Some(&hint) {
                return Err(TelegramError::InvalidVault);
            }
            Ok(actual)
        } else {
            Ok(None)
        }
    }
    async fn pinned_message(
        &self,
        chat: ChatId,
        cancel: &CancellationToken,
    ) -> Result<Option<RemoteMessage>, TelegramError> {
        match self
            .client
            .read(
                json!({"@type":"getChatPinnedMessage","chat_id":chat.get()}),
                cancel,
            )
            .await
        {
            Ok(value) => parse_message(&value, chat).map(Some),
            Err(TelegramError::Request { code: 404 }) => Ok(None),
            Err(error) => Err(error),
        }
    }
    async fn audio_info(
        &self,
        chat: ChatId,
        id: MessageId,
        cancel: &CancellationToken,
    ) -> Result<Option<AudioInfo>, TelegramError> {
        let value = self
            .client
            .read(
                json!({"@type":"getMessage","chat_id":chat.get(),"message_id":id.get()}),
                cancel,
            )
            .await?;
        if parse_message(&value, chat)?.id != id {
            return Err(TelegramError::InvalidResponse);
        }
        parse_audio_info(&value["content"])
    }
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
        self.validate_upload(&path, cancel).await?;
        self.send_content(chat,json!({"@type":"inputMessageDocument","document":{"@type":"inputDocument","document":{"@type":"inputFileLocal","path":path.to_str().ok_or(TelegramError::InvalidInput)?},"thumbnail":null,"disable_content_type_detection":true},"caption":{"@type":"formattedText","text":caption,"entities":[]}}),cancel).await
    }
    async fn send_audio(
        &self,
        chat: ChatId,
        audio: AudioUpload,
        caption: String,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, TelegramError> {
        if !audio.path.is_absolute()
            || !audio.path.is_file()
            || audio.title.chars().count() > 64
            || audio.performer.chars().count() > 64
            || caption.encode_utf16().count() > 1024
        {
            return Err(TelegramError::InvalidInput);
        }
        let duration =
            i32::try_from(audio.duration_seconds).map_err(|_| TelegramError::InvalidInput)?;
        self.validate_upload(&audio.path, cancel).await?;
        self.send_content(
            chat,
            json!({
                "@type": "inputMessageAudio",
                "audio": {
                    "@type": "inputAudio",
                    "audio": {"@type": "inputFileLocal", "path": audio.path.to_str().ok_or(TelegramError::InvalidInput)?},
                    "album_cover_thumbnail": null,
                    "duration": duration,
                    "title": audio.title,
                    "performer": audio.performer,
                },
                "caption": {"@type": "formattedText", "text": caption, "entities": []},
            }),
            cancel,
        ).await
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
        // TDLib ends an outstanding synchronous download when another range for
        // that file is requested. Share a per-file gate across transport clones.
        let _download = self.downloads.acquire(file.file_id, cancel).await?;
        let lease = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(TelegramError::Cancelled),
            lease = self.storage.clone().read_owned() => lease,
        };
        let response=self.client.request(json!({"@type":"downloadFile","file_id":file.file_id,"priority":16,"offset":offset,"limit":limit,"synchronous":true}),Duration::from_secs(300),cancel).await;
        let value = match response {
            Ok(value) => value,
            Err(error) => {
                // Cancellation stops transport work as well; no shared library mutation.
                let _=self.client.call(json!({"@type":"cancelDownloadFile","file_id":file.file_id,"only_if_pending":false})).await;
                return Err(error);
            }
        };
        let mut range = downloaded_range(&value, &file, offset, limit)?;
        range.storage_lease = Some(Arc::new(lease));
        Ok(range)
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
        if properties["can_be_copied"] != true {
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
        // Subscribe before sending: the completion update can precede the
        // correlated response carrying the temporary message ID.
        let updates = self.client.subscribe();
        let response = self
            .client
            .request(
                json!({"@type":"forwardMessages","chat_id":recipient_chat.get(),"topic_id":null,"from_chat_id":source_chat.get(),"message_ids":[message_id.get()],"options":null,"send_copy":true,"remove_caption":true}),
                Duration::from_secs(60),
                cancel,
            )
            .await?;
        let messages = response["messages"]
            .as_array()
            .filter(|messages| messages.len() == 1)
            .ok_or(TelegramError::InvalidResponse)?;
        let forwarded = self
            .confirm_send(recipient_chat, messages[0].clone(), updates, cancel)
            .await?;
        if forwarded.file.is_none() {
            return Err(TelegramError::InvalidResponse);
        }
        Ok(forwarded)
    }
    async fn incoming_media(
        &self,
        source_chat: ChatId,
        message_id: MessageId,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, TelegramError> {
        let source = self
            .message(source_chat, message_id, cancel)
            .await?
            .ok_or(TelegramError::Request { code: 404 })?;
        if !matches!(
            source.content,
            MessageContent::Audio | MessageContent::Document
        ) || source.file.is_none()
            || source.record()?.is_some()
        {
            return Err(TelegramError::Unshareable);
        }
        let properties = self.client.read(
            json!({"@type":"getMessageProperties","chat_id":source_chat.get(),"message_id":message_id.get()}),
            cancel,
        ).await?;
        if properties["can_be_copied"] != true {
            return Err(TelegramError::Unshareable);
        }
        Ok(source)
    }
    async fn copy_media(
        &self,
        source_chat: ChatId,
        message_id: MessageId,
        target_chat: ChatId,
        caption: String,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, TelegramError> {
        if caption.encode_utf16().count() > 1024
            || !matches!(
                vault_protocol::classify_text(&caption),
                Ok(Some(Record::Media(_)))
            )
        {
            return Err(TelegramError::InvalidInput);
        }
        self.incoming_media(source_chat, message_id, cancel).await?;
        self.send_content(
            target_chat,
            json!({
                "@type": "inputMessageForwarded",
                "from_chat_id": source_chat.get(),
                "message_id": message_id.get(),
                "in_game_share": false,
                "replace_video_start_timestamp": false,
                "new_video_start_timestamp": 0,
                "copy_options": {
                    "@type": "messageCopyOptions",
                    "send_copy": true,
                    "replace_caption": true,
                    "new_caption": {"@type":"formattedText","text":caption,"entities":[]},
                    "new_show_caption_above_media": false,
                },
            }),
            cancel,
        )
        .await
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
fn parse_audio_info(content: &Value) -> Result<Option<AudioInfo>, TelegramError> {
    if content["@type"] != "messageAudio" {
        return Ok(None);
    }
    let audio = &content["audio"];
    let duration =
        u64::try_from(integer(&audio["duration"])?).map_err(|_| TelegramError::InvalidResponse)?;
    let file =
        |raw: &Value, name: String, mime_type: String| -> Result<RemoteFile, TelegramError> {
            let file_id =
                i32::try_from(integer(&raw["id"])?).map_err(|_| TelegramError::InvalidResponse)?;
            let size = integer(&raw["size"])?;
            if file_id <= 0 || size < 0 {
                return Err(TelegramError::InvalidResponse);
            }
            Ok(RemoteFile {
                file_id,
                size,
                name,
                mime_type,
            })
        };
    let thumbnail = if audio["album_cover_thumbnail"].is_null() {
        None
    } else {
        let mime = match audio["album_cover_thumbnail"]["format"]["@type"].as_str() {
            Some("thumbnailFormatJpeg") => "image/jpeg",
            Some("thumbnailFormatPng") => "image/png",
            Some("thumbnailFormatWebp") => "image/webp",
            _ => "application/octet-stream",
        };
        Some(file(
            &audio["album_cover_thumbnail"]["file"],
            String::new(),
            mime.into(),
        )?)
    };
    Ok(Some(AudioInfo {
        title: audio["title"]
            .as_str()
            .ok_or(TelegramError::InvalidResponse)?
            .into(),
        performer: audio["performer"]
            .as_str()
            .ok_or(TelegramError::InvalidResponse)?
            .into(),
        duration_ms: duration
            .checked_mul(1000)
            .ok_or(TelegramError::InvalidResponse)?,
        file: file(
            &audio["audio"],
            audio["file_name"].as_str().unwrap_or("").into(),
            audio["mime_type"].as_str().unwrap_or("").into(),
        )?,
        thumbnail,
    }))
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
        content: match content["@type"].as_str() {
            Some("messageText") => MessageContent::Text,
            Some("messageAudio") => MessageContent::Audio,
            Some("messageDocument") => MessageContent::Document,
            _ => MessageContent::Other,
        },
        sent_at_unix: integer(&value["date"])
            .ok()
            .and_then(|n| u64::try_from(n).ok())
            .filter(|n| *n > 0),
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
            if !value["message"]["sending_state"].is_null()
                || !value["message"]["scheduling_state"].is_null()
            {
                None
            } else {
                let id = chat(&value["message"]["chat_id"])?;
                Some(Update::New(parse_message(&value["message"], id)?))
            }
        }
        Some("updateMessageContent" | "updateMessageEdited") => {
            let id = integer(&value["message_id"])?;
            // TDLib also updates pending/local and scheduled messages. Those
            // IDs never enter the vault log and are not a connection failure.
            validate_server_id(id)
                .ok()
                .map(|message_id| {
                    chat(&value["chat_id"]).map(|chat_id| Update::Changed {
                        chat_id,
                        message_id,
                    })
                })
                .transpose()?
        }
        Some("updateDeleteMessages") if value["is_permanent"] == true => {
            let ids = value["message_ids"]
                .as_array()
                .ok_or(TelegramError::InvalidResponse)?
                .iter()
                .map(integer)
                .collect::<Result<Vec<_>, _>>()?;
            let message_ids: Vec<_> = ids
                .into_iter()
                .filter_map(|id| validate_server_id(id).ok())
                .collect();
            if message_ids.is_empty() {
                None
            } else {
                Some(Update::Deleted {
                    chat_id: chat(&value["chat_id"])?,
                    message_ids,
                })
            }
        }
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

fn downloaded_range(
    value: &Value,
    file: &RemoteFile,
    offset: i64,
    limit: i64,
) -> Result<DownloadedRange, TelegramError> {
    if integer(&value["id"])? != i64::from(file.file_id) {
        return Err(TelegramError::InvalidResponse);
    }
    let local = &value["local"];
    let complete = local["is_downloading_completed"] == true;
    let start = integer(&local["download_offset"])?;
    let prefix = integer(&local["downloaded_prefix_size"])?;
    let size = integer(&value["size"])?;
    if start < 0 || prefix < 0 || size < 0 || size != file.size || offset > size {
        return Err(TelegramError::InvalidResponse);
    }
    let available = if complete {
        size - offset
    } else {
        let end = start
            .checked_add(prefix)
            .filter(|end| *end <= size)
            .ok_or(TelegramError::InvalidResponse)?;
        if start > offset || end < offset {
            return Err(TelegramError::Timeout);
        }
        end - offset
    };
    if available < 0 || (limit == 0 && !complete) || (limit > 0 && available < limit && !complete) {
        return Err(TelegramError::Timeout);
    }
    Ok(DownloadedRange {
        storage_lease: None,
        path: PathBuf::from(
            local["path"]
                .as_str()
                .ok_or(TelegramError::InvalidResponse)?,
        ),
        // download_offset locates the ready prefix, not the beginning of local
        // file contents: TDLib 1.8.69 FileDownloader writes with pwrite(part.offset).
        file_offset: 0,
        offset,
        available,
        complete,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod download_tests {
    #[test]
    fn pending_message_changes_do_not_report_a_disconnection() {
        use serde_json::json;
        for kind in ["updateMessageContent", "updateMessageEdited"] {
            assert!(
                super::parse_update(&json!({"@type":kind,"chat_id":-100,"message_id":10485761}))
                    .unwrap()
                    .is_none()
            );
        }
        let deletion = super::parse_update(&json!({"@type":"updateDeleteMessages", "chat_id":-100,
            "message_ids":[10485761,10485760],"is_permanent":true}))
        .unwrap();
        assert!(
            matches!(deletion, Some(super::Update::Deleted {message_ids,..}) if message_ids.len()==1 && message_ids[0].get()==10485760)
        );
        assert!(
            super::parse_update(
                &json!({"@type":"updateMessageContent","chat_id":-100,"message_id":1.5})
            )
            .is_err()
        );
    }
    use super::*;
    use std::io::{Read, Seek, SeekFrom, Write};

    fn remote() -> RemoteFile {
        RemoteFile {
            file_id: 5,
            size: 8192,
            name: "test.wav".into(),
            mime_type: "audio/wav".into(),
        }
    }
    fn response(path: &std::path::Path, complete: bool) -> Value {
        json!({"@type":"file", "id":5, "size":8192, "local":{
            "path":path, "download_offset":4096, "downloaded_prefix_size":2048,
            "is_downloading_completed":complete
        }})
    }
    #[test]
    fn nonzero_tdlib_download_offset_does_not_shift_local_file_origin() {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.as_file_mut().seek(SeekFrom::Start(4096)).unwrap();
        file.write_all(&[7u8; 2048]).unwrap();
        let range = downloaded_range(&response(file.path(), false), &remote(), 4128, 64).unwrap();
        assert_eq!(range.file_offset, 0);
        let mut input = std::fs::File::open(range.path).unwrap();
        input
            .seek(SeekFrom::Start((range.offset - range.file_offset) as u64))
            .unwrap();
        let mut bytes = [0u8; 64];
        input.read_exact(&mut bytes).unwrap();
        assert_eq!(bytes, [7u8; 64]);
    }
    #[test]
    fn complete_file_covers_ranges_before_the_last_download_offset() {
        let range = downloaded_range(
            &response(std::path::Path::new("file"), true),
            &remote(),
            0,
            64,
        )
        .unwrap();
        assert_eq!(range.file_offset, 0);
        assert_eq!(range.available, 8192);
    }
    #[tokio::test]
    async fn same_file_ranges_serialize_and_waiting_is_cancellable() {
        let locks = DownloadLocks::default();
        let first = locks.acquire(5, &CancellationToken::new()).await.unwrap();
        let other_file = locks.acquire(6, &CancellationToken::new()).await.unwrap();
        let cancel = CancellationToken::new();
        let waiter = locks.acquire(5, &cancel);
        tokio::pin!(waiter);
        assert!(matches!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(waiter.as_mut().poll(cx))).await,
            std::task::Poll::Pending
        ));
        cancel.cancel();
        assert!(matches!(waiter.await, Err(TelegramError::Cancelled)));
        drop(first);
        drop(other_file);
        assert!(locks.acquire(5, &CancellationToken::new()).await.is_ok());
    }
    #[test]
    fn incomplete_or_malformed_ranges_never_expose_unready_bytes() {
        let path = std::path::Path::new("file");
        assert!(matches!(
            downloaded_range(&response(path, false), &remote(), 0, 64),
            Err(TelegramError::Timeout)
        ));
        assert!(matches!(
            downloaded_range(&response(path, false), &remote(), 6100, 64),
            Err(TelegramError::Timeout)
        ));
        for (field, value) in [
            ("download_offset", -1),
            ("downloaded_prefix_size", -1),
            ("downloaded_prefix_size", i64::MAX),
        ] {
            let mut input = response(path, false);
            input["local"][field] = json!(value);
            assert!(matches!(
                downloaded_range(&input, &remote(), 4128, 64),
                Err(TelegramError::InvalidResponse)
            ));
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod metadata_tests {
    use super::*;
    #[test]
    fn audio_preview_reads_metadata_and_thumbnail_without_download() {
        let content = json!({"@type":"messageAudio","audio":{"duration":180,"title":"Song","performer":"Artist",
            "file_name":"song.mp3","mime_type":"audio/mpeg","audio":{"id":8,"size":5000000},
            "album_cover_thumbnail":{"format":{"@type":"thumbnailFormatJpeg"},"file":{"id":9,"size":1200}}}});
        let result = parse_audio_info(&content).unwrap().unwrap();
        assert_eq!(result.title, "Song");
        assert_eq!(result.duration_ms, 180000);
        assert_eq!(result.file.size, 5000000);
        assert_eq!(result.thumbnail.unwrap().size, 1200);
        let mut invalid = content;
        invalid["audio"]["duration"] = json!(-1);
        assert!(parse_audio_info(&invalid).is_err());
        assert!(
            parse_audio_info(&json!({"@type":"messageDocument"}))
                .unwrap()
                .is_none()
        );
    }
}
