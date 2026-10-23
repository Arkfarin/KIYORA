#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${NDK_HOME:?Set NDK_HOME to the Android NDK (verified with r28c)}"
if [[ -z "${KIYORA_NDK_HOST_TAG:-}" ]]; then
  case "$(uname -s)" in
    Linux) KIYORA_NDK_HOST_TAG=linux-x86_64 ;;
    Darwin) KIYORA_NDK_HOST_TAG=darwin-x86_64 ;;
    *) echo "Set KIYORA_NDK_HOST_TAG for this NDK host" >&2; exit 1 ;;
  esac
fi
kiyora_toolchain="$NDK_HOME/toolchains/llvm/prebuilt/$KIYORA_NDK_HOST_TAG/bin"
# CPAL 0.16 links AAudio, so API 24 compile-only checks are insufficient.
export CC_aarch64_linux_android="$kiyora_toolchain/aarch64-linux-android26-clang"
export AR_aarch64_linux_android="$kiyora_toolchain/llvm-ar"
export CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$CC_aarch64_linux_android"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PWD/target/android-rust}"
python3 - <<'PY'
import json
with open('tauri.conf.json') as source:
    assert json.load(source)['bundle']['android']['minSdkVersion'] >= 26, 'AAudio requires API 26'
PY
cargo build --locked -p kiyora --lib --target aarch64-linux-android
kiyora_library="$CARGO_TARGET_DIR/aarch64-linux-android/debug/libkiyora_lib.so"
"$kiyora_toolchain/llvm-readelf" -h -l -d "$kiyora_library" > "$CARGO_TARGET_DIR/kiyora-elf.txt"
"$kiyora_toolchain/llvm-nm" --dynamic --defined-only "$kiyora_library" > "$CARGO_TARGET_DIR/kiyora-symbols.txt"
python3 - "$CARGO_TARGET_DIR" <<'PY'
import pathlib, re, sys
root = pathlib.Path(sys.argv[1])
elf = (root / 'kiyora-elf.txt').read_text()
symbols = (root / 'kiyora-symbols.txt').read_text()
assert re.search(r'Machine:\s+AArch64', elf), 'wrong native architecture'
assert 'Shared library: [libaaudio.so]' in elf, 'missing AAudio linkage'
segments = [line.split() for line in elf.splitlines() if line.strip().startswith('LOAD ')]
assert segments and all(int(parts[-1], 16) >= 16384 for parts in segments), '16 KiB LOAD alignment required'
for symbol in ['Java_io_github_arkfarin_kiyora_Rust_onCreate', 'Java_app_tauri_plugin_PluginManager_sendChannelData']:
    assert symbol in symbols, f'missing JNI entry: {symbol}'
print('Android arm64 library linked; AAudio, JNI exports and 16 KiB LOAD alignment verified.')
print('This is not a complete APK/TDLib or device playback test.')
PY
