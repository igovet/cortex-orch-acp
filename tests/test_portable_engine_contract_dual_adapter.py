"""T026 acceptance bullet 1: one parametrized portable contract suite run
for BOTH pinned adapters (codex-acp 1.12.0, claude-agent-acp 0.78.0)
against fake engines, with each run's own adapter-specific metadata only.

The claude-agent-acp run specifically asserts the absence of every
Codex-specific token/env var/path anywhere it could leak (launch command,
process environment, ``session/new`` payload, and the qualified delivery
mechanism's own instructions payload) -- proving
``delegation.live_adapters``'s per-engine dispatch (T026, see
``tests/test_engine_option_mapping_dispatch.py``) and
``connections.environment.build_process_env`` never leak one engine's
identity into the other's connection.

Covers, per adapter, in one parametrized pass: identity (launch command
shape), configuration/route application (option mapping accept/reject),
session lifecycle (negotiate/create_session/shutdown through the real
production ``EngineSessionManagerAdapter``), and generations/deliveries
(``delegation.generations.tier_for``, ``engine.instruction_delivery_registry
.delivery_for``) -- each adapter's own qualification, never forced to
match the other's.
"""
from __future__ import annotations

import json
from dataclasses import dataclass
from pathlib import Path

import pytest

from cortex_runtime.configuration.service import ConfigurationRejected
from cortex_runtime.delegation import generations as delegation_generations
from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter
from cortex_runtime.engine.claude_adapter_acp import claude_option_mapping
from cortex_runtime.engine.codex_adapter_acp import codex_option_mapping
from cortex_runtime.engine.instruction_delivery_registry import delivery_for
from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute
from cortex_runtime.portable.instruction_delivery import DeliveryQualification
from cortex_runtime.portable.roles import load_role

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario

FIXTURES = Path(__file__).resolve().parent / "fixtures/catalog"


@dataclass(frozen=True)
class AdapterFixture:
    engine: str
    version: str
    option_mapping: object
    config_options_path: Path
    own_effort_param: str
    own_effort_value: str
    own_effort_evidence_key: str
    foreign_mode_value: str  # a mode value valid for the OTHER engine, never this one
    expected_delivery_qualification: DeliveryQualification
    expected_tier: str


ADAPTERS = (
    AdapterFixture(
        engine="codex-acp", version="1.12.0", option_mapping=codex_option_mapping,
        config_options_path=FIXTURES / "codex_acp_config_options.json",
        own_effort_param="thought_level", own_effort_value="medium", own_effort_evidence_key="reasoning_effort",
        foreign_mode_value="acceptEdits",  # a claude-agent-acp-only mode value
        expected_delivery_qualification=DeliveryQualification.UNQUALIFIED,
        expected_tier="automatic",
    ),
    AdapterFixture(
        engine="claude-agent-acp", version="0.78.0", option_mapping=claude_option_mapping,
        config_options_path=FIXTURES / "claude_agent_acp_config_options.json",
        own_effort_param="thought_level", own_effort_value="medium", own_effort_evidence_key="effort",
        foreign_mode_value="agent",  # a codex-acp-only mode value
        expected_delivery_qualification=DeliveryQualification.QUALIFIED,
        expected_tier="automatic",
    ),
)


def _ids(fixture: AdapterFixture) -> str:
    return fixture.engine


def _config_options(fixture: AdapterFixture) -> list:
    return json.loads(fixture.config_options_path.read_text(encoding="utf-8"))["configOptions"]


def _revision(fixture: AdapterFixture, model="model-a", supported_parameters=()):
    route = ExecutionRoute(
        engine=fixture.engine, connection_id="cn_" + "0" * 12, model=model, supported_parameters=supported_parameters,
    )
    return ConfigurationRevision(id="cr_" + "0" * 12, task_id="t_aaaaaaaaaaaa", revision=1, route=route)


@pytest.mark.parametrize("fixture", ADAPTERS, ids=_ids)
def test_identity_launch_command_is_this_engines_own(tmp_path, fixture):
    """Identity: a connection record for this engine carries this engine's
    own name, never the other's -- basic precondition every other check
    below assumes."""
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    record = make_connection_record(connection_id, launch_command=("/usr/bin/x", "/opt/x.js"), engine=fixture.engine, environ=environ)
    assert record.engine == fixture.engine
    other_engines = {"codex-acp", "claude-agent-acp"} - {fixture.engine}
    assert record.engine not in other_engines


