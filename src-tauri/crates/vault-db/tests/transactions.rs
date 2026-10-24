#![allow(clippy::unwrap_used)]
use uuid::Uuid;
use vault_db::*;
use vault_domain::*;
use vault_protocol::*;

fn binding() -> Binding {
    Binding {
        vault_id: Uuid::from_u128(1),
        owner_id: UserId::new(42).unwrap(),
        chat_id: ChatId::new(-1001234567890).unwrap(),
    }
}
fn create(n: u128) -> Record {
    Record::Event(Box::new(Event {
        vault_id: binding().vault_id,
        op_id: Uuid::from_u128(n),
        device_id: "test".into(),
        base_generation: 0,
        entity_id: Uuid::from_u128(20),
        action: Action::PlaylistCreate {
            title: "Night music".into(),
        },
    }))
}

fn rename(n: u128) -> Record {
    Record::Event(Box::new(Event {
        vault_id: binding().vault_id,
        op_id: Uuid::from_u128(n),
        device_id: "test".into(),
        base_generation: 0,
        entity_id: Uuid::from_u128(20),
        action: Action::PlaylistRename {
            title: "Renamed in batch".into(),
        },
    }))
}

#[test]
fn ordered_record_batch_preserves_application_order_and_cursor() {
    let mut db = Database::memory(binding()).unwrap();
    let records = [
        (MessageId::new(9).unwrap(), create(3)),
        (MessageId::new(12).unwrap(), rename(4)),
    ];
    assert_eq!(
        db.apply_records(&records).unwrap(),
        vec![Some(Outcome::Applied), Some(Outcome::Applied)]
    );
    assert_eq!(db.cursor().unwrap(), 12);
    let state = db.state().unwrap();
    assert_eq!(
        state.playlists[&Uuid::from_u128(20)].title,
        "Renamed in batch"
    );
    assert_eq!(state.applied.len(), 2);
}

#[test]
fn repeated_message_in_batch_returns_the_original_outcome() {
    let mut db = Database::memory(binding()).unwrap();
    let records = [
        (MessageId::new(9).unwrap(), create(3)),
        (MessageId::new(9).unwrap(), create(3)),
        (MessageId::new(12).unwrap(), create(3)),
        (MessageId::new(15).unwrap(), rename(4)),
    ];
    let expected = vec![Some(Outcome::Applied); records.len()];
    assert_eq!(db.apply_records(&records).unwrap(), expected);
    let state = db.state().unwrap();
    assert_eq!(state.applied.len(), 2);
    assert_eq!(state.applied[&Uuid::from_u128(3)].message_id.get(), 9);
    assert_eq!(db.apply_records(&records).unwrap(), expected);
    assert_eq!(db.state().unwrap(), state);
    assert_eq!(db.cursor().unwrap(), 15);
}

