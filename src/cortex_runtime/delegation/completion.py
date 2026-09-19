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
   per-assignment accept step), ``assignment_failed``, or
   ``assignment_cancelled``.
2. No unconsumed *meaningful* event remains for the task (see
   ``delegation.events.MEANINGFUL_EVENT_TYPES``) -- an un-reacted-to worker
   result/question/failure/cancellation/user message.
3. Every terminal-by-``worker_result`` assignment's ``report_ref`` resolves
   to a ``reports`` row for this task *and* the report file exists on disk
   -- a claimed success with no stored evidence never gates a host-success
   completion.

This module never inspects assignment status via anything other than the
durable ``task_events`` log and the ``reports`` table -- there is no
separate "assignment" table (see ``acp.orchestrating_runtime`` module
docstring on the process-boundary/durable-log design).
"""
from __future__ import annotations

import json

from . import events as delegation_events
from . import revision as delegation_revision


def _task_directory(store, db, task_id: str):
    from ..store import private_directory, project_task_directory

    row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
    if row is None:
        return None
    cortex_root = project_task_directory(row["project_root"])
    return private_directory(cortex_root / task_id)


def _unintegrated_worker_changes(db, assignment_ref: str, attempt_id: str) -> set[str]:
    """Paths this worker changed in its own copy that are not in the
    project, as the same comparison ``delegation.integration`` itself uses
    (its recorded base manifest against a fresh scan of its workspace).

    Empty when the worker changed nothing, when an integration for this
    assignment has already applied its changes, or when the workspace is
    gone (an attempt whose workspace was cleaned up has nothing pending by
    definition -- this never guesses that something is missing).
    """
    from . import integration as delegation_integration

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
        "SELECT root_path, manifest_json, cleaned_up_at FROM workspace_snapshots "
        "WHERE attempt_id=? ORDER BY created_at DESC LIMIT 1",
        (attempt_id,),
    ).fetchone()
    if binding is None or not binding["root_path"] or binding["cleaned_up_at"]:
        return set()
    try:
        base_manifest = json.loads(binding["manifest_json"] or "{}")
        worker_manifest = delegation_integration._rescan_workspace(binding["root_path"])
    except Exception:  # noqa: BLE001 - an unreadable workspace never blocks acceptance
        return set()
    pending = set()
    for relpath in set(base_manifest) | set(worker_manifest):
        if relpath in already or delegation_integration._is_technical(relpath):
            continue
        if not delegation_integration._entries_equal(base_manifest.get(relpath), worker_manifest.get(relpath)):
            pending.add(relpath)
    return pending


def assignment_is_terminal(db, task_id: str, assignment_ref: str) -> bool:
    """True once this assignment has finished, by any of the three ways it
    can: a retained ``worker_result``, an ``assignment_failed``, or an
    ``assignment_cancelled``.

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


def gate(store, task_id: str) -> tuple[bool, list[str]]:
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
    with store.connection() as db:
        project_root_row = db.execute("SELECT project_root FROM tasks WHERE id=?", (task_id,)).fetchone()
        project_root = project_root_row["project_root"] if project_root_row is not None else None
        requested = db.execute(
            "SELECT DISTINCT assignment_ref FROM task_events "
            "WHERE task_id=? AND event_type='delegation_requested' AND superseded=0",
            (task_id,),
        ).fetchall()
        assignment_refs = [row["assignment_ref"] for row in requested]

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
                "SELECT attempt_id FROM delegation_provisioning WHERE idempotency_key=?", (assignment_ref,),
            ).fetchone()
            worker_key = provisioning_row["attempt_id"] if provisioning_row is not None else assignment_ref
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
            if result_row is not None and not (failed or cancelled):
                payload = json.loads(result_row["payload_json"])
                report_ref = payload.get("report_ref")
                if not report_ref or not _report_file_exists(store, db, task_id, report_ref):
                    reasons.append(
                        f"assignment {assignment_ref}'s report_ref {report_ref!r} does not "
                        "resolve to a stored report file for this task."
                    )
                else:
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
                    pending = _unintegrated_worker_changes(db, assignment_ref, worker_key)
                    if pending:
                        shown = ", ".join(sorted(pending)[:5])
                        reasons.append(
                            f"assignment {assignment_ref} changed {shown} in its own workspace and "
                            "those changes are not in the project: integrate them (or record why "
                            "they are not wanted) before accepting."
                        )

        unconsumed_meaningful = db.execute(
            "SELECT event_type FROM task_events WHERE task_id=? AND consumed_by IS NULL AND superseded=0 "
            "AND event_type IN ({})".format(
                ",".join("?" for _ in delegation_events.MEANINGFUL_EVENT_TYPES)
            ),
            (task_id, *delegation_events.MEANINGFUL_EVENT_TYPES),
        ).fetchall()
    if unconsumed_meaningful:
        kinds = sorted({row["event_type"] for row in unconsumed_meaningful})
        reasons.append(f"unconsumed meaningful event(s) remain for this task: {', '.join(kinds)}.")

    # T019: artifact revision binding, at coordinator-acceptance time. An
    # unavailable *current* capture never refuses -- it only leaves the
    # earlier captures' 'observed' checks unbound (unknown), it never
    # assumes staleness from an inconclusive comparison.
    if observed_checks and project_root is not None:
        current = delegation_revision.record_attempt_revision(store, f"gate:{task_id}", "gate_acceptance", project_root)
        if current["kind"] == "observed":
            for assignment_ref, captured in observed_checks:
                if (captured["head"], captured["dirty_digest"]) != (current["head"], current["dirty_digest"]):
                    reasons.append(f"stale_verification:{assignment_ref}")

    return (not reasons, reasons)
