"""Explicit owner pause, independent of engine turn cancellation.

Only the service-bound owner adapter calls apply. Models cannot propose either
transition. The original lifecycle and questions remain recorded across restart.
"""
import json

from ..workspace_api.storage import WorkspaceError, append_public_tx, digest
from .state import TERMINAL, event, now


def saved_tx(db, task_id):
    row = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='OWNER_TASK_PAUSED' ORDER BY id DESC LIMIT 1",
                     (task_id,)).fetchone()
    return json.loads(row[0]) if row else None


def apply(api, execution_id, action, key, expected_version):
    fingerprint = digest([execution_id, action, expected_version])
    def write(db):
        api._project(db)
        execution = db.execute("SELECT e.*,t.version AS task_version FROM workspace_executions e JOIN workspace_tasks t USING(task_id) "
            "WHERE e.execution_id=? AND t.project_id=?", (execution_id, api.project_id)).fetchone()
        if execution is None:
            raise WorkspaceError("not_found")
        task_id = execution["runtime_task_id"]
        old = db.execute("SELECT digest,result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?",
                         (task_id, "workspace:" + api.actor_id, key)).fetchone()
        if old:
            if old["digest"] != fingerprint:
                raise WorkspaceError("payload_conflict")
            return dict(json.loads(old["result_json"]), changed=False)
        # Stop is an urgent intent for this exact execution, not an approval
        # of displayed content. Background progress must not make Stop fail.
        if action == "resume" and str(execution["task_version"]) != expected_version:
            raise WorkspaceError("stale_version")
        owner = db.execute("SELECT epoch FROM task_owners WHERE task_id=?", (task_id,)).fetchone()
        if not owner or owner[0] != execution["owner_epoch"]:
            raise WorkspaceError("stale_epoch")
        task = db.execute("SELECT * FROM runtime_tasks WHERE task_id=?", (task_id,)).fetchone()
        if not task or task["state"] in TERMINAL:
            raise WorkspaceError("task_not_resumable")
        before = task["state"]
        if action == "pause":
            if before == "PAUSED":
                raise WorkspaceError("already_paused")
            questions = [r[0] for r in db.execute("SELECT id FROM pending_questions WHERE task_id=? AND status='open'", (task_id,))]
            from . import worker_waits
            waits = [w for w in worker_waits.projection(db, task_id)
                     if w['status'] in {'open', 'ready'} and worker_waits.valid(db, w)]
            for wait in waits:
                worker_waits._change(db, wait, wait['status'], owner_paused=True, continuation_required=True)
                db.execute("UPDATE trusted_channels SET revoked_at=? WHERE channel_id=? AND revoked_at IS NULL",
                           (now(), wait['channel_id']))
            running = bool(db.execute("SELECT 1 FROM runtime_nodes WHERE task_id=? AND status=\'running\'", (task_id,)).fetchone())
            event(db, task_id, "OWNER_TASK_PAUSED", dict(previous=before, blocker=task["blocker"], questionIds=questions,
                waitIds=[w["wait_id"] for w in waits],
                resumeState="RECOVERING" if running and not waits and before != "WAITING_USER" else before))
            target, blocker = "PAUSED", task["blocker"]
        elif action == "resume":
            saved = saved_tx(db, task_id)
            if before != "PAUSED" or saved is None:
                raise WorkspaceError("task_not_paused")
            target, blocker = saved.get("resumeState", saved["previous"]), saved["blocker"]
            event(db, task_id, "OWNER_TASK_RESUMED", dict(state=target, questionIds=saved["questionIds"]))
        else:
            raise WorkspaceError("unsupported")
        db.execute("UPDATE runtime_tasks SET state=?,blocker=?,updated_at=? WHERE task_id=?", (target, blocker, now(), task_id))
        event(db, task_id, "TASK_STATE_CHANGED", dict(previous=before, state=target, blocker=blocker))
        # Runtime state and public invalidation versions commit in one transaction.
        db.execute("UPDATE workspace_executions SET version=version+1 WHERE execution_id=?", (execution_id,))
        db.execute("UPDATE workspace_tasks SET version=version+1 WHERE task_id=?", (execution["task_id"],))
        db.execute("UPDATE workspace_projects SET version=version+1 WHERE project_id=?", (api.project_id,))
        append_public_tx(db, project_id=api.project_id, entity_id=execution_id, kind="execution", version=execution["version"]+1)
        public = db.execute("SELECT version FROM workspace_tasks WHERE task_id=?", (execution["task_id"],)).fetchone()[0]
        append_public_tx(db, project_id=api.project_id, entity_id=execution["task_id"], kind="task", version=public)
        result = dict(outcome="recorded", state=target)
        db.execute("INSERT INTO runtime_requests VALUES (?,?,?,?,?,?)",
            (task_id, key, "workspace:" + api.actor_id, "owner_" + action, fingerprint, json.dumps(result)))
        return dict(result, changed=True)
    return api.store.immediate_write(write)
