#![allow(clippy::unwrap_used)]
use serde_json::json;
#[cfg(target_os = "linux")]
use std::{process::Command, time::Duration};
#[cfg(target_os = "linux")]
use tokio_util::sync::CancellationToken;
use vault_telegram::*;

#[cfg(target_os = "linux")]
#[tokio::test]
async fn abi_worker_correlates_cancels_times_out_logs_out_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let lib = dir.path().join("fixture.so");
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/native_fixture.c");
    assert!(
        Command::new("cc")
            .args(["-shared", "-fPIC", "-o"])
            .arg(&lib)
            .arg(source)
            .status()
            .unwrap()
            .success()
    );
    let client = TdClient::open(&lib).await.unwrap();
    assert!(matches!(
        TdClient::open(&lib).await,
        Err(TelegramError::AlreadyRunning)
    ));
    client.verify_version().await.unwrap();
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..40 {
        let client = client.clone();
        tasks.spawn(async move { client.call(json!({"@type":"getMe"})).await });
    }
    while let Some(result) = tasks.join_next().await {
        assert_eq!(result.unwrap().unwrap()["@type"], "ok");
    }
    assert_eq!(
        client
            .request(
                json!({"@type":"neverReply"}),
                Duration::from_millis(100),
                &CancellationToken::new()
            )
            .await,
        Err(TelegramError::Timeout)
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        client
            .request(
                json!({"@type":"neverReply"}),
                Duration::from_secs(1),
                &cancel
            )
            .await,
        Err(TelegramError::Cancelled)
    );
    assert_eq!(
        client.call(json!({"@type":"rateLimited"})).await,
        Err(TelegramError::RateLimited {
            retry_after_seconds: 23
        })
    );
    assert_eq!(
        client
            .read(json!({"@type":"sendMessage"}), &CancellationToken::new())
            .await,
        Err(TelegramError::InvalidInput)
    );
    client.logout().await.unwrap();
    assert_eq!(
        client.call(json!({"@type":"getMe"})).await,
        Err(TelegramError::Closed)
    );
    let next = TdClient::open(&lib).await.unwrap();
    next.close().await.unwrap();
}

#[test]
fn errors_redact_messages_and_auth_maps_revocation() {
    let result =
        check_response(json!({"@type":"error","code":400,"message":"secret login code 12345"}))
            .unwrap_err();
    assert!(!format!("{result:?} {result}").contains("12345"));
    assert_eq!(
        AuthStatus::from_td(&json!({"@type":"authorizationStateClosed"})),
        AuthStatus::Closed
    );
    assert_eq!(
        AuthStatus::from_td(&json!({"@type":"futureAuthState"})),
        AuthStatus::Unsupported
    );
}

#[test]
fn server_errors_and_timeouts_do_not_prove_a_write_was_rejected() {
    for code in [0, 408, 500, 502, 503, 504] {
        assert!(!TelegramError::Request { code }.definitely_not_sent());
    }
    for code in [400, 401, 403, 404, 420, 429] {
        assert!(TelegramError::Request { code }.definitely_not_sent());
    }
    for error in [
        TelegramError::Timeout,
        TelegramError::Cancelled,
        TelegramError::Closed,
        TelegramError::UpdatesLost,
        TelegramError::InvalidResponse,
    ] {
        assert!(!error.definitely_not_sent());
    }
}
