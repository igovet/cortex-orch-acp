"""Replays of the live post-result engine kill that lost worker/reviewer tokens.

Retained v4b evidence (dev daemon engine observations, metadata only): all 68
worker and reviewer engines logged ``jail_authority_revoked`` 0.04-1.01 s after
their accepted result, then ``process_exited`` -15, ``transport_eof`` and
``prompt_turn_abandoned``. Result acceptance revokes the attempt's MCP channel
(``orchestration.state``), and the 1 s authority watchdog of the jailed engine
(``engine.process.EngineProcess._watch_authority``) treated that like any other
revocation and SIGTERMed the engine before its final ``PromptResponse.usage``.

These tests run the real watchdog method and the real ``_jail_for`` authority
probe against a scripted engine subprocess, in the live order: result_submit
publishes ``worker_result``, acceptance revokes the channel, the controller
calls ``finish_owned``. No paid engine is involved.
"""
from __future__ import annotations

import json
import threading
import time

import pytest

from cortex_runtime.acp.orchestrating_runtime import EngineTurnFailed
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import live_adapters
from cortex_runtime.delegation.live_adapters import _jail_for
from cortex_runtime.observability import usage
from cortex_runtime.trusted_channel import issue_channel, revoke_channel

from engine_fakes import default_scenario
from test_engine_session_manager import REQUESTED_CAPABILITIES, _make_manager
from test_observability_usage import _task
from test_token_coverage_live_shape import TOKENS, _completion_adapter, _settlement_events

# Streaming usage_update carries context occupancy only, as both installed
# adapters send it; the token counts exist only on the settled PromptResponse.
_CONTEXT_ONLY = dict(sessionUpdate="usage_update", used=84000, size=200000)


def _worker_channel(store, task_id, attempt):
    with store.connection() as db:
        parent = db.execute(
            "SELECT channel_id, owner_epoch FROM trusted_channels WHERE task_id=? AND role='coordinator'",
            (task_id,)).fetchone()
    return issue_channel(store, task_id, "worker", parent["owner_epoch"],
                         parent_channel_id=parent["channel_id"], attempt_id=attempt)


def _publish_result(store, task_id, attempt, channel_id):
    delegation_events.append_event(store, task_id, "worker_result", f"result:{attempt}", {},
                                   origin=channel_id, assignment_ref=attempt)


def _accept_result(store, task_id, attempt, channel_id):
    """The live order: result_submit publishes, then acceptance revokes."""
    _publish_result(store, task_id, attempt, channel_id)
    revoke_channel(store, channel_id)


def _start_authority_watch(manager, jail):
    # EngineProcess starts exactly this method for a jailed engine; it is
    # started directly here so the replay needs no bubblewrap jail.
    watcher = threading.Thread(target=manager._process._watch_authority, args=(jail["alive"],),
                               name="test-engine-authority", daemon=True)
    watcher.start()
    return watcher


class _Turn:
    """Runs adapter.prompt on its own thread, like the worker future."""

    def __init__(self, adapter, session):
        self.observed = threading.Event()
        self.errors = []
        self.thread = threading.Thread(target=self._run, args=(adapter, session))
        self.thread.start()

    def _run(self, adapter, session):
        try:
            adapter.prompt(session, "replay", threading.Event(),
                           on_display_update=lambda _: self.observed.set())
        except BaseException as exc:  # noqa: BLE001 - asserted by the test
            self.errors.append(exc)


def _live(tmp_path, engine, attempt, scenario):
    project = tmp_path / "project"
    project.mkdir()
    store, task_id = _task(project)
    manager = _make_manager(tmp_path, scenario, engine=engine)
    manager.negotiate(REQUESTED_CAPABILITIES)
    session = manager.create_session(task_id, "route")
    channel_id, secret = _worker_channel(store, task_id, attempt)
    jail = _jail_for(store, (channel_id, secret, str(project)))
    _start_authority_watch(manager, jail)
    adapter = _completion_adapter(store, task_id, attempt, session, manager)
    return store, task_id, manager, session, channel_id, adapter


