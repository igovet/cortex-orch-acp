#!/usr/bin/env python3
"""Read-only public-fixture assertions over one real live task, never a launcher.

No raw response/history/export. Expected output bytes are the public literal in
cortex_read_api_fixture.py; expected schemas come from this stamped checkout.
"""
import argparse
import hashlib
import json
import os
import re
import stat
from pathlib import Path
import sys

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))
from cortex_runtime.channeled_tools import channeled_base_tools
from cortex_runtime.contracts import StoreError, error_help
from cortex_runtime.storage.inspection import read_binding_page
from cortex_runtime.delegation.mcp_tools import tool_definitions
from cortex_runtime.engine.mcp_bridge import always_load
from cortex_runtime.read_api_evidence import fingerprint, summary
from cortex_runtime.tool_read_api import BATCH_READS
from cortex_qualification_assert import _retained_sizes
from cortex_read_api_fixture import PUBLIC, REPORT


def _rows(args):
    """Only the bound inspector authenticates task/session/owner and channels.

    Check every page, not just the final page: missing or mixed snapshot segments
    must never turn into a complete actor-scoped evidence set.
    """
    expected = dict(project_root=str(Path(args.project).resolve()), session_id=args.session,
                    task_id=args.task, owner_epoch=args.owner_epoch)
    rows, snapshot, total, cursor = [], None, None, None
    while True:
        page = read_binding_page(args.project, args.session, args.task, args.owner_epoch,
                                 cursor=cursor, limit=200)
        digest = page.get("snapshot_sha256")
        if (page.get("binding") != expected or page.get("offset") != len(rows)
                or not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest)
                or type(page.get("total")) is not int or page["total"] < 1
                or not isinstance(page.get("rows"), list) or not page["rows"]
                or len(page["rows"]) > 200
                or (snapshot is not None and (snapshot != digest or total != page["total"]))):
            raise StoreError("inspection_snapshot_changed")
        snapshot, total = digest, page["total"]
        rows.extend(page["rows"])
        cursor = page.get("next_cursor")
        if (len(rows) > total or page.get("complete") is not (len(rows) == total)
                or (cursor is None) is not (len(rows) == total)
                or (cursor is not None and cursor != f"{snapshot}:{len(rows)}")):
            raise StoreError("inspection_snapshot_changed")
        if cursor is None:
            if fingerprint(dict(binding=expected, rows=rows))["sha256"] != snapshot:
                raise StoreError("inspection_snapshot_changed")
            return expected, snapshot, rows


