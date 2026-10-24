#!/usr/bin/env python3
"""Extract a locally built installer and verify its actual native payload."""
import argparse
import pathlib
import subprocess
import sys
import tarfile
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument('installer', type=pathlib.Path)
args = parser.parse_args()
installer = args.installer.resolve()
root = pathlib.Path(__file__).resolve().parent.parent
with tempfile.TemporaryDirectory(prefix='kiyora-installer-') as temporary:
    destination = pathlib.Path(temporary)
    if installer.suffix == '.deb':
        members = subprocess.check_output(['ar', 't', str(installer)], text=True).splitlines()
        data = [name for name in members if name.startswith('data.tar')]
        if len(data) != 1:
            raise SystemExit('Debian package must contain one data archive')
        archive = destination / data[0]
        with archive.open('wb') as output:
            subprocess.run(['ar', 'p', str(installer), data[0]], stdout=output, check=True)
        with tarfile.open(archive) as contents:
            contents.extractall(destination, filter='data')
        executable = destination / 'usr/bin/kiyora'
        resources = destination / 'usr/lib/kiyora'
    elif installer.suffix == '.msi':
        result = subprocess.run(['msiexec', '/a', str(installer), '/qn', 'TARGETDIR=' + str(destination)])
        if result.returncode not in (0, 3010):
            raise SystemExit('MSI administrative extraction failed: ' + str(result.returncode))
        executables = list(destination.rglob('kiyora.exe'))
        if len(executables) != 1:
            raise SystemExit('MSI must contain exactly one application executable')
        executable = executables[0]
        resources = executable.parent
    else:
        raise SystemExit('Expected a .deb or .msi installer')
    subprocess.run([sys.executable, str(root / 'scripts/verify-desktop-package.py'),
                    '--resources', str(resources), '--executable', str(executable)], check=True)
