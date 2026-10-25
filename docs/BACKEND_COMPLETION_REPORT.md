# Backend completion report

Audit date: 2026-10-24–25. Status: **RELEASE_BLOCKED**.

This report covers the backend production-readiness request. It distinguishes
implemented behavior, deterministic simulations, native runtime tests and live
acceptance. The current changes are based on `aba97dad78a8cb2f1b25b63b68cd630b64f11c4a`;
they have not been pushed or tested on a new GitHub Actions run. Frontend source,
user vaults, personal Telegram sessions and user audio were not changed.

## 1. Initial state and architecture

The repository already had a working Rust workspace of 11 libraries and the
Tauri application. The audit preserved these boundaries:

`Tauri / CLI → vault-core → vault-sync → vault-telegram → pinned tdjson`

`vault-sync → vault-protocol / vault-domain → vault-db (SQLite)`

`vault-sync → vault-snapshot (full checkpoints, backup, explicit GC)`

`vault-core → vault-media (cache, range reads, decoder/mixer workers)`

`vault-core → vault-security / tauri-plugin-kiyora-native (profiles, secrets, Android)`

Previously implemented hardening is recorded in [readiness](BACKEND_READINESS.md).
It includes idempotent command/import journals, safe server copies, schema
migrations, tombstones, full A/B snapshots, conservative GC, audio decoders,
resumable imports and native packaging. These implementations were checked rather
than rewritten. The starting account-free gate passed 153 tests, with the optional
real TDLib test ignored. Successful Linux compilation did not imply Windows or
Android acceptance: the actual CI logs contained two platform failures.

Component status uses the requested vocabulary. COMPLETE refers only to the
stated local/code scope; PARTIAL and UNVERIFIED retain the external acceptance
work listed below.

| Component | Status | Evidence and remaining scope |
| --- | --- | --- |
| TDLib transport | PARTIAL | Pinned real library lifecycle; C ABI send/copy/error fixtures; live sends, DC migration and account recovery unverified |
| Authentication/session management | PARTIAL | Typed phone/code/2FA/QR/email states, revocation mapping, process lease, serialized profile transition; real-account acceptance unverified |
| Vault discovery/provisioning | PARTIAL | Owner/privacy/marker checks, main+archive scan, durable create intent; real renamed/deleted/multiple channels unverified |
| Local database | COMPLETE | Atomic outcomes/cursor/receipts, WAL, migrations, rollback, recovery and real SQLITE_FULL tests on Linux |
| Synchronization | PARTIAL | Multi-client simulation and fault regressions pass; Android↔Linux live acceptance unverified |
| Event log/reducer/tombstones | COMPLETE | Canonical strict protocol, unique op IDs, property tests, deterministic delete-wins; external manual loss remains detectable only where evidence exists |
| Snapshots/compaction | PARTIAL | Full barrier snapshots, A/B read-back, integrity/schema checks and cold restore tests; live-account acceptance unverified |
| Garbage collection | PARTIAL | Explicit confirmed plans, coordinator checks, interrupted batch tests, protected-media assertions; no real vault was pruned |
| Metadata/media import | PARTIAL | Real format fixtures, metadata and durable uploads, account-aware size checks; live transfer acceptance unverified |
| Audio streaming/playback | PARTIAL | Real codecs/PCM, bounded reads, seek/cancel tests and isolated OS output; audible hardware/suspend acceptance unverified |
| Sharing | PARTIAL | Server-copy permission and ACK handling, durable per-profile op journal; live recipient/protected-content cases unverified |
| Background services | PARTIAL | Durable jobs, cancellation, profile fencing; physical long-running Android playback unverified |
| IPC commands/events | COMPLETE | Shared typed dispatch, session validation, structured errors, schemas, lag/resync and secret boundary tests |
| Native integration | PARTIAL | Linux real loader and Android emulator tests; Windows native execution unverified |
| Packaging/CI | PARTIAL | Actual installer/APK inspection; latest remote Windows/Android run failed before these fixes; final CI still required |

No WebRTC subsystem was introduced; it is outside the v1 requirement.

## 2. Real defects fixed in this audit

Paths below are relative to `src-tauri` unless prefixed otherwise.

