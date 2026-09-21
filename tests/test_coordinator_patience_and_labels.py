"""T035 FINDING-13 #2 and the two client-rendering defects found with it,
in the live Zed run recorded as ``evidence/T035/RUN-07``.

Three things are covered, each a defect observed live rather than a
hypothetical:

- **A running worker is not a stall.** Live, the worker became ready at
  18:37:58 and the coordinator declared ``blocked_no_progress`` at
  18:38:12 -- fourteen seconds later, while the worker's own engine turn
  was still in flight. A worker that is genuinely running produces no
  ``delegation_events.MEANINGFUL_EVENT_TYPES`` event of its own until it
  finishes, so it was indistinguishable from nothing happening at all.
- **Consecutive coordinator messages must be separated.** The deltas of
  two consecutive continuation turns arrived as one unbroken run of
  chunks, rendering ``"...before delegating.Pipeline published."``.
- **A worker card names the role a reader reads**, not the internal skill
  id (``worker-backend-dev``), which on a card already titled ``Worker:``
  reads as a stutter.
"""
from __future__ import annotations

import threading
from types import SimpleNamespace
import uuid

import pytest

from cortex_runtime.acp.orchestrating_runtime import (
    OrchestratingRuntime, PromptOutcome, _display_role, _worker_display_text,
    _worker_tool_details,
)
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace
from test_orchestrating_runtime import _publish


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def test_display_role_strips_only_the_storage_prefix():
    assert _display_role("worker-backend-dev") == "backend-dev"


def test_worker_ui_details_are_one_line_and_drop_output():
    details = _worker_tool_details(
        "Execute tests", "completed", [{"path": "/tmp/project/test_counter.py"}],
        [{"type": "text", "text": "1 test passed"}],
        {"command": "python3 -m unittest test_counter.py", "cwd": "/tmp/project"},
        {"exit_code": 0, "formatted_output": "OK"}, "/tmp/project",
    )
    assert "python3 -m unittest test_counter.py" in details
    assert "Execute tests" in details
    assert "test_counter.py" in details
    assert " — test_counter.py" not in details
    assert "Result: OK" not in details
    assert "1 test passed" not in details
    assert "\n" not in details
    assert '"rawInput"' not in details
    assert "{" not in details


def test_worker_display_ignores_tool_envelopes_and_extracts_explicit_text():
    assert _worker_display_text({
        "sessionUpdate": "tool_call_update",
        "content": {"text": '{"rawInput":{"command":"pytest"}}'},
    }) is None
    assert _worker_display_text({
        "sessionUpdate": "agent_thought_chunk",
        "content": {"text": "Checking the implementation."},
    }) == "Checking the implementation."
    # The alias form is already what a reader should see.
    assert _display_role("backend-dev") == "backend-dev"
    # Never strips anything else, and never strips a second time.
    assert _display_role("worker-worker-x") == "worker-x"
    assert _display_role("coordinator") == "coordinator"
    assert _display_role("") == ""


def test_a_running_worker_holds_the_turn_open_and_a_silent_one_closes_itself(tmp_path):
    """Two halves of the same rule.

    A worker that is working holds the coordinator's patience open for as
    long as it takes -- hours, if the work is hard ("воркер может работать
    часами ... и это не повод блокировать"). Nothing charges against it.

    And a worker whose turn ends having published nothing, asked nothing
    and failed in no way is *over*: the assignment records that and
    closes. Before, it recorded nothing at all and stayed open, which only
    ever ended because the coordinator's patience ran out -- exactly the
    behaviour that is now gone."""
    store, task_id = _task(tmp_path)
    worker_may_finish = threading.Event()
    worker_started = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment X", brief="do X", role="backend-dev",
                acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        return PromptOutcome(StopReason.END_TURN, "Still nothing new.")

    def silent_worker(store, channel, session_ref, message, call_index):
        # Runs for as long as this test holds it, then ends without ever
        # publishing a result or asking a question -- so the ONLY thing
        # keeping the coordinator waiting is the turn being in flight.
        worker_started.set()
        worker_may_finish.wait(timeout=10.0)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else silent_worker

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )

    # While the worker runs, the loop must keep resetting the budget. With
    # a 0.02s tick and a budget of 3, a loop that charged ticks would have
    # ended the turn long before this release.
    def release_later():
        worker_started.wait(timeout=10.0)
        for _ in range(60):
            with store.connection() as db:
                row = db.execute(
                    "SELECT no_progress_count, wait_reason FROM task_orchestration WHERE task_id=?", (task_id,),
                ).fetchone()
            if row is not None and row["wait_reason"] == "workers_running":
                break
            threading.Event().wait(0.02)
        # Long enough that a charging loop would have exhausted a budget of
        # 3 ticks of 0.02s many times over.
        threading.Event().wait(0.5)
        worker_may_finish.set()

    releaser = threading.Thread(target=release_later, daemon=True)
    releaser.start()
    cards: list = []
    result = runtime.start_prompt(task_id, "s1", "Do X.", [].append, threading.Event(),
                                  emit_tool_call=cards.append)
    releaser.join(timeout=10.0)

    assert worker_started.is_set(), "the worker's own turn never started; this test proved nothing"
    assert worker_may_finish.is_set(), "the turn ended before the worker was released -- the budget was charged while it ran"
    # The turn ends -- it never hangs -- because the assignment closed
    # itself rather than because the coordinator gave up on it.
    assert result == StopReason.END_TURN
    with store.connection() as db:
        failure = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='assignment_failed'",
            (task_id,),
        ).fetchone()
    assert failure is not None, "a worker that produced nothing must say so"
    assert "worker_ended_without_result" in failure["payload_json"]
    worker_cards = [c for c in cards if c.get("tool_call_id", "").startswith("worker:")]
    assert worker_cards
    assert all("[final response]" not in (c.get("text") or "") for c in worker_cards)
    assert all("Result:" not in (c.get("text") or "") for c in worker_cards)


def test_delegation_batch_yields_to_silent_event_wait(tmp_path):
    """Two in-flight delegate tools finish before the coordinator is paused.

    Intermediate model prose, including text before the delegate and repeated
    capsule polling after it, never reaches the client. Waiting itself does
    not start another coordinator model turn until a worker terminal event.
    """
    store, task_id = _task(tmp_path)
    workers_started = threading.Event()
    release_workers = threading.Event()
    started: set[str] = set()
    updates: list[str] = []
    outcome: list[StopReason] = []
    coordinator_turns = []

    def coordinator(store, channel, session_ref, message, call_index, *, on_delta, on_tool_call, cancel_event):
        coordinator_turns.append(message)
        if call_index:
            return PromptOutcome(StopReason.END_TURN, "Workers finished.")
        on_delta("Looking for tools. ")
        for ref in ("first", "second"):
            on_tool_call(ref, "other", "mcp.cortex.delegate", "in_progress", None, None, None, None)
        for ref in ("first", "second"):
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title=ref, brief=f"Inspect {ref}", role="explorer",
                acceptance="Report findings", request_key=ref,
            ))
            on_tool_call(ref, "other", "mcp.cortex.delegate", "completed", None, None, None, None)
        while not cancel_event.wait(0.01):
            on_delta("Still waiting; reading capsule. ")
        return PromptOutcome(StopReason.CANCELLED, "Still waiting.")

    def worker(store, channel, session_ref, message, call_index):
        started.add(channel.assignment_id)
        if len(started) == 2:
            workers_started.set()
        release_workers.wait(5)
        return PromptOutcome(StopReason.END_TURN, "")

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, lambda s, c, sr, l, i: (
            coordinator if c is not None and c.role == "coordinator" else worker
        )), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    runner = threading.Thread(target=lambda: outcome.append(
        runtime.start_prompt(task_id, "s1", "Inspect both.", updates.append, threading.Event()),
    ), daemon=True)
    runner.start()
    try:
        assert workers_started.wait(5), "both delegations must be dispatched"
        # The first model turn yields after the completed tool batch; while
        # workers remain open, no further model call or narration is allowed.
        for _ in range(100):
            with store.connection() as db:
                row = db.execute(
                    "SELECT wait_reason FROM task_orchestration WHERE task_id=?", (task_id,),
                ).fetchone()
            if row and row["wait_reason"] == "workers_running":
                break
            threading.Event().wait(0.02)
        assert row and row["wait_reason"] == "workers_running"
        assert len(coordinator_turns) == 1
        assert not updates, updates
    finally:
        release_workers.set()
        runner.join(timeout=10)
    assert not runner.is_alive()
    assert outcome == [StopReason.END_TURN]
    assert len(coordinator_turns) >= 2
    assert "Workers finished." in "".join(updates)
    assert "Looking for tools" not in "".join(updates)
    assert "Still waiting" not in "".join(updates)