def evaluate(rows, binding, *, attempt_id, command_ref, public_report=None, report_id=None, occurrences=None):
    failures = []
    attempts = [r for r in rows if r.get("section") == "attempts" and r.get("attempt_id") == attempt_id]
    if len(attempts) != 1 or attempts[0].get("owner_epoch") != binding["owner_epoch"]:
        return ["live_attempt_identity"]
    attempt = attempts[0]
    nodes = [r for r in rows if r.get("section") == "nodes" and r.get("assignment_ref") == attempt["idempotency_key"]]
    if (len(nodes) != 1 or not attempt.get("engine_session_ref")
            or nodes[0].get("role") != attempt.get("role_id")
            or nodes[0].get("stage") not in ("execution", "review")
            or not attempt.get("assignment_id")
            or any(sum(r.get("section") == "attempts" and r.get(key) == attempt.get(key) for r in rows) != 1
                   for key in ("idempotency_key", "assignment_id", "engine_session_ref"))):
        return ["live_assignment_session_identity"]
    node = nodes[0]
    observations = [r for r in rows if r.get("section") == "read_api_calls"]
    # Authenticate ALL observations before actor selection. "bound" is computed
    # by the inspector from durable current-owner trusted channels, not an actor
    # label supplied in the read payload. Unavailable metadata cannot be scoped.
    for row in observations:
        if row.get("available") is not True:
            failures.append("unavailable_read_occurrence")
            if occurrences is not None:
                occurrences.append(dict(operation=None, arguments=None, count=1,
                                        identities=[dict(id=row.get("id"), attempt_id=row.get("attempt_id"))]))
        if (row.get("bound") is not True or row.get("owner_epoch") != binding["owner_epoch"]
                or not re.fullmatch(r"obs_[0-9a-f]{12}", row.get("id") or "")
                or not re.fullmatch(r"[0-9a-f]{64}", row.get("request_hash") or "")
                or not re.fullmatch(r"[0-9a-f]{32}", row.get("channel_hash") or "")
                or sum(r.get("id") == row.get("id") for r in observations) != 1
                or sum(r.get("request_hash") == row.get("request_hash") and r.get("channel_hash") == row.get("channel_hash")
                       for r in observations) != 1
                or any(r.get("channel_hash") == row.get("channel_hash") and r.get("attempt_id") != row.get("attempt_id")
                       for r in observations)):
            failures.append("read_occurrence_identity")
    selected = [r for r in observations if r.get("attempt_id") == attempt_id]
    other = [r for r in observations if r.get("attempt_id") != attempt_id]
    if occurrences is not None:
        occurrences.append(dict(scope="other_actors", operation=None, arguments=None, count=len(other)))

    def find(operation, arguments, *, error=False):
        # Shape is not identity: a fresh idempotent read may return identical bytes.
        # Shape matching follows authoritative actor binding, never the reverse.
        # Same-task coordinator/reviewer reads are observations, not substitutes.
        args = fingerprint(arguments)
        matches = [r for r in selected if r.get("operation") == operation and r.get("arguments") == args]
        if occurrences is not None:
            occurrences.append(dict(operation=operation, arguments=args, count=len(matches),
                                    other_actor_count=sum(r.get("operation") == operation and r.get("arguments") == args for r in other),
                                    identities=[{k: r.get(k) for k in ("id", "request_hash", "channel_hash", "attempt_id",
                                                                      "owner_epoch")} for r in matches]))
        if not matches:
            failures.append("missing_" + operation)
        for row in matches:
            if (row.get("available") is not True or row.get("bound") is not True or row.get("is_error") is not error
                    or row.get("attempt_id") != attempt_id or row.get("owner_epoch") != binding["owner_epoch"]
                    or not re.fullmatch(r"obs_[0-9a-f]{12}", row.get("id") or "")
                    or not re.fullmatch(r"[0-9a-f]{64}", row.get("request_hash") or "")
                    or not re.fullmatch(r"[0-9a-f]{32}", row.get("channel_hash") or "")
                    or sum(r.get("id") == row.get("id") for r in observations) != 1
                    or sum(r.get("request_hash") == row.get("request_hash") and r.get("channel_hash") == row.get("channel_hash")
                           for r in observations) != 1):
                failures.append("bound_" + operation)
        return matches

    def exact(matches, expected):
        for row in matches:
            if row.get("response") != summary(expected):
                failures.append("expected_response_digest")

    catalogue = sorted([always_load(t) for t in list(channeled_base_tools()) +
                        tool_definitions("worker", role_id=attempt["role_id"], stage=node["stage"])], key=lambda t: t["name"])
    revision = fingerprint(catalogue)["sha256"]
    exact(find("tools_discover", dict(name="read_report")),
          dict(catalog_revision=revision, tool=next(t for t in catalogue if t["name"] == "read_report")))
    previews = [dict(name=t["name"], title=t["title"], description=t["description"][:160],
                     description_truncated=len(t["description"]) > 160, read_batch_supported=t["name"] in BATCH_READS)
                for t in catalogue]
    if len(catalogue) > 50:
        failures.append("catalogue_requires_new_fixture_expectation")
    else:
        exact(find("tools_discover", dict(limit=50)), dict(catalog_revision=revision, tools=previews, next_cursor=None))
    exact(find("tools_discover", dict(name="read_report", limit=1), error=True),
          dict(error="invalid_arguments", message=error_help("invalid_arguments"), field="arguments.limit",
               received="limit is present while name is supplied", expected="limit omitted when name is supplied",
               correction="Omit limit when name is supplied; use the advertised schema form.", write_state="rejected"))

    receipts = [r for r in rows if r.get("section") == "action_receipts" and r.get("tool_call_ref") == command_ref]
    if (len(receipts) != 1 or _retained_sizes(receipts) != [len(PUBLIC.encode())]
            or receipts[0].get("channel_attempt_id") != attempt_id
            or receipts[0].get("channel_owner_epoch") != binding["owner_epoch"]):
        failures.append("public_command_receipt")
        return failures
    receipt = receipts[0]
    output_id = receipt["large_output_id"]
    args = dict(large_output_id=output_id, limit=1000)
    expected = dict(id=output_id, chunk=PUBLIC, encoding="utf-8", offset=0, next_offset=None,
                    byte_count=len(PUBLIC.encode()), truncated=False, exit_status=dict(exit_code=0, signal=None),
                    failure=None, source_kind="terminal", source_ref=command_ref)
    exact(find("read_large_output", args), expected)
    call = dict(id="public", tool="read_large_output", arguments=args)
    batch_args = dict(calls=[call])
    good = dict(id="public", tool="read_large_output", status="ok", result=expected)
    exact(find("read_batch", batch_args), dict(outcome="complete", results=[good], executed_count=1, returned_count=1))
    missing_args = dict(large_output_id="lo_" + "0" * 12, limit=100)
    mixed_args = dict(calls=[call, dict(id="missing", tool="read_large_output", arguments=missing_args)])
    mixed = find("read_batch", mixed_args, error=True)
    safe_missing = dict(code="unknown_large_output_ref",
                        correction="Use the current tools_discover schema and an individually smaller page if needed.")
    exact(mixed, dict(outcome="incomplete", results=[good, dict(id="missing", tool="read_large_output",
                     status="error", error=safe_missing)], executed_count=2, returned_count=1))
    for row in mixed:
        members = row.get("members", [])
        if (len(members) != 2 or members[0].get("status") != "ok"
                or members[0].get("response") != summary(expected) or members[1].get("status") != "error"
                or members[1].get("response") != fingerprint(dict(code="unknown_large_output_ref",
                    correction="Use the current tools_discover schema and an individually smaller page if needed."))):
            failures.append("mixed_native_results")
    rejected_args = dict(calls=[call, dict(id="forbidden", tool="command_run", arguments={})])
    rejected = find("read_batch", rejected_args, error=True)
    exact(rejected, dict(error="invalid_arguments", message=error_help("invalid_arguments"), field="arguments",
                         correction="Use the current tools_discover schema and an individually smaller page if needed.", write_state="rejected"))
    for row in rejected:
        executions = [r for r in rows if r.get("section") == "observability" and r.get("category") == "tool_read_api"
                      and r.get("outcome") == "executed" and r.get("read_api", {}).get("request_hash") == row["request_hash"]
                      and r.get("read_api", {}).get("channel_hash") == row["channel_hash"]]
        rejections = [r for r in rows if r.get("section") == "observability" and r.get("category") == "tool_read_api"
                      and r.get("outcome") == "rejected" and r.get("read_api", {}).get("request_hash") == row["request_hash"]
                      and r.get("read_api", {}).get("channel_hash") == row["channel_hash"]
                      and r.get("attempt_id") == attempt_id]
        if executions or len(rejections) != 1:
            failures.append("rejected_envelope_zero_execution")

    if public_report is not None:
        # This file is a PUBLIC expected fixture, not a dump of private history.
        content = public_report
        pattern = rb"Cortex draft ID: `d_[0-9a-f]{12}`\n\n" + re.escape(REPORT.encode())
        editions = [r for r in rows if r.get("section") == "reports" and r.get("id") == report_id]
        if (re.fullmatch(pattern, content) is None or len(editions) != 1
                or editions[0].get("digest") != hashlib.sha256(content).hexdigest()
                or editions[0].get("size_bytes") != len(content)):
            failures.append("public_fixture_identity")
        report_args = dict(report_id=report_id, limit=4000)
        individual = find("read_report", report_args)
        deferred_args = dict(calls=[dict(id="deferred", tool="read_report", arguments=report_args)])
        deferred = find("read_batch", deferred_args, error=True)
        for row in individual:
            native = row["response"]
            if (native.get("content_sha256") != hashlib.sha256(content).hexdigest()
                    or native.get("content_bytes") != len(content) or native.get("paged") is not False
                    or native.get("bytes", 0) <= 6144 or native.get("truncated") is True
                    or native.get("partial") is True or native.get("complete") is False):
                failures.append("public_report_expected_content")
            for deferred_row in deferred:
                members = deferred_row.get("members", [])
                if (len(members) != 1 or members[0].get("status") != "deferred"
                        or members[0].get("response") != {k: native[k] for k in ("sha256", "bytes")}):
                    failures.append("deferred_native_parity")
                # The retrieval content digest must equal the complete canonical
                # native JSON, not just a claimed retained-output digest.
                retained_id = members[0].get("large_output_id") if len(members) == 1 else None
                retrievals = find("read_large_output", dict(large_output_id=retained_id, limit=16000))
                for retrieval in retrievals:
                    response = retrieval.get("response", {})
                    if (response.get("content_sha256") != native.get("sha256")
                            or response.get("content_bytes") != native.get("bytes")
                            or response.get("paged") is not False or response.get("truncated") is not False
                            or response.get("partial") is True or response.get("complete") is False):
                        failures.append("deferred_exact_retrieval")
                delivery = members[0].get("delivery_request") if len(members) == 1 else None
                if delivery is None or any(r.get("section") == "report_reads" and r.get("attempt_id") == attempt_id
                                           and r.get("batch_request") == delivery for r in rows):
                    failures.append("deferred_is_not_report_delivery")
    else:
        failures.append("deferred_public_fixture_not_supplied")
    return failures


