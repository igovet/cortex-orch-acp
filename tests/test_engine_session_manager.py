"""EngineSessionManager integration: negotiate/create_session/prompt_turn/
cancel/release, cross-engine isolation with identical wire ids, and the
distinct cancel/EOF/timeout/process-exit outcomes."""
from __future__ import annotations

import base64
import json
import time
from types import SimpleNamespace

import pytest

from cortex_runtime.connections import quota
from cortex_runtime.engine.claude_adapter_acp import claude_option_mapping
from cortex_runtime.engine.codex_adapter_acp import codex_option_mapping
from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.portable.engine_contract import EngineCapability, EngineEventKind, StopReason, UsageBasis
from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario

REQUESTED_CAPABILITIES = (
    EngineCapability.SESSION_CREATION,
    EngineCapability.PROMPT_STREAMING,
    EngineCapability.CANCELLATION,
    EngineCapability.CONFIGURATION_OPTIONS,
)


def _make_manager(tmp_path, scenario, *, call_timeout=5.0, abandon_wait_seconds=5.0, engine="codex-acp", option_mapping=None):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), engine=engine, environ=environ)
    return EngineSessionManager(
        record, str(tmp_path), environ=environ, call_timeout=call_timeout,
        abandon_wait_seconds=abandon_wait_seconds, option_mapping=option_mapping,
    )


def test_negotiate_create_session_and_prompt(tmp_path):
    manager = _make_manager(tmp_path, default_scenario())
    try:
        granted = manager.negotiate(REQUESTED_CAPABILITIES)
        assert EngineCapability.SESSION_CREATION in granted
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        assert session_id.startswith("es_")
        events = list(manager.prompt_turn(session_id, "hello"))
        deltas = [event.delta_text for event in events if event.kind == EngineEventKind.TURN_DELTA]
        assert deltas == ["hello ", "world"]
        assert events[-1].kind == EngineEventKind.TURN_END
        assert events[-1].stop_reason == StopReason.END_TURN
    finally:
        manager.shutdown(grace=2.0)


@pytest.mark.parametrize(
    ("engine", "effort_option_id", "option_mapping"),
    [
        ("codex-acp", "reasoning_effort", codex_option_mapping),
        ("claude-agent-acp", "effort", claude_option_mapping),
    ],
)
def test_configure_sends_the_effective_coordinator_default_effort(tmp_path, engine, effort_option_id, option_mapping):
    """The first-task revision's effective default reaches each adapter's wire.

    ``test_pending_pre_task_selection_is_proposed_on_first_prompt`` proves
    the ACP selector persists this exact ``thought_level=medium`` in the
    coordinator revision.  This integration check proves the two adapter
    mappings issue the matching ``session/set_config_option`` RPC.
    """
    request_log = tmp_path / "requests.jsonl"
    scenario = default_scenario(
        config_options=[
            dict(id="model", type="select", currentValue="model-a",
                 options=[dict(value="model-a", name="Model A")]),
            dict(id=effort_option_id, type="select", currentValue="low",
                 options=[dict(value=value, name=value) for value in ("low", "medium", "high")]),
        ],
        request_log_path=str(request_log),
    )
    manager = _make_manager(tmp_path, scenario, engine=engine, option_mapping=option_mapping)
    revision = ConfigurationRevision(
        id="cr_aaaaaaaaaaaa", task_id="t_aaaaaaaaaaaa", revision=1,
        route=ExecutionRoute(
            engine=engine, connection_id=manager._record.id, model="model-a",
            supported_parameters=(("thought_level", "medium"),),
        ),
    )
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        manager.configure(session_id, revision)
    finally:
        manager.shutdown(grace=2.0)

    calls = [json.loads(line) for line in request_log.read_text(encoding="utf-8").splitlines()]
    applied = [
        (call["params"]["configId"], call["params"]["value"])
        for call in calls if call.get("method") == "session/set_config_option"
    ]
    assert (effort_option_id, "medium") in applied


