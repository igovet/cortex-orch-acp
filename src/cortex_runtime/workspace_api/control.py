"""Service-bound owner controls, never a model MCP operation."""
from ..acp.protocol import RpcError
from .storage import WorkspaceError
from .projections import _facts


def observe_owner(agent, session_id, session):
    """Advance display binding only from this runtime's exact active session."""
    from ..acp.agent import _current_owner_epoch_obj
    from .storage import append_public_tx
    if not agent._workspace_apis:
        return
    store = agent._store_for(session["project"])
    runtime = agent.runtime
    if hasattr(runtime, "_runtime_for_project"):
        runtime = runtime._runtime_for_project(session["project"])
    epoch = getattr(runtime, "active_session_owner_epoch", lambda *_: None)(session["task_id"], session_id)
    if epoch is None:
        return
    def write(db):
        current = _current_owner_epoch_obj(db, session["task_id"])
        if current is None or current.epoch != epoch:
            return None
        row = db.execute("SELECT e.*,t.project_id FROM workspace_executions e JOIN workspace_tasks t USING(task_id) "
                         "WHERE e.runtime_task_id=? AND e.acp_session_id=?", (session["task_id"], session_id)).fetchone()
        if row is not None and row["owner_epoch"] < epoch:
            db.execute("UPDATE workspace_executions SET owner_epoch=?,version=version+1 WHERE execution_id=?", (epoch,row["execution_id"]))
            db.execute("UPDATE workspace_tasks SET version=version+1 WHERE task_id=?", (row["task_id"],))
            db.execute("UPDATE workspace_projects SET version=version+1 WHERE project_id=?", (row["project_id"],))
            append_public_tx(db,project_id=row["project_id"],entity_id=row["execution_id"],kind="execution",version=row["version"]+1)
            version = db.execute("SELECT version FROM workspace_tasks WHERE task_id=?", (row["task_id"],)).fetchone()[0]
            append_public_tx(db,project_id=row["project_id"],entity_id=row["task_id"],kind="task",version=version)
        return current
    current = store.immediate_write(write)
    if current is not None:
        session["owner_epoch"] = current


def apply_settings(agent, api, session, session_id, task_id, chosen):
    """Serialize against prompts; explicitly report partial effects, never hide them."""
    import copy
    from . import task_options
    from ..workspace_contract import validate
    validate("CreatePayload", {"description": "options", "settings": chosen})
    fields = ["model", "effort", "language", "mode"]
    applied = []
    with session["lock"]:
        if session.get("active") is not None:
            raise WorkspaceError("busy")
        old = {key: copy.deepcopy(session.get(key)) for key in
               ("pending_selection", "default_selection", "pending_language", "workspace_explicit_selection")}
        try:
            agent._workspace_select(session, chosen)
            entries, _ = agent._selectable_entries_and_connections(session["project"])
            entry = next(e for e in entries if e.route.id == session["pending_selection"]["route_id"])
            success, _, error = agent._apply_mid_task_selection(api.store, session, entry, session["pending_selection"]["parameters"])
            if not success:
                raise error
            applied.extend(["model", "effort"])
            session["pending_selection"] = None
            success, _, error = agent._apply_language_selection(api.store, session, chosen["language"])
            if not success:
                raise error
            applied.append("language")
            success, _, error = agent._apply_access_mode_selection(api.store, session, session_id, chosen["mode"], origin="acp_config_option")
            if not success:
                raise error
            applied.append("mode")
        except Exception:
            # A failure may have committed before acknowledgement. Never replay it.
            session.update(old)
            return dict(outcome="partial_effect", applied=applied,
                        notApplied=[f for f in fields if f not in applied],
                        message="Settings were not fully acknowledged. Applied: " + (", ".join(applied) or "none confirmed") +
                                ". Not confirmed: " + ", ".join(f for f in fields if f not in applied) +
                                ". Refresh the session before retrying; a failed step may have committed.")
        def save(db):
            options = task_options.read(db, task_id)
            options["settings"] = chosen
            task_options.save(db, task_id, options)
        try:
            api.store.immediate_write(save)
        except Exception:
            return dict(outcome="partial_effect", applied=applied, notApplied=["saved display"],
                        message="Session settings applied, but their saved display could not be updated. Refresh before retrying.")
    return dict(outcome="recorded")


