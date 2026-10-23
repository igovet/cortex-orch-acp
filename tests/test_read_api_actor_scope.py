"""Actor identity precedes request shape; real channels, disposable stores, no engines."""
from contextlib import closing
from copy import deepcopy
import json
import sqlite3
from types import SimpleNamespace

import pytest

from cortex_runtime.read_api_evidence import fingerprint, request_hash
from cortex_runtime.server import Server
from cortex_runtime.trusted_channel import issue_channel, resolve_channel
from runtime_scenarios import provision_fixture
from test_read_api_live_assert import cli, matrix, snapshot, insert
from test_tool_read_api import call, data


@pytest.fixture
def actors(tmp_path, matrix):
    fixture, context = matrix
    store, binding = context["store"], fixture[2]
    cid, secret = issue_channel(store, binding["task_id"], "coordinator",
                                binding["owner_epoch"], attempt_id="co_" + "a" * 12)
    coordinator = resolve_channel(store, cid, secret)
    coord = Server(channel_credentials=(cid, secret, str(tmp_path)))
    reviewer, ref = provision_fixture(store, coordinator, key="review", role="code-reviewer")
    with store.connection() as db:
        db.execute("UPDATE delegation_provisioning SET engine_session_ref=? WHERE attempt_id=?",
                   ("es_" + "b" * 12, reviewer.attempt_id))
        db.execute("UPDATE runtime_nodes SET stage='review' WHERE assignment_ref=?", (ref,))
        db.commit()
    cid, secret = issue_channel(store, binding["task_id"], "worker", binding["owner_epoch"],
                                parent_channel_id=coordinator.channel_id,
                                assignment_id=reviewer.assignment_id, attempt_id=reviewer.attempt_id)
    review = Server(channel_credentials=(cid, secret, str(tmp_path)))
    # Both original failure shapes, using genuinely different role catalogues.
    catalogue = data(call(coord, "tools_discover", dict(limit=50), ident="coord-catalogue"))
    worker_catalogue = data(call(context["server"], "tools_discover", dict(limit=50),
                                 ident="worker-catalogue-repeat"))
    assert catalogue != worker_catalogue
    call(review, "read_report", context["report_args"], ident="review-report")
    call(review, "tools_discover", dict(limit=50), ident="review-catalogue")
    sibling, ref = provision_fixture(store, coordinator, key="sibling", role="backend-dev")
    with store.connection() as db:
        db.execute("UPDATE delegation_provisioning SET engine_session_ref=? WHERE attempt_id=?",
                   ("es_" + "c" * 12, sibling.attempt_id))
        db.commit()
    cid, secret = issue_channel(store, binding["task_id"], "worker", binding["owner_epoch"],
                                parent_channel_id=coordinator.channel_id,
                                assignment_id=sibling.assignment_id, attempt_id=sibling.attempt_id)
    other = Server(channel_credentials=(cid, secret, str(tmp_path)))
    call(other, "tools_discover", dict(name="read_report"), ident="sibling-schema")
    call(other, "tools_discover", dict(limit=50), ident="sibling-catalogue")
    return fixture, context, coord, review


@pytest.mark.parametrize("shape", ["catalogue", "report"])
def test_original_mixed_role_failure_shapes(tmp_path, actors, shape):
    fixture, _, _, _ = actors
    rows, _ = snapshot(tmp_path, fixture)
    operation = "tools_discover" if shape == "catalogue" else "read_report"
    args = dict(limit=50) if shape == "catalogue" else dict(report_id=fixture[6], limit=4000)
    matches = [r for r in rows if r.get("operation") == operation and r.get("arguments") == fingerprint(args)]
    assert any(r["attempt_id"] == fixture[3] for r in matches)
    assert any(r["attempt_id"] != fixture[3] for r in matches)
    assert all(r["bound"] for r in matches)
    code, result = cli(tmp_path, fixture)
    assert code == 0 and result["failures"] == [], result
    scoped = [r for r in result["occurrences"] if r.get("arguments") == fingerprint(args)]
    assert scoped and all(r["other_actor_count"] >= 1 for r in scoped)


