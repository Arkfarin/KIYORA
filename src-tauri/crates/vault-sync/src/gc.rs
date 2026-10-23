use crate::*;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_snapshot::gc::{GcPlan, GcProgress, GcStatus};
use vault_telegram::{Telegram, scan_history};

impl<T: Telegram> SyncEngine<T> {
    pub(crate) async fn check_coordinator_change(
        &self,
        previous: &Option<String>,
        force_confirmed: bool,
    ) -> Result<(), SyncError> {
        if !force_confirmed
            && (previous.as_deref().is_some_and(|id| id != self.device_id)
                || self.database(|db| db.has_pending_gc()).await?)
        {
            return Err(SyncError::CoordinatorConflict);
        }
        Ok(())
    }

    /// This is an application role, not a Telegram lock. A forced takeover
    /// requires explicit confirmation and cannot make a concurrently running
    /// old client stop a delete request that Telegram has already accepted.
    pub async fn assign_gc_coordinator(
        &mut self,
        op_id: Uuid,
        target: Option<String>,
        expected_previous: Option<String>,
        force_confirmed: bool,
        cancel: &CancellationToken,
    ) -> Result<vault_db::Command, SyncError> {
        self.require_online(cancel).await?;
        if let Some(existing) = self.command(op_id).await? {
            if existing.intent.entity_id != self.binding.vault_id
                || !matches!(&existing.intent.action, vault_domain::Action::VaultSettingsUpdate { settings } if settings.gc_device_id == target)
            {
                return Err(vault_db::DbError::CommandCollision.into());
            }
            return Ok(existing);
        }
        if target
            .as_ref()
            .is_some_and(|id| id.is_empty() || id.len() > 256 || id.chars().any(char::is_control))
        {
            return Err(SyncError::CoordinatorConflict);
        }
        let mut settings = self.reader().shared_settings().await?;
        if settings.gc_device_id != expected_previous {
            return Err(SyncError::CoordinatorConflict);
        }
        if settings.gc_device_id != target {
            self.check_coordinator_change(&settings.gc_device_id, force_confirmed)
                .await?;
            if force_confirmed {
                // Discard local stale plans before publishing the takeover. A
                // new coordinator must build a fresh verified dry-run plan.
                self.database(|db| db.abandon_gc_plans()).await?;
            }
        }
        settings.gc_device_id = target;
        self.submit_action(
            op_id,
            self.binding.vault_id,
            vault_domain::Action::VaultSettingsUpdate { settings },
            cancel,
        )
        .await
    }

