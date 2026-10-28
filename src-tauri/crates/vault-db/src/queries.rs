use crate::{Database, DbError};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vault_domain::{Playlist, PlaylistEntry, SharedSettings, State, Track};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaylistItem {
    pub entry: PlaylistEntry,
    pub track: Track,
    /// Position in the complete playlist, including when results are filtered.
    #[serde(default)]
    pub position: u32,
    #[serde(default)]
    pub move_up_after: Option<Uuid>,
    #[serde(default)]
    pub next_entry_id: Option<Uuid>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaylistDetail {
    pub playlist: Playlist,
    pub total: u64,
    pub offset: u32,
    pub items: Vec<PlaylistItem>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TrackPage {
    pub total: u64,
    pub offset: u32,
    pub tracks: Vec<Track>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AlbumSummary {
    pub title: String,
    pub artist: String,
    pub track_count: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtistSummary {
    pub name: String,
    pub track_count: u64,
}

impl Database {
    pub fn has_applied_operation(&self, op: Uuid) -> Result<bool, DbError> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM applied_events WHERE vault_id=?1 AND op_id=?2)",
            params![self.binding.vault_id.to_string(), op.to_string()],
            |row| row.get(0),
        )?)
    }
    pub fn local_settings(
        &self,
        prefix: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<(String, String)>, DbError> {
        let mut statement = self.connection.prepare("SELECT key,value FROM local_settings WHERE substr(key,1,length(?1))=?1 ORDER BY key LIMIT ?2 OFFSET ?3")?;
        Ok(statement
            .query_map(params![prefix, limit.clamp(1, 500), offset], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })?
            .collect::<Result<_, _>>()?)
    }
    /// Reads one consistent playlist and pages its domain order. Receipts and
    /// unrelated tracks are never loaded for a playlist detail query.
    pub fn playlist_detail(
        &self,
        id: Uuid,
        offset: u32,
        limit: u32,
    ) -> Result<Option<PlaylistDetail>, DbError> {
        self.playlist_search(id, "", offset, limit)
    }
    pub fn playlist_search(
        &self,
        id: Uuid,
        query: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Option<PlaylistDetail>, DbError> {
        let tx = self.connection.unchecked_transaction()?;
        let vault = self.binding.vault_id.to_string();
        let raw: Option<String> = tx
            .query_row(
                "SELECT json FROM playlists WHERE vault_id=?1 AND playlist_id=?2",
                params![vault, id.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        let Some(raw) = raw else {
            return Ok(None);
        };
        let playlist = serde_json::from_str(&raw)?;
        let mut order_state = State::new(self.binding.vault_id);
        let mut statement =
            tx.prepare("SELECT json FROM playlist_entries WHERE vault_id=?1 AND playlist_id=?2")?;
        for row in statement.query_map(params![vault, id.to_string()], |r| r.get::<_, String>(0))? {
            let entry: PlaylistEntry = serde_json::from_str(&row?)?;
            order_state.entries.insert(entry.entry_id, entry);
        }
        drop(statement);
        let order = order_state.playlist_order(id)?;
        let query = query.trim();
        let matches = if query.is_empty() {
            None
        } else {
            // User text is a literal FTS phrase, never query syntax. Search all
            // entries before paging; duplicates keep their distinct entry IDs.
            let literal = format!("\"{}\"", query.replace('"', "\"\""));
            let mut stmt = tx.prepare(
                "SELECT track_id FROM track_fts WHERE vault_id=?1 AND track_fts MATCH ?2",
            )?;
            let ids = stmt
                .query_map(params![vault, literal], |r| r.get::<_, String>(0))?
                .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
            Some(ids)
        };
        let filtered: Vec<_> = order
            .iter()
            .enumerate()
            .filter(|(_, entry_id)| {
                matches.as_ref().is_none_or(|ids| {
                    order_state
                        .entries
                        .get(entry_id)
                        .is_some_and(|entry| ids.contains(&entry.track_id.to_string()))
                })
            })
            .collect();
        let total = filtered.len() as u64;
        let mut items = Vec::new();
        let mut tracks = std::collections::BTreeMap::new();
        for (position, entry_id) in filtered
            .into_iter()
            .skip(offset as usize)
            .take(limit.clamp(1, 500) as usize)
        {
            let entry = order_state
                .entries
                .remove(entry_id)
                .ok_or(DbError::Schema)?;
            let track = if let Some(track) = tracks.get(&entry.track_id) {
                track
            } else {
                let raw: String = tx.query_row(
                    "SELECT json FROM tracks WHERE vault_id=?1 AND track_id=?2",
                    params![vault, entry.track_id.to_string()],
                    |r| r.get(0),
                )?;
                tracks
                    .entry(entry.track_id)
                    .or_insert(serde_json::from_str::<Track>(&raw)?)
            };
            items.push(PlaylistItem {
                entry,
                track: track.clone(),
                position: u32::try_from(position).map_err(|_| DbError::Schema)?,
                move_up_after: position.checked_sub(2).and_then(|i| order.get(i)).copied(),
                next_entry_id: order.get(position + 1).copied(),
            });
        }
        tx.commit()?;
        Ok(Some(PlaylistDetail {
            playlist,
            total,
            offset,
            items,
        }))
    }

    pub fn shared_settings(&self) -> Result<SharedSettings, DbError> {
        let raw: String = self.connection.query_row(
            "SELECT settings_json FROM vaults WHERE vault_id=?1",
            [self.binding.vault_id.to_string()],
            |r| r.get(0),
        )?;
        Ok(serde_json::from_str(&raw)?)
    }
    pub fn favorite_sets(&self, offset: u32, limit: u32) -> Result<Vec<String>, DbError> {
        let mut stmt = self.connection.prepare("SELECT DISTINCT set_name FROM favorites WHERE vault_id=?1 ORDER BY set_name LIMIT ?2 OFFSET ?3")?;
        Ok(stmt
            .query_map(
                params![
                    self.binding.vault_id.to_string(),
                    limit.clamp(1, 500),
                    offset
                ],
                |r| r.get(0),
            )?
            .collect::<Result<_, _>>()?)
    }
    pub fn favorites(&self, set_name: &str, offset: u32, limit: u32) -> Result<TrackPage, DbError> {
        let tx = self.connection.unchecked_transaction()?;
        let vault = self.binding.vault_id.to_string();
        let total = tx.query_row(
            "SELECT count(*) FROM favorites WHERE vault_id=?1 AND set_name=?2",
            params![vault, set_name],
            |r| count(r, 0),
        )?;
        let mut stmt = tx.prepare("SELECT t.json FROM favorites f JOIN tracks t ON t.vault_id=f.vault_id AND t.track_id=f.track_id WHERE f.vault_id=?1 AND f.set_name=?2 ORDER BY t.title,t.track_id LIMIT ?3 OFFSET ?4")?;
        let mut tracks = Vec::new();
        for row in stmt.query_map(params![vault, set_name, limit.clamp(1, 500), offset], |r| {
            r.get::<_, String>(0)
        })? {
            tracks.push(serde_json::from_str(&row?)?);
        }
        drop(stmt);
        tx.commit()?;
        Ok(TrackPage {
            total,
            offset,
            tracks,
        })
    }
    pub fn albums(
        &self,
        query: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<AlbumSummary>, DbError> {
        let mut stmt = self.connection.prepare("SELECT album, COALESCE(NULLIF(json_extract(json,'$.metadata.album_artist'),''),performer) AS artist, count(*) FROM tracks WHERE vault_id=?1 AND album<>'' AND instr(lower(album),lower(?2))>0 GROUP BY album,artist ORDER BY album,artist LIMIT ?3 OFFSET ?4")?;
        Ok(stmt
            .query_map(
                params![
                    self.binding.vault_id.to_string(),
                    query,
                    limit.clamp(1, 500),
                    offset
                ],
                |r| {
                    Ok(AlbumSummary {
                        title: r.get(0)?,
                        artist: r.get(1)?,
                        track_count: count(r, 2)?,
                    })
                },
            )?
            .collect::<Result<_, _>>()?)
    }
    pub fn artists(
        &self,
        query: &str,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<ArtistSummary>, DbError> {
        let mut stmt = self.connection.prepare("SELECT performer,count(*) FROM tracks WHERE vault_id=?1 AND performer<>'' AND instr(lower(performer),lower(?2))>0 GROUP BY performer ORDER BY performer LIMIT ?3 OFFSET ?4")?;
        Ok(stmt
            .query_map(
                params![
                    self.binding.vault_id.to_string(),
                    query,
                    limit.clamp(1, 500),
                    offset
                ],
                |r| {
                    Ok(ArtistSummary {
                        name: r.get(0)?,
                        track_count: count(r, 1)?,
                    })
                },
            )?
            .collect::<Result<_, _>>()?)
    }
}

fn count(row: &rusqlite::Row<'_>, column: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(column)?;
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

/// Local, read-only accounting; no Telegram completeness is inferred from it.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectionDiagnostics {
    pub generation: u64,
    pub cursor: String,
    pub snapshot_barrier: String,
    pub database_bytes: u64,
    pub active_tracks: u64,
    pub active_playlists: u64,
    pub playlist_entries: u64,
    pub uncovered_events: u64,
    pub pending_commands: u64,
    pub gc_plans: Vec<GcPlanSummary>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GcPlanSummary {
    pub plan_id: Uuid,
    pub status: String,
}
impl Database {
    pub fn diagnostics(&self) -> Result<ProjectionDiagnostics, DbError> {
        let tx = self.connection.unchecked_transaction()?;
        let vault = self.binding.vault_id.to_string();
        let (generation, cursor, barrier) = tx.query_row(
            "SELECT v.generation,s.cursor,s.barrier_id FROM vaults v JOIN sync_state s USING(vault_id) WHERE vault_id=?1",
            [&vault], |row| Ok((count(row,0)?, row.get::<_,i64>(1)?, row.get::<_,i64>(2)?)))?;
        let counts = |query: &str| tx.query_row(query, [&vault], |row| count(row, 0));
        let active_tracks = counts("SELECT count(*) FROM tracks WHERE vault_id=?1")?;
        let active_playlists = counts("SELECT count(*) FROM playlists WHERE vault_id=?1")?;
        let playlist_entries = counts("SELECT count(*) FROM playlist_entries WHERE vault_id=?1")?;
        let uncovered_events = tx.query_row(
            "SELECT count(*) FROM applied_events WHERE vault_id=?1 AND message_id>?2",
            params![vault, barrier],
            |row| count(row, 0),
        )?;
        let pending_commands = counts(
            "SELECT count(*) FROM inflight_commands WHERE vault_id=?1 AND status NOT IN ('APPLIED','REJECTED_BY_REDUCER','TELEGRAM_ERROR')",
        )?;
        let pages: u64 = tx.query_row("PRAGMA page_count", [], |row| count(row, 0))?;
        let page_size: u64 = tx.query_row("PRAGMA page_size", [], |row| count(row, 0))?;
        let mut statement = tx.prepare(
            "SELECT plan_id,status FROM gc_plans WHERE vault_id=?1 ORDER BY rowid DESC LIMIT 100",
        )?;
        let rows = statement.query_map([vault], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut gc_plans = Vec::new();
        for row in rows {
            let (id, status) = row?;
            gc_plans.push(GcPlanSummary {
                plan_id: Uuid::parse_str(&id).map_err(|_| DbError::Corrupt)?,
                status: serde_json::from_str(&status)?,
            });
        }
        drop(statement);
        tx.commit()?;
        Ok(ProjectionDiagnostics {
            generation,
            cursor: cursor.to_string(),
            snapshot_barrier: barrier.to_string(),
            database_bytes: pages.checked_mul(page_size).ok_or(DbError::Corrupt)?,
            active_tracks,
            active_playlists,
            playlist_entries,
            uncovered_events,
            pending_commands,
            gc_plans,
        })
    }
    /// One revision per playlist, including its ordered entry content. Deleted
    /// playlists disappear from this map so observers can invalidate them too.
    pub fn playlist_revisions(&self) -> Result<std::collections::BTreeMap<Uuid, String>, DbError> {
        use sha2::Digest;
        let tx = self.connection.unchecked_transaction()?;
        let mut revisions = std::collections::BTreeMap::<Uuid, sha2::Sha256>::new();
        let mut statement = tx.prepare(
            "SELECT playlist_id,json FROM playlists WHERE vault_id=?1 ORDER BY playlist_id",
        )?;
        for row in statement.query_map([self.binding.vault_id.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (id, json) = row?;
            let mut digest = sha2::Sha256::new();
            digest.update(json.as_bytes());
            revisions.insert(Uuid::parse_str(&id).map_err(|_| DbError::Corrupt)?, digest);
        }
        drop(statement);
        let mut statement = tx.prepare("SELECT playlist_id,json FROM playlist_entries WHERE vault_id=?1 ORDER BY playlist_id,entry_id")?;
        for row in statement.query_map([self.binding.vault_id.to_string()], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        })? {
            let (id, json) = row?;
            let id = Uuid::parse_str(&id).map_err(|_| DbError::Corrupt)?;
            let digest = revisions.get_mut(&id).ok_or(DbError::Corrupt)?;
            digest.update([0]);
            digest.update(json.as_bytes());
        }
        drop(statement);
        tx.commit()?;
        Ok(revisions
            .into_iter()
            .map(|(id, digest)| (id, hex::encode(digest.finalize())))
            .collect())
    }
    pub fn command_revisions(&self) -> Result<Vec<crate::Command>, DbError> {
        let mut statement = self
            .connection
            .prepare("SELECT op_id FROM inflight_commands WHERE vault_id=?1 ORDER BY op_id")?;
        let mut result = Vec::new();
        for row in statement.query_map([self.binding.vault_id.to_string()], |row| {
            row.get::<_, String>(0)
        })? {
            let id = Uuid::parse_str(&row?).map_err(|_| DbError::Corrupt)?;
            result.push(self.command(id)?.ok_or(DbError::Corrupt)?);
        }
        Ok(result)
    }
}
