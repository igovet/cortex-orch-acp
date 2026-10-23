"""Real channel dispatch/stdio coverage; fakes only for races and size boundaries."""
import hashlib
import json

import pytest

from cortex_runtime import tool_read_api as api
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.orchestration.state import TaskMachine
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.recovery.large_output_store import persist_output
from cortex_runtime.server import Server
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import resolve_channel, revoke_channel, issue_channel
from runtime_scenarios import provision_fixture
from test_stdio import exchange


def setup(tmp_path, role=None, stage="execution"):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin="cli", event_id="read-api"),
                          "SOURCE_BODY_SENTINEL", "owner")
    channel = resolve_channel(store, boot.channel_id, boot.secret)
    credentials = (boot.channel_id, boot.secret, str(tmp_path))
    ref = None
    if role:
        channel, ref = provision_fixture(store, channel, role=role)
        # Issue another real channel to the same assignment for transport tests.
        ident, secret = issue_channel(store, boot.task_id, "worker", channel.owner_epoch,
                                      parent_channel_id=boot.channel_id,
                                      assignment_id=channel.assignment_id, attempt_id=channel.attempt_id)
        credentials = (ident, secret, str(tmp_path))
        channel = resolve_channel(store, ident, secret)
        with store.connection() as db:
            db.execute("UPDATE runtime_nodes SET stage=? WHERE assignment_ref=?", (stage, ref))
            db.commit()
    else:
        TaskMachine(store).create(boot.task_id, "Read API")
    return store, boot, channel, Server(channel_credentials=credentials), ref


def call(server, name, args=None, ident="request"):
    return server.dispatch("tools/call", dict(name=name, arguments=args or {}), request_id=ident)


def data(reply):
    return reply.get("structuredContent") or json.loads(reply["content"][0]["text"])


def member(ident="one", tool="list_reports", **arguments):
    return dict(id=ident, tool=tool, arguments=arguments)


def observations(store):
    with store.connection() as db:
        return [dict(row) for row in db.execute("SELECT * FROM report_read_observations")]


@pytest.mark.parametrize("role,stage", [
    (None, "execution"), ("backend-dev", "execution"), ("architect", "execution"),
    ("code-reviewer", "review"), ("senior-consultant", "execution"),
])
def test_discovery_exact_effective_records_and_hidden_names(tmp_path, role, stage):
    store, boot, channel, server, ref = setup(tmp_path, role, stage)
    offered = server.dispatch("tools/list", {})["tools"]
    for tool in offered:
        reply = call(server, "tools_discover", dict(name=tool["name"]))
        assert not reply["isError"], data(reply)
        assert data(reply)["tool"] == tool
    hidden = "project_write" if role != "backend-dev" else "plan_submit"
    missing = data(call(server, "tools_discover", dict(name="not_installed")))
    assert data(call(server, "tools_discover", dict(name=hidden))) == missing
    assert missing["error"] == "tool_not_available"
    if stage == "review":
        schema = data(call(server, "tools_discover", dict(name="result_submit")))["tool"]["inputSchema"]
        assert "verdict" in schema["required"]
    assert {"read_batch", "tools_discover"} <= {tool["name"] for tool in offered}


def test_catalogue_pages_bind_identity_revision_and_utf8_budget(tmp_path):
    store, boot, channel, server, ref = setup(tmp_path)
    names = []
    args = dict(limit=3)
    while True:
        page = data(call(server, "tools_discover", args))
        assert len(api.encoded(page)) <= api.DISCOVERY_BYTES
        assert all("inputSchema" not in item for item in page["tools"])
        assert any(item["description_truncated"] for item in page["tools"])
        names.extend(item["name"] for item in page["tools"])
        if not page["next_cursor"]:
            break
        args = dict(limit=3, cursor=page["next_cursor"])
    assert names == sorted(tool["name"] for tool in server.dispatch("tools/list", {})["tools"])
    cursor = data(call(server, "tools_discover", dict(limit=1)))["next_cursor"]
    other_id, secret = issue_channel(store, boot.task_id, "coordinator", channel.owner_epoch)
    other = Server(channel_credentials=(other_id, secret, str(tmp_path)))
    assert data(call(other, "tools_discover", dict(cursor=cursor)))["error"] == "invalid_cursor"
    assert data(call(server, "tools_discover", dict(cursor="td1.bad")))["error"] == "invalid_cursor"


