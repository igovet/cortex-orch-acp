"""User routing profiles: revision checks, materialization and divergence."""
import json

import pytest

from cortex_runtime.configuration import management, routing_profiles
from cortex_runtime.portable.errors import PortableError
from cortex_runtime.routing import auto_route


@pytest.fixture
def env(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    return {"HOME": str(home), "XDG_CONFIG_HOME": str(tmp_path / "config")}


def _read(env):
    return management.execute({"surface": "routing_profiles", "action": "read"}, environ=env)["result"]


def _run(env, action, revision, **kwargs):
    return management.execute(dict(surface="routing_profiles", action=action,
                                   expected_revision=revision, **kwargs), environ=env)


def test_create_activate_diverge_and_deactivate(env):
    first = _read(env)
    assert first["revision"] == "absent" and first["state"] == "none" and first["profiles"] == []
    rules = {"providers": ["claude-agent-acp"], "deny": ["codex-acp/*", "opencode/*"]}
    created = _run(env, "create", first["revision"], name="claude-only", rules=rules)
    assert created["ok"] and created["result"]["profiles"][0]["rules"] == rules
    assert _run(env, "create", first["revision"], name="mix", rules={})["error"]["code"] == "revision_conflict"
    view = _read(env)
    active = _run(env, "activate", view["revision"], name="claude-only",
                  expected_routing_revision=view["user_routing_revision"])
    assert active["ok"] and active["result"]["state"] == "in_sync"
    path = routing_profiles._paths(env)[1]
    assert json.loads(path.read_text()) == rules
    assert path.stat().st_mode & 0o777 == 0o600
    assert auto_route.load_overrides(environ=env) == rules
    assert auto_route._effective_profile_label(None, env) == {"name": "claude-only", "state": "in_sync"}
    assert _run(env, "delete", _read(env)["revision"], name="claude-only")["error"]["code"] == "profile_active"
    path.write_text('{"providers":["codex-acp"]}')
    assert _read(env)["state"] == "diverged"
    stale = _run(env, "activate", _read(env)["revision"], name="claude-only",
                 expected_routing_revision=view["user_routing_revision"])
    assert stale["error"]["code"] == "revision_conflict"
    assert json.loads(path.read_text()) == {"providers": ["codex-acp"]}
    again = _read(env)
    assert _run(env, "activate", again["revision"], name="claude-only",
                expected_routing_revision=again["user_routing_revision"])["result"]["state"] == "in_sync"
    detached = _run(env, "deactivate", _read(env)["revision"])
    assert detached["result"]["state"] == "none" and json.loads(path.read_text()) == rules


def test_user_save_writes_through_but_project_save_does_not(env, tmp_path):
    created = _run(env, "create", "absent", name="mix", rules={})["result"]
    activated = _run(env, "activate", created["revision"], name="mix",
                     expected_routing_revision="absent")["result"]
    saved = management.update_routing("user", activated["user_routing_revision"], {"deny": ["opencode/*"]},
                                      environ=env)
    assert saved["profile"]["written_through"] is True
    assert _read(env)["state"] == "in_sync"
    assert _read(env)["profiles"][0]["rules"] == {"deny": ["opencode/*"]}
    project = tmp_path / "project"
    (project / ".cortex").mkdir(parents=True)
    management.update_routing("project", "absent", {"deny": ["codex-acp/*"]},
                              project_root=str(project), environ=env)
    assert routing_profiles.read_profiles(str(project), env)["shadowed_by"] == "project"
    assert auto_route._effective_profile_label(str(project), env) is None
    assert _read(env)["profiles"][0]["rules"] == {"deny": ["opencode/*"]}


def test_active_update_requires_routing_revision_and_diverged_save_does_not_write_through(env):
    created = _run(env, "create", "absent", name="mix", rules={})["result"]
    active = _run(env, "activate", created["revision"], name="mix",
                  expected_routing_revision="absent")["result"]
    failed = _run(env, "update", active["revision"], name="mix", rules={"deny": ["opencode/*"]},
                  expected_routing_revision="absent")
    assert failed["error"]["code"] == "revision_conflict"
    updated = _run(env, "update", active["revision"], name="mix", rules={"deny": ["opencode/*"]},
                   expected_routing_revision=active["user_routing_revision"])["result"]
    assert updated["state"] == "in_sync"
    assert auto_route.load_overrides(environ=env) == {"deny": ["opencode/*"]}
    path = routing_profiles._paths(env)[1]
    path.write_text('{"deny":["codex-acp/*"]}')
    revision = _read(env)["user_routing_revision"]
    saved = management.update_routing("user", revision, {"deny": ["claude-agent-acp/*"]}, environ=env)
    assert saved["profile"] == {"name": "mix", "state": "diverged"}
    assert _read(env)["profiles"][0]["rules"] == {"deny": ["opencode/*"]}


def test_validation_format_and_unsafe_storage(env):
    for name in ("Bad", "-bad", "x" * 41):
        assert _run(env, "create", "absent", name=name, rules={})["error"]["code"] == "invalid_profile_name"
    path, _ = routing_profiles._paths(env)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text('{"format":2,"profiles":{}}')
    path.chmod(0o600)
    assert management.execute({"surface": "routing_profiles", "action": "read"}, environ=env)["error"]["code"] == "unsupported_profiles_format"
    routing_save = management.update_routing("user", "absent", {"deny": ["opencode/*"]}, environ=env)
    assert routing_save["profile"] == {"state": "unavailable", "code": "unsupported_profiles_format"}
    assert auto_route.load_overrides(environ=env) == {"deny": ["opencode/*"]}
    path.unlink()
    target = path.parent / "target"
    target.write_text("{}")
    path.symlink_to(target)
    assert management.execute({"surface": "routing_profiles", "action": "read"}, environ=env)["error"]["code"] == "unsafe_storage"


def test_failed_second_write_is_visible_as_diverged(env, monkeypatch):
    created = _run(env, "create", "absent", name="mix", rules={"deny": ["opencode/*"]})["result"]
    active = _run(env, "activate", created["revision"], name="mix",
                  expected_routing_revision="absent")["result"]
    other = _run(env, "create", active["revision"], name="claude-only",
                 rules={"deny": ["codex-acp/*"]})["result"]
    original = management._replace_file

    def replace(path, body):
        if path.name == "routing-profiles.json":
            raise RuntimeError("simulated crash")
        original(path, body)

    monkeypatch.setattr(management, "_replace_file", replace)
    with pytest.raises(RuntimeError, match="simulated crash"):
        routing_profiles.mutate("activate", other["revision"], name="claude-only",
                                expected_routing_revision=other["user_routing_revision"], environ=env)
    assert _read(env)["state"] == "diverged"
    monkeypatch.undo()
    current = _read(env)
    assert _run(env, "activate", current["revision"], name="claude-only",
                expected_routing_revision=current["user_routing_revision"])["result"]["state"] == "in_sync"