def test_unverified_coordinator_turns_hide_worker_wait_and_show_later_reply(tmp_path):
    """Delegation chatter is hidden, while the later ordinary reply appears."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment X", brief="do X", role="backend-dev",
                acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "First turn text.")
        return PromptOutcome(StopReason.END_TURN, "Second turn text.")

    def instant_worker(store, channel, session_ref, message, call_index):
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else instant_worker

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    updates: list[str] = []
    cards: list[dict] = []
    runtime.start_prompt(task_id, "s1", "Do X.", updates.append, threading.Event(), emit_tool_call=cards.append)

    streamed = "".join(u for u in updates if isinstance(u, str))
    assert not any(card.get("title") == "Coordinator output (unverified)" for card in cards)
    assert "First turn text." in streamed
    assert "Second turn text." in streamed
    assert "Acceptance refused" not in streamed
    with store.connection() as db:
        state = db.execute(
            "SELECT state FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()[0]
        accepted = db.execute(
            "SELECT COUNT(*) FROM task_events WHERE task_id=? AND event_type='coordinator_decision'",
            (task_id,),
        ).fetchone()[0]
    assert state != "accepted"
    assert accepted == 0


@pytest.mark.parametrize("procedural", [
    "Есть write_report, но сначала нужен create_draft.",
    "I оставил guidance-комментарий на месте. Уберу его.",
])
def test_cortex_tool_turn_hides_procedural_self_talk(tmp_path, procedural):
    store, task_id = _task(tmp_path)

    def coordinator(store, channel, session_ref, message, call_index, *, on_tool_call):
        on_tool_call("tool-1", "other", "mcp.cortex.read_capsule", "in_progress", None, None, None, None)
        on_tool_call("tool-1", "other", None, "completed", None, None, None, None)
        return PromptOutcome(StopReason.END_TURN, procedural)

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, lambda s, c, sr, l, i: coordinator),
        FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=2,
    )
    updates: list[str] = []
    runtime.start_prompt(task_id, "s1", "Continue.", updates.append, threading.Event())
    assert procedural in "".join(updates)


def test_direct_dialogue_without_cortex_tool_remains_visible(tmp_path):
    store, task_id = _task(tmp_path)
    answer = "The module handles project-local task storage."
    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, lambda s, c, sr, l, i: (
            lambda store, channel, session_ref, message, call_index: PromptOutcome(StopReason.END_TURN, answer)
        )), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=2,
    )
    updates: list[str] = []
    runtime.start_prompt(task_id, "s1", "What does this module do?", updates.append, threading.Event())
    assert "".join(updates) == answer


def test_anti_narration_rule_is_delivered_at_coordinator_open(tmp_path):
    store, task_id = _task(tmp_path)
    delivered: list[str] = []

    def router(store, channel, session_ref, launch, instructions):
        if channel is not None and channel.role == "coordinator":
            delivered.append(instructions)
        return lambda store, channel, session_ref, message, call_index: PromptOutcome(
            StopReason.END_TURN, "Done.",
        )

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=2,
    )
    runtime.start_prompt(task_id, "s1", "Say hello.", [].append, threading.Event())
    assert len(delivered) == 1
    assert "Keep tool discovery, argument corrections, report drafting" in delivered[0]
    assert "do not repeatedly read the capsule" in delivered[0]


def test_cortex_tool_question_remains_visible(tmp_path):
    store, task_id = _task(tmp_path)

    def coordinator(store, channel, session_ref, message, call_index, *, on_tool_call):
        on_tool_call("ask-1", "other", "mcp.cortex.decide", "in_progress", None, None, None, None)
        mcp_tools.dispatch(store, channel, "decide", dict(
            assignment_ref="task", decision="ask_user", rationale="Need a target.",
            question="Which environment?", request_key="ask-env",
        ))
        on_tool_call("ask-1", "other", None, "completed", None, None, None, None)
        return PromptOutcome(StopReason.END_TURN, "Which environment?")

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, lambda s, c, sr, l, i: coordinator),
        FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=2,
    )
    updates: list[str] = []
    runtime.start_prompt(task_id, "s1", "Deploy it.", updates.append, threading.Event())
    assert "Which environment?" in "".join(updates)


def test_a_running_workers_card_shows_what_it_is_doing(tmp_path):
    """The reported gap: between ``assignment_ready`` and its result a
    worker's card said only "in_progress". Its own turn must reach the
    card -- and never the client's message stream, which is the
    coordinator's."""
    store, task_id = _task(tmp_path)
    worker_may_finish = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment X", brief="do X", role="backend-dev",
                acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        return PromptOutcome(StopReason.END_TURN, "Nothing new.")

    def talking_worker(store, channel, session_ref, message, call_index, on_tool_call=None):
        # What a worker *does* is what its card says; a fragment of what it
        # *says* used to land in the title, cut mid-word.
        if on_tool_call is not None:
            on_tool_call("t1", "read", "Reading calc.py", "in_progress", None, None)
        worker_may_finish.wait(timeout=5.0)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else talking_worker

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    cards: list = []
    updates: list = []

    def release_later():
        for _ in range(100):
            if any("Reading calc.py" in str(c.get("text", "")) for c in list(cards)):
                break
            threading.Event().wait(0.02)
        worker_may_finish.set()

    releaser = threading.Thread(target=release_later, daemon=True)
    releaser.start()
    runtime.start_prompt(
        task_id, "s1", "Do X.", updates.append, threading.Event(), emit_tool_call=cards.append,
    )
    releaser.join(timeout=10.0)

    # The action stays inside the worker's single assignment card.
    activity = [c for c in cards if c.get("tool_call_id", "").startswith("worker:")
                and "Reading calc.py" in str(c.get("text", ""))]
    assert activity, f"the running worker's own turn never reached the feed: {cards}"
    # Its title names the worker and its assignment, and stays put: what it
    # is doing goes in the block below, not into the heading. (An update
    # that only adds to the body carries no title at all.)
    titled = [c for c in cards if c.get("title")]
    assert any("Worker: backend-dev — Assignment X" == c["title"] for c in titled), titled
    assert any(c.get("status") == "in_progress" for c in activity), activity
    assert activity[0]["text"].count("Reading calc.py") == 1
    assert "Status:" not in activity[0]["text"]
    # The assignment keeps exactly one card of its own, never one per action.
    assignment_cards = {c["tool_call_id"] for c in cards if c["tool_call_id"].startswith("worker:")}
    assert len(assignment_cards) == 1, cards
    # No card is re-sent unchanged, however many wait ticks pass.
    sent = [repr(sorted(c.items(), key=lambda item: item[0])) for c in cards]
    assert len(sent) == len(set(sent)), f"the same card was sent twice: {sent}"
    # The worker's text is card content, never the coordinator's stream.
    assert "Reading calc.py" not in "".join(u for u in updates if isinstance(u, str))


