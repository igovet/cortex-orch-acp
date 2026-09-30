"""routing.auto_route selection and connections.quota parsing."""
from __future__ import annotations

import json
import time
from types import SimpleNamespace

import pytest

from cortex_runtime.connections import quota
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.routing import auto_route


def _entry(connection, model, efforts=("low", "medium", "high")):
    route = SimpleNamespace(
        id=f"{connection}:{model}", engine="codex-acp" if connection.startswith("codex") else "claude-agent-acp",
        model=model, supported_parameters=tuple(("thought_level", e) for e in efforts),
    )
    return SimpleNamespace(connection_id=connection, route=route)


# The real catalog orders: codex-acp lists its newest generation first,
# claude-agent-acp its current models before older generations.
CODEX = [_entry("codex", m) for m in ("gpt-6-astra", "gpt-6-sol", "gpt-6-luna", "gpt-5.6-sol", "gpt-5.6-terra",
                                      "gpt-5.6-luna", "gpt-5.5")]
CLAUDE = [_entry("claude", m) for m in ("claude-opus-5[1m]", "sonnet", "haiku", "claude-opus-4-6")]
ENTRIES = CODEX + CLAUDE
# Models neither table names: placed by position.
UNKNOWN = [_entry("codex", m) for m in ("alpha", "beta", "gamma", "delta", "epsilon")]


def _usage(codex=None, claude=None):
    return lambda entry: codex if entry.connection_id == "codex" else claude


def _pick(tier, codex=None, claude=None, overrides=None):
    return auto_route.pick(tier, ENTRIES, _usage(codex, claude), overrides).route.model


def test_known_models_have_named_tiers_and_only_unknown_ones_are_positional():
    tiers = auto_route.model_tiers(ENTRIES + UNKNOWN, {})
    assert [tiers[e.route.id] for e in CODEX] == ["flagship", "deep", "light", "standard", "standard", "light", "light"]
    # Claude lists older generations last; its family names decide instead.
    assert [tiers[e.route.id] for e in CLAUDE] == ["deep", "standard", "light", "deep"]
    assert [tiers[e.route.id] for e in UNKNOWN] == ["deep", "deep", "standard", "standard", "light"]


def test_each_ordinary_tier_has_a_candidate_from_both_providers():
    tiers = auto_route.model_tiers(ENTRIES, {})
    for tier in ("deep", "standard", "light"):
        engines = {e.route.engine for e in ENTRIES if tiers[e.route.id] == tier}
        assert engines == {"codex-acp", "claude-agent-acp"}, tier
    assert {e.route.model for e in ENTRIES if tiers[e.route.id] == "flagship"} == {"gpt-6-astra"}


def test_tier_override_moves_one_model():
    assert auto_route.model_tiers(ENTRIES, {"gpt-6-luna": "standard"})["codex:gpt-6-luna"] == "standard"


def test_heavy_work_avoids_an_account_above_the_threshold():
    assert _pick("deep", codex=90, claude=40) == "claude-opus-5[1m]"
    assert _pick("deep", codex=40, claude=90) == "gpt-6-sol"
    assert _pick("standard", codex=90, claude=40) == "sonnet"


def test_light_work_also_avoids_the_account_above_the_threshold():
    """Light work no longer spends a hot plan first: every tier prefers the
    plan with more room (the rank never sees when a window resets)."""
    assert _pick("light", codex=90, claude=40) == "haiku"
    assert _pick("light", codex=40, claude=90) == "gpt-6-luna"


def test_both_hot_still_picks_the_one_with_more_left():
    assert _pick("deep", codex=95, claude=88) == "claude-opus-5[1m]"


def test_an_exhausted_account_is_never_chosen():
    assert _pick("light", codex=100, claude=40) == "haiku"
    assert auto_route.pick("deep", CODEX, _usage(codex=100)) is None


def test_an_unavailable_route_is_skipped_across_providers():
    """Every Codex route recorded unavailable: the work crosses to Claude."""
    codex = frozenset(e.route.id for e in CODEX)
    assert auto_route.pick("light", ENTRIES, _usage(codex=90, claude=40), excluded=codex).route.model == "haiku"
    assert auto_route.pick("deep", ENTRIES, _usage(codex=10, claude=40), excluded=codex).route.model == "claude-opus-5[1m]"


def test_excluding_a_route_never_moves_another_models_tier():
    """Unknown models' tiers come from list position; skipping alpha must not make epsilon standard."""
    picked = auto_route.pick("light", UNKNOWN, _usage(codex=10), excluded=frozenset({"codex:alpha", "codex:epsilon"}))
    assert picked.route.model == "gamma"


def test_nothing_usable_is_no_pick():
    assert auto_route.pick("deep", ENTRIES, _usage(), excluded=frozenset(e.route.id for e in ENTRIES)) is None


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
    assert auto_route.pick("light", only_flagship, _usage()) is None
    assert auto_route.pick("flagship", only_flagship, _usage()).route.model == "gpt-6-astra"


ALL = ("low", "medium", "high", "xhigh", "max", "ultra")


def test_ultra_is_never_permitted_and_max_only_on_a_light_model():
    assert auto_route.permitted_efforts(_entry("codex", "terra", ALL), "standard") == ("low", "medium", "high", "xhigh")
    assert auto_route.permitted_efforts(_entry("codex", "alpha", ALL), "light") == ("low", "medium", "high", "xhigh", "max")
    # Luna: never below high, and max even when a tiers override moves it.
    assert auto_route.permitted_efforts(_entry("codex", "gpt-6-luna", ALL), "light") == ("high", "xhigh", "max")
    assert auto_route.permitted_efforts(_entry("codex", "gpt-6-luna", ALL), "standard") == ("high", "xhigh", "max")
    # A Luna that offers nothing at or above the floor keeps what it offers.
    assert auto_route.permitted_efforts(_entry("codex", "gpt-6-luna", ("low", "medium")), "light") == ("low", "medium")


def test_flagship_models_never_go_above_high():
    assert auto_route.permitted_efforts(_entry("codex", "gpt-6-astra", ALL), "deep") == ("low", "medium", "high")
    assert auto_route.permitted_efforts(_entry("claude", "claude-fable-5-1[1m]", ALL), "deep") == ("low", "medium", "high")


def test_nearest_permitted_effort():
    assert auto_route.nearest("high", ("low", "medium")) == "medium"
    assert auto_route.nearest("low", ("high", "xhigh")) == "high"
    assert auto_route.nearest("ultra", ("low", "medium", "high", "xhigh")) == "xhigh"
    assert auto_route.nearest("high", ()) is None


