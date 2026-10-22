use crate::{CacheLease, CacheManager, MAX_IMPORT_BYTES, MediaError};
use sha2::{Digest, Sha256};
use std::io::SeekFrom;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;
use vault_telegram::{RemoteFile, Telegram};

const RANGE_BYTES: i64 = 1024 * 1024;
const MAX_RETRIES: usize = 3;

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub cache_key: String,
    pub expected_sha256: Option<String>,
    pub pinned: bool,
}

/// Fetches a complete file in bounded ranges, hashes it while streaming, then
/// publishes it to the cache only after size and optional digest verification.
/// The callback receives monotonic byte progress and can publish job events.
pub async fn download_to_cache<T: Telegram>(
    telegram: &T,
    file: &RemoteFile,
    cache: &CacheManager,
    options: DownloadOptions,
    cancel: &CancellationToken,
    progress: &(dyn Fn(u64, u64) + Sync),
) -> Result<CacheLease, MediaError> {
    if file.file_id <= 0
        || file.size <= 0
        || u64::try_from(file.size).map_or(true, |size| size > MAX_IMPORT_BYTES)
        || options
            .expected_sha256
            .as_deref()
            .is_some_and(|hash| !valid_digest(hash))
    {
        return Err(MediaError::Range);
    }
    let total = u64::try_from(file.size).map_err(|_| MediaError::Range)?;
    let mut staged = tempfile::Builder::new()
        .prefix("kiyora-download-")
        .tempfile()?;
    let mut output = tokio::fs::File::from_std(staged.as_file_mut().try_clone()?);
    let mut hasher = Sha256::new();
    let mut offset = 0i64;
    while offset < file.size {
        if cancel.is_cancelled() {
            return Err(MediaError::Telegram(
                vault_telegram::TelegramError::Cancelled,
            ));
        }
        let limit = (file.size - offset).min(RANGE_BYTES);
        let mut last_error = None;
        let range = {
            let mut downloaded = None;
            for attempt in 0..MAX_RETRIES {
                match telegram.download(file.clone(), offset, limit, cancel).await {
                    Ok(value) => {
                        downloaded = Some(value);
                        break;
                    }
                    Err(error) => {
                        if matches!(error, vault_telegram::TelegramError::Cancelled) {
                            return Err(MediaError::Telegram(error));
                        }
                        last_error = Some(error);
                        if attempt + 1 < MAX_RETRIES {
                            tokio::select! {
                                _ = cancel.cancelled() => return Err(MediaError::Telegram(vault_telegram::TelegramError::Cancelled)),
                                _ = tokio::time::sleep(std::time::Duration::from_millis(150 * (attempt as u64 + 1))) => {}
                            }
                        }
                    }
                }
            }
            downloaded.ok_or_else(|| {
                MediaError::Telegram(last_error.unwrap_or(vault_telegram::TelegramError::Timeout))
            })?
        };
        if range.file_offset < 0
            || range.file_offset > offset
            || range.offset != offset
            || range.available < limit
        {
            return Err(MediaError::Range);
        }
        let path_metadata = std::fs::symlink_metadata(&range.path)?;
        if !path_metadata.is_file() || path_metadata.file_type().is_symlink() {
            return Err(MediaError::Range);
        }
        let local_end =
            u64::try_from(offset - range.file_offset + limit).map_err(|_| MediaError::Range)?;
        if path_metadata.len() < local_end {
            return Err(MediaError::Range);
        }
        let mut input = tokio::fs::File::open(&range.path).await?;
        input
            .seek(SeekFrom::Start(
                u64::try_from(offset - range.file_offset).map_err(|_| MediaError::Range)?,
            ))
            .await?;
        let mut remaining = usize::try_from(limit).map_err(|_| MediaError::Range)?;
        let mut buffer = vec![0u8; 64 * 1024];
        while remaining > 0 {
            let read_cap = remaining.min(buffer.len());
            let read = input.read(&mut buffer[..read_cap]).await?;
            if read == 0 {
                return Err(MediaError::Range);
            }
            hasher.update(&buffer[..read]);
            output.write_all(&buffer[..read]).await?;
            remaining -= read;
        }
        offset = offset.checked_add(limit).ok_or(MediaError::Range)?;
        progress(u64::try_from(offset).map_err(|_| MediaError::Range)?, total);
    }
    output.flush().await?;
    drop(output);
    staged.as_file().sync_all()?;
    let digest = hex::encode(hasher.finalize());
    if options
        .expected_sha256
        .as_deref()
        .is_some_and(|expected| expected != digest)
    {
        return Err(MediaError::Range);
    }
    cache.insert(options.cache_key, staged.path(), &digest, options.pinned)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
