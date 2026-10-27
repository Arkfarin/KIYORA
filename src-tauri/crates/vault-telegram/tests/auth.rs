#![allow(clippy::unwrap_used)]
use serde_json::json;
use vault_telegram::{AuthStatus, CodeDelivery, TelegramError, check_response};

#[test]
fn every_authorization_state_is_recognized() {
    for (td, expected) in [
        ("WaitTdlibParameters", "PARAMETERS_REQUIRED"),
        ("WaitPhoneNumber", "PHONE_REQUIRED"),
        ("WaitCode", "CODE_REQUIRED"),
        ("WaitPassword", "PASSWORD_REQUIRED"),
        ("WaitOtherDeviceConfirmation", "QR_CONFIRMATION"),
        ("WaitEmailAddress", "EMAIL_REQUIRED"),
        ("WaitEmailCode", "EMAIL_CODE_REQUIRED"),
        ("WaitRegistration", "REGISTRATION_REQUIRED"),
        ("WaitPremiumPurchase", "PREMIUM_REQUIRED"),
        ("Ready", "READY"),
        ("LoggingOut", "LOGGING_OUT"),
        ("Closing", "CLOSING"),
        ("Closed", "CLOSED"),
        ("FutureState", "UNSUPPORTED"),
    ] {
        let status = AuthStatus::from_td(&json!({"@type":format!("authorizationState{td}")}));
        assert_eq!(status.name(), expected);
        let wire = serde_json::to_value(&status).unwrap();
        assert_eq!(wire["state"], expected);
        assert_eq!(serde_json::from_value::<AuthStatus>(wire).unwrap(), status);
    }
}

#[test]
fn code_info_preserves_delivery_length_timeout_and_next_method_without_secrets() {
    for (td, delivery) in [
        ("TelegramMessage", CodeDelivery::TelegramMessage),
        ("Sms", CodeDelivery::Sms),
        ("SmsWord", CodeDelivery::SmsWord),
        ("SmsPhrase", CodeDelivery::SmsPhrase),
        ("Call", CodeDelivery::Call),
        ("FlashCall", CodeDelivery::FlashCall),
        ("MissedCall", CodeDelivery::MissedCall),
        ("Fragment", CodeDelivery::Fragment),
        ("FirebaseAndroid", CodeDelivery::FirebaseAndroid),
        ("FirebaseIos", CodeDelivery::FirebaseIos),
        ("FutureType", CodeDelivery::Unknown),
    ] {
        let status = AuthStatus::from_td(&json!({
            "@type":"authorizationStateWaitCode",
            "code_info": {
                "phone_number":"PRIVATE_PHONE",
                "type":{"@type":format!("authenticationCodeType{td}"),"length":5,
                    "url":"PRIVATE_URL","receipt":"PRIVATE_RECEIPT","first_word":"PRIVATE_WORD"},
                "next_type":{"@type":"authenticationCodeTypeSms","length":6},"timeout":42
            }
        }));
        let AuthStatus::CodeRequired { ref code_info } = status else {
            panic!("missing code info")
        };
        assert_eq!(code_info.delivery, delivery);
        assert_eq!(code_info.expected_length, Some(5));
        assert_eq!(code_info.timeout_seconds, Some(42));
        assert_eq!(code_info.next_delivery, Some(CodeDelivery::Sms));
        assert!(
            !format!("{status:?} {}", serde_json::to_string(&status).unwrap()).contains("PRIVATE")
        );
    }
}

#[test]
fn absent_or_malformed_code_info_does_not_enable_resend() {
    for info in [
        serde_json::Value::Null,
        json!({}),
        json!({"timeout":-1,"type":{"length":0},"next_type":null}),
    ] {
        let state =
            AuthStatus::from_td(&json!({"@type":"authorizationStateWaitCode","code_info":info}));
        let AuthStatus::CodeRequired { code_info } = state else {
            panic!("missing code state")
        };
        assert_eq!(code_info.expected_length, None);
        assert_eq!(code_info.timeout_seconds, None);
        assert_eq!(code_info.next_delivery, None);
    }
}

