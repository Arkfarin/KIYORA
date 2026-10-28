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
    let before = server.history_calls().unwrap();
    recovering.require_online(&cancel).await.unwrap();
    assert!(
        server.history_calls().unwrap() - before <= 4,
        "a verified idle client must not rescan the 1,000-event journal"
    );
    assert_eq!(recovering.state().await.unwrap(), state);
}

#[tokio::test]
async fn online_guard_discovers_missed_new_events_and_reaudits_deleted_history() {
    let (server, mut client) = setup().await;
    let cancel = CancellationToken::new();
    server
        .set_faults(Faults {
            drop_updates: true,
            ..Default::default()
        })
        .unwrap();
    let playlist = Uuid::new_v4();
    let event = Event {
        vault_id: binding().vault_id,
        op_id: Uuid::new_v4(),
        device_id: "other".into(),
        base_generation: 0,
        entity_id: playlist,
        action: Action::PlaylistCreate {
            title: "Missed update".into(),
        },
    };
    let message = server
        .client()
        .unwrap()
        .send_text(
            binding().chat_id,
            encode_text(&Record::Event(Box::new(event))).unwrap(),
            &cancel,
        )
        .await
        .unwrap();
    client.require_online(&cancel).await.unwrap();
    assert_eq!(
        client.state().await.unwrap().playlists[&playlist].title,
        "Missed update"
    );
    let before = client.state().await.unwrap();
    // Even without a live delete update, loss of the independently confirmed
    // server anchor must trigger the full receipt audit and block writes.
    server.manual_delete(message.id).unwrap();
    assert!(client.require_online(&cancel).await.is_err());
    assert_eq!(client.mode(), RuntimeMode::DegradedReadonly);
    assert_eq!(client.state().await.unwrap(), before);
}

