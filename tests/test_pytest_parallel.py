"""Default scheduling policy and real startup controls (no paid engines)."""
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace

import pytest

from conftest import _auto_parallel

ROOT = Path(__file__).resolve().parents[1]


def config(**options):
    return SimpleNamespace(
        option=SimpleNamespace(numprocesses=None, dist="no", tx=[], collectonly=False,
                               looponfail=False, **options),
        pluginmanager=SimpleNamespace(hasplugin=lambda name: name == "xdist"),
        getini=lambda name: [],
        invocation_params=SimpleNamespace(args=()),
    )


def test_unconfigured_top_level_uses_parallel():
    assert _auto_parallel(config(), {})


@pytest.mark.parametrize("key,value", [
    ("CORTEX_PYTEST_SERIAL", "1"), ("CORTEX_PYTEST_CHILD", "1"),
    ("PYTEST_XDIST_WORKER", "gw0"), ("PYTEST_CURRENT_TEST", "parent (call)"),
])
def test_nested_workers_and_serial_escape_do_not_fan_out(key, value):
    assert not _auto_parallel(config(), {key: value})


def test_xdist_absent_or_disabled():
    cfg = config()
    cfg.pluginmanager.hasplugin = lambda name: False
    assert not _auto_parallel(cfg, {})


@pytest.mark.parametrize("processes", [0, 1, 2, "auto", "logical"])
def test_explicit_process_count_is_preserved(processes):
    cfg = config()
    cfg.option.numprocesses = processes
    assert not _auto_parallel(cfg, {})


@pytest.mark.parametrize("name,value", [
    ("dist", "load"), ("tx", ["popen"]), ("collectonly", True), ("looponfail", True),
])
def test_other_distribution_modes_are_preserved(name, value):
    cfg = config()
    setattr(cfg.option, name, value)
    assert not _auto_parallel(cfg, {})


@pytest.mark.parametrize("source", ["cli", "ini", "env"])
@pytest.mark.parametrize("args", [("--dist=no",), ("--dist", "no")])
def test_explicit_serial_distribution_is_preserved(source, args):
    cfg = config()
    env = {}
    if source == "cli":
        cfg.invocation_params.args = args
    elif source == "ini":
        cfg.getini = lambda name: list(args)
    else:
        env["PYTEST_ADDOPTS"] = " ".join(args)
    assert not _auto_parallel(cfg, env)


def test_workerinput_without_environment_is_serial():
    cfg = config()
    cfg.workerinput = {}
    assert not _auto_parallel(cfg, {})


def test_startup_probe(pytestconfig):
    # Normally this is also an assertion about a real nested run's inheritance.
    expected = os.environ.get("CORTEX_PARALLEL_PROBE")
    if expected:
        assert hasattr(pytestconfig, "workerinput") is (expected in ("parallel", "explicit"))
        # xdist resets dist to "no" inside workers; the controller scheduler
        # is asserted from its real verbose startup output below.
        assert os.environ["CORTEX_PYTEST_CHILD"] == "1"


@pytest.mark.parametrize("mode,flags", [
    ("parallel", []), ("serial", ["-n0"]), ("serial-env", []),
    ("nested", []), ("disabled", ["-p", "no:xdist"]),
    ("no-autoload", []), ("explicit", ["-n", "1"]),
])
def test_real_startup_policy(tmp_path, mode, flags):
    import importlib.util
    xdist_available = importlib.util.find_spec("xdist") is not None
    env = os.environ.copy()
    for key in ("CORTEX_PYTEST_CHILD", "CORTEX_PYTEST_SERIAL", "PYTEST_CURRENT_TEST",
                "PYTEST_XDIST_WORKER", "PYTEST_XDIST_WORKER_COUNT", "PYTEST_ADDOPTS",
                "PYTEST_DISABLE_PLUGIN_AUTOLOAD"):
        env.pop(key, None)
    env.update(PYTHONDONTWRITEBYTECODE="1", CORTEX_PARALLEL_PROBE=(
        "parallel" if mode in ("parallel", "explicit") else "serial"))
    if mode == "serial-env":
        env["CORTEX_PYTEST_SERIAL"] = "1"
    if mode == "nested":
        env["CORTEX_PYTEST_CHILD"] = "1"
    if mode == "no-autoload":
        env["PYTEST_DISABLE_PLUGIN_AUTOLOAD"] = "1"
    # Bound startup cost without replacing xdist or its scheduler.
    env["PYTEST_XDIST_AUTO_NUM_WORKERS"] = "2"
    if mode == "explicit":
        # Explicit -n does not request the automatic loadgroup default.
        env["CORTEX_PARALLEL_PROBE"] = "explicit"
    if not xdist_available and mode in ("parallel", "explicit"):
        pytest.skip("pytest-xdist is not installed; its scheduling modes cannot be exercised")
    if not xdist_available:
        env["CORTEX_PARALLEL_PROBE"] = "serial"
    result = subprocess.run(
        [sys.executable, "-B", "-m", "pytest", "-v", *flags,
         "tests/test_pytest_parallel.py::test_startup_probe"],
        cwd=ROOT, env=env, capture_output=True, text=True, timeout=120)
    assert result.returncode == 0, result.stdout + result.stderr
    assert "1 passed" in result.stdout
    if mode == "parallel" and xdist_available:
        assert "scheduling tests via LoadGroupScheduling" in result.stdout
    elif mode == "explicit" and xdist_available:
        assert "scheduling tests via LoadScheduling" in result.stdout
    else:
        assert "scheduling tests via" not in result.stdout
