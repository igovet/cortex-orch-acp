"""Saved per-task choices, separate from machine defaults and the logical store."""
import json
import os
import hashlib
import re
import unicodedata
from pathlib import Path
import subprocess

from .storage import WorkspaceError


def git(root, *args):
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    result = subprocess.run(["git", *args], cwd=root, env=env, capture_output=True, timeout=20)
    if result.returncode:
        raise WorkspaceError("task_checkout_unavailable")
    return result.stdout.decode().strip()


def read(db, task_id):
    row = db.execute("SELECT options_json FROM workspace_task_options WHERE task_id=?", (task_id,)).fetchone()
    return json.loads(row[0]) if row else {}


def save(db, task_id, options):
    db.execute("INSERT INTO workspace_task_options VALUES (?,?) ON CONFLICT(task_id) DO UPDATE SET options_json=excluded.options_json",
               (task_id, json.dumps(options, sort_keys=True)))


def exclude_helpers(root):
    probe = subprocess.run(["git", "rev-parse", "--path-format=absolute", "--git-path", "info/exclude"],
                           cwd=root, capture_output=True, timeout=10)
    if probe.returncode:
        return
    path = Path(probe.stdout.decode().strip())
    if path.is_symlink():
        raise WorkspaceError("unsafe_git_exclude")
    path.parent.mkdir(parents=True, exist_ok=True)
    text = path.read_text() if path.exists() else ""
    for pattern in ("/.cortex/", "/.cortex-dev/", "/.cortex-dev-owner.json", "/.cortex-workspace-dev.json", "/.cortex-workspace.json"):
        if pattern not in text.splitlines():
            text += ("\n" if text and not text.endswith("\n") else "") + pattern + "\n"
    path.write_text(text)


def checkout_leaf(db, task_id):
    """A stable, readable Git name, not the public transport identity."""
    row = db.execute("SELECT description FROM workspace_tasks WHERE task_id=?", (task_id,)).fetchone()
    text = unicodedata.normalize("NFKD", row[0] if row else "").encode("ascii", "ignore").decode().lower()
    slug = re.sub(r"[^a-z0-9]+", "-", text).strip("-")[:48].rstrip("-") or "task"
    return slug + "-" + hashlib.sha256(task_id.encode()).hexdigest()[:12]


def prepare(store, task_id, environ):
    exclude_helpers(store.project_root)
    with store.connection() as db:
        options = read(db, task_id)
        leaf = checkout_leaf(db, task_id)
    checkout = options.get("checkout") or {"kind": "main"}
    try:
        git(store.project_root, "rev-parse", "--is-inside-work-tree")
    except WorkspaceError:
        if (Path(store.project_root) / ".git").exists() or (Path(store.project_root) / ".git").is_symlink():
            raise
        # A plain folder has one writable location, never a fictional worktree.
        checkout = options["checkout"] = {"kind": "main"}
        store.immediate_write(lambda db: save(db, task_id, options))
    if "baseRevision" not in options:
        try:
            options["baseRevision"] = git(store.project_root, "rev-parse", "HEAD")
        except WorkspaceError:
            if checkout["kind"] != "main":
                raise
        store.immediate_write(lambda db: save(db, task_id, options))
    if checkout["kind"] == "main":
        return store.project_root
    state = environ.get("CORTEX_WORKSPACE_STATE_DIR")
    if not state:
        raise WorkspaceError("worktree_settings_unavailable")
    settings_path = Path(state) / "machine.json"
    settings = json.loads(settings_path.read_text()) if settings_path.exists() else {}
    default = Path(environ.get("XDG_DATA_HOME", str(Path(environ["HOME"]) / ".local/share"))) / "cortex-workspace/worktrees"
    raw = settings.get("worktreeRoot", str(default))
    if raw.startswith("~/"):
        raw = str(Path(environ["HOME"]) / raw[2:])
    base = Path(raw)
    if not base.is_absolute() or base.is_symlink() or {".cortex", ".cortex-dev"}.intersection(base.parts):
        raise WorkspaceError("unsafe_worktree_root")
    base = base.resolve()
    ancestor = base
    while not ancestor.exists():
        ancestor = ancestor.parent
    probe = subprocess.run(["git", "rev-parse", "--is-inside-work-tree"], cwd=ancestor, capture_output=True, timeout=10)
    if probe.returncode == 0 or base == Path(store.project_root) or Path(store.project_root) in base.parents:
        raise WorkspaceError("unsafe_worktree_root")
    project = git(store.project_root, "rev-parse", "--path-format=absolute", "--git-common-dir")
    group = hashlib.sha256(project.encode()).hexdigest()[:16]
    # Saved locations survive default-root changes. Keep legacy names too; never
    # rename a branch or move an owner's checkout just to improve its label.
    legacy = base / group / task_id
    path = Path(checkout["path"]) if checkout.get("path") else legacy if legacy.exists() else base / group / leaf
    branch = checkout.get("branch") or "cw/" + (task_id if path == legacy else leaf)
    managed = checkout.get("createdByWorkspace", False)
    if path.exists():
        # An interrupted start is never blindly reused as a different checkout.
        if git(path, "branch", "--show-current") != branch or git(path, "rev-parse", "--path-format=absolute", "--git-common-dir") != project:
            raise WorkspaceError("task_checkout_conflict")
    else:
        path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        git(store.project_root, "worktree", "add", "-b", branch, str(path))
        managed = True
    options["checkout"] = {"kind": "worktree", "path": str(path), "branch": branch, "createdByWorkspace": managed}
    store.immediate_write(lambda db: save(db, task_id, options))
    return str(path)
