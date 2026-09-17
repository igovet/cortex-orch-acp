"""Catalog value types: entries, snapshots, and their derived views."""
from __future__ import annotations

from dataclasses import dataclass
from types import MappingProxyType
from typing import Mapping

from ..connections.model import ConnectionState
from ..portable.errors import PortableError, PortableErrorCategory
from ..portable.identity import ExecutionRoute, _validate_prefixed_id
from .evidence import EvidenceState, Fact

_UNQUALIFIED_MODES = frozenset({"gateway"})


@dataclass(frozen=True)
class CatalogEntry:
    """One truthful (route, connection, label, evidence) catalog row."""

    route: ExecutionRoute
    connection_id: str
    label: str
    disambiguated_label: str
    evidence: EvidenceState
    catalog_snapshot_id: str
    # T024: optional, backward-compatible route-level context-window budget
    # hint (in tokens). Absent for every existing caller (no discovery path
    # observes this from any pinned adapter/route today -- see
    # ``recovery.token_budget``'s conservative-default fallback, always used
    # when this is ``None``). When present it is a real evidenced bound, not
    # a Cortex-invented number, and must be a positive integer.
    context_window_tokens: int | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.route, ExecutionRoute):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'route' must be an ExecutionRoute instance.",
                retry_eligible=False,
                field="route",
            )
        _validate_prefixed_id(self.connection_id, "cn_", "connection_id")
        if self.route.connection_id != self.connection_id:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="route_connection_mismatch",
                remediation="A catalog entry's connection_id must equal its route's connection_id.",
                retry_eligible=False,
                field="connection_id",
            )
        for field_name in ("label", "disambiguated_label"):
            value = getattr(self, field_name)
            if not isinstance(value, str) or not value:
                raise PortableError(
                    category=PortableErrorCategory.VALIDATION,
                    code="invalid_field",
                    remediation=f"'{field_name}' must be a non-empty string.",
                    retry_eligible=False,
                    field=field_name,
                )
        if not isinstance(self.evidence, EvidenceState):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'evidence' must be an EvidenceState instance.",
                retry_eligible=False,
                field="evidence",
            )
        _validate_prefixed_id(self.catalog_snapshot_id, "cs_", "catalog_snapshot_id")
        if self.context_window_tokens is not None and (
            not isinstance(self.context_window_tokens, int)
            or isinstance(self.context_window_tokens, bool)
            or self.context_window_tokens <= 0
        ):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'context_window_tokens' must be a positive integer or None.",
                retry_eligible=False,
                field="context_window_tokens",
            )

    def is_selectable(self, connection_state: ConnectionState, auth_mode: str) -> bool:
        """True only when this entry may be offered for a new execution.

        Excludes: an authorization-required or otherwise non-configured
        connection, an unqualified/gateway auth mode, and an entry the
        evidence marks as invalid (``authorization_required`` fact present
        without a later ``configured`` promotion is treated as not
        selectable, matching connection state).
        """
        if connection_state != ConnectionState.CONFIGURED:
            return False
        if auth_mode in _UNQUALIFIED_MODES:
            return False
        if self.evidence.has(Fact.AUTHORIZATION_REQUIRED) and not self.evidence.has(Fact.CONFIGURED):
            return False
        return True


@dataclass(frozen=True)
class CatalogSnapshot:
    """One point-in-time catalog build across every known connection."""

    id: str
    created_at: str
    connection_registry_fingerprint: str
    entries: tuple[CatalogEntry, ...]
    unavailable: Mapping[str, str]
    setup_action: str | None = None

    def __post_init__(self) -> None:
        _validate_prefixed_id(self.id, "cs_")
        if not isinstance(self.created_at, str) or not self.created_at:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'created_at' must be a non-empty ISO-8601 timestamp string.",
                retry_eligible=False,
                field="created_at",
            )
        if not isinstance(self.connection_registry_fingerprint, str) or not self.connection_registry_fingerprint:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'connection_registry_fingerprint' must be a non-empty string.",
                retry_eligible=False,
                field="connection_registry_fingerprint",
            )
        if not isinstance(self.entries, tuple) or not all(isinstance(item, CatalogEntry) for item in self.entries):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'entries' must be a tuple of CatalogEntry instances.",
                retry_eligible=False,
                field="entries",
            )
        if not isinstance(self.unavailable, Mapping):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'unavailable' must be a mapping.",
                retry_eligible=False,
                field="unavailable",
            )
        object.__setattr__(self, "unavailable", MappingProxyType(dict(self.unavailable)))
        if self.setup_action is not None and (not isinstance(self.setup_action, str) or not self.setup_action):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'setup_action' must be a non-empty string or None.",
                retry_eligible=False,
                field="setup_action",
            )

    def all_entries(self) -> tuple[CatalogEntry, ...]:
        """Every discovered/declared entry, regardless of selectability."""
        return self.entries

    def entry_for_route(self, route_id: str) -> CatalogEntry | None:
        for entry in self.entries:
            if entry.route.id == route_id:
                return entry
        return None

    def selectable_for_new_execution(
        self, connections: Mapping[str, "object"]
    ) -> tuple[CatalogEntry, ...]:
        """Entries a user may pick for a brand-new task, given live connection state.

        ``connections`` maps ``connection_id -> ConnectionRecord``; only
        entries whose connection is currently ``CONFIGURED`` with a
        qualified (non-gateway) auth mode, and whose own evidence is not
        stuck at ``authorization_required``, are returned.
        """
        selectable = []
        for entry in self.entries:
            record = connections.get(entry.connection_id)
            if record is None:
                continue
            if entry.is_selectable(record.state, record.auth_mode.value):
                selectable.append(entry)
        return tuple(selectable)
