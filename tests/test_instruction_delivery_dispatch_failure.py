"""Generation qualification errors reach route availability before reporting.

This exercises the runtime's real worker dispatch loop with in-process
provisioning and generation fakes; no model, account or engine is contacted.
"""
import threading
from types import SimpleNamespace

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.portable.errors import PortableError, PortableErrorCategory
from test_cancelled_worker_open_failure import _runtime, _failures


@pytest.mark.parametrize('disconnected', [False, True])
def test_generation_refusal_is_retained_before_assignment_failure(tmp_path, disconnected):
    error = PortableError(
        category=PortableErrorCategory.VALIDATION, code="instruction_delivery_unqualified",
        remediation="No exact delivery channel.", retry_eligible=False,
    )
    store, task_id, runtime, channel, payload = _runtime(tmp_path, "running", error)
    recorded, terminated = [], []
    runtime._provisioning = SimpleNamespace(delegate=lambda *args: SimpleNamespace(
        engine_session_ref="es_worker", attempt_id="xa_worker",
    ))
    runtime._engine = SimpleNamespace(
        record_instruction_delivery_failure=lambda session, failure: recorded.append((session, failure)),
        terminate_owned=terminated.append,
    )
    runtime._lock = threading.RLock()
    runtime._worker_sessions = {}
    runtime._worker_recoveries_in_flight = set()
    runtime._shutting_down = threading.Event()
    runtime.transport_lost = lambda task: disconnected
    runtime._task_workspace_root = lambda task: str(tmp_path)
    runtime._resolve_language = lambda task: "en"
    runtime._finalize_generation_delivery = lambda *args, **kwargs: None
    runtime._mark_work_attempt_failed = lambda *args: None

    def refuse_generation(*args):
        raise error

    runtime._begin_generation_delivery = refuse_generation
    OrchestratingRuntime._provision_and_prompt_inner(runtime, task_id, channel, payload, threading.Event())
    assert recorded == ([] if disconnected else [("es_worker", error)])
    assert terminated == (["xa_worker"] if disconnected else [])
    assert [failure["error"] for failure in _failures(store, task_id)] == (
        [] if disconnected else ["PortableError:instruction_delivery_unqualified"])
    if disconnected:
        assert not runtime._worker_sessions
