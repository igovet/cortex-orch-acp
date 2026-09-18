#!/usr/bin/env python3
"""Reversible, dry-run-by-default uninstall of Cortex-owned artifacts (T031).

Deletes *only* what Cortex itself created:

- ``$XDG_CONFIG_HOME/cortex`` (connection registry, Cortex-managed API-key
  secret files, the persisted user-default route/parameters) and
- ``$XDG_STATE_HOME/cortex`` (per-connection isolated engine-native state
  directories Cortex itself created for isolation, and the catalog
  snapshot cache),

and, only with ``--project PATH``, that one project's own ``.cortex``
store (never any other file in the project, never the project's Git
history or working changes).

Never touches: a real engine's own global install/config (there is none
Cortex owns here to begin with -- see docs/acp/packaging-and-onboarding.md
for why no engine/npm/pip artifact is ever installed by this project), the
user's actual shell/editor config, SSH/Git credentials, or anything
outside the two computed roots above.

Default is ``--dry-run`` (the implicit default with no flags at all):
lists exactly what would be removed and exits 0 without deleting
anything. Real deletion requires the specific ``--remove-*`` flag *and*
``--i-confirm-removal`` together; each scope is independent (removing the
user scope never implies removing a project's ``.cortex``, and vice
versa).
"""
from __future__ import annotations

import argparse
import os
import shutil
import stat
import sys
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPTS.parent / "src"))

from cortex_runtime.connections import registry  # noqa: E402
from cortex_runtime.portable.errors import PortableError  # noqa: E402
from cortex_runtime.project_storage import canonical_project, project_store_directory  # noqa: E402


def _owner_private_or_absent(path: Path):
    """Return ``"absent"``, ``"owner_private"``, or ``"unsafe"`` -- never
    deletes anything itself."""
    try:
        info = path.lstat()
    except FileNotFoundError:
        return "absent"
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid():
        return "unsafe"
    return "owner_private"


def _size(path: Path) -> int:
    total = 0
    for root, _dirs, files in os.walk(path, followlinks=False):
        for name in files:
            try:
                total += (Path(root) / name).lstat().st_size
            except OSError:
                pass
    return total


def plan(environ=None, project=None):
    """Read-only: describe exactly what would be removed."""
    entries = []
    try:
        config_root = registry.config_root(environ)
        entries.append(("user_config", config_root))
    except PortableError as error:
        entries.append(("user_config_error", error.code))
    try:
        state_root = registry.state_root(environ)
        entries.append(("user_state", state_root))
    except PortableError as error:
        entries.append(("user_state_error", error.code))
    if project is not None:
        canonical = canonical_project(project)
        entries.append(("project_cortex_store", project_store_directory(canonical)))

    report = []
    for label, value in entries:
        if not isinstance(value, Path):
            report.append(dict(scope=label, path=None, status=str(value), bytes=0))
            continue
        status = _owner_private_or_absent(value)
        size = _size(value) if status == "owner_private" else 0
        report.append(dict(scope=label, path=str(value), status=status, bytes=size))
    return report


def remove_user_scope(environ=None):
    """Delete ``$XDG_CONFIG_HOME/cortex`` and ``$XDG_STATE_HOME/cortex``.

    Each is independently re-verified as an owner-private, non-symlinked
    directory immediately before deletion (never trusts an earlier
    ``plan()`` call -- a TOCTOU window between plan and act is possible).
    """
    removed = []
    for getter in (registry.config_root, registry.state_root):
        path = getter(environ)
        status = _owner_private_or_absent(path)
        if status == "absent":
            continue
        if status == "unsafe":
            raise SystemExit(f"error: refusing to remove {path}: not an owner-private real directory.")
        shutil.rmtree(path)
        removed.append(str(path))
    return removed


def remove_project_store(project):
    canonical = canonical_project(project)
    directory = project_store_directory(canonical)
    status = _owner_private_or_absent(directory)
    if status == "absent":
        return None
    if status == "unsafe":
        raise SystemExit(f"error: refusing to remove {directory}: not an owner-private real directory.")
    shutil.rmtree(directory)
    return str(directory)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--project", type=Path, default=None,
                         help="Also plan/remove this one project's own .cortex store.")
    parser.add_argument("--remove-user-scope", action="store_true",
                         help="Actually delete $XDG_CONFIG_HOME/cortex and $XDG_STATE_HOME/cortex.")
    parser.add_argument("--remove-project-store", action="store_true",
                         help="Actually delete --project's own .cortex store (requires --project).")
    parser.add_argument("--i-confirm-removal", dest="confirm", action="store_true",
                         help="Required together with any --remove-* flag; a dry-run plan needs no flag at all.")
    args = parser.parse_args(argv)

    import json

    if not args.remove_user_scope and not args.remove_project_store:
        print(json.dumps(dict(dry_run=True, would_remove=plan(project=args.project)), indent=2, sort_keys=True))
        return 0

    if not args.confirm:
        print("error: --remove-user-scope/--remove-project-store requires --i-confirm-removal.", file=sys.stderr)
        return 2
    if args.remove_project_store and args.project is None:
        print("error: --remove-project-store requires --project PATH.", file=sys.stderr)
        return 2

    removed = dict()
    if args.remove_user_scope:
        removed["user_scope"] = remove_user_scope()
    if args.remove_project_store:
        removed["project_store"] = remove_project_store(args.project)
    print(json.dumps(dict(dry_run=False, removed=removed), indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
