"""Project skill links: ``.claude/skills/<name> -> ../../.agents/skills/<name>``.

A writing worker may create or remove exactly one relative directory
symlink that exposes a shared project skill in ``.agents/skills/<name>`` to
Claude Code, which reads project skills only from ``.claude/skills``. Unlike
a generic file or command write there, it verifies the source, never writes a
file, never follows a link and never touches any other ``.claude`` path.

Rules (see docs/features/permissions-and-containment/index.md, "Project skill links"):

- ``name`` is 1-64 lowercase letters, digits and single hyphens.
- The source ``.agents/skills/<name>`` must be a real directory inside the
  project (no symlinked component) holding a regular ``SKILL.md``.
- ``.claude`` and ``.claude/skills`` must be real directories (created when
  absent); a symlinked or non-directory component refuses the operation.
- The link text is always exactly ``../../.agents/skills/<name>``: relative,
  so Git stores it as a symlink (mode 120000) that works in every clone.
- An existing regular file, directory or other non-link at the link path is
  never replaced or removed. A symlink with another target (stale or foreign)
  is replaced only when the caller names its exact current target and the
  user confirms; ``unlink`` removes only the exact managed link.
"""
import errno
import os
import re
import stat
import uuid

from ..contracts import StoreError

NAME = re.compile(r'[a-z0-9]+(?:-[a-z0-9]+)*')
MAX_NAME = 64
_DIR_FLAGS = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW


def check_name(name):
    if len(name) > MAX_NAME or NAME.fullmatch(name) is None:
        raise StoreError('skill_link_name_invalid', 'name', name[:80],
                         '1-64 lowercase letters, digits and single hyphens (e.g. release-notes)',
                         'Use the exact directory name of the skill under .agents/skills.')
    return name


def link_path(name):
    return f'.claude/skills/{name}'


def link_target(name):
    return f'../../.agents/skills/{name}'


def _open_dir(parent, name, *, code, create=False, missing_ok=False, effects=None):
    """A real child directory by no-follow descriptor; ``None`` when absent
    and ``missing_ok``, created when absent and ``create`` (which marks
    ``effects['possible']`` first)."""
    for _ in range(2):
        try:
            return os.open(name, _DIR_FLAGS, dir_fd=parent)
        except FileNotFoundError:
            if missing_ok and not create:
                return None
            if not create:
                raise StoreError(code, 'path', name, 'an existing real directory') from None
            if effects is not None:
                effects['possible'] = True
            try:
                os.mkdir(name, 0o755, dir_fd=parent)
                os.fsync(parent)
            except FileExistsError:
                pass  # created concurrently: open (and check) it again
        except OSError as error:
            if error.errno in (errno.ELOOP, errno.ENOTDIR):
                raise StoreError(code, 'path', name, 'a real directory, not a symlink or file') from None
            raise
    raise StoreError(code, 'path', name, 'a real directory')


def _verify_source(root_fd, name):
    opened = []
    try:
        fd = root_fd
        for part in ('.agents', 'skills', name):
            fd = _open_dir(fd, part, code='skill_source_missing')
            opened.append(fd)
        try:
            skill = os.open('SKILL.md', os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=fd)
        except OSError:
            raise StoreError('skill_source_missing', 'path', f'.agents/skills/{name}/SKILL.md',
                             'a regular SKILL.md file') from None
        try:
            if not stat.S_ISREG(os.fstat(skill).st_mode):
                raise StoreError('skill_source_missing', 'path', f'.agents/skills/{name}/SKILL.md',
                                 'a regular SKILL.md file')
        finally:
            os.close(skill)
    finally:
        for fd in opened:
            os.close(fd)


def _observe(skills_fd, name):
    """``(kind, link_text)`` of the entry at ``.claude/skills/<name>``."""
    try:
        info = os.stat(name, dir_fd=skills_fd, follow_symlinks=False)
    except FileNotFoundError:
        return 'absent', None
    if stat.S_ISLNK(info.st_mode):
        return 'symlink', os.readlink(name, dir_fd=skills_fd)
    return ('directory' if stat.S_ISDIR(info.st_mode) else
            'file' if stat.S_ISREG(info.st_mode) else 'other'), None


def _skills_dir(root_fd, *, create, effects=None):
    """``.claude/skills`` by descriptor, or ``None`` when absent (and not created)."""
    claude = _open_dir(root_fd, '.claude', code='skill_link_parent_unsafe', create=create, missing_ok=True,
                       effects=effects)
    if claude is None:
        return None
    try:
        return _open_dir(claude, 'skills', code='skill_link_parent_unsafe', create=create, missing_ok=True,
                         effects=effects)
    finally:
        os.close(claude)