def test_image_blocks_forward_unchanged_on_initial_and_followup_turns(tmp_path):
    """The outgoing ACP client preserves image bytes and MIME on every turn.

    This exercises the same wire seam used by both the Codex and Claude ACP
    adapters, with the no-paid fake engine recording exact ``session/prompt``
    params.
    """
    prompt_log = tmp_path / "prompt-log.jsonl"
    scenario = default_scenario(prompt_log_path=str(prompt_log), prompt_updates=[])
    manager = _make_manager(tmp_path, scenario)
    image_bytes = b"\x00\x01opaque-image"
    image = dict(type="image", data=base64.b64encode(image_bytes).decode("ascii"), mimeType="image/jpeg")
    first_blocks = [dict(type="text", text="first A"), image, dict(type="text", text="first B")]
    followup_blocks = [dict(type="text", text="follow-up A"), image, dict(type="text", text="follow-up B")]
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        list(manager.prompt_turn(session_id, "first", first_blocks))
        list(manager.prompt_turn(session_id, "follow-up", followup_blocks))
    finally:
        manager.shutdown(grace=2.0)

    calls = [json.loads(line) for line in prompt_log.read_text().splitlines()]
    assert len(calls) == 2
    assert calls[0]["prompt"] == first_blocks
    assert calls[1]["prompt"] == followup_blocks


def test_usage_update_with_real_token_counts_is_engine_reported(tmp_path):
    """T035 FINDING-03: a `usage_update` notification carrying actual
    integer token fields is honestly attributed to the engine."""
    scenario = default_scenario(prompt_updates=[
        dict(delay=0.0, sessionUpdate="usage_update", usage=dict(inputTokens=12, outputTokens=3)),
        dict(delay=0.0, sessionUpdate="agent_message_chunk", content=dict(type="text", text="ok")),
    ])
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        events = list(manager.prompt_turn(session_id, "hello"))
        usage_events = [event for event in events if event.kind == EngineEventKind.USAGE_UPDATE]
        assert len(usage_events) == 1
        assert usage_events[0].usage_basis == UsageBasis.ENGINE_REPORTED
        assert usage_events[0].usage_values == dict(inputTokens=12, outputTokens=3)
    finally:
        manager.shutdown(grace=2.0)


