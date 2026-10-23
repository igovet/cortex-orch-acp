"""No-paid transport, immutable-store and adversarial expectation regressions."""
from copy import deepcopy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys

import pytest

from cortex_runtime.acp import sessions
from cortex_runtime.read_api_evidence import evaluate_manifest, fingerprint, project, request_hash
from cortex_runtime.storage.inspection import read_binding_page
from test_tool_read_api import setup, call, data

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("read_fixture", ROOT / "scripts/cortex_read_api_fixture.py")
fixture = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixture)


def test_public_real_stdio_fixture_and_subprocess_assertion(tmp_path):
    result = fixture.run_fixture(tmp_path)
    assert result["passed"] and result["expected_calls"] >= 6
    assert result["paid_dispatch"] is False and result["accepted"] is False
    assert result["public_bytes"] == len(fixture.PUBLIC.encode())


def test_fixture_cli_does_not_claim_paid_dispatch_or_acceptance():
    run = subprocess.run([sys.executable, "-B", str(ROOT / "scripts/cortex_read_api_fixture.py")],
                         capture_output=True, text=True, timeout=60)
    assert run.returncode == 0, run.stdout
    assert json.loads(run.stdout)["phase"] == "fixture_asserted"


def bound(tmp_path, role=None):
    store, boot, channel, server, ref = setup(tmp_path, role)
    if role:
        with store.connection() as db:
            db.execute("UPDATE delegation_provisioning SET engine_session_ref=? WHERE attempt_id=?",
                       ("es_" + "f" * 12, channel.attempt_id))
            db.commit()
    sid = "acps_" + "f" * 12
    sessions.create_session(store, sid, str(tmp_path), {"name": "fixture"}, {}, [])
    sessions.bind_task(store, sid, boot.task_id)
    return store, boot, channel, server, sid


def inspect(tmp_path, boot, channel, sid):
    page = read_binding_page(str(tmp_path), sid, boot.task_id, channel.owner_epoch, limit=200)
    rows = list(page["rows"])
    while page["next_cursor"]:
        page = read_binding_page(str(tmp_path), sid, boot.task_id, channel.owner_epoch,
                                 cursor=page["next_cursor"], limit=200)
        rows.extend(page["rows"])
    return rows, page["binding"]


def evidence(tmp_path, role=None):
    store, boot, channel, server, sid = bound(tmp_path, role)
    args = dict(name="read_report")
    reply = call(server, "tools_discover", args, ident="unique")
    rows, binding = inspect(tmp_path, boot, channel, sid)
    [row] = [r for r in rows if r["section"] == "read_api_calls"]
    # This test's expected boundary is independently serialized response data;
    # the semantic schema equality is already established in test_tool_read_api.
    expected = dict(version=1, available=True, operation="tools_discover", owner_epoch=channel.owner_epoch,
                    channel_hash=row["channel_hash"], request_hash=request_hash("unique"),
                    arguments=fingerprint(args), response=dict(fingerprint(data(reply)), paged=False), is_error=False)
    return store, boot, channel, server, sid, rows, binding, dict(binding=binding, calls=[expected])


@pytest.mark.parametrize("role", [None, "backend-dev"])
def test_bound_runtime_fingerprints_have_exact_native_identity(tmp_path, role):
    store, boot, channel, server, sid, rows, binding, manifest = evidence(tmp_path, role)
    assert evaluate_manifest(rows, binding, manifest) == []
    with store.connection() as db:
        stored = [dict(r) for r in db.execute("SELECT detail_json FROM observability_events WHERE category='read_api_call'")]
    assert "SOURCE_BODY_SENTINEL" not in json.dumps(stored)
    assert "read_report" not in json.dumps(stored)  # arguments are hashed, not exported
    assert request_hash(1) != request_hash("1")


