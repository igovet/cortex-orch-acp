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
from test_orchestrating_runtime import _publish


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
    result = runtime.start_prompt(task_id, "s1", "Do X.", [].append, threading.Event())
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
            if any("Reading calc.py" in str(c.get("title", "")) for c in list(cards)):
                break
            threading.Event().wait(0.02)
        worker_may_finish.set()

    releaser = threading.Thread(target=release_later, daemon=True)
    releaser.start()
    runtime.start_prompt(
        task_id, "s1", "Do X.", updates.append, threading.Event(), emit_tool_call=cards.append,
    )
    releaser.join(timeout=10.0)

    # What the worker is doing reaches the feed while it is doing it, as
    # its own card, saying whose action it is.
    activity = [c for c in cards if "Reading calc.py" in str(c.get("text", ""))]
    assert activity, f"the running worker's own turn never reached the feed: {cards}"
    # Its title names the worker and its assignment, and stays put: what it
    # is doing goes in the block below, not into the heading. (An update
    # that only adds to the body carries no title at all.)
    titled = [c for c in cards if c.get("title")]
    assert titled and all("backend-dev" in c["title"] for c in titled), titled
    assert all("Reading calc.py" not in c.get("title", "") for c in cards), cards
    assert any(c.get("status") == "in_progress" for c in activity), activity
    # And it is still there at the end: the account of the work is never
    # blanked, least of all by the result that finally explains it.
    final = [c for c in cards if c.get("status") in ("completed", "failed")]
    if final:
        assert "Reading calc.py" in str(final[-1].get("text", "")), final[-1]
    # The assignment keeps exactly one card of its own, never one per action.
    assignment_cards = {c["tool_call_id"] for c in cards if c["tool_call_id"].startswith("worker:")}
    assert len(assignment_cards) == 1, cards
    # No card is re-sent unchanged, however many wait ticks pass.
    sent = [(c["tool_call_id"], c.get("title"), c.get("text")) for c in cards]
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

    actions = [c for c in cards if "Read calc.py" in str(c.get("text", ""))
               or "Edit calc.py" in str(c.get("text", ""))]
    assert actions, f"the worker's actions never reached the feed: {cards}"

    # Everything this worker does belongs to its one card. Nothing it does
    # becomes a card of its own in the feed.
    assignment_cards = _worker_card_ids(cards)
    assert len(assignment_cards) == 1, assignment_cards
    assert all(c["tool_call_id"] in assignment_cards for c in actions), actions

    # Both actions are there, in order, visibly separated -- not one
    # replacing the other.
    latest = actions[-1]["text"]
    assert "Read calc.py" in latest and "Edit calc.py" in latest, latest
    assert latest.index("Read calc.py") < latest.index("Edit calc.py"), latest
    assert "───" in latest, latest


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
    assert cards == [], f"nothing of this runtime's own plumbing is shown: {cards}"

    # Real work still gets its own card, with its own name.
    on_tool_call("r1", "read", "Read calc.py", "completed")
    assert [c.get("title") for c in cards] == ["Read calc.py"]


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
    buried the work the report was about, so they are not shown at all."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    on_tool_call = _R._engine_tool_call_emitter(_R.__new__(_R), cards.append)

    on_tool_call(
        "e1", "edit", "Edit file", "completed",
        [{"path": "/p/.cortex/draft-reports/r1.md"}], None,
    )
    assert cards == [], f"writing a report is not an edit to the project: {cards}"

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

    def coordinator_behavior(store, channel, session_ref, message, call_index):
        if call_index == 0:
            mcp_tools.dispatch(store, channel, "delegate", dict(
                title="Guard divide", brief="do X", role="backend-dev",
                acceptance="X done", request_key="reqX",
            ))
            return PromptOutcome(StopReason.END_TURN, "Delegated.")
        return PromptOutcome(StopReason.END_TURN, "Nothing new.")

    def worker_behavior(store, channel, session_ref, message, call_index, on_tool_call=None):
        # Exactly how delegation.live_adapters calls it: six positionals.
        on_tool_call("t1", "read", "Read calc.py", "completed",
                     [{"path": "/p/calc.py"}], None)
        on_tool_call("t2", "edit", "Edit calc.py", "in_progress",
                     [{"path": "/p/calc.py"}], [{"type": "diff", "path": "/p/calc.py",
                                                 "oldText": "a", "newText": "b"}])
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
    # Accumulated in the worker's own card, under a title that names the
    # worker and stays put.
    bodies = [str(c.get("text", "")) for c in cards]
    assert any("Read calc.py" in body for body in bodies), f"the worker's action never reached its card: {cards}"
    assert any("Edit calc.py" in body for body in bodies), cards
    assert not any("Read calc.py" in str(c.get("title", "")) for c in cards), cards
    # A worker assignment is not a command being run.
    created = [c for c in cards if c.get("new")]
    assert created and created[0]["kind"] != "execute", created[0]


