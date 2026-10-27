#![allow(clippy::unwrap_used)]
use super::*;
use serde_json::Value;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;

const OWNER: i64 = 42;
const CHAT: i64 = -100;
fn owner() -> UserId {
    UserId::new(OWNER).unwrap()
}
fn chat(id: i64) -> ChatId {
    ChatId::new(id).unwrap()
}
fn init(chat_id: i64, id: i64, vault: u128) -> Value {
    let record = Record::Init(VaultInit {
        vault_id: Uuid::from_u128(vault),
        owner_telegram_user_id: owner(),
        created_by: "kiyora".into(),
        created_at_utc: "2026-10-25T00:00:00Z".into(),
    });
    message(chat_id, id, &vault_protocol::encode_text(&record).unwrap())
}
fn message(chat_id: i64, id: i64, text: &str) -> Value {
    json!({"@type":"message","id":id << 20,"chat_id":chat_id,"sending_state":null,
        "scheduling_state":null,"is_channel_post":true,"forward_info":null,"edit_date":0,
        "sender_id":{"@type":"messageSenderChat","chat_id":chat_id},
        "content":{"@type":"messageText","text":{"text":text,"entities":[]}}})
}
#[derive(Clone)]
struct Chat {
    title: String,
    channel: bool,
    creator: bool,
    pin: Option<Value>,
    search: Vec<Value>,
    messages: Vec<Value>,
    history: Vec<Value>,
    history_page: usize,
}
impl Default for Chat {
    fn default() -> Self {
        let marker = init(CHAT, 2, 1);
        Self {
            title: "kiyora.42".into(),
            channel: true,
            creator: true,
            pin: None,
            search: vec![marker.clone()],
            messages: vec![marker],
            history: vec![],
            history_page: 100,
        }
    }
}
struct Fake {
    me: i64,
    local: Vec<i64>,
    server: Vec<i64>,
    chats: BTreeMap<i64, Chat>,
    lists: BTreeMap<String, Vec<i64>>,
    loaded: BTreeMap<String, usize>,
    calls: Vec<Value>,
    error: Option<(&'static str, TelegramError)>,
    hang: Option<&'static str>,
    disconnect_at: Option<&'static str>,
    repeat_history: bool,
    endless_load: bool,
    search_more: bool,
}
impl Default for Fake {
    fn default() -> Self {
        Self {
            me: OWNER,
            local: vec![],
            server: vec![],
            chats: BTreeMap::from([(CHAT, Chat::default())]),
            lists: BTreeMap::new(),
            loaded: BTreeMap::new(),
            calls: vec![],
            error: None,
            hang: None,
            disconnect_at: None,
            repeat_history: false,
            endless_load: false,
            search_more: false,
        }
    }
}
impl Fake {
    fn response(
        &mut self,
        r: &Value,
        updates: &broadcast::Sender<Value>,
    ) -> Result<Value, TelegramError> {
        let method = r["@type"].as_str().unwrap();
        if let Some((name, error)) = &self.error
            && *name == method
        {
            return Err(error.clone());
        }
        if method == "getMe" {
            return Ok(json!({"id":self.me}));
        }
        if method == "searchChats" || method == "searchChatsOnServer" {
            assert_eq!(r["query"], "kiyora.42");
            assert_eq!(r["type_filter"]["@type"], "searchChatTypeFilterChannel");
            assert_eq!(r["limit"], SEARCH_LIMIT);
            let ids = if method == "searchChats" {
                &self.local
            } else {
                &self.server
            };
            return Ok(json!({"chat_ids":ids}));
        }
        if method == "getChats" || method == "loadChats" {
            let kind = r["chat_list"]["@type"].as_str().unwrap();
            let ids = self.lists.get(kind).cloned().unwrap_or_default();
            let loaded = self.loaded.entry(kind.into()).or_default();
            if method == "getChats" {
                assert_eq!(r["limit"], MAX_DEEP_CHATS);
                return Ok(json!({"chat_ids":ids.iter().take(*loaded).collect::<Vec<_>>()}));
            }
            assert_eq!(r["limit"], 100);
            if *loaded == ids.len() {
                return if self.endless_load {
                    Ok(json!({"@type":"ok"}))
                } else {
                    Err(TelegramError::Request { code: 404 })
                };
            }
            let end = (*loaded + 100).min(ids.len());
            for id in &ids[*loaded..end] {
                updates
                    .send(json!({"@type":"updateNewChat","chat":{"id":id}}))
                    .unwrap();
            }
            *loaded = end;
            return Ok(json!({"@type":"ok"}));
        }
        let id = if method.starts_with("getSupergroup") {
            -r["supergroup_id"].as_i64().unwrap()
        } else {
            r["chat_id"].as_i64().unwrap()
        };
        let c = self.chats.get(&id).cloned().unwrap_or(Chat {
            title: "ordinary chat".into(),
            channel: false,
            ..Chat::default()
        });
        Ok(match method {
            "getChat" => {
                json!({"id":id,"title":c.title,"type":{"@type":if c.channel {"chatTypeSupergroup"} else {"chatTypePrivate"}, "is_channel":c.channel,"supergroup_id":-id},"message_auto_delete_time":0})
            }
            "getSupergroup" => {
                json!({"is_channel":true,"status":{"@type":if c.creator {"chatMemberStatusCreator"} else {"chatMemberStatusMember"},"is_member":true},"has_location":false,"has_linked_chat":false,"usernames":null})
            }
            "getSupergroupFullInfo" => {
                json!({"member_count":1,"administrator_count":1,"linked_chat_id":0})
            }
            "getChatPinnedMessage" => return c.pin.ok_or(TelegramError::Request { code: 404 }),
            "searchChatMessages" => {
                assert_eq!(r["query"], "VAULT_INIT");
                assert_eq!(r["limit"], 20);
                json!({"messages":c.search,"next_from_message_id":if self.search_more { 1 << 20 } else { 0 }})
            }
            "getMessage" => {
                return c
                    .messages
                    .iter()
                    .find(|m| m["id"] == r["message_id"])
                    .cloned()
                    .ok_or(TelegramError::Request { code: 404 });
            }
            "getChatHistory" => {
                assert_eq!(r["limit"], 100);
                let from = r["from_message_id"].as_i64().unwrap();
                let values = c
                    .history
                    .into_iter()
                    .filter(|m| {
                        self.repeat_history || from == 0 || m["id"].as_i64().unwrap() <= from
                    })
                    .take(c.history_page)
                    .collect::<Vec<_>>();
                json!({"messages":values})
            }
            other => panic!("unexpected TDLib method: {other}"),
        })
    }
    fn count(&self, method: &str) -> usize {
        self.calls.iter().filter(|r| r["@type"] == method).count()
    }
}
fn mock(fake: Fake) -> (TdTransport, Arc<Mutex<Fake>>) {
    let preloaded = fake
        .loaded
        .iter()
        .flat_map(|(kind, count)| {
            fake.lists
                .get(kind)
                .into_iter()
                .flatten()
                .take(*count)
                .copied()
        })
        .collect::<Vec<_>>();
    let state = Arc::new(Mutex::new(fake));
    let shared = state.clone();
    let client = TdClient::mock(move |r, updates| {
        let state = shared.clone();
        async move {
            let (hang, disconnect) = {
                let mut s = state.lock().unwrap();
                s.calls.push(r.clone());
                (
                    s.hang.is_some_and(|name| r["@type"] == name),
                    s.disconnect_at.is_some_and(|name| r["@type"] == name),
                )
            };
            if disconnect {
                updates.send(json!({"@type":"updateConnectionState","state":{"@type":"connectionStateWaitingForNetwork"}})).unwrap();
            }
            if hang {
                std::future::pending::<()>().await;
            }
            state.lock().unwrap().response(&r, &updates)
        }
    });
    client.seed_known_chat_ids(&preloaded);
    (TdTransport::new(client), state)
}
async fn discover(t: &TdTransport, deep: bool) -> DiscoveryReport {
    t.discover_with_options(
        owner(),
        DiscoveryOptions { deep, saved: None },
        None,
        &CancellationToken::new(),
    )
    .await
    .unwrap()
}
fn saved() -> SavedVault {
    SavedVault {
        chat_id: chat(CHAT),
        vault_id: Uuid::from_u128(1),
        owner_id: owner(),
        init_message_id: Some(MessageId::new(2 << 20).unwrap()),
    }
}

#[tokio::test]
async fn saved_renamed_vault_uses_five_calls_and_no_search_or_history() {
    let mut f = Fake::default();
    f.chats.get_mut(&CHAT).unwrap().title = "renamed".into();
    let (t, calls) = mock(f);
    let report = t
        .discover_with_options(
            owner(),
            DiscoveryOptions {
                saved: Some(saved()),
                deep: false,
            },
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].channel.title, "renamed");
    assert_eq!(report.stats.tdlib_requests, 5);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.count("getMe"), 1);
    assert_eq!(calls.calls.len(), 5);
    assert_eq!(
        calls.count("getChats")
            + calls.count("loadChats")
            + calls.count("getChatHistory")
            + calls.count("searchChats"),
        0
    );
}

