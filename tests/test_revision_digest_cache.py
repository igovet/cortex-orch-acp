"""Incremental hashing preserves the pre-cache manifest algorithm."""
from collections import OrderedDict
from concurrent.futures import ThreadPoolExecutor
import ctypes
import hashlib
import os
import time

import pytest

from cortex_runtime.delegation import revision


def _legacy_hash_file(path: str, budget: dict, deadline: float, *, parent_fd=None, name=None, expected=None) -> str:
    h = hashlib.sha256()
    size = 0
    try:
        fd = (revision._open_regular_nofollow_at(parent_fd, name, path, expected)
              if parent_fd is not None else revision._open_regular_nofollow(path))
    except OSError as error:
        raise revision._Unavailable(f"unreadable_entry:{path}:{error}") from error
    try:
        try:
            opened_st = os.fstat(fd)
        except OSError as error:
            raise revision._Unavailable(f"unreadable_entry:{path}:{error}") from error
        if (opened_st.st_size > revision._CONTENT_HASH_MAX_BYTES
                or budget["total_bytes"] + opened_st.st_size > revision._MAX_TOTAL_HASH_BYTES):
            # Build artifacts (an .apk, native .so files: 1.7 GB in one real
            # mobile project) blew the hash budget, so no capture was ever
            # available and review never started. A large file is identified
            # by its metadata instead: ctime changes on every write and no
            # process can set it back.
            return "stat:" + hashlib.sha256(
                f"{opened_st.st_size}:{opened_st.st_ino}:{opened_st.st_mtime_ns}:{opened_st.st_ctime_ns}".encode()
            ).hexdigest()
        while True:
            if time.monotonic() > deadline:
                raise revision._Unavailable("capture_timed_out")
            try:
                chunk = os.read(fd, revision._READ_CHUNK)
            except OSError as error:
                raise revision._Unavailable(f"unreadable_entry:{path}:{error}") from error
            if not chunk:
                break
            size += len(chunk)
            if size > revision._MAX_FILE_HASH_BYTES:
                raise revision._Unavailable("workspace_too_large")
            budget["total_bytes"] += len(chunk)
            if budget["total_bytes"] > revision._MAX_TOTAL_HASH_BYTES:
                raise revision._Unavailable("workspace_too_large")
            h.update(chunk)
        try:
            final_st = os.fstat(fd)
        except OSError as error:
            raise revision._Unavailable(f"unreadable_entry:{path}:{error}") from error
        if not revision._same_node_state(opened_st, final_st):
            raise revision._Unavailable(f"changed_while_reading:{path}")
    finally:
        os.close(fd)
    return h.hexdigest()


@pytest.fixture(autouse=True)
def isolated_cache(monkeypatch):
    monkeypatch.setattr(revision, "_CONTENT_HASH_CACHE", OrderedDict())
    monkeypatch.setattr(revision, "_git_listed_paths", lambda root: None)


@pytest.fixture
def aged_clock(monkeypatch):
    # Advance the clock, not ctime (which ordinary processes cannot forge).
    clock = time.time_ns() + 5_000_000_000
    monkeypatch.setattr(revision.time, "time_ns", lambda: clock)
    monkeypatch.setattr(revision, "_timestamp_guard_ns", lambda fd: 1_000_000_000)


def _legacy_tree(root, monkeypatch):
    with monkeypatch.context() as patch:
        patch.setattr(revision, "_hash_file", _legacy_hash_file)
        return revision._tree_digest(root)


def _count_reads(monkeypatch):
    calls = []
    real = os.read

    def read(fd, size):
        calls.append(fd)
        return real(fd, size)

    monkeypatch.setattr(os, "read", read)
    return calls


def test_cached_manifest_is_byte_identical_and_avoids_reads(tmp_path, monkeypatch, aged_clock):
    (tmp_path / "a").write_bytes(b"hello\x00world")
    (tmp_path / "empty").touch()
    (tmp_path / "sub").mkdir()
    (tmp_path / "sub" / "b").write_bytes(b"bytes")
    (tmp_path / "link").symlink_to("a")
    os.mkfifo(tmp_path / "fifo")
    expected = _legacy_tree(tmp_path, monkeypatch)
    calls = _count_reads(monkeypatch)
    assert revision._tree_digest(tmp_path) == expected
    assert calls
    calls.clear()
    assert revision._tree_digest(tmp_path) == expected
    assert not calls
    assert len(revision._CONTENT_HASH_CACHE) == 3


def test_real_filesystem_guard(tmp_path, monkeypatch):
    path = tmp_path / "file"
    path.write_bytes(b"actual native fstatfs")
    fd = os.open(path, os.O_RDONLY)
    try:
        guard = revision._timestamp_guard_ns(fd)
        assert guard in (None, 1_000_000_000)
    finally:
        os.close(fd)
    # Exercise the real native guard through the whole capture path; only
    # clock ageing is simulated, so unknown filesystems must stay uncached.
    stamp = max(path.stat().st_mtime_ns, path.stat().st_ctime_ns)
    monkeypatch.setattr(revision.time, "time_ns", lambda: stamp + 5_000_000_000)
    calls = _count_reads(monkeypatch)
    expected = revision._tree_digest(tmp_path)
    assert calls
    calls.clear()
    assert revision._tree_digest(tmp_path) == expected
    if guard is None:
        assert calls and not revision._CONTENT_HASH_CACHE
    else:
        assert not calls and len(revision._CONTENT_HASH_CACHE) == 1


