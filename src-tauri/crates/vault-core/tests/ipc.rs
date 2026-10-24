#![allow(clippy::unwrap_used)]
use tokio_util::sync::CancellationToken;
use vault_core::*;
use vault_security::ProfileStore;

#[tokio::test]
async fn state_snapshot_is_local_consistent_and_rejects_an_old_profile_session() {
    use uuid::Uuid;
    use vault_domain::*;
    let root = tempfile::tempdir().unwrap();
    let backend = Backend::new(ProfileStore::at(root.path()).unwrap(), "missing.so");
    let profile = backend.create_profile("Snapshot".into()).unwrap();
    let old = backend.switch_profile(profile.profile_id).await.unwrap();
    backend.shutdown().await.unwrap();
    let directory = root
        .path()
        .join("profiles")
        .join(profile.profile_id.to_string());
    let binding = vault_db::Binding {
        vault_id: Uuid::new_v4(),
        owner_id: UserId::new(42).unwrap(),
        chat_id: ChatId::new(-100).unwrap(),
    };
    let path = directory.join("backend.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["binding"] = serde_json::to_value(binding).unwrap();
    std::fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();
    let mut db = vault_db::Database::open(directory.join("vault.sqlite"), binding).unwrap();
    let id = Uuid::new_v4();
    db.apply_record(
        MessageId::new(10485760).unwrap(),
        &vault_protocol::Record::Event(Box::new(Event {
            vault_id: binding.vault_id,
            op_id: Uuid::new_v4(),
            device_id: "fixture".into(),
            base_generation: 0,
            entity_id: id,
            action: Action::PlaylistDelete {},
        })),
    )
    .unwrap();
    let expected = db.state().unwrap();
    drop(db);
    let current = backend.switch_profile(profile.profile_id).await.unwrap();
    let response = backend
        .invoke(
            BackendRequest {
                version: 1,
                request: BackendCommand::StateSnapshot {
                    session_id: current.context.session_id,
                },
            },
            &CancellationToken::new(),
        )
        .await;
    let BackendReply::StateSnapshot(snapshot) = response.result.unwrap() else {
        panic!("snapshot expected")
    };
    assert_eq!(snapshot.context, current.context);
    assert_eq!(snapshot.cursor, "10485760");
    assert_eq!(snapshot.state, expected);
    assert!(snapshot.state.tombstones.playlists.contains(&id));
    assert_eq!(
        snapshot.event_sequence,
        backend.events().sequence().unwrap()
    );
    assert_eq!(
        backend
            .state_snapshot(old.context.session_id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    backend.shutdown().await.unwrap();
}

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
