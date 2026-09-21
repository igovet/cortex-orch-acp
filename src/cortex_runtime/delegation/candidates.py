"""Durable, isolated candidate staging for worker workspace results.

The candidate is a materialized copy of the project plus the exact changes
from one or more isolated worker attempts.  It is deliberately independent
of integration: callers can inspect and verify the candidate before anything
is written to the user's project.
"""
from __future__ import annotations

from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import uuid

from ..contracts import StoreError
from ..project_storage import StorageLocator, canonical_project
from .workspace_isolation import capture_and_copy_workspace


CANDIDATE_SCHEMA = """
CREATE TABLE IF NOT EXISTS candidate_staging (
 candidate_id TEXT PRIMARY KEY,
 task_id TEXT NOT NULL REFERENCES tasks(id),
 project_root TEXT NOT NULL,
 stage_path TEXT NOT NULL,
 attempt_ids_json TEXT NOT NULL,
 source_bindings_json TEXT NOT NULL,
 base_identity_json TEXT NOT NULL,
 lineage_candidate_id TEXT,
 project_base_manifest_json TEXT NOT NULL DEFAULT '{}',
 project_base_digest TEXT,
 content_digest TEXT,
 status TEXT NOT NULL,
 conflict_paths_json TEXT NOT NULL DEFAULT '[]',
 error TEXT,
 created_at TEXT NOT NULL,
 cleaned_up_at TEXT
);
CREATE INDEX IF NOT EXISTS candidate_staging_by_task
 ON candidate_staging(task_id, created_at);
"""

_EXCLUDED = {".git", ".cortex"}
_ID = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$")


class CandidateError(StoreError):
    """A candidate could not be prepared safely."""


class CandidateConflict(CandidateError):
    """Two selected attempts changed the same path differently."""

    def __init__(self, candidate: dict):
        self.candidate = candidate
        paths = ", ".join(candidate.get("conflict_paths", []))
        super().__init__(f"candidate_conflict:{paths}")


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _json(value) -> str:
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def _ensure_schema(store) -> None:
    with store.connection() as db:
        db.executescript(CANDIDATE_SCHEMA)
        columns = {row["name"] for row in db.execute("PRAGMA table_info(candidate_staging)")}
        if "project_base_digest" not in columns:
            db.execute("ALTER TABLE candidate_staging ADD COLUMN project_base_digest TEXT")
        if "lineage_candidate_id" not in columns:
            db.execute("ALTER TABLE candidate_staging ADD COLUMN lineage_candidate_id TEXT")
        if "project_base_manifest_json" not in columns:
            db.execute("ALTER TABLE candidate_staging ADD COLUMN project_base_manifest_json TEXT NOT NULL DEFAULT '{}'")


def _safe_rel(path: str) -> str:
    value = Path(path)
    if value.is_absolute() or ".." in value.parts or not path or path == ".":
        raise CandidateError("unsafe_candidate_path")
    if value.parts[0] in _EXCLUDED:
        raise CandidateError("excluded_candidate_path")
    return value.as_posix()


def _safe_id(value: str) -> str:
    if not isinstance(value, str) or not _ID.fullmatch(value):
        raise CandidateError("unsafe_candidate_id")
    return value


def _comparable(entry: dict | None) -> dict | None:
    """Normalize workspace and candidate manifests to the same identity."""
    if entry is None:
        return None
    kind = entry.get("kind")
    if kind == "f":
        return {"kind": "f", "sha256": entry.get("sha256"), "mode": entry.get("mode")}
    if kind == "d":
        return {"kind": "d", "mode": entry.get("mode")}
    if kind == "l":
        return {"kind": "l", "target": entry.get("target"), "mode": entry.get("mode")}
    return {key: entry[key] for key in sorted(entry)}


