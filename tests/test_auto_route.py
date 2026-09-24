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


def test_light_work_prefers_the_account_above_the_threshold():
    assert _pick("light", codex=90, claude=40) == "gpt-6-luna"
    assert _pick("light", codex=40, claude=90) == "haiku"


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
    assert auto_route.permitted_efforts(_entry("codex", "luna", ALL), "light") == ("low", "medium", "high", "xhigh", "max")


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
    assert "Haiku" in unavailable.value.remediation


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
        assert _rank("light", "worker-explorer", entries=entries)[0].route.model == "gpt-6-luna"
        assert _rank("light", "worker-technical-writer", entries=entries)[0].route.model == "haiku"


def test_every_plannable_role_has_a_known_work_kind():
    from cortex_runtime.portable.roles import WORKER_ALIASES

    assert set(auto_route.ROLE_WORK_KINDS) == set(WORKER_ALIASES.values())
    assert set(auto_route.ROLE_WORK_KINDS.values()) <= set(auto_route.WORK_KIND_ENGINES)


def test_quota_still_outranks_the_role():
    assert _rank("standard", "worker-backend-dev", codex=90, claude=40)[0].route.model == "sonnet"
    assert _rank("light", "worker-technical-writer", codex=90, claude=40)[0].route.engine == "codex-acp"
    assert _rank("standard", "worker-backend-dev", codex=100, claude=40)[0].route.model == "sonnet"


def test_a_preference_is_honoured_among_the_tiers_candidates():
    picked, decision = _rank("deep", "worker-security-auditor", preference="codex-acp")
    assert picked.route.model == "gpt-6-sol"
    assert decision["tier"] == "deep" and decision["preference_status"] == "honoured"
    assert decision["candidates"][0] == dict(route="codex-acp/gpt-6-sol", connection_id="codex", used_percent=None,
                                             hot=False, quota_state="unknown", balance_excess=0, suitability=0, preferred=True)
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
    picked, decision = _rank("standard", "worker-backend-dev", codex=30, claude=10)
    assert picked.route.model == "gpt-5.6-sol"
    assert decision["preference"] is None and decision["preference_status"] == "none"
    assert [c["suitability"] for c in decision["candidates"]] == [1, 1, 0]


def test_balance_overrides_role_suitability_only_beyond_tolerance():
    recent = {"standard": {"codex-acp": 5, "claude-agent-acp": 0}}
    picked, decision = auto_route.rank(
        "standard", ENTRIES, _usage(), role="worker-backend-dev", recent=recent,
    )
    assert picked.route.engine == "claude-agent-acp"
    assert decision["rank_version"] == 2
    assert decision["balance"]["counts"] == recent["standard"]
    assert next(c for c in decision["candidates"] if c["route"] == "codex-acp/gpt-5.6-sol")["balance_excess"] == 1

    recent["standard"]["codex-acp"] = 4
    picked, decision = auto_route.rank(
        "standard", ENTRIES, _usage(), role="worker-backend-dev", recent=recent,
    )
    assert picked.route.engine == "codex-acp"
    assert all(c["balance_excess"] == 0 for c in decision["candidates"])


def test_unknown_quota_never_wins_the_percentage_tiebreak():
    # Same suitability (role omitted) and balance. The list order decides
    # when either quota is unknown, regardless of the known percentage.
    entries = [CODEX[4], CLAUDE[1]]
    picked, decision = auto_route.rank("standard", entries, _usage(codex=50, claude=None))
    assert picked is entries[0]
    assert decision["used_percent_compared"] is False
    assert [c["quota_state"] for c in decision["candidates"]] == ["known", "unknown"]
    picked, decision = auto_route.rank("standard", entries, _usage(codex=50, claude=20))
    assert picked is entries[1] and decision["used_percent_compared"] is True


