"""T016: real cortex.py MCP subprocess over a trusted channel -- wire-level
exposure of the delegation operations (delegate/list_events/clarify/cancel/
decide for a coordinator channel; ask_question/publish_result for a worker
channel), role-filtered in both tools/list and tools/call.

Spawns the real ``plugins/cortex/scripts/cortex.py`` entry point as a
subprocess (never a real engine) against a tmp project store, exactly the
process boundary described in docs/acp/orchestration.md: this test's
subprocess plays the role of "an engine's own Cortex MCP child process".
"""
from __future__ import annotations

import json
import os
from pathlib import Path
import subprocess
import sys
import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx

ROOT = Path(__file__).resolve().parents[1]
CORTEX_PY = ROOT / "plugins/cortex/scripts/cortex.py"


def _storage_only_home(home: Path) -> Path:
    """Keep this wire test independent of the optional gateway child (see tests/test_stdio.py)."""
    cortex = home / "cortex"
    cortex.mkdir(parents=True, exist_ok=True)
    cortex.chmod(0o700)
    config = cortex / "config.toml"
    config.write_text("schema_version = 1\n[gateway]\nenabled = false\n")
    config.chmod(0o600)
    return home


def _bootstrap(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return project, store, result.task_id, result.channel_id, result.secret


def _worker_channel(store, task_id, coordinator_channel_id, attempt_id="xa_wire1"):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        epoch = current_owner_epoch(db, task_id)
        channel_id, secret = issue_channel_tx(
            db, task_id, "worker", epoch, parent_channel_id=coordinator_channel_id,
            assignment_id="a_wire1", attempt_id=attempt_id,
        )
        db.commit()
    return channel_id, secret, attempt_id


def _call(project, channel_id, secret, messages, tmp_path):
    home = _storage_only_home(tmp_path / "home")
    (home / "sessions").mkdir(parents=True, exist_ok=True)
    env = os.environ | dict(
        CODEX_HOME=str(home),
        CORTEX_CHANNEL_ID=channel_id, CORTEX_CHANNEL_SECRET=secret,
        CORTEX_CHANNEL_PROJECT_ROOT=str(project),
    )
    result = subprocess.run(
        [sys.executable, "-B", str(CORTEX_PY)],
        input="\n".join(json.dumps(m) for m in messages) + "\n",
        text=True, capture_output=True, env=env, timeout=30,
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


def test_coordinator_tools_list_includes_role_filtered_delegation_tools(tmp_path):
    project, store, task_id, channel_id, secret = _bootstrap(tmp_path)
    replies = _call(project, channel_id, secret, _init_and("tools/list", {}), tmp_path)
    names = {t["name"] for t in replies[1]["result"]["tools"]}
    assert {"create_task", "write_report", "delegate", "list_events", "clarify", "cancel", "decide"} <= names
    # T018: list_sources/read_source are shared read-only source-history
    # operations, available on both coordinator and worker channels.
    assert {"list_sources", "read_source"} <= names
    assert "ask_question" not in names and "publish_result" not in names
    assert len(names) == 14


def test_worker_tools_list_is_worker_only(tmp_path):
    project, store, task_id, coordinator_channel_id, _ = _bootstrap(tmp_path)
    worker_channel_id, worker_secret, attempt_id = _worker_channel(store, task_id, coordinator_channel_id)
    replies = _call(project, worker_channel_id, worker_secret, _init_and("tools/list", {}), tmp_path)
    names = {t["name"] for t in replies[1]["result"]["tools"]}
    assert {"ask_question", "publish_result"} <= names
    assert "delegate" not in names and "decide" not in names


def test_unauthenticated_process_still_sees_exactly_the_frozen_seven(tmp_path):
    project = tmp_path / "project2"; project.mkdir()
    home = _storage_only_home(tmp_path / "home2"); (home / "sessions").mkdir(parents=True)
    result = subprocess.run(
        [sys.executable, "-B", str(CORTEX_PY)],
        input="\n".join(json.dumps(m) for m in _init_and("tools/list", {})) + "\n",
        text=True, capture_output=True, env=os.environ | dict(CODEX_HOME=str(home)), timeout=30,
    )
    assert result.returncode == 0 and result.stderr == ""
    replies = [json.loads(line) for line in result.stdout.splitlines()]
    assert len(replies[1]["result"]["tools"]) == 7


def test_delegate_over_the_wire_writes_a_delegation_requested_event(tmp_path):
    project, store, task_id, channel_id, secret = _bootstrap(tmp_path)
    call = dict(name="delegate", arguments=dict(
        title="Assignment W", brief="do W", role="backend-dev", acceptance="W done", request_key="reqW",
    ))
    replies = _call(project, channel_id, secret, _init_and("tools/call", call), tmp_path)
    data = json.loads(replies[1]["result"]["content"][0]["text"])
    assert data["status"] == "provisioning" and data["assignment_ref"]
    with store.connection() as db:
        row = db.execute(
            "SELECT * FROM task_events WHERE task_id=? AND event_type='delegation_requested'", (task_id,),
        ).fetchone()
    assert row is not None and row["assignment_ref"] == data["assignment_ref"]

    # Retrying the identical call is idempotent (same assignment_ref).
    replies2 = _call(project, channel_id, secret, _init_and("tools/call", call), tmp_path)
    data2 = json.loads(replies2[1]["result"]["content"][0]["text"])
    assert data2["assignment_ref"] == data["assignment_ref"]
    with store.connection() as db:
        count = db.execute(
            "SELECT COUNT(*) AS n FROM task_events WHERE task_id=? AND event_type='delegation_requested'", (task_id,),
        ).fetchone()["n"]
    assert count == 1


def test_worker_publish_then_coordinator_decide_accept_over_the_wire(tmp_path):
    project, store, task_id, coordinator_channel_id, coordinator_secret = _bootstrap(tmp_path)
    worker_channel_id, worker_secret, attempt_id = _worker_channel(store, task_id, coordinator_channel_id)

    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", ("r_" + "d" * 12, task_id, "report", "reports/w.md", store._now()))
        db.commit()

    publish_call = dict(name="publish_result", arguments=dict(report_ref="r_" + "d" * 12, summary="done", request_key="pub1"))
    replies = _call(project, worker_channel_id, worker_secret, _init_and("tools/call", publish_call), tmp_path)
    published = json.loads(replies[1]["result"]["content"][0]["text"])
    assert published["recorded"] is True

    ask_call = dict(name="ask_question", arguments=dict(question="Ready for review?", request_key="ask1"))
    replies = _call(project, worker_channel_id, worker_secret, _init_and("tools/call", ask_call), tmp_path)
    asked = json.loads(replies[1]["result"]["content"][0]["text"])
    assert asked["recorded"] is True

    decide_call = dict(name="decide", arguments=dict(
        assignment_ref=attempt_id, decision="accept", rationale="Worker result looks complete.", request_key="dec1",
    ))
    replies = _call(project, coordinator_channel_id, coordinator_secret, _init_and("tools/call", decide_call), tmp_path)
    decided = json.loads(replies[1]["result"]["content"][0]["text"])
    assert decided["recorded"] is True and decided["decision"] == "accept"

    with store.connection() as db:
        events = db.execute(
            "SELECT event_type FROM task_events WHERE task_id=? ORDER BY id", (task_id,),
        ).fetchall()
    types = [row["event_type"] for row in events]
    assert types == ["worker_result", "worker_question", "coordinator_decision"]


def test_worker_channel_cannot_call_a_coordinator_operation_over_the_wire(tmp_path):
    project, store, task_id, coordinator_channel_id, _ = _bootstrap(tmp_path)
    worker_channel_id, worker_secret, attempt_id = _worker_channel(store, task_id, coordinator_channel_id)
    call = dict(name="delegate", arguments=dict(title="X", brief="Y", role="backend-dev", acceptance="Z", request_key="r"))
    replies = _call(project, worker_channel_id, worker_secret, _init_and("tools/call", call), tmp_path)
    reply = replies[1]
    # 'delegate' is not among this worker channel's own advertised tools
    # (see tools/list), so it is refused as an unknown tool at the
    # protocol level -- never dispatched to the coordinator-only handler.
    assert "error" in reply and reply["error"]["code"] == -32602
