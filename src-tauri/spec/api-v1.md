# Kiyora backend API v1

Generated request, response/error, and event schemas live in `spec/schema/`.
Regenerate them with `cargo run -p vault-core --example generate-api-schema`; the
backend check script verifies the checked-in schemas have not drifted.

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
| `backend_export_backup` | `session_id`, `path`, `kind` (`metadata` or `full`) | verified archive summary |
| `backend_play_track` | `session_id`, `track_id`, `source_id` | unit; starts local playback |
| `backend_playback_state` | `session_id` | current playback state |
| `backend_control_playback` | `session_id`, tagged action | updated playback state |
| `backend_disconnect` | `session_id` | unit |

`backend_authenticate.input` is tagged as `{ "kind": "phone|code|password|qr|email|email_code", "value": ... }`.
Secrets are moved into zeroizing values and persisted only through the OS secret
store. `backend_vault_request` supports `sync`, `initialize`, `verify`, `snapshot`,
GC planning/inspection/confirmed execution, media import with duplicate review,
ranged offline download, download status/cancel, cache pin/clear, guarded event
mutation, and `share_track` with one track/source and one explicit recipient chat.
It also supports `import_incoming_audio` by explicitly selected source chat and
message ID. The service fetches and validates the message, downloads the audio
in bounded ranges, inspects the media, and then uses the same duplicate-review
and vault registration flow as local imports. It also supports `import_backup`,
which restores a verified metadata or full
archive into a fresh empty vault by uploading media and creating new track,
playlist, entry, and source references. The restore plan is persisted locally so
the operation can resume after interruption. `backend_export_backup` writes
metadata archives directly; full archives require every referenced media file
in the verified local cache. Backup restore/export progress is emitted as backend
events.
Downloads emit byte progress and enter cache only after complete size and SHA-256
validation. Sharing rejects unrelated control records and checks TDLib forward
permission. Shared library writes require `ONLINE_READY`; local projection reads
and cache clearing remain available while offline.
Playback opens an OS output device and retains a verified cache lease while the
decoder uses the file. `backend_play_track` requires the selected track/source to
be fully cached; use `download_for_offline` first. Pause, resume, stop, seek, and
volume controls emit contextual `playback.state` events. Android playback remains
unavailable until its native audio context is wired.

Backend events are emitted on `kiyora://backend`. Each event includes the profile
and session context and a monotonically increasing sequence number scoped to that
profile. Consumers must discard events whose session ID is no longer current.

The CLI is a second adapter over the same `vault-core` services. Run
`kiyora-cli --help` for its command list; it shares profile storage, session
guards, API errors, sync and mutation logic with Tauri.

This contract describes implemented commands. Download jobs are cancellable while
the process is running; interrupted partial files are restarted after process exit.
Native Android playback/session commands are not part of v1 yet.
