"""Portable identities and relationship state.

Stdlib-only, engine-agnostic value types.  Identifiers follow the
``<prefix>_<12 lowercase hex characters>`` convention used by
``cortex_runtime.store.identifier`` (see store.py:120), generated here
independently via :func:`new_id` so this package has no dependency on
``store.py``.  ``ExecutionRoute`` is the one identity whose id is a
deterministic content fingerprint rather than a random allocation; see
:func:`route_fingerprint`.
"""
from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
import hashlib
import json
from types import MappingProxyType
from typing import Any, Mapping
import re
import uuid

from .errors import PortableError, PortableErrorCategory

_RANDOM_SUFFIX = re.compile(r"[0-9a-f]{12}\Z")
_FINGERPRINT_SUFFIX = re.compile(r"[0-9a-f]{24}\Z")


def new_id(prefix: str) -> str:
    """Allocate a runtime-side identifier mirroring store.py:120."""
    return prefix + uuid.uuid4().hex[:12]


def _invalid_identifier(prefix: str, value: Any, field_name: str) -> PortableError:
    return PortableError(
        category=PortableErrorCategory.VALIDATION,
        code="invalid_identifier",
        remediation=(
            f"Use an identifier beginning with '{prefix}' followed by 12 "
            "lowercase hexadecimal characters, exactly as issued; never "
            "invent or truncate one."
        ),
        retry_eligible=False,
        field=field_name,
    )


def _validate_prefixed_id(value: Any, prefix: str, field_name: str = "id") -> str:
    if not isinstance(value, str) or not value.startswith(prefix) or not _RANDOM_SUFFIX.fullmatch(value[len(prefix):]):
        raise _invalid_identifier(prefix, value, field_name)
    return value


def _validate_fingerprint_id(value: Any, prefix: str, field_name: str = "id") -> str:
    if not isinstance(value, str) or not value.startswith(prefix) or not _FINGERPRINT_SUFFIX.fullmatch(value[len(prefix):]):
        raise _invalid_identifier(prefix, value, field_name)
    return value


def _require_str(value: Any, field_name: str, *, allow_empty: bool = False) -> str:
    if not isinstance(value, str) or (not allow_empty and not value):
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="invalid_field",
            remediation=f"'{field_name}' must be a non-empty string.",
            retry_eligible=False,
            field=field_name,
        )
    return value


_FREEZE_SCALAR_TYPES = (str, int, float, bool, bytes, type(None))


def freeze_snapshot(value: Any, field_name: str = "snapshot") -> Any:
    """Recursively deep-freeze a value into an immutable equivalent.

    dict/Mapping -> ``MappingProxyType`` of recursively frozen values;
    list/tuple -> ``tuple`` of recursively frozen items; set/frozenset ->
    ``frozenset`` of recursively frozen items; str/int/float/bool/bytes/None
    are returned unchanged (already immutable). Any other type -- including
    mutable containers such as ``bytearray`` and arbitrary objects -- is
    rejected with a ``PortableError`` rather than silently accepted as
    "frozen" while still being mutable or non-reproducible.
    """
    if isinstance(value, Mapping):
        return MappingProxyType({key: freeze_snapshot(item, field_name) for key, item in value.items()})
    if isinstance(value, (list, tuple)):
        return tuple(freeze_snapshot(item, field_name) for item in value)
    if isinstance(value, (set, frozenset)):
        return frozenset(freeze_snapshot(item, field_name) for item in value)
    if isinstance(value, _FREEZE_SCALAR_TYPES):
        return value
    raise PortableError(
        category=PortableErrorCategory.VALIDATION,
        code="unfreezable_value",
        remediation=(
            f"'{field_name}' contains a value of type {type(value).__name__} that cannot be "
            "deep-frozen; use only mappings, lists/tuples, sets, strings, numbers, bytes, "
            "bools, or None inside an immutable snapshot."
        ),
        retry_eligible=False,
        field=field_name,
    )


def _require_positive_int(value: Any, field_name: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="invalid_field",
            remediation=f"'{field_name}' must be a non-negative integer.",
            retry_eligible=False,
            field=field_name,
        )
    return value


@dataclass(frozen=True)
class HostSession:
    """One host-side session that owns coordinator/worker identity."""

    id: str

    def __post_init__(self) -> None:
        _validate_prefixed_id(self.id, "hs_")


