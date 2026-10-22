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
The current criterion-by-criterion evidence and gaps are in `docs/acceptance-status.md`.

## Environment audit

- Initial repository: clean `d5ece51`, Tauri greet command only, no backend crates.
- RFC found at `~/Downloads/telegram_music_architecture_ru.md`; copied into spec.
- Linux x86_64, system Rust/Cargo 1.99.1, GTK/WebKit/ALSA development packages present.
- `rustup` unavailable; Windows and Android toolchains not yet verified.
- The installed Rust sysroot contains only `x86_64-unknown-linux-gnu`; no Android
  NDK or Windows linker is installed. `scripts/check-backend.sh` runs the complete
  portable Rust check suite and runs the native TDLib lifecycle smoke when
  `KIYORA_TDJSON` is set.
- Generated request, response/error, and backend event schemas cover the typed
  application boundary; a canonical TMV EVT schema envelope and byte fixture are
  also checked in.
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

## Transport, sync, snapshots and GC

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
- Pinned Linux tdjson build completed, and its credential-free native start/version/
  close smoke passed. The full backend check plus that opt-in native smoke was
  repeated on 2026-10-22 and passed. Real Telegram account operations remain unverified and were
  not attempted.

## Application service, CLI and local media (in progress)

- Added `vault-core`: bounded actor-backed service, typed API/errors, contextual
  sequenced events, profile/session fencing, Tauri commands, startup restoration,
  secure auth-secret handling, vault discovery/attach/create/resume and shared
  service methods. `auth logout` invokes TDLib `logOut`; `disconnect` only closes
  the current native worker so a later explicit reconnect can reuse its session.
  Frontend files remain untouched.
- Added `kiyora-cli` for profiles, auth, vault operations, sync, library/search,
  playlist show/add/remove/move, snapshots, GC, backup verification/restore/export,
  local and incoming media import, playback controls and diagnostics.
  The CLI shares the same backend guards and profile store as Tauri. `status`,
  `profiles list`, and `auth status` support machine-readable `--json` output;
  integration tests parse and verify status and profile JSON.
- Added `vault-media`: Lofty metadata/artwork inspection, stable staged import,
  SHA-256 duplicate review, Telegram document upload followed by track registration,
  local cache quota/LRU/pins/leases, and desktop Rodio playback controls.
- Desktop playback was checked against generated PCM WAV data without requiring an
  audio device. It supports configured FLAC, MP3, MP4/AAC, Vorbis and WAV decoders.
  Tauri and CLI expose cached-source playback, state, pause/resume/stop, seek, and
  volume controls; the decoder retains an active cache lease and emits state events.
- Full `cargo test --workspace` passes 54 tests; one native TDLib smoke test is
  opt-in/ignored by default. `cargo check --workspace` passes.
- Android secure storage, Android media session/foreground playback and CPAL
  Activity/JavaVM wiring are not implemented. Android playback fails explicitly.
- Streaming/ranged Telegram downloads feed verified full files into the profile-local
  cache. Service requests expose byte progress, status, cancellation, cache usage
  and quota statistics, pinning and clearing unpinned cache; CLI supports cache
  stats and offline download/cache clear and waits for completion, sending
  cancellation on Ctrl-C. Downloads run outside the sync actor, and shutdown
  cancels and joins active download tasks. Partial ranged downloads are durably
  staged in the profile cache, resume from the last complete chunk after reopening,
  and are reported as `PARTIALLY_CACHED`; concurrent jobs for the same source are
  rejected. Sharing supports one explicitly selected registered
  media source and recipient, checks TDLib `can_be_forwarded`, uses `forwardMessages`
  in copy mode with caption removed, rejects unrelated vault service records, and
  exposes a CLI/API result. Simulator integration confirms only the selected
  recipient is targeted. Incoming-audio import accepts an explicitly selected
  source chat and message, validates and range-downloads the audio, then reuses the
  local media inspection, duplicate review, upload, and registration pipeline.
- Backup codec can verify/export and create new-vault reference plans. Tauri/API
  and CLI can export metadata/full archives and restore verified archives into a
  fresh empty vault, uploading referenced media and mapping all library references
  to new IDs. Restore plans and progress are persisted/emitted for interruption
  recovery. Native TDLib account authorization,
  upload and recovery were not exercised against a real account.

Remaining: Android native layers, CI/platform builds,
expanded chaos/load tests and final acceptance traceability. Windows and Android
toolchains are unavailable/unverified in this environment. No end-to-end backend
acceptance gate is claimed complete.
Sync currently rechecks all post-barrier receipts per catch-up to detect known
manual deletion. Performance of long tails must be measured before optimizing.
Initial setup/discovery currently scans history for INIT; cold recovery itself
reads recent commit plus post-barrier history. Search-based INIT hints remain.
No end-to-end backend acceptance gate is claimed complete.
