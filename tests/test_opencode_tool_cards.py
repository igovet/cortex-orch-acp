"""OpenCode Code Mode tool calls render in the common Codex/Claude card format.

The OpenCode updates below are the structural shapes observed in a real
OpenCode 2.x session (engine.opencode_tool_calls docstring): a pending
``execute`` wrapper, an in_progress update carrying only ``rawInput.code``,
and a completed update with ``rawOutput.metadata.toolCalls`` and the returned
text only in ACP ``content``.
"""
from __future__ import annotations

from types import SimpleNamespace

from cortex_runtime.acp.orchestrating_runtime import (
    OrchestratingRuntime,
    _human_tool_title,
    _worker_tool_details,
)
from cortex_runtime.engine.opencode_tool_calls import CodeModeToolCalls
from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.portable.engine_contract import EngineEventKind

RESULT_TEXT = '{"status": "accepted"}'


def _opencode_updates(call_id, code, tool_calls, *, text=RESULT_TEXT, error=None):
    metadata = {"toolCalls": tool_calls, "truncated": False}
    if error is not None:
        metadata["error"] = error
    return [
        dict(sessionUpdate="tool_call", toolCallId=call_id, kind="other", title="execute",
             status="pending", rawInput={}),
        dict(sessionUpdate="tool_call_update", toolCallId=call_id, status="in_progress",
             rawInput={"code": code}),
        dict(sessionUpdate="tool_call_update", toolCallId=call_id, status="completed",
             rawOutput={"metadata": metadata},
             content=[{"type": "content", "content": {"type": "text", "text": text}}]),
    ]


READ_REPORT_CODE = 'return await tools.cortex.read_report({ report_id: "r_abc" })'
READ_REPORT_CALLS = [{"tool": "cortex.read_report", "input": {"report_id": "r_abc"}, "status": "completed"}]


def _normalize_all(updates, normalizer=None, session="s1"):
    normalizer = normalizer or CodeModeToolCalls()
    return [normalizer.normalize(session, update) for update in updates]


def test_code_mode_call_is_rewritten_into_the_codex_mcp_envelope():
    pending, running, done = _normalize_all(_opencode_updates("tc1", READ_REPORT_CODE, READ_REPORT_CALLS))
    # The wrapper name never reaches a presenter: "execute" reads as a shell.
    assert pending["title"] == "Code Mode"
    assert pending["kind"] == "other" and pending["status"] == "pending" and pending["rawInput"] == {}
    # in_progress: provisional operation from the code, no arguments yet.
    assert running["title"] == "mcp.cortex.read_report"
    assert running["rawInput"] == {"server": "cortex", "tool": "read_report", "code": READ_REPORT_CODE}
    # completed: authoritative operation and arguments from metadata.toolCalls.
    assert done["title"] == "mcp.cortex.read_report"  # terminal evidence is self-contained
    assert done["status"] == "completed"
    assert done["rawInput"] == {
        "server": "cortex", "tool": "read_report", "arguments": {"report_id": "r_abc"},
        "code": READ_REPORT_CODE,
    }
    assert done["rawOutput"] == {
        "result": {"content": [{"type": "text", "text": RESULT_TEXT}]},
        "metadata": {"toolCalls": READ_REPORT_CALLS, "truncated": False},
    }
    # ACP content is kept as the engine sent it (Claude sends it too).
    assert done["content"] == [{"type": "content", "content": {"type": "text", "text": RESULT_TEXT}}]


def test_inner_cortex_error_marks_the_card_failed_with_the_error_text():
    calls = [{"tool": "cortex.answer_submit", "input": {"answer": "x"}, "status": "error"}]
    *_, done = _normalize_all(_opencode_updates(
        "tc1", "return await tools.cortex.answer_submit({answer: 'x'})", calls,
        text="answer_rejected: no open question", error=True))
    assert done["status"] == "failed"
    assert done["rawOutput"]["error"] == "answer_rejected: no open question"
    assert done["rawOutput"]["metadata"]["error"] is True
    line = _worker_tool_details(_human_tool_title("mcp.cortex.answer_submit"), done["status"], None,
                                done["content"], done["rawInput"], done["rawOutput"])
    assert line.startswith("Answer submit — cortex.answer_submit answer=x")
    assert "Error: answer_rejected: no open question" in line and line.endswith("[failed]")


