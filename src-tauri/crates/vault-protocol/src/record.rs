use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;
use vault_domain::{Action, Event, MessageId, UserId, valid_digest};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VaultInit {
    pub vault_id: Uuid,
    pub owner_telegram_user_id: UserId,
    pub created_by: String,
    pub created_at_utc: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Barrier {
    pub vault_id: Uuid,
    pub snapshot_id: Uuid,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CopyLabel {
    A,
    B,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotMarker {
    pub vault_id: Uuid,
    pub snapshot_id: Uuid,
    pub generation: u64,
    pub barrier_message_id: MessageId,
    pub copy: CopyLabel,
    pub sha256_compressed: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotCommit {
    pub vault_id: Uuid,
    pub snapshot_id: Uuid,
    pub generation: u64,
    pub barrier_message_id: MessageId,
    pub snapshot_a_message_id: MessageId,
    pub snapshot_b_message_id: MessageId,
    pub sha256_uncompressed: String,
    pub sha256_compressed: String,
    pub previous_commit_message_id: Option<MessageId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MediaMarker {
    pub vault_id: Uuid,
    pub import_id: Uuid,
    pub sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GcReport {
    pub vault_id: Uuid,
    pub plan_id: Uuid,
    pub commit_message_id: MessageId,
    pub plan_sha256: String,
    pub deleted_message_ids: Vec<MessageId>,
    pub complete: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Record {
    Init(VaultInit),
    Event(Box<Event>),
    Barrier(Barrier),
    Snapshot(SnapshotMarker),
    Commit(SnapshotCommit),
    Media(MediaMarker),
    GcReport(GcReport),
}

impl Record {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Init(_) => "VAULT_INIT",
            Self::Event(_) => "EVT",
            Self::Barrier(_) => "BARRIER",
            Self::Snapshot(_) => "SNAPSHOT",
            Self::Commit(_) => "SNAPSHOT_COMMIT",
            Self::Media(_) => "MEDIA",
            Self::GcReport(_) => "GC_REPORT",
        }
    }
    pub fn vault_id(&self) -> Uuid {
        match self {
            Self::Init(v) => v.vault_id,
            Self::Event(v) => v.vault_id,
            Self::Barrier(v) => v.vault_id,
            Self::Snapshot(v) => v.vault_id,
            Self::Commit(v) => v.vault_id,
            Self::Media(v) => v.vault_id,
            Self::GcReport(v) => v.vault_id,
        }
    }
    fn validate(&self) -> Result<(), ProtocolError> {
        let valid = !self.vault_id().is_nil()
            && match self {
                Self::Init(v) => {
                    !v.created_by.is_empty()
                        && v.created_by.len() <= 128
                        && v.created_at_utc.len() <= 64
                }
                Self::Event(v) => {
                    !v.op_id.is_nil()
                        && !v.entity_id.is_nil()
                        && !v.device_id.is_empty()
                        && v.device_id.len() <= 128
                }
                Self::Barrier(v) => !v.snapshot_id.is_nil() && v.generation <= i64::MAX as u64,
                Self::Snapshot(v) => !v.snapshot_id.is_nil() && valid_digest(&v.sha256_compressed),
                Self::Commit(v) => {
                    !v.snapshot_id.is_nil()
                        && valid_digest(&v.sha256_compressed)
                        && valid_digest(&v.sha256_uncompressed)
                        && v.snapshot_a_message_id != v.snapshot_b_message_id
                        && v.snapshot_a_message_id > v.barrier_message_id
                        && v.snapshot_b_message_id > v.barrier_message_id
                        && v.previous_commit_message_id
                            .is_none_or(|p| p < v.barrier_message_id)
                }
                Self::Media(v) => {
                    !v.import_id.is_nil() && v.sha256.as_deref().is_none_or(valid_digest)
                }
                Self::GcReport(v) => !v.plan_id.is_nil() && valid_digest(&v.plan_sha256),
            };
        if valid {
            Ok(())
        } else {
            Err(ProtocolError::Envelope)
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireEvent {
    vault_id: Uuid,
    op_id: Uuid,
    device_id: String,
    base_generation: u64,
    entity_id: Uuid,
    action: String,
    payload: Value,
}

pub fn encode(record: &Record) -> Result<Vec<u8>, ProtocolError> {
    record.validate()?;
    let mut value = match record {
        Record::Event(event) => {
            let action = serde_json::to_value(&event.action)?;
            serde_json::to_value(WireEvent {
                vault_id: event.vault_id,
                op_id: event.op_id,
                device_id: event.device_id.clone(),
                base_generation: event.base_generation,
                entity_id: event.entity_id,
                action: event.action.name().into(),
                payload: action["payload"].clone(),
            })?
        }
        Record::Init(v) => serde_json::to_value(v)?,
        Record::Barrier(v) => serde_json::to_value(v)?,
        Record::Snapshot(v) => serde_json::to_value(v)?,
        Record::Commit(v) => serde_json::to_value(v)?,
        Record::Media(v) => serde_json::to_value(v)?,
        Record::GcReport(v) => serde_json::to_value(v)?,
    };
    value["magic"] = json!("TMV");
    value["protocol"] = json!(PROTOCOL);
    value["type"] = json!(record.kind());
    let encoded = canonical_json(&value)?;
    if encoded.len() > MAX_RECORD_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    Ok(encoded)
}

pub fn decode(bytes: &[u8]) -> Result<Record, ProtocolError> {
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    let mut value = strict_json(bytes)?;
    let obj = value.as_object_mut().ok_or(ProtocolError::Envelope)?;
    if obj.remove("magic") != Some(json!("TMV")) {
        return Err(ProtocolError::Envelope);
    }
    if obj.remove("protocol") != Some(json!(PROTOCOL)) {
        return Err(ProtocolError::Unsupported);
    }
    let kind = obj.remove("type").ok_or(ProtocolError::Envelope)?;
    let record = match kind.as_str() {
        Some("VAULT_INIT") => Record::Init(serde_json::from_value(value)?),
        Some("EVT") => {
            let wire: WireEvent = serde_json::from_value(value)?;
            let action: Action =
                serde_json::from_value(json!({"action":wire.action,"payload":wire.payload}))?;
            Record::Event(Box::new(Event {
                vault_id: wire.vault_id,
                op_id: wire.op_id,
                device_id: wire.device_id,
                base_generation: wire.base_generation,
                entity_id: wire.entity_id,
                action,
            }))
        }
        Some("BARRIER") => Record::Barrier(serde_json::from_value(value)?),
        Some("SNAPSHOT") => Record::Snapshot(serde_json::from_value(value)?),
        Some("SNAPSHOT_COMMIT") => Record::Commit(serde_json::from_value(value)?),
        Some("MEDIA") => Record::Media(serde_json::from_value(value)?),
        Some("GC_REPORT") => Record::GcReport(serde_json::from_value(value)?),
        _ => return Err(ProtocolError::Unsupported),
    };
    record.validate()?;
    Ok(record)
}

pub fn encode_text(record: &Record) -> Result<String, ProtocolError> {
    let bytes = encode(record)?;
    let body = String::from_utf8(bytes).map_err(|_| ProtocolError::Envelope)?;
    Ok(format!("TMV/1 {}\n{body}", record.kind()))
}

/// Ordinary user text is outside the protocol and must never become a GC candidate.
pub fn classify_text(text: &str) -> Result<Option<Record>, ProtocolError> {
    let text = text.trim();
    if text.starts_with("TMV/") {
        let (prefix, body) = text.split_once('\n').ok_or(ProtocolError::Envelope)?;
        let record = decode(body.as_bytes())?;
        if prefix != format!("TMV/1 {}", record.kind()) {
            return Err(ProtocolError::Unsupported);
        }
        return Ok(Some(record));
    }
    if text.starts_with('{') && text.contains("\"TMV\"") {
        return decode(text.as_bytes()).map(Some);
    }
    Ok(None)
}
