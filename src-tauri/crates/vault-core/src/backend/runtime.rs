//! Process-owned runtime recovery and safe UI status. No WebView owns a session.
use super::*;

pub(super) struct OperationCancellation {
    pub(super) token: CancellationToken,
    forwarding: tokio::task::JoinHandle<()>,
}
impl Drop for OperationCancellation {
    fn drop(&mut self) {
        self.token.cancel();
        self.forwarding.abort();
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct CreationInfo {
    pub vault_id: Uuid,
    pub chat_id: Option<vault_domain::ChatId>,
    pub stage: String,
}
#[derive(Debug, Clone, Serialize)]
pub struct SessionInfo {
    pub context: Context,
    pub event_sequence: String,
    pub chat_id: Option<vault_domain::ChatId>,
    pub api_configured: bool,
    pub telegram_connection: String,
    pub service_running: bool,
    pub repair_required: bool,
    pub playback_volume: f32,
    pub creation: Option<CreationInfo>,
    pub diagnostics: Vec<crate::DiagnosticEvent>,
}
impl Backend {
    pub(super) async fn operation_cancellation(&self, session_id: Uuid, cancel: &CancellationToken) -> Result<OperationCancellation, ApiError> {
        let token = self.job_registry(session_id).await?.cancellation().child_token();
        let lifetime = token.clone();
        let caller = cancel.clone();
        let forwarding = tokio::spawn(async move {
            tokio::select! {
                _ = caller.cancelled() => lifetime.cancel(),
                _ = lifetime.cancelled() => {},
            }
        });
        Ok(OperationCancellation { token, forwarding })
    }
    /// Short-lived clients can await the first real sync result before closing.
    /// The long-lived GUI uses events and can read SQLite during this wait.
    pub async fn settled_status(&self, session_id: Uuid, cancel: &CancellationToken) -> Result<Status, ApiError> {
        match self.ensure_vault_service(session_id, cancel).await {
            Ok(()) => {}
            Err(error) if matches!(error.code, ErrorCode::NoVault | ErrorCode::AuthRequired | ErrorCode::DatabaseCorrupt) => return self.runtime_status().await,
            Err(error) => return Err(error),
        }
        let service = {
            let guard = self.session.lock().await;
            let s = guard.as_ref().ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            s.service.clone().ok_or_else(|| ApiError::new(ErrorCode::NoVault))?
        };
        let mut changes = service.status_changes();
        let wait = async {
            loop {
                let status = changes.borrow_and_update().clone();
                if status.vault.is_some() || service.is_closed() { return Ok::<_, ApiError>(service.status()); }
                changes.changed().await.map_err(|_| ApiError::new(ErrorCode::Closed))?;
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(ApiError::new(ErrorCode::Cancelled)),
            result = tokio::time::timeout(std::time::Duration::from_secs(60), wait) =>
                result.map_err(|_| ApiError::diagnostic(ErrorCode::Timeout, "initial vault reconciliation is still running; use the persistent application or retry sync"))?,
        }
    }
    pub async fn session_info(&self, session_id: Uuid) -> Result<SessionInfo, ApiError> {
        let guard = self.session.lock().await;
        let s = guard.as_ref().ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        let connection = s.native.as_ref().map(|n| n.client.connection_state());
        let connection = connection.as_ref().and_then(|state| state["@type"].as_str()).unwrap_or("not_started");
        Ok(SessionInfo {
            context: s.context.clone(),
            event_sequence: self.events.sequence()?,
            chat_id: s.config.binding.map(|binding| binding.chat_id),
            api_configured: s.config.api_id.is_some(),
            telegram_connection: connection.to_owned(),
            service_running: s.service.as_ref().is_some_and(|service| !service.is_closed()),
            repair_required: s.repair_required,
            playback_volume: s.playback_volume,
            creation: s.config.creation.as_ref().map(|intent| CreationInfo {
                vault_id: intent.vault_id,
                chat_id: intent.chat_id,
                stage: match intent.stage {
                    CreateStage::CreatingUnknown => "creating_unknown",
                    CreateStage::ChannelCreated => "channel_created",
                    CreateStage::InitUnknown => "init_unknown",
                    CreateStage::Initialized => "initialized",
                }.into(),
            }),
            diagnostics: self.events.diagnostics(session_id),
        })
    }
    pub(super) fn creation_progress(&self, context: &Context, phase: vault_sync::Phase) {
        self.events.emit(context, EventData::Operation(vault_sync::OperationProgress {
            operation: vault_sync::Operation::Create, phase, completed: 0, total: None,
        }));
    }
    /// Wait for TDLib's actual auth transition, not for a speculative ready flag.
    /// Local-only reads never call this method and remain available offline.
    pub(super) async fn ensure_authorized(&self, session_id: Uuid, cancel: &CancellationToken) -> Result<(), ApiError> {
        let mut updates = self.auth_updates(session_id).await?;
        let wait = async {
            loop {
                match updates.current().status {
                    AuthStatus::Ready => return self.activate_account(session_id, cancel).await,
                    AuthStatus::ParametersRequired => updates.changed(cancel).await?,
                    _ => return Err(ApiError::new(ErrorCode::AuthRequired)),
                }
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => Err(ApiError::new(ErrorCode::Cancelled)),
            result = tokio::time::timeout(std::time::Duration::from_secs(30), wait) =>
                result.map_err(|_| ApiError::diagnostic(ErrorCode::Timeout, "Telegram session is still starting; retry after authorization is ready"))?,
        }
    }
    pub(super) async fn ensure_vault_service(&self, session_id: Uuid, cancel: &CancellationToken) -> Result<(), ApiError> {
        {
            let mut guard = self.session.lock().await;
            let s = guard.as_mut().ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            if s.config.binding.is_none() {
                return Err(ApiError::diagnostic(ErrorCode::NoVault, "no vault binding is saved; discover or attach an existing channel"));
            }
            if s.repair_required { return Err(ApiError::new(ErrorCode::DatabaseCorrupt)); }
            if s.service.as_ref().is_some_and(|service| !service.is_closed()) { return Ok(()); }
            s.service = None;
        }
        self.ensure_authorized(session_id, cancel).await?;
        let mut guard = self.session.lock().await;
        let s = guard.as_mut().ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
        check_session(s, session_id)?;
        if s.reader.is_none() {
            let binding = s.config.binding.ok_or_else(|| ApiError::new(ErrorCode::NoVault))?;
            s.reader = Some(ProjectionReader::new(Database::open(s.lease.database_path(), binding)?));
        }
        self.start_service(s, s.context.clone());
        Ok(())
    }
    /// Reconnect using the OS-protected credentials without exposing them to UI.
    pub async fn reconnect(&self, session_id: Uuid) -> Result<AuthStatus, ApiError> {
        let (id, profile) = {
            let guard = self.session.lock().await;
            let s = guard.as_ref().ok_or_else(|| ApiError::new(ErrorCode::NoProfile))?;
            check_session(s, session_id)?;
            (s.config.api_id.ok_or_else(|| ApiError::new(ErrorCode::AuthRequired))?, s.context.profile_id)
        };
        let secrets = self.secrets.clone();
        let hash = tokio::task::spawn_blocking(move || secrets.get(&format!("{profile}:api-hash")))
            .await.map_err(|_| ApiError::new(ErrorCode::Internal))??
            .ok_or_else(|| ApiError::new(ErrorCode::SecureStorageUnavailable))?;
        self.connect_native(session_id, id, Zeroizing::new(String::from_utf8_lossy(&hash).into_owned())).await
    }
    /// Render only the current QR credential with the existing QR encoder.
    pub async fn auth_qr(&self, session_id: Uuid) -> Result<Option<String>, ApiError> {
        let AuthStatus::QrConfirmation { link } = self.auth_status(session_id).await? else { return Ok(None); };
        let code = qrcode::QrCode::new(link.as_bytes()).map_err(|_| ApiError::new(ErrorCode::InvalidInput))?;
        let size = code.width();
        let mut svg = format!("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\"><path fill=\"white\" d=\"M0 0h{}v{}H0z\"/><g fill=\"black\">", size + 8, size + 8, size + 8, size + 8);
        for y in 0..size {
            for x in 0..size {
                if code[(x, y)] == qrcode::Color::Dark {
                    use std::fmt::Write;
                    let _ = write!(svg, "<path d=\"M{} {}h1v1h-1z\"/>", x + 4, y + 4);
                }
            }
        }
        svg.push_str("</g></svg>");
        Ok(Some(svg))
    }
}
