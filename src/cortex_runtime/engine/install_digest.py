"""SHA-256 of pinned engine install files, memoized per file identity.

Every engine launch re-verifies its pinned files (a 270 MB bundled codex
binary included), two or three times per launch. The digest is reused only
while the open file's device, inode, size, mtime and ctime are unchanged and
were unchanged across the read; any write changes ctime, so a drifted file
is always hashed again.
"""
import hashlib
import os
import threading

_lock = threading.Lock()
_digests: dict[tuple, str] = {}
_MAX_ENTRIES = 64


def _identity(path, info):
    return (os.fspath(path), info.st_dev, info.st_ino, info.st_size, info.st_mtime_ns, info.st_ctime_ns)


def sha256_file(path, chunk_size: int = 1024 * 1024) -> str:
    """Raises OSError when ``path`` cannot be read."""
    with open(path, "rb") as handle:
        before = os.fstat(handle.fileno())
        key = _identity(path, before)
        with _lock:
            cached = _digests.get(key)
        if cached is not None:
            return cached
        hasher = hashlib.sha256()
        while chunk := handle.read(chunk_size):
            hasher.update(chunk)
        digest = hasher.hexdigest()
        if _identity(path, os.fstat(handle.fileno())) == key:
            with _lock:
                if len(_digests) >= _MAX_ENTRIES:
                    _digests.pop(next(iter(_digests)))
                _digests[key] = digest
    return digest
