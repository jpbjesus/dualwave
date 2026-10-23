#!/usr/bin/env python3
"""Stage a checksummed, source-only Arch DKMS package without root or downloads."""
from pathlib import Path
import hashlib
import shutil
import subprocess


def main():
    root = Path(__file__).resolve().parent.parent
    stage = root / "target/packages/dualwave-hid-dkms"
    stage.mkdir(parents=True, exist_ok=True)
    kernel = root / "packaging/kernel"
    sources = [
        kernel / "source/hid-playstation.c", kernel / "source/hid-ids.h",
        kernel / "dualwave-audio.h", kernel / "dualwave-routing.h",
        kernel / "Makefile", kernel / "dkms.conf", kernel / "dualwave-hid.conf",
        root / "packaging/70-dualwave.rules", kernel / "COPYING", kernel / "source/upstream.json",
    ]
    checksums = []
    for source in sources:
        shutil.copy2(source, stage / source.name)
        checksums.append(hashlib.sha256(source.read_bytes()).hexdigest())
    recipe = (root / "packaging/arch/dualwave-hid-dkms.PKGBUILD").read_text()
    recipe += "\nsha256sums=(\n" + "".join(f"    '{value}'\n" for value in checksums) + ")\n"
    (stage / "PKGBUILD").write_text(recipe)
    subprocess.run(["makepkg", "--nodeps", "--nocheck", "--cleanbuild", "--force"], cwd=stage, check=True)
    for package in sorted(stage.glob("*.pkg.tar.zst")):
        print(f"Built: {package}")
        print(f"SHA-256: {hashlib.sha256(package.read_bytes()).hexdigest()}")


if __name__ == "__main__":
    main()