#[tokio::test]
async fn fast_search_with_15000_chats_only_checks_named_channels_including_archive() {
    let mut f = Fake {
        server: vec![CHAT],
        ..Default::default()
    };
    f.lists
        .insert("chatListMain".into(), (-16000..-1000).collect());
    f.loaded.insert("chatListMain".into(), 15_000);
    f.lists.insert("chatListArchive".into(), vec![CHAT]);
    f.chats.get_mut(&CHAT).unwrap().title = "KIYORA . 42.backup".into();
    let (t, calls) = mock(f);
    let report = discover(&t, false).await;
    assert!(report.complete);
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.candidates[0].protocol, 1);
    assert_eq!(report.stats.chats_enumerated, 0);
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls.count("getChats") + calls.count("loadChats") + calls.count("getChatHistory"),
        0
    );
    assert_eq!(calls.count("getChat"), 1);
    assert_eq!(calls.calls.len(), 9);
    assert_eq!(report.stats.tdlib_requests, 9);
}

#[tokio::test]
async fn wrong_names_types_and_permissions_never_read_history() {
    let mut f = Fake {
        local: vec![-100, -101, -102],
        server: vec![-100, -101, -102],
        ..Default::default()
    };
    f.chats.get_mut(&CHAT).unwrap().title = "kiyora.420".into();
    f.chats.insert(
        -101,
        Chat {
            channel: false,
            ..Default::default()
        },
    );
    f.chats.insert(
        -102,
        Chat {
            creator: false,
            ..Default::default()
        },
    );
    let (t, calls) = mock(f);
    let report = discover(&t, false).await;
    assert!(report.candidates.is_empty());
    let calls = calls.lock().unwrap();
    assert_eq!(calls.count("getChat"), 3);
    assert_eq!(
        calls.count("getSupergroupFullInfo")
            + calls.count("searchChatMessages")
            + calls.count("getChatPinnedMessage")
            + calls.count("getChatHistory"),
        0
    );
}

