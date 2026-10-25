# Backend readiness — RELEASE_BLOCKED

Scope: the complete backend hardening request supplied on 2026-10-23. This is a
living evidence record, not an MVP acceptance declaration. Existing historical
reports under `src-tauri/docs` are leads, not proof of current behavior.

## Current audit (2026-10-25)

The complete requirement matrix, file changes, CI causes and remaining acceptance
criteria are in [BACKEND_COMPLETION_REPORT.md](BACKEND_COMPLETION_REPORT.md).
The sections below retain implementation and measurement history; the current
report and this section supersede older pending-verification notes.

- Final current Rust gate: **PASS**, 160 workspace tests, zero failures, two
  explicitly opt-in tests ignored. The gate additionally ran the real pinned
  TDLib start/version/close/reopen test successfully. Formatting, all-targets
  check, strict all-targets Clippy, CLI help and schema equality pass.
- Audio output failure/reopen: **PASS**, one opt-in test using real Rodio/ALSA
  null output and a simulated CPAL error signal. It proves worker cleanup and
  explicit restart; it does not prove hardware route migration or audible output.
- Android 15 x86_64 emulator: **PASS**, 14 plugin instrumentation tests and one
  installed-app test of packaged TDLib digest/loading, WebView and Rust IPC.
  Final-source APK passed the corrected teardown harness and two repeats;
  `scripts/check-android-app.sh` rejects runner crashes even when adb exits zero.
- Linux Debian extraction/native lifecycle: **PASS**. Newly added AppImage
  verification exposed a manifest mismatch caused by linuxdeploy adding RUNPATH;
  native preparation now supplies `$ORIGIN` before hashing. The final extracted
  AppImage digest/dependencies/native lifecycle also pass.
- The extracted AppImage also launched/rendered in isolated Xvfb for 15 seconds
  with a new empty data directory; this is a GUI startup smoke, not audio/account
  or full end-user acceptance. Screenshot and logs are in `target/audit-evidence`.
- Final ARM64 debug and unsigned release APKs and Linux debug/release
  Debian/AppImage native checks pass. Windows and live Telegram acceptance remain
  unverified. No personal account or vault was used.
- Current scale acceptance: **PASS**, 10,000 tracks / 100,000 entries / 10,000 tail
  events restored with exact reducer equality. Raw measurements and concurrent
  build load are documented in the completion report.
- Current GC acceptance: **PASS** at 1k/10k/50k/100k events, including preserved
  media/tombstones/post-barrier events and cold-restore equality. The 100k GC took
  39,381 ms in this uncontrolled host run; it is not evidence of a speedup over
  the historical figures below.

New fixes cover deletion during reconciliation, overlapping/cancelled profile
transitions, account-aware upload bounds, FloodWait, failed cache unpin, failed
background-job checkpoint, audio-device failures, actual Windows Clippy and
Android wrapper/ELF-verifier failures. Real SQLite SQLITE_FULL rollback/reopen
coverage was added. Frontend and public IPC schemas are unchanged in this increment.

## Implemented

Initial code audit found the workspace in `src-tauri` (11 libraries plus the Tauri
application), with a real TDLib 1.8.69 adapter, deterministic reducers, atomic
SQLite projections, full barrier snapshots, persisted GC plans, streaming PCM
workers, durable imports, cache leases, versioned IPC and Android native services.
The root `crates`/`spec` directories do not contain another active workspace.

Existing tests cover reducer convergence, tombstones, missing ACK, snapshot A/B
corruption, crash recovery, projection replacement, cache ownership, range reads,
profile isolation, JNI permissions and Android service lifecycle. Runtime and
platform results must be revalidated for this worktree.

## Fixed

- `vault-telegram/src/transport.rs`: server copies use `can_be_copied`, subscribe
  before sending and wait for the final TDLib message ID. A failure update is
  returned as failure. Removed a bot-only protected-content option. A C ABI
  fixture exercises completion arriving before the request response.
- `vault-db/src/{lib,migrations}.rs`: sequential migrations run in one writer
  transaction. Future schemas are rejected before any journal-mode change.
  Regression tests check byte preservation and rollback of a partial upgrade.
- `vault-sync/src/engine.rs` and `vault-core/src/service.rs`: warm reconciliation
  scans from a server anchor established by successful sync in this process.
  It independently checks the head and ownership, discovers missing new updates,
  and forces a full audit after disconnects, edits/deletions, overrun, changed
  anchors or new commits. Cold starts never trust a persisted readiness flag.
- `vault-snapshot/src/gc.rs`: GC now verifies actual message content types;
  an audio message with a control-looking caption is retained. All current track
  and artwork source references are protected, including obsolete service IDs.
