#!/usr/bin/env python3
"""Check actual APK native payloads, their pinned manifest and ELF dependencies."""
import argparse
import hashlib
import json
import pathlib
import re
import subprocess
import tempfile
import zipfile

parser = argparse.ArgumentParser()
parser.add_argument('apk', type=pathlib.Path)
parser.add_argument('--readelf', required=True, type=pathlib.Path)
parser.add_argument('--aapt', required=True, type=pathlib.Path)
args = parser.parse_args()
manifest_tree = subprocess.check_output([
    str(args.aapt), 'dump', 'xmltree', '--file', 'AndroidManifest.xml', str(args.apk)
], text=True)
if not re.search(r':extractNativeLibs\([^)]*\)=(?:true|\(type 0x12\)0xffffffff)\s*$', manifest_tree, re.MULTILINE):
    raise SystemExit('APK must explicitly extract native libraries for nativeLibraryDir loading')
allowed = {'libandroid.so', 'libaaudio.so', 'liblog.so', 'libm.so', 'libdl.so', 'libc.so'}
with zipfile.ZipFile(args.apk) as apk, tempfile.TemporaryDirectory() as directory:
    names = set(apk.namelist())
    libraries = [name for name in names if name.startswith('lib/') and name.endswith('/libkiyora_lib.so')]
    if not libraries:
        raise SystemExit('No application native library in APK')
    for app in libraries:
        abi = app.split('/')[1]
        if abi not in {'arm64-v8a', 'x86_64'}:
            raise SystemExit('Unsupported native ABI: ' + abi)
        tdlib = f'lib/{abi}/libtdjson.so'
        if tdlib not in names:
            raise SystemExit('Missing bundled TDLib: ' + tdlib)
        manifest_name = f'assets/native-licenses/tdlib-{abi}.json'
        manifest = json.loads(apk.read(manifest_name))
        if (manifest['tdlib_version'], manifest['tdlib_revision'], manifest['abi']) != (
            '1.8.69', '9d92b49bc5d0d2e29fe518239af647a88f8680e6', abi
        ):
            raise SystemExit('Wrong pinned TDLib identity')
        for name in [app, tdlib]:
            payload = apk.read(name)
            if name == tdlib and hashlib.sha256(payload).hexdigest() != manifest['sha256']:
                raise SystemExit('Packaged TDLib digest differs from prepared library')
            path = pathlib.Path(directory) / pathlib.Path(name).name
            path.write_bytes(payload)
            elf = subprocess.check_output([str(args.readelf), '-h', '-l', '-d', str(path)], text=True)
            # Read the standardized ELF field, not readelf's vendor-dependent
            # spelling (LLVM uses "Advanced Micro Devices X86-64").
            machine = 183 if abi == 'arm64-v8a' else 62
            if payload[:6] != b'\x7fELF\x02\x01' or int.from_bytes(payload[18:20], 'little') != machine:
                raise SystemExit('Wrong ELF architecture: ' + name)
            loads = [line.split() for line in elf.splitlines() if line.strip().startswith('LOAD ')]
            if not loads or any(int(row[-1], 16) < 16384 for row in loads):
                raise SystemExit('ELF lacks 16 KiB page alignment: ' + name)
            for dependency in re.findall(r'Shared library: \[([^]]+)\]', elf):
                if dependency not in allowed and f'lib/{abi}/{dependency}' not in names:
                    raise SystemExit('Missing native dependency: ' + dependency)
    for license_name in ['TDLib.txt', 'OpenSSL.txt', 'zlib.txt', 'libopus-COPYING', 'opusic-sys-LICENSE', 'opus-rs-LICENSE-MIT', 'opus-rs-LICENSE-APACHE']:
        if not apk.read('assets/native-licenses/' + license_name):
            raise SystemExit('Missing native license: ' + license_name)
print('APK native payload, pinned TDLib, dependencies, licenses and 16 KiB ELF alignment verified.')
