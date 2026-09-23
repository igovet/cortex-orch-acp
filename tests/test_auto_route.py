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
                                             hot=False, suitability=0, preferred=True)
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
