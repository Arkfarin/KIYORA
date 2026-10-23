#![allow(clippy::unwrap_used)]
use tokio_util::sync::CancellationToken;
use vault_core::*;
use vault_security::ProfileStore;

#[tokio::test]
async fn versioned_json_boundary_dispatches_profiles_and_offline_cache_with_structured_errors() {
    let dir = tempfile::tempdir().unwrap();
    let backend = Backend::new(ProfileStore::at(dir.path()).unwrap(), "missing.so");
    let token = CancellationToken::new();
    let rejected = backend.invoke(serde_json::from_value(serde_json::json!({"version":2,"request":{"command":"create_profile","args":{"label":"must not exist"}}})).unwrap(), &token).await;
    assert_eq!(rejected.result.unwrap_err().code, ErrorCode::InvalidInput);
    assert!(backend.profiles().unwrap().is_empty());
    let response = backend.invoke(serde_json::from_value(serde_json::json!({"version":1,"request":{"command":"create_profile","args":{"label":"IPC"}}})).unwrap(), &token).await;
    let BackendReply::Profile(profile) = response.result.unwrap() else {
        panic!("profile reply expected");
    };
    let response = backend
        .invoke(
            BackendRequest {
                version: 1,
                request: BackendCommand::SwitchProfile {
                    profile_id: profile.profile_id,
                },
            },
            &token,
        )
        .await;
    let BackendReply::Status(status) = response.result.unwrap() else {
        panic!("status reply expected");
    };
    let response = backend
        .invoke(
            BackendRequest {
                version: 1,
                request: BackendCommand::Vault {
                    session_id: status.context.session_id,
                    request: VaultRequest::GetCacheStats,
                },
            },
            &token,
        )
        .await;
    let encoded = serde_json::to_value(response).unwrap();
    assert_eq!(encoded["version"], 1);
    assert_eq!(encoded["result"]["Ok"]["kind"], "vault");
    assert_eq!(encoded["result"]["Ok"]["result"]["kind"], "cache_stats");
    assert!(serde_json::from_value::<BackendRequest>(serde_json::json!({"version":1,"request":{"command":"create_profile","args":{"label":"bad","unexpected":true}}})).is_err());
    backend.shutdown().await.unwrap();
}

#[test]
fn chat_ids_preserve_decimal_strings_at_the_public_boundary() {
    let call: BackendRequest = serde_json::from_value(serde_json::json!({"version":1,"request":{"command":"attach_vault","args":{"session_id":uuid::Uuid::new_v4(),"chat_id":"-9007199254740993"}}})).unwrap();
    let encoded = serde_json::to_value(call).unwrap();
    assert_eq!(encoded["request"]["args"]["chat_id"], "-9007199254740993");
    let schema = serde_json::to_value(schemars::schema_for!(BackendResponse)).unwrap();
    assert_eq!(
        schema["definitions"]["ChannelInfo"]["properties"]["chat_id"]["type"],
        "string"
    );
}

#[test]
fn checked_in_schemas_match_the_runtime_boundary() {
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../spec/schema");
    for (name, schema) in [
        (
            "backend-request-v1.schema.json",
            serde_json::to_value(schemars::schema_for!(BackendRequest)).unwrap(),
        ),
        (
            "backend-response-v1.schema.json",
            serde_json::to_value(schemars::schema_for!(BackendResponse)).unwrap(),
        ),
        (
            "backend-event-v1.schema.json",
            serde_json::to_value(schemars::schema_for!(BackendEvent)).unwrap(),
        ),
    ] {
        let stored: serde_json::Value =
            serde_json::from_slice(&std::fs::read(directory.join(name)).unwrap()).unwrap();
        assert_eq!(stored, schema, "schema drift: {name}");
    }
}
