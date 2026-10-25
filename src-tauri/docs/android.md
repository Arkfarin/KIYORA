# Android backend validation

The Tauri Android shell uses the local `tauri-plugin-kiyora-native` plugin for
Keystore storage and native playback integration. Profiles, TDLib sessions, cache and encrypted secret records
are app-private and excluded from automatic Android backup. The implementation
and failure semantics are recorded in [ADR 006](../spec/adr/006-android-keystore.md).
Frontend files are not involved in the checks below.

The app and native plugin require Android 8.0/API 26 or newer: the locked CPAL
0.16 backend links AAudio, which is [available since API 26](https://developer.android.com/ndk/guides/stable_apis).
`bundle.android.minSdkVersion` and the plugin's `minSdk` both declare 26. Using an
API 24 NDK linker fails with missing `-laaudio`, even though `cargo check` succeeds.

## Native plugin build and device tests

Install Android SDK platform 36, build-tools 36.0.0 and platform-tools, plus Gradle
8.14.3 and a compatible JDK. The standalone harness pins AGP 8.10.1 and Kotlin
2.2.0. The verified local run used Java 24.0.2. From `src-tauri`:

```sh
ANDROID_HOME=/path/to/android-sdk GRADLE=/path/to/gradle-8.14.3/bin/gradle \
  scripts/check-android-plugin.sh
```

This builds the library and its instrumentation APK and runs Android Lint. It
uses the Kotlin sources from the locked Tauri dependency and places build/cache
artifacts under `target/android-plugin`. It does not build the full app or TDLib.

To run tests, explicitly choose one emulator or test device (never all connected
devices). The runner installs a separate instrumentation package and uses unique
test aliases/directories, removed after each test. It never logs into Telegram:

```sh
ANDROID_HOME=/path/to/android-sdk GRADLE=/path/to/gradle-8.14.3/bin/gradle \
  KIYORA_ANDROID_DEVICE_TESTS=1 KIYORA_ANDROID_SERIAL=emulator-5584 \
  scripts/check-android-plugin.sh
```

The device log is `target/android-plugin/instrumentation.log`. The script checks
the runner result because `am instrument` can return exit code zero after failure.

The complete Rust native library can be built independently with a matching Rust
compiler, Android standard library and NDK (verified with r28c). The script also
checks AArch64, AAudio linkage, JNI exports and 16 KiB ELF LOAD alignment:

```sh
rustup target add aarch64-linux-android
NDK_HOME=/path/to/android-ndk-r28c scripts/check-android-rust.sh
```

For a distro-packaged Rust compiler, upstream target libraries can have incompatible
compiler metadata even at the same release. Use a complete matching toolchain.
The local check used the official Rust 1.99.1 compiler and host/Android standard
libraries under `target/android-tools/rust-sysroot`, without changing system Rust.

## Playback lifecycle

Rust owns the decoder, PCM queue, output device and cache leases. The native plugin
owns the Android `MediaSession`, notification, `mediaPlayback` foreground service,
audio focus and wake lock; it does not decode audio or use WebView playback. All
native lifecycle transitions run on the Android main thread. Commands are private
Rust-to-native calls with an owner UUID, and notification actions have the same
owner fence. Closing a player revokes its pending notification actions.
Replacement playback waits for the old service's asynchronous `onDestroy`; it
never attaches to an instance that already called `stopSelf`. Audio focus
callbacks carry their own request identity and cannot pause a newer session.
Foreground notification updates use the service's ordered publication path;
the paused notification is published before detaching the foreground service.
This prevents an older in-flight notification from restoring a stale Pause action.

`preparePlayback` and `resumeNativePlayback` promote the service before acquiring
audio focus, then resolve the Rust call only when focus is granted. This ordering
meets the [Android 15+ audio-focus requirement](https://developer.android.com/media/optimize/audio-focus).
Focus loss, including transient loss/ducking, and an unplugged audio route request
a pause. Resumption is explicit. The manifest declares the
[media-playback foreground-service type and permissions](https://developer.android.com/develop/background-work/services/fgs/service-types#media).
Pausing releases focus and the wake lock and leaves a resumable notification.
Stopping removes the notification and service. The service is not sticky; it
stops after missing Rust heartbeats for more than 30 seconds (checked every
10 seconds), or after five minutes paused.

The locked Tao 0.37.1 initializes `ndk-context` during Android Activity creation
and retains a global Application reference. The Rust media adapter checks that
context before CPAL opens its audio device; it does not initialize JNI twice.
Native startup errors are handled by a unit-returning mobile entry wrapper,
instead of discarding the desktop `run()` result.

Keystore and profile paths resolve the framework-owned `noBackupFilesDir` base
because `/data/user/0` may alias `/data/data`. The application's own child
directories, ciphertext records and AtomicFile siblings remain subject to
symlink rejection. Tests cover the legitimate alias and a malicious record link.

## Packaged application and remaining acceptance

The complete app additionally needs the Android NDK, a Tauri Android project and
the pinned TDLib build described in [tdlib.md](tdlib.md). Package `libtdjson.so`
and its native dependencies for `arm64-v8a`. The current bridge resolves TDLib
under `ApplicationInfo.nativeLibraryDir`; configure the app's Android packaging
with `jniLibs.useLegacyPackaging = true` so those libraries are extracted there.
Verify native dependencies and startup on the final packaged application.

Current evidence: native plugin assembly/Lint, complete ARM64 and x86_64 debug
APKs plus an optimized unsigned ARM64 release APK, all with native
dependency/digest/license/16 KiB LOAD checks,
six Keystore, six playback-service and two document-import tests (14 passing in total) on an Android 15/API 35
AOSP x86_64 emulator, with the test APK targeting SDK 36. Playback tests exercise
metadata, notification/transport commands, focus loss, background resume,
heartbeat failure and ownership fencing. Android Lint reports two dependency-version
notices for the pinned AndroidX test libraries, no errors.
The immediate-replacement and notification-background-resume cases also passed
five consecutive runs together (10 extra case executions); the local record is
`target/android-plugin/lifecycle-repeat.log`. This repetition caught and then
verified fixes for asynchronous service destruction and stale notification actions.

These instrumentation tests exercise Android OS integration with a test command
consumer. They do not execute the Rust PCM engine through the Tauri JNI bridge.
The separate installed-app test now verifies complete app startup, the extracted
TDLib digest/loader and a real WebView-to-Rust IPC response. Run it after building
a debug package:

```sh
ANDROID_HOME=/path/to/android-sdk KIYORA_ANDROID_ABI=x86_64 \
  KIYORA_ANDROID_SERIAL=emulator-5580 bash scripts/check-android-app.sh
```

It stops the Activity and lets the instrumentation runner report before process
termination; destroying Tauri's last Activity first races Tao's process exit with
the result reporter. The script requires `OK (1 test)` and rejects runner crashes.
The corrected harness and two repeats passed on the final x86_64 debug APK.
See [the completion report](../../docs/BACKEND_COMPLETION_REPORT.md) for exact
build variants and results. Audible physical output, headset hardware, actual
16 KiB-page devices, extended screen-off streaming and live Telegram remain
separate acceptance gates.
