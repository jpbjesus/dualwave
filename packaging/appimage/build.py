#!/usr/bin/env python3
"""Build a CachyOS AppImage from the native release executables."""
from pathlib import Path
import argparse
import hashlib
import os
import re
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[2]
EXCLUDED = re.compile(r'^(ld-linux|lib(c|m|pthread|dl|rt|resolv|util)\.so|lib(GL|EGL|GLX|OpenGL|GLdispatch|vulkan|drm|gbm))')

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--appimagetool', type=Path, required=True)
    args = parser.parse_args()
    subprocess.run(['cargo', 'build', '--release', '--locked', '-p', 'dsz', '-p', 'dualwave'], cwd=ROOT, check=True)
    subprocess.run(['python3', 'packaging/build-kernel-package.py'], cwd=ROOT, check=True)
    listing = subprocess.run(['makepkg', '--packagelist'], cwd=ROOT / 'target/packages/dualwave-hid-dkms',
                             check=True, text=True, capture_output=True)
    packages = listing.stdout.splitlines()
    if len(packages) != 1:
        raise RuntimeError('Expected exactly one freshly built DualWave system package')
    package = Path(packages[0])
    if not package.is_absolute():
        package = ROOT / 'target/packages/dualwave-hid-dkms' / package
    if not package.is_file():
        raise RuntimeError(f'The freshly built DualWave system package is missing: {package}')
    app = ROOT / 'target/appimage/DualWave.AppDir'
    if app.exists():
        shutil.rmtree(app)
    binary = app / 'usr/bin'
    library = app / 'usr/lib'
    payload = library / 'dualwave'
    payload.mkdir(parents=True)
    binary.mkdir(parents=True)
    shutil.copy2(ROOT / 'target/release/dsz', binary / 'dualwave')
    shutil.copy2(ROOT / 'target/release/dualwave', binary / 'dualwave-cli')
    for name in ['AppRun', 'dualwave.desktop', 'dualwave.svg']:
        shutil.copy2(ROOT / 'packaging/appimage' / name, app / name)
    shutil.copy2(app / 'dualwave.svg', app / '.DirIcon')
    shutil.copy2(ROOT / 'packaging/appimage/setup-driver.sh', payload / 'setup-driver.sh')
    shutil.copy2(package, payload / package.name)
    (payload / 'driver-package').write_text(package.name + '\n')
    (payload / 'driver-version').write_text('0.2.1\n')
    (payload / 'driver.sha256').write_text(hashlib.sha256(package.read_bytes()).hexdigest() + '\n')
    for name in ['pipewire-0.3', 'spa-0.2']:
        shutil.copytree(Path('/usr/lib') / name, library / name, symlinks=False)
    config = app / 'usr/share/pipewire'
    config.mkdir(parents=True)
    shutil.copy2('/usr/share/pipewire/client.conf', config / 'client.conf')
    # Bundle userspace libraries and plugins, preserving host graphics drivers/glibc.
    queue = [*binary.iterdir(), *library.rglob('*.so')]
    seen = set()
    bundled_sources = set()
    while queue:
        file = queue.pop()
        result = subprocess.run(['ldd', str(file)], text=True, capture_output=True)
        if result.returncode or 'not found' in result.stdout:
            raise RuntimeError(f'Could not resolve shared libraries for {file}: {result.stdout}{result.stderr}')
        for line in result.stdout.splitlines():
            match = re.search(r'=> (/\S+)', line)
            if not match:
                continue
            source = Path(match[1])
            if source.name in seen or EXCLUDED.match(source.name):
                continue
            seen.add(source.name)
            bundled_sources.add(source)
            destination = library / source.name
            shutil.copy2(source, destination, follow_symlinks=True)
            queue.append(destination)
    licenses = app / 'usr/share/licenses/dualwave'
    licenses.mkdir(parents=True)
    shutil.copy2(ROOT / 'LICENSE', licenses / 'LICENSE')
    shutil.copy2(ROOT / 'packaging/kernel/COPYING', licenses / 'kernel-COPYING')
    shutil.copy2(ROOT / 'vendor/opus/COPYING', licenses / 'opus-COPYING')
    owners = set()
    for source in bundled_sources:
        owner = subprocess.run(['pacman', '-Qqo', str(source)], capture_output=True, text=True, check=True)
        owners.update(owner.stdout.splitlines())
    for owner in owners:
        directory = Path('/usr/share/licenses') / owner
        if directory.is_dir():
            shutil.copytree(directory, licenses / owner, symlinks=False, dirs_exist_ok=True)
    output = ROOT / 'target/appimage/DualWave-x86_64.AppImage'
    env = dict(os.environ, ARCH='x86_64', APPIMAGE_EXTRACT_AND_RUN='1')
    subprocess.run([str(args.appimagetool.resolve()), str(app), str(output)], env=env, check=True)
    output.with_suffix('.AppImage.sha256').write_text(hashlib.sha256(output.read_bytes()).hexdigest() + '  ' + output.name + '\n')
    print(output)

if __name__ == '__main__':
    main()
