use super::{ImportInfo, Journal};
use crate::{ApiError, ErrorCode};
use std::{
    fs,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Pending imports are not an evictable cache. One store belongs to the profile's
/// single writer; the profile lease prevents competing processes. In-memory test
/// projections get a temporary store with the same lifetime as their service.
pub(crate) struct Storage {
    root: PathBuf,
    _temporary: Option<tempfile::TempDir>,
}

// Bound abandoned/crashed jobs too; completed imports release their copy.
const MAX_STAGED_BYTES: u64 = vault_media::MAX_IMPORT_BYTES;

impl Storage {
    pub(super) fn open(database: Option<PathBuf>, vault: Uuid) -> Result<Self, ApiError> {
        let (base, temporary) = match database {
            Some(database) => {
                let mut name = database.file_name().ok_or_else(integrity)?.to_os_string();
                name.push(".imports");
                (database.with_file_name(name), None)
            }
            None => {
                let temporary = tempfile::tempdir()?;
                (temporary.path().join("imports"), Some(temporary))
            }
        };
        private_dir(&base)?;
        let root = base.join(vault.to_string());
        private_dir(&root)?;
        let store = Self {
            root: root.canonicalize()?,
            _temporary: temporary,
        };
        // Only incomplete copies live in .copy. Uploads always use the final
        // published path, so removing these cannot invalidate an unknown send.
        for entry in fs::read_dir(&store.root)? {
            let entry = entry?;
            if canonical_uuid(&entry.file_name().to_string_lossy()).is_some() {
                let job = entry.path();
                checked_dir(&job)?;
                let scratch = job.join(".copy");
                match fs::symlink_metadata(&scratch) {
                    Ok(metadata) => {
                        if metadata.file_type().is_symlink() || !metadata.is_dir() {
                            return Err(integrity());
                        }
                        for file in fs::read_dir(&scratch)? {
                            let file = file?;
                            let metadata = fs::symlink_metadata(file.path())?;
                            if !metadata.is_file() || metadata.file_type().is_symlink() {
                                return Err(integrity());
                            }
                            fs::remove_file(file.path())?;
                        }
                        fs::remove_dir(&scratch)?;
                        sync_dir(&job)?;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        Ok(store)
    }

    pub(super) fn root(&self) -> PathBuf {
        self.root.clone()
    }

    pub(super) fn prepare(
        root: &Path,
        journal: &Journal,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, ApiError> {
        let name = journal.staged_name.as_deref().ok_or_else(integrity)?;
        let path = owned_path(root, journal.info.op_id, name)?;
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                verify(&path, &journal.info, cancel)?;
                return Ok(path);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let asset = vault_media::inspect_with_cancel(&journal.path, cancel)?;
        if asset.sha256 != journal.info.sha256 || asset.size_bytes != journal.info.size_bytes {
            return Err(integrity());
        }
        let used = directory_bytes(root, 2)?;
        if used
            .checked_add(asset.size_bytes)
            .is_none_or(|total| total > MAX_STAGED_BYTES)
        {
            return Err(ApiError::new(ErrorCode::Storage));
        }
        let job = path.parent().ok_or_else(integrity)?;
        private_dir(job)?;
        let scratch = job.join(".copy");
        private_dir(&scratch)?;
        let copy = vault_media::stage_with_cancel(&asset, &scratch, cancel)?;
        if cancel.is_cancelled() {
            return Err(ApiError::new(ErrorCode::Cancelled));
        }
        // Never overwrite another operation's bytes, including a publication
        // recovered from a previous process. Data reaches disk before the journal
        // may advance to Prepared.
        copy.persist_noclobber(&path)
            .map_err(|error| ApiError::from(error.error))?
            .sync_all()?;
        sync_dir(job)?;
        fs::remove_dir(scratch)?;
        sync_dir(job)?;
        Ok(path)
    }

    pub(super) fn verified(
        root: &Path,
        journal: &Journal,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, ApiError> {
        let path = owned_path(
            root,
            journal.info.op_id,
            journal.staged_name.as_deref().ok_or_else(integrity)?,
        )?;
        verify(&path, &journal.info, cancel)?;
        Ok(path)
    }

    pub(super) fn remove(root: &Path, journal: &Journal) -> Result<(), ApiError> {
        let Some(name) = journal.staged_name.as_deref() else {
            return Ok(());
        };
        let path = owned_path(root, journal.info.op_id, name)?;
        let cover = path.parent().ok_or_else(integrity)?.join(".artwork");
        match fs::symlink_metadata(&cover) {
            Ok(meta) if meta.is_file() && !meta.file_type().is_symlink() => fs::remove_file(cover)?,
            Ok(_) => return Err(integrity()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                fs::remove_file(&path)?
            }
            Ok(_) => return Err(integrity()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        }
        let job = path.parent().ok_or_else(integrity)?;
        sync_dir(job)?;
        match fs::remove_dir(job) {
            Ok(()) => sync_dir(root)?,
            Err(error) if error.kind() == std::io::ErrorKind::DirectoryNotEmpty => {}
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }
}

fn integrity() -> ApiError {
    ApiError::new(ErrorCode::Integrity)
}
fn canonical_uuid(value: &str) -> Option<Uuid> {
    value
        .parse::<Uuid>()
        .ok()
        .filter(|id| !id.is_nil() && id.to_string() == value)
}
fn checked_dir(path: &Path) -> Result<(), ApiError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(integrity());
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<(), ApiError> {
    match fs::create_dir(path) {
        Ok(()) => {
            if let Some(parent) = path.parent() {
                sync_dir(parent)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error.into()),
    }
    checked_dir(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
fn owned_path(root: &Path, op: Uuid, name: &str) -> Result<PathBuf, ApiError> {
    let mut components = Path::new(name).components();
    if op.is_nil()
        || name.is_empty()
        || name.len() > 240
        || name.contains(['\0', '/', '\\'])
        || matches!(name, "." | ".." | ".copy")
        || name.trim_end_matches(['.', ' ']) != name
        || !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return Err(integrity());
    }
    checked_dir(root)?;
    let job = root.join(op.to_string());
    match fs::symlink_metadata(&job) {
        Ok(_) => checked_dir(&job)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(job.join(name))
}
fn verify(path: &Path, info: &ImportInfo, cancel: &CancellationToken) -> Result<(), ApiError> {
    let asset = vault_media::inspect_with_cancel(path, cancel).map_err(|error| {
        if matches!(
            error,
            vault_media::MediaError::Telegram(vault_telegram::TelegramError::Cancelled)
        ) {
            ApiError::new(ErrorCode::Cancelled)
        } else {
            integrity()
        }
    })?;
    if asset.sha256 != info.sha256 || asset.size_bytes != info.size_bytes {
        return Err(integrity());
    }
    Ok(())
}
fn directory_bytes(path: &Path, depth: u8) -> Result<u64, ApiError> {
    let mut total = 0u64;
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let metadata = fs::symlink_metadata(entry.path())?;
        if metadata.file_type().is_symlink() {
            return Err(integrity());
        }
        let bytes = if metadata.is_file() {
            metadata.len()
        } else if metadata.is_dir() && depth > 0 {
            directory_bytes(&entry.path(), depth - 1)?
        } else {
            return Err(integrity());
        };
        total = total.checked_add(bytes).ok_or_else(integrity)?;
    }
    Ok(total)
}
fn sync_dir(path: &Path) -> Result<(), ApiError> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::ImportStage;

    fn wav() -> Vec<u8> {
        let mut bytes = b"RIFF".to_vec();
        bytes.extend(44u32.to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(8000u32.to_le_bytes());
        bytes.extend(16000u32.to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(8u32.to_le_bytes());
        bytes.extend([0; 8]);
        bytes
    }

    fn journal(original: &Path) -> Journal {
        fs::write(original, wav()).unwrap();
        let asset = vault_media::inspect(original).unwrap();
        Journal {
            artwork_upload_unknown: false,
            path: original.to_owned(),
            staged_name: Some(asset.filename),
            info: ImportInfo {
                op_id: Uuid::new_v4(),
                track_id: Uuid::new_v4(),
                source_id: Uuid::new_v4(),
                metadata: asset.metadata,
                sha256: asset.sha256,
                size_bytes: asset.size_bytes,
                stage: ImportStage::Staging,
                media_message_id: None,
            },
        }
    }

    #[test]
    fn pending_import_budget_counts_existing_files_and_cancel_does_not_publish() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(Some(dir.path().join("vault.sqlite")), Uuid::new_v4()).unwrap();
        let journal = journal(&dir.path().join("original.wav"));
        let occupied = storage.root.join("unclassified.bin");
        fs::File::create(&occupied)
            .unwrap()
            .set_len(MAX_STAGED_BYTES)
            .unwrap();
        assert_eq!(
            Storage::prepare(&storage.root, &journal, &CancellationToken::new())
                .unwrap_err()
                .code,
            ErrorCode::Storage
        );
        assert_eq!(fs::metadata(&occupied).unwrap().len(), MAX_STAGED_BYTES);
        assert!(!storage.root.join(journal.info.op_id.to_string()).exists());
        fs::remove_file(occupied).unwrap();
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            Storage::prepare(&storage.root, &journal, &cancel)
                .unwrap_err()
                .code,
            ErrorCode::Cancelled
        );
        assert_eq!(fs::read_dir(&storage.root).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn staging_and_cleanup_refuse_job_links_and_path_escape() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::open(Some(dir.path().join("vault.sqlite")), Uuid::new_v4()).unwrap();
        let mut journal = journal(&dir.path().join("original.wav"));
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let target = outside.join("original.wav");
        fs::write(&target, wav()).unwrap();
        symlink(&outside, storage.root.join(journal.info.op_id.to_string())).unwrap();
        assert_eq!(
            Storage::prepare(&storage.root, &journal, &CancellationToken::new())
                .unwrap_err()
                .code,
            ErrorCode::Integrity
        );
        assert_eq!(
            Storage::remove(&storage.root, &journal).unwrap_err().code,
            ErrorCode::Integrity
        );
        journal.staged_name = Some("../outside/original.wav".into());
        assert_eq!(
            Storage::remove(&storage.root, &journal).unwrap_err().code,
            ErrorCode::Integrity
        );
        assert_eq!(fs::read(target).unwrap(), wav());
    }

    #[cfg(unix)]
    #[test]
    fn recovery_does_not_follow_a_scratch_directory_link() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let database = dir.path().join("vault.sqlite");
        let vault = Uuid::new_v4();
        let storage = Storage::open(Some(database.clone()), vault).unwrap();
        let job = storage.root.join(Uuid::new_v4().to_string());
        fs::create_dir(&job).unwrap();
        let outside = dir.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let target = outside.join("keep");
        fs::write(&target, b"preserve").unwrap();
        symlink(outside, job.join(".copy")).unwrap();
        drop(storage);
        assert!(matches!(
            Storage::open(Some(database), vault),
            Err(ApiError {
                code: ErrorCode::Integrity,
                ..
            })
        ));
        assert_eq!(fs::read(target).unwrap(), b"preserve");
    }
}
