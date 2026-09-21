"""The ACP timeline shows compact project and MCP activity."""
from __future__ import annotations

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.acp.orchestrating_runtime import _worker_action_tool_call_id
import cortex_runtime.acp.orchestrating_runtime as orchestrating_runtime


def _emitter(cards):
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    return runtime._engine_tool_call_emitter(cards.append, id_scope="coordinator")


def test_worker_action_identity_includes_attempt_for_fallback_reuse():
    first = _worker_action_tool_call_id("assignment", "attempt-a", "call-1")
    fallback = _worker_action_tool_call_id("assignment", "attempt-b", "call-1")
    assert first != fallback
    assert first.endswith(":attempt-a:call-1")
    assert fallback.endswith(":attempt-b:call-1")


def test_one_logical_call_occupies_one_line_through_all_statuses():
    cards = []
    emit = _emitter(cards)
    emit("read-1", "read", "Read calc.py", "pending", None, None,
         {"path": "calc.py", "limit": 20}, None)
    emit("read-1", None, None, "in_progress")
    emit("read-1", None, None, "completed", None,
         [{"type": "text", "text": "private tool output"}], None,
         {"formatted_output": "private result"})

    assert cards
    assert len({card["tool_call_id"] for card in cards}) == 1
    body = cards[-1]["text"]
    assert body.count("Read calc.py") == 1
    assert body.count("\n") == 0
    assert "calc.py" in body
    assert "private tool output" not in body
    assert "private result" not in body
    assert "pending" not in body and "in_progress" not in body and "completed" not in body
    assert "{" not in body and "}" not in body


def test_generic_terminal_placeholder_is_deferred_until_command_arrives():
    """A provider's provisional Terminal envelope must not become an empty
    coordinator group.  The later command-bearing update creates one useful
    line for the same logical call."""
    cards = []
    emit = _emitter(cards)

    emit("terminal-1", "execute", "Terminal", "pending")
    emit("terminal-1", None, None, "in_progress")
    emit("terminal-1", None, None, "completed")
    assert cards == []

    emit("terminal-1", None, None, "completed", None, None,
         {"command": "python3 -B -m pytest -q"}, None)
    assert len(cards) == 1
    assert cards[0]["text"] == "Command — python3 -B -m pytest -q"
    assert cards[0]["text"].count("Terminal") == 0


def test_consecutive_calls_are_individual_cards():
    cards = []
    emit = _emitter(cards)
    emit("before-1", "read", "Read README", "completed")
    emit("before-2", "execute", "Run tests", "completed")
    assert len(cards) == 2
    first_group = cards[0]["tool_call_id"]
    assert cards[0]["text"] == "Read README"
    assert cards[1]["text"] == "Run tests"
    emit.close_group()
    assert cards[-1]["status"] == "completed"
    emit("after", "edit", "Edit calc.py", "completed")
    assert cards[-1]["tool_call_id"] != first_group
    assert cards[-1]["text"].splitlines() == ["Edit calc.py"]


def test_group_close_preserves_failed_terminal_status():
    cards = []
    emit = _emitter(cards)
    emit("run", "execute", "Run tests", "in_progress")
    emit("run", None, None, "failed", None, None, None, {"error": "hidden"})
    emit.close_group()

    failed_updates = [card for card in cards if card.get("status") == "failed"]
    assert len(failed_updates) == 1
    assert failed_updates[0]["new"] is False
    assert len({card["tool_call_id"] for card in cards}) == 1


def test_failed_group_is_rotated_before_a_later_tool_event():
    cards = []
    emit = _emitter(cards)
    emit("failed-run", "execute", "Run broken check", "failed")
    failed_group = cards[-1]["tool_call_id"]
    emit("later-read", "read", "Read diagnostics", "completed")

    failed_cards = [card for card in cards if card["tool_call_id"] == failed_group]
    assert failed_cards
    assert all("Read diagnostics" not in card.get("text", "") for card in failed_cards)
    assert failed_cards[-1]["status"] == "failed"
    assert cards[-1]["tool_call_id"] != failed_group
    assert cards[-1]["text"] == "Read diagnostics"


def test_long_input_is_preserved_in_its_single_line():
    cards = []
    emit = _emitter(cards)
    long_value = "unique_input_" + "x" * 2000
    emit("command-1", "execute", "Run command", "completed", None, None,
         {"command": long_value}, None)
    body = cards[-1]["text"]
    assert "unique_input_" in body
    assert len(body) > 2000
    assert not body.endswith("…")
    assert "\n" not in body


