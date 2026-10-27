# Telegram authentication in Kiyora CLI

Build with `cargo build --locked -p vault-core --bin kiyora-cli` from `src-tauri`.
Install the pinned [TDLib and OS secret storage](tdlib.md) first. The library is
loaded via the existing `KIYORA_TDJSON`/packaging discovery mechanism.

Supply `KIYORA_API_ID` and `KIYORA_API_HASH` from your Telegram application through
your environment or secret manager. The ID must be positive; the hash must be
32 hexadecimal characters. Never put credentials into CLI arguments or logs.
Saved profiles restore their API hash and database key through OS secure storage;
you do not need these environment variables again after successful setup.

```sh
kiyora-cli profiles create test
kiyora-cli profiles switch <uuid>
kiyora-cli auth qr
kiyora-cli auth status
kiyora-cli vault discover
```

Use the UUID printed by `profiles create`. `auth qr` restores this profile and
uses its existing TDLib database. If already authorized, it reports the account
and profile without requesting another QR. No profile or Telegram session is
reset by a QR error or ordinary cancellation.

On Android, open the **already authorized Telegram app → Settings → Devices →
Link Desktop Device**, scan the QR displayed in the terminal, and confirm the
login. A cloud password, if required, is entered **in Kiyora** without echo.
Scanning alone is not success: wait for “Telegram authorization successful!” and
the account/user/profile summary. `auth status --json` then reports `READY`.

TDLib supplies the exact link encoded locally by the `qrcode` crate. Changes to
`authorizationStateWaitOtherDeviceConfirmation` replace the terminal QR; Kiyora
never manufactures a token or refreshes one on its own timer. On disconnection,
the QR is hidden. After reconnection, Kiyora waits for a new link from TDLib.

The renderer uses UTF-8 blocks, a four-module quiet zone and terminal dimensions.
It selects full or half blocks to fit, redraws on resize, and asks you to enlarge
an undersized window instead of wrapping/cropping the code. ANSI terminals
(including Kitty, Konsole and Alacritty) get explicit black/white colors and an
alternate screen erased on refresh and exit. With `TERM=dumb` or no `TERM`, plain
Unicode output pushes the old frame out of the viewport and marks it invalid.
This fallback cannot erase scrollback; scan only the newest frame, and use a
light foreground on a dark background. Redirected stdout is refused for QR
rendering so the credential image does not become a log file. No browser or
external QR service is used.

## Phone login and cancellation

`kiyora-cli auth login` still accepts phone, code, email and 2FA challenges.
At the phone/code/email prompts, `/qr` switches to QR when the current TDLib state
allows it. `/resend` retains Telegram's delivery/delay checks. If no alternate
code delivery is available, use `/qr` with a logged-in phone; Kiyora cannot force
SMS delivery. Enter `/cancel` or press Ctrl+C to leave any challenge or QR wait.
Input is hidden, including these commands; press Enter after a slash command.

Ctrl+C also interrupts a pending auth request. Kiyora then calls TDLib `close`
and waits for `authorizationStateClosed` before releasing the profile lease.
A signal during initial database opening is latched until startup completes,
then the client is closed. This can take the native request timeout. Cancellation
never calls `logOut` or `destroy`. Use `auth logout` only to intentionally revoke
the session. A resumed `PasswordRequired` challenge continues 2FA directly.

## Troubleshooting

| Result | Action |
| --- | --- |
| `NoProfile` | Create and switch to a profile first. |
| `InvalidInput` during setup | Check that API ID/hash are present and correctly formatted. Their values are never echoed. |
| `NativeUnavailable` | Install the pinned TDLib 1.8.69 library; check `KIYORA_TDJSON`. |
| `SecureStorageUnavailable` | Unlock/start the OS keyring. No plaintext fallback is provided. |
| `ProfileBusy` | Close the CLI or GUI currently using this profile, then retry. |
| `Conflict` during auth | An auth query may still be pending, or account/profile ownership differs. Wait/close cleanly and inspect `auth status`; do not delete the session. |
| `AuthStateMismatch` | The challenge changed; follow the current prompt. QR cannot be requested while already awaiting QR confirmation, ready, closing, or in an unknown state. |
| `ResendUnavailable` | Telegram offers no other code delivery method. Enter `/qr` when available. |
| `InvalidPassword` | Correct the cloud password at the hidden 2FA prompt. |
| `Network`, `Timeout`, `RateLimited` | No mutation is retried automatically. Honor the displayed delay and resume `auth qr` or `auth login` later. |
| Registration/Premium/unknown step | Follow the explanation in the terminal; complete unsupported steps in the official Telegram app. The saved profile remains intact. |

`auth status --json` omits the QR link. Diagnostic events and Rust `Debug` output
also redact it. The backend's typed auth stream and existing UI auth events carry
the ephemeral link for rendering; consumers must never persist or log it.

## Shared backend API

QR requests still use `Backend::authenticate(session_id, AuthInput::Qr, cancel)`.
`Backend::auth_updates(session_id)` returns `AuthUpdates` with `current()` and
`changed(cancel)` for the latest authorization/connection state. Slow consumers
coalesce updates rather than replaying obsolete tokens. No raw TDLib requests
are made by the CLI. Existing `auth.state.changed` events remain available.

`Backend::authorized_account(session_id, cancel)` returns `AccountInfo`
(`user_id`, `display_name`) only after `Ready` and profile owner validation.
`AuthStatus::can_request_qr()` exposes state eligibility. `TdClient` adds
`auth_updates()` and `authenticate_cancellable()`; `AuthBusy` maps to the existing
API `Conflict`. A native query stays fenced after caller cancellation/timeout
until its response or session closure. Process exclusion remains the existing
`ProfileLease` OS lock; session secrets remain in the existing OS secret store.

See [manual acceptance](e2e.md#qr-authorization-acceptance) for real-account checks.