#[test]
fn record_batch_rolls_back_all_events_and_cursor_on_mid_batch_failure() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("batch.sqlite");
    let mut db = Database::open(&path, binding()).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_second BEFORE INSERT ON applied_events WHEN NEW.message_id=12 BEGIN SELECT RAISE(ABORT,'injected batch crash'); END;").unwrap();
    let records = [
        (MessageId::new(9).unwrap(), create(3)),
        (MessageId::new(12).unwrap(), rename(4)),
    ];
    for (_, record) in &records {
        if let Record::Event(event) = record {
            db.create_command(event).unwrap();
            db.transition_command(event.op_id, CommandStatus::Sending, None)
                .unwrap();
        }
    }
    assert!(db.apply_records(&records).is_err());
    drop(db);
    let mut db = Database::open(&path, binding()).unwrap();
    assert_eq!(db.cursor().unwrap(), 0);
    assert!(db.state().unwrap().playlists.is_empty());
    assert!(db.state().unwrap().applied.is_empty());
    assert!(db.playlists(Some("batch"), 0, 10).unwrap().is_empty());
    assert_eq!(
        conn.query_row("SELECT count(*) FROM observed_messages", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    for id in [3, 4] {
        assert_eq!(
            db.command(Uuid::from_u128(id)).unwrap().unwrap().status,
            CommandStatus::Sending
        );
    }
    conn.execute_batch("DROP TRIGGER fail_second;").unwrap();
    assert_eq!(db.apply_records(&records).unwrap().len(), 2);
    assert_eq!(db.cursor().unwrap(), 12);
    for id in [3, 4] {
        assert_eq!(
            db.command(Uuid::from_u128(id)).unwrap().unwrap().status,
            CommandStatus::Applied
        );
    }
}

#[test]
fn invalid_batch_never_commits_a_valid_prefix() {
    let mut foreign = rename(4);
    if let Record::Event(event) = &mut foreign {
        event.vault_id = Uuid::from_u128(999);
    }
    for (id, record) in [
        (8, rename(4)),
        (9, rename(4)),
        (12, rename(3)),
        (12, foreign),
    ] {
        let mut db = Database::memory(binding()).unwrap();
        let result = db.apply_records(&[
            (MessageId::new(9).unwrap(), create(3)),
            (MessageId::new(id).unwrap(), record),
        ]);
        assert!(result.is_err());
        assert_eq!(db.cursor().unwrap(), 0);
        assert_eq!(db.state().unwrap(), State::new(binding().vault_id));
        // A retry proves the failed batch left no observed-message marker behind.
        assert_eq!(
            db.apply_record(MessageId::new(9).unwrap(), &create(3))
                .unwrap(),
            Some(Outcome::Applied)
        );
    }
}

#[test]
fn sqlite_failure_rolls_back_projection_outcome_and_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.sqlite");
    let mut db = Database::open(&path, binding()).unwrap();
    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_outcome BEFORE INSERT ON applied_events BEGIN SELECT RAISE(ABORT,'injected crash'); END;").unwrap();
    assert!(
        db.apply_record(MessageId::new(9).unwrap(), &create(3))
            .is_err()
    );
    assert_eq!(db.cursor().unwrap(), 0);
    assert!(db.state().unwrap().playlists.is_empty());
    assert!(db.state().unwrap().applied.is_empty());
    conn.execute_batch("DROP TRIGGER fail_outcome;").unwrap();
    db.apply_record(MessageId::new(9).unwrap(), &create(3))
        .unwrap();
    drop(db);
    let db = Database::open(&path, binding()).unwrap();
    assert_eq!(db.cursor().unwrap(), 9);
    assert_eq!(db.state().unwrap().playlists.len(), 1);
    assert_eq!(db.state().unwrap().applied.len(), 1);
}

#[test]
fn numeric_order_exact_roundtrip_and_unknown_protocol_does_not_advance() {
    let mut db = Database::memory(binding()).unwrap();
    let r = create(3);
    for mid in [9, 10, 9_007_199_254_740_993, i64::MAX] {
        db.apply_record(MessageId::new(mid).unwrap(), &r).unwrap();
    }
    assert_eq!(db.cursor().unwrap(), i64::MAX);
    assert_eq!(db.state().unwrap().applied.len(), 1);
    let mut unknown =
        serde_json::from_slice::<serde_json::Value>(&encode(&create(4)).unwrap()).unwrap();
    unknown["protocol"] = serde_json::json!(2);
    assert!(
        db.apply_wire(
            MessageId::new(100).unwrap(),
            &serde_json::to_vec(&unknown).unwrap()
        )
        .is_err()
    );
    assert_eq!(db.cursor().unwrap(), i64::MAX);
}

