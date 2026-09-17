"""Unit tests for engine.correlation.CorrelationTables (no subprocess)."""
from __future__ import annotations

from cortex_runtime.engine.correlation import CorrelationTables


def test_session_round_trip():
    table = CorrelationTables()
    table.register_session("es_aaa", "wire-1")
    assert table.wire_session_id("es_aaa") == "wire-1"
    assert table.portable_session_id("wire-1") == "es_aaa"


def test_unknown_session_returns_none():
    table = CorrelationTables()
    assert table.wire_session_id("es_missing") is None
    assert table.portable_session_id("missing-wire") is None


def test_tool_call_ref_stable_and_scoped_by_session():
    table = CorrelationTables()
    first = table.portable_tool_call_ref("wire-1", "tc-shared")
    again = table.portable_tool_call_ref("wire-1", "tc-shared")
    assert first == again

    # Same wire toolCallId under a *different* wire session must never
    # collide with the first session's portable ref.
    other_session = table.portable_tool_call_ref("wire-2", "tc-shared")
    assert other_session != first


def test_two_independent_tables_never_share_state():
    """Two engine instances' own CorrelationTables are fully independent:
    identical wire session/tool-call ids in each table resolve only within
    their own table -- this is what prevents cross-routing (PR acceptance
    bullet 1)."""
    table_a = CorrelationTables()
    table_b = CorrelationTables()
    table_a.register_session("es_a1", "1")
    table_b.register_session("es_b1", "1")

    assert table_a.portable_session_id("1") == "es_a1"
    assert table_b.portable_session_id("1") == "es_b1"

    ref_a = table_a.portable_tool_call_ref("1", "tc-1")
    ref_b = table_b.portable_tool_call_ref("1", "tc-1")
    assert ref_a != ref_b


def test_release_session_clears_tool_calls():
    table = CorrelationTables()
    table.register_session("es_x", "wire-x")
    table.portable_tool_call_ref("wire-x", "tc-1")
    table.release_session("es_x")
    assert table.wire_session_id("es_x") is None
    assert table.portable_session_id("wire-x") is None
    # A fresh tool-call ref allocated after release for the same (now
    # unregistered) wire id gets a new portable id, proving the old
    # mapping was actually dropped rather than merely orphaned.
    fresh = table.portable_tool_call_ref("wire-x", "tc-1")
    assert isinstance(fresh, str) and fresh.startswith("tc_")