| Problem | Cause | Fix and changed files |
| --- | --- | --- |
| A deletion during full reconciliation could still grant ONLINE_READY | Captured history survived a delete notification; the full-audit flag was not enforced before readiness | Drain and reject continuity loss before projection replacement and at final verification; reject head regression. `crates/vault-sync/src/engine.rs`, `tests/convergence.rs`, `vault-testkit/src/lib.rs` |
| Concurrent profile switches could interleave teardown and publication | Releasing the session mutex while closing workers left the entire transition unserialized | Separate transition mutex; accepted switch/shutdown finishes if the caller disappears. `vault-core/src/backend.rs`, `tests/profiles.rs` |
| Oversized local files were hashed/staged before Telegram rejected them | Local media bound was incorrectly used as the only upload bound | Query TDLib `is_premium`; enforce the pinned transport's 2000/4000 MiB upload limit before inspection and immediately before send. `vault-core/src/imports.rs`, `tests/service.rs`, `vault-telegram/src/transport.rs`, `tests/forwarding.rs`, `tests/forwarding_fixture.c`, `vault-testkit/src/lib.rs` |
| Cached downloads retried FloodWait too early | Every retry used 150/300 ms | Respect server retry-after, allow cancellation during wait, retry only transient errors. `vault-media/src/download.rs`, `tests/local_media.rs` |
| Failed unpin exposed protected media to cache cleanup | Memory changed before atomic index publication without rollback | Restore the pin on persistence failure; test cleanup and reopen. `vault-media/src/cache.rs`, `tests/local_media.rs` |
| Output-device errors were logged without changing playback state | Default Rodio callback only logged errors | Signal worker, stop decoder/native service, release leases, report AUDIO_DEVICE_UNAVAILABLE and reopen output on next explicit Play. Preserve alternative-device fallback. `vault-media/src/playback.rs` |
| Windows CI stopped at Clippy | `Command::Play` had a platform-dependent large enum variant | Box the owned playback input; keep strict lint enabled. `vault-media/src/playback.rs` |
| Android CI rejected the Gradle distribution | Wrapper SHA-256 contained an extra trailing character | Correct official digest; exercise wrapper before lengthy native compilation. `gen/android/gradle/wrapper/gradle-wrapper.properties`, `../.github/workflows/backend.yml` |
| AppImage changed TDLib after its manifest was hashed | linuxdeploy inserted `$ORIGIN` RUNPATH into the bundled library | Supply the final RUNPATH at native link time, then strip/hash; the extracted binary must still match the prepared digest. `scripts/prepare-linux-native.sh` |
| Failed job checkpoint could be reported as Succeeded | In-memory job status changed before durable publication | Publish success only after persistence; report storage failure and permit explicit retry when persistence fails. `vault-core/src/jobs.rs` and its new fault test |
| x86_64 APK verifier rejected a valid library | Architecture matched a readelf display label instead of ELF identity | Check ELF64 little endian and numeric machine 62/183; retain dependency and 16 KiB checks. `scripts/verify-android-package.py` |
| Installed-app instrumentation intermittently reported a crash after assertions passed | Destroying the last Activity made Tao exit(0) before JUnit reported results | Stop the Activity and let the runner report before process termination; require explicit `OK (1 test)`. `gen/android/app/src/androidTest/java/io/github/arkfarin/kiyora/NativePackagingTest.kt`, `gen/android/app/build.gradle.kts`, `scripts/check-android-app.sh` |
| AppImage payload had no installer verification route | Verifier supported only Debian/MSI | Extract trusted local AppImage without FUSE and reuse digest/license/dependency/native-lifecycle checks; all installer checks use temporary paths containing spaces and Unicode. `scripts/check-installer.py` |

Additional integrity coverage: `vault-db/src/fault_tests.rs` forces SQLite's real
page limit to raise SQLITE_FULL without filling the host filesystem. It checks
whole-batch rollback, old data, command journal and cursor, reopen and successful
retry. `vault-db/src/lib.rs` registers it. Documentation changes include this report, readiness, the root README and packaging/E2E guides. `scripts/check-backend.sh` now checks
all workspace targets explicitly.

