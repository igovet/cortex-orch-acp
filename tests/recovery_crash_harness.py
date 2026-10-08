"""T023: subprocess crash-injection harness for coordinator recovery.

Mirrors ``fault_injection_harness.py``'s pattern (see that module's
docstring) but drives a real ``OrchestratingRuntime.start_prompt`` call --
through the real ``EngineSessionManagerAdapter`` +
``tests/fake_engine_process.py`` subprocess -- in a child process that
crashes (``os._exit(137)``) at one of the six named T023 recovery
checkpoints (see ``cortex_runtime.recovery.coordinator_recovery`` and
``acp.orchestrating_runtime``'s ``_fault_injection.checkpoint`` call
sites)."""
from __future__ import annotations

import os
import subprocess
import sys
import textwrap
from pathlib import Path

RUNTIME_ROOT = str(Path(__file__).resolve().parents[1] / "src")
TESTS_ROOT = str(Path(__file__).resolve().parent)

CHILD_TEMPLATE = textwrap.dedent(
    """
    import sys, threading
    sys.path.insert(0, {runtime_root!r})
    sys.path.insert(0, {tests_root!r})
    from cortex_runtime.store import Store
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter, ConfigurationServiceSnapshotSource
    from cortex_runtime.configuration.service import ConfigurationService
    # Match conftest's fake-engine launch seam in this child process too.
    from cortex_runtime.delegation import live_adapters
    live_adapters._jail_for = lambda store, credentials: None
    from orchestration_fakes import FakeWorkspace
    from engine_fakes import fake_launch_command, make_connection_record

    store = Store({store_dir!r}, initialize=False)
    environ = {environ!r}
    record = make_connection_record(
        {connection_id!r}, launch_command=fake_launch_command({scenario_path!r}), environ=environ,
    )
    adapter = EngineSessionManagerAdapter(
        lambda t: record, lambda t: {workspace_root!r}, environ=environ, call_timeout=10.0,
        store=store, capacity_limit=4,
    )
    runtime = OrchestratingRuntime(
        store, adapter, FakeWorkspace(), ConfigurationServiceSnapshotSource(ConfigurationService(store)),
        allow_unqualified_delivery=True, wait_timeout_seconds=10.0,
    )
    updates = []
    result = runtime.start_prompt({task_id!r}, {session_id!r}, "Continue please.", updates.append, threading.Event())
    print("completed-without-crash", result)
    """
)


def crash_recovery(
    store_dir, task_id, connection_id, scenario_path, environ, workspace_root, injection_point, session_id="s_child",
):
    """Run ``start_prompt`` (which internally recovers the coordinator) in
    a child process expected to crash (``os._exit(137)``) at
    ``injection_point``."""
    script = CHILD_TEMPLATE.format(
        runtime_root=RUNTIME_ROOT, tests_root=TESTS_ROOT, store_dir=str(store_dir),
        environ=environ, connection_id=connection_id, scenario_path=str(scenario_path),
        workspace_root=str(workspace_root), task_id=task_id, session_id=session_id,
    )
    child_env = dict(os.environ)
    child_env.update(environ)
    child_env["CORTEX_FAULT_INJECTION_POINT"] = injection_point
    return subprocess.run(
        [sys.executable, "-B", "-c", script], env=child_env, capture_output=True, text=True, timeout=30,
    )
