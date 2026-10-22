use std::{
    io::{self, Write},
    path::PathBuf,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_core::{
    ApiError, Backend, ErrorCode, Mutation, PlaybackAction, Status, VaultRequest, VaultResponse,
};
use vault_domain::{Action, ChatId};
use zeroize::Zeroizing;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error[{:?}]: {}", error.code, error.message);
        std::process::exit(exit_code(error.code));
    }
}
fn exit_code(code: ErrorCode) -> i32 {
    match code {
        ErrorCode::Readonly | ErrorCode::Conflict => 3,
        ErrorCode::Network | ErrorCode::Timeout | ErrorCode::RateLimited => 4,
        ErrorCode::InvalidInput => 2,
        _ => 1,
    }
}
async fn run() -> Result<(), ApiError> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        print_help();
        return Ok(());
    }
    let backend = Backend::default_backend()?;
    match args[0].as_str() {
        "profiles" => profiles(&backend, &args[1..]).await?,
        "auth" => auth(&backend, &args[1..]).await?,
        "status" => {
            let status = active(&backend).await?;
            print_status(&status);
        }
        "vault" => vault(&backend, &args[1..]).await?,
        "sync" => sync(&backend, &args[1..]).await?,
        "library" => library(&backend, &args[1..]).await?,
        "playlist" => playlist(&backend, &args[1..]).await?,
        "snapshot" => snapshot(&backend, &args[1..]).await?,
        "gc" => gc(&backend, &args[1..]).await?,
        "backup" => backup(&backend, &args[1..]).await?,
        "media" => media(&backend, &args[1..]).await?,
        "playback" => playback(&backend, &args[1..]).await?,
        "diagnose" => diagnose(&backend).await?,
        _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
    }
    backend.shutdown().await?;
    Ok(())
}
fn print_help() {
    println!(
        "kiyora-cli\n\nUSAGE:\n  kiyora-cli <command>\n\nCOMMANDS:\n  profiles list|create <label>|switch <uuid>\n  auth login|status|logout\n  status\n  vault discover|attach <chat-id>|create\n  sync run|status\n  library list|search <query>|import <path>|delete <track-id>|share <track-id> <source-id> <recipient-chat-id>\n  playlist list|create <title>|rename <id> <title>|delete <id>\n  snapshot create|verify\n  gc plan|inspect <plan-id>|execute <plan-id> <plan-sha256>\n  backup verify <file>|export <file> [--full]|import <file>\n  media import <path> [--allow-duplicate]|download <track-id> <source-id>|clear-cache\n  playback play <track-id> <source-id>|pause|resume|stop|seek <milliseconds>|volume <0..1>|status\n  diagnose"
    );
    println!("  media import-incoming <chat-id> <message-id>");
    println!(
        "  playlist show <id>|add <playlist-id> <track-id> [after-entry-id]|remove <entry-id>|move <entry-id> [after-entry-id]"
    );
    println!("  library favorite <track-id> <set-name> on|off");
}
async fn active(backend: &Backend) -> Result<Status, ApiError> {
    let id = backend
        .active_profile_id()?
        .ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
    backend.restore_active_profile().await?;
    let status = backend.runtime_status().await?;
    if status.context.profile_id != id {
        return Err(ApiError::new(ErrorCode::Conflict));
    }
    Ok(status)
}
fn print_status(s: &Status) {
    println!(
        "mode={:?} profile={} session={} vault={}",
        s.mode,
        s.context.profile_id,
        s.context.session_id,
        s.context
            .vault_id
            .map(|v| v.to_string())
            .unwrap_or_else(|| "-".into())
    );
}
async fn profiles(backend: &Backend, args: &[String]) -> Result<(), ApiError> {
    match args.first().map(String::as_str) {
        Some("list") => {
            for p in backend.profiles()? {
                println!(
                    "{}\t{}\towner={}",
                    p.profile_id,
                    p.label,
                    p.owner_id
                        .map(|x| x.to_string())
                        .unwrap_or_else(|| "-".into())
                );
            }
        }
        Some("create") => {
            let label = args
                .get(1)
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            println!("{}", backend.create_profile(label.clone())?.profile_id);
        }
        Some("switch") => print_status(&backend.switch_profile(parse_uuid(args.get(1))?).await?),
        _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
    }
    Ok(())
}
async fn auth(backend: &Backend, args: &[String]) -> Result<(), ApiError> {
    let status = active(backend).await?;
    match args.first().map(String::as_str) {
        Some("status") => println!(
            "{:?}",
            backend.auth_status(status.context.session_id).await?
        ),
        Some("logout") => backend.disconnect(status.context.session_id).await?,
        Some("login") => login(backend, status).await?,
        _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
    }
    Ok(())
}
async fn login(backend: &Backend, status: Status) -> Result<(), ApiError> {
    let mut state = backend.auth_status(status.context.session_id).await?;
    if state == vault_telegram::AuthStatus::ParametersRequired {
        let api_id = std::env::var("KIYORA_API_ID")
            .ok()
            .and_then(|v| v.parse::<i32>().ok())
            .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
        let api_hash =
            std::env::var("KIYORA_API_HASH").map_err(|_| ApiError::new(ErrorCode::InvalidInput))?;
        state = backend
            .connect_native(status.context.session_id, api_id, Zeroizing::new(api_hash))
            .await?;
    }
    for _ in 0..12 {
        state = match state {
            vault_telegram::AuthStatus::PhoneRequired => {
                backend
                    .authenticate(
                        status.context.session_id,
                        vault_telegram::AuthInput::Phone(Zeroizing::new(prompt(
                            "Telegram phone number: ",
                        )?)),
                        &CancellationToken::new(),
                    )
                    .await?
            }
            vault_telegram::AuthStatus::CodeRequired => {
                backend
                    .authenticate(
                        status.context.session_id,
                        vault_telegram::AuthInput::Code(Zeroizing::new(prompt(
                            "Telegram login code: ",
                        )?)),
                        &CancellationToken::new(),
                    )
                    .await?
            }
            vault_telegram::AuthStatus::PasswordRequired => {
                let value = rpassword::prompt_password("Telegram 2FA password: ")
                    .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?;
                backend
                    .authenticate(
                        status.context.session_id,
                        vault_telegram::AuthInput::Password(Zeroizing::new(value)),
                        &CancellationToken::new(),
                    )
                    .await?
            }
            vault_telegram::AuthStatus::EmailRequired => {
                backend
                    .authenticate(
                        status.context.session_id,
                        vault_telegram::AuthInput::Email(Zeroizing::new(prompt(
                            "Telegram email: ",
                        )?)),
                        &CancellationToken::new(),
                    )
                    .await?
            }
            vault_telegram::AuthStatus::EmailCodeRequired => {
                backend
                    .authenticate(
                        status.context.session_id,
                        vault_telegram::AuthInput::EmailCode(Zeroizing::new(prompt(
                            "Telegram email code: ",
                        )?)),
                        &CancellationToken::new(),
                    )
                    .await?
            }
            vault_telegram::AuthStatus::QrConfirmation { link } => {
                println!("Approve this Telegram login on your other device: {link}");
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    let next = backend.auth_status(status.context.session_id).await?;
                    if !matches!(next, vault_telegram::AuthStatus::QrConfirmation { .. }) {
                        break next;
                    }
                }
            }
            value => {
                if value == vault_telegram::AuthStatus::Ready {
                    print_status(&backend.runtime_status().await?);
                    return Ok(());
                }
                println!("{:?}", value);
                return Ok(());
            }
        };
        if state == vault_telegram::AuthStatus::Ready {
            print_status(&backend.runtime_status().await?);
            return Ok(());
        }
    }
    Err(ApiError::new(ErrorCode::Timeout))
}
fn prompt(label: &str) -> Result<String, ApiError> {
    print!("{label}");
    io::stdout()
        .flush()
        .map_err(|_| ApiError::new(ErrorCode::Storage))?;
    let mut value = String::new();
    io::stdin()
        .read_line(&mut value)
        .map_err(|_| ApiError::new(ErrorCode::Storage))?;
    while value.ends_with(['\n', '\r']) {
        value.pop();
    }
    Ok(value)
}
async fn vault(backend: &Backend, args: &[String]) -> Result<(), ApiError> {
    let status = active(backend).await?;
    let token = CancellationToken::new();
    match args.first().map(String::as_str) {
        Some("discover") => {
            for c in backend
                .discover_vaults(status.context.session_id, &token)
                .await?
            {
                println!(
                    "chat={} vault={} title={}",
                    c.channel.chat_id, c.init.vault_id, c.channel.title
                );
            }
        }
        Some("attach") => {
            let chat = ChatId::new(
                args.get(1)
                    .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?
                    .parse()
                    .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?,
            )
            .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?;
            print_status(
                &backend
                    .attach_vault(status.context.session_id, chat, &token)
                    .await?,
            );
        }
        Some("create") => print_status(
            &backend
                .create_vault(status.context.session_id, &token)
                .await?,
        ),
        _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
    }
    Ok(())
}
async fn sync(backend: &Backend, args: &[String]) -> Result<(), ApiError> {
    let status = active(backend).await?;
    match args.first().map(String::as_str) {
        Some("status") => print_status(&backend.runtime_status().await?),
        Some("run") => {
            backend
                .service_request(
                    status.context.session_id,
                    VaultRequest::Sync,
                    &CancellationToken::new(),
                )
                .await?;
            print_status(&backend.runtime_status().await?);
        }
        _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
    }
    Ok(())
}
async fn library(backend: &Backend, args: &[String]) -> Result<(), ApiError> {
    let status = active(backend).await?;
    let reader = backend.local_reader(status.context.session_id).await?;
    match args.first().map(String::as_str) {
        Some("list") | Some("search") => {
            let query = if args[0] == "search" {
                Some(
                    args.get(1)
                        .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?
                        .clone(),
                )
            } else {
                None
            };
            for t in reader
                .library(query, 0, 500)
                .await
                .map_err(ApiError::from)?
            {
                println!(
                    "{}\t{}\t{}",
                    t.track_id, t.metadata.performer, t.metadata.title
                );
            }
        }
        Some("delete") => {
            let id = parse_uuid(args.get(1))?;
            let state = reader.state().await.map_err(ApiError::from)?;
            let count = state
                .tracks
                .get(&id)
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?
                .sources
                .len();
            let response = backend
                .service_request(
                    status.context.session_id,
                    VaultRequest::Mutate(Box::new(Mutation {
                        op_id: Uuid::new_v4(),
                        entity_id: id,
                        action: Action::TrackDelete {},
                    })),
                    &CancellationToken::new(),
                )
                .await?;
            println!("removed track with {count} source(s); {response:?}");
        }
        Some("favorite") => {
            let track_id = parse_uuid(args.get(1))?;
            let set_name = args
                .get(2)
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?
                .clone();
            let value = match args.get(3).map(String::as_str) {
                Some("on") => true,
                Some("off") => false,
                _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
            };
            let result = backend
                .service_request(
                    status.context.session_id,
                    VaultRequest::Mutate(Box::new(Mutation {
                        op_id: Uuid::new_v4(),
                        entity_id: track_id,
                        action: Action::FavoriteSet { set_name, value },
                    })),
                    &CancellationToken::new(),
                )
                .await?;
            println!("{result:?}");
        }
        Some("import") => import(backend, &status, args).await?,
        Some("share") => {
            let track_id = parse_uuid(args.get(1))?;
            let source_id = parse_uuid(args.get(2))?;
            let recipient_chat_id = args
                .get(3)
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?
                .parse::<ChatId>()
                .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?;
            let result = backend
                .service_request(
                    status.context.session_id,
                    VaultRequest::ShareTrack {
                        track_id,
                        source_id,
                        recipient_chat_id,
                    },
                    &CancellationToken::new(),
                )
                .await?;
            println!("{result:?}");
        }
        _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
    }
    Ok(())
}
async fn import(backend: &Backend, status: &Status, args: &[String]) -> Result<(), ApiError> {
    let path = args
        .get(1)
        .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
    let response = backend
        .service_request(
            status.context.session_id,
            VaultRequest::Import {
                path: path.clone(),
                track_id: Uuid::new_v4(),
                source_id: Uuid::new_v4(),
                op_id: Uuid::new_v4(),
                allow_duplicate: args.iter().any(|v| v == "--allow-duplicate"),
            },
            &CancellationToken::new(),
        )
        .await?;
    match response {
        VaultResponse::ImportReview {
            metadata,
            sha256,
            duplicate_track_ids,
            ..
        } => println!(
            "duplicate {} sha256={} matching={}; use --allow-duplicate to import a separate track",
            metadata.title,
            sha256,
            duplicate_track_ids
                .iter()
                .map(Uuid::to_string)
                .collect::<Vec<_>>()
                .join(",")
        ),
        VaultResponse::Command(command) => {
            println!("{:?} operation={}", command.status, command.intent.op_id)
        }
        _ => return Err(ApiError::new(ErrorCode::Internal)),
    }
    Ok(())
}
async fn media(backend: &Backend, args: &[String]) -> Result<(), ApiError> {
    let status = active(backend).await?;
    match args.first().map(String::as_str) {
        Some("import") => import(backend, &status, args).await,
        Some("import-incoming") => {
            let source_chat_id = args
                .get(1)
                .and_then(|value| value.parse::<ChatId>().ok())
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            let message_id = args
                .get(2)
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            let result = backend
                .service_request(
                    status.context.session_id,
                    VaultRequest::ImportIncomingAudio {
                        source_chat_id,
                        message_id,
                        track_id: Uuid::new_v4(),
                        source_id: Uuid::new_v4(),
                        op_id: Uuid::new_v4(),
                        allow_duplicate: args.iter().any(|arg| arg == "--allow-duplicate"),
                    },
                    &CancellationToken::new(),
                )
                .await?;
            println!("{result:?}");
            Ok(())
        }
        Some("download") => {
            let download_id = Uuid::new_v4();
            let track_id = parse_uuid(args.get(1))?;
            let source_id = parse_uuid(args.get(2))?;
            let result = backend
                .service_request(
                    status.context.session_id,
                    VaultRequest::DownloadForOffline {
                        download_id,
                        track_id,
                        source_id,
                        pinned: true,
                    },
                    &CancellationToken::new(),
                )
                .await?;
            println!("{result:?}");
            let interrupted = tokio::signal::ctrl_c();
            tokio::pin!(interrupted);
            let mut cancel_sent = false;
            loop {
                tokio::select! {
                    _ = &mut interrupted, if !cancel_sent => {
                        cancel_sent = true;
                        let _ = backend.service_request(
                            status.context.session_id,
                            VaultRequest::CancelDownload { download_id },
                            &CancellationToken::new(),
                        ).await;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
                }
                let current = backend
                    .service_request(
                        status.context.session_id,
                        VaultRequest::GetDownloadStatus {
                            download_id,
                            track_id,
                            source_id,
                        },
                        &CancellationToken::new(),
                    )
                    .await?;
                println!("{current:?}");
                let VaultResponse::Download(info) = current else {
                    return Err(ApiError::new(ErrorCode::Internal));
                };
                if info.state != vault_core::DownloadState::Downloading {
                    if info.state == vault_core::DownloadState::FullyCached {
                        return Ok(());
                    }
                    return Err(ApiError::new(
                        if info.state == vault_core::DownloadState::Cancelled {
                            ErrorCode::Cancelled
                        } else {
                            ErrorCode::Network
                        },
                    ));
                }
            }
        }
        Some("clear-cache") => {
            let result = backend
                .service_request(
                    status.context.session_id,
                    VaultRequest::ClearLocalCache,
                    &CancellationToken::new(),
                )
                .await?;
            println!("{result:?}");
            Ok(())
        }
        _ => Err(ApiError::new(ErrorCode::InvalidInput)),
    }
}
async fn playback(backend: &Backend, args: &[String]) -> Result<(), ApiError> {
    let status = active(backend).await?;
    let session_id = status.context.session_id;
    let state = match args.first().map(String::as_str) {
        Some("play") => {
            backend
                .play_track(
                    session_id,
                    parse_uuid(args.get(1))?,
                    parse_uuid(args.get(2))?,
                )
                .await?;
            backend.playback_state(session_id).await?
        }
        Some("status") => backend.playback_state(session_id).await?,
        Some("pause") => {
            backend
                .control_playback(session_id, PlaybackAction::Pause)
                .await?
        }
        Some("resume") => {
            backend
                .control_playback(session_id, PlaybackAction::Resume)
                .await?
        }
        Some("stop") => {
            backend
                .control_playback(session_id, PlaybackAction::Stop)
                .await?
        }
        Some("seek") => {
            let position_ms = args
                .get(1)
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            backend
                .control_playback(session_id, PlaybackAction::Seek(position_ms))
                .await?
        }
        Some("volume") => {
            let volume = args
                .get(1)
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            backend
                .control_playback(session_id, PlaybackAction::SetVolume(volume))
                .await?
        }
        _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
    };
    println!("{state:?}");
    Ok(())
}
async fn playlist(backend: &Backend, args: &[String]) -> Result<(), ApiError> {
    let status = active(backend).await?;
    let op = args
        .first()
        .map(String::as_str)
        .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
    if op == "list" {
        for p in backend
            .local_reader(status.context.session_id)
            .await?
            .playlists(None, 0, 500)
            .await
            .map_err(ApiError::from)?
        {
            println!("{}\t{}", p.playlist_id, p.title);
        }
        return Ok(());
    }
    if op == "show" {
        let playlist_id = parse_uuid(args.get(1))?;
        let state = backend
            .local_reader(status.context.session_id)
            .await?
            .state()
            .await
            .map_err(ApiError::from)?;
        let playlist = state
            .playlists
            .get(&playlist_id)
            .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
        println!("{}\t{}", playlist.playlist_id, playlist.title);
        let mut entries = state
            .entries
            .values()
            .filter(|entry| entry.playlist_id == playlist_id)
            .collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.order_key);
        for entry in entries {
            let title = state
                .tracks
                .get(&entry.track_id)
                .map(|track| track.metadata.title.as_str())
                .unwrap_or("<missing track>");
            println!("{}\t{}\t{}", entry.entry_id, entry.track_id, title);
        }
        return Ok(());
    }
    let reader = backend.local_reader(status.context.session_id).await?;
    let state = reader.state().await.map_err(ApiError::from)?;
    let (entity, action) = match op {
        "create" => (
            Uuid::new_v4(),
            Action::PlaylistCreate {
                title: args
                    .get(1)
                    .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?
                    .clone(),
            },
        ),
        "rename" => (
            parse_uuid(args.get(1))?,
            Action::PlaylistRename {
                title: args
                    .get(2)
                    .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?
                    .clone(),
            },
        ),
        "delete" => (parse_uuid(args.get(1))?, Action::PlaylistDelete {}),
        "add" => (
            parse_uuid(args.get(1))?,
            Action::PlaylistTrackAdd {
                track_id: parse_uuid(args.get(2))?,
                entry_id: Uuid::new_v4(),
                after_entry_id: args
                    .get(3)
                    .map(|value| Uuid::parse_str(value))
                    .transpose()
                    .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?,
            },
        ),
        "remove" | "move" => {
            let entry_id = parse_uuid(args.get(1))?;
            let entry = state
                .entries
                .get(&entry_id)
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            let action = if op == "remove" {
                Action::PlaylistTrackRemove { entry_id }
            } else {
                Action::PlaylistEntryMove {
                    entry_id,
                    after_entry_id: args
                        .get(2)
                        .map(|value| Uuid::parse_str(value))
                        .transpose()
                        .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?,
                }
            };
            (entry.playlist_id, action)
        }
        _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
    };
    let result = backend
        .service_request(
            status.context.session_id,
            VaultRequest::Mutate(Box::new(Mutation {
                op_id: Uuid::new_v4(),
                entity_id: entity,
                action,
            })),
            &CancellationToken::new(),
        )
        .await?;
    println!("{result:?}");
    Ok(())
}
async fn snapshot(backend: &Backend, args: &[String]) -> Result<(), ApiError> {
    let status = active(backend).await?;
    let request = match args.first().map(String::as_str) {
        Some("create") => VaultRequest::Snapshot,
        Some("verify") => VaultRequest::Verify,
        _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
    };
    println!(
        "{:?}",
        backend
            .service_request(
                status.context.session_id,
                request,
                &CancellationToken::new()
            )
            .await?
    );
    Ok(())
}
async fn gc(backend: &Backend, args: &[String]) -> Result<(), ApiError> {
    let status = active(backend).await?;
    let request = match args.first().map(String::as_str) {
        Some("plan") => VaultRequest::PlanGc,
        Some("inspect") => VaultRequest::InspectGc {
            plan_id: parse_uuid(args.get(1))?,
        },
        Some("execute") => VaultRequest::ExecuteGc {
            plan_id: parse_uuid(args.get(1))?,
            confirmed_hash: args
                .get(2)
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?
                .clone(),
        },
        _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
    };
    println!(
        "{:?}",
        backend
            .service_request(
                status.context.session_id,
                request,
                &CancellationToken::new()
            )
            .await?
    );
    Ok(())
}
async fn backup(backend: &Backend, args: &[String]) -> Result<(), ApiError> {
    match args.first().map(String::as_str) {
        Some("verify") => {
            let file = PathBuf::from(
                args.get(1)
                    .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?,
            );
            let b = vault_snapshot::backup::inspect(&file, Default::default())
                .map_err(|_| ApiError::new(ErrorCode::Integrity))?;
            println!(
                "{:?} vault={} generation={} tracks={}",
                b.manifest.kind, b.manifest.vault_id, b.manifest.generation, b.manifest.track_count
            );
        }
        Some("import") => {
            let status = active(backend).await?;
            let file = args
                .get(1)
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            let result = backend
                .service_request(
                    status.context.session_id,
                    VaultRequest::ImportBackup { path: file.clone() },
                    &CancellationToken::new(),
                )
                .await?;
            println!("{result:?}");
        }
        Some("export") => {
            let status = active(backend).await?;
            let path = PathBuf::from(
                args.get(1)
                    .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?,
            );
            let kind = if args.iter().any(|arg| arg == "--full") {
                vault_snapshot::backup::BackupKind::Full
            } else {
                vault_snapshot::backup::BackupKind::Metadata
            };
            let manifest = backend
                .export_backup(status.context.session_id, path, kind)
                .await?;
            println!(
                "{:?} backup tracks={} sha256={}",
                manifest.kind,
                manifest.track_count,
                manifest
                    .entries
                    .get("state.json.zst")
                    .map(|v| v.sha256.as_str())
                    .unwrap_or("unknown")
            );
        }
        _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
    }
    Ok(())
}
async fn diagnose(backend: &Backend) -> Result<(), ApiError> {
    println!("profiles={}", backend.profiles()?.len());
    println!("active_profile={:?}", backend.active_profile_id()?);
    println!(
        "tdjson={}",
        std::env::var_os("KIYORA_TDJSON")
            .map(|_| "configured".to_owned())
            .unwrap_or_else(|| "system loader default".into())
    );
    if let Ok(s) = backend.runtime_status().await {
        print_status(&s);
    }
    Ok(())
}
fn parse_uuid(raw: Option<&String>) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw.ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?)
        .map_err(|_| ApiError::new(ErrorCode::InvalidInput))
}
