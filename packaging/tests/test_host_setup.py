from pathlib import Path
import importlib.util
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


SETUP = load(ROOT / 'packaging/linux/setup-host.py', 'host_setup')
BUILDER = load(ROOT / 'packaging/build-host-support.py', 'host_builder')


class HostSetupTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.path = Path(self.temporary.name)
        self.modules = self.path / 'modules'
        (self.modules / '6.18-test').mkdir(parents=True)

    def test_plan_selects_matching_distribution_and_kernel_headers(self):
        for name, manager, headers in [('ubuntu', 'apt-get', 'linux-headers-6.18-test'),
                                       ('debian', 'apt-get', 'linux-headers-6.18-test'),
                                       ('fedora', 'dnf', 'kernel-devel-6.18-test')]:
            with self.subTest(name=name):
                plan = SETUP.plan({'ID': name}, '6.18-test', self.modules)
                self.assertEqual(plan['commands'][-1][0], manager)
                self.assertIn(headers, plan['commands'][-1])
                self.assertTrue(plan['experimental'])

    def test_arch_uses_kernel_package_metadata_not_guessed_headers(self):
        (self.modules / '6.18-test/pkgbase').write_text('linux-lts\n')
        plan = SETUP.plan({'ID': 'cachyos'}, '6.18-test', self.modules)
        self.assertIn('linux-lts-headers', plan['commands'][0])
        self.assertFalse(plan['experimental'])
        (self.modules / '6.18-test/pkgbase').write_text('linux; echo bad\n')
        with self.assertRaises(ValueError): SETUP.plan({'ID': 'arch'}, '6.18-test', self.modules)

    def test_existing_headers_are_not_reinstalled(self):
        headers = self.modules / '6.18-test/build'
        (headers / 'include/config').mkdir(parents=True)
        (headers / 'include/config/auto.conf').touch()
        (headers / 'Module.symvers').touch()
        plan = SETUP.plan({'ID': 'debian'}, '6.18-test', self.modules)
        self.assertTrue(plan['headers_ready'])
        self.assertNotIn('linux-headers-6.18-test', plan['commands'][-1])

    def test_os_release_is_parsed_as_data_not_a_shell_script(self):
        path = self.path / 'os-release'
        path.write_text('ID="linuxmint"\nID_LIKE="ubuntu debian"\nNAME="$(touch injected)"\n')
        self.assertEqual(SETUP.family(SETUP.os_info(path)), 'debian')
        self.assertFalse((self.path / 'injected').exists())

    def test_unsupported_distributions_and_kernel_injection_fail(self):
        with self.assertRaises(ValueError): SETUP.plan({'ID': 'unknown'}, '6.18-test', self.modules)
        with self.assertRaises(ValueError): SETUP.plan({'ID': 'debian'}, '../../etc', self.modules)

    def test_checksum_check_is_complete_and_read_only(self):
        payload = BUILDER.build(self.path / 'payload')
        before = {p.name: p.read_bytes() for p in payload.iterdir()}
        self.assertEqual(SETUP.verify(payload)['version'], '0.2.1')
        with patch.object(SETUP.subprocess, 'run', side_effect=AssertionError('host mutation')):
            SETUP.main(['--check', '--payload', str(payload)])
        self.assertEqual(before, {p.name: p.read_bytes() for p in payload.iterdir()})
        (payload / 'hid-playstation.c').write_text('corrupted')
        with self.assertRaises(ValueError): SETUP.verify(payload)

    def test_incomplete_manifest_or_symlink_is_rejected(self):
        payload = BUILDER.build(self.path / 'payload')
        (payload / 'manifest.sha256').write_text('')
        with self.assertRaises(ValueError): SETUP.verify(payload)
        BUILDER.build(payload)
        (payload / 'hid-ids.h').unlink()
        (payload / 'hid-ids.h').symlink_to(ROOT / 'packaging/kernel/source/hid-ids.h')
        with self.assertRaises(ValueError): SETUP.verify(payload)

    def test_install_rejects_fixture_overrides_and_unprivileged_use(self):
        with self.assertRaises(SystemExit): SETUP.main(['--install', '--kernel', '6.18-test'])
        with patch.object(SETUP.os, 'geteuid', return_value=1000):
            with self.assertRaises(ValueError): SETUP.install(self.path, {'version': '0.2.1'}, {}, '1000')


if __name__ == '__main__':
    unittest.main()
