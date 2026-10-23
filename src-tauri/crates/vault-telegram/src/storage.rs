//! Local TDLib storage maintenance. Never deletes Telegram messages.
use crate::{TdTransport, TelegramError};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TdStorageStats {
    pub files_bytes: u64,
    pub files: u64,
    pub database_bytes: u64,
    pub language_pack_bytes: u64,
    pub log_bytes: u64,
}

impl TdTransport {
    pub async fn storage_stats(
        &self,
        cancel: &CancellationToken,
    ) -> Result<TdStorageStats, TelegramError> {
        let value = self
            .client()
            .request(
                json!({"@type":"getStorageStatisticsFast"}),
                Duration::from_secs(30),
                cancel,
            )
            .await?;
        let number = |key: &str| {
            value[key]
                .as_u64()
                .or_else(|| value[key].as_str()?.parse().ok())
                .ok_or(TelegramError::InvalidResponse)
        };
        if value["@type"] != "storageStatisticsFast" {
            return Err(TelegramError::InvalidResponse);
        }
        Ok(TdStorageStats {
            files_bytes: number("files_size")?,
            files: number("file_count")?,
            database_bytes: number("database_size")?,
            language_pack_bytes: number("language_pack_database_size")?,
            log_bytes: number("log_size")?,
        })
    }

    /// Reclaim downloaded audio/documents up to the requested target. All local
    /// readers finish before TDLib is asked to optimize its private directory.
    /// TDLib may retain in-use/uploading files, so report actual usage afterwards.
    pub async fn trim_storage(
        &self,
        target_bytes: u64,
        cancel: &CancellationToken,
    ) -> Result<TdStorageStats, TelegramError> {
        let size = i64::try_from(target_bytes).map_err(|_| TelegramError::InvalidInput)?;
        let guard = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(TelegramError::Cancelled),
            guard = self.storage.clone().write_owned() => guard,
        };
        // Cancellation after dispatch must not release the gate while TDLib is
        // still removing files. Own the request + gate in a task until completion.
        let transport = self.clone();
        let task = tokio::spawn(async move {
            let _guard = guard;
            transport.client().maintenance(json!({"@type":"optimizeStorage","size":size,
                "ttl":i32::MAX,"count":i32::MAX,"immunity_delay":0,
                "file_types":[{"@type":"fileTypeAudio"},{"@type":"fileTypeDocument"}],
                "chat_ids":[],"exclude_chat_ids":[],"return_deleted_file_statistics":false,"chat_limit":0})).await?;
            transport.storage_stats(&CancellationToken::new()).await
        });
        tokio::select! {
            _ = cancel.cancelled() => Err(TelegramError::Cancelled),
            result = task => result.map_err(|_| TelegramError::Closed)?,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use tokio::sync::RwLock;
    #[tokio::test]
    async fn cloned_range_keeps_maintenance_out_until_last_reader_finishes()
    -> Result<(), Box<dyn std::error::Error>> {
        let gate = Arc::new(RwLock::new(()));
        let range = crate::DownloadedRange {
            storage_lease: Some(Arc::new(gate.clone().read_owned().await)),
            path: "media".into(),
            file_offset: 0,
            offset: 0,
            available: 1,
            complete: true,
        };
        let second = range.clone();
        assert!(gate.try_write().is_err());
        drop(range);
        assert!(gate.try_write().is_err());
        drop(second);
        assert!(gate.try_write().is_ok());
        Ok(())
    }
}
