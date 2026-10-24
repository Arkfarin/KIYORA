#!/usr/bin/env python3
"""Verify an extracted/installed desktop payload, without a Telegram account."""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--resources', type=pathlib.Path, required=True)
parser.add_argument('--executable', type=pathlib.Path, required=True)
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parent.parent
manifest = json.loads((args.resources / 'native/manifest.json').read_text())
pins = json.loads((root / 'scripts/native-sources.json').read_text())
if (manifest['tdlib_version'], manifest['tdlib_revision']) != (pins['tdlib']['version'], pins['tdlib']['revision']):
    raise SystemExit('Packaged TDLib identity differs from the reviewed pin')
expected_platform = 'windows-x86_64' if os.name == 'nt' else 'linux-x86_64'
if manifest['platform'] != expected_platform:
    raise SystemExit('Packaged TDLib platform does not match this test host')
library = (args.resources / 'native' / ('tdjson.dll' if os.name == 'nt' else 'libtdjson.so')).resolve()
with library.open('rb') as source:
    digest = hashlib.file_digest(source, 'sha256').hexdigest()
if digest != manifest['sha256']:
    raise SystemExit('Installed TDLib digest differs from the staged binary')
if not args.executable.is_file():
    raise SystemExit('Installed application executable is missing')
for name in ['TDLib.txt', 'OpenSSL.txt', 'zlib.txt']:
    if not (args.resources / 'licenses/native' / name).read_bytes():
        raise SystemExit('Missing native dependency license: ' + name)
for name in ['libopus-COPYING', 'opusic-sys-LICENSE', 'opus-rs-LICENSE-MIT', 'opus-rs-LICENSE-APACHE']:
    if not (args.resources / 'licenses' / name).read_bytes():
        raise SystemExit('Missing decoder license: ' + name)
if os.name != 'nt':
    for binary in [args.executable, library]:
        linkage = subprocess.check_output(['ldd', str(binary.resolve())], text=True)
        if 'not found' in linkage:
            raise SystemExit('Missing installed dependency: ' + linkage)
# Run the real Rust adapter against the file from the package, including reopen
# and close. Use the explicit path, never a developer's KIYORA_TDJSON override.
environment = dict(os.environ, KIYORA_TDJSON=str(library))
subprocess.run(['cargo', 'test', '--locked', '-p', 'vault-telegram', '--test', 'real_tdlib', '--', '--ignored'],
               cwd=root, env=environment, check=True)
print('Installed native payload, licenses and credential-free TDLib lifecycle passed.')