@pytest.mark.parametrize("engine", TOKENS)
@pytest.mark.parametrize("attempt", ["xa_worker", "xa_review"])
@pytest.mark.parametrize("reply_delay", [1.5, 3.5], ids=["response_after_result", "late_response"])
def test_prompt_response_usage_after_accepted_result_is_recorded(tmp_path, engine, attempt, reply_delay):
    """result_submit is accepted mid-turn; the engine's final PromptResponse,
    the only token carrier, arrives after the authority watchdog's next poll."""
    store, task_id, manager, session, channel_id, adapter = _live(
        tmp_path, engine, attempt, default_scenario(
            prompt_updates=[_CONTEXT_ONLY], prompt_behavior="late_reply",
            late_reply_delay=reply_delay, prompt_usage=TOKENS[engine]))
    try:
        adapter._RESULT_SETTLE_SECONDS = 6.0
        turn = _Turn(adapter, session)
        assert turn.observed.wait(3)
        _accept_result(store, task_id, attempt, channel_id)
        adapter.finish_owned(attempt)
        cleanup = adapter._result_settlements.get(attempt)
        turn.thread.join(10)
        assert not turn.thread.is_alive()
        if cleanup is not None:
            cleanup.join(5)
        assert turn.errors == []
        row, = usage.list_usage(store, task_id)
        assert (row["attempt_id"], row["engine_session_ref"]) == (attempt, session)
        assert row["basis"] == "engine_reported"
        assert row["tokens"] == TOKENS[engine]
        assert row["stop_reason"] == "end_turn" and row["failed"] is False
        assert [event["outcome"] for event in _settlement_events(store, attempt)] == ["prompt_recorded"]
        assert manager._process.wait_exited(3)  # the settlement still tears the engine down
    finally:
        manager.shutdown(grace=0.5)


@pytest.mark.parametrize("engine", TOKENS)
def test_usage_that_never_arrives_stays_unknown_and_settlement_reaps(tmp_path, engine):
    store, task_id, manager, session, channel_id, adapter = _live(
        tmp_path, engine, "xa_never", default_scenario(
            prompt_updates=[_CONTEXT_ONLY], prompt_behavior="one_update_then_hang"))
    try:
        adapter._RESULT_SETTLE_SECONDS = 2.5
        turn = _Turn(adapter, session)
        assert turn.observed.wait(3)
        _accept_result(store, task_id, "xa_never", channel_id)
        accepted = time.monotonic()
        adapter.finish_owned("xa_never")
        cleanup = adapter._result_settlements["xa_never"]
        # The engine is not killed before the bounded settlement ends.
        assert not manager._process.wait_exited(1.6)
        cleanup.join(6)
        turn.thread.join(3)
        assert not cleanup.is_alive() and not turn.thread.is_alive()
        assert manager._process.wait_exited(0)
        assert time.monotonic() - accepted < 6
        assert len(turn.errors) == 1 and isinstance(turn.errors[0], EngineTurnFailed)
        row, = usage.list_usage(store, task_id)
        assert row["basis"] == "unknown" and row["tokens"] == {}  # never zero
        assert row["stop_reason"] == "released_after_result" and row["failed"] is False
        event, = _settlement_events(store, "xa_never")
        assert event["outcome"] == "timeout"
        assert event["reason"] == "post_result_prompt_settlement_timeout"
    finally:
        manager.shutdown(grace=0.5)


@pytest.mark.parametrize("engine", TOKENS)
def test_watchdog_backstop_terminates_after_the_bounded_result_grace(tmp_path, engine, monkeypatch):
    """No settlement ever runs (no controller pass): the watchdog still ends
    the engine of a result-revoked channel, only after the bounded grace."""
    monkeypatch.setattr(live_adapters, "_RESULT_REVOCATION_GRACE_SECONDS", 2.5, raising=False)
    store, task_id, manager, session, channel_id, adapter = _live(
        tmp_path, engine, "xa_backstop", default_scenario(
            prompt_updates=[_CONTEXT_ONLY], prompt_behavior="one_update_then_hang"))
    try:
        turn = _Turn(adapter, session)
        assert turn.observed.wait(3)
        _accept_result(store, task_id, "xa_backstop", channel_id)
        accepted = time.monotonic()
        assert not manager._process.wait_exited(1.6)
        assert manager._process.wait_exited(4.5)
        assert time.monotonic() - accepted < 5.5
        turn.thread.join(3)
        assert not turn.thread.is_alive() and len(turn.errors) == 1
        row, = usage.list_usage(store, task_id)
        assert row["basis"] == "unknown" and row["tokens"] == {}
        assert row["stop_reason"] == "released_after_result"
    finally:
        manager.shutdown(grace=0.5)