@dataclass(frozen=True)
class Task:
    """A durable unit of work; reuses the ``t_`` convention from store.py."""

    id: str

    def __post_init__(self) -> None:
        _validate_prefixed_id(self.id, "t_")


@dataclass(frozen=True)
class OwnershipEpoch:
    """A monotonic ownership generation over some resource.

    ``owner`` is an opaque reference (for example a ``Task.id``) identifying
    who currently holds the epoch.  There is no default epoch: callers that
    need to fail closed on an absent epoch pass ``None`` explicitly and
    validators reject it (see :func:`validate_assignment`).
    """

    epoch: int
    owner: str

    def __post_init__(self) -> None:
        _require_positive_int(self.epoch, "epoch")
        _require_str(self.owner, "owner")


@dataclass(frozen=True)
class Connection:
    """An engine connection descriptor. Never carries a secret value."""

    id: str
    engine: str
    provider: str
    endpoint: str | None
    auth_mode: str
    credential_ref: str | None

    def __post_init__(self) -> None:
        _validate_prefixed_id(self.id, "cn_")
        _require_str(self.engine, "engine")
        _require_str(self.provider, "provider")
        _require_str(self.auth_mode, "auth_mode")
        if self.endpoint is not None:
            _require_str(self.endpoint, "endpoint")
        if self.credential_ref is not None:
            _require_str(self.credential_ref, "credential_ref")


def _normalize_supported_parameters(
    supported_parameters: tuple[tuple[str, str], ...] | None,
) -> tuple[tuple[str, str], ...]:
    if supported_parameters is None:
        return ()
    normalized = []
    for item in supported_parameters:
        if (
            not isinstance(item, tuple)
            or len(item) != 2
            or not isinstance(item[0], str)
            or not isinstance(item[1], str)
        ):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'supported_parameters' must be an iterable of (name, value) string pairs.",
                retry_eligible=False,
                field="supported_parameters",
            )
        normalized.append((item[0], item[1]))
    return tuple(sorted(normalized))


def route_fingerprint(
    engine: str,
    connection_id: str,
    model: str,
    supported_parameters: tuple[tuple[str, str], ...],
) -> str:
    """Deterministic id for an :class:`ExecutionRoute`.

    Parameter order never affects the fingerprint (the caller-normalized,
    sorted tuple is used); a different ``connection_id`` for the same
    ``engine``/``model`` always produces a different fingerprint.
    """
    canonical = json.dumps(
        {
            "engine": engine,
            "connection_id": connection_id,
            "model": model,
            "supported_parameters": [list(pair) for pair in supported_parameters],
        },
        sort_keys=True,
        ensure_ascii=False,
        separators=(",", ":"),
    )
    digest = hashlib.sha256(canonical.encode("utf-8")).hexdigest()
    return "xr_" + digest[:24]


@dataclass(frozen=True)
class ExecutionRoute:
    """A resolved (engine, connection, model, parameters) selection.

    ``id`` is derived, never supplied by the caller: it is a content
    fingerprint of the other fields, so two routes with the same engine and
    model but different connections always have different ids, and
    parameter order never changes the id.
    """

    engine: str
    connection_id: str
    model: str
    supported_parameters: tuple[tuple[str, str], ...] = ()
    id: str = field(init=False)

    def __post_init__(self) -> None:
        _require_str(self.engine, "engine")
        _validate_prefixed_id(self.connection_id, "cn_", "connection_id")
        _require_str(self.model, "model")
        normalized = _normalize_supported_parameters(self.supported_parameters)
        object.__setattr__(self, "supported_parameters", normalized)
        object.__setattr__(
            self,
            "id",
            route_fingerprint(self.engine, self.connection_id, self.model, normalized),
        )


@dataclass(frozen=True)
class WorkspaceBinding:
    """Binds a task to a concrete workspace root and known base revision."""

    id: str
    task_id: str
    workspace_root: str
    known_base_revision: str | None

    def __post_init__(self) -> None:
        _validate_prefixed_id(self.id, "wb_")
        _validate_prefixed_id(self.task_id, "t_", "task_id")
        _require_str(self.workspace_root, "workspace_root")
        if self.known_base_revision is not None:
            _require_str(self.known_base_revision, "known_base_revision")


