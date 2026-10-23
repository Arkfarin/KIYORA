use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_core::{ApiError, ErrorCode};
use vault_db::{Binding, Database};
use vault_domain::{Action, ChatId, UserId};
use vault_protocol::{Record, VaultInit, encode_text};
use vault_sync::SyncEngine;
use vault_telegram::Telegram;
use vault_testkit::{FakeServer, Faults};

pub async fn run(args: &[String]) -> Result<(), ApiError> {
    let count = args
        .first()
        .map(|s| s.parse::<u32>())
        .transpose()
        .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?
        .unwrap_or(100);
    if count == 0 || count > 10_000 || args.len() > 1 {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    let binding = Binding {
        vault_id: Uuid::new_v4(),
        owner_id: UserId::new(42).map_err(|_| ApiError::new(ErrorCode::Internal))?,
        chat_id: ChatId::new(-100).map_err(|_| ApiError::new(ErrorCode::Internal))?,
    };
    let server = FakeServer::new(binding.chat_id, binding.owner_id);
    let cancel = CancellationToken::new();
    let transport = server.client()?;
    transport
        .send_text(
            binding.chat_id,
            encode_text(&Record::Init(VaultInit {
                vault_id: binding.vault_id,
                owner_telegram_user_id: binding.owner_id,
                created_by: "simulation".into(),
                created_at_utc: "2026-01-01T00:00:00Z".into(),
            }))
            .map_err(|_| ApiError::new(ErrorCode::Internal))?,
            &cancel,
        )
        .await?;
    let mut clients = Vec::new();
    for i in 0..3 {
        clients.push(SyncEngine::new(
            server.client()?,
            Database::memory(binding)?,
            format!("sim-{i}"),
        ));
    }
    clients[0].initialize(&cancel).await?;
    clients[1].sync(&cancel).await?;
    clients[2].sync(&cancel).await?;
    server.set_faults(Faults {
        history_page_size: Some(17),
        duplicate_updates: true,
        ..Default::default()
    })?;
    let playlist = Uuid::new_v4();
    for index in 0..count {
        let action = if index == 0 {
            Action::PlaylistCreate {
                title: "Initial".into(),
            }
        } else {
            Action::PlaylistRename {
                title: format!("Revision {index}"),
            }
        };
        let client = &mut clients[index as usize % 3];
        let command = client
            .execute(Uuid::new_v4(), playlist, action, &cancel)
            .await?;
        if command.status != vault_db::CommandStatus::Applied {
            return Err(ApiError::new(ErrorCode::Integrity));
        }
    }
    clients[0]
        .assign_gc_coordinator(Uuid::new_v4(), Some("sim-0".into()), None, false, &cancel)
        .await?;
    clients[0].request_snapshot(&cancel).await?;
    for client in &mut clients {
        client.sync(&cancel).await?;
    }
    let expected = clients[0].state().await?;
    for client in &clients[1..] {
        if client.state().await? != expected {
            return Err(ApiError::new(ErrorCode::Integrity));
        }
    }
    let mut cold = SyncEngine::new(
        server.client()?,
        Database::memory(binding)?,
        "cold-recovery".into(),
    );
    cold.sync(&cancel).await?;
    if cold.state().await? != expected {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    println!(
        "{}",
        serde_json::json!({"clients":3,"events":count,"converged":true,"cold_recovery":true,"transport":"simulated"})
    );
    Ok(())
}
