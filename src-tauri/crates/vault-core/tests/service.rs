#![allow(clippy::unwrap_used)]
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_core::*;
use vault_db::{Binding, CommandStatus, Database};
use vault_domain::*;
use vault_protocol::{Record, VaultInit, encode_text};
use vault_sync::{RuntimeMode, SyncEngine};
use vault_telegram::Telegram;
use vault_testkit::{FakeServer, Faults};

async fn ready(service: &VaultService) {
    let mut status = service.status_changes();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if status.borrow_and_update().mode == RuntimeMode::OnlineReady {
                break;
            }
            status.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
}
async fn fixture() -> (FakeServer, Binding) {
    let binding = Binding {
        vault_id: Uuid::new_v4(),
        owner_id: UserId::new(42).unwrap(),
        chat_id: ChatId::new(-100).unwrap(),
    };
    let server = FakeServer::new(binding.chat_id, binding.owner_id);
    let transport = server.client().unwrap();
    let token = CancellationToken::new();
    transport
        .send_text(
            binding.chat_id,
            encode_text(&Record::Init(VaultInit {
                vault_id: binding.vault_id,
                owner_telegram_user_id: binding.owner_id,
                created_by: "kiyora".into(),
                created_at_utc: "2026-10-21T00:00:00Z".into(),
            }))
            .unwrap(),
            &token,
        )
        .await
        .unwrap();
    let mut engine = SyncEngine::new(
        transport,
        Database::memory(binding).unwrap(),
        "setup".into(),
    );
    engine.initialize(&token).await.unwrap();
    (server, binding)
}
fn service(server: &FakeServer, binding: Binding, bus: EventBus) -> VaultService {
    VaultService::start(
        SyncEngine::new(
            server.client().unwrap(),
            Database::memory(binding).unwrap(),
            "device".into(),
        ),
        Context {
            profile_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            vault_id: None,
        },
        bus,
    )
}