def test_overrides_are_validated_and_located_by_precedence(tmp_path):
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    environ = {"HOME": str(tmp_path)}
    assert auto_route.load_overrides(project, environ) == {}
    (project / ".cortex" / "routing.json").write_text(json.dumps({"deny": ["gpt-5.5"]}))
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
    (project / ".cortex" / "routing.json").write_text(json.dumps({"providers": ["claude-agent-acp"]}))
    assert auto_route.load_overrides(project, environ) == {"providers": ["claude-agent-acp"]}
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


def test_default_route_skips_haiku_and_an_explicit_pin_can_restore_it(tmp_path, monkeypatch):
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
    monkeypatch.setattr(quota, "used_percent", lambda *_args: None)

    automatic = auto_route.choose("worker-technical-writer", project, environ)
    assert automatic.model == "gpt-5.6-luna"
    assert automatic.basis == "auto"

    (project / ".cortex" / "routing.json").write_text(
        json.dumps({"pin": {"worker-technical-writer": "claude-agent-acp/haiku"}})
    )
    pinned = auto_route.choose("worker-technical-writer", project, environ)
    assert pinned.model == "haiku"
    assert pinned.basis == "auto:pin"

    (project / ".cortex" / "routing.json").unlink()
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
    assert quota.used_percent(record, "opus[1m]", environ) == pytest.approx(90.0)
    assert quota.used_percent(record, "sonnet", environ) == pytest.approx(30.0)
    quota.observe(connection_id, {"status": "rejected", "rateLimitType": "five_hour", "resetsAt": future}, environ)
    assert quota.used_percent(record, "sonnet", environ) == 100.0


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
    return auto_route.rank(tier, entries, _usage(codex, claude), excluded=excluded, role=role, preference=preference)


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
    assert _rank("standard", "worker-backend-dev", codex=90, claude=40)[0].route.model == "sonnet"
    assert _rank("light", "worker-backend-dev", codex=90, claude=40)[0].route.engine == "claude-agent-acp"
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
    picked, decision = _rank("deep", "worker-backend-dev", preference="codex-acp", codex=90, claude=40)
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
    picked, decision = _rank("standard", "worker-backend-dev", codex=30, claude=10)
    assert picked.route.model == "sonnet" and decision["decided_by"] == "usage"
    assert decision["rank_version"] == 8
    assert decision["usage"] == dict(compared=True, margin=10.0, least_used_percent=10)
    assert {c["route"]: c["usage_band"] for c in decision["candidates"]} == {
        "claude-agent-acp/sonnet": 0, "codex-acp/gpt-5.6-sol": 2, "codex-acp/gpt-5.6-terra": 2}
    assert "balance" not in decision
    assert all("balance_excess" not in c for c in decision["candidates"])
    # 9.9 points is inside the margin: affinity decides.
    picked, decision = _rank("standard", "worker-backend-dev", codex=19.9, claude=10,
                             entries=[CODEX[4], CLAUDE[1]])
    assert picked.route.engine == "codex-acp" and decision["decided_by"] == "affinity"
    # The same holds the other way round for review work.
    picked, decision = _rank("deep", "worker-security-auditor", codex=5, claude=40)
    assert picked.route.engine == "codex-acp" and decision["decided_by"] == "usage"


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
    picked, decision = auto_route.rank("standard", entries, _usage(codex=50, claude=None))
    assert picked is entries[0]
    assert decision["used_percent_compared"] is False
    assert [c["quota_state"] for c in decision["candidates"]] == ["known", "unknown"]
    picked, decision = auto_route.rank("standard", entries, _usage(codex=50, claude=20))
    assert picked is entries[1] and decision["used_percent_compared"] is True


def test_unknown_quota_is_not_hot_and_never_compared():
    entries = [CLAUDE[2], CODEX[2]]
    picked, decision = auto_route.rank("light", entries, _usage(codex=None, claude=None))
    assert picked is entries[0] and decision["used_percent_compared"] is False
    assert decision["usage"] == dict(compared=False, margin=10.0, least_used_percent=None)
    picked, decision = auto_route.rank("light", entries, _usage(codex=90, claude=None))
    assert picked is entries[0] and decision["decided_by"] == "quota"
    # Below the hot band, a known low usage never beats an unknown one.
    picked, decision = auto_route.rank("light", [CODEX[2], CLAUDE[2]], _usage(codex=60, claude=None),
                                       role="worker-explorer")
    assert picked is CLAUDE[2] and decision["decided_by"] == "affinity"


def test_policy_exclusion_is_distinct_from_unavailable():
    entries = [CLAUDE[2], CODEX[2]]
    picked, decision = auto_route.rank(
        "light", entries, _usage(), excluded=frozenset({CLAUDE[2].route.id}),
        policy_excluded=frozenset({CLAUDE[2].route.id}),
    )
    assert picked is CODEX[2]
    assert decision["excluded_by_policy"] == ["claude-agent-acp/haiku"]
    assert decision["unavailable"] == []


def test_auto_allow_restores_haiku_but_deny_still_wins(tmp_path, monkeypatch):
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
    monkeypatch.setattr(quota, "used_percent", lambda *_: None)
    route_file = project / ".cortex" / "routing.json"
    route_file.write_text(json.dumps({"auto_allow": ["claude-agent-acp/haiku"]}))
    assert auto_route.choose("worker-technical-writer", project, environ).model == "haiku"
    route_file.write_text(json.dumps({"auto_allow": ["claude-agent-acp/haiku"], "deny": ["haiku"]}))
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
    monkeypatch.setattr(quota, "used_percent", lambda *_: None)
    choice = auto_route.choose("worker-explorer", project, environ, escalate=True)
    assert choice.effort == "medium"
    assert choice.decision["tier"] == "light"
    assert choice.decision["effort_basis"] == "escalation_step"


def test_quota_and_unavailability_dominate_affinity():
    picked, decision = auto_route.rank(
        "deep", ENTRIES, _usage(codex=90, claude=40), role="worker-backend-dev",
    )
    assert picked.route.engine == "claude-agent-acp"
    # The runner-up is the older Opus on the same account, not Codex.
    assert decision["decided_by"] == "engine_order"
    blocked = frozenset(e.route.id for e in CLAUDE)
    picked, decision = auto_route.rank(
        "deep", ENTRIES, _usage(codex=30, claude=40),
        excluded=blocked, role="worker-security-auditor",
    )
    assert picked.route.engine == "codex-acp"
    assert "claude-agent-acp/claude-opus-5[1m]" in decision["unavailable"]



