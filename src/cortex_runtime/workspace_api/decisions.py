"""Public question identities over existing runtime answer transactions."""
import json

from ..contracts import StoreError
from ..orchestration import question_batches, worker_waits
from ..workspace_contract import ContractError, validate
from . import projections
from .storage import WorkspaceError, digest, outbox_position


def execution_tx(db, project_id, execution_id):
    row = db.execute("SELECT e.*,p.project_root FROM workspace_executions e "
                     "JOIN workspace_tasks t USING(task_id) JOIN workspace_projects p USING(project_id) "
                     "WHERE t.project_id=? AND e.execution_id=?", (project_id, execution_id)).fetchone()
    if row is None:
        raise WorkspaceError("not_found")
    return row


def question_tx(db, project_id, decision_id):
    row = db.execute("SELECT q.*,e.execution_id FROM pending_questions q "
                     "JOIN workspace_executions e ON e.runtime_task_id=q.task_id "
                     "JOIN workspace_tasks t ON t.task_id=e.task_id "
                     "WHERE t.project_id=? AND q.id=?", (project_id, decision_id.removeprefix("wd_"))).fetchone()
    if row is None or decision_id != "wd_" + row["id"]:
        raise WorkspaceError("not_found")
    return row


def _wait(db, question, batch):
    waits = worker_waits.projection(db, question["task_id"],
                                   question_id=batch["question_id"] if batch else question["id"])
    return waits[-1] if waits else None


def projection_tx(db, execution, question, *, answer_enabled):
    task_id = execution["runtime_task_id"]
    batch = question_batches.find_tx(db, task_id, question_id=question["id"])
    wait = _wait(db, question, batch)
    # Access/policy and capacity decisions have their own runtime authority.
    if wait and wait.get("kind", "question") != "question":
        raise WorkspaceError("unsupported")
    runtime, mapping, _ = projections._facts(db, execution, execution["project_root"])
    session = db.execute("SELECT closed_at FROM acp_host_sessions WHERE session_id=?",
                         (execution["acp_session_id"],)).fetchone()
    valid = mapping.state != "unknown" and session is not None and not session["closed_at"]
    valid = valid and not db.execute("SELECT 1 FROM task_stopped WHERE task_id=?", (task_id,)).fetchone()
    if wait:
        valid = valid and wait["status"] == "open" and worker_waits.valid(db, wait)
    elif question["assignment_ref"]:
        valid = False
    else:
        latest = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? "
                            "AND type='USER_QUESTION_REQUESTED' ORDER BY id DESC LIMIT 1", (task_id,)).fetchone()
        saved = json.loads(latest[0]) if latest else {}
        from ..orchestration.task_pause import saved_tx
        paused = saved_tx(db, task_id) if runtime["state"] == "PAUSED" else None
        valid = valid and (runtime["state"] == "WAITING_USER" or bool(paused and paused["previous"] == "WAITING_USER"))
        valid = valid and saved.get("question_id") == (batch["question_id"] if batch else question["id"])
    if batch:
        valid = valid and runtime is not None and runtime["revision"] == batch["revision"]
    status = {"open": "pending", "answered": "answered", "superseded": "superseded",
              "cancelled": "superseded"}.get(question["status"], "unknown")
    if status == "pending" and not valid:
        status = "superseded"
    requirement = db.execute("SELECT requirement_revision FROM task_orchestration WHERE task_id=?",
                             (task_id,)).fetchone()
    labels = json.loads(question["options_json"]) if question["options_json"] else []
    options = [dict(optionId="wo_" + digest([question["id"], i, label])[:40], label=label)
               for i, label in enumerate(labels)]
    # Conservative per-decision CAS: each decision inherits its enclosing
    # execution's monotonic public invalidation version, never a plan revision.
    value = dict(createdAt=question["created_at"], answeredAt=question["answered_at"], answerText=question["answer_text"], decisionId="wd_" + question["id"], kind="question_batch" if batch else "question",
                 questionId=question["id"], batchId=batch["batch_id"] if batch else None,
                 executionId=execution["execution_id"], acpSessionId=execution["acp_session_id"],
                 scope=question["assignment_ref"] or "task", prompt=question["text"], options=options,
                 version=str(execution["version"]), ownerEpoch=str(execution["owner_epoch"]),
                 requirementRevision=str(requirement[0] if requirement else 0), status=status,
                 allowedOperations=["decisions.answer"] if answer_enabled and status == "pending" and runtime["state"] != "PAUSED" else [])
    if batch:
        value["questionIds"] = [q["question_id"] for q in batch["questions"]]
    try:
        return validate("Decision", value)
    except ContractError as error:
        if str(error) != "page_too_large":
            raise
        value.update(prompt="", options=[], allowedOperations=[], contentAvailability="unavailable")
        return validate("Decision", value)


def get_tx(db, project_id, decision_id, *, answer_enabled):
    question = question_tx(db, project_id, decision_id)
    execution = execution_tx(db, project_id, question["execution_id"])
    return projection_tx(db, execution, question, answer_enabled=answer_enabled)


