//! Deliberately small value types; internal SQLite and protocol models stay Rust-only.

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InitOptions {
    /// None preserves the existing KIYORA platform data directory.
    pub data_directory: Option<String>,
    pub resource_directory: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Profile {
    pub id: String,
    pub label: String,
    pub owner_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Context {
    pub profile_id: String,
    pub session_id: String,
    pub vault_id: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeMode {
    AuthRequired,
    Discovering,
    Syncing,
    OnlineReady,
    Reconnecting,
    DegradedReadonly,
}

#[derive(Clone, Debug)]
pub struct RuntimeStatus {
    pub context: Context,
    pub mode: RuntimeMode,
}

#[derive(Clone, Debug)]
pub struct Initialization {
    /// Process generation: old clients cannot issue calls after shutdown/restart.
    pub client_id: String,
    pub profiles: Vec<Profile>,
    pub status: Option<RuntimeStatus>,
    pub restore_error: Option<Failure>,
}

#[flutter_rust_bridge::frb(non_opaque)]
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct Failure {
    pub code: FailureCode,
    pub message: String,
    pub retryable: bool,
    pub retry_after_seconds: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FailureCode {
    AuthRequired,
    InvalidCode,
    CodeExpired,
    InvalidPassword,
    AuthStateMismatch,
    ResendUnavailable,
    ResendTooEarly,
    TelegramRejected,
    NoProfile,
    NoVault,
    ProfileBusy,
    SecureStorageUnavailable,
    NativeUnavailable,
    InvalidInput,
    Conflict,
    EditNotSent,
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

impl From<vault_core::Profile> for Profile {
    fn from(value: vault_core::Profile) -> Self {
        Self {
            id: value.profile_id.to_string(),
            label: value.label,
            owner_id: value.owner_id.map(|id| id.to_string()),
        }
    }
}
impl From<vault_core::Context> for Context {
    fn from(value: vault_core::Context) -> Self {
        Self {
            profile_id: value.profile_id.to_string(),
            session_id: value.session_id.to_string(),
            vault_id: value.vault_id.map(|id| id.to_string()),
        }
    }
}
impl From<vault_core::Status> for RuntimeStatus {
    fn from(value: vault_core::Status) -> Self {
        use vault_sync::RuntimeMode as M;
        Self {
            context: value.context.into(),
            mode: match value.mode {
                M::AuthRequired => RuntimeMode::AuthRequired,
                M::Discovering => RuntimeMode::Discovering,
                M::Syncing => RuntimeMode::Syncing,
                M::OnlineReady => RuntimeMode::OnlineReady,
                M::Reconnecting => RuntimeMode::Reconnecting,
                M::DegradedReadonly => RuntimeMode::DegradedReadonly,
            },
        }
    }
}

impl From<vault_core::ApiError> for Failure {
    fn from(value: vault_core::ApiError) -> Self {
        use vault_core::ErrorCode as E;
        Self {
            code: match value.code {
                E::AuthRequired => FailureCode::AuthRequired,
                E::InvalidCode => FailureCode::InvalidCode,
                E::CodeExpired => FailureCode::CodeExpired,
                E::InvalidPassword => FailureCode::InvalidPassword,
                E::AuthStateMismatch => FailureCode::AuthStateMismatch,
                E::ResendUnavailable => FailureCode::ResendUnavailable,
                E::ResendTooEarly => FailureCode::ResendTooEarly,
                E::TelegramRejected => FailureCode::TelegramRejected,
                E::NoProfile => FailureCode::NoProfile,
                E::NoVault => FailureCode::NoVault,
                E::ProfileBusy => FailureCode::ProfileBusy,
                E::SecureStorageUnavailable => FailureCode::SecureStorageUnavailable,
                E::NativeUnavailable => FailureCode::NativeUnavailable,
                E::InvalidInput => FailureCode::InvalidInput,
                E::Conflict => FailureCode::Conflict,
                E::EditNotSent => FailureCode::EditNotSent,
                E::Readonly => FailureCode::Readonly,
                E::Integrity => FailureCode::Integrity,
                E::UnsafeVault => FailureCode::UnsafeVault,
                E::ProtocolUnsupported => FailureCode::ProtocolUnsupported,
                E::SnapshotUnavailable => FailureCode::SnapshotUnavailable,
                E::Backpressure => FailureCode::Backpressure,
                E::Cancelled => FailureCode::Cancelled,
                E::Timeout => FailureCode::Timeout,
                E::RateLimited => FailureCode::RateLimited,
                E::Network => FailureCode::Network,
                E::Storage => FailureCode::Storage,
                E::DatabaseCorrupt => FailureCode::DatabaseCorrupt,
                E::MediaUnavailable => FailureCode::MediaUnavailable,
                E::MediaSourceMissing => FailureCode::MediaSourceMissing,
                E::PlaybackUnavailable => FailureCode::PlaybackUnavailable,
                E::Closed => FailureCode::Closed,
                E::Internal => FailureCode::Internal,
            },
            message: value.message,
            retryable: value.retryable,
            retry_after_seconds: value.retry_after_seconds,
        }
    }
}