def test_model_hint_honour_and_tier_guard():
    entries = [CODEX[4], CLAUDE[1], CODEX[0]]
    _, decision = auto_route.rank(
        "standard", entries, _usage(codex=20, claude=20),
        role="worker-backend-dev", model_hint="claude-agent-acp/sonnet",
        model_reason="The work requires prose analysis.")
    assert (decision["chosen"], decision["model_hint_status"]) == ("claude-agent-acp/sonnet", "honoured")
    assert decision["model_hint_reason"] == "The work requires prose analysis."
    _, decision = auto_route.rank(
        "standard", entries, _usage(codex=20, claude=20),
        role="worker-backend-dev", model_hint="codex-acp/gpt-6-astra")
    assert decision["tier"] == "standard"
    assert decision["model_hint_status"] == "ignored:tier_mismatch"


@pytest.mark.parametrize("hint,excluded,policy,usage,expected", [
    ("unknown-acp/model", frozenset(), frozenset(), _usage(20, 20), "ignored:not_offered"),
    ("claude-agent-acp/sonnet", frozenset({"claude:sonnet"}), frozenset(),
     _usage(20, 20), "ignored:unavailable"),
    ("claude-agent-acp/sonnet", frozenset({"claude:sonnet"}),
     frozenset({"claude:sonnet"}), _usage(20, 20), "ignored:auto_excluded"),
    ("codex-acp/gpt-5.6-terra", frozenset(), frozenset(), _usage(90, 20),
     "ignored:near_quota_limit"),
])
def test_model_hint_rejection_reasons(hint, excluded, policy, usage, expected):
    entries = [CODEX[4], CLAUDE[1]]
    _, decision = auto_route.rank(
        "standard", entries, usage, excluded=excluded, policy_excluded=policy,
        role="worker-backend-dev", model_hint=hint)
    assert decision["model_hint_status"] == expected


def test_escalation_ignores_hint_and_prefers_another_engine():
    entries = [CODEX[4], CLAUDE[1]]
    _, decision = auto_route.rank(
        "standard", entries, _usage(20, 20), role="worker-backend-dev",
        model_hint="codex-acp/gpt-5.6-terra", model_reason="Legacy preference.",
        escalated=True, previous_engine="codex-acp")
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
    monkeypatch.setattr(quota, "used_percent", lambda *_: None)
    first = auto_route.route_menu(project, environ)
    assert {item["route"] for item in first["tiers"]["light"]} == {
        "claude-agent-acp/haiku", "codex-acp/gpt-5.6-luna"}
    assert next(item for item in first["tiers"]["light"] if item["model"] == "haiku")["auto_eligible"] is False
    (project / ".cortex" / "routing.json").write_text(json.dumps({
        "providers": ["claude-agent-acp"], "auto_allow": ["haiku"]}))
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
def test_model_line_parses_family_and_version(model, name, expected):
    assert auto_route.model_line(model, name) == expected


def test_only_the_newest_version_of_each_family_is_routable():
    superseded = auto_route.superseded_routes(LIVE_CODEX + LIVE_CLAUDE)
    assert superseded == {
        "codex:gpt-5.6-sol": "codex-acp/gpt-6-sol",
        "codex:gpt-5.6-luna": "codex-acp/gpt-6-luna",
        "claude:claude-opus-5[1m]": "claude-agent-acp/opus[1m]",
        "claude:claude-opus-4-8": "claude-agent-acp/opus[1m]",
        "claude:claude-opus-4-7": "claude-agent-acp/opus[1m]",
        "claude:claude-fable-5": "claude-agent-acp/claude-fable-5-1[1m]",
        "claude:claude-sonnet-4-6": "claude-agent-acp/claude-sonnet-5",
    }


def test_a_new_release_supersedes_its_family_automatically():
    codex = LIVE_CODEX + [_named("codex", "gpt-7-luna", "7 Luna")]
    claude = LIVE_CLAUDE + [_named("claude", "claude-sonnet-5-1", "Sonnet 5.1")]
    superseded = auto_route.superseded_routes(codex + claude)
    assert superseded["codex:gpt-6-luna"] == "codex-acp/gpt-7-luna"
    assert superseded["codex:gpt-5.6-luna"] == "codex-acp/gpt-7-luna"
    assert superseded["claude:claude-sonnet-5"] == "claude-agent-acp/claude-sonnet-5-1"
    assert "codex:gpt-7-luna" not in superseded and "claude:claude-sonnet-5-1" not in superseded
    # A release no tier table names keeps its line's tier.
    assert auto_route.model_tiers(codex, {})["codex:gpt-7-luna"] == "light"


def test_families_are_per_engine_and_unversioned_aliases_are_never_superseded():
    entries = [_named("claude", "sonnet", "sonnet"), _named("claude", "claude-sonnet-4-6", "Sonnet 4.6"),
               _named("codex", "gpt-5.6-terra")]
    assert auto_route.superseded_routes(entries) == {}


def _live_choose(monkeypatch, tmp_path, entries, routing=None):
    from cortex_runtime.connections import registry

    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True, exist_ok=True)
    environ = {"HOME": str(tmp_path)}
    if routing is not None:
        (project / ".cortex" / "routing.json").write_text(json.dumps(routing))
    connections = {"claude": SimpleNamespace(launch_command=("claude",)),
                   "codex": SimpleNamespace(launch_command=("codex",))}
    monkeypatch.setattr(registry, "read_registry", lambda _: SimpleNamespace(connections=connections))
    monkeypatch.setattr(auto_route, "_entries", lambda overrides, *_: [
        e for e in entries if not auto_route._denied(overrides, e.route.engine, e.route.model)])
    monkeypatch.setattr(auto_route, "_is_seed", lambda _: False)
    monkeypatch.setattr(quota, "used_percent", lambda *_: None)
    return project, environ


def test_choose_routes_only_the_newest_non_denied_version(tmp_path, monkeypatch):
    entries = LIVE_CODEX + LIVE_CLAUDE
    project, environ = _live_choose(monkeypatch, tmp_path, entries)
    # Light implementation work leans to Codex, so the newest Luna is chosen.
    choice = auto_route.choose("worker-backend-dev", project, environ, complexity="low")
    assert choice.model == "gpt-6-luna"
    assert choice.decision["superseded"]["codex-acp/gpt-5.6-luna"] == "codex-acp/gpt-6-luna"
    assert "codex-acp/gpt-5.6-luna" not in choice.decision["excluded_by_policy"]
    assert all(c["route"] != "codex-acp/gpt-5.6-luna" for c in choice.decision["candidates"])
    deep = auto_route.choose("worker-security-auditor", project, environ)
    assert {c["route"] for c in deep.decision["candidates"]} == {
        "codex-acp/gpt-6-sol", "claude-agent-acp/opus[1m]"}
    # Deny removes the newest version: the newest non-denied one is routed.
    project, environ = _live_choose(monkeypatch, tmp_path, entries, {"deny": ["gpt-6-luna"]})
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
    assert choice.decision["model_hint_status"] == "ignored:superseded"


