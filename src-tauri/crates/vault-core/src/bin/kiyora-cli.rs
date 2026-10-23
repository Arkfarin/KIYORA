use std::{
    io::{self, Write},
    path::PathBuf,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_core::{
    ApiError, Backend, CoordinatorTarget, ErrorCode, LocalQuery, LocalResponse, Mutation,
    PlaybackAction, Status, VaultRequest, VaultResponse,
};
use vault_domain::{Action, ChatId};
use zeroize::Zeroizing;
#[path = "kiyora-cli/simulation.rs"]
mod simulation;

#[tokio::main]
async fn main() {
    vault_core::initialize_diagnostics();
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
    if args[0] == "simulate" {
        return simulation::run(&args[1..]).await;
    }
    let backend = Backend::default_backend()?;
    match args[0].as_str() {
        "serve" => vault_core::serve_stdio(&backend).await?,
        "storage" => {
            let status = active(&backend).await?;
            let cancel = CancellationToken::new();
            let report = match args.get(1).map(String::as_str) {
                None | Some("status") => {
                    backend
                        .storage_usage(status.context.session_id, &cancel)
                        .await?
                }
                Some("trim") => {
                    wait_authorized(&backend, status.context.session_id).await?;
                    let bytes = args
                        .get(2)
                        .and_then(|v| v.parse().ok())
                        .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
                    backend
                        .trim_telegram_storage(status.context.session_id, bytes, &cancel)
                        .await?
                }
                _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
            };
            print_json(&report)?;
        }
        "profiles" => profiles(&backend, &args[1..]).await?,
        "auth" => auth(&backend, &args[1..]).await?,
        "status" => {
            let status = active(&backend).await?;
            if args.iter().any(|arg| arg == "--json") {
                print_json(&status)?;
            } else {
                print_status(&status);
            }
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
        "repair" => {
            let status = active(&backend).await?;
            wait_authorized(&backend, status.context.session_id).await?;
            let cancel = CancellationToken::new();
            let result = match args.get(1) {
                None => {
                    backend
                        .repair_local_database(status.context.session_id, &cancel)
                        .await?
                }
                Some(path) if args.len() == 2 => {
                    backend
                        .recover_database_from_backup(
                            status.context.session_id,
                            path.into(),
                            &cancel,
                        )
                        .await?
                }
                _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
            };
            print_json(&result)?;
        }
        "device" => {
            let status = active(&backend).await?;
            print_json(
                &backend
                    .query(status.context.session_id, LocalQuery::Device)
                    .await?,
            )?;
        }
        "command" => {
            let status = active(&backend).await?;
            print_json(
                &backend
                    .query(
                        status.context.session_id,
                        LocalQuery::CommandStatus {
                            op_id: parse_uuid(args.get(1))?,
                        },
                    )
                    .await?,
            )?;
        }
        "settings" => {
            let status = active(&backend).await?;
            print_json(
                &backend
                    .query(status.context.session_id, LocalQuery::Settings)
                    .await?,
            )?;
        }
        _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
    }
    backend.shutdown().await?;
    Ok(())
}
fn print_help() {
    println!(
        "kiyora-cli\n\nUSAGE:\n  kiyora-cli <command>\n\nCOMMANDS:\n  profiles list [--json]|create <label>|switch <uuid>\n  auth login|status [--json]|logout\n  status [--json]\n  vault discover|attach <chat-id>|create\n  sync run|status\n  library list|search <query>|import <path>|delete <track-id>|share <track-id> <source-id> <recipient-chat-id>\n  playlist list|create <title>|rename <id> <title>|delete <id>\n  snapshot create|verify\n  gc plan|inspect <plan-id>|execute <plan-id> <plan-sha256>\n  backup verify <file>|export <file> [--full]|import <file>\n  media import <path> [--allow-duplicate]|download <track-id> <source-id>|cache stats|clear-cache\n  playback play <track-id> <source-id>|pause|resume|stop|seek <milliseconds>|volume <0..1>|status\n  diagnose"
    );
    println!("  media import-incoming <chat-id> <message-id>");
    println!(
        "  playlist show <id>|add <playlist-id> <track-id> [after-entry-id]|remove <entry-id>|move <entry-id> [after-entry-id]"
    );
    println!("  library favorite <track-id> <set-name> on|off");
    println!("  device | command <op-id> | settings");
    println!("  gc coordinator this|none|<device-id> [--force-confirmed] | abandon <plan-id>");
    println!("  media cache quota <bytes>");
    println!("  media imports|resume <op-id>|orphans");
    println!("  vault resume <chat-id>");
    println!("  library metadata <track-id> <patch.json>");
    println!("  simulate [event-count] (three simulated clients; no Telegram account)");
    println!("  snapshot repair (publishes a new verified A/B generation)");
    println!("  playback playlist <playlist-id> (interactive next/previous/pause/resume/stop)");
    println!("  repair [backup-file] (verified recovery; keeps the original SQLite files)");
    println!("  storage status|trim <target-bytes> (local TDLib files only)");
    println!("  serve (persistent JSON-lines API, including jobs and playback controls)");
}
async fn wait_authorized(backend: &Backend, session_id: Uuid) -> Result<(), ApiError> {
    for _ in 0..60 {
        match backend.auth_status(session_id).await? {
            vault_telegram::AuthStatus::Ready => return Ok(()),
            vault_telegram::AuthStatus::ParametersRequired => {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await
            }
            _ => return Err(ApiError::new(ErrorCode::AuthRequired)),
        }
    }
    Err(ApiError::new(ErrorCode::Timeout))
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
fn print_json(value: &impl serde::Serialize) -> Result<(), ApiError> {
    let encoded = serde_json::to_string(value).map_err(|_| ApiError::new(ErrorCode::Storage))?;
    println!("{encoded}");
    Ok(())
}
async fn profiles(backend: &Backend, args: &[String]) -> Result<(), ApiError> {
    match args.first().map(String::as_str) {
        Some("list") => {
            let profiles = backend.profiles()?;
            if args.iter().any(|arg| arg == "--json") {
                print_json(&profiles)?;
            } else {
                for p in profiles {
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
        Some("status") => {
            let auth = backend.auth_status(status.context.session_id).await?;
            if args.iter().any(|arg| arg == "--json") {
                print_json(&auth)?;
            } else {
                println!("{auth:?}");
            }
        }
        Some("logout") => backend.logout(status.context.session_id).await?,
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
        Some("resume") => {
            let chat = args
                .get(1)
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            print_status(
                &backend
                    .resume_vault_creation(status.context.session_id, chat, &token)
                    .await?,
            );
        }
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
        Some("metadata") => {
            let track_id = parse_uuid(args.get(1))?;
            let path = args
                .get(2)
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            if tokio::fs::metadata(path).await?.len() > 64 * 1024 {
                return Err(ApiError::new(ErrorCode::InvalidInput));
            }
            let bytes = tokio::fs::read(path).await?;
            let patch = serde_json::from_slice(&bytes)
                .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?;
            print_json(
                &backend
                    .service_request(
                        status.context.session_id,
                        VaultRequest::Mutate(Box::new(Mutation {
                            op_id: Uuid::new_v4(),
                            entity_id: track_id,
                            action: Action::TrackMetadataUpdate { patch },
                        })),
                        &CancellationToken::new(),
                    )
                    .await?,
            )?;
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
    let op_id = Uuid::new_v4();
    println!("operation={op_id}");
    let response = backend
        .service_request(
            status.context.session_id,
            VaultRequest::Import {
                path: path.clone(),
                track_id: Uuid::new_v4(),
                source_id: Uuid::new_v4(),
                op_id,
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
        Some("imports") => print_json(
            &backend
                .query(
                    status.context.session_id,
                    LocalQuery::Imports {
                        offset: 0,
                        limit: 500,
                    },
                )
                .await?,
        ),
        Some("resume") => print_json(
            &backend
                .service_request(
                    status.context.session_id,
                    VaultRequest::ResumeImport {
                        op_id: parse_uuid(args.get(1))?,
                    },
                    &CancellationToken::new(),
                )
                .await?,
        ),
        Some("orphans") => print_json(
            &backend
                .service_request(
                    status.context.session_id,
                    VaultRequest::InspectOrphanMedia,
                    &CancellationToken::new(),
                )
                .await?,
        ),
        Some("cache") if args.get(1).is_some_and(|arg| arg == "quota") => {
            let bytes = args
                .get(2)
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            print_json(
                &backend
                    .service_request(
                        status.context.session_id,
                        VaultRequest::SetCacheQuota { bytes },
                        &CancellationToken::new(),
                    )
                    .await?,
            )
        }
        Some("cache") if args.get(1).is_some_and(|arg| arg == "stats") => {
            let result = backend
                .service_request(
                    status.context.session_id,
                    VaultRequest::GetCacheStats,
                    &CancellationToken::new(),
                )
                .await?;
            if let VaultResponse::CacheStats(stats) = result {
                println!(
                    "entries={} pinned={} partial_entries={} partial_bytes={} reserved_bytes={} used_bytes={} quota_bytes={}",
                    stats.entries,
                    stats.pinned_entries,
                    stats.partial_entries,
                    stats.partial_bytes,
                    stats.reserved_bytes,
                    stats.used_bytes,
                    stats.quota_bytes
                );
                Ok(())
            } else {
                Err(ApiError::new(ErrorCode::Internal))
            }
        }
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
        Some("playlist") => {
            let events = backend.subscribe_events();
            backend
                .queue_playlist(session_id, parse_uuid(args.get(1))?, 0, true)
                .await?;
            return playback_session(backend, session_id, events).await;
        }
        Some("play") => {
            let events = backend.subscribe_events();
            backend
                .play_track(
                    session_id,
                    parse_uuid(args.get(1))?,
                    parse_uuid(args.get(2))?,
                )
                .await?;
            return playback_session(backend, session_id, events).await;
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
// A CLI process owns its audio device/session. Keep it alive while playing and
// accept controls in this process; separate invocations cannot control that sink.
async fn playback_session(
    backend: &Backend,
    session_id: Uuid,
    mut events: tokio::sync::broadcast::Receiver<vault_core::BackendEvent>,
) -> Result<(), ApiError> {
    eprintln!(
        "Playing. Controls: next, previous, pause, resume, seek <ms>, volume <0..1>, stop. Ctrl-C stops playback."
    );
    let (tx, mut lines) = tokio::sync::mpsc::channel(8);
    std::thread::Builder::new()
        .name("kiyora-playback-input".into())
        .spawn(move || {
            use std::io::BufRead;
            for line in std::io::stdin().lock().lines() {
                let Ok(line) = line else {
                    break;
                };
                if tx.blocking_send(line).is_err() {
                    break;
                }
            }
        })
        .map_err(|_| ApiError::new(ErrorCode::Internal))?;
    let mut input_open = true;
    let mut last_state = None;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                backend.control_playback(session_id, PlaybackAction::Stop).await?;
                return Ok(());
            }
            line = lines.recv(), if input_open => {
                let Some(line) = line else { input_open = false; continue; };
                let words: Vec<_> = line.split_whitespace().collect();
                let action = match words.as_slice() {
                    ["next"] => Some(PlaybackAction::Next),
                    ["previous"] => Some(PlaybackAction::Previous),
                    ["pause"] => Some(PlaybackAction::Pause),
                    ["resume"] => Some(PlaybackAction::Resume),
                    ["stop"] => Some(PlaybackAction::Stop),
                    ["seek", value] => value.parse().ok().map(PlaybackAction::Seek),
                    ["volume", value] => value.parse().ok().map(PlaybackAction::SetVolume),
                    _ => None,
                };
                if let Some(action) = action {
                    if let Err(error) = backend.control_playback(session_id, action).await { eprintln!("{}", error.message); }
                    if action == PlaybackAction::Stop { return Ok(()); }
                } else { eprintln!("Use pause, resume, seek <ms>, volume <0..1>, or stop."); }
            }
            event = events.recv() => {
                let state = match event {
                    Ok(event) if event.context.session_id == session_id => match event.data {
                        vault_core::EventData::Playback(state) => state,
                        _ => continue,
                    },
                    Ok(_) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => backend.playback_state(session_id).await?,
                    Err(_) => return Err(ApiError::new(ErrorCode::Closed)),
                };
                if matches!(state, vault_media::PlaybackState::Failed { .. }) { return Err(ApiError::new(ErrorCode::PlaybackUnavailable)); }
                if matches!(state, vault_media::PlaybackState::Idle)
                    && backend.playback_queue(session_id,0,1).await?.info.position.is_none() { return Ok(()); }
                // Print transitions, not a line for every position tick.
                let kind = std::mem::discriminant(&state);
                if last_state != Some(kind) { println!("{state:?}"); last_state = Some(kind); }
            }
        }
    }
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
        let reader = backend.local_reader(status.context.session_id).await?;
        let mut offset = 0u32;
        loop {
            let page = reader
                .playlist_detail(playlist_id, offset, 500)
                .await
                .map_err(ApiError::from)?
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            if offset == 0 {
                println!("{}\t{}", page.playlist.playlist_id, page.playlist.title);
            }
            for item in &page.items {
                println!(
                    "{}\t{}\t{}",
                    item.entry.entry_id, item.entry.track_id, item.track.metadata.title
                );
            }
            offset = offset
                .checked_add(page.items.len() as u32)
                .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
            if u64::from(offset) >= page.total || page.items.is_empty() {
                break;
            }
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
        Some("repair") => VaultRequest::Snapshot,
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
        Some("abandon") => VaultRequest::AbandonGc {
            plan_id: parse_uuid(args.get(1))?,
        },
        Some("coordinator") => {
            let target = match args.get(1).map(String::as_str) {
                Some("this") => CoordinatorTarget::ThisDevice,
                Some("none") => CoordinatorTarget::Disabled,
                Some(value) if !value.starts_with('-') => CoordinatorTarget::Device(value.into()),
                _ => return Err(ApiError::new(ErrorCode::InvalidInput)),
            };
            // Refresh before presenting the previous role to the command. The
            // engine checks it again immediately before sending the event.
            backend
                .service_request(
                    status.context.session_id,
                    VaultRequest::Sync,
                    &CancellationToken::new(),
                )
                .await?;
            let LocalResponse::Settings(settings) = backend
                .query(status.context.session_id, LocalQuery::Settings)
                .await?
            else {
                return Err(ApiError::new(ErrorCode::Internal));
            };
            let op_id = Uuid::new_v4();
            println!(
                "op_id={op_id} previous_coordinator={}",
                settings.gc_device_id.as_deref().unwrap_or("none")
            );
            VaultRequest::AssignGcCoordinator {
                op_id,
                target,
                expected_previous: settings.gc_device_id,
                force_confirmed: args.iter().any(|arg| arg == "--force-confirmed"),
            }
        }
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
    if backend.active_profile_id()?.is_some() {
        let status = active(backend).await?;
        print_status(&status);
        if let Ok(reader) = backend.local_reader(status.context.session_id).await {
            print_json(&reader.diagnostics().await.map_err(ApiError::from)?)?;
        }
        if wait_authorized(backend, status.context.session_id)
            .await
            .is_ok()
        {
            print_json(
                &backend
                    .service_request(
                        status.context.session_id,
                        VaultRequest::Diagnose,
                        &CancellationToken::new(),
                    )
                    .await?,
            )?;
        }
    }
    Ok(())
}
fn parse_uuid(raw: Option<&String>) -> Result<Uuid, ApiError> {
    Uuid::parse_str(raw.ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?)
        .map_err(|_| ApiError::new(ErrorCode::InvalidInput))
}
