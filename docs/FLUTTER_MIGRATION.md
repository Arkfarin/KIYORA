# KIYORA Flutter migration

Status: **IN PROGRESS**. The complete requested scope is preserved in
[FLUTTER_MIGRATION_REQUIREMENTS.md](FLUTTER_MIGRATION_REQUIREMENTS.md). The existing
Tauri client remains available until the new client passes functional acceptance.
Neither application replacement nor release publication is authorized by this work.

## Architecture map

| Responsibility | Existing implementation | Flutter boundary |
| --- | --- | --- |
| Process/profile ownership | `vault-core::Backend`, `vault-security::ProfileStore`, exclusive profile lease | One process singleton; generation ID fences old clients; explicit shutdown |
| Telegram authorization and native loading | `vault-telegram`, pinned TDLib 1.8.69; `vault-core::native_location` | Typed auth states and inputs; same credential storage and native lookup |
| Durable library | `vault-db` SQLite projection, `vault-domain` reducers, `vault-protocol` records | Bounded typed pages; protocol models stay in Rust |
| Live/catch-up sync | `vault-sync`, `vault-snapshot`, recovery journals in `vault-core` | Existing service; typed invalidation events; no Dart reducer |
| Queue and playback | `vault-core::backend::queue`, `vault-media` Rodio/CPAL/Symphonia/Opus | Existing queue ID/revision and engine; no Dart audio player |
| Events | Sequenced `EventBus`, bounded reliable subscription | Demand-driven asynchronous reads; explicit loss marker and cancellation |
| Android host | Tauri native plugin: Keystore, JNI, documents, MediaSession/service | Adaptation still required; Flutter bridge currently fails closed on Android |
| Desktop media | Tauri plugin's Souvlaki adapter: Linux MPRIS, Windows SMTC/ HWND | Extraction of the host-independent adapter still required |
| Presentation | Existing React client is only a feature reference | `flutter/`: native Flutter UI and generated FRB API; implementation ongoing |

Dependencies point from Flutter presentation to application state, typed generated
FRB API, `kiyora-flutter-bridge`, and the existing Rust backend. The new crate has
no Tauri dependency. It intentionally exposes a limited set of value models
instead of generating bindings for the complete internal workspace.

## Bridge and lifecycle

`src-tauri/crates/kiyora-flutter-bridge/src/api` contains the reviewed public API.
`runtime.rs` owns the process singleton. Repeated initialization with the same
options reuses one generation. Different options require shutdown first. Ordinary
widget or Activity recreation must never call shutdown. Commands hold a read
lease; shutdown cancels child operations and waits for outstanding leases before
closing the backend. An old generation cannot operate on a replacement runtime.

Long calls accept a request UUID. Concurrent reuse is rejected, and cancellation
is separate from retry. Durable mutation IDs remain the backend's responsibility;
cancellation never asserts that a remote send was undone.

Subscribe with `open_events` before reading state. `next_event` permits one
pending read per subscription and keeps the bounded Rust broadcast backlog in
Rust. This avoids silently transferring an unbounded backlog into a Dart stream.
`close_events` interrupts a pending read. `event_watermark` is a **lower** bound:
read it before fetching state, then treat subsequent events as idempotent
invalidations, including overlapping changes. A resync marker requires rereading
all relevant state. It is not an atomic cross-query snapshot. Dart consumption and
reconnection logic remain to be implemented and tested.

## Data compatibility

The desktop default continues using
`ProjectDirs::from("io.github", "arkfarin", "kiyora")`, the original profile IDs,
`active-profile`, `backend.json`, `vault.sqlite`, TDLib databases, cache,
playback checkpoints, and secure storage names. There is no protocol or SQLite
schema migration. `KIYORA_DATA_DIR` remains available for an explicit isolated
launch. Integration tests create temporary profiles and never open the user's
personal Telegram session.

Android must preserve the existing package ID, signing certificate, Keystore
aliases and app-private paths. These have **not** yet been accepted for Flutter.
The bridge refuses Android initialization until the native adapters are connected;
plaintext secrets or an alternative Dart player are not fallback implementations.
A new APK must not replace an existing installation until an actual in-place
upgrade/data-preservation test passes. If identity cannot be preserved, the
explicit transfer path required by the specification must be implemented first.

## Toolchain and generation

Selected versions: Flutter **3.47.9** (Dart **3.13.7**), FRB runtime/codegen
**2.13.0**, existing Rust workspace MSRV **1.88** (local Rust **1.99.1**).
FRB versions must match exactly. The SDK version and checksum were read from the
[official release archive](https://docs.flutter.dev/install/archive); FRB from
[the published package](https://pub.dev/packages/flutter_rust_bridge/versions).

The local Flutter SDK is being installed outside the repository. Its Linux x64
archive SHA-256 is
`25ed8c3c29d51a7353f2a944310b478af8214920639524b19c50a19e6c044c5a`.

Intended generation command, from `flutter/` after SDK setup and `flutter pub get`:

```sh
cargo install flutter_rust_bridge_codegen --version 2.13.0 --locked
flutter_rust_bridge_codegen generate
```

The configuration is `flutter/flutter_rust_bridge.yaml`. Generation, native
packaging and Flutter runner setup are still being completed; this is not yet a
verified clean-checkout Flutter build procedure.

Rust verification, from `src-tauri/`:

```sh
cargo test --locked -p kiyora-flutter-bridge
cargo clippy --locked -p kiyora-flutter-bridge --all-targets -- -D warnings
```

## Platform builds still to complete

- Linux: Flutter runner/CMake, bundling the bridge and pinned TDLib/licenses,
  native dialogs/MPRIS, release bundle, Debian/AppImage validation and GUI startup.
- Windows: Flutter runner/CMake, bridge DLL and TDLib bundle, SMTC HWND ownership,
  MSI/MSIX packaging and installed startup. Windows execution is not available on
  this Linux host.
- Android: Flutter runner, Kotlin/JNI/Keystore/service/document adapters, existing
  signing identity, ARM64 and x86_64 native builds, APK/AAB dependency/ABI/16 KiB
  verification, emulator install and physical-device acceptance.

Existing native preparation scripts under `src-tauri/scripts` remain the source
of pinned TDLib/OpenSSL/zlib builds. Existing `backend.yml` checks the Tauri
application and must not be treated as Flutter CI evidence.
