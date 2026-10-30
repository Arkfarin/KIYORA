use crate::api::models::{Failure, InitOptions};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};
use tokio::sync::{RwLock, RwLockReadGuard};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_core::{ApiError, Backend, ErrorCode};

pub(crate) struct Runtime {
    pub id: Uuid,
    pub options: InitOptions,
    pub backend: Arc<Backend>,
    pub lifetime: CancellationToken,
    pub operations: Arc<Mutex<HashMap<Uuid, CancellationToken>>>,
}

pub(crate) struct Operation {
    id: Uuid,
    pub cancel: CancellationToken,
    registry: Arc<Mutex<HashMap<Uuid, CancellationToken>>>,
}
impl Runtime {
    pub fn operation(&self, request: &str) -> Result<Operation, Failure> {
        let id = id(request)?;
        let mut operations = self
            .operations
            .lock()
            .map_err(|_| failure(ErrorCode::Internal))?;
        if operations.contains_key(&id) {
            return Err(failure(ErrorCode::Conflict));
        }
        if operations.len() >= 64 {
            return Err(failure(ErrorCode::Backpressure));
        }
        let cancel = self.lifetime.child_token();
        operations.insert(id, cancel.clone());
        Ok(Operation {
            id,
            cancel,
            registry: self.operations.clone(),
        })
    }
}
impl Drop for Operation {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Ok(mut registry) = self.registry.lock() {
            registry.remove(&self.id);
        }
    }
}

pub(crate) fn slot() -> &'static RwLock<Option<Runtime>> {
    static SLOT: OnceLock<RwLock<Option<Runtime>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

pub(crate) fn failure(code: ErrorCode) -> Failure {
    ApiError::new(code).into()
}
pub(crate) fn id(value: &str) -> Result<Uuid, Failure> {
    Uuid::parse_str(value)
        .ok()
        .filter(|id| !id.is_nil())
        .ok_or_else(|| failure(ErrorCode::InvalidInput))
}

pub(crate) struct Lease(RwLockReadGuard<'static, Runtime>);
impl std::ops::Deref for Lease {
    type Target = Runtime;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
pub(crate) async fn lease(client: &str) -> Result<Lease, Failure> {
    let client = id(client)?;
    let guard = slot().read().await;
    let guard = RwLockReadGuard::try_map(guard, |slot| {
        slot.as_ref()
            .filter(|runtime| runtime.id == client && !runtime.lifetime.is_cancelled())
    })
    .map_err(|_| failure(ErrorCode::Closed))?;
    Ok(Lease(guard))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::FailureCode;

    #[test]
    fn request_ids_are_exclusive_and_tokens_follow_lifetime()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let runtime = Runtime {
            id: Uuid::new_v4(),
            options: InitOptions {
                data_directory: None,
                resource_directory: None,
            },
            backend: Arc::new(Backend::new(
                vault_security::ProfileStore::at(root.path())?,
                "unused.so",
            )),
            lifetime: CancellationToken::new(),
            operations: Arc::default(),
        };
        let request = Uuid::new_v4().to_string();
        let operation = runtime.operation(&request)?;
        assert!(matches!(
            runtime.operation(&request),
            Err(Failure {
                code: FailureCode::Conflict,
                ..
            })
        ));
        let observer = operation.cancel.clone();
        drop(operation);
        assert!(observer.is_cancelled());
        let retry = runtime.operation(&request)?;
        assert!(!retry.cancel.is_cancelled());
        runtime.lifetime.cancel();
        assert!(retry.cancel.is_cancelled());
        Ok(())
    }
}