def _manifest(root: str) -> dict:
    """Read a bounded, symlink-averse manifest of a worker root."""
    result = {}

    def walk(directory: Path, prefix: str = ""):
        try:
            entries = sorted(os.scandir(directory), key=lambda item: item.name)
        except OSError as error:
            raise CandidateError(f"attempt_workspace_unreadable:{error}") from error
        for entry in entries:
            if not prefix and entry.name in _EXCLUDED:
                continue
            rel = _safe_rel(f"{prefix}/{entry.name}" if prefix else entry.name)
            try:
                info = entry.stat(follow_symlinks=False)
            except OSError as error:
                raise CandidateError(f"attempt_workspace_unreadable:{error}") from error
            mode = stat.S_IMODE(info.st_mode)
            if stat.S_ISDIR(info.st_mode):
                result[rel] = {"kind": "d", "mode": mode}
                walk(Path(entry.path), rel)
            elif stat.S_ISREG(info.st_mode):
                h = hashlib.sha256()
                try:
                    with open(entry.path, "rb") as stream:
                        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                            h.update(chunk)
                except OSError as error:
                    raise CandidateError(f"attempt_workspace_unreadable:{error}") from error
                result[rel] = {"kind": "f", "sha256": h.hexdigest(), "mode": mode}
            elif stat.S_ISLNK(info.st_mode):
                try:
                    result[rel] = {"kind": "l", "target": os.readlink(entry.path), "mode": mode}
                except OSError as error:
                    raise CandidateError(f"attempt_workspace_unreadable:{error}") from error
            else:
                raise CandidateError(f"unsupported_attempt_entry:{rel}")

    walk(Path(root))
    return result


def _changes(base: dict, current: dict) -> dict:
    paths = set(base) | set(current)
    return {path: current.get(path) for path in sorted(paths)
            if _comparable(base.get(path)) != _comparable(current.get(path))}


def _source_bindings(rows) -> list[dict]:
    return [
        {
            "workspace_binding_id": row["workspace_binding_id"],
            "attempt_id": row["attempt_id"],
            "mode": row["mode"],
            "base_kind": row["base_kind"],
            "base_head": row["base_head"],
            "base_dirty_digest": row["base_dirty_digest"],
        }
        for row in rows
    ]


def _ensure_parent(stage: Path, rel: str) -> Path:
    current = stage
    parts = Path(rel).parts
    for part in parts[:-1]:
        current /= part
        try:
            info = current.lstat()
        except FileNotFoundError:
            current.mkdir(mode=0o700)
            info = current.lstat()
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise CandidateError(f"candidate_parent_not_directory:{rel}")
    return stage / rel


def _source_path(root: Path, rel: str) -> Path:
    """Resolve a workspace path without traversing a symlinked ancestor."""
    current = root
    for part in Path(rel).parts[:-1]:
        current /= part
        try:
            info = current.lstat()
        except OSError as error:
            raise CandidateError(f"attempt_workspace_unreadable:{error}") from error
        if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
            raise CandidateError(f"unsafe_attempt_path:{rel}")
    return root / rel


def _remove_path(path: Path) -> None:
    try:
        info = path.lstat()
    except FileNotFoundError:
        return
    if stat.S_ISDIR(info.st_mode) and not stat.S_ISLNK(info.st_mode):
        shutil.rmtree(path)
    else:
        path.unlink()


def _apply_change(stage: Path, source_root: Path, rel: str, entry: dict | None) -> None:
    destination = _ensure_parent(stage, rel)
    if entry is None:
        _remove_path(destination)
        return
    source = _source_path(source_root, rel)
    kind = entry["kind"]
    if kind == "d":
        try:
            existing = destination.lstat()
        except FileNotFoundError:
            existing = None
        if existing is not None and (stat.S_ISLNK(existing.st_mode) or not stat.S_ISDIR(existing.st_mode)):
            _remove_path(destination)
            existing = None
        if existing is None:
            destination.mkdir(mode=entry.get("mode", 0o700))
        os.chmod(destination, entry.get("mode", 0o700))
    elif kind == "l":
        _remove_path(destination)
        destination.symlink_to(entry["target"])
    elif kind == "f":
        _remove_path(destination)
        temporary = destination.with_name(f".{destination.name}.cortex-{uuid.uuid4().hex}.tmp")
        try:
            flags = os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0)
            source_fd = os.open(source, flags)
            try:
                source_info = os.fstat(source_fd)
                if not stat.S_ISREG(source_info.st_mode):
                    raise CandidateError(f"unsafe_attempt_path:{rel}")
                with os.fdopen(source_fd, "rb") as src, open(temporary, "xb") as dst:
                    digest = hashlib.sha256()
                    while True:
                        chunk = src.read(1024 * 1024)
                        if not chunk:
                            break
                        digest.update(chunk)
                        dst.write(chunk)
                    if digest.hexdigest() != entry.get("sha256"):
                        raise CandidateError(f"attempt_workspace_changed:{rel}")
                    dst.flush()
                    os.fsync(dst.fileno())
            except BaseException:
                # fdopen owns the descriptor after successful construction;
                # close the raw descriptor for all failures before that point.
                try:
                    os.close(source_fd)
                except OSError:
                    pass
                raise
            os.chmod(temporary, entry.get("mode", 0o600))
            os.replace(temporary, destination)
        finally:
            try:
                temporary.unlink()
            except FileNotFoundError:
                pass
    else:
        raise CandidateError(f"unsupported_candidate_kind:{kind}")