@dataclass(frozen=True)
class Assignment:
    """One task's binding to a route, workspace, and configuration revision."""

    id: str
    task_id: str
    route: ExecutionRoute
    workspace_binding_id: str
    configuration_revision_id: str

    def __post_init__(self) -> None:
        _validate_prefixed_id(self.id, "a_")
        _validate_prefixed_id(self.task_id, "t_", "task_id")
        if not isinstance(self.route, ExecutionRoute):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'route' must be an ExecutionRoute instance.",
                retry_eligible=False,
                field="route",
            )
        _validate_prefixed_id(self.workspace_binding_id, "wb_", "workspace_binding_id")
        _validate_prefixed_id(self.configuration_revision_id, "cr_", "configuration_revision_id")


@dataclass(frozen=True)
class ExecutionAttempt:
    """One immutable, applied-configuration snapshot for an assignment.

    ``applied_configuration`` is recursively deep-frozen (see
    :func:`freeze_snapshot`) into nested read-only mappings/tuples/
    frozensets, so neither the caller's original object nor any value
    reachable from the stored snapshot can be mutated afterward.
    """

    id: str
    assignment_id: str
    engine_session_id: str
    applied_configuration: Mapping[str, Any]

    def __post_init__(self) -> None:
        _validate_prefixed_id(self.id, "xa_")
        _validate_prefixed_id(self.assignment_id, "a_", "assignment_id")
        _validate_prefixed_id(self.engine_session_id, "es_", "engine_session_id")
        if not isinstance(self.applied_configuration, Mapping):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'applied_configuration' must be a mapping snapshot.",
                retry_eligible=False,
                field="applied_configuration",
            )
        object.__setattr__(self, "applied_configuration", freeze_snapshot(self.applied_configuration, "applied_configuration"))


@dataclass(frozen=True)
class EngineSession:
    """A live engine-side session bound to one task and route."""

    id: str
    task_id: str
    route_id: str
    current_context_generation_id: str | None = None

    def __post_init__(self) -> None:
        _validate_prefixed_id(self.id, "es_")
        _validate_prefixed_id(self.task_id, "t_", "task_id")
        _validate_fingerprint_id(self.route_id, "xr_", "route_id")
        if self.current_context_generation_id is not None:
            _validate_prefixed_id(self.current_context_generation_id, "cg_", "current_context_generation_id")


class ContextGenerationReason(str, Enum):
    """Why a new context generation was started for an engine session."""

    CREATED = "created"
    COMPACTION = "compaction"
    RESET = "reset"
    SESSION_REPLACEMENT = "session_replacement"


@dataclass(frozen=True)
class ContextGeneration:
    """One monotonic generation of an engine session's context."""

    id: str
    engine_session_id: str
    reason: ContextGenerationReason
    number: int

    def __post_init__(self) -> None:
        _validate_prefixed_id(self.id, "cg_")
        _validate_prefixed_id(self.engine_session_id, "es_", "engine_session_id")
        if not isinstance(self.reason, ContextGenerationReason):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'reason' must be a ContextGenerationReason member.",
                retry_eligible=False,
                field="reason",
            )
        number = _require_positive_int(self.number, "number")
        if number < 1:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'number' must be a monotonic generation counter starting at 1.",
                retry_eligible=False,
                field="number",
            )


@dataclass(frozen=True)
class ConfigurationRevision:
    """One numbered configuration state for a task."""

    id: str
    task_id: str
    revision: int
    route: ExecutionRoute

    def __post_init__(self) -> None:
        _validate_prefixed_id(self.id, "cr_")
        _validate_prefixed_id(self.task_id, "t_", "task_id")
        _require_positive_int(self.revision, "revision")
        if not isinstance(self.route, ExecutionRoute):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'route' must be an ExecutionRoute instance.",
                retry_eligible=False,
                field="route",
            )


class ApplicationConfirmationState(str, Enum):
    """Whether a selected configuration was confirmed as actually applied."""

    UNCONFIRMED = "unconfirmed"
    CONFIRMED = "confirmed"
    FAILED = "failed"


