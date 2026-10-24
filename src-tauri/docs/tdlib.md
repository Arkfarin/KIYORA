# Native TDLib for kiyora

The adapter pins TDLib **1.8.69**, commit
`9d92b49bc5d0d2e29fe518239af647a88f8680e6`, and rejects a different runtime version.
JSON fields are taken from that revision's
[official schema](https://github.com/tdlib/td/blob/9d92b49bc5d0d2e29fe518239af647a88f8680e6/td/generate/scheme/td_api.tl)
and [C ABI](https://github.com/tdlib/td/blob/9d92b49bc5d0d2e29fe518239af647a88f8680e6/td/telegram/td_json_client.h).
In particular `inputMessageAudio.audio` and `inputMessageDocument.document` now
contain `inputAudio`/`inputDocument` objects; older flat request examples do not apply.

On Linux install a C++17 compiler, CMake, make, OpenSSL, zlib and gperf, then from
`src-tauri` run `bash scripts/build-tdlib.sh`. Two compilation jobs are the default
to bound memory. Set `KIYORA_TDJSON` to the absolute `target/native/td-build/libtdjson.so`
path when starting kiyora. The library is dynamically loaded, so normal Rust tests
do not need it. TDLib uses the Boost Software License 1.0; keep upstream license
files with packaged native binaries.

Run `bash scripts/check-backend.sh` for formatting, workspace compilation, Clippy
and all offline tests. When `KIYORA_TDJSON` is set, the script also runs the
credential-free native lifecycle test.

The credential-free native test is explicit:

```sh
KIYORA_TDJSON="$PWD/target/native/td-build/libtdjson.so" \
  cargo test -p vault-telegram --test real_tdlib -- --ignored
```

It checks native initialization, version query, initial authorization state and
graceful close. It does **not** prove account login, history, uploads or streaming.
Authenticated tests must use a separately supplied test account/channel. Existing
personal Telegram sessions are never imported automatically.

Use [the native packaging scripts](packaging.md) for Linux Debian, Windows MSI
and Android ARM64 APK builds. They prepare pinned TDLib and native dependencies,
include licenses and verify the actual installer payload. A standalone Rust
build is not a distributable installer. Current execution evidence and remaining
platform checks are recorded in [backend readiness](../../docs/BACKEND_READINESS.md).

Exactly one receive worker owns `td_receive` in a process. Profile switching must
await `authorizationStateClosed` before opening the next client. Request and update
queues are bounded; lag forces reconciliation. Only an explicit allowlist of reads
is retried, including FLOOD_WAIT delays. Mutation timeouts retain unknown outcomes.
Native logs are disabled, and raw TDLib errors/updates never go directly to UI logs.

Linux keys use Secret Service with encrypted D-Bus transport; Windows uses Credential
Manager and macOS Keychain. No usable secure store means an explicit error, never a
plaintext key file. The Android Tauri shell injects the native Keystore provider
described in [android.md](android.md); the CLI without that provider still refuses
credentials. Session directories, SQLite and cache are per profile, and
an OS file lock prevents two processes from opening the same TDLib database.
