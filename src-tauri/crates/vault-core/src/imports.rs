//! Durable upload/registration boundary. Unknown sends are reconciled by their
//! MEDIA marker; lack of a marker never proves an in-flight send was cancelled.
use crate::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_db::CommandStatus;
use vault_domain::{Action, Metadata, Source};
use vault_protocol::{MediaMarker, Record};
use vault_sync::{ProjectionReader, SyncEngine};
use vault_telegram::{RemoteMessage, Telegram};
mod artwork;
mod incoming;
mod storage;
pub(crate) use incoming::try_start as try_incoming;
pub(crate) use storage::Storage;

const PREFIX: &str = "import.v1:";

/// Retries use the durable import or receipt and must not reopen a revoked SAF URI.
pub(crate) async fn has_durable_import(
    reader: &ProjectionReader,
    op: Uuid,
) -> Result<bool, ApiError> {
    Ok(reader.command(op).await?.is_some()
        || !reader.local_settings(key(op), 0, 1).await?.is_empty()
        || reader.has_applied_operation(op).await?)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ImportStage {
    Staging,
    Prepared,
    UploadUnknown,
    Uploaded,
    Complete,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportInfo {
    pub op_id: Uuid,
    pub track_id: Uuid,
    pub source_id: Uuid,
    pub metadata: Metadata,
    pub sha256: Option<String>,
    pub size_bytes: u64,
    pub stage: ImportStage,
    #[schemars(with = "Option<String>")]
    pub media_message_id: Option<vault_domain::MessageId>,
}

// Private path is local-only, never included in events, query DTOs or backups.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    info: ImportInfo,
    path: PathBuf,
    #[serde(default)]
    staged_name: Option<String>,
    #[serde(default)]
    artwork_upload_unknown: bool,
    #[serde(default)]
    incoming: Option<incoming::Origin>,
}

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrphanMedia {
    pub import_id: Uuid,
    #[schemars(with = "String")]
    pub message_id: vault_domain::MessageId,
    pub sha256: Option<String>,
    pub size_bytes: Option<u64>,
    pub recoverable_import: bool,
}

fn key(op_id: Uuid) -> String {
    format!("{PREFIX}{op_id}")
}
fn decode(raw: &str, op_id: Uuid) -> Result<Journal, ApiError> {
    let journal: Journal =
        serde_json::from_str(raw).map_err(|_| ApiError::new(ErrorCode::Integrity))?;
    if journal.info.op_id != op_id {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    Ok(journal)
}
async fn save<T: Telegram>(engine: &SyncEngine<T>, journal: &Journal) -> Result<(), ApiError> {
    engine
        .set_local_setting(
            key(journal.info.op_id),
            serde_json::to_string(journal).map_err(|_| ApiError::new(ErrorCode::Internal))?,
        )
        .await?;
    Ok(())
}

pub(crate) async fn open_storage<T: Telegram>(engine: &SyncEngine<T>) -> Result<Storage, ApiError> {
    let database = engine.database_path().await?;
    let vault = engine.binding().vault_id;
    let storage = blocking(move || Storage::open(database, vault)).await?;
    // A crash may follow command commit but precede local-file cleanup. Resolve
    // this from the durable command receipt, not merely an upload ACK.
    let reader = engine.reader();
    let mut offset = 0;
    loop {
        let page = reader.local_settings(PREFIX.into(), offset, 500).await?;
        let count = page.len();
        for (key, raw) in page {
            let op = key
                .strip_prefix(PREFIX)
                .and_then(|value| value.parse().ok())
                .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
            let mut journal = decode(&raw, op)?;
            if journal.info.stage == ImportStage::Complete && journal.staged_name.is_none() {
                continue;
            }
            let command = engine.command(op).await?;
            if let Some(command) = &command {
                validate_command(command, &journal, engine.binding())?;
            }
            if journal.info.stage == ImportStage::Complete
                || command.is_some_and(|command| command.status == CommandStatus::Applied)
            {
                journal.info.stage = ImportStage::Complete;
                save(engine, &journal).await?;
                cleanup(engine, &storage, &mut journal).await?;
            }
        }
        if count < 500 {
            break;
        }
        offset = offset
            .checked_add(500)
            .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
    }
    Ok(storage)
}

async fn blocking<R: Send + 'static>(
    work: impl FnOnce() -> Result<R, ApiError> + Send + 'static,
) -> Result<R, ApiError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))?
}

async fn cleanup<T: Telegram>(
    engine: &SyncEngine<T>,
    storage: &Storage,
    journal: &mut Journal,
) -> Result<(), ApiError> {
    if journal.staged_name.is_none() {
        return Ok(());
    }
    let root = storage.root();
    let saved = journal.clone();
    blocking(move || Storage::remove(&root, &saved)).await?;
    journal.staged_name = None;
    journal.path = PathBuf::new();
    save(engine, journal).await
}

