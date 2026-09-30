"""Catalog persistence: user-scoped snapshots and project-scoped route history.

Two independent stores, matching the packet-wide split between user-scoped
credential/connection state and project-scoped ``.cortex`` task metadata:

- **Snapshot cache** (this module's ``save_snapshot``/``load_connection_snapshot``):
  one JSON file per ``(connection_id, snapshot_id)`` under
  ``$XDG_STATE_HOME/cortex/catalog/<connection_id>/<snapshot_id>.json``,
  written with the same private-directory/atomic-write primitives as
  ``connections.registry`` (never reimplemented here). Refreshing the catalog
  only ever adds a new snapshot file; it never rewrites or deletes an
  existing one.
- **Route selection history** (``CATALOG_SCHEMA``'s ``task_route_selections``
  table, composed into ``store.py``'s per-project SQLite database): an
  append-only sequence per task, guarded by triggers that reject UPDATE and
  reject DELETE outside the existing retention cleanup path (see
  ``cleanup.finish_deletions``). A catalog refresh never mutates a saved row;
  ``is_stale`` tells a caller a previously selected route no longer appears
  in the latest snapshot without ever silently replacing it.
"""
from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timezone
import json
from pathlib import Path

from ..connections import registry
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import ExecutionRoute, _validate_prefixed_id
from .evidence import EvidenceState, Fact, Provenance, ProvenanceSource
from .model import CatalogEntry, CatalogSnapshot

CATALOG_SCHEMA = '''
CREATE TABLE IF NOT EXISTS task_route_selections (
 task_id TEXT NOT NULL REFERENCES tasks(id), sequence INTEGER NOT NULL,
 route_id TEXT NOT NULL, label_at_selection TEXT NOT NULL,
 supported_parameters TEXT NOT NULL, catalog_snapshot_id TEXT NOT NULL,
 evidence TEXT NOT NULL, selected_at TEXT NOT NULL,
 PRIMARY KEY(task_id,sequence)
);
CREATE INDEX IF NOT EXISTS task_route_selections_by_task ON task_route_selections(task_id,sequence);
CREATE TRIGGER IF NOT EXISTS task_route_selections_no_update BEFORE UPDATE ON task_route_selections
BEGIN SELECT RAISE(ABORT, 'immutable'); END;
CREATE TRIGGER IF NOT EXISTS task_route_selections_no_delete BEFORE DELETE ON task_route_selections
WHEN NOT EXISTS (
 SELECT 1 FROM pending_deletions d WHERE d.task_id=OLD.task_id
) BEGIN SELECT RAISE(ABORT, 'immutable'); END;
'''

CATALOG_TABLES = frozenset({"task_route_selections"})


def _now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="microseconds")


def _fail_closed(code: str, remediation: str, *, field: str | None = None) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION,
        code=code,
        remediation=remediation,
        retry_eligible=False,
        field=field,
    )


# --- snapshot cache (user-scoped, filesystem) -------------------------------

def _snapshot_dir(connection_id: str, environ: dict[str, str] | None = None) -> Path:
    _validate_prefixed_id(connection_id, "cn_", "connection_id")
    return registry.state_root(environ) / "catalog" / connection_id


def _snapshot_path(connection_id: str, snapshot_id: str, environ: dict[str, str] | None = None) -> Path:
    _validate_prefixed_id(snapshot_id, "cs_", "snapshot_id")
    return _snapshot_dir(connection_id, environ) / f"{snapshot_id}.json"


def _serialize_evidence(evidence: EvidenceState) -> dict[str, object]:
    return {
        fact.value: {
            "source": provenance.source.value,
            "adapter_version": provenance.adapter_version,
            "observed_at": provenance.observed_at,
            "evidence_ref": provenance.evidence_ref,
        }
        for fact, provenance in evidence.facts.items()
    }


