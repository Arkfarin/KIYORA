use crate::{RuntimeMode, SyncEngine, SyncError};
use serde::{Deserialize, Serialize};
use vault_telegram::Telegram;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VaultDiagnostics {
    pub snapshot_age_seconds: Option<u64>,
    /// UTF-8 text bytes in the last fully audited post-barrier history.
    pub uncovered_log_text_bytes: Option<u64>,
    pub mode: RuntimeMode,
    pub vault_id: uuid::Uuid,
    pub projection: vault_db::ProjectionDiagnostics,
    pub primary_healthy: Option<bool>,
    pub backup_healthy: Option<bool>,
    pub recovered_from_history: bool,
    /// None means the current history has not been completely verified.
    pub unknown_protocol_records: Option<u64>,
    pub gc_coordinator_device_id: Option<String>,
    pub known_missing_media_sources: u64,
    pub unchecked_media_sources: u64,
    pub source_checks_failed: u64,
    pub snapshot_recommended: bool,
}
impl<T: Telegram> SyncEngine<T> {
    pub async fn diagnostics(&mut self) -> Result<VaultDiagnostics, SyncError> {
        let status = self.status().await?;
        let reader = self.reader();
        let projection = reader.diagnostics().await?;
        let settings = reader.shared_settings().await?;
        let state = self.state().await?;
        let mut missing = 0;
        let mut unchecked = 0;
        let mut failed = 0;
        // Read local health in bounded pages instead of reloading the complete
        // projection separately for every track.
        let mut saved = std::collections::BTreeMap::new();
        let mut offset = 0;
        loop {
            let page = reader
                .local_settings("source.health.v1:".into(), offset, 500)
                .await?;
            if page.is_empty() {
                break;
            }
            offset += page.len() as u32;
            for (_, raw) in page {
                let health: crate::SourceHealth =
                    serde_json::from_str(&raw).map_err(|_| SyncError::Integrity)?;
                saved.insert((health.track_id, health.source_id), health);
            }
        }
        for (id, track) in &state.tracks {
            for (source_id, source) in &track.sources {
                let health = saved
                    .get(&(*id, *source_id))
                    .filter(|h| h.chat_id == source.chat_id && h.message_id == source.message_id);
                match health.map(|h| h.availability) {
                    Some(crate::SourceAvailability::Available) => {}
                    Some(crate::SourceAvailability::Unavailable) => missing += 1,
                    Some(
                        crate::SourceAvailability::FailedPermanent
                        | crate::SourceAvailability::FailedRetryable,
                    ) => failed += 1,
                    _ => unchecked += 1,
                }
            }
        }
        let snapshot = self.verified_snapshot();
        let observed = self
            .local_setting("snapshot.diagnostics.v1".into())
            .await?
            .map(|raw| {
                serde_json::from_str::<serde_json::Value>(&raw).map_err(|_| SyncError::Integrity)
            })
            .transpose()?;
        let observed = observed.filter(|value| {
            snapshot.is_some_and(|snapshot| {
                serde_json::to_value(snapshot.commit_id).ok().as_ref() == Some(&value["commit_id"])
            })
        });
        let snapshot_age_seconds = observed
            .as_ref()
            .and_then(|value| value["created_at_unix"].as_u64())
            .and_then(|created| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_secs()
                    .checked_sub(created)
            });
        let uncovered_log_text_bytes = observed
            .as_ref()
            .and_then(|value| value["uncovered_log_bytes"].as_u64());
        let recovered_from_history =
            snapshot.is_some_and(|s| !s.primary_healthy && !s.backup_healthy);
        let snapshot_recommended = projection.uncovered_events >= 10_000
            || recovered_from_history
            || (projection.uncovered_events > 0
                && (snapshot_age_seconds.is_some_and(|age| age >= 86_400)
                    || uncovered_log_text_bytes.is_some_and(|bytes| bytes >= 16 * 1024 * 1024)));
        Ok(VaultDiagnostics {
            snapshot_age_seconds,
            uncovered_log_text_bytes,
            mode: status.mode,
            vault_id: self.binding().vault_id,
            projection,
            primary_healthy: snapshot.map(|s| s.primary_healthy),
            // A normal sync may verify only A. Never label an unchecked B damaged.
            backup_healthy: snapshot.and_then(|s| if s.backup_healthy { Some(true) } else { None }),
            recovered_from_history,
            unknown_protocol_records: (status.mode == RuntimeMode::OnlineReady).then_some(0),
            gc_coordinator_device_id: settings.gc_device_id,
            known_missing_media_sources: missing,
            unchecked_media_sources: unchecked,
            source_checks_failed: failed,
            snapshot_recommended,
        })
    }
}
