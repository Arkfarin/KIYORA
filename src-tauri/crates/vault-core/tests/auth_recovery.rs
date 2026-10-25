#![allow(clippy::unwrap_used)]
use vault_core::{ApiError, ErrorCode, LoginInput};
use vault_telegram::{AuthInput, TelegramError};

#[test]
fn api_preserves_auth_error_categories_and_retry_delay() {
    for (raw, code, retryable, wait) in [
        (
            TelegramError::InvalidCode,
            ErrorCode::InvalidCode,
            false,
            None,
        ),
        (
            TelegramError::CodeExpired,
            ErrorCode::CodeExpired,
            false,
            None,
        ),
        (
            TelegramError::InvalidPassword,
            ErrorCode::InvalidPassword,
            false,
            None,
        ),
        (TelegramError::Network, ErrorCode::Network, true, None),
        (TelegramError::Timeout, ErrorCode::Timeout, false, None),
        (
            TelegramError::Request { code: 403 },
            ErrorCode::TelegramRejected,
            false,
            None,
        ),
        (
            TelegramError::Request { code: 500 },
            ErrorCode::Network,
            true,
            None,
        ),
        (
            TelegramError::RateLimited {
                retry_after_seconds: 100_000,
            },
            ErrorCode::RateLimited,
            true,
            Some(100_000),
        ),
        (
            TelegramError::ResendTooEarly {
                retry_after_seconds: 42,
            },
            ErrorCode::ResendTooEarly,
            true,
            Some(42),
        ),
    ] {
        let safe = ApiError::from(raw);
        assert_eq!(safe.code, code);
        assert_eq!(safe.retryable, retryable);
        assert_eq!(safe.retry_after_seconds, wait);
        assert_eq!(
            serde_json::from_value::<ApiError>(serde_json::to_value(&safe).unwrap()).unwrap(),
            safe
        );
    }
    let input: LoginInput =
        serde_json::from_value(serde_json::json!({"kind":"resend_code"})).unwrap();
    assert!(matches!(AuthInput::from(input), AuthInput::ResendCode));
    let old: ApiError = serde_json::from_value(
        serde_json::json!({"code":"NETWORK","message":"Telegram request failed","retryable":true}),
    )
    .unwrap();
    assert_eq!(old.retry_after_seconds, None);
}

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use std::{
        collections::BTreeMap,
        io::Write,
        sync::{Arc, Mutex},
    };
    use tokio_util::sync::CancellationToken;
    use vault_core::Backend;
    use vault_security::{ProfileStore, SecretStore, SecurityError};
    use vault_telegram::AuthStatus;
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct Secrets(Mutex<BTreeMap<String, Vec<u8>>>);
    impl SecretStore for Secrets {
        fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecurityError> {
            Ok(self.0.lock().unwrap().get(key).cloned().map(Zeroizing::new))
        }
        fn set(&self, key: &str, value: &[u8]) -> Result<(), SecurityError> {
            self.0
                .lock()
                .unwrap()
                .insert(key.to_owned(), value.to_owned());
            Ok(())
        }
    }
    #[derive(Clone, Default)]
    struct Log(Arc<Mutex<Vec<u8>>>);
    impl Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn restores_each_saved_login_stage_without_deleting_profile_or_leaking_credentials() {
        let log = Log::default();
        let writer = log.clone();
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .json()
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .finish(),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let library = dir.path().join("auth.so");
        assert!(
            std::process::Command::new("cc")
                .args(["-shared", "-fPIC", "-o"])
                .arg(&library)
                .arg(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../vault-telegram/tests/auth_fixture.c"
                ))
                .status()
                .unwrap()
                .success()
        );
        let data = dir.path().join("data");
        let secrets = Arc::new(Secrets::default());
        let open =
            || Backend::with_secrets(ProfileStore::at(&data).unwrap(), &library, secrets.clone());
        let backend = open();
        let profile = backend.create_profile("Auth recovery".into()).unwrap();
        let status = backend.switch_profile(profile.profile_id).await.unwrap();
        let id = status.context.session_id;
        backend
            .connect_native(
                id,
                123,
                "PRIVATE_API_HASH_123456789012345".to_owned().into(),
            )
            .await
            .unwrap();
        let cancel = CancellationToken::new();
        let token = CancellationToken::new();
        token.cancel();
        assert_eq!(
            backend
                .authenticate(
                    id,
                    AuthInput::Phone("PRIVATE_PHONE".to_owned().into()),
                    &token
                )
                .await
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
        assert!(matches!(
            backend
                .authenticate(
                    id,
                    AuthInput::Phone("PRIVATE_PHONE".to_owned().into()),
                    &cancel
                )
                .await
                .unwrap(),
            AuthStatus::CodeRequired { .. }
        ));
        assert_eq!(
            backend
                .authenticate(id, AuthInput::ResendCode, &cancel)
                .await
                .unwrap_err()
                .retry_after_seconds,
            Some(30)
        );
        for (input, expected) in [
            ("WRONG_CODE_PRIVATE", ErrorCode::InvalidCode),
            ("EXPIRED_CODE_PRIVATE", ErrorCode::CodeExpired),
            ("NETWORK_CODE_PRIVATE", ErrorCode::Network),
            ("REJECT_CODE_PRIVATE", ErrorCode::TelegramRejected),
        ] {
            assert_eq!(
                backend
                    .authenticate(id, AuthInput::Code(input.to_owned().into()), &cancel)
                    .await
                    .unwrap_err()
                    .code,
                expected
            );
        }
        backend.shutdown().await.unwrap();
        drop(backend);

        let profile_dir = data.join("profiles").join(profile.profile_id.to_string());
        let session_file = profile_dir.join("tdlib/fixture-session");
        let marker = profile_dir.join("tdlib/existing-session-data");
        std::fs::write(&marker, "PRIVATE_SESSION_DATA").unwrap();
        // Resume all states TDLib can persist, including a completed login.
        for (saved, expected) in [
            ("WaitPhoneNumber", "PHONE_REQUIRED"),
            ("WaitCode", "CODE_REQUIRED"),
            ("WaitPassword", "PASSWORD_REQUIRED"),
            ("WaitEmailAddress", "EMAIL_REQUIRED"),
            ("WaitEmailCode", "EMAIL_CODE_REQUIRED"),
            ("WaitOtherDeviceConfirmation", "QR_CONFIRMATION"),
            ("WaitRegistration", "REGISTRATION_REQUIRED"),
            ("WaitPremiumPurchase", "PREMIUM_REQUIRED"),
            ("Ready", "READY"),
        ] {
            let contents = format!("{saved} 17 1 0");
            std::fs::write(&session_file, &contents).unwrap();
            let backend = open();
            let restored = backend.restore_active_profile().await.unwrap().unwrap();
            assert_eq!(restored.context.profile_id, profile.profile_id);
            assert_ne!(restored.context.session_id, id);
            assert_eq!(
                backend
                    .auth_status(restored.context.session_id)
                    .await
                    .unwrap()
                    .name(),
                expected
            );
            assert_eq!(std::fs::read_to_string(&session_file).unwrap(), contents);
            assert_eq!(
                std::fs::read_to_string(&marker).unwrap(),
                "PRIVATE_SESSION_DATA"
            );
            assert_eq!(backend.profiles().unwrap().len(), 1);
            backend.shutdown().await.unwrap();
        }
        // Finish an interrupted 2FA step using the same saved profile and keys.
        std::fs::write(&session_file, "WaitPassword 0 1 0").unwrap();
        let backend = open();
        let restored = backend.restore_active_profile().await.unwrap().unwrap();
        assert_eq!(
            backend
                .authenticate(
                    restored.context.session_id,
                    AuthInput::Password("PRIVATE_PASSWORD".to_owned().into()),
                    &cancel
                )
                .await
                .unwrap(),
            AuthStatus::Ready
        );
        backend.shutdown().await.unwrap();

        let diagnostics = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
        assert!(
            !diagnostics.contains("PRIVATE"),
            "credentials must not appear in diagnostics"
        );
        assert!(!diagnostics.contains(&profile_dir.to_string_lossy().to_string()));
        let events: Vec<serde_json::Value> = diagnostics
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert!(
            events
                .iter()
                .any(|e| e["fields"]["state"] == "CODE_REQUIRED")
        );
        assert!(
            events
                .iter()
                .any(|e| e["fields"]["operation"] == "resend_code"
                    && e["fields"]["retry_after_seconds"] == 30)
        );
        for category in [
            "invalid_code",
            "code_expired",
            "network",
            "telegram_rejected",
        ] {
            assert!(events.iter().any(|e| e["fields"]["error_code"] == category));
        }
    }
}
