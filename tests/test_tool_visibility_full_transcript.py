"""Focused no-paid coverage for Paseo tool-card visibility."""
from __future__ import annotations

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
from cortex_runtime.acp import protocol
from cortex_runtime.acp.replay_store import append_host_update, replay_session
from cortex_runtime.acp.sessions import create_session
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
import uuid
import json
import pytest


def test_coordinator_cortex_tools_have_human_titles_and_full_readable_input():
    cards = []
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    emit = runtime._engine_tool_call_emitter(cards.append, id_scope="coordinator")
    skill_name = "knowledge-harvest-" + ("x" * 5000)
    emit(
        "skill", "other", "mcp__cortex__read_skill", "completed", None, None,
        {"skill": skill_name}, None,
    )
    assert cards == []


def test_native_provider_skill_remains_visible():
    cards = []
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    emit = runtime._engine_tool_call_emitter(cards.append, id_scope="coordinator")
    emit("skill", "other", "Skill", "completed", None, None,
         {"skill": "knowledge-harvest"}, None)
    assert len(cards) == 1
    assert cards[0]["title"] == "Skill"


@pytest.mark.parametrize(
    "raw_input, expected",
    [
        # Codex native file tool shape.
        ({"filePath": "/project/src/calc.py", "lineStart": 12, "lineEnd": 18}, "Read — src/calc.py:12-18"),
        # Claude native file tool shape.
        ({"file_path": "/project/src/parse.py"}, "Read — src/parse.py"),
        # Cortex MCP reads are runtime bookkeeping and are hidden.
        ({"server": "cortex", "tool": "read_report", "arguments": {"report_id": "r-17"}}, None),
        ({"server": "cortex", "tool": "read_skill", "arguments": {"skill": "knowledge-harvest"}}, None),
    ],
)
def test_read_collapsed_card_names_codex_claude_and_mcp_shapes(raw_input, expected):
    cards = []
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    emit = runtime._engine_tool_call_emitter(
        cards.append, id_scope="coordinator", project_root="/project",
    )
    title = "Read" if "server" not in raw_input else f"mcp.cortex.{raw_input['tool']}"
    emit("read", "read", title, "completed", None, None, raw_input, {"content": "bounded"})
    if expected is None:
        assert cards == []
        return
    assert cards[-1]["text"] == expected
    # Expansion still receives the complete redacted provider payload.
    assert cards[-1]["raw_input"] == raw_input
    assert cards[-1]["raw_output"] == {"content": "bounded"}


def test_command_backed_read_uses_read_path_only_for_a_single_cat():
    cards = []
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    emit = runtime._engine_tool_call_emitter(
        cards.append, id_scope="coordinator", project_root="/project",
    )

    simple = {"command": "cat /project/README.md 2>/dev/null"}
    emit("read-simple", "read", "Read", "completed", None, None, simple, None)
    assert cards[-1]["text"] == "Read — README.md"
    assert cards[-1]["raw_input"] == simple

    compound = {"command": "cat /project/README.md 2>/dev/null; echo ---; ls -la /project"}
    emit.close_group()
    emit("read-compound", "read", "Read", "completed", None, None, compound, None)
    assert cards[-1]["text"].startswith("Command — cat /project/README.md")
    assert "; echo ---; ls -la /project" in cards[-1]["text"]
    assert cards[-1]["raw_input"] == compound


def test_failed_coordinator_call_keeps_redacted_error_without_json_wall():
    cards = []
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    emit = runtime._engine_tool_call_emitter(cards.append, id_scope="coordinator")
    emit(
        "report", "other", "mcp__cortex__write_report", "failed", None, None,
        {"title": "Release note"},
        {"error": "write rejected", "token": "sk-live-secret"},
    )
    assert cards == []


