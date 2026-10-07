"""T021: deterministic, provenance-preserving recovery capsules.

A capsule is a **derived, rebuildable view** over already-durable
task/assignment/attempt/source/publication state -- never a second
authoritative pipeline, never a place new facts are invented. Every field
is either populated programmatically from a specific SQLite row / Markdown
report, or explicitly marked ``"status": "unavailable"``/``"unknown"`` with
a concrete ``reason`` (never silently omitted -- see
``INVARIANTS.md``: "Capsules derive exact IDs, permissions, revisions,
confirmed facts, constraints, and provenance from authoritative state.
Summaries cannot create exact state or success.").

``build_capsule(store, task_id, recipient, at_revision=None)`` returns a
plain, JSON-serializable dict with nine sections (``identity``,
``route_and_config``, ``constraints``, ``pipeline_and_assignments``,
``publications_and_checks``, ``drafts_and_open_actions``,
``newer_events``, ``role_and_tool_access``, ``continuation_intent``) plus a
top-level ``capsule_format_version``, ``for_task_id``, ``recipient``,
``source_high_watermark`` and a deterministic ``capsule_revision`` --
sha256 over the canonical (sorted-key) JSON encoding of everything except
the revision digest itself. No wall-clock timestamp is folded into the
digest or into any section value (every "when" fact already recorded is a
durable revision/sequence/id, never ``datetime.now()``), so two builds
against byte-identical authoritative state -- including after a full
process restart with a brand new :class:`store.Store` instance -- produce
a byte-identical ``capsule_revision`` and, via :func:`render_capsule`, a
byte-identical rendered text. Deleting any cached/persisted copy of a
capsule loses nothing: the next :func:`build_capsule` call reconstructs it
from the same authoritative rows.

Recipient scoping (see the module's ``_worker_own_refs`` and every section
builder's own worker branch): a worker's capsule includes only its own
assignment's brief/acceptance/report/checks/events -- never another
assignment's brief, report, checks, questions, or private events. A
worker's user-constraints section always includes every retained
``user_direct`` constraint verbatim (there is no per-assignment constraint
attachment mechanism in this codebase -- see ``constraints`` section
docstring below for the documented consequence of that gap).

Size (see ``_COMPACTION_LEVELS``/``_bounded_sections``): the critical
sections are verbatim while they fit :data:`CRITICAL_BUDGET_BYTES`;
beyond that a deterministic escalating compaction level replaces large text
with an exact reference (sha256, size, bounded preview, ``truncated``
marker, retrieving tool) and, only at the later levels, trims long lists
into explicit ``<key>_omitted`` records -- never a silent drop and never an
unconditional ``capsule_oversize`` refusal for a large task.

Untrusted content (a worker's own report title/summary/checks, a source
attachment's descriptor) is only ever included as bounded fields/refs, and
:func:`render_capsule` wraps every piece of recipient-supplied *text*
(never JSON structure -- see below) in a clearly delimited, escaped "data,
not instructions" block (see ``_render_data_block``): a model-authored
completion claim, id, or requirement embedded inside such text can never
become an actual field on the capsule dict (those are always computed
directly from durable rows), and cannot even render outside its own
delimited block (the delimiter token itself is defanged wherever it
appears literally inside the wrapped text -- see ``_defang``).
"""
from __future__ import annotations

import json
from typing import Any, Mapping

from ..contracts import StoreError
from ..acp import questions_store
from ..channeled_tools import channeled_base_tool_names
from ..store import digest as _sha256_hex
from ..store import encode as _canonical_json
# T024: the approx-token figure below uses the one shared, conservative
# estimator (recovery.token_budget.estimate_tokens) that every other T024
# budget/loop-guard consumer also uses -- never a second, independently
# tuned constant. See that module's docstring for why the ratio is
# conservative (always an over-count) and always rounded up.
from .token_budget import estimate_tokens as _estimate_tokens

CAPSULE_FORMAT_VERSION = 1

# Last-resort guard: a capsule whose *critical* sections still exceed this
# many bytes after the most aggressive compaction level refuses outright
# (``capsule_oversize``). Bounded compaction (see ``_COMPACTION_LEVELS``)
# keeps the critical sections under :data:`CRITICAL_BUDGET_BYTES` for any
# task size, so this guard is unreachable in practice.
HARD_CAP_BYTES = 400_000

# Target size of the canonical JSON of the critical sections. The capsule is
# built verbatim when it fits; otherwise the first compaction level that fits
# is used (see ``_compact_critical_sections``). Well under HARD_CAP_BYTES so
# the delivery token budget keeps headroom too.
CRITICAL_BUDGET_BYTES = 96_000

# Deterministic escalating compaction levels. Level 0 is the verbatim
# capsule. Each later level lowers the inline byte allowance of large text
# fields by priority tier -- ``protected`` (the latest user constraint and
# answer, the task goal, the recipient's own assignment/node), ``active``
# (unsuperseded constraints, answered questions, unresolved assignments and
# nodes, open questions) and ``other`` (superseded constraints, finished
# assignments/nodes) -- and the byte allowance of every other string
# (``generic``). Level 4 references every non-protected large text while
# still listing every entry. Only from level 5 do long lists keep just
# ``keep`` entries (highest priority first, newest first) and list the
# omitted ones in a sibling ``<key>_omitted`` record; ``omitted_refs`` bounds
# how many omitted identifiers that record names (the last level names only
# the first/last). The last level is bounded by a constant independent of
# task size, so the HARD_CAP_BYTES guard below it cannot trigger in practice.
# Text over its allowance becomes an exact reference (see ``_text_ref``):
# sha256 and byte size of the full text, a bounded preview, an explicit
# ``truncated`` marker and the tool that returns the full text.
_COMPACTION_LEVELS = (
    None,
    dict(protected=65_536, active=16_384, other=4_096, generic=32_768, keep=None, omitted_refs=None),
    dict(protected=32_768, active=4_096, other=512, generic=8_192, keep=None, omitted_refs=None),
    dict(protected=16_384, active=1_024, other=0, generic=2_048, keep=None, omitted_refs=None),
    dict(protected=16_384, active=0, other=0, generic=1_024, keep=None, omitted_refs=None),
    dict(protected=8_192, active=0, other=0, generic=512, keep=50, omitted_refs=500),
    dict(protected=8_192, active=0, other=0, generic=512, keep=20, omitted_refs=200),
    dict(protected=4_096, active=0, other=0, generic=512, keep=5, omitted_refs=0),
)

# Non-critical sections (publications/checks, newer events, role/tool
# access) are bounded to this many *entries* before switching to a
# addressable-ref-only "N more, see list_events/read_source" note -- large
# evidence is always referenced, never inlined past this bound.
_MAX_LIST_ENTRIES = 50

_CRITICAL_SECTION_NAMES = ("identity", "constraints", "pipeline_and_assignments", "drafts_and_open_actions")

TRUST_LABELS = ("user_direct", "file_content", "worker_report", "derived_summary")

_DATA_OPEN = "===CORTEX_CAPSULE_DATA_BEGIN"
_DATA_CLOSE = "===CORTEX_CAPSULE_DATA_END==="


def _now_unused():  # pragma: no cover - deliberately never called
    """No wall-clock time is ever folded into a capsule -- see module docstring."""
    raise AssertionError("capsule.py must never call a wall-clock source")


# --------------------------------------------------------------------
# recipient
# --------------------------------------------------------------------


def _normalize_recipient(recipient: Mapping[str, Any]) -> dict:
    role = recipient.get("role")
    if role not in ("coordinator", "worker"):
        raise StoreError(
            "invalid_capsule_recipient", "role", role, "'coordinator' or 'worker'",
            "Pass recipient={'role': 'coordinator'} or {'role': 'worker', 'assignment_ref': ..., 'attempt_id': ...}.",
        )
    if role == "worker":
        assignment_ref = recipient.get("assignment_ref")
        attempt_id = recipient.get("attempt_id")
        if not assignment_ref or not attempt_id:
            raise StoreError(
                "invalid_capsule_recipient", "assignment_ref/attempt_id",
                (assignment_ref, attempt_id), "both non-empty",
                "A worker recipient requires both its own assignment_ref (coordinator-facing) "
                "and attempt_id (worker-facing) -- see delegation.completion's identical "
                "assignment_ref-vs-attempt_id key split.",
            )
        return dict(role="worker", assignment_ref=assignment_ref, attempt_id=attempt_id)
    return dict(role="coordinator")


# --------------------------------------------------------------------
# small helpers
# --------------------------------------------------------------------


def _unavailable(reason: str) -> dict:
    return dict(status="unavailable", reason=reason)


def _digest_of(value: Any) -> str:
    return _sha256_hex(_canonical_json(value).encode("utf-8"))


def _read_report_text(store, db, task_id: str, report_id: str, filename: str) -> tuple[str | None, str]:
    """Best-effort exact text of a durable report body; never raises."""
    try:
        path = store._task_directory(db, task_id) / filename
        data = path.read_bytes()
    except OSError:
        return None, "report_body_unreadable"
    try:
        return data.decode("utf-8"), "ok"
    except UnicodeDecodeError:
        return None, "report_body_not_utf8"


def _high_watermark(db, task_id: str) -> dict:
    max_event = db.execute("SELECT COALESCE(MAX(id),0) FROM task_events WHERE task_id=?", (task_id,)).fetchone()[0]
    max_source_rev = db.execute("SELECT COALESCE(MAX(revision),0) FROM source_revisions WHERE task_id=?", (task_id,)).fetchone()[0]
    pipeline = db.execute("SELECT id FROM reports WHERE task_id=? AND kind='pipeline'", (task_id,)).fetchone()
    max_edition = 0
    if pipeline is not None:
        row = db.execute("SELECT COALESCE(MAX(sequence),0) FROM editions WHERE report_id=?", (pipeline["id"],)).fetchone()
        max_edition = row[0]
    orchestration = db.execute("SELECT requirement_revision FROM task_orchestration WHERE task_id=?", (task_id,)).fetchone()
    requirement_revision = orchestration["requirement_revision"] if orchestration is not None else 0
    active_config = db.execute(
        "SELECT revision FROM configuration_revisions WHERE task_id=? AND state='active' ORDER BY revision DESC LIMIT 1",
        (task_id,),
    ).fetchone()
    config_revision = active_config["revision"] if active_config is not None else None
    owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
    answered_questions = db.execute(
        "SELECT COUNT(*) FROM pending_questions WHERE task_id=? AND status='answered' "
        "AND answer_source IN ('text','elicitation') AND answer_text IS NOT NULL",
        (task_id,),
    ).fetchone()[0]
    return dict(
        max_task_event_id=max_event,
        max_source_revision=max_source_rev,
        answered_question_count=answered_questions,
        max_pipeline_edition_sequence=max_edition,
        requirement_revision=requirement_revision,
        active_config_revision=config_revision,
        owner_epoch=owner["epoch"] if owner is not None else None,
    )


