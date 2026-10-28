#![allow(clippy::unwrap_used)]
use std::{collections::BTreeMap, fs::File, io::Write};
use uuid::Uuid;
use vault_domain::*;
use vault_snapshot::backup::*;

#[test]
fn metadata_backup_roundtrips_without_secrets_and_never_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kiyora.tmv-backup");
    let state = State::new(Uuid::new_v4());
    export(
        &path,
        &state,
        UserId::new(42).unwrap(),
        None,
        BackupKind::Metadata,
        &BTreeMap::new(),
        Limits::default(),
    )
    .unwrap();
    let backup = inspect(&path, Limits::default()).unwrap();
    assert_eq!(backup.state, state);
    assert_eq!(backup.manifest.entries.len(), 2);
    assert!(
        export(
            &path,
            &state,
            UserId::new(42).unwrap(),
            None,
            BackupKind::Metadata,
            &BTreeMap::new(),
            Limits::default()
        )
        .is_err()
    );
    assert!(
        inspect(
            &path,
            Limits {
                total_bytes: 1,
                ..Limits::default()
            }
        )
        .is_err()
    );
}
#[test]
fn archive_rejects_traversal_links_duplicates_and_oversized_headers() {
    for variant in 0..4 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.tmv-backup");
        let mut file = File::create(&path).unwrap();
        let mut header = tar::Header::new_ustar();
        header.set_mode(0o600);
        header.set_size(1);
        if variant == 0 {
            header.as_mut_bytes()[..12].copy_from_slice(b"../escape123");
        } else {
            header.set_path("manifest.json").unwrap();
        }
        if variant == 1 {
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_link_name("/tmp/escape").unwrap();
        }
        if variant == 3 {
            header.set_size(8 * 1024 * 1024);
        }
        header.set_cksum();
        file.write_all(header.as_bytes()).unwrap();
        file.write_all(&[0; 512]).unwrap();
        if variant == 2 {
            file.write_all(header.as_bytes()).unwrap();
            file.write_all(&[0; 512]).unwrap();
        }
        file.write_all(&[0; 1024]).unwrap();
        drop(file);
        assert!(inspect(&path, Limits::default()).is_err());
        assert!(!dir.path().join("escape123").exists());
    }
}
#[test]
fn full_backup_verifies_media_and_restore_requires_new_references() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("audio.flac");
    std::fs::write(&source, b"media fixture").unwrap();
    let mut state = State::new(Uuid::new_v4());
    let track = Uuid::new_v4();
    let chat = ChatId::new(-100).unwrap();
    state
        .apply(
            MessageId::new(10).unwrap(),
            &Event {
                vault_id: state.vault_id,
                op_id: Uuid::new_v4(),
                device_id: "test".into(),
                base_generation: 0,
                entity_id: track,
                action: Action::TrackRegister {
                    metadata: Metadata {
                        title: "Music".into(),
                        ..Default::default()
                    },
                    source: Source {
                        source_id: Uuid::new_v4(),
                        chat_id: chat,
                        message_id: MessageId::new(7).unwrap(),
                        sha256: Some(vault_snapshot::digest(b"media fixture")),
                        size_bytes: Some(13),
                    },
                },
            },
        )
        .unwrap();
    let path = dir.path().join("full.tmv-backup");
    export(
        &path,
        &state,
        UserId::new(42).unwrap(),
        None,
        BackupKind::Full,
        &BTreeMap::from([(track, source)]),
        Limits::default(),
    )
    .unwrap();
    let backup = inspect(&path, Limits::default()).unwrap();
    assert_eq!(
        std::fs::read(backup.media_path(track).unwrap()).unwrap(),
        b"media fixture"
    );
    let new_vault = Uuid::new_v4();
    let new_chat = ChatId::new(-200).unwrap();
    assert!(plan_new_vault(&backup, new_vault, new_chat, &BTreeMap::new()).is_err());
    let plan = plan_new_vault(
        &backup,
        new_vault,
        new_chat,
        &BTreeMap::from([(
            track,
            Source {
                source_id: Uuid::new_v4(),
                chat_id: new_chat,
                message_id: MessageId::new(24).unwrap(),
                sha256: Some(vault_snapshot::digest(b"media fixture")),
                size_bytes: Some(13),
            },
        )]),
    )
    .unwrap();
    let mut restored = State::new(new_vault);
    for (i, operation) in plan.operations.into_iter().enumerate() {
        assert_eq!(
            restored
                .apply(
                    MessageId::new(100 + i as i64).unwrap(),
                    &Event {
                        vault_id: new_vault,
                        op_id: operation.op_id,
                        device_id: "restore".into(),
                        base_generation: 0,
                        entity_id: operation.entity_id,
                        action: operation.action
                    }
                )
                .unwrap(),
            Outcome::Applied
        );
    }
    assert_eq!(restored.tracks.len(), 1);
    assert!(!restored.tracks.contains_key(&track));
    assert!(
        restored
            .tracks
            .values()
            .all(|t| t.sources.values().all(|s| s.chat_id == new_chat))
    );
}

