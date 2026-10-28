use super::*;
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StorageUsage {
    pub context: Context,
    /// Sum of regular-file lengths in this profile; filesystem compression,
    /// sparse allocation and OS secure storage are outside this measurement.
    pub profile_bytes: u64,
    pub verified_cache: vault_media::CacheStats,
    pub cache_directory_bytes: u64,
    pub tdlib_files_bytes: u64,
    pub tdlib_database_bytes: u64,
    pub projection_bytes: u64,
    pub staged_import_bytes: u64,
    pub staged_import_limit_bytes: u64,
    pub other_profile_bytes: u64,
    pub telegram: Option<vault_telegram::TdStorageStats>,
}

fn usage(path: &Path, depth: usize) -> Result<u64, ApiError> {
    if depth > 32 {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    // Never follow a link outside the owned profile during accounting.
    if metadata.file_type().is_symlink() {
        return Ok(0);
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    if !metadata.is_dir() {
        return Ok(0);
    }
    let mut bytes = 0_u64;
    for entry in std::fs::read_dir(path)? {
        bytes = bytes
            .checked_add(usage(&entry?.path(), depth + 1)?)
            .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
    }
    Ok(bytes)
}

impl Backend {
    /// Rehydrate transfers after a WebView reload without starting any downloads.
    pub async fn downloads(&self, session_id: Uuid) -> Result<Vec<DownloadInfo>, ApiError> {
        let guard = self.session.lock().await;
        let session = guard
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(session, session_id)?;
        session
            .service
            .as_ref()
            .map_or(Ok(Vec::new()), VaultService::downloads)
    }
    pub async fn cached_media(
        &self,
        session_id: Uuid,
    ) -> Result<Vec<vault_media::CacheEntry>, ApiError> {
        let guard = self.session.lock().await;
        let session = guard
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(session, session_id)?;
        Ok(session.cache.entries()?)
    }
    pub async fn storage_usage(
        &self,
        session_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<StorageUsage, ApiError> {
        let (context, directory, cache, transport) = {
            let guard = self.session.lock().await;
            let session = guard
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(session, session_id)?;
            (
                session.context.clone(),
                session.lease.directory().to_owned(),
                session.cache.clone(),
                session
                    .native
                    .as_ref()
                    .map(|native| native.transport.clone()),
            )
        };
        let verified_cache = cache.stats()?;
        let sizes = tokio::task::spawn_blocking(move || {
            let mut sizes = BTreeMap::new();
            for entry in std::fs::read_dir(directory)? {
                let entry = entry?;
                sizes.insert(
                    entry.file_name().to_string_lossy().into_owned(),
                    usage(&entry.path(), 0)?,
                );
            }
            Ok::<_, ApiError>(sizes)
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))??;
        let profile_bytes = sizes
            .values()
            .try_fold(0_u64, |a, b| a.checked_add(*b))
            .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
        let size = |name: &str| sizes.get(name).copied().unwrap_or_default();
        let projection_bytes =
            size("vault.sqlite") + size("vault.sqlite-wal") + size("vault.sqlite-shm");
        let cache_directory_bytes = size("cache");
        let tdlib_files_bytes = size("files");
        let tdlib_database_bytes = size("tdlib");
        let staged_import_bytes = size("vault.sqlite.imports");
        let other_profile_bytes = profile_bytes
            - projection_bytes
            - cache_directory_bytes
            - tdlib_files_bytes
            - tdlib_database_bytes
            - staged_import_bytes;
        let telegram = match transport {
            Some(transport) => Some(transport.storage_stats(cancel).await?),
            None => None,
        };
        Ok(StorageUsage {
            context,
            profile_bytes,
            verified_cache,
            cache_directory_bytes,
            tdlib_files_bytes,
            tdlib_database_bytes,
            projection_bytes,
            staged_import_bytes,
            staged_import_limit_bytes: vault_media::MAX_IMPORT_BYTES,
            other_profile_bytes,
            telegram,
        })
    }

    /// Explicit local cleanup, independent of remote GC and the pinned cache.
    pub async fn trim_telegram_storage(
        &self,
        session_id: Uuid,
        target_bytes: u64,
        cancel: &CancellationToken,
    ) -> Result<StorageUsage, ApiError> {
        let transport = {
            let guard = self.session.lock().await;
            let session = guard
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(session, session_id)?;
            session
                .native
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?
                .transport
                .clone()
        };
        transport.trim_storage(target_bytes, cancel).await?;
        self.storage_usage(session_id, cancel).await
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn accounting_includes_nested_files_but_never_follows_links()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let outside = tempfile::tempdir()?;
        std::fs::create_dir(root.path().join("files"))?;
        std::fs::write(root.path().join("files/audio"), [0_u8; 9])?;
        std::fs::write(outside.path().join("private"), [0_u8; 20])?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(outside.path(), root.path().join("outside"))?;
        assert_eq!(super::usage(root.path(), 0)?, 9);
        Ok(())
    }
}
