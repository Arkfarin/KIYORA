use crate::*;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    time::Duration,
};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;
use vault_domain::{ChatId, MessageId, UserId};
use vault_protocol::{Record, VaultInit};

#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct VaultCandidate {
    /// All candidates passed the existing protocol decoder for this version.
    pub protocol: u32,
    pub channel: ChannelInfo,
    pub init: VaultInit,
    #[schemars(with = "String")]
    pub init_message_id: MessageId,
}

/// Bounded reverse scan that never treats short pages or numerical gaps as EOF.
pub async fn scan_history<T: Telegram>(
    transport: &T,
    chat: ChatId,
    after: i64,
    through: Option<MessageId>,
    max_records: usize,
    cancel: &CancellationToken,
) -> Result<Vec<RemoteMessage>, TelegramError> {
    let mut before = None;
    let mut records = BTreeMap::new();
    loop {
        if cancel.is_cancelled() {
            return Err(TelegramError::Cancelled);
        }
        let page = transport.history(chat, before, 100, cancel).await?;
        if page.is_empty() {
            break;
        }
        let next = page
            .iter()
            .map(|m| m.id)
            .min()
            .ok_or(TelegramError::IncompleteHistory)?;
        if before.is_some_and(|last| next >= last) {
            return Err(TelegramError::IncompleteHistory);
        }
        for message in page {
            if message.chat_id != chat || before.is_some_and(|last| message.id >= last) {
                return Err(TelegramError::IncompleteHistory);
            }
            if message.id.get() > after && through.is_none_or(|end| message.id <= end) {
                if let Some(old) = records.insert(message.id, message.clone())
                    && old != message
                {
                    return Err(TelegramError::IncompleteHistory);
                }
                if records.len() > max_records {
                    return Err(TelegramError::IncompleteHistory);
                }
            }
        }
        if next.get() <= after {
            break;
        }
        before = Some(next);
    }
    Ok(records.into_values().collect())
}

