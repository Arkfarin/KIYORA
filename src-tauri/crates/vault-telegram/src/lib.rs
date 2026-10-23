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

/// Telegram error descriptions may contain PII. Keep only code and a bounded wait.
pub fn check_response(value: serde_json::Value) -> Result<serde_json::Value, TelegramError> {
    if value["@type"] != "error" {
        return Ok(value);
    }
    let code = value["code"]
        .as_i64()
        .ok_or(TelegramError::InvalidResponse)?;
    if code == 429 {
        let message = value["message"].as_str().unwrap_or("");
        let wait = message
            .split(|c: char| !c.is_ascii_digit())
            .filter(|s| !s.is_empty())
            .find_map(|s| s.parse::<u64>().ok())
            .unwrap_or(60)
            .clamp(1, 86400);
        return Err(TelegramError::RateLimited {
            retry_after_seconds: wait,
        });
    }
    Err(TelegramError::Request { code })
}