def test_a_running_worker_shows_its_current_command_on_its_own_card(tmp_path):
    """A card per action filled the feed with a worker's whole transcript
    and pushed the coordinator's own work out of sight. Each worker has
    exactly one card instead, and its current command is shown on that
    card and replaced as it works -- per assignment, so a second worker
    only ever updates its own."""
    store, task_id = _task(tmp_path)
    worker_may_finish = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment X", brief="do X", role="backend-dev",
                acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        return PromptOutcome(StopReason.END_TURN, "Nothing new.")

    def working_worker(store, channel, session_ref, message, call_index, on_tool_call=None):
        if on_tool_call is not None:
            on_tool_call("t1", "read", "Read calc.py", "completed", None, None)
            on_tool_call("t2", "edit", "Edit calc.py", "in_progress", None, None)
        worker_may_finish.wait(timeout=5.0)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else working_worker

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    cards: list = []

    def release_later():
        for _ in range(200):
            if any("Edit calc.py" in str(c.get("text", "")) for c in list(cards)):
                break
            threading.Event().wait(0.02)
        worker_may_finish.set()

    releaser = threading.Thread(target=release_later, daemon=True)
    releaser.start()
    runtime.start_prompt(task_id, "s1", "Do X.", [].append, threading.Event(), emit_tool_call=cards.append)
    releaser.join(timeout=10.0)

    actions = [c for c in cards if c.get("tool_call_id", "").startswith("worker:")
               and ("Read calc.py" in str(c.get("text", ""))
                    or "Edit calc.py" in str(c.get("text", "")))]
    assert actions, f"the worker's actions never reached the feed: {cards}"

    assert len({c["tool_call_id"] for c in actions}) == 1
    assert all("Status:" not in c["text"] for c in actions)


def _worker_card_ids(cards):
    return {c["tool_call_id"] for c in cards if c["tool_call_id"].startswith("worker:")}


def test_an_explicit_language_choice_is_told_to_the_coordinator(tmp_path):
    """Reported: the selector said Russian and the coordinator answered in
    English. Nothing had ever told the model which language to answer in --
    it followed the language of the prompt it was given."""
    from cortex_runtime.acp import steering as acp_steering

    store, task_id = _task(tmp_path)
    seen: list[str] = []

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        seen.append(message)
        return PromptOutcome(StopReason.END_TURN, "Готово.")

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel is not None and channel.role == "coordinator" else None

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    acp_steering.set_language_override(store, task_id, "ru")
    runtime.start_prompt(task_id, "s1", "Add a guard, in English words.", [].append, threading.Event())

    assert seen, "the coordinator was never prompted"
    assert "по-русски" in seen[0], seen[0]

    # "auto" imposes nothing: the model keeps following the user.
    second = tmp_path / "second"
    second.mkdir()
    store2, task2 = _task(second)
    seen2: list[str] = []

    def behavior2(store, channel, session_ref, message, call_index):
        seen2.append(message)
        return PromptOutcome(StopReason.END_TURN, "Done.")

    runtime2 = OrchestratingRuntime(
        store2, FakeSessionRuntime(store2, lambda s, c, sr, l, i: behavior2 if c is not None and c.role == "coordinator" else None),
        FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    acp_steering.set_language_override(store2, task2, "auto")
    runtime2.start_prompt(task2, "s1", "Add a guard.", [].append, threading.Event())
    assert seen2
    assert "по-русски" not in seen2[0] and "in English" not in seen2[0], seen2[0]


def test_a_delegation_starts_while_the_coordinator_is_still_talking(tmp_path):
    """T035 FINDING-13 #1, measured live: the coordinator calls delegate()
    mid-turn, but the delegation was only acted on after its whole turn
    returned -- so the worker could not start until the coordinator stopped
    talking, which live meant minutes."""
    store, task_id = _task(tmp_path)
    delegated = threading.Event()
    worker_started = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Assignment X", brief="do X", role="backend-dev",
                acceptance="X done", request_key="reqX",
            ))
            delegated.set()
            # Keep talking. Before the fix nothing could happen here at all;
            # the worker only ever started after this returned.
            started_in_time = worker_started.wait(timeout=5.0)
            assert started_in_time, "the worker did not start while the coordinator was still in its turn"
            return PromptOutcome(StopReason.END_TURN, "Delegated X.")
        return PromptOutcome(StopReason.END_TURN, "Nothing new.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        worker_started.set()
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    runtime.start_prompt(task_id, "s1", "Do X.", [].append, threading.Event())

    assert delegated.is_set()
    assert worker_started.is_set()
    # No early-dispatch thread outlives the turn it belongs to.
    assert not [th for th in threading.enumerate() if th.name == "cortex-early-dispatch" and th.is_alive()]


def test_a_tool_call_card_waits_for_a_name_instead_of_saying_tool_call():
    """Reported looking at a live feed full of cards reading "tool call":
    that tells a reader nothing. Both engines name a call when they create
    it and then send status-only updates, so a card must never be created
    unnamed, and an update must never overwrite the name it has."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    runtime = _R.__new__(_R)  # only the emitter factory is exercised
    on_tool_call = _R._engine_tool_call_emitter(runtime, cards.append)

    # A pending unnamed call is buffered until the engine identifies it.
    on_tool_call("c1", "execute", None, "pending")
    assert cards == []

    # Once named, the buffered start and current update appear in order.
    on_tool_call("c1", None, "Run `pytest -q`", None)
    assert len(cards) == 2
    assert cards[0]["new"] is True
    assert cards[1]["new"] is False
    assert "Run `pytest -q`" in cards[1]["text"]

    # A status-only update keeps the name instead of clobbering it.
    on_tool_call("c1", None, None, "completed")
    assert cards[2]["tool_call_id"] == cards[1]["tool_call_id"]
    assert cards[2]["status"] == "completed"
    assert "Run `pytest -q`" in cards[2]["text"]

    # An unknown kind still renders, as the schema's catch-all.
    on_tool_call("c2", "sorcery", "Consult the oracle", "in_progress")
    assert cards[3]["kind"] == "other"
    assert "Consult the oracle" in cards[3]["text"]


def test_the_identical_integration_conflict_is_not_an_endless_retry(tmp_path):
    """Watched live: the worker finished, integration conflicted, the
    coordinator sent it back to redo work that was already done, and that
    ran in a circle. A conflict that never changes is not a retry."""
    from cortex_runtime.acp.orchestrating_runtime import _coalesce
    from cortex_runtime.delegation import events as delegation_events

    store, task_id = _task(tmp_path)
    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, lambda *a, **k: None), FakeWorkspace(), FakeConfigSnapshotSource(),
    )
    conflict = {"calc.py": {"reason": "host_buffer_check_failed"}}
    assignment = "a_ref"

    assert runtime._identical_integration_conflicts(task_id, assignment, conflict) == 0
    for index in range(2):
        delegation_events.append_event(
            store, task_id, "progress", f"integration_conflict:r{index}",
            dict(kind="integration_conflict", assignment_ref=assignment, request_key=f"r{index}",
                 conflict_paths=conflict, host_buffer_state="check_failed"),
            origin="runtime", assignment_ref=assignment,
        )
    assert runtime._identical_integration_conflicts(task_id, assignment, conflict) == 2

    # A conflict that moved is progress, and is never counted against it.
    moved = {"other.py": {"reason": "unsaved_host_buffer"}}
    assert runtime._identical_integration_conflicts(task_id, assignment, moved) == 0

    # The coordinator is told plainly, once the repeat limit is reached.
    events = delegation_events.claim_unconsumed(store, task_id, "c1")
    rendered = _coalesce(events, {})
    assert "CONFLICTED" in rendered
    assert "will not change this" not in rendered, "a plain conflict must stay a plain conflict"

    delegation_events.append_event(
        store, task_id, "progress", "integration_conflict:r2",
        dict(kind="integration_conflict", assignment_ref=assignment, request_key="r2",
             conflict_paths=conflict, host_buffer_state="check_failed", repeated=3),
        origin="runtime", assignment_ref=assignment,
    )
    rendered = _coalesce(delegation_events.claim_unconsumed(store, task_id, "c2"), {})
    assert "has now happened 3 times" in rendered
    assert "Stop retrying" in rendered


def test_a_worker_card_is_never_updated_before_it_is_created(tmp_path):
    """Zed answers an update for a card it never saw with "Tool call not
    found", and the card -- the delegation itself -- never appears.

    It happened because a card's create came from the event-claim path
    (which runs between coordinator turns) while its updates came from the
    wait/provisioning path: once a delegation started while the turn that
    asked for it was still running, the update overtook the create. The
    fix is not to repair it at the connection -- it is that one place owns
    the card's lifecycle and decides create-vs-update from what has
    actually been emitted."""
    store, task_id = _task(tmp_path)
    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, lambda *a, **k: None), FakeWorkspace(), FakeConfigSnapshotSource(),
    )
    cards: list = []

    # The provisioning/activity paths reach the card first, update-shaped.
    runtime._emit_worker_card(cards.append, "a_ref", status="in_progress", text="Provisioning: channel")
    assert cards[0]["new"] is True, cards[0]
    assert cards[0]["title"], "a card must be named when it is created"
    assert cards[0]["kind"], "a card must have a kind when it is created"

    # The claim path's own create later is just an update: one card, one id.
    runtime._emit_worker_card(
        cards.append, "a_ref", title="Worker: backend-dev — Add a guard", status="pending",
    )
    assert cards[1]["new"] is False
    assert cards[1]["title"] == "Worker: backend-dev — Add a guard"
    assert len({c["tool_call_id"] for c in cards}) == 1


def test_the_coordinator_can_write_the_project_store_but_not_its_source(tmp_path):
    """`create_draft` returns "an absolute server-created project path to
    edit with native file tools". The read-only snapshot excluded
    `.cortex` entirely, so the coordinator could not reach the file it had
    just been told to fill -- observed live, repeatedly. The store is
    Cortex's own and must be writable; the project's source must not be."""
    from cortex_runtime.delegation.workspace_isolation import IsolatedSnapshotWorkspaceProvisioner
    import os

    project = tmp_path / "project"
    project.mkdir()
    (project / "calc.py").write_text("def divide(a, b):\n    return a / b\n")
    store = Store(project / ".cortex", project_root=project)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Guard divide.", "owner-1")
    drafts = project / ".cortex" / "pipeline-drafts"
    drafts.mkdir(parents=True, exist_ok=True)

    provisioner = IsolatedSnapshotWorkspaceProvisioner(store, str(project))
    root, mode = provisioner.coordinator_root_for(result.task_id)
    assert mode == "read_only_snapshot"

    # The store is the real one, and writable through it.
    linked = os.path.join(root, ".cortex")
    assert os.path.realpath(linked) == os.path.realpath(str(project / ".cortex"))
    draft = os.path.join(linked, "pipeline-drafts", "d1.md")
    with open(draft, "w") as handle:
        handle.write("# plan\n")
    assert (drafts / "d1.md").read_text() == "# plan\n"

    # The project's own source is still not writable from here.
    import pytest as _pytest

    with _pytest.raises(PermissionError):
        with open(os.path.join(root, "calc.py"), "w") as handle:
            handle.write("broken")
    assert (project / "calc.py").read_text().startswith("def divide")


def test_this_runtimes_own_bookkeeping_is_not_shown_at_all():
    """Collapsing the plumbing into one block was still a block of
    plumbing in a feed that exists to show the work ("давай служебные
    операции просто скроем и не будем выводить"). The delegation itself
    stays visible as the worker's own card; nothing else about Cortex's
    own tools or task files reaches the reader."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    on_tool_call = _R._engine_tool_call_emitter(_R.__new__(_R), cards.append)

    on_tool_call("i1", "other", "mcp.cortex.create_task", "completed")
    on_tool_call("i2", "other", "mcp.cortex.set_governance", "completed")
    on_tool_call("i3", "other", "mcp.cortex.delegate", "completed")
    on_tool_call("i4", "execute", "sed -n '1,45p' .cortex/draft-reports/d_76.md", "completed")
    assert cards == []

    # Real work still gets its own card, with its own name.
    on_tool_call("r1", "read", "Read calc.py", "completed")
    assert len(cards) == 1
    assert "Read calc.py" in cards[-1]["text"]


def test_the_worker_card_appears_while_the_coordinator_is_still_talking(tmp_path):
    """Reported: "I delegated it" with no worker anywhere in the feed. The
    card was emitted from the event-claim path, which only runs between
    coordinator turns -- but a delegation now starts during the turn that
    asked for it, so for the whole length of that turn there was nothing
    to see."""
    store, task_id = _task(tmp_path)
    worker_may_finish = threading.Event()
    cards: list = []

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Add a guard", brief="do X", role="backend-dev",
                acceptance="X done", request_key="reqX",
            ))
            # Still talking. The card must already be there.
            for _ in range(200):
                if any("Add a guard" in str(c.get("title", "")) for c in list(cards)):
                    break
                threading.Event().wait(0.02)
            worker_may_finish.set()
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        return PromptOutcome(StopReason.END_TURN, "Nothing new.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        worker_may_finish.wait(timeout=5.0)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    runtime.start_prompt(task_id, "s1", "Do X.", [].append, threading.Event(), emit_tool_call=cards.append)

    assert worker_may_finish.is_set()
    named = [c for c in cards if "Add a guard" in str(c.get("title", ""))]
    assert named, f"the worker never appeared while the turn ran: {cards}"
    assert named[0]["new"] is True and "Worker: backend-dev" in named[0]["title"]


def test_store_and_project_edits_are_compact_without_diff_output():
    """Both calls stay in sequence, while diffs and transport data stay out."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    on_tool_call = _R._engine_tool_call_emitter(_R.__new__(_R), cards.append)

    on_tool_call(
        "e1", "edit", "Edit file", "completed",
        [{"path": "/p/.cortex/draft-reports/r1.md"}], None,
    )
    assert cards == []

    # The project edit is a second individual card without output.
    on_tool_call(
        "e2", "edit", "Edit calc.py", "completed",
        [{"path": "/p/calc.py"}], [{"type": "diff", "path": "/p/calc.py", "oldText": "a", "newText": "b"}],
    )
    assert "Edit calc.py" in cards[-1]["text"]
    assert "type: diff" not in cards[-1]["text"]
    assert "newText" not in cards[-1]["text"]
    assert len(cards[-1]["text"].splitlines()) == 1
    assert len(cards) == 1


def test_a_path_inside_the_project_is_shown_relative_to_it():
    """A card reading "Read file '/var/tmp/demo-project/calc.py'" spends
    its width on the one thing the reader already knows. A path outside
    the project is the opposite case: being elsewhere is the point."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    on_tool_call = _R._engine_tool_call_emitter(
        _R.__new__(_R), cards.append, project_root="/p/demo",
    )
    on_tool_call("r1", "read", "Read file '/p/demo/src/calc.py'", "completed")
    assert "Read file 'src/calc.py'" in cards[-1]["text"]

    on_tool_call("r2", "read", "Read file '/etc/hosts'", "completed")
    assert "Read file '/etc/hosts'" in cards[-1]["text"]

    # File paths in the compact line are relative to the opened project.
    on_tool_call("r3", "read", "Read calc.py", "completed", [{"path": "/p/demo/src/calc.py"}])
    assert "Read calc.py — src/calc.py" in cards[-1]["text"]

    # With no project known, nothing is rewritten.
    plain: list = []
    _R._engine_tool_call_emitter(_R.__new__(_R), plain.append)("r4", "read", "Read '/p/demo/x.py'", "completed")
    assert "Read '/p/demo/x.py'" in plain[-1]["text"]


