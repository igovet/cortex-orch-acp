"""T025: technical application of an isolated worker workspace onto the
main project root, against its known base, with pure-Python conflict
detection -- never last-write-wins.

``integrate`` is the one function this module exposes for production use.
It is idempotent (``UNIQUE(assignment_ref, request_key)`` on
``workspace_integrations``): a retry with the same key returns the
previously recorded outcome unchanged, never re-applying or re-deciding.

Algorithm (see the T025 task card / lead design notes):

1. Load the attempt's base manifest (``workspace_snapshots.manifest_json``,
   written at snapshot time -- see ``workspace_isolation``) and its current
   workspace manifest (rescanned now, same exclusions/caps). Diffing the
   two gives the worker's own change set (added/modified/deleted), never
   assuming a change just because a path exists. The pristine
   ``base_path`` copy (retained read-only alongside the engine's own
   mutable ``root_path`` copy -- see ``workspace_isolation``) supplies the
   *raw base bytes* a three-way text merge needs; the manifest alone only
   ever retains a sha256.
2. For every changed path, read the *current* state of that one path in
   the main project root:
   - unchanged since base (matches the base entry exactly) -> apply the
     worker's version directly.
   - changed since base and the worker's edit does not overlap it -> a
     line-level three-way text merge (base/main/worker) for two regular
     text files; any binary content, kind mismatch (file vs symlink vs
     directory vs deletion), or overlapping edit -> a conflict, never a
     guess.
   - a host-negotiated *unsaved buffer* for that exact path (see
     ``host_fs_checker``) is treated the same as "changed since base and
     conflicting" -- refused, never silently overwritten by an old
     engine-side result.
3. Every path is checked *before* any write (no filesystem mutation during
   step 2) -- if even one path conflicts, nothing is written at all
   (all-or-nothing, to the extent a plain filesystem allows: see the
   "Known limitations" note below for the one gap this does not close).
4. Only once every changed path is a clean apply are the writes actually
   performed, each as its own atomic temp-write + ``fsync`` + ``rename``
   (mode preserved), never in place.
5. The resulting main project root's own T019 revision
   (``delegation.revision.capture_revision``) is captured and durably
   recorded as this integration's ``integrated_head``/``integrated_dirty_digest``
   -- this is the "new" revision a fresh check must bind to; see
   ``delegation.completion``'s ``stale_verification`` gate, which already
   refuses to certify a revision a claimed check never actually ran
   against.

Known limitations (documented, not fixed by this task): applying more than
one file is not one atomic filesystem transaction -- a crash *between* two
already-conflict-free per-file renames could leave a partial apply on
disk. The pre-check in step 3 means this can only ever leave *fewer* of
the worker's own (already known-safe, non-conflicting) changes applied,
never a corrupted file and never someone else's untouched content
overwritten; a resumed/retried ``integrate`` call with the same
``request_key`` is refused (see idempotency above) rather than silently
retried past a partial apply -- reconciling a partial apply out of band is
left to the coordinator, matching this project's existing "reconcile
uncertain mutations before retry" convention (see also
``delegation.provisioning``'s own intent/outcome pattern for the same
class of gap).
"""
from __future__ import annotations

import codecs
import difflib
import json
import os
import re
import stat
import subprocess
import uuid
from datetime import datetime, timezone
from pathlib import Path

from ..contracts import StoreError
from . import revision as delegation_revision
from . import workspace_isolation

INTEGRATION_SCHEMA = '''
CREATE TABLE IF NOT EXISTS workspace_integrations (
 id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 assignment_ref TEXT NOT NULL,
 request_key TEXT NOT NULL,
 attempt_id TEXT NOT NULL,
 workspace_binding_id TEXT,
 status TEXT NOT NULL,
 applied_paths_json TEXT NOT NULL,
 conflict_paths_json TEXT NOT NULL,
 integrated_head TEXT,
 integrated_dirty_digest TEXT,
 host_buffer_state TEXT NOT NULL DEFAULT 'unknown',
 created_at TEXT NOT NULL,
 UNIQUE(assignment_ref, request_key)
);
CREATE TABLE IF NOT EXISTS workspace_integration_journal (
 journal_key TEXT NOT NULL,
 relpath TEXT NOT NULL,
 before_json TEXT NOT NULL,
 before_bytes BLOB,
 desired_json TEXT NOT NULL,
 state TEXT NOT NULL,
 created_at TEXT NOT NULL,
 PRIMARY KEY(journal_key, relpath)
);
'''

