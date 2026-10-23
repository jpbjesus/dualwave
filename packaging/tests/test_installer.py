from pathlib import Path
import os
import subprocess
import tempfile
import unittest

SOURCE = Path(__file__).resolve().parents[1] / "appimage/setup-driver.sh"
APPRUN = SOURCE.with_name("AppRun")


def headers(modules):
    result = subprocess.run(["bash", str(SOURCE), "--headers", str(modules)], capture_output=True, text=True)
    if result.returncode:
        raise ValueError(result.stderr)
    return result.stdout.splitlines()


class HeadersTest(unittest.TestCase):
    def test_installer_resolves_only_missing_headers_for_installed_kernels(self):
        with tempfile.TemporaryDirectory() as temporary:
            modules = Path(temporary)
            current = modules / "7.2.9-cachyos"
            (current / "build/include/config").mkdir(parents=True)
            (current / "pkgbase").write_text("linux-cachyos\n")
            (current / "build/include/config/auto.conf").touch()
            (current / "build/Module.symvers").touch()
            lts = modules / "6.18-cachyos-lts"
            lts.mkdir()
            (lts / "pkgbase").write_text("linux-cachyos-lts\n")
            self.assertEqual(headers(modules), ["linux-cachyos-lts-headers"])

    def test_unknown_kernel_does_not_invent_a_package_or_run_a_command(self):
        with tempfile.TemporaryDirectory() as temporary:
            kernel = Path(temporary) / "custom"
            kernel.mkdir()
            with self.assertRaises(ValueError):
                headers(Path(temporary))


class AppRunTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.path = Path(self.temporary.name)
        self.tools = self.path / "tools"
        self.tools.mkdir()
        self.app = self.path / "app"
        (self.app / "usr/lib/dualwave").mkdir(parents=True)
        (self.app / "usr/bin").mkdir(parents=True)
        (self.app / "usr/lib/dualwave/driver-package").write_text("dualwave-hid-dkms-0.2.1-2-any.pkg.tar.zst\n")
        self.calls = self.path / "calls"
        self.env = dict(os.environ, APPDIR=str(self.app), PATH=str(self.tools), CALLS=str(self.calls))
        self.stub("sed", "printf 'cachyos\\n'\n")
        self.stub("tr", "/usr/bin/cat\n")
        self.stub("pkexec", "printf 'authorization must not run\\n' >> \"$CALLS\"; exit 99\n")
        self.stub("kdialog", "printf 'dialog %s\\n' \"$*\" >> \"$CALLS\"; exit 1\n")
        launcher = self.app / "usr/bin/dualwave"
        launcher.write_text("#!/usr/bin/bash\nprintf 'app launched\\n' >> \"$CALLS\"\n")
        launcher.chmod(0o755)

    def stub(self, name, body):
        path = self.tools / name
        path.write_text("#!/usr/bin/bash\n" + body)
        path.chmod(0o755)

    def run_app(self, *args):
        return subprocess.run(["/usr/bin/bash", str(APPRUN), *args], env=self.env, text=True, capture_output=True)

    def test_new_package_release_prompts_existing_install_without_authorizing_on_decline(self):
        self.stub("pacman", "printf 'dualwave-hid-dkms 0.2.1-1\\n'\n")
        result = self.run_app()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = self.calls.read_text()
        self.assertIn("--yesno", calls)
        self.assertIn("app launched", calls)
        self.assertNotIn("authorization must not run", calls)

    def test_exact_installed_package_release_opens_without_setup_prompt(self):
        self.stub("pacman", "printf 'dualwave-hid-dkms 0.2.1-2\\n'\n")
        result = self.run_app()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls.read_text(), "app launched\n")

    def test_missing_authorization_tool_is_reported_before_starting_setup(self):
        self.stub("pacman", "exit 0\n")
        (self.tools / "pkexec").unlink()
        result = self.run_app("--setup")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("pkexec", result.stderr)
        self.assertIn("--error", self.calls.read_text())
        self.assertNotIn("authorization must not run", self.calls.read_text())


if __name__ == "__main__":
    unittest.main()