def test_metadata_error_alone_also_fails_the_card():
    calls = [{"tool": "cortex.read_report", "input": {"report_id": "r_x"}, "status": "completed"}]
    *_, done = _normalize_all(_opencode_updates("tc1", READ_REPORT_CODE, calls, error=True))
    assert done["status"] == "failed"


def test_code_mode_search_helper_is_named_by_its_own_tool():
    code = 'return await search({ namespace: "cortex", query: "report", limit: 5 })'
    calls = [{"tool": "search", "input": {"namespace": "cortex", "query": "report", "limit": 5},
              "status": "completed"}]
    pending, running, done = _normalize_all(_opencode_updates("tc1", code, calls))
    assert pending["title"] == "Code Mode"
    # A bare search(...) is not an MCP call: nothing provisional is invented.
    assert "title" not in running and running["rawInput"] == {"code": code}
    assert done["title"] == "search"
    assert done["rawInput"] == {"tool": "search", "arguments": calls[0]["input"], "code": code}
    line = _worker_tool_details(_human_tool_title(done["title"]), done["status"], None, None,
                                done["rawInput"], done["rawOutput"])
    assert line == "search — namespace=cortex, query=report, limit=5"


def test_several_inner_calls_are_named_after_the_first_mcp_call():
    code = 'await search({namespace: "cortex"}); return await tools.cortex.list_reports({})'
    calls = [
        {"tool": "search", "input": {"namespace": "cortex"}, "status": "completed"},
        {"tool": "cortex.list_reports", "input": {}, "status": "completed"},
    ]
    *_, done = _normalize_all(_opencode_updates("tc1", code, calls))
    assert done["rawInput"]["tool"] == "list_reports" and done["rawInput"]["server"] == "cortex"
    # Every inner call stays in OpenCode's own metadata.
    assert done["rawOutput"]["metadata"]["toolCalls"] == calls


def test_other_mcp_servers_use_the_same_spelling_as_codex():
    code = "return await tools.codebase_memory_mcp.search_graph({ name_pattern: 'x' })"
    calls = [{"tool": "codebase_memory_mcp.search_graph", "input": {"name_pattern": "x"}, "status": "completed"}]
    _, running, _ = _normalize_all(_opencode_updates("tc1", code, calls))
    assert running["title"] == "mcp.codebase_memory_mcp.search_graph"


def test_code_without_a_tool_call_stays_a_code_mode_card_not_a_shell():
    updates = _opencode_updates("tc1", "return 1 + 1", [], text="2")
    pending, running, done = _normalize_all(updates)
    assert pending["title"] == "Code Mode"
    assert "title" not in running and running["rawInput"] == {"code": "return 1 + 1"}
    assert done["status"] == "completed"
    assert done["rawOutput"]["result"] == {"content": [{"type": "text", "text": "2"}]}
    line = _worker_tool_details(_human_tool_title("Code Mode"), "in_progress", None, None,
                                running["rawInput"], None)
    assert line == "Code Mode — code=return 1 + 1"


def test_non_code_mode_updates_pass_through_unchanged():
    normalizer = CodeModeToolCalls()
    updates = [
        # OpenCode's own native shell (documented kind execute + rawInput.command).
        dict(sessionUpdate="tool_call", toolCallId="sh", kind="execute", title="execute",
             status="pending", rawInput={"command": "ls"}),
        dict(sessionUpdate="tool_call_update", toolCallId="sh", status="completed", rawOutput={"output": "a"}),
        # Codex and Claude MCP shapes.
        dict(sessionUpdate="tool_call", toolCallId="cx", kind="execute", title="mcp.cortex.read_report",
             status="in_progress", rawInput={"server": "cortex", "tool": "read_report",
                                            "arguments": {"report_id": "r_abc"}}),
        dict(sessionUpdate="tool_call", toolCallId="cl", kind="other", title="mcp__cortex__read_report",
             status="pending", rawInput={"report_id": "r_abc"}),
        dict(sessionUpdate="tool_call_update", toolCallId="cl", status="completed", rawOutput="text",
             content=[{"type": "content", "content": {"type": "text", "text": "text"}}]),
        # No toolCallId at all.
        dict(sessionUpdate="tool_call", kind="other", title="execute"),
    ]
    for update in updates:
        assert normalizer.normalize("s1", update) is update


