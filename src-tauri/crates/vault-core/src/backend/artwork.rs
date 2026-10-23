use super::*;
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ArtworkData {
    pub mime_type: String,
    pub sha256: String,
    pub bytes: Vec<u8>,
}
impl Backend {
    pub async fn artwork(
        &self,
        session_id: Uuid,
        track_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<Option<ArtworkData>, ApiError> {
        let reader = self.local_reader(session_id).await?;
        let state = reader.state().await?;
        let track = state
            .tracks
            .get(&track_id)
            .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
        let Some(artwork) = track.metadata.artwork.clone() else {
            return Ok(None);
        };
        let (cache, transport, owner) = {
            let guard = self.session.lock().await;
            let s = guard
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            (
                s.cache.clone(),
                s.native
                    .as_ref()
                    .filter(|n| n.activated)
                    .map(|n| (n.transport.clone(), n.cancel.clone())),
                s.lease.profile().owner_id,
            )
        };
        let key = format!("artwork:{}", artwork.sha256);
        let cache_copy = cache.clone();
        let lookup = key.clone();
        let local = tokio::task::spawn_blocking(move || cache_copy.acquire(&lookup))
            .await
            .map_err(|_| ApiError::new(ErrorCode::Internal))??;
        let lease = match local {
            Some(lease) => lease,
            None => {
                let source = artwork
                    .source
                    .as_ref()
                    .ok_or_else(|| ApiError::new(ErrorCode::MediaSourceMissing))?;
                if source.chat_id != reader.binding().chat_id {
                    return Err(ApiError::new(ErrorCode::Integrity));
                }
                let (transport, lifetime) =
                    transport.ok_or_else(|| ApiError::new(ErrorCode::MediaUnavailable))?;
                let transfer = async {
                    transport
                        .verify_channel(
                            source.chat_id,
                            owner.ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?,
                            cancel,
                        )
                        .await?;
                    let file = transport
                        .message(source.chat_id, source.message_id, cancel)
                        .await?
                        .and_then(|m| m.file)
                        .ok_or_else(|| ApiError::new(ErrorCode::MediaSourceMissing))?;
                    if file.size <= 0
                        || file.size > vault_media::MAX_ARTWORK_BYTES as i64
                        || source.size_bytes.is_some_and(|n| n != file.size as u64)
                    {
                        return Err(ApiError::new(ErrorCode::Integrity));
                    }
                    vault_media::download_to_cache(
                        &transport,
                        &file,
                        &cache,
                        vault_media::DownloadOptions {
                            cache_key: key,
                            expected_sha256: Some(artwork.sha256.clone()),
                            pinned: false,
                        },
                        cancel,
                        &|_, _| {},
                    )
                    .await
                    .map_err(ApiError::from)
                };
                tokio::select! {_=lifetime.cancelled()=>return Err(ApiError::new(ErrorCode::Cancelled)),result=transfer=>result?}
            }
        };
        let token = cancel.clone();
        tokio::task::spawn_blocking(move || {
            use std::io::Read;
            if token.is_cancelled() {
                return Err(ApiError::new(ErrorCode::Cancelled));
            }
            let mut bytes = Vec::new();
            std::fs::File::open(&lease.path)?
                .take(vault_media::MAX_ARTWORK_BYTES as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > vault_media::MAX_ARTWORK_BYTES
                || vault_snapshot::digest(&bytes) != artwork.sha256
            {
                return Err(ApiError::new(ErrorCode::Integrity));
            }
            Ok(Some(ArtworkData {
                mime_type: artwork.mime_type,
                sha256: artwork.sha256,
                bytes,
            }))
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))?
    }
}
