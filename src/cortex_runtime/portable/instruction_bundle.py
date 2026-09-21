"""Deterministic, provenance-aware instruction bundles for ACP sessions.

The runtime may create a bundle before it knows which adapter session will
receive it.  This module keeps those two facts separate: an assembled bundle
describes the intended instructions, while :meth:`InstructionBundle.bind`
records the exact attempt/engine representation that was sent.  Callers can
persist ``to_dict()`` (or its JSON form) and reconstruct the same object after
an orchestrator restart without relying on in-memory state.
"""
from __future__ import annotations

from dataclasses import dataclass, replace
import hashlib
import json
from datetime import datetime, timezone
import sqlite3
from typing import Any, Iterable, Mapping

from .roles import RoleInstructionSet, composed_instructions


def _canonical(value: Any) -> str:
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"))


def _digest(value: str | bytes) -> str:
    if isinstance(value, str):
        value = value.encode("utf-8")
    return hashlib.sha256(value).hexdigest()


@dataclass(frozen=True)
class InstructionSource:
    """One source contributing to a bundle's provenance manifest.

    ``content`` is optional for catalog entries whose body is fetched through
    a runtime reference.  ``sha256`` is always required so a persisted bundle
    can detect a changed source even when the content is not retained.
    """

    source_id: str
    kind: str
    version: str
    sha256: str
    provenance: str = ""
    content: str | None = None

    @classmethod
    def from_content(
        cls,
        source_id: str,
        kind: str,
        content: str,
        *,
        version: str = "",
        provenance: str = "",
    ) -> "InstructionSource":
        return cls(source_id, kind, version, _digest(content), provenance, content)

    def manifest(self) -> dict[str, str]:
        return {
            "source_id": self.source_id,
            "kind": self.kind,
            "version": self.version,
            "sha256": self.sha256,
            "provenance": self.provenance,
        }

    def to_dict(self, *, include_content: bool = True) -> dict[str, Any]:
        result: dict[str, Any] = self.manifest()
        if include_content and self.content is not None:
            result["content"] = self.content
        return result

    @classmethod
    def from_dict(cls, value: Mapping[str, Any]) -> "InstructionSource":
        return cls(
            str(value["source_id"]), str(value["kind"]), str(value.get("version", "")),
            str(value["sha256"]), str(value.get("provenance", "")),
            value.get("content"),
        )


@dataclass(frozen=True)
class InstructionBundle:
    """The intended instruction package and its delivery binding."""

    task_id: str
    assignment_ref: str
    attempt_id: str
    role_id: str
    role_kind: str
    engine: str
    engine_version: str
    text: str
    sources: tuple[InstructionSource, ...]
    assembled_sha256: str
    bundle_id: str
    sent_representation_sha256: str | None = None
    sent_engine: str | None = None
    sent_engine_version: str | None = None
    sent_attempt_id: str | None = None
    session_ref: str | None = None
    # ``engine_version`` is the selected/pinned routing version.  This field
    # is intentionally separate: an adapter can expose the installed version
    # only after negotiation, and that observation must never be represented
    # as if it had been known during routing.
    observed_engine_version: str | None = None
    sent_observed_engine_version: str | None = None

    @property
    def source_manifest_sha256(self) -> str:
        return _digest(_canonical([source.manifest() for source in self.sources]))

    @property
    def delivery_bound(self) -> bool:
        return self.sent_representation_sha256 is not None

    def bind(
        self,
        representation: str | bytes,
        *,
        engine: str,
        engine_version: str,
        attempt_id: str,
        session_ref: str | None = None,
        observed_engine_version: str | None = None,
    ) -> "InstructionBundle":
        """Bind the bundle to the actual adapter delivery.

        A representation from another engine or attempt is rejected instead
        of overwriting the previous receipt.  Rebinding the identical receipt
        is idempotent and is useful during crash recovery.
        """
        sent_hash = _digest(representation)
        identity = (engine, engine_version, attempt_id)
        existing = (self.sent_engine, self.sent_engine_version, self.sent_attempt_id)
        if self.delivery_bound:
            if (existing != identity or self.sent_representation_sha256 != sent_hash
                    or self.session_ref != session_ref
                    or self.sent_observed_engine_version != observed_engine_version):
                raise ValueError("instruction_bundle_delivery_conflict")
            return self
        if identity != (self.engine, self.engine_version, self.attempt_id):
            raise ValueError("instruction_bundle_attempt_mismatch")
        return replace(
            self,
            sent_representation_sha256=sent_hash,
            sent_engine=engine,
            sent_engine_version=engine_version,
            sent_attempt_id=attempt_id,
            session_ref=session_ref,
            observed_engine_version=observed_engine_version,
            sent_observed_engine_version=observed_engine_version,
        )

    def to_dict(self, *, include_content: bool = True) -> dict[str, Any]:
        return {
            "task_id": self.task_id,
            "assignment_ref": self.assignment_ref,
            "attempt_id": self.attempt_id,
            "role_id": self.role_id,
            "role_kind": self.role_kind,
            "engine": self.engine,
            "engine_version": self.engine_version,
            "text": self.text,
            "sources": [source.to_dict(include_content=include_content) for source in self.sources],
            "assembled_sha256": self.assembled_sha256,
            "source_manifest_sha256": self.source_manifest_sha256,
            "bundle_id": self.bundle_id,
            "sent_representation_sha256": self.sent_representation_sha256,
            "sent_engine": self.sent_engine,
            "sent_engine_version": self.sent_engine_version,
            "sent_attempt_id": self.sent_attempt_id,
            "session_ref": self.session_ref,
            "observed_engine_version": self.observed_engine_version,
            "sent_observed_engine_version": self.sent_observed_engine_version,
        }

    def to_json(self, *, include_content: bool = True) -> str:
        return _canonical(self.to_dict(include_content=include_content))

    @classmethod
    def from_dict(cls, value: Mapping[str, Any]) -> "InstructionBundle":
        sources = tuple(InstructionSource.from_dict(item) for item in value.get("sources", ()))
        bundle = cls(
            task_id=str(value["task_id"]), assignment_ref=str(value["assignment_ref"]),
            attempt_id=str(value["attempt_id"]), role_id=str(value["role_id"]),
            role_kind=str(value["role_kind"]), engine=str(value["engine"]),
            engine_version=str(value["engine_version"]), text=str(value["text"]),
            sources=sources, assembled_sha256=str(value["assembled_sha256"]),
            bundle_id=str(value["bundle_id"]),
            sent_representation_sha256=value.get("sent_representation_sha256"),
            sent_engine=value.get("sent_engine"), sent_engine_version=value.get("sent_engine_version"),
            sent_attempt_id=value.get("sent_attempt_id"), session_ref=value.get("session_ref"),
            observed_engine_version=value.get("observed_engine_version"),
            sent_observed_engine_version=value.get("sent_observed_engine_version"),
        )
        if _digest(bundle.text) != bundle.assembled_sha256:
            raise ValueError("instruction_bundle_assembled_hash_mismatch")
        return bundle