#[test]
fn full_backup_carries_lyrics_and_new_vault_cannot_keep_old_references() {
    let dir = tempfile::tempdir().unwrap();
    let audio = dir.path().join("audio");
    let lyrics_path = dir.path().join("lyrics.json");
    std::fs::write(&audio, b"audio").unwrap();
    let document = LyricsDocument {
        version: 1,
        language: Some("ru".into()),
        lines: vec![LyricLine {
            start_ms: Some(1234),
            end_ms: Some(2400),
            text: "Строка с таймингом".into(),
        }],
    };
    let bytes = serde_json::to_vec(&document).unwrap();
    std::fs::write(&lyrics_path, &bytes).unwrap();
    let old_chat = ChatId::new(-100).unwrap();
    let track = Uuid::new_v4();
    let source = Source {
        source_id: Uuid::new_v4(),
        chat_id: old_chat,
        message_id: MessageId::new(1).unwrap(),
        sha256: Some(vault_snapshot::digest(b"audio")),
        size_bytes: Some(5),
    };
    let reference = LyricsReference {
        sha256: vault_snapshot::digest(&bytes),
        source: Source {
            source_id: Uuid::new_v4(),
            chat_id: old_chat,
            message_id: MessageId::new(2).unwrap(),
            sha256: Some(vault_snapshot::digest(&bytes)),
            size_bytes: Some(bytes.len() as u64),
        },
    };
    let mut state = State::new(Uuid::new_v4());
    state
        .apply(
            MessageId::new(3).unwrap(),
            &Event {
                vault_id: state.vault_id,
                op_id: Uuid::new_v4(),
                device_id: "test".into(),
                base_generation: 0,
                entity_id: track,
                action: Action::TrackRegister {
                    metadata: Metadata {
                        title: "Трек".into(),
                        lyrics: Some(reference.clone()),
                        ..Default::default()
                    },
                    source: source.clone(),
                },
            },
        )
        .unwrap();
    let archive = dir.path().join("complete.tmv-backup");
    assert!(
        export(
            &archive,
            &state,
            UserId::new(42).unwrap(),
            None,
            BackupKind::Full,
            &BTreeMap::from([(track, audio.clone())]),
            Limits::default()
        )
        .is_err()
    );
    export_with_attachments(
        &archive,
        &state,
        UserId::new(42).unwrap(),
        None,
        BackupKind::Full,
        &BTreeMap::from([(track, audio)]),
        &BTreeMap::new(),
        &BTreeMap::from([(track, lyrics_path)]),
        Limits::default(),
    )
    .unwrap();
    let backup = inspect(&archive, Limits::default()).unwrap();
    assert_eq!(
        std::fs::read(backup.lyrics_path(track).unwrap()).unwrap(),
        bytes
    );
    let new_vault = Uuid::new_v4();
    let new_chat = ChatId::new(-200).unwrap();
    let new_source = Source {
        source_id: Uuid::new_v4(),
        chat_id: new_chat,
        ..source
    };
    let sources = BTreeMap::from([(track, new_source)]);
    assert!(plan_new_vault(&backup, new_vault, new_chat, &sources).is_err());
    assert!(
        plan_new_vault_with_lyrics(
            &backup,
            new_vault,
            new_chat,
            &sources,
            &BTreeMap::from([(track, reference.source.clone())])
        )
        .is_err()
    );
    let new_lyrics = Source {
        source_id: Uuid::new_v4(),
        chat_id: new_chat,
        message_id: MessageId::new(9).unwrap(),
        ..reference.source
    };
    let plan = plan_new_vault_with_lyrics(
        &backup,
        new_vault,
        new_chat,
        &sources,
        &BTreeMap::from([(track, new_lyrics.clone())]),
    )
    .unwrap();
    let Action::TrackRegister { metadata, .. } = &plan.operations[0].action else {
        panic!("expected registration")
    };
    assert_eq!(metadata.lyrics.as_ref().unwrap().source, new_lyrics);
}
