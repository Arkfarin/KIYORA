#!/usr/bin/env python3
"""Record a staged native package and emit the Tauri resource overlay."""
import argparse
import hashlib
import json
import pathlib
import re
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('package', type=pathlib.Path)
parser.add_argument('--platform', choices=['linux-x86_64', 'windows-x86_64'], required=True)
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parent.parent
package = args.package.resolve()
pins = json.loads((root / 'scripts/native-sources.json').read_text())
name = 'libtdjson.so' if args.platform.startswith('linux') else 'tdjson.dll'
library = package / 'native' / name
if args.platform.startswith('linux'):
    elf = subprocess.check_output(['readelf', '-h', '-d', str(library)], text=True)
    if not re.search(r'Machine:\s+Advanced Micro Devices X86-64', elf):
        raise SystemExit('TDLib is not Linux x86_64')
    dependencies = set(re.findall(r'Shared library: \[([^]]+)\]', elf))
    allowed = {'libc.so.6', 'libm.so.6', 'libdl.so.2', 'libpthread.so.0', 'librt.so.1', 'libstdc++.so.6', 'libgcc_s.so.1', 'ld-linux-x86-64.so.2'}
    if dependencies - allowed:
        raise SystemExit('Unexpected TDLib dynamic dependencies: ' + str(dependencies - allowed))
    linkage = subprocess.check_output(['ldd', str(library)], text=True)
    if 'not found' in linkage:
        raise SystemExit('A TDLib system dependency is missing: ' + linkage)
else:
    pe = subprocess.check_output(['dumpbin', '/headers', '/dependents', str(library)], text=True)
    if '8664 machine (x64)' not in pe:
        raise SystemExit('TDLib is not Windows x86_64')
    dependencies = set(re.findall(r'^\s+([\w.-]+\.dll)\s*$', pe, re.MULTILINE | re.IGNORECASE))
    allowed = {'kernel32.dll', 'user32.dll', 'advapi32.dll', 'ws2_32.dll', 'mswsock.dll', 'psapi.dll', 'crypt32.dll', 'bcrypt.dll', 'ntdll.dll', 'iphlpapi.dll', 'shell32.dll', 'ole32.dll', 'secur32.dll', 'normaliz.dll', 'ucrtbase.dll'}
    missing = {dll for dll in dependencies if dll.lower() not in allowed and not dll.lower().startswith('api-ms-win-')}
    if missing:
        raise SystemExit('Unbundled TDLib DLL dependencies: ' + str(missing))
for name in ['TDLib.txt', 'OpenSSL.txt', 'zlib.txt']:
    if not (package / 'licenses' / name).read_bytes():
        raise SystemExit('Missing license: ' + name)
with library.open('rb') as source:
    digest = hashlib.file_digest(source, 'sha256').hexdigest()
manifest = {'platform': args.platform, 'tdlib_version': pins['tdlib']['version'],
            'tdlib_revision': pins['tdlib']['revision'], 'sha256': digest,
            'dependencies': sorted(dependencies), 'static_dependencies': {name: pins[name]['version'] for name in ['openssl', 'zlib']}}
(package / 'native/manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
overlay = {'bundle': {'resources': {'licenses/': 'licenses/', 'target/native/desktop/native/': 'native/', 'target/native/desktop/licenses/': 'licenses/native/'}}}
(package / 'tauri-bundle.json').write_text(json.dumps(overlay, indent=2) + '\n')
print('Staged native package and dependency checks passed:', library)
