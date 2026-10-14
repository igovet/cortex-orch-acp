"""routing.auto_route selection and connections.quota parsing."""
from __future__ import annotations

import json
import time
from types import SimpleNamespace

import pytest

from cortex_runtime.connections import quota
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.routing import auto_route, model_policy
from model_policy_fixtures import MODELS, routing_fixture


def _entry(connection, model, efforts=("low", "medium", "high", "xhigh", "max")):
    route = SimpleNamespace(
        id=f"{connection}:{model}", engine="codex-acp" if connection.startswith("codex") else "claude-agent-acp",
        model=model, supported_parameters=tuple(("thought_level", e) for e in efforts),
    )
    return SimpleNamespace(connection_id=connection, route=route)


def _is_stretch(model):
    return model_policy.policy_for("codex-acp", model, {"models": MODELS}).stretch


def _efforts(entry, tier):
    policy = model_policy.policy_for(entry.route.engine, entry.route.model, {"models": MODELS})
    return auto_route.permitted_efforts(entry, tier, policy)


# Historical catalog fixtures: codex-acp lists its newest generation first,
# claude-agent-acp its current models before older generations.
CODEX = [_entry("codex", m) for m in ("gpt-6-astra", "gpt-6-sol", "gpt-6-luna", "gpt-5.6-sol", "gpt-5.6-terra",
                                      "gpt-5.6-luna", "gpt-5.5")]
CLAUDE = [_entry("claude", m) for m in ("claude-opus-5[1m]", "sonnet", "haiku", "claude-opus-4-6")]
ENTRIES = CODEX + CLAUDE
# Owner fixture assigns arbitrary IDs; catalogue order has no tier authority.
UNKNOWN = [_entry("codex", m) for m in ("alpha", "beta", "gamma", "delta", "epsilon")]


def _usage(codex=None, claude=None):
    return lambda entry: codex if entry.connection_id == "codex" else claude


def _pick(tier, codex=None, claude=None, overrides=None):
    return auto_route.pick(tier, ENTRIES, _usage(codex, claude), overrides, models=MODELS).route.model


def test_catalog_tiers_are_explicit_fixture_data_not_inferred():
    tiers = auto_route.model_tiers(ENTRIES + UNKNOWN, {}, models=MODELS)
    assert [tiers[e.route.id] for e in CODEX] == ["flagship", "deep", "light", "standard", "standard", "light", "light"]
    # Exact fixture data, not family names, places each model.
    assert [tiers[e.route.id] for e in CLAUDE] == ["deep", "standard", "light", "deep"]
    assert [tiers[e.route.id] for e in UNKNOWN] == ["deep", "deep", "standard", "standard", "light"]


def test_each_ordinary_tier_has_a_candidate_from_both_providers():
    tiers = auto_route.model_tiers(ENTRIES, {}, models=MODELS)
    for tier in ("deep", "standard", "light"):
        engines = {e.route.engine for e in ENTRIES if tiers[e.route.id] == tier}
        assert engines == {"codex-acp", "claude-agent-acp"}, tier
    assert {e.route.model for e in ENTRIES if tiers[e.route.id] == "flagship"} == {"gpt-6-astra"}


def test_tier_override_moves_one_model():
    assert auto_route.model_tiers(ENTRIES, {"gpt-6-luna": "standard"}, models=routing_fixture({"tiers": {"gpt-6-luna": "standard"}})["models"])["codex:gpt-6-luna"] == "standard"


def test_heavy_work_avoids_an_account_above_the_threshold():
    assert _pick("deep", codex=95, claude=40) == "claude-opus-5[1m]"
    assert _pick("deep", codex=40, claude=95) == "gpt-6-sol"
    assert _pick("standard", codex=95, claude=40) == "sonnet"


def test_light_work_also_avoids_the_account_above_the_threshold():
    """Light work no longer spends a hot plan first: every tier prefers the
    plan with more room (the rank never sees when a window resets)."""
    assert _pick("light", codex=95, claude=40) == "haiku"
    assert _pick("light", codex=40, claude=95) == "gpt-6-luna"


def test_both_hot_still_picks_the_one_with_more_left():
    assert _pick("deep", codex=95, claude=88) == "claude-opus-5[1m]"


def test_an_exhausted_account_is_never_chosen():
    assert _pick("light", codex=100, claude=40) == "haiku"
    assert auto_route.pick("deep", CODEX, _usage(codex=100), models=MODELS) is None


def test_an_unavailable_route_is_skipped_across_providers():
    """Every Codex route recorded unavailable: the work crosses to Claude."""
    codex = frozenset(e.route.id for e in CODEX)
    assert auto_route.pick("light", ENTRIES, _usage(codex=95, claude=40), excluded=codex, models=MODELS).route.model == "haiku"
    assert auto_route.pick("deep", ENTRIES, _usage(codex=10, claude=40), excluded=codex, models=MODELS).route.model == "claude-opus-5[1m]"


def test_excluding_a_route_never_moves_another_models_tier():
    """Excluding entries never moves explicit per-model tiers."""
    picked = auto_route.pick("light", UNKNOWN, _usage(codex=10), excluded=frozenset({"codex:alpha", "codex:epsilon"}), models=MODELS)
    assert picked.route.model == "gamma"


def test_nothing_usable_is_no_pick():
    assert auto_route.pick("deep", ENTRIES, _usage(), excluded=frozenset(e.route.id for e in ENTRIES), models=MODELS) is None


def test_unavailable_reasons_name_each_record_and_exhausted_account():
    blocked = {e.route.id: dict(engine="codex-acp", connection_id="codex", model=None, scope="connection",
                                reason="quota_exhausted", expires_at="2026-09-26T09:18:34+00:00") for e in CODEX}
    reasons = auto_route._unavailable_reasons(ENTRIES, blocked, _usage(claude=100))
    assert reasons == [
        "claude-agent-acp on claude: plan window at 100% (engine-reported)",
        "codex-acp (all models) on codex: quota_exhausted until 2026-09-26T09:18:34+00:00",
    ]


def test_non_explicit_routing_never_falls_back_to_a_flagship():
    only_flagship = [CODEX[0]]
    assert auto_route.pick("light", only_flagship, _usage(), models=MODELS) is None
    assert auto_route.pick("flagship", only_flagship, _usage(), models=MODELS).route.model == "gpt-6-astra"


ALL = ("low", "medium", "high", "xhigh", "max", "ultra")


def test_ultra_is_never_permitted_and_max_only_on_a_light_model():
    assert _efforts(_entry("codex", "terra", ALL), "standard") == ("low", "medium", "high", "xhigh")
    assert _efforts(_entry("codex", "alpha", ALL), "light") == ("low", "medium", "high", "xhigh", "max")
    # Luna: never below high, and max even when a tiers override moves it.
    assert _efforts(_entry("codex", "gpt-6-luna", ALL), "light") == ("high", "xhigh", "max")
    assert _efforts(_entry("codex", "gpt-6-luna", ALL), "standard") == ("high", "xhigh", "max")
    # Required floor/ladder support is never silently weakened.
    with pytest.raises(PortableError, match="model_effort_policy_unsupported"):
        _efforts(_entry("codex", "gpt-6-luna", ("low", "medium")), "light")


def test_flagship_models_never_go_above_high():
    assert _efforts(_entry("codex", "gpt-6-astra", ALL), "deep") == ("low", "medium", "high")
    assert _efforts(_entry("claude", "claude-fable-5-1[1m]", ALL), "deep") == ("low", "medium", "high")


def test_nearest_permitted_effort():
    assert auto_route.nearest("high", ("low", "medium")) == "medium"
    assert auto_route.nearest("low", ("high", "xhigh")) == "high"
    assert auto_route.nearest("ultra", ("low", "medium", "high", "xhigh")) == "xhigh"
    assert auto_route.nearest("high", ()) is None


def test_overrides_are_validated_and_located_by_precedence(tmp_path):
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    environ = {"HOME": str(tmp_path)}
    with pytest.raises(PortableError, match="model_policy_migration_required"):
        auto_route.load_overrides(project, environ)
    (project / ".cortex" / "routing.json").write_text(json.dumps(routing_fixture({"deny": ["gpt-5.5"]})))
    assert auto_route.is_denied("codex-acp", "gpt-5.5", project, environ)
    assert not auto_route.is_denied("codex-acp", "luna", project, environ)
    with pytest.raises(PortableError) as refused:
        auto_route.refuse_if_denied("codex-acp", "gpt-5.5", project, environ)
    assert refused.value.code == "route_denied"
    (project / ".cortex" / "routing.json").write_text(json.dumps({"tiers": {"luna": "huge"}}))
    with pytest.raises(PortableError) as malformed:
        auto_route.load_overrides(project, environ)
    assert malformed.value.code == "invalid_routing_overrides"


def test_providers_override_is_validated(tmp_path):
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    environ = {"HOME": str(tmp_path)}
    (project / ".cortex" / "routing.json").write_text(json.dumps(routing_fixture({"providers": ["claude-agent-acp"]})))
    assert auto_route.load_overrides(project, environ) == routing_fixture({"providers": ["claude-agent-acp"]})
    for bad in ({"providers": []}, {"providers": "codex-acp"}, {"providers": ["not-an-engine"]},
                {"providers": [1]}):
        (project / ".cortex" / "routing.json").write_text(json.dumps(bad))
        with pytest.raises(PortableError) as malformed:
            auto_route.load_overrides(project, environ)
        assert malformed.value.code == "invalid_routing_overrides"


def test_allowed_restricts_entries_to_the_listed_providers():
    assert auto_route._allowed({}, ENTRIES) == ENTRIES
    assert auto_route._allowed({"providers": ["claude-agent-acp"]}, ENTRIES) == CLAUDE
    assert auto_route._allowed({"providers": ["codex-acp", "claude-agent-acp"]}, ENTRIES) == ENTRIES


def test_pin_accepts_alias_full_id_and_wildcard():
    assert auto_route._pin_for({"pin": {"planner": "codex-acp/luna:high"}}, "worker-planner") == ("codex-acp", "luna", "high")
    assert auto_route._pin_for({"pin": {"*": "codex-acp/luna"}}, "worker-debugger") == ("codex-acp", "luna", None)
    assert auto_route._pin_for({"pin": {"planner": "codex-acp/luna"}}, "worker-debugger") is None


def test_owner_auto_exclusion_and_explicit_pin(tmp_path, monkeypatch):
    from cortex_runtime.connections import registry

    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    environ = {"HOME": str(tmp_path)}
    candidates = [_entry("claude", "haiku"), _entry("codex", "gpt-5.6-luna")]
    connections = {
        "claude": SimpleNamespace(launch_command=("claude",)),
        "codex": SimpleNamespace(launch_command=("codex",)),
    }
    monkeypatch.setattr(registry, "read_registry", lambda _environ: SimpleNamespace(connections=connections))
    monkeypatch.setattr(auto_route, "_entries", lambda *_args: candidates)
    monkeypatch.setattr(auto_route, "_is_seed", lambda _entry: False)
    monkeypatch.setattr(quota, "used_percent", lambda *_args, **_kwargs: None)

    (project / ".cortex" / "routing.json").write_text(json.dumps(routing_fixture({"models": {"claude-agent-acp": {"auto": []}, "codex-acp": {"auto": ["gpt-5.6-luna"]}}})))
    automatic = auto_route.choose("worker-technical-writer", project, environ)
    assert automatic.model == "gpt-5.6-luna"
    assert automatic.basis == "auto"

    (project / ".cortex" / "routing.json").write_text(
        json.dumps(routing_fixture({"pin": {"worker-technical-writer": "claude-agent-acp/haiku"}}))
    )
    pinned = auto_route.choose("worker-technical-writer", project, environ)
    assert pinned.model == "haiku"
    assert pinned.basis == "auto:pin"

    (project / ".cortex" / "routing.json").write_text(json.dumps(routing_fixture({"models": {"claude-agent-acp": {"auto": []}, "codex-acp": {"auto": ["gpt-5.6-luna"]}}})))
    candidates[:] = [_entry("claude", "claude-haiku-4-5-20251001")]
    with pytest.raises(PortableError) as unavailable:
        auto_route.choose("worker-technical-writer", project, environ)
    assert unavailable.value.code == "no_available_route"
    assert "claude-agent-acp/claude-haiku-4-5-20251001 excluded from automatic routing" in unavailable.value.remediation


def test_codex_usage_is_read_from_its_newest_rollout(tmp_path):
    day = tmp_path / "sessions" / "2026" / "09" / "22"
    day.mkdir(parents=True)
    future = time.time() + 3600
    limits = {"primary": {"used_percent": 47.0, "resets_at": future},
              "secondary": {"used_percent": 91.0, "resets_at": future}}
    (day / "rollout.jsonl").write_text(
        json.dumps({"type": "event_msg", "payload": {"type": "token_count", "rate_limits": None}}) + "\n"
        + json.dumps({"type": "event_msg", "payload": {"type": "token_count", "rate_limits": limits}}) + "\n"
    )
    record = SimpleNamespace(engine="codex-acp", id="cn_" + "a" * 12, credential_ref=SimpleNamespace(path=str(tmp_path)))
    assert quota.used_percent(record, "gpt-5.6-luna") == 91.0
    assert quota.used_percent(record, "gpt-5.6-luna", now=future + 1) is None


def test_the_reset_is_the_most_used_windows_own(tmp_path):
    """An exhausted 5-hour window resets long before the weekly one; the
    route comes back when the window that ran out resets."""
    environ = {"HOME": str(tmp_path), "XDG_STATE_HOME": str(tmp_path / "state")}
    connection_id = "cn_" + "c" * 12
    now = time.time()
    quota.observe(connection_id, {"status": "rejected", "rateLimitType": "five_hour", "resetsAt": now + 3600}, environ)
    quota.observe(connection_id, {"status": "allowed", "rateLimitType": "seven_day", "utilization": 0.4,
                                  "resetsAt": now + 5 * 86400}, environ)
    record = SimpleNamespace(engine="claude-agent-acp", id=connection_id, credential_ref=None)
    assert quota.resets_at(record, "sonnet", environ, now=now) == pytest.approx(now + 3600)
    assert quota.resets_at(record, "sonnet", environ, now=now + 7200) == pytest.approx(now + 5 * 86400)
    assert quota.resets_at(SimpleNamespace(engine="claude-agent-acp", id="cn_" + "d" * 12, credential_ref=None),
                           "sonnet", environ) is None


