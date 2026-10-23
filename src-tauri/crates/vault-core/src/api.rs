use serde::{Deserialize, Serialize};
use uuid::Uuid;
use vault_db::Command;
use vault_domain::{Action, ChatId, MessageId};
use vault_snapshot::{Recovery, gc::GcProgress};
use vault_sync::{RuntimeMode, RuntimeStatus, SyncError};
use vault_telegram::TelegramError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    AuthRequired,
    NoProfile,
    NoVault,
    ProfileBusy,
    SecureStorageUnavailable,
    NativeUnavailable,
    InvalidInput,
    Conflict,
    Readonly,
    Integrity,
    UnsafeVault,
    ProtocolUnsupported,
    SnapshotUnavailable,
    Backpressure,
    Cancelled,
    Timeout,
    RateLimited,
    Network,
    Storage,
    DatabaseCorrupt,
    MediaUnavailable,
    MediaSourceMissing,
    PlaybackUnavailable,
    Closed,
    Internal,
}

/// Deliberately contains no raw server messages, credentials or filesystem paths.
#[derive(
    Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error, schemars::JsonSchema,
)]
#[error("{message}")]
#[serde(deny_unknown_fields)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    pub retryable: bool,
}
impl ApiError {
    pub fn new(code: ErrorCode) -> Self {
        use ErrorCode::*;
        let message = match code {
            AuthRequired => "Telegram authorization is required",
            NoProfile => "select a profile first",
            NoVault => "attach a vault first",
            ProfileBusy => "profile is already in use",
            SecureStorageUnavailable => "OS secure storage is unavailable",
            NativeUnavailable => "pinned TDLib native library is unavailable",
            InvalidInput => "invalid request",
            Conflict => "request conflicts with persisted state",
            Readonly => "shared changes require ONLINE_READY",
            Integrity => "vault integrity or compatibility check failed",
            UnsafeVault => "vault must be a private channel owned only by this account",
            ProtocolUnsupported => {
                "vault protocol is invalid or unsupported; update the application and verify the vault"
            }
            SnapshotUnavailable => {
                "both snapshot copies are unavailable; recover from a matching backup or retained history"
            }
            Backpressure => "job queue is full",
            Cancelled => "job cancelled; inspect its command before retrying",
            Timeout => "request timed out; inspect its command before retrying",
            RateLimited => "Telegram rate limit reached",
            Network => "Telegram request failed",
            Storage => "local storage operation failed",
            DatabaseCorrupt => {
                "local database is damaged; restore it from a verified Telegram snapshot"
            }
            MediaUnavailable => "media is not available in the local cache",
            MediaSourceMissing => "registered media sources are missing or no longer match",
            PlaybackUnavailable => "system audio playback is unavailable",
            Closed => "session is closed",
            Internal => "backend worker failed",
        };
        Self {
            code,
            message: message.into(),
            retryable: matches!(code, Backpressure | RateLimited | Network),
        }
    }
}
impl From<SyncError> for ApiError {
    fn from(error: SyncError) -> Self {
        use SyncError::*;
        match error {
            Telegram(error) => error.into(),
            Database(vault_db::DbError::CommandCollision) => Self::new(ErrorCode::Conflict),
            Database(error) => Self::new(if error.is_corruption() {
                ErrorCode::DatabaseCorrupt
            } else {
                ErrorCode::Storage
            }),
            Readonly => Self::new(ErrorCode::Readonly),
            CoordinatorConflict => Self::new(ErrorCode::Conflict),
            InitializationRequired => Self::new(ErrorCode::NoVault),
            Worker => Self::new(ErrorCode::Internal),
            UnknownOutcome => Self::new(ErrorCode::Timeout),
            ReconcileRequired => Self::new(ErrorCode::Network),
            Snapshot(vault_snapshot::SnapshotError::BothCopiesInvalid) => {
                Self::new(ErrorCode::SnapshotUnavailable)
            }
            Snapshot(vault_snapshot::SnapshotError::Telegram(error)) => error.into(),
            Snapshot(_) | Integrity => Self::new(ErrorCode::Integrity),
            MediaSourceMissing => Self::new(ErrorCode::MediaSourceMissing),
        }
    }
}
impl From<TelegramError> for ApiError {
    fn from(error: TelegramError) -> Self {
        use TelegramError::*;
        Self::new(match error {
            NativeLibrary | UnsupportedVersion => ErrorCode::NativeUnavailable,
            AlreadyRunning => ErrorCode::ProfileBusy,
            Closed => ErrorCode::Closed,
            Timeout => ErrorCode::Timeout,
            Cancelled => ErrorCode::Cancelled,
            Backpressure => ErrorCode::Backpressure,
            AuthRequired => ErrorCode::AuthRequired,
            RateLimited { .. } => ErrorCode::RateLimited,
            Request { .. } | UpdatesLost => ErrorCode::Network,
            InvalidInput => ErrorCode::InvalidInput,
            LocalFile => ErrorCode::Storage,
            Unshareable => ErrorCode::Conflict,
            UnsafeChannel => ErrorCode::UnsafeVault,
            Protocol => ErrorCode::ProtocolUnsupported,
            InvalidResponse | InvalidVault | IncompleteHistory => ErrorCode::Integrity,
        })
    }
}
impl From<vault_security::SecurityError> for ApiError {
    fn from(error: vault_security::SecurityError) -> Self {
        use vault_security::SecurityError::*;
        Self::new(match error {
            ProfileBusy => ErrorCode::ProfileBusy,
            SecureStorageUnavailable | MissingKey => ErrorCode::SecureStorageUnavailable,
            InvalidProfile => ErrorCode::InvalidInput,
            AccountMismatch => ErrorCode::Conflict,
            Io(_) => ErrorCode::Storage,
            Random => ErrorCode::Internal,
        })
    }
}
impl From<std::io::Error> for ApiError {
    fn from(_: std::io::Error) -> Self {
        Self::new(ErrorCode::Storage)
    }
}
impl From<vault_db::DbError> for ApiError {
    fn from(error: vault_db::DbError) -> Self {
        SyncError::Database(error).into()
    }
}
impl From<vault_media::MediaError> for ApiError {
    fn from(error: vault_media::MediaError) -> Self {
        match error {
            vault_media::MediaError::Telegram(error) => error.into(),
            vault_media::MediaError::Io(_) => Self::new(ErrorCode::Storage),
            vault_media::MediaError::CacheFull => Self::new(ErrorCode::Storage),
            vault_media::MediaError::CacheBusy => Self::new(ErrorCode::Conflict),
            vault_media::MediaError::InvalidAudio
            | vault_media::MediaError::TooLarge
            | vault_media::MediaError::Artwork => Self::new(ErrorCode::InvalidInput),
            vault_media::MediaError::CacheIndex | vault_media::MediaError::Range => {
                Self::new(ErrorCode::Integrity)
            }
        }
    }
}
impl From<vault_media::PlaybackError> for ApiError {
    fn from(error: vault_media::PlaybackError) -> Self {
        match error {
            vault_media::PlaybackError::DeviceUnavailable => {
                Self::new(ErrorCode::PlaybackUnavailable)
            }
            vault_media::PlaybackError::Decode | vault_media::PlaybackError::Seek => {
                Self::new(ErrorCode::InvalidInput)
            }
            vault_media::PlaybackError::Closed => Self::new(ErrorCode::Closed),
            vault_media::PlaybackError::Media(error) => error.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub profile_id: Uuid,
    pub session_id: Uuid,
    pub vault_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Status {
    pub context: Context,
    pub mode: RuntimeMode,
    pub vault: Option<RuntimeStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum LocalQuery {
    SourceHealth {
        track_id: Uuid,
    },
    Imports {
        offset: u32,
        limit: u32,
    },
    Playlist {
        playlist_id: Uuid,
        offset: u32,
        limit: u32,
    },
    CommandStatus {
        op_id: Uuid,
    },
    FavoriteSets {
        offset: u32,
        limit: u32,
    },
    Favorites {
        set_name: String,
        offset: u32,
        limit: u32,
    },
    Settings,
    Device,
    Albums {
        query: String,
        offset: u32,
        limit: u32,
    },
    Artists {
        query: String,
        offset: u32,
        limit: u32,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeviceInfo {
    pub context: Context,
    pub device_id: String,
    pub cache_quota_bytes: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    content = "result",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum LocalResponse {
    Sources(Vec<vault_sync::SourceHealth>),
    Imports(Vec<crate::ImportInfo>),
    Playlist(Option<vault_db::PlaylistDetail>),
    CommandStatus(Option<Box<Command>>),
    FavoriteSets(Vec<String>),
    Favorites(vault_db::TrackPage),
    Settings(vault_domain::SharedSettings),
    Device(DeviceInfo),
    Albums(Vec<vault_db::AlbumSummary>),
    Artists(Vec<vault_db::ArtistSummary>),
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Mutation {
    pub op_id: Uuid,
    pub entity_id: Uuid,
    pub action: Action,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PlaybackAction {
    Next,
    Previous,
    Pause,
    Resume,
    Stop,
    Seek(u64),
    SetVolume(f32),
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum VaultRequest {
    PreviewIncomingAudio {
        #[schemars(with = "String")]
        chat_id: ChatId,
        #[schemars(with = "String")]
        message_id: MessageId,
    },
    RelinkSource {
        track_id: Uuid,
        source_id: Uuid,
        #[schemars(with = "String")]
        message_id: MessageId,
        op_id: Uuid,
    },
    Diagnose,
    CheckSources {
        track_id: Option<Uuid>,
    },
    ResumeImport {
        op_id: Uuid,
    },
    InspectOrphanMedia,
    Sync,
    Initialize,
    Verify,
    Snapshot,
    PlanGc,
    AssignGcCoordinator {
        op_id: Uuid,
        target: CoordinatorTarget,
        expected_previous: Option<String>,
        force_confirmed: bool,
    },
    AbandonGc {
        plan_id: Uuid,
    },
    ExecuteGc {
        plan_id: Uuid,
        confirmed_hash: String,
    },
    InspectGc {
        plan_id: Uuid,
    },
    Import {
        path: String,
        track_id: Uuid,
        source_id: Uuid,
        op_id: Uuid,
        allow_duplicate: bool,
    },
    ImportIncomingAudio {
        #[schemars(with = "String")]
        source_chat_id: ChatId,
        #[schemars(with = "String")]
        message_id: MessageId,
        track_id: Uuid,
        source_id: Uuid,
        op_id: Uuid,
        allow_duplicate: bool,
    },
    DownloadForOffline {
        download_id: Uuid,
        track_id: Uuid,
        source_id: Uuid,
        pinned: bool,
    },
    GetDownloadStatus {
        download_id: Uuid,
        track_id: Uuid,
        source_id: Uuid,
    },
    CancelDownload {
        download_id: Uuid,
    },
    PinCachedMedia {
        track_id: Uuid,
        source_id: Uuid,
        pinned: bool,
    },
    GetCacheStats,
    SetCacheQuota {
        bytes: u64,
    },
    ClearLocalCache,
    ImportBackup {
        path: String,
    },
    ShareTrack {
        track_id: Uuid,
        source_id: Uuid,
        #[schemars(with = "String")]
        recipient_chat_id: ChatId,
    },
    Mutate(Box<Mutation>),
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    content = "device_id",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CoordinatorTarget {
    ThisDevice,
    Device(String),
    Disabled,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SnapshotInfo {
    #[schemars(with = "String")]
    pub commit_id: MessageId,
    #[schemars(with = "String")]
    pub barrier_id: MessageId,
    pub generation: u64,
    pub primary_healthy: bool,
    pub backup_healthy: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DownloadState {
    Unavailable,
    RemoteOnly,
    Downloading,
    PartiallyCached,
    FullyCached,
    FailedRetryable,
    FailedPermanent,
    Cancelled,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DownloadInfo {
    pub download_id: Uuid,
    pub track_id: Uuid,
    pub state: DownloadState,
    pub bytes_downloaded: u64,
    pub total_bytes: Option<u64>,
    pub pinned: bool,
}
impl From<Recovery> for SnapshotInfo {
    fn from(r: Recovery) -> Self {
        Self {
            commit_id: r.commit_id,
            barrier_id: r.commit.barrier_message_id,
            generation: r.commit.generation,
            primary_healthy: r.primary_healthy,
            backup_healthy: r.backup_healthy,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", content = "result", rename_all = "snake_case")]
pub enum VaultResponse {
    AudioInfo(Option<vault_telegram::AudioInfo>),
    Diagnostics(vault_sync::VaultDiagnostics),
    Sources(Vec<vault_sync::SourceHealth>),
    OrphanMedia(Vec<crate::OrphanMedia>),
    Synced,
    Initialized,
    Command(Box<Command>),
    Snapshot(SnapshotInfo),
    Gc(Option<GcProgress>),
    ImportReview {
        metadata: Box<vault_domain::Metadata>,
        sha256: String,
        size_bytes: u64,
        duplicate_track_ids: Vec<Uuid>,
        possible_duplicate_track_ids: Vec<Uuid>,
    },
    Shared {
        #[schemars(with = "String")]
        recipient_chat_id: ChatId,
        #[schemars(with = "String")]
        message_id: MessageId,
    },
    Download(DownloadInfo),
    CachePinned,
    CacheStats(vault_media::CacheStats),
    CacheCleared {
        bytes_removed: u64,
    },
    BackupRestored {
        source_vault_id: Uuid,
        tracks: usize,
        playlists: usize,
        operations: usize,
    },
}
