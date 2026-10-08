"""engine.claude_adapter_acp.claude_option_mapping() against the pinned
claude-agent-acp 0.78.0 configOptions fixture (tests/fixtures/catalog/
claude_agent_acp_config_options.json -- see that file's own fixture note
for the dist file:line citations this shape is derived from). Mirrors
tests/test_engine_codex_option_mapping.py exactly."""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from cortex_runtime.configuration.service import ConfigurationRejected
from cortex_runtime.engine.claude_adapter_acp import claude_option_mapping
from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute

FIXTURE_PATH = Path(__file__).resolve().parent / "fixtures/catalog/claude_agent_acp_config_options.json"


def _observed_config_options():
    data = json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))
    return data["configOptions"]


def _revision(model="model-a", supported_parameters=()):
    route = ExecutionRoute(
        engine="claude-agent-acp", connection_id="cn_" + "0" * 12, model=model, supported_parameters=supported_parameters,
    )
    return ConfigurationRevision(id="cr_" + "0" * 12, task_id="t_" + "0" * 12, revision=1, route=route)


def test_model_only_mapping():
    mapping = claude_option_mapping(_revision(model="model-a"), _observed_config_options())
    assert mapping == [("model", "model-a")]


def test_model_and_effort_mapping_thought_level_name():
    revision = _revision(model="model-b", supported_parameters=(("thought_level", "medium"),))
    mapping = claude_option_mapping(revision, _observed_config_options())
    assert mapping == [("model", "model-b"), ("effort", "medium")]


def test_model_and_effort_mapping_effort_name():
    revision = _revision(model="model-a", supported_parameters=(("effort", "default"),))
    mapping = claude_option_mapping(revision, _observed_config_options())
    assert mapping == [("model", "model-a"), ("effort", "default")]


def test_mode_is_mapped_only_when_present_and_supported():
    revision = _revision(model="model-a", supported_parameters=(("mode", "acceptEdits"),))
    mapping = claude_option_mapping(revision, _observed_config_options())
    assert ("mode", "acceptEdits") in mapping


@pytest.mark.parametrize("mode_value", ("default", "acceptEdits", "plan", "auto", "bypassPermissions"))
def test_every_observed_mode_value_is_accepted(mode_value):
    revision = _revision(model="model-a", supported_parameters=(("mode", mode_value),))
    mapping = claude_option_mapping(revision, _observed_config_options())
    assert ("mode", mode_value) in mapping


def test_unsupported_model_is_rejected_before_prompt():
    revision = _revision(model="not-a-real-model")
    with pytest.raises(ConfigurationRejected) as excinfo:
        claude_option_mapping(revision, _observed_config_options())
    assert "model" in excinfo.value.reason


def test_unsupported_effort_is_rejected():
    revision = _revision(model="model-a", supported_parameters=(("thought_level", "extreme"),))
    with pytest.raises(ConfigurationRejected) as excinfo:
        claude_option_mapping(revision, _observed_config_options())
    assert "effort" in excinfo.value.reason


def test_unsupported_mode_is_rejected():
    revision = _revision(model="model-a", supported_parameters=(("mode", "god-mode"),))
    with pytest.raises(ConfigurationRejected) as excinfo:
        claude_option_mapping(revision, _observed_config_options())
    assert "mode" in excinfo.value.reason


def test_effort_option_missing_from_observed_options_is_rejected():
    observed = [option for option in _observed_config_options() if option["id"] != "effort"]
    revision = _revision(model="model-a", supported_parameters=(("thought_level", "medium"),))
    with pytest.raises(ConfigurationRejected) as excinfo:
        claude_option_mapping(revision, observed)
    assert excinfo.value.reason == "claude_acp_option_not_offered:effort"


def test_model_missing_from_observed_options_is_rejected():
    observed = [option for option in _observed_config_options() if option["id"] != "model"]
    with pytest.raises(ConfigurationRejected) as excinfo:
        claude_option_mapping(_revision(model="model-a"), observed)
    assert excinfo.value.reason == "claude_acp_option_not_offered:model"


