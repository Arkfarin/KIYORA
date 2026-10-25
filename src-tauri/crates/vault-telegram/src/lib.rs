//! TDLib 1.8.69 native JSON transport. Never uses Telegram Bot API.
mod auth;
mod client;
mod discovery;
mod native;
mod storage;
mod transport;
pub use auth::*;
pub use client::*;
pub use discovery::*;
pub use storage::*;
pub use transport::*;

pub const TDLIB_VERSION: &str = "1.8.69";
pub const TDLIB_REVISION: &str = "9d92b49bc5d0d2e29fe518239af647a88f8680e6";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TelegramError {
    #[error("TDLib shared library unavailable or incompatible")]
    NativeLibrary,
    #[error("a TDLib receive worker is already active in this process")]
    AlreadyRunning,
    #[error("TDLib worker is closed")]
    Closed,
    #[error("TDLib request timed out; write outcome may be unknown")]
    Timeout,
    #[error("TDLib request cancelled; write outcome may be unknown")]
    Cancelled,
    #[error("TDLib request queue is full")]
    Backpressure,
    #[error("malformed TDLib JSON or unexpected response shape")]
    InvalidResponse,
    #[error("TDLib request rejected with code {code}")]
    Request { code: i64 },
    #[error("Telegram authentication code is invalid")]
    InvalidCode,
    #[error("Telegram authentication code has expired")]
    CodeExpired,
    #[error("Telegram 2FA password is invalid")]
    InvalidPassword,
    #[error("Telegram connection failed")]
    Network,
    #[error("authentication input does not match the current state")]
    AuthStateMismatch,
    #[error("Telegram does not offer code resending in this state")]
    ResendUnavailable,
    #[error("code resending is available in {retry_after_seconds} seconds")]
    ResendTooEarly { retry_after_seconds: u64 },
    #[error("Telegram rate limit; retry after {retry_after_seconds} seconds")]
    RateLimited { retry_after_seconds: u64 },
    #[error("unsupported TDLib version")]
    UnsupportedVersion,
    #[error("Telegram authorization is required")]
    AuthRequired,
    #[error("channel ownership or privacy validation failed")]
    UnsafeChannel,
    #[error("selected media message is not allowed to be shared")]
    Unshareable,
    #[error("vault marker is missing, ambiguous or incompatible")]
    InvalidVault,
    #[error("TDLib updates were lost; history reconciliation is required")]
    UpdatesLost,
    #[error("invalid request input")]
    InvalidInput,
    #[error("history did not advance or exceeds the configured bound")]
    IncompleteHistory,
    #[error("invalid or incompatible vault protocol record")]
    Protocol,
    #[error("local file operation failed")]
    LocalFile,
}

impl TelegramError {
    /// Only explicit rejections prove a write was not accepted. A server 5xx,
    /// request timeout (408), missing response or cancellation is ambiguous.
    pub fn definitely_not_sent(&self) -> bool {
        matches!(self,
            Self::InvalidInput | Self::Unshareable | Self::AuthRequired
            | Self::UnsafeChannel | Self::RateLimited { .. }
            | Self::InvalidCode | Self::CodeExpired | Self::InvalidPassword
            | Self::AuthStateMismatch | Self::ResendUnavailable | Self::ResendTooEarly { .. }
            | Self::Request { code: 400..=407 | 409..=499 })
    }
}

/// Telegram descriptions may contain PII. Classify exact known identifiers and
/// discard the message. In particular, TDLib forbids processing code 406 text.
pub fn check_response(value: serde_json::Value) -> Result<serde_json::Value, TelegramError> {
    if value["@type"] != "error" {
        return Ok(value);
    }
    let code = value["code"]
        .as_i64()
        .ok_or(TelegramError::InvalidResponse)?;
    if code == 406 {
        return Err(TelegramError::Request { code });
    }
    let message = value["message"].as_str().unwrap_or("");
    let wait = [
        "Too Many Requests: retry after ",
        "FLOOD_WAIT_",
        "FLOOD_PREMIUM_WAIT_",
    ]
    .iter()
    .find_map(|prefix| message.strip_prefix(prefix))
    .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    .and_then(|s| s.parse::<u64>().ok());
    if matches!(code, 420 | 429) || wait.is_some() {
        return Err(TelegramError::RateLimited {
            retry_after_seconds: wait.unwrap_or(60).max(1),
        });
    }
    Err(match message {
        "PHONE_CODE_INVALID"
        | "PHONE_CODE_EMPTY"
        | "EMAIL_CODE_INVALID"
        | "EMAIL_CODE_EMPTY"
        | "Code must be non-empty" => TelegramError::InvalidCode,
        "Authentication code can't be resend" => TelegramError::ResendUnavailable,
        "PHONE_CODE_EXPIRED" | "EMAIL_CODE_EXPIRED" => TelegramError::CodeExpired,
        "PASSWORD_HASH_INVALID" => TelegramError::InvalidPassword,
        "NETWORK_ERROR" | "CONNECTION_FAILED" | "CONNECTION_TIMEOUT" => TelegramError::Network,
        _ => TelegramError::Request { code },
    })
}