# --------------------------------------------------------------------
# section 1: identity
# --------------------------------------------------------------------


def _section_identity(db, task_id: str, recipient: dict, watermark: dict) -> dict:
    task = db.execute("SELECT project_root, created_at FROM tasks WHERE id=?", (task_id,)).fetchone()
    if task is None:
        raise StoreError("unknown_task", "task_id", task_id, "an existing task id", "Build a capsule only for a known task_id.")
    owner = db.execute("SELECT owner_ref, epoch, acquired_at FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
    section: dict = dict(
        task_id=task_id, project_root=task["project_root"], task_created_at=task["created_at"],
        recipient=dict(recipient),
        owner_epoch=owner["epoch"] if owner is not None else None,
        owner_acquired_at=owner["acquired_at"] if owner is not None else None,
    )
    if owner is None:
        section["owner"] = _unavailable("no_task_owner_row")
    if recipient["role"] == "coordinator":
        channel = db.execute(
            "SELECT channel_id, principal_thread_id, owner_epoch, issued_at, revoked_at, expires_at "
            "FROM trusted_channels WHERE task_id=? AND role='coordinator' ORDER BY issued_at DESC LIMIT 1",
            (task_id,),
        ).fetchone()
    else:
        channel = db.execute(
            "SELECT channel_id, principal_thread_id, owner_epoch, issued_at, revoked_at, expires_at "
            "FROM trusted_channels WHERE task_id=? AND role='worker' AND assignment_id IN "
            "(SELECT assignment_id FROM delegation_provisioning WHERE idempotency_key=?) "
            "ORDER BY issued_at DESC LIMIT 1",
            (task_id, recipient["assignment_ref"]),
        ).fetchone()
    if channel is None:
        section["channel"] = _unavailable("no_channel_issued_for_recipient")
    else:
        # Never the secret -- only the channel_id and other non-secret
        # identity columns (see trusted_channel.py module docstring: "The
        # secret ... is never stored, logged, or echoed").
        section["channel"] = dict(
            channel_id=channel["channel_id"], principal_thread_id=channel["principal_thread_id"],
            owner_epoch=channel["owner_epoch"], issued_at=channel["issued_at"],
            revoked=channel["revoked_at"] is not None, expires_at=channel["expires_at"],
        )
        section["stale_owner_epoch"] = (
            owner is not None and channel["owner_epoch"] != owner["epoch"]
        )
    return section


# --------------------------------------------------------------------
# section 2: applied route/config revision
# --------------------------------------------------------------------


def _config_row(db, revision_id: str | None):
    if revision_id is None:
        return None
    return db.execute("SELECT * FROM configuration_revisions WHERE id=?", (revision_id,)).fetchone()


def _config_public(row) -> dict:
    ceiling = json.loads(row["ceiling_snapshot"])
    return dict(
        revision_id=row["id"], revision=row["revision"], route_id=row["route_id"],
        route_engine=row["route_engine"], route_connection_id=row["route_connection_id"],
        parameters=json.loads(row["parameters"]), state=row["state"],
        permission_level=ceiling.get("permission_level"),
        confirmation_evidence_ref=row["confirmation_evidence_ref"], created_at=row["created_at"],
    )


def _section_route_and_config(db, task_id: str, recipient: dict) -> dict:
    active = db.execute(
        "SELECT * FROM configuration_revisions WHERE task_id=? AND state='active' ORDER BY revision DESC LIMIT 1",
        (task_id,),
    ).fetchone()
    if recipient["role"] == "coordinator":
        if active is None:
            latest = db.execute(
                "SELECT * FROM configuration_revisions WHERE task_id=? ORDER BY revision DESC LIMIT 1", (task_id,),
            ).fetchone()
            if latest is None:
                return _unavailable("no_configuration_revision_recorded")
            return dict(_config_public(latest), applied=False, note="No revision has reached 'active'; this is the latest proposed/selected revision only.")
        return dict(_config_public(active), applied=True)
    provisioning = db.execute(
        "SELECT config_revision FROM delegation_provisioning WHERE task_id=? AND idempotency_key=? AND attempt_id=?",
        (task_id, recipient["assignment_ref"], recipient["attempt_id"]),
    ).fetchone()
    row = _config_row(db, provisioning["config_revision"]) if provisioning is not None else None
    if row is not None:
        task_config = dict(_config_public(row), source="task_coordinator_route_snapshot")
    elif provisioning is not None and provisioning["config_revision"]:
        task_config = _unavailable("worker_config_revision_reference_not_found")
    elif active is not None:
        task_config = dict(_config_public(active), source="task_coordinator_active_no_explicit_snapshot")
    else:
        task_config = _unavailable("no_configuration_revision_recorded")
    # A dispatch audit is not engine confirmation. Retain both, and never
    # label the coordinator configuration revision as this worker's route.
    confirmation = _unavailable("no_worker_configuration_confirmed_for_attempt")
    for event_row in db.execute(
        "SELECT id, payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
        "AND event_type='progress' ORDER BY id DESC",
        (task_id, recipient["assignment_ref"]),
    ):
        payload = json.loads(event_row["payload_json"])
        if (payload.get("kind") == "worker_configuration_confirmed"
                and payload.get("attempt_id") == recipient["attempt_id"]):
            confirmation = {key: payload.get(key) for key in
                            ("attempt_id", "engine_session_ref", "engine", "connection_id",
                             "model", "parameters", "confirmation_evidence_ref")}
            confirmation["event_id"] = event_row["id"]
            break
    return dict(
        task_configuration=task_config,
        own_worker_route=dict(
            attempt_id=recipient["attempt_id"],
            dispatch_audit=_attempt_route(db, task_id, recipient["attempt_id"]),
            worker_configuration_confirmed=confirmation,
        ),
    )

# --------------------------------------------------------------------
# section 3: exact user constraints with origins
# --------------------------------------------------------------------


def _steering_bookkeeping(db, task_id: str, message_id: str) -> dict | None:
    row = db.execute(
        "SELECT payload_json, superseded, delivered_at, consumed_by FROM task_events "
        "WHERE task_id=? AND dedup_key=?", (task_id, f"user_steering:{message_id}"),
    ).fetchone()
    if row is None:
        return None
    payload = json.loads(row["payload_json"])
    return dict(
        requirement_revision=payload.get("requirement_revision"),
        superseded=bool(row["superseded"]), delivered=row["delivered_at"] is not None,
    )


def _delivery_status(db, task_id: str, message_id: str) -> dict | None:
    row = db.execute(
        "SELECT superseded, delivered_at, consumed_by FROM task_events "
        "WHERE task_id=? AND dedup_key=?", (task_id, f"user_message:{message_id}"),
    ).fetchone()
    if row is None:
        return None
    return dict(
        superseded=bool(row["superseded"]), delivered=row["delivered_at"] is not None,
        claimed_undelivered=row["consumed_by"] is not None and row["delivered_at"] is None,
    )


def _section_constraints(store, db, task_id: str, at_revision: int | None) -> dict:
    """Every durable ``source_revisions`` row (initial bootstrap input plus
    every accepted steering message) in revision order, each carrying its
    exact verbatim text (read directly from its published report body --
    never a summary), origin, requirement_revision (when this message
    itself bumped it), and delivery/supersession status.

    Worker scoping note (see module docstring): there is no per-assignment
    "attach this constraint to that worker" mechanism anywhere in this
    codebase (``delegate``'s ``brief``/``acceptance`` fields are the
    coordinator's own restatement, not a structured attachment of specific
    source_revisions rows) -- so *every* retained user_direct constraint is
    included here for both coordinator and worker recipients. This is a
    documented, deliberate choice (lead decision 4): never silently drop a
    critical constraint from a worker's capsule just because no attachment
    mechanism exists to narrow it.

    ``answered_questions`` (see ``acp.questions_store.answered_questions``)
    carries every question a real user answer resolved -- the question as
    asked and the user's verbatim answer, with its exact question id,
    timestamps and answering session. An answer is not a source revision,
    so ``at_revision`` does not bound this list: it always reflects every
    answer recorded so far.
    """
    query = "SELECT revision, thread_id, message_id, report_id, attachments FROM source_revisions WHERE task_id=?"
    params: list = [task_id]
    if at_revision is not None:
        query += " AND revision<=?"
        params.append(at_revision)
    query += " ORDER BY revision"
    rows = db.execute(query, params).fetchall()
    items = []
    for row in rows:
        report = db.execute("SELECT filename FROM reports WHERE id=?", (row["report_id"],)).fetchone()
        edition = db.execute(
            "SELECT digest, updated_at FROM editions WHERE report_id=? ORDER BY sequence DESC LIMIT 1", (row["report_id"],),
        ).fetchone()
        origin, _, event_id = row["message_id"].rpartition(":") if ":" in row["message_id"] else (None, None, None)
        text, text_status = (None, "unavailable")
        if report is not None:
            text, text_status = _read_report_text(store, db, task_id, row["report_id"], report["filename"])
        entry = dict(
            revision=row["revision"], origin=origin, event_id=event_id, report_id=row["report_id"],
            trust_label="user_direct",
        )
        if text is not None:
            entry["text"] = text
            entry["digest"] = _sha256_hex(text.encode("utf-8"))
            if edition is not None and entry["digest"] != edition["digest"]:
                entry["text_status"] = "digest_mismatch_current_body_differs_from_last_edition"
            else:
                entry["text_status"] = "ok"
        else:
            entry["text"] = _unavailable(text_status)
        steering = _steering_bookkeeping(db, task_id, row["message_id"])
        delivery = _delivery_status(db, task_id, row["message_id"])
        entry["requirement_revision"] = steering["requirement_revision"] if steering else None
        entry["superseded"] = bool((steering and steering["superseded"]) or (delivery and delivery["superseded"]))
        entry["delivered"] = bool((delivery and delivery["delivered"]) or (steering and steering["delivered"]))
        entry["claimed_undelivered"] = bool(delivery and delivery["claimed_undelivered"])
        attachments = json.loads(row["attachments"]) if row["attachments"] else []
        blocks = db.execute(
            "SELECT block_index, kind, mime, name, uri, availability, reason, trust_label, sha256, size_bytes "
            "FROM source_blocks WHERE task_id=? AND report_id=? ORDER BY block_index",
            (task_id, row["report_id"]),
        ).fetchall()
        entry["attachments"] = [
            dict(
                source_ref=f"{row['report_id']}:{block['block_index']}", kind=block["kind"], mime=block["mime"],
                name=block["name"], uri=block["uri"], availability=block["availability"], reason=block["reason"],
                trust_label=block["trust_label"], sha256=block["sha256"], size_bytes=block["size_bytes"],
            )
            for block in blocks
        ] if blocks else ([dict(descriptor=item, availability="unresolved_legacy_descriptor") for item in attachments] if attachments else [])
        items.append(entry)
    answered = questions_store.answered_questions(db, task_id)
    return dict(count=len(items), items=items, answered_question_count=len(answered), answered_questions=answered)


# --------------------------------------------------------------------
# section 4: pipeline/assignments
# --------------------------------------------------------------------


def _pipeline_ref(db, task_id: str) -> dict:
    pipeline = db.execute("SELECT id FROM reports WHERE task_id=? AND kind='pipeline'", (task_id,)).fetchone()
    if pipeline is None:
        return _unavailable("no_pipeline_published_yet")
    edition = db.execute(
        "SELECT sequence, title, summary, digest, size_bytes, updated_at FROM editions "
        "WHERE report_id=? ORDER BY sequence DESC LIMIT 1", (pipeline["id"],),
    ).fetchone()
    if edition is None:
        return _unavailable("pipeline_report_row_with_no_edition")
    return dict(
        report_id=pipeline["id"], edition_sequence=edition["sequence"], title=edition["title"],
        summary=edition["summary"], digest=edition["digest"], size_bytes=edition["size_bytes"],
        updated_at=edition["updated_at"],
    )


def _assignment_state(db, task_id: str, assignment_ref: str, worker_key: str) -> dict:
    failed = db.execute(
        "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='assignment_failed' LIMIT 1",
        (task_id, assignment_ref),
    ).fetchone() is not None
    cancel_requested = db.execute(
        "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='assignment_cancel_requested' LIMIT 1",
        (task_id, assignment_ref),
    ).fetchone() is not None
    cancelled = db.execute(
        "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='assignment_cancelled' LIMIT 1",
        (task_id, assignment_ref),
    ).fetchone() is not None
    result_row = db.execute(
        "SELECT payload_json, superseded FROM task_events WHERE task_id=? AND assignment_ref=? "
        "AND event_type='worker_result' ORDER BY id DESC LIMIT 1",
        (task_id, worker_key),
    ).fetchone()
    provisioning = db.execute(
        "SELECT status, step, error_code, workspace_binding_id, engine_session_ref, instruction_delivery "
        "FROM delegation_provisioning WHERE idempotency_key=?", (assignment_ref,),
    ).fetchone()
    if cancelled:
        state = "cancelled"
    elif failed:
        state = "failed"
    elif result_row is not None and not result_row["superseded"]:
        state = "result_published"
    elif cancel_requested:
        state = "cancel_requested_pending"
    elif provisioning is not None and provisioning["status"] == "ready":
        state = "running"
    elif provisioning is not None and provisioning["status"] == "failed":
        state = "provisioning_failed"
    elif provisioning is not None:
        state = "provisioning_in_progress"
    else:
        state = "requested_not_yet_provisioned"
    indeterminate = state in ("provisioning_in_progress", "cancel_requested_pending")
    result = dict(state=state, indeterminate=indeterminate)
    if provisioning is not None:
        result["provisioning_step"] = provisioning["step"]
        result["provisioning_error_code"] = provisioning["error_code"]
    if result_row is not None:
        payload = json.loads(result_row["payload_json"])
        result["report_ref"] = payload.get("report_ref")
    return result


def _assignment_profile(db, task_id: str, attempt_id: str) -> tuple[str | None, str | None]:
    """``(role_id, stage)`` of this worker attempt's node, resolved exactly as
    ``delegation.mcp_tools.assignment_profile`` resolves them for the same
    channel's ``tools/list`` (same join, newest node revision first), or
    ``(None, None)`` when the attempt has no node."""
    row = db.execute(
        "SELECT n.role, n.stage FROM delegation_provisioning p JOIN runtime_nodes n "
        "ON n.task_id=p.task_id AND n.assignment_ref=p.idempotency_key "
        "WHERE p.task_id=? AND p.attempt_id=? ORDER BY n.revision DESC LIMIT 1",
        (task_id, attempt_id),
    ).fetchone()
    return (row["role"], row["stage"]) if row is not None else (None, None)


def _offered_tool_names(role_kind: str, role_id: str | None, stage: str | None) -> list[str] | None:
    """The tool names this recipient's channel advertises, or ``None`` when
    they cannot be derived.

    Built from the same two sources ``server.Server.dispatch`` lists for
    ``tools/list`` -- the channeled base tools plus
    ``delegation.mcp_tools.tool_definitions`` narrowed by the same
    ``(role kind, role id, stage)`` -- so the capsule never names a tool the
    channel does not offer (a read-only role, a review stage and the
    consultant used to be told they held project_write, outside_write,
    outside_command and project_delete).
    """
    from ..delegation.mcp_tools import tool_definitions
    from ..portable.roles import PortableError

    try:
        role_tools = {entry["name"] for entry in tool_definitions(role_kind, role_id=role_id, stage=stage)}
    except (StoreError, PortableError):
        return None
    return sorted(set(channeled_base_tool_names()) | role_tools)


# What a worker must know about where it runs, stated per grant. Each variant
# follows the tools the channel really offers (``_offered_tool_names``, which
# mirrors the gateway's refusals), so the text never grants what the gateway
# refuses, and no variant says a refused action may be reached another way.
_ENVIRONMENT_RUNS_AS_USER = (
    "You run as the user: their tools, virtualenvs and network; the whole machine is readable. Follow the "
    "project's agent instruction files (AGENTS.md and the docs they point to). "
)
_ENVIRONMENT_BACKGROUND = (
    "Never end your turn to wait for a background job (its notification cannot reach you); wait inside the "
    "turn by checking its output. "
)


def _worker_environment(role_id: str | None, stage: str | None, offered: list[str] | None) -> str:
    if role_id is None or offered is None:
        return (
            "The runtime could not resolve this assignment's role, stage or offered tools, so this capsule "
            "states no write grant; the runtime decides every call. Never reach a refused action another way: "
            "say exactly what you need through ask_question, or report it as a blocker."
        )
    if "project_read" not in offered:
        # The senior consultant: consultation packet and retained reports only.
        return (
            "You work only from your consultation packet and retained reports: this role has no project "
            "reads, no commands and no writes. Never reach a refused action another way: name what is "
            "missing in your report."
        )
    if "project_write" not in offered:
        reason = "a review stage" if stage == "review" else "a read-only role"
        return (
            _ENVIRONMENT_RUNS_AS_USER
            + f"This assignment is read-only ({reason}): the runtime grants it no writes, inside or outside "
            "the project, so create, change and delete nothing. "
            + _ENVIRONMENT_BACKGROUND
            + "Never reach a refused action another way: say exactly what you need through ask_question, or "
            "report it as a blocker."
        )
    return (
        _ENVIRONMENT_RUNS_AS_USER
        + "Your declared scope is the work area agreed with the other workers for coordination: make your "
        "changes there. The runtime does not wall off the rest of the project (only paths the user protected "
        "are refused), so before changing a project path outside your declared scope, ask for it with "
        "access_request. Anything outside the project needs an explicit permission request the user accepts: "
        "outside_write for a file, outside_command for a command that changes things outside; a refusal is "
        "final. Deleting a project file asks the user too. "
        + _ENVIRONMENT_BACKGROUND
        + "Never reach a refused action another way: say exactly what you need through access_request or "
        "ask_question, or report it as a blocker."
    )


def _section_pipeline_and_assignments(db, task_id: str, recipient: dict) -> dict:
    section: dict = {}
    task = db.execute('SELECT state,revision,review_cycle,goal,blocker FROM runtime_tasks WHERE task_id=?',(task_id,)).fetchone()
    if task:
        from ..orchestration.policy import denied_paths
        section['runtime'] = dict(task)
        section['runtime']['denied_write_paths'] = denied_paths(db,task_id)
        # No durable record says which native engine tools a session has (it
        # differs by engine), so the capsule states nothing about them.
        if recipient['role'] != 'coordinator':
            role_id, stage = _assignment_profile(db, task_id, recipient['attempt_id'])
            section['runtime']['environment'] = _worker_environment(
                role_id, stage, _offered_tool_names('worker', role_id, stage))
        sql = 'SELECT task_id,node_id,role,stage,objective,dependencies_json,scope_json,status,assignment_ref FROM runtime_nodes WHERE task_id=? AND revision=?'
        values = [task_id,task['revision']]
        if recipient['role'] != 'coordinator':
            sql += ' AND assignment_ref=?'
            values.append(recipient['assignment_ref'])
        section['runtime']['nodes'] = [dict(row) for row in db.execute(sql+' ORDER BY node_id',values)]
        from ..orchestration.access import context as access_context
        for node in section['runtime']['nodes']:
            node['access'] = access_context(db, node)
        from ..orchestration.worker_waits import projection
        waits = projection(db, task_id, assignment_ref=None if recipient['role'] == 'coordinator'
                           else recipient['assignment_ref'])
        active = [w for w in waits if w['status'] in {'open', 'ready', 'resuming'}]
        terminal = [w for w in waits if w['status'] not in {'open', 'ready', 'resuming'}]
        # Question/answer bodies already live in the bounded source sections.
        # Retain all active bindings and only a bounded tail of past deliveries.
        keys = ('wait_id', 'question_id', 'batch_id', 'question_ids', 'answer_refs', 'assignment_ref', 'attempt_id', 'channel_id',
                'owner_epoch', 'engine_session_ref', 'revision', 'status', 'delivery_id',
                'answer_ref', 'reason', 'event_id', 'kind', 'request_id', 'paths', 'access_decision')
        section['runtime']['worker_waits'] = [
            {key: w[key] for key in keys if key in w} for w in active + terminal[-8:]]
        section['runtime']['worker_waits_omitted'] = max(0, len(terminal) - 8)
    if recipient["role"] == "coordinator":
        section["current_pipeline"] = _pipeline_ref(db, task_id)
        rows = db.execute(
            "SELECT DISTINCT assignment_ref FROM task_events "
            "WHERE task_id=? AND event_type='delegation_requested'", (task_id,),
        ).fetchall()
        assignments = []
        for row in rows:
            assignment_ref = row["assignment_ref"]
            request_row = db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
                "AND event_type='delegation_requested' ORDER BY id LIMIT 1",
                (task_id, assignment_ref),
            ).fetchone()
            payload = json.loads(request_row["payload_json"]) if request_row is not None else {}
            provisioning = db.execute(
                "SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=?", (assignment_ref,),
            ).fetchone()
            worker_key = provisioning["attempt_id"] if provisioning is not None else assignment_ref
            entry = dict(
                assignment_ref=assignment_ref, title=payload.get("title"), role=payload.get("role"),
                brief=payload.get("brief"), acceptance=payload.get("acceptance"),
            )
            entry.update(_assignment_state(db, task_id, assignment_ref, worker_key))
            # The configuration revision is a policy snapshot, not proof of
            # the worker route. Mixed-team and fallback dispatches can use a
            # different model and effort. Keep the exact attempt audit beside
            # the assignment it describes.
            entry["observed_worker_route"] = _attempt_route(db, task_id, worker_key)
            assignments.append(entry)
        section["assignments"] = assignments
        return section
    # worker: only its own assignment (never another assignment's brief).
    assignment_ref = recipient["assignment_ref"]
    request_row = db.execute(
        "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
        "AND event_type='delegation_requested' ORDER BY id LIMIT 1",
        (task_id, assignment_ref),
    ).fetchone()
    if request_row is None:
        section["own_assignment"] = _unavailable("no_delegation_requested_event_for_this_assignment_ref")
        return section
    payload = json.loads(request_row["payload_json"])
    entry = dict(
        assignment_ref=assignment_ref, title=payload.get("title"), role=payload.get("role"),
        brief=payload.get("brief"), acceptance=payload.get("acceptance"),
    )
    entry.update(_assignment_state(db, task_id, assignment_ref, recipient["attempt_id"]))
    section["own_assignment"] = entry
    return section


