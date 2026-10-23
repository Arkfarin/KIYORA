#![allow(clippy::unwrap_used)]
use proptest::prelude::*;
use uuid::Uuid;
use vault_domain::*;

fn id(n: u128) -> Uuid {
    Uuid::from_u128(n)
}
fn event(n: u128, entity: u128, action: Action) -> Event {
    Event {
        vault_id: id(1),
        op_id: id(n + 1000),
        device_id: "test-device".into(),
        base_generation: 0,
        entity_id: id(entity),
        action,
    }
}
fn apply(state: &mut State, n: i64, entity: u128, action: Action) -> Outcome {
    state
        .apply(
            MessageId::new(n).unwrap(),
            &event(n as u128, entity, action),
        )
        .unwrap()
}
fn track() -> Action {
    Action::TrackRegister {
        metadata: Metadata {
            title: "Music".into(),
            ..Default::default()
        },
        source: Source {
            source_id: id(400),
            chat_id: ChatId::new(-100).unwrap(),
            message_id: MessageId::new(7).unwrap(),
            sha256: None,
            size_bytes: None,
        },
    }
}

#[test]
fn large_entry_forest_rejects_cycles_or_cross_playlist_anchors_without_recursion() {
    let mut state = State::new(id(1));
    apply(&mut state, 10, 20, track());
    apply(
        &mut state,
        11,
        30,
        Action::PlaylistCreate {
            title: "Long chain".into(),
        },
    );
    apply(
        &mut state,
        12,
        31,
        Action::PlaylistCreate {
            title: "Another".into(),
        },
    );
    for i in 0..20_000_u128 {
        let entry_id = id(100_000 + i);
        state.entries.insert(
            entry_id,
            PlaylistEntry {
                entry_id,
                playlist_id: id(30),
                track_id: id(20),
                after_entry_id: (i != 0).then(|| id(99_999 + i)),
                order_key: MessageId::new(12).unwrap(),
            },
        );
    }
    state.validate().unwrap();
    state.entries.get_mut(&id(100_000)).unwrap().after_entry_id = Some(id(119_999));
    assert!(state.validate().is_err());
    state.entries.get_mut(&id(100_000)).unwrap().after_entry_id = None;
    state.entries.get_mut(&id(119_999)).unwrap().playlist_id = id(31);
    assert!(state.validate().is_err());
    state.entries.get_mut(&id(119_999)).unwrap().after_entry_id = None;
    state.validate().unwrap();
    state.entries.get_mut(&id(119_999)).unwrap().after_entry_id = Some(id(999_999));
    assert!(state.validate().is_err());
}

#[test]
fn duplicate_operation_is_a_pure_no_change_and_collision_fails() {
    let mut s = State::new(id(1));
    let e = event(
        1,
        10,
        Action::PlaylistCreate {
            title: "Old".into(),
        },
    );
    let result = s.apply(MessageId::new(10).unwrap(), &e).unwrap();
    let before = s.clone();
    assert_eq!(s.apply(MessageId::new(99).unwrap(), &e).unwrap(), result);
    assert_eq!(s, before);
    let mut conflict = e;
    conflict.action = Action::PlaylistDelete {};
    assert_eq!(
        s.apply(MessageId::new(100).unwrap(), &conflict),
        Err(DomainError::OperationCollision)
    );
    assert_eq!(s, before);
}

