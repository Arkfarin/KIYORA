//! Full bounded snapshots, independent server read-back and A/B cold recovery.
pub mod backup;
pub mod gc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_domain::{ChatId, MessageId, State, UserId};
use vault_protocol::{
    CopyLabel, Record, SnapshotCommit, SnapshotMarker, canonical_json, encode_text, strict_json,
};
use vault_telegram::{RemoteMessage, Telegram, TelegramError};

pub const MAX_COMPRESSED: u64 = 32 * 1024 * 1024;
pub const MAX_UNCOMPRESSED: u64 = 128 * 1024 * 1024;
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("snapshot size or decompression limit exceeded")]
    Limit,
    #[error("snapshot digest, schema, context or invariants invalid")]
    Corrupt,
    #[error("both snapshot copies are unavailable or invalid")]
    BothCopiesInvalid,
    #[error("conflicting snapshot generations or barriers")]
    Conflict,
    #[error(transparent)]
    Telegram(#[from] TelegramError),
    #[error("local snapshot file operation failed")]
    Io(#[from] std::io::Error),
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FullSnapshot {
    pub magic: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub protocol: u32,
    pub state_schema: u32,
    pub vault_id: Uuid,
    pub snapshot_id: Uuid,
    pub owner_telegram_user_id: UserId,
    pub generation: u64,
    pub barrier_message_id: MessageId,
    pub state: State,
}
pub struct EncodedSnapshot {
    pub snapshot: FullSnapshot,
    pub compressed: Vec<u8>,
    pub sha256_compressed: String,
    pub sha256_uncompressed: String,
}
pub fn build(
    mut state: State,
    owner: UserId,
    snapshot_id: Uuid,
    generation: u64,
    barrier: MessageId,
) -> Result<EncodedSnapshot, SnapshotError> {
    state.generation = generation;
    let snapshot = FullSnapshot {
        magic: "TMV".into(),
        kind: "FULL_SNAPSHOT".into(),
        protocol: 1,
        state_schema: 1,
        vault_id: state.vault_id,
        snapshot_id,
        owner_telegram_user_id: owner,
        generation,
        barrier_message_id: barrier,
        state,
    };
    validate(&snapshot)?;
    let bytes = canonical_json(&snapshot).map_err(|_| SnapshotError::Corrupt)?;
    if bytes.len() as u64 > MAX_UNCOMPRESSED {
        return Err(SnapshotError::Limit);
    }
    let compressed = zstd::stream::encode_all(bytes.as_slice(), 3)?;
    if compressed.len() as u64 > MAX_COMPRESSED {
        return Err(SnapshotError::Limit);
    }
    Ok(EncodedSnapshot {
        snapshot,
        sha256_compressed: digest(&compressed),
        sha256_uncompressed: digest(&bytes),
        compressed,
    })
}
fn validate(snapshot: &FullSnapshot) -> Result<(), SnapshotError> {
    if snapshot.magic != "TMV"
        || snapshot.kind != "FULL_SNAPSHOT"
        || snapshot.protocol != 1
        || snapshot.state_schema != 1
        || snapshot.snapshot_id.is_nil()
        || snapshot.state.vault_id != snapshot.vault_id
        || snapshot.state.generation != snapshot.generation
        || snapshot
            .state
            .last_event_message_id
            .is_some_and(|id| id >= snapshot.barrier_message_id)
    {
        return Err(SnapshotError::Corrupt);
    }
    snapshot
        .state
        .validate()
        .map_err(|_| SnapshotError::Corrupt)
}
pub fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn decode(
    bytes: &[u8],
    commit: &SnapshotCommit,
    owner: UserId,
) -> Result<FullSnapshot, SnapshotError> {
    if bytes.len() as u64 > MAX_COMPRESSED {
        return Err(SnapshotError::Limit);
    }
    if digest(bytes) != commit.sha256_compressed {
        return Err(SnapshotError::Corrupt);
    }
    let mut decoder =
        zstd::stream::read::Decoder::new(bytes).map_err(|_| SnapshotError::Corrupt)?;
    // Bound decoder window as well as output; hostile headers cannot allocate GBs.
    decoder
        .window_log_max(27)
        .map_err(|_| SnapshotError::Corrupt)?;
    let mut unpacked = Vec::new();
    decoder
        .take(MAX_UNCOMPRESSED + 1)
        .read_to_end(&mut unpacked)
        .map_err(|_| SnapshotError::Corrupt)?;
    if unpacked.len() as u64 > MAX_UNCOMPRESSED {
        return Err(SnapshotError::Limit);
    }
    if digest(&unpacked) != commit.sha256_uncompressed {
        return Err(SnapshotError::Corrupt);
    }
    let value = strict_json(&unpacked).map_err(|_| SnapshotError::Corrupt)?;
    let snapshot: FullSnapshot =
        serde_json::from_value(value).map_err(|_| SnapshotError::Corrupt)?;
    validate(&snapshot)?;
    if snapshot.vault_id != commit.vault_id
        || snapshot.snapshot_id != commit.snapshot_id
        || snapshot.generation != commit.generation
        || snapshot.barrier_message_id != commit.barrier_message_id
        || snapshot.owner_telegram_user_id != owner
    {
        return Err(SnapshotError::Corrupt);
    }
    if canonical_json(&snapshot).map_err(|_| SnapshotError::Corrupt)? != unpacked {
        return Err(SnapshotError::Corrupt);
    }
    Ok(snapshot)
}

#[derive(Debug, Clone)]
pub struct Recovery {
    pub snapshot: FullSnapshot,
    pub commit: SnapshotCommit,
    pub commit_id: MessageId,
    pub primary_healthy: bool,
    pub backup_healthy: bool,
}
pub async fn read_copy<T: Telegram>(
    transport: &T,
    chat: ChatId,
    commit: &SnapshotCommit,
    owner: UserId,
    copy: CopyLabel,
    cancel: &CancellationToken,
) -> Result<FullSnapshot, SnapshotError> {
    let id = match copy {
        CopyLabel::A => commit.snapshot_a_message_id,
        CopyLabel::B => commit.snapshot_b_message_id,
    };
    let message = transport
        .message(chat, id, cancel)
        .await?
        .ok_or(SnapshotError::Corrupt)?;
    let expected = SnapshotMarker {
        vault_id: commit.vault_id,
        snapshot_id: commit.snapshot_id,
        generation: commit.generation,
        barrier_message_id: commit.barrier_message_id,
        copy,
        sha256_compressed: commit.sha256_compressed.clone(),
    };
    if message.record()? != Some(Record::Snapshot(expected)) {
        return Err(SnapshotError::Corrupt);
    }
    let file = message.file.ok_or(SnapshotError::Corrupt)?;
    if file.size <= 0 || file.size as u64 > MAX_COMPRESSED {
        return Err(SnapshotError::Limit);
    }
    let range = transport.download(file, 0, 0, cancel).await?;
    if !range.complete {
        return Err(SnapshotError::Corrupt);
    }
    let commit = commit.clone();
    tokio::task::spawn_blocking(move || {
        let file = std::fs::File::open(range.path)?;
        let mut bytes = Vec::new();
        file.take(MAX_COMPRESSED + 1).read_to_end(&mut bytes)?;
        decode(&bytes, &commit, owner)
    })
    .await
    .map_err(|_| SnapshotError::Corrupt)?
}

pub async fn recover<T: Telegram>(
    transport: &T,
    chat: ChatId,
    commit_message: &RemoteMessage,
    owner: UserId,
    verify_both: bool,
    cancel: &CancellationToken,
) -> Result<Recovery, SnapshotError> {
    let Some(Record::Commit(commit)) = commit_message.record()? else {
        return Err(SnapshotError::Corrupt);
    };
    if commit_message.chat_id != chat
        || commit_message.id <= commit.snapshot_a_message_id
        || commit_message.id <= commit.snapshot_b_message_id
    {
        return Err(SnapshotError::Corrupt);
    }
    let primary = read_copy(transport, chat, &commit, owner, CopyLabel::A, cancel).await;
    if let Err(SnapshotError::Telegram(TelegramError::Cancelled)) = &primary {
        return Err(TelegramError::Cancelled.into());
    }
    let primary_healthy = primary.is_ok();
    let backup = if verify_both || primary.is_err() {
        Some(read_copy(transport, chat, &commit, owner, CopyLabel::B, cancel).await)
    } else {
        None
    };
    let backup_healthy = backup.as_ref().is_some_and(Result::is_ok);
    let snapshot = match (primary, backup) {
        (Ok(primary), Some(Ok(backup))) => {
            if primary != backup {
                return Err(SnapshotError::Conflict);
            }
            primary
        }
        (Ok(primary), _) => primary,
        (Err(_), Some(Ok(backup))) => backup,
        (_, Some(Err(SnapshotError::Telegram(TelegramError::Cancelled)))) => {
            return Err(TelegramError::Cancelled.into());
        }
        _ => return Err(SnapshotError::BothCopiesInvalid),
    };
    Ok(Recovery {
        snapshot,
        commit,
        commit_id: commit_message.id,
        primary_healthy,
        backup_healthy,
    })
}

/// Publishes A/B and a separate commit only after independent read-back. Caller
/// owns the online mutation guard, local exclusive job and exact barrier replay.
pub async fn publish<T: Telegram>(
    transport: &T,
    chat: ChatId,
    encoded: EncodedSnapshot,
    previous: Option<MessageId>,
    cancel: &CancellationToken,
) -> Result<Recovery, SnapshotError> {
    let mut file = tempfile::Builder::new().suffix(".json.zst").tempfile()?;
    file.write_all(&encoded.compressed)?;
    file.as_file().sync_all()?;
    let snap = &encoded.snapshot;
    let mut copies = Vec::new();
    for copy in [CopyLabel::A, CopyLabel::B] {
        let marker = Record::Snapshot(SnapshotMarker {
            vault_id: snap.vault_id,
            snapshot_id: snap.snapshot_id,
            generation: snap.generation,
            barrier_message_id: snap.barrier_message_id,
            copy,
            sha256_compressed: encoded.sha256_compressed.clone(),
        });
        let caption = encode_text(&marker).map_err(|_| SnapshotError::Corrupt)?;
        let sent = transport
            .send_document(chat, file.path().to_path_buf(), caption, cancel)
            .await?;
        if sent.record()? != Some(marker) || sent.id <= snap.barrier_message_id {
            return Err(SnapshotError::Corrupt);
        }
        copies.push(sent.id);
    }
    let [a, b] = copies.as_slice() else {
        return Err(SnapshotError::Corrupt);
    };
    let commit = SnapshotCommit {
        vault_id: snap.vault_id,
        snapshot_id: snap.snapshot_id,
        generation: snap.generation,
        barrier_message_id: snap.barrier_message_id,
        snapshot_a_message_id: *a,
        snapshot_b_message_id: *b,
        sha256_compressed: encoded.sha256_compressed,
        sha256_uncompressed: encoded.sha256_uncompressed,
        previous_commit_message_id: previous,
    };
    for copy in [CopyLabel::A, CopyLabel::B] {
        let verified = read_copy(
            transport,
            chat,
            &commit,
            snap.owner_telegram_user_id,
            copy,
            cancel,
        )
        .await?;
        if verified != encoded.snapshot {
            return Err(SnapshotError::Corrupt);
        }
    }
    let record = Record::Commit(commit.clone());
    let sent = transport
        .send_text(
            chat,
            encode_text(&record).map_err(|_| SnapshotError::Corrupt)?,
            cancel,
        )
        .await?;
    let read = transport
        .message(chat, sent.id, cancel)
        .await?
        .ok_or(SnapshotError::Corrupt)?;
    if read.record()? != Some(record) || read.id <= *a || read.id <= *b {
        return Err(SnapshotError::Corrupt);
    }
    Ok(Recovery {
        snapshot: encoded.snapshot,
        commit,
        commit_id: read.id,
        primary_healthy: true,
        backup_healthy: true,
    })
}