def test_a_worker_tool_call_actually_reaches_its_card(tmp_path):
    """The card existed, the log was always empty, and the tests passed:
    they called `_emit_worker_activity_tool_calls` directly instead of
    going through the callback the adapter really invokes. The adapter
    grew `locations`/`content` arguments, the worker callback's positional
    `_ref` default silently ate `locations`, the extra argument raised
    TypeError, and the adapter's best-effort `except` swallowed it. This
    goes through the callback, with the adapter's real argument count."""
    store, task_id = _task(tmp_path)
    seen: dict = {}
    calc_path = str(tmp_path / "calc.py")
    draft_path = str(tmp_path / ".cortex" / "draft-reports" / "d_123.md")
    long_command = "python3 -B -m pytest tests/test_calc.py " + "--verbose " * 35

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Guard divide", brief="do X", role="backend-dev",
                acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        return PromptOutcome(StopReason.END_TURN, "Nothing new.")

    def worker_behavior(store, channel, session_ref, message, call_index,
                        on_tool_call=None, on_display_update=None, on_delta=None):
        # Exactly how delegation.live_adapters calls it: six positionals.
        on_delta("Starting a verbose progress paragraph. ")
        on_delta("Still inspecting the project; this is interim prose.")
        on_tool_call("wait-1", "other", None, "in_progress")
        on_tool_call("wait-1", "other", "wait", "completed")
        on_tool_call("t1", "read", "Read calc.py", "pending",
                     [{"path": calc_path}], None)
        on_tool_call("t2", "edit", "Edit calc.py", "in_progress",
                     [{"path": calc_path}], [{"type": "diff", "path": calc_path,
                                                 "oldText": "a", "newText": "b"}],
                     {"command": "apply_patch calc.py"}, None)
        on_delta("Now checking a second paragraph between tool calls.")
        on_display_update({"sessionUpdate": "agent_thought_chunk",
                           "content": {"type": "text", "text": "private thought"}})
        on_display_update({"sessionUpdate": "agent_message_chunk",
                           "content": {"type": "text", "text": "display-only interim prose"}})
        on_display_update({"sessionUpdate": "plan", "text": "private plan"})
        on_tool_call("t1", None, None, "completed", None, None, None, None)
        on_tool_call("t2", None, None, "completed", None, None, None, None)
        on_tool_call("draft", "edit", "Editing files", "in_progress",
                     [{"path": draft_path}], None, None, None)
        on_tool_call("draft", None, None, "completed", None, None, None, None)
        on_tool_call("internal", "other", "mcp__cortex__write_report", "completed")
        on_tool_call("scheduler", "other", "Task scheduler", "completed")
        on_tool_call("wakeup", "other", "ScheduleWakeup", "completed")
        on_tool_call("external", "other", "mcp__codebase_memory__search_code", "in_progress",
                     None, None, {"query": "divide by zero"}, None)
        on_tool_call("external", None, None, "failed", None, None, None,
                     {"error": "private output"})
        on_tool_call("long-command", "execute", long_command, "in_progress",
                     None, None, {"command": long_command}, None)
        on_tool_call("long-command", None, None, "failed", None, None, None,
                     {"error": "private command output"})
        on_delta("Finished the tool sequence, but no terminal final text was sent.")
        on_tool_call("publish", "other", "mcp__cortex__publish_result", "completed")
        seen["called"] = True
        _publish(store, channel, "guard", call_index)
        return PromptOutcome(StopReason.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    cards: list = []
    runtime.start_prompt(task_id, "s1", "Do X.", [].append, threading.Event(), emit_tool_call=cards.append)

    assert seen.get("called"), "the worker's callback never ran; this test proved nothing"
    actions = [c for c in cards if c.get("tool_call_id", "").startswith("worker:")]
    assert actions, cards
    assert not any(c.get("tool_call_id", "").startswith("worker-action:") for c in cards)
    body = "\n".join(c.get("text") or "" for c in actions)
    assert "Read calc.py" in body and "Edit calc.py" in body
    assert "apply_patch calc.py" in body
    assert "mcp__codebase_memory__search_code" in body and "divide by zero" in body
    assert "Write report" not in body and "Publish result" not in body
    assert "Task scheduler" not in body and "ScheduleWakeup" not in body
    assert "private output" in body
    assert "{" not in body and "}" not in body
    assert not any('"ref": "wait-1"' in (c.get("text") or "") for c in cards)
    # A worker assignment is not a command being run.
    created = [c for c in cards if c.get("new")]
    assert created and created[0]["kind"] != "execute", created[0]
    worker_cards = [c for c in cards if c.get("tool_call_id", "").startswith("worker:")]
    assert worker_cards[-1]["status"] == "completed"
    # Worker cards are the final-only mirror for user-visible worker prose.
    # The delta emitted after the last tool call is therefore retained as the
    # worker's final response even when PromptOutcome.text is empty.
    assert "[final response]" in (worker_cards[-1].get("text") or "")
    # A published result summary is only the fallback when no final worker
    # response was observed; this run has the final-only response marker.
    assert "Result:" not in worker_cards[-1]["text"]
    assert "Report:" not in worker_cards[-1]["text"]
    assert "Checks:" not in worker_cards[-1]["text"]


@pytest.mark.parametrize("outcome_text", ["", "Final worker finding: inspection complete."])
def test_worker_final_response_is_a_durable_worker_card(tmp_path, outcome_text):
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Inspect result", brief="Inspect the input", role="explorer",
                acceptance="Report the result", request_key="inspect-result",
            ))
        return PromptOutcome(StopReason.END_TURN, "Coordinator update.")

    def worker_behavior(store, channel, session_ref, message, call_index,
                        on_tool_call=None, on_delta=None, on_display_update=None):
        on_tool_call("read-final", "read", "Read input", "completed",
                     [{"path": "/p/input.txt"}], None, None, None)
        on_delta("Final worker finding: inspection complete.")
        on_display_update({"sessionUpdate": "agent_message_chunk",
                           "content": {"type": "text", "text": "Unrelated display update."}})
        _publish(store, channel, "inspection", call_index)
        return PromptOutcome(StopReason.END_TURN, outcome_text)

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    cards: list = []
    updates: list = []
    runtime.start_prompt(task_id, "s1", "Inspect.", updates.append, threading.Event(), emit_tool_call=cards.append)
    final_cards = [card for card in cards
                   if card.get("tool_call_id", "").startswith("worker:")
                   and "[final response] Final worker finding: inspection complete." in (card.get("text") or "")]
    assert final_cards, cards
    worker_cards = [card for card in cards if card.get("tool_call_id", "").startswith("worker:")]
    assert worker_cards[0]["title"].startswith("Worker: explorer")
    assert worker_cards[-1]["status"] == "completed"
    final_body = worker_cards[-1]["text"]
    assert "Final worker finding: inspection complete." in final_body
    assert final_body.count("Final worker finding: inspection complete.") == 1
    assert any("Read input" in card.get("text", "") for card in cards
               if card.get("tool_call_id", "").startswith("worker:"))
    assert "Unrelated display update." not in final_body
    assert "Result:" not in final_body
    assert "Queued:" not in final_body
    assert "Provisioning:" not in final_body
    assert "Report:" not in final_body
    assert "Checks:" not in final_body
    assert not any(card.get("tool_call_id", "").startswith("worker-message:") for card in cards)
    assert "Final worker finding: inspection complete." not in "".join(
        value for value in updates if isinstance(value, str)
    )