@pytest.mark.parametrize("bad", [
    {}, {"calls":[]}, {"calls":[member(str(i)) for i in range(9)]},
    {"calls":[member(), member()]},
    {"calls":[member()], "task_id":"PRIVATE"},
    {"calls":[dict(member(), extra="PRIVATE")]},
    {"calls":[member(tool="read_report", report_ref="PRIVATE")]},
    {"calls":[member(tool="read_report", report_id="r_000000000000", limit=4001)]},
    {"calls":[member(ident="bad id")]},
    {"calls":[member(tool="list_sources", limit=True)]},
    *[{"calls":[member(tool="list_sources", **{key:"PRIVATE"})]} for key in
      ["task_id","attempt_id","session_id","channel_id","owner_epoch","engine","effort"]],
])
def test_preflight_is_all_or_nothing(tmp_path, bad, monkeypatch):
    store, boot, channel, server, ref = setup(tmp_path)
    def unexpected(*args):
        pytest.fail("preflight executed a member")
    monkeypatch.setattr(api, "_execute", unexpected)
    reply = call(server, "read_batch", bad)
    assert reply["isError"]
    assert "PRIVATE" not in json.dumps(reply)


@pytest.mark.parametrize("tool", [
    "command_run", "outside_command", "project_read", "read_skill", "project_edit",
    "write_report", "result_submit", "read_batch", "tools_discover",
    "plan_submit", "server/discover", "mcp__external__read", "not_installed",
])
def test_forbidden_tools_never_execute_even_valid_sibling(tmp_path, tool, monkeypatch):
    store, boot, channel, server, ref = setup(tmp_path)
    monkeypatch.setattr(api, "_execute", lambda *args: pytest.fail("forbidden dispatch"))
    reply = call(server, "read_batch", dict(calls=[member(), member("bad", tool)]))
    assert reply["isError"]
    assert observations(store) == []


def test_request_bytes_and_depth_fail_before_validation(tmp_path, monkeypatch):
    store, boot, channel, server, ref = setup(tmp_path)
    nested = {}
    for _ in range(17):
        nested = dict(nested=nested)
    monkeypatch.setattr(api, "_execute", lambda *args: pytest.fail("oversized dispatch"))
    for args in [dict(calls=[member(tool="read_report", report_id="😀"*10000)]),
                 dict(calls=[member(tool="list_sources", nested=nested)])]:
        assert call(server, "read_batch", args)["isError"]


def test_all_six_reads_keep_native_data_and_independent_failures(tmp_path):
    store, boot, channel, server, ref = setup(tmp_path)
    draft = data(call(server, "create_draft", dict(template="general")))
    sources = data(call(server, "list_sources"))
    source_ref = sources["entries"][0]["source_ref"]
    output = persist_output(store, task_id=boot.task_id, owner_scope="coordinator",
                            source_kind="tool_result", data=b"retained")
    calls = [
        member("reports", "list_reports", limit=1),
        member("report", "read_report", report_id=boot.report_id, limit=80),
        member("draft", "read_draft", draft_id=draft["draft_id"], limit=80),
        member("sources", "list_sources", limit=1),
        member("source", "read_source", source_ref=source_ref, limit=8),
        member("output", "read_large_output", large_output_id=output["id"], limit=4),
    ]
    expected = [data(call(server, item["tool"], item["arguments"])) for item in calls]
    reply = call(server, "read_batch", dict(calls=calls))
    assert not reply["isError"], data(reply)
    assert [item["result"] for item in data(reply)["results"]] == expected
    assert data(reply)["executed_count"] == data(reply)["returned_count"] == 6
    calls.insert(1, member("missing", "read_report", report_id="r_000000000000"))
    reply = call(server, "read_batch", dict(calls=calls))
    assert reply["isError"]
    result = data(reply)
    assert result["outcome"] == "incomplete" and result["executed_count"] == 7
    assert [item["status"] for item in result["results"]] == ["ok", "error"] + ["ok"]*5
    assert "received" not in result["results"][1]["error"]


def test_inline_report_evidence_has_typed_outer_item_binding(tmp_path):
    store, boot, channel, server, ref = setup(tmp_path, "backend-dev")
    calls = [member("first", "read_report", report_id=boot.report_id, limit=80),
             member("second", "read_report", report_id=boot.report_id, limit=80)]
    for ident in [1, "1"]:
        assert not call(server, "read_batch", dict(calls=calls), ident=ident)["isError"]
    rows = observations(store)
    assert {row["request_id"] for row in rows} == {
        api._member_request_id(ident, item["id"]) for ident in [1, "1"] for item in calls}
    assert {row["attempt_id"] for row in rows} == {channel.attempt_id}
    assert len(rows) == 4


