"""Isolated launcher regressions; only fresh scratch and existing dependencies."""
import json
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace

import pytest

import cortex_test_isolated as launcher

ROOT = Path(__file__).resolve().parents[1]
MODULE = 'tests/test_cortex_test_isolated.py'


@pytest.fixture
def launch_args(tmp_path, monkeypatch):
    scratch = tmp_path / 'environment'
    scratch.mkdir(mode=0o700)
    for name in ('home', 'config', 'state', 'cache', 'tmp'):
        (scratch / name).mkdir(mode=0o700)
    # Empty path-validation fixture; never installed/imported/copied dependencies.
    deps = tmp_path / f'python{sys.version_info.major}.{sys.version_info.minor}' / 'site-packages'
    (deps / 'pytest').mkdir(parents=True)
    argv = ['launcher', '--environment-root', str(scratch), '--dependency-path', str(deps), MODULE]
    monkeypatch.setattr(sys, 'argv', argv)
    return scratch, deps, argv


@pytest.mark.parametrize('exit_codes', [(0, 0), (0, 7), (9,)])
def test_new_preflight_then_targeted_pytest_same_interpreter_and_final_environment(launch_args, monkeypatch, exit_codes):
    scratch, deps, argv = launch_args
    calls = []
    monkeypatch.setenv('PYTHONPATH', '/unrelated')
    monkeypatch.setenv('PYTHONHOME', '/unrelated')
    monkeypatch.setenv('PYTEST_ADDOPTS', '--whole-suite')
    def run(command, *, cwd, env):
        calls.append((command, cwd, env.copy()))
        return SimpleNamespace(returncode=exit_codes[len(calls)-1])
    monkeypatch.setattr(launcher.subprocess, 'run', run)
    assert launcher.main() == exit_codes[-1]
    assert len(calls) == len(exit_codes)
    for command, cwd, env in calls:
        assert command[0:2] == [sys.executable, '-B']
        assert cwd == ROOT and 'PYTHONHOME' not in env
        assert env['PYTHONPATH'] == f'{deps}:{ROOT}/src:{ROOT}/scripts'
        assert env['PYTHONDONTWRITEBYTECODE'] == '1' and env['PYTEST_ADDOPTS'] == ''
        for key, name in [('HOME','home'), ('XDG_CONFIG_HOME','config'), ('XDG_STATE_HOME','state'),
                          ('XDG_CACHE_HOME','cache'), ('TMPDIR','tmp')]:
            assert env[key] == str(scratch/name)
    assert calls[0][0][2] == '-c' and 'import os,pytest,cortex_runtime' in calls[0][0][3]
    if len(calls) == 2:
        assert calls[1][0] == [sys.executable, '-B', '-m', 'pytest', '-q', MODULE]


@pytest.mark.parametrize('change', ['root', 'relative', 'symlink', 'git', 'public', 'child-symlink',
                                   'child-mode', 'missing-child', 'interpreter', 'dependency-symlink',
                                   'dependency-absent', 'module-dir', 'module-traversal', 'module-missing'])
def test_invalid_paths_modules_and_interpreters_refuse_before_subprocess(launch_args, monkeypatch, tmp_path, change):
    scratch, deps, argv = launch_args
    if change == 'root': monkeypatch.setattr(launcher.os, 'getuid', lambda: 0)
    if change == 'relative': argv[2] = 'relative'
    if change == 'symlink':
        alias = tmp_path/'alias'; alias.symlink_to(scratch); argv[2] = str(alias)
    if change == 'git': (scratch/'.git').write_text('gitdir: fixture')
    if change == 'public': scratch.chmod(0o755)
    if change == 'child-symlink':
        (scratch/'home').rmdir(); (scratch/'home').symlink_to(scratch/'config')
    if change == 'child-mode': (scratch/'home').chmod(0o755)
    if change == 'missing-child': (scratch/'home').rmdir()
    if change == 'interpreter': monkeypatch.setattr(launcher.sys, 'version_info', SimpleNamespace(major=9, minor=9))
    if change == 'dependency-symlink':
        alias = tmp_path/'site-packages'; alias.symlink_to(deps); argv[4] = str(alias)
    if change == 'dependency-absent': (deps/'pytest').rmdir()
    if change == 'module-dir': argv[-1] = 'tests'
    if change == 'module-traversal': argv[-1] = 'tests/test_../../scripts/cortex_paseo_dev.py'
    if change == 'module-missing': argv[-1] = 'tests/test_absent_fixture.py'
    monkeypatch.setattr(launcher.subprocess, 'run', lambda *a, **k: pytest.fail('subprocess before validation'))
    with pytest.raises((SystemExit, FileNotFoundError)) as error:
        launcher.main()
    if isinstance(error.value, SystemExit): assert error.value.code == 2