_EXCLUDED_TOP_LEVEL = {".git", ".cortex"}
_MAX_MERGE_FILE_BYTES = 64 * 1024 * 1024


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _existing_row(store, assignment_ref: str, request_key: str):
    with store.connection() as db:
        return db.execute(
            "SELECT * FROM workspace_integrations WHERE assignment_ref=? AND request_key=?",
            (assignment_ref, request_key),
        ).fetchone()


def _result_from_row(row) -> dict:
    return dict(
        id=row["id"], status=row["status"],
        applied_paths=json.loads(row["applied_paths_json"]),
        conflict_paths=json.loads(row["conflict_paths_json"]),
        integrated_head=row["integrated_head"], integrated_dirty_digest=row["integrated_dirty_digest"],
        host_buffer_state=row["host_buffer_state"],
    )


def _journal_key(assignment_ref: str, request_key: str) -> str:
    return f"{assignment_ref}:{request_key}"


def _json_entry(entry):
    if entry is None:
        return None
    return {key: value for key, value in entry.items() if key != "bytes"}


def _recover_journal(store, project_root: str, assignment_ref: str, request_key: str) -> None:
    """Roll back an interrupted multi-file apply when every touched path is
    still exactly the value Cortex wrote. External drift is preserved and
    surfaced as an uncertain integration instead of being overwritten."""
    key = _journal_key(assignment_ref, request_key)
    with store.connection() as db:
        if db.execute(
            "SELECT 1 FROM workspace_integrations WHERE assignment_ref=? AND request_key=?",
            (assignment_ref, request_key),
        ).fetchone() is not None:
            db.execute("DELETE FROM workspace_integration_journal WHERE journal_key=?", (key,))
            return
        rows = db.execute(
            "SELECT * FROM workspace_integration_journal WHERE journal_key=?",
            (key,),
        ).fetchall()
    if not rows:
        return
    for row in rows:
        current = _read_current_entry(project_root, row["relpath"])
        desired = json.loads(row["desired_json"])
        if not _entries_equal(current, desired):
            raise StoreError("integration_recovery_external_drift")
    for row in rows:
        relpath = row["relpath"]
        before = json.loads(row["before_json"])
        full = os.path.join(project_root, relpath)
        if before is None:
            _delete(project_root, relpath)
        elif before.get("kind") == "f":
            _write_atomic(project_root, relpath, row["before_bytes"] or b"", before.get("mode", 0o600))
        elif before.get("kind") == "l":
            try:
                os.remove(full)
            except FileNotFoundError:
                pass
            os.makedirs(os.path.dirname(full), exist_ok=True)
            os.symlink(before["target"], full)
        elif before.get("kind") == "d":
            os.makedirs(full, mode=before.get("mode", 0o700), exist_ok=True)
            os.chmod(full, before.get("mode", 0o700))
        else:
            raise StoreError("integration_recovery_unrestorable_entry")
    with store.connection() as db:
        db.execute("DELETE FROM workspace_integration_journal WHERE journal_key=?", (key,))


def _journal_prepare(store, key, project_root, relpath, before, desired):
    before_bytes = before.get("bytes") if before and before.get("kind") == "f" else None
    with store.connection() as db:
        db.execute(
            "INSERT OR REPLACE INTO workspace_integration_journal "
            "(journal_key,relpath,before_json,before_bytes,desired_json,state,created_at) VALUES (?,?,?,?,?,?,?)",
            (key, relpath, json.dumps(_json_entry(before), sort_keys=True), before_bytes,
             json.dumps(_json_entry(desired), sort_keys=True), "prepared", _now()),
        )


def _journal_applied(store, key, relpath):
    with store.connection() as db:
        db.execute(
            "UPDATE workspace_integration_journal SET state='applied' WHERE journal_key=? AND relpath=?",
            (key, relpath),
        )


