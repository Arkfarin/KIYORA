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
    pub(crate) async fn read<R: Send + 'static>(
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
    /// The shared writer mutex covers both reads, so the cursor describes this
    /// projection even while the service is applying remote events.
    pub async fn snapshot(&self) -> Result<(State, String), SyncError> {
        self.read(|db| Ok((db.state()?, db.cursor()?.to_string())))
            .await
    }
    pub async fn diagnostics(&self) -> Result<vault_db::ProjectionDiagnostics, SyncError> {
        self.read(Database::diagnostics).await
    }
    pub async fn has_pending_gc(&self) -> Result<bool, SyncError> {
        self.read(Database::has_pending_gc).await
    }
    pub async fn playlist_revisions(
        &self,
    ) -> Result<std::collections::BTreeMap<Uuid, String>, SyncError> {
        self.read(Database::playlist_revisions).await
    }
    pub async fn command_revisions(&self) -> Result<Vec<Command>, SyncError> {
        self.read(Database::command_revisions).await
    }
    pub async fn cursor(&self) -> Result<i64, SyncError> {
        self.read(Database::cursor).await
    }
    pub async fn command(&self, op: Uuid) -> Result<Option<Command>, SyncError> {
        self.read(move |db| db.command(op)).await
    }
    pub async fn has_applied_operation(&self, op: Uuid) -> Result<bool, SyncError> {
        self.read(move |db| db.has_applied_operation(op)).await
    }
    pub async fn local_settings(
        &self,
        prefix: String,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<(String, String)>, SyncError> {
        self.read(move |db| db.local_settings(&prefix, offset, limit))
            .await
    }
    pub async fn playlist_detail(
        &self,
        id: Uuid,
        offset: u32,
        limit: u32,
    ) -> Result<Option<vault_db::PlaylistDetail>, SyncError> {
        self.read(move |db| db.playlist_detail(id, offset, limit))
            .await
    }
    pub async fn shared_settings(&self) -> Result<vault_domain::SharedSettings, SyncError> {
        self.read(Database::shared_settings).await
    }
    pub async fn favorite_sets(&self, offset: u32, limit: u32) -> Result<Vec<String>, SyncError> {
        self.read(move |db| db.favorite_sets(offset, limit)).await
    }
    pub async fn favorites(
        &self,
        set_name: String,
        offset: u32,
        limit: u32,
    ) -> Result<vault_db::TrackPage, SyncError> {
        self.read(move |db| db.favorites(&set_name, offset, limit))
            .await
    }
    pub async fn albums(
        &self,
        query: String,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<vault_db::AlbumSummary>, SyncError> {
        self.read(move |db| db.albums(&query, offset, limit)).await
    }
    pub async fn artists(
        &self,
        query: String,
        offset: u32,
        limit: u32,
    ) -> Result<Vec<vault_db::ArtistSummary>, SyncError> {
        self.read(move |db| db.artists(&query, offset, limit)).await
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