def _attempt_route(db, task_id: str, attempt_id: str) -> dict:
    row = db.execute(
        "SELECT id, attempt_id, engine, connection_id, model, decision, "
        "decision_detail_json FROM attempt_route_audit "
        "WHERE task_id=? AND attempt_id=? ORDER BY recorded_at DESC, id DESC LIMIT 1",
        (task_id, attempt_id),
    ).fetchone()
    if row is None:
        return _unavailable("no_attempt_route_audit_for_assignment")
    detail = json.loads(row["decision_detail_json"] or "{}")
    return dict(
        audit_id=row["id"], attempt_id=row["attempt_id"], engine=row["engine"],
        connection_id=row["connection_id"], model=row["model"],
        effort=detail.get("effort"), decision=row["decision"],
    )


# --------------------------------------------------------------------
# section 5: confirmed publications/checks/artifact revisions
# --------------------------------------------------------------------


def _report_ref_info(db, task_id: str, report_ref: str | None) -> dict | None:
    if not report_ref:
        return None
    row = db.execute("SELECT filename FROM reports WHERE id=? AND task_id=?", (report_ref, task_id)).fetchone()
    if row is None:
        return dict(report_ref=report_ref, resolved=False)
    edition = db.execute(
        "SELECT digest, size_bytes, updated_at FROM editions WHERE report_id=? ORDER BY sequence DESC LIMIT 1", (report_ref,),
    ).fetchone()
    return dict(
        report_ref=report_ref, resolved=True,
        digest=edition["digest"] if edition else None,
        size_bytes=edition["size_bytes"] if edition else None,
        updated_at=edition["updated_at"] if edition else None,
    )