@pytest.mark.parametrize("change", [
    "missing_catalogue", "missing_report", "forged_selected", "unknown_actor",
    "other_owner", "other_channel", "reused_observation", "reused_request",
    "wrong_role_catalogue", "ambiguous_assignment", "ambiguous_session", "ambiguous_role",
    "ambiguous_assignment_id", "repeat_wrong", "repeat_error", "repeat_truncated", "repeat_withheld",
])
def test_mixed_role_cli_does_not_hide_missing_or_misattributed_evidence(tmp_path, actors, change):
    fixture, _, _, _ = actors
    with closing(sqlite3.connect(tmp_path / ".cortex/cortex.sqlite3")) as db, db:
        db.row_factory = sqlite3.Row
        # Corrupt only owned throwaway falsification fixtures.
        db.execute("CREATE TABLE corrupt AS SELECT * FROM observability_events")
        db.execute("DROP TABLE observability_events")
        db.execute("ALTER TABLE corrupt RENAME TO observability_events")
        records = [dict(r) for r in db.execute("SELECT * FROM observability_events WHERE category='read_api_call'")]
        catalogue = [r for r in records if json.loads(r["detail_json"]).get("arguments") == fingerprint(dict(limit=50))]
        worker = next(r for r in catalogue if r["attempt_id"] == fixture[3])
        other = next(r for r in catalogue if r["attempt_id"] != fixture[3])
        row = deepcopy(other)
        detail = json.loads(row["detail_json"])
        if change.startswith("missing_"):
            operation = "tools_discover" if change == "missing_catalogue" else "read_report"
            arguments = dict(limit=50) if change == "missing_catalogue" else dict(report_id=fixture[6], limit=4000)
            for r in records:
                d = json.loads(r["detail_json"])
                if r["attempt_id"] == fixture[3] and d.get("operation") == operation and d.get("arguments") == fingerprint(arguments):
                    db.execute("DELETE FROM observability_events WHERE id=?", (r["id"],))
        elif change == "forged_selected":
            row["attempt_id"] = fixture[3]
        elif change == "unknown_actor":
            row["attempt_id"] = "co_" + "0" * 12
        elif change == "other_owner":
            detail["owner_epoch"] += 1
        elif change == "other_channel":
            detail["channel_hash"] = json.loads(worker["detail_json"])["channel_hash"]
        elif change == "reused_observation":
            row["id"] = worker["id"]
            insert(db, dict(row, detail_json=json.dumps(detail)))
        elif change == "reused_request":
            row = deepcopy(worker)
            detail = json.loads(row["detail_json"])
            row["id"] = "obs_" + "f" * 12
            insert(db, row)
        elif change == "wrong_role_catalogue":
            row = deepcopy(worker)
            selected = json.loads(row["detail_json"])
            selected["response"] = detail["response"]
            detail = selected
        elif change == "ambiguous_assignment":
            db.execute("CREATE TABLE corrupt_nodes AS SELECT * FROM runtime_nodes")
            db.execute("DROP TABLE runtime_nodes")
            db.execute("ALTER TABLE corrupt_nodes RENAME TO runtime_nodes")
            ref = db.execute("SELECT idempotency_key FROM delegation_provisioning WHERE attempt_id=?", (fixture[3],)).fetchone()[0]
            db.execute("INSERT INTO runtime_nodes SELECT * FROM runtime_nodes WHERE assignment_ref=?", (ref,))
        elif change == "ambiguous_session":
            session = db.execute("SELECT engine_session_ref FROM delegation_provisioning WHERE attempt_id=?", (fixture[3],)).fetchone()[0]
            db.execute("UPDATE delegation_provisioning SET engine_session_ref=?", (session,))
        elif change == "ambiguous_role":
            db.execute("UPDATE runtime_nodes SET role='worker-architect' WHERE stage='execution'")
        elif change == "ambiguous_assignment_id":
            assignment = db.execute("SELECT assignment_id FROM delegation_provisioning WHERE attempt_id=?", (fixture[3],)).fetchone()[0]
            db.execute("UPDATE delegation_provisioning SET assignment_id=?", (assignment,))
        elif change.startswith("repeat_"):
            named = [r for r in records if r["attempt_id"] == fixture[3] and
                     json.loads(r["detail_json"]).get("arguments") == fingerprint(dict(name="read_report"))]
            assert len(named) == 2
            row = named[1]
            detail = json.loads(row["detail_json"])
            if change == "repeat_wrong":
                detail["response"]["sha256"] = "0" * 64
            elif change == "repeat_error":
                detail["is_error"] = True
            elif change == "repeat_truncated":
                detail["response"]["truncated"] = True
            else:
                detail["available"] = False
        if change in ("forged_selected", "unknown_actor", "other_owner", "other_channel", "wrong_role_catalogue") or change.startswith("repeat_"):
            db.execute("UPDATE observability_events SET attempt_id=?,detail_json=? WHERE id=?",
                       (row["attempt_id"], json.dumps(detail), row["id"]))
    code, result = cli(tmp_path, fixture)
    assert code == 1 and not result["passed"] and result["failures"], result
    if change.startswith("missing_"):
        assert "missing_" + operation in result["failures"]
    elif change == "wrong_role_catalogue":
        assert "expected_response_digest" in result["failures"]


@pytest.mark.parametrize("change", ["binding_task", "binding_session", "binding_owner", "digest",
                                    "offset", "total", "empty", "incomplete", "cursor", "content"])
def test_every_snapshot_page_is_bound_and_complete(tmp_path, matrix, monkeypatch, change):
    fixture, _ = matrix
    live, _, binding, _, _, _, _ = fixture
    args = SimpleNamespace(project=str(tmp_path), session=binding["session_id"], task=binding["task_id"],
                           owner_epoch=binding["owner_epoch"])
    original = live.read_binding_page
    calls = []

    def corrupt(*a, **kw):
        page = deepcopy(original(*a, **kw))
        calls.append(page)
        # Tamper only the second page, so the healthy first page cannot hide it.
        if len(calls) == 2:
            if change.startswith("binding_"):
                key = dict(task="task_id", session="session_id", owner="owner_epoch")[change[8:]]
                page["binding"][key] = "foreign"
            elif change == "digest":
                page["snapshot_sha256"] = "0" * 64
            elif change == "offset":
                page["offset"] += 1
            elif change == "total":
                page["total"] += 1
            elif change == "empty":
                page["rows"] = []
            elif change == "content":
                page["rows"][0]["section"] = "forged"
            elif change == "incomplete":
                page["complete"] = False
            else:
                page["next_cursor"] = page["snapshot_sha256"] + ":1"
        return page

    monkeypatch.setattr(live, "read_binding_page", corrupt)
    from cortex_runtime.contracts import StoreError
    with pytest.raises(StoreError, match="inspection_snapshot_changed"):
        live._rows(args)
    assert len(calls) == 2
