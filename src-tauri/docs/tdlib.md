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

Vault discovery uses a scoped read policy without automatic retries: rate limits
are returned with their retry delay, and every candidate and operation has a
deadline. See [ordinary and deep discovery](vault-discovery.md) for the exact
TDLib methods, limits and recovery commands.

### Authorization and diagnostics

`kiyora-cli auth qr` provides interactive terminal QR login, live token updates,
hidden 2FA input and Ctrl+C cancellation. See [CLI authentication](cli-auth.md)
for Android scanning, profile restoration, terminal fallback and error guidance.
The flow uses the pinned schema's `requestQrCodeAuthentication` and
`authorizationStateWaitOtherDeviceConfirmation`, through the existing Backend.

`kiyora-cli auth login` resumes the authorization state in the selected profile's
TDLib database. A normal CLI exit, an error, or EOF closes the native client;
it does not log out or delete the profile/session. Run the same command again to
continue. TDLib can expire an unfinished login and ask for the phone again.

When a code is required, the CLI identifies its delivery method (for example,
a message in another active Telegram session, SMS, a call, or Fragment), expected
length when known, and the next method and delay offered by Telegram. Enter
`/resend` at the code prompt or run `kiyora-cli auth resend`. Each explicit resend
re-reads `getAuthorizationState`, then calls `resendAuthenticationCode` with
`resendCodeReasonUserRequest` only if `next_type` is present and `timeout` is zero.
Email code resends use the same documented method. Kiyora never forces SMS or
automatically retries an auth mutation. FloodWait delays block further auth
attempts in the running client; Telegram remains authoritative across restarts.

`auth status --json` includes `code_info.delivery`, `expected_length`,
`timeout_seconds` and `next_delivery` in `CODE_REQUIRED`. The timeout is a snapshot
of TDLib's remaining delay; resending always checks it again. Unknown lengths and
missing metadata are represented by null. Numbers, code hints, Firebase receipts
and delivery URLs are excluded from this metadata.

Errors distinguish `INVALID_CODE`, `CODE_EXPIRED`, `INVALID_PASSWORD`,
`TELEGRAM_REJECTED`, `NETWORK`, `TIMEOUT`, `RATE_LIMITED`, `RESEND_TOO_EARLY` and
`RESEND_UNAVAILABLE`. Delay errors include `retry_after_seconds`. Incorrect input
can be corrected in the interactive login; network failures and FloodWait exit
with guidance to resume later. Codes, phone/email input and passwords are hidden
when entered from a terminal. Avoid putting credentials in command arguments.

Set `KIYORA_DIAGNOSTICS=1` to enable JSON diagnostics on stderr. The `kiyora::auth`
target records allowlisted state/operation names, delivery metadata, connection
state, elapsed milliseconds, safe error categories, numeric rejection codes and
retry delays. It never records request/response bodies, phone numbers, codes,
passwords, API hash, encryption keys, QR tokens or TDLib database contents.

Offline regression tests use a tdjson ABI double and temporary profile/session
directories, covering all auth states, resend restrictions, concurrent attempts,
invalid/expired codes, FloodWait, diagnostic redaction and interrupted login:

```sh
cargo test --locked -p vault-telegram --test auth --test qr_auth --lib
cargo test --locked -p vault-core --test auth_recovery --bin kiyora-cli
```

The ABI fixture tests execute on Linux; portable state/error tests run on every
target. These tests do not exercise an actual Telegram account or prove live
code delivery on Linux, Windows or Android.

Linux keys use Secret Service with encrypted D-Bus transport; Windows uses Credential
Manager and macOS Keychain. No usable secure store means an explicit error, never a
plaintext key file. The Android Tauri shell injects the native Keystore provider
described in [android.md](android.md); the CLI without that provider still refuses
credentials. Session directories, SQLite and cache are per profile, and
an OS file lock prevents two processes from opening the same TDLib database.
