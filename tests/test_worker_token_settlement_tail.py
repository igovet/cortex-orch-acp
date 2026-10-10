"""Replays of the v6 R1 reviewer turns whose closing message outlived settlement.

Retained v6 evidence (dev daemon stopped; immutable SQLite reads and engine
observation metadata only, no message bodies):

- claude ``t_d38953defe17`` reviewer ``xa_3e5c7d7e4b74``: ``worker_result``
  20:10:47.051, ``WORKER_COMPLETED`` 47.071, a context-only ``usage_update``
  49.086, ``usage_settlement`` ``timeout`` 52.101, ``process_exited -15``
  52.283, ``transport_eof`` 52.291, ``prompt_turn_abandoned`` 52.299, usage row
  ``unknown``/``released_after_result`` 52.302.
- codex ``t_0b009d6bba0f`` reviewer ``xa_69417b065a02``: ``worker_result``
  20:20:38.286, context-only ``usage_update`` 38.331, ``usage_settlement``
  ``timeout`` 43.317, ``process_exited -15`` 43.326, ``transport_eof`` 43.328,
  ``prompt_turn_abandoned`` 43.360, usage row ``unknown`` 43.362.

Neither engine logged ``jail_authority_revoked``: the watchdog grace held, and
the post-result settlement bound itself tore the turn down before the final
``PromptResponse`` (the only token carrier). Across the v5/v6 dev runs the
other result-bearing turns ended naturally 2.51-4.92 s after their result, so
the closing message routinely approaches the old 5 s bound.

These tests use the production settlement bound and the real watchdog and
``_jail_for`` probe against a scripted engine subprocess.
"""
from __future__ import annotations

import threading
import time

import pytest

from cortex_runtime.delegation import live_adapters
from cortex_runtime.observability import usage

from engine_fakes import default_scenario
from test_token_coverage_live_shape import TOKENS, _settlement_events
from test_worker_token_settlement import _CONTEXT_ONLY, _Turn, _accept_result, _live


def _chunk(delay, text="."):
    return dict(delay=delay, sessionUpdate="agent_message_chunk", content=dict(type="text", text=text))


# The closing message after result_submit, as each engine streamed it in R1.
# claude: a context fill about 2 s after the result, then more prose.
# codex: a context fill right after the result, then no event for seconds
# (reasoning is not streamed), then the prose. Both end about 6.5 s after it.
_TAILS = {
    "claude-agent-acp": [_CONTEXT_ONLY]
    + [_chunk(0.5) for _ in range(4)]
    + [dict(_CONTEXT_ONLY, used=86000)]
    + [_chunk(0.5) for _ in range(9)],
    "codex-acp": [_CONTEXT_ONLY, dict(_CONTEXT_ONLY, delay=0.05, used=85000), _chunk(4.0)]
    + [_chunk(0.5) for _ in range(5)],
}
_TAIL_SECONDS = 6.5


@pytest.mark.parametrize("engine", TOKENS)
@pytest.mark.parametrize("attempt", ["xa_review", "xa_worker"])
def test_closing_message_longer_than_five_seconds_still_banks_usage(tmp_path, engine, attempt):
    store, task_id, manager, session, channel_id, adapter = _live(
        tmp_path, engine, attempt, default_scenario(
            prompt_updates=_TAILS[engine], prompt_usage=TOKENS[engine]))
    # The shared fixture shortens settlement; this replay uses production bounds.
    adapter.__dict__.pop("_RESULT_SETTLE_SECONDS", None)
    try:
        turn = _Turn(adapter, session)
        assert turn.observed.wait(3)
        _accept_result(store, task_id, attempt, channel_id)
        accepted = time.monotonic()
        adapter.finish_owned(attempt)
        cleanup = adapter._result_settlements.get(attempt)
        turn.thread.join(_TAIL_SECONDS + 10)
        assert not turn.thread.is_alive()
        ended = time.monotonic() - accepted
        if cleanup is not None:
            cleanup.join(5)
        assert turn.errors == []
        # The live turn really outlived the former 5 s bound before it ended.
        assert ended > 5.5
        row, = usage.list_usage(store, task_id)
        assert (row["attempt_id"], row["engine_session_ref"]) == (attempt, session)
        assert row["basis"] == "engine_reported"
        assert row["tokens"] == TOKENS[engine]
        assert row["stop_reason"] == "end_turn" and row["failed"] is False
        event, = _settlement_events(store, attempt)
        assert event["outcome"] == "prompt_recorded" and event["reason"] is None
        assert manager._process.wait_exited(3)  # settlement still reaps the engine
    finally:
        manager.shutdown(grace=0.5)


def test_settlement_bound_covers_observed_tails_and_stays_inside_the_watchdog_grace():
    settle = live_adapters.EngineSessionManagerAdapter._RESULT_SETTLE_SECONDS
    # Longest natural post-result tail seen live (v5/v6): 4.92 s, with 4 of 25
    # turns cut at the old 5 s bound. Keep a margin, but stay short.
    assert 10.0 <= settle <= 20.0
    # The watchdog backstop must never preempt a running settlement: its
    # grace starts at the first revoked observation (up to one 1 s poll
    # early), the settlement starts when the controller observes the result.
    assert live_adapters._RESULT_REVOCATION_GRACE_SECONDS >= settle + 2.0


def test_settlement_timeout_marks_usage_coverage_unavailable(tmp_path, monkeypatch):
    """After the bounded teardown no installed adapter can still send usage:
    the engine is SIGTERMed and its PromptResponse is the only token carrier."""
    store, task_id, manager, session, channel_id, adapter = _live(
        tmp_path, "codex-acp", "xa_hung", default_scenario(
            prompt_updates=[_CONTEXT_ONLY], prompt_behavior="one_update_then_hang"))
    try:
        adapter._RESULT_SETTLE_SECONDS = 1.0
        turn = _Turn(adapter, session)
        assert turn.observed.wait(3)
        _accept_result(store, task_id, "xa_hung", channel_id)
        adapter.finish_owned("xa_hung")
        adapter._result_settlements["xa_hung"].join(6)
        turn.thread.join(3)
        assert not turn.thread.is_alive()
        row, = usage.list_usage(store, task_id)
        assert row["basis"] == "unknown" and row["tokens"] == {}  # never zero
        assert row["stop_reason"] == "released_after_result"
        event, = _settlement_events(store, "xa_hung")
        assert event["outcome"] == "timeout"
        assert event["reason"] == "post_result_prompt_settlement_timeout"
        assert event["usage_coverage"] == "unavailable"
    finally:
        manager.shutdown(grace=0.5)