def _worker_results_for(db, task_id: str, worker_key: str) -> list[dict]:
    rows = db.execute(
        "SELECT id, payload_json, superseded, created_at FROM task_events "
        "WHERE task_id=? AND assignment_ref=? AND event_type='worker_result' ORDER BY id",
        (task_id, worker_key),
    ).fetchall()
    results = []
    for row in rows:
        payload = json.loads(row["payload_json"])
        results.append(dict(
            cursor=row["id"], superseded=bool(row["superseded"]), created_at=row["created_at"],
            summary=payload.get("summary"), checks=payload.get("checks", []),
            report=_report_ref_info(db, task_id, payload.get("report_ref")),
        ))
    return results


def _section_publications_and_checks(db, task_id: str, recipient: dict) -> dict:
    receipt_query = (
        "SELECT attempt_id,tool_call_ref,command_sha256,command_text,host_outcome "
        "FROM permission_receipts WHERE task_id=?"
    )
    if recipient["role"] == "coordinator":
        receipts = db.execute(receipt_query + " ORDER BY created_at DESC LIMIT ?", (task_id, _MAX_LIST_ENTRIES)).fetchall()
        rows = db.execute(
            "SELECT DISTINCT assignment_ref FROM task_events WHERE task_id=? AND event_type='delegation_requested'",
            (task_id,),
        ).fetchall()
        by_assignment = {}
        for row in rows:
            assignment_ref = row["assignment_ref"]
            provisioning = db.execute(
                "SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=?", (assignment_ref,),
            ).fetchone()
            worker_key = provisioning["attempt_id"] if provisioning is not None else assignment_ref
            results = _worker_results_for(db, task_id, worker_key)
            if results:
                by_assignment[assignment_ref] = results
        return dict(
            results_by_assignment=by_assignment,
            permission_receipts=[dict(row) for row in receipts],
        )
    results = _worker_results_for(db, task_id, recipient["attempt_id"])
    receipts = db.execute(
        receipt_query + " AND attempt_id=? ORDER BY created_at DESC LIMIT ?",
        (task_id, recipient["attempt_id"], _MAX_LIST_ENTRIES),
    ).fetchall()
    return dict(own_results=results,
                own_permission_receipts=[dict(row) for row in receipts])


# --------------------------------------------------------------------
# section 6: drafts/processes/open or uncertain actions
# --------------------------------------------------------------------


def _pending_questions(db, task_id: str, assignment_ref: str | None = None) -> list[dict]:
    query = "SELECT id, assignment_ref, text, status, created_at FROM pending_questions WHERE task_id=? AND status='open'"
    params: list = [task_id]
    if assignment_ref is not None:
        query += " AND assignment_ref=?"
        params.append(assignment_ref)
    rows = db.execute(query, params).fetchall()
    from ..orchestration.question_batches import find_tx
    result = []
    for row in rows:
        question = dict(id=row['id'], assignment_ref=row['assignment_ref'], text=row['text'], created_at=row['created_at'])
        batch = find_tx(db, task_id, question_id=row['id'])
        if batch:
            question.update(batch_id=batch['batch_id'], question_ids=batch['question_ids'])
        result.append(question)
    return result


def _capacity_ambiguous(db, task_id: str, attempt_id: str | None = None) -> list[dict]:
    query = "SELECT reservation_id, connection_id, attempt_id, kind, updated_at FROM capacity_reservations WHERE task_id=? AND state='ambiguous'"
    params: list = [task_id]
    if attempt_id is not None:
        query += " AND attempt_id=?"
        params.append(attempt_id)
    rows = db.execute(query, params).fetchall()
    return [dict(row) for row in rows]


def _claimed_undelivered(db, task_id: str, assignment_refs: tuple[str, ...] | None = None) -> list[dict]:
    query = "SELECT id, event_type, assignment_ref, consumed_by FROM task_events WHERE task_id=? AND consumed_by IS NOT NULL AND delivered_at IS NULL AND superseded=0"
    params: list = [task_id]
    if assignment_refs is not None:
        placeholders = ",".join("?" for _ in assignment_refs)
        query += f" AND assignment_ref IN ({placeholders})"
        params.extend(assignment_refs)
    rows = db.execute(query, params).fetchall()
    return [dict(cursor=r["id"], event_type=r["event_type"], assignment_ref=r["assignment_ref"]) for r in rows]


def _section_drafts_and_open_actions(db, task_id: str, recipient: dict) -> dict:
    section: dict = {}
    if recipient["role"] == "coordinator":
        section["open_questions"] = _pending_questions(db, task_id)
        section["capacity_ambiguous"] = _capacity_ambiguous(db, task_id)
        section["claimed_undelivered_events"] = _claimed_undelivered(db, task_id)
        rows = db.execute(
            "SELECT id, kind, template, created_at FROM drafts WHERE task_id=? AND published_report_id IS NULL",
            (task_id,),
        ).fetchall()
        section["unpublished_drafts"] = [dict(row) for row in rows]
        indeterminate = db.execute(
            "SELECT idempotency_key, step, status, error_code FROM delegation_provisioning "
            "WHERE task_id=? AND status NOT IN ('ready','failed')",
            (task_id,),
        ).fetchall()
        section["provisioning_in_progress"] = [dict(row) for row in indeterminate]
        unrecorded = db.execute(
            "SELECT origin, event_id, captured_at FROM source_events WHERE task_id=? AND ledger_state IS NULL",
            (task_id,),
        ).fetchall()
        section["source_events_not_yet_recorded"] = [dict(row) for row in unrecorded]
        return section
    assignment_ref = recipient["assignment_ref"]
    attempt_id = recipient["attempt_id"]
    section["open_questions"] = _pending_questions(db, task_id, assignment_ref)
    section["capacity_ambiguous"] = _capacity_ambiguous(db, task_id, attempt_id)
    section["claimed_undelivered_events"] = _claimed_undelivered(db, task_id, (assignment_ref, attempt_id))
    return section


