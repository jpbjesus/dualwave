#!/usr/bin/env python3
"""Stage a source-only host setup payload, without privileged actions."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil

ROOT = Path(__file__).resolve().parents[1]


def build(destination):
    destination.mkdir(parents=True, exist_ok=True)
    sources = [ROOT / 'packaging/kernel' / name for name in [
        'dualwave-audio.h', 'dualwave-routing.h', 'Makefile', 'dkms.conf', 'dualwave-hid.conf', 'COPYING']]
    sources += [ROOT / 'packaging/kernel/source' / name for name in ['hid-playstation.c', 'hid-ids.h', 'upstream.json']]
    sources += [ROOT / 'packaging/linux' / name for name in [
        'setup-host.py', 'usbip-host.py', 'dualwave-usbip.service', 'dualwave-usbip.socket', 'dualwave.sysusers', 'dualwave.modules-load']]
    sources.append(ROOT / 'packaging/70-dualwave.rules')
    for source in sources:
        shutil.copy2(source, destination / source.name)
    version = re.search(r'PACKAGE_VERSION="([0-9.]+)"', (destination / 'dkms.conf').read_text())[1]
    (destination / 'metadata.json').write_text(json.dumps({'module': 'dualwave-hid', 'version': version,
        'status': 'Experimental outside CachyOS; module compilation and hardware compatibility require validation.'}, indent=2) + '\n')
    files = sorted([source.name for source in sources] + ['metadata.json'])
    (destination / 'manifest.sha256').write_text(''.join(
        hashlib.sha256((destination / name).read_bytes()).hexdigest() + '  ' + name + '\n' for name in files))
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=ROOT / 'target/host-support')
    args = parser.parse_args()
    print(build(args.output / 'dualwave-host-support'))


if __name__ == '__main__':
    main()
