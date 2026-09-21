"""T017 host-success completion gating.

A prompt may end as a successful host final response only when a
task-level ``decide(accept)`` is grounded in durable evidence, never on
model-asserted text alone. :func:`gate` is the single source of truth for
this check; it is called from two places that must never disagree:

- ``delegation.mcp_tools._coordinator_decide`` -- refuses to even *record*
  a task-level accept decision when the gate visibly fails at call time
  (``StoreError`` with a concrete correction).
- ``acp.orchestrating_runtime.OrchestratingRuntime._recheck_accept``
  (called from ``_check_terminal_decision``, on every continuation, right
  before a turn could return a host-success ``END_TURN``) -- re-runs this
  same gate against current durable state before honoring an
  already-recorded task-level accept decision, on top of two of its own
  additional checks this module has no visibility into: (a) the decision
  is discarded outright (never even reaching the gate) if any
  ``delegation_requested``/meaningful event committed *after* it -- for
  example the coordinator calling ``decide(accept)`` and then
  ``delegate(...)`` in the very same turn; (b) the decision is refused
  while any of this runtime's own in-flight worker futures for the task
  are still running/queued (``OrchestratingRuntime._running``), which the
  durable log alone cannot see. Any refusal records a durable
  ``acceptance_refused`` progress event with concrete reasons.

Conditions (see T017 task card / lead design notes):

1. No assignment is still running/provisioning: every distinct
   ``assignment_ref`` this task has ever delegated (a non-superseded
   ``delegation_requested`` event) must be terminal -- a retained
   (non-superseded) ``worker_result`` (the task-level accept, per
   ``delegation.contracts``'s ``decide`` description, "covers every
   retained assignment result"; there is no separate required
   per-assignment accept step), or an explicit cancellation. A retained
   ``assignment_failed`` is never an acceptable completion result; it blocks
   acceptance until the work is superseded by a successful attempt or
   explicitly abandoned by the user.
2. No unconsumed *meaningful* event remains for the task (see
   ``delegation.events.MEANINGFUL_EVENT_TYPES``) -- an un-reacted-to worker
   result/question/failure/cancellation/user message.
3. Every terminal-by-``worker_result`` assignment's ``report_ref`` resolves
   to a ``reports`` row for this task *and* the report file exists on disk
   -- a claimed success with no stored evidence never gates a host-success
   completion.
4. If the runtime has published its bootstrap pipeline edition, that edition
   must have been superseded by a coordinator-authored current edition.  The
   bootstrap is an honest receipt that the task opened; it is not the model's
   work graph, evidence, or acceptance rationale.  This check does not inspect
   or synthesize model content: it only enforces the durable author/edition
   boundary.
5. A retained worker result cannot contain an explicit ``failed`` or
   ``not_run``/``unverified`` check, or a check whose exact attempt observation is
   ``contradicted``.  These are hard evidence of failed or incomplete
   verification and remain blockers; the gate does not require that every
   assignment provide a check.

Completion is decided from the durable ``task_events`` log, published
reports, and current pipeline edition. The work graph also persists
assignment/attempt metadata, but a graph status alone is not proof that a
worker result was published or accepted.
"""
from __future__ import annotations

import json
import hashlib
import re

from . import events as delegation_events
from . import revision as delegation_revision
from ..engine.permissions import is_rm_command


# Assignment-level acceptance is intentionally narrower than task completion:
# an informational/review assignment may publish no structured checks.  A
# user-facing rendered/UI requirement is different, however.  It must be
# represented by a structured publish_result check backed by an observed tool
# call; prose in the report or coordinator rationale is never a substitute.
_RENDERED_REQUIREMENT_RE = re.compile(
    r"\b(?:render(?:ed|ing)?|visual(?:ly)?|browser|responsive|focus\s+ring|keyboard\s+focus)\b",
    re.IGNORECASE,
)
_RENDERED_CHECK_RE = re.compile(
    r"\b(?:render(?:ed|ing)?|visual(?:ly)?|browser|responsive|focus|page)\b",
    re.IGNORECASE,
)


def _assignment_requires_rendered_check(db, task_id: str, assignment_ref: str) -> bool:
    """Return whether the original assignment explicitly asks for UI proof.

    This is deliberately a local, conservative classification of the
    coordinator's durable assignment text.  It does not inspect report prose,
    and it does not turn every assignment into a test stage.
    """
    row = db.execute(
        "SELECT payload_json FROM task_events "
        "WHERE task_id=? AND assignment_ref=? AND event_type='delegation_requested' "
        "AND superseded=0 ORDER BY id DESC LIMIT 1",
        (task_id, assignment_ref),
    ).fetchone()
    if row is None:
        alias = db.execute(
            "SELECT idempotency_key FROM delegation_provisioning "
            "WHERE task_id=? AND attempt_id=? LIMIT 1",
            (task_id, assignment_ref),
        ).fetchone()
        if alias is not None:
            row = db.execute(
                "SELECT payload_json FROM task_events "
                "WHERE task_id=? AND assignment_ref=? AND event_type='delegation_requested' "
                "AND superseded=0 ORDER BY id DESC LIMIT 1",
                (task_id, alias["idempotency_key"]),
            ).fetchone()
    if row is None:
        return False
    try:
        payload = json.loads(row["payload_json"])
    except (TypeError, ValueError):
        return False
    text = " ".join(str(payload.get(field) or "") for field in ("title", "brief", "acceptance"))
    return bool(_RENDERED_REQUIREMENT_RE.search(text))