#[test]
fn delete_wins_even_before_create_and_removes_related_entries() {
    let mut s = State::new(id(1));
    apply(&mut s, 10, 20, Action::PlaylistDelete {});
    assert_eq!(
        apply(
            &mut s,
            20,
            20,
            Action::PlaylistCreate {
                title: "Zombie".into()
            }
        ),
        Outcome::Rejected(RejectReason::Deleted)
    );
    apply(&mut s, 30, 30, track());
    apply(
        &mut s,
        40,
        40,
        Action::PlaylistCreate {
            title: "Live".into(),
        },
    );
    apply(
        &mut s,
        50,
        40,
        Action::PlaylistTrackAdd {
            track_id: id(30),
            entry_id: id(50),
            after_entry_id: None,
        },
    );
    apply(
        &mut s,
        60,
        30,
        Action::FavoriteSet {
            set_name: "default".into(),
            value: true,
        },
    );
    apply(&mut s, 70, 30, Action::TrackDelete {});
    assert!(s.entries.is_empty());
    assert!(s.favorites.is_empty());
    assert!(s.tombstones.entries.contains(&id(50)));
    assert_eq!(
        apply(&mut s, 80, 30, track()),
        Outcome::Rejected(RejectReason::Deleted)
    );
    s.validate().unwrap();
}

#[test]
fn inserts_moves_removes_preserve_order_and_references() {
    let mut s = State::new(id(1));
    apply(&mut s, 10, 30, track());
    apply(
        &mut s,
        20,
        40,
        Action::PlaylistCreate {
            title: "Mix".into(),
        },
    );
    for (n, entry, after) in [
        (30, 50, None),
        (40, 51, Some(id(50))),
        (50, 52, Some(id(50))),
    ] {
        apply(
            &mut s,
            n,
            40,
            Action::PlaylistTrackAdd {
                track_id: id(30),
                entry_id: id(entry),
                after_entry_id: after,
            },
        );
    }
    assert_eq!(
        s.playlist_order(id(40)).unwrap(),
        vec![id(50), id(51), id(52)]
    );
    apply(
        &mut s,
        60,
        40,
        Action::PlaylistEntryMove {
            entry_id: id(50),
            after_entry_id: Some(id(52)),
        },
    );
    assert_eq!(
        s.playlist_order(id(40)).unwrap(),
        vec![id(51), id(52), id(50)]
    );
    apply(
        &mut s,
        70,
        40,
        Action::PlaylistTrackRemove { entry_id: id(52) },
    );
    assert_eq!(s.playlist_order(id(40)).unwrap(), vec![id(51), id(50)]);
    assert_eq!(
        apply(
            &mut s,
            80,
            40,
            Action::PlaylistTrackAdd {
                track_id: id(30),
                entry_id: id(52),
                after_entry_id: None
            }
        ),
        Outcome::Rejected(RejectReason::Deleted)
    );
    s.validate().unwrap();
}

#[test]
fn replay_order_and_wrong_vault_errors_do_not_mutate() {
    let mut s = State::new(id(1));
    apply(
        &mut s,
        100,
        40,
        Action::PlaylistCreate {
            title: "Mix".into(),
        },
    );
    let before = s.clone();
    assert_eq!(
        s.apply(
            MessageId::new(99).unwrap(),
            &event(2, 40, Action::PlaylistDelete {})
        ),
        Err(DomainError::OutOfOrder)
    );
    let mut e = event(3, 40, Action::PlaylistDelete {});
    e.vault_id = id(2);
    assert_eq!(
        s.apply(MessageId::new(101).unwrap(), &e),
        Err(DomainError::WrongVault)
    );
    assert_eq!(s, before);
}

