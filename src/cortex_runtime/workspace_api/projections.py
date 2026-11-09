"""Read-transaction-only mapping of runtime facts to the public wire DTOs."""
from dataclasses import dataclass
import json
from typing import TypedDict

from ..workspace_contract import ContractError, LIMITS, MAPPING_VERSION, validate
from .storage import WorkspaceError, digest, encode, event_cursor, outbox_position
from .routes import observed_route

LIFECYCLE = {
    "CREATED": ("planning", "starting"),
    "PLANNING": ("planning", "running"), "PLAN_READY": ("planning", "running"),
    "EXECUTING": ("execution", "running"), "REVIEWING": ("review", "running"),
    "VERIFYING": ("verification", "running"), "RECONCILING": ("finalizing", "running"),
    "ANSWER_READY": ("finalizing", "running"), "WAITING_USER": ("waiting", "waiting_user"),
    "RECOVERING": ("recovery", "recovering"), "BLOCKED": ("blocked", "blocked"),
    "FAILED": ("failed", "failed"), "CANCELLED": ("stopped", "stopped"),
    "COMPLETED": ("result", "result"), "PAUSED": ("paused", "paused"),
}


@dataclass(frozen=True)
class LifecycleProjection:
    phase: str
    state: str
    warnings: tuple[str, ...] = ()


def map_lifecycle(runtime_state, *, has_execution=True, contradictory=False):
    if contradictory:
        return LifecycleProjection("unknown", "unknown", ("contradictory_evidence",))
    if not has_execution:
        if runtime_state is None:
            return LifecycleProjection("not_started", "not_started")
        return LifecycleProjection("unknown", "unknown", ("unexpected_runtime_evidence",))
    if runtime_state not in LIFECYCLE:
        return LifecycleProjection("unknown", "unknown", ("runtime_state_unavailable",))
    return LifecycleProjection(*LIFECYCLE[runtime_state])


class TaskProjection(TypedDict):
    taskId: str
    description: str
    title: str | None
    titleAvailability: str
    version: str
    executionId: str | None
    phase: str
    runtimeState: str | None
    executionState: str
    waitReason: str | None
    lastConfirmedAt: str | None
    warnings: list[str]
    coordinatorRoute: dict[str, str] | None


class ExecutionProjection(TypedDict):
    executionId: str
    taskId: str
    version: str
    runtimeTaskId: str | None
    acpSessionId: str | None
    runtimePlanRevision: str | None
    ownerEpoch: str | None
    state: str
    warnings: list[str]


# The runtime's own launch-retry notices (OrchestratingRuntime._ensure_coordinator_retrying), newest decides.
LAUNCH_NOTICES = {
    "engine_retry.launch": ("planning", "starting", "engine_launch_retrying"),
    "engine_retry.gave_up": ("blocked", "blocked", "engine_launch_failed"),
    "engine_retry.recovered": None,
}


def _launch(db, task_id):
    """A started task whose runtime row does not exist yet: its coordinator's engine is still opening."""
    if db.execute("SELECT 1 FROM task_stopped WHERE task_id=?", (task_id,)).fetchone():
        return LifecycleProjection("stopped", "stopped"), None
    row = db.execute(
        "SELECT payload_json FROM task_events WHERE task_id=? AND event_type='progress' AND "
        "json_extract(payload_json,'$.message_key') IN (" + ",".join("?" * len(LAUNCH_NOTICES)) + ") "
        "ORDER BY id DESC LIMIT 1", (task_id, *LAUNCH_NOTICES)).fetchone()
    notice = json.loads(row[0]) if row else {}
    known = LAUNCH_NOTICES.get(notice.get("message_key"))
    if known is None:
        return LifecycleProjection("planning", "starting"), None
    phase, state, warning = known
    note = notice.get("note")
    return LifecycleProjection(phase, state, (warning,)), note[:1000] if isinstance(note, str) else None