def test_route_menu_reports_superseded_versions(tmp_path, monkeypatch):
    project, environ = _live_choose(monkeypatch, tmp_path, LIVE_CODEX + LIVE_CLAUDE)
    menu = auto_route.route_menu(project, environ)
    items = {item["route"]: item for group in menu["tiers"].values() for item in group}
    old = items["claude-agent-acp/claude-opus-4-8"]
    assert (old["auto_eligible"], old["superseded_by"]) == (False, "claude-agent-acp/opus[1m]")
    new = items["claude-agent-acp/opus[1m]"]
    assert (new["auto_eligible"], new["superseded_by"]) == (True, None)


@pytest.mark.parametrize("model", ["haiku", "claude-haiku-4-5", "claude-haiku-4-5-20251001", "claude-haiku-5[1m]"])
def test_a_bare_haiku_deny_covers_every_haiku_id(model):
    assert auto_route._denied({"deny": ["haiku"]}, "claude-agent-acp", model)
    assert auto_route._denied({"deny": ["claude-agent-acp/haiku"]}, "claude-agent-acp", model)
    assert not auto_route._denied({"deny": ["codex-acp/haiku"]}, "claude-agent-acp", model)


def test_deny_tokens_never_match_another_family_or_version():
    deny = {"deny": ["gpt-5.5", "haiku"]}
    assert auto_route._denied(deny, "codex-acp", "gpt-5.5")
    assert auto_route._denied(deny, "codex-acp", "gpt-5.5[1m]")
    for model in ("gpt-5.6-luna", "gpt-6-luna", "gpt-5.5-codex", "gpt-6-sol"):
        assert not auto_route._denied(deny, "codex-acp", model), model
    for model in ("opus[1m]", "claude-sonnet-5", "claude-fable-5-1[1m]", "sonnet"):
        assert not auto_route._denied(deny, "claude-agent-acp", model), model
    # A token with a variant is exact: it never widens to the whole family.
    assert not auto_route._denied({"deny": ["opus[1m]"]}, "claude-agent-acp", "claude-opus-5[1m]")


def test_the_default_deny_removes_haiku_from_automatic_routing_and_the_selector(tmp_path, monkeypatch):
    entries = [_named("claude", "haiku", "Haiku 4.5"), _named("claude", "claude-haiku-4-5-20251001"),
               _named("codex", "gpt-5.5")] + LIVE_CODEX
    project, environ = _live_choose(monkeypatch, tmp_path, entries, {"deny": ["gpt-5.5", "haiku"]})
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
    picked, decision = auto_route.rank("standard", entries, _usage(), role="worker-backend-dev")
    assert picked is CODEX[4] and decision["affinity"] == dict(
        work_kind="implementation", engine="codex-acp", source="default")
    picked, decision = auto_route.rank("standard", entries, _usage(), role="worker-backend-dev",
                                       affinity={"implementation": "claude-agent-acp"})
    assert picked is CLAUDE[1] and decision["decided_by"] == "affinity"
    assert decision["affinity"]["source"] == "routing.json"


def test_engine_affinity_validation(tmp_path):
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    environ = {"HOME": str(tmp_path)}
    route_file = project / ".cortex" / "routing.json"
    valid = {"engine_affinity": {"implementation": "claude-agent-acp", "debugging": None, "*": "codex-acp"}}
    route_file.write_text(json.dumps(valid))
    assert auto_route.load_overrides(project, environ) == valid
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
                                           policy_excluded=NO_HAIKU, role=role)
        assert picked.route.engine == "codex-acp" and decision["tier"] == "light", role
        assert {c["route"] for c in decision["candidates"]} == {
            "codex-acp/gpt-6-luna", "codex-acp/gpt-5.6-luna", "codex-acp/gpt-5.5"}
        assert "cross_tier" not in decision and "chosen_cross_tier" not in decision
        assert all("cross_tier" not in c for c in decision["candidates"])
    # Even a Claude plan with far more room does not pull light work up to Sonnet.
    picked, decision = auto_route.rank("light", ENTRIES, _usage(codex=80, claude=0), excluded=NO_HAIKU,
                                       policy_excluded=NO_HAIKU, role="worker-technical-writer")
    assert picked.route.model == "gpt-6-luna" and decision["tier"] == "light"


def test_no_task_is_promoted_above_its_tier_while_the_tier_has_a_usable_model():
    tiers = auto_route.model_tiers(ENTRIES, {})
    # Standard work with only Codex in standard never gets Claude's deep model.
    live = [e for e in LIVE_CODEX + LIVE_CLAUDE if "sonnet" not in e.route.model]
    superseded = auto_route.superseded_routes(live)
    live_tiers = auto_route.model_tiers(live, {})
    _, decision = auto_route.rank("standard", live, _usage(), excluded=frozenset(superseded),
                                  policy_excluded=frozenset(superseded), superseded=superseded,
                                  role="worker-code-reviewer")
    assert decision["tier"] == "standard"
    assert {live_tiers[c["connection_id"] + ":" + c["route"].split("/", 1)[1]] for c in decision["candidates"]} == {
        "standard"}
    # A Claude hint or preference cannot lift light work to a stronger Claude model.
    for kwargs in (dict(model_hint="claude-agent-acp"), dict(model_hint="claude-agent-acp/sonnet"),
                   dict(preference="claude-agent-acp")):
        picked, decision = auto_route.rank("light", ENTRIES, _usage(), excluded=NO_HAIKU,
                                           policy_excluded=NO_HAIKU, role="worker-technical-writer", **kwargs)
        assert tiers[picked.route.id] == "light" and picked.route.engine == "codex-acp", kwargs
    # Only when no engine has a usable model in the tier does it fall back.
    no_light = frozenset(e.route.id for e in ENTRIES if tiers[e.route.id] == "light")
    picked, decision = auto_route.rank("light", ENTRIES, _usage(), excluded=no_light, role="worker-technical-writer")
    assert decision["tier"] == "standard" and decision["role_tier"] == "light"


def test_light_work_runs_on_the_light_model_at_the_light_effort(tmp_path, monkeypatch):
    project, environ = _live_choose(monkeypatch, tmp_path, LIVE_CODEX + LIVE_CLAUDE)
    choice = auto_route.choose("worker-technical-writer", project, environ)
    # Luna never runs below high (its light-work effort).
    assert (choice.model, choice.effort) == ("gpt-6-luna", "high")
    assert choice.decision["effort_basis"] == "luna:light"
    assert choice.decision["tier"] == "light" and choice.decision["decided_by"] == "only_candidate"
    # Allowing Haiku is the owner's lever for Claude on light work.
    haiku = _named("claude", "claude-haiku-4-5-20251001", "Haiku 4.5")
    project, environ = _live_choose(monkeypatch, tmp_path, LIVE_CODEX + LIVE_CLAUDE + [haiku],
                                    {"auto_allow": ["haiku"]})
    choice = auto_route.choose("worker-technical-writer", project, environ)
    assert choice.model == "claude-haiku-4-5-20251001" and choice.decision["decided_by"] == "affinity"


