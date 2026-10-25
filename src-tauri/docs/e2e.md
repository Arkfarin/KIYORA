# Optional live-account and device acceptance

Live account and physical-device checks are opt-in and are separate from
account-free CI. Emulator/native results and outstanding live checks are recorded
in [the completion report](../../docs/BACKEND_COMPLETION_REPORT.md).
Use dedicated Telegram accounts, a disposable private test vault and a test
recipient. Do not reuse personal session files. Use generated tone fixtures from
`spec/test-vectors/audio`, and record build revision, platform, app/TDLib versions,
test case and expected/actual result. Keep phone numbers, codes, API hash, session
paths and raw TDLib logs out of artifacts.

## Desktop account setup

Build an installer using [packaging.md](packaging.md), verify it and install it on
the target OS. Build the diagnostic CLI with `cargo build --locked -p vault-core
--bin kiyora-cli`. From `src-tauri`, an isolated Linux CLI acceptance directory can
be selected as follows (PowerShell uses the equivalent `$env:` variables):

```sh
export KIYORA_DATA_DIR="$(mktemp -d -t kiyora-e2e-XXXXXX)"
export KIYORA_TDJSON="$PWD/target/native/desktop/native/libtdjson.so"
cli="$PWD/target/debug/kiyora-cli"
"$cli" profiles create e2e
"$cli" profiles list --json
"$cli" profiles switch <profile-uuid>
```

Provide `KIYORA_API_ID` and `KIYORA_API_HASH` through a private environment/secret
provider, then run `"$cli" auth login`. Values must not be embedded in command
history or source files. An unavailable keyring must produce a structured error;
do not substitute plaintext storage. Check wrong/expired code and wrong 2FA
password, cancel the attempt, then complete login. Exit and reopen the CLI and
verify `auth status --json` without another login. Test QR login through
`kiyora-cli serve` using the `authenticate` variant in the generated API schema.

Run `vault discover`, `vault create`, `status --json` and `diagnose`. Capture only
redacted application results. Create a second isolated directory/profile on a
second device, log in to the same **test account**, discover and attach the test
vault. Wait for `online_ready` on both clients before mutations.

## Account data and recovery

| Test | Actions | Required result |
| --- | --- | --- |
| Import formats | `media import` each audio fixture; inspect `library list` and Telegram | Title/duration/source correct, Telegram audio metadata present, playback decodes |
| Duplicate file | Import the same bytes under a second filename | Exact duplicate is reported; no second audio upload |
| Incoming audio | `media import-incoming <chat> <message>` for a test audio with cover | Server copy has a new message ID; audio isn't downloaded/reuploaded, cover is fetched separately |
| Protected content | Repeat from a protected test chat | Explicit refusal, no bypass upload |
| Sharing | `library share <track> <source> <recipient> --op-id <uuid>` twice | Recipient receives one copy; journal has final ID only after ACK |
| Ambiguous sharing | Disconnect after send; inspect `get_share_status` through `serve` | `unknown` is retained unless final ACK was observed; replay doesn't send again |
| Offline | Disconnect network; query library/playlists and play a fully cached track | Reads/playback work; cloud writes, shares and new remote downloads fail |
| Reconnect | Restore network after a second device changed a playlist | Reconciliation completes before mutations; both projections converge |
| Concurrent edits | Rename/delete a playlist while the other client changes its entries | Deterministic convergence; deleted playlist stays deleted |
| Snapshot | `snapshot create`, then `snapshot verify`; cold attach a third profile | Both copies are read back and verified; complete state restored |
| Corrupt A | In the disposable vault only, remove/corrupt one snapshot copy | B restores full state; repair publishes a new verified generation |
| Interrupted snapshot | Stop the process before commit, restart | Uncommitted copy doesn't become active; history remains recoverable |
| GC | `gc plan`, inspect exact digest, `gc execute <plan> <digest>` in the test vault only | Live audio/artwork and current snapshot copies retained |
| Interrupted GC | Stop after one deletion batch, restart and inspect/resume | Persisted plan resumes conservatively; no blind new deletion plan |
| Second coordinator | Transfer coordinator while old GC is interrupted | Old plan refuses further destructive work after revalidation |
| Long-absent client | Keep a disconnected profile, delete playlist, snapshot/GC, reconnect it | Tombstone survives; deleted UUID doesn't return |
| Local damage | With app closed, damage a copy of the disposable SQLite DB, run `repair` | Original main/WAL/SHM are quarantined; verified snapshot+tail restore state |
| Lost ACK | Drop transport during import/mutation; restart and resume same `op_id` | Confirmed server result is discovered; no blind resend |
| Profile switch | Switch while import/download/playback is active | Old work is canceled/fenced and can't affect the new profile |
| Logout | Log out then restart | Session revoked, no stale writer; local reads obey selected profile |

