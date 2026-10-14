"""Private project names, shared by plan admission, file tools and mounts.

``.cortex`` is the runtime store and ``.git`` the repository; neither is ever
a worker path. A project's ``.claude`` and ``.codex`` (skills, agents,
commands, settings) are ordinary project paths that a writing worker may
change like any other file; the owner's engine state directories in HOME stay
protected by the engine jail (``sandbox.ENGINE_CONFIG``).
"""
from pathlib import PurePosixPath

PRIVATE = frozenset({'.cortex', '.git'})


def permitted_path(value):
    path = PurePosixPath(value)
    return not (
        path.is_absolute() or '..' in path.parts or '\\' in value or '\x00' in value
        or PRIVATE.intersection(path.parts)
    )