def inspect(root, name):
    """The current entry at the link path, without changing anything."""
    root_fd = os.open(root, _DIR_FLAGS)
    try:
        skills = _skills_dir(root_fd, create=False)
        if skills is None:
            return 'absent', None
        try:
            return _observe(skills, name)
        finally:
            os.close(skills)
    finally:
        os.close(root_fd)


def _occupied(name, kind, text):
    if kind == 'symlink':
        return StoreError('skill_link_occupied', 'replace_symlink_target', text,
                          f'the exact current link text, passed to replace it with {link_target(name)}',
                          'Pass replace_symlink_target with this exact text to replace the link after the user confirms, or leave it.')
    return StoreError('skill_link_collision', 'path', link_path(name), f'no {kind} at the link path',
                      f'A {kind} is never replaced or removed by this tool: move its content into '
                      f'.agents/skills/{name} and ask the user to remove it.')


def apply(root, args, effects):
    """Perform ``args['action']``; ``effects['possible']`` is set just before
    the one mutating syscall, so a later failure is reported as uncertain."""
    name = check_name(args['name'])
    target = link_target(name)
    replace = args.get('replace_symlink_target')
    root_fd = os.open(root, _DIR_FLAGS)
    try:
        if args['action'] == 'unlink':
            skills = _skills_dir(root_fd, create=False)
            if skills is None:
                return dict(action='unlink', path=link_path(name), target=target, changed=False, previous=None)
            try:
                kind, text = _observe(skills, name)
                if kind == 'absent':
                    return dict(action='unlink', path=link_path(name), target=target, changed=False, previous=None)
                if kind != 'symlink' or text != target:
                    raise StoreError('skill_link_not_managed', 'path', link_path(name),
                                     f'a symlink to exactly {target}',
                                     f'Only the exact managed link is removed; this entry is a {kind}'
                                     + (f' to {text}' if text else '') + '. Leave it or ask the user.')
                effects['possible'] = True
                os.unlink(name, dir_fd=skills)
                os.fsync(skills)
                return dict(action='unlink', path=link_path(name), target=target, changed=True,
                            previous=dict(kind='symlink', target=text))
            finally:
                os.close(skills)
        _verify_source(root_fd, name)
        skills = _skills_dir(root_fd, create=True, effects=effects)
        try:
            kind, text = _observe(skills, name)
            if kind == 'symlink' and text == target:
                return dict(action='link', path=link_path(name), target=target, changed=False, previous=None)
            if kind == 'absent':
                effects['possible'] = True
                os.symlink(target, name, dir_fd=skills)
                os.fsync(skills)
                return dict(action='link', path=link_path(name), target=target, changed=True, previous=None)
            if kind != 'symlink' or replace != text:
                if kind == 'symlink' and replace is not None:
                    raise StoreError('skill_link_conflict', 'replace_symlink_target', replace, text,
                                     'The link changed or the text is wrong; nothing was replaced.')
                raise _occupied(name, kind, text)
            temporary = '.cortex-link-' + uuid.uuid4().hex
            effects['possible'] = True
            os.symlink(target, temporary, dir_fd=skills)
            try:
                # Only ever a symlink over the confirmed symlink (os.replace
                # of a link never follows either name).
                os.replace(temporary, name, src_dir_fd=skills, dst_dir_fd=skills)
                os.fsync(skills)
            finally:
                try:
                    os.unlink(temporary, dir_fd=skills)
                except FileNotFoundError:
                    pass
            return dict(action='link', path=link_path(name), target=target, changed=True,
                        previous=dict(kind='symlink', target=text))
        finally:
            os.close(skills)
    finally:
        os.close(root_fd)


def git_retention(root, name):
    """Whether the project's Git would ignore the link or its skill source.

    Observed with the read-only sandboxed ``git check-ignore`` of
    ``delegation.revision.sandboxed_git``.
    ``None`` values: not a Git checkout, or the check could not run.
    """
    from ..delegation.revision import sandboxed_git
    paths = [link_path(name), f'.agents/skills/{name}/SKILL.md']
    result = dict(link_ignored=None, source_ignored=None,
                  scope='root and nested .gitignore, info/exclude, core.excludesFile')
    try:
        out = sandboxed_git(str(root), ['check-ignore', '--no-index', '-v', '-n', '--', *paths], ok_codes=(0, 1))
    except Exception:  # noqa: BLE001 - an observation only; the link already exists
        out = None
    if out is None:
        return result
    ignored = {}
    for line in out.decode('utf-8', 'replace').splitlines():
        source, _, path = line.partition('\t')
        pattern = source.split(':', 2)[-1] if source.count(':') >= 2 else ''
        ignored[path] = bool(pattern) and not pattern.startswith('!')
    if set(paths) <= set(ignored):
        result.update(link_ignored=ignored[paths[0]], source_ignored=ignored[paths[1]])
    return result
