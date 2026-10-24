#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
[[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || { echo 'Linux x86_64 host required' >&2; exit 1; }
python3 scripts/fetch-native-sources.py
native_root="$PWD/target/native"
jobs="${KIYORA_BUILD_JOBS:-2}"
openssl_root="$native_root/linux-openssl"
zlib_root="$native_root/linux-zlib"
if [[ ! -f "$openssl_root/lib/libcrypto.a" ]]; then
  mkdir -p "$native_root/linux-openssl-build"
  (
    cd "$native_root/linux-openssl-build"
    "$native_root/openssl-3.5.9/Configure" linux-x86_64 no-shared no-tests no-apps no-module \
      --prefix="$openssl_root" --libdir=lib -fPIC
    make --jobs="$jobs"
    make install_sw
  )
fi
cmake -S "$native_root/zlib-1.3.2" -B "$native_root/linux-zlib-build" \
  -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX="$zlib_root" \
  -DCMAKE_INSTALL_LIBDIR=lib -DCMAKE_POSITION_INDEPENDENT_CODE=ON \
  -DZLIB_BUILD_SHARED=OFF -DZLIB_BUILD_TESTING=OFF
cmake --build "$native_root/linux-zlib-build" --parallel "$jobs"
cmake --install "$native_root/linux-zlib-build"
gperf_path="${GPERF_EXECUTABLE:-$(command -v gperf || true)}"
if [[ -z "$gperf_path" && -x "$native_root/tools/bin/gperf" ]]; then
  gperf_path="$native_root/tools/bin/gperf"
fi
[[ -n "$gperf_path" ]] || { echo 'Install gperf or set GPERF_EXECUTABLE' >&2; exit 1; }
# This is separate from the host generator build used by Android cross compilation.
cmake -S "$native_root/td-9d92b49bc5d0d2e29fe518239af647a88f8680e6" -B "$native_root/linux-td-build" \
  -DCMAKE_BUILD_TYPE=Release '-DCMAKE_CXX_FLAGS_RELEASE=-O2 -DNDEBUG' \
  -DGPERF_EXECUTABLE="$gperf_path" -DTD_ENABLE_JNI=OFF -DTD_ENABLE_DOTNET=OFF \
  -DOPENSSL_ROOT_DIR="$openssl_root" -DOPENSSL_INCLUDE_DIR="$openssl_root/include" \
  -DOPENSSL_CRYPTO_LIBRARY="$openssl_root/lib/libcrypto.a" \
  -DOPENSSL_SSL_LIBRARY="$openssl_root/lib/libssl.a" -DOPENSSL_USE_STATIC_LIBS=TRUE \
  -DZLIB_INCLUDE_DIR="$zlib_root/include" -DZLIB_LIBRARY="$zlib_root/lib/libz.a"
cmake --build "$native_root/linux-td-build" --target tdjson --parallel "$jobs"
package="$native_root/desktop"
mkdir -p "$package/native" "$package/licenses"
cp "$native_root/linux-td-build/libtdjson.so" "$package/native/libtdjson.so.part"
strip --strip-unneeded "$package/native/libtdjson.so.part"
mv "$package/native/libtdjson.so.part" "$package/native/libtdjson.so"
cp "$native_root/td-9d92b49bc5d0d2e29fe518239af647a88f8680e6/LICENSE_1_0.txt" "$package/licenses/TDLib.txt"
cp "$native_root/openssl-3.5.9/LICENSE.txt" "$package/licenses/OpenSSL.txt"
cp "$native_root/zlib-1.3.2/LICENSE" "$package/licenses/zlib.txt"
python3 scripts/desktop-native-manifest.py "$package" --platform linux-x86_64
