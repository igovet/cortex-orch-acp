"""FsBridge: path validation/containment, technical-storage refusal, size
caps, atomic writes, and revision capture (T014)."""
from __future__ import annotations

import os
import threading
import time

import pytest

from cortex_runtime.engine.fs_bridge import FsBridge, FsBridgeError, HostFsForwarder, RevisionLog
from cortex_runtime.engine.resource_binding import ResourceBinding


def _binding(root, **overrides):
    fields = dict(
        engine_instance_id="eirt_a", engine_session_id="es_a", context_generation_id="gen_1",
        assignment_id="asg_1", attempt_id="att_1", workspace_binding_id="wb_1",
        workspace_root=str(root), ownership_epoch="epoch_1", role="worker",
    )
    fields.update(overrides)
    return ResourceBinding(**fields)


def test_relative_path_rejected(tmp_path):
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(_binding(tmp_path), dict(path="relative/file.txt"))


def test_nul_byte_rejected(tmp_path):
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(_binding(tmp_path), dict(path=str(tmp_path) + "/a\x00b"))


def test_windows_drive_path_rejected(tmp_path):
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(_binding(tmp_path), dict(path="C:\\Windows\\file.txt"))


def test_traversal_outside_workspace_rejected(tmp_path):
    outside = tmp_path.parent / "outside-fs-bridge-test"
    outside.mkdir(exist_ok=True)
    (outside / "secret.txt").write_text("nope")
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(_binding(tmp_path), dict(path=str(tmp_path) + "/../" + outside.name + "/secret.txt"))


def test_absolute_path_outside_workspace_rejected(tmp_path):
    outside = tmp_path.parent / "outside-fs-bridge-test-2"
    outside.mkdir(exist_ok=True)
    (outside / "secret.txt").write_text("nope")
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(_binding(tmp_path), dict(path=str(outside / "secret.txt")))


def test_symlink_escape_via_file_rejected(tmp_path):
    outside = tmp_path.parent / "outside-fs-bridge-test-3"
    outside.mkdir(exist_ok=True)
    (outside / "secret.txt").write_text("nope")
    link = tmp_path / "escape.txt"
    os.symlink(outside / "secret.txt", link)
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(_binding(tmp_path), dict(path=str(link)))


def test_symlink_escape_via_parent_dir_rejected(tmp_path):
    outside = tmp_path.parent / "outside-fs-bridge-test-4"
    outside.mkdir(exist_ok=True)
    link_dir = tmp_path / "escape-dir"
    os.symlink(outside, link_dir)
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.write_text_file(_binding(tmp_path), dict(path=str(link_dir / "new.txt"), content="x"))


def test_git_directory_refused(tmp_path):
    (tmp_path / ".git").mkdir()
    (tmp_path / ".git" / "config").write_text("x")
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(_binding(tmp_path), dict(path=str(tmp_path / ".git" / "config")))


def test_cortex_directory_refused(tmp_path):
    (tmp_path / ".cortex").mkdir()
    (tmp_path / ".cortex" / "state.db").write_text("x")
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(_binding(tmp_path), dict(path=str(tmp_path / ".cortex" / "state.db")))


@pytest.mark.parametrize("relative", [
    ".cortex/cortex.sqlite3", ".cortex/cortex.sqlite3-wal", ".cortex/t_deadbeef0001/report.md",
    ".cortex/t_deadbeef0001/pipeline.md", ".cortex/t_deadbeef0001/.orphaned/x.md.abc123abc123",
    ".cortex/.source_blocks_staging/blk_1", ".cortex/draft-reports/d_1.md", ".cortex/pipeline-drafts/d_2.md",
])
def test_cortex_technical_storage_subpaths_refused_for_read_and_write(tmp_path, relative):
    """T019 entrypoint audit: every concrete .cortex/** technical-storage
    subpath (SQLite metadata, per-task report/pipeline bodies, the new
    T019 orphan-quarantine directory, source-block staging, and draft
    directories) is refused through the worker's own fs_bridge channel --
    never only the bare '.cortex' prefix in the abstract."""
    path = tmp_path / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"technical content")
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(_binding(tmp_path), dict(path=str(path)))
    with pytest.raises(FsBridgeError):
        bridge.write_text_file(_binding(tmp_path), dict(path=str(path), content="x"))


