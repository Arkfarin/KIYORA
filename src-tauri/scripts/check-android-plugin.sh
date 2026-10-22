#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${ANDROID_HOME:?Set ANDROID_HOME to an Android SDK with platform 36 and build-tools 36.0.0}"
mkdir -p target/android-plugin
# Work from a local copy: Gradle must not write inside Cargo's registry sources.
cargo metadata --locked --format-version 1 > target/android-plugin/metadata.json
python3 - <<'PY'
import json, pathlib, shutil
metadata = json.load(open('target/android-plugin/metadata.json'))
package = next(p for p in metadata['packages'] if p['name'] == 'tauri')
source = pathlib.Path(package['manifest_path']).parent / 'mobile/android'
shutil.copytree(source, 'target/android-plugin/tauri-api', dirs_exist_ok=True)
PY
export KIYORA_TAURI_ANDROID="$PWD/target/android-plugin/tauri-api"
export GRADLE_USER_HOME="${GRADLE_USER_HOME:-$PWD/target/android-plugin/gradle-home}"
export ANDROID_USER_HOME="${ANDROID_USER_HOME:-$PWD/target/android-plugin/android-home}"
tasks=(":kiyora-native:assembleDebug" ":kiyora-native:assembleDebugAndroidTest" ":kiyora-native:lintDebug")
"${GRADLE:-gradle}" --no-daemon --console=plain \
  --project-cache-dir "$PWD/target/android-plugin/project-cache" \
  -p scripts/android-plugin "${tasks[@]}"
if [[ "${KIYORA_ANDROID_DEVICE_TESTS:-0}" == 1 ]]; then
  : "${KIYORA_ANDROID_SERIAL:?Select one test device/emulator explicitly}"
  adb="$ANDROID_HOME/platform-tools/adb"
  "$adb" -s "$KIYORA_ANDROID_SERIAL" install -r \
    target/android-plugin/kiyora-native/outputs/apk/androidTest/debug/kiyora-native-debug-androidTest.apk
  "$adb" -s "$KIYORA_ANDROID_SERIAL" shell am instrument -w \
    io.github.arkfarin.kiyora.nativebridge.test/androidx.test.runner.AndroidJUnitRunner \
    | tee target/android-plugin/instrumentation.log
  # `am instrument` can return exit 0 when tests fail or the runner crashes.
  python3 - <<'PY'
import pathlib, re
result = pathlib.Path('target/android-plugin/instrumentation.log').read_text()
if not re.search(r'OK \([1-9][0-9]* tests?\)', result) or 'FAILURES!!!' in result:
    raise SystemExit('Android Keystore instrumentation did not pass')
PY
fi