def test_state_is_scoped_per_wire_session_and_released():
    normalizer = CodeModeToolCalls(max_tracked=2)
    first = _opencode_updates("tc1", READ_REPORT_CODE, READ_REPORT_CALLS)
    normalizer.normalize("s1", first[0])
    # The same toolCallId in another session is not a Code Mode call there.
    later = first[1]
    assert normalizer.normalize("s2", later) is later
    normalizer.clear_session("s1")
    assert normalizer.normalize("s1", later) is later
    for index in range(3):
        normalizer.normalize("s1", dict(first[0], toolCallId=f"b{index}"))
    assert len(normalizer._calls) == 2
    normalizer.normalize("s1", dict(first[2], toolCallId="b2"))
    assert ("s1", "b2") not in normalizer._calls


def _manager(tmp_path, engine):
    return EngineSessionManager(SimpleNamespace(engine=engine, id="cn_test"), str(tmp_path))


def _tool_events(manager, updates, session="wire-1"):
    events = [manager._translate_update(session, update) for update in updates]
    return [event for event in events if event is not None and event.kind == EngineEventKind.TOOL_CALL_OBSERVED]


def test_session_manager_normalizes_only_opencode(tmp_path):
    updates = _opencode_updates("tc1", READ_REPORT_CODE, READ_REPORT_CALLS)
    opencode = _tool_events(_manager(tmp_path, "opencode"), updates)
    assert [event.tool_call_title for event in opencode] == ["Code Mode", "mcp.cortex.read_report", "mcp.cortex.read_report"]
    assert [event.tool_call_status for event in opencode] == ["pending", "in_progress", "completed"]
    assert opencode[-1].tool_call_raw_input["arguments"] == {"report_id": "r_abc"}
    assert opencode[-1].tool_call_raw_output["result"]["content"][0]["text"] == RESULT_TEXT
    assert len({event.tool_call_ref for event in opencode}) == 1

    # The same wire updates from another engine are passed through verbatim.
    for engine in ("codex-acp", "claude-agent-acp"):
        other = _tool_events(_manager(tmp_path, engine), updates)
        assert [event.tool_call_title for event in other] == ["execute", None, None]
        assert other[1].tool_call_raw_input == {"code": READ_REPORT_CODE}
        assert other[-1].tool_call_raw_output == updates[-1]["rawOutput"]


def test_session_manager_keeps_the_compaction_signal_for_every_engine(tmp_path):
    update = dict(sessionUpdate="tool_call", toolCallId="cmp", kind="other", title="execute",
                  status="in_progress", _meta={"contextCompaction": {}})
    for engine in ("opencode", "codex-acp", "claude-agent-acp"):
        event = _manager(tmp_path, engine)._translate_update("wire-1", update)
        assert event.kind == EngineEventKind.COMPACTION_SIGNAL


def _coordinator_cards(calls):
    cards = []
    emit = OrchestratingRuntime.__new__(OrchestratingRuntime)._engine_tool_call_emitter(cards.append)
    for args in calls:
        emit(*args)
    return cards


def _event_args(event):
    return (event.tool_call_ref, event.tool_call_kind, event.tool_call_title, event.tool_call_status,
            event.tool_call_locations, event.tool_call_content, event.tool_call_raw_input,
            event.tool_call_raw_output)


