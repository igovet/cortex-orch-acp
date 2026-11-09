#!/usr/bin/env python3
"""Machine facts and owner actions behind the Workspace service, one JSON request per run.

The service resolves the project root and state directory itself; a client never names a
path outside a checkout. Git goes through fixed argument lists, Cortex configuration through
its own management API, and the project store is opened read-only.
"""
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import sqlite3
import stat
import subprocess
import sys
import tempfile
import time
from urllib.parse import quote

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "src"))
sys.path.insert(0, str(Path(__file__).resolve().parent))

from cortex_runtime.project_storage import project_store_directory

FRAME_BUDGET = 900_000
GIT_TIMEOUT = 25
MAX_TREE = 20_000
MAX_DIFF_BYTES = 400_000
MAX_FILE_BYTES = 512 * 1024
ENGINES = ("claude-agent-acp", "codex-acp", "opencode")
RESERVED = {".git", ".cortex", ".cortex-dev", ".cortex-dev-owner.json", ".cortex-workspace-dev.json", ".cortex-workspace.json"}
DEFAULT_MACHINE = {"name": "This machine", "retentionDays": 30, "autoArchiveDays": 7, "worktreeRoot": str(Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share"))) / "cortex-workspace/worktrees")}
DAY_MS = 86_400_000


class Refused(Exception):
    def __init__(self, code, message):
        super().__init__(code)
        self.code, self.message = code, message


def now_ms():
    return int(time.time() * 1000)


# --- private state -------------------------------------------------------------

def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    handle, name = tempfile.mkstemp(dir=path.parent, prefix=".tmp-")
    try:
        with os.fdopen(handle, "w", encoding="utf-8") as out:
            json.dump(value, out, ensure_ascii=False, sort_keys=True)
        os.replace(name, path)
    except BaseException:
        if os.path.exists(name):
            os.unlink(name)
        raise


def read_json(path, default):
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return default
    except ValueError:
        raise Refused("state_unreadable", "Saved Workspace state could not be read.") from None


class Locked:
    """One writer at a time, because the desktop and an automatic run can act together."""

    def __init__(self, state, name="machine.lock"):
        self.path = Path(state) / name

    def __enter__(self):
        self.path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        self.handle = open(self.path, "w", encoding="utf-8")
        fcntl.flock(self.handle, fcntl.LOCK_EX)

    def __exit__(self, *_):
        self.handle.close()


def machine_settings(state):
    return {**DEFAULT_MACHINE, **read_json(Path(state) / "machine.json", {})}


def product_path(state, project):
    if not re.fullmatch(r"[A-Za-z0-9_:.-]{1,128}", project or ""):
        raise Refused("invalid_project", "Unknown project.")
    return Path(state) / "product" / (project + ".json")


def product_of(state, project):
    return {"tasks": {}, "sprints": [], "drafts": {}, "messages": [], "comments": {}, **read_json(product_path(state, project), {})}


def conversation_action(project, state, params, *, comments=False):
    with Locked(state):
        product = product_of(state, project)
        task = params.get("taskId")
        if comments and (not isinstance(task, str) or not task):
            raise Refused("unknown_task", "Unknown task.")
        rows = product["comments"].setdefault(task, []) if comments else product["messages"]
        action, key = params.get("action"), params.get("commentId" if comments else "messageId")
        if action == "add":
            item = params.get("comment" if comments else "message")
            if not isinstance(item, dict) or not isinstance(item.get("id"), str) or not isinstance(item.get("text"), str) or not item["text"].strip() or len(item["text"]) > 32000:
                raise Refused("invalid_message", "Write a message first, up to 32000 characters.")
            if not comments and (item.get("delivery") != "queued" or item.get("workerId") is not None):
                raise Refused("invalid_message", "Only unsent task messages go into the queue.")
            if not comments:
                from cortex_runtime.workspace_api.attachments import checked
                checked(item.get("attachments", []))
            old = next((r for r in rows if r["id"] == item["id"]), None)
            if old and old != item:
                raise Refused("payload_conflict", "This message identity already has different content.")
            if not old:
                rows.append(item)
        elif action == "mark_sent" and comments:
            ids = params.get("ids") or []
            for row in rows:
                if row["id"] in ids:
                    row["sent"] = True
        else:
            index = next((i for i, row in enumerate(rows) if row["id"] == key), None)
            if index is None:
                raise Refused("unknown_message", "This item is no longer here.")
            if comments and rows[index].get("sent"):
                raise Refused("already_sent", "Sent comments cannot be changed.")
            if action == "remove":
                rows.pop(index)
            elif action == "move" and not comments:
                before_id = params.get("before")
                target = next((row for row in rows if row["id"] == before_id), None)
                if before_id == key:
                    return rows
                if before_id is not None and (target is None or target["taskId"] != rows[index]["taskId"]):
                    raise Refused("unknown_message", "The destination changed or belongs to another task. Refresh the queue.")
                item = rows.pop(index)
                before = next((i for i, row in enumerate(rows) if row["id"] == before_id), len(rows))
                rows.insert(before, item)
            elif action == "edit" and comments and isinstance(params.get("text"), str) and params["text"].strip() and len(params["text"]) <= 32000:
                rows[index]["text"] = params["text"]
            else:
                raise Refused("unsupported_action", "This action is not available.")
        if len(json.dumps(product).encode()) > FRAME_BUDGET:
            raise Refused("queue_full", "Saved conversation state is full. Remove queued items first.")
        write_json(product_path(state, project), product)
        return rows


# --- Git ------------------------------------------------------------------------

def git_env():
    env = dict(os.environ, GIT_TERMINAL_PROMPT="0", LC_ALL="C", GIT_OPTIONAL_LOCKS="0")
    env.pop("GIT_DIR", None)
    env.pop("GIT_WORK_TREE", None)
    return env


# These commands inspect only. Mutations retain the owner's normal filter/hook policy.
READ_GIT = {"status", "diff", "log", "show", "ls-files", "check-attr", "rev-list"}
_FILTER_CONFIG = {}


def inspection_config(cwd):
    """Per-command raw inspection, even when required filter executables are absent."""
    key = str(Path(cwd).resolve())
    if key not in _FILTER_CONFIG:
        # Read names, never command values. Command-line settings override local, included,
        # global and inherited settings without rewriting any of them.
        found = subprocess.run(["git", "config", "--null", "--name-only", "--get-regexp", r"^filter\."],
                               cwd=cwd, env=git_env(), capture_output=True, timeout=GIT_TIMEOUT)
        if found.returncode not in (0, 1):
            raise Refused("git_failed", "Git filter settings could not be inspected.")
        drivers = {name.rsplit(".", 1)[0] for name in found.stdout.decode("utf-8", "replace").split("\0") if name}
        _FILTER_CONFIG[key] = [part for driver in sorted(drivers)
                               for setting, value in (("clean", ""), ("smudge", ""), ("process", ""), ("required", "false"))
                               for part in ("-c", f"{driver}.{setting}={value}")]
    return ["-c", "core.hooksPath=/dev/null", "-c", "core.fsmonitor=false",
            "-c", "core.untrackedCache=false", *_FILTER_CONFIG[key]]


def git(cwd, *args, check=True, timeout=GIT_TIMEOUT, stdin=None):
    try:
        config = inspection_config(cwd) if args and args[0] in READ_GIT else []
        if args and args[0] in {"diff", "log", "show"}:
            args = (args[0], "--no-ext-diff", "--no-textconv", *args[1:])
        argv = ["git", "-c", "core.quotepath=false", "-c", "color.ui=false", *config, *args]
        done = subprocess.run(argv, cwd=cwd, env=git_env(), input=stdin, capture_output=True, timeout=timeout)
    except subprocess.TimeoutExpired:
        raise Refused("git_timeout", "Git did not answer in time.") from None
    except FileNotFoundError:
        raise Refused("git_unavailable", "Git is not installed on this machine.") from None
    if check and done.returncode:
        raise Refused("git_failed", (done.stderr or done.stdout).decode("utf-8", "replace").strip()[:600] or "Git refused.")
    return done


def out(cwd, *args, **kwargs):
    return git(cwd, *args, **kwargs).stdout.decode("utf-8", "replace")


def is_repo(root):
    return git(root, "rev-parse", "--is-inside-work-tree", check=False).returncode == 0


def clean_remote(url):
    return re.sub(r"//[^/@]+@", "//", url.strip())


def default_branch(root):
    head = git(root, "symbolic-ref", "--short", "refs/remotes/origin/HEAD", check=False).stdout.decode().strip()
    if head:
        return head.split("/", 1)[-1]
    locals_ = out(root, "for-each-ref", "--format=%(refname:short)", "refs/heads").split()
    for name in ("main", "master"):
        if name in locals_:
            return name
    return locals_[0] if locals_ else ""


def worktrees(root):
    rows, entry = [], {}
    for line in out(root, "worktree", "list", "--porcelain").splitlines() + [""]:
        if not line:
            if entry:
                rows.append(entry)
            entry = {}
        elif line.startswith("worktree "):
            entry["path"] = line[9:]
        elif line.startswith("branch "):
            entry["branch"] = line[7:].removeprefix("refs/heads/")
        elif line == "detached":
            entry["branch"] = ""
    return rows


def checkout_id(project, path, main):
    return project + ":main" if main else project + ":wt:" + hashlib.sha256(path.encode()).hexdigest()[:16]


def resolve_checkout(root, project, wanted):
    if not is_repo(root):
        if wanted == project + ":main":
            return str(root)
        raise Refused("unknown_checkout", "This folder has no separate Git worktrees.")
    for index, tree in enumerate(worktrees(root)):
        if checkout_id(project, tree["path"], index == 0) == wanted:
            return tree["path"]
    raise Refused("unknown_checkout", "That checkout is not part of this project.")


def relative(file):
    if not isinstance(file, str) or not file or file.startswith("/") or "\0" in file or "\\" in file or len(file) > 4096:
        raise Refused("invalid_path", "That path is not inside this checkout.")
    parts = file.split("/")
    if any(part in {"", ".", ".."} for part in parts) or parts[0] in RESERVED:
        raise Refused("invalid_path", "That path is not inside this checkout.")
    return file


def inside(checkout, file):
    path, base = (Path(checkout) / relative(file)).resolve(), Path(checkout).resolve()
    if base not in path.parents:
        raise Refused("invalid_path", "That path is not inside this checkout.")
    return path


HUNK = re.compile(r"^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@")


def parse_diff(text, staged):
    """Unified diff text to the file records the Code and Changes views read."""
    files, current, hunk, old, new = {}, None, None, 0, 0
    for line in text.split("\n"):
        if line.startswith("diff --git "):
            current, hunk = None, None
        elif line.startswith("+++ ") and current is None:
            name = line[4:].split("\t")[0]
            current = name[2:] if name.startswith("b/") else None
        elif line.startswith("--- ") and current is None:
            name = line[4:].split("\t")[0]
            if name.startswith("a/"):
                files.setdefault("__old__", name[2:])
        elif line.startswith("@@"):
            match = HUNK.match(line)
            if match is None:
                continue
            key = current or files.get("__old__")
            if key is None:
                continue
            record = files.setdefault(key, {"hunks": [], "add": 0, "del": 0})
            old, new = int(match[1]), int(match[2])
            hunk = {"header": line, "lines": [], "staged": staged}
            record["hunks"].append(hunk)
            files.pop("__old__", None)
            current = key
        elif hunk is not None and line[:1] in {" ", "+", "-"}:
            kind = {" ": "ctx", "+": "add", "-": "del"}[line[0]]
            hunk["lines"].append({"kind": kind, "old": None if kind == "add" else old, "new": None if kind == "del" else new, "text": line[1:]})
            old += kind != "add"
            new += kind != "del"
            files[current]["add" if kind == "add" else "del"] += kind != "ctx"
    files.pop("__old__", None)
    return files


UNTRACKED_SKIP = ("node_modules", ".venv", "__pycache__")


def listed_files(path, *, untracked_only=False):
    args = ["ls-files", "-z", "--others", "--exclude-standard"]
    if not untracked_only:
        args.append("--cached")
    # Git prunes these directories before traversing them. Tracked files stay visible.
    args.extend(f"--exclude={name}/" for name in (*UNTRACKED_SKIP, *sorted(RESERVED)))
    return [name for name in out(path, *args).split("\0") if name]


def file_filters(path, names):
    if not names:
        return {}
    raw = git(path, "check-attr", "-z", "--stdin", "filter",
              stdin=("\0".join(names) + "\0").encode()).stdout.decode("utf-8", "replace").split("\0")
    return {raw[i]: raw[i + 2] for i in range(0, len(raw) - 2, 3)
            if raw[i + 2] not in {"unspecified", "unset"}}


def display_diff(path, *args):
    names = out(path, "diff", "--name-only", "-z", *args).split("\0")
    filters = file_filters(path, [name for name in names if name])
    exclusions = [f":(literal,exclude){name}" for name in filters]
    return out(path, "diff", "--unified=3", *args, "--", ".", *exclusions)


def status_files(path):
    """Raw status, never normalized through clean filters; filtered content has no hunks."""
    raw = git(path, "status", "--porcelain=v2", "-z", "--untracked-files=no").stdout.decode("utf-8", "replace")
    entries, items = raw.split("\0"), []
    index = 0
    while index < len(entries):
        entry = entries[index]
        index += 1
        if not entry:
            continue
        kind = entry[0]
        if kind == "?":
            items.append(("untracked", "U", entry[2:]))
        elif kind == "u":
            items.append(("unstaged", "M", entry.split(" ", 10)[-1]))
        elif kind in "12":
            parts = entry.split(" ", 9 if kind == "2" else 8)
            name = parts[-1]
            if kind == "2":
                index += 1
            xy = parts[1]
            letter = {"A": "A", "D": "D"}
            if xy[0] != ".":
                items.append(("staged", letter.get(xy[0], "M"), name))
            if xy[1] != ".":
                items.append(("unstaged", letter.get(xy[1], "M"), name))
    items.extend(("untracked", "U", name) for name in listed_files(path, untracked_only=True))
    filters = file_filters(path, sorted({name for _, _, name in items}))
    exclusions = [f":(literal,exclude){name}" for name in filters]
    scope = ["--", ".", *exclusions]
    plain = out(path, "diff", "--unified=3", *scope)
    cached = out(path, "diff", "--cached", "--unified=3", *scope)
    keep = len(plain) + len(cached) <= MAX_DIFF_BYTES
    unstaged = parse_diff(plain, False) if keep else {}
    staged = parse_diff(cached, True) if keep else {}
    counts = {}
    for args, stage in ((("diff", "--numstat", "-z"), "unstaged"), (("diff", "--cached", "--numstat", "-z"), "staged")):
        for row in out(path, *args, *scope).split("\0"):
            pieces = row.split("\t")
            if len(pieces) == 3:
                counts[(stage, pieces[2])] = (int(pieces[0]) if pieces[0].isdigit() else 0, int(pieces[1]) if pieces[1].isdigit() else 0)
    files = []
    for stage, letter, name in items:
        if name.split("/", 1)[0] in RESERVED:
            continue
        record = (staged if stage == "staged" else unstaged).get(name, {})
        if stage == "untracked" and name not in filters:
            file = Path(path) / name
            # Never follow a symlink for preview. Keep the existing shared frame budget.
            if not file.is_symlink() and file.is_file() and file.stat().st_size <= MAX_DIFF_BYTES:
                raw = file.read_bytes()
                try:
                    text = raw.decode("utf-8")
                except UnicodeError:
                    text = ""
                if "\0" not in text and text:
                    lines = text.splitlines()
                    record = {"add": len(lines), "del": 0, "hunks": []}
                    if keep and len(raw) + len(plain.encode()) + len(cached.encode()) <= MAX_DIFF_BYTES:
                        record["hunks"] = [{"header": f"@@ -0,0 +1,{len(lines)} @@", "staged": False,
                            "lines": [{"kind": "add", "old": None, "new": i + 1, "text": line} for i, line in enumerate(lines)]}]
                        plain += text
        add, removed = counts.get((stage, name), (record.get("add", 0), record.get("del", 0)))
        files.append({"path": name, "status": letter, "add": add, "del": removed, "hunks": record.get("hunks", []), "stage": stage, **({"filter": filters[name]} if name in filters else {})})
    return files


def track(text):
    ahead = re.search(r"ahead (\d+)", text or "")
    behind = re.search(r"behind (\d+)", text or "")
    return (int(ahead[1]) if ahead else 0, int(behind[1]) if behind else 0)


def commits(root):
    unpushed = set()
    has_upstream = git(root, "rev-parse", "--verify", "--quiet", "@{u}", check=False).returncode == 0
    if has_upstream:
        unpushed = set(out(root, "rev-list", "@{u}..HEAD").split())
    if git(root, "rev-parse", "--verify", "--quiet", "HEAD", check=False).returncode:
        return []
    rows, line = [], {"paths": [{"cls": "g1", "d": "M 12 0 V 34"}], "dot": {"cls": "d1", "x": 12}}
    text = out(root, "log", "-n", "100", "--date=relative", "--format=%x1e%H%x00%h%x00%s%x00%an%x00%ar%x00%D", "--shortstat")
    for block in text.split("\x1e"):
        if not block.strip():
            continue
        head, _, tail = block.partition("\n")
        fields = head.split("\0")
        if len(fields) != 6:
            continue
        full, sha, message, author, when, refs = fields
        stats = re.search(r"(\d+) files? changed(?:, (\d+) insertions?\(\+\))?(?:, (\d+) deletions?\(-\))?", tail)
        tags = [ref[5:] for ref in refs.split(", ") if ref.startswith("tag: ")]
        branch = next((ref.split(" -> ")[-1] for ref in refs.split(", ") if ref and not ref.startswith("tag: ") and not ref.startswith("origin/")), "")
        rows.append({"sha": sha, "message": message, "author": author, "when": when, "branch": branch, "taskId": None, "tags": tags,
                     "files": int(stats[1]) if stats else 0, "add": int(stats[2] or 0) if stats else 0, "del": int(stats[3] or 0) if stats else 0,
                     "pushed": has_upstream and full not in unpushed, "graph": line})
    return rows


def repo_snapshot(root, project, state):
    if not is_repo(root):
        tree = []
        for folder, dirs, files in os.walk(root, followlinks=False):
            dirs[:] = sorted(d for d in dirs if d not in RESERVED and d not in UNTRACKED_SKIP
                             and not (Path(folder) / d).is_symlink())
            tree.extend(str((Path(folder) / f).relative_to(root)) for f in sorted(files) if f not in RESERVED)
            if len(tree) > MAX_TREE:
                break
        return {"projectId": project, "hasGit": False, "remote": "", "tree": sorted(tree)[:MAX_TREE],
                "treeTruncated": len(tree) > MAX_TREE, "sources": {}, "commits": [], "branches": [],
                "checkouts": [{"id": project + ":main", "projectId": project, "kind": "main", "branch": "",
                               "path": str(root), "taskId": None, "label": "Project folder", "base": "", "upstream": "",
                               "ahead": 0, "behind": 0, "behindOf": {}, "files": [], "draft": "", "sources": {},
                               "updateConflict": None}],
                "taskDiffs": {task: {"files": [], "reason": "This folder has no Git history. Review files in Code; result acceptance is still available."}
                              for task in product_read(root, state, project)["tasks"]},
                "fetchedAt": None, "openBranch": "", "defaultBranch": ""}
    product = product_read(root, state, project, review=False)
    drafts = product.get("drafts", {})
    main_branch = out(root, "branch", "--show-current").strip()
    target = default_branch(root)
    remote = git(root, "remote", "get-url", "origin", check=False).stdout.decode("utf-8", "replace")
    branches, upstream = [], {}
    for row in out(root, "for-each-ref", "--format=%(refname:short)%00%(upstream:short)%00%(upstream:track,nobracket)", "refs/heads").splitlines():
        name, up, tracked = (row.split("\0") + ["", ""])[:3]
        ahead, behind = track(tracked)
        upstream[name] = (up, ahead, behind)
        branches.append({"name": name, "where": up, "task": None, "ahead": ahead, "behind": behind})
    checkouts = []
    for index, tree in enumerate(worktrees(root)):
        branch = tree.get("branch", "")
        identity = checkout_id(project, tree["path"], index == 0)
        task_id = next((task for task, record in product["tasks"].items() if record.get("checkout", {}).get("path") == tree["path"]), None)
        up, ahead, behind = upstream.get(branch, ("", 0, 0))
        files = status_files(tree["path"]) if Path(tree["path"]).is_dir() else []
        base = up or target
        if not up and target and branch and branch != target:
            counted = git(tree["path"], "rev-list", "--left-right", "--count", target + "..." + branch, check=False)
            if counted.returncode == 0:
                behind, ahead = (int(n) for n in counted.stdout.split())
        checkouts.append({"id": identity, "projectId": project, "kind": "main" if index == 0 else "worktree", "branch": branch, "path": tree["path"],
                          "taskId": task_id, "label": None, "base": base, "upstream": up, "ahead": ahead, "behind": behind, "behindOf": {},
                          "files": files, "draft": drafts.get(identity, ""), "sources": {}, "updateConflict": None})
    task_diffs = {}
    for task, record in product["tasks"].items():
        if not record.get("baseRevision"):
            continue
        import task_git
        removed = removed_checkout(record)
        path = str(root) if removed else task_git.task_path(sys.modules[__name__], root, record)
        revisions = [record["baseRevision"], record["integration"]["sourceHead"]] if removed else [record["baseRevision"]]
        diff = display_diff(path, *revisions)
        if len(diff.encode()) > MAX_DIFF_BYTES:
            task_diffs[task] = {"files": [], "reason": "The task diff exceeds the display limit; review it in Git."}
            continue
        files = [{"path": name, "status": "M", **data} for name, data in parse_diff(diff, False).items()]
        if not removed:
            files.extend({k:v for k,v in f.items() if k != "stage"} for f in status_files(path) if f["stage"] == "untracked")
        task_diffs[task] = {"files": files}
    listing = listed_files(root)
    tree = sorted({name for name in listing if name and name.split("/", 1)[0] not in RESERVED})
    truncated = len(tree) > MAX_TREE
    fetched = None
    marker = git(root, "rev-parse", "--git-path", "FETCH_HEAD", check=False).stdout.decode().strip()
    if marker and (Path(root) / marker).is_file():
        fetched = int((Path(root) / marker).stat().st_mtime * 1000)
    snapshot = {"projectId": project, "hasGit": True, "remote": clean_remote(remote), "tree": tree[:MAX_TREE], "treeTruncated": truncated, "sources": {}, "commits": commits(root),
                "branches": branches, "checkouts": checkouts, "taskDiffs": task_diffs, "fetchedAt": fetched, "openBranch": main_branch, "defaultBranch": target}
    while len(json.dumps(snapshot)) > FRAME_BUDGET:
        heavy = [file for checkout in snapshot["checkouts"] for file in checkout["files"] if file["hunks"]]
        if heavy:
            for file in heavy:
                file["hunks"] = []
            continue
        if len(snapshot["tree"]) < 100:
            raise Refused("repo_too_large", "This project is too large to read in one pass.")
        snapshot["tree"] = snapshot["tree"][:len(snapshot["tree"]) // 2]
        snapshot["treeTruncated"] = True
    return snapshot


def revision_of(data):
    return hashlib.sha256(data).hexdigest()


# The runtime's own record decides quiescence. A paused task is one the runtime has confirmed stopped.
QUIET_STATES = ("COMPLETED", "FAILED", "CANCELLED", "PAUSED")
UNCONFIRMED = "unconfirmed"


def task_checkout(root, state, task, options):
    checkout = options.get("checkout") or {"kind": "main"}
    if checkout.get("kind") == "main":
        return Path(root).resolve()
    if checkout.get("path"):
        return Path(checkout["path"]).resolve()
    common = out(root, "rev-parse", "--path-format=absolute", "--git-common-dir").strip()
    group = hashlib.sha256(common.encode()).hexdigest()[:16]
    return (Path(machine_settings(state)["worktreeRoot"]).expanduser() / group / task).resolve()


def writer_in(root, project, state, checkout):
    """The task whose runtime has not confirmed it stopped writing in this checkout, or None.

    An unreadable record is not evidence of quiet, so it answers UNCONFIRMED rather than None.
    """
    try:
        db = store_reader(root)
        if db is None:
            return None
        try:
            tables = {row[0] for row in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            if not {"workspace_tasks", "workspace_executions", "runtime_tasks"} <= tables:
                return None
            options = "o.options_json" if "workspace_task_options" in tables else "NULL"
            joined = "LEFT JOIN workspace_task_options o USING(task_id)" if options != "NULL" else ""
            quiet = ",".join("?" * len(QUIET_STATES))
            # A start that has no runtime row yet is about to write, so it counts as held.
            rows = db.execute(f"SELECT t.task_id,{options} AS options_json FROM workspace_tasks t "
                              f"LEFT JOIN workspace_executions e ON e.execution_id=t.execution_id "
                              f"LEFT JOIN runtime_tasks r ON r.task_id=e.runtime_task_id {joined} "
                              f"WHERE t.project_id=? AND t.execution_id IS NOT NULL AND (r.state IS NULL OR r.state NOT IN ({quiet})) "
                              f"ORDER BY t.sequence", (project, *QUIET_STATES)).fetchall()
        finally:
            db.close()
        here = Path(checkout).resolve()
        for row in rows:
            if task_checkout(root, state, row["task_id"], loads(row["options_json"]) if row["options_json"] else {}) == here:
                return row["task_id"]
        return None
    except (sqlite3.Error, OSError, Refused, ValueError):
        return UNCONFIRMED


def confined_parent(checkout, file):
    """The open folder holding `file`, reached one name at a time so no link can lead elsewhere.

    The descriptor stays valid if a folder on the way is swapped afterwards, which a resolved path would not.
    """
    parts = relative(file).split("/")
    if any(part in RESERVED for part in parts):
        raise Refused("invalid_path", "That path is not inside this checkout.")
    folder = os.open(checkout, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        for part in parts[:-1]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC, dir_fd=folder)
            os.close(folder)
            folder = child
    except OSError as error:
        os.close(folder)
        if error.errno in (errno.ELOOP, errno.ENOTDIR):
            raise Refused("symlink_refused", "This path goes through a link. Open the file at its real location.") from None
        if error.errno == errno.ENOENT:
            raise Refused("folder_missing", "The folder of this file is gone. Your text is kept here.") from None
        raise
    return folder, parts[-1]


def held_bytes(folder, name):
    """What the file holds now, or None when it is absent. It is read without following a link."""
    try:
        handle = os.open(name, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC | os.O_NONBLOCK, dir_fd=folder)
    except FileNotFoundError:
        return None
    except OSError as error:
        if error.errno == errno.ELOOP:
            raise Refused("symlink_refused", "This file is a link. Open the file it points to.") from None
        raise
    with os.fdopen(handle, "rb") as source:
        info = os.fstat(source.fileno())
        if not stat.S_ISREG(info.st_mode):
            raise Refused("not_a_file", "Only regular files can be edited here.")
        if info.st_size > MAX_FILE_BYTES:
            raise Refused("file_too_large", "This file is too large to edit here.")
        return source.read(MAX_FILE_BYTES + 1)


def edit_refusal(checkout, file, data):
    """Why the file that was just read cannot be edited, or None. Reading it stays allowed."""
    try:
        folder, name = confined_parent(checkout, file)
    except Refused as error:
        return error.message
    try:
        held_bytes(folder, name)
    except Refused as error:
        return error.message
    finally:
        os.close(folder)
    try:
        data.decode("utf-8")
    except UnicodeDecodeError:
        return "This file is not UTF-8 text, so saving it here would change its bytes."
    return None


def repo_file(root, project, state, params):
    checkout = resolve_checkout(root, project, params.get("checkoutId") or project + ":main")
    path = inside(checkout, params.get("file"))
    driver = file_filters(checkout, [params["file"]]).get(params["file"]) if is_repo(checkout) else None
    if driver:
        label = "Git LFS" if driver == "lfs" else f"Git filter '{driver}'"
        return {"file": params["file"], "text": None, "revision": None,
                "reason": f"{label}-managed file. Content comparison and editing are unavailable here; no filter is run."}
    if not path.is_file():
        return {"file": params["file"], "text": None, "revision": None, "reason": "This file is not in the checkout."}
    if path.stat().st_size > MAX_FILE_BYTES:
        return {"file": params["file"], "text": None, "revision": None, "reason": "This file is too large to show here."}
    data = path.read_bytes()
    if b"\0" in data:
        return {"file": params["file"], "text": None, "revision": None, "reason": "This is a binary file."}
    text = data.decode("utf-8", "replace")
    if len(json.dumps(text, ensure_ascii=False).encode()) > FRAME_BUDGET:
        return {"file": params["file"], "text": None, "revision": None, "reason": "This file is too large to show here."}
    return {"file": params["file"], "text": text, "revision": revision_of(data), "size": len(data),
            "readOnly": edit_refusal(checkout, params["file"], data), "writer": writer_in(root, project, state, checkout)}


def conflict_with(current):
    return {"outcome": "conflict", "revision": None if current is None else revision_of(current)}


def replace_held(folder, name, data, expected, mode):
    """Puts `data` in place of the file only while it still holds `expected`. Otherwise returns the conflict it found."""
    temporary = ".cw-save-" + os.urandom(8).hex() + ".tmp"
    handle = os.open(temporary, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC, 0o666 if mode is None else 0o600, dir_fd=folder)
    try:
        with os.fdopen(handle, "wb") as target:
            target.write(data)
            target.flush()
            if mode is not None:
                os.fchmod(target.fileno(), mode)
            os.fsync(target.fileno())
        # The file can move while the new copy is written, so the base is checked once more at the last moment.
        current = held_bytes(folder, name)
        if current != expected:
            return conflict_with(current)
        if expected is None:
            try:
                os.link(temporary, name, src_dir_fd=folder, dst_dir_fd=folder, follow_symlinks=False)
            except FileExistsError:
                return conflict_with(held_bytes(folder, name))
        else:
            os.rename(temporary, name, src_dir_fd=folder, dst_dir_fd=folder)
        os.fsync(folder)
        return None
    finally:
        try:
            os.unlink(temporary, dir_fd=folder)
        except FileNotFoundError:
            pass


def repo_file_save(root, project, state, params):
    """One owner save, bound to the revision the owner last saw. It never writes over anything newer."""
    file, text, base = params.get("file"), params.get("text"), params.get("revision", False)
    if not isinstance(text, str) or not (base is None or isinstance(base, str)):
        raise Refused("invalid_request", "This save has no text or no base revision.")
    try:
        data = text.encode("utf-8")
    except UnicodeEncodeError:
        raise Refused("invalid_text", "This text cannot be stored as UTF-8.") from None
    if len(data) > MAX_FILE_BYTES:
        raise Refused("file_too_large", "This file is too large to save here.")
    if b"\0" in data:
        raise Refused("binary_file", "This text holds a NUL byte, which would turn the file binary.")
    checkout = resolve_checkout(root, project, params.get("checkoutId") or project + ":main")
    with Locked(state):
        writer = writer_in(root, project, state, checkout)
        if writer is not None:
            return {"outcome": "writer_active", "writer": writer}
        folder, name = confined_parent(checkout, file)
        try:
            current = held_bytes(folder, name)
            if current is not None and b"\0" in current:
                raise Refused("binary_file", "This is a binary file.")
            found = None if current is None else revision_of(current)
            if found != base:
                return conflict_with(current)
            if current == data:
                return {"outcome": "saved", "revision": found, "size": len(data)}
            if current is not None:
                try:
                    current.decode("utf-8")
                except UnicodeDecodeError:
                    raise Refused("not_text", "This file is not UTF-8 text, so saving it here would change its bytes.") from None
            mode = None if current is None else stat.S_IMODE(os.stat(name, dir_fd=folder, follow_symlinks=False).st_mode)
            return replace_held(folder, name, data, current, mode) or {"outcome": "saved", "revision": revision_of(data), "size": len(data)}
        finally:
            os.close(folder)


def repo_locate(root, project, params):
    """Where a checkout, or a file in it, is on this machine. Only the native bridge asks, to open it elsewhere."""
    checkout = resolve_checkout(root, project, params.get("checkoutId") or project + ":main")
    file = params.get("file")
    if file is None:
        return {"root": checkout, "path": None}
    path = inside(checkout, file)
    if not path.is_file():
        raise Refused("file_missing", "This file is not in the checkout.")
    return {"root": checkout, "path": str(path)}


def hunk_patch(path, name, index, cached):
    """The one hunk, as a patch of its own, so it can be staged or reverted alone."""
    args = ["diff", "--no-ext-diff", "--unified=3"] + (["--cached"] if cached else []) + ["--", name]
    text = out(path, *args)
    header, _, body = text.partition("\n@@")
    chunks = ["@@" + chunk for chunk in ("\n@@" + body).split("\n@@")[1:]]
    if index >= len(chunks):
        raise Refused("stale_hunk", "That change moved. Look again and retry.")
    return (header + "\n" + chunks[index].rstrip("\n") + "\n").encode()


BRANCH = re.compile(r"[A-Za-z0-9._/-]{1,200}")


def git_action(root, project, state, params):
    action = params.get("action")
    if action == "git_init":
        with Locked(state):
            if is_repo(root) or (Path(root) / ".git").exists() or (Path(root) / ".git").is_symlink():
                raise Refused("repository_exists", "This project already has a Git repository.")
            git(root, "init")
        return {"done": True, "hasGit": True}
    if not is_repo(root):
        raise Refused("git_required", "This folder has no Git repository. Initialize Git to use Git operations; tasks and review do not require it.")
    if action == "fetch":
        git(root, "fetch", "--prune", timeout=GIT_TIMEOUT)
        return {"done": True}
    if action == "branch":
        name, at = params.get("name"), params.get("at")
        if not isinstance(name, str) or not BRANCH.fullmatch(name) or git(root, "check-ref-format", "--branch", name, check=False).returncode:
            raise Refused("invalid_branch", "That is not a valid branch name.")
        git(root, "branch", name, *([at] if isinstance(at, str) and re.fullmatch(r"[0-9a-f]{4,64}", at) else []))
        return {"done": True}
    wanted = params.get("checkoutId") or project + ":main"
    path = resolve_checkout(root, project, wanted)
    if action == "draft":
        with Locked(state):
            product = product_of(state, project)
            product["drafts"][wanted] = str(params.get("message", ""))[:8000]
            write_json(product_path(state, project), product)
        return {"done": True}
    if action == "stage":
        name, hunk = relative(params.get("path")), params.get("hunk")
        if isinstance(hunk, int):
            staged = params.get("staged")
            git(path, "apply", "--cached", *([] if staged else ["-R"]), stdin=hunk_patch(path, name, hunk, not staged))
        elif params.get("staged"):
            git(path, "add", "--", name)
        else:
            git(path, "restore", "--staged", "--", name)
        return {"done": True}
    if action == "revert":
        name, hunk = relative(params.get("path")), params.get("hunk")
        if params.get("confirmed") is not True:
            raise Refused("confirmation_required", "Confirm before discarding changes.")
        if isinstance(hunk, int):
            git(path, "apply", "-R", stdin=hunk_patch(path, name, hunk, False))
        elif git(path, "ls-files", "--error-unmatch", "--", name, check=False).returncode:
            target = inside(path, name)
            if target.is_file():
                target.unlink()
        else:
            git(path, "restore", "--source=HEAD", "--staged", "--worktree", "--", name)
        return {"done": True}
    if action == "commit":
        import task_git
        if any(task_git.helper_path(p) for p in out(path, "diff", "--cached", "--name-only").splitlines()):
            raise Refused("private_index", "Unstage Workspace helper data before committing.")
        message = params.get("message")
        if not isinstance(message, str) or not message.strip():
            raise Refused("empty_message", "Write a commit message first.")
        if not out(path, "diff", "--cached", "--name-only", "-z"):
            files = sorted({relative(f["path"]) for f in status_files(path)})
            if files:
                git(path, "add", "-A", "--", *files)
        git(path, "commit", "-m", message, *(["--amend"] if params.get("amend") else []))
        with Locked(state):
            product = product_of(state, project)
            product["drafts"].pop(wanted, None)
            write_json(product_path(state, project), product)
        return {"done": True}
    if action == "pull":
        if git(path, "rev-parse", "--verify", "--quiet", "@{u}", check=False).returncode:
            raise Refused("upstream_required", "No upstream is configured. Configure a remote and tracking branch before pulling.")
        git(path, "pull", "--ff-only")
        return {"done": True}
    if action == "push":
        remote, branch = params.get("remote"), params.get("branch")
        if params.get("confirmed") is not True or not isinstance(branch, str) or not BRANCH.fullmatch(branch):
            raise Refused("confirmation_required", "Confirm the remote and branch first.")
        if remote not in out(path, "remote").split():
            raise Refused("unknown_remote", "That remote is not configured.")
        git(path, "push", remote, branch)
        return {"done": True}
    if action == "revert_commit":
        sha = params.get("sha")
        if params.get("confirmed") is not True or not isinstance(sha, str) or not re.fullmatch(r"[0-9a-f]{4,64}", sha):
            raise Refused("confirmation_required", "Confirm before reverting a commit.")
        if git(path, "diff", "--cached", "--quiet", check=False).returncode:
            raise Refused("dirty_index", "Commit or unstage your selected changes before reverting a commit.")
        git(path, "revert", "--no-commit", sha)
        git(path, "commit", "-m", "Revert commit " + sha)
        return {"done": True}
    if action == "delete_worktree":
        if wanted == project + ":main" or params.get("confirmed") is not True:
            raise Refused("confirmation_required", "Only a confirmed separate worktree can be deleted.")
        if status_files(path):
            raise Refused("dirty_worktree", "Commit or move the work yourself first; cleanup never forces deletion.")
        git(root, "worktree", "remove", path)
        return {"done": True}
    raise Refused("unsupported_action", "This machine does not support that yet.")


# --- Workers --------------------------------------------------------------------

def store_reader(root):
    path = project_store_directory(root) / "cortex.sqlite3"
    if not path.is_file():
        return None
    db = sqlite3.connect("file:" + quote(str(path), safe="/") + "?mode=ro", uri=True, timeout=5)
    db.row_factory = sqlite3.Row
    return db


def loads(text):
    try:
        value = json.loads(text)
    except (TypeError, ValueError):
        return {}
    return value if isinstance(value, dict) else {}


def iso_ms(text):
    if not isinstance(text, str):
        return None
    try:
        from datetime import datetime
        return int(datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp() * 1000)
    except ValueError:
        return None


def worker_activity(events):
    from cortex_runtime.workspace_api.history import hidden_step, merge_tool_update, tool_detail
    activity, calls = [], {}
    for number, event in enumerate(events):
        payload = loads(event["payload_json"])
        ref = payload.get("activity_ref")
        if isinstance(ref, str) and ref.startswith("tool-error:"):
            ref = "tool:" + ref[len("tool-error:"):]
        if ref and payload.get("tool_kind"):
            if ref in calls:
                first, merged = calls[ref]
                calls[ref] = (first, merge_tool_update(merged, payload))
                continue
            calls[ref] = (event, dict(payload))
        else:
            calls[f"event:{event['id']}"] = (event, payload)
    for event, payload in calls.values():
        kind = payload.get("tool_kind")
        if kind and hidden_step(payload.get("title")):
            continue
        detail = tool_detail(payload)
        title = payload.get("title") or payload.get("message") or payload.get("summary") or payload.get("text") or event["event_type"]
        if not isinstance(title, str):
            title = json.dumps(title, ensure_ascii=False)
        item = {"error": payload.get("status") == "failed" or event["event_type"] == "assignment_failed", "sequence": event["id"] * 2, "text": history_text(title), "kind": "tool" if kind else "note", "at": iso_ms(event["created_at"])}
        if kind:
            item["args"] = json.dumps(detail, ensure_ascii=False)
            if payload.get("activity_ref"):
                item["toolCallRef"] = payload["activity_ref"]
        if detail.get("command"):
            item.update(kind="command", text=history_text(detail["command"]), command={"state": "native", "exit": None, "took": None, "tail": history_text(detail.get("output", "")), "reason": None})
        elif kind == "edit" or "newText" in detail:
            item["kind"] = "edit"
            old, new = detail.get("oldText", "").splitlines(), detail.get("newText", "").splitlines()
            item["diff"] = dict(add=len(new), **{"del": len(old)}, lines=["-" + line for line in old] + ["+" + line for line in new])
        activity.append(item)
    return activity


STATUS = {"completed": "completed", "accepted": "completed", "failed": "failed", "cancelled": "cancelled", "running": "working",
          "reported": "completed", "active": "working", "dispatched": "working", "pending": "pending", "ready": "pending", "blocked": "pending"}


def worker_controls(db, runtime, channel_id, task_id, worker_id, status, events):
    activity, messages = [], []
    if not channel_id:
        return activity, messages
    for row in (r for r in events if r["type"] == "COMMAND_PROCESS_STARTED"):
        data = loads(row["payload_json"])
        ref = data.get("tool_call_ref")
        later = db.execute("SELECT type,payload_json FROM runtime_events WHERE task_id=? AND type IN "
                           "('COMMAND_PROCESS_EXITED','COMMAND_STOPPED','COMMAND_STOP_MISSED','COMMAND_STOP_REQUESTED') "
                           "AND json_extract(payload_json,'$.tool_call_ref')=? ORDER BY id DESC LIMIT 1", (runtime, ref)).fetchone()
        detail = loads(later["payload_json"]) if later else {}
        state = {"COMMAND_STOP_REQUESTED":"stopping","COMMAND_STOPPED":"stopped","COMMAND_STOP_MISSED":"failed"}.get(later["type"] if later else "")
        if state is None:
            state = "running" if not later else "passed" if detail.get("exit_code") == 0 and not detail.get("failure") else "failed"
        import shlex
        argv = data.get("argv")
        command = shlex.join(argv) if isinstance(argv, list) and all(isinstance(a, str) for a in argv) else "Command text was not recorded"
        completed = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='TOOL_COMPLETED' "
                               "AND json_extract(payload_json,'$.tool_call_ref')=? ORDER BY id DESC LIMIT 1", (runtime, ref)).fetchone()
        result = loads(completed["payload_json"]).get("result", {}) if completed else {}
        tail = result.get("output", "") if isinstance(result, dict) else ""
        activity.append(dict(sequence=row["id"] * 2 + 1, kind="command", text=history_text(command), at=iso_ms(row["created_at"]),
            toolCallRef=ref, command=dict(state=state, exit=detail.get("exit_code"), took=None, tail=history_text(tail), reason=history_text(detail["reason"]) if detail.get("reason") else None)))
    for row in (r for r in events if r["type"] == "WORKER_GUIDANCE"):
        data = loads(row["payload_json"])
        delivered = db.execute("SELECT type,created_at FROM runtime_events WHERE task_id=? AND "
                               "type IN ('WORKER_GUIDANCE_DELIVERED','WORKER_GUIDANCE_WITHDRAWN') "
                               "AND json_extract(payload_json,'$.messageEvent')=? ORDER BY id DESC LIMIT 1", (runtime, row["id"])).fetchone()
        delivery = "delivered" if delivered and delivered["type"] == "WORKER_GUIDANCE_DELIVERED" else "withdrawn" if delivered else "recorded" if status in ("working","waiting_you") else "failed"
        messages.append(dict(id=data["requestId"],taskId=task_id,workerId=worker_id,text=history_text(data["text"]),mode="send_now",
            delivery=delivery,at=iso_ms(row["created_at"]),deliveredAt=iso_ms(delivered["created_at"]) if delivered else None,
            failure="Not delivered · worker finished, stopped or replaced" if delivery == "failed" else None,
            attachments=[a["name"] for a in data.get("attachmentNames", [])]))
        activity.append(dict(sequence=row["id"] * 2 + 1,kind="message",text=history_text(data["text"]),messageId=data["requestId"],at=iso_ms(row["created_at"])))
    return activity, messages


def history_bytes(value):
    return len(json.dumps(value, ensure_ascii=False).encode("utf-8"))


def history_text(value, limit=8192):
    value = str(value)
    raw = value.encode("utf-8")
    if history_bytes(value) <= limit:
        return value
    low, high = 0, len(value)
    while low < high:
        middle = (low + high + 1) // 2
        if history_bytes(value[:middle]) <= limit - 128:
            low = middle
        else:
            high = middle - 1
    prefix = value[:low]
    return prefix + f"\n[Content clipped: first {len(prefix.encode('utf-8'))} of {len(raw)} UTF-8 bytes]"


def worker_history(db, runtime, refs, channel, task_id, worker_id, status, binding, cursor, budget):
    marks = ",".join("?" for _ in refs)
    maximum = 9223372036854775807
    before_t = db.execute("SELECT COALESCE(MAX(id),0)+1 FROM task_events WHERE task_id=?", (runtime,)).fetchone()[0]
    before_r = db.execute("SELECT COALESCE(MAX(id),0)+1 FROM runtime_events WHERE task_id=?", (runtime,)).fetchone()[0]
    if cursor is not None:
        parts = cursor.split(".") if isinstance(cursor, str) else []
        if (len(parts) != 4 or parts[:2] != ["wa2", binding]
                or any(not p.isascii() or not p.isdecimal() or len(p) > 19
                       or str(int(p)) != p or not 0 < int(p) <= maximum for p in parts[2:])):
            raise Refused("invalid_cursor", "This activity page no longer belongs to this attempt. Reopen its history.")
        before_t, before_r = map(int, parts[2:])
    task_rows = db.execute(f"SELECT id,event_type,created_at,payload_json FROM task_events WHERE task_id=? "
        f"AND assignment_ref IN ({marks}) AND id<? AND "
        "(event_type IN ('worker_result','worker_question','assignment_failed','assignment_cancelled') OR "
        "(json_valid(payload_json) AND json_extract(payload_json,'$.kind')='worker_activity')) "
        "ORDER BY id DESC LIMIT 201", (runtime, *refs, before_t)).fetchall()
    runtime_rows = db.execute("SELECT id,type,created_at,payload_json FROM runtime_events WHERE task_id=? AND id<? AND "
        "((type='COMMAND_PROCESS_STARTED' AND actor=?) OR (type='WORKER_GUIDANCE' AND "
        "json_extract(payload_json,'$.channelId')=?)) ORDER BY id DESC LIMIT 201",
        (runtime, before_r, channel, channel)).fetchall() if channel else []
    # Both tables have independent monotonically increasing ids. A continuation
    # carries BOTH consumed positions; newer inserts never enter an older page.
    pending = []
    t_index = r_index = 0
    while t_index < len(task_rows) or r_index < len(runtime_rows):
        # Merge the two id-ordered prefixes without reordering either one.
        # Timestamps captured before lock acquisition can arrive out of order;
        # sorting a namespace by time would skip/repeat ids at the next cursor.
        take_runtime = t_index == len(task_rows) or (
            r_index < len(runtime_rows) and
            (runtime_rows[r_index]["created_at"], runtime_rows[r_index]["id"] * 2 + 1) >
            (task_rows[t_index]["created_at"], task_rows[t_index]["id"] * 2))
        if take_runtime:
            pending.append((runtime_rows[r_index], 1))
            r_index += 1
        else:
            pending.append((task_rows[t_index], 0))
            t_index += 1
    activity, messages, consumed = [], [], 0
    def continuation(t, r):
        return f"wa2.{binding}.{t}.{r}"
    for row, namespace in pending[:200]:
        if namespace:
            added, said = worker_controls(db, runtime, channel, task_id, worker_id, status, [row])
        else:
            payload = loads(row["payload_json"])
            ref = payload.get("activity_ref")
            related = [row]
            if ref and payload.get("tool_kind"):
                # Recover split input/status updates even across a page boundary.
                canonical = "tool:" + ref[len("tool-error:"):] if ref.startswith("tool-error:") else ref
                error_ref = "tool-error:" + canonical[len("tool:"):] if canonical.startswith("tool:") else canonical
                call_sql = ("SELECT id,event_type,created_at,payload_json FROM task_events WHERE task_id=? "
                    "AND assignment_ref IN (" + marks + ") AND json_extract(payload_json,'$.activity_ref') IN (?,?) "
                    "AND id<? ")
                args = (runtime, *refs, canonical, error_ref, before_t)
                related = db.execute(call_sql + "ORDER BY id DESC LIMIT 100", args).fetchall()[::-1]
                initial = db.execute(call_sql + "ORDER BY id LIMIT 1", args).fetchone()
                if initial and initial["id"] != related[0]["id"]:
                    related.insert(0, initial)
            if len(related) >= 100:
                latest = dict(related[-1]); payload = loads(latest["payload_json"]); payload["projection_partial"] = True
                latest["payload_json"] = json.dumps(payload); related[-1] = latest
            added, said = worker_activity(related), []
            for item in added:
                item["text"] = history_text(item["text"])
        t, r = (before_t, row["id"]) if namespace else (row["id"], before_r)
        candidate = dict(activity=activity + added, messages=messages + said,
                         activityPartial=True, activityCursor=continuation(t, r))
        if history_bytes(candidate) > budget:
            break
        for item in added:
            if not any(a["sequence"] == item["sequence"] for a in activity):
                activity.append(item)
        messages.extend(said)
        before_t, before_r = t, r
        consumed += 1
    partial = consumed < len(pending)
    return dict(activity=sorted(activity, key=lambda a: (a["at"] or 0, a["sequence"])),
                messages=messages, activityPartial=partial,
                activityCursor=continuation(before_t, before_r) if partial else None)


def workers_of(root, project, task_ids, *, worker_id=None, cursor=None):
    db = store_reader(root)
    if db is None:
        return []
    try:
        db.execute("BEGIN")
        rows = _workers_of_db(db, project, task_ids, worker_id=worker_id, cursor=cursor)
        if history_bytes(dict(ok=True, result=rows)) > 786432:
            raise Refused("worker_snapshot_too_large", "Select fewer tasks to read this worker snapshot.")
        return rows
    finally:
        db.close()


def coordinator_of_db(db, project, task_id, cursor):
    from cortex_runtime.workspace_api.history import STEP_PREFIX, activity_item, page_tx
    from cortex_runtime.workspace_api.routes import observed_route
    from cortex_runtime.workspace_api.storage import WorkspaceError
    execution = db.execute("SELECT e.* FROM workspace_executions e JOIN workspace_tasks t USING(task_id) "
                           "WHERE t.project_id=? AND t.task_id=?", (project, task_id)).fetchone()
    if execution is None:
        return None
    runtime = execution["runtime_task_id"]
    row = db.execute("SELECT state FROM runtime_tasks WHERE task_id=?", (runtime,)).fetchone()
    state = row["state"] if row else None
    from cortex_runtime.workspace_api.projections import LIFECYCLE
    status = {"COMPLETED": "completed", "FAILED": "failed", "CANCELLED": "cancelled",
              "WAITING_USER": "waiting_you", "BLOCKED": "pending", "PAUSED": "pending"}.get(state, "working" if state in LIFECYCLE else "unknown")
    try:
        page = page_tx(db, project, task_id, limit=100, cursor=cursor, byte_budget=60000)
    except WorkspaceError as error:
        raise Refused(str(error), "This coordinator history is unavailable. Reopen the task.") from None
    activity = []
    for item in page["items"]:
        if item.get("activity"):
            activity.append(item["activity"])
        elif item.get("text") is not None:
            detail = loads(item["text"].removeprefix(STEP_PREFIX)) if item["text"].startswith(STEP_PREFIX) else None
            activity.append(activity_item(item["kind"], item["text"], int(item["sequence"]),
                                          item["recordedAt"], detail, item.get("toolStatus")))
        else:
            activity.append(dict(sequence=int(item["sequence"]), text="Content unavailable in the retained display history.",
                                 kind="note", at=iso_ms(item["recordedAt"])))
    return dict(id="coordinator:" + task_id, taskId=task_id, executionId=execution["execution_id"],
                role="coordinator", lane="execution", status=status, goal="Coordinator for this task",
                now=None, waitsFor=None, scope=[], route=observed_route(db, runtime),
                attempt=None, elapsedMs=None, runningSince=None, context=None, usage=None,
                result=None, messages=[], activity=activity, activityPartial=page["nextCursor"] is not None,
                activityCursor=page["nextCursor"])


def _workers_of_db(db, project, task_ids, *, worker_id=None, cursor=None):
    if isinstance(worker_id, str) and worker_id.startswith("coordinator:"):
        task_id = worker_id.removeprefix("coordinator:")
        if task_id not in task_ids:
            raise Refused("unknown_task", "This coordinator belongs to another task.")
        item = coordinator_of_db(db, project, task_id, cursor)
        return [item] if item else []
    tables = {row[0] for row in db.execute("select name from sqlite_master where type='table'")}
    if not {"work_assignments", "work_assignment_attempts", "task_events", "workspace_executions"} <= tables:
        return []
    if len(task_ids) > 100:
        raise Refused("worker_snapshot_too_large", "Select at most 100 tasks for a worker snapshot.")
    found = []
    for task_id in task_ids:
        if "workspace_tasks" in tables and not db.execute("SELECT 1 FROM workspace_tasks WHERE task_id=? AND project_id=?", (task_id, project)).fetchone():
            continue
        row = db.execute("select execution_id,runtime_task_id from workspace_executions where task_id=? order by rowid desc limit 1", (task_id,)).fetchone()
        if row is None:
            continue
        runtime, execution = row["runtime_task_id"], row["execution_id"]
        assignments = db.execute("select * from work_assignments where task_id=? and (? IS NULL OR assignment_id=? "
                                 "OR assignment_id IN (SELECT assignment_id FROM work_assignment_attempts WHERE attempt_id=?)) "
                                 "order by created_at limit 101", (runtime, worker_id, worker_id, worker_id)).fetchall()
        if len(assignments) > 100:
            raise Refused("worker_snapshot_too_large", "Select a specific worker to read its history.")
        work = []
        for assignment in assignments:
            attempts = db.execute("SELECT * FROM work_assignment_attempts WHERE assignment_id=? ORDER BY attempt_no",
                                  (assignment["assignment_id"],)).fetchall()
            for attempt in attempts or [None]:
                identity = assignment["assignment_id"] if attempt is None or attempt == attempts[-1] else attempt["attempt_id"]
                if worker_id is None or worker_id == identity:
                    work.append((assignment, attempt, identity, attempts[-1]["attempt_no"] if attempts else 0))
            if len(work) > 100:
                raise Refused("worker_snapshot_too_large", "Select a specific worker attempt to read its history.")
        for assignment, attempt, identity, total in work:
            request = db.execute("SELECT assignment_ref,payload_json FROM task_events WHERE task_id=? "
                "AND event_type='delegation_requested' AND json_valid(payload_json) "
                "AND json_extract(payload_json,'$.work_attempt_ref')=? ORDER BY id DESC LIMIT 1",
                (runtime, attempt["attempt_id"] if attempt else None)).fetchone()
            spec = loads(request["payload_json"]) if request else {}
            provision = db.execute("SELECT p.* FROM delegation_provisioning p JOIN task_events e ON e.task_id=p.task_id AND e.assignment_ref=p.idempotency_key "
                                   "WHERE p.task_id=? AND e.event_type=\'delegation_requested\' AND json_extract(e.payload_json,\'$.work_attempt_ref\')=? "
                                   "ORDER BY p.created_at DESC LIMIT 1", (runtime, attempt["attempt_id"] if attempt else None)).fetchone()
            refs = ([attempt["attempt_id"]] if attempt else [assignment["assignment_id"], assignment["idempotency_key"]])
            refs += ([provision["idempotency_key"], provision["attempt_id"]] if provision else [request["assignment_ref"]] if request else [])
            marks = ",".join("?" for _ in refs)
            binding = hashlib.sha256(json.dumps([project, task_id, assignment["assignment_id"], refs]).encode()).hexdigest()[:24]
            events = db.execute(f"select id,event_type,created_at,payload_json from task_events where task_id=? and assignment_ref in ({marks}) "
                                "and event_type in ('worker_result','assignment_failed','assignment_cancelled') order by id desc limit 1", (runtime, *refs)).fetchall()
            terminal = next((e["event_type"] for e in reversed(events) if e["event_type"] in ("worker_result", "assignment_failed", "assignment_cancelled")), None)
            status = {"worker_result": "completed", "assignment_failed": "failed", "assignment_cancelled": "cancelled"}.get(terminal) or STATUS.get(attempt["status"] if attempt else assignment["status"], "unknown")
            role = spec.get("role") if isinstance(spec.get("role"), str) else assignment["node_key"]
            result = None
            for event in db.execute(f"SELECT payload_json,event_type FROM task_events WHERE task_id=? AND assignment_ref IN ({marks}) "
                                    "AND event_type='worker_result' ORDER BY id DESC LIMIT 1", (runtime, *refs)):
                if event["event_type"] == "worker_result":
                    payload = loads(event["payload_json"])
                    active = payload.get("checks", [])
                    if provision:
                        from cortex_runtime.orchestration.worker_checks import active_checks
                        node = db.execute("SELECT result_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
                                          (runtime, provision["idempotency_key"])).fetchone()
                        if node and node["result_json"]:
                            # Display events omit probe reasons. Revalidate the matching
                            # accepted result using exact provisioning and receipts.
                            stored = loads(node["result_json"]).get("checks", [])
                            check_identity = lambda checks: [(c.get("name"), c.get("outcome"), c.get("tool_call_ref")) for c in checks]
                            if check_identity(stored) == check_identity(active):
                                active = [c for _, c in active_checks(db, runtime, provision["idempotency_key"], stored)]
                    checks = [{"name": c.get("name", ""), "outcome": c.get("outcome") if c.get("outcome") in ("passed", "failed", "not_run", "unverified") else "unverified", "detail": c.get("blocker") or "", "source": "worker"}
                              for c in active if isinstance(c, dict)]
                    result = {"summary": str(payload.get("summary") or payload.get("text") or "")[:8000], "checks": checks, "reportId": payload.get("report_ref")}
                    break
            created = iso_ms(assignment["created_at"])
            from cortex_runtime.workspace_api.routes import worker_route_attempts
            route_attempts = worker_route_attempts(db, runtime, provision["attempt_id"],
                                                  provision["idempotency_key"]) if provision else []
            route = route_attempts[-1]["route"] if route_attempts else None
            found.append({"id": identity, "taskId": task_id, "executionId": execution, "role": role.removeprefix("worker-"), "status": status, "lane": "review" if "review" in role else "execution",
                          "goal": str(spec.get("title") or assignment["node_key"])[:400], "now": None, "waitsFor": None, "scope": [], "route": route, "routeAttempts": route_attempts,
                          "attempt": {"n": attempt["attempt_no"], "of": total} if attempt else None,
                          "elapsedMs": None, "runningSince": created if status == "working" else None, "context": None, "usage": None,
                          "result": result})
            # Bound the complete worker DTO, including duplicated guidance text,
            # and reserve a shared history budget for the enclosing snapshot.
            metadata = history_bytes(found[-1])
            if metadata > 60000:
                raise Refused("worker_content_too_large", "This worker metadata cannot fit a bounded activity page.")
            budget = min(65536 - metadata - 2, max(0, 524288 - history_bytes(found)))
            page = worker_history(db, runtime, refs, provision["channel_id"] if provision else None,
                                  task_id, identity, status, binding, cursor, budget)
            if worker_id is not None and page["activityPartial"] and not page["activity"]:
                raise Refused("worker_content_too_large", "This worker's metadata leaves no room for an activity page.")
            found[-1].update(page)
    return found


# --- Task and sprint decisions --------------------------------------------------

def product_read(root, state, project, *, review=True):
    product = product_of(state, project)
    db = store_reader(root)
    if db is not None:
        try:
            tables = {r[0] for r in db.execute("SELECT name FROM sqlite_master WHERE type='table'")}
            if "workspace_task_options" in tables:
                for row in db.execute("SELECT o.* FROM workspace_task_options o JOIN workspace_tasks t USING(task_id) WHERE t.project_id=?", (project,)):
                    options = loads(row["options_json"])
                    record = task_record(product, row["task_id"])
                    record.update({k: v for k, v in options.items() if k in {"checkout", "settings", "baseRevision"}})
                    checkout = record.get("checkout")
                    if checkout and checkout["kind"] == "main":
                        checkout["branch"] = out(root, "branch", "--show-current").strip() if is_repo(root) else ""
                    elif checkout and "path" not in checkout and is_repo(root):
                        common = out(root, "rev-parse", "--path-format=absolute", "--git-common-dir").strip()
                        group = hashlib.sha256(common.encode()).hexdigest()[:16]
                        from cortex_runtime.workspace_api.task_options import checkout_leaf
                        base = Path(machine_settings(state)["worktreeRoot"]).expanduser() / group
                        leaf = row["task_id"] if (base / row["task_id"]).exists() else checkout_leaf(db, row["task_id"])
                        checkout.update(path=str(base / leaf), branch="cw/" + leaf)
            if review and {"workspace_executions", "runtime_tasks"} <= tables:
                from cortex_runtime.store import Store
                from cortex_runtime.workspace_api.projections import result_actions, result_projection
                from cortex_runtime.workspace_api import WorkspaceError
                from cortex_runtime.contracts import StoreError
                store = None
                for row in db.execute("SELECT t.task_id,e.execution_id,r.state FROM workspace_tasks t "
                                      "JOIN workspace_executions e ON e.execution_id=t.execution_id "
                                      "LEFT JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE t.project_id=?", (project,)):
                    record = task_record(product, row["task_id"])
                    actions = dict(accept=False, requestChanges=False, reason="Wait for Cortex to finish this result.",
                                   unblockAction="wait", acknowledgeOpenChecks=False)
                    if row["state"] == "COMPLETED" and record.get("status") != "done":
                        try:
                            if store is None:
                                store = Store(project_store_directory(root), initialize=False, project_root=root, runtime_access=True)
                            result = result_projection(db, store, project, "wr_" + row["execution_id"])
                            actions = result_actions(result)
                            actions.update(resultId=result["resultId"], version=result["version"],
                                           digest=result.get("observedWorkspaceDigest") or f'result:{result["resultId"]}:{result["version"]}')
                            if record.get("requiresFreshResult") == {"resultId": result["resultId"], "version": str(result["version"])}:
                                actions.update(accept=False, reason="The source changed after this result. Request changes for fresh checks and review.",
                                               unblockAction="request_changes")
                        except (WorkspaceError, StoreError):
                            actions.update(reason="This result binding is unavailable. Refresh or reopen the project before requesting a new result.",
                                           unblockAction="refresh")
                    record["reviewActions"] = actions
        finally:
            db.close()
    # Conflict completion is promoted only by the scheduler after fresh runtime verification.
    import task_git
    for task, record in product["tasks"].items():
        if review and record.get("status") == "ready_to_merge":
            record["integrationFacts"] = task_git.integration_facts(sys.modules[__name__], root, record, task)
    product["hasGit"] = is_repo(root)
    product["ghAvailable"] = bool(shutil.which("gh"))
    return product


def task_record(product, task):
    return product["tasks"].setdefault(task, {})


def removed_checkout(record):
    checkout = record.get("checkout") or {}
    return (record.get("integration") or {}).get("worktreeCleanup") == "removed" and checkout.get("path") and not Path(checkout["path"]).exists()


def unmerged_work(root, record):
    if removed_checkout(record):
        return False
    import task_git
    path = task_git.task_path(sys.modules[__name__], root, record)
    changed_head = is_repo(path) and (record.get("integration") or {}).get("sourceHead") and out(path, "rev-parse", "HEAD").strip() != record["integration"]["sourceHead"]
    return bool(changed_head) or (bool(status_files(path)) if is_repo(path) else False) or record.get("work") not in ("merged", "discard_confirmed")


def task_path_for_discard(root, record):
    import task_git
    return task_git.task_path(sys.modules[__name__], root, record)


def purge_record(root, project, state, product, task, record):
    if not record.get("archive"):
        raise Refused("not_archived", "Only archived tasks can be deleted.")
    from cortex_runtime.delegation.revision import capture_revision
    current_discard = capture_revision(task_path_for_discard(root, record)) if record.get("work") == "discard_confirmed" else None
    discard_valid = current_discard and current_discard["kind"] == "observed" and current_discard["dirty_digest"] == record.get("discardRevision")
    if unmerged_work(root, record) and not discard_valid:
        record.update(archiveQuestion=True, archive=None)
        write_json(product_path(state, project), product)
        raise Refused("unmerged_work", "This task has unmerged work. Decide whether it may be discarded first.")
    from cortex_runtime.workspace_api.purge import purge_task
    from cortex_runtime.workspace_api.storage import WorkspaceError
    from cortex_runtime.store import Store
    import task_git
    checkout = record.get("checkout") or {}
    retained_path = task_git.task_path(sys.modules[__name__], root, record) if checkout.get("kind") == "worktree" and not removed_checkout(record) else None
    result = {"vacuum_status":"not_needed","reclaimed_bytes":0}
    reclaimed = 0
    if (project_store_directory(root) / "cortex.sqlite3").is_file():
        store = Store(project_store_directory(root), project_root=root)
        try:
            result = purge_task(store, project, task)
        except WorkspaceError as error:
            raise Refused(str(error), "This task is still active, unknown, or no longer belongs to this project.") from None
        reclaimed = result.get("reclaimed_bytes", 0)
    retained = None
    if retained_path is not None:
        path = retained_path
        if status_files(path) or git(root, "worktree", "remove", path, check=False).returncode:
            retained = path
    product["tasks"][task] = {"purged": True, "reclamation": {key:value for key,value in result.items() if key in {"vacuum_status","reclaimed_bytes","vacuum_error"}}, **({"retainedWorktree": retained} if retained else {})}
    product["messages"] = [m for m in product["messages"] if m["taskId"] != task]
    product["comments"].pop(task, None)
    for sprint in product["sprints"]:
        sprint["taskIds"] = [t for t in sprint["taskIds"] if t != task]
    return reclaimed


def task_action(root, project, state, params):
    action, task = params.get("action"), params.get("taskId")
    if not isinstance(task, str) or not re.fullmatch(r"[A-Za-z0-9_:.-]{1,128}", task):
        raise Refused("invalid_task", "Unknown task.")
    with Locked(state):
        # Each effect rechecks its own evidence; unrelated result projections do not belong under the writer lock.
        product = product_read(root, state, project, review=False)
        record = task_record(product, task)
        at = now_ms()
        if action == "settings":
            from cortex_runtime.workspace_contract import validate
            from cortex_runtime.workspace_api import task_options
            from cortex_runtime.store import Store
            chosen = params.get("settings")
            validate("CreatePayload", {"description": "settings", "settings": chosen})
            store = Store(project_store_directory(root), project_root=root)
            def save(db):
                row = db.execute("SELECT * FROM workspace_tasks WHERE task_id=? AND project_id=?", (task, project)).fetchone()
                if row is None or row["execution_id"] is not None:
                    raise Refused("busy", "Change a running task through its session, not a draft.")
                options = task_options.read(db, task)
                options["settings"] = chosen
                task_options.save(db, task, options)
            store.immediate_write(save)
        elif action == "rename":
            title = str(params.get("title", "")).strip()
            if not title:
                raise Refused("empty_title", "Write a title first.")
            record.update(title=title[:200], titleState="user")
        elif action in ("resolve_block", "finish_merge", "abort_merge", "resolution_started"):
            if record.get("conflict") == "resolving" and action != "resolution_started":
                raise Refused("busy", "Stop Cortex and wait for the task to stop before resolving manually.")
            import task_git
            task_git.resolve(sys.modules[__name__], root, record, params)
        elif action == "reopen":
            for key in ("status", "acceptance", "automaticReady"):
                record.pop(key, None)
        elif action == "accept":
            import task_git
            from cortex_runtime.delegation.revision import capture_revision
            verified = task_git.runtime_result(sys.modules[__name__], root, project, task, params.get("digest"))
            if verified is not None and record.get("requiresFreshResult") == {"resultId": verified["resultId"], "version": str(verified["version"])}:
                raise Refused("review_required", "The source changed after this result. Run fresh checks and review the new result.")
            if verified is not None:
                if str(params.get("version")) != str(verified["version"]):
                    raise Refused("stale_result", "This result version changed. Refresh and review the current result.")
                open_checks = [c for c in verified["checks"] if c.get("kind") != "manual" and c["outcome"] not in ("passed", "not_applicable")]
                if open_checks and any(not c.get("technicalReason", "").strip() for c in open_checks):
                    raise Refused("checks_required", "Run the unpassed checks first; only a recorded technical blocker permits acceptance.")
                if open_checks and params.get("acknowledgeOpenChecks") is not True:
                    raise Refused("acknowledgement_required", "Explicitly acknowledge the recorded technical blockers before accepting.")
            observed = capture_revision(task_git.task_path(sys.modules[__name__], root, record))
            if observed["kind"] != "observed":
                raise Refused("workspace_unobservable", "The workspace could not be observed; acceptance was not recorded.")
            record["acceptedRevision"] = observed["dirty_digest"]
            record.update(status="ready_to_merge", acceptance={"version": params.get("version"), "digest": params.get("digest"), "at": at,
                          "open": {outcome: sum(c.get("kind") != "manual" and c["outcome"] == outcome for c in verified["checks"]) for outcome in ("failed", "not_run", "unverified")} if verified is not None else params.get("open")})
        elif action == "auto_ready":
            record.update(status="ready_to_merge", automaticReady=True)
        elif action == "target":
            record["targetBranch"] = params.get("branch")
            record["targetInherited"] = False
        elif action == "close":
            choice = params.get("choice")
            if choice not in ("as_is", "cancel", "external", "already_in_target") or params.get("confirmed") is not True:
                raise Refused("confirmation_required", "Choose how to close the task and confirm.")
            if choice in ("external", "already_in_target"):
                if record.get("status") == "done":
                    raise Refused("already_closed", "This task is already closed. Refresh its state.")
                if record.get("conflict", "none") != "none":
                    raise Refused("merge_in_progress", "Abort the active merge before closing the task.")
                import task_git
                task_git.require_settled(sys.modules[__name__], root, project, task)
                if choice == "already_in_target" and not task_git.integration_facts(sys.modules[__name__], root, record, task)["alreadyInTarget"]:
                    raise Refused("containment_changed", "The target changed. Refresh before closing; nothing was merged.")
                record.update(status="done", closedAs="external", doneAt=at, notVerifiedByCortex=True,
                              acceptance=None, integration=None, automaticReady=False, work="unmerged")
                write_json(product_path(state, project), product)
                return {"done": True}
            # A result already in Ready to merge passed review and verification (accepted, or promoted after passed checks);
            # closing it, as a folder without Git must, keeps that fact instead of marking it Not verified.
            checked = record.get("status") == "ready_to_merge" and bool(record.get("acceptance") or record.get("automaticReady"))
            record.update(status="done", closedAs=choice, doneAt=at, notVerifiedByCortex=choice == "as_is" and not checked)
        elif action == "archive":
            if record.get("status") != "done":
                raise Refused("not_done", "Only a finished task can go to the Archive.")
            if unmerged_work(root, record):
                record["archiveQuestion"] = True
            else:
                record.update(archive={"at": at, "previousStatus": "done"}, archiveQuestion=False)
        elif action == "answer_archive":
            if params.get("discard") is True:
                from cortex_runtime.delegation.revision import capture_revision
                observed = capture_revision(task_path_for_discard(root, record))
                if observed["kind"] != "observed":
                    raise Refused("workspace_unobservable", "The work could not be observed; no discard decision was recorded.")
                record["discardRevision"] = observed["dirty_digest"]
                record.update(work="discard_confirmed", archive={"at": at, "previousStatus": "done"}, archiveQuestion=False)
            else:
                record.update(archiveQuestion=False, archiveAskAfter=at + machine_settings(state)["autoArchiveDays"] * DAY_MS)
        elif action == "restore":
            record.update(archive=None, purgeBlocker=None)
        elif action == "clear_old":
            cutoff = params.get("cutoff")
            if (params.get("confirmed") is not True or type(cutoff) not in (int, float)
                    or not 0 <= cutoff <= at or record.get("archive") or record.get("status") != "done"
                    or type(record.get("doneAt")) not in (int, float) or record["doneAt"] > cutoff
                    or unmerged_work(root, record)):
                raise Refused("clear_preview_changed", "The task is no longer eligible. Refresh the preview; no deletion was attempted.")
            record["archive"] = {"at": at, "previousStatus": "done"}
            purge_record(root, project, state, product, task, record)
        elif action == "purge":
            if params.get("confirmed") is not True:
                raise Refused("confirmation_required", "Confirm before deleting.")
            purge_record(root, project, state, product, task, record)
        elif action == "merged":
            record.update(status="done", work="merged", doneAt=at, integration=params.get("integration"))
        elif action == "sprint":
            sprint = next((s for s in product["sprints"] if s["id"] == params.get("sprintId")), None)
            if sprint is None:
                raise Refused("unknown_sprint", "Unknown sprint.")
            record.update(sprintId=sprint["id"], dependencies=params.get("dependencies") or [], planning=bool(params.get("planning")))
            if "targetBranch" not in record or record.get("targetInherited"):
                record.update(targetBranch=sprint.get("targetBranch"), targetInherited=True)
        else:
            raise Refused("unsupported_action", "This machine does not support that yet.")
        write_json(product_path(state, project), product)
    return {"done": True, "reclamation": product["tasks"].get(task, {}).get("reclamation")}


def integrate(root, project, state, params):
    import task_git
    return task_git.integrate(sys.modules[__name__], root, project, state, params)


def sprint_action(project, state, params):
    action = params.get("action")
    with Locked(state):
        product = product_of(state, project)
        if action == "create":
            sprint = {"id": "sp_" + os.urandom(6).hex(), "projectId": project, "description": str(params.get("description", ""))[:32000], "title": None, "runMode": params.get("runMode", "manual"),
                      "targetBranch": params.get("targetBranch"), "settings": params.get("settings"), "planning": "planning", "conversation": [], "plan": None, "history": [], "materializedVersion": None, "taskIds": [], "queueState": "idle", "pauseReason": None}
            if not sprint["description"].strip() or sprint["runMode"] not in ("manual", "automatic"):
                raise Refused("invalid_sprint", "Describe the sprint first.")
            if sprint.get("settings"):
                from cortex_runtime.workspace_contract import validate
                validate("CreatePayload", {"description":"sprint", "settings":sprint["settings"]})
            product["sprints"].append(sprint)
            write_json(product_path(state, project), product)
            return {"sprintId": sprint["id"]}
        sprint = next((s for s in product["sprints"] if s["id"] == params.get("sprintId")), None)
        if sprint is None:
            raise Refused("unknown_sprint", "Unknown sprint.")
        patch = params.get("patch")
        if action == "patch" and isinstance(patch, dict):
            allowed = {"title", "runMode", "targetBranch", "planning", "plan", "history", "materializedVersion", "taskIds", "queueState", "pauseReason", "settings", "planningTaskId"}
            if set(patch) - allowed:
                raise Refused("invalid_sprint", "That sprint change is not supported.")
            if patch.get("settings"):
                from cortex_runtime.workspace_contract import validate
                validate("CreatePayload", {"description":"sprint", "settings":patch["settings"]})
            if "planning" in patch:
                sprint.pop("planningAttempt", None)
                sprint.pop("planningFailure", None)
            sprint.update(patch)
            if "queueState" in patch:
                # Explicit owner queue controls supersede a runtime question pause.
                sprint.pop("pauseKind", None)
                sprint.pop("questionExecutions", None)
            if "targetBranch" in patch:
                for record in product["tasks"].values():
                    if record.get("sprintId") == sprint["id"] and record.get("targetInherited"):
                        record["targetBranch"] = patch["targetBranch"]
        elif action == "note":
            sprint["conversation"].append({"kind": params.get("kind", "note"), "text": str(params.get("text", ""))[:32000], "at": now_ms()})
        else:
            raise Refused("unsupported_action", "This machine does not support that yet.")
        write_json(product_path(state, project), product)
    return {"done": True}


# --- Machine --------------------------------------------------------------------

def tree_mb(path):
    total = 0
    for directory, _, files in os.walk(path):
        for name in files:
            try:
                total += os.lstat(os.path.join(directory, name)).st_size
            except OSError:
                pass
    return round(total / 1_048_576, 1)


def overview(state, projects):
    settings = machine_settings(state)
    stored = [{"label": "Workspace state", "mb": tree_mb(state)}]
    reclamation_failures = 0
    for project in projects:
        reclamation_failures += sum(1 for record in product_of(state, project["id"])["tasks"].values() if record.get("purged") and record.get("reclamation", {}).get("vacuum_status") == "failed")
        directory = project_store_directory(project["root"])
        if directory.is_dir():
            stored.append({"label": project["name"], "mb": tree_mb(directory)})
    identity = Path(__file__).resolve().parents[2] / "package-identity.json"
    version = read_json(identity, {}).get("version") if identity.is_file() else None
    extra = read_json(Path(state) / "machine-runs.json", {})
    return {**settings, "defaultWorktreeRoot": DEFAULT_MACHINE["worktreeRoot"], "reclamationFailures": reclamation_failures, "ghAvailable": bool(shutil.which("gh")), "version": version, "diskMb": round(sum(item["mb"] for item in stored), 1), "storage": stored, "cleanup": extra.get("cleanup"), "diagnostics": extra.get("diagnostics")}


def machine_update(state, params, projects=()):
    patch = params.get("patch")
    if not isinstance(patch, dict) or set(patch) - set(DEFAULT_MACHINE):
        raise Refused("invalid_setting", "That setting is not supported.")
    for key in ("retentionDays", "autoArchiveDays"):
        if key in patch and not (isinstance(patch[key], int) and 1 <= patch[key] <= 3650):
            raise Refused("invalid_setting", "Use a whole number of days from 1.")
    if "name" in patch and not (isinstance(patch["name"], str) and 0 < len(patch["name"].strip()) <= 80):
        raise Refused("invalid_setting", "Use a short name.")
    if "worktreeRoot" in patch and not (isinstance(patch["worktreeRoot"], str) and re.match(r"^(/|~/)", patch["worktreeRoot"]) and "\0" not in patch["worktreeRoot"]):
        raise Refused("invalid_setting", "Use an absolute folder, or one under ~.")
    if "worktreeRoot" in patch:
        candidate = Path(patch["worktreeRoot"]).expanduser().resolve()
        ancestor = candidate
        while not ancestor.exists():
            ancestor = ancestor.parent
        if not ancestor.is_dir():
            raise Refused("unsafe_worktree_root", "A file occupies this location. Choose a folder for new worktrees.")
        if is_repo(ancestor) or any(candidate.is_relative_to(Path(p["root"]).resolve()) for p in projects):
            raise Refused("unsafe_worktree_root", "Choose a folder outside every project checkout, including its private Workspace data.")
        if not os.access(ancestor, os.W_OK | os.X_OK):
            raise Refused("unsafe_worktree_root", "This location is not writable. Choose a writable folder or change its permissions, then save again.")
        patch["worktreeRoot"] = str(candidate)
    with Locked(state):
        write_json(Path(state) / "machine.json", {**read_json(Path(state) / "machine.json", {}), **patch})
    return {"done": True}


def record_run(state, key, value):
    with Locked(state):
        path = Path(state) / "machine-runs.json"
        write_json(path, {**read_json(path, {}), key: value})


def diagnostics(state, projects):
    items = []

    def check(name, ok, detail, action):
        items.append({"name": name, "ok": bool(ok), "detail": detail, "action": "" if ok else action})

    check("Python", sys.version_info >= (3, 11), sys.version.split()[0], "Use Python 3.11 or newer, then run diagnostics again.")
    for tool in ("git", "node", "npm", "bwrap"):
        available = bool(shutil.which(tool))
        check(tool, available, "Available" if available else "Not found by the desktop service",
              "Make " + tool + " available to the desktop service (not just your terminal), restart the service, then run diagnostics again.")
    writable = os.access(state, os.W_OK)
    check("Private state", writable, "Writable" if writable else "The app cannot save its private state",
          "Restore write permission for this app’s private state folder, then run diagnostics again.")
    for project in projects:
        exists = Path(project["root"]).is_dir()
        check("Project " + project["name"], exists, "Folder available" if exists else "Project folder is missing",
              "Reconnect the drive or register the project at its current location, then run diagnostics again.")
    failed = [item for item in items if not item["ok"]]
    run = {"at": now_ms(), "status": "ok" if not failed else "degraded", "checks": len(items), "items": items}
    record_run(state, "diagnostics", run)
    return run


def cleanup(state, projects, *, clock=now_ms):
    """Done tasks move to the Archive and old archived ones go, on the machine's own day counts.

    Tests may inject a callable; no wire, environment or packaged-launch option selects it.
    """
    settings, at = machine_settings(state), clock()
    removed = asked = freed = skipped = reclamation_failed = 0
    with Locked(state):
        for project in projects:
            product = product_read(project["root"], state, project["id"])
            for task, record in list(product["tasks"].items()):
                archive = record.get("archive")
                if archive and at - archive["at"] >= settings["retentionDays"] * DAY_MS and not record.get("purgeBlocker"):
                    try:
                        freed += purge_record(project["root"], project["id"], state, product, task, record)
                        removed += 1
                        reclamation_failed += int(product["tasks"][task].get("reclamation", {}).get("vacuum_status") not in {"completed", "not_needed"})
                    except Refused as error:
                        record["purgeBlocker"] = error.message
                        skipped += 1
                        asked += int(error.code == "unmerged_work")
                elif record.get("status") == "done" and not archive and record.get("doneAt") and at - record["doneAt"] >= settings["autoArchiveDays"] * DAY_MS:
                    if not unmerged_work(project["root"], record):
                        record["archive"] = {"at": at, "previousStatus": "done"}
                    elif (record.get("archiveAskAfter") or 0) <= at:
                        record["archiveQuestion"] = True
                        asked += 1
            write_json(product_path(state, project["id"]), product)
    run = {"at": at, "removed": removed, "skipped": skipped, "asked": asked, "freedMb": freed / (1024 * 1024), "reclamationFailed": reclamation_failed}
    record_run(state, "cleanup", run)
    return run


# --- Cortex configuration -------------------------------------------------------

def revision_number(token):
    match = re.fullmatch(r"sha256:([0-9a-f]{12})[0-9a-f]*", token or "")
    return int(match[1], 16) if match else 0


def routing_view(rules, effective_source, builtin):
    rules = rules or {}
    models, quota, enabled = rules.get("models") or {}, rules.get("quota_windows") or {}, rules.get("enabled") or {}
    engines = {}
    for engine in ENGINES:
        config = models.get(engine) or {}
        policy = {}
        for model, values in (config.get("policy") or {}).items():
            policy[model] = {"effortFloor": values.get("effort_floor"), "maxEffort": values.get("max_effort"), "effortLadder": values.get("effort_ladder"), "alsoStandardFor": values.get("also_standard_for"),
                             "preferredFor": values.get("preferred_for"), "preferredWhen": values.get("preferred_when"), "requestNames": values.get("request_names")}
        tiers = {model: value if isinstance(value, list) else [value] for model, value in (config.get("tiers") or {}).items()}
        engines[engine] = {"enabled": enabled.get(engine) or [], "auto": config.get("auto") or [], "tiers": tiers, "policy": policy, "quota": quota.get(engine) or []}
    pins = {}
    for role, value in (rules.get("pin") or {}).items():
        route, _, effort = value.partition(":")
        engine, _, model = route.partition("/")
        pins[role] = {"engine": engine, "model": model, "effort": effort or None}
    return {"version": rules.get("model_policy_version") or 2, "deny": rules.get("deny") or [], "providers": rules.get("providers") or list(ENGINES), "engines": engines, "pins": pins,
            "affinity": rules.get("engine_affinity") or {}, "autoAllow": rules.get("auto_allow") or [], "legacyTiers": sorted(rules.get("tiers") or [])}


def routing_document(view, previous):
    document = {key: value for key, value in (previous or {}).items() if key in ("tiers",)}
    document["model_policy_version"] = view.get("version", 2)
    for key, value in (("deny", view.get("deny")), ("auto_allow", view.get("autoAllow"))):
        if value:
            document[key] = value
    if view.get("providers") and set(view["providers"]) != set(ENGINES):
        document["providers"] = view["providers"]
    pins = {role: pin["engine"] + "/" + pin["model"] + (":" + pin["effort"] if pin.get("effort") else "") for role, pin in (view.get("pins") or {}).items()}
    if pins:
        document["pin"] = pins
    if view.get("affinity"):
        document["engine_affinity"] = view["affinity"]
    enabled, models, quota = {}, {}, {}
    for engine, config in (view.get("engines") or {}).items():
        enabled[engine] = config.get("enabled") or []
        entry = {}
        if config.get("auto"):
            entry["auto"] = config["auto"]
        if config.get("tiers"):
            entry["tiers"] = {model: tiers[0] if len(tiers) == 1 else tiers for model, tiers in config["tiers"].items() if tiers}
        policy = {}
        for model, values in (config.get("policy") or {}).items():
            written = {"effort_floor": values.get("effortFloor"), "max_effort": values.get("maxEffort"), "effort_ladder": values.get("effortLadder"), "also_standard_for": values.get("alsoStandardFor"),
                       "preferred_for": values.get("preferredFor"), "preferred_when": values.get("preferredWhen"), "request_names": values.get("requestNames")}
            written = {key: value for key, value in written.items() if value is not None}
            if written:
                policy[model] = written
        if policy:
            entry["policy"] = policy
        if entry:
            models[engine] = entry
        if config.get("quota"):
            quota[engine] = config["quota"]
    document["enabled"] = enabled
    if models:
        document["models"] = models
    if quota:
        document["quota_windows"] = quota
    return document


def ceiling_view(raw):
    raw = raw or {}
    fallback, limits = raw.get("fallback_policy") or {}, raw.get("limits") or {}
    return {"allowedRoutes": raw.get("allowed_routes"), "allowedConnections": raw.get("allowed_connections"), "allowedRecipients": raw.get("allowed_data_recipients"), "attempts": limits.get("attempts"),
            "failureClasses": fallback.get("allowed_failure_classes"), "targetRoutes": fallback.get("allowed_target_routes"), "budget": fallback.get("budget"), "consent": bool(fallback.get("data_recipient_consent")),
            "stored": {"permissionLevel": raw.get("permission_level"), "calls": limits.get("calls"), "tokens": limits.get("tokens"), "concurrency": limits.get("concurrency"), "spendLimit": raw.get("spend_limit"), "spendEnforceable": bool(raw.get("spend_limit_enforceable"))}}


def ceiling_document(view):
    stored = view.get("stored") or {}
    limits = {key: value for key, value in (("attempts", view.get("attempts")), ("calls", stored.get("calls")), ("tokens", stored.get("tokens")), ("concurrency", stored.get("concurrency"))) if value is not None}
    return {"allowed_routes": view.get("allowedRoutes"), "allowed_connections": view.get("allowedConnections"), "allowed_data_recipients": view.get("allowedRecipients"), "permission_level": stored.get("permissionLevel"),
            "limits": limits, "spend_limit": stored.get("spendLimit"), "spend_limit_enforceable": bool(stored.get("spendEnforceable")),
            "fallback_policy": {"allowed_target_routes": view.get("targetRoutes"), "allowed_failure_classes": view.get("failureClasses"), "budget": view.get("budget"), "data_recipient_consent": bool(view.get("consent"))}}


def defaults_view(view, workspace):
    raw = view.get("defaults") or {}
    coordinator = raw.get("coordinator")
    choice = None
    if isinstance(coordinator, dict):
        parameters = coordinator.get("parameters") or {}
        choice = {"engine": coordinator.get("engine"), "model": coordinator.get("model"), "effort": coordinator.get("effort") or parameters.get("thought_level") or "", "parameters": parameters}
    route = None
    if raw.get("route_id"):
        route = {"routeId": raw["route_id"], "connectionId": raw.get("route_connection_id"), "model": raw.get("route_model"), "parameters": raw.get("parameters") or {}}
    return {"coordinator": choice, "routeDefault": route, "language": raw.get("language") or "auto", "mode": workspace.get("mode", "guarded"), "checkout": workspace.get("checkout", "main"),
            "ceiling": ceiling_view(view.get("ceiling")), "preserved": view.get("preserved_keys") or []}


def mcp_view(view):
    servers = []
    for entry in view.get("servers", []):
        config = entry.get("config") or {}
        servers.append({"name": entry.get("name"), "command": config.get("command") or "", "args": config.get("args") or [], "enabled": entry.get("enabled") is True,
                        "env": [{"name": item.get("name"), "stored": bool(item.get("value_set")), "action": "keep", "value": ""} for item in config.get("env") or []],
                        "roles": config.get("roles"), "engines": config.get("engines"), "allowedTools": config.get("allowed_tools") or [], "readOnlyTools": config.get("read_only_tools") or [],
                        "coordinatorTools": config.get("coordinator_tools"), "alwaysLoadTools": config.get("always_load_tools"), "readPaths": config.get("read_paths") or [], "writePaths": config.get("write_paths") or [],
                        "network": bool(config.get("network")), "runtimeDirEnv": config.get("runtime_dir_env") or [], "handTools": [], "unsupportedKeys": entry.get("unsupported_keys") or [],
                        "problem": (entry.get("problem") or {}).get("message")})
    return servers


def mcp_operations(previous, servers):
    known = {entry.get("name") for entry in previous.get("servers", [])}
    wanted = {server["name"] for server in servers}
    operations = [{"op": "remove", "name": name} for name in sorted(known - wanted)]
    for server in servers:
        entry = {"command": server.get("command"), "args": server.get("args") or [], "enabled": bool(server.get("enabled")), "allowed_tools": server.get("allowedTools") or [], "read_only_tools": server.get("readOnlyTools") or [],
                 "read_paths": server.get("readPaths") or [], "write_paths": server.get("writePaths") or [], "network": bool(server.get("network")), "runtime_dir_env": server.get("runtimeDirEnv") or []}
        for key, value in (("coordinator_tools", server.get("coordinatorTools")), ("always_load_tools", server.get("alwaysLoadTools")), ("roles", server.get("roles")), ("engines", server.get("engines"))):
            if value is not None:
                entry[key] = value
        env = []
        for item in server.get("env") or []:
            if item.get("action") == "remove":
                continue
            env.append({"name": item["name"], "keep": True} if item.get("action") == "keep" and item.get("stored") else {"name": item["name"], "value": item.get("value", "")})
        if env:
            entry["env"] = env
        operations.append({"op": "upsert", "name": server["name"], "server": entry})
    return operations


def profile_sections(settings, workspace):
    """Map saved, non-secret profile policy through the same views as live settings."""
    result = {}
    if "routing" in settings:
        result["routing"] = routing_view(settings["routing"], "user", False)
    if "user_defaults" in settings:
        result["defaults"] = defaults_view(settings["user_defaults"], workspace)
    if "mcp_servers" in settings:
        servers = mcp_view({"servers": [{"name": name, "enabled": spec.get("enabled", True), "config": spec}
                                      for name, spec in sorted(settings["mcp_servers"].items())]})
        result["mcp"] = [{k: v for k, v in server.items() if k not in
                          {"command", "args", "env", "handTools", "unsupportedKeys", "problem"}} for server in servers]
    if "connections" in settings:
        result["sessions"] = {key: spec["max_concurrent_sessions"] for key, spec in settings["connections"].items()}
    return result


def connection_view(record, sessions_default=1):
    engine = {"claude-agent-acp": "claude", "codex-acp": "codex"}.get(record.get("engine"), "opencode")
    ready = record.get("state") in ("configured", "ready", "validated", "connected")
    return {"id": record.get("id"), "engine": engine, "label": record.get("name"), "note": record.get("auth_mode") or "", "sessions": record.get("max_concurrent_sessions") or sessions_default,
            "state": "connected" if ready else "unavailable" if record.get("state") in ("unavailable", "invalid") else "needs_login", "checkedAt": None,
            "detail": {"connectionId": record.get("id"), "provider": record.get("provider") or "", "authMode": record.get("auth_mode") or "", "keyKind": record.get("key_kind"), "endpoint": record.get("endpoint"),
                       "launchCommand": " ".join(record.get("launch_command") or []), "credential": (record.get("credential") or {}).get("kind", ""), "qualified": bool(record.get("qualified")),
                       "createdAt": record.get("created_at") or "", "updatedAt": record.get("updated_at") or ""}}


def config_call(request):
    from cortex_runtime.configuration import management
    reply = management.execute(request)
    if not reply.get("ok"):
        error = reply.get("error") or {}
        raise Refused(error.get("code") or "config_refused", error.get("message") or error.get("remediation") or "Cortex refused that setting.")
    return reply["result"]


def plan_usage():
    """What each engine itself reported about its plan. Nothing is estimated, and OpenCode reports none."""
    from cortex_runtime.connections import quota, registry
    seen = {engine: [] for engine in ENGINES}
    usage = {}
    records = sorted(registry.inspect_registry(None).connections.values(), key=lambda record: record.id)
    for record in records:
        name = {"claude-agent-acp": "claude", "codex-acp": "codex"}.get(record.engine)
        if name is None:
            continue
        usage.setdefault(name, {"engine": name, "connected": True, "usedPercent": None, "resetAt": None, "observedAt": None, "window": "weekly"})
        try:
            windows = quota.visible_windows(record)
        except (OSError, ValueError, TypeError):
            continue
        for window in windows:
            label = window["name"].casefold()
            # Codex slots are not durations: primary can be the weekly window.
            # Never label a short or unknown-duration slot as weekly.
            if record.engine == "codex-acp":
                if window.get("windowMinutes") != 7 * 24 * 60:
                    continue
            elif not any(marker in label for marker in ("week", "seven")):
                continue
            seen[record.engine].append({"window": window["name"], "percent": window["usedPercent"]})
            used = window["usedPercent"]
            if usage[name]["usedPercent"] is None or used > usage[name]["usedPercent"]:
                reset = window.get("resetsAt")
                usage[name].update(usedPercent=used, observedAt=None,
                                   resetAt=time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(reset)) if reset else None)
    return list(usage.values()), seen


def tell_apart(models, owners):
    """Two connections can offer one model. The connection's name tells them apart, and a route id never does."""
    same = {}
    for model in models:
        same.setdefault(model["label"], []).append(model)
    for label, group in same.items():
        if len(group) > 1:
            for number, model in enumerate(group, 1):
                model["label"] = f"{label} · {owners.get(model['routeId']) or f'Connection {number}'}"


def config_read(state, project_root=None):
    from cortex_runtime.configuration import management
    surfaces = management.read_all(project_root)

    def part(name):
        value = surfaces.get(name) or {}
        return value.get("result") if value.get("ok") else None

    defaults, routing, profiles, mcp, connections = (part(name) for name in ("user_defaults", "routing", "routing_profiles", "mcp_servers", "connections"))
    unavailable = next((value["error"].get("message") for value in surfaces.values() if not value.get("ok")), None)
    workspace = read_json(Path(state) / "defaults.json", {})
    catalogue = {engine: {"observedAt": None, "failed": False, "models": []} for engine in ENGINES}
    try:
        routes = config_call({"action": "read", "surface": "route_catalog"})
        from cortex_runtime.acp import config_options
        from cortex_runtime.connections import registry
        registered = registry.read_registry().connections
        entries = config_options.selectable_entries_from_snapshots(registered)
        owners = {entry.route.id: getattr(registered.get(entry.connection_id), "name", None) for entry in entries}
        from cortex_runtime.connections import context_window
        contexts = {entry.route.id: context_window.observed(entry.connection_id, entry.route.model) for entry in entries}
        observed_efforts = {entry.route.id: [value for name, value in entry.route.supported_parameters if name == "thought_level" and value != "engine_default"] for entry in entries}
        for route in routes.get("routes", []):
            engine = route.get("engine")
            if route.get("selectable", route.get("in_selector")) and engine in catalogue and not any(m["routeId"] == route["route_id"] for m in catalogue[engine]["models"]):
                efforts = observed_efforts.get(route["route_id"], [])
                catalogue[engine]["models"].append({"id": route["model"], "routeId": route["route_id"], "label": route.get("model_name") or route["model"], "efforts": efforts, "observed": bool(route.get("observed")), **({"contextWindow": contexts[route["route_id"]]} if contexts.get(route["route_id"]) else {})})
        for engine in catalogue.values():
            tell_apart(engine["models"], owners)
        for engine in routes.get("engines", []):
            times = [c.get("catalog_observed_at") for c in engine.get("connections", []) if c.get("catalog_observed_at")]
            if engine.get("engine") in catalogue and times:
                catalogue[engine["engine"]]["observedAt"] = iso_ms(max(times))
    except Refused:
        pass
    setup = {engine: {"status": "not_downloaded", "automatic": False, "reason": None, "autoConnectDisabled": False} for engine in ENGINES}
    try:
        found = config_call({"action": "discover_connections", "surface": "connections"})
        for item in found.get("engines", []):
            if item.get("engine") in setup:
                setup[item["engine"]] = {"status": item.get("status", "not_downloaded"), "automatic": bool(item.get("automatic_setup")), "reason": item.get("automatic_setup_reason"), "autoConnectDisabled": bool(item.get("auto_connect_disabled"))}
    except Refused:
        pass
    routing_rules = (routing or {}).get("effective_rules") or {}
    profile_list = []
    for entry in (profiles or {}).get("profiles", []):
        profile_list.append({"id": entry["name"], "name": entry["name"], "description": entry.get("description") or "", "updatedAt": iso_ms(entry.get("updated_at")), "sections": profile_sections(entry.get("settings") or {}, workspace)})
    sources = {view.get("source"): view for view in (routing or {}).get("sources", [])}
    user_route = sources.get("user") or {}
    tokens = {"routing": user_route.get("revision") or "absent", "user_defaults": (defaults or {}).get("revision") or "absent", "mcp_servers": (mcp or {}).get("revision") or "absent",
              "routing_profiles": (profiles or {}).get("revision") or "absent", "connections": (connections or {}).get("revision") or "absent",
              "profile_sections": (profiles or {}).get("current_revisions") or {}}
    usage, quota_seen = plan_usage()
    return {
        "config": {
            "revisions": {"routing": revision_number(tokens["routing"]), "user_defaults": revision_number(tokens["user_defaults"]), "project_ceiling": 0, "mcp_servers": revision_number(tokens["mcp_servers"])},
            "surfaces": {"routing": routing_view(routing_rules, (routing or {}).get("effective_source"), (routing or {}).get("builtin")), "user_defaults": defaults_view(defaults or {}, workspace), "mcp_servers": mcp_view(mcp or {})},
            "projects": {}, "active": (profiles or {}).get("active"), "interrupted": bool((profiles or {}).get("pending")), "unavailable": unavailable, "profiles": profile_list,
            "catalogue": catalogue, "quotaObserved": quota_seen, "mcpObserved": {}, "setup": setup,
            "files": {"envRouting": None, "invalidRouting": bool(routing) and not routing.get("effective_valid", False) and routing.get("effective_source") != "missing", "migration": False, "missing": [name for name, value in (("routing.json", user_route.get("exists")), ("defaults.json", (defaults or {}).get("exists")), ("mcp-servers.json", (mcp or {}).get("exists"))) if not value],
                      "backups": 0, "preservedRouting": [], "preservedDefaults": (defaults or {}).get("preserved_keys") or [], "askpass": False, "env": {"engineUpdate": None, "engineFollow": None, "opencodeBinary": None}},
        },
        "connections": [connection_view(record) for record in (connections or {}).get("connections", [])],
        "tokens": tokens,
        "usage": usage,
    }


def project_config(root, params=None):
    routing = config_call({"action": "read", "surface": "routing", "project": root})
    ceiling = config_call({"action": "read", "surface": "project_ceiling", "project": root})
    project = next((s for s in routing.get("sources", []) if s.get("source") == "project"), {})
    if params is None:
        value = {}
        if project.get("exists"):
            value["routing"] = routing_view(project.get("rules") or {}, "project", routing.get("builtin"))
        if ceiling.get("ceiling") is not None:
            value["ceiling"] = ceiling_view(ceiling["ceiling"])
        tokens = {"routing": project.get("revision") or "absent", "project_ceiling": ceiling["revision"]}
        value["revisions"] = {key:revision_number(token) for key,token in tokens.items()}
        return {"value": value, "tokens": tokens}
    surface, value, tokens = params.get("surface"), params.get("value"), params.get("tokens") or {}
    if surface == "routing":
        return config_call({"action": "update", "surface": "routing", "project": root, "target": "project",
                            "expected_revision": tokens.get("routing") or "absent", "rules": routing_document(value, project.get("rules")) if value is not None else None})
    if surface == "project_ceiling":
        return config_call({"action": "update", "surface": "project_ceiling", "project": root,
                            "expected_revision": tokens.get("project_ceiling") or "absent", "ceiling": ceiling_document(value) if value is not None else None})
    raise Refused("unsupported_action", "Only routing and ceilings can be overridden by a project.")


def config_save(state, params):
    surface, value, tokens = params.get("surface"), params.get("value"), params.get("tokens") or {}
    if surface == "routing":
        current = config_call({"action": "read", "surface": "routing"})
        user = next((view for view in current.get("sources", []) if view.get("source") == "user"), {})
        return config_call({"action": "update", "surface": "routing", "target": "user", "expected_revision": tokens.get("routing") or user.get("revision") or "absent", "rules": routing_document(value, user.get("rules"))})
    if surface == "user_defaults":
        choice, route = value.get("coordinator"), value.get("routeDefault")
        patch = {"coordinator": None, "route_id": None, "route_connection_id": None, "route_model": None, "language": None if value.get("language") in (None, "auto") else value["language"]}
        if choice:
            parameters = dict(choice.get("parameters") or {})
            if choice.get("effort"):
                parameters["thought_level"] = choice["effort"]
            patch["coordinator"] = {"engine": choice["engine"], "model": choice["model"], "parameters": parameters}
        elif route:
            patch.update(route_id=route["routeId"], route_connection_id=route["connectionId"], route_model=route["model"], parameters=route.get("parameters") or None)
        result = config_call({"action": "update", "surface": "user_defaults", "expected_revision": tokens.get("user_defaults") or "absent", "defaults": patch, "ceiling": ceiling_document(value.get("ceiling") or {})})
        # A revision refusal must not commit the Workspace-only half of this save.
        with Locked(state):
            path = Path(state) / "defaults.json"
            write_json(path, {**read_json(path, {}), "mode": value.get("mode", "guarded"), "checkout": value.get("checkout", "main")})
        return result
    if surface == "mcp_servers":
        current = config_call({"action": "read", "surface": "mcp_servers"})
        operations = mcp_operations(current, value or [])
        if not operations:
            return {"changed": False}
        return config_call({"action": "update", "surface": "mcp_servers", "expected_revision": tokens.get("mcp_servers") or current.get("revision"), "operations": operations})
    raise Refused("unsupported_action", "This machine does not support that yet.")


CONFIG_ACTIONS = {"apply_default_preset": ("default_preset", ()), "reset_default_preset": ("default_preset", ("expected_revision", "confirm")), "refresh": ("route_catalog", ()),
                  "connection_update": ("connections", ("expected_revision", "connection_id", "operation", "name", "max_concurrent_sessions")),
                  "install_engine": ("connections", ("engine",)), "onboard_opencode": ("connections", ("expected_revision", "explicit_opt_in", "name")),
                  "profile": ("routing_profiles", ("verb", "expected_revision", "expected_revisions", "name", "rename_to", "description", "discard_modifications"))}


def config_action(params):
    action = params.get("action")
    if action not in CONFIG_ACTIONS:
        raise Refused("unsupported_action", "This machine does not support that yet.")
    surface, fields = CONFIG_ACTIONS[action]
    request = {key: params[key] for key in fields if key in params}
    if action == "connection_update":
        return config_call({**request, "action": "update", "surface": surface})
    if action == "profile":
        return config_call({**{k: v for k, v in request.items() if k != "verb"}, "action": request.get("verb"), "surface": surface})
    if action == "reset_default_preset":
        request["expected_revision"] = config_call({"action": "read", "surface": "default_preset"}).get("revision")
    return config_call({**request, "action": action, "surface": surface})


# Only fixed event categories are retained, never request/response data or stderr.
def log_folder(state):
    folder = Path(state) / "logs"
    folder.mkdir(parents=True, exist_ok=True, mode=0o700)
    info = folder.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise OSError("private_logs_required")
    return folder


def service_log(state, event):
    if event not in {"helper_ok", "helper_refused", "helper_failed", "agent_started", "agent_stopped", "scheduler_failed"}:
        raise ValueError("unknown_log_event")
    # Reply diagnostics must not queue behind product writes or slow Git operations.
    with Locked(state, "service-log.lock"):
        folder = log_folder(state)
        current, previous = folder / "service.log", folder / "service.log.1"
        for path in (current, previous):
            if path.is_symlink():
                raise OSError("unsafe_log_file")
        if current.exists() and current.stat().st_size >= 65_536:
            os.replace(current, previous)
        fd = os.open(current, os.O_WRONLY | os.O_CREAT | os.O_APPEND | os.O_NOFOLLOW, 0o600)
        with os.fdopen(fd, "w") as handle:
            os.fchmod(handle.fileno(), 0o600)
            handle.write(json.dumps({"at": now_ms(), "event": event}) + "\n")


# --- Dispatch -------------------------------------------------------------------

def run(request):
    op, root, project, state, params = (request.get(key) for key in ("op", "root", "projectId", "stateDir", "params"))
    params = params or {}
    projects = request.get("projects") or []
    if op == "workflow.describe":
        import owner_workflows
        return owner_workflows.describe(params)
    if op == "maintenance":
        import maintenance
        return maintenance.action(state, params, projects=projects)
    if op == "engine.connection":
        import engine_connections
        return engine_connections.action(params)
    if op == "logs.folder":
        return {"folder": str(log_folder(state))}
    if op == "overview":
        return overview(state, projects)
    if op == "machine.update":
        return machine_update(state, params, projects)
    if op == "diagnostics.run":
        return diagnostics(state, projects)
    if op == "cleanup.run":
        return cleanup(state, projects)
    if op == "config.mcp.discover":
        import mcp_discovery
        return mcp_discovery.discover(params.get("server"))
    if op == "config.read":
        return config_read(state)
    if op == "config.save":
        return config_save(state, params)
    if op == "config.action":
        return config_action(params)
    if not root:
        raise Refused("project_required", "Unknown project.")
    if op == "project.config.read":
        return project_config(root)
    if op == "project.config.save":
        return project_config(root, params)
    if op == "repo.snapshot":
        return repo_snapshot(root, project, state)
    if op == "repo.file":
        return repo_file(root, project, state, params)
    if op == "repo.file.save":
        return repo_file_save(root, project, state, params)
    if op == "repo.locate":
        return repo_locate(root, project, params)
    if op == "git_init":
        return git_action(root, project, state, {"action": "git_init"})
    if op == "git":
        return git_action(root, project, state, params)
    if op == "workers":
        return workers_of(root, project, params.get("taskIds") or [], worker_id=params.get("workerId"), cursor=params.get("cursor"))
    if op == "product.read":
        return product_read(root, state, project)
    if op in ("product.message", "product.comment"):
        return conversation_action(project, state, params, comments=op == "product.comment")
    if op == "product.task":
        return task_action(root, project, state, params)
    if op == "product.integrate":
        return integrate(root, project, state, params)
    if op == "product.sprint":
        return sprint_action(project, state, params)
    raise Refused("unsupported_action", "This machine does not support that yet.")


def main():
    request = {}
    try:
        request = json.loads(sys.stdin.read())
        reply = {"ok": True, "result": run(request)}
    except Refused as error:
        reply = {"ok": False, "code": error.code, "message": error.message}
    except (ValueError, KeyError, TypeError, OSError, sqlite3.Error) as error:
        reply = {"ok": False, "code": "machine_tool_failed", "message": type(error).__name__}
    try:
        service_log(request["stateDir"], "helper_ok" if reply["ok"] else "helper_refused")
    except (KeyError, TypeError, OSError):
        pass  # Logging failure never changes an already committed action.
    sys.stdout.write(json.dumps(reply, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
