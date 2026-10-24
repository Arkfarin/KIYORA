//! Sharing has no cloud idempotency key. Record intent before sending, return
//! unknown on ambiguous failures, and never resend the same operation ID.
use crate::{ApiError, ErrorCode};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_domain::{ChatId, MessageId};
use vault_sync::SyncEngine;
use vault_telegram::Telegram;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShareState {
    Sent,
    NotSent,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShareInfo {
    pub op_id: Uuid,
    pub track_id: Uuid,
    pub source_id: Uuid,
    #[schemars(with = "String")]
    pub recipient_chat_id: ChatId,
    pub state: ShareState,
    #[schemars(with = "Option<String>")]
    pub message_id: Option<MessageId>,
    pub error: Option<ApiError>,
}

fn key(op: Uuid) -> String {
    format!("share.v1:{op}")
}

pub(crate) async fn status<T: Telegram>(
    engine: &SyncEngine<T>,
    op: Uuid,
) -> Result<Option<ShareInfo>, ApiError> {
    if op.is_nil() {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    let Some(raw) = engine.local_setting(key(op)).await? else {
        return Ok(None);
    };
    let info: ShareInfo =
        serde_json::from_str(&raw).map_err(|_| ApiError::new(ErrorCode::Integrity))?;
    if info.op_id != op
        || info.track_id.is_nil()
        || info.source_id.is_nil()
        || (info.state == ShareState::Sent) != info.message_id.is_some()
        || (info.state == ShareState::Sent && info.error.is_some())
    {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    Ok(Some(info))
}

async fn save<T: Telegram>(engine: &SyncEngine<T>, info: &ShareInfo) -> Result<(), ApiError> {
    engine
        .set_local_setting(
            key(info.op_id),
            serde_json::to_string(info).map_err(|_| ApiError::new(ErrorCode::Internal))?,
        )
        .await?;
    Ok(())
}

pub(crate) async fn send<T: Telegram>(
    engine: &mut SyncEngine<T>,
    op_id: Uuid,
    track_id: Uuid,
    source_id: Uuid,
    recipient_chat_id: ChatId,
    cancel: &CancellationToken,
) -> Result<ShareInfo, ApiError> {
    if op_id.is_nil() || track_id.is_nil() || source_id.is_nil() {
        return Err(ApiError::new(ErrorCode::InvalidInput));
    }
    if let Some(info) = status(engine, op_id).await? {
        if info.track_id != track_id
            || info.source_id != source_id
            || info.recipient_chat_id != recipient_chat_id
        {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        return Ok(info);
    }
    engine.require_online(cancel).await?;
    let state = engine.state().await?;
    if crate::imports::has_durable_import(&engine.reader(), op_id).await? {
        return Err(ApiError::new(ErrorCode::Conflict));
    }
    let _source = state
        .tracks
        .get(&track_id)
        .and_then(|track| track.sources.get(&source_id))
        .filter(|source| source.chat_id == engine.binding().chat_id)
        .ok_or_else(|| ApiError::new(ErrorCode::Conflict))?;
    if cancel.is_cancelled() {
        return Err(ApiError::new(ErrorCode::Cancelled));
    }
    let mut info = ShareInfo {
        op_id,
        track_id,
        source_id,
        recipient_chat_id,
        state: ShareState::Unknown,
        message_id: None,
        error: None,
    };
    save(engine, &info).await?;
    match engine
        .share_track(track_id, source_id, recipient_chat_id, cancel)
        .await
    {
        Ok(message) => {
            info.state = ShareState::Sent;
            info.message_id = Some(message.id);
        }
        Err(error) => {
            // These errors prove the single send was rejected or the guard never
            // dispatched it. Cancellation/closure/lost updates remain unknown.
            if error.definitely_not_sent() {
                info.state = ShareState::NotSent;
            }
            info.error = Some(error.into());
        }
    }
    save(engine, &info).await?;
    Ok(info)
}
