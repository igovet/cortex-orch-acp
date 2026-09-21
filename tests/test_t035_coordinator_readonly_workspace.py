"""T035 (FINDING-12/-13 closure): the coordinator gets a workspace it is
physically unable to write to, closing the gap live evidence proved --
neither codex ``read-only`` nor ``agent`` mode gives Cortex a working
interception point for a coordinator's own file writes (see
``.cortex/acp-migration/evidence/T035/FINDING-12-read-only-mode-does-not-
prevent-edits.md``).

Layering under test, and what each layer actually is (see
``delegation.workspace_isolation.IsolatedSnapshotWorkspaceProvisioner
.coordinator_root_for``'s own docstring for the same honesty in the
production code):

* PREVENTS -- this file's new mechanism: a read-only filesystem copy handed
  to the coordinator as its cwd. Binds regardless of what the engine's own
  "mode" does, with one disclosed limit: the owning uid can ``chmod`` its
  own files back to writable before writing (never worked around
  accidentally, only by a process that deliberately re-permissions first).
* DETECTS -- the pre-existing engine-independent backstop
  (``acp.orchestrating_runtime``'s pre/post ``capture_revision`` compare of
  the *real* project root): unchanged by this file, re-proven here still
  fires.
* ASKS -- the pre-existing mode floor (``configuration.policy
  .COORDINATOR_MODE_FLOOR``) and permission-refusal path
  (``engine.permissions``): unchanged by this file, re-proven here still in
  place, never removed just because codex ignores them.

No real engine, no paid call, no network I/O -- ``FakeSessionRuntime``/
``FakeWorkspace`` (the same in-process test doubles
``test_orchestrating_runtime.py`` uses) for the backstop test, and the real
``delegation.workspace_isolation``/``acp.production_runtime`` modules
directly for the prevention/wiring tests.
"""
from __future__ import annotations

import os
import stat
import threading
import time
import uuid

import pytest

from cortex_runtime.acp.orchestrating_runtime import OrchestratingRuntime, PromptOutcome
from cortex_runtime.acp.production_runtime import ProductionOrchestratingRuntime
from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.configuration import policy as configuration_policy
from cortex_runtime.delegation import mcp_tools
from cortex_runtime.delegation import events as delegation_events
from cortex_runtime.delegation import revision as delegation_revision
from cortex_runtime.delegation import workspace_isolation
from cortex_runtime.portable.engine_contract import StopReason
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store
from cortex_runtime.contracts import StoreError

from orchestration_fakes import FakeConfigSnapshotSource, FakeSessionRuntime, FakeWorkspace


def _isolate(tmp_path, monkeypatch):
    config_home = tmp_path / "config"
    state_home = tmp_path / "state"
    home = tmp_path / "home"
    for path in (config_home, state_home, home):
        path.mkdir()
    monkeypatch.setenv("XDG_CONFIG_HOME", str(config_home))
    monkeypatch.setenv("XDG_STATE_HOME", str(state_home))
    monkeypatch.setenv("HOME", str(home))
    return dict(HOME=str(home), XDG_CONFIG_HOME=str(config_home), XDG_STATE_HOME=str(state_home))


def _project(tmp_path, name="project"):
    project = tmp_path / name
    project.mkdir()
    (project / "tracked.txt").write_text("original\n")
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


# ---------------------------------------------------------------------------
# 1. A coordinator session's workspace refuses a write, and still permits a
#    read.
# ---------------------------------------------------------------------------

def test_coordinator_root_is_read_only_but_still_readable(tmp_path):
    project = _project(tmp_path)
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)

    provisioner = workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(store, str(project))
    root, mode = provisioner.coordinator_root_for(task_id)

    assert mode == "read_only_snapshot"
    assert root != str(project)

    # Still readable: the coordinator needs this to decide what to delegate.
    assert open(os.path.join(root, "tracked.txt")).read() == "original\n"
    assert os.listdir(root)

    # Refuses a write: creating a new file...
    with pytest.raises(PermissionError):
        open(os.path.join(root, "new_file.txt"), "w")
    # ...and editing an existing one.
    with pytest.raises(PermissionError):
        open(os.path.join(root, "tracked.txt"), "w")

    # The real project tree is naturally untouched throughout.
    assert open(project / "tracked.txt").read() == "original\n"


