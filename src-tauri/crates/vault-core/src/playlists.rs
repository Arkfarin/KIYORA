//! Playlist artwork uses the same durable attachment journal as track metadata.
use crate::{ApiError, ErrorCode, VaultResponse};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_domain::{Action, Artwork, MessageId, PlaylistPatch};
use vault_sync::SyncEngine;
use vault_telegram::Telegram;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaylistEdit {
    pub title: String,
    pub artwork: Option<Vec<u8>>,
    pub clear_artwork: bool,
    #[schemars(with = "String")]
    pub expected_order: MessageId,
}

pub(crate) async fn edit<T: Telegram>(
    engine: &mut SyncEngine<T>,
    op_id: Uuid,
    playlist_id: Uuid,
    edit: PlaylistEdit,
    cancel: &CancellationToken,
) -> Result<VaultResponse, ApiError> {
    if op_id.is_nil()
        || playlist_id.is_nil()
        || edit
            .artwork
            .as_ref()
            .is_some_and(|bytes| bytes.len() > 8 * 1024 * 1024)
        || edit.title.len() > 4096
    {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    engine.require_online(cancel).await?;
    let key = format!("playlist-edit.v1:{op_id}");
    let fingerprint = vault_snapshot::digest(
        &serde_json::to_vec(&(playlist_id, &edit))
            .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?,
    );
    let saved = engine.local_setting(key.clone()).await?;
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
    let playlist = state
        .playlists
        .get(&playlist_id)
        .ok_or_else(|| ApiError::new(preflight))?;
    if playlist.updated_order != edit.expected_order
        || (edit.clear_artwork && edit.artwork.is_some())
    {
        return Err(ApiError::new(preflight));
    }
    let mut patch = PlaylistPatch {
        title: Some(edit.title),
        clear_artwork: edit.clear_artwork,
        expected_order: Some(edit.expected_order),
        ..Default::default()
    };
    if !vault_domain::valid_playlist_patch(&patch) {
        return Err(ApiError::new(preflight));
    }
    // Decode and validate before persisting an intent or sending a document.
    let art = if let Some(bytes) = edit.artwork {
        Some(
            tokio::task::spawn_blocking(move || vault_media::artwork(&bytes, None))
                .await
                .map_err(|_| ApiError::new(ErrorCode::Internal))?
                .map_err(|_| ApiError::new(preflight))?,
        )
    } else {
        None
    };
    engine.set_local_setting(key, fingerprint).await?;
    if let Some(art) = art {
        let source = crate::attachments::publish(
            engine,
            Uuid::new_v5(&op_id, b"playlist-cover"),
            art.bytes,
            cancel,
        )
        .await?;
        patch.artwork = Some(Artwork {
            sha256: art.sha256,
            mime_type: art.mime_type,
            source: Some(source),
        });
    }
    let command = engine
        .execute(
            op_id,
            playlist_id,
            Action::PlaylistMetadataUpdate { patch },
            cancel,
        )
        .await?;
    Ok(VaultResponse::Command(Box::new(command)))
}
