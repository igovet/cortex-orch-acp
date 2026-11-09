"""Project-scoped Workspace draft/identity adapter (wire 1.0, mapping 1).

Construction is not activation. The host supplies trusted project and actor
bindings; client JSON cannot grant either. Saving never dispatches. Explicit
start requires a trusted host callback and a durable claim before prompt capture.
Results are read-only content, never owner acceptance or integration.
"""
import copy
import json

from ..workspace_contract import ContractError, capabilities, validate
from . import decisions, history, projections, storage
from .storage import (WorkspaceError, digest, new_identity, now, receipt,
                      record, replay, require_schema)

OPERATIONS = (
    "capabilities.get", "project.snapshot", "tasks.list", "tasks.get",
    "tasks.create", "executions.get", "events.page", "requests.get", "results.get",
    "decisions.list", "decisions.get", "history.page", "reports.read",
)


class WorkspaceAPI:
    def __init__(self, store, *, project_id, actor_id, build_identity="workspace-adapter-1", generation="1", dispatch_start=None, dispatch_answer=None):
        validate("Identity", project_id)
        validate("Identity", actor_id)
        if store.project_root is None:
            raise WorkspaceError("project_context_unavailable")
        self.store = store
        self.project_id = project_id
        self.actor_id = actor_id
        self.build_identity = build_identity
        self.generation = generation
        self.dispatch_start = dispatch_start
        self.dispatch_answer = dispatch_answer
        # The host's own answer to why a start cannot run now, asked before anything is recorded.
        self.start_blocker = None
        self._activated = False

    def activate(self, *, request_id, expected_version="0"):
        """Explicit host action; service-local, not a model or wire operation."""
        validate("Identity", request_id)
        validate("Decimal", expected_version)
        command_digest = digest(["activate", self.project_id, self.actor_id, expected_version])
        def write(db):
            require_schema(db)
            old = replay(db, self.project_id, self.actor_id, request_id, command_digest)
            if old is not None:
                return old
            project = db.execute("SELECT * FROM workspace_projects WHERE project_root=? OR project_id=?",
                                 (self.store.project_root, self.project_id)).fetchone()
            if project is not None:
                if (project["project_id"] != self.project_id or project["project_root"] != self.store.project_root
                        or project["activated_by"] != self.actor_id):
                    raise WorkspaceError("unauthorized")
                return record(db, self.project_id, self.actor_id, request_id, "projects.activate",
                              command_digest, receipt(request_id, "stale_version", self.project_id, project["version"]))
            if expected_version != "0":
                raise WorkspaceError("stale_version")
            at = now()
            db.execute("INSERT INTO workspace_projects VALUES (?,?,?,?,1)",
                       (self.project_id, self.store.project_root, self.actor_id, at))
            return record(db, self.project_id, self.actor_id, request_id, "projects.activate",
                          command_digest, receipt(request_id, "recorded", self.project_id, "1"))
        result = self.store.immediate_write(write)
        if result["outcome"] in ("recorded", "stale_version") and not self._activated:
            # A new controller observes old pending dispatches; it never replays them.
            def recover(db):
                self._project(db)
                for row in db.execute("SELECT * FROM workspace_starts WHERE project_id=? AND state!='unknown'",
                                      (self.project_id,)).fetchall():
                    execution = db.execute("SELECT * FROM workspace_executions WHERE execution_id=?",
                                           (row["execution_id"],)).fetchone()
                    if execution is not None:
                        _, mapping, _ = projections._facts(db, execution, self.store.project_root)
                        if mapping.state in ("result", "failed", "stopped", "paused"):
                            continue
                    storage.start_state_tx(db, row["execution_id"], "unknown")
            self.store.immediate_write(recover)
            self._activated = True
        return result

    def _project(self, db):
        require_schema(db)
        project = db.execute("SELECT * FROM workspace_projects WHERE project_id=?", (self.project_id,)).fetchone()
        if project is None:
            raise WorkspaceError("project_inactive")
        if project["project_root"] != self.store.project_root or project["activated_by"] != self.actor_id:
            raise WorkspaceError("unauthorized")
        return project

    def handle(self, request):
        request = copy.deepcopy(request)
        is_command = isinstance(request, dict) and "payload" in request
        validate("Command" if is_command else "Query", request)
        if request["protocol"]["major"] != 1:
            raise WorkspaceError("version_mismatch")
        if request["projectId"] != self.project_id or (is_command and request["actorId"] != self.actor_id):
            raise WorkspaceError("unauthorized")
        if is_command:
            if request["operation"] == "tasks.start" and self.dispatch_start is not None:
                return self._start(request)
            if request["operation"] == "decisions.answer" and self.dispatch_answer is not None:
                return self._answer(request)
            return self.store.immediate_write(lambda db: self._command(db, request))
        with self.store.connection() as db:
            db.execute("BEGIN")
            # All private facts, entity versions, pages and high-water mark
            # are read on this connection in the same SQLite snapshot.
            return self._query(db, request)

    def _start(self, command):
        prepared = []
        blocked = self.start_blocker() if self.start_blocker is not None else None
        def prepare(db):
            self._project(db)
            command_digest = digest(command)
            old = replay(db, self.project_id, self.actor_id, command["requestId"], command_digest)
            if old is not None:
                return old
            task = db.execute("SELECT * FROM workspace_tasks WHERE project_id=? AND task_id=?",
                              (self.project_id, command["target"])).fetchone()
            outcome = None
            if task is None:
                outcome = "invalid_request"
            elif command["ownerEpoch"] is not None:
                outcome = "stale_epoch"
            elif command["expectedVersion"] != str(task["version"]):
                outcome = "stale_version"
            elif task["execution_id"] is not None:
                outcome = "busy"
            elif blocked:
                outcome = "unsupported"
            else:
                for execution in db.execute("SELECT e.* FROM workspace_executions e "
                                            "JOIN workspace_tasks t USING(task_id) WHERE t.project_id=?",
                                            (self.project_id,)):
                    _, mapping, _ = projections._facts(db, execution, self.store.project_root)
                    if mapping.state not in ("result", "failed", "stopped"):
                        outcome = "busy"
                        break
                if db.execute("SELECT 1 FROM workspace_starts s LEFT JOIN workspace_executions e USING(execution_id) "
                              "WHERE s.project_id=? AND (e.execution_id IS NULL OR s.state='unknown')",
                              (self.project_id,)).fetchone():
                    outcome = "busy"
                # Ordinary ACP tasks share this checkout too. Missing lifecycle
                # evidence is not a terminal execution, unless the task was stopped
                # before its runtime row existed: a stopped task never opens an engine.
                if db.execute("SELECT 1 FROM tasks t LEFT JOIN runtime_tasks r ON r.task_id=t.id "
                              "WHERE t.project_root=? AND (r.state IS NULL OR r.state NOT IN "
                              "('COMPLETED','FAILED','CANCELLED')) AND NOT (r.state IS NULL AND EXISTS "
                              "(SELECT 1 FROM task_stopped s WHERE s.task_id=t.id)) LIMIT 1",
                              (self.store.project_root,)).fetchone():
                    outcome = "busy"
            if outcome:
                result = receipt(command["requestId"], outcome, command["target"],
                                 str(task["version"]) if task else "0",
                                 **({"reason": blocked} if blocked and outcome == "unsupported" else {}))
            else:
                execution_id = new_identity("we_")
                db.execute("INSERT INTO workspace_starts VALUES (?,?,?,?,?,?,?,'prepare',1,?)",
                           (execution_id, task["task_id"], self.project_id, self.actor_id, command["requestId"],
                            "workspace:" + self.project_id, new_identity("source_"), now()))
                db.execute("UPDATE workspace_tasks SET execution_id=? WHERE task_id=?",
                           (execution_id, task["task_id"]))
                storage.start_state_tx(db, execution_id, "prepare")
                version = db.execute("SELECT version FROM workspace_tasks WHERE task_id=?", (task["task_id"],)).fetchone()[0]
                result = receipt(command["requestId"], "recorded", task["task_id"], version, executionId=execution_id)
                prepared.append(execution_id)
            return record(db, self.project_id, self.actor_id, command["requestId"], "tasks.start", command_digest, result)
        result = self.store.immediate_write(prepare)
        if prepared:
            # Both boundaries commit before any prompt/engine side effect.
            start = storage.claim_start(self.store, prepared[0])
            try:
                self.dispatch_start(start)
            except Exception:
                self.store.immediate_write(lambda db: storage.start_state_tx(db, prepared[0], "unknown"))
            else:
                def dispatched(db):
                    state = db.execute("SELECT state FROM workspace_starts WHERE execution_id=?", (prepared[0],)).fetchone()[0]
                    if state == "claimed":
                        storage.start_state_tx(db, prepared[0], "dispatched")
                self.store.immediate_write(dispatched)
        return result

    def _answer(self, command):
        from . import attachments
        from ..acp import replay_store
        items = [item for a in command["payload"]["answers"] for item in a.get("attachments", [])]
        attachments.checked(items)
        blocks = attachments.stage(self.store.project_root, items) if items else []
        rollbacks, staged = [], [b.staged_path for b in blocks if b.staged_path is not None]
        handoff = []
        def write(db):
            self._project(db)
            command_digest = digest(command)
            old = replay(db, self.project_id, self.actor_id, command["requestId"], command_digest)
            if old is not None:
                return old
            try:
                answer = decisions.answer_tx(db, self.project_id, command)
            except WorkspaceError as error:
                outcome = str(error)
                if outcome not in {"stale_version", "stale_epoch", "already_resolved", "superseded", "unsupported"}:
                    outcome = "invalid_request"
                execution = db.execute(
                    "SELECT e.version FROM workspace_executions e JOIN workspace_tasks t USING(task_id) "
                    "WHERE t.project_id=? AND e.execution_id=?",
                    (self.project_id, command["payload"]["executionId"])).fetchone()
                result = receipt(command["requestId"], outcome, command["target"],
                                 str(execution["version"]) if execution else "0")
            else:
                if blocks:
                    attachments.answer_source_tx(self.store, db, answer["task_id"],
                                                 digest([self.project_id, self.actor_id, command["requestId"]]),
                                                 "\n".join(a["answer"] for a in answer["answers"]),
                                                 blocks, rollbacks, staged)
                version = db.execute("SELECT version FROM workspace_executions WHERE execution_id=?",
                                     (answer["execution_id"],)).fetchone()[0]
                result = receipt(command["requestId"], "recorded", command["target"], version,
                                 executionId=answer["execution_id"], ownerEpoch=command["ownerEpoch"])
                handoff.append(answer)
            return record(db, self.project_id, self.actor_id, command["requestId"],
                          command["operation"], command_digest, result)
        try:
            result = self.store.immediate_write(write)
        except BaseException:
            self.store._rollback_files(rollbacks)
            raise
        else:
            self.store._finalize_files(rollbacks)
        finally:
            replay_store.cleanup_staged(blocks)
        if handoff:
            # Exactly the winning committed transaction hands off. Replays only
            # return its immutable receipt, even after the owner/session changes.
            try:
                self.dispatch_answer(handoff[0])
            except Exception as error:
                raise WorkspaceError("answer_continuation_unconfirmed") from error
        return result

    def _command(self, db, command):
        self._project(db)
        command_digest = digest(command)
        old = replay(db, self.project_id, self.actor_id, command["requestId"], command_digest)
        if old is not None:
            return old
        operation = command["operation"]
        if operation not in OPERATIONS:
            result = receipt(command["requestId"], "unsupported", command["target"])
        elif command["target"] != self.project_id or command["ownerEpoch"] is not None:
            result = receipt(command["requestId"], "invalid_request", command["target"])
        else:
            from .attachments import checked
            checked(command["payload"].get("attachments", []))
            description = command["payload"]["description"]
            if not description.strip():
                raise WorkspaceError("description_required")
            task_id = new_identity("wt_")
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES (?,?,?,?)",
                       (task_id, self.project_id, description, now()))
            from . import task_options
            task_options.save(db, task_id, {key: value for key, value in command["payload"].items() if key != "description"})
            db.execute("UPDATE workspace_projects SET version=version+1 WHERE project_id=?", (self.project_id,))
            storage.append_public_tx(db, project_id=self.project_id, entity_id=task_id, kind="task", version=1)
            result = receipt(command["requestId"], "recorded", task_id, "1")
        return record(db, self.project_id, self.actor_id, command["requestId"], operation, command_digest, result)

    def _query(self, db, request):
        project = self._project(db)
        operation = request["operation"]
        if operation not in OPERATIONS:
            raise WorkspaceError("unsupported")
        if operation == "capabilities.get":
            result = capabilities(build_identity=self.build_identity, generation=self.generation,
                                  operations=OPERATIONS + (("tasks.start",) if self.dispatch_start is not None else ())
                                  + (("decisions.answer",) if self.dispatch_answer is not None else ()))
            result["attachments"] = self.dispatch_answer is not None
            return result
        if operation in ("tasks.list", "project.snapshot"):
            if operation == "project.snapshot" and (request.get("cursor") is not None or request.get("limit") is not None):
                raise WorkspaceError("invalid_request")
            if request.get("target") not in (None, self.project_id):
                raise WorkspaceError("invalid_request")
            page = projections.task_page(db, project, limit=request.get("limit", 50),
                                         cursor=request.get("cursor"), target=request.get("target"))
            if operation == "tasks.list":
                return page
            return validate("ProjectSnapshot", dict(projectId=self.project_id, version=str(project["version"]),
                                                     highWatermark=page["highWatermark"], eventCursor=page["eventCursor"],
                                                     retainedFloor=page["retainedFloor"], tasks=page))
        if operation == "events.page":
            if request.get("target") not in (None, self.project_id):
                raise WorkspaceError("invalid_request")
            return storage.event_page(db, self.project_id, cursor=request.get("cursor"), limit=request.get("limit", 100))
        if operation == "reports.read":
            from .reports import page_tx
            return page_tx(db, self.store, self.project_id, request["target"],
                           cursor=request.get("cursor"), limit=request.get("limit", 4000))
        if operation == "history.page":
            return history.page_tx(db, self.project_id, request.get("target"),
                                   limit=request.get("limit", 100), cursor=request.get("cursor"))
        if operation == "decisions.list":
            if request.get("target") is None:
                raise WorkspaceError("invalid_request")
            return decisions.page_tx(db, self.project_id, request["target"], limit=request.get("limit", 100),
                                     cursor=request.get("cursor"), answer_enabled=self.dispatch_answer is not None)
        if request.get("cursor") is not None or request.get("limit") is not None:
            raise WorkspaceError("invalid_request")
        target = request.get("target")
        if target is None:
            raise WorkspaceError("invalid_request")
        if operation == "decisions.get":
            return decisions.get_tx(db, self.project_id, target, answer_enabled=self.dispatch_answer is not None)
        if operation == "tasks.get":
            task = db.execute("SELECT * FROM workspace_tasks WHERE project_id=? AND task_id=?",
                              (self.project_id, target)).fetchone()
            if task is None:
                raise WorkspaceError("not_found")
            return projections.task_projection(db, task, project["project_root"])
        if operation == "executions.get":
            execution = db.execute(
                "SELECT e.* FROM workspace_executions e JOIN workspace_tasks t USING(task_id) "
                "WHERE t.project_id=? AND e.execution_id=?", (self.project_id, target)).fetchone()
            if execution is None:
                start = db.execute("SELECT * FROM workspace_starts WHERE project_id=? AND execution_id=?",
                                   (self.project_id, target)).fetchone()
                if start is None:
                    raise WorkspaceError("not_found")
                return validate("Execution", dict(executionId=target, taskId=start["task_id"],
                    version=str(start["version"]), runtimeTaskId=None, acpSessionId=None,
                    runtimePlanRevision=None, ownerEpoch=None, state="unknown", warnings=["dispatch_unconfirmed"]))
            return projections.execution_projection(db, execution, project["project_root"])
        if operation == "results.get":
            return projections.result_projection(db, self.store, self.project_id, target)
        if operation == "requests.get":
            row = db.execute("SELECT receipt_json FROM workspace_requests "
                             "WHERE project_id=? AND actor_id=? AND request_id=?",
                             (self.project_id, self.actor_id, target)).fetchone()
            if row is None:
                raise WorkspaceError("not_found")
            return validate("Receipt", json.loads(row["receipt_json"]))
        raise WorkspaceError("unsupported")