/// A saved marker is only a lookup hint. Its message and account are revalidated.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SavedVault {
    pub chat_id: ChatId,
    pub vault_id: Uuid,
    pub owner_id: UserId,
    pub init_message_id: Option<MessageId>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct DiscoveryOptions {
    pub deep: bool,
    pub saved: Option<SavedVault>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiscoveryStats {
    pub tdlib_requests: u64,
    pub chats_enumerated: usize,
    pub candidates_found: usize,
    pub channels_inspected: usize,
    pub candidates_validated: usize,
    pub history_messages: usize,
    pub vaults_found: usize,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiscoveryPhase {
    #[default]
    Saved,
    Search,
    Validate,
    Deep,
    Finished,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiscoveryProgress {
    pub phase: DiscoveryPhase,
    pub stats: DiscoveryStats,
    pub query: Option<String>,
    pub current_chat: Option<ChatId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryDiagnostic {
    pub chat_id: Option<ChatId>,
    pub error: TelegramError,
}

/// `complete` refers only to the requested search scope, never to all Telegram.
/// Partial results are retained on cancellation, rate limiting and timeouts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveryReport {
    pub candidates: Vec<VaultCandidate>,
    pub stats: DiscoveryStats,
    pub complete: bool,
    pub diagnostics: Vec<DiscoveryDiagnostic>,
    pub failure: Option<TelegramError>,
}

const SEARCH_LIMIT: usize = 20;
const MAX_CANDIDATES: usize = 40;
const FAST_HISTORY_PAGES: usize = 3;
const DEEP_HISTORY_PAGES: usize = 1000;
const MAX_DEEP_CHATS: usize = 20_000;
const MAX_LOAD_PAGES: usize = 1000;
const FAST_DEADLINE: Duration = Duration::from_secs(120);
const DEEP_DEADLINE: Duration = Duration::from_secs(900);
const FAST_CANDIDATE_DEADLINE: Duration = Duration::from_secs(30);
const DEEP_CANDIDATE_DEADLINE: Duration = Duration::from_secs(120);

/// Manual attach also stays bounded; only explicit deep discovery reads a full
/// legacy history. Discovery never grants ONLINE_READY: sync still audits it.
pub async fn inspect_vault<T: Telegram>(
    transport: &T,
    chat: ChatId,
    owner: UserId,
    expected: Option<Uuid>,
    cancel: &CancellationToken,
) -> Result<VaultCandidate, TelegramError> {
    inspect_vault_with_hint(transport, chat, owner, expected, None, cancel).await
}

pub async fn inspect_vault_with_hint<T: Telegram>(
    transport: &T,
    chat: ChatId,
    owner: UserId,
    expected: Option<Uuid>,
    known_message: Option<MessageId>,
    cancel: &CancellationToken,
) -> Result<VaultCandidate, TelegramError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(TelegramError::Cancelled),
        result = tokio::time::timeout(FAST_CANDIDATE_DEADLINE, async {
            let channel = transport.verify_channel(chat, owner, cancel).await?;
            inspect_verified_vault(transport, channel, expected, known_message, false, cancel, &mut |_| {}).await
        }) => result.map_err(|_| TelegramError::Timeout)?,
    }
}

fn marker(
    channel: &ChannelInfo,
    message: RemoteMessage,
    expected: Option<Uuid>,
) -> Result<Option<VaultCandidate>, TelegramError> {
    let Some(Record::Init(init)) = message.record()? else {
        return Ok(None);
    };
    if message.chat_id != channel.chat_id
        || message.content != MessageContent::Text
        || init.owner_telegram_user_id != channel.owner_id
        || expected.is_some_and(|id| init.vault_id != id)
    {
        return Err(TelegramError::InvalidVault);
    }
    Ok(Some(VaultCandidate {
        protocol: vault_protocol::PROTOCOL,
        channel: channel.clone(),
        init,
        init_message_id: message.id,
    }))
}

async fn inspect_verified_vault<T: Telegram>(
    transport: &T,
    channel: ChannelInfo,
    expected: Option<Uuid>,
    known_message: Option<MessageId>,
    deep: bool,
    cancel: &CancellationToken,
    progress: &mut (impl FnMut(usize) + Send),
) -> Result<VaultCandidate, TelegramError> {
    let chat = channel.chat_id;
    if let Some(id) = known_message
        && let Some(message) = transport.message(chat, id, cancel).await?
        && let Some(candidate) = marker(&channel, message, expected)?
    {
        return Ok(candidate);
    }
    if let Some(message) = transport.pinned_message(chat, cancel).await?
        && let Some(candidate) = marker(&channel, message, expected)?
    {
        return Ok(candidate);
    }
    if let Some(message) = transport.init_hint(chat, cancel).await?
        && let Some(candidate) = marker(&channel, message, expected)?
    {
        return Ok(candidate);
    }
    let mut before = None;
    let mut found: Option<VaultCandidate> = None;
    let mut vault_ids = BTreeSet::new();
    for _ in 0..if deep {
        DEEP_HISTORY_PAGES
    } else {
        FAST_HISTORY_PAGES
    } {
        let page = transport.history(chat, before, 100, cancel).await?;
        progress(page.len());
        if page.is_empty() {
            return found.ok_or(TelegramError::InvalidVault);
        }
        let next = page
            .iter()
            .map(|m| m.id)
            .min()
            .ok_or(TelegramError::IncompleteHistory)?;
        if before.is_some_and(|last| next >= last) {
            return Err(TelegramError::IncompleteHistory);
        }
        for message in page {
            if message.chat_id != chat || before.is_some_and(|last| message.id >= last) {
                return Err(TelegramError::IncompleteHistory);
            }
            if let Some(record) = message.record()? {
                vault_ids.insert(record.vault_id());
                if vault_ids.len() > 1 || expected.is_some_and(|id| record.vault_id() != id) {
                    return Err(TelegramError::InvalidVault);
                }
            }
            if let Some(candidate) = marker(&channel, message, expected)? {
                if found.is_some() {
                    return Err(TelegramError::InvalidVault);
                }
                found = Some(candidate);
            }
        }
        before = Some(next);
    }
    // No claim of absence, or uniqueness, when the bound cut off the scan.
    Err(TelegramError::IncompleteHistory)
}

fn title_matches(title: &str, owner: UserId) -> bool {
    // TDLib does token/prefix matching. Accept case and separator variations but
    // never let owner 42 match 420; a suffix such as .backup is allowed.
    let words = title
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    words.len() >= 2 && words[0].eq_ignore_ascii_case("kiyora") && words[1] == owner.to_string()
}

struct DiscoveryRun<'a> {
    transport: &'a TdTransport,
    owner: UserId,
    options: DiscoveryOptions,
    progress: Option<&'a watch::Sender<DiscoveryProgress>>,
    state: DiscoveryProgress,
    report: DiscoveryReport,
    visited: BTreeSet<ChatId>,
}
impl DiscoveryRun<'_> {
    fn publish(&self) {
        if let Some(tx) = self.progress {
            let mut state = self.state.clone();
            state.stats.tdlib_requests = self.transport.client().request_count();
            tx.send_replace(state);
        }
    }
    fn diagnostic(&mut self, chat_id: Option<ChatId>, error: TelegramError) {
        if matches!(
            error,
            TelegramError::IncompleteHistory | TelegramError::Timeout
        ) {
            self.report.complete = false;
        }
        self.report
            .diagnostics
            .push(DiscoveryDiagnostic { chat_id, error });
    }
    async fn check(
        &mut self,
        chat: ChatId,
        saved: Option<SavedVault>,
        filter_title: bool,
        cancel: &CancellationToken,
    ) -> Result<(), TelegramError> {
        if !self.visited.insert(chat) {
            return Ok(());
        }
        self.state.current_chat = Some(chat);
        self.publish();
        let timeout = if self.options.deep {
            DEEP_CANDIDATE_DEADLINE
        } else {
            FAST_CANDIDATE_DEADLINE
        };
        let result = tokio::time::timeout(timeout, async {
            let client = self.transport.client();
            let info = client
                .read(json!({"@type":"getChat","chat_id":chat.get()}), cancel)
                .await?;
            if info["type"]["@type"] != "chatTypeSupergroup"
                || info["type"]["is_channel"] != true
                || integer(&info["message_auto_delete_time"])? != 0
            {
                return Err(TelegramError::UnsafeChannel);
            }
            if filter_title
                && !title_matches(
                    info["title"]
                        .as_str()
                        .ok_or(TelegramError::InvalidResponse)?,
                    self.owner,
                )
            {
                // A renamed channel returned by username search can still be
                // checked by the later title-independent deep enumeration.
                self.visited.remove(&chat);
                return Err(TelegramError::UnsafeChannel);
            }
            self.state.stats.channels_inspected += 1;
            self.publish();
            let group_id = integer(&info["type"]["supergroup_id"])?;
            let group = client
                .read(
                    json!({"@type":"getSupergroup","supergroup_id":group_id}),
                    cancel,
                )
                .await?;
            // Reject ordinary members/admins before full info or any history.
            if group["status"]["@type"] != "chatMemberStatusCreator"
                || group["status"]["is_member"] != true
            {
                return Err(TelegramError::UnsafeChannel);
            }
            let full = client
                .read(
                    json!({"@type":"getSupergroupFullInfo","supergroup_id":group_id}),
                    cancel,
                )
                .await?;
            let channel = validate_channel(&info, &group, &full, chat, self.owner)?;
            self.state.phase = DiscoveryPhase::Validate;
            self.publish();
            let candidate = inspect_verified_vault(
                self.transport,
                channel,
                saved.map(|s| s.vault_id),
                saved.and_then(|s| s.init_message_id),
                self.options.deep,
                cancel,
                &mut |count| {
                    self.state.stats.history_messages += count;
                    self.publish();
                },
            )
            .await;
            self.state.stats.candidates_validated += 1;
            candidate
        })
        .await
        .unwrap_or(Err(TelegramError::Timeout));
        match result {
            Ok(candidate) => {
                self.report.candidates.push(candidate);
                self.state.stats.vaults_found = self.report.candidates.len();
            }
            Err(
                error @ (TelegramError::UnsafeChannel
                | TelegramError::InvalidVault
                | TelegramError::Protocol
                | TelegramError::IncompleteHistory
                | TelegramError::Timeout
                | TelegramError::Request {
                    code: 400 | 403 | 404,
                }),
            ) => {
                // Do not flood deep output with ordinary personal chats/groups.
                if !matches!(error, TelegramError::UnsafeChannel) || saved.is_some() {
                    self.diagnostic(Some(chat), error);
                }
            }
            Err(error) => return Err(error),
        }
        self.publish();
        Ok(())
    }
    async fn run(&mut self, cancel: &CancellationToken) -> Result<(), TelegramError> {
        self.publish();
        let me = self
            .transport
            .client()
            .read(json!({"@type":"getMe"}), cancel)
            .await?;
        if integer(&me["id"])? != self.owner.get() {
            return Err(TelegramError::AuthRequired);
        }
        if let Some(saved) = self.options.saved {
            if saved.owner_id == self.owner {
                self.check(saved.chat_id, Some(saved), false, cancel)
                    .await?;
                if !self.report.candidates.is_empty() && !self.options.deep {
                    return Ok(());
                }
            } else {
                self.diagnostic(Some(saved.chat_id), TelegramError::AuthRequired);
            }
        }
        self.state.phase = DiscoveryPhase::Search;
        self.state.query = Some(format!("kiyora.{}", self.owner));
        self.state.current_chat = None;
        self.publish();
        let mut ids = BTreeSet::new();
        // Both calls are needed: local results do not establish server coverage.
        for method in ["searchChats", "searchChatsOnServer"] {
            let value = self.transport.client().read(json!({"@type":method,
                "query":self.state.query,"type_filter":{"@type":"searchChatTypeFilterChannel"},"limit":SEARCH_LIMIT}), cancel).await?;
            let page = chat_ids(&value)?;
            if page.len() >= SEARCH_LIMIT && !self.options.deep {
                self.diagnostic(None, TelegramError::IncompleteHistory);
            }
            ids.extend(page.into_iter().take(SEARCH_LIMIT));
        }
        self.state.stats.candidates_found = ids.len();
        self.publish();
        for chat in ids.into_iter().take(MAX_CANDIDATES) {
            self.check(chat, None, true, cancel).await?;
        }
        if self.options.deep {
            self.deep(cancel).await?;
        }
        Ok(())
    }
    async fn deep(&mut self, cancel: &CancellationToken) -> Result<(), TelegramError> {
        // getChats can itself load a large prefix and has no offset. Use the
        // receive worker's passive ID cache, then load pages through loadChats.
        // The cache is updated before the load reply, so broadcast lag cannot
        // silently drop a page and repeated discoveries see previously loaded IDs.
        let mut enumerated = BTreeSet::new();
        self.state.phase = DiscoveryPhase::Deep;
        self.publish();
        let known = self.transport.client().known_chat_ids(MAX_DEEP_CHATS + 1)?;
        self.deep_page(known, &mut enumerated, cancel).await?;
        for kind in ["chatListMain", "chatListArchive"] {
            let mut exhausted = false;
            for _ in 0..MAX_LOAD_PAGES {
                self.state.phase = DiscoveryPhase::Deep;
                self.publish();
                match self
                    .transport
                    .client()
                    .request(
                        json!({"@type":"loadChats","chat_list":{"@type":kind},"limit":100}),
                        Duration::from_secs(30),
                        cancel,
                    )
                    .await
                {
                    Ok(_) => {}
                    Err(TelegramError::Request { code: 404 }) => exhausted = true,
                    // No more requests after a rate limit or network error.
                    Err(error) => return Err(error),
                }
                let page = self.transport.client().known_chat_ids(MAX_DEEP_CHATS + 1)?;
                self.deep_page(page, &mut enumerated, cancel).await?;
                if exhausted {
                    break;
                }
            }
            if !exhausted {
                return Err(TelegramError::IncompleteHistory);
            }
        }
        Ok(())
    }
    async fn deep_page(
        &mut self,
        page: Vec<ChatId>,
        seen: &mut BTreeSet<ChatId>,
        cancel: &CancellationToken,
    ) -> Result<(), TelegramError> {
        for chat in page {
            if !seen.insert(chat) {
                continue;
            }
            if seen.len() > MAX_DEEP_CHATS {
                return Err(TelegramError::IncompleteHistory);
            }
            self.state.stats.chats_enumerated = seen.len();
            self.check(chat, None, false, cancel).await?;
        }
        self.publish();
        Ok(())
    }
}

