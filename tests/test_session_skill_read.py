"""Session-bound project/user skill reads never become arbitrary file reads."""
from __future__ import annotations

import sqlite3
import os
from types import SimpleNamespace

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import contracts, mcp_tools
from cortex_runtime.delegation.session_skills import MAX_SKILL_BYTES, read_session_skill, record_session_skills
from cortex_runtime.delegation.live_adapters import _session_new_meta_for
from cortex_runtime.engine.unified_system_prompt import project_skills_section


class _Store:
    def __init__(self, role="worker"):
        self.db = sqlite3.connect(":memory:")
        self.db.row_factory = sqlite3.Row
        self.db.execute("CREATE TABLE context_generations (id TEXT, engine_session_ref TEXT, task_id TEXT, recipient TEXT, sequence INTEGER)")
        self.db.execute("INSERT INTO context_generations VALUES ('cg_one', 'session-one', 'task-one', ?, 1)", (role,))
        self.db.execute("INSERT INTO context_generations VALUES ('cg_two', 'session-two', 'task-one', ?, 1)", (role,))
        self.db.execute("CREATE TABLE trusted_channels (channel_id TEXT, task_id TEXT, attempt_id TEXT, role TEXT, context_generation_id TEXT, revoked_at TEXT, expires_at TEXT)")
        self.db.execute("INSERT INTO trusted_channels VALUES ('channel-one', 'task-one', 'attempt-one', ?, 'cg_one', NULL, NULL)", (role,))

    def _now(self):
        return "2026-10-02T00:00:00+00:00"

    def connection(self):
        return self.db


def _skill(root, rel, body="---\nname: demo\ndescription: Test.\n---\n\nBody.\n"):
    path = root / rel / "SKILL.md"
    path.parent.mkdir(parents=True)
    path.write_text(body, encoding="utf-8")
    return path


def _setup(tmp_path, role):
    project, home = tmp_path / "project", tmp_path / "home"
    project.mkdir()
    home.mkdir()
    project_skill = _skill(project, ".agents/skills/demo")
    user_skill = _skill(home, ".codex/skills/personal", "---\nname: personal\n---\n\nPrivate body.\n")
    environ = {"HOME": str(home)}
    section = project_skills_section(str(project), environ)
    store = _Store(role)
    record_session_skills(store, task_id="task-one", attempt_id="attempt-one", session_ref="session-one",
                          project_root=str(project), environ=environ,
                          sent_meta={"systemPrompt": "# Cortex session\n\n" + section})
    channel = SimpleNamespace(role=role, task_id="task-one", attempt_id="attempt-one",
                              context_generation_id="cg_one", channel_id="channel-one")
    return store, channel, project_skill, user_skill, project, home


@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_only_exact_session_listed_skill_paths_are_readable(tmp_path, role):
    store, channel, project_skill, user_skill, project, home = _setup(tmp_path, role)
    listed_project = ".agents/skills/demo/SKILL.md"
    assert mcp_tools.dispatch(store, channel, "read_skill", {"skill": listed_project})["body"] == project_skill.read_text()
    assert mcp_tools.dispatch(store, channel, "read_skill", {"skill": str(user_skill)})["body"] == user_skill.read_text()
    sibling = project_skill.parent / "notes.md"
    sibling.write_text("not a skill")
    late = _skill(project, ".agents/skills/late")
    denied = [str(sibling), ".agents/skills/demo/notes.md", ".agents/skills/demo/../demo/SKILL.md",
              "../home/.codex/skills/personal/SKILL.md", str(late),
              str(tmp_path / "unlisted/SKILL.md"), str(home / ".codex/skills/other/SKILL.md")]
    for path in denied:
        with pytest.raises(StoreError, match="unknown_skill"):
            mcp_tools.dispatch(store, channel, "read_skill", {"skill": path})
    store.db.execute("UPDATE trusted_channels SET context_generation_id='cg_two'")
    with pytest.raises(StoreError, match="unknown_skill"):
        read_session_skill(store, channel, listed_project)


