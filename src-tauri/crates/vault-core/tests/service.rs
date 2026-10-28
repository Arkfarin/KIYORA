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
fn wav_fixture() -> Vec<u8> {
    let samples = [0i16; 4_800];
    let data_len = (samples.len() * 2) as u32;
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&48_000u32.to_le_bytes());
    wav.extend_from_slice(&96_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    wav
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
fn service_with_cache(
    server: &FakeServer,
    binding: Binding,
    bus: EventBus,
    cache: vault_media::CacheManager,
) -> VaultService {
    VaultService::start_with_cache(
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
        cache,
    )
}

#[tokio::test]
async fn application_api_assigns_current_coordinator_then_creates_snapshot_and_gc_plan() {
    let (server, binding) = fixture().await;
    let service = service(&server, binding, EventBus::default());
    ready(&service).await;
    let cancel = CancellationToken::new();
    assert!(
        service
            .request(VaultRequest::Snapshot, &cancel)
            .await
            .is_err()
    );
    let response = service
        .request(
            VaultRequest::AssignGcCoordinator {
                op_id: Uuid::new_v4(),
                target: CoordinatorTarget::ThisDevice,
                expected_previous: None,
                force_confirmed: false,
            },
            &cancel,
        )
        .await
        .unwrap();
    assert!(matches!(response, VaultResponse::Command(c) if c.status == CommandStatus::Applied));
    assert_eq!(
        service
            .reader()
            .shared_settings()
            .await
            .unwrap()
            .gc_device_id
            .as_deref(),
        Some("device")
    );
    assert!(
        matches!(service.request(VaultRequest::Snapshot, &cancel).await.unwrap(), VaultResponse::Snapshot(s) if s.primary_healthy && s.backup_healthy)
    );
    let VaultResponse::Gc(Some(plan)) = service
        .request(VaultRequest::PlanGc, &cancel)
        .await
        .unwrap()
    else {
        panic!("GC plan expected");
    };
    assert!(!plan.plan.candidates.is_empty());
    let before = server.messages().unwrap();
    assert!(
        service
            .request(
                VaultRequest::AssignGcCoordinator {
                    op_id: Uuid::new_v4(),
                    target: CoordinatorTarget::Disabled,
                    expected_previous: Some("device".into()),
                    force_confirmed: false
                },
                &cancel
            )
            .await
            .is_err()
    );
    service
        .request(
            VaultRequest::AbandonGc {
                plan_id: plan.plan.plan_id,
            },
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(
        server.messages().unwrap(),
        before,
        "abandoning a plan is local and must not delete remote records"
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn automatic_snapshot_is_verified_and_keeps_all_history() {
    let (server, binding) = fixture().await;
    let events = EventBus::default();
    let mut subscription = events.subscribe();
    let service = service(&server, binding, events);
    ready(&service).await;
    let cancel = CancellationToken::new();
    service
        .request(
            VaultRequest::AssignGcCoordinator {
                op_id: Uuid::new_v4(),
                target: CoordinatorTarget::ThisDevice,
                expected_previous: None,
                force_confirmed: false,
            },
            &cancel,
        )
        .await
        .unwrap();
    let config = MaintenanceConfig {
        max_events: 1,
        idle_seconds: 0,
        check_interval_seconds: 1,
        ..Default::default()
    };
    service
        .request(
            VaultRequest::SetMaintenanceConfig {
                config: config.clone(),
            },
            &cancel,
        )
        .await
        .unwrap();
    assert!(
        matches!(service.request(VaultRequest::GetMaintenanceConfig, &cancel).await.unwrap(), VaultResponse::MaintenanceConfig(actual) if actual == config)
    );
    let before = server.messages().unwrap();
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    tokio::time::resume();
    let snapshot = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let EventData::Snapshot {
                running: false,
                result: Some(snapshot),
            } = subscription.recv().await.unwrap().data
            {
                break snapshot;
            }
        }
    })
    .await
    .unwrap();
    assert!(snapshot.primary_healthy && snapshot.backup_healthy);
    let after = server.messages().unwrap();
    assert!(after.len() > before.len());
    assert!(
        before.iter().all(|message| after.contains(message)),
        "snapshot scheduler must not perform GC"
    );
    service.shutdown().await.unwrap();
    let mut cold = SyncEngine::new(
        server.client().unwrap(),
        Database::memory(binding).unwrap(),
        "new-device".into(),
    );
    cold.sync(&cancel).await.unwrap();
    assert_eq!(
        cold.reader()
            .shared_settings()
            .await
            .unwrap()
            .gc_device_id
            .as_deref(),
        Some("device")
    );
    assert_eq!(
        cold.verified_snapshot().unwrap().commit_id,
        snapshot.commit_id
    );
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
async fn incoming_audio_is_downloaded_validated_and_registered_in_vault() {
    let (server, binding) = fixture().await;
    let incoming = server
        .add_incoming_document("voice.wav".into(), "audio/wav".into(), wav_fixture())
        .unwrap();
    let service = service(&server, binding, EventBus::default());
    ready(&service).await;
    let track_id = Uuid::new_v4();
    let response = service
        .request(
            VaultRequest::ImportIncomingAudio {
                source_chat_id: incoming.chat_id,
                message_id: incoming.id,
                track_id,
                source_id: Uuid::new_v4(),
                op_id: Uuid::new_v4(),
                allow_duplicate: false,
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        matches!(response, VaultResponse::Command(command) if command.status == CommandStatus::Applied)
    );
    let track = service.reader().state().await.unwrap().tracks[&track_id].clone();
    assert_eq!(track.metadata.title, "voice");
    assert_eq!(track.sources.len(), 1);
    assert_ne!(
        track.sources.values().next().unwrap().message_id,
        incoming.id
    );
    let stored_id = track.sources.values().next().unwrap().message_id;
    let stored = server
        .messages()
        .unwrap()
        .into_iter()
        .find(|message| message.id == stored_id)
        .unwrap();
    assert_eq!(
        stored.file, incoming.file,
        "validated documents also use server copy"
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn incoming_telegram_audio_copies_without_downloading_and_recovers_lost_ack() {
    let (server, binding) = fixture().await;
    let incoming = server
        .add_incoming_audio(
            "incoming.mp3".into(),
            vec![7; 42],
            "Remote song".into(),
            "Remote artist".into(),
            123_000,
        )
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("vault.sqlite");
    let profile_id = Uuid::new_v4();
    let start = || {
        VaultService::start(
            SyncEngine::new(
                server.client().unwrap(),
                Database::open(&database, binding).unwrap(),
                "device".into(),
            ),
            Context {
                profile_id,
                session_id: Uuid::new_v4(),
                vault_id: Some(binding.vault_id),
            },
            EventBus::default(),
        )
    };
    let service = start();
    ready(&service).await;
    let downloads = server.download_calls().unwrap();
    let op_id = Uuid::new_v4();
    let track_id = Uuid::new_v4();
    let source_id = Uuid::new_v4();
    let request = VaultRequest::ImportIncomingAudio {
        source_chat_id: incoming.chat_id,
        message_id: incoming.id,
        track_id,
        source_id,
        op_id,
        allow_duplicate: false,
    };
    server
        .set_faults(Faults {
            lost_ack: true,
            ..Default::default()
        })
        .unwrap();
    let cancel = CancellationToken::new();
    assert_eq!(
        service
            .request(request.clone(), &cancel)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Timeout
    );
    let uploaded: Vec<_> = server
        .messages()
        .unwrap()
        .into_iter()
        .filter(|message| matches!(message.record().unwrap(), Some(Record::Media(_))))
        .collect();
    assert_eq!(uploaded.len(), 1);
    assert_eq!(
        uploaded[0].file, incoming.file,
        "copy reuses the server file"
    );
    assert_eq!(downloads, server.download_calls().unwrap());
    service.shutdown().await.unwrap();
    drop(service);
    let service = start();
    ready(&service).await;
    let downloads = server.download_calls().unwrap();
    let response = service.request(request.clone(), &cancel).await.unwrap();
    assert!(
        matches!(response, VaultResponse::Command(command) if command.status == CommandStatus::Applied)
    );
    let state = service.reader().state().await.unwrap();
    let track = &state.tracks[&track_id];
    assert_eq!(track.metadata.title, "Remote song");
    assert_eq!(track.metadata.performer, "Remote artist");
    assert_eq!(track.metadata.duration_ms, 123_000);
    assert_eq!(
        track.sources[&source_id].sha256, None,
        "server copy does not invent a hash"
    );
    assert_eq!(track.sources[&source_id].message_id, uploaded[0].id);
    let before = server.messages().unwrap();
    service.request(request, &cancel).await.unwrap();
    assert_eq!(before, server.messages().unwrap());
    assert_eq!(downloads, server.download_calls().unwrap());
    let recipient = ChatId::new(4242).unwrap();
    let share_op = Uuid::new_v4();
    let share = VaultRequest::ShareTrack {
        op_id: share_op,
        track_id,
        source_id,
        recipient_chat_id: recipient,
    };
    server
        .set_faults(Faults {
            lost_ack: true,
            ..Default::default()
        })
        .unwrap();
    let unknown = service.request(share.clone(), &cancel).await.unwrap();
    assert!(
        matches!(unknown, VaultResponse::Shared(info) if info.state == ShareState::Unknown && info.message_id.is_none())
    );
    assert_eq!(server.shared_messages().unwrap().len(), 1);
    service.shutdown().await.unwrap();
    drop(service);
    let service = start();
    ready(&service).await;
    let unknown = service.request(share, &cancel).await.unwrap();
    assert!(matches!(unknown, VaultResponse::Shared(info) if info.state == ShareState::Unknown));
    assert_eq!(
        server.shared_messages().unwrap().len(),
        1,
        "unknown share must not be repeated after restart"
    );
    assert_eq!(
        service
            .request(
                VaultRequest::ShareTrack {
                    op_id: share_op,
                    track_id,
                    source_id,
                    recipient_chat_id: ChatId::new(4243).unwrap(),
                },
                &cancel
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    let sent_op = Uuid::new_v4();
    let share = VaultRequest::ShareTrack {
        op_id: sent_op,
        track_id,
        source_id,
        recipient_chat_id: recipient,
    };
    for _ in 0..2 {
        let sent = service.request(share.clone(), &cancel).await.unwrap();
        assert!(
            matches!(sent, VaultResponse::Shared(info) if info.state == ShareState::Sent && info.message_id.is_some())
        );
    }
    assert_eq!(
        server.shared_messages().unwrap().len(),
        2,
        "duplicate UI requests share one durable operation"
    );
    let rejected = service
        .request(
            VaultRequest::ShareTrack {
                op_id: Uuid::new_v4(),
                track_id,
                source_id,
                recipient_chat_id: binding.chat_id,
            },
            &cancel,
        )
        .await
        .unwrap();
    assert!(matches!(rejected, VaultResponse::Shared(info) if info.state == ShareState::NotSent));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn incoming_audio_imports_separate_cover_without_reading_audio_bytes() {
    let (server, binding) = fixture().await;
    let incoming = server
        .add_incoming_audio(
            "song.mp3".into(),
            vec![7; 42],
            "Song".into(),
            "Artist".into(),
            1000,
        )
        .unwrap();
    let cover = vec![0xff, 0xd8, 0xff, 0xe0, 0, 0, 0xff, 0xd9];
    server
        .set_audio_thumbnail(incoming.id, cover.clone(), "image/jpeg".into())
        .unwrap();
    let service = service(&server, binding, EventBus::default());
    ready(&service).await;
    let before = server.download_calls().unwrap();
    let track_id = Uuid::new_v4();
    let request = VaultRequest::ImportIncomingAudio {
        source_chat_id: incoming.chat_id,
        message_id: incoming.id,
        track_id,
        source_id: Uuid::new_v4(),
        op_id: Uuid::new_v4(),
        allow_duplicate: false,
    };
    let cancel = CancellationToken::new();
    service.request(request.clone(), &cancel).await.unwrap();
    assert_eq!(
        server.download_calls().unwrap() - before,
        2,
        "thumbnail read and cover verification only"
    );
    let state = service.reader().state().await.unwrap();
    let artwork = state.tracks[&track_id].metadata.artwork.as_ref().unwrap();
    assert_eq!(artwork.sha256, vault_snapshot::digest(&cover));
    assert!(artwork.source.is_some());
    let downloads = server.download_calls().unwrap();
    let messages = server.messages().unwrap();
    service.request(request, &cancel).await.unwrap();
    assert_eq!(downloads, server.download_calls().unwrap());
    assert_eq!(messages, server.messages().unwrap());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn incoming_copy_restrictions_offline_and_unknown_sends_never_upload_a_fallback() {
    for document in [false, true] {
        let (server, binding) = fixture().await;
        let incoming = if document {
            server
                .add_incoming_document("track.wav".into(), "audio/wav".into(), wav_fixture())
                .unwrap()
        } else {
            server
                .add_incoming_audio(
                    "track.mp3".into(),
                    vec![7; 42],
                    "Title".into(),
                    "Artist".into(),
                    123_000,
                )
                .unwrap()
        };
        let service = service(&server, binding, EventBus::default());
        ready(&service).await;
        let op_id = Uuid::new_v4();
        let request = VaultRequest::ImportIncomingAudio {
            source_chat_id: incoming.chat_id,
            message_id: incoming.id,
            track_id: Uuid::new_v4(),
            source_id: Uuid::new_v4(),
            op_id,
            allow_duplicate: false,
        };
        let cancel = CancellationToken::new();
        let before = server.messages().unwrap();
        let downloads = server.download_calls().unwrap();
        server
            .set_faults(Faults {
                prohibit_copy: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            service
                .request(request.clone(), &cancel)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Conflict
        );
        assert_eq!(downloads, server.download_calls().unwrap());
        server
            .set_faults(Faults {
                offline: true,
                ..Default::default()
            })
            .unwrap();
        assert!(service.request(request.clone(), &cancel).await.is_err());
        assert_eq!(downloads, server.download_calls().unwrap());
        assert_eq!(before, server.messages().unwrap());
        server.set_faults(Faults::default()).unwrap();
        service.request(VaultRequest::Sync, &cancel).await.unwrap();
        server
            .set_faults(Faults {
                document_timeout_before_send: true,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            service
                .request(request.clone(), &cancel)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Timeout
        );
        for _ in 0..2 {
            assert_eq!(
                service
                    .request(request.clone(), &cancel)
                    .await
                    .unwrap_err()
                    .code,
                ErrorCode::Timeout
            );
        }
        assert_eq!(before, server.messages().unwrap());
        assert!(service.reader().command(op_id).await.unwrap().is_none());
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn oversized_import_is_rejected_before_journaling_or_uploading() {
    let (server, binding) = fixture().await;
    let service = service(&server, binding, EventBus::default());
    ready(&service).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("size-limited.wav");
    let bytes = wav_fixture();
    std::fs::write(&path, &bytes).unwrap();
    let op_id = Uuid::new_v4();
    let request = VaultRequest::Import {
        path: path.to_string_lossy().into_owned(),
        track_id: Uuid::new_v4(),
        source_id: Uuid::new_v4(),
        op_id,
        allow_duplicate: false,
    };
    let before = server.messages().unwrap();
    server
        .set_faults(Faults {
            upload_limit_bytes: Some(bytes.len() as u64 - 1),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        service
            .request(request.clone(), &CancellationToken::new())
            .await
            .unwrap_err()
            .code,
        ErrorCode::InvalidInput
    );
    assert_eq!(server.messages().unwrap(), before);
    assert!(service.reader().command(op_id).await.unwrap().is_none());
    assert!(
        service
            .reader()
            .local_settings(format!("import.v1:{op_id}"), 0, 1)
            .await
            .unwrap()
            .is_empty()
    );
    // An account limit change allows the same intent without a phantom unknown
    // upload or abandoned staging entry from the rejected attempt.
    server
        .set_faults(Faults {
            upload_limit_bytes: Some(bytes.len() as u64),
            ..Default::default()
        })
        .unwrap();
    service
        .request(request, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(
        service
            .reader()
            .command(op_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        CommandStatus::Applied
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn unknown_upload_without_visible_marker_never_blindly_retries() {
    let (server, binding) = fixture().await;
    let service = service(&server, binding, EventBus::default());
    ready(&service).await;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uncertain.wav");
    std::fs::write(&path, wav_fixture()).unwrap();
    let op_id = Uuid::new_v4();
    let token = CancellationToken::new();
    let before = server.messages().unwrap();
    server
        .set_faults(Faults {
            document_timeout_before_send: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        service
            .request(
                VaultRequest::Import {
                    path: path.to_string_lossy().into_owned(),
                    track_id: Uuid::new_v4(),
                    source_id: Uuid::new_v4(),
                    op_id,
                    allow_duplicate: false
                },
                &token
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Timeout
    );
    for _ in 0..3 {
        assert_eq!(
            service
                .request(VaultRequest::ResumeImport { op_id }, &token)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Timeout
        );
    }
    assert_eq!(server.messages().unwrap(), before);
    assert!(service.reader().command(op_id).await.unwrap().is_none());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn upload_server_errors_remain_unknown_until_the_marker_is_found() {
    for accepted in [false, true] {
        let (server, binding) = fixture().await;
        let service = service(&server, binding, EventBus::default());
        ready(&service).await;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("server-error.wav");
        std::fs::write(&path, wav_fixture()).unwrap();
        let op_id = Uuid::new_v4();
        let before = server.messages().unwrap();
        server
            .set_faults(Faults {
                send_error_before: (!accepted).then_some(500),
                send_error_after: accepted.then_some(500),
                ..Default::default()
            })
            .unwrap();
        let cancel = CancellationToken::new();
        assert_eq!(
            service
                .request(
                    VaultRequest::Import {
                        path: path.to_string_lossy().into_owned(),
                        track_id: Uuid::new_v4(),
                        source_id: Uuid::new_v4(),
                        op_id,
                        allow_duplicate: false,
                    },
                    &cancel
                )
                .await
                .unwrap_err()
                .code,
            ErrorCode::Network
        );
        for _ in 0..2 {
            let result = service
                .request(VaultRequest::ResumeImport { op_id }, &cancel)
                .await;
            if accepted {
                assert!(
                    matches!(result.unwrap(), VaultResponse::Command(command) if command.status == vault_db::CommandStatus::Applied)
                );
            } else {
                assert_eq!(result.unwrap_err().code, ErrorCode::Timeout);
            }
        }
        if accepted {
            assert_eq!(server.messages().unwrap().iter().filter(|message| matches!(message.record().unwrap(), Some(Record::Media(marker)) if marker.import_id == op_id)).count(), 1);
        } else {
            assert_eq!(server.messages().unwrap(), before);
        }
        service.shutdown().await.unwrap();
    }
}

enum ImportRestart {
    Prepared,
    PublishedBeforeCheckpoint,
    CorruptCopy,
}

async fn prepared_import_survives_restart(incoming_audio: bool, restart: ImportRestart) {
    let (server, binding) = fixture().await;
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("vault.sqlite");
    let profile_id = Uuid::new_v4();
    let start = || {
        VaultService::start(
            SyncEngine::new(
                server.client().unwrap(),
                Database::open(&database, binding).unwrap(),
                "device".into(),
            ),
            Context {
                profile_id,
                session_id: Uuid::new_v4(),
                vault_id: Some(binding.vault_id),
            },
            EventBus::default(),
        )
    };
    let original = directory.path().join("original.wav");
    std::fs::write(&original, wav_fixture()).unwrap();
    let op_id = Uuid::new_v4();
    let track_id = Uuid::new_v4();
    let source_id = Uuid::new_v4();
    let request = if incoming_audio {
        let incoming = server
            .add_incoming_document("original.wav".into(), "audio/wav".into(), wav_fixture())
            .unwrap();
        VaultRequest::ImportIncomingAudio {
            source_chat_id: incoming.chat_id,
            message_id: incoming.id,
            track_id,
            source_id,
            op_id,
            allow_duplicate: false,
        }
    } else {
        VaultRequest::Import {
            path: original.to_string_lossy().into_owned(),
            track_id,
            source_id,
            op_id,
            allow_duplicate: false,
        }
    };
    let first = start();
    ready(&first).await;
    server
        .set_faults(Faults {
            reject_document: true,
            ..Default::default()
        })
        .unwrap();
    let token = CancellationToken::new();
    assert_eq!(
        first.request(request, &token).await.unwrap_err().code,
        ErrorCode::TelegramRejected
    );
    assert!(
        !server
            .messages()
            .unwrap()
            .iter()
            .any(|message| matches!(message.record().unwrap(), Some(Record::Media(_))))
    );
    first.shutdown().await.unwrap();
    drop(first);
    let staged_dir = directory
        .path()
        .join("vault.sqlite.imports")
        .join(binding.vault_id.to_string())
        .join(op_id.to_string());
    let staged = staged_dir.join("original.wav");
    assert_eq!(std::fs::read(&staged).unwrap(), wav_fixture());
    match restart {
        ImportRestart::Prepared => {}
        ImportRestart::PublishedBeforeCheckpoint => {
            let mut db = Database::open(&database, binding).unwrap();
            let key = format!("import.v1:{op_id}");
            let mut journal: serde_json::Value =
                serde_json::from_str(&db.local_setting(&key).unwrap().unwrap()).unwrap();
            journal["info"]["stage"] = "staging".into();
            journal["path"] = original.to_string_lossy().into_owned().into();
            db.set_local_setting(&key, &journal.to_string()).unwrap();
            let scratch = staged_dir.join(".copy");
            std::fs::create_dir(&scratch).unwrap();
            std::fs::write(scratch.join("interrupted-copy"), b"partial").unwrap();
        }
        ImportRestart::CorruptCopy => {
            let mut damaged = wav_fixture();
            damaged[44] ^= 1;
            std::fs::write(&staged, damaged).unwrap();
        }
    }
    std::fs::remove_file(original).unwrap();

    let second = start();
    ready(&second).await;
    let reply = second
        .request(VaultRequest::ResumeImport { op_id }, &token)
        .await;
    if matches!(restart, ImportRestart::CorruptCopy) {
        assert_eq!(reply.unwrap_err().code, ErrorCode::Integrity);
        assert!(
            !server
                .messages()
                .unwrap()
                .iter()
                .any(|message| matches!(message.record().unwrap(), Some(Record::Media(_))))
        );
        assert!(
            staged.exists(),
            "corruption must not trigger overwrite or deletion"
        );
        second.shutdown().await.unwrap();
        return;
    }
    let reply = reply.unwrap();
    assert!(matches!(reply, VaultResponse::Command(c) if c.status == CommandStatus::Applied));
    let track = second.reader().state().await.unwrap().tracks[&track_id].clone();
    assert_eq!(
        track.metadata.title, "original",
        "staging must preserve filename fallback metadata"
    );
    assert_eq!(
        track.sources[&source_id].size_bytes,
        Some(wav_fixture().len() as u64)
    );
    assert_eq!(
        server
            .messages()
            .unwrap()
            .iter()
            .filter(|message| matches!(message.record().unwrap(), Some(Record::Media(_))))
            .count(),
        1
    );
    assert!(
        !staged_dir.exists(),
        "applied imports release their durable copy"
    );
    let uploaded = server
        .messages()
        .unwrap()
        .into_iter()
        .find(|message| matches!(message.record().unwrap(), Some(Record::Media(_))))
        .unwrap();
    assert_eq!(uploaded.file.unwrap().name, "original.wav");
    second.shutdown().await.unwrap();
}

#[tokio::test]
async fn prepared_local_import_resumes_after_original_disappears() {
    prepared_import_survives_restart(false, ImportRestart::Prepared).await;
}

#[tokio::test]
async fn prepared_incoming_import_resumes_after_temporary_download_disappears() {
    prepared_import_survives_restart(true, ImportRestart::Prepared).await;
}

#[tokio::test]
async fn published_import_copy_survives_crash_before_prepared_checkpoint() {
    prepared_import_survives_restart(false, ImportRestart::PublishedBeforeCheckpoint).await;
}

#[tokio::test]
async fn damaged_prepared_import_is_not_uploaded_or_silently_replaced() {
    prepared_import_survives_restart(false, ImportRestart::CorruptCopy).await;
}

#[tokio::test]
async fn import_cleanup_recovers_after_command_commit_and_preserves_unclassified_files() {
    let (server, binding) = fixture().await;
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("vault.sqlite");
    let start = || {
        VaultService::start(
            SyncEngine::new(
                server.client().unwrap(),
                Database::open(&database, binding).unwrap(),
                "device".into(),
            ),
            Context {
                profile_id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                vault_id: Some(binding.vault_id),
            },
            EventBus::default(),
        )
    };
    let original = directory.path().join("original.wav");
    std::fs::write(&original, wav_fixture()).unwrap();
    let op_id = Uuid::new_v4();
    let token = CancellationToken::new();
    let first = start();
    ready(&first).await;
    first
        .request(
            VaultRequest::Import {
                path: original.to_string_lossy().into_owned(),
                track_id: Uuid::new_v4(),
                source_id: Uuid::new_v4(),
                op_id,
                allow_duplicate: false,
            },
            &token,
        )
        .await
        .unwrap();
    first.shutdown().await.unwrap();
    drop(first);
    let root = directory
        .path()
        .join("vault.sqlite.imports")
        .join(binding.vault_id.to_string());
    let staged_dir = root.join(op_id.to_string());
    std::fs::create_dir(&staged_dir).unwrap();
    std::fs::copy(&original, staged_dir.join("original.wav")).unwrap();
    let orphan_dir = root.join(Uuid::new_v4().to_string());
    std::fs::create_dir(&orphan_dir).unwrap();
    let orphan = orphan_dir.join("unclassified.wav");
    std::fs::copy(&original, &orphan).unwrap();
    {
        let mut db = Database::open(&database, binding).unwrap();
        let key = format!("import.v1:{op_id}");
        let mut journal: serde_json::Value =
            serde_json::from_str(&db.local_setting(&key).unwrap().unwrap()).unwrap();
        journal["info"]["stage"] = "uploaded".into();
        journal["staged_name"] = "original.wav".into();
        db.set_local_setting(&key, &journal.to_string()).unwrap();
    }
    let messages = server.messages().unwrap();
    let second = start();
    ready(&second).await;
    assert!(!staged_dir.exists());
    assert_eq!(std::fs::read(&orphan).unwrap(), wav_fixture());
    assert_eq!(std::fs::read(&original).unwrap(), wav_fixture());
    assert_eq!(server.messages().unwrap(), messages);
    second.shutdown().await.unwrap();
}

#[tokio::test]
async fn import_recovers_uploaded_media_after_lost_ack_and_restart_without_the_original_file() {
    let (server, binding) = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("vault.sqlite");
    let start = || {
        VaultService::start(
            SyncEngine::new(
                server.client().unwrap(),
                Database::open(&database, binding).unwrap(),
                "device".into(),
            ),
            Context {
                profile_id: Uuid::new_v4(),
                session_id: Uuid::new_v4(),
                vault_id: Some(binding.vault_id),
            },
            EventBus::default(),
        )
    };
    let first = start();
    ready(&first).await;
    let path = dir.path().join("resumable.wav");
    std::fs::write(&path, wav_fixture()).unwrap();
    let track_id = Uuid::new_v4();
    let source_id = Uuid::new_v4();
    let op_id = Uuid::new_v4();
    let token = CancellationToken::new();
    server
        .set_faults(Faults {
            lost_ack: true,
            ..Default::default()
        })
        .unwrap();
    assert!(
        first
            .request(
                VaultRequest::Import {
                    path: path.to_string_lossy().into_owned(),
                    track_id,
                    source_id,
                    op_id,
                    allow_duplicate: false
                },
                &token
            )
            .await
            .is_err()
    );
    let uploaded = server
        .messages()
        .unwrap()
        .into_iter()
        .filter(|m| matches!(m.record().unwrap(), Some(Record::Media(_))))
        .collect::<Vec<_>>();
    assert_eq!(uploaded.len(), 1);
    assert!(
        matches!(first.request(VaultRequest::InspectOrphanMedia, &token).await.unwrap(), VaultResponse::OrphanMedia(items) if items.len() == 1 && items[0].recoverable_import)
    );
    first.shutdown().await.unwrap();
    drop(first);
    std::fs::remove_file(path).unwrap();
    let second = start();
    ready(&second).await;
    let resumed = second
        .request(VaultRequest::ResumeImport { op_id }, &token)
        .await
        .unwrap();
    assert!(matches!(resumed, VaultResponse::Command(c) if c.status == CommandStatus::Applied));
    let track = second.reader().state().await.unwrap().tracks[&track_id].clone();
    assert_eq!(track.sources[&source_id].message_id, uploaded[0].id);
    let before = server.messages().unwrap();
    let again = second
        .request(VaultRequest::ResumeImport { op_id }, &token)
        .await
        .unwrap();
    assert!(matches!(again, VaultResponse::Command(c) if c.status == CommandStatus::Applied));
    assert_eq!(
        server.messages().unwrap(),
        before,
        "repeated resume must not upload another file or register another event"
    );
    let orphans = second
        .request(VaultRequest::InspectOrphanMedia, &token)
        .await
        .unwrap();
    assert!(matches!(orphans, VaultResponse::OrphanMedia(items) if items.is_empty()));
    second.shutdown().await.unwrap();
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
                op_id: Uuid::new_v4(),
                track_id: track,
                source_id: source,
                recipient_chat_id: recipient,
            },
            &token,
        )
        .await
        .unwrap();
    assert!(
        matches!(shared, VaultResponse::Shared(info) if info.recipient_chat_id == recipient && info.state == ShareState::Sent)
    );
    assert_eq!(
        server.shared_messages().unwrap(),
        vec![(binding.chat_id, media.id, recipient)]
    );
    assert!(
        service
            .request(
                VaultRequest::ShareTrack {
                    op_id: Uuid::new_v4(),
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

#[tokio::test]
async fn offline_download_uses_verified_cache_progress_status_and_pin() {
    let (server, binding) = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let cache = vault_media::CacheManager::open(dir.path().join("cache"), 2_000_000).unwrap();
    let service = service_with_cache(&server, binding, EventBus::default(), cache.clone());
    ready(&service).await;
    let path = dir.path().join("offline.wav");
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
    let track_id = Uuid::new_v4();
    let source_id = Uuid::new_v4();
    let imported = service
        .request(
            VaultRequest::Import {
                path: path.to_string_lossy().into_owned(),
                track_id,
                source_id,
                op_id: Uuid::new_v4(),
                allow_duplicate: false,
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(imported, VaultResponse::Command(_)));
    service
        .request(VaultRequest::Sync, &CancellationToken::new())
        .await
        .unwrap();

    let resume_key = format!("{track_id}:{source_id}");
    let partial_lease = cache
        .prepare_partial_download(&resume_key, 123, 1_500_000, None)
        .unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(partial_lease.path())
        .unwrap()
        .set_len(1024 * 1024)
        .unwrap();
    let partial = service
        .download_status(Uuid::new_v4(), track_id, source_id)
        .unwrap();
    assert_eq!(partial.state, DownloadState::PartiallyCached);
    assert_eq!(partial.bytes_downloaded, 1024 * 1024);
    assert_eq!(partial.total_bytes, Some(1_500_000));
    partial_lease.discard().unwrap();

    server
        .set_faults(Faults {
            download_delay_ms: 3_000,
            ..Default::default()
        })
        .unwrap();
    let cancelled_id = Uuid::new_v4();
    let downloading = service.clone();
    let cancellation = tokio::spawn(async move {
        downloading
            .request(
                VaultRequest::DownloadForOffline {
                    download_id: cancelled_id,
                    track_id,
                    source_id,
                    pinned: true,
                },
                &CancellationToken::new(),
            )
            .await
    });
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let status = service
                .request(
                    VaultRequest::GetDownloadStatus {
                        download_id: cancelled_id,
                        track_id,
                        source_id,
                    },
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            if matches!(
                status,
                VaultResponse::Download(DownloadInfo {
                    state: DownloadState::Downloading,
                    ..
                })
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        service
            .request(
                VaultRequest::DownloadForOffline {
                    download_id: Uuid::new_v4(),
                    track_id,
                    source_id,
                    pinned: false,
                },
                &CancellationToken::new(),
            )
            .await,
        Err(ApiError {
            code: ErrorCode::Conflict,
            ..
        })
    ));
    tokio::time::timeout(std::time::Duration::from_secs(1), async {
        while server.download_calls().unwrap() == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    service
        .request(
            VaultRequest::CancelDownload {
                download_id: cancelled_id,
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(
        cancellation.await.unwrap().unwrap(),
        VaultResponse::Download(DownloadInfo {
            state: DownloadState::Downloading,
            ..
        })
    ));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let status = service
                .request(
                    VaultRequest::GetDownloadStatus {
                        download_id: cancelled_id,
                        track_id,
                        source_id,
                    },
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            if matches!(
                status,
                VaultResponse::Download(DownloadInfo {
                    state: DownloadState::Cancelled,
                    ..
                })
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(cache.entries().unwrap().is_empty());
    server.set_faults(Faults::default()).unwrap();

    let download_id = Uuid::new_v4();
    let result = service
        .request(
            VaultRequest::DownloadForOffline {
                download_id,
                track_id,
                source_id,
                pinned: true,
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(
        result,
        VaultResponse::Download(DownloadInfo {
            state: DownloadState::Downloading,
            pinned: true,
            ..
        })
    ));
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let status = service
                .request(
                    VaultRequest::GetDownloadStatus {
                        download_id,
                        track_id,
                        source_id,
                    },
                    &CancellationToken::new(),
                )
                .await
                .unwrap();
            if matches!(
                status,
                VaultResponse::Download(DownloadInfo {
                    state: DownloadState::FullyCached,
                    ..
                })
            ) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let status = service
        .request(
            VaultRequest::GetDownloadStatus {
                download_id,
                track_id,
                source_id,
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(
        status,
        VaultResponse::Download(DownloadInfo {
            state: DownloadState::FullyCached,
            ..
        })
    ));
    assert_eq!(cache.entries().unwrap().len(), 1);
    assert!(matches!(
        service
            .request(VaultRequest::GetCacheStats, &CancellationToken::new())
            .await
            .unwrap(),
        VaultResponse::CacheStats(vault_media::CacheStats {
            entries: 1,
            pinned_entries: 1,
            partial_entries: 0,
            partial_bytes: 0,
            reserved_bytes: 0,
            used_bytes: 16_044,
            quota_bytes: 2_000_000,
        })
    ));
    assert_eq!(server.shared_messages().unwrap().len(), 0);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn full_backup_restores_to_empty_vault_with_new_track_and_media_references() {
    let (server, binding) = fixture().await;
    let service = service(&server, binding, EventBus::default());
    ready(&service).await;
    let temp = tempfile::tempdir().unwrap();
    let media_path = temp.path().join("audio.bin");
    let media = b"portable full backup media";
    std::fs::write(&media_path, media).unwrap();
    let source_vault = Uuid::new_v4();
    let old_track = Uuid::new_v4();
    let old_source_id = Uuid::new_v4();
    let old_playlist = Uuid::new_v4();
    let old_entry = Uuid::new_v4();
    let old_chat = ChatId::new(-200).unwrap();
    let mut source_state = State::new(source_vault);
    source_state
        .apply(
            MessageId::new(10).unwrap(),
            &Event {
                vault_id: source_vault,
                op_id: Uuid::new_v4(),
                device_id: "source-device".into(),
                base_generation: 0,
                entity_id: old_track,
                action: Action::TrackRegister {
                    metadata: Metadata {
                        title: "Restored song".into(),
                        performer: "Kiyora test".into(),
                        ..Default::default()
                    },
                    source: Source {
                        source_id: old_source_id,
                        chat_id: old_chat,
                        message_id: MessageId::new(2_097_152).unwrap(),
                        sha256: Some(vault_snapshot::digest(media)),
                        size_bytes: Some(media.len() as u64),
                    },
                },
            },
        )
        .unwrap();
    source_state
        .apply(
            MessageId::new(20).unwrap(),
            &Event {
                vault_id: source_vault,
                op_id: Uuid::new_v4(),
                device_id: "source-device".into(),
                base_generation: 0,
                entity_id: old_playlist,
                action: Action::PlaylistCreate {
                    title: "Saved playlist".into(),
                },
            },
        )
        .unwrap();
    source_state
        .apply(
            MessageId::new(30).unwrap(),
            &Event {
                vault_id: source_vault,
                op_id: Uuid::new_v4(),
                device_id: "source-device".into(),
                base_generation: 0,
                entity_id: old_playlist,
                action: Action::PlaylistTrackAdd {
                    track_id: old_track,
                    entry_id: old_entry,
                    after_entry_id: None,
                },
            },
        )
        .unwrap();
    let archive = temp.path().join("portable.tmv-backup");
    vault_snapshot::backup::export(
        &archive,
        &source_state,
        binding.owner_id,
        None,
        vault_snapshot::backup::BackupKind::Full,
        &std::collections::BTreeMap::from([(old_track, media_path)]),
        Default::default(),
    )
    .unwrap();

    let result = service
        .request(
            VaultRequest::ImportBackup {
                path: archive.to_string_lossy().into_owned(),
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(matches!(
        result,
        VaultResponse::BackupRestored {
            source_vault_id,
            tracks: 1,
            playlists: 1,
            operations: 4,
        } if source_vault_id == source_vault
    ));
    let restored = service.reader().state().await.unwrap();
    assert_eq!(restored.tracks.len(), 1);
    assert_eq!(restored.playlists.len(), 1);
    let (new_track, track) = restored.tracks.iter().next().unwrap();
    assert_ne!(*new_track, old_track);
    assert_eq!(track.metadata.title, "Restored song");
    let (new_source_id, source) = track.sources.iter().next().unwrap();
    assert_ne!(*new_source_id, old_source_id);
    assert_eq!(source.chat_id, binding.chat_id);
    assert_ne!(source.message_id, MessageId::new(2_097_152).unwrap());
    let entry = restored.entries.values().next().unwrap();
    assert_ne!(entry.entry_id, old_entry);
    assert_eq!(entry.track_id, *new_track);
    let uploaded = server
        .messages()
        .unwrap()
        .into_iter()
        .find(|message| {
            message.file.is_some() && matches!(message.record(), Ok(Some(Record::Media(_))))
        })
        .unwrap();
    let download_client = server.client().unwrap();
    let range = download_client
        .download(uploaded.file.unwrap(), 0, 0, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(std::fs::read(range.path).unwrap(), media);

    let before_retry = server.messages().unwrap();
    assert!(
        service
            .request(
                VaultRequest::ImportBackup {
                    path: archive.to_string_lossy().into_owned(),
                },
                &CancellationToken::new(),
            )
            .await
            .is_err()
    );
    assert_eq!(server.messages().unwrap(), before_retry);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn metadata_backup_copies_verified_available_media_and_rejects_missing_sources() {
    for missing in [false, true] {
        let (server, binding) = fixture().await;
        let service = service(&server, binding, EventBus::default());
        ready(&service).await;
        let media = wav_fixture();
        let original = server
            .add_incoming_document("saved.wav".into(), "audio/wav".into(), media.clone())
            .unwrap();
        let source_vault = Uuid::new_v4();
        let track_id = Uuid::new_v4();
        let mut state = State::new(source_vault);
        state
            .apply(
                MessageId::new(1).unwrap(),
                &Event {
                    vault_id: source_vault,
                    op_id: Uuid::new_v4(),
                    device_id: "backup-test".into(),
                    base_generation: 0,
                    entity_id: track_id,
                    action: Action::TrackRegister {
                        metadata: Metadata {
                            title: "Recovered metadata".into(),
                            ..Default::default()
                        },
                        source: Source {
                            source_id: Uuid::new_v4(),
                            chat_id: binding.chat_id,
                            message_id: original.id,
                            sha256: Some(vault_snapshot::digest(&media)),
                            size_bytes: Some(media.len() as u64),
                        },
                    },
                },
            )
            .unwrap();
        let temporary = tempfile::tempdir().unwrap();
        let archive = temporary.path().join("metadata.tmv-backup");
        vault_snapshot::backup::export(
            &archive,
            &state,
            binding.owner_id,
            None,
            vault_snapshot::backup::BackupKind::Metadata,
            &Default::default(),
            Default::default(),
        )
        .unwrap();
        if missing {
            server.manual_delete(original.id).unwrap();
        }
        let before = server.messages().unwrap();
        let result = service
            .request(
                VaultRequest::ImportBackup {
                    path: archive.to_string_lossy().into_owned(),
                },
                &CancellationToken::new(),
            )
            .await;
        if missing {
            assert!(result.is_err());
            assert!(service.reader().state().await.unwrap().tracks.is_empty());
            assert_eq!(server.messages().unwrap(), before);
        } else {
            assert!(matches!(
                result.unwrap(),
                VaultResponse::BackupRestored { tracks: 1, .. }
            ));
            let restored = service.reader().state().await.unwrap();
            let track = restored.tracks.values().next().unwrap();
            assert_ne!(track.track_id, track_id);
            assert_eq!(track.metadata.title, "Recovered metadata");
            let source = track.sources.values().next().unwrap();
            assert_ne!(source.message_id, original.id);
            assert_eq!(source.sha256, Some(vault_snapshot::digest(&media)));
        }
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn deleted_media_has_unavailable_status_without_removing_metadata() {
    let (server, binding) = fixture().await;
    let service = service(&server, binding, EventBus::default());
    ready(&service).await;
    let temporary = tempfile::tempdir().unwrap();
    let path = temporary.path().join("source.wav");
    std::fs::write(&path, wav_fixture()).unwrap();
    let track_id = Uuid::new_v4();
    let source_id = Uuid::new_v4();
    service
        .request(
            VaultRequest::Import {
                path: path.to_string_lossy().into_owned(),
                track_id,
                source_id,
                op_id: Uuid::new_v4(),
                allow_duplicate: false,
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let original = service.reader().state().await.unwrap();
    let media_id = original.tracks[&track_id].sources[&source_id].message_id;
    server.manual_delete(media_id).unwrap();
    service
        .request(VaultRequest::Sync, &CancellationToken::new())
        .await
        .unwrap();
    let response = service
        .request(
            VaultRequest::CheckSources {
                track_id: Some(track_id),
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let VaultResponse::Sources(sources) = response else {
        panic!("source diagnostics expected")
    };
    assert_eq!(sources.len(), 1);
    assert_eq!(
        sources[0].availability,
        vault_sync::SourceAvailability::Unavailable
    );
    assert_eq!(
        service.reader().state().await.unwrap().tracks,
        original.tracks
    );
    assert_eq!(
        service.reader().source_health(track_id).await.unwrap(),
        sources
    );
    let transport = server.client().unwrap();
    let cancel = CancellationToken::new();
    let wrong = temporary.path().join("wrong.wav");
    let mut changed = wav_fixture();
    let last = changed.len() - 1;
    changed[last] ^= 1;
    std::fs::write(&wrong, changed).unwrap();
    let wrong = transport
        .send_document(binding.chat_id, wrong, String::new(), &cancel)
        .await
        .unwrap();
    assert!(
        service
            .request(
                VaultRequest::RelinkSource {
                    track_id,
                    source_id,
                    message_id: wrong.id,
                    op_id: Uuid::new_v4()
                },
                &cancel
            )
            .await
            .is_err()
    );
    assert_eq!(
        service.reader().state().await.unwrap().tracks,
        original.tracks
    );
    let replacement = transport
        .send_document(binding.chat_id, path, String::new(), &cancel)
        .await
        .unwrap();
    let op_id = Uuid::new_v4();
    let result = service
        .request(
            VaultRequest::RelinkSource {
                track_id,
                source_id,
                message_id: replacement.id,
                op_id,
            },
            &cancel,
        )
        .await
        .unwrap();
    assert!(
        matches!(result,VaultResponse::Command(command) if command.status==CommandStatus::Applied)
    );
    let again = service
        .request(
            VaultRequest::RelinkSource {
                track_id,
                source_id,
                message_id: replacement.id,
                op_id,
            },
            &cancel,
        )
        .await
        .unwrap();
    assert!(
        matches!(again,VaultResponse::Command(command) if command.status==CommandStatus::Applied)
    );
    let track = &service.reader().state().await.unwrap().tracks[&track_id];
    assert_eq!(track.metadata, original.tracks[&track_id].metadata);
    assert_eq!(track.sources[&source_id].message_id, replacement.id);
    assert!(
        matches!(service.request(VaultRequest::CheckSources {track_id:Some(track_id)},&cancel).await.unwrap(),VaultResponse::Sources(sources) if sources[0].availability==vault_sync::SourceAvailability::Available)
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn streaming_input_resolves_registered_source_and_expires_with_session() {
    use std::io::Read;
    let (server, binding) = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let cache = vault_media::CacheManager::open(dir.path().join("cache"), 2_000_000).unwrap();
    let service = service_with_cache(&server, binding, EventBus::default(), cache.clone());
    ready(&service).await;
    let path = dir.path().join("stream.wav");
    let bytes = wav_fixture();
    std::fs::write(&path, &bytes).unwrap();
    let track_id = Uuid::new_v4();
    let source_id = Uuid::new_v4();
    service
        .request(
            VaultRequest::Import {
                path: path.to_string_lossy().into_owned(),
                track_id,
                source_id,
                op_id: Uuid::new_v4(),
                allow_duplicate: false,
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        service
            .playback_input(track_id, Uuid::new_v4(), &CancellationToken::new())
            .await
            .is_err()
    );
    let input = service
        .playback_input(track_id, source_id, &CancellationToken::new())
        .await
        .unwrap();
    assert!(cache.entries().unwrap().is_empty());
    let vault_media::PlaybackInput::Remote(stream) = input else {
        panic!("expected remote source");
    };
    let read_stream = stream.clone();
    let observed = tokio::task::spawn_blocking(move || {
        let mut reader = read_stream.reader(CancellationToken::new());
        let mut bytes = [0u8; 12];
        reader.read_exact(&mut bytes).unwrap();
        bytes
    })
    .await
    .unwrap();
    assert_eq!(observed, bytes[..12]);
    service.shutdown().await.unwrap();
    assert!(
        tokio::task::spawn_blocking(move || {
            stream
                .reader(CancellationToken::new())
                .read_exact(&mut [0u8; 12])
        })
        .await
        .unwrap()
        .is_err()
    );
}

#[tokio::test]
async fn remote_playlist_replay_emits_invalidation_and_local_diagnostics() {
    let (server, binding) = fixture().await;
    let bus = EventBus::default();
    let mut events = bus.subscribe();
    let reader = service(&server, binding, bus);
    ready(&reader).await;
    while events.try_recv().is_ok() {}
    let mut writer = SyncEngine::new(
        server.client().unwrap(),
        Database::memory(binding).unwrap(),
        "other-device".into(),
    );
    let cancel = CancellationToken::new();
    writer.sync(&cancel).await.unwrap();
    let playlist = Uuid::new_v4();
    writer
        .execute(
            Uuid::new_v4(),
            playlist,
            Action::PlaylistCreate {
                title: "Remote playlist".into(),
            },
            &cancel,
        )
        .await
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if matches!(events.recv().await.unwrap().data, EventData::Playlist {playlist_id} if playlist_id == playlist) {break;}
        }
    }).await.unwrap();
    let diagnostics = reader.reader().diagnostics().await.unwrap();
    assert_eq!(diagnostics.active_playlists, 1);
    assert_eq!(diagnostics.uncovered_events, 1);
    reader.shutdown().await.unwrap();
}

#[tokio::test]
async fn import_retry_after_projection_rebuild_reuses_receipt_without_upload() {
    let (server, binding) = fixture().await;
    let first = service(&server, binding, EventBus::default());
    ready(&first).await;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("once.wav");
    std::fs::write(&path, wav_fixture()).unwrap();
    let track_id = Uuid::new_v4();
    let source_id = Uuid::new_v4();
    let op_id = Uuid::new_v4();
    let request = VaultRequest::Import {
        path: path.to_string_lossy().into_owned(),
        track_id,
        source_id,
        op_id,
        allow_duplicate: false,
    };
    let cancel = CancellationToken::new();
    first.request(request.clone(), &cancel).await.unwrap();
    first
        .request(
            VaultRequest::AssignGcCoordinator {
                op_id: Uuid::new_v4(),
                target: CoordinatorTarget::ThisDevice,
                expected_previous: None,
                force_confirmed: false,
            },
            &cancel,
        )
        .await
        .unwrap();
    first
        .request(VaultRequest::Snapshot, &cancel)
        .await
        .unwrap();
    first.shutdown().await.unwrap();
    std::fs::remove_file(path).unwrap();
    let before = server.messages().unwrap().len();
    let second = service(&server, binding, EventBus::default());
    ready(&second).await;
    let result = second.request(request, &cancel).await.unwrap();
    assert!(
        matches!(result,VaultResponse::Command(command) if command.status==CommandStatus::Applied)
    );
    assert_eq!(server.messages().unwrap().len(), before);
    second.shutdown().await.unwrap();
}

#[tokio::test]
async fn imported_embedded_artwork_is_published_verified_and_referenced() {
    let (server, binding) = fixture().await;
    let service = service(&server, binding, EventBus::default());
    ready(&service).await;
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("cover.wav");
    let cover = b"\x89PNG\r\n\x1a\nembedded-cover-fixture";
    let mut picture = vec![0];
    picture.extend_from_slice(b"image/png\0\x03\0");
    picture.extend_from_slice(cover);
    let mut frames = b"APIC".to_vec();
    frames.extend_from_slice(&(picture.len() as u32).to_be_bytes());
    frames.extend_from_slice(&[0, 0]);
    frames.extend_from_slice(&picture);
    let size = frames.len() as u32;
    let mut id3 = b"ID3\x03\0\0".to_vec();
    id3.extend_from_slice(&[
        ((size >> 21) & 127) as u8,
        ((size >> 14) & 127) as u8,
        ((size >> 7) & 127) as u8,
        (size & 127) as u8,
    ]);
    id3.extend_from_slice(&frames);
    let mut wav = wav_fixture();
    wav.extend_from_slice(b"id3 ");
    wav.extend_from_slice(&(id3.len() as u32).to_le_bytes());
    wav.extend_from_slice(&id3);
    if !id3.len().is_multiple_of(2) {
        wav.push(0);
    }
    let size = wav.len() as u32 - 8;
    wav[4..8].copy_from_slice(&size.to_le_bytes());
    std::fs::write(&path, wav).unwrap();
    let inspected = vault_media::inspect(&path).unwrap();
    assert_eq!(inspected.artwork.as_ref().unwrap().bytes, cover);
    let track_id = Uuid::new_v4();
    let op_id = Uuid::new_v4();
    let result = service
        .request(
            VaultRequest::Import {
                path: path.to_string_lossy().into_owned(),
                track_id,
                source_id: Uuid::new_v4(),
                op_id,
                allow_duplicate: false,
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(
        matches!(result,VaultResponse::Command(command) if command.status==CommandStatus::Applied)
    );
    let state = service.reader().state().await.unwrap();
    let art = state.tracks[&track_id].metadata.artwork.as_ref().unwrap();
    assert_eq!(art.sha256, vault_snapshot::digest(cover));
    let source = art.source.as_ref().unwrap();
    assert_eq!(source.source_id, Uuid::new_v5(&op_id, b"artwork"));
    let message = server
        .messages()
        .unwrap()
        .into_iter()
        .find(|m| m.id == source.message_id)
        .unwrap();
    assert!(
        matches!(message.record().unwrap(),Some(Record::Media(marker)) if marker.import_id==source.source_id)
    );
    assert_eq!(source.size_bytes, Some(cover.len() as u64));
    assert!(
        matches!(service.request(VaultRequest::InspectOrphanMedia,&CancellationToken::new()).await.unwrap(),VaultResponse::OrphanMedia(items) if items.is_empty())
    );
    let archive = temp.path().join("artwork-backup.tar");
    let cover_path = temp.path().join("cover.png");
    std::fs::write(&cover_path, cover).unwrap();
    vault_snapshot::backup::export_with_artwork(
        &archive,
        &state,
        binding.owner_id,
        None,
        vault_snapshot::backup::BackupKind::Full,
        &std::collections::BTreeMap::from([(track_id, path)]),
        &std::collections::BTreeMap::from([(track_id, cover_path)]),
        Default::default(),
    )
    .unwrap();
    let verified = vault_snapshot::backup::inspect(&archive, Default::default()).unwrap();
    assert_eq!(
        std::fs::read(verified.artwork_path(track_id).unwrap()).unwrap(),
        cover
    );
    let (target, target_binding) = fixture().await;
    // Separate fake servers use the same synthetic chat ID and ID sequence.
    // Advance this target so the archive's old message references cannot alias.
    for _ in 0..20 {
        target
            .client()
            .unwrap()
            .send_text(
                target_binding.chat_id,
                "test padding".into(),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
    }
    let restored = VaultService::start(
        SyncEngine::new(
            target.client().unwrap(),
            Database::memory(target_binding).unwrap(),
            "restore".into(),
        ),
        Context {
            profile_id: Uuid::new_v4(),
            session_id: Uuid::new_v4(),
            vault_id: Some(target_binding.vault_id),
        },
        EventBus::default(),
    );
    ready(&restored).await;
    restored
        .request(
            VaultRequest::ImportBackup {
                path: archive.to_string_lossy().into_owned(),
            },
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let restored_state = restored.reader().state().await.unwrap();
    let restored_art = restored_state
        .tracks
        .values()
        .next()
        .unwrap()
        .metadata
        .artwork
        .as_ref()
        .unwrap();
    assert_eq!(restored_art.sha256, art.sha256);
    let restored_source = restored_art.source.as_ref().unwrap();
    let restored_message = target
        .messages()
        .unwrap()
        .into_iter()
        .find(|m| m.id == restored_source.message_id)
        .unwrap();
    assert!(
        matches!(restored_message.record().unwrap(),Some(Record::Media(marker)) if marker.vault_id==target_binding.vault_id)
    );
    restored.shutdown().await.unwrap();
    service.shutdown().await.unwrap();
}

// UI management uses the real service, SQLite, reducer and media pipeline. Only
// the Telegram transport is account-free; no production IPC is replaced.
#[tokio::test]
async fn preview_original_upload_and_logical_edit_roundtrip_through_snapshot() {
    let (server, binding) = fixture().await;
    let service = service(&server, binding, EventBus::default());
    ready(&service).await;
    let token = CancellationToken::new();
    let dir = tempfile::tempdir().unwrap();
    let original = dir.path().join("original.wav");
    let bytes = wav_fixture();
    std::fs::write(&original, &bytes).unwrap();
    let id = Uuid::new_v4();
    let before = server.messages().unwrap().len();
    let preview = service
        .request(
            VaultRequest::PreviewImport {
                path: original.to_string_lossy().into_owned(),
                preview_id: id,
            },
            &token,
        )
        .await
        .unwrap();
    assert!(
        matches!(preview, VaultResponse::ImportPreview(p) if p.sha256 == vault_snapshot::digest(&bytes))
    );
    assert_eq!(
        server.messages().unwrap().len(),
        before,
        "preview must be local only"
    );
    let upload = VaultRequest::ImportPrepared {
        preview_id: id,
        track_id: id,
        source_id: id,
        edit: None,
        write_tags: false,
        allow_duplicate: false,
    };
    let result = service.request(upload.clone(), &token).await.unwrap();
    assert!(matches!(result, VaultResponse::Command(c) if c.status == CommandStatus::Applied));
    assert_eq!(std::fs::read(&original).unwrap(), bytes);
    let before = server.messages().unwrap().len();
    let result = service.request(upload, &token).await.unwrap();
    assert!(matches!(result, VaultResponse::Command(c) if c.status == CommandStatus::Applied));
    assert_eq!(
        server.messages().unwrap().len(),
        before,
        "double Import cannot upload twice"
    );
    let state = service.reader().state().await.unwrap();
    assert_eq!(
        state.tracks[&id].sources[&id].sha256,
        Some(vault_snapshot::digest(&bytes))
    );
    let order = state.tracks[&id].updated_order;
    let lyrics =
        vault_media::parse_lyrics("[00:01.00]Новая строка\n[00:03.00]次の行", "lrc").unwrap();
    let op = Uuid::new_v4();
    let request = VaultRequest::EditMetadata {
        op_id: op,
        track_id: id,
        edit: Box::new(MediaEdit {
            patch: MetadataPatch {
                title: Some("Обновлённая композиция".into()),
                expected_order: Some(order),
                ..Default::default()
            },
            artwork: None,
            lyrics: Some(lyrics.clone()),
        }),
    };
    assert!(
        matches!(service.request(request.clone(), &token).await.unwrap(), VaultResponse::Command(c) if c.status == CommandStatus::Applied)
    );
    let after = service.reader().state().await.unwrap();
    assert_eq!(
        after.tracks[&id].sources, state.tracks[&id].sources,
        "logical edits leave audio unchanged"
    );
    let reference = after.tracks[&id].metadata.lyrics.as_ref().unwrap();
    let transport = server.client().unwrap();
    let file = transport
        .message(binding.chat_id, reference.source.message_id, &token)
        .await
        .unwrap()
        .unwrap()
        .file
        .unwrap();
    let document = transport.download(file, 0, 0, &token).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<LyricsDocument>(&std::fs::read(document.path).unwrap()).unwrap(),
        lyrics
    );
    let before = server.messages().unwrap().len();
    assert!(
        matches!(service.request(request, &token).await.unwrap(), VaultResponse::Command(c) if c.status == CommandStatus::Applied)
    );
    assert_eq!(
        server.messages().unwrap().len(),
        before,
        "retry reuses the attachment and event"
    );
    let stale = VaultRequest::EditMetadata {
        op_id: Uuid::new_v4(),
        track_id: id,
        edit: Box::new(MediaEdit {
            patch: MetadataPatch {
                title: Some("Stale editor".into()),
                expected_order: Some(order),
                ..Default::default()
            },
            artwork: None,
            lyrics: None,
        }),
    };
    assert_eq!(
        service.request(stale, &token).await.unwrap_err().code,
        ErrorCode::EditNotSent
    );
    assert_eq!(server.messages().unwrap().len(), before);
    service
        .request(
            VaultRequest::AssignGcCoordinator {
                op_id: Uuid::new_v4(),
                target: CoordinatorTarget::ThisDevice,
                expected_previous: None,
                force_confirmed: false,
            },
            &token,
        )
        .await
        .unwrap();
    service
        .request(VaultRequest::Snapshot, &token)
        .await
        .unwrap();
    let restored = self::service(&server, binding, EventBus::default());
    ready(&restored).await;
    assert_eq!(
        restored.reader().state().await.unwrap().tracks[&id],
        after.tracks[&id]
    );
    restored.shutdown().await.unwrap();
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn explicit_tag_rewrite_keeps_identity_old_source_and_playlist_after_lost_ack() {
    let (server, binding) = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let cache =
        vault_media::CacheManager::open(dir.path().join("cache"), 32 * 1024 * 1024).unwrap();
    let service = service_with_cache(&server, binding, EventBus::default(), cache);
    ready(&service).await;
    let token = CancellationToken::new();
    let original = dir.path().join("original.flac");
    let bytes = include_bytes!("../../../spec/test-vectors/audio/tone.flac");
    std::fs::write(&original, bytes).unwrap();
    let track_id = Uuid::new_v4();
    let source_id = Uuid::new_v4();
    service
        .request(
            VaultRequest::Import {
                path: original.to_string_lossy().into_owned(),
                track_id,
                source_id,
                op_id: Uuid::new_v4(),
                allow_duplicate: false,
            },
            &token,
        )
        .await
        .unwrap();
    let playlist_id = Uuid::new_v4();
    let entry_id = Uuid::new_v4();
    for (entity_id, action) in [
        (
            playlist_id,
            Action::PlaylistCreate {
                title: "Keep".into(),
            },
        ),
        (
            playlist_id,
            Action::PlaylistTrackAdd {
                track_id,
                entry_id,
                after_entry_id: None,
            },
        ),
        (
            track_id,
            Action::FavoriteSet {
                set_name: "liked".into(),
                value: true,
            },
        ),
    ] {
        service
            .request(
                VaultRequest::Mutate(Box::new(Mutation {
                    op_id: Uuid::new_v4(),
                    entity_id,
                    action,
                })),
                &token,
            )
            .await
            .unwrap();
    }
    let before = service.reader().state().await.unwrap();
    let op_id = Uuid::new_v4();
    let rewrite = VaultRequest::RewriteTrack {
        op_id,
        track_id,
        edit: Box::new(MediaEdit {
            patch: MetadataPatch {
                title: Some("New file tags".into()),
                expected_order: Some(before.tracks[&track_id].updated_order),
                ..Default::default()
            },
            artwork: None,
            lyrics: None,
        }),
    };
    server
        .set_faults(Faults {
            lost_ack: true,
            ..Default::default()
        })
        .unwrap();
    assert!(service.request(rewrite.clone(), &token).await.is_err());
    assert_eq!(
        service.reader().state().await.unwrap().tracks[&track_id],
        before.tracks[&track_id]
    );
    server.set_faults(Faults::default()).unwrap();
    service.request(VaultRequest::Sync, &token).await.unwrap();
    let count = server.messages().unwrap().len();
    let result = service.request(rewrite.clone(), &token).await.unwrap();
    assert!(matches!(result, VaultResponse::Command(c) if c.status == CommandStatus::Applied));
    assert_eq!(
        server.messages().unwrap().len(),
        count + 1,
        "only the metadata/source event remains"
    );
    let after = service.reader().state().await.unwrap();
    let track = &after.tracks[&track_id];
    assert_eq!(track.sources.len(), 2);
    assert_eq!(
        track.sources[&source_id],
        before.tracks[&track_id].sources[&source_id]
    );
    assert_ne!(track.preferred_source_id, Some(source_id));
    assert_eq!(after.entries[&entry_id], before.entries[&entry_id]);
    assert_eq!(after.favorites, before.favorites);
    assert_eq!(std::fs::read(original).unwrap(), bytes);
    let count = server.messages().unwrap().len();
    assert!(
        matches!(service.request(rewrite, &token).await.unwrap(), VaultResponse::Command(c) if c.status == CommandStatus::Applied)
    );
    assert_eq!(server.messages().unwrap().len(), count);
    assert_eq!(server.delete_calls().unwrap(), 0);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn retagging_legacy_embedded_cover_preserves_artwork_and_publishes_reference() {
    let (server, binding) = fixture().await;
    let dir = tempfile::tempdir().unwrap();
    let cache =
        vault_media::CacheManager::open(dir.path().join("cache"), 32 * 1024 * 1024).unwrap();
    let service = service_with_cache(&server, binding, EventBus::default(), cache);
    ready(&service).await;
    let token = CancellationToken::new();
    let original = dir.path().join("legacy.flac");
    std::fs::write(
        &original,
        include_bytes!("../../../spec/test-vectors/audio/tone.flac"),
    )
    .unwrap();
    // Generated 2x2 RGB PNG, with no external asset or account dependency.
    let png = hex::decode("89504e470d0a1a0a0000000d4948445200000002000000020802000000fdd49a730000001249444154789c63f8afe2f15fc583014201002a9e05ada6985abb0000000049454e44ae426082").unwrap();
    let art = vault_media::artwork(&png, None).unwrap();
    vault_media::write_tags(
        &original,
        &Metadata {
            title: "Legacy".into(),
            ..Default::default()
        },
        Some(&art),
        None,
    )
    .unwrap();
    let original_bytes = std::fs::read(&original).unwrap();
    let track_id = Uuid::new_v4();
    service
        .request(
            VaultRequest::Import {
                path: original.to_string_lossy().into_owned(),
                track_id,
                source_id: Uuid::new_v4(),
                op_id: Uuid::new_v4(),
                allow_duplicate: false,
            },
            &token,
        )
        .await
        .unwrap();
    service
        .request(
            VaultRequest::Mutate(Box::new(Mutation {
                op_id: Uuid::new_v4(),
                entity_id: track_id,
                action: Action::TrackMetadataUpdate {
                    patch: MetadataPatch {
                        artwork: Some(Artwork {
                            sha256: art.sha256.clone(),
                            mime_type: art.mime_type.clone(),
                            source: None,
                        }),
                        ..Default::default()
                    },
                },
            })),
            &token,
        )
        .await
        .unwrap();
    let before = service.reader().state().await.unwrap();
    let result = service
        .request(
            VaultRequest::RewriteTrack {
                op_id: Uuid::new_v4(),
                track_id,
                edit: Box::new(MediaEdit {
                    patch: MetadataPatch {
                        title: Some("Retagged".into()),
                        expected_order: Some(before.tracks[&track_id].updated_order),
                        ..Default::default()
                    },
                    artwork: None,
                    lyrics: None,
                }),
            },
            &token,
        )
        .await
        .unwrap();
    assert!(matches!(result, VaultResponse::Command(c) if c.status == CommandStatus::Applied));
    let after = service.reader().state().await.unwrap();
    let cover = after.tracks[&track_id].metadata.artwork.as_ref().unwrap();
    assert_eq!(cover.sha256, art.sha256);
    assert_eq!(cover.source.as_ref().unwrap().chat_id, binding.chat_id);
    assert_eq!(after.tracks[&track_id].sources.len(), 2);
    assert_eq!(std::fs::read(original).unwrap(), original_bytes);
    service.shutdown().await.unwrap();
}
