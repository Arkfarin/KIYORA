#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/android-env.sh

missing=0
for tool in java javac node pnpm cargo rustc rustup cmake make g++ perl gperf curl python3 sdkmanager adb; do
  if ! command -v "$tool" >/dev/null; then
    echo "Missing command: $tool" >&2
    missing=1
  fi
done
for file in "$ANDROID_HOME/platforms/android-36/android.jar" \
  "$ANDROID_HOME/build-tools/36.0.0/aapt2" \
  "$ANDROID_HOME/build-tools/36.0.0/apksigner" \
  "$NDK_HOME/build/cmake/android.toolchain.cmake"; do
  if [[ ! -f "$file" ]]; then
    echo "Missing Android SDK/NDK file: $file" >&2
    missing=1
  fi
done
case "${KIYORA_ANDROID_ABI:-arm64-v8a}" in
  arm64-v8a) target=aarch64-linux-android ;;
  x86_64) target=x86_64-linux-android ;;
  *) echo 'Supported ABIs: arm64-v8a, x86_64' >&2; exit 1 ;;
esac
if command -v rustup >/dev/null && ! rustup target list --installed | grep -qx "$target"; then
  echo "Missing Rust target; run: rustup target add $target" >&2
  missing=1
fi
if [[ "$missing" != 0 ]]; then
  echo 'See src-tauri/docs/packaging.md for Android setup.' >&2
  exit 1
fi
python3 - <<'PY'
import subprocess
import sys
import re

if sys.version_info < (3, 12):
    raise SystemExit('Python 3.12+ is required for native source preparation')
version = subprocess.check_output(['javac', '-version'], text=True, stderr=subprocess.STDOUT)
if int(re.search(r'javac (\d+)', version).group(1)) < 17:
    raise SystemExit('JDK 17+ is required; set JAVA_HOME to a supported JDK')
PY
printf 'ANDROID_HOME=%s\nNDK_HOME=%s\nJAVA_HOME=%s\nRust target: %s\n' \
  "$ANDROID_HOME" "$NDK_HOME" "${JAVA_HOME:-<system Java>}" "$target"
java -version
cargo --version
sdkmanager --version
gen/android/gradlew --version
echo 'Android build prerequisites are available.'