def test_coordinator_card_matches_the_codex_card_for_the_same_operation(tmp_path):
    opencode = _tool_events(_manager(tmp_path, "opencode"),
                            _opencode_updates("tc1", READ_REPORT_CODE, READ_REPORT_CALLS))
    codex = _tool_events(_manager(tmp_path, "codex-acp"), [
        dict(sessionUpdate="tool_call", toolCallId="tc1", kind="execute", title="mcp.cortex.read_report",
             status="in_progress", rawInput={"server": "cortex", "tool": "read_report",
                                            "arguments": {"report_id": "r_abc"}}),
        dict(sessionUpdate="tool_call_update", toolCallId="tc1", status="completed",
             rawOutput={"result": {"content": [{"type": "text", "text": RESULT_TEXT}]}}),
    ])
    opencode_cards = _coordinator_cards([_event_args(event) for event in opencode])
    codex_cards = _coordinator_cards([_event_args(event) for event in codex])

    def merged(cards):
        card = {}
        for update in cards:
            card.update(update)
        return card

    opencode_card, codex_card = merged(opencode_cards), merged(codex_cards)
    assert opencode_card["title"] == codex_card["title"] == "mcp.cortex.read_report"
    assert opencode_card["status"] == codex_card["status"] == "completed"
    for key in ("server", "tool", "arguments"):
        assert opencode_card["raw_input"][key] == codex_card["raw_input"][key]
    assert opencode_card["raw_output"]["result"] == codex_card["raw_output"]["result"]
    assert all("execute" != card.get("title") for card in opencode_cards)


def test_worker_line_matches_the_codex_worker_line(tmp_path):
    opencode = _tool_events(_manager(tmp_path, "opencode"),
                            _opencode_updates("tc1", READ_REPORT_CODE, READ_REPORT_CALLS))
    final = opencode[-1]
    title = _human_tool_title("mcp.cortex.read_report")
    opencode_line = _worker_tool_details(title, final.tool_call_status, None, final.tool_call_content,
                                         final.tool_call_raw_input, final.tool_call_raw_output)
    codex_line = _worker_tool_details(title, "completed", None, None,
                                      {"server": "cortex", "tool": "read_report",
                                       "arguments": {"report_id": "r_abc"}}, None)
    assert opencode_line == codex_line == "Read report — cortex.read_report report_id=r_abc"


def test_engine_failed_wrapper_sets_error_and_preserves_existing_result():
    updates = _opencode_updates("tc1", READ_REPORT_CODE, [], text="JS exception")
    updates[-1]["status"] = "failed"
    _, running, done = _normalize_all(updates)
    assert done["title"] == "mcp.cortex.read_report"
    assert done["status"] == "failed"
    assert done["rawOutput"]["error"] == "JS exception"
    line = _worker_tool_details(_human_tool_title(done["title"]), done["status"], None,
                                done["content"], done.get("rawInput", running["rawInput"]), done["rawOutput"])
    assert "Error: JS exception" in line and line.endswith("[failed]")
    updates[-1]["rawOutput"]["result"] = {"structuredContent": {"diagnostic": "retained"}}
    *_, done = _normalize_all(updates)
    assert done["rawOutput"]["error"] == "JS exception"
    assert done["rawOutput"]["result"] == updates[-1]["rawOutput"]["result"]


def test_completion_observation_keeps_resolved_title(tmp_path):
    from cortex_runtime.delegation.evidence import record_tool_result_observed
    from test_delegation_evidence import _fixture

    store, task, _, worker, _ = _fixture(tmp_path, "opencode-completion-title")
    events = _tool_events(_manager(tmp_path, "opencode"),
                          _opencode_updates("tc1", READ_REPORT_CODE, READ_REPORT_CALLS))
    for event in events:
        record_tool_result_observed(store, task.task_id, worker.attempt_id,
                                    event.tool_call_ref, event.tool_call_kind,
                                    event.tool_call_title, event.tool_call_status)
    with store.connection() as db:
        rows = db.execute(
            "SELECT title, status FROM tool_result_observations "
            "WHERE task_id=? AND attempt_id=? AND tool_call_ref=? ORDER BY rowid",
            (task.task_id, worker.attempt_id, events[0].tool_call_ref),
        ).fetchall()
    assert [dict(row) for row in rows] == [
        {"title": "Code Mode", "status": "pending"},
        {"title": "mcp.cortex.read_report", "status": "in_progress"},
        {"title": "mcp.cortex.read_report", "status": "completed"},
    ]
