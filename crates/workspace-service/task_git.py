"""Task-owned Git effects. No reset, stash, force removal or implicit remote push."""
from pathlib import Path
from cortex_runtime.project_storage import project_store_directory
import json
import os
from contextlib import contextmanager
import shutil
import subprocess
import time
import tempfile
import stat


def helper_path(name):
    return name.split("/", 1)[0] in {".cortex", ".cortex-dev", ".cortex-dev-owner.json", ".cortex-workspace-dev.json", ".cortex-workspace.json"}


def stage_work(h, path):
    # Exclusions also cover a helper accidentally tracked by an older app.
    if any(helper_path(p) for p in h.out(path, "diff", "--cached", "--name-only").splitlines()):
        raise h.Refused("private_index", "Unstage Workspace helper data before committing.")
    # Enumerate real user paths. Even an exclusion pathspec can make Git complain
    # about an ignored helper directory. Literal paths also handle pathspec metacharacters.
    names = set()
    for args in (("diff", "--name-only", "-z"), ("ls-files", "--others", "--exclude-standard", "-z")):
        names.update(p for p in h.out(path, *args).split("\0") if p and not helper_path(p))
    if names:
        h.git(path, "add", "-A", "--", *[":(literal)" + p for p in sorted(names)])


@contextmanager
def index_transaction(h, path):
    """Restore the exact prior staging selection on failure, never user file contents."""
    index = Path(h.out(path, "rev-parse", "--path-format=absolute", "--git-path", "index").strip())
    before = index.read_bytes() if index.exists() else None
    head = h.out(path, "rev-parse", "HEAD").strip()
    try:
        yield
    except Exception:
        if h.out(path, "rev-parse", "HEAD").strip() != head:
            raise h.Refused("integration_unknown", "A commit was recorded before integration stopped. Inspect Git history and refresh the task before continuing; do not repeat the merge blindly.") from None
        lock = index.with_name(index.name + ".lock")
        try:
            with lock.open("xb") as stream:
                if before is not None:
                    stream.write(before)
            if before is None:
                index.unlink(missing_ok=True)
                lock.unlink()
            else:
                os.replace(lock, index)
        except OSError:
            raise h.Refused("index_restore_failed", "Integration stopped and the previous staging selection could not be restored. Close other Git operations, inspect staged files in Changes, then retry. Your file contents were not discarded.") from None
        raise


def valid_branch(h, root, name):
    if not isinstance(name, str) or name.startswith("-") or h.git(root, "check-ref-format", "--branch", name, check=False).returncode:
        raise h.Refused("invalid_branch", "Choose a valid target branch.")
    if h.git(root, "show-ref", "--verify", "--quiet", "refs/heads/" + name, check=False).returncode:
        raise h.Refused("unknown_branch", "The target branch must exist locally.")
    return name


def task_path(h, root, record):
    checkout = record.get("checkout") or {"kind": "main"}
    if checkout["kind"] == "main":
        return str(root)
    path = checkout.get("path")
    if not any(str(Path(t["path"]).resolve()) == path and t.get("branch") == checkout.get("branch") for t in h.worktrees(root)):
        raise h.Refused("unknown_checkout", "The task worktree is missing or its branch changed.")
    return path