def test_unknown_quota_is_neutral_in_light_hot_band():
    entries = [CLAUDE[2], CODEX[2]]
    picked, decision = auto_route.rank("light", entries, _usage(codex=None, claude=None))
    assert picked is entries[0] and decision["used_percent_compared"] is False
    picked, _ = auto_route.rank("light", entries, _usage(codex=90, claude=None))
    assert picked is entries[1]


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
    entry = _entry("codex", "gpt-5.6-luna")
    monkeypatch.setattr(registry, "read_registry", lambda _: SimpleNamespace(
        connections={"codex": SimpleNamespace(launch_command=("codex",))}))
    monkeypatch.setattr(auto_route, "_entries", lambda *_: [entry])
    monkeypatch.setattr(auto_route, "_is_seed", lambda _: False)
    monkeypatch.setattr(quota, "used_percent", lambda *_: None)
    choice = auto_route.choose("worker-explorer", project, environ, escalate=True)
    assert choice.effort == "medium"
    assert choice.decision["tier"] == "light"
    assert choice.decision["effort_basis"] == "escalation_step"


def test_quota_and_unavailability_still_dominate_balance():
    recent = {"deep": {"codex-acp": 0, "claude-agent-acp": 20}}
    picked, decision = auto_route.rank(
        "deep", ENTRIES, _usage(codex=90, claude=40),
        role="worker-backend-dev", recent=recent,
    )
    assert picked.route.engine == "claude-agent-acp"
    assert decision["candidates"][0]["balance_excess"] == 16
    blocked = frozenset(e.route.id for e in CLAUDE)
    picked, decision = auto_route.rank(
        "deep", ENTRIES, _usage(codex=30, claude=40),
        excluded=blocked, role="worker-security-auditor", recent=recent,
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


@pytest.mark.parametrize("hint,excluded,policy,usage,recent,expected", [
    ("unknown-acp/model", frozenset(), frozenset(), _usage(20, 20), None, "ignored:not_offered"),
    ("claude-agent-acp/sonnet", frozenset({"claude:sonnet"}), frozenset(),
     _usage(20, 20), None, "ignored:unavailable"),
    ("claude-agent-acp/sonnet", frozenset({"claude:sonnet"}),
     frozenset({"claude:sonnet"}), _usage(20, 20), None, "ignored:auto_excluded"),
    ("codex-acp/gpt-5.6-terra", frozenset(), frozenset(), _usage(90, 20), None,
     "ignored:near_quota_limit"),
    ("codex-acp/gpt-5.6-terra", frozenset(), frozenset(), _usage(20, 20),
     {"standard": {"codex-acp": 20, "claude-agent-acp": 0}}, "ignored:engine_overused"),
])
def test_model_hint_rejection_reasons(hint, excluded, policy, usage, recent, expected):
    entries = [CODEX[4], CLAUDE[1]]
    _, decision = auto_route.rank(
        "standard", entries, usage, excluded=excluded, policy_excluded=policy,
        role="worker-backend-dev", model_hint=hint, recent=recent)
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
    choice = auto_route.choose("worker-explorer", project, environ)
    assert choice.model == "gpt-6-luna"
    assert choice.decision["superseded"]["codex-acp/gpt-5.6-luna"] == "codex-acp/gpt-6-luna"
    assert "codex-acp/gpt-5.6-luna" not in choice.decision["excluded_by_policy"]
    assert all(c["route"] != "codex-acp/gpt-5.6-luna" for c in choice.decision["candidates"])
    deep = auto_route.choose("worker-security-auditor", project, environ)
    assert {c["route"] for c in deep.decision["candidates"]} == {
        "codex-acp/gpt-6-sol", "claude-agent-acp/opus[1m]"}
    # Deny removes the newest version: the newest non-denied one is routed.
    project, environ = _live_choose(monkeypatch, tmp_path, entries, {"deny": ["gpt-6-luna"]})
    assert auto_route.choose("worker-explorer", project, environ).model == "gpt-5.6-luna"


def test_an_explicit_pin_of_an_older_version_is_honoured(tmp_path, monkeypatch):
    project, environ = _live_choose(monkeypatch, tmp_path, LIVE_CODEX + LIVE_CLAUDE,
                                    {"pin": {"worker-explorer": "codex-acp/gpt-5.6-luna"}})
    choice = auto_route.choose("worker-explorer", project, environ)
    assert (choice.model, choice.basis) == ("gpt-5.6-luna", "auto:pin")


def test_a_model_hint_for_a_superseded_version_is_ignored(tmp_path, monkeypatch):
    project, environ = _live_choose(monkeypatch, tmp_path, LIVE_CODEX + LIVE_CLAUDE)
    choice = auto_route.choose("worker-explorer", project, environ, model_hint="codex-acp/gpt-5.6-luna")
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