def test_acp_context_fill_and_prompt_response_usage_are_carried(tmp_path):
    """ACP's own shapes: `usage_update` reports the context fill as top-level
    `used`/`size` (a display update, never token accounting), and the
    `session/prompt` response carries the turn's token counts."""
    scenario = default_scenario(
        prompt_updates=[
            dict(delay=0.0, sessionUpdate="usage_update", used=84000, size=200000),
            dict(delay=0.0, sessionUpdate="usage_update", used=5, size=0),
            dict(delay=0.0, sessionUpdate="agent_message_chunk", content=dict(type="text", text="ok")),
        ],
        prompt_usage=dict(inputTokens=1200, outputTokens=300, cachedReadTokens=900, totalTokens=1500, thoughtTokens=None),
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        events = list(manager.prompt_turn(session_id, "hello"))
    finally:
        manager.shutdown(grace=2.0)
    usage_events = [event for event in events if event.kind == EngineEventKind.USAGE_UPDATE]
    assert usage_events[0].display_update == dict(sessionUpdate="usage_update", used=84000, size=200000)
    assert usage_events[0].usage_basis == UsageBasis.UNKNOWN
    assert usage_events[1].display_update is None  # a zero-sized window is not a fill
    assert usage_events[-1].usage_basis == UsageBasis.ENGINE_REPORTED
    assert usage_events[-1].usage_values == dict(inputTokens=1200, outputTokens=300, cachedReadTokens=900, totalTokens=1500)
    assert events[-1].kind == EngineEventKind.TURN_END



def test_claude_rate_limit_and_per_turn_tokens_use_installed_adapter_shapes(tmp_path):
    """A Claude context notification carries no billable counts; its final
    PromptResponse does, and the forwarded rate limit updates the plan window.
    """
    reset = int(time.time()) + 3600
    scenario = default_scenario(
        prompt_updates=[
            dict(delay=0.0, sessionUpdate="usage_update", used=80, size=200000),
            dict(delay=0.0, sessionUpdate="usage_update", used=80, size=200000,
                 _meta={"_claude/rateLimit": {
                     "status": "allowed", "rateLimitType": "five_hour",
                     "utilization": 0.35, "resetsAt": reset,
                 }}),
        ],
        prompt_usage=dict(inputTokens=12, outputTokens=3, cachedReadTokens=50,
                          cachedWriteTokens=4, totalTokens=69),
    )
    manager = _make_manager(tmp_path, scenario, engine="claude-agent-acp")
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        events = list(manager.prompt_turn(session_id, "hello"))
        usage_events = [event for event in events if event.kind == EngineEventKind.USAGE_UPDATE]
        assert len(usage_events) == 3
        assert [event.usage_basis for event in usage_events] == [
            UsageBasis.UNKNOWN, UsageBasis.UNKNOWN, UsageBasis.ENGINE_REPORTED,
        ]
        assert usage_events[-1].usage_values == {
            "inputTokens": 12, "outputTokens": 3, "cachedReadTokens": 50,
            "cachedWriteTokens": 4, "totalTokens": 69,
        }
        assert quota.visible_windows(
            SimpleNamespace(engine="claude-agent-acp", id=manager._record.id),
            manager._environ,
        ) == [{"name": "five_hour", "usedPercent": 35.0, "resetsAt": reset}]
    finally:
        manager.shutdown(grace=2.0)


def test_usage_update_with_no_recognized_token_fields_is_unknown_not_engine_reported(tmp_path):
    """T035 FINDING-03: a `usage_update` notification whose `usage` payload
    yields no recognized integer token field (here: empty) must never be
    recorded as ENGINE_REPORTED -- that would defeat observability.usage's
    "unknown is never zero" protection for exactly the calls it exists to
    protect. See also test_observability_usage.py for the storage-layer
    half of this same protection."""
    scenario = default_scenario(prompt_updates=[
        dict(delay=0.0, sessionUpdate="usage_update", usage={}),
        dict(delay=0.0, sessionUpdate="agent_message_chunk", content=dict(type="text", text="ok")),
    ])
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        events = list(manager.prompt_turn(session_id, "hello"))
        usage_events = [event for event in events if event.kind == EngineEventKind.USAGE_UPDATE]
        assert len(usage_events) == 1
        assert usage_events[0].usage_basis == UsageBasis.UNKNOWN
        assert usage_events[0].usage_values == {}
    finally:
        manager.shutdown(grace=2.0)


def test_two_engines_with_identical_wire_ids_never_cross_route(tmp_path):
    """Both fake engines are scripted to use the exact same wire session id
    and toolCallId; each EngineSessionManager must only ever resolve wire
    ids inside its own CorrelationTables (PR acceptance bullet 1)."""
    scenario = default_scenario(
        session_id="same-session-id",
        prompt_updates=[
            dict(sessionUpdate="tool_call", toolCallId="same-tool-call-id"),
            dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="done")),
        ],
    )
    (tmp_path / "a").mkdir()
    (tmp_path / "b").mkdir()
    manager_a = _make_manager(tmp_path / "a", scenario)
    manager_b = _make_manager(tmp_path / "b", dict(scenario))
    try:
        manager_a.negotiate(REQUESTED_CAPABILITIES)
        manager_b.negotiate(REQUESTED_CAPABILITIES)
        session_a = manager_a.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        session_b = manager_b.create_session("t_bbbbbbbbbbbb", "xr_" + "1" * 24)
        assert session_a != session_b  # distinct portable ids despite identical wire ids

        events_a = list(manager_a.prompt_turn(session_a, "hi a"))
        events_b = list(manager_b.prompt_turn(session_b, "hi b"))

        tool_ref_a = next(event.tool_call_ref for event in events_a if event.kind == EngineEventKind.TOOL_CALL_OBSERVED)
        tool_ref_b = next(event.tool_call_ref for event in events_b if event.kind == EngineEventKind.TOOL_CALL_OBSERVED)
        assert tool_ref_a != tool_ref_b

        # Each manager's correlation table only ever knows its own session.
        assert manager_a.correlation.wire_session_id(session_b) is None
        assert manager_b.correlation.wire_session_id(session_a) is None
    finally:
        manager_a.shutdown(grace=2.0)
        manager_b.shutdown(grace=2.0)


