#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let backend = std::sync::Arc::new(vault_core::Backend::default_backend()?);
            let mut events = backend.subscribe_events();
            app.manage(backend);
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tauri::Emitter;
                loop {
                    match events.recv().await {
                        Ok(event) => {
                            let _ = handle.emit("kiyora://backend", event);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            let backend = app
                .state::<std::sync::Arc<vault_core::Backend>>()
                .inner()
                .clone();
            tauri::async_runtime::spawn(async move {
                let _ = backend.restore_active_profile().await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            backend_list_profiles,
            backend_create_profile,
            backend_switch_profile,
            backend_runtime_status,
            backend_connect_telegram,
            backend_authenticate,
            backend_auth_status,
            backend_discover_vaults,
            backend_attach_vault,
            backend_create_vault,
            backend_resume_vault_creation,
            backend_library,
            backend_playlists,
            backend_vault_request,
            backend_disconnect
        ])
        .build(tauri::generate_context!())?;
    app.run(|handle, event| {
        if matches!(event, tauri::RunEvent::Exit)
            && let Some(backend) = handle.try_state::<std::sync::Arc<vault_core::Backend>>()
        {
            let _ = tauri::async_runtime::block_on(backend.shutdown());
        }
    });
    Ok(())
}

use serde::{Deserialize, Serialize};
use tauri::Manager;
use tauri::State;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_core::{ApiError, Backend, Profile, Status, VaultRequest, VaultResponse};
use vault_telegram::{AuthInput, AuthStatus, VaultCandidate};
use zeroize::Zeroizing;

#[derive(Debug, Clone, Serialize, Deserialize)]
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
        }
    }
}
#[tauri::command]
fn backend_list_profiles(
    backend: State<'_, std::sync::Arc<Backend>>,
) -> Result<Vec<Profile>, ApiError> {
    backend.profiles()
}
#[tauri::command]
fn backend_create_profile(
    backend: State<'_, std::sync::Arc<Backend>>,
    label: String,
) -> Result<Profile, ApiError> {
    backend.create_profile(label)
}
#[tauri::command]
async fn backend_switch_profile(
    backend: State<'_, std::sync::Arc<Backend>>,
    profile_id: Uuid,
) -> Result<Status, ApiError> {
    backend.switch_profile(profile_id).await
}
#[tauri::command]
async fn backend_runtime_status(
    backend: State<'_, std::sync::Arc<Backend>>,
) -> Result<Status, ApiError> {
    backend.runtime_status().await
}
#[tauri::command]
async fn backend_connect_telegram(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
    api_id: i32,
    api_hash: String,
) -> Result<AuthStatus, ApiError> {
    backend
        .connect_native(session_id, api_id, Zeroizing::new(api_hash))
        .await
}
#[tauri::command]
async fn backend_authenticate(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
    input: LoginInput,
) -> Result<AuthStatus, ApiError> {
    backend
        .authenticate(session_id, input.into(), &CancellationToken::new())
        .await
}
#[tauri::command]
async fn backend_auth_status(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
) -> Result<AuthStatus, ApiError> {
    backend.auth_status(session_id).await
}
#[tauri::command]
async fn backend_discover_vaults(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
) -> Result<Vec<VaultCandidate>, ApiError> {
    backend
        .discover_vaults(session_id, &CancellationToken::new())
        .await
}
#[tauri::command]
async fn backend_attach_vault(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
    chat_id: vault_domain::ChatId,
) -> Result<Status, ApiError> {
    backend
        .attach_vault(session_id, chat_id, &CancellationToken::new())
        .await
}
#[tauri::command]
async fn backend_create_vault(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
) -> Result<Status, ApiError> {
    backend
        .create_vault(session_id, &CancellationToken::new())
        .await
}
#[tauri::command]
async fn backend_resume_vault_creation(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
    chat_id: vault_domain::ChatId,
) -> Result<Status, ApiError> {
    backend
        .resume_vault_creation(session_id, chat_id, &CancellationToken::new())
        .await
}
#[tauri::command]
async fn backend_library(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
    query: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<Vec<vault_domain::Track>, ApiError> {
    backend
        .local_reader(session_id)
        .await?
        .library(query, offset, limit)
        .await
        .map_err(Into::into)
}
#[tauri::command]
async fn backend_playlists(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
    query: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<Vec<vault_domain::Playlist>, ApiError> {
    backend
        .local_reader(session_id)
        .await?
        .playlists(query, offset, limit)
        .await
        .map_err(Into::into)
}
#[tauri::command]
async fn backend_vault_request(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
    request: VaultRequest,
) -> Result<VaultResponse, ApiError> {
    backend
        .service_request(session_id, request, &CancellationToken::new())
        .await
}
#[tauri::command]
async fn backend_disconnect(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
) -> Result<(), ApiError> {
    backend.disconnect(session_id).await
}