def test_read_write_roundtrip_records_disk_revision(tmp_path):
    revision_log = RevisionLog()
    bridge = FsBridge(revision_log=revision_log)
    binding = _binding(tmp_path)
    target = tmp_path / "notes.txt"
    bridge.write_text_file(binding, dict(path=str(target), content="hello world"))
    result = bridge.read_text_file(binding, dict(path=str(target)))
    assert result == {"content": "hello world"}
    entries = revision_log.entries_for(binding.workspace_binding_id)
    assert len(entries) == 2
    assert all(entry.source == "disk" for entry in entries)


def test_read_line_and_limit(tmp_path):
    bridge = FsBridge()
    binding = _binding(tmp_path)
    target = tmp_path / "lines.txt"
    target.write_text("a\nb\nc\nd\n")
    result = bridge.read_text_file(binding, dict(path=str(target), line=2, limit=2))
    assert result == {"content": "b\nc\n"}


def test_read_oversize_file_rejected(tmp_path):
    bridge = FsBridge(max_bytes=16)
    binding = _binding(tmp_path)
    target = tmp_path / "big.txt"
    target.write_text("x" * 100)
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(binding, dict(path=str(target)))


def test_read_non_utf8_rejected(tmp_path):
    bridge = FsBridge()
    binding = _binding(tmp_path)
    target = tmp_path / "binary.dat"
    target.write_bytes(b"\xff\xfe\x00\x01")
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(binding, dict(path=str(target)))


def test_write_is_atomic_and_preserves_mode(tmp_path):
    bridge = FsBridge()
    binding = _binding(tmp_path)
    target = tmp_path / "existing.txt"
    target.write_text("old")
    os.chmod(target, 0o640)
    bridge.write_text_file(binding, dict(path=str(target), content="new content"))
    assert target.read_text() == "new content"
    assert (os.stat(target).st_mode & 0o777) == 0o640
    # No leftover temp files in the parent directory.
    leftovers = [name for name in os.listdir(tmp_path) if name.startswith(".cortex-fswrite-")]
    assert leftovers == []


def test_worker_a_binding_cannot_touch_worker_b_worktree(tmp_path):
    root_a = tmp_path / "worker-a"
    root_b = tmp_path / "worker-b"
    root_a.mkdir()
    root_b.mkdir()
    (root_b / "b-secret.txt").write_text("b's data")
    binding_a = _binding(root_a, engine_session_id="es_a", workspace_binding_id="wb_a")
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(binding_a, dict(path=str(root_b / "b-secret.txt")))


# --- T014 review (HIGH): TOCTOU fd-based checks -----------------------

def test_dangling_symlink_rejected(tmp_path):
    link = tmp_path / "dangling.txt"
    os.symlink(tmp_path / "does-not-exist.txt", link)
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(_binding(tmp_path), dict(path=str(link)))


def test_swap_between_check_and_open_cannot_succeed(tmp_path, monkeypatch):
    """Simulates an attacker racing to replace the target with a symlink to
    outside content: since the fix opens (O_NOFOLLOW) *before* any
    containment check and verifies the *opened fd's* real path (never
    re-resolving the original path string), a hook injected exactly at the
    old check-then-open boundary can no longer matter -- patching
    ``os.path.realpath`` (the pre-fix check's own resolution call) to lie
    must have zero effect on the outcome, since the fixed code path never
    calls it for read_text_file."""
    outside = tmp_path.parent / "outside-swap-test"
    outside.mkdir(exist_ok=True)
    (outside / "secret.txt").write_text("nope")
    target = tmp_path / "swap-me.txt"
    target.write_text("legit content")
    binding = _binding(tmp_path)  # constructed before the patch below

    def lying_realpath(path, *args, **kwargs):
        # If the fixed code ever called this to "check" containment
        # (it must not), lie that the outside secret is inside the
        # workspace -- proving the fix does not depend on it.
        return str(target)

    monkeypatch.setattr(os.path, "realpath", lying_realpath)
    bridge = FsBridge()
    result = bridge.read_text_file(binding, dict(path=str(target)))
    assert result == {"content": "legit content"}
    # Now actually swap the target for a symlink escaping the workspace,
    # still with the lying realpath patched: the fd-based fix must still
    # refuse it (O_NOFOLLOW on the open call itself).
    target.unlink()
    os.symlink(outside / "secret.txt", target)
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(binding, dict(path=str(target)))