#[tokio::test]
async fn live_services_converge_without_manual_sync_and_events_are_contextual() {
    let (server, binding) = fixture().await;
    let bus = EventBus::default();
    let mut events = bus.subscribe();
    let a = service(&server, binding, bus.clone());
    let b = service(&server, binding, bus);
    ready(&a).await;
    ready(&b).await;
    let id = Uuid::new_v4();
    let response = a
        .request(
            VaultRequest::Mutate(Box::new(Mutation {
                op_id: Uuid::new_v4(),
                entity_id: id,
                action: Action::PlaylistCreate {
                    title: "Shared live playlist".into(),
                },
            })),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(response,VaultResponse::Command(c) if c.status==CommandStatus::Applied));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.context == *b.context()
                && matches!(event.data, EventData::Library { .. })
                && b.reader()
                    .state()
                    .await
                    .unwrap()
                    .playlists
                    .contains_key(&id)
            {
                break;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(
        a.reader().state().await.unwrap(),
        b.reader().state().await.unwrap()
    );
    a.shutdown().await.unwrap();
    b.shutdown().await.unwrap();
    assert_eq!(
        a.request(VaultRequest::Sync, &CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::Closed
    );
}

#[tokio::test]
async fn offline_service_keeps_local_reads_and_does_not_queue_shared_intents() {
    let (server, binding) = fixture().await;
    let a = service(&server, binding, EventBus::default());
    ready(&a).await;
    let token = CancellationToken::new();
    let op = Uuid::new_v4();
    let before = server.messages().unwrap();
    server
        .set_faults(Faults {
            offline: true,
            ..Default::default()
        })
        .unwrap();
    assert!(
        a.request(
            VaultRequest::Mutate(Box::new(Mutation {
                op_id: op,
                entity_id: Uuid::new_v4(),
                action: Action::PlaylistCreate {
                    title: "must not queue".into()
                }
            })),
            &token
        )
        .await
        .is_err()
    );
    assert!(a.reader().command(op).await.unwrap().is_none());
    assert!(a.reader().library(None, 0, 50).await.unwrap().is_empty());
    assert_eq!(before, server.messages().unwrap());
    assert_ne!(a.status().mode, RuntimeMode::OnlineReady);
    a.shutdown().await.unwrap();
}

#[tokio::test]
async fn event_sequence_survives_session_changes_and_cancel_prevents_submission() {
    let (server, binding) = fixture().await;
    let bus = EventBus::default();
    let mut events = bus.subscribe();
    let a = service(&server, binding, bus.clone());
    ready(&a).await;
    let cancel = CancellationToken::new();
    cancel.cancel();
    let before = server.messages().unwrap();
    assert_eq!(
        a.request(VaultRequest::Snapshot, &cancel)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert_eq!(before, server.messages().unwrap());
    a.shutdown().await.unwrap();
    let b = service(&server, binding, bus);
    ready(&b).await;
    b.shutdown().await.unwrap();
    let mut last = 0u64;
    let mut sessions = std::collections::BTreeSet::new();
    while let Ok(event) = events.try_recv() {
        let seq = event.sequence.parse::<u64>().unwrap();
        assert!(seq > last);
        last = seq;
        sessions.insert(event.context.session_id);
    }
    assert!(sessions.contains(&a.context().session_id));
    assert!(sessions.contains(&b.context().session_id));
}

#[tokio::test]
async fn import_uploads_verified_media_then_registers_and_requires_duplicate_review() {
    let (server, binding) = fixture().await;
    let service = service(&server, binding, EventBus::default());
    ready(&service).await;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("import.wav");
    let mut wav = Vec::new();
    wav.extend(b"RIFF");
    wav.extend(16036u32.to_le_bytes());
    wav.extend(b"WAVEfmt ");
    wav.extend(16u32.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(1u16.to_le_bytes());
    wav.extend(8000u32.to_le_bytes());
    wav.extend(16000u32.to_le_bytes());
    wav.extend(2u16.to_le_bytes());
    wav.extend(16u16.to_le_bytes());
    wav.extend(b"data");
    wav.extend(16000u32.to_le_bytes());
    wav.extend(vec![0u8; 16000]);
    std::fs::write(&path, wav).unwrap();
    let token = CancellationToken::new();
    let op = Uuid::new_v4();
    let track = Uuid::new_v4();
    let source = Uuid::new_v4();
    let first = service
        .request(
            VaultRequest::Import {
                path: path.to_string_lossy().into_owned(),
                track_id: track,
                source_id: source,
                op_id: op,
                allow_duplicate: false,
            },
            &token,
        )
        .await
        .unwrap();
    assert!(matches!(first,VaultResponse::Command(ref c) if c.status==CommandStatus::Applied));
    let state = service.reader().state().await.unwrap();
    assert!(state.tracks.contains_key(&track));
    assert_eq!(
        state.tracks[&track].sources[&source].size_bytes,
        Some(16044)
    );
    let media = server
        .messages()
        .unwrap()
        .into_iter()
        .find(|m| {
            m.file.is_some()
                && matches!(m.record().unwrap(), Some(vault_protocol::Record::Media(_)))
        })
        .unwrap();
    assert_eq!(media.file.as_ref().unwrap().size, 16044);
    assert_eq!(media.record().unwrap().unwrap().kind(), "MEDIA");
    let recipient = ChatId::new(4242).unwrap();
    let shared = service
        .request(
            VaultRequest::ShareTrack {
                track_id: track,
                source_id: source,
                recipient_chat_id: recipient,
            },
            &token,
        )
        .await
        .unwrap();
    assert!(
        matches!(shared, VaultResponse::Shared { recipient_chat_id, .. } if recipient_chat_id == recipient)
    );
    assert_eq!(
        server.shared_messages().unwrap(),
        vec![(binding.chat_id, media.id, recipient)]
    );
    assert!(
        service
            .request(
                VaultRequest::ShareTrack {
                    track_id: Uuid::new_v4(),
                    source_id: source,
                    recipient_chat_id: recipient,
                },
                &token,
            )
            .await
            .is_err()
    );
    assert_eq!(server.shared_messages().unwrap().len(), 1);
    let before = server.messages().unwrap();
    let duplicate = service
        .request(
            VaultRequest::Import {
                path: path.to_string_lossy().into_owned(),
                track_id: Uuid::new_v4(),
                source_id: Uuid::new_v4(),
                op_id: Uuid::new_v4(),
                allow_duplicate: false,
            },
            &token,
        )
        .await
        .unwrap();
    assert!(
        matches!(duplicate,VaultResponse::ImportReview{ref duplicate_track_ids,..} if duplicate_track_ids==&vec![track])
    );
    assert_eq!(before, server.messages().unwrap());
    service.shutdown().await.unwrap();
}