def _deserialize_evidence(data: object) -> EvidenceState:
    if not isinstance(data, dict):
        raise _fail_closed("invalid_catalog_snapshot", "'evidence' must be an object.")
    state = EvidenceState.empty()
    for fact_name, payload in data.items():
        try:
            fact = Fact(fact_name)
            provenance = Provenance(
                source=ProvenanceSource(payload["source"]),
                adapter_version=payload["adapter_version"],
                observed_at=payload["observed_at"],
                evidence_ref=payload.get("evidence_ref"),
            )
        except (ValueError, KeyError, TypeError):
            raise _fail_closed("invalid_catalog_snapshot", "an evidence entry has an unexpected shape.") from None
        state = state.promote(fact, provenance)
    return state


def _serialize_entry(entry: CatalogEntry) -> dict[str, object]:
    return {
        "route": {
            "id": entry.route.id,
            "engine": entry.route.engine,
            "connection_id": entry.route.connection_id,
            "model": entry.route.model,
            "supported_parameters": [list(pair) for pair in entry.route.supported_parameters],
        },
        "connection_id": entry.connection_id,
        "label": entry.label,
        "disambiguated_label": entry.disambiguated_label,
        "evidence": _serialize_evidence(entry.evidence),
        "catalog_snapshot_id": entry.catalog_snapshot_id,
        "model_name": entry.model_name,
        "model_description": entry.model_description,
        # T024: optional -- absent (None) for every entry with no evidenced
        # route context-window; a cached snapshot written before this field
        # existed simply has no key here, which deserializes back to None.
        "context_window_tokens": entry.context_window_tokens,
    }


def _deserialize_entry(data: object) -> CatalogEntry:
    if not isinstance(data, dict):
        raise _fail_closed("invalid_catalog_snapshot", "a catalog entry must be an object.")
    try:
        route_data = data["route"]
        route = ExecutionRoute(
            engine=route_data["engine"],
            connection_id=route_data["connection_id"],
            model=route_data["model"],
            supported_parameters=tuple(tuple(pair) for pair in route_data["supported_parameters"]),
        )
    except (KeyError, TypeError):
        raise _fail_closed("invalid_catalog_snapshot", "a catalog entry route has an unexpected shape.") from None
    if route.id != route_data.get("id"):
        raise _fail_closed("invalid_catalog_snapshot", "a catalog entry route id does not match its recomputed fingerprint.")
    return CatalogEntry(
        route=route,
        connection_id=data["connection_id"],
        label=data["label"],
        disambiguated_label=data["disambiguated_label"],
        evidence=_deserialize_evidence(data["evidence"]),
        catalog_snapshot_id=data["catalog_snapshot_id"],
        model_name=data.get("model_name"),
        model_description=data.get("model_description"),
        context_window_tokens=data.get("context_window_tokens"),
    )


