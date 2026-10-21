use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_domain::{ChatId, MessageId, UserId};
use vault_protocol::{Record, VaultInit};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultCandidate {
    pub channel: ChannelInfo,
    pub init: VaultInit,
    pub init_message_id: MessageId,
}

/// Bounded reverse scan that never treats short pages or numerical gaps as EOF.
pub async fn scan_history<T: Telegram>(
    transport: &T,
    chat: ChatId,
    after: i64,
    through: Option<MessageId>,
    max_records: usize,
    cancel: &CancellationToken,
) -> Result<Vec<RemoteMessage>, TelegramError> {
    let mut before = None;
    let mut records = BTreeMap::new();
    loop {
        if cancel.is_cancelled() {
            return Err(TelegramError::Cancelled);
        }
        let page = transport.history(chat, before, 100, cancel).await?;
        if page.is_empty() {
            break;
        }
        let next = page
            .iter()
            .map(|m| m.id)
            .min()
            .ok_or(TelegramError::IncompleteHistory)?;
        if before.is_some_and(|last| next >= last) {
            return Err(TelegramError::IncompleteHistory);
        }
        for message in page {
            if message.chat_id != chat || before.is_some_and(|last| message.id >= last) {
                return Err(TelegramError::IncompleteHistory);
            }
            if message.id.get() > after && through.is_none_or(|end| message.id <= end) {
                if let Some(old) = records.insert(message.id, message.clone())
                    && old != message
                {
                    return Err(TelegramError::IncompleteHistory);
                }
                if records.len() > max_records {
                    return Err(TelegramError::IncompleteHistory);
                }
            }
        }
        if next.get() <= after {
            break;
        }
        before = Some(next);
    }
    Ok(records.into_values().collect())
}

pub async fn inspect_vault<T: Telegram>(
    transport: &T,
    chat: ChatId,
    owner: UserId,
    expected: Option<Uuid>,
    cancel: &CancellationToken,
) -> Result<VaultCandidate, TelegramError> {
    let channel = transport.verify_channel(chat, owner, cancel).await?;
    let messages = scan_history(transport, chat, 0, None, 1_000_000, cancel).await?;
    let mut init = None;
    let mut vault_ids = BTreeSet::new();
    for message in messages {
        if let Some(record) = message.record()? {
            vault_ids.insert(record.vault_id());
            if expected.is_some_and(|id| record.vault_id() != id) {
                return Err(TelegramError::InvalidVault);
            }
            if let Record::Init(marker) = record {
                if marker.owner_telegram_user_id != owner || init.is_some() {
                    return Err(TelegramError::InvalidVault);
                }
                init = Some((message.id, marker));
            }
        }
    }
    let (init_message_id, init) = init.ok_or(TelegramError::InvalidVault)?;
    if vault_ids.len() != 1 {
        return Err(TelegramError::InvalidVault);
    }
    Ok(VaultCandidate {
        channel,
        init,
        init_message_id,
    })
}

impl TdTransport {
    /// Enumerates loaded main and archived chats, independent of titles. A failed
    /// server load remains an error, never a reason to create a replacement vault.
    pub async fn discover(
        &self,
        owner: UserId,
        cancel: &CancellationToken,
    ) -> Result<Vec<VaultCandidate>, TelegramError> {
        let mut chats = BTreeSet::new();
        for kind in ["chatListMain", "chatListArchive"] {
            let mut exhausted = false;
            for _ in 0..1000 {
                match self
                    .client()
                    .request(
                        json!({"@type":"loadChats","chat_list":{"@type":kind},"limit":100}),
                        std::time::Duration::from_secs(30),
                        cancel,
                    )
                    .await
                {
                    Ok(_) => {}
                    Err(TelegramError::Request { code: 404 }) => {
                        exhausted = true;
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
            if !exhausted {
                return Err(TelegramError::IncompleteHistory);
            }
            let result = self
                .client()
                .read(
                    json!({"@type":"getChats","chat_list":{"@type":kind},"limit":100_000}),
                    cancel,
                )
                .await?;
            for value in result["chat_ids"]
                .as_array()
                .ok_or(TelegramError::InvalidResponse)?
            {
                chats.insert(
                    ChatId::new(integer(value)?).map_err(|_| TelegramError::InvalidResponse)?,
                );
            }
        }
        let mut candidates = Vec::new();
        for chat in chats {
            // Non-vault/non-owned chats are expected during broad discovery. Network
            // failures are propagated instead of silently implying no vault exists.
            match inspect_vault(self, chat, owner, None, cancel).await {
                Ok(candidate) => candidates.push(candidate),
                Err(TelegramError::UnsafeChannel | TelegramError::InvalidVault) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(candidates)
    }
    /// Only called for an explicit create intent. The caller journals the returned
    /// chat before publishing INIT and resumes that chat after interruption.
    pub async fn create_channel(
        &self,
        owner: UserId,
        cancel: &CancellationToken,
    ) -> Result<ChannelInfo, TelegramError> {
        let result=self.client().request(json!({"@type":"createNewSupergroupChat","title":format!("kiyora.{owner}"),"is_forum":false,"is_channel":true,"description":"kiyora personal music vault","location":null,"message_auto_delete_time":0,"for_import":false}),std::time::Duration::from_secs(60),cancel).await?;
        let chat =
            ChatId::new(integer(&result["id"])?).map_err(|_| TelegramError::InvalidResponse)?;
        self.verify_channel(chat, owner, cancel).await
    }
}
