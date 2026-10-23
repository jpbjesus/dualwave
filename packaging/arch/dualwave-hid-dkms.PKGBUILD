pkgname=dualwave-hid-dkms
pkgver=0.2.0
pkgrel=1
pkgdesc='DualSense Bluetooth media isolation for DualWave, preserving physical gamepad input'
arch=('any')
url='https://github.com/jpbjesus/dualwave'
license=('GPL-2.0-or-later')
depends=('dkms')
optdepends=('clang: build against LLVM-based kernel headers' 'lld: link modules for LLVM-based kernels')
source=(hid-playstation.c hid-ids.h dualwave-audio.h dualwave-routing.h Makefile dkms.conf dualwave-hid.conf 70-dualwave.rules COPYING upstream.json)

# The staging builder appends SHA-256 checksums for these exact local sources.
package() {
    local destination="${pkgdir}/usr/src/dualwave-hid-${pkgver}"
    for file in hid-playstation.c hid-ids.h dualwave-audio.h dualwave-routing.h Makefile dkms.conf upstream.json; do
        install -Dm644 "${srcdir}/${file}" "${destination}/${file}"
    done
    install -Dm644 "${srcdir}/dualwave-hid.conf" "${pkgdir}/usr/lib/modprobe.d/dualwave-hid.conf"
    install -Dm644 "${srcdir}/70-dualwave.rules" "${pkgdir}/usr/lib/udev/rules.d/70-dualwave.rules"
    install -Dm644 "${srcdir}/COPYING" "${pkgdir}/usr/share/licenses/${pkgname}/COPYING"
}