@pytest.mark.parametrize("magic", [0xEF53, 0x58465342, 0x9123683E, 0x01021994])
def test_local_filesystem_guard(monkeypatch, magic):
    def fstatfs(fd, buf):
        ctypes.cast(buf, ctypes.POINTER(ctypes.c_long))[0] = magic
        return 0

    monkeypatch.setattr(revision, "_FSTATFS", fstatfs)
    assert revision._timestamp_guard_ns(123) == 1_000_000_000


@pytest.mark.parametrize("magic", [0x6969, 0x794C7630, 0])
def test_unknown_remote_and_overlay_filesystems_not_cached(tmp_path, monkeypatch, aged_clock, magic):
    def fstatfs(fd, buf):
        ctypes.cast(buf, ctypes.POINTER(ctypes.c_long))[0] = magic
        return 0

    monkeypatch.setattr(revision, "_FSTATFS", fstatfs)
    # Restore the real filesystem gate replaced by aged_clock.
    monkeypatch.setattr(revision, "_timestamp_guard_ns", _REAL_TIMESTAMP_GUARD)
    (tmp_path / "file").write_bytes(b"data")
    calls = _count_reads(monkeypatch)
    assert revision._tree_digest(tmp_path)[0]
    calls.clear()
    assert revision._tree_digest(tmp_path)[0]
    assert calls and not revision._CONTENT_HASH_CACHE


_REAL_TIMESTAMP_GUARD = revision._timestamp_guard_ns


@pytest.mark.parametrize("fstatfs", [None, lambda fd, buf: -1])
def test_unavailable_filesystem_guard(monkeypatch, fstatfs):
    monkeypatch.setattr(revision, "_FSTATFS", fstatfs)
    assert revision._timestamp_guard_ns(123) is None


def test_forged_mtime_same_size_content_change(tmp_path, monkeypatch, aged_clock):
    path = tmp_path / "file"
    path.write_bytes(b"aaaa")
    before_st = path.stat()
    before = revision._tree_digest(tmp_path)
    assert revision._CONTENT_HASH_CACHE
    path.write_bytes(b"bbbb")
    os.utime(path, ns=(before_st.st_atime_ns, before_st.st_mtime_ns))
    after_st = path.stat()
    assert after_st.st_size == before_st.st_size
    assert after_st.st_mtime_ns == before_st.st_mtime_ns
    assert after_st.st_ctime_ns != before_st.st_ctime_ns
    after = revision._tree_digest(tmp_path)
    assert after[0] != before[0]
    assert after == _legacy_tree(tmp_path, monkeypatch)


@pytest.mark.parametrize("operation", ["rename", "replace", "delete", "symlink", "chmod"])
def test_entry_changes_match_original(tmp_path, monkeypatch, aged_clock, operation):
    path = tmp_path / "file"
    path.write_bytes(b"old!")
    before = revision._tree_digest(tmp_path)
    assert revision._CONTENT_HASH_CACHE
    if operation == "rename":
        path.rename(tmp_path / "renamed")
    elif operation == "replace":
        other = tmp_path / "replacement"
        other.write_bytes(b"new!")
        os.replace(other, path)
    elif operation == "delete":
        path.unlink()
    elif operation == "symlink":
        path.unlink()
        path.symlink_to("missing")
    else:
        path.chmod(0o600)
    after = revision._tree_digest(tmp_path)
    assert after == _legacy_tree(tmp_path, monkeypatch)
    assert (after[0] == before[0]) == (operation == "chmod")


def test_cached_file_still_requires_read_permission(tmp_path, aged_clock):
    path = tmp_path / "file"
    path.write_bytes(b"readable")
    assert revision._tree_digest(tmp_path)[0]
    path.chmod(0)
    try:
        if os.access(path, os.R_OK):
            pytest.skip("owner bypasses file permissions")
        digest, reason, manifest = revision._tree_digest(tmp_path)
        assert digest is manifest is None
        assert reason.startswith("unreadable_entry:")
    finally:
        path.chmod(0o600)


@pytest.mark.parametrize("offset_ns", [0, 999_999_999, 1_000_000_000, -1_000_000_000])
def test_recent_boundary_and_future_timestamps_never_cached(tmp_path, monkeypatch, offset_ns):
    path = tmp_path / "file"
    path.write_bytes(b"data")
    stamp = max(path.stat().st_mtime_ns, path.stat().st_ctime_ns)
    monkeypatch.setattr(revision, "_timestamp_guard_ns", lambda fd: 1_000_000_000)
    monkeypatch.setattr(revision.time, "time_ns", lambda: stamp + offset_ns)
    calls = _count_reads(monkeypatch)
    first = revision._tree_digest(tmp_path)
    calls.clear()
    assert revision._tree_digest(tmp_path) == first
    assert calls and not revision._CONTENT_HASH_CACHE


