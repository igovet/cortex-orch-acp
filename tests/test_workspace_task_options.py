"""Saved task options and real worktree preparation; no engine calls."""
import os
from pathlib import Path
from types import SimpleNamespace

import pytest

from cortex_runtime.workspace_api import task_options
from cortex_runtime.acp.agent import Agent
from test_workspace_api import api_for, create_command
from test_workspace_machine_tools import machine


def test_plain_folder_uses_main_even_if_machine_default_requested_worktree(tmp_path):
    api = api_for(tmp_path)
    command = create_command()
    command["payload"]["checkout"] = {"kind": "worktree"}
    task = api.handle(command)["target"]
    assert task_options.prepare(api.store, task, {}) == str(tmp_path)
    with api.store.connection() as db:
        assert task_options.read(db, task)["checkout"] == {"kind": "main"}
    assert not (tmp_path / ".git").exists()


def test_task_choices_persist_and_prepare_one_shared_store(machine, tmp_path):
    api = api_for(machine.repo)
    settings = dict(model="route", effort="high", mode="guarded", language="en")
    command = create_command()
    command["payload"].update(settings=settings, checkout={"kind":"worktree"})
    task = api.handle(command)["target"]
    env = dict(HOME=str(tmp_path), CORTEX_WORKSPACE_STATE_DIR=str(tmp_path / "state"), XDG_DATA_HOME=str(tmp_path / "data"))
    tree = Path(task_options.prepare(api.store, task, env))
    assert tree.is_dir() and tree != machine.repo
    assert task_options.prepare(api.store, task, env) == str(tree)
    from cortex_runtime.project_storage import logical_project_root
    assert logical_project_root(tree) == str(machine.repo)
    assert not (tree / ".cortex").exists()
    with api.store.connection() as db:
        saved = task_options.read(db, task)
    assert saved["settings"] == settings
    assert saved["checkout"]["path"] == str(tree)
    assert saved["baseRevision"]
    assert task not in str(tree) and task not in saved["checkout"]["branch"]
    assert saved["checkout"]["branch"].startswith("cw/")
    assert tree.name == saved["checkout"]["branch"].removeprefix("cw/")
    # Changing the default root never moves or recreates an existing checkout.
    state = tmp_path / "state"
    state.mkdir(exist_ok=True)
    (state / "machine.json").write_text(__import__("json").dumps({"worktreeRoot": str(tmp_path / "new-trees")}))
    assert task_options.prepare(api.store, task, env) == str(tree)
    (machine.repo / ".cortex-workspace-dev.json").write_text("{}")
    assert ".cortex-workspace-dev.json" not in __import__("subprocess").check_output(["git","status","--porcelain"],cwd=machine.repo,text=True)


def test_existing_legacy_checkout_keeps_its_real_name(machine, tmp_path):
    import hashlib
    import subprocess
    api = api_for(machine.repo)
    command = create_command()
    command["payload"]["checkout"] = {"kind": "worktree"}
    task = api.handle(command)["target"]
    common = task_options.git(machine.repo, "rev-parse", "--path-format=absolute", "--git-common-dir")
    base = tmp_path / "data/cortex-workspace/worktrees"
    tree = base / hashlib.sha256(common.encode()).hexdigest()[:16] / task
    tree.parent.mkdir(parents=True)
    branch = "cw/" + task
    subprocess.run(["git", "worktree", "add", "-b", branch, str(tree)], cwd=machine.repo, capture_output=True, check=True)
    env = dict(HOME=str(tmp_path), CORTEX_WORKSPACE_STATE_DIR=str(tmp_path / "state"), XDG_DATA_HOME=str(tmp_path / "data"))
    assert task_options.prepare(api.store, task, env) == str(tree)
    assert task_options.git(tree, "branch", "--show-current") == branch


def test_configured_route_effort_mode_and_language_apply_without_changing_defaults(tmp_path, monkeypatch):
    import threading
    from test_acp_config_options import _isolate, _configured_connection, _build_and_save_snapshot, _new_agent
    from cortex_runtime.configuration import store as configuration_store
    from cortex_runtime.configuration.service import ConfigurationService
    from cortex_runtime.acp.steering import get_language_override
    from cortex_runtime.configuration.access_mode import current
    env = _isolate(tmp_path, monkeypatch)
    record = _configured_connection(environ=env)
    _build_and_save_snapshot(record,environ=env,thought_level="high")
    project = tmp_path / "project"
    project.mkdir()
    agent, _ = _new_agent(tmp_path, env)
    sid = agent._session_new(dict(cwd=str(project),mcpServers=[]))["sessionId"]
    session = agent.sessions[sid]
    entry = agent._selectable_entries_and_connections(str(project))[0][0]
    before = configuration_store.read_user_defaults(env)
    agent._workspace_select(session,dict(model=entry.route.id,effort="high",mode="auto",language="en"))
    store = agent._store_for(str(project))
    agent._apply_access_mode_selection(store,session,sid,"auto",origin="acp_pre_task_selection")
    agent._run_prompt(42,sid,session,"Explain this fixture.",threading.Event())
    task = session["task_id"]
    latest = ConfigurationService(store).history(task)[-1]
    assert latest["route_id"] == entry.route.id
    assert latest["parameters"]["thought_level"] == "high"
    assert get_language_override(store,task) == "en"
    assert current(store,task) == "auto"
    assert configuration_store.read_user_defaults(env) == before


def test_selection_is_session_local_and_auto_does_not_inherit_language(monkeypatch):
    entry = SimpleNamespace(route=SimpleNamespace(id="route",model="model"),connection_id="connection")
    agent = object.__new__(Agent)
    agent.environ = {}
    monkeypatch.setattr(agent, "_selectable_entries_and_connections", lambda *_: ([entry], {}))
    session = {"project":"/fixture","task_id":None,"default_selection":{"old":True}}
    agent._workspace_select(session, dict(model="route",effort="high",mode="guarded",language="auto"))
    assert session["pending_selection"]["parameters"] == {"thought_level":"high"}
    assert session["default_selection"] is None
    monkeypatch.setattr(agent, "_default_language_from_user_defaults", lambda: "ru")
    assert agent._current_language_for_session(None, session) is None
    with pytest.raises(Exception, match="task_route_unavailable"):
        agent._workspace_select(session, dict(model="unknown",effort="",mode="guarded",language="en"))


def test_multi_setting_failure_reports_exact_applied_fields(tmp_path):
    import threading
    from cortex_runtime.workspace_api.control import apply_settings
    api = api_for(tmp_path)
    task = api.handle(create_command())["target"]
    entry = SimpleNamespace(route=SimpleNamespace(id="route"))
    chosen = dict(model="route", effort="high", mode="auto", language="en")
    session = dict(project=str(tmp_path), lock=threading.RLock(), active=None)
    def select(session, chosen):
        session["pending_selection"] = dict(route_id="route", parameters={})
    agent = SimpleNamespace(
        _workspace_select=select,
        _selectable_entries_and_connections=lambda *_: ([entry], {}),
        _apply_mid_task_selection=lambda *_: (True, None, None),
        _apply_language_selection=lambda *_: (_ for _ in ()).throw(RuntimeError("secret-not-for-output")),
        _apply_access_mode_selection=lambda *_: pytest.fail("must stop after failure"))
    reply = apply_settings(agent, api, session, "session", task, chosen)
    assert reply["outcome"] == "partial_effect"
    assert reply["applied"] == ["model", "effort"]
    assert reply["notApplied"] == ["language", "mode"]
    assert "secret" not in str(reply)

