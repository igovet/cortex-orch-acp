"""T025 (lead follow-up item 2): verify a worker ``ResourceBinding`` rooted
at an isolated snapshot's own ``work`` directory (which lives *under* the
main project's ``.cortex/workspaces/<attempt>/work``) can still read/write
its own files through ``FsBridge``/``TerminalBridge`` -- both bridges check
containment *relative to* ``workspace_root``, never by rejecting any
ancestor path component named ``.cortex`` -- and that the same binding
cannot reach the main project's real store files (a sibling of its own
workspace root, not a path under it)."""
import os
import uuid

from cortex_runtime.bootstrap import bootstrap_task
from cortex_runtime.delegation.workspace_isolation import IsolatedSnapshotWorkspaceProvisioner
from cortex_runtime.engine.fs_bridge import FsBridge, FsBridgeError
from cortex_runtime.engine.resource_binding import ResourceBinding
from cortex_runtime.engine.terminal_bridge import TerminalBridge, TerminalBridgeError
from cortex_runtime.portable.identity import SourceEvent
from cortex_runtime.store import Store


def _setup(tmp_path):
    project = tmp_path / "project"
    project.mkdir()
    (project / "a.txt").write_text("hi\n")
    store = Store(project / ".cortex", project_root=str(project))
    result = bootstrap_task(store, SourceEvent(origin="cli", event_id=str(uuid.uuid4())), "Build.", "owner-1")
    task_id = result.task_id
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        db.execute(
            "INSERT INTO delegation_provisioning "
            "(idempotency_key, task_id, assignment_id, attempt_id, role_id, step, status, owner_epoch, created_at, updated_at) "
            "VALUES (?,?,?,?,?,?,?,?,?,?)",
            ("idem1", task_id, "a1", "xa1", "backend-dev", "workspace", "in_progress", 1, store._now(), store._now()),
        )
        db.commit()
    provisioner = IsolatedSnapshotWorkspaceProvisioner(store, str(project))
    binding_id = provisioner.create("xa1")
    root = provisioner.root_for(binding_id)
    binding = ResourceBinding(
        engine_instance_id="e1", engine_session_id="s1", context_generation_id="g1",
        assignment_id="a1", attempt_id="xa1", workspace_binding_id=binding_id,
        workspace_root=root, ownership_epoch="ep1", role="worker",
    )
    return project, store, root, binding


def test_snapshot_work_dir_is_under_the_project_cortex_directory(tmp_path):
    """Confirms the actual (non-obvious) location this test protects: the
    snapshot's work root is a *descendant* of the main project's own
    `.cortex`, which is exactly the shape fs_bridge's technical-storage
    refusal must not choke on for paths *inside* that root."""
    project, store, root, binding = _setup(tmp_path)
    assert root.startswith(str(project / ".cortex" / "workspaces"))


def test_fs_bridge_reads_and_writes_within_the_snapshot_work_dir(tmp_path):
    project, store, root, binding = _setup(tmp_path)
    bridge = FsBridge()
    result = bridge.read_text_file(binding, {"path": os.path.join(root, "a.txt")})
    assert result == {"content": "hi\n"}
    bridge.write_text_file(binding, {"path": os.path.join(root, "b.txt"), "content": "new\n"})
    assert open(os.path.join(root, "b.txt")).read() == "new\n"


def test_fs_bridge_refuses_the_real_project_cortex_store(tmp_path):
    """A worker bound to the snapshot work dir must never reach the main
    project's real technical storage -- the main `.cortex` is a *sibling*
    subtree of the snapshot's own `.cortex/workspaces/<attempt>/work`
    ancestor, not a path under `workspace_root`, so plain containment
    already refuses it; this pins that behaviour down explicitly."""
    project, store, root, binding = _setup(tmp_path)
    bridge = FsBridge()
    try:
        bridge.read_text_file(binding, {"path": str(project / ".cortex" / "cortex.sqlite3")})
        assert False, "must have refused"
    except FsBridgeError as error:
        assert "outside the bound workspace root" in error.message


def test_fs_bridge_refuses_a_dot_cortex_component_inside_the_snapshot_itself(tmp_path):
    """If the worker's own work dir somehow contained a `.cortex` entry
    (never true for a real snapshot -- excluded at capture time), the
    bridge's own technical-storage check (relative to workspace_root, not
    an ancestor) would still refuse it -- this is a defence-in-depth check,
    not a claim that snapshots ever contain one."""
    project, store, root, binding = _setup(tmp_path)
    os.makedirs(os.path.join(root, ".cortex"), exist_ok=True)
    with open(os.path.join(root, ".cortex", "leak.txt"), "w") as f:
        f.write("nope\n")
    bridge = FsBridge()
    try:
        bridge.read_text_file(binding, {"path": os.path.join(root, ".cortex", "leak.txt")})
        assert False, "must have refused"
    except FsBridgeError as error:
        assert "technical storage" in error.message


def test_terminal_bridge_cwd_containment_within_snapshot_work_dir(tmp_path):
    project, store, root, binding = _setup(tmp_path)
    bridge = TerminalBridge()
    terminal_id = bridge.create(binding, dict(command="/bin/ls", args=["-a"]), output_byte_limit_max=65536)
    try:
        terminal = bridge._get(binding.engine_session_id, terminal_id)
        assert terminal.wait_for_exit(timeout=5)
        output = terminal.output()
        assert "a.txt" in output["output"]
        # The engine process really ran with cwd inside the isolated
        # snapshot, not the main project root -- its own listing shows the
        # snapshot's excluded-.git/.cortex-free shape (the top-level .git
        # is never copied into the snapshot; only "b" and "a.txt" plus a
        # possible leaked test file from an earlier test could appear).
        assert "cortex.sqlite3" not in output["output"]
    finally:
        bridge.release(binding.engine_session_id, terminal_id)


def test_terminal_bridge_refuses_cwd_outside_the_snapshot_work_dir(tmp_path):
    project, store, root, binding = _setup(tmp_path)
    bridge = TerminalBridge()
    try:
        bridge.create(binding, dict(command="/bin/ls", cwd=str(project)), output_byte_limit_max=65536)
        assert False, "must have refused"
    except TerminalBridgeError as error:
        assert "outside the bound workspace root" in error.message