- `vault-sync/src/gc.rs`: full snapshot/dependency validation is done once per
  execution/resume. Each batch checks new history, coordinator tenure, immutable
  copy/commit identities, exact candidate digests and confirmed absence. Only
  allowlisted deletion notifications are expected. Changed/unknown updates still
  invalidate the proof. Batches use TDLib's 100-message limit.
- GC dependency lookup uses indexed sets/maps; progress persistence no longer
  serializes the immutable complete allowlist on every batch.
- `src-tauri/src/lib.rs`: Tauri emits `kiyora://resync-required` on event overrun.
  `tauri.conf.json` enables a restricted production CSP; development has a
  separate policy for Vite.
- Local music imports now send TDLib `inputMessageAudio` with title, performer
  and duration through the confirmed-send path. Full metadata stays in the event;
  Telegram display fields are bounded separately. Snapshot/artwork files still
  use document uploads. C ABI tests validate the pinned request structures.
- Incoming imports check copy permission before reading content. Telegram audio
  copies server-side with a durable MEDIA marker, preserving Telegram metadata
  without downloading audio bytes. Audio documents are inspected locally, then
  copied server-side and read back against the known digest before registration.
  Lost ACK/restart uses the existing import journal; no blind resend or protected
  content fallback. Preview/import are behind the online guard.
  Separate Telegram cover thumbnails are downloaded with a 4 MiB bound, journaled,
  published and SHA-256 verified without fetching the audio payload.
- `vault-core/src/sharing.rs`: local write-ahead sharing records bind `op_id` to
  track/source/recipient and persist `sent` / `not_sent` / `unknown`. Replaying an
  operation never sends it again, including after restart. Only final TDLib ACKs
  produce a sent message ID. A caller can inspect the journal without sending.
- `vault-media/src/stream.rs`: a distant seek cancels an irrelevant prefetch
  immediately, including when playback remains paused. Repeated seek calls do not
  start replacement requests until the decoder reads. A delayed-fetch regression
  failed before the fix and passes after it.
- `vault-media/src/pcm/opus.rs`: Ogg/Opus mono/stereo decoding feeds the existing
  bounded Rodio PCM worker through statically bundled libopus 1.6.1. Applies
  pre-skip, end trimming, output gain and seek pre-roll. AIFF/ALAC features are
  enabled on the existing Rodio decoder. Format fixtures check import metadata,
  remote playback and seek for MP3/FLAC/AAC/M4A/ALAC/Vorbis/Opus/AIFF; Opus tests
  additionally compare exact frame counts and seek samples across Ogg pages.
- Native handles are cached per canonical library path instead of being leaked
  again on each profile connection. The real pinned TDLib test also closes and
  reopens a second client. Desktop lookup uses bundle/executable locations;
  Windows DLL dependencies use safe search flags. Tests cover aliases and lookup.
- Write journals no longer treat arbitrary Telegram errors as definite rejection.
  Server 5xx and request 408 retain unknown outcome for events, media, artwork,
  backup uploads and sharing. The import regression reproduced an unsafe resend
  before the fix and now passes for both accepted and absent server messages.

## Added

- Configurable automatic snapshots in `vault-core/src/maintenance.rs` and the
  single writer service. Defaults: 10,000 events, 16 MiB approximate log text,
  24 hours, 15 seconds idle, checked every 30 seconds. Configuration is persisted
  locally through `GetMaintenanceConfig` / `SetMaintenanceConfig`. Only the
  designated coordinator runs snapshots; pending GC or uncertain commands defer
  maintenance. It never performs automatic deletion.
- `state_snapshot` IPC returns a local projection and cursor under the writer
  lock, with session context and an event watermark. It needs no network and
  rejects stale profile sessions. The resubscription protocol is in
  `src-tauri/docs/api.md`; existing event names and APIs remain available.
- Tests for early send ACK, migration rollback, offline snapshots, subscriber
  lag, missing live events, reconnect/overrun, scheduler publication/read-back,
  GC interruptions/role transfer/copy loss between batches, audio captions and
  source retention. Test transport now supports deterministic between-batch faults.
- `vault-sync/examples/gc-benchmark.rs` verifies restore equality, live media,
  post-barrier events and tombstones at all four requested scales.
- Native source checksums, Linux/Windows preparation scripts, installer resource
  maps and Debian/MSI extraction verification; Android JNI staging, explicit
  extraction and APK payload verification. CI now includes these package gates.
  These are being exercised locally; script presence is not runtime proof. Exact
  commands and prerequisites: `src-tauri/docs/packaging.md`.
- Optional live-account, installed-app and physical-device acceptance procedure
  in `src-tauri/docs/e2e.md`; no personal credentials are read or required by CI.
  Android instrumentation now includes installed TDLib digest/loader and actual
  Tauri-to-Rust IPC startup checks (now executed on the Android 15 emulator).

