"""OpenCode v2 through Cortex's own launch path, with a loopback fake model; never a paid call.

Opt in with ``CORTEX_PINNED_OPENCODE_BINARY=<absolute path of the installed
opencode binary>``. The check runs the production chain -- verified install,
``EngineSessionManager``/``EngineProcess``, the engine jail with process
isolation, the masked user configuration, the in-memory agent prompt, the
brokered Cortex MCP server and the permission bridge -- in a disposable HOME
and project. The only test seam is a fake OpenAI-compatible provider added
to Cortex's own OpenCode configuration, because a production configuration
never names a provider. No real credential, production HOME or remote model is
used. The owner's own configuration names a second provider on the same
loopback endpoint with a stand-in key, which Cortex must carry over (as the
owner's custom providers exist only there) while hiding everything else.

The owner's data directory is the one OpenCode itself creates, with
stand-in logins in its login database: one for a models.dev provider
(``nvidia``), which on OpenCode 2.0.20 made ``session/new`` offer only
``build``/``plan`` before its configuration loads unless Cortex removes the
built-in ``opencode.agent``/``opencode.plan`` plugins, and one for a third
owner-configured provider whose key exists only as that login. Building that directory runs the binary once, prompt-free,
and fetches OpenCode's public models.dev catalog.
"""
from __future__ import annotations

import sqlite3
import json
import os
import shutil
import subprocess
import uuid
from pathlib import Path

import pytest

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.connections import environment
from cortex_runtime.delegation.live_adapters import _option_mapping_for
from cortex_runtime.engine import opencode_adapter_acp, opencode_install
from cortex_runtime.engine.resource_binding import ResourceBinding
from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.orchestration.sandbox import engine_jail
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.engine_contract import EngineEventKind, StopReason
from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute, SourceEvent
from cortex_runtime.store import Store
from engine_fakes import isolated_environ, make_connection_record, new_connection_id
from opencode_support import FakeProvider, bootstrap_login_database, last_user_text, tool_results

BINARY = os.environ.get("CORTEX_PINNED_OPENCODE_BINARY")
pytestmark = [
    pytest.mark.skipif(not BINARY, reason="set CORTEX_PINNED_OPENCODE_BINARY for the installed no-paid OpenCode check"),
    pytest.mark.skipif(shutil.which("bwrap") is None, reason="bubblewrap is required"),
]

ROLE_MARKER = "CORTEX-ROLE-MARKER-7f3a"
RULES_MARKER = "PROJECT-RULES-MARKER-19c2"
MODEL = "fake/m1"
OWNER_MODEL = "owner/m2"
OWNER_KEY = "OWNER-STAND-IN-KEY-5d2e"
LOGIN_MODEL = "ownerdb/m3"
LOGIN_KEY = "OWNER-LOGIN-STAND-IN-KEY-8a41"

_CORTEX_PROBE = (
    "const items = search({ namespace: 'cortex', limit: 100 }).items.map(i => i.path);\n"
    "let reports = null;\n"
    "if (items.includes('tools.cortex.list_reports')) reports = await tools.cortex.list_reports({});\n"
    "return JSON.stringify({ items, reports }).slice(0, 4000);"
)


def _turns():
    """What the fake model does, per user prompt: one list of steps each."""
    return {
        "turn-tools": [
            ("tool", "execute", {"code": _CORTEX_PROBE}),
            ("tool", "shell", {"command": "echo in > src/in.txt; echo out > out.txt; echo done",
                               "description": "write inside and outside the scope"}),
            ("text", "tools done"),
        ],
        "turn-delete": [("tool", "shell", {"command": "rm src/keep.txt", "description": "delete"}),
                        ("text", "delete done")],
        "turn-stall": [("stall", 120)],
        "turn-owner": [("text", "owner done")],
        "turn-login": [("text", "login done")],
    }