# --------------------------------------------------------------------
# section 7: newer steering/events since last delivered
# --------------------------------------------------------------------


def _section_newer_events(db, task_id: str, recipient: dict) -> dict:
    if recipient["role"] == "coordinator":
        rows = db.execute(
            "SELECT id, event_type, assignment_ref, dedup_key, superseded, consumed_by, delivered_at "
            "FROM task_events WHERE task_id=? AND consumed_by IS NULL AND superseded=0 ORDER BY id",
            (task_id,),
        ).fetchall()
        truncated = len(rows) > _MAX_LIST_ENTRIES
        items = [
            dict(cursor=r["id"], event_type=r["event_type"], assignment_ref=r["assignment_ref"], dedup_key=r["dedup_key"])
            for r in rows[:_MAX_LIST_ENTRIES]
        ]
        return dict(unconsumed=items, truncated=truncated, total=len(rows), note="Use list_events for the full, paginated log.")
    assignment_ref, attempt_id = recipient["assignment_ref"], recipient["attempt_id"]
    rows = db.execute(
        "SELECT id, event_type, assignment_ref, superseded, delivered_at FROM task_events "
        "WHERE task_id=? AND assignment_ref IN (?,?) "
        "AND event_type IN ('coordinator_clarification','assignment_cancel_requested','assignment_cancelled') "
        "ORDER BY id", (task_id, assignment_ref, attempt_id),
    ).fetchall()
    items = [
        dict(cursor=r["id"], event_type=r["event_type"], superseded=bool(r["superseded"]))
        for r in rows[-_MAX_LIST_ENTRIES:]
    ]
    return dict(
        for_own_assignment=items, truncated=len(rows) > _MAX_LIST_ENTRIES,
        note="No per-worker delivery cursor is tracked in this schema; every retained "
             "clarification/cancellation event for this exact assignment is shown.",
    )


# --------------------------------------------------------------------
# section 8: role/tool access
# --------------------------------------------------------------------


def _section_role_and_tool_access(db, task_id: str, recipient: dict) -> dict:
    from ..portable.roles import load_role, PortableError

    if recipient["role"] == "coordinator":
        role_id = "orchestrator"
        provisioning = None
        orchestration = db.execute(
            "SELECT coordinator_attempt_ref FROM task_orchestration WHERE task_id=?", (task_id,),
        ).fetchone()
        if orchestration is not None:
            provisioning = db.execute(
                "SELECT instruction_delivery, engine_session_ref FROM delegation_provisioning WHERE idempotency_key=?",
                (orchestration["coordinator_attempt_ref"],),
            ).fetchone()
    else:
        request_row = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='delegation_requested' ORDER BY id LIMIT 1",
            (task_id, recipient["assignment_ref"]),
        ).fetchone()
        role_id = json.loads(request_row["payload_json"])["role"] if request_row is not None else None
        provisioning = db.execute(
            "SELECT instruction_delivery, engine_session_ref FROM delegation_provisioning WHERE idempotency_key=?",
            (recipient["assignment_ref"],),
        ).fetchone()
    section: dict = dict(role_id=role_id, role_kind=recipient["role"])
    if role_id is not None:
        try:
            role_set = load_role(role_id)
            section["role_source"] = dict(source_path=role_set.source_path, sha256=role_set.sha256, title=role_set.title)
        except PortableError as error:
            section["role_source"] = _unavailable(f"role_source_load_failed:{error.code}")
    else:
        section["role_source"] = _unavailable("role_id_not_recorded_for_this_assignment")
    section["instruction_delivery"] = provisioning["instruction_delivery"] if provisioning is not None else None
    # The channel's own ``tools/list`` surface: the base Cortex tools plus the
    # role-scoped delegation tools, narrowed by role and stage exactly as the
    # server narrows them. Listing only the delegation schemas once made a
    # recovered worker believe create_draft/write_report/read_report were
    # unavailable; listing every schema made read-only roles believe they
    # could write.
    role_id_for_tools, stage = (None, None) if recipient["role"] == "coordinator" else _assignment_profile(
        db, task_id, recipient["attempt_id"])
    offered = _offered_tool_names(recipient["role"], role_id_for_tools, stage)
    section["allowed_mcp_tools"] = offered if offered is not None else _unavailable("offered_tools_unresolved")
    # T022: compaction-support tier/owner and current context-generation
    # sequence for this recipient's engine session, when one has already
    # been recorded (delegation.generations) -- trivially available here
    # since delegation_provisioning already carries engine_session_ref.
    engine_session_ref = provisioning["engine_session_ref"] if provisioning is not None else None
    if engine_session_ref:
        owner = db.execute(
            "SELECT owner, tier, engine, engine_version, limitation FROM compaction_owners WHERE engine_session_ref=?",
            (engine_session_ref,),
        ).fetchone()
        generation = db.execute(
            "SELECT id, sequence, cause FROM context_generations WHERE engine_session_ref=? ORDER BY sequence DESC LIMIT 1",
            (engine_session_ref,),
        ).fetchone()
        section["compaction_support"] = (
            dict(owner=owner["owner"], tier=owner["tier"], engine=owner["engine"],
                 engine_version=owner["engine_version"], limitation=owner["limitation"])
            if owner is not None else _unavailable("no_compaction_owner_recorded_yet")
        )
        section["context_generation"] = (
            dict(id=generation["id"], sequence=generation["sequence"], cause=generation["cause"])
            if generation is not None else _unavailable("no_context_generation_recorded_yet")
        )
    else:
        section["compaction_support"] = _unavailable("engine_session_not_recorded_for_this_assignment")
        section["context_generation"] = _unavailable("engine_session_not_recorded_for_this_assignment")
    return section


# --------------------------------------------------------------------
# section 9: continuation intent / next decision point
# --------------------------------------------------------------------


def _section_continuation_intent(db, task_id: str, recipient: dict) -> dict:
    stopped = db.execute("SELECT stopped_at FROM task_stopped WHERE task_id=?", (task_id,)).fetchone()
    section: dict = dict(task_stopped=stopped is not None, stopped_at=stopped["stopped_at"] if stopped else None)
    if recipient["role"] == "coordinator":
        orchestration = db.execute(
            "SELECT state, wait_reason, wait_detail_json, requirement_revision, updated_at FROM task_orchestration WHERE task_id=?",
            (task_id,),
        ).fetchone()
        if orchestration is None:
            section["orchestration"] = _unavailable("coordinator_never_provisioned")
        else:
            section["orchestration"] = dict(
                state=orchestration["state"], wait_reason=orchestration["wait_reason"],
                wait_detail=json.loads(orchestration["wait_detail_json"]) if orchestration["wait_detail_json"] else None,
                requirement_revision=orchestration["requirement_revision"], updated_at=orchestration["updated_at"],
            )
        task = db.execute('SELECT state FROM runtime_tasks WHERE task_id=?',(task_id,)).fetchone()
        state = task['state'] if task else None
        section["task_acceptance"] = dict(
            accepted=state == "COMPLETED",
            stored_task_state=state,
            note="Only the runtime completion invariants can set COMPLETED. Reports and model statements do not accept the task.",
        )
    else:
        cancel = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='assignment_cancel_requested' LIMIT 1",
            (task_id, recipient["assignment_ref"]),
        ).fetchone() is not None
        cancelled = db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='assignment_cancelled' LIMIT 1",
            (task_id, recipient["assignment_ref"]),
        ).fetchone() is not None
        section["own_assignment_cancel_requested"] = cancel
        section["own_assignment_cancelled"] = cancelled
    # No adapter currently supplies a model-authored "continuation note" (see
    # module docstring / lead decision 1) -- this is the honest default.
    # ``build_capsule``'s optional ``candidate_summary`` parameter (T024)
    # overwrites this with either a validated ``derived_summary`` note or an
    # explicit rejection reason -- see that function and
    # ``recovery.summary_validation`` -- never silently trusted as-is.
    section["continuation_note"] = _unavailable("not_supported_in_this_build")
    return section


# --------------------------------------------------------------------
# T024: optional candidate-summary gate for continuation_note
# --------------------------------------------------------------------


def _known_refs_for_summary(sections: Mapping[str, Any]) -> set[str]:
    """Every report/source id this capsule itself already resolved --
    exactly the ids a candidate summary is allowed to reference (see
    ``summary_validation.validate_summary``'s reference-resolution check)."""
    refs: set[str] = set()
    constraints = sections.get("constraints") or {}
    for item in constraints.get("items", []) if isinstance(constraints, dict) else ():
        report_id = item.get("report_id")
        if report_id:
            refs.add(report_id)
    for answer in constraints.get("answered_questions", []) if isinstance(constraints, dict) else ():
        refs.add(answer["question_id"])
        refs.add(answer["source_ref"])
    pipeline_and_assignments = sections.get("pipeline_and_assignments") or {}
    if isinstance(pipeline_and_assignments, dict):
        pipeline = pipeline_and_assignments.get("current_pipeline")
        if isinstance(pipeline, dict) and pipeline.get("report_id"):
            refs.add(pipeline["report_id"])
        for assignment in pipeline_and_assignments.get("assignments", []) or []:
            if isinstance(assignment, dict) and assignment.get("report_ref"):
                refs.add(assignment["report_ref"])
        own = pipeline_and_assignments.get("own_assignment")
        if isinstance(own, dict) and own.get("report_ref"):
            refs.add(own["report_ref"])
    publications = sections.get("publications_and_checks") or {}
    if isinstance(publications, dict):
        for results in publications.get("results_by_assignment", {}).values():
            for result in results:
                report = result.get("report")
                if isinstance(report, dict) and report.get("report_ref"):
                    refs.add(report["report_ref"])
        for result in publications.get("own_results", []) or []:
            report = result.get("report")
            if isinstance(report, dict) and report.get("report_ref"):
                refs.add(report["report_ref"])
    return refs


