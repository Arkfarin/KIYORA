//! Simulated Telegram GC acceptance: wall time and actual transport call counts.
//! Run with optional event counts, e.g. `gc-benchmark 1000 10000 50000 100000`.
use std::{error::Error, time::Instant};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_db::{Binding, Database};
use vault_domain::*;
use vault_protocol::{Record, VaultInit, encode_text};
use vault_sync::SyncEngine;
use vault_telegram::Telegram;
use vault_testkit::FakeServer;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let mut sizes = std::env::args()
        .skip(1)
        .map(|value| value.parse::<usize>())
        .collect::<Result<Vec<_>, _>>()?;
    if sizes.is_empty() {
        sizes = vec![1_000, 10_000, 50_000, 100_000];
    }
    for count in sizes {
        if count < 2 {
            return Err("at least two events required".into());
        }
        let total = Instant::now();
        let binding = Binding {
            vault_id: Uuid::new_v4(),
            owner_id: UserId::new(42)?,
            chat_id: ChatId::new(-100)?,
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
                    created_by: "benchmark".into(),
                    created_at_utc: "2026-10-24T00:00:00Z".into(),
                }))?,
                &cancel,
            )
            .await?;
        let mut engine = SyncEngine::new(
            transport.clone(),
            Database::memory(binding)?,
            "benchmark".into(),
        );
        engine.initialize(&cancel).await?;
        engine
            .assign_gc_coordinator(
                Uuid::new_v4(),
                Some("benchmark".into()),
                None,
                false,
                &cancel,
            )
            .await?;
        let playlist = Uuid::new_v4();
        for index in 0..count {
            let action = if index == 0 {
                Action::PlaylistCreate {
                    title: "Old".into(),
                }
            } else if index == count - 1 {
                Action::PlaylistDelete {}
            } else {
                Action::PlaylistRename {
                    title: format!("Version {index}"),
                }
            };
            let event = Event {
                vault_id: binding.vault_id,
                op_id: Uuid::new_v4(),
                device_id: "seed".into(),
                base_generation: 0,
                entity_id: playlist,
                action,
            };
            transport
                .send_text(
                    binding.chat_id,
                    encode_text(&Record::Event(Box::new(event)))?,
                    &cancel,
                )
                .await?;
        }
        let media =
            server.add_incoming_document("tone.mp3".into(), "audio/mpeg".into(), vec![1; 1024])?;
        let source = Source {
            source_id: Uuid::new_v4(),
            chat_id: binding.chat_id,
            message_id: media.id,
            sha256: None,
            size_bytes: Some(1024),
        };
        engine
            .execute(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Action::TrackRegister {
                    metadata: Metadata {
                        title: "Live audio".into(),
                        ..Default::default()
                    },
                    source,
                },
                &cancel,
            )
            .await?;
        let sync_start = Instant::now();
        let history_before = server.history_calls()?;
        engine.require_online(&cancel).await?;
        let idle_sync_ms = sync_start.elapsed().as_millis();
        let idle_history_calls = server.history_calls()? - history_before;
        assert!(idle_history_calls <= 4);
        engine.request_snapshot(&cancel).await?;
        let plan = engine.plan_gc(&cancel).await?;
        let tail = engine
            .execute(
                Uuid::new_v4(),
                Uuid::new_v4(),
                Action::PlaylistCreate {
                    title: "After barrier".into(),
                },
                &cancel,
            )
            .await?
            .server_message_id
            .ok_or("missing confirmation")?;
        let expected = engine.state().await?;
        let history = server.history_calls()?;
        let messages = server.message_calls()?;
        let downloads = server.download_calls()?;
        let deletes = server.delete_calls()?;
        let started = Instant::now();
        let completed = engine
            .execute_gc(plan.plan.plan_id, &plan.plan.hash()?, &cancel)
            .await?;
        let gc_ms = started.elapsed().as_millis();
        let gc_history_calls = server.history_calls()? - history;
        let gc_message_calls = server.message_calls()? - messages;
        let gc_snapshot_downloads = server.download_calls()? - downloads;
        let gc_delete_calls = server.delete_calls()? - deletes;
        assert_eq!(completed.completed.len(), plan.plan.candidates.len());
        assert!(
            transport
                .message(binding.chat_id, media.id, &cancel)
                .await?
                .is_some()
        );
        assert!(
            transport
                .message(binding.chat_id, tail, &cancel)
                .await?
                .is_some()
        );
        let mut cold = SyncEngine::new(server.client()?, Database::memory(binding)?, "cold".into());
        cold.sync(&cancel).await?;
        assert_eq!(cold.state().await?, expected);
        assert!(cold.state().await?.tombstones.playlists.contains(&playlist));
        println!(
            "{}",
            serde_json::json!({"simulated":true,"events":count,"candidates":plan.plan.candidates.len(),"idle_sync_ms":idle_sync_ms,"idle_history_calls":idle_history_calls,"gc_ms":gc_ms,"gc_history_calls":gc_history_calls,"gc_message_calls":gc_message_calls,"gc_snapshot_downloads":gc_snapshot_downloads,"gc_delete_calls":gc_delete_calls,"total_ms":total.elapsed().as_millis(),"cold_restore_equal":true,"live_media_preserved":true,"post_barrier_event_preserved":true,"tombstone_preserved":true})
        );
    }
    Ok(())
}
