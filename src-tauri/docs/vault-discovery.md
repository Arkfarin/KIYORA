# Vault discovery

```sh
kiyora-cli vault discover
kiyora-cli vault discover --deep
kiyora-cli vault discover --json
kiyora-cli vault discover --deep --json
kiyora-cli vault attach <chat-id>
```

Discovery uses the currently selected, authorized Telegram profile. It never
creates a channel, changes the selected binding, or automatically attaches a
result. Multiple results include name, chat ID, vault UUID and protocol version;
choose the chat explicitly with `vault attach`. Existing profiles, VaultInit
messages and raw JSON protocol-1 markers remain readable.

## Ordinary discovery

1. Wait for a ready connection and use `getMe` to verify the profile's account.
2. If this profile has a saved binding, fetch its chat, verify ownership/privacy
   and re-read its VaultInit. Validate both its owner and saved vault UUID. A
   valid saved vault is returned immediately, even if renamed or archived.
3. Otherwise call `searchChats` and `searchChatsOnServer` for `kiyora.<user-id>`,
   with `searchChatTypeFilterChannel` and a limit of 20 for each call. Deduplicate
   by chat ID; inspect at most 40 search results. Local title filtering accepts
   case/separator differences and suffixes (`KIYORA . 42.backup`), but not a
   different user ID (`kiyora.420`). No history is read before this filter and
   channel/ownership/privacy validation pass.
4. Find and validate VaultInit in this order: saved `getMessage` ID, newest
   `getChatPinnedMessage`, `searchChatMessages` for the text `VAULT_INIT` (20
   matches, re-fetch the selected result), then at most three 100-message
   history pages. Short pages are not EOF. Ambiguous, untrusted, corrupt,
   mismatching and unsupported protocol records are rejected.

There are **no `loadChats` or `getChats` calls in ordinary discovery**. Tests
model an account with 15,000 chats and assert this directly. With ready TDLib,
no retries and no rejected candidates, request counts are:

| Path | TDLib calls |
| --- | ---: |
| Saved binding and message ID | 5 |
| No saved binding, no search matches | 3 |
| One named channel, no pin, indexed marker | 9 |

These are TDLib method calls, not Telegram network round trips: some methods use
TDLib's cache. Profile startup/authentication and any separate sync activity
are outside these counters.

