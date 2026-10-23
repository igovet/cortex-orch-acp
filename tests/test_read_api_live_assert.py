"""Real assertion CLI over complete disposable snapshots, never a paid engine."""
from contextlib import closing
from copy import deepcopy
import json
import sqlite3
import subprocess
import sys

import pytest

from cortex_runtime.read_api_evidence import fingerprint, request_hash
from cortex_runtime.storage.inspection import read_binding_page
from test_read_api_evidence import ROOT, live_fixture
from test_tool_read_api import call, data


def cli(tmp_path, fixture, **overrides):
    _, _, binding, attempt, command, _, report = fixture
    fields = dict(project=str(tmp_path), session=binding["session_id"], task=binding["task_id"],
                  owner_epoch=binding["owner_epoch"], attempt=attempt, command_ref=command,
                  report_id=report, public_report=str(tmp_path / "public-read-api-report.md"))
    fields.update(overrides)
    argv = [sys.executable, "-B", str(ROOT / "scripts/cortex_read_api_live_assert.py")]
    for key, value in fields.items():
        argv.extend(["--" + key.replace("_", "-"), str(value)])
    run = subprocess.run(argv, capture_output=True, text=True, timeout=30)
    assert not run.stderr
    return run.returncode, json.loads(run.stdout)


def snapshot(tmp_path, fixture):
    binding = fixture[2]
    page = read_binding_page(str(tmp_path), binding["session_id"], binding["task_id"],
                             binding["owner_epoch"], limit=200)
    rows, digest, pages = list(page["rows"]), page["snapshot_sha256"], 1
    while page["next_cursor"]:
        page = read_binding_page(str(tmp_path), binding["session_id"], binding["task_id"],
                                 binding["owner_epoch"], cursor=page["next_cursor"], limit=200)
        assert page["snapshot_sha256"] == digest
        rows.extend(page["rows"])
        pages += 1
    assert page["complete"] and len(rows) == page["total"] and pages >= 2
    return rows, digest


def insert(db, row):
    keys = list(row)
    db.execute("INSERT INTO observability_events (" + ",".join(keys) + ") VALUES (" +
               ",".join("?" for _ in keys) + ")", [row[k] for k in keys])


@pytest.fixture
def matrix(tmp_path, unjailed_gateway_command):
    context = {}
    fixture = live_fixture(tmp_path, repeat_named=True, context=context)
    # Force the actual CLI's 200-row cursor path, not a helper-only mock.
    with closing(sqlite3.connect(tmp_path / ".cortex/cortex.sqlite3")) as db, db:
        db.row_factory = sqlite3.Row
        template = dict(db.execute("SELECT * FROM observability_events LIMIT 1").fetchone())
        for index in range(205):
            insert(db, dict(template, id="obs_" + format(index, "012x"),
                            category="validation", detail_json="{}"))
    snapshot(tmp_path, fixture)
    return fixture, context


def test_two_correct_distinct_calls_pass_complete_cli_snapshot(tmp_path, matrix):
    fixture, _ = matrix
    rows, digest = snapshot(tmp_path, fixture)
    expected = [r for r in rows if r["section"] == "read_api_calls" and
                r.get("arguments") == fingerprint(dict(name="read_report"))]
    assert len(expected) == 2
    assert len({r["request_hash"] for r in expected}) == len({r["id"] for r in expected}) == 2
    assert expected[0]["response"] == expected[1]["response"]
    code, result = cli(tmp_path, fixture)
    assert code == 0 and result["passed"] and not result["accepted"] and result["failures"] == []
    assert result["snapshot_sha256"] == digest
    [occurrence] = [r for r in result["occurrences"] if r["arguments"] == fingerprint(dict(name="read_report"))]
    assert occurrence["count"] == 2
    assert {r["id"] for r in occurrence["identities"]} == {r["id"] for r in expected}


@pytest.mark.parametrize("shape", ["native", "successful", "mixed", "rejected", "report", "deferred", "retrieval"])
def test_other_distinct_legitimate_repetitions_are_all_checked(tmp_path, matrix, shape):
    fixture, context = matrix
    _, rows, _, _, _, _, _ = fixture
    native = next(r for r in rows if r["section"] == "action_receipts" and r.get("tool_call_ref") == fixture[4])
    args = dict(large_output_id=native["large_output_id"], limit=1000)
    item = dict(id="public", tool="read_large_output", arguments=args)
    report_args = context["report_args"]
    choices = dict(native=("read_large_output", args), successful=("read_batch", dict(calls=[item])),
                   mixed=("read_batch", dict(calls=[item, dict(id="missing", tool="read_large_output",
                         arguments=dict(large_output_id="lo_" + "0" * 12, limit=100))])),
                   rejected=("read_batch", dict(calls=[item, dict(id="forbidden", tool="command_run", arguments={})])),
                   report=("read_report", report_args),
                   deferred=("read_batch", dict(calls=[dict(id="deferred", tool="read_report", arguments=report_args)])))
    if shape == "retrieval":
        deferred = next(r for r in rows if r["section"] == "read_api_calls" and
                        any(m.get("status") == "deferred" for m in r.get("members", [])))
        choices[shape] = ("read_large_output", dict(large_output_id=deferred["members"][0]["large_output_id"], limit=16000))
    operation, arguments = choices[shape]
    reply = data(call(context["server"], operation, arguments, ident="repeat-" + shape))
    if shape == "deferred":
        call(context["server"], "read_large_output",
             dict(large_output_id=reply["results"][0]["large_output_id"], limit=16000), ident="repeat-deferred-retrieve")
    code, result = cli(tmp_path, fixture)
    assert code == 0 and result["failures"] == [], result
    matches = [r for r in result["occurrences"] if r["operation"] == operation and r["arguments"] == fingerprint(arguments)]
    assert matches and all(r["count"] == 2 for r in matches)


