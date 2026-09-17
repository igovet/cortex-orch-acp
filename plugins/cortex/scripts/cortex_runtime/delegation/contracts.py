"""Internal (non-ACP) delegation operation schemas.

Two small, closed operation sets:

- coordinator: ``delegate``, ``list_events``, ``clarify``, ``cancel``.
- worker: ``ask_question``, ``publish_result``.

None of these schemas carry a task/attempt/session/channel/workspace/epoch
id field. Callers supply *work meaning* (title, brief, role, acceptance,
question, report reference, ...); every technical identifier is filled from
the caller's trusted binding by ``delegation.provisioning`` (see T005's
``trusted_channel``/``ownership``), never accepted as a model-supplied
argument. ``delegate`` is the only operation that creates new standing; its
result carries an opaque ``assignment_ref`` string that later operations
(``clarify``, ``cancel``) pass back unchanged -- it is a reference, not a
new ID the model invents or reconstructs.

``schemas_for(role_kind)`` returns a **fresh, independent copy** of the
schema dict on every call. Call it again whenever a session/generation
(re)starts instead of caching a prior return value, so a stale in-memory
schema can never diverge from what ``validate`` actually enforces (T015's
"current tool schemas remain available after a fresh session/generation"
acceptance line).
"""
from __future__ import annotations

import copy

from ..contracts import StoreError, integer, string

REQUEST_KEY = string(
    "Caller-chosen unique delivery key for this logical operation (a UUID is "
    "suitable). Reuse the exact key and arguments only to retry after an "
    "uncertain response; the same key with different arguments conflicts.",
    128,
)

ASSIGNMENT_REF = string(
    "Opaque assignment reference returned by a prior 'delegate' operation's "
    "result. Copy it exactly; never construct or guess one.",
    128,
)


def _operation(properties, required):
    properties = dict(properties)
    properties.setdefault("request_key", REQUEST_KEY)
    required = list(required)
    if "request_key" not in required:
        required.append("request_key")
    return dict(properties=properties, required=sorted(set(required)))


COORDINATOR_OPERATIONS = {
    "delegate": _operation(
        {
            "title": string("Short English title for the delegated unit of work.", 200),
            "brief": string(
                "Complete work meaning: outcome, constraints, checks, and owned "
                "resources. No technical IDs; the runtime fills every binding.",
                16_000,
            ),
            "role": string(
                "Role id or alias resolved by portable.roles.resolve_role "
                "(for example 'worker-backend-dev' or 'backend-dev').",
                80,
            ),
            "acceptance": string(
                "Concrete, evidence-checkable acceptance criteria for this delegation.",
                4_000,
            ),
        },
        ["title", "brief", "role", "acceptance"],
    ),
    "list_events": _operation(
        {
            "after": string(
                "Opaque event cursor from a prior list_events result; omit to start "
                "from the beginning of the retained window.",
                128,
                minimum=0,
            ),
            "limit": integer("Maximum number of events to return in one page.", 200),
        },
        [],
    ),
    "clarify": _operation(
        {
            "assignment_ref": ASSIGNMENT_REF,
            "message": string("Clarification or steering message for the delegated worker.", 8_000),
        },
        ["assignment_ref", "message"],
    ),
    "cancel": _operation(
        {
            "assignment_ref": ASSIGNMENT_REF,
            "reason": string("Concrete reason for cancelling this delegation.", 2_000),
        },
        ["assignment_ref", "reason"],
    ),
}

WORKER_OPERATIONS = {
    "ask_question": _operation(
        {
            "question": string("One bounded question for the delegating coordinator.", 4_000),
        },
        ["question"],
    ),
    "publish_result": _operation(
        {
            "report_ref": string(
                "Opaque published-report reference from an acknowledged publication "
                "receipt (see contracts.REPORT). Never a draft or governance id.",
                14,
                minimum=14,
                pattern=r"^r_[0-9a-f]{12}$",
            ),
            "summary": string("Concise summary of the published result for the coordinator.", 4_000),
        },
        ["report_ref", "summary"],
    ),
}

