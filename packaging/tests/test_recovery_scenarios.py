import importlib.util
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('validation', Path(__file__).parents[1] / 'validate-linux.py')
validation = importlib.util.module_from_spec(spec)
spec.loader.exec_module(validation)


class RecoveryHarnessTests(unittest.TestCase):
    def test_desktop_environment_is_replaced(self):
        with patch.dict(os.environ, {'PIPEWIRE_REMOTE': 'desktop', 'PIPEWIRE_CONFIG_DIR': '/desktop'}):
            env = validation.isolated_environment(Path('/tmp/isolated'))
        self.assertNotIn('PIPEWIRE_REMOTE', env)
        self.assertNotIn('PIPEWIRE_CONFIG_DIR', env)
        self.assertEqual(env['XDG_RUNTIME_DIR'], env['PIPEWIRE_RUNTIME_DIR'])
        self.assertEqual(env['DSZ_DATA_DIR'], '/tmp/isolated/settings')

    def test_empty_filter_and_failure_are_not_passes(self):
        for code, output in [(0, 'test result: ok. 0 passed;'), (1, 'test result: FAILED.')]:
            with patch.object(subprocess, 'run', return_value=subprocess.CompletedProcess([], code, output, '')):
                self.assertEqual(validation.run_check('test', [], {})['status'], 'FAIL')
        with patch.object(subprocess, 'run', side_effect=subprocess.TimeoutExpired([], 300)):
            self.assertEqual(validation.run_check('test', [], {})['status'], 'FAIL')

    def test_nonempty_counts_with_zero_suffix_are_valid(self):
        with patch.object(subprocess, 'run', return_value=subprocess.CompletedProcess([], 0, 'test result: ok. 10 passed;', '')):
            self.assertEqual(validation.run_check('test', [], {})['status'], 'PASS')

    def test_runtime_is_private_and_removed_and_skip_is_explicit(self):
        runtimes = []
        def check(name, args, env):
            runtime = Path(env['PIPEWIRE_RUNTIME_DIR'])
            runtimes.append(runtime)
            self.assertEqual(runtime.stat().st_mode & 0o777, 0o700)
            self.assertTrue(runtime.name.startswith('dualwave-pipewire-recovery-'))
            return dict(name=name, status='PASS')
        with patch.object(validation, 'run_check', side_effect=check), patch.object(validation.shutil, 'which', return_value=None):
            report = validation.validate()
        self.assertTrue(all(not path.exists() for path in runtimes))
        self.assertEqual([item['status'] for item in report['results']], ['PASS', 'PASS', 'SKIP'] + ['MANUAL_PENDING'] * 3)

    def test_exit_status_and_manifest(self):
        for statuses, expected in [(['PASS', 'MANUAL_PENDING'], 0), (['SKIP'], 2), (['FAIL', 'SKIP'], 1)]:
            with tempfile.TemporaryDirectory() as folder:
                output = Path(folder) / 'result.json'
                report = {'results': [dict(name='case', status=value) for value in statuses]}
                with patch.object(validation, 'validate', return_value=report), patch('sys.argv', ['validate-linux.py', '--output', str(output)]):
                    self.assertEqual(validation.main(), expected)
                self.assertTrue(output.exists())


if __name__ == '__main__':
    unittest.main()