def _facts(db, execution, project_root):
    runtime = db.execute(
        "SELECT r.*,t.project_root FROM runtime_tasks r JOIN tasks t ON t.id=r.task_id WHERE r.task_id=?",
        (execution["runtime_task_id"],)).fetchone()
    session = db.execute("SELECT * FROM acp_host_sessions WHERE session_id=?",
                         (execution["acp_session_id"],)).fetchone()
    owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?",
                       (execution["runtime_task_id"],)).fetchone()
    contradictory = (runtime is None or session is None or owner is None
                     or runtime["project_root"] != project_root
                     or session["project_root"] != project_root
                     or session["task_id"] != execution["runtime_task_id"]
                     or owner["epoch"] != execution["owner_epoch"]
                     or runtime["revision"] < 0
                     or (runtime["state"] == "COMPLETED" and bool(runtime["blocker"])))
    start = db.execute("SELECT state FROM workspace_starts WHERE execution_id=?",
                       (execution["execution_id"],)).fetchone()
    if runtime is None and start is not None and start["state"] in ("claimed", "dispatched"):
        # Only an owner start binds before the runtime row exists (storage.bind_execution_tx).
        # Recovery fences and reacquires ownership on each failed open. The same bound
        # task/session may advance, never regress; runtime-bound executions still require equality.
        base = db.execute("SELECT project_root FROM tasks WHERE id=?", (execution["runtime_task_id"],)).fetchone()
        if (base is not None and session is not None and owner is not None
                and base["project_root"] == session["project_root"] == project_root
                and session["task_id"] == execution["runtime_task_id"]
                and owner["epoch"] >= execution["owner_epoch"]):
            mapping, reason = _launch(db, execution["runtime_task_id"])
            return None, mapping, reason
    contradictory = contradictory or (start is not None and start["state"] == "unknown")
    state = runtime["state"] if runtime is not None else None
    mapping = map_lifecycle(state, contradictory=contradictory)
    wait = db.execute("SELECT wait_reason FROM task_orchestration WHERE task_id=?",
                      (execution["runtime_task_id"],)).fetchone()
    reason = wait["wait_reason"] if wait is not None else None
    # Reasons are content, never a second lifecycle or authority.
    if mapping.state == "unknown":
        reason = None
    return runtime, mapping, reason


def _bounded_task(value):
    try:
        return validate("Task", value)
    except ContractError as error:
        if str(error) != "page_too_large":
            raise
        # Do not truncate a user's description or accidentally exceed the page.
        value["description"] = ""
        value["descriptionAvailability"] = "unavailable"
        value["warnings"].append("description_unavailable:page_limit")
        return validate("Task", value)


def task_projection(db, task, project_root) -> TaskProjection:
    execution = None
    if task["execution_id"] is not None:
        execution = db.execute("SELECT * FROM workspace_executions WHERE execution_id=? AND task_id=?",
                               (task["execution_id"], task["task_id"])).fetchone()
    runtime, reason = None, None
    if execution is not None:
        runtime, mapping, reason = _facts(db, execution, project_root)
    elif task["execution_id"] is not None:
        mapping = map_lifecycle(None, contradictory=True)
    else:
        mapping = map_lifecycle(None, has_execution=False)
    return _bounded_task(dict(
        taskId=task["task_id"], description=task["description"], title=None,
        titleAvailability="not_connected", version=str(task["version"]),
        executionId=task["execution_id"], phase=mapping.phase,
        runtimeState=runtime["state"] if runtime is not None and runtime["state"] in LIFECYCLE else None,
        executionState=mapping.state, waitReason=reason,
        lastConfirmedAt=runtime["updated_at"] if runtime is not None else task["created_at"],
        coordinatorRoute=observed_route(db, execution["runtime_task_id"]) if execution is not None else None,
        warnings=list(mapping.warnings)))


