use crate::*;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_sync::{ProjectionReader, RuntimeMode, SyncEngine, SyncError};
use vault_telegram::{Telegram, Update};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreSession {
    fingerprint: String,
    plan: vault_snapshot::backup::RestorePlan,
}

struct Job {
    request: VaultRequest,
    cancel: CancellationToken,
    reply: oneshot::Sender<Result<VaultResponse, ApiError>>,
}
struct PlaybackJob {
    track_id: Uuid,
    source_id: Uuid,
    cancel: CancellationToken,
    reply: oneshot::Sender<Result<vault_media::PlaybackInput, ApiError>>,
}
struct DownloadJob {
    cancel: CancellationToken,
    info: DownloadInfo,
    source_id: Uuid,
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
    playback_requests: mpsc::Sender<PlaybackJob>,
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
        let (playback_requests, playback_jobs) = mpsc::channel(8);
        let (status_tx, status) = watch::channel(Status {
            context: context.clone(),
            mode: engine.mode(),
            vault: None,
        });
        let (closed_tx, closed) = watch::channel(false);
        let cancel = CancellationToken::new();
        let mut progress = engine.subscribe_progress();
        let token = cancel.clone();
        let progress_events = events.clone();
        let progress_context = context.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => break,
                    update = progress.recv() => match update {
                        Ok(update) => progress_events.emit(&progress_context, EventData::Operation(update)),
                        Err(broadcast::error::RecvError::Lagged(_)) => {},
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        });
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
            playback_jobs,
        ));
        Self {
            context,
            requests,
            playback_requests,
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
        let mut status = self.status.borrow().clone();
        if self.is_closed() {
            status.mode = RuntimeMode::DegradedReadonly;
            if let Some(vault) = &mut status.vault {
                vault.mode = status.mode;
            }
        }
        status
    }
    pub fn is_closed(&self) -> bool {
        *self.closed.borrow() || self.lifetime.0.is_cancelled()
    }
    pub fn status_changes(&self) -> watch::Receiver<Status> {
        self.status.clone()
    }
    /// Resolves a registered source through the same reconciled engine as other
    /// media operations. Encoded bytes are fetched later by the decoder worker.
    pub async fn playback_input(
        &self,
        track_id: Uuid,
        source_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<vault_media::PlaybackInput, ApiError> {
        if self.lifetime.0.is_cancelled() {
            return Err(ApiError::new(ErrorCode::Closed));
        }
        if cancel.is_cancelled() {
            return Err(ApiError::new(ErrorCode::Cancelled));
        }
        let token = cancel.child_token();
        let guard = token.clone().drop_guard();
        let (reply, result) = oneshot::channel();
        self.playback_requests
            .try_send(PlaybackJob {
                track_id,
                source_id,
                cancel: token,
                reply,
            })
            .map_err(|error| {
                ApiError::new(match error {
                    mpsc::error::TrySendError::Full(_) => ErrorCode::Backpressure,
                    mpsc::error::TrySendError::Closed(_) => ErrorCode::Closed,
                })
            })?;
        let value = tokio::select! {
            _ = cancel.cancelled() => Err(ApiError::new(ErrorCode::Cancelled)),
            result = result => result.map_err(|_| ApiError::new(ErrorCode::Closed))?,
        };
        drop(guard);
        value
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
                source_id,
                pinned,
            } => Some((
                *download_id,
                *track_id,
                *source_id,
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
        if let Some((id, track_id, source_id, info)) = &download {
            let mut jobs = self
                .downloads
                .lock()
                .map_err(|_| ApiError::new(ErrorCode::Internal))?;
            if jobs.contains_key(id)
                || jobs.values().any(|job| {
                    job.info.track_id == *track_id
                        && job.source_id == *source_id
                        && job.info.state == DownloadState::Downloading
                })
            {
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
                    source_id: *source_id,
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
            if let Some((id, _, _, _)) = &download
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
        cached_download_status(self.cache.as_ref(), id, track_id, source_id)
    }

    pub fn downloads(&self) -> Result<Vec<DownloadInfo>, ApiError> {
        let jobs = self.downloads.lock().map_err(|_| ApiError::new(ErrorCode::Internal))?;
        let mut entries: Vec<_> = jobs.values().collect();
        entries.sort_by_key(|job| job.updated);
        Ok(entries.into_iter().map(|job| job.info.clone()).collect())
    }
}

pub(crate) fn cached_download_status(
    cache: Option<&vault_media::CacheManager>,
    id: Uuid,
    track_id: Uuid,
    source_id: Uuid,
) -> Result<DownloadInfo, ApiError> {
    let key = media_cache_key(track_id, source_id);
    if let Some(cache) = cache
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
    if let Some(cache) = cache
        && let Some((bytes_downloaded, total_bytes)) = cache
            .partial_download_progress(&key)
            .map_err(ApiError::from)?
    {
        return Ok(DownloadInfo {
            download_id: id,
            track_id,
            state: DownloadState::PartiallyCached,
            bytes_downloaded,
            total_bytes: Some(total_bytes),
            pinned: false,
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
    mut playback_jobs: mpsc::Receiver<PlaybackJob>,
) {
    let mut last_cursor = ProjectionObservation::default();
    let mut retry: Option<tokio::time::Instant> = None;
    let mut backoff = Duration::from_secs(2);
    let mut listening = true;
    let import_storage = crate::imports::open_storage(&engine).await;
    if let Err(error) = &import_storage {
        events.emit(&context, EventData::Warning(error.clone()));
    }
    if let Err(error) = engine.reconcile_interrupted_commands().await {
        events.emit(&context, EventData::Warning(error.into()));
        closed.send_replace(true);
        return;
    }
    let initial = engine.sync(&cancel).await;
    report_error(&initial, &context, &events);
    refresh(&mut engine, &context, &events, &status, &mut last_cursor).await;
    schedule_retry(&engine, &mut retry, &mut backoff);
    let mut maintenance = match crate::maintenance::load(&engine).await {
        Ok(config) => config,
        Err(error) => {
            events.emit(&context, EventData::Warning(error));
            MaintenanceConfig {
                enabled: false,
                ..Default::default()
            }
        }
    };
    let mut last_activity = tokio::time::Instant::now();
    let mut next_maintenance =
        last_activity + Duration::from_secs(maintenance.check_interval_seconds);
    loop {
        let deadline =
            retry.unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(86400));
        tokio::select! {
            biased;
            _=cancel.cancelled()=>break,
            job=playback_jobs.recv()=> {
                let Some(job) = job else { break; };
                last_activity = tokio::time::Instant::now();
                let result = tokio::select! {
                    _ = cancel.cancelled() => Err(ApiError::new(ErrorCode::Closed)),
                    result = engine.media_file(job.track_id, job.source_id, &job.cancel) => result.map_err(ApiError::from),
                }.and_then(|(file, _, _)| {
                    vault_media::RemoteStream::new(engine.transport_clone(), file)
                        .map(|remote| vault_media::PlaybackInput::Remote(remote.with_lifetime(cancel.clone())))
                        .map_err(ApiError::from)
                });
                refresh(&mut engine, &context, &events, &status, &mut last_cursor).await;
                let _ = job.reply.send(result);
            },
            job=jobs.recv()=> {
                let Some(job)=job else {break};
                last_activity = tokio::time::Instant::now();
                if matches!(&job.request, VaultRequest::Sync | VaultRequest::Initialize) {
                    let mut syncing = status.borrow().clone();
                    syncing.mode = RuntimeMode::Syncing;
                    if let Some(vault) = &mut syncing.vault { vault.mode = RuntimeMode::Syncing; }
                    status.send_replace(syncing.clone());
                    events.emit(&context, EventData::Runtime(syncing));
                }
                let new_maintenance = match &job.request {
                    VaultRequest::SetMaintenanceConfig { config } => Some(config.clone()),
                    _ => None,
                };
                let download_id = match &job.request {
                    VaultRequest::DownloadForOffline { download_id, .. } => Some(*download_id),
                    _ => None,
                };
                let result=if job.cancel.is_cancelled(){Err(ApiError::new(ErrorCode::Cancelled))}
                    else {handle(&mut engine,job.request,&job.cancel,&context,&events,cache.as_ref(),&downloads,&import_storage).await};
                if let (Some(id), Err(error)) = (download_id, &result) {
                    finish_download_error(id, error, &downloads, &context, &events);
                }
                if let Err(error)=&result {events.emit(&context,EventData::Warning(error.clone()));}
                if result.is_ok() && let Some(config) = new_maintenance {
                    maintenance = config;
                    next_maintenance = tokio::time::Instant::now() + Duration::from_secs(maintenance.check_interval_seconds);
                }
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
                    let result=engine.catch_up(&cancel).await;
                    report_error(&result,&context,&events);
                    refresh(&mut engine,&context,&events,&status,&mut last_cursor).await;
                    schedule_retry(&engine,&mut retry,&mut backoff);
                }
            },
            _=tokio::time::sleep_until(deadline),if retry.is_some()=>{
                let result=engine.catch_up(&cancel).await;
                report_error(&result,&context,&events);
                refresh(&mut engine,&context,&events,&status,&mut last_cursor).await;
                schedule_retry(&engine,&mut retry,&mut backoff);
            },
            _=tokio::time::sleep_until(next_maintenance),if maintenance.enabled=>{
                if last_activity.elapsed() >= Duration::from_secs(maintenance.idle_seconds) {
                    let result = match crate::maintenance::due(&mut engine, &maintenance).await {
                        Ok(true) => handle(&mut engine, VaultRequest::Snapshot, &cancel, &context, &events, cache.as_ref(), &downloads, &import_storage).await.map(|_| ()),
                        Ok(false) => Ok(()),
                        Err(error) => Err(error),
                    };
                    if let Err(error) = result { events.emit(&context, EventData::Warning(error)); }
                    refresh(&mut engine,&context,&events,&status,&mut last_cursor).await;
                    schedule_retry(&engine,&mut retry,&mut backoff);
                }
                next_maintenance = tokio::time::Instant::now() + Duration::from_secs(maintenance.check_interval_seconds);
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
pub(crate) fn media_cache_key(track_id: Uuid, source_id: Uuid) -> String {
    format!("{track_id}:{source_id}")
}
fn restore_uuid(vault_id: Uuid, fingerprint: &str, track_id: Uuid, purpose: &[u8]) -> Uuid {
    let mut name = Vec::with_capacity(fingerprint.len() + 36 + purpose.len());
    name.extend_from_slice(fingerprint.as_bytes());
    name.push(b':');
    name.extend_from_slice(track_id.as_bytes());
    name.push(b':');
    name.extend_from_slice(purpose);
    Uuid::new_v5(&vault_id, &name)
}
async fn collect_restore_media<T: Telegram>(
    telegram: &T,
    chat_id: vault_domain::ChatId,
    vault_id: Uuid,
    cancel: &CancellationToken,
) -> Result<std::collections::BTreeMap<Uuid, vault_telegram::RemoteMessage>, ApiError> {
    let mut before = None;
    let mut found = std::collections::BTreeMap::new();
    for _ in 0..10_000 {
        let page = telegram.history(chat_id, before, 100, cancel).await?;
        if page.is_empty() {
            return Ok(found);
        }
        let mut oldest = None;
        for message in page {
            if message.chat_id != chat_id || before.is_some_and(|id| message.id >= id) {
                return Err(ApiError::new(ErrorCode::Integrity));
            }
            oldest =
                Some(oldest.map_or(message.id, |id: vault_domain::MessageId| id.min(message.id)));
            if let Some(vault_protocol::Record::Media(marker)) = message.record()?
                && marker.vault_id == vault_id
            {
                if let Some(existing) = found.get(&marker.import_id)
                    && existing.record()?.is_some_and(|record| {
                        matches!(record, vault_protocol::Record::Media(old) if old.sha256 != marker.sha256)
                    })
                {
                    return Err(ApiError::new(ErrorCode::Integrity));
                }
                let replace = found
                    .get(&marker.import_id)
                    .is_none_or(|existing| existing.id < message.id);
                if replace {
                    found.insert(marker.import_id, message);
                }
            }
        }
        let Some(oldest) = oldest else {
            return Err(ApiError::new(ErrorCode::Integrity));
        };
        before = Some(oldest);
    }
    Err(ApiError::new(ErrorCode::Integrity))
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
            ErrorCode::MediaSourceMissing => DownloadState::Unavailable,
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
    observed: &mut ProjectionObservation,
) {
    match engine.status().await {
        Ok(vault) => {
            if observed.cursor != vault.cursor {
                observed.cursor = vault.cursor.clone();
                events.emit(
                    context,
                    EventData::Library {
                        cursor: observed.cursor.clone(),
                    },
                );
                match engine.reader().playlist_revisions().await {
                    Ok(revisions) => {
                        for id in revisions
                            .keys()
                            .chain(observed.playlists.keys())
                            .copied()
                            .collect::<std::collections::BTreeSet<_>>()
                        {
                            if revisions.get(&id) != observed.playlists.get(&id) {
                                events.emit(context, EventData::Playlist { playlist_id: id });
                            }
                        }
                        observed.playlists = revisions;
                    }
                    Err(error) => events.emit(context, EventData::Warning(error.into())),
                }
            }
            match engine.reader().command_revisions().await {
                Ok(commands) => {
                    for command in commands {
                        let id = command.intent.op_id;
                        if observed.commands.get(&id) != Some(&command) {
                            observed.commands.insert(id, command.clone());
                            events.emit(context, EventData::Command(Box::new(command)));
                        }
                    }
                }
                Err(error) => events.emit(context, EventData::Warning(error.into())),
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
#[derive(Default)]
struct ProjectionObservation {
    cursor: String,
    playlists: std::collections::BTreeMap<Uuid, String>,
    commands: std::collections::BTreeMap<Uuid, vault_db::Command>,
}
#[allow(clippy::too_many_arguments)]
async fn handle<T: Telegram + Clone + 'static>(
    engine: &mut SyncEngine<T>,
    request: VaultRequest,
    cancel: &CancellationToken,
    context: &Context,
    events: &EventBus,
    cache: Option<&vault_media::CacheManager>,
    downloads: &Arc<Mutex<HashMap<Uuid, DownloadJob>>>,
    import_storage: &Result<crate::imports::Storage, ApiError>,
) -> Result<VaultResponse, ApiError> {
    Ok(match request {
        VaultRequest::GetMaintenanceConfig => {
            VaultResponse::MaintenanceConfig(crate::maintenance::load(engine).await?)
        }
        VaultRequest::SetMaintenanceConfig { config } => {
            crate::maintenance::save(engine, &config).await?;
            VaultResponse::MaintenanceConfig(config)
        }
        VaultRequest::PreviewIncomingAudio {
            chat_id,
            message_id,
        } => {
            engine.require_online(cancel).await?;
            VaultResponse::AudioInfo(
                engine
                    .transport()
                    .audio_info(chat_id, message_id, cancel)
                    .await?,
            )
        }
        VaultRequest::RelinkSource {
            track_id,
            source_id,
            message_id,
            op_id,
        } => crate::sources::relink(engine, track_id, source_id, message_id, op_id, cancel).await?,
        VaultRequest::Diagnose => VaultResponse::Diagnostics(engine.diagnostics().await?),
        VaultRequest::CheckSources { track_id } => {
            VaultResponse::Sources(engine.check_sources(track_id, cancel).await?)
        }
        VaultRequest::ResumeImport { op_id } => {
            crate::imports::resume(
                engine,
                import_storage.as_ref().map_err(Clone::clone)?,
                op_id,
                cancel,
                context,
                events,
            )
            .await?
        }
        VaultRequest::InspectOrphanMedia => {
            VaultResponse::OrphanMedia(crate::imports::orphans(engine, cancel).await?)
        }
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
        VaultRequest::RemoveCachedMedia {
            track_id,
            source_id,
        } => VaultResponse::CacheCleared {
            bytes_removed: cache
                .ok_or_else(|| ApiError::new(ErrorCode::Storage))?
                .remove_local(&media_cache_key(track_id, source_id))?,
        },
        VaultRequest::ListImportPreviews => VaultResponse::ImportPreviews(
            crate::imports::preview::list(import_storage.as_ref().map_err(Clone::clone)?).await?,
        ),
        VaultRequest::PreviewImport { path, preview_id } => {
            if path.len() > 4096 {
                return Err(ApiError::new(ErrorCode::InvalidInput));
            }
            VaultResponse::ImportPreview(Box::new(
                crate::imports::preview::inspect(
                    import_storage.as_ref().map_err(Clone::clone)?,
                    path.into(),
                    preview_id,
                    cancel,
                )
                .await?,
            ))
        }
        VaultRequest::DiscardImportPreview { preview_id } => {
            if crate::imports::pending(engine, preview_id).await? {
                return Err(ApiError::new(ErrorCode::Conflict));
            }
            crate::imports::preview::discard(
                import_storage.as_ref().map_err(Clone::clone)?,
                preview_id,
            )
            .await?;
            VaultResponse::PreviewDiscarded
        }
        VaultRequest::ImportPrepared {
            preview_id,
            track_id,
            source_id,
            edit,
            write_tags,
            allow_duplicate,
        } => {
            crate::imports::preview::commit(
                engine,
                import_storage.as_ref().map_err(Clone::clone)?,
                preview_id,
                track_id,
                source_id,
                edit.map(|e| *e),
                write_tags,
                allow_duplicate,
                cancel,
                context,
                events,
            )
            .await?
        }
        VaultRequest::RewriteTrack {
            op_id,
            track_id,
            edit,
        } => {
            crate::imports::rewrite::rewrite(
                engine,
                import_storage.as_ref().map_err(Clone::clone)?,
                cache.ok_or_else(|| ApiError::new(ErrorCode::Storage))?,
                track_id,
                op_id,
                *edit,
                cancel,
                context,
                events,
            )
            .await?
        }
        VaultRequest::EditMetadata {
            op_id,
            track_id,
            edit,
        } => crate::metadata::edit(engine, op_id, track_id, *edit, cancel).await?,
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
        VaultRequest::AbandonGc { plan_id } => {
            VaultResponse::Gc(Some(engine.abandon_gc(plan_id).await?))
        }
        VaultRequest::AssignGcCoordinator {
            op_id,
            target,
            expected_previous,
            force_confirmed,
        } => {
            let target = match target {
                CoordinatorTarget::ThisDevice => Some(engine.device_id().to_owned()),
                CoordinatorTarget::Device(id) => Some(id),
                CoordinatorTarget::Disabled => None,
            };
            let command = engine
                .assign_gc_coordinator(op_id, target, expected_previous, force_confirmed, cancel)
                .await?;
            events.emit(context, EventData::Command(Box::new(command.clone())));
            VaultResponse::Command(Box::new(command))
        }
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
            crate::imports::start(
                engine,
                import_storage.as_ref().map_err(Clone::clone)?,
                std::path::PathBuf::from(path),
                None,
                track_id,
                source_id,
                op_id,
                allow_duplicate,
                None,
                None,
                cancel,
                context,
                events,
            )
            .await?
        }
        VaultRequest::ImportIncomingAudio {
            source_chat_id,
            message_id,
            track_id,
            source_id,
            op_id,
            allow_duplicate,
        } => {
            if track_id.is_nil() || source_id.is_nil() || op_id.is_nil() {
                return Err(ApiError::new(ErrorCode::InvalidInput));
            }
            if let Some(response) = crate::imports::try_incoming(
                engine,
                import_storage.as_ref().map_err(Clone::clone)?,
                source_chat_id,
                message_id,
                track_id,
                source_id,
                op_id,
                allow_duplicate,
                cancel,
                context,
                events,
            )
            .await?
            {
                return Ok(response);
            }
            if crate::imports::pending(engine, op_id).await? {
                return crate::imports::start(
                    engine,
                    import_storage.as_ref().map_err(Clone::clone)?,
                    std::path::PathBuf::new(),
                    Some((source_chat_id, message_id)),
                    track_id,
                    source_id,
                    op_id,
                    allow_duplicate,
                    None,
                    None,
                    cancel,
                    context,
                    events,
                )
                .await;
            }
            let remote = engine
                .transport()
                .message(source_chat_id, message_id, cancel)
                .await?
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            if remote.record()?.is_some() {
                return Err(ApiError::new(ErrorCode::InvalidInput));
            }
            let file = remote
                .file
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            let bytes = u64::try_from(file.size)
                .ok()
                .filter(|size| *size > 0 && *size <= vault_media::MAX_IMPORT_BYTES)
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            let temporary = tempfile::tempdir().map_err(|_| ApiError::new(ErrorCode::Storage))?;
            let cache =
                vault_media::CacheManager::open(temporary.path(), bytes).map_err(ApiError::from)?;
            let lease = vault_media::download_to_cache(
                engine.transport(),
                &file,
                &cache,
                vault_media::DownloadOptions {
                    cache_key: format!("incoming:{source_chat_id}:{message_id}"),
                    expected_sha256: None,
                    pinned: true,
                },
                cancel,
                &|_, _| {},
            )
            .await
            .map_err(ApiError::from)?;
            let filename = std::path::Path::new(&file.name)
                .file_name()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty() && value.len() <= 240)
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            let _extension = std::path::Path::new(filename)
                .extension()
                .and_then(|value| value.to_str())
                .filter(|value| !value.is_empty() && value.len() <= 8)
                .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
            let audio_path = temporary.path().join(filename);
            let source_path = lease.path.clone();
            let destination_path = audio_path.clone();
            tokio::task::spawn_blocking(move || std::fs::copy(source_path, destination_path))
                .await
                .map_err(|_| ApiError::new(ErrorCode::Internal))?
                .map_err(|_| ApiError::new(ErrorCode::Storage))?;
            crate::imports::start(
                engine,
                import_storage.as_ref().map_err(Clone::clone)?,
                audio_path,
                Some((source_chat_id, message_id)),
                track_id,
                source_id,
                op_id,
                allow_duplicate,
                None,
                None,
                cancel,
                context,
                events,
            )
            .await?
        }
        VaultRequest::ShareTrack {
            op_id,
            track_id,
            source_id,
            recipient_chat_id,
        } => {
            let info = crate::sharing::send(
                engine,
                op_id,
                track_id,
                source_id,
                recipient_chat_id,
                cancel,
            )
            .await?;
            if let Some(message_id) = info.message_id {
                events.emit(
                    context,
                    EventData::Share {
                        recipient_chat_id,
                        message_id,
                    },
                );
            }
            VaultResponse::Shared(info)
        }
        VaultRequest::GetShareStatus { op_id } => {
            VaultResponse::ShareStatus(crate::sharing::status(engine, op_id).await?)
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
        VaultRequest::GetCacheStats => {
            let cache = cache.ok_or_else(|| ApiError::new(ErrorCode::Storage))?;
            VaultResponse::CacheStats(cache.stats().map_err(ApiError::from)?)
        }
        VaultRequest::SetCacheQuota { bytes } => {
            if bytes == 0 {
                return Err(ApiError::new(ErrorCode::InvalidInput));
            }
            let cache = cache
                .ok_or_else(|| ApiError::new(ErrorCode::Storage))?
                .clone();
            let stats = tokio::task::spawn_blocking(move || cache.set_quota(bytes))
                .await
                .map_err(|_| ApiError::new(ErrorCode::Internal))??;
            VaultResponse::CacheStats(stats)
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
        VaultRequest::ImportBackup { path } => {
            engine.require_online(cancel).await?;
            if path.len() > 4096 {
                return Err(ApiError::new(ErrorCode::InvalidInput));
            }
            let archive_path = std::path::PathBuf::from(path);
            let file_type = std::fs::symlink_metadata(&archive_path)
                .map_err(|_| ApiError::new(ErrorCode::InvalidInput))?;
            if file_type.file_type().is_symlink() || !file_type.is_file() {
                return Err(ApiError::new(ErrorCode::InvalidInput));
            }
            let backup = tokio::task::spawn_blocking(move || {
                vault_snapshot::backup::inspect(
                    &archive_path,
                    vault_snapshot::backup::Limits::default(),
                )
            })
            .await
            .map_err(|_| ApiError::new(ErrorCode::Internal))?
            .map_err(|_| ApiError::new(ErrorCode::Integrity))?;
            let fingerprint = vault_snapshot::digest(
                &vault_protocol::canonical_json(&backup.manifest)
                    .map_err(|_| ApiError::new(ErrorCode::Integrity))?,
            );
            let setting_key = "backup.restore.session.v1".to_owned();
            let persisted = engine
                .local_setting(setting_key.clone())
                .await?
                .map(|json| {
                    serde_json::from_str::<RestoreSession>(&json)
                        .map_err(|_| ApiError::new(ErrorCode::Integrity))
                })
                .transpose()?;
            let plan = if let Some(session) = persisted {
                if session.fingerprint != fingerprint
                    || session.plan.target_vault_id != engine.binding().vault_id
                    || session.plan.source_vault_id != backup.manifest.vault_id
                {
                    return Err(ApiError::new(ErrorCode::Conflict));
                }
                session.plan
            } else {
                let state = engine.state().await?;
                if state.generation != 0
                    || state.last_event_message_id.is_some()
                    || !state.tracks.is_empty()
                    || !state.playlists.is_empty()
                    || !state.entries.is_empty()
                    || !state.favorites.is_empty()
                    || !state.tombstones.tracks.is_empty()
                    || !state.tombstones.playlists.is_empty()
                    || !state.tombstones.entries.is_empty()
                    || !state.applied.is_empty()
                {
                    return Err(ApiError::new(ErrorCode::Conflict));
                }
                if backup.manifest.kind == vault_snapshot::backup::BackupKind::Metadata
                    && backup.manifest.owner_id != engine.binding().owner_id
                {
                    return Err(ApiError::new(ErrorCode::Conflict));
                }
                let target_vault = engine.binding().vault_id;
                let target_chat = engine.binding().chat_id;
                let mut sources = std::collections::BTreeMap::new();
                events.emit(
                    context,
                    EventData::BackupRestore {
                        phase: "media".into(),
                        completed: 0,
                        total: backup.state.tracks.len(),
                    },
                );
                let existing =
                    collect_restore_media(engine.transport(), target_chat, target_vault, cancel)
                        .await?;
                let temporary = tempfile::tempdir()?;
                let download_cache = vault_media::CacheManager::open(
                    temporary.path(),
                    vault_media::MAX_IMPORT_BYTES,
                )?;
                let mut artworks = std::collections::BTreeMap::new();
                for (old_track_id, track) in &backup.state.tracks {
                    let downloaded;
                    let (media_path, entry) = if let Some(path) = backup.media_path(*old_track_id) {
                        (
                            path.to_owned(),
                            backup
                                .manifest
                                .entries
                                .get(&format!("media/{old_track_id}"))
                                .ok_or_else(|| ApiError::new(ErrorCode::Integrity))?
                                .clone(),
                        )
                    } else {
                        let (lease, checksum) = crate::backups::download_source(
                            engine.transport(),
                            track,
                            engine.binding().owner_id,
                            &download_cache,
                            cancel,
                        )
                        .await?;
                        downloaded = lease;
                        (downloaded.path.clone(), checksum)
                    };
                    let import_id =
                        restore_uuid(target_vault, &fingerprint, *old_track_id, b"media");
                    let message = crate::backups::upload_restore(
                        engine,
                        &existing,
                        import_id,
                        media_path.clone(),
                        &entry,
                        cancel,
                    )
                    .await?;
                    if let Some(artwork) = crate::backups::restore_artwork(
                        engine,
                        &backup,
                        track,
                        &media_path,
                        &download_cache,
                        &existing,
                        restore_uuid(target_vault, &fingerprint, *old_track_id, b"artwork"),
                        cancel,
                    )
                    .await?
                    {
                        artworks.insert(
                            restore_uuid(target_vault, &fingerprint, *old_track_id, b"source"),
                            artwork,
                        );
                    }
                    sources.insert(
                        *old_track_id,
                        vault_domain::Source {
                            source_id: restore_uuid(
                                target_vault,
                                &fingerprint,
                                *old_track_id,
                                b"source",
                            ),
                            chat_id: target_chat,
                            message_id: message.id,
                            sha256: Some(entry.sha256.clone()),
                            size_bytes: Some(entry.bytes),
                        },
                    );
                    events.emit(
                        context,
                        EventData::BackupRestore {
                            phase: "media".into(),
                            completed: sources.len(),
                            total: backup.state.tracks.len(),
                        },
                    );
                    if !track.sources.values().any(|source| {
                        source
                            .sha256
                            .as_deref()
                            .is_none_or(|sha| sha == entry.sha256)
                    }) {
                        return Err(ApiError::new(ErrorCode::Integrity));
                    }
                }
                let mut plan = vault_snapshot::backup::plan_new_vault(
                    &backup,
                    target_vault,
                    target_chat,
                    &sources,
                )
                .map_err(|_| ApiError::new(ErrorCode::Integrity))?;
                for operation in &mut plan.operations {
                    if let vault_domain::Action::TrackRegister { metadata, source } =
                        &mut operation.action
                        && let Some(artwork) = metadata.artwork.as_mut()
                    {
                        artwork.source = artworks.get(&source.source_id).cloned();
                    }
                }
                let session = RestoreSession {
                    fingerprint,
                    plan: plan.clone(),
                };
                let json = serde_json::to_string(&session)
                    .map_err(|_| ApiError::new(ErrorCode::Internal))?;
                engine.set_local_setting(setting_key.clone(), json).await?;
                plan
            };
            events.emit(
                context,
                EventData::BackupRestore {
                    phase: "library".into(),
                    completed: 0,
                    total: plan.operations.len(),
                },
            );
            let mut applied = 0usize;
            for operation in &plan.operations {
                let command = engine
                    .execute(
                        operation.op_id,
                        operation.entity_id,
                        operation.action.clone(),
                        cancel,
                    )
                    .await?;
                match command.status {
                    vault_db::CommandStatus::Applied => applied += 1,
                    vault_db::CommandStatus::RejectedByReducer => {
                        return Err(ApiError::new(ErrorCode::Conflict));
                    }
                    _ => return Err(ApiError::new(ErrorCode::Timeout)),
                }
                events.emit(context, EventData::Command(Box::new(command)));
                events.emit(
                    context,
                    EventData::BackupRestore {
                        phase: "library".into(),
                        completed: applied,
                        total: plan.operations.len(),
                    },
                );
            }
            engine.clear_local_setting(setting_key).await?;
            VaultResponse::BackupRestored {
                source_vault_id: plan.source_vault_id,
                tracks: backup.state.tracks.len(),
                playlists: backup.state.playlists.len(),
                operations: applied,
            }
        }
        VaultRequest::GetDownloadStatus { .. } | VaultRequest::CancelDownload { .. } => {
            return Err(ApiError::new(ErrorCode::Internal));
        }
    })
}
