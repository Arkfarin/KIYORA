//! Device-local history. It never participates in the shared event protocol.
use crate::*;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListeningEntry {
    pub track_id: Uuid,
    pub last_playback_id: Uuid,
    pub completed_plays: u64,
    pub last_played_ms: u64,
}
impl Database {
    fn listening_key(&self) -> String {
        format!("listening.v1:{}", self.binding.vault_id)
    }
    pub fn listening_history(&self) -> Result<Vec<ListeningEntry>, DbError> {
        let Some(raw) = self.local_setting(&self.listening_key())? else {
            return Ok(Vec::new());
        };
        if raw.len() > 512 * 1024 {
            return Err(DbError::Corrupt);
        }
        let entries: Vec<ListeningEntry> = serde_json::from_str(&raw)?;
        if entries.len() > 1000 {
            return Err(DbError::Corrupt);
        }
        Ok(entries)
    }
    pub fn record_completed_play(
        &mut self,
        track_id: Uuid,
        playback_id: Uuid,
        at_ms: u64,
    ) -> Result<(), DbError> {
        if track_id.is_nil() || playback_id.is_nil() {
            return Err(DbError::Schema);
        }
        let mut entries = self.listening_history()?;
        if entries.iter().any(|e| e.last_playback_id == playback_id) {
            return Ok(());
        }
        let plays = entries
            .iter()
            .find(|e| e.track_id == track_id)
            .map_or(1, |e| e.completed_plays.saturating_add(1));
        entries.retain(|e| e.track_id != track_id);
        entries.insert(
            0,
            ListeningEntry {
                track_id,
                last_playback_id: playback_id,
                completed_plays: plays,
                last_played_ms: at_ms,
            },
        );
        entries.truncate(1000);
        self.set_local_setting(&self.listening_key(), &serde_json::to_string(&entries)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completed_play_is_idempotent_and_survives_reopen() -> Result<(), Box<dyn std::error::Error>>
    {
        let dir = tempfile::tempdir()?;
        let binding = Binding {
            vault_id: Uuid::new_v4(),
            owner_id: UserId::new(42)?,
            chat_id: ChatId::new(-100)?,
        };
        let path = dir.path().join("history.sqlite");
        let track = Uuid::new_v4();
        let playback = Uuid::new_v4();
        let mut db = Database::open(&path, binding)?;
        db.record_completed_play(track, playback, 100)?;
        db.record_completed_play(track, playback, 200)?;
        db.record_completed_play(track, Uuid::new_v4(), 300)?;
        drop(db);
        let entries = Database::open(path, binding)?.listening_history()?;
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].completed_plays, 2);
        assert_eq!(entries[0].last_played_ms, 300);
        Ok(())
    }
}
