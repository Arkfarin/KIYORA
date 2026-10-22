# Android backend validation

The Tauri Android shell uses the local `tauri-plugin-kiyora-native` plugin for
Keystore storage. Profiles, TDLib sessions, cache and encrypted secret records
are app-private and excluded from automatic Android backup. The implementation
and failure semantics are recorded in [ADR 006](../spec/adr/006-android-keystore.md).
Frontend files are not involved in the checks below.

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

The Rust bridge can be checked independently with a matching Rust compiler and
Android standard library:

```sh
rustup target add aarch64-linux-android
cargo check --locked -p tauri-plugin-kiyora-native --target aarch64-linux-android
```

For a distro-packaged Rust compiler, upstream target libraries can have incompatible
compiler metadata even at the same release. Use a complete matching toolchain.
The local check used the official Rust 1.99.1 compiler and host/Android standard
libraries under `target/android-tools/rust-sysroot`, without changing system Rust.

## Full application requirements still to verify

The complete app additionally needs the Android NDK, a Tauri Android project and
the pinned TDLib build described in [tdlib.md](tdlib.md). Package `libtdjson.so`
and its native dependencies for `arm64-v8a`. The current bridge resolves TDLib
under `ApplicationInfo.nativeLibraryDir`; configure the app's Android packaging
with `jniLibs.useLegacyPackaging = true` so those libraries are extracted there.
Verify native dependencies and startup on the final packaged application.

Current evidence: native plugin assembly/Lint, arm64 Rust bridge check and five
Keystore tests in Android 15/API 35 AOSP x86_64 emulator. Android Lint reports two
dependency-version notices for the pinned AndroidX test libraries, no errors.
No physical-device or complete app/TDLib build is claimed. Android playback and
media-session/foreground-service integration remain unimplemented.