@pytest.mark.parametrize("parameter_name", ("model_config", "collaboration_mode", "fast-mode", "fast"))
def test_never_maps_fast_or_collaboration_mode_or_model_config(parameter_name):
    """"fast" (category "model_config") is never mapped, regardless of what
    a revision's parameters contain -- excluded by design (lead-approved
    design item 1, the same excluded category as codex-acp's own
    "fast-mode")."""
    revision = _revision(model="model-a", supported_parameters=((parameter_name, "true"),))
    mapping = claude_option_mapping(revision, _observed_config_options())
    mapped_ids = {option_id for option_id, _value in mapping}
    assert "fast" not in mapped_ids
    assert mapped_ids == {"model"}


def test_empty_observed_options_rejects_model():
    with pytest.raises(ConfigurationRejected):
        claude_option_mapping(_revision(), [])


def test_sdk_unknown_model_takes_its_effort_from_session_new_meta_not_a_config_option():
    """A model the engine's SDK does not know keeps its bare id as its name
    and gets no effort option; its effort rides on session/new's _meta
    (see claude_adapter_acp.SDK_EFFORTS for the captured evidence)."""
    from cortex_runtime.engine.claude_adapter_acp import (
        EFFORT_CHANNEL_PARAM, SDK_EFFORTS, SESSION_NEW_META_CHANNEL, build_claude_session_new_meta,
        claude_native_subagent_restriction_meta, sdk_unknown_model,
    )

    observed = _observed_config_options()
    model_option = next(option for option in observed if option["id"] == "model")
    unknown = "claude-opus-5[1m]"
    model_option["options"].append({"value": unknown, "name": unknown, "description": ""})

    assert sdk_unknown_model(unknown, unknown) is True
    assert sdk_unknown_model("model-a", "Model A") is False

    mapping = claude_option_mapping(_revision(model=unknown, supported_parameters=(
        ("thought_level", "max"), (EFFORT_CHANNEL_PARAM, SESSION_NEW_META_CHANNEL))), observed)
    assert mapping == [("model", unknown)]
    # A model the engine does name still configures its effort the usual way.
    assert ("effort", "medium") in claude_option_mapping(
        _revision(model="model-a", supported_parameters=(("thought_level", "medium"),)), observed)

    options = build_claude_session_new_meta("role", role="coordinator", effort="max")["claudeCode"]["options"]
    assert options["effort"] == "max"
    assert "effort" not in claude_native_subagent_restriction_meta(role="worker")["claudeCode"]["options"]
    with pytest.raises(Exception):
        build_claude_session_new_meta("role", effort="ultra")
    assert SDK_EFFORTS == ("low", "medium", "high", "xhigh", "max")


def test_0_87_grouped_models_and_out_of_picker_current_model():
    observed = [{"id": "model", "type": "select", "currentValue": "custom-current",
                 "options": [{"group": "SDK", "name": "SDK",
                              "options": [{"value": "haiku", "name": "Haiku 4.5"}]}]}]
    assert claude_option_mapping(_revision(model="haiku"), observed) == [("model", "haiku")]
    assert claude_option_mapping(_revision(model="custom-current"), observed) == [("model", "custom-current")]
    with pytest.raises(ConfigurationRejected):
        claude_option_mapping(_revision(model="not-advertised"), observed)


@pytest.mark.parametrize("effort", ("low", "medium", "high", "xhigh", "max"))
def test_0_87_effort_choices_remain_observed_not_hardcoded(effort):
    observed = _observed_config_options()
    option = next(item for item in observed if item["id"] == "effort")
    option["options"] = [{"value": effort, "name": effort}]
    assert ("effort", effort) in claude_option_mapping(
        _revision(supported_parameters=(("effort", effort),)), observed)
    with pytest.raises(ConfigurationRejected):
        claude_option_mapping(_revision(supported_parameters=(("effort", "default"),)), observed)


def test_current_effort_is_not_permission_to_set_an_unoffered_effort():
    observed = _observed_config_options()
    option = next(item for item in observed if item["id"] == "effort")
    option["currentValue"] = "max"
    with pytest.raises(ConfigurationRejected):
        claude_option_mapping(_revision(supported_parameters=(("effort", "max"),)), observed)
