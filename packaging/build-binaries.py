#!/usr/bin/env python3
"""Build the native Linux release archive; run on the oldest supported host."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--skip-build', action='store_true')
    parser.add_argument('--output', type=Path, default=ROOT / 'target/binaries')
    parser.add_argument('--host-setup', type=Path, help='Optional host setup directory, included without executing it')
    args = parser.parse_args()
    if platform.machine() != 'x86_64':
        parser.error('Only x86_64 archives are supported')
    if not args.skip_build:
        subprocess.run(['cargo', 'build', '--release', '--locked', '-p', 'dsz', '-p', 'dualwave'], cwd=ROOT, check=True)
    target = Path(os.environ.get('CARGO_TARGET_DIR', ROOT / 'target'))
    if not target.is_absolute():
        target = ROOT / target
    if args.host_setup is None:
        subprocess.run(['python3', str(ROOT / 'packaging/build-host-support.py'), '--output', str(ROOT / 'target/host-support')], check=True)
        args.host_setup = ROOT / 'target/host-support/dualwave-host-support'
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory() as temporary:
        stage = Path(temporary)
        (stage / 'bin').mkdir()
        licenses = stage / 'share/licenses/dualwave'
        licenses.mkdir(parents=True)
        for source in [ROOT / 'LICENSE', ROOT / 'vendor/opus/COPYING', *sorted((ROOT / 'crates/dsz/assets').glob('*LICENSE*')), *sorted((ROOT / 'crates/dsz/assets').glob('*OFL*'))]:
            shutil.copy2(source, licenses / source.name)
        (stage / 'libexec').mkdir()
        dependencies = {}
        for source, name in [('dsz', 'dualwave'), ('dualwave', 'dualwave-cli')]:
            binary = target / 'release' / source
            shutil.copy2(binary, stage / ('libexec' if name == 'dualwave' else 'bin') / name)
            result = subprocess.run(['ldd', str(binary)], text=True, capture_output=True, check=True)
            if 'not found' in result.stdout:
                raise RuntimeError('Missing dependencies: ' + result.stdout)
            dependencies[name] = sorted(set(re.findall(r'^\s*(\S+)\s+=>', result.stdout, re.M)))
        (stage / 'dependencies.json').write_text(json.dumps({
            'architecture': 'x86_64', 'build_libc': platform.libc_ver(), 'libraries': dependencies,
            'host_requirements': ['Linux', 'PipeWire for Bluetooth audio', 'FUSE only for AppImage'],
            'note': 'Native shared libraries are provided by the host. Host setup is separate and optional.'
        }, indent=2) + '\n')
        applications = stage / 'share/applications'
        applications.mkdir(parents=True)
        shutil.copy2(ROOT / 'packaging/appimage/dualwave.desktop', applications / 'dualwave.desktop')
        icons = stage / 'share/icons/hicolor/scalable/apps'
        icons.mkdir(parents=True)
        shutil.copy2(ROOT / 'packaging/appimage/dualwave.svg', icons / 'dualwave.svg')
        if args.host_setup:
            shutil.copytree(args.host_setup, stage / 'share/dualwave/host-setup')
        launcher = stage / 'bin/dualwave'
        launcher.write_text('''#!/bin/sh
set -eu
# Resolve the installer symlink so the payload remains relocatable.
self=$(readlink -f -- "$0")
payload=$(CDPATH= cd -- "$(dirname -- "$self")/.." && pwd)
case "${1:-}" in
    --setup) shift; exec pkexec /usr/bin/python3 "$payload/share/dualwave/host-setup/setup-host.py" --install "$@" ;;
    --plan|--check) exec python3 "$payload/share/dualwave/host-setup/setup-host.py" "$@" ;;
esac
exec "$payload/libexec/dualwave" "$@"
''')
        launcher.chmod(0o755)
        archive = args.output / 'DualWave-linux-x86_64.tar.gz'
        with tarfile.open(archive, 'w:gz') as output:
            for path in sorted(stage.iterdir()):
                output.add(path, arcname=path.name)
        (archive.parent / (archive.name + '.sha256')).write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + '  ' + archive.name + '\n')
        print(archive)


if __name__ == '__main__':
    main()