@pytest.fixture
def setup(tmp_path, monkeypatch):
    environ = isolated_environ(tmp_path)
    home = Path(environ["HOME"])
    markers = tmp_path / "markers"
    markers.mkdir()
    # The owner's own OpenCode configuration: an agent and an MCP server that
    # would leave a marker. Cortex must hide both.
    (home / ".config" / "opencode").mkdir(parents=True)
    (home / ".config" / "opencode" / "opencode.json").write_text(json.dumps({
        "agent": {"owneragent": {"mode": "primary", "prompt": "OWNER-AGENT"}},
        "mcp": {"ownermcp": {"type": "local", "command": ["/bin/sh", "-c", f"touch {markers}/user-mcp; sleep 5"]}},
    }))
    project = (tmp_path / "project").resolve()
    (project / "src").mkdir(parents=True)
    (project / "src" / "keep.txt").write_text("keep\n")
    (project / "AGENTS.md").write_text(RULES_MARKER + "\n")
    # Project configuration: an agent, a plugin and an agent file; none may load.
    (project / "opencode.json").write_text(json.dumps({"agent": {"projectagent": {"mode": "primary", "prompt": "P"}}}))
    (project / ".opencode" / "plugin").mkdir(parents=True)
    (project / ".opencode" / "plugin" / "mark.js").write_text(
        f"import fs from 'fs'; fs.writeFileSync('{markers}/plugin', 'x'); export const Mark = async () => ({{}});")
    (project / ".opencode" / "agent").mkdir(parents=True)
    (project / ".opencode" / "agent" / "dotagent.md").write_text("---\nmode: primary\n---\nDOT\n")

    turns = _turns()

    def route(body):
        text = last_user_text(body)
        queue = next((steps for name, steps in turns.items() if name in text), None)
        return queue.pop(0) if queue else ("text", "done")

    provider = FakeProvider(router=route)
    environ["OWNER_TEST_BASE_URL"] = provider.base_url
    # The owner's configuration also defines a provider of its own; Cortex
    # carries only that, its key moved into a read-only jail file.
    (home / ".config" / "opencode" / "opencode.json").write_text(json.dumps({
        "agent": {"owneragent": {"mode": "primary", "prompt": "OWNER-AGENT"}},
        "mcp": {"ownermcp": {"type": "local", "command": ["/bin/sh", "-c", f"touch {markers}/user-mcp; sleep 5"]}},
        "provider": {"owner": {"npm": "@ai-sdk/openai-compatible", "name": "Owner",
                               "options": {"baseURL": "{env:OWNER_TEST_BASE_URL}", "apiKey": OWNER_KEY},
                               "models": {"m2": {"name": "M2"}}},
                     # Its key exists only as a login in the owner's database.
                     "ownerdb": {"npm": "@ai-sdk/openai-compatible", "name": "OwnerDb",
                                 "options": {"baseURL": "{env:OWNER_TEST_BASE_URL}"},
                                 "models": {"m3": {"name": "M3"}}}},
    }))
    original = opencode_adapter_acp.build_opencode_config

    def with_fake_provider(prompt_path):
        config = original(prompt_path)
        config["provider"] = {"fake": {"npm": "@ai-sdk/openai-compatible", "name": "Fake",
                                       "options": {"baseURL": provider.base_url, "apiKey": "none"},
                                       "models": {"m1": {"name": "M1"}}}}
        return config

    monkeypatch.setattr(opencode_adapter_acp, "build_opencode_config", with_fake_provider)

    store = Store(project / ".cortex", project_root=project)
    boot = bootstrap_task(store, SourceEvent(origin="opencode-qualification", event_id=str(uuid.uuid4())),
                          "OpenCode qualification", "owner")
    with store.connection() as db:
        epoch = current_owner_epoch(db, boot.task_id)
    install = opencode_install.verify_install(opencode_install.build_install(BINARY, environ), environ)
    record = make_connection_record(new_connection_id(), opencode_install.launch_command_for(install),
                                    engine="opencode", environ=environ)
    login_database = bootstrap_login_database(
        BINARY, environment.credential_directory(record, environ), tmp_path / "bootstrap",
        {"nvidia": "NVIDIA-LOGIN-STAND-IN", "ownerdb": LOGIN_KEY})
    with sqlite3.connect(login_database) as db:
        login_rows = db.execute("SELECT * FROM credential ORDER BY id").fetchall()
    requests = []

    def host_requester(engine_session_id, params, timeout):
        requests.append(params)
        reject = next(o["optionId"] for o in params["options"] if o["kind"].startswith("reject"))
        return {"outcome": {"outcome": "selected", "optionId": reject}}

    prompt = opencode_adapter_acp.build_agent_prompt(
        f"{ROLE_MARKER}: you are a worker in a check.", role="worker", project_root=str(project), environ=environ)
    manager = EngineSessionManager(
        record, str(project), environ=environ, install=install,
        channel_credentials=(boot.channel_id, boot.secret, str(project)), broker_mcp=True, mcp_role="worker",
        host_permission_requester=host_requester, jail={"scopes": ["src/**"], "readonly": []},
        engine_instructions=prompt, initialize_timeout=60.0, call_timeout=60.0,
        # The production dispatch (EngineSessionManagerAdapter.open).
        option_mapping=_option_mapping_for("opencode"),
    )
    try:
        yield dict(manager=manager, provider=provider, project=project, markers=markers, task_id=boot.task_id,
                   epoch=epoch, record=record, requests=requests, tmp_path=tmp_path, home=home,
                   login_database=login_database, login_rows=login_rows)
    finally:
        manager.shutdown()
        provider.close()


