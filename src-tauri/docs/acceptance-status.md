# Acceptance status

This matrix tracks the 20 completion criteria in `spec/backend-task.md` §22.
“Pass” means the available evidence directly covers the criterion in its stated
scope. “Partial” means the implementation exists but required real-platform or
end-to-end evidence is missing. No partial item is a completion claim.

| # | Criterion | Status | Current evidence / remaining proof |
|---|---|---|---|
| 1 | Workspace builds on available targets | Partial | Linux workspace gate passes. Android plugin assembly/Lint and full Rust arm64 native-library linking pass; JNI/AAudio and 16 KiB LOAD alignment verified. Complete APK/TDLib and Windows builds remain unverified. App minimum is API 26 because CPAL links AAudio. |
| 2 | TDLib adapter is real | Pass | Native tdjson worker, request correlation, updates, auth, history, send/upload/download paths; pinned 1.8.69 start/version/close smoke passes. |
| 3 | Authorization and session persistence | Partial | Native ABI fixture exercises Backend connect, asynchronous Ready, transient getMe retry and idempotent service activation. No real account login/reopen was tested. Android Keystore provider has six passing emulator tests; complete app startup and physical-device behavior remain unverified. |
| 4 | Vault discovery/creation | Pass (simulated) | Ownership/privacy and discovery/attach/create/resume paths have service and Telegram simulator coverage. No live vault operation was attempted. |
| 5 | SQLite and migrations | Pass | Transaction, atomic batch rollback, duplicate receipt, future-schema, FTS, snapshot-restore and reopen tests pass in `crates/vault-db/tests/transactions.rs`; mixed-action batch boundaries match pure and sequential replay in `batch_replay.rs`. |
| 6 | Required reducer events | Pass | Reducer action and invariant tests in `crates/vault-domain/tests/reducer.rs`; wire format fixtures in `crates/vault-protocol/tests/wire.rs`. |
| 7 | Shared writes require ONLINE_READY | Pass | Offline guard and no-queue behavior tested in `crates/vault-sync/tests/convergence.rs` and `crates/vault-core/tests/service.rs`. |
| 8 | History and live-update sync | Pass (simulated) | Multi-client convergence, reverse/short pages, delayed/duplicate updates, lost ACK and catch-up tests in `crates/vault-sync/tests/convergence.rs`. Live Telegram account path remains untested. |
| 9 | Snapshot recovery | Partial | A/B validation, one corrupt-copy recovery, interrupted initialization and old-state recovery have simulator tests. Safe older-generation fallback after both copies fail and damaged-SQLite repair remain. |
| 10 | Safe verifiable GC | Pass (simulated) | Digest-confirmed, resumable deletion and recovery tests in `crates/vault-sync/tests/convergence.rs`; no live Telegram deletion run. |
| 11 | Local audio import | Partial | Metadata/hash checks, exact duplicates and upload/register simulator tests pass. Durable upload journal reconciles lost ACK without sending twice. Durable pre-upload staging, artwork publication, heuristic review and bulk jobs remain. |
| 12 | Downloads, cache and real playback | Partial | Ranged streaming/PCM, seek/retry/cancel and codec tests pass; cache operations/quotas work without authorization. Android service/session/focus/notifications have emulator tests. Full Tauri JNI-to-PCM playback, physical output, live Telegram streaming, source availability and TDLib file-store accounting remain. |
| 13 | Sharing | Pass (simulated) | Explicit recipient/source checks and simulator integration verify selected recipient; real Telegram forwarding is untested. |
| 14 | Backup/export/import | Partial | Codec and simulator cover metadata/full export and full-backup restore into a fresh vault. Nonempty metadata backup restore, same-vault recovery and automatic downloading of missing full-export media remain. |
| 15 | CLI works | Partial | CLI tests cover profiles/status/JSON and domain-correct playlist order. Added command/settings/device, GC role/abandon, quota and import resume/orphans. Simulate, metadata editing, comprehensive diagnostics and cross-process playback controls remain. |
| 16 | Typed Tauri backend API | Pass | Strict request/response/error/event DTOs, Tauri commands, generated request/response/event schemas and schema drift checks. |
| 17 | Available automated checks pass | Pass | `scripts/check-backend.sh` passes fmt, workspace check, clippy, tests, CLI smoke and schema comparisons; opt-in run also passed native TDLib smoke. |
| 18 | Frontend unchanged | Pass | Frontend source remains unchanged; work concerns backend, native code, configuration and documentation. |
| 19 | Real Telegram smoke when safe credentials are provided | Not run | No explicit test-account configuration was supplied. No personal Telegram session was used. |
| 20 | Unverified platforms are listed honestly | Pass | Linux is tested; Android plugin/Keystore/media-service tests and arm64 compilation are separated from full APK/TDLib/physical-device acceptance. Windows remains unverified. |

## Remaining acceptance work

- Validate Keystore and playback through the complete Android app and on physical
  hardware. See `docs/android.md` for implemented OS integration and emulator evidence.
- Finish the outstanding implementation work in [remediation-status.md](remediation-status.md),
  including metadata restore, source availability, artwork, durable import staging,
  repair, scalability, cancellation/diagnostics, bulk/SAF and the playback queue.
- Obtain an isolated test Telegram account/channel configuration before attempting
  account authorization, vault creation, sharing, upload, restore or GC on Telegram.
- Exercise desktop output-device playback; decoder-only tests do not prove audible
  device playback.
- Add/execute CI and Windows/Android target builds when those toolchains are available.
- Extend the 1,000-event sync regression and 10,000-event local SQLite benchmark to
  larger end-to-end histories, and add broader chaos/load tests. Local debug replay
  timings and reproduction commands are recorded in `implementation-status.md`.