The pinned [TDLib 1.8.69 schema](https://github.com/tdlib/td/blob/9d92b49bc5d0d2e29fe518239af647a88f8680e6/td/generate/scheme/td_api.tl)
defines `searchChats` as an offline search and `searchChatsOnServer` as a server
search over **known chats**. Neither discovers arbitrary private channels, and
neither takes a main/archive list restriction. Known archived channels can be
returned; coverage of every archived/private channel is not guaranteed. A miss
is not proof that the account has no vault. Text search is a hint, not a JSON
field index or proof that no other service message exists.

## Explicit deep discovery

`--deep` also enumerates main and archived lists, regardless of channel titles.
It continues after validating a saved vault so other vaults can be listed.

`getChats` has no offset and can itself load more chats in the pinned API, so
neither discovery mode calls it. The receive worker passively remembers IDs from
`updateNewChat` / `updateChatPosition`, without issuing requests. Deep discovery
first checks this cache, then calls `loadChats` in pages of at most 100 for main
and archive. Each page updates the cache before its request reply. This also
covers previously loaded chats and repeated discoveries without dependence on
a lossy broadcast consumer. A 404 from `loadChats` means that list is exhausted.
Chat IDs are deduplicated. An exhausted budget, malformed update or failed
request makes the report incomplete, rather than claiming the scan succeeded.

Personal chats and groups are excluded before reading history. Only private
channels owned by the current account with one member/admin, no public username,
no linked chat/location and no auto-delete pass the existing protocol's checks.
After marker hints fail, this explicit mode permits a full history traversal,
limited to 1,000 pages / 100,000 messages per channel. It validates the scanned
protocol context and rejects multiple Init records or mixed vault UUIDs.

| Limit | Ordinary | Deep |
| --- | ---: | ---: |
| Concurrent requests | 1 | 1 |
| Individual TDLib request | 30 s | 30 s |
| Candidate validation | 30 s | 120 s |
| Total search, after initial connection wait | 120 s | 900 s |
| Initial connection wait | 30 s | 30 s |
| History pages per candidate | 3 | 1,000 |
| Enumerated chats, both lists combined | 0 | 20,000 |
| `loadChats` requests per list | 0 | 1,000 |

These are upper bounds, not goals. Very large or slow legacy vaults may exceed
them. A user can pin the original valid VaultInit in Telegram and retry a manual
attach. Discovery does not alter pins or the VaultInit wire format.

## Progress, cancellation and JSON

The terminal updates elapsed time and real counters while work is running:
chats enumerated, search candidates found, channels inspected, marker validation
attempts completed, history messages read, vaults found and TDLib calls issued.
A candidate rejected for its title/type does not count as a channel inspected.
A failed marker check counts as a completed validation, not as a found vault.
Non-terminal stderr receives changed progress lines. `Ctrl+C` cancels the read
operation, releases the discovery lock and follows normal TDLib shutdown.

Only one discovery can run on a shared transport at a time. The existing profile
process lock also prevents two CLI processes from opening the same profile.
A profile switch cancels/invalidates the old operation; results cannot be stored
in another profile. Discovery waits for connectivity and stops on connection loss.

Discovery does not automatically retry timeouts, network errors or `FLOOD_WAIT`.
Rate limiting stops the operation immediately and reports `retry_after_seconds`;
retry only after that delay. Partial verified results are retained in the report.
A per-candidate timeout is diagnosed and other candidates may still be checked
within the overall deadline. Missing/deleted/inaccessible saved chats fall back
to targeted search; failures never trigger deep discovery or creation.

`--json` prints one JSON document on stdout and disables progress. The report has
`candidates`, `stats`, `complete`, `diagnostics`, and `failure`. IDs remain decimal
strings. Each candidate includes `protocol`. Pre-search errors instead produce
`{"error": <ApiError>}`. Errors may also be described on stderr; credentials,
session tokens, raw TDLib errors and message bodies are not printed. Incomplete
reports return a nonzero exit code even if some candidates were validated.
`complete: true` means the selected search scope finished, **not** that every
possible private Telegram vault was ruled out.

## Local hints and recovery

The existing per-profile `backend.json` binding remains authoritative for the
user's selection. Optional `init_message_id` and `discovery_hints` fields are
lookup accelerators; every message is fetched and validated again. Discovery
remembers up to 40 validated candidates so `vault attach <chat-id>` after deep
recovery does not repeat a long history scan. The selected vault's marker ID is
stored independently. The cache is bounded to stay below the existing 16 KiB
configuration limit. Old configuration files need no migration.

`vault create` records the permanent Init message ID after Telegram confirms it;
resumed creation also remembers an existing Init. An existing binding or pending
creation prevents another create. A zero-result discovery never creates a new
vault. Attachment still runs the existing sync/recovery checks before granting
ONLINE_READY; a discovery result alone does not audit the full event log.

## Manual live E2E

Use a dedicated test account, a configured secure store and the pinned TDLib
library; see [authentication](cli-auth.md) and [native setup](tdlib.md). From
`src-tauri`:

```sh
cargo build --locked -p vault-core --bin kiyora-cli
cli="$PWD/target/debug/kiyora-cli"
"$cli" auth status --json
"$cli" vault discover
"$cli" vault discover --json > /tmp/kiyora-discovery.json
python3 -m json.tool /tmp/kiyora-discovery.json
"$cli" vault discover --deep
"$cli" vault attach <validated-chat-id>
"$cli" vault discover --json
```

Confirm the last discovery uses its saved binding, has zero enumerated chats and
no history fallback. Rename and archive the **test** channel in Telegram, then
repeat: a saved binding must still work. In a clean test profile authorized to
the same account, verify targeted lookup where available and `--deep` recovery
of the renamed channel; attach explicitly. Test multiple valid vaults with the
same title and verify that all returned chat IDs remain distinct.

Run deep discovery and press Ctrl+C, then retry to verify lock release. Temporarily
disconnect the test machine to check a partial/error report. Do not manufacture
FLOOD_WAIT with live request spam; use the offline fixture tests. Corrupt/deleted
markers and future protocol versions should likewise be tested only in disposable
test channels or mocks. Real authenticated Telegram discovery and Windows/Android
behavior require these live checks; offline tests do not establish them.

```sh
cargo fmt --all -- --check
cargo check --locked -p vault-telegram
cargo check --locked -p vault-core --bin kiyora-cli
cargo test --locked -p vault-telegram
cargo test --locked -p vault-core
```

## Verification recorded on 2026-10-26

On this Linux workspace, all five commands above passed: formatting, both Cargo
checks, 44 `vault-telegram` tests and 76 `vault-core` tests. One opt-in native
TDLib test remained ignored. The discovery module contains 22 mock-TDLib tests;
its 15,000-chat regression also seeds the passive cache and still observes zero
chat enumeration, one candidate lookup and nine TDLib requests. Additional
checks cover profile-config compatibility, bounded hint persistence, CLI flags,
JSON reports and JSON errors. Authenticated live Telegram E2E was not run.