def _bind(manager, session_id, project, epoch):
    manager.bind_resource(ResourceBinding(
        engine_instance_id=manager._process.engine_instance_id, engine_session_id=session_id,
        context_generation_id="cg_" + uuid.uuid4().hex[:12], assignment_id="asg_qualification",
        attempt_id="xa_qualification", workspace_binding_id="shared_project_root",
        workspace_root=os.path.realpath(project), ownership_epoch=str(epoch), role="worker",
    ))


def _turn(manager, session_id, text):
    return list(manager.prompt_turn(session_id, text))


def _opencode_pids(root_pid):
    """Only OpenCode processes descended from this fixture's owned launch."""
    found, pending, seen = [], [str(root_pid)], set()
    while pending:
        name = pending.pop()
        if name in seen:
            continue
        seen.add(name)
        try:
            for task in Path(f"/proc/{name}/task").iterdir():
                pending.extend((task / "children").read_text().split())
            argv = Path(f"/proc/{name}/cmdline").read_bytes().split(b"\0")
        except OSError:
            continue
        if argv and argv[0].endswith(b"opencode"):
            found.append(name)
    return found


@pytest.mark.requires_bwrap
def test_opencode_runs_through_the_cortex_launch_path(setup):
    manager, provider, project = setup["manager"], setup["provider"], setup["project"]

    try:
        manager.negotiate(())
    except Exception:
        # The engine's own (redacted) stderr says why it did not start.
        log = manager._process.logs.directory / "stderr.log"
        pytest.fail("OpenCode did not initialize: " + (log.read_text()[-3000:] if log.exists() else "no stderr"))
    session_id = manager.create_session(setup["task_id"], "route")
    options = manager.observed_config_options(session_id)
    modes = [c["value"] for o in options if o["id"] == "mode" for c in o["options"]]
    models = [c["value"] for o in options if o["id"] == "model" for c in o["options"]]
    # Only Cortex's agent: the owner's agent, the project's agent and the
    # .opencode agent never load -- and a login in the owner's database does
    # not hide it, because no built-in primary agent exists to be offered first.
    assert modes == ["cortex"]
    assert MODEL in models and OWNER_MODEL in models and LOGIN_MODEL in models
    # The logged-in models.dev provider arrives with the custom ones.
    assert any(model.startswith("nvidia/") for model in models)
    login_database = setup["login_database"]
    assert sorted(path.name for path in login_database.parent.glob("*.db")) == ["opencode.db"]

    route = ExecutionRoute(engine="opencode", connection_id=setup["record"].id, model=MODEL,
                           supported_parameters=(("thought_level", "high"), ("mode", "cortex")))
    evidence = manager.configure(session_id, ConfigurationRevision(
        id="cr_" + uuid.uuid4().hex[:12], task_id=setup["task_id"], revision=1, route=route))
    assert evidence.endswith("model=fake/m1;effort=high;mode=cortex")
    _bind(manager, session_id, project, setup["epoch"])

    # Turn 1: the Cortex MCP gateway through Code Mode, then native writes.
    events = _turn(manager, session_id, "turn-tools")
    end = [e for e in events if e.kind == EngineEventKind.TURN_END]
    assert end and end[-1].stop_reason == StopReason.END_TURN
    assert any(e.kind == EngineEventKind.USAGE_UPDATE for e in events)
    first = provider.agent_requests[0]
    system = provider.system_text(0)
    assert ROLE_MARKER in system and RULES_MARKER in system
    assert "OWNER-AGENT" not in json.dumps(provider.requests)
    tools = provider.tool_names(0)
    assert "execute" in tools and "shell" in tools
    assert not {"subagent", "question", "skill", "task"} & set(tools)
    assert first.get("model") == "m1"
    results = tool_results(provider.agent_requests[-1])
    probe = next((result for result in results if "tools.cortex." in result), None)
    if probe is None:
        import re

        catalog = re.findall(r"(?m)^- \S+ \([^)]*\).*$", system)
        observed = (manager._process.logs.directory / "observations.jsonl").read_text()[-4000:]
        stderr = (manager._process.logs.directory / "stderr.log")
        pytest.fail(json.dumps(dict(results=[r[:300] for r in results], catalog=catalog, observed=observed,
                                    stderr=stderr.read_text()[-2000:] if stderr.exists() else None), indent=1))
    assert "tools.cortex.list_reports" in probe and '"reports":null' not in probe.replace(" ", "")
    assert (project / "src" / "in.txt").read_text() == "in\n"
    assert not (project / "out.txt").exists()
    assert not (setup["markers"] / "user-mcp").exists() and not (setup["markers"] / "plugin").exists()
    # Native calls were asked about and answered by Cortex's bridge, without
    # a user card: nothing here deletes.
    assert setup["requests"] == []

    # Another jail cannot read the control server's environment.
    pids = [pid for pid in _opencode_pids(manager._process.process.pid)
            if b"serve" in Path(f"/proc/{pid}/cmdline").read_bytes().split(b"\0")]
    assert pids
    probe_script = ("import sys\nok=0\nfor p in sys.argv[1:]:\n try:\n  open('/proc/%s/environ'%p,'rb').close();ok+=1\n"
                    " except OSError: pass\nprint(ok)")
    scratch = setup["tmp_path"] / "sibling"
    scratch.mkdir()
    with engine_jail(project, scratch, ["/usr/bin/python3", "-c", probe_script, *pids], [],
                     env={"HOME": str(setup["home"]), "PATH": "/usr/bin:/bin"}) as (command, fds):
        sibling = subprocess.run(command, pass_fds=fds, env={"PATH": "/usr/bin:/bin"},
                                 capture_output=True, text=True, timeout=60)
    assert sibling.stdout.strip() == "0", sibling.stderr

    # Turn 2: a deletion reaches the user; refused, the file stays, and
    # OpenCode ends the turn "cancelled" -- which Cortex recognises as its own
    # refusal (EngineSessionManagerAdapter continues the session on it).
    deleted = _turn(manager, session_id, "turn-delete")
    assert [e.stop_reason for e in deleted if e.kind == EngineEventKind.TURN_END] == [StopReason.CANCELLED]
    assert manager.take_permission_refusal(session_id) is True
    assert manager.take_permission_refusal(session_id) is False
    assert len(setup["requests"]) == 1
    assert setup["requests"][0]["toolCall"]["title"].startswith("Removal blocked pending confirmation")
    assert (project / "src" / "keep.txt").read_text() == "keep\n"

    # Turn 3: cancellation ends a stalled turn as cancelled.
    import threading

    stalled = []
    worker = threading.Thread(target=lambda: stalled.extend(_turn(manager, session_id, "turn-stall")))
    worker.start()
    for _ in range(100):
        if any("turn-stall" in last_user_text(r) for r in provider.agent_requests):
            break
        worker.join(0.2)
    manager.cancel(session_id)
    worker.join(60)
    provider.release()
    assert not worker.is_alive()
    assert [e.stop_reason for e in stalled if e.kind == EngineEventKind.TURN_END] == [StopReason.CANCELLED]

    # Turn 4: the owner's own provider, with the key from its jail file.
    owner_route = ExecutionRoute(engine="opencode", connection_id=setup["record"].id, model=OWNER_MODEL,
                                 supported_parameters=(("mode", "cortex"),))
    owner_evidence = manager.configure(session_id, ConfigurationRevision(
        id="cr_" + uuid.uuid4().hex[:12], task_id=setup["task_id"], revision=2, route=owner_route))
    assert "model=owner/m2" in owner_evidence
    owner_turn = _turn(manager, session_id, "turn-owner")
    assert [e.stop_reason for e in owner_turn if e.kind == EngineEventKind.TURN_END] == [StopReason.END_TURN]
    assert provider.agent_requests[-1].get("model") == "m2"
    assert provider.authorizations[-1] == f"Bearer {OWNER_KEY}"

    # Turn 5: a provider whose key is only a login in the owner's database.
    login_route = ExecutionRoute(engine="opencode", connection_id=setup["record"].id, model=LOGIN_MODEL,
                                 supported_parameters=(("mode", "cortex"),))
    manager.configure(session_id, ConfigurationRevision(
        id="cr_" + uuid.uuid4().hex[:12], task_id=setup["task_id"], revision=3, route=login_route))
    login_turn = _turn(manager, session_id, "turn-login")
    assert [e.stop_reason for e in login_turn if e.kind == EngineEventKind.TURN_END] == [StopReason.END_TURN]
    assert provider.agent_requests[-1].get("model") == "m3"
    assert provider.authorizations[-1] == f"Bearer {LOGIN_KEY}"
    # Inspect only this fixture's owned processes, never another session.
    pids = _opencode_pids(manager._process.process.pid)
    assert pids
    for name in pids:
        environ_bytes = Path(f"/proc/{name}/environ").read_bytes()
        assert OWNER_KEY.encode() not in environ_bytes and LOGIN_KEY.encode() not in environ_bytes
    # Sessions share the native database; login rows must remain unchanged.
    with sqlite3.connect(login_database) as db:
        assert db.execute("SELECT * FROM credential ORDER BY id").fetchall() == setup["login_rows"]

    # Turn 6: a manual compaction still completes although Cortex removes
    # OpenCode's built-in agent plugin (and with it the hidden `compaction`
    # helper agent); it is summarized by the fake model, never a paid one.
    before = len(provider.requests)
    compacted = _turn(manager, session_id, "/compact")
    assert [e.stop_reason for e in compacted if e.kind == EngineEventKind.TURN_END] == [StopReason.END_TURN]
    assert len(provider.requests) > before
    assert sorted(path.name for path in login_database.parent.glob("*.db")) == ["opencode.db"]