@pytest.mark.parametrize("loss", ["revoked", "expired", "epoch", "stopped", "assignment", "stage", "role"])
def test_authority_or_offers_change_mid_batch_withholds_all(tmp_path, loss, monkeypatch):
    store, boot, channel, server, ref = setup(tmp_path, "backend-dev")
    server._channel_context()  # Deliberately warm the pre-existing context cache.
    execute = api._execute
    seen = []
    def interleave(*args):
        result = execute(*args)
        seen.append(args[2])
        with store.connection() as db:
            if loss == "revoked":
                db.execute("UPDATE trusted_channels SET revoked_at='now' WHERE channel_id=?", (channel.channel_id,))
            elif loss == "expired":
                db.execute("UPDATE trusted_channels SET expires_at='2000' WHERE channel_id=?", (channel.channel_id,))
            elif loss == "epoch":
                db.execute("UPDATE task_owners SET epoch=epoch+1 WHERE task_id=?", (boot.task_id,))
            elif loss == "stopped":
                db.execute("UPDATE runtime_tasks SET state='CANCELLED' WHERE task_id=?", (boot.task_id,))
            elif loss == "assignment":
                db.execute("UPDATE runtime_nodes SET status='cancelled' WHERE assignment_ref=?", (ref,))
            elif loss == "stage":
                db.execute("UPDATE runtime_nodes SET stage='review' WHERE assignment_ref=?", (ref,))
            else:
                db.execute("UPDATE runtime_nodes SET role='worker-architect' WHERE assignment_ref=?", (ref,))
            db.commit()
        return result
    monkeypatch.setattr(api, "_execute", interleave)
    reply = call(server, "read_batch", dict(calls=[
        member("one", "read_report", report_id=boot.report_id, limit=50), member("two")]))
    assert reply["isError"] and "results" not in data(reply)
    assert seen == ["read_report"]
    assert observations(store) == []
    with store.connection() as db:
        assert db.execute("SELECT count(*) FROM observability_events WHERE category='tool_read_api' AND outcome='executed'").fetchone()[0] == 1


@pytest.mark.parametrize("operation", ["tools_discover", "read_batch"])
def test_cached_revocation_and_unbound_channels_fail_closed(tmp_path, operation):
    store, boot, channel, server, ref = setup(tmp_path)
    server._channel_context()
    revoke_channel(store, channel.channel_id)
    args = {} if operation == "tools_discover" else dict(calls=[member()])
    assert call(server, operation, args)["isError"]
    assert Server().dispatch("tools/call", dict(name=operation, arguments=args))["isError"]


def test_unresolved_worker_profile_is_not_a_schema_union(tmp_path):
    store, boot, channel, server, ref = setup(tmp_path, "backend-dev")
    with store.connection() as db:
        db.execute("UPDATE runtime_nodes SET role='' WHERE assignment_ref=?", (ref,))
        db.commit()
    assert data(call(server, "tools_discover"))["error"] == "tool_profile_unavailable"


def test_admission_budget_does_not_claim_to_preempt_inflight_read(tmp_path, monkeypatch):
    store, boot, channel, server, ref = setup(tmp_path)
    clock = [0.0]
    execute = api._execute
    def slow(*args):
        result = execute(*args)
        clock[0] += 6
        return result
    monkeypatch.setattr(api.time, "monotonic", lambda: clock[0])
    monkeypatch.setattr(api, "_execute", slow)
    reply = call(server, "read_batch", dict(calls=[member("one"), member("two")]))
    assert reply["isError"]
    assert [item["status"] for item in data(reply)["results"]] == ["ok", "not_run"]
    assert data(reply)["executed_count"] == 1


