"""Selection completeness, hard bounds and failure propagation for the default runner."""
import os
from pathlib import Path
import subprocess
import sys
from types import SimpleNamespace

import pytest

import run_tests_fast as fast

ROOT = Path(__file__).resolve().parents[1]


def test_every_test_file_is_fast_or_explicitly_slow():
    # Independent enumeration, not the selector's own list as its oracle.
    expected = {p.relative_to(ROOT).as_posix() for p in (ROOT / "tests").rglob("*.py")
                if p.match("test_*.py") or p.match("*_test.py")}
    selected = fast.selection(ROOT)
    slow = fast.selection(ROOT, slow=True)
    assert not set(selected) & set(slow)
    assert set(selected) | set(slow) == expected
    flattened = [p for batch in fast.batches(selected) for p in batch]
    assert flattened == selected and len(flattened) == len(set(flattened))
    assert all(0 < len(batch) <= fast.BATCH_SIZE for batch in fast.batches(selected))
    assert all(fast.SLOW_FILES[p].strip() for p in slow)


def test_new_file_is_selected_and_stale_exclusion_fails(tmp_path, monkeypatch):
    tests = tmp_path / "tests" / "nested"
    tests.mkdir(parents=True)
    (tests / "test_future.py").touch()
    (tests / "future_test.py").touch()
    assert fast.selection(tmp_path) == ["tests/nested/future_test.py", "tests/nested/test_future.py"]
    monkeypatch.setattr(fast, "SLOW_FILES", {"tests/missing.py": "not a real exclusion"})
    with pytest.raises(ValueError, match="Invalid slow-file"):
        fast.selection(tmp_path)


@pytest.mark.parametrize("exit_code", [0, 1, 2, 124, -9])
def test_batches_propagate_every_failure_and_do_not_drop_later_files(monkeypatch, exit_code):
    files = [f"tests/test_fixture_{i}.py" for i in range(fast.BATCH_SIZE + 1)]
    calls = []
    def run(argv, **kwargs):
        calls.append(argv)
        return SimpleNamespace(returncode=exit_code if len(calls) == 1 else 0)
    monkeypatch.setattr(fast.subprocess, "run", run)
    assert fast.run_batches(ROOT, files, {}) == int(exit_code != 0)
    assert len(calls) == 2
    assert [p for argv in calls for p in argv if p.startswith("tests/")] == files
    for argv in calls:
        assert argv[:3] == ["timeout", "--kill-after=5", str(fast.BATCH_SECONDS)]
        assert argv[3] == str(ROOT / ".venv/bin/python")
        assert "--timeout=60" in argv and "--timeout-method=thread" in argv
        assert "no:cacheprovider" in argv and "--max-worker-restart=0" in argv


def test_cleanup_removes_a_directory_it_cannot_enter(tmp_path):
    from conftest import remove_tree
    closed = tmp_path / "tree/overlay-work/work"
    closed.mkdir(parents=True)
    (closed / "inner").touch()
    closed.chmod(0)
    remove_tree(tmp_path / "tree")
    assert not (tmp_path / "tree").exists()


def built_service(tmp_path, monkeypatch, exit_code=0):
    """Run build_service against a stand-in cargo, returning its outcome and calls."""
    binary = tmp_path / "target/debug/workspace-service"
    binary.parent.mkdir(parents=True)
    calls = []
    def run(argv, **kwargs):
        calls.append((argv, kwargs))
        binary.touch()
        return SimpleNamespace(returncode=exit_code)
    monkeypatch.setattr(fast.subprocess, "run", run)
    return fast.build_service(tmp_path, dict(
        PATH="/usr/bin", HOME=str(tmp_path), CARGO_TARGET_DIR="/elsewhere", CORTEX_WORKSPACE_CHANNEL="release",
        CORTEX_WORKSPACE_RELEASE_AUTHORIZED="inherited")), calls


