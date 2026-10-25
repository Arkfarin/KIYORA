# Backend native packaging

Use these scripts for distributable installers. A standalone Rust build does not
bundle TDLib. No Telegram account or API credentials are needed to build or check
a package. These scripts do not upload or publish it.

Install frontend dependencies from the repository root with
`pnpm install --frozen-lockfile` (pnpm 12.11.2; CI uses Node 24). The root lockfile
preserves the versions already used by the local frontend/Tauri build. Commands
below run from `src-tauri`.

## Linux x86_64

Use a Linux x86_64 builder with Rust, a C/C++ compiler, CMake, make, Perl, gperf,
Python 3.12+, curl, binutils and Tauri's GTK/WebKit/ALSA/D-Bus development packages.
Ubuntu 24.04 is the CI distribution baseline. Building on a newer distribution
does not prove compatibility with Ubuntu 24.04.

```sh
python3 scripts/package-desktop.py
python3 scripts/check-installer.py target/release/bundle/deb/kiyora_0.1.0_amd64.deb
```

Preparation verifies pinned source archives, builds static OpenSSL and zlib,
builds and strips TDLib, checks ELF architecture/dependencies, and generates
`target/native/desktop/tauri-bundle.json`. The package command passes that resource
map to Tauri. The installed library is `/usr/lib/kiyora/native/libtdjson.so`.
Its manifest and TDLib/OpenSSL/zlib/Opus licenses are included. Standard platform
libraries such as glibc and libstdc++ remain system dependencies.

The verifier extracts the Debian archive to a temporary directory containing
spaces and Unicode characters, checks the
actual payload and runs the Rust TDLib version/start/close/reopen test against
that extracted file. It does not test the graphical shell or audio hardware.
`--debug` selects a debug package for local iteration. `--bundles deb,appimage`
builds both Linux formats with Tauri's packaging tools. Verify the AppImage with:

```sh
python3 scripts/check-installer.py target/release/bundle/appimage/kiyora_0.1.0_amd64.AppImage
```

Extraction uses the AppImage runtime without FUSE. Run it only on the locally
built/trusted installer. The verifier checks the extracted AppDir and native
library lifecycle; it does not establish graphical startup or audio output.
Linux CI checks both formats and records whole-installer `SHA256SUMS` in the
bundle directory after successful verification.

## Windows x86_64

Use an x64 Visual Studio Developer PowerShell with Rust MSVC, CMake, Python 3.12+,
Git, curl, NASM and Strawberry Perl. CI provisions tools on Windows Server 2022.
A pinned vcpkg checkout builds only the host gperf generator. OpenSSL and zlib
use the reviewed archives shared with the other platform scripts. TDLib and its
dependencies use the static MSVC CRT.

```powershell
python scripts/package-desktop.py
$msi = @(Get-ChildItem target/release/bundle/msi/*.msi)
if ($msi.Count -ne 1) { throw 'Expected one MSI' }
python scripts/check-installer.py $msi[0].FullName
```

`native/tdjson.dll` is installed beside the application with its manifest and
licenses. `dumpbin` checks architecture and rejects unexpected non-system DLL
dependencies. The verifier performs an MSI administrative extraction to a
temporary directory containing spaces and Unicode and tests the extracted DLL
through the Rust adapter.
These commands must run on Windows before claiming Windows runtime verification.

## Android ARM64

Install Rust's `aarch64-linux-android` target through rustup, JDK 17+, Android SDK
platform 36/build-tools 36.0.0 and NDK r28c (28.2.13676358). The host also needs
CMake, a C/C++ compiler, make, Perl, gperf, curl and Python 3.12+.

```sh
export ANDROID_HOME=/absolute/path/to/android-sdk
export NDK_HOME="$ANDROID_HOME/ndk/28.2.13676358"
bash scripts/package-android.sh
```

The default is a release ARM64 APK. Use `--debug` for a locally signed development
APK. For an x86_64 emulator, install Rust's `x86_64-linux-android` target and set
`KIYORA_ANDROID_ABI=x86_64`. ARMv7 and x86 are not release targets.

Host TDLib TL generators are built separately; cross compilation does not need
a complete host TDLib. TDLib statically includes OpenSSL, zlib and libc++, while
the application statically includes Opus. Prepared JNI libraries and licenses
are added by the tracked Gradle project. Explicit legacy JNI packaging extracts
the libraries to Android's `ApplicationInfo.nativeLibraryDir`; Rust receives this
runtime path from the native plugin. The installed app does not use a build-machine
library path. Gradle fails if a requested ABI has no prepared TDLib.

The script checks the resulting APK for application and TDLib libraries, ABI,
pinned digest, licenses, unresolved native dependencies and 16 KiB ELF LOAD
alignment. JNI libraries are compressed and extracted, so this is not a claim
about uncompressed ZIP page alignment. APK inspection does not prove startup,
audible playback, background behavior or real-account operations. Device checks
remain separate; see [Android integration](android.md) and the
[optional live/device acceptance procedure](e2e.md).

The ABI check reads the numeric ELF machine field, so GNU/LLVM readelf spelling
differences cannot reject a valid x86_64 package. The Gradle wrapper verifies the
official 8.14.3 distribution checksum; CI exercises it before native compilation.

## Dependency and loader policy

`scripts/native-sources.json` records versions, upstream archive locations and
SHA-256 values. Update pins deliberately, rebuild all native payloads and rerun
the package checks. Runtime TDLib still rejects a version other than 1.8.69.
Source pins do not guarantee byte-reproducible binaries across compilers or OSes.

Desktop lookup checks Tauri resources and executable-relative locations, without
searching the working directory. `KIYORA_TDJSON` remains an explicit developer
override; use an absolute trusted path. Windows uses safe DLL search flags.
Library handles are cached by canonical path for the process lifetime because
TDLib worker threads must never execute in an unloaded library. Clients and
receive workers are still closed separately between profiles.

Execution evidence and remaining platform checks are recorded in
[backend readiness](../../docs/BACKEND_READINESS.md).