def _digest_manifest(manifest: dict) -> str:
    return hashlib.sha256(_json(manifest).encode()).hexdigest()


def _row_dict(row) -> dict:
    if row is None:
        return None
    result = dict(row)
    for source, target in (
        ("attempt_ids_json", "attempt_ids"),
        ("source_bindings_json", "source_bindings"),
        ("base_identity_json", "base_identity"),
        ("conflict_paths_json", "conflict_paths"),
    ):
        result[target] = json.loads(result.pop(source))
    return result


def _row_seed_candidate(row) -> str | None:
    """Read the optional lineage binding added by workspace provisioning."""
    try:
        keys = row.keys()
    except AttributeError:
        return None
    return row["seed_candidate_id"] if "seed_candidate_id" in keys else None


def _attempt_rows(store, task_id: str, attempt_ids: list[str]):
    """Load workspace bindings and optional workspace-safety lineage."""
    placeholders = ",".join("?" for _ in attempt_ids)
    with store.connection() as db:
        lineage_table = db.execute(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='workspace_snapshot_lineage'"
        ).fetchone()
        if lineage_table is not None:
            return db.execute(
                f"SELECT w.*, l.candidate_id AS seed_candidate_id "
                f"FROM workspace_snapshots w LEFT JOIN workspace_snapshot_lineage l "
                f"ON l.workspace_binding_id=w.workspace_binding_id "
                f"WHERE w.task_id=? AND w.attempt_id IN ({placeholders}) "
                "AND w.cleaned_up_at IS NULL ORDER BY w.created_at",
                (task_id, *attempt_ids),
            ).fetchall()
        return db.execute(
            f"SELECT * FROM workspace_snapshots WHERE task_id=? AND attempt_id IN ({placeholders}) "
            "AND cleaned_up_at IS NULL ORDER BY created_at", (task_id, *attempt_ids)
        ).fetchall()


def get_candidate(store, candidate_id: str) -> dict | None:
    _ensure_schema(store)
    with store.connection() as db:
        return _row_dict(db.execute(
            "SELECT * FROM candidate_staging WHERE candidate_id=?", (candidate_id,)
        ).fetchone())


