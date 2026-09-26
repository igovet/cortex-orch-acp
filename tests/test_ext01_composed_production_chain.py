"""Both engine protocols through the actual production composition.

The incoming ACP Agent, ProductionOrchestratingRuntime, stores, configuration,
channel broker, MCP subprocesses and Linux verification sandbox are real.
Only model reasoning is scripted by fake_engine_process.py, which calls the
actual broker tools for plan, report, result and synthesis. This proves the
complete deterministic cycle without paid engines; it does not prove GUI
rendering or a real model's behavior.
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
from cortex_runtime.connections import quota
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


def _register_work_connection(tmp_path, engine_dir, environ, *, session_new_params_dump_path=None, engine="claude-agent-acp"):
    """One real connection registered purely through ``cortex_acp_setup.py``
    (the real one-time-setup script), pinned as the default route -- reused
    for both the coordinator and every worker attempt below (the same
    single-connection shape ``test_acp_setup_fresh_session.py`` uses).

    Scripted to send the same ``[edit, delete]`` permission-request pair on
    each normal prompt, regardless of which role opened it. The coordinator
    continuation carrying Cortex's local role refusal skips the scripted
    action once, so the model can react to the instruction --
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
            dict(id="mode", currentValue="default" if engine=="claude-agent-acp" else "agent", options=[dict(value="default" if engine=="claude-agent-acp" else "agent", name="Manual")]),
        ],
        configure_behavior="accept",
        prompt_updates=[dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="Ready."))],
        prompt_stop_reason="end_turn",
        prompt_behavior="client_requests_sequence_skip_role_refusal",
        client_requests=[_EDIT_REQUEST, _DELETE_REQUEST],
        session_new_params_dump_path=str(session_new_params_dump_path) if session_new_params_dump_path else None,
    ))
    secret_file = tmp_path / "secret-work.txt"
    secret_file.write_bytes(b"fake-api-key")
    conn = setup.add_connection(
        name="work", engine=engine, provider="anthropic" if engine=="claude-agent-acp" else "openai", auth_mode=AuthMode.API_KEY,
        key_kind="anthropic_api_key" if engine=="claude-agent-acp" else "openai_api_key", api_key_reader=lambda: secret_file.read_bytes(),
        launch_command=fake_launch_command(scenario_path), environ=environ,
    )
    pinned = setup.pin_route(connection_id=conn["connection_id"], model=_MODEL, environ=environ)
    return conn, pinned


def _wire_mcp_call(env, cwd, messages, *, command=None):
    result = subprocess.run(
        command or [sys.executable, "-B", str(CORTEX_PY)],
        input="\n".join(json.dumps(m) for m in messages) + "\n",
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


@pytest.mark.parametrize("engine", ["codex-acp", "claude-agent-acp"])
def test_composed_production_chain(tmp_path, monkeypatch,engine):
    """Actual incoming/outgoing ACP and MCP subprocesses; only reasoning is scripted."""
    # The real runtime lazily refreshes claude-agent-acp plan usage as part
    # of ordinary session activity; this composed chain runs a real
    # connection with a real (Cortex-owned) isolated credential directory,
    # so unlike a narrower unit test it does reach this spawn point. No
    # paid/real CLI call belongs in this no-paid suite, so it is faked the
    # same way tests/test_usage_accounting.py fakes it.
    def _no_claude_cli(argv, env, cwd, timeout):
        raise FileNotFoundError("no real claude CLI in this test")
    monkeypatch.setattr(quota, "_run_claude_usage_subprocess", _no_claude_cli)
    environ=_isolate(tmp_path,monkeypatch)
    project=tmp_path/'project';project.mkdir()
    engine_dir=tmp_path/'engine';engine_dir.mkdir()
    setup.init_storage(environ=environ)
    conn,pinned=_register_work_connection(tmp_path,engine_dir,environ,engine=engine)
    scenario_path=next(engine_dir.glob('scenario-*.json'))
    scenario=json.loads(scenario_path.read_text());scenario['prompt_behavior']='runtime_gateway'
    scenario_path.write_text(json.dumps(scenario))
    harness=_ProductionHarness(environ);harness.start()
    try:
        init=harness.send('initialize',dict(protocolVersion=1,clientCapabilities={}))
        assert 'result' in harness.response_for(init)
        new=harness.send('session/new',dict(cwd=str(project),mcpServers=[]))
        session=harness.response_for(new)['result']['sessionId']
        prompt=harness.send('session/prompt',dict(sessionId=session,prompt=[dict(type='text',text='Inspect the fixture.')]))
        response=harness.response_for(prompt,timeout=45)
        assert response.get('result',{}).get('stopReason')=='end_turn',response
        from cortex_runtime.store import Store
        from cortex_runtime.orchestration.state import TaskMachine
        store=Store(project/'.cortex',initialize=False,project_root=project)
        with store.connection() as db:
            task=db.execute('SELECT task_id FROM runtime_tasks').fetchone()[0]
        context=TaskMachine(store).context(task)
        assert context['state']=='COMPLETED',context
        # An inspection that changed no project file needs no reviewer; the
        # runtime records why, and verification binds to that observation.
        assert [node['stage'] for node in context['nodes']]==['execution']
        assert context['review_not_required']['reason']=='no_project_changes'
        assert context['verification']['revision_digest']==context['review_not_required']['revision_digest']
        assert context['verification']['checks'][0]['exit_code']==0
        assert all(node['result']['report_digest'] for node in context['nodes'])
        updates=[m['params']['update'] for m in harness.stdout.lines if m.get('method')=='session/update']
        titles=[u.get('title','') for u in updates]
        for operation in ['plan_submit','synthesis_submit']:
            assert any(operation in title for title in titles),(operation,titles)
        # What a worker does stays in the worker: never a coordinator card.
        for operation in ['draft_write','write_report','result_submit']:
            assert not any(operation in title for title in titles),(operation,titles)
        assert not any(str(u.get('toolCallId','')).startswith('worker-action:') for u in updates)
        assert not any(m.get('method')=='session/request_permission' for m in harness.stdout.lines)
    finally:harness.stop()