def _record(store, *, task_id, assignment_ref, request_key, attempt_id, workspace_binding_id,
            status, applied_paths, conflict_paths, integrated, host_buffer_state) -> dict:
    row_id = "wi_" + uuid.uuid4().hex[:12]
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            db.execute(
                "INSERT INTO workspace_integrations VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)",
                (row_id, task_id, assignment_ref, request_key, attempt_id, workspace_binding_id, status,
                 json.dumps(sorted(applied_paths)), json.dumps(conflict_paths, sort_keys=True),
                 integrated.get("head") if integrated else None,
                 integrated.get("dirty_digest") if integrated else None, host_buffer_state, _now()),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return dict(id=row_id, status=status, applied_paths=sorted(applied_paths),
                conflict_paths=conflict_paths, integrated_head=integrated.get("head") if integrated else None,
                integrated_dirty_digest=integrated.get("dirty_digest") if integrated else None,
                host_buffer_state=host_buffer_state)


# -- path-scoped filesystem reads (main project root) -----------------------

# Generated artifacts that are never a deliverable, whether or not the
# project is a repository with its own ignore rules.
_ALWAYS_GENERATED = (
    re.compile(r"(?:^|/)__pycache__(?:/|$)"),
    re.compile(r"\.py[co]$"),
)


def _not_deliverable(project_root: str, relpaths) -> set[str]:
    """The subset of ``relpaths`` the project does not track: what its own
    git ignore rules exclude, plus always-generated artifacts.

    Asks git once, in the project (the worker's copy has no ``.git`` of
    its own by design). A project that is not a repository, or a git that
    fails for any reason, simply contributes nothing here -- the built-in
    list still applies, and nothing is ever *added* to the delivery by
    this function.
    """
    ignored = {path for path in relpaths if any(rule.search(path) for rule in _ALWAYS_GENERATED)}
    candidates = [path for path in relpaths if path not in ignored]
    if not candidates:
        return ignored
    try:
        result = subprocess.run(
            ["git", "-C", project_root, "check-ignore", "--stdin"],
            input="\n".join(candidates), capture_output=True, text=True, timeout=10, check=False,
        )
    except Exception:  # noqa: BLE001 - no git, no repository, no timeout budget: not a failure
        return ignored
    if result.returncode not in (0, 1):
        return ignored
    ignored.update(line.strip() for line in result.stdout.splitlines() if line.strip())
    return ignored


def _is_technical(relpath: str) -> bool:
    top = relpath.split("/", 1)[0]
    return top in _EXCLUDED_TOP_LEVEL


def _read_current_entry(project_root: str, relpath: str):
    """Read the current state of one path under ``project_root`` as a
    manifest-shaped dict (``None`` if absent). Never follows a symlink to
    decide "regular file" -- matches ``workspace_isolation``'s own
    conventions."""
    full = os.path.join(project_root, relpath)
    try:
        info = os.lstat(full)
    except OSError:
        return None
    if stat.S_ISLNK(info.st_mode):
        return dict(kind="l", target=os.readlink(full), mode=stat.S_IMODE(info.st_mode))
    if stat.S_ISDIR(info.st_mode):
        return dict(kind="d", mode=stat.S_IMODE(info.st_mode))
    if not stat.S_ISREG(info.st_mode):
        return dict(kind="o", mode=stat.S_IMODE(info.st_mode))
    fd = os.open(full, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode):
            return dict(kind="o", mode=stat.S_IMODE(st.st_mode))
        import hashlib
        h = hashlib.sha256()
        data = bytearray()
        too_large = False
        while True:
            chunk = os.read(fd, 1024 * 1024)
            if not chunk:
                break
            h.update(chunk)
            if not too_large:
                data.extend(chunk)
                if len(data) > _MAX_MERGE_FILE_BYTES:
                    too_large = True
        entry = dict(kind="f", sha256=h.hexdigest(), mode=stat.S_IMODE(st.st_mode))
        entry["bytes"] = None if too_large else bytes(data)
        return entry
    finally:
        os.close(fd)


def _entries_equal(a, b) -> bool:
    if a is None or b is None:
        return a is None and b is None
    if a.get("kind") != b.get("kind"):
        return False
    if a["kind"] == "f":
        return a.get("sha256") == b.get("sha256")
    if a["kind"] == "l":
        return a.get("target") == b.get("target")
    return True


def _read_bytes(root: str, relpath: str) -> bytes:
    full = os.path.join(root, relpath)
    fd = os.open(full, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    try:
        st = os.fstat(fd)
        if not stat.S_ISREG(st.st_mode):
            raise StoreError("unsafe_storage")
        chunks = []
        while True:
            chunk = os.read(fd, 1024 * 1024)
            if not chunk:
                break
            chunks.append(chunk)
        return b"".join(chunks)
    finally:
        os.close(fd)


def _decode_text(data: bytes):
    try:
        codecs.decode(data, "utf-8", "strict")
    except UnicodeDecodeError:
        return None
    return data.decode("utf-8").splitlines(keepends=True)


def _edit_ops(base_lines, other_lines):
    matcher = difflib.SequenceMatcher(None, base_lines, other_lines, autojunk=False)
    return [op for op in matcher.get_opcodes() if op[0] != "equal"]


def _ranges_conflict(a, b) -> bool:
    (i1a, i2a), (i1b, i2b) = a, b
    return i1a <= i2b and i1b <= i2a


def merge_three_way(base_lines, main_lines, worker_lines):
    """Line-level three-way merge. Returns ``(merged_lines, None)`` on a
    clean (non-overlapping) merge, or ``(None, conflict_detail)`` if any
    edit range overlaps -- conservative by design (an edit "touching" the
    same base line range on both sides is always a conflict, even if the
    resulting text would happen to agree)."""
    main_ops = _edit_ops(base_lines, main_lines)
    worker_ops = _edit_ops(base_lines, worker_lines)
    for main_op in main_ops:
        for worker_op in worker_ops:
            if _ranges_conflict((main_op[1], main_op[2]), (worker_op[1], worker_op[2])):
                return None, dict(reason="overlapping_edit",
                                   main_range=[main_op[1], main_op[2]], worker_range=[worker_op[1], worker_op[2]])
    tagged = [("main", op) for op in main_ops] + [("worker", op) for op in worker_ops]
    tagged.sort(key=lambda t: (t[1][1], t[1][2]))
    merged = []
    cursor = 0
    for origin, op in tagged:
        tag, i1, i2, j1, j2 = op
        merged.extend(base_lines[cursor:i1])
        if tag in ("replace", "insert"):
            source = main_lines if origin == "main" else worker_lines
            merged.extend(source[j1:j2])
        cursor = i2
    merged.extend(base_lines[cursor:])
    return merged, None


def _write_atomic(project_root: str, relpath: str, data: bytes, mode: int) -> None:
    full = os.path.join(project_root, relpath)
    parent = os.path.dirname(full)
    os.makedirs(parent, exist_ok=True)
    tmp = full + f".cortex-integrate-{uuid.uuid4().hex[:8]}"
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    try:
        os.write(fd, data)
        os.fchmod(fd, mode)
        os.fsync(fd)
    finally:
        os.close(fd)
    os.replace(tmp, full)
    dir_fd = os.open(parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(dir_fd)
    finally:
        os.close(dir_fd)


def _delete(project_root: str, relpath: str) -> None:
    full = os.path.join(project_root, relpath)
    try:
        if os.path.isdir(full) and not os.path.islink(full):
            os.rmdir(full)
        else:
            os.remove(full)
    except FileNotFoundError:
        pass


def integrate(
    store, *, task_id: str, project_root: str, assignment_ref: str, request_key: str,
    attempt_id: str, workspace_binding_id: str, host_fs_checker=None,
) -> dict:
    """Apply attempt ``attempt_id``'s isolated workspace changes onto
    ``project_root``. Idempotent per ``(assignment_ref, request_key)``.

    ``host_fs_checker`` (optional) -- ``(relpath) -> bytes | None``, may
    raise -- a caller-supplied read of the *host's* own negotiated buffer
    for a project-relative path (see ``engine.fs_bridge.HostFsForwarder``),
    or the whole parameter is ``None`` when the host has no such
    buffer/capability at all (recorded ``host_buffer_state: "unknown"``).
    Only ever consulted for a path this integration is about to modify; a
    path the worker never touched is never blocked on a host buffer it does
    not change. Per path: a clean ``None`` return means "host confirms no
    buffer content here" (not a conflict by itself); *raising* means the
    round-trip itself could not be trusted (timeout, closed transport,
    malformed reply) and is always treated as a conflict for that path
    (recorded ``host_buffer_state: "check_failed"``) -- uncertainty is never
    silently treated the same as "checked, no conflict".
    """
    existing = _existing_row(store, assignment_ref, request_key)
    if existing is not None:
        return _result_from_row(existing)
    _recover_journal(store, project_root, assignment_ref, request_key)

    # In the ACP product a worker edits the exact project root selected by
    # Zed.  The historical isolated-snapshot path is still supported for
    # retained tasks and migration, but a direct/shared binding has no copy
    # to diff or apply.  Record a durable reconciliation outcome instead of
    # rejecting it as "not isolated" (or, worse, trying to copy an old
    # workspace over the user's current files).  The revision is captured
    # after the worker result and is the evidence boundary used by the
    # completion gate.  This operation remains idempotent through the same
    # integration row as the isolated path.
    if workspace_binding_id in {"shared_project_root", "serialized_shared_root"}:
        integrated = delegation_revision.capture_revision(project_root)
        return _record(
            store, task_id=task_id, assignment_ref=assignment_ref,
            request_key=request_key, attempt_id=attempt_id,
            workspace_binding_id=workspace_binding_id, status="noop",
            applied_paths=[], conflict_paths={}, integrated=integrated,
            # There is no path delta to probe here, and therefore the
            # callback was not invoked.  Merely having a callback available
            # is not evidence that the host buffer was checked.
            host_buffer_state="unknown",
        )

    # T025 (lead follow-up) / EXT-01 #5 (three-state fix): a host that never
    # negotiated fs (or whose capability the caller does not have a live
    # requester for -- ``host_fs_checker`` is ``None``) means this
    # integration cannot know whether an unsaved buffer exists for any
    # touched path -- recorded as ``unknown`` (capability absent), a visible,
    # durable limitation, rather than silently treated the same as "checked,
    # no conflict". A *third* state, ``check_failed``, is recorded below when
    # a requester *was* supplied but at least one touched path's own
    # round-trip failed (timeout/closed transport/malformed reply, see
    # ``acp.orchestrating_runtime._make_host_fs_checker``) -- that failure
    # itself is never conflated with "no buffer" (which stays a clean,
    # non-exception ``None`` return): each such path is refused as a
    # conflict below, exactly like a positively-observed unsaved buffer,
    # because uncertainty must never become permission to overwrite.
    host_buffer_state = "checked" if host_fs_checker is not None else "unknown"
    buffer_check_failed = False

    provisioner = workspace_isolation.IsolatedSnapshotWorkspaceProvisioner(store, project_root, task_id)
    binding_row = provisioner.row_for_binding(workspace_binding_id)
    if binding_row is None:
        # Shared-root adapters intentionally do not create a snapshot row;
        # their binding is the explicit sentinel used by provisioning.
        if workspace_binding_id not in {"shared_project_root", "serialized_shared_root"}:
            raise StoreError("workspace_binding_not_found")
    if binding_row["mode"] != "isolated_snapshot":
        # A legacy shared-root row has no trustworthy base/work pair. Treating
        # it as a no-op would allow acceptance to hide an unverified direct
        # write, so integration is refused explicitly.
        raise StoreError("workspace_not_isolated")

    workspace_root = binding_row["root_path"]
    base_root = binding_row["base_path"]
    base_manifest = json.loads(binding_row["manifest_json"])
    worker_manifest = _rescan_workspace(workspace_root)

    changed = {}
    for relpath in set(base_manifest) | set(worker_manifest):
        base_entry = base_manifest.get(relpath)
        worker_entry = worker_manifest.get(relpath)
        if _entries_equal(base_entry, worker_entry):
            continue
        changed[relpath] = (base_entry, worker_entry)
    # Whatever the project itself does not track is not something a worker
    # delivers. Live: running the self-check inside the copy produced
    # __pycache__/calc.cpython-312.pyc, integration treated it as part of
    # the delivery, the host had no such file, and the whole integration
    # stopped on it -- the real change never reached the project.
    for relpath in _not_deliverable(project_root, changed):
        changed.pop(relpath, None)

    plan_writes: dict[str, tuple] = {}
    plan_deletes: set[str] = set()
    conflicts: dict[str, dict] = {}
    # Keep the exact main entries inspected during planning. The short
    # recheck immediately before mutation catches another Cortex integration
    # racing with this one; it cannot claim to serialize arbitrary external
    # editors, which remains an observable conflict/uncertain outcome.
    planned_main: dict[str, object] = {}

    for relpath, (base_entry, worker_entry) in changed.items():
        if _is_technical(relpath):
            # A worker's workspace never legitimately contains a top-level
            # .git/.cortex change (excluded at snapshot time); defensive
            # only -- refuse rather than ever touch project storage here.
            conflicts[relpath] = dict(reason="technical_path_refused")
            continue
        main_entry = _read_current_entry(project_root, relpath)
        planned_main[relpath] = main_entry
        if host_fs_checker is not None:
            try:
                host_content = host_fs_checker(relpath)
            except Exception:
                # The round-trip itself failed (or returned something the
                # checker could not trust) -- uncertain, never "no buffer".
                buffer_check_failed = True
                conflicts[relpath] = dict(reason="host_buffer_check_failed")
                continue
            if host_content is not None:
                main_bytes = main_entry.get("bytes") if main_entry and main_entry.get("kind") == "f" else None
                if main_bytes is None or host_content != main_bytes:
                    conflicts[relpath] = dict(reason="unsaved_host_buffer")
                    continue
        if _entries_equal(main_entry, base_entry):
            # Main has not moved since the known base: the worker's edit
            # applies cleanly.
            if worker_entry is None:
                plan_deletes.add(relpath)
            else:
                plan_writes[relpath] = ("direct", worker_entry)
            continue
        # Main changed since base. Only a same-kind regular-file vs
        # regular-file edit is ever merge-eligible; anything else
        # (add/add divergent, delete/modify, kind mismatch, directory or
        # symlink change) is a conflict.
        if worker_entry is not None and _entries_equal(main_entry, worker_entry):
            continue  # main already carries an identical result; nothing to do
        if (base_entry is None or base_entry.get("kind") == "f") and worker_entry is not None and worker_entry.get("kind") == "f" \
                and main_entry is not None and main_entry.get("kind") == "f":
            if main_entry.get("bytes") is None:
                conflicts[relpath] = dict(reason="file_too_large_for_merge")
                continue
            try:
                worker_text = _decode_text(_read_bytes(workspace_root, relpath))
                main_text = _decode_text(main_entry["bytes"])
                base_text = [] if base_entry is None else _decode_text(_read_bytes(base_root, relpath))
            except OSError as error:
                conflicts[relpath] = dict(reason=f"unreadable:{error}")
                continue
            if worker_text is None or main_text is None or base_text is None:
                conflicts[relpath] = dict(reason="binary_conflict")
                continue
            merged, detail = merge_three_way(base_text, main_text, worker_text)
            if merged is None:
                conflicts[relpath] = detail | dict(reason=detail.get("reason", "overlapping_edit"))
                continue
            plan_writes[relpath] = ("merged", dict(kind="f", mode=worker_entry["mode"],
                                                    data="".join(merged).encode("utf-8")))
            continue
        conflicts[relpath] = dict(
            reason="delete_modify_conflict" if (base_entry is None) != (worker_entry is None)
            else "kind_or_binary_conflict",
        )

    if buffer_check_failed:
        # A failed host round-trip always produced its own conflict entry
        # above, so ``conflicts`` is guaranteed non-empty here -- this only
        # ever affects which branch below records; it can never itself let
        # an apply proceed uncertain.
        host_buffer_state = "check_failed"

    if conflicts:
        return _record(
            store, task_id=task_id, assignment_ref=assignment_ref, request_key=request_key,
            attempt_id=attempt_id, workspace_binding_id=workspace_binding_id, status="conflict",
            applied_paths=[], conflict_paths=conflicts, integrated=None, host_buffer_state=host_buffer_state,
        )

    # Serialize Cortex integrations for a project and revalidate the exact
    # precondition under the lock. This closes the check-then-write race
    # between two runtime processes without pretending to control an editor
    # that changes the filesystem after this final check.
    lock_owner = workspace_isolation.SerializedSharedRootWorkspaceProvisioner(project_root)
    with lock_owner.writer_lock(timeout=30):
        race_conflicts = {}
        for relpath, expected in planned_main.items():
            current = _read_current_entry(project_root, relpath)
            if not _entries_equal(current, expected):
                race_conflicts[relpath] = dict(reason="project_changed_during_integration")
        if race_conflicts:
            return _record(
                store, task_id=task_id, assignment_ref=assignment_ref, request_key=request_key,
                attempt_id=attempt_id, workspace_binding_id=workspace_binding_id, status="conflict",
                applied_paths=[], conflict_paths=race_conflicts, integrated=None,
                host_buffer_state=host_buffer_state,
            )
        applied = []
        for relpath, (kind, payload) in plan_writes.items():
            desired = dict(payload)
            if kind == "merged" and desired.get("kind") == "f":
                import hashlib
                desired["sha256"] = hashlib.sha256(desired.pop("data")).hexdigest()
            else:
                desired.pop("data", None)
            journal_key = _journal_key(assignment_ref, request_key)
            _journal_prepare(store, journal_key, project_root, relpath,
                             planned_main.get(relpath), desired)
            if payload["kind"] == "f":
                data = payload["data"] if kind == "merged" else _read_bytes(workspace_root, relpath)
                _write_atomic(project_root, relpath, data, payload["mode"])
            elif payload["kind"] == "l":
                full = os.path.join(project_root, relpath)
                os.makedirs(os.path.dirname(full), exist_ok=True)
                try:
                    os.remove(full)
                except FileNotFoundError:
                    pass
                os.symlink(payload["target"], full)
            elif payload["kind"] == "d":
                os.makedirs(os.path.join(project_root, relpath), exist_ok=True)
            from .. import _fault_injection
            _fault_injection.checkpoint("integration_after_write_before_journal_mark")
            _journal_applied(store, journal_key, relpath)
            applied.append(relpath)
        for relpath in plan_deletes:
            journal_key = _journal_key(assignment_ref, request_key)
            _journal_prepare(store, journal_key, project_root, relpath,
                             planned_main.get(relpath), None)
            _delete(project_root, relpath)
            from .. import _fault_injection
            _fault_injection.checkpoint("integration_after_write_before_journal_mark")
            _journal_applied(store, journal_key, relpath)
            applied.append(relpath)

    integrated = delegation_revision.capture_revision(project_root)
    result = _record(
        store, task_id=task_id, assignment_ref=assignment_ref, request_key=request_key,
        attempt_id=attempt_id, workspace_binding_id=workspace_binding_id,
        status="applied" if applied else "noop",
        applied_paths=applied, conflict_paths={}, integrated=integrated, host_buffer_state=host_buffer_state,
    )
    with store.connection() as db:
        db.execute("DELETE FROM workspace_integration_journal WHERE journal_key=?",
                   (_journal_key(assignment_ref, request_key),))
    return result


def integrate_candidate(
    store, *, task_id: str, project_root: str, candidate_id: str, request_key: str,
    expected_base_digest: str, host_fs_checker=None,
) -> dict:
    """Apply a ready, runtime-owned candidate after an exact base check.

    The candidate is a complete staged tree, so this operation derives a
    direct delta from the recorded base digest to the staged content. It never
    performs a three-way merge: any project drift is a conflict and leaves the
    candidate available for inspection or a new candidate revision.
    """
    from . import candidates

    candidate = candidates.get_candidate(store, candidate_id)
    if candidate is None or candidate.get("task_id") != task_id:
        raise StoreError("candidate_not_found")
    if candidate.get("status") != "ready" or candidate.get("cleaned_up_at"):
        raise StoreError("candidate_not_ready")
    root = Path(candidate["stage_path"])
    store_candidates = Path(workspace_isolation.StorageLocator(project_root).store_dir()) / "candidates"
    try:
        if root.resolve(strict=True).parent.parent != store_candidates.resolve(strict=True):
            raise StoreError("unsafe_candidate_path")
    except FileNotFoundError as error:
        raise StoreError("candidate_stage_missing") from error
    assignment_ref = f"candidate:{candidate_id}"
    existing = _existing_row(store, assignment_ref, request_key)
    if existing is not None:
        return _result_from_row(existing)
    _recover_journal(store, project_root, assignment_ref, request_key)
    staged = candidates._manifest(str(root))
    if candidates._digest_manifest(staged) != candidate.get("content_digest"):
        raise StoreError("candidate_content_changed")
    current = candidates._manifest(project_root)
    current_digest = candidates._digest_manifest(current)
    if current_digest != expected_base_digest:
        return {"status": "conflict", "candidate_id": candidate_id,
                "conflict_paths": ["<project-root>"], "reason": "candidate_base_drift"}
    changed = {path for path in set(current) | set(staged)
               if candidates._comparable(current.get(path)) != candidates._comparable(staged.get(path))}
    lock_owner = workspace_isolation.SerializedSharedRootWorkspaceProvisioner(project_root)
    with lock_owner.writer_lock(timeout=30):
        # Recheck under the writer lock immediately before mutation.
        if candidates._digest_manifest(candidates._manifest(project_root)) != expected_base_digest:
            return {"status": "conflict", "candidate_id": candidate_id,
                    "conflict_paths": ["<project-root>"], "reason": "candidate_base_drift"}
        journal_key = _journal_key(assignment_ref, request_key)
        applied = []
        # Create directories before their children and remove children before
        # their parent directories.
        ordered = sorted(changed, key=lambda path: (
            staged.get(path) is None,
            -len(Path(path).parts) if staged.get(path) is None else len(Path(path).parts),
            path,
        ))

        # Host buffers are an external source of truth.  Probe every affected
        # path while the Cortex writer lock is held, before preparing or
        # writing even the first journal entry.  Checking inside the write
        # loop could leave an earlier path applied when a later path reports
        # an unsaved buffer; returning ``conflict`` must therefore be a
        # genuinely non-mutating outcome.  The lock cannot prevent an editor
        # changing its buffer after this preflight, so such a later race is
        # still recorded as an external/uncertain condition by the caller's
        # host integration contract rather than being silently overwritten.
        preflight_conflicts = {}
        before_by_path = {}
        if host_fs_checker is not None:
            for relpath in ordered:
                before = _read_current_entry(project_root, relpath)
                before_by_path[relpath] = before
                try:
                    host = host_fs_checker(relpath)
                except Exception as error:
                    raise StoreError("candidate_host_buffer_check_failed") from error
                if host is not None and (before is None or before.get("bytes") != host):
                    preflight_conflicts[relpath] = {"reason": "unsaved_host_buffer"}
            if preflight_conflicts:
                return {"status": "conflict", "candidate_id": candidate_id,
                        "conflict_paths": sorted(preflight_conflicts),
                        "reason": "unsaved_host_buffer"}
        for relpath in ordered:
            before = before_by_path.get(relpath)
            if before is None and relpath not in before_by_path:
                before = _read_current_entry(project_root, relpath)
            desired = staged.get(relpath)
            _journal_prepare(store, journal_key, project_root, relpath, before, desired)
            full = os.path.join(project_root, relpath)
            if desired is None:
                _delete(project_root, relpath)
            elif desired["kind"] == "f":
                data = _read_bytes(str(root), relpath)
                _write_atomic(project_root, relpath, data, desired.get("mode", 0o600))
            elif desired["kind"] == "d":
                os.makedirs(full, mode=desired.get("mode", 0o700), exist_ok=True)
            elif desired["kind"] == "l":
                _delete(project_root, relpath)
                os.makedirs(os.path.dirname(full), exist_ok=True)
                os.symlink(desired["target"], full)
            from .. import _fault_injection
            _fault_injection.checkpoint("integration_after_write_before_journal_mark")
            applied.append(relpath)
        # Verify the complete staged tree before converting prepared journal
        # entries into applied entries. A crash or external edit during the
        # loop therefore remains recoverable on retry.
        if candidates._digest_manifest(candidates._manifest(project_root)) != candidate.get("content_digest"):
            raise StoreError("candidate_apply_verification_failed")
        for relpath in applied:
            _journal_applied(store, journal_key, relpath)
        integrated = delegation_revision.capture_revision(project_root)
    result = _record(store, task_id=task_id, assignment_ref=assignment_ref,
                     request_key=request_key, attempt_id=candidate_id,
                     workspace_binding_id=None, status="applied" if applied else "noop",
                     applied_paths=applied, conflict_paths={}, integrated=integrated,
                     host_buffer_state="checked" if host_fs_checker else "unknown")
    with store.connection() as db:
        db.execute("DELETE FROM workspace_integration_journal WHERE journal_key=?", (journal_key,))
    return result


def _rescan_workspace(root: str) -> dict:
    """Re-derive a manifest for the *current* state of an already-copied
    workspace (never re-copies -- it is already private storage the worker
    wrote into directly)."""
    manifest: dict = {}
    _scan(root, root, "", manifest, top=True)
    return manifest


def _scan(project_root, root, current, manifest, *, top) -> None:
    directory = os.path.join(root, current) if current else root
    for entry in os.scandir(directory):
        relpath = f"{current}/{entry.name}" if current else entry.name
        if top and entry.name in _EXCLUDED_TOP_LEVEL:
            continue
        if entry.is_symlink():
            target = os.readlink(entry.path)
            mode = stat.S_IMODE(entry.stat(follow_symlinks=False).st_mode)
            manifest[relpath] = dict(kind="l", target=target, mode=mode)
            continue
        info = entry.stat(follow_symlinks=False)
        if stat.S_ISDIR(info.st_mode):
            manifest[relpath] = dict(kind="d", mode=stat.S_IMODE(info.st_mode))
            _scan(project_root, root, relpath, manifest, top=False)
        elif stat.S_ISREG(info.st_mode):
            import hashlib
            fd = os.open(entry.path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
            try:
                h = hashlib.sha256()
                while True:
                    chunk = os.read(fd, 1024 * 1024)
                    if not chunk:
                        break
                    h.update(chunk)
            finally:
                os.close(fd)
            manifest[relpath] = dict(kind="f", sha256=h.hexdigest(), mode=stat.S_IMODE(info.st_mode))
        else:
            manifest[relpath] = dict(kind="o", mode=stat.S_IMODE(info.st_mode))