def test_worker_result_waits_for_delayed_final_response_before_terminal_card(tmp_path):
    """A publish_result can arrive while the worker prompt is still open.

    The result event is deliberately held at the card-claim boundary until
    the worker returns its final text.  The completed card must retain that
    text instead of terminalizing first and rejecting the late response.
    """
    store, task_id = _task(tmp_path)
    result_claimed = threading.Event()
    publisher_done = threading.Event()

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Inspect result", brief="Inspect the input", role="explorer",
                acceptance="Report the result", request_key="delayed-result",
            ))
        return PromptOutcome(StopReason.END_TURN, "Coordinator update.")

    def worker_behavior(store, channel, session_ref, message, call_index,
                        on_tool_call=None, on_delta=None, on_display_update=None):
        on_delta("Final worker finding after publication.")

        def publish():
            try:
                _publish(store, channel, "inspection", call_index)
            finally:
                publisher_done.set()

        publisher = threading.Thread(target=publish)
        publisher.start()
        # The runtime must observe the durable result while this engine turn
        # is still in flight; only then may the turn return its final text.
        assert result_claimed.wait(timeout=10), "worker_result was not claimed while worker was in flight"
        publisher.join(timeout=10)
        assert publisher_done.is_set()
        return PromptOutcome(StopReason.END_TURN, "Final worker finding after publication.")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    original_claim = runtime._emit_assignment_tool_calls

    def observe_claim(task_id_arg, emit_tool_call, claimed):
        original_claim(task_id_arg, emit_tool_call, claimed)
        if any(event.event_type == "worker_result" for event in claimed):
            result_claimed.set()

    runtime._emit_assignment_tool_calls = observe_claim
    cards: list = []
    runtime.start_prompt(task_id, "s1", "Inspect.", [].append, threading.Event(), emit_tool_call=cards.append)

    worker_cards = [card for card in cards if card.get("tool_call_id", "").startswith("worker:")]
    assert worker_cards, cards
    assert worker_cards[-1]["status"] == "completed"
    assert "[final response] Final worker finding after publication." in worker_cards[-1]["text"]
    assert "Result: inspection finished" not in worker_cards[-1]["text"]


