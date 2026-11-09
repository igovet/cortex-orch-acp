"""Bundled adapters replace same-version npm paths without changing account policy."""
import pytest

from cortex_runtime.configuration import management
from cortex_runtime.connections import discovery, registry
from test_approved_adapter_follow import setup


@pytest.mark.parametrize("old,new,changed", [
    ("0.88.0", "0.97.0", True),
    ("0.97.0", "0.97.0", True),
    ("0.97.0", "0.88.0", False),
])
def test_bundle_relocation_preserves_account_and_never_downgrades(tmp_path, monkeypatch, old, new, changed):
    env, record = setup(tmp_path, "claude-agent-acp", old)
    tree = tmp_path / "bundled"
    entry = tree / "node_modules/adapter/entry.js"
    info = discovery.EngineDiscovery("claude-agent-acp", new,
                                    (str(tree / "node/bin/node"), str(entry)), str(tmp_path / ".claude"))
    monkeypatch.setattr(discovery, "bundled_root", lambda: tree)
    monkeypatch.setattr(discovery, "discover_engine", lambda *a, **k: info)
    events = management.follow_approved_adapters(environ=env)
    actual = registry.read_registry(env).connections[record.id]
    assert bool(events) == changed
    assert actual.credential_ref == record.credential_ref
    assert actual.provider == record.provider and actual.endpoint == record.endpoint
    assert actual.max_concurrent_sessions == record.max_concurrent_sessions
    assert actual.launch_command == (info.launch_command if changed else record.launch_command)
    assert management.follow_approved_adapters(environ=env) == []