#[test]
fn metadata_sources_favorites_settings_and_rejected_outcomes_are_complete() {
    let mut s = State::new(id(1));
    apply(&mut s, 10, 30, track());
    let patch = MetadataPatch {
        album: Some("Album".into()),
        album_artist: Some("Artist".into()),
        year: Some(2026),
        track_number: Some(7),
        ..Default::default()
    };
    assert_eq!(
        apply(&mut s, 20, 30, Action::TrackMetadataUpdate { patch }),
        Outcome::Applied
    );
    assert_eq!(s.tracks[&id(30)].metadata.album, "Album");
    assert_eq!(
        apply(
            &mut s,
            30,
            30,
            Action::TrackMetadataUpdate {
                patch: MetadataPatch {
                    title: Some("".into()),
                    ..Default::default()
                }
            }
        ),
        Outcome::Rejected(RejectReason::InvalidPayload)
    );
    assert_eq!(s.tracks[&id(30)].metadata.title, "Music");
    let source = Source {
        source_id: id(400),
        chat_id: ChatId::new(-200).unwrap(),
        message_id: MessageId::new(99).unwrap(),
        sha256: Some("a".repeat(64)),
        size_bytes: Some(1024),
    };
    apply(&mut s, 40, 30, Action::SourceRelink { source });
    assert_eq!(s.tracks[&id(30)].sources[&id(400)].message_id.get(), 99);
    apply(
        &mut s,
        50,
        30,
        Action::FavoriteSet {
            set_name: "default".into(),
            value: true,
        },
    );
    assert!(s.favorites["default"].contains(&id(30)));
    apply(
        &mut s,
        60,
        30,
        Action::FavoriteSet {
            set_name: "default".into(),
            value: false,
        },
    );
    assert!(s.favorites.is_empty());
    apply(
        &mut s,
        70,
        1,
        Action::VaultSettingsUpdate {
            settings: SharedSettings {
                gc_device_id: Some("desktop".into()),
                display_name: "kiyora".into(),
            },
        },
    );
    assert_eq!(s.settings.gc_device_id.as_deref(), Some("desktop"));
    s.validate().unwrap();
    assert_eq!(s.applied.len(), 7);
}

proptest! {
    #[test]
    fn arbitrary_replays_preserve_invariants_and_checkpoint_equivalence(ops in prop::collection::vec((0u8..7, 0u8..8, 0u8..8), 1..120), cut in 0usize..120) {
        let mut whole = State::new(id(1));
        apply(&mut whole, 1, 30, track());
        apply(&mut whole, 2, 40, Action::PlaylistCreate { title: "Mix".into() });
        let events: Vec<_> = ops.iter().enumerate().map(|(i, (kind, a, b))| {
            let action = match kind {
                0 => Action::PlaylistTrackAdd { track_id:id(30), entry_id:id(50+u128::from(*a)), after_entry_id:None },
                1 => Action::PlaylistEntryMove { entry_id:id(50+u128::from(*a)), after_entry_id:Some(id(50+u128::from(*b))) },
                2 => Action::PlaylistTrackRemove { entry_id:id(50+u128::from(*a)) },
                3 => Action::PlaylistRename { title: format!("Mix {a}") },
                4 => Action::PlaylistDelete {},
                5 => Action::PlaylistCreate { title: "Again".into() },
                _ => Action::PlaylistEntryMove { entry_id:id(50+u128::from(*a)), after_entry_id:None },
            };
            (MessageId::new(10 + i as i64 * 17).unwrap(), event(10+i as u128,40,action))
        }).collect();
        let mut prefix = whole.clone();
        for (mid,e) in &events {
            whole.apply(*mid,e).unwrap();
            prop_assert!(whole.validate().is_ok());
            let before = whole.clone();
            whole.apply(*mid,e).unwrap();
            prop_assert_eq!(&whole, &before);
        }
        let split = cut.min(events.len());
        for (mid,e) in &events[..split] { prefix.apply(*mid,e).unwrap(); }
        let encoded = serde_json::to_vec(&prefix).unwrap();
        let mut restored: State = serde_json::from_slice(&encoded).unwrap();
        for (mid,e) in &events[split..] { restored.apply(*mid,e).unwrap(); }
        prop_assert_eq!(whole, restored);
    }

    #[test]
    fn telegram_identifier_json_roundtrip(n in 1i64..=i64::MAX) {
        let id = MessageId::new(n).unwrap();
        let json = serde_json::to_string(&id).unwrap();
        prop_assert_eq!(&json, &format!("\"{n}\""));
        prop_assert_eq!(serde_json::from_str::<MessageId>(&json).unwrap(), id);
        prop_assert!(serde_json::from_str::<MessageId>(&n.to_string()).is_err());
    }
}
