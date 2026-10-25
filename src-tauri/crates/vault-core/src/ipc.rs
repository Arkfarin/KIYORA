//! Versioned application boundary used directly by Tauri and schema generation.
use crate::*;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_domain::ChatId;
use vault_telegram::{AuthInput, AuthStatus, VaultCandidate};
use zeroize::Zeroizing;

// No Debug: the wire input may contain passwords and login codes.
#[derive(Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum LoginInput {
    Phone(String),
    Code(String),
    Password(String),
    Qr,
    Email(String),
    EmailCode(String),
    ResendCode,
}
impl From<LoginInput> for AuthInput {
    fn from(input: LoginInput) -> Self {
        match input {
            LoginInput::Phone(v) => Self::Phone(Zeroizing::new(v)),
            LoginInput::Code(v) => Self::Code(Zeroizing::new(v)),
            LoginInput::Password(v) => Self::Password(Zeroizing::new(v)),
            LoginInput::Qr => Self::Qr,
            LoginInput::Email(v) => Self::Email(Zeroizing::new(v)),
            LoginInput::EmailCode(v) => Self::EmailCode(Zeroizing::new(v)),
            LoginInput::ResendCode => Self::ResendCode,
        }
    }
}

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackendRequest {
    #[schemars(range(min = 1, max = 1))]
    pub version: u32,
    pub request: BackendCommand,
}