def _as_source(value: InstructionSource | Mapping[str, Any]) -> InstructionSource:
    if isinstance(value, InstructionSource):
        return value
    return InstructionSource.from_dict(value)


def build_instruction_bundle(
    *,
    task_id: str,
    assignment_ref: str,
    attempt_id: str,
    role_set: RoleInstructionSet,
    engine: str,
    engine_version: str,
    project_sources: Iterable[InstructionSource | Mapping[str, Any]] = (),
    skill_catalog: Iterable[InstructionSource | Mapping[str, Any]] = (),
    routes: Mapping[str, Any] | None = None,
    capabilities: Mapping[str, Any] | None = None,
    extra_blocks: Iterable[str] = (),
) -> InstructionBundle:
    """Build one deterministic bundle for a concrete assignment attempt."""
    project = tuple(_as_source(item) for item in project_sources)
    skills = tuple(_as_source(item) for item in skill_catalog)
    sources = (InstructionSource(
        role_set.role_id, "role", "", role_set.sha256, role_set.source_path,
        composed_instructions(role_set),
    ),) + project + skills
    sections = [composed_instructions(role_set)]
    if project:
        sections.append("## Project instructions\n\n" + "\n\n".join(
            f"### {source.source_id} (version {source.version or 'unspecified'}, sha256 {source.sha256})\n\n"
            + (source.content or "[content available through runtime reference]")
            for source in project
        ))
    sections.append("## Available skill and reference catalog\n\n" + (
        "\n".join(f"- `{s.source_id}` ({s.kind}, version {s.version or 'unspecified'}, sha256 {s.sha256})" for s in skills)
        if skills else "No additional catalog entries were selected."
    ))
    sections.append("## Runtime capabilities\n\n" + _canonical({
        "routes": dict(routes or {}), "capabilities": dict(capabilities or {}),
    }))
    sections.extend(str(block) for block in extra_blocks)
    text = "\n\n".join(sections)
    assembled = _digest(text)
    identity = _canonical({
        "task_id": task_id, "assignment_ref": assignment_ref, "attempt_id": attempt_id,
        "role_id": role_set.role_id, "role_kind": role_set.kind,
        "engine": engine, "engine_version": engine_version,
        "sources": [source.manifest() for source in sources], "assembled_sha256": assembled,
    })
    return InstructionBundle(
        task_id, assignment_ref, attempt_id, role_set.role_id, role_set.kind,
        engine, engine_version, text, sources, assembled, _digest(identity),
    )