@pytest.mark.parametrize("engine", TOKENS)
def test_cancellation_during_settlement_terminates_at_once_and_stays_unknown(tmp_path, engine):
    store, task_id, manager, session, channel_id, adapter = _live(
        tmp_path, engine, "xa_cancel", default_scenario(
            prompt_updates=[_CONTEXT_ONLY], prompt_behavior="late_reply",
            late_reply_delay=6.0, prompt_usage=TOKENS[engine]))
    try:
        adapter._RESULT_SETTLE_SECONDS = 6.0
        turn = _Turn(adapter, session)
        assert turn.observed.wait(3)
        _accept_result(store, task_id, "xa_cancel", channel_id)
        adapter.finish_owned("xa_cancel")
        cleanup = adapter._result_settlements["xa_cancel"]
        assert not manager._process.wait_exited(1.6)  # settling, not killed by the watchdog
        cancelled = time.monotonic()
        adapter.terminate_owned("xa_cancel")  # explicit cancellation never waits on settlement
        assert manager._process.wait_exited(0)
        assert time.monotonic() - cancelled < 3
        turn.thread.join(3)
        cleanup.join(7)
        assert not turn.thread.is_alive() and not cleanup.is_alive()
        assert len(turn.errors) == 1
        row, = usage.list_usage(store, task_id)
        assert row["basis"] == "unknown" and row["tokens"] == {}
        assert row["stop_reason"] == "released_after_result"
    finally:
        manager.shutdown(grace=0.5)


@pytest.mark.parametrize("engine", TOKENS)
def test_revocation_without_a_published_result_still_terminates_on_the_next_poll(tmp_path, engine):
    """Containment guard: a stop/cancel revocation gets no result grace."""
    store, task_id, manager, session, channel_id, adapter = _live(
        tmp_path, engine, "xa_stopped", default_scenario(
            prompt_updates=[_CONTEXT_ONLY], prompt_behavior="one_update_then_hang"))
    try:
        turn = _Turn(adapter, session)
        assert turn.observed.wait(3)
        revoke_channel(store, channel_id)
        assert manager._process.wait_exited(2.5)
        turn.thread.join(3)
        assert not turn.thread.is_alive() and len(turn.errors) == 1
        row, = usage.list_usage(store, task_id)
        assert row["basis"] == "unknown" and row["tokens"] == {} and row["failed"] is True
    finally:
        manager.shutdown(grace=0.5)


def test_result_grace_is_bounded_and_limited_to_the_attempts_own_result(tmp_path, monkeypatch):
    clock = [1000.0]
    monkeypatch.setattr(live_adapters.time, "monotonic", lambda: clock[0])
    store, task_id = _task(tmp_path)
    channel_id, secret = _worker_channel(store, task_id, "xa_own")
    jail = _jail_for(store, (channel_id, secret, str(tmp_path)))
    assert jail["alive"]()
    _accept_result(store, task_id, "xa_own", channel_id)
    assert jail["alive"]()  # first revoked observation starts the grace
    clock[0] += live_adapters._RESULT_REVOCATION_GRACE_SECONDS - 0.01
    assert jail["alive"]()
    clock[0] += 0.02
    assert not jail["alive"]()
    assert live_adapters._RESULT_REVOCATION_GRACE_SECONDS > live_adapters.EngineSessionManagerAdapter._RESULT_SETTLE_SECONDS

    # Another attempt's result, or a superseded (cancelled) one, grants nothing.
    other_id, other_secret = _worker_channel(store, task_id, "xa_other")
    other = _jail_for(store, (other_id, other_secret, str(tmp_path)))
    revoke_channel(store, other_id)
    assert not other["alive"]()
    cancelled_id, cancelled_secret = _worker_channel(store, task_id, "xa_cancelled")
    cancelled = _jail_for(store, (cancelled_id, cancelled_secret, str(tmp_path)))
    _publish_result(store, task_id, "xa_cancelled", cancelled_id)
    delegation_events.mark_superseded(store, task_id, "xa_cancelled")
    revoke_channel(store, cancelled_id)
    assert not cancelled["alive"]()


def test_unprovable_result_cause_fails_closed(tmp_path, monkeypatch):
    from cortex_runtime.contracts import StoreError

    store, task_id = _task(tmp_path)
    channel_id, secret = _worker_channel(store, task_id, "xa_lookup")
    jail = _jail_for(store, (channel_id, secret, str(tmp_path)))
    _accept_result(store, task_id, "xa_lookup", channel_id)

    def busy(*args):
        raise StoreError("storage_busy")

    monkeypatch.setattr(live_adapters, "_result_published", busy, raising=False)
    assert not jail["alive"]()  # the revocation is certain; its cause is not