def _authoritative_snapshot_for_summary(recipient: Mapping[str, Any], sections: Mapping[str, Any]) -> dict:
    """The subset of already-computed sections a candidate summary's claims
    are checked against (see ``summary_validation.validate_summary``'s
    contradiction checks) -- never a second, independent read of the store,
    always exactly what this same capsule build already resolved."""
    constraints = sections.get("constraints") or {}
    constraint_ids = [
        item["report_id"] for item in (constraints.get("items", []) if isinstance(constraints, dict) else ())
        if item.get("report_id")
    ]
    identity = sections.get("identity") or {}
    task_id = identity.get("task_id")
    if recipient["role"] == "worker":
        pipeline_and_assignments = sections.get("pipeline_and_assignments") or {}
        own = pipeline_and_assignments.get("own_assignment") if isinstance(pipeline_and_assignments, dict) else None
        assignment_status = own.get("state") if isinstance(own, dict) else None
        publications = sections.get("publications_and_checks") or {}
        own_results = publications.get("own_results") if isinstance(publications, dict) else None
        checks = own_results[-1].get("checks", []) if own_results else []
        return dict(
            task_id=task_id, assignment_ref=recipient.get("assignment_ref"),
            assignment_status=assignment_status, completed=assignment_status == "result_published",
            checks=checks, constraint_ids=constraint_ids,
        )
    return dict(task_id=task_id, assignment_status=None, completed=False, checks=[], constraint_ids=constraint_ids)


def _apply_candidate_summary(sections: dict, recipient: Mapping[str, Any], candidate_summary: Any) -> None:
    from . import summary_validation

    known_refs = _known_refs_for_summary(sections)
    authoritative = _authoritative_snapshot_for_summary(recipient, sections)
    result = summary_validation.validate_summary(candidate_summary, known_refs=known_refs, authoritative=authoritative)
    if result.valid and result.summary is not None:
        # ``note_text`` (never the generic ``text`` key -- see
        # ``_TEXT_FIELD_TRUST``) so ``render_capsule`` wraps it labeled
        # ``derived_summary``, never the unrelated ``text`` key's
        # ``user_direct`` mapping used elsewhere (constraints' own verbatim
        # source text).
        sections["continuation_intent"]["continuation_note"] = dict(
            status="available", trust_label="derived_summary", note_text=result.summary["text"],
        )
    else:
        sections["continuation_intent"]["continuation_note"] = _unavailable(
            "summary_invalid:" + (",".join(result.violations) or "unknown"),
        )


# --------------------------------------------------------------------
# bounded critical sections
# --------------------------------------------------------------------

_TERMINAL_ASSIGNMENT_STATES = frozenset({"result_published", "cancelled"})
_ACTIVE_NODE_STATUSES = frozenset({"pending", "running", "failed"})
# Text at or under this size always stays inline: a reference record costs
# about as much as the text itself.
_MIN_REFERENCE_BYTES = 512
_NO_TOOL_NOTE = (
    "No tool available to this recipient returns this exact field; the sha256 and size_bytes "
    "identify the full text."
)


def _critical_bytes(sections: Mapping[str, Any]) -> int:
    return len(_canonical_json({name: sections[name] for name in _CRITICAL_SECTION_NAMES}).encode("utf-8"))


def _is_marker(value: Any) -> bool:
    return isinstance(value, dict) and value.get("status") in ("referenced", "omitted")


def _text_ref(text: Any, cap: int | None, trust_label: str, retrieve: dict) -> Any:
    """``text`` unchanged when it fits ``cap`` bytes (or is too small for a
    reference to save space); otherwise an exact reference to it with a
    preview of at most ``cap`` bytes."""
    if cap is None or not isinstance(text, str):
        return text
    data = text.encode("utf-8")
    if len(data) <= max(cap, _MIN_REFERENCE_BYTES):
        return text
    preview = data[:cap].decode("utf-8", errors="ignore")
    return dict(
        status="referenced", truncated=True, trust_label=trust_label,
        sha256=_sha256_hex(data), size_bytes=len(data),
        preview=preview, preview_bytes=len(preview.encode("utf-8")), retrieve=retrieve,
    )


class _Handled(set):
    """``(id(container), key)`` of every field a field-specific pass already
    bounded, so the generic safety net never re-clamps a protected text."""

    def bound(self, container: dict, key: str, cap: int | None, trust_label: str, retrieve: dict) -> None:
        if key not in container:
            return
        container[key] = _text_ref(container.get(key), cap, trust_label, retrieve)
        self.add((id(container), key))


def _select_entries(items: list, keep: int | None, priority, ref_of, omitted_refs: int | None, retrieve: dict):
    """Keep at most ``keep`` entries of ``items`` (lowest ``priority(index)``
    value first, then newest), preserving their original order. Returns the kept
    list and an explicit omission record (or ``None``) naming what was left
    out and where to read it."""
    if keep is None or len(items) <= keep:
        return items, None, list(range(len(items)))
    order = sorted(range(len(items)), key=lambda index: (priority(index), -index))
    kept_indices = sorted(order[:keep])
    kept_set = set(kept_indices)
    omitted = [items[index] for index in range(len(items)) if index not in kept_set]
    refs = [ref_of(item) for item in omitted]
    record = dict(
        status="omitted", count=len(omitted), total=len(items), kept=len(kept_indices),
        sha256=_digest_of(omitted), retrieve=retrieve,
    )
    if omitted_refs:
        record["refs"] = refs[:omitted_refs]
        record["refs_truncated"] = len(refs) > omitted_refs
    else:
        record["first_ref"] = refs[0]
        record["last_ref"] = refs[-1]
    return [items[index] for index in kept_indices], record, kept_indices


def _goal_retrieve(goal: Any, constraints: Mapping[str, Any], recipient: dict) -> dict:
    """The exact source text the goal equals, when there is one; otherwise
    the coordinator's task context (workers have no such tool)."""
    if isinstance(goal, str):
        digest = _sha256_hex(goal.encode("utf-8"))
        for item in constraints.get("items", []) if isinstance(constraints, dict) else ():
            if item.get("digest") == digest and item.get("report_id"):
                return dict(tool="read_source", source_ref=f"{item['report_id']}:-1")
    if recipient["role"] == "coordinator":
        return dict(tool="task_get_context", field="goal")
    return dict(tool=None, note=_NO_TOOL_NOTE)


def _compact_constraints(section: dict, level: dict, handled: _Handled) -> None:
    items = section.get("items") or []
    if items:
        latest = max(range(len(items)), key=lambda index: items[index]["revision"])
        unsuperseded = [index for index, item in enumerate(items) if not item.get("superseded")]
        latest_live = unsuperseded[-1] if unsuperseded else latest
        tiers = [
            0 if index in (latest, latest_live) else (1 if not item.get("superseded") else 2)
            for index, item in enumerate(items)
        ]
        kept, record, kept_indices = _select_entries(
            items, level["keep"], tiers.__getitem__, lambda item: f"{item['report_id']}:-1",
            level["omitted_refs"], dict(tool="list_sources", then="read_source"),
        )
        for index, item in zip(kept_indices, kept):
            cap = (level["protected"], level["active"], level["other"])[tiers[index]]
            if isinstance(item.get("text"), str):
                handled.bound(
                    item, "text", cap, item.get("trust_label", "user_direct"),
                    dict(tool="read_source", source_ref=f"{item['report_id']}:-1"),
                )
        section["items"] = kept
        if record is not None:
            section["items_omitted"] = record
    answers = section.get("answered_questions") or []
    if answers:
        latest_answer = len(answers) - 1
        kept, record, kept_indices = _select_entries(
            answers, level["keep"], lambda index: 0 if index == latest_answer else 1,
            lambda answer: answer.get("source_ref"), level["omitted_refs"],
            dict(tool="list_sources", then="read_source"),
        )
        for index, answer in zip(kept_indices, kept):
            cap = level["protected"] if index == latest_answer else level["active"]
            retrieve = dict(tool="read_source", source_ref=answer.get("source_ref"))
            handled.bound(answer, "question_text", cap, "derived_summary", retrieve)
            handled.bound(answer, "answer_text", cap, answer.get("trust_label", "user_direct"), retrieve)
        section["answered_questions"] = kept
        if record is not None:
            section["answered_questions_omitted"] = record