    pub async fn abandon_gc(&self, id: Uuid) -> Result<GcProgress, SyncError> {
        let mut progress = self.inspect_gc(id).await?.ok_or(SyncError::Integrity)?;
        if progress.status != GcStatus::Complete {
            progress.status = GcStatus::Abandoned;
            self.persist_gc(&progress).await?;
        }
        Ok(progress)
    }
    /// Builds and persists a reviewable dry-run allowlist. Does not delete anything.
    pub async fn plan_gc(&mut self, cancel: &CancellationToken) -> Result<GcProgress, SyncError> {
        self.require_online(cancel).await?;
        let commit = self.latest_commit(cancel).await?;
        let recovery = vault_snapshot::recover(
            &self.transport,
            self.binding.chat_id,
            &commit,
            self.binding.owner_id,
            true,
            cancel,
        )
        .await?;
        let messages = scan_history(
            &self.transport,
            self.binding.chat_id,
            0,
            None,
            1_000_000,
            cancel,
        )
        .await?;
        let state = self.state().await?;
        let plan = vault_snapshot::gc::plan(&messages, &recovery, &state, &self.device_id)?;
        let progress = GcProgress {
            plan,
            completed: Default::default(),
            status: GcStatus::Planned,
        };
        self.persist_gc(&progress).await?;
        Ok(progress)
    }
    async fn persist_gc(&self, progress: &GcProgress) -> Result<(), SyncError> {
        let plan = serde_json::to_string(&progress.plan).map_err(|_| SyncError::Integrity)?;
        let completed =
            serde_json::to_string(&progress.completed).map_err(|_| SyncError::Integrity)?;
        let status = serde_json::to_string(&progress.status).map_err(|_| SyncError::Integrity)?;
        let id = progress.plan.plan_id;
        self.database(move |db| db.save_gc_plan(id, &plan, &completed, &status))
            .await
    }
    pub async fn inspect_gc(&self, id: Uuid) -> Result<Option<GcProgress>, SyncError> {
        let row = self.database(move |db| db.gc_plan(id)).await?;
        row.map(|(plan, completed, status)| {
            Ok(GcProgress {
                plan: serde_json::from_str(&plan).map_err(|_| SyncError::Integrity)?,
                completed: serde_json::from_str(&completed).map_err(|_| SyncError::Integrity)?,
                status: serde_json::from_str(&status).map_err(|_| SyncError::Integrity)?,
            })
        })
        .transpose()
    }
    /// Confirmation binds to the exact displayed plan digest. Already deleted IDs
    /// are checked as absent and recorded after crash; unknown records are retained.
    pub async fn execute_gc(
        &mut self,
        id: Uuid,
        confirmed_hash: &str,
        cancel: &CancellationToken,
    ) -> Result<GcProgress, SyncError> {
        self.require_online(cancel).await?;
        let mut progress = self.inspect_gc(id).await?.ok_or(SyncError::Integrity)?;
        if progress.status == GcStatus::Abandoned
            || progress.plan.hash()? != confirmed_hash
            || progress.plan.vault_id != self.binding.vault_id
            || progress.plan.coordinator != self.device_id
        {
            return Err(SyncError::Readonly);
        }
        if progress.status == GcStatus::Complete {
            return Ok(progress);
        }
        let result = self.execute_gc_inner(&mut progress, cancel).await;
        if let Err(error) = &result {
            progress.status = GcStatus::Stopped;
            self.persist_gc(&progress).await?;
            self.fail(error);
        }
        result?;
        Ok(progress)
    }
    async fn validate_gc_head(
        &mut self,
        plan: &GcPlan,
        cancel: &CancellationToken,
    ) -> Result<(), SyncError> {
        self.sync(cancel).await?;
        let state = self.state().await?;
        if state.settings.gc_device_id.as_deref() != Some(&self.device_id) {
            return Err(SyncError::Readonly);
        }
        let head = self.latest_commit(cancel).await?;
        if head.id != plan.commit_id {
            return Err(SyncError::Integrity);
        }
        let verified = vault_snapshot::recover(
            &self.transport,
            self.binding.chat_id,
            &head,
            self.binding.owner_id,
            true,
            cancel,
        )
        .await?;
        if !verified.primary_healthy
            || !verified.backup_healthy
            || verified.commit.barrier_message_id != plan.barrier_id
            || verified.commit.generation != plan.generation
        {
            return Err(SyncError::Integrity);
        }
        // Recompute coverage and dependencies. Missing candidates are acceptable
        // only when resuming an already persisted plan under this same verified cut.
        let all = scan_history(
            &self.transport,
            self.binding.chat_id,
            0,
            None,
            1_000_000,
            cancel,
        )
        .await?;
        // Even if the role was transferred away and later back, a plan from
        // the previous tenure cannot resume. A newer commit also invalidates
        // it above, including when old settings events have been pruned.
        for message in &all {
            if message.id > plan.commit_id
                && let Some(vault_protocol::Record::Event(event)) = message.record()?
                && let vault_domain::Action::VaultSettingsUpdate { settings } = event.action
                && settings.gc_device_id.as_deref() != Some(&self.device_id)
            {
                return Err(SyncError::CoordinatorConflict);
            }
        }
        let fresh = vault_snapshot::gc::plan(&all, &verified, &state, &self.device_id)?;
        for candidate in &plan.candidates {
            if let Some(message) = all.iter().find(|m| m.id == candidate.message_id) {
                if !vault_snapshot::gc::matches(candidate, message)? {
                    return Err(SyncError::Integrity);
                }
                if matches!(candidate.kind.as_str(), "EVT" | "BARRIER")
                    && !fresh.candidates.iter().any(|c| {
                        c.message_id == candidate.message_id
                            && c.record_sha256 == candidate.record_sha256
                    })
                {
                    return Err(SyncError::Integrity);
                }
                // An old group may be partially deleted after a crash. Its initial
                // proof is preserved by the digest-bound local plan, while current
                // copy IDs and any newly referring commits are checked independently.
                if matches!(candidate.kind.as_str(), "SNAPSHOT" | "SNAPSHOT_COMMIT") {
                    if candidate.message_id >= plan.barrier_id
                        || [
                            verified.commit.snapshot_a_message_id,
                            verified.commit.snapshot_b_message_id,
                            verified.commit_id,
                        ]
                        .contains(&candidate.message_id)
                    {
                        return Err(SyncError::Integrity);
                    }
                    for message in &all {
                        if let Some(vault_protocol::Record::Commit(commit)) = message.record()?
                            && [commit.snapshot_a_message_id, commit.snapshot_b_message_id]
                                .contains(&candidate.message_id)
                            && !plan.candidates.iter().any(|c| c.message_id == message.id)
                        {
                            return Err(SyncError::Integrity);
                        }
                    }
                }
            }
        }
        Ok(())
    }
    async fn execute_gc_inner(
        &mut self,
        progress: &mut GcProgress,
        cancel: &CancellationToken,
    ) -> Result<(), SyncError> {
        progress.status = GcStatus::Running;
        self.persist_gc(progress).await?;
        let pending: Vec<_> = progress
            .plan
            .candidates
            .iter()
            .filter(|c| !progress.completed.contains(&c.message_id))
            .cloned()
            .collect();
        for batch in pending.chunks(32) {
            self.validate_gc_head(&progress.plan, cancel).await?;
            let mut present = Vec::new();
            for candidate in batch {
                if let Some(message) = self
                    .transport
                    .message(self.binding.chat_id, candidate.message_id, cancel)
                    .await?
                {
                    if !vault_snapshot::gc::matches(candidate, &message)? {
                        return Err(SyncError::Integrity);
                    }
                    present.push(candidate.message_id);
                }
            }
            if !present.is_empty() {
                self.transport
                    .delete_messages(self.binding.chat_id, present, cancel)
                    .await?;
            }
            for candidate in batch {
                if self
                    .transport
                    .message(self.binding.chat_id, candidate.message_id, cancel)
                    .await?
                    .is_some()
                {
                    return Err(SyncError::Integrity);
                }
                progress.completed.insert(candidate.message_id);
            }
            self.persist_gc(progress).await?;
            self.report(
                crate::Operation::Gc,
                crate::Phase::DeleteBatch,
                progress.completed.len(),
                Some(progress.plan.candidates.len()),
            );
        }
        self.validate_gc_head(&progress.plan, cancel).await?;
        progress.status = GcStatus::Complete;
        self.persist_gc(progress).await?;
        self.report(
            crate::Operation::Gc,
            crate::Phase::Complete,
            progress.completed.len(),
            Some(progress.plan.candidates.len()),
        );
        Ok(())
    }
}
