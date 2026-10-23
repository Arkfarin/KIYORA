//! Atomic SQLite projection and local queries for kiyora.

mod projection;
mod queries;
pub use queries::*;
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};
use uuid::Uuid;
use vault_domain::*;
use vault_protocol::{Record, SnapshotCommit};

#[derive(Debug, thiserror::Error)]
pub enum DbError {
    #[error("SQLite operation failed")]
    Sql(#[from] rusqlite::Error),
    #[error("invalid stored JSON")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Domain(#[from] DomainError),
    #[error(transparent)]
    Protocol(#[from] vault_protocol::ProtocolError),
    #[error("unsupported database schema")]
    Schema,
    #[error("database integrity check failed")]
    Corrupt,
    #[error("profile or vault binding mismatch")]
    Binding,
    #[error("server message changed after it was observed")]
    MessageChanged,
    #[error("invalid snapshot boundary")]
    SnapshotBoundary,
    #[error("command already exists with different intent")]
    CommandCollision,
    #[error("invalid command lifecycle transition")]
    CommandTransition,
}
impl DbError {
    pub fn is_corruption(&self) -> bool {
        match self {
            Self::Corrupt | Self::Json(_) => true,
            Self::Sql(rusqlite::Error::SqliteFailure(error, _)) => matches!(
                error.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            ),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub vault_id: Uuid,
    pub owner_id: UserId,
    pub chat_id: ChatId,
}

pub struct Database {
    connection: Connection,
    binding: Binding,
}

impl Database {
    pub fn has_pending_gc(&self) -> Result<bool, DbError> {
        Ok(self.connection.query_row("SELECT EXISTS(SELECT 1 FROM gc_plans WHERE vault_id=?1 AND status NOT IN ('\"complete\"','\"abandoned\"'))", [self.binding.vault_id.to_string()], |r| r.get(0))?)
    }
    pub fn abandon_gc_plans(&mut self) -> Result<(), DbError> {
        self.connection.execute("UPDATE gc_plans SET status='\"abandoned\"' WHERE vault_id=?1 AND status<>'\"complete\"'", [self.binding.vault_id.to_string()])?;
        Ok(())
    }
    pub fn save_gc_plan(
        &mut self,
        plan_id: Uuid,
        json: &str,
        completed: &str,
        status: &str,
    ) -> Result<(), DbError> {
        self.connection.execute("INSERT INTO gc_plans VALUES (?1,?2,?3,?4,?5) ON CONFLICT(plan_id) DO UPDATE SET completed_json=excluded.completed_json,status=excluded.status",params![plan_id.to_string(),self.binding.vault_id.to_string(),json,completed,status])?;
        Ok(())
    }
    pub fn gc_plan(&self, plan_id: Uuid) -> Result<Option<(String, String, String)>, DbError> {
        Ok(self
            .connection
            .query_row(
                "SELECT json,completed_json,status FROM gc_plans WHERE plan_id=?1 AND vault_id=?2",
                params![plan_id.to_string(), self.binding.vault_id.to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    }
    pub fn open(path: impl AsRef<Path>, binding: Binding) -> Result<Self, DbError> {
        Self::initialize(Connection::open(path)?, binding)
    }
    pub fn memory(binding: Binding) -> Result<Self, DbError> {
        Self::initialize(Connection::open_in_memory()?, binding)
    }
    fn initialize(mut connection: Connection, binding: Binding) -> Result<Self, DbError> {
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.execute_batch(
            "PRAGMA foreign_keys=ON; PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL;",
        )?;
        let version: i64 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 1 {
            return Err(DbError::Schema);
        }
        if version == 0 {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(include_str!("../migrations/001_projection.sql"))?;
            tx.commit()?;
        }
        let existing: Option<(String, i64, i64)> = connection
            .query_row(
                "SELECT vault_id,telegram_user_id,chat_id FROM vaults LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((vault, owner, chat)) = existing {
            if vault != binding.vault_id.to_string()
                || owner != binding.owner_id.get()
                || chat != binding.chat_id.get()
            {
                return Err(DbError::Binding);
            }
        } else {
            if binding.vault_id.is_nil() {
                return Err(DbError::Binding);
            }
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                "INSERT INTO vaults VALUES (?1,?2,?3,0,?4)",
                params![
                    binding.vault_id.to_string(),
                    binding.owner_id.get(),
                    binding.chat_id.get(),
                    serde_json::to_string(&SharedSettings::default())?
                ],
            )?;
            tx.execute(
                "INSERT INTO sync_state(vault_id) VALUES (?1)",
                [binding.vault_id.to_string()],
            )?;
            tx.commit()?;
        }
        Ok(Self {
            connection,
            binding,
        })
    }
    pub fn binding(&self) -> Binding {
        self.binding
    }
    /// Validate disk structure and the portable projection before exposing a
    /// reopened or reconstructed database. Never rewrites damaged rows.
    pub fn validate(&self) -> Result<(), DbError> {
        let check: String = self
            .connection
            .query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if check != "ok"
            || self
                .connection
                .prepare("PRAGMA foreign_key_check")?
                .exists([])?
        {
            return Err(DbError::Corrupt);
        }
        self.state()?.validate().map_err(|_| DbError::Corrupt)?;
        Ok(())
    }
    /// Produces a self-contained SQLite file for the recovery installer.
    pub fn checkpoint(&self) -> Result<(), DbError> {
        let (busy, log, checkpointed): (i64, i64, i64) =
            self.connection
                .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?;
        if busy != 0 || log != checkpointed {
            return Err(DbError::Corrupt);
        }
        Ok(())
    }
    /// Local database location; never included in the portable projection.
    pub fn path(&self) -> Option<std::path::PathBuf> {
        self.connection
            .path()
            .filter(|path| !path.is_empty())
            .map(Into::into)
    }
    pub fn state(&self) -> Result<State, DbError> {
        let tx = self.connection.unchecked_transaction()?;
        let state = projection::read(&tx, self.binding.vault_id)?;
        tx.commit()?;
        Ok(state)
    }
    pub fn cursor(&self) -> Result<i64, DbError> {
        Ok(self.connection.query_row(
            "SELECT cursor FROM sync_state WHERE vault_id=?1",
            [self.binding.vault_id.to_string()],
            |r| r.get(0),
        )?)
    }
    pub fn apply_wire(&mut self, mid: MessageId, bytes: &[u8]) -> Result<Option<Outcome>, DbError> {
        self.apply_record(mid, &vault_protocol::decode(bytes)?)
    }
    pub fn apply_record(
        &mut self,
        mid: MessageId,
        record: &Record,
    ) -> Result<Option<Outcome>, DbError> {
        let outcomes = self.apply_records(&[(mid, record.clone())])?;
        outcomes.into_iter().next().ok_or(DbError::Schema)
    }

    /// Apply one ordered history batch atomically, reconstructing the projection once.
    pub fn apply_records(
        &mut self,
        records: &[(MessageId, Record)],
    ) -> Result<Vec<Option<Outcome>>, DbError> {
        if records.is_empty() {
            return Ok(Vec::new());
        }
        let vault = self.binding.vault_id.to_string();
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let before = projection::read(&tx, self.binding.vault_id)?;
        let mut after = before.clone();
        let mut cursor: i64 = tx.query_row(
            "SELECT cursor FROM sync_state WHERE vault_id=?1",
            [&vault],
            |r| r.get(0),
        )?;
        let mut outcomes = Vec::with_capacity(records.len());

        for (mid, record) in records {
            if record.vault_id() != self.binding.vault_id {
                return Err(DbError::Binding);
            }
            if let Record::Init(init) = record
                && init.owner_telegram_user_id != self.binding.owner_id
            {
                return Err(DbError::Binding);
            }
            let digest = hex::encode(Sha256::digest(vault_protocol::encode(record)?));
            let seen: Option<String> = tx
                .query_row(
                    "SELECT record_sha256 FROM observed_messages WHERE vault_id=?1 AND message_id=?2",
                    params![vault, mid.get()],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(seen) = seen {
                if seen != digest {
                    return Err(DbError::MessageChanged);
                }
                outcomes.push(match record {
                    Record::Event(event) => Some(
                        after
                            .applied
                            .get(&event.op_id)
                            .ok_or(DbError::Schema)?
                            .outcome
                            .clone(),
                    ),
                    _ => None,
                });
                continue;
            }
            if mid.get() <= cursor {
                return Err(DomainError::OutOfOrder.into());
            }
            let outcome = if let Record::Event(event) = record {
                let outcome = after.apply(*mid, event)?;
                // A reconciled receipt, not the network ACK, makes the command terminal.
                let status = match outcome {
                    Outcome::Applied | Outcome::NoOp(_) => CommandStatus::Applied,
                    Outcome::Rejected(_) => CommandStatus::RejectedByReducer,
                };
                let receipt_id = after
                    .applied
                    .get(&event.op_id)
                    .ok_or(DbError::Schema)?
                    .message_id;
                tx.execute("UPDATE inflight_commands SET status=?1,server_msg_id=?2,result_json=?3 WHERE vault_id=?4 AND op_id=?5",params![status.as_str(),receipt_id.get(),serde_json::to_string(&outcome)?,vault,event.op_id.to_string()])?;
                Some(outcome)
            } else {
                None
            };
            tx.execute(
                "INSERT INTO observed_messages VALUES (?1,?2,?3,?4)",
                params![vault, mid.get(), record.kind(), digest],
            )?;
            cursor = mid.get();
            tx.execute(
                "UPDATE sync_state SET cursor=?1 WHERE vault_id=?2",
                params![cursor, vault],
            )?;
            outcomes.push(outcome);
        }
        projection::write_delta(&tx, &before, &after)?;
        tx.commit()?;
        Ok(outcomes)
    }

    /// Install only a verified full snapshot. Local cache/settings/commands survive.
    pub fn restore(
        &mut self,
        state: &State,
        barrier: MessageId,
        commit_id: MessageId,
        commit: &SnapshotCommit,
    ) -> Result<(), DbError> {
        state.validate()?;
        if state.vault_id != self.binding.vault_id || commit.vault_id != state.vault_id {
            return Err(DbError::Binding);
        }
        if state.last_event_message_id.is_some_and(|id| id >= barrier)
            || commit.barrier_message_id != barrier
            || commit.generation != state.generation
            || commit_id <= commit.snapshot_a_message_id
            || commit_id <= commit.snapshot_b_message_id
        {
            return Err(DbError::SnapshotBoundary);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        // Replace the entire shared projection in FK order, within the same transaction.
        // Local cache, settings and inflight commands are deliberately separate tables.
        for table in [
            "playlist_entries",
            "favorites",
            "track_sources",
            "tracks",
            "playlists",
            "tombstones",
            "applied_events",
        ] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE vault_id=?1"),
                [state.vault_id.to_string()],
            )?;
        }
        projection::write_delta(&tx, &State::new(state.vault_id), state)?;
        tx.execute(
            "DELETE FROM observed_messages WHERE vault_id=?1",
            [state.vault_id.to_string()],
        )?;
        tx.execute("UPDATE sync_state SET cursor=?1,barrier_id=?1,last_verified_commit_id=?2 WHERE vault_id=?3",params![barrier.get(),commit_id.get(),state.vault_id.to_string()])?;
        tx.execute(
            "INSERT OR REPLACE INTO snapshot_registry VALUES (?1,?2,?3,?4,?5,'verified',?6)",
            params![
                state.vault_id.to_string(),
                commit.snapshot_id.to_string(),
                commit_id.get(),
                barrier.get(),
                i64::try_from(commit.generation).map_err(|_| DbError::Schema)?,
                serde_json::to_string(commit)?
            ],
        )?;
        for (op, receipt) in &state.applied {
            let status = if matches!(receipt.outcome, Outcome::Rejected(_)) {
                CommandStatus::RejectedByReducer
            } else {
                CommandStatus::Applied
            };
            tx.execute("UPDATE inflight_commands SET status=?1,server_msg_id=?2,result_json=?3 WHERE vault_id=?4 AND op_id=?5",params![status.as_str(),receipt.message_id.get(),serde_json::to_string(&receipt.outcome)?,state.vault_id.to_string(),op.to_string()])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn library(
        &self,
        query: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<Track>, DbError> {
        self.search("tracks", "track_fts", "track_id", query, offset, limit)
    }
    pub fn playlists(
        &self,
        query: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<Playlist>, DbError> {
        self.search(
            "playlists",
            "playlist_fts",
            "playlist_id",
            query,
            offset,
            limit,
        )
    }
    fn search<T: serde::de::DeserializeOwned>(
        &self,
        table: &str,
        fts: &str,
        id: &str,
        query: Option<&str>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<T>, DbError> {
        let query = query.map(str::trim).filter(|s| !s.is_empty());
        let sql = if query.is_some() {
            format!(
                "SELECT t.json FROM {table} t JOIN {fts} f ON f.vault_id=t.vault_id AND f.{id}=t.{id} WHERE t.vault_id=?1 AND {fts} MATCH ?4 ORDER BY t.{id} LIMIT ?2 OFFSET ?3"
            )
        } else {
            format!("SELECT json FROM {table} WHERE vault_id=?1 ORDER BY {id} LIMIT ?2 OFFSET ?3")
        };
        let mut stmt = self.connection.prepare(&sql)?;
        let vault = self.binding.vault_id.to_string();
        let limit = limit.clamp(1, 500);
        let mut rows = if let Some(q) = query {
            // Treat UI search as literal text, never as executable FTS query syntax.
            let literal = format!("\"{}\"", q.replace('"', "\"\""));
            stmt.query(params![vault, limit, offset, literal])?
        } else {
            stmt.query(params![vault, limit, offset])?
        };
        let mut results = Vec::new();
        while let Some(row) = rows.next()? {
            results.push(serde_json::from_str(&row.get::<_, String>(0)?)?);
        }
        Ok(results)
    }

    pub fn create_command(&mut self, event: &Event) -> Result<Command, DbError> {
        if event.vault_id != self.binding.vault_id {
            return Err(DbError::Binding);
        }
        if let Some(existing) = self.command(event.op_id)? {
            if existing.intent != *event {
                return Err(DbError::CommandCollision);
            }
            return Ok(existing);
        }
        let intent = String::from_utf8(vault_protocol::encode(&Record::Event(Box::new(
            event.clone(),
        )))?)
        .map_err(|_| DbError::Schema)?;
        self.connection.execute("INSERT INTO inflight_commands(vault_id,op_id,intent,status) VALUES (?1,?2,?3,'CREATED')",params![event.vault_id.to_string(),event.op_id.to_string(),intent])?;
        Ok(Command {
            intent: event.clone(),
            status: CommandStatus::Created,
            server_message_id: None,
            outcome: None,
        })
    }
    /// Recreate a lost local command only from its verified portable receipt and
    /// the exact original server intent. No new Telegram send is necessary.
    pub fn recover_command(&mut self, event: &Event) -> Result<Command, DbError> {
        if event.vault_id != self.binding.vault_id {
            return Err(DbError::Binding);
        }
        if let Some(existing) = self.command(event.op_id)? {
            return if existing.intent == *event {
                Ok(existing)
            } else {
                Err(DbError::CommandCollision)
            };
        }
        let (message_id,digest,outcome):(i64,String,String)=self.connection.query_row(
            "SELECT message_id,event_sha256,outcome FROM applied_events WHERE vault_id=?1 AND op_id=?2",
            params![event.vault_id.to_string(),event.op_id.to_string()],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
        if digest != vault_domain::event_digest(event)? {
            return Err(DbError::CommandCollision);
        }
        let outcome: Outcome = serde_json::from_str(&outcome)?;
        let status = if matches!(outcome, Outcome::Rejected(_)) {
            CommandStatus::RejectedByReducer
        } else {
            CommandStatus::Applied
        };
        let intent = String::from_utf8(vault_protocol::encode(&Record::Event(Box::new(
            event.clone(),
        )))?)
        .map_err(|_| DbError::Schema)?;
        self.connection.execute("INSERT INTO inflight_commands(vault_id,op_id,intent,status,server_msg_id,result_json) VALUES (?1,?2,?3,?4,?5,?6)",
            params![event.vault_id.to_string(),event.op_id.to_string(),intent,status.as_str(),message_id,serde_json::to_string(&outcome)?])?;
        self.command(event.op_id)?.ok_or(DbError::Corrupt)
    }
    pub fn command(&self, op_id: Uuid) -> Result<Option<Command>, DbError> {
        let row:Option<(String,String,Option<i64>,Option<String>)>=self.connection.query_row("SELECT intent,status,server_msg_id,result_json FROM inflight_commands WHERE vault_id=?1 AND op_id=?2",params![self.binding.vault_id.to_string(),op_id.to_string()],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let Some((intent, status, server, result)) = row else {
            return Ok(None);
        };
        let Record::Event(intent) = vault_protocol::decode(intent.as_bytes())? else {
            return Err(DbError::Schema);
        };
        Ok(Some(Command {
            intent: *intent,
            status: serde_json::from_value(serde_json::Value::String(status))?,
            server_message_id: server
                .map(MessageId::new)
                .transpose()
                .map_err(|_| DbError::Schema)?,
            outcome: result.map(|s| serde_json::from_str(&s)).transpose()?,
        }))
    }
    pub fn transition_command(
        &mut self,
        op_id: Uuid,
        status: CommandStatus,
        server: Option<MessageId>,
    ) -> Result<(), DbError> {
        let existing = self.command(op_id)?.ok_or(DbError::CommandTransition)?;
        if !existing.status.can_transition(status) {
            return Err(DbError::CommandTransition);
        }
        self.connection.execute("UPDATE inflight_commands SET status=?1,server_msg_id=COALESCE(?2,server_msg_id) WHERE vault_id=?3 AND op_id=?4",params![status.as_str(),server.map(MessageId::get),self.binding.vault_id.to_string(),op_id.to_string()])?;
        Ok(())
    }
    pub fn mark_interrupted_commands_unknown(&mut self) -> Result<(), DbError> {
        self.connection.execute("UPDATE inflight_commands SET status='UNKNOWN_ON_TIMEOUT' WHERE vault_id=?1 AND status IN ('SENDING','SERVER_ACK','RECONCILING')",[self.binding.vault_id.to_string()])?;
        Ok(())
    }
    pub fn local_setting(&self, key: &str) -> Result<Option<String>, DbError> {
        Ok(self
            .connection
            .query_row(
                "SELECT value FROM local_settings WHERE key=?1",
                [key],
                |r| r.get(0),
            )
            .optional()?)
    }
    pub fn set_local_setting(&mut self, key: &str, value: &str) -> Result<(), DbError> {
        self.connection.execute("INSERT INTO local_settings VALUES (?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",params![key,value])?;
        Ok(())
    }
    pub fn clear_local_setting(&mut self, key: &str) -> Result<(), DbError> {
        self.connection
            .execute("DELETE FROM local_settings WHERE key=?1", [key])?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CommandStatus {
    Created,
    Sending,
    ServerAck,
    Reconciling,
    Applied,
    RejectedByReducer,
    UnknownOnTimeout,
    TelegramError,
}
impl CommandStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Created => "CREATED",
            Self::Sending => "SENDING",
            Self::ServerAck => "SERVER_ACK",
            Self::Reconciling => "RECONCILING",
            Self::Applied => "APPLIED",
            Self::RejectedByReducer => "REJECTED_BY_REDUCER",
            Self::UnknownOnTimeout => "UNKNOWN_ON_TIMEOUT",
            Self::TelegramError => "TELEGRAM_ERROR",
        }
    }
    fn can_transition(self, next: Self) -> bool {
        use CommandStatus::*;
        matches!(
            (self, next),
            (Created, Sending)
                | (Sending, ServerAck | UnknownOnTimeout | TelegramError)
                | (ServerAck, Reconciling | UnknownOnTimeout)
                | (UnknownOnTimeout, Reconciling)
                | (Reconciling, UnknownOnTimeout)
        )
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Command {
    pub intent: Event,
    pub status: CommandStatus,
    #[schemars(with = "Option<String>")]
    pub server_message_id: Option<MessageId>,
    pub outcome: Option<Outcome>,
}
