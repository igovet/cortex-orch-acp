import json
from pathlib import Path

import pytest

from cortex_runtime.portable.engine_contract import (
    EngineCapability,
    EngineEvent,
    EngineEventKind,
    EngineRuntime,
    StopReason,
    UsageBasis,
)

ROOT = Path(__file__).resolve().parents[1]
# .cortex/ is gitignored (see .gitignore:3) and so is not guaranteed present
# in a fresh checkout/clone; the mandatory cross-check below therefore reads
# the vendored copy in tests/fixtures/engine/ (verbatim, see the NOTICE
# there), and only uses the gitignored original as an additional drift check
# when it happens to be present locally.
CAPABILITY_MATRIX = ROOT / ".cortex/acp-migration/T002/CAPABILITY_MATRIX.json"
VENDORED_CAPABILITY_MATRIX = Path(__file__).resolve().parent / "fixtures" / "engine" / "CAPABILITY_MATRIX.json"

# Verbatim engine-applicable subset of "capabilities_to_record" from
# .cortex/acp-migration/T002/CAPABILITY_MATRIX.json as read on 2026-09-17:
# every recorded capability except the ones that JSON's
# cortex_directions.capabilities_primarily_incoming_client_offered marks as
# primarily offered by the incoming ACP client (filesystem_execution_location,
# unsaved_buffers, terminals, permissions, native_tool_containment,
# tool_schema_delivery, boolean_config) rather than an outgoing engine.
# Hardcoded (rather than read from that gitignored file at test time) so
# this test does not depend on an untracked path existing in a fresh clone.
EXPECTED_ENGINE_APPLICABLE_CAPABILITIES = {
    "protocol_version", "session_creation", "prompt_streaming", "configuration_options",
    "model_discovery", "model_application_confirmation", "reasoning_levels", "auth_modes",
    "credential_config_isolation", "runtime_bridge", "role_instruction_delivery", "cancellation",
    "confirmed_pause", "history_load_with_replay", "resume_without_replay", "elicitation",
    "compaction_observation", "compaction_request", "recovery_delivery_channel",
    "context_generation_observation", "usage", "cost", "summarizer_selection",
    "native_subagents_optional",
}


def test_engine_capability_matches_matrix_engine_applicable_subset():
    actual = {member.value for member in EngineCapability}
    assert actual == EXPECTED_ENGINE_APPLICABLE_CAPABILITIES
    assert len(actual) == 24


def test_engine_capability_cross_check_against_capability_matrix_when_present():
    """Mandatory cross-check: always runs against the vendored copy of
    CAPABILITY_MATRIX.json (tests/fixtures/engine/), so it cannot silently
    skip in a fresh clone where the gitignored .cortex/ evidence tree is
    absent. Additionally verifies the gitignored original is byte-identical
    to the vendored copy when that original happens to be present."""
    matrix = json.loads(VENDORED_CAPABILITY_MATRIX.read_text())
    all_capabilities = set(matrix["capabilities_to_record"])
    client_only = set(matrix["cortex_directions"]["capabilities_primarily_incoming_client_offered"])
    assert EXPECTED_ENGINE_APPLICABLE_CAPABILITIES == all_capabilities - client_only

    if CAPABILITY_MATRIX.is_file():
        assert CAPABILITY_MATRIX.read_text() == VENDORED_CAPABILITY_MATRIX.read_text(), (
            "tests/fixtures/engine/CAPABILITY_MATRIX.json has drifted from "
            ".cortex/acp-migration/T002/CAPABILITY_MATRIX.json; re-copy it verbatim"
        )


def test_engine_event_turn_end_requires_stop_reason():
    event = EngineEvent(kind=EngineEventKind.TURN_END, stop_reason=StopReason.END_TURN)
    assert event.stop_reason == StopReason.END_TURN

    with pytest.raises(TypeError):
        EngineEvent(kind=EngineEventKind.TURN_END, stop_reason=None)


def test_engine_event_usage_update_requires_basis_and_unknown_is_not_zero():
    event = EngineEvent(kind=EngineEventKind.USAGE_UPDATE, usage_basis=UsageBasis.UNKNOWN, usage_values=None)
    assert event.usage_basis == UsageBasis.UNKNOWN
    assert event.usage_values is None  # unknown basis carries no zero-filled values

    with pytest.raises(TypeError):
        EngineEvent(kind=EngineEventKind.USAGE_UPDATE, usage_basis=None)


def test_engine_event_kinds_are_closed_and_cover_spec():
    assert {member.value for member in EngineEventKind} == {
        "turn_delta", "turn_end", "tool_call_observed", "usage_update",
        "compaction_signal", "error",
    }


def test_engine_runtime_protocol_is_structural():
    class FakeEngine:
        def negotiate(self, requested_capabilities):
            return tuple(requested_capabilities)

        def create_session(self, task_id, route_id):
            return "es_" + "0" * 12

        def configure(self, engine_session_id, revision):
            return None

        def prompt_turn(self, engine_session_id, prompt):
            return iter(())

        def cancel(self, engine_session_id):
            return None

        def release(self, engine_session_id):
            return None

        def recovery_observation(self, engine_session_id):
            return None

    assert isinstance(FakeEngine(), EngineRuntime)

    class IncompleteEngine:
        def negotiate(self, requested_capabilities):
            return ()

    assert not isinstance(IncompleteEngine(), EngineRuntime)


def test_no_native_or_host_specific_fields_on_engine_event():
    from dataclasses import fields
    names = {item.name for item in fields(EngineEvent)}
    forbidden = {"subagent_id", "provider_memory", "tool_name", "host_metadata", "thread_id"}
    assert names.isdisjoint(forbidden)
