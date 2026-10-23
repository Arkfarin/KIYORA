#![allow(clippy::unwrap_used)]
use uuid::Uuid;
use vault_db::{Binding, Database};
use vault_domain::*;
use vault_protocol::Record;

#[test]
fn local_queries_preserve_forest_order_and_page_catalogues() {
    let binding = Binding {
        vault_id: Uuid::new_v4(),
        owner_id: UserId::new(42).unwrap(),
        chat_id: ChatId::new(-100).unwrap(),
    };
    let mut db = Database::memory(binding).unwrap();
    let track = Uuid::new_v4();
    let playlist = Uuid::new_v4();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let c = Uuid::new_v4();
    let actions = [
        (
            track,
            Action::TrackRegister {
                metadata: Metadata {
                    title: "Song".into(),
                    performer: "Singer".into(),
                    album: "Album".into(),
                    album_artist: "Various".into(),
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
                title: "Playlist".into(),
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
        (
            track,
            Action::FavoriteSet {
                set_name: "liked".into(),
                value: true,
            },
        ),
        (
            binding.vault_id,
            Action::VaultSettingsUpdate {
                settings: SharedSettings {
                    gc_device_id: Some("device".into()),
                    display_name: "Vault".into(),
                },
            },
        ),
    ];
    for (i, (entity_id, action)) in actions.into_iter().enumerate() {
        let event = Event {
            vault_id: binding.vault_id,
            op_id: Uuid::new_v4(),
            device_id: "device".into(),
            base_generation: 0,
            entity_id,
            action,
        };
        assert_eq!(
            db.apply_record(
                MessageId::new(i as i64 + 10).unwrap(),
                &Record::Event(Box::new(event))
            )
            .unwrap(),
            Some(Outcome::Applied)
        );
    }
    let detail = db.playlist_detail(playlist, 0, 500).unwrap().unwrap();
    assert_eq!(
        detail
            .items
            .iter()
            .map(|i| i.entry.entry_id)
            .collect::<Vec<_>>(),
        [a, c, b]
    );
    assert_eq!(detail.total, 3);
    assert_eq!(detail.items[0].track.metadata.title, "Song");
    assert_eq!(
        db.playlist_detail(playlist, 1, 1).unwrap().unwrap().items[0]
            .entry
            .entry_id,
        c
    );
    assert!(db.playlist_detail(Uuid::new_v4(), 0, 1).unwrap().is_none());
    assert_eq!(db.favorite_sets(0, 5).unwrap(), ["liked"]);
    assert_eq!(
        db.favorites("liked", 0, 5).unwrap().tracks[0].track_id,
        track
    );
    assert_eq!(db.favorites("liked", 1, 5).unwrap().total, 1);
    assert!(db.favorites("liked", 1, 5).unwrap().tracks.is_empty());
    let albums = db.albums("alb", 0, 5).unwrap();
    assert_eq!(albums[0].artist, "Various");
    assert_eq!(albums[0].track_count, 1);
    assert!(db.albums("absent", 0, 5).unwrap().is_empty());
    assert_eq!(db.artists("sing", 0, 5).unwrap()[0].name, "Singer");
    assert_eq!(
        db.shared_settings().unwrap().gc_device_id.as_deref(),
        Some("device")
    );
}
