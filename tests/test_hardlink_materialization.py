"""Development/Auto writers detach shared aliases before granting tool trees."""
import errno
import logging
import os
import shutil
import stat
import subprocess
from pathlib import Path

import pytest

from cortex_runtime.contracts import StoreError
from cortex_runtime.orchestration import sandbox


@pytest.fixture
def tree(tmp_path, monkeypatch):
    # Assembly tests do not execute this executable; real cases use real bwrap.
    monkeypatch.setattr(sandbox.shutil, "which", lambda name: "/usr/bin/bwrap")
    root = tmp_path / "project"
    root.mkdir()
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    home = tmp_path / "home"
    home.mkdir()
    outside = tmp_path / "cache"
    outside.write_bytes(b"shared package\n")
    env = root / ".venv"
    (env / "bin").mkdir(parents=True)
    outside.chmod(0o755)
    os.utime(outside, ns=(123000000000, 456000000000))
    for index in range(3):
        (env / "bin" / str(index)).hardlink_to(outside)
    (env / "python").symlink_to("/usr/bin/python3")
    monkeypatch.setattr(sandbox, "_HARDLINK_DIR_LIMIT", 1)
    return root, scratch, home, outside


def prepare(tree, **kwargs):
    root, scratch, home, _ = tree
    return sandbox.development_command(root, scratch, ["true"], ["."],
                                       environ={"HOME": str(home)}, **kwargs)


def test_writer_detaches_before_grants_preserving_metadata_and_shared_content(tree, caplog):
    root, _, _, outside = tree
    before = outside.stat()
    with caplog.at_level(logging.INFO, logger=sandbox.__name__):
        with prepare(tree) as (command, fds):
            assert fds
            mounts = [command[i:i+3] for i in range(len(command)-2)]
            assert not any(m[0] == "--ro-bind" and m[2].startswith(str(root / ".venv"))
                           for m in mounts)
    for index in range(3):
        path = root / ".venv" / "bin" / str(index)
        assert path.read_bytes() == b"shared package\n"
        assert path.stat().st_ino != before.st_ino
        assert stat.S_IMODE(path.stat().st_mode) == 0o755
        assert path.stat().st_mtime_ns == before.st_mtime_ns
        # In-place writes now affect only the independent project inode.
        path.write_bytes(b"changed")
    assert outside.read_bytes() == b"shared package\n"
    assert outside.stat().st_ino == before.st_ino
    assert (root / ".venv" / "python").readlink() == Path("/usr/bin/python3")
    assert not list(root.rglob(".cortex-unlink-*"))
    assert "files=3 bytes=45" in caplog.text


def test_streaming_fallback_and_short_writes(tree, monkeypatch):
    import fcntl
    def unsupported(*args):
        raise OSError(errno.EOPNOTSUPP, "no reflink")
    monkeypatch.setattr(fcntl, "ioctl", unsupported)
    real_write = os.write
    monkeypatch.setattr(os, "write", lambda fd, data: real_write(fd, data[:2]))
    with prepare(tree):
        pass
    assert (tree[0] / ".venv" / "bin" / "0").read_bytes() == tree[3].read_bytes()


@pytest.mark.parametrize("shape", ["denied", "private", "symlink-private"])
def test_no_materialization_of_protected_aliases(tree, shape):
    root, _, _, outside = tree
    kwargs = {}
    if shape == "denied":
        kwargs["readonly"] = (".venv/**",)
    elif shape == "private":
        (root / ".venv").rename(root / ".cortex")
    else:
        (root / ".cortex").symlink_to(".venv", target_is_directory=True)
    with prepare(tree, **kwargs):
        pass
    linked = root / (".cortex" if shape == "private" else ".venv") / "bin" / "0"
    assert linked.stat().st_ino == outside.stat().st_ino
    assert outside.read_bytes() == b"shared package\n"


def test_internal_links_still_share_an_inode(tree):
    root, _, _, _ = tree
    first = root / "one"
    first.write_bytes(b"internal")
    (root / "two").hardlink_to(first)
    with prepare(tree) as (command, _):
        assert not any(command[i] == "--ro-bind" and command[i+2] == str(first)
                       for i in range(len(command)-2))
    assert first.stat().st_ino == (root / "two").stat().st_ino