def conflicts(h, path, source, target, record):
    files = []
    remaining = 30_000
    failure = None
    for name in h.out(path, "diff", "--name-only", "--diff-filter=U", "-z").split("\0"):
        if not name:
            continue
        sides = {}
        note = ""
        stages = [line.split("\t", 1)[0].split() for line in h.out(path, "ls-files", "-u", "--", name).splitlines()]
        if {s[2] for s in stages} != {"1", "2", "3"} or len({s[0] for s in stages}) != 1 or any(s[0] not in {"100644", "100755"} for s in stages):
            note = "Add/delete, symlink or mode conflict. Resolve the intended file type, contents and executable bit in Git/editor, then git add (or git rm for deletion) and Finish merge."
        for key, stage in (("base", 1), ("ours", 2), ("theirs", 3)):
            size = h.out(path, "cat-file", "-s", f":{stage}:{name}", check=False).strip()
            text = ""
            if size.isdigit() and int(size) <= min(30_000, remaining) and not note:
                raw = h.git(path, "show", f":{stage}:{name}").stdout
                try:
                    text = raw.decode("utf-8")
                    if "\0" in text:
                        raise ValueError()
                    remaining -= len(raw)
                except (UnicodeError, ValueError):
                    note = "Binary conflict. Resolve the intended bytes with Git/editor, git add the file, then Finish merge."
            elif not note:
                note = "Preview limit reached. Resolve this file with Git/editor, git add the file, then Finish merge."
            sides[key] = {"ref": key, "start": 1, "lines": text.splitlines(), "hit": []}
        if note:
            for side in sides.values():
                side["lines"] = [note]
        results = {key: {"start": 1, "lines": sides[key]["lines"], "hit": []} for key in ("ours", "theirs")}
        results["both"] = {"start": 1, "lines": sides["ours"]["lines"] + sides["theirs"]["lines"], "hit": []}
        results["cortex"] = {"start": 1, "lines": [], "hit": []}
        files.append({"path": name, "blocks": [{"id": name, "title": name, **sides, "results": results, "choice": None, "cortexNote": note}]})
        if len(json.dumps(files).encode()) > 350_000 or len(files) >= 100:
            files.pop()
            failure = "Conflict display budget reached. Additional paths remain unresolved. Use Git status in the task checkout, resolve and stage every file, then Finish merge."
            break
    return {"source": target, "into": source, "startedAt": int(time.time()*1000), "handedAt": None, "finishedAt": None, "guidance": None, "step": 0, "failure": failure, "finished": False,
            "files": files, "clean": [], "checks": [], "before": {"status": "ready_to_merge", "acceptance": record.get("acceptance"), "result": None, "digest": None}}


def resolve(h, root, record, params):
    path = task_path(h, root, record)
    action = params["action"]
    merge = record.get("merge")
    if not merge:
        raise h.Refused("no_conflict", "There is no retained merge conflict for this task.")
    if action == "resolve_block":
        file = next((f for f in merge["files"] if f["path"] == params.get("path")), None)
        if file is None:
            raise h.Refused("stale_conflict", "This conflict changed. Read it again.")
        block = next((b for b in file["blocks"] if b["id"] == params.get("blockId")), None)
        choice = params.get("choice")
        if block is None or choice not in ("ours", "theirs", "both"):
            raise h.Refused("invalid_choice", "Choose one of the displayed file versions.")
        if block.get("cortexNote"):
            raise h.Refused("unsafe_conflict", block["cortexNote"])
        stages = [line.split("\t", 1)[0].split() for line in h.out(path, "ls-files", "-u", "--", file["path"]).splitlines()]
        if {s[2] for s in stages} != {"1", "2", "3"} or len({s[0] for s in stages}) != 1 or any(s[0] not in {"100644", "100755"} for s in stages):
            raise h.Refused("unsafe_conflict", "Resolve add/delete, symlink or mode conflicts with Git in your editor.")
        target = Path(path) / h.relative(file["path"])
        h.inside(path, file["path"])
        if target.is_symlink():
            raise h.Refused("unsafe_conflict", "Resolve this symlink conflict with Git yourself.")
        text = "\n".join(block["results"][choice]["lines"]) + "\n"
        if "\0" in text:
            raise h.Refused("binary_conflict", "Resolve this binary conflict with Git yourself.")
        target.write_text(text)
        h.git(path, "add", "--", file["path"])
        block["choice"] = choice
    elif action == "finish_merge":
        if h.out(path, "diff", "--name-only", "--diff-filter=U").strip() or h.git(path, "diff", "--cached", "--check", check=False).returncode:
            raise h.Refused("unresolved_conflict", "Resolve and stage every conflict without markers first.")
        if h.git(path, "rev-parse", "--verify", "MERGE_HEAD", check=False).returncode == 0:
            h.git(path, "commit", "-m", "Resolve task merge conflicts")
        record.update(conflict="none", resolvedBy="you", acceptance=None)
        record.pop("status", None)
        merge.update(finished=True, finishedAt=int(time.time()*1000))
    elif action == "abort_merge":
        h.git(path, "merge", "--abort")
        record.update(conflict="none", merge=None, acceptance=None)
    elif action == "resolution_started":
        record.update(conflict="resolving", acceptance=None)
        merge.update(handedAt=int(time.time()*1000), guidance=params.get("guidance"))
    else:
        raise h.Refused("unsupported_action", "That merge action is unavailable.")


