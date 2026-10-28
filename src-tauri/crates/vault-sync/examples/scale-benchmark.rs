//! Deterministic 10k tracks / 100k entries / 10k tail, without Telegram credentials.
use std::{collections::BTreeMap, error::Error, time::Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_db::{Binding, Database};
use vault_domain::*;
use vault_protocol::{Barrier, Record, VaultInit, encode_text};
use vault_sync::SyncEngine;
use vault_telegram::Telegram;
use vault_testkit::FakeServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let binding = Binding {
        vault_id: Uuid::from_u128(1),
        chat_id: ChatId::new(-100)?,
        owner_id: UserId::new(42)?,
    };
    let server = FakeServer::new(binding.chat_id, binding.owner_id);
    let transport = server.client()?;
    let cancel = CancellationToken::new();
    transport
        .send_text(
            binding.chat_id,
            encode_text(&Record::Init(VaultInit {
                vault_id: binding.vault_id,
                owner_telegram_user_id: binding.owner_id,
                created_by: "scale".into(),
                created_at_utc: "2026-10-23T00:00:00Z".into(),
            }))?,
            &cancel,
        )
        .await?;
    let mut state = State::new(binding.vault_id);
    let order = MessageId::new(1)?;
    state.apply(
        order,
        &Event {
            vault_id: binding.vault_id,
            op_id: Uuid::from_u128(2),
            device_id: "fixture".into(),
            base_generation: 0,
            entity_id: binding.vault_id,
            action: Action::VaultSettingsUpdate {
                settings: SharedSettings::default(),
            },
        },
    )?;
    for i in 0..10_000_u128 {
        let id = Uuid::from_u128(100_000 + i);
        let source = Source {
            source_id: Uuid::from_u128(200_000 + i),
            chat_id: binding.chat_id,
            message_id: order,
            sha256: Some("ab".repeat(32)),
            size_bytes: Some(1000),
        };
        state.tracks.insert(
            id,
            Track {
                preferred_source_id: None,
                track_id: id,
                metadata: Metadata {
                    title: format!("Track {i}"),
                    performer: format!("Artist {}", i % 100),
                    ..Default::default()
                },
                sources: BTreeMap::from([(source.source_id, source)]),
                updated_order: order,
            },
        );
    }
    for i in 0..100_u128 {
        let playlist = Uuid::from_u128(300_000 + i);
        state.playlists.insert(
            playlist,
            Playlist {
                playlist_id: playlist,
                title: format!("Playlist {i}"),
                created_order: order,
                updated_order: order,
            },
        );
        let mut previous = None;
        for j in 0..1000_u128 {
            let entry = Uuid::from_u128(400_000 + i * 1000 + j);
            state.entries.insert(
                entry,
                PlaylistEntry {
                    entry_id: entry,
                    playlist_id: playlist,
                    track_id: Uuid::from_u128(100_000 + (i * 1000 + j) % 10_000),
                    after_entry_id: previous,
                    order_key: order,
                },
            );
            previous = Some(entry);
        }
    }
    state.validate()?;
    let snapshot_id = Uuid::new_v4();
    let barrier = transport
        .send_text(
            binding.chat_id,
            encode_text(&Record::Barrier(Barrier {
                vault_id: binding.vault_id,
                snapshot_id,
                generation: 0,
            }))?,
            &cancel,
        )
        .await?;
    let begin = Instant::now();
    let encoded =
        vault_snapshot::build(state.clone(), binding.owner_id, snapshot_id, 0, barrier.id)?;
    let compressed_bytes = encoded.compressed.len();
    vault_snapshot::publish(&transport, binding.chat_id, encoded, None, &cancel).await?;
    let snapshot_ms = begin.elapsed().as_millis();
    for i in 0..10_000_u128 {
        let event = Event {
            vault_id: binding.vault_id,
            op_id: Uuid::from_u128(600_000 + i),
            device_id: "scale".into(),
            base_generation: 0,
            entity_id: Uuid::from_u128(100_000 + i),
            action: Action::TrackMetadataUpdate {
                patch: MetadataPatch {
                    title: Some(format!("Updated {i}")),
                    ..Default::default()
                },
            },
        };
        let message = transport
            .send_text(
                binding.chat_id,
                encode_text(&Record::Event(Box::new(event.clone())))?,
                &cancel,
            )
            .await?;
        state.apply(message.id, &event)?;
    }
    let directory = tempfile::tempdir()?;
    let mut engine = SyncEngine::new(
        server.client()?,
        Database::open(directory.path().join("vault.sqlite"), binding)?,
        "cold".into(),
    );
    let calls = server.history_calls()?;
    let begin = Instant::now();
    engine.sync(&cancel).await?;
    let sync_ms = begin.elapsed().as_millis();
    assert_eq!(engine.state().await?, state);
    let begin = Instant::now();
    let page = engine
        .reader()
        .playlist_detail(Uuid::from_u128(300_000), 100, 100)
        .await?
        .ok_or("playlist missing")?;
    assert_eq!(page.items.len(), 100);
    assert_eq!(page.total, 1000);
    let query_ms = begin.elapsed().as_millis();
    println!(
        "{}",
        serde_json::json!({"tracks":10_000,"entries":100_000,"tail_events":10_000,"snapshot_ms":snapshot_ms,"snapshot_compressed_bytes":compressed_bytes,"cold_sync_ms":sync_ms,"history_requests":server.history_calls()? - calls,"playlist_page_ms":query_ms,"matches_reducer":true,"profile":if cfg!(debug_assertions){"debug"}else{"release"}})
    );
    Ok(())
}