def test_hint_is_honoured_unless_the_hinted_plan_is_near_its_quota_band():
    assert auto_route.HINT_TOLERANCE == auto_route.HOT_PERCENT == 85.0
    entries = [CODEX[4], CLAUDE[1]]
    for codex, claude, status, chosen in (
            (84, 10, "honoured", "codex-acp/gpt-5.6-terra"),
            (85, 10, "ignored:near_quota_limit", "claude-agent-acp/sonnet"),
            # Every plan near its limit: nothing better to protect.
            (95, 90, "honoured", "codex-acp/gpt-5.6-terra")):
        _, decision = auto_route.rank("standard", entries, _usage(codex, claude),
                                      role="worker-code-reviewer", model_hint="codex-acp")
        assert (decision["model_hint_status"], decision["chosen"]) == (status, chosen), (codex, claude)
    # An honoured hint outranks plan usage.
    _, decision = auto_route.rank("standard", entries, _usage(84, 10),
                                  role="worker-code-reviewer", model_hint="codex-acp")
    assert decision["decided_by"] == "hint"



def test_a_preference_near_its_quota_band_is_ignored_in_every_tier():
    picked, decision = _rank("light", "worker-backend-dev", preference="codex-acp", codex=90, claude=10)
    assert picked.route.engine == "claude-agent-acp"
    assert decision["preference_status"] == "ignored:provider_near_quota_limit"


def test_every_tier_orders_a_hot_account_last():
    """Light work used to spend a hot plan first (7b7aca22); a large usage
    gap (the usage-gap signal, not the quota band) is what decides it now
    that quota is only a late tie breaker."""
    assert auto_route._order_key(10.0) < auto_route._order_key(90.0)
    assert auto_route._order_key(None) == auto_route._order_key(0.0)
    entries = [CODEX[2], CLAUDE[2]]  # gpt-6-luna, Claude haiku (both light)
    picked, decision = auto_route.rank("light", entries, _usage(codex=10, claude=90), role="worker-explorer")
    assert picked is CODEX[2] and decision["decided_by"] == "usage"
    picked, decision = auto_route.rank("light", entries, _usage(codex=90, claude=10), role="worker-backend-dev")
    assert picked is CLAUDE[2] and decision["decided_by"] == "usage"


# --- softer 85% quota band (rank version 6) --------------------------------

def test_crossing_85_percent_alone_no_longer_overrides_affinity():
    """A one-point crossing of HOT_PERCENT is not a meaningful usage gap
    (well under USAGE_MARGIN), so engine affinity -- the task/model fit --
    still decides, even though the affinity-preferred candidate is the
    (barely) hot one."""
    entries = [CODEX[4], CLAUDE[1]]  # gpt-5.6-terra, sonnet (both standard)
    # Codex is preferred for implementation work; a 2-point gap is far
    # inside USAGE_MARGIN, so it is not a meaningful usage difference.
    picked, decision = auto_route.rank("standard", entries, _usage(codex=86, claude=84),
                                       role="worker-backend-dev")
    assert picked.route.engine == "codex-acp" and decision["decided_by"] == "affinity"
    assert decision["candidates"][0]["hot"] is True


def test_a_small_near_boundary_gap_falls_to_the_quota_tie_breaker():
    """With no preference, hint, diversify or affinity difference, a small
    (sub-USAGE_MARGIN) gap that straddles HOT_PERCENT is still decided by
    the quota band -- it remains a meaningful, if late, signal."""
    entries = [CODEX[4], CLAUDE[1]]
    picked, decision = auto_route.rank("standard", entries, _usage(codex=86, claude=84))
    assert picked.route.engine == "claude-agent-acp" and decision["decided_by"] == "quota"
    picked, decision = auto_route.rank("standard", entries, _usage(codex=84, claude=86))
    assert picked.route.engine == "codex-acp" and decision["decided_by"] == "quota"


@pytest.mark.parametrize("codex,claude,expected_engine,expected_decided_by", [
    # 84% is not hot: the account below HOT_PERCENT is preferred at the
    # quota tie breaker even with only a 1-point gap (inside USAGE_MARGIN).
    (84.0, 85.0, "codex-acp", "quota"),
    # Exactly 85% is hot (HOT_PERCENT is inclusive).
    (85.0, 84.0, "claude-agent-acp", "quota"),
    (86.0, 84.0, "claude-agent-acp", "quota"),
])
def test_boundary_percentages_around_84_85_86(codex, claude, expected_engine, expected_decided_by):
    entries = [CODEX[4], CLAUDE[1]]
    picked, decision = auto_route.rank("standard", entries, _usage(codex=codex, claude=claude))
    assert picked.route.engine == expected_engine
    assert decision["decided_by"] == expected_decided_by


def test_a_large_gap_still_wins_via_the_usage_signal_regardless_of_the_band():
    """A gap well past USAGE_MARGIN decides through the usage-gap signal,
    not the quota band -- quota influence is retained, just moved earlier
    into the existing usage-band mechanism instead of a hard 85% cliff."""
    entries = [CODEX[4], CLAUDE[1]]
    picked, decision = auto_route.rank("standard", entries, _usage(codex=40, claude=20))
    assert picked.route.engine == "claude-agent-acp" and decision["decided_by"] == "usage"


def test_both_hot_candidates_are_still_separated_by_the_usage_gap_or_exact_percent():
    entries = [CODEX[4], CLAUDE[1]]
    # A meaningful (>= USAGE_MARGIN) gap between two hot plans: usage decides.
    picked, decision = auto_route.rank("standard", entries, _usage(codex=99, claude=87))
    assert picked.route.engine == "claude-agent-acp" and decision["decided_by"] == "usage"
    # An immaterial gap between two hot plans: exact used percent decides.
    picked, decision = auto_route.rank("standard", entries, _usage(codex=90, claude=86))
    assert picked.route.engine == "claude-agent-acp" and decision["decided_by"] == "used_percent"


