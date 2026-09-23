"""T024: recovery.token_budget -- shared estimator and per-route budget planning."""
from __future__ import annotations

import math

from cortex_runtime.recovery import capsule as capsule_mod
from cortex_runtime.recovery.token_budget import (
    BYTES_PER_TOKEN_ESTIMATE,
    DEFAULT_CONTEXT_WINDOW_TOKENS,
    ENGINE_REPORTED_BASIS,
    ESTIMATED_BASIS,
    context_window_for_route,
    estimate_tokens,
    estimate_tokens_for_text,
    plan_budget,
)


def test_estimate_tokens_is_conservative_and_rounds_up():
    assert estimate_tokens(0) == 0
    # 1 byte must never estimate to 0 tokens (always rounds up).
    assert estimate_tokens(1) == 1
    assert estimate_tokens(7) == math.ceil(7 / BYTES_PER_TOKEN_ESTIMATE)


def test_capsule_uses_the_same_shared_estimator():
    text = "hello world, this is a test payload for the shared estimator" * 5
    assert capsule_mod._estimate_tokens(len(text)) == estimate_tokens(len(text))


def test_context_window_prefers_real_route_metadata_over_default():
    tokens, basis = context_window_for_route(128_000)
    assert (tokens, basis) == (128_000, ENGINE_REPORTED_BASIS)
    tokens, basis = context_window_for_route(None)
    assert (tokens, basis) == (DEFAULT_CONTEXT_WINDOW_TOKENS, ESTIMATED_BASIS)


def test_plan_budget_fits_when_everything_is_small():
    decision = plan_budget(
        context_window_tokens=32_000,
        role_instructions_bytes=500,
        tool_schemas_bytes=500,
        chat_history_bytes=500,
        capsule_bytes=500,
    )
    assert decision.decision == "fits"
    names = {c.name for c in decision.components}
    # Every mandatory consumer named in the task card is accounted for, not
    # only the capsule.
    assert names == {
        "role_instructions", "tool_schemas", "capsule",
        "response_reserve", "expected_tool_output_reserve", "chat_history",
    }


def test_plan_budget_needs_reference_mode_for_large_optional_evidence():
    decision = plan_budget(
        context_window_tokens=32_000,
        role_instructions_bytes=500,
        tool_schemas_bytes=500,
        chat_history_bytes=500,
        capsule_bytes=500,
        reference_candidate_bytes=200_000,
    )
    assert decision.decision == "needs_reference_mode"
    assert decision.limit_name == "context_window_tokens"
    assert "addressable reference" in decision.message


def test_plan_budget_overflow_names_the_concrete_limit_and_never_truncates():
    decision = plan_budget(
        context_window_tokens=1_000,
        role_instructions_bytes=50_000,
        tool_schemas_bytes=50_000,
        chat_history_bytes=50_000,
        capsule_bytes=50_000,
    )
    assert decision.decision == "overflow"
    assert decision.limit_name == "context_window_tokens"
    assert "context_window_tokens=1000" in decision.message
    assert "split the assignment" in decision.message
    assert "never silently discards" in decision.message


def test_plan_budget_prefers_exact_usage_over_estimate_for_chat_history():
    estimated = plan_budget(
        context_window_tokens=32_000, role_instructions_bytes=100, tool_schemas_bytes=100,
        chat_history_bytes=400_000, capsule_bytes=100,
    )
    exact = plan_budget(
        context_window_tokens=32_000, role_instructions_bytes=100, tool_schemas_bytes=100,
        chat_history_bytes=400_000, capsule_bytes=100,
        exact_usage_tokens=50, exact_usage_basis=ENGINE_REPORTED_BASIS,
    )
    assert exact.decision == "fits"
    assert estimated.decision != "fits"
    chat_component = next(c for c in exact.components if c.name == "chat_history")
    assert chat_component.tokens == 50 and chat_component.basis == ENGINE_REPORTED_BASIS


def test_estimate_tokens_for_text_matches_byte_estimate():
    text = "abc" * 100
    assert estimate_tokens_for_text(text) == estimate_tokens(len(text.encode("utf-8")))


def test_plan_budget_unknown_window_is_unverified_not_overflow():
    decision = plan_budget(
        context_window_tokens=None,
        role_instructions_bytes=50_000,
        tool_schemas_bytes=50_000,
        chat_history_bytes=50_000,
        capsule_bytes=50_000,
    )
    assert decision.decision == "unverified_window"
    assert decision.context_window_basis == ESTIMATED_BASIS
    assert "not reported its real context window" in decision.message


def test_observed_context_window_is_kept_per_connection_and_model(tmp_path):
    from cortex_runtime.connections import context_window

    environ = {"HOME": str(tmp_path), "XDG_STATE_HOME": str(tmp_path / "state")}
    assert context_window.observed("conn-1", "gpt-5.6-luna", environ) is None
    context_window.observe("conn-1", "gpt-5.6-luna", 272_000, environ)
    context_window.observe("conn-1", "gpt-5.6-luna", 0, environ)
    context_window.observe("conn-1", "other", True, environ)
    assert context_window.observed("conn-1", "gpt-5.6-luna", environ) == 272_000
    assert context_window.observed("conn-1", "other", environ) is None
    assert context_window.observed("conn-2", "gpt-5.6-luna", environ) is None
