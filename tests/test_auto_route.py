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


CODEX = [_entry("codex", m) for m in ("astra", "sol", "terra", "luna", "gpt-5.5")]
CLAUDE = [_entry("claude", m) for m in ("claude-opus-5[1m]", "sonnet", "haiku", "claude-opus-4-6")]
ENTRIES = CODEX + CLAUDE


def _usage(codex=None, claude=None):
    return lambda entry: codex if entry.connection_id == "codex" else claude


def _pick(tier, codex=None, claude=None, overrides=None):
    return auto_route.pick(tier, ENTRIES, _usage(codex, claude), overrides).route.model


def test_model_tiers_follow_each_engines_own_order():
    tiers = auto_route.model_tiers(ENTRIES, {})
    assert [tiers[e.route.id] for e in CODEX] == ["deep", "deep", "standard", "standard", "light"]
    # Claude lists older generations last; its family names decide instead.
    assert [tiers[e.route.id] for e in CLAUDE] == ["deep", "standard", "light", "deep"]


def test_tier_override_moves_one_model():
    assert auto_route.model_tiers(ENTRIES, {"luna": "light"})["codex:luna"] == "light"


def test_heavy_work_avoids_an_account_above_the_threshold():
    assert _pick("deep", codex=90, claude=40) == "claude-opus-5[1m]"
    assert _pick("deep", codex=40, claude=90) == "astra"
    assert _pick("standard", codex=90, claude=40) == "sonnet"


def test_light_work_prefers_the_account_above_the_threshold():
    assert _pick("light", codex=90, claude=40) == "gpt-5.5"
    assert _pick("light", codex=40, claude=90) == "haiku"


def test_both_hot_still_picks_the_one_with_more_left():
    assert _pick("deep", codex=95, claude=88) == "claude-opus-5[1m]"


def test_an_exhausted_account_is_never_chosen():
    assert _pick("light", codex=100, claude=40) == "haiku"
    assert auto_route.pick("deep", CODEX, _usage(codex=100)) is None


def test_missing_tier_falls_back_to_the_nearest():
    only_deep = [CODEX[0]]
    assert auto_route.pick("light", only_deep, _usage()).route.model == "astra"


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


def test_debugging_starts_standard_and_escalates_one_tier():
    assert auto_route.tier_for("worker-debugger") == "standard"
    assert auto_route.tier_for("worker-debugger", escalate=True) == "deep"
    assert auto_route.tier_for("worker-explorer", escalate=True) == "standard"
    assert auto_route.tier_for("worker-security-auditor", escalate=True) == "deep"


def test_planned_complexity_shifts_the_role_tier_one_step():
    assert auto_route.tier_for("worker-debugger", complexity="high") == "deep"
    assert auto_route.tier_for("worker-debugger", complexity="low") == "light"
    assert auto_route.tier_for("worker-security-auditor", complexity="low") == "standard"
    assert auto_route.tier_for("worker-explorer", complexity="low") == "light"
    assert auto_route.tier_for("worker-debugger", escalate=True, complexity="low") == "standard"
