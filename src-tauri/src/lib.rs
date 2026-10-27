// The generated mobile entry point expects (), whereas desktop startup reports
// errors to main. Do not silently discard setup failures at the JNI boundary.
#[cfg(mobile)]
#[tauri::mobile_entry_point]
fn run_mobile() {
    if run().is_err() {
        panic!("kiyora application startup failed");
    }
}

pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    vault_core::initialize_diagnostics();
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_kiyora_native::init())
        .setup(|app| {
            #[cfg(not(target_os = "android"))]
            let backend = std::sync::Arc::new(vault_core::Backend::default_backend_with_resources(
                Some(&app.path().resource_dir()?),
            )?);
            #[cfg(target_os = "android")]
            let backend = {
                let secrets = app.state::<tauri_plugin_kiyora_native::AndroidSecrets<tauri::Wry>>();
                let paths = secrets.storage_paths()?;
                let media = app.state::<tauri_plugin_kiyora_native::AndroidMedia<tauri::Wry>>();
                let documents =
                    app.state::<tauri_plugin_kiyora_native::AndroidDocuments<tauri::Wry>>();
                std::sync::Arc::new(
                    vault_core::Backend::with_secrets(
                        vault_security::ProfileStore::at(paths.profiles)?,
                        paths.tdjson,
                        std::sync::Arc::new(secrets.inner().clone()),
                    )
                    .with_playback_platform(std::sync::Arc::new(media.inner().clone()))
                    .with_import_resolver(std::sync::Arc::new(documents.inner().clone())),
                )
            };
            let mut events = backend.events().subscribe_reliable();
            app.manage(backend);
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                use tauri::Emitter;
                loop {
                    match events.recv().await {
                        Some(vault_core::EventDelivery::Event(event)) => {
                            let _ = handle.emit("kiyora://backend", event);
                        }
                        Some(vault_core::EventDelivery::ResyncRequired(lost)) => {
                            let _ = handle.emit("kiyora://resync-required", lost);
                        }
                        None => break,
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
            backend_bootstrap,
            backend_session_info,
            backend_reconnect,
            backend_auth_qr,
            backend_resume_job,
            backend_invoke,
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
            backend_query,
            backend_vault_request,
            backend_export_backup,
            backend_play_track,
            backend_playback_state,
            backend_control_playback,
            backend_logout,
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

use tauri::Manager;
use tauri::State;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_core::LoginInput;
use vault_core::{ApiError, Backend, Profile, Status, VaultRequest, VaultResponse};
use vault_telegram::{AuthStatus, VaultCandidate};
use zeroize::Zeroizing;

#[tauri::command]
async fn backend_bootstrap(backend: State<'_, std::sync::Arc<Backend>>) -> Result<Option<Status>, ApiError> {
    backend.restore_active_profile().await
}
#[tauri::command]
async fn backend_session_info(backend: State<'_, std::sync::Arc<Backend>>, session_id: Uuid) -> Result<vault_core::SessionInfo, ApiError> {
    backend.session_info(session_id).await
}
#[tauri::command]
async fn backend_reconnect(backend: State<'_, std::sync::Arc<Backend>>, session_id: Uuid) -> Result<AuthStatus, ApiError> {
    backend.reconnect(session_id).await
}
#[tauri::command]
async fn backend_auth_qr(backend: State<'_, std::sync::Arc<Backend>>, session_id: Uuid) -> Result<Option<String>, ApiError> {
    backend.auth_qr(session_id).await
}
#[tauri::command]
async fn backend_resume_job(backend: State<'_, std::sync::Arc<Backend>>, session_id: Uuid, job_id: Uuid) -> Result<vault_core::JobInfo, ApiError> {
    backend.resume_job(session_id, job_id).await
}

#[tauri::command]
async fn backend_invoke(
    backend: State<'_, std::sync::Arc<Backend>>,
    request: vault_core::BackendRequest,
) -> Result<vault_core::BackendResponse, ApiError> {
    Ok(backend.invoke(request, &CancellationToken::new()).await)
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
async fn backend_query(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
    query: vault_core::LocalQuery,
) -> Result<vault_core::LocalResponse, ApiError> {
    backend.query(session_id, query).await
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
async fn backend_export_backup(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
    path: String,
    kind: vault_snapshot::backup::BackupKind,
) -> Result<vault_snapshot::backup::Manifest, ApiError> {
    if path.len() > 4096 {
        return Err(ApiError::new(vault_core::ErrorCode::InvalidInput));
    }
    backend
        .export_backup(session_id, std::path::PathBuf::from(path), kind)
        .await
}
#[tauri::command]
async fn backend_play_track(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
    track_id: Uuid,
    source_id: Uuid,
) -> Result<(), ApiError> {
    backend.play_track(session_id, track_id, source_id).await
}
#[tauri::command]
async fn backend_playback_state(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
) -> Result<vault_media::PlaybackState, ApiError> {
    backend.playback_state(session_id).await
}
#[tauri::command]
async fn backend_control_playback(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
    action: vault_core::PlaybackAction,
) -> Result<vault_media::PlaybackState, ApiError> {
    backend.control_playback(session_id, action).await
}
#[tauri::command]
async fn backend_logout(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
) -> Result<(), ApiError> {
    backend.logout(session_id).await
}
#[tauri::command]
async fn backend_disconnect(
    backend: State<'_, std::sync::Arc<Backend>>,
    session_id: Uuid,
) -> Result<(), ApiError> {
    backend.disconnect(session_id).await
}