def assignment_acceptance_evidence(store, task_id: str, assignment_ref: str) -> list[str]:
    """Return hard blockers for accepting one UI/rendering assignment.

    The result is empty for assignments with no explicit rendered requirement,
    preserving review and informational work.  For a rendered requirement,
    the exact worker result must contain a matching passed check whose
    ``evidence`` was resolved to an observed engine tool call.  Empty,
    unverified, failed, not-run, claimed-only, or unrelated checks block.
    """
    with store.connection() as db:
        if not _assignment_requires_rendered_check(db, task_id, assignment_ref):
            return []
        provision = db.execute(
            "SELECT attempt_id FROM delegation_provisioning "
            "WHERE task_id=? AND idempotency_key=? LIMIT 1",
            (task_id, assignment_ref),
        ).fetchone()
        if provision is None:
            provision = db.execute(
                "SELECT attempt_id FROM delegation_provisioning "
                "WHERE task_id=? AND attempt_id=? LIMIT 1",
                (task_id, assignment_ref),
            ).fetchone()
        worker_key = provision["attempt_id"] if provision is not None else assignment_ref
        result = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='worker_result' AND superseded=0 ORDER BY id DESC LIMIT 1",
            (task_id, worker_key),
        ).fetchone()
    if result is None:
        return [f"assignment {assignment_ref} has no retained worker_result for its rendered requirement."]
    try:
        payload = json.loads(result["payload_json"])
    except (TypeError, ValueError):
        return [f"assignment {assignment_ref} has malformed worker_result evidence for its rendered requirement."]
    checks = payload.get("checks")
    if not isinstance(checks, list) or not checks:
        # A worker may have reported ``checks=[]`` when the runtime's
        # candidate-review path performed the verification after publication.
        # That path is valid only when the exact staged candidate is still
        # current, is sourced from this assignment's exact attempt, and has
        # a separately assessed passing check.  Never use report prose or a
        # check from an unrelated candidate as a substitute.
        if _candidate_check_satisfies_assignment(store, task_id, assignment_ref, worker_key):
            return []
        return [
            f"assignment {assignment_ref} requires a structured rendered check; "
            "publish_result checks are missing (report prose cannot satisfy it)."
        ]
    matching = [
        check for check in checks
        if isinstance(check, dict) and _RENDERED_CHECK_RE.search(str(check.get("name") or ""))
    ]
    if not matching:
        if _candidate_check_satisfies_assignment(store, task_id, assignment_ref, worker_key):
            return []
        return [
            f"assignment {assignment_ref} requires a structured rendered check; "
            "the published checks contain no matching rendered/UI check."
        ]
    # A later exact candidate check may replace an earlier worker-side
    # ``unverified``/``not_run`` observation (for example, a browser sandbox
    # was unavailable during publication).  An explicit ``failed`` result is
    # different: it is a contradictory observation and cannot be silently
    # overwritten by another check.
    if any(check.get("outcome") == "failed" for check in matching):
        return [
            f"assignment {assignment_ref} rendered check is failed; "
            "resolve the failed verification before accepting."
        ]
    if any(check.get("outcome") != "passed" or check.get("evidence") != "observed" for check in matching):
        if _candidate_check_satisfies_assignment(store, task_id, assignment_ref, worker_key):
            return []
        return [
            f"assignment {assignment_ref} rendered check is not observed as passed; "
            "report prose or a claimed check cannot establish acceptance."
        ]
    return []