@dataclass(frozen=True)
class ExecutionConfiguration:
    """A view distinguishing selected, next-effective, and active revisions.

    Selecting a revision never implies it was applied: ``active`` only
    reflects a revision the engine confirmed, and ``confirmation_state`` is
    ``confirmed`` only alongside a non-empty ``confirmation_evidence_ref``.
    """

    selected: ConfigurationRevision
    next_effective: ConfigurationRevision | None
    active: ConfigurationRevision | None
    confirmation_state: ApplicationConfirmationState
    confirmation_evidence_ref: str | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.selected, ConfigurationRevision):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'selected' must be a ConfigurationRevision instance.",
                retry_eligible=False,
                field="selected",
            )
        for name, value in (("next_effective", self.next_effective), ("active", self.active)):
            if value is not None and not isinstance(value, ConfigurationRevision):
                raise PortableError(
                    category=PortableErrorCategory.VALIDATION,
                    code="invalid_field",
                    remediation=f"'{name}' must be a ConfigurationRevision instance or None.",
                    retry_eligible=False,
                    field=name,
                )
        if not isinstance(self.confirmation_state, ApplicationConfirmationState):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'confirmation_state' must be an ApplicationConfirmationState member.",
                retry_eligible=False,
                field="confirmation_state",
            )
        if self.confirmation_state == ApplicationConfirmationState.CONFIRMED and not self.confirmation_evidence_ref:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="missing_confirmation_evidence",
                remediation=(
                    "A 'confirmed' application state requires a non-empty "
                    "confirmation_evidence_ref; selecting a revision never "
                    "implies it was applied."
                ),
                retry_eligible=False,
                field="confirmation_evidence_ref",
            )


@dataclass(frozen=True)
class SourceEvent:
    """An externally originated event, deduplicated by identity.

    Equality and hashing use only ``(origin, event_id)``; any carried
    ``text`` is excluded from both (``compare=False``), so two events with
    the same identity but different observed text still dedup as one.
    """

    origin: str
    event_id: str
    text: str | None = field(default=None, compare=False)

    def __post_init__(self) -> None:
        _require_str(self.origin, "origin")
        _require_str(self.event_id, "event_id")


def validate_assignment(
    task: Task,
    assignment: Assignment,
    workspace_binding: WorkspaceBinding,
    config_revision: ConfigurationRevision,
    *,
    current_epoch: OwnershipEpoch | None,
    current_revision: int | None,
) -> None:
    """Reject cross-task links and stale/missing ownership or revision.

    ``current_epoch`` and ``current_revision`` are required keyword
    arguments (no default) precisely so a caller cannot forget to supply
    them; passing ``None`` for either fails closed rather than skipping the
    check.
    """
    if assignment.task_id != task.id:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="cross_task_assignment",
            remediation="An assignment's task_id must match the task it is validated against.",
            retry_eligible=False,
            field="task_id",
        )
    if workspace_binding.task_id != task.id:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="cross_task_workspace_binding",
            remediation="A workspace binding's task_id must match the task it is validated against.",
            retry_eligible=False,
            field="task_id",
        )
    if config_revision.task_id != task.id:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="cross_task_configuration_revision",
            remediation="A configuration revision's task_id must match the task it is validated against.",
            retry_eligible=False,
            field="task_id",
        )
    if assignment.workspace_binding_id != workspace_binding.id:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="workspace_binding_mismatch",
            remediation="assignment.workspace_binding_id must reference the supplied workspace_binding.",
            retry_eligible=False,
            field="workspace_binding_id",
        )
    if assignment.configuration_revision_id != config_revision.id:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="configuration_revision_mismatch",
            remediation="assignment.configuration_revision_id must reference the supplied config_revision.",
            retry_eligible=False,
            field="configuration_revision_id",
        )
    if current_epoch is None:
        raise PortableError(
            category=PortableErrorCategory.AUTHORIZATION,
            code="missing_ownership_epoch",
            remediation="Supply the current ownership epoch before validating this assignment; absence is rejected, never treated as authorized.",
            retry_eligible=False,
            field="current_epoch",
        )
    if current_epoch.owner != task.id:
        raise PortableError(
            category=PortableErrorCategory.AUTHORIZATION,
            code="stale_ownership_epoch",
            remediation="The supplied ownership epoch does not belong to this task; obtain the current epoch before retrying.",
            retry_eligible=False,
            field="current_epoch",
        )
    if current_revision is None:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="missing_configuration_revision",
            remediation="Supply the current configuration revision number before validating this assignment.",
            retry_eligible=False,
            field="current_revision",
        )
    if config_revision.revision != current_revision:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="stale_configuration_revision",
            remediation="This configuration revision is no longer current; re-read the latest revision and retry.",
            retry_eligible=True,
            field="current_revision",
        )
