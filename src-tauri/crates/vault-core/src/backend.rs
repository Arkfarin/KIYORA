use crate::service::media_cache_key;
use crate::*;
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_db::{Binding, Database};
use vault_protocol::{Record, VaultInit, encode_text};
use vault_security::{OsSecrets, Profile, ProfileLease, ProfileStore, SecretStore};
use vault_sync::{ProjectionReader, RuntimeMode, SyncEngine};
use vault_telegram::{
    AuthInput, AuthStatus, ChannelInfo, TdClient, TdParameters, TdTransport, Telegram,
};
use zeroize::Zeroizing;
mod artwork;
mod queue;
mod storage;
pub use artwork::ArtworkData;
pub use queue::{QueueInfo, QueuePage};
pub use storage::StorageUsage;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AccountInfo {
    #[schemars(with = "String")]
    pub user_id: vault_domain::UserId,
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectionSnapshot {
    pub context: Context,
    /// Subscribe before requesting. Ignore events <= this sequence; later
    /// invalidations may overlap the snapshot and must be applied idempotently.
    pub event_sequence: String,
    pub cursor: String,
    pub state: vault_domain::State,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileConfig {
    schema: u32,
    device_id: String,
    api_id: Option<i32>,
    binding: Option<Binding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    init_message_id: Option<vault_domain::MessageId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    discovery_hints: Vec<vault_telegram::SavedVault>,
    creation: Option<CreateIntent>,
    #[serde(default = "default_cache_quota")]
    cache_quota_bytes: u64,
}
fn default_cache_quota() -> u64 {
    10 * 1024 * 1024 * 1024
}
impl ProfileConfig {
    fn remember_discovered_vault(&mut self, candidate: &vault_telegram::VaultCandidate) {
        self.discovery_hints
            .retain(|hint| hint.chat_id != candidate.channel.chat_id);
        self.discovery_hints.push(vault_telegram::SavedVault {
            chat_id: candidate.channel.chat_id,
            owner_id: candidate.channel.owner_id,
            vault_id: candidate.init.vault_id,
            init_message_id: Some(candidate.init_message_id),
        });
        // backend.json has a 16 KiB read limit. Keep this optional lookup cache
        // small; the selected binding's marker is stored independently.
        if self.discovery_hints.len() > 40 {
            self.discovery_hints.remove(0);
        }
        if self.binding.is_some_and(|b| {
            b.chat_id == candidate.channel.chat_id
                && b.vault_id == candidate.init.vault_id
                && b.owner_id == candidate.channel.owner_id
        }) {
            self.init_message_id = Some(candidate.init_message_id);
        }
    }
    fn fresh() -> Self {
        Self {
            schema: 2,
            device_id: Uuid::new_v4().to_string(),
            api_id: None,
            binding: None,
            init_message_id: None,
            discovery_hints: Vec::new(),
            creation: None,
            cache_quota_bytes: default_cache_quota(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
enum CreateStage {
    CreatingUnknown,
    ChannelCreated,
    InitUnknown,
    Initialized,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateIntent {
    vault_id: Uuid,
    stage: CreateStage,
    chat_id: Option<vault_domain::ChatId>,
}
struct NativeSession {
    epoch: Uuid,
    client: TdClient,
    transport: TdTransport,
    activated: bool,
    cancel: CancellationToken,
    watcher: Option<tokio::task::JoinHandle<()>>,
}
impl Drop for NativeSession {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(watcher) = self.watcher.take() {
            watcher.abort();
        }
    }
}
struct Session {
    queue: queue::Queue,
    playback_id: Option<Uuid>,
    playback_ticket: Uuid,
    playback_cancel: CancellationToken,
    jobs: crate::jobs::JobRegistry,
    lease: ProfileLease,
    config: ProfileConfig,
    native: Option<NativeSession>,
    service: Option<VaultService>,
    reader: Option<ProjectionReader>,
    repair_required: bool,
    cache: vault_media::CacheManager,
    playback: Option<vault_media::PlaybackEngine>,
    context: Context,
}
#[derive(Clone)]
pub struct Backend {
    job_access: Arc<std::sync::Mutex<Option<(Context, crate::jobs::JobRegistry)>>>,
    import_resolver: Option<Arc<dyn crate::ImportResolver>>,
    store: Arc<ProfileStore>,
    secrets: Arc<dyn SecretStore + Send + Sync>,
    playback_platform: Option<Arc<dyn vault_media::PlaybackPlatform>>,
    native_path: PathBuf,
    events: EventBus,
    session: Arc<Mutex<Option<Session>>>,
    // Separate from session: closing jobs must let their stale calls acquire
    // session and fail, while another switch/shutdown waits for the full close.
    session_transition: Arc<Mutex<()>>,
}
impl Backend {
    pub fn new(store: ProfileStore, native_path: impl Into<PathBuf>) -> Self {
        Self::with_secrets(store, native_path, Arc::new(OsSecrets))
    }
    /// Supply the native platform's secure storage. The provider runs on blocking
    /// workers, never on a Tokio executor or the Android UI thread.
    pub fn with_secrets(
        store: ProfileStore,
        native_path: impl Into<PathBuf>,
        secrets: Arc<dyn SecretStore + Send + Sync>,
    ) -> Self {
        Self {
            job_access: Arc::new(std::sync::Mutex::new(None)),
            import_resolver: None,
            store: Arc::new(store),
            secrets,
            playback_platform: None,
            native_path: native_path.into(),
            events: EventBus::default(),
            session: Arc::new(Mutex::new(None)),
            session_transition: Arc::new(Mutex::new(())),
        }
    }
    pub fn default_backend() -> Result<Self, ApiError> {
        Self::default_backend_with_resources(None)
    }
    pub fn default_backend_with_resources(
        resources: Option<&std::path::Path>,
    ) -> Result<Self, ApiError> {
        let store = match std::env::var_os("KIYORA_DATA_DIR") {
            Some(path) => ProfileStore::at(path)?,
            None => ProfileStore::default_location()?,
        };
        Ok(Self::new(
            store,
            crate::native_location::resolve(resources)?,
        ))
    }
    pub fn with_playback_platform(
        mut self,
        platform: Arc<dyn vault_media::PlaybackPlatform>,
    ) -> Self {
        self.playback_platform = Some(platform);
        self
    }
    pub fn events(&self) -> EventBus {
        self.events.clone()
    }
    pub fn with_import_resolver(mut self, resolver: Arc<dyn crate::ImportResolver>) -> Self {
        self.import_resolver = Some(resolver);
        self
    }
    pub fn subscribe_events(&self) -> tokio::sync::broadcast::Receiver<BackendEvent> {
        self.events.subscribe()
    }
    pub fn profiles(&self) -> Result<Vec<Profile>, ApiError> {
        self.store.list().map_err(Into::into)
    }
    pub fn active_profile_id(&self) -> Result<Option<Uuid>, ApiError> {
        self.store.active_profile().map_err(Into::into)
    }
    pub fn create_profile(&self, label: String) -> Result<Profile, ApiError> {
        self.store.create(label).map_err(Into::into)
    }
    pub async fn context(&self) -> Option<Context> {
        self.session
            .lock()
            .await
            .as_ref()
            .map(|s| s.context.clone())
    }
    pub async fn switch_profile(&self, id: Uuid) -> Result<Status, ApiError> {
        let transition = self.session_transition.clone().lock_owned().await;
        let backend = self.clone();
        // Once accepted, finish closing/publishing even if the IPC waiter goes
        // away. The transition lock belongs to the worker until it is complete.
        tokio::spawn(async move {
            let _transition = transition;
            backend.switch_profile_inner(id).await
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))?
    }
    async fn switch_profile_inner(&self, id: Uuid) -> Result<Status, ApiError> {
        // Acquire the new process lock and validate its durable binding before closing
        // the current account, so a failed switch leaves the old session usable.
        let store = self.store.clone();
        let (lease, config, reader, cache, repair_required, jobs) =
            tokio::task::spawn_blocking(move || {
                let lease = store.acquire(id)?;
                let config = load_config(&lease)?;
                save_config(&lease, &config)?;
                crate::recovery::resume_install(&lease.database_path())?;
                let mut repair_required = false;
                let reader = match config.binding {
                    Some(binding) => {
                        match Database::open(lease.database_path(), binding).and_then(|db| {
                            db.validate()?;
                            Ok(db)
                        }) {
                            Ok(db) => Some(ProjectionReader::new(db)),
                            Err(error) if error.is_corruption() => {
                                repair_required = true;
                                None
                            }
                            Err(error) => return Err(error.into()),
                        }
                    }
                    None => None,
                };
                let cache =
                    vault_media::CacheManager::open(lease.cache_path(), config.cache_quota_bytes)
                        .map_err(ApiError::from)?;
                let jobs = crate::jobs::JobRegistry::open(lease.directory().join("jobs"))?;
                Ok::<_, ApiError>((lease, config, reader, cache, repair_required, jobs))
            })
            .await
            .map_err(|_| ApiError::new(ErrorCode::Internal))??;
        let old = self.session.lock().await.take();
        *self
            .job_access
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal))? = None;
        if let Some(old) = old {
            close_session(old).await?;
        }
        self.store.set_active_profile(id)?;
        let context = Context {
            profile_id: id,
            session_id: Uuid::new_v4(),
            vault_id: config.binding.map(|b| b.vault_id),
        };
        let api_id = config.api_id;
        let status = Status {
            context: context.clone(),
            mode: RuntimeMode::AuthRequired,
            vault: None,
        };
        let session = Session {
            queue: queue::Queue::default(),
            playback_id: None,
            playback_ticket: Uuid::nil(),
            playback_cancel: CancellationToken::new(),
            jobs,
            lease,
            config,
            native: None,
            service: None,
            reader,
            repair_required,
            cache,
            playback: None,
            context: context.clone(),
        };
        *self
            .job_access
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal))? =
            Some((context.clone(), session.jobs.clone()));
        *self.session.lock().await = Some(session);
        self.events
            .emit(&context, EventData::Runtime(status.clone()));
        if repair_required {
            self.events.emit(
                &context,
                EventData::Warning(ApiError::new(ErrorCode::DatabaseCorrupt)),
            );
        }
        // Restore TDLib only when this profile already has an OS-protected API hash.
        // A missing key keeps SQLite readable and asks for explicit authentication.
        if let Some(api_id) = api_id {
            let item = format!("{id}:api-hash");
            let secrets = self.secrets.clone();
            let hash = tokio::task::spawn_blocking(move || secrets.get(&item))
                .await
                .map_err(|_| ApiError::new(ErrorCode::Internal))
                .and_then(|value| value.map_err(ApiError::from));
            match hash {
                Ok(Some(hash)) => {
                    let auth = self
                        .connect_native(
                            context.session_id,
                            api_id,
                            Zeroizing::new(String::from_utf8_lossy(&hash).into_owned()),
                        )
                        .await;
                    if let Err(error) = auth {
                        // Native or secure-storage restoration failures must not
                        // hide the already opened local projection and cache.
                        self.events.emit(&context, EventData::Warning(error));
                    }
                }
                Ok(None) => self.events.emit(
                    &context,
                    EventData::Warning(ApiError::new(ErrorCode::SecureStorageUnavailable)),
                ),
                Err(error) => self.events.emit(&context, EventData::Warning(error)),
            }
        }
        self.runtime_status().await
    }
    pub async fn restore_active_profile(&self) -> Result<Option<Status>, ApiError> {
        match self.store.active_profile()? {
            Some(id) => self.switch_profile(id).await.map(Some),
            None => Ok(None),
        }
    }
    pub async fn runtime_status(&self) -> Result<Status, ApiError> {
        let guard = self.session.lock().await;
        let s = guard
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        Ok(s.service
            .as_ref()
            .map(VaultService::status)
            .unwrap_or(Status {
                context: s.context.clone(),
                mode: if s.repair_required {
                    RuntimeMode::DegradedReadonly
                } else if s.native.as_ref().is_some_and(|n| n.activated) {
                    RuntimeMode::Discovering
                } else {
                    RuntimeMode::AuthRequired
                },
                vault: None,
            }))
    }
    pub async fn local_reader(&self, session_id: Uuid) -> Result<ProjectionReader, ApiError> {
        let guard = self.session.lock().await;
        let s = guard
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        s.service
            .as_ref()
            .map(VaultService::reader)
            .or_else(|| s.reader.clone())
            .ok_or_else(|| {
                ApiError::new(if s.repair_required {
                    ErrorCode::DatabaseCorrupt
                } else {
                    ErrorCode::NoVault
                })
            })
    }
    /// Local-only recovery for a late or lagging IPC subscriber. Hold the session
    /// gate while reading so a profile switch cannot mix contexts or projections.
    pub async fn state_snapshot(&self, session_id: Uuid) -> Result<ProjectionSnapshot, ApiError> {
        let guard = self.session.lock().await;
        let s = guard
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        let reader = s
            .service
            .as_ref()
            .map(VaultService::reader)
            .or_else(|| s.reader.clone())
            .ok_or_else(|| {
                ApiError::new(if s.repair_required {
                    ErrorCode::DatabaseCorrupt
                } else {
                    ErrorCode::NoVault
                })
            })?;
        let event_sequence = self.events.sequence()?;
        let (state, cursor) = reader.snapshot().await?;
        Ok(ProjectionSnapshot {
            context: s.context.clone(),
            event_sequence,
            state,
            cursor,
        })
    }
    /// The owned session lock survives dropped IPC callers until replacement
    /// ends, preventing another profile operation from racing the installer.
    pub async fn repair_local_database(
        &self,
        session_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<SnapshotInfo, ApiError> {
        self.repair_database(session_id, None, cancel).await
    }
    pub async fn recover_database_from_backup(
        &self,
        session_id: Uuid,
        archive: PathBuf,
        cancel: &CancellationToken,
    ) -> Result<SnapshotInfo, ApiError> {
        self.repair_database(session_id, Some(archive), cancel)
            .await
    }
    async fn repair_database(
        &self,
        session_id: Uuid,
        archive: Option<PathBuf>,
        cancel: &CancellationToken,
    ) -> Result<SnapshotInfo, ApiError> {
        let sessions = self.session.clone();
        let events = self.events.clone();
        let cancel = cancel.clone();
        tokio::spawn(async move {
            let mut guard = sessions.lock_owned().await;
            let s = guard
                .as_mut()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            if !s.repair_required
                && (archive.is_none()
                    || s.service.as_ref().is_none_or(|service| {
                        service.status().mode != RuntimeMode::DegradedReadonly
                    }))
            {
                return Err(ApiError::new(ErrorCode::Conflict));
            }
            let binding = s
                .config
                .binding
                .ok_or_else(|| ApiError::new(ErrorCode::NoVault))?;
            let native = s
                .native
                .as_ref()
                .filter(|n| n.activated && n.client.auth_status() == AuthStatus::Ready)
                .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?;
            let transport = native.transport.clone();
            s.repair_required = true;
            close_playback(s.playback.take()).await?;
            if let Some(service) = s.service.take() {
                service.shutdown().await?;
            }
            s.reader.take();
            let result = crate::recovery::rebuild_from_backup(
                transport,
                s.lease.database_path(),
                binding,
                s.config.device_id.clone(),
                archive,
                &cancel,
            )
            .await?;
            let path = s.lease.database_path();
            let db = tokio::task::spawn_blocking(move || Database::open(path, binding))
                .await
                .map_err(|_| ApiError::new(ErrorCode::Internal))??;
            s.reader = Some(ProjectionReader::new(db));
            s.repair_required = false;
            start_session_service(s, s.context.clone(), &events);
            Ok(result)
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))?
    }
    pub(crate) async fn job_registry(
        &self,
        session_id: Uuid,
    ) -> Result<crate::jobs::JobRegistry, ApiError> {
        Ok(self.job_access(session_id)?.1)
    }
    pub(crate) fn job_access(
        &self,
        session_id: Uuid,
    ) -> Result<(Context, crate::jobs::JobRegistry), ApiError> {
        let guard = self
            .job_access
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal))?;
        let (context, jobs) = guard
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        if context.session_id != session_id {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        Ok((context.clone(), jobs.clone()))
    }
    /// Local projections remain readable before authorization and after disconnect.
    pub async fn query(
        &self,
        session_id: Uuid,
        query: LocalQuery,
    ) -> Result<LocalResponse, ApiError> {
        if matches!(query, LocalQuery::Device) {
            let guard = self.session.lock().await;
            let s = guard
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            return Ok(LocalResponse::Device(DeviceInfo {
                context: s.context.clone(),
                device_id: s.config.device_id.clone(),
                cache_quota_bytes: s.config.cache_quota_bytes,
            }));
        }
        let reader = self.local_reader(session_id).await?;
        Ok(match query {
            LocalQuery::SourceHealth { track_id } => {
                LocalResponse::Sources(reader.source_health(track_id).await?)
            }
            LocalQuery::Imports { offset, limit } => {
                LocalResponse::Imports(crate::imports::list(&reader, offset, limit).await?)
            }
            LocalQuery::Playlist {
                playlist_id,
                offset,
                limit,
            } => LocalResponse::Playlist(reader.playlist_detail(playlist_id, offset, limit).await?),
            LocalQuery::CommandStatus { op_id } => {
                LocalResponse::CommandStatus(reader.command(op_id).await?.map(Box::new))
            }
            LocalQuery::FavoriteSets { offset, limit } => {
                LocalResponse::FavoriteSets(reader.favorite_sets(offset, limit).await?)
            }
            LocalQuery::Favorites {
                set_name,
                offset,
                limit,
            } => {
                if set_name.len() > 1024 {
                    return Err(ApiError::new(ErrorCode::InvalidInput));
                }
                LocalResponse::Favorites(reader.favorites(set_name, offset, limit).await?)
            }
            LocalQuery::Settings => LocalResponse::Settings(reader.shared_settings().await?),
            LocalQuery::Albums {
                query,
                offset,
                limit,
            } => {
                if query.len() > 1024 {
                    return Err(ApiError::new(ErrorCode::InvalidInput));
                }
                LocalResponse::Albums(reader.albums(query, offset, limit).await?)
            }
            LocalQuery::Artists {
                query,
                offset,
                limit,
            } => {
                if query.len() > 1024 {
                    return Err(ApiError::new(ErrorCode::InvalidInput));
                }
                LocalResponse::Artists(reader.artists(query, offset, limit).await?)
            }
            LocalQuery::Device => return Err(ApiError::new(ErrorCode::Internal)),
        })
    }
    pub async fn play_track(
        &self,
        session_id: Uuid,
        track_id: Uuid,
        source_id: Uuid,
    ) -> Result<(), ApiError> {
        let queue = self.set_queue(session_id, vec![track_id], 0, false).await?;
        let (ticket, cancel) = self.begin_playback(session_id, queue.queue_id).await?;
        self.play_track_inner(session_id, track_id, source_id, ticket, &cancel)
            .await
    }
    async fn play_track_inner(
        &self,
        session_id: Uuid,
        track_id: Uuid,
        source_id: Uuid,
        ticket: Uuid,
        cancel: &CancellationToken,
    ) -> Result<(), ApiError> {
        let (reader, cache, service) = {
            let guard = self.session.lock().await;
            let session = guard
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(session, session_id)?;
            (
                session
                    .reader
                    .clone()
                    .ok_or_else(|| ApiError::new(ErrorCode::NoVault))?,
                session.cache.clone(),
                session.service.clone(),
            )
        };
        let state = reader.state().await.map_err(ApiError::from)?;
        let track = state
            .tracks
            .get(&track_id)
            .filter(|track| track.sources.contains_key(&source_id))
            .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
        let key = format!("{track_id}:{source_id}");
        let lease = tokio::task::spawn_blocking(move || cache.acquire(&key))
            .await
            .map_err(|_| ApiError::new(ErrorCode::Internal))??;
        let input = match lease {
            Some(lease) => vault_media::PlaybackInput::Cached(lease),
            None => {
                service
                    .ok_or_else(|| ApiError::new(ErrorCode::MediaUnavailable))?
                    .playback_input(track_id, source_id, cancel)
                    .await?
            }
        };
        let metadata = vault_media::PlaybackMetadata {
            track_id,
            title: track.metadata.title.clone(),
            performer: track.metadata.performer.clone(),
            duration_ms: track.metadata.duration_ms,
        };
        let mut guard = self.session.lock().await;
        let session = guard
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(session, session_id)?;
        if session.playback_ticket != ticket || cancel.is_cancelled() {
            return Err(ApiError::new(ErrorCode::Cancelled));
        }
        if session.playback.is_none() {
            let platform = self.playback_platform.clone();
            let engine = tokio::task::spawn_blocking(move || match platform {
                Some(platform) => vault_media::PlaybackEngine::open_with_platform(platform),
                None => vault_media::PlaybackEngine::open(),
            })
            .await
            .map_err(|_| ApiError::new(ErrorCode::Internal))??;
            let mut changes = engine.state_changes();
            let events = self.events.clone();
            let context = session.context.clone();
            let weak_session = Arc::downgrade(&self.session);
            let session_transition = self.session_transition.clone();
            let job_access = self.job_access.clone();
            let import_resolver = self.import_resolver.clone();
            let store = self.store.clone();
            let secrets = self.secrets.clone();
            let playback_platform = self.playback_platform.clone();
            let native_path = self.native_path.clone();
            tokio::spawn(async move {
                while changes.changed().await.is_ok() {
                    let value = changes.borrow_and_update().clone();
                    events.emit(&context, EventData::Playback(value.clone()));
                    if let vault_media::PlaybackState::Ended { playback_id, .. } = value
                        && let Some(session) = weak_session.upgrade()
                    {
                        let backend = Backend {
                            store: store.clone(),
                            secrets: secrets.clone(),
                            playback_platform: playback_platform.clone(),
                            native_path: native_path.clone(),
                            events: events.clone(),
                            session,
                            session_transition: session_transition.clone(),
                            job_access: job_access.clone(),
                            import_resolver: import_resolver.clone(),
                        };
                        if let Err(error) = backend
                            .advance_queue(context.session_id, true, Some(playback_id))
                            .await
                            && !matches!(
                                error.code,
                                ErrorCode::Conflict | ErrorCode::Cancelled | ErrorCode::NoProfile
                            )
                        {
                            events.emit(&context, EventData::Warning(error));
                        }
                    }
                }
            });
            session.playback = Some(engine);
        }
        let playback = session
            .playback
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::Internal))?;
        session.playback_id = Some(
            playback
                .play_input_identified(metadata, input)
                .map_err(ApiError::from)?,
        );
        Ok(())
    }
    pub async fn playback_state(
        &self,
        session_id: Uuid,
    ) -> Result<vault_media::PlaybackState, ApiError> {
        let guard = self.session.lock().await;
        let session = guard
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(session, session_id)?;
        Ok(session
            .playback
            .as_ref()
            .map(vault_media::PlaybackEngine::state)
            .unwrap_or(vault_media::PlaybackState::Idle))
    }
    pub async fn control_playback(
        &self,
        session_id: Uuid,
        action: PlaybackAction,
    ) -> Result<vault_media::PlaybackState, ApiError> {
        if matches!(action, PlaybackAction::Next | PlaybackAction::Previous) {
            self.advance_queue(session_id, action == PlaybackAction::Next, None)
                .await?;
            return self.playback_state(session_id).await;
        }
        let mut guard = self.session.lock().await;
        let session = guard
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(session, session_id)?;
        if action == PlaybackAction::Stop {
            session.playback_cancel.cancel();
            session.playback_id = None;
        }
        let playback = session
            .playback
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::Conflict))?;
        match action {
            PlaybackAction::Pause => playback.pause(),
            PlaybackAction::Resume => playback.resume(),
            PlaybackAction::Stop => playback.stop(),
            PlaybackAction::Seek(position_ms) => playback.seek(position_ms),
            PlaybackAction::SetVolume(volume) => playback.set_volume(volume),
            PlaybackAction::Next | PlaybackAction::Previous => {
                return Err(ApiError::new(ErrorCode::Internal));
            }
        }
        .map_err(ApiError::from)?;
        Ok(playback.state())
    }
    pub async fn export_backup(
        &self,
        session_id: Uuid,
        destination: PathBuf,
        kind: vault_snapshot::backup::BackupKind,
    ) -> Result<vault_snapshot::backup::Manifest, ApiError> {
        self.export_backup_cancellable(session_id, destination, kind, &CancellationToken::new())
            .await
    }
    pub async fn export_backup_cancellable(
        &self,
        session_id: Uuid,
        destination: PathBuf,
        kind: vault_snapshot::backup::BackupKind,
        cancel: &CancellationToken,
    ) -> Result<vault_snapshot::backup::Manifest, ApiError> {
        let (reader, cache, owner, transport, lifetime) = {
            let guard = self.session.lock().await;
            let session = guard
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(session, session_id)?;
            let owner = session
                .lease
                .profile()
                .owner_id
                .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?;
            (
                session
                    .reader
                    .clone()
                    .ok_or_else(|| ApiError::new(ErrorCode::NoVault))?,
                session.cache.clone(),
                owner,
                session
                    .native
                    .as_ref()
                    .filter(|n| n.activated)
                    .map(|n| n.transport.clone()),
                session.native.as_ref().map(|n| n.cancel.clone()),
            )
        };
        let state = reader.state().await.map_err(ApiError::from)?;
        let mut leases = Vec::new();
        let mut media = std::collections::BTreeMap::new();
        let mut artwork = std::collections::BTreeMap::new();
        let mut covers = Vec::new();
        if kind == vault_snapshot::backup::BackupKind::Full {
            for (track_id, track) in &state.tracks {
                if cancel.is_cancelled()
                    || lifetime
                        .as_ref()
                        .is_some_and(CancellationToken::is_cancelled)
                {
                    return Err(ApiError::new(ErrorCode::Cancelled));
                }
                let mut found = None;
                for source_id in track.sources.keys() {
                    let key = format!("{track_id}:{source_id}");
                    if let Some(lease) = cache.acquire(&key).map_err(ApiError::from)? {
                        found = Some(lease);
                        break;
                    }
                }
                let lease = match found {
                    Some(lease) => lease,
                    None => {
                        let transport = transport
                            .as_ref()
                            .ok_or_else(|| ApiError::new(ErrorCode::MediaUnavailable))?;
                        let operation = crate::backups::download_source(
                            transport, track, owner, &cache, cancel,
                        );
                        match &lifetime {
                            Some(lifetime) => tokio::select! {
                                _ = lifetime.cancelled() => return Err(ApiError::new(ErrorCode::Cancelled)),
                                result = operation => result?.0,
                            },
                            None => operation.await?.0,
                        }
                    }
                };
                media.insert(*track_id, lease.path.clone());
                if let Some(art) = &track.metadata.artwork {
                    let bytes = if art.source.is_some() {
                        self.artwork(session_id, *track_id, cancel)
                            .await?
                            .ok_or_else(|| ApiError::new(ErrorCode::MediaSourceMissing))?
                            .bytes
                    } else {
                        let path = lease.path.clone();
                        let asset = tokio::task::spawn_blocking(move || {
                            vault_media::embedded_artwork(&path)
                        })
                        .await
                        .map_err(|_| ApiError::new(ErrorCode::Internal))??
                        .ok_or_else(|| ApiError::new(ErrorCode::MediaSourceMissing))?;
                        if asset.sha256 != art.sha256 {
                            return Err(ApiError::new(ErrorCode::Integrity));
                        }
                        asset.bytes
                    };
                    let file = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
                        use std::io::Write;
                        let mut file = tempfile::NamedTempFile::new()?;
                        file.write_all(&bytes)?;
                        Ok(file)
                    })
                    .await
                    .map_err(|_| ApiError::new(ErrorCode::Internal))??;
                    artwork.insert(*track_id, file.path().to_owned());
                    covers.push(file);
                }
                leases.push(lease);
            }
        }
        if cancel.is_cancelled() {
            return Err(ApiError::new(ErrorCode::Cancelled));
        }
        tokio::task::spawn_blocking(move || {
            let _leases = leases;
            let _covers = covers;
            vault_snapshot::backup::export_with_artwork(
                &destination,
                &state,
                owner,
                None,
                kind,
                &media,
                &artwork,
                Default::default(),
            )
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))?
        .map_err(|_| ApiError::new(ErrorCode::Storage))
    }
    pub async fn connect_native(
        &self,
        session_id: Uuid,
        api_id: i32,
        api_hash: zeroize::Zeroizing<String>,
    ) -> Result<AuthStatus, ApiError> {
        let mut guard = self.session.clone().lock_owned().await;
        let s = guard
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        if let Some(native) = &s.native
            && !matches!(
                native.client.auth_status(),
                AuthStatus::Closed | AuthStatus::Closing
            )
        {
            return Ok(native.client.auth_status());
        }
        if let Some(native) = s.native.take() {
            native.cancel.cancel();
            native.client.close().await?;
            if let Some(service) = s.service.take() {
                service.shutdown().await?;
            }
        }
        if s.config.api_id.is_some_and(|saved| saved != api_id) {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        if !vault_telegram::valid_api_credentials(api_id, &api_hash) {
            return Err(ApiError::new(ErrorCode::InvalidInput));
        }
        let secrets = self.secrets.clone();
        // Carry the owned session lock into the worker so a concurrent profile
        // switch cannot interleave key/config persistence with TDLib startup.
        let (mut guard, api_hash, key) = tokio::task::spawn_blocking(move || {
            let s = guard
                .as_mut()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            let item = format!("{}:api-hash", s.lease.profile().profile_id);
            secrets.set(&item, api_hash.as_bytes())?;
            let saved = secrets
                .get(&item)?
                .ok_or_else(|| ApiError::new(ErrorCode::SecureStorageUnavailable))?;
            if saved.as_slice() != api_hash.as_bytes() {
                return Err(ApiError::new(ErrorCode::SecureStorageUnavailable));
            }
            let key = s.lease.database_key(secrets.as_ref())?;
            s.config.api_id = Some(api_id);
            save_config(&s.lease, &s.config)?;
            Ok::<_, ApiError>((guard, api_hash, key))
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))??;
        let s = guard
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        let client = TdClient::open(&self.native_path).await?;
        if client.auth_status() != AuthStatus::ParametersRequired {
            client.close().await.map_err(ApiError::from)?;
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        let parameters = TdParameters {
            api_id,
            api_hash,
            database_key: key,
            database_directory: s.lease.tdlib_path(),
            files_directory: s.lease.files_path(),
        };
        if let Err(error) = parameters.initialize(&client).await {
            let _ = client.close().await;
            return Err(error.into());
        }
        let transport = TdTransport::new(client.clone());
        let state = match client
            .read(
                serde_json::json!({"@type":"getAuthorizationState"}),
                &CancellationToken::new(),
            )
            .await
        {
            Ok(state) => state,
            Err(error) => {
                let _ = client.close().await;
                return Err(error.into());
            }
        };
        let status = AuthStatus::from_td(&state);
        s.native = Some(NativeSession {
            epoch: Uuid::new_v4(),
            client,
            transport,
            activated: false,
            cancel: CancellationToken::new(),
            watcher: None,
        });
        self.events
            .emit(&s.context, EventData::Auth(status.clone()));
        self.watch_authorization(s);
        if status == AuthStatus::Ready {
            activate_session(s, &self.events, &CancellationToken::new()).await?;
        }
        Ok(status)
    }
    fn watch_authorization(&self, session: &mut Session) {
        let Some(native) = &mut session.native else {
            return;
        };
        let mut changes = native.client.authorization_changes();
        let epoch = native.epoch;
        let cancel = native.cancel.clone();
        let context = session.context.clone();
        let sessions = Arc::downgrade(&self.session);
        let events = self.events.clone();
        native.watcher = Some(tokio::spawn(async move {
            let mut previous = None;
            let mut retry = false;
            let mut backoff = std::time::Duration::from_secs(1);
            loop {
                let auth = AuthStatus::from_td(&changes.borrow_and_update());
                if retry || previous.as_ref() != Some(&auth) {
                    retry = false;
                    previous = Some(auth.clone());
                    let Some(sessions) = sessions.upgrade() else {
                        break;
                    };
                    let mut guard = tokio::select! {
                        _ = cancel.cancelled() => break,
                        guard = sessions.lock() => guard,
                    };
                    let Some(session) = guard.as_mut().filter(|s| {
                        s.context.session_id == context.session_id
                            && s.native.as_ref().is_some_and(|n| n.epoch == epoch)
                    }) else {
                        break;
                    };
                    // An auth transition may supersede this value while waiting
                    // for a session operation. Never activate from a stale Ready.
                    if session
                        .native
                        .as_ref()
                        .is_some_and(|n| n.client.auth_status() == auth)
                    {
                        events.emit(&session.context, EventData::Auth(auth.clone()));
                        let result = if auth == AuthStatus::Ready {
                            activate_session(session, &events, &cancel).await
                        } else {
                            if let Some(native) = &mut session.native {
                                native.activated = false;
                            }
                            let result = if let Some(service) = session.service.take() {
                                service.shutdown().await
                            } else {
                                Ok(())
                            };
                            events.emit(
                                &session.context,
                                EventData::Runtime(Status {
                                    context: session.context.clone(),
                                    mode: RuntimeMode::AuthRequired,
                                    vault: None,
                                }),
                            );
                            result
                        };
                        if let Err(error) = result {
                            retry = auth == AuthStatus::Ready
                                && (error.retryable
                                    || matches!(
                                        error.code,
                                        ErrorCode::Timeout | ErrorCode::Backpressure
                                    ));
                            events.emit(&session.context, EventData::Warning(error));
                        } else {
                            backoff = std::time::Duration::from_secs(1);
                        }
                    }
                }
                tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = tokio::time::sleep(backoff), if retry => { backoff = (backoff * 2).min(std::time::Duration::from_secs(30)); },
                    changed = changes.changed() => if changed.is_err() { break; },
                }
            }
        }));
    }
    pub async fn authenticate(
        &self,
        session_id: Uuid,
        input: AuthInput,
        cancel: &CancellationToken,
    ) -> Result<AuthStatus, ApiError> {
        if cancel.is_cancelled() {
            return Err(ApiError::new(ErrorCode::Cancelled));
        }
        let (client, context, epoch) = {
            let guard = self.session.lock().await;
            let s = guard
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            let native = s
                .native
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?;
            (native.client.clone(), s.context.clone(), native.epoch)
        };
        let status = client.authenticate_cancellable(input, cancel).await?;
        {
            let guard = self.session.lock().await;
            let session = guard
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(session, session_id)?;
            if !session
                .native
                .as_ref()
                .is_some_and(|native| native.epoch == epoch)
            {
                return Err(ApiError::new(ErrorCode::Closed));
            }
        }
        self.events.emit(&context, EventData::Auth(status.clone()));
        if status == AuthStatus::Ready {
            self.activate_account(session_id, cancel).await?;
        }
        Ok(status)
    }
    async fn activate_account(
        &self,
        session_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<(), ApiError> {
        let mut guard = self.session.lock().await;
        let s = guard
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        activate_session(s, &self.events, cancel).await
    }
    pub async fn auth_status(&self, session_id: Uuid) -> Result<AuthStatus, ApiError> {
        let guard = self.session.lock().await;
        let s = guard
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        let status = s
            .native
            .as_ref()
            .map(|n| n.client.auth_status())
            .unwrap_or(AuthStatus::ParametersRequired);
        let ready = status == AuthStatus::Ready;
        drop(guard);
        if ready {
            self.activate_account(session_id, &CancellationToken::new())
                .await?;
        }
        Ok(status)
    }
    /// Subscribe after native initialization and before requesting QR authentication.
    /// The stream coalesces updates and is scoped to this native session.
    pub async fn auth_updates(
        &self,
        session_id: Uuid,
    ) -> Result<vault_telegram::AuthUpdates, ApiError> {
        let guard = self.session.lock().await;
        let session = guard
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(session, session_id)?;
        Ok(session
            .native
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?
            .client
            .auth_updates())
    }
    /// Account identity is read only after Ready and the profile owner check.
    pub async fn authorized_account(
        &self,
        session_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<AccountInfo, ApiError> {
        let mut guard = self.session.lock().await;
        let session = guard
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(session, session_id)?;
        activate_session(session, &self.events, cancel).await?;
        let native = session
            .native
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?;
        let me = native
            .client
            .read(serde_json::json!({"@type":"getMe"}), cancel)
            .await?;
        if native.client.auth_status() != AuthStatus::Ready {
            return Err(ApiError::new(ErrorCode::AuthRequired));
        }
        let user_id = vault_domain::UserId::new(td_integer(&me["id"])?)
            .map_err(|_| ApiError::new(ErrorCode::Integrity))?;
        if session.lease.profile().owner_id != Some(user_id) {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        let display_name = [
            me["first_name"].as_str().unwrap_or(""),
            me["last_name"].as_str().unwrap_or(""),
        ]
        .join(" ")
        .trim()
        .to_owned();
        Ok(AccountInfo {
            user_id,
            display_name,
        })
    }
    pub async fn discover_vaults(
        &self,
        session_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<Vec<vault_telegram::VaultCandidate>, ApiError> {
        let report = self
            .discover_vaults_with_options(session_id, false, None, cancel)
            .await?;
        if let Some(error) = report.failure {
            return Err(error.into());
        }
        if !report.complete {
            return Err(vault_telegram::TelegramError::IncompleteHistory.into());
        }
        Ok(report.candidates)
    }

    pub async fn discover_vaults_with_options(
        &self,
        session_id: Uuid,
        deep: bool,
        progress: Option<&tokio::sync::watch::Sender<vault_telegram::DiscoveryProgress>>,
        cancel: &CancellationToken,
    ) -> Result<vault_telegram::DiscoveryReport, ApiError> {
        let (transport, owner, saved, session_cancel) = {
            let guard = self.session.lock().await;
            let s = guard
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            let owner = s
                .lease
                .profile()
                .owner_id
                .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?;
            let native = s
                .native
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?;
            let saved = s.config.binding.map(|b| vault_telegram::SavedVault {
                chat_id: b.chat_id,
                owner_id: b.owner_id,
                vault_id: b.vault_id,
                init_message_id: s.config.init_message_id,
            });
            (
                native.transport.clone(),
                owner,
                saved,
                native.cancel.clone(),
            )
        };
        let report = tokio::select! {
            biased;
            _ = session_cancel.cancelled() => return Err(ApiError::new(ErrorCode::Cancelled)),
            result = transport.discover_with_options(owner, vault_telegram::DiscoveryOptions { deep, saved }, progress, cancel) => result?,
        };
        // Save lookup hints, never select or attach a discovered vault. Recheck
        // the session after I/O so results cannot leak into a switched profile.
        let mut guard = self.session.lock().await;
        let s = guard
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        for candidate in &report.candidates {
            s.config.remember_discovered_vault(candidate);
        }
        if !report.candidates.is_empty() {
            save_config(&s.lease, &s.config)?;
        }
        Ok(report)
    }
    pub async fn attach_vault(
        &self,
        session_id: Uuid,
        chat_id: vault_domain::ChatId,
        cancel: &CancellationToken,
    ) -> Result<Status, ApiError> {
        let (transport, owner, context, hint) = {
            let g = self.session.lock().await;
            let s = g
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            (
                s.native
                    .as_ref()
                    .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?
                    .transport
                    .clone(),
                s.lease
                    .profile()
                    .owner_id
                    .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?,
                s.context.clone(),
                s.config
                    .discovery_hints
                    .iter()
                    .find(|hint| {
                        hint.chat_id == chat_id && Some(hint.owner_id) == s.lease.profile().owner_id
                    })
                    .copied(),
            )
        };
        let candidate = vault_telegram::inspect_vault_with_hint(
            &transport,
            chat_id,
            owner,
            hint.map(|h| h.vault_id),
            hint.and_then(|h| h.init_message_id),
            cancel,
        )
        .await?;
        let binding = Binding {
            vault_id: candidate.init.vault_id,
            owner_id: owner,
            chat_id,
        };
        let mut g = self.session.lock().await;
        let s = g
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        if s.config.binding.is_some_and(|existing| existing != binding) {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        s.config.binding = Some(binding);
        s.config.init_message_id = Some(candidate.init_message_id);
        save_config(&s.lease, &s.config)?;
        let db = Database::open(s.lease.database_path(), binding)?;
        s.reader = Some(ProjectionReader::new(db));
        let status = self.start_service(s, context);
        Ok(status)
    }
    pub async fn create_vault(
        &self,
        session_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<Status, ApiError> {
        self.create_vault_named(session_id, None, cancel).await
    }
    pub async fn create_vault_named(
        &self,
        session_id: Uuid,
        title: Option<String>,
        cancel: &CancellationToken,
    ) -> Result<Status, ApiError> {
        if title.as_ref().is_some_and(|title| {
            title.trim().is_empty() || title.chars().count() > 128 || title.contains('\0')
        }) {
            return Err(ApiError::new(ErrorCode::InvalidInput));
        }
        let (transport, owner) = {
            let mut g = self.session.lock().await;
            let s = g
                .as_mut()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            if s.config.creation.is_some() || s.config.binding.is_some() {
                return Err(ApiError::new(ErrorCode::Conflict));
            }
            let owner = s
                .lease
                .profile()
                .owner_id
                .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?;
            s.config.creation = Some(CreateIntent {
                vault_id: Uuid::new_v4(),
                stage: CreateStage::CreatingUnknown,
                chat_id: None,
            });
            save_config(&s.lease, &s.config)?;
            (
                s.native
                    .as_ref()
                    .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?
                    .transport
                    .clone(),
                owner,
            )
        };
        // A lost create response cannot safely be retried; retain the durable intent
        // until the user explicitly resumes by selecting the resulting channel.
        let channel = transport
            .create_channel_named(owner, title, cancel)
            .await
            .map_err(ApiError::from)?;
        self.resume_created_channel(session_id, channel, cancel)
            .await?;
        let context = self
            .context()
            .await
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        self.events
            .emit(&context, EventData::Runtime(self.runtime_status().await?));
        let _ = self
            .service_request(session_id, VaultRequest::Initialize, cancel)
            .await?;
        self.complete_vault_creation(session_id).await?;
        self.runtime_status().await
    }
    pub async fn resume_vault_creation(
        &self,
        session_id: Uuid,
        chat_id: vault_domain::ChatId,
        cancel: &CancellationToken,
    ) -> Result<Status, ApiError> {
        self.resume_created_channel(
            session_id,
            self.require_transport(session_id)
                .await?
                .verify_channel(chat_id, self.owner(session_id).await?, cancel)
                .await?,
            cancel,
        )
        .await?;
        let _ = self
            .service_request(session_id, VaultRequest::Initialize, cancel)
            .await?;
        self.complete_vault_creation(session_id).await?;
        self.runtime_status().await
    }
    async fn resume_created_channel(
        &self,
        session_id: Uuid,
        channel: ChannelInfo,
        cancel: &CancellationToken,
    ) -> Result<(), ApiError> {
        let mut g = self.session.lock().await;
        let s = g
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        let intent = s
            .config
            .creation
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::Conflict))?;
        intent.stage = CreateStage::ChannelCreated;
        intent.chat_id = Some(channel.chat_id);
        let vault = intent.vault_id;
        let binding = Binding {
            vault_id: vault,
            owner_id: channel.owner_id,
            chat_id: channel.chat_id,
        };
        s.config.binding = Some(binding);
        save_config(&s.lease, &s.config)?;
        let db = Database::open(s.lease.database_path(), binding)?;
        s.reader = Some(ProjectionReader::new(db));
        let context = s.context.clone();
        self.start_service(s, context);
        drop(g);
        // Detect a prior successful INIT before retrying an upload whose ACK was lost.
        let transport = self.require_transport(session_id).await?;
        let history =
            vault_telegram::scan_history(&transport, channel.chat_id, 0, None, 100_000, cancel)
                .await?;
        let mut init_exists = None;
        for message in history {
            if let Some(record) = message.record()? {
                if record.vault_id() != vault {
                    return Err(ApiError::new(ErrorCode::Integrity));
                }
                match record {
                    Record::Init(marker)
                        if marker.owner_telegram_user_id == channel.owner_id
                            && init_exists.is_none() =>
                    {
                        init_exists = Some(message.id)
                    }
                    Record::Init(_) => return Err(ApiError::new(ErrorCode::Integrity)),
                    _ => return Err(ApiError::new(ErrorCode::Conflict)),
                }
            }
        }
        if let Some(id) = init_exists {
            self.save_init_hint(session_id, id).await?;
            return Ok(());
        }
        let init = Record::Init(VaultInit {
            vault_id: vault,
            owner_telegram_user_id: channel.owner_id,
            created_by: "kiyora".into(),
            created_at_utc: utc_now(),
        });
        {
            let mut g = self.session.lock().await;
            let s = g
                .as_mut()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            if let Some(intent) = s.config.creation.as_mut() {
                intent.stage = CreateStage::InitUnknown;
            }
            save_config(&s.lease, &s.config)?;
        }
        let message = transport
            .send_text(
                channel.chat_id,
                encode_text(&init).map_err(|_| ApiError::new(ErrorCode::Integrity))?,
                cancel,
            )
            .await?;
        self.save_init_hint(session_id, message.id).await?;
        Ok(())
    }

    async fn save_init_hint(
        &self,
        session_id: Uuid,
        id: vault_domain::MessageId,
    ) -> Result<(), ApiError> {
        let mut guard = self.session.lock().await;
        let s = guard
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        s.config.init_message_id = Some(id);
        save_config(&s.lease, &s.config)
    }

    pub async fn complete_vault_creation(&self, session_id: Uuid) -> Result<(), ApiError> {
        let mut g = self.session.lock().await;
        let s = g
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        if let Some(intent) = s.config.creation.as_mut() {
            intent.stage = CreateStage::Initialized;
            save_config(&s.lease, &s.config)?;
            Ok(())
        } else {
            Err(ApiError::new(ErrorCode::Conflict))
        }
    }
    async fn owner(&self, session_id: Uuid) -> Result<vault_domain::UserId, ApiError> {
        let g = self.session.lock().await;
        let s = g
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        s.lease
            .profile()
            .owner_id
            .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))
    }
    async fn require_transport(&self, session_id: Uuid) -> Result<TdTransport, ApiError> {
        let g = self.session.lock().await;
        let s = g
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        Ok(s.native
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?
            .transport
            .clone())
    }
    fn start_service(&self, s: &mut Session, context: Context) -> Status {
        start_session_service(s, context, &self.events)
    }
    pub async fn service_request(
        &self,
        session_id: Uuid,
        mut request: VaultRequest,
        cancel: &CancellationToken,
    ) -> Result<VaultResponse, ApiError> {
        if cancel.is_cancelled() {
            return Err(ApiError::new(ErrorCode::Cancelled));
        }
        if matches!(
            request,
            VaultRequest::GetCacheStats
                | VaultRequest::ClearLocalCache
                | VaultRequest::PinCachedMedia { .. }
                | VaultRequest::SetCacheQuota { .. }
                | VaultRequest::GetDownloadStatus { .. }
        ) {
            // Keep the profile lease until disk work finishes, even if the caller
            // drops its future. A new session cannot race the old cache writer.
            let mut guard = self.session.clone().lock_owned().await;
            let s = guard
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            if let VaultRequest::GetDownloadStatus {
                download_id,
                track_id,
                source_id,
            } = &request
                && let Some(service) = &s.service
            {
                return Ok(VaultResponse::Download(service.download_status(
                    *download_id,
                    *track_id,
                    *source_id,
                )?));
            }
            return tokio::task::spawn_blocking(move || {
                let s = guard
                    .as_mut()
                    .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
                Ok(match request {
                    VaultRequest::GetCacheStats => VaultResponse::CacheStats(s.cache.stats()?),
                    VaultRequest::ClearLocalCache => VaultResponse::CacheCleared {
                        bytes_removed: s.cache.clear_unpinned()?,
                    },
                    VaultRequest::PinCachedMedia {
                        track_id,
                        source_id,
                        pinned,
                    } => {
                        s.cache.pin(&media_cache_key(track_id, source_id), pinned)?;
                        VaultResponse::CachePinned
                    }
                    VaultRequest::SetCacheQuota { bytes } => {
                        if bytes == 0 {
                            return Err(ApiError::new(ErrorCode::InvalidInput));
                        }
                        let stats = s.cache.set_quota(bytes)?;
                        let previous = std::mem::replace(&mut s.config.cache_quota_bytes, bytes);
                        if let Err(error) = save_config(&s.lease, &s.config) {
                            s.config.cache_quota_bytes = previous;
                            let _ = s.cache.set_quota(previous);
                            return Err(error);
                        }
                        VaultResponse::CacheStats(stats)
                    }
                    VaultRequest::GetDownloadStatus {
                        download_id,
                        track_id,
                        source_id,
                    } => VaultResponse::Download(crate::service::cached_download_status(
                        Some(&s.cache),
                        download_id,
                        track_id,
                        source_id,
                    )?),
                    _ => return Err(ApiError::new(ErrorCode::Internal)),
                })
            })
            .await
            .map_err(|_| ApiError::new(ErrorCode::Internal))?;
        }
        let service = {
            let g = self.session.lock().await;
            let s = g
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            s.service
                .clone()
                .ok_or_else(|| ApiError::new(ErrorCode::NoVault))?
        };
        let mut document = None;
        if let VaultRequest::Import { path, op_id, .. } = &mut request
            && path.starts_with("content://")
            && !crate::imports::has_durable_import(&service.reader(), *op_id).await?
        {
            let resolver = self
                .import_resolver
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            let token = cancel.child_token();
            let lifetime = self.job_registry(session_id).await?.cancellation();
            let future = resolver.resolve(path.clone(), *op_id, token.clone());
            tokio::pin!(future);
            let resolved = tokio::select! {
                result=&mut future=>result?,
                _=lifetime.cancelled()=>{token.cancel();let _=future.await;return Err(ApiError::new(ErrorCode::Cancelled));},
            };
            *path = resolved
                .to_str()
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?
                .to_owned();
            document = Some(*op_id);
        }
        let result = service.request(request, cancel).await;
        let complete = matches!(&result, Ok(VaultResponse::ImportReview { .. }))
            || matches!(&result,Ok(VaultResponse::Command(command)) if command.status==vault_db::CommandStatus::Applied);
        if let Some(op) = document
            && complete
            && let Some(resolver) = &self.import_resolver
        {
            let _ = resolver.release(op).await;
        }
        result
    }
    pub async fn disconnect(&self, session_id: Uuid) -> Result<(), ApiError> {
        let native = {
            let mut g = self.session.lock().await;
            let s = g
                .as_mut()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            close_playback(s.playback.take()).await?;
            if let Some(service) = s.service.take() {
                service.shutdown().await?;
            }
            s.native.take()
        };
        if let Some(native) = native {
            native.client.close().await.map_err(ApiError::from)?;
        }
        Ok(())
    }
    pub async fn logout(&self, session_id: Uuid) -> Result<(), ApiError> {
        let (native, context) = {
            let mut guard = self.session.lock().await;
            let session = guard
                .as_mut()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(session, session_id)?;
            close_playback(session.playback.take()).await?;
            if let Some(service) = session.service.take() {
                service.shutdown().await?;
            }
            (session.native.take(), session.context.clone())
        };
        if let Some(native) = native
            && let Err(error) = native.client.logout().await
        {
            let _ = native.client.close().await;
            return Err(ApiError::from(error));
        }
        self.events
            .emit(&context, EventData::Auth(AuthStatus::Closed));
        Ok(())
    }
    pub async fn shutdown(&self) -> Result<(), ApiError> {
        let transition = self.session_transition.clone().lock_owned().await;
        let backend = self.clone();
        tokio::spawn(async move {
            let _transition = transition;
            backend.shutdown_inner().await
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))?
    }
    async fn shutdown_inner(&self) -> Result<(), ApiError> {
        if let Some((_, jobs)) = self
            .job_access
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal))?
            .take()
        {
            jobs.cancel_all();
        }
        let session = self.session.lock().await.take();
        if let Some(session) = session {
            close_session(session).await?;
        }
        Ok(())
    }
}
async fn activate_session(
    s: &mut Session,
    events: &EventBus,
    cancel: &CancellationToken,
) -> Result<(), ApiError> {
    let native = s
        .native
        .as_mut()
        .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?;
    if native.client.auth_status() != AuthStatus::Ready {
        return Err(ApiError::new(ErrorCode::AuthRequired));
    }
    let first = !native.activated;
    if first {
        let me = native
            .client
            .read(serde_json::json!({"@type":"getMe"}), cancel)
            .await?;
        let user_id = vault_domain::UserId::new(td_integer(&me["id"])?)
            .map_err(|_| ApiError::new(ErrorCode::Integrity))?;
        if s.lease
            .profile()
            .owner_id
            .is_some_and(|owner| owner != user_id)
            || s.config.binding.is_some_and(|b| b.owner_id != user_id)
        {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        // Recheck after getMe: a revocation queued during the request cannot
        // open the writable service with an already closed authorization.
        if native.client.auth_status() != AuthStatus::Ready {
            return Err(ApiError::new(ErrorCode::AuthRequired));
        }
        s.lease.bind_owner(user_id)?;
        native.activated = true;
    }
    if s.config.binding.is_some() && s.reader.is_some() && s.service.is_none() {
        start_session_service(s, s.context.clone(), events);
    } else if first && s.service.is_none() {
        events.emit(
            &s.context,
            EventData::Runtime(Status {
                context: s.context.clone(),
                mode: if s.repair_required {
                    RuntimeMode::DegradedReadonly
                } else {
                    RuntimeMode::Discovering
                },
                vault: None,
            }),
        );
    }
    Ok(())
}

fn start_session_service(s: &mut Session, mut context: Context, events: &EventBus) -> Status {
    if let Some(service) = &s.service {
        return service.status();
    }
    context.vault_id = s.config.binding.map(|b| b.vault_id);
    let status = match (s.native.as_ref().filter(|n| n.activated), s.reader.clone()) {
        (Some(native), Some(reader)) => {
            let engine = SyncEngine::with_projection(
                native.transport.clone(),
                reader,
                s.config.device_id.clone(),
            );
            let service = VaultService::start_with_cache(
                engine,
                context.clone(),
                events.clone(),
                s.cache.clone(),
            );
            let status = service.status();
            s.service = Some(service);
            status
        }
        _ => Status {
            context: context.clone(),
            mode: RuntimeMode::AuthRequired,
            vault: None,
        },
    };
    s.context = context;
    events.emit(&s.context, EventData::Runtime(status.clone()));
    status
}

async fn close_session(mut session: Session) -> Result<(), ApiError> {
    session.playback_cancel.cancel();
    session.jobs.shutdown().await;
    close_playback(session.playback.take()).await?;
    if let Some(service) = session.service.take() {
        service.shutdown().await?;
    }
    if let Some(native) = session.native.take() {
        native.client.close().await.map_err(ApiError::from)?;
    }
    Ok(())
}
async fn close_playback(playback: Option<vault_media::PlaybackEngine>) -> Result<(), ApiError> {
    if let Some(playback) = playback {
        tokio::task::spawn_blocking(move || drop(playback))
            .await
            .map_err(|_| ApiError::new(ErrorCode::Internal))?;
    }
    Ok(())
}
fn check_session(session: &Session, expected: Uuid) -> Result<(), ApiError> {
    if session.context.session_id != expected {
        Err(ApiError::new(ErrorCode::Conflict))
    } else {
        Ok(())
    }
}
fn load_config(lease: &ProfileLease) -> Result<ProfileConfig, ApiError> {
    let path = lease.directory().join("backend.json");
    if !path.exists() {
        return Ok(ProfileConfig::fresh());
    }
    let metadata = std::fs::symlink_metadata(&path)?;
    if metadata.file_type().is_symlink() || metadata.len() > 16 * 1024 {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    let config: ProfileConfig = serde_json::from_slice(&std::fs::read(path)?)
        .map_err(|_| ApiError::new(ErrorCode::Integrity))?;
    if config.schema != 2 || config.device_id.is_empty() || config.cache_quota_bytes == 0 {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    Ok(config)
}
fn save_config(lease: &ProfileLease, config: &ProfileConfig) -> Result<(), ApiError> {
    let bytes = serde_json::to_vec(config).map_err(|_| ApiError::new(ErrorCode::Internal))?;
    let dir = lease.directory();
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    use std::io::Write;
    temp.write_all(&bytes)?;
    temp.as_file().sync_all()?;
    temp.persist(dir.join("backend.json"))
        .map_err(|e| ApiError::from(e.error))?;
    #[cfg(unix)]
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}
fn td_integer(value: &serde_json::Value) -> Result<i64, ApiError> {
    value
        .as_i64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .ok_or_else(|| ApiError::new(ErrorCode::Integrity))
}
fn utc_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let days = (now / 86400) as i64;
    let (year, month, day) = civil_from_days(days);
    let secs = now % 86400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    y += i64::from(m <= 2);
    (y, m as u32, d as u32)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod discovery_config_tests {
    use super::*;
    use vault_domain::{ChatId, MessageId, UserId};

    #[test]
    fn old_profile_config_loads_without_discovery_hints() {
        let old = serde_json::json!({"schema":2,"device_id":"old-device","api_id":null,"binding":null,"creation":null});
        let config: ProfileConfig = serde_json::from_value(old).unwrap();
        assert!(config.init_message_id.is_none());
        assert!(config.discovery_hints.is_empty());
        assert!(
            serde_json::to_value(config)
                .unwrap()
                .get("discovery_hints")
                .is_none()
        );
    }

    #[test]
    fn hints_stay_with_the_profile_and_never_replace_a_selected_binding() {
        let mut config = ProfileConfig::fresh();
        let owner = UserId::new(42).unwrap();
        let binding = Binding {
            chat_id: ChatId::new(-100).unwrap(),
            owner_id: owner,
            vault_id: Uuid::from_u128(1),
        };
        config.binding = Some(binding);
        let mut candidate = vault_telegram::VaultCandidate {
            protocol: vault_protocol::PROTOCOL,
            channel: ChannelInfo {
                chat_id: binding.chat_id,
                owner_id: owner,
                title: "renamed".into(),
            },
            init: VaultInit {
                vault_id: binding.vault_id,
                owner_telegram_user_id: owner,
                created_by: "kiyora".into(),
                created_at_utc: "2026-10-25T00:00:00Z".into(),
            },
            init_message_id: MessageId::new(2 << 20).unwrap(),
        };
        config.remember_discovered_vault(&candidate);
        assert_eq!(config.init_message_id, Some(candidate.init_message_id));
        for n in 101..301 {
            candidate.channel.chat_id = ChatId::new(-n).unwrap();
            candidate.init.vault_id = Uuid::from_u128(n as u128);
            config.remember_discovered_vault(&candidate);
        }
        assert_eq!(config.binding, Some(binding));
        assert_eq!(config.discovery_hints.len(), 40);
        assert_eq!(config.init_message_id, Some(candidate.init_message_id));
        let bytes = serde_json::to_vec(&config).unwrap();
        assert!(bytes.len() < 16 * 1024);
        let restored: ProfileConfig = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(restored.discovery_hints.len(), 40);
        assert!(ProfileConfig::fresh().discovery_hints.is_empty());
    }
}