def prepare_candidate(store, task_id: str, attempt_ids, *, candidate_id: str | None = None) -> dict:
    """Materialize selected attempt changes on an isolated candidate copy.

    The main project is read only.  A conflict or any later staging error is
    recorded and the candidate directory is deliberately retained for
    inspection or cleanup by the caller.
    """
    _ensure_schema(store)
    attempt_ids = list(dict.fromkeys(str(value) for value in attempt_ids))
    if not attempt_ids:
        raise CandidateError("candidate_attempts_required")
    project_root = canonical_project(store.project_root)
    candidate_id = _safe_id(candidate_id or f"cand_{uuid.uuid4().hex}")
    store_dir = StorageLocator(project_root).store_dir()
    candidate_root = store_dir / "candidates" / candidate_id
    stage = candidate_root / "stage"
    created = _now()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        existing = db.execute("SELECT * FROM candidate_staging WHERE candidate_id=?", (candidate_id,)).fetchone()
        if existing is not None:
            prior = _row_dict(existing)
            if (prior["task_id"] != task_id or prior["attempt_ids"] != attempt_ids
                    or prior["project_root"] != project_root):
                db.rollback()
                raise CandidateError("candidate_id_reused")
            db.commit()
            if prior["status"] == "conflict":
                raise CandidateConflict(prior)
            if prior["status"] == "ready" and Path(prior["stage_path"]).is_dir():
                return prior
            raise CandidateError(f"candidate_already_exists:{prior['status']}")
        task = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
        if task is None or task["project_root"] != project_root:
            db.rollback()
            raise CandidateError("candidate_task_mismatch")
        db.execute(
            "INSERT INTO candidate_staging(candidate_id,task_id,project_root,stage_path,attempt_ids_json,source_bindings_json,base_identity_json,lineage_candidate_id,project_base_manifest_json,project_base_digest,status,created_at) "
            "VALUES(?,?,?,?,?,?,?,?,?,?,?,?)",
            (candidate_id, task_id, project_root, str(stage), _json(attempt_ids), "[]", "[]", None, "{}", None, "preparing", created),
        )
        db.commit()
    try:
        rows = _attempt_rows(store, task_id, attempt_ids)
        by_attempt = {row["attempt_id"]: row for row in rows}
        if len(by_attempt) != len(attempt_ids):
            missing = sorted(set(attempt_ids) - set(by_attempt))
            # DirectProjectWorkspaceProvisioner deliberately does not create
            # a private workspace_snapshots row.  Reconcile such attempts by
            # their durable provisioning binding and snapshot the exact
            # project root below; an absent/ambiguous binding still fails
            # closed.
            with store.connection() as db:
                placeholders = ",".join("?" for _ in attempt_ids)
                direct = db.execute(
                    f"SELECT attempt_id,workspace_binding_id FROM delegation_provisioning "
                    f"WHERE task_id=? AND attempt_id IN ({placeholders})",
                    (task_id, *attempt_ids),
                ).fetchall()
            if (len(direct) == len(attempt_ids)
                    and all(row["workspace_binding_id"] in {
                        "shared_project_root", "serialized_shared_root"
                    } for row in direct)):
                rows = [dict(
                    attempt_id=row["attempt_id"], workspace_binding_id=row["workspace_binding_id"],
                    mode=row["workspace_binding_id"], root_path=project_root,
                    project_root=project_root, base_path=None, manifest_json="{}",
                    base_kind=None, base_head=None, base_dirty_digest=None,
                ) for row in direct]
                by_attempt = {row["attempt_id"]: row for row in rows}
            else:
                raise CandidateError(f"candidate_attempt_missing:{','.join(missing)}")
        direct_rows = [row for row in rows if row["mode"] in {
            "shared_project_root", "serialized_shared_root"
        }]
        if direct_rows:
            # Direct ACP attempts already materialize their result in the
            # project root opened by Zed.  Build a candidate as an immutable
            # evidence snapshot of that exact root; never copy a stale
            # private workspace over it and never pretend that a second
            # integration is required.  Mixing direct and isolated attempts
            # is ambiguous and therefore refused.
            if len(direct_rows) != len(rows):
                raise CandidateError("candidate_mixed_workspace_modes")
            if any(row["project_root"] != project_root or row["root_path"] != project_root
                   for row in rows):
                raise CandidateError("candidate_project_root_mismatch")
            candidate_root.mkdir(mode=0o700, parents=True, exist_ok=False)
            project_before = _manifest(project_root)
            capture_and_copy_workspace(project_root, str(stage))
            if _manifest(project_root) != project_before:
                raise CandidateError("project_changed_during_candidate_capture")
            source_bindings = _source_bindings(rows)
            base_identity = [{
                "attempt_id": row["attempt_id"], "base_kind": row["base_kind"],
                "base_head": row["base_head"], "base_dirty_digest": row["base_dirty_digest"],
            } for row in rows]
            digest = _digest_manifest(project_before)
            with store.connection() as db:
                db.execute(
                    "UPDATE candidate_staging SET source_bindings_json=?,base_identity_json=?,"
                    "project_base_manifest_json=?,project_base_digest=?,content_digest=?,"
                    "status=?,conflict_paths_json=? WHERE candidate_id=?",
                    (_json(source_bindings), _json(base_identity), _json(project_before),
                     digest, _digest_manifest(_manifest(str(stage))), "ready", "[]", candidate_id),
                )
            return get_candidate(store, candidate_id)
        if any(row["mode"] != "isolated_snapshot" or not row["root_path"] for row in rows):
            raise CandidateError("candidate_requires_isolated_attempts")
        seed_ids = {_row_seed_candidate(row) for row in rows if _row_seed_candidate(row)}
        if len(seed_ids) > 1:
            raise CandidateError("candidate_multiple_lineages")
        lineage_candidate_id = next(iter(seed_ids), None)
        lineage = get_candidate(store, lineage_candidate_id) if lineage_candidate_id else None
        if lineage_candidate_id and (lineage is None or lineage["task_id"] != task_id
                                     or lineage["status"] != "ready"
                                     or lineage.get("cleaned_up_at")):
            raise CandidateError("candidate_seed_not_ready")
        candidate_root.mkdir(mode=0o700, parents=True, exist_ok=False)
        # Bind the candidate to one observed main-project version.  If the
        # project changes while the copy is being made, do not silently stage
        # a mixture of two versions.
        project_before = _manifest(project_root)
        if lineage is not None:
            lineage_stage = Path(lineage["stage_path"])
            if not lineage_stage.is_dir():
                raise CandidateError("candidate_seed_stage_missing")
            lineage_stage_manifest = _manifest(str(lineage_stage))
            project_base_manifest = json.loads(lineage.get("project_base_manifest_json") or "{}")
            if lineage.get("project_base_manifest_json") is None or not lineage.get("project_base_digest"):
                raise CandidateError("candidate_seed_base_identity_missing")
            project_base_digest = lineage["project_base_digest"]
            capture_source = str(lineage_stage)
        else:
            project_base_manifest = project_before
            project_base_digest = _digest_manifest(project_before)
            capture_source = project_root
        capture_and_copy_workspace(capture_source, str(stage))
        if _manifest(project_root) != project_before:
            raise CandidateError("project_changed_during_candidate_capture")
        changes = []
        source_manifests = {}
        source_bindings = _source_bindings(rows)
        base_identity = []
        for row in rows:
            root = Path(row["root_path"])
            if not root.is_dir():
                raise CandidateError(f"attempt_workspace_missing:{row['attempt_id']}")
            current = _manifest(str(root))
            source_manifests[row["attempt_id"]] = current
            base = json.loads(row["manifest_json"])
            if lineage is None and not base_identity:
                project_base_manifest = base
            elif lineage is None:
                prior_base = project_base_manifest
                if {path: _comparable(value) for path, value in prior_base.items()} != {
                    path: _comparable(value) for path, value in base.items()
                }:
                    raise CandidateError("candidate_attempt_base_mismatch")
            if lineage is not None and {
                path: _comparable(value) for path, value in json.loads(row["manifest_json"]).items()
            } != {
                path: _comparable(value) for path, value in lineage_stage_manifest.items()
            }:
                raise CandidateError(f"candidate_seed_mismatch:{row['attempt_id']}")
            changes.append((row["attempt_id"], row["root_path"], _changes(base, current)))
            base_identity.append({"attempt_id": row["attempt_id"], "base_kind": row["base_kind"],
                                 "base_head": row["base_head"], "base_dirty_digest": row["base_dirty_digest"]})
        # Three-way guard: a worker result can only be applied to the current
        # project when every touched path (and its parent directories) still
        # agrees with the worker's recorded base.
        main_conflicts = set()
        for _, _, change in changes:
            for path, worker_value in change.items():
                parts = Path(path).parts
                ancestors = [Path(*parts[:index]).as_posix() for index in range(1, len(parts) + 1)]
                for touched in ancestors:
                    base_value = project_base_manifest.get(touched)
                    main_value = project_before.get(touched)
                    if _comparable(main_value) != _comparable(base_value):
                        # An exact path may already contain the same result;
                        # an ancestor change always remains a conflict.
                        if touched != path or _comparable(main_value) != _comparable(worker_value):
                            main_conflicts.add(path)
        if main_conflicts:
            source_bindings = _source_bindings(rows)
            base_identity = [{"attempt_id": row["attempt_id"], "base_kind": row["base_kind"],
                              "base_head": row["base_head"], "base_dirty_digest": row["base_dirty_digest"]} for row in rows]
            result = {"candidate_id": candidate_id, "task_id": task_id, "project_root": project_root,
                      "stage_path": str(stage), "attempt_ids": attempt_ids, "source_bindings": source_bindings,
                      "base_identity": base_identity, "lineage_candidate_id": lineage_candidate_id,
                      "project_base_digest": project_base_digest,
                      "status": "conflict", "conflict_paths": sorted(main_conflicts)}
            with store.connection() as db:
                db.execute("UPDATE candidate_staging SET source_bindings_json=?,base_identity_json=?,lineage_candidate_id=?,project_base_manifest_json=?,project_base_digest=?,status=?,conflict_paths_json=?,error=? WHERE candidate_id=?",
                           (_json(source_bindings), _json(base_identity), lineage_candidate_id, _json(project_base_manifest), project_base_digest, "conflict", _json(sorted(main_conflicts)), "project_changed_since_attempt_base", candidate_id))
            raise CandidateConflict(result)
        path_values = {}
        conflicts = set()
        for attempt_id, _, change in changes:
            for path, value in change.items():
                prior = path_values.get(path)
                if prior is not None and prior[1] != value:
                    conflicts.add(path)
                else:
                    path_values[path] = (attempt_id, value)
        if conflicts:
            result = {"candidate_id": candidate_id, "task_id": task_id, "project_root": project_root,
                      "stage_path": str(stage), "attempt_ids": attempt_ids, "source_bindings": source_bindings,
                      "base_identity": base_identity, "lineage_candidate_id": lineage_candidate_id,
                      "project_base_digest": project_base_digest,
                      "status": "conflict", "conflict_paths": sorted(conflicts)}
            with store.connection() as db:
                db.execute("UPDATE candidate_staging SET source_bindings_json=?,base_identity_json=?,lineage_candidate_id=?,project_base_manifest_json=?,project_base_digest=?,status=?,conflict_paths_json=?,error=? WHERE candidate_id=?",
                           (_json(source_bindings), _json(base_identity), lineage_candidate_id, _json(project_base_manifest), project_base_digest, "conflict", _json(sorted(conflicts)), "overlapping_attempt_changes", candidate_id))
            raise CandidateConflict(result)
        for attempt_id, root, change in changes:
            deletions = sorted(
                ((rel, entry) for rel, entry in change.items() if entry is None),
                key=lambda item: (-len(Path(item[0]).parts), item[0]),
            )
            writes = sorted(
                ((rel, entry) for rel, entry in change.items() if entry is not None),
                key=lambda item: (len(Path(item[0]).parts), item[0]),
            )
            for rel, entry in [*deletions, *writes]:
                _apply_change(stage, Path(root), rel, entry)
        for attempt_id, root, _ in changes:
            if _manifest(root) != source_manifests[attempt_id]:
                raise CandidateError(f"attempt_workspace_changed:{attempt_id}")
        manifest = _manifest(str(stage))
        with store.connection() as db:
            db.execute("UPDATE candidate_staging SET source_bindings_json=?,base_identity_json=?,lineage_candidate_id=?,project_base_manifest_json=?,project_base_digest=?,content_digest=?,status=?,conflict_paths_json=? WHERE candidate_id=?",
                       (_json(source_bindings), _json(base_identity), lineage_candidate_id, _json(project_base_manifest), project_base_digest, _digest_manifest(manifest), "ready", "[]", candidate_id))
        return get_candidate(store, candidate_id)
    except CandidateConflict:
        raise
    except Exception as error:
        with store.connection() as db:
            db.execute("UPDATE candidate_staging SET status=?,error=? WHERE candidate_id=?", ("failed", str(error), candidate_id))
        raise


def cleanup_candidate(store, candidate_id: str, *, reason: str = "cleaned_up") -> dict | None:
    """Explicitly remove a staged candidate and retain its durable record."""
    candidate = get_candidate(store, candidate_id)
    if candidate is None:
        return None
    stage_path = Path(candidate["stage_path"])
    candidate_dir = stage_path.parent
    if candidate_dir.is_dir():
        shutil.rmtree(candidate_dir)
    with store.connection() as db:
        db.execute("UPDATE candidate_staging SET status=?,cleaned_up_at=?,error=? WHERE candidate_id=?",
                   ("cleaned", _now(), reason, candidate_id))
    return get_candidate(store, candidate_id)


__all__ = ["CandidateConflict", "CandidateError", "CANDIDATE_SCHEMA", "cleanup_candidate", "get_candidate", "prepare_candidate"]