def test_coordinator_expanded_io_survives_redaction_and_session_replay(tmp_path):
    """Collapsed text stays concise while Paseo's expanded fields retain the
    complete structured call/result and the replay returns the same update."""
    cards = []
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    emit = runtime._engine_tool_call_emitter(cards.append, id_scope="coordinator")
    raw_input = {
        "request_key": "rk-expanded",
        "arguments": {"title": "A" * 5000, "path": "/project/private.txt"},
        "token": "super-secret",
    }
    raw_output = {
        "ok": False,
        "error": {"code": "write_rejected", "message": "permission denied"},
        "items": list(range(1100)),
    }
    emit("write", "other", "mcp__codebase_memory__write_report", "failed",
         [{"path": "/project/private.txt"}],
         [{"type": "diff", "path": "private.txt", "oldText": "a", "newText": "b"}],
         raw_input, raw_output)

    card = cards[-1]
    assert card["raw_input"]["request_key"] == "rk-expanded"
    assert len(card["raw_input"]["arguments"]["title"]) == 5000
    assert card["raw_input"]["arguments"]["path"] == "/project/private.txt"
    assert card["raw_input"]["token"] == "[REDACTED]"
    assert card["raw_output"]["error"] == raw_output["error"]
    assert card["raw_output"]["items"] == raw_output["items"]
    assert card["content"][0]["type"] == "diff"
    assert card["locations"][0]["path"] == "/project/private.txt"

    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    create_session(store, "session-io", str(tmp_path), {}, {}, [])
    update = protocol.tool_call(
        "session-io", card["tool_call_id"], card["title"], card["kind"],
        card["status"], text=card["text"], raw_input=card["raw_input"],
        raw_output=card["raw_output"], content=card["content"],
        locations=card["locations"],
    )["params"]["update"]
    append_host_update(store, "session-io", update)
    replayed = replay_session(store, "session-io")
    assert replayed == [{"kind": "update", "update": update}]


def test_delegate_is_represented_only_by_worker_card_in_coordinator_stream():
    cards = []
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    emit = runtime._engine_tool_call_emitter(cards.append, id_scope="coordinator")
    emit("delegate", "other", "mcp.cortex.delegate", "completed", None, None,
         {"title": "Worker task"}, {"assignment_ref": "a1"})
    emit("read", "other", "Skill", "completed", None, None,
         {"skill": "knowledge-harvest"}, {"text": "ok"})
    assert [card["tool_call_id"] for card in cards] == ["call:coordinator:read"]


def test_worker_lifecycle_card_contains_full_ordered_actions_without_worker_siblings():
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    import threading
    runtime._lock = threading.RLock()
    runtime._worker_card_terminal = {}
    runtime._worker_card_project_root = {}
    runtime._worker_action_log = {}
    runtime._worker_action_refs = {}
    runtime._worker_card_emitters = {}
    runtime._worker_card_sent = {}
    runtime._emitted_worker_cards = set()
    cards = []
    long_command = "python3 -B -m pytest " + ("tests/test_long.py " * 500)
    runtime._emit_worker_card(
        cards.append, "assignment-1", title="Worker: backend-dev — Repair",
        kind="other", status="pending",
    )
    runtime._emit_worker_card(
        cards.append, "assignment-1", status="in_progress",
        entry="Tool: Command — " + long_command, entry_ref="worker-tool:run",
    )
    runtime._emit_worker_card(
        cards.append, "assignment-1", status="in_progress",
        entry="Error: Command failed: " + "permission denied " * 400,
        entry_ref="worker-tool-error:run",
    )
    runtime._emit_worker_card(
        cards.append, "assignment-1", status="completed",
        entry="[final response] " + "verified " * 1000,
        entry_ref="worker-final",
    )
    assert cards
    lifecycle = [card for card in cards if card["tool_call_id"] == "worker:assignment-1"]
    assert lifecycle
    body = lifecycle[-1]["text"]
    assert body.index("Tool: Command") < body.index("Error: Command failed") < body.index("[final response]")
    assert len(body) > 10000
    assert not any(card["tool_call_id"].startswith("worker-action:") for card in cards)
    assert "permission denied" in body


