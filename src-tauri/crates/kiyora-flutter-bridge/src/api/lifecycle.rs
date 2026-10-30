use super::models::*;
use crate::runtime::{Runtime, failure, id, lease, slot};
use std::{path::Path, sync::Arc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_core::{Backend, ErrorCode};
use vault_security::ProfileStore;

#[derive(Clone, Debug)]
pub struct SessionSnapshot {
    /// Lower bound for subsequent invalidations, read before any state queries.
    pub event_watermark: String,
    pub status: Option<RuntimeStatus>,
    pub playback: Option<super::music::Playback>,
    pub queue: Option<super::music::Queue>,
    pub api_configured: bool,
    pub repair_required: bool,
    pub connection: String,
}

/// Refresh after opening/reopening the event stream or receiving a loss marker.
/// The caller must separately invalidate loaded library pages and job details.
pub async fn session_snapshot(client_id: String) -> Result<SessionSnapshot, Failure> {
    let runtime = lease(&client_id).await?;
    let event_watermark = runtime.backend.events().sequence().map_err(Failure::from)?;
    let Some(context) = runtime.backend.context().await else {
        return Ok(SessionSnapshot {
            event_watermark,
            status: None,
            playback: None,
            queue: None,
            api_configured: false,
            repair_required: false,
            connection: "not_started".into(),
        });
    };
    let status = runtime
        .backend
        .runtime_status()
        .await
        .map_err(Failure::from)?;
    let playback = runtime
        .backend
        .playback_state(context.session_id)
        .await
        .map_err(Failure::from)?;
    let queue = runtime
        .backend
        .playback_queue(context.session_id, 0, 1)
        .await
        .map_err(Failure::from)?;
    let info = runtime
        .backend
        .session_info(context.session_id)
        .await
        .map_err(Failure::from)?;
    if status.context.session_id != context.session_id {
        return Err(failure(ErrorCode::Closed));
    }
    Ok(SessionSnapshot {
        event_watermark,
        status: Some(status.into()),
        playback: Some(playback.into()),
        queue: Some(queue.info.into()),
        api_configured: info.api_configured,
        repair_required: info.repair_required,
        connection: info.telegram_connection,
    })
}

pub async fn restore_profile(client_id: String) -> Result<Option<RuntimeStatus>, Failure> {
    Ok(lease(&client_id)
        .await?
        .backend
        .restore_active_profile()
        .await
        .map_err(Failure::from)?
        .map(Into::into))
}

/// Idempotent process initialization. Rebuilding Flutter widgets never opens
/// another backend, TDLib client, SQLite writer or audio engine.
pub async fn initialize(options: InitOptions) -> Result<Initialization, Failure> {
    let mut guard = slot().write().await;
    if let Some(runtime) = guard.as_ref() {
        if runtime.options != options || runtime.lifetime.is_cancelled() {
            return Err(failure(ErrorCode::Conflict));
        }
        return snapshot(runtime, None).await;
    }
    // Android requires the existing Keystore and service adapters before it can
    // safely open a legacy profile. Fail closed until the native host supplies them.
    if cfg!(target_os = "android") {
        return Err(failure(ErrorCode::NativeUnavailable));
    }
    let config = options.clone();
    let backend = tokio::task::spawn_blocking(move || {
        let resources = config.resource_directory.as_deref().map(Path::new);
        match config.data_directory {
            Some(path) => Backend::with_store_and_resources(ProfileStore::at(path)?, resources),
            None => Backend::default_backend_with_resources(resources),
        }
    })
    .await
    .map_err(|_| failure(ErrorCode::Internal))?
    .map_err(Failure::from)?;
    let runtime = Runtime {
        id: Uuid::new_v4(),
        options,
        backend: Arc::new(backend),
        lifetime: CancellationToken::new(),
        operations: Arc::default(),
    };
    let restore_error = runtime
        .backend
        .restore_active_profile()
        .await
        .err()
        .map(Into::into);
    let result = snapshot(&runtime, restore_error).await;
    *guard = Some(runtime);
    result
}

async fn snapshot(
    runtime: &Runtime,
    restore_error: Option<Failure>,
) -> Result<Initialization, Failure> {
    let status = if runtime.backend.context().await.is_some() {
        Some(
            runtime
                .backend
                .runtime_status()
                .await
                .map_err(Failure::from)?
                .into(),
        )
    } else {
        None
    };
    Ok(Initialization {
        client_id: runtime.id.to_string(),
        profiles: runtime
            .backend
            .profiles()
            .map_err(Failure::from)?
            .into_iter()
            .map(Into::into)
            .collect(),
        status,
        restore_error,
    })
}

pub async fn profiles(client_id: String) -> Result<Vec<Profile>, Failure> {
    let runtime = lease(&client_id).await?;
    Ok(runtime
        .backend
        .profiles()
        .map_err(Failure::from)?
        .into_iter()
        .map(Into::into)
        .collect())
}

pub async fn create_profile(client_id: String, label: String) -> Result<Profile, Failure> {
    let runtime = lease(&client_id).await?;
    Ok(runtime
        .backend
        .create_profile(label)
        .map_err(Failure::from)?
        .into())
}

pub async fn switch_profile(
    client_id: String,
    profile_id: String,
) -> Result<RuntimeStatus, Failure> {
    let runtime = lease(&client_id).await?;
    Ok(runtime
        .backend
        .switch_profile(id(&profile_id)?)
        .await
        .map_err(Failure::from)?
        .into())
}

pub async fn runtime_status(client_id: String) -> Result<Option<RuntimeStatus>, Failure> {
    let runtime = lease(&client_id).await?;
    if runtime.backend.context().await.is_none() {
        return Ok(None);
    }
    Ok(Some(
        runtime
            .backend
            .runtime_status()
            .await
            .map_err(Failure::from)?
            .into(),
    ))
}

/// Returns false when the operation already completed; it never retries a write.
pub async fn cancel_operation(client_id: String, request_id: String) -> Result<bool, Failure> {
    let runtime = lease(&client_id).await?;
    let operations = runtime
        .operations
        .lock()
        .map_err(|_| failure(ErrorCode::Internal))?;
    if let Some(cancel) = operations.get(&id(&request_id)?) {
        cancel.cancel();
        return Ok(true);
    }
    Ok(false)
}

/// Explicit process shutdown, never called for an Activity/widget recreation.
pub async fn shutdown(client_id: String) -> Result<(), Failure> {
    let client = id(&client_id)?;
    {
        let guard = slot().read().await;
        match guard.as_ref() {
            None => return Ok(()),
            Some(runtime) if runtime.id == client => runtime.lifetime.cancel(),
            _ => return Err(failure(ErrorCode::Closed)),
        }
    }
    let mut guard = slot().write().await;
    if let Some(runtime) = guard.as_ref() {
        if runtime.id != client {
            return Err(failure(ErrorCode::Closed));
        }
        runtime.backend.shutdown().await.map_err(Failure::from)?;
        *guard = None;
    }
    Ok(())
}