def test_oversized_result_is_exact_owned_deferred_json_not_report_read(tmp_path, monkeypatch):
    store, boot, channel, server, ref = setup(tmp_path, "backend-dev")
    # Lower the cap to exercise real read_report data without synthetic report content.
    monkeypatch.setattr(api, "ITEM_BYTES", 50)
    native = data(call(server, "read_report", dict(report_id=boot.report_id, limit=80), ident=None))
    reply = call(server, "read_batch", dict(calls=[
        member(tool="read_report", report_id=boot.report_id, limit=80)]))
    item = data(reply)["results"][0]
    assert reply["isError"] and item["status"] == "deferred"
    raw = api.encoded(native)
    assert item["byte_count"] == len(raw) and item["result_sha256"] == hashlib.sha256(raw).hexdigest()
    chunks = []
    offset = 0
    while True:
        page = data(call(server, "read_large_output", dict(
            large_output_id=item["large_output_id"], offset=offset, limit=100)))
        chunks.append(page["chunk"])
        if page["next_offset"] is None:
            break
        offset = page["next_offset"]
    assert "".join(chunks).encode() == raw
    assert observations(store) == []
    other, other_ref = provision_fixture(store, resolve_channel(store, boot.channel_id, boot.secret),
                                          key="other", role="architect")
    with pytest.raises(StoreError, match="unknown_large_output_ref"):
        mcp_tools.dispatch(store, other, "read_large_output", dict(large_output_id=item["large_output_id"]))


def test_large_output_has_no_nested_spill_and_retains_failure_metadata(tmp_path):
    store, boot, channel, server, ref = setup(tmp_path)
    output = persist_output(store, task_id=boot.task_id, owner_scope="coordinator", source_kind="terminal",
                            data=("😀"*5000).encode(), exit_code=9, failure="failed", truncated=True)
    reply = call(server, "read_batch", dict(calls=[
        member(tool="read_large_output", large_output_id=output["id"], limit=20000)]))
    assert reply["isError"]
    assert data(reply)["results"][0]["error"]["code"] == "batch_result_limit"
    page = data(call(server, "read_batch", dict(calls=[
        member(tool="read_large_output", large_output_id=output["id"], limit=10)])))["results"][0]["result"]
    assert page["failure"] == "failed" and page["truncated"] and page["exit_status"]["exit_code"] == 9


def test_retention_failure_and_capture_cap_are_errors_not_success(tmp_path, monkeypatch):
    store, boot, channel, server, ref = setup(tmp_path)
    monkeypatch.setattr(api, "ITEM_BYTES", 1)
    monkeypatch.setattr(api, "_retain", lambda *a: (_ for _ in ()).throw(OSError("PRIVATE")))
    reply = call(server, "read_batch", dict(calls=[member()]))
    assert data(reply)["results"][0]["error"]["code"] == "storage_error"
    assert "PRIVATE" not in json.dumps(reply)
    monkeypatch.setattr(api, "_execute", lambda *a: dict(body="x"*(api.CAPTURE_BYTES+1)))
    reply = call(server, "read_batch", dict(calls=[member()]))
    assert data(reply)["results"][0]["error"]["code"] == "result_too_large"


def test_named_schema_defer_and_stale_discovery_cursor(tmp_path, monkeypatch):
    store, boot, channel, server, ref = setup(tmp_path)
    cursor = data(call(server, "tools_discover", dict(limit=1)))["next_cursor"]
    original = api.effective_catalogue
    def changed(*args, **kwargs):
        tools = original(*args, **kwargs)
        tools[0] = dict(tools[0], description="changed")
        return tools
    monkeypatch.setattr(api, "effective_catalogue", changed)
    assert data(call(server, "tools_discover", dict(cursor=cursor)))["error"] == "discovery_cursor_stale"
    monkeypatch.setattr(api, "SCHEMA_BYTES", 1)
    reply = call(server, "tools_discover", dict(name="read_report"))
    assert reply["isError"] and data(reply)["status"] == "deferred"
    assert "tool" not in data(reply)


def test_batch_images_fail_explicitly_without_flattening_a_real_image(tmp_path):
    from test_image_read_parity import _image_attachment_task
    store, boot = _image_attachment_task(tmp_path)
    TaskMachine(store).create(boot.task_id, "Read image")
    server = Server(channel_credentials=(boot.channel_id, boot.secret, str(tmp_path)))
    reply = call(server, "read_batch", dict(calls=[
        member(tool="read_source", source_ref=f"{boot.report_id}:0")]))
    assert reply["isError"] and data(reply)["results"][0]["error"]["code"] == "batch_multimodal_unsupported"
    assert call(server, "read_source", dict(source_ref=f"{boot.report_id}:0"))["content"][0]["type"] == "image"