@pytest.mark.parametrize("scopes", [[".venv/bin/*"], [".venv/bin/0"]])
def test_narrow_file_grants_remain_writable_after_detaching(tree, scopes):
    root, scratch, home, outside = tree
    with sandbox.development_command(root, scratch, ["true"], scopes,
                                     environ={"HOME": str(home)}) as (command, _):
        assert any(command[i] == "--bind" and command[i+2] == str(root / ".venv/bin/0")
                   for i in range(len(command)-2))
    assert (root / ".venv/bin/0").stat().st_ino != outside.stat().st_ino


def test_size_limit_fails_before_mutation(tree, monkeypatch):
    monkeypatch.setattr(sandbox, "_MATERIALIZE_BYTES_LIMIT", 1)
    with pytest.raises(StoreError, match="command_hardlink_materialization_limit"):
        with prepare(tree):
            pytest.fail("must not grant writes")
    assert (tree[0] / ".venv/bin/0").stat().st_ino == tree[3].stat().st_ino


@pytest.mark.parametrize("failure", ["copy", "replace", "timeout", "changed"])
def test_failure_never_publishes_partial_files_and_is_resumable(tree, monkeypatch, failure):
    import fcntl
    real_replace = os.replace
    def unsupported(*args):
        raise OSError(errno.EOPNOTSUPP, "no reflink")
    monkeypatch.setattr(fcntl, "ioctl", unsupported)
    with monkeypatch.context() as patch:
        if failure == "copy":
            patch.setattr(os, "write", lambda *args: (_ for _ in ()).throw(OSError(errno.ENOSPC, "full")))
        elif failure == "replace":
            patch.setattr(os, "replace", lambda *args, **kw: (_ for _ in ()).throw(OSError(errno.EIO, "rename")))
        elif failure == "timeout":
            patch.setattr(sandbox, "_MATERIALIZE_SECONDS_LIMIT", -1)
        else:
            real_fsync = os.fsync
            def change(fd):
                real_fsync(fd)
                # A concurrent package update changes the source inode.
                tree[3].write_bytes(b"concurrent data")
            patch.setattr(os, "fsync", change)
        with pytest.raises(StoreError, match="command_hardlink_"):
            with prepare(tree):
                pytest.fail("must not launch on incomplete preparation")
    for index in range(3):
        assert (tree[0] / ".venv/bin" / str(index)).read_bytes() == tree[3].read_bytes()
    assert not list(tree[0].rglob(".cortex-unlink-*"))
    assert os.replace is real_replace
    with prepare(tree):
        pass
    assert (tree[0] / ".venv/bin/0").stat().st_ino != tree[3].stat().st_ino


def test_interrupted_batch_keeps_a_complete_usable_environment(tree, monkeypatch):
    real_replace = os.replace
    replaced = 0
    def interrupt(*args, **kwargs):
        nonlocal replaced
        if replaced:
            raise OSError(errno.ENOSPC, "interrupt after a complete file")
        real_replace(*args, **kwargs)
        replaced += 1
    with monkeypatch.context() as patch:
        patch.setattr(os, "replace", interrupt)
        with pytest.raises(StoreError, match="command_hardlink_materialization_failed"):
            with prepare(tree):
                pytest.fail("no command before all files are detached")
    assert replaced == 1
    for index in range(3):
        assert (tree[0] / ".venv/bin" / str(index)).read_bytes() == b"shared package\n"
    with prepare(tree):
        pass
    assert all((tree[0] / ".venv/bin" / str(i)).stat().st_ino != tree[3].stat().st_ino
               for i in range(3))


def test_denied_internal_alias_stays_untouched(tree):
    root, _, _, _ = tree
    first = root / "allowed"
    first.write_bytes(b"internal")
    denied = root / "denied"
    denied.hardlink_to(first)
    old_inode = denied.stat().st_ino
    with prepare(tree, readonly=("denied",)):
        pass
    first.write_bytes(b"changed")
    assert denied.stat().st_ino == old_inode
    assert denied.read_bytes() == b"internal"


def test_xattrs_are_copied_without_touching_shared_inode(tree):
    path = tree[3]
    try:
        os.setxattr(path, "user.cortex-test", b"metadata")
    except OSError as error:
        if error.errno in {errno.ENOTSUP, errno.EOPNOTSUPP}:
            pytest.skip("filesystem lacks extended attributes")
        raise
    with prepare(tree):
        pass
    target = tree[0] / ".venv/bin/0"
    assert os.getxattr(target, "user.cortex-test") == b"metadata"
    os.setxattr(target, "user.cortex-test", b"new")
    assert os.getxattr(path, "user.cortex-test") == b"metadata"


