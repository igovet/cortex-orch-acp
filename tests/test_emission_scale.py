"""No-paid scale bounds for everything Cortex streams to Paseo during a task.

Paseo 0.9.2 retains every provider event of a session, so the bytes Cortex
sends are the bytes its daemon keeps.  The live qualification in report
``r_236acb13fabb`` measured 287,938 update bytes for a three-worker task
against 107,579 for the baseline build; the host journal showed the growth was
not the worker cards but the coordinator polling ``task_get_context`` 57 times
(each output mirrored twice, as MCP text and ``structuredContent``, and resent
in full on every identical poll).  This test drives the real Agent, pipe
transport and runtime with the fake engine through N workers x short and very
long transcripts with large writes, plus that polling coordinator, and pins:

* every ``session/update`` fits the 32 KB hard cap;
* each worker card's total bytes stay under a fixed bound, independent of the
  worker's transcript length;
* each worker card's number of updates is a constant (lifecycle transitions
  plus a fixed progress budget) for 10, 60 or 300 tools and for a worker
  running for hours on a simulated clock -- the live qualification in report
  ``r_a3d87956eb0f`` found 296 worker-card updates for three workers against
  9 on the baseline, from one long title spelled two ways;
* repeated identical coordinator polls cost a small constant each;
* the small three-worker workload stays below the live baseline figure.

Per-tool_call-id byte accounting is printed (visible with ``-s`` and on
failure).
"""
from __future__ import annotations

import collections
import json
import time
import uuid

import pytest

from cortex_runtime.acp.orchestrating_runtime import PromptOutcome
from cortex_runtime.acp.wire_budget import SESSION_UPDATE_MAX_BYTES
from cortex_runtime.portable.engine_contract import StopReason

#: Cortex ``session/update`` bytes the baseline build (45c1951d) sent for the
#: live three-worker workload in report r_236acb13fabb.
LIVE_BASELINE_THREE_WORKER_BYTES = 107_579
#: Owner target for everything one worker's card costs the host.
PASEO_WORKER_CARD_MAX_BYTES = 20 * 1024
#: One repeated identical coordinator poll: its create plus its completion.
REPEATED_POLL_MAX_BYTES = 1024
POLLS = 20


def _update_bytes(update) -> int:
    return len(json.dumps(update, ensure_ascii=False, separators=(",", ":")).encode("utf-8"))


def _mcp_result(value):
    """The rawOutput shape codex-acp forwards for an MCP tools/call result."""
    text = json.dumps(value, ensure_ascii=False)
    return {"error": None, "result": {"_meta": None, "content": [{"type": "text", "text": text}],
                                      "structuredContent": value}}


def _publish_large_result(store, channel):
    """A worker result near the realistic upper end: long summary, many checks."""
    from cortex_runtime.delegation import mcp_tools

    draft = store.call("create_draft", dict(template="general"), channel.principal_thread_id,
                       channel.parent_principal_thread_id, trusted=True)
    mcp_tools.dispatch(store, channel, "draft_write", dict(
        draft_id=draft["draft_id"], request_key=str(uuid.uuid4()),
        content=draft["required_first_line"] + "\n\n# Inspection\n\n" + "Observed detail. " * 2000 + "\n"))
    report = store.call("write_report", dict(draft_id=draft["draft_id"], title="Inspection",
                                             summary="Fixture observation", author="explorer",
                                             request_key=str(uuid.uuid4())),
                        channel.principal_thread_id, channel.parent_principal_thread_id, trusted=True)
    checks = [dict(name=f"check {index} " + "n" * 150, outcome="not_run",
                   blocker="Fixture blocker: " + "b" * 900) for index in range(20)]
    mcp_tools.dispatch(store, channel, "result_submit", dict(
        report=report["report_id"], summary="Observed summary. " * 220, checks=checks,
        request_key=str(uuid.uuid4())))
    return PromptOutcome(StopReason.END_TURN, "Visible worker result")


#: A realistic long assignment title: the live regression needed a card title
#: over the 300-character bound (role, route and objective) to show.
LONG_OBJECTIVE = ("Create exactly two independent original Markdown notes on zero-trust "
                  "service-to-service authentication and authorization, each at least "
                  "eighteen hundred words, with threat model, key rotation, audit trail "
                  "and failure-mode sections, and cite no external sources. Area {index}.")


LIVE_ROUTE_LABEL = "codex · 6 Luna · high · light · hint ignored · band normal · quota 41% · 5h window"


