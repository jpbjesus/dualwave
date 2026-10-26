#!/usr/bin/env python3
"""Hardware-free Linux lifecycle checks. Never launches the application."""
import argparse
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
MANUAL = [
    'Bluetooth: record controller model/firmware; reconnect during idle and active input; verify one virtual controller and no stuck buttons.',
    'Suspend/resume: with consent, suspend during idle and audio playback; verify input, microphone and headphones recover without stale audio.',
    'Real games: record titles, Steam/Proton versions and launch order; verify native/fallback input, manual profile override, exit and relaunch.',
]


def isolated_environment(runtime):
    env = os.environ.copy()
    # Explicit runtime paths and remote removal prevent inherited desktop routing.
    for key in ('PIPEWIRE_REMOTE', 'PIPEWIRE_CONFIG_DIR', 'PIPEWIRE_CONFIG_NAME', 'PIPEWIRE_CONFIG_PREFIX'):
        env.pop(key, None)
    env.update(XDG_RUNTIME_DIR=str(runtime), PIPEWIRE_RUNTIME_DIR=str(runtime),
               DSZ_DATA_DIR=str(runtime / 'settings'))
    return env


def run_check(name, args, env):
    try:
        result = subprocess.run(args, cwd=ROOT, env=env, capture_output=True, text=True, timeout=300)
        # Cargo may succeed with zero matching tests: that is not validation.
        counts = re.findall(r'test result: ok\.\s+(\d+) passed;', result.stdout)
        passed = result.returncode == 0 and any(int(count) > 0 for count in counts)
        return dict(name=name, status='PASS' if passed else 'FAIL', returncode=result.returncode,
                    output=(result.stdout + result.stderr)[-16000:])
    except (OSError, subprocess.TimeoutExpired) as error:
        return dict(name=name, status='FAIL', output=str(error))


def validate():
    results = []
    with tempfile.TemporaryDirectory(prefix='dualwave-pipewire-recovery-', dir='/tmp') as folder:
        runtime = Path(folder)
        runtime.chmod(0o700)
        env = isolated_environment(runtime)
        command = ['cargo', 'test', '-p', 'dsz', '--locked']
        results.append(run_check('game-process-and-profile-lifecycle', command + ['games::tests::', '--', '--test-threads=1'], env))
        results.append(run_check('audio-recovery-state', command + ['device::microphone_tests::', '--', '--test-threads=1'], env))
        if shutil.which('pipewire'):
            results.append(run_check('isolated-pipewire-daemon-restart', command + [
                'device::microphone_tests::isolated_pipewire_recovery_recreates_endpoints_after_daemon_restart',
                '--', '--ignored', '--exact', '--test-threads=1'], env))
        else:
            results.append(dict(name='isolated-pipewire-daemon-restart', status='SKIP', reason='pipewire executable unavailable'))
    results.extend(dict(name=item, status='MANUAL_PENDING') for item in MANUAL)
    return dict(scope='No physical controller, input injection, desktop audio or host suspend exercised', results=results)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, default=Path('linux-validation.json'))
    args = parser.parse_args()
    report = validate()
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    for result in report['results']:
        print(f"{result['status']}: {result['name']}")
    print(f'Results: {args.output}')
    # Skipped automated coverage is incomplete, distinct from failed coverage.
    statuses = {item['status'] for item in report['results']}
    return 1 if 'FAIL' in statuses else 2 if 'SKIP' in statuses else 0


if __name__ == '__main__':
    raise SystemExit(main())
