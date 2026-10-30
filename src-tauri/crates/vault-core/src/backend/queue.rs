use super::*;

#[derive(Default)]
pub(super) struct Queue {
    id: Uuid,
    tracks: Vec<Uuid>,
    // Stable occurrence ranks restore the remaining sequence, including duplicates.
    order: Vec<usize>,
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
    pub(super) fn info(&self) -> QueueInfo {
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
#[derive(Default, Serialize, Deserialize)]
struct PlaybackPreferences {
    repeat: RepeatMode,
    shuffle: bool,
}
#[derive(Serialize, Deserialize)]
struct QueueCheckpoint {
    tracks: Vec<Uuid>,
    order: Vec<usize>,
    position: Option<usize>,
}
fn persist_local(lease: &ProfileLease, name: &str, value: &impl Serialize) -> Result<(), ApiError> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(lease.directory())?;
    serde_json::to_writer(&mut file, value).map_err(|_| ApiError::new(ErrorCode::Internal))?;
    file.flush()?;
    file.as_file().sync_all()?;
    file.persist(lease.directory().join(name))
        .map_err(|e| ApiError::from(e.error))?;
    #[cfg(unix)]
    std::fs::File::open(lease.directory())?.sync_all()?;
    Ok(())
}
impl PlaybackPreferences {
    fn save(&self, lease: &ProfileLease) -> Result<(), ApiError> {
        persist_local(lease, "playback-preferences.v1.json", self)
    }
}
impl Queue {
    fn save_checkpoint(&self, lease: &ProfileLease) -> Result<(), ApiError> {
        persist_local(
            lease,
            "playback-queue.v1.json",
            &QueueCheckpoint {
                tracks: self.tracks.clone(),
                order: self.order.clone(),
                position: self.position,
            },
        )
    }
    fn checkpoint(&self, lease: &ProfileLease) {
        // Local listening history is independent of the vault protocol. A full
        // disk must not interrupt audio or invalidate a confirmed queue edit.
        if self.save_checkpoint(lease).is_err() {
            tracing::warn!("could not save local playback queue");
        }
    }
    pub(super) fn load_preferences(lease: &ProfileLease) -> Self {
        let path = lease.directory().join("playback-preferences.v1.json");
        // Optional preferences must never prevent opening a valid library.
        let preferences = std::fs::symlink_metadata(&path)
            .ok()
            .filter(|m| m.is_file() && !m.file_type().is_symlink() && m.len() <= 4096)
            .and_then(|_| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<PlaybackPreferences>(&bytes).ok())
            .unwrap_or_default();
        let mut queue = Self {
            id: Uuid::new_v4(),
            repeat: preferences.repeat,
            shuffle: preferences.shuffle,
            ..Self::default()
        };
        let path = lease.directory().join("playback-queue.v1.json");
        let checkpoint = std::fs::symlink_metadata(&path)
            .ok()
            .filter(|m| m.is_file() && !m.file_type().is_symlink() && m.len() <= 8 * 1024 * 1024)
            .and_then(|_| std::fs::read(path).ok())
            .and_then(|bytes| serde_json::from_slice::<QueueCheckpoint>(&bytes).ok());
        if let Some(saved) = checkpoint {
            let count = saved.tracks.len();
            let valid_position = saved.position.is_none_or(|p| p < count);
            if count <= 100_000 && valid_position && saved.order.len() == count {
                let mut ranks = saved.order.clone();
                ranks.sort_unstable();
                if ranks.iter().copied().eq(0..count) {
                    queue.tracks = saved.tracks;
                    queue.order = saved.order;
                    queue.position = saved.position;
                }
            }
        }
        queue
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
            let (repeat, shuffle) = (s.queue.repeat, s.queue.shuffle);
            s.queue = Queue {
                id: Uuid::new_v4(),
                position: (!tracks.is_empty()).then_some(position as usize),
                order: (0..tracks.len()).collect(),
                tracks,
                repeat,
                ..Queue::default()
            };
            if shuffle {
                s.queue.edit(QueueAction::Shuffle { enabled: true })?;
            }
            s.queue.checkpoint(&s.lease);
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
        if let Some(preferred) = track.preferred_source_id
            && let Some(index) = sources.iter().position(|id| *id == preferred)
        {
            sources.remove(index);
            sources.insert(0, preferred);
        }
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
                let next = next_position(&s.queue, forward, ended.is_some(), |id| {
                    state.tracks.contains_key(id)
                });
                s.queue.position = next;
                s.queue.revision = s.queue.revision.wrapping_add(1);
                s.queue.checkpoint(&s.lease);
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
    let Some(current) = queue.position else {
        return if forward {
            (0..n).find(|i| exists(&queue.tracks[*i]))
        } else {
            (0..n).rev().find(|i| exists(&queue.tracks[*i]))
        };
    };
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
        if self.order.len() != self.tracks.len() {
            self.order = (0..self.tracks.len()).collect();
        }
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
                let rank = if next {
                    self.position
                        .map_or(self.tracks.len(), |p| self.order[p] + 1)
                } else {
                    self.tracks.len()
                };
                for old in &mut self.order {
                    if *old >= rank {
                        *old += track_ids.len();
                    }
                }
                self.order.splice(at..at, rank..rank + track_ids.len());
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
                let removed = self.order.remove(index);
                for rank in &mut self.order {
                    if *rank > removed {
                        *rank -= 1;
                    }
                }
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
                // An explicit user reorder becomes the new sequential order.
                self.order = (0..self.tracks.len()).collect();
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
                let start = self.position.map_or(0, |p| p + 1);
                if enabled != self.shuffle {
                    let mut tail: Vec<_> = self
                        .tracks
                        .drain(start..)
                        .zip(self.order.drain(start..))
                        .map(|(id, rank)| (Uuid::new_v4(), rank, id))
                        .collect();
                    if enabled {
                        tail.sort_unstable_by_key(|(random, _, _)| *random);
                    } else {
                        tail.sort_unstable_by_key(|(_, rank, _)| *rank);
                    }
                    for (_, rank, id) in tail {
                        self.tracks.push(id);
                        self.order.push(rank);
                    }
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
            if matches!(
                action,
                QueueAction::Repeat { .. } | QueueAction::Shuffle { .. }
            ) {
                let preferences = PlaybackPreferences {
                    repeat: if let QueueAction::Repeat { mode } = action {
                        mode
                    } else {
                        s.queue.repeat
                    },
                    shuffle: if let QueueAction::Shuffle { enabled } = action {
                        enabled
                    } else {
                        s.queue.shuffle
                    },
                };
                preferences.save(&s.lease)?;
            }
            let restart = s.queue.edit(action)?;
            s.queue.checkpoint(&s.lease);
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
    fn checkpoint_restores_occurrences_and_history_without_starting_audio() {
        let temp = tempfile::tempdir().unwrap();
        let profiles = ProfileStore::at(temp.path()).unwrap();
        let profile = profiles.create("Listening".into()).unwrap();
        let lease = profiles.acquire(profile.profile_id).unwrap();
        let duplicate = Uuid::new_v4();
        let mut queue = Queue {
            id: Uuid::new_v4(),
            tracks: vec![duplicate, Uuid::new_v4(), duplicate, Uuid::new_v4()],
            position: Some(1),
            ..Queue::default()
        };
        queue.edit(QueueAction::Shuffle { enabled: true }).unwrap();
        PlaybackPreferences {
            repeat: RepeatMode::Queue,
            shuffle: true,
        }
        .save(&lease)
        .unwrap();
        queue.save_checkpoint(&lease).unwrap();
        let mut restored = Queue::load_preferences(&lease);
        assert_eq!(restored.tracks, queue.tracks);
        assert_eq!(restored.position, queue.position);
        assert_eq!(restored.repeat, RepeatMode::Queue);
        assert!(restored.shuffle);
        assert_ne!(restored.id, queue.id);
        restored
            .edit(QueueAction::Shuffle { enabled: false })
            .unwrap();
        assert_eq!(restored.tracks[0], duplicate);
        assert_eq!(restored.tracks[2], duplicate);
        // Invalid occurrence indexes must not prevent opening the profile.
        std::fs::write(
            lease.directory().join("playback-queue.v1.json"),
            br#"{"tracks":[],"order":[],"position":9}"#,
        )
        .unwrap();
        assert!(Queue::load_preferences(&lease).tracks.is_empty());
        std::fs::write(
            lease.directory().join("playback-preferences.v1.json"),
            b"interrupted",
        )
        .unwrap();
        assert_eq!(Queue::load_preferences(&lease).repeat, RepeatMode::Off);
    }
    #[test]
    fn shuffle_roundtrip_preserves_history_and_duplicate_occurrences() {
        let mut ids: Vec<_> = (0..20).map(|_| Uuid::new_v4()).collect();
        ids[7] = ids[6];
        let mut q = Queue {
            tracks: ids.clone(),
            position: Some(3),
            ..Queue::default()
        };
        q.edit(QueueAction::Shuffle { enabled: true }).unwrap();
        assert_eq!(&q.tracks[..4], &ids[..4]);
        let mut sorted = q.tracks.clone();
        sorted.sort();
        let mut original = ids.clone();
        original.sort();
        assert_eq!(sorted, original);
        q.edit(QueueAction::Shuffle { enabled: false }).unwrap();
        assert_eq!(q.tracks, ids);
        assert_eq!(q.position, Some(3));
    }
    #[test]
    fn toggling_after_navigation_preserves_history_and_restores_only_unplayed_occurrences() {
        let ids: Vec<_> = (0..24).map(|_| Uuid::new_v4()).collect();
        let mut q = Queue {
            tracks: ids,
            position: Some(2),
            ..Queue::default()
        };
        q.edit(QueueAction::Shuffle { enabled: true }).unwrap();
        q.position = Some(9);
        let history = q.tracks[..10].to_vec();
        q.edit(QueueAction::Remove { index: 15 }).unwrap();
        q.edit(QueueAction::Append {
            track_ids: vec![Uuid::new_v4()],
            next: true,
        })
        .unwrap();
        q.edit(QueueAction::Shuffle { enabled: false }).unwrap();
        assert_eq!(&q.tracks[..10], history);
        assert!(q.order[10..].windows(2).all(|pair| pair[0] < pair[1]));
        let ranks: std::collections::BTreeSet<_> = q.order.iter().copied().collect();
        assert_eq!(ranks.len(), q.tracks.len());
        assert_eq!(ranks.last(), Some(&(q.tracks.len() - 1)));
    }
    #[test]
    fn empty_and_finished_queue_can_start_again() {
        let mut q = Queue::default();
        assert_eq!(next_position(&q, true, false, |_| true), None);
        q.tracks = vec![Uuid::new_v4(), Uuid::new_v4()];
        assert_eq!(next_position(&q, true, false, |_| true), Some(0));
    }
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