## Performance

The initial implementation ran full sync before every mutation and full-history
planning before every 32-message deletion batch. The current warm idle guard uses
two history calls at every measured scale. GC downloads snapshots three times per
execution, independent of the number of deletion batches.

Release-mode measurements on the local Linux host, Rust 1.99.1, `FakeServer` and
in-memory SQLite (single runs, **not Telegram latency or disk throughput**):

| Events | GC before progress serialization fix (ms) | GC after (ms) | History calls | Delete calls |
| ---: | ---: | ---: | ---: | ---: |
| 1,000 | 29 | 27 | 53 | 11 |
| 10,000 | 782 | 568 | 413 | 101 |
| 50,000 | 14,069 | 6,110 | 2,013 | 501 |
| 100,000 | 50,811 | 16,309 | 4,013 | 1,001 |

Both columns already include the new GC batching algorithm; they are not a
comparison against the original full-history-per-batch implementation. Raw rows:
[`gc-2026-10-24.jsonl`](benchmarks/gc-2026-10-24.jsonl) and
[`gc-progress-optimized-2026-10-24.jsonl`](benchmarks/gc-progress-optimized-2026-10-24.jsonl).
All eight cases passed the preservation and cold-restore assertions. Per-message
reads and cumulative completed-ID persistence still leave further scaling work.

The final 2026-10-25 validation is retained separately in
[`gc-2026-10-25.jsonl`](benchmarks/gc-2026-10-25.jsonl) and
[`scale-2026-10-25.jsonl`](benchmarks/scale-2026-10-25.jsonl).
All four GC preservation/restore assertions and the 10k-track/100k-entry/10k-tail
scale assertion passed. GC wall times were 75 / 997 / 12,446 / 39,381 ms; no
controlled before/after performance comparison was performed in this final run.

## Tests

- Initial `bash src-tauri/scripts/check-backend.sh`: FAILED at rustfmt in
  `vault-sync/src/engine.rs::latest_commit_and_tail` before compilation/tests.
- Baseline `cargo test --locked --workspace --no-fail-fast`: 129 passed, zero
  failed, one optional native test ignored.
- `bash src-tauri/scripts/check-backend.sh` after sync, scheduler, IPC and GC
  changes: PASS (fmt, workspace check, strict Clippy, 139 tests, CLI smoke and
  schema equality; one optional native test ignored).
- Both `cargo run --locked --release -p vault-sync --example gc-benchmark` runs
  above completed at 1k/10k/50k/100k with assertions enabled.
- Resumed `bash src-tauri/scripts/check-backend.sh`, including the additional
  scheduler gate test: PASS, 140 passed / 0 failed / 1 optional native test ignored.
- Media regressions: `cargo test --locked -p vault-core --test service`: PASS,
  24 tests including server-copy permission/offline guards, unknown copy outcomes,
  restart recovery, sharing ACK loss and operation replay. The native ABI forwarding,
  metadata upload and copy test passes. These are deterministic simulations, not
  live Telegram account acceptance.
- Complete gate after media/sharing changes: PASS, 142 passed / 0 failed /
  1 optional native test ignored; formatting, strict Clippy and schemas pass.
- Explicit `KIYORA_TDJSON="$PWD/target/native/td-build/libtdjson.so" cargo test
  --locked -p vault-telegram --test real_tdlib -- --ignored`: PASS (1 test,
  pinned version, authorization-parameters state, clean native close; no account).
- Subsequent `cargo test --locked -p vault-media` after the seek fix: PASS,
  29 tests. A final aggregate gate remains due after the remaining changes.
  Local logs: `/tmp/kiyora-*.log`.
- After decoder support: `cargo test --locked -p vault-media`: PASS, 32 tests.
  The subsequent complete gate includes the truncation regression: PASS,
  147 passed / 0 failed / 1 optional native test ignored. Separate cover-import
  regression added afterward: service suite PASS, 25 tests.
- Android ARM64 native app with bundled Opus: `scripts/check-android-rust.sh`
  PASS using NDK r28c and the matching Rust 1.99.1 sysroot. AAudio linkage, JNI
  exports and 16 KiB ELF LOAD alignment pass. This is an app `.so`, not an APK or
  physical-device playback acceptance.
- Subsequent complete gate after native lookup/cache and cover handling: PASS,
  150 passed / 0 failed / 1 optional native test ignored. Formatting, Clippy,
  workspace compilation, CLI smoke and generated schemas pass.
- `cargo check --locked -p vault-telegram --target x86_64-pc-windows-msvc`:
  PASS with the matching Rust 1.99.1 Windows standard library. This checks the
  Windows loader branch; it does not link/run the complete Windows app or DLL.
