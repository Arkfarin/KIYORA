use super::*;
use std::io::{Read, Write};

pub(super) async fn publish<T: Telegram>(
    engine: &mut SyncEngine<T>,
    storage: &Storage,
    journal: &mut Journal,
    cancel: &CancellationToken,
) -> Result<(), ApiError> {
    let Some(artwork) = journal
        .info
        .metadata
        .artwork
        .clone()
        .filter(|art| art.source.is_none())
    else {
        return Ok(());
    };
    let artwork_id = Uuid::new_v5(&journal.info.op_id, b"artwork");
    let marker = Record::Media(MediaMarker {
        vault_id: engine.binding().vault_id,
        import_id: artwork_id,
        sha256: Some(artwork.sha256.clone()),
    });
    let uploaded = if journal.artwork_upload_unknown {
        let history = vault_telegram::scan_history(
            engine.transport(),
            engine.binding().chat_id,
            0,
            None,
            1_000_000,
            cancel,
        )
        .await?;
        let mut matches = Vec::new();
        for message in history {
            if message.record()? == Some(marker.clone()) {
                matches.push(message);
            }
        }
        if matches.len() > 1 {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        matches
            .pop()
            .ok_or_else(|| ApiError::new(ErrorCode::Timeout))?
    } else {
        let root = storage.root();
        let saved = journal.clone();
        let token = cancel.clone();
        let path = blocking(move || {
            let audio = Storage::verified(&root, &saved, &token)?;
            let cover = audio
                .parent()
                .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?
                .join(".artwork");
            let expected = saved
                .info
                .metadata
                .artwork
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
            if cover.exists() {
                let meta = std::fs::symlink_metadata(&cover)?;
                if meta.file_type().is_symlink()
                    || !meta.is_file()
                    || meta.len() > vault_media::MAX_ARTWORK_BYTES as u64
                {
                    return Err(ApiError::new(ErrorCode::Integrity));
                }
                let mut bytes = Vec::new();
                std::fs::File::open(&cover)?
                    .take(vault_media::MAX_ARTWORK_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)?;
                if vault_snapshot::digest(&bytes) != expected.sha256 {
                    return Err(ApiError::new(ErrorCode::Integrity));
                }
                return Ok(cover);
            }
            let asset = vault_media::inspect_with_cancel(&audio, &token)?
                .artwork
                .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
            if asset.sha256 != expected.sha256 || asset.mime_type != expected.mime_type {
                return Err(ApiError::new(ErrorCode::Integrity));
            }
            let parent = audio
                .parent()
                .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
            let mut temp = tempfile::NamedTempFile::new_in(parent)?;
            temp.write_all(&asset.bytes)?;
            temp.as_file().sync_all()?;
            temp.persist_noclobber(&cover)
                .map_err(|_| ApiError::new(ErrorCode::Storage))?;
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            Ok(cover)
        })
        .await?;
        journal.artwork_upload_unknown = true;
        save(engine, journal).await?;
        match engine
            .upload_document(
                path,
                vault_protocol::encode_text(&marker)
                    .map_err(|_| ApiError::new(ErrorCode::Integrity))?,
                cancel,
            )
            .await
        {
            Ok(message) => message,
            Err(error) => {
                if matches!(
                    error,
                    SyncError::Readonly
                        | SyncError::Telegram(
                            TelegramError::InvalidInput
                                | TelegramError::RateLimited { .. }
                                | TelegramError::Request { .. }
                        )
                ) {
                    journal.artwork_upload_unknown = false;
                    save(engine, journal).await?;
                }
                return Err(error.into());
            }
        }
    };
    if uploaded.record()? != Some(marker) {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    let file = uploaded
        .file
        .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
    let bytes = u64::try_from(file.size).map_err(|_| ApiError::new(ErrorCode::Integrity))?;
    if bytes == 0 || bytes > vault_media::MAX_ARTWORK_BYTES as u64 {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    // Independent read-back proves the published cover before it enters the
    // track metadata; the uncertain send journal remains resumable on failure.
    let range = engine.transport().download(file, 0, 0, cancel).await?;
    if !range.complete {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    let expected = artwork.sha256.clone();
    blocking(move || {
        let _lease = range.storage_lease;
        let mut contents = Vec::new();
        std::fs::File::open(range.path)?
            .take(vault_media::MAX_ARTWORK_BYTES as u64 + 1)
            .read_to_end(&mut contents)?;
        if contents.len() as u64 != bytes || vault_snapshot::digest(&contents) != expected {
            return Err(ApiError::new(ErrorCode::Integrity));
        }
        Ok(())
    })
    .await?;
    if let Some(art) = journal.info.metadata.artwork.as_mut() {
        art.source = Some(Source {
            source_id: artwork_id,
            chat_id: engine.binding().chat_id,
            message_id: uploaded.id,
            sha256: Some(artwork.sha256),
            size_bytes: Some(bytes),
        });
    }
    journal.artwork_upload_unknown = false;
    save(engine, journal).await
}
