use vault_core::{ApiError, ErrorCode};
use vault_telegram::{AuthCodeInfo, AuthStatus, CodeDelivery};

fn delivery_name(delivery: CodeDelivery) -> &'static str {
    match delivery {
        CodeDelivery::TelegramMessage => "a Telegram message in another active Telegram session",
        CodeDelivery::Sms => "an SMS to your phone",
        CodeDelivery::SmsWord => "an SMS word to your phone",
        CodeDelivery::SmsPhrase => "an SMS phrase to your phone",
        CodeDelivery::Call => "a voice call to your phone",
        CodeDelivery::FlashCall => "a flash call (the calling number is the code)",
        CodeDelivery::MissedCall => "a missed call (enter the last digits of the calling number)",
        CodeDelivery::Fragment => "Fragment (open fragment.com with the wallet owning your number)",
        CodeDelivery::FirebaseAndroid => "Firebase in the official Telegram Android app",
        CodeDelivery::FirebaseIos => "Firebase in the official Telegram iOS app",
        CodeDelivery::Unknown => "a delivery method this version does not recognize",
    }
}

pub(super) fn code_description(info: &AuthCodeInfo) -> String {
    let mut text = format!(
        "Telegram sent the code via {}.",
        delivery_name(info.delivery)
    );
    if let Some(length) = info.expected_length {
        text.push_str(&format!(" Expected length: {length}."));
    }
    match (info.next_delivery, info.timeout_seconds) {
        (Some(next), Some(seconds)) => text.push_str(&format!(
            " Next delivery: {}; available in {seconds} seconds (Telegram checks the remaining delay). Enter /resend to request it.",
            delivery_name(next))),
        (Some(next), None) => text.push_str(&format!(
            " Next delivery: {}; its availability is unknown. Enter /resend to check with Telegram.", delivery_name(next))),
        (None, _) => text.push_str(" Telegram does not currently offer another delivery method. Enter /qr to sign in using Telegram on your phone."),
    }
    text
}

pub(super) fn status_description(status: &AuthStatus) -> String {
    match status {
        AuthStatus::CodeRequired { code_info } => code_description(code_info),
        AuthStatus::EmailCodeRequired { expected_length } => {
            let length = expected_length.map(|n| format!(" Expected length: {n}.")).unwrap_or_default();
            format!("Telegram sent a code to your login email.{length} Enter /resend to request another code.")
        }
        AuthStatus::ParametersRequired => "TDLib needs API credentials. Set KIYORA_API_ID and KIYORA_API_HASH, then run auth login.".into(),
        AuthStatus::PhoneRequired => "Telegram needs your phone number.".into(),
        AuthStatus::PasswordRequired => "Telegram needs your 2FA password.".into(),
        AuthStatus::EmailRequired => "Telegram needs your login email address.".into(),
        AuthStatus::QrConfirmation { .. } => "Confirm this login in another Telegram session.".into(),
        AuthStatus::RegistrationRequired => "Telegram requires registration. Complete it in an official Telegram app, then run auth login again.".into(),
        AuthStatus::PremiumRequired => "Telegram requires a Premium purchase for this login. Complete the required step in an official Telegram app.".into(),
        AuthStatus::Ready => "Telegram authorization is complete.".into(),
        AuthStatus::LoggingOut => "Telegram is logging out. Wait for completion.".into(),
        AuthStatus::Closing => "Telegram is closing. Run auth login again after it closes.".into(),
        AuthStatus::Closed => "Telegram is closed. Run auth login to restore the session.".into(),
        AuthStatus::Unsupported => "This Telegram authorization state is unsupported. Check the installed TDLib version.".into(),
    }
}