def test_symlink_escape_and_retargeting_are_refused(tmp_path):
    store, channel, project_skill, _user_skill, project, _home = _setup(tmp_path, "worker")
    outside = _skill(tmp_path, "outside/secret", "secret")
    escape = project / ".agents/skills/escape"
    escape.symlink_to(outside.parent, target_is_directory=True)
    with pytest.raises(StoreError, match="unknown_skill"):
        read_session_skill(store, channel, ".agents/skills/escape/SKILL.md")
    project_skill.unlink()
    project_skill.symlink_to(outside)
    with pytest.raises(StoreError, match="unknown_skill"):
        read_session_skill(store, channel, ".agents/skills/demo/SKILL.md")


def test_catalog_skips_symlink_to_other_file_inside_project(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    other = _skill(project, "private/secret", "do not list")
    link = project / ".agents/skills/leak"
    link.parent.mkdir(parents=True)
    link.symlink_to(other.parent, target_is_directory=True)
    assert "leak" not in project_skills_section(str(project), environ={})


def test_catalog_skips_hardlinked_skill_body(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    outside = _skill(tmp_path, "outside/secret", "private")
    link = project / ".agents/skills/leak/SKILL.md"
    link.parent.mkdir(parents=True)
    os.link(outside, link)
    assert "leak" not in project_skills_section(str(project), environ={})


def test_skill_size_cap_and_unverified_prompt_fail_closed(tmp_path):
    store, channel, project_skill, _user_skill, project, home = _setup(tmp_path, "coordinator")
    project_skill.write_bytes(b"x" * (MAX_SKILL_BYTES + 1))
    with pytest.raises(StoreError, match="skill_unavailable"):
        read_session_skill(store, channel, ".agents/skills/demo/SKILL.md")
    record_session_skills(store, task_id="task-one", attempt_id="attempt-one", session_ref="session-two",
                          project_root=str(project), environ={"HOME": str(home)}, sent_meta={"systemPrompt": "no skills"})
    store.db.execute("UPDATE trusted_channels SET context_generation_id='cg_two'")
    with pytest.raises(StoreError, match="unknown_skill"):
        read_session_skill(store, channel, ".agents/skills/demo/SKILL.md")
    assert "`demo`" not in project_skills_section(str(project), {"HOME": str(home)})


def test_path_input_is_advertised_and_schema_bounded():
    for role in ("coordinator", "worker"):
        entry = next(tool for tool in mcp_tools.tool_definitions(role) if tool["name"] == "read_skill")
        assert "exact path shown" in entry["description"]
        schema = entry["inputSchema"]["properties"]["skill"]
        assert schema["maxLength"] >= 4096
        contracts.validate(role, "read_skill", {"skill": ".agents/skills/demo/SKILL.md"})
        with pytest.raises(StoreError, match="invalid_arguments"):
            contracts.validate(role, "read_skill", {"skill": "x" * (schema["maxLength"] + 1)})


@pytest.mark.parametrize("engine,version", [("claude-agent-acp", "0.78.0"), ("codex-acp", "1.13.0")])
def test_actual_engine_session_metadata_binds_the_skill_list(tmp_path, engine, version):
    project, home = tmp_path / "project", tmp_path / "home"
    project.mkdir()
    home.mkdir()
    skill = _skill(project, ".agents/skills/demo")
    environ = {"HOME": str(home)}
    meta = _session_new_meta_for(engine, version, "Role text.", role="worker",
                                 project_root=str(project), environ=environ)
    store = _Store()
    record_session_skills(store, task_id="task-one", attempt_id="attempt-one", session_ref="session-one",
                          project_root=str(project), environ=environ, sent_meta=meta)
    channel = SimpleNamespace(role="worker", task_id="task-one", attempt_id="attempt-one",
                              context_generation_id="cg_one", channel_id="channel-one")
    assert read_session_skill(store, channel, ".agents/skills/demo/SKILL.md")["body"] == skill.read_text()


def test_unbound_session_skill_read_fails_closed_before_store_access():
    with pytest.raises(StoreError, match="unknown_skill"):
        read_session_skill(None, None, "../../etc/passwd")


def test_channel_without_attempt_identity_fails_closed_before_store_access():
    channel = SimpleNamespace(channel_id="channel", task_id="task", role="worker")
    with pytest.raises(StoreError, match="unknown_skill"):
        read_session_skill(object(), channel, "../../etc/passwd")


# Real issuance and adapter seams: HEAD's in-memory query fixture does not
# exercise the NULL binding cached by MCP discovery before session/new.
def _issued_session(tmp_path, role, *, engine="codex-acp", version="2.0.1",
                    prompt_mode="observed", custom_codex_home=False):
    import uuid
    from cortex_runtime.bootstrap import bootstrap_task
    from cortex_runtime.portable.identity import SourceEvent
    from cortex_runtime.store import Store
    from cortex_runtime.server import Server
    from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel
    from cortex_runtime.delegation.live_adapters import EngineSessionManagerAdapter

    project, home = tmp_path / "project", tmp_path / "home"
    project.mkdir()
    home.mkdir()
    project_skill = _skill(project, ".agents/skills/demo")
    codex_home = home / "custom-codex" if custom_codex_home else home / ".codex"
    user_skill = _skill(codex_home, "skills/personal")
    environ = {"HOME": str(home)}
    if custom_codex_home:
        environ["CODEX_HOME"] = str(codex_home)
    store = Store(project / ".cortex", project_root=project)
    task = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())),
                          "Read listed skills.", "owner-1")
    attempt = "xa_skill_read"
    with store.connection() as db:
        channel_id, secret = issue_channel_tx(
            db, task.task_id, role, 1, attempt_id=attempt,
            parent_channel_id=task.channel_id if role == "worker" else None)
    channel = resolve_channel(store, channel_id, secret)
    assert channel.context_generation_id is None
    server = Server(channel_credentials=(channel_id, secret, str(project)))
    # tools/list resolves and caches the genuinely unbound channel.
    server.dispatch("tools/list", {})
    assert server._channel_context().context_generation_id is None
    meta = _session_new_meta_for(engine, version, "Role text.", role=role,
                                 project_root=str(project), environ=environ)
    section = "# Cortex session\n\n" + project_skills_section(str(project), environ)
    if prompt_mode == "native":
        meta = {"systemPrompt": "native role only"}
    elif prompt_mode == "unobserved":
        meta = None
    manager = SimpleNamespace(_workspace_root=str(project),
                              sent_session_new_meta=lambda session: meta,
                              sent_engine_instructions=lambda: section if engine == "opencode" else None,
                              invalidate_generation=lambda *args: None)
    record = SimpleNamespace(engine=engine, id="connection-skill", model=None,
                             observed_engine_version=version)
    adapter = EngineSessionManagerAdapter(lambda task_id: record, lambda task_id: str(project),
                                          store=store, environ=environ)
    launch = {"channel_id": channel_id, "role_id": "worker-backend-dev" if role == "worker" else "orchestrator"}
    if role == "worker":
        launch["assignment_id"] = "assignment-skill"
    adapter._open_generation(manager, record, "session-one", task.task_id, attempt, launch,
                             instructions="Role text.", cause="session_open")
    return SimpleNamespace(store=store, task=task, attempt=attempt, channel=channel,
                           channel_id=channel_id, secret=secret, server=server,
                           project=project, home=home, project_skill=project_skill,
                           user_skill=user_skill, adapter=adapter, manager=manager,
                           record=record, launch=launch)


