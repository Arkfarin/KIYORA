#!/usr/bin/env python3
"""Fetch reviewed native source archives; verify before extracting or executing."""
import hashlib
import json
import pathlib
import subprocess
import sys
import tarfile

root = pathlib.Path(__file__).resolve().parent.parent
pins = json.loads((root / 'scripts/native-sources.json').read_text())
destination = root / 'target/native'
destination.mkdir(parents=True, exist_ok=True)
for name in sys.argv[1:] or pins:
    pin = pins[name]
    archive = destination / pin['archive']
    if not archive.is_file():
        partial = archive.with_suffix(archive.suffix + '.part')
        subprocess.run(['curl', '--fail', '--location', '--retry', '3', pin['url'], '-o', str(partial)], check=True)
        partial.replace(archive)
    with archive.open('rb') as source:
        digest = hashlib.file_digest(source, 'sha256').hexdigest()
    if digest != pin['sha256']:
        raise SystemExit('Native source checksum mismatch: ' + str(archive))
    source = destination / pin['directory']
    marker = source / '.kiyora-archive-sha256'
    if not marker.is_file() or marker.read_text() != digest:
        with tarfile.open(archive) as contents:
            if any(pathlib.PurePosixPath(item.name).parts[0] != pin['directory'] for item in contents):
                raise SystemExit('Unexpected native source archive root: ' + str(archive))
            contents.extractall(destination, filter='data')
        marker.write_text(digest)
    print(name + ': verified ' + pin['version'])
