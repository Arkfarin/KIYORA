param([int]$Jobs = 2)
$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Set-Location (Join-Path $PSScriptRoot '..')
if (-not [Environment]::Is64BitOperatingSystem -or $env:OS -ne 'Windows_NT') {
    throw 'Run in the x64 Visual Studio Developer PowerShell on Windows'
}
function Invoke-Checked([string]$Program, [string[]]$Arguments) {
    & $Program @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$Program exited with $LASTEXITCODE" }
}
foreach ($tool in @('cl', 'nmake', 'dumpbin', 'perl', 'nasm', 'cmake', 'python', 'git')) {
    Get-Command $tool -ErrorAction Stop | Out-Null
}
Invoke-Checked python @('scripts/fetch-native-sources.py')
$nativeRoot = Join-Path $PWD 'target/native'
$opensslRoot = Join-Path $nativeRoot 'windows-openssl'
$zlibRoot = Join-Path $nativeRoot 'windows-zlib'
if (-not (Test-Path "$opensslRoot/lib/libcrypto.lib")) {
    New-Item -ItemType Directory -Force "$nativeRoot/windows-openssl-build" | Out-Null
    Push-Location "$nativeRoot/windows-openssl-build"
    try {
        Invoke-Checked perl @("$nativeRoot/openssl-3.5.9/Configure", 'VC-WIN64A', 'no-shared', 'no-tests', 'no-apps', 'no-module', "--prefix=$opensslRoot", '--libdir=lib')
        Invoke-Checked nmake @()
        Invoke-Checked nmake @('install_sw')
    } finally { Pop-Location }
}
Invoke-Checked cmake @('-S', "$nativeRoot/zlib-1.3.2", '-B', "$nativeRoot/windows-zlib-build", '-A', 'x64',
    "-DCMAKE_INSTALL_PREFIX=$zlibRoot", '-DCMAKE_INSTALL_LIBDIR=lib', '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded',
    '-DZLIB_BUILD_SHARED=OFF', '-DZLIB_BUILD_TESTING=OFF')
Invoke-Checked cmake @('--build', "$nativeRoot/windows-zlib-build", '--config', 'Release', '--parallel', "$Jobs")
Invoke-Checked cmake @('--install', "$nativeRoot/windows-zlib-build", '--config', 'Release')

# vcpkg is used only for the host gperf generator. Runtime dependencies above
# are built from the same reviewed archive pins as Linux and Android.
$vcpkgRevision = '0c2a9ace1964b818b15f7ae50ba1c42404918a7b'
$vcpkg = Join-Path $nativeRoot "vcpkg-$vcpkgRevision"
if (-not (Test-Path "$vcpkg/.git")) {
    Invoke-Checked git @('init', $vcpkg)
    Invoke-Checked git @('-C', $vcpkg, 'fetch', '--depth', '1', 'https://github.com/microsoft/vcpkg.git', $vcpkgRevision)
    Invoke-Checked git @('-C', $vcpkg, 'checkout', '--detach', 'FETCH_HEAD')
}
if ((& git -C $vcpkg rev-parse HEAD) -ne $vcpkgRevision) { throw 'Unexpected vcpkg revision' }
Invoke-Checked "$vcpkg/bootstrap-vcpkg.bat" @('-disableMetrics')
Invoke-Checked "$vcpkg/vcpkg.exe" @('install', '--classic', 'gperf:x64-windows-static', '--disable-metrics')
$gperf = "$vcpkg/installed/x64-windows-static/tools/gperf/gperf.exe"
Invoke-Checked cmake @('-S', "$nativeRoot/td-9d92b49bc5d0d2e29fe518239af647a88f8680e6", '-B', "$nativeRoot/windows-td-build", '-A', 'x64',
    '-DCMAKE_POLICY_DEFAULT_CMP0091=NEW', '-DCMAKE_MSVC_RUNTIME_LIBRARY=MultiThreaded',
    "-DGPERF_EXECUTABLE=$gperf", '-DTD_ENABLE_JNI=OFF', '-DTD_ENABLE_DOTNET=OFF',
    "-DOPENSSL_ROOT_DIR=$opensslRoot", "-DOPENSSL_INCLUDE_DIR=$opensslRoot/include", '-DOPENSSL_USE_STATIC_LIBS=TRUE',
    "-DOPENSSL_CRYPTO_LIBRARY=$opensslRoot/lib/libcrypto.lib", "-DOPENSSL_SSL_LIBRARY=$opensslRoot/lib/libssl.lib",
    "-DZLIB_INCLUDE_DIR=$zlibRoot/include", "-DZLIB_LIBRARY=$zlibRoot/lib/zs.lib")
Invoke-Checked cmake @('--build', "$nativeRoot/windows-td-build", '--target', 'tdjson', '--config', 'Release', '--parallel', "$Jobs")
$package = "$nativeRoot/desktop"
New-Item -ItemType Directory -Force "$package/native", "$package/licenses" | Out-Null
Copy-Item "$nativeRoot/windows-td-build/Release/tdjson.dll" "$package/native/tdjson.dll"
Copy-Item "$nativeRoot/td-9d92b49bc5d0d2e29fe518239af647a88f8680e6/LICENSE_1_0.txt" "$package/licenses/TDLib.txt"
Copy-Item "$nativeRoot/openssl-3.5.9/LICENSE.txt" "$package/licenses/OpenSSL.txt"
Copy-Item "$nativeRoot/zlib-1.3.2/LICENSE" "$package/licenses/zlib.txt"
Invoke-Checked python @('scripts/desktop-native-manifest.py', $package, '--platform', 'windows-x86_64')
