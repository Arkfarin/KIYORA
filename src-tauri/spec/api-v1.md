# Kiyora backend API v1

The Tauri v2 adapter exposes the commands below. Every session-scoped call takes
`session_id` from the current `Status.context`; a stale ID is rejected after a
profile switch. Errors serialize as `{ "code", "message", "retryable" }` and do
not contain raw TDLib messages, credentials, or local file paths. Telegram IDs in
the domain/protocol contract are decimal strings so JavaScript does not round them.

| Command | Arguments | Result |
| --- | --- | --- |
| `backend_list_profiles` | — | profile list |
| `backend_create_profile` | `label` | profile |
| `backend_switch_profile` | `profile_id` | runtime status and new session ID |
| `backend_runtime_status` | — | current runtime status |
| `backend_connect_telegram` | `session_id`, `api_id`, `api_hash` | authorization state |
| `backend_authenticate` | `session_id`, tagged `input` | authorization state |
| `backend_auth_status` | `session_id` | authorization state |
| `backend_discover_vaults` | `session_id` | owned compatible vault candidates |
| `backend_attach_vault` | `session_id`, decimal-string `chat_id` | runtime status |
| `backend_create_vault` | `session_id` | runtime status |
| `backend_resume_vault_creation` | `session_id`, `chat_id` | runtime status |
| `backend_library` | `session_id`, search/page fields | local projection page |
| `backend_playlists` | `session_id`, page fields | local projection page |
| `backend_vault_request` | `session_id`, typed request | typed operation result |
| `backend_disconnect` | `session_id` | unit |

`backend_authenticate.input` is tagged as `{ "kind": "phone|code|password|qr|email|email_code", "value": ... }`.
Secrets are moved into zeroizing values and persisted only through the OS secret
store. `backend_vault_request` supports `sync`, `initialize`, `verify`, `snapshot`,
GC planning/inspection/confirmed execution, media import with duplicate review,
and guarded event mutation. Shared library writes require `ONLINE_READY`; local
projection reads remain available while offline.

Backend events are emitted on `kiyora://backend`. Each event includes the profile
and session context and a monotonically increasing sequence number scoped to that
profile. Consumers must discard events whose session ID is no longer current.

The CLI is a second adapter over the same `vault-core` services. Run
`kiyora-cli --help` for its command list; it shares profile storage, session
guards, API errors, sync and mutation logic with Tauri.

This contract describes implemented commands. Streaming download management,
sharing and native Android playback/session commands are not part of v1 yet.
