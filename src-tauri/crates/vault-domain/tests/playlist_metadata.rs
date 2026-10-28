#![allow(clippy::unwrap_used)]
use uuid::Uuid;
use vault_domain::{Action, Event, MessageId, Outcome, RejectReason, State};

fn apply(state: &mut State, playlist: Uuid, order: i64, action: Action) -> Outcome {
    state
        .apply(
            MessageId::new(order).unwrap(),
            &Event {
                vault_id: state.vault_id,
                op_id: Uuid::new_v4(),
                device_id: "test".into(),
                base_generation: 0,
                entity_id: playlist,
                action,
            },
        )
        .unwrap()
}

#[test]
fn playlist_edit_is_atomic_versioned_and_delete_wins() {
    let mut state = State::new(Uuid::new_v4());
    let id = Uuid::new_v4();
    apply(
        &mut state,
        id,
        1,
        Action::PlaylistCreate {
            title: "Original".into(),
        },
    );
    let edit: Action = serde_json::from_value(serde_json::json!({
        "action": "playlist.metadata.update", "payload": {"patch": {
            "title": "Edited", "expected_order": "1", "artwork": {
                "sha256": "a".repeat(64), "mime_type": "image/png", "source": {
                    "source_id": Uuid::new_v4(), "chat_id": "-1001", "message_id": "2",
                    "sha256": "a".repeat(64), "size_bytes": 100
                }
            }
        }}
    }))
    .unwrap();
    assert_eq!(apply(&mut state, id, 3, edit.clone()), Outcome::Applied);
    assert_eq!(
        apply(&mut state, id, 4, edit.clone()),
        Outcome::Rejected(RejectReason::VersionConflict)
    );
    assert_eq!(state.playlists[&id].title, "Edited");
    state.validate().unwrap();
    let restored: State = serde_json::from_slice(&serde_json::to_vec(&state).unwrap()).unwrap();
    assert_eq!(restored, state);
    apply(&mut state, id, 5, Action::PlaylistDelete {});
    assert_eq!(
        apply(&mut state, id, 6, edit),
        Outcome::Rejected(RejectReason::Deleted)
    );
    assert!(state.playlists.is_empty());
}

#[test]
fn legacy_playlist_serialization_is_unchanged() {
    let original = serde_json::json!({ "playlist_id": Uuid::new_v4(), "title": "Legacy",
        "created_order": "1", "updated_order": "2" });
    let playlist: vault_domain::Playlist = serde_json::from_value(original.clone()).unwrap();
    assert!(playlist.artwork.is_none());
    assert_eq!(serde_json::to_value(playlist).unwrap(), original);
}

#[test]
fn invalid_playlist_cover_rejects_the_entire_patch() {
    let mut state = State::new(Uuid::new_v4());
    let id = Uuid::new_v4();
    apply(
        &mut state,
        id,
        1,
        Action::PlaylistCreate {
            title: "Original".into(),
        },
    );
    let original = state.playlists[&id].clone();
    for (offset, (sha, size)) in [
        ("b".repeat(64), 100),
        ("a".repeat(64), 0),
        ("a".repeat(64), 4 * 1024 * 1024 + 1),
    ]
    .into_iter()
    .enumerate()
    {
        let action = Action::PlaylistMetadataUpdate {
            patch: vault_domain::PlaylistPatch {
                title: Some("Must not appear".into()),
                expected_order: Some(original.updated_order),
                artwork: Some(vault_domain::Artwork {
                    sha256: "a".repeat(64),
                    mime_type: "image/png".into(),
                    source: Some(vault_domain::Source {
                        source_id: Uuid::new_v4(),
                        chat_id: vault_domain::ChatId::new(-100).unwrap(),
                        message_id: MessageId::new(1).unwrap(),
                        sha256: Some(sha),
                        size_bytes: Some(size),
                    }),
                }),
                ..Default::default()
            },
        };
        assert_eq!(
            apply(&mut state, id, 2 + offset as i64, action),
            Outcome::Rejected(RejectReason::InvalidPayload)
        );
        assert_eq!(state.playlists[&id], original);
    }
}
