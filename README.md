# Kiyora

Kiyora is a Tauri application with a Rust backend for a personal Telegram music
vault. The active Rust workspace is in `src-tauri`.

Run `bash src-tauri/scripts/check-backend.sh` for the offline backend checks.
Use the [native packaging instructions](src-tauri/docs/packaging.md) to build
installers with TDLib for Linux, Windows and Android. A standalone Cargo build
does not bundle TDLib. Current verification and remaining work are recorded in
[backend readiness](docs/BACKEND_READINESS.md).

## Recommended IDE Setup

- [VS Code](https://code.visualstudio.com/) + [Tauri](https://marketplace.visualstudio.com/items?itemName=tauri-apps.tauri-vscode) + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer)