def test_claude_usage_keeps_the_latest_window_per_kind_and_scopes_model_windows(tmp_path):
    environ = {"HOME": str(tmp_path), "XDG_STATE_HOME": str(tmp_path / "state")}
    connection_id = "cn_" + "b" * 12
    future = time.time() + 3600
    quota.observe(connection_id, {"status": "allowed", "rateLimitType": "five_hour", "utilization": 0.3, "resetsAt": future}, environ)
    quota.observe(connection_id, {"status": "allowed_warning", "rateLimitType": "seven_day_opus", "utilization": 0.9, "resetsAt": future}, environ)
    quota.observe(connection_id, {"status": "allowed", "rateLimitType": "overage", "utilization": 1.0}, environ)
    record = SimpleNamespace(engine="claude-agent-acp", id=connection_id, credential_ref=None)
    # Scope is now explicit owner data; cache/auth/provider behavior is unchanged.
    routing = {"quota_windows": {"claude-agent-acp": [
        {"window": "seven_day_opus", "models": ["opus[1m]"]}]}}
    assert quota.used_percent(record, "opus[1m]", environ, routing=routing) == pytest.approx(90.0)
    assert quota.used_percent(record, "sonnet", environ, routing=routing) == pytest.approx(30.0)
    quota.observe(connection_id, {"status": "rejected", "rateLimitType": "five_hour", "resetsAt": future}, environ)
    assert quota.used_percent(record, "sonnet", environ, routing=routing) == 100.0


def test_explicit_escalation_is_the_only_automatic_flagship_path():
    assert auto_route.tier_for("worker-debugger") == "standard"
    assert auto_route.tier_for("worker-debugger", complexity="high") == "deep"
    assert auto_route.tier_for("worker-debugger", escalate=True) == "deep"
    assert auto_route.tier_for("worker-debugger", escalate=True, complexity="high") == "flagship"
    assert auto_route.tier_for("worker-explorer", escalate=True, complexity="high") == "light"
    assert auto_route.tier_for("worker-security-auditor", complexity="high") == "deep"
    assert auto_route.tier_for("worker-security-auditor", escalate=True) == "flagship"


def test_planned_complexity_shifts_the_role_tier_one_step():
    assert auto_route.tier_for("worker-debugger", complexity="high") == "deep"
    assert auto_route.tier_for("worker-debugger", complexity="low") == "light"
    assert auto_route.tier_for("worker-security-auditor", complexity="low") == "standard"
    assert auto_route.tier_for("worker-explorer", complexity="low") == "light"
    assert auto_route.tier_for("worker-explorer", complexity="high") == "light"
    assert auto_route.tier_for("worker-debugger", escalate=True, complexity="low") == "standard"


def _rank(tier, role=None, preference=None, codex=None, claude=None, entries=ENTRIES, excluded=frozenset()):
    return auto_route.rank(tier, entries, _usage(codex, claude), excluded=excluded, role=role, preference=preference, models=MODELS)


def test_the_role_decides_between_equally_placed_providers():
    """Neither account reports usage (the usual state): the kind of work
    decides, never which connection happens to be listed first."""
    for entries in (ENTRIES, CLAUDE + CODEX):
        assert _rank("standard", "worker-backend-dev", entries=entries)[0].route.engine == "codex-acp"
        assert _rank("deep", "worker-security-auditor", entries=entries)[0].route.engine == "claude-agent-acp"
        # Investigation is reading: it leans to Claude (it leaned to Codex before).
        assert _rank("light", "worker-explorer", entries=entries)[0].route.model == "haiku"
        assert _rank("light", "worker-technical-writer", entries=entries)[0].route.model == "haiku"
        assert _rank("light", "worker-backend-dev", entries=entries)[0].route.model == "gpt-6-luna"


def test_every_plannable_role_has_a_known_work_kind():
    from cortex_runtime.portable.roles import WORKER_ALIASES

    assert set(auto_route.ROLE_WORK_KINDS) == set(WORKER_ALIASES.values())
    assert set(auto_route.ROLE_WORK_KINDS.values()) <= set(auto_route.WORK_KIND_ENGINES)


def test_quota_still_outranks_the_role():
    assert _rank("standard", "worker-backend-dev", codex=95, claude=40)[0].route.model == "sonnet"
    assert _rank("light", "worker-backend-dev", codex=95, claude=40)[0].route.engine == "claude-agent-acp"
    assert _rank("standard", "worker-backend-dev", codex=100, claude=40)[0].route.model == "sonnet"


def test_a_preference_is_honoured_among_the_tiers_candidates():
    picked, decision = _rank("deep", "worker-security-auditor", preference="codex-acp")
    assert picked.route.model == "gpt-6-sol"
    assert decision["tier"] == "deep" and decision["preference_status"] == "honoured"
    assert decision["candidates"][0] == dict(route="codex-acp/gpt-6-sol", connection_id="codex", used_percent=None,
                                             hot=False, quota_state="unknown", usage_band=None, suitability=0, preferred=True)
    assert {c["route"] for c in decision["candidates"]} == {
        "codex-acp/gpt-6-sol", "claude-agent-acp/claude-opus-5[1m]",
        "claude-agent-acp/claude-opus-4-6"}


def test_a_preference_never_moves_the_tier():
    only_deep_codex = CODEX[:2] + CLAUDE
    picked, decision = _rank("light", "worker-explorer", preference="codex-acp", entries=only_deep_codex)
    assert picked.route.model == "haiku"
    assert decision["preference_status"] == "ignored:no_candidate_in_tier"


def test_a_preference_for_an_unavailable_provider_is_ignored():
    codex = frozenset(e.route.id for e in CODEX)
    picked, decision = _rank("deep", "worker-backend-dev", preference="codex-acp", excluded=codex)
    assert picked.route.engine == "claude-agent-acp"
    assert decision["preference_status"] == "ignored:provider_unavailable"
    assert "codex-acp/gpt-6-sol" in decision["unavailable"]


def test_a_preference_never_spends_a_nearly_exhausted_plan_on_heavy_work():
    picked, decision = _rank("deep", "worker-backend-dev", preference="codex-acp", codex=95, claude=40)
    assert picked.route.engine == "claude-agent-acp"
    assert decision["preference_status"] == "ignored:provider_near_quota_limit"
    # Both near the limit: nothing better to protect, so it is honoured.
    picked, decision = _rank("deep", "worker-backend-dev", preference="codex-acp", codex=95, claude=88)
    assert picked.route.engine == "codex-acp" and decision["preference_status"] == "honoured"


def test_without_a_preference_the_decision_says_so():
    picked, decision = _rank("standard", "worker-backend-dev", codex=30, claude=25)
    assert picked.route.model == "gpt-5.6-sol"
    assert decision["preference"] is None and decision["preference_status"] == "none"
    assert [c["suitability"] for c in decision["candidates"]] == [1, 1, 0]


def test_plan_usage_decides_between_suitable_engines_before_affinity():
    """Implementation leans to Codex, but a Claude plan with at least
    USAGE_MARGIN points more room takes the work; a smaller gap is left to
    the kind of work."""
    assert auto_route.USAGE_MARGIN == 10.0
    picked, decision = _rank("standard", "worker-backend-dev", codex=95, claude=75)
    # Rank version 11: the hot/non-hot quota flag now decides before
    # the usage-gap band even applies; the pick is unchanged.
    assert picked.route.model == "sonnet" and decision["decided_by"] == "quota"
    assert decision["rank_version"] == 11
    assert decision["usage"] == dict(compared=True, active=True, status="active", threshold=95.0, margin=10.0, least_used_percent=75)
    assert {c["route"]: c["usage_band"] for c in decision["candidates"]} == {
        "claude-agent-acp/sonnet": 0, "codex-acp/gpt-5.6-sol": 2, "codex-acp/gpt-5.6-terra": 2}
    assert "balance" not in decision
    assert all("balance_excess" not in c for c in decision["candidates"])
    # 9.9 points is inside the margin: affinity decides.
    picked, decision = _rank("standard", "worker-backend-dev", codex=19.9, claude=10,
                             entries=[CODEX[4], CLAUDE[1]])
    assert picked.route.engine == "codex-acp" and decision["decided_by"] == "affinity"
    # The same holds the other way round for review work.
    picked, decision = _rank("deep", "worker-security-auditor", codex=5, claude=95)
    assert picked.route.engine == "codex-acp" and decision["decided_by"] == "quota"


def test_rank_has_no_attempt_count_input():
    """How often an engine was chosen before is not evidence of fit."""
    import inspect

    for function in (auto_route.rank, auto_route.pick, auto_route.choose):
        assert not {"recent", "inflight"} & set(inspect.signature(function).parameters)
    assert "balance" not in auto_route.RANK_KEYS
    assert not hasattr(auto_route, "BALANCE_TOLERANCE")
    first = _rank("standard", "worker-backend-dev", codex=20, claude=20)
    assert all(_rank("standard", "worker-backend-dev", codex=20, claude=20)[1] == first[1] for _ in range(5))


def test_unknown_quota_never_wins_the_percentage_tiebreak():
    # Same suitability (role omitted). The list order decides
    # when either quota is unknown, regardless of the known percentage.
    entries = [CODEX[4], CLAUDE[1]]
    picked, decision = auto_route.rank("standard", entries, _usage(codex=50, claude=None), models=MODELS)
    assert picked is entries[0]
    assert decision["used_percent_compared"] is False
    assert [c["quota_state"] for c in decision["candidates"]] == ["known", "unknown"]
    picked, decision = auto_route.rank("standard", entries, _usage(codex=95, claude=20), models=MODELS)
    assert picked is entries[1] and decision["used_percent_compared"] is True


def test_unknown_quota_is_not_hot_and_never_compared():
    entries = [CLAUDE[2], CODEX[2]]
    picked, decision = auto_route.rank("light", entries, _usage(codex=None, claude=None), models=MODELS)
    assert picked is entries[0] and decision["used_percent_compared"] is False
    assert decision["usage"] == dict(compared=False, active=False, status="unavailable", threshold=95.0, margin=10.0, least_used_percent=None)
    picked, decision = auto_route.rank("light", entries, _usage(codex=95, claude=None), models=MODELS)
    assert picked is entries[0] and decision["decided_by"] == "quota"
    # Below the hot band, a known low usage never beats an unknown one.
    picked, decision = auto_route.rank("light", [CODEX[2], CLAUDE[2]], _usage(codex=60, claude=None),
                                       role="worker-explorer", models=MODELS)
    assert picked is CLAUDE[2] and decision["decided_by"] == "affinity"


def test_policy_exclusion_is_distinct_from_unavailable():
    entries = [CLAUDE[2], CODEX[2]]
    picked, decision = auto_route.rank(
        "light", entries, _usage(), excluded=frozenset({CLAUDE[2].route.id}),
        policy_excluded=frozenset({CLAUDE[2].route.id}),
        models=MODELS)
    assert picked is CODEX[2]
    assert decision["excluded_by_policy"] == ["claude-agent-acp/haiku"]
    assert decision["unavailable"] == []


def test_exact_selection_enables_a_model_but_deny_still_wins(tmp_path, monkeypatch):
    from cortex_runtime.connections import registry

    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    environ = {"HOME": str(tmp_path)}
    entries = [_entry("claude", "haiku"), _entry("codex", "gpt-5.6-luna")]
    connections = {
        "claude": SimpleNamespace(launch_command=("claude",)),
        "codex": SimpleNamespace(launch_command=("codex",)),
    }
    monkeypatch.setattr(registry, "read_registry", lambda _: SimpleNamespace(connections=connections))
    monkeypatch.setattr(auto_route, "_entries", lambda *_: [
        e for e in entries if not auto_route._denied(auto_route.load_overrides(project, environ),
                                                       e.route.engine, e.route.model)
    ])
    monkeypatch.setattr(auto_route, "_is_seed", lambda _: False)
    monkeypatch.setattr(quota, "used_percent", lambda *_, **_kwargs: None)
    route_file = project / ".cortex" / "routing.json"
    route_file.write_text(json.dumps(routing_fixture({"auto_allow": ["claude-agent-acp/haiku"], "models": {"claude-agent-acp": {"auto": ["haiku"]}, "codex-acp": {"auto": ["gpt-5.6-luna"]}}})))
    assert auto_route.choose("worker-technical-writer", project, environ).model == "haiku"
    route_file.write_text(json.dumps(routing_fixture({"auto_allow": ["claude-agent-acp/haiku"], "deny": ["haiku"], "models": {"claude-agent-acp": {"auto": ["haiku"]}, "codex-acp": {"auto": ["gpt-5.6-luna"]}}})))
    assert auto_route.choose("worker-technical-writer", project, environ).model == "gpt-5.6-luna"
    for invalid in ("haiku", [None], [""], [3]):
        route_file.write_text(json.dumps({"auto_allow": invalid}))
        with pytest.raises(PortableError) as refused:
            auto_route.choose("worker-technical-writer", project, environ)
        assert refused.value.code == "invalid_routing_overrides"


def test_escalated_explorer_raises_effort_without_raising_tier(tmp_path, monkeypatch):
    from cortex_runtime.connections import registry

    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    environ = {"HOME": str(tmp_path)}
    # A light model that is not Luna (Luna has its own effort steps).
    entry = _entry("codex", "gpt-5.5")
    monkeypatch.setattr(registry, "read_registry", lambda _: SimpleNamespace(
        connections={"codex": SimpleNamespace(launch_command=("codex",))}))
    monkeypatch.setattr(auto_route, "_entries", lambda *_: [entry])
    monkeypatch.setattr(auto_route, "_is_seed", lambda _: False)
    monkeypatch.setattr(quota, "used_percent", lambda *_, **_kwargs: None)
    (project / ".cortex" / "routing.json").write_text(json.dumps(routing_fixture()))
    choice = auto_route.choose("worker-explorer", project, environ, escalate=True)
    assert choice.effort == "medium"
    assert choice.decision["tier"] == "light"
    assert choice.decision["effort_basis"] == "escalation_step"