def _candidate_check_satisfies_assignment(
    store, task_id: str, assignment_ref: str, worker_key: str,
) -> bool:
    """Resolve a current, assessed candidate check for one exact attempt.

    Candidate checks are a separate durable verification lane.  They may
    satisfy a rendered requirement only when the candidate contains this
    assignment's exact attempt, its content digest still matches the staged
    tree, the check completed successfully without truncation, its requirement
    scope is still current, and a non-superseded Cortex assessment explicitly
    recorded ``pass``.  This deliberately does not inspect coordinator prose.
    """
    from .candidates import _digest_manifest, _manifest
    from .check_requests import _row as check_row
    from .candidates import _ensure_schema as ensure_candidate_schema
    from .mcp_tools import _check_requirement_currency

    ensure_candidate_schema(store)
    with store.connection() as db:
        # A candidate check is semantic evidence only when the assignment's
        # durable requirement binding says what it verifies.  A successful
        # static command on a candidate is never allowed to certify a
        # rendered/browser requirement merely because its argv happens to
        # contain a suggestive word.
        requested = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? "
            "AND assignment_ref=? AND event_type='delegation_requested' "
            "AND superseded=0 ORDER BY id DESC LIMIT 1",
            (task_id, assignment_ref),
        ).fetchone()
        if requested is None:
            return False
        try:
            requested_payload = json.loads(requested["payload_json"])
        except (TypeError, ValueError):
            return False
        plan_ref = requested_payload.get("plan_assignment_ref")
        # Normal planned delegations carry the plan node explicitly.  Older
        # live sessions can legitimately have a delegation recorded before
        # the coordinator creates/binds its graph node; in that case the
        # candidate itself is the durable bridge.  Resolve those bindings by
        # exact candidate_ref below, never by report prose or by a loose
        # task-wide requirement union.
        binding_sources = []
        if isinstance(plan_ref, str) and plan_ref:
            planned = db.execute(
                "SELECT requirement_bindings_json FROM work_assignments "
                "WHERE task_id=? AND assignment_id=?",
                (task_id, plan_ref),
            ).fetchone()
            if planned is None:
                return False
            try:
                bindings = json.loads(planned["requirement_bindings_json"] or "{}")
            except (TypeError, ValueError):
                return False
            if not isinstance(bindings, dict) or not bindings:
                return False
            binding_sources.append((None, bindings))
        else:
            # Candidate-bound plan nodes are the only fallback.  The
            # candidate id is checked again against the staged row in the
            # loop below, so a different candidate cannot borrow its scope.
            for planned in db.execute(
                "SELECT candidate_ref,requirement_bindings_json FROM work_assignments "
                "WHERE task_id=? AND candidate_ref IS NOT NULL",
                (task_id,),
            ).fetchall():
                candidate_ref = planned["candidate_ref"]
                if not isinstance(candidate_ref, str) or not candidate_ref.startswith("candidate:"):
                    continue
                try:
                    bindings = json.loads(planned["requirement_bindings_json"] or "{}")
                except (TypeError, ValueError):
                    continue
                if isinstance(bindings, dict) and bindings:
                    binding_sources.append((candidate_ref, bindings))
            if not binding_sources:
                return False

        def rendered_bindings(bindings):
            rendered = set()
            for requirement_id, binding in bindings.items():
                revision = binding.get("revision") if isinstance(binding, dict) else binding
                requirement = db.execute(
                    "SELECT r.current_revision,v.statement,v.criteria_json "
                    "FROM work_requirements r JOIN work_requirement_revisions v "
                    "ON v.requirement_id=r.requirement_id AND v.revision=r.current_revision "
                    "WHERE r.task_id=? AND r.requirement_id=?",
                    (task_id, requirement_id),
                ).fetchone()
                if requirement is None or requirement["current_revision"] != revision:
                    continue
                try:
                    criteria = json.loads(requirement["criteria_json"] or "[]")
                except (TypeError, ValueError):
                    criteria = []
                requirement_text = " ".join(
                    [str(requirement["statement"] or ""), *(str(item) for item in criteria)]
                )
                if _RENDERED_REQUIREMENT_RE.search(requirement_text):
                    rendered.add(requirement_id)
            return rendered
        candidates = db.execute(
            "SELECT * FROM candidate_staging WHERE task_id=? AND status='ready' "
            "AND cleaned_up_at IS NULL ORDER BY created_at DESC",
            (task_id,),
        ).fetchall()
        for candidate_row in candidates:
            try:
                attempt_ids = json.loads(candidate_row["attempt_ids_json"] or "[]")
            except (TypeError, ValueError):
                continue
            if worker_key not in attempt_ids:
                continue
            candidate_id = candidate_row["candidate_id"]
            candidate_digest = candidate_row["content_digest"]
            stage_path = candidate_row["stage_path"]
            try:
                if not candidate_digest or _digest_manifest(_manifest(stage_path)) != candidate_digest:
                    continue
            except Exception:  # noqa: BLE001 - uncertainty is a failed gate
                continue
            candidate_sources = [
                bindings for candidate_ref, bindings in binding_sources
                if candidate_ref is None or candidate_ref == f"candidate:{candidate_id}"
            ]
            rendered_binding_ids = set()
            for bindings in candidate_sources:
                rendered_binding_ids.update(rendered_bindings(bindings))
            if not rendered_binding_ids:
                continue
            requests = db.execute(
                "SELECT * FROM check_requests WHERE task_id=? AND candidate_id=? "
                "AND candidate_digest=? AND status='completed' ORDER BY finished_at DESC",
                (task_id, candidate_id, candidate_digest),
            ).fetchall()
            for request_row in requests:
                request = check_row(request_row)
                observed = (request.get("result") or {}).get("observed_result") or {}
                if (observed.get("status") != "completed"
                        or observed.get("exit_code") != 0
                        or observed.get("output_truncated")):
                    continue
                applicable, _ = _check_requirement_currency(db, request)
                if not applicable:
                    continue
                if not rendered_binding_ids.issubset(request.get("requirement_bindings", {})):
                    continue
                assessment_rows = db.execute(
                    "SELECT payload_json FROM task_events "
                    "WHERE task_id=? AND event_type='progress' AND superseded=0 "
                    "AND payload_json LIKE ? ORDER BY id DESC",
                    (task_id, f'%\"kind\": \"candidate_check_assessed\"%'),
                ).fetchall()
                for assessment_row in assessment_rows:
                    try:
                        assessment = json.loads(assessment_row["payload_json"])
                    except (TypeError, ValueError):
                        continue
                    recorded_evidence_id = (request.get("result") or {}).get("evidence_id")
                    if (assessment.get("check_ref") == request["check_id"]
                            and assessment.get("candidate_ref") == candidate_id
                            and assessment.get("candidate_digest") == candidate_digest
                            # The assessment must point at the exact durable
                            # observation recorded by this request.  Matching
                            # only check/candidate IDs would let a copied or
                            # replayed pass assessment certify a different
                            # observation.
                            and isinstance(recorded_evidence_id, str)
                            and assessment.get("evidence_ref") == recorded_evidence_id
                            and assessment.get("assessment") == "pass"):
                        return True
    return False


def _task_directory(store, db, task_id: str):
    from ..store import private_directory, project_task_directory

    row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
    if row is None:
        return None
    cortex_root = project_task_directory(row["project_root"])
    return private_directory(cortex_root / task_id)