No wire-format, SQLite schema or frontend IPC schema change was required in this
increment. The new upload-limit method is internal to the Rust transport trait.
The pinned limit is derived from TDLib 1.8.69 `PartsManager.h` and
`FileLoaderUtils.cpp`; `is_premium` is a documented
[TDLib option](https://core.telegram.org/tdlib/options). Download and server-copy
limits remain independent of the sender's local upload allowance.

## 3. Requirement and invariant verification

All test paths below are under `src-tauri/crates`. Simulation evidence is not live
Telegram or cross-platform acceptance. A successful cold restore of an old
checkpoint is logical-time evidence, not a years-long endurance run.

| Requested cases | Authoritative evidence / result scope |
| --- | --- |
| Two/three clients; Android creates/Linux receives; simultaneous edits | `vault-sync/tests/convergence.rs::three_clients_converge_with_short_reverse_pages_delayed_duplicate_updates_and_delete_wins`; `vault-core/tests/service.rs::live_services_converge_without_manual_sync_and_events_are_contextual`. Simulated clients; physical Android↔Linux remains live gate |
| Delete vs add, old create/rename, tombstone permanence | `vault-domain/tests/reducer.rs::delete_wins_even_before_create_and_removes_related_entries` plus arbitrary replay/checkpoint property test and snapshot convergence cases |
| Duplicate messages/op IDs/tracks, wrong-vault/version rejection | Reducer duplicate/collision tests, strict protocol tests, DB batch replay properties, import duplicate tests; no unknown record skipped |
| Disconnect and ACK loss, accepted 5xx, reconnect, overrun | Convergence lost-ACK/server-error/overrun tests and service import/share journal tests; unknown sends are retained, never blindly resent |
| TDLib ready alone; offline write prohibition; stale old session | Online guard tests, service offline test, profile/IPC stale-session tests; cache reads/playback independent of sync |
| Deletion/edit during reconciliation; removed known history; vault access loss | New in-flight deletion regression, known-event deletion/anchor tests, channel validation; retain projection and refuse unsafe writes |
| Old local SQLite, new device, old snapshot plus tail | Snapshot compaction/cold-client convergence and local rebuild tests; confirmed hash and receipts used to reject incomplete recovery |
| Crash during batch/projector/checkpoint | `vault-db/tests/transactions.rs`, property batch tests and real SQLITE_FULL test; transaction retains previous cursor/outcomes |
| Schema upgrades/future schema | `vault-db/src/migrations.rs` sequential upgrade/rollback test; transactions future-schema byte preservation test |
| WAL/foreign keys/unique keys/busy timeout/indexes | `vault-db/src/lib.rs`, migrations SQL and transaction/query tests; multiple writers use profile locks and one service writer |
| Corrupt DB and sidecars | `vault-core/src/recovery.rs` tests simulate every installation rename and prove quarantine preservation; profile repair test retains access for explicit repair |
| Backups and traversal | `vault-snapshot/tests/backup.rs` rejects traversal, links, duplicate entries and oversized headers; full backup restore creates new media references |
| Snapshot completeness, checkpoint/tombstones/schema/digest | `vault-snapshot/tests/codec.rs`, sync snapshot tests; complete state, compressed hash, exact barrier and version checked |
| A corrupt/missing, both copies lost, missing history | A/B fallback, both-missing hash reconstruction and backup seed tests in convergence; insufficient evidence remains read-only |
| Interrupted snapshot publication and resume | Failed-initial-snapshot test and durable coordinator/upload logic; uncommitted copies do not authorize GC |
| Interrupted/partial GC, ACK loss, restart, second coordinator | GC resume/coordinator transfer/between-batch loss tests; persisted allowlist/digest checked before deletion |
| GC must retain media/post-barrier events and both current copies | GC live-source/control-caption test and scale examples; all source IDs, artwork, audio and new events protected |
| All named formats | Real MP3, FLAC, AAC, M4A, ALAC, Vorbis and Opus fixtures pass metadata/decoder/remote seek tests; AIFF/WAV also covered; no audible hardware claim |
| Metadata and fallback | `vault-media/tests/local_media.rs`, import tests, cover tests; full hash before local import, filename fallback, separate cover bounded to 4 MiB |
| Streaming, seek before download, slow network, cancellation | `vault-media/tests/streaming.rs`, PCM remote tests and distant-prefetch cancellation regression; bounded memory/ranges, no network on mixer callback |
| Long tracks | Chunk/position arithmetic and bounds inspected; no multi-hour hardware soak completed |
| LRU/quota/pins/active files/partials/reopen | Local-media cache suite including failed publication/unpin; cache never grants remote deletion authority |
| Upload size and rate limit | New transport ABI/service size tests and paused-time download FloodWait test; real Premium upload is unverified |
| Sharing/recipient/content protection | C ABI can_be_copied/final-ACK test, `vault-core/src/sharing.rs` and service regressions; no protected-content fallback, no channel access granted |
| Import incoming media and lost copy ACK | Server audio copies retain metadata; document bytes validated; service restart/marker tests; cover fetched independently |
| Phone/QR/code/2FA/errors/logout/session revoke | `vault-telegram/src/auth.rs`, lifecycle ABI test and native backend fixture cover mapping/correlation/close; actual account behavior is UNVERIFIED |
| Discovery rename/multiple/deleted/lost config/recreate | `vault-telegram/src/discovery.rs`, owner/private marker checks, durable creation/resume; title is not identity. Real Telegram scenarios are UNVERIFIED |
| Profile transitions, IPC cancellation and close | New simultaneous switch/cancel/shutdown test; job drain test; Tauri Exit awaits shutdown; process leases and session IDs fence old callers |
| Long jobs, event lag and refresh | Job persistence/interruption tests; event sequence/lag test and state_snapshot session/watermark tests; explicit resync-required event |
| Secrets/logging/local storage | No Debug on secret inputs; Zeroizing and redacted error mapping; desktop keyring/profile permissions and Android Keystore; native secret IPC rejects WebView access |
| CSP/capabilities/paths | Restricted production CSP, main-window capability, no native secret ACL grant; archive/import symlink and path-validation regressions |
| Android JNI/Keystore/services/document picker | Plugin instrumentation on isolated Android 15 x86_64 emulator; actual app/loader/IPC test tracked below; physical routes and lockscreen soak remain UNVERIFIED |
| Native resources/installers/release integrity | Pinned source SHA-256, packaged binary digest, licenses, dependency/ABI checks and extracted native lifecycle; platform results below |

Safety limits remain explicit: Telegram has no cross-message transaction/CAS.
An offline client with no independent receipt or backup cannot prove that a
manually deleted, never-observed event once existed. Read-only refusal is correct
when proof is missing; deleting/recreating local SQLite is not a repair strategy.

## 4. Validation commands and evidence

Commands run from `src-tauri`. Current logs are under `target/audit-evidence/`
(ignored local build output, not committed account data). Exact final gate counts
and installer/device results below were observed, not inferred from artifacts alone.

| Command | Result |
| --- | --- |
| `KIYORA_TDJSON="$PWD/target/native/desktop/native/libtdjson.so" bash scripts/check-backend.sh` | PASS: fmt, all-target check, strict Clippy, 160 workspace tests / 0 failures / 2 opt-in tests ignored; CLI and schemas; additionally 1 real TDLib lifecycle test passed (`backend-final.log`) |
| Gate's `cargo fmt --all -- --check` | PASS in final gate |
| `cargo check --locked --workspace --all-targets` | PASS in final gate |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | PASS in final gate; no lint exemptions added for Windows failure |
| `cargo test --locked --workspace` | PASS in final gate; ignored tests reported separately |
| `cargo test --locked -p vault-media --lib output_device_failure_releases_media_and_next_play_reopens -- --ignored` with isolated `ALSA_CONFIG_PATH` | PASS, 1 test; real OS output/mixer, simulated device-error signal |
| `python3 scripts/package-desktop.py --debug --bundles deb,appimage` | PASS, debug Debian 111.66 MiB + AppImage 203.15 MiB on Arch x86_64; final native RUNPATH included before hash (`linux-package-final.log`) |
| `python3 scripts/check-installer.py <deb-or-AppImage>` | PASS for both debug and release packages, including real TDLib lifecycle and release extraction paths with spaces/Unicode (`deb-final.log`, `appimage-final.log`, `deb-release.log`, `appimage-release.log`) |
| `python3 scripts/package-desktop.py --bundles deb,appimage` | PASS, optimized current release Debian 23.94 MiB + AppImage 127.42 MiB; subsequent extracted verification passed (`linux-release-package.log`) |
| Extracted debug and release AppImage `AppRun` with isolated Xvfb and fresh data/cache/config directories | PASS: each remained live and rendered the existing frontend for 15 seconds; screenshots inspected (`linux-gui-result.log`, `linux-release-gui-result.log`, `linux-release-startup.png`). Terminated only these test processes; no account loaded |
| `gen/android/gradlew --version` with isolated Gradle home | PASS, Gradle 8.14.3; corrected checksum distribution installed |
| `KIYORA_ANDROID_ABI=x86_64 bash scripts/package-android.sh --debug` | PASS, final-source debug APK and native checks (`android-x86-final.log`); initial readelf-label rejection reproduced and fixed; NDK r28c, Rust 1.99.1 |
| `KIYORA_ANDROID_DEVICE_TESTS=1 KIYORA_ANDROID_SERIAL=emulator-5580 bash scripts/check-android-plugin.sh` | PASS: build/lint and `OK (14 tests)` on Android 15 x86_64 (`android-plugin.log`) |
| `KIYORA_ANDROID_ABI=arm64-v8a bash scripts/package-android.sh --debug` | PASS, final source; build and actual APK verification (`android-arm64-final.log`) |
| `KIYORA_ANDROID_ABI=arm64-v8a bash scripts/package-android.sh` | PASS, optimized unsigned release APK, 20.50 MiB; actual native payload/ABI/digest/dependencies/licenses/16 KiB verification (`android-arm64-release.log`) |
| Installed app `NativePackagingTest` | PASS on final-source APK with `bash scripts/check-android-app.sh`: `OK (1 test)` plus two consecutive repeats (`android-app-harness-final.log`, `android-app-repeat-{1,2}.log`); digest/loader, Activity/WebView and Rust IPC |
| `cargo run --locked --release -p vault-sync --example scale-benchmark` | PASS: 10,000 tracks / 100,000 entries / 10,000 tail events; exact reducer equality, 102 history requests; cold sync 109,520 ms, snapshot 5,147 ms, page query 6 ms (`scale-final.log`) |
| `cargo run --locked --release -p vault-sync --example gc-benchmark -- 1000 10000 50000 100000` | PASS at all four scales: live media, post-barrier event, tombstone and cold-restore equality; GC 75 / 997 / 12,446 / 39,381 ms (`gc-final.log`) |

The current scale result is retained in
[`benchmarks/scale-2026-10-25.jsonl`](benchmarks/scale-2026-10-25.jsonl).
It used the deterministic fake server while the Android release build was active.
These wall-clock values include host contention and are neither Telegram latency
measurements nor a controlled performance comparison against the historical runs.
The current GC rows are retained in
[`benchmarks/gc-2026-10-25.jsonl`](benchmarks/gc-2026-10-25.jsonl).
GC wall time is higher than the historical run; this single uncontrolled host run
does not establish a performance improvement or isolate a regression. Request
counts remain 2 history reads for idle sync and 3 snapshot downloads per GC;
the 100k case makes 1,001 delete calls and 203,025 message checks. The remaining
per-message verification cost is explicit rather than hidden by a small fixture.

Tests that reproduced failures before fixes: in-flight history deletion, concurrent
profile switches, FloodWait delay, failed unpin and job checkpoint failure.
Packaging checks additionally reproduced the AppImage digest and x86_64 ELF-label
failures. SQLite disk-full coverage adds a
real failure mode rather than asserting a previously observed production incident.
The native ABI tests remain fixtures, not a logged-in Telegram service.

The final Android APK initially produced `shortMsg=Process crashed` during test
teardown despite its assertions passing. Logcat and ActivityManager exit history
showed `EXIT_SELF`, status 0, after the test finished: Tao's last-Activity shutdown
raced instrumentation reporting. The revised harness keeps the Activity stopped,
uses AndroidX's supported `waitForActivitiesToComplete=false` argument, and still
requires all assertions and an explicit successful runner result. It passed once
through the new script and twice consecutively afterward. The failed run is
retained as `android-app-final.log`; it is not counted as a successful check.


Whole-file SHA-256 of the verified local release packages:

| Artifact | SHA-256 |
| --- | --- |
| Debian | `cdd8f6913178f8823ed18a5a94b625a9b9a22dc80cf54e2540dc59e82528113b` |
| AppImage | `0c8034eb1c38960cb4a7aec52a5504e725863c83d157433593e6c3c430e8eaa6` |
| ARM64 unsigned APK | `a3f894c6d3b9f430ebbe0b631a3fb5c9241a49ac951ba882cc275f896d1912ea` |

These identify this local build, not a signed or published release.

## 5. CI and platform status

[GitHub run 40105548845](https://github.com/Arkfarin/kiyora/actions/runs/40105548845)
for the base revision is completed with failure:

- Linux/Ubuntu 24.04: passed Rust gates, scale acceptance, release Debian packaging
  and extracted-installer verification.
- Windows Server 2022: all-target check passed; strict Clippy failed on the large
  `Command::Play` variant. Test/packaging stages were not reached.
- Android: ARM64/JNI/16 KiB checks and plugin compile/lint passed. App APK build
  stopped on the malformed Gradle SHA-256. This is not a native linker failure.

The causes were established from job logs, not guessed from exit code 1. The
uncommitted fixes have not been exercised by Windows CI. Local cross-compilation
of the loader branch does not establish Windows MSI/runtime readiness.

| Target | Current status | Unverified acceptance |
| --- | --- | --- |
| Linux x86_64 | Current local native/decoder and extracted debug + release Debian/AppImage verification pass; base CI Ubuntu release package passed | Xvfb GUI startup passed; audible PipeWire/PulseAudio/hardware, suspend, live Telegram and current Ubuntu release build remain unverified |
| Windows x86_64 | PLATFORM_UNVERIFIED; identified Clippy cause corrected | Full current Windows CI, DLL/MSI, Unicode paths, installed launch, audio/suspend/account |
| Android ARM64 | Current-source debug and optimized unsigned release APKs built and passed native payload/ABI/digest/dependencies/16 KiB verification (`android-arm64-final.log`, `android-arm64-release.log`) | Physical ARM64 playback, 16 KiB device, background/lockscreen/Bluetooth/account |
| Android x86_64 | PARTIAL: APK payload verification, 14 plugin tests and 1 installed-app test passed | Physical audio and live-account acceptance |

Debug packages and optimized Linux/ARM64 release packages are checked locally;
results are recorded separately above. Android release signing and distribution
were not performed. No package has been published as a signed release. No claim
of reproducible binary output across compilers or distro compatibility follows
from matching pinned source archives.

## 6. Remaining limitations and unresolved release gates

- **P0 release gate — live Telegram acceptance:** no explicitly provisioned test
  credentials/account/vault/recipient. Real login, revoke, upload/copy/share,
  multi-device sync and live snapshot/GC recovery have not been run. Do not use
  ignored personal sessions to fill this gap.
- **P0 release gate — Windows:** rerun required Windows CI and installed MSI/DLL
  lifecycle on Windows; this Linux environment cannot provide that evidence.
- **P1 platform acceptance:** physical audio routes, Bluetooth/headset controls,
  Android lockscreen/background endurance and force-stop/restart; normal desktop
  sessions and Windows suspend/resume. Linux Xvfb launch is only a startup smoke.
  Emulator/null output tests are narrower evidence.
- **P1 integrity boundary:** manual deletion of all remote proof and absence of a
  local receipt/backup is not recoverable. Retain metadata backups; the backend
  refuses unsafe reconstruction rather than synthesizing a library.
- **P2 authorization scope:** existing-account phone/code/2FA/QR and email
  verification inputs are implemented. Registration and Premium-purchase states
  are recognized but do not have an in-app signup/purchase flow; use an existing
  Telegram account. This audit did not add a registration/payment subsystem.
- **P2 scale bounds:** snapshot limits (32 MiB compressed / 128 MiB uncompressed) and complete
  tombstone/receipt retention bound very large libraries; GC still has per-message
  verification and cumulative progress cost. Existing 1k–100k simulated benchmark
  rows in `docs/benchmarks` are not network latency or Android memory measurements.
- **P2 sharing semantics:** duplicate protection is per durable local profile/vault
  journal. Unknown sends require inspection before a deliberate new operation;
  independent devices/deleted journals cannot deduplicate against one another.
- **P2 playback recovery:** a failed output is reported and next explicit Play
  reopens it; automatic hardware route migration or position restoration is not
  claimed. Android focus/route callbacks pause independently.

## 7. Readiness decision

**RELEASE_BLOCKED**, with extensive account-free tests and native verification.
Do not label this PRODUCTION_READY. Passing Rust tests, simulation and emulator
checks does not satisfy required live-account, Windows and physical acceptance.
No destructive GC was performed outside the deterministic fake server.

## 8. Release acceptance and next steps

1. Review the current patch and its passing local gates; retain the platform
   distinctions in this report. A local debug build is not a release candidate.
2. Run the committed patch through all three required CI jobs, retaining installer
   evidence and artifact checksums. Inspect actual extracted Debian/AppImage/MSI
   and installed APK payloads, not just target directories. The new Linux CI gates
   exercise AppImage verification and the isolated audio-output failure test.
3. Follow [live/device acceptance](../src-tauri/docs/e2e.md) using explicitly
   supplied disposable accounts, vault and recipient; record expected/actual
   results for every unverified row, including lost ACK, stale client and GC.
4. Exercise physical Linux/Windows/ARM64 audio and process/background scenarios.
   Any failure becomes a concrete regression before release.
5. Only change readiness to PRODUCTION_READY when all required platform/CI/live
   gates pass and no critical correctness or security issue remains.