def test_quota_and_unavailability_dominate_affinity():
    picked, decision = auto_route.rank(
        "deep", ENTRIES, _usage(codex=95, claude=40), role="worker-backend-dev",
        models=MODELS)
    assert picked.route.engine == "claude-agent-acp"
    # The runner-up is the older Opus on the same account, not Codex.
    assert decision["decided_by"] == "engine_order"
    blocked = frozenset(e.route.id for e in CLAUDE)
    picked, decision = auto_route.rank(
        "deep", ENTRIES, _usage(codex=30, claude=40),
        excluded=blocked, role="worker-security-auditor",
        models=MODELS)
    assert picked.route.engine == "codex-acp"
    assert "claude-agent-acp/claude-opus-5[1m]" in decision["unavailable"]



def test_model_hint_honour_and_tier_guard():
    entries = [CODEX[4], CLAUDE[1], CODEX[0]]
    _, decision = auto_route.rank(
        "standard", entries, _usage(codex=20, claude=20),
        role="worker-backend-dev", model_hint="claude-agent-acp/sonnet",
        model_reason="The work requires prose analysis.", models=MODELS)
    assert (decision["chosen"], decision["model_hint_status"]) == ("claude-agent-acp/sonnet", "honoured")
    assert decision["model_hint_reason"] == "The work requires prose analysis."
    _, decision = auto_route.rank(
        "standard", entries, _usage(codex=20, claude=20),
        role="worker-backend-dev", model_hint="codex-acp/gpt-6-astra", models=MODELS)
    assert decision["tier"] == "standard"
    assert decision["model_hint_status"] == "ignored:tier_mismatch"


@pytest.mark.parametrize("hint,excluded,policy,usage,expected", [
    ("unknown-acp/model", frozenset(), frozenset(), _usage(20, 20), "ignored:not_offered"),
    ("claude-agent-acp/sonnet", frozenset({"claude:sonnet"}), frozenset(),
     _usage(20, 20), "ignored:unavailable"),
    ("claude-agent-acp/sonnet", frozenset({"claude:sonnet"}),
     frozenset({"claude:sonnet"}), _usage(20, 20), "ignored:auto_excluded"),
    ("codex-acp/gpt-5.6-terra", frozenset(), frozenset(), _usage(95, 20),
     "ignored:near_quota_limit"),
])
def test_model_hint_rejection_reasons(hint, excluded, policy, usage, expected):
    entries = [CODEX[4], CLAUDE[1]]
    _, decision = auto_route.rank(
        "standard", entries, usage, excluded=excluded, policy_excluded=policy,
        role="worker-backend-dev", model_hint=hint, models=MODELS)
    assert decision["model_hint_status"] == expected


def test_escalation_ignores_hint_and_prefers_another_engine():
    entries = [CODEX[4], CLAUDE[1]]
    _, decision = auto_route.rank(
        "standard", entries, _usage(20, 20), role="worker-backend-dev",
        model_hint="codex-acp/gpt-5.6-terra", model_reason="Legacy preference.",
        escalated=True, previous_engine="codex-acp", models=MODELS)
    assert decision["model_hint_status"] == "ignored:escalated"
    assert decision["diversify_from"] == "codex-acp"
    assert decision["chosen"] == "claude-agent-acp/sonnet"



def test_route_menu_reflects_live_catalog_and_policy(tmp_path, monkeypatch):
    from cortex_runtime.connections import registry
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    environ = {"HOME": str(tmp_path)}
    candidates = [_entry("claude", "haiku"), _entry("codex", "gpt-5.6-luna")]
    connections = {"claude": SimpleNamespace(launch_command=("claude",)),
                   "codex": SimpleNamespace(launch_command=("codex",))}
    monkeypatch.setattr(registry, "read_registry", lambda _: SimpleNamespace(connections=connections))
    monkeypatch.setattr(auto_route, "_entries", lambda *_: candidates)
    monkeypatch.setattr(auto_route, "_is_seed", lambda _: False)
    monkeypatch.setattr(quota, "used_percent", lambda *_, **_kwargs: None)
    (project / ".cortex" / "routing.json").write_text(json.dumps(routing_fixture()))
    first = auto_route.route_menu(project, environ)
    assert {item["route"] for item in first["tiers"]["light"]} == {
        "claude-agent-acp/haiku", "codex-acp/gpt-5.6-luna"}
    assert next(item for item in first["tiers"]["light"] if item["model"] == "haiku")["auto_eligible"] is False
    (project / ".cortex" / "routing.json").write_text(json.dumps(routing_fixture({
        "providers": ["claude-agent-acp"], "auto_allow": ["haiku"],
        "models": {"claude-agent-acp": {"auto": ["haiku"]}}})))
    second = auto_route.route_menu(project, environ)
    assert [item["route"] for item in second["tiers"]["light"]] == ["claude-agent-acp/haiku"]
    assert second["tiers"]["light"][0]["auto_eligible"] is True
    assert second["roles"]["worker-backend-dev"]["high"] == "deep"


# --- latest-version-only routing and family deny tokens -------------------

def _named(connection, model, name=None):
    entry = _entry(connection, model)
    entry.model_name = name
    return entry


# The live engine-confirmed lists (claude-agent-acp names its aliases'
# versions only in the display name).
LIVE_CODEX = [_named("codex", m) for m in ("gpt-6-astra", "gpt-6-sol", "gpt-6-luna", "gpt-5.6-sol",
                                           "gpt-5.6-terra", "gpt-5.6-luna")]
LIVE_CLAUDE = [_named("claude", m, n) for m, n in (
    ("opus[1m]", "Opus 5.5"), ("claude-opus-5[1m]", "claude-opus-5[1m]"), ("claude-fable-5-1[1m]", "Fable 5.1"),
    ("claude-fable-5", "claude-fable-5"), ("claude-sonnet-5", "Sonnet 5"), ("claude-opus-4-8", "Opus 4.8"),
    ("claude-opus-4-7", "Opus 4.7"), ("claude-sonnet-4-6", "Sonnet 4.6"))]


@pytest.mark.parametrize("model,name,expected", [
    ("gpt-6-luna", None, ("luna", (6,))),
    ("gpt-5.6-luna", "5.6 Luna", ("luna", (5, 6))),
    ("gpt-5.5", None, ("gpt", (5, 5))),
    ("claude-opus-4-8", None, ("opus", (4, 8))),
    ("claude-fable-5-1[1m]", None, ("fable", (5, 1))),
    ("claude-haiku-4-5-20251001", None, ("haiku", (4, 5))),
    ("opus[1m]", "Opus 5.5", ("opus", (5, 5))),
    ("haiku", "Haiku 4.5", ("haiku", (4, 5))),
    ("sonnet", "sonnet", ("sonnet", None)),
    ("default", "Default (recommended)", ("default", None)),
    ("opus", "Sonnet 5", ("opus", None)),  # a name for another family is never borrowed
])
def test_names_and_display_names_never_infer_policy(model, name, expected):
    policy = model_policy.policy_for("codex-acp", model)
    assert policy.tier is None and not policy.request_names
    assert auto_route.family_of("codex-acp/" + model) == "codex-acp/" + model


def test_exact_lists_never_infer_supersession():
    assert auto_route.superseded_routes(LIVE_CODEX + LIVE_CLAUDE) == {}


def test_new_releases_remain_unplaced_without_settings():
    entries = LIVE_CODEX + LIVE_CLAUDE + [_named("codex", "gpt-8-luna"), _named("claude", "claude-sonnet-5-1")]
    assert auto_route.superseded_routes(entries) == {}
    tiers = auto_route.model_tiers(entries, {}, models=MODELS)
    assert tiers["codex:gpt-8-luna"] is None and tiers["claude:claude-sonnet-5-1"] is None


def test_families_are_per_engine_and_unversioned_aliases_are_never_superseded():
    entries = [_named("claude", "sonnet", "sonnet"), _named("claude", "claude-sonnet-4-6", "Sonnet 4.6"),
               _named("codex", "gpt-5.6-terra")]
    assert auto_route.superseded_routes(entries) == {}


def _live_choose(monkeypatch, tmp_path, entries, routing=None):
    from cortex_runtime.connections import registry

    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True, exist_ok=True)
    environ = {"HOME": str(tmp_path)}
    (project / ".cortex" / "routing.json").write_text(json.dumps(routing_fixture(routing)))
    connections = {"claude": SimpleNamespace(launch_command=("claude",)),
                   "codex": SimpleNamespace(launch_command=("codex",))}
    monkeypatch.setattr(registry, "read_registry", lambda _: SimpleNamespace(connections=connections))
    monkeypatch.setattr(auto_route, "_entries", lambda overrides, *_: [
        e for e in entries if not auto_route._denied(overrides, e.route.engine, e.route.model)])
    monkeypatch.setattr(auto_route, "_is_seed", lambda _: False)
    monkeypatch.setattr(quota, "used_percent", lambda *_, **_kwargs: None)
    return project, environ


def test_choose_uses_exact_owner_selection_not_newest_name(tmp_path, monkeypatch):
    entries = LIVE_CODEX + LIVE_CLAUDE
    project, environ = _live_choose(monkeypatch, tmp_path, entries)
    # Light implementation work leans to Codex, so the newest Luna is chosen.
    choice = auto_route.choose("worker-backend-dev", project, environ, complexity="low")
    assert choice.model == "gpt-6-luna"
    assert choice.decision["superseded"] == {}
    assert "codex-acp/gpt-5.6-luna" in choice.decision["excluded_by_policy"]
    assert all(c["route"] != "codex-acp/gpt-5.6-luna" for c in choice.decision["candidates"])
    deep = auto_route.choose("worker-security-auditor", project, environ)
    assert {c["route"] for c in deep.decision["candidates"]} == {
        "codex-acp/gpt-6-sol", "claude-agent-acp/opus[1m]"}
    # Deny never silently promotes an unselected older version.
    project, environ = _live_choose(monkeypatch, tmp_path, entries, {"deny": ["gpt-6-luna"]})
    assert auto_route.choose("worker-backend-dev", project, environ, complexity="low").model == "gpt-5.6-terra"
    project, environ = _live_choose(monkeypatch, tmp_path, entries, {"deny": ["gpt-6-luna"],
        "models": {"codex-acp": {"auto": ["gpt-5.6-luna"]}}})
    assert auto_route.choose("worker-backend-dev", project, environ, complexity="low").model == "gpt-5.6-luna"


def test_an_explicit_pin_of_an_older_version_is_honoured(tmp_path, monkeypatch):
    project, environ = _live_choose(monkeypatch, tmp_path, LIVE_CODEX + LIVE_CLAUDE,
                                    {"pin": {"worker-explorer": "codex-acp/gpt-5.6-luna"}})
    choice = auto_route.choose("worker-explorer", project, environ)
    assert (choice.model, choice.basis) == ("gpt-5.6-luna", "auto:pin")


def test_a_model_hint_for_a_superseded_version_is_ignored(tmp_path, monkeypatch):
    project, environ = _live_choose(monkeypatch, tmp_path, LIVE_CODEX + LIVE_CLAUDE)
    choice = auto_route.choose("worker-backend-dev", project, environ, complexity="low",
                               model_hint="codex-acp/gpt-5.6-luna")
    assert choice.model == "gpt-6-luna"
    assert choice.decision["model_hint_status"] == "ignored:auto_excluded"


def test_route_menu_reports_superseded_versions(tmp_path, monkeypatch):
    project, environ = _live_choose(monkeypatch, tmp_path, LIVE_CODEX + LIVE_CLAUDE)
    menu = auto_route.route_menu(project, environ)
    items = {item["route"]: item for group in menu["tiers"].values() for item in group}
    old = items["claude-agent-acp/claude-opus-4-8"]
    assert (old["auto_eligible"], old["superseded_by"]) == (False, None)
    new = items["claude-agent-acp/opus[1m]"]
    assert (new["auto_eligible"], new["superseded_by"]) == (True, None)


@pytest.mark.parametrize("model", ["haiku", "claude-haiku-4-5", "claude-haiku-4-5-20251001", "claude-haiku-5[1m]"])
def test_a_bare_model_deny_is_exact_only(model):
    assert auto_route._listed(["haiku"], "claude-agent-acp", model) == (model == "haiku")
    assert not auto_route._listed(["codex-acp/haiku"], "claude-agent-acp", model)


def test_deny_tokens_never_match_another_family_or_version():
    deny = {"deny": ["gpt-5.5", "haiku", "claude-haiku-4-5-20251001"]}
    assert auto_route._listed(deny["deny"], "codex-acp", "gpt-5.5")
    assert not auto_route._listed(deny["deny"], "codex-acp", "gpt-5.5[1m]")
    for model in ("gpt-5.6-luna", "gpt-6-luna", "gpt-5.5-codex", "gpt-6-sol"):
        assert not auto_route._listed(deny["deny"], "codex-acp", model), model
    for model in ("opus[1m]", "claude-sonnet-5", "claude-fable-5-1[1m]", "sonnet"):
        assert not auto_route._listed(deny["deny"], "claude-agent-acp", model), model
    # A token with a variant is exact: it never widens to the whole family.
    assert not auto_route._listed(["opus[1m]"], "claude-agent-acp", "claude-opus-5[1m]")


def test_the_default_deny_removes_haiku_from_automatic_routing_and_the_selector(tmp_path, monkeypatch):
    entries = [_named("claude", "haiku", "Haiku 4.5"), _named("claude", "claude-haiku-4-5-20251001"),
               _named("codex", "gpt-5.5")] + LIVE_CODEX
    project, environ = _live_choose(monkeypatch, tmp_path, entries, {"deny": ["gpt-5.5", "haiku", "claude-haiku-4-5-20251001"]})
    menu = auto_route.route_menu(project, environ)
    routes = {item["route"] for group in menu["tiers"].values() for item in group}
    assert not any("haiku" in route or route.endswith("/gpt-5.5") for route in routes)
    assert auto_route.is_denied("claude-agent-acp", "claude-haiku-4-5-20251001", project, environ)
    with pytest.raises(PortableError) as refused:
        auto_route.refuse_if_denied("claude-agent-acp", "haiku", project, environ)
    assert refused.value.code == "route_denied"


