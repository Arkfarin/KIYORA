# Kiyora

Kiyora is a Tauri application with a Rust backend for a personal Telegram music
vault. The active Rust workspace is in `src-tauri`.

Run `bash src-tauri/scripts/check-backend.sh` for the offline backend checks.
Use the [native packaging instructions](src-tauri/docs/packaging.md) to build
installers with TDLib for Linux, Windows and Android. A standalone Cargo build
does not bundle TDLib. Current verification and remaining work are recorded in
[backend readiness](docs/BACKEND_READINESS.md) and the
[completion report](docs/BACKEND_COMPLETION_REPORT.md). Readiness remains
`RELEASE_BLOCKED` until Windows, physical-device and live-account acceptance pass.

Use `kiyora-cli vault discover` for bounded lookup of a saved or named vault,
or `kiyora-cli vault discover --deep` to recover renamed/legacy channels.
See [Vault discovery](src-tauri/docs/vault-discovery.md) for limits, JSON output,
cancellation and manual attachment.

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