def _mcp_skill(session, path):
    return session.server.dispatch("tools/call", {"name": "read_skill", "arguments": {"skill": path}})


def _assert_unknown(reply):
    import json
    assert reply["isError"], reply
    assert json.loads(reply["content"][0]["text"])["error"] == "unknown_skill", reply


@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_real_issued_channel_reads_through_cached_mcp_context(tmp_path, role):
    session = _issued_session(tmp_path, role)
    from cortex_runtime.trusted_channel import resolve_channel
    assert resolve_channel(session.store, session.channel_id, session.secret).context_generation_id
    for path, body in [(".agents/skills/demo/SKILL.md", session.project_skill.read_text()),
                       (str(session.user_skill), session.user_skill.read_text())]:
        reply = _mcp_skill(session, path)
        assert not reply["isError"], reply
        assert reply["structuredContent"]["body"] == body
    sibling = session.project_skill.parent / "notes.md"
    sibling.write_text("not listed")
    late = _skill(session.project, ".agents/skills/late")
    for path in [str(sibling), str(late), "../home/.codex/skills/personal/SKILL.md",
                 ".agents/skills/demo/../demo/SKILL.md", str(session.home / "unlisted/SKILL.md")]:
        _assert_unknown(_mcp_skill(session, path))


@pytest.mark.parametrize("role", ["coordinator", "worker"])
@pytest.mark.parametrize("fence", ["task", "attempt", "session", "recipient", "revoked", "expired", "superseded"])
def test_real_issued_channel_binding_fences(tmp_path, role, fence):
    from cortex_runtime.delegation import generations
    session = _issued_session(tmp_path, role)
    with session.store.connection() as db:
        if fence in ("task", "attempt"):
            column = "task_id" if fence == "task" else "attempt_id"
            db.execute(f"UPDATE session_skill_paths SET {column}=?", ("different-identity",))
        elif fence == "session":
            db.execute("UPDATE session_skill_paths SET session_ref='another-session'")
        elif fence == "recipient":
            db.execute("UPDATE context_generations SET recipient=?", ("worker" if role == "coordinator" else "coordinator",))
        elif fence == "revoked":
            db.execute("UPDATE trusted_channels SET revoked_at=? WHERE channel_id=?", (session.store._now(), session.channel_id))
        elif fence == "expired":
            db.execute("UPDATE trusted_channels SET expires_at=? WHERE channel_id=?", ("2000-01-01T00:00:00+00:00", session.channel_id))
    if fence == "superseded":
        generations.open_generation(session.store, task_id=session.task.task_id,
                                    engine_session_ref="session-one", recipient=role,
                                    cause="observed_compaction", evidence_source="test")
    _assert_unknown(_mcp_skill(session, ".agents/skills/demo/SKILL.md"))