def capture_instruction_bundle(
    *,
    task_id: str,
    assignment_ref: str,
    attempt_id: str,
    role_id: str,
    role_kind: str,
    engine: str,
    engine_version: str,
    observed_engine_version: str | None = None,
    assembled_text: str,
    sources: Iterable[InstructionSource | Mapping[str, Any]] = (),
) -> InstructionBundle:
    """Capture an already assembled representation without changing it.

    This is the adapter-facing entry point.  In particular, it does not call
    :func:`composed_instructions` or append a runtime section: the text hash
    is the exact text handed to the adapter.  The role source should normally
    be included in ``sources`` by the caller when it has that provenance.
    """
    normalized = tuple(_as_source(item) for item in sources)
    assembled_sha256 = _digest(assembled_text)
    identity = _canonical({
        "task_id": task_id,
        "assignment_ref": assignment_ref,
        "attempt_id": attempt_id,
        "role_id": role_id,
        "role_kind": role_kind,
        "engine": engine,
        "engine_version": engine_version,
        "sources": [source.manifest() for source in normalized],
        "assembled_sha256": assembled_sha256,
    })
    return InstructionBundle(
        task_id, assignment_ref, attempt_id, role_id, role_kind,
        engine, engine_version, assembled_text, normalized, assembled_sha256,
        _digest(identity), observed_engine_version=observed_engine_version,
    )