def test_symlink_ancestors_and_leaves_are_never_copied(tree):
    root, _, _, outside = tree
    (root / "alias").symlink_to(".venv", target_is_directory=True)
    with pytest.raises(StoreError):
        sandbox._materialize_hardlinks(root, ["alias/bin/0"])
    (root / "leaf").symlink_to(outside)
    with pytest.raises(StoreError, match="command_hardlink_changed"):
        sandbox._materialize_hardlinks(root, ["leaf"])
    assert outside.read_bytes() == b"shared package\n"


def test_guarded_preparation_does_not_detach_or_widen(tree):
    root, scratch, _, outside = tree
    with sandbox.gateway_command(root, scratch, ["true"], ["."]) as (command, _):
        assert any(command[i] == "--ro-bind" and command[i+2] == str(root / ".venv")
                   for i in range(len(command)-2))
    assert (root / ".venv/bin/0").stat().st_ino == outside.stat().st_ino


def test_exclusive_temporary_collision_never_removes_an_existing_file(tree, monkeypatch):
    monkeypatch.setattr(sandbox.secrets, "token_hex", lambda size: "existing")
    existing = tree[0] / ".venv/bin/.cortex-unlink-existing"
    existing.write_bytes(b"keep this file")
    with pytest.raises(StoreError, match="command_hardlink_materialization_failed"):
        with prepare(tree):
            pytest.fail("must not launch after exclusive create fails")
    assert existing.read_bytes() == b"keep this file"
    assert (tree[0] / ".venv/bin/0").stat().st_ino == tree[3].stat().st_ino


def test_preparation_callback_refusal_precedes_all_copy_mutations(tree):
    def refuse():
        raise StoreError("execution_policy_changed")
    with pytest.raises(StoreError, match="execution_policy_changed"):
        with prepare(tree, before_materialize=refuse):
            pytest.fail("must not launch")
    assert (tree[0] / ".venv/bin/0").stat().st_ino == tree[3].stat().st_ino
    assert not list(tree[0].rglob(".cortex-unlink-*"))


@pytest.mark.parametrize("failure", ["partial-copy", "spawn"])
def test_gateway_fences_persistent_preparation_on_failure(tmp_path, monkeypatch, failure):
    import json
    from cortex_runtime.configuration import access_mode
    from cortex_runtime.delegation import mcp_tools
    from cortex_runtime.orchestration import gateway
    from cortex_runtime.trusted_channel import resolve_channel
    from test_command_run_arguments import _worker

    root, store, channel_id, secret = _worker(tmp_path)
    worker = resolve_channel(store, channel_id, secret)
    access_mode.switch(store, worker.task_id, "development", origin="acp_set_mode")
    home = tmp_path / "home"
    home.mkdir()
    monkeypatch.setenv("HOME", str(home))
    monkeypatch.setenv("XDG_CONFIG_HOME", str(home / ".config"))
    monkeypatch.setenv("XDG_STATE_HOME", str(home / ".local/state"))
    for name in ("CODEX_HOME", "CLAUDE_CONFIG_DIR"):
        monkeypatch.delenv(name, raising=False)
    outside = tmp_path / "cache"
    outside.write_bytes(b"shared")
    env = root / ".venv"
    env.mkdir()
    for i in range(3):
        (env / str(i)).hardlink_to(outside)
    real_which = shutil.which
    monkeypatch.setattr(shutil, "which", lambda name: "/usr/bin/bwrap" if name == "bwrap" else real_which(name))
    replaced = 0
    real_replace = os.replace
    def replace(*args, **kwargs):
        nonlocal replaced
        if str(args[0]).startswith(".cortex-unlink-"):
            if failure == "partial-copy" and replaced:
                raise OSError(errno.ENOSPC, "full")
            replaced += 1
        return real_replace(*args, **kwargs)
    monkeypatch.setattr(os, "replace", replace)
    real_popen = subprocess.Popen
    def popen(argv, *args, **kwargs):
        if str(argv[0]).endswith("bwrap"):
            if failure == "partial-copy":
                pytest.fail("partial preparation launched a command")
            raise OSError(errno.EIO, "spawn failed")
        return real_popen(argv, *args, **kwargs)
    monkeypatch.setattr(gateway.subprocess, "Popen", popen)
    args = dict(argv=["true"], request_key="materialization-failure")
    with pytest.raises(StoreError):
        mcp_tools.dispatch(store, worker, "command_run", args)
    assert replaced == (1 if failure == "partial-copy" else 3)
    assert outside.read_bytes() == b"shared"
    with store.connection() as db:
        row = db.execute("SELECT result_json FROM runtime_requests WHERE request_key=?",
                         ("materialization-failure",)).fetchone()
        assert json.loads(row["result_json"])["complete"] is False
    with pytest.raises(StoreError, match="gateway_action_outcome_unknown"):
        mcp_tools.dispatch(store, worker, "command_run", args)


