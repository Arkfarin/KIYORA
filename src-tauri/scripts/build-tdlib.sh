#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
revision=9d92b49bc5d0d2e29fe518239af647a88f8680e6
native_root="$PWD/target/native"
mkdir -p "$native_root"
if [[ ! -f "$native_root/td-$revision/CMakeLists.txt" ]]; then
  curl --fail --location "https://codeload.github.com/tdlib/td/tar.gz/$revision" -o "$native_root/tdlib.tar.gz"
  tar -xzf "$native_root/tdlib.tar.gz" -C "$native_root"
fi
gperf_path="${GPERF_EXECUTABLE:-$(command -v gperf || true)}"
if [[ -z "$gperf_path" && -x "$native_root/tools/bin/gperf" ]]; then
  gperf_path="$native_root/tools/bin/gperf"
fi
if [[ -z "$gperf_path" ]]; then
  echo 'Install gperf or set GPERF_EXECUTABLE to its absolute path.' >&2
  exit 1
fi
cmake -S "$native_root/td-$revision" -B "$native_root/td-build" \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$native_root/td-install" \
  -DGPERF_EXECUTABLE="$gperf_path" -DTD_ENABLE_JNI=OFF -DTD_ENABLE_DOTNET=OFF
cmake --build "$native_root/td-build" --target tdjson --parallel "${KIYORA_BUILD_JOBS:-2}"
