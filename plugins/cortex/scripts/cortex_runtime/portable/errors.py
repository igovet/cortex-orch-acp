"""Portable, engine-agnostic error taxonomy.

This module intentionally does not import anything from the rest of
``cortex_runtime``.  ``PORTABLE_ERROR_MAPPING`` below is a hand-maintained,
documented mapping from a few representative existing
``cortex_runtime.contracts.StoreError`` codes (see that module's
``ERROR_HELP``) to the portable taxonomy; it exists to show how a
host-specific error surfaces here, not to make this package depend on
``contracts.py``.
"""
from __future__ import annotations

from enum import Enum


class PortableErrorCategory(str, Enum):
    """Closed set of portable failure categories.

    validation
        The request itself is malformed or violates a precondition; retrying
        the identical request without changing it will fail again.
    authorization
        The caller lacks standing (ownership, binding, or a required
        epoch/receipt) to perform the action.
    transport
        A recoverable communication or contention failure between this
        process and its counterpart (host, engine process, or storage lock).
    engine_failure
        The engine itself reported a failure while otherwise reachable.
    indeterminate_write
        A mutation was attempted but its outcome is unknown; the prior
        attempt may or may not have applied.
    """

    VALIDATION = "validation"
    AUTHORIZATION = "authorization"
    TRANSPORT = "transport"
    ENGINE_FAILURE = "engine_failure"
    INDETERMINATE_WRITE = "indeterminate_write"


class PortableError(Exception):
    """Structured, model-actionable portable failure.

    ``retry_eligible`` is required and has no default: every raise site must
    make an explicit, considered choice about whether the identical request
    may be retried unchanged.  ``category=INDETERMINATE_WRITE`` and
    ``retry_eligible=True`` together are rejected at construction (raises
    ``ValueError``): an indeterminate write's outcome is unknown, so a blind
    retry is never safe.  Sending the identical request under the same
    delivery key is idempotent replay/reconciliation, not a blind retry (see
    ``lifecycle.reconcile``), and governs when retry becomes eligible again.
    """

    def __init__(
        self,
        category: PortableErrorCategory,
        code: str,
        remediation: str,
        retry_eligible: bool,
        *,
        field: str | None = None,
        details: str | None = None,
    ) -> None:
        if not isinstance(category, PortableErrorCategory):
            raise TypeError("category must be a PortableErrorCategory")
        if not isinstance(code, str) or not code:
            raise TypeError("code must be a non-empty string")
        if not isinstance(remediation, str) or not remediation:
            raise TypeError("remediation must be a non-empty string")
        if not isinstance(retry_eligible, bool):
            raise TypeError("retry_eligible must be an explicit bool")
        if category == PortableErrorCategory.INDETERMINATE_WRITE and retry_eligible:
            raise ValueError(
                "an indeterminate_write PortableError must never be constructed with "
                "retry_eligible=True: the prior mutation's outcome is unknown, so a blind "
                "retry is unsafe. Reconcile first (see lifecycle.reconcile), then issue a "
                "new, idempotent request under the same delivery key."
            )
        super().__init__(code)
        self.category = category
        self.code = code
        self.remediation = remediation
        self.retry_eligible = retry_eligible
        self.field = field
        self.details = details

    def __repr__(self) -> str:  # pragma: no cover - debugging aid only
        return (
            f"PortableError(category={self.category!r}, code={self.code!r}, "
            f"retry_eligible={self.retry_eligible!r})"
        )


# A few representative cortex_runtime.contracts.StoreError codes, mapped by
# hand into the portable taxonomy.  Source: plugins/cortex/scripts/
# cortex_runtime/contracts.py ERROR_HELP.  This dict is documentation plus a
# convenience lookup; it does not import contracts.py and is not exhaustive.
STORE_ERROR_CODE_MAPPING: dict[str, tuple[PortableErrorCategory, str, bool]] = {
    # (category, remediation, retry_eligible)
    "invalid_arguments": (
        PortableErrorCategory.VALIDATION,
        "Correct the named field to match the advertised schema before "
        "sending a new request; retrying the identical arguments repeats "
        "the same failure.",
        False,
    ),
    "coordinator_only": (
        PortableErrorCategory.AUTHORIZATION,
        "Only the bound coordinator may perform this action; a worker must "
        "not retry it under its own identity.",
        False,
    ),
    "storage_busy": (
        PortableErrorCategory.TRANSPORT,
        "Offline maintenance currently owns the storage lock; wait for it "
        "to complete and retry the identical request.",
        True,
    ),
    "storage_error": (
        PortableErrorCategory.INDETERMINATE_WRITE,
        "Storage did not complete cleanly; the mutation's outcome is "
        "unknown. This is not a blind retry: reconcile first, then send "
        "only the identical request and delivery key so an already-applied "
        "write is recognized as an idempotent replay rather than repeated "
        "as a new attempt. (See cortex_runtime.contracts.ERROR_HELP"
        "['storage_error'].)",
        False,
    ),
    "identifier_unavailable": (
        PortableErrorCategory.TRANSPORT,
        "Short identifier allocation was exhausted; retry the identical "
        "request later rather than inventing an identifier.",
        True,
    ),
}
