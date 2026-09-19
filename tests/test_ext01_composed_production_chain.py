"""EXT-01 composed production-chain integration pass.

The reviewer's own closing ask (see
``.cortex/acp-migration/reviews/EXT-01-independent-production-chain-review.md``,
"What the reviewer asked for next"): one unpaid pass through the *real
production composition* -- entrypoint -> real store -> isolated worker ->
real MCP subprocess -> protocol configuration -> cancel -> two recovery
generations -- with the fake engine used only as a controllable source of
failures, never as a substitute for a Cortex component. Every individual
seam here already has its own regression test (named in each phase's
docstring below); this file composes the same *real* objects those tests
use, in production order, in one run, and asserts they hold together.

WHAT IS REAL, UNCHANGED, in this file:
  - ``cortex_acp_setup.py`` (the real one-time setup script) writes the
    real connection registry / pinned route (fake API key bytes, never a
    real credential).
  - ``acp.agent.Agent`` + ``acp.transport.StdioTransport`` -- the exact
    classes ``cortex_acp.py``'s own ``main()`` constructs -- driven over a
    real ``os.pipe()`` (a pipe instead of the process's real stdio is the
    same substitution every other real-Agent test in this suite makes;
    seeing this spawned as a literal OS subprocess would additionally
    require a real downstream engine speaking full ACP over its own stdio,
    which is out of scope -- no real engine, no paid call).
  - ``acp.production_runtime.ProductionOrchestratingRuntime`` -- the exact
    runtime class ``cortex_acp.py`` passes to ``main()`` -- resolves its
    own real ``Store``, real ``ConfigurationService``, real connections
    registry, and lazily builds one real
    ``delegation.live_adapters.EngineSessionManagerAdapter`` +
    ``delegation.workspace_isolation.IsolatedSnapshotWorkspaceProvisioner``
    per project, exactly as production does.
  - The Cortex MCP server (``src/cortex.py``) is
    launched as a literal subprocess for the worker, with the exact
    ``mcpServers`` entry the real adapter computed for it.
  - Only the ACP engine at the far end of each launch_command is a
    standalone fake (``tests/fake_engine_process.py``) -- never any Cortex
    module.

WHAT IS DISCLOSED, NOT HIDDEN:
  - The coordinator's own "call its `delegate` MCP tool" step is invoked
    directly via ``delegation.mcp_tools.dispatch`` (the real production
    dispatch function ``cortex.py`` itself calls) rather than round-tripped
    through the fake engine's own wire, because the standalone fake engine
    is a scripted ACP peer with no MCP client of its own -- it cannot
    itself speak MCP to a spawned ``cortex.py``. Every other landed
    regression test that drives a real coordinator this way
    (``test_acp_setup_fresh_session.py``,
    ``test_context_generations.py``, ...) makes the identical
    substitution; the worker side is where the real MCP round trip is
    actually proven (phase 3 below), which is what EXT-01 finding #1 is
    about.
  - The worker's *second* engine session (the "two recovery generations"
    requirement) is produced by directly re-invoking the real adapter's own
    ``open()``/delivery methods for the same ``attempt_id`` after
    ``terminate_owned`` -- standing in for whatever real trigger (a crash,
    a lost heartbeat, ``recovery.worker_reconciliation``) would cause a
    genuine retry in production. The *methods* invoked are the exact ones
    that trigger would call; only the trigger itself is skipped.
  - See the bottom of this file ("WHAT THIS PASS WOULD NOT HAVE CAUGHT")
    for the honest limitations list.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time
import uuid
from pathlib import Path

import pytest

from cortex_runtime.acp.agent import Agent
from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime
from cortex_runtime.acp.transport import StdioTransport
from cortex_runtime.configuration.service import ConfigurationService
from cortex_runtime.connections.model import AuthMode
from cortex_runtime.delegation import capacity as delegation_capacity
from cortex_runtime.delegation import deliveries as delegation_deliveries
from cortex_runtime.delegation import generations as delegation_generations
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation.workspace_isolation import IsolatedSnapshotWorkspaceProvisioner
from cortex_runtime.engine.client import EngineClient
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.trusted_channel import issue_channel_tx

import cortex_acp_setup as setup
from engine_fakes import default_scenario, fake_launch_command, write_scenario

ROOT = Path(__file__).resolve().parents[1]
CORTEX_PY = ROOT / "src/cortex.py"
_MODEL = "claude-opus-placeholder"

_EDIT_REQUEST = dict(
    method="session/request_permission",
    params=dict(sessionId="sess-1", toolCall=dict(toolCallId="tc-edit", kind="edit"),
                options=[dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")]),
)
_DELETE_REQUEST = dict(
    method="session/request_permission",
    params=dict(sessionId="sess-1", toolCall=dict(toolCallId="tc-delete", kind="delete"),
                options=[dict(optionId="allow", kind="allow_once"), dict(optionId="deny", kind="reject_once")]),
)


# --------------------------------------------------------------------
# Real Agent, driven over a real pipe -- same pattern as
# tests/test_acp_elicitation.py's ``_Harness``/tests/test_acp_host_
# permission_requester.py, but wired to the real
# ``ProductionOrchestratingRuntime`` instead of a hand-assembled one.
# --------------------------------------------------------------------


class _RecordingStdout:
    def __init__(self):
        self.lines = []
        self._lock = threading.Lock()

    def write(self, data):
        with self._lock:
            for line in data.splitlines():
                if line:
                    self.lines.append(json.loads(line.decode("utf-8")))

    def flush(self):
        pass


class _ProductionHarness:
    """One real ``Agent`` (the exact class ``cortex_acp.py``'s ``main()``
    constructs), backed by one real ``ProductionOrchestratingRuntime`` (the
    exact class ``main()`` passes it), driven over a real pipe."""

    def __init__(self, environ):
        self.runtime = ProductionOrchestratingRuntime(environ=environ)
        self.agent = Agent(self.runtime, environ=environ)
        self.stdout = _RecordingStdout()
        self.read_fd, self.write_fd = os.pipe()
        self.stdin = os.fdopen(self.read_fd, "rb")
        self.transport = StdioTransport(self.agent.handle_message, raw_stdout=self.stdout, raw_stdin=self.stdin)
        self.agent.bind_transport(self.transport)
        self._request_id = 0

    def start(self):
        self.transport.start()

    def stop(self):
        self.transport.stop_writer()
        try:
            os.close(self.write_fd)
        except OSError:
            pass
        try:
            self.stdin.close()
        except OSError:
            pass
        # Kill every engine subprocess this runtime's own real adapters
        # spawned -- never leave an orphan behind.
        for orch_runtime in list(self.runtime._runtimes.values()):
            for manager in list(orch_runtime._engine._managers.values()):
                try:
                    manager.shutdown(grace=2.0)
                except Exception:
                    pass

    def send(self, method, params, *, notification=False):
        self._request_id += 1
        request_id = self._request_id
        body = dict(jsonrpc="2.0", method=method, params=params)
        if not notification:
            body["id"] = request_id
        os.write(self.write_fd, (json.dumps(body) + "\n").encode("utf-8"))
        return request_id

    def send_raw(self, body):
        os.write(self.write_fd, (json.dumps(body) + "\n").encode("utf-8"))

    def wait_for(self, predicate, timeout=60.0):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            with self.stdout._lock:
                for message in self.stdout.lines:
                    if predicate(message):
                        return message
            time.sleep(0.02)
        raise AssertionError(f"timed out waiting for a matching message; got {self.stdout.lines}")

    def response_for(self, request_id, timeout=60.0):
        return self.wait_for(lambda m: m.get("id") == request_id and ("result" in m or "error" in m), timeout=timeout)


def _isolate(tmp_path, monkeypatch):
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))
    return dict(HOME=str(home), XDG_CONFIG_HOME=str(config_home), XDG_STATE_HOME=str(state_home))


def _register_work_connection(tmp_path, engine_dir, environ, *, session_new_params_dump_path=None):
    """One real connection registered purely through ``cortex_acp_setup.py``
    (the real one-time-setup script), pinned as the default route -- reused
    for both the coordinator and every worker attempt below (the same
    single-connection shape ``test_acp_setup_fresh_session.py`` uses).

    Scripted to send the same ``[edit, delete]`` permission-request pair on
    every prompt turn it runs, regardless of which role opened it --
    ``engine.permissions.PermissionBridge`` is what must tell a worker's
    edit (auto-approved) from a coordinator's edit (refused) apart, not
    this scenario.
    """
    scenario_path = write_scenario(engine_dir, default_scenario(
        config_options=[
            dict(id="model", currentValue=_MODEL, options=[dict(value=_MODEL, name="Placeholder")]),
            # claude-agent-acp's own WORKER_MODE_FLOOR/COORDINATOR_MODE_FLOOR
            # (configuration.policy) both require "default" to be an
            # actually-offered mode option before either role's own engine
            # session can be trusted to auto-approve/refuse a permission
            # request without a live host round trip -- see
            # test_delegation_worker_mode_floor.py's own fixture for the
            # identical shape.
            dict(id="mode", currentValue="default", options=[dict(value="default", name="Manual")]),
        ],
        configure_behavior="accept",
        prompt_updates=[dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="Ready."))],
        prompt_stop_reason="end_turn",
        prompt_behavior="client_requests_sequence",
        client_requests=[_EDIT_REQUEST, _DELETE_REQUEST],
        session_new_params_dump_path=str(session_new_params_dump_path) if session_new_params_dump_path else None,
    ))
    secret_file = tmp_path / "secret-work.txt"
    secret_file.write_bytes(b"fake-api-key")
    conn = setup.add_connection(
        name="work", engine="claude-agent-acp", provider="anthropic", auth_mode=AuthMode.API_KEY,
        key_kind="anthropic_api_key", api_key_reader=lambda: secret_file.read_bytes(),
        launch_command=fake_launch_command(scenario_path), environ=environ,
    )
    pinned = setup.pin_route(connection_id=conn["connection_id"], model=_MODEL, environ=environ)
    return conn, pinned


def _wire_mcp_call(env, cwd, messages):
    result = subprocess.run(
        [sys.executable, "-B", str(CORTEX_PY)], input="\n".join(json.dumps(m) for m in messages) + "\n",
        text=True, capture_output=True, env=env, cwd=cwd, timeout=30,
    )
    assert result.returncode == 0 and result.stderr == "", result.stderr
    return [json.loads(line) for line in result.stdout.splitlines()]


def _init_and(method, params, id_=2):
    return [
        dict(jsonrpc="2.0", id=1, method="initialize",
             params=dict(protocolVersion="2025-11-25", capabilities={}, clientInfo=dict(name="t", version="1"))),
        dict(jsonrpc="2.0", method="notifications/initialized"),
        dict(jsonrpc="2.0", id=id_, method=method, params=params),
    ]


def test_composed_production_chain(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = tmp_path / "project"
    project.mkdir()
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    session_new_dump = engine_dir / "session_new_params.json"

    # -----------------------------------------------------------------
    # Phase 0: real one-time setup (cortex_acp_setup.py), real connection
    # registry, fake credential bytes.
    # -----------------------------------------------------------------
    setup.init_storage(environ=environ)
    conn, pinned = _register_work_connection(tmp_path, engine_dir, environ, session_new_params_dump_path=session_new_dump)

    harness = _ProductionHarness(environ)
    harness.start()
    try:
        # -------------------------------------------------------------
        # Phase 1: real entrypoint classes (Agent + ProductionOrchestrating
        # Runtime) over a real pipe. First prompt of a brand-new task:
        # bootstrap -> auto-propose the setup default -> real coordinator
        # engine session opens -> its configuration is confirmed over the
        # real wire (EXT-01 #2's own property: this is the exact
        # first-open path test_acp_setup_fresh_session.py proves in
        # isolation; the composed property under test here is that it
        # still holds when reached through the real Agent/pipe instead of
        # a hand-built runtime) -- and the same turn's own scripted
        # [edit, delete] permission requests are BOTH refused locally,
        # by the real PermissionBridge, because this is a coordinator
        # binding: no session/request_permission for either ever reaches
        # this real incoming host side.
        # -------------------------------------------------------------
        init_id = harness.send("initialize", dict(protocolVersion=1, clientCapabilities={}))
        harness.response_for(init_id)
        new_id = harness.send("session/new", dict(cwd=str(project), mcpServers=[]))
        session_id = harness.response_for(new_id)["result"]["sessionId"]

        prompt_id = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Ship the report.")]))
        final = harness.response_for(prompt_id, timeout=60.0)
        assert final["result"]["stopReason"] == "end_turn"
        assert not any(m.get("method") == "session/request_permission" for m in harness.stdout.lines), (
            "a coordinator's own mutating requests must never reach the real host side"
        )

        store = harness.agent._store_for(str(project))
        task_id = harness.agent.sessions[session_id]["task_id"]
        assert task_id

        service = ConfigurationService(store)
        active = service.current(task_id)
        assert active is not None, "the coordinator's first open never confirmed its configuration"
        assert active["confirmation_evidence_ref"], "confirmation must carry real wire evidence, not a guess"
        assert active["route_connection_id"] == conn["connection_id"]

        with store.connection() as db:
            refusal = db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='progress'", (task_id,),
            ).fetchone()
        assert refusal is not None, "the coordinator mutation refusal must be durably recorded for the model to read"
        refusal_payload = json.loads(refusal["payload_json"])
        assert refusal_payload["message_key"] == "coordinator_mutation_refused"

        # -------------------------------------------------------------
        # Phase 2: EXT-01 #8 -- a failed engine startup must never leak
        # capacity or an orphan process. Driven against a *separate*
        # adapter instance sharing this same real, on-disk Store (so it
        # does not disturb the primary adapter's own live bookkeeping
        # mid-composition), same real fake-engine connection/launch
        # command, same technique as
        # test_delegation_live_adapters_open_cleanup.py.
        # -------------------------------------------------------------
        from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter

        registry_state = _read_registry(environ)
        record = registry_state.connections[conn["connection_id"]]
        fault_adapter = EngineSessionManagerAdapter(
            lambda _t: record, lambda _t: str(project), environ=environ, call_timeout=10.0,
            store=store, capacity_limit=4,
        )
        original_call = EngineClient.call
        pids: list[int] = []
        active_before_fault = delegation_capacity.count_active(store, conn["connection_id"])

        def _raise_on_session_new(self, method, params=None, *, timeout=60.0):
            pids.append(self._process.process.pid)
            if method == "session/new":
                raise RuntimeError("injected session/new failure")
            return original_call(self, method, params, timeout=timeout)

        # NOTE: deliberately NOT monkeypatch.setattr/monkeypatch.undo() --
        # monkeypatch.undo() reverts *every* patch this fixture has made so
        # far in the test, including _isolate()'s own HOME/XDG_CONFIG_HOME/
        # XDG_STATE_HOME env redirection, which would silently point every
        # later real-side-effecting call (cortex_acp_setup.add_connection
        # with no explicit environ=) at the real user's own
        # ~/.config/cortex. Restore the one attribute directly instead.
        EngineClient.call = _raise_on_session_new
        try:
            with pytest.raises(RuntimeError, match="injected session/new failure"):
                fault_adapter.open("shared_project_root", dict(task_id=task_id, attempt_id="xa_fault"), "instructions")
        finally:
            EngineClient.call = original_call
        assert pids, "the real engine process must actually have spawned before the injected failure"
        assert not fault_adapter._managers and not fault_adapter._sessions and not fault_adapter._reservations
        assert _pid_gone(pids[0]), "no orphan process left behind by the failed open"
        with store.connection() as db:
            row = db.execute(
                "SELECT state, release_observation FROM capacity_reservations WHERE task_id=? AND attempt_id=?",
                (task_id, "xa_fault"),
            ).fetchone()
        assert row["state"] == "released"
        assert row["release_observation"] == "process_exited"
        assert delegation_capacity.count_active(store, conn["connection_id"]) == active_before_fault, (
            "the injected startup failure must not leave a phantom reservation counted against this connection "
            "beyond whatever was already legitimately active (this task's own still-open coordinator session)"
        )

        # -------------------------------------------------------------
        # Phase 3: real delegation through the real continuation loop --
        # mcp_tools.dispatch("delegate") (the real production dispatch
        # function; see this file's own module docstring for why this
        # stands in for the engine's own MCP call) queues the delegation,
        # then a second real session/prompt over the real pipe drives
        # ProductionOrchestratingRuntime -> OrchestratingRuntime ->
        # ProvisioningService -> the real IsolatedSnapshotWorkspaceProvisioner
        # -> the real EngineSessionManagerAdapter.open() for the worker,
        # spawning a second real fake-engine subprocess. The worker's own
        # scripted [edit, delete] pair: edit is auto-approved locally
        # (worker role, non-mutating-enough), delete reaches this real
        # incoming host side and is answered over the real pipe (EXT-01 #5).
        # -------------------------------------------------------------
        prod_runtime = harness.runtime._runtime_for_project(str(project))
        coordinator_channel = prod_runtime._coordinator_channels[task_id]
        mcp_tools.dispatch(store, coordinator_channel, "delegate", dict(
            role="backend-dev", title="Investigate", brief="Look into it.", acceptance="done",
            request_key=str(uuid.uuid4()),
        ))

        prompt2_id = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Please continue.")]))

        permission_request = harness.wait_for(
            lambda m: m.get("method") == "session/request_permission" and
            m.get("params", {}).get("toolCall", {}).get("toolCallId") == "tc-delete",
            timeout=60.0,
        )
        assert isinstance(permission_request.get("id"), str)
        harness.send_raw(dict(jsonrpc="2.0", id=permission_request["id"],
                               result={"outcome": {"outcome": "selected", "optionId": "deny"}}))

        final2 = harness.response_for(prompt2_id, timeout=60.0)
        assert final2["result"]["stopReason"] == "end_turn"

        with store.connection() as db:
            provisioned = db.execute(
                "SELECT attempt_id, workspace_binding_id, engine_session_ref FROM delegation_provisioning WHERE task_id=?", (task_id,),
            ).fetchone()
        assert provisioned is not None, "the real continuation loop never provisioned a worker"
        attempt_id = provisioned["attempt_id"]
        workspace_binding_id = provisioned["workspace_binding_id"]
        # Read from the durable provisioning row, not the adapter's live
        # map: this worker's turn ended without publishing anything, which
        # closes the assignment and tears its session down -- correct in
        # production (a worker that reported nothing is over), while phase
        # 5 below deliberately reuses this exact attempt to drive two
        # recovery generations through the real adapter.
        g1_session_ref = provisioned["engine_session_ref"]
        assert g1_session_ref

        # Real isolated workspace copy actually exists, excludes .git/.cortex.
        workspace_root = Path(prod_runtime._workspace.root_for(workspace_binding_id))
        assert workspace_root != project
        assert not (workspace_root / ".cortex").exists()
        assert not (workspace_root / ".git").exists()

        # Exactly one delete permission request ever reached this real host
        # side -- never the auto-approved edit, never a coordinator's own.
        delete_requests = [
            m for m in harness.stdout.lines
            if m.get("method") == "session/request_permission"
        ]
        assert len(delete_requests) == 1
        assert delete_requests[0]["params"]["toolCall"]["toolCallId"] == "tc-delete"

        # -------------------------------------------------------------
        # Phase 4: EXT-01 #1 -- the worker's real Cortex MCP subprocess
        # must see the project's own logical root, never its isolated
        # workspace copy. Captured from the exact session/new params the
        # real adapter computed for this worker's session (dumped by the
        # fake engine itself), then driven for real over JSON-RPC, exactly
        # as test_ext01_worker_mcp_project_root.py's own regression test
        # does -- composed here after the rest of the chain rather than in
        # isolation.
        # -------------------------------------------------------------
        session_new_params = json.loads(session_new_dump.read_text())
        assert session_new_params["cwd"] == str(workspace_root)
        mcp_entry = session_new_params["mcpServers"][0]
        mcp_env = {item["name"]: item["value"] for item in mcp_entry["env"]}
        assert mcp_env["CORTEX_CHANNEL_PROJECT_ROOT"] == str(project)
        assert mcp_env["CORTEX_CHANNEL_PROJECT_ROOT"] != str(workspace_root)

        wire_env = os.environ | dict(
            CODEX_HOME=str(engine_dir / "mcp_home"),
            CORTEX_CHANNEL_ID=mcp_env["CORTEX_CHANNEL_ID"],
            CORTEX_CHANNEL_SECRET=mcp_env["CORTEX_CHANNEL_SECRET"],
            CORTEX_CHANNEL_PROJECT_ROOT=mcp_env["CORTEX_CHANNEL_PROJECT_ROOT"],
        )
        (Path(wire_env["CODEX_HOME"]) / "sessions").mkdir(parents=True, exist_ok=True)

        replies = _wire_mcp_call(wire_env, str(workspace_root), _init_and(
            "tools/call", dict(name="list_sources", arguments=dict(request_key="list-ext01c"))))
        listed = json.loads(replies[1]["result"]["content"][0]["text"])
        assert listed["entries"], "the worker's own MCP subprocess must reach the real project's own store"
        source_ref = listed["entries"][0]["source_ref"]

        replies = _wire_mcp_call(wire_env, str(workspace_root), _init_and(
            "tools/call", dict(name="read_source", arguments=dict(source_ref=source_ref, request_key="read-ext01c"))))
        read = json.loads(replies[1]["result"]["content"][0]["text"])
        assert "Ship the report" in read.get("chunk", "") or "Please continue" in read.get("chunk", "")

        stores_found = list(tmp_path.glob("**/cortex.sqlite3"))
        assert stores_found == [project / ".cortex" / "cortex.sqlite3"], (
            "the fix must never spin up a second store inside the isolated workspace copy"
        )

        # -------------------------------------------------------------
        # Phase 5: two recovery generations for this exact worker attempt
        # -- the binding replacement and delivery-confirmation transition
        # from EXT-01 #3, driven through the *same real*
        # EngineSessionManagerAdapter production instance the continuation
        # loop above already used (``prod_runtime._engine``), never a
        # re-implementation. Generation 1 already opened in phase 3;
        # this closes it out (terminate) and opens generation 2 for the
        # identical attempt_id, the disclosed substitute for whatever
        # real trigger (a crash, a lost heartbeat) would cause this in
        # production -- see this file's own module docstring.
        # -------------------------------------------------------------
        adapter = prod_runtime._engine
        g1 = delegation_generations.latest_generation(store, g1_session_ref)
        assert g1["cause"] == "session_open"

        adapter.terminate_owned(attempt_id)
        assert attempt_id not in adapter._managers

        launch2 = dict(task_id=task_id, attempt_id=attempt_id, assignment_id=provisioned["attempt_id"], recovery=True)
        g2_session_ref = adapter.open(workspace_binding_id, launch2, "instructions")
        try:
            assert g2_session_ref != g1_session_ref, "a replacement must be a genuinely new engine session"
            g2 = delegation_generations.latest_generation(store, g2_session_ref)
            assert g2["cause"] == "session_replacement"
            assert g2["id"] != g1["id"]

            # Delivery-confirmation transition on this fresh generation,
            # via the real adapter's own production methods.
            delivery = adapter.begin_delivery(g2_session_ref, capsule_revision="composed-pass-rev")
            assert delivery is not None
            assert delegation_deliveries.get(store, delivery["id"])["state"] == "built"
            adapter.mark_delivery_transmitted(g2_session_ref, delivery["id"])
            assert delegation_deliveries.get(store, delivery["id"])["state"] == "transmitted"
            adapter.mark_delivery_confirmed(g2_session_ref, delivery["id"])
            confirmed = delegation_deliveries.get(store, delivery["id"])
            assert confirmed["state"] == "delivery_confirmed"
            assert adapter.pending_delivery(g2_session_ref) is None
        finally:
            adapter.terminate_owned(attempt_id)

        # -------------------------------------------------------------
        # Phase 6: cancellation that does not depend on the engine
        # emitting anything (EXT-01 #4) -- driven against a real fake-
        # engine subprocess scripted with ``silent_until_cancel`` (built
        # for exactly this), through the real adapter, never a hand-rolled
        # stand-in manager.
        # -------------------------------------------------------------
        silent_scenario_path = write_scenario(engine_dir, default_scenario(
            config_options=[
                dict(id="model", currentValue=_MODEL, options=[dict(value=_MODEL, name="Placeholder")]),
                dict(id="mode", currentValue="default", options=[dict(value="default", name="Manual")]),
            ],
            configure_behavior="accept",
            prompt_behavior="silent_until_cancel",
        ))
        silent_conn = setup.add_connection(
            name="silent", engine="claude-agent-acp", provider="anthropic", auth_mode=AuthMode.API_KEY,
            key_kind="anthropic_api_key", api_key_reader=lambda: b"fake-api-key",
            launch_command=fake_launch_command(silent_scenario_path), environ=environ,
        )
        silent_registry_state = _read_registry(environ)
        silent_record = silent_registry_state.connections[silent_conn["connection_id"]]
        cancel_adapter = EngineSessionManagerAdapter(
            lambda _t: silent_record, lambda _t: str(project), environ=environ, call_timeout=10.0,
            store=store, capacity_limit=4,
        )
        try:
            cancel_session_ref = cancel_adapter.open(
                "shared_project_root", dict(task_id=task_id, attempt_id="xa_cancel"), "instructions")
            cancel_event = threading.Event()
            outcome_box: dict = {}

            def _run_prompt():
                outcome_box["outcome"] = cancel_adapter.prompt(cancel_session_ref, "do the thing", cancel_event)

            prompt_thread = threading.Thread(target=_run_prompt, daemon=True)
            prompt_thread.start()
            time.sleep(0.3)  # let it actually park inside the real engine's own silent turn
            cancel_event.set()
            prompt_thread.join(timeout=10.0)
            assert not prompt_thread.is_alive(), (
                "the real engine's own session/cancel must arrive even though it never emitted an event"
            )
            assert outcome_box["outcome"].stop_reason.value == "cancelled"
        finally:
            cancel_adapter.terminate_owned("xa_cancel")
    finally:
        harness.stop()


def _read_registry(environ):
    from cortex_runtime.connections import registry as connection_registry

    return connection_registry.read_registry(environ)


def _pid_gone(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except ProcessLookupError:
        return True
    except OSError:
        return True
    return False


# ---------------------------------------------------------------------------
# WHAT THIS PASS WOULD NOT HAVE CAUGHT
#
# - A real downstream engine's own actual ACP quirks (codex-acp/
#   claude-agent-acp specific wire shapes, timing, or bugs) -- the fake
#   engine is scripted and cooperative; qualification against the real
#   adapters is covered elsewhere (tests/test_engine_codex_evidence.py,
#   tests/test_engine_claude_install.py, docs/acp/*-adapter.md) and is
#   explicitly out of scope here (no paid calls).
# - Anything requiring a real downstream engine to itself drive an MCP
#   tool call over its own stdio to a spawned cortex.py -- the coordinator
#   side of that link is exercised via direct dispatch (see this file's
#   own module docstring); only the worker side's MCP round trip is
#   literal.
# - ``ProvisioningService.delegate``'s own routing/fallback-authorization
#   logic when more than one connection is qualified for a role (this pass
#   registers exactly one connection) -- covered by
#   tests/test_delegation_routing_dispatch.py.
# - The full crash-recovery trigger for a second engine generation
#   (``recovery.worker_reconciliation`` detecting an actually-dead
#   process) -- this pass calls the same real adapter methods that trigger
#   would call, but does not reproduce the crash detection itself; that is
#   covered by tests/test_delegation_provisioning_recovery.py and
#   tests/test_recovery_cross_process.py.
# - Two independent OS processes/hosts talking to the same project
#   concurrently -- this pass is single-process throughout.
# ---------------------------------------------------------------------------
