"""Bounded advisory metadata helpers for ACP reports.

These helpers describe model-authored evidence quality. They do not authorize,
route, accept, or reject work.
"""
from __future__ import annotations

from itertools import islice
import math
from collections.abc import Mapping

ADVISORY_EVIDENCE_FIELDS = (
    "delivery_state", "acceptance_boundary", "causal_model_delta",
    "predecessor_rollout", "retry_discriminator", "receipt_references",
)
_MAX_DEPTH = 3
_MAX_ITEMS = 32
_MAX_TEXT = 512
_MAX_INTEGER = (1 << 53) - 1
_MAX_FLOAT = 1e308
_REVIEW_FIELDS = ("artifact_revision", "acceptance_boundary", "check_identity")


def _bounded(value, depth=0):
    if value is None or type(value) is bool:
        return value
    if type(value) is int:
        return value if abs(value) <= _MAX_INTEGER else None
    if type(value) is float:
        return value if math.isfinite(value) and abs(value) <= _MAX_FLOAT else None
    if isinstance(value, str):
        return value[:_MAX_TEXT]
    if depth >= _MAX_DEPTH:
        return None
    if isinstance(value, dict):
        result = {}
        for key, item in islice(value.items(), _MAX_ITEMS):
            if isinstance(key, str) and key and len(key) <= _MAX_TEXT:
                result[key] = _bounded(item, depth + 1)
        return result
    if isinstance(value, (list, tuple)):
        return [_bounded(item, depth + 1) for item in islice(value, _MAX_ITEMS)]
    return None


def _present(value):
    return value is not None and value not in ("", [], {}, ())


def _justified(value):
    if isinstance(value, str):
        return bool(value.strip())
    if isinstance(value, (dict, list, tuple, set)):
        return bool(value)
    return value is not None and value is not False and value != 0


def structured_advisory_evidence(fields=None, **values):
    supplied = {}
    if isinstance(fields, Mapping):
        for key in ADVISORY_EVIDENCE_FIELDS:
            if key in fields:
                supplied[key] = fields[key]
    supplied.update({key: value for key, value in values.items()
                     if key in ADVISORY_EVIDENCE_FIELDS and value is not None})
    result = {key: _bounded(supplied.get(key)) for key in ADVISORY_EVIDENCE_FIELDS}
    missing = [key for key in ADVISORY_EVIDENCE_FIELDS if not _present(result[key])]
    result.update(
        missing_fields=missing,
        diagnostics=[{"code": "missing_advisory_evidence_field", "field": key,
                      "severity": "advisory", "advisory_only": True} for key in missing],
        advisory_only=True, automatic_routing=False, acceptance_gate=False,
    )
    return result


def _review_identity(value):
    if not isinstance(value, dict):
        return None
    items = tuple(value.get(key) for key in _REVIEW_FIELDS)
    if any(type(item) is not str or not item or len(item) > _MAX_TEXT for item in items):
        return None
    return items


def duplicate_review_reuse_diagnostic(previous_checks, current_check, *,
                                      fresh_evidence=None, rerun_reason=None,
                                      max_checks=128):
    if type(max_checks) is not int or max_checks < 1:
        return None
    identity = _review_identity(current_check)
    if identity is None or _justified(fresh_evidence) or _justified(rerun_reason):
        return None
    if isinstance(previous_checks, dict):
        candidates = [previous_checks]
    elif isinstance(previous_checks, (list, tuple)):
        candidates = islice(previous_checks, min(max_checks, 128))
    else:
        return None
    if not any(_review_identity(candidate) == identity for candidate in candidates):
        return None
    return {"code": "duplicate_review_reuse",
            "identity": dict(zip(_REVIEW_FIELDS, identity)),
            "severity": "advisory", "advisory_only": True,
            "automatic_routing": False, "acceptance_gate": False}


def advisory_replanning_metadata(*, failed_canary=None, review=None, cost=None,
                                 evidence=None, delivery_state=None,
                                 acceptance_boundary=None, causal_model_delta=None,
                                 predecessor_rollout=None, retry_discriminator=None,
                                 receipt_references=None, previous_checks=None,
                                 current_check=None, fresh_evidence=None,
                                 rerun_reason=None, incident_decision=None,
                                 incident_trace=None):
    result = {"failed_canary": _bounded(failed_canary), "review": _bounded(review),
              "cost": _bounded(cost), "advisory_only": True,
              "automatic_routing": False, "acceptance_gate": False}
    direct = {"delivery_state": delivery_state,
              "acceptance_boundary": acceptance_boundary,
              "causal_model_delta": causal_model_delta,
              "predecessor_rollout": predecessor_rollout,
              "retry_discriminator": retry_discriminator,
              "receipt_references": receipt_references}
    if evidence is not None or any(value is not None for value in direct.values()):
        result["evidence"] = structured_advisory_evidence(
            evidence, **{key: value for key, value in direct.items() if value is not None})
    duplicate = duplicate_review_reuse_diagnostic(
        previous_checks, current_check, fresh_evidence=fresh_evidence,
        rerun_reason=rerun_reason)
    if duplicate is not None:
        result["diagnostics"] = [duplicate]
    if incident_decision is not None:
        result["incident_decision"] = _bounded(incident_decision)
    if incident_trace is not None:
        from .incident_quality import evaluate_trace
        result["incident_quality"] = evaluate_trace(incident_trace)
    return result