async fn cleanup_or_warn<T: Telegram>(
    engine: &SyncEngine<T>,
    storage: &Storage,
    journal: &mut Journal,
    context: &Context,
    events: &EventBus,
) {
    if let Err(error) = cleanup(engine, storage, journal).await {
        events.emit(context, EventData::Warning(error));
    }
}
pub(crate) async fn list(
    reader: &ProjectionReader,
    offset: u32,
    limit: u32,
) -> Result<Vec<ImportInfo>, ApiError> {
    let mut result = Vec::new();
    for (key, raw) in reader.local_settings(PREFIX.into(), offset, limit).await? {
        let op_id = key
            .strip_prefix(PREFIX)
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
        let mut info = decode(&raw, op_id)?.info;
        if reader
            .command(op_id)
            .await?
            .is_some_and(|c| c.status == CommandStatus::Applied)
        {
            info.stage = ImportStage::Complete;
        }
        result.push(info);
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn start<T: Telegram + Clone + 'static>(
    engine: &mut SyncEngine<T>,
    storage: &Storage,
    path: PathBuf,
    incoming: Option<(vault_domain::ChatId, vault_domain::MessageId)>,
    track_id: Uuid,
    source_id: Uuid,
    op_id: Uuid,
    allow_duplicate: bool,
    cancel: &CancellationToken,
    context: &Context,
    events: &EventBus,
) -> Result<VaultResponse, ApiError> {
    let incoming = incoming.map(|(chat_id, message_id)| incoming::Origin {
        chat_id,
        message_id,
    });
    if let Some(raw) = engine.local_setting(key(op_id)).await? {
        let journal = decode(&raw, op_id)?;
        if journal.info.track_id != track_id
            || journal.info.source_id != source_id
            || journal
                .incoming
                .as_ref()
                .is_some_and(|origin| Some(origin) != incoming.as_ref())
        {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        return resume(engine, storage, op_id, cancel, context, events).await;
    }
    // Compatibility with imports made before the upload journal existed.
    if let Some(command) = engine.command(op_id).await? {
        if command.intent.entity_id != track_id
            || !matches!(&command.intent.action, Action::TrackRegister { source, .. } if source.source_id == source_id)
        {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        return Ok(VaultResponse::Command(Box::new(command)));
    }
    engine.require_online(cancel).await?;
    if let Some(receipt) = engine.state().await?.applied.get(&op_id) {
        let original = engine
            .transport()
            .message(engine.binding().chat_id, receipt.message_id, cancel)
            .await?
            .and_then(|message| message.record().transpose())
            .transpose()?;
        let Some(Record::Event(event)) = original else {
            return Err(ApiError::new(ErrorCode::Conflict));
        };
        if event.op_id != op_id
            || event.entity_id != track_id
            || !matches!(&event.action,Action::TrackRegister {source,..} if source.source_id==source_id)
        {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        return Ok(VaultResponse::Command(Box::new(
            engine
                .execute(op_id, track_id, event.action, cancel)
                .await?,
        )));
    }
    let inspect_path = path.clone();
    let token = cancel.clone();
    let asset = tokio::task::spawn_blocking(move || {
        vault_media::inspect_with_cancel(&inspect_path, &token)
    })
    .await
    .map_err(|_| ApiError::new(ErrorCode::Internal))?
    .map_err(ApiError::from)?;
    let state = engine.state().await?;
    let duplicate_track_ids = state
        .tracks
        .iter()
        .filter(|(_, track)| {
            track
                .sources
                .values()
                .any(|s| s.sha256.as_deref() == Some(&asset.sha256))
        })
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    let possible_duplicate_track_ids = state
        .tracks
        .iter()
        .filter(|(id, track)| {
            !duplicate_track_ids.contains(id)
                && vault_media::metadata_similar(&asset.metadata, &track.metadata)
        })
        .map(|(id, _)| *id)
        .collect::<Vec<_>>();
    if (!duplicate_track_ids.is_empty() || !possible_duplicate_track_ids.is_empty())
        && !allow_duplicate
    {
        return Ok(VaultResponse::ImportReview {
            metadata: Box::new(asset.metadata),
            sha256: Some(asset.sha256),
            size_bytes: asset.size_bytes,
            duplicate_track_ids,
            possible_duplicate_track_ids,
        });
    }
    if state.tracks.contains_key(&track_id) || state.tombstones.tracks.contains(&track_id) {
        return Err(ApiError::new(ErrorCode::Conflict));
    }
    let journal = Journal {
        incoming,
        artwork_upload_unknown: false,
        path,
        staged_name: Some(asset.filename),
        info: ImportInfo {
            op_id,
            track_id,
            source_id,
            metadata: asset.metadata,
            sha256: Some(asset.sha256),
            size_bytes: asset.size_bytes,
            stage: ImportStage::Staging,
            media_message_id: None,
        },
    };
    save(engine, &journal).await?;
    events.emit(context, EventData::Import(Box::new(journal.info.clone())));
    resume(engine, storage, op_id, cancel, context, events).await
}

pub(crate) async fn pending<T: Telegram>(
    engine: &SyncEngine<T>,
    op_id: Uuid,
) -> Result<bool, ApiError> {
    Ok(engine.local_setting(key(op_id)).await?.is_some() || engine.command(op_id).await?.is_some())
}

pub(crate) async fn resume<T: Telegram + Clone + 'static>(
    engine: &mut SyncEngine<T>,
    storage: &Storage,
    op_id: Uuid,
    cancel: &CancellationToken,
    context: &Context,
    events: &EventBus,
) -> Result<VaultResponse, ApiError> {
    engine.require_online(cancel).await?;
    let raw = engine
        .local_setting(key(op_id))
        .await?
        .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
    let mut journal = decode(&raw, op_id)?;
    if let Some(command) = engine.command(op_id).await? {
        validate_command(&command, &journal, engine.binding())?;
        if command.status == CommandStatus::Applied {
            journal.info.stage = ImportStage::Complete;
            save(engine, &journal).await?;
            cleanup_or_warn(engine, storage, &mut journal, context, events).await;
        }
        events.emit(context, EventData::Import(Box::new(journal.info)));
        events.emit(context, EventData::Command(Box::new(command.clone())));
        return Ok(VaultResponse::Command(Box::new(command)));
    }
    if journal.info.stage == ImportStage::UploadUnknown {
        let messages = vault_telegram::scan_history(
            engine.transport(),
            engine.binding().chat_id,
            0,
            None,
            1_000_000,
            cancel,
        )
        .await?;
        let mut found = None;
        for message in messages {
            if matches!(message.record()?, Some(Record::Media(ref marker)) if marker.import_id == op_id && marker.vault_id == engine.binding().vault_id)
            {
                validate_media(&message, &journal.info, engine.binding().vault_id)?;
                if found.is_some() {
                    return Err(ApiError::new(ErrorCode::Conflict));
                }
                found = Some(message.id);
            }
        }
        journal.info.media_message_id =
            Some(found.ok_or_else(|| ApiError::new(ErrorCode::Timeout))?);
        journal.info.stage = ImportStage::Uploaded;
        save(engine, &journal).await?;
    }
    if journal.info.stage == ImportStage::Prepared
        && journal.staged_name.is_none()
        && journal.incoming.is_none()
    {
        // Migrate the old original-path-only journal on its first resume.
        let path = journal.path.clone();
        let token = cancel.clone();
        let asset =
            blocking(move || vault_media::inspect_with_cancel(&path, &token).map_err(Into::into))
                .await?;
        if Some(&asset.sha256) != journal.info.sha256.as_ref()
            || asset.size_bytes != journal.info.size_bytes
        {
            return Err(ApiError::new(ErrorCode::Integrity));
        }
        journal.staged_name = Some(asset.filename);
        journal.info.stage = ImportStage::Staging;
        save(engine, &journal).await?;
    }
    if journal.info.stage == ImportStage::Staging {
        let root = storage.root();
        let saved = journal.clone();
        let token = cancel.clone();
        blocking(move || Storage::prepare(&root, &saved, &token)).await?;
        journal.path = PathBuf::new();
        journal.info.stage = ImportStage::Prepared;
        save(engine, &journal).await?;
        events.emit(context, EventData::Import(Box::new(journal.info.clone())));
    }
    if journal.info.stage == ImportStage::Prepared {
        let staged = if journal.incoming.is_none() || journal.staged_name.is_some() {
            let root = storage.root();
            let saved = journal.clone();
            let token = cancel.clone();
            Some(blocking(move || Storage::verified(&root, &saved, &token)).await?)
        } else {
            None
        };
        if cancel.is_cancelled() {
            return Err(ApiError::new(ErrorCode::Cancelled));
        }
        let marker = Record::Media(MediaMarker {
            vault_id: engine.binding().vault_id,
            import_id: op_id,
            sha256: journal.info.sha256.clone(),
        });
        let caption = vault_protocol::encode_text(&marker)
            .map_err(|_| ApiError::new(ErrorCode::Integrity))?;
        // Commit before the irreversible send. Cancellation/crash in this window
        // is deliberately ambiguous and must not automatically create a duplicate.
        journal.info.stage = ImportStage::UploadUnknown;
        save(engine, &journal).await?;
        let uploaded = if let Some(origin) = &journal.incoming {
            engine
                .copy_media(origin.chat_id, origin.message_id, caption, cancel)
                .await
        } else {
            let staged = staged.ok_or_else(|| ApiError::new(ErrorCode::Integrity))?;
            engine
                .upload_audio(staged, &journal.info.metadata, caption, cancel)
                .await
        };
        let media = match uploaded {
            Ok(media) => media,
            Err(error) => {
                if error.definitely_not_sent() {
                    journal.info.stage = ImportStage::Prepared;
                    save(engine, &journal).await?;
                }
                events.emit(context, EventData::Import(Box::new(journal.info)));
                return Err(error.into());
            }
        };
        validate_media(&media, &journal.info, engine.binding().vault_id)?;
        journal.info.media_message_id = Some(media.id);
        journal.info.stage = ImportStage::Uploaded;
        save(engine, &journal).await?;
        events.emit(context, EventData::Import(Box::new(journal.info.clone())));
    }
    incoming::verify_copy(engine, &journal, cancel).await?;
    let cover = incoming::prepare_cover(engine, &mut journal, cancel).await?;
    artwork::publish(
        engine,
        storage,
        &mut journal,
        cover.as_ref().map(|file| file.path().to_owned()),
        cancel,
    )
    .await?;
    let source = Source {
        source_id: journal.info.source_id,
        chat_id: engine.binding().chat_id,
        message_id: journal
            .info
            .media_message_id
            .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?,
        sha256: journal.info.sha256.clone(),
        size_bytes: Some(journal.info.size_bytes),
    };
    let command = engine
        .execute(
            op_id,
            journal.info.track_id,
            Action::TrackRegister {
                metadata: journal.info.metadata.clone(),
                source,
            },
            cancel,
        )
        .await?;
    if command.status == CommandStatus::Applied {
        journal.info.stage = ImportStage::Complete;
        save(engine, &journal).await?;
        cleanup_or_warn(engine, storage, &mut journal, context, events).await;
    }
    events.emit(context, EventData::Import(Box::new(journal.info)));
    events.emit(context, EventData::Command(Box::new(command.clone())));
    Ok(VaultResponse::Command(Box::new(command)))
}

fn validate_command(
    command: &vault_db::Command,
    journal: &Journal,
    binding: vault_db::Binding,
) -> Result<(), ApiError> {
    if command.intent.op_id != journal.info.op_id
        || command.intent.entity_id != journal.info.track_id
        || command.intent.vault_id != binding.vault_id
        || !matches!(&command.intent.action, Action::TrackRegister { metadata, source }
            if metadata == &journal.info.metadata && source.source_id == journal.info.source_id
            && source.chat_id == binding.chat_id && Some(source.message_id) == journal.info.media_message_id
            && source.sha256 == journal.info.sha256 && source.size_bytes == Some(journal.info.size_bytes))
    {
        return Err(ApiError::new(ErrorCode::Conflict));
    }
    Ok(())
}

fn validate_media(
    message: &RemoteMessage,
    info: &ImportInfo,
    vault_id: Uuid,
) -> Result<(), ApiError> {
    if message.record()?
        != Some(Record::Media(MediaMarker {
            vault_id,
            import_id: info.op_id,
            sha256: info.sha256.clone(),
        }))
        || message
            .file
            .as_ref()
            .is_none_or(|file| u64::try_from(file.size).ok() != Some(info.size_bytes))
    {
        return Err(ApiError::new(ErrorCode::Integrity));
    }
    Ok(())
}

pub(crate) async fn orphans<T: Telegram>(
    engine: &mut SyncEngine<T>,
    cancel: &CancellationToken,
) -> Result<Vec<OrphanMedia>, ApiError> {
    engine.require_online(cancel).await?;
    let state = engine.state().await?;
    let referenced = state
        .tracks
        .values()
        .flat_map(|t| {
            t.sources
                .values()
                .chain(t.metadata.artwork.as_ref().and_then(|a| a.source.as_ref()))
        })
        .map(|s| (s.chat_id, s.message_id))
        .collect::<std::collections::BTreeSet<_>>();
    let mut result = Vec::new();
    for message in vault_telegram::scan_history(
        engine.transport(),
        engine.binding().chat_id,
        0,
        None,
        1_000_000,
        cancel,
    )
    .await?
    {
        if !referenced.contains(&(message.chat_id, message.id))
            && let Some(Record::Media(marker)) = message.record()?
            && marker.vault_id == engine.binding().vault_id
        {
            result.push(OrphanMedia {
                import_id: marker.import_id,
                message_id: message.id,
                sha256: marker.sha256,
                size_bytes: message.file.and_then(|f| u64::try_from(f.size).ok()),
                recoverable_import: engine.local_setting(key(marker.import_id)).await?.is_some(),
            });
        }
    }
    Ok(result)
}