@pytest.mark.parametrize("change", [
    "missing", "duplicate", "forged", "wrong_owner", "unbound", "unavailable",
    "args_digest", "args_bytes", "result_digest", "result_bytes", "truncated",
    "wrong_task", "wrong_session", "stale_expected", "duplicate_expected",
    "wrong_attempt", "missing_session", "duplicate_node",
])
def test_missing_forged_replayed_mismatched_evidence_blocks(tmp_path, change):
    store, boot, channel, server, sid, rows, binding, manifest = evidence(tmp_path, "backend-dev")
    index = next(i for i, r in enumerate(rows) if r["section"] == "read_api_calls")
    row = rows[index]
    if change == "missing":
        rows.pop(index)
    elif change == "duplicate":
        rows.append(deepcopy(row))
    elif change == "forged":
        row["request_hash"] = "0" * 64
    elif change == "wrong_owner":
        row["owner_epoch"] += 1
    elif change == "unbound":
        row["bound"] = False
    elif change == "unavailable":
        row["available"] = False
    elif change == "args_digest":
        row["arguments"]["sha256"] = "0" * 64
    elif change == "args_bytes":
        row["arguments"]["bytes"] += 1
    elif change == "result_digest":
        row["response"]["sha256"] = "0" * 64
    elif change == "result_bytes":
        row["response"]["bytes"] += 1
    elif change == "truncated":
        row["response"]["truncated"] = True
    elif change in ("wrong_task", "wrong_session"):
        manifest["binding"] = dict(binding, **{("task_id" if change == "wrong_task" else "session_id"): "wrong"})
    elif change == "stale_expected":
        manifest["calls"][0]["owner_epoch"] += 1
    elif change == "duplicate_expected":
        manifest["calls"].append(deepcopy(manifest["calls"][0]))
    elif change == "wrong_attempt":
        row["attempt_id"] = "xa_" + "0" * 12
    elif change == "missing_session":
        next(r for r in rows if r["section"] == "attempts")["engine_session_ref"] = None
    elif change == "duplicate_node":
        rows.append(deepcopy(next(r for r in rows if r["section"] == "nodes")))
    assert evaluate_manifest(rows, binding, manifest)


def test_reused_request_id_is_not_unique_evidence(tmp_path):
    store, boot, channel, server, sid, rows, binding, manifest = evidence(tmp_path)
    call(server, "tools_discover", dict(name="read_report"), ident="unique")
    rows, binding = inspect(tmp_path, boot, channel, sid)
    assert evaluate_manifest(rows, binding, manifest)


def test_absent_request_and_unsupported_fingerprints_are_explicit(tmp_path):
    store, boot, channel, server, sid = bound(tmp_path)
    call(server, "tools_discover", dict(name="read_report"), ident=None)
    rows, binding = inspect(tmp_path, boot, channel, sid)
    [row] = [r for r in rows if r["section"] == "read_api_calls"]
    assert row["available"] is False and row["bound"] is False
    assert project(dict(available=True, arbitrary="PRIVATE")) == dict(available=False)


def test_deferred_worker_retrieval_never_establishes_report_delivery(tmp_path):
    store, boot, channel, server, sid = bound(tmp_path, "backend-dev")
    # A genuinely large native pipeline page; no lowered fixture-only limits.
    draft = data(call(server, "create_draft", dict(template="general", request_key="draft")))
    text = draft["required_first_line"] + "\n\n" + fixture.REPORT
    assert not call(server, "draft_write", dict(draft_id=draft["draft_id"], content=text, request_key="write"))["isError"]
    published = data(call(server, "write_report", dict(draft_id=draft["draft_id"], title="Public fixture",
                          summary="Public fixture.", author="fixture", request_key="publish")))
    reply = data(call(server, "read_batch", dict(calls=[dict(id="large", tool="read_report",
                  arguments=dict(report_id=published["report_id"], limit=4000))]), ident="defer"))
    [member] = reply["results"]
    assert member["status"] == "deferred"
    retrieval = data(call(server, "read_large_output", dict(large_output_id=member["large_output_id"], limit=16000)))
    assert json.loads(retrieval["chunk"])["markdown"] == text
    with store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM report_read_observations").fetchone()[0] == 0


