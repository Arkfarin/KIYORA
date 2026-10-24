#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
: "${NDK_HOME:?Set NDK_HOME to Android NDK r28c or later}"
abi="${KIYORA_ANDROID_ABI:-arm64-v8a}"
case "$abi" in arm64-v8a|x86_64) ;; *) echo 'Unsupported Android ABI' >&2; exit 1;; esac
bash scripts/build-tdlib-android.sh
package="$PWD/target/native/android-$abi/package"
destination="$PWD/target/native/android-jni/$abi"
mkdir -p "$destination" target/native/android-assets/native-licenses
case "$(uname -s)" in
  Linux) host_tag=linux-x86_64 ;;
  Darwin) host_tag=darwin-x86_64 ;;
  *) echo 'Unsupported NDK host' >&2; exit 1 ;;
esac
cp "$package/libtdjson.so" "$destination/libtdjson.so.part"
"$NDK_HOME/toolchains/llvm/prebuilt/$host_tag/bin/llvm-strip" --strip-unneeded "$destination/libtdjson.so.part"
mv "$destination/libtdjson.so.part" "$destination/libtdjson.so"
cp "$package"/licenses/* target/native/android-assets/native-licenses/
cp licenses/* target/native/android-assets/native-licenses/
python3 - "$destination/libtdjson.so" <<'PY'
import hashlib, json, pathlib, sys
library = pathlib.Path(sys.argv[1])
manifest = {'tdlib_version': '1.8.69', 'tdlib_revision': '9d92b49bc5d0d2e29fe518239af647a88f8680e6',
            'abi': library.parent.name, 'sha256': hashlib.file_digest(library.open('rb'), 'sha256').hexdigest()}
pathlib.Path('target/native/android-assets/native-licenses/tdlib-' + library.parent.name + '.json').write_text(json.dumps(manifest, indent=2) + '\n')
print('Prepared TDLib for Android:', library, library.stat().st_size, 'bytes')
PY