def test_symlinked_parent_swapped_after_binding_rejected(tmp_path):
    real_dir = tmp_path / "real-subdir"
    real_dir.mkdir()
    binding = _binding(real_dir, workspace_binding_id="wb_swap")
    outside = tmp_path.parent / "outside-swapped-parent"
    outside.mkdir(exist_ok=True)
    real_dir.rmdir()
    os.symlink(outside, real_dir)
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.write_text_file(binding, dict(path=str(real_dir / "new.txt"), content="x"))


def test_write_parent_directory_itself_symlinked_rejected(tmp_path):
    outside = tmp_path.parent / "outside-write-parent-test"
    outside.mkdir(exist_ok=True)
    link_dir = tmp_path / "linked-parent"
    os.symlink(outside, link_dir)
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.write_text_file(_binding(tmp_path), dict(path=str(link_dir / "new.txt"), content="x"))


# --- T014 review (MEDIUM): non-regular files refused without blocking --

def test_fifo_rejected_without_blocking(tmp_path):
    fifo_path = tmp_path / "a.fifo"
    os.mkfifo(fifo_path)
    bridge = FsBridge()

    result_holder = {}

    def call():
        try:
            bridge.read_text_file(_binding(tmp_path), dict(path=str(fifo_path)))
        except FsBridgeError as error:
            result_holder["error"] = error

    thread = threading.Thread(target=call)
    started = time.monotonic()
    thread.start()
    thread.join(timeout=2.0)
    elapsed = time.monotonic() - started
    assert not thread.is_alive()  # never blocked waiting for a FIFO writer
    assert elapsed < 2.0
    assert "error" in result_holder


def test_directory_target_rejected_for_read(tmp_path):
    sub = tmp_path / "a-directory"
    sub.mkdir()
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.read_text_file(_binding(tmp_path), dict(path=str(sub)))


def test_fifo_as_existing_write_target_rejected(tmp_path):
    fifo_path = tmp_path / "existing.fifo"
    os.mkfifo(fifo_path)
    bridge = FsBridge()
    with pytest.raises(FsBridgeError):
        bridge.write_text_file(_binding(tmp_path), dict(path=str(fifo_path), content="x"))


def test_host_buffer_revision_is_distinct_from_disk_revision(tmp_path):
    revision_log = RevisionLog()
    disk_bridge = FsBridge(revision_log=revision_log)
    binding = _binding(tmp_path)
    target = tmp_path / "shared-name.txt"
    disk_bridge.write_text_file(binding, dict(path=str(target), content="disk-content"))

    def host_fs_requester(method, params):
        if method == "fs/read_text_file":
            return {"content": "unsaved-host-buffer-content"}
        return {}

    forwarder = HostFsForwarder(host_fs_requester=host_fs_requester, revision_log=revision_log)
    forwarder.read_text_file(binding, dict(path=str(target)))
    entries = revision_log.entries_for(binding.workspace_binding_id)
    sources = sorted(entry.source for entry in entries)
    assert sources == ["disk", "host_buffer"]
    disk_entry = next(entry for entry in entries if entry.source == "disk")
    host_entry = next(entry for entry in entries if entry.source == "host_buffer")
    assert disk_entry.sha256 != host_entry.sha256  # never silently equated

def test_write_fsyncs_file_before_rename_and_parent_after(tmp_path, monkeypatch):
    import stat
    bridge = FsBridge()
    events = []
    fsync, replace = os.fsync, os.replace
    def sync(fd):
        events.append('directory' if stat.S_ISDIR(os.fstat(fd).st_mode) else 'file')
        return fsync(fd)
    def rename(*args, **kwargs):
        events.append('replace')
        return replace(*args, **kwargs)
    monkeypatch.setattr(os, 'fsync', sync)
    monkeypatch.setattr(os, 'replace', rename)
    bridge.write_text_file(_binding(tmp_path), dict(path=str(tmp_path/'new'), content='durable'))
    assert events == ['file', 'replace', 'directory']
    assert (tmp_path/'new').read_text() == 'durable'
