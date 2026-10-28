//! Durable documents for artwork and lyrics. An uncertain send is reconciled,
//! never blindly retried. Original audio and library state are untouched here.
use crate::*;
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_domain::Source;
use vault_protocol::{MediaMarker, Record};
use vault_sync::SyncEngine;
use vault_telegram::Telegram;

#[derive(Serialize, Deserialize)]
struct Publication {
    sha256: String,
    unknown: bool,
    source: Option<Source>,
}
pub(crate) async fn publish<T: Telegram>(
    engine: &mut SyncEngine<T>,
    id: Uuid,
    bytes: Vec<u8>,
    cancel: &CancellationToken,
) -> Result<Source, ApiError> {
    if bytes.is_empty() || bytes.len() > vault_media::MAX_ARTWORK_BYTES {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    engine.require_online(cancel).await?;
    let key = format!("attachment.v1:{id}");
    let digest = vault_snapshot::digest(&bytes);
    let mut publication: Publication = match engine.local_setting(key.clone()).await? {
        Some(value) => {
            serde_json::from_str(&value).map_err(|_| ApiError::new(ErrorCode::Integrity))?
        }
        None => Publication {
            sha256: digest.clone(),
            unknown: false,
            source: None,
        },
    };
    if publication.sha256 != digest {
        return Err(ApiError::new(ErrorCode::Conflict));
    }
    if let Some(source) = publication.source {
        return Ok(source);
    }
    let marker = Record::Media(MediaMarker {
        vault_id: engine.binding().vault_id,
        import_id: id,
        sha256: Some(digest.clone()),
    });
    let message = if publication.unknown {
        let history = vault_telegram::scan_history(
            engine.transport(),
            engine.binding().chat_id,
            0,
            None,
            1_000_000,
            cancel,
        )
        .await?;
        let mut found = None;
        for message in history {
            if message.record()? == Some(marker.clone()) {
                if found.is_some() {
                    return Err(ApiError::new(ErrorCode::Conflict));
                }
                found = Some(message);
            }
        }
        found.ok_or_else(|| ApiError::new(ErrorCode::Timeout))?
    } else {
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        publication.unknown = true;
        engine
            .set_local_setting(
                key.clone(),
                serde_json::to_string(&publication)
                    .map_err(|_| ApiError::new(ErrorCode::Internal))?,
            )
            .await?;
        match engine
            .upload_document(
                file.path().to_owned(),
                vault_protocol::encode_text(&marker)
                    .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?,
                cancel,
            )
            .await
        {
            Ok(message) => message,
            Err(error) => {
                if error.definitely_not_sent() {
                    publication.unknown = false;
                    engine
                        .set_local_setting(
                            key.clone(),
                            serde_json::to_string(&publication)
                                .map_err(|_| ApiError::new(ErrorCode::Internal))?,
                        )
                        .await?;
                }
                return Err(error.into());
            }
        }
    };
    if message.record()? != Some(marker) {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    let file = message
        .file
        .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
    if u64::try_from(file.size).ok() != Some(bytes.len() as u64) {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    let range = engine.transport().download(file, 0, 0, cancel).await?;
    if !range.complete {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    let size = bytes.len() as u64;
    let expected = digest.clone();
    tokio::task::spawn_blocking(move || -> Result<(), ApiError> {
        let _lease = range.storage_lease;
        let mut read = Vec::new();
        std::fs::File::open(range.path)?
            .take(vault_media::MAX_ARTWORK_BYTES as u64 + 1)
            .read_to_end(&mut read)?;
        if read != bytes || vault_snapshot::digest(&read) != expected {
            return Err(ApiError::new(ErrorCode::Integrity));
        }
        Ok(())
    })
    .await
    .map_err(|_| ApiError::new(ErrorCode::Internal))??;
    let source = Source {
        source_id: id,
        chat_id: engine.binding().chat_id,
        message_id: message.id,
        sha256: Some(digest),
        size_bytes: Some(size),
    };
    publication.source = Some(source.clone());
    publication.unknown = false;
    engine
        .set_local_setting(
            key,
            serde_json::to_string(&publication).map_err(|_| ApiError::new(ErrorCode::Internal))?,
        )
        .await?;
    Ok(source)
}