@pytest.mark.parametrize("change", [
    "duplicate_row", "duplicate_identity", "same_request", "wrong", "error", "truncated", "incomplete",
    "withheld", "foreign_attempt", "foreign_owner", "foreign_channel", "foreign_task", "missing",
    "deferred_wrong_retrieval", "deferred_error_retrieval", "deferred_truncated_retrieval",
    "mixed_wrong_member", "rejected_executed",
])
def test_cli_rejects_every_contradictory_or_replayed_occurrence(tmp_path, matrix, change):
    fixture, context = matrix
    with closing(sqlite3.connect(tmp_path / ".cortex/cortex.sqlite3")) as db, db:
        db.row_factory = sqlite3.Row
        records = [dict(r) for r in db.execute("SELECT * FROM observability_events WHERE category='read_api_call'")]
        named = [r for r in records if json.loads(r["detail_json"]).get("arguments") == fingerprint(dict(name="read_report"))]
        first, second = named
        row = second
        detail = json.loads(row["detail_json"])
        # Deliberately corrupt only this owned disposable fixture, not retained
        # live evidence. Healthy storage forbids changing immutable event rows.
        db.execute("CREATE TABLE corrupt_observations AS SELECT * FROM observability_events")
        db.execute("DROP TABLE observability_events")
        db.execute("ALTER TABLE corrupt_observations RENAME TO observability_events")
        if change in ("duplicate_row", "duplicate_identity"):
            duplicate = deepcopy(second)
            if change == "duplicate_identity":
                detail["request_hash"] = request_hash("different-outer")
                duplicate["detail_json"] = json.dumps(detail)
            insert(db, duplicate)
        elif change == "same_request":
            detail["request_hash"] = json.loads(first["detail_json"])["request_hash"]
        elif change == "wrong":
            detail["response"]["sha256"] = "0" * 64
        elif change == "error":
            detail["is_error"] = True
        elif change in ("truncated", "incomplete"):
            detail["response"]["truncated" if change == "truncated" else "complete"] = change == "truncated"
        elif change == "withheld":
            detail["available"] = False
        elif change == "foreign_attempt":
            row["attempt_id"] = "xa_" + "0" * 12
        elif change == "foreign_owner":
            detail["owner_epoch"] += 1
        elif change == "foreign_channel":
            detail["channel_hash"] = "0" * 32
        elif change in ("foreign_task", "missing"):
            for member in named:
                if change == "missing":
                    db.execute("DELETE FROM observability_events WHERE id=?", (member["id"],))
                else:
                    db.execute("UPDATE observability_events SET task_id=? WHERE id=?", ("t_" + "0" * 12, member["id"]))
        elif change.startswith("deferred_"):
            row = next(r for r in records if json.loads(r["detail_json"]).get("request_hash") == request_hash("retrieve"))
            detail = json.loads(row["detail_json"])
            duplicate = deepcopy(row)
            duplicate["id"] = "obs_" + "f" * 12
            detail["request_hash"] = request_hash("second-retrieval")
            if change == "deferred_wrong_retrieval":
                detail["response"]["content_sha256"] = "0" * 64
            elif change == "deferred_error_retrieval":
                detail["is_error"] = True
            else:
                detail["response"]["truncated"] = True
            duplicate["detail_json"] = json.dumps(detail)
            insert(db, duplicate)
        elif change == "mixed_wrong_member":
            row = next(r for r in records if json.loads(r["detail_json"]).get("request_hash") == request_hash("mixed"))
            detail = json.loads(row["detail_json"])
            detail["members"][0]["status"] = "not_run"
        elif change == "rejected_executed":
            execution = next(dict(r) for r in db.execute("SELECT * FROM observability_events WHERE category='tool_read_api' AND outcome='executed'"))
            rejection = next(dict(r) for r in db.execute("SELECT * FROM observability_events WHERE category='tool_read_api' AND outcome='rejected'")
                             if json.loads(r["detail_json"]).get("request_hash") == request_hash("rejected"))
            extra = dict(execution, id="obs_" + "e" * 12)
            extra["detail_json"] = rejection["detail_json"]
            insert(db, extra)
        if change not in ("duplicate_row", "duplicate_identity", "missing", "foreign_task", "rejected_executed") and not change.startswith("deferred_"):
            db.execute("UPDATE observability_events SET attempt_id=?,detail_json=? WHERE id=?",
                       (row["attempt_id"], json.dumps(detail), row["id"]))
    _, digest = snapshot(tmp_path, fixture)
    code, result = cli(tmp_path, fixture)
    assert code == 1 and not result["passed"] and result["failures"], result
    assert result["snapshot_sha256"] == digest


@pytest.mark.parametrize("field,value", [("task", "t_" + "0" * 12), ("session", "acps_" + "0" * 12),
                                        ("owner_epoch", 2), ("attempt", "xa_" + "0" * 12)])
def test_cli_foreign_binding_cannot_borrow_good_occurrences(tmp_path, matrix, field, value):
    fixture, _ = matrix
    code, result = cli(tmp_path, fixture, **{field: value})
    assert code == (1 if field == "attempt" else 2) and not result["passed"]