def test_command_title_and_input_are_shown_once_with_readable_failure():
    cards = []
    emit = _emitter(cards)
    command = "python3 -B -m pytest tests/test_counter.py " + "--verbose " * 35
    emit("command", "execute", command, "in_progress", None, None,
         {"command": command}, None)
    emit("command", None, None, "failed", None, None, None,
         {"error": "private tool output"})

    line = cards[-1]["text"]
    assert line.startswith("Command — python3 -B -m pytest")
    assert line.count("python3 -B") == 1
    assert line.endswith("[failed]")
    assert len(line) > 80
    assert "private tool output" in line


def test_input_preview_takes_priority_over_file_locations():
    cards = []
    emit = _emitter(cards)
    emit("edit-1", "edit", "Edit file", "completed",
         [{"path": "/project/calc.py"}], None,
         {"path": "calc.py", "new_text": "return quotient"}, None)
    body = cards[-1]["text"]
    # Edit cards use the host's native expandable input fields.  The compact
    # line remains path-oriented and the complete input is retained separately.
    assert body == "Edit file — calc.py"
    assert cards[-1]["raw_input"]["new_text"] == "return quotient"
    assert body.count("\n") == 0

    emit("read-2", "read", "Read file", "completed",
         [{"path": "/project/README.md"}], None, None, None)
    assert "/project/README.md" in cards[-1]["text"]


def test_execute_card_uses_command_summary_and_preserves_structured_content():
    cards = []
    emit = _emitter(cards)
    content = [{"type": "text", "text": "pytest completed"}]
    emit("run", "execute", "Bash", "completed", [{"path": "/project"}], content,
         {"command": "python3 -B -m pytest tests/test_one.py"}, {"exit_code": 0})

    assert cards[-1]["text"] == "Command — python3 -B -m pytest tests/test_one.py"
    assert cards[-1]["content"] == content
    assert cards[-1]["locations"] == [{"path": "/project"}]


def test_edit_card_exposes_native_aliases_only_for_supplied_values():
    cards = []
    emit = _emitter(cards)
    raw_input = {
        "file_path": "calc.py",
        "old_string": "return a",
        "new_string": "return b",
    }
    emit("edit", "edit", "Edit", "completed", [{"path": "calc.py"}],
         [{"type": "diff", "path": "calc.py", "oldText": "return a", "newText": "return b"}],
         raw_input, None)

    assert cards[-1]["raw_input"]["filePath"] == "calc.py"
    assert cards[-1]["raw_input"]["oldString"] == "return a"
    assert cards[-1]["raw_input"]["newString"] == "return b"
    assert "unifiedDiff" not in cards[-1]["raw_input"]
    assert cards[-1]["content"][0]["type"] == "diff"


def test_generic_edit_waits_for_target_and_hides_cortex_draft():
    cards = []
    emit = _emitter(cards)

    # The live ACP adapter can name an edit before it supplies the paths.
    emit("draft", "edit", "Editing files", "in_progress")
    assert cards == []
    emit("draft", None, None, "completed",
         [{"path": "/project/.cortex/draft-reports/d_123.md"}], None, None, None)
    assert cards == []

    emit("project", "edit", "Editing files", "in_progress")
    assert cards == []
    emit("project", None, None, "completed",
         [{"path": "/project/calc.py"}], None, None, None)
    assert len(cards) == 1
    assert cards[-1]["text"] == "Editing files — /project/calc.py"


def test_file_ui_placeholders_wait_for_concrete_updates_and_keep_one_group():
    """Provider UI labels are provisional until a path or useful content
    arrives; a later concrete update must still render the same logical call.
    """
    cards = []
    emit = _emitter(cards)

    for ref, title in (("prepare", "Preparing file…"), ("read", "Read File"),
                       ("edit", "Edit")):
        emit(ref, "other", title, "pending")
        emit(ref, None, None, "in_progress")
        emit(ref, None, None, "completed")
    assert cards == []

    emit("read", None, None, "completed",
         [{"path": "/project/README.md"}], None, None, None)
    emit("edit", None, None, "completed", None, None,
         {"path": "calc.py", "new_text": "return quotient"}, None)
    assert len(cards) == 2
    assert {card["tool_call_id"] for card in cards} == {
        "call:coordinator:read", "call:coordinator:edit",
    }
    assert [card["text"] for card in cards] == [
        "Read File — /project/README.md", "Edit — calc.py",
    ]
    assert cards[-1]["raw_input"]["path"] == "calc.py"
    assert cards[-1]["raw_input"]["filePath"] == "calc.py"
    assert cards[-1]["raw_input"]["new_text"] == "return quotient"


