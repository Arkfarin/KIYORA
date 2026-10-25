#![allow(clippy::unwrap_used)]
use super::*;

#[test]
fn sqlite_full_rolls_back_the_whole_batch_and_reopens_without_data_loss() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("bounded.sqlite");
    let binding = Binding {
        vault_id: Uuid::from_u128(1),
        owner_id: UserId::new(42).unwrap(),
        chat_id: ChatId::new(-100).unwrap(),
    };
    let mut db = Database::open(&path, binding).unwrap();
    let record = |id| {
        Record::Event(Box::new(Event {
            vault_id: binding.vault_id,
            op_id: Uuid::from_u128(id),
            device_id: "bounded-storage".into(),
            base_generation: 0,
            entity_id: Uuid::from_u128(id + 10_000),
            action: Action::PlaylistCreate {
                title: format!("Playlist {id}"),
            },
        }))
    };
    db.apply_record(MessageId::new(1).unwrap(), &record(1))
        .unwrap();
    db.set_local_setting("preserve", "local preference")
        .unwrap();
    let before = db.state().unwrap();
    let records: Vec<_> = (2..=500)
        .map(|id| (MessageId::new(id as i64).unwrap(), record(id)))
        .collect();
    let Record::Event(intent) = &records[0].1 else {
        unreachable!()
    };
    db.create_command(intent).unwrap();
    db.transition_command(intent.op_id, CommandStatus::Sending, None)
        .unwrap();

    // SQLite's actual page limit raises SQLITE_FULL without filling the host
    // filesystem or replacing the database implementation with a fake.
    let pages: u32 = db
        .connection
        .query_row("PRAGMA page_count", [], |row| row.get(0))
        .unwrap();
    db.connection
        .pragma_update(None, "max_page_count", pages)
        .unwrap();
    let error = db.apply_records(&records).unwrap_err();
    assert!(
        matches!(error, DbError::Sql(rusqlite::Error::SqliteFailure(ref code, _)) if code.code == rusqlite::ErrorCode::DiskFull)
    );
    assert_eq!(db.state().unwrap(), before);
    assert_eq!(db.cursor().unwrap(), 1);
    assert_eq!(
        db.command(intent.op_id).unwrap().unwrap().status,
        CommandStatus::Sending
    );
    db.validate().unwrap();
    drop(db);

    let mut db = Database::open(&path, binding).unwrap();
    assert_eq!(db.state().unwrap(), before);
    assert_eq!(
        db.local_setting("preserve").unwrap().as_deref(),
        Some("local preference")
    );
    db.connection
        .pragma_update(None, "max_page_count", pages + 10_000)
        .unwrap();
    db.apply_records(&records).unwrap();
    assert_eq!(db.cursor().unwrap(), 500);
    assert_eq!(db.state().unwrap().playlists.len(), 500);
    assert_eq!(db.state().unwrap().applied.len(), 500);
    assert_eq!(
        db.command(intent.op_id).unwrap().unwrap().status,
        CommandStatus::Applied
    );
    db.validate().unwrap();
}