def test_cached_hit_rechecks_identity_after_lookup(tmp_path, monkeypatch, aged_clock):
    path = tmp_path / "file"
    path.write_bytes(b"aaaa")
    assert revision._tree_digest(tmp_path)[0]
    original = revision._cached_content_hash

    def changing_lookup(key):
        digest = original(key)
        path.write_bytes(b"bbbb")
        return digest

    monkeypatch.setattr(revision, "_cached_content_hash", changing_lookup)
    digest, reason, manifest = revision._tree_digest(tmp_path)
    assert digest is manifest is None
    assert reason == f"changed_while_reading:{path}"


def test_change_during_uncached_read_not_published(tmp_path, monkeypatch, aged_clock):
    path = tmp_path / "file"
    path.write_bytes(b"aaaa")
    real = os.read

    def changing_read(fd, size):
        data = real(fd, size)
        if data:
            path.write_bytes(b"bbbb")
        return data

    monkeypatch.setattr(os, "read", changing_read)
    assert revision._tree_digest(tmp_path)[1] == f"changed_while_reading:{path}"
    assert not revision._CONTENT_HASH_CACHE


def test_logical_budget_and_large_file_fallback_identical(tmp_path, monkeypatch, aged_clock):
    for name in ("a", "b", "c"):
        (tmp_path / name).write_bytes(b"1234")
    # Cache the files before constraining the budget.
    assert revision._tree_digest(tmp_path)[0]
    monkeypatch.setattr(revision, "_MAX_TOTAL_HASH_BYTES", 5)
    expected = _legacy_tree(tmp_path, monkeypatch)
    assert revision._tree_digest(tmp_path) == expected
    assert sum("\tstat:" in row for row in expected[2]) == 2
    monkeypatch.setattr(revision, "_CONTENT_HASH_MAX_BYTES", 3)
    expected = _legacy_tree(tmp_path, monkeypatch)
    assert revision._tree_digest(tmp_path) == expected
    assert all("\tstat:" in row for row in expected[2])


def test_cache_does_not_bypass_file_cap_or_deadline(tmp_path, monkeypatch, aged_clock):
    (tmp_path / "file").write_bytes(b"1234")
    assert revision._tree_digest(tmp_path)[0]
    monkeypatch.setattr(revision, "_MAX_FILE_HASH_BYTES", 3)
    assert revision._tree_digest(tmp_path) == _legacy_tree(tmp_path, monkeypatch)
    monkeypatch.setattr(revision, "_MAX_FILE_HASH_BYTES", 64 * 1024 * 1024)
    with pytest.raises(revision._Unavailable, match="capture_timed_out"):
        revision._hash_file(str(tmp_path / "file"), {"total_bytes": 0}, time.monotonic() - 1)


def test_lru_is_bounded_under_concurrent_captures(tmp_path, monkeypatch, aged_clock):
    for index in range(12):
        (tmp_path / str(index)).write_bytes(str(index).encode())
    monkeypatch.setattr(revision, "_CONTENT_HASH_CACHE_MAX_ENTRIES", 4)
    expected = _legacy_tree(tmp_path, monkeypatch)
    with ThreadPoolExecutor(max_workers=8) as pool:
        results = list(pool.map(lambda _: revision._tree_digest(tmp_path), range(32)))
    assert all(result == expected for result in results)
    assert len(revision._CONTENT_HASH_CACHE) == 4


def test_lru_eviction_order(monkeypatch):
    monkeypatch.setattr(revision, "_CONTENT_HASH_CACHE_MAX_ENTRIES", 2)
    for key in [(1,), (2,)]:
        revision._remember_content_hash(key, "digest")
    assert revision._cached_content_hash((1,)) == "digest"
    revision._remember_content_hash((3,), "digest")
    assert list(revision._CONTENT_HASH_CACHE) == [(1,), (3,)]
    assert revision._cached_content_hash((2,)) is None


def test_disabled_cache_matches_original(tmp_path, monkeypatch, aged_clock):
    (tmp_path / "file").write_bytes(b"data")
    monkeypatch.setattr(revision, "_CONTENT_HASH_CACHE_MAX_ENTRIES", 0)
    assert revision._tree_digest(tmp_path) == _legacy_tree(tmp_path, monkeypatch)
    assert not revision._CONTENT_HASH_CACHE


def test_warm_cache_does_not_hide_symlink_retarget(tmp_path, monkeypatch, aged_clock):
    (tmp_path / "file").write_bytes(b"data")
    link = tmp_path / "link"
    link.symlink_to("file")
    before = revision._tree_digest(tmp_path)
    link.unlink()
    link.symlink_to("other")
    after = revision._tree_digest(tmp_path)
    assert after[0] != before[0]
    assert after == _legacy_tree(tmp_path, monkeypatch)
