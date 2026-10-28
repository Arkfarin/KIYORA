//! Logical metadata edits use the existing event journal and reducer CAS.
use crate::*;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_domain::{Action, Artwork, LyricsDocument, LyricsReference, MetadataPatch};
use vault_sync::SyncEngine;
use vault_telegram::Telegram;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MediaEdit {
    pub patch: MetadataPatch,
    pub artwork: Option<Vec<u8>>,
    pub lyrics: Option<LyricsDocument>,
}
pub(crate) async fn prepare_patch<T: Telegram>(
    engine: &mut SyncEngine<T>,
    op_id: Uuid,
    edit: MediaEdit,
    cancel: &CancellationToken,
) -> Result<MetadataPatch, ApiError> {
    let mut patch = edit.patch;
    // Arbitrary remote references are never accepted by this editor boundary.
    if patch.artwork.is_some() || patch.lyrics.is_some() {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    // Validate every attachment before the first remote side effect.
    let lyrics_bytes = edit
        .lyrics
        .as_ref()
        .map(|document| {
            if !document.valid() {
                return Err(ApiError::new(ErrorCode::InvalidInput));
            }
            let bytes =
                serde_json::to_vec(document).map_err(|_| ApiError::new(ErrorCode::InvalidInput))?;
            if bytes.len() > 512 * 1024 {
                return Err(ApiError::new(ErrorCode::InvalidInput));
            }
            Ok(bytes)
        })
        .transpose()?;
    if let Some(bytes) = edit.artwork {
        let artwork = tokio::task::spawn_blocking(move || vault_media::artwork(&bytes, None))
            .await
            .map_err(|_| ApiError::new(ErrorCode::Internal))??;
        let source = crate::attachments::publish(
            engine,
            Uuid::new_v5(&op_id, b"cover"),
            artwork.bytes,
            cancel,
        )
        .await?;
        patch.artwork = Some(Artwork {
            sha256: artwork.sha256,
            mime_type: artwork.mime_type,
            source: Some(source),
        });
        patch.clear_artwork = false;
    }
    if let Some(bytes) = lyrics_bytes {
        let sha256 = vault_snapshot::digest(&bytes);
        let source =
            crate::attachments::publish(engine, Uuid::new_v5(&op_id, b"lyrics"), bytes, cancel)
                .await?;
        patch.lyrics = Some(LyricsReference { sha256, source });
        patch.clear_lyrics = false;
    }
    Ok(patch)
}
pub(crate) async fn edit<T: Telegram>(
    engine: &mut SyncEngine<T>,
    op_id: Uuid,
    track_id: Uuid,
    edit: MediaEdit,
    cancel: &CancellationToken,
) -> Result<VaultResponse, ApiError> {
    if op_id.is_nil() || track_id.is_nil() || edit.patch.expected_order.is_none() {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    engine.require_online(cancel).await?;
    let intent_key = format!("metadata-edit.v1:{op_id}");
    let fingerprint = vault_snapshot::digest(
        &serde_json::to_vec(&(track_id, &edit))
            .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?,
    );
    let saved = engine.local_setting(intent_key.clone()).await?;
    let preflight = if saved.is_none() {
        ErrorCode::EditNotSent
    } else {
        ErrorCode::Conflict
    };
    if let Some(saved) = saved {
        if saved != fingerprint {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        if let Some(command) = engine.command(op_id).await? {
            return Ok(VaultResponse::Command(Box::new(command)));
        }
    }
    let state = engine.state().await?;
    let track = state
        .tracks
        .get(&track_id)
        .ok_or_else(|| ApiError::new(preflight))?;
    if Some(track.updated_order) != edit.patch.expected_order {
        return Err(ApiError::diagnostic(
            preflight,
            "track changed on another device; review the current version before saving",
        ));
    }
    let mut candidate = track.metadata.clone();
    edit.patch.apply_to(&mut candidate);
    if !vault_domain::valid_metadata(&candidate) {
        return Err(ApiError::new(preflight));
    }
    engine.set_local_setting(intent_key, fingerprint).await?;
    let patch = prepare_patch(engine, op_id, edit, cancel).await?;
    let command = engine
        .execute(
            op_id,
            track_id,
            Action::TrackMetadataUpdate { patch },
            cancel,
        )
        .await?;
    Ok(VaultResponse::Command(Box::new(command)))
}
