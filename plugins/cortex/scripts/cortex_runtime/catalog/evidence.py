"""Distinct catalog facts with evidence provenance.

Six facts -- ``configured``, ``authorization_required``, ``catalog_obtained``,
``selected``, ``configuration_applied``, ``execution_verified`` -- are tracked
independently, each with its own :class:`Provenance`.  Promotion to the two
"an engine actually did this" facts (``configuration_applied``,
``execution_verified``) is only ever allowed from ``engine_report`` or
``observation`` provenance carrying a non-empty ``evidence_ref``; ``manual``
provenance is never allowed to claim ``catalog_obtained`` (that fact means
automatic discovery actually enumerated a route, which a manual declaration
never does).
"""
from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
from types import MappingProxyType
from typing import Mapping

from ..portable.errors import PortableError, PortableErrorCategory


class Fact(str, Enum):
    """Closed set of independently tracked catalog facts."""

    CONFIGURED = "configured"
    AUTHORIZATION_REQUIRED = "authorization_required"
    CATALOG_OBTAINED = "catalog_obtained"
    SELECTED = "selected"
    CONFIGURATION_APPLIED = "configuration_applied"
    EXECUTION_VERIFIED = "execution_verified"


class ProvenanceSource(str, Enum):
    """Where evidence for one fact came from."""

    DISCOVERY = "discovery"
    MANUAL = "manual"
    ENGINE_REPORT = "engine_report"
    OBSERVATION = "observation"


_CONFIRMATION_SOURCES = (ProvenanceSource.ENGINE_REPORT, ProvenanceSource.OBSERVATION)
_CONFIRMATION_FACTS = (Fact.CONFIGURATION_APPLIED, Fact.EXECUTION_VERIFIED)


@dataclass(frozen=True)
class Provenance:
    """Who/what asserted a fact, and when; ``evidence_ref`` is opaque."""

    source: ProvenanceSource
    adapter_version: str
    observed_at: str
    evidence_ref: str | None = None

    def __post_init__(self) -> None:
        if not isinstance(self.source, ProvenanceSource):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'source' must be a ProvenanceSource member.",
                retry_eligible=False,
                field="source",
            )
        if not isinstance(self.adapter_version, str) or not self.adapter_version:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'adapter_version' must be a non-empty string.",
                retry_eligible=False,
                field="adapter_version",
            )
        if not isinstance(self.observed_at, str) or not self.observed_at:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'observed_at' must be a non-empty ISO-8601 timestamp string.",
                retry_eligible=False,
                field="observed_at",
            )
        if self.evidence_ref is not None and (not isinstance(self.evidence_ref, str) or not self.evidence_ref):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'evidence_ref' must be a non-empty string or None.",
                retry_eligible=False,
                field="evidence_ref",
            )


