//! Local queries remain usable while the single writer performs network work.
use crate::SyncError;
use std::sync::{Arc, Mutex};
use uuid::Uuid;
use vault_db::{Binding, Command, Database, DbError};
use vault_domain::{Playlist, State, Track};

#[derive(Clone)]
pub struct ProjectionReader {
    pub(crate) db: Arc<Mutex<Database>>,
    pub(crate) binding: Binding,
}
impl ProjectionReader {
    pub fn new(database: Database) -> Self {
        Self {
            binding: database.binding(),
            db: Arc::new(Mutex::new(database)),
        }
    }
    pub fn binding(&self) -> Binding {
        self.binding
    }
    async fn read<R: Send + 'static>(
        &self,
        f: impl FnOnce(&Database) -> Result<R, DbError> + Send + 'static,
    ) -> Result<R, SyncError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let db = db.lock().map_err(|_| SyncError::Worker)?;
            f(&db).map_err(SyncError::Database)
        })
        .await
        .map_err(|_| SyncError::Worker)?
    }
    pub async fn state(&self) -> Result<State, SyncError> {
        self.read(Database::state).await
    }
    pub async fn cursor(&self) -> Result<i64, SyncError> {
        self.read(Database::cursor).await
    }
    pub async fn command(&self, op: Uuid) -> Result<Option<Command>, SyncError> {
        self.read(move |db| db.command(op)).await
    }
    pub async fn library(
        &self,
        query: Option<String>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<Track>, SyncError> {
        self.read(move |db| db.library(query.as_deref(), offset, limit))
            .await
    }
    pub async fn playlists(
        &self,
        query: Option<String>,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<Playlist>, SyncError> {
        self.read(move |db| db.playlists(query.as_deref(), offset, limit))
            .await
    }
}