@pytest.mark.parametrize("fixture", ADAPTERS, ids=_ids)
def test_configuration_route_application_accepts_own_shape_rejects_foreign_mode(fixture):
    """Configuration/route application: this engine's own option_mapping
    accepts a value shaped for its own fixture, and rejects a mode value
    that is only ever valid for the *other* engine -- proving no foreign
    parameter silently passes."""
    options = _config_options(fixture)
    mapping = fixture.option_mapping(_revision(fixture, supported_parameters=((fixture.own_effort_param, fixture.own_effort_value),)), options)
    mapped = dict(mapping)
    assert mapped["model"] == "model-a"
    assert mapped[fixture.own_effort_evidence_key] == fixture.own_effort_value

    with pytest.raises(ConfigurationRejected) as excinfo:
        fixture.option_mapping(_revision(fixture, supported_parameters=(("mode", fixture.foreign_mode_value),)), options)
    assert "mode" in excinfo.value.reason


@pytest.mark.parametrize("fixture", ADAPTERS, ids=_ids)
def test_session_lifecycle_through_real_production_adapter(tmp_path, fixture):
    """Session lifecycle: negotiate/create_session/shutdown through the
    real ``EngineSessionManagerAdapter`` (T026 per-engine dispatch),
    against the standalone fake-engine subprocess -- never a real engine."""
    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    scenario_path = write_scenario(engine_dir, default_scenario(config_options=_config_options(fixture)))
    connection_id = new_connection_id()
    record = make_connection_record(
        connection_id, launch_command=fake_launch_command(scenario_path), engine=fixture.engine, environ=environ,
    )
    adapter = EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(tmp_path), environ=environ, call_timeout=10.0,
    )
    attempt_id = "xa_" + connection_id[3:]
    session_id = adapter.open("shared_project_root", dict(task_id="t_aaaaaaaaaaaa", attempt_id=attempt_id), "instructions")
    try:
        assert session_id.startswith("es_")
        manager = adapter._managers[attempt_id]
        assert manager._option_mapping is fixture.option_mapping
    finally:
        for manager in list(adapter._managers.values()):
            manager.shutdown(grace=2.0)


@pytest.mark.parametrize("fixture", ADAPTERS, ids=_ids)
def test_generations_tier_and_instruction_delivery_qualification_are_this_engines_own(fixture):
    """Generations/deliveries: each engine's own recovery tier and
    instruction-delivery qualification -- never forced to match the other
    engine's (they are legitimately different by design: codex-acp's own
    leading-block channel is UNQUALIFIED, claude-agent-acp's own
    session/new channel is QUALIFIED, per T026's fresh dist evidence)."""
    tier = delegation_generations.tier_for(fixture.engine, fixture.version)
    assert tier.tier == fixture.expected_tier

    role = load_role("worker-backend-dev")
    plan = delivery_for(fixture.engine, fixture.version).plan(role, [])
    assert plan.qualification == fixture.expected_delivery_qualification


def test_claude_agent_acp_run_carries_no_codex_metadata_anywhere(tmp_path):
    """The core residual-audit assertion: a claude-agent-acp route's own
    launch command, process environment, and session/new payload never
    contain a Codex-specific token/env var/path -- and its role
    instructions never mention codex-acp either."""
    claude = ADAPTERS[1]
    assert claude.engine == "claude-agent-acp"

    engine_dir = tmp_path / "engine"
    engine_dir.mkdir()
    environ = isolated_environ(engine_dir)
    env_dump_path = engine_dir / "env-dump.json"
    session_new_dump_path = engine_dir / "session-new.json"
    role = load_role("worker-backend-dev")
    scenario_path = write_scenario(engine_dir, default_scenario(
        config_options=_config_options(claude),
        env_dump_path=str(env_dump_path),
        session_new_params_dump_path=str(session_new_dump_path),
    ))
    connection_id = new_connection_id()
    record = make_connection_record(
        connection_id, launch_command=fake_launch_command(scenario_path), engine=claude.engine, environ=environ,
    )
    assert "codex" not in " ".join(record.launch_command).lower()

    adapter = EngineSessionManagerAdapter(
        lambda task_id: record, lambda task_id: str(tmp_path), environ=environ, call_timeout=10.0,
    )
    attempt_id = "xa_" + connection_id[3:]
    adapter.open("shared_project_root", dict(task_id="t_aaaaaaaaaaaa", attempt_id=attempt_id), role.body_portable)
    for manager in list(adapter._managers.values()):
        manager.shutdown(grace=2.0)

    observed_env = json.loads(env_dump_path.read_text(encoding="utf-8"))
    for name in observed_env:
        assert "CODEX" not in name.upper(), f"codex-shaped env var leaked into a claude-agent-acp process: {name}"
    assert "CLAUDE_CONFIG_DIR" in observed_env

    session_new_params = json.loads(session_new_dump_path.read_text(encoding="utf-8"))
    dumped_text = json.dumps(session_new_params)
    assert "codex" not in dumped_text.lower()
    assert session_new_params["_meta"]["systemPrompt"] == role.body_portable
    assert "codex" not in role.body_portable.lower()
