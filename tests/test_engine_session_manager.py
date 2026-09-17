"""EngineSessionManager integration: negotiate/create_session/prompt_turn/
cancel/release, cross-engine isolation with identical wire ids, and the
distinct cancel/EOF/timeout/process-exit outcomes."""
from __future__ import annotations

import time

import pytest

from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.portable.engine_contract import EngineCapability, EngineEventKind, StopReason

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario

REQUESTED_CAPABILITIES = (
    EngineCapability.SESSION_CREATION,
    EngineCapability.PROMPT_STREAMING,
    EngineCapability.CANCELLATION,
    EngineCapability.CONFIGURATION_OPTIONS,
)


def _make_manager(tmp_path, scenario, *, call_timeout=5.0, abandon_wait_seconds=5.0):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    return EngineSessionManager(
        record, str(tmp_path), environ=environ, call_timeout=call_timeout, abandon_wait_seconds=abandon_wait_seconds,
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


def test_timeout_alive_is_distinct_from_process_exit(tmp_path):
    hang_scenario = default_scenario(prompt_behavior="hang")
    manager = _make_manager(tmp_path, hang_scenario, call_timeout=0.3)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        events = list(manager.prompt_turn(session_id, "hi"))
        assert events[0].kind == EngineEventKind.ERROR
        assert events[0].error_message == "timeout_alive"
        assert manager._process.alive()  # timeout alone never proves death
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
