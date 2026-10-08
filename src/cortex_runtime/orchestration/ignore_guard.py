"""Protect the project-local runtime store at runtime-owned edit boundaries."""
from ..contracts import StoreError


def protected_content(content: str) -> str:
    # Last rule wins. Ignoring the directory itself prevents descendant
    # negations (including nested ignore files) from reopening its contents.
    last = content.removesuffix('\n').split('\n')[-1]
    if last in {'/.cortex/', '/.cortex/\r'}:
        return content
    newline = '\r\n' if '\r\n' in content else '\n'
    return content + (newline if content and not content.endswith('\n') else '') + '/.cortex/' + newline


def snapshot(root):
    from .gateway import _read_file_snapshot
    try:
        return _read_file_snapshot(root, '.gitignore')
    except FileNotFoundError:
        return dict(content='', sha256='absent')


def enforce(store, channel):
    """Recheck live write authority, then repair using the normal digest writer.

    This boundary also covers native edits without pretending a host pre-hook
    exists. Read-only roles and already-correct files are never mutated.
    """
    from .. import task_workspace
    from .gateway import _authority, _matches, _write, write_denials
    with store.connection() as db:
        db.execute('BEGIN IMMEDIATE')
        scopes, _ = _authority(db, channel)
        if not scopes:
            return None
        root = task_workspace.task_root(db, channel.task_id)
        current = snapshot(root)
        if current.get('binary'):
            raise StoreError('gitignore_guard_non_utf8')
        content = protected_content(current['content'])
        if content == current['content']:
            return None
        if not _matches('.gitignore', scopes) or _matches('.gitignore', write_denials(db, channel.task_id)):
            raise StoreError('gateway_scope_denied')
        result = _write(root, dict(path='.gitignore', content=content, expected_sha256=current['sha256']))
        db.commit()
        return result


def reconcile_submission(store, channel):
    from .gateway import _authority
    from .skill_gate import observed_changed_paths
    with store.connection() as db:
        scopes, _ = _authority(db, channel)
    if not scopes:
        return None
    changed, _ = observed_changed_paths(store, channel.task_id, channel.attempt_id)
    # A complete unchanged observation preserves no-edit/advice tasks. Without
    # a baseline, ensure the rule rather than trusting an absent signal.
    if changed is None or '.gitignore' in changed:
        return enforce(store, channel)
    return None
