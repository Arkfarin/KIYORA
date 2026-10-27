use crate::{
    TelegramError, check_response,
    native::{Native, ReceiverPermit},
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_domain::ChatId;

/// Passive ID cache, populated before request replies. It never loads chats.
/// This also retains previously delivered pages for later deep discoveries.
#[derive(Default)]
struct KnownChats {
    ids: BTreeSet<ChatId>,
    invalid: bool,
}
impl KnownChats {
    fn observe(&mut self, update: &Value) {
        let id = match update["@type"].as_str() {
            Some("updateNewChat") => &update["chat"]["id"],
            Some("updateChatPosition") => &update["chat_id"],
            _ => return,
        };
        match crate::integer(id)
            .and_then(|id| ChatId::new(id).map_err(|_| TelegramError::InvalidResponse))
        {
            Ok(id) => {
                self.ids.insert(id);
            }
            Err(_) => self.invalid = true,
        }
    }
}

const CAPACITY: usize = 128;
type Reply = oneshot::Sender<Result<Value, TelegramError>>;
struct Request {
    value: Value,
    deadline: Option<Instant>,
    reply: Reply,
}
struct Pending {
    deadline: Option<Instant>,
    reply: Reply,
    authorization: bool,
}

/// Cloneable request handle; the sole receive worker closes after the last handle
/// is dropped. Raw updates are internal backend data, never emitted directly to UI.
#[derive(Clone)]
pub struct TdClient {
    // Discovery reports rate limits immediately and never hides them in retries.
    retry_reads: bool,
    request_counter: Option<Arc<std::sync::atomic::AtomicU64>>,
    known_chats: Arc<Mutex<KnownChats>>,
    pub(crate) auth_attempts: Arc<tokio::sync::Mutex<crate::auth::AuthAttempts>>,
    requests: mpsc::Sender<Request>,
    updates: broadcast::Sender<Value>,
    auth: watch::Receiver<Value>,
    connection: watch::Receiver<Value>,
    closed: watch::Receiver<bool>,
    shutdown: CancellationToken,
}

impl TdClient {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self, TelegramError> {
        let permit = ReceiverPermit::acquire()?;
        let path = path.as_ref().to_path_buf();
        let (requests, rx) = mpsc::channel(CAPACITY);
        let (updates, _) = broadcast::channel(1024);
        let (auth_tx, auth) =
            watch::channel(json!({"@type":"authorizationStateWaitTdlibParameters"}));
        let (conn_tx, connection) =
            watch::channel(json!({"@type":"connectionStateWaitingForNetwork"}));
        let (closed_tx, closed) = watch::channel(false);
        let (started_tx, started_rx) = oneshot::channel();
        let shutdown = CancellationToken::new();
        let known_chats = Arc::new(Mutex::new(KnownChats::default()));
        let worker = Worker {
            known_chats: known_chats.clone(),
            rx,
            updates: updates.clone(),
            auth: auth_tx,
            connection: conn_tx,
            closed: closed_tx,
            shutdown: shutdown.clone(),
            pending: BTreeMap::new(),
            pending_auth: None,
        };
        std::thread::Builder::new()
            .name("kiyora-tdjson".into())
            .spawn(move || {
                let closed = worker.closed.clone();
                match Native::load(&path)
                    .and_then(|native| native.initialize().map(|id| (native, id)))
                {
                    Ok((native, id)) => {
                        let _ = started_tx.send(Ok(()));
                        worker.run(native, id);
                    }
                    Err(error) => {
                        let _ = started_tx.send(Err(error));
                    }
                }
                drop(permit);
                closed.send_replace(true);
            })
            .map_err(|_| TelegramError::NativeLibrary)?;
        started_rx.await.map_err(|_| TelegramError::Closed)??;
        Ok(Self {
            retry_reads: true,
            request_counter: None,
            known_chats,
            auth_attempts: Arc::default(),
            requests,
            updates,
            auth,
            connection,
            closed,
            shutdown,
        })
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.updates.subscribe()
    }
    pub(crate) fn known_chat_ids(&self, limit: usize) -> Result<Vec<ChatId>, TelegramError> {
        let known = self.known_chats.lock().map_err(|_| TelegramError::Closed)?;
        if known.invalid {
            return Err(TelegramError::InvalidResponse);
        }
        Ok(known.ids.iter().take(limit).copied().collect())
    }
    pub(crate) fn without_read_retries(&self) -> Self {
        Self {
            retry_reads: false,
            request_counter: Some(Arc::default()),
            ..self.clone()
        }
    }
    pub(crate) fn request_count(&self) -> u64 {
        self.request_counter.as_ref().map_or(0, |counter| {
            counter.load(std::sync::atomic::Ordering::Relaxed)
        })
    }
    pub fn authorization(&self) -> Value {
        self.auth.borrow().clone()
    }
    pub fn authorization_changes(&self) -> watch::Receiver<Value> {
        self.auth.clone()
    }
    pub fn auth_updates(&self) -> crate::AuthUpdates {
        crate::AuthUpdates {
            auth: self.auth.clone(),
            connection: self.connection.clone(),
        }
    }
    pub fn connection_state(&self) -> Value {
        self.connection.borrow().clone()
    }
    pub(crate) fn connection_changes(&self) -> watch::Receiver<Value> {
        self.connection.clone()
    }
    pub async fn request(
        &self,
        value: Value,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value, TelegramError> {
        if cancel.is_cancelled() {
            return Err(TelegramError::Cancelled);
        }
        if self.shutdown.is_cancelled() || *self.closed.borrow() {
            return Err(TelegramError::Closed);
        }
        if !value.is_object() || !value["@type"].is_string() || value.get("@extra").is_some() {
            return Err(TelegramError::InvalidInput);
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(TelegramError::InvalidInput)?;
        let (reply, rx) = oneshot::channel();
        self.requests
            .try_send(Request {
                value,
                deadline: Some(deadline),
                reply,
            })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => TelegramError::Backpressure,
                mpsc::error::TrySendError::Closed(_) => TelegramError::Closed,
            })?;
        if let Some(counter) = &self.request_counter {
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        tokio::select! {
            biased;
            _=cancel.cancelled()=>Err(TelegramError::Cancelled),
            result=tokio::time::timeout(timeout,rx)=>result.map_err(|_|TelegramError::Timeout)?.map_err(|_|TelegramError::Closed)?,
        }
    }
    pub async fn call(&self, value: Value) -> Result<Value, TelegramError> {
        self.request(value, Duration::from_secs(30), &CancellationToken::new())
            .await
    }
    /// Internal maintenance requests must hold their storage gate until the
    /// native response (or client close), even if the API caller goes away.
    pub(crate) async fn maintenance(&self, value: Value) -> Result<Value, TelegramError> {
        if self.shutdown.is_cancelled() || *self.closed.borrow() {
            return Err(TelegramError::Closed);
        }
        if value["@type"] != "optimizeStorage" || value.get("@extra").is_some() {
            return Err(TelegramError::InvalidInput);
        }
        let (reply, rx) = oneshot::channel();
        self.requests
            .try_send(Request {
                value,
                deadline: None,
                reply,
            })
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => TelegramError::Backpressure,
                mpsc::error::TrySendError::Closed(_) => TelegramError::Closed,
            })?;
        rx.await.map_err(|_| TelegramError::Closed)?
    }
    /// Only use for idempotent reads. Writes are never automatically retried.
    pub async fn read(
        &self,
        value: Value,
        cancel: &CancellationToken,
    ) -> Result<Value, TelegramError> {
        if !matches!(
            value["@type"].as_str(),
            Some(
                "getMe"
                    | "getAuthorizationState"
                    | "getChat"
                    | "getMessage"
                    | "getChatPinnedMessage"
                    | "getChatHistory"
                    | "searchChatMessages"
                    | "getSupergroup"
                    | "getSupergroupFullInfo"
                    | "getChatMember"
                    | "getChats"
                    | "searchChats"
                    | "searchChatsOnServer"
                    | "getOption"
                    | "getMessageProperties"
            )
        ) {
            return Err(TelegramError::InvalidInput);
        }
        for attempt in 0..3 {
            let result = self
                .request(value.clone(), Duration::from_secs(30), cancel)
                .await;
            let wait = match &result {
                Err(TelegramError::RateLimited {
                    retry_after_seconds,
                }) => Some(Duration::from_secs(*retry_after_seconds)),
                Err(TelegramError::Timeout) => Some(Duration::from_secs(1 << attempt)),
                _ => None,
            };
            if !self.retry_reads || attempt == 2 || wait.is_none() {
                return result;
            }
            if let Some(wait) = wait {
                tokio::select! {_=cancel.cancelled()=>return Err(TelegramError::Cancelled),_=tokio::time::sleep(wait)=>{}}
            }
        }
        Err(TelegramError::Timeout)
    }
    pub async fn verify_version(&self) -> Result<(), TelegramError> {
        let version = self
            .call(json!({"@type":"getOption","name":"version"}))
            .await?;
        if version["value"] != crate::TDLIB_VERSION {
            return Err(TelegramError::UnsupportedVersion);
        }
        Ok(())
    }
    pub async fn close(&self) -> Result<(), TelegramError> {
        self.shutdown.cancel();
        let mut closed = self.closed.clone();
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                if *closed.borrow() {
                    return Ok(());
                }
                closed.changed().await.map_err(|_| TelegramError::Closed)?;
            }
        })
        .await
        .map_err(|_| TelegramError::Timeout)?
    }
}

