from pathlib import Path
import sys
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'src'))
sys.path.insert(0,str(Path(__file__).resolve().parents[1]/'scripts'))

import threading
import time

import pytest

# --- T009 incident guard -----------------------------------------------
#
# An early draft of a T009 ACP transport test passed the real repository
# checkout (via os.getcwd()) as an ACP session's project root. Opening a
# Store against that path triggered the one-way legacy
# .codex/cortex -> .cortex migration on the real project (see
# project_storage.migrate_legacy_project_storage). This guard exists so
# that mistake, or anything like it, fails the test run immediately
# instead of silently mutating the real checkout again.
#
# It monkeypatches the small set of functions that are the only entry
# points into project-root-scoped storage (canonical_project,
# project_store_directory, migrate_legacy_project_storage,
# logical_project_root) to raise AssertionError whenever the resolved
# path is this repository's own root or anything inside it. tmp_path
# fixtures and isolated scratch clones (anywhere outside this checkout)
# are unaffected.
#
# This only covers code running *inside this pytest process*. It cannot
# reach into a subprocess (the ACP agent is normally exercised as a real
# subprocess -- see tests/acp_fakes.py); subprocess-based ACP tests are
# covered separately by a static source scan (see
# test_acp_transport.py::test_no_test_source_uses_repo_root_as_project_root)
# that flags os.getcwd()/Path.cwd()/ROOT used in a project-root position
# in test source, which is what would have caught the original mistake.
# This module intentionally adds no test-only hook to production code
# (no env var project_storage would have to honor).

REPO_ROOT = Path(__file__).resolve().parents[1]

_GUARDED_NAMES = (
    "canonical_project",
    "project_store_directory",
    "migrate_legacy_project_storage",
    "logical_project_root",
)


def _is_forbidden(value):
    try:
        candidate = Path(value).resolve()
    except (OSError, RuntimeError, TypeError, ValueError):
        return False
    return candidate == REPO_ROOT or REPO_ROOT in candidate.parents


def _guard_value(value, function_name):
    if _is_forbidden(value):
        raise AssertionError(
            "test guard (tests/conftest.py): refusing to let "
            f"{function_name}({value!r}) touch this repository's own "
            f"checkout ({REPO_ROOT}) as a project root. Use tmp_path or an "
            "isolated scratch clone outside the checkout instead."
        )


def _wrap(function_name, original):
    def guarded(value, *args, **kwargs):
        _guard_value(value, function_name)
        return original(value, *args, **kwargs)
    guarded.__cortex_test_guard_original__ = original
    return guarded


def _install_project_root_guard():
    """Patch every currently-bound reference to the guarded functions.

    Patches ``cortex_runtime.project_storage`` itself (the source of
    truth: every other in-package caller resolves these names against
    that module's own globals at call time, including lazy
    ``from .project_storage import ...`` imports inside functions like
    ``store.Store.__init__``, so patching it alone already covers those).
    It also re-points any *already-imported* module that captured its own
    top-level ``from ... import <name>`` reference before this fixture
    ran (for example ``cortex_runtime.server``), since that binding is
    otherwise independent of the source module's attribute.
    """
    import cortex_runtime.project_storage as project_storage_module

    originals = {name: getattr(project_storage_module, name) for name in _GUARDED_NAMES}
    guarded = {name: _wrap(name, original) for name, original in originals.items()}
    patched = []

    for name, replacement in guarded.items():
        setattr(project_storage_module, name, replacement)
        patched.append((project_storage_module, name, originals[name]))

    for module in list(sys.modules.values()):
        if module is None or module is project_storage_module:
            continue
        for name, original in originals.items():
            try:
                if getattr(module, name, None) is original:
                    setattr(module, name, guarded[name])
                    patched.append((module, name, original))
            except Exception:
                continue
    return patched


@pytest.fixture(autouse=True, scope="session")
def _forbid_real_checkout_as_project_root():
    patched = _install_project_root_guard()
    try:
        yield
    finally:
        for module, name, original in patched:
            setattr(module, name, original)


# --- T012 review item 2: no leaked acp-reader/acp-writer threads ----------
#
# `acp.transport.StdioTransport` names its two threads "acp-reader" and
# "acp-writer" (used by both the incoming ACP agent and, via
# `engine.client.EngineClient`, every outgoing engine connection). A test
# that constructs one directly or via `engine.process.EngineProcess` +
# `engine.client.EngineClient` without ever letting it clean up (the
# original T012 review finding: `EngineProcess.release()`/`terminate()`
# never stopped the paired `EngineClient`'s writer thread) leaves daemon
# threads parked in the pytest process for the rest of the run -- harmless
# individually, but their accumulation is the confirmed root cause of the
# "process is multi-threaded, use of fork() may lead to deadlocks"
# DeprecationWarning seen when a later test (e.g.
# `tests/test_source_reports.py`) forks. This autouse fixture is an
# explicit, general safety net (not scoped to only the engine tests) so any
# future test with the same mistake fails immediately and locally rather
# than only being noticed as an unrelated warning far away in the run.
_LEAKED_THREAD_NAME_PREFIXES = ("acp-reader", "acp-writer")
_THREAD_LEAK_JOIN_GRACE_SECONDS = 1.0


