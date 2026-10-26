#!/usr/bin/env python3
"""Explicit host setup. Planning and checksum checks never modify the system."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shlex
import shutil
import subprocess
import sys

SOURCE_FILES = ('hid-playstation.c', 'hid-ids.h', 'dualwave-audio.h', 'dualwave-routing.h', 'Makefile', 'dkms.conf', 'upstream.json')
SYSTEM_FILES = {
    'dualwave-hid.conf': '/usr/lib/modprobe.d/dualwave-hid.conf',
    '70-dualwave.rules': '/usr/lib/udev/rules.d/70-dualwave.rules',
    'usbip-host.py': '/usr/lib/dualwave/usbip-host.py',
    'dualwave-usbip.service': '/usr/lib/systemd/system/dualwave-usbip.service',
    'dualwave-usbip.socket': '/usr/lib/systemd/system/dualwave-usbip.socket',
    'dualwave.sysusers': '/usr/lib/sysusers.d/dualwave.conf',
    'dualwave.modules-load': '/usr/lib/modules-load.d/dualwave-usbip.conf',
    'COPYING': '/usr/share/licenses/dualwave-host/COPYING',
}


def os_info(path):
    values = {}
    for line in path.read_text().splitlines():
        if '=' in line and not line.startswith('#'):
            key, value = line.split('=', 1)
            parts = shlex.split(value)
            values[key] = ' '.join(parts)
    return values


def family(info):
    identities = [info.get('ID', ''), *info.get('ID_LIKE', '').split()]
    for candidate in identities:
        if candidate in ('arch', 'cachyos', 'endeavouros', 'manjaro'): return 'arch'
        if candidate in ('debian', 'ubuntu', 'linuxmint', 'pop'): return 'debian'
        if candidate == 'fedora': return 'fedora'
    raise ValueError('Unsupported distribution; use the documented manual host setup.')


def verify(payload):
    expected_names = set(SOURCE_FILES) | set(SYSTEM_FILES) | {'setup-host.py', 'metadata.json'}
    names = set()
    for line in (payload / 'manifest.sha256').read_text().splitlines():
        digest, name = line.split('  ', 1)
        if not re.fullmatch('[a-f0-9]{64}', digest) or name not in expected_names or name in names:
            raise ValueError('Invalid host manifest')
        path = payload / name
        if path.is_symlink() or not path.is_file() or hashlib.sha256(path.read_bytes()).hexdigest() != digest:
            raise ValueError('Host payload checksum failed: ' + name)
        names.add(name)
    if names != expected_names: raise ValueError('Incomplete host payload')
    metadata = json.loads((payload / 'metadata.json').read_text())
    if metadata.get('module') != 'dualwave-hid' or not re.fullmatch(r'[0-9]+(?:\.[0-9]+)+', metadata.get('version', '')):
        raise ValueError('Invalid module metadata')
    return metadata


def plan(info, kernel, modules):
    distro = family(info)
    if not re.fullmatch(r'[A-Za-z0-9._+-]+', kernel): raise ValueError('Invalid kernel release')
    headers_ready = (modules / kernel / 'build/include/config/auto.conf').is_file() and (modules / kernel / 'build/Module.symvers').is_file()
    if distro == 'arch':
        packages = ['dkms', 'clang', 'lld', 'make', 'gcc', 'python', 'systemd']
        if not headers_ready:
            base = (modules / kernel / 'pkgbase').read_text().strip()
            if not re.fullmatch(r'linux[a-z0-9+_.-]*', base): raise ValueError('Unknown kernel package; install its matching headers manually')
            packages.append(base + '-headers')
        commands = [['pacman', '-S', '--needed', '--noconfirm', *packages]]
    elif distro == 'debian':
        packages = ['dkms', 'build-essential', 'clang', 'lld', 'python3', 'kmod', 'systemd']
        if not headers_ready: packages.append('linux-headers-' + kernel)
        commands = [['apt-get', 'update'], ['apt-get', 'install', '-y', *packages]]
    else:
        packages = ['dkms', 'make', 'gcc', 'clang', 'lld', 'python3', 'kmod', 'systemd']
        if not headers_ready: packages.append('kernel-devel-' + kernel)
        commands = [['dnf', 'install', '-y', *packages]]
    return dict(distribution=distro, kernel=kernel, headers_ready=headers_ready, commands=commands,
        experimental=distro != 'arch' or info.get('ID') != 'cachyos',
        activation='Reboot required; loaded modules and active controllers are not changed.')


def install(payload, metadata, setup, uid):
    if os.geteuid() != 0: raise ValueError('Host setup requires explicit administrator authorization (sudo or pkexec).')
    if not uid.isdigit() or int(uid) <= 0: raise ValueError('Identify the desktop user with SUDO_UID, PKEXEC_UID or --user-uid.')
    user = subprocess.check_output(['getent', 'passwd', uid], text=True).split(':')[0]
    if not user: raise ValueError('Desktop user not found')
    version = metadata['version']
    destination = Path('/usr/src/dualwave-hid-' + version)
    # A distribution package owns its source directory; never overwrite it.
    marker = destination / '.dualwave-host-setup'
    if destination.exists() and not marker.is_file():
        raise ValueError('Existing module sources are package-managed; update or remove that package first.')
    for command in setup['commands']: subprocess.run(command, check=True)
    kernel = setup['kernel']
    headers = Path('/usr/lib/modules') / kernel / 'build'
    if not (headers / 'include/config/auto.conf').is_file() or not (headers / 'Module.symvers').is_file():
        raise ValueError('Matching kernel build headers are still unavailable; no module was installed.')
    destination.mkdir(parents=True, exist_ok=True)
    for name in SOURCE_FILES: shutil.copy2(payload / name, destination / name)
    marker.write_text('dualwave-host-setup\n')
    # Stop before changing host configuration if compilation cannot succeed.
    subprocess.run(['dkms', 'build', '-m', 'dualwave-hid', '-v', version, '-k', kernel], check=True)
    subprocess.run(['dkms', 'install', '-m', 'dualwave-hid', '-v', version, '-k', kernel], check=True)
    for name, target in SYSTEM_FILES.items():
        path = Path(target)
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(payload / name, path)
        path.chmod(0o644)
    subprocess.run(['systemd-sysusers', '/usr/lib/sysusers.d/dualwave.conf'], check=True)
    subprocess.run(['usermod', '-a', '-G', 'dualwave', user], check=True)
    subprocess.run(['depmod', '-a', kernel], check=True)
    if setup['distribution'] == 'arch': command = ['mkinitcpio', '-P']
    elif setup['distribution'] == 'debian': command = ['update-initramfs', '-u', '-k', kernel]
    else: command = ['dracut', '--force', '--kver', kernel]
    subprocess.run(command, check=True)
    subprocess.run(['systemctl', 'daemon-reload'], check=True)
    subprocess.run(['systemctl', 'enable', 'dualwave-usbip.socket'], check=True)
    print('Host support installed. Reboot to activate the module and group membership.')
    print('Secure Boot systems may require enrolling the DKMS signing key before the module can load.')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    for name in ('plan', 'check', 'install'): mode.add_argument('--' + name, action='store_true')
    parser.add_argument('--payload', type=Path, default=Path(__file__).resolve().parent)
    parser.add_argument('--os-release', type=Path, default=Path('/etc/os-release'), help='Plan only: distribution fixture')
    parser.add_argument('--modules', type=Path, default=Path('/usr/lib/modules'), help='Plan only: kernel fixture')
    parser.add_argument('--kernel', help='Plan only: kernel release fixture')
    parser.add_argument('--user-uid', default=os.environ.get('PKEXEC_UID', os.environ.get('SUDO_UID', '')))
    args = parser.parse_args(argv)
    if args.install and (args.os_release != Path('/etc/os-release') or args.modules != Path('/usr/lib/modules') or args.kernel):
        parser.error('Installation uses the actual running host; fixture overrides are plan-only.')
    metadata = verify(args.payload)
    if args.check:
        print('Host source payload verified: dualwave-hid ' + metadata['version'])
        return 0
    setup = plan(os_info(args.os_release), args.kernel or os.uname().release, args.modules)
    if args.plan:
        print(json.dumps(setup, indent=2))
        return 0
    install(args.payload, metadata, setup, args.user_uid)
    return 0


if __name__ == '__main__':
    try: raise SystemExit(main())
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print('Host setup failed: ' + str(error), file=sys.stderr)
        raise SystemExit(1)