def execution_projection(db, execution, project_root) -> ExecutionProjection:
    runtime, mapping, _ = _facts(db, execution, project_root)
    return validate("Execution", dict(
        executionId=execution["execution_id"], taskId=execution["task_id"],
        version=str(execution["version"]), runtimeTaskId=execution["runtime_task_id"],
        acpSessionId=execution["acp_session_id"],
        runtimePlanRevision=str(runtime["revision"]) if runtime is not None and runtime["revision"] >= 0 else None,
        ownerEpoch=str(execution["owner_epoch"]), state=mapping.state, warnings=list(mapping.warnings),
        resultId=("wr_" + execution["execution_id"]) if runtime is not None and runtime["synthesis"] else None))


def task_page(db, project, *, limit=50, cursor=None, target=None):
    """Keyset page bound to the project/query and current projection version.

    All rows, typed reads and the public cursor resolve on the caller's read
    transaction. A mutation between pages expires the cursor, never mixes pages.
    """
    binding = digest([project["project_id"], str(project["version"]), limit, target])[:32]
    after = 0
    if cursor is not None:
        parts = cursor.split(".")
        if (len(parts) != 3 or parts[0] != "wt1" or parts[1] != binding
                or not parts[2].isascii() or not parts[2].isdecimal()
                or len(parts[2]) > 19 or str(int(parts[2])) != parts[2]):
            raise WorkspaceError("invalid_cursor")
        after = int(parts[2])
        if after > 9223372036854775807:
            raise WorkspaceError("invalid_cursor")
        if db.execute("SELECT 1 FROM workspace_tasks WHERE project_id=? AND sequence=?",
                      (project["project_id"], after)).fetchone() is None:
            raise WorkspaceError("invalid_cursor")
    rows = db.execute("SELECT * FROM workspace_tasks WHERE project_id=? AND sequence>? "
                      "ORDER BY sequence LIMIT ?", (project["project_id"], after, limit + 1)).fetchall()
    high, floor = outbox_position(db, project["project_id"])
    page = dict(items=[], nextCursor=None, highWatermark=str(high),
                eventCursor=event_cursor(project["project_id"], high), retainedFloor=str(floor))
    last = after
    # Leave room for the ProjectSnapshot wrapper.
    budget = LIMITS["pageContentBytes"] - 1024
    for row in rows[:limit]:
        item = task_projection(db, row, project["project_root"])
        if len(encode(dict(page, items=page["items"] + [item])).encode("utf-8")) > budget:
            if page["items"]:
                break
            item["description"] = ""
            item["descriptionAvailability"] = "unavailable"
            item["warnings"].append("description_unavailable:page_limit")
        page["items"].append(item)
        last = row["sequence"]
    if len(rows) > len(page["items"]):
        page["nextCursor"] = "wt1." + binding + "." + str(last)
    return validate("TaskPage", page)


def result_actions(result):
    """Owner review controls, also used by the mutation gate. Not merge authority."""
    reason, unblock = None, None
    if result["runtimeState"] != "COMPLETED":
        reason, unblock = "Wait for Cortex to finish this result.", "wait"
    elif result.get("warnings") or result.get("checksAvailability") == "unavailable":
        reason, unblock = "Result evidence is unavailable or the files changed. Request changes and run fresh checks.", "request_changes"
    elif result["kind"] not in ("answer", "work"):
        reason, unblock = "This result has no recorded answer or work evidence. Request a new result.", "request_changes"
    elif result["kind"] == "work" and result["reviewStatus"] not in ("APPROVE", "not_applicable"):
        reason, unblock = "Independent review is not approved. Request changes to complete review.", "request_changes"
    elif result["kind"] == "work" and (not result["workspaceBindingValid"] or result["verificationStatus"] != "passed"):
        reason, unblock = "Run the current runtime verification checks before accepting work.", "rerun_checks"
    else:
        pending = [c for c in result["checks"] if c.get("kind") != "manual" and c["outcome"] not in ("passed", "not_applicable")]
        if any(not c.get("technicalReason", "").strip() for c in pending):
            reason, unblock = "Run the unpassed checks first, or request changes. Only recorded technical blockers may be acknowledged.", "rerun_checks"
    return dict(accept=reason is None, requestChanges=result["runtimeState"] == "COMPLETED",
                reason=reason, unblockAction=unblock,
                acknowledgeOpenChecks=any(c.get("kind") != "manual" and c["outcome"] not in ("passed", "not_applicable")
                                          for c in result["checks"]))


