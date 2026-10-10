"""Budgeted, deterministic worker briefs for the provisioning boundary."""
from __future__ import annotations

from collections.abc import Mapping
import hashlib

from ..contracts import StoreError
from ..delegation.provisioning import WORKER_BRIEF_MAX_CHARS
from .state import encoded


_GOAL_REFERENCE = {
    "tool": "read_capsule",
    "arguments": {},
    "section": "constraints",
}


def _evidence_references(review_evidence, assignment_ref):
    if isinstance(review_evidence, Mapping):
        records = [review_evidence]
    elif isinstance(review_evidence, (list, tuple)):
        records = [item for item in review_evidence if isinstance(item, Mapping)]
    else:
        records = []

    kinds = {item.get("kind") for item in records}
    if review_evidence and not kinds.intersection({"status", "diff"}):
        kinds.update({"status", "diff"})

    references = []
    for kind in ("status", "diff"):
        if kind not in kinds:
            continue
        request_key = f"worker-brief:{assignment_ref}:{kind}"
        if len(request_key) > 128:
            request_key = f"worker-brief:{hashlib.sha256(request_key.encode('utf-8')).hexdigest()}:{kind}"
        references.append({
            "kind": kind,
            "tool": "project_git",
            "arguments": {"kind": kind, "request_key": request_key},
        })
        for record in records:
            output_id = record.get("large_output_id")
            if record.get("kind") == kind and isinstance(output_id, str) and output_id:
                references.append({
                    "kind": kind,
                    "tool": "read_large_output",
                    "arguments": {"large_output_id": output_id, "offset": 0},
                })
                break
    for record in records:
        if record.get("kind") == "runtime_verified_facts" and record.get("large_output_id"):
            references.append({
                "kind": "runtime_verified_facts",
                "tool": "read_large_output",
                "arguments": {"large_output_id": record["large_output_id"], "offset": 0},
            })
    return references


def build_worker_brief(*, objective, scope, dependency_reports, original_goal,
                       review_evidence, assignment_ref):
    """Encode a brief within the unchanged provisioning limit.

    Assignment facts and dependency references are mandatory. Only the original
    task goal and optional review evidence may be shortened, and any shortening
    is explicit and carries a worker-readable source reference.
    """
    if not isinstance(original_goal, str):
        raise StoreError("invalid_provisioning_request", "brief", type(original_goal).__name__,
                         "a UTF-8 task goal", "Supply the original task goal as text.")

    mandatory = {
        "objective": objective,
        "scope": scope,
        "dependency_reports": dependency_reports,
    }
    mandatory_length = len(encoded(mandatory))
    if mandatory_length > WORKER_BRIEF_MAX_CHARS:
        raise StoreError(
            "invalid_provisioning_request",
            "brief",
            mandatory_length,
            f"mandatory fields must fit within {WORKER_BRIEF_MAX_CHARS} characters",
            "Reduce the objective, scope, or dependency report references; these fields are never truncated.",
        )

    evidence_text = encoded(review_evidence)
    try:
        goal_bytes = len(original_goal.encode("utf-8"))
        evidence_bytes = len(evidence_text.encode("utf-8"))
    except UnicodeEncodeError as error:
        raise StoreError(
            "invalid_provisioning_request", "brief", "invalid Unicode",
            "UTF-8 encodable text", "Replace unpaired surrogate code points before dispatch.",
        ) from error

    evidence_references = _evidence_references(review_evidence, assignment_ref)

    def compose(goal_value, evidence_value):
        return encoded({
            "objective": objective,
            "scope": scope,
            "dependency_reports": dependency_reports,
            "original_goal": goal_value,
            "review_evidence": evidence_value,
        })

    full = compose(original_goal, review_evidence)
    if len(full) <= WORKER_BRIEF_MAX_CHARS:
        return full

    def goal_preview(shown):
        value = original_goal[:shown]
        return {
            "truncated": True,
            "original_characters": len(original_goal),
            "original_utf8_bytes": goal_bytes,
            "shown_characters": len(value),
            "preview": value,
            "full_text_reference": _GOAL_REFERENCE,
        }

    def evidence_preview(shown):
        value = evidence_text[:shown]
        return {
            "truncated": True,
            "original_characters": len(evidence_text),
            "original_utf8_bytes": evidence_bytes,
            "shown_characters": len(value),
            "preview": value,
            "full_text_references": evidence_references,
        }

    def largest_prefix(factory, source_length):
        low, high = 0, source_length
        if len(factory(0)) > WORKER_BRIEF_MAX_CHARS:
            return None
        while low < high:
            middle = (low + high + 1) // 2
            if len(factory(middle)) <= WORKER_BRIEF_MAX_CHARS:
                low = middle
            else:
                high = middle - 1
        return low, factory(low)

    candidates = []

    # Keep one optional source complete whenever that leaves room for a useful
    # preview of the other. Compare both priorities by total source characters
    # shown, with the task goal winning an exact tie.
    goal_candidate = largest_prefix(
        lambda shown: compose(original_goal, evidence_preview(shown)),
        len(evidence_text),
    )
    if goal_candidate is not None:
        shown, brief = goal_candidate
        candidates.append((brief, len(original_goal) + shown, 1))

    evidence_candidate = largest_prefix(
        lambda shown: compose(goal_preview(shown), review_evidence),
        len(original_goal),
    )
    if evidence_candidate is not None:
        shown, brief = evidence_candidate
        candidates.append((brief, shown + len(evidence_text), 0))

    # If neither source fits whole, increase both previews at the same
    # deterministic fraction and measure every candidate after JSON encoding.
    if len(compose(goal_preview(0), evidence_preview(0))) <= WORKER_BRIEF_MAX_CHARS:
        low, high = 0, 1_000_000
        while low < high:
            fraction = (low + high + 1) // 2
            goal_shown = len(original_goal) * fraction // 1_000_000
            evidence_shown = len(evidence_text) * fraction // 1_000_000
            candidate = compose(goal_preview(goal_shown), evidence_preview(evidence_shown))
            if len(candidate) <= WORKER_BRIEF_MAX_CHARS:
                low = fraction
            else:
                high = fraction - 1
        goal_shown = len(original_goal) * low // 1_000_000
        evidence_shown = len(evidence_text) * low // 1_000_000
        brief = compose(goal_preview(goal_shown), evidence_preview(evidence_shown))
        candidates.append((brief, goal_shown + evidence_shown, 0))

    if not candidates:
        raise StoreError(
            "invalid_provisioning_request",
            "brief",
            len(compose(goal_preview(0), evidence_preview(0))),
            f"mandatory fields and truncation references must fit within {WORKER_BRIEF_MAX_CHARS} characters",
            "Reduce mandatory assignment content so its truncation references can also be delivered.",
        )

    brief, _, _ = max(candidates, key=lambda candidate: (candidate[1], candidate[2]))
    return brief
