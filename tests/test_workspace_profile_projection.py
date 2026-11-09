"""Native profile metadata must describe its actual saved policy, without executable definitions."""
import importlib.util
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def test_profile_sections_map_all_policy_without_definition_fields():
    spec = importlib.util.spec_from_file_location("profile_machine", ROOT / "crates/workspace-service/machine_tools.py")
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    raw = {
        "routing": {"model_policy_version": 2, "enabled": {"codex-acp": ["fixture"]}},
        "user_defaults": {"defaults": {"language": "en"}, "ceiling": {"limits": {"attempts": 5}}},
        "mcp_servers": {"fixture": {"enabled": True, "allowed_tools": ["read"],
                                   "read_only_tools": ["read"], "coordinator_tools": []}},
        "connections": {"cn_123456789abc": {"max_concurrent_sessions": 2}},
    }
    projected = m.profile_sections(raw, {"mode": "guarded", "checkout": "main"})
    assert set(projected) == {"routing", "defaults", "mcp", "sessions"}
    assert projected["routing"]["engines"]["codex-acp"]["enabled"] == ["fixture"]
    assert projected["defaults"]["ceiling"]["attempts"] == 5
    assert projected["defaults"]["language"] == "en"
    assert projected["mcp"][0]["allowedTools"] == ["read"]
    assert not {"command", "args", "env"} & projected["mcp"][0].keys()
    assert projected["sessions"] == {"cn_123456789abc": 2}
    assert m.profile_sections({"routing": {}}, {}) .keys() == {"routing"}