def review_content_identity(db, task_id, runtime):
    nodes = db.execute("SELECT assignment_ref,result_json FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid",
                       (task_id, runtime["revision"])).fetchall()
    return digest([runtime["plan_json"], [(n["assignment_ref"], n["result_json"]) for n in nodes]])


def result_projection(db, store, project_id, result_id):
    """Structured runtime facts only. Synthesis is content, never parsed authority."""
    from .. import task_workspace
    from ..contracts import StoreError
    from ..delegation.revision import capture_revision
    from ..orchestration.artifacts import report_digest
    from ..orchestration.state import current_review_waiver

    execution_id = result_id[3:] if result_id.startswith("wr_") else None
    execution = db.execute("SELECT e.* FROM workspace_executions e JOIN workspace_tasks t USING(task_id) "
                           "WHERE t.project_id=? AND e.execution_id=?", (project_id, execution_id)).fetchone()
    if execution is None:
        raise WorkspaceError("result_unavailable")
    runtime, mapping, _ = _facts(db, execution, store.project_root)
    if runtime is None or not runtime["synthesis"] or mapping.state == "unknown":
        raise WorkspaceError("result_unavailable")
    task_id, revision = execution["runtime_task_id"], runtime["revision"]
    nodes = db.execute("SELECT * FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid",
                       (task_id, revision)).fetchall()
    answer = db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND type='ANSWER_SUBMITTED' LIMIT 1",
                        (task_id,)).fetchone()
    kind = "work" if nodes else "answer" if answer else "unknown"
    warnings, evidence, checks = [], [], []
    review_status = "not_applicable" if kind == "answer" else "unknown"
    from ..orchestration.check_closure import runtime_gap_closures
    from ..orchestration.worker_checks import active_checks, reconcile_closures, unresolved, latest_attempts
    closed_checks = reconcile_closures(db, task_id, revision, runtime_gap_closures(db, task_id, revision))
    check_attempts = unresolved(db, task_id, revision)
    for node in nodes:
        result = json.loads(node["result_json"]) if node["result_json"] else {}
        if node["stage"] == "review":
            verdict = result.get("verdict")
            review_status = verdict if verdict in ("APPROVE", "REQUEST_CHANGES", "BLOCK") else "unknown"
        reference, expected = result.get("report_ref"), result.get("report_digest")
        report = db.execute("SELECT filename FROM reports WHERE task_id=? AND id=?", (task_id, reference)).fetchone()
        try:
            actual = report_digest(store, task_id, report["filename"]) if report else None
        except (OSError, ValueError, StoreError):
            actual = None
        if actual is not None and actual == expected:
            evidence.append(dict(reference=reference, digest=actual))
        else:
            warnings.append("report_evidence_unavailable")
            if node["stage"] == "review":
                review_status = "unknown"
        for check_index, check in active_checks(db, task_id, node["assignment_ref"], result.get("checks", [])):
            outcome = check.get("outcome", "unknown")
            if outcome not in ("passed", "failed", "not_run", "unverified", "not_applicable"):
                outcome = "unknown"
            # Do not promote the model's text or an absent command receipt.
            ref = check.get("tool_call_ref")
            if outcome == "passed" and (not ref or actual != expected or not actual):
                outcome = "unverified"
            closure = closed_checks.get((node["assignment_ref"], check_index))
            if closure and actual and actual == expected:
                outcome = "passed"
            attempt = check_attempts.get((node["assignment_ref"], check_index), {}) if not closure else {}
            checks.append(dict(dict(name=check.get("name") or "Worker check", outcome=outcome,
                                    evidenceRef=closure if closure and actual == expected else reference if actual and actual == expected else None),
                               **attempt))

    if kind == "work" and review_status == "unknown" and current_review_waiver(db, task_id):
        review_status = "not_applicable"
    verification = db.execute("SELECT id,type,payload_json FROM runtime_events WHERE task_id=? "
                              "AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1",
                              (task_id,)).fetchone()
    verified = json.loads(verification["payload_json"]) if verification else {}
    verification_status = "not_applicable" if kind == "answer" else "not_run"
    observed = None
    if verification is not None:
        current = verified.get("revision") == revision
        verification_status = ("passed" if verification["type"] == "VERIFICATION_PASSED" else "failed") if current else "unverified"
        observed = verified.get("revision_digest") if current else None
        valid_digest = (isinstance(observed, str) and len(observed) == 64
                        and all(c in "0123456789abcdef" for c in observed))
        plan = json.loads(runtime["plan_json"]) if runtime["plan_json"] else {}
        expected_commands = plan.get("verification", [])
        observed_checks = verified.get("checks", [])
        for index in range(max(len(expected_commands), len(observed_checks))):
            check = observed_checks[index] if index < len(observed_checks) else None
            outcome = "not_run"
            if check is not None:
                matching = index < len(expected_commands) and check.get("argv") == expected_commands[index]
                if not current or not matching or check.get("complete") is not True:
                    outcome = "unverified"
                elif (type(check.get("exit_code")) is int and check["exit_code"] == 0
                      and valid_digest and check.get("revision_digest") == observed):
                    outcome = "passed"
                elif type(check.get("exit_code")) is int and check["exit_code"] != 0:
                    outcome = "failed"
                else:
                    outcome = "unverified"
            attempt_key = ("runtime-verification", index)
            attempt_row = latest_attempts(db, task_id, revision).get(attempt_key)
            attempt = check_attempts.get(attempt_key, {}) if attempt_row and attempt_row[0]["id"] > verification["id"] else {}
            checks.append(dict(dict(name="Runtime verification " + str(index + 1), outcome=outcome,
                                    evidenceRef=None, evidenceDigest=digest(check) if check is not None else None), **attempt))
        if verification_status == "passed" and (not valid_digest or
                any(c["outcome"] != "passed" for c in checks if c["name"].startswith("Runtime verification "))):
            verification_status = "unverified"
            warnings.append("verification_evidence_incomplete")
        if not valid_digest:
            observed = None
    elif kind == "work":
        plan = json.loads(runtime["plan_json"]) if runtime["plan_json"] else {}
        for index, _ in enumerate(plan.get("verification", [])):
            checks.append(dict(name="Runtime verification " + str(index + 1), outcome="not_run", evidenceRef=None))

    binding_valid = False
    if observed:
        observation = capture_revision(task_workspace.task_root(db, task_id))
        binding_valid = observation["kind"] == "observed" and observation["dirty_digest"] == observed
        if not binding_valid:
            warnings.append("workspace_drift" if observation["kind"] == "observed" else "workspace_unobservable")
    result_count = db.execute("SELECT count(*) FROM runtime_events WHERE task_id=? "
                              "AND type IN (\'ANSWER_SUBMITTED\',\'SYNTHESIS_SUBMITTED\')", (task_id,)).fetchone()[0]
    text = runtime["synthesis"]
    previous = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND actor='runtime' "
                          "AND type='WORKSPACE_REVIEW_MESSAGE' ORDER BY id DESC LIMIT 1", (task_id,)).fetchone()
    if previous and kind == "work" and binding_valid:
        retained = json.loads(previous[0])
        # Even a newly planned no-file follow-up must not erase the implementation.
        # This retains content only; the new plan/check evidence still governs acceptance.
        if retained.get("digest") == observed:
            text = retained["text"]
    # Execution versions also track host transcripts and progress. Those display-only
    # writes must not invalidate an unchanged reviewed result between render and Accept.
    # Runtime evidence events still advance this token; source drift is checked separately.
    result_version = db.execute("SELECT max(id) FROM runtime_events WHERE task_id=?", (task_id,)).fetchone()[0] or 1
    value = dict(sequenceNumber=str(max(1, result_count)), resultId=result_id, version=str(result_version), taskId=execution["task_id"],
                 executionId=execution_id, kind=kind, text=text,
                 evidence=evidence, checks=checks, observedWorkspaceDigest=observed if binding_valid else None,
                 recordedWorkspaceDigest=observed, workspaceBindingValid=binding_valid,
                 runtimeState=runtime["state"], reviewStatus=review_status, verificationStatus=verification_status,
                 label="Runtime result; not accepted or merged",
                 acceptanceAvailable=False, integrationAvailable=False, warnings=warnings)
    # Whole fields become explicitly unavailable, never partially truncated.
    if len(checks) > 100:
        value.update(checks=[], checksAvailability="unavailable")
        warnings.append("checks_unavailable:count_limit")
    if len(value["text"]) > 32000:
        value.update(text="", textAvailability="unavailable")
        warnings.append("text_unavailable:content_limit")
    from ..orchestration.worker_checks import targets
    target_checks = {t["projection_index"]: t for t in targets(db, task_id, revision)}
    attempts = latest_attempts(db, task_id, revision)
    for index, check in enumerate(checks):
        target = target_checks.get(index)
        if target:
            check["kind"] = target["kind"]
            check["detail"] = target.get("detail", "")
            check["command"] = target["argv"]
            check["cwd"] = target["cwd"]
            check["rerunnable"] = bool(target["argv"]) and not target.get("policy_blocker")
            if target.get("policy_blocker"):
                check.update(outcome="not_run", technicalReason=target["policy_blocker"])
            attempt = attempts.get((target["assignment"], target["index"]))
            if attempt:
                observed = attempt[1].get("result", {})
                runs = observed.get("checks", [])
                if len(runs) == 1:
                    from ..observability.redact import scrub_display_text
                    check["output"] = scrub_display_text(runs[0].get("output_tail") or "", max_chars=8000)
                    check["exitCode"] = runs[0].get("exit_code")
                    if runs[0].get("failure") == "timeout":
                        check["detail"] = "The check timed out after 60 seconds. Use a smaller check or request changes."
        elif check["outcome"] in ("not_run", "unverified"):
            check.update(kind="manual", rerunnable=False)
    value["acceptanceAvailable"] = result_actions(value)["accept"]
    try:
        return validate("Result", value)
    except ContractError as error:
        if str(error) != "page_too_large":
            raise
        value.update(text="", evidence=[], checks=[], acceptanceAvailable=False, textAvailability="unavailable",
                     evidenceAvailability="unavailable", checksAvailability="unavailable")
        warnings.append("result_content_unavailable:page_limit")
        return validate("Result", value)


class ReconnectState:
    """Pure consumer guard; projections/refetch, not events, supply entity data.

    A caller installs a fresh snapshot after any resync_required exception and
    displays that transition. This guard performs no I/O or lifecycle decisions;
    service/UI subscription wiring belongs to their own implementation.
    """
    def __init__(self, project_id, high_watermark, versions=None):
        validate("Identity", project_id)
        validate("Decimal", high_watermark)
        self.project_id = project_id
        self.sequence = int(high_watermark)
        self.versions = {}
        for identity, version in (versions or {}).items():
            validate("Identity", identity)
            validate("PositiveDecimal", version)
            self.versions[identity] = int(version)

    def accept(self, event):
        validate("Event", event)
        if event.get("projectId") != self.project_id:
            raise WorkspaceError("resync_required:project_mismatch")
        sequence = int(event["sequence"])
        if sequence <= self.sequence:
            return False
        if sequence != self.sequence + 1:
            raise WorkspaceError("resync_required:sequence_gap")
        version = int(event["entityVersion"])
        if version < self.versions.get(event["entityId"], 0):
            raise WorkspaceError("resync_required:backwards_version")
        self.sequence = sequence
        self.versions[event["entityId"]] = version
        return True