- `scripts/prepare-linux-native.sh`: PASS, pinned TDLib built/stripped with static
  OpenSSL 3.5.9 and zlib 1.3.2; x86_64 ELF and system dependency checks passed.
  `scripts/package-desktop.py --debug` produced a 111.58 MiB Debian installer.
  `scripts/check-installer.py target/debug/bundle/deb/kiyora_0.1.0_amd64.deb`:
  PASS, extracted payload/digest/licenses/linkage and real TDLib start/version/
  close/reopen. Built on local Arch Linux; Ubuntu compatibility and audible
  output are not established by this result.
- Focused `upload_server_errors_remain_unknown_until_the_marker_is_found`: FAIL
  before the fix (retry uploaded again), PASS after the fix. Added event-journal
  and error classification regressions require the next complete gate.
- Complete gate after ambiguous-server-error fixes: PASS, 153 passed / 0 failed /
  1 optional native test ignored, including event intent reconciliation and
  rejection classification. Formatting, strict Clippy and schemas pass.
- Full Tauri Android debug APK assembly: PASS after fixing AndroidX opt-in,
  all-module Build Tools 36 selection, compatible buildSrc JVM targets and a
  configuration-cache-safe native dependency guard. Installed-app instrumentation
  APK compiles (108 Gradle tasks). Subsequent device execution and final
  payload checks pass; see the current audit above.

## Platform Status

| Platform | Status | Evidence / outstanding requirement |
| --- | --- | --- |
| Linux x86_64 | PARTIAL | Current Rust/native tests and extracted debug/release Debian pass; AppImage extraction and real native lifecycle pass; base Ubuntu 24.04 CI release package passed; isolated Xvfb AppImage GUI startup passed; hardware/live acceptance absent |
| Windows x86_64 | PLATFORM_UNVERIFIED | Base CI all-target check passed; identified large-enum Clippy failure fixed; current full CI, DLL/MSI and installed acceptance required |
| Android ARM64 | PARTIAL | Current debug APK ABI/dependencies/digest/16 KiB verification passed; physical/account acceptance required |
| Android x86_64 | PARTIAL | APK ABI/digest/dependencies/16 KiB checks and installed app+plugin emulator tests pass; no physical-audio/account claim |

macOS is not a required release target for this request.

## Remaining Issues

The local code/fault audit has produced concrete fixes and tests. Release remains
blocked on the following evidence, rather than the older open-ended audit plan:

1. Dedicated test-account live Telegram authentication, multi-device library,
   sharing, interrupted sends, long-absent restore and snapshot/GC acceptance.
2. Current Windows CI and installed MSI/DLL/audio/suspend acceptance.
3. Physical Linux/Windows/ARM64 audio, route changes and Android background,
   lockscreen, headset/Bluetooth and process restart endurance.
4. Retain current artifact verifications recorded in the completion report; rerun release-mode CI after publishing the patch for review.

Known limits: no Telegram cross-message transaction/CAS; no reconstruction after
all proofs are manually destroyed; sharing deduplication is local to its durable
journal; full tombstone/receipt retention and snapshot bounds constrain very large
vaults. Device error handling reopens output on the next explicit Play; it does
not claim automatic route/position migration. These limits and their priorities
are detailed in the completion report.

Do not push or publish a release. Do not use the ignored personal Telegram session
under `tmp_files`; use only explicitly provisioned test-account configuration.

## Earlier API changes (already present in the audit baseline)

`share_track` now requires `op_id`; its `shared` response adds explicit delivery
state and nullable message/error fields. Callers must check `state == "sent"`.
CLI was updated; no frontend consumer of this command exists in the checked tree.
`ImportInfo.sha256` and `import_review.sha256` now allow null for server copies
whose bytes have not been read locally. Existing string hashes/journals remain
readable; no fake digest is introduced. Other IPC commands/configuration and
response variants are additive; generated schemas were updated.
Existing vault wire protocol and SQLite schema
remain version 1; no destructive migration. Internal `RemoteMessage` retains the
actual content kind (older serialized hints default to unclassified). Frontend
design and source files are unchanged.

The existing lockfile retains previous package versions and adds four dependencies
for Opus/ALAC: `opus`, `opusic-sys` (bundled libopus), `cmake`,
`symphonia-codec-alac`. The existing Rust toolchain is unchanged. Native builds now
need CMake; Android passes its NDK toolchain/API explicitly. Added native licenses
are included in Tauri bundle resources; current Debian/AppImage/APK payload
verification is recorded in the completion report.

## Release Readiness

**RELEASE_BLOCKED**. Account-free tests and native/emulator checks do not satisfy
Windows, physical audio and mandatory Live E2E requirements. See the completion
report for reproducible commands and the exact remaining release criteria.
