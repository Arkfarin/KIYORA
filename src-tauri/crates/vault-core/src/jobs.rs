//! Durable, explicitly resumable jobs. Registry access is independent of the
//! serialized vault actor, so cancellation remains available during network I/O.
use crate::*;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io::Write, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum JobRequest {
    Vault(VaultRequest),
    ExportBackup {
        path: String,
        kind: vault_snapshot::backup::BackupKind,
    },
    RepairDatabase,
    RecoverDatabaseFromBackup {
        path: String,
    },
    BulkImport {
        files: Vec<ImportFile>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ImportFile {
    pub path: String,
    pub track_id: Uuid,
    pub source_id: Uuid,
    pub op_id: Uuid,
    pub allow_duplicate: bool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum JobState {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JobInfo {
    pub job_id: Uuid,
    pub state: JobState,
    pub completed: usize,
    pub total: usize,
    pub result: Option<Box<JobResult>>,
    pub error: Option<ApiError>,
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(
    tag = "kind",
    content = "result",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum JobResult {
    Vault(VaultResponse),
    Backup(vault_snapshot::backup::Manifest),
    Repaired(SnapshotInfo),
    Bulk(Vec<BulkItemResult>),
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BulkItemResult {
    pub op_id: Uuid,
    pub result: Result<VaultResponse, ApiError>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Saved {
    request: JobRequest,
    info: JobInfo,
}
struct Entry {
    saved: Saved,
    cancel: CancellationToken,
}
#[derive(Clone)]
pub(crate) struct JobRegistry {
    path: PathBuf,
    entries: Arc<Mutex<BTreeMap<Uuid, Entry>>>,
    lifetime: CancellationToken,
    tasks: Arc<Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}
impl JobRegistry {
    pub(crate) fn cancellation(&self) -> CancellationToken {
        self.lifetime.clone()
    }
    pub(crate) fn open(path: PathBuf) -> Result<Self, ApiError> {
        std::fs::create_dir_all(&path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        }
        let lifetime = CancellationToken::new();
        let mut entries = BTreeMap::new();
        for file in std::fs::read_dir(&path)? {
            let file = file?;
            if file.path().extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            if entries.len() >= 128
                || file.metadata()?.len() > 64 * 1024 * 1024
                || !file.file_type()?.is_file()
            {
                return Err(ApiError::new(ErrorCode::Storage));
            }
            let mut saved: Saved = serde_json::from_slice(&std::fs::read(file.path())?)
                .map_err(|_| ApiError::new(ErrorCode::Storage))?;
            if file.file_name().to_string_lossy() != format!("{}.json", saved.info.job_id) {
                return Err(ApiError::new(ErrorCode::Integrity));
            }
            if saved.info.state == JobState::Running {
                saved.info.state = JobState::Interrupted;
            }
            entries.insert(
                saved.info.job_id,
                Entry {
                    saved,
                    cancel: lifetime.child_token(),
                },
            );
        }
        Ok(Self {
            path,
            entries: Arc::new(Mutex::new(entries)),
            lifetime,
            tasks: Arc::new(Mutex::new(Vec::new())),
        })
    }
    async fn persist(&self, saved: Saved) -> Result<(), ApiError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<(), ApiError> {
            let mut file = tempfile::NamedTempFile::new_in(&path)?;
            file.write_all(
                &serde_json::to_vec(&saved).map_err(|_| ApiError::new(ErrorCode::Internal))?,
            )?;
            file.as_file().sync_all()?;
            file.persist(path.join(format!("{}.json", saved.info.job_id)))
                .map_err(|_| ApiError::new(ErrorCode::Storage))?;
            #[cfg(unix)]
            std::fs::File::open(path)?.sync_all()?;
            Ok(())
        })
        .await
        .map_err(|_| ApiError::new(ErrorCode::Internal))?
    }
    async fn update(&self, info: JobInfo) -> Result<(), ApiError> {
        let mut entries = self.entries.lock().await;
        let entry = entries
            .get_mut(&info.job_id)
            .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
        let saved = Saved {
            request: entry.saved.request.clone(),
            info,
        };
        if let Err(error) = self.persist(saved.clone()).await {
            // An unpersisted success must not be returned by job_status, and a
            // completed worker must not remain Running and prevent resumption.
            entry.saved.info.state = JobState::Failed;
            entry.saved.info.error = Some(error.clone());
            return Err(error);
        }
        entry.saved = saved;
        Ok(())
    }
    pub(crate) fn cancel_all(&self) {
        self.lifetime.cancel();
    }
    pub(crate) async fn shutdown(&self) {
        self.cancel_all();
        let tasks = std::mem::take(&mut *self.tasks.lock().await);
        for task in tasks {
            let _ = task.await;
        }
    }
}
impl Backend {
    /// Resume the original durable request without exposing its paths to UI or
    /// generating a second import op_id after a process/WebView interruption.
    pub async fn resume_job(&self, session_id: Uuid, job_id: Uuid) -> Result<JobInfo, ApiError> {
        let registry = self.job_registry(session_id).await?;
        let request = registry.entries.lock().await.get(&job_id)
            .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?.saved.request.clone();
        self.start_job(session_id, job_id, request, true).await
    }
    pub async fn list_jobs(&self, session_id: Uuid) -> Result<Vec<JobInfo>, ApiError> {
        let registry = self.job_registry(session_id).await?;
        Ok(registry
            .entries
            .lock()
            .await
            .values()
            .map(|e| e.saved.info.clone())
            .collect())
    }
    pub async fn job_status(&self, session_id: Uuid, job_id: Uuid) -> Result<JobInfo, ApiError> {
        self.list_jobs(session_id)
            .await?
            .into_iter()
            .find(|job| job.job_id == job_id)
            .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))
    }
    pub async fn cancel_job(&self, session_id: Uuid, job_id: Uuid) -> Result<JobInfo, ApiError> {
        let registry = self.job_registry(session_id).await?;
        let entries = registry.entries.lock().await;
        let entry = entries
            .get(&job_id)
            .ok_or_else(|| ApiError::new(ErrorCode::InvalidInput))?;
        entry.cancel.cancel();
        Ok(entry.saved.info.clone())
    }
    pub async fn forget_job(&self, session_id: Uuid, job_id: Uuid) -> Result<(), ApiError> {
        let registry = self.job_registry(session_id).await?;
        let mut entries = registry.entries.lock().await;
        if entries
            .get(&job_id)
            .is_some_and(|e| e.saved.info.state == JobState::Running)
        {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        tokio::fs::remove_file(registry.path.join(format!("{job_id}.json"))).await?;
        entries.remove(&job_id);
        Ok(())
    }
    pub async fn start_job(
        &self,
        session_id: Uuid,
        job_id: Uuid,
        request: JobRequest,
        resume: bool,
    ) -> Result<JobInfo, ApiError> {
        validate_request(job_id, &request)?;
        let registry = self.job_registry(session_id).await?;
        // Serialize spawn with shutdown. A closed profile cannot acquire a new task.
        let mut tasks = registry.tasks.lock().await;
        if registry.lifetime.is_cancelled() {
            return Err(ApiError::new(ErrorCode::Closed));
        }
        let mut entries = registry.entries.lock().await;
        let total = match &request {
            JobRequest::BulkImport { files } => files.len(),
            _ => 1,
        };
        let mut info = JobInfo {
            job_id,
            state: JobState::Running,
            completed: 0,
            total,
            result: None,
            error: None,
        };
        if let Some(entry) = entries.get(&job_id) {
            if serde_json::to_value(&entry.saved.request).ok()
                != serde_json::to_value(&request).ok()
            {
                return Err(ApiError::new(ErrorCode::Conflict));
            }
            if !resume
                || matches!(
                    entry.saved.info.state,
                    JobState::Running | JobState::Succeeded
                )
            {
                return Ok(entry.saved.info.clone());
            }
            info.result = entry.saved.info.result.clone();
        } else if entries.len() >= 128 {
            return Err(ApiError::new(ErrorCode::Backpressure));
        }
        let cancel = registry.lifetime.child_token();
        let saved = Saved {
            request: request.clone(),
            info: info.clone(),
        };
        registry.persist(saved.clone()).await?;
        entries.insert(
            job_id,
            Entry {
                saved,
                cancel: cancel.clone(),
            },
        );
        drop(entries);
        let backend = self.clone();
        let worker_registry = registry.clone();
        let initial = info.clone();
        let context = self.job_access(session_id)?.0;
        if context.session_id != session_id {
            return Err(ApiError::new(ErrorCode::Conflict));
        }
        let events = self.events();
        events.emit(&context, EventData::Job(info.clone()));
        tasks.retain(|task| !task.is_finished());
        tasks.push(tokio::spawn(async move {
            let mut info = initial;
            let result = run_job(
                &backend,
                session_id,
                request,
                &cancel,
                &worker_registry,
                &mut info,
                &context,
                &events,
            )
            .await;
            match result {
                Ok(result) => {
                    info.state = JobState::Succeeded;
                    info.completed = info.total;
                    info.result = Some(Box::new(result));
                }
                Err(error) => {
                    info.state = if error.code == ErrorCode::Cancelled {
                        JobState::Cancelled
                    } else {
                        JobState::Failed
                    };
                    info.error = Some(error);
                }
            }
            if let Err(error) = worker_registry.update(info.clone()).await {
                info.state = JobState::Failed;
                info.error = Some(error);
            }
            events.emit(&context, EventData::Job(info));
        }));
        Ok(info)
    }
}
fn validate_request(id: Uuid, request: &JobRequest) -> Result<(), ApiError> {
    let invalid = || ApiError::new(ErrorCode::InvalidInput);
    if id.is_nil() {
        return Err(invalid());
    }
    match request {
        JobRequest::ExportBackup { path, .. } if path.is_empty() || path.len() > 4096 => {
            return Err(invalid());
        }
        JobRequest::BulkImport { files } => {
            if files.is_empty() || files.len() > 1000 {
                return Err(invalid());
            }
            let mut ids = std::collections::BTreeSet::new();
            for file in files {
                if file.path.is_empty()
                    || file.path.len() > 4096
                    || file.op_id.is_nil()
                    || file.track_id.is_nil()
                    || file.source_id.is_nil()
                    || !ids.insert(file.op_id)
                {
                    return Err(invalid());
                }
            }
        }
        _ => {}
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
async fn run_job(
    backend: &Backend,
    session: Uuid,
    request: JobRequest,
    cancel: &CancellationToken,
    registry: &JobRegistry,
    info: &mut JobInfo,
    context: &Context,
    events: &EventBus,
) -> Result<JobResult, ApiError> {
    if cancel.is_cancelled() {
        return Err(ApiError::new(ErrorCode::Cancelled));
    }
    Ok(match request {
        JobRequest::Vault(request) => {
            JobResult::Vault(backend.service_request(session, request, cancel).await?)
        }
        JobRequest::ExportBackup { path, kind } => JobResult::Backup(
            backend
                .export_backup_cancellable(session, path.into(), kind, cancel)
                .await?,
        ),
        JobRequest::RepairDatabase => {
            JobResult::Repaired(backend.repair_local_database(session, cancel).await?)
        }
        JobRequest::RecoverDatabaseFromBackup { path } => {
            let snapshot = backend
                .recover_database_from_backup(session, path.into(), cancel)
                .await?;
            JobResult::Repaired(snapshot)
        }
        JobRequest::BulkImport { files } => {
            let mut results = match info.result.clone().map(|r| *r) {
                Some(JobResult::Bulk(results)) => results,
                _ => Vec::new(),
            };
            for file in files {
                if cancel.is_cancelled() {
                    return Err(ApiError::new(ErrorCode::Cancelled));
                }
                if results
                    .iter()
                    .any(|r| r.op_id == file.op_id && r.result.is_ok())
                {
                    continue;
                }
                let result = backend
                    .service_request(
                        session,
                        VaultRequest::Import {
                            path: file.path,
                            track_id: file.track_id,
                            source_id: file.source_id,
                            op_id: file.op_id,
                            allow_duplicate: file.allow_duplicate,
                        },
                        cancel,
                    )
                    .await;
                let result = result.and_then(|response| match &response {
                    VaultResponse::Command(command)
                        if command.status == vault_db::CommandStatus::RejectedByReducer =>
                    {
                        Err(ApiError::new(ErrorCode::Conflict))
                    }
                    VaultResponse::Command(command)
                        if command.status != vault_db::CommandStatus::Applied =>
                    {
                        Err(ApiError::new(ErrorCode::Timeout))
                    }
                    _ => Ok(response),
                });
                if matches!(&result,Err(error) if error.code==ErrorCode::Cancelled) {
                    return Err(ApiError::new(ErrorCode::Cancelled));
                }
                results.retain(|r| r.op_id != file.op_id);
                results.push(BulkItemResult {
                    op_id: file.op_id,
                    result,
                });
                info.completed = results.len();
                info.result = Some(Box::new(JobResult::Bulk(results.clone())));
                registry.update(info.clone()).await?;
                events.emit(context, EventData::Job(info.clone()));
            }
            if results.iter().any(|r| r.result.is_err()) {
                return Err(ApiError::new(ErrorCode::Conflict));
            }
            JobResult::Bulk(results)
        }
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn failed_job_checkpoint_never_reports_success_or_blocks_explicit_retry() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("jobs");
        let registry = JobRegistry::open(root.clone()).unwrap();
        let id = Uuid::new_v4();
        let info = JobInfo {
            job_id: id,
            state: JobState::Running,
            completed: 0,
            total: 1,
            result: None,
            error: None,
        };
        registry
            .persist(Saved {
                request: JobRequest::Vault(VaultRequest::Sync),
                info: info.clone(),
            })
            .await
            .unwrap();
        drop(registry);
        let registry = JobRegistry::open(root.clone()).unwrap();
        let file = root.join(format!("{id}.json"));
        let original = root.join("checkpoint.saved");
        std::fs::rename(&file, &original).unwrap();
        std::fs::create_dir(&file).unwrap();
        let mut completed = info;
        completed.state = JobState::Succeeded;
        completed.completed = 1;
        assert!(registry.update(completed.clone()).await.is_err());
        let cached = registry
            .entries
            .lock()
            .await
            .get(&id)
            .unwrap()
            .saved
            .info
            .clone();
        assert_eq!(cached.state, JobState::Failed);
        assert_eq!(cached.error.unwrap().code, ErrorCode::Storage);
        std::fs::remove_dir(&file).unwrap();
        std::fs::rename(&original, &file).unwrap();
        let reopened = JobRegistry::open(root).unwrap();
        assert_eq!(
            reopened
                .entries
                .lock()
                .await
                .get(&id)
                .unwrap()
                .saved
                .info
                .state,
            JobState::Interrupted
        );
        registry.update(completed).await.unwrap();
        assert_eq!(
            registry
                .entries
                .lock()
                .await
                .get(&id)
                .unwrap()
                .saved
                .info
                .state,
            JobState::Succeeded
        );
    }

    #[tokio::test]
    async fn reopened_jobs_require_explicit_resume_after_crash() {
        let temp = tempfile::tempdir().unwrap();
        let registry = JobRegistry::open(temp.path().join("jobs")).unwrap();
        let id = Uuid::new_v4();
        registry
            .persist(Saved {
                request: JobRequest::Vault(VaultRequest::Sync),
                info: JobInfo {
                    job_id: id,
                    state: JobState::Running,
                    completed: 0,
                    total: 1,
                    result: None,
                    error: None,
                },
            })
            .await
            .unwrap();
        drop(registry);
        let registry = JobRegistry::open(temp.path().join("jobs")).unwrap();
        assert_eq!(
            registry
                .entries
                .lock()
                .await
                .get(&id)
                .unwrap()
                .saved
                .info
                .state,
            JobState::Interrupted
        );
        assert!(registry.tasks.lock().await.is_empty());
    }
}