def test_hint_ignored_only_when_hot_and_meaningfully_worse_than_an_alternative():
    entries = [CODEX[4], CLAUDE[1]]
    # Hinted route is hot but the alternative is barely better: honoured.
    _, decision = auto_route.rank("standard", entries, _usage(codex=90, claude=86),
                                  role="worker-backend-dev", model_hint="codex-acp")
    assert decision["model_hint_status"] == "honoured"
    # Hinted route is hot and meaningfully worse than the alternative: ignored.
    _, decision = auto_route.rank("standard", entries, _usage(codex=90, claude=70),
                                  role="worker-backend-dev", model_hint="codex-acp")
    assert decision["model_hint_status"] == "ignored:near_quota_limit"
    # Hinted route is not hot at all: honoured even with a large gap.
    _, decision = auto_route.rank("standard", entries, _usage(codex=84, claude=10),
                                  role="worker-backend-dev", model_hint="codex-acp")
    assert decision["model_hint_status"] == "honoured"
    # Hinted route's usage is unknown: quota stays neutral, honoured.
    _, decision = auto_route.rank("standard", entries, _usage(None, 10),
                                  role="worker-backend-dev", model_hint="codex-acp")
    assert decision["model_hint_status"] == "honoured"
    # The alternative's usage is unknown: nothing to compare against, honoured.
    _, decision = auto_route.rank("standard", entries, _usage(90, None),
                                  role="worker-backend-dev", model_hint="codex-acp")
    assert decision["model_hint_status"] == "honoured"


def test_preference_ignored_only_when_hot_and_meaningfully_worse_than_an_alternative():
    picked, decision = _rank("standard", "worker-backend-dev", preference="codex-acp", codex=90, claude=86)
    assert picked.route.engine == "codex-acp" and decision["preference_status"] == "honoured"
    picked, decision = _rank("standard", "worker-backend-dev", preference="codex-acp", codex=90, claude=70)
    assert picked.route.engine == "claude-agent-acp"
    assert decision["preference_status"] == "ignored:provider_near_quota_limit"


def test_exhausted_100_percent_remains_a_hard_exclusion_regardless_of_margin():
    """The 100% exclusion is unaffected by the softer 85% band: an
    exhausted route never enters the rank, hinted or not."""
    entries = [CODEX[4], CLAUDE[1]]
    _, decision = auto_route.rank("standard", entries, _usage(codex=100, claude=99),
                                  role="worker-backend-dev", model_hint="codex-acp")
    assert decision["chosen"] == "claude-agent-acp/sonnet"
    assert all(c["route"] != "codex-acp/gpt-5.6-terra" for c in decision["candidates"])
    assert decision["model_hint_status"] == "ignored:unavailable"


@pytest.mark.parametrize("kwargs,expected", [
    (dict(preference="claude-agent-acp"), "preference"),
    (dict(model_hint="claude-agent-acp/sonnet"), "hint"),
    (dict(usage=_usage(codex=90, claude=10)), "usage"),
    (dict(escalated=True, previous_engine="codex-acp"), "diversify"),
    (dict(usage=_usage(codex=40, claude=20)), "usage"),
    (dict(), "affinity"),
    (dict(usage=_usage(codex=25, claude=20)), "affinity"),
    (dict(role=None, usage=_usage(codex=25, claude=20)), "used_percent"),
    (dict(role=None), "engine_order"),
])
def test_decided_by_names_the_first_separating_key(kwargs, expected):
    entries = [CODEX[4], CLAUDE[1]]
    kwargs = dict(dict(role="worker-backend-dev", usage=_usage()), **kwargs)
    usage = kwargs.pop("usage")
    _, decision = auto_route.rank("standard", entries, usage, **kwargs)
    assert decision["decided_by"] == expected
    _, decision = auto_route.rank("standard", [CODEX[4]], usage, **kwargs)
    assert decision["decided_by"] == "only_candidate"


# --- the Codex Luna line ------------------------------------------------------

def _all_efforts(entries):
    for entry in entries:
        entry.route.supported_parameters = tuple(("thought_level", e) for e in ALL)
    return entries


def _live_all(monkeypatch, tmp_path, routing=None, codex=None, claude=None):
    entries = _all_efforts([_named("codex", e.route.model) for e in LIVE_CODEX]
                           + [_named("claude", e.route.model, e.model_name) for e in LIVE_CLAUDE])
    project, environ = _live_choose(monkeypatch, tmp_path, entries, routing)
    monkeypatch.setattr(quota, "used_percent", lambda record, *_: (
        codex if record.launch_command == ("codex",) else claude))
    return project, environ


def test_luna_effort_steps_with_the_work_and_never_goes_below_high():
    assert auto_route.LUNA_EFFORT_FLOOR == "high"
    assert auto_route.luna_effort("light") == "high"
    assert auto_route.luna_effort("light", escalate=True) == "xhigh"
    assert auto_route.luna_effort("standard") == "xhigh"
    assert auto_route.luna_effort("standard", escalate=True) == "max"
    assert auto_route.is_luna("gpt-6-luna") and auto_route.is_luna("gpt-7-luna")
    assert not auto_route.is_luna("gpt-5.6-terra")


def test_light_work_runs_luna_at_high(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path)
    for role, complexity in (("worker-technical-writer", "normal"), ("worker-backend-dev", "low"),
                             ("worker-explorer", "high")):
        choice = auto_route.choose(role, project, environ, complexity=complexity)
        assert (choice.model, choice.effort) == ("gpt-6-luna", "high"), role
        assert choice.decision["luna"]["status"] == "light_tier"
    # A pin to Luna at low is raised to the floor, not honoured below it.
    project, environ = _live_all(monkeypatch, tmp_path, {"pin": {"*": "codex-acp/gpt-6-luna:low"}})
    choice = auto_route.choose("worker-backend-dev", project, environ)
    assert (choice.model, choice.effort) == ("gpt-6-luna", "high")
    assert choice.decision["luna"] == dict(status="pinned", chosen=True)
    # A worker's requested effort is clamped the same way at dispatch.
    assert auto_route.allowed_efforts("codex-acp", "gpt-6-luna", project, environ) == ("high", "xhigh", "max")