def runtime_result(h, root, project, task, wanted, integration=False):
    if not (project_store_directory(root) / "cortex.sqlite3").is_file():
        raise h.Refused("result_unavailable", "Activate Cortex and review its runtime result first.")
    from cortex_runtime.store import Store
    from cortex_runtime.workspace_api.projections import result_actions, result_projection, _facts
    store = Store(project_store_directory(root), project_root=root)
    with store.connection() as db:
        row = db.execute("SELECT e.* FROM workspace_executions e JOIN workspace_tasks t USING(task_id) WHERE t.project_id=? AND e.task_id=? AND e.execution_id=t.execution_id", (project, task)).fetchone()
        if row is None or _facts(db, row, root)[1].state != "result":
            raise h.Refused("result_unavailable", "Only a completed runtime result can be accepted.")
        result = result_projection(db, store, project, "wr_" + row["execution_id"])
    digest = result.get("observedWorkspaceDigest") or f'result:{result["resultId"]}:{result["version"]}'
    if digest != wanted or result.get("warnings") or result.get("checksAvailability") == "unavailable":
        raise h.Refused("stale_result", "This result or its workspace changed. Rerun checks and review the new result.")
    if integration and result["kind"] != "work":
        raise h.Refused("checks_required", "This is an informational answer, not verified code for merging.")
    actions = result_actions(result)
    if not actions["accept"]:
        raise h.Refused("checks_required", actions["reason"])
    return result