def test_a_terminal_content_block_is_never_forwarded_to_the_client():
    """Seen in Zed's own log: `-32603 ... Terminal with id ... not found`,
    and the whole card lost with it. A tool call's `terminal` content
    refers to a terminal in this agent's session with the engine; the
    client has no such id. Text and diffs mean the same thing in either
    session and still pass through."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    on_tool_call = _R._engine_tool_call_emitter(_R.__new__(_R), cards.append)
    on_tool_call(
        "t1", "execute", "Run pytest", "completed", None,
        [{"type": "terminal", "terminalId": "exec-fa8ac10d"},
         {"type": "diff", "path": "/p/calc.py", "oldText": "a", "newText": "b"}],
    )
    content = cards[-1]["content"]
    assert all(part.get("type") != "terminal" for part in content), content
    assert any(part.get("type") == "diff" for part in content), content

    # A call whose only content was a terminal carries none, not an empty list.
    on_tool_call("t2", "execute", "Run again", "completed", None,
                 [{"type": "terminal", "terminalId": "exec-b"}])
    assert "content" not in cards[-1], cards[-1]


def test_a_native_wait_is_not_shown_and_a_native_subagent_is_refused(tmp_path):
    """Seen live, and the reason the whole delegation path was bypassed: the
    coordinator used codex's own subagents and its `wait` instead of
    `delegate`, so no isolated workspace, no conflict-checked integration,
    no per-role model policy and no audit ever happened -- and the feed
    filled with a `wait` the user asked not to see.

    This runtime cannot stop the engine's own tool. It can refuse to
    dress it up as progress, and it can make sure the coordinator hears
    about it."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    store, task_id = _task(tmp_path)
    runtime = OrchestratingRuntime(
        store, FakeSessionRuntime(store, lambda *a, **k: None), FakeWorkspace(), FakeConfigSnapshotSource(),
    )
    cards: list = []
    on_tool_call = runtime._engine_tool_call_emitter(cards.append, task_id=task_id)

    on_tool_call("w1", "other", "wait", "completed")
    assert cards == [], "a native wait is nothing happening; it does not belong in the feed"

    on_tool_call("s1", "other", "Complete subagent implement_zero_division_guard", "completed")
    assert cards, "a native subagent must not pass unnoticed"
    assert cards[-1]["status"] == "failed"
    assert "subagent" in str(cards[-1].get("text", "")).lower()

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

    assert len(cards) == 1, f"only the real command is shown: {cards}"
    assert cards[0]["title"].endswith("python3 -m pytest -q")


def test_the_engine_thinking_out_loud_is_not_an_action_card():
    """"Guardian Review" and its kin arrived as `think`-kind tool calls and
    sat between the commands the worker actually ran. Deliberation is not
    an action taken on the project."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    on_tool_call = _R._engine_tool_call_emitter(_R.__new__(_R), cards.append)

    on_tool_call("t1", "think", "Guardian Review", "completed")
    assert cards == [], f"thinking is not an action: {cards}"

    on_tool_call("c1", "execute", "pytest -q", "completed")
    assert [c.get("title") for c in cards] == ["pytest -q"]


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

    actions = [c for c in cards if "pytest -q" in (c.get("text") or "")]
    assert actions, f"neither worker's command reached the feed: {[c.get('text') for c in cards]}"
    # Both engines deliberately used the same wire call id. Each worker's
    # work still lands on its own card, and no card ever carries the
    # other's: that is what keeps two running workers legible.
    for card in actions:
        assert card["text"].rsplit("test_", 1)[1].startswith(("X.py", "Y.py")), card
    by_card = {}
    for card in actions:
        by_card.setdefault(card["tool_call_id"], set()).add(
            card["text"].rsplit("test_", 1)[1][:4].rstrip())
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
    assert cards == [], f"the engine reviewing itself is not an action: {cards}"

    emit("c1", "execute", "pytest -q", "completed")
    assert [c["title"] for c in cards] == ["backend-dev\npytest -q"]


def test_this_runtimes_tools_are_hidden_whichever_engine_spells_them():
    """codex writes `mcp.cortex.create_task`; claude writes
    `mcp__cortex__delegate`. Matching only the first spelling put every one
    of the coordinator's own calls straight back into the feed the moment
    the engine changed -- which is exactly what the user saw on sonnet.

    The engine's own tool-discovery step goes with them: like its
    self-review, it is the engine working out what it can do, not an
    action on the project."""
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime as _R

    cards: list = []
    emit = _R._engine_tool_call_emitter(_R.__new__(_R), cards.append)

    emit("a", "other", "mcp__cortex__delegate", "completed")
    emit("b", "other", "mcp__cortex__set_governance", "completed")
    emit("c", "other", "mcp.cortex.create_task", "completed")
    emit("d", "other", "ToolSearch", "completed")
    assert cards == [], f"none of this is the user's business: {cards}"

    # Another server's tool is somebody's real work and stays visible.
    emit("e", "other", "mcp__codebase_memory__search_code", "completed")
    assert [c["title"] for c in cards] == ["mcp__codebase_memory__search_code"]


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

    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._store = store
    runtime._close_assignment_that_produced_nothing("t1", "assign-1")

    events = delegation_events.list_since(store, "t1")
    assert not [e for e in events if e.event_type == "assignment_failed"], events
    result = next(e for e in events if e.event_type == "worker_result")
    assert result.payload["report_ref"] == "r_published"
    assert "without reporting it" in result.payload["summary"]