def _unintegrated_worker_changes(db, assignment_ref: str, attempt_id: str,
                                 workspace_binding_id: str | None = None) -> tuple[set[str], str | None]:
    """Paths this worker changed in its own copy that are not in the
    project, as the same comparison ``delegation.integration`` itself uses
    (its recorded base manifest against a fresh scan of its workspace).

    The second return value is an uncertainty reason. Missing, cleaned or
    unreadable evidence is never converted into an empty change set: the
    acceptance gate must refuse until the exact result is reconciled.
    """
    from . import integration as delegation_integration

    # ACP workers run in the exact project root opened by the client.  There
    # is no private result tree to merge in that mode: the worker's durable
    # report and the current project revision are the result boundary.  Keep
    # the integration event for lifecycle/idempotency purposes, but never
    # manufacture an "unintegrated copy" finding for a shared-root binding.
    if workspace_binding_id in {"shared_project_root", "serialized_shared_root"}:
        return set(), None

    applied = db.execute(
        "SELECT applied_paths_json FROM workspace_integrations "
        "WHERE assignment_ref=? AND status!='noop'",
        (assignment_ref,),
    ).fetchall()
    already = set()
    for row in applied:
        try:
            already.update(json.loads(row["applied_paths_json"]) or ())
        except Exception:  # noqa: BLE001 - a malformed row never blocks acceptance
            continue
    binding = db.execute(
        "SELECT root_path, manifest_json, cleaned_up_at, task_id FROM workspace_snapshots "
        "WHERE attempt_id=? OR workspace_binding_id=? ORDER BY created_at DESC LIMIT 1",
        (attempt_id, workspace_binding_id),
    ).fetchone()
    if binding is None:
        # Review/investigation results created without workspace provisioning
        # legitimately have no snapshot. A delegated attempt always has a
        # provisioning row, so absence is blocking in that case.
        provisioned = db.execute(
            "SELECT 1 FROM delegation_provisioning WHERE attempt_id=? OR idempotency_key=? LIMIT 1",
            (attempt_id, assignment_ref),
        ).fetchone()
        if provisioned is None:
            return set(), None
        return set(), "workspace_evidence_unavailable:binding_missing"
    if binding["cleaned_up_at"]:
        # Cleanup is safe only after an exact integration outcome. Preserve
        # completion evidence for that durable proof even though the copy is
        # gone; a cleaned copy without such a row remains blocking.
        outcome = db.execute(
            "SELECT status FROM workspace_integrations WHERE assignment_ref=? "
            "AND status IN ('applied','noop') ORDER BY created_at DESC LIMIT 1",
            (assignment_ref,),
        ).fetchone()
        if outcome is not None:
            return set(), None
        return set(), "workspace_evidence_unavailable:workspace_cleaned"
    if not binding["root_path"]:
        return set(), "workspace_evidence_unavailable:workspace_cleaned"
    try:
        base_manifest = json.loads(binding["manifest_json"] or "{}")
        worker_manifest = delegation_integration._rescan_workspace(binding["root_path"])
        task = db.execute("SELECT project_root FROM tasks WHERE id=?", (binding["task_id"],)).fetchone()
        project_manifest = delegation_integration._rescan_workspace(task["project_root"]) if task else {}
    except Exception as error:  # noqa: BLE001 - uncertainty blocks acceptance
        return set(), f"workspace_evidence_unavailable:workspace_unreadable:{error}"
    pending = set()
    for relpath in set(base_manifest) | set(worker_manifest):
        if relpath in already or delegation_integration._is_technical(relpath):
            continue
        if not delegation_integration._entries_equal(base_manifest.get(relpath), worker_manifest.get(relpath)):
            if not delegation_integration._entries_equal(project_manifest.get(relpath), worker_manifest.get(relpath)):
                pending.add(relpath)
    return pending, None


def latest_integration_conflicted(db, task_id: str, assignment_ref: str) -> dict | None:
    """The conflict payload when this assignment's most recent integration
    attempt conflicted and wrote nothing, else ``None``.

    An assignment that never asked for an integration (a review, an
    investigation) has no such event and is unaffected -- only one that
    tried and was refused.
    """
    row = db.execute(
        "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
        "AND event_type='progress' AND superseded=0 "
        "AND (payload_json LIKE '%integration_applied%' OR payload_json LIKE '%integration_conflict%') "
        "ORDER BY id DESC LIMIT 1",
        (task_id, assignment_ref),
    ).fetchone()
    if row is None:
        return None
    payload = json.loads(row["payload_json"])
    return payload if payload.get("kind") == "integration_conflict" else None


def assignment_is_terminal(db, task_id: str, assignment_ref: str) -> bool:
    """True once this assignment has reached a durable terminal marker.

    This helper answers only lifecycle completion. The acceptance gate still
    rejects retained ``assignment_failed`` events because terminal failure is
    not evidence of successful task completion.

    Coordinator-facing events key ``assignment_ref`` by the delegate-time
    idempotency key, worker-facing ones by the provisioned attempt id, so
    a ``worker_result`` is looked up through ``delegation_provisioning``
    (the same bridge :func:`gate` walks).
    """
    for event_type in ("assignment_failed", "assignment_cancelled"):
        if db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type=? LIMIT 1",
            (task_id, assignment_ref, event_type),
        ).fetchone() is not None:
            return True
    row = db.execute(
        "SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=?", (assignment_ref,),
    ).fetchone()
    worker_key = row["attempt_id"] if row is not None else assignment_ref
    return db.execute(
        "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? "
        "AND event_type='worker_result' AND superseded=0 LIMIT 1",
        (task_id, worker_key),
    ).fetchone() is not None


