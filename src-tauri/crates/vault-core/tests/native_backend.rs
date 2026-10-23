#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use vault_core::{Backend, EventData};
use vault_security::{ProfileStore, SecretStore, SecurityError};
use vault_telegram::AuthStatus;
use zeroize::Zeroizing;

#[derive(Default)]
struct MemorySecrets(Mutex<BTreeMap<String, Vec<u8>>>);
impl SecretStore for MemorySecrets {
    fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecurityError> {
        Ok(self.0.lock().unwrap().get(key).cloned().map(Zeroizing::new))
    }
    fn set(&self, key: &str, value: &[u8]) -> Result<(), SecurityError> {
        self.0.lock().unwrap().insert(key.into(), value.into());
        Ok(())
    }
}

#[tokio::test]
async fn native_connect_activates_async_authorization_and_status_reads_are_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let native = temp.path().join("fixture.so");
    assert!(
        std::process::Command::new("cc")
            .args(["-shared", "-fPIC", "-o"])
            .arg(&native)
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/backend_native_fixture.c"
            ))
            .status()
            .unwrap()
            .success()
    );
    let backend = Backend::with_secrets(
        ProfileStore::at(temp.path().join("data")).unwrap(),
        &native,
        Arc::new(MemorySecrets::default()),
    );
    let profile = backend.create_profile("Native integration".into()).unwrap();
    let status = backend.switch_profile(profile.profile_id).await.unwrap();
    let mut events = backend.subscribe_events();
    let auth = backend
        .connect_native(status.context.session_id, 123, "a".repeat(32).into())
        .await
        .unwrap();
    assert_eq!(auth, AuthStatus::PhoneRequired);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if matches!(event.data, EventData::Runtime(ref s) if s.mode == vault_sync::RuntimeMode::Discovering) {
                assert_eq!(event.context.session_id, status.context.session_id);
                break;
            }
        }
    }).await.unwrap();
    assert_eq!(backend.profiles().unwrap()[0].owner_id.unwrap().get(), 123);
    while events.try_recv().is_ok() {}
    for _ in 0..5 {
        assert_eq!(
            backend
                .auth_status(status.context.session_id)
                .await
                .unwrap(),
            AuthStatus::Ready
        );
    }
    assert!(
        events.try_recv().is_err(),
        "status reads must not restart/republish activation"
    );
    backend.disconnect(status.context.session_id).await.unwrap();
    backend
        .connect_native(status.context.session_id, 123, "a".repeat(32).into())
        .await
        .unwrap();
    backend.shutdown().await.unwrap();

    // A persisted vault starts one service. An unavailable channel deliberately
    // leaves it read-only; polling auth must not replace that actor with another.
    let binding = vault_db::Binding {
        vault_id: uuid::Uuid::new_v4(),
        owner_id: vault_domain::UserId::new(123).unwrap(),
        chat_id: vault_domain::ChatId::new(-100).unwrap(),
    };
    let directory = temp
        .path()
        .join("data/profiles")
        .join(profile.profile_id.to_string());
    let config_path = directory.join("backend.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    config["binding"] = serde_json::to_value(binding).unwrap();
    std::fs::write(config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    drop(vault_db::Database::open(directory.join("vault.sqlite"), binding).unwrap());
    let restored = backend.switch_profile(profile.profile_id).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.context.session_id == restored.context.session_id && matches!(event.data, EventData::Runtime(ref s) if s.mode == vault_sync::RuntimeMode::DegradedReadonly) { break; }
        }
    }).await.unwrap();
    while events.try_recv().is_ok() {}
    for _ in 0..5 {
        assert_eq!(
            backend
                .auth_status(restored.context.session_id)
                .await
                .unwrap(),
            AuthStatus::Ready
        );
    }
    assert!(
        events.try_recv().is_err(),
        "polling must preserve the existing vault actor"
    );
    backend.shutdown().await.unwrap();
}