def _scale_router(*, workers: int, tools: int, write_every: int, write_bytes: int):
    from runtime_scenarios import publish_result
    from cortex_runtime.delegation import mcp_tools

    phase = {"planned": False}

    def coordinator_behavior(store, channel, session_ref, message, call_index, *, on_tool_call):
        if not phase["planned"]:
            phase["planned"] = True
            plan = dict(
                tasks=[dict(id=f"n{index}", role="explorer", objective=LONG_OBJECTIVE.format(index=index),
                            dependencies=[], scope=["."], model="auto") for index in range(workers)],
                verification=[["python3", "-B", "-c", 'print("verified")']],
            )
            mcp_tools.dispatch(store, channel, "plan_submit",
                               dict(plan=plan, request_key=str(uuid.uuid4())))
            # The live coordinator kept polling while its own turn was still
            # open (its workers already run meanwhile); every poll is a card.
            envelope = {"server": "cortex", "tool": "task_get_context", "arguments": {}}
            for index in range(POLLS):
                context = mcp_tools.dispatch(store, channel, "task_get_context", {})
                ref = f"poll-{index}"
                on_tool_call(ref, "other", "mcp.cortex.task_get_context", "in_progress", None, None,
                             envelope, None)
                on_tool_call(ref, None, None, "completed", None, None, envelope, _mcp_result(context))
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
        for index in range(tools):
            ref = f"read-{index}"
            on_tool_call(ref, "read", "Read", "in_progress", None, None,
                         {"path": f"/fixture/src/module_{index:04d}.py"}, None)
            on_tool_call(ref, "read", "Read", "completed", None, None, None, {"content": "ok " * 200})
            if write_every and index % write_every == 0:
                on_tool_call(
                    f"write-{index}", "other", "Project write", "completed", None, None,
                    {"server": "cortex", "tool": "project_write",
                     "arguments": {"path": f"src/generated_{index}.py",
                                   "content": "code words " * (write_bytes // 11)}},
                    _mcp_result({"path": f"src/generated_{index}.py", "bytes": write_bytes}),
                )
            if index % 5 == 4:
                for word in range(60):
                    on_delta(f"word{word} ")
        _publish_large_result(store, channel)
        return PromptOutcome(StopReason.END_TURN, "Worker finished: " + "done " * 40)

    def router(store, channel, session_ref, launch, instructions):
        return coordinator_behavior if channel and channel.role == "coordinator" else worker_behavior

    return router


class _SimulatedClock:
    """A card clock on which every reading is ``step`` seconds later: a fast
    fixture run then covers a worker running for hours."""

    def __init__(self, step: float):
        self.now, self.step, self._lock = 0.0, step, __import__("threading").Lock()

    def __call__(self) -> float:
        with self._lock:
            self.now += self.step
            return self.now


def _run(tmp_path, *, workers: int, tools: int, seconds_per_reading: float | None = None):
    from test_acp_elicitation import _Harness, _project

    tmp_path.mkdir(parents=True, exist_ok=True)
    harness = _Harness(tmp_path, _scale_router(workers=workers, tools=tools, write_every=10,
                                               write_bytes=20_000))
    if seconds_per_reading is not None:
        harness.runtime._worker_card_clock = _SimulatedClock(seconds_per_reading)
    # The fake engine records no route; a live worker's card names its route
    # (engine, model, effort, rank notes), which put the live titles over the
    # 300-character bound.
    harness.runtime._delegation_route_label = lambda task_id, assignment_ref: LIVE_ROUTE_LABEL
    harness.start()
    try:
        init_id = harness.send("initialize", dict(
            protocolVersion=1, clientCapabilities={}, clientInfo=dict(name="paseo", version="test")))
        harness.response_for(init_id)
        new_id = harness.send("session/new", dict(cwd=str(_project(tmp_path)), mcpServers=[]))
        session_id = harness.response_for(new_id)["result"]["sessionId"]
        prompt_id = harness.send("session/prompt", dict(
            sessionId=session_id, prompt=[dict(type="text", text="Inspect it.")]))
        harness.response_for(prompt_id, timeout=120.0)

        def terminal_workers():
            with harness.stdout._lock:
                return {m["params"]["update"]["toolCallId"] for m in harness.stdout.lines
                        if m.get("method") == "session/update"
                        and str(m["params"]["update"].get("toolCallId", "")).startswith("worker:")
                        and m["params"]["update"].get("status") in {"completed", "failed"}}

        # Wait for the N planned workers to settle (the reviewer may follow).
        deadline = time.monotonic() + 180.0
        while len(terminal_workers()) < workers:
            assert time.monotonic() < deadline, "planned workers did not settle"
            time.sleep(0.05)
        with harness.stdout._lock:
            return [m["params"]["update"] for m in harness.stdout.lines
                    if m.get("method") == "session/update"]
    finally:
        harness.stop()


def _accounting(updates):
    per_id = collections.OrderedDict()
    for update in updates:
        key = update.get("toolCallId") or update.get("sessionUpdate")
        entry = per_id.setdefault(key, dict(count=0, bytes=0, largest=0, statuses=[]))
        size = _update_bytes(update)
        entry["count"] += 1
        entry["bytes"] += size
        entry["largest"] = max(entry["largest"], size)
        if update.get("status"):
            entry["statuses"].append(update["status"])
    return per_id


def _print_accounting(label, updates, per_id):
    total = sum(_update_bytes(u) for u in updates)
    print(f"\n[{label}] updates={len(updates)} bytes={total}")
    for key, entry in per_id.items():
        print(f"  {key[:72]:72s} n={entry['count']:3d} bytes={entry['bytes']:7d} "
              f"largest={entry['largest']:6d} {'/'.join(entry['statuses'])}")
    return total


#: (workers, label) -> (tools per worker, simulated seconds per clock reading).
#: "hours" replays the 60-tool worker on a clock where every card decision is
#: 20 s after the previous one, i.e. a worker running for hours.
SCALE_CASES = {
    (3, "short"): (10, None), (3, "long"): (60, None), (3, "huge"): (300, None),
    (3, "hours"): (60, 20.0),
    (10, "short"): (10, None), (10, "long"): (60, None),
}


def _scale_group(workers: int):
    """Keep every test reading one worker count's runs on one xdist worker
    (``--dist loadgroup``), so each run happens once per test session."""
    return pytest.mark.xdist_group(f"emission-scale-{workers}-workers")


def _scale_params():
    return [pytest.param(case, marks=_scale_group(case[0])) for case in SCALE_CASES]


class _LazyRuns(dict):
    """Runs a scale case on first use: a process only pays for the cases its
    tests read."""

    def __init__(self, tmp_path_factory):
        super().__init__()
        self._tmp_path_factory = tmp_path_factory

    def __missing__(self, case):
        workers, label = case
        tools, step = SCALE_CASES[case]
        updates = _run(self._tmp_path_factory.mktemp(f"w{workers}-{label}"), workers=workers, tools=tools,
                       seconds_per_reading=step)
        per_id = _accounting(updates)
        total = _print_accounting(f"{workers} workers, {label} ({tools} tools)", updates, per_id)
        self[case] = dict(updates=updates, per_id=per_id, total=total)
        return self[case]


@pytest.fixture(scope="module")
def scale_runs(tmp_path_factory):
    return _LazyRuns(tmp_path_factory)


def _worker_cards(per_id):
    return {key: entry for key, entry in per_id.items() if str(key).startswith("worker:")}


@pytest.mark.parametrize("case", _scale_params())
def test_every_update_fits_the_hard_cap(scale_runs, case):
    run = scale_runs[case]
    largest = max(_update_bytes(u) for u in run["updates"])
    assert largest <= SESSION_UPDATE_MAX_BYTES, largest


@pytest.mark.parametrize("workers", [pytest.param(count, marks=_scale_group(count)) for count in (3, 10)])
def test_worker_card_bytes_are_bounded_and_independent_of_transcript_length(scale_runs, workers):
    short = _worker_cards(scale_runs[(workers, "short")]["per_id"])
    longer = [_worker_cards(scale_runs[(count, label)]["per_id"]) for count, label in SCALE_CASES
              if count == workers and label != "short"]
    # All planned workers (plus any reviewer that already started) have cards.
    for cards in (short, *longer):
        assert len(cards) >= workers
        for key, entry in cards.items():
            assert entry["bytes"] <= PASEO_WORKER_CARD_MAX_BYTES, (key, entry)
            # One create, one start, one result; no redraw per tool or word.
            assert entry["count"] <= 4, (key, entry)
    # O(1) in transcript length: 6-30x more tools and writes add nothing.
    for cards in longer:
        assert max(e["bytes"] for e in cards.values()) <= max(e["bytes"] for e in short.values()) + 512


@pytest.mark.parametrize("case", _scale_params())
def test_repeated_identical_coordinator_polls_are_not_resent(scale_runs, case):
    workers, label = case
    per_id = scale_runs[(workers, label)]["per_id"]
    polls = [entry for key, entry in per_id.items()
             if str(key).startswith("call:coordinator:") and ":poll-" in str(key)]
    assert len(polls) == POLLS
    first = polls[0]
    # Workers start while the coordinator still polls, so a repeat carries
    # the context again only when it really changed; an unchanged repeat is
    # a small receipt.
    results = {}
    for update in scale_runs[(workers, label)]["updates"]:
        ref = str(update.get("toolCallId", ""))
        if ":poll-" in ref and isinstance(update.get("rawOutput"), dict) and "result" in update["rawOutput"]:
            results[ref.rsplit(":poll-", 1)[1]] = update["rawOutput"]["result"]["structuredContent"]
    previous = results["0"]
    unchanged = 0
    for index in range(1, POLLS):
        entry = next(e for k, e in per_id.items() if str(k).endswith(f":poll-{index}"))
        result = results.get(str(index))
        # No result at all: the emitter already elided a repeated output.
        if result is None or result.get("unchanged_since_last_read"):
            unchanged += 1
            assert entry["bytes"] <= REPEATED_POLL_MAX_BYTES, entry
        else:
            assert result != previous
            previous = result
    assert unchanged
    # The first poll carries the context once: structuredContent, no text copy.
    first_update = [u for u in scale_runs[(workers, label)]["updates"]
                    if str(u.get("toolCallId", "")).endswith(":poll-0") and u.get("rawOutput")][0]
    assert "content" not in first_update["rawOutput"]["result"]
    assert first_update["rawOutput"]["result"]["structuredContent"]["state"]
    assert first["bytes"] <= SESSION_UPDATE_MAX_BYTES


@_scale_group(3)
def test_small_workload_stays_below_the_live_baseline(scale_runs):
    assert scale_runs[(3, "short")]["total"] <= LIVE_BASELINE_THREE_WORKER_BYTES
    # A very long three-worker task costs the host no more than a short one
    # beyond the fixed per-tool coordinator budget (none here).
    for label in ("long", "huge", "hours"):
        assert scale_runs[(3, label)]["total"] <= scale_runs[(3, "short")]["total"] + 4 * 1024, label


#: Worker-card updates per worker in the live baseline (45c1951d) first wave
#: of report r_a3d87956eb0f: create, start, result (9 for three workers).
LIVE_BASELINE_UPDATES_PER_WORKER = 3


def _planned_worker_cards(run, workers):
    """The N planned workers' cards: every worker card carrying the plan's title."""
    from cortex_runtime.acp import orchestrating_runtime as runtime_module

    titles = {}
    for update in run["updates"]:
        key = str(update.get("toolCallId", ""))
        if key.startswith("worker:") and "Create exactly two" in str(update.get("title", "")):
            titles.setdefault(key, []).append(update["title"])
    cards = {key: entry for key, entry in run["per_id"].items() if key in titles}
    assert len(cards) == workers, sorted(cards)
    # The fixture reproduces the live shape: a title at the display bound.
    longest = max(len(title) for values in titles.values() for title in values)
    assert longest == runtime_module._WORKER_CARD_TITLE_MAX_CHARS, longest
    return cards


@pytest.mark.parametrize("case", _scale_params())
def test_worker_card_event_count_is_a_constant(scale_runs, case):
    """Paseo keeps every update, so the number of updates per worker must not
    grow with its tools, output or duration -- only lifecycle transitions and
    a fixed progress budget reach the host."""
    from cortex_runtime.acp import orchestrating_runtime as runtime_module

    workers, _label = case
    hard_bound = (3 + runtime_module._WORKER_CARD_PROGRESS_MAX_UPDATES
                  + runtime_module._WORKER_CARD_TERMINAL_REVISIONS)
    for key, entry in _worker_cards(scale_runs[case]["per_id"]).items():
        assert entry["count"] <= hard_bound, (key, entry)
    for key, entry in _planned_worker_cards(scale_runs[case], workers).items():
        # Paseo cards carry no body, so nothing but lifecycle reaches it.
        assert entry["count"] <= LIVE_BASELINE_UPDATES_PER_WORKER, (key, entry)
        # Every lifecycle transition is delivered, in order.
        assert entry["statuses"] == ["pending", "in_progress", "completed"], (key, entry)


@_scale_group(3)
def test_event_count_does_not_depend_on_tool_count_or_duration(scale_runs):
    def counts(case):
        return sorted(e["count"] for e in _planned_worker_cards(scale_runs[case], 3).values())

    assert counts((3, "short")) == counts((3, "long")) == counts((3, "huge")) == counts((3, "hours"))


@_scale_group(3)
def test_three_worker_wave_is_not_above_the_live_baseline_event_count(scale_runs):
    for label in ("short", "long", "huge", "hours"):
        cards = _planned_worker_cards(scale_runs[(3, label)], 3)
        assert sum(e["count"] for e in cards.values()) <= 3 * LIVE_BASELINE_UPDATES_PER_WORKER, (label, cards)
        assert max(e["bytes"] for e in cards.values()) <= PASEO_WORKER_CARD_MAX_BYTES


# -- Coordinator and engine tool cards: a per-task total bound ---------------
#
# Worker cards are bounded per worker above; a coordinator making N distinct
# tool calls used to send N retained cards.  These cases drive the real Agent
# over a pipe transport as a Paseo client, with the fake engine's coordinator
# making 10, 500 and 1,500 distinct calls over four coordinator prompts: a
# first turn that asks the user a question, the same turn's continuation
# after the answer (a new emitter), a follow-up turn, and a follow-up after a
# runtime restart (a new Agent and runtime on the same store, preceded by a
# session/load replay).  Every 97th call fails.  The largest case stays above
# the coordinator's LRU bookkeeping size so eviction is exercised.

COORDINATOR_CALL_CASES = (10, 500, 1500)
#: Coordinator prompts per case (see above) and the one that asks.
COORDINATOR_PROMPTS = 4
ASKING_PROMPT = 0


def _coordinator_calls_router(total: int, state: dict):
    from cortex_runtime.delegation import mcp_tools

    share = -(-total // COORDINATOR_PROMPTS)

    def coordinator(store, channel, session_ref, message, call_index, *, on_tool_call):
        prompt = state["prompts"]
        state["prompts"] += 1
        for index in range(prompt * share, min(total, (prompt + 1) * share)):
            ref = f"c{index}"
            on_tool_call(ref, "execute", "Terminal", "in_progress", None, None,
                         {"command": f"python3 -B -m pytest -q tests/test_module_{index:05d}.py"}, None)
            if index % 97 == 5:
                on_tool_call(ref, None, None, "failed", None, None, None, {"error": f"check failed {index}"})
            else:
                on_tool_call(ref, None, None, "completed", None, None, None, {"stdout": "ok " * 300})
        if prompt == ASKING_PROMPT:
            mcp_tools.dispatch(store, channel, "user_question_request", dict(
                question="Continue with the rest?", options=["Yes", "No"], request_key="ask-continue"))
            return PromptOutcome(StopReason.END_TURN, "")
        mcp_tools.dispatch(store, channel, "answer_submit",
                           dict(text=f"Answer {prompt}.", request_key=f"answer-{prompt}"))
        return PromptOutcome(StopReason.END_TURN, f"Answer {prompt}.")

    def router(store, channel, session_ref, launch, instructions):
        assert channel and channel.role == "coordinator", "no worker is planned"
        return coordinator

    return router


def _paseo_harness(tmp_path, router, step: float):
    from test_acp_elicitation import _Harness

    home = tmp_path.parent / f"{tmp_path.name}-home"
    home.mkdir(exist_ok=True)
    harness = _Harness(tmp_path, router, environ={
        "HOME": str(home), "XDG_CONFIG_HOME": str(home / ".config"),
        "XDG_STATE_HOME": str(home / ".local" / "state"), "CORTEX_PASEO_QUESTION_UI": "1"})
    harness.runtime._coordinator_card_clock = _SimulatedClock(step)
    harness.start()
    init_id = harness.send("initialize", dict(
        protocolVersion=1, clientCapabilities={}, clientInfo=dict(name="paseo", version="test")))
    harness.response_for(init_id)
    return harness


def _prompt(harness, session_id, text, *, answer_question=False):
    prompt_id = harness.send("session/prompt", dict(sessionId=session_id, prompt=[dict(type="text", text=text)]))
    question = None
    if answer_question:
        question = harness.wait_for(lambda m: m.get("method") == "session/request_permission", timeout=120.0)
        harness.send_raw(dict(jsonrpc="2.0", id=question["id"],
                              result=dict(outcome=dict(outcome="selected", optionId="cortex-question-1"))))
    response = harness.response_for(prompt_id, timeout=300.0)
    assert response["result"]["stopReason"] == "end_turn", response
    return question


def _session_updates(harness):
    with harness.stdout._lock:
        return [m["params"]["update"] for m in harness.stdout.lines if m.get("method") == "session/update"]


def _budget_sizes(harness, task_id):
    budget = harness.runtime._coordinator_card_budgets[task_id]
    return dict(seen=len(budget.seen), sent=len(budget.sent), outputs=len(budget.outputs))


def _run_coordinator(tmp_path, total: int, *, step: float = 1.0):
    from test_acp_elicitation import _project

    tmp_path.mkdir(parents=True, exist_ok=True)
    state = dict(prompts=0)
    router = _coordinator_calls_router(total, state)
    first = _paseo_harness(tmp_path, router, step)
    try:
        new_id = first.send("session/new", dict(cwd=str(_project(tmp_path)), mcpServers=[]))
        session_id = first.response_for(new_id)["result"]["sessionId"]
        question = _prompt(first, session_id, "Read the modules.", answer_question=True)
        _prompt(first, session_id, "And the rest?")
        task_id = first.agent.sessions[session_id]["task_id"]
        sizes = [_budget_sizes(first, task_id)]
        live = _session_updates(first)
    finally:
        first.stop()
    # Restart: a new Agent and runtime over the same store, as after the
    # Paseo daemon restarted Cortex.  The budget is seeded from the journal.
    second = _paseo_harness(tmp_path, router, step)
    try:
        load_id = second.send("session/load", dict(sessionId=session_id, cwd=str(_project(tmp_path)),
                                                   mcpServers=[]))
        second.response_for(load_id, timeout=120.0)
        replayed = _session_updates(second)
        _prompt(second, session_id, "One more pass.")
        after_restart = _session_updates(second)[len(replayed):]
        sizes.append(_budget_sizes(second, task_id))
    finally:
        second.stop()
    assert state["prompts"] == COORDINATOR_PROMPTS
    return dict(live=live + after_restart, after_restart=after_restart, replayed=replayed,
                question=question, sizes=sizes,
                session_id=session_id, task_id=task_id)


def _coordinator_cards(updates):
    from cortex_runtime.acp import wire_budget

    own = collections.OrderedDict()
    folded = []
    for update in updates:
        card_id = str(update.get("toolCallId", ""))
        if card_id.startswith(wire_budget.COORDINATOR_CARD_PREFIX):
            own.setdefault(card_id, []).append(update)
        elif card_id.startswith(wire_budget.FOLDED_CARD_PREFIX):
            folded.append(update)
    return own, folded


#: Every test reading ``coordinator_runs`` runs on one xdist worker.
_coordinator_group = pytest.mark.xdist_group("emission-scale-coordinator")


@pytest.fixture(scope="module")
def coordinator_runs(tmp_path_factory):
    runs = {}
    for total in COORDINATOR_CALL_CASES:
        run = _run_coordinator(tmp_path_factory.mktemp(f"coordinator-{total}"), total)
        own, folded = _coordinator_cards(run["live"])
        card_updates = [u for updates in own.values() for u in updates] + folded
        run.update(own=own, folded=folded, card_updates=len(card_updates),
                   card_bytes=sum(_update_bytes(u) for u in card_updates))
        print(f"\n[coordinator {total} calls] session/updates={len(run['live'])} "
              f"card updates={run['card_updates']} card bytes={run['card_bytes']} "
              f"own cards={len(own)} aggregate updates={len(folded)} "
              f"replayed updates={len(run['replayed'])} bookkeeping={run['sizes']}")
        if folded:
            print(f"  aggregate: {folded[-1]['title']}")
        runs[total] = run
    return runs


def _card_update_bound():
    from cortex_runtime.acp import wire_budget

    return ((wire_budget.COORDINATOR_CARD_BUDGET + wire_budget.COORDINATOR_ERROR_CARD_BUDGET)
            * wire_budget.COORDINATOR_CARD_MAX_UPDATES + wire_budget.COORDINATOR_FOLDED_MAX_UPDATES)


@_coordinator_group
@pytest.mark.parametrize("total", COORDINATOR_CALL_CASES)
def test_coordinator_tool_cards_are_bounded_per_task(coordinator_runs, total):
    from cortex_runtime.acp import wire_budget

    run = coordinator_runs[total]
    own, folded = run["own"], run["folded"]
    limit = wire_budget.COORDINATOR_CARD_BUDGET + wire_budget.COORDINATOR_ERROR_CARD_BUDGET
    assert len(own) <= limit
    assert len(own) == total if total <= wire_budget.COORDINATOR_CARD_BUDGET else len(own) >= wire_budget.COORDINATOR_CARD_BUDGET
    assert all(len(updates) <= wire_budget.COORDINATOR_CARD_MAX_UPDATES for updates in own.values())
    assert len(folded) <= wire_budget.COORDINATOR_FOLDED_MAX_UPDATES
    assert run["card_updates"] <= _card_update_bound()
    assert max(_update_bytes(u) for u in run["live"]) <= SESSION_UPDATE_MAX_BYTES
    # The restarted runtime seeded its budget from the journal: past the
    # budget, the only new own cards are failures within the error budget.
    if total > wire_budget.COORDINATOR_CARD_BUDGET:
        own_after, _ = _coordinator_cards(run["after_restart"])
        assert all(updates[-1]["status"] == "failed" for updates in own_after.values()), own_after.keys()
    # Bookkeeping stays within its fixed LRU sizes, whatever the call count.
    from cortex_runtime.acp import orchestrating_runtime as runtime_module
    for sizes in run["sizes"]:
        assert sizes["seen"] <= runtime_module._COORDINATOR_SEEN_MEMORY
        assert sizes["sent"] <= runtime_module._COORDINATOR_SEEN_MEMORY
        assert sizes["outputs"] <= runtime_module._COORDINATOR_REPEAT_MEMORY


@_coordinator_group
def test_coordinator_card_cost_does_not_grow_with_the_call_count(coordinator_runs):
    from cortex_runtime.acp import orchestrating_runtime as runtime_module, wire_budget

    small, large = coordinator_runs[500], coordinator_runs[1500]
    assert max(COORDINATOR_CALL_CASES) > runtime_module._COORDINATOR_SEEN_MEMORY
    # Three times more calls add at most the error-card budget (a failure past
    # the budget keeps its own card) and the aggregate card's fixed updates.
    extra_updates = (wire_budget.COORDINATOR_ERROR_CARD_BUDGET * wire_budget.COORDINATOR_CARD_MAX_UPDATES
                     + wire_budget.COORDINATOR_FOLDED_MAX_UPDATES)
    assert large["card_updates"] <= small["card_updates"] + extra_updates
    largest_card = max(_update_bytes(u) for u in large["live"]
                       if str(u.get("toolCallId", "")).startswith("call:"))
    assert large["card_bytes"] <= small["card_bytes"] + extra_updates * largest_card
    # The whole 1,500-call task costs the host well under one megabyte.
    assert large["card_bytes"] <= 1024 * 1024, large["card_bytes"]


@_coordinator_group
def test_folded_calls_are_counted_on_one_aggregate_card(coordinator_runs):
    from cortex_runtime.acp import wire_budget

    assert coordinator_runs[10]["folded"] == []
    for total in (500, 1500):
        run = coordinator_runs[total]
        folded = run["folded"]
        # One card per task: created once, then only updated -- also after
        # the restart, which must not grant a second create or budget.
        assert len({u["toolCallId"] for u in folded}) == 1
        assert [u["sessionUpdate"] for u in folded].count("tool_call") == 1
        assert folded[0]["sessionUpdate"] == "tool_call"
        assert all(u["status"] == "completed" for u in folded)
        last = folded[-1]["rawOutput"]
        if last["final"]:
            assert len(folded) == wire_budget.COORDINATOR_FOLDED_MAX_UPDATES
            assert "count no longer updated" in folded[-1]["title"]
        else:
            # Turn end sent the exact count: every call is on a card or counted.
            assert last["furtherToolCalls"] + len(run["own"]) == total


def test_aggregate_card_interval_doubles_and_its_updates_are_capped():
    """On a clock where every fold is 10 s after the previous one, the
    aggregate card waits 30 s, then 60 s, 120 s ... between updates, sends at
    most COORDINATOR_FOLDED_MAX_UPDATES in total and says when it stops."""
    from cortex_runtime.acp import wire_budget
    from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime

    runtime = OrchestratingRuntime.__new__(OrchestratingRuntime)
    clock = runtime._coordinator_card_clock = _SimulatedClock(10.0)
    cards = []
    emit = runtime._engine_tool_call_emitter(cards.append)
    budget = wire_budget.COORDINATOR_CARD_BUDGET

    def call(index):
        emit(f"c{index}", "execute", "Terminal", "completed", None, None, {"command": str(index)}, {"ok": 1})

    def folded():
        return [c for c in cards if c["tool_call_id"].startswith(wire_budget.FOLDED_CARD_PREFIX)]

    for index in range(budget + 100):
        call(index)
    counts = [c["raw_output"]["furtherToolCalls"] for c in folded()]
    # Created at the first folded call, then 3 folds (30 s), 6 (60 s), 12 ...
    assert counts == [1, 4, 10, 22, 46, 94], counts
    # Months later, every fold is past any interval: the cap still holds.
    clock.step = 10.0 ** 8
    for index in range(budget + 100, budget + 200):
        call(index)
    folded = folded()
    assert len(folded) == wire_budget.COORDINATOR_FOLDED_MAX_UPDATES
    assert folded[-1]["raw_output"]["final"] is True
    assert "count no longer updated" in folded[-1]["title"]
    emit.close_group()
    assert len(cards) == budget + len(folded), "nothing is sent after the cap"


@_coordinator_group
@pytest.mark.parametrize("total", COORDINATOR_CALL_CASES)
def test_card_lifecycle_question_and_failures_survive_the_budget(coordinator_runs, total):
    from cortex_runtime.acp import wire_budget

    run = coordinator_runs[total]
    # The user question is a host request, never a folded card, even after
    # thousands of calls.
    question = run["question"]
    assert question is not None
    assert question["params"]["toolCall"]["toolCallId"].startswith("cortex-question:")
    for card_id, updates in run["own"].items():
        # Created before it is updated, and settled.
        assert updates[0]["sessionUpdate"] == "tool_call", card_id
        assert all(u["sessionUpdate"] == "tool_call_update" for u in updates[1:]), card_id
        assert updates[-1]["status"] in {"completed", "failed"}, card_id
    failed = [card_id for card_id, updates in run["own"].items() if updates[-1]["status"] == "failed"]
    expected = [index for index in range(total) if index % 97 == 5]
    past_budget = [index for index in expected if index >= wire_budget.COORDINATOR_CARD_BUDGET]
    assert len(failed) == len(expected) - len(past_budget) + min(
        len(past_budget), wire_budget.COORDINATOR_ERROR_CARD_BUDGET)
    for card_id in failed:
        assert run["own"][card_id][-1]["rawOutput"]["error"].startswith("check failed")


@_coordinator_group
@pytest.mark.parametrize("total", COORDINATOR_CALL_CASES)
def test_session_load_replay_respects_the_same_bound(coordinator_runs, total):
    from cortex_runtime.acp import wire_budget

    run = coordinator_runs[total]
    replayed_own, replayed_folded = _coordinator_cards(run["replayed"])
    # Replay reproduces what the first runtime sent, no more.
    first_runtime = run["live"][:len(run["live"]) - len(run["after_restart"])]
    live_own, live_folded = _coordinator_cards(first_runtime)
    assert set(replayed_own) == set(live_own)
    assert len(replayed_folded) <= len(live_folded)
    assert len(replayed_own) <= wire_budget.COORDINATOR_CARD_BUDGET + wire_budget.COORDINATOR_ERROR_CARD_BUDGET
    assert len(replayed_folded) <= wire_budget.COORDINATOR_FOLDED_MAX_UPDATES
    assert sum(len(v) for v in replayed_own.values()) + len(replayed_folded) <= _card_update_bound()
    assert all("cortexReplay" in (u.get("_meta") or {}) for u in run["replayed"]
               if str(u.get("toolCallId", "")).startswith("call:"))


def test_legacy_journal_replay_is_folded_to_the_bound(tmp_path):
    """A journal written before the budget (one card per call) replays with
    the live bound: the first cards as recorded, the rest on one card."""
    from test_acp_elicitation import _project
    from cortex_runtime.acp import replay_store, wire_budget

    state = dict(prompts=ASKING_PROMPT + 1)
    first = _paseo_harness(tmp_path, _coordinator_calls_router(4, state), 1.0)
    try:
        new_id = first.send("session/new", dict(cwd=str(_project(tmp_path)), mcpServers=[]))
        session_id = first.response_for(new_id)["result"]["sessionId"]
        _prompt(first, session_id, "Read the modules.")
    finally:
        first.stop()
    legacy = 500
    for index in range(legacy):
        card_id = f"{wire_budget.COORDINATOR_CARD_PREFIX}legacy:{index}"
        replay_store.append_host_update(first.store, session_id, dict(
            sessionUpdate="tool_call", toolCallId=card_id, title="Read", kind="read", status="in_progress",
            rawInput={"path": f"/legacy/{index}"}))
        replay_store.append_host_update(first.store, session_id, dict(
            sessionUpdate="tool_call_update", toolCallId=card_id,
            status="failed" if index % 10 == 0 else "completed", rawOutput={"content": "legacy " * 50}))
    second = _paseo_harness(tmp_path, _coordinator_calls_router(4, state), 1.0)
    try:
        load_id = second.send("session/load", dict(sessionId=session_id, cwd=str(_project(tmp_path)),
                                                   mcpServers=[]))
        second.response_for(load_id, timeout=120.0)
        replayed = _session_updates(second)
    finally:
        second.stop()
    own, folded = _coordinator_cards(replayed)
    limit = wire_budget.COORDINATOR_CARD_BUDGET + wire_budget.COORDINATOR_ERROR_CARD_BUDGET
    # Unfolded, the journal would replay well past the bound asserted below.
    assert 2 * legacy > 2 * limit + 50
    print(f"\n[legacy replay] journal cards={legacy + 1} replayed updates={len(replayed)} "
          f"own cards={len(own)} aggregate={folded[-1]['title'] if folded else None}")
    assert len(own) == limit
    assert len(folded) == 1 and folded[0]["sessionUpdate"] == "tool_call"
    assert folded[0]["_meta"]["cortexReplay"] == dict(
        sessionId=session_id, sequences=[], foldedCards=legacy + 1 - limit)
    assert folded[0]["rawOutput"]["furtherToolCalls"] + len(own) == legacy + 1
    assert len(replayed) <= 2 * limit + 50


# Byte-first restart: report r_f9afa07c6a12 (REQUEST_CHANGES) found that
# `_seed_coordinator_card_budget` restored `shown` but not `shown_bytes`, so a
# task that crossed the 4 MiB byte budget before its 150th distinct call could
# emit another ~4 MiB tranche after every Cortex restart. These rounds issue
# large-payload calls, so the first round alone crosses the byte budget while
# still under the card-count limit, then restart twice with more distinct
# calls that must all fold instead of reopening the budget.

BYTES_FIRST_ROUND_CALLS = (140, 50, 50)
#: Just under the per-field cap (``_COORDINATOR_CARD_FIELD_MAX_BYTES``), so
#: ``bound_value`` returns it verbatim instead of shrinking it to a small
#: fraction of the cap.
BYTES_FIRST_PAYLOAD_BYTES = 15_900


def _big_text(size: int, tag) -> str:
    """``size`` bytes of ordinary varied words tagged with ``tag``, so
    every call's payload differs (the repeat-output cache must not fold
    these into a tiny reference) and the credential scrubber's regexes --
    which are slow only on pathological single-character runs, not real
    text -- stay fast."""
    words = [f"call-{tag} "]
    total = len(words[0])
    index = 0
    while total < size:
        word = f"word{index} "
        words.append(word)
        total += len(word)
        index += 1
    return "".join(words)[:size]


def _coordinator_bytes_first_router(payload_bytes: int, state: dict):
    from cortex_runtime.delegation import mcp_tools

    def coordinator(store, channel, session_ref, message, call_index, *, on_tool_call):
        first = state["invocations"] == 0
        state["invocations"] += 1
        # A restarted runtime may reconnect and invoke the coordinator once
        # on its own before this fixture's own explicit prompt; each
        # invocation (auto or explicit) simply consumes the next queued
        # round, so this stays correct regardless of exactly how many
        # invocations a restart produces.
        count = state["remaining_rounds"].pop(0) if state["remaining_rounds"] else 0
        for _ in range(count):
            ref = f"b{state['issued']}"
            tag = state["issued"]
            state["issued"] += 1
            on_tool_call(ref, "execute", "Terminal", "in_progress", None, None,
                         _big_text(payload_bytes, tag), None)
            on_tool_call(ref, None, None, "completed", None, None, None, _big_text(payload_bytes, tag))
        if first:
            # A structured response is required every turn; asking a
            # question here (never a plan) keeps this a coordinator-only
            # fixture, as in ``_coordinator_calls_router``.
            mcp_tools.dispatch(store, channel, "user_question_request", dict(
                question="Continue with the rest?", options=["Yes", "No"], request_key="ask-continue"))
            return PromptOutcome(StopReason.END_TURN, "")
        mcp_tools.dispatch(store, channel, "answer_submit", dict(
            text=f"Invocation {state['invocations']}.", request_key=f"answer-{state['invocations']}"))
        return PromptOutcome(StopReason.END_TURN, f"Invocation {state['invocations']}.")

    def router(store, channel, session_ref, launch, instructions):
        assert channel and channel.role == "coordinator", "no worker is planned"
        return coordinator

    return router


def _run_coordinator_bytes_first(tmp_path, rounds: tuple, payload_bytes: int):
    from test_acp_elicitation import _project

    tmp_path.mkdir(parents=True, exist_ok=True)
    state = dict(remaining_rounds=list(rounds), invocations=0, issued=0)
    router = _coordinator_bytes_first_router(payload_bytes, state)
    session_id = None
    task_id = None
    round_updates = []
    sizes = []
    for round_index in range(len(rounds)):
        harness = _paseo_harness(tmp_path, router, 1.0)
        try:
            if round_index == 0:
                new_id = harness.send("session/new", dict(cwd=str(_project(tmp_path)), mcpServers=[]))
                session_id = harness.response_for(new_id)["result"]["sessionId"]
                replayed = []
            else:
                load_id = harness.send("session/load", dict(
                    sessionId=session_id, cwd=str(_project(tmp_path)), mcpServers=[]))
                harness.response_for(load_id, timeout=120.0)
                replayed = _session_updates(harness)
            _prompt(harness, session_id, f"Round {round_index}.", answer_question=(round_index == 0))
            all_updates = _session_updates(harness)
            round_updates.append(all_updates[len(replayed):])
            task_id = harness.agent.sessions[session_id]["task_id"]
            sizes.append(_budget_sizes(harness, task_id))
        finally:
            harness.stop()
    assert state["issued"] == sum(rounds), state
    return dict(round_updates=round_updates, sizes=sizes, session_id=session_id, task_id=task_id)


def test_byte_budget_survives_restart_before_the_card_count_limit(tmp_path_factory):
    """Reach the byte budget before 150 distinct cards, restart twice, and
    confirm the total emitted bytes/updates stay within the same per-task
    constants instead of growing by another tranche on every restart."""
    from cortex_runtime.acp import wire_budget

    tmp_path = tmp_path_factory.mktemp("coordinator-bytes-first")
    run = _run_coordinator_bytes_first(tmp_path, BYTES_FIRST_ROUND_CALLS, BYTES_FIRST_PAYLOAD_BYTES)

    first_own, first_folded = _coordinator_cards(run["round_updates"][0])
    first_bytes = (sum(_update_bytes(u) for updates in first_own.values() for u in updates)
                   + sum(_update_bytes(u) for u in first_folded))
    print(f"\n[bytes-first] round0 own cards={len(first_own)} bytes={first_bytes} "
          f"sizes={run['sizes']}")
    # The first round alone crosses the byte budget while still under the
    # card-count limit: this is the scenario the restart seeding must survive.
    assert len(first_own) < wire_budget.COORDINATOR_CARD_BUDGET
    assert first_bytes >= wire_budget.COORDINATOR_CARD_BYTES_BUDGET

    total_own_ids = set(first_own)
    for round_index, later_updates in enumerate(run["round_updates"][1:], start=1):
        later_own, later_folded = _coordinator_cards(later_updates)
        # A restarted runtime seeds shown_bytes from the durable journal, so
        # none of this round's new distinct calls get their own card.
        assert later_own == {}, (round_index, sorted(later_own))
        later_bytes = sum(_update_bytes(u) for u in later_folded)
        assert later_bytes < 8 * 1024, (round_index, later_bytes)
        total_own_ids.update(later_own)
    # No restart grows the set of own cards past what the first round showed.
    assert total_own_ids == set(first_own)
    for sizes in run["sizes"]:
        from cortex_runtime.acp import orchestrating_runtime as runtime_module
        assert sizes["seen"] <= runtime_module._COORDINATOR_SEEN_MEMORY
        assert sizes["sent"] <= runtime_module._COORDINATOR_SEEN_MEMORY
