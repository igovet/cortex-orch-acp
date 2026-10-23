#!/usr/bin/env python3
"""No-paid, real-stdio read API assertions in a new disposable store.

Expected content is public literal data. Native individual reads establish
parity separately from batching; schema expectations come from tools/list.
No caller project, credentials, history bodies or paid engines are read.
"""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "src"))
from cortex_runtime.acp import sessions
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.read_api_evidence import evaluate_manifest
from cortex_runtime.recovery.large_output_store import persist_output
from cortex_runtime.storage.inspection import read_binding_page
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel

PUBLIC = "Cortex public fixture v1: λ\n"
REPORT = "# Public fixture\n" + "λ" * 3600 + "\n"


def canonical(value):
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode()


def fp(value):
    raw = canonical(value)
    return dict(sha256=hashlib.sha256(raw).hexdigest(), bytes=len(raw))


def expected_summary(value):
    result = dict(fp(value), paged=any(value.get(k) is not None for k in ("next_cursor", "next_offset")))
    for key in ("complete", "partial", "truncated"):
        if type(value.get(key)) is bool:
            result[key] = value[key]
    text = value.get("markdown", value.get("chunk"))
    if isinstance(text, str):
        raw = text.encode()
        result.update(content_sha256=hashlib.sha256(raw).hexdigest(), content_bytes=len(raw))
    return result


class Fixture:
    def __init__(self, root):
        self.root = root
        self.store = Store(root / ".cortex", project_root=root)
        self.boot = bootstrap_task(self.store, SourceEvent(origin="cli", event_id="public-read-fixture-v1"), PUBLIC, "fixture")
        TaskMachine(self.store).create(self.boot.task_id, "Public read fixture")
        self.channel = resolve_channel(self.store, self.boot.channel_id, self.boot.secret)
        self.session = "acps_" + "a" * 12
        sessions.create_session(self.store, self.session, str(root), {"name": "fixture"}, {}, [])
        sessions.bind_task(self.store, self.session, self.boot.task_id)
        self.binding = dict(project_root=str(root), session_id=self.session, task_id=self.boot.task_id,
                            owner_epoch=self.channel.owner_epoch)
        self.expected = []
        self.counter = 0

    def rpc(self, method, params):
        self.counter += 1
        request_id = self.counter
        env = dict(os.environ, CORTEX_CHANNEL_ID=self.boot.channel_id, CORTEX_CHANNEL_SECRET=self.boot.secret,
                   CORTEX_CHANNEL_PROJECT_ROOT=str(self.root), CODEX_HOME=str(self.root / "host"))
        host = self.root / "host" / "cortex"
        host.mkdir(parents=True, exist_ok=True)
        host.chmod(0o700)
        config = host / "config.toml"
        config.write_text("schema_version = 1\n[gateway]\nenabled = false\n")
        config.chmod(0o600)
        messages = [
            dict(jsonrpc="2.0", id=0, method="initialize", params=dict(protocolVersion="2025-11-25",
                 capabilities={}, clientInfo=dict(name="public-fixture", version="1"))),
            dict(jsonrpc="2.0", method="notifications/initialized"),
            dict(jsonrpc="2.0", id=request_id, method=method, params=params)]
        run = subprocess.run([sys.executable, "-B", str(ROOT / "src/cortex.py")],
                             input="\n".join(json.dumps(m) for m in messages) + "\n",
                             text=True, capture_output=True, env=env, timeout=30)
        assert run.returncode == 0 and not run.stderr, "stdio_process_failed"
        replies = [json.loads(line) for line in run.stdout.splitlines()]
        assert len(replies) == 2 and replies[-1]["id"] == request_id and "result" in replies[-1], "stdio_reply_identity"
        reply = replies[-1]["result"]
        return request_id, reply

    def call(self, name, args, expected=None, *, error=False, members=None):
        request_id, reply = self.rpc("tools/call", dict(name=name, arguments=args))
        body = reply.get("structuredContent")
        if body is None:
            assert len(reply["content"]) == 1 and reply["content"][0]["type"] == "text"
            body = json.loads(reply["content"][0]["text"])
        assert reply["isError"] is error, "unexpected_mcp_outcome"
        if expected is not None:
            assert body == expected, "semantic_response_mismatch"
        # Expectations are built from independently supplied native/schema/literal
        # values. No observed boundary row is copied into the expected manifest.
        if expected is not None:
            channel_hash = hashlib.sha256(canonical([
                self.channel.channel_id, self.channel.owner_epoch, self.channel.attempt_id])).hexdigest()[:32]
            entry = dict(version=1, available=True, operation=name, owner_epoch=self.channel.owner_epoch,
                         channel_hash=channel_hash, request_hash=fp(["int", request_id])["sha256"],
                         arguments=fp(args), response=expected_summary(expected), is_error=error)
            if members is not None:
                entry["members"] = [dict(member, delivery_request="batch:" + fp(
                    ["int", request_id, args["calls"][i]["id"]])["sha256"]) for i, member in enumerate(members)]
            self.expected.append(entry)
        return body


