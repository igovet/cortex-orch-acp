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
    _worker_tool_details, worker_card_outcome, worker_result_projection,
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
    assert "".join(updates).count(answer) == 3
    assert "structured_proposal_missing" in updates[-1]


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
    assert "Keep tool discovery, corrections and report drafting private" not in delivered[0]
    assert "end the engine turn without text" not in delivered[0]
    assert "No waiting announcements, sleeps or polling" not in delivered[0]








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
    assert "по-русски" not in seen2[0] and "in English" in seen2[0], seen2[0]








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
    runtime._worker_card_outcome = lambda task_id, assignment: ("completed", None)
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


def test_worker_card_outcome_distinguishes_clean_result_issues_and_execution_failure():
    assert worker_card_outcome("completed", '{"checks":[]}') == ("completed", None)
    assert worker_card_outcome("failed", '{"checks":[{"outcome":"failed"}]}') == ("failed", None)
    assert worker_card_outcome("completed", '{"checks":[{"outcome":"not_run"}]}') == ("completed", "Completed with issues")
    assert worker_card_outcome("failed", None) == ("failed", None)


def test_worker_result_projection_bounds_and_shapes_like_pipeline_reader():
    """Same ``{summary, checks}`` shape/bounds as Paseo's RPC-read projection
    (``integrations/paseo/server/pipeline.ts``'s ``clip``/checks logic), so
    an embedded card and a fallback RPC read render identically."""
    payload = {
        "summary": "s" * 900,
        "checks": [
            {"name": "n" * 400, "outcome": "failed", "blocker": "b" * 900},
            {"name": "ok", "outcome": "passed"},
            {"name": "", "outcome": None},
        ] + [{"name": f"extra-{i}", "outcome": "passed"} for i in range(40)],
    }
    projection = worker_result_projection(payload)
    assert projection["summary"] is not None
    assert len(projection["summary"]) <= 600
    assert len(projection["checks"]) == 30
    first = projection["checks"][0]
    assert len(first["name"]) <= 300
    assert first["outcome"] == "failed"
    assert len(first["blocker"]) <= 600
    assert projection["checks"][1] == {"name": "ok", "outcome": "passed"}
    # An empty/omitted name falls back to "check"; an omitted outcome to
    # "unverified" -- exactly pipeline.ts's own fallback values.
    assert projection["checks"][2]["name"] == "check"
    assert projection["checks"][2]["outcome"] == "unverified"


def test_worker_result_projection_scrubs_secrets():
    projection = worker_result_projection({
        "summary": "token: sk-abcdefghijklmnop",
        "checks": [{"name": "check", "outcome": "passed", "blocker": "password=hunter2"}],
    })
    assert "sk-abcdefghijklmnop" not in projection["summary"]
    assert "hunter2" not in (projection["checks"][0].get("blocker") or "")


def test_completed_worker_card_embeds_result_projection():
    """The terminal ACP card's ``rawOutput`` carries the same structured
    projection, so a client can render the result section without a
    follow-up RPC. See the primary design in the T-worker-timeline-instant
    assignment: Paseo's ``pipeline.ts`` RPC remains only the fallback."""
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
    runtime._worker_card_outcome = lambda task_id, assignment: ("completed", None)
    cards = []
    result = SimpleNamespace(
        event_type="worker_result", assignment_ref="attempt",
        payload={"summary": "Did the thing.",
                 "checks": [{"name": "tests pass", "outcome": "passed"},
                            {"name": "docs updated", "outcome": "not_run", "blocker": "no time"}],
                 "report_ref": "r_secret"},
    )
    runtime._emit_assignment_tool_calls("task", cards.append, [result])
    assert len(cards) == 1
    assert cards[0]["raw_output"] == {
        "cortexWorkerOutcome": {"status": "completed", "openIssues": 1},
        "summary": "Did the thing.",
        "checks": [
            {"name": "tests pass", "outcome": "passed"},
            {"name": "docs updated", "outcome": "not_run", "blocker": "no time"},
        ],
    }


def test_cancelled_assignment_card_carries_a_portable_cancelled_title_suffix():
    """ACP's ``ToolCallStatus`` has no ``cancelled`` value, so a cancelled
    assignment is rendered ``failed`` (see the module docstring near
    ``_TOOL_CALL_STATUS_FOR_CANCELLED``). A client with zero live pipeline
    data must still be able to tell it apart from a genuine failure, so this
    reuses T035's portable title channel -- the same one "Completed with
    issues" already uses -- instead of requiring an RPC. See Paseo's
    ``resolveWorkerCardStatus`` (client/worker-display.ts)."""
    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    runtime._lock = threading.RLock()
    runtime._worker_action_log = {}
    runtime._worker_action_refs = {}
    runtime._worker_card_sent = {}
    runtime._worker_card_terminal = {}
    runtime._emitted_worker_cards = set()
    runtime._worker_final_response_seen = set()
    runtime._worker_card_title = {"assignment": "Worker: debugger — Audit"}
    runtime._close_assignment = lambda task_id, assignment: None
    cards = []
    event = SimpleNamespace(
        event_type="assignment_cancelled", assignment_ref="assignment",
        payload={"reason": "superseded"},
    )
    runtime._emit_assignment_tool_calls("task", cards.append, [event])
    assert len(cards) == 1
    assert cards[0]["status"] == "failed"
    assert cards[0]["title"] == "Worker: debugger — Audit · Cancelled"






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

    assert runtime._delegation_route_label(task_id, "a_ref") == "codex · gpt-5-codex (alias) · medium"


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
    assert runtime._delegation_route_label(task_id, "e_ref") == "codex · gpt-5.6-luna (alias) · high"


def test_a_worker_card_shows_the_engine_observed_model_name_not_the_bare_alias(tmp_path):
    """T035: ``opus[1m]`` is a routing alias, not a version -- when
    ``live_adapters._resolve_route_model_name`` already wrote real,
    engine-observed evidence into the audit's own ``detail["model_name"]``
    at dispatch time, the card shows that resolved name (``Opus 5.5
    (1M)``) instead of the bare alias. The routing id itself (used for
    pin/deny) is unchanged in the audit's own ``model`` column -- only the
    displayed text differs."""
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
            ("cfg-m", task_id, 1, "route-1", "claude-agent-acp", "conn-1",
             _json.dumps({"effort": "medium"}), "{}", "confirmed", now, now),
        )
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, attempt_id, role_id, step, status, config_revision, "
            " owner_epoch, created_at, updated_at) VALUES (?,?,?,?,?,?,?,?,?,?)",
            ("m_ref", task_id, "att-m", "worker-backend-dev", "engine_session", "done", "cfg-m", 1, now, now),
        )
        db.commit()
    routing_store.record_attempt_audit(
        store, attempt_id="att-m", task_id=task_id, role="worker-backend-dev", engine="claude-agent-acp",
        connection_id="conn-1", model="opus[1m]", params_fingerprint="fp", config_revision_id="cfg-m",
        decision="authorized", detail={"effort": "medium", "model_name": "Opus 5.5 (1M)"},
    )

    assert runtime._delegation_route_label(task_id, "m_ref") == "claude · Opus 5.5 (1M) · medium"


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










# -- a worker that published its report and never said so -------------------

def test_a_silent_worker_that_published_a_report_delivers_it_instead_of_failing(tmp_path):
    """Live (2026-09-19, the user's own thread): a worker published its report
    through its own channel at 14:43:10, the runtime declared
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