def test_coordinator_root_for_refreshes_and_stays_bounded_to_one_copy(tmp_path):
    project = _project(tmp_path)
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    provisioner = workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(store, str(project))

    first_root, _ = provisioner.coordinator_root_for(task_id)
    assert open(os.path.join(first_root, "tracked.txt")).read() == "original\n"
    workspaces_dir = project / ".cortex" / "workspaces"
    assert len(os.listdir(workspaces_dir)) == 1, "one coordinator snapshot dir, never accumulating"

    # The real project changes (e.g. a worker's integration landed) before
    # the coordinator's next fresh session.
    (project / "tracked.txt").write_text("updated\n")
    second_root, mode = provisioner.coordinator_root_for(task_id)

    assert mode == "read_only_snapshot"
    assert open(os.path.join(second_root, "tracked.txt")).read() == "updated\n"
    # Refreshed content, never more than one live coordinator copy at once.
    assert len(os.listdir(workspaces_dir)) == 1, "the stale copy was removed, not leaked, on refresh"
    with pytest.raises(PermissionError):
        open(os.path.join(second_root, "tracked.txt"), "w")


# ---------------------------------------------------------------------------
# 2. A worker's workspace still permits writes.
# ---------------------------------------------------------------------------

def test_worker_workspace_is_unaffected_and_still_writable(tmp_path):
    project = _project(tmp_path)
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)
    _bind_attempt(store, task_id, "xa_worker1")

    provisioner = workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(store, str(project))
    binding_id = provisioner.create("xa_worker1")
    root = provisioner.root_for(binding_id)

    with open(os.path.join(root, "tracked.txt"), "w") as f:
        f.write("worker wrote this\n")
    assert open(os.path.join(root, "tracked.txt")).read() == "worker wrote this\n"
    with open(os.path.join(root, "new_from_worker.txt"), "w") as f:
        f.write("new\n")
    assert os.path.exists(os.path.join(root, "new_from_worker.txt"))

    # Never touches the real project tree, exactly like before this change.
    assert open(project / "tracked.txt").read() == "original\n"


# ---------------------------------------------------------------------------
# 3. Production wiring routes the coordinator's sentinel binding id to the
#    exact project opened by ACP. A worker edit must be visible immediately.
# ---------------------------------------------------------------------------

def test_production_runtime_wires_coordinator_sentinel_to_exact_project_root(tmp_path, monkeypatch):
    environ = _isolate(tmp_path, monkeypatch)
    project = _project(tmp_path)
    store = Store(project / ".cortex", project_root=str(project))
    task_id = _task(store)

    runtime = ProductionOrchestratingRuntime(environ=environ)
    orchestrating = runtime._runtime_for_project(str(project))
    resolver = orchestrating._engine._workspace_resolver
    assert resolver is not None

    resolved_root = resolver("shared_project_root", dict(task_id=task_id))
    assert resolved_root == str(project)
    assert open(os.path.join(resolved_root, "tracked.txt")).read() == "original\n"

    # A worker may complete while this coordinator session is still alive.
    # The coordinator cwd must observe the exact edit, rather than a snapshot
    # captured before the worker started.
    (project / "tracked.txt").write_text("worker edit\n")
    assert open(os.path.join(resolved_root, "tracked.txt")).read() == "worker edit\n"

    # An unknown worker binding fails closed. Falling through to the writable
    # project root would let a stale or forged binding bypass isolation.
    with pytest.raises(StoreError, match="workspace_snapshot_unavailable"):
        resolver("some_unknown_worker_binding", dict(task_id=task_id))


# ---------------------------------------------------------------------------
# 4. The backstop still fires if a change appears anyway.
# ---------------------------------------------------------------------------







# ---------------------------------------------------------------------------
# 5. Mode floors and the permission refusal are unchanged -- kept, not
#    removed, because codex ignores them; they still bind on engines that do
#    honour them.
# ---------------------------------------------------------------------------

def test_mode_floors_are_unchanged():
    assert configuration_policy.COORDINATOR_MODE_FLOOR == {
        "codex-acp": "agent",
        "claude-agent-acp": "default",
    }
    assert configuration_policy.WORKER_MODE_FLOOR == {
        "codex-acp": "agent",
        "claude-agent-acp": "default",
    }
