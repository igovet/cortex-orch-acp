"""Public install data is explicit, safe, and preserves the retained seven-route baseline."""
from copy import deepcopy
import json
import hashlib
from types import SimpleNamespace

import pytest

from cortex_runtime.routing import auto_route, defaults, model_policy
from cortex_runtime.portable.errors import PortableError


def entry(engine, model, efforts=("low", "medium", "high", "xhigh", "max", "ultra")):
    return SimpleNamespace(connection_id=engine, route=SimpleNamespace(
        id=f"{engine}/{model}", engine=engine, model=model,
        supported_parameters=tuple(("thought_level", v) for v in efforts)))


def test_public_asset_preserves_exact_baseline_tiers_efforts_and_coordinator():
    template = defaults.load_model_settings_template()
    routing = template["routing"]
    assert template["coordinator"] == {"engine": "claude-agent-acp", "model": "opus[1m]",
                                       "parameters": {"thought_level": "medium"}}
    assert template["language"] == "ru"
    # Complete portable production default snapshot, saved 2026-10-06 08:41 UTC.
    canonical = json.dumps(template, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    assert hashlib.sha256(canonical.encode()).hexdigest() == "7c157e97370a58cd5f0cd207d42cbab3b3701954290da38b37086258409e5f40"
    expected = {
        ("claude-agent-acp", "default"): ("deep",),
        ("claude-agent-acp", "opus[1m]"): ("flagship", "deep"),
        ("claude-agent-acp", "claude-fable-5-1[1m]"): ("flagship",),
        ("claude-agent-acp", "claude-sonnet-5"): ("standard",),
        ("codex-acp", "gpt-6.1-sol"): ("deep", "standard"),
        ("codex-acp", "gpt-6-astra"): ("flagship",),
        ("codex-acp", "gpt-6-luna"): ("light",),
    }
    assert {(e, m) for e, models in routing["enabled"].items() for m in models} == set(expected)
    for (engine, model), tiers in expected.items():
        policy = model_policy.policy_for(engine, model, routing)
        assert policy.selected and policy.tiers == tiers and policy.tier_source == "configured"
        for tier in tiers:
            efforts = {
                "default": ("medium", "high", "xhigh"),
                "opus[1m]": ("medium", "high", "xhigh"),
                "claude-fable-5-1[1m]": ("low", "medium", "high"),
                "claude-sonnet-5": ("medium", "high", "xhigh"),
                "gpt-6.1-sol": ("medium", "high", "xhigh"),
                "gpt-6-astra": ("low", "medium", "high"),
                "gpt-6-luna": ("high", "xhigh", "max"),
            }[model]
            assert auto_route.permitted_efforts(entry(engine, model), tier, policy) == efforts
    assert routing["enabled"]["opencode"] == []
    assert routing["models"]["opencode"]["auto"] == ["byteplus-plan/glm-5.3-flash"]
    assert routing["models"]["opencode"]["tiers"]["byteplus-plan/glm-5.2"] == "deep"
    assert routing["providers"] == ["claude-agent-acp", "codex-acp"]
    assert routing["auto_allow"] == ["opencode/byteplus-plan/glm-5.3-flash"]
    assert routing["deny"] == ["gpt-5.5", "haiku", "claude-agent-acp/claude-sonnet-4-6",
        "claude-agent-acp/claude-opus-4-6", "claude-agent-acp/claude-opus-4-7", "claude-agent-acp/claude-opus-4-8",
        "claude-agent-acp/claude-opus-5[1m]", "claude-agent-acp/claude-fable-5", "codex-acp/gpt-5.6-luna",
        "codex-acp/gpt-5.6-sol", "codex-acp/gpt-5.6-terra", "codex-acp/gpt-6-sol"]
    assert routing["models"]["codex-acp"]["policy"]["gpt-6-astra"]["request_names"] == ["astra", "астр"]
    assert routing["models"]["claude-agent-acp"]["policy"]["claude-fable-5-1[1m]"]["request_names"] == ["fable", "фейбл", "фэйбл"]
    assert all(v == "configured" for (e, m) in expected for v in model_policy.policy_for(e, m, routing).attribute_sources.values())


def test_arbitrary_renames_order_new_and_withdrawn_ids():
    routing = defaults.default_routing()
    renamed = deepcopy(routing)
    renamed["deny"] = []
    entries = []
    for engine, config in renamed["models"].items():
        ids = {m: f"vendor/arbitrary-{i}" for i, m in enumerate(config["tiers"])}
        renamed["enabled"][engine] = [ids[m] for m in renamed["enabled"][engine]]
        config["auto"] = [ids[m] for m in config["auto"]]
        config["tiers"] = {ids[m]: tiers for m, tiers in config["tiers"].items()}
        config["policy"] = {ids[m]: p for m, p in config["policy"].items()}
        for model in config["tiers"]:
            entries.append(entry(engine, model))
    auto_route.validate_routing(renamed)
    first = model_policy.resolve_policies(entries, renamed)
    assert model_policy.resolve_policies(entries[::-1], renamed) == first
    new = entry("codex-acp", "gpt-999-luna")
    p = model_policy.resolve_policies([new], renamed)[new.route.id]
    assert p.tier is None and not p.selected and not p.stretch
    assert not auto_route.model_enabled(renamed, "codex-acp", new.route.model)
    # Withdrawn settings remain unmatched; another entry is never substituted.
    withdrawn = entries.pop()
    assert withdrawn.route.id not in model_policy.resolve_policies(entries, renamed)
    assert withdrawn.route.model in renamed["models"][withdrawn.route.engine]["tiers"]


@pytest.mark.parametrize("change", ["version", "private", "route_id", "coordinator-disabled", "coordinator-param", "language", "envelope"])
def test_template_rejects_invalid_or_private_data(tmp_path, change):
    data = defaults.load_model_settings_template()
    if change == "version": data["schema_version"] = 99
    elif change == "private": data["routing"]["mcp"] = {}
    elif change == "route_id": data["coordinator"]["route_id"] = "local-fingerprint"
    elif change == "coordinator-disabled": data["routing"]["enabled"]["claude-agent-acp"] = []
    elif change == "coordinator-param": data["coordinator"]["parameters"]["credential"] = "not-secret"
    elif change == "language": data["language"] = "unsupported"
    else: data["owner_profile"] = "not-portable"
    path = tmp_path / "template.json"
    path.write_text(json.dumps(data))
    with pytest.raises(ValueError): defaults.load_model_settings_template(path)


def test_template_reads_are_fresh_and_runtime_never_uses_asset(tmp_path, monkeypatch):
    first = defaults.default_routing()
    first["deny"].clear()
    assert defaults.default_routing()["deny"]
    monkeypatch.setattr(defaults, "load_model_settings_template", lambda: pytest.fail("runtime template fallback"))
    with pytest.raises(PortableError, match="model_policy_migration_required"):
        auto_route.load_overrides(tmp_path, {"HOME": str(tmp_path)})


def test_legacy_template_without_language_and_with_explicit_mode_still_valid(tmp_path):
    data = defaults.load_model_settings_template()
    del data["language"]
    data["coordinator"]["parameters"]["mode"] = "agent"
    path = tmp_path / "template.json"
    path.write_text(json.dumps(data))
    assert defaults.load_model_settings_template(path) == data


@pytest.mark.parametrize("language", [None, {}, [], True, "unsupported"])
def test_install_language_rejects_non_supported_values(tmp_path, language):
    data = defaults.load_model_settings_template()
    data["language"] = language
    path = tmp_path / "template.json"
    path.write_text(json.dumps(data))
    with pytest.raises(ValueError, match="invalid install language"):
        defaults.load_model_settings_template(path)