def _report_file_exists(store, db, task_id: str, report_ref: str) -> bool:
    row = db.execute(
        "SELECT task_id, filename FROM reports WHERE id=?", (report_ref,),
    ).fetchone()
    if row is None or row["task_id"] != task_id:
        return False
    task_dir = _task_directory(store, db, task_id)
    if task_dir is None:
        return False
    path = task_dir / row["filename"]
    try:
        return path.is_file()
    except OSError:
        return False


def _has_valid_successor_result(store, db, task_id: str, assignment_ref: str) -> bool:
    """Whether an exact distinct replacement has durable valid evidence.

    A supersedes marker alone is not enough to retire a failed/canceled
    assignment. Failed successors and self/cyclic replacement declarations
    cannot provide a terminal success result.
    """
    for successor in db.execute(
        "SELECT assignment_ref,payload_json FROM task_events "
        "WHERE task_id=? AND event_type='delegation_requested' AND superseded=0",
        (task_id,),
    ).fetchall():
        successor_ref = successor["assignment_ref"]
        if successor_ref == assignment_ref:
            continue
        try:
            successor_payload = json.loads(successor["payload_json"])
        except (TypeError, ValueError):
            continue
        if successor_payload.get("supersedes_assignment_ref") != assignment_ref:
            continue
        if db.execute(
            "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? "
            "AND event_type='assignment_failed' AND superseded=0 LIMIT 1",
            (task_id, successor_ref),
        ).fetchone() is not None:
            continue
        successor_provisioning = db.execute(
            "SELECT attempt_id FROM delegation_provisioning "
            "WHERE task_id=? AND idempotency_key=?",
            (task_id, successor_ref),
        ).fetchone()
        successor_key = (
            successor_provisioning["attempt_id"]
            if successor_provisioning is not None else successor_ref
        )
        successor_result = db.execute(
            "SELECT payload_json FROM task_events WHERE task_id=? "
            "AND assignment_ref=? AND event_type='worker_result' AND superseded=0 "
            "ORDER BY id DESC LIMIT 1",
            (task_id, successor_key),
        ).fetchone()
        if successor_result is None:
            continue
        try:
            successor_result_payload = json.loads(successor_result["payload_json"])
        except (TypeError, ValueError):
            continue
        successor_report = successor_result_payload.get("report_ref")
        successor_checks = successor_result_payload.get("checks") or ()
        if (not successor_report
                or not _report_file_exists(store, db, task_id, successor_report)
                or any(
                    not isinstance(check, dict)
                    or check.get("outcome") in {"failed", "not_run", "unverified"}
                    or check.get("evidence") == "contradicted"
                    for check in successor_checks
                )):
            continue
        return True
    return False


def _pipeline_requires_coordinator_update(db, task_id: str) -> bool:
    """Whether the current pipeline is still the runtime bootstrap edition.

    Missing pipelines remain governed by the existing bootstrap/recovery
    paths. Once a bootstrap exists, accepting it as the current plan is
    unsafe because it explicitly says that the coordinator has not run.
    """
    row = db.execute(
        "SELECT e.author, e.sequence "
        "FROM reports r JOIN editions e ON e.report_id=r.id "
        "WHERE r.task_id=? AND r.kind='pipeline' "
        "AND e.sequence=(SELECT MAX(x.sequence) FROM editions x WHERE x.report_id=r.id)",
        (task_id,),
    ).fetchone()
    # ``editions.sequence`` is global across the project, so it is not a
    # per-pipeline generation number.  The runtime is the only publisher of
    # the opening edition; any later current edition must be coordinator-
    # authored.
    return row is not None and row["author"] == "runtime"


def current_pipeline_binding(db, task_id: str) -> dict | None:
    """Return the immutable identity of the current pipeline edition.

    A task-level acceptance is bound to this exact edition.  The digest is
    the content identity, while ``report_id`` and ``sequence`` make the
    binding unambiguous even if a store is migrated or a report is later
    repaired.  Runtime bootstrap (and a missing pipeline) is deliberately not
    a coordinator binding.
    """
    row = db.execute(
        "SELECT r.id AS report_id, e.sequence, e.digest, e.author "
        "FROM reports r JOIN editions e ON e.report_id=r.id "
        "WHERE r.task_id=? AND r.kind='pipeline' "
        "ORDER BY e.sequence DESC LIMIT 1",
        (task_id,),
    ).fetchone()
    if row is None or row["author"] == "runtime":
        return None
    return {
        "report_id": row["report_id"],
        "sequence": int(row["sequence"]),
        "digest": row["digest"],
    }


def pipeline_binding_matches(db, task_id: str, binding) -> bool:
    """Check that ``binding`` still names the current coordinator edition."""
    if not isinstance(binding, dict):
        return False
    current = current_pipeline_binding(db, task_id)
    return current is not None and all(
        current.get(field) == binding.get(field)
        for field in ("report_id", "sequence", "digest")
    )


