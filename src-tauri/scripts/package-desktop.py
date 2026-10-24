#!/usr/bin/env python3
"""Build a desktop installer containing the reviewed native TDLib package."""
import argparse
import os
import pathlib
import shutil
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument('--debug', action='store_true', help='For local packaging checks; release is the default')
parser.add_argument('--bundles', help='Tauri bundle targets; defaults to deb on Linux, msi on Windows')
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parent.parent
windows = os.name == 'nt'
prepare = ['powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', 'scripts/prepare-windows-native.ps1'] if windows else ['bash', 'scripts/prepare-linux-native.sh']
subprocess.run(prepare, cwd=root, check=True)
library = root / 'target/native/desktop/native' / ('tdjson.dll' if windows else 'libtdjson.so')
subprocess.run(['cargo', 'test', '--locked', '-p', 'vault-telegram', '--test', 'real_tdlib', '--', '--ignored'],
               cwd=root, env=dict(os.environ, KIYORA_TDJSON=str(library)), check=True)
pnpm = shutil.which('pnpm')
if pnpm is None:
    raise SystemExit('Install pnpm and the existing frontend dependencies first')
command = [pnpm, 'tauri', 'build', '--config', 'src-tauri/target/native/desktop/tauri-bundle.json',
           '--bundles', args.bundles or ('msi' if windows else 'deb'), '--', '--locked']
if args.debug:
    command.insert(3, '--debug')
subprocess.run(command, cwd=root.parent, check=True)
