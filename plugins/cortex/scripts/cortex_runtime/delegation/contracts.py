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

# T017: one claimed evidence-checkable item. ``tool_call_ref`` is optional --
# copy it verbatim from a tool call this same attempt's own engine session
# actually made (never invented); the runtime resolves it against durable
# engine-observed evidence for this exact attempt (see delegation.evidence)
# and labels the check claimed/observed/contradicted in the coordinator's
# continuation -- a claim is never itself treated as confirmed.
CHECK_ITEM = dict(
    type="object",
    properties=dict(
        name=string("Short name of this checked item (for example a test or acceptance clause).", 200),
        outcome=string("One of 'passed', 'failed', or 'not_run'.", 16, pattern=r"^(passed|failed|not_run)$"),
        tool_call_ref=string(
            "Optional: the exact tool_call_ref of a tool call this attempt's own engine "
            "session made that evidences this outcome. Copy it exactly; never guess one.",
            128,
        ),
    ),
    required=["name", "outcome"],
)
CHECKS = dict(
    type="array",
    description=(
        "Optional, bounded list of claimed evidence-checkable items for this result. "
        "Each is stored as a claim; only a check whose tool_call_ref matches a "
        "completed engine-observed tool call from this same attempt becomes "
        "'observed', and a matching failed observation is flagged 'contradicted'."
    ),
    maxItems=20,
    items=CHECK_ITEM,
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
    "decide": _operation(
        {
            "assignment_ref": string(
                "Opaque assignment reference from a prior 'delegate' result, or the "
                "literal 'task' for an overall task-level acceptance decision that "
                "covers every retained assignment result.",
                128,
            ),
            "decision": string(
                "One of 'accept', 'revise', or 'ask_user'.", 16,
                pattern=r"^(accept|revise|ask_user)$",
            ),
            "rationale": string("Concrete rationale grounded in retained results/events.", 4_000),
            "question": string(
                "Required only when decision='ask_user': the exact question for the user.",
                4_000,
            ),
        },
        ["assignment_ref", "decision", "rationale"],
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
            "checks": CHECKS,
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
        elif rule["type"] == "array":
            if not isinstance(value, list):
                raise StoreError("invalid_arguments", key, _received(value), "JSON array", f"Use a JSON array for {key}.")
            if len(value) > rule["maxItems"]:
                raise StoreError(
                    "invalid_arguments", key, f"{len(value)} items", f"at most {rule['maxItems']} items",
                    f"Shorten {key} to the advertised bound.",
                )
            item_schema = rule["items"]
            for index, item in enumerate(value):
                _validate_object_item(key, index, item, item_schema)
    return args


def _validate_object_item(field: str, index: int, item, item_schema: dict) -> None:
    label = f"{field}[{index}]"
    if not isinstance(item, dict):
        raise StoreError("invalid_arguments", label, _received(item), "JSON object", f"Use a JSON object for {label}.")
    properties = item_schema["properties"]
    unknown = set(item) - set(properties)
    if unknown:
        raise StoreError(
            "invalid_arguments", f"{label}: " + ", ".join(sorted(unknown)), "unadvertised field(s)",
            ", ".join(sorted(properties)),
            f"Remove unadvertised fields from {label} and resend using only its advertised fields.",
        )
    missing = set(item_schema["required"]) - set(item)
    if missing:
        raise StoreError(
            "invalid_arguments", f"{label}: " + ", ".join(sorted(missing)), "missing", "required field(s)",
            f"Add every required field to {label}.",
        )
    for key, value in item.items():
        rule = properties[key]
        item_label = f"{label}.{key}"
        if rule["type"] != "string":
            raise StoreError("invalid_arguments", item_label, _received(value), "JSON string", f"Use a JSON string for {item_label}.")
        if not isinstance(value, str):
            raise StoreError("invalid_arguments", item_label, _received(value), "JSON string", f"Use a JSON string for {item_label}.")
        if len(value) < rule.get("minLength", 1) or len(value) > rule["maxLength"]:
            raise StoreError(
                "invalid_arguments", item_label, f"{len(value)} Unicode characters",
                f"{rule.get('minLength', 1)} through {rule['maxLength']} Unicode characters",
                f"Shorten or lengthen {item_label} to the advertised bound.",
            )
        if "pattern" in rule:
            import re

            if not re.match(rule["pattern"], value):
                raise StoreError("invalid_arguments", item_label, _received(value), rule["pattern"], f"Match {item_label} to the advertised pattern.")