# --- engine affinity, no cross-tier promotion, plan usage, decided_by -------

def test_default_engine_affinity_leans_only_implementation_and_operations_to_codex():
    codex_kinds = {kind for kind, engine in auto_route.WORK_KIND_ENGINES.items() if engine == "codex-acp"}
    assert codex_kinds == {"implementation", "operations"}
    assert auto_route.WORK_KIND_ENGINES["debugging"] is None
    assert auto_route.WORK_KIND_ENGINES["testing"] is None
    assert auto_route.WORK_KIND_ENGINES["investigation"] == "claude-agent-acp"
    assert auto_route.suitability("worker-debugger", "codex-acp") == 0
    assert auto_route.suitability("worker-debugger", "claude-agent-acp") == 0
    assert auto_route.suitability("worker-explorer", "claude-agent-acp") == 1
    # An unlisted role is implementation work.
    assert auto_route.affinity_engine("worker-unknown") == "codex-acp"


def test_engine_affinity_override_per_kind_then_star():
    affinity = {"implementation": "claude-agent-acp", "*": None}
    assert auto_route.affinity_engine("worker-backend-dev", affinity) == "claude-agent-acp"
    # "*" replaces every built-in kind the file does not name.
    assert auto_route.affinity_engine("worker-security-auditor", affinity) is None
    assert auto_route.affinity_engine("worker-devops-engineer", {"*": "claude-agent-acp"}) == "claude-agent-acp"
    assert auto_route.affinity_engine("worker-devops-engineer", {"review": None}) == "codex-acp"
    entries = [CODEX[4], CLAUDE[1]]
    picked, decision = auto_route.rank("standard", entries, _usage(), role="worker-backend-dev", models=MODELS)
    assert picked is CODEX[4] and decision["affinity"] == dict(
        work_kind="implementation", engine="codex-acp", source="default")
    picked, decision = auto_route.rank("standard", entries, _usage(), role="worker-backend-dev",
                                       affinity={"implementation": "claude-agent-acp"}, models=MODELS)
    assert picked is CLAUDE[1] and decision["decided_by"] == "affinity"
    assert decision["affinity"]["source"] == "routing.json"


def test_engine_affinity_validation(tmp_path):
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    environ = {"HOME": str(tmp_path)}
    route_file = project / ".cortex" / "routing.json"
    valid = {"engine_affinity": {"implementation": "claude-agent-acp", "debugging": None, "*": "codex-acp"}}
    route_file.write_text(json.dumps(routing_fixture(valid)))
    assert auto_route.load_overrides(project, environ) == routing_fixture(valid)
    for bad, needle in (({"engine_affinity": ["codex-acp"]}, "must map"),
                        ({"engine_affinity": {"coding": "codex-acp"}}, "unknown work kind 'coding'"),
                        ({"engine_affinity": {"review": "gemini-acp"}}, "unknown engine 'gemini-acp'"),
                        ({"engine_affinity": {"review": ""}}, "unknown engine ''")):
        route_file.write_text(json.dumps(bad))
        with pytest.raises(PortableError) as refused:
            auto_route.load_overrides(project, environ)
        assert refused.value.code == "invalid_routing_overrides" and needle in refused.value.remediation


def test_engine_affinity_from_routing_json_reaches_choose(tmp_path, monkeypatch):
    project, environ = _live_choose(monkeypatch, tmp_path, LIVE_CODEX + LIVE_CLAUDE,
                                    {"engine_affinity": {"implementation": "claude-agent-acp"}})
    choice = auto_route.choose("worker-backend-dev", project, environ)
    assert choice.model == "claude-sonnet-5"
    assert choice.decision["affinity"]["source"] == "routing.json"


# The observed deployment: Haiku auto-excluded, so light has only Codex.
NO_HAIKU = frozenset({"claude:haiku"})


def test_a_single_engine_light_tier_is_never_widened_with_a_stronger_model():
    """Light work stays on a light model even when only one engine has one:
    another engine lacking a light model is no reason to spend a stronger
    tier. ``auto_allow`` Haiku is the lever for Claude on light work."""
    assert not hasattr(auto_route, "cross_tier_candidates")
    for role in ("worker-technical-writer", "worker-explorer", "worker-planner", "worker-backend-dev"):
        picked, decision = auto_route.rank("light", ENTRIES, _usage(), excluded=NO_HAIKU,
                                           policy_excluded=NO_HAIKU, role=role, models=MODELS)
        assert picked.route.engine == "codex-acp" and decision["tier"] == "light", role
        assert {c["route"] for c in decision["candidates"]} == {
            "codex-acp/gpt-6-luna", "codex-acp/gpt-5.6-luna", "codex-acp/gpt-5.5"}
        assert "cross_tier" not in decision and "chosen_cross_tier" not in decision
        assert all("cross_tier" not in c for c in decision["candidates"])
    # Even a Claude plan with far more room does not pull light work up to Sonnet.
    picked, decision = auto_route.rank("light", ENTRIES, _usage(codex=80, claude=0), excluded=NO_HAIKU,
                                       policy_excluded=NO_HAIKU, role="worker-technical-writer", models=MODELS)
    assert picked.route.model == "gpt-6-luna" and decision["tier"] == "light"


def test_no_task_is_promoted_above_its_tier_while_the_tier_has_a_usable_model():
    tiers = auto_route.model_tiers(ENTRIES, {}, models=MODELS)
    # Standard work with only Codex in standard never gets Claude's deep model.
    live = [e for e in LIVE_CODEX + LIVE_CLAUDE if "sonnet" not in e.route.model]
    superseded = auto_route.superseded_routes(live)
    live_tiers = auto_route.model_tiers(live, {}, models=MODELS)
    _, decision = auto_route.rank("standard", live, _usage(), excluded=frozenset(superseded),
                                  policy_excluded=frozenset(superseded), superseded=superseded,
                                  role="worker-code-reviewer", models=MODELS)
    assert decision["tier"] == "standard"
    assert {live_tiers[c["connection_id"] + ":" + c["route"].split("/", 1)[1]] for c in decision["candidates"]} == {
        "standard"}
    # A Claude hint or preference cannot lift light work to a stronger Claude model.
    for kwargs in (dict(model_hint="claude-agent-acp"), dict(model_hint="claude-agent-acp/sonnet"),
                   dict(preference="claude-agent-acp")):
        picked, decision = auto_route.rank("light", ENTRIES, _usage(), excluded=NO_HAIKU,
                                           policy_excluded=NO_HAIKU, role="worker-technical-writer", **kwargs, models=MODELS)
        assert tiers[picked.route.id] == "light" and picked.route.engine == "codex-acp", kwargs
    # Only when no engine has a usable model in the tier does it fall back.
    no_light = frozenset(e.route.id for e in ENTRIES if tiers[e.route.id] == "light")
    picked, decision = auto_route.rank("light", ENTRIES, _usage(), excluded=no_light, role="worker-technical-writer", models=MODELS)
    assert decision["tier"] == "standard" and decision["role_tier"] == "light"


def test_light_work_runs_on_the_light_model_at_the_light_effort(tmp_path, monkeypatch):
    project, environ = _live_choose(monkeypatch, tmp_path, LIVE_CODEX + LIVE_CLAUDE)
    choice = auto_route.choose("worker-technical-writer", project, environ)
    # Luna never runs below high (its light-work effort).
    assert (choice.model, choice.effort) == ("gpt-6-luna", "high")
    assert choice.decision["effort_basis"] == "stretch:light"
    assert choice.decision["tier"] == "light" and choice.decision["decided_by"] == "only_candidate"
    # Allowing Haiku is the owner's lever for Claude on light work.
    haiku = _named("claude", "claude-haiku-4-5-20251001", "Haiku 4.5")
    project, environ = _live_choose(monkeypatch, tmp_path, LIVE_CODEX + LIVE_CLAUDE + [haiku],
                                    {"auto_allow": ["haiku"], "models": {"claude-agent-acp": {"auto": ["claude-haiku-4-5-20251001"]}}})
    choice = auto_route.choose("worker-technical-writer", project, environ)
    assert choice.model == "claude-haiku-4-5-20251001" and choice.decision["decided_by"] == "affinity"


def test_hint_is_honoured_unless_the_hinted_plan_is_near_its_quota_band():
    assert auto_route.HINT_TOLERANCE == auto_route.HOT_PERCENT == 95.0
    entries = [CODEX[4], CLAUDE[1]]
    for codex, claude, status, chosen in (
            (94, 10, "honoured", "codex-acp/gpt-5.6-terra"),
            (95, 10, "ignored:near_quota_limit", "claude-agent-acp/sonnet"),
            # Every plan near its limit: nothing better to protect.
            (95, 90, "honoured", "codex-acp/gpt-5.6-terra")):
        _, decision = auto_route.rank("standard", entries, _usage(codex, claude),
                                      role="worker-code-reviewer", model_hint="codex-acp", models=MODELS)
        assert (decision["model_hint_status"], decision["chosen"]) == (status, chosen), (codex, claude)
    # An honoured hint outranks plan usage.
    _, decision = auto_route.rank("standard", entries, _usage(94, 10),
                                  role="worker-code-reviewer", model_hint="codex-acp", models=MODELS)
    assert decision["decided_by"] == "hint"



def test_a_preference_near_its_quota_band_is_ignored_in_every_tier():
    picked, decision = _rank("light", "worker-backend-dev", preference="codex-acp", codex=95, claude=10)
    assert picked.route.engine == "claude-agent-acp"
    assert decision["preference_status"] == "ignored:provider_near_quota_limit"


def test_every_tier_orders_a_hot_account_last():
    """Light work used to spend a hot plan first (7b7aca22); the hot/non-hot
    quota flag (rank version 11) now decides this directly, ahead of
    the usage-gap band."""
    assert auto_route._order_key(10.0) < auto_route._order_key(95.0)
    assert auto_route._order_key(None) == auto_route._order_key(0.0)
    entries = [CODEX[2], CLAUDE[2]]  # gpt-6-luna, Claude haiku (both light)
    picked, decision = auto_route.rank("light", entries, _usage(codex=10, claude=95), role="worker-explorer", models=MODELS)
    assert picked is CODEX[2] and decision["decided_by"] == "quota"
    picked, decision = auto_route.rank("light", entries, _usage(codex=95, claude=10), role="worker-backend-dev", models=MODELS)
    assert picked is CLAUDE[2] and decision["decided_by"] == "quota"


# --- quota band restored ahead of affinity (rank version 11) --------

def test_a_known_hot_candidate_now_always_outranks_affinity():
    """Even a one-point, far-inside-USAGE_MARGIN crossing
    of HOT_PERCENT now outranks engine affinity -- the task/model fit --
    though a prior design (rank version 6) let affinity decide here. A
    known >=95% candidate must rank behind any non-hot one, known or
    unknown, never merely a late tie breaker affinity could still beat."""
    entries = [CODEX[4], CLAUDE[1]]  # gpt-5.6-terra, sonnet (both standard)
    # Codex is preferred for implementation work, but is the (barely) hot one.
    picked, decision = auto_route.rank("standard", entries, _usage(codex=96, claude=94),
                                       role="worker-backend-dev", models=MODELS)
    assert picked.route.engine == "claude-agent-acp" and decision["decided_by"] == "quota"
    assert decision["candidates"][0]["hot"] is False


def test_a_small_near_boundary_gap_falls_to_the_quota_tie_breaker():
    """With no preference, hint, diversify or affinity difference, a small
    (sub-USAGE_MARGIN) gap that straddles HOT_PERCENT is still decided by
    the quota band -- it remains a meaningful, if late, signal."""
    entries = [CODEX[4], CLAUDE[1]]
    picked, decision = auto_route.rank("standard", entries, _usage(codex=96, claude=94), models=MODELS)
    assert picked.route.engine == "claude-agent-acp" and decision["decided_by"] == "quota"
    picked, decision = auto_route.rank("standard", entries, _usage(codex=94, claude=96), models=MODELS)
    assert picked.route.engine == "codex-acp" and decision["decided_by"] == "quota"


@pytest.mark.parametrize("codex,claude,expected_engine,expected_decided_by", [
    # 94% is not hot: the account below HOT_PERCENT is preferred at the
    # quota tie breaker even with only a 1-point gap (inside USAGE_MARGIN).
    (94.0, 95.0, "codex-acp", "quota"),
    # Exactly 95% is hot (HOT_PERCENT is inclusive).
    (95.0, 94.0, "claude-agent-acp", "quota"),
    (96.0, 94.0, "claude-agent-acp", "quota"),
])
def test_boundary_percentages_around_94_95_96(codex, claude, expected_engine, expected_decided_by):
    entries = [CODEX[4], CLAUDE[1]]
    picked, decision = auto_route.rank("standard", entries, _usage(codex=codex, claude=claude), models=MODELS)
    assert picked.route.engine == expected_engine
    assert decision["decided_by"] == expected_decided_by


def test_a_large_gap_also_decides_through_the_now_earlier_quota_flag():
    """A gap well past USAGE_MARGIN still picks the non-hot candidate, but
    (rank version 11) the hot/non-hot quota flag decides it directly,
    ahead of the usage-gap band it used to reach first."""
    entries = [CODEX[4], CLAUDE[1]]
    picked, decision = auto_route.rank("standard", entries, _usage(codex=95, claude=20), models=MODELS)
    assert picked.route.engine == "claude-agent-acp" and decision["decided_by"] == "quota"


