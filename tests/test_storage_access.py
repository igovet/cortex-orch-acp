"""StorageAccess path-scoped classification regressions.

Classification never grants access by itself; the second-thread draft
ownership case below reuses the real MCP path (via ``storage_fixture``) to
show the actual per-draft SQL ``owner_thread_id`` boundary that classification
documents but does not replace.
"""
import os
from pathlib import Path

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.project_storage import StorageAccess
import sys
sys.path.insert(0, os.path.dirname(__file__))
from storage_fixture import call_store


def _project(tmp_path):
    project = tmp_path / "project"; project.mkdir()
    store = project / ".cortex"; store.mkdir(mode=0o700)
    (store / "draft-reports").mkdir(mode=0o700)
    (store / "pipeline-drafts").mkdir(mode=0o700)
    (store / "draft-reports" / "d1.md").write_text("draft")
    (store / "pipeline-drafts" / "p1.md").write_text("pipeline draft")
    task = store / "t_0123456789ab"; task.mkdir(mode=0o700)
    (task / "pipeline.md").write_text("published pipeline")
    (store / "cortex.sqlite3").write_bytes(b"")
    return project, store


def test_own_draft_area_classification(tmp_path):
    project, store = _project(tmp_path)
    access = StorageAccess(project)
    assert access.classify(store / "draft-reports" / "d1.md") == "own_draft_area"
    assert access.classify(store / "pipeline-drafts" / "p1.md") == "own_draft_area"


def test_readable_publication_classification(tmp_path):
    project, store = _project(tmp_path)
    access = StorageAccess(project)
    assert access.classify(store / "t_0123456789ab" / "pipeline.md") == "readable_publication"


def test_runtime_metadata_classification(tmp_path):
    project, store = _project(tmp_path)
    access = StorageAccess(project)
    assert access.classify(store / "cortex.sqlite3") == "runtime_metadata"
    assert access.classify(store / ".access.lock") == "runtime_metadata"
    assert access.classify(store / "cortex.sqlite3-wal") == "runtime_metadata"


def test_outside_store_classification(tmp_path):
    project, store = _project(tmp_path)
    access = StorageAccess(project)
    assert access.classify(project / "outside.md") == "outside_store"
    assert access.classify(Path("/tmp/unrelated.md")) == "outside_store"
    assert access.classify("relative/path.md") == "outside_store"


def test_traversal_component_is_rejected(tmp_path):
    project, store = _project(tmp_path)
    access = StorageAccess(project)
    traversal = store / "draft-reports" / ".." / ".." / "etc" / "passwd"
    assert access.classify(traversal) == "outside_store"


def test_symlink_within_store_is_rejected(tmp_path):
    project, store = _project(tmp_path)
    os.symlink("/etc", store / "evil_link")
    access = StorageAccess(project)
    assert access.classify(store / "evil_link" / "passwd") == "outside_store"


def test_require_writable_draft_rejects_non_draft_area(tmp_path):
    project, store = _project(tmp_path)
    access = StorageAccess(project)
    access.require_writable_draft(store / "draft-reports" / "d1.md")
    with pytest.raises(StoreError, match="project_context_conflict"):
        access.require_writable_draft(store / "t_0123456789ab" / "pipeline.md")
    with pytest.raises(StoreError, match="project_context_conflict"):
        access.require_writable_draft(store / "cortex.sqlite3")


def identifier():
    import uuid
    return str(uuid.uuid4())


def metadata(thread, parent=None):
    return {"threadId": thread,
            "x-codex-turn-metadata": ({"parent_thread_id": parent} if parent else {})}


def dispatch(server, name, arguments, thread, parent=None):
    return server.dispatch("tools/call", {
        "name": name,
        "arguments": dict(arguments),
        "_meta": metadata(thread, parent),
    })


def successful(result):
    assert not result["isError"], result
    return result["structuredContent"]


def error_code(result):
    assert result["isError"], result
    import json
    return json.loads(result["content"][0]["text"])["error"]


def test_second_thread_cannot_write_or_read_another_threads_draft(tmp_path):
    """Reuses the real storage path: classification alone never grants draft access.

    ``StorageAccess`` would classify both drafts' paths as ``own_draft_area``
    (same shape), but the actual store enforces per-draft
    ``owner_thread_id`` ownership; a second, unrelated thread (here, a
    second task's thread in the same project store) must still be rejected
    when it tries to act on the first thread's draft id.
    """
    from cortex_runtime.store import Store
    from storage_fixture import call_store

    project = tmp_path / "project"; project.mkdir()
    store = Store(project / ".cortex", project_root=project)
    owner_task = call_store(store, "create_task", dict(
        project_root=str(project), request="Owner request.", request_key="owner-task"))["task_id"]
    stranger_task = call_store(store, "create_task", dict(
        project_root=str(project), request="Stranger request.", request_key="stranger-task"))["task_id"]

    draft = call_store(store, "create_draft", dict(
        task_id=owner_task, template="general", request_key="owner-draft"))

    # A different, unrelated task's thread cannot publish against the owner's draft id.
    with pytest.raises(StoreError):
        call_store(store, "write_report", dict(
            task_id=stranger_task, title="Hijack", summary="Should not be permitted.",
            author="stranger", draft_id=draft["draft_id"], request_key="stranger-write"))
