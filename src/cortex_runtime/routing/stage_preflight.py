"""Read-only preflight: which stages of a run an exact selected route covers.

A captured user model request covers planned exact-route nodes, their
retries and their review fixes (``orchestration.state.attempt_routing``).
The runtime-created mandatory review node has no plan hint, so no user request
ever covers it; only an owner pin decides its route deterministically
(``auto_route.choose``: user request, then pin, then ranking). This preflight
lets a caller refuse a paid run visibly before launch when the review stage
would not run on the selected route, instead of finding a substituted
reviewer afterwards.

It reads only the owner routing policy (``auto_route.load_overrides``). It never
reads or refreshes connection registries, quota caches or a task store, never
grants consent and never writes. Connection availability, quota and a task's
own profile/ceiling are re-checked at dispatch, where a failure is a visible
refusal, never a substitute route.
"""
from __future__ import annotations

import hashlib
import os
import re
from collections.abc import Mapping

from . import auto_route, model_policy
from ..portable.errors import PortableError

REVIEW_ROLE = "worker-code-reviewer"
_ROUTE = re.compile(r"([a-z0-9-]{1,64})/([A-Za-z0-9_.\[\]-]{1,128})(?::([a-z]{1,16}))?")
_LIMITS = (
    "a captured user request covers execution, retries and review fixes only after the user names the route",
    "connection availability, quota and the task profile/ceiling are re-checked at dispatch",
    "a pin effort the model policy does not permit is clamped and recorded at dispatch",
)


def _routing_source(project_root, environ) -> dict | None:
    """The policy file load_overrides uses: its kind and content digest only."""
    kinds = (["project"] if project_root else []) + (["environment"] if environ.get(auto_route.ENV_VAR) else []) + ["user"]
    for kind, path in zip(kinds, auto_route._candidate_paths(project_root, environ)):
        if path.is_file():
            return dict(kind=kind, sha256=hashlib.sha256(path.read_bytes()).hexdigest())
    return None


def _hard_policy(overrides: Mapping, engine: str, model: str) -> str | None:
    """The owner-policy refusal ``auto_route`` would raise for this route."""
    if auto_route._denied(overrides, engine, model):
        return "route_denied_by_owner_policy"
    providers = overrides.get("providers")
    if providers is not None and engine not in providers:
        return "route_provider_not_allowed"
    if not model_policy.policy_for(engine, model, overrides).tiers:
        return "route_policy_missing"
    return None


def preflight(route: str, project_root=None, environ: Mapping[str, str] | None = None) -> dict:
    """Assess ``engine/model[:effort]`` for every stage; ``ready`` only when all are covered."""
    match = _ROUTE.fullmatch(route) if isinstance(route, str) else None
    if match is None:
        return dict(ready=False, refusals=["invalid_route"], stages={}, limits=list(_LIMITS))
    engine, model, effort = match.groups()
    label = f"{engine}/{model}"
    environ = dict(os.environ if environ is None else environ)
    try:
        overrides = auto_route.load_overrides(project_root, environ)
    except PortableError as error:
        return dict(ready=False, route=label, effort=effort, refusals=[f"routing_policy_unavailable:{error.code}"],
                    stages={}, limits=list(_LIMITS))
    refusals = []
    blocked = _hard_policy(overrides, engine, model)
    execution = dict(status="refused", code=blocked) if blocked else dict(
        status="requires_captured_user_request", hard_policy="allowed")
    if blocked:
        refusals.append(f"execution:{blocked}")
    pin = auto_route._pin_for(overrides, REVIEW_ROLE)
    if pin is None:
        review = dict(status="refused", code="review_route_not_pinned", resolved="automatic_ranking")
    else:
        pin_engine, pin_model, pin_effort = pin
        resolved = f"{pin_engine}/{pin_model}" + (f":{pin_effort}" if pin_effort else "")
        code = ("review_route_pin_mismatch" if (pin_engine, pin_model) != (engine, model)
                else "review_effort_mismatch" if effort is not None and pin_effort != effort
                else _hard_policy(overrides, pin_engine, pin_model))
        review = dict(status="refused", code=code, resolved=resolved) if code else dict(status="pinned", resolved=resolved)
    if review["status"] == "refused":
        refusals.append(f"review:{review['code']}")
    stages = dict(execution=execution, review_fix=dict(status="inherits_execution"), review=review)
    return dict(ready=not refusals, route=label, effort=effort, routing_source=_routing_source(project_root, environ),
                stages=stages, refusals=refusals, limits=list(_LIMITS))
