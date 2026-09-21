"""Canonical source-payload hashing for the repository-owned Paseo plugin."""
from __future__ import annotations

import hashlib
import os
from pathlib import Path


# Dependencies and generated/local state are not shipped plugin assets. Keep
# this directory based so every file that can be a source, manifest,
# configuration, or lock asset remains covered without an extension allowlist.
EXCLUDED_DIRS = frozenset({
    ".cache", ".git", ".next", ".tmp", ".turbo", "__pycache__",
    "build", "cache", "coverage", "dist", "node_modules", "out",
    "target", "temp", "tmp",
})


def payload_digest(root: Path) -> str:
    """Hash all plugin payload files in canonical relative-path order."""
    root = Path(root)
    files = []
    for parent, directories, names in os.walk(root, topdown=True, followlinks=False):
        directories[:] = sorted(name for name in directories if name not in EXCLUDED_DIRS)
        files.extend(Path(parent) / name for name in names)

    digest = hashlib.sha256()
    for path in sorted(files, key=lambda item: item.relative_to(root).as_posix()):
        if path.name == "plugin-integrity.json":
            continue
        relative = path.relative_to(root).as_posix().encode()
        body = path.read_bytes()
        digest.update(len(relative).to_bytes(8, "big"))
        digest.update(relative)
        digest.update(len(body).to_bytes(8, "big"))
        digest.update(body)
    return digest.hexdigest()