def run_fixture(root):
    f = Fixture(root)
    _, listing = f.rpc("tools/list", {})
    catalogue = listing["tools"]
    revision = fp(catalogue)["sha256"]
    schema = next(t for t in catalogue if t["name"] == "read_report")
    f.call("tools_discover", dict(name="read_report"), dict(catalog_revision=revision, tool=schema))
    # Page ending avoids an opaque cursor expectation; all catalogue entries
    # currently fit at the advertised maximum (asserted, never silently clipped).
    assert len(catalogue) <= 50
    previews = [dict(name=t["name"], title=t["title"], description=t["description"][:160],
                     description_truncated=len(t["description"]) > 160,
                     read_batch_supported=t["name"] in {"list_reports", "read_report", "read_draft", "list_sources", "read_source", "read_large_output"})
                for t in catalogue]
    f.call("tools_discover", dict(limit=50), dict(catalog_revision=revision, tools=previews, next_cursor=None))
    # The exact diagnostic is independent public expected data, not just isError.
    from cortex_runtime.contracts import error_help
    diagnostic = dict(error="invalid_arguments", message=error_help("invalid_arguments"),
                      field="arguments.limit", received="limit is present while name is supplied",
                      expected="limit omitted when name is supplied",
                      correction="Omit limit when name is supplied; use the advertised schema form.", write_state="rejected")
    f.call("tools_discover", dict(name="read_report", limit=1), diagnostic, error=True)

    raw = PUBLIC.encode()
    record = persist_output(f.store, task_id=f.boot.task_id, owner_scope="coordinator",
                            source_kind="tool_result", data=raw)
    output = dict(id=record["id"], chunk=PUBLIC, encoding="utf-8", offset=0, next_offset=None,
                  byte_count=len(raw), truncated=False, exit_status=dict(exit_code=None, signal=None),
                  failure=None, source_kind="tool_result", source_ref=None)
    arguments = dict(large_output_id=record["id"], limit=1000)
    f.call("read_large_output", arguments, output)
    item = dict(id="public", tool="read_large_output", arguments=arguments)
    native = dict(index=0, item_hash=fp("public")["sha256"], operation="read_large_output",
                  arguments=fp(arguments), status="ok", response=expected_summary(output))
    good = dict(outcome="complete", results=[dict(id="public", tool="read_large_output", status="ok", result=output)],
                executed_count=1, returned_count=1)
    f.call("read_batch", dict(calls=[item]), good, members=[native])

    draft = f.call("create_draft", dict(template="general", request_key="public-draft"))
    markdown = draft["required_first_line"] + "\n\n" + REPORT
    f.call("draft_write", dict(draft_id=draft["draft_id"], content=markdown, request_key="public-write"))
    published = f.call("write_report", dict(draft_id=draft["draft_id"], title="Public fixture",
                       summary="Public deterministic fixture.", author="fixture", request_key="public-publish"))
    read_args = dict(report_id=published["report_id"], limit=4000)
    individual = f.call("read_report", read_args)
    assert individual["markdown"] == markdown and individual["next_cursor"] is None, "public_report_content"
    # Replay the independently checked individual value as the parity expectation.
    f.call("read_report", read_args, individual)
    batch_args = dict(calls=[dict(id="deferred", tool="read_report", arguments=read_args)])
    deferred = f.call("read_batch", batch_args, error=True)
    [member] = deferred["results"]
    assert member["status"] == "deferred" and member["result_sha256"] == fp(individual)["sha256"]
    assert member["byte_count"] == len(canonical(individual))
    retrieval_args = dict(large_output_id=member["large_output_id"], limit=16000)
    retrieved = f.call("read_large_output", retrieval_args)
    assert retrieved["next_offset"] is None and not retrieved["truncated"]
    assert retrieved["chunk"].encode() == canonical(individual), "deferred_exact_bytes"
    f.call("read_large_output", retrieval_args, retrieved)
    # No successful deferred report delivery observation (coordinator reads have
    # no worker handoff observation either). Worker-specific regressions cover it.
    before = f.store.connection()
    with before as db:
        count = db.execute("SELECT COUNT(*) FROM report_read_observations").fetchone()[0]
    assert count == 0

    mixed_args = dict(calls=[item, dict(id="missing", tool="read_large_output",
                                      arguments=dict(large_output_id="lo_" + "0" * 12, limit=100))])
    mixed = f.call("read_batch", mixed_args, error=True)
    assert mixed["results"][0] == good["results"][0]
    assert mixed["results"][1]["status"] == "error" and mixed["executed_count"] == 2
    # Rejected envelope executes no member, even its valid sibling.
    rejected_args = dict(calls=[item, dict(id="forbidden", tool="command_run", arguments={})])
    with f.store.connection() as db:
        executions = db.execute("SELECT COUNT(*) FROM observability_events WHERE category='tool_read_api' AND outcome='executed'").fetchone()[0]
    rejected = f.call("read_batch", rejected_args, error=True)
    assert rejected["error"] == "invalid_arguments"
    with f.store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM observability_events WHERE category='tool_read_api' AND outcome='executed'").fetchone()[0] == executions

    page = read_binding_page(str(root), f.session, f.boot.task_id, f.channel.owner_epoch, limit=200)
    rows = list(page["rows"])
    while page["next_cursor"]:
        page = read_binding_page(str(root), f.session, f.boot.task_id, f.channel.owner_epoch,
                                 cursor=page["next_cursor"], limit=200)
        rows.extend(page["rows"])
    manifest = dict(binding=f.binding, calls=f.expected)
    failures = evaluate_manifest(rows, f.binding, manifest)
    differences = []
    for index, expected in enumerate(f.expected):
        matches = [r for r in rows if r.get("section") == "read_api_calls" and r.get("request_hash") == expected["request_hash"]]
        if len(matches) == 1:
            differences.append((index, [key for key in expected if expected[key] != matches[0].get(key)]))
    assert not failures, ("immutable_evidence_mismatch", failures, differences)
    expected_path = root / "public-expectations.json"
    expected_path.write_bytes(canonical(manifest))
    state = next(r["state"] for r in rows if r["section"] == "runtime")
    check = subprocess.run([sys.executable, "-B", str(ROOT / "scripts/cortex_qualification_assert.py"),
                            "--project", str(root), "--session", f.session, "--task", f.boot.task_id,
                            "--owner-epoch", str(f.channel.owner_epoch), "--expect-state", state,
                            "--read-api-expectations", str(expected_path)], capture_output=True, text=True, timeout=30)
    assert check.returncode == 0, "assertion_cli_failed"
    return dict(passed=True, phase="fixture_asserted", paid_dispatch=False, accepted=False,
                expected_calls=len(f.expected), binding=f.binding, expectations_sha256=fp(manifest)["sha256"],
                public_sha256=hashlib.sha256(raw).hexdigest(), public_bytes=len(raw),
                snapshot_sha256=page["snapshot_sha256"])


def main():
    try:
        with tempfile.TemporaryDirectory(prefix="cortex-public-read-api-") as directory:
            result = run_fixture(Path(directory).resolve())
        print(json.dumps(result, sort_keys=True))
        return 0
    except Exception as error:
        # Never leak a response, secret, path, assertion body or host log.
        print(json.dumps(dict(passed=False, phase="fixture_failed", error=type(error).__name__,
                              paid_dispatch=False, accepted=False)))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
