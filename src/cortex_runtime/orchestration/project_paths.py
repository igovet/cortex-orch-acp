"""Project notes that are data, not executable engine configuration.

Keep this exception exact and shared by plan admission, file tools and mounts.
Directory/glob grants of private engine state remain forbidden.
"""
from pathlib import PurePosixPath

PRIVATE = frozenset({'.cortex', '.git', '.codex', '.claude'})
PROJECT_NOTES = frozenset(
    f'{directory}/{name}'
    for directory in ('.claude', '.codex')
    for name in ('session-history.md', 'lessons-learned.md')
)


def permitted_path(value):
    path = PurePosixPath(value)
    return not (
        path.is_absolute() or '..' in path.parts or '\\' in value or '\x00' in value
        or (PRIVATE.intersection(path.parts) and path.as_posix() not in PROJECT_NOTES)
    )
