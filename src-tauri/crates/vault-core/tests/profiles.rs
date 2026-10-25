#![allow(clippy::unwrap_used)]
use tempfile::tempdir;
use uuid::Uuid;
use vault_core::*;
use vault_security::ProfileStore;
use vault_sync::RuntimeMode;

#[tokio::test]
async fn injected_secret_failure_prevents_native_open_and_plaintext_persistence() {
    struct Unavailable;
    impl vault_security::SecretStore for Unavailable {
        fn get(
            &self,
            _: &str,
        ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, vault_security::SecurityError> {
            Err(vault_security::SecurityError::SecureStorageUnavailable)
        }
        fn set(&self, _: &str, _: &[u8]) -> Result<(), vault_security::SecurityError> {
            Err(vault_security::SecurityError::SecureStorageUnavailable)
        }
    }
    let temp = tempdir().unwrap();
    let backend = Backend::with_secrets(
        ProfileStore::at(temp.path()).unwrap(),
        "must-not-be-opened.so",
        std::sync::Arc::new(Unavailable),
    );
    let profile = backend.create_profile("Android".into()).unwrap();
    let status = backend.switch_profile(profile.profile_id).await.unwrap();
    let error = backend
        .connect_native(status.context.session_id, 123, "a".repeat(32).into())
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::SecureStorageUnavailable);
    let profile_dir = temp
        .path()
        .join("profiles")
        .join(profile.profile_id.to_string());
    assert_eq!(
        std::fs::read_dir(profile_dir.join("tdlib"))
            .unwrap()
            .count(),
        0
    );
    let config = std::fs::read_to_string(profile_dir.join("backend.json")).unwrap();
    assert!(!config.contains(&"a".repeat(32)));
    backend.shutdown().await.unwrap();
    let mut config: serde_json::Value = serde_json::from_str(&config).unwrap();
    config["api_id"] = 123.into();
    std::fs::write(
        profile_dir.join("backend.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let restored = backend.switch_profile(profile.profile_id).await.unwrap();
    assert_eq!(restored.mode, RuntimeMode::AuthRequired);
    assert!(matches!(
        backend
            .service_request(
                restored.context.session_id,
                VaultRequest::GetCacheStats,
                &tokio_util::sync::CancellationToken::new()
            )
            .await
            .unwrap(),
        VaultResponse::CacheStats(_)
    ));
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn switching_profile_changes_session_and_fences_stale_calls() {
    let temp = tempdir().unwrap();
    let backend = Backend::new(ProfileStore::at(temp.path()).unwrap(), "missing-tdjson.so");
    let first = backend.create_profile("Personal".into()).unwrap();
    let second = backend.create_profile("Work".into()).unwrap();
    let initial = backend.switch_profile(first.profile_id).await.unwrap();
    assert_eq!(initial.context.profile_id, first.profile_id);
    assert_eq!(initial.mode, RuntimeMode::AuthRequired);
    let next = backend.switch_profile(second.profile_id).await.unwrap();
    assert_ne!(initial.context.session_id, next.context.session_id);
    assert_eq!(
        backend
            .auth_status(initial.context.session_id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    assert!(backend.local_reader(next.context.session_id).await.is_err());
    let again = backend.switch_profile(first.profile_id).await.unwrap();
    assert_eq!(again.context.profile_id, first.profile_id);
    assert_ne!(again.context.session_id, initial.context.session_id);
    backend.shutdown().await.unwrap();
}

#[test]
fn typed_ipc_errors_and_profile_context_do_not_expose_native_messages() {
    let raw = vault_telegram::TelegramError::Request { code: 400 };
    let safe = ApiError::from(raw);
    assert_eq!(safe.code, ErrorCode::TelegramRejected);
    assert_eq!(safe.message, "Telegram rejected the request");
    let context = Context {
        profile_id: Uuid::new_v4(),
        session_id: Uuid::new_v4(),
        vault_id: None,
    };
    assert_eq!(
        serde_json::from_str::<Context>(&serde_json::to_string(&context).unwrap()).unwrap(),
        context
    );
}

#[tokio::test]
async fn profile_switches_serialize_through_secret_restoration() {
    use std::sync::{Arc, Mutex, mpsc};
    struct HeldSecrets {
        entered: tokio::sync::Notify,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl vault_security::SecretStore for HeldSecrets {
        fn get(
            &self,
            _: &str,
        ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, vault_security::SecurityError> {
            self.entered.notify_one();
            self.release.lock().unwrap().recv().unwrap();
            Ok(None)
        }
        fn set(&self, _: &str, _: &[u8]) -> Result<(), vault_security::SecurityError> {
            unreachable!("this test restores a profile with no saved API hash")
        }
    }
    let temp = tempdir().unwrap();
    let (release, receive) = mpsc::channel();
    let secrets = Arc::new(HeldSecrets {
        entered: tokio::sync::Notify::new(),
        release: Mutex::new(receive),
    });
    let backend = Backend::with_secrets(
        ProfileStore::at(temp.path()).unwrap(),
        "missing-tdjson.so",
        secrets.clone(),
    );
    let first = backend.create_profile("First".into()).unwrap();
    let second = backend.create_profile("Second".into()).unwrap();
    backend.switch_profile(first.profile_id).await.unwrap();
    backend.shutdown().await.unwrap();
    let path = temp
        .path()
        .join("profiles")
        .join(first.profile_id.to_string())
        .join("backend.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    config["api_id"] = 123.into();
    std::fs::write(path, serde_json::to_vec(&config).unwrap()).unwrap();

    let first_backend = backend.clone();
    let first_switch =
        tokio::spawn(async move { first_backend.switch_profile(first.profile_id).await });
    secrets.entered.notified().await;
    let second_backend = backend.clone();
    let mut second_switch =
        tokio::spawn(async move { second_backend.switch_profile(second.profile_id).await });
    // The first transition is held at a known boundary. The second must wait;
    // release before asserting so a regression cannot strand the blocking worker.
    let premature =
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut second_switch).await;
    release.send(()).unwrap();
    let first_result = first_switch.await.unwrap().unwrap();
    let waited = premature.is_err();
    let second_result = match premature {
        Ok(result) => result.unwrap().unwrap(),
        Err(_) => second_switch.await.unwrap().unwrap(),
    };
    backend.shutdown().await.unwrap();
    assert!(
        waited,
        "a second switch interleaved with secret restoration"
    );
    assert_eq!(first_result.context.profile_id, first.profile_id);
    assert_eq!(second_result.context.profile_id, second.profile_id);
    assert_eq!(
        backend.active_profile_id().unwrap(),
        Some(second.profile_id)
    );

    // Dropping the IPC caller must not release the transition while its worker
    // still owns the profile. Shutdown must wait, then close the published session.
    let cancelled_backend = backend.clone();
    let cancelled =
        tokio::spawn(async move { cancelled_backend.switch_profile(first.profile_id).await });
    secrets.entered.notified().await;
    cancelled.abort();
    assert!(cancelled.await.unwrap_err().is_cancelled());
    let shutdown_backend = backend.clone();
    let mut shutdown = tokio::spawn(async move { shutdown_backend.shutdown().await });
    let premature =
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut shutdown).await;
    release.send(()).unwrap();
    let waited = premature.is_err();
    match premature {
        Ok(result) => result.unwrap().unwrap(),
        Err(_) => shutdown.await.unwrap().unwrap(),
    }
    assert!(waited, "shutdown raced an accepted profile transition");
    assert!(backend.context().await.is_none());
    ProfileStore::at(temp.path())
        .unwrap()
        .acquire(first.profile_id)
        .unwrap();
}

#[tokio::test]
async fn local_cache_and_device_queries_work_without_telegram_and_quota_persists() {
    let temp = tempdir().unwrap();
    let backend = Backend::new(ProfileStore::at(temp.path()).unwrap(), "missing-tdjson.so");
    let profile = backend.create_profile("Offline".into()).unwrap();
    let status = backend.switch_profile(profile.profile_id).await.unwrap();
    let session = status.context.session_id;
    let cancel = tokio_util::sync::CancellationToken::new();
    let device = match backend.query(session, LocalQuery::Device).await.unwrap() {
        LocalResponse::Device(device) => device,
        response => panic!("unexpected {response:?}"),
    };
    assert!(!device.device_id.is_empty());
    assert!(matches!(
        backend
            .service_request(session, VaultRequest::GetCacheStats, &cancel)
            .await
            .unwrap(),
        VaultResponse::CacheStats(_)
    ));
    assert!(
        matches!(backend.service_request(session, VaultRequest::SetCacheQuota { bytes: 1024 }, &cancel).await.unwrap(), VaultResponse::CacheStats(s) if s.quota_bytes == 1024)
    );
    assert!(matches!(
        backend
            .service_request(session, VaultRequest::ClearLocalCache, &cancel)
            .await
            .unwrap(),
        VaultResponse::CacheCleared { bytes_removed: 0 }
    ));
    assert_eq!(
        backend
            .query(session, LocalQuery::Settings)
            .await
            .unwrap_err()
            .code,
        ErrorCode::NoVault
    );
    assert_eq!(
        backend
            .query(Uuid::new_v4(), LocalQuery::Device)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    cancel.cancel();
    assert_eq!(
        backend
            .service_request(
                session,
                VaultRequest::SetCacheQuota { bytes: 2048 },
                &cancel
            )
            .await
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    backend.shutdown().await.unwrap();
    let status = backend.switch_profile(profile.profile_id).await.unwrap();
    assert!(
        matches!(backend.query(status.context.session_id, LocalQuery::Device).await.unwrap(), LocalResponse::Device(d) if d.device_id == device.device_id && d.cache_quota_bytes == 1024)
    );
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn corrupt_sqlite_keeps_profile_open_for_explicit_repair() {
    let temp = tempdir().unwrap();
    let backend = Backend::new(ProfileStore::at(temp.path()).unwrap(), "missing-tdjson.so");
    let profile = backend.create_profile("Recovery".into()).unwrap();
    backend.switch_profile(profile.profile_id).await.unwrap();
    backend.shutdown().await.unwrap();
    let directory = temp
        .path()
        .join("profiles")
        .join(profile.profile_id.to_string());
    let config_path = directory.join("backend.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    config["binding"] =
        serde_json::json!({"vault_id":Uuid::new_v4(), "owner_id":"42", "chat_id":"-100"});
    std::fs::write(config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    std::fs::write(directory.join("vault.sqlite"), b"broken projection").unwrap();
    let status = backend.switch_profile(profile.profile_id).await.unwrap();
    assert_eq!(status.mode, RuntimeMode::DegradedReadonly);
    let error = backend
        .query(status.context.session_id, LocalQuery::Settings)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::DatabaseCorrupt);
    let error = backend
        .repair_local_database(
            status.context.session_id,
            &tokio_util::sync::CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::AuthRequired);
    assert_eq!(
        std::fs::read(directory.join("vault.sqlite")).unwrap(),
        b"broken projection"
    );
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn background_jobs_persist_results_and_fence_reused_identifiers() {
    let temp = tempdir().unwrap();
    let backend = Backend::new(ProfileStore::at(temp.path()).unwrap(), "missing-tdjson.so");
    let profile = backend.create_profile("Jobs".into()).unwrap();
    let status = backend.switch_profile(profile.profile_id).await.unwrap();
    let session = status.context.session_id;
    let id = Uuid::new_v4();
    let request = JobRequest::Vault(VaultRequest::GetCacheStats);
    backend
        .start_job(session, id, request.clone(), false)
        .await
        .unwrap();
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let info = backend.job_status(session, id).await.unwrap();
            if info.state != JobState::Running {
                break info;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(result.state, JobState::Succeeded);
    assert!(matches!(
        result.result.as_deref(),
        Some(JobResult::Vault(VaultResponse::CacheStats(_)))
    ));
    assert_eq!(
        backend
            .start_job(session, id, request.clone(), true)
            .await
            .unwrap()
            .state,
        JobState::Succeeded
    );
    assert_eq!(
        backend
            .start_job(session, id, JobRequest::RepairDatabase, false)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Conflict
    );
    backend.shutdown().await.unwrap();
    let next = backend
        .switch_profile(profile.profile_id)
        .await
        .unwrap()
        .context
        .session_id;
    assert_eq!(
        backend.job_status(session, id).await.unwrap_err().code,
        ErrorCode::Conflict
    );
    assert_eq!(
        backend.job_status(next, id).await.unwrap().state,
        JobState::Succeeded
    );
    backend.forget_job(next, id).await.unwrap();
    assert!(backend.list_jobs(next).await.unwrap().is_empty());
    backend.shutdown().await.unwrap();
}

#[tokio::test]
async fn switching_profiles_drains_background_jobs_without_lock_inversion() {
    let temp = tempdir().unwrap();
    let backend = Backend::new(ProfileStore::at(temp.path()).unwrap(), "missing-tdjson.so");
    let first = backend.create_profile("First".into()).unwrap();
    let second = backend.create_profile("Second".into()).unwrap();
    let session = backend
        .switch_profile(first.profile_id)
        .await
        .unwrap()
        .context
        .session_id;
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let start = backend.start_job(
            session,
            Uuid::new_v4(),
            JobRequest::Vault(VaultRequest::GetCacheStats),
            false,
        );
        let switch = backend.switch_profile(second.profile_id);
        let (_, switched) = tokio::join!(start, switch);
        switched.unwrap();
        backend.shutdown().await.unwrap();
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn playback_queue_keeps_captured_order_and_skips_remotely_deleted_tracks() {
    use vault_domain::*;
    let temp = tempdir().unwrap();
    let backend = Backend::new(ProfileStore::at(temp.path()).unwrap(), "missing-tdjson.so");
    let profile = backend.create_profile("Queue".into()).unwrap();
    backend.switch_profile(profile.profile_id).await.unwrap();
    backend.shutdown().await.unwrap();
    let directory = temp
        .path()
        .join("profiles")
        .join(profile.profile_id.to_string());
    let binding = vault_db::Binding {
        vault_id: Uuid::new_v4(),
        owner_id: UserId::new(42).unwrap(),
        chat_id: ChatId::new(-100).unwrap(),
    };
    let config_path = directory.join("backend.json");
    let mut config: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&config_path).unwrap()).unwrap();
    config["binding"] = serde_json::to_value(binding).unwrap();
    std::fs::write(config_path, serde_json::to_vec(&config).unwrap()).unwrap();
    let mut db = vault_db::Database::open(directory.join("vault.sqlite"), binding).unwrap();
    let a = Uuid::new_v4();
    let b = Uuid::new_v4();
    let playlist = Uuid::new_v4();
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let track = || Action::TrackRegister {
        metadata: Metadata {
            title: "Queue track".into(),
            ..Default::default()
        },
        source: Source {
            source_id: Uuid::new_v4(),
            chat_id: binding.chat_id,
            message_id: MessageId::new(1000).unwrap(),
            sha256: None,
            size_bytes: None,
        },
    };
    let actions = [
        (a, track()),
        (b, track()),
        (
            playlist,
            Action::PlaylistCreate {
                title: "Captured".into(),
            },
        ),
        (
            playlist,
            Action::PlaylistTrackAdd {
                track_id: a,
                entry_id: first,
                after_entry_id: None,
            },
        ),
        (
            playlist,
            Action::PlaylistTrackAdd {
                track_id: b,
                entry_id: second,
                after_entry_id: Some(first),
            },
        ),
    ];
    for (index, (entity_id, action)) in actions.into_iter().enumerate() {
        db.apply_record(
            MessageId::new(index as i64 + 1).unwrap(),
            &vault_protocol::Record::Event(Box::new(Event {
                vault_id: binding.vault_id,
                op_id: Uuid::new_v4(),
                device_id: "remote".into(),
                base_generation: 0,
                entity_id,
                action,
            })),
        )
        .unwrap();
    }
    let session = backend
        .switch_profile(profile.profile_id)
        .await
        .unwrap()
        .context
        .session_id;
    let queue = backend
        .queue_playlist(session, playlist, 0, false)
        .await
        .unwrap();
    assert_eq!(queue.current_track_id, Some(a));
    db.apply_record(
        MessageId::new(6).unwrap(),
        &vault_protocol::Record::Event(Box::new(Event {
            vault_id: binding.vault_id,
            op_id: Uuid::new_v4(),
            device_id: "remote".into(),
            base_generation: 0,
            entity_id: b,
            action: Action::TrackDelete {},
        })),
    )
    .unwrap();
    let captured = backend.playback_queue(session, 0, 500).await.unwrap();
    assert_eq!(captured.track_ids, vec![a, b]);
    assert_eq!(captured.info.queue_id, queue.queue_id);
    assert!(matches!(
        backend
            .control_playback(session, PlaybackAction::Next)
            .await
            .unwrap(),
        vault_media::PlaybackState::Idle
    ));
    assert_eq!(
        backend
            .playback_queue(session, 0, 500)
            .await
            .unwrap()
            .info
            .position,
        None
    );
    backend.shutdown().await.unwrap();
}
