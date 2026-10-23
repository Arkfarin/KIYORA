//! Crash-safe local replacement. Only the disposable SQLite projection is rebuilt;
//! the original database and its sidecars remain together in a private quarantine.
use crate::{ApiError, ErrorCode};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_db::{Binding, Database};
use vault_sync::SyncEngine;
use vault_telegram::Telegram;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Install {
    version: u32,
    id: Uuid,
    original: [bool; 3],
}
const SUFFIXES: [&str; 3] = ["", "-wal", "-shm"];
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}
fn directory(path: &Path) -> Result<&Path, ApiError> {
    path.parent()
        .ok_or_else(|| ApiError::new(ErrorCode::Storage))
}
fn sync_dir(path: &Path) -> Result<(), ApiError> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
fn install_directory(path: &Path, id: Uuid) -> PathBuf {
    sibling(path, &format!(".recovery-{id}"))
}

/// Called under the profile process lock before opening SQLite. Replaying this
/// journal never truncates or opens the old database, including a damaged WAL.
pub(crate) fn resume_install(path: &Path) -> Result<(), ApiError> {
    let marker = sibling(path, ".recovery.json");
    let bytes = match fs::read(&marker) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let install: Install =
        serde_json::from_slice(&bytes).map_err(|_| ApiError::new(ErrorCode::Integrity))?;
    if install.version != 1 || install.id.is_nil() {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    let work = install_directory(path, install.id);
    let prepared = work.join("prepared.sqlite");
    if prepared.exists() {
        // The absence of prepared.sqlite is the durable install-complete marker.
        // Until then each old file is moved at most once, with no replacement.
        for (suffix, existed) in SUFFIXES.into_iter().zip(install.original) {
            let old = sibling(path, suffix);
            let saved = work.join(format!("original.sqlite{suffix}"));
            if saved.exists() {
                if old.exists() {
                    return Err(ApiError::new(ErrorCode::Conflict));
                }
            } else if existed {
                fs::rename(&old, &saved)?;
                sync_dir(&work)?;
                sync_dir(directory(path)?)?;
            } else if old.exists() {
                return Err(ApiError::new(ErrorCode::Conflict));
            }
        }
        fs::rename(&prepared, path)?;
        sync_dir(directory(path)?)?;
        sync_dir(&work)?;
    } else if !path.exists() {
        return Err(ApiError::new(ErrorCode::Storage));
    }
    fs::remove_file(marker)?;
    sync_dir(directory(path)?)?;
    Ok(())
}

/// No remote mutations. Failure/cancellation before publication leaves the old
/// bytes untouched. Publication is completed despite cancellation, or resumed on
/// next profile open; callers must close every old writer before entering here.
#[cfg(test)]
pub(crate) async fn rebuild<T: Telegram>(
    transport: T,
    path: PathBuf,
    binding: Binding,
    device: String,
    cancel: &CancellationToken,
) -> Result<crate::SnapshotInfo, ApiError> {
    rebuild_from_backup(transport, path, binding, device, None, cancel).await
}

pub(crate) async fn rebuild_from_backup<T: Telegram>(
    transport: T,
    path: PathBuf,
    binding: Binding,
    device: String,
    archive: Option<PathBuf>,
    cancel: &CancellationToken,
) -> Result<crate::SnapshotInfo, ApiError> {
    let parent = directory(&path)?.to_owned();
    let temporary = tempfile::Builder::new()
        .prefix(".kiyora-rebuild-")
        .tempdir_in(&parent)?;
    let prepared = temporary.path().join("prepared.sqlite");
    let open_path = prepared.clone();
    let db = tokio::task::spawn_blocking(move || Database::open(open_path, binding))
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))??;
    let mut engine = SyncEngine::new(transport, db, device);
    if let Some(archive) = archive {
        let backup = tokio::task::spawn_blocking(move || {
            vault_snapshot::backup::inspect(&archive, vault_snapshot::backup::Limits::default())
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))?
        .map_err(vault_sync::SyncError::from)?;
        engine
            .seed_from_backup(backup.state.clone(), backup.manifest.owner_id, cancel)
            .await?;
    }
    engine.sync(cancel).await?;
    let verified = engine.verify(cancel).await?;
    let result = crate::SnapshotInfo::from(verified);
    drop(engine);
    let check_path = prepared.clone();
    tokio::task::spawn_blocking(move || -> Result<(), ApiError> {
        let db = Database::open(&check_path, binding)?;
        db.validate()?;
        db.checkpoint()?;
        drop(db);
        fs::File::open(&check_path)?.sync_all()?;
        Ok(())
    })
    .await
    .map_err(|_| ApiError::new(ErrorCode::Internal))??;
    if cancel.is_cancelled() {
        return Err(ApiError::new(ErrorCode::Cancelled));
    }
    // The blocking transaction must finish even if its async caller is dropped.
    tokio::task::spawn_blocking(move || -> Result<(), ApiError> {
        let install = Install {
            version: 1,
            id: Uuid::new_v4(),
            original: SUFFIXES.map(|suffix| sibling(&path, suffix).exists()),
        };
        let work = install_directory(&path, install.id);
        fs::rename(temporary.path(), &work)?;
        sync_dir(&parent)?;
        let mut marker = tempfile::NamedTempFile::new_in(&parent)?;
        marker.write_all(
            &serde_json::to_vec(&install).map_err(|_| ApiError::new(ErrorCode::Internal))?,
        )?;
        marker.as_file().sync_all()?;
        marker
            .persist_noclobber(sibling(&path, ".recovery.json"))
            .map_err(|_| ApiError::new(ErrorCode::Conflict))?;
        sync_dir(&parent)?;
        resume_install(&path)
    })
    .await
    .map_err(|_| ApiError::new(ErrorCode::Internal))??;
    Ok(result)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use vault_domain::{Action, ChatId, UserId};
    use vault_protocol::{Record, VaultInit, encode_text};
    use vault_testkit::FakeServer;

    fn binding() -> Binding {
        Binding {
            vault_id: Uuid::new_v4(),
            owner_id: UserId::new(42).unwrap(),
            chat_id: ChatId::new(-100).unwrap(),
        }
    }

    #[test]
    fn interrupted_install_resumes_at_every_rename_and_preserves_original_sidecars() {
        for completed in 0..=4 {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("vault.sqlite");
            let binding = binding();
            let install = Install {
                version: 1,
                id: Uuid::new_v4(),
                original: [true; 3],
            };
            let work = install_directory(&path, install.id);
            fs::create_dir(&work).unwrap();
            let db = Database::open(work.join("prepared.sqlite"), binding).unwrap();
            db.validate().unwrap();
            db.checkpoint().unwrap();
            drop(db);
            for suffix in SUFFIXES {
                fs::write(sibling(&path, suffix), format!("damaged{suffix}")).unwrap();
            }
            fs::write(
                sibling(&path, ".recovery.json"),
                serde_json::to_vec(&install).unwrap(),
            )
            .unwrap();
            for suffix in SUFFIXES.iter().take(completed.min(3)) {
                fs::rename(
                    sibling(&path, suffix),
                    work.join(format!("original.sqlite{suffix}")),
                )
                .unwrap();
            }
            if completed == 4 {
                fs::rename(work.join("prepared.sqlite"), &path).unwrap();
            }
            resume_install(&path).unwrap();
            resume_install(&path).unwrap();
            Database::open(&path, binding).unwrap().validate().unwrap();
            for suffix in SUFFIXES {
                assert_eq!(
                    fs::read(work.join(format!("original.sqlite{suffix}"))).unwrap(),
                    format!("damaged{suffix}").as_bytes()
                );
            }
            assert!(!sibling(&path, ".recovery.json").exists());
        }
    }

    #[tokio::test]
    async fn rebuild_restores_snapshot_and_tail_without_remote_writes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("vault.sqlite");
        fs::write(&path, b"corrupt sqlite").unwrap();
        let binding = binding();
        let server = FakeServer::new(binding.chat_id, binding.owner_id);
        let transport = server.client().unwrap();
        let cancel = CancellationToken::new();
        transport
            .send_text(
                binding.chat_id,
                encode_text(&Record::Init(VaultInit {
                    vault_id: binding.vault_id,
                    owner_telegram_user_id: binding.owner_id,
                    created_by: "test".into(),
                    created_at_utc: "2026-10-23T00:00:00Z".into(),
                }))
                .unwrap(),
                &cancel,
            )
            .await
            .unwrap();
        let mut writer = SyncEngine::new(
            transport.clone(),
            Database::memory(binding).unwrap(),
            "writer".into(),
        );
        writer.initialize(&cancel).await.unwrap();
        writer
            .assign_gc_coordinator(Uuid::new_v4(), Some("writer".into()), None, false, &cancel)
            .await
            .unwrap();
        let playlist = Uuid::new_v4();
        writer
            .execute(
                Uuid::new_v4(),
                playlist,
                Action::PlaylistCreate {
                    title: "Before snapshot".into(),
                },
                &cancel,
            )
            .await
            .unwrap();
        writer.request_snapshot(&cancel).await.unwrap();
        writer
            .execute(
                Uuid::new_v4(),
                playlist,
                Action::PlaylistRename {
                    title: "After snapshot".into(),
                },
                &cancel,
            )
            .await
            .unwrap();
        let expected = writer.state().await.unwrap();
        let head = transport
            .history(binding.chat_id, None, 1, &cancel)
            .await
            .unwrap();
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(
            rebuild(
                transport.clone(),
                path.clone(),
                binding,
                "reader".into(),
                &cancelled
            )
            .await
            .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"corrupt sqlite");
        rebuild(
            transport.clone(),
            path.clone(),
            binding,
            "reader".into(),
            &cancel,
        )
        .await
        .unwrap();
        let db = Database::open(&path, binding).unwrap();
        db.validate().unwrap();
        assert_eq!(db.state().unwrap(), expected);
        assert_eq!(
            transport
                .history(binding.chat_id, None, 1, &cancel)
                .await
                .unwrap(),
            head
        );
        let work = fs::read_dir(temp.path())
            .unwrap()
            .filter_map(Result::ok)
            .find(|entry| entry.file_name().to_string_lossy().contains(".recovery-"))
            .unwrap()
            .path();
        assert_eq!(
            fs::read(work.join("original.sqlite")).unwrap(),
            b"corrupt sqlite"
        );
    }
}