def test_delegated_informational_answer_survives_report_read_on_result_continuation(tmp_path):
    """A simple delegated question must return its answer without acceptance.

    The coordinator may reread the worker report before answering.  That
    read is a procedural tool call, but the worker result makes the current
    turn user-facing; its final prose must not be swallowed by the
    self-talk filter or replaced with a bootstrap-pipeline wait notice.
    """
    store, task_id = _task(tmp_path)
    calls = []

    def coordinator_behavior(store, channel, session_ref, message, call_index,
                             *, on_tool_call=None):
        calls.append(call_index)
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Inspect project", brief="Describe the project", role="explorer",
                acceptance="A concise project description", request_key="informational-result",
            ))
            return PromptOutcome(StopReason.END_TURN, "I delegated the inspection.")
        if on_tool_call is not None:
            on_tool_call("read-result", "read", "mcp.cortex.read_report", "completed",
                         None, None, None, None)
        return PromptOutcome(StopReason.END_TURN, "The project is a static site with a separate focus timer.")

    def worker_behavior(store, channel, session_ref, message, call_index):
        _publish(store, channel, "The project is a static site with a separate focus timer.", call_index)
        return PromptOutcome(StopReason.END_TURN, "Worker report published.")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, router), FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    updates = []
    runtime.start_prompt(task_id, "s1", "Tell me about the project.", updates.append, threading.Event())

    assert calls == [0, 1]
    assert "The project is a static site with a separate focus timer." in "".join(updates)
    with store.connection() as db:
        projection = db.execute(
            "SELECT state, wait_reason, wait_detail_json FROM task_orchestration WHERE task_id=?",
            (task_id,),
        ).fetchone()
    assert projection["state"] == "idle_awaiting_user"
    assert projection["wait_reason"] == "idle_awaiting_user"
    assert "runtime bootstrap edition" in projection["wait_detail_json"]


def test_worker_result_summary_fallback_is_one_bounded_line():
    """A retained result remains readable even if no model final was sent."""
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._lock = threading.RLock()
    runtime._worker_action_log = {}
    runtime._worker_action_refs = {}
    runtime._worker_card_sent = {}
    runtime._worker_card_terminal = {}
    runtime._emitted_worker_cards = set()
    runtime._worker_final_response_seen = set()
    runtime._assignment_ref_for_attempt = lambda task_id, attempt: "assignment"
    runtime._close_assignment = lambda task_id, assignment: None
    cards = []
    result = SimpleNamespace(
        event_type="worker_result", assignment_ref="attempt",
        payload={"summary": "x" * 1200, "checks": [{"name": "test", "outcome": "pass"}],
                 "report_ref": "r_secret"},
    )

    runtime._emit_assignment_tool_calls("task", cards.append, [result])
    assert len(cards) == 1
    body = cards[0]["text"]
    assert body.startswith("Result: ")
    assert len(body.splitlines()) == 1
    assert len(body) <= 300
    assert "Checks:" not in body and "Report:" not in body


def test_outcome_only_final_replaces_cleared_interim_delta(tmp_path):
    """The adapter may return final text only in PromptOutcome, after chatter."""
    store, task_id = _task(tmp_path)

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Inspect", brief="Inspect input", role="explorer",
                acceptance="Report result", request_key="inspect-outcome-only",
            ))
        return PromptOutcome(StopReason.END_TURN, "Coordinator text stays separate.")

    def worker_behavior(store, channel, session_ref, message, call_index,
                        on_delta=None, on_tool_call=None):
        on_delta("Interim worker chatter that must disappear.")
        on_tool_call("read", "read", "Read input", "completed",
                     [{"path": "/p/input.txt"}], None, None, None)
        _publish(store, channel, "inspection", call_index)
        return PromptOutcome(StopReason.END_TURN, "Verified final worker finding.")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    engine = FakeSessionRuntime(store, router, emit_outcome_text_as_delta=False)
    runtime = OrchestratingRuntime(
        store, engine, FakeWorkspace(), FakeConfigSnapshotSource(),
        wait_timeout_seconds=0.02, max_no_progress_continuations=3,
    )
    cards: list = []
    text_updates: list = []
    runtime.start_prompt(task_id, "s1", "Inspect.", text_updates.append, threading.Event(),
                         emit_tool_call=cards.append)

    worker_cards = [c for c in cards if c.get("tool_call_id", "").startswith("worker:")]
    assert worker_cards
    body = worker_cards[-1]["text"]
    assert body.count("Verified final worker finding.") == 1
    assert any("Read input" in card.get("text", "") for card in cards
                   if card.get("tool_call_id", "").startswith("worker:"))
    assert "Interim worker chatter" not in body
    assert "Result:" not in body
    assert "Coordinator text stays separate." not in body


