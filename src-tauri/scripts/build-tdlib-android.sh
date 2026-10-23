#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${NDK_HOME:?Set NDK_HOME to Android NDK r28c or later}"
revision=9d92b49bc5d0d2e29fe518239af647a88f8680e6
openssl_version=3.5.9
openssl_sha256=603f5602e2eef00d77fbd429d34dcd5822bb301757a1bc9cdb24c670f1eb859a
native_root="$PWD/target/native"
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
if [[ ! -f "$native_root/td-build/CMakeCache.txt" ]]; then bash scripts/build-tdlib.sh; fi
cmake --build "$native_root/td-build" --target prepare_cross_compiling --parallel "${KIYORA_BUILD_JOBS:-2}"
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
cmake -S "$native_root/td-$revision" -B "$native_root/android-$abi/td-build" \
  -DCMAKE_TOOLCHAIN_FILE="$NDK_HOME/build/cmake/android.toolchain.cmake" \
  -DANDROID_ABI="$abi" -DANDROID_PLATFORM=android-26 -DANDROID_STL=c++_static \
  -DCMAKE_BUILD_TYPE=Release '-DCMAKE_CXX_FLAGS_RELEASE=-O2 -DNDEBUG' \
  -DCMAKE_SHARED_LINKER_FLAGS=-Wl,-z,max-page-size=16384 \
  -DOPENSSL_ROOT_DIR="$openssl_root" -DOPENSSL_INCLUDE_DIR="$openssl_root/include" \
  -DOPENSSL_CRYPTO_LIBRARY="$openssl_root/lib/libcrypto.a" \
  -DOPENSSL_SSL_LIBRARY="$openssl_root/lib/libssl.a" -DOPENSSL_USE_STATIC_LIBS=TRUE \
  -DTD_ENABLE_JNI=OFF -DTD_ENABLE_DOTNET=OFF
cmake --build "$native_root/android-$abi/td-build" --target tdjson --parallel "${KIYORA_BUILD_JOBS:-2}"
output="$native_root/android-$abi/package"
mkdir -p "$output/licenses"
cp "$native_root/android-$abi/td-build/libtdjson.so" "$output/libtdjson.so"
cp "$native_root/td-$revision/LICENSE_1_0.txt" "$output/licenses/TDLib.txt"
cp "$native_root/android-$abi/openssl-$openssl_version/LICENSE.txt" "$output/licenses/OpenSSL.txt"
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
