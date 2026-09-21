"""T017 (lead follow-up): claimed vs. observed vs. contradicted evidence for
publish_result's optional ``checks``, over ``delegation.evidence`` and the
``mcp_tools._worker_publish_result`` wiring. Fakes only, no real engine."""
from __future__ import annotations

import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import contracts as delegation_contracts
from cortex_runtime.delegation import evidence as delegation_evidence
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import ChannelContext, issue_channel_tx


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build.", "owner-1")
    return store, result.task_id


def _worker_channel(store, task_id, attempt_id):
    with store.connection() as db:
        owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
        db.execute("BEGIN IMMEDIATE")
        coordinator_channel_id, _ = issue_channel_tx(db, task_id, "coordinator", owner["epoch"])
        channel_id, secret = issue_channel_tx(
            db, task_id, "worker", owner["epoch"],
            parent_channel_id=coordinator_channel_id, attempt_id=attempt_id,
        )
        db.commit()
    with store.connection() as db:
        row = db.execute("SELECT * FROM trusted_channels WHERE channel_id=?", (channel_id,)).fetchone()
    return ChannelContext(
        channel_id=row["channel_id"], task_id=row["task_id"], principal_thread_id=row["principal_thread_id"],
        parent_principal_thread_id=row["parent_principal_thread_id"], role=row["role"], owner_epoch=row["owner_epoch"],
        assignment_id=row["assignment_id"], attempt_id=row["attempt_id"], workspace_binding_id=row["workspace_binding_id"],
        context_generation_id=row["context_generation_id"],
    )


def _report(store, task_id, channel):
    """T019: also inserts a matching ``drafts`` row owned by this channel's
    ``principal_thread_id`` so ``publish_result``'s report_ref-authorship
    binding accepts it."""
    report_id = "r_" + uuid.uuid4().hex[:12]
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        task_dir = store._task_directory(db, task_id)
        path = task_dir / f"{report_id}.md"
        path.write_text("# done\n", encoding="utf-8")
        info = path.stat()
        db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", (report_id, task_id, "report", f"{report_id}.md", store._now()))
        db.execute(
            "INSERT INTO drafts(id,task_id,owner_thread_id,kind,template,path,created_at,device,inode,published_report_id,published_digest) "
            "VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            ("d_" + uuid.uuid4().hex[:12], task_id, channel.principal_thread_id, "report", "general",
             str(path), store._now(), info.st_dev, info.st_ino, report_id, "test-digest"),
        )
        db.commit()
    return report_id


def test_check_with_no_tool_call_ref_stays_claimed(tmp_path):
    store, task_id = _task(tmp_path)
    attempt_id = "xa_" + uuid.uuid4().hex[:12]
    channel = _worker_channel(store, task_id, attempt_id)
    report_id = _report(store, task_id, channel)
    result = mcp_tools.dispatch(store, channel, "publish_result", dict(
        report_ref=report_id, summary="done", request_key="pub1",
        checks=[dict(name="unit tests", outcome="passed")],
    ))
    assert result["recorded"] is True
    with store.connection() as db:
        row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='worker_result'", (task_id,),
        ).fetchone()
    import json

    checks = json.loads(row["payload_json"])["checks"]
    assert checks == [dict(name="unit tests", outcome="passed", tool_call_ref=None, evidence="claimed")]


def test_check_with_matching_completed_observation_is_observed(tmp_path):
    store, task_id = _task(tmp_path)
    attempt_id = "xa_" + uuid.uuid4().hex[:12]
    channel = _worker_channel(store, task_id, attempt_id)
    report_id = _report(store, task_id, channel)
    delegation_evidence.record_tool_result_observed(
        store, task_id, attempt_id, "tc_abc123", "execute", "run pytest", "pending",
    )
    delegation_evidence.record_tool_result_observed(
        store, task_id, attempt_id, "tc_abc123", "execute", "run pytest", "completed",
    )
    result = mcp_tools.dispatch(store, channel, "publish_result", dict(
        report_ref=report_id, summary="done", request_key="pub1",
        checks=[dict(name="unit tests", outcome="passed", tool_call_ref="tc_abc123")],
    ))
    import json

    with store.connection() as db:
        row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='worker_result'", (task_id,),
        ).fetchone()
    checks = json.loads(row["payload_json"])["checks"]
    assert checks[0]["evidence"] == "observed"


