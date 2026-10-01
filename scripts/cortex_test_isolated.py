#!/usr/bin/env python3
"""Run targeted pytest in NEW processes under a final isolated environment.

Existing dependencies are read explicitly, never installed or copied. Scratch
creation/writes require the caller's ordinary outside-command authorization.
"""
import argparse
import os
import re
import stat
from pathlib import Path
import subprocess
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--environment-root', required=True, help='existing fresh absolute scratch outside every Git repository')
    parser.add_argument('--dependency-path', required=True, help='existing interpreter-compatible site-packages directory')
    parser.add_argument('modules', nargs='+', help='explicit targeted tests/test_*.py modules (no whole suite)')
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    scratch, deps = Path(args.environment_root), Path(args.dependency_path)
    if (os.getuid() == 0 or not scratch.is_absolute() or scratch.resolve(strict=True) != scratch
            or not scratch.is_dir() or scratch.stat().st_uid != os.getuid()
            or stat.S_IMODE(scratch.stat().st_mode) & 0o077
            or any((p / '.git').exists() for p in (scratch, *scratch.parents))
            or not deps.is_absolute() or deps.resolve(strict=True) != deps or not deps.is_dir() or not (deps / 'pytest').is_dir()
            or deps.name != 'site-packages' or deps.parent.name != f'python{sys.version_info.major}.{sys.version_info.minor}'
            or any(not re.fullmatch(r'tests/test_[A-Za-z0-9_]+\.py', m)
                   or not (root / m).is_file() or (root / m).resolve() != root / m for m in args.modules)):
        parser.error('non-root, exact isolated scratch, compatible existing dependencies and explicit modules required')
    env = os.environ.copy()
    for key, name in [('HOME', 'home'), ('XDG_CONFIG_HOME', 'config'), ('XDG_STATE_HOME', 'state'),
                      ('XDG_CACHE_HOME', 'cache'), ('TMPDIR', 'tmp')]:
        path = scratch / name
        if (not path.is_dir() or path.resolve(strict=True) != path or path.stat().st_uid != os.getuid()
                or stat.S_IMODE(path.stat().st_mode) & 0o077):
            parser.error('pre-create isolated home/config/state/cache/tmp directories')
        env[key] = str(path)
    env.pop('PYTHONHOME', None)
    env['PYTHONPATH'] = f'{deps}:{root}/src:{root}/scripts'
    env['PYTHONDONTWRITEBYTECODE'] = '1'
    env['PYTEST_ADDOPTS'] = ''
    check = subprocess.run([sys.executable, '-B', '-c',
                            'import os,pytest,cortex_runtime.storage.inspection,cortex_runtime.configuration.service; '
                            'assert os.getuid()!=0; print("fresh-process import preflight: ok")'], cwd=root, env=env)
    if check.returncode:
        return check.returncode
    return subprocess.run([sys.executable, '-B', '-m', 'pytest', '-q', *args.modules], cwd=root, env=env).returncode


if __name__ == '__main__':
    raise SystemExit(main())
