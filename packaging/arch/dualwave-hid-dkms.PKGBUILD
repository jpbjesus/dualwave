pkgname=dualwave-hid-dkms
pkgver=0.2.1
pkgrel=2
pkgdesc='DualWave DualSense Bluetooth media isolation and privileged USB/IP host support'
arch=('any')
url='https://github.com/jpbjesus/dualwave'
license=('GPL-2.0-or-later')
depends=('dkms' 'clang' 'lld' 'python' 'systemd')
optdepends=('kdialog: graphical AppImage system setup' 'polkit: administrator authorization for AppImage setup')
source=(hid-playstation.c hid-ids.h dualwave-audio.h dualwave-routing.h Makefile dkms.conf dualwave-hid.conf 70-dualwave.rules COPYING upstream.json usbip-host.py dualwave-usbip.service dualwave-usbip.socket dualwave.sysusers dualwave.modules-load)

# The staging builder appends SHA-256 checksums for these exact local sources.
package() {
    local destination="${pkgdir}/usr/src/dualwave-hid-${pkgver}"
    for file in hid-playstation.c hid-ids.h dualwave-audio.h dualwave-routing.h Makefile dkms.conf upstream.json; do
        install -Dm644 "${srcdir}/${file}" "${destination}/${file}"
    done
    install -Dm644 "${srcdir}/dualwave-hid.conf" "${pkgdir}/usr/lib/modprobe.d/dualwave-hid.conf"
    install -Dm644 "${srcdir}/70-dualwave.rules" "${pkgdir}/usr/lib/udev/rules.d/70-dualwave.rules"
    install -Dm644 "${srcdir}/usbip-host.py" "${pkgdir}/usr/lib/dualwave/usbip-host.py"
    for unit in dualwave-usbip.service dualwave-usbip.socket; do
        install -Dm644 "${srcdir}/${unit}" "${pkgdir}/usr/lib/systemd/system/${unit}"
    done
    install -Dm644 "${srcdir}/dualwave.sysusers" "${pkgdir}/usr/lib/sysusers.d/dualwave.conf"
    install -Dm644 "${srcdir}/dualwave.modules-load" "${pkgdir}/usr/lib/modules-load.d/dualwave-usbip.conf"
    install -Dm644 "${srcdir}/COPYING" "${pkgdir}/usr/share/licenses/${pkgname}/COPYING"
}
