use super::*;

#[derive(Default)]
pub(super) struct Queue {
    id: Uuid,
    tracks: Vec<Uuid>,
    position: Option<usize>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QueueInfo {
    pub queue_id: Uuid,
    pub total: usize,
    pub position: Option<usize>,
    pub current_track_id: Option<Uuid>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QueuePage {
    pub info: QueueInfo,
    pub offset: u32,
    pub track_ids: Vec<Uuid>,
}
impl Queue {
    fn info(&self) -> QueueInfo {
        QueueInfo {
            queue_id: self.id,
            total: self.tracks.len(),
            position: self.position,
            current_track_id: self.position.and_then(|i| self.tracks.get(i)).copied(),
        }
    }
}
impl Backend {
    /// Capture an immutable order. Remote playlist edits do not reorder this
    /// queue; deleted tracks are skipped when navigation selects the next item.
    pub async fn set_queue(
        &self,
        session_id: Uuid,
        tracks: Vec<Uuid>,
        position: u32,
        autoplay: bool,
    ) -> Result<QueueInfo, ApiError> {
        if tracks.len() > 100_000
            || (tracks.is_empty() && position != 0)
            || (!tracks.is_empty() && position as usize >= tracks.len())
        {
            return Err(ApiError::new(ErrorCode::InvalidInput));
        }
        let state = self.local_reader(session_id).await?.state().await?;
        if tracks.iter().any(|id| !state.tracks.contains_key(id)) {
            return Err(ApiError::new(ErrorCode::InvalidInput));
        }
        let (info, context) = {
            let mut guard = self.session.lock().await;
            let s = guard
                .as_mut()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            s.playback_cancel.cancel();
            s.playback_id = None;
            if let Some(playback) = &s.playback {
                playback.stop()?;
            }
            s.queue = Queue {
                id: Uuid::new_v4(),
                position: (!tracks.is_empty()).then_some(position as usize),
                tracks,
            };
            (s.queue.info(), s.context.clone())
        };
        self.events.emit(&context, EventData::Queue(info.clone()));
        if autoplay && info.current_track_id.is_some() {
            self.play_queue_item(session_id, info.queue_id).await?;
        }
        Ok(info)
    }
    pub async fn queue_playlist(
        &self,
        session_id: Uuid,
        playlist_id: Uuid,
        position: u32,
        autoplay: bool,
    ) -> Result<QueueInfo, ApiError> {
        let state = self.local_reader(session_id).await?.state().await?;
        if !state.playlists.contains_key(&playlist_id) {
            return Err(ApiError::new(ErrorCode::InvalidInput));
        }
        let tracks = state
            .playlist_order(playlist_id)
            .map_err(|_| ApiError::new(ErrorCode::Integrity))?
            .into_iter()
            .map(|id| {
                state
                    .entries
                    .get(&id)
                    .map(|entry| entry.track_id)
                    .ok_or_else(|| ApiError::new(ErrorCode::Integrity))
            })
            .collect::<Result<Vec<_>, _>>()?;
        self.set_queue(session_id, tracks, position, autoplay).await
    }
    pub async fn playback_queue(
        &self,
        session_id: Uuid,
        offset: u32,
        limit: u32,
    ) -> Result<QueuePage, ApiError> {
        let guard = self.session.lock().await;
        let s = guard
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        Ok(QueuePage {
            info: s.queue.info(),
            offset,
            track_ids: s
                .queue
                .tracks
                .iter()
                .skip(offset as usize)
                .take(limit.clamp(1, 500) as usize)
                .copied()
                .collect(),
        })
    }
    pub(super) async fn begin_playback(
        &self,
        session_id: Uuid,
        queue_id: Uuid,
    ) -> Result<(Uuid, CancellationToken), ApiError> {
        let mut guard = self.session.lock().await;
        let s = guard
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        if s.queue.id != queue_id {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        s.playback_cancel.cancel();
        s.playback_cancel = CancellationToken::new();
        s.playback_ticket = Uuid::new_v4();
        s.playback_id = None;
        Ok((s.playback_ticket, s.playback_cancel.clone()))
    }
    async fn play_queue_item(&self, session_id: Uuid, queue_id: Uuid) -> Result<(), ApiError> {
        let (track_id, ticket, cancel, reader, cache) = {
            let mut guard = self.session.lock().await;
            let s = guard
                .as_mut()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            if s.queue.id != queue_id {
                return Err(ApiError::new(ErrorCode::Conflict));
            }
            let track = s
                .queue
                .info()
                .current_track_id
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            s.playback_cancel.cancel();
            s.playback_cancel = CancellationToken::new();
            s.playback_ticket = Uuid::new_v4();
            s.playback_id = None;
            (
                track,
                s.playback_ticket,
                s.playback_cancel.clone(),
                s.reader
                    .clone()
                    .ok_or_else(|| ApiError::new(ErrorCode::NoVault))?,
                s.cache.clone(),
            )
        };
        let state = reader.state().await?;
        let track = state
            .tracks
            .get(&track_id)
            .ok_or_else(|| ApiError::new(ErrorCode::MediaSourceMissing))?;
        let mut sources = track.sources.keys().copied().collect::<Vec<_>>();
        // Prefer any complete local copy before trying the network, then try
        // alternate registered sources when a message is unavailable.
        sources = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
            let mut cached = Vec::new();
            let mut remote = Vec::new();
            for id in sources {
                if cache.acquire(&media_cache_key(track_id, id))?.is_some() {
                    cached.push(id);
                } else {
                    remote.push(id);
                }
            }
            cached.extend(remote);
            Ok(cached)
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))??;
        let mut last = ApiError::new(ErrorCode::MediaSourceMissing);
        for source in sources {
            match self
                .play_track_inner(session_id, track_id, source, ticket, &cancel)
                .await
            {
                Ok(()) => return Ok(()),
                Err(error)
                    if matches!(
                        error.code,
                        ErrorCode::MediaSourceMissing | ErrorCode::MediaUnavailable
                    ) =>
                {
                    last = error
                }
                Err(error) => return Err(error),
            }
        }
        Err(last)
    }
    // Explicit future type breaks the watcher -> advance -> player -> watcher
    // construction cycle while preserving Send on the background observer.
    pub(super) fn advance_queue(
        &self,
        session_id: Uuid,
        forward: bool,
        ended: Option<Uuid>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<QueueInfo, ApiError>> + Send + '_>>
    {
        Box::pin(async move {
            let reader = self.local_reader(session_id).await?;
            let state = reader.state().await?;
            let (info, context) = {
                let mut guard = self.session.lock().await;
                let s = guard
                    .as_mut()
                    .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
                check_session(s, session_id)?;
                if ended.is_some() && s.playback_id != ended {
                    return Err(ApiError::new(ErrorCode::Conflict));
                }
                let start = if forward {
                    s.queue.position.and_then(|i| i.checked_add(1))
                } else {
                    s.queue
                        .position
                        .map(|i| i.saturating_sub(1))
                        .or_else(|| s.queue.tracks.len().checked_sub(1))
                };
                let next = start.and_then(|start| {
                    if forward {
                        (start..s.queue.tracks.len())
                            .find(|i| state.tracks.contains_key(&s.queue.tracks[*i]))
                    } else {
                        (0..=start)
                            .rev()
                            .find(|i| state.tracks.contains_key(&s.queue.tracks[*i]))
                    }
                });
                s.queue.position = next;
                s.playback_id = None;
                s.playback_cancel.cancel();
                if next.is_none()
                    && let Some(playback) = &s.playback
                {
                    playback.stop()?;
                }
                (s.queue.info(), s.context.clone())
            };
            self.events.emit(&context, EventData::Queue(info.clone()));
            if info.current_track_id.is_some() {
                self.play_queue_item(session_id, info.queue_id).await?;
            }
            Ok(info)
        })
    }
}