@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_compaction_advances_live_binding_once_without_expanding_catalog(tmp_path, role):
    session = _issued_session(tmp_path, role)
    late = _skill(session.project, ".agents/skills/late")
    invalidations = []
    session.manager.invalidate_generation = lambda *args: invalidations.append(args)
    event = SimpleNamespace(tool_call_ref="compact-one", compaction_reason="completed")
    for _ in range(2):
        session.adapter._observe_compaction_signal(session.manager, "session-one", event)
    assert len(invalidations) == 1
    with session.store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM context_generations WHERE engine_session_ref='session-one'").fetchone()[0] == 2
        current = db.execute("SELECT context_generation_id FROM trusted_channels WHERE channel_id=?", (session.channel_id,)).fetchone()[0]
    assert current == invalidations[0][1]
    assert not _mcp_skill(session, str(session.user_skill))["isError"]
    _assert_unknown(_mcp_skill(session, str(late)))


@pytest.mark.parametrize("engine,version,mode", [
    ("codex-acp", "1.13.0", "observed"), ("codex-acp", "2.0.1", "observed"),
    ("claude-agent-acp", "0.78.0", "observed"), ("opencode", "2.0.20", "observed"),
    ("codex-acp", "2.0.1", "native"), ("codex-acp", "2.0.1", "unobserved")])
