"""No-paid bounds for what Cortex streams to an ACP host about its workers.

Paseo 0.9.2 keeps every provider event of a session in memory, so the bytes
Cortex sends are the bytes the daemon retains.  A worker card that re-sent
its whole growing transcript on every tool event and streamed word grew
quadratically and exhausted the daemon heap.  These tests pin the bounded
contract: the Paseo card carries no transcript body, other clients receive a
small rolling tail, body-only redraws are rate limited, status changes are
immediate, and every outbound ``session/update`` has a hard size cap.  The
durable Workers-panel history keeps full detail.
"""
from __future__ import annotations

import json
import threading

import pytest

from cortex_runtime.acp import orchestrating_runtime as runtime_module
from cortex_runtime.acp import protocol
from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.acp.replay_store import append_host_update, replay_session
from cortex_runtime.acp.sessions import create_session
from cortex_runtime.acp.wire_budget import SESSION_UPDATE_MAX_BYTES, cap_session_update
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.store import Store


def _update_bytes(update) -> int:
    return len(json.dumps(update, ensure_ascii=False, separators=(",", ":")).encode("utf-8"))


# -- End-to-end through the real Agent, pipe transport and runtime ----------

def _worker_router(*, tools: int, words_per_block: int, write_every: int, write_bytes: int):
    from runtime_scenarios import propose_inspection, publish_result
    from cortex_runtime.delegation import mcp_tools

    phase = {"delegated": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if not phase["delegated"]:
            phase["delegated"] = True
            propose_inspection(store, channel)
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        context = mcp_tools.dispatch(store, channel, "task_get_context", {})
        if context["state"] == "RECONCILING":
            mcp_tools.dispatch(store, channel, "synthesis_submit", dict(text="Checked."))
        return PromptOutcome(StopReason.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index, *, on_delta, on_tool_call):
        with store.connection() as db:
            role = db.execute(
                "SELECT role_id FROM delegation_provisioning WHERE attempt_id=?", (channel.attempt_id,),
            ).fetchone()[0]
        if role == "worker-code-reviewer" or call_index:
            return publish_result(store, channel)
        on_delta("Interim narration before first tool. ")
        for index in range(tools):
            ref = f"read-{index}"
            on_tool_call(ref, "read", "Read", "in_progress", None, None,
                         {"path": f"/fixture/src/module_{index:04d}.py"}, None)
            on_tool_call(ref, "read", "Read", "completed", None, None, None, {"content": "ok"})
            if write_every and index % write_every == 0:
                on_tool_call(
                    f"write-{index}", "other", "Project write", "completed", None, None,
                    {"server": "cortex", "tool": "project_write",
                     "arguments": {"path": "src/generated.py", "content": "code words " * (write_bytes // 11)}},
                    None,
                )
            if index % 5 == 4:
                for word in range(words_per_block):
                    on_delta(f"word{word} ")
        publish_result(store, channel)
        return PromptOutcome(StopReason.END_TURN, "Worker finished: " + "done " * 40)

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else worker_behavior

    return router


def run_worker_session(tmp_path, *, client_name: str, tools: int, words_per_block: int = 60,
                       write_every: int = 0, write_bytes: int = 0):
    """Run one delegated worker through the real Agent and return every
    ``session/update`` the host received plus the harness (for durable reads)."""
    from test_acp_elicitation import _Harness, _project

    tmp_path.mkdir(parents=True, exist_ok=True)
    harness = _Harness(tmp_path, _worker_router(
        tools=tools, words_per_block=words_per_block, write_every=write_every, write_bytes=write_bytes,
    ))
    harness.start()
    try:
        init_id = harness.send("initialize", dict(
            protocolVersion=1, clientCapabilities={}, clientInfo=dict(name=client_name, version="test"),
        ))
        harness.response_for(init_id)
        new_id = harness.send("session/new", dict(cwd=str(_project(tmp_path)), mcpServers=[]))
        session_id = harness.response_for(new_id)["result"]["sessionId"]
        prompt_id = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Inspect it.")]))
        harness.response_for(prompt_id, timeout=60.0)
        # The worker runs in the background; wait for its terminal card.
        harness.wait_for(lambda m: m.get("method") == "session/update"
                         and str(m["params"]["update"].get("toolCallId", "")).startswith("worker:")
                         and m["params"]["update"].get("status") in {"completed", "failed"},
                         timeout=60.0)
        with harness.stdout._lock:
            updates = [m["params"]["update"] for m in harness.stdout.lines
                       if m.get("method") == "session/update"]
        return updates, session_id, harness
    finally:
        harness.stop()


def _worker_updates(updates):
    return [u for u in updates if str(u.get("toolCallId", "")).startswith("worker:")]


def _stats(updates):
    worker = _worker_updates(updates)
    sizes = [_update_bytes(u) for u in worker]
    return dict(count=len(sizes), total=sum(sizes), largest=max(sizes, default=0))


@pytest.mark.parametrize("client_name", ["paseo", "zed"])
def test_worker_card_bytes_stay_small_and_linear_as_worker_grows(tmp_path, client_name):
    small, _session, _ = run_worker_session(
        tmp_path / "small", client_name=client_name, tools=40, write_every=10, write_bytes=20_000,
    )
    large, _session, _ = run_worker_session(
        tmp_path / "large", client_name=client_name, tools=80, write_every=10, write_bytes=20_000,
    )
    small_stats, large_stats = _stats(small), _stats(large)
    assert small_stats["count"] and large_stats["count"]
    # The old cumulative card grew ~4x per doubling (171 MB at 80 tools with
    # these writes).  The bounded card is at most linear, and small.
    assert large_stats["total"] <= 2.2 * small_stats["total"] + 8 * 1024, (small_stats, large_stats)
    assert large_stats["largest"] <= 16 * 1024 + 1024
    if client_name == "paseo":
        assert large_stats["total"] <= 24 * 1024, large_stats
    else:
        assert large_stats["total"] <= 1024 * 1024, large_stats


def test_paseo_worker_card_has_no_transcript_body_but_panel_history_is_full(tmp_path):
    updates, _session, harness = run_worker_session(
        tmp_path, client_name="paseo", tools=30, write_every=10, write_bytes=20_000,
    )
    worker = _worker_updates(updates)
    assert worker
    for update in worker:
        assert "rawInput" not in update, update
        assert "content" not in update, update
    terminal = [u for u in worker if u.get("status") in {"completed", "failed"}]
    assert terminal and terminal[-1]["status"] == "completed"
    assert "summary" in terminal[-1]["rawOutput"]
    # Paseo shows no per-token or per-tool body redraws.
    assert len(worker) <= 8, [u.get("status") for u in worker]
    # The durable Workers-panel history still holds every tool call in full.
    with harness.store.connection() as db:
        payloads = [json.loads(row[0]) for row in db.execute(
            "SELECT payload_json FROM task_events WHERE event_type='progress' "
            "AND payload_json LIKE '%worker_activity%'")]
    writes = [p for p in payloads if str(p.get("activity_ref", "")).startswith("tool:write-")]
    assert writes
    assert len(writes[0]["raw_input"]["arguments"]["content"]) >= 19_000
    tools = {p["activity_ref"] for p in payloads if str(p.get("activity_ref", "")).startswith("tool:read-")}
    assert len(tools) == 30


def test_worker_interim_prose_is_not_published_to_card_or_history(tmp_path):
    updates, _session, harness = run_worker_session(
        tmp_path, client_name="zed", tools=10, words_per_block=3,
    )
    with harness.store.connection() as db:
        payloads = [json.loads(row[0]) for row in db.execute(
            "SELECT payload_json FROM task_events WHERE event_type='progress' "
            "AND payload_json LIKE '%worker_activity%'"
        )]
    assert not any("[worker message]" in str(item.get("text", "")) for item in payloads)
    assert not any("Interim narration before first tool" in str(item.get("text", "")) for item in payloads)
    assert not any("word0 word1 word2" in str(item.get("text", "")) for item in payloads)
    assert any("[final response] Worker finished:" in str(item.get("text", "")) for item in payloads)
    assert not any("[worker message]" in json.dumps(item) for item in _worker_updates(updates))
    assert not any("Interim narration before first tool" in json.dumps(item) for item in _worker_updates(updates))
    assert not any("word0 word1 word2" in json.dumps(item) for item in _worker_updates(updates))


def test_generic_worker_card_body_is_a_bounded_rolling_tail(tmp_path):
    updates, _session, _ = run_worker_session(
        tmp_path, client_name="zed", tools=60, write_every=10, write_bytes=50_000,
    )
    worker = _worker_updates(updates)
    for update in worker:
        assert "rawInput" not in update
        for block in update.get("content") or ():
            text = block["content"]["text"]
            assert len(text.encode("utf-8")) <= runtime_module._WORKER_CARD_BODY_MAX_BYTES
            assert len(text.splitlines()) <= runtime_module._WORKER_CARD_TAIL_ENTRIES + 1
    final_body = [u for u in worker if u.get("content")][-1]["content"][0]["content"]["text"]
    assert "earlier actions in task history" in final_body
    assert "module_0000.py" not in final_body
    assert "code words code words" not in final_body


def test_session_load_replays_the_same_bounded_worker_cards(tmp_path):
    updates, session_id, harness = run_worker_session(
        tmp_path, client_name="paseo", tools=20, write_every=5, write_bytes=20_000,
    )
    replayed = replay_session(harness.store, session_id)
    cards = [entry["update"] for entry in replayed if entry["kind"] == "update"
             and str(entry["update"].get("toolCallId", "")).startswith("worker:")]
    assert cards
    for card in cards:
        assert _update_bytes(card) <= 16 * 1024 + 1024
        assert "rawInput" not in card
    live_terminal = [u for u in _worker_updates(updates) if u.get("status") == "completed"][-1]
    replay_terminal = [c for c in cards if c.get("status") == "completed"][-1]
    assert replay_terminal["rawOutput"] == live_terminal["rawOutput"]


# -- Unit level: the emission contract of _emit_worker_card ------------------

class _Clock:
    def __init__(self):
        self.now = 1000.0

    def __call__(self):
        return self.now


def _card_runtime(*, paseo: bool):
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._lock = threading.RLock()
    runtime._worker_action_log = {}
    runtime._worker_action_refs = {}
    runtime._worker_card_sent = {}
    runtime._worker_card_terminal = {}
    runtime._emitted_worker_cards = set()
    runtime._worker_card_panel_mode = {"a1": paseo}
    runtime._worker_card_clock = _Clock()
    return runtime


def test_worker_body_updates_rate_limited_but_status_immediate():
    runtime = _card_runtime(paseo=False)
    clock = runtime._worker_card_clock
    cards = []
    runtime._emit_worker_card(cards.append, "a1", status="pending", title="Worker: explorer — Inspect")
    runtime._emit_worker_card(cards.append, "a1", status="in_progress", entry="Tool: Read — a.py", entry_ref="t:a")
    runtime._emit_worker_card(cards.append, "a1", status="in_progress", entry="Tool: Read — b.py", entry_ref="t:b")
    sent = len(cards)
    # Body-only changes inside the progress window are held back ...
    for index in range(50):
        clock.now += 0.1
        runtime._emit_worker_card(cards.append, "a1", status="in_progress",
                                  entry=f"Tool: Read — f{index}.py", entry_ref=f"t:{index}")
    assert len(cards) == sent
    # ... and the next body change after the window carries the latest tail.
    clock.now += runtime_module._WORKER_CARD_PROGRESS_MIN_INTERVAL_SECONDS
    runtime._emit_worker_card(cards.append, "a1", status="in_progress")
    assert len(cards) == sent + 1
    assert "f49.py" in cards[-1]["text"]
    # A terminal transition is never delayed, even inside the window.
    clock.now += 0.01
    runtime._emit_worker_card(cards.append, "a1", status="completed",
                              result_payload={"summary": "done", "checks": []}, entry="Result: done")
    assert cards[-1]["status"] == "completed"
    assert cards[-1]["text"].endswith("Result: done")
    assert cards[-1]["raw_output"]["summary"] == "done"


@pytest.mark.parametrize("paseo", [True, False])
def test_worker_card_event_count_is_a_constant_whatever_the_worker_does(paseo):
    """One create, one start, one terminal plus a fixed progress budget,
    for 10 or 10,000 tool events over minutes or hours."""
    counts = {}
    for events, step in ((10, 1.0), (10_000, 0.5)):
        runtime = _card_runtime(paseo=paseo)
        clock = runtime._worker_card_clock
        cards = []
        runtime._emit_worker_card(cards.append, "a1", status="pending", title="Worker: explorer — Inspect")
        runtime._emit_worker_card(cards.append, "a1", status="in_progress")
        progress_times = []
        for index in range(events):
            clock.now += step
            before = len(cards)
            runtime._emit_worker_card(cards.append, "a1", status="in_progress",
                                      entry=f"Tool: Read — f{index}.py", entry_ref=f"t:{index}")
            if len(cards) > before:
                progress_times.append(clock.now)
        runtime._emit_worker_card(cards.append, "a1", status="completed",
                                  result_payload={"summary": "done", "checks": []}, entry="Result: done")
        statuses = [card.get("status") for card in cards if card.get("status")]
        assert statuses == ["pending", "in_progress", "completed"]
        assert len(progress_times) <= runtime_module._WORKER_CARD_PROGRESS_MAX_UPDATES
        assert all(later - earlier >= runtime_module._WORKER_CARD_PROGRESS_MIN_INTERVAL_SECONDS
                   for earlier, later in zip(progress_times, progress_times[1:]))
        if not paseo:
            assert cards[-1]["text"].endswith("Result: done")
        counts[events] = len(cards)
    bound = 3 + runtime_module._WORKER_CARD_PROGRESS_MAX_UPDATES
    assert counts[10] <= bound and counts[10_000] <= bound, counts


def test_long_title_from_two_paths_is_one_title():
    """The live regression: a >300-character title reached the card clipped
    from the activity tick and unclipped from the resolved title, and the two
    alternated on every tool event (117 title-only updates for one worker)."""
    runtime = _card_runtime(paseo=True)
    title = "Worker: technical-writer · codex · 6 Luna · high — " + "Write the notes " * 30
    runtime._worker_card_title = {"a1": title}
    cards = []
    runtime._emit_worker_card(cards.append, "a1", status="pending", title=title)
    runtime._emit_worker_card(cards.append, "a1", status="in_progress")
    for index in range(100):
        runtime._emit_worker_card(cards.append, "a1", status="in_progress",
                                  title=runtime_module._compact_text(title, 300))
        runtime._emit_worker_card(cards.append, "a1", status="in_progress",
                                  entry=f"Tool: Read — f{index}.py", entry_ref=f"t:{index}")
    assert len(cards) == 2, [sorted(card) for card in cards]
    assert len(cards[0]["title"]) <= runtime_module._WORKER_CARD_TITLE_MAX_CHARS


def test_outcome_suffix_survives_title_bound():
    title = "Worker: explorer — " + "long " * 100
    for suffix in ("Cancelled", "Interrupted", "Completed with issues"):
        rendered = runtime_module._worker_card_title(title, suffix)
        assert rendered.endswith(" · " + suffix)
        assert len(rendered) <= runtime_module._WORKER_CARD_TITLE_MAX_CHARS
        assert runtime_module._worker_card_title(rendered) == rendered


def test_stale_pending_never_regresses_a_started_card():
    runtime = _card_runtime(paseo=True)
    cards = []
    runtime._emit_worker_card(cards.append, "a1", status="in_progress", title="Worker: explorer — Inspect")
    runtime._emit_worker_card(cards.append, "a1", status="pending", title="Worker: explorer — Inspect")
    assert [card.get("status") for card in cards] == ["in_progress"]


def test_settled_card_refinement_is_immediate_and_bounded():
    runtime = _card_runtime(paseo=True)
    cards = []
    runtime._emit_worker_card(cards.append, "a1", status="pending", title="Worker: explorer — Inspect")
    runtime._emit_worker_card(cards.append, "a1", status="completed",
                              result_payload={"summary": "done", "checks": []})
    runtime._emit_worker_card(cards.append, "a1", status="completed",
                              title="Worker: explorer — Inspect · Completed with issues")
    assert cards[-1]["title"].endswith("Completed with issues")
    for index in range(20):
        runtime._emit_worker_card(cards.append, "a1", status="completed", title=f"Worker · {index}")
    assert len(cards) == 3 + runtime_module._WORKER_CARD_TERMINAL_REVISIONS - 1


def test_title_change_is_sent_immediately_inside_the_rate_window():
    runtime = _card_runtime(paseo=True)
    cards = []
    runtime._emit_worker_card(cards.append, "a1", status="pending", title="Worker: explorer — Inspect")
    runtime._emit_worker_card(cards.append, "a1", status="in_progress", entry="Tool: Read — a.py")
    runtime._emit_worker_card(cards.append, "a1", status="in_progress", title="Worker: explorer — Inspect · retry")
    assert [card.get("title") for card in cards][-1] == "Worker: explorer — Inspect · retry"


def test_paseo_card_update_carries_no_body_and_body_changes_emit_nothing():
    runtime = _card_runtime(paseo=True)
    clock = runtime._worker_card_clock
    cards = []
    runtime._emit_worker_card(cards.append, "a1", status="pending", title="Worker: explorer — Inspect")
    for index in range(200):
        clock.now += 5
        runtime._emit_worker_card(cards.append, "a1", status="in_progress",
                                  entry=f"Tool: Read — f{index}.py " + "x" * 400, entry_ref=f"t:{index}")
    runtime._emit_worker_card(cards.append, "a1", status="completed",
                              result_payload={"summary": "done", "checks": []})
    assert [card["status"] for card in cards] == ["pending", "in_progress", "completed"]
    for card in cards:
        assert "text" not in card and "raw_input" not in card and "content" not in card
        assert _update_bytes(card) <= 2048
    assert cards[-1]["raw_output"] == {"summary": "done", "checks": []}


def test_failed_card_carries_one_short_failure_line():
    for paseo in (True, False):
        runtime = _card_runtime(paseo=paseo)
        cards = []
        runtime._emit_worker_card(cards.append, "a1", status="pending", title="Worker")
        runtime._emit_worker_card(cards.append, "a1", status="failed", entry="Worker failed: " + "boom " * 1000)
        assert cards[-1]["status"] == "failed"
        assert cards[-1]["raw_output"].startswith("Worker failed: boom")
        assert len(cards[-1]["raw_output"].encode("utf-8")) <= runtime_module._WORKER_CARD_FAILURE_MAX_BYTES
        assert "raw_input" not in cards[-1]


def test_completed_result_projection_is_capped():
    runtime = _card_runtime(paseo=True)
    cards = []
    checks = [dict(name=f"check {index} " + "n" * 150, outcome="passed") for index in range(30)]
    runtime._emit_worker_card(cards.append, "a1", status="pending", title="Worker")
    runtime._emit_worker_card(cards.append, "a1", status="completed",
                              result_payload={"summary": "s" * 5000, "checks": checks})
    projection = cards[-1]["raw_output"]
    assert _update_bytes(projection) <= runtime_module._WORKER_CARD_RESULT_MAX_BYTES
    assert projection["summary"]


def test_worker_action_log_is_bounded_and_late_evicted_updates_are_dropped():
    runtime = _card_runtime(paseo=False)
    for index in range(500):
        runtime._append_worker_entry("a1", f"Action {index} " + "y" * 2000, f"ref-{index}")
    log = runtime._worker_action_log["a1"]
    assert len(log) == runtime_module._WORKER_CARD_TAIL_ENTRIES
    assert log[-1].startswith("Action 499")
    assert all(len(line) <= runtime_module._WORKER_ACTIVITY_MAX_CHARS for line in log)
    assert len(runtime._worker_action_refs["a1"]) <= runtime_module._WORKER_CARD_TAIL_ENTRIES
    # A late status update for a retained ref replaces its row ...
    runtime._append_worker_entry("a1", "Action 499 complete", "ref-499")
    assert runtime._worker_action_log["a1"][-1] == "Action 499 complete"
    assert len(runtime._worker_action_log["a1"]) == runtime_module._WORKER_CARD_TAIL_ENTRIES
    # ... while one for an evicted ref cannot resurrect it at the tail.
    runtime._append_worker_entry("a1", "Action 470 complete", "ref-470")
    assert "Action 470 complete" not in runtime._worker_action_log["a1"]
    # Tombstones themselves are bounded.
    assert len(runtime._worker_action_evicted_refs["a1"]) <= runtime_module._WORKER_ACTION_EVICTED_REFS_MAX
    assert runtime._worker_action_dropped["a1"] == 480


def test_tool_line_arguments_clipped_and_content_elided():
    line = runtime_module._worker_tool_details(
        "Project write", "completed", None, None,
        {"server": "cortex", "tool": "project_write",
         "arguments": {"path": "src/" + "p" * 400 + ".py", "content": "secret-free code " * 5000,
                       "request_key": "rk"}},
        None, "/fixture",
    )
    assert "content=<85000 bytes>" in line
    assert "secret-free code" not in line
    assert len(line) <= runtime_module._WORKER_TOOL_LINE_MAX_CHARS
    command = runtime_module._worker_tool_details(
        "Shell", "completed", None, None, {"command": "echo " + "z" * 5000}, None, "/fixture",
        kind="execute",
    )
    assert len(command) <= runtime_module._WORKER_TOOL_LINE_MAX_CHARS


# -- Coordinator tool cards (P2) --------------------------------------------

def test_coordinator_tool_card_fields_are_capped_with_marker():
    cards = []
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    emit = runtime._engine_tool_call_emitter(cards.append, id_scope="coordinator")
    emit("write", "edit", "Write", "completed", [{"path": "/p/a.py"}],
         [{"type": "diff", "path": "/p/a.py", "oldText": "", "newText": "line\n" * 20_000}],
         {"file_path": "/p/a.py", "content": "line\n" * 20_000},
         {"stdout": "o" * 100_000, "items": list(range(20_000))})
    card = cards[-1]
    cap = runtime_module._COORDINATOR_CARD_FIELD_MAX_BYTES
    assert _update_bytes(card["raw_input"]) <= cap
    assert _update_bytes(card["raw_output"]) <= cap
    for block in card["content"]:
        assert _update_bytes(block) <= cap
    assert card["raw_input"]["file_path"] == "/p/a.py"
    assert "full record in task history" in card["raw_input"]["content"]
    assert card["content"][0]["type"] == "diff" and card["content"][0]["path"] == "/p/a.py"


def _mcp_output(value, text=None):
    return {"error": None, "result": {"content": [{"type": "text", "text": text if text is not None
                                                   else json.dumps(value)}],
                                      "structuredContent": value}}


def test_coordinator_tool_card_sends_nothing_twice():
    cards = []
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    emit = runtime._engine_tool_call_emitter(cards.append, id_scope="coordinator")
    envelope = {"server": "cortex", "tool": "task_get_context", "arguments": {}}
    context = {"state": "PLAN_READY", "goal": "g" * 3000}
    emit("p1", "other", "mcp.cortex.task_get_context", "in_progress", None, None, envelope, None)
    emit("p1", None, None, "completed", None, None, envelope, _mcp_output(context))
    # The completion repeats neither the input nor the title; the MCP text
    # copy of structuredContent is dropped.
    assert "raw_input" not in cards[-1] and "title" not in cards[-1]
    assert cards[-1]["raw_output"]["result"] == {"structuredContent": context}
    # An identical poll references the first card instead of resending.
    emit("p2", "other", "mcp.cortex.task_get_context", "in_progress", None, None, envelope, None)
    emit("p2", None, None, "completed", None, None, envelope, _mcp_output(context))
    assert cards[-1]["raw_output"]["repeated_output"] is True
    assert cards[-1]["raw_output"]["same_as"] == "call:coordinator:p1"
    assert _update_bytes(cards[-1]) <= 512
    # A changed output is sent in full, and becomes the new reference.
    changed = dict(context, state="EXECUTING")
    emit("p3", "other", "mcp.cortex.task_get_context", "in_progress", None, None, envelope, None)
    emit("p3", None, None, "completed", None, None, envelope, _mcp_output(changed))
    assert cards[-1]["raw_output"]["result"]["structuredContent"] == changed
    # A different input is never treated as a repeat.
    other = {"server": "cortex", "tool": "read_report", "arguments": {"report_id": "r_1"}}
    emit("p4", "other", "mcp.cortex.read_report", "in_progress", None, None, other, None)
    emit("p4", None, None, "completed", None, None, other, _mcp_output(changed))
    assert cards[-1]["raw_output"]["result"]["structuredContent"] == changed
    # An update that changes nothing is not sent at all.
    count = len(cards)
    emit("p5", "other", "Shell", "in_progress", None, None, {"command": "ls"}, None)
    emit("p5", "other", "Shell", "in_progress", None, None, {"command": "ls"}, None)
    assert len(cards) == count + 1


def test_mcp_text_that_is_not_the_structured_mirror_is_kept():
    cards = []
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    emit = runtime._engine_tool_call_emitter(cards.append, id_scope="coordinator")
    output = _mcp_output({"state": "PLAN_READY"}, text="Plan accepted; workers start after this turn.")
    emit("x", "other", "mcp.cortex.plan_submit", "completed", None, None, {"plan": {}}, output)
    assert cards[-1]["raw_output"]["result"]["content"][0]["text"].startswith("Plan accepted")


def test_worker_card_update_omits_unchanged_title():
    runtime = _card_runtime(paseo=True)
    cards = []
    title = "Worker: explorer · codex · Luna — Inspect the fixture " + "t" * 200
    runtime._emit_worker_card(cards.append, "a1", status="pending", title=title)
    runtime._emit_worker_card(cards.append, "a1", status="in_progress", title=title)
    runtime._emit_worker_card(cards.append, "a1", status="completed", title=title,
                              result_payload={"summary": "done", "checks": []})
    assert cards[0]["title"] == title
    assert [card.get("title") for card in cards[1:]] == [None, None]
    assert [card["status"] for card in cards] == ["pending", "in_progress", "completed"]


# -- Per-update hard cap (P3) -----------------------------------------------

def test_outbound_session_update_hard_cap_keeps_identity_and_status():
    update = protocol.tool_call_update(
        "s", "worker:a1", status="completed", title="Worker",
        text="t" * 200_000, raw_input={"a": "b" * 200_000, "c": list(range(50_000))},
        raw_output={"summary": "done", "blob": "q" * 100_000},
    )["params"]["update"]
    capped = cap_session_update(update)
    assert _update_bytes(capped) <= SESSION_UPDATE_MAX_BYTES
    assert capped["sessionUpdate"] == "tool_call_update"
    assert capped["toolCallId"] == "worker:a1"
    assert capped["status"] == "completed"
    assert capped["title"] == "Worker"
    assert capped["rawOutput"]["summary"] == "done"
    assert "truncated" in json.dumps(capped)
    small = dict(sessionUpdate="tool_call_update", toolCallId="x", status="in_progress")
    assert cap_session_update(small) is small


def test_agent_persists_and_sends_only_the_capped_update(tmp_path):
    from test_acp_elicitation import _Harness, _initialize_and_new_session, _project

    harness = _Harness(tmp_path, lambda *a, **k: None)
    harness.start()
    try:
        session_id = _initialize_and_new_session(harness, _project(tmp_path), elicitation_form=None)
        emit = harness.agent._make_emit_tool_call(session_id, threading.Event())
        assert emit(dict(tool_call_id="call:x", new=True, title="Big", kind="other", status="completed",
                         raw_output={"stdout": "z" * 500_000}))
        message = harness.wait_for(lambda m: m.get("method") == "session/update"
                                   and m["params"]["update"].get("toolCallId") == "call:x")
        assert _update_bytes(message["params"]["update"]) <= SESSION_UPDATE_MAX_BYTES
        stored = replay_session(harness.store, session_id)
        assert stored[-1]["update"] == message["params"]["update"]
    finally:
        harness.stop()


# -- Durable card lookups (P5) ----------------------------------------------

def test_durable_card_lookups_use_sql_projection(tmp_path, monkeypatch):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent
    import uuid

    task = bootstrap_task(store, SourceEvent(origin="test", event_id=str(uuid.uuid4())), "Inspect.", "owner")
    create_session(store, "s1", str(tmp_path), {}, {}, [])
    with store.connection() as db:
        db.execute("UPDATE acp_host_sessions SET task_id=? WHERE session_id='s1'", (task.task_id,))
        db.commit()
    big = "x" * 300_000
    append_host_update(store, "s1", dict(sessionUpdate="tool_call", toolCallId="worker:a1",
                                         title="Worker: explorer — Inspect", kind="other", status="pending"))
    for index in range(5):
        append_host_update(store, "s1", dict(sessionUpdate="tool_call_update", toolCallId="worker:a1",
                                             status="in_progress", rawInput=big))
    append_host_update(store, "s1", dict(sessionUpdate="tool_call_update", toolCallId="worker:a10",
                                         status="failed", title="Other · Interrupted"))
    append_host_update(store, "s1", dict(sessionUpdate="tool_call_update", toolCallId="worker:a1",
                                         status="completed", title="Worker: explorer — Inspect · Cancelled"))

    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    parsed = []
    real_loads = json.loads

    def counting_loads(value, *args, **kwargs):
        parsed.append(len(value) if isinstance(value, (str, bytes)) else 0)
        return real_loads(value, *args, **kwargs)

    monkeypatch.setattr(runtime_module.json, "loads", counting_loads)
    assert runtime._durable_worker_card_sessions(task.task_id, "a1") == {"s1": "completed"}
    assert runtime._durable_worker_card(task.task_id, "a1") == (True, "Worker: explorer — Inspect")
    assert runtime._durable_worker_card(task.task_id, "missing") == (False, None)
    assert not any(size > 10_000 for size in parsed), parsed
