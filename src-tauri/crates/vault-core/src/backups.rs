//! Resolves portable archives without assuming that old Telegram IDs still exist.
use crate::{ApiError, ErrorCode};
use tokio_util::sync::CancellationToken;
use vault_domain::{Track, UserId};
use vault_telegram::{Telegram, TelegramError};

pub(crate) async fn download_source<T: Telegram>(
    transport: &T,
    track: &Track,
    owner: UserId,
    cache: &vault_media::CacheManager,
    cancel: &CancellationToken,
) -> Result<(vault_media::CacheLease, vault_snapshot::backup::Checksum), ApiError> {
    for source in track.sources.values() {
        if cancel.is_cancelled() {
            return Err(ApiError::new(ErrorCode::Cancelled));
        }
        // Metadata archives can contain arbitrary references. Verify ownership
        // before reading any referenced chat and hash every downloaded byte.
        transport
            .verify_channel(source.chat_id, owner, cancel)
            .await?;
        let message = match transport
            .message(source.chat_id, source.message_id, cancel)
            .await
        {
            Ok(Some(message)) => message,
            Ok(None) | Err(TelegramError::Request { code: 404 }) => continue,
            Err(error) => return Err(error.into()),
        };
        let Some(file) = message.file else { continue };
        let bytes = u64::try_from(file.size).map_err(|_| ApiError::new(ErrorCode::Integrity))?;
        if source.size_bytes.is_some_and(|expected| expected != bytes) {
            continue;
        }
        let lease = vault_media::download_to_cache(
            transport,
            &file,
            cache,
            vault_media::DownloadOptions {
                cache_key: format!("restore:{}:{}", track.track_id, source.source_id),
                expected_sha256: source.sha256.clone(),
                pinned: false,
            },
            cancel,
            &|_, _| {},
        )
        .await?;
        let path = lease.path.clone();
        let token = cancel.clone();
        let checksum = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut file = std::fs::File::open(path)?;
            let mut digest = sha2::Sha256::new();
            let mut buffer = [0u8; 256 * 1024];
            let mut count = 0u64;
            loop {
                if token.is_cancelled() {
                    return Err(ApiError::new(ErrorCode::Cancelled));
                }
                let n = file.read(&mut buffer)?;
                if n == 0 {
                    break;
                }
                count = count
                    .checked_add(n as u64)
                    .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
                sha2::Digest::update(&mut digest, &buffer[..n]);
            }
            if count != bytes {
                return Err(ApiError::new(ErrorCode::Integrity));
            }
            Ok(vault_snapshot::backup::Checksum {
                bytes,
                sha256: hex::encode(sha2::Digest::finalize(digest)),
            })
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))??;
        return Ok((lease, checksum));
    }
    Err(ApiError::new(ErrorCode::MediaSourceMissing))
}

use sha2::Digest;

pub(crate) async fn upload_restore<T: Telegram>(
    engine: &mut vault_sync::SyncEngine<T>,
    existing: &std::collections::BTreeMap<uuid::Uuid, vault_telegram::RemoteMessage>,
    import_id: uuid::Uuid,
    path: std::path::PathBuf,
    sum: &vault_snapshot::backup::Checksum,
    cancel: &CancellationToken,
) -> Result<vault_telegram::RemoteMessage, ApiError> {
    let marker = vault_protocol::Record::Media(vault_protocol::MediaMarker {
        vault_id: engine.binding().vault_id,
        import_id,
        sha256: Some(sum.sha256.clone()),
    });
    if let Some(message) = existing.get(&import_id) {
        if message.record()? != Some(marker)
            || message
                .file
                .as_ref()
                .is_none_or(|file| u64::try_from(file.size).ok() != Some(sum.bytes))
        {
            return Err(ApiError::new(ErrorCode::Integrity));
        }
        return Ok(message.clone());
    }
    let key = format!("backup.restore.upload.v1:{import_id}");
    if engine.local_setting(key.clone()).await?.is_some() {
        return Err(ApiError::new(ErrorCode::Timeout));
    }
    engine
        .set_local_setting(key.clone(), sum.sha256.clone())
        .await?;
    let result = engine
        .upload_document(
            path,
            vault_protocol::encode_text(&marker)
                .map_err(|_| ApiError::new(ErrorCode::Integrity))?,
            cancel,
        )
        .await;
    match result {
        Ok(message) => {
            if message.record()? != Some(marker)
                || message
                    .file
                    .as_ref()
                    .is_none_or(|file| u64::try_from(file.size).ok() != Some(sum.bytes))
            {
                return Err(ApiError::new(ErrorCode::Integrity));
            }
            Ok(message)
        }
        Err(error) => {
            if error.definitely_not_sent() {
                engine.clear_local_setting(key).await?;
            }
            Err(error.into())
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn restore_artwork<T: Telegram>(
    engine: &mut vault_sync::SyncEngine<T>,
    backup: &vault_snapshot::backup::VerifiedBackup,
    track: &vault_domain::Track,
    media_path: &std::path::Path,
    cache: &vault_media::CacheManager,
    existing: &std::collections::BTreeMap<uuid::Uuid, vault_telegram::RemoteMessage>,
    import_id: uuid::Uuid,
    cancel: &CancellationToken,
) -> Result<Option<vault_domain::Source>, ApiError> {
    let Some(art) = &track.metadata.artwork else {
        return Ok(None);
    };
    let downloaded;
    let extracted;
    let path = if let Some(path) = backup.artwork_path(track.track_id) {
        path.to_owned()
    } else if let Some(source) = &art.source {
        let mut only = track.clone();
        only.sources = std::collections::BTreeMap::from([(source.source_id, source.clone())]);
        downloaded = download_source(
            engine.transport(),
            &only,
            engine.binding().owner_id,
            cache,
            cancel,
        )
        .await?
        .0;
        downloaded.path.clone()
    } else {
        let audio = media_path.to_owned();
        let expected = art.sha256.clone();
        extracted = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
            use std::io::Write;
            let art = vault_media::embedded_artwork(&audio)?
                .ok_or_else(|| ApiError::new(ErrorCode::MediaSourceMissing))?;
            if art.sha256 != expected {
                return Err(ApiError::new(ErrorCode::Integrity));
            }
            let mut file = tempfile::NamedTempFile::new()?;
            file.write_all(&art.bytes)?;
            file.as_file().sync_all()?;
            Ok(file)
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))??;
        extracted.path().to_owned()
    };
    let checked = path.clone();
    let expected = art.sha256.clone();
    let sum = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
        use std::io::Read;
        let mut bytes = Vec::new();
        std::fs::File::open(checked)?
            .take(vault_media::MAX_ARTWORK_BYTES as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.is_empty()
            || bytes.len() > vault_media::MAX_ARTWORK_BYTES
            || vault_snapshot::digest(&bytes) != expected
        {
            return Err(ApiError::new(ErrorCode::Integrity));
        }
        Ok(vault_snapshot::backup::Checksum {
            bytes: bytes.len() as u64,
            sha256: expected,
        })
    })
    .await
    .map_err(|_| ApiError::new(ErrorCode::Internal))??;
    let message = upload_restore(engine, existing, import_id, path, &sum, cancel).await?;
    Ok(Some(vault_domain::Source {
        source_id: import_id,
        chat_id: engine.binding().chat_id,
        message_id: message.id,
        sha256: Some(sum.sha256),
        size_bytes: Some(sum.bytes),
    }))
}