def test_service_is_built_into_the_tested_path_on_the_dev_channel(tmp_path, monkeypatch):
    monkeypatch.setattr(fast, "cargo_program", lambda env: "/toolchain/bin/cargo")
    binary, [(argv, options)] = built_service(tmp_path, monkeypatch)
    assert binary == tmp_path / "target/debug/workspace-service"
    assert argv == ["timeout", "--kill-after=5", str(fast.BUILD_SECONDS), "/toolchain/bin/cargo", "build",
                    "--locked", "-p", "workspace-service"]
    env = options["env"]
    assert options["cwd"] == tmp_path and env["CARGO_TARGET_DIR"] == str(tmp_path / "target")
    assert env["CORTEX_WORKSPACE_CHANNEL"] == "dev" and "CORTEX_WORKSPACE_RELEASE_AUTHORIZED" not in env
    assert env["PATH"].split(os.pathsep) == ["/toolchain/bin", "/usr/bin"]


@pytest.mark.parametrize("exit_code", [1, 101, 124])
def test_failed_service_build_stops_the_suite_instead_of_reusing_an_old_binary(tmp_path, monkeypatch, exit_code):
    monkeypatch.setattr(fast, "cargo_program", lambda env: "/toolchain/bin/cargo")
    assert built_service(tmp_path, monkeypatch, exit_code)[0] is None


def test_missing_cargo_is_an_explicit_failure(tmp_path, monkeypatch):
    monkeypatch.setattr(fast, "cargo_program", lambda env: None)
    binary, calls = built_service(tmp_path, monkeypatch)
    assert binary is None and calls == []


def test_cargo_is_found_below_the_account_home_when_path_omits_it(tmp_path):
    cargo = tmp_path / ".cargo/bin/cargo"
    cargo.parent.mkdir(parents=True)
    cargo.write_text("#!/bin/sh\n")
    cargo.chmod(0o755)
    assert fast.cargo_program(dict(PATH=str(tmp_path / "empty"), HOME=str(tmp_path))) == str(cargo)
    moved = dict(PATH=str(tmp_path / "empty"), HOME="/nonexistent", CARGO_HOME=str(tmp_path / ".cargo"))
    assert fast.cargo_program(moved) == str(cargo)
    assert fast.cargo_program(dict(PATH=str(tmp_path / "empty"), HOME="/nonexistent")) is None


def test_a_service_older_than_its_sources_is_refused(tmp_path, monkeypatch):
    from test_workspace_service_integration import service_binary
    monkeypatch.delenv("WORKSPACE_TEST_SERVICE_BINARY", raising=False)
    binary = tmp_path / "workspace-service"
    source = tmp_path / "with space/child.rs"
    source.parent.mkdir()
    source.touch()
    binary.touch()
    os.utime(source, (1000, 1000))
    os.utime(binary, (2000, 2000))
    record = binary.with_name("workspace-service.d")
    escaped = str(source).replace(" ", "\\ ")
    record.write_text(f"{binary}: {escaped}\n")
    assert service_binary(binary) == binary
    os.utime(source, (3000, 3000))
    with pytest.raises(AssertionError, match="cargo build --locked -p workspace-service"):
        service_binary(binary)
    os.utime(source, (1000, 1000))
    source.unlink()
    with pytest.raises(AssertionError, match="is older than"):
        service_binary(binary)
    record.unlink()
    with pytest.raises(AssertionError, match="first"):
        service_binary(binary)
    monkeypatch.setenv("WORKSPACE_TEST_SERVICE_BINARY", str(binary))
    assert service_binary(binary) == binary


@pytest.mark.parametrize("body,expected", [("assert True", 0), ("assert False", 1),
                                         ("import time; time.sleep(60)", 1)])
def test_real_success_failure_and_hang_are_distinguished(tmp_path, body, expected):
    sample = tmp_path / "test_runner_canary.py"
    sample.write_text("def test_named_canary():\n    " + body + "\n")
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1", CORTEX_PYTEST_SERIAL="1",
               PYTEST_ADDOPTS="", PYTEST_DISABLE_PLUGIN_AUTOLOAD="1")
    result = subprocess.run(
        ["timeout", "--kill-after=2", "10", sys.executable, "-B", "-m", "pytest",
         "-p", "pytest_timeout", "-p", "no:cacheprovider", "--timeout=0.3",
         "--timeout-method=thread", "-v", str(sample)],
        cwd=tmp_path, env=env, capture_output=True, text=True, timeout=15)
    assert result.returncode == expected, result.stdout + result.stderr
    assert "test_named_canary" in result.stdout
    assert ("Timeout" in result.stdout) == ("time.sleep" in body)
