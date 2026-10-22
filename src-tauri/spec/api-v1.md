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
| `backend_play_track` | `session_id`, `track_id`, `source_id` | unit; queues cached or remote streaming playback |
| `backend_playback_state` | `session_id` | current playback state |
| `backend_control_playback` | `session_id`, tagged action | updated playback state |
| `backend_logout` | `session_id` | revoke this profile's Telegram authorization |
| `backend_disconnect` | `session_id` | close the current native worker while preserving TDLib session data |
| `backend_disconnect` | `session_id` | unit |

`backend_authenticate.input` is tagged as `{ "kind": "phone|code|password|qr|email|email_code", "value": ... }`.
Secrets are moved into zeroizing values and persisted only through the OS secret
store. `backend_vault_request` supports `sync`, `initialize`, `verify`, `snapshot`,
GC planning/inspection/confirmed execution, media import with duplicate review,
ranged offline download, download status/cancel, cache stats/pin/clear, guarded event
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
validation. `get_cache_stats` returns complete `entries`, `pinned_entries`,
`partial_entries`, `partial_bytes`, `reserved_bytes`, `used_bytes` and `quota_bytes`.
Used bytes include complete files and partial data already written; reserved bytes
are additional space held for active downloads. Starting a download reserves its
full expected size and may evict unused, unpinned complete files. Interrupted
partial files remain resumable and count toward the quota until explicitly cleared.
Cache clearing skips active downloads and files leased by playback. A competing
writer for the same source returns `CONFLICT`. Sharing rejects unrelated control records and checks TDLib forward
permission. Shared library writes require `ONLINE_READY`; local projection reads
and cache clearing remain available while offline.
Playback opens an OS output device. A complete cached track/source retains a cache
lease while the decoder uses it; otherwise the service resolves the registered
source online and reads bounded Telegram ranges. Decoding runs on a separate
worker feeding a bounded PCM queue. `buffering` reports the track, position and
duration while waiting for audio; underflow silence does not advance track
position. Pause, resume, stop, seek, and volume controls emit contextual
`playback.state` events. Streamed prefixes are not marked fully cached: explicit
offline download remains the path to whole-file integrity verification and
offline availability. Closing a profile cancels its remote readers. Android playback remains
unavailable until its native audio context is wired.

Backend events are emitted on `kiyora://backend`. Each event includes the profile
and session context and a monotonically increasing sequence number scoped to that
profile. Consumers must discard events whose session ID is no longer current.

The CLI is a second adapter over the same `vault-core` services. Run
`kiyora-cli --help` for its command list; it shares profile storage, session
guards, API errors, sync and mutation logic with Tauri.
`kiyora-cli playback play <track-id> <source-id>` stays running until the track
ends or is stopped. Type `pause`, `resume`, `seek <ms>`, `volume <0..1>`, or `stop`
in that process; Ctrl-C also stops playback. A separate CLI process has no access
to the active process's audio engine.

This contract describes implemented commands. Download jobs are cancellable while
the process is running; completed ranged chunks resume after process exit and are
reported as `partially_cached` until the full file passes integrity verification.
Native Android playback/session commands are not part of v1 yet.
