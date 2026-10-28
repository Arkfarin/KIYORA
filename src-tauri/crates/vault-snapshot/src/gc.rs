//! Dependency-aware control-record allowlist. Never selects media or user notes.
use crate::{Recovery, SnapshotError, digest};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;
use vault_domain::{MessageId, State};
use vault_protocol::{CopyLabel, Record, canonical_json, encode};
use vault_telegram::{MessageContent, RemoteMessage};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    #[schemars(with = "String")]
    pub message_id: MessageId,
    pub kind: String,
    pub record_sha256: String,
    pub reason: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GcPlan {
    pub plan_id: Uuid,
    pub vault_id: Uuid,
    pub coordinator: String,
    #[schemars(with = "String")]
    pub commit_id: MessageId,
    pub generation: u64,
    #[schemars(with = "String")]
    pub barrier_id: MessageId,
    pub candidates: Vec<Candidate>,
}
impl GcPlan {
    pub fn hash(&self) -> Result<String, SnapshotError> {
        Ok(digest(
            &canonical_json(self).map_err(|_| SnapshotError::Corrupt)?,
        ))
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GcProgress {
    pub plan: GcPlan,
    #[schemars(with = "BTreeSet<String>")]
    pub completed: BTreeSet<MessageId>,
    pub status: GcStatus,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum GcStatus {
    Planned,
    Running,
    Complete,
    Stopped,
    Abandoned,
}

pub fn plan(
    messages: &[RemoteMessage],
    recovery: &Recovery,
    state: &State,
    coordinator: &str,
) -> Result<GcPlan, SnapshotError> {
    if !recovery.primary_healthy
        || !recovery.backup_healthy
        || state.settings.gc_device_id.as_deref() != Some(coordinator)
        || recovery.commit.vault_id != state.vault_id
    {
        return Err(SnapshotError::Conflict);
    }
    let current = &recovery.commit;
    let mut records = BTreeMap::new();
    let mut controls = BTreeSet::new();
    for message in messages {
        if let Some(record) = message.record()? {
            if disposable_content(message, &record) {
                controls.insert(message.id);
            }
            if record.vault_id() != state.vault_id || records.insert(message.id, record).is_some() {
                return Err(SnapshotError::Conflict);
            }
        }
    }
    if records.get(&recovery.commit_id) != Some(&Record::Commit(current.clone())) {
        return Err(SnapshotError::Conflict);
    }
    let mut protected = BTreeSet::from([
        recovery.commit_id,
        current.snapshot_a_message_id,
        current.snapshot_b_message_id,
    ]);
    for source in state.media_sources() {
        protected.insert(source.message_id);
    }
    let mut candidates = BTreeMap::new();
    let mut references: BTreeMap<MessageId, BTreeSet<MessageId>> = BTreeMap::new();
    for (id, record) in &records {
        if let Record::Commit(commit) = record {
            for copy in [commit.snapshot_a_message_id, commit.snapshot_b_message_id] {
                references.entry(copy).or_default().insert(*id);
            }
        }
    }
    for (id, record) in &records {
        if *id > current.barrier_message_id || protected.contains(id) || !controls.contains(id) {
            continue;
        }
        let reason = match record {
            Record::Event(event) => {
                let receipt = recovery
                    .snapshot
                    .state
                    .applied
                    .get(&event.op_id)
                    .ok_or(SnapshotError::Conflict)?;
                if receipt.event_sha256
                    != vault_domain::event_digest(event).map_err(|_| SnapshotError::Corrupt)?
                {
                    return Err(SnapshotError::Conflict);
                }
                Some("event outcome is covered by the current full snapshot")
            }
            Record::Barrier(_) => {
                Some("barrier is at or before the committed full snapshot boundary")
            }
            _ => None,
        };
        if let Some(reason) = reason {
            candidates.insert(*id, candidate(*id, record, reason)?);
        }
    }
    // Old committed groups are considered together. Keep uncommitted staging,
    // shared copies, unknown dependencies, and any record beyond the current cut.
    for (id, record) in &records {
        let Record::Commit(old) = record else {
            continue;
        };
        let ids = [*id, old.snapshot_a_message_id, old.snapshot_b_message_id];
        if old.generation >= current.generation
            || old.barrier_message_id >= current.barrier_message_id
            || ids.iter().any(|x| {
                *x > current.barrier_message_id || protected.contains(x) || !controls.contains(x)
            })
        {
            continue;
        }
        let mut group_valid = true;
        for (copy_id, label) in [
            (old.snapshot_a_message_id, CopyLabel::A),
            (old.snapshot_b_message_id, CopyLabel::B),
        ] {
            let valid = match records.get(&copy_id) {
                Some(Record::Snapshot(marker)) => {
                    marker.snapshot_id == old.snapshot_id
                        && marker.generation == old.generation
                        && marker.barrier_message_id == old.barrier_message_id
                        && marker.copy == label
                        && marker.sha256_compressed == old.sha256_compressed
                }
                _ => false,
            };
            group_valid &= valid
                && references
                    .get(&copy_id)
                    .is_some_and(|refs| refs.len() == 1 && refs.contains(id));
        }
        if group_valid {
            for old_id in ids {
                if let Some(record) = records.get(&old_id) {
                    candidates.insert(
                        old_id,
                        candidate(
                            old_id,
                            record,
                            "superseded complete snapshot group with no live copy dependency",
                        )?,
                    );
                }
            }
        }
    }
    Ok(GcPlan {
        plan_id: Uuid::new_v4(),
        vault_id: state.vault_id,
        coordinator: coordinator.into(),
        commit_id: recovery.commit_id,
        generation: current.generation,
        barrier_id: current.barrier_message_id,
        candidates: candidates.into_values().collect(),
    })
}
fn candidate(
    message_id: MessageId,
    record: &Record,
    reason: &str,
) -> Result<Candidate, SnapshotError> {
    Ok(Candidate {
        message_id,
        kind: record.kind().into(),
        record_sha256: digest(&encode(record).map_err(|_| SnapshotError::Corrupt)?),
        reason: reason.into(),
    })
}
/// Rechecks the exact classified record, not only its message id or generation.
pub fn matches(candidate: &Candidate, message: &RemoteMessage) -> Result<bool, SnapshotError> {
    if candidate.message_id != message.id {
        return Ok(false);
    }
    let Some(record) = message.record()? else {
        return Ok(false);
    };
    if !disposable_content(message, &record) {
        return Ok(false);
    }
    Ok(candidate.kind == record.kind()
        && candidate.record_sha256 == digest(&encode(&record).map_err(|_| SnapshotError::Corrupt)?))
}

fn disposable_content(message: &RemoteMessage, record: &Record) -> bool {
    match record {
        Record::Event(_) | Record::Barrier(_) | Record::Commit(_) => {
            message.content == MessageContent::Text && message.file.is_none()
        }
        Record::Snapshot(_) => {
            message.content == MessageContent::Document
                && message
                    .file
                    .as_ref()
                    .is_some_and(|file| !file.mime_type.starts_with("audio/"))
        }
        _ => false,
    }
}
