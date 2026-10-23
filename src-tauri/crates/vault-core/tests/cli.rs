#![allow(clippy::unwrap_used)]
use std::{
    path::PathBuf,
    process::{Command, Output},
};
use tempfile::tempdir;

fn run(data: &PathBuf, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_kiyora-cli"))
        .env("KIYORA_DATA_DIR", data)
        .env("KIYORA_TDJSON", "unavailable-tdjson.so")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn persistent_stdio_keeps_profile_context_and_recovers_from_bad_requests() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::Stdio;
    let directory = tempdir().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_kiyora-cli"))
        .env("KIYORA_DATA_DIR", directory.path())
        .env("KIYORA_TDJSON", "unavailable-tdjson.so")
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let mut call = |request: serde_json::Value| {
        writeln!(
            input,
            "{}",
            serde_json::json!({"version":1,"request":request})
        )
        .unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        serde_json::from_str::<serde_json::Value>(&line).unwrap()["result"].clone()
    };
    let created =
        call(serde_json::json!({"command":"create_profile","args":{"label":"Persistent CLI"}}));
    let id = created["Ok"]["result"]["profile_id"].clone();
    let switched = call(serde_json::json!({"command":"switch_profile","args":{"profile_id":id}}));
    assert!(switched.get("Ok").is_some());
    assert!(
        call(serde_json::json!({"command":"invalid"}))
            .get("Err")
            .is_some()
    );
    let status = call(serde_json::json!({"command":"runtime_status"}));
    assert_eq!(
        status["Ok"]["result"]["context"],
        switched["Ok"]["result"]["context"]
    );
    drop(input);
    assert!(child.wait().unwrap().success());
}

#[test]
fn cli_help_is_standalone_and_lists_backend_operations() {
    let dir = tempdir().unwrap();
    let result = run(&dir.path().to_path_buf(), &["--help"]);
    assert!(result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("vault discover|attach"));
    assert!(text.contains("backup verify"));
    assert!(text.contains("media import-incoming"));
    assert!(text.contains("playlist show <id>"));
    assert!(text.contains("library favorite"));
    assert!(text.contains("library list|search"));
    assert!(text.contains("share <track-id> <source-id> <recipient-chat-id>"));
    assert!(text.contains("playback play <track-id> <source-id>"));
}

#[test]
fn cli_profile_create_switch_and_status_use_shared_durable_store() {
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let created = run(&root, &["profiles", "create", "Test profile"]);
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let id = String::from_utf8(created.stdout).unwrap().trim().to_owned();
    assert!(!id.is_empty());
    let switched = run(&root, &["profiles", "switch", &id]);
    assert!(
        switched.status.success(),
        "{}",
        String::from_utf8_lossy(&switched.stderr)
    );
    assert!(String::from_utf8_lossy(&switched.stdout).contains("AuthRequired"));
    let status = run(&root, &["status"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert!(String::from_utf8_lossy(&status.stdout).contains(&id));
    let status_json = run(&root, &["status", "--json"]);
    assert!(status_json.status.success());
    let status: serde_json::Value = serde_json::from_slice(&status_json.stdout).unwrap();
    assert_eq!(status["context"]["profile_id"], id);
    assert_eq!(status["mode"], "AUTH_REQUIRED");
    let profiles_json = run(&root, &["profiles", "list", "--json"]);
    assert!(profiles_json.status.success());
    let profiles: serde_json::Value = serde_json::from_slice(&profiles_json.stdout).unwrap();
    assert_eq!(profiles[0]["profile_id"], id);
}

#[test]
fn cli_playlist_show_uses_domain_order_from_the_offline_projection() {
    use uuid::Uuid;
    use vault_domain::*;
    let dir = tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let created = run(&root, &["profiles", "create", "Offline playlist"]);
    assert!(created.status.success());
    let profile_id = String::from_utf8(created.stdout).unwrap().trim().to_owned();
    assert!(
        run(&root, &["profiles", "switch", &profile_id])
            .status
            .success()
    );
    let directory = root.join("profiles").join(&profile_id);
    let binding = vault_db::Binding {
        vault_id: Uuid::new_v4(),
        owner_id: UserId::new(42).unwrap(),
        chat_id: ChatId::new(-100).unwrap(),
    };
    let config_path = directory.join("backend.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    config["binding"] = serde_json::to_value(binding).unwrap();
    std::fs::write(config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let mut db = vault_db::Database::open(directory.join("vault.sqlite"), binding).unwrap();
    let playlist = Uuid::new_v4();
    let track = Uuid::new_v4();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let c = Uuid::new_v4();
    for (i, (entity_id, action)) in [
        (
            track,
            Action::TrackRegister {
                metadata: Metadata {
                    title: "Track".into(),
                    ..Default::default()
                },
                source: Source {
                    source_id: Uuid::new_v4(),
                    chat_id: binding.chat_id,
                    message_id: MessageId::new(1).unwrap(),
                    sha256: None,
                    size_bytes: None,
                },
            },
        ),
        (
            playlist,
            Action::PlaylistCreate {
                title: "Order".into(),
            },
        ),
        (
            playlist,
            Action::PlaylistTrackAdd {
                track_id: track,
                entry_id: a,
                after_entry_id: None,
            },
        ),
        (
            playlist,
            Action::PlaylistTrackAdd {
                track_id: track,
                entry_id: b,
                after_entry_id: None,
            },
        ),
        (
            playlist,
            Action::PlaylistTrackAdd {
                track_id: track,
                entry_id: c,
                after_entry_id: Some(a),
            },
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let event = Event {
            vault_id: binding.vault_id,
            op_id: Uuid::new_v4(),
            entity_id,
            action,
            device_id: "fixture".into(),
            base_generation: 0,
        };
        db.apply_record(
            MessageId::new(i as i64 + 10).unwrap(),
            &vault_protocol::Record::Event(Box::new(event)),
        )
        .unwrap();
    }
    drop(db);
    let output = run(&root, &["playlist", "show", &playlist.to_string()]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let order = stdout
        .lines()
        .skip(1)
        .map(|line| line.split('\t').next().unwrap().to_owned())
        .collect::<Vec<_>>();
    assert_eq!(order, [a.to_string(), c.to_string(), b.to_string()]);
}

#[test]
fn cli_invalid_command_returns_documented_validation_exit() {
    let dir = tempdir().unwrap();
    let result = run(&dir.path().to_path_buf(), &["not-a-command"]);
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("InvalidInput"));
}

#[test]
fn cli_simulation_runs_three_clients_without_account_credentials() {
    let temp = tempdir().unwrap();
    let result = run(&temp.path().to_owned(), &["simulate", "12"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(report["clients"], 3);
    assert_eq!(report["converged"], true);
    assert_eq!(report["cold_recovery"], true);
}
