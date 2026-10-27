# QR authorization implementation report

Implemented in the existing TDLib/Backend/profile architecture. The CLI now
supports `auth qr`, live QR replacement, hidden 2FA and other existing interactive
challenges, `/qr` from phone/code/email prompts, `/resend`, `/cancel`, Ctrl+C and
SIGINT. Account details are fetched only after Ready and owner validation.

The existing profile lease, encrypted native database, OS secret storage and
session activation remain authoritative. QR links are transient UI data, never
written to Kiyora's SQLite/config files or diagnostics. TDLib manages its own
internal encrypted authorization state. No parallel Telegram client or MTProto
implementation was added. No music-library, snapshot, GC, playback or frontend
implementation was changed.

## Files

Paths below are relative to `src-tauri`.

| Files | Change |
| --- | --- |
| `Cargo.lock`, `crates/vault-core/Cargo.toml` | `qrcode`, `crossterm`, async event stream; test-only `rqrr` decoder; replace blocking password reader. |
| `crates/vault-telegram/src/auth.rs` | QR availability, safe Debug, typed latest-state subscription, credential validation, cancellable auth. |
| `crates/vault-telegram/src/client.rs` | Auth/connection subscription; native auth query gate that survives caller cancellation/timeouts. |
| `crates/vault-telegram/src/lib.rs` | `AuthBusy` error and safe known-error classification. |
| `crates/vault-core/src/backend.rs` | Expose auth subscription/account identity; cancellable auth outside session mutex with session/epoch fencing. |
| `crates/vault-core/src/api.rs` | Map `AuthBusy` to existing `Conflict`. |
| `crates/vault-core/src/bin/kiyora-cli.rs` | Command/help/dispatch, signal cancellation, redact QR status JSON, surface close failures. |
| `crates/vault-core/src/bin/kiyora-cli/auth_ui.rs` | QR fallback guidance; adapt existing phone recovery regression. |
| `crates/vault-core/src/bin/kiyora-cli/auth_flow.rs` (new) | Shared phone/QR interactive state machine. |
| `crates/vault-core/src/bin/kiyora-cli/auth_terminal.rs` (new) | Local QR renderer, sizing, clearing, hidden cancellable input and QR decoding test. |
| `crates/vault-core/src/bin/kiyora-cli/auth_flow_tests.rs` (new) | Backend/UI flow tests and isolated PTY child. |
| `crates/vault-telegram/tests/auth.rs` | QR eligibility/redaction and concurrent-query regression. |
| `crates/vault-telegram/tests/auth_fixture.c` | Existing ABI double extended with rotating QR, connection/confirmation controls, query counts and Ready-only getMe. |
| `crates/vault-telegram/tests/qr_auth.rs` (new) | Native adapter QR lifecycle, pending-query cancellation and session persistence regression. |
| `crates/vault-core/tests/auth_recovery.rs` | Valid hexadecimal fixture credentials and explicit API-hash/link log-redaction checks. |
| `crates/vault-core/tests/cli.rs` | Help, missing profile and missing/invalid credential command checks. |
| `crates/vault-core/tests/qr_terminal_pty.py` (new) | Linux PTY rotation, Ctrl+C/SIGINT, hidden password, process exclusion, restart and dumb-terminal checks. |
| `docs/cli-auth.md` (new), `docs/tdlib.md`, `docs/e2e.md` | CLI usage, shared APIs, failure recovery and real Android scanning procedure. |
| `docs/qr-auth-report.md` (new) | This report and coverage/verification record. |

## Public interfaces

- Existing `AuthInput::Qr`, `AuthStatus::QrConfirmation` and
  `Backend::authenticate` remain the QR request path. Authentication now rejects
  concurrent mutations instead of queueing them, and observes cancellation while
  waiting. A cancelled native query retains its gate until reply/close.
- `AuthStatus::can_request_qr()`, `valid_api_credentials()`.
- `TdClient::auth_updates()` → `AuthUpdates`; `current()` → `AuthUpdate`
  (`status`, `connected`); `changed(cancel)` waits for an update/cancellation.
  Values coalesce so slow consumers do not replay old QR links.
- `TdClient::authenticate_cancellable(input, cancel)`;
  `TelegramError::AuthBusy` → existing core `ErrorCode::Conflict`.
- `Backend::auth_updates(session_id)` and
  `Backend::authorized_account(session_id, cancel)` → `AccountInfo`
  (`user_id`, `display_name`). Existing GUI auth events remain available.
