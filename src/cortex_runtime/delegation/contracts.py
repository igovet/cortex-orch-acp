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
    "Required caller-chosen unique delivery key for this logical operation "
    "(a UUID is suitable). Reuse the exact key and arguments only to retry "
    "after an uncertain response; the same key with different arguments "
    "conflicts.",
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
        outcome=string(
            "One of 'passed', 'failed', 'not_run', or 'unverified'. Use 'unverified' "
            "when the check was attempted but its result cannot be established "
            "(for example a renderer timed out or exited before producing a "
            "trustworthy result); use 'not_run' only when it was never attempted.",
            16, pattern=r"^(passed|failed|not_run|unverified)$",
        ),
        tool_call_ref=string(
            "Optional: the exact tool_call_ref of a tool call this attempt's own engine "
            "session made that evidences this outcome. Copy it exactly; never guess one.",
            128,
        ),
    ),
    required=["name", "outcome"],
    additionalProperties=False,
)
CHECKS = dict(
    type="array",
    description=(
        "Bounded list of claimed evidence-checkable items for this result. Include one "
        "item for every acceptance check actually attempted; each item must name the "
        "check, report its outcome, and copy the exact tool_call_ref from the worker's "
        "own tool call when one produced the evidence. Use an empty array only when "
        "the assignment has no evidence-checkable acceptance criterion. "
        "Each is stored as a claim; only a check whose tool_call_ref matches a "
        "completed engine-observed tool call from this same attempt becomes "
        "'observed', and a matching failed observation is flagged 'contradicted'."
    ),
    maxItems=20,
    items=CHECK_ITEM,
)


def _operation(properties, required, *, delivery_key=True):
    properties = dict(properties)
    required = list(required)
    if delivery_key:
        properties.setdefault("request_key", REQUEST_KEY)
        if "request_key" not in required:
            required.append("request_key")
    # Keep the generated tools/list contract closed at every object level.
    # The delegation validator rejects unknown keys, so JSON Schema clients
    # must see the same boundary before sending a call.
    return dict(properties=properties, required=sorted(set(required)),
                additionalProperties=False)