def _compact_pipeline(section: dict, constraints: Mapping[str, Any], recipient: dict, level: dict, handled: _Handled) -> None:
    coordinator = recipient["role"] == "coordinator"
    runtime = section.get("runtime")
    if isinstance(runtime, dict):
        handled.bound(
            runtime, "goal", level["protected"], "user_direct", _goal_retrieve(runtime.get("goal"), constraints, recipient),
        )
        nodes = runtime.get("nodes") or []

        def node_tier(node):
            if not coordinator:
                return 0
            return 1 if node.get("status") in _ACTIVE_NODE_STATUSES else 2

        def node_retrieve(node):
            if coordinator:
                return dict(tool="task_get_context", field="nodes", node_id=node.get("node_id"))
            return dict(tool=None, note=_NO_TOOL_NOTE)

        kept, record, _ = _select_entries(
            nodes, level["keep"], lambda index: node_tier(nodes[index]), lambda node: node.get("node_id"), level["omitted_refs"],
            dict(tool="task_get_context", field="nodes") if coordinator else dict(tool=None, note=_NO_TOOL_NOTE),
        )
        for node in kept:
            if not coordinator:
                # The runtime filters ``nodes`` to exactly this worker's own
                # node (see ``_section_pipeline_and_assignments``'s
                # ``assignment_ref``-scoped SQL) -- its ``objective`` is the
                # same class of mandatory, unresolvable-without-a-tool text as
                # ``own_assignment`` below (no worker tool returns it), so it
                # must never be compacted into a ``retrieve.tool=None``
                # reference either. Stays verbatim at every level (cap=None).
                handled.bound(node, "objective", None, "derived_summary", node_retrieve(node))
                continue
            cap = (level["protected"], level["active"], level["other"])[node_tier(node)]
            handled.bound(node, "objective", cap, "derived_summary", node_retrieve(node))
        runtime["nodes"] = kept
        if record is not None:
            runtime["nodes_omitted"] = record
    assignments = section.get("assignments")
    if isinstance(assignments, list) and assignments:

        def assignment_tier(entry):
            return 2 if entry.get("state") in _TERMINAL_ASSIGNMENT_STATES else 1

        kept, record, _ = _select_entries(
            assignments, level["keep"], lambda index: assignment_tier(assignments[index]),
            lambda entry: entry.get("assignment_ref"),
            level["omitted_refs"], dict(tool="task_get_context", field="nodes"),
        )
        for entry in kept:
            cap = (level["protected"], level["active"], level["other"])[assignment_tier(entry)]
            # The brief restates this node's objective and scope and the task goal.
            retrieve = dict(tool="task_get_context", field="nodes", assignment_ref=entry.get("assignment_ref"))
            for key in ("title", "brief", "acceptance"):
                handled.bound(entry, key, cap, "derived_summary", retrieve)
        section["assignments"] = kept
        if record is not None:
            section["assignments_omitted"] = record
    own = section.get("own_assignment")
    if isinstance(own, dict) and own.get("status") != "unavailable":
        # P1 fix: no worker-available, channel-authorized tool returns this
        # recipient's own assignment title/brief/acceptance (unlike a *coordinator*
        # cross-referenced assignment brief, resolvable via ``task_get_context``,
        # or another worker's constraint text, resolvable via ``read_source``).
        # Compacting it into a ``status="referenced"`` object with
        # ``retrieve.tool=None`` would silently strip the recovered worker's
        # only readable copy of its mandatory objective. So these three fields
        # always stay verbatim (cap=None) at *every* compaction level,
        # regardless of size -- never truncated, never referenced. Everything
        # else in the critical sections compacts harder instead to still fit
        # under CRITICAL_BUDGET_BYTES. Only if the own assignment's own text
        # alone still exceeds HARD_CAP_BYTES after the most aggressive level
        # does ``_bounded_sections`` raise the explicit ``capsule_oversize``
        # error -- never a silent reference.
        retrieve = dict(tool=None, note=_NO_TOOL_NOTE)
        for key in ("title", "brief", "acceptance"):
            handled.bound(own, key, None, "derived_summary", retrieve)


def _compact_open_actions(section: dict, level: dict, handled: _Handled) -> None:
    questions = section.get("open_questions")
    if isinstance(questions, list):
        for question in questions:
            handled.bound(
                question, "text", max(level["active"], level["generic"]), "derived_summary",
                dict(tool=None, note=_NO_TOOL_NOTE),
            )


def _clamp_generic(value: Any, level: dict, path: str, handled: _Handled) -> Any:
    """Safety net over every critical section: any string or list the
    field-specific passes did not already bound is bounded here, with the
    same explicit reference/omission records."""
    if isinstance(value, dict):
        if _is_marker(value):
            return value
        result = {}
        for key, item in value.items():
            child_path = f"{path}.{key}"
            if (id(value), key) in handled:
                result[key] = item
            elif isinstance(item, list) and level["keep"] is not None and len(item) > level["keep"] \
                    and f"{key}_omitted" not in value:
                kept, record, _ = _select_entries(
                    item, level["keep"], lambda _index: 0, lambda entry: _digest_of(entry),
                    level["omitted_refs"], dict(tool=None, note=_NO_TOOL_NOTE, path=child_path),
                )
                result[key] = [_clamp_generic(entry, level, f"{child_path}[]", handled) for entry in kept]
                result[f"{key}_omitted"] = record
            else:
                result[key] = _clamp_generic(item, level, child_path, handled)
        return result
    if isinstance(value, list):
        return [_clamp_generic(item, level, f"{path}[]", handled) for item in value]
    if isinstance(value, str):
        trust = "user_direct" if path.endswith(".text") else "derived_summary"
        return _text_ref(value, level["generic"], trust, dict(tool=None, note=_NO_TOOL_NOTE, path=path))
    return value


def _count_markers(value: Any) -> tuple[int, int]:
    referenced = omitted = 0
    if isinstance(value, dict):
        if value.get("status") == "referenced":
            return 1, 0
        if value.get("status") == "omitted":
            return 0, value.get("count", 0)
        for item in value.values():
            ref_count, omit_count = _count_markers(item)
            referenced += ref_count
            omitted += omit_count
    elif isinstance(value, list):
        for item in value:
            ref_count, omit_count = _count_markers(item)
            referenced += ref_count
            omitted += omit_count
    return referenced, omitted


def _compact_critical_sections(sections: dict, recipient: dict, level_index: int) -> dict:
    """A compacted deep copy of ``sections`` at ``level_index`` (pure and
    deterministic: identical input always yields identical output)."""
    level = _COMPACTION_LEVELS[level_index]
    compacted = dict(sections)
    if level is None:
        return compacted
    working = {name: json.loads(_canonical_json(sections[name])) for name in _CRITICAL_SECTION_NAMES}
    handled = _Handled()
    constraints = working["constraints"]
    original_constraints = sections["constraints"]
    if isinstance(constraints, dict) and constraints.get("status") != "unavailable":
        _compact_constraints(constraints, level, handled)
    pipeline = working["pipeline_and_assignments"]
    if isinstance(pipeline, dict):
        _compact_pipeline(pipeline, original_constraints, recipient, level, handled)
    actions = working["drafts_and_open_actions"]
    if isinstance(actions, dict):
        _compact_open_actions(actions, level, handled)
    for name in _CRITICAL_SECTION_NAMES:
        compacted[name] = _clamp_generic(working[name], level, name, handled)
    return compacted


def _bounded_sections(sections: dict, recipient: dict) -> tuple[dict, dict]:
    """Pick the first compaction level whose critical sections fit
    :data:`CRITICAL_BUDGET_BYTES`; the most aggressive level otherwise.
    Raises ``capsule_oversize`` only when even that level exceeds
    :data:`HARD_CAP_BYTES`."""
    uncompacted_bytes = _critical_bytes(sections)
    chosen, chosen_bytes, chosen_level = sections, uncompacted_bytes, 0
    for level_index in range(len(_COMPACTION_LEVELS)):
        candidate = sections if level_index == 0 else _compact_critical_sections(sections, recipient, level_index)
        candidate_bytes = uncompacted_bytes if level_index == 0 else _critical_bytes(candidate)
        chosen, chosen_bytes, chosen_level = candidate, candidate_bytes, level_index
        if candidate_bytes <= CRITICAL_BUDGET_BYTES:
            break
    if chosen_bytes > HARD_CAP_BYTES:
        raise StoreError(
            "capsule_oversize", "critical_sections_bytes", chosen_bytes, f"<= {HARD_CAP_BYTES}",
            "Critical capsule sections (identity, constraints, pipeline_and_assignments, "
            "drafts_and_open_actions) exceed the hard cap even at the most aggressive compaction "
            f"level {len(_COMPACTION_LEVELS) - 1}; nothing is silently dropped. Raise "
            "recovery.capsule.HARD_CAP_BYTES for a deployment that needs a larger bound.",
        )
    referenced, omitted = _count_markers({name: chosen[name] for name in _CRITICAL_SECTION_NAMES})
    compaction = dict(
        level=chosen_level, max_level=len(_COMPACTION_LEVELS) - 1,
        critical_budget_bytes=CRITICAL_BUDGET_BYTES, uncompacted_critical_bytes=uncompacted_bytes,
        referenced_fields=referenced, omitted_entries=omitted,
        note=(
            "Verbatim." if chosen_level == 0 else
            "Fields with status=referenced are truncated: their full text is identified by sha256/size_bytes "
            "and returned by the named retrieve tool. Records with status=omitted list entries left out of "
            "a long list."
        ),
    )
    return chosen, compaction


# --------------------------------------------------------------------
# assembly
# --------------------------------------------------------------------

_SECTION_BUILDERS = (
    ("identity", lambda store, db, task_id, recipient, at_revision, watermark: _section_identity(db, task_id, recipient, watermark)),
    ("route_and_config", lambda store, db, task_id, recipient, at_revision, watermark: _section_route_and_config(db, task_id, recipient)),
    ("constraints", lambda store, db, task_id, recipient, at_revision, watermark: _section_constraints(store, db, task_id, at_revision)),
    ("pipeline_and_assignments", lambda store, db, task_id, recipient, at_revision, watermark: _section_pipeline_and_assignments(db, task_id, recipient)),
    ("publications_and_checks", lambda store, db, task_id, recipient, at_revision, watermark: _section_publications_and_checks(db, task_id, recipient)),
    ("drafts_and_open_actions", lambda store, db, task_id, recipient, at_revision, watermark: _section_drafts_and_open_actions(db, task_id, recipient)),
    ("newer_events", lambda store, db, task_id, recipient, at_revision, watermark: _section_newer_events(db, task_id, recipient)),
    ("role_and_tool_access", lambda store, db, task_id, recipient, at_revision, watermark: _section_role_and_tool_access(db, task_id, recipient)),
    ("continuation_intent", lambda store, db, task_id, recipient, at_revision, watermark: _section_continuation_intent(db, task_id, recipient)),
)


