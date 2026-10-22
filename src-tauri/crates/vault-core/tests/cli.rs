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
fn cli_help_is_standalone_and_lists_backend_operations() {
    let dir = tempdir().unwrap();
    let result = run(&dir.path().to_path_buf(), &["--help"]);
    assert!(result.status.success());
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.contains("vault discover|attach"));
    assert!(text.contains("backup verify"));
    assert!(text.contains("media import-incoming"));
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
}

#[test]
fn cli_invalid_command_returns_documented_validation_exit() {
    let dir = tempdir().unwrap();
    let result = run(&dir.path().to_path_buf(), &["not-a-command"]);
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("InvalidInput"));
}