def live_fixture(tmp_path, *, repeat_named=False, context=None):
    sys.path.insert(0, str(ROOT / "scripts"))
    import cortex_read_api_live_assert as live
    store, boot, channel, server, sid = bound(tmp_path, "backend-dev")
    for args in (dict(name="read_report"), dict(limit=50), dict(name="read_report", limit=1)):
        call(server, "tools_discover", args, ident="discover-" + str(len(args)) + str("limit" in args))
    if repeat_named:
        call(server, "tools_discover", dict(name="read_report"), ident="discover-repeat")
    command = data(call(server, "command_run", dict(argv=[sys.executable, "-B", "-c",
                        "import sys;sys.stdout.write(" + repr(fixture.PUBLIC) + ")"],
                        request_key="public-output"), ident="command"))
    assert command["exit_code"] == 0
    args = dict(large_output_id=command["large_output_id"], limit=1000)
    assert not call(server, "read_large_output", args, ident="single")["isError"]
    item = dict(id="public", tool="read_large_output", arguments=args)
    call(server, "read_batch", dict(calls=[item]), ident="good")
    call(server, "read_batch", dict(calls=[item, dict(id="missing", tool="read_large_output",
         arguments=dict(large_output_id="lo_" + "0" * 12, limit=100))]), ident="mixed")
    call(server, "read_batch", dict(calls=[item, dict(id="forbidden", tool="command_run", arguments={})]), ident="rejected")
    draft = data(call(server, "create_draft", dict(template="general", request_key="live-draft")))
    text = draft["required_first_line"] + "\n\n" + fixture.REPORT
    call(server, "draft_write", dict(draft_id=draft["draft_id"], content=text, request_key="live-write"))
    pub = data(call(server, "write_report", dict(draft_id=draft["draft_id"], title="Public fixture",
               summary="Public fixture.", author="fixture", request_key="live-publish")))
    (tmp_path / "public-read-api-report.md").write_text(text)
    report_args = dict(report_id=pub["report_id"], limit=4000)
    call(server, "read_report", report_args, ident="report")
    deferred = data(call(server, "read_batch", dict(calls=[
        dict(id="deferred", tool="read_report", arguments=report_args)]), ident="deferred"))
    call(server, "read_large_output", dict(large_output_id=deferred["results"][0]["large_output_id"],
         limit=16000), ident="retrieve")
    rows, binding = inspect(tmp_path, boot, channel, sid)
    if context is not None:
        context.update(store=store, server=server, report_args=report_args)
    return live, rows, binding, channel.attempt_id, command["tool_call_ref"], text.encode(), pub["report_id"]


@pytest.mark.usefixtures("unjailed_gateway_command")
def test_live_assertions_use_real_current_task_command_and_read_evidence(tmp_path):
    live, rows, binding, attempt, command, content, report = live_fixture(tmp_path)
    assert live.evaluate(rows, binding, attempt_id=attempt, command_ref=command,
                         public_report=content, report_id=report) == []
    run = subprocess.run([sys.executable, "-B", str(ROOT / "scripts/cortex_read_api_live_assert.py"),
                          "--project", str(tmp_path), "--session", binding["session_id"],
                          "--task", binding["task_id"], "--owner-epoch", str(binding["owner_epoch"]),
                          "--attempt", attempt, "--command-ref", command, "--report-id", report,
                          "--public-report", str(tmp_path / "public-read-api-report.md")],
                         capture_output=True, text=True, timeout=30)
    assert run.returncode == 0 and json.loads(run.stdout)["passed"] is True, run.stdout
    # The public reader cannot be redirected to private storage or link targets.
    import os
    private = tmp_path / "not-the-public-fixture"
    private.write_bytes(content)
    with pytest.raises(ValueError):
        live.public_fixture_bytes(tmp_path, private)
    public = tmp_path / "public-read-api-report.md"
    public.unlink()
    public.symlink_to(private)
    with pytest.raises(OSError):
        live.public_fixture_bytes(tmp_path, public)
    public.unlink()
    os.link(private, public)
    with pytest.raises(ValueError):
        live.public_fixture_bytes(tmp_path, public)


@pytest.mark.usefixtures("unjailed_gateway_command")
@pytest.mark.parametrize("change", ["missing", "wrong_receipt", "wrong_owner", "forged_content",
                                    "replayed", "truncated", "pending", "native_mismatch", "report_truncated"])
def test_live_assertions_cannot_pass_incomplete_or_forged_cases(tmp_path, change):
    live, rows, binding, attempt, command, content, report = live_fixture(tmp_path)
    if change == "wrong_receipt":
        command = "gateway:" + "0" * 32
    elif change == "wrong_owner":
        binding = dict(binding, owner_epoch=binding["owner_epoch"] + 1)
    elif change == "forged_content":
        content += b"x"
    elif change == "report_truncated":
        next(r for r in rows if r["section"] == "read_api_calls" and r.get("operation") == "read_report")["response"]["truncated"] = True
    else:
        index = next(i for i, r in enumerate(rows) if r["section"] == "read_api_calls"
                     and r.get("operation") == "read_large_output")
        if change == "missing":
            rows.pop(index)
        elif change == "replayed":
            rows.append(deepcopy(rows[index]))
        elif change == "truncated":
            rows[index]["response"]["truncated"] = True
        elif change == "pending":
            rows[index]["available"] = False
        elif change == "native_mismatch":
            rows[index]["response"]["sha256"] = "0" * 64
    assert live.evaluate(rows, binding, attempt_id=attempt, command_ref=command,
                         public_report=content, report_id=report)

