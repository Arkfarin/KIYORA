# kiyora backend implementation ledger

Scope: `spec/backend-task.md` and `spec/telegram_music_architecture_ru.md` (read in full).
All implementation lives under `src-tauri`; frontend must remain unchanged.
Project/CLI name: kiyora. Protocol marker remains TMV, protocol version 1.

## Execution plan and evidence

1. Repository audit, workspace, strict protocol, pure reducer, atomic SQLite/FTS.
2. Native tdjson lifecycle, request correlation, authorization, secure profiles,
   ownership-aware discovery and resumable initialization.
3. History/live sync, central online guard, command reconciliation, core service,
   Tauri adapters and contextual sequenced events.
4. Full barrier snapshots, independent A/B verification, cold recovery.
5. Persisted conservative GC and portable metadata/full backup restoration.
6. Media metadata/import, ranges, cache/leases, native desktop and Android playback.
7. Explicit sharing and incoming audio import.
8. Shared-service CLI, multi-client simulator, properties, crash/chaos regression suite.
9. CI, platform checks, API schemas, acceptance audit and final operating guide.

Each step needs implementation and tests; this list is not a completion claim.
Track evidence and remaining gaps here as modules land. Final acceptance must cover
all 20 criteria in the task, all RFC invariants, and the A01–G10 regression matrix.

## Environment audit

- Initial repository: clean `d5ece51`, Tauri greet command only, no backend crates.
- RFC found at `~/Downloads/telegram_music_architecture_ru.md`; copied into spec.
- Linux x86_64, system Rust/Cargo 1.99.1, GTK/WebKit/ALSA development packages present.
- `rustup` unavailable; Windows and Android toolchains not yet verified.
- No tdjson registered with ldconfig; no explicitly supplied test-account config.
- Real Telegram operations are opt-in only; existing personal sessions are not used.

## Verification log

- First foundation implemented: vault-domain, vault-protocol, vault-db, schema 1.
- 16 automated tests pass (including proptest): all reducer actions, deletion,
  entry order, replay/checkpoint equivalence, strict wire records, large IDs,
  actual SQLite trigger-injected rollback, restore rollback, local FTS, profile
  mismatch, future schema refusal, command ACK/unknown-result lifecycle.
- `cargo check --workspace` passed on Linux (including the original Tauri shell).
- `cargo fmt --all -- --check` and clippy with `-D warnings` passed for all targets
  of the three backend crates. Full workspace clippy/test still to be run.
- Linux linker override `cc` fixes the system toolchain's missing default
  `x86_64-linux-gnu-gcc`; no system configuration was modified.
- Remaining foundation checks: generated JSON schemas/fixtures, load benchmarks,
  expanded randomized all-action tests and acceptance-to-test traceability.
- Database currently reconstructs the projection per event; delta writes avoid
  rewriting untouched rows, but large-library replay still needs profiling.
- TDLib official source inspected at commit
  `9d92b49bc5d0d2e29fe518239af647a88f8680e6` (1.8.69); native build next.

## Transport, sync, snapshots and GC (in progress)

- Added real `vault-telegram` native JSON worker, auth requests, channel ownership
  and privacy checks, main/archive discovery, confirmed sends, range downloads.
- Added `vault-security`: profile directories, OS locks, account binding and
  Linux Secret Service / Windows Credential Manager / macOS Keychain providers.
  Android secure-store provider is still missing and explicitly refuses secrets.
- Added `vault-snapshot`: canonical full state, bounded zstd decode, A/B documents,
  read-back digest/context checks and independent append-only commit.
- Added `vault-sync`: snapshot bootstrap/recovery, server-ordered catch-up, bounded
  live updates, central guard, persisted command lifecycle, lost-ACK reconciliation.
- Added `vault-testkit`: independent clients sharing a server, gaps, reverse/short
  history, delayed/lost/duplicated updates, timeouts, flood wait, lost ACK, deletion,
  corrupt snapshot files and injected failures before calls/after delete.
- GC persists reviewable plans and progress, requires the exact confirmed digest,
  checks coordinator/current A/B before batches, selects covered events/barriers and
  old snapshot groups with checked copy dependencies, verifies deletion and resumes.
  Ancestry `previous_commit_message_id` is informational after full verification;
  it is not a required old-data dependency for a current full snapshot.
- Eight sync integration tests pass: 3-client convergence; unknown protocol;
  lost ACK/offline guard; old playlists/tombstones; corrupt A/B recovery;
  interrupted initialization; known manual deletion; crash-resumable conservative GC.
- Six transport/security tests pass; codec context/digest/truncation tests pass.
- Portable metadata/full backup codec passes three tests: roundtrip, no-clobber
  export, media digests, new-vault reference mapping, traversal/link/duplicate/size
  rejection. Restore planning exists; application-service execution is pending.
- Full workspace clippy (`-D warnings`) and tests pass on Linux: 34 tests passed,
  one opt-in native smoke ignored. Native build is still in progress.
- Native Linux tdjson is actively building at the pinned revision. Real account
  operations remain unverified and have not been attempted.

Remaining: native smoke result; native send/discovery contract tests; live service
task and Tauri bridge; durable channel-create intent; structured application API;
CLI; backup service/restore execution; media import/cache/stream/playback; sharing; Android
native layers; schema fixtures; CI; expanded chaos/load tests and final acceptance.
Sync currently rechecks all post-barrier receipts per catch-up to detect known
manual deletion. Performance of long tails must be measured before optimizing.
Initial setup/discovery currently scans history for INIT; cold recovery itself
reads recent commit plus post-barrier history. Search-based INIT hints remain.
No end-to-end backend acceptance gate is claimed complete.