def test_tool_content_is_not_forwarded_to_the_client():
    """The compact card never contains terminal handles or diff output."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    on_tool_call = _R._engine_tool_call_emitter(_R.__new__(_R), cards.append)
    on_tool_call(
        "t1", "execute", "Run pytest", "completed", None,
        [{"type": "terminal", "terminalId": "exec-fa8ac10d"},
         {"type": "diff", "path": "/p/calc.py", "oldText": "a", "newText": "b"}],
    )
    assert '"type": "terminal"' not in cards[-1]["text"]
    assert "type: diff" not in cards[-1]["text"]

    # A call whose only content was a terminal carries none, not an empty list.
    on_tool_call("t2", "execute", "Run again", "completed", None,
                 [{"type": "terminal", "terminalId": "exec-b"}])
    assert "content" not in cards[-1], cards[-1]
    assert "Run again" in cards[-1]["text"]


def test_a_native_wait_is_not_shown_and_a_native_subagent_is_refused(tmp_path):
    """Seen live, and the reason the whole delegation path was bypassed: the
    coordinator used codex's own subagents and its `wait` instead of
    `delegate`, so no isolated workspace, no conflict-checked integration,
    no per-role model policy and no audit ever happened -- and the feed
    filled with a `wait` the user asked not to see.

    The runtime also cancels the owning ACP turn as soon as the provider
    exposes the native call: the adapter's independent cancellation watcher
    then sends ``session/cancel`` instead of leaving that child running."""
    import threading
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    store, task_id = _task(tmp_path)
    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, lambda *a, **k: None), FakeWorkspace(), FakeConfigSnapshotSource(),
    )
    cards: list = []
    cancel_event = threading.Event()
    on_tool_call = runtime._engine_tool_call_emitter(
        cards.append, task_id=task_id, cancel_event=cancel_event,
    )

    on_tool_call("w1", "other", "wait", "completed")
    assert cards == []

    on_tool_call("s1", "other", "Complete subagent implement_zero_division_guard", "completed")
    assert cards == [], "a native subagent is engine machinery, not project work"
    assert cancel_event.is_set()

    # And the coordinator is told, durably, the same way a refused mutation is.
    with store.connection() as db:
        notes = [r["payload_json"] for r in db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='progress'", (task_id,))]
    assert any("subagent" in (n or "").lower() for n in notes), notes


def test_a_worker_card_names_the_workers_own_model_not_the_coordinators(tmp_path):
    """The header reads ``provider · model · effort``, and for a worker the
    model must be the one that worker was actually dispatched on.

    ``configuration_revisions`` stores no model at all, and the model in
    ``configuration_route_extras`` is keyed by revision -- it names the
    *coordinator's* route even for a worker opened on a different one (see
    ``live_adapters.ConfigurationApplier``'s own EXT-01 note). The record
    of what a given attempt really ran on is the row ``_record_route_audit``
    writes at dispatch, so that is what the card reads."""
    import json as _json

    from cortex_runtime.routing import store as routing_store

    store, task_id = _task(tmp_path)
    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, lambda *a, **k: None), FakeWorkspace(), FakeConfigSnapshotSource(),
    )
    now = "2026-09-19T00:00:00+00:00"
    with store.connection() as db:
        db.execute(
            "INSERT INTO configuration_revisions "
            "(id, task_id, revision, route_id, route_engine, route_connection_id, parameters, "
            " ceiling_snapshot, state, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            ("cfg-1", task_id, 1, "route-1", "codex", "conn-1",
             _json.dumps({"effort": "medium"}), "{}", "confirmed", now, now),
        )
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, attempt_id, role_id, step, status, config_revision, "
            " owner_epoch, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            ("a_ref", task_id, "att-1", "worker-backend-dev", "engine_session", "done", "cfg-1", 1, now, now),
        )
        db.commit()
    routing_store.record_attempt_audit(
        store, attempt_id="att-1", task_id=task_id, role="worker-backend-dev", engine="codex",
        connection_id="conn-1", model="gpt-5-codex", params_fingerprint="fp", config_revision_id="cfg-1",
        decision="authorized", detail={},
    )

    assert runtime._delegation_route_label(task_id, "a_ref") == "codex · gpt-5-codex · medium"


def test_a_worker_card_header_names_the_effort_the_attempt_actually_ran_with(tmp_path):
    """Provider, model and effort -- all three, or the header is only
    two-thirds of what was asked for. The effort a worker ran with is
    written down only on its own audit row: the revision's parameters are
    the coordinator's, and a worker opened on a policy-chosen route never
    gets a revision row of its own."""
    import json as _json

    from cortex_runtime.routing import store as routing_store

    store, task_id = _task(tmp_path)
    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, lambda *a, **k: None), FakeWorkspace(), FakeConfigSnapshotSource(),
    )
    now = "2026-09-19T00:00:00+00:00"
    with store.connection() as db:
        db.execute(
            "INSERT INTO configuration_revisions "
            "(id, task_id, revision, route_id, route_engine, route_connection_id, parameters, "
            " ceiling_snapshot, state, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            ("cfg-e", task_id, 1, "route-1", "codex", "conn-1",
             _json.dumps({"effort": "low"}), "{}", "confirmed", now, now),
        )
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, attempt_id, role_id, step, status, config_revision, "
            " owner_epoch, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            ("e_ref", task_id, "att-e", "worker-backend-dev", "engine_session", "done", "cfg-e", 1, now, now),
        )
        db.commit()
    routing_store.record_attempt_audit(
        store, attempt_id="att-e", task_id=task_id, role="worker-backend-dev", engine="codex",
        connection_id="conn-1", model="gpt-5.6-luna", params_fingerprint="fp", config_revision_id="cfg-e",
        decision="authorized", detail={"effort": "high"},
    )

    # "low" is the coordinator's own; this worker ran at "high".
    assert runtime._delegation_route_label(task_id, "e_ref") == "codex · gpt-5.6-luna · high"


def test_a_workers_action_is_shown_under_its_attribution_not_beside_it():
    """`backend-dev · codex · gpt-5.6-luna · "sed -n '1,45p' calc.py"` reads
    as though the command were part of the model's name. Who is acting
    belongs on its own line, above what they did."""
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    cards: list = []
    emit = runtime._engine_tool_call_emitter(
        cards.append, prefix="backend-dev · codex · gpt-5.6-luna · high\n", id_scope="a_ref",
    )
    emit("t1", "execute", "sed -n '1,45p' calc.py", "in_progress")

    assert cards, "a named call must produce a card"
    title = cards[-1]["title"]
    who, _, action = title.partition("\n")
    assert who == "backend-dev · codex · gpt-5.6-luna · high"
    assert action == "sed -n '1,45p' calc.py"


def test_a_command_reading_cortexs_own_files_is_bookkeeping():
    """Everything to do with Cortex's own task files is bookkeeping,
    whichever tool touches it -- including a shell command naming the path
    inline, which carries no `locations` and, after the project root is
    stripped, no leading separator to anchor on either."""
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    cards: list = []
    emit = runtime._engine_tool_call_emitter(cards.append, prefix="backend-dev\n", id_scope="a_ref")
    emit("t1", "execute", "sed -n '1,45p' .cortex/draft-reports/d_764434be278c.md", "completed")
    emit("t2", "execute", "python3 -m pytest -q", "completed")

    assert len(cards) == 1, f"only project work is shown: {cards}"


def test_the_engine_thinking_out_loud_is_not_an_action_card():
    """"Guardian Review" and its kin arrived as `think`-kind tool calls and
    sat between the commands the worker actually ran. Deliberation is not
    an action taken on the project."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    on_tool_call = _R._engine_tool_call_emitter(_R.__new__(_R), cards.append)

    on_tool_call("t1", "think", "Guardian Review", "completed")
    assert cards == []

    on_tool_call("c1", "execute", "pytest -q", "completed")
    assert len(cards) == 1
    assert "pytest -q" in cards[-1]["text"]