COORDINATOR_OPERATIONS = {
    "delegate": _operation(
        {
            "title": string("Short English title for the delegated unit of work.", 200),
            "brief": string(
                "Complete work meaning, in English: outcome, constraints, checks, and "
                "owned resources. English whatever language you are speaking with the "
                "user -- a worker's whole world is English. No technical IDs; the "
                "runtime fills every binding.",
                16_000,
            ),
            "role": string(
                "Role id or alias resolved by portable.roles.resolve_role "
                "(for example 'worker-backend-dev' or 'backend-dev').",
                80,
            ),
            "acceptance": string(
                "Concrete, evidence-checkable acceptance criteria for this delegation, "
                "in English like the rest of what a worker is given.",
                4_000,
            ),
            "engine": string(
                "Optional: the engine to dispatch this role's worker on (for example "
                "'codex-acp' or 'claude-agent-acp'), chosen from this role's own line in the "
                "allowed-routes table delivered with these instructions. Omit to let the "
                "runtime pick that role's first allowed route from the route policy.",
                64,
                minimum=0,
            ),
            "model": string(
                "Optional: model name for 'engine', from the same allowed-routes line. Only "
                "meaningful together with 'engine'; ignored if 'engine' is omitted.",
                80,
                minimum=0,
            ),
            "effort": string(
                "Optional: effort/reasoning level for 'engine'+'model' (for example 'medium' "
                "or 'high'), from the same allowed-routes line's '{...}' set. Omit to use that "
                "route's own default effort.",
                32,
                minimum=0,
            ),
            "plan_assignment_ref": string(
                "Optional exact assignment id returned by update_plan for this unit of work. "
                "The runtime validates its task, requirements and predecessor bindings before dispatch.",
                128, minimum=0,
            ),
            "handoff_mode": string(
                "When this task already has a published worker result, choose exactly one: "
                "'independent' means this worker does not review that result; 'report_review' "
                "means this worker reviews the exact published predecessor report named by "
                "predecessor_report_ref. Omit only before this task has any worker result.",
                16, enum=["independent", "report_review"],
            ),
            "predecessor_report_ref": string(
                "Required with handoff_mode='report_review': exact report_ref copied unchanged "
                "from the predecessor worker_result event. It must be a published worker report "
                "from this task. Do not guess, reconstruct, or use a title or summary.",
                14, minimum=14, pattern=r"^r_[0-9a-f]{12}$",
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
            "limit": integer(
                "Optional event count per page, from 1 through 200; omit for the default 50. "
                "For more events, continue with the returned cursor in after rather than "
                "requesting a larger page. A rejected value must be corrected to the advertised bound.",
                200,
            ),
        },
        [], delivery_key=False,
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
                "Required target for this decision: copy the exact assignment_ref from the "
                "prior delegate result for an assignment decision, or use the literal 'task' "
                "for an overall task-level decision covering every retained assignment result. "
                "Accepting an assignment "
                "does not complete the task; successful completion requires a "
                "separate accept decision with assignment_ref='task'. Before task-level "
                "acceptance, publish a coordinator-authored current pipeline edition "
                "with the decision basis and reconcile all pending meaningful events. "
                "If acceptance_refused says to finish this turn, do so and wait for "
                "the next continuation instead of retrying in the same turn.",
                128,
            ),
            "decision": string(
                "One of 'accept', 'revise', or 'ask_user'.", 16,
                pattern=r"^(accept|revise|ask_user)$",
            ),
            "rationale": string("Concrete rationale grounded in retained results/events.", 4_000),
            "revision_brief": string(
                "Concrete follow-up instructions for decision='revise'; defaults to rationale when omitted.", 4_000,
            ),
            "question": string(
                "Required only when decision='ask_user': the exact question for the user.",
                4_000,
            ),
            "options": dict(
                type="array",
                description=(
                    "Optional, bounded list of short answer choices to offer the user "
                    "alongside decision='ask_user' (rendered as a single-select form field "
                    "when the host supports structured elicitation, else listed in the "
                    "text fallback). Omit for a free-text question."
                ),
                maxItems=8,
                items=string("One short answer choice.", 200),
            ),
        },
        ["assignment_ref", "decision", "rationale"],
    ),
    "integrate": _operation(
        {
            "assignment_ref": ASSIGNMENT_REF,
            "candidate_ref": string(
                "Optional exact candidate id returned by prepare_candidate's ready event. "
                "When supplied, integration applies this candidate after validating its digest and base.",
                128,
            ),
            "expected_base_digest": string(
                "Required with candidate_ref: exact base content digest returned by prepare_candidate. "
                "A changed main project is reported as a conflict.",
                64,
                pattern=r"^[0-9a-f]{64}$",
            ),
        },
        ["assignment_ref"],
    ),
    "update_plan": _operation(
        {
            "action": string(
                "Required operation selector. For a first plan call use "
                "create_requirement with statement (and optional criteria). "
                "Use revise_requirement with requirement_ref and statement, "
                "add_assignment with node_key (and optional exact requirement_bindings "
                "and predecessors), or bind_candidate with assignment_ref and candidate_ref. "
                "Do not use title, summary, body, or any other field name; those are not aliases "
                "for this contract. Each action changes only this task's model-owned work graph.",
                32, pattern=r"^(create_requirement|revise_requirement|add_assignment|bind_candidate)$",
            ),
            "assignment_ref": string(
                "For bind_candidate, copy the exact assignment_id returned by the earlier "
                "update_plan action='add_assignment' call (the plan node). Do not use the "
                "assignment_ref returned by delegate(); that is the executable worker "
                "assignment and is a different reference. For other actions, use the exact "
                "reference required by that action.", 128,
            ),
            "candidate_ref": string(
                "Exact ready candidate id returned by prepare_candidate; required for bind_candidate.", 128,
            ),
            "requirement_ref": string(
                "Exact requirement id from an earlier update_plan result; required for revise_requirement.", 128,
            ),
            "statement": string(
                "The statement field: user-visible requirement or updated criterion. Required for "
                "create_requirement and revise_requirement; for a first plan call, put the "
                "requirement text here (not in title, summary, or body).", 8_000,
            ),
            "criteria": dict(
                type="array", maxItems=32,
                description="Optional bounded, concrete acceptance criteria for a "
                            "create_requirement or revise_requirement call.",
                items=string("One concrete criterion.", 1_000),
            ),
            "node_key": string(
                "Short coordinator-chosen label for this work unit; required for "
                "add_assignment (for example 'frontend-implementation').", 200,
            ),
            "requirement_bindings": dict(
                type="array", maxItems=32,
                description="Exact requirement revisions that this assignment serves.",
                items=dict(type="object", properties=dict(
                    requirement_ref=string("Exact requirement id from update_plan.", 128),
                    revision=string("Existing decimal revision number.", 10, pattern=r"^[1-9][0-9]*$"),
                ), required=["requirement_ref", "revision"], additionalProperties=False),
            ),
            "predecessors": dict(
                type="array", maxItems=16,
                description="Exact completed predecessor plan assignments and their bound ready candidates. "
                            "Each assignment_ref is the assignment_id returned by update_plan/add_assignment, "
                            "never the assignment_ref returned by delegate().",
                items=dict(type="object", properties=dict(
                    assignment_ref=string("Exact assignment id from update_plan.", 128),
                    candidate_ref=string("Exact candidate reference bound to that predecessor.", 128),
                ), required=["assignment_ref", "candidate_ref"], additionalProperties=False),
            ),
        },
        ["action"],
    ),
    "read_plan": _operation(
        {"limit": dict(type="integer", description="Maximum graph nodes returned in this bounded read; default 100.",
                       minimum=1, maximum=100)}, [], delivery_key=False,
    ),
    "prepare_candidate": _operation(
        {"assignment_refs": dict(type="array", minItems=1, maxItems=8,
                                 description="Exact delegation references whose isolated results form one candidate.",
                                 items=ASSIGNMENT_REF)},
        ["assignment_refs"],
    ),
    "request_check": _operation(
        {
            "candidate_ref": string("Exact ready candidate id returned by prepare_candidate.", 128),
            "candidate_digest": string("Exact content digest from that candidate's ready event.", 64,
                                       pattern=r"^[0-9a-f]{64}$"),
            "argv": dict(type="array", minItems=1, maxItems=64,
                         description="Direct executable and arguments for a read-only candidate check; no shell.",
                         items=string("One literal argv element.", 4096)),
            "requirement_bindings": dict(
                type="array", maxItems=32,
                description=(
                    "Exact current requirement revisions this check evaluates. Before the first "
                    "request_check call, read_plan and copy every applicable requirement_ref "
                    "with its current revision here; this binds the observation at request time "
                    "and avoids a duplicate check after adding bindings. Omit only when the "
                    "check is intentionally unscoped or no requirement applies. Never guess a "
                    "revision; a stale binding is rejected."
                ),
                items=dict(type="object", properties=dict(
                    requirement_ref=string("Exact requirement id returned by update_plan.", 128),
                    revision=string("Current decimal requirement revision.", 10, pattern=r"^[1-9][0-9]*$"),
                ), required=["requirement_ref", "revision"], additionalProperties=False),
            ),
            "checked_files": dict(type="array", maxItems=128,
                                  description="Project-relative files covered by this check; omit if it covers the whole candidate.",
                                  items=string("One project-relative path.", 512)),
        },
        ["candidate_ref", "candidate_digest", "argv"],
    ),
    "read_check": _operation(
        {"check_ref": string("Exact check id returned by request_check; read only this task's observation.", 128)},
        ["check_ref"], delivery_key=False,
    ),
    "assess_check": _operation(
        {
            "check_ref": string("Exact completed check id returned by request_check.", 128),
            "assessment": string("Coordinator assessment: pass, fail, or inconclusive.", 16,
                                 pattern=r"^(pass|fail|inconclusive)$"),
            "rationale": string("Why this observation satisfies or does not satisfy a stated criterion.", 4_000),
        },
        ["check_ref", "assessment", "rationale"],
    ),
}


