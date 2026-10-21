use crate::*;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DomainError {
    #[error("event belongs to another vault")]
    WrongVault,
    #[error("event is not in increasing server order")]
    OutOfOrder,
    #[error("op_id was reused with different content")]
    OperationCollision,
    #[error("invalid state: {0}")]
    InvalidState(&'static str),
    #[error("event cannot be encoded")]
    Encoding,
}

pub fn event_digest(event: &Event) -> Result<String, DomainError> {
    let bytes = serde_json::to_vec(event).map_err(|_| DomainError::Encoding)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

pub fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn valid_text(value: &str, max: usize, required: bool) -> bool {
    value.len() <= max && (!required || !value.trim().is_empty()) && !value.contains('\0')
}

fn valid_source(source: &Source) -> bool {
    !source.source_id.is_nil()
        && source.sha256.as_deref().is_none_or(valid_digest)
        && source.size_bytes.is_none_or(|n| n <= i64::MAX as u64)
}

fn valid_metadata(metadata: &Metadata) -> bool {
    valid_text(&metadata.title, 4096, true)
        && [
            &metadata.performer,
            &metadata.album,
            &metadata.album_artist,
            &metadata.genre,
            &metadata.codec,
        ]
        .iter()
        .all(|s| valid_text(s, 4096, false))
        && metadata.duration_ms <= i64::MAX as u64
        && metadata.artwork.as_ref().is_none_or(|a| {
            valid_digest(&a.sha256)
                && matches!(
                    a.mime_type.as_str(),
                    "image/jpeg" | "image/png" | "image/webp"
                )
                && a.source.as_ref().is_none_or(valid_source)
        })
}

impl State {
    /// Replay one final server event. A protocol/order error leaves state untouched.
    /// A duplicate returns its original outcome without changing any state field.
    pub fn apply(&mut self, message_id: MessageId, event: &Event) -> Result<Outcome, DomainError> {
        if event.vault_id != self.vault_id {
            return Err(DomainError::WrongVault);
        }
        if event.op_id.is_nil() {
            return Err(DomainError::InvalidState("nil operation UUID"));
        }
        let digest = event_digest(event)?;
        if let Some(receipt) = self.applied.get(&event.op_id) {
            return if receipt.event_sha256 == digest {
                Ok(receipt.outcome.clone())
            } else {
                Err(DomainError::OperationCollision)
            };
        }
        if self
            .last_event_message_id
            .is_some_and(|last| message_id <= last)
        {
            return Err(DomainError::OutOfOrder);
        }
        let outcome = if event.op_id.is_nil()
            || event.entity_id.is_nil()
            || !valid_text(&event.device_id, 128, true)
        {
            Outcome::Rejected(RejectReason::InvalidPayload)
        } else if event.base_generation > self.generation {
            Outcome::Rejected(RejectReason::FutureGeneration)
        } else {
            self.apply_action(message_id, event)
        };
        self.applied.insert(
            event.op_id,
            Receipt {
                message_id,
                action: event.action.name().into(),
                event_sha256: digest,
                outcome: outcome.clone(),
            },
        );
        self.last_event_message_id = Some(message_id);
        Ok(outcome)
    }

    fn apply_action(&mut self, mid: MessageId, e: &Event) -> Outcome {
        use Action::*;
        use RejectReason::*;
        let id = e.entity_id;
        let result: Result<Outcome, RejectReason> = (|| {
            match &e.action {
                TrackRegister { metadata, source } => {
                    if self.tombstones.tracks.contains(&id) {
                        return Err(Deleted);
                    }
                    if self.tracks.contains_key(&id) {
                        return Ok(Outcome::NoOp(NoOpReason::AlreadyExists));
                    }
                    if !valid_metadata(metadata) || !valid_source(source) {
                        return Err(InvalidPayload);
                    }
                    self.tracks.insert(
                        id,
                        Track {
                            track_id: id,
                            metadata: metadata.clone(),
                            sources: BTreeMap::from([(source.source_id, source.clone())]),
                            updated_order: mid,
                        },
                    );
                }
                TrackMetadataUpdate { patch } => {
                    self.require_track(id)?;
                    let Some(track) = self.tracks.get_mut(&id) else {
                        return Err(MissingEntity);
                    };
                    let mut metadata = track.metadata.clone();
                    patch.apply_to(&mut metadata);
                    if !valid_metadata(&metadata)
                        || (patch.clear_artwork && patch.artwork.is_some())
                    {
                        return Err(InvalidPayload);
                    }
                    track.metadata = metadata;
                    track.updated_order = mid;
                }
                TrackDelete {} => {
                    if !self.tombstones.tracks.insert(id) {
                        return Ok(Outcome::NoOp(NoOpReason::AlreadyDeleted));
                    }
                    self.tracks.remove(&id);
                    let entries: Vec<_> = self
                        .entries
                        .values()
                        .filter(|x| x.track_id == id)
                        .map(|x| x.entry_id)
                        .collect();
                    for entry in entries {
                        self.remove_entry(entry);
                    }
                    self.favorites.retain(|_, values| {
                        values.remove(&id);
                        !values.is_empty()
                    });
                }
                PlaylistCreate { title } => {
                    if self.tombstones.playlists.contains(&id) {
                        return Err(Deleted);
                    }
                    if self.playlists.contains_key(&id) {
                        return Ok(Outcome::NoOp(NoOpReason::AlreadyExists));
                    }
                    if !valid_text(title, 4096, true) {
                        return Err(InvalidPayload);
                    }
                    self.playlists.insert(
                        id,
                        Playlist {
                            playlist_id: id,
                            title: title.clone(),
                            created_order: mid,
                            updated_order: mid,
                        },
                    );
                }
                PlaylistRename { title } => {
                    self.require_playlist(id)?;
                    if !valid_text(title, 4096, true) {
                        return Err(InvalidPayload);
                    }
                    let Some(playlist) = self.playlists.get_mut(&id) else {
                        return Err(MissingEntity);
                    };
                    playlist.title.clone_from(title);
                    playlist.updated_order = mid;
                }
                PlaylistDelete {} => {
                    if !self.tombstones.playlists.insert(id) {
                        return Ok(Outcome::NoOp(NoOpReason::AlreadyDeleted));
                    }
                    self.playlists.remove(&id);
                    let ids: Vec<_> = self
                        .entries
                        .values()
                        .filter(|x| x.playlist_id == id)
                        .map(|x| x.entry_id)
                        .collect();
                    for entry in ids {
                        self.remove_entry(entry);
                    }
                }
                PlaylistTrackAdd {
                    track_id,
                    entry_id,
                    after_entry_id,
                } => {
                    self.require_playlist(id)?;
                    self.require_track(*track_id)?;
                    if entry_id.is_nil() {
                        return Err(InvalidPayload);
                    }
                    if self.tombstones.entries.contains(entry_id) {
                        return Err(Deleted);
                    }
                    if let Some(entry) = self.entries.get(entry_id) {
                        return if entry.playlist_id == id && entry.track_id == *track_id {
                            Ok(Outcome::NoOp(NoOpReason::AlreadyExists))
                        } else {
                            Err(WrongEntity)
                        };
                    }
                    self.require_anchor(id, *entry_id, *after_entry_id)?;
                    self.entries.insert(
                        *entry_id,
                        PlaylistEntry {
                            entry_id: *entry_id,
                            playlist_id: id,
                            track_id: *track_id,
                            after_entry_id: *after_entry_id,
                            order_key: mid,
                        },
                    );
                }
                PlaylistTrackRemove { entry_id } => {
                    self.require_playlist(id)?;
                    if self.tombstones.entries.contains(entry_id) {
                        return Ok(Outcome::NoOp(NoOpReason::AlreadyDeleted));
                    }
                    if entry_id.is_nil() {
                        return Err(InvalidPayload);
                    }
                    if self
                        .entries
                        .get(entry_id)
                        .is_some_and(|x| x.playlist_id != id)
                    {
                        return Err(WrongEntity);
                    }
                    self.remove_entry(*entry_id);
                }
                PlaylistEntryMove {
                    entry_id,
                    after_entry_id,
                } => {
                    self.require_playlist(id)?;
                    if self.tombstones.entries.contains(entry_id) {
                        return Err(Deleted);
                    }
                    let entry = self.entries.get(entry_id).ok_or(MissingEntity)?;
                    if entry.playlist_id != id {
                        return Err(WrongEntity);
                    }
                    self.require_anchor(id, *entry_id, *after_entry_id)?;
                    let old_anchor = entry.after_entry_id;
                    for child in self
                        .entries
                        .values_mut()
                        .filter(|x| x.after_entry_id == Some(*entry_id))
                    {
                        child.after_entry_id = old_anchor;
                    }
                    let Some(entry) = self.entries.get_mut(entry_id) else {
                        return Err(MissingEntity);
                    };
                    entry.after_entry_id = *after_entry_id;
                    entry.order_key = mid;
                }
                FavoriteSet { set_name, value } => {
                    self.require_track(id)?;
                    if !valid_text(set_name, 128, true) {
                        return Err(InvalidPayload);
                    }
                    if *value {
                        self.favorites
                            .entry(set_name.clone())
                            .or_default()
                            .insert(id);
                    } else if let Some(set) = self.favorites.get_mut(set_name) {
                        set.remove(&id);
                        if set.is_empty() {
                            self.favorites.remove(set_name);
                        }
                    }
                }
                SourceRelink { source } => {
                    self.require_track(id)?;
                    if !valid_source(source) {
                        return Err(InvalidPayload);
                    }
                    let Some(track) = self.tracks.get_mut(&id) else {
                        return Err(MissingEntity);
                    };
                    track.sources.insert(source.source_id, source.clone());
                    track.updated_order = mid;
                }
                VaultSettingsUpdate { settings } => {
                    if id != self.vault_id {
                        return Err(WrongEntity);
                    }
                    if !valid_text(&settings.display_name, 4096, false)
                        || settings
                            .gc_device_id
                            .as_ref()
                            .is_some_and(|s| !valid_text(s, 128, true))
                    {
                        return Err(InvalidPayload);
                    }
                    self.settings = settings.clone();
                }
            }
            Ok(Outcome::Applied)
        })();
        result.unwrap_or_else(Outcome::Rejected)
    }

    fn require_track(&self, id: Uuid) -> Result<(), RejectReason> {
        if self.tombstones.tracks.contains(&id) {
            Err(RejectReason::Deleted)
        } else if !self.tracks.contains_key(&id) {
            Err(RejectReason::MissingEntity)
        } else {
            Ok(())
        }
    }
    fn require_playlist(&self, id: Uuid) -> Result<(), RejectReason> {
        if self.tombstones.playlists.contains(&id) {
            Err(RejectReason::Deleted)
        } else if !self.playlists.contains_key(&id) {
            Err(RejectReason::MissingEntity)
        } else {
            Ok(())
        }
    }
    fn require_anchor(
        &self,
        playlist: Uuid,
        entry: Uuid,
        anchor: Option<Uuid>,
    ) -> Result<(), RejectReason> {
        if let Some(anchor) = anchor
            && (anchor == entry
                || !self
                    .entries
                    .get(&anchor)
                    .is_some_and(|a| a.playlist_id == playlist))
        {
            return Err(RejectReason::InvalidAnchor);
        }
        Ok(())
    }
    fn remove_entry(&mut self, id: Uuid) {
        if let Some(entry) = self.entries.remove(&id) {
            for child in self
                .entries
                .values_mut()
                .filter(|x| x.after_entry_id == Some(id))
            {
                child.after_entry_id = entry.after_entry_id;
            }
        }
        self.tombstones.entries.insert(id);
    }

    /// Iterative forest traversal; malformed snapshots cannot exhaust the stack.
    pub fn playlist_order(&self, playlist: Uuid) -> Result<Vec<Uuid>, DomainError> {
        let mut children: BTreeMap<Option<Uuid>, Vec<&PlaylistEntry>> = BTreeMap::new();
        let mut count = 0;
        for entry in self.entries.values().filter(|e| e.playlist_id == playlist) {
            children
                .entry(entry.after_entry_id)
                .or_default()
                .push(entry);
            count += 1;
        }
        for siblings in children.values_mut() {
            siblings.sort_by_key(|e| (e.order_key, e.entry_id));
        }
        let mut stack: Vec<_> = children
            .get(&None)
            .into_iter()
            .flatten()
            .rev()
            .copied()
            .collect();
        let mut visited = BTreeSet::new();
        let mut ordered = Vec::with_capacity(count);
        while let Some(entry) = stack.pop() {
            if !visited.insert(entry.entry_id) {
                return Err(DomainError::InvalidState("cyclic entry order"));
            }
            ordered.push(entry.entry_id);
            if let Some(next) = children.get(&Some(entry.entry_id)) {
                stack.extend(next.iter().rev().copied());
            }
        }
        if ordered.len() != count {
            return Err(DomainError::InvalidState("unreachable entry"));
        }
        Ok(ordered)
    }

    pub fn validate(&self) -> Result<(), DomainError> {
        let invalid = DomainError::InvalidState;
        if self.vault_id.is_nil() {
            return Err(invalid("nil vault"));
        }
        if self.generation > i64::MAX as u64
            || !valid_text(&self.settings.display_name, 4096, false)
            || self
                .settings
                .gc_device_id
                .as_ref()
                .is_some_and(|s| !valid_text(s, 128, true))
        {
            return Err(invalid("generation or shared settings"));
        }
        for (id, track) in &self.tracks {
            if id.is_nil()
                || *id != track.track_id
                || self.tombstones.tracks.contains(id)
                || !valid_metadata(&track.metadata)
                || track.sources.is_empty()
            {
                return Err(invalid("track"));
            }
            for (source_id, source) in &track.sources {
                if *source_id != source.source_id || !valid_source(source) {
                    return Err(invalid("source"));
                }
            }
        }
        for (id, playlist) in &self.playlists {
            if id.is_nil()
                || *id != playlist.playlist_id
                || self.tombstones.playlists.contains(id)
                || !valid_text(&playlist.title, 4096, true)
            {
                return Err(invalid("playlist"));
            }
            self.playlist_order(*id)?;
        }
        for (id, entry) in &self.entries {
            if id.is_nil()
                || *id != entry.entry_id
                || self.tombstones.entries.contains(id)
                || !self.tracks.contains_key(&entry.track_id)
                || !self.playlists.contains_key(&entry.playlist_id)
            {
                return Err(invalid("entry"));
            }
            if self
                .require_anchor(entry.playlist_id, *id, entry.after_entry_id)
                .is_err()
            {
                return Err(invalid("anchor"));
            }
        }
        for (name, set) in &self.favorites {
            if !valid_text(name, 128, true)
                || set.is_empty()
                || set.iter().any(|id| !self.tracks.contains_key(id))
            {
                return Err(invalid("favorite"));
            }
        }
        if self.tombstones.tracks.contains(&Uuid::nil())
            || self.tombstones.playlists.contains(&Uuid::nil())
            || self.tombstones.entries.contains(&Uuid::nil())
        {
            return Err(invalid("nil tombstone"));
        }
        let mut message_ids = BTreeSet::new();
        for (op_id, receipt) in &self.applied {
            if op_id.is_nil()
                || !valid_digest(&receipt.event_sha256)
                || !message_ids.insert(receipt.message_id)
                || !self
                    .last_event_message_id
                    .is_some_and(|last| receipt.message_id <= last)
            {
                return Err(invalid("receipt"));
            }
        }
        if self.applied.values().map(|r| r.message_id).max() != self.last_event_message_id {
            return Err(invalid("receipt boundary"));
        }
        let within_boundary = |id| self.last_event_message_id.is_some_and(|last| id <= last);
        if self
            .tracks
            .values()
            .any(|t| !within_boundary(t.updated_order))
            || self
                .playlists
                .values()
                .any(|p| p.created_order > p.updated_order || !within_boundary(p.updated_order))
            || self.entries.values().any(|e| !within_boundary(e.order_key))
        {
            return Err(invalid("object order beyond replay boundary"));
        }
        Ok(())
    }
}