def test_actual_fresh_python_startup_imports_and_pytest_exit_propagation(tmp_path):
    child = os.environ.get('CORTEX_TEST_LAUNCHER_CHILD')
    if child:
        import tempfile
        import cortex_runtime.storage.inspection as inspection
        expected = json.loads(os.environ['CORTEX_TEST_LAUNCHER_EXPECTED'])
        for key, value in expected.items(): assert os.environ[key] == value
        assert Path(tempfile.gettempdir()) == Path(expected['TMPDIR'])
        assert Path(pytest.__file__).is_relative_to(Path(os.environ['PYTHONPATH'].split(':')[0]))
        assert sys.flags.dont_write_bytecode
        assert Path(inspection.__file__).is_relative_to(ROOT/'src')
        assert os.getuid() != 0
        assert child == 'pass', 'intentional child exit-propagation canary'
        return
    scratch = tmp_path/'startup'; scratch.mkdir(mode=0o700)
    expected = {}
    for key, name in [('HOME','home'), ('XDG_CONFIG_HOME','config'), ('XDG_STATE_HOME','state'),
                      ('XDG_CACHE_HOME','cache'), ('TMPDIR','tmp')]:
        path = scratch/name; path.mkdir(mode=0o700); expected[key] = str(path)
    deps = Path(pytest.__file__).resolve().parent.parent
    before = {p: p.read_bytes() for p in scratch.rglob('*') if p.is_file()}
    for mode, exit_code in [('pass', 0), ('fail', 1)]:
        env = os.environ.copy()
        env.update(CORTEX_TEST_LAUNCHER_CHILD=mode, CORTEX_TEST_LAUNCHER_EXPECTED=json.dumps(expected))
        result = subprocess.run([sys.executable, '-B', str(ROOT/'scripts/cortex_test_isolated.py'),
                                 '--environment-root', str(scratch), '--dependency-path', str(deps), MODULE],
                                cwd=ROOT, env=env, capture_output=True, text=True, timeout=30)
        assert result.returncode == exit_code, result.stdout + result.stderr
        assert 'fresh-process import preflight: ok' in result.stdout
        assert ('intentional child exit-propagation canary' in result.stdout) is (mode == 'fail')
    assert before == {}  # Dependencies were only read; new files are confined to scratch.
    assert not list(scratch.rglob('*.pyc'))

    # Ordinary pytest adds checkout imports in-process via conftest; unlike the
    # launcher it does not export them to children. Exercise both real startups.
    for form in ('ordinary', 'isolated'):
        env = os.environ.copy()
        env.pop('PYTHONHOME', None)
        env.update(expected, PYTHONPATH=str(deps), PYTHONDONTWRITEBYTECODE='1', PYTEST_ADDOPTS='')
        if form == 'ordinary':
            argv = [sys.executable, '-B', '-m', 'pytest', '-q', 'tests/test_qualification_binding.py']
        else:
            argv = [sys.executable, '-B', str(ROOT/'scripts/cortex_test_isolated.py'),
                    '--environment-root', str(scratch), '--dependency-path', str(deps),
                    'tests/test_qualification_binding.py']
        result = subprocess.run(argv, cwd=ROOT, env=env, capture_output=True, text=True, timeout=60)
        assert result.returncode == 0, f'{form} binding matrix exit={result.returncode}'
        assert '[100%]' in result.stdout and 'failed' not in result.stdout and 'error' not in result.stdout
