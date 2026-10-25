#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${ANDROID_HOME:?Set ANDROID_HOME to the Android SDK}"
: "${KIYORA_ANDROID_SERIAL:?Select a dedicated test device/emulator explicitly}"
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-$PWD/target/android-plugin/gradle-home}"
# Build the app with package-android.sh first. No Telegram account is used.
abi="${KIYORA_ANDROID_ABI:-arm64-v8a}"
case "$abi" in
  arm64-v8a) arch=arm64; target=aarch64; rust_task=Arm64 ;;
  x86_64) arch=x86_64; target=x86_64; rust_task=X86_64 ;;
  *) echo "Unsupported test ABI: $abi" >&2; exit 1 ;;
esac
gen/android/gradlew --no-daemon --console=plain --project-dir gen/android \
  :app:assembleUniversalDebugAndroidTest \
  "-PabiList=$abi" "-ParchList=$arch" "-PtargetList=$target" \
  "-x" ":app:rustBuild${rust_task}Debug"
adb="$ANDROID_HOME/platform-tools/adb"
apk="${KIYORA_ANDROID_APK:-gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk}"
"$adb" -s "$KIYORA_ANDROID_SERIAL" install -r "$apk"
"$adb" -s "$KIYORA_ANDROID_SERIAL" install -r \
  gen/android/app/build/outputs/apk/androidTest/universal/debug/app-universal-debug-androidTest.apk
mkdir -p target/android-app
# Tauri intentionally exits when the last Activity is destroyed. Stop it in the
# test, then let the runner finish reporting before Android terminates the process.
"$adb" -s "$KIYORA_ANDROID_SERIAL" shell am instrument -w \
  -e waitForActivitiesToComplete false \
  -e class io.github.arkfarin.kiyora.NativePackagingTest \
  io.github.arkfarin.kiyora.test/androidx.test.runner.AndroidJUnitRunner \
  | tee target/android-app/instrumentation.log
python3 - <<'PY'
import pathlib, re
result = pathlib.Path('target/android-app/instrumentation.log').read_text()
if not re.search(r'OK \(1 test\)', result) or 'FAILURES!!!' in result or 'shortMsg=' in result:
    raise SystemExit('Installed Android app instrumentation did not pass')
PY