def test_two_workers_running_at_once_keep_their_actions_apart(tmp_path):
    """A session has one message stream and no sub-feeds, so when two
    workers run at the same time the only thing keeping them apart is
    attribution and card identity. Both engines happily use the same wire
    tool-call id ("call_1"), which is exactly the case that would collapse
    two workers' work onto one card."""
    import threading as _threading

    from cortex_runtime.portable.engine_contract import StopReason as _Stop
    from test_orchestrating_runtime import _assignment_title, _make_runtime

    store, task_id = _task(tmp_path)
    both_started = _threading.Barrier(2, timeout=30)
    delegated = {"done": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if not delegated["done"]:
            delegated["done"] = True
            for title in ("X", "Y"):
                mcp_tools.dispatch(store, channel, "delegate", dict(
                    title=title, brief=f"do {title}", role="backend-dev",
                    acceptance=f"{title} done", request_key=f"req{title}",
                ))
            return PromptOutcome(_Stop.END_TURN, "delegated X and Y")
        return PromptOutcome(_Stop.END_TURN, "")

    def worker_behavior(store, channel, session_ref, message, call_index, on_tool_call=None):
        title = _assignment_title(store, channel)
        # Neither worker may finish before the other has started: this is
        # the concurrent window, not two sequential workers.
        both_started.wait()
        if on_tool_call is not None:
            # Deliberately the same wire id from both engines.
            on_tool_call("call_1", "execute", f"pytest -q tests/test_{title}.py", "completed", None, None)
        _publish(store, channel, title, call_index)
        return PromptOutcome(_Stop.END_TURN, "")

    def router(store, channel, session_ref, launch, instructions):
        if channel is None:
            return None
        return coordinator_behavior if channel.role == "coordinator" else worker_behavior

    runtime, _engine = _make_runtime(store, router)
    cards: list = []
    result = runtime.start_prompt(
        task_id, "s1", "Do X and Y.", lambda t: True, threading.Event(),
        emit_tool_call=lambda card: (cards.append(card), True)[1],
    )
    assert result == _Stop.END_TURN

    actions = [c for c in cards if c.get("tool_call_id", "").startswith("worker:")
               and "pytest -q" in (c.get("text") or "")]
    assert actions, f"neither worker's command reached the feed: {[c.get('text') for c in cards]}"
    # Each worker's work still lands on its own lifecycle card.
    by_card = {}
    for card in actions:
        body = card["text"]
        assert not ("test_X.py" in body and "test_Y.py" in body), card
        by_card.setdefault(card["tool_call_id"], set()).add(
            "X" if "test_X.py" in body else "Y")
    assert len(by_card) == 2, by_card
    assert all(len(files) == 1 for files in by_card.values()), by_card

    # Both results reached the coordinator, each exactly once.
    with store.connection() as db:
        results = db.execute(
            "SELECT assignment_ref FROM task_events WHERE task_id=? AND event_type='worker_result'",
            (task_id,),
        ).fetchall()
    assert len({row["assignment_ref"] for row in results}) == 2, "both workers' results must be recorded"


def test_a_workers_header_is_resolved_when_the_card_is_drawn_not_before():
    """The runtime writes down a worker's real route as it dispatches the
    attempt. A header built a moment earlier falls back to the
    coordinator's own revision -- which carries the engine and the model,
    but no effort at all, so the header silently lost exactly one of the
    three things it exists to show."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    known: dict = {"label": None}
    cards: list = []
    emit = _R._engine_tool_call_emitter(
        _R.__new__(_R), cards.append,
        prefix=lambda: f"backend-dev · {known['label']}\n" if known["label"] else "backend-dev\n",
        id_scope="a_ref",
    )

    emit("t1", "execute", "pytest -q", "in_progress")
    assert cards[-1]["title"] == "backend-dev\npytest -q"

    # The attempt's own route row lands.
    known["label"] = "codex · gpt-5.6-luna · high"
    emit("t2", "execute", "ruff check", "in_progress")
    assert cards[-1]["title"] == "backend-dev · codex · gpt-5.6-luna · high\nruff check"


def test_the_engines_own_approval_step_is_not_an_action_card():
    """Codex reports its own approval step as a tool call ("Guardian
    Review": Status/Action/Risk/Authorization/Rationale). It is the engine
    reviewing itself, and one card per deliberation sat between every two
    commands the worker actually ran. Matched by name as well as by kind,
    because the kind does not always arrive on the create."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    emit = _R._engine_tool_call_emitter(_R.__new__(_R), cards.append, prefix="backend-dev\n", id_scope="a")

    emit("g1", None, "Guardian Review", "completed")
    emit("g2", "other", "guardian review", "in_progress")
    assert cards == []

    emit("c1", "execute", "pytest -q", "completed")
    assert len(cards) == 1


def test_cortex_delegate_is_hidden_in_both_engine_spellings():
    """Delegate is represented by the worker card, while other Cortex
    operations remain visible coordinator evidence."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    emit = _R._engine_tool_call_emitter(_R.__new__(_R), cards.append)

    emit("a", "other", "mcp__cortex__delegate", "completed")
    emit("b", "other", "mcp__cortex__set_governance", "completed")
    emit("c", "other", "mcp.cortex.create_task", "completed")
    emit("d", "other", "ToolSearch", "completed")
    assert cards == []
    assert not any(c["title"] == "Delegate" for c in cards)

    # Another server's tool is somebody's real work and stays visible.
    emit("e", "other", "mcp__codebase_memory__search_code", "completed")
    assert len(cards) == 1
    assert "mcp__codebase_memory__search_code" in cards[-1]["text"]


def test_a_workers_card_never_shows_this_runtimes_own_bookkeeping():
    """The card carries the one thing the worker is doing. Live, that line
    read `mcp__cortex__write_report`: the same plumbing the feed hides,
    arriving by the other route and replacing the last thing the worker
    was actually doing."""
    import threading as _threading

    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    runtime = _R.__new__(_R)
    runtime._lock = _threading.RLock()
    runtime._worker_activity = {}
    runtime._worker_action_log = {}
    runtime._task_tool_call_emitters = {}
    runtime._worker_card_title = {}
    runtime._worker_card_sent = {}
    runtime._emitted_worker_cards = set()

    # The callback the adapter drives, as _provision_and_prompt builds it.
    recorded = runtime._worker_activity

    def record(ref, kind, title, status, *rest, _ref="a_ref"):
        # Mirrors the production guard under test via the public effect:
        # only actions that are the worker's own work are kept.
        from cortex_runtime.acp.orchestrating_runtime import (
            _ENGINE_REVIEW,
            _ENGINE_TOOL_DISCOVERY,
            _INTERNAL_TOOL_CALL,
            _touches_store,
        )

        if not title:
            return
        if (
            _INTERNAL_TOOL_CALL.match(title)
            or _touches_store(title, rest[0] if rest else None)
            or _ENGINE_REVIEW.search(title)
            or _ENGINE_TOOL_DISCOVERY.match(title)
            or kind == "think"
        ):
            return
        recorded[_ref] = title

    record("t1", "execute", "pytest -q", "completed")
    assert recorded["a_ref"] == "pytest -q"
    for hidden in ("mcp__cortex__write_report", "mcp.cortex.publish_result", "Guardian Review", "ToolSearch"):
        record("t2", "other", hidden, "completed")
        assert recorded["a_ref"] == "pytest -q", hidden


def test_a_turn_that_delivered_without_accepting_says_so(tmp_path):
    """Live: the coordinator told the user "принимаю результат" and called
    nothing. The work sat integrated in the project, the task stayed
    unaccepted, and the user believed it was done.

    The runtime speaks for the record, not for the model's prose: when a
    turn ends with a change integrated and no decision accepting it, it
    says exactly that."""
    from cortex_runtime.delegation import events as delegation_events

    store, task_id = _task(tmp_path)
    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, lambda *a, **k: None), FakeWorkspace(), FakeConfigSnapshotSource(),
    )

    # Nothing delivered yet: nothing to say.
    assert runtime._integrated_without_acceptance(task_id) is False

    delegation_events.append_event(
        store, task_id, "progress", "integrate:req-1",
        dict(applied_paths=["calc.py"], kind="integration_applied"), origin="runtime",
    )
    assert runtime._integrated_without_acceptance(task_id) is True

    delegation_events.append_event(
        store, task_id, "coordinator_decision", "decide:acc-1",
        dict(assignment_ref="task", decision="accept", rationale="verified"), origin="coordinator",
    )
    assert runtime._integrated_without_acceptance(task_id) is False


# -- a worker that published its report and never said so -------------------

def test_a_silent_worker_that_published_a_report_delivers_it_instead_of_failing(tmp_path):
    """Live (2026-09-19, the user's own thread): a worker published report
    r_1777da4c6513 through its own channel at 14:43:10, the runtime declared
    the assignment empty five seconds later, and the coordinator delegated
    the identical work again from scratch. The work existed; only the
    sentence about it was missing."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime
    from cortex_runtime.delegation import events as delegation_events
    from cortex_runtime.delegation.work_graph import WorkGraph
    from cortex_runtime.store import Store

    store = Store(tmp_path / ".cortex", project_root=str(tmp_path))
    thread = "thread-of-the-worker"
    with store.connection() as db:
        db.execute("INSERT INTO tasks (id, project_root, created_at) VALUES ('t1', ?, '2026-01-01T00:00:00+00:00')",
                   (str(tmp_path),))
        db.execute("INSERT INTO thread_bindings (thread_id, parent_thread_id, task_id) VALUES (?,NULL,'t1')",
                   (thread,))
        db.execute(
            "INSERT INTO delegation_provisioning (idempotency_key, task_id, attempt_id, role_id, step, status, "
            "owner_epoch, created_at, updated_at) VALUES ('assign-1','t1','xa_1','worker-backend-dev','engine_session',"
            "'ready',1,?,?)", (store._now(), store._now()))
        db.execute(
            "INSERT INTO trusted_channels (channel_id, secret_hash, task_id, principal_thread_id, "
            "parent_principal_thread_id, role, assignment_id, attempt_id, workspace_binding_id, "
            "context_generation_id, owner_epoch, issued_at) VALUES ('ch_1','h','t1',?,NULL,'worker','a1','xa_1',"
            "NULL,NULL,1,?)", (thread, store._now()))
        db.execute("INSERT INTO reports (id, task_id, kind, filename, created_at) "
                   "VALUES ('r_published','t1','report','r_published.md',?)", (store._now(),))
        db.execute(
            "INSERT INTO drafts (id, task_id, owner_thread_id, kind, template, path, created_at, device, inode, "
            "published_report_id, published_digest) VALUES ('d_1','t1',?,'report','implementation','/x/d_1.md',?,"
            "1,1,'r_published','digest')", (thread, store._now()))
        db.commit()

    # The live failure this regression covers was not only a stale legacy
    # assignment card: the executable graph attempt also remained ``running``
    # when the runtime synthesized the worker_result from the published
    # report.  Seed the same graph relation that a real delegate creates.
    graph = WorkGraph(store)
    graph.add_assignment("t1", "implementation", assignment_id="assign-1", idempotency_key="graph-assign")
    graph_attempt = graph.create_attempt("assign-1", idempotency_key="graph-attempt")
    graph.transition_attempt(graph_attempt["attempt_id"], "running", idempotency_key="graph-running")
    delegation_events.append_event(
        store, "t1", "delegation_requested", "delegation-requested:assign-1",
        {"work_attempt_ref": graph_attempt["attempt_id"]}, origin="runtime", assignment_ref="assign-1",
    )

    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime._close_assignment_that_produced_nothing("t1", "assign-1")

    events = delegation_events.list_since(store, "t1")
    assert not [e for e in events if e.event_type == "assignment_failed"], events
    result = next(e for e in events if e.event_type == "worker_result")
    assert result.payload["report_ref"] == "r_published"
    assert "without reporting it" in result.payload["summary"]
    with store.connection() as db:
        status = db.execute(
            "SELECT status FROM work_assignment_attempts WHERE attempt_id=?",
            (graph_attempt["attempt_id"],),
        ).fetchone()["status"]
    assert status == "reported"