#[tokio::test]
async fn deletion_during_full_reconciliation_cannot_authorize_a_mutation() {
    let (server, mut client) = setup().await;
    let cancel = CancellationToken::new();
    let playlist = Uuid::new_v4();
    let created = execute(
        &mut client,
        playlist,
        Action::PlaylistCreate {
            title: "Keep local evidence".into(),
        },
    )
    .await;
    // Keep the head intact: checking only the newest message cannot reveal loss
    // of an older event between receiving the history page and accepting it.
    execute(
        &mut client,
        playlist,
        Action::PlaylistRename {
            title: "Latest title".into(),
        },
    )
    .await;
    let before = client.state().await.unwrap();
    server
        .set_faults(Faults {
            remove_after_history: created.server_message_id,
            ..Default::default()
        })
        .unwrap();

    assert!(client.sync(&cancel).await.is_err());
    assert_ne!(client.mode(), RuntimeMode::OnlineReady);
    assert_eq!(client.state().await.unwrap(), before);
    // A fresh audit now sees the missing receipt and must retain local data.
    assert!(client.sync(&cancel).await.is_err());
    assert_eq!(client.mode(), RuntimeMode::DegradedReadonly);
    let count = server.messages().unwrap().len();
    assert!(
        client
            .execute(Uuid::new_v4(), playlist, Action::PlaylistDelete {}, &cancel)
            .await
            .is_err()
    );
    assert_eq!(server.messages().unwrap().len(), count);
    assert_eq!(client.state().await.unwrap(), before);
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
async fn server_error_after_send_keeps_unknown_intent_until_reconciled() {
    for accepted in [false, true] {
        let (server, mut engine) = setup().await;
        let op = Uuid::new_v4();
        let playlist = Uuid::new_v4();
        let action = Action::PlaylistCreate {
            title: "Ambiguous server response".into(),
        };
        let cancel = CancellationToken::new();
        server
            .set_faults(Faults {
                send_error_before: (!accepted).then_some(500),
                send_error_after: accepted.then_some(500),
                drop_updates: true,
                ..Default::default()
            })
            .unwrap();
        let result = engine
            .execute(op, playlist, action.clone(), &cancel)
            .await
            .unwrap();
        assert_eq!(result.status, CommandStatus::UnknownOnTimeout);
        let count = server.messages().unwrap().len();
        engine.sync(&cancel).await.unwrap();
        let result = engine.execute(op, playlist, action, &cancel).await.unwrap();
        assert_eq!(
            result.status,
            if accepted {
                CommandStatus::Applied
            } else {
                CommandStatus::UnknownOnTimeout
            }
        );
        assert_eq!(
            engine
                .state()
                .await
                .unwrap()
                .playlists
                .contains_key(&playlist),
            accepted
        );
        assert_eq!(server.messages().unwrap().len(), count);
    }
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
    for n in 0..140 {
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
    assert!(plan.plan.candidates.len() > 100);
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
    a.abandon_gc(plan.plan.plan_id).await.unwrap();
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

#[tokio::test]
async fn coordinator_transfer_requires_handoff_or_confirmed_takeover_and_discards_old_plans() {
    let (server, mut desktop) = setup().await;
    let cancel = CancellationToken::new();
    let op = Uuid::new_v4();
    let assigned = desktop
        .assign_gc_coordinator(op, Some("desktop".into()), None, false, &cancel)
        .await
        .unwrap();
    assert_eq!(assigned.status, CommandStatus::Applied);
    assert_eq!(
        desktop
            .assign_gc_coordinator(op, Some("desktop".into()), None, false, &cancel)
            .await
            .unwrap()
            .intent
            .op_id,
        op
    );
    desktop.request_snapshot(&cancel).await.unwrap();
    let plan = desktop.plan_gc(&cancel).await.unwrap();
    let mut phone = engine(&server, "phone");
    phone.sync(&cancel).await.unwrap();
    let before = server.messages().unwrap();
    assert!(
        desktop
            .assign_gc_coordinator(
                Uuid::new_v4(),
                Some("phone".into()),
                Some("desktop".into()),
                false,
                &cancel
            )
            .await
            .is_err()
    );
    assert!(
        phone
            .assign_gc_coordinator(
                Uuid::new_v4(),
                Some("phone".into()),
                Some("desktop".into()),
                false,
                &cancel
            )
            .await
            .is_err()
    );
    assert!(
        phone
            .execute(
                Uuid::new_v4(),
                binding().vault_id,
                Action::VaultSettingsUpdate {
                    settings: SharedSettings {
                        gc_device_id: Some("phone".into()),
                        display_name: "".into()
                    }
                },
                &cancel
            )
            .await
            .is_err(),
        "generic settings must not bypass takeover confirmation"
    );
    assert_eq!(server.messages().unwrap(), before);
    desktop.abandon_gc(plan.plan.plan_id).await.unwrap();
    desktop
        .assign_gc_coordinator(
            Uuid::new_v4(),
            Some("phone".into()),
            Some("desktop".into()),
            false,
            &cancel,
        )
        .await
        .unwrap();
    assert!(
        desktop
            .execute_gc(plan.plan.plan_id, &plan.plan.hash().unwrap(), &cancel)
            .await
            .is_err()
    );
    phone.sync(&cancel).await.unwrap();
    phone.request_snapshot(&cancel).await.unwrap();
    let old_phone_plan = phone.plan_gc(&cancel).await.unwrap();
    desktop
        .assign_gc_coordinator(
            Uuid::new_v4(),
            Some("desktop".into()),
            Some("phone".into()),
            true,
            &cancel,
        )
        .await
        .unwrap();
    phone
        .assign_gc_coordinator(
            Uuid::new_v4(),
            Some("phone".into()),
            Some("desktop".into()),
            true,
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(
        phone
            .inspect_gc(old_phone_plan.plan.plan_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        vault_snapshot::gc::GcStatus::Abandoned
    );
    let before = server.messages().unwrap();
    assert!(
        phone
            .execute_gc(
                old_phone_plan.plan.plan_id,
                &old_phone_plan.plan.hash().unwrap(),
                &cancel
            )
            .await
            .is_err()
    );
    assert_eq!(server.messages().unwrap(), before);
    assert!(
        phone
            .assign_gc_coordinator(Uuid::new_v4(), None, Some("desktop".into()), true, &cancel)
            .await
            .is_err(),
        "stale role expectation must not change the new head"
    );
}

#[tokio::test]
async fn both_missing_copies_reconstruct_only_when_committed_hash_proves_full_history() {
    let (server, mut writer) = setup().await;
    let cancel = CancellationToken::new();
    writer
        .assign_gc_coordinator(Uuid::new_v4(), Some("desktop".into()), None, false, &cancel)
        .await
        .unwrap();
    let playlist = Uuid::new_v4();
    let created = execute(
        &mut writer,
        playlist,
        Action::PlaylistCreate {
            title: "Retained history".into(),
        },
    )
    .await;
    let snapshot = writer.request_snapshot(&cancel).await.unwrap();
    execute(
        &mut writer,
        playlist,
        Action::PlaylistRename {
            title: "After barrier".into(),
        },
    )
    .await;
    for id in [
        snapshot.commit.snapshot_a_message_id,
        snapshot.commit.snapshot_b_message_id,
    ] {
        server.manual_delete(id).unwrap();
    }
    let mut fresh = engine(&server, "recovered");
    fresh.sync(&cancel).await.unwrap();
    assert_eq!(fresh.state().await.unwrap(), writer.state().await.unwrap());
    let verified = fresh.verify(&cancel).await.unwrap();
    assert!(!verified.primary_healthy && !verified.backup_healthy);
    assert_eq!(verified.commit_id, snapshot.commit_id);
    // An apparently replayable older generation is insufficient when even one
    // covered event was deleted. Hash proof must fail, preserving the projection.
    server
        .manual_delete(created.server_message_id.unwrap())
        .unwrap();
    let before = fresh.state().await.unwrap();
    assert!(fresh.sync(&cancel).await.is_err());
    assert_eq!(fresh.mode(), RuntimeMode::DegradedReadonly);
    assert_eq!(fresh.state().await.unwrap(), before);
}

#[tokio::test]
async fn backup_seed_recovers_pruned_missing_snapshots_and_rejects_wrong_state() {
    let (server, mut writer) = setup().await;
    let cancel = CancellationToken::new();
    writer
        .assign_gc_coordinator(Uuid::new_v4(), Some("desktop".into()), None, false, &cancel)
        .await
        .unwrap();
    let playlist = Uuid::new_v4();
    let created = execute(
        &mut writer,
        playlist,
        Action::PlaylistCreate {
            title: "Preserved".into(),
        },
    )
    .await;
    let snapshot = writer.request_snapshot(&cancel).await.unwrap();
    let archive_state = writer.state().await.unwrap();
    execute(
        &mut writer,
        playlist,
        Action::PlaylistRename {
            title: "Tail survives".into(),
        },
    )
    .await;
    for id in [
        snapshot.commit.snapshot_a_message_id,
        snapshot.commit.snapshot_b_message_id,
        created.server_message_id.unwrap(),
    ] {
        server.manual_delete(id).unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("restored.sqlite");
    let mut fresh = SyncEngine::new(
        server.client().unwrap(),
        Database::open(&path, binding()).unwrap(),
        "recovered".into(),
    );
    assert!(fresh.sync(&cancel).await.is_err());
    assert!(
        fresh
            .seed_from_backup(archive_state.clone(), UserId::new(7).unwrap(), &cancel)
            .await
            .is_err()
    );
    let mut wrong = archive_state.clone();
    wrong.playlists.get_mut(&playlist).unwrap().title = "Forged".into();
    assert!(
        fresh
            .seed_from_backup(wrong, binding().owner_id, &cancel)
            .await
            .is_err()
    );
    let mut wrong = archive_state.clone();
    wrong.generation -= 1;
    assert!(
        fresh
            .seed_from_backup(wrong, binding().owner_id, &cancel)
            .await
            .is_err()
    );
    fresh
        .seed_from_backup(archive_state, binding().owner_id, &cancel)
        .await
        .unwrap();
    fresh.sync(&cancel).await.unwrap();
    assert_eq!(fresh.state().await.unwrap(), writer.state().await.unwrap());
    drop(fresh);
    let mut reopened = SyncEngine::new(
        server.client().unwrap(),
        Database::open(&path, binding()).unwrap(),
        "desktop".into(),
    );
    reopened.sync(&cancel).await.unwrap();
    assert_eq!(
        reopened.state().await.unwrap().playlists[&playlist].title,
        "Tail survives"
    );
    // Publish healthy A/B after verified recovery; other clients need no archive.
    reopened.request_snapshot(&cancel).await.unwrap();
    let mut cold = engine(&server, "cold");
    cold.sync(&cancel).await.unwrap();
    assert_eq!(cold.state().await.unwrap(), reopened.state().await.unwrap());
}

#[tokio::test]
async fn retry_after_local_database_loss_never_resends_an_applied_operation() {
    let (server, mut writer) = setup().await;
    let cancel = CancellationToken::new();
    writer
        .assign_gc_coordinator(Uuid::new_v4(), Some("desktop".into()), None, false, &cancel)
        .await
        .unwrap();
    let id = Uuid::new_v4();
    let op = Uuid::new_v4();
    let action = Action::PlaylistCreate {
        title: "Exactly once".into(),
    };
    let original = writer
        .execute(op, id, action.clone(), &cancel)
        .await
        .unwrap();
    writer.request_snapshot(&cancel).await.unwrap();
    let head = server.messages().unwrap().len();
    let mut fresh = engine(&server, "fresh");
    fresh.sync(&cancel).await.unwrap();
    let replay = fresh
        .execute(op, id, action.clone(), &cancel)
        .await
        .unwrap();
    assert_eq!(replay, original);
    assert_eq!(server.messages().unwrap().len(), head);
    assert!(
        fresh
            .execute(
                op,
                id,
                Action::PlaylistCreate {
                    title: "Different intent".into()
                },
                &cancel
            )
            .await
            .is_err()
    );
    server
        .manual_delete(original.server_message_id.unwrap())
        .unwrap();
    let head = server.messages().unwrap().len();
    let mut pruned = engine(&server, "pruned");
    pruned.sync(&cancel).await.unwrap();
    assert!(pruned.execute(op, id, action, &cancel).await.is_err());
    assert_eq!(server.messages().unwrap().len(), head);
    assert_eq!(pruned.mode(), RuntimeMode::OnlineReady);
}

#[tokio::test]
async fn gc_stops_between_batches_when_coordinator_changes_or_a_copy_is_lost() {
    for lose_copy in [false, true] {
        let (server, mut writer) = setup().await;
        let cancel = CancellationToken::new();
        writer
            .assign_gc_coordinator(Uuid::new_v4(), Some("desktop".into()), None, false, &cancel)
            .await
            .unwrap();
        let transport = server.client().unwrap();
        for i in 0..220 {
            let event = Event {
                vault_id: binding().vault_id,
                op_id: Uuid::new_v4(),
                device_id: "other".into(),
                base_generation: 0,
                entity_id: Uuid::new_v4(),
                action: Action::PlaylistCreate {
                    title: format!("{i}"),
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
        let snapshot = writer.request_snapshot(&cancel).await.unwrap();
        let plan = writer.plan_gc(&cancel).await.unwrap();
        let transfer = Event {
            vault_id: binding().vault_id,
            op_id: Uuid::new_v4(),
            device_id: "phone".into(),
            base_generation: 1,
            entity_id: binding().vault_id,
            action: Action::VaultSettingsUpdate {
                settings: SharedSettings {
                    gc_device_id: Some("phone".into()),
                    display_name: "".into(),
                },
            },
        };
        server
            .set_faults(Faults {
                text_after_delete: (!lose_copy)
                    .then(|| encode_text(&Record::Event(Box::new(transfer))).unwrap()),
                remove_after_delete: lose_copy.then_some(snapshot.commit.snapshot_a_message_id),
                ..Default::default()
            })
            .unwrap();
        let calls = server.delete_calls().unwrap();
        assert!(
            writer
                .execute_gc(plan.plan.plan_id, &plan.plan.hash().unwrap(), &cancel)
                .await
                .is_err()
        );
        assert_eq!(
            server.delete_calls().unwrap() - calls,
            1,
            "must stop before the second destructive batch"
        );
        let progress = writer.inspect_gc(plan.plan.plan_id).await.unwrap().unwrap();
        assert_eq!(progress.status, vault_snapshot::gc::GcStatus::Stopped);
        assert_eq!(progress.completed.len(), 100);
        let mut cold = engine(&server, "cold");
        cold.sync(&cancel).await.unwrap();
        assert_eq!(cold.state().await.unwrap().playlists.len(), 220);
    }
}

#[tokio::test]
async fn gc_retains_audio_with_control_captions_and_all_live_source_references() {
    let (server, mut writer) = setup().await;
    let cancel = CancellationToken::new();
    writer
        .assign_gc_coordinator(Uuid::new_v4(), Some("desktop".into()), None, false, &cancel)
        .await
        .unwrap();
    let event = execute(
        &mut writer,
        Uuid::new_v4(),
        Action::PlaylistCreate {
            title: "Retain".into(),
        },
    )
    .await;
    let snapshot = writer.request_snapshot(&cancel).await.unwrap();
    let state = writer.state().await.unwrap();
    let mut messages = server.messages().unwrap();
    let normal = vault_snapshot::gc::plan(&messages, &snapshot, &state, "desktop").unwrap();
    let candidate = normal
        .candidates
        .iter()
        .find(|c| Some(c.message_id) == event.server_message_id)
        .unwrap();
    let message = messages
        .iter_mut()
        .find(|m| m.id == candidate.message_id)
        .unwrap();
    message.content = MessageContent::Audio;
    message.file = Some(RemoteFile {
        file_id: 123,
        size: 42,
        name: "song.mp3".into(),
        mime_type: "audio/mpeg".into(),
    });
    assert!(!vault_snapshot::gc::matches(candidate, message).unwrap());
    let plan = vault_snapshot::gc::plan(&messages, &snapshot, &state, "desktop").unwrap();
    assert!(
        plan.candidates
            .iter()
            .all(|c| c.message_id != candidate.message_id)
    );
    let mut linked = state;
    let track_id = Uuid::new_v4();
    let source = Source {
        source_id: Uuid::new_v4(),
        chat_id: binding().chat_id,
        message_id: candidate.message_id,
        sha256: None,
        size_bytes: None,
    };
    linked.tracks.insert(
        track_id,
        Track {
            preferred_source_id: None,
            track_id,
            metadata: Metadata {
                title: "Reference".into(),
                ..Default::default()
            },
            sources: [(source.source_id, source)].into(),
            updated_order: candidate.message_id,
        },
    );
    let plan = vault_snapshot::gc::plan(&server.messages().unwrap(), &snapshot, &linked, "desktop")
        .unwrap();
    assert!(
        plan.candidates
            .iter()
            .all(|c| c.message_id != candidate.message_id)
    );
}

#[tokio::test]
async fn update_overrun_and_reconnect_revoke_the_incremental_proof() {
    let (server, mut client) = setup().await;
    let cancel = CancellationToken::new();
    let before = server.download_calls().unwrap();
    for _ in 0..2100 {
        server.emit_update(Update::Connected).unwrap();
    }
    client.require_online(&cancel).await.unwrap();
    assert!(
        server.download_calls().unwrap() > before,
        "lost continuity must reverify the snapshot"
    );
    server.emit_update(Update::Disconnected).unwrap();
    let op = Uuid::new_v4();
    assert!(
        client
            .execute(op, Uuid::new_v4(), Action::PlaylistDelete {}, &cancel)
            .await
            .is_err()
    );
    assert!(client.command(op).await.unwrap().is_none());
    let before = server.download_calls().unwrap();
    server.emit_update(Update::Connected).unwrap();
    client.catch_up(&cancel).await.unwrap();
    assert!(server.download_calls().unwrap() > before);
    assert_eq!(client.mode(), RuntimeMode::OnlineReady);
}
