"""T024: trust boundary for an optional, model-authored semantic summary.

A "semantic summary" here means an engine/model-produced free-text-or-JSON
digest of prior conversation/work -- something categorically different from
a :mod:`recovery.capsule` (which is *derived, never authored*: see that
module's docstring). No summary is ever a second source of exact state.
This module never itself builds a summary; it only decides whether one
already produced by some other component is safe to use *at all*, and if
not, says exactly why (never silently accepting invalid content, and never
treating "is valid JSON" as "is correct" -- task card line 31).

Checked, in order (any failure -> rejected, never partially trusted):

1. **Structure** -- must parse to a JSON object matching the small fixed
   schema (see :data:`REQUIRED_FIELDS`/:data:`OPTIONAL_FIELDS`); an
   unexpected top-level shape (list, scalar, missing required key, wrong
   type) is rejected outright.
2. **Provenance label** -- ``provenance`` must literally equal
   ``"derived_summary"`` (:mod:`recovery.capsule`'s ``TRUST_LABELS``); a
   summary that claims a stronger trust label (``user_direct``,
   ``worker_report``, ``file_content``) is always rejected -- a summarizer
   can never promote its own output to a higher trust tier than what it
   actually is (task card line 58: "Do not let a summarizer author
   permissions, identities, revisions, or successful statuses.").
3. **Size cap** -- the encoded summary must not exceed
   :data:`MAX_SUMMARY_BYTES`; an oversized "summary" defeats its own
   purpose and is rejected rather than truncated (truncating could cut a
   contradiction-bearing clause in half and hide it).
4. **Reference resolution** -- every id the summary claims to reference
   (``report_ref``/``source_ref``/``assignment_ref`` entries in
   ``references``) must resolve against the caller-supplied
   ``known_refs`` (ids the caller already independently confirmed exist in
   authoritative state, e.g. from a fresh :func:`recovery.capsule.build_capsule`
   call). An unresolvable reference is exactly the shape of a fabricated
   citation and is rejected.
5. **Contradiction with authoritative state** -- the summary is checked
   against a caller-supplied ``authoritative`` snapshot (a plain dict; the
   caller passes exactly the fields it can independently verify, e.g. from
   the same fresh capsule) for:
   - a claimed ``assignment_status`` that does not match
     ``authoritative["assignment_status"]``;
   - a claimed ``id`` (``task_id``/``assignment_ref``/``attempt_id``/
     ``report_id`` fields inside ``claims``) that does not match the
     corresponding authoritative id;
   - a claimed check outcome (``checks``: ``{name, status}`` pairs) that
     does not match ``authoritative["checks"]`` by name;
   - an omitted or altered constraint: every ``constraint_id`` present in
     ``authoritative["constraint_ids"]`` must still be present, verbatim by
     id, in the summary's own ``constraints_acknowledged`` list -- a summary
     that silently drops or renumbers a mandatory constraint is rejected,
     never "flagged and used anyway" for a *constraint* specifically (loss
     of a legal requirement is always fatal here; other contradictions are
     reported as an itemized, size-bounded ``violations`` list on the
     :class:`SummaryValidationResult`, whose caller decides whether it
     still wants to reject entirely -- see :func:`validate_summary`'s
     ``strict`` flag, default ``True`` -- rejecting is always the default).

Valid JSON is not semantic correctness (task card line 31): a summary can
pass every structural/schema check above and still be rejected purely on
contradiction grounds.
"""
from __future__ import annotations

import json
from dataclasses import dataclass, field
from typing import Any, Mapping

DERIVED_SUMMARY_LABEL = "derived_summary"
MAX_SUMMARY_BYTES = 8_000

REQUIRED_FIELDS = frozenset({"provenance", "text"})
OPTIONAL_FIELDS = frozenset({"references", "claims", "checks", "constraints_acknowledged"})
_ALL_FIELDS = REQUIRED_FIELDS | OPTIONAL_FIELDS

_CLAIM_ID_FIELDS = ("task_id", "assignment_ref", "attempt_id", "report_id")


@dataclass(frozen=True)
class SummaryValidationResult:
    valid: bool
    violations: tuple[str, ...] = ()
    summary: Mapping[str, Any] | None = None

    def as_dict(self) -> dict:
        return dict(valid=self.valid, violations=list(self.violations))


def _reject(*violations: str) -> SummaryValidationResult:
    return SummaryValidationResult(valid=False, violations=tuple(violations))


