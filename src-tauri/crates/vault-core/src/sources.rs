use crate::{ApiError, ErrorCode, VaultResponse};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_domain::{Action, MessageId};
use vault_sync::SyncEngine;
use vault_telegram::Telegram;

pub(crate) async fn relink<T: Telegram>(
    engine: &mut SyncEngine<T>,
    track_id: Uuid,
    source_id: Uuid,
    message_id: MessageId,
    op_id: Uuid,
    cancel: &CancellationToken,
) -> Result<VaultResponse, ApiError> {
    engine.require_online(cancel).await?;
    if let Some(command) = engine.command(op_id).await? {
        if command.intent.entity_id != track_id
            || !matches!(&command.intent.action,Action::SourceRelink {source}
            if source.source_id==source_id&&source.message_id==message_id&&source.chat_id==engine.binding().chat_id)
        {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        return Ok(VaultResponse::Command(Box::new(command)));
    }
    let state = engine.state().await?;
    let mut track = state
        .tracks
        .get(&track_id)
        .cloned()
        .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
    let mut source = track
        .sources
        .get(&source_id)
        .cloned()
        .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
    source.chat_id = engine.binding().chat_id;
    source.message_id = message_id;
    track.sources = std::collections::BTreeMap::from([(source_id, source.clone())]);
    // A fresh cache forces verification of the replacement bytes. An old cache
    // hit must never prove that a newly selected Telegram message is identical.
    let temp = tempfile::tempdir()?;
    let cache = vault_media::CacheManager::open(temp.path(), vault_media::MAX_IMPORT_BYTES)?;
    let (_lease, checksum) = crate::backups::download_source(
        engine.transport(),
        &track,
        engine.binding().owner_id,
        &cache,
        cancel,
    )
    .await?;
    source.sha256 = Some(checksum.sha256);
    source.size_bytes = Some(checksum.bytes);
    let command = engine
        .execute(op_id, track_id, Action::SourceRelink { source }, cancel)
        .await?;
    Ok(VaultResponse::Command(Box::new(command)))
}