def test_normal_testing_ranks_luna_with_the_standard_models(tmp_path, monkeypatch):
    assert auto_route.LUNA_WORK_KINDS == frozenset({"testing"})
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
    assert decision["effort_basis"] == "luna:standard"
    assert decision["luna"] == dict(status="standard_candidate", candidate=True, chosen=True, outranked_by=None)
    # The rank still decides: a hot Codex plan sends the work to Sonnet.
    project, environ = _live_all(monkeypatch, tmp_path, codex=90.0, claude=10.0)
    choice = auto_route.choose("worker-qa-engineer", project, environ)
    assert choice.model == "claude-sonnet-5" and choice.effort == "medium"
    assert choice.decision["luna"] == dict(status="standard_candidate", candidate=True, chosen=False,
                                           outranked_by="usage")
    # A coordinator hint picks either standard model; a Luna hint is not a tier mismatch.
    project, environ = _live_all(monkeypatch, tmp_path)
    choice = auto_route.choose("worker-qa-engineer", project, environ, model_hint="codex-acp/gpt-5.6-terra",
                               model_reason="Terra for this module.")
    assert choice.model == "gpt-5.6-terra" and choice.decision["luna"]["outranked_by"] == "hint"
    choice = auto_route.choose("worker-qa-engineer", project, environ, model_hint="codex-acp/gpt-6-luna",
                               model_reason="Cheap and strong enough.")
    assert choice.decision["model_hint_status"] == "honoured"
    assert (choice.model, choice.effort) == ("gpt-6-luna", "xhigh")


@pytest.mark.parametrize("complexity", ["low", "normal"])
def test_escalated_standard_testing_runs_luna_at_max(tmp_path, monkeypatch, complexity):
    project, environ = _live_all(monkeypatch, tmp_path, codex=20.0, claude=20.0,
                                 routing={"engine_affinity": {"testing": "codex-acp"}})
    choice = auto_route.choose("worker-qa-engineer", project, environ, complexity=complexity, escalate=True)
    assert choice.decision["base_role_tier"] == ("light" if complexity == "low" else "standard")
    assert choice.decision["role_tier"] == "standard"
    assert choice.decision["luna"]["candidate"] is True
    assert (choice.model, choice.effort) == ("gpt-6-luna", "max")
    assert choice.decision["effort_basis"] == "luna:standard:escalated"
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
def test_hard_deep_and_flagship_work_never_goes_to_luna(tmp_path, monkeypatch, role, kwargs, status):
    project, environ = _live_all(monkeypatch, tmp_path)
    choice = auto_route.choose(role, project, environ, **kwargs)
    assert not auto_route.is_luna(choice.model)
    assert choice.decision["luna"]["status"] == status
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
def test_standard_work_that_is_not_testing_is_not_a_luna_candidate(tmp_path, monkeypatch, role, kwargs):
    project, environ = _live_all(monkeypatch, tmp_path)
    for model_hint in ("auto", "codex-acp/gpt-6-luna"):
        choice = auto_route.choose(role, project, environ, model_hint=model_hint,
                                   model_reason=None if model_hint == "auto" else "Cheap.", **kwargs)
        assert choice.decision["tier"] == "standard"
        assert choice.decision["luna"]["status"].startswith("not_candidate:work_kind_")
        assert choice.decision["luna"]["candidate"] is False
        assert all("luna" not in c["route"] for c in choice.decision["candidates"])
        assert not auto_route.is_luna(choice.model)


def test_standard_development_retry_escalates_without_luna(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path)
    choice = auto_route.choose("worker-backend-dev", project, environ, escalate=True)
    assert choice.decision["base_role_tier"] == "standard"
    assert choice.decision["role_tier"] == choice.decision["tier"] == "deep"
    assert not auto_route.is_luna(choice.model)
    assert choice.decision["luna"]["status"] == "excluded:deep_work"


def test_route_menu_and_hint_preview_describe_the_luna_rule(tmp_path, monkeypatch):
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
    assert luna["policy"]["attribute_sources"] == dict(effort_floor="builtin", effort_ladder="builtin",
                                                       also_standard_for="builtin")
    assert (luna["tier_source"], luna["selection"], luna["auto_reason"]) == ("builtin", "builtin", "eligible")
    assert "luna" not in menu and "flagship_note" not in menu
    assert auto_route.hint_serves_tier("gpt-6-luna", "light", "worker-qa-engineer", "standard")
    assert not auto_route.hint_serves_tier("gpt-6-luna", "light", "worker-backend-dev", "standard")
    assert not auto_route.hint_serves_tier("gpt-6-luna", "light", "worker-debugger", "standard")
    assert not auto_route.hint_serves_tier("gpt-6-luna", "light", "worker-qa-engineer", "deep", "high")
    assert not auto_route.hint_serves_tier("gpt-6-luna", "light", "worker-ux-designer", "standard")
    assert not auto_route.hint_serves_tier("gpt-5.6-terra", "standard", "worker-backend-dev", "light", "low")


@pytest.mark.parametrize("previous_engine,codex,claude,reason", [
    ("codex-acp", 20.0, 20.0, "diversify"),
    ("claude-agent-acp", 90.0, 20.0, "usage"),
])
def test_standard_retry_can_choose_claude_over_luna(tmp_path, monkeypatch, previous_engine, codex, claude, reason):
    project, environ = _live_all(monkeypatch, tmp_path, codex=codex, claude=claude)
    choice = auto_route.choose("worker-qa-engineer", project, environ, escalate=True,
                               previous_engine=previous_engine, model_hint="codex-acp/gpt-6-luna")
    assert (choice.model, choice.effort) == ("claude-sonnet-5", "high")
    assert choice.decision["model_hint_status"] == "ignored:escalated"
    assert choice.decision["luna"] == dict(status="standard_candidate", candidate=True,
                                          chosen=False, outranked_by=reason)


def test_standard_retry_falls_back_when_no_standard_or_luna_route_is_usable(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path, {
        "deny": ["luna", "terra", "sonnet", "gpt-5.6-sol"]})
    choice = auto_route.choose("worker-qa-engineer", project, environ, escalate=True)
    assert choice.decision["base_role_tier"] == choice.decision["role_tier"] == "standard"
    assert choice.decision["tier"] == "deep"
    assert not auto_route.is_luna(choice.model)
    assert choice.effort == "high"


@pytest.mark.parametrize("suffix,effort", [("", "high"), (":medium", "medium")])
def test_standard_retry_preserves_non_luna_pin_effort(tmp_path, monkeypatch, suffix, effort):
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


@pytest.mark.parametrize("routing,codex,unavailable,status", [
    ({"deny": ["gpt-6-astra"]}, None, (), "refused:denied"),
    ({"deny": ["astra"]}, None, (), "refused:denied"),
    ({"providers": ["claude-agent-acp"]}, None, (), "refused:provider_not_allowed"),
    (None, 100.0, (), "refused:quota_exhausted"),
    (None, None, ({"connection_id": "codex", "model": "gpt-6-astra", "engine": "codex-acp",
                   "reason": "quota_exhausted", "expires_at": "later"},), "refused:unavailable"),
])
def test_policy_still_wins_over_a_user_requested_flagship(tmp_path, monkeypatch, routing, codex, unavailable, status):
    project, environ = _live_all(monkeypatch, tmp_path, routing, codex=codex)
    choice = auto_route.choose("worker-backend-dev", project, environ, model_hint=ASTRA,
                               user_request=USER_ASKED, unavailable=unavailable)
    assert choice.model != "gpt-6-astra" and choice.basis == "auto"
    assert choice.decision["role_tier"] == "standard"
    assert choice.decision["user_model_request"] == dict(USER_ASKED, status=status)