def test_cancel_outcome_is_distinct_from_normal_completion(tmp_path):
    scenario = default_scenario(
        prompt_behavior="normal",
        prompt_updates=[dict(delay=0.3, sessionUpdate="agent_message_chunk", content=dict(type="text", text="slow"))],
        prompt_stop_reason="end_turn",
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        results = []

        def run():
            for event in manager.prompt_turn(session_id, "hi"):
                results.append(event)

        import threading
        worker = threading.Thread(target=run)
        worker.start()
        time.sleep(0.05)
        manager.cancel(session_id)
        worker.join(timeout=5.0)
        assert results[-1].kind == EngineEventKind.TURN_END
        assert results[-1].stop_reason == StopReason.CANCELLED
    finally:
        manager.shutdown(grace=2.0)


def test_prompt_classifies_observed_broker_close_before_tools(tmp_path):
    scenario = default_scenario(prompt_updates=[
        dict(delay=0.3, sessionUpdate="agent_message_chunk", content=dict(type="text", text="late")),
    ])
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        class ClosedBroker:
            def readiness(self, timeout):
                return "mcp_server_exited"
            def close(self):
                pass
        manager._process.mcp_broker = ClosedBroker()
        events = list(manager.prompt_turn(session_id, "hi"))
        assert events[-1].kind == EngineEventKind.ERROR
        assert events[-1].error_message == "cortex_mcp_connection_failed:mcp_server_exited"
    finally:
        manager.shutdown(grace=2.0)


def test_prompt_fails_named_when_a_ready_cortex_server_is_lost_mid_turn(tmp_path):
    """A Cortex MCP server lost after tools/list (the broker reports
    ``mcp_server_crashed``) cancels the running turn and names the loss. The
    turn must not end as a normal end_turn: the worker's result/question
    tools are gone, so a clean end would only become a same-session
    follow-up through dead tools and then ``worker_ended_without_result``."""
    scenario = default_scenario(prompt_updates=[
        dict(delay=0.1, sessionUpdate="agent_message_chunk", content=dict(type="text", text="working")),
        dict(delay=2.0, sessionUpdate="agent_message_chunk", content=dict(type="text", text="late")),
    ])
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        lost_after = time.monotonic() + 0.4

        class LosingBroker:
            def readiness(self, timeout):
                return "mcp_server_crashed" if time.monotonic() >= lost_after else "ready"
            def close(self):
                pass
        manager._process.mcp_broker = LosingBroker()
        events = list(manager.prompt_turn(session_id, "hi"))
        assert events[-1].kind == EngineEventKind.ERROR
        assert events[-1].error_message == "cortex_mcp_connection_failed:mcp_server_crashed"
        assert not any(event.kind == EngineEventKind.TURN_END for event in events)
    finally:
        manager.shutdown(grace=2.0)


def test_prompt_turn_is_never_bound_by_call_timeout_while_streaming(tmp_path):
    """T035 FINDING-06: ``session/prompt`` carries no wall-clock timeout at
    all -- a turn whose own total duration comfortably exceeds the old
    (and here, deliberately tiny) ``call_timeout`` must still complete
    normally, proving the prompt path is fully decoupled from the short
    RPC budget used by initialize/session/new/configure."""
    scenario = default_scenario(
        prompt_updates=[
            dict(delay=0.1, sessionUpdate="agent_message_chunk", content=dict(type="text", text="a")),
            dict(delay=0.1, sessionUpdate="agent_message_chunk", content=dict(type="text", text="b")),
            dict(delay=0.1, sessionUpdate="agent_message_chunk", content=dict(type="text", text="c")),
        ],
        prompt_stop_reason="end_turn",
    )
    # call_timeout=0.05 is far shorter than this turn's ~0.3s total
    # duration; under the old flat-timeout design this would have failed
    # with timeout_alive well before the engine ever replied.
    manager = _make_manager(tmp_path, scenario, call_timeout=0.05)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        events = list(manager.prompt_turn(session_id, "hi"))
        deltas = [event.delta_text for event in events if event.kind == EngineEventKind.TURN_DELTA]
        assert deltas == ["a", "b", "c"]
        assert events[-1].kind == EngineEventKind.TURN_END
        assert events[-1].stop_reason == StopReason.END_TURN
    finally:
        manager.shutdown(grace=2.0)


def test_timeout_alive_no_longer_produced_for_a_silent_prompt_turn(tmp_path):
    """T035 FINDING-06 (stronger requirement): a genuinely silent engine
    (no updates, no reply) is never itself classified timeout_alive by
    prompt_turn any more -- there is no clock left to produce that
    classification from this call. The turn simply blocks (proven here by
    a bounded wait that observes nothing happening) until cancelled (see
    test_cancel_is_the_only_way_out_of_a_silent_prompt_turn) or the
    process/transport actually ends (see
    test_process_exit_is_distinct_from_timeout)."""
    hang_scenario = default_scenario(prompt_behavior="hang")
    manager = _make_manager(tmp_path, hang_scenario, call_timeout=0.3)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        results = []

        def run():
            for event in manager.prompt_turn(session_id, "hi"):
                results.append(event)

        import threading
        # daemon=True: this call.timeout=0.3 governed *only* the old (now
        # removed) flat prompt timeout; the worker is left genuinely stuck
        # until manager.shutdown() below resolves it via transport EOF, not
        # any clock -- never joined synchronously here for that reason.
        worker = threading.Thread(target=run, daemon=True)
        worker.start()
        time.sleep(1.0)  # far past the old call_timeout=0.3
        assert worker.is_alive()  # still stuck: no timeout ever fired
        assert results == []  # no ERROR/TURN_END event was ever produced
        assert manager._process.alive()  # never proved dead by a timeout
    finally:
        manager.shutdown(grace=2.0)  # resolves the still-blocked call via EOF


def test_cancel_is_the_only_way_out_of_a_silent_prompt_turn(tmp_path):
    """T035 FINDING-06: with no timeout on session/prompt at all, an engine
    that streams nothing and replies to nothing except session/cancel can
    only ever be unstuck by cancellation -- prove that path works, and
    works promptly (bounded join well under the old 30s cap), on a turn
    that would otherwise hang indefinitely."""
    scenario = default_scenario(prompt_behavior="silent_until_cancel")
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        results = []

        def run():
            for event in manager.prompt_turn(session_id, "hi"):
                results.append(event)

        import threading
        worker = threading.Thread(target=run)
        worker.start()
        time.sleep(0.2)  # long enough to prove the turn is genuinely stuck
        assert worker.is_alive()
        manager.cancel(session_id)
        worker.join(timeout=5.0)
        assert not worker.is_alive()  # cancellation is prompt, not sluggish
        assert results[-1].kind == EngineEventKind.TURN_END
        assert results[-1].stop_reason == StopReason.CANCELLED
    finally:
        manager.shutdown(grace=2.0)


def test_process_exit_is_distinct_from_timeout(tmp_path):
    crash_scenario = default_scenario(prompt_behavior="crash")
    manager = _make_manager(tmp_path, crash_scenario, call_timeout=5.0)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        events = list(manager.prompt_turn(session_id, "hi"))
        assert events[-1].kind == EngineEventKind.ERROR
        assert events[-1].error_message == "transport_eof"
        assert manager._process.wait_exited(5.0)
        assert manager._process.exit_code == 7
    finally:
        manager.shutdown(grace=2.0)


def test_replay_updates_never_produce_events_or_mutate(tmp_path):
    scenario = default_scenario(
        replay_updates=[
            dict(sessionUpdate="user_message_chunk", content=dict(type="text", text="a previous user turn")),
            dict(sessionUpdate="user_message_chunk", content=dict(type="text", text="another previous turn")),
        ],
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        # The replay updates were emitted during session/new, before any
        # prompt_turn queue existed; they must never have been queued or
        # turned into an EngineEvent for any session.
        events = list(manager.prompt_turn(session_id, "first real prompt"))
        deltas = [event.delta_text for event in events if event.kind == EngineEventKind.TURN_DELTA]
        assert deltas == ["hello ", "world"]
        assert "a previous user turn" not in deltas
    finally:
        manager.shutdown(grace=2.0)


# --- F1 (T012 review, HIGH): abandoned turn never leaks into the next one --

def test_abandoned_turn_then_second_turn_gets_only_its_own_deltas(tmp_path):
    scenario = default_scenario(
        prompt_updates=[
            dict(delay=0.0, sessionUpdate="agent_message_chunk", content=dict(type="text", text="first-a")),
            dict(delay=0.3, sessionUpdate="agent_message_chunk", content=dict(type="text", text="first-b")),
        ],
        prompt_stop_reason="end_turn",
    )
    manager = _make_manager(tmp_path, scenario, abandon_wait_seconds=3.0)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)

        gen = manager.prompt_turn(session_id, "first prompt")
        first_event = next(gen)
        assert first_event.kind == EngineEventKind.TURN_DELTA
        assert first_event.delta_text == "first-a"
        gen.close()  # abandon mid-stream while the wire call is still in flight

        second_events = list(manager.prompt_turn(session_id, "second prompt"))
        deltas = [event.delta_text for event in second_events if event.kind == EngineEventKind.TURN_DELTA]
        assert deltas == ["first-a", "first-b"]  # only this turn's own deltas
        assert second_events[-1].kind == EngineEventKind.TURN_END
        assert second_events[-1].stop_reason == StopReason.END_TURN
    finally:
        manager.shutdown(grace=2.0)


def test_abandonment_with_engine_ignoring_cancel_poisons_the_session(tmp_path):
    scenario = default_scenario(
        prompt_behavior="one_update_then_hang",
        prompt_updates=[dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="only-update"))],
    )
    manager = _make_manager(tmp_path, scenario, abandon_wait_seconds=0.2)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)

        gen = manager.prompt_turn(session_id, "first prompt")
        first_event = next(gen)  # a real suspended-at-yield generator, not an unstarted one
        assert first_event.kind == EngineEventKind.TURN_DELTA
        # The fake engine never replies after this (and ignores our cancel,
        # per "one_update_then_hang"/"hang" semantics): abandoning here must
        # poison the session rather than let a next turn start.
        gen.close()

        import cortex_runtime.portable.errors as portable_errors
        with pytest.raises(portable_errors.PortableError) as excinfo:
            next(manager.prompt_turn(session_id, "second prompt"))
        assert excinfo.value.code == "engine_session_unusable"
        assert excinfo.value.retry_eligible is False
    finally:
        manager.shutdown(grace=2.0)


