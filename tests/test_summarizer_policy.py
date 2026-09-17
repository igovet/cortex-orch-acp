"""T024: recovery.summarizer_policy -- adapter findings and bounded fallback."""
from __future__ import annotations

import json

import pytest

from cortex_runtime.recovery.summarizer_policy import (
    MAX_RETRIES,
    resolve_summary,
    summarizer_capability,
)

AUTHORITATIVE = dict(task_id="t_1", assignment_status="running", completed=False, checks=[], constraint_ids=[])
GOOD = dict(provenance="derived_summary", text="ok", references=[], claims={}, checks=[], constraints_acknowledged=[])


def test_codex_acp_summarizer_is_engine_owned_not_configurable():
    result = summarizer_capability("codex-acp")
    assert result.supported is False
    assert "engine-owned" in result.reason.lower() or "engine_owned" in result.reason.lower()


def test_claude_agent_acp_summarizer_is_engine_owned_not_configurable():
    result = summarizer_capability("claude-agent-acp")
    assert result.supported is False


def test_unknown_engine_never_assumed_supported():
    result = summarizer_capability("some-future-engine")
    assert result.supported is False


def test_valid_first_attempt_returns_summary_no_fallback():
    calls = []

    def produce():
        calls.append(1)
        return json.dumps(GOOD)

    result = resolve_summary(produce)
    assert not result.used_deterministic_fallback
    assert result.attempts == 1
    assert result.escalated is False


def test_unavailable_summarizer_falls_back_deterministically_after_bounded_retries():
    calls = []

    def produce():
        calls.append(1)
        raise RuntimeError("summarizer down")

    result = resolve_summary(produce)
    assert result.used_deterministic_fallback
    assert result.summary is None
    assert result.attempts == MAX_RETRIES + 1
    assert len(calls) == MAX_RETRIES + 1
    assert result.escalated is False
    assert "summarizer_unavailable" in result.reason


def test_invalid_summary_every_attempt_falls_back_never_uses_partial_content():
    def produce():
        return json.dumps(dict(GOOD, provenance="worker_report"))  # always invalid

    result = resolve_summary(produce)
    assert result.used_deterministic_fallback
    assert result.summary is None
    assert result.attempts == MAX_RETRIES + 1


def test_second_attempt_succeeds_within_bound():
    attempts = {"n": 0}

    def produce():
        attempts["n"] += 1
        if attempts["n"] == 1:
            raise RuntimeError("transient")
        return json.dumps(GOOD)

    result = resolve_summary(produce)
    assert not result.used_deterministic_fallback
    assert result.attempts == 2


def test_retry_token_budget_bounds_attempts_even_with_high_max_retries():
    calls = []

    def produce():
        calls.append(1)
        return "x" * 100_000  # not valid JSON -> keeps retrying until budget exhausted

    result = resolve_summary(produce, max_retries=1000, max_retry_token_budget=1)
    assert result.used_deterministic_fallback
    # Must stop far short of 1000 retries because the token budget is tiny.
    assert len(calls) <= 2


def test_never_escalates_to_a_different_route_on_fallback():
    def produce():
        raise RuntimeError("down")

    result = resolve_summary(produce)
    assert result.escalated is False