#[derive(Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "command",
    content = "args",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum BackendCommand {
    StateSnapshot {
        session_id: Uuid,
    },
    RecoverDatabaseFromBackup {
        session_id: Uuid,
        path: String,
    },
    StorageUsage {
        session_id: Uuid,
    },
    TrimTelegramStorage {
        session_id: Uuid,
        target_bytes: u64,
    },
    Artwork {
        session_id: Uuid,
        track_id: Uuid,
    },
    SetQueue {
        session_id: Uuid,
        track_ids: Vec<Uuid>,
        position: u32,
        autoplay: bool,
    },
    QueuePlaylist {
        session_id: Uuid,
        playlist_id: Uuid,
        position: u32,
        autoplay: bool,
    },
    PlaybackQueue {
        session_id: Uuid,
        offset: u32,
        limit: u32,
    },
    StartJob {
        session_id: Uuid,
        job_id: Uuid,
        request: JobRequest,
        #[serde(default)]
        resume: bool,
    },
    JobStatus {
        session_id: Uuid,
        job_id: Uuid,
    },
    ListJobs {
        session_id: Uuid,
    },
    CancelJob {
        session_id: Uuid,
        job_id: Uuid,
    },
    ForgetJob {
        session_id: Uuid,
        job_id: Uuid,
    },
    ListProfiles,
    CreateProfile {
        label: String,
    },
    SwitchProfile {
        profile_id: Uuid,
    },
    RuntimeStatus,
    RepairLocalDatabase {
        session_id: Uuid,
    },
    ConnectTelegram {
        session_id: Uuid,
        api_id: i32,
        api_hash: String,
    },
    Authenticate {
        session_id: Uuid,
        input: LoginInput,
    },
    AuthStatus {
        session_id: Uuid,
    },
    DiscoverVaults {
        session_id: Uuid,
    },
    AttachVault {
        session_id: Uuid,
        #[schemars(with = "String")]
        chat_id: ChatId,
    },
    CreateVault {
        session_id: Uuid,
        #[serde(default)]
        title: Option<String>,
    },
    ResumeVaultCreation {
        session_id: Uuid,
        #[schemars(with = "String")]
        chat_id: ChatId,
    },
    Library {
        session_id: Uuid,
        query: Option<String>,
        offset: u32,
        limit: u32,
    },
    Playlists {
        session_id: Uuid,
        query: Option<String>,
        offset: u32,
        limit: u32,
    },
    Query {
        session_id: Uuid,
        query: LocalQuery,
    },
    Vault {
        session_id: Uuid,
        request: VaultRequest,
    },
    ExportBackup {
        session_id: Uuid,
        path: String,
        kind: vault_snapshot::backup::BackupKind,
    },
    PlayTrack {
        session_id: Uuid,
        track_id: Uuid,
        source_id: Uuid,
    },
    PlaybackState {
        session_id: Uuid,
    },
    ControlPlayback {
        session_id: Uuid,
        action: PlaybackAction,
    },
    Logout {
        session_id: Uuid,
    },
    Disconnect {
        session_id: Uuid,
    },
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BackendResponse {
    #[schemars(range(min = 1, max = 1))]
    pub version: u32,
    pub result: Result<BackendReply, ApiError>,
}
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    content = "result",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum BackendReply {
    StateSnapshot(Box<ProjectionSnapshot>),
    Storage(StorageUsage),
    Artwork(Option<ArtworkData>),
    Queue(QueueInfo),
    QueuePage(QueuePage),
    Job(JobInfo),
    Jobs(Vec<JobInfo>),
    Profiles(Vec<Profile>),
    Profile(Profile),
    Status(Status),
    Auth(AuthStatus),
    Vaults(Vec<VaultCandidate>),
    Tracks(Vec<vault_domain::Track>),
    Playlists(Vec<vault_domain::Playlist>),
    Query(LocalResponse),
    Vault(VaultResponse),
    Backup(vault_snapshot::backup::Manifest),
    Repaired(SnapshotInfo),
    Playback(vault_media::PlaybackState),
    Done,
}
impl Backend {
    pub async fn invoke(
        &self,
        call: BackendRequest,
        cancel: &CancellationToken,
    ) -> BackendResponse {
        let result = if call.version != 1 {
            Err(ApiError::new(ErrorCode::InvalidInput))
        } else if cancel.is_cancelled() {
            Err(ApiError::new(ErrorCode::Cancelled))
        } else {
            self.dispatch(call.request, cancel).await
        };
        BackendResponse { version: 1, result }
    }
    async fn dispatch(
        &self,
        command: BackendCommand,
        cancel: &CancellationToken,
    ) -> Result<BackendReply, ApiError> {
        use BackendCommand::*;
        Ok(match command {
            StateSnapshot { session_id } => {
                BackendReply::StateSnapshot(Box::new(self.state_snapshot(session_id).await?))
            }
            RecoverDatabaseFromBackup { session_id, path } => BackendReply::Repaired(
                self.recover_database_from_backup(session_id, path.into(), cancel)
                    .await?,
            ),
            StorageUsage { session_id } => {
                BackendReply::Storage(self.storage_usage(session_id, cancel).await?)
            }
            TrimTelegramStorage {
                session_id,
                target_bytes,
            } => BackendReply::Storage(
                self.trim_telegram_storage(session_id, target_bytes, cancel)
                    .await?,
            ),
            Artwork {
                session_id,
                track_id,
            } => BackendReply::Artwork(self.artwork(session_id, track_id, cancel).await?),
            SetQueue {
                session_id,
                track_ids,
                position,
                autoplay,
            } => BackendReply::Queue(
                self.set_queue(session_id, track_ids, position, autoplay)
                    .await?,
            ),
            QueuePlaylist {
                session_id,
                playlist_id,
                position,
                autoplay,
            } => BackendReply::Queue(
                self.queue_playlist(session_id, playlist_id, position, autoplay)
                    .await?,
            ),
            PlaybackQueue {
                session_id,
                offset,
                limit,
            } => BackendReply::QueuePage(self.playback_queue(session_id, offset, limit).await?),
            StartJob {
                session_id,
                job_id,
                request,
                resume,
            } => BackendReply::Job(self.start_job(session_id, job_id, request, resume).await?),
            JobStatus { session_id, job_id } => {
                BackendReply::Job(self.job_status(session_id, job_id).await?)
            }
            ListJobs { session_id } => BackendReply::Jobs(self.list_jobs(session_id).await?),
            CancelJob { session_id, job_id } => {
                BackendReply::Job(self.cancel_job(session_id, job_id).await?)
            }
            ForgetJob { session_id, job_id } => {
                self.forget_job(session_id, job_id).await?;
                BackendReply::Done
            }
            ListProfiles => BackendReply::Profiles(self.profiles()?),
            CreateProfile { label } => BackendReply::Profile(self.create_profile(label)?),
            SwitchProfile { profile_id } => {
                BackendReply::Status(self.switch_profile(profile_id).await?)
            }
            RuntimeStatus => BackendReply::Status(self.runtime_status().await?),
            RepairLocalDatabase { session_id } => {
                BackendReply::Repaired(self.repair_local_database(session_id, cancel).await?)
            }
            ConnectTelegram {
                session_id,
                api_id,
                api_hash,
            } => BackendReply::Auth(
                self.connect_native(session_id, api_id, Zeroizing::new(api_hash))
                    .await?,
            ),
            Authenticate { session_id, input } => {
                BackendReply::Auth(self.authenticate(session_id, input.into(), cancel).await?)
            }
            AuthStatus { session_id } => BackendReply::Auth(self.auth_status(session_id).await?),
            DiscoverVaults { session_id } => {
                BackendReply::Vaults(self.discover_vaults(session_id, cancel).await?)
            }
            AttachVault {
                session_id,
                chat_id,
            } => BackendReply::Status(self.attach_vault(session_id, chat_id, cancel).await?),
            CreateVault { session_id, title } => {
                BackendReply::Status(self.create_vault_named(session_id, title, cancel).await?)
            }
            ResumeVaultCreation {
                session_id,
                chat_id,
            } => BackendReply::Status(
                self.resume_vault_creation(session_id, chat_id, cancel)
                    .await?,
            ),
            Library {
                session_id,
                query,
                offset,
                limit,
            } => BackendReply::Tracks(
                self.local_reader(session_id)
                    .await?
                    .library(query, offset, limit)
                    .await?,
            ),
            Playlists {
                session_id,
                query,
                offset,
                limit,
            } => BackendReply::Playlists(
                self.local_reader(session_id)
                    .await?
                    .playlists(query, offset, limit)
                    .await?,
            ),
            Query { session_id, query } => {
                BackendReply::Query(self.query(session_id, query).await?)
            }
            Vault {
                session_id,
                request,
            } => BackendReply::Vault(self.service_request(session_id, request, cancel).await?),
            ExportBackup {
                session_id,
                path,
                kind,
            } => {
                if path.len() > 4096 {
                    return Err(ApiError::new(ErrorCode::InvalidInput));
                }
                BackendReply::Backup(
                    self.export_backup_cancellable(session_id, path.into(), kind, cancel)
                        .await?,
                )
            }
            PlayTrack {
                session_id,
                track_id,
                source_id,
            } => {
                self.play_track(session_id, track_id, source_id).await?;
                BackendReply::Done
            }
            PlaybackState { session_id } => {
                BackendReply::Playback(self.playback_state(session_id).await?)
            }
            ControlPlayback { session_id, action } => {
                BackendReply::Playback(self.control_playback(session_id, action).await?)
            }
            Logout { session_id } => {
                self.logout(session_id).await?;
                BackendReply::Done
            }
            Disconnect { session_id } => {
                self.disconnect(session_id).await?;
                BackendReply::Done
            }
        })
    }
}