# Stop answers once the interrupted turn has recorded its end, at most this long after it.
PAUSE_SETTLE_SECONDS = 10


def settled(session, seconds=PAUSE_SETTLE_SECONDS):
    """Wait, bounded, until the interrupted turn has released the session.

    The turn records its end before releasing it, and those records move the task's
    public version. A window reads the task after Stop answers and sends that version
    with Resume, so Stop answers after them, never before.
    """
    import time
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        with session["lock"]:
            if session.get("active") is None:
                return True
        time.sleep(.05)
    return False


def handle(agent, params):
    from ..workspace_contract import validate
    from ..orchestration import command_control, worker_messages
    from ..acp.agent import _current_owner_epoch_obj
    import json

    project, execution_id = params.get("projectId"), params.get("executionId")
    api = agent._workspace_apis.get(project)
    if api is None:
        raise WorkspaceError("project_inactive")
    action = params.get("action")
    key = params.get("requestId")
    validate("Identity", key)
    with api.store.connection() as db:
        api._project(db)
        execution = db.execute("SELECT e.* FROM workspace_executions e JOIN workspace_tasks t USING(task_id) "
                               "WHERE t.project_id=? AND e.execution_id=?", (project, execution_id)).fetchone()
        if execution is None:
            raise WorkspaceError("not_found")
        from . import task_options
        if task_options.read(db, execution["task_id"]).get("deletingSprint") and action not in ("close_task", "command_stop"):
            raise WorkspaceError("task_deleting")
        runtime_row, mapping, _ = _facts(db, execution, api.store.project_root)
        if mapping.state == "unknown":
            raise WorkspaceError("stale_epoch")
        session_id, task_id = execution["acp_session_id"], execution["runtime_task_id"]
        owner = _current_owner_epoch_obj(db, task_id)
        session = agent.sessions.get(session_id)
        terminal = runtime_row is not None and runtime_row["state"] == "COMPLETED"
        if session is None and ((action == "resume" and mapping.state == "paused") or terminal):
            # Restore only this exact recorded session. This does not start an engine
            # or replay history; Resume is the subsequent explicit owner action.
            import threading
            from .. import task_workspace
            template = agent.sessions.get(getattr(agent, "_service_template", ""), {})
            session = dict(session_id=session_id, project=api.store.project_root,
                workspace=task_workspace.session_root(api.store, session_id) or api.store.project_root,
                task_id=task_id, channel=None, active=None, lock=threading.Lock(), owner_epoch=owner,
                pending_selection=None, default_selection=None, pending_language=None,
                pending_access_mode=None, client_capabilities=dict(agent.client_capabilities),
                mcp_servers=template.get("mcp_servers") or [])
            agent.sessions[session_id] = session
        if terminal and session is not None:
            # Restore only this exact settled result session, never a live owner.
            with session["lock"]:
                if (session.get("active") is None and session.get("task_id") == task_id
                        and session.get("project") == api.store.project_root):
                    session["owner_epoch"] = owner
                    session["channel"] = None
        # A failed open fences the coordinator before there is an engine/channel for
        # observe_owner to adopt. Only stopping that exact bound launch can use its
        # original host epoch; no prompt, resume or worker mutation gets this exception.
        launch_stop = (runtime_row is None and action in ("pause", "close_task") and session is not None
                       and session.get("project") == api.store.project_root
                       and getattr(session.get("owner_epoch"), "epoch", None) == execution["owner_epoch"])
        if session is None or session.get("task_id") != task_id or (session.get("owner_epoch") != owner and not launch_stop):
            raise WorkspaceError("execution_session_unavailable")
        worker = None
        if action in ("worker_message", "worker_stop"):
            worker = db.execute(
                "SELECT p.* FROM delegation_provisioning p JOIN trusted_channels c ON c.channel_id=p.channel_id "
                "JOIN task_owners o ON o.task_id=p.task_id AND o.epoch=p.owner_epoch "
                "WHERE p.task_id=? AND (p.assignment_id=? OR p.idempotency_key=? OR EXISTS "
                "(SELECT 1 FROM task_events e JOIN work_assignment_attempts a ON a.attempt_id=json_extract(e.payload_json,\'$.work_attempt_ref\') "
                "WHERE e.task_id=p.task_id AND e.assignment_ref=p.idempotency_key AND e.event_type=\'delegation_requested\' AND a.assignment_id=?)) AND c.revoked_at IS NULL "
                "ORDER BY p.created_at DESC LIMIT 1",
                (task_id, params.get("workerId"), params.get("workerId"), params.get("workerId"))).fetchone()
            if worker is None:
                raise WorkspaceError("worker_unavailable")
    if action in ("pause", "resume"):
        from ..orchestration import task_pause
        runtime = agent.runtime
        if hasattr(runtime, "_runtime_for_project"):
            runtime = runtime._runtime_for_project(session["project"])
        if runtime_row is None:
            # The runtime has not recorded the task yet: its engine is still opening or
            # retrying. There is no state to pause, so Stop cancels that launch outright.
            if action == "resume":
                raise WorkspaceError("task_not_resumable")
            if mapping.state == "stopped":
                return dict(outcome="recorded")
            runtime.cancel_task(task_id, session_id)
            return dict(outcome="recorded", _settle=lambda: settled(session))
        with session["lock"]:
            if action == "resume" and session.get("active") is not None:
                raise WorkspaceError("pause_settling")
            result = task_pause.apply(api, execution_id, action, key, params.get("expectedVersion"))
        if result["changed"] and action == "pause":
            agent._session_cancel(dict(sessionId=session_id))
            with api.store.connection() as db:
                live = db.execute("SELECT assignment_ref FROM runtime_nodes WHERE task_id=? AND status='running' AND assignment_ref IS NOT NULL", (task_id,)).fetchall()
            for row in live:
                runtime._cancel_assignment(task_id, row["assignment_ref"], "owner_paused", session_id=session_id)
            # The caller sends the reply after this, off its message reader.
            return dict({k: v for k, v in result.items() if k != "changed"}, _settle=lambda: settled(session))
        if result["changed"] and action == "resume" and result["state"] != "WAITING_USER":
            import threading
            cancel = threading.Event()
            with session["lock"]:
                session["active"] = cancel
            worker = threading.Thread(target=agent._run_queued_message_continuation,
                args=(session_id, session, "workspace-resume", key, "Continue from the recorded task state.", cancel),
                kwargs=dict(resume_recorded=True), name="acp-workspace-resume", daemon=True)
            with agent._workers_lock:
                agent._workers.append(worker)
            worker.start()
        return {k: v for k, v in result.items() if k != "changed"}
    if action == "close_task":
        if params.get("confirmed") is not True:
            raise WorkspaceError("confirmation_required")
        runtime = agent.runtime
        if hasattr(runtime, "_runtime_for_project"):
            runtime = runtime._runtime_for_project(session["project"])
        runtime.cancel_task(task_id, session_id)
        return dict(outcome="recorded")
    if mapping.state == "paused" and action != "command_stop":
        raise WorkspaceError("task_paused")
    if action == "task_settings":
        return apply_settings(agent, api, session, session_id, execution["task_id"], params.get("settings"))
    if action == "rerun_check" or action == "prompt":
        text = params.get("text")
        legacy_name = None
        if action == "prompt":
            validate("Text", text)
            # The prompt-based rerun control uses this one closed request template.
            # Resolve its name against retained checks; never parse executable text.
            import re
            match = re.fullmatch(r'Rerun the check "([^"\n]+)" for this result\. Do not skip it\. Use the retained exact command and working directory, and report the new evidence\. If it cannot run, report the concrete technical blocker; never mark it passed without evidence\.', text)
            legacy_name = match[1] if match else None
        if action == "rerun_check" or legacy_name is not None:
            from ..orchestration import worker_checks
            from .projections import result_projection
            runtime = agent.runtime
            if hasattr(runtime, "_runtime_for_project"):
                runtime = runtime._runtime_for_project(session["project"])
            with session["lock"]:
                if session.get("active") is not None:
                    raise WorkspaceError("busy")
                with api.store.connection() as db:
                    fingerprint = worker_checks._hash(params)
                    previous = db.execute("SELECT payload_json FROM runtime_events WHERE task_id=? "
                                          "AND type=\'WORKER_CHECK_REQUESTED\' AND json_extract(payload_json,\'$.request_key\')=?",
                                          (task_id, "owner-check:" + key)).fetchone()
                    if previous:
                        if json.loads(previous[0])["binding"].get("input_digest") != fingerprint:
                            raise WorkspaceError("payload_conflict")
                        return dict(outcome="recorded")
                    current = db.execute("SELECT revision FROM runtime_tasks WHERE task_id=?", (task_id,)).fetchone()
                    if current is None:  # still launching: there is no result or check yet
                        raise WorkspaceError("check_unavailable")
                    if action == "rerun_check":
                        result = result_projection(db, api.store, project, "wr_" + execution_id)
                        if (result["version"] != params.get("resultVersion") or
                                (result["observedWorkspaceDigest"] or f'result:{result["resultId"]}:{result["version"]}') != params.get("digest")):
                            raise WorkspaceError("stale_result")
                    candidates = worker_checks.targets(db, task_id, current[0])
                    name = legacy_name or params.get("checkName")
                    candidates = [t for t in candidates if t["name"] == name and
                                  (action == "prompt" or type(params.get("checkIndex")) is int and
                                   t["projection_index"] == params["checkIndex"])]
                    if len(candidates) != 1 or not candidates[0]["argv"] or candidates[0].get("policy_blocker"):
                        return dict(outcome="unsupported", message="This item has no runnable command or needs owner authorization. Check it manually; it has not passed.")
                import threading
                cancel = threading.Event()
                session["active"] = cancel
            try:
                return worker_checks.execute(runtime, task_id, candidates[0], "owner-check:" + key,
                                             cancel=cancel, emit_tool_call=agent._make_emit_tool_call(session_id, cancel),
                                             input_digest=fingerprint)
            finally:
                agent._finish_prompt(session_id, session, cancel)
    if action == "prompt":
        from .attachments import content
        if terminal:
            # Keep the reviewed implementation content separate from conversational replies.
            from .projections import result_projection, review_content_identity
            from ..orchestration.state import event
            def retain(db):
                current = db.execute("SELECT * FROM runtime_tasks WHERE task_id=?", (task_id,)).fetchone()
                if current["state"] != "COMPLETED" or not current["synthesis"]:
                    return
                prior = db.execute("SELECT 1 FROM runtime_events WHERE task_id=? AND type='WORKSPACE_REVIEW_MESSAGE' "
                                   "AND json_extract(payload_json,'$.requestId')=?", (task_id, key)).fetchone()
                if prior:
                    return
                result = result_projection(db, api.store, project, "wr_" + execution_id)
                if result["kind"] == "work" and result.get("workspaceBindingValid"):
                    event(db, task_id, "WORKSPACE_REVIEW_MESSAGE", dict(requestId=key,
                        revision=current["revision"], digest=result["observedWorkspaceDigest"],
                        identity=review_content_identity(db, task_id, current), text=result["text"]))
            api.store.immediate_write(retain)
        agent._queue_paseo_message(None, session_id, session, json.dumps(dict(
            clientMessageId=key, workspaceAttachments=True, content=[dict(type="text", text=text)] + content(params.get("attachments", [])))), respond=False)
        return dict(outcome="recorded")
    if action == "worker_message":
        return worker_messages.capture(api.store, task_id, worker["channel_id"], key, params.get("text"), attachments=params.get("attachments"))
    if action == "command_stop":
        return command_control.request_stop(api.store, task_id, params.get("toolCallRef"), params.get("reason"), key,
                                            origin="acp_command", session_id=session_id, owner_epoch=owner.epoch)
    if action == "worker_stop":
        runtime = agent.runtime
        if hasattr(runtime, "_runtime_for_project"):
            runtime = runtime._runtime_for_project(session["project"])
        reason = params.get("reason") or "Stopped by owner"
        if not isinstance(reason, str) or len(reason) > 2000:
            raise WorkspaceError("invalid_request")
        runtime._cancel_assignment(task_id, worker["idempotency_key"], reason, session_id=session_id)
        return dict(outcome="stop_requested")
    raise WorkspaceError("unsupported")