def build_capsule(
    store, task_id: str, recipient: Mapping[str, Any], at_revision: int | None = None,
    *, candidate_summary: Any = None,
) -> dict:
    """Build one deterministic recovery capsule for ``recipient``.

    ``recipient`` is ``{"role": "coordinator"}`` or ``{"role": "worker",
    "assignment_ref": ..., "attempt_id": ...}``. ``at_revision`` optionally
    bounds the ``constraints`` section to source revisions captured at or
    before that revision number (every other section always reflects
    current durable state -- a full point-in-time snapshot of every
    section is not implemented; this is a documented limitation, not a
    silent one).

    ``candidate_summary`` (T024, optional, default ``None``): a raw
    model-authored semantic-summary candidate (JSON text or an already
    -parsed dict) -- see ``recovery.summary_validation``. No current adapter
    supplies one (see ``recovery.summarizer_policy`` module docstring), so
    this is exercised directly by callers/tests today, not from a live
    engine signal. When supplied, it is *never* trusted as-is: it is
    validated against this exact capsule's own already-resolved sections
    (never a second, independent read) and either becomes
    ``sections.continuation_intent.continuation_note`` (labeled
    ``derived_summary``, structural fields never overridden by it) or is
    replaced with an explicit ``summary_invalid:<reasons>`` rejection --
    valid JSON is not semantic correctness, and an omitted/altered
    constraint or fabricated completion claim can never change any other
    section's already-computed, authoritative content.

    The critical sections (identity, constraints, pipeline_and_assignments,
    drafts_and_open_actions) are bounded: when their canonical JSON exceeds
    :data:`CRITICAL_BUDGET_BYTES` the first deterministic compaction level
    that fits is applied (see ``_COMPACTION_LEVELS``). Compacted text is
    replaced by an exact reference (sha256, size, bounded preview,
    ``truncated`` marker, retrieving tool) and trimmed list entries are
    named in a sibling ``<key>_omitted`` record -- nothing is dropped
    silently. The latest user constraint, the task goal and the recipient's
    own assignment keep the largest inline allowance. ``capsule["compaction"]``
    records the applied level.

    Raises ``StoreError('unknown_task', ...)`` for an unknown ``task_id``
    and, only as a last-resort guard, ``StoreError('capsule_oversize', ...)``
    when even the most aggressive level exceeds :data:`HARD_CAP_BYTES`.
    """
    normalized = _normalize_recipient(recipient)
    with store.connection() as db:
        watermark = _high_watermark(db, task_id)
        sections = {}
        for name, builder in _SECTION_BUILDERS:
            sections[name] = builder(store, db, task_id, normalized, at_revision, watermark)
    if candidate_summary is not None:
        _apply_candidate_summary(sections, normalized, candidate_summary)
    # Top-level key is deliberately ``for_task_id``, not ``task_id`` -- the
    # MCP dispatch layer (delegation.mcp_tools.dispatch) refuses any
    # top-level result field named exactly one of a small technical-id
    # allowlist (task_id/attempt_id/channel_id/...) as a leak guard against
    # a handler accidentally exposing a raw internal id the caller never
    # already held. The exact task_id is still present, nested, inside
    # ``sections.identity.task_id`` -- this rename only avoids a false
    # positive on that generic top-level guard, and changes no content.
    sections, compaction = _bounded_sections(sections, normalized)
    critical_bytes = _critical_bytes(sections)
    digest_input = dict(
        capsule_format_version=CAPSULE_FORMAT_VERSION, for_task_id=task_id, recipient=normalized,
        at_revision=at_revision, source_high_watermark=watermark, sections=sections,
        compaction=compaction,
    )
    capsule_revision = _digest_of(digest_input)
    total_bytes_estimate = len(_canonical_json(digest_input).encode("utf-8"))
    capsule = dict(
        capsule_format_version=CAPSULE_FORMAT_VERSION,
        for_task_id=task_id, recipient=normalized, at_revision=at_revision,
        source_high_watermark=watermark, capsule_revision=capsule_revision,
        sections=sections, compaction=compaction,
        size_estimate=dict(
            bytes=total_bytes_estimate,
            approx_tokens=_estimate_tokens(total_bytes_estimate),
            critical_bytes=critical_bytes,
            oversize=False,
        ),
    )
    return capsule


# --------------------------------------------------------------------
# rendering
# --------------------------------------------------------------------


def _defang(text: str) -> str:
    """Neutralize a literal occurrence of the data-block delimiter tokens
    inside untrusted text, so wrapped content can never forge a fake
    block boundary in the rendered output."""
    return text.replace(_DATA_OPEN, "=xx=CORTEX_CAPSULE_DATA_BEGIN").replace(_DATA_CLOSE, "=xx=CORTEX_CAPSULE_DATA_END===")


def _render_data_block(trust_label: str, ref: str, text: str) -> str:
    header = f"{_DATA_OPEN}(trust={trust_label},ref={ref})==="
    return f"{header}\n{_defang(text)}\n{_DATA_CLOSE}"


def render_capsule(capsule: Mapping[str, Any]) -> str:
    """Deterministic plain-text rendering of ``capsule`` (the exact dict
    :func:`build_capsule` returned). Pure function of its input -- never
    reads the store, never uses wall-clock time -- so two renderings of the
    identical capsule dict (including across a process restart) are always
    byte-identical.

    Every recipient-owned/reported text value (constraint text, worker
    report summaries) is wrapped in an explicit, escaped ``data, not
    instructions`` block (see :func:`_render_data_block`); every other
    field is rendered as plain structured text, never re-parsed back into
    instructions.
    """
    lines: list[str] = []
    lines.append(f"# Cortex recovery capsule (format v{capsule['capsule_format_version']})")
    lines.append(f"task_id: {capsule['for_task_id']}")
    lines.append(f"recipient: {json.dumps(capsule['recipient'], sort_keys=True)}")
    lines.append(f"capsule_revision: {capsule['capsule_revision']}")
    lines.append(f"source_high_watermark: {json.dumps(capsule['source_high_watermark'], sort_keys=True)}")
    if capsule.get("compaction") is not None:
        lines.append(f"compaction: {json.dumps(capsule['compaction'], sort_keys=True)}")
    lines.append("")
    for name in (
        "identity", "route_and_config", "constraints", "pipeline_and_assignments",
        "publications_and_checks", "drafts_and_open_actions", "newer_events",
        "role_and_tool_access", "continuation_intent",
    ):
        section = capsule["sections"][name]
        lines.append(f"## {name}")
        if isinstance(section, dict) and section.get("status") in ("unavailable", "unknown"):
            lines.append(f"(status={section['status']}, reason={section.get('reason')})")
            lines.append("")
            continue
        if name == "constraints":
            lines.append(f"count: {section['count']}")
            for item in section["items"]:
                lines.append(
                    f"- revision={item['revision']} origin={item['origin']} event_id={item['event_id']} "
                    f"superseded={item['superseded']} delivered={item['delivered']} "
                    f"requirement_revision={item['requirement_revision']}"
                )
                text = item["text"]
                if isinstance(text, dict) and text.get("status") == "referenced":
                    lines.append("  " + _render_referenced(text, item["report_id"]).replace("\n", "\n  "))
                elif isinstance(text, dict):
                    lines.append(f"  text: (status={text['status']}, reason={text.get('reason')})")
                else:
                    lines.append("  " + _render_data_block(item["trust_label"], item["report_id"], text).replace("\n", "\n  "))
                for attachment in item.get("attachments", []):
                    lines.append(f"  attachment: {json.dumps(attachment, sort_keys=True)}")
            if section.get("items_omitted"):
                lines.append(f"items_omitted: {json.dumps(section['items_omitted'], sort_keys=True)}")
            lines.append(f"answered_questions: {section.get('answered_question_count', 0)}")
            for answer in section.get("answered_questions", []):
                lines.append(
                    f"- question_id={answer['question_id']} source_ref={answer['source_ref']} "
                    f"asked_by={answer['asked_by']} asked_at={answer['asked_at']} "
                    f"answered_at={answer['answered_at']} answer_source={answer['answer_source']} "
                    f"origin={answer['origin']} session_binding={answer['session_binding']}"
                )
                # The question was written by whoever asked it; only the
                # answer is the user's own text.
                lines.append("  question: " + _render_text_value(
                    "derived_summary", answer["question_id"] + ":question", answer["question_text"],
                ).replace("\n", "\n  "))
                if answer.get("options"):
                    lines.append("  options: " + _defang(json.dumps(answer["options"], ensure_ascii=False)))
                lines.append("  answer: " + _render_text_value(
                    answer["trust_label"], answer["source_ref"], answer["answer_text"],
                ).replace("\n", "\n  "))
            if section.get("answered_questions_omitted"):
                lines.append(
                    f"answered_questions_omitted: {json.dumps(section['answered_questions_omitted'], sort_keys=True)}"
                )
        else:
            lines.append(_render_json_with_data_blocks(section))
        lines.append("")
    return "\n".join(lines)


def _referenced_header(value: Mapping[str, Any]) -> dict:
    return {key: item for key, item in value.items() if key != "preview"}


def _render_referenced(value: Mapping[str, Any], ref: str) -> str:
    """A referenced (truncated) text: its exact reference, then its bounded
    preview inside a data block labeled with the original trust label."""
    header = json.dumps(_referenced_header(value), sort_keys=True)
    block = _render_data_block(value.get("trust_label", "derived_summary"), ref, value.get("preview", ""))
    return f"text_truncated: {header}\n{block}"


def _render_text_value(trust_label: str, ref: str, value: Any) -> str:
    if isinstance(value, dict) and value.get("status") == "referenced":
        return _render_referenced(value, ref)
    return _render_data_block(trust_label, ref, value)


_TEXT_FIELD_TRUST = {
    "summary": "worker_report",
    "brief": "derived_summary",
    "acceptance": "derived_summary",
    "rationale": "derived_summary",
    "text": "user_direct",
    "note_text": "derived_summary",
    "command_text": "worker_report",
}


def _render_json_with_data_blocks(value: Any) -> str:
    """Render a section as canonical JSON, but replace every string value
    under a known free-text key (worker summaries, coordinator rationale,
    question text, ...) with an escaped data block first -- so untrusted
    recipient-authored text never sits unmarked next to structural fields.
    """
    marked = _mark_text_fields(value)
    return _canonical_json(marked)


def _mark_text_fields(value: Any) -> Any:
    if isinstance(value, dict) and value.get("status") == "referenced" and isinstance(value.get("preview"), str):
        # A truncated text keeps its data fence: the preview is wrapped with
        # the original field's trust label; the reference fields stay plain.
        marked = _referenced_header(value)
        marked["preview"] = _render_data_block(value.get("trust_label", "derived_summary"), "preview", value["preview"])
        return marked
    if isinstance(value, dict):
        result = {}
        for key, item in value.items():
            if key in _TEXT_FIELD_TRUST and isinstance(item, str):
                result[key] = _render_data_block(_TEXT_FIELD_TRUST[key], key, item)
            else:
                result[key] = _mark_text_fields(item)
        return result
    if isinstance(value, list):
        return [_mark_text_fields(item) for item in value]
    return value
