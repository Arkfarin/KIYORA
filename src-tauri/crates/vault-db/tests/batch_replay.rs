#![allow(clippy::unwrap_used)]
use proptest::prelude::*;
use uuid::Uuid;
use vault_db::{Binding, Database};
use vault_domain::*;
use vault_protocol::{Barrier, Record};

fn binding() -> Binding {
    Binding {
        vault_id: Uuid::from_u128(1),
        owner_id: UserId::new(42).unwrap(),
        chat_id: ChatId::new(-1001234567890).unwrap(),
    }
}
fn source(n: u8) -> Source {
    Source {
        source_id: Uuid::from_u128(400 + u128::from(n)),
        chat_id: binding().chat_id,
        message_id: MessageId::new(9_007_199_254_740_993 + i64::from(n)).unwrap(),
        sha256: None,
        size_bytes: Some(1_024),
    }
}
fn record(index: usize, kind: u8, a: u8, b: u8, flag: bool) -> Record {
    let track = Uuid::from_u128(100 + u128::from(a));
    let playlist = Uuid::from_u128(200 + u128::from(a));
    let entry = Uuid::from_u128(300 + u128::from(b));
    let (entity_id, action) = match kind {
        0 => (
            track,
            Action::TrackRegister {
                metadata: Metadata {
                    title: format!("Track {a}"),
                    ..Default::default()
                },
                source: source(a),
            },
        ),
        1 => (
            track,
            Action::TrackMetadataUpdate {
                patch: MetadataPatch {
                    title: Some(format!("Renamed {b}")),
                    ..Default::default()
                },
            },
        ),
        2 => (track, Action::TrackDelete {}),
        3 => (
            playlist,
            Action::PlaylistCreate {
                title: format!("Playlist {a}"),
            },
        ),
        4 => (
            playlist,
            Action::PlaylistRename {
                title: format!("Mix {b}"),
            },
        ),
        5 => (playlist, Action::PlaylistDelete {}),
        6 => (
            playlist,
            Action::PlaylistTrackAdd {
                track_id: Uuid::from_u128(100 + u128::from(b)),
                entry_id: entry,
                after_entry_id: None,
            },
        ),
        7 => (playlist, Action::PlaylistTrackRemove { entry_id: entry }),
        8 => (
            playlist,
            Action::PlaylistEntryMove {
                entry_id: entry,
                after_entry_id: flag.then_some(Uuid::from_u128(300 + u128::from(a))),
            },
        ),
        9 => (
            track,
            Action::FavoriteSet {
                set_name: "favorites".into(),
                value: flag,
            },
        ),
        10 => (track, Action::SourceRelink { source: source(b) }),
        11 => (
            binding().vault_id,
            Action::VaultSettingsUpdate {
                settings: SharedSettings {
                    display_name: format!("Vault {b}"),
                    gc_device_id: Some("device".into()),
                },
            },
        ),
        _ => {
            return Record::Barrier(Barrier {
                vault_id: binding().vault_id,
                snapshot_id: Uuid::from_u128(30_000 + index as u128),
                generation: 0,
            });
        }
    };
    Record::Event(Box::new(Event {
        vault_id: binding().vault_id,
        op_id: Uuid::from_u128(10_000 + index as u128),
        device_id: "property-test".into(),
        base_generation: 0,
        entity_id,
        action,
    }))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]
    #[test]
    fn batch_boundaries_do_not_change_mixed_replay(
        ops in prop::collection::vec((0u8..13, 0u8..4, 0u8..4, any::<bool>(), 0u8..3), 0..65),
        batch_size in 1usize..32,
    ) {
        // Seed live references before random operations so both successful cascades
        // and rejected operations on tombstones occur across arbitrary boundaries.
        let mut inputs = Vec::new();
        for a in 0..4 { inputs.push((0, a, a, false, 0)); }
        for a in 0..4 { inputs.push((3, a, a, false, 0)); }
        for a in 0..4 { inputs.push((6, a, a, false, 0)); }
        inputs.extend(ops);
        let mut records = Vec::new();
        for (index, (kind, a, b, flag, duplicate)) in inputs.into_iter().enumerate() {
            let id = MessageId::new(9_007_199_254_750_000 + index as i64 * 17).unwrap();
            let item = record(index, kind, a, b, flag);
            records.push((id, item.clone()));
            if duplicate != 0 {
                let duplicate_id = if duplicate == 1 { id } else { MessageId::new(id.get() + 1).unwrap() };
                records.push((duplicate_id, item));
            }
        }
        let mut reference = State::new(binding().vault_id);
        let mut expected_outcomes = Vec::new();
        for (mid, record) in &records {
            expected_outcomes.push(match record {
                Record::Event(event) => Some(reference.apply(*mid, event).unwrap()),
                _ => None,
            });
        }
        let mut sequential = Database::memory(binding()).unwrap();
        for (mid, record) in &records {
            sequential.apply_record(*mid, record).unwrap();
        }
        let mut batched = Database::memory(binding()).unwrap();
        let mut actual_outcomes = Vec::new();
        for page in records.chunks(batch_size) {
            let outcome = batched.apply_records(page);
            prop_assert!(outcome.is_ok(), "batch failure: {:?}", outcome);
            actual_outcomes.extend(outcome.unwrap());
        }
        prop_assert_eq!(actual_outcomes, expected_outcomes);
        prop_assert_eq!(batched.state().unwrap(), reference);
        prop_assert_eq!(batched.state().unwrap(), sequential.state().unwrap());
        prop_assert_eq!(batched.cursor().unwrap(), records.last().unwrap().0.get());
        for page in records.chunks(batch_size) { batched.apply_records(page).unwrap(); }
        prop_assert_eq!(batched.state().unwrap(), sequential.state().unwrap());
    }
}
