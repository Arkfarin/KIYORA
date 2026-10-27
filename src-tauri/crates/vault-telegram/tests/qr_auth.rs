#![cfg(target_os = "linux")]
#![allow(clippy::unwrap_used)]
use serde_json::json;
use tokio_util::sync::CancellationToken;
use vault_telegram::{AuthInput, AuthStatus, TdClient, TdParameters, TelegramError};

#[tokio::test]
async fn native_qr_updates_confirmation_2fa_cancellation_reconnect_and_session_restore() {
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
    let mut stream = client.auth_updates();
    assert_eq!(stream.current().status, AuthStatus::PhoneRequired);
    // Two simultaneous requests must not both reach TDLib, even from clones.
    let (first, second) = tokio::join!(
        client.authenticate(AuthInput::Qr),
        client.authenticate(AuthInput::Qr)
    );
    assert!(matches!(first.unwrap(), AuthStatus::QrConfirmation { .. }));
    assert_eq!(second, Err(TelegramError::AuthBusy));
    let cancel = CancellationToken::new();
    stream.changed(&cancel).await.unwrap();
    let first = stream.current();
    assert!(first.connected);
    assert_eq!(
        first.status,
        AuthStatus::QrConfirmation {
            link: "tg://login?token=PRIVATE_QR_TOKEN_0".into()
        }
    );
    client.call(json!({"@type":"testRotateQr"})).await.unwrap();
    stream.changed(&cancel).await.unwrap();
    assert_eq!(
        stream.current().status,
        AuthStatus::QrConfirmation {
            link: "tg://login?token=PRIVATE_QR_TOKEN_1".into()
        }
    );
    // No QR request was needed for rotation, disconnect or reconnect.
    for ready in [0, 1] {
        client
            .call(json!({"@type":"testConnection","ready":ready}))
            .await
            .unwrap();
        stream.changed(&cancel).await.unwrap();
        assert_eq!(stream.current().connected, ready == 1);
    }
    assert_eq!(
        client.call(json!({"@type":"testStats"})).await.unwrap()["qr_calls"],
        1
    );
    client
        .call(json!({"@type":"testSetState","state":"WaitPassword"}))
        .await
        .unwrap();
    stream.changed(&cancel).await.unwrap();
    assert_eq!(stream.current().status, AuthStatus::PasswordRequired);
    assert_eq!(
        client
            .authenticate(AuthInput::Password("PASSWORD_PRIVATE".to_owned().into()))
            .await
            .unwrap(),
        AuthStatus::Ready
    );
    assert_eq!(
        client.call(json!({"@type":"getMe"})).await.unwrap()["id"],
        123
    );
    client.close().await.unwrap();
    drop(client);
    let saved = std::fs::read(dir.path().join("fixture-session")).unwrap();
    assert!(!String::from_utf8_lossy(&saved).contains("token"));
    let client = TdClient::open(&library).await.unwrap();
    initialize(client.clone()).await;
    assert_eq!(
        client.refresh_auth_status().await.unwrap(),
        AuthStatus::Ready
    );
    assert_eq!(
        client.authenticate(AuthInput::Qr).await,
        Err(TelegramError::AuthStateMismatch)
    );
    assert_eq!(
        client.call(json!({"@type":"testStats"})).await.unwrap()["qr_calls"],
        0
    );
    assert_eq!(
        std::fs::read(dir.path().join("fixture-session")).unwrap(),
        saved
    );

    // Every documented entry state, plus unsupported/terminal states.
    for state in [
        "WaitPhoneNumber",
        "WaitCode",
        "WaitPassword",
        "WaitEmailAddress",
        "WaitEmailCode",
        "WaitRegistration",
        "WaitPremiumPurchase",
    ] {
        client
            .call(json!({"@type":"testSetState","state":state}))
            .await
            .unwrap();
        assert!(matches!(
            client.authenticate(AuthInput::Qr).await.unwrap(),
            AuthStatus::QrConfirmation { .. }
        ));
    }
    client
        .call(json!({"@type":"testSetState","state":"FutureState"}))
        .await
        .unwrap();
    assert_eq!(
        client.authenticate(AuthInput::Qr).await,
        Err(TelegramError::AuthStateMismatch)
    );
    // Confirmation without 2FA is an update, not a response to another QR request.
    client
        .call(json!({"@type":"testSetState","state":"WaitPhoneNumber"}))
        .await
        .unwrap();
    client.authenticate(AuthInput::Qr).await.unwrap();
    let mut stream = client.auth_updates();
    stream.current();
    client
        .call(json!({"@type":"testSetState","state":"Ready"}))
        .await
        .unwrap();
    stream.changed(&cancel).await.unwrap();
    assert_eq!(stream.current().status, AuthStatus::Ready);

    // A cancelled Rust future must not release the outstanding native query gate.
    client
        .call(json!({"@type":"testSetState","state":"WaitCode"}))
        .await
        .unwrap();
    let mut stream = client.auth_updates();
    stream.current();
    let token = CancellationToken::new();
    let pending = client.authenticate_cancellable(
        AuthInput::Code("INTERRUPTED_CODE_PRIVATE".to_owned().into()),
        &token,
    );
    tokio::pin!(pending);
    tokio::select! {
        result = &mut pending => panic!("unexpected result: {result:?}"),
        _ = async {
            loop {
                stream.changed(&cancel).await.unwrap();
                if stream.current().status == AuthStatus::PasswordRequired { break; }
            }
        } => {}
    }
    token.cancel();
    assert_eq!(pending.await, Err(TelegramError::Cancelled));
    assert_eq!(
        client.authenticate(AuthInput::Qr).await,
        Err(TelegramError::AuthBusy)
    );
    client
        .call(json!({"@type":"testReleaseAuth"}))
        .await
        .unwrap();
    assert!(client.authenticate(AuthInput::Qr).await.is_ok());
    let token = CancellationToken::new();
    token.cancel();
    assert_eq!(stream.changed(&token).await, Err(TelegramError::Cancelled));
    client.close().await.unwrap();
}
