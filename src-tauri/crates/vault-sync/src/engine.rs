use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_db::{Binding, Command, CommandStatus, Database, DbError};
use vault_domain::{Action, Event, MessageId, State};
use vault_protocol::{Barrier, Record, encode_text};
use vault_snapshot::{Recovery, SnapshotError};
use vault_telegram::{RemoteMessage, Telegram, TelegramError, Update, scan_history};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum RuntimeMode {
    AuthRequired,
    Discovering,
    Syncing,
    OnlineReady,
    Reconnecting,
    DegradedReadonly,
}
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Telegram(#[from] TelegramError),
    #[error(transparent)]
    Database(#[from] DbError),
    #[error(transparent)]
    Snapshot(#[from] SnapshotError),
    #[error("shared mutations require ONLINE_READY")]
    Readonly,
    #[error("GC coordinator changed or a cleanup plan is still pending")]
    CoordinatorConflict,
    #[error("vault initialization is incomplete")]
    InitializationRequired,
    #[error("vault history failed integrity or compatibility checks")]
    Integrity,
    #[error("registered media source is unavailable")]
    MediaSourceMissing,
    #[error("history is still changing; another catch-up is required")]
    ReconcileRequired,
    #[error("operation result is unknown; reconcile before retry")]
    UnknownOutcome,
    #[error("database worker failed")]
    Worker,
}
impl SyncError {
    pub fn definitely_not_sent(&self) -> bool {
        match self {
            Self::Readonly => true,
            Self::Telegram(error) => error.definitely_not_sent(),
            _ => false,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuntimeStatus {
    pub mode: RuntimeMode,
    pub vault_id: Uuid,
    pub generation: u64,
    pub cursor: String,
    #[schemars(with = "Option<String>")]
    pub verified_commit_id: Option<MessageId>,
}

/// One mutable owner serializes jobs. CLI and Tauri both call this application
/// boundary; callers cannot manufacture a ready flag or directly promote mode.
pub struct SyncEngine<T: Telegram> {
    progress: broadcast::Sender<crate::OperationProgress>,
    pub(crate) transport: T,
    pub(crate) db: Arc<Mutex<Database>>,
    pub(crate) binding: Binding,
    pub(crate) device_id: String,
    updates: broadcast::Receiver<Update>,
    pub(crate) mode: RuntimeMode,
    verified: Option<Recovery>,
    // In-memory proof from a completed reconciliation, never restored from a
    // local checkpoint. Edits, deletions, disconnects and update loss revoke it.
    confirmed_head: Option<RemoteMessage>,
    full_reconcile_required: bool,
}
impl<T: Telegram> SyncEngine<T> {
    pub fn new(transport: T, db: Database, device_id: String) -> Self {
        Self::with_projection(transport, crate::ProjectionReader::new(db), device_id)
    }
    pub fn with_projection(
        transport: T,
        projection: crate::ProjectionReader,
        device_id: String,
    ) -> Self {
        let binding = projection.binding;
        let updates = transport.subscribe();
        let (progress, _) = broadcast::channel(256);
        Self {
            progress,
            transport,
            db: projection.db,
            binding,
            device_id,
            updates,
            mode: RuntimeMode::Syncing,
            verified: None,
            confirmed_head: None,
            full_reconcile_required: true,
        }
    }
    pub fn reader(&self) -> crate::ProjectionReader {
        crate::ProjectionReader {
            db: self.db.clone(),
            binding: self.binding,
        }
    }
    pub fn subscribe_progress(&self) -> broadcast::Receiver<crate::OperationProgress> {
        self.progress.subscribe()
    }
    pub(crate) fn report(
        &self,
        operation: crate::Operation,
        phase: crate::Phase,
        completed: usize,
        total: Option<usize>,
    ) {
        let _ = self.progress.send(crate::OperationProgress {
            operation,
            phase,
            completed,
            total,
        });
    }
    pub fn transport(&self) -> &T {
        &self.transport
    }
    pub fn transport_clone(&self) -> T
    where
        T: Clone,
    {
        self.transport.clone()
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Update> {
        self.transport.subscribe()
    }
    pub async fn reconcile_interrupted_commands(&self) -> Result<(), SyncError> {
        self.database(|db| db.mark_interrupted_commands_unknown())
            .await
    }
    pub async fn verify(&mut self, cancel: &CancellationToken) -> Result<Recovery, SyncError> {
        self.sync(cancel).await?;
        let message = self.latest_commit(cancel).await?;
        let result = self.recover_latest(&message, true, cancel).await;
        if let Err(error) = &result {
            self.fail(error);
        }
        result
    }
    pub(crate) async fn database<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Database) -> Result<R, DbError> + Send + 'static,
    ) -> Result<R, SyncError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || {
            let mut db = db.lock().map_err(|_| SyncError::Worker)?;
            f(&mut db).map_err(SyncError::Database)
        })
        .await
        .map_err(|_| SyncError::Worker)?
    }
    pub async fn state(&self) -> Result<State, SyncError> {
        self.database(|db| db.state()).await
    }
    pub async fn database_path(&self) -> Result<Option<std::path::PathBuf>, SyncError> {
        self.database(|db| Ok(db.path())).await
    }
    pub async fn command(&self, op: Uuid) -> Result<Option<Command>, SyncError> {
        self.database(move |db| db.command(op)).await
    }
    pub async fn local_setting(&self, key: String) -> Result<Option<String>, SyncError> {
        self.database(move |db| db.local_setting(&key)).await
    }
    pub async fn set_local_setting(&self, key: String, value: String) -> Result<(), SyncError> {
        self.database(move |db| db.set_local_setting(&key, &value))
            .await
    }
    pub async fn clear_local_setting(&self, key: String) -> Result<(), SyncError> {
        self.database(move |db| db.clear_local_setting(&key)).await
    }
    pub fn mode(&self) -> RuntimeMode {
        self.mode
    }
    pub fn binding(&self) -> Binding {
        self.binding
    }
    pub fn verified_snapshot(&self) -> Option<&Recovery> {
        self.verified.as_ref()
    }
    pub async fn status(&mut self) -> Result<RuntimeStatus, SyncError> {
        self.invalidate_from_updates();
        let state = self.state().await?;
        let cursor = self.database(|db| db.cursor()).await?;
        Ok(RuntimeStatus {
            mode: self.mode,
            vault_id: self.binding.vault_id,
            generation: state.generation,
            cursor: cursor.to_string(),
            verified_commit_id: self.verified.as_ref().map(|r| r.commit_id),
        })
    }
    fn invalidate_from_updates(&mut self) -> bool {
        self.invalidate_with_expected_deletions(&std::collections::BTreeSet::new())
    }
    fn invalidate_with_expected_deletions(
        &mut self,
        expected: &std::collections::BTreeSet<MessageId>,
    ) -> bool {
        let mut changed = false;
        loop {
            match self.updates.try_recv() {
                Ok(Update::AuthRequired) => {
                    self.full_reconcile_required = true;
                    self.mode = RuntimeMode::AuthRequired;
                    changed = true;
                }
                Ok(Update::Disconnected) => {
                    self.full_reconcile_required = true;
                    self.mode = RuntimeMode::Reconnecting;
                    changed = true;
                }
                Ok(Update::New(message)) if message.chat_id == self.binding.chat_id => {
                    changed = true;
                }
                Ok(Update::Deleted {
                    chat_id,
                    message_ids,
                }) if chat_id == self.binding.chat_id
                    && message_ids.iter().all(|id| expected.contains(id)) => {}
                Ok(Update::Changed { chat_id, .. } | Update::Deleted { chat_id, .. })
                    if chat_id == self.binding.chat_id =>
                {
                    self.full_reconcile_required = true;
                    changed = true;
                }
                Ok(_) => {}
                Err(broadcast::error::TryRecvError::Lagged(_)) => {
                    self.full_reconcile_required = true;
                    changed = true;
                    self.mode = RuntimeMode::Syncing;
                }
                Err(broadcast::error::TryRecvError::Closed) => {
                    self.full_reconcile_required = true;
                    self.mode = RuntimeMode::AuthRequired;
                    changed = true;
                    break;
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
            }
        }
        if changed && self.mode == RuntimeMode::OnlineReady {
            self.mode = RuntimeMode::Syncing;
        }
        changed
    }
    pub(crate) fn fail(&mut self, error: &SyncError) {
        self.full_reconcile_required = true;
        self.mode = match error {
            SyncError::Telegram(TelegramError::AuthRequired | TelegramError::Closed) => {
                RuntimeMode::AuthRequired
            }
            SyncError::Telegram(
                TelegramError::Timeout
                | TelegramError::RateLimited { .. }
                | TelegramError::Cancelled
                | TelegramError::UpdatesLost
                | TelegramError::Backpressure,
            ) => RuntimeMode::Reconnecting,
            SyncError::ReconcileRequired => RuntimeMode::Syncing,
            SyncError::InitializationRequired => RuntimeMode::Discovering,
            _ => RuntimeMode::DegradedReadonly,
        }
    }
    pub async fn sync(&mut self, cancel: &CancellationToken) -> Result<(), SyncError> {
        self.full_reconcile_required = true;
        self.mode = RuntimeMode::Syncing;
        let result = self.sync_inner(cancel).await;
        match &result {
            Ok(()) => {
                self.mode = RuntimeMode::OnlineReady;
                self.full_reconcile_required = false;
            }
            Err(error) => self.fail(error),
        }
        result
    }
    /// Replay only the suffix after an independently reconciled server anchor.
    /// A cold client or any loss of continuity goes through the full audit.
    /// There is no time-based freshness assumption and no persisted ready flag.
    pub async fn catch_up(&mut self, cancel: &CancellationToken) -> Result<(), SyncError> {
        self.catch_up_with_expected_deletions(cancel, &std::collections::BTreeSet::new())
            .await
    }
    pub(crate) async fn catch_up_with_expected_deletions(
        &mut self,
        cancel: &CancellationToken,
        expected: &std::collections::BTreeSet<MessageId>,
    ) -> Result<(), SyncError> {
        self.invalidate_with_expected_deletions(expected);
        if self.full_reconcile_required || self.confirmed_head.is_none() || self.verified.is_none()
        {
            return self.sync(cancel).await;
        }
        self.mode = RuntimeMode::Syncing;
        match self.catch_up_inner(cancel, expected).await {
            Ok(()) => {
                self.mode = RuntimeMode::OnlineReady;
                Ok(())
            }
            Err(SyncError::ReconcileRequired) => self.sync(cancel).await,
            Err(error) => {
                self.fail(&error);
                Err(error)
            }
        }
    }
    async fn catch_up_inner(
        &mut self,
        cancel: &CancellationToken,
        expected: &std::collections::BTreeSet<MessageId>,
    ) -> Result<(), SyncError> {
        let mut added_bytes = 0u64;
        let mut anchor = self
            .confirmed_head
            .clone()
            .ok_or(SyncError::ReconcileRequired)?;
        self.transport
            .verify_channel(self.binding.chat_id, self.binding.owner_id, cancel)
            .await?;
        for _ in 0..16 {
            // Include the anchor itself. A missing/edited head, including a
            // silently dropped delete update, cannot authorize a write.
            let messages = scan_history(
                &self.transport,
                self.binding.chat_id,
                anchor.id.get() - 1,
                None,
                1_000_000,
                cancel,
            )
            .await?;
            if messages.first() != Some(&anchor) {
                return Err(SyncError::ReconcileRequired);
            }
            // Detect a generation change before applying any part of this page.
            for message in messages.iter().skip(1) {
                if let Some(record) = message.record()? {
                    if record.vault_id() != self.binding.vault_id {
                        return Err(SyncError::Integrity);
                    }
                    if matches!(record, Record::Commit(_)) {
                        return Err(SyncError::ReconcileRequired);
                    }
                }
            }
            self.apply_messages(&messages[1..], cancel).await?;
            added_bytes = messages.iter().skip(1).fold(added_bytes, |total, message| {
                total.saturating_add(message.text.len() as u64)
            });
            let last = messages.last().ok_or(SyncError::Integrity)?.clone();
            let changed = self.invalidate_with_expected_deletions(expected);
            if self.full_reconcile_required {
                return Err(SyncError::ReconcileRequired);
            }
            let head = self
                .transport
                .history(self.binding.chat_id, None, 1, cancel)
                .await?;
            if head.first() == Some(&last) && !changed {
                self.transport
                    .verify_channel(self.binding.chat_id, self.binding.owner_id, cancel)
                    .await?;
                let changed = self.invalidate_with_expected_deletions(expected);
                if self.full_reconcile_required {
                    return Err(SyncError::ReconcileRequired);
                }
                if changed {
                    anchor = last;
                    continue;
                }
                self.confirmed_head = Some(last);
                if added_bytes > 0 {
                    let raw = self
                        .local_setting("snapshot.diagnostics.v1".into())
                        .await?
                        .ok_or(SyncError::Integrity)?;
                    let mut observation: serde_json::Value =
                        serde_json::from_str(&raw).map_err(|_| SyncError::Integrity)?;
                    let previous = observation["uncovered_log_bytes"]
                        .as_u64()
                        .ok_or(SyncError::Integrity)?;
                    observation["uncovered_log_bytes"] =
                        previous.saturating_add(added_bytes).into();
                    self.set_local_setting(
                        "snapshot.diagnostics.v1".into(),
                        observation.to_string(),
                    )
                    .await?;
                }
                return Ok(());
            }
            anchor = last;
        }
        Err(SyncError::ReconcileRequired)
    }
    pub(crate) async fn latest_commit(
        &self,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, SyncError> {
        let mut before = None;
        for _ in 0..10_000 {
            let mut page = self
                .transport
                .history(self.binding.chat_id, before, 100, cancel)
                .await?;
            if page.is_empty() {
                return Err(SyncError::InitializationRequired);
            }
            page.sort_by_key(|m| std::cmp::Reverse(m.id));
            for message in &page {
                if message.chat_id != self.binding.chat_id
                    || before.is_some_and(|last| message.id >= last)
                {
                    return Err(SyncError::Integrity);
                }
                if let Some(record) = message.record()? {
                    if record.vault_id() != self.binding.vault_id {
                        return Err(SyncError::Integrity);
                    }
                    if matches!(record, Record::Commit(_)) {
                        return Ok(message.clone());
                    }
                }
            }
            before = page.last().map(|m| m.id);
        }
        Err(SyncError::Integrity)
    }
    /// The backwards scan that locates the newest commit already contains most
    /// of its tail. Keep those pages, then finish only the gap to its barrier.
    async fn latest_commit_and_tail(
        &self,
        cancel: &CancellationToken,
    ) -> Result<(RemoteMessage, Vec<RemoteMessage>), SyncError> {
        let mut before = None;
        let mut found = None;
        let mut after = 0;
        let mut buffered = std::collections::BTreeMap::new();
        for _ in 0..10_000 {
            let mut page = self
                .transport
                .history(self.binding.chat_id, before, 100, cancel)
                .await?;
            if page.is_empty() {
                return found
                    .map(|commit| {
                        (
                            commit,
                            buffered
                                .into_values()
                                .filter(|message: &RemoteMessage| message.id.get() > after)
                                .collect(),
                        )
                    })
                    .ok_or(SyncError::InitializationRequired);
            }
            page.sort_by_key(|message| std::cmp::Reverse(message.id));
            let next = page
                .last()
                .map(|message| message.id)
                .ok_or(SyncError::Integrity)?;
            for message in page {
                if message.chat_id != self.binding.chat_id
                    || before.is_some_and(|last| message.id >= last)
                {
                    return Err(SyncError::Integrity);
                }
                if found.is_none()
                    && let Some(record) = message.record()?
                {
                    if record.vault_id() != self.binding.vault_id {
                        return Err(SyncError::Integrity);
                    }
                    if let Record::Commit(commit) = record {
                        after = commit.barrier_message_id.get();
                        found = Some(message.clone());
                    }
                }
                if let Some(old) = buffered.insert(message.id, message.clone())
                    && old != message
                {
                    return Err(SyncError::Integrity);
                }
            }
            if next.get() <= after
                && let Some(commit) = found
            {
                return Ok((
                    commit,
                    buffered
                        .into_values()
                        .filter(|message| message.id.get() > after)
                        .collect(),
                ));
            }
            before = Some(next);
        }
        Err(SyncError::Integrity)
    }
    async fn sync_inner(&mut self, cancel: &CancellationToken) -> Result<(), SyncError> {
        self.report(crate::Operation::Sync, crate::Phase::Verify, 0, None);
        self.invalidate_from_updates();
        // This attempt re-audits the state invalidated before it started. Any
        // subsequent loss of continuity invalidates this attempt as well.
        self.full_reconcile_required = false;
        self.transport
            .verify_channel(self.binding.chat_id, self.binding.owner_id, cancel)
            .await?;
        let (commit_message, tail) = self.latest_commit_and_tail(cancel).await?;
        let recovery = self.recover_latest(&commit_message, false, cancel).await?;
        if self.verified.as_ref().is_some_and(|old| {
            recovery.commit_id < old.commit_id
                || recovery.commit.generation < old.commit.generation
                || recovery.commit.barrier_message_id < old.commit.barrier_message_id
        }) {
            return Err(SyncError::Integrity);
        }
        let cursor = self.database(|db| db.cursor()).await?;
        // Independent local receipts reveal deletion/editing that Telegram alone
        // cannot prove. Audit before replacing a usable local projection.
        let local = self.state().await?;
        audit_receipts(&local, &recovery, &tail)?;
        self.check_reconcile_continuity()?;
        let tail_bytes: u64 = tail.iter().map(|message| message.text.len() as u64).sum();
        let observed = serde_json::json!({"commit_id": commit_message.id, "created_at_unix":commit_message.sent_at_unix, "uncovered_log_bytes":tail_bytes});
        if self.verified.as_ref().map(|r| r.commit_id) != Some(recovery.commit_id)
            || cursor < recovery.commit.barrier_message_id.get()
        {
            let snapshot = recovery.clone();
            self.database(move |db| {
                db.restore(
                    &snapshot.snapshot.state,
                    snapshot.commit.barrier_message_id,
                    snapshot.commit_id,
                    &snapshot.commit,
                )
            })
            .await?;
        }
        self.verified = Some(recovery);
        let mut audited_tail = Some(tail);
        for _ in 0..16 {
            let cursor = self.database(|db| db.cursor()).await?;
            let messages = if let Some(tail) = audited_tail.take() {
                tail.into_iter()
                    .filter(|message| message.id.get() > cursor)
                    .collect()
            } else {
                scan_history(
                    &self.transport,
                    self.binding.chat_id,
                    cursor,
                    None,
                    1_000_000,
                    cancel,
                )
                .await?
            };
            let last = messages.last().map(|m| m.id.get()).unwrap_or(cursor);
            self.apply_messages(&messages, cancel).await?;
            // Drain updates before the independent head read. A subsequent update
            // remains queued and will invalidate the next mutation/status request.
            let changed = self.check_reconcile_continuity()?;
            self.report(crate::Operation::Sync, crate::Phase::ReconcileHead, 0, None);
            let head_message = self
                .transport
                .history(self.binding.chat_id, None, 1, cancel)
                .await?
                .first()
                .cloned();
            let head = head_message.as_ref().map(|m| m.id.get()).unwrap_or(0);
            if head < last {
                return Err(SyncError::ReconcileRequired);
            }
            if head == last && !changed {
                self.transport
                    .verify_channel(self.binding.chat_id, self.binding.owner_id, cancel)
                    .await?;
                if self.check_reconcile_continuity()? {
                    continue;
                }
                self.set_local_setting("snapshot.diagnostics.v1".into(), observed.to_string())
                    .await?;
                self.confirmed_head = head_message;
                self.report(crate::Operation::Sync, crate::Phase::Complete, 1, Some(1));
                return Ok(());
            }
            let latest = self.latest_commit(cancel).await?;
            if self
                .verified
                .as_ref()
                .is_some_and(|r| latest.id != r.commit_id)
            {
                return Err(SyncError::ReconcileRequired);
            }
        }
        Err(SyncError::ReconcileRequired)
    }
    /// A delete/edit, auth transition or overrun requires a new receipt/snapshot
    /// audit. Fetching only newer messages cannot repair a stale history page.
    fn check_reconcile_continuity(&mut self) -> Result<bool, SyncError> {
        let changed = self.invalidate_from_updates();
        if self.full_reconcile_required {
            return Err(match self.mode {
                RuntimeMode::AuthRequired => TelegramError::AuthRequired.into(),
                RuntimeMode::Reconnecting => TelegramError::Timeout.into(),
                _ => SyncError::ReconcileRequired,
            });
        }
        Ok(changed)
    }
    async fn apply_messages(
        &self,
        messages: &[RemoteMessage],
        cancel: &CancellationToken,
    ) -> Result<(), SyncError> {
        const REPLAY_BATCH: usize = 1000;
        for (index, page) in messages.chunks(REPLAY_BATCH).enumerate() {
            if cancel.is_cancelled() {
                return Err(TelegramError::Cancelled.into());
            }
            let mut records = Vec::with_capacity(page.len());
            for message in page {
                let Some(record) = message.record()? else {
                    continue;
                };
                if record.vault_id() != self.binding.vault_id {
                    return Err(SyncError::Integrity);
                }
                if let Record::Commit(commit) = &record
                    && self.verified.as_ref().is_none_or(|r| {
                        message.id > r.commit_id || commit.generation > r.commit.generation
                    })
                {
                    return Err(SyncError::ReconcileRequired);
                }
                records.push((message.id, record));
            }
            if !records.is_empty() {
                self.database(move |db| db.apply_records(&records)).await?;
            }
            self.report(
                crate::Operation::Sync,
                crate::Phase::Replay,
                (index * REPLAY_BATCH + page.len()).min(messages.len()),
                Some(messages.len()),
            );
        }
        Ok(())
    }
    /// Explicit initialization continuation, allowed only for an INIT-only vault.
    /// This exception creates the empty S0 needed before ONLINE_READY is possible.
    pub async fn initialize(&mut self, cancel: &CancellationToken) -> Result<(), SyncError> {
        self.mode = RuntimeMode::Discovering;
        let result = self.initialize_inner(cancel).await;
        if let Err(error) = &result {
            self.fail(error);
        }
        result
    }
    async fn initialize_inner(&mut self, cancel: &CancellationToken) -> Result<(), SyncError> {
        self.transport
            .verify_channel(self.binding.chat_id, self.binding.owner_id, cancel)
            .await?;
        let history = scan_history(
            &self.transport,
            self.binding.chat_id,
            0,
            None,
            1_000_000,
            cancel,
        )
        .await?;
        let mut init = false;
        let mut committed = false;
        for message in &history {
            if let Some(record) = message.record()? {
                if record.vault_id() != self.binding.vault_id {
                    return Err(SyncError::Integrity);
                }
                match record {
                    Record::Init(marker) => {
                        if init || marker.owner_telegram_user_id != self.binding.owner_id {
                            return Err(SyncError::Integrity);
                        }
                        init = true;
                    }
                    Record::Commit(_) => committed = true,
                    Record::Event(_) | Record::Media(_) | Record::GcReport(_) => {
                        return Err(SyncError::Integrity);
                    }
                    Record::Barrier(_) | Record::Snapshot(_) => {}
                }
            }
        }
        if !init {
            return Err(SyncError::InitializationRequired);
        }
        if committed {
            return self.sync(cancel).await;
        }
        let snapshot_id = Uuid::new_v4();
        let record = Record::Barrier(Barrier {
            vault_id: self.binding.vault_id,
            snapshot_id,
            generation: 0,
        });
        let barrier = self
            .transport
            .send_text(
                self.binding.chat_id,
                encode_text(&record).map_err(|_| SyncError::Integrity)?,
                cancel,
            )
            .await?;
        // Re-read after barrier: another initializer may have committed meanwhile.
        let history = scan_history(
            &self.transport,
            self.binding.chat_id,
            0,
            None,
            1_000_000,
            cancel,
        )
        .await?;
        for message in &history {
            if let Some(Record::Commit(_)) = message.record()? {
                return self.sync(cancel).await;
            }
            if matches!(message.record()?, Some(Record::Event(_) | Record::Media(_))) {
                return Err(SyncError::Integrity);
            }
        }
        let encoded = vault_snapshot::build(
            State::new(self.binding.vault_id),
            self.binding.owner_id,
            snapshot_id,
            0,
            barrier.id,
        )?;
        vault_snapshot::publish(&self.transport, self.binding.chat_id, encoded, None, cancel)
            .await?;
        self.sync(cancel).await
    }
    pub async fn require_online(&mut self, cancel: &CancellationToken) -> Result<(), SyncError> {
        self.invalidate_from_updates();
        // A known disconnected/read-only client never queues an offline intent.
        if !matches!(self.mode, RuntimeMode::OnlineReady | RuntimeMode::Syncing) {
            return Err(SyncError::Readonly);
        }
        self.catch_up(cancel).await?;
        if self.mode != RuntimeMode::OnlineReady {
            return Err(SyncError::Readonly);
        }
        Ok(())
    }
    pub async fn execute(
        &mut self,
        op_id: Uuid,
        entity_id: Uuid,
        action: Action,
        cancel: &CancellationToken,
    ) -> Result<Command, SyncError> {
        self.require_online(cancel).await?;
        if self.command(op_id).await?.is_none()
            && let Action::VaultSettingsUpdate { settings } = &action
        {
            let current = self.reader().shared_settings().await?;
            if current.gc_device_id != settings.gc_device_id {
                self.check_coordinator_change(&current.gc_device_id, false)
                    .await?;
            }
        }
        self.submit_action(op_id, entity_id, action, cancel).await
    }
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Caller has reconciled the online head and validated any privileged role
    /// transition. The durable command remains the sole retry identity.
    pub(crate) async fn submit_action(
        &mut self,
        op_id: Uuid,
        entity_id: Uuid,
        action: Action,
        cancel: &CancellationToken,
    ) -> Result<Command, SyncError> {
        if let Some(existing) = self.command(op_id).await? {
            if existing.intent.entity_id != entity_id || existing.intent.action != action {
                return Err(DbError::CommandCollision.into());
            }
            return Ok(existing);
        }
        let state = self.state().await?;
        let event = Event {
            vault_id: self.binding.vault_id,
            op_id,
            device_id: self.device_id.clone(),
            base_generation: state.generation,
            entity_id,
            action,
        };
        if let Some(receipt) = state.applied.get(&op_id) {
            let original = self
                .transport
                .message(self.binding.chat_id, receipt.message_id, cancel)
                .await?
                .and_then(|message| message.record().transpose())
                .transpose()?;
            let Some(Record::Event(original)) = original else {
                return Err(DbError::CommandCollision.into());
            };
            if original.op_id != op_id
                || original.entity_id != entity_id
                || original.action != event.action
            {
                return Err(DbError::CommandCollision.into());
            }
            return self.database(move |db| db.recover_command(&original)).await;
        }
        let text = encode_text(&Record::Event(Box::new(event.clone())))
            .map_err(|_| SyncError::Integrity)?;
        let saved = event.clone();
        self.database(move |db| db.create_command(&saved)).await?;
        self.database(move |db| db.transition_command(op_id, CommandStatus::Sending, None))
            .await?;
        match self
            .transport
            .send_text(self.binding.chat_id, text, cancel)
            .await
        {
            Ok(message) => {
                let id = message.id;
                self.database(move |db| {
                    db.transition_command(op_id, CommandStatus::ServerAck, Some(id))
                })
                .await?;
                self.database(move |db| {
                    db.transition_command(op_id, CommandStatus::Reconciling, None)
                })
                .await?;
            }
            Err(error) => {
                let status = if error.definitely_not_sent() {
                    CommandStatus::TelegramError
                } else {
                    CommandStatus::UnknownOnTimeout
                };
                self.database(move |db| db.transition_command(op_id, status, None))
                    .await?;
                self.fail(&SyncError::Telegram(error));
                return self.command(op_id).await?.ok_or(SyncError::Integrity);
            }
        }
        // Reconciliation may fail after a server ACK; keep the durable intent.
        if self.catch_up(cancel).await.is_err() {
            return self.command(op_id).await?.ok_or(SyncError::Integrity);
        }
        self.command(op_id).await?.ok_or(SyncError::Integrity)
    }
    /// Media is uploaded only after the same centralized online check used for
    /// event mutations. The durable track registration is a separate later EVT.
    pub async fn upload_document(
        &mut self,
        path: std::path::PathBuf,
        caption: String,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, SyncError> {
        self.require_online(cancel).await?;
        self.transport
            .send_document(self.binding.chat_id, path, caption, cancel)
            .await
            .map_err(Into::into)
    }
    pub async fn upload_audio(
        &mut self,
        path: std::path::PathBuf,
        metadata: &vault_domain::Metadata,
        caption: String,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, SyncError> {
        self.require_online(cancel).await?;
        let audio = vault_telegram::AudioUpload {
            path,
            // Telegram display limits do not truncate the durable metadata.
            title: metadata.title.chars().take(64).collect(),
            performer: metadata.performer.chars().take(64).collect(),
            duration_seconds: u32::try_from(metadata.duration_ms / 1000)
                .map_err(|_| SyncError::Telegram(TelegramError::InvalidInput))?,
        };
        self.transport
            .send_audio(self.binding.chat_id, audio, caption, cancel)
            .await
            .map_err(Into::into)
    }
    pub async fn copy_media(
        &mut self,
        source_chat: vault_domain::ChatId,
        message_id: MessageId,
        caption: String,
        cancel: &CancellationToken,
    ) -> Result<RemoteMessage, SyncError> {
        self.require_online(cancel).await?;
        self.transport
            .copy_media(
                source_chat,
                message_id,
                self.binding.chat_id,
                caption,
                cancel,
            )
            .await
            .map_err(Into::into)
    }
    pub async fn media_file(
        &mut self,
        track_id: Uuid,
        source_id: Uuid,
        cancel: &CancellationToken,
    ) -> Result<(vault_telegram::RemoteFile, Option<String>, u64), SyncError> {
        self.require_online(cancel).await?;
        let state = self.state().await?;
        let source = state
            .tracks
            .get(&track_id)
            .and_then(|track| track.sources.get(&source_id))
            .filter(|source| source.chat_id == self.binding.chat_id)
            .ok_or(SyncError::Integrity)?;
        let file = self.probe_source(track_id, source, cancel).await?;
        let size = u64::try_from(file.size).map_err(|_| SyncError::Integrity)?;
        if source.size_bytes.is_some_and(|expected| expected != size) {
            return Err(SyncError::Integrity);
        }
        Ok((file, source.sha256.clone(), size))
    }
    /// Shares only a registered source in this vault, after online reconciliation.
    /// TDLib independently checks the message forwarding permission and recipient.
    pub async fn share_track(
        &mut self,
        track_id: Uuid,
        source_id: Uuid,
        recipient_chat: vault_domain::ChatId,
        cancel: &CancellationToken,
    ) -> Result<vault_telegram::RemoteMessage, SyncError> {
        self.require_online(cancel).await?;
        let state = self.state().await?;
        let track = state.tracks.get(&track_id).ok_or(SyncError::Integrity)?;
        let source = track
            .sources
            .get(&source_id)
            .filter(|source| source.chat_id == self.binding.chat_id)
            .ok_or(SyncError::Integrity)?;
        self.transport
            .forward_media(
                source.chat_id,
                source.message_id,
                self.binding.vault_id,
                recipient_chat,
                cancel,
            )
            .await
            .map_err(Into::into)
    }
    pub async fn request_snapshot(
        &mut self,
        cancel: &CancellationToken,
    ) -> Result<Recovery, SyncError> {
        self.require_online(cancel).await?;
        let state = self.state().await?;
        if state.settings.gc_device_id.as_deref() != Some(&self.device_id) {
            return Err(SyncError::Readonly);
        }
        self.mode = RuntimeMode::Syncing;
        let result = self.snapshot_inner(cancel).await;
        match &result {
            Ok(_) => {
                self.sync(cancel).await?;
            }
            Err(error) => self.fail(error),
        }
        result
    }
    async fn snapshot_inner(&mut self, cancel: &CancellationToken) -> Result<Recovery, SyncError> {
        self.report(crate::Operation::Snapshot, crate::Phase::Barrier, 0, None);
        let previous = self
            .verified
            .clone()
            .ok_or(SyncError::InitializationRequired)?;
        let snapshot_id = Uuid::new_v4();
        let generation = previous
            .commit
            .generation
            .checked_add(1)
            .ok_or(SyncError::Integrity)?;
        let barrier_record = Record::Barrier(Barrier {
            vault_id: self.binding.vault_id,
            snapshot_id,
            generation,
        });
        let barrier = self
            .transport
            .send_text(
                self.binding.chat_id,
                encode_text(&barrier_record).map_err(|_| SyncError::Integrity)?,
                cancel,
            )
            .await?;
        if barrier.record()? != Some(barrier_record) {
            return Err(SyncError::Integrity);
        }
        // Build from the verified prior snapshot, never from a projection that may
        // already include events newer than this barrier.
        let mut state = previous.snapshot.state;
        let messages = scan_history(
            &self.transport,
            self.binding.chat_id,
            previous.commit.barrier_message_id.get(),
            Some(barrier.id),
            1_000_000,
            cancel,
        )
        .await?;
        for message in messages {
            if let Some(record) = message.record()? {
                if record.vault_id() != state.vault_id {
                    return Err(SyncError::Integrity);
                }
                match record {
                    Record::Event(event) if message.id < barrier.id => {
                        state
                            .apply(message.id, &event)
                            .map_err(|_| SyncError::Integrity)?;
                    }
                    Record::Commit(commit)
                        if message.id != previous.commit_id
                            && commit.generation >= previous.commit.generation =>
                    {
                        return Err(SyncError::Integrity);
                    }
                    _ => {}
                }
            }
        }
        if state.settings.gc_device_id.as_deref() != Some(&self.device_id) {
            return Err(SyncError::Readonly);
        }
        let owner = self.binding.owner_id;
        self.report(crate::Operation::Snapshot, crate::Phase::Encode, 0, None);
        let barrier_id = barrier.id;
        let encoded = tokio::task::spawn_blocking(move || {
            vault_snapshot::build(state, owner, snapshot_id, generation, barrier_id)
        })
        .await
        .map_err(|_| SyncError::Worker)??;
        self.report(
            crate::Operation::Snapshot,
            crate::Phase::PublishCopies,
            0,
            Some(2),
        );
        let result = vault_snapshot::publish(
            &self.transport,
            self.binding.chat_id,
            encoded,
            Some(previous.commit_id),
            cancel,
        )
        .await?;
        self.report(
            crate::Operation::Snapshot,
            crate::Phase::Complete,
            2,
            Some(2),
        );
        Ok(result)
    }
}

fn audit_receipts(
    local: &State,
    recovery: &Recovery,
    tail: &[RemoteMessage],
) -> Result<(), SyncError> {
    let mut events = std::collections::BTreeMap::new();
    for message in tail {
        if let Some(record) = message.record()? {
            if record.vault_id() != local.vault_id {
                return Err(SyncError::Integrity);
            }
            if let Record::Event(event) = record {
                events.insert(message.id, event);
            }
        }
    }
    for (op, receipt) in &local.applied {
        if receipt.message_id < recovery.commit.barrier_message_id {
            if recovery.snapshot.state.applied.get(op) != Some(receipt) {
                return Err(SyncError::Integrity);
            }
        } else {
            let event = events
                .get(&receipt.message_id)
                .ok_or(SyncError::Integrity)?;
            if event.op_id != *op
                || vault_domain::event_digest(event).map_err(|_| SyncError::Integrity)?
                    != receipt.event_sha256
            {
                return Err(SyncError::Integrity);
            }
        }
    }
    Ok(())
}