fn chat_ids(value: &serde_json::Value) -> Result<Vec<ChatId>, TelegramError> {
    value["chat_ids"]
        .as_array()
        .ok_or(TelegramError::InvalidResponse)?
        .iter()
        .map(|id| ChatId::new(integer(id)?).map_err(|_| TelegramError::InvalidResponse))
        .collect()
}

impl TdTransport {
    pub async fn discover(
        &self,
        owner: UserId,
        cancel: &CancellationToken,
    ) -> Result<Vec<VaultCandidate>, TelegramError> {
        let report = self
            .discover_with_options(owner, DiscoveryOptions::default(), None, cancel)
            .await?;
        if let Some(error) = report.failure {
            return Err(error);
        }
        if !report.complete {
            return Err(TelegramError::IncompleteHistory);
        }
        Ok(report.candidates)
    }

    pub async fn discover_with_options(
        &self,
        owner: UserId,
        options: DiscoveryOptions,
        progress: Option<&watch::Sender<DiscoveryProgress>>,
        cancel: &CancellationToken,
    ) -> Result<DiscoveryReport, TelegramError> {
        let _guard = self
            .discovery_gate
            .try_lock()
            .map_err(|_| TelegramError::AlreadyRunning)?;
        let mut transport = self.clone();
        transport.client = self.client.without_read_retries();
        let mut run = DiscoveryRun {
            transport: &transport,
            owner,
            options,
            progress,
            state: DiscoveryProgress::default(),
            visited: BTreeSet::new(),
            report: DiscoveryReport {
                candidates: Vec::new(),
                stats: DiscoveryStats::default(),
                complete: true,
                diagnostics: Vec::new(),
                failure: None,
            },
        };
        let deadline = if options.deep {
            DEEP_DEADLINE
        } else {
            FAST_DEADLINE
        };
        let mut connection = self.client().connection_changes();
        // A cached getChat/getMessage response alone cannot establish access
        // while offline. Wait for initial connectivity, then stop on its loss.
        let connected = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(TelegramError::Cancelled),
            result = tokio::time::timeout(Duration::from_secs(30), async {
                while connection.borrow()["@type"] != "connectionStateReady" {
                    connection.changed().await.map_err(|_| TelegramError::Closed)?;
                }
                Ok(())
            }) => result.unwrap_or(Err(TelegramError::Network)),
        };
        let disconnected = async {
            loop {
                if connection.changed().await.is_err() {
                    return TelegramError::Closed;
                }
                if matches!(
                    connection.borrow()["@type"].as_str(),
                    Some(
                        "connectionStateWaitingForNetwork"
                            | "connectionStateConnecting"
                            | "connectionStateConnectingToProxy"
                    )
                ) {
                    return TelegramError::Network;
                }
            }
        };
        // Keep the monitor alive only after connectivity was established.
        let result = tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(TelegramError::Cancelled),
            error = disconnected, if connected.is_ok() => Err(error),
            result = tokio::time::timeout(deadline, async { connected?; run.run(cancel).await }) => result.unwrap_or(Err(TelegramError::Timeout)),
        };
        if let Err(error) = result {
            run.report.complete = false;
            run.report.failure = Some(error);
        }
        run.state.phase = DiscoveryPhase::Finished;
        run.publish();
        run.report.stats = run.state.stats;
        run.report.stats.tdlib_requests = transport.client().request_count();
        Ok(run.report)
    }
    /// Only called for an explicit create intent. The caller journals the returned
    /// chat before publishing INIT and resumes that chat after interruption.
    pub async fn create_channel(
        &self,
        owner: UserId,
        cancel: &CancellationToken,
    ) -> Result<ChannelInfo, TelegramError> {
        self.create_channel_named(owner, None, cancel).await
    }
    pub async fn create_channel_named(
        &self,
        owner: UserId,
        title: Option<String>,
        cancel: &CancellationToken,
    ) -> Result<ChannelInfo, TelegramError> {
        let title = title.unwrap_or_else(|| format!("kiyora.{owner}"));
        if title.trim().is_empty() || title.chars().count() > 128 || title.contains('\0') {
            return Err(TelegramError::InvalidInput);
        }
        let result=self.client().request(json!({"@type":"createNewSupergroupChat","title":title,"is_forum":false,"is_channel":true,"description":"kiyora personal music vault","location":null,"message_auto_delete_time":0,"for_import":false}),std::time::Duration::from_secs(60),cancel).await?;
        let chat =
            ChatId::new(integer(&result["id"])?).map_err(|_| TelegramError::InvalidResponse)?;
        self.verify_channel(chat, owner, cancel).await
    }
}

#[cfg(test)]
mod tests;