def page_tx(db, project_id, target, *, limit=100, cursor=None, answer_enabled):
    if type(limit) is not int or not 1 <= limit <= 100:
        raise WorkspaceError("invalid_request")
    execution = execution_tx(db, project_id, target)
    binding = digest([project_id, target, str(execution["version"]), limit])[:32]
    after = 0
    if cursor is not None:
        parts = cursor.split(".")
        if (len(parts) != 3 or parts[:2] != ["wd1", binding] or not parts[2].isascii()
                or not parts[2].isdecimal() or len(parts[2]) > 19 or str(int(parts[2])) != parts[2]
                or int(parts[2]) > 9223372036854775807):
            raise WorkspaceError("invalid_cursor")
        after = int(parts[2])
    rows = db.execute("SELECT rowid AS position,* FROM pending_questions WHERE task_id=? AND rowid>? "
                      "ORDER BY rowid LIMIT ?", (execution["runtime_task_id"], after, limit + 1)).fetchall()
    page = dict(items=[], nextCursor=None, highWatermark=str(outbox_position(db, project_id)[0]))
    for row in rows:
        try:
            item = projection_tx(db, execution, row, answer_enabled=answer_enabled)
        except WorkspaceError as error:
            if str(error) == "unsupported":
                after = row["position"]
                continue
            raise
        if len(page["items"]) == limit:
            break
        # Reserve the actual continuation cursor's bytes before admitting an
        # item. A DTO fitting alone can still exceed the enclosing page.
        candidate = dict(page, items=page["items"] + [item],
                         nextCursor="wd1." + binding + "." + str(row["position"]))
        try:
            validate("DecisionPage", candidate)
        except ContractError as error:
            if str(error) != "page_too_large":
                raise
            if page["items"]:
                break
            item.update(prompt="", options=[], allowedOperations=[], contentAvailability="unavailable")
            validate("DecisionPage", dict(candidate, items=[item]))
        page["items"].append(item)
        after = row["position"]
    if rows and after < rows[-1]["position"]:
        page["nextCursor"] = "wd1." + binding + "." + str(after)
    elif len(rows) == limit + 1:
        page["nextCursor"] = "wd1." + binding + "." + str(after)
    return validate("DecisionPage", page)


def answer_tx(db, project_id, command):
    """Validate every member before sharing the existing answer transaction."""
    payload = command["payload"]
    execution = execution_tx(db, project_id, payload["executionId"])
    if command["ownerEpoch"] != str(execution["owner_epoch"]):
        raise WorkspaceError("stale_epoch")
    owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (execution["runtime_task_id"],)).fetchone()
    if owner is None or command["ownerEpoch"] != str(owner[0]):
        raise WorkspaceError("stale_epoch")
    if payload["acpSessionId"] != execution["acp_session_id"]:
        raise WorkspaceError("invalid_request")
    batch = question_batches.find_tx(db, execution["runtime_task_id"], batch_id=payload["batchId"]) if payload["batchId"] else None
    if payload["batchId"]:
        if batch is None or command["target"] != batch["batch_id"]:
            raise WorkspaceError("invalid_request")
        ids = [q["question_id"] for q in batch["questions"]]
    else:
        question = question_tx(db, project_id, command["target"])
        if question["task_id"] != execution["runtime_task_id"]:
            raise WorkspaceError("invalid_request")
        if question_batches.find_tx(db, question["task_id"], question_id=question["id"]):
            raise WorkspaceError("invalid_request")
        ids = [question["id"]]
    answers = payload["answers"]
    if len(answers) != len(ids) or {a["questionId"] for a in answers} != set(ids):
        raise WorkspaceError("invalid_request")
    converted = []
    for item in answers:
        question = question_tx(db, project_id, item["decisionId"])
        if question["id"] != item["questionId"] or question["task_id"] != execution["runtime_task_id"]:
            raise WorkspaceError("invalid_request")
        decision = projection_tx(db, execution, question, answer_enabled=True)
        if decision["status"] == "answered":
            raise WorkspaceError("already_resolved")
        if decision["status"] != "pending":
            raise WorkspaceError("superseded")
        if command["expectedVersion"] != decision["version"] or item["expectedVersion"] != decision["version"]:
            raise WorkspaceError("stale_version")
        if not decision["allowedOperations"]:
            raise WorkspaceError("unsupported")
        text = item["text"]
        if item["optionId"] is not None:
            selected = next((o for o in decision["options"] if o["optionId"] == item["optionId"]), None)
            if selected is None or (text and text != selected["label"]):
                raise WorkspaceError("invalid_request")
            text = selected["label"]
        if not text.strip():
            raise WorkspaceError("invalid_request")
        converted.append(dict(questionId=question["id"], answer=text))
    # Namespaced keys cannot collide with existing card submissions.
    key = "workspace:" + digest([project_id, command["actorId"], command["requestId"]])
    kwargs = dict(session_id=execution["acp_session_id"], owner_epoch=int(command["ownerEpoch"]))
    db.execute("SAVEPOINT workspace_answer")
    try:
        if batch:
            question_batches.answer_batch_tx(db, execution["runtime_task_id"], batch["batch_id"], converted, key, **kwargs)
        else:
            question_batches.answer_single_tx(db, execution["runtime_task_id"], ids[0], converted[0]["answer"], key, **kwargs)
    except StoreError as error:
        db.execute("ROLLBACK TO workspace_answer")
        raise WorkspaceError("superseded") from error
    finally:
        db.execute("RELEASE workspace_answer")
    return dict(task_id=execution["runtime_task_id"], session_id=execution["acp_session_id"],
                question_ids=ids, answers=converted, execution_id=execution["execution_id"])