def test_check_with_failed_observation_is_contradicted(tmp_path):
    store, task_id = _task(tmp_path)
    attempt_id = "xa_" + uuid.uuid4().hex[:12]
    channel = _worker_channel(store, task_id, attempt_id)
    report_id = _report(store, task_id, channel)
    delegation_evidence.record_tool_result_observed(
        store, task_id, attempt_id, "tc_xyz789", "execute", "run pytest", "failed",
    )
    result = mcp_tools.dispatch(store, channel, "publish_result", dict(
        report_ref=report_id, summary="done", request_key="pub1",
        checks=[dict(name="unit tests", outcome="passed", tool_call_ref="tc_xyz789")],
    ))
    import json

    with store.connection() as db:
        row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='worker_result'", (task_id,),
        ).fetchone()
    checks = json.loads(row["payload_json"])["checks"]
    assert checks[0]["evidence"] == "contradicted"


def test_cross_attempt_tool_call_ref_is_never_trusted(tmp_path):
    store, task_id = _task(tmp_path)
    other_attempt_id = "xa_" + uuid.uuid4().hex[:12]
    delegation_evidence.record_tool_result_observed(
        store, task_id, other_attempt_id, "tc_shared", "execute", "run pytest", "completed",
    )
    attempt_id = "xa_" + uuid.uuid4().hex[:12]
    channel = _worker_channel(store, task_id, attempt_id)
    report_id = _report(store, task_id, channel)
    result = mcp_tools.dispatch(store, channel, "publish_result", dict(
        report_ref=report_id, summary="done", request_key="pub1",
        checks=[dict(name="unit tests", outcome="passed", tool_call_ref="tc_shared")],
    ))
    import json

    with store.connection() as db:
        row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='worker_result'", (task_id,),
        ).fetchone()
    checks = json.loads(row["payload_json"])["checks"]
    # A tool_call_ref that only resolves under a DIFFERENT attempt is never
    # trusted -- stays claimed, not silently promoted to observed.
    assert checks[0]["evidence"] == "claimed"


def test_checks_schema_rejects_unadvertised_field_and_bad_outcome(tmp_path):
    store, task_id = _task(tmp_path)
    try:
        delegation_contracts.validate("worker", "publish_result", dict(
            report_ref="r_" + "0" * 12, summary="ok", request_key="k1",
            checks=[dict(name="x", outcome="maybe")],
        ))
        raise AssertionError("expected StoreError for an invalid outcome value")
    except StoreError:
        pass

    # An attempted check whose result cannot be established has a distinct
    # durable outcome.  It must not be misreported as ``not_run`` (which means
    # the check was never attempted).
    delegation_contracts.validate("worker", "publish_result", dict(
        report_ref="r_" + "0" * 12, summary="ok", request_key="k-unverified",
        checks=[dict(name="rendered page", outcome="unverified")],
    ))
    try:
        delegation_contracts.validate("worker", "publish_result", dict(
            report_ref="r_" + "0" * 12, summary="ok", request_key="k1",
            checks=[dict(name="x", outcome="passed", raw_output="leaked")],
        ))
        raise AssertionError("expected StoreError for an unadvertised checks[] field")
    except StoreError:
        pass
    # 21 items exceeds the bounded maxItems=20.
    try:
        delegation_contracts.validate("worker", "publish_result", dict(
            report_ref="r_" + "0" * 12, summary="ok", request_key="k1",
            checks=[dict(name="x", outcome="passed") for _ in range(21)],
        ))
        raise AssertionError("expected StoreError for too many checks")
    except StoreError:
        pass