#[tokio::test]
async fn deep_finds_renamed_archived_vault_and_paginates_without_inspecting_personal_history() {
    let mut f = Fake::default();
    f.chats.get_mut(&CHAT).unwrap().title = "old vault".into();
    f.lists
        .insert("chatListMain".into(), (-1350..-1000).collect());
    f.lists.insert("chatListArchive".into(), vec![CHAT]);
    // Already loaded chats must also be enumerated, not only new updates.
    f.loaded.insert("chatListMain".into(), 125);
    let (t, calls) = mock(f);
    let (tx, rx) = watch::channel(DiscoveryProgress::default());
    let report = t
        .discover_with_options(
            owner(),
            DiscoveryOptions {
                deep: true,
                saved: None,
            },
            Some(&tx),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(report.complete, "{report:?}");
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(report.stats.chats_enumerated, 351);
    assert_eq!(report.stats.channels_inspected, 1);
    assert_eq!(rx.borrow().stats.vaults_found, 1);
    let calls = calls.lock().unwrap();
    assert_eq!(calls.count("getChats"), 0);
    assert_eq!(calls.count("loadChats"), 6);
    assert_eq!(calls.count("searchChatMessages"), 1);
    assert_eq!(calls.count("getChatHistory"), 0);
}

#[tokio::test]
async fn pinned_marker_avoids_search_and_history_and_unrelated_pin_falls_back() {
    for is_init in [true, false] {
        let mut f = Fake {
            local: vec![CHAT],
            ..Default::default()
        };
        f.chats.get_mut(&CHAT).unwrap().pin = Some(if is_init {
            init(CHAT, 2, 1)
        } else {
            message(CHAT, 3, "personal note")
        });
        let (t, calls) = mock(f);
        assert_eq!(discover(&t, false).await.candidates.len(), 1);
        let calls = calls.lock().unwrap();
        assert_eq!(calls.count("searchChatMessages"), usize::from(!is_init));
        assert_eq!(calls.count("getChatHistory"), 0);
    }
}

#[tokio::test]
async fn legacy_raw_json_init_and_short_history_pages_remain_supported() {
    let mut f = Fake {
        server: vec![CHAT],
        ..Default::default()
    };
    let c = f.chats.get_mut(&CHAT).unwrap();
    let old = init(CHAT, 2, 1)["content"]["text"]["text"]
        .as_str()
        .unwrap()
        .split_once('\n')
        .unwrap()
        .1
        .to_owned();
    c.search.clear();
    c.history = vec![message(CHAT, 20, "note"), message(CHAT, 2, &old)];
    c.history_page = 1;
    let (t, calls) = mock(f);
    let report = discover(&t, false).await;
    assert!(report.complete);
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(calls.lock().unwrap().count("getChatHistory"), 3);
}

#[tokio::test]
async fn fast_history_is_bounded_and_deep_recovers_old_unindexed_init() {
    let mut f = Fake {
        local: vec![CHAT],
        ..Default::default()
    };
    let c = f.chats.get_mut(&CHAT).unwrap();
    c.search.clear();
    c.history = (3..=402)
        .rev()
        .map(|id| message(CHAT, id, "note"))
        .collect();
    c.history.push(init(CHAT, 2, 1));
    let (t, calls) = mock(f);
    let fast = discover(&t, false).await;
    assert!(!fast.complete);
    assert!(fast.candidates.is_empty());
    assert_eq!(fast.stats.history_messages, 300);
    assert_eq!(calls.lock().unwrap().count("getChatHistory"), 3);
    calls.lock().unwrap().calls.clear();
    let deep = discover(&t, true).await;
    assert!(deep.complete, "{deep:?}");
    assert_eq!(deep.candidates.len(), 1);
    assert_eq!(calls.lock().unwrap().count("getChatHistory"), 6);
}

#[tokio::test]
async fn corrupt_future_wrong_owner_nil_and_untrusted_markers_are_rejected() {
    let valid = init(CHAT, 2, 1);
    let text = valid["content"]["text"]["text"].as_str().unwrap();
    let variants = [
        text.replace("\"protocol\":1", "\"protocol\":2"),
        text.replace(
            "\"owner_telegram_user_id\":\"42\"",
            "\"owner_telegram_user_id\":\"43\"",
        ),
        text.replace(&Uuid::from_u128(1).to_string(), &Uuid::nil().to_string()),
        "TMV/1 VAULT_INIT\n{broken}".into(),
    ];
    for text in variants {
        assert_ne!(text, valid["content"]["text"]["text"].as_str().unwrap());
        let mut f = Fake {
            local: vec![CHAT],
            ..Default::default()
        };
        f.chats.get_mut(&CHAT).unwrap().search = vec![message(CHAT, 2, &text)];
        let (t, _) = mock(f);
        let result = discover(&t, false).await;
        assert!(result.candidates.is_empty(), "{result:?}");
        assert!(!result.diagnostics.is_empty());
    }
    for field in ["forward_info", "edit_date", "sender_id"] {
        let mut bad = valid.clone();
        bad[field] = match field {
            "edit_date" => json!(1),
            "sender_id" => json!({"@type":"messageSenderUser","user_id":42}),
            _ => json!({}),
        };
        let mut f = Fake {
            local: vec![CHAT],
            ..Default::default()
        };
        f.chats.get_mut(&CHAT).unwrap().pin = Some(bad);
        let (t, _) = mock(f);
        assert!(discover(&t, false).await.candidates.is_empty());
    }
}

#[tokio::test]
async fn multiple_identical_names_are_returned_without_selection_and_ids_are_strings_in_json() {
    let mut f = Fake {
        local: vec![CHAT, -101],
        server: vec![CHAT, -101],
        ..Default::default()
    };
    let m = init(-101, 3, 2);
    f.chats.insert(
        -101,
        Chat {
            search: vec![m.clone()],
            messages: vec![m],
            ..Default::default()
        },
    );
    let (t, calls) = mock(f);
    let report = discover(&t, false).await;
    assert_eq!(report.candidates.len(), 2);
    assert_eq!(calls.lock().unwrap().count("getChat"), 2);
    let value = serde_json::to_value(&report).unwrap();
    assert_eq!(value["candidates"][0]["protocol"], 1);
    assert!(value["candidates"][0]["channel"]["chat_id"].is_string());
    assert!(
        calls
            .lock()
            .unwrap()
            .calls
            .iter()
            .all(|r| !r["@type"].as_str().unwrap().starts_with("create"))
    );
}

#[tokio::test]
async fn duplicate_init_and_missing_deleted_marker_do_not_identify_a_vault() {
    for duplicate in [true, false] {
        let mut f = Fake {
            local: vec![CHAT],
            ..Default::default()
        };
        f.chats.get_mut(&CHAT).unwrap().search = if duplicate {
            vec![init(CHAT, 2, 1), init(CHAT, 3, 1)]
        } else {
            vec![]
        };
        f.chats.get_mut(&CHAT).unwrap().messages.clear();
        let (t, _) = mock(f);
        let report = t
            .discover_with_options(
                owner(),
                DiscoveryOptions {
                    saved: Some(saved()),
                    deep: false,
                },
                None,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(report.candidates.is_empty());
        assert!(
            report
                .diagnostics
                .iter()
                .any(|d| d.error == TelegramError::InvalidVault)
        );
    }
}

#[tokio::test]
async fn stale_saved_chat_falls_back_and_account_mismatch_never_inspects_it() {
    let (t, calls) = mock(Fake {
        local: vec![CHAT],
        ..Default::default()
    });
    let stale = SavedVault {
        chat_id: chat(-900),
        ..saved()
    };
    let report = t
        .discover_with_options(
            owner(),
            DiscoveryOptions {
                saved: Some(stale),
                deep: false,
            },
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(report.candidates.len(), 1);
    assert_eq!(calls.lock().unwrap().count("getChat"), 2);
    let (t, calls) = mock(Fake {
        me: 43,
        ..Default::default()
    });
    let report = t
        .discover_with_options(
            owner(),
            DiscoveryOptions {
                saved: Some(saved()),
                deep: false,
            },
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(report.failure, Some(TelegramError::AuthRequired));
    assert_eq!(calls.lock().unwrap().calls.len(), 1);
    let (t, calls) = mock(Fake::default());
    let foreign = SavedVault {
        owner_id: UserId::new(43).unwrap(),
        ..saved()
    };
    t.discover_with_options(
        owner(),
        DiscoveryOptions {
            saved: Some(foreign),
            deep: false,
        },
        None,
        &CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(calls.lock().unwrap().count("getChat"), 0);
}

#[tokio::test]
async fn no_results_uses_only_identity_and_two_targeted_requests() {
    let (t, calls) = mock(Fake::default());
    let report = discover(&t, false).await;
    assert!(report.complete && report.candidates.is_empty());
    assert_eq!(calls.lock().unwrap().calls.len(), 3);
}

#[tokio::test]
async fn network_and_flood_wait_stop_without_retries_or_fallback_enumeration() {
    for error in [
        TelegramError::Network,
        TelegramError::RateLimited {
            retry_after_seconds: 3600,
        },
    ] {
        for method in ["searchChatsOnServer", "getSupergroupFullInfo"] {
            let (t, calls) = mock(Fake {
                local: vec![CHAT],
                error: Some((method, error.clone())),
                ..Default::default()
            });
            let report = discover(&t, false).await;
            assert_eq!(report.failure, Some(error.clone()));
            assert!(!report.complete);
            let calls = calls.lock().unwrap();
            assert_eq!(calls.count(method), 1);
            assert_eq!(calls.calls.last().unwrap()["@type"], method);
            assert_eq!(calls.count("getChats"), 0);
        }
    }
}

#[tokio::test(start_paused = true)]
async fn hung_candidate_times_out_and_reports_partial_scope() {
    let (t, calls) = mock(Fake {
        local: vec![CHAT],
        hang: Some("getChatPinnedMessage"),
        ..Default::default()
    });
    let report = discover(&t, false).await;
    assert!(!report.complete);
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.error == TelegramError::Timeout)
    );
    assert_eq!(calls.lock().unwrap().count("getChatPinnedMessage"), 1);
}

#[tokio::test]
async fn concurrent_discovery_is_rejected_and_ctrl_c_token_cancels_in_flight_read() {
    let (t, calls) = mock(Fake {
        hang: Some("searchChatsOnServer"),
        ..Default::default()
    });
    let cancel = CancellationToken::new();
    let transport = t.clone();
    let token = cancel.clone();
    let first = tokio::spawn(async move {
        transport
            .discover_with_options(owner(), DiscoveryOptions::default(), None, &token)
            .await
            .unwrap()
    });
    while calls.lock().unwrap().count("searchChatsOnServer") == 0 {
        tokio::task::yield_now().await;
    }
    assert!(matches!(
        t.discover(owner(), &CancellationToken::new()).await,
        Err(TelegramError::AlreadyRunning)
    ));
    cancel.cancel();
    let report = first.await.unwrap();
    assert_eq!(report.failure, Some(TelegramError::Cancelled));
    assert!(!report.complete);
    // The operation gate is released even when the request is cancelled.
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(matches!(
        t.discover(owner(), &cancelled).await,
        Err(TelegramError::Cancelled)
    ));
}

#[tokio::test]
async fn repeated_history_or_endless_chat_loading_is_bounded() {
    let mut f = Fake {
        local: vec![CHAT],
        repeat_history: true,
        ..Default::default()
    };
    let c = f.chats.get_mut(&CHAT).unwrap();
    c.search.clear();
    c.history = vec![message(CHAT, 10, "note")];
    let (t, calls) = mock(f);
    assert!(!discover(&t, true).await.complete);
    assert_eq!(calls.lock().unwrap().count("getChatHistory"), 2);
    let (t, calls) = mock(Fake {
        endless_load: true,
        ..Default::default()
    });
    let report = discover(&t, true).await;
    assert_eq!(report.failure, Some(TelegramError::IncompleteHistory));
    assert_eq!(calls.lock().unwrap().count("loadChats"), MAX_LOAD_PAGES);
}

#[tokio::test]
async fn truncated_search_is_reported_and_candidate_count_is_bounded() {
    let (t, calls) = mock(Fake {
        local: (-1000..-900).collect(),
        server: (-2000..-1900).collect(),
        ..Default::default()
    });
    let report = discover(&t, false).await;
    assert!(!report.complete);
    assert_eq!(calls.lock().unwrap().count("getChat"), MAX_CANDIDATES);
    assert_eq!(report.stats.candidates_found, MAX_CANDIDATES);
}

#[tokio::test(start_paused = true)]
async fn connection_loss_interrupts_a_hung_request_before_its_timeout() {
    let (t, _) = mock(Fake {
        local: vec![CHAT],
        hang: Some("getSupergroupFullInfo"),
        disconnect_at: Some("getSupergroupFullInfo"),
        ..Default::default()
    });
    let started = tokio::time::Instant::now();
    let report = discover(&t, false).await;
    assert_eq!(report.failure, Some(TelegramError::Network));
    assert!(started.elapsed() < FAST_CANDIDATE_DEADLINE);
}

#[tokio::test]
async fn deep_preserves_found_vaults_on_later_failure_and_checks_other_owned_vaults() {
    let mut f = Fake {
        local: vec![CHAT],
        ..Default::default()
    };
    f.lists.insert("chatListMain".into(), vec![CHAT, -101]);
    let other = init(-101, 2, 2);
    f.chats.insert(
        -101,
        Chat {
            title: "renamed backup".into(),
            search: vec![other.clone()],
            messages: vec![other],
            ..Default::default()
        },
    );
    let (t, calls) = mock(f);
    let report = t
        .discover_with_options(
            owner(),
            DiscoveryOptions {
                saved: Some(saved()),
                deep: true,
            },
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(report.candidates.len(), 2);
    assert_eq!(calls.lock().unwrap().count("getChat"), 2);
    let (t, _) = mock(Fake {
        local: vec![CHAT],
        error: Some(("loadChats", TelegramError::Network)),
        ..Default::default()
    });
    let partial = discover(&t, true).await;
    assert_eq!(partial.candidates.len(), 1);
    assert_eq!(partial.failure, Some(TelegramError::Network));
}

#[tokio::test]
async fn unverified_deleted_search_hint_and_saved_uuid_mismatch_are_rejected() {
    let mut f = Fake {
        local: vec![CHAT],
        ..Default::default()
    };
    f.chats.get_mut(&CHAT).unwrap().messages.clear();
    let (t, _) = mock(f);
    let report = discover(&t, false).await;
    assert!(report.candidates.is_empty());
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.error == TelegramError::InvalidVault)
    );
    let (t, _) = mock(Fake::default());
    let wrong = SavedVault {
        vault_id: Uuid::from_u128(2),
        ..saved()
    };
    let report = t
        .discover_with_options(
            owner(),
            DiscoveryOptions {
                saved: Some(wrong),
                deep: false,
            },
            None,
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(report.candidates.is_empty());
    assert!(
        report
            .diagnostics
            .iter()
            .any(|d| d.error == TelegramError::InvalidVault)
    );
}

#[tokio::test]
async fn incomplete_marker_search_does_not_bypass_bounded_history() {
    let mut f = Fake {
        local: vec![CHAT],
        search_more: true,
        ..Default::default()
    };
    f.chats.get_mut(&CHAT).unwrap().history = vec![init(CHAT, 2, 1)];
    let (t, calls) = mock(f);
    assert_eq!(discover(&t, false).await.candidates.len(), 1);
    assert_eq!(calls.lock().unwrap().count("getMessage"), 0);
    assert_eq!(calls.lock().unwrap().count("getChatHistory"), 2);
}

#[tokio::test]
async fn deep_does_not_repeat_rejected_marker_checks_but_revisits_renamed_search_results() {
    for renamed in [true, false] {
        let mut f = Fake {
            local: vec![CHAT],
            ..Default::default()
        };
        f.lists.insert("chatListMain".into(), vec![CHAT]);
        if renamed {
            f.chats.get_mut(&CHAT).unwrap().title = "renamed".into();
        } else {
            f.chats.get_mut(&CHAT).unwrap().search =
                vec![message(CHAT, 2, "TMV/1 VAULT_INIT\n{corrupt}")];
        }
        let (t, calls) = mock(f);
        let report = discover(&t, true).await;
        assert_eq!(report.candidates.len(), usize::from(renamed));
        assert_eq!(calls.lock().unwrap().count("searchChatMessages"), 1);
    }
}
