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

No end-to-end backend acceptance gate is claimed complete. Native transport,
auth, sync, snapshot transport, GC, media, sharing, services/CLI and bridge remain.
