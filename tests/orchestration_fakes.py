"""T016: in-process fakes for OrchestratingRuntime tests.

No real engine, workspace, or network I/O anywhere in this module. The
"fake model" a scripted worker/coordinator behavior represents calls
``delegation.mcp_tools.dispatch`` directly with the session's own resolved
``ChannelContext`` -- exactly the in-process substitute for "an engine's
own tool call to its Cortex MCP subprocess" the T016 task card asks tests
to use.
"""
from __future__ import annotations

import threading

from cortex_runtime.acp.orchestrating_runtime import PromptOutcome
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.trusted_channel import ChannelContext


def channel_context_by_id(store, channel_id):
    """Test-only: resolve a ``ChannelContext`` by id without a secret.

    Production code always authenticates via ``trusted_channel.resolve_channel``
    (channel_id + secret). This fake stands in for "the engine process
    already authenticated over its own trusted channel" without this test
    double needing to also simulate secret delivery to a spawned process --
    see ``delegation.live_adapters`` module docstring for why that wiring is
    out of this task's scope.
    """
    with store.connection() as db:
        row = db.execute("SELECT * FROM trusted_channels WHERE channel_id=?", (channel_id,)).fetchone()
    if row is None:
        return None
    return ChannelContext(
        channel_id=row["channel_id"], task_id=row["task_id"],
        principal_thread_id=row["principal_thread_id"],
        parent_principal_thread_id=row["parent_principal_thread_id"],
        role=row["role"], owner_epoch=row["owner_epoch"],
        assignment_id=row["assignment_id"], attempt_id=row["attempt_id"],
        workspace_binding_id=row["workspace_binding_id"],
        context_generation_id=row["context_generation_id"],
    )


class FakeWorkspace:
    def create(self, attempt_id):
        return "shared_project_root"

    def lookup(self, attempt_id):
        return "present"

    def remove(self, attempt_id):
        return None


class FakeConfigSnapshotSource:
    def __init__(self, engine="codex-acp", version="1.12.0"):
        self.engine = engine
        self.version = version

    def snapshot(self, task_id):
        return dict(engine=self.engine, version=self.version, config_revision="cr_" + task_id[2:])


class FakeSessionRuntime:
    """A ``SessionRuntime`` whose ``prompt`` calls a test-supplied ``router``.

    ``router(store, channel, session_ref, launch, instructions) -> behavior``
    is called once, at ``open()`` time, to pick the scripted behavior for
    that session; ``behavior(store, channel, session_ref, message,
    call_index) -> PromptOutcome`` is then invoked once per ``prompt()``
    call. This lets each test script exactly what a coordinator/worker
    "model" does on each turn, including calling ``mcp_tools.dispatch``.
    """

    def __init__(self, store, router):
        self._store = store
        self._router = router
        self.sessions = {}
        self._owned = {}
        self._counter = 0
        self.prompt_calls = []
        self._lock = threading.Lock()

    def open(self, workspace_binding_id, launch, instructions):
        with self._lock:
            self._counter += 1
            session_ref = f"es_{self._counter}"
        channel = channel_context_by_id(self._store, launch.get("channel_id")) if launch.get("channel_id") else None
        behavior = self._router(self._store, channel, session_ref, launch, instructions)
        self.sessions[session_ref] = dict(channel=channel, behavior=behavior, calls=0, attempt_id=launch.get("attempt_id"))
        if launch.get("attempt_id"):
            self._owned[launch["attempt_id"]] = session_ref
        return session_ref

    def lookup_owned(self, attempt_id):
        return "alive" if attempt_id in self._owned else "absent"

    def terminate_owned(self, attempt_id):
        self._owned.pop(attempt_id, None)

    def prompt(self, session_ref, message, cancel_event):
        info = self.sessions[session_ref]
        self.prompt_calls.append(session_ref)
        call_index = info["calls"]
        info["calls"] += 1
        behavior = info["behavior"]
        if behavior is None:
            return PromptOutcome(StopReason.END_TURN, "")
        return behavior(self._store, info["channel"], session_ref, message, call_index)


def coordinator_prompt_count(fake: FakeSessionRuntime, coordinator_session_ref: str) -> int:
    return sum(1 for ref in fake.prompt_calls if ref == coordinator_session_ref)
