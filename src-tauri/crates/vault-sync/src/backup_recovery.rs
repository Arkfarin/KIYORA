//! An archive is a recovery candidate, never authority to roll back a vault.
use crate::{SyncEngine, SyncError};
use tokio_util::sync::CancellationToken;
use vault_domain::{State, UserId};
use vault_protocol::Record;
use vault_snapshot::{Recovery, SnapshotError};
use vault_telegram::{RemoteMessage, Telegram};

const KEY: &str = "recovery.snapshot_seed.v1";

impl<T: Telegram> SyncEngine<T> {
    /// Only used on a new temporary projection. The server's latest commit must
    /// authenticate every field, receipt and tombstone in this archive state.
    pub async fn seed_from_backup(
        &mut self,
        state: State,
        owner: UserId,
        cancel: &CancellationToken,
    ) -> Result<(), SyncError> {
        if owner != self.binding().owner_id || state.vault_id != self.binding().vault_id {
            return Err(SyncError::Integrity);
        }
        self.transport()
            .verify_channel(self.binding().chat_id, owner, cancel)
            .await?;
        let message = self.latest_commit(cancel).await?;
        authenticate(state.clone(), &message, owner).await?;
        let raw = serde_json::to_string(&state).map_err(|_| SyncError::Integrity)?;
        self.set_local_setting(KEY.into(), raw).await?;
        Ok(())
    }

    pub(crate) async fn recover_latest(
        &self,
        message: &RemoteMessage,
        both: bool,
        cancel: &CancellationToken,
    ) -> Result<Recovery, SyncError> {
        match vault_snapshot::recover_with_history(
            self.transport(),
            self.binding().chat_id,
            message,
            self.binding().owner_id,
            both,
            cancel,
        )
        .await
        {
            Ok(recovery) => Ok(recovery),
            Err(SnapshotError::BothCopiesInvalid) => {
                let Some(raw) = self.local_setting(KEY.into()).await? else {
                    return Err(SnapshotError::BothCopiesInvalid.into());
                };
                if raw.len() as u64 > vault_snapshot::MAX_UNCOMPRESSED {
                    return Err(SyncError::Integrity);
                }
                let state = serde_json::from_str(&raw).map_err(|_| SyncError::Integrity)?;
                authenticate(state, message, self.binding().owner_id).await
            }
            Err(error) => Err(error.into()),
        }
    }
}

async fn authenticate(
    state: State,
    message: &RemoteMessage,
    owner: UserId,
) -> Result<Recovery, SyncError> {
    let Some(Record::Commit(commit)) = message.record()? else {
        return Err(SyncError::Integrity);
    };
    if state.vault_id != commit.vault_id
        || state.generation != commit.generation
        || message.id <= commit.snapshot_a_message_id
        || message.id <= commit.snapshot_b_message_id
    {
        return Err(SyncError::Integrity);
    }
    let target = commit.clone();
    let encoded = tokio::task::spawn_blocking(move || {
        vault_snapshot::build(
            state,
            owner,
            target.snapshot_id,
            target.generation,
            target.barrier_message_id,
        )
    })
    .await
    .map_err(|_| SyncError::Worker)??;
    if encoded.sha256_uncompressed != commit.sha256_uncompressed {
        return Err(SyncError::Integrity);
    }
    Ok(Recovery {
        snapshot: encoded.snapshot,
        commit,
        commit_id: message.id,
        primary_healthy: false,
        backup_healthy: false,
    })
}