def test_both_hot_candidates_are_still_separated_by_the_usage_gap_or_exact_percent():
    entries = [CODEX[4], CLAUDE[1]]
    # Codex (99%) is hot; Claude (87%) is not, so quota decides this one
    # directly now -- not the usage-gap band it used to reach first.
    picked, decision = auto_route.rank("standard", entries, _usage(codex=99, claude=87), models=MODELS)
    assert picked.route.engine == "claude-agent-acp" and decision["decided_by"] == "quota"
    # An immaterial gap between two hot plans: exact used percent decides.
    picked, decision = auto_route.rank("standard", entries, _usage(codex=99, claude=96), models=MODELS)
    assert picked.route.engine == "claude-agent-acp" and decision["decided_by"] == "used_percent"


def test_hint_ignored_only_when_hot_and_meaningfully_worse_than_an_alternative():
    entries = [CODEX[4], CLAUDE[1]]
    # Hinted route is hot but the alternative is barely better: honoured.
    _, decision = auto_route.rank("standard", entries, _usage(codex=95, claude=96),
                                  role="worker-backend-dev", model_hint="codex-acp", models=MODELS)
    assert decision["model_hint_status"] == "honoured"
    # Hinted route is hot and meaningfully worse than the alternative: ignored.
    _, decision = auto_route.rank("standard", entries, _usage(codex=95, claude=70),
                                  role="worker-backend-dev", model_hint="codex-acp", models=MODELS)
    assert decision["model_hint_status"] == "ignored:near_quota_limit"
    # Hinted route is not hot at all: honoured even with a large gap.
    _, decision = auto_route.rank("standard", entries, _usage(codex=94, claude=10),
                                  role="worker-backend-dev", model_hint="codex-acp", models=MODELS)
    assert decision["model_hint_status"] == "honoured"
    # Hinted route's usage is unknown: quota stays neutral, honoured.
    _, decision = auto_route.rank("standard", entries, _usage(None, 10),
                                  role="worker-backend-dev", model_hint="codex-acp", models=MODELS)
    assert decision["model_hint_status"] == "honoured"
    # The alternative's usage is unknown: nothing to compare against, honoured.
    _, decision = auto_route.rank("standard", entries, _usage(95, None),
                                  role="worker-backend-dev", model_hint="codex-acp", models=MODELS)
    assert decision["model_hint_status"] == "honoured"


def test_preference_ignored_only_when_hot_and_meaningfully_worse_than_an_alternative():
    picked, decision = _rank("standard", "worker-backend-dev", preference="codex-acp", codex=95, claude=96)
    assert picked.route.engine == "codex-acp" and decision["preference_status"] == "honoured"
    picked, decision = _rank("standard", "worker-backend-dev", preference="codex-acp", codex=95, claude=70)
    assert picked.route.engine == "claude-agent-acp"
    assert decision["preference_status"] == "ignored:provider_near_quota_limit"


def test_exhausted_100_percent_remains_a_hard_exclusion_regardless_of_margin():
    """The 100% exclusion is unaffected by the softer 95% band: an
    exhausted route never enters the rank, hinted or not."""
    entries = [CODEX[4], CLAUDE[1]]
    _, decision = auto_route.rank("standard", entries, _usage(codex=100, claude=99),
                                  role="worker-backend-dev", model_hint="codex-acp", models=MODELS)
    assert decision["chosen"] == "claude-agent-acp/sonnet"
    assert all(c["route"] != "codex-acp/gpt-5.6-terra" for c in decision["candidates"])
    assert decision["model_hint_status"] == "ignored:unavailable"


@pytest.mark.parametrize("kwargs,expected", [
    (dict(preference="claude-agent-acp"), "preference"),
    (dict(model_hint="claude-agent-acp/sonnet"), "hint"),
    (dict(usage=_usage(codex=95, claude=10)), "quota"),
    (dict(escalated=True, previous_engine="codex-acp"), "diversify"),
    (dict(usage=_usage(codex=95, claude=20)), "quota"),
    (dict(), "affinity"),
    (dict(usage=_usage(codex=99, claude=96)), "affinity"),
    (dict(role=None, usage=_usage(codex=99, claude=96)), "used_percent"),
    (dict(role=None), "engine_order"),
])
def test_decided_by_names_the_first_separating_key(kwargs, expected):
    entries = [CODEX[4], CLAUDE[1]]
    kwargs = dict(dict(role="worker-backend-dev", usage=_usage()), **kwargs)
    usage = kwargs.pop("usage")
    _, decision = auto_route.rank("standard", entries, usage, **kwargs, models=MODELS)
    assert decision["decided_by"] == expected
    _, decision = auto_route.rank("standard", [CODEX[4]], usage, **kwargs, models=MODELS)
    assert decision["decided_by"] == "only_candidate"


# --- the configured stretch policy ------------------------------------------------------

def _all_efforts(entries):
    for entry in entries:
        entry.route.supported_parameters = tuple(("thought_level", e) for e in ALL)
    return entries


def _live_all(monkeypatch, tmp_path, routing=None, codex=None, claude=None):
    entries = _all_efforts([_named("codex", e.route.model) for e in LIVE_CODEX]
                           + [_named("claude", e.route.model, e.model_name) for e in LIVE_CLAUDE])
    project, environ = _live_choose(monkeypatch, tmp_path, entries, routing)
    monkeypatch.setattr(quota, "used_percent", lambda record, *_, **_kwargs: (
        codex if record.launch_command == ("codex",) else claude))
    return project, environ


def test_stretch_effort_steps_with_the_work_and_never_goes_below_high():
    assert "high" == "high"
    assert auto_route.stretch_effort("light", ladder=("high", "xhigh", "max")) == "high"
    assert auto_route.stretch_effort("light", escalate=True, ladder=("high", "xhigh", "max")) == "xhigh"
    assert auto_route.stretch_effort("standard", ladder=("high", "xhigh", "max")) == "xhigh"
    assert auto_route.stretch_effort("standard", escalate=True, ladder=("high", "xhigh", "max")) == "max"
    assert _is_stretch("gpt-6-luna") and _is_stretch("gpt-7-luna")
    assert not _is_stretch("gpt-5.6-terra")


def test_light_work_runs_stretch_at_high(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path)
    for role, complexity in (("worker-technical-writer", "normal"), ("worker-backend-dev", "low"),
                             ("worker-explorer", "high")):
        choice = auto_route.choose(role, project, environ, complexity=complexity)
        assert (choice.model, choice.effort) == ("gpt-6-luna", "high"), role
        assert choice.decision["stretch"]["status"] == "light_tier"
    # A pin to Luna at low is raised to the floor, not honoured below it.
    project, environ = _live_all(monkeypatch, tmp_path, {"pin": {"*": "codex-acp/gpt-6-luna:low"}})
    choice = auto_route.choose("worker-backend-dev", project, environ)
    assert (choice.model, choice.effort) == ("gpt-6-luna", "high")
    assert choice.decision["stretch"] == dict(status="pinned", chosen=True)
    # A worker's requested effort is clamped the same way at dispatch.
    assert auto_route.allowed_efforts("codex-acp", "gpt-6-luna", project, environ) == ("high", "xhigh", "max")


def test_normal_testing_ranks_stretch_with_the_standard_models(tmp_path, monkeypatch):
    assert frozenset({"testing"}) == frozenset({"testing"})
    project, environ = _live_all(monkeypatch, tmp_path, codex=20.0, claude=20.0,
                                 routing={"engine_affinity": {"testing": "codex-acp"}})
    choice = auto_route.choose("worker-qa-engineer", project, environ)
    decision = choice.decision
    assert decision["tier"] == "standard" and decision["role_tier"] == "standard"
    assert {c["route"] for c in decision["candidates"]} == {
        "codex-acp/gpt-6-luna", "codex-acp/gpt-5.6-terra", "claude-agent-acp/claude-sonnet-5"}
    # Same Codex plan and affinity: Codex's own list order (newest
    # generation first) puts gpt-6-luna ahead of the older gpt-5.6-terra.
    assert (choice.model, choice.effort) == ("gpt-6-luna", "xhigh")
    assert decision["effort_basis"] == "stretch:standard"
    assert decision["stretch"] == dict(status="standard_candidate", candidate=True, chosen=True, outranked_by=None)
    # The rank still decides: a hot Codex plan sends the work to Sonnet.
    project, environ = _live_all(monkeypatch, tmp_path, codex=95.0, claude=10.0)
    choice = auto_route.choose("worker-qa-engineer", project, environ)
    assert choice.model == "claude-sonnet-5" and choice.effort == "medium"
    assert choice.decision["stretch"] == dict(status="standard_candidate", candidate=True, chosen=False,
                                           outranked_by="quota")
    # A coordinator hint picks either standard model; a Luna hint is not a tier mismatch.
    project, environ = _live_all(monkeypatch, tmp_path)
    choice = auto_route.choose("worker-qa-engineer", project, environ, model_hint="codex-acp/gpt-5.6-terra",
                               model_reason="Terra for this module.")
    assert choice.model == "gpt-5.6-terra" and choice.decision["stretch"]["outranked_by"] == "hint"
    choice = auto_route.choose("worker-qa-engineer", project, environ, model_hint="codex-acp/gpt-6-luna",
                               model_reason="Cheap and strong enough.")
    assert choice.decision["model_hint_status"] == "honoured"
    assert (choice.model, choice.effort) == ("gpt-6-luna", "xhigh")


@pytest.mark.parametrize("complexity", ["low", "normal"])
def test_escalated_standard_testing_runs_stretch_at_max(tmp_path, monkeypatch, complexity):
    project, environ = _live_all(monkeypatch, tmp_path, codex=20.0, claude=20.0,
                                 routing={"engine_affinity": {"testing": "codex-acp"}})
    choice = auto_route.choose("worker-qa-engineer", project, environ, complexity=complexity, escalate=True)
    assert choice.decision["base_role_tier"] == ("light" if complexity == "low" else "standard")
    assert choice.decision["role_tier"] == "standard"
    assert choice.decision["stretch"]["candidate"] is True
    assert (choice.model, choice.effort) == ("gpt-6-luna", "max")
    assert choice.decision["effort_basis"] == "stretch:standard:escalated"
    # Escalated light work stays light: one Luna step up.
    choice = auto_route.choose("worker-explorer", project, environ, escalate=True)
    assert (choice.model, choice.effort) == ("gpt-6-luna", "xhigh")


@pytest.mark.parametrize("role,kwargs,status", [
    ("worker-backend-dev", dict(complexity="high"), "excluded:complexity_high"),
    ("worker-planner", dict(complexity="high"), "excluded:complexity_high"),
    ("worker-backend-dev", dict(complexity="high", escalate=True), "excluded:complexity_high"),
    ("worker-code-reviewer", dict(escalate=True), "excluded:deep_work"),
    ("worker-architect", dict(complexity="low", escalate=True), "excluded:deep_work"),
    ("worker-architect", dict(complexity="low"), "excluded:deep_role"),
    ("worker-security-auditor", dict(escalate=True), "excluded:flagship_work"),
    ("worker-architect", dict(), "excluded:deep_work"),
])
def test_hard_deep_and_flagship_work_never_goes_to_stretch(tmp_path, monkeypatch, role, kwargs, status):
    project, environ = _live_all(monkeypatch, tmp_path)
    choice = auto_route.choose(role, project, environ, **kwargs)
    assert not _is_stretch(choice.model)
    assert choice.decision["stretch"]["status"] == status
    assert all("luna" not in c["route"] for c in choice.decision["candidates"])
    # Not even as the last fallback: with only Luna left, the work is refused.
    luna_only = _all_efforts([_named("codex", "gpt-6-luna")])
    project, environ = _live_choose(monkeypatch, tmp_path, luna_only)
    with pytest.raises(PortableError) as refused:
        auto_route.choose(role, project, environ, **kwargs)
    assert refused.value.code == "no_available_route"
    assert "(codex-acp/gpt-6-luna) are not used for this work" in refused.value.remediation


@pytest.mark.parametrize("role,kwargs", [
    ("worker-backend-dev", {}),
    ("worker-refactorer", {}),
    ("worker-devops-engineer", {}),
    ("worker-debugger", {}),
    ("worker-performance-engineer", {}),
    ("worker-ux-designer", {}),
    ("worker-backend-dev", dict(complexity="low", escalate=True)),
    ("worker-technical-writer", dict(escalate=True)),
])
def test_standard_work_that_is_not_testing_is_not_a_stretch_candidate(tmp_path, monkeypatch, role, kwargs):
    project, environ = _live_all(monkeypatch, tmp_path)
    for model_hint in ("auto", "codex-acp/gpt-6-luna"):
        choice = auto_route.choose(role, project, environ, model_hint=model_hint,
                                   model_reason=None if model_hint == "auto" else "Cheap.", **kwargs)
        assert choice.decision["tier"] == "standard"
        assert choice.decision["stretch"]["status"].startswith("not_candidate:work_kind_")
        assert choice.decision["stretch"]["candidate"] is False
        assert all("luna" not in c["route"] for c in choice.decision["candidates"])
        assert not _is_stretch(choice.model)


def test_standard_development_retry_escalates_without_stretch(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path)
    choice = auto_route.choose("worker-backend-dev", project, environ, escalate=True)
    assert choice.decision["base_role_tier"] == "standard"
    assert choice.decision["role_tier"] == choice.decision["tier"] == "deep"
    assert not _is_stretch(choice.model)
    assert choice.decision["stretch"]["status"] == "excluded:deep_work"


