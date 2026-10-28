//! A Telegram audio can be copied without reading its payload. Its digest stays
//! unknown; only locally inspected bytes can establish a SHA-256 value.
use super::*;
use vault_domain::{ChatId, MessageId};

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Origin {
    pub chat_id: ChatId,
    pub message_id: MessageId,
}

/// Fetch only the copied message's separate thumbnail. Persist its digest before
/// publishing; an uncertain cover upload uses the ordinary artwork journal.
pub(super) async fn prepare_cover<T: Telegram>(
    engine: &mut SyncEngine<T>,
    journal: &mut Journal,
    cancel: &CancellationToken,
) -> Result<Option<tempfile::NamedTempFile>, ApiError> {
    if journal.incoming.is_none()
        || journal.staged_name.is_some()
        || journal.artwork_upload_unknown
        || journal
            .info
            .metadata
            .artwork
            .as_ref()
            .is_some_and(|art| art.source.is_some())
    {
        return Ok(None);
    }
    let id = journal
        .info
        .media_message_id
        .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
    let audio = engine
        .transport()
        .audio_info(engine.binding().chat_id, id, cancel)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
    let Some(cover) = audio.thumbnail else {
        if journal.info.metadata.artwork.is_some() {
            return Err(ApiError::new(ErrorCode::Integrity));
        }
        return Ok(None);
    };
    let size = usize::try_from(cover.size)
        .ok()
        .filter(|size| *size > 0 && *size <= vault_media::MAX_ARTWORK_BYTES)
        .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
    if !matches!(
        cover.mime_type.as_str(),
        "image/jpeg" | "image/png" | "image/webp"
    ) {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    let range = engine
        .transport()
        .download(cover.clone(), 0, 0, cancel)
        .await?;
    if !range.complete
        || range.file_offset != 0
        || range.offset != 0
        || range.available != cover.size
    {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    let (file, digest) = blocking(move || {
        use std::io::{Read, Write};
        let _lease = range.storage_lease;
        let metadata = std::fs::symlink_metadata(&range.path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() || metadata.len() != size as u64
        {
            return Err(ApiError::new(ErrorCode::Integrity));
        }
        let mut bytes = Vec::with_capacity(size);
        std::fs::File::open(&range.path)?
            .take(size as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() != size {
            return Err(ApiError::new(ErrorCode::Integrity));
        }
        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        Ok((file, vault_snapshot::digest(&bytes)))
    })
    .await?;
    if journal
        .info
        .metadata
        .artwork
        .as_ref()
        .is_some_and(|art| art.sha256 != digest || art.mime_type != cover.mime_type)
    {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    journal.info.metadata.artwork = Some(vault_domain::Artwork {
        sha256: digest,
        mime_type: cover.mime_type,
        source: None,
    });
    save(engine, journal).await?;
    Ok(Some(file))
}

/// A document was inspected locally before copying. Verify the saved copy too:
/// Telegram allows its owner to edit the original between inspection and copy.
pub(super) async fn verify_copy<T: Telegram + Clone + 'static>(
    engine: &SyncEngine<T>,
    journal: &Journal,
    cancel: &CancellationToken,
) -> Result<(), ApiError> {
    if journal.incoming.is_none() {
        return Ok(());
    }
    let Some(expected) = &journal.info.sha256 else {
        return Ok(());
    };
    let id = journal
        .info
        .media_message_id
        .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
    let media = engine
        .transport()
        .message(engine.binding().chat_id, id, cancel)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
    validate_media(&media, &journal.info, engine.binding().vault_id)?;
    let file = media
        .file
        .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
    let temporary = tempfile::tempdir().map_err(|_| ApiError::new(ErrorCode::Storage))?;
    let cache = vault_media::CacheManager::open(temporary.path(), journal.info.size_bytes)?;
    let _lease = vault_media::download_to_cache(
        engine.transport(),
        &file,
        &cache,
        vault_media::DownloadOptions {
            cache_key: format!("copy:{}", journal.info.op_id),
            expected_sha256: Some(expected.clone()),
            pinned: true,
        },
        cancel,
        &|_, _| {},
    )
    .await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn try_start<T: Telegram + Clone + 'static>(
    engine: &mut SyncEngine<T>,
    storage: &Storage,
    source_chat: ChatId,
    message_id: MessageId,
    track_id: Uuid,
    source_id: Uuid,
    op_id: Uuid,
    allow_duplicate: bool,
    cancel: &CancellationToken,
    context: &Context,
    events: &EventBus,
) -> Result<Option<VaultResponse>, ApiError> {
    let origin = Origin {
        chat_id: source_chat,
        message_id,
    };
    if let Some(raw) = engine.local_setting(key(op_id)).await? {
        let journal = decode(&raw, op_id)?;
        if let Some(saved) = &journal.incoming {
            if saved != &origin
                || journal.info.track_id != track_id
                || journal.info.source_id != source_id
            {
                return Err(ApiError::new(ErrorCode::Conflict));
            }
            return resume(engine, storage, op_id, cancel, context, events)
                .await
                .map(Some);
        }
        return Ok(None); // Resume an older staged import through its existing path.
    }
    if engine.command(op_id).await?.is_some() {
        return Ok(None);
    }
    engine.require_online(cancel).await?;
    // After projection recovery an applied operation may outlive local journals.
    // A missing journal is never permission to create another server copy.
    if engine.state().await?.applied.contains_key(&op_id) {
        return Err(ApiError::new(ErrorCode::Conflict));
    }
    let remote = engine
        .transport()
        .incoming_media(source_chat, message_id, cancel)
        .await?;
    if remote.record()?.is_some() {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    if remote.content != vault_telegram::MessageContent::Audio {
        return Ok(None); // Documents need local format and metadata inspection.
    }
    let audio = engine
        .transport()
        .audio_info(source_chat, message_id, cancel)
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
    let size_bytes = u64::try_from(audio.file.size)
        .ok()
        .filter(|size| *size > 0 && *size <= vault_media::MAX_IMPORT_BYTES)
        .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
    if remote.file.as_ref() != Some(&audio.file) {
        return Err(ApiError::new(ErrorCode::Conflict));
    }
    let filename = audio.file.name.replace('\\', "/");
    let path = std::path::Path::new(&filename);
    let title = if audio.title.trim().is_empty() {
        path.file_stem()
            .and_then(|value| value.to_str())
            .filter(|value| !value.is_empty())
            .unwrap_or("Untitled")
            .to_owned()
    } else {
        audio.title
    };
    if title.len() > 1024
        || audio.performer.len() > 1024
        || title.contains('\0')
        || audio.performer.contains('\0')
        || audio.duration_ms > i64::MAX as u64
    {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    let metadata = Metadata {
        title,
        performer: audio.performer,
        duration_ms: audio.duration_ms,
        codec: path
            .extension()
            .and_then(|value| value.to_str())
            .filter(|value| value.len() <= 16)
            .unwrap_or("")
            .to_ascii_lowercase(),
        ..Metadata::default()
    };
    let state = engine.state().await?;
    if state.tracks.contains_key(&track_id) || state.tombstones.tracks.contains(&track_id) {
        return Err(ApiError::new(ErrorCode::Conflict));
    }
    let possible_duplicate_track_ids: Vec<_> = state
        .tracks
        .iter()
        .filter(|(_, track)| vault_media::metadata_similar(&metadata, &track.metadata))
        .map(|(id, _)| *id)
        .collect();
    if !possible_duplicate_track_ids.is_empty() && !allow_duplicate {
        return Ok(Some(VaultResponse::ImportReview {
            metadata: Box::new(metadata),
            sha256: None,
            size_bytes,
            duplicate_track_ids: Vec::new(),
            possible_duplicate_track_ids,
        }));
    }
    let journal = Journal {
        replacement_order: None,
        info: ImportInfo {
            op_id,
            track_id,
            source_id,
            metadata,
            sha256: None,
            size_bytes,
            stage: ImportStage::Prepared,
            media_message_id: None,
        },
        path: PathBuf::new(),
        staged_name: None,
        artwork_upload_unknown: false,
        incoming: Some(origin),
    };
    save(engine, &journal).await?;
    events.emit(context, EventData::Import(Box::new(journal.info.clone())));
    resume(engine, storage, op_id, cancel, context, events)
        .await
        .map(Some)
}