def test_paseo_surface_keeps_history_durable_but_shows_final_and_panel_hint():
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    import threading
    runtime._lock = threading.RLock()
    runtime._worker_card_terminal = {}
    runtime._worker_card_project_root = {}
    runtime._worker_action_log = {}
    runtime._worker_action_refs = {}
    runtime._worker_card_emitters = {}
    runtime._worker_card_sent = {}
    runtime._emitted_worker_cards = set()
    runtime._worker_panel_mode = {"assignment-paseo": True}
    cards = []
    runtime._emit_worker_card(cards.append, "assignment-paseo", status="pending", title="Worker: explorer — Inspect")
    runtime._emit_worker_card(cards.append, "assignment-paseo", status="in_progress",
                              entry="Tool: Read skill — " + "knowledge-harvest " * 500,
                              entry_ref="worker-tool:skill")
    runtime._emit_worker_card(cards.append, "assignment-paseo", status="in_progress",
                              entry="[final response] Verified the result.", entry_ref="worker-final")
    runtime._emit_worker_card(cards.append, "assignment-paseo", status="completed")
    body = cards[-1]["text"]
    assert body == "[final response] Verified the result.\nOpen Workers in the header for full history"
    assert "knowledge-harvest" in "\n".join(runtime._worker_action_log["assignment-paseo"])
    assert "knowledge-harvest" not in body


def test_non_paseo_surface_keeps_full_worker_card_transcript():
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    import threading
    runtime._lock = threading.RLock()
    runtime._worker_card_terminal = {}
    runtime._worker_card_project_root = {}
    runtime._worker_action_log = {}
    runtime._worker_action_refs = {}
    runtime._worker_card_emitters = {}
    runtime._worker_card_sent = {}
    runtime._emitted_worker_cards = set()
    runtime._worker_panel_mode = {"assignment-zed": False}
    cards = []
    runtime._emit_worker_card(cards.append, "assignment-zed", status="pending")
    runtime._emit_worker_card(cards.append, "assignment-zed", status="completed",
                              entry="[final response] Full worker finding", entry_ref="worker-final")
    assert "Full worker finding" in cards[-1]["text"]
    assert "Open Workers" not in cards[-1]["text"]


def test_worker_activity_is_durable_nonmeaningful_panel_history(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(
        store, SourceEvent(origin="test", event_id=str(uuid.uuid4())), "Inspect.", "owner",
    )
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    long_text = "Tool: Read skill — " + ("knowledge-harvest " * 1000)
    runtime._record_worker_activity(task.task_id, "assignment", "attempt-1", "tool:read", long_text)
    with store.connection() as db:
        row = db.execute(
            "SELECT event_type, assignment_ref, payload_json, consumed_by FROM task_events "
            "WHERE task_id=? AND dedup_key LIKE ?",
            (task.task_id, "worker_activity:attempt-1:tool:read:%"),
        ).fetchone()
    assert row is not None
    assert row["event_type"] == "progress"
    assert row["assignment_ref"] == "attempt-1"
    assert "knowledge-harvest" in row["payload_json"]
    assert len(row["payload_json"]) > 12000
    assert row["consumed_by"] is None

    runtime._record_worker_activity(task.task_id, "assignment", "attempt-1", "tool:read", "Tool: Read skill — done", status="completed")
    runtime._record_worker_activity(task.task_id, "assignment", "attempt-1", "tool:read", "Tool: Read skill — done", status="completed")
    with store.connection() as db:
        rows = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='progress' "
            "AND assignment_ref=? ORDER BY id",
            (task.task_id, "attempt-1"),
        ).fetchall()
    assert len(rows) == 2
    assert "completed" in rows[-1]["payload_json"]


