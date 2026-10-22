#![allow(clippy::unwrap_used)]
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_db::{Binding, CommandStatus, Database};
use vault_domain::*;
use vault_protocol::{Record, VaultInit, encode_text};
use vault_sync::*;
use vault_telegram::*;
use vault_testkit::*;

fn binding() -> Binding {
    Binding {
        vault_id: Uuid::from_u128(1),
        chat_id: ChatId::new(-100).unwrap(),
        owner_id: UserId::new(42).unwrap(),
    }
}
fn engine(server: &FakeServer, device: &str) -> SyncEngine<FakeTelegram> {
    SyncEngine::new(
        server.client().unwrap(),
        Database::memory(binding()).unwrap(),
        device.into(),
    )
}
async fn setup() -> (FakeServer, SyncEngine<FakeTelegram>) {
    let b = binding();
    let server = FakeServer::new(b.chat_id, b.owner_id);
    let transport = server.client().unwrap();
    transport
        .send_text(
            b.chat_id,
            encode_text(&Record::Init(VaultInit {
                vault_id: b.vault_id,
                owner_telegram_user_id: b.owner_id,
                created_by: "kiyora".into(),
                created_at_utc: "2026-10-21T00:00:00Z".into(),
            }))
            .unwrap(),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let mut first = engine(&server, "desktop");
    first.initialize(&CancellationToken::new()).await.unwrap();
    (server, first)
}
async fn execute(
    engine: &mut SyncEngine<FakeTelegram>,
    entity: Uuid,
    action: Action,
) -> vault_db::Command {
    engine
        .execute(Uuid::new_v4(), entity, action, &CancellationToken::new())
        .await
        .unwrap()
}

#[tokio::test]
async fn three_clients_converge_with_short_reverse_pages_delayed_duplicate_updates_and_delete_wins()
{
    let (server, mut a) = setup().await;
    let cancel = CancellationToken::new();
    let mut b = engine(&server, "phone");
    let mut c = engine(&server, "laptop");
    b.sync(&cancel).await.unwrap();
    c.sync(&cancel).await.unwrap();
    server
        .set_faults(Faults {
            history_page_size: Some(1),
            delay_updates: true,
            duplicate_updates: true,
            ..Default::default()
        })
        .unwrap();
    let playlist = Uuid::new_v4();
    execute(
        &mut a,
        playlist,
        Action::PlaylistCreate {
            title: "Ancient playlist".into(),
        },
    )
    .await;
    let (x, y) = tokio::join!(
        execute(
            &mut a,
            playlist,
            Action::PlaylistRename {
                title: "Linux".into()
            }
        ),
        execute(
            &mut b,
            playlist,
            Action::PlaylistRename {
                title: "Android".into()
            }
        )
    );
    assert_eq!(x.status, CommandStatus::Applied);
    assert_eq!(y.status, CommandStatus::Applied);
    execute(&mut c, playlist, Action::PlaylistDelete {}).await;
    let rejected = execute(
        &mut a,
        playlist,
        Action::PlaylistCreate {
            title: "Resurrect".into(),
        },
    )
    .await;
    assert_eq!(rejected.status, CommandStatus::RejectedByReducer);
    server.flush_updates_reversed().unwrap();
    for client in [&mut a, &mut b, &mut c] {
        client.sync(&cancel).await.unwrap();
    }
    assert_eq!(a.state().await.unwrap(), b.state().await.unwrap());
    assert_eq!(a.state().await.unwrap(), c.state().await.unwrap());
    assert!(
        a.state()
            .await
            .unwrap()
            .tombstones
            .playlists
            .contains(&playlist)
    );
}

#[tokio::test]
async fn long_history_catch_up_batches_a_thousand_events_and_reaches_ready_state() {
    let (server, _) = setup().await;
    let cancel = CancellationToken::new();
    server
        .set_faults(Faults {
            history_page_size: Some(100),
            ..Default::default()
        })
        .unwrap();
    let transport = server.client().unwrap();
    for index in 0..1_000u128 {
        let event = Event {
            vault_id: binding().vault_id,
            op_id: Uuid::from_u128(10_000 + index),
            device_id: "load-test".into(),
            base_generation: 0,
            entity_id: Uuid::from_u128(20_000 + index),
            action: Action::PlaylistCreate {
                title: format!("Playlist {index}"),
            },
        };
        transport
            .send_text(
                binding().chat_id,
                encode_text(&Record::Event(Box::new(event))).unwrap(),
                &cancel,
            )
            .await
            .unwrap();
    }
    let mut recovering = engine(&server, "recovery-load-test");
    recovering.sync(&cancel).await.unwrap();
    assert_eq!(recovering.mode(), RuntimeMode::OnlineReady);
    let state = recovering.state().await.unwrap();
    assert_eq!(state.playlists.len(), 1_000);
    assert_eq!(state.applied.len(), 1_000);
}

#[tokio::test]
async fn lost_ack_is_reconciled_without_duplicate_and_offline_never_queues_mutations() {
    let (server, mut a) = setup().await;
    let cancel = CancellationToken::new();
    let op = Uuid::new_v4();
    let playlist = Uuid::new_v4();
    let action = Action::PlaylistCreate {
        title: "Single".into(),
    };
    server
        .set_faults(Faults {
            lost_ack: true,
            drop_updates: true,
            ..Default::default()
        })
        .unwrap();
    let unknown = a
        .execute(op, playlist, action.clone(), &cancel)
        .await
        .unwrap();
    assert_eq!(unknown.status, CommandStatus::UnknownOnTimeout);
    server.set_faults(Faults::default()).unwrap();
    a.sync(&cancel).await.unwrap();
    assert_eq!(
        a.command(op).await.unwrap().unwrap().status,
        CommandStatus::Applied
    );
    let count = server.messages().unwrap().len();
    assert_eq!(
        a.execute(op, playlist, action, &cancel)
            .await
            .unwrap()
            .status,
        CommandStatus::Applied
    );
    assert_eq!(server.messages().unwrap().len(), count);
    server
        .set_faults(Faults {
            offline: true,
            ..Default::default()
        })
        .unwrap();
    let offline_op = Uuid::new_v4();
    assert!(
        a.execute(offline_op, playlist, Action::PlaylistDelete {}, &cancel)
            .await
            .is_err()
    );
    assert!(a.command(offline_op).await.unwrap().is_none());
    assert!(a.state().await.unwrap().playlists.contains_key(&playlist));
    assert_ne!(a.mode(), RuntimeMode::OnlineReady);
}

#[tokio::test]
async fn snapshots_keep_old_playlists_tombstones_and_recover_from_b_after_compaction() {
    let (server, mut a) = setup().await;
    let cancel = CancellationToken::new();
    let active = Uuid::new_v4();
    let deleted = Uuid::new_v4();
    execute(
        &mut a,
        active,
        Action::PlaylistCreate {
            title: "2023 playlist".into(),
        },
    )
    .await;
    execute(
        &mut a,
        deleted,
        Action::PlaylistCreate {
            title: "Removed".into(),
        },
    )
    .await;
    execute(&mut a, deleted, Action::PlaylistDelete {}).await;
    execute(
        &mut a,
        binding().vault_id,
        Action::VaultSettingsUpdate {
            settings: SharedSettings {
                gc_device_id: Some("desktop".into()),
                display_name: "kiyora".into(),
            },
        },
    )
    .await;
    let snapshot = a.request_snapshot(&cancel).await.unwrap();
    assert!(snapshot.snapshot.state.playlists.contains_key(&active));
    assert!(
        snapshot
            .snapshot
            .state
            .tombstones
            .playlists
            .contains(&deleted)
    );
    // Simulate already completed conservative event pruning.
    for message in server.messages().unwrap() {
        if message.id < snapshot.commit.barrier_message_id
            && matches!(message.record().unwrap(), Some(Record::Event(_)))
        {
            server.manual_delete(message.id).unwrap();
        }
    }
    let primary = server
        .messages()
        .unwrap()
        .into_iter()
        .find(|m| m.id == snapshot.commit.snapshot_a_message_id)
        .unwrap();
    server.corrupt_file(primary.file.unwrap().file_id).unwrap();
    let mut new = engine(&server, "fresh");
    new.sync(&cancel).await.unwrap();
    assert_eq!(a.state().await.unwrap(), new.state().await.unwrap());
    assert!(!new.verified_snapshot().unwrap().primary_healthy);
    assert!(new.verified_snapshot().unwrap().backup_healthy);
    assert_eq!(
        execute(
            &mut new,
            deleted,
            Action::PlaylistCreate {
                title: "Zombie".into()
            }
        )
        .await
        .status,
        CommandStatus::RejectedByReducer
    );
    let backup = server
        .messages()
        .unwrap()
        .into_iter()
        .find(|m| m.id == snapshot.commit.snapshot_b_message_id)
        .unwrap();
    server.corrupt_file(backup.file.unwrap().file_id).unwrap();
    let before = new.state().await.unwrap();
    assert!(new.sync(&cancel).await.is_err());
    assert_eq!(new.mode(), RuntimeMode::DegradedReadonly);
    assert_eq!(new.state().await.unwrap(), before);
}

#[tokio::test]
async fn unknown_protocol_blocks_without_advancing_cursor() {
    let (server, mut a) = setup().await;
    let cancel = CancellationToken::new();
    let before = a.status().await.unwrap().cursor;
    server
        .client()
        .unwrap()
        .send_text(binding().chat_id, "TMV/2 EVT\n{}".into(), &cancel)
        .await
        .unwrap();
    assert!(a.sync(&cancel).await.is_err());
    assert_eq!(a.mode(), RuntimeMode::DegradedReadonly);
    assert_eq!(a.status().await.unwrap().cursor, before);
    assert!(a.request_snapshot(&cancel).await.is_err());
}

#[tokio::test]
async fn manual_deletion_of_known_event_keeps_projection_and_blocks_mutations() {
    let (server, mut a) = setup().await;
    let cancel = CancellationToken::new();
    let playlist = Uuid::new_v4();
    let command = execute(
        &mut a,
        playlist,
        Action::PlaylistCreate {
            title: "Keep locally".into(),
        },
    )
    .await;
    let state = a.state().await.unwrap();
    server
        .manual_delete(command.server_message_id.unwrap())
        .unwrap();
    assert!(a.sync(&cancel).await.is_err());
    assert_eq!(a.mode(), RuntimeMode::DegradedReadonly);
    assert_eq!(a.state().await.unwrap(), state);
}

#[tokio::test]
async fn failed_initial_snapshot_can_resume_same_channel_without_duplicate_init() {
    let b = binding();
    let server = FakeServer::new(b.chat_id, b.owner_id);
    let transport = server.client().unwrap();
    let cancel = CancellationToken::new();
    transport
        .send_text(
            b.chat_id,
            encode_text(&Record::Init(VaultInit {
                vault_id: b.vault_id,
                owner_telegram_user_id: b.owner_id,
                created_by: "kiyora".into(),
                created_at_utc: "2026-10-21T00:00:00Z".into(),
            }))
            .unwrap(),
            &cancel,
        )
        .await
        .unwrap();
    server
        .set_faults(Faults {
            fail_call: Some(7),
            ..Default::default()
        })
        .unwrap();
    let mut first = engine(&server, "desktop");
    assert!(first.initialize(&cancel).await.is_err());
    server.set_faults(Faults::default()).unwrap();
    let mut second = engine(&server, "desktop");
    second.initialize(&cancel).await.unwrap();
    assert_eq!(
        server
            .messages()
            .unwrap()
            .iter()
            .filter(|m| matches!(m.record().unwrap(), Some(Record::Init(_))))
            .count(),
        1
    );
    assert_eq!(second.mode(), RuntimeMode::OnlineReady);
}

#[tokio::test]
async fn gc_preserves_media_notes_new_events_and_resumes_after_delete_ack_loss() {
    let (server, mut a) = setup().await;
    let cancel = CancellationToken::new();
    let chat = binding().chat_id;
    let transport = server.client().unwrap();
    let note = transport
        .send_text(chat, "User note must remain".into(), &cancel)
        .await
        .unwrap();
    let media = transport
        .send_text(
            chat,
            encode_text(&Record::Media(vault_protocol::MediaMarker {
                vault_id: binding().vault_id,
                import_id: Uuid::new_v4(),
                sha256: None,
            }))
            .unwrap(),
            &cancel,
        )
        .await
        .unwrap();
    let playlist = Uuid::new_v4();
    execute(
        &mut a,
        playlist,
        Action::PlaylistCreate {
            title: "Durable".into(),
        },
    )
    .await;
    for n in 0..40 {
        execute(
            &mut a,
            playlist,
            Action::PlaylistRename {
                title: format!("Version {n}"),
            },
        )
        .await;
    }
    execute(
        &mut a,
        binding().vault_id,
        Action::VaultSettingsUpdate {
            settings: SharedSettings {
                gc_device_id: Some("desktop".into()),
                display_name: "kiyora".into(),
            },
        },
    )
    .await;
    a.request_snapshot(&cancel).await.unwrap();
    let plan = a.plan_gc(&cancel).await.unwrap();
    let hash = plan.plan.hash().unwrap();
    assert!(plan.plan.candidates.len() > 32);
    assert!(
        plan.plan
            .candidates
            .iter()
            .all(|c| c.message_id != note.id && c.message_id != media.id)
    );
    assert!(
        a.execute_gc(plan.plan.plan_id, "wrong confirmation", &cancel)
            .await
            .is_err()
    );
    let fresh = execute(
        &mut a,
        playlist,
        Action::PlaylistRename {
            title: "After barrier".into(),
        },
    )
    .await;
    server
        .set_faults(Faults {
            fail_after_delete: true,
            ..Default::default()
        })
        .unwrap();
    assert!(
        a.execute_gc(plan.plan.plan_id, &hash, &cancel)
            .await
            .is_err()
    );
    let mut cold = engine(&server, "fresh");
    cold.sync(&cancel).await.unwrap();
    assert_eq!(
        cold.state().await.unwrap().playlists[&playlist].title,
        "After barrier"
    );
    server.set_faults(Faults::default()).unwrap();
    a.sync(&cancel).await.unwrap();
    let complete = a
        .execute_gc(plan.plan.plan_id, &hash, &cancel)
        .await
        .unwrap();
    assert_eq!(complete.status, vault_snapshot::gc::GcStatus::Complete);
    assert_eq!(complete.completed.len(), complete.plan.candidates.len());
    for preserved in [note.id, media.id, fresh.server_message_id.unwrap()] {
        assert!(
            transport
                .message(chat, preserved, &cancel)
                .await
                .unwrap()
                .is_some()
        );
    }
    cold.sync(&cancel).await.unwrap();
    assert_eq!(cold.state().await.unwrap(), a.state().await.unwrap());
}

#[tokio::test]
async fn gc_stops_if_role_changes_or_either_snapshot_disappears() {
    let (server, mut a) = setup().await;
    let cancel = CancellationToken::new();
    execute(
        &mut a,
        binding().vault_id,
        Action::VaultSettingsUpdate {
            settings: SharedSettings {
                gc_device_id: Some("desktop".into()),
                display_name: "kiyora".into(),
            },
        },
    )
    .await;
    let snap = a.request_snapshot(&cancel).await.unwrap();
    let plan = a.plan_gc(&cancel).await.unwrap();
    server
        .manual_delete(snap.commit.snapshot_a_message_id)
        .unwrap();
    let count = server.messages().unwrap().len();
    assert!(
        a.execute_gc(plan.plan.plan_id, &plan.plan.hash().unwrap(), &cancel)
            .await
            .is_err()
    );
    assert_eq!(server.messages().unwrap().len(), count);
    a.sync(&cancel).await.unwrap();
    execute(
        &mut a,
        binding().vault_id,
        Action::VaultSettingsUpdate {
            settings: SharedSettings {
                gc_device_id: Some("phone".into()),
                display_name: "kiyora".into(),
            },
        },
    )
    .await;
    assert!(
        a.execute_gc(plan.plan.plan_id, &plan.plan.hash().unwrap(), &cancel)
            .await
            .is_err()
    );
    assert!(a.plan_gc(&cancel).await.is_err());
}