def test_production_generation_uses_only_observed_prompt_grants(tmp_path, engine, version, mode):
    session = _issued_session(tmp_path, "worker", engine=engine, version=version, prompt_mode=mode)
    reply = _mcp_skill(session, str(session.user_skill))
    if mode == "observed":
        assert not reply["isError"], reply
        assert reply["structuredContent"]["body"] == session.user_skill.read_text()
    else:
        _assert_unknown(reply)
        with session.store.connection() as db:
            assert db.execute("SELECT COUNT(*) FROM session_skill_paths").fetchone()[0] == 0


def test_custom_codex_home_and_mismatched_snapshot_environment(tmp_path):
    session = _issued_session(tmp_path, "worker", custom_codex_home=True)
    assert not _mcp_skill(session, str(session.user_skill))["isError"]
    other_home = tmp_path / "other-home"
    other_home.mkdir()
    _skill(other_home, ".codex/skills/different")
    record_session_skills(session.store, task_id=session.task.task_id, attempt_id=session.attempt,
                          session_ref="mismatch", project_root=str(session.project),
                          environ={"HOME": str(other_home)},
                          sent_meta=session.manager.sent_session_new_meta("session-one"))
    with session.store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM session_skill_paths WHERE session_ref='mismatch'").fetchone()[0] == 0


@pytest.mark.parametrize("role", ["coordinator", "worker"])
def test_replacement_requires_fresh_channel_not_cross_session_rebinding(tmp_path, role):
    from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel
    from cortex_runtime.delegation.session_skills import bind_session_skill_generation
    from cortex_runtime.server import Server
    session = _issued_session(tmp_path, role)
    with pytest.raises(StoreError, match="session_skill_binding_invalid"):
        session.adapter._open_generation(session.manager, session.record, "replacement", session.task.task_id,
                                         session.attempt, session.launch, instructions="Role text.", cause="session_replacement")
    with session.store.connection() as db:
        generation = db.execute("SELECT id FROM context_generations WHERE engine_session_ref='replacement'").fetchone()[0]
        channel_id, secret = issue_channel_tx(db, session.task.task_id, role, 1, attempt_id=session.attempt,
                                             parent_channel_id=session.task.channel_id if role == "worker" else None)
    assert resolve_channel(session.store, channel_id, secret).context_generation_id is None
    bind_session_skill_generation(session.store, channel_id=channel_id, task_id=session.task.task_id,
                                  attempt_id=session.attempt, generation_id=generation, session_ref="replacement")
    session.server = Server(channel_credentials=(channel_id, secret, str(session.project)))
    assert not _mcp_skill(session, str(session.user_skill))["isError"]


@pytest.mark.parametrize("cross_process", [False, True])
def test_termination_revokes_grants_even_when_process_exit_is_unconfirmed(tmp_path, monkeypatch, cross_process):
    from cortex_runtime.recovery import registry
    session = _issued_session(tmp_path, "worker")
    adapter = session.adapter
    if cross_process:
        registry.record_process_launch(session.store, attempt_id=session.attempt, task_id=session.task.task_id,
                                       role="worker", pid=99999999, pgid=99999999, start_ticks="0", boot_id="test",
                                       launch_command_digest="test", engine_session_ref="session-one")
        monkeypatch.setattr(adapter, "_terminate_cross_process", lambda attempt: False)
    else:
        adapter._sessions[session.attempt] = "session-one"
        adapter._session_to_attempt["session-one"] = session.attempt
        def shutdown(**kwargs):
            with session.store.connection() as db:
                assert db.execute("SELECT COUNT(*) FROM session_skill_paths").fetchone()[0] == 0
        adapter._managers[session.attempt] = SimpleNamespace(shutdown=shutdown,
                                                           _process=SimpleNamespace(wait_exited=lambda timeout: False))
    adapter.terminate_owned(session.attempt)
    _assert_unknown(_mcp_skill(session, str(session.user_skill)))
    with session.store.connection() as db:
        assert db.execute("SELECT COUNT(*) FROM session_skill_paths").fetchone()[0] == 0
