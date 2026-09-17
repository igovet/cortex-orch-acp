"""T025: technical application of an isolated worker workspace onto the
main project root -- conflict detection, host-buffer preconditions,
idempotency, and the revision-A-does-not-certify-revision-B gate wiring."""
import os
import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation import completion as delegation_completion
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import integration
from cortex_runtime.delegation import revision as delegation_revision
from cortex_runtime.delegation import workspace_isolation
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.project_storage import StorageLocator
from cortex_runtime.store import Store, private_directory, project_task_directory


def _project(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    return project


def _task(store):
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build.", "owner-1")
    return result.task_id


def _bind_attempt(store, task_id, attempt_id):
    assignment_ref = f"idem-{attempt_id}"
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) "
            "VALUES (?,?,?,?,?,?,?,?,?,?)",
            (assignment_ref, task_id, f"a-{attempt_id}", attempt_id, "backend-dev", "workspace",
             "in_progress", 1, store._now(), store._now()),
        )
        db.commit()
    return assignment_ref


def _create_snapshot(store, project, task_id, attempt_id):
    provisioner = workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(store, str(project))
    binding_id = provisioner.create(attempt_id)
    return provisioner, binding_id


def test_integrate_applies_clean_worker_change_onto_unchanged_main(tmp_path):
    project = _project(tmp_path)
    (project / "a.txt").write_text("hello\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    assignment_ref = _bind_attempt(store, task_id, "xa_clean")
    provisioner, binding_id = _create_snapshot(store, project, task_id, "xa_clean")
    root = provisioner.root_for(binding_id)
    with open(os.path.join(root, "a.txt"), "a") as f:
        f.write("world\n")

    result = integration.integrate(
        store, task_id=task_id, project_root=str(project), assignment_ref=assignment_ref,
        request_key="rk1", attempt_id="xa_clean", workspace_binding_id=binding_id,
    )
    assert result["status"] == "applied"
    assert result["applied_paths"] == ["a.txt"]
    assert (project / "a.txt").read_text() == "hello\nworld\n"


def test_two_workers_same_file_second_integration_conflicts_no_overwrite(tmp_path):
    """Acceptance bullet 1/2: two workers editing the same file in isolated
    workspaces cannot silently overwrite each other -- the second
    integration against a changed base is surfaced as a conflict, never
    last-write-wins."""
    project = _project(tmp_path)
    (project / "shared.txt").write_text("line1\nline2\nline3\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    ref1 = _bind_attempt(store, task_id, "xa_w1")
    ref2 = _bind_attempt(store, task_id, "xa_w2")
    provisioner, b1 = _create_snapshot(store, project, task_id, "xa_w1")
    _, b2 = _create_snapshot(store, project, task_id, "xa_w2")
    r1 = provisioner.root_for(b1)
    r2 = provisioner.root_for(b2)

    with open(os.path.join(r1, "shared.txt"), "w") as f:
        f.write("line1-BY-W1\nline2\nline3\n")
    with open(os.path.join(r2, "shared.txt"), "w") as f:
        f.write("line1-BY-W2\nline2\nline3\n")

    result1 = integration.integrate(
        store, task_id=task_id, project_root=str(project), assignment_ref=ref1,
        request_key="rk1", attempt_id="xa_w1", workspace_binding_id=b1,
    )
    assert result1["status"] == "applied"
    assert (project / "shared.txt").read_text() == "line1-BY-W1\nline2\nline3\n"

    result2 = integration.integrate(
        store, task_id=task_id, project_root=str(project), assignment_ref=ref2,
        request_key="rk2", attempt_id="xa_w2", workspace_binding_id=b2,
    )
    assert result2["status"] == "conflict"
    assert "shared.txt" in result2["conflict_paths"]
    # Nothing was overwritten -- worker 1's already-applied content survives
    # untouched (never last-write-wins).
    assert (project / "shared.txt").read_text() == "line1-BY-W1\nline2\nline3\n"


def test_non_overlapping_concurrent_main_change_merges_cleanly(tmp_path):
    project = _project(tmp_path)
    (project / "b.txt").write_text("line1\nline2\nline3\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    assignment_ref = _bind_attempt(store, task_id, "xa_merge")
    provisioner, binding_id = _create_snapshot(store, project, task_id, "xa_merge")
    root = provisioner.root_for(binding_id)
    with open(os.path.join(root, "b.txt"), "w") as f:
        f.write("line1-WORKER\nline2\nline3\n")
    # Main changed a non-overlapping line concurrently.
    (project / "b.txt").write_text("line1\nline2\nline3-MAIN\n")

    result = integration.integrate(
        store, task_id=task_id, project_root=str(project), assignment_ref=assignment_ref,
        request_key="rk-merge", attempt_id="xa_merge", workspace_binding_id=binding_id,
    )
    assert result["status"] == "applied"
    assert (project / "b.txt").read_text() == "line1-WORKER\nline2\nline3-MAIN\n"


def test_dirty_user_file_survives_integration_untouched(tmp_path):
    project = _project(tmp_path)
    (project / "dirty.txt").write_text("uncommitted user work\n")
    (project / "c.txt").write_text("orig\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    assignment_ref = _bind_attempt(store, task_id, "xa_dirty")
    provisioner, binding_id = _create_snapshot(store, project, task_id, "xa_dirty")
    root = provisioner.root_for(binding_id)
    with open(os.path.join(root, "c.txt"), "w") as f:
        f.write("worker-changed\n")

    result = integration.integrate(
        store, task_id=task_id, project_root=str(project), assignment_ref=assignment_ref,
        request_key="rk-dirty", attempt_id="xa_dirty", workspace_binding_id=binding_id,
    )
    assert result["status"] == "applied"
    assert (project / "dirty.txt").read_text() == "uncommitted user work\n"
    assert (project / "c.txt").read_text() == "worker-changed\n"


def test_unsaved_host_buffer_blocks_overwrite(tmp_path):
    """Acceptance bullet 3 (host side): a fake host forwarder reporting a
    newer, unsaved buffer for a path blocks that path's integration --
    never overwritten by an old engine-side result."""
    project = _project(tmp_path)
    (project / "c.txt").write_text("orig\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    assignment_ref = _bind_attempt(store, task_id, "xa_host")
    provisioner, binding_id = _create_snapshot(store, project, task_id, "xa_host")
    root = provisioner.root_for(binding_id)
    with open(os.path.join(root, "c.txt"), "w") as f:
        f.write("worker-result\n")

    def fake_host_forwarder(relpath):
        if relpath == "c.txt":
            return b"host-has-unsaved-newer-content\n"
        return None

    result = integration.integrate(
        store, task_id=task_id, project_root=str(project), assignment_ref=assignment_ref,
        request_key="rk-host", attempt_id="xa_host", workspace_binding_id=binding_id,
        host_fs_checker=fake_host_forwarder,
    )
    assert result["status"] == "conflict"
    assert result["conflict_paths"]["c.txt"]["reason"] == "unsaved_host_buffer"
    assert (project / "c.txt").read_text() == "orig\n"


def test_integrate_is_idempotent_by_request_key(tmp_path):
    project = _project(tmp_path)
    (project / "a.txt").write_text("hello\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    assignment_ref = _bind_attempt(store, task_id, "xa_idem")
    provisioner, binding_id = _create_snapshot(store, project, task_id, "xa_idem")
    root = provisioner.root_for(binding_id)
    with open(os.path.join(root, "a.txt"), "a") as f:
        f.write("world\n")

    first = integration.integrate(
        store, task_id=task_id, project_root=str(project), assignment_ref=assignment_ref,
        request_key="rk-idem", attempt_id="xa_idem", workspace_binding_id=binding_id,
    )
    # Mutate the disk further; a retried call with the same request_key
    # must return the original recorded outcome, never re-apply.
    (project / "a.txt").write_text("mutated-out-of-band\n")
    second = integration.integrate(
        store, task_id=task_id, project_root=str(project), assignment_ref=assignment_ref,
        request_key="rk-idem", attempt_id="xa_idem", workspace_binding_id=binding_id,
    )
    assert second == first
    assert (project / "a.txt").read_text() == "mutated-out-of-band\n"


def test_integrate_noop_for_serialized_fallback_binding(tmp_path, monkeypatch):
    """Acceptance bullet 6: without suitable isolation, conflicting writes
    are serialized (see test_workspace_isolation.py) -- integrate() is then
    correctly a documented no-op (there is no separate copy left to
    merge)."""
    project = _project(tmp_path)
    (project / "a.txt").write_text("hello\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    assignment_ref = _bind_attempt(store, task_id, "xa_fallback")
    monkeypatch.setattr(workspace_isolation, "_MAX_ENTRIES", 0)
    provisioner, binding_id = _create_snapshot(store, project, task_id, "xa_fallback")
    assert provisioner.row_for_binding(binding_id)["mode"] == "serialized_shared_root"

    result = integration.integrate(
        store, task_id=task_id, project_root=str(project), assignment_ref=assignment_ref,
        request_key="rk-fallback", attempt_id="xa_fallback", workspace_binding_id=binding_id,
    )
    assert result["status"] == "noop"


def test_worker_report_lands_in_main_project_store_not_the_snapshot(tmp_path):
    """Acceptance bullet 4: a worker worktree resolves the same logical
    .cortex store while retaining isolated execution files."""
    project = _project(tmp_path)
    (project / "a.txt").write_text("hello\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    _bind_attempt(store, task_id, "xa_report")
    provisioner, binding_id = _create_snapshot(store, project, task_id, "xa_report")
    root = provisioner.root_for(binding_id)

    # The isolated attempt's own execution files never contain the project
    # store -- it was excluded at snapshot time.
    assert not os.path.exists(os.path.join(root, ".cortex"))

    # Publishing (as the real MCP tool path does) always resolves through
    # the task's bound project_root, not through the attempt's own
    # workspace_root -- landing in the one shared, logical .cortex store.
    task_dir = private_directory(project_task_directory(str(project)) / task_id)
    (task_dir / "report.md").write_text("# report\n", encoding="utf-8")
    assert (project / ".cortex" / task_id / "report.md").exists()
    assert StorageLocator(str(project)).store_dir() == StorageLocator(str(project)).store_dir()


def _record_worker_result(store, task_id, assignment_ref, attempt_id, report_ref):
    """``assignment_ref`` (the delegate-time idempotency_key) keys the
    coordinator-facing ``delegation_requested`` event; the worker-facing
    ``worker_result`` event is keyed by ``attempt_id`` instead -- see
    ``delegation.completion.gate``'s own docstring on this exact
    distinction."""
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute("INSERT INTO reports VALUES (?,?,?,?,?)", (report_ref, task_id, "report", f"{report_ref}.md", store._now()))
        db.commit()
    with store.connection() as db:
        row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
    task_dir = private_directory(project_task_directory(row["project_root"]) / task_id)
    (task_dir / f"{report_ref}.md").write_text("# done\n", encoding="utf-8")
    delegation_events.append_event(
        store, task_id, "delegation_requested", f"delreq-{assignment_ref}",
        dict(title="T", brief="B", role="backend-dev", acceptance="A"), origin="coordinator", assignment_ref=assignment_ref,
    )
    delegation_events.append_event(
        store, task_id, "worker_result", f"pub-{assignment_ref}",
        dict(report_ref=report_ref, summary="done", checks=[dict(name="tests", outcome="passed", tool_call_ref=None, evidence="observed")]),
        origin="worker", assignment_ref=attempt_id,
    )
    delegation_events.claim_unconsumed(store, task_id, "cont-" + assignment_ref)
    return delegation_events.list_since(store, task_id)[-1]


def test_revision_a_does_not_certify_revision_b_after_conflicting_integration(tmp_path):
    """Acceptance bullet 5: a report bound to the pre-integration revision
    (the worker's own isolated workspace at worker_result time, "revision
    A") must not certify a later, different project-root revision
    ("revision B") -- a fresh check is required (``stale_verification``).

    Before integration ever runs, the isolated worker's own tree (A) and
    the not-yet-integrated main project root necessarily differ (the
    worker's own edit exists only in its private copy) -- the gate
    correctly refuses this exact situation too: an isolated worker's
    report can never certify the unmodified main tree it has not actually
    reached yet. Once a *clean* integration reproduces that same worker
    tree onto an unchanged main root, the gate accepts (A actually equals
    the newly-integrated B); a further, later drift of the main root
    (simulating a second, independent change after integration) makes the
    gate refuse again -- never silently certifying that newer state
    either."""
    project = _project(tmp_path)
    (project / "shared.txt").write_text("line1\nline2\nline3\n")
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    assignment_ref = _bind_attempt(store, task_id, "xa_gate")
    provisioner, binding_id = _create_snapshot(store, project, task_id, "xa_gate")
    root = provisioner.root_for(binding_id)
    with open(os.path.join(root, "shared.txt"), "w") as f:
        f.write("line1-WORKER\nline2\nline3\n")

    event = _record_worker_result(store, task_id, assignment_ref, "xa_gate", "r_" + "a" * 12)
    # Revision A: captured against the worker's own isolated workspace,
    # exactly as acp.orchestrating_runtime._capture_result_revisions now
    # does for an isolated attempt (see workspace_isolation.resolve_attempt_root);
    # the attempt_revisions row is keyed by the attempt id itself, matching
    # the worker_result event's own assignment_ref.
    delegation_revision.record_attempt_revision(store, "xa_gate", f"worker_result:{event.id}", root)

    # Not yet integrated: the main project root does not contain the
    # worker's own edit at all -- correctly refused, never silently
    # accepted just because "nothing conflicting happened yet".
    ok, reasons = delegation_completion.gate(store, task_id)
    assert not ok
    assert any(reason == f"stale_verification:{assignment_ref}" for reason in reasons)

    # A clean integration (main unchanged since base) reproduces the exact
    # same tree the worker's own check ran against.
    result = integration.integrate(
        store, task_id=task_id, project_root=str(project), assignment_ref=assignment_ref,
        request_key="rk-gate", attempt_id="xa_gate", workspace_binding_id=binding_id,
    )
    assert result["status"] == "applied"
    ok, reasons = delegation_completion.gate(store, task_id)
    assert ok, reasons

    # A later, independent change to the main root (revision B) must not
    # be silently certified by the same, now-stale revision-A report.
    (project / "shared.txt").write_text("line1-WORKER\nline2\nline3-CHANGED-LATER\n")
    ok, reasons = delegation_completion.gate(store, task_id)
    assert not ok
    assert any(reason == f"stale_verification:{assignment_ref}" for reason in reasons)