pub(super) fn retry_in_login(error: &ApiError) -> bool {
    matches!(
        error.code,
        ErrorCode::InvalidCode
            | ErrorCode::CodeExpired
            | ErrorCode::InvalidPassword
            | ErrorCode::ResendTooEarly
            | ErrorCode::ResendUnavailable
            | ErrorCode::AuthStateMismatch
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivery_guidance_and_resend_availability_are_explicit() {
        for (delivery, expected) in [
            (
                CodeDelivery::TelegramMessage,
                "another active Telegram session",
            ),
            (CodeDelivery::Sms, "SMS to your phone"),
            (CodeDelivery::SmsWord, "SMS word"),
            (CodeDelivery::SmsPhrase, "SMS phrase"),
            (CodeDelivery::Call, "voice call"),
            (CodeDelivery::FlashCall, "calling number is the code"),
            (CodeDelivery::MissedCall, "last digits"),
            (CodeDelivery::Fragment, "fragment.com"),
            (CodeDelivery::FirebaseAndroid, "Android"),
            (CodeDelivery::FirebaseIos, "iOS"),
            (CodeDelivery::Unknown, "does not recognize"),
        ] {
            let mut info = AuthCodeInfo {
                delivery,
                expected_length: Some(5),
                timeout_seconds: Some(23),
                next_delivery: Some(CodeDelivery::Sms),
            };
            let text = code_description(&info);
            assert!(text.contains(expected));
            assert!(text.contains("Expected length: 5"));
            assert!(text.contains("23 seconds"));
            assert!(text.contains("/resend"));
            info.next_delivery = None;
            assert!(!code_description(&info).contains("/resend"));
        }
    }

    #[test]
    fn login_retries_user_corrections_but_never_automatically_retries_network_or_flood_wait() {
        for code in [
            ErrorCode::InvalidCode,
            ErrorCode::CodeExpired,
            ErrorCode::InvalidPassword,
            ErrorCode::ResendTooEarly,
        ] {
            assert!(retry_in_login(&ApiError::new(code)));
        }
        for code in [
            ErrorCode::Network,
            ErrorCode::Timeout,
            ErrorCode::RateLimited,
            ErrorCode::TelegramRejected,
        ] {
            assert!(!retry_in_login(&ApiError::new(code)));
        }
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    #[allow(clippy::unwrap_used)]
    async fn interactive_login_recovers_after_bad_codes_resend_and_interruption_at_2fa() {
        let _guard = super::super::auth_flow::NATIVE_TEST_LOCK.lock().await;
        use std::{
            collections::BTreeMap,
            sync::{Arc, Mutex},
        };
        use vault_security::{ProfileStore, SecretStore, SecurityError};
        use zeroize::Zeroizing;
        #[derive(Default)]
        struct Secrets(Mutex<BTreeMap<String, Vec<u8>>>);
        impl SecretStore for Secrets {
            fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, SecurityError> {
                Ok(self.0.lock().unwrap().get(key).cloned().map(Zeroizing::new))
            }
            fn set(&self, key: &str, value: &[u8]) -> Result<(), SecurityError> {
                self.0.lock().unwrap().insert(key.into(), value.into());
                Ok(())
            }
        }
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
        let backend = vault_core::Backend::with_secrets(
            ProfileStore::at(dir.path().join("data")).unwrap(),
            &library,
            Arc::new(Secrets::default()),
        );
        let profile = backend.create_profile("CLI auth".into()).unwrap();
        let status = backend.switch_profile(profile.profile_id).await.unwrap();
        backend
            .connect_native(status.context.session_id, 123, "a".repeat(32).into())
            .await
            .unwrap();
        let mut inputs = [
            "PRIVATE_PHONE",
            "WRONG_CODE_PRIVATE",
            "EXPIRED_CODE_PRIVATE",
            "/resend",
            "VALID_CODE_PRIVATE",
        ]
        .into_iter();
        let mut prompts = Vec::new();
        let result = super::super::auth_flow::with_input(&backend, status.clone(), |label| {
            prompts.push(label.to_owned());
            inputs
                .next()
                .map(|s| s.to_owned().into())
                .ok_or_else(|| ApiError::new(ErrorCode::Cancelled))
        })
        .await;
        assert_eq!(result.unwrap_err().code, ErrorCode::Cancelled);
        assert_eq!(prompts.len(), 6);
        assert_eq!(
            backend
                .auth_status(status.context.session_id)
                .await
                .unwrap(),
            AuthStatus::PasswordRequired
        );
        backend.shutdown().await.unwrap();
        let restored = backend.restore_active_profile().await.unwrap().unwrap();
        let mut inputs = ["WRONG_PASSWORD_PRIVATE", "PASSWORD_PRIVATE"].into_iter();
        super::super::auth_flow::with_input(&backend, restored.clone(), |label| {
            assert!(label.contains("2FA"));
            Ok(inputs.next().unwrap().to_owned().into())
        })
        .await
        .unwrap();
        assert_eq!(
            backend
                .auth_status(restored.context.session_id)
                .await
                .unwrap(),
            AuthStatus::Ready
        );
        backend.shutdown().await.unwrap();
    }
}
