use crate::*;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_sync::{ProjectionReader, RuntimeMode, SyncEngine, SyncError};
use vault_telegram::{Telegram, Update};

struct Job {
    request: VaultRequest,
    cancel: CancellationToken,
    reply: oneshot::Sender<Result<VaultResponse, ApiError>>,
}
struct DownloadJob {
    cancel: CancellationToken,
    info: DownloadInfo,
    updated: Instant,
    task: Option<tokio::task::JoinHandle<()>>,
}
struct Lifetime(CancellationToken);
impl Drop for Lifetime {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

/// Bounded single writer. Local projection reads do not wait for network jobs.
#[derive(Clone)]
pub struct VaultService {
    context: Context,
    requests: mpsc::Sender<Job>,
    reader: ProjectionReader,
    status: watch::Receiver<Status>,
    closed: watch::Receiver<bool>,
    lifetime: Arc<Lifetime>,
    cache: Option<vault_media::CacheManager>,
    downloads: Arc<Mutex<HashMap<Uuid, DownloadJob>>>,
}
impl VaultService {
    pub fn start<T: Telegram + Clone + 'static>(
        engine: SyncEngine<T>,
        context: Context,
        events: EventBus,
    ) -> Self {
        Self::start_inner(engine, context, events, None)
    }
    pub fn start_with_cache<T: Telegram + Clone + 'static>(
        engine: SyncEngine<T>,
        context: Context,
        events: EventBus,
        cache: vault_media::CacheManager,
    ) -> Self {
        Self::start_inner(engine, context, events, Some(cache))
    }
    fn start_inner<T: Telegram + Clone + 'static>(
        engine: SyncEngine<T>,
        mut context: Context,
        events: EventBus,
        cache: Option<vault_media::CacheManager>,
    ) -> Self {
        context.vault_id = Some(engine.binding().vault_id);
        let reader = engine.reader();
        let updates = engine.subscribe();
        let (requests, jobs) = mpsc::channel(64);
        let (status_tx, status) = watch::channel(Status {
            context: context.clone(),
            mode: engine.mode(),
            vault: None,
        });
        let (closed_tx, closed) = watch::channel(false);
        let cancel = CancellationToken::new();
        let downloads = Arc::new(Mutex::new(HashMap::new()));
        tokio::spawn(run(
            engine,
            context.clone(),
            events,
            jobs,
            updates,
            status_tx,
            closed_tx,
            cancel.clone(),
            cache.clone(),
            downloads.clone(),
        ));
        Self {
            context,
            requests,
            reader,
            status,
            closed,
            lifetime: Arc::new(Lifetime(cancel)),
            cache,
            downloads,
        }
    }
    pub fn context(&self) -> &Context {
        &self.context
    }
    pub fn reader(&self) -> ProjectionReader {
        self.reader.clone()
    }
    pub fn status(&self) -> Status {
        self.status.borrow().clone()
    }
    pub fn status_changes(&self) -> watch::Receiver<Status> {
        self.status.clone()
    }
    pub async fn request(
        &self,
        request: VaultRequest,
        cancel: &CancellationToken,
    ) -> Result<VaultResponse, ApiError> {
        if self.lifetime.0.is_cancelled() {
            return Err(ApiError::new(ErrorCode::Closed));
        }
        if cancel.is_cancelled() {
            return Err(ApiError::new(ErrorCode::Cancelled));
        }
        match &request {
            VaultRequest::GetDownloadStatus {
                download_id,
                track_id,
                source_id,
            } => {
                return Ok(VaultResponse::Download(self.download_status(
                    *download_id,
                    *track_id,
                    *source_id,
                )?));
            }
            VaultRequest::CancelDownload { download_id } => {
                self.cancel_download(*download_id)?;
                let jobs = self
                    .downloads
                    .lock()
                    .map_err(|_| ApiError::new(ErrorCode::Internal))?;
                let info = jobs
                    .get(download_id)
                    .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?
                    .info
                    .clone();
                return Ok(VaultResponse::Download(info));
            }
            _ => {}
        }
        let token = self.lifetime.0.child_token();
        let guard = token.clone().drop_guard();
        let download = match &request {
            VaultRequest::DownloadForOffline {
                download_id,
                track_id,
                pinned,
                ..
            } => Some((
                *download_id,
                DownloadInfo {
                    download_id: *download_id,
                    track_id: *track_id,
                    state: DownloadState::Downloading,
                    bytes_downloaded: 0,
                    total_bytes: None,
                    pinned: *pinned,
                },
            )),
            _ => None,
        };
        let download_cancel = download.as_ref().map(|_| self.lifetime.0.child_token());
        if let Some((id, info)) = &download {
            let mut jobs = self
                .downloads
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal))?;
            if jobs.contains_key(id) {
                return Err(ApiError::new(ErrorCode::Conflict));
            }
            prune_downloads(&mut jobs);
            jobs.insert(
                *id,
                DownloadJob {
                    cancel: download_cancel
                        .as_ref()
                        .cloned()
                        .ok_or_else(|| ApiError::new(ErrorCode::Internal))?,
                    info: info.clone(),
                    updated: Instant::now(),
                    task: None,
                },
            );
        }
        let (reply, result) = oneshot::channel();
        if let Err(error) = self.requests.try_send(Job {
            request,
            cancel: token,
            reply,
        }) {
            if let Some(download_cancel) = &download_cancel {
                download_cancel.cancel();
            }
            if let Some((id, _)) = &download
                && let Ok(mut jobs) = self.downloads.lock()
            {
                jobs.remove(id);
            }
            return Err(ApiError::new(match error {
                mpsc::error::TrySendError::Full(_) => ErrorCode::Backpressure,
                mpsc::error::TrySendError::Closed(_) => ErrorCode::Closed,
            }));
        }
        let value = tokio::select! {
            _=cancel.cancelled()=>Err(ApiError::new(ErrorCode::Cancelled)),
            result=result=>result.map_err(|_|ApiError::new(ErrorCode::Closed))?
        };
        drop(guard);
        value
    }
    pub async fn shutdown(&self) -> Result<(), ApiError> {
        self.lifetime.0.cancel();
        let mut closed = self.closed.clone();
        tokio::time::timeout(Duration::from_secs(35), async move {
            while !*closed.borrow_and_update() {
                if closed.changed().await.is_err() {
                    break;
                }
            }
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Timeout))?;
        let tasks = {
            let mut downloads = self
                .downloads
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal))?;
            downloads
                .values_mut()
                .filter_map(|job| job.task.take())
                .collect::<Vec<_>>()
        };
        tokio::time::timeout(Duration::from_secs(35), async move {
            for task in tasks {
                let _ = task.await;
            }
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Timeout))?;
        Ok(())
    }
    pub fn cancel_download(&self, id: Uuid) -> Result<(), ApiError> {
        let jobs = self
            .downloads
            .lock()
            .map_err(|_| ApiError::new(ErrorCode::Internal))?;
        let job = jobs
            .get(&id)
            .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
        if job.info.state != DownloadState::Downloading {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        job.cancel.cancel();
        Ok(())
    }
    pub fn download_status(
        &self,
        id: Uuid,
        track_id: Uuid,
        source_id: Uuid,
    ) -> Result<DownloadInfo, ApiError> {
        if let Ok(jobs) = self.downloads.lock()
            && let Some(job) = jobs.get(&id)
        {
            return Ok(job.info.clone());
        }
        let key = media_cache_key(track_id, source_id);
        if let Some(cache) = &self.cache
            && let Some(lease) = cache.acquire(&key).map_err(ApiError::from)?
        {
            return Ok(DownloadInfo {
                download_id: id,
                track_id,
                state: DownloadState::FullyCached,
                bytes_downloaded: lease.entry.size_bytes,
                total_bytes: Some(lease.entry.size_bytes),
                pinned: lease.entry.pinned,
            });
        }
        Ok(DownloadInfo {
            download_id: id,
            track_id,
            state: DownloadState::RemoteOnly,
            bytes_downloaded: 0,
            total_bytes: None,
            pinned: false,
        })
    }
}