#[test]
fn fts_search_is_local_paginated_and_survives_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.sqlite");
    let mut db = Database::open(&path, binding()).unwrap();
    db.apply_record(MessageId::new(10).unwrap(), &create(3))
        .unwrap();
    assert_eq!(db.playlists(Some("Night"), 0, 10).unwrap().len(), 1);
    assert!(db.playlists(Some("Night"), 1, 10).unwrap().is_empty());
    assert!(db.playlists(Some("\" OR *"), 0, 10).unwrap().is_empty());
    drop(db);
    let db = Database::open(&path, binding()).unwrap();
    assert_eq!(db.playlists(Some("music"), 0, 10).unwrap().len(), 1);
    let mut wrong = binding();
    wrong.owner_id = UserId::new(43).unwrap();
    assert!(Database::open(&path, wrong).is_err());
}

#[test]
fn ack_is_not_applied_and_unknown_commands_cannot_resend() {
    let mut db = Database::memory(binding()).unwrap();
    let Record::Event(event) = create(3) else {
        panic!("fixture");
    };
    db.create_command(&event).unwrap();
    db.transition_command(event.op_id, CommandStatus::Sending, None)
        .unwrap();
    db.transition_command(
        event.op_id,
        CommandStatus::ServerAck,
        Some(MessageId::new(10).unwrap()),
    )
    .unwrap();
    assert_eq!(
        db.command(event.op_id).unwrap().unwrap().status,
        CommandStatus::ServerAck
    );
    db.mark_interrupted_commands_unknown().unwrap();
    assert!(
        db.transition_command(event.op_id, CommandStatus::Sending, None)
            .is_err()
    );
    db.apply_record(MessageId::new(10).unwrap(), &Record::Event(event.clone()))
        .unwrap();
    assert_eq!(
        db.command(event.op_id).unwrap().unwrap().status,
        CommandStatus::Applied
    );
    assert!(
        db.transition_command(event.op_id, CommandStatus::Sending, None)
            .is_err()
    );
}

#[test]
fn snapshot_restore_is_atomic_and_preserves_local_settings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("vault.sqlite");
    let mut db = Database::open(&path, binding()).unwrap();
    db.apply_record(MessageId::new(10).unwrap(), &create(3))
        .unwrap();
    db.set_local_setting("volume", "0.5").unwrap();
    let mut snapshot = db.state().unwrap();
    snapshot.generation = 1;
    let commit = SnapshotCommit {
        vault_id: binding().vault_id,
        snapshot_id: Uuid::from_u128(50),
        generation: 1,
        barrier_message_id: MessageId::new(20).unwrap(),
        snapshot_a_message_id: MessageId::new(30).unwrap(),
        snapshot_b_message_id: MessageId::new(40).unwrap(),
        sha256_uncompressed: "a".repeat(64),
        sha256_compressed: "b".repeat(64),
        previous_commit_message_id: None,
    };
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_snapshot BEFORE INSERT ON snapshot_registry BEGIN SELECT RAISE(ABORT,'injected crash'); END;").unwrap();
    assert!(
        db.restore(
            &snapshot,
            commit.barrier_message_id,
            MessageId::new(50).unwrap(),
            &commit
        )
        .is_err()
    );
    assert_eq!(db.cursor().unwrap(), 10);
    assert_eq!(db.state().unwrap().generation, 0);
    connection
        .execute_batch("DROP TRIGGER fail_snapshot;")
        .unwrap();
    db.restore(
        &snapshot,
        commit.barrier_message_id,
        MessageId::new(50).unwrap(),
        &commit,
    )
    .unwrap();
    assert_eq!(db.state().unwrap(), snapshot);
    assert_eq!(db.cursor().unwrap(), 20);
    assert_eq!(db.local_setting("volume").unwrap().as_deref(), Some("0.5"));
}

#[test]
fn future_schema_is_refused_without_changing_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("future.sqlite");
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection.execute_batch("PRAGMA user_version=2;").unwrap();
    let original = std::fs::read(&path).unwrap();
    assert!(matches!(
        Database::open(&path, binding()),
        Err(DbError::Schema)
    ));
    assert_eq!(
        connection
            .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        2
    );
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(
        connection
            .query_row("PRAGMA journal_mode", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "delete"
    );
}
