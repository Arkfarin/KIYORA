#![allow(clippy::unwrap_used)]
use tempfile::tempdir;
use uuid::Uuid;
use vault_core::*;
use vault_security::ProfileStore;
use vault_sync::RuntimeMode;

#[tokio::test]
async fn switching_profile_changes_session_and_fences_stale_calls() {
    let temp = tempdir().unwrap();
    let backend = Backend::new(ProfileStore::at(temp.path()).unwrap(), "missing-tdjson.so");
    let first = backend.create_profile("Personal".into()).unwrap();
    let second = backend.create_profile("Work".into()).unwrap();
    let initial = backend.switch_profile(first.profile_id).await.unwrap();
    assert_eq!(initial.context.profile_id, first.profile_id);
    assert_eq!(initial.mode, RuntimeMode::AuthRequired);
    let next = backend.switch_profile(second.profile_id).await.unwrap();
    assert_ne!(initial.context.session_id, next.context.session_id);
    assert_eq!(
        backend
            .auth_status(initial.context.session_id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert!(backend.local_reader(next.context.session_id).await.is_err());
    let again = backend.switch_profile(first.profile_id).await.unwrap();
    assert_eq!(again.context.profile_id, first.profile_id);
    assert_ne!(again.context.session_id, initial.context.session_id);
    backend.shutdown().await.unwrap();
}

#[test]
fn typed_ipc_errors_and_profile_context_do_not_expose_native_messages() {
    let raw = vault_telegram::TelegramError::Request { code: 400 };
    let safe = ApiError::from(raw);
    assert_eq!(safe.code, ErrorCode::Network);
    assert_eq!(safe.message, "Telegram request failed");
    let context = Context {
        profile_id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        vault_id: None,
    };
    assert_eq!(
        serde_json::from_str::<Context>(&serde_json::to_string(&context).unwrap()).unwrap(),
        context
    );
}