def test_no_leaked_threads_after_abandonment(tmp_path):
    import threading as _threading
    scenario = default_scenario(
        prompt_behavior="one_update_then_hang",
        prompt_updates=[dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text="only-update"))],
    )
    manager = _make_manager(tmp_path, scenario, abandon_wait_seconds=0.2)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        before = _threading.active_count()
        gen = manager.prompt_turn(session_id, "first prompt")
        next(gen)
        gen.close()
        time.sleep(0.3)
        after_abandon = _threading.active_count()
        # The abandon-call worker thread is a daemon bounded by call_timeout
        # (5.0s default here); it may still be alive briefly, but must not
        # accumulate across repeated abandonments.
        assert after_abandon <= before + 2
    finally:
        manager.shutdown(grace=2.0)


# --- F2 (T012 review, MEDIUM): release() during an in-flight prompt -------

def test_release_during_in_flight_prompt_yields_distinct_terminal_outcome(tmp_path):
    scenario = default_scenario(
        prompt_updates=[
            dict(delay=0.0, sessionUpdate="agent_message_chunk", content=dict(type="text", text="chunk-1")),
            dict(delay=0.5, sessionUpdate="agent_message_chunk", content=dict(type="text", text="chunk-2")),
        ],
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        events = []

        def run():
            for event in manager.prompt_turn(session_id, "hi"):
                events.append(event)

        import threading
        worker = threading.Thread(target=run)
        worker.start()
        time.sleep(0.05)
        manager.release(session_id)
        worker.join(timeout=5.0)
        assert not worker.is_alive()
        released_markers = [event for event in events if event.kind == EngineEventKind.ERROR and event.error_message == "released_mid_turn"]
        assert len(released_markers) == 1
    finally:
        manager.shutdown(grace=2.0)


def test_double_cancel_is_safe(tmp_path):
    manager = _make_manager(tmp_path, default_scenario())
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        manager.cancel(session_id)
        manager.cancel(session_id)  # must not raise, no duplicate-related error
    finally:
        manager.shutdown(grace=2.0)


def test_concurrent_prompt_turn_callers_on_one_session_serialize(tmp_path):
    scenario = default_scenario(
        prompt_updates=[dict(delay=0.1, sessionUpdate="agent_message_chunk", content=dict(type="text", text="x"))],
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        started_order = []
        finished_order = []
        lock_guard = []

        import threading

        def run(label):
            started_order.append(label)
            list(manager.prompt_turn(session_id, label))
            finished_order.append(label)

        first = threading.Thread(target=run, args=("first",))
        second = threading.Thread(target=run, args=("second",))
        first.start()
        time.sleep(0.02)
        second.start()
        first.join(timeout=5.0)
        second.join(timeout=5.0)
        assert not first.is_alive() and not second.is_alive()
        # Both turns completed without interleaving corruption; the second
        # call's own generator only ever ran after the first released the
        # session lock (serialized, never concurrent wire calls).
        assert finished_order == ["first", "second"]
    finally:
        manager.shutdown(grace=2.0)


def test_inbound_flood_with_slow_consumer_cancels_turn_with_observation(tmp_path):
    """A bounded update queue and a slow (never-draining, in this test's
    setup) consumer must never grow memory without bound: on overflow the
    turn is cancelled and an observation is recorded."""
    flood_updates = [
        dict(sessionUpdate="agent_message_chunk", content=dict(type="text", text=f"chunk-{index}"))
        for index in range(200)
    ]
    scenario = default_scenario(prompt_updates=flood_updates, prompt_stop_reason="end_turn")
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    manager = EngineSessionManager(record, str(tmp_path), environ=environ, call_timeout=10.0, update_queue_maxsize=4)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        events = []
        gen = manager.prompt_turn(session_id, "flood me")
        # Drain very slowly (well slower than the fake engine emits) so the
        # bounded queue (maxsize=4) overflows and the manager cancels.
        for event in gen:
            events.append(event)
            time.sleep(0.01)
        assert events[-1].kind == EngineEventKind.TURN_END
        observations = (manager._process.logs.directory / "observations.jsonl").read_text()
        assert "update_backpressure_overflow" in observations
    finally:
        manager.shutdown(grace=2.0)


def test_unsupported_configure_never_reaches_the_wire_per_request_log(tmp_path):
    """Raw-frame capability gating: when a session advertises no config
    options, configure() must never even send a session/set_config_option
    frame -- verified against the fake engine's own request log, not just
    the raised exception."""
    request_log = tmp_path / "requests.jsonl"
    scenario = default_scenario(config_options=[], request_log_path=str(request_log))
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        from cortex_runtime.configuration.service import ConfigurationRejected
        from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute
        route = ExecutionRoute(engine="codex-acp", connection_id="cn_" + "0" * 12, model="gpt-x")
        revision = ConfigurationRevision(id="cr_" + "0" * 12, task_id="t_" + "0" * 12, revision=1, route=route)
        with pytest.raises(ConfigurationRejected):
            manager.configure(session_id, revision)
        logged_methods = [
            __import__("json").loads(line)["method"] for line in request_log.read_text().splitlines() if line
        ]
        assert "session/set_config_option" not in logged_methods
    finally:
        manager.shutdown(grace=2.0)


def test_observed_tool_title_is_not_silently_truncated(tmp_path):
    manager = _make_manager(tmp_path, default_scenario())
    title = 'Actual engine tool title: ' + 'detail ' * 100
    event = manager._translate_update('wire', {'sessionUpdate': 'tool_call', 'title': title,
                                              'rawInput': {'path': 'calc.py'}, 'kind': 'read'})
    assert event.tool_call_title == title
    assert event.tool_call_raw_input == {'path': 'calc.py'}


@pytest.mark.parametrize("replacement", ["unresolved", "incomplete", "complete"])
def test_cancel_time_replacement_requires_fresh_broker_proof(tmp_path, replacement):
    # Real manager/watch/cancel wiring with a fake ACP process, and the actual
    # broker state machine without socket IO. No installed engine/model runs.
    import threading
    from cortex_runtime.engine.broker import McpBroker
    broker = object.__new__(McpBroker)
    broker._state_lock = threading.RLock()
    broker._generation = 0
    broker._ready = threading.Event()
    broker._closed = threading.Event()
    broker._stop = threading.Event()
    broker._failure = None
    broker._tools_receipt = None
    broker._observe = None
    broker.close = lambda: None
    tools = dict(result=dict(tools=[
        dict(name=name, inputSchema=dict(type="object"))
        for name in ("create_draft", "write_report")]))
    old = broker._begin_generation()
    broker._observe_tools_list(tools, 1, old)
    broker._server_exited(old, 1, 7)

    manager = _make_manager(tmp_path, default_scenario(prompt_updates=[
        dict(delay=1.0, sessionUpdate="agent_message_chunk", content=dict(type="text", text="working")),
    ]))
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        manager._process.mcp_broker = broker
        real_cancel = manager.cancel_wire
        cancellations = []
        def replace_during_cancel(wire):
            cancellations.append(wire)
            generation = broker._begin_generation()
            if replacement != "unresolved":
                broker._observe_tools_list(tools if replacement == "complete" else dict(result=dict(tools=[])),
                                           2, generation)
            # Old buffered replies arriving here must never heal the new state.
            broker._observe_tools_list(tools, 1, old)
            broker._server_exited(old, 1, 0)
            return real_cancel(wire)
        manager.cancel_wire = replace_during_cancel
        events = list(manager.prompt_turn(session_id, "hi"))
        assert len(cancellations) == 1
        if replacement == "complete":
            assert events[-1].kind == EngineEventKind.TURN_END
            assert broker.readiness(0) == "ready"
        else:
            assert events[-1].kind == EngineEventKind.ERROR
            assert events[-1].error_message == "cortex_mcp_connection_failed:mcp_server_crashed"
            assert not any(event.kind == EngineEventKind.TURN_END for event in events)
            assert broker.readiness(0) != "ready"
    finally:
        manager.shutdown(grace=2.0)