def test_worker_message_activity_keeps_identical_chunks_and_replays_by_sequence(tmp_path):
    """Equal text is not an event identity for a streamed worker message."""
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(
        store, SourceEvent(origin="test", event_id=str(uuid.uuid4())), "Inspect.", "owner",
    )
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store

    for sequence, chunk in enumerate((" ", "word", "word"), start=1):
        runtime._record_worker_activity(
            task.task_id, "assignment", "attempt-stream", "message",
            "[worker message] " + chunk, stream_sequence=sequence,
        )
    # Replaying provider sequence 2 is idempotent, while the distinct equal
    # chunk at sequence 3 remains a separate durable row.
    runtime._record_worker_activity(
        task.task_id, "assignment", "attempt-stream", "message",
        "[worker message] word", stream_sequence=2,
    )

    with store.connection() as db:
        rows = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='progress' ORDER BY id",
            (task.task_id, "attempt-stream"),
        ).fetchall()
    payloads = [json.loads(row["payload_json"]) for row in rows]
    assert [payload["stream_sequence"] for payload in payloads] == [1, 2, 3]
    assert [payload["text"] for payload in payloads] == [
        "[worker message]  ", "[worker message] word", "[worker message] word",
    ]


def test_worker_message_activity_sequences_are_isolated_between_attempts(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(
        store, SourceEvent(origin="test", event_id=str(uuid.uuid4())), "Inspect.", "owner",
    )
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store

    for attempt in ("attempt-a", "attempt-b"):
        runtime._record_worker_activity(
            task.task_id, "assignment", attempt, "message", "[worker message] same",
            stream_sequence=1,
        )

    with store.connection() as db:
        rows = db.execute(
            "SELECT DISTINCT assignment_ref FROM task_events WHERE task_id=? "
            "AND event_type='progress' ORDER BY assignment_ref",
            (task.task_id,),
        ).fetchall()
    assert [row["assignment_ref"] for row in rows] == ["attempt-a", "attempt-b"]


def test_worker_activity_provider_event_identity_is_replay_idempotent(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(
        store, SourceEvent(origin="test", event_id=str(uuid.uuid4())), "Inspect.", "owner",
    )
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store

    kwargs = dict(
        task_id=task.task_id, assignment_ref="assignment", attempt_id="attempt-provider",
        activity_ref="message", text="[worker message] repeated", stream_sequence=7,
        provider_event_id="provider-event-7",
    )
    runtime._record_worker_activity(**kwargs)
    runtime._record_worker_activity(**kwargs)

    with store.connection() as db:
        count = db.execute(
            "SELECT COUNT(*) FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='progress'",
            (task.task_id, "attempt-provider"),
        ).fetchone()[0]
    assert count == 1


def test_worker_activity_keeps_full_redacted_acp_tool_io_across_status_updates(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    task = bootstrap_task(
        store, SourceEvent(origin="test", event_id=str(uuid.uuid4())), "Inspect.", "owner",
    )
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    raw_input = {"command": "line 1\n" + ("input-value " * 700)}
    raw_output = {"stdout": "line A\n" + ("output-value " * 700)}
    runtime._record_worker_activity(
        task.task_id, "assignment", "attempt-io", "tool:run", "Tool: Run",
        status="in_progress", input_value=raw_input, output_value=None,
        content_value=[{"type": "text", "text": "content\n" + ("x" * 5000)}],
    )
    runtime._record_worker_activity(
        task.task_id, "assignment", "attempt-io", "tool:run", "Tool: Run",
        status="completed", input_value=raw_input, output_value=raw_output,
        content_value=[{"type": "text", "text": "content\n" + ("x" * 5000)}],
    )
    # Exact replay of the completed update is deduplicated by its envelope.
    runtime._record_worker_activity(
        task.task_id, "assignment", "attempt-io", "tool:run", "Tool: Run",
        status="completed", input_value=raw_input, output_value=raw_output,
        content_value=[{"type": "text", "text": "content\n" + ("x" * 5000)}],
    )
    with store.connection() as db:
        rows = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='progress' ORDER BY id", (task.task_id, "attempt-io"),
        ).fetchall()
    assert len(rows) == 2
    first, second = (json.loads(row["payload_json"]) for row in rows)
    assert first["raw_input"] == raw_input
    assert first["content"][0]["text"] == "content\n" + ("x" * 5000)
    assert second["raw_output"] == raw_output
    assert second["status"] == "completed"
