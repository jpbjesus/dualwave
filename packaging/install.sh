#!/bin/sh
# Download this script from the desired release, then run it with --version vX.Y.Z.
exec python3 - "$@" <<'PY'
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import shlex
import shutil
import sys
import tarfile
import tempfile
import urllib.request

OWNER = 'dualwave-user-install-v1'
TAG = re.compile(r'v[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9][A-Za-z0-9.-]*)?\Z')

def fetch(url):
    with urllib.request.urlopen(url, timeout=120) as response:
        if not response.url.startswith('https://'):
            raise ValueError('Download redirected outside HTTPS')
        return response.read()

def managed(path):
    try:
        return not path.is_symlink() and json.loads((path / 'install.json').read_text())['owner'] == OWNER
    except (OSError, ValueError, KeyError):
        return False

def main(argv=None):
    parser = argparse.ArgumentParser(description='Install DualWave in a user-owned prefix; never installs drivers.')
    parser.add_argument('--version')
    parser.add_argument('--format', choices=['appimage', 'native'], default='appimage')
    parser.add_argument('--prefix', type=Path, default=Path.home() / '.local')
    parser.add_argument('--dry-run', action='store_true')
    parser.add_argument('--uninstall', action='store_true')
    args = parser.parse_args(argv)
    if os.geteuid() == 0:
        parser.error('Run this installer as your regular user, without sudo')
    if not args.uninstall and (not args.version or not TAG.fullmatch(args.version)):
        parser.error('--version must be an explicit tag such as v0.2.1')
    if platform.machine() not in ('x86_64', 'amd64'):
        parser.error('Only x86_64 releases are supported')
    prefix = args.prefix.expanduser().absolute()
    if '\n' in str(prefix) or '\r' in str(prefix):
        parser.error('Prefix cannot contain line breaks')
    home = Path.home().resolve()
    if prefix.resolve() != home and home not in prefix.resolve().parents:
        parser.error('--prefix must be inside your home directory')
    root = prefix / 'lib/dualwave-user'
    links = {prefix / 'bin/dualwave': root / 'current/bin/dualwave',
             prefix / 'bin/dualwave-cli': root / 'current/bin/dualwave-cli',
             prefix / 'share/applications/dualwave-user.desktop': root / 'current/dualwave.desktop'}
    for path, target in links.items():
        if os.path.lexists(path) and (not path.is_symlink() or os.readlink(path) != str(target)):
            raise ValueError('Refusing to replace unrelated file: ' + str(path))
    if os.path.lexists(root) and not managed(root):
        raise ValueError('Refusing unmanaged installation: ' + str(root))
    if args.uninstall:
        if args.dry_run:
            print('Would remove managed installation at', root)
            return
        if managed(root):
            for path in links:
                if path.is_symlink():
                    path.unlink()
            shutil.rmtree(root)
        print('Removed managed DualWave installation; settings preserved')
        return
    asset = 'DualWave-x86_64.AppImage' if args.format == 'appimage' else 'DualWave-linux-x86_64.tar.gz'
    url = 'https://github.com/jpbjesus/dualwave/releases/download/' + args.version + '/' + asset
    if args.dry_run:
        print('Would verify and install', url, 'in', root)
        return
    data = fetch(url)
    checksums = fetch(url + '.sha256').decode('ascii').splitlines()
    expected = [line.split()[0] for line in checksums if len(line.split()) == 2 and line.split()[1].lstrip('*') == asset]
    if len(expected) != 1 or not re.fullmatch('[0-9a-fA-F]{64}', expected[0]) or hashlib.sha256(data).hexdigest() != expected[0].lower():
        raise ValueError('SHA-256 verification failed; existing installation preserved')
    root.parent.mkdir(parents=True, exist_ok=True)
    if not root.exists():
        root.mkdir()
        (root / 'install.json').write_text(json.dumps({'owner': OWNER}))
    with tempfile.TemporaryDirectory(prefix='.staging-', dir=root) as temporary:
        stage = Path(temporary) / 'release'
        stage.mkdir()
        if args.format == 'appimage':
            (stage / 'bin').mkdir()
            (stage / 'bin/dualwave').write_bytes(data)
            (stage / 'bin/dualwave').chmod(0o755)
            (stage / 'bin/dualwave-cli').write_text('#!/bin/sh\nexec ' + shlex.quote(str(root / 'current/bin/dualwave')) + ' --cli "$@"\n')
            (stage / 'bin/dualwave-cli').chmod(0o755)
        else:
            with tarfile.open(fileobj=io.BytesIO(data), mode='r:gz') as archive:
                for member in archive.getmembers():
                    parts = Path(member.name).parts
                    if member.name.startswith('/') or '..' in parts or not (member.isdir() or member.isfile()):
                        raise ValueError('Unsafe archive member')
                archive.extractall(stage, filter='data')
            for name in ('dualwave', 'dualwave-cli'):
                binary = stage / 'bin' / name
                if not binary.is_file() or not os.access(binary, os.X_OK):
                    raise ValueError('Archive is missing executable ' + name)
        executable = str(prefix / 'bin/dualwave')
        quoted = '"' + executable.replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$') + '"'
        (stage / 'dualwave.desktop').write_text('[Desktop Entry]\nType=Application\nName=DualWave\nExec=' + quoted + '\nIcon=input-gaming\nTerminal=false\nCategories=Settings;HardwareSettings;\n')
        release = root / ('release-' + args.version + '-' + args.format + '-' + os.urandom(6).hex())
        stage.rename(release)
        created_links = []
        try:
            for path, target in links.items():
                path.parent.mkdir(parents=True, exist_ok=True)
                if not path.is_symlink():
                    path.symlink_to(target)
                    created_links.append(path)
            (root / '.next').symlink_to(release.name)
            os.replace(root / '.next', root / 'current')
        except OSError:
            for path in created_links:
                path.unlink()
            (root / '.next').unlink(missing_ok=True)
            shutil.rmtree(release)
            raise
    print('Installed', args.version, 'at', root, '; add', prefix / 'bin', 'to PATH')

if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, tarfile.TarError) as error:
        sys.exit(str(error))
PY