def test_route_menu_and_hint_preview_describe_the_stretch_rule(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path)
    menu = auto_route.route_menu(project, environ)
    luna = next(item for item in menu["tiers"]["light"] if item["model"] == "gpt-6-luna")
    assert luna["efforts"] == ["high", "xhigh", "max"]
    stretch = {item["route"]: item for item in menu["stretch"]}
    assert set(stretch) == {"codex-acp/gpt-6-luna", "codex-acp/gpt-5.6-luna"}
    rule = stretch["codex-acp/gpt-6-luna"]
    assert rule["efforts"] == dict(light="high", standard="xhigh", standard_escalated="max")
    assert rule["standard_candidate_roles"] == ["worker-qa-engineer"]
    assert (rule["effort_floor"], rule["standard_work_kinds"], rule["auto_eligible"]) == ("high", ["testing"], True)
    assert stretch["codex-acp/gpt-5.6-luna"]["auto_eligible"] is False
    assert luna["policy"]["attribute_sources"] == dict(effort_floor="configured", effort_ladder="configured",
                                                       also_standard_for="configured")
    assert (luna["tier_source"], luna["selection"], luna["auto_reason"]) == ("configured", "listed", "eligible")
    assert "luna" not in menu and "flagship_note" not in menu
    assert auto_route.hint_serves_tier("gpt-6-luna", "light", "worker-qa-engineer", "standard", also_standard_for=["testing"])
    assert not auto_route.hint_serves_tier("gpt-6-luna", "light", "worker-backend-dev", "standard")
    assert not auto_route.hint_serves_tier("gpt-6-luna", "light", "worker-debugger", "standard")
    assert not auto_route.hint_serves_tier("gpt-6-luna", "light", "worker-qa-engineer", "deep", "high")
    assert not auto_route.hint_serves_tier("gpt-6-luna", "light", "worker-ux-designer", "standard")
    assert not auto_route.hint_serves_tier("gpt-5.6-terra", "standard", "worker-backend-dev", "light", "low")


@pytest.mark.parametrize("previous_engine,codex,claude,reason", [
    ("codex-acp", 20.0, 20.0, "diversify"),
    ("claude-agent-acp", 95.0, 20.0, "quota"),
])
def test_standard_retry_can_choose_claude_over_stretch(tmp_path, monkeypatch, previous_engine, codex, claude, reason):
    project, environ = _live_all(monkeypatch, tmp_path, codex=codex, claude=claude)
    choice = auto_route.choose("worker-qa-engineer", project, environ, escalate=True,
                               previous_engine=previous_engine, model_hint="codex-acp/gpt-6-luna")
    assert (choice.model, choice.effort) == ("claude-sonnet-5", "high")
    assert choice.decision["model_hint_status"] == "ignored:escalated"
    assert choice.decision["stretch"] == dict(status="standard_candidate", candidate=True,
                                          chosen=False, outranked_by=reason)


def test_standard_retry_falls_back_when_no_standard_or_stretch_route_is_usable(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path, {
        "deny": ["gpt-6-luna", "gpt-5.6-luna", "gpt-5.6-terra", "claude-sonnet-5", "claude-sonnet-4-6", "gpt-5.6-sol"]})
    choice = auto_route.choose("worker-qa-engineer", project, environ, escalate=True)
    assert choice.decision["base_role_tier"] == "standard"
    assert choice.decision["role_tier"] == "deep"
    assert choice.decision["tier"] == "deep"
    assert not _is_stretch(choice.model)
    assert choice.effort == "high"


@pytest.mark.parametrize("suffix,effort", [("", "high"), (":medium", "medium")])
def test_standard_retry_preserves_non_stretch_pin_effort(tmp_path, monkeypatch, suffix, effort):
    project, environ = _live_all(monkeypatch, tmp_path, {
        "pin": {"worker-qa-engineer": "claude-agent-acp/claude-sonnet-5" + suffix}})
    choice = auto_route.choose("worker-qa-engineer", project, environ, escalate=True)
    assert (choice.model, choice.effort, choice.basis) == ("claude-sonnet-5", effort, "auto:pin")


# --- a flagship route the user explicitly asked for ----------------------------

ASTRA = "codex-acp/gpt-6-astra"
FABLE = "claude-agent-acp/claude-fable-5-1[1m]"
USER_ASKED = dict(source_ref="r_0123456789ab:-1", family="astra", matched="Astra")


def test_a_user_requested_flagship_hint_runs_on_that_route_and_is_audited(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path)
    choice = auto_route.choose("worker-backend-dev", project, environ, model_hint=ASTRA,
                               model_reason="The user asked for Astra.", user_request=USER_ASKED)
    assert (choice.model, choice.effort, choice.basis) == ("gpt-6-astra", "high", "auto:user_request")
    decision = choice.decision
    assert (decision["base_role_tier"], decision["role_tier"], decision["tier"]) == ("standard", "flagship", "flagship")
    assert (decision["basis"], decision["decided_by"]) == ("user_request", "user_request")
    assert decision["model_hint_status"] == "honoured"
    assert decision["user_model_request"] == dict(USER_ASKED, status="honoured")
    assert [c["route"] for c in decision["candidates"]] == [ASTRA]
    fable = auto_route.choose("worker-code-reviewer", project, environ, model_hint=FABLE,
                              user_request=dict(USER_ASKED, family="fable", matched="Fable"))
    assert (fable.model, fable.effort, fable.basis) == ("claude-fable-5-1[1m]", "high", "auto:user_request")


def test_a_flagship_hint_without_a_user_request_stays_escalation_only(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path)
    choice = auto_route.choose("worker-backend-dev", project, environ, model_hint=ASTRA,
                               model_reason="The user asked for Astra.")
    assert choice.model != "gpt-6-astra" and choice.basis == "auto"
    assert (choice.decision["role_tier"], choice.decision["tier"]) == ("standard", "standard")
    assert choice.decision["model_hint_status"] == "ignored:tier_mismatch"
    assert choice.decision["user_model_request"] == dict(status="not_requested_by_user")


@pytest.mark.parametrize("routing,codex,unavailable,reason", [
    ({"deny": ["gpt-6-astra"]}, None, (), "denied"),
    ({"providers": ["claude-agent-acp"]}, None, (), "provider not allowed"),
    (None, 100.0, (), "quota exhausted"),
    (None, None, ({"connection_id": "codex", "model": "gpt-6-astra", "engine": "codex-acp",
                   "reason": "quota_exhausted", "expires_at": "later"},), "unavailable"),
])
def test_hard_refusal_never_substitutes_a_user_requested_flagship(tmp_path, monkeypatch, routing, codex, unavailable, reason):
    project, environ = _live_all(monkeypatch, tmp_path, routing, codex=codex)
    with pytest.raises(PortableError, match="user_requested_route_unavailable") as caught:
        auto_route.choose("worker-backend-dev", project, environ, model_hint=ASTRA,
                          user_request=USER_ASKED, unavailable=unavailable)
    assert reason in caught.value.remediation and ASTRA in caught.value.remediation
    assert "no substitution" in caught.value.remediation
    assert caught.value.retry_eligible is False


def test_user_requested_flagship_overrides_a_pin(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path, {"pin": {"backend-dev": "codex-acp/gpt-6-sol"}})
    choice = auto_route.choose("worker-backend-dev", project, environ, model_hint=ASTRA, user_request=USER_ASKED)
    assert (choice.model, choice.basis) == ("gpt-6-astra", "auto:user_request")
    assert choice.decision["user_model_request"] == dict(USER_ASKED, status="honoured")


def test_user_request_applies_to_non_flagship_but_not_unhinted_nodes(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path)
    sonnet = auto_route.choose("worker-backend-dev", project, environ, model_hint="claude-agent-acp/claude-sonnet-5",
                               user_request=dict(USER_ASKED, family="sonnet"))
    assert (sonnet.model, sonnet.basis, sonnet.decision["tier"]) == ("claude-sonnet-5", "auto:user_request", "standard")
    assert sonnet.decision["user_model_request"]["status"] == "honoured"
    plain = auto_route.choose("worker-backend-dev", project, environ)
    assert plain.decision["tier"] == "standard" and "user_model_request" not in plain.decision


def test_a_user_requested_flagship_survives_a_retry(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path)
    choice = auto_route.choose("worker-backend-dev", project, environ, model_hint=ASTRA, user_request=USER_ASKED,
                               escalate=True, previous_engine="codex-acp")
    assert (choice.model, choice.basis) == ("gpt-6-astra", "auto:user_request")
    assert choice.decision["escalated"] is True


def test_route_menu_names_the_user_request_exception(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path)
    menu = auto_route.route_menu(project, environ)
    assert menu["flagship"]["rule"] == "escalation, or an exact route the user explicitly asked for in this task"
    assert {item["route"] for item in menu["tiers"]["flagship"]} >= {ASTRA, FABLE}
    assert set(menu["flagship"]["routes"]) >= {ASTRA, FABLE}
    assert menu["flagship"]["request_names"][ASTRA] == ["astra", "астр"]
    assert menu["affinity"]["implementation"] == "codex-acp" and menu["affinity"]["testing"] is None
    assert menu["role_work_kinds"]["worker-qa-engineer"] == "testing"
    assert menu["preferred"] == {}


# --- routing.json models: selection, tiers and per-model policy ---------------

OPENCODE_PLAIN = "vendor/nebula"
OPENCODE_VARIANT = "vendor/nebula,thinking"


def _opencode(model):
    route = SimpleNamespace(id=f"opencode:{model}", engine="opencode", model=model, supported_parameters=())
    return SimpleNamespace(connection_id="opencode", route=route, model_name=model)


def _configured(monkeypatch, tmp_path, routing=None, extra=(), codex=None, claude=None):
    from cortex_runtime.connections import registry

    entries = _all_efforts([_named("codex", e.route.model) for e in LIVE_CODEX]
                           + [_named("claude", e.route.model, e.model_name) for e in LIVE_CLAUDE]) + list(extra)
    project, environ = _live_choose(monkeypatch, tmp_path, entries, routing)
    connections = {name: SimpleNamespace(launch_command=(name,)) for name in ("claude", "codex", "opencode")}
    monkeypatch.setattr(registry, "read_registry", lambda _: SimpleNamespace(connections=connections))
    monkeypatch.setattr(quota, "used_percent", lambda record, *_, **_kwargs: (
        codex if record.launch_command == ("codex",) else claude if record.launch_command == ("claude",) else None))
    return project, environ


def _menu_items(menu):
    return {item["route"]: dict(item, tier=tier) for tier, group in menu["tiers"].items() for item in group}


def test_optional_engine_models_stay_off_until_enabled(tmp_path, monkeypatch):
    project, environ = _configured(monkeypatch, tmp_path, None, [_opencode(OPENCODE_PLAIN), _opencode(OPENCODE_VARIANT)])
    items = _menu_items(auto_route.route_menu(project, environ))
    assert not any(route.startswith("opencode/") for route in items)
    assert auto_route.is_denied("opencode", OPENCODE_VARIANT, project, environ)
    assert not auto_route.is_denied("codex-acp", "gpt-6-luna", project, environ)


def test_without_models_an_optional_engine_stays_out_of_automatic_routing(tmp_path, monkeypatch):
    routing = {"enabled": {"opencode": [OPENCODE_PLAIN, OPENCODE_VARIANT]}}
    project, environ = _configured(monkeypatch, tmp_path, routing, [_opencode(OPENCODE_PLAIN), _opencode(OPENCODE_VARIANT)])
    items = _menu_items(auto_route.route_menu(project, environ))
    assert f"opencode/{OPENCODE_VARIANT}" not in items
    choice = auto_route.choose("worker-backend-dev", project, environ)
    assert all(not c["route"].startswith("opencode/") for c in choice.decision["candidates"])


def test_a_selection_list_makes_exactly_the_listed_models_automatic(tmp_path, monkeypatch):
    routing = {"enabled": {"opencode": [OPENCODE_PLAIN, OPENCODE_VARIANT]}, "models": {
        "opencode": {"auto": [OPENCODE_VARIANT], "tiers": {OPENCODE_VARIANT: "standard"}},
        "codex-acp": {"auto": ["gpt-5.6-terra"]}}}
    project, environ = _configured(monkeypatch, tmp_path, routing, [_opencode(OPENCODE_PLAIN), _opencode(OPENCODE_VARIANT)])
    items = _menu_items(auto_route.route_menu(project, environ))
    thinking = items[f"opencode/{OPENCODE_VARIANT}"]
    assert (thinking["tier"], thinking["tier_source"], thinking["selection"]) == ("standard", "configured", "listed")
    assert thinking["auto_eligible"] is True
    assert f"opencode/{OPENCODE_PLAIN}" not in items
    assert items["codex-acp/gpt-6-luna"]["auto_reason"] == "not_selected"
    assert items["codex-acp/gpt-5.6-terra"]["auto_eligible"] is True
    # Claude has an independent explicit fixture list.
    assert items["claude-agent-acp/claude-sonnet-5"]["auto_eligible"] is True
    assert items["claude-agent-acp/claude-sonnet-4-6"]["auto_reason"] == "not_selected"
    choice = auto_route.choose("worker-backend-dev", project, environ)
    assert {c["route"] for c in choice.decision["candidates"]} == {
        "codex-acp/gpt-5.6-terra", "claude-agent-acp/claude-sonnet-5", f"opencode/{OPENCODE_VARIANT}"}
    assert choice.model == "gpt-5.6-terra" and choice.decision["decided_by"] == "affinity"
    assert choice.decision["rank_version"] == 11
    # An exact hint may now name a route whose id holds '/' and ','.
    hinted = auto_route.choose("worker-backend-dev", project, environ, model_hint=f"opencode/{OPENCODE_VARIANT}",
                               model_reason="The owner selected it.")
    assert (hinted.model, hinted.effort, hinted.decision["model_hint_status"]) == (OPENCODE_VARIANT, None, "honoured")
    # A pin still reaches a model the list leaves out.
    project, environ = _configured(monkeypatch, tmp_path, dict(routing, pin={"*": f"opencode/{OPENCODE_PLAIN}"}),
                                   [_opencode(OPENCODE_PLAIN), _opencode(OPENCODE_VARIANT)])
    with pytest.raises(PortableError, match="policy_missing"):
        auto_route.choose("worker-backend-dev", project, environ)
    # A manual-only identity becomes pinnable for automatic dispatch only
    # after the owner supplies sufficient tier policy.
    routing["models"]["opencode"]["tiers"][OPENCODE_PLAIN] = "standard"
    project, environ = _configured(monkeypatch, tmp_path, dict(routing, pin={"*": f"opencode/{OPENCODE_PLAIN}"}),
                                   [_opencode(OPENCODE_PLAIN), _opencode(OPENCODE_VARIANT)])
    pinned = auto_route.choose("worker-backend-dev", project, environ)
    assert (pinned.model, pinned.basis) == (OPENCODE_PLAIN, "auto:pin")


