"""Install-only model data; existing files stay byte-identical, cleanup is explicit."""
import json
import stat
from pathlib import Path

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
    return Path(environ["XDG_CONFIG_HOME"]) / "cortex"


def test_default_routing_json_is_exactly_the_owner_default(environ):
    result = defaults.ensure_default_routing(environ)
    path = _config(environ) / "routing.json"
    assert result["status"] == "created" and result["changed"]
    assert json.loads(path.read_text()) == defaults.default_routing()
    assert stat.S_IMODE(path.stat().st_mode) == 0o600
    assert defaults.ensure_default_routing(environ)["status"] == "current"
    assert auto_route.load_overrides(None, environ) == defaults.default_routing()
    assert auto_route.is_denied("claude-agent-acp", "claude-haiku-4-5-20251001", None, environ)


@pytest.mark.parametrize("data,status", [
    ({"deny": ["custom-model"], "providers": ["codex-acp"], "pin": {"planner": "codex-acp/custom-model"}},
     "model_policy_migration_required"),
    ({"model_policy_version": 2, "enabled": {"codex-acp": []}, "deny": []}, "current"),
    ({"model_policy_version": 99}, "model_policy_version_unsupported"),
])
def test_existing_owner_bytes_are_never_patched(environ, data, status):
    path = _config(environ) / "routing.json"
    original = json.dumps(data, separators=(",", ":")) + "\n"
    path.write_text(original)
    path.chmod(0o600)
    result = defaults.ensure_default_routing(environ)
    assert result["status"] == status and not result["changed"]
    assert path.read_text() == original


def test_an_invalid_routing_json_is_reported_and_left_untouched(environ):
    path = _config(environ) / "routing.json"
    path.write_text("{not json")
    path.chmod(0o600)
    assert defaults.ensure_default_routing(environ)["status"] == "invalid"
    assert path.read_text() == "{not json"


def test_install_never_cleans_legacy_owner_policy(environ):
    path = _config(environ) / "route-policy.json"
    original = '{"roles":{"worker-explorer":[{"engine":"codex-acp","model":"gpt-5.5"}]}}'
    path.write_text(original)
    result = defaults.apply(environ)
    assert result["routing"]["status"] == "created"
    assert result["route_policy"] == {"status": "not_requested", "changed": False}
    assert path.read_text() == original
    assert not list(path.parent.glob("*.cortex-backup-*"))


def test_explicit_route_policy_cleanup_preserves_unmatched_variants_and_keeps_backup(environ):
    (_config(environ) / "routing.json").write_text(json.dumps({"model_policy_version": 2, "deny": ["haiku", "gpt-5.5"]}))
    policy = {"_comment": ["kept"], "roles": {
        "worker-technical-writer": [{"engine": "codex-acp", "model": "gpt-6-luna"},
                                     {"engine": "claude-agent-acp", "model": "haiku"}],
        "worker-explorer": [{"engine": "codex-acp", "model": "gpt-5.5"}],
        "coordinator": [{"engine": "claude-agent-acp", "model": "*"}]},
        "default": [{"engine": "claude-agent-acp", "model": "claude-haiku-4-5-20251001"},
                    {"engine": "codex-acp", "model": "*"}]}
    path = _config(environ) / "route-policy.json"
    original = json.dumps(policy)
    path.write_text(original)
    result = defaults.clean_route_policy(environ)
    assert result["status"] == "cleaned"
    assert result["removed"] == ["claude-agent-acp/haiku", "codex-acp/gpt-5.5"]
    assert result["removed_roles"] == ["worker-explorer"]
    expected = {**policy, "roles": {k: v for k, v in policy["roles"].items() if k != "worker-explorer"}}
    expected["roles"]["worker-technical-writer"] = [{"engine": "codex-acp", "model": "gpt-6-luna"}]
    assert json.loads(path.read_text()) == expected
    backup = Path(result["backup"])
    assert backup.read_text() == original and stat.S_IMODE(backup.stat().st_mode) == 0o600
    assert defaults.clean_route_policy(environ)["status"] == "current"


def test_explicit_cleanup_uses_owner_extra_exact_deny(environ):
    (_config(environ) / "routing.json").write_text(json.dumps({"model_policy_version": 2, "deny": ["gpt-5.6-luna"]}))
    path = _config(environ) / "route-policy.json"
    path.write_text(json.dumps({"roles": {"worker-general": [
        {"engine": "codex-acp", "model": "gpt-5.6-luna"}, {"engine": "codex-acp", "model": "gpt-6-luna"}]}}))
    result = defaults.clean_route_policy(environ)
    assert result["removed"] == ["codex-acp/gpt-5.6-luna"]
    assert json.loads(path.read_text())["roles"]["worker-general"] == [{"engine": "codex-acp", "model": "gpt-6-luna"}]


def test_absent_route_policy_is_not_created(environ):
    assert defaults.clean_route_policy(environ)["status"] == "absent"
    assert not (_config(environ) / "route-policy.json").exists()


def test_symlink_file_refused_without_touching_target(environ, tmp_path):
    target = tmp_path / "owner.json"
    target.write_text("owner content")
    (_config(environ) / "routing.json").symlink_to(target)
    assert defaults.ensure_default_routing(environ)["status"] == "unsafe_storage"
    assert target.read_text() == "owner content"


def test_racing_owner_creation_wins(environ, monkeypatch):
    real_link = defaults.os.link
    path = _config(environ) / "routing.json"
    def competing_link(source, destination):
        path.write_text("owner won")
        return real_link(source, destination)
    monkeypatch.setattr(defaults.os, "link", competing_link)
    result = defaults.ensure_default_routing(environ)
    assert result["status"] == "existing" and not result["changed"]
    assert path.read_text() == "owner won"
    assert not list(path.parent.glob("*.tmp"))


def test_invalid_asset_blocks_all_writes(environ, tmp_path, monkeypatch):
    path = tmp_path / "asset.json"
    path.write_text('{"schema_version":99}')
    monkeypatch.setattr(defaults, "TEMPLATE_PATH", path)
    assert defaults.ensure_default_routing(environ)["status"] == "invalid"
    assert not (_config(environ) / "routing.json").exists()


def test_setup_step_applies_only_install_data(environ, monkeypatch, capsys):
    monkeypatch.syspath_prepend(str(Path(__file__).resolve().parent.parent / "scripts"))
    for key, value in environ.items(): monkeypatch.setenv(key, value)
    import cortex_acp_setup as setup
    assert setup.main(["apply-routing-defaults"]) == 0
    printed = json.loads(capsys.readouterr().out)
    assert printed["routing"]["status"] == "created"
    assert json.loads((_config(environ) / "routing.json").read_text()) == defaults.default_routing()


def test_explicit_cleanup_requires_migrated_routing_and_never_changes_legacy_bytes(environ):
    routing = _config(environ) / "routing.json"
    routing.write_text('{"deny":["owner-model"]}')
    path = _config(environ) / "route-policy.json"
    path.write_text('{"default":[{"engine":"codex-acp","model":"owner-model"}]}')
    original = path.read_bytes()
    result = defaults.clean_route_policy(environ)
    assert result["status"] == "model_policy_migration_required" and not result["changed"]
    assert path.read_bytes() == original
