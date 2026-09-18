"""EXT-01 finding #1 regression: an isolated worker's real Cortex MCP
subprocess must be told the project's one *logical* project root -- where
``.cortex/cortex.sqlite3`` actually lives -- never its own throwaway
execution workspace copy (which deliberately excludes both ``.git`` and
``.cortex``, see ``delegation.workspace_isolation``'s own module docstring).

This drives the real production wiring
(``delegation.live_adapters.EngineSessionManagerAdapter.open``, backed by a
real ``engine.session_manager.EngineSessionManager``) against the standalone
test-double engine (``tests/fake_engine_process.py`` -- never a real engine)
to capture *exactly* the ``mcpServers`` entry production code computed for
this worker's session, then launches that entry's own ``command``/``args``
as a real ``src/cortex.py`` subprocess -- the actual
Cortex MCP server, not a hand-assembled stand-in -- with its ``cwd`` set to
the isolated workspace copy (exactly where a real engine would run it from),
and drives it over real JSON-RPC to read the task's sources and publish its
result, exactly as a real worker's engine-owned MCP child would.
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import uuid
from pathlib import Path

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.delegation.workspace_isolation import IsolatedSnapshotWorkspaceProvisioner
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario

ROOT = Path(__file__).resolve().parents[1]
CORTEX_PY = ROOT / "src/cortex.py"


def _bootstrap(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(
        store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())),
        "Please read the attached design notes and summarize them.", "owner-1",
    )
    return project, store, result.task_id, result.channel_id, result.secret


def _worker_channel(store, task_id, coordinator_channel_id, attempt_id="xa_ext01"):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        epoch = current_owner_epoch(db, task_id)
        channel_id, secret = issue_channel_tx(
            db, task_id, "worker", epoch, parent_channel_id=coordinator_channel_id,
            assignment_id="a_ext01", attempt_id=attempt_id,
        )
        db.commit()
    return channel_id, secret


def _wire_call(command, args, env, messages):
    result = subprocess.run(
        [*command, *args], input="\n".join(json.dumps(m) for m in messages) + "\n",
        text=True, capture_output=True, env=env, cwd=env.get("_TEST_CWD") or None, timeout=30,
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


def test_isolated_worker_real_mcp_subprocess_uses_project_root_not_workspace(tmp_path):
    project, store, task_id, coordinator_channel_id, _ = _bootstrap(tmp_path)
    worker_channel_id, worker_secret = _worker_channel(store, task_id, coordinator_channel_id)
    attempt_id = "xa_ext01"

    # A real isolated snapshot workspace: a throwaway copy of the project
    # tree, excluding .git/.cortex (see IsolatedSnapshotWorkspaceProvisioner
    # module docstring) -- this is what a worker's engine actually gets as
    # its cwd.
    workspace_provisioner = IsolatedSnapshotWorkspaceProvisioner(store, str(project), task_id=task_id)
    binding_id = workspace_provisioner.create(attempt_id)
    row = workspace_provisioner.row_for_binding(binding_id)
    assert row["mode"] == "isolated_snapshot", "test needs a real isolated copy, not the shared-root fallback"
    workspace_root = workspace_provisioner.root_for(binding_id)
    assert workspace_root != str(project)
    assert not (Path(workspace_root) / ".cortex").exists()
    assert not (Path(workspace_root) / "cortex.sqlite3").exists()

    # Drive the real EngineSessionManagerAdapter.open() -- the actual
    # production seam this bug lives in -- against the standalone fake
    # engine, capturing the exact session/new params (including the
    # mcpServers entry) it computed for this worker's session.
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    dump_path = engine_dir / "session_new_params.json"
    scenario_path = write_scenario(engine_dir, default_scenario(session_new_params_dump_path=str(dump_path)))
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)

    adapter = EngineSessionManagerAdapter(
        lambda task_id: record,
        lambda task_id: str(project),  # unused: workspace_resolver takes precedence below
        environ=environ, call_timeout=10.0,
        workspace_resolver=lambda binding_id_, launch: workspace_provisioner.root_for(binding_id_),
        project_root_factory=lambda task_id: str(project),
    )
    launch = dict(
        task_id=task_id, attempt_id=attempt_id, channel_id=worker_channel_id,
        channel_secret=worker_secret, assignment_id="a_ext01",
    )
    try:
        adapter.open(binding_id, launch, instructions="")
        session_new_params = json.loads(dump_path.read_text())
    finally:
        adapter.terminate_owned(attempt_id)

    assert session_new_params["cwd"] == workspace_root
    mcp_servers = session_new_params["mcpServers"]
    assert len(mcp_servers) == 1
    server_entry = mcp_servers[0]
    server_env = {item["name"]: item["value"] for item in server_entry["env"]}
    # The heart of the fix: the MCP server's own project root must be the
    # real project, never the throwaway workspace above.
    assert server_env["CORTEX_CHANNEL_PROJECT_ROOT"] == str(project)
    assert server_env["CORTEX_CHANNEL_PROJECT_ROOT"] != workspace_root

    # Now launch exactly that computed command/args as a real subprocess --
    # the real src/cortex.py MCP server, not a hand-
    # assembled stand-in -- with its cwd set to the isolated workspace, the
    # same place a real engine would actually run it from.
    assert server_entry["args"][-1] == str(CORTEX_PY)
    wire_env = os.environ | dict(
        CODEX_HOME=str(engine_dir / "home2"), CORTEX_CHANNEL_ID=server_env["CORTEX_CHANNEL_ID"],
        CORTEX_CHANNEL_SECRET=server_env["CORTEX_CHANNEL_SECRET"],
        CORTEX_CHANNEL_PROJECT_ROOT=server_env["CORTEX_CHANNEL_PROJECT_ROOT"],
        _TEST_CWD=workspace_root,
    )
    (Path(wire_env["CODEX_HOME"]) / "sessions").mkdir(parents=True, exist_ok=True)

    # Reads: list_sources/read_source, scoped by the store the
    # CORTEX_CHANNEL_PROJECT_ROOT env var resolves to. If this subprocess
    # were (as the pre-fix bug did) pointed at the workspace copy instead,
    # there would be no store there at all and this would fail closed with
    # task_not_bound.
    replies = _wire_call([sys.executable, "-B"], [str(CORTEX_PY)], wire_env, _init_and("tools/call", dict(
        name="list_sources", arguments=dict(request_key="list-ext01"),
    )))
    listed = json.loads(replies[1]["result"]["content"][0]["text"])
    assert listed["entries"], "expected the bootstrap source to be readable from the real project store"
    source_ref = listed["entries"][0]["source_ref"]

    replies = _wire_call([sys.executable, "-B"], [str(CORTEX_PY)], wire_env, _init_and("tools/call", dict(
        name="read_source", arguments=dict(source_ref=source_ref, request_key="read-ext01"),
    )))
    read = json.loads(replies[1]["result"]["content"][0]["text"])
    assert "design notes" in read.get("chunk", "")

    # Publish: fixture up a report/draft the worker's own principal thread
    # owns (mirrors tests/test_delegation_mcp_wire.py), then publish over
    # the real wire and confirm it lands in the one real project store.
    with store.connection() as db:
        principal_thread_id = db.execute(
            "SELECT principal_thread_id FROM trusted_channels WHERE channel_id=?", (worker_channel_id,),
        ).fetchone()["principal_thread_id"]
        db.execute("BEGIN IMMEDIATE")
        task_dir = store._task_directory(db, task_id)
        path = task_dir / "w.md"
        path.write_text("# summary\n", encoding="utf-8")
        info = path.stat()
        db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", ("r_" + "e" * 12, task_id, "report", "w.md", store._now()))
        db.execute(
            "INSERT INTO drafts(id,task_id,owner_thread_id,kind,template,path,created_at,device,inode,published_report_id,published_digest) "
            "VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            ("d_" + "f" * 12, task_id, principal_thread_id, "report", "general",
             str(path), store._now(), info.st_dev, info.st_ino, "r_" + "e" * 12, "test-digest"),
        )
        db.commit()

    replies = _wire_call([sys.executable, "-B"], [str(CORTEX_PY)], wire_env, _init_and("tools/call", dict(
        name="publish_result", arguments=dict(report_ref="r_" + "e" * 12, summary="done", request_key="pub-ext01"),
    )))
    published = json.loads(replies[1]["result"]["content"][0]["text"])
    assert published["recorded"] is True

    with store.connection() as db:
        event = db.execute(
            "SELECT event_type FROM task_events WHERE task_id=? AND event_type='worker_result'", (task_id,),
        ).fetchone()
    assert event is not None

    # Single-store invariant: exactly one cortex.sqlite3 exists for this
    # project -- the fix must never spin up a second store inside the
    # workspace copy to paper over the missing one.
    stores_found = list(tmp_path.glob("**/cortex.sqlite3"))
    assert stores_found == [project / ".cortex" / "cortex.sqlite3"], stores_found
