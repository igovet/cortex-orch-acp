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
top-level ``capsule_format_version``, ``task_id``, ``recipient``,
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

# A capsule whose *critical* sections alone exceed this many bytes refuses
# outright (``capsule_oversize``) rather than ever dropping/truncating a
# critical section (identity, constraints, the recipient's own scope, and
# open/uncertain actions -- see ``_CRITICAL_SECTION_NAMES``).
HARD_CAP_BYTES = 400_000

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
    return dict(
        max_task_event_id=max_event,
        max_source_revision=max_source_rev,
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
        "SELECT config_revision FROM delegation_provisioning WHERE idempotency_key=?",
        (recipient["assignment_ref"],),
    ).fetchone()
    if provisioning is not None and provisioning["config_revision"]:
        row = _config_row(db, provisioning["config_revision"])
        if row is not None:
            return dict(_config_public(row), applied=True, source="own_provisioning_snapshot")
        return _unavailable("worker_config_revision_reference_not_found")
    if active is not None:
        return dict(_config_public(active), applied=True, source="inherited_coordinator_active_no_explicit_snapshot")
    return _unavailable("no_configuration_revision_recorded")


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
    return dict(count=len(items), items=items)


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


def _section_pipeline_and_assignments(db, task_id: str, recipient: dict) -> dict:
    section: dict = {}
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


def _coordinator_decisions_for(db, task_id: str, assignment_ref: str | None = None) -> list[dict]:
    query = "SELECT id, payload_json, superseded, created_at FROM task_events WHERE task_id=? AND event_type='coordinator_decision'"
    params: list = [task_id]
    if assignment_ref is not None:
        query += " AND assignment_ref=?"
        params.append(assignment_ref)
    query += " ORDER BY id"
    rows = db.execute(query, params).fetchall()
    decisions = []
    for row in rows:
        payload = json.loads(row["payload_json"])
        decisions.append(dict(
            cursor=row["id"], superseded=bool(row["superseded"]), created_at=row["created_at"],
            assignment_ref=payload.get("assignment_ref"), decision=payload.get("decision"),
            rationale=payload.get("rationale"),
        ))
    return decisions


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
            coordinator_decisions=_coordinator_decisions_for(db, task_id)[-_MAX_LIST_ENTRIES:],
            permission_receipts=[dict(row) for row in receipts],
        )
    results = _worker_results_for(db, task_id, recipient["attempt_id"])
    decisions = _coordinator_decisions_for(db, task_id, recipient["assignment_ref"])
    receipts = db.execute(
        receipt_query + " AND attempt_id=? ORDER BY created_at DESC LIMIT ?",
        (task_id, recipient["attempt_id"], _MAX_LIST_ENTRIES),
    ).fetchall()
    return dict(own_results=results, coordinator_decisions_for_own_assignment=decisions,
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
    return [dict(id=r["id"], assignment_ref=r["assignment_ref"], text=r["text"], created_at=r["created_at"]) for r in rows]


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
    from ..delegation import contracts as delegation_contracts
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
    try:
        # A worker's trusted MCP server advertises the base Cortex tools and
        # the role-scoped delegation tools together.  Keep this derived list
        # identical to the server's ``tools/list`` surface; listing only the
        # delegation schemas made a recovered worker believe that
        # create_draft/write_report/read_report and the other base operations
        # were unavailable even though the channel exposed them.
        base_tools = set(channeled_base_tool_names())
        role_tools = set(delegation_contracts.schemas_for(recipient["role"]))
        section["allowed_mcp_tools"] = sorted(base_tools | role_tools)
    except StoreError:
        section["allowed_mcp_tools"] = _unavailable("unknown_role_kind")
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
        decisions = _coordinator_decisions_for(db, task_id)
        section["last_coordinator_decision"] = decisions[-1] if decisions else _unavailable("no_coordinator_decision_recorded")
        task_accepts = [d for d in decisions if d["assignment_ref"] == "task" and d["decision"] == "accept" and not d["superseded"]]
        state = orchestration["state"] if orchestration is not None else None
        section["task_acceptance"] = dict(
            accepted=state == "accepted" and bool(task_accepts),
            stored_task_state=state,
            task_accept_decision_cursor=task_accepts[-1]["cursor"] if task_accepts else None,
            note="An assignment decision or report prose does not accept the task; only a durable task-level decision with accepted task state does.",
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

    Raises ``StoreError('unknown_task', ...)`` for an unknown ``task_id``
    and ``StoreError('capsule_oversize', ...)`` when the critical sections
    alone (identity, constraints, pipeline_and_assignments,
    drafts_and_open_actions) exceed :data:`HARD_CAP_BYTES` -- never
    silently drops a critical constraint to fit.
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
    digest_input = dict(
        capsule_format_version=CAPSULE_FORMAT_VERSION, for_task_id=task_id, recipient=normalized,
        at_revision=at_revision, source_high_watermark=watermark, sections=sections,
    )
    critical_bytes = len(_canonical_json({name: sections[name] for name in _CRITICAL_SECTION_NAMES}).encode("utf-8"))
    if critical_bytes > HARD_CAP_BYTES:
        raise StoreError(
            "capsule_oversize", "critical_sections_bytes", critical_bytes, f"<= {HARD_CAP_BYTES}",
            "Critical capsule sections (identity, constraints, pipeline_and_assignments, "
            "drafts_and_open_actions) alone exceed the hard cap; they are never truncated. "
            "Reduce retained source/assignment volume for this task before requesting a capsule, "
            "or raise recovery.capsule.HARD_CAP_BYTES for a deployment that needs a larger bound.",
        )
    capsule_revision = _digest_of(digest_input)
    total_bytes_estimate = len(_canonical_json(digest_input).encode("utf-8"))
    capsule = dict(
        capsule_format_version=CAPSULE_FORMAT_VERSION,
        for_task_id=task_id, recipient=normalized, at_revision=at_revision,
        source_high_watermark=watermark, capsule_revision=capsule_revision,
        sections=sections,
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
                if isinstance(text, dict):
                    lines.append(f"  text: (status={text['status']}, reason={text.get('reason')})")
                else:
                    lines.append("  " + _render_data_block(item["trust_label"], item["report_id"], text).replace("\n", "\n  "))
                for attachment in item.get("attachments", []):
                    lines.append(f"  attachment: {json.dumps(attachment, sort_keys=True)}")
        else:
            lines.append(_render_json_with_data_blocks(section))
        lines.append("")
    return "\n".join(lines)


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
