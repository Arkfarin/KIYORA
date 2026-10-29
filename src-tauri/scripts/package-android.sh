#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
source scripts/android-env.sh
kiyora_signing_properties="${KIYORA_ANDROID_SIGNING_PROPERTIES:-$HOME/.local/share/kiyora/android-signing/keystore.properties}"
if [[ "${KIYORA_ANDROID_REQUIRE_SIGNING:-0}" == 1 && ! -f "$kiyora_signing_properties" ]]; then
  echo "Release signing is not configured. Restore the signing directory or set KIYORA_ANDROID_SIGNING_PROPERTIES." >&2
  exit 1
fi
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
python3 - "$NDK_HOME/toolchains/llvm/prebuilt/$host_tag/bin/llvm-readelf" "$ANDROID_HOME/build-tools/36.0.0" "$kiyora_signing_properties" "$@" <<'PY'
import hashlib, json, os, pathlib, subprocess, sys
variant = 'debug' if any(arg in ('--debug', '-d') for arg in sys.argv[4:]) else 'release'
output = pathlib.Path('gen/android/app/build/outputs/apk/universal', variant)
# Gradle's metadata identifies this build even if an older unsigned APK remains.
elements = json.loads((output / 'output-metadata.json').read_text())['elements']
if len(elements) != 1:
    raise SystemExit('Expected exactly one current universal APK in Gradle output metadata')
apk = output / elements[0]['outputFile']
build_tools = pathlib.Path(sys.argv[2])
subprocess.run(['python3', 'scripts/verify-android-package.py', str(apk), '--readelf', sys.argv[1], '--aapt', str(build_tools / 'aapt2')], check=True)
if variant == 'debug' or pathlib.Path(sys.argv[3]).is_file() or os.environ.get('KIYORA_ANDROID_REQUIRE_SIGNING') == '1':
    subprocess.run([str(build_tools / 'apksigner'), 'verify', '--verbose', '--print-certs', str(apk)], check=True)
subprocess.run([str(build_tools / 'zipalign'), '-c', '4', str(apk)], check=True)
with apk.open('rb') as source:
    digest = hashlib.file_digest(source, 'sha256').hexdigest()
(output / 'SHA256SUMS').write_text(f'{digest}  {apk.name}\n')
print('Verified APK:', apk.resolve())
PY
