"""engine.codex_adapter_acp.codex_option_mapping() against the pinned
codex-acp 1.12.0 configOptions fixture (tests/fixtures/catalog/
codex_acp_config_options.json -- see that file's own fixture note for the
dist/index.js line citations this shape is derived from)."""
from __future__ import annotations

import json
from pathlib import Path

import pytest

from cortex_runtime.configuration.service import ConfigurationRejected
from cortex_runtime.engine.codex_adapter_acp import codex_option_mapping
from cortex_runtime.portable.identity import ConfigurationRevision, ExecutionRoute

FIXTURE_PATH = Path(__file__).resolve().parent / "fixtures/catalog/codex_acp_config_options.json"


def _observed_config_options():
    data = json.loads(FIXTURE_PATH.read_text(encoding="utf-8"))
    return data["configOptions"]


def _revision(model="model-a", supported_parameters=()):
    route = ExecutionRoute(
        engine="codex-acp", connection_id="cn_" + "0" * 12, model=model, supported_parameters=supported_parameters,
    )
    return ConfigurationRevision(id="cr_" + "0" * 12, task_id="t_" + "0" * 12, revision=1, route=route)


def test_model_only_mapping():
    mapping = codex_option_mapping(_revision(model="model-a"), _observed_config_options())
    assert mapping == [("model", "model-a")]


def test_model_and_reasoning_effort_mapping_thought_level_name():
    revision = _revision(model="model-b", supported_parameters=(("thought_level", "medium"),))
    mapping = codex_option_mapping(revision, _observed_config_options())
    assert mapping == [("model", "model-b"), ("reasoning_effort", "medium")]


def test_model_and_reasoning_effort_mapping_effort_name():
    revision = _revision(model="model-a", supported_parameters=(("effort", "none"),))
    mapping = codex_option_mapping(revision, _observed_config_options())
    assert mapping == [("model", "model-a"), ("reasoning_effort", "none")]


def test_mode_is_mapped_only_when_present_and_supported():
    revision = _revision(model="model-a", supported_parameters=(("mode", "agent"),))
    mapping = codex_option_mapping(revision, _observed_config_options())
    assert ("mode", "agent") in mapping


def test_unsupported_model_is_rejected_before_prompt():
    revision = _revision(model="not-a-real-model")
    with pytest.raises(ConfigurationRejected) as excinfo:
        codex_option_mapping(revision, _observed_config_options())
    assert "model" in excinfo.value.reason


def test_unsupported_reasoning_effort_is_rejected():
    revision = _revision(model="model-a", supported_parameters=(("thought_level", "extreme"),))
    with pytest.raises(ConfigurationRejected) as excinfo:
        codex_option_mapping(revision, _observed_config_options())
    assert "reasoning_effort" in excinfo.value.reason


def test_unsupported_mode_is_rejected():
    revision = _revision(model="model-a", supported_parameters=(("mode", "god-mode"),))
    with pytest.raises(ConfigurationRejected) as excinfo:
        codex_option_mapping(revision, _observed_config_options())
    assert "mode" in excinfo.value.reason


def test_reasoning_effort_option_missing_from_observed_options_is_rejected():
    observed = [option for option in _observed_config_options() if option["id"] != "reasoning_effort"]
    revision = _revision(model="model-a", supported_parameters=(("thought_level", "medium"),))
    with pytest.raises(ConfigurationRejected) as excinfo:
        codex_option_mapping(revision, observed)
    assert excinfo.value.reason == "codex_acp_option_not_offered:reasoning_effort"


def test_model_missing_from_observed_options_is_rejected():
    observed = [option for option in _observed_config_options() if option["id"] != "model"]
    with pytest.raises(ConfigurationRejected) as excinfo:
        codex_option_mapping(_revision(model="model-a"), observed)
    assert excinfo.value.reason == "codex_acp_option_not_offered:model"


@pytest.mark.parametrize("parameter_name", ("model_config", "collaboration_mode", "fast-mode"))
def test_never_maps_collaboration_mode_or_fast_mode_or_model_config(parameter_name):
    """Neither collaboration_mode nor fast-mode is ever mapped, regardless
    of what a revision's parameters contain -- these categories are
    excluded by design (lead-approved design item 3)."""
    revision = _revision(model="model-a", supported_parameters=((parameter_name, "true"),))
    mapping = codex_option_mapping(revision, _observed_config_options())
    mapped_ids = {option_id for option_id, _value in mapping}
    assert "collaboration_mode" not in mapped_ids
    assert "fast-mode" not in mapped_ids
    assert mapped_ids == {"model"}


def test_empty_observed_options_rejects_model():
    with pytest.raises(ConfigurationRejected):
        codex_option_mapping(_revision(), [])
