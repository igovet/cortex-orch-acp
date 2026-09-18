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
import uuid

from cortex_runtime.acp.orchestrating_runtime import (
    OrchestratingRuntime, PromptOutcome, _display_role,
)
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _task(tmp_path):
    store = Store(tmp_path / ".cortex", project_root=tmp_path)
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build a page.", "owner-1")
    return store, result.task_id


def test_display_role_strips_only_the_storage_prefix():
    assert _display_role("worker-backend-dev") == "backend-dev"
    # The alias form is already what a reader should see.
    assert _display_role("backend-dev") == "backend-dev"
    # Never strips anything else, and never strips a second time.
    assert _display_role("worker-worker-x") == "worker-x"
    assert _display_role("coordinator") == "coordinator"
    assert _display_role("") == ""


def test_a_worker_turn_in_flight_never_exhausts_the_no_progress_budget(tmp_path):
    """The live defect: a worker whose engine turn is genuinely running
    must hold the coordinator's patience open, and the budget must still
    apply the moment that turn ends without producing anything."""
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
    result = runtime.start_prompt(task_id, "s1", "Do X.", [].append, threading.Event())
    releaser.join(timeout=10.0)

    assert worker_started.is_set(), "the worker's own turn never started; this test proved nothing"
    assert worker_may_finish.is_set(), "the turn ended before the worker was released -- the budget was charged while it ran"
    # The budget still applies once the worker's turn is over and nothing
    # meaningful came of it: the turn ends blocked, never hangs.
    assert result == StopReason.END_TURN
    with store.connection() as db:
        final = db.execute("SELECT state FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    assert final["state"] == "blocked"


def test_consecutive_coordinator_turns_are_separated_in_the_client_stream(tmp_path):
    """Two continuation turns must not render as one run-on sentence."""
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
    runtime.start_prompt(task_id, "s1", "Do X.", updates.append, threading.Event())

    streamed = "".join(u for u in updates if isinstance(u, str))
    # The worker here publishes nothing, so the block that follows the
    # coordinator's own turn is the blocked explanation rather than a
    # second turn -- either way it is a separate block and must not be
    # glued onto the previous sentence, which is exactly what was observed
    # live ("First turn text.Blocked: ...").
    assert "First turn text." in streamed
    assert "First turn text.Blocked" not in streamed, (
        "a notice ran together with the preceding turn's text, with no separator"
    )
    assert "First turn text.\n\n" in streamed


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

    def talking_worker(store, channel, session_ref, message, call_index, on_delta=None):
        if on_delta is not None:
            on_delta("Reading calc.py")
            on_delta(" and adding the guard.")
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

    # What it is doing shows on the card's own title, next to whose card it
    # is -- the collapsed card alone answers "what is the worker on".
    activity = [c for c in cards if "Reading calc.py" in str(c.get("title", ""))]
    assert activity, f"the running worker's own turn never reached its card: {cards}"
    assert all(not c.get("new") for c in activity), "activity must update the existing card, never create a second one"
    assert all(c["status"] == "in_progress" for c in activity)
    assert all("Worker: backend-dev" in c["title"] for c in activity), "the card stopped saying whose work it is"
    # An unchanged card is never re-sent, however many wait ticks pass
    # (the create and the ready update legitimately carry the same base
    # title; it is the activity updates that used to repeat per tick).
    activity_titles = [c["title"] for c in cards if " \u00b7 " in str(c.get("title", ""))]
    assert len(activity_titles) == len(set(activity_titles)), (
        f"the same activity card was sent more than once: {activity_titles}"
    )
    # The worker's text is card content, never the coordinator's stream.
    assert "Reading calc.py" not in "".join(u for u in updates if isinstance(u, str))


def test_a_workers_actions_are_collapsed_into_its_card_body(tmp_path):
    """Requested while watching: the worker's actions should not all be in
    the feed -- the card shows the one it is on, and the rest is there
    when the card is opened."""
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
            on_tool_call("t1", "read", "Read calc.py", "completed")
            on_tool_call("t2", "edit", "Edit calc.py", "in_progress")
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
        for _ in range(100):
            if any("Edit calc.py" in str(c.get("title", "")) for c in list(cards)):
                break
            threading.Event().wait(0.02)
        worker_may_finish.set()

    releaser = threading.Thread(target=release_later, daemon=True)
    releaser.start()
    runtime.start_prompt(task_id, "s1", "Do X.", [].append, threading.Event(), emit_tool_call=cards.append)
    releaser.join(timeout=10.0)

    latest = [c for c in cards if "Edit calc.py" in str(c.get("title", ""))]
    assert latest, f"the action the worker is on never reached its card title: {cards}"
    # The earlier action is not in the title any more, but it is still in
    # the body -- that is the "open it to see the rest" half.
    body = latest[-1].get("text") or ""
    assert "Read calc.py" in body and "Edit calc.py" in body, body
    assert "Read calc.py" not in latest[-1]["title"]
    # One card for the assignment, not one per action.
    assert len({c["tool_call_id"] for c in cards}) == 1, cards


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

    # An unnamed call produces nothing at all -- not a placeholder card.
    on_tool_call("c1", "execute", None, "pending")
    assert cards == [], cards

    # Once named, the card appears, keeping the status seen while unnamed.
    on_tool_call("c1", None, "Run `pytest -q`", None)
    assert len(cards) == 1
    assert cards[0]["new"] is True
    assert cards[0]["title"] == "Run `pytest -q`"
    assert cards[0]["kind"] == "execute"
    assert cards[0]["status"] == "pending"

    # A status-only update keeps the name instead of clobbering it.
    on_tool_call("c1", None, None, "completed")
    assert cards[1] == dict(tool_call_id=cards[0]["tool_call_id"], status="completed",
                            new=False, title="Run `pytest -q`")

    # An unknown kind still renders, as the schema's catch-all.
    on_tool_call("c2", "sorcery", "Consult the oracle", "in_progress")
    assert cards[2]["kind"] == "other"


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


def test_only_consecutive_internal_calls_share_one_block():
    """This runtime's own bookkeeping calls collapse so they do not bury
    the work -- but a run that real work interrupted is finished. Folding
    the whole session into one block puts later operations into a card the
    reader scrolled past long ago."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    on_tool_call = _R._engine_tool_call_emitter(_R.__new__(_R), cards.append)

    on_tool_call("i1", "other", "mcp.cortex.create_task", "completed")
    on_tool_call("i2", "other", "mcp.cortex.set_governance", "completed")
    internal = [c for c in cards if c["tool_call_id"].startswith("call:coordinator:internal")]
    assert len({c["tool_call_id"] for c in internal}) == 1, "a consecutive run is one block"
    assert internal[-1]["text"].splitlines() == ["create_task", "set_governance"]

    # Real work in between ends that run.
    on_tool_call("r1", "read", "Read calc.py", "completed")
    on_tool_call("i3", "other", "mcp.cortex.list_events", "completed")

    blocks = {c["tool_call_id"] for c in cards if ":internal" in c["tool_call_id"]}
    assert len(blocks) == 2, f"a new run must open a new block: {blocks}"
    last = [c for c in cards if c["tool_call_id"] == "call:coordinator:internal:2"]
    assert last[-1]["text"].splitlines() == ["list_events"], last[-1]
    # The real work kept its own card, with its own name.
    assert any(c.get("title") == "Read calc.py" for c in cards)


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


def test_edits_to_the_store_are_bookkeeping_not_edits_to_the_project():
    """Filling a draft or writing a report is done with ordinary file
    operations against `<project>/.cortex` -- by design. To a reader those
    are not edits to their code, and one "Editing files" card per report
    buried the work the report was about."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    on_tool_call = _R._engine_tool_call_emitter(_R.__new__(_R), cards.append)

    on_tool_call(
        "e1", "edit", "Edit file", "completed",
        [{"path": "/p/.cortex/draft-reports/r1.md"}], None,
    )
    assert cards, "the call vanished entirely"
    assert ":internal" in cards[-1]["tool_call_id"], cards[-1]
    assert cards[-1]["title"] == "Task bookkeeping"

    # A real edit to the project keeps its own card, its diff and its path.
    on_tool_call(
        "e2", "edit", "Edit calc.py", "completed",
        [{"path": "/p/calc.py"}], [{"type": "diff", "path": "/p/calc.py", "oldText": "a", "newText": "b"}],
    )
    project_card = [c for c in cards if c.get("title") == "Edit calc.py"]
    assert project_card, cards
    assert project_card[0]["kind"] == "edit"
    assert project_card[0]["locations"] == [{"path": "/p/calc.py"}]
    assert project_card[0]["content"][0]["type"] == "diff"


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
    assert cards[-1]["title"] == "Read file 'src/calc.py'"

    on_tool_call("r2", "read", "Read file '/etc/hosts'", "completed")
    assert cards[-1]["title"] == "Read file '/etc/hosts'"

    # The locations a client turns into links stay absolute -- a relative
    # path is for reading, not for opening.
    on_tool_call("r3", "read", "Read calc.py", "completed", [{"path": "/p/demo/src/calc.py"}])
    assert cards[-1]["locations"] == [{"path": "/p/demo/src/calc.py"}]

    # With no project known, nothing is rewritten.
    plain: list = []
    _R._engine_tool_call_emitter(_R.__new__(_R), plain.append)("r4", "read", "Read '/p/demo/x.py'", "completed")
    assert plain[-1]["title"] == "Read '/p/demo/x.py'"
