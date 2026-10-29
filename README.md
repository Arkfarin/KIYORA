# Kiyora

Kiyora is a Tauri application with a Rust backend for a personal Telegram music
vault. The active Rust workspace is in `src-tauri`.

The React interface includes profile-aware setup, a responsive library/player,
imports, metadata/artwork/lyrics editing, playlists and offline cache controls.
See [music UI implementation and acceptance](docs/MUSIC_UI_ACCEPTANCE.md) for
current behavior, compatibility constraints, checks and Linux/Android commands.

Run `pnpm build` for the production frontend and `pnpm test:ui` for isolated
Chromium component checks. Test fixtures are not included in the application.

Run `bash src-tauri/scripts/check-backend.sh` for the offline backend checks.
Use the [native packaging instructions](src-tauri/docs/packaging.md) to build
installers with TDLib for Linux, Windows and Android. A standalone Cargo build
does not bundle TDLib. Current verification and remaining work are recorded in
[backend readiness](docs/BACKEND_READINESS.md) and the
[completion report](docs/BACKEND_COMPLETION_REPORT.md). Readiness remains
`RELEASE_BLOCKED` until Windows, physical-device and live-account acceptance pass.

For Android, run `pnpm android:doctor` to check the local toolchain, then
`pnpm android:apk` to build a signed ARM64 debug APK with bundled TDLib.
The APK is written to
`src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk`.
`pnpm android:apk:release` builds a signed release APK using the local private key; see
[Android setup and signing](src-tauri/docs/packaging.md#android-arm64).

Use `kiyora-cli vault discover` for bounded lookup of a saved or named vault,
or `kiyora-cli vault discover --deep` to recover renamed/legacy channels.
See [Vault discovery](src-tauri/docs/vault-discovery.md) for limits, JSON output,
cancellation and manual attachment.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
