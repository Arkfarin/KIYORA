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
    pub fail_after_delete: bool,
    pub offline: bool,
    pub lost_ack: bool,
    pub drop_updates: bool,
    pub duplicate_updates: bool,
    pub delay_updates: bool,
    pub history_page_size: Option<usize>,
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
    next_file: i32,
    download_calls: usize,
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
            next_file: 1,
            download_calls: 0,
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
    pub fn download_calls(&self) -> Result<usize, TelegramError> {
        Ok(self.lock()?.download_calls)
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
        self.next = self
            .next
            .checked_add(17)
            .ok_or(TelegramError::InvalidResponse)?;
        let id = MessageId::new(self.next).map_err(|_| TelegramError::InvalidResponse)?;
        let message = RemoteMessage {
            chat_id: chat,
            id,
            text,
            file,
            trusted_control: true,
        };
        self.messages.insert(id, message.clone());
        self.emit(Update::New(message.clone()));
        if std::mem::take(&mut self.faults.lost_ack) {
            return Err(TelegramError::Timeout);
        }
        Ok(message)
    }
}
impl Telegram for FakeTelegram {
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
        Ok(server
            .messages
            .values()
            .rev()
            .filter(|m| before.is_none_or(|id| m.id < id))
            .take(limit)
            .cloned()
            .collect())
    }
    async fn message(
        &self,
        chat: ChatId,
        id: MessageId,
        cancel: &CancellationToken,
    ) -> Result<Option<RemoteMessage>, TelegramError> {
        let mut server = self.server.lock()?;
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
        let bytes = tokio::fs::read(&path)
            .await
            .map_err(|_| TelegramError::LocalFile)?;
        let mut server = self.server.lock()?;
        server.gate(cancel)?;
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
        server.files.insert(file_id, bytes);
        server.append(chat, caption, Some(file))
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
        Ok(forwarded)
    }
    async fn delete_messages(
        &self,
        chat: ChatId,
        ids: Vec<MessageId>,
        cancel: &CancellationToken,
    ) -> Result<(), TelegramError> {
        let mut server = self.server.lock()?;
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
        if std::mem::take(&mut server.faults.fail_after_delete) {
            return Err(TelegramError::Timeout);
        }
        Ok(())
    }
}