def own_assignment_accept_notices(db, task_id: str, channel_id: str) -> set[int]:
    """Return only pending reminders backed by this coordinator's real accept.

    These reminders carry no worker evidence. The explicit task decision
    answers them, but only the decision's own channel may discharge them.
    """
    notices = db.execute(
        "SELECT id,assignment_ref,dedup_key,payload_json FROM task_events "
        "WHERE task_id=? AND event_type='progress' AND origin='runtime' "
        "AND consumed_by IS NULL AND superseded=0",
        (task_id,),
    ).fetchall()
    result = set()
    for notice in notices:
        try:
            payload = json.loads(notice["payload_json"])
            decision_id = payload.get("decision_event_id")
            ref = notice["assignment_ref"]
            if (payload.get("kind") != "assignment_accept_requires_task"
                    or payload.get("assignment_ref") != ref
                    or notice["dedup_key"] != f"assignment-accept-followup:{ref}"
                    or type(decision_id) is not int or not ref):
                continue
            decision = db.execute(
                "SELECT id,origin,assignment_ref,payload_json FROM task_events "
                "WHERE task_id=? AND id=? AND event_type='coordinator_decision' "
                "AND superseded=0 AND id<?",
                (task_id, decision_id, notice["id"]),
            ).fetchone()
            if decision is None or decision["origin"] != channel_id:
                continue
            accepted = json.loads(decision["payload_json"])
            alias = db.execute(
                "SELECT attempt_id FROM delegation_provisioning "
                "WHERE task_id=? AND idempotency_key=?",
                (task_id, ref),
            ).fetchone()
            refs = {ref, alias["attempt_id"] if alias is not None else ref}
            if (accepted.get("decision") == "accept"
                    and accepted.get("assignment_ref") in refs
                    and decision["assignment_ref"] in refs):
                result.add(notice["id"])
        except (TypeError, ValueError):
            continue
    return result


