"""Owner upload snapshots: no real Paseo home, network, or model calls."""
import hashlib
import json
import os
import threading

import pytest

from cortex_runtime.acp import replay_store
from cortex_runtime.acp.agent import Agent
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.ownership import current_owner_epoch
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.recovery.capsule import build_capsule
from cortex_runtime.store import Store
from cortex_runtime.trusted_channel import issue_channel_tx, resolve_channel
from test_acp_input_history_and_replay import _CountingFakeRuntime, _RecordingTransport


@pytest.fixture
def upload(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    home = tmp_path / "home"
    path = home / ".paseo" / "uploads" / "upload_test" / "quick-add-fab.md"
    path.parent.mkdir(parents=True)
    path.write_text("owner attachment body", encoding="utf-8")
    env = dict(HOME=str(home), PASEO_HOME=str(home / ".paseo"),
               XDG_CONFIG_HOME=str(home / "config"), XDG_STATE_HOME=str(home / "state"))
    return project, path, env


def resolve(upload, text=None, blocks=None):
    project, path, env = upload
    blocks = blocks if blocks is not None else [dict(type="text", text=text or f"Read\n{path}")]
    return replay_store.resolve_prompt_blocks(blocks, str(project), environ=env)


@pytest.mark.parametrize("form", ["plain", "inline", "quoted", "paseo", "link"])
def test_upload_delivery_forms(upload, form):
    _, path, _ = upload
    texts = dict(plain=f"Read\n{path}", inline=f"Read {path}",
                 quoted=f'Read "{path}"', paseo=f"Uploaded file: {path.name}\nPath: {path}\nMIME: text/markdown\nSize: 21 bytes")
    blocks = [dict(type="resource_link", uri=path.as_uri(), name=path.name)] if form == "link" else None
    capture, = resolve(upload, text=texts.get(form), blocks=blocks)
    assert capture.availability == "stored"
    assert capture.body == b"owner attachment body"
    assert capture.name == path.name
    assert capture.uri == path.as_uri()


def test_paths_with_spaces_and_multiple_unique_indices(upload):
    _, path, _ = upload
    other = path.with_name("another note.md")
    other.write_text("second")
    blocks = [dict(type="text", text=f"Read\n{path}\nPath: {other}\n{path}"),
              dict(type="resource", resource=dict(uri="mem://note", text="inline"))]
    captures = resolve(upload, blocks=blocks)
    assert len(captures) == 3
    assert len({c.block_index for c in captures}) == 3
    assert {c.body for c in captures} == {b"inline", b"owner attachment body", b"second"}


def test_same_text_path_and_link_not_captured_twice(upload):
    _, path, _ = upload
    captures = resolve(upload, blocks=[dict(type="text", text=str(path)),
                                      dict(type="resource_link", uri=path.as_uri())])
    assert len(captures) == 1 and captures[0].block_index == 1


def test_outside_uploads_not_opened_and_diagnostic_retained(upload, monkeypatch):
    project, _, _ = upload
    outside = project / "private.txt"
    outside.write_text("not an upload")

    def forbidden(*args, **kwargs):
        pytest.fail("Outside-upload path must not be opened")

    monkeypatch.setattr(replay_store, "_open_resource_link", forbidden)
    capture, = resolve(upload, text=f"Read\n{outside}")
    assert capture.body is None and capture.reason == "outside_paseo_uploads"


@pytest.mark.parametrize("form", ["plain", "link"])
def test_symlink_escape_rejected(upload, form):
    project, path, _ = upload
    outside = project / "outside.md"
    outside.write_text("outside")
    escape = path.parent / "escape.md"
    escape.symlink_to(outside)
    blocks = [dict(type="resource_link", uri=escape.as_uri())] if form == "link" else None
    capture, = resolve(upload, text=str(escape), blocks=blocks)
    assert capture.availability == "inaccessible" and capture.reason == "symlink_escape"
    assert capture.body is None


def test_uploads_directory_symlink_rejected(upload):
    project, path, env = upload
    other_home = project / ".paseo"
    other_home.mkdir()
    (other_home / "uploads").symlink_to(path.parent.parent, target_is_directory=True)
    env["PASEO_HOME"] = str(other_home)
    alias = other_home / "uploads" / "upload_test" / path.name
    capture, = resolve(upload, text=str(alias))
    assert capture.reason == "symlink_escape" and capture.body is None


def test_intermediate_directory_swap_uses_fixed_root(upload, monkeypatch):
    project, path, _ = upload
    external = project / "external"
    external.mkdir()
    (external / path.name).write_text("external body")
    original = replay_store._open_resource_link

    def swap(raw):
        path.parent.rename(path.parent.with_name("saved"))
        path.parent.symlink_to(external, target_is_directory=True)
        return original(raw)

    monkeypatch.setattr(replay_store, "_open_resource_link", swap)
    capture, = resolve(upload)
    assert capture.reason == "symlink_escape" and capture.body is None


@pytest.mark.parametrize("kind", ["fifo", "directory", "hardlink"])
def test_non_regular_or_hardlinked_upload_rejected(upload, kind):
    _, path, _ = upload
    target = path.with_name(kind)
    if kind == "fifo":
        os.mkfifo(target)
    elif kind == "directory":
        target.mkdir()
    else:
        os.link(path, target)
    capture, = resolve(upload, text=str(target))
    assert capture.body is None
    assert capture.reason == ("multiple_hardlinks" if kind == "hardlink" else "not_a_regular_file")


@pytest.mark.parametrize("form", ["plain", "link"])
def test_oversize_upload_rejected(upload, monkeypatch, form):
    _, path, _ = upload
    monkeypatch.setattr(replay_store, "MAX_BLOCK_BYTES", 4)
    blocks = [dict(type="resource_link", uri=path.as_uri())] if form == "link" else None
    capture, = resolve(upload, blocks=blocks)
    assert capture.availability == "too_large" and capture.body is None


def test_total_attachment_budget_shared_with_embedded_blocks(upload, monkeypatch):
    _, path, _ = upload
    monkeypatch.setattr(replay_store, "MAX_PROMPT_ATTACHMENT_BYTES", 25)
    captures = resolve(upload, blocks=[dict(type="resource", resource=dict(text="123456")),
                                      dict(type="text", text=str(path))])
    assert captures[0].availability == "stored"
    assert captures[1].reason == "prompt_attachment_total_exceeds_10mib_cap"


def test_missing_upload_diagnostic(upload):
    _, path, _ = upload
    capture, = resolve(upload, text=str(path.with_name("missing.md")))
    assert capture.reason == "missing_upload" and capture.body is None


def test_launch_home_is_only_authority(upload):
    _, path, env = upload
    prod = env["PASEO_HOME"]
    dev = path.parents[3] / ".paseo-dev"
    dev_path = dev / "uploads" / "upload_dev" / path.name
    dev_path.parent.mkdir(parents=True)
    dev_path.write_text("dev body")
    env["PASEO_HOME"] = str(dev)
    assert resolve(upload)[0].reason == "outside_paseo_uploads"
    assert resolve(upload, text=str(dev_path))[0].body == b"dev body"
    env["PASEO_HOME"] = prod
    assert resolve(upload, text=str(dev_path))[0].reason == "outside_paseo_uploads"
    env["PASEO_HOME"] = "~/.paseo"
    assert resolve(upload)[0].availability == "stored"
    del env["PASEO_HOME"]
    assert resolve(upload)[0].reason == "paseo_home_unavailable"


def test_never_scan_embedded_attachment_contents(upload):
    _, path, _ = upload
    capture, = resolve(upload, blocks=[dict(type="resource", resource=dict(text=f"Read\n{path}"))])
    assert capture.kind == "resource" and capture.body == f"Read\n{path}".encode()


def test_worker_listing_paging_capsule_and_read_only_snapshot(upload):
    project, path, _ = upload
    before = {str(p): (p.stat().st_size, p.stat().st_mtime_ns, p.stat().st_mode)
              for p in path.parent.parent.rglob("*")}
    store = Store(project / ".cortex", project_root=project)
    staged = replay_store.stage_blocks(str(project), resolve(upload))
    result = bootstrap_task(store, SourceEvent(origin="acp:owner", event_id="upload-1"),
                            f"Read\n{path}", "owner-1", blocks=staged)
    coordinator = resolve_channel(store, result.channel_id, result.secret)
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        channel_id, secret = issue_channel_tx(
            db, result.task_id, "worker", current_owner_epoch(db, result.task_id),
            parent_channel_id=coordinator.channel_id, assignment_id="a_upload", attempt_id="xa_upload",
        )
        db.commit()
    worker = resolve_channel(store, channel_id, secret)
    entries = mcp_tools.dispatch(store, worker, "list_sources", {})["entries"]
    attachment, = [entry for entry in entries if entry["kind"] == "resource_link"]
    assert attachment["availability"] == "stored" and attachment["trust_label"] == "user_direct"
    ref = attachment["source_ref"]
    first = mcp_tools.dispatch(store, worker, "read_source", dict(source_ref=ref, limit=5))
    second = mcp_tools.dispatch(store, worker, "read_source", dict(source_ref=ref, offset=first["next_offset"]))
    assert first["chunk"] + second["chunk"] == "owner attachment body"
    assert first["sha256"] == hashlib.sha256(b"owner attachment body").hexdigest()
    assert first["origin"] == "acp:owner" and first["event_id"] == "upload-1"
    capsule = build_capsule(store, result.task_id, dict(role="worker", assignment_ref="a_upload", attempt_id="xa_upload"))
    assert capsule["sections"]["constraints"]["items"][0]["attachments"][0]["source_ref"] == ref
    after = {str(p): (p.stat().st_size, p.stat().st_mtime_ns, p.stat().st_mode)
             for p in path.parent.parent.rglob("*")}
    assert before == after
    path.unlink()
    assert mcp_tools.dispatch(store, worker, "read_source", dict(source_ref=ref))["chunk"] == "owner attachment body"


def test_agent_owner_prompt_uses_launch_environment(upload):
    project, path, env = upload
    agent = Agent(_CountingFakeRuntime(), environ=env)
    agent.bind_transport(_RecordingTransport())
    agent.initialized = True
    sid = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    text = f"Read\n{path}"
    agent._run_prompt(request_id=1, session_id=sid, session=agent.sessions[sid],
                      text=text, cancel_event=threading.Event(), raw_blocks=[dict(type="text", text=text)])
    store = agent._store_for(str(project))
    task_id = agent.sessions[sid]["task_id"]
    entries = replay_store.list_sources(store, task_id)["entries"]
    attachment, = [entry for entry in entries if entry["kind"] == "resource_link"]
    assert replay_store.read_source(store, task_id, attachment["source_ref"])["chunk"] == "owner attachment body"


def test_queued_owner_upload_is_durable_and_retry_keeps_one_snapshot(upload):
    from cortex_runtime.acp import sessions
    from cortex_runtime.orchestration.state import TaskMachine

    project, path, env = upload
    agent = Agent(_CountingFakeRuntime(), environ=env)
    agent.bind_transport(_RecordingTransport())
    agent.initialized = True
    sid = agent._session_new(dict(cwd=str(project), mcpServers=[]))["sessionId"]
    store = agent._store_for(str(project))
    task = bootstrap_task(store, SourceEvent(origin="acp:owner", event_id="first"),
                          "Inspect", "owner-1")
    session = agent.sessions[sid]
    session["task_id"], session["owner_epoch"] = task.task_id, task.owner_epoch
    session["active"] = threading.Event()
    sessions.bind_task(store, sid, task.task_id)
    TaskMachine(store).create(task.task_id, "Inspect")
    with store.connection() as db:
        db.execute("INSERT INTO task_orchestration (task_id,state,updated_at,requirement_revision) "
                   "VALUES (?,?,?,0)", (task.task_id, "running", "now"))
        db.commit()
    value = json.dumps(dict(clientMessageId="owner-upload",
                            content=[dict(type="text", text=f"Read\n{path}")]))
    agent._queue_paseo_message(2, sid, session, value)
    path.unlink()
    agent._queue_paseo_message(3, sid, session, value)
    attachments = [entry for entry in replay_store.list_sources(store, task.task_id)["entries"]
                   if entry["kind"] == "resource_link"]
    assert len(attachments) == 1
    assert replay_store.read_source(store, task.task_id, attachments[0]["source_ref"])["chunk"] == "owner attachment body"
    steering = TaskMachine(store).context(task.task_id)["steering"]
    assert len(steering) == 1
    assert steering[0]["attachments"][0]["source_ref"] == attachments[0]["source_ref"]


def test_rejected_upload_diagnostic_is_durable(upload):
    project, path, _ = upload
    missing = path.with_name("absent.md")
    store = Store(project / ".cortex", project_root=project)
    staged = replay_store.stage_blocks(str(project), resolve(upload, text=str(missing)))
    task = bootstrap_task(store, SourceEvent(origin="acp:owner", event_id="missing"),
                          str(missing), "owner-1", blocks=staged)
    entry, = [e for e in replay_store.list_sources(store, task.task_id)["entries"]
              if e["kind"] == "resource_link"]
    assert entry["reason"] == "missing_upload"
    read = replay_store.read_source(store, task.task_id, entry["source_ref"])
    assert read["reason"] == "missing_upload" and read["chunk"] == ""


def test_device_upload_never_read(upload, monkeypatch):
    import stat
    from types import SimpleNamespace

    _, path, _ = upload
    original = os.fstat

    def device(fd):
        info = original(fd)
        return SimpleNamespace(st_mode=stat.S_IFCHR, st_uid=info.st_uid, st_nlink=1)

    def forbidden(*args):
        pytest.fail("A device descriptor must never be read")

    monkeypatch.setattr(replay_store.os, "fstat", device)
    monkeypatch.setattr(replay_store.os, "read", forbidden)
    capture, = resolve(upload)
    assert capture.reason == "not_a_regular_file" and capture.body is None


def test_final_symlink_swap_rejected(upload, monkeypatch):
    _, path, _ = upload
    original = replay_store._open_resource_link
    other = path.with_name("other.md")
    other.write_text("other")

    def swap(raw):
        path.unlink()
        path.symlink_to(other)
        return original(raw)

    monkeypatch.setattr(replay_store, "_open_resource_link", swap)
    capture, = resolve(upload)
    assert capture.reason == "symlink_path" and capture.body is None


def test_uploads_root_swap_does_not_rebase_authority(upload, monkeypatch):
    project, path, _ = upload
    root = path.parent.parent
    external = project / "external"
    (external / path.parent.name).mkdir(parents=True)
    (external / path.parent.name / path.name).write_text("external")
    original = replay_store._open_resource_link

    def swap(raw):
        root.rename(root.with_name("saved"))
        root.symlink_to(external, target_is_directory=True)
        return original(raw)

    monkeypatch.setattr(replay_store, "_open_resource_link", swap)
    capture, = resolve(upload)
    assert capture.reason == "symlink_escape" and capture.body is None


def test_file_uri_of_other_launch_home_is_not_read(upload):
    project, path, _ = upload
    other = project.parent / "other-paseo" / "uploads" / "upload_other" / path.name
    other.parent.mkdir(parents=True)
    other.write_text("other home")
    capture, = resolve(upload, blocks=[dict(type="resource_link", uri=other.as_uri())])
    assert capture.availability == "reference_only" and capture.body is None
    assert capture.reason == "outside_allowed_attachment_roots"


def test_remote_reference_does_not_hide_owner_upload_path(upload):
    _, path, _ = upload
    captures = resolve(upload, blocks=[
        dict(type="text", text=str(path)),
        dict(type="resource_link", uri=f"https://example.invalid{path}"),
    ])
    assert [c.availability for c in captures] == ["reference_only", "stored"]


@pytest.mark.parametrize("form", ["plain", "link"])
def test_missing_upload_in_both_forms(upload, form):
    _, path, _ = upload
    missing = path.with_name("not-present")
    blocks = [dict(type="resource_link", uri=missing.as_uri())] if form == "link" else None
    capture, = resolve(upload, text=str(missing), blocks=blocks)
    assert capture.reason == "missing_upload" and capture.body is None


def test_binary_snapshot_uses_existing_bounded_source_encoding(upload):
    import base64

    project, path, _ = upload
    path.write_bytes(b"\xff\x00binary")
    store = Store(project / ".cortex", project_root=project)
    task = bootstrap_task(store, SourceEvent(origin="acp:owner", event_id="binary"),
                          str(path), "owner-1",
                          blocks=replay_store.stage_blocks(str(project), resolve(upload)))
    attachment, = [e for e in replay_store.list_sources(store, task.task_id)["entries"]
                   if e["kind"] == "resource_link"]
    read = replay_store.read_source(store, task.task_id, attachment["source_ref"], limit=2)
    assert read["encoding"] == "base64"
    assert base64.b64decode(read["chunk"]) == b"\xff\x00"
    assert read["next_offset"] == 2


def test_exact_attachment_size_cap_is_stored(upload, monkeypatch):
    _, path, _ = upload
    monkeypatch.setattr(replay_store, "MAX_BLOCK_BYTES", path.stat().st_size)
    assert resolve(upload)[0].availability == "stored"
