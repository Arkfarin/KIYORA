use crate::ApiError;
use std::{future::Future, path::PathBuf, pin::Pin};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
/// Resolve an explicitly selected platform document into a private durable file.
/// The resolver owns ingress storage; Rust's import journal owns the upload copy.
pub trait ImportResolver: Send + Sync {
    fn resolve(
        &self,
        uri: String,
        op_id: Uuid,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<PathBuf, ApiError>> + Send + '_>>;
    fn release(
        &self,
        op_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<(), ApiError>> + Send + '_>>;
}
