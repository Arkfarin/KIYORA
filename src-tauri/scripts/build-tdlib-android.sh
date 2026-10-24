#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${NDK_HOME:?Set NDK_HOME to Android NDK r28c or later}"
revision=9d92b49bc5d0d2e29fe518239af647a88f8680e6
openssl_version=3.5.9
openssl_sha256=603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a
native_root="$PWD/target/native"
python3 scripts/fetch-native-sources.py
abi="${KIYORA_ANDROID_ABI:-arm64-v8a}"
case "$abi" in
  arm64-v8a) openssl_target=android-arm64 ;;
  x86_64) openssl_target=android-x86_64 ;;
  *) echo 'Supported ABIs: arm64-v8a, x86_64' >&2; exit 1 ;;
esac
case "$(uname -s)" in
  Linux) host_tag=linux-x86_64 ;;
  Darwin) host_tag=darwin-x86_64 ;;
  *) echo 'Build from Linux or macOS' >&2; exit 1 ;;
esac
# Cross compilation only needs host TL generators, not a complete host TDLib.
gperf_path="${GPERF_EXECUTABLE:-$(command -v gperf || true)}"
if [[ -z "$gperf_path" && -x "$native_root/tools/bin/gperf" ]]; then
  gperf_path="$native_root/tools/bin/gperf"
fi
[[ -n "$gperf_path" ]] || { echo 'Install gperf or set GPERF_EXECUTABLE' >&2; exit 1; }
cmake -S "$native_root/td-$revision" -B "$native_root/td-host-generator" \
  -DCMAKE_BUILD_TYPE=Release -DTD_GENERATE_SOURCE_FILES=ON -DGPERF_EXECUTABLE="$gperf_path"
cmake --build "$native_root/td-host-generator" --target prepare_cross_compiling --parallel "${KIYORA_BUILD_JOBS:-2}"
mkdir -p "$native_root/android-$abi"
archive="$native_root/openssl-$openssl_version.tar.gz"
if [[ ! -f "$archive" ]]; then
  curl --fail --location --retry 3 "https://github.com/openssl/openssl/releases/download/openssl-$openssl_version/openssl-$openssl_version.tar.gz" -o "$archive.part"
  mv "$archive.part" "$archive"
fi
printf '%s  %s\n' "$openssl_sha256" "$archive" | sha256sum --check
openssl_root="$native_root/android-$abi/openssl"
if [[ ! -f "$openssl_root/lib/libcrypto.a" ]]; then
  if [[ ! -d "$native_root/android-$abi/openssl-$openssl_version" ]]; then
    tar -xzf "$archive" -C "$native_root/android-$abi"
  fi
  (
    cd "$native_root/android-$abi/openssl-$openssl_version"
    export ANDROID_NDK_ROOT="$NDK_HOME"
    export PATH="$NDK_HOME/toolchains/llvm/prebuilt/$host_tag/bin:$PATH"
    ./Configure "$openssl_target" -D__ANDROID_API__=26 no-shared no-tests no-apps \
      --prefix="$openssl_root" --libdir=lib -fPIC
    make --jobs="${KIYORA_BUILD_JOBS:-2}"
    make install_sw
  )
fi
zlib_root="$native_root/android-$abi/zlib"
cmake -S "$native_root/zlib-1.3.2" -B "$native_root/android-$abi/zlib-build" \
  -DCMAKE_TOOLCHAIN_FILE="$NDK_HOME/build/cmake/android.toolchain.cmake" \
  -DANDROID_ABI="$abi" -DANDROID_PLATFORM=android-26 \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$zlib_root" \
  -DCMAKE_INSTALL_LIBDIR=lib -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
  -DZLIB_BUILD_SHARED=OFF -DZLIB_BUILD_TESTING=OFF
cmake --build "$native_root/android-$abi/zlib-build" --parallel "${KIYORA_BUILD_JOBS:-2}"
cmake --install "$native_root/android-$abi/zlib-build"
cmake -S "$native_root/td-$revision" -B "$native_root/android-$abi/td-build" \
  -DCMAKE_TOOLCHAIN_FILE="$NDK_HOME/build/cmake/android.toolchain.cmake" \
  -DANDROID_ABI="$abi" -DANDROID_PLATFORM=android-26 -DANDROID_STL=c++_static \
  -DCMAKE_BUILD_TYPE=Release '-DCMAKE_CXX_FLAGS_RELEASE=-O2 -DNDEBUG' \
  -DCMAKE_SHARED_LINKER_FLAGS=-Wl,-z,max-page-size=16384 \
  -DOPENSSL_ROOT_DIR="$openssl_root" -DOPENSSL_INCLUDE_DIR="$openssl_root/include" \
  -DOPENSSL_CRYPTO_LIBRARY="$openssl_root/lib/libcrypto.a" \
  -DOPENSSL_SSL_LIBRARY="$openssl_root/lib/libssl.a" -DOPENSSL_USE_STATIC_LIBS=TRUE \
  -DZLIB_INCLUDE_DIR="$zlib_root/include" -DZLIB_LIBRARY="$zlib_root/lib/libz.a" \
  -DTD_ENABLE_JNI=OFF -DTD_ENABLE_DOTNET=OFF
cmake --build "$native_root/android-$abi/td-build" --target tdjson --parallel "${KIYORA_BUILD_JOBS:-2}"
output="$native_root/android-$abi/package"
mkdir -p "$output/licenses"
cp "$native_root/android-$abi/td-build/libtdjson.so" "$output/libtdjson.so"
cp "$native_root/td-$revision/LICENSE_1_0.txt" "$output/licenses/TDLib.txt"
cp "$native_root/android-$abi/openssl-$openssl_version/LICENSE.txt" "$output/licenses/OpenSSL.txt"
cp "$native_root/zlib-1.3.2/LICENSE" "$output/licenses/zlib.txt"
"$NDK_HOME/toolchains/llvm/prebuilt/$host_tag/bin/llvm-readelf" -h -l -d "$output/libtdjson.so" > "$output/elf.txt"
python3 - "$output" "$abi" <<'PY'
import pathlib,sys
root=pathlib.Path(sys.argv[1]);elf=(root/'elf.txt').read_text()
assert ('AArch64' if sys.argv[2]=='arm64-v8a' else 'X86-64') in elf
segments=[line.split() for line in elf.splitlines() if line.strip().startswith('LOAD ')]
assert segments and all(int(row[-1],16)>=16384 for row in segments)
for dependency in ['libssl.so','libcrypto.so','libc++_shared.so']:
    assert dependency not in elf, 'unexpected dynamic dependency: '+dependency
print('TDLib Android package:',root)
PY