fn prune_downloads(jobs: &mut HashMap<Uuid, DownloadJob>) {
    if jobs.len() < 512 {
        return;
    }
    let mut terminal = jobs
        .iter()
        .filter(|(_, job)| job.info.state != DownloadState::Downloading)
        .map(|(id, job)| (*id, job.updated))
        .collect::<Vec<_>>();
    terminal.sort_by_key(|(_, updated)| *updated);
    for (id, _) in terminal.into_iter().take(jobs.len().saturating_sub(384)) {
        jobs.remove(&id);
    }
}

// The actor owns all mutable sync state and exclusively performs remote writes.
#[allow(clippy::too_many_arguments)]
async fn run<T: Telegram + Clone + 'static>(
    mut engine: SyncEngine<T>,
    context: Context,
    events: EventBus,
    mut jobs: mpsc::Receiver<Job>,
    mut updates: tokio::sync::broadcast::Receiver<Update>,
    status: watch::Sender<Status>,
    closed: watch::Sender<bool>,
    cancel: CancellationToken,
    cache: Option<vault_media::CacheManager>,
    downloads: Arc<Mutex<HashMap<Uuid, DownloadJob>>>,
) {
    let mut last_cursor = String::new();
    let mut retry: Option<tokio::time::Instant> = None;
    let mut backoff = Duration::from_secs(2);
    let mut listening = true;
    if let Err(error) = engine.reconcile_interrupted_commands().await {
        events.emit(&context, EventData::Warning(error.into()));
        closed.send_replace(true);
        return;
    }
    let initial = engine.sync(&cancel).await;
    report_error(&initial, &context, &events);
    refresh(&mut engine, &context, &events, &status, &mut last_cursor).await;
    schedule_retry(&engine, &mut retry, &mut backoff);
    loop {
        let deadline =
            retry.unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(86400));
        tokio::select! {
            biased;
            _=cancel.cancelled()=>break,
            job=jobs.recv()=> {
                let Some(job)=job else {break};
                let download_id = match &job.request {
                    VaultRequest::DownloadForOffline { download_id, .. } => Some(*download_id),
                    _ => None,
                };
                let result=if job.cancel.is_cancelled(){Err(ApiError::new(ErrorCode::Cancelled))}
                    else {handle(&mut engine,job.request,&job.cancel,&context,&events,cache.as_ref(),&downloads).await};
                if let (Some(id), Err(error)) = (download_id, &result) {
                    finish_download_error(id, error, &downloads, &context, &events);
                }
                if let Err(error)=&result {events.emit(&context,EventData::Warning(error.clone()));}
                refresh(&mut engine,&context,&events,&status,&mut last_cursor).await;
                schedule_retry(&engine,&mut retry,&mut backoff);
                let _=job.reply.send(result);
            },
            update=updates.recv(),if listening=> {
                let relevant=match update {
                    Ok(update)=>relevant(&update,engine.binding().chat_id),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_))=>true,
                    Err(tokio::sync::broadcast::error::RecvError::Closed)=>{listening=false;false}
                };
                if relevant {
                    // Briefly coalesce a Telegram update burst. Cancellation remains responsive.
                    tokio::select!{_=cancel.cancelled()=>break,_=tokio::time::sleep(Duration::from_millis(40))=>{}}
                    while updates.try_recv().is_ok() {}
                    refresh(&mut engine,&context,&events,&status,&mut last_cursor).await;
                    let result=engine.sync(&cancel).await;
                    report_error(&result,&context,&events);
                    refresh(&mut engine,&context,&events,&status,&mut last_cursor).await;
                    schedule_retry(&engine,&mut retry,&mut backoff);
                }
            },
            _=tokio::time::sleep_until(deadline),if retry.is_some()=>{
                let result=engine.sync(&cancel).await;
                report_error(&result,&context,&events);
                refresh(&mut engine,&context,&events,&status,&mut last_cursor).await;
                schedule_retry(&engine,&mut retry,&mut backoff);
            }
        }
    }
    jobs.close();
    while let Some(job) = jobs.recv().await {
        let _ = job.reply.send(Err(ApiError::new(ErrorCode::Closed)));
    }
    let final_status = Status {
        context: context.clone(),
        mode: RuntimeMode::AuthRequired,
        vault: None,
    };
    status.send_replace(final_status.clone());
    events.emit(&context, EventData::Runtime(final_status));
    closed.send_replace(true);
}
fn schedule_retry<T: Telegram>(
    engine: &SyncEngine<T>,
    retry: &mut Option<tokio::time::Instant>,
    backoff: &mut Duration,
) {
    if matches!(
        engine.mode(),
        RuntimeMode::Reconnecting | RuntimeMode::Syncing
    ) {
        *retry = Some(tokio::time::Instant::now() + *backoff);
        *backoff = (*backoff * 2).min(Duration::from_secs(60));
    } else {
        *retry = None;
        *backoff = Duration::from_secs(2);
    }
}
fn relevant(update: &Update, chat: vault_domain::ChatId) -> bool {
    match update {
        Update::New(m) => m.chat_id == chat,
        Update::Changed { chat_id, .. } | Update::Deleted { chat_id, .. } => *chat_id == chat,
        _ => true,
    }
}
fn media_cache_key(track_id: Uuid, source_id: Uuid) -> String {
    format!("{track_id}:{source_id}")
}
fn update_download(downloads: &Arc<Mutex<HashMap<Uuid, DownloadJob>>>, info: DownloadInfo) {
    if let Ok(mut jobs) = downloads.lock()
        && let Some(job) = jobs.get_mut(&info.download_id)
    {
        job.info = info;
        job.updated = Instant::now();
    }
}
fn finish_download_error(
    id: Uuid,
    error: &ApiError,
    downloads: &Arc<Mutex<HashMap<Uuid, DownloadJob>>>,
    context: &Context,
    events: &EventBus,
) {
    let info = {
        let Ok(mut jobs) = downloads.lock() else {
            return;
        };
        let Some(job) = jobs.get_mut(&id) else { return };
        if job.info.state != DownloadState::Downloading {
            return;
        }
        job.info.state = match error.code {
            ErrorCode::Cancelled => DownloadState::Cancelled,
            ErrorCode::Network | ErrorCode::Timeout | ErrorCode::RateLimited => {
                DownloadState::FailedRetryable
            }
            _ => DownloadState::FailedPermanent,
        };
        job.updated = Instant::now();
        job.info.clone()
    };
    events.emit(context, EventData::Download(info));
}
fn report_error(result: &Result<(), SyncError>, context: &Context, events: &EventBus) {
    // Error conversion takes ownership; use only a safe category for background errors.
    if result.is_err() {
        events.emit(
            context,
            EventData::Warning(ApiError::new(ErrorCode::Readonly)),
        );
    }
}
async fn refresh<T: Telegram>(
    engine: &mut SyncEngine<T>,
    context: &Context,
    events: &EventBus,
    status: &watch::Sender<Status>,
    last_cursor: &mut String,
) {
    match engine.status().await {
        Ok(vault) => {
            if *last_cursor != vault.cursor {
                *last_cursor = vault.cursor.clone();
                events.emit(
                    context,
                    EventData::Library {
                        cursor: last_cursor.clone(),
                    },
                );
            }
            let next = Status {
                context: context.clone(),
                mode: vault.mode,
                vault: Some(vault),
            };
            let changed = status.borrow().mode != next.mode;
            status.send_replace(next.clone());
            if changed {
                events.emit(context, EventData::Runtime(next));
            }
        }
        Err(error) => {
            events.emit(context, EventData::Warning(error.into()));
        }
    }
}
async fn handle<T: Telegram + Clone + 'static>(
    engine: &mut SyncEngine<T>,
    request: VaultRequest,
    cancel: &CancellationToken,
    context: &Context,
    events: &EventBus,
    cache: Option<&vault_media::CacheManager>,
    downloads: &Arc<Mutex<HashMap<Uuid, DownloadJob>>>,
) -> Result<VaultResponse, ApiError> {
    Ok(match request {
        VaultRequest::Sync => {
            events.emit(context, EventData::Sync { running: true });
            let r = engine.sync(cancel).await;
            events.emit(context, EventData::Sync { running: false });
            r?;
            VaultResponse::Synced
        }
        VaultRequest::Initialize => {
            engine.initialize(cancel).await?;
            VaultResponse::Initialized
        }
        VaultRequest::Mutate(mutation) => {
            let command = engine
                .execute(mutation.op_id, mutation.entity_id, mutation.action, cancel)
                .await?;
            if command.intent.action.name().starts_with("playlist.") {
                events.emit(
                    context,
                    EventData::Playlist {
                        playlist_id: command.intent.entity_id,
                    },
                );
            }
            events.emit(context, EventData::Command(Box::new(command.clone())));
            VaultResponse::Command(Box::new(command))
        }
        VaultRequest::Verify => VaultResponse::Snapshot(engine.verify(cancel).await?.into()),
        VaultRequest::Snapshot => {
            events.emit(
                context,
                EventData::Snapshot {
                    running: true,
                    result: None,
                },
            );
            let result = engine
                .request_snapshot(cancel)
                .await
                .map(SnapshotInfo::from);
            events.emit(
                context,
                EventData::Snapshot {
                    running: false,
                    result: result.as_ref().ok().cloned(),
                },
            );
            VaultResponse::Snapshot(result?)
        }
        VaultRequest::PlanGc => VaultResponse::Gc(Some(engine.plan_gc(cancel).await?)),
        VaultRequest::InspectGc { plan_id } => VaultResponse::Gc(engine.inspect_gc(plan_id).await?),
        VaultRequest::ExecuteGc {
            plan_id,
            confirmed_hash,
        } => {
            events.emit(
                context,
                EventData::Gc {
                    running: true,
                    completed: 0,
                },
            );
            let result = engine.execute_gc(plan_id, &confirmed_hash, cancel).await;
            events.emit(
                context,
                EventData::Gc {
                    running: false,
                    completed: result.as_ref().map(|p| p.completed.len()).unwrap_or(0),
                },
            );
            VaultResponse::Gc(Some(result?))
        }
        VaultRequest::Import {
            path,
            track_id,
            source_id,
            op_id,
            allow_duplicate,
        } => {
            if path.len() > 4096 || track_id.is_nil() || source_id.is_nil() || op_id.is_nil() {
                return Err(ApiError::new(ErrorCode::InvalidInput));
            }
            let path = std::path::PathBuf::from(path);
            let (asset, staged) = tokio::task::spawn_blocking(move || {
                let asset = vault_media::inspect(&path)?;
                let staged = vault_media::stage(&asset, &std::env::temp_dir())?;
                Ok::<_, vault_media::MediaError>((asset, staged))
            })
            .await
            .map_err(|_| ApiError::new(ErrorCode::Internal))?
            .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?;
            let state = engine.state().await?;
            let duplicate_track_ids = state
                .tracks
                .iter()
                .filter(|(_, track)| {
                    track
                        .sources
                        .values()
                        .any(|source| source.sha256.as_deref() == Some(&asset.sha256))
                })
                .map(|(id, _)| *id)
                .collect::<Vec<_>>();
            if !duplicate_track_ids.is_empty() && !allow_duplicate {
                return Ok(VaultResponse::ImportReview {
                    metadata: Box::new(asset.metadata),
                    sha256: asset.sha256,
                    size_bytes: asset.size_bytes,
                    duplicate_track_ids,
                });
            }
            let marker = vault_protocol::Record::Media(vault_protocol::MediaMarker {
                vault_id: engine.binding().vault_id,
                import_id: op_id,
                sha256: Some(asset.sha256.clone()),
            });
            let caption = vault_protocol::encode_text(&marker)
                .map_err(|_| ApiError::new(ErrorCode::Integrity))?;
            let media = engine
                .upload_document(staged.path().to_owned(), caption, cancel)
                .await?;
            if media.file.is_none() {
                return Err(ApiError::new(ErrorCode::Integrity));
            }
            let source = vault_domain::Source {
                source_id,
                chat_id: engine.binding().chat_id,
                message_id: media.id,
                sha256: Some(asset.sha256),
                size_bytes: Some(asset.size_bytes),
            };
            let action = vault_domain::Action::TrackRegister {
                metadata: asset.metadata,
                source,
            };
            let command = engine.execute(op_id, track_id, action, cancel).await?;
            events.emit(context, EventData::Command(Box::new(command.clone())));
            VaultResponse::Command(Box::new(command))
        }
        VaultRequest::ShareTrack {
            track_id,
            source_id,
            recipient_chat_id,
        } => {
            let forwarded = engine
                .share_track(track_id, source_id, recipient_chat_id, cancel)
                .await?;
            events.emit(
                context,
                EventData::Share {
                    recipient_chat_id,
                    message_id: forwarded.id,
                },
            );
            VaultResponse::Shared {
                recipient_chat_id,
                message_id: forwarded.id,
            }
        }
        VaultRequest::DownloadForOffline {
            download_id,
            track_id,
            source_id,
            pinned,
        } => {
            if download_id.is_nil() || track_id.is_nil() || source_id.is_nil() {
                return Err(ApiError::new(ErrorCode::InvalidInput));
            }
            let cache = cache.ok_or_else(|| ApiError::new(ErrorCode::Storage))?;
            let key = media_cache_key(track_id, source_id);
            if let Some(lease) = cache.acquire(&key).map_err(ApiError::from)? {
                let info = DownloadInfo {
                    download_id,
                    track_id,
                    state: DownloadState::FullyCached,
                    bytes_downloaded: lease.entry.size_bytes,
                    total_bytes: Some(lease.entry.size_bytes),
                    pinned: lease.entry.pinned || pinned,
                };
                if pinned && !lease.entry.pinned {
                    cache.pin(&key, true).map_err(ApiError::from)?;
                }
                update_download(downloads, info.clone());
                events.emit(context, EventData::Download(info.clone()));
                VaultResponse::Download(info)
            } else {
                let (file, expected_sha, total) =
                    engine.media_file(track_id, source_id, cancel).await?;
                if file.size < 0 || u64::try_from(file.size).ok() != Some(total) {
                    return Err(ApiError::new(ErrorCode::Integrity));
                }
                if let Ok(mut jobs) = downloads.lock()
                    && let Some(job) = jobs.get_mut(&download_id)
                {
                    job.info.total_bytes = Some(total);
                    job.updated = Instant::now();
                }
                let initial = DownloadInfo {
                    download_id,
                    track_id,
                    state: DownloadState::Downloading,
                    bytes_downloaded: 0,
                    total_bytes: Some(total),
                    pinned,
                };
                events.emit(context, EventData::Download(initial.clone()));
                let download_cancel = downloads
                    .lock()
                    .map_err(|_| ApiError::new(ErrorCode::Internal))?
                    .get(&download_id)
                    .map(|job| job.cancel.clone())
                    .ok_or_else(|| ApiError::new(ErrorCode::Internal))?;
                let transport = engine.transport_clone();
                let cache = cache.clone();
                let task_downloads = downloads.clone();
                let task_events = events.clone();
                let task_context = context.clone();
                let task = tokio::spawn(async move {
                    let progress_downloads = task_downloads.clone();
                    let progress_events = task_events.clone();
                    let progress_context = task_context.clone();
                    let result = vault_media::download_to_cache(
                        &transport,
                        &file,
                        &cache,
                        vault_media::DownloadOptions {
                            cache_key: key,
                            expected_sha256: expected_sha,
                            pinned,
                        },
                        &download_cancel,
                        &move |bytes, total| {
                            let info = DownloadInfo {
                                download_id,
                                track_id,
                                state: DownloadState::Downloading,
                                bytes_downloaded: bytes,
                                total_bytes: Some(total),
                                pinned,
                            };
                            update_download(&progress_downloads, info.clone());
                            progress_events.emit(&progress_context, EventData::Download(info));
                        },
                    )
                    .await;
                    match result {
                        Ok(lease) => {
                            let info = DownloadInfo {
                                download_id,
                                track_id,
                                state: DownloadState::FullyCached,
                                bytes_downloaded: lease.entry.size_bytes,
                                total_bytes: Some(lease.entry.size_bytes),
                                pinned: lease.entry.pinned,
                            };
                            update_download(&task_downloads, info.clone());
                            task_events.emit(&task_context, EventData::Download(info));
                        }
                        Err(error) => {
                            let error = ApiError::from(error);
                            finish_download_error(
                                download_id,
                                &error,
                                &task_downloads,
                                &task_context,
                                &task_events,
                            );
                            task_events.emit(&task_context, EventData::Warning(error));
                        }
                    }
                });
                if let Ok(mut jobs) = downloads.lock()
                    && let Some(job) = jobs.get_mut(&download_id)
                {
                    job.task = Some(task);
                }
                VaultResponse::Download(initial)
            }
        }
        VaultRequest::PinCachedMedia {
            track_id,
            source_id,
            pinned,
        } => {
            let cache = cache.ok_or_else(|| ApiError::new(ErrorCode::Storage))?;
            cache
                .pin(&media_cache_key(track_id, source_id), pinned)
                .map_err(ApiError::from)?;
            VaultResponse::CachePinned
        }
        VaultRequest::ClearLocalCache => {
            let cache = cache
                .ok_or_else(|| ApiError::new(ErrorCode::Storage))?
                .clone();
            let bytes_removed = tokio::task::spawn_blocking(move || cache.clear_unpinned())
                .await
                .map_err(|_| ApiError::new(ErrorCode::Internal))?
                .map_err(ApiError::from)?;
            VaultResponse::CacheCleared { bytes_removed }
        }
        VaultRequest::GetDownloadStatus { .. } | VaultRequest::CancelDownload { .. } => {
            return Err(ApiError::new(ErrorCode::Internal));
        }
    })
}