def _bounded_integer(description, maximum, *, minimum=0):
    return dict(type="integer", description=description, minimum=minimum, maximum=maximum)


# T018: shared read-only source-history operations, available on *both*
# coordinator and worker channels (task-scoped by the caller's own channel;
# see delegation.mcp_tools._shared_read_source/_shared_list_sources). No
# technical id beyond the opaque source_ref list_sources itself returns.
SOURCE_OPERATIONS = {
    "list_sources": _operation(
        {
            "after": _bounded_integer("Opaque revision cursor from a prior list_sources result; omit to start from the beginning.", 1_000_000_000),
            "limit": _bounded_integer("Maximum number of source entries to return in one page.", 200, minimum=1),
        },
        [], delivery_key=False,
    ),
    "read_source": _operation(
        {
            "source_ref": string(
                "Opaque source_ref from a prior list_sources entry. Copy it exactly; never construct or guess one.",
                64,
            ),
            "offset": _bounded_integer("Byte/character offset to resume a bounded read from.", 100_000_000),
            "limit": _bounded_integer("Maximum chunk size to return (bounded to 64 KiB server-side).", 65_536, minimum=1),
        },
        ["source_ref"], delivery_key=False,
    ),
}
COORDINATOR_OPERATIONS.update(SOURCE_OPERATIONS)

# T021: shared read-only recovery-capsule operation, available on *both*
# coordinator and worker channels (task/recipient-scoped by the caller's
# own channel -- see delegation.mcp_tools._shared_read_capsule; a channel
# can never request another task's or another role's capsule, since no
# task/recipient id beyond the channel's own binding is ever accepted).
# T035: the situational shared skills, readable by name. The role text
# has always said things like "load/apply `cortex:documentation-sync`",
# from when a plugin host could load a skill by name; in an ACP session
# nothing could, so that instruction was unsatisfiable. The always-on
# shared skills are delivered with the role itself (portable.roles
# .ALWAYS_ON_SKILLS); this is how a model reaches the rest.
SKILL_OPERATIONS = {
    "read_skill": _operation(
        {
            "skill": string(
                "The exact shared skill/reference id to read, for example "
                "'documentation-sync' or 'knowledge-harvest'. This is the only "
                "accepted input field: send it as `skill`; do not rename it to "
                "`topic`, `name`, or `id`. Only this runtime's own shared skills "
                "are readable; a role's own instructions are delivered, never fetched.",
                80,
            ),
        },
        ["skill"], delivery_key=False,
    ),
}