class InstructionBundleLedger:
    """Durable SQLite ledger for intended and actually sent representations.

    ``store`` is the existing Cortex ``Store`` (it only needs its
    ``connection()`` context manager).  The table is additive and created on
    first use so this helper can be introduced without a schema-version bump.
    """

    def __init__(self, store: Any):
        if not hasattr(store, "connection"):
            raise TypeError("store must provide connection()")
        self.store = store

    def _schema(self, db: sqlite3.Connection) -> None:
        db.execute(
            """CREATE TABLE IF NOT EXISTS instruction_bundles (
                bundle_id TEXT PRIMARY KEY,
                task_id TEXT NOT NULL,
                assignment_ref TEXT NOT NULL,
                attempt_id TEXT NOT NULL,
                role_id TEXT NOT NULL,
                role_kind TEXT NOT NULL,
                engine TEXT NOT NULL,
                engine_version TEXT NOT NULL,
                assembled_text TEXT NOT NULL,
                assembled_sha256 TEXT NOT NULL,
                sources_json TEXT NOT NULL,
                intended_json TEXT NOT NULL,
                sent_representation BLOB,
                sent_representation_sha256 TEXT,
                sent_engine TEXT,
                sent_engine_version TEXT,
                sent_attempt_id TEXT,
                session_ref TEXT,
                observed_engine_version TEXT,
                sent_observed_engine_version TEXT,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            )"""
        )
        db.execute(
            """CREATE TABLE IF NOT EXISTS instruction_bundle_deliveries (
                bundle_id TEXT NOT NULL,
                session_ref TEXT NOT NULL,
                representation BLOB NOT NULL,
                representation_sha256 TEXT NOT NULL,
                engine TEXT NOT NULL,
                engine_version TEXT NOT NULL,
                observed_engine_version TEXT,
                attempt_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                PRIMARY KEY(bundle_id, session_ref),
                FOREIGN KEY(bundle_id) REFERENCES instruction_bundles(bundle_id)
            )"""
        )
        # Existing stores may have the original additive table without the
        # selected/observed distinction.  Keep this migration local and
        # additive; the main Store schema version does not need to change.
        columns = {row[1] for row in db.execute("PRAGMA table_info(instruction_bundles)")}
        for name in ("observed_engine_version", "sent_observed_engine_version"):
            if name not in columns:
                db.execute(f"ALTER TABLE instruction_bundles ADD COLUMN {name} TEXT")

    @staticmethod
    def _now() -> str:
        return datetime.now(timezone.utc).isoformat()

    def save_intended(self, bundle: InstructionBundle) -> InstructionBundle:
        """Persist the intended bundle, idempotently and conflict-safely."""
        payload = bundle.to_json(include_content=True)
        sources = _canonical([source.to_dict(include_content=True) for source in bundle.sources])
        now = self._now()
        with self.store.connection() as db:
            self._schema(db)
            row = db.execute(
                "SELECT intended_json, assembled_sha256 FROM instruction_bundles WHERE bundle_id=?",
                (bundle.bundle_id,),
            ).fetchone()
            if row is not None:
                if row[0] != payload or row[1] != bundle.assembled_sha256:
                    raise ValueError("instruction_bundle_persistence_conflict")
                return bundle
            db.execute(
                """INSERT INTO instruction_bundles
                   (bundle_id,task_id,assignment_ref,attempt_id,role_id,role_kind,
                    engine,engine_version,assembled_text,assembled_sha256,sources_json,
                    intended_json,observed_engine_version,created_at,updated_at)
                   VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)""",
                (bundle.bundle_id, bundle.task_id, bundle.assignment_ref, bundle.attempt_id,
                 bundle.role_id, bundle.role_kind, bundle.engine, bundle.engine_version,
                 bundle.text, bundle.assembled_sha256, sources, payload,
                 bundle.observed_engine_version, now, now),
            )
        return bundle

    def record_sent(
        self,
        bundle: InstructionBundle,
        representation: str | bytes,
        *,
        engine: str,
        engine_version: str,
        attempt_id: str,
        session_ref: str | None = None,
        observed_engine_version: str | None = None,
    ) -> InstructionBundle:
        """Persist the exact sent bytes and return the delivery-bound bundle."""
        # A bundle is immutable at the assembly level but may have multiple
        # session deliveries during replacement/recovery.  Bind each delivery
        # from an unbound view; the per-session ledger below is authoritative.
        intended = replace(
            bundle, sent_representation_sha256=None, sent_engine=None,
            sent_engine_version=None, sent_attempt_id=None, session_ref=None,
            sent_observed_engine_version=None,
        )
        bound = intended.bind(
            representation, engine=engine, engine_version=engine_version,
            attempt_id=attempt_id, session_ref=session_ref,
            observed_engine_version=observed_engine_version,
        )
        self.save_intended(intended)
        raw = representation.encode("utf-8") if isinstance(representation, str) else bytes(representation)
        with self.store.connection() as db:
            self._schema(db)
            row = db.execute(
                """SELECT task_id,attempt_id
                   FROM instruction_bundles WHERE bundle_id=?""",
                (bundle.bundle_id,),
            ).fetchone()
            if row is None:
                raise ValueError("instruction_bundle_missing_intended")
            if row[0] != intended.task_id or row[1] != intended.attempt_id:
                raise ValueError("instruction_bundle_identity_conflict")
            delivery_key = session_ref or "__no_session__"
            delivery = db.execute(
                """SELECT representation,representation_sha256,engine,engine_version,
                          observed_engine_version,attempt_id
                     FROM instruction_bundle_deliveries
                    WHERE bundle_id=? AND session_ref=?""",
                (bundle.bundle_id, delivery_key),
            ).fetchone()
            if delivery is not None:
                if (bytes(delivery[0]) != raw or delivery[1] != bound.sent_representation_sha256 or
                        delivery[2] != engine or delivery[3] != engine_version or
                        delivery[4] != observed_engine_version or delivery[5] != attempt_id):
                    raise ValueError("instruction_bundle_delivery_conflict")
                return bound
            now = self._now()
            db.execute(
                """INSERT INTO instruction_bundle_deliveries
                   (bundle_id,session_ref,representation,representation_sha256,engine,
                    engine_version,observed_engine_version,attempt_id,created_at,updated_at)
                   VALUES (?,?,?,?,?,?,?,?,?,?)""",
                (bundle.bundle_id, delivery_key, raw, bound.sent_representation_sha256,
                 engine, engine_version, observed_engine_version, attempt_id, now, now),
            )
            db.execute(
                """UPDATE instruction_bundles
                   SET sent_representation=?, sent_representation_sha256=?,
                       sent_engine=?, sent_engine_version=?, sent_attempt_id=?,
                       session_ref=?, sent_observed_engine_version=?, updated_at=? WHERE bundle_id=?""",
                (raw, bound.sent_representation_sha256, engine, engine_version,
                 attempt_id, session_ref, observed_engine_version, now, bundle.bundle_id),
            )
        return bound

    def get(self, bundle_id: str) -> InstructionBundle | None:
        """Load the durable bundle, including its sent-delivery receipt."""
        with self.store.connection() as db:
            self._schema(db)
            row = db.execute("SELECT intended_json FROM instruction_bundles WHERE bundle_id=?", (bundle_id,)).fetchone()
        if row is None:
            return None
        # The intended JSON is immutable; sent fields are read separately so
        # the original assembled representation remains auditable.
        bundle = InstructionBundle.from_dict(json.loads(row[0]))
        with self.store.connection() as db:
            row = db.execute(
                """SELECT sent_representation_sha256,sent_engine,sent_engine_version,
                          sent_attempt_id,session_ref,sent_observed_engine_version
                     FROM instruction_bundles WHERE bundle_id=?""", (bundle_id,)
            ).fetchone()
        if row is None or row[0] is None:
            return bundle
        return replace(bundle, sent_representation_sha256=row[0], sent_engine=row[1],
                       sent_engine_version=row[2], sent_attempt_id=row[3], session_ref=row[4],
                       sent_observed_engine_version=row[5])
