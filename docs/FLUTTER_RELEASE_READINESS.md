# Flutter release readiness — NOT READY

Updated 2026-10-30. This is a migration progress record, not release approval.
Full scope: [requirements](FLUTTER_MIGRATION_REQUIREMENTS.md).

## Evidence for the current increment

- `cargo test -p kiyora-flutter-bridge`: four automated tests passed, one
  explicit real-output test ignored in the default gate.
- Verified process singleton reuse, conflicting initialization options, profile
  persistence, restored active session, stale-generation rejection, explicit
  subscription close, event-loss reporting and delivery resumption.
- Verified SQLite paging through **10,001 tracks** and searching for the last
  track without a 500-result frontend cutoff. This is not a Flutter frame-time or
  memory benchmark.
- Real Rust playback bridge test: **PASS** using generated silence and ALSA null
  output. Verified Play/Pause, seek to 30 seconds, Next and rejection of an old
  queue revision after Repeat changed. This proves actual decoding/engine control,
  not audible hardware output.
- Strict bridge all-target Clippy with `-D warnings`: **PASS**. Rust formatting: **PASS**.
- Operation request exclusivity, cancellation on drop and parent-lifetime
  cancellation: **PASS**.
- Existing `vault-core --test profiles`: **9 passed**, including corrupt SQLite,
  profile-switch fencing, job draining and queue checkpoint restoration.
- Existing pinned native TDLib start/version/close/reopen test: **PASS** with
  the prepared Linux library, without Telegram credentials or account access.
- Direct generated Dart calls, native Flutter builds and screenshots are not yet
  verified. The Linux Flutter SDK download is in progress outside the repository.

## Native dependencies and signing

FRB runtime/codegen pinned to 2.13.0. Existing TDLib 1.8.69, static OpenSSL/zlib
and bundled Opus preparation remain unchanged. New Flutter payload verification
is pending. No private signing material has been read into reports or added to
version control. Android package/signing/Keystore/data continuity remains an
explicit gate; no claim of in-place APK compatibility is made.

## Platform evidence

| Target | Flutter build | Native playback/session | Install/upgrade |
| --- | --- | --- | --- |
| Linux x86_64 | Pending SDK/runner/build | Rust bridge audio test passed; Flutter MPRIS pending | Pending |
| Windows x64 | Not run | SMTC adaptation and Windows execution pending | Installer pending |
| Android ARM64 | Not run | Kotlin/Keystore/foreground service adaptation pending | APK and legacy upgrade pending |
| Android x86_64 | Not run | Emulator scenario pending | APK/install pending |

Existing Rust/Tauri acceptance reports do not prove any of these Flutter gates.

## Required completion gates

1. Direct generated Flutter/Rust integration and real output.
2. All specified screens/actions, no production mocks or inert controls.
3. Real test-account Telegram auth, Vault discovery/creation/sync, import and
   recovery. No personal session is used automatically.
4. Android background notification, lockscreen, media keys, focus, route/headset,
   Bluetooth, seek and Activity recreation with the original Rust engine.
5. Desktop media keys, MPRIS/SMTC, resize/keyboard, native loader and installed startup.
6. Proven preservation of profiles, sessions, keys, database, music/cache, queue,
   metadata/artwork/lyrics, playlists and recovery journals.
7. Flutter widget/state/navigation/golden/integration/accessibility tests;
   explicit separation of automated, emulator and physical-device evidence.
8. Spotify composition review using actual Flutter screenshots, KIYORA themes,
   responsive states, gestures and reduced motion.
9. Pinned Windows/Linux/Android CI builds with bundled native dependencies,
   actual installers, ABI and 16 KiB verification; no publication without approval.

A release must remain unapproved until these gates have authoritative evidence.
