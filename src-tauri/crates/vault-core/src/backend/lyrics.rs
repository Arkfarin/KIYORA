use super::*;
impl Backend {
    pub async fn lyrics(
        &self,
        session_id: Uuid,
        track_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<Option<vault_domain::LyricsDocument>, ApiError> {
        let reader = self.local_reader(session_id).await?;
        let state = reader.state().await?;
        let track = state
            .tracks
            .get(&track_id)
            .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
        let Some(artwork) = track.metadata.lyrics.clone() else {
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
        let key = format!("lyrics:{}", artwork.sha256);
        let cache_copy = cache.clone();
        let lookup = key.clone();
        let local = tokio::task::spawn_blocking(move || cache_copy.acquire(&lookup))
            .await
            .map_err(|_| ApiError::new(ErrorCode::Internal))??;
        let lease = match local {
            Some(lease) => lease,
            None => {
                let source = &artwork.source;
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
                        || file.size > (512 * 1024) as i64
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
                .take((512 * 1024) as u64 + 1)
                .read_to_end(&mut bytes)?;
            if bytes.len() > (512 * 1024) || vault_snapshot::digest(&bytes) != artwork.sha256 {
                return Err(ApiError::new(ErrorCode::Integrity));
            }
            let document: vault_domain::LyricsDocument =
                serde_json::from_slice(&bytes).map_err(|_| ApiError::new(ErrorCode::Integrity))?;
            if !document.valid() {
                return Err(ApiError::new(ErrorCode::Integrity));
            }
            Ok(Some(document))
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))?
    }
}
