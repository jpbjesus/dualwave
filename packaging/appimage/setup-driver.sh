#!/usr/bin/env bash
set -euo pipefail

headers_needed() {
    local modules=$1 kernel base
    [[ -d $modules ]] || { echo 'Kernel module directory not found.' >&2; return 1; }
    for kernel in "$modules"/*; do
        [[ -d $kernel ]] || continue
        if [[ -r $kernel/build/include/config/auto.conf && -r $kernel/build/Module.symvers ]]; then
            continue
        fi
        [[ -r $kernel/pkgbase ]] || { echo "Missing headers for an unknown kernel: ${kernel##*/}" >&2; return 1; }
        read -r base < "$kernel/pkgbase"
        [[ $base =~ ^linux[a-z0-9+_.-]*$ ]] || { echo 'Unsupported kernel package name.' >&2; return 1; }
        printf '%s-headers\n' "$base"
    done | sort -u
}

if [[ ${1:-} == --headers ]]; then
    headers_needed "${2:-/usr/lib/modules}"
    exit
fi
case ${1:-} in
    --check|--plan|--install) ;;
    *) echo 'Use --check, --plan or the AppImage setup action.' >&2; exit 1 ;;
esac

payload=${2:-$(cd -- "$(dirname -- "$0")" && pwd)}
read -r package < "$payload/driver-package"
read -r digest < "$payload/driver.sha256"
read -r version < "$payload/driver-version"
[[ $package =~ ^dualwave-hid-dkms-[0-9][A-Za-z0-9._+-]*-any\.pkg\.tar\.zst$ ]] || { echo 'Invalid package filename.' >&2; exit 1; }
[[ $digest =~ ^[a-f0-9]{64}$ && $version =~ ^[0-9][0-9.]*$ ]] || { echo 'Invalid package manifest.' >&2; exit 1; }
actual=$(sha256sum -- "$payload/$package")
[[ ${actual%% *} == "$digest" ]] || { echo 'Driver package checksum failed.' >&2; exit 1; }

if [[ ${1:-} == --check ]]; then
    printf 'Driver payload verified: %s\n' "$package"
    exit
fi
os_id=$(sed -n 's/^ID=//p' /etc/os-release | tr -d '"')
[[ $os_id == cachyos ]] || { echo 'Automatic driver setup currently supports CachyOS.' >&2; exit 1; }
header_lines=$(headers_needed /usr/lib/modules)
if [[ ${1:-} == --plan ]]; then
    printf 'Driver: %s\nMissing headers: %s\n' "$package" "${header_lines:-none}"
    printf 'Install via pacman; its DKMS, udev and initramfs hooks apply the configuration. Reboot afterwards.\n'
    exit
fi
[[ ${1:-} == --install && $EUID -eq 0 ]] || { echo 'Use the AppImage setup action to authorize installation.' >&2; exit 1; }
install_uid=${PKEXEC_UID:-${SUDO_UID:-}}
[[ $install_uid =~ ^[0-9]{1,10}$ && $install_uid -gt 0 ]] || { echo 'Could not identify the desktop user for DualWave setup.' >&2; exit 1; }
install_user=$(getent passwd "$install_uid" | cut -d: -f1)
[[ -n $install_user ]] || { echo 'Desktop user was not found.' >&2; exit 1; }
if [[ -n $header_lines ]]; then
    mapfile -t header_packages <<< "$header_lines"
    pacman -S --needed --noconfirm "${header_packages[@]}"
fi
pacman -U --needed --noconfirm -- "$payload/$package"
status=$(dkms status -m dualwave-hid -v "$version")
for kernel in /usr/lib/modules/*; do
    [[ -r $kernel/pkgbase && -r $kernel/build/include/config/auto.conf ]] || continue
    found=false
    while IFS= read -r line; do
        if [[ $line == "dualwave-hid/$version, ${kernel##*/}, "*": installed"* ]]; then
            found=true
            break
        fi
    done <<< "$status"
    if [[ $found != true ]]; then
        printf 'Driver was not installed for %s. Check the DKMS build log before rebooting.\n' "${kernel##*/}" >&2
        exit 1
    fi
done
systemd-sysusers /usr/lib/sysusers.d/dualwave.conf
usermod -a -G dualwave -- "$install_user"
systemctl daemon-reload
systemctl enable dualwave-usbip.socket
printf 'DualWave system support installed. Reboot to activate it and the new group permissions.\n'