struct Worker {
    known_chats: Arc<Mutex<KnownChats>>,
    rx: mpsc::Receiver<Request>,
    updates: broadcast::Sender<Value>,
    auth: watch::Sender<Value>,
    connection: watch::Sender<Value>,
    closed: watch::Sender<bool>,
    shutdown: CancellationToken,
    pending: BTreeMap<String, Pending>,
    // Kept until the native reply, even when the caller cancels or times out.
    // Dropping a Rust future does not cancel Telegram's pending query.
    pending_auth: Option<String>,
}
impl Worker {
    fn run(mut self, native: Native, id: i32) {
        let mut closing = false;
        // Starts update delivery even before any caller request.
        if native
            .send(id, &json!({"@type":"getAuthorizationState"}))
            .is_err()
        {
            self.shutdown.cancel();
        }
        loop {
            if !closing && (self.shutdown.is_cancelled() || self.rx.is_closed()) {
                closing = true;
                self.rx.close();
                let _ = native.send(id, &json!({"@type":"close"}));
                for (_, pending) in std::mem::take(&mut self.pending) {
                    let _ = pending.reply.send(Err(TelegramError::Closed));
                }
            }
            if !closing {
                for _ in 0..32 {
                    let Ok(mut request) = self.rx.try_recv() else {
                        break;
                    };
                    if request.reply.is_closed() {
                        continue;
                    }
                    if request
                        .deadline
                        .is_some_and(|deadline| deadline <= Instant::now())
                    {
                        let _ = request.reply.send(Err(TelegramError::Timeout));
                        continue;
                    }
                    if self.pending.len() >= CAPACITY {
                        let _ = request.reply.send(Err(TelegramError::Backpressure));
                        continue;
                    }
                    let auth_mutation = matches!(
                        request.value["@type"].as_str(),
                        Some(
                            "setAuthenticationPhoneNumber"
                                | "checkAuthenticationCode"
                                | "checkAuthenticationPassword"
                                | "requestQrCodeAuthentication"
                                | "setAuthenticationEmailAddress"
                                | "checkAuthenticationEmailCode"
                                | "resendAuthenticationCode"
                        )
                    );
                    if auth_mutation && self.pending_auth.is_some() {
                        let _ = request.reply.send(Err(TelegramError::AuthBusy));
                        continue;
                    }
                    let correlation = Uuid::new_v4().to_string();
                    request.value["@extra"] = json!(correlation);
                    match native.send(id, &request.value) {
                        Ok(()) => {
                            if auth_mutation {
                                self.pending_auth = Some(correlation.clone());
                            }
                            self.pending.insert(
                                correlation,
                                Pending {
                                    deadline: request.deadline,
                                    reply: request.reply,
                                    authorization: request.value["@type"]
                                        == "getAuthorizationState",
                                },
                            );
                        }
                        Err(error) => {
                            let _ = request.reply.send(Err(error));
                        }
                    }
                }
            }
            match native.receive() {
                Ok(Some(value)) => {
                    if value["@client_id"].as_i64() != Some(i64::from(id)) {
                        continue;
                    }
                    if let Some(correlation) = value["@extra"].as_str() {
                        if self.pending_auth.as_deref() == Some(correlation) {
                            self.pending_auth = None;
                        }
                        if let Some(pending) = self.pending.remove(correlation) {
                            if pending.authorization
                                && value["@type"]
                                    .as_str()
                                    .is_some_and(|s| s.starts_with("authorizationState"))
                            {
                                self.update_authorization(value.clone());
                            }
                            let _ = pending.reply.send(check_response(value));
                        }
                    } else {
                        if let Ok(mut chats) = self.known_chats.lock() {
                            chats.observe(&value);
                        }
                        if value["@type"] == "updateAuthorizationState" {
                            let state = value["authorization_state"].clone();
                            let is_closed = state["@type"] == "authorizationStateClosed";
                            self.update_authorization(state);
                            if is_closed {
                                break;
                            }
                        }
                        if value["@type"] == "updateConnectionState" {
                            self.connection.send_replace(value["state"].clone());
                        }
                        let _ = self.updates.send(value);
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    self.shutdown.cancel();
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
            let expired: Vec<_> = self
                .pending
                .iter()
                .filter(|(_, p)| {
                    p.deadline
                        .is_some_and(|deadline| deadline <= Instant::now())
                        || p.reply.is_closed()
                })
                .map(|(key, _)| key.clone())
                .collect();
            for key in expired {
                if let Some(p) = self.pending.remove(&key) {
                    let _ = p.reply.send(Err(TelegramError::Timeout));
                }
            }
        }
        self.rx.close();
        for (_, p) in self.pending {
            let _ = p.reply.send(Err(TelegramError::Closed));
        }
        // Dropping the permit happens only after authorizationStateClosed.
    }
    fn update_authorization(&self, mut state: Value) {
        // Correlation metadata is transport data, not authorization state.
        if let Some(object) = state.as_object_mut() {
            object.remove("@extra");
            object.remove("@client_id");
        }
        if *self.auth.borrow() != state {
            crate::AuthStatus::from_td(&state).trace();
            self.auth.send_replace(state);
        }
    }
}

#[cfg(test)]
impl TdClient {
    /// In-process JSON TDLib double: exercises real request cancellation,
    /// timeouts, read policies and update subscriptions without native FFI.
    pub(crate) fn mock<F, Fut>(mut handler: F) -> Self
    where
        F: FnMut(Value, broadcast::Sender<Value>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<Value, TelegramError>> + Send + 'static,
    {
        let (requests, mut rx) = mpsc::channel::<Request>(CAPACITY);
        let (updates, _) = broadcast::channel(1024);
        let (auth_tx, auth) = watch::channel(json!({"@type":"authorizationStateReady"}));
        let (connection_tx, connection) = watch::channel(json!({"@type":"connectionStateReady"}));
        let (closed_tx, closed) = watch::channel(false);
        let mut connection_updates: broadcast::Receiver<Value> = updates.subscribe();
        tokio::spawn(async move {
            while let Ok(value) = connection_updates.recv().await {
                if value["@type"] == "updateConnectionState" {
                    connection_tx.send_replace(value["state"].clone());
                }
            }
        });
        let output = updates.clone();
        let known_chats = Arc::new(Mutex::new(KnownChats::default()));
        let worker_chats = known_chats.clone();
        let mut chat_updates = updates.subscribe();
        tokio::spawn(async move {
            let _states = (auth_tx, closed_tx);
            while let Some(request) = rx.recv().await {
                let result = handler(request.value, output.clone())
                    .await
                    .and_then(check_response);
                while let Ok(value) = chat_updates.try_recv() {
                    if let Ok(mut chats) = worker_chats.lock() {
                        chats.observe(&value);
                    }
                }
                let _ = request.reply.send(result);
            }
        });
        Self {
            retry_reads: true,
            request_counter: None,
            known_chats,
            auth_attempts: Arc::default(),
            requests,
            updates,
            auth,
            connection,
            closed,
            shutdown: CancellationToken::new(),
        }
    }
    pub(crate) fn seed_known_chat_ids(&self, ids: &[i64]) {
        if let Ok(mut chats) = self.known_chats.lock() {
            for id in ids {
                chats.observe(&json!({"@type":"updateNewChat","chat":{"id":id}}));
            }
        }
    }
}