CAPSULE_OPERATIONS = {
    "read_capsule": _operation(
        {
            "at_revision": _bounded_integer(
                "Optional: bound the returned constraints section to source revisions "
                "captured at or before this revision number; omit for current state.",
                1_000_000_000,
            ),
        },
        [], delivery_key=False,
    ),
}
COORDINATOR_OPERATIONS.update(CAPSULE_OPERATIONS)
COORDINATOR_OPERATIONS.update(SKILL_OPERATIONS)

# T024: shared read-only large-output operation, available on *both*
# coordinator and worker channels (owner-scoped by the caller's own channel
# -- see delegation.mcp_tools._shared_read_large_output; a channel can
# never read another assignment's/the other role's persisted output, since
# no owner id beyond the channel's own binding is ever accepted).
LARGE_OUTPUT_OPERATIONS = {
    "read_large_output": _operation(
        {
            "large_output_id": string(
                "Opaque large_output_id from a terminal/tool-result reference (for "
                "example a terminal/output response's own large_output_id). Copy it "
                "exactly; never construct or guess one.",
                20,
            ),
            "offset": _bounded_integer("Byte offset to resume a bounded read from.", 1_000_000_000),
            "limit": _bounded_integer("Maximum chunk size to return (bounded to 64 KiB server-side).", 65_536, minimum=1),
        },
        ["large_output_id"], delivery_key=False,
    ),
}
COORDINATOR_OPERATIONS.update(LARGE_OUTPUT_OPERATIONS)

# T029: coordinator-only read-only diagnostics/usage summary for this
# channel's own task (see delegation.mcp_tools._coordinator_read_diagnostics
# -- never accepts a task id, always channel.task_id). Not offered on
# worker channels: resource attribution/caps are a coordinator-level
# concern in this runtime (see observability.metrics).
DIAGNOSTICS_OPERATIONS = {
    "read_diagnostics": _operation({}, [], delivery_key=False),
}
COORDINATOR_OPERATIONS.update(DIAGNOSTICS_OPERATIONS)

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
                "The report YOU published for this assignment. The successful write_report "
                "receipt names this same value report_id; copy that receipt's report_id "
                "unchanged into this field as report_ref (see contracts.REPORT). You publish it yourself; "
                "the coordinator never issues, confirms or hands one over -- asking it "
                "for one only stalls the assignment. Never a draft or governance id.",
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

WORKER_OPERATIONS.update(SOURCE_OPERATIONS)
WORKER_OPERATIONS.update(CAPSULE_OPERATIONS)
WORKER_OPERATIONS.update(LARGE_OUTPUT_OPERATIONS)
WORKER_OPERATIONS.update(SKILL_OPERATIONS)

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
            if len(value) < rule.get("minItems", 0):
                raise StoreError(
                    "invalid_arguments", key, f"{len(value)} items",
                    f"at least {rule['minItems']} items",
                    f"Add at least {rule['minItems']} item(s) to {key}.",
                )
            if len(value) > rule["maxItems"]:
                raise StoreError(
                    "invalid_arguments", key, f"{len(value)} items", f"at most {rule['maxItems']} items",
                    f"Shorten {key} to the advertised bound.",
                )
            item_schema = rule["items"]
            for index, item in enumerate(value):
                if item_schema.get("type") == "string":
                    _validate_string_item(key, index, item, item_schema)
                else:
                    _validate_object_item(key, index, item, item_schema)
    return args


def _validate_string_item(field: str, index: int, item, item_schema: dict) -> None:
    """T020: a plain scalar (string) array item -- for example ``decide``'s
    optional ``options`` list (short answer choices) -- as opposed to
    :func:`_validate_object_item`'s pre-T020 object-item convention (for
    example ``CHECKS``)."""
    label = f"{field}[{index}]"
    if not isinstance(item, str):
        raise StoreError("invalid_arguments", label, _received(item), "JSON string", f"Use a JSON string for {label}.")
    max_length = item_schema.get("maxLength", 4_000)
    min_length = item_schema.get("minLength", 1)
    if len(item) < min_length or len(item) > max_length:
        raise StoreError(
            "invalid_arguments", label, f"{len(item)} Unicode characters",
            f"{min_length} through {max_length} Unicode characters",
            f"Shorten or lengthen {label} to the advertised bound.",
        )


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
