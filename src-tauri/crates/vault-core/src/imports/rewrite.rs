//! Explicit audio versioning. Old sources stay referenced; publication and the
//! metadata/source switch are one replayed event after confirmed upload.
use super::*;
use crate::MediaEdit;
use std::io::Read;
use vault_media::{CacheLease, CacheManager};

async fn fetch<T: Telegram>(
    engine: &SyncEngine<T>,
    cache: &CacheManager,
    source: &Source,
    max: u64,
    cancel: &CancellationToken,
) -> Result<CacheLease, ApiError> {
    if source.chat_id != engine.binding().chat_id {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    let key = format!("revision:{}", source.source_id);
    if let Some(lease) = cache.acquire(&key)? {
        return Ok(lease);
    }
    let file = engine
        .transport()
        .message(source.chat_id, source.message_id, cancel)
        .await?
        .and_then(|m| m.file)
        .ok_or_else(|| ApiError::new(ErrorCode::MediaSourceMissing))?;
    if file.size <= 0
        || file.size as u64 > max
        || source.size_bytes.is_some_and(|n| n != file.size as u64)
    {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    Ok(vault_media::download_to_cache(
        engine.transport(),
        &file,
        cache,
        vault_media::DownloadOptions {
            cache_key: key,
            expected_sha256: source.sha256.clone(),
            pinned: false,
        },
        cancel,
        &|_, _| {},
    )
    .await?)
}
async fn bytes<T: Telegram>(
    engine: &SyncEngine<T>,
    cache: &CacheManager,
    source: &Source,
    max: u64,
    cancel: &CancellationToken,
) -> Result<Vec<u8>, ApiError> {
    let lease = fetch(engine, cache, source, max, cancel).await?;
    blocking(move || {
        let mut data = Vec::new();
        std::fs::File::open(&lease.path)?
            .take(max + 1)
            .read_to_end(&mut data)?;
        if data.len() as u64 > max {
            return Err(ApiError::new(ErrorCode::Integrity));
        }
        Ok(data)
    })
    .await
}
#[allow(clippy::too_many_arguments)]
pub(crate) async fn rewrite<T: Telegram + Clone + 'static>(
    engine: &mut SyncEngine<T>,
    storage: &Storage,
    cache: &CacheManager,
    track_id: Uuid,
    op_id: Uuid,
    edit: MediaEdit,
    cancel: &CancellationToken,
    context: &Context,
    events: &EventBus,
) -> Result<VaultResponse, ApiError> {
    if track_id.is_nil() || op_id.is_nil() || edit.patch.expected_order.is_none() {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    engine.require_online(cancel).await?;
    let key = format!("rewrite.v1:{op_id}");
    let fingerprint = vault_snapshot::digest(
        &serde_json::to_vec(&(track_id, &edit))
            .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?,
    );
    if let Some(saved) = engine.local_setting(key.clone()).await? {
        if saved != fingerprint {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
    } else {
        engine.set_local_setting(key, fingerprint).await?;
    }
    if pending(engine, op_id).await? {
        return resume(engine, storage, op_id, cancel, context, events).await;
    }
    let state = engine.state().await?;
    let track = state
        .tracks
        .get(&track_id)
        .ok_or_else(|| ApiError::new(ErrorCode::Conflict))?;
    if Some(track.updated_order) != edit.patch.expected_order {
        return Err(ApiError::new(ErrorCode::Conflict));
    }
    let source = track
        .preferred_source_id
        .and_then(|id| track.sources.get(&id))
        .or_else(|| track.sources.values().next())
        .ok_or_else(|| ApiError::new(ErrorCode::MediaSourceMissing))?;
    let audio = match cache.acquire(&crate::service::media_cache_key(track_id, source.source_id))? {
        Some(lease) => lease,
        None => fetch(engine, cache, source, vault_media::MAX_IMPORT_BYTES, cancel).await?,
    };
    let mut metadata = track.metadata.clone();
    let cover_bytes = if let Some(data) = &edit.artwork {
        Some(data.clone())
    } else if edit.patch.clear_artwork {
        None
    } else if let Some(art) = &metadata.artwork {
        match &art.source {
            Some(source) => Some(
                bytes(
                    engine,
                    cache,
                    source,
                    vault_media::MAX_ARTWORK_BYTES as u64,
                    cancel,
                )
                .await?,
            ),
            None => None,
        }
    } else {
        None
    };
    let lyrics = if let Some(document) = &edit.lyrics {
        Some(document.clone())
    } else if edit.patch.clear_lyrics {
        None
    } else if let Some(reference) = &metadata.lyrics {
        let data = bytes(engine, cache, &reference.source, 512 * 1024, cancel).await?;
        Some(
            serde_json::from_slice::<vault_domain::LyricsDocument>(&data)
                .map_err(|_| ApiError::new(ErrorCode::Integrity))?,
        )
    } else {
        None
    };
    if lyrics.as_ref().is_some_and(|l| !l.valid()) {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    let expected_order = track.updated_order;
    let patch = crate::metadata::prepare_patch(engine, op_id, edit, cancel).await?;
    patch.apply_to(&mut metadata);
    if !vault_domain::valid_metadata(&metadata) {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    let codec = metadata.codec.to_ascii_lowercase();
    if !matches!(
        codec.as_str(),
        "mp3" | "flac" | "ogg" | "opus" | "m4a" | "mp4" | "wav" | "aiff" | "aif"
    ) {
        return Err(ApiError::diagnostic(
            ErrorCode::InvalidInput,
            "embedded tag writing is unsupported for this format; use Kiyora metadata mode",
        ));
    }
    let root = storage.root();
    let path = root.join(format!("revision-{op_id}.{codec}"));
    let target = path.clone();
    let metadata_copy = metadata.clone();
    blocking(move || {
        if target.exists() {
            vault_media::inspect(&target)?;
            return Ok(());
        }
        let copy = tempfile::Builder::new()
            .suffix(&format!(".{codec}"))
            .tempfile_in(&root)?;
        std::fs::copy(&audio.path, copy.path())?;
        let cover = cover_bytes
            .map(|data| vault_media::artwork(&data, None))
            .transpose()?;
        vault_media::write_tags(copy.path(), &metadata_copy, cover.as_ref(), lyrics.as_ref())?;
        copy.persist_noclobber(&target)
            .map_err(|e| ApiError::from(e.error))?;
        #[cfg(unix)]
        std::fs::File::open(root)?.sync_all()?;
        Ok(())
    })
    .await?;
    let result = start(
        engine,
        storage,
        path.clone(),
        None,
        track_id,
        Uuid::new_v5(&op_id, b"source"),
        op_id,
        true,
        Some(metadata),
        Some(expected_order),
        cancel,
        context,
        events,
    )
    .await?;
    if matches!(&result,VaultResponse::Command(c) if c.status==CommandStatus::Applied) {
        tokio::fs::remove_file(path).await?;
    }
    Ok(result)
}
