#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${ANDROID_HOME:?Set ANDROID_HOME to the Android SDK}"
: "${NDK_HOME:?Set NDK_HOME to Android NDK r28c or later}"
export ANDROID_NDK_HOME="$NDK_HOME"
export ANDROID_PLATFORM=android-26
bash scripts/prepare-android-native.sh
case "${KIYORA_ANDROID_ABI:-arm64-v8a}" in
  arm64-v8a) target=aarch64 ;;
  x86_64) target=x86_64 ;;
  *) echo 'Unsupported ABI' >&2; exit 1 ;;
esac
case "$(uname -s)" in
  Linux) host_tag=linux-x86_64 ;;
  Darwin) host_tag=darwin-x86_64 ;;
  *) echo 'Unsupported NDK host' >&2; exit 1 ;;
esac
# Defaults to release. --debug is accepted for local packaging checks.
pnpm --dir .. tauri android build --target "$target" --apk --ci "$@"
python3 - "$NDK_HOME/toolchains/llvm/prebuilt/$host_tag/bin/llvm-readelf" "$ANDROID_HOME/build-tools/36.0.0/aapt2" "$@" <<'PY'
import pathlib, subprocess, sys
variant = 'debug' if '--debug' in sys.argv[3:] else 'release'
packages = list(pathlib.Path('gen/android/app/build/outputs/apk/universal', variant).glob('*.apk'))
if len(packages) != 1:
    raise SystemExit('Expected exactly one current universal APK, found: ' + str(packages))
subprocess.run(['python3', 'scripts/verify-android-package.py', str(packages[0]), '--readelf', sys.argv[1], '--aapt', sys.argv[2]], check=True)
PY
