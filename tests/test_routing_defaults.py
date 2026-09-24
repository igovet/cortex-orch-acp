"""routing.defaults: the default global routing.json and the leftover
route-policy.json cleanup run by explicit setup."""
from __future__ import annotations

import json
import stat

import pytest

from cortex_runtime.routing import auto_route, defaults


@pytest.fixture
def environ(tmp_path):
    home = tmp_path / "home"
    (tmp_path / "config" / "cortex").mkdir(parents=True)
    home.mkdir()
    return {"HOME": str(home), "XDG_CONFIG_HOME": str(tmp_path / "config"),
            "XDG_STATE_HOME": str(tmp_path / "state")}


def _config(environ):
    from pathlib import Path

    return Path(environ["XDG_CONFIG_HOME"]) / "cortex"


def test_default_routing_json_is_exactly_the_owner_default(environ):
    result = defaults.ensure_default_routing(environ)
    path = _config(environ) / "routing.json"
    assert result["status"] == "created" and result["changed"] is True
    assert path.read_text(encoding="utf-8") == '{\n  "deny": ["gpt-5.5", "haiku"]\n}\n'
    assert json.loads(path.read_text(encoding="utf-8")) == {"deny": ["gpt-5.5", "haiku"]}
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert defaults.ensure_default_routing(environ)["status"] == "current"
    # The runtime reads it as the global override.
    assert auto_route.load_overrides(None, environ) == {"deny": ["gpt-5.5", "haiku"]}
    assert auto_route.is_denied("claude-agent-acp", "claude-haiku-4-5-20251001", None, environ)


def test_an_existing_routing_json_keeps_its_settings_and_gains_missing_defaults(environ):
    path = _config(environ) / "routing.json"
    path.write_text(json.dumps({"deny": ["gpt-5.5", "gpt-5.6-sol"], "providers": ["codex-acp"],
                                "pin": {"planner": "codex-acp/gpt-6-luna"}}))
    result = defaults.ensure_default_routing(environ)
    assert (result["status"], result["added"]) == ("updated", ["haiku"])
    assert json.loads(path.read_text(encoding="utf-8")) == {
        "deny": ["gpt-5.5", "gpt-5.6-sol", "haiku"], "providers": ["codex-acp"],
        "pin": {"planner": "codex-acp/gpt-6-luna"}}


def test_an_invalid_routing_json_is_reported_and_left_untouched(environ):
    path = _config(environ) / "routing.json"
    path.write_text("{not json")
    assert defaults.ensure_default_routing(environ)["status"] == "invalid"
    assert path.read_text(encoding="utf-8") == "{not json"


def test_route_policy_loses_denied_models_and_keeps_everything_else(environ):
    policy = {
        "_comment": ["kept"],
        "roles": {
            "worker-technical-writer": [
                {"engine": "codex-acp", "model": "gpt-6-luna", "efforts": ["high"]},
                {"engine": "claude-agent-acp", "model": "haiku", "efforts": ["high"]},
            ],
            "worker-explorer": [{"engine": "codex-acp", "model": "gpt-5.5", "efforts": ["low"]}],
            "coordinator": [{"engine": "claude-agent-acp", "model": "*", "efforts": ["*"]}],
        },
        "default": [{"engine": "claude-agent-acp", "model": "claude-haiku-4-5-20251001"},
                    {"engine": "codex-acp", "model": "*"}],
    }
    path = _config(environ) / "route-policy.json"
    path.write_text(json.dumps(policy))
    original = path.read_text(encoding="utf-8")
    result = defaults.apply(environ)
    assert result["routing"]["status"] == "created"
    cleaned = result["route_policy"]
    assert cleaned["status"] == "cleaned"
    assert cleaned["removed"] == ["claude-agent-acp/claude-haiku-4-5-20251001", "claude-agent-acp/haiku",
                                  "codex-acp/gpt-5.5"]
    assert cleaned["removed_roles"] == ["worker-explorer"]
    assert json.loads(path.read_text(encoding="utf-8")) == {
        "_comment": ["kept"],
        "roles": {
            "worker-technical-writer": [{"engine": "codex-acp", "model": "gpt-6-luna", "efforts": ["high"]}],
            "coordinator": [{"engine": "claude-agent-acp", "model": "*", "efforts": ["*"]}],
        },
        "default": [{"engine": "codex-acp", "model": "*"}],
    }
    from pathlib import Path

    backup = Path(cleaned["backup"])
    assert backup.read_text(encoding="utf-8") == original
    assert stat.S_IMODE(backup.stat().st_mode) == 0o600
    assert defaults.clean_route_policy(environ)["status"] == "current"


def test_route_policy_cleanup_uses_the_users_own_extra_deny(environ):
    (_config(environ) / "routing.json").write_text(json.dumps({"deny": ["gpt-5.6-luna"]}))
    path = _config(environ) / "route-policy.json"
    path.write_text(json.dumps({"roles": {"worker-general": [
        {"engine": "codex-acp", "model": "gpt-5.6-luna"}, {"engine": "codex-acp", "model": "gpt-6-luna"}]}}))
    result = defaults.apply(environ)
    assert result["routing"]["added"] == ["gpt-5.5", "haiku"]
    assert result["route_policy"]["removed"] == ["codex-acp/gpt-5.6-luna"]
    assert json.loads(path.read_text(encoding="utf-8"))["roles"]["worker-general"] == [
        {"engine": "codex-acp", "model": "gpt-6-luna"}]


def test_absent_route_policy_is_not_created(environ):
    assert defaults.clean_route_policy(environ)["status"] == "absent"
    assert not (_config(environ) / "route-policy.json").exists()


def test_setup_step_applies_the_defaults(environ, monkeypatch, capsys):
    from pathlib import Path

    scripts = Path(__file__).resolve().parent.parent / "scripts"
    monkeypatch.syspath_prepend(str(scripts))
    for key, value in environ.items():
        monkeypatch.setenv(key, value)
    import cortex_acp_setup as setup

    assert setup.main(["apply-routing-defaults"]) == 0
    printed = json.loads(capsys.readouterr().out)
    assert printed["routing"]["status"] == "created"
    assert json.loads((_config(environ) / "routing.json").read_text()) == {"deny": ["gpt-5.5", "haiku"]}