def test_an_empty_selection_routes_no_model_of_that_engine(tmp_path, monkeypatch):
    project, environ = _configured(monkeypatch, tmp_path, {"models": {"codex-acp": {"auto": []}}})
    choice = auto_route.choose("worker-technical-writer", project, environ)
    assert choice.model == "claude-sonnet-5" and choice.decision["tier"] == "standard"
    assert all(c["route"].startswith("claude-agent-acp/") for c in choice.decision["candidates"])


def test_a_listed_engine_routes_a_selected_older_version(tmp_path, monkeypatch):
    project, environ = _configured(monkeypatch, tmp_path, {"models": {"codex-acp": {"auto": ["gpt-5.6-luna"]}}})
    choice = auto_route.choose("worker-backend-dev", project, environ, complexity="low")
    assert choice.model == "gpt-5.6-luna"
    assert not any(route.startswith("codex-acp/") for route in choice.decision["superseded"])
    old = _menu_items(auto_route.route_menu(project, environ))["codex-acp/gpt-5.6-luna"]
    assert (old["auto_eligible"], old["superseded_by"]) == (True, None)


def test_an_explicit_auto_list_selects_exact_ids_but_deny_still_wins(tmp_path, monkeypatch):
    haiku = _all_efforts([_named("claude", "claude-haiku-4-5-20251001", "Haiku 4.5")])
    selection = {"claude-agent-acp": {"auto": ["claude-haiku-4-5-20251001", "claude-sonnet-5"]}}
    project, environ = _configured(monkeypatch, tmp_path, {"models": selection}, haiku)
    assert auto_route.choose("worker-technical-writer", project, environ).model == "claude-haiku-4-5-20251001"
    project, environ = _configured(monkeypatch, tmp_path, {"models": selection, "deny": ["claude-haiku-4-5-20251001"]}, haiku)
    assert auto_route.choose("worker-technical-writer", project, environ).model == "gpt-6-luna"


def test_a_configured_stretch_policy_moves_the_rule_to_another_light_model(tmp_path, monkeypatch):
    stretch = {"effort_floor": "high", "effort_ladder": ["high", "xhigh", "max"], "also_standard_for": ["testing"]}
    cleared = {"effort_floor": None, "effort_ladder": None, "also_standard_for": None}
    routing = {"models": {"codex-acp": {"policy": {"gpt-5.5": stretch, "gpt-6-luna": cleared}}}}
    project, environ = _configured(monkeypatch, tmp_path, routing, _all_efforts([_named("codex", "gpt-5.5")]))
    menu = auto_route.route_menu(project, environ)
    # The superseded older Luna keeps its built-in rule but is not eligible.
    stretch = {item["route"]: item for item in menu["stretch"]}
    assert {route: item["auto_eligible"] for route, item in stretch.items()} == {
        "codex-acp/gpt-5.6-luna": False, "codex-acp/gpt-5.5": True}
    assert stretch["codex-acp/gpt-5.5"]["efforts"] == dict(light="high", standard="xhigh", standard_escalated="max")
    items = _menu_items(menu)
    assert items["codex-acp/gpt-6-luna"]["efforts"] == ["low", "medium", "high", "xhigh", "max"]
    assert items["codex-acp/gpt-6-luna"]["policy"]["attribute_sources"] == {}
    assert items["codex-acp/gpt-5.5"]["policy"]["attribute_sources"]["effort_ladder"] == "configured"
    # The cleared Luna runs light work like any light model ...
    writer = auto_route.choose("worker-technical-writer", project, environ)
    assert (writer.model, writer.effort, writer.decision["effort_basis"]) == ("gpt-6-luna", "low", "role_tier:light")
    # ... and only the configured stretch model joins standard testing work.
    tester = auto_route.choose("worker-qa-engineer", project, environ)
    routes = {c["route"] for c in tester.decision["candidates"]}
    assert "codex-acp/gpt-5.5" in routes and "codex-acp/gpt-6-luna" not in routes
    assert tester.decision["stretch"]["status"] == "standard_candidate"
    hinted = auto_route.choose("worker-qa-engineer", project, environ, model_hint="codex-acp/gpt-5.5",
                               model_reason="Cheap and strong enough.")
    assert (hinted.model, hinted.effort, hinted.decision["effort_basis"]) == ("gpt-5.5", "xhigh", "stretch:standard")
    assert auto_route.hint_serves_tier("gpt-5.5", "light", "worker-qa-engineer", "standard",
                                       also_standard_for=["testing"])
    assert not auto_route.hint_serves_tier("gpt-6-luna", "light", "worker-qa-engineer", "standard",
                                           also_standard_for=[])


def test_the_flagship_effort_ceiling_follows_the_tier_and_the_policy(tmp_path, monkeypatch):
    routing = {"models": {
        "claude-agent-acp": {"tiers": {"claude-sonnet-5": "flagship"}},
        "codex-acp": {"tiers": {"gpt-6-astra": "deep"}, "policy": {"gpt-6-astra": {"max_effort": None}}}}}
    project, environ = _configured(monkeypatch, tmp_path, routing)
    items = _menu_items(auto_route.route_menu(project, environ))
    sonnet, astra = items["claude-agent-acp/claude-sonnet-5"], items["codex-acp/gpt-6-astra"]
    assert (sonnet["tier"], sonnet["tier_source"], sonnet["efforts"]) == ("flagship", "configured",
                                                                          ["low", "medium", "high"])
    assert (astra["tier"], astra["efforts"]) == ("deep", ["low", "medium", "high", "xhigh"])
    project, environ = _configured(monkeypatch, tmp_path, {"models": {
        "claude-agent-acp": {"tiers": {"claude-sonnet-5": "flagship"},
                             "policy": {"claude-sonnet-5": {"max_effort": "xhigh"}}}}})
    items = _menu_items(auto_route.route_menu(project, environ))
    assert items["claude-agent-acp/claude-sonnet-5"]["efforts"] == ["low", "medium", "high", "xhigh"]


def test_the_owners_preferred_model_for_a_kind_of_work_ranks_first(tmp_path, monkeypatch):
    project, environ = _configured(monkeypatch, tmp_path)
    assert auto_route.choose("worker-security-auditor", project, environ).model == "opus[1m]"
    routing = {"models": {"codex-acp": {"policy": {"gpt-6-sol": {"preferred_for": ["review", "design"]}}}}}
    project, environ = _configured(monkeypatch, tmp_path, routing)
    choice = auto_route.choose("worker-security-auditor", project, environ)
    assert (choice.model, choice.decision["decided_by"]) == ("gpt-6-sol", "purpose")
    assert choice.decision["purpose"] == dict(work_kind="review", preferred=["codex-acp/gpt-6-sol"], status="honoured")
    assert auto_route.route_menu(project, environ)["preferred"] == {
        "design": ["codex-acp/gpt-6-sol"], "review": ["codex-acp/gpt-6-sol"]}
    # It never moves a tier, and a meaningful plan-usage gap still wins.
    light = auto_route.choose("worker-explorer", project, environ)
    assert light.decision["tier"] == "light"
    project, environ = _configured(monkeypatch, tmp_path, routing, codex=95.0, claude=10.0)
    busy = auto_route.choose("worker-security-auditor", project, environ)
    assert (busy.model, busy.decision["decided_by"]) == ("opus[1m]", "quota")
    assert busy.decision["purpose"]["status"] == "ignored:near_quota_limit"


def test_a_conditional_preference_applies_only_where_the_coordinator_names_it(tmp_path, monkeypatch):
    """``preferred_when`` is owner text the runtime cannot judge: the model
    ranks as preferred only on a node whose hint names its exact route."""
    when = "frontend changes without design work"
    routing = {"models": {"codex-acp": {"policy": {"gpt-6-sol": {"preferred_for": ["review"],
                                                                  "preferred_when": when}}}}}
    project, environ = _configured(monkeypatch, tmp_path, routing)
    menu = auto_route.route_menu(project, environ)
    assert menu["preferred"] == {}
    assert menu["preferred_when"] == {"codex-acp/gpt-6-sol": dict(when=when, work_kinds=["review"])}
    plain = auto_route.choose("worker-security-auditor", project, environ)
    assert plain.model == "opus[1m]"
    assert plain.decision["purpose"] == dict(work_kind="review", preferred=[], status="none")
    met = auto_route.choose("worker-security-auditor", project, environ, model_hint="codex-acp/gpt-6-sol",
                            model_reason=f"Meets the owner's condition: {when}.")
    assert (met.model, met.decision["purpose"]["status"]) == ("gpt-6-sol", "honoured")
    assert met.decision["purpose"]["preferred"] == ["codex-acp/gpt-6-sol"]
    # The kinds still narrow the condition, and retries ignore it.
    project, environ = _configured(monkeypatch, tmp_path, {"models": {"codex-acp": {"policy": {"gpt-6-sol": {
        "preferred_for": ["design"], "preferred_when": when}}}}})
    other = auto_route.choose("worker-security-auditor", project, environ, model_hint="codex-acp/gpt-6-sol")
    assert other.decision["purpose"]["status"] == "none"
    project, environ = _configured(monkeypatch, tmp_path, {"models": {"codex-acp": {"policy": {"gpt-6-sol": {
        "preferred_when": when}}}}})
    retry = auto_route.choose("worker-security-auditor", project, environ, model_hint="codex-acp/gpt-6-sol",
                              escalate=True)
    assert retry.decision["purpose"]["preferred"] == []


def test_a_conditional_preference_outranks_an_unconditional_one_where_it_is_named(tmp_path, monkeypatch):
    routing = {"models": {
        "claude-agent-acp": {"policy": {"opus[1m]": {"preferred_for": ["review"]}}},
        "codex-acp": {"policy": {"gpt-6-sol": {"preferred_for": ["review"], "preferred_when": "Frontend only."}}}}}
    project, environ = _configured(monkeypatch, tmp_path, routing)
    assert auto_route.choose("worker-security-auditor", project, environ).model == "opus[1m]"
    met = auto_route.choose("worker-security-auditor", project, environ, model_hint="codex-acp/gpt-6-sol")
    assert (met.model, met.decision["model_hint_status"]) == ("gpt-6-sol", "honoured")


def test_route_menu_only_advertises_eligible_preferences_and_affinity_source(tmp_path, monkeypatch):
    routing = {"engine_affinity": {"review": "codex-acp"}, "deny": ["haiku"],
               "models": {"claude-agent-acp": {"auto": ["haiku", "opus[1m]", "claude-sonnet-5"], "policy": {"haiku": {"preferred_for": ["review"]}}},
                          "codex-acp": {"policy": {"gpt-6-sol": {"preferred_for": ["review"]}}}}}
    project, environ = _configured(monkeypatch, tmp_path, routing)
    menu = auto_route.route_menu(project, environ)
    assert menu["preferred"] == {"review": ["codex-acp/gpt-6-sol"]}
    assert menu["affinity_source"]["review"] == "routing.json"
    assert menu["affinity_source"]["design"] == "builtin"


@pytest.mark.parametrize("models,needle", [
    ([], "must map an engine id"),
    ({"gemini-acp": {}}, "engine must be one of"),
    ({"codex-acp": {"auto": "gpt-6-luna"}}, "exact model ids"),
    ({"codex-acp": {"auto": ["a b"]}}, "without whitespace"),
    ({"codex-acp": {"auto": ["x", "x"]}}, "listed twice"),
    ({"codex-acp": {"select": []}}, "use one of auto, tiers, policy"),
    ({"codex-acp": {"tiers": {"x": "huge"}}}, "tier must be one of"),
    ({"codex-acp": {"policy": {"x": {"effort_floor": "ultra"}}}}, "must be null or one of"),
    ({"codex-acp": {"policy": {"x": {"effort_floor": "max", "max_effort": "high"}}}}, "not be above max_effort"),
    ({"codex-acp": {"policy": {"x": {"effort_ladder": ["xhigh", "high"]}}}}, "strictly ascending"),
    ({"codex-acp": {"policy": {"x": {"also_standard_for": ["coding"]}}}}, "unique work kinds"),
    ({"codex-acp": {"policy": {"x": {"preferred_for": ["review", "review"]}}}}, "unique work kinds"),
    ({"codex-acp": {"policy": {"x": {"speed": 1}}}}, "use one of effort_floor"),
    ({"codex-acp": {"policy": {"x": {"preferred_when": "   "}}}}, "non-blank text"),
    ({"codex-acp": {"policy": {"x": {"preferred_when": "x" * 501}}}}, "at most 500 characters"),
    ({"codex-acp": {"policy": {"x": {"preferred_when": "a\x00b"}}}}, "without control characters"),
    ({"codex-acp": {"policy": {"x": {"preferred_when": ["frontend"]}}}}, "non-blank text"),
    ({"codex-acp": {"policy": {"x": {"request_names": ["Astra"]}}}}, "lowercase word"),
    ({"codex-acp": {"policy": {"x": {"request_names": ["use"]}}}}, "ordinary request"),
    ({"codex-acp": {"policy": {"x": {"request_names": ["используй"]}}}}, "ordinary request"),
    ({"codex-acp": {"policy": {"x": {"request_names": ["instead"]}}}}, "ordinary request"),
])
def test_models_are_validated(tmp_path, models, needle):
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    (project / ".cortex" / "routing.json").write_text(json.dumps({"models": models}))
    with pytest.raises(PortableError) as refused:
        auto_route.load_overrides(project, {"HOME": str(tmp_path)})
    assert refused.value.code == "invalid_routing_overrides" and needle in refused.value.remediation


def test_request_names_are_only_owner_data(tmp_path):
    exact = "codex-acp/gpt-6-astra"
    assert auto_route.requests_family("используй астру", exact) is None
    assert auto_route.requests_family("use Astra", exact) is None
    assert auto_route.requests_family("используй звезду", exact, ["звезд"])
    assert auto_route.requests_family("use codex-acp/gpt-6-astra", exact)
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    environ = {"HOME": str(tmp_path)}
    file = project / ".cortex" / "routing.json"
    file.write_text(json.dumps(routing_fixture({"models": {"codex-acp": {"policy": {
        "gpt-6-astra": {"request_names": ["звезд"]}}}}})))
    assert auto_route.request_names(exact, project, environ) == ("звезд",)
    file.write_text("{broken")
    with pytest.raises(PortableError, match="invalid_routing_overrides"):
        auto_route.request_names(exact, project, environ)