- `auth status --json` intentionally omits the link in `QR_CONFIRMATION`.
  In-process auth state and UI events still expose it for rendering.

## Automated coverage

All fixtures use temporary directories and fake account data. Ordinary tests do
not require a Telegram account, network credentials or installed TDLib. Linux
ABI/PTY regressions require `cc` and Python 3; other platforms skip Linux-only
native fixture tests. The decoder checks both full/half-block output against the
exact TDLib URL, including punctuation, and checks the four-module quiet zone.

| Requested scenario | Evidence |
| --- | --- |
| 1. Start QR | `qr_auth.rs`: one native request; CLI flow uses same Backend input. |
| 2. First QR | `qr_auth.rs`, `auth_flow_tests.rs`: exact first link delivered/rendered. |
| 3. Rotation | Native update changes link without another request; UI clears before replacement; PTY checks screen clearing. |
| 4. Device confirmation | Mock update transitions directly to Ready in `qr_auth.rs`. |
| 5. 2FA | Mock update transitions to PasswordRequired; UI announces scan and prompts. |
| 6. Success after 2FA | Wrong password recovery, then Ready/account summary; PTY verifies no password echo. |
| 7. Ctrl+C | Real PTY key and SIGINT; cancellation token and `/cancel`; terminal mode restored; native query gate survives cancelled caller. |
| 8. Connection loss | Native connection update; UI clears/hides QR. |
| 9. Reconnection | UI waits for a fresh link; no old QR redisplay or synthetic token request. |
| 10. Unexpected state | Adapter rejects invalid QR start; UI exits with safe guidance. |
| 11. Already authorized | Repeat QR flow after fixture process restart reports authorized; no new QR request. |
| 12. Phone → QR | `/qr` at actual code challenge uses existing session; old phone/resend tests still pass. |
| 13. No active profile | Actual CLI command returns NoProfile. |
| 14. Credentials | Missing, malformed, nonpositive ID and nonhex hash rejected before native startup. |
| 15. Competing auth | Simultaneous requests, cancelled pending query, second Backend lease and second PTY process all reject conflicts. |
| 16. Session persistence | Adapter close/reopen, backend restore and independent PTY processes preserve completed authorization; recovery test keeps existing session marker. |

## Verification on Linux

Executed successfully:

```sh
cargo fmt --all -- --check
cargo check --locked -p vault-core --bin kiyora-cli
cargo test --locked -p vault-telegram
cargo test --locked -p vault-core
cargo clippy --locked -p vault-core --all-targets -- -D warnings
cargo clippy --locked -p vault-telegram --all-targets -- -D warnings
```

`vault-telegram`: 22 passed; one opt-in native test excluded from the default run.
`vault-core`: 71 passed, including the PTY driver and decoder test.
`kiyora-cli --help` lists `auth qr`. `git diff --check` passes.

Also explicitly executed the opt-in native test against the locally installed
pinned TDLib 1.8.69:

```sh
KIYORA_TDJSON="$PWD/target/native/td-build/libtdjson.so.1.8.69" \
  cargo test --locked -p vault-telegram --test real_tdlib -- --ignored
```

Result: 1 passed. This verifies native startup/version/close/reopen, without API
credentials or a Telegram account. It does not verify live QR scanning.

## Manual verification and limits

Follow [the Android QR acceptance procedure](e2e.md#qr-authorization-acceptance):
create/switch a test profile, run `auth qr`, scan from Telegram → Settings → Devices
→ Link Desktop Device, complete 2FA, restart CLI, check `auth status`, then
`vault discover`. API credentials and the pinned native library must be configured.

**UNVERIFIED:** actual Telegram server login, physical phone scanning, real-account
session restoration, individual Kitty/Konsole/Alacritty rendering, and Windows
terminal execution. Linux PTY/native checks cannot substitute for those tests.
The implementation exists and was exercised through the ABI double; no real
account login is claimed.

With `TERM=dumb`, old QR output is scrolled out and marked invalid; scrollback
cannot be erased without terminal control. Use a light foreground/dark background
in that fallback. QR rendering refuses redirected stdout. Too-small windows must
be enlarged. Reconnection waits for TDLib to issue a fresh link. Unsupported
registration/Premium steps retain the profile and direct the user to the official
app. Cancellation during native initialization waits for startup to finish before
closing, bounded by the native request timeout.
