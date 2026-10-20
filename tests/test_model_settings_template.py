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
    assert template["coordinator"] == {"engine": "codex-acp", "model": "gpt-6.1-sol",
                                       "parameters": {"thought_level": "medium"}}
    assert template["language"] == "ru"
    # Complete portable production default snapshot, saved 2026-10-12 at 14:23 UTC;
    # 2026-10-13: Opus also standard-tier (UI/UX preferred_when) with request labels;
    # 2026-10-14: the Opus condition covers frontend layout/styling, Sol's excludes it;
    # 2026-10-14: implementation kinds split; Astra is flagship tier only;
    # 2026-10-14: synced from the owner's edited production routing.json -- Astra is
    # preferred for the generic `design` work kind again (tier membership stays
    # flagship-only) and Opus's configured tiers list is reordered strongest-first.
    # 2026-10-20: synced from the owner's edited production routing.json -- Sonnet
    # also serves the deep tier, and Opus's preferred_for narrows to design,
    # planning, implementation_frontend_design, debugging and analysis (dropping
    # implementation_backend, implementation_frontend and review).
    # 2026-10-21: synced again from the owner's edited production routing.json --
    # Opus drops the `standard` tier (flagship/deep only now), its preferred_for
    # regains implementation_backend and implementation_frontend, and Sol's
    # preferred_for gains testing, investigation and writing.
    # Latest sync: Sol preferences are unconditional; the saved default coordinator
    # uses medium effort (confirmed by the owner), not the session-selected route.
    assert routing["models"]["codex-acp"]["policy"]["gpt-6.1-sol"]["preferred_when"] is None
    canonical = json.dumps(template, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    assert hashlib.sha256(canonical.encode()).hexdigest() == "ffa81eb0c8e3e7cd7401860828faa60746d2f81dd1bee17f9dbf1afee6cd3c30"
    expected = {
        ("claude-agent-acp", "default"): ("deep",),
        ("claude-agent-acp", "opus[1m]"): ("flagship", "deep"),
        ("claude-agent-acp", "claude-fable-5-1[1m]"): ("flagship",),
        ("claude-agent-acp", "claude-sonnet-5"): ("deep", "standard"),
        ("claude-agent-acp", "haiku"): ("light",),
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
                "haiku": ("high", "xhigh", "max"),
                "gpt-6.1-sol": ("medium", "high", "xhigh"),
                "gpt-6-astra": ("low", "medium", "high"),
                "gpt-6-luna": ("high", "xhigh", "max"),
            }[model]
            assert auto_route.permitted_efforts(entry(engine, model), tier, policy) == efforts
    assert routing["enabled"]["opencode"] == []
    assert routing["models"]["opencode"]["auto"] == ["byteplus-plan/glm-5.3-flash"]
    assert routing["models"]["opencode"]["tiers"]["byteplus-plan/glm-5.2"] == "deep"
    assert routing["providers"] == ["claude-agent-acp", "codex-acp"]
    assert routing["auto_allow"] == []
    assert routing["deny"] == ["gpt-5.5", "claude-agent-acp/claude-haiku-4-5-20251001", "claude-agent-acp/claude-sonnet-4-6",
        "claude-agent-acp/claude-opus-4-6", "claude-agent-acp/claude-opus-4-7", "claude-agent-acp/claude-opus-4-8",
        "claude-agent-acp/claude-opus-5[1m]", "claude-agent-acp/claude-fable-5", "codex-acp/gpt-5.6-luna",
        "codex-acp/gpt-5.6-sol", "codex-acp/gpt-5.6-terra", "codex-acp/gpt-6-sol", "opencode/*"]
    assert routing["models"]["claude-agent-acp"]["auto"] == routing["enabled"]["claude-agent-acp"]
    assert routing["models"]["claude-agent-acp"]["policy"]["haiku"] == routing["models"]["codex-acp"]["policy"]["gpt-6-luna"]
    assert {"window": "seven_day_haiku", "models": ["haiku"]} in routing["quota_windows"]["claude-agent-acp"]
    assert routing["models"]["codex-acp"]["policy"]["gpt-6-astra"]["request_names"] == ["astra", "астр"]
    assert routing["models"]["claude-agent-acp"]["policy"]["claude-fable-5-1[1m]"]["request_names"] == ["fable", "фейбл", "фэйбл"]
    assert routing["models"]["claude-agent-acp"]["policy"]["opus[1m]"]["request_names"] == ["opus", "опус"]
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
    elif change == "coordinator-disabled": data["routing"]["enabled"][data["coordinator"]["engine"]] = []
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


@pytest.mark.parametrize("role,hint,chosen,hint_status", [
    # Opus remains ineligible at standard even with an exact hint. Sol now
    # has an unconditional preference for generic design and frontend work.
    ("worker-ux-designer", "claude-agent-acp/opus[1m]", "codex-acp/gpt-6.1-sol", "ignored:tier_mismatch"),
    ("worker-ux-designer", "auto", "codex-acp/gpt-6.1-sol", "auto"),
    ("worker-frontend-dev", "auto", "codex-acp/gpt-6.1-sol", "auto"),
    ("worker-frontend-dev", "claude-agent-acp/opus[1m]", "codex-acp/gpt-6.1-sol", "ignored:tier_mismatch"),
])
def test_default_standard_tier_design_and_frontend_work_no_longer_reaches_opus(role, hint, chosen, hint_status):
    routing = defaults.default_routing()
    entries = [entry(engine, model) for engine, models in routing["enabled"].items() for model in models]
    _picked, decision = auto_route.rank(
        auto_route.tier_for(role), entries, lambda e: 10.0, role=role, model_hint=hint,
        affinity=routing.get("engine_affinity") or {}, models=routing["models"])
    assert decision["tier"] == "standard"
    assert decision["chosen"] == chosen
    assert decision["model_hint_status"] == hint_status


@pytest.mark.parametrize("escalated", [False, True])
@pytest.mark.parametrize("hot", [False, True])
def test_default_sol_preference_needs_no_hint_but_still_yields_to_quota(escalated, hot):
    routing = defaults.default_routing()
    entries = [entry(engine, model) for engine, models in routing["enabled"].items() for model in models]
    picked, decision = auto_route.rank(
        "deep" if escalated else "standard", entries,
        lambda e: 99.0 if hot and e.route.engine == "codex-acp" else 10.0,
        role="worker-ux-designer", models=routing["models"], escalated=escalated)
    assert (picked.route.model == "gpt-6.1-sol") is (not hot)
    assert decision["purpose"]["status"] == ("ignored:near_quota_limit" if hot else "honoured")
    assert decision["model_hint_status"] == "auto"


def test_default_design_preference_is_only_opus_and_astra_never_serves_deep():
    routing = defaults.default_routing()
    design = "implementation_frontend_design"
    preferred = [(engine, model) for engine, config in routing["models"].items()
                 for model, policy in config["policy"].items()
                 if design in (policy.get("preferred_for") or ())]
    assert preferred == [("claude-agent-acp", "opus[1m]")]
    assert all(design not in (policy.get("also_standard_for") or ())
               for config in routing["models"].values() for policy in config["policy"].values())
    entries = [entry(engine, model) for engine, models in routing["enabled"].items() for model in models]
    for escalated in (False, True):
        picked, decision = auto_route.rank(
            "deep" if escalated else "standard", entries, lambda e: 10.0,
            role="worker-frontend-dev", work_kind_override=design, models=routing["models"],
            model_hint="claude-agent-acp/opus[1m]", escalated=escalated)
        # Structured frontend-design preference survives quality escalation.
        assert picked.route.model == "opus[1m]"
        assert decision["purpose"]["work_kind"] == design
        assert "codex-acp/gpt-6-astra" not in [c["route"] for c in decision["candidates"]]
    # An unconfirmed Astra hint cannot promote a flagship-only model into deep.
    picked, decision = auto_route.rank("deep", entries, lambda e: 10.0,
                                      role="worker-backend-dev", models=routing["models"],
                                      model_hint="codex-acp/gpt-6-astra")
    assert picked.route.model != "gpt-6-astra"
    assert decision["model_hint_status"] == "ignored:tier_mismatch"


@pytest.mark.parametrize(
    "escalated,previous_engine",
    [
        pytest.param(False, None, id="initial"),
        pytest.param(True, "claude-agent-acp", id="quality-retry"),
        pytest.param(True, "claude-agent-acp", id="review-fix-after-claude-attempt"),
    ],
)
def test_default_frontend_design_uses_opus_on_initial_retry_and_fix(escalated, previous_engine):
    role = "worker-frontend-dev"
    design = "implementation_frontend_design"
    routing = defaults.load_model_settings_template()["routing"]
    entries = [entry(engine, model) for engine, models in routing["enabled"].items() for model in models]

    picked, decision = auto_route.rank(
        auto_route.tier_for(role, escalate=escalated),
        entries,
        lambda _entry: 0.0,
        role=role,
        models=routing["models"],
        model_hint="claude-agent-acp/opus[1m]",
        work_kind_override=design,
        escalated=escalated,
        previous_engine=previous_engine,
    )

    assert picked.route.id == "claude-agent-acp/opus[1m]"
    assert decision["role_tier"] == ("deep" if escalated else "standard")
    # Opus no longer serves standard; even an initial attempt falls back to deep.
    assert decision["tier"] == "deep"


def test_default_frontend_design_falls_back_only_on_affinity_engine_when_opus_is_unavailable():
    role = "worker-frontend-dev"
    design = "implementation_frontend_design"
    routing = defaults.load_model_settings_template()["routing"]
    entries = [
        entry(engine, model)
        for engine, models in routing["enabled"].items()
        for model in models
        if (engine, model) != ("claude-agent-acp", "opus[1m]")
    ]

    picked, _decision = auto_route.rank(
        auto_route.tier_for(role, escalate=True),
        entries,
        lambda _entry: 0.0,
        role=role,
        models=routing["models"],
        work_kind_override=design,
        escalated=True,
        previous_engine="claude-agent-acp",
    )

    assert picked.route.engine == "claude-agent-acp"
    assert picked.route.model != "opus[1m]"


@pytest.mark.parametrize("escalated", [False, True])
@pytest.mark.parametrize("availability", ["available", "unconfigured", "unavailable", "exhausted", "engine_unavailable"])
def test_default_frontend_design_dispatch_keeps_affinity(monkeypatch, escalated, availability):
    from cortex_runtime.connections import quota, registry

    routing = defaults.load_model_settings_template()["routing"]
    entries = [entry(engine, model) for engine, models in routing["enabled"].items() for model in models]
    if availability == "unconfigured":
        entries = [e for e in entries if e.route.model != "opus[1m]"]
    monkeypatch.setattr(auto_route, "load_overrides", lambda *_: routing)
    monkeypatch.setattr(auto_route, "_effective_profile_label", lambda *_: None)
    monkeypatch.setattr(registry, "read_registry", lambda _: SimpleNamespace(
        connections={e.connection_id: SimpleNamespace() for e in entries}))
    monkeypatch.setattr(auto_route, "_entries", lambda *_: entries)
    monkeypatch.setattr(auto_route, "_is_seed", lambda _: False)
    monkeypatch.setattr(quota, "used_percent", lambda _connection, model, *_, **__:
                        100.0 if availability == "exhausted" and model == "opus[1m]" else 0.0)
    unavailable = []
    if availability in ("unavailable", "engine_unavailable"):
        unavailable = [{"connection_id": "claude-agent-acp", "engine": "claude-agent-acp",
                        "model": "opus[1m]", "scope": "connection" if availability == "engine_unavailable" else "model",
                        "reason": "test", "expires_at": "2099-01-01T00:00:00+00:00"}]
    kwargs = dict(escalate=escalated, work_kind_override="implementation_frontend_design",
                  previous_engine="claude-agent-acp", model_hint="claude-agent-acp/opus[1m]",
                  unavailable=unavailable)
    if availability == "engine_unavailable":
        # Owner correction: the whole affinity engine (Claude) being
        # unavailable is no longer a refusal -- dispatch falls back to the
        # normal cross-engine selection, here the preset's sole remaining
        # configured engine, Codex, rather than inheriting the coordinator
        # or refusing visibly.
        choice = auto_route.choose("worker-frontend-dev", **kwargs)
        assert choice.decision["chosen_connection_id"] == choice.connection_id
        assert choice.connection_id != "claude-agent-acp"
        assert choice.decision["affinity"]["required"] is True
        assert choice.decision["affinity"]["cross_engine_fallback"] is True
        return
    choice = auto_route.choose("worker-frontend-dev", **kwargs)
    assert choice.connection_id == "claude-agent-acp"
    assert (choice.model == "opus[1m]") == (availability == "available")
    assert choice.effort in ("medium", "high", "xhigh")
    assert choice.decision["affinity"]["required"] is True
    assert choice.decision["affinity"]["cross_engine_fallback"] is False
