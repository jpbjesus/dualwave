import hashlib
import io
import os
from pathlib import Path
import tarfile
import tempfile
import types
import unittest
from unittest.mock import patch

SOURCE = Path(__file__).resolve().parents[1] / 'install.sh'


def installer():
    module = types.ModuleType('installer')
    exec(compile(SOURCE.read_text().split("<<'PY'\n", 1)[1].rsplit('\nPY', 1)[0], str(SOURCE), 'exec'), module.__dict__)
    return module


def native_archive():
    data = io.BytesIO()
    with tarfile.open(fileobj=data, mode='w:gz') as archive:
        for name in ('dualwave', 'dualwave-cli'):
            content = b'#!/bin/sh\nexit 0\n'
            entry = tarfile.TarInfo('bin/' + name)
            entry.mode = 0o755
            entry.size = len(content)
            archive.addfile(entry, io.BytesIO(content))
    return data.getvalue()


class UserInstallTests(unittest.TestCase):
    def setUp(self):
        self.module = installer()
        self.temp = tempfile.TemporaryDirectory(dir=Path.home(), prefix='dualwave test $ ')
        self.addCleanup(self.temp.cleanup)
        self.prefix = Path(self.temp.name) / 'user prefix'
        self.args = ['--version', 'v0.2.1', '--prefix', str(self.prefix)]

    def fixture(self, payload, mismatch=False):
        def fetch(url):
            self.assertTrue(url.startswith('https://github.com/jpbjesus/dualwave/releases/download/v0.2.1/'))
            if url.endswith('.sha256'):
                checksum = '0' * 64 if mismatch else hashlib.sha256(payload).hexdigest()
                return (checksum + '  ' + url.rsplit('/', 1)[1][:-7] + '\n').encode()
            return payload
        return patch.object(self.module, 'fetch', side_effect=fetch)

    def test_root_install_is_rejected_before_download(self):
        with patch.object(self.module.os, 'geteuid', return_value=0), self.assertRaises(SystemExit):
            self.module.main(self.args)
        self.assertFalse(self.prefix.exists())

    def test_version_validation(self):
        for tag in ('latest', '../bad', 'v1.2.3/evil', 'v1.2.3;touch x', 'https://evil'):
            with self.subTest(tag=tag), self.assertRaises(SystemExit):
                self.module.main(['--version', tag, '--prefix', str(self.prefix)])
        self.assertFalse(self.prefix.exists())

    def test_dry_run_has_no_download_or_mutation(self):
        with patch.object(self.module, 'fetch', side_effect=AssertionError('download')):
            self.module.main(self.args + ['--dry-run'])
        self.assertFalse(self.prefix.exists())

    def test_checksum_failure_preserves_installation(self):
        with self.fixture(b'first'):
            self.module.main(self.args)
        root = self.prefix / 'lib/dualwave-user'
        previous = os.readlink(root / 'current')
        with self.fixture(b'bad', mismatch=True), self.assertRaisesRegex(ValueError, 'SHA-256'):
            self.module.main(self.args)
        self.assertEqual(os.readlink(root / 'current'), previous)
        self.assertEqual((self.prefix / 'bin/dualwave').read_bytes(), b'first')

    def test_native_install_and_uninstall_preserve_unrelated(self):
        with self.fixture(native_archive()):
            self.module.main(self.args + ['--format', 'native'])
        unrelated = self.prefix / 'bin/other'
        unrelated.write_text('keep')
        settings = self.prefix / 'settings.json'
        settings.write_text('keep')
        self.module.main(['--uninstall', '--prefix', str(self.prefix)])
        self.assertFalse(os.path.lexists(self.prefix / 'bin/dualwave'))
        self.assertEqual(unrelated.read_text(), 'keep')
        self.assertEqual(settings.read_text(), 'keep')

    def test_uninstall_refuses_unmanaged_directory(self):
        root = self.prefix / 'lib/dualwave-user'
        root.mkdir(parents=True)
        (root / 'precious').write_text('keep')
        with self.assertRaisesRegex(ValueError, 'unmanaged'):
            self.module.main(['--uninstall', '--prefix', str(self.prefix)])
        self.assertTrue((root / 'precious').exists())

    def test_appimage_wrapper_quotes_paths(self):
        with self.fixture(b'#!/bin/sh\nprintf "%s\\n" "$@"\n'):
            self.module.main(self.args)
        import subprocess
        result = subprocess.run([str(self.prefix / 'bin/dualwave-cli'), 'two words', '$(bad)'], text=True, capture_output=True, check=True)
        self.assertEqual(result.stdout, '--cli\ntwo words\n$(bad)\n')

    def test_unsafe_archive_preserves_current(self):
        with self.fixture(b'first'):
            self.module.main(self.args)
        data = io.BytesIO()
        with tarfile.open(fileobj=data, mode='w:gz') as archive:
            entry = tarfile.TarInfo('../escaped')
            archive.addfile(entry)
        with self.fixture(data.getvalue()), self.assertRaisesRegex(ValueError, 'Unsafe'):
            self.module.main(self.args + ['--format', 'native'])
        self.assertEqual((self.prefix / 'bin/dualwave').read_bytes(), b'first')


if __name__ == '__main__':
    unittest.main()