def test_real_stdio_transport_discovery_batch_errors_and_native_fallback(tmp_path):
    store, boot, channel, server, ref = setup(tmp_path)
    messages = [
        dict(jsonrpc="2.0", id=1, method="tools/list"),
        dict(jsonrpc="2.0", id=2, method="tools/call", params=dict(name="tools_discover", arguments=dict(name="read_report"))),
        dict(jsonrpc="2.0", id=3, method="tools/call", params=dict(name="read_batch", arguments=dict(calls=[
            member("good"), member("missing", "read_report", report_id="r_000000000000")]))),
        dict(jsonrpc="2.0", id=4, method="tools/call", params=dict(name="list_reports", arguments={})),
        *[dict(jsonrpc="2.0", id=5+i, method="tools/call", params=dict(
            name="read_batch", arguments=dict(calls=[member("good"), member("bad", name)])))
          for i, name in enumerate(["command_run", "write_report", "read_batch", "mcp__external__read"])],
    ]
    replies = exchange(tmp_path, messages, channel=boot)
    assert replies[1]["result"]["structuredContent"]["tool"] == next(
        tool for tool in replies[0]["result"]["tools"] if tool["name"] == "read_report")
    assert replies[2]["result"]["isError"] and replies[2]["result"]["structuredContent"]["outcome"] == "incomplete"
    assert not replies[3]["result"]["isError"]
    assert all(reply["result"]["isError"] and "results" not in data(reply["result"])
               for reply in replies[4:])


@pytest.mark.parametrize("name", ["read_batch", "tools_discover"])
def test_new_reads_never_expand_bodies_into_display_history(name):
    from cortex_runtime.acp.read_history import ReadHistoryFilter
    filter_ = ReadHistoryFilter()
    for update in [
        dict(toolCallId="one", title="mcp__cortex__"+name, status="in_progress"),
        dict(toolCallId="one", status="completed", rawOutput={"result":"PRIVATE_BODY"}),
    ]:
        assert "PRIVATE_BODY" not in json.dumps(filter_.project(update))


def test_final_authority_gate_withholds_body_and_report_observation(tmp_path, monkeypatch):
    store, boot, channel, server, ref = setup(tmp_path, "backend-dev")
    audit = api._audit
    def revoke_at_summary(store, channel, outcome, **detail):
        audit(store, channel, outcome, **detail)
        if detail.get("operation") == "read_batch" and outcome == "complete":
            revoke_channel(store, channel.channel_id)
    monkeypatch.setattr(api, "_audit", revoke_at_summary)
    reply = call(server, "read_batch", dict(calls=[
        member(tool="read_report", report_id=boot.report_id, limit=50)]))
    assert reply["isError"] and "results" not in data(reply)
    assert observations(store) == []