class EvidenceState:
    """An immutable ``Fact -> Provenance`` mapping.

    Construct with :meth:`empty` and grow only through :meth:`promote`
    (never by mutating ``facts`` in place); every returned instance is a new,
    independent, frozen snapshot. ``__init__`` is not a public constructor:
    it requires an internal sentinel token that only :meth:`empty` and
    :meth:`promote` hold, so a caller can never bypass ``promote``'s
    provenance-source/evidence_ref rules by constructing an
    ``EvidenceState`` directly (e.g. handing ``execution_verified`` manual
    provenance straight to ``__init__``). ``__slots__`` blocks arbitrary
    extra attributes, ``__setattr__`` blocks reassigning ``_facts`` after
    construction, and ``__reduce__`` blocks the pickle/``copy.deepcopy``
    reconstruction path (which calls ``cls.__new__`` directly and would
    otherwise restore ``_facts`` without ever going through ``__init__``'s
    token check).

    These are practical guards, not a hard security boundary: any code
    already running in this same Python process can still call
    ``EvidenceState.__new__(EvidenceState)`` and then
    ``object.__setattr__(instance, "_facts", ...)`` to forge a state,
    exactly as it could reach into any other Python object's private
    fields. In-process forgery via ``__new__``/``object.__setattr__`` is
    not preventable in pure Python and is not the threat this module
    defends against. The real integrity boundary is that every legitimate
    producer of an ``EvidenceState`` -- :func:`discovery.build_catalog` and
    ``catalog.store``'s snapshot deserialization -- only ever calls
    :meth:`empty` and :meth:`promote`, so a fact can only ever reach this
    state through ``promote``'s provenance-source/evidence_ref rules.
    """

    __slots__ = ("_facts",)

    _SENTINEL = object()

    def __init__(self, facts: Mapping[Fact, Provenance], _token: object = None) -> None:
        if _token is not EvidenceState._SENTINEL:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_evidence_construction",
                remediation=(
                    "EvidenceState must be built via EvidenceState.empty() and "
                    "promote(), never constructed directly; those are the only "
                    "callers that hold the internal construction token."
                ),
                retry_eligible=False,
                field="facts",
            )
        object.__setattr__(self, "_facts", MappingProxyType(dict(facts)))

    def __setattr__(self, name: str, value: object) -> None:
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="immutable_evidence_state",
            remediation=(
                "EvidenceState is immutable after construction; build a new "
                "state via promote() instead of assigning to an existing one."
            ),
            retry_eligible=False,
            field=name,
        )

    def __reduce__(self):
        raise PortableError(
            category=PortableErrorCategory.VALIDATION,
            code="unpicklable_evidence_state",
            remediation=(
                "EvidenceState does not support pickling/deepcopy, which "
                "would otherwise reconstruct it without going through "
                "promote()'s validation; rebuild it via EvidenceState.empty()/"
                "promote() or the catalog.store deserialization path."
            ),
            retry_eligible=False,
            field=None,
        )

    @classmethod
    def empty(cls) -> "EvidenceState":
        return cls({}, cls._SENTINEL)

    @property
    def facts(self) -> Mapping[Fact, Provenance]:
        return self._facts

    def has(self, fact: Fact) -> bool:
        return fact in self._facts

    def get(self, fact: Fact) -> Provenance | None:
        return self._facts.get(fact)

    def promote(self, fact: Fact, provenance: Provenance) -> "EvidenceState":
        """Return a new state with ``fact`` set to ``provenance``.

        Rejects (never silently downgrades or drops):
        - ``manual`` provenance claiming ``catalog_obtained``.
        - ``configuration_applied``/``execution_verified`` from any source
          other than ``engine_report``/``observation``.
        - ``configuration_applied``/``execution_verified`` with no
          ``evidence_ref``.
        """
        if not isinstance(fact, Fact):
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="invalid_field",
                remediation="'fact' must be a Fact member.",
                retry_eligible=False,
                field="fact",
            )
        if provenance.source == ProvenanceSource.MANUAL and fact == Fact.CATALOG_OBTAINED:
            raise PortableError(
                category=PortableErrorCategory.VALIDATION,
                code="unverified_promotion",
                remediation=(
                    "A manual declaration never establishes 'catalog_obtained'; that fact "
                    "means automatic discovery actually enumerated this route."
                ),
                retry_eligible=False,
                field="fact",
            )
        if fact in _CONFIRMATION_FACTS:
            if provenance.source not in _CONFIRMATION_SOURCES:
                raise PortableError(
                    category=PortableErrorCategory.VALIDATION,
                    code="unverified_promotion",
                    remediation=(
                        f"'{fact.value}' may only be promoted from engine_report or "
                        "observation provenance; selecting a route never implies it was "
                        "applied or executed."
                    ),
                    retry_eligible=False,
                    field="fact",
                )
            if not provenance.evidence_ref:
                raise PortableError(
                    category=PortableErrorCategory.VALIDATION,
                    code="missing_confirmation_evidence",
                    remediation=(
                        f"'{fact.value}' requires a non-empty evidence_ref confirming the "
                        "engine actually applied/executed this configuration."
                    ),
                    retry_eligible=False,
                    field="evidence_ref",
                )
        updated = dict(self._facts)
        updated[fact] = provenance
        return EvidenceState(updated, EvidenceState._SENTINEL)

    def __eq__(self, other: object) -> bool:
        return isinstance(other, EvidenceState) and dict(self._facts) == dict(other._facts)

    def __repr__(self) -> str:  # pragma: no cover - debugging aid only
        return f"EvidenceState({dict(self._facts)!r})"