def gate(store, task_id: str, *, completing_channel_id: str | None = None) -> tuple[bool, list[str]]:
    """Return ``(ok, reasons)``. ``reasons`` is empty iff ``ok``.

    ``reasons`` are concrete, English, one line per failing condition --
    suitable both for a ``StoreError`` correction and for a durable
    ``progress`` event note.
    """
    reasons: list[str] = []
    project_root = None
    # T019: (assignment_ref, attempt_revisions row captured at worker_result
    # time) for every accepted assignment whose worker_result included at
    # least one 'observed' check -- verified only below, against a fresh
    # capture of the *current* shared workspace root (see
    # delegation.revision module docstring: any worker/coordinator/user
    # change to the shared root invalidates every attempt's own already-
    # captured observed checks, not only the attempt that made it).
    observed_checks = []
    shared_publications = []
    with store.connection() as db:
        project_root_row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
        project_root = project_root_row["project_root"] if project_root_row is not None else None
        requested = db.execute(
            "SELECT DISTINCT assignment_ref FROM task_events "
            "WHERE task_id=? AND event_type='delegation_requested' AND superseded=0",
            (task_id,),
        ).fetchall()
        assignment_refs = [row["assignment_ref"] for row in requested]

        # A model-authored report cannot establish that a client approved a
        # destructive command. For every observed rm action, require the
        # exact task/attempt/tool-call/command binding returned by the live
        # ACP permission bridge after a selected allow_once host response.
        # This remains host-response evidence, not proof of a human click;
        # clients may have their own auto-accept policy.
        for action in db.execute(
            "SELECT DISTINCT x.attempt_id,x.tool_call_ref,x.title FROM tool_result_observations x "
            "WHERE x.task_id=? AND x.kind='execute' AND x.title IS NOT NULL "
            "AND (SELECT y.status FROM tool_result_observations y "
            "WHERE y.attempt_id=x.attempt_id AND y.tool_call_ref=x.tool_call_ref "
            "ORDER BY y.id DESC LIMIT 1)='completed'", (task_id,),
        ).fetchall():
            command = action["title"]
            if not is_rm_command(command):
                continue
            receipt = db.execute(
                "SELECT command_sha256,host_outcome FROM permission_receipts "
                "WHERE task_id=? AND attempt_id=? AND tool_call_ref=?",
                (task_id, action["attempt_id"], action["tool_call_ref"]),
            ).fetchone()
            if (
                receipt is None or receipt["host_outcome"] != "allow_once"
                or receipt["command_sha256"] != hashlib.sha256(command.encode("utf-8")).hexdigest()
            ):
                reasons.append(
                    f"unconfirmed_rm_permission:{action['attempt_id']}:{action['tool_call_ref']}"
                )

        for assignment_ref in assignment_refs:
            failed = db.execute(
                "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='assignment_failed' LIMIT 1",
                (task_id, assignment_ref),
            ).fetchone() is not None
            cancelled = db.execute(
                "SELECT 1 FROM task_events WHERE task_id=? AND assignment_ref=? AND event_type='assignment_cancelled' LIMIT 1",
                (task_id, assignment_ref),
            ).fetchone() is not None
            # Coordinator-facing events (delegation_requested, assignment_
            # failed/cancelled, coordinator_decision, ...) key assignment_ref
            # by the delegate-time idempotency_key/hash; worker-facing events
            # (worker_result, worker_question) key it by the provisioned
            # attempt_id instead (see delegation.provisioning -- a fresh
            # "xa_..." id, never the hash). Resolve the bridge via
            # delegation_provisioning before looking up a worker_result.
            provisioning_row = db.execute(
                "SELECT attempt_id,workspace_binding_id FROM delegation_provisioning WHERE idempotency_key=?", (assignment_ref,),
            ).fetchone()
            worker_key = provisioning_row["attempt_id"] if provisioning_row is not None else assignment_ref
            result_count = db.execute(
                "SELECT COUNT(*) AS n FROM task_events WHERE task_id=? AND assignment_ref=? "
                "AND event_type='worker_result' AND superseded=0",
                (task_id, worker_key),
            ).fetchone()["n"]
            if result_count > 1:
                reasons.append(
                    f"assignment {assignment_ref} has ambiguous duplicate worker_result events "
                    f"for attempt {worker_key}; reconcile the retained evidence before accepting."
                )
                continue
            result_row = db.execute(
                "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=? "
                "AND event_type='worker_result' AND superseded=0 ORDER BY id DESC LIMIT 1",
                (task_id, worker_key),
            ).fetchone()
            if not (failed or cancelled or result_row is not None):
                reasons.append(
                    f"assignment {assignment_ref} is not terminal (no retained worker_result, "
                    "assignment_failed, or assignment_cancelled event)."
                )
                continue
            # A failed assignment is not a successful terminal result.  The
            # previous gate treated ``assignment_failed`` as merely one of
            # the ways an assignment could become terminal, which allowed a
            # coordinator to accept the task after every worker route had
            # failed and then claim that it completed the work itself.  That
            # is especially dangerous for mutating tasks: the coordinator's
            # prose (or a command it ran in its own workspace) is not a
            # candidate, an integration, or check evidence.  A deliberate
            # cancellation can still be paired with another successful
            # assignment; an actual failure always blocks acceptance until
            # the work is re-delegated and produces a verified result.
            if failed:
                # A revision creates a new assignment that explicitly
                # supersedes the old one.  Its historical failure must not
                # poison a later, successful attempt; only the current
                # assignment remains relevant to acceptance.
                if _has_valid_successor_result(store, db, task_id, assignment_ref):
                    continue
                # A user-directed abandonment is a valid way to remove an
                # assignment from the task (the remaining successful work
                # may still be accepted).  It must be explicit, though: a
                # cancellation that claims the coordinator completed the
                # failed work is precisely the unsafe fallback this gate is
                # intended to reject.
                cancellation = db.execute(
                    "SELECT payload_json FROM task_events WHERE task_id=? "
                    "AND assignment_ref=? AND event_type='assignment_cancelled' "
                    "AND superseded=0 ORDER BY id DESC LIMIT 1",
                    (task_id, assignment_ref),
                ).fetchone()
                cancellation_reason = ""
                if cancellation is not None:
                    try:
                        cancellation_reason = str(json.loads(cancellation["payload_json"]).get("reason") or "").lower()
                    except (TypeError, ValueError):
                        cancellation_reason = ""
                explicit_abandonment = any(
                    marker in cancellation_reason
                    for marker in ("no longer needed", "out of scope", "user cancelled", "user canceled")
                )
                coordinator_fallback = any(
                    marker in cancellation_reason
                    for marker in ("completed", "fixed", "verified", "directly", "coordinator")
                )
                if explicit_abandonment and not coordinator_fallback:
                    continue
                reasons.append(
                    f"assignment {assignment_ref} has a retained assignment_failed event; "
                    "re-delegate the work and obtain a verified result before accepting."
                )
            # Cancellation can complete an assignment only when there is no
            # retained worker result. If a result is still retained, validate
            # its evidence even when a cancellation marker also exists;
            # superseded historical results are already excluded above.
            if result_row is not None:
                # Retire a canceled result only when an exact, durable
                # successor replaces it and that successor already has its
                # own valid result. The successor is validated independently
                # in its own loop iteration.
                if (cancelled and not failed
                        and _has_valid_successor_result(store, db, task_id, assignment_ref)):
                    continue
                payload = json.loads(result_row["payload_json"])
                # A worker result may carry optional claimed checks. They
                # remain model-authored evidence, but an explicit failed or
                # unrun check (or one contradicted by this attempt's observed
                # tool result) is a durable hard blocker.
                for check in payload.get("checks") or ():
                    if not isinstance(check, dict):
                        reasons.append(
                            f"assignment {assignment_ref} has malformed check evidence; "
                            "reconcile the worker result before accepting."
                        )
                        continue
                    name = str(check.get("name") or "unnamed check")
                    outcome = check.get("outcome")
                    evidence = check.get("evidence")
                    if outcome in {"failed", "not_run", "unverified"}:
                        # A worker-side rendered check can be unverified when
                        # its native browser was unavailable.  A later exact
                        # candidate check is allowed to supersede that one,
                        # but only through the same requirement-bound,
                        # current-candidate path used by assignment-level
                        # acceptance.  Explicit ``failed`` remains a hard
                        # contradiction and is never superseded silently.
                        superseded_by_candidate = (
                            outcome == "unverified"
                            # ``assignment_acceptance_evidence`` intentionally
                            # returns an empty list for informational
                            # assignments.  That empty result is not evidence
                            # that a candidate check superseded this worker
                            # observation; require the rendered/UI binding
                            # before consulting the candidate lane.
                            and _assignment_requires_rendered_check(
                                db, task_id, assignment_ref
                            )
                            and not assignment_acceptance_evidence(
                                store, task_id, assignment_ref
                            )
                        )
                        if superseded_by_candidate:
                            continue
                        reasons.append(
                            f"assignment {assignment_ref} check {name!r} is {outcome}; "
                            "resolve the failed or incomplete verification before accepting."
                        )
                    elif evidence == "contradicted":
                        reasons.append(
                            f"assignment {assignment_ref} check {name!r} is contradicted "
                            "by the observed tool result; rerun or reconcile it before accepting."
                        )
                report_ref = payload.get("report_ref")
                if not report_ref or not _report_file_exists(store, db, task_id, report_ref):
                    reasons.append(
                        f"assignment {assignment_ref}'s report_ref {report_ref!r} does not "
                        "resolve to a stored report file for this task."
                    )
                else:
                    if (provisioning_row is not None and
                            provisioning_row["workspace_binding_id"] in {
                                "shared_project_root", "serialized_shared_root",
                            }):
                        # Shared-root acceptance must reconcile the current
                        # manifest even when the worker supplied no observed
                        # checks.  Otherwise an external/coordinator edit
                        # between turns becomes the next pre-turn revision
                        # and can be accepted as if it were the worker's.
                        shared_publications.append((assignment_ref, worker_key, report_ref))
                    if any((check.get("evidence") == "observed") for check in (payload.get("checks") or [])):
                        captured = db.execute(
                            "SELECT kind, head, dirty_digest FROM attempt_revisions "
                            "WHERE attempt_id=? AND phase LIKE 'worker_result:%' ORDER BY captured_at DESC LIMIT 1",
                            (worker_key,),
                        ).fetchone()
                        if captured is not None and captured["kind"] == "observed":
                            observed_checks.append((assignment_ref, dict(captured)))
                    # A delivered change that is still only in the worker's
                    # own copy is not in the project, whatever the report
                    # says. Live, twice: a worker did the work correctly in
                    # its workspace, nobody integrated it, the coordinator
                    # accepted, and the project file was exactly as it had
                    # been -- the user was told the task was done and
                    # nothing had changed.
                    pending, evidence_error = _unintegrated_worker_changes(
                        db, assignment_ref, worker_key,
                        provisioning_row["workspace_binding_id"] if provisioning_row is not None else None,
                    )
                    if evidence_error:
                        reasons.append(f"assignment {assignment_ref} {evidence_error}.")
                    if pending:
                        shown = ", ".join(sorted(pending)[:5])
                        reasons.append(
                            f"assignment {assignment_ref} changed {shown} in its own workspace and "
                            "those changes are not in the project: integrate them (or record why "
                            "they are not wanted) before accepting."
                        )

        unconsumed_candidates = db.execute(
            "SELECT * FROM task_events WHERE task_id=? AND consumed_by IS NULL AND superseded=0 "
            "AND (event_type IN ({}) OR event_type='progress')".format(
                ",".join("?" for _ in delegation_events.MEANINGFUL_EVENT_TYPES)
            ),
            (task_id, *delegation_events.MEANINGFUL_EVENT_TYPES),
        ).fetchall()
        own_notices = (own_assignment_accept_notices(db, task_id, completing_channel_id)
                       if completing_channel_id is not None else set())
        unconsumed_meaningful = [delegation_events.TaskEvent._from_row(row) for row in unconsumed_candidates
                                 if row["id"] not in own_notices
                                 and delegation_events.is_meaningful(delegation_events.TaskEvent._from_row(row))]
        if _pipeline_requires_coordinator_update(db, task_id):
            reasons.append(
                "the current pipeline is only the runtime bootstrap edition; "
                "publish a coordinator-authored current pipeline edition with "
                "the decision rationale, assignments, evidence, and anticipated "
                "acceptance before accepting."
            )
    if unconsumed_meaningful:
        kinds = sorted({row.event_type for row in unconsumed_meaningful})
        reasons.append(
            f"unconsumed meaningful event(s) remain for this task: {', '.join(kinds)}. "
            "Finish this coordinator turn without retrying acceptance; the runtime will "
            "deliver those events in the next continuation, where you can decide again."
        )

    # T019: artifact revision binding, at coordinator-acceptance time. The
    # current capture is required to be an exact observation before an
    # earlier observed check can certify the project. An unavailable, capped,
    # racy, or otherwise non-observed capture is incomplete and refuses
    # acceptance with its concrete capture reason.
    if (observed_checks or shared_publications) and project_root is not None:
        current = delegation_revision.record_attempt_revision(store, f"gate:{task_id}", "gate_acceptance", project_root)
        if current.get("kind") != "observed":
            kind = current.get("kind") or "unknown"
            detail = current.get("detail") or "no capture detail"
            reasons.append(f"current_revision_unavailable:{kind}:{detail}")
        else:
            for assignment_ref, captured in observed_checks:
                # HEAD is provenance metadata, not the artifact content. A
                # commit or branch move with identical bytes must not make a
                # previously observed check stale; bind acceptance to the
                # content digest only.
                if captured["dirty_digest"] != current["dirty_digest"]:
                    reasons.append(f"stale_verification:{assignment_ref}")
            for assignment_ref, attempt_id, report_ref in shared_publications:
                with store.connection() as db:
                    publication_rows = db.execute(
                        "SELECT kind, dirty_digest, manifest_json FROM attempt_revisions "
                        "WHERE attempt_id=? AND phase=? ORDER BY rowid ASC",
                        (attempt_id, f"worker_publish:{report_ref}"),
                    ).fetchall()
                if len(publication_rows) > 1:
                    reasons.append(f"shared_root_publication_ambiguous:{assignment_ref}")
                    continue
                row = publication_rows[0] if publication_rows else None
                if row is None:
                    reasons.append(f"shared_root_publication_unavailable:{assignment_ref}")
                    continue
                try:
                    publication = dict(
                        kind=row["kind"], dirty_digest=row["dirty_digest"],
                        manifest=json.loads(row["manifest_json"]) if row["manifest_json"] else None,
                    )
                except (TypeError, ValueError):
                    reasons.append(f"shared_root_publication_malformed:{assignment_ref}")
                    continue
                attributed, reason = delegation_revision.shared_publication_attribution(
                    store, task_id, attempt_id, publication, current,
                )
                if not attributed:
                    reasons.append(reason or f"shared_root_attribution_refused:{assignment_ref}")

    return (not reasons, reasons)