ROLE_KIND_OPERATIONS = {
    "coordinator": COORDINATOR_OPERATIONS,
    "worker": WORKER_OPERATIONS,
}


def schemas_for(role_kind: str) -> dict:
    """Return a fresh, independent copy of the operation schemas for a role kind.

    Raises ``StoreError('unknown_role_kind')`` for anything other than
    'coordinator' or 'worker' (delegation operations are not defined for
    the 'shared'/coordinator-support skill kind).
    """
    if role_kind not in ROLE_KIND_OPERATIONS:
        raise StoreError(
            "unknown_role_kind",
            "role_kind",
            role_kind,
            "'coordinator' or 'worker'",
            "Pass the delegation role_kind ('coordinator' or 'worker'), not a portable.roles.RoleInstructionSet.kind value directly unless it is one of these two.",
        )
    return copy.deepcopy(ROLE_KIND_OPERATIONS[role_kind])


def _received(value):
    if isinstance(value, str):
        return value if len(value) <= 160 else f"string with {len(value)} Unicode characters"
    if value is None or type(value) in (bool, int, float):
        return value
    return type(value).__name__


def validate(role_kind: str, operation: str, args: dict) -> dict:
    """Validate ``args`` against the current schema for ``role_kind``/``operation``.

    Always re-derives the schema via :func:`schemas_for` (never a cached
    copy), so validation reflects the current, not a stale, contract.
    Returns ``args`` unchanged on success; raises ``StoreError`` with a
    concrete field/received/expected/correction on failure -- the same
    shape ``contracts.validate`` uses for the advertised MCP tools.
    """
    schemas = schemas_for(role_kind)
    if operation not in schemas:
        raise StoreError(
            "unknown_operation",
            "operation",
            operation,
            ", ".join(sorted(schemas)),
            f"Use one of the {role_kind} delegation operations: {', '.join(sorted(schemas))}.",
        )
    schema = schemas[operation]
    if not isinstance(args, dict):
        raise StoreError(
            "invalid_arguments", "arguments", _received(args), "JSON object",
            "Send one object matching the operation's advertised schema.",
        )
    unknown = set(args) - set(schema["properties"])
    if unknown:
        raise StoreError(
            "invalid_arguments", ", ".join(sorted(unknown)), "unadvertised field(s)",
            ", ".join(sorted(schema["properties"])),
            "Remove unadvertised fields (task/attempt/session/channel/workspace/epoch "
            "ids are never accepted here) and resend using only schemas_for()'s current fields.",
        )
    missing = set(schema["required"]) - set(args)
    if missing:
        raise StoreError(
            "invalid_arguments", ", ".join(sorted(missing)), "missing", "required field(s)",
            "Add every named required field using its advertised description.",
        )
    for key, value in args.items():
        rule = schema["properties"][key]
        if rule["type"] == "string":
            if not isinstance(value, str):
                raise StoreError("invalid_arguments", key, _received(value), "JSON string", f"Use a JSON string for {key}.")
            if len(value) < rule.get("minLength", 1) or len(value) > rule["maxLength"]:
                raise StoreError(
                    "invalid_arguments", key, f"{len(value)} Unicode characters",
                    f"{rule.get('minLength', 1)} through {rule['maxLength']} Unicode characters",
                    f"Shorten or lengthen {key} to the advertised bound.",
                )
            if "pattern" in rule:
                import re

                if not re.match(rule["pattern"], value):
                    raise StoreError("invalid_arguments", key, _received(value), rule["pattern"], f"Match {key} to the advertised pattern.")
        elif rule["type"] == "integer":
            if not isinstance(value, int) or isinstance(value, bool):
                raise StoreError("invalid_arguments", key, _received(value), "JSON integer", f"Use a JSON integer for {key}.")
            if not rule["minimum"] <= value <= rule["maximum"]:
                raise StoreError(
                    "invalid_arguments", key, value, f"{rule['minimum']} through {rule['maximum']}",
                    f"Use an integer within the advertised bound for {key}.",
                )
    return args
