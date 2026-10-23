//! Reconstruct missing snapshot bytes only when the newest commit's canonical
//! digest proves the complete result. Message-ID gaps alone never prove coverage.
use crate::*;

pub async fn recover_with_history<T: Telegram>(
    transport: &T,
    chat: ChatId,
    message: &RemoteMessage,
    owner: UserId,
    verify_both: bool,
    cancel: &CancellationToken,
) -> Result<Recovery, SnapshotError> {
    reconstruct(transport, chat, message, owner, verify_both, cancel, 0).await
}

fn reconstruct<'a, T: Telegram>(
    transport: &'a T,
    chat: ChatId,
    message: &'a RemoteMessage,
    owner: UserId,
    verify_both: bool,
    cancel: &'a CancellationToken,
    depth: usize,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Recovery, SnapshotError>> + Send + 'a>>
{
    Box::pin(async move {
        match recover(transport, chat, message, owner, verify_both, cancel).await {
            Ok(recovery) => return Ok(recovery),
            Err(SnapshotError::BothCopiesInvalid) if depth < 64 => {}
            Err(error) => return Err(error),
        }
        let Some(Record::Commit(commit)) = message.record()? else {
            return Err(SnapshotError::Corrupt);
        };
        let (mut state, after) = match commit.previous_commit_message_id {
            Some(previous_id) => {
                let previous_message = transport
                    .message(chat, previous_id, cancel)
                    .await?
                    .ok_or(SnapshotError::BothCopiesInvalid)?;
                let previous = reconstruct(
                    transport,
                    chat,
                    &previous_message,
                    owner,
                    false,
                    cancel,
                    depth + 1,
                )
                .await?;
                if previous.commit.vault_id != commit.vault_id
                    || previous.commit.generation.checked_add(1) != Some(commit.generation)
                    || previous.commit.barrier_message_id >= commit.barrier_message_id
                {
                    return Err(SnapshotError::Conflict);
                }
                (
                    previous.snapshot.state,
                    previous.commit.barrier_message_id.get(),
                )
            }
            None if commit.generation == 0 => (State::new(commit.vault_id), 0),
            None => return Err(SnapshotError::BothCopiesInvalid),
        };
        let history = vault_telegram::scan_history(
            transport,
            chat,
            after,
            Some(commit.barrier_message_id),
            1_000_000,
            cancel,
        )
        .await?;
        let mut barrier_found = false;
        let mut init_found = after != 0;
        for remote in history {
            let Some(record) = remote.record()? else {
                continue;
            };
            if record.vault_id() != commit.vault_id {
                return Err(SnapshotError::Conflict);
            }
            if remote.id == commit.barrier_message_id {
                barrier_found = matches!(record, Record::Barrier(ref barrier) if barrier.snapshot_id == commit.snapshot_id);
                continue;
            }
            match record {
                Record::Init(init)
                    if after == 0 && !init_found && init.owner_telegram_user_id == owner =>
                {
                    init_found = true
                }
                Record::Init(_) => return Err(SnapshotError::Conflict),
                Record::Event(event) => {
                    state
                        .apply(remote.id, &event)
                        .map_err(|_| SnapshotError::Corrupt)?;
                }
                Record::Commit(previous)
                    if Some(remote.id) == commit.previous_commit_message_id
                        && previous.generation == state.generation => {}
                Record::Commit(_) => return Err(SnapshotError::Conflict),
                _ => {}
            }
        }
        if !barrier_found || !init_found {
            return Err(SnapshotError::BothCopiesInvalid);
        }
        let target = commit.clone();
        let encoded = tokio::task::spawn_blocking(move || {
            build(
                state,
                owner,
                target.snapshot_id,
                target.generation,
                target.barrier_message_id,
            )
        })
        .await
        .map_err(|_| SnapshotError::Corrupt)??;
        // Compressed output can differ between zstd releases. The canonical
        // uncompressed digest authenticates every field, receipt and tombstone.
        if encoded.sha256_uncompressed != commit.sha256_uncompressed {
            return Err(SnapshotError::BothCopiesInvalid);
        }
        Ok(Recovery {
            snapshot: encoded.snapshot,
            commit,
            commit_id: message.id,
            primary_healthy: false,
            backup_healthy: false,
        })
    })
}