@pytest.mark.requires_bwrap
def test_real_writer_recreates_environment_without_changing_cache(tmp_path):
    if shutil.which("bwrap") is None:
        pytest.skip("bubblewrap is unavailable")
    root = tmp_path / "project"
    root.mkdir()
    scratch = tmp_path / "scratch"
    scratch.mkdir()
    home = tmp_path / "home"
    home.mkdir()
    env = root / ".venv"
    env.mkdir()
    outside = tmp_path / "cache"
    outside.write_text("shared")
    for i in range(257):
        (env / str(i)).hardlink_to(outside)
    script = "set -e; echo changed > .venv/0; rm -r .venv; mkdir .venv; echo new > .venv/new"
    with sandbox.development_command(root, scratch, ["/bin/sh", "-c", script], ["."],
                                     environ={"HOME": str(home)}) as (command, fds):
        done = subprocess.run(command, pass_fds=fds, capture_output=True, text=True, timeout=60)
    assert done.returncode == 0, done.stdout + done.stderr
    assert outside.read_text() == "shared"
    assert (env / "new").read_text() == "new\n"


def test_a_bulky_ignored_directory_stays_read_only_instead_of_timing_out(tmp_path, monkeypatch):
    """727 retained verification copies (~1.5 million pnpm links) in a
    git-ignored directory timed out every writer command (2026-10-21)."""
    monkeypatch.setattr(sandbox.shutil, "which", lambda name: "/usr/bin/bwrap")
    root = tmp_path / "project"
    (root / "scripts/.verification/copy").mkdir(parents=True)
    (root / "scripts/build.mjs").write_text("export {}\n")
    subprocess.run(["git", "init", "-q", str(root)], check=True)
    (root / ".gitignore").write_text("scripts/.verification/\n")
    outside = tmp_path / "store-file"
    outside.write_bytes(b"package\n")
    for index in range(5):
        (root / "scripts/.verification/copy" / str(index)).hardlink_to(outside)
    scratch, home = tmp_path / "scratch", tmp_path / "home"
    scratch.mkdir(); home.mkdir()
    monkeypatch.setattr(sandbox, "_BULKY_IGNORED_LINKS", 2)
    monkeypatch.setattr(sandbox, "_materialize_hardlinks",
                        lambda root_, relatives, before=None: relatives and pytest.fail(f"copied {relatives}"))
    with sandbox.development_command(root, scratch, ["true"], ["."], environ={"HOME": str(home)}) as (command, _):
        mounts = [command[i:i + 3] for i in range(len(command) - 2)]
        bulky = str(root / "scripts/.verification")
        assert any(m[0] == "--ro-bind" and m[2] == bulky for m in mounts)
        # One mount for the directory, none per shared file.
        assert not any(m[0] == "--ro-bind" and m[2].startswith(bulky + "/") for m in mounts)
    assert (root / "scripts/.verification/copy/0").stat().st_ino == outside.stat().st_ino


def test_a_materialization_limit_names_the_directory_to_clean(tree, monkeypatch):
    monkeypatch.setattr(sandbox, "_MATERIALIZE_BYTES_LIMIT", 1)
    with pytest.raises(StoreError, match="command_hardlink_materialization_limit") as refused:
        with prepare(tree):
            pytest.fail("must not grant writes")
    assert refused.value.received == ".venv/bin"
    assert "under .venv/bin" in refused.value.correction and "run the command again" in refused.value.correction