def test_a_pin_still_wins_over_a_user_requested_flagship(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path, {"pin": {"backend-dev": "codex-acp/gpt-6-sol"}})
    choice = auto_route.choose("worker-backend-dev", project, environ, model_hint=ASTRA, user_request=USER_ASKED)
    assert (choice.model, choice.basis) == ("gpt-6-sol", "auto:pin")
    assert choice.decision["user_model_request"] == dict(USER_ASKED, status="ignored:pinned")


def test_a_user_request_never_raises_a_non_flagship_hint_or_an_unhinted_node(tmp_path, monkeypatch):
    project, environ = _live_all(monkeypatch, tmp_path)
    sonnet = auto_route.choose("worker-backend-dev", project, environ, model_hint="claude-agent-acp/claude-sonnet-5",
                               user_request=dict(USER_ASKED, family="sonnet"))
    assert (sonnet.model, sonnet.basis, sonnet.decision["tier"]) == ("claude-sonnet-5", "auto", "standard")
    assert sonnet.decision["user_model_request"]["status"] == "not_flagship"
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
    assert menu["flagship"]["request_names"][ASTRA] == ["астр"]
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
    monkeypatch.setattr(quota, "used_percent", lambda record, *_: (
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
    opencode = items[f"opencode/{OPENCODE_VARIANT}"]
    assert (opencode["auto_eligible"], opencode["auto_reason"], opencode["selection"]) == (False, "excluded_by_default", "builtin")
    assert opencode["tier_source"] == "position" and opencode["efforts"] == []
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
    assert items[f"opencode/{OPENCODE_PLAIN}"]["auto_reason"] == "not_selected"
    assert items["codex-acp/gpt-6-luna"]["auto_reason"] == "not_selected"
    assert items["codex-acp/gpt-5.6-terra"]["auto_eligible"] is True
    # Claude has no list: its built-in rules are unchanged.
    assert items["claude-agent-acp/claude-sonnet-5"]["auto_eligible"] is True
    assert items["claude-agent-acp/claude-sonnet-4-6"]["auto_reason"] == "superseded"
    choice = auto_route.choose("worker-backend-dev", project, environ)
    assert {c["route"] for c in choice.decision["candidates"]} == {
        "codex-acp/gpt-5.6-terra", "claude-agent-acp/claude-sonnet-5", f"opencode/{OPENCODE_VARIANT}"}
    assert choice.model == "gpt-5.6-terra" and choice.decision["decided_by"] == "affinity"
    assert choice.decision["rank_version"] == 8
    # An exact hint may now name a route whose id holds '/' and ','.
    hinted = auto_route.choose("worker-backend-dev", project, environ, model_hint=f"opencode/{OPENCODE_VARIANT}",
                               model_reason="The owner selected it.")
    assert (hinted.model, hinted.effort, hinted.decision["model_hint_status"]) == (OPENCODE_VARIANT, None, "honoured")
    # A pin still reaches a model the list leaves out.
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
    assert (old["auto_eligible"], old["superseded_by"]) == (True, "codex-acp/gpt-6-luna")


def test_a_selection_list_overrides_the_haiku_default_but_deny_still_wins(tmp_path, monkeypatch):
    haiku = _all_efforts([_named("claude", "claude-haiku-4-5-20251001", "Haiku 4.5")])
    selection = {"claude-agent-acp": {"auto": ["claude-haiku-4-5-20251001", "claude-sonnet-5"]}}
    project, environ = _configured(monkeypatch, tmp_path, {"models": selection}, haiku)
    assert auto_route.choose("worker-technical-writer", project, environ).model == "claude-haiku-4-5-20251001"
    project, environ = _configured(monkeypatch, tmp_path, {"models": selection, "deny": ["haiku"]}, haiku)
    assert auto_route.choose("worker-technical-writer", project, environ).model == "gpt-6-luna"


def test_a_configured_stretch_policy_moves_the_luna_rule_to_another_light_model(tmp_path, monkeypatch):
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
    assert tester.decision["luna"]["status"] == "standard_candidate"
    hinted = auto_route.choose("worker-qa-engineer", project, environ, model_hint="codex-acp/gpt-5.5",
                               model_reason="Cheap and strong enough.")
    assert (hinted.model, hinted.effort, hinted.decision["effort_basis"]) == ("gpt-5.5", "xhigh", "luna:standard")
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
    assert choice.decision["purpose"] == dict(work_kind="review", preferred=["codex-acp/gpt-6-sol"])
    assert auto_route.route_menu(project, environ)["preferred"] == {
        "design": ["codex-acp/gpt-6-sol"], "review": ["codex-acp/gpt-6-sol"]}
    # It never moves a tier, and a meaningful plan-usage gap still wins.
    light = auto_route.choose("worker-explorer", project, environ)
    assert light.decision["tier"] == "light"
    project, environ = _configured(monkeypatch, tmp_path, routing, codex=90.0, claude=10.0)
    busy = auto_route.choose("worker-security-auditor", project, environ)
    assert (busy.model, busy.decision["decided_by"]) == ("opus[1m]", "usage")


def test_route_menu_only_advertises_eligible_preferences_and_affinity_source(tmp_path, monkeypatch):
    routing = {"engine_affinity": {"review": "codex-acp"},
               "models": {"claude-agent-acp": {"policy": {"haiku": {"preferred_for": ["review"]}}},
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


def test_configured_request_names_replace_the_builtin_spellings(tmp_path):
    astra = "codex-acp/gpt-6-astra"
    assert auto_route.requests_family("используй астру", "astra")
    assert auto_route.requests_family("используй астру", "astra", []) is None
    assert auto_route.requests_family("используй звезду", "astra", ["звезд"])
    assert auto_route.requests_family("please use the star", "astra", ["star"]) == "star"
    assert auto_route.requests_family("use Astra", "astra", []) == "Astra"
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    environ = {"HOME": str(tmp_path)}
    assert auto_route.request_names(astra, project, environ) == ("астр",)
    (project / ".cortex" / "routing.json").write_text(json.dumps({"models": {"codex-acp": {"policy": {
        "gpt-6-astra": {"request_names": ["звезд"]}}}}}))
    assert auto_route.request_names(astra, project, environ) == ("звезд",)
    (project / ".cortex" / "routing.json").write_text("{broken")
    assert auto_route.request_names(astra, project, environ) == ("астр",)


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