#[test]
fn errors_are_classified_without_retaining_telegram_messages() {
    for (code, message, expected) in [
        (400, "PHONE_CODE_INVALID", TelegramError::InvalidCode),
        (400, "PHONE_CODE_EMPTY", TelegramError::InvalidCode),
        (400, "PHONE_CODE_EXPIRED", TelegramError::CodeExpired),
        (400, "EMAIL_CODE_INVALID", TelegramError::InvalidCode),
        (400, "EMAIL_CODE_EXPIRED", TelegramError::CodeExpired),
        (400, "PASSWORD_HASH_INVALID", TelegramError::InvalidPassword),
        (
            400,
            "PHONE_NUMBER_BANNED",
            TelegramError::Request { code: 400 },
        ),
        (
            500,
            "PRIVATE_SERVER_MESSAGE",
            TelegramError::Request { code: 500 },
        ),
        (400, "NETWORK_ERROR", TelegramError::Network),
        (
            429,
            "Too Many Requests: retry after 23",
            TelegramError::RateLimited {
                retry_after_seconds: 23,
            },
        ),
        (
            420,
            "FLOOD_WAIT_100000",
            TelegramError::RateLimited {
                retry_after_seconds: 100000,
            },
        ),
        (
            400,
            "FLOOD_WAIT_17",
            TelegramError::RateLimited {
                retry_after_seconds: 17,
            },
        ),
        (
            429,
            "PRIVATE_PHONE_123456789",
            TelegramError::RateLimited {
                retry_after_seconds: 60,
            },
        ),
        (
            406,
            "PHONE_CODE_INVALID",
            TelegramError::Request { code: 406 },
        ),
    ] {
        let error =
            check_response(json!({"@type":"error","code":code,"message":message})).unwrap_err();
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains(message));
    }
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn native_auth_resends_only_when_allowed_and_recovers_interrupted_login() {
    use vault_telegram::{AuthInput, TdClient, TdParameters};
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("auth.so");
    assert!(
        std::process::Command::new("cc")
            .args(["-shared", "-fPIC", "-o"])
            .arg(&library)
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/auth_fixture.c"))
            .status()
            .unwrap()
            .success()
    );
    let initialize = |client: TdClient| {
        let directory = dir.path().to_owned();
        async move {
            TdParameters {
                api_id: 123,
                api_hash: "a".repeat(32).into(),
                database_key: vec![7; 32].into(),
                database_directory: directory.clone(),
                files_directory: directory,
            }
            .initialize(&client)
            .await
            .unwrap();
        }
    };
    let client = TdClient::open(&library).await.unwrap();
    initialize(client.clone()).await;
    assert_eq!(
        client
            .authenticate(AuthInput::Code("SECRET".to_owned().into()))
            .await,
        Err(TelegramError::AuthStateMismatch)
    );
    let state = client
        .authenticate(AuthInput::Phone("PRIVATE_PHONE".to_owned().into()))
        .await
        .unwrap();
    assert!(matches!(state, AuthStatus::CodeRequired { .. }));
    for (code, error) in [
        ("WRONG_CODE_PRIVATE", TelegramError::InvalidCode),
        ("EXPIRED_CODE_PRIVATE", TelegramError::CodeExpired),
        ("NETWORK_CODE_PRIVATE", TelegramError::Network),
        ("REJECT_CODE_PRIVATE", TelegramError::Request { code: 403 }),
    ] {
        assert_eq!(
            client
                .authenticate(AuthInput::Code(code.to_owned().into()))
                .await,
            Err(error)
        );
        assert!(matches!(
            client.auth_status(),
            AuthStatus::CodeRequired { .. }
        ));
    }
    assert_eq!(
        client.resend_authentication_code().await,
        Err(TelegramError::ResendTooEarly {
            retry_after_seconds: 30
        })
    );
    assert_eq!(
        client.call(json!({"@type":"testStats"})).await.unwrap()["resend_calls"],
        0
    );
    // The cached update still says 30. The fresh TDLib response must take precedence.
    client
        .call(json!({"@type":"testSetTimeout","delay":0}))
        .await
        .unwrap();
    let (a, b) = tokio::join!(
        client.resend_authentication_code(),
        client.resend_authentication_code()
    );
    assert!(a.is_ok());
    assert_eq!(b, Err(TelegramError::AuthBusy));
    assert_eq!(
        client.call(json!({"@type":"testStats"})).await.unwrap()["resend_calls"],
        1
    );
    let AuthStatus::CodeRequired { code_info } = a.unwrap() else {
        panic!("code missing")
    };
    assert_eq!(code_info.delivery, CodeDelivery::Sms);

    client
        .call(json!({"@type":"testSetState","state":"WaitCode","delay":0,"next":0}))
        .await
        .unwrap();
    assert_eq!(
        client.resend_authentication_code().await,
        Err(TelegramError::ResendUnavailable)
    );
    let before = client.call(json!({"@type":"testStats"})).await.unwrap()["code_calls"]
        .as_u64()
        .unwrap();
    assert_eq!(
        client
            .authenticate(AuthInput::Code("FLOOD_CODE_PRIVATE".to_owned().into()))
            .await,
        Err(TelegramError::RateLimited {
            retry_after_seconds: 77
        })
    );
    assert!(matches!(
        client
            .authenticate(AuthInput::Code("VALID_CODE_PRIVATE".to_owned().into()))
            .await,
        Err(TelegramError::RateLimited { .. })
    ));
    assert!(matches!(
        client.resend_authentication_code().await,
        Err(TelegramError::RateLimited { .. })
    ));
    assert_eq!(
        client.call(json!({"@type":"testStats"})).await.unwrap()["code_calls"],
        before + 1
    );

    // Reopening the same directory resumes WaitCode without sending a phone or a new code.
    let saved = std::fs::read(dir.path().join("fixture-session")).unwrap();
    client.close().await.unwrap();
    drop(client);
    let client = TdClient::open(&library).await.unwrap();
    initialize(client.clone()).await;
    assert!(matches!(
        client.refresh_auth_status().await.unwrap(),
        AuthStatus::CodeRequired { .. }
    ));
    assert_eq!(
        std::fs::read(dir.path().join("fixture-session")).unwrap(),
        saved
    );
    assert_eq!(
        client.call(json!({"@type":"testStats"})).await.unwrap()["phone_calls"],
        0
    );
    // Interrupt a code check after TDLib accepted it but before its reply arrived.
    // Reopening must resume at 2FA, never submit the phone/code again.
    let mut changes = client.authorization_changes();
    let interrupted = {
        let client = client.clone();
        tokio::spawn(async move {
            client
                .authenticate(AuthInput::Code(
                    "INTERRUPTED_CODE_PRIVATE".to_owned().into(),
                ))
                .await
        })
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if AuthStatus::from_td(&changes.borrow_and_update()) == AuthStatus::PasswordRequired {
                break;
            }
            changes.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    interrupted.abort();
    assert!(interrupted.await.unwrap_err().is_cancelled());
    client.close().await.unwrap();
    drop(client);
    let client = TdClient::open(&library).await.unwrap();
    initialize(client.clone()).await;
    assert_eq!(
        client.refresh_auth_status().await.unwrap(),
        AuthStatus::PasswordRequired
    );
    assert_eq!(
        client
            .authenticate(AuthInput::Password(
                "WRONG_PASSWORD_PRIVATE".to_owned().into()
            ))
            .await,
        Err(TelegramError::InvalidPassword)
    );
    assert_eq!(
        client
            .authenticate(AuthInput::Password("PASSWORD_PRIVATE".to_owned().into()))
            .await
            .unwrap(),
        AuthStatus::Ready
    );
    assert_eq!(
        client.resend_authentication_code().await,
        Err(TelegramError::ResendUnavailable)
    );

    // Alternate login states and terminal states never trigger an implicit resend.
    for state in [
        "WaitTdlibParameters",
        "WaitPhoneNumber",
        "WaitPassword",
        "WaitRegistration",
        "WaitPremiumPurchase",
        "Ready",
        "LoggingOut",
        "Closing",
        "FutureState",
    ] {
        client
            .call(json!({"@type":"testSetState","state":state}))
            .await
            .unwrap();
        assert_eq!(
            client.resend_authentication_code().await,
            Err(TelegramError::ResendUnavailable)
        );
    }
    client
        .call(json!({"@type":"testSetState","state":"WaitPhoneNumber"}))
        .await
        .unwrap();
    assert!(matches!(
        client.authenticate(AuthInput::Qr).await.unwrap(),
        AuthStatus::QrConfirmation { .. }
    ));
    client
        .call(json!({"@type":"testSetState","state":"WaitEmailAddress"}))
        .await
        .unwrap();
    assert_eq!(
        client
            .authenticate(AuthInput::Email("PRIVATE_EMAIL".to_owned().into()))
            .await
            .unwrap(),
        AuthStatus::EmailCodeRequired {
            expected_length: Some(6)
        }
    );
    assert!(matches!(
        client.resend_authentication_code().await.unwrap(),
        AuthStatus::EmailCodeRequired { .. }
    ));
    assert!(matches!(
        client
            .authenticate(AuthInput::EmailCode("EMAIL_CODE_PRIVATE".to_owned().into()))
            .await
            .unwrap(),
        AuthStatus::CodeRequired { .. }
    ));
    client.close().await.unwrap();
}

#[test]
fn qr_credentials_are_redacted_from_debug_and_availability_is_explicit() {
    let status = AuthStatus::QrConfirmation {
        link: "tg://login?token=PRIVATE_QR".into(),
    };
    assert!(!format!("{status:?}").contains("PRIVATE_QR"));
    for state in [
        "WaitPhoneNumber",
        "WaitCode",
        "WaitPassword",
        "WaitEmailAddress",
        "WaitEmailCode",
        "WaitRegistration",
        "WaitPremiumPurchase",
    ] {
        assert!(
            AuthStatus::from_td(&json!({"@type":format!("authorizationState{state}")}))
                .can_request_qr()
        );
    }
    for state in [
        "Ready",
        "WaitTdlibParameters",
        "WaitOtherDeviceConfirmation",
        "Closing",
        "Closed",
        "Future",
    ] {
        assert!(
            !AuthStatus::from_td(&json!({"@type":format!("authorizationState{state}")}))
                .can_request_qr()
        );
    }
}
