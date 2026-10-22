# Acceptance status

This matrix tracks the 20 completion criteria in `spec/backend-task.md` §22.
“Pass” means the available evidence directly covers the criterion in its stated
scope. “Partial” means the implementation exists but required real-platform or
end-to-end evidence is missing. No partial item is a completion claim.

| # | Criterion | Status | Current evidence / remaining proof |
|---|---|---|---|
| 1 | Workspace builds on available targets | Partial | Linux `cargo check --workspace` passes. Android native plugin assembly/Lint and arm64 Rust plugin check pass; complete Android app/TDLib and Windows builds remain unverified. |
| 2 | TDLib adapter is real | Pass | Native tdjson worker, request correlation, updates, auth, history, send/upload/download paths; pinned 1.8.69 start/version/close smoke passes. |
| 3 | Authorization and session persistence | Partial | Auth flow and OS-backed encrypted TDLib key are implemented; no account login/reopen was tested with a provided test account. Android Keystore provider has five passing emulator tests; complete app startup and physical-device behavior remain unverified. |
| 4 | Vault discovery/creation | Pass (simulated) | Ownership/privacy and discovery/attach/create/resume paths have service and Telegram simulator coverage. No live vault operation was attempted. |
| 5 | SQLite and migrations | Pass | Transaction, atomic batch rollback, duplicate receipt, future-schema, FTS, snapshot-restore and reopen tests pass in `crates/vault-db/tests/transactions.rs`; mixed-action batch boundaries match pure and sequential replay in `batch_replay.rs`. |
| 6 | Required reducer events | Pass | Reducer action and invariant tests in `crates/vault-domain/tests/reducer.rs`; wire format fixtures in `crates/vault-protocol/tests/wire.rs`. |
| 7 | Shared writes require ONLINE_READY | Pass | Offline guard and no-queue behavior tested in `crates/vault-sync/tests/convergence.rs` and `crates/vault-core/tests/service.rs`. |
| 8 | History and live-update sync | Pass (simulated) | Multi-client convergence, reverse/short pages, delayed/duplicate updates, lost ACK and catch-up tests in `crates/vault-sync/tests/convergence.rs`. Live Telegram account path remains untested. |
| 9 | Snapshot recovery | Pass (simulated) | A/B validation, corrupt-copy recovery, interrupted initialization and old-state recovery tests in sync/snapshot suites. |
| 10 | Safe verifiable GC | Pass (simulated) | Digest-confirmed, resumable deletion and recovery tests in `crates/vault-sync/tests/convergence.rs`; no live Telegram deletion run. |
| 11 | Local audio import | Pass | Real audio metadata probing, hash/duplicate and invalid-input tests in `crates/vault-media/tests/local_media.rs`; service test verifies upload/register flow through simulator. |
| 12 | Downloads, cache and real playback | Partial | Ranged streaming feeds a bounded PCM queue; start-before-full-download, seek, retry, cancellation and WAV/MP3/FLAC/M4A/Vorbis decode tests pass. Full offline downloads retain integrity/quota/lease/publication coverage. Streaming against a live account and desktop output on hardware remain unverified; Android playback is not implemented. |
| 13 | Sharing | Pass (simulated) | Explicit recipient/source checks and simulator integration verify selected recipient; real Telegram forwarding is untested. |
| 14 | Backup/export/import | Pass (simulated) | Codec and service tests cover metadata/full export, media integrity, fresh-vault ID remapping and restore. Upload/recovery against Telegram is untested. |
| 15 | CLI works | Pass | `kiyora-cli` integration tests cover help, exit codes, shared profile store/status and JSON output. Full backend check runs the CLI help smoke. |
| 16 | Typed Tauri backend API | Pass | Strict request/response/error/event DTOs, Tauri commands, generated request/response/event schemas and schema drift checks. |
| 17 | Available automated checks pass | Pass | `scripts/check-backend.sh` passes fmt, workspace check, clippy, tests, CLI smoke and schema comparisons; opt-in run also passed native TDLib smoke. |
| 18 | Frontend unchanged | Pass | All implementation changes are confined to `src-tauri`; current repository diff is checked before each commit. |
| 19 | Real Telegram smoke when safe credentials are provided | Not run | No explicit test-account configuration was supplied. No personal Telegram session was used. |
| 20 | Unverified platforms are listed honestly | Pass | Linux is tested; Android plugin/Keystore evidence is separated from unverified full app/TDLib/physical-device builds and unimplemented playback. Windows remains unverified. |

## Remaining acceptance work

- Validate the Keystore bridge in the complete Android app and on physical hardware;
  implement Android playback/media-session integration. See `docs/android.md` for
  the native plugin, arm64 check and emulator tests already available.
- Obtain an isolated test Telegram account/channel configuration before attempting
  account authorization, vault creation, sharing, upload, restore or GC on Telegram.
- Exercise desktop output-device playback; decoder-only tests do not prove audible
  device playback.
- Add/execute CI and Windows/Android target builds when those toolchains are available.
- Extend the 1,000-event sync regression and 10,000-event local SQLite benchmark to
  larger end-to-end histories, and add broader chaos/load tests. Local debug replay
  timings and reproduction commands are recorded in `implementation-status.md`.