Fault timing and the three-year-old checkpoint case are deterministic in the
simulator tests; real network toggles alone cannot force an exact ACK boundary.
Use the simulator regression as evidence of that boundary and label manual
network acceptance separately. Do not claim a live case ran if timing was not
observed. Full-backup export/restore and deliberate disk-full tests must use a
separate test filesystem, not the user's home volume.

## Installed Android checks

Select an emulator or test device explicitly with `adb -s <serial>`. Install the
debug app APK produced by `scripts/package-android.sh --debug`, then build the
instrumentation APK from the already built Tauri project:

```sh
gen/android/gradlew --project-dir gen/android \
  :app:assembleUniversalDebugAndroidTest \
  -PabiList=arm64-v8a -ParchList=arm64 -PtargetList=aarch64 \
  -x :app:rustBuildArm64Debug
adb -s <serial> install -r gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
adb -s <serial> install -r gen/android/app/build/outputs/apk/androidTest/universal/debug/app-universal-debug-androidTest.apk
adb -s <serial> shell am instrument -w \
  -e waitForActivitiesToComplete false \
  -e class io.github.arkfarin.kiyora.NativePackagingTest \
  io.github.arkfarin.kiyora.test/androidx.test.runner.AndroidJUnitRunner
```

For x86_64 use `abiList=x86_64`, `archList=x86_64`, `targetList=x86_64` and exclude
`:app:rustBuildX86_64Debug`. This test checks actual package digest/extraction,
Android TDLib loading, Activity/WebView startup and a Rust IPC response. A passing
runner must report `OK (1 test)`, not merely exit code zero. Run the native plugin
instrumentation separately with `KIYORA_ANDROID_DEVICE_TESTS=1`, an explicit
`KIYORA_ANDROID_SERIAL`, and `scripts/check-android-plugin.sh`.

Alternatively, `KIYORA_ANDROID_SERIAL=<serial> KIYORA_ANDROID_ABI=x86_64 bash
scripts/check-android-app.sh` builds the instrumentation, installs both APKs and
requires a successful result. Set `KIYORA_ANDROID_APK` for a separately saved
debug APK. The explicit runner argument is required: Tauri exits its process
when the last Activity is destroyed. The test stops the Activity and lets the
instrumentation runner report results before Android ends the process. This
avoids a teardown/reporting race; it does not suppress application exceptions
or replace any digest, loader, WebView or IPC assertion.

On physical ARM64 hardware, use `kiyora-cli serve`'s equivalent Tauri commands via
the existing API integration to execute the account cases above. Then play a
cached track and a remote FLAC/Opus track, rapidly seek beyond the buffered region,
pause/resume and advance the queue. Lock the screen for at least 15 minutes;
verify foreground notification/MediaSession and continued audible playback.
Exercise headset/Bluetooth controls, route disconnection, a competing audio app,
background notification resume, document picker import and process restart.
Focus/route loss must pause; resumption must be explicit and use the current owner.
Closing playback must stop the service and release focus/wake lock. Swapping
profiles must invalidate old notification actions. Android 15+ needs an active
foreground service before background audio focus is acquired.

## Desktop audio and failure checks

On installed Linux and Windows apps, repeat local/remote playback, seek, queue,
pause/resume, missing remote audio, track/playlist deletion and close-while-playing.
Disconnect Bluetooth, switch the default output, suspend/resume Windows, and
restart the test user's PipeWire output on Linux. Verify position preservation,
structured errors and recovery without blocking library reads or leaking audio
workers. Capture measured memory/request counts for a large FLAC and rapid seek.
Decoder tests do not prove these hardware behaviors. Final readiness must mark
each unexecuted physical/account check `UNVERIFIED` with its reason.

An account-free Linux check exercises the real output/mixer/decoder using an
isolated ALSA null device. It injects the device-error notification, verifies a
structured failure and lease release, then starts a new track and checks output
reopening. It neither plays audible sound nor changes the user's default output:

```sh
audio_config=$(mktemp)
cat > "$audio_config" <<'ALSA'
pcm.!default { type null }
ctl.!default { type hw card 0 }
ALSA
ALSA_CONFIG_PATH="$audio_config" cargo test --locked -p vault-media --lib \
  output_device_failure_releases_media_and_next_play_reopens -- --ignored
rm "$audio_config"
```

Actual route removal is still a physical-device acceptance test. After a hard
output failure, the next explicit Play reopens the output; automatic migration
and position restoration are not promised. Also verify that a cancelled download
returns promptly while Telegram imposes a long FloodWait, and that an oversized
local import is rejected before staging/upload (2000 MiB basic, 4000 MiB Premium
for the pinned TDLib transport). Downloading or server-copying existing large
messages is a distinct operation.
