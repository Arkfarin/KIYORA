#![allow(clippy::unwrap_used, clippy::expect_used)]
use vault_telegram::*;

/// No credentials or network account access: checks actual native lifecycle only.
#[tokio::test]
#[ignore = "set KIYORA_TDJSON to the pinned native library; explicit opt-in"]
async fn real_native_start_version_close() {
    let path = std::env::var_os("KIYORA_TDJSON").expect("KIYORA_TDJSON is required");
    let client = TdClient::open(std::path::PathBuf::from(path))
        .await
        .unwrap();
    client.verify_version().await.unwrap();
    let status = client
        .call(serde_json::json!({"@type":"getAuthorizationState"}))
        .await
        .unwrap();
    assert_eq!(AuthStatus::from_td(&status), AuthStatus::ParametersRequired);
    client.close().await.unwrap();
    // A profile switch reuses the process library but creates a fresh client and
    // receive worker; stale close updates must not terminate the new session.
    let client = TdClient::open(std::path::PathBuf::from(
        std::env::var_os("KIYORA_TDJSON").expect("KIYORA_TDJSON is required"),
    ))
    .await
    .unwrap();
    client.verify_version().await.unwrap();
    client.close().await.unwrap();
}
