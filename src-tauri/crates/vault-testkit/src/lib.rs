//! Explicit simulation/test transport. Never selected by the native application.
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use vault_domain::*;
use vault_protocol::Record;
use vault_telegram::*;

#[derive(Debug, Clone, Default)]
pub struct Faults {
    pub upload_limit_bytes: Option<u64>,
    /// Definite remote rejection: no message was created, retry is safe.
    pub reject_document: bool,
    pub prohibit_copy: bool,
    /// Simulates an ambiguous upload timeout with no message visible yet.
    pub document_timeout_before_send: bool,
    pub fail_after_delete: bool,
    /// Inject a concurrent writer between destructive batches.
    pub text_after_delete: Option<String>,
    pub remove_after_delete: Option<MessageId>,
    pub offline: bool,
    pub lost_ack: bool,
    pub send_error_before: Option<i64>,
    pub send_error_after: Option<i64>,
    pub drop_updates: bool,
    pub duplicate_updates: bool,
    pub delay_updates: bool,
    pub history_page_size: Option<usize>,
    /// Delete after capturing the next history response, before its caller replays it.
    pub remove_after_history: Option<MessageId>,
    pub flood_wait: Option<u64>,
    /// Fail before the Nth subsequent call (1 is next); may model a process crash.
    pub fail_call: Option<usize>,
    pub download_delay_ms: u64,
}
struct Server {
    channel: ChannelInfo,
    private: bool,
    next: i64,
    messages: BTreeMap<MessageId, RemoteMessage>,
    files: BTreeMap<i32, Vec<u8>>,
    audio: BTreeMap<i32, AudioInfo>,
    next_file: i32,
    download_calls: usize,
    history_calls: usize,
    message_calls: usize,
    delete_calls: usize,
    subscribers: Vec<broadcast::Sender<Update>>,
    delayed: VecDeque<Update>,
    shared: Vec<(ChatId, MessageId, ChatId)>,
    faults: Faults,
}
#[derive(Clone)]
pub struct FakeServer(Arc<Mutex<Server>>);
#[derive(Clone)]
pub struct FakeTelegram {
    server: FakeServer,
    updates: broadcast::Sender<Update>,
    downloads: Arc<tempfile::TempDir>,
}
impl FakeServer {
    pub fn new(chat: ChatId, owner: UserId) -> Self {
        Self(Arc::new(Mutex::new(Server {
            channel: ChannelInfo {
                chat_id: chat,
                owner_id: owner,
                title: "kiyora test vault".into(),
            },
            private: true,
            next: 7,
            messages: BTreeMap::new(),
            files: BTreeMap::new(),
            audio: BTreeMap::new(),
            next_file: 1,
            download_calls: 0,
            history_calls: 0,
            message_calls: 0,
            delete_calls: 0,
            subscribers: Vec::new(),
            delayed: VecDeque::new(),
            shared: Vec::new(),
            faults: Faults::default(),
        })))
    }
    fn lock(&self) -> Result<MutexGuard<'_, Server>, TelegramError> {
        self.0.lock().map_err(|_| TelegramError::InvalidResponse)
    }
    pub fn client(&self) -> Result<FakeTelegram, TelegramError> {
        let (updates, _) = broadcast::channel(2048);
        self.lock()?.subscribers.push(updates.clone());
        Ok(FakeTelegram {
            server: self.clone(),
            updates,
            downloads: Arc::new(tempfile::tempdir().map_err(|_| TelegramError::LocalFile)?),
        })
    }
    pub fn set_faults(&self, faults: Faults) -> Result<(), TelegramError> {
        self.lock()?.faults = faults;
        Ok(())
    }
    pub fn rename(&self, title: String) -> Result<(), TelegramError> {
        self.lock()?.channel.title = title;
        Ok(())
    }
    pub fn set_private(&self, private: bool) -> Result<(), TelegramError> {
        self.lock()?.private = private;
        Ok(())
    }
    pub fn messages(&self) -> Result<Vec<RemoteMessage>, TelegramError> {
        Ok(self.lock()?.messages.values().cloned().collect())
    }
    pub fn add_incoming_document(
        &self,
        name: String,
        mime_type: String,
        bytes: Vec<u8>,
    ) -> Result<RemoteMessage, TelegramError> {
        let mut server = self.lock()?;
        let file_id = server.next_file;
        server.next_file = server
            .next_file
            .checked_add(1)
            .ok_or(TelegramError::InvalidResponse)?;
        let file = RemoteFile {
            file_id,
            size: i64::try_from(bytes.len()).map_err(|_| TelegramError::InvalidInput)?,
            name,
            mime_type,
        };
        server.files.insert(file_id, bytes);
        let chat_id = server.channel.chat_id;
        let mut message = server.append(chat_id, "incoming audio".into(), Some(file))?;
        message.trusted_control = false;
        server.messages.insert(message.id, message.clone());
        Ok(message)
    }
    pub fn shared_messages(&self) -> Result<Vec<(ChatId, MessageId, ChatId)>, TelegramError> {
        Ok(self.lock()?.shared.clone())
    }
    pub fn add_incoming_audio(
        &self,
        name: String,
        bytes: Vec<u8>,
        title: String,
        performer: String,
        duration_ms: u64,
    ) -> Result<RemoteMessage, TelegramError> {
        let mut message = self.add_incoming_document(name, "audio/mpeg".into(), bytes)?;
        message.content = MessageContent::Audio;
        let mut server = self.lock()?;
        let file = message.file.clone().ok_or(TelegramError::InvalidResponse)?;
        server.audio.insert(
            file.file_id,
            AudioInfo {
                title,
                performer,
                duration_ms,
                file,
                thumbnail: None,
            },
        );
        server.messages.insert(message.id, message.clone());
        Ok(message)
    }
    pub fn download_calls(&self) -> Result<usize, TelegramError> {
        Ok(self.lock()?.download_calls)
    }
    pub fn set_audio_thumbnail(
        &self,
        message_id: MessageId,
        bytes: Vec<u8>,
        mime: String,
    ) -> Result<(), TelegramError> {
        let mut server = self.lock()?;
        let media = server
            .messages
            .get(&message_id)
            .and_then(|message| message.file.as_ref())
            .ok_or(TelegramError::InvalidInput)?
            .file_id;
        let file_id = server.next_file;
        server.next_file += 1;
        let thumbnail = RemoteFile {
            file_id,
            size: bytes.len() as i64,
            name: "cover.jpg".into(),
            mime_type: mime,
        };
        server
            .audio
            .get_mut(&media)
            .ok_or(TelegramError::InvalidInput)?
            .thumbnail = Some(thumbnail);
        server.files.insert(file_id, bytes);
        Ok(())
    }
    pub fn history_calls(&self) -> Result<usize, TelegramError> {
        Ok(self.lock()?.history_calls)
    }
    pub fn message_calls(&self) -> Result<usize, TelegramError> {
        Ok(self.lock()?.message_calls)
    }
    pub fn delete_calls(&self) -> Result<usize, TelegramError> {
        Ok(self.lock()?.delete_calls)
    }
    pub fn emit_update(&self, update: Update) -> Result<(), TelegramError> {
        self.lock()?.emit(update);
        Ok(())
    }
    pub fn corrupt_file(&self, file_id: i32) -> Result<(), TelegramError> {
        self.lock()?.files.insert(file_id, b"corrupt".to_vec());
        Ok(())
    }
    pub fn manual_delete(&self, id: MessageId) -> Result<(), TelegramError> {
        let mut server = self.lock()?;
        server.messages.remove(&id);
        let chat_id = server.channel.chat_id;
        server.emit(Update::Deleted {
            chat_id,
            message_ids: vec![id],
        });
        Ok(())
    }
    pub fn flush_updates_reversed(&self) -> Result<(), TelegramError> {
        let mut server = self.lock()?;
        while let Some(update) = server.delayed.pop_back() {
            for subscriber in &server.subscribers {
                let _ = subscriber.send(update.clone());
            }
        }
        Ok(())
    }
}
impl Server {
    fn gate(&mut self, cancel: &CancellationToken) -> Result<(), TelegramError> {
        if cancel.is_cancelled() {
            return Err(TelegramError::Cancelled);
        }
        if self.faults.offline {
            return Err(TelegramError::Timeout);
        }
        if let Some(wait) = self.faults.flood_wait.take() {
            return Err(TelegramError::RateLimited {
                retry_after_seconds: wait,
            });
        }
        if let Some(left) = self.faults.fail_call.as_mut() {
            *left = left.saturating_sub(1);
            if *left == 0 {
                self.faults.fail_call = None;
                return Err(TelegramError::Timeout);
            }
        }
        Ok(())
    }
    fn emit(&mut self, update: Update) {
        if self.faults.drop_updates {
            return;
        }
        if self.faults.delay_updates {
            self.delayed.push_back(update);
            return;
        }
        for sender in &self.subscribers {
            let _ = sender.send(update.clone());
            if self.faults.duplicate_updates {
                let _ = sender.send(update.clone());
            }
        }
    }
    fn append(
        &mut self,
        chat: ChatId,
        text: String,
        file: Option<RemoteFile>,
    ) -> Result<RemoteMessage, TelegramError> {
        if chat != self.channel.chat_id {
            return Err(TelegramError::UnsafeChannel);
        }
        if let Some(code) = self.faults.send_error_before.take() {
            return Err(TelegramError::Request { code });
        }
        self.next = self
            .next
            .checked_add(17)
            .ok_or(TelegramError::InvalidResponse)?;
        let id = MessageId::new(self.next).map_err(|_| TelegramError::InvalidResponse)?;
        let message = RemoteMessage {
            content: if file
                .as_ref()
                .is_some_and(|file| self.audio.contains_key(&file.file_id))
            {
                MessageContent::Audio
            } else if file.is_some() {
                MessageContent::Document
            } else {
                MessageContent::Text
            },
            sent_at_unix: Some(1_792_713_600),
            chat_id: chat,
            id,
            text,
            file,
            trusted_control: true,
        };
        self.messages.insert(id, message.clone());
        self.emit(Update::New(message.clone()));
        if let Some(code) = self.faults.send_error_after.take() {
            return Err(TelegramError::Request { code });
        }
        if std::mem::take(&mut self.faults.lost_ack) {
            return Err(TelegramError::Timeout);
        }
        Ok(message)
    }
}
impl FakeTelegram {
    async fn send_file(
        &self,
        chat: ChatId,
        path: PathBuf,
        caption: String,
        audio: Option<AudioUpload>,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, TelegramError> {
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|_| TelegramError::LocalFile)?;
        let mut server = self.server.lock()?;
        server.gate(cancel)?;
        if std::mem::take(&mut server.faults.reject_document) {
            return Err(TelegramError::Request { code: 400 });
        }
        if std::mem::take(&mut server.faults.document_timeout_before_send) {
            return Err(TelegramError::Timeout);
        }
        let file_id = server.next_file;
        server.next_file += 1;
        let file = RemoteFile {
            file_id,
            size: i64::try_from(bytes.len()).map_err(|_| TelegramError::LocalFile)?,
            name: path
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
            mime_type: "application/zstd".into(),
        };
        if let Some(audio) = audio {
            server.audio.insert(
                file_id,
                AudioInfo {
                    title: audio.title,
                    performer: audio.performer,
                    duration_ms: u64::from(audio.duration_seconds) * 1000,
                    file: file.clone(),
                    thumbnail: None,
                },
            );
        }
        server.files.insert(file_id, bytes);
        server.append(chat, caption, Some(file))
    }
}
impl Telegram for FakeTelegram {
    async fn upload_limit(&self, cancel: &CancellationToken) -> Result<u64, TelegramError> {
        let mut server = self.server.lock()?;
        server.gate(cancel)?;
        Ok(server
            .faults
            .upload_limit_bytes
            .unwrap_or(2000 * 1024 * 1024))
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
        let mut server = self.server.lock()?;
        server.gate(cancel)?;
        if !server.private || server.channel.chat_id != chat || server.channel.owner_id != owner {
            return Err(TelegramError::UnsafeChannel);
        }
        Ok(server.channel.clone())
    }
    async fn history(
        &self,
        chat: ChatId,
        before: Option<MessageId>,
        limit: u8,
        cancel: &CancellationToken,
    ) -> Result<Vec<RemoteMessage>, TelegramError> {
        let mut server = self.server.lock()?;
        server.history_calls += 1;
        server.gate(cancel)?;
        if server.channel.chat_id != chat {
            return Err(TelegramError::UnsafeChannel);
        }
        let limit = server
            .faults
            .history_page_size
            .unwrap_or(usize::from(limit))
            .max(1)
            .min(usize::from(limit));
        let page = server
            .messages
            .values()
            .rev()
            .filter(|m| before.is_none_or(|id| m.id < id))
            .take(limit)
            .cloned()
            .collect();
        if let Some(id) = server.faults.remove_after_history.take() {
            server.messages.remove(&id);
            server.emit(Update::Deleted {
                chat_id: chat,
                message_ids: vec![id],
            });
        }
        Ok(page)
    }
    async fn message(
        &self,
        chat: ChatId,
        id: MessageId,
        cancel: &CancellationToken,
    ) -> Result<Option<RemoteMessage>, TelegramError> {
        let mut server = self.server.lock()?;
        server.message_calls += 1;
        server.gate(cancel)?;
        if server.channel.chat_id != chat {
            return Err(TelegramError::UnsafeChannel);
        }
        Ok(server.messages.get(&id).cloned())
    }
    async fn send_text(
        &self,
        chat: ChatId,
        text: String,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, TelegramError> {
        let mut server = self.server.lock()?;
        server.gate(cancel)?;
        server.append(chat, text, None)
    }
    async fn send_document(
        &self,
        chat: ChatId,
        path: PathBuf,
        caption: String,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, TelegramError> {
        self.send_file(chat, path, caption, None, cancel).await
    }
    async fn send_audio(
        &self,
        chat: ChatId,
        audio: AudioUpload,
        caption: String,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, TelegramError> {
        self.send_file(chat, audio.path.clone(), caption, Some(audio), cancel)
            .await
    }
    async fn audio_info(
        &self,
        chat: ChatId,
        id: MessageId,
        cancel: &CancellationToken,
    ) -> Result<Option<AudioInfo>, TelegramError> {
        let Some(message) = self.message(chat, id, cancel).await? else {
            return Ok(None);
        };
        let server = self.server.lock()?;
        Ok(message
            .file
            .and_then(|file| server.audio.get(&file.file_id).cloned()))
    }
    async fn download(
        &self,
        file: RemoteFile,
        offset: i64,
        limit: i64,
        cancel: &CancellationToken,
    ) -> Result<DownloadedRange, TelegramError> {
        let delay = {
            let mut server = self.server.lock()?;
            server.gate(cancel)?;
            server.download_calls = server.download_calls.saturating_add(1);
            server.faults.download_delay_ms
        };
        if delay > 0 {
            tokio::select! {
                _ = cancel.cancelled() => return Err(TelegramError::Cancelled),
                _ = tokio::time::sleep(std::time::Duration::from_millis(delay)) => {}
            }
        }
        let bytes = {
            let mut server = self.server.lock()?;
            server.gate(cancel)?;
            server
                .files
                .get(&file.file_id)
                .cloned()
                .ok_or(TelegramError::Request { code: 404 })?
        };
        let size = i64::try_from(bytes.len()).map_err(|_| TelegramError::LocalFile)?;
        if offset < 0 || offset > size || limit < 0 {
            return Err(TelegramError::InvalidInput);
        }
        let path =
            self.downloads
                .path()
                .join(format!("{}-{}.bin", file.file_id, uuid::Uuid::new_v4()));
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|_| TelegramError::LocalFile)?;
        Ok(DownloadedRange {
            storage_lease: None,
            path,
            file_offset: 0,
            offset,
            available: if limit == 0 {
                size - offset
            } else {
                limit.min(size - offset)
            },
            complete: limit == 0 || offset + limit >= size,
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
        let mut server = self.server.lock()?;
        server.gate(cancel)?;
        if source_chat == recipient_chat || source_chat != server.channel.chat_id {
            return Err(TelegramError::InvalidInput);
        }
        let source = server
            .messages
            .get(&message_id)
            .cloned()
            .ok_or(TelegramError::Request { code: 404 })?;
        let record = source.record()?;
        if source.file.is_none()
            || record.is_some_and(
                |record| !matches!(record, Record::Media(media) if media.vault_id == vault_id),
            )
        {
            return Err(TelegramError::Unshareable);
        }
        server
            .shared
            .push((source_chat, message_id, recipient_chat));
        server.next = server
            .next
            .checked_add(17)
            .ok_or(TelegramError::InvalidResponse)?;
        let mut forwarded = source;
        forwarded.chat_id = recipient_chat;
        forwarded.id = MessageId::new(server.next).map_err(|_| TelegramError::InvalidResponse)?;
        forwarded.trusted_control = false;
        forwarded.text.clear();
        if std::mem::take(&mut server.faults.lost_ack) {
            return Err(TelegramError::Timeout);
        }
        Ok(forwarded)
    }
    async fn incoming_media(
        &self,
        chat: ChatId,
        id: MessageId,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, TelegramError> {
        let source = self
            .message(chat, id, cancel)
            .await?
            .ok_or(TelegramError::Request { code: 404 })?;
        if self.server.lock()?.faults.prohibit_copy
            || !matches!(
                source.content,
                MessageContent::Audio | MessageContent::Document
            )
            || source.file.is_none()
            || source.record()?.is_some()
        {
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
        let mut server = self.server.lock()?;
        server.gate(cancel)?;
        if server.faults.prohibit_copy {
            return Err(TelegramError::Unshareable);
        }
        if std::mem::take(&mut server.faults.reject_document) {
            return Err(TelegramError::Request { code: 400 });
        }
        if std::mem::take(&mut server.faults.document_timeout_before_send) {
            return Err(TelegramError::Timeout);
        }
        let source = server
            .messages
            .get(&message_id)
            .cloned()
            .filter(|source| source.chat_id == source_chat)
            .ok_or(TelegramError::Request { code: 404 })?;
        if !matches!(
            source.content,
            MessageContent::Audio | MessageContent::Document
        ) || source.file.is_none()
            || source.record()?.is_some()
        {
            return Err(TelegramError::Unshareable);
        }
        server.append(target_chat, caption, source.file)
    }
    async fn delete_messages(
        &self,
        chat: ChatId,
        ids: Vec<MessageId>,
        cancel: &CancellationToken,
    ) -> Result<(), TelegramError> {
        let mut server = self.server.lock()?;
        server.delete_calls += 1;
        server.gate(cancel)?;
        if chat != server.channel.chat_id {
            return Err(TelegramError::UnsafeChannel);
        }
        for id in &ids {
            server.messages.remove(id);
        }
        server.emit(Update::Deleted {
            chat_id: chat,
            message_ids: ids,
        });
        if let Some(text) = server.faults.text_after_delete.take() {
            server.append(chat, text, None)?;
        }
        if let Some(id) = server.faults.remove_after_delete.take() {
            server.messages.remove(&id);
            server.emit(Update::Deleted {
                chat_id: chat,
                message_ids: vec![id],
            });
        }
        if std::mem::take(&mut server.faults.fail_after_delete) {
            return Err(TelegramError::Timeout);
        }
        Ok(())
    }
}
