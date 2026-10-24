//! Automatic snapshots share the existing service's single writer. Destructive
//! GC retains its separate persisted plan and explicit confirmation boundary.
use crate::{ApiError, ErrorCode};
use serde::{Deserialize, Serialize};
use vault_sync::{RuntimeMode, SyncEngine};
use vault_telegram::Telegram;

const KEY: &str = "maintenance.config.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceConfig {
    pub enabled: bool,
    pub max_events: u64,
    pub max_log_bytes: u64,
    pub max_age_seconds: u64,
    pub idle_seconds: u64,
    pub check_interval_seconds: u64,
}
impl Default for MaintenanceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_events: 10_000,
            max_log_bytes: 16 * 1024 * 1024,
            max_age_seconds: 86_400,
            idle_seconds: 15,
            check_interval_seconds: 30,
        }
    }
}
impl MaintenanceConfig {
    pub fn validate(&self) -> Result<(), ApiError> {
        if self.max_events == 0
            || self.max_log_bytes == 0
            || self.max_age_seconds == 0
            || self.max_age_seconds > 365 * 86_400
            || self.idle_seconds > 86_400
            || !(1..=3600).contains(&self.check_interval_seconds)
        {
            return Err(ApiError::new(ErrorCode::InvalidInput));
        }
        Ok(())
    }
    fn threshold_reached(&self, events: u64, bytes: Option<u64>, age: Option<u64>) -> bool {
        self.enabled
            && events > 0
            && (events >= self.max_events
                || bytes.is_some_and(|bytes| bytes >= self.max_log_bytes)
                || age.is_some_and(|age| age >= self.max_age_seconds))
    }
}

pub(crate) async fn load<T: Telegram>(
    engine: &SyncEngine<T>,
) -> Result<MaintenanceConfig, ApiError> {
    let config = match engine.local_setting(KEY.into()).await? {
        Some(raw) => serde_json::from_str(&raw).map_err(|_| ApiError::new(ErrorCode::Integrity))?,
        None => MaintenanceConfig::default(),
    };
    config.validate()?;
    Ok(config)
}
pub(crate) async fn save<T: Telegram>(
    engine: &SyncEngine<T>,
    config: &MaintenanceConfig,
) -> Result<(), ApiError> {
    config.validate()?;
    engine
        .set_local_setting(
            KEY.into(),
            serde_json::to_string(config).map_err(|_| ApiError::new(ErrorCode::Internal))?,
        )
        .await?;
    Ok(())
}

pub(crate) async fn due<T: Telegram>(
    engine: &mut SyncEngine<T>,
    config: &MaintenanceConfig,
) -> Result<bool, ApiError> {
    if !config.enabled || engine.status().await?.mode != RuntimeMode::OnlineReady {
        return Ok(false);
    }
    if engine
        .reader()
        .shared_settings()
        .await?
        .gc_device_id
        .as_deref()
        != Some(engine.device_id())
    {
        return Ok(false);
    }
    let metrics = engine.reader().diagnostics().await?;
    if metrics.pending_commands > 0 || engine.reader().has_pending_gc().await? {
        return Ok(false);
    }
    let observation = engine
        .local_setting("snapshot.diagnostics.v1".into())
        .await?
        .map(|raw| {
            serde_json::from_str::<serde_json::Value>(&raw)
                .map_err(|_| ApiError::new(ErrorCode::Integrity))
        })
        .transpose()?;
    let bytes = observation
        .as_ref()
        .and_then(|v| v["uncovered_log_bytes"].as_u64());
    let age = observation
        .as_ref()
        .and_then(|v| v["created_at_unix"].as_u64())
        .and_then(|created| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .ok()?
                .as_secs()
                .checked_sub(created)
        });
    let repair_copies = engine
        .verified_snapshot()
        .is_some_and(|snapshot| !snapshot.primary_healthy);
    Ok(repair_copies || config.threshold_reached(metrics.uncovered_events, bytes, age))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maintenance_thresholds_do_not_create_empty_snapshots_or_guess_unknown_age() {
        let config = MaintenanceConfig::default();
        assert!(!config.threshold_reached(0, Some(u64::MAX), Some(u64::MAX)));
        assert!(!config.threshold_reached(1, None, None));
        assert!(config.threshold_reached(config.max_events, None, None));
        assert!(config.threshold_reached(1, Some(config.max_log_bytes), None));
        assert!(config.threshold_reached(1, None, Some(config.max_age_seconds)));
        assert!(
            !MaintenanceConfig {
                enabled: false,
                ..config.clone()
            }
            .threshold_reached(u64::MAX, None, None)
        );
        assert!(
            MaintenanceConfig {
                check_interval_seconds: 0,
                ..config
            }
            .validate()
            .is_err()
        );
    }
    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn automatic_maintenance_respects_coordinator_gc_and_connection_gates() {
        use tokio_util::sync::CancellationToken;
        use uuid::Uuid;
        use vault_domain::{ChatId, UserId};
        use vault_protocol::{Record, VaultInit, encode_text};
        use vault_testkit::{FakeServer, Faults};
        let binding = vault_db::Binding {
            vault_id: Uuid::new_v4(),
            chat_id: ChatId::new(-100).unwrap(),
            owner_id: UserId::new(42).unwrap(),
        };
        let server = FakeServer::new(binding.chat_id, binding.owner_id);
        let transport = server.client().unwrap();
        let cancel = CancellationToken::new();
        transport
            .send_text(
                binding.chat_id,
                encode_text(&Record::Init(VaultInit {
                    vault_id: binding.vault_id,
                    owner_telegram_user_id: binding.owner_id,
                    created_by: "test".into(),
                    created_at_utc: "2026-10-24T00:00:00Z".into(),
                }))
                .unwrap(),
                &cancel,
            )
            .await
            .unwrap();
        let mut engine = SyncEngine::new(
            transport,
            vault_db::Database::memory(binding).unwrap(),
            "owner".into(),
        );
        engine.initialize(&cancel).await.unwrap();
        let config = MaintenanceConfig {
            max_events: 1,
            ..Default::default()
        };
        save(&engine, &config).await.unwrap();
        assert_eq!(load(&engine).await.unwrap(), config);
        assert!(!due(&mut engine, &config).await.unwrap());
        engine
            .assign_gc_coordinator(Uuid::new_v4(), Some("owner".into()), None, false, &cancel)
            .await
            .unwrap();
        assert!(due(&mut engine, &config).await.unwrap());
        let gc = engine.plan_gc(&cancel).await.unwrap();
        assert!(
            !due(&mut engine, &config).await.unwrap(),
            "a pending GC plan owns maintenance"
        );
        engine.abandon_gc(gc.plan.plan_id).await.unwrap();
        assert!(due(&mut engine, &config).await.unwrap());
        server
            .set_faults(Faults {
                offline: true,
                ..Default::default()
            })
            .unwrap();
        assert!(engine.catch_up(&cancel).await.is_err());
        let calls = server.history_calls().unwrap();
        assert!(!due(&mut engine, &config).await.unwrap());
        assert_eq!(
            server.history_calls().unwrap(),
            calls,
            "offline maintenance must not attempt remote work"
        );
    }
}
