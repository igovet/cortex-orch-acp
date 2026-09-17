"""T019: subprocess crash-injection harness for write_report/pipeline
publication.

A real ``write_report`` call runs in a child process with
``CORTEX_FAULT_INJECTION_POINT`` set to one of the named checkpoints in
``cortex_runtime._fault_injection``/``store.py``; the child calls
``os._exit(137)`` at that exact point (see ``_fault_injection.checkpoint``)
-- no Python cleanup, no ``except``/``finally`` runs, modeling an OS-level
kill. The parent then reopens a fresh ``Store`` over the same project root
and asserts recovery/reconciliation.
"""
from __future__ import annotations

import os
import subprocess
import sys
import textwrap
from pathlib import Path

RUNTIME_ROOT = str(Path(__file__).resolve().parents[1] / "plugins/cortex/scripts")

CHILD_TEMPLATE = textwrap.dedent(
    """
    import sys
    sys.path.insert(0, {runtime_root!r})
    from cortex_runtime.store import Store

    # No project_root is passed here, matching the existing test convention
    # (tests/test_storage_v11.py etc.) of a storage directory decoupled from
    # the project root path (Store(tmp_path/'private')) -- project_root
    # binding is exercised separately (tests/test_storage_locator.py etc.),
    # not the concern of this crash-injection harness.
    store = Store({store_dir!r}, initialize=False)
    store.call(
        "write_report",
        dict(title="Work", summary={summary!r}, author="worker",
             draft_id={draft_id!r}, request_key={request_key!r}),
        {thread_id!r}, None,
    )
    print("completed-without-crash")
    """
)


def crash_write_report(store_dir, project_root, thread_id, draft_id, request_key,
                        injection_point, summary="A concise result.", env=None):
    """Run ``write_report`` for ``draft_id`` in a child process that is
    expected to crash (``os._exit(137)``) at ``injection_point``. Returns
    the completed ``subprocess.CompletedProcess``."""
    script = CHILD_TEMPLATE.format(
        runtime_root=RUNTIME_ROOT, store_dir=str(store_dir), project_root=str(project_root),
        summary=summary, draft_id=draft_id, request_key=request_key, thread_id=thread_id,
    )
    child_env = dict(os.environ)
    child_env.update(env or {})
    child_env["CORTEX_FAULT_INJECTION_POINT"] = injection_point
    result = subprocess.run(
        [sys.executable, "-c", script], env=child_env, capture_output=True, text=True, timeout=30,
    )
    return result
