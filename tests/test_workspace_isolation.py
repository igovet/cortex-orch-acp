"""T025: isolated per-attempt workspace snapshots and the serialized
shared-root fallback."""
import os
import stat
import threading
import time
import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.contracts import StoreError
from cortex_runtime.delegation import workspace_isolation
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _project(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    return project


def _task(store):
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build.", "owner-1")
    return result.task_id


def _bind_attempt(store, task_id, attempt_id):
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) "
            "VALUES (?,?,?,?,?,?,?,?,?,?)",
            (f"idem-{attempt_id}", task_id, f"a-{attempt_id}", attempt_id, "backend-dev", "workspace",
             "in_progress", 1, store._now(), store._now()),
        )
        db.commit()
    return f"idem-{attempt_id}"


def test_isolated_snapshot_excludes_git_and_cortex_and_preserves_dirty_files(tmp_path):
    project = _project(tmp_path)
    (project / ".git").mkdir()
    (project / ".git" / "HEAD").write_text("ref: refs/heads/main\n")
    (project / "tracked.txt").write_text("tracked\n")
    (project / "dirty-uncommitted.txt").write_text("never committed by the user\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    _bind_attempt(store, task_id, "xa_dirty1")

    provisioner = workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(store, str(project))
    binding_id = provisioner.create("xa_dirty1")
    row = provisioner.row_for_binding(binding_id)
    assert row["mode"] == "isolated_snapshot"
    root = provisioner.root_for(binding_id)
    base = provisioner.base_root_for(binding_id)

    assert not os.path.exists(os.path.join(root, ".git"))
    assert not os.path.exists(os.path.join(root, ".cortex"))
    assert not os.path.exists(os.path.join(base, ".git"))
    assert not os.path.exists(os.path.join(base, ".cortex"))
    assert open(os.path.join(root, "dirty-uncommitted.txt")).read() == "never committed by the user\n"
    assert open(os.path.join(base, "dirty-uncommitted.txt")).read() == "never committed by the user\n"
    # The real project tree itself is completely untouched (no reset/stash).
    assert open(project / "dirty-uncommitted.txt").read() == "never committed by the user\n"
    assert (project / ".git" / "HEAD").exists()


def test_two_workers_isolated_workspaces_cannot_observe_each_other(tmp_path):
    project = _project(tmp_path)
    (project / "shared.txt").write_text("original\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    _bind_attempt(store, task_id, "xa_w1")
    _bind_attempt(store, task_id, "xa_w2")

    provisioner = workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(store, str(project))
    b1 = provisioner.create("xa_w1")
    b2 = provisioner.create("xa_w2")
    r1 = provisioner.root_for(b1)
    r2 = provisioner.root_for(b2)
    assert r1 != r2

    with open(os.path.join(r1, "shared.txt"), "w") as f:
        f.write("worker-1 wrote this\n")

    # worker 2's own copy is untouched by worker 1's write, and the real
    # project tree is untouched by either until an explicit integration.
    assert open(os.path.join(r2, "shared.txt")).read() == "original\n"
    assert open(project / "shared.txt").read() == "original\n"


def test_fallback_to_serialized_when_snapshot_unavailable(tmp_path, monkeypatch):
    project = _project(tmp_path)
    (project / "a.txt").write_text("a\n")
    (project / "b.txt").write_text("b\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    _bind_attempt(store, task_id, "xa_big")

    monkeypatch.setattr(workspace_isolation, "_MAX_ENTRIES", 1)
    provisioner = workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(store, str(project))
    binding_id = provisioner.create("xa_big")
    row = provisioner.row_for_binding(binding_id)
    assert row["mode"] == "serialized_shared_root"
    assert row["base_kind"] == "unavailable"
    assert provisioner.lookup("xa_big") == "present"
    assert provisioner.root_for(binding_id) == str(project)


def test_serialized_writer_lock_serializes_concurrent_writers(tmp_path):
    project = _project(tmp_path)
    store = Store(project / ".cortex", project_root=str(project))
    fallback = workspace_isolation.SerializedSharedRootWorkspaceProvisioner(str(project))

    order = []
    barrier_started = threading.Event()

    def writer(name, delay):
        with fallback.writer_lock():
            order.append(f"{name}-start")
            if name == "A":
                barrier_started.set()
            time.sleep(delay)
            order.append(f"{name}-end")

    t1 = threading.Thread(target=writer, args=("A", 0.2))
    t2 = threading.Thread(target=writer, args=("B", 0.0))
    t1.start()
    barrier_started.wait(timeout=2)
    t2.start()
    t1.join(timeout=5)
    t2.join(timeout=5)
    assert order == ["A-start", "A-end", "B-start", "B-end"], order


def test_writer_lock_times_out_visibly_when_held(tmp_path):
    project = _project(tmp_path)
    fallback = workspace_isolation.SerializedSharedRootWorkspaceProvisioner(str(project))
    held = threading.Event()
    release = threading.Event()

    def holder():
        with fallback.writer_lock():
            held.set()
            release.wait(timeout=5)

    t = threading.Thread(target=holder)
    t.start()
    held.wait(timeout=2)
    try:
        raised = False
        try:
            with fallback.writer_lock(timeout=0.05):
                pass
        except StoreError as error:
            raised = True
            assert str(error) == "workspace_writer_busy"
        assert raised
    finally:
        release.set()
        t.join(timeout=5)


def test_cleanup_binding_removes_only_owned_snapshot_dirs(tmp_path):
    project = _project(tmp_path)
    (project / "a.txt").write_text("a\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    _bind_attempt(store, task_id, "xa_clean")
    provisioner = workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(store, str(project))
    binding_id = provisioner.create("xa_clean")
    root = provisioner.root_for(binding_id)
    base = provisioner.base_root_for(binding_id)
    assert os.path.isdir(root) and os.path.isdir(base)

    ok = provisioner.cleanup_binding(binding_id, reason="test")
    assert ok
    assert not os.path.exists(root)
    assert not os.path.exists(base)
    assert provisioner.lookup("xa_clean") == "absent"
    # The real project tree is never touched by cleanup.
    assert (project / "a.txt").exists()
    # A second cleanup call is a safe no-op (already cleaned up).
    assert provisioner.cleanup_binding(binding_id, reason="test-again") is False


def test_resolve_assignment_root_maps_through_provisioning_row(tmp_path):
    project = _project(tmp_path)
    (project / "a.txt").write_text("a\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    assignment_ref = _bind_attempt(store, task_id, "xa_map")
    provisioner = workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(store, str(project))
    binding_id = provisioner.create("xa_map")
    resolved = workspace_isolation.resolve_assignment_root(store, task_id, assignment_ref)
    assert resolved == provisioner.root_for(binding_id)
    assert workspace_isolation.resolve_assignment_root(store, task_id, "no-such-assignment") is None
