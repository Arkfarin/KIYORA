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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfileConfig {
    schema: u32,
    device_id: String,
    api_id: Option<i32>,
    binding: Option<Binding>,
    creation: Option<CreateIntent>,
}
impl ProfileConfig {
    fn fresh() -> Self {
        Self {
            schema: 2,
            device_id: Uuid::new_v4().to_string(),
            api_id: None,
            binding: None,
            creation: None,
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
    client: TdClient,
    transport: TdTransport,
}
struct Session {
    lease: ProfileLease,
    config: ProfileConfig,
    native: Option<NativeSession>,
    service: Option<VaultService>,
    reader: Option<ProjectionReader>,
    cache: vault_media::CacheManager,
    context: Context,
}
#[derive(Clone)]
pub struct Backend {
    store: Arc<ProfileStore>,
    native_path: PathBuf,
    events: EventBus,
    session: Arc<Mutex<Option<Session>>>,
}
impl Backend {
    pub fn new(store: ProfileStore, native_path: impl Into<PathBuf>) -> Self {
        Self {
            store: Arc::new(store),
            native_path: native_path.into(),
            events: EventBus::default(),
            session: Arc::new(Mutex::new(None)),
        }
    }
    pub fn default_backend() -> Result<Self, ApiError> {
        let store = match std::env::var_os("KIYORA_DATA_DIR") {
            Some(path) => ProfileStore::at(path)?,
            None => ProfileStore::default_location()?,
        };
        Ok(Self::new(store, native_library_path()))
    }
    pub fn events(&self) -> EventBus {
        self.events.clone()
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
        // Acquire the new process lock and validate its durable binding before closing
        // the current account, so a failed switch leaves the old session usable.
        let store = self.store.clone();
        let (lease, config, reader, cache) = tokio::task::spawn_blocking(move || {
            let lease = store.acquire(id)?;
            let config = load_config(&lease)?;
            save_config(&lease, &config)?;
            let reader = match config.binding {
                Some(binding) => Some(ProjectionReader::new(Database::open(
                    lease.database_path(),
                    binding,
                )?)),
                None => None,
            };
            let cache =
                vault_media::CacheManager::open(lease.cache_path(), 10 * 1024 * 1024 * 1024)
                    .map_err(ApiError::from)?;
            Ok::<_, ApiError>((lease, config, reader, cache))
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))??;
        if let Some(old) = self.session.lock().await.take() {
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
            lease,
            config,
            native: None,
            service: None,
            reader,
            cache,
            context: context.clone(),
        };
        *self.session.lock().await = Some(session);
        self.events
            .emit(&context, EventData::Runtime(status.clone()));
        // Restore TDLib only when this profile already has an OS-protected API hash.
        // A missing key keeps SQLite readable and asks for explicit authentication.
        if let Some(api_id) = api_id {
            let item = format!("{id}:api-hash");
            match OsSecrets.get(&item)? {
                Some(hash) => {
                    let auth = self
                        .connect_native(
                            context.session_id,
                            api_id,
                            Zeroizing::new(String::from_utf8_lossy(&hash).into_owned()),
                        )
                        .await?;
                    if auth == AuthStatus::Ready {
                        self.activate_account(context.session_id, &CancellationToken::new())
                            .await?;
                    }
                }
                None => self.events.emit(
                    &context,
                    EventData::Warning(ApiError::new(ErrorCode::SecureStorageUnavailable)),
                ),
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
                mode: RuntimeMode::AuthRequired,
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
            .ok_or_else(|| ApiError::new(ErrorCode::NoVault))
    }
    pub async fn connect_native(
        &self,
        session_id: Uuid,
        api_id: i32,
        api_hash: zeroize::Zeroizing<String>,
    ) -> Result<AuthStatus, ApiError> {
        let mut guard = self.session.lock().await;
        let s = guard
            .as_mut()
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        if let Some(native) = &s.native {
            return Ok(native.client.auth_status());
        }
        if s.config.api_id.is_some_and(|saved| saved != api_id) {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        if api_id <= 0 || api_hash.len() != 32 {
            return Err(ApiError::new(ErrorCode::InvalidInput));
        }
        let item = format!("{}:api-hash", s.lease.profile().profile_id);
        OsSecrets.set(&item, api_hash.as_bytes())?;
        let saved = OsSecrets
            .get(&item)?
            .ok_or_else(|| ApiError::new(ErrorCode::SecureStorageUnavailable))?;
        if saved.as_slice() != api_hash.as_bytes() {
            return Err(ApiError::new(ErrorCode::SecureStorageUnavailable));
        }
        s.config.api_id = Some(api_id);
        save_config(&s.lease, &s.config)?;
        let key = s.lease.database_key(&OsSecrets)?;
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
        let state = client
            .read(
                serde_json::json!({"@type":"getAuthorizationState"}),
                &CancellationToken::new(),
            )
            .await?;
        let status = AuthStatus::from_td(&state);
        s.native = Some(NativeSession { client, transport });
        self.events
            .emit(&s.context, EventData::Auth(status.clone()));
        Ok(status)
    }
    pub async fn authenticate(
        &self,
        session_id: Uuid,
        input: AuthInput,
        cancel: &CancellationToken,
    ) -> Result<AuthStatus, ApiError> {
        let status = {
            let guard = self.session.lock().await;
            let s = guard
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            let native = s
                .native
                .as_ref()
                .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?;
            native.client.authenticate(input).await?
        };
        self.events.emit(
            &self
                .context()
                .await
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?,
            EventData::Auth(status.clone()),
        );
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
        let native = s
            .native
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?;
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
        {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        s.lease.bind_owner(user_id)?;
        if let Some(binding) = s.config.binding
            && binding.owner_id != user_id
        {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        if s.config.binding.is_some() && s.reader.is_some() {
            let context = s.context.clone();
            self.start_service(s, context);
        }
        Ok(())
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
    pub async fn discover_vaults(
        &self,
        session_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<Vec<vault_telegram::VaultCandidate>, ApiError> {
        let (transport, owner) = {
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
            (
                s.native
                    .as_ref()
                    .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?
                    .transport
                    .clone(),
                owner,
            )
        };
        let context = self
            .context()
            .await
            .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        self.events
            .emit(&context, EventData::Sync { running: true });
        let result = transport
            .discover(owner, cancel)
            .await
            .map_err(ApiError::from);
        self.events
            .emit(&context, EventData::Sync { running: false });
        result
    }
    pub async fn attach_vault(
        &self,
        session_id: Uuid,
        chat_id: vault_domain::ChatId,
        cancel: &CancellationToken,
    ) -> Result<Status, ApiError> {
        let (transport, owner, context) = {
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
            )
        };
        let candidate =
            vault_telegram::inspect_vault(&transport, chat_id, owner, None, cancel).await?;
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
        let (transport, owner) = {
            let mut g = self.session.lock().await;
            let s = g
                .as_mut()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            if s.config.creation.is_some() {
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
            .create_channel(owner, cancel)
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
        // Detect a prior successful INIT before retrying an upload whose ACK was lost.
        let transport = self.require_transport(session_id).await?;
        let history =
            vault_telegram::scan_history(&transport, channel.chat_id, 0, None, 100_000, cancel)
                .await?;
        let mut init_exists = false;
        for message in history {
            if let Some(record) = message.record()? {
                if record.vault_id() != vault {
                    return Err(ApiError::new(ErrorCode::Integrity));
                }
                match record {
                    Record::Init(marker)
                        if marker.owner_telegram_user_id == channel.owner_id && !init_exists =>
                    {
                        init_exists = true
                    }
                    Record::Init(_) => return Err(ApiError::new(ErrorCode::Integrity)),
                    _ => return Err(ApiError::new(ErrorCode::Conflict)),
                }
            }
        }
        if init_exists {
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
        transport
            .send_text(
                channel.chat_id,
                encode_text(&init).map_err(|_| ApiError::new(ErrorCode::Integrity))?,
                cancel,
            )
            .await?;
        Ok(())
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
        let native = s
            .native
            .as_ref()
            .ok_or_else(|| ApiError::new(ErrorCode::AuthRequired));
        let status = match (native, s.reader.clone()) {
            (Ok(native), Some(reader)) => {
                let engine = SyncEngine::with_projection(
                    native.transport.clone(),
                    reader.clone(),
                    s.config.device_id.clone(),
                );
                let service = VaultService::start_with_cache(
                    engine,
                    context.clone(),
                    self.events.clone(),
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
        self.events
            .emit(&s.context, EventData::Runtime(status.clone()));
        status
    }
    pub async fn service_request(
        &self,
        session_id: Uuid,
        request: VaultRequest,
        cancel: &CancellationToken,
    ) -> Result<VaultResponse, ApiError> {
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
        service.request(request, cancel).await
    }
    pub async fn disconnect(&self, session_id: Uuid) -> Result<(), ApiError> {
        let native = {
            let mut g = self.session.lock().await;
            let s = g
                .as_mut()
                .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
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
    pub async fn shutdown(&self) -> Result<(), ApiError> {
        let session = self.session.lock().await.take();
        if let Some(session) = session {
            close_session(session).await?;
        }
        Ok(())
    }
}
async fn close_session(mut session: Session) -> Result<(), ApiError> {
    if let Some(service) = session.service.take() {
        service.shutdown().await?;
    }
    if let Some(native) = session.native.take() {
        native.client.close().await.map_err(ApiError::from)?;
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
    if config.schema != 2 || config.device_id.is_empty() {
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
fn native_library_path() -> PathBuf {
    if let Some(value) = std::env::var_os("KIYORA_TDJSON") {
        return value.into();
    }
    if cfg!(target_os = "windows") {
        "tdjson.dll".into()
    } else {
        "libtdjson.so".into()
    }
}