def integration_facts(h, root, record, task=None):
    """Read local containment, independently of remote publication. Never alter the real index."""
    target = record.get("targetBranch") or (h.default_branch(root) if h.is_repo(root) else "")
    facts = dict(targetBranch=target, alreadyInTarget=False, pushStatus=None)
    if not h.is_repo(root):
        return facts
    try:
        target = valid_branch(h, root, target)
        if h.out(root, "remote").strip():
            upstream = h.out(root, "for-each-ref", "--format=%(upstream)", "refs/heads/" + target).strip()
            facts["pushStatus"] = "unknown"
            if upstream and not h.git(root, "rev-parse", "--verify", upstream, check=False).returncode:
                facts["pushStatus"] = "pushed" if not h.out(root, "rev-list", upstream + "..refs/heads/" + target).strip() else "not_pushed"
        path = task_path(h, root, record)
        if h.out(path, "branch", "--show-current").strip() == target:
            import same_branch
            facts.update(same_branch.facts(h, root, path, record, task, target))
            return facts
        base = record.get("baseRevision")
        if not base or h.git(path, "rev-parse", "--verify", base + "^{commit}", check=False).returncode:
            return facts
        if h.out(path, "diff", "--name-only", "--diff-filter=U").strip():
            return facts
        objects = h.out(path, "rev-parse", "--path-format=absolute", "--git-path", "objects").strip()
        # Temporary objects and index prevent even a read from writing user Git state.
        with tempfile.TemporaryDirectory(prefix="cortex-containment-") as folder:
            obj = Path(folder) / "objects"
            obj.mkdir()
            env = dict(h.git_env(), GIT_INDEX_FILE=str(Path(folder) / "index"),
                       GIT_OBJECT_DIRECTORY=str(obj), GIT_ALTERNATE_OBJECT_DIRECTORIES=objects)
            def shadow(*args, data=None):
                config = h.inspection_config(path) if args[0] in h.READ_GIT else ["-c", "core.fsmonitor=false"]
                done = subprocess.run(["git", *config, *args], cwd=path, env=env,
                                      input=data, capture_output=True, timeout=8)
                if done.returncode:
                    raise ValueError("containment_unavailable")
                return done.stdout
            shadow("read-tree", "HEAD")
            names = set(h.out(path, "diff", "--name-only", "-z", "HEAD").split("\0"))
            names.update(h.out(path, "ls-files", "--others", "--exclude-standard", "-z").split("\0"))
            for name in sorted(n for n in names if n and not helper_path(n)):
                file = Path(path) / h.relative(name)
                if not file.exists() and not file.is_symlink():
                    shadow("update-index", "--force-remove", "--", name)
                    continue
                if file.is_symlink():
                    data, mode = os.readlink(file).encode(), "120000"
                else:
                    h.inside(path, name)
                    if not file.is_file() or file.stat().st_size > 2_000_000:
                        return facts
                    data = file.read_bytes()
                    mode = "100755" if file.stat().st_mode & stat.S_IXUSR else "100644"
                oid = shadow("hash-object", "-w", "--stdin", data=data).decode().strip()
                shadow("update-index", "--add", "--cacheinfo", mode, oid, name)
            exclusions = [":(exclude)" + n for n in (".cortex", ".cortex-dev", ".cortex-dev-owner.json", ".cortex-workspace-dev.json", ".cortex-workspace.json")]
            patch = shadow("diff", "--cached", "--no-ext-diff", "--no-textconv", "--binary", "--full-index", base, "--", ".", *exclusions)
            if len(patch) > 2_000_000:
                return facts
            if patch:
                shadow("read-tree", "refs/heads/" + target)
                shadow("apply", "--reverse", "--check", "--cached", "--whitespace=nowarn", data=patch)
            facts["alreadyInTarget"] = True
    except (h.Refused, ValueError, OSError, subprocess.TimeoutExpired):
        pass  # Unknown is never a positive containment or push assertion.
    return facts


def require_settled(h, root, project, task):
    """A durable owner overlay cannot hide a live or unobservable execution."""
    db = h.store_reader(root)
    if db is None:
        return
    try:
        from cortex_runtime.workspace_api.projections import _facts
        row = db.execute("SELECT e.* FROM workspace_executions e JOIN workspace_tasks t USING(task_id) "
                         "WHERE t.task_id=? AND t.project_id=?", (task, project)).fetchone()
        if row is None:
            return
        mapping = _facts(db, row, str(root))[1]
        live = db.execute("SELECT 1 FROM runtime_nodes WHERE task_id=? AND status='running' LIMIT 1",
                          (row["runtime_task_id"],)).fetchone()
        if mapping.state not in ("paused", "stopped", "result", "failed") or live:
            raise h.Refused("stop_required", "Stop the task and wait for every worker to stop first.")
    finally:
        db.close()