def public_fixture_bytes(project, target):
    """One exact public fixture, no links or private-storage traversal."""
    root = Path(project).resolve(strict=True)
    path = Path(target).absolute()
    if path != root / "public-read-api-report.md":
        raise ValueError("public_fixture_path")
    fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW)
    try:
        before = os.fstat(fd)
        if (not stat.S_ISREG(before.st_mode) or before.st_uid != os.getuid()
                or before.st_nlink != 1 or not 0 <= before.st_size <= 16000):
            raise ValueError("public_fixture_type")
        raw = os.read(fd, 16001)
        after = os.fstat(fd)
        current = path.lstat()
        identity = lambda info: (info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns,
                                 info.st_ctime_ns, info.st_nlink, info.st_mode)
        if (len(raw) != before.st_size or identity(before) != identity(after)
                or identity(current) != identity(before)):
            raise ValueError("public_fixture_changed")
        return raw
    finally:
        os.close(fd)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    for field in ("project", "session", "task", "attempt", "command-ref", "report-id"):
        parser.add_argument("--" + field, required=True)
    parser.add_argument("--owner-epoch", type=int, required=True)
    parser.add_argument("--public-report", type=Path, required=True,
                        help="public expected fixture file created before its read calls, never an observed response dump")
    args = parser.parse_args()
    try:
        content = public_fixture_bytes(args.project, args.public_report)
        binding, snapshot, rows = _rows(args)
        occurrences = []
        failures = evaluate(rows, binding, attempt_id=args.attempt, command_ref=args.command_ref,
                            public_report=content, report_id=args.report_id, occurrences=occurrences)
    except Exception:
        print(json.dumps(dict(passed=False, phase="assertion_refused")))
        return 2
    print(json.dumps(dict(passed=not failures, phase="asserted", accepted=False, binding=binding,
                          snapshot_sha256=snapshot, failures=failures, occurrences=occurrences), sort_keys=True))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
