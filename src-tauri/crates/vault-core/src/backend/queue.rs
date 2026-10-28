use super::*;

#[derive(Default)]
pub(super) struct Queue {
    id: Uuid,
    tracks: Vec<Uuid>,
    position: Option<usize>,
    revision: u64,
    repeat: RepeatMode,
    shuffle: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct QueueInfo {
    pub queue_id: Uuid,
    pub total: usize,
    pub position: Option<usize>,
    pub current_track_id: Option<Uuid>,
    pub revision: u64,
    pub repeat: RepeatMode,
    pub shuffle: bool,
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
            revision: self.revision,
            repeat: self.repeat,
            shuffle: self.shuffle,
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
                ..Queue::default()
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
        if let Some(preferred) = track.preferred_source_id
            && let Some(index) = sources.iter().position(|id| *id == preferred)
        {
            sources.remove(index);
            sources.insert(0, preferred);
        }
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
                let next = next_position(&s.queue, forward, ended.is_some(), |id| {
                    state.tracks.contains_key(id)
                });
                s.queue.position = next;
                s.queue.revision = s.queue.revision.wrapping_add(1);
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

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RepeatMode {
    #[default]
    Off,
    Queue,
    Track,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QueueAction {
    Append { track_ids: Vec<Uuid>, next: bool },
    Remove { index: usize },
    Move { from: usize, to: usize },
    Jump { index: usize },
    Repeat { mode: RepeatMode },
    Shuffle { enabled: bool },
}
fn next_position(
    queue: &Queue,
    forward: bool,
    ended: bool,
    exists: impl Fn(&Uuid) -> bool,
) -> Option<usize> {
    let n = queue.tracks.len();
    if n == 0 {
        return None;
    }
    if ended
        && queue.repeat == RepeatMode::Track
        && let Some(i) = queue.position.filter(|i| exists(&queue.tracks[*i]))
    {
        return Some(i);
    }
    let current = queue.position.unwrap_or(if forward { n - 1 } else { 0 });
    for step in 1..=n {
        let candidate = if forward {
            current.checked_add(step)
        } else {
            current.checked_sub(step)
        };
        let index = match candidate.filter(|i| *i < n) {
            Some(i) => i,
            None if queue.repeat == RepeatMode::Queue => {
                if forward {
                    (current + step) % n
                } else {
                    (current + n - step) % n
                }
            }
            _ => return None,
        };
        if exists(&queue.tracks[index]) {
            return Some(index);
        }
    }
    None
}
impl Queue {
    fn edit(&mut self, action: QueueAction) -> Result<bool, ApiError> {
        let invalid = || ApiError::new(ErrorCode::InvalidInput);
        let mut restart = false;
        match action {
            QueueAction::Append { track_ids, next } => {
                if self.tracks.len().saturating_add(track_ids.len()) > 100_000 {
                    return Err(invalid());
                }
                let at = if next {
                    self.position.map_or(self.tracks.len(), |p| p + 1)
                } else {
                    self.tracks.len()
                };
                self.tracks.splice(at..at, track_ids);
                if self.position.is_none() && !self.tracks.is_empty() {
                    self.position = Some(0);
                }
            }
            QueueAction::Remove { index } => {
                if index >= self.tracks.len() {
                    return Err(invalid());
                }
                self.tracks.remove(index);
                if let Some(position) = self.position {
                    if index < position {
                        self.position = Some(position - 1);
                    } else if index == position {
                        self.position = if self.tracks.is_empty() {
                            None
                        } else {
                            Some(position.min(self.tracks.len() - 1))
                        };
                        restart = true;
                    }
                }
            }
            QueueAction::Move { from, to } => {
                if from >= self.tracks.len() || to >= self.tracks.len() {
                    return Err(invalid());
                }
                let track = self.tracks.remove(from);
                self.tracks.insert(to, track);
                self.position = self.position.map(|p| {
                    if p == from {
                        to
                    } else if from < p && p <= to {
                        p - 1
                    } else if to <= p && p < from {
                        p + 1
                    } else {
                        p
                    }
                });
            }
            QueueAction::Jump { index } => {
                if index >= self.tracks.len() {
                    return Err(invalid());
                }
                self.position = Some(index);
                restart = true;
            }
            QueueAction::Repeat { mode } => self.repeat = mode,
            QueueAction::Shuffle { enabled } => {
                if enabled && !self.shuffle {
                    let start = self.position.map_or(0, |p| p + 1);
                    // Random keys shuffle only upcoming items; current playback is untouched.
                    let mut tail: Vec<_> = self
                        .tracks
                        .drain(start..)
                        .map(|id| (Uuid::new_v4(), id))
                        .collect();
                    tail.sort_unstable();
                    self.tracks.extend(tail.into_iter().map(|(_, id)| id));
                }
                self.shuffle = enabled;
            }
        }
        self.revision = self.revision.wrapping_add(1);
        Ok(restart)
    }
}
impl Backend {
    pub async fn edit_queue(
        &self,
        session_id: Uuid,
        queue_id: Uuid,
        revision: u64,
        action: QueueAction,
    ) -> Result<QueueInfo, ApiError> {
        if let QueueAction::Append { track_ids, .. } = &action {
            if track_ids.len() > 100_000 {
                return Err(ApiError::new(ErrorCode::InvalidInput));
            }
            let state = self.local_reader(session_id).await?.state().await?;
            if track_ids.iter().any(|id| !state.tracks.contains_key(id)) {
                return Err(ApiError::new(ErrorCode::InvalidInput));
            }
        }
        let (info, context, restart) = {
            let mut guard = self.session.lock().await;
            let s = guard
                .as_mut()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            if s.queue.id != queue_id || s.queue.revision != revision {
                return Err(ApiError::new(ErrorCode::Conflict));
            }
            let restart = s.queue.edit(action)?;
            if restart {
                s.playback_cancel.cancel();
                s.playback_id = None;
                if let Some(playback) = &s.playback {
                    playback.stop()?;
                }
            }
            (s.queue.info(), s.context.clone(), restart)
        };
        self.events.emit(&context, EventData::Queue(info.clone()));
        if restart && info.current_track_id.is_some() {
            self.play_queue_item(session_id, info.queue_id).await?;
        }
        Ok(info)
    }
}
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn reorder_preserves_current_occurrence_and_rejects_stale_indices() {
        let ids = vec![Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4()];
        let mut queue = Queue {
            tracks: ids.clone(),
            position: Some(1),
            ..Queue::default()
        };
        assert!(!queue.edit(QueueAction::Move { from: 0, to: 2 }).unwrap());
        assert_eq!(queue.info().current_track_id, Some(ids[1]));
        assert_eq!(queue.position, Some(0));
        assert!(queue.edit(QueueAction::Remove { index: 9 }).is_err());
        assert!(queue.edit(QueueAction::Remove { index: 0 }).unwrap());
        assert_eq!(queue.info().current_track_id, Some(ids[2]));
    }
    #[test]
    fn repeat_skips_deleted_and_track_repeat_only_applies_at_end() {
        let ids = vec![Uuid::new_v4(), Uuid::new_v4()];
        let mut q = Queue {
            tracks: ids.clone(),
            position: Some(1),
            repeat: RepeatMode::Queue,
            ..Queue::default()
        };
        assert_eq!(next_position(&q, true, true, |_| true), Some(0));
        assert_eq!(next_position(&q, true, true, |_| false), None);
        q.repeat = RepeatMode::Track;
        assert_eq!(next_position(&q, true, true, |_| true), Some(1));
        assert_eq!(next_position(&q, true, false, |_| true), None);
        q.position = Some(0);
        q.repeat = RepeatMode::Off;
        assert_eq!(next_position(&q, false, false, |_| true), None);
    }
}