def save_snapshot(snapshot: CatalogSnapshot, environ: dict[str, str] | None = None) -> None:
    """Persist ``snapshot`` under every connection id it touches.

    Never rewrites an existing snapshot file (its name is content-addressed
    by ``snapshot.id``, which callers must allocate freshly per refresh via
    ``discovery.build_catalog``).
    """
    by_connection: dict[str, list[CatalogEntry]] = {}
    for entry in snapshot.entries:
        by_connection.setdefault(entry.connection_id, []).append(entry)
    connection_ids = set(by_connection) | set(snapshot.unavailable)
    for connection_id in connection_ids:
        payload = {
            "snapshot_id": snapshot.id,
            "created_at": snapshot.created_at,
            "connection_registry_fingerprint": snapshot.connection_registry_fingerprint,
            "setup_action": snapshot.setup_action,
            "unavailable_reason": snapshot.unavailable.get(connection_id),
            "entries": [_serialize_entry(entry) for entry in by_connection.get(connection_id, [])],
        }
        directory = _snapshot_dir(connection_id, environ)
        registry.ensure_private_dir(directory, root=registry.state_root(environ), environ=environ)
        path = _snapshot_path(connection_id, snapshot.id, environ)
        body = json.dumps(payload, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
        registry.atomic_write_private(path, body, directory=directory)


@dataclass(frozen=True)
class ConnectionSnapshotView:
    """The subset of one :class:`CatalogSnapshot` scoped to a connection."""

    connection_id: str
    snapshot_id: str
    created_at: str
    entries: tuple[CatalogEntry, ...]
    unavailable_reason: str | None


def load_connection_snapshot(
    connection_id: str, snapshot_id: str, environ: dict[str, str] | None = None
) -> ConnectionSnapshotView | None:
    path = _snapshot_path(connection_id, snapshot_id, environ)
    data = registry.read_owner_file(path)
    if data is None:
        return None
    try:
        raw = json.loads(data.decode("utf-8"))
    except (UnicodeDecodeError, ValueError):
        raise _fail_closed("invalid_catalog_snapshot", "the cached catalog snapshot is not valid UTF-8 JSON.") from None
    if not isinstance(raw, dict):
        raise _fail_closed("invalid_catalog_snapshot", "the cached catalog snapshot must be a JSON object.")
    entries = tuple(_deserialize_entry(item) for item in raw.get("entries", []))
    return ConnectionSnapshotView(
        connection_id=connection_id,
        snapshot_id=raw["snapshot_id"],
        created_at=raw["created_at"],
        entries=entries,
        unavailable_reason=raw.get("unavailable_reason"),
    )


def latest_connection_entries(
    connection_id: str, environ: dict[str, str] | None = None
) -> tuple[CatalogEntry, ...] | None:
    """:func:`latest_connection_snapshot`'s entries, or ``None``."""
    view = latest_connection_snapshot(connection_id, environ)
    return None if view is None else view.entries


def latest_connection_snapshot(
    connection_id: str, environ: dict[str, str] | None = None
) -> ConnectionSnapshotView | None:
    """This connection's most recently cached *genuine* snapshot, or
    ``None`` when it has never been observed. Skips a placeholder file (no
    entries, an ``unavailable_reason``) written into this connection's own
    directory by an unrelated ``build_catalog`` call for a sibling
    connection (see ``acp.config_options._is_cross_connection_placeholder``,
    the identical rule applied here for the same reason: a newer placeholder
    must never hide an older real observation). Used to resolve a route's
    real, engine-observed model display name after the fact (e.g. for a
    worker's route audit), never to decide selectability -- that stays
    ``acp.config_options.selectable_entries_from_snapshots``'s job.
    """
    directory = _snapshot_dir(connection_id, environ)
    if not directory.is_dir():
        return None
    files = sorted(
        (path for path in directory.glob("cs_*.json") if path.is_file()),
        key=lambda path: path.stat().st_mtime,
        reverse=True,
    )
    for path in files:
        view = load_connection_snapshot(connection_id, path.stem, environ)
        if view is None:
            continue
        if not view.entries and view.unavailable_reason is not None:
            continue
        return view
    return None


# --- route selection history (project-scoped, SQLite) -----------------------

def record_task_route(store, task_id: str, entry: CatalogEntry) -> dict[str, object]:
    """Append one immutable route selection row for ``task_id``.

    Never mutates or removes a prior row; a catalog refresh must call this
    again with a fresh entry rather than editing history in place.
    """
    supported_parameters = json.dumps(
        [list(pair) for pair in entry.route.supported_parameters], sort_keys=True, ensure_ascii=False, separators=(",", ":")
    )
    evidence = json.dumps(_serialize_evidence(entry.evidence), sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    selected_at = _now()
    with store.connection() as db:
        db.execute("BEGIN IMMEDIATE")
        try:
            if db.execute("SELECT 1 FROM tasks WHERE id=?", (task_id,)).fetchone() is None:
                raise _fail_closed("unknown_task", "task_id must reference an existing task.", field="task_id")
            row = db.execute(
                "SELECT COALESCE(MAX(sequence),0) FROM task_route_selections WHERE task_id=?", (task_id,)
            ).fetchone()
            sequence = row[0] + 1
            db.execute(
                "INSERT INTO task_route_selections VALUES (?,?,?,?,?,?,?,?)",
                (
                    task_id,
                    sequence,
                    entry.route.id,
                    entry.disambiguated_label,
                    supported_parameters,
                    entry.catalog_snapshot_id,
                    evidence,
                    selected_at,
                ),
            )
            db.commit()
        except BaseException:
            db.rollback()
            raise
    return dict(
        task_id=task_id,
        sequence=sequence,
        route_id=entry.route.id,
        label_at_selection=entry.disambiguated_label,
        catalog_snapshot_id=entry.catalog_snapshot_id,
        selected_at=selected_at,
    )


def current_task_route(store, task_id: str) -> dict[str, object] | None:
    """The most recently recorded route selection for ``task_id``, or ``None``."""
    with store.connection() as db:
        row = db.execute(
            "SELECT * FROM task_route_selections WHERE task_id=? ORDER BY sequence DESC LIMIT 1", (task_id,)
        ).fetchone()
    if row is None:
        return None
    return dict(
        task_id=row["task_id"],
        sequence=row["sequence"],
        route_id=row["route_id"],
        label_at_selection=row["label_at_selection"],
        supported_parameters=tuple(tuple(pair) for pair in json.loads(row["supported_parameters"])),
        catalog_snapshot_id=row["catalog_snapshot_id"],
        evidence=json.loads(row["evidence"]),
        selected_at=row["selected_at"],
    )


def task_route_history(store, task_id: str) -> tuple[dict[str, object], ...]:
    """Every recorded route selection for ``task_id``, oldest first."""
    with store.connection() as db:
        rows = db.execute(
            "SELECT * FROM task_route_selections WHERE task_id=? ORDER BY sequence ASC", (task_id,)
        ).fetchall()
    return tuple(
        dict(
            task_id=row["task_id"],
            sequence=row["sequence"],
            route_id=row["route_id"],
            label_at_selection=row["label_at_selection"],
            supported_parameters=tuple(tuple(pair) for pair in json.loads(row["supported_parameters"])),
            catalog_snapshot_id=row["catalog_snapshot_id"],
            evidence=json.loads(row["evidence"]),
            selected_at=row["selected_at"],
        )
        for row in rows
    )


def find_route_identity(
    connection_ids, route_id: str, environ: dict[str, str] | None = None
) -> tuple[str, str] | None:
    """Best-effort reverse lookup of a stale route id's ``(connection_id,
    model)`` identity by scanning every still-cached snapshot file for each
    of ``connection_ids`` (never the live/current snapshot only -- a
    snapshot file is never rewritten or deleted, see :func:`save_snapshot`,
    so a route id that ever existed is still found here even after a later
    refresh minted a new id for the same model).

    Used only as a one-time migration path for a ``defaults.json`` written
    before this identity (``route_connection_id``/``route_model``) was
    stored alongside ``route_id`` -- a fresh write always carries both, and
    a caller with them never needs this scan (see
    ``configuration.resolution.resolve_saved_route``). Returns ``None`` if
    no cached snapshot for any of ``connection_ids`` ever contained this
    route id.
    """
    for connection_id in connection_ids:
        try:
            paths = sorted(_snapshot_dir(connection_id, environ).glob("cs_*.json"))
        except OSError:
            continue
        for path in paths:
            data = registry.read_owner_file(path)
            if data is None:
                continue
            try:
                raw = json.loads(data.decode("utf-8"))
            except (UnicodeDecodeError, ValueError):
                continue
            for item in raw.get("entries", []) if isinstance(raw, dict) else []:
                route = item.get("route") if isinstance(item, dict) else None
                if isinstance(route, dict) and route.get("id") == route_id:
                    model = route.get("model")
                    if isinstance(model, str) and model:
                        return connection_id, model
    return None


def is_stale(route_id: str, snapshot: CatalogSnapshot) -> bool:
    """True when ``route_id`` no longer appears anywhere in ``snapshot``.

    Refreshing the catalog never rewrites the saved task route; callers use
    this to flag staleness in a read path without touching history.
    """
    return snapshot.entry_for_route(route_id) is None
