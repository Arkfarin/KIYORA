use crate::{ProjectionReader, SyncEngine, SyncError};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_domain::{ChatId, MessageId, Source};
use vault_telegram::{RemoteFile, Telegram, TelegramError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceAvailability {
    Unknown,
    Available,
    Unavailable,
    FailedRetryable,
    FailedPermanent,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceHealth {
    pub track_id: Uuid,
    pub source_id: Uuid,
    #[schemars(with = "String")]
    pub chat_id: ChatId,
    #[schemars(with = "String")]
    pub message_id: MessageId,
    pub availability: SourceAvailability,
}

impl ProjectionReader {
    pub async fn source_health(&self, track_id: Uuid) -> Result<Vec<SourceHealth>, SyncError> {
        let state = self.state().await?;
        let track = state
            .tracks
            .get(&track_id)
            .ok_or(SyncError::MediaSourceMissing)?;
        let mut result = Vec::new();
        for source in track.sources.values() {
            let key = format!("source.health.v1:{track_id}:{}", source.source_id);
            let raw = self.read(move |db| db.local_setting(&key)).await?;
            let saved = raw
                .map(|raw| serde_json::from_str::<SourceHealth>(&raw))
                .transpose()
                .map_err(|_| SyncError::Integrity)?;
            result.push(
                saved
                    .filter(|saved| {
                        saved.track_id == track_id
                            && saved.source_id == source.source_id
                            && saved.chat_id == source.chat_id
                            && saved.message_id == source.message_id
                    })
                    .unwrap_or(SourceHealth {
                        track_id,
                        source_id: source.source_id,
                        chat_id: source.chat_id,
                        message_id: source.message_id,
                        availability: SourceAvailability::Unknown,
                    }),
            );
        }
        Ok(result)
    }
}

impl<T: Telegram> SyncEngine<T> {
    pub(crate) async fn probe_source(
        &self,
        track_id: Uuid,
        source: &Source,
        cancel: &CancellationToken,
    ) -> Result<RemoteFile, SyncError> {
        let remote = self
            .transport
            .message(source.chat_id, source.message_id, cancel)
            .await;
        let (availability, result) = match remote {
            Ok(Some(message)) => match message.file {
                Some(file)
                    if source
                        .size_bytes
                        .is_none_or(|size| u64::try_from(file.size).ok() == Some(size)) =>
                {
                    (SourceAvailability::Available, Ok(file))
                }
                _ => (
                    SourceAvailability::Unavailable,
                    Err(SyncError::MediaSourceMissing),
                ),
            },
            Ok(None) | Err(TelegramError::Request { code: 404 }) => (
                SourceAvailability::Unavailable,
                Err(SyncError::MediaSourceMissing),
            ),
            Err(TelegramError::Cancelled) => return Err(TelegramError::Cancelled.into()),
            Err(error) => {
                let availability = if matches!(
                    error,
                    TelegramError::Timeout
                        | TelegramError::RateLimited { .. }
                        | TelegramError::Backpressure
                        | TelegramError::UpdatesLost
                        | TelegramError::Request { code: 500..=599 }
                ) {
                    SourceAvailability::FailedRetryable
                } else {
                    SourceAvailability::FailedPermanent
                };
                (availability, Err(SyncError::Telegram(error)))
            }
        };
        let health = SourceHealth {
            track_id,
            source_id: source.source_id,
            chat_id: source.chat_id,
            message_id: source.message_id,
            availability,
        };
        self.set_local_setting(
            format!("source.health.v1:{track_id}:{}", source.source_id),
            serde_json::to_string(&health).map_err(|_| SyncError::Integrity)?,
        )
        .await?;
        result
    }

    pub async fn check_sources(
        &mut self,
        track_id: Option<Uuid>,
        cancel: &CancellationToken,
    ) -> Result<Vec<SourceHealth>, SyncError> {
        self.require_online(cancel).await?;
        let state = self.state().await?;
        if track_id.is_some_and(|id| !state.tracks.contains_key(&id)) {
            return Err(SyncError::MediaSourceMissing);
        }
        let mut result = Vec::new();
        for (id, track) in &state.tracks {
            if track_id.is_some_and(|expected| expected != *id) {
                continue;
            }
            for source in track.sources.values() {
                if source.chat_id != self.binding.chat_id {
                    return Err(SyncError::Integrity);
                }
                match self.probe_source(*id, source, cancel).await {
                    Ok(_) | Err(SyncError::MediaSourceMissing) => {}
                    Err(error) => return Err(error),
                }
            }
            result.extend(self.reader().source_health(*id).await?);
        }
        Ok(result)
    }
}