def test_file_placeholder_on_cortex_store_stays_hidden():
    """A later path-bearing update to the runtime store remains bookkeeping.
    It must not turn a deferred generic label into a visible card."""
    cards = []
    emit = _emitter(cards)
    emit("draft", "edit", "Edit", "pending")
    emit("draft", None, None, "completed",
         [{"path": "/project/.cortex/pipeline.md"}], None, None, None)
    assert cards == []


def test_exact_cortex_store_path_is_hidden_but_similar_name_is_visible():
    """The store directory itself is bookkeeping too, without hiding
    ordinary project paths such as ``my.cortex-file``."""
    cards = []
    emit = _emitter(cards)
    emit("store-title", "read", "Read .cortex", "completed")
    emit("store-location", "read", "Read File", "completed",
         [{"path": "/project/.cortex"}], None, None, None)
    assert cards == []

    emit("similar", "read", "Read my.cortex-file", "completed")
    assert cards[-1]["text"] == "Read my.cortex-file"


def test_internal_engine_calls_are_hidden_but_project_and_mcp_failures_remain():
    cards = []
    emit = _emitter(cards)
    for ref, kind, title in (
        ("wait", "other", "wait"),
        ("await", "other", "await"),
        ("scheduler", "other", "Task scheduler"),
        ("wakeup", "other", "ScheduleWakeup"),
        ("thinking", "think", "Thinking"),
        ("search", "search", "ToolSearch"),
        ("review", "other", "Guardian Review"),
        ("cortex", "other", "mcp__cortex__write_report"),
        ("cortex-dot", "other", "mcp.cortex.decide"),
    ):
        emit(ref, kind, title, "completed")
    assert cards == []

    emit("project", "edit", "Edit calc.py", "failed", None, None,
         {"path": "calc.py"}, {"error": "private failure detail"})
    emit("mcp", "other", "mcp__codebase_memory__search_code", "failed", None, None,
         {"query": "divide by zero"}, {"error": "private MCP failure detail"})
    body = "\n".join(card["text"] for card in cards)
    assert len(body.splitlines()) == 4
    assert "Edit calc.py" in body
    assert "mcp__codebase_memory__search_code" in body
    assert "query=divide by zero" in body
    assert body.count("failed") == 2
    assert "Error: private failure detail" in body
    assert "Error: private MCP failure detail" in body


def test_unknown_untitled_call_waits_for_a_name_and_preserves_its_input():
    cards = []
    emit = _emitter(cards)
    emit("read-1", "read", None, "pending", None, None, {"path": "README.md"}, None)
    assert cards == []
    emit("read-1", "read", "Read README", "completed")
    assert cards
    assert cards[-1]["text"].count("Read README") == 1
    assert "README.md" in cards[-1]["text"]


def test_titleless_status_updates_retain_first_command_until_named():
    cards = []
    emit = _emitter(cards)
    emit("run-1", "execute", None, "pending", None, None,
         {"command": "python3 -B -m pytest -q"}, None)
    emit("run-1", None, None, "in_progress", None, None, None, None)
    assert cards == []
    emit("run-1", "execute", "Run command", "completed", None, None, None, None)

    assert cards
    assert len({card["tool_call_id"] for card in cards}) == 1
    assert cards[-1]["text"] == "Command — python3 -B -m pytest -q"


def test_titleless_failure_stays_failed_after_late_name():
    cards = []
    emit = _emitter(cards)
    emit("run-2", "execute", None, "pending", None, None,
         {"command": "python3 -B -m pytest -q"}, None)
    emit("run-2", None, None, "failed", None, None, None,
         {"error": "private failure output"})
    emit("run-2", "execute", "Run command", "completed", None, None, None, None)

    assert cards
    assert len({card["tool_call_id"] for card in cards}) == 1
    assert cards[-1]["text"].startswith("Command — python3 -B -m pytest -q")
    assert "Error: private failure output" in cards[-1]["text"]


def test_oversized_group_rotates_without_losing_previous_calls(monkeypatch):
    cards = []
    monkeypatch.setattr(orchestrating_runtime, "_COORDINATOR_ACTION_LOG_MAX_CHARS", 90)
    emit = _emitter(cards)
    emit("first", "execute", "First action with a long description", "completed")
    emit("second", "execute", "Second action with a long description", "completed")
    emit("third", "execute", "Third action with a long description", "completed")
    assert "First action" in " ".join(card.get("text", "") for card in cards)
    assert "Third action" in cards[-1]["text"]
    assert len({card["tool_call_id"] for card in cards}) >= 2
