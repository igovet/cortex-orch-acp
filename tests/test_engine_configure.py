"""EngineSessionManager.configure(): accept/reject/crash and evidence_ref."""
from __future__ import annotations

import pytest

from cortex_runtime.configuration.service import ConfigurationRejected
from cortex_runtime.engine.session_manager import EngineSessionManager
from cortex_runtime.portable.engine_contract import EngineCapability
from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute

from engine_fakes import default_scenario, fake_launch_command, isolated_environ, make_connection_record, new_connection_id, write_scenario

REQUESTED_CAPABILITIES = (EngineCapability.SESSION_CREATION, EngineCapability.CONFIGURATION_OPTIONS)


def _make_manager(tmp_path, scenario, *, call_timeout=5.0):
    environ = isolated_environ(tmp_path)
    connection_id = new_connection_id()
    scenario_path = write_scenario(tmp_path, scenario)
    record = make_connection_record(connection_id, launch_command=fake_launch_command(scenario_path), environ=environ)
    return EngineSessionManager(record, str(tmp_path), environ=environ, call_timeout=call_timeout)


def _revision(model="gpt-x"):
    route = ExecutionRoute(engine="codex-acp", connection_id="cn_" + "0" * 12, model=model,
                            supported_parameters=(("thought_level", "high"),))
    return ConfigurationRevision(id="cr_" + "0" * 12, task_id="t_" + "0" * 12, revision=1, route=route)


def test_configure_accept_returns_evidence_ref(tmp_path):
    scenario = default_scenario(
        config_options=[
            dict(id="model", currentValue="old", options=[dict(value="gpt-x", name="GPT X")]),
            dict(id="thought_level", currentValue="low", options=[dict(value="high", name="High")]),
        ],
        configure_behavior="accept",
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        evidence_ref = manager.configure(session_id, _revision())
        assert evidence_ref.startswith(manager._process.engine_instance_id + ":")
        assert "model=gpt-x" in evidence_ref
        assert "thought_level=high" in evidence_ref
    finally:
        manager.shutdown(grace=2.0)


def test_configure_mismatch_is_rejected(tmp_path):
    scenario = default_scenario(
        config_options=[dict(id="model", currentValue="old", options=[dict(value="gpt-x", name="GPT X")])],
        configure_behavior="mismatch",
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        with pytest.raises(ConfigurationRejected):
            manager.configure(session_id, _revision())
    finally:
        manager.shutdown(grace=2.0)


def test_configure_explicit_engine_error_is_rejected(tmp_path):
    scenario = default_scenario(
        config_options=[dict(id="model", currentValue="old", options=[dict(value="gpt-x", name="GPT X")])],
        configure_behavior="error",
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        with pytest.raises(ConfigurationRejected):
            manager.configure(session_id, _revision())
        log = (manager._process.logs.directory / "observations.jsonl").read_text()
        assert '"event": "set_config_option_rejected"' in log and '"option_id": "model"' in log
    finally:
        manager.shutdown(grace=2.0)


def test_configure_unsupported_never_reaches_the_wire(tmp_path):
    scenario = default_scenario(config_options=[])  # engine advertises no config options
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        with pytest.raises(ConfigurationRejected) as excinfo:
            manager.configure(session_id, _revision())
        assert excinfo.value.reason == "engine_does_not_support_configuration_options"
    finally:
        manager.shutdown(grace=2.0)


# --- F4 (T012 review) coverage -------------------------------------------

def test_configure_partial_multi_option_mismatch_is_rejected(tmp_path):
    scenario = default_scenario(
        config_options=[
            dict(id="model", currentValue="old", options=[dict(value="gpt-x", name="GPT X")]),
            dict(id="thought_level", currentValue="low", options=[dict(value="high", name="High")]),
        ],
        configure_behavior="mismatch",
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        with pytest.raises(ConfigurationRejected):
            manager.configure(session_id, _revision())
    finally:
        manager.shutdown(grace=2.0)


def test_configure_success_without_config_options_raises_plain_exception(tmp_path):
    """Documented choice (T012 review F4): a 'successful' reply that omits
    configOptions entirely gives no evidence of the outcome -- it is
    treated as unknown, raising a plain exception (never
    ConfigurationRejected) so ConfigurationService.apply() maps it to
    'indeterminate', the same family as a timeout/transport exit."""
    scenario = default_scenario(
        config_options=[dict(id="model", currentValue="old", options=[dict(value="gpt-x", name="GPT X")])],
        configure_behavior="accept_no_config_options",
    )
    manager = _make_manager(tmp_path, scenario)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        from cortex_runtime.engine.session_manager import EngineConfigurationOutcomeUnknown
        with pytest.raises(EngineConfigurationOutcomeUnknown):
            manager.configure(session_id, _revision())
    finally:
        manager.shutdown(grace=2.0)


def test_configure_timeout_raises_plain_exception(tmp_path):
    scenario = default_scenario(
        config_options=[dict(id="model", currentValue="old", options=[dict(value="gpt-x", name="GPT X")])],
        configure_behavior="hang",
    )
    manager = _make_manager(tmp_path, scenario, call_timeout=0.3)
    try:
        manager.negotiate(REQUESTED_CAPABILITIES)
        session_id = manager.create_session("t_aaaaaaaaaaaa", "xr_" + "0" * 24)
        from cortex_runtime.engine.client import EngineTimeout
        with pytest.raises(EngineTimeout):
            manager.configure(session_id, _revision())
    finally:
        manager.shutdown(grace=2.0)