def test_a_model_in_several_tiers_is_a_candidate_of_each(tmp_path, monkeypatch):
    routing = {"providers": ["codex-acp"], "models": {"codex-acp": {
        "auto": ["gpt-5.6-terra"], "tiers": {"gpt-5.6-terra": ["light", "deep"]}}}}
    project, environ = _configured(monkeypatch, tmp_path, routing)
    menu = auto_route.route_menu(project, environ)
    listed = {tier for tier, group in menu["tiers"].items() for item in group if item["route"] == "codex-acp/gpt-5.6-terra"}
    assert listed == {"light", "deep"}
    terra = next(item for item in menu["tiers"]["deep"] if item["route"] == "codex-acp/gpt-5.6-terra")
    assert (terra["tiers"], terra["tier_source"]) == (["deep", "light"], "configured")
    light = auto_route.choose("worker-explorer", project, environ)
    deep = auto_route.choose("worker-architect", project, environ)
    assert (light.model, light.decision["tier"], light.effort) == ("gpt-5.6-terra", "light", "low")
    assert (deep.model, deep.decision["tier"], deep.effort) == ("gpt-5.6-terra", "deep", "high")
    # Standard work has no standard candidate: the nearest tier (deep) serves it.
    standard = auto_route.choose("worker-backend-dev", project, environ)
    assert (standard.model, standard.decision["tier"]) == ("gpt-5.6-terra", "deep")
    assert auto_route.allowed_efforts("codex-acp", "gpt-5.6-terra", project, environ)


# --- an owner configuration that prefers an OpenCode model --------------------

# The owner's own shape: Codex disabled, Claude and OpenCode allowed, one
# OpenCode model enabled, selected for automatic routing, placed in the light
# and standard tiers and preferred for most kinds of work.
OPENCODE_PREFERRED = {
    "deny": ["codex-acp/*"],
    "providers": ["claude-agent-acp", "opencode"],
    "enabled": {"opencode": [OPENCODE_PLAIN]},
    "models": {"opencode": {
        "auto": [OPENCODE_PLAIN],
        "tiers": {OPENCODE_PLAIN: ["light", "standard"]},
        "policy": {OPENCODE_PLAIN: {"preferred_for": [
            "implementation", "debugging", "testing", "investigation", "analysis", "writing"]}}}},
}
NEBULA = f"opencode/{OPENCODE_PLAIN}"


def _opencode_preferred(monkeypatch, tmp_path, routing=None, claude=None, opencode=None):
    project, environ = _configured(monkeypatch, tmp_path, routing or OPENCODE_PREFERRED,
                                  [_opencode(OPENCODE_PLAIN)], claude=claude)
    monkeypatch.setattr(quota, "used_percent", lambda record, *_, **_kwargs: (
        opencode if record.launch_command == ("opencode",) else
        claude if record.launch_command == ("claude",) else None))
    return project, environ


@pytest.mark.parametrize("role", ["worker-backend-dev", "worker-debugger", "worker-qa-engineer"])
def test_an_engine_hint_never_outranks_the_owners_preferred_model(tmp_path, monkeypatch, role):
    """Regression: the coordinator was told to hint the built-in affinity
    engine, and an honoured hint ranked above ``preferred_for``, so standard
    work the owner prefers on OpenCode ran on Claude's standard model."""
    project, environ = _opencode_preferred(monkeypatch, tmp_path)
    plain = auto_route.choose(role, project, environ)
    assert (_route(plain), plain.decision["tier"]) == (NEBULA, "standard")
    hinted = auto_route.choose(role, project, environ, model_hint="claude-agent-acp",
                               model_reason="Affinity engine for this kind of work.")
    assert _route(hinted) == NEBULA
    assert hinted.decision["decided_by"] == "purpose"
    assert hinted.decision["model_hint_status"] == "ignored:owner_preferred"
    assert hinted.decision["purpose"]["status"] == "honoured"
    exact = auto_route.choose(role, project, environ, model_hint="claude-agent-acp/claude-sonnet-5",
                              model_reason="Stronger prose.")
    assert (_route(exact), exact.decision["model_hint_status"]) == (NEBULA, "ignored:owner_preferred")


def test_a_hint_still_decides_work_the_owner_named_no_model_for(tmp_path, monkeypatch):
    """``preferred_for`` is a rank key, not a grant: review is not listed,
    so an honoured hint still decides among the tier's candidates."""
    project, environ = _opencode_preferred(monkeypatch, tmp_path)
    choice = auto_route.choose("worker-security-auditor", project, environ, complexity="low",
                               model_hint="claude-agent-acp", model_reason="Review reads prose.")
    assert choice.decision["purpose"] == dict(work_kind="review", preferred=[], status="none")
    assert (choice.model, choice.decision["model_hint_status"]) == ("claude-sonnet-5", "honoured")


def test_legacy_provider_preference_retains_priority_and_reports_the_winning_hint(tmp_path, monkeypatch):
    project, environ = _opencode_preferred(monkeypatch, tmp_path)
    choice = auto_route.choose("worker-backend-dev", project, environ,
                               preference="claude-agent-acp", model_hint="claude-agent-acp")
    assert _route(choice) == "claude-agent-acp/claude-sonnet-5"
    assert choice.decision["decided_by"] == "preference"
    assert choice.decision["model_hint_status"] == "honoured"


def test_a_retry_stays_on_the_owners_preferred_model(tmp_path, monkeypatch):
    """Retry diversification is a heuristic; the owner's preference wins it."""
    routing = json.loads(json.dumps(OPENCODE_PREFERRED))
    routing["models"]["opencode"]["tiers"][OPENCODE_PLAIN] = ["light", "standard", "deep"]
    project, environ = _opencode_preferred(monkeypatch, tmp_path, routing)
    retry = auto_route.choose("worker-backend-dev", project, environ, escalate=True, previous_engine="opencode")
    assert (_route(retry), retry.decision["tier"], retry.decision["decided_by"]) == (NEBULA, "deep", "purpose")


def test_deep_work_leaves_a_model_the_owner_placed_only_in_lower_tiers(tmp_path, monkeypatch):
    """By design: ``preferred_for`` never moves a tier. A deep role, or
    standard work the plan marks ``complexity: high``, runs on a deep model;
    the owner puts the OpenCode model in the deep tier to keep it there."""
    project, environ = _opencode_preferred(monkeypatch, tmp_path)
    high = auto_route.choose("worker-backend-dev", project, environ, complexity="high")
    assert (_route(high), high.decision["tier"]) == ("claude-agent-acp/opus[1m]", "deep")
    routing = json.loads(json.dumps(OPENCODE_PREFERRED))
    routing["models"]["opencode"]["tiers"][OPENCODE_PLAIN] = ["light", "standard", "deep"]
    project, environ = _opencode_preferred(monkeypatch, tmp_path, routing)
    high = auto_route.choose("worker-backend-dev", project, environ, complexity="high")
    assert (_route(high), high.decision["tier"], high.decision["decided_by"]) == (NEBULA, "deep", "purpose")


def test_route_menu_withholds_affinity_engines_the_owner_turned_off(tmp_path, monkeypatch):
    project, environ = _opencode_preferred(monkeypatch, tmp_path)
    menu = auto_route.route_menu(project, environ)
    assert menu["affinity"]["implementation"] is None and menu["affinity"]["operations"] is None
    assert menu["affinity_not_eligible"] == {"implementation": "codex-acp", "operations": "codex-acp"}
    assert menu["affinity"]["review"] == "claude-agent-acp"
    assert menu["preferred"]["implementation"] == [NEBULA]


@pytest.mark.parametrize("opencode,claude,expected,status", [
    (60, 10, NEBULA, "honoured"),  # non-hot usage gap does not override the owner
    (86, 80, NEBULA, "honoured"),  # crossing the hot band alone is insufficient
    (95, 10, "claude-agent-acp/claude-sonnet-5", "ignored:near_quota_limit"),
    (100, 10, "claude-agent-acp/claude-sonnet-5", "none"),
    (None, 10, NEBULA, "honoured"),
])
def test_owner_preference_keeps_quota_safety(tmp_path, monkeypatch, opencode, claude, expected, status):
    project, environ = _opencode_preferred(monkeypatch, tmp_path, opencode=opencode, claude=claude)
    choice = auto_route.choose("worker-backend-dev", project, environ)
    assert _route(choice) == expected
    assert choice.decision["purpose"]["status"] == status


@pytest.mark.parametrize("constraint", ["deny", "providers", "enabled", "builtin", "unavailable", "pin"])
def test_owner_preference_does_not_grant_a_route(tmp_path, monkeypatch, constraint):
    routing = json.loads(json.dumps(OPENCODE_PREFERRED))
    unavailable = ()
    if constraint == "deny":
        routing["deny"].append(NEBULA)
    elif constraint == "providers":
        routing["providers"] = ["claude-agent-acp"]
    elif constraint == "enabled":
        routing["enabled"]["opencode"] = []
    elif constraint == "builtin":
        del routing["models"]["opencode"]["auto"]  # preferred_for does not opt in
    elif constraint == "unavailable":
        unavailable = ({"connection_id": "opencode", "engine": "opencode", "model": OPENCODE_PLAIN,
                        "reason": "engine_start_failed", "expires_at": "later"},)
    else:
        routing["pin"] = {"*": "claude-agent-acp/claude-sonnet-5"}
    project, environ = _opencode_preferred(monkeypatch, tmp_path, routing)
    choice = auto_route.choose("worker-backend-dev", project, environ, unavailable=unavailable)
    assert _route(choice) == "claude-agent-acp/claude-sonnet-5"


def test_profile_activation_materializes_preferences_but_does_not_override_project_or_env(tmp_path, monkeypatch):
    from cortex_runtime.configuration import management
    from cortex_runtime.connections import registry

    project, environ = _opencode_preferred(monkeypatch, tmp_path)
    user_root = registry.config_root(environ)
    user_root.mkdir(parents=True, exist_ok=True)
    project_path = project / ".cortex" / "routing.json"
    project_path.rename(user_root / "routing.json")

    def profiles(action, **kwargs):
        view = management.execute({"surface": "routing_profiles", "action": "read"}, environ=environ)
        assert view["ok"], view
        view = view["result"]
        request = dict(surface="routing_profiles", action=action, name="opencode-first",
                       expected_revision=view["revision"], expected_revisions=view["current_revisions"], **kwargs)
        response = management.execute(request, environ=environ)
        assert response["ok"], response
        return response["result"]

    profiles("create")
    changed = management.update_routing("user", management._revision((user_root / "routing.json").read_bytes()),
                                        routing_fixture({"providers": ["claude-agent-acp"]}), environ=environ)
    assert changed["profile"]["state"] == "modified"
    assert auto_route.choose("worker-backend-dev", project, environ).connection_id == "claude"
    profiles("activate", discard_modifications=True)
    assert auto_route.load_overrides(project, environ) == routing_fixture(OPENCODE_PREFERRED)
    choice = auto_route.choose("worker-backend-dev", project, environ, model_hint="claude-agent-acp")
    assert _route(choice) == NEBULA
    assert auto_route._effective_profile_label(project, environ) == {"name": "opencode-first", "state": "in_sync"}

    env_path = tmp_path / "override.json"
    env_path.write_text(json.dumps(routing_fixture({"providers": ["claude-agent-acp"]})))
    environ["CORTEX_ROUTING"] = str(env_path)
    assert auto_route.choose("worker-backend-dev", project, environ).connection_id == "claude"
    assert auto_route._effective_profile_label(project, environ) is None
    project_path.write_text("{}")  # even an empty project file wins wholesale
    with pytest.raises(PortableError, match="model_policy_migration_required"):
        auto_route.load_overrides(project, environ)
    assert auto_route._effective_profile_label(project, environ) is None
    assert json.loads((user_root / "routing.json").read_text()) == routing_fixture(OPENCODE_PREFERRED)


def _route(choice):
    engine = "opencode" if choice.connection_id == "opencode" else (
        "codex-acp" if choice.connection_id == "codex" else "claude-agent-acp")
    return f"{engine}/{choice.model}"


@pytest.mark.parametrize("connection,engine", [("codex", "codex-acp"), ("claude", "claude-agent-acp")])
@pytest.mark.parametrize("role,escalate,pin,effort,status", [
    ("worker-explorer", False, False, "high", "light_tier"),
    ("worker-qa-engineer", False, False, "xhigh", "standard_candidate"),
    ("worker-qa-engineer", True, False, "max", "standard_candidate"),
    ("worker-explorer", False, True, "high", "pinned"),
])
def test_arbitrary_configured_model_has_neutral_stretch_audit(
        tmp_path, monkeypatch, connection, engine, role, escalate, pin, effort, status):
    model = "owner-defined-fast-42"
    routing = {
        "enabled": {engine: [model]},
        "models": {engine: {"auto": [model], "tiers": {model: "light"},
                           "policy": {model: {"effort_floor": "high",
                               "effort_ladder": ["high", "xhigh", "max"],
                               "also_standard_for": ["testing"]}}}},
    }
    if pin:
        routing["pin"] = {"*": engine + "/" + model + ":low"}
    project, environ = _live_choose(
        monkeypatch, tmp_path, _all_efforts([_named(connection, model)]), routing)
    choice = auto_route.choose(role, project, environ, escalate=escalate)
    assert (choice.model, choice.effort) == (model, effort)
    assert choice.decision["stretch"]["status"] == status
    assert choice.decision["stretch"]["chosen"] is True
    assert "luna" not in choice.decision
    if not pin:
        tier = "light" if role == "worker-explorer" else "standard"
        assert choice.decision["effort_basis"] == "stretch:" + tier + (":escalated" if escalate else "")
