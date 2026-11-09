"""Fixture-only cloning: production bytes stay identical and secrets never escape."""
import hashlib
import importlib.util
import json
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]


def cloner():
    spec = importlib.util.spec_from_file_location("clone_settings", ROOT / "scripts/clone_cortex_dev_settings.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def hashes(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob("*") if p.is_file()}


def test_clone_settings_without_secrets_or_state(tmp_path):
    source, target = tmp_path / "prod", tmp_path / "dev/cortex"
    source.mkdir()
    data = {
        "connections.json": {"schema_version": 1, "connections": {
            "cn_native": {"name": "Codex", "auth_mode": "native_account", "engine": "codex-acp",
                          "credential_ref": {"kind": "engine_native_dir", "path": "/native/account", "owned": False},
                          "launch_command": ["/bin/engine", "--token", "CANARY"],
                          "state": "configured"},
            "cn_key": {"auth_mode": "api_key", "credential_ref": {"path": "/secret"}}}},
        "routing.json": {"version": 2, "workers": {"default": {"model": "model"}}},
        "mcp-servers.json": {"version": 1, "servers": {"safe": {
            "command": "/bin/mcp", "args": ["--token=CANARY"], "env": {"TOKEN": "CANARY"},
            "allowed_tools": ["read"], "read_only_tools": ["read"], "coordinator_tools": ["read"]}}},
        "defaults.json": {"defaults": {"language": "en"}, "token": "CANARY"},
    }
    for name, value in data.items():
        (source / name).write_text(json.dumps(value))
    (source / "secrets").mkdir()
    (source / "secrets/key").write_text("CANARY")
    (source / "catalog.sqlite3").write_bytes(b"CANARY")
    before = hashes(source)
    result = cloner().clone_settings(source, target)
    assert hashes(source) == before
    assert set(p.name for p in target.iterdir()) == set(data)
    joined = "".join(p.read_text() for p in target.iterdir())
    assert "CANARY" not in joined
    connections = json.loads((target / "connections.json").read_text())
    assert set(connections["connections"]) == {"cn_native"}
    assert connections["connections"]["cn_native"]["launch_command"] is None
    assert connections["connections"]["cn_native"]["state"] == "authorization_required"
    mcp = json.loads((target / "mcp-servers.json").read_text())["servers"]["safe"]
    assert mcp["enabled"] is False and mcp["args"] == [] and mcp["env"] == {}
    assert mcp["coordinator_tools"] == ["read"]
    assert result["skipped"]
    assert "CANARY" not in json.dumps(result)
    with pytest.raises(ValueError, match="overwrite"):
        cloner().clone_settings(source, target)
    cloner().clone_settings(source, target, overwrite=True)
    assert hashes(source) == before


@pytest.mark.parametrize("child", ["", "nested"])
def test_refuses_production_target_without_any_mutation(tmp_path, child):
    source = tmp_path / "prod"
    source.mkdir()
    (source / "defaults.json").write_text("{}")
    before = hashes(source)
    with pytest.raises(ValueError, match="production"):
        cloner().clone_settings(source, source / child)
    assert hashes(source) == before


def test_cloned_native_connection_is_valid_for_the_real_registry(tmp_path):
    from cortex_runtime.connections import registry
    source, target = tmp_path / "prod", tmp_path / "dev"
    source.mkdir()
    record = dict(id="cn_123456789abc", name="Codex", engine="codex-acp", provider="openai",
                  auth_mode="native_account", key_kind=None, endpoint=None, launch_command=None,
                  credential_ref=dict(kind="engine_native_dir", path="/native/account", owned=False),
                  approved_fingerprint="fixture-fingerprint", state="configured", qualified=False,
                  created_at="now", updated_at="now", max_concurrent_sessions=2)
    (source / "connections.json").write_text(json.dumps(dict(schema_version=1,
        connections={record["id"]: record}, override_history={})))
    cloner().clone_settings(source, target)
    data = json.loads((target / "connections.json").read_text())
    parsed = registry._record_from_dict(data["connections"][record["id"]])
    assert parsed.credential_ref.owned is False
    assert parsed.max_concurrent_sessions == 2
    assert parsed.approved_fingerprint == "dev-review-required"


def test_refuses_production_state_and_account_locations_without_creating_target(tmp_path):
    source = tmp_path / "prod-config"
    state = tmp_path / "prod-state"
    source.mkdir()
    state.mkdir()
    for target in (state, state / "nested"):
        with pytest.raises(ValueError, match="production"):
            cloner().clone_settings(source, target, production_paths=(state,))
    assert list(state.iterdir()) == []
    with pytest.raises(ValueError, match="Paseo"):
        cloner().clone_settings(tmp_path / ".paseo", tmp_path / "dev")
    assert not (tmp_path / ".paseo").exists()


def test_refuses_links_before_writing(tmp_path):
    source, target = tmp_path / "prod", tmp_path / "dev"
    source.mkdir()
    (source / "defaults.json").symlink_to(tmp_path / "secret")
    with pytest.raises(ValueError, match="link"):
        cloner().clone_settings(source, target)
    assert not target.exists()