def test_natural_large_source_defers_and_keeps_source_cursor(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    boot = bootstrap_task(store, SourceEvent(origin="cli", event_id="large"),
                          "🧪" * 10000, "owner")
    TaskMachine(store).create(boot.task_id, "Read source")
    server = Server(channel_credentials=(boot.channel_id, boot.secret, str(tmp_path)))
    source_ref = data(call(server, "list_sources"))["entries"][0]["source_ref"]
    native = data(call(server, "read_source", dict(source_ref=source_ref, limit=20000)))
    reply = call(server, "read_batch", dict(calls=[
        member(tool="read_source", source_ref=source_ref, limit=20000)]))
    item = data(reply)["results"][0]
    assert reply["isError"] and item["status"] == "deferred"
    raw = data(call(server, "read_large_output", dict(large_output_id=item["large_output_id"])))["chunk"].encode()
    assert json.loads(raw) == native
    assert json.loads(raw)["next_offset"] == native["next_offset"]


def test_native_stale_draft_cursor_is_one_item_error(tmp_path):
    store, boot, channel, server, ref = setup(tmp_path, "backend-dev")
    draft = data(call(server, "create_draft", dict(template="general")))
    page = data(call(server, "read_draft", dict(draft_id=draft["draft_id"], limit=5)))
    assert page["next_cursor"]
    content = draft["required_first_line"] + "\n\nChanged draft"
    assert not call(server, "draft_write", dict(draft_id=draft["draft_id"], content=content, request_key="changed"))["isError"]
    reply = call(server, "read_batch", dict(calls=[
        member(tool="read_draft", draft_id=draft["draft_id"], cursor=page["next_cursor"]),
        member("next")]))
    assert reply["isError"]
    assert [item["status"] for item in data(reply)["results"]] == ["error", "ok"]


def test_cross_task_reads_do_not_rebind(tmp_path):
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.mkdir()
    second.mkdir()
    store, boot, channel, server, ref = setup(first)
    other_store, other_boot, other_channel, other_server, ref = setup(second)
    source_ref = data(call(other_server, "list_sources"))["entries"][0]["source_ref"]
    reply = call(server, "read_batch", dict(calls=[
        member("foreign_report", "read_report", report_id=other_boot.report_id),
        member("foreign_source", "read_source", source_ref=source_ref),
        member("own")]))
    assert reply["isError"]
    assert [item["status"] for item in data(reply)["results"]] == ["error", "error", "ok"]
    assert "SOURCE_BODY_SENTINEL" not in json.dumps(data(reply)["results"][:2])


def test_eight_maximum_inline_objects_fit_final_bound(tmp_path, monkeypatch):
    store, boot, channel, server, ref = setup(tmp_path)
    native = dict(body="x"*(api.ITEM_BYTES-len(api.encoded(dict(body="")))))
    assert len(api.encoded(native)) == api.ITEM_BYTES
    monkeypatch.setattr(api, "_execute", lambda *a: native)
    reply = call(server, "read_batch", dict(calls=[member(str(i)) for i in range(8)]))
    assert not reply["isError"]
    assert len(api.encoded(data(reply))) <= api.RESULT_BYTES
    monkeypatch.setattr(api, "_execute", lambda *a: dict(body=native["body"]+"x"))
    assert data(call(server, "read_batch", dict(calls=[member()])))["results"][0]["status"] == "deferred"


def test_discovery_oversized_schema_fails_explicitly(tmp_path, monkeypatch):
    store, boot, channel, server, ref = setup(tmp_path)
    original = api.effective_catalogue
    def large(*args, **kwargs):
        tools = original(*args, **kwargs)
        return [dict(tool, description="x"*(api.CAPTURE_BYTES+1))
                if tool["name"] == "read_report" else tool for tool in tools]
    monkeypatch.setattr(api, "effective_catalogue", large)
    reply = call(server, "tools_discover", dict(name="read_report"))
    assert reply["isError"] and data(reply)["error"] == "schema_too_large"


@pytest.mark.parametrize("args,expected_fields", [
    (dict(name="read_report", cursor="invalid"), ["arguments.cursor"]),
    (dict(name="read_report", limit=1), ["arguments.limit"]),
    (dict(name="read_report", cursor="invalid", limit=1),
     ["arguments.cursor", "arguments.limit"]),
])
def test_discovery_conditional_errors_name_the_incompatible_fields(tmp_path, args, expected_fields):
    store, boot, channel, server, ref = setup(tmp_path)
    reply = call(server, "tools_discover", args)
    assert reply["isError"] and "tool" not in data(reply)
    error = data(reply)
    assert error["field"] == expected_fields[0], error
    assert "name" in error["received"]
    assert error["expected"] == f"{expected_fields[0].rsplit('.', 1)[-1]} omitted when name is supplied"
    assert error["correction"] == (
        f"Omit {expected_fields[0].rsplit('.', 1)[-1]} when name is supplied; "
        "use the advertised schema form."
    )
    assert not any(term in json.dumps(error).lower() for term in ("command", "tool_call_ref", "blocker"))


@pytest.mark.parametrize("args", [
    dict(limit=0), dict(limit=51), dict(cursor="x"*257), dict(name="x"*65),
])
def test_discovery_other_malformed_requests_still_fail_without_a_schema(tmp_path, args):
    store, boot, channel, server, ref = setup(tmp_path)
    reply = call(server, "tools_discover", args)
    assert reply["isError"] and "tool" not in data(reply)
    assert data(reply)["field"]


def test_audit_is_metadata_only_and_measurement_is_bytes_not_tokens(tmp_path):
    store, boot, channel, server, ref = setup(tmp_path)
    calls = [member(str(i), "read_report", report_id=boot.report_id, limit=100) for i in range(3)]
    reply = call(server, "read_batch", dict(calls=calls))
    assert not reply["isError"]
    with store.connection() as db:
        audit = [dict(row) for row in db.execute("SELECT * FROM observability_events WHERE category='tool_read_api'")]
    assert "SOURCE_BODY_SENTINEL" not in json.dumps(audit)
    assert boot.report_id not in json.dumps(audit)
    assert all(row["basis"] == "observed" for row in audit)
    individual_request_bytes = sum(len(api.encoded(item["arguments"])) for item in calls)
    batch_request_bytes = len(api.encoded(dict(calls=calls)))
    assert batch_request_bytes > individual_request_bytes
    assert len(api.encoded(data(reply))) <= api.RESULT_BYTES