def integrate(h, root, project, state, params):
    if params.get("confirmed") is not True:
        raise h.Refused("confirmation_required", "Confirm this Git action first.")
    task = params.get("taskId")
    with h.Locked(state):
        product = h.product_of(state, project)
        record = product["tasks"].get(task) or {}
        if record.get("status") != "ready_to_merge" or not record.get("acceptance") or record["acceptance"].get("digest") != params.get("digest"):
            raise h.Refused("stale_result", "Review and accept the current result before finishing.")
        facts = integration_facts(h, root, dict(record, targetBranch=params.get("targetBranch") or record.get("targetBranch")), task)
        if facts["alreadyInTarget"]:
            raise h.Refused("already_in_target", "Already in " + facts["targetBranch"] + ". Confirm Close task instead; no Git operation is needed.")
        verified = runtime_result(h, root, project, task, params.get("digest"), True)
        if verified is not None and record.get("requiresFreshResult") == {"resultId": verified["resultId"], "version": str(verified["version"])}:
            raise h.Refused("review_required", "The source changed after this result. Run fresh checks and review the new result before integration.")
        path = task_path(h, root, record)
        if record.get("acceptedRevision"):
            from cortex_runtime.delegation.revision import capture_revision
            current = capture_revision(path)
            if current["kind"] != "observed" or current["dirty_digest"] != record["acceptedRevision"]:
                raise h.Refused("workspace_drift", "Files changed after acceptance. Rerun checks and accept the new result.")
        if not h.is_repo(path):
            raise h.Refused("git_required", "The accepted changes are already in the project folder. Close the task as-is, or initialize Git before using merge or PR operations.")
        source = h.out(path, "branch", "--show-current").strip()
        target = valid_branch(h, root, params.get("targetBranch") or record.get("targetBranch") or h.default_branch(root))
        base = record.get("baseRevision")
        if base and h.git(path, "merge-base", "--is-ancestor", base, target, check=False).returncode:
            raise h.Refused("incompatible_target", "This target predates the task base. Choose a descendant branch, or integrate this patch manually.")
        message = params.get("message")
        if not isinstance(message, str) or not message.strip():
            raise h.Refused("empty_message", "Write a commit message first.")
        if not source:
            raise h.Refused("detached_checkout", "A task must have a branch before merging.")
        if h.out(path, "diff", "--name-only", "--diff-filter=U").strip():
            raise h.Refused("merge_conflict", "Resolve the task's conflict first.")
        method = params.get("method")
        if source == target:
            if method != "commit":
                raise h.Refused("same_branch", "This task is already on " + target + ". Use Commit to this branch, or confirm Close task; there is no branch integration.")
            if not facts.get("commitAvailable"):
                raise h.Refused("task_content_unknown", facts.get("reason") or "Refresh the task before committing.")
        elif method not in ("squash", "pr"):
            raise h.Refused("unsupported_action", "Choose squash merge or pull request.")
        if method == "pr" and not shutil.which("gh"):
            raise h.Refused("gh_unavailable", "Install and sign in to GitHub CLI (gh) to create a pull request.")
        target_tree = next((t["path"] for t in h.worktrees(root) if t.get("branch") == target), None)
        if target_tree and target_tree != path and h.status_files(target_tree):
            raise h.Refused("dirty_target", "The target checkout has uncommitted work. Commit or move it yourself first.")
        if source == target and method == "pr":
            raise h.Refused("same_branch", "Choose a different target branch for a pull request.")
        try:
            with index_transaction(h, path):
                if method == "commit":
                    import same_branch
                    current_facts = integration_facts(h, root, record, task)
                    if not current_facts.get("commitAvailable") or current_facts["pendingPaths"] != facts["pendingPaths"]:
                        raise h.Refused("task_content_unknown", "Task files changed. Refresh before committing.")
                    same_branch.commit(h, path, facts["pendingPaths"], message)
                else:
                    stage_work(h, path)
                    if h.git(path, "diff", "--cached", "--quiet", check=False).returncode:
                        h.git(path, "commit", "-m", message)
        except h.Refused as error:
            if error.code in ("git_failed", "git_timeout"):
                raise h.Refused("integration_failed", "Git could not save the task changes. The previous staging selection was restored and your files are unchanged. Check your Git identity and commit hooks in this checkout, fix the reported local problem in Git, then retry " + ("Commit." if method == "commit" else "Merge.")) from None
            raise
        source_head = h.out(path, "rev-parse", "HEAD").strip()
        if method == "commit":
            record.update(status="done", work="merged", doneAt=int(time.time()*1000), integration={"method": "commit", "targetBranch": target, "digest": params["digest"], "message": message, "sourceHead": source_head})
            h.write_json(h.product_path(state, project), product)
            return {"done": True}
        if source != target and h.git(path, "merge-base", "--is-ancestor", target, source, check=False).returncode:
            merged = h.git(path, "merge", "-m", "Update task branch from target", "--", target, check=False)
            if merged.returncode:
                if not h.out(path, "diff", "--name-only", "--diff-filter=U").strip():
                    raise h.Refused("git_failed", "Git could not update the task branch. Inspect it before retrying.")
                if verified is not None:
                    record["requiresFreshResult"] = {"resultId": verified["resultId"], "version": str(verified["version"])}
                record.update(conflict="conflict", merge=conflicts(h, path, source, target, record), acceptance=None)
                h.write_json(h.product_path(state, project), product)
                raise h.Refused("merge_conflict", "The conflict is in the task worktree. The target was not changed.")
            if verified is not None:
                record["requiresFreshResult"] = {"resultId": verified["resultId"], "version": str(verified["version"])}
            record.update(acceptance=None, automaticReady=False)
            h.write_json(h.product_path(state, project), product)
            raise h.Refused("review_required", "The task branch was updated from the target. Rerun checks and review it before merging.")
        if method == "pr":
            if record.get("prAttempt") or (record.get("integration") or {}).get("method") == "pr":
                raise h.Refused("unknown_outcome", "A pull request was already attempted. Check GitHub before creating another.")
            # No push is hidden here. gh requires the branch to have been pushed explicitly.
            if h.git(path, "rev-parse", "--verify", "@{u}", check=False).returncode or h.out(path, "rev-list", "@{u}..HEAD").strip():
                raise h.Refused("push_required", "Push this task branch from Git first, then create the pull request.")
            record["prAttempt"] = {"state":"unknown", "target":target, "head":source}
            h.write_json(h.product_path(state, project), product)
            try:
                done = subprocess.run(["gh", "pr", "create", "--base", target, "--head", source, "--title", message.splitlines()[0], "--body", params.get("body") or "", * (["--draft"] if params.get("draft") else [])],
                                      cwd=path, env=h.git_env(), capture_output=True, timeout=25)
            except subprocess.TimeoutExpired:
                raise h.Refused("unknown_outcome", "GitHub did not answer. Check your pull requests before retrying.") from None
            if done.returncode:
                raise h.Refused("gh_failed", "GitHub CLI could not create the pull request. Check gh login and repository access.")
            record["integration"] = {"method": "pr", "targetBranch": target, "digest": params["digest"], "title": message}
        else:
            temporary = None
            if source != target:
                if target_tree is None:
                    temporary = Path(state) / "merge-targets" / project / task
                    temporary.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                    h.git(root, "worktree", "add", str(temporary), target)
                    target_tree = str(temporary)
                try:
                    with index_transaction(h, target_tree):
                        h.git(target_tree, "merge", "--squash", "--", source)
                        if h.git(target_tree, "diff", "--cached", "--quiet", check=False).returncode:
                            h.git(target_tree, "commit", "-m", message)
                except h.Refused as error:
                    if error.code in ("git_failed", "git_timeout"):
                        raise h.Refused("integration_failed", "Git could not finish the target commit. Its previous staging selection was restored; the task commit is retained. Inspect the target checkout in Changes and fix its Git identity or commit hooks, then commit the retained changes there and refresh the task. Do not repeat Merge while the target has uncommitted changes.") from None
                    raise
                if temporary is not None:
                    h.git(root, "worktree", "remove", str(temporary))
            record.update(status="done", work="merged", doneAt=int(time.time()*1000), integration={"method": "squash", "targetBranch": target, "digest": params["digest"], "message": message, "sourceHead": source_head})
        # Commit the integration before optional cleanup: cleanup failure cannot undo a merge.
        h.write_json(h.product_path(state, project), product)
        if method == "squash" and params.get("deleteWorktree") is True and (record.get("checkout") or {}).get("kind") == "worktree":
            cleanup = "retained"
            if source != target and not h.status_files(path):
                try:
                    h.git(root, "worktree", "remove", "--", path)
                    cleanup = "removed"
                except h.Refused:
                    pass
            record["integration"]["worktreeCleanup"] = cleanup
            h.write_json(h.product_path(state, project), product)
    return {"done": True}