def _parse(raw: Any) -> tuple[dict | None, str | None]:
    if isinstance(raw, (bytes, str)):
        try:
            parsed = json.loads(raw)
        except (ValueError, TypeError):
            return None, "not_valid_json"
    elif isinstance(raw, dict):
        parsed = raw
    else:
        return None, "unsupported_input_type"
    if not isinstance(parsed, dict):
        return None, "top_level_must_be_object"
    return parsed, None


def validate_summary(
    raw: Any,
    *,
    known_refs: set[str] | frozenset[str] = frozenset(),
    authoritative: Mapping[str, Any] | None = None,
    strict: bool = True,
) -> SummaryValidationResult:
    """Validate one candidate semantic summary. Never raises on malformed
    input -- every failure mode is a normal, itemized rejection.

    ``known_refs``: ids the caller has independently confirmed exist (e.g.
    every ``report_id``/``assignment_ref`` already present in a freshly
    built capsule). ``authoritative``: the caller's own trusted snapshot
    used for the contradiction checks in the module docstring's step 5.
    ``strict`` (default ``True``): any violation rejects the whole summary;
    ``False`` returns ``valid=True`` with a non-empty ``violations`` list
    only when every violation found is a *non-constraint* contradiction
    (an omitted/altered constraint is always fatal regardless of
    ``strict`` -- see module docstring).
    """
    parsed, parse_error = _parse(raw)
    if parsed is None:
        return _reject(parse_error)

    violations: list[str] = []
    fatal: list[str] = []

    missing = REQUIRED_FIELDS - parsed.keys()
    if missing:
        fatal.append(f"missing_required_fields:{sorted(missing)}")
    unknown = set(parsed.keys()) - _ALL_FIELDS
    if unknown:
        fatal.append(f"unknown_fields:{sorted(unknown)}")

    if parsed.get("provenance") != DERIVED_SUMMARY_LABEL:
        fatal.append(f"provenance_must_be_derived_summary:{parsed.get('provenance')!r}")

    if not isinstance(parsed.get("text"), str):
        fatal.append("text_must_be_a_string")

    encoded_size = len(json.dumps(parsed, sort_keys=True).encode("utf-8"))
    if encoded_size > MAX_SUMMARY_BYTES:
        fatal.append(f"exceeds_size_cap:{encoded_size}>{MAX_SUMMARY_BYTES}")

    if fatal:
        return _reject(*fatal)

    references = parsed.get("references") or []
    if not isinstance(references, list):
        fatal.append("references_must_be_a_list")
    else:
        for ref in references:
            ref_id = ref.get("ref") if isinstance(ref, dict) else ref
            if not isinstance(ref_id, str) or ref_id not in known_refs:
                violations.append(f"unresolved_reference:{ref_id!r}")

    if fatal:
        return _reject(*fatal)

    authoritative = authoritative or {}
    claims = parsed.get("claims") or {}
    if isinstance(claims, dict):
        auth_status = authoritative.get("assignment_status")
        claimed_status = claims.get("assignment_status")
        if claimed_status is not None and auth_status is not None and claimed_status != auth_status:
            violations.append(f"contradicts_assignment_status:claimed={claimed_status!r},actual={auth_status!r}")
        for id_field in _CLAIM_ID_FIELDS:
            claimed_id = claims.get(id_field)
            actual_id = authoritative.get(id_field)
            if claimed_id is not None and actual_id is not None and claimed_id != actual_id:
                violations.append(f"contradicts_{id_field}:claimed={claimed_id!r},actual={actual_id!r}")
        if claims.get("completed") is True and authoritative.get("completed") is not True:
            violations.append("fabricated_completion_claim")
    elif claims:
        violations.append("claims_must_be_an_object")

    summary_checks = parsed.get("checks") or []
    authoritative_checks = {c.get("name"): c.get("status") for c in (authoritative.get("checks") or [])}
    if isinstance(summary_checks, list):
        for check in summary_checks:
            if not isinstance(check, dict):
                violations.append("check_entry_must_be_an_object")
                continue
            name, status = check.get("name"), check.get("status")
            if name in authoritative_checks and authoritative_checks[name] != status:
                violations.append(f"contradicts_check:{name}:claimed={status!r},actual={authoritative_checks[name]!r}")
    else:
        violations.append("checks_must_be_a_list")

    # Constraint omission/alteration is always fatal, regardless of strict.
    required_constraint_ids = set(authoritative.get("constraint_ids") or [])
    acknowledged = set(parsed.get("constraints_acknowledged") or [])
    missing_constraints = required_constraint_ids - acknowledged
    if missing_constraints:
        return _reject(f"omitted_constraints:{sorted(missing_constraints)}")

    if violations and strict:
        return _reject(*violations)
    return SummaryValidationResult(valid=True, violations=tuple(violations), summary=parsed)
