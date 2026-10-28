use super::*;
use crate::{ArtworkData, MediaEdit};
use std::{
    io::{Read, Write},
    path::Path,
};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportPreview {
    pub preview_id: Uuid,
    pub filename: String,
    pub metadata: Metadata,
    pub sha256: String,
    pub size_bytes: u64,
    pub artwork: Option<ArtworkData>,
    pub lyrics: Option<vault_domain::LyricsDocument>,
    pub warnings: Vec<String>,
}
fn preview_dir(storage: &Storage, id: Uuid) -> Result<PathBuf, ApiError> {
    if id.is_nil() {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    Ok(storage.root().join(format!("preview-{id}")))
}
fn load(dir: &Path) -> Result<ImportPreview, ApiError> {
    let mut bytes = Vec::new();
    std::fs::File::open(dir.join("preview.json"))?
        .take(24 * 1024 * 1024)
        .read_to_end(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|_| ApiError::new(ErrorCode::Integrity))
}
fn total_bytes(dir: &Path, depth: u32) -> Result<u64, ApiError> {
    if depth > 4 {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    let mut total = 0u64;
    for item in std::fs::read_dir(dir)? {
        let item = item?;
        let meta = item.metadata()?;
        if item.file_type()?.is_symlink() {
            return Err(ApiError::new(ErrorCode::Integrity));
        }
        total = total.saturating_add(if meta.is_dir() {
            total_bytes(&item.path(), depth + 1)?
        } else {
            meta.len()
        });
    }
    Ok(total)
}
pub(crate) async fn inspect(
    storage: &Storage,
    path: PathBuf,
    id: Uuid,
    cancel: &CancellationToken,
) -> Result<ImportPreview, ApiError> {
    let root = storage.root();
    let dir = preview_dir(storage, id)?;
    let token = cancel.clone();
    blocking(move || {
        if dir.exists() {
            return load(&dir);
        }
        let asset = vault_media::inspect_with_cancel(&path, &token)?;
        if total_bytes(&root, 0)?.saturating_add(asset.size_bytes) > vault_media::MAX_IMPORT_BYTES {
            return Err(ApiError::new(ErrorCode::Storage));
        }
        let temporary = tempfile::Builder::new()
            .prefix(".preview-")
            .tempdir_in(&root)?;
        let staged = vault_media::stage_with_cancel(&asset, temporary.path(), &token)?;
        let stable = temporary.path().join(&asset.filename);
        staged
            .persist_noclobber(&stable)
            .map_err(|e| ApiError::from(e.error))?;
        let mut warnings = Vec::new();
        let artwork = match asset.artwork {
            Some(art) => match vault_media::artwork(&art.bytes, None) {
                Ok(art) => Some(ArtworkData {
                    bytes: art.bytes,
                    sha256: art.sha256,
                    mime_type: art.mime_type,
                }),
                Err(_) => {
                    warnings.push(
                        "Встроенная обложка повреждена или превышает допустимые размеры.".into(),
                    );
                    None
                }
            },
            None => None,
        };
        let lyrics = match vault_media::embedded_lyrics(&stable) {
            Ok(lyrics) => lyrics,
            Err(_) => {
                warnings.push(
                    "Встроенный текст не удалось разобрать; его можно добавить вручную.".into(),
                );
                None
            }
        };
        let preview = ImportPreview {
            preview_id: id,
            filename: asset.filename,
            metadata: asset.metadata,
            sha256: asset.sha256,
            size_bytes: asset.size_bytes,
            artwork,
            lyrics,
            warnings,
        };
        let mut file = std::fs::File::create(temporary.path().join("preview.json"))?;
        file.write_all(
            &serde_json::to_vec(&preview).map_err(|_| ApiError::new(ErrorCode::Internal))?,
        )?;
        file.sync_all()?;
        std::fs::rename(temporary.path(), &dir)?;
        #[cfg(unix)]
        std::fs::File::open(&root)?.sync_all()?;
        Ok(preview)
    })
    .await
}
pub(crate) async fn discard(storage: &Storage, id: Uuid) -> Result<(), ApiError> {
    let dir = preview_dir(storage, id)?;
    blocking(move || {
        if dir.exists() {
            std::fs::remove_dir_all(dir)?;
        }
        Ok(())
    })
    .await
}
#[allow(clippy::too_many_arguments)]
pub(crate) async fn commit<T: Telegram + Clone + 'static>(
    engine: &mut SyncEngine<T>,
    storage: &Storage,
    id: Uuid,
    track_id: Uuid,
    source_id: Uuid,
    edit: Option<MediaEdit>,
    write_tags: bool,
    allow_duplicate: bool,
    cancel: &CancellationToken,
    context: &Context,
    events: &EventBus,
) -> Result<VaultResponse, ApiError> {
    if id.is_nil() || track_id.is_nil() || source_id.is_nil() {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    engine.require_online(cancel).await?;
    let fingerprint = vault_snapshot::digest(
        &serde_json::to_vec(&(track_id, source_id, &edit, write_tags))
            .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?,
    );
    let intent_key = format!("prepared-import.v1:{id}");
    if let Some(saved) = engine.local_setting(intent_key.clone()).await? {
        if saved != fingerprint {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
    } else {
        engine.set_local_setting(intent_key, fingerprint).await?;
    }
    // Check request identity even when an existing journal has already uploaded.
    if pending(engine, id).await? {
        return resume(engine, storage, id, cancel, context, events).await;
    }
    let dir = preview_dir(storage, id)?;
    let load_dir = dir.clone();
    let preview = blocking(move || load(&load_dir)).await?;
    let mut metadata = preview.metadata.clone();
    let mut edit = edit.unwrap_or(MediaEdit {
        patch: Default::default(),
        artwork: None,
        lyrics: None,
    });
    // Full metadata is kept in Kiyora even when audio bytes are deliberately unchanged.
    if edit.lyrics.is_none() && !edit.patch.clear_lyrics {
        edit.lyrics = preview.lyrics.clone();
    }
    if edit.artwork.is_none() && !edit.patch.clear_artwork {
        edit.artwork = preview.artwork.as_ref().map(|a| a.bytes.clone());
        if edit.artwork.is_none() {
            edit.patch.clear_artwork = true;
        }
    }
    let raw_cover = edit.artwork.clone();
    let lyrics = edit.lyrics.clone();
    let patch = crate::metadata::prepare_patch(engine, id, edit, cancel).await?;
    patch.apply_to(&mut metadata);
    if !vault_domain::valid_metadata(&metadata) {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    let mut path = dir.join(&preview.filename);
    if write_tags {
        let original = path.clone();
        let target = dir.join(format!("tagged-{}", preview.filename));
        let target_copy = target.clone();
        let metadata_copy = metadata.clone();
        let token = cancel.clone();
        blocking(move || {
            if target_copy.exists() {
                vault_media::inspect_with_cancel(&target_copy, &token)?;
                return Ok(());
            }
            let asset = vault_media::inspect_with_cancel(&original, &token)?;
            let copy = vault_media::stage_with_cancel(
                &asset,
                original
                    .parent()
                    .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?,
                &token,
            )?;
            let cover = raw_cover
                .map(|bytes| vault_media::artwork(&bytes, None))
                .transpose()?;
            vault_media::write_tags(copy.path(), &metadata_copy, cover.as_ref(), lyrics.as_ref())?;
            copy.persist_noclobber(&target_copy)
                .map_err(|e| ApiError::from(e.error))?;
            Ok(())
        })
        .await?;
        path = target;
    }
    let result = start(
        engine,
        storage,
        path,
        None,
        track_id,
        source_id,
        id,
        allow_duplicate,
        Some(metadata),
        None,
        cancel,
        context,
        events,
    )
    .await?;
    if matches!(&result, VaultResponse::Command(command) if command.status == CommandStatus::Applied)
    {
        discard(storage, id).await?;
    }
    Ok(result)
}

pub(crate) async fn list(storage: &Storage) -> Result<Vec<ImportPreview>, ApiError> {
    let root = storage.root();
    blocking(move || {
        let mut previews = Vec::new();
        for item in std::fs::read_dir(root)? {
            let item = item?;
            if item.file_name().to_string_lossy().starts_with("preview-")
                && item.file_type()?.is_dir()
                && !item.file_type()?.is_symlink()
            {
                previews.push(load(&item.path())?);
                if previews.len() >= 50 {
                    break;
                }
            }
        }
        Ok(previews)
    })
    .await
}