@pytest.fixture(autouse=True)
def _no_leaked_acp_transport_threads():
    yield
    deadline = time.monotonic() + _THREAD_LEAK_JOIN_GRACE_SECONDS
    while time.monotonic() < deadline:
        leaked = [
            thread for thread in threading.enumerate()
            if thread.is_alive() and thread.name.startswith(_LEAKED_THREAD_NAME_PREFIXES)
        ]
        if not leaked:
            return
        time.sleep(0.02)
    leaked = [
        thread for thread in threading.enumerate()
        if thread.is_alive() and thread.name.startswith(_LEAKED_THREAD_NAME_PREFIXES)
    ]
    assert not leaked, (
        "test guard (tests/conftest.py): leaked acp-reader/acp-writer thread(s) "
        f"still alive after the test: {[thread.name for thread in leaked]!r}. "
        "Always call EngineClient.close() (or let EngineSessionManager.shutdown() "
        "do it) and/or EngineProcess.release() so both transport threads stop."
    )


# --- Claude usage-CLI spawn guard -----------------------------------------
#
# connections.quota lazily runs the real `claude -p /usage` CLI for
# claude-agent-acp connections whose credential_ref directory exists on
# disk. Several existing fixtures (e.g. engine_fakes.make_connection_record
# + isolated_environ, whose PATH deliberately mirrors the real host PATH so
# fake-engine launch_command resolution stays realistic) construct exactly
# such a connection, so a test that reaches quota.used_percent/resets_at/
# visible_windows for one without its own fake runner would otherwise spawn
# the real, installed `claude` binary. connections.quota._run_claude_usage_
# subprocess is the one seam every real call passes through (see its own
# docstring), so it is replaced here, for every test, with a stub that
# records the attempt and fails it -- rather than a silent no-op -- so a
# test that needs this path must explicitly monkeypatch its own fake
# runner (as tests/test_usage_accounting.py does), exactly like the no-paid
# default suite must never spawn the real CLI at all.
@pytest.fixture(autouse=True)
def _short_engine_retry_window(monkeypatch):
    """Fake engines fail at once, so the production retry window (delays up
    to a minute, ten minutes of retries) would only stall the suite. Retries
    here start again without a delay and stop after half a second of
    repeated failures; tests/test_engine_retry_window.py covers the real
    policy and sets its own window."""
    from cortex_runtime.engine import retry_policy

    monkeypatch.setattr(retry_policy, "RETRY_FIRST_DELAY_SECONDS", 0.0)
    monkeypatch.setattr(retry_policy, "RETRY_MAX_DELAY_SECONDS", 0.0)
    monkeypatch.setattr(retry_policy, "RETRY_WINDOW_SECONDS", 0.5)


@pytest.fixture(autouse=True)
def _forbid_real_claude_usage_cli(monkeypatch):
    from cortex_runtime.connections import quota as quota_module

    violations = []

    def guard(argv, env, cwd, timeout):
        violations.append(argv)
        raise AssertionError("connections.quota must not spawn the real claude CLI in tests")

    monkeypatch.setattr(quota_module, "_run_claude_usage_subprocess", guard)
    yield
    assert not violations, (
        "test guard (tests/conftest.py): this test reached "
        "connections.quota._run_claude_usage_subprocess (the real `claude` CLI "
        "spawn point) without monkeypatching it with its own fake runner first. "
        f"argv attempted: {violations!r}"
    )


# The fake engine writes its logs and dumps under the test's /tmp directory,
# which an engine jail replaces with private scratch. Plumbing tests run it
# unjailed; tests/test_engine_jail.py exercises the jail itself.
@pytest.fixture(autouse=True)
def _fake_engines_run_unjailed(request, monkeypatch):
    if request.node.get_closest_marker("engine_jail") is None:
        from cortex_runtime.delegation import live_adapters
        monkeypatch.setattr(live_adapters, "_jail_for", lambda store, credentials: None)


# Lifecycle tests exercise real commands, retention, revision checks and
# cancellation, not Linux namespace policy. Only the launch preparation is
# replaced; real sandbox tests opt out explicitly and require a successful
# namespace probe. Production never imports this fixture or offers a bypass.
def pytest_configure(config):
    config.addinivalue_line("markers", "requires_bwrap: executes a real bubblewrap sandbox")
    config.addinivalue_line("markers", "real_verification_sandbox: keep production verification launch preparation")


@pytest.fixture(scope="session")
def _bwrap_namespace_probe():
    import shutil
    import subprocess
    from sandbox_fakes import bwrap_namespace_reason
    return bwrap_namespace_reason(shutil.which("bwrap"), subprocess.run)


@pytest.fixture(autouse=True)
def _real_bwrap_requires_namespaces(request):
    if request.node.get_closest_marker("requires_bwrap") is not None:
        reason = request.getfixturevalue("_bwrap_namespace_probe")
        if reason:
            pytest.skip(reason)


@pytest.fixture(autouse=True)
def _lifecycle_verification_launch(request, monkeypatch):
    if request.node.get_closest_marker("real_verification_sandbox") is not None:
        return
    from cortex_runtime.orchestration import controller
    from sandbox_fakes import verification_command
    monkeypatch.setattr(controller, "_verification_command", verification_command)


@pytest.fixture(autouse=True)
def _git_context_launch(request, monkeypatch):
    if request.node.get_closest_marker("requires_bwrap") is not None:
        return
    from cortex_runtime.orchestration import artifacts
    from cortex_runtime.delegation import revision
    from sandbox_fakes import project_git_command, sandboxed_git
    monkeypatch.setattr(artifacts, "_project_git_command", project_git_command)
    monkeypatch.setattr(revision, "sandboxed_git", sandboxed_git)


@pytest.fixture
def unjailed_gateway_command(monkeypatch):
    """Opt-in for receipt/recovery tests, never for containment assertions."""
    from cortex_runtime.orchestration import sandbox
    from sandbox_fakes import unjailed_gateway_argv
    monkeypatch.setattr(sandbox, "compact_argv", unjailed_gateway_argv)
