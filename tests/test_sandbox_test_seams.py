"""Launch seams must retain command outcomes, not invent verification success."""
import subprocess
import sys

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.orchestration import controller, artifacts, sandbox
from sandbox_fakes import verification_command

PRODUCTION_VERIFICATION_COMMAND = controller._verification_command
PRODUCTION_GIT_COMMAND = artifacts._project_git_command


def test_test_runner_preserves_literal_argv_cwd_and_failure(tmp_path):
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    payload = "; touch injected"
    script = "import os,sys; print(os.getcwd()); print(sys.argv[1]); sys.exit(7)"
    with verification_command(tmp_path, scratch, [sys.executable, "-c", script, payload]) as (argv, fds):
        result = subprocess.run(argv, pass_fds=fds, capture_output=True, text=True)
    assert result.returncode == 7
    assert result.stdout.splitlines() == [str(tmp_path), payload]
    assert not (tmp_path / "injected").exists()


def test_production_launch_seams_refuse_missing_bwrap(tmp_path, monkeypatch):
    monkeypatch.setattr(sandbox.shutil, "which", lambda _: None)
    with pytest.raises(StoreError, match="verification_sandbox_unavailable"):
        with PRODUCTION_VERIFICATION_COMMAND(tmp_path, tmp_path, ["true"]):
            pytest.fail("no unsandboxed production fallback")
    with pytest.raises(StoreError, match="verification_sandbox_unavailable"):
        PRODUCTION_GIT_COMMAND(tmp_path, tmp_path, ["true"], [str(tmp_path)])


def test_production_verification_preparation_keeps_sandbox_options(tmp_path):
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    with PRODUCTION_VERIFICATION_COMMAND(tmp_path, scratch, ["true"]) as (command, fds):
        assert command[0].endswith("bwrap")
        assert ["--ro-bind", "/", "/"] == command[command.index("--ro-bind"):command.index("--ro-bind") + 3]
        assert command[-1] == "true"
        assert "--clearenv" in command
        assert "--chdir" in command
        assert isinstance(fds, (tuple, list))


@pytest.mark.parametrize("code,stderr,expected", [
    (0, "", None),
    (1, "bwrap: No permissions to create a new namespace",
     "bwrap cannot create user namespace in this environment (nested sandbox)"),
    (1, "bwrap: Creating new namespace failed: Operation not permitted",
     "bwrap cannot create user namespace in this environment (nested sandbox)"),
])
def test_namespace_probe_classifies_only_known_denials(code, stderr, expected):
    from types import SimpleNamespace
    from sandbox_fakes import bwrap_namespace_reason
    calls = []
    def run(argv, **kwargs):
        calls.append((argv, kwargs))
        return SimpleNamespace(returncode=code, stderr=stderr)
    assert bwrap_namespace_reason("/usr/bin/bwrap", run) == expected
    assert len(calls) == 1 and calls[0][0][1] == "--unshare-user"


def test_unexpected_probe_error_is_never_a_skip():
    from types import SimpleNamespace
    from sandbox_fakes import bwrap_namespace_reason
    with pytest.raises(AssertionError, match="failed unexpectedly"):
        bwrap_namespace_reason("bwrap", lambda *a, **kw: SimpleNamespace(
            returncode=1, stderr="bwrap: mount failed: invalid option"))


def test_missing_bwrap_does_not_try_to_probe():
    from sandbox_fakes import bwrap_namespace_reason
    assert bwrap_namespace_reason(None, lambda *a, **kw: pytest.fail("unexpected launch")) == "bubblewrap is not installed"
