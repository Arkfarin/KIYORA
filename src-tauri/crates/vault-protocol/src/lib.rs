//! Strict, bounded TMV v1 wire encoding.

mod json;
mod record;
pub use json::{canonical_json, strict_json};
pub use record::*;

pub const PROTOCOL: u32 = 1;
pub const MAX_RECORD_BYTES: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("unsupported protocol, record, or action")]
    Unsupported,
    #[error("invalid TMV envelope")]
    Envelope,
    #[error("record exceeds size limit")]
    TooLarge,
    #[error("invalid JSON/schema")]
    Json(#[from] serde_json::Error),
}
