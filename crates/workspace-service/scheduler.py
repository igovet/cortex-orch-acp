"""Window-independent queues in the service's owning ACP child; never replay starts."""
import json

import machine_tools as machine


def query(api, op, target=None, **fields):
    return api.handle(dict(protocol=dict(major=1, minor=2), requestId="scheduler-read", projectId=api.project_id,
                           operation=op, target=target, **fields))


def tick(api, state):
    # A bounded consistent page walk; any stale cursor aborts this tick before effects.
    snapshot = query(api, "project.snapshot")
    tasks = list(snapshot["tasks"]["items"])
    cursor = snapshot["tasks"]["nextCursor"]
    for _ in range(200):
        if cursor is None:
            break
        page = query(api, "tasks.list", cursor=cursor)
        tasks.extend(page["items"])
        cursor = page["nextCursor"]
    if cursor is not None:
        raise ValueError("scheduler_task_read_budget")
    by_id = {t["taskId"]: t for t in tasks}
    results = {}
    for task in tasks:
        if task["phase"] == "result" and task["executionId"]:
            execution = query(api, "executions.get", task["executionId"])
            if execution.get("resultId"):
                result = query(api, "results.get", execution["resultId"])
                if result["executionId"] != task["executionId"] or result["taskId"] != task["taskId"]:
                    raise ValueError("scheduler_result_binding")
                results[task["taskId"]] = result
    starts = []
    with machine.Locked(state):
        product = machine.product_of(state, api.project_id)
        for task_id, result in results.items():
            record = product["tasks"].setdefault(task_id, {})
            # Integration/owner closure is durable. Removing a merged checkout makes
            # its old evidence unavailable; that must not reopen Done or block Archive.
            if record.get("status") == "done" or record.get("purged"):
                record["automaticReady"] = False
                continue
            checks = result.get("checks") or []
            current = str(result.get("version"))
            passed = bool(checks) and all(c["outcome"] == "passed" for c in checks) and not result.get("warnings") and result.get("verificationStatus") == "passed" and result.get("workspaceBindingValid") is True
            previous = record.get("requiresFreshResult")
            if previous and previous == {"resultId": result.get("resultId"), "version": current}:
                passed = False
            elif passed and previous:
                record.pop("requiresFreshResult", None)
            if record.get("automaticReady") and (not passed or record.get("readyVersion") != current):
                record.pop("status", None)
                record.pop("acceptance", None)
                record["automaticReady"] = False
            if record.get("conflict") == "resolving" and passed:
                import task_git
                path = task_git.task_path(machine, api.store.project_root, record)
                if machine.out(path, "diff", "--name-only", "--diff-filter=U").strip() or machine.git(path, "rev-parse", "--verify", "MERGE_HEAD", check=False).returncode == 0:
                    passed = False
                else:
                    record.update(conflict="none", resolvedBy="cortex")
                    record["merge"].update(finished=True, finishedAt=machine.now_ms())
            if passed and record.get("status") != "done":
                record.update(status="ready_to_merge", automaticReady=True, readyVersion=current)
        for sprint in product["sprints"]:
            planning = results.get(sprint.get("planningTaskId"))
            if sprint["planning"] in ("planning", "question") and planning:
                import planning_jobs
                try:
                    parsed = planning_jobs.parse(planning["text"])
                    plan = {"version": 1, "items": parsed["items"]} if parsed["kind"] == "plan" else None
                except (ValueError, TypeError):
                    plan = None
                sprint.update(planning="ready" if plan else "cancelled", plan=plan, history=[plan] if plan else [])
                if not plan:
                    sprint.setdefault("conversation", []).append(dict(kind="event", text="The older planner returned no usable plan. Create a new sprint to plan without executing tasks.", at=machine.now_ms()))
            own = [by_id.get(t) for t in sprint["taskIds"]]
            # Owner completion settles dependencies, not verification. Never mask a live/unknown execution.
            own = [dict(t, phase="result") if t and t["phase"] in ("not_started", "result", "stopped", "failed")
                   and product["tasks"].get(t["taskId"], {}).get("closedAs") in ("external", "as_is")
                   and product["tasks"][t["taskId"]].get("status") == "done" else t for t in own]
            # Only a runtime-question pause is self-clearing. Owner pauses and
            # failed-check pauses must never restart merely because time passed.
            if sprint["queueState"] == "paused" and sprint.get("pauseKind") == "question":
                waiting = sprint.get("questionExecutions", {})
                if waiting and all(by_id.get(t) and by_id[t]["executionId"] == execution
                                   and by_id[t]["phase"] not in ("waiting", "recovery") for t, execution in waiting.items()):
                    sprint.update(queueState="running", pauseReason=None, pauseKind=None)
                    sprint.pop("questionExecutions", None)
            if sprint["queueState"] != "running":
                continue
            reason = None
            if sprint.get("runMode") != "automatic":
                reason = "This sprint is in manual mode."
            elif any(t is None for t in own):
                reason = "A sprint task is missing; review the queue."
            elif any(product["tasks"].get(t["taskId"], {}).get("closedAs") == "cancel" for t in own):
                reason = "A task was cancelled. Review the task before continuing the queue."
            elif any(t["phase"] in ("failed", "stopped", "blocked", "unknown", "recovery") for t in own):
                reason = "A task needs attention or has an unknown outcome."
            elif any(t["phase"] == "result" and product["tasks"].get(t["taskId"], {}).get("status") not in ("ready_to_merge", "done") for t in own):
                reason = "A result has missing or unsuccessful checks."
            elif any(product["tasks"].get(t["taskId"], {}).get("conflict") in ("conflict", "failed", "resolving") for t in own):
                reason = "Resolve the task conflict first."
            elif any(t["phase"] == "waiting" for t in own):
                sprint.update(queueState="paused", pauseReason="A task is waiting for an answer.", pauseKind="question",
                              questionExecutions={t["taskId"]: t["executionId"] for t in own if t["phase"] == "waiting"})
                continue
            if reason:
                sprint.update(queueState="paused", pauseReason=reason, pauseKind="attention")
                continue
            settled = {t["taskId"] for t in own if product["tasks"].get(t["taskId"], {}).get("status") in ("ready_to_merge", "done")}
            if len(settled) == len(own):
                sprint.update(queueState="complete", pauseReason=None)
                continue
            if any(t["phase"] not in ("not_started", "result") for t in own):
                continue
            next_task = next((t for t in own if t["phase"] == "not_started" and all(d in settled for d in product["tasks"].get(t["taskId"], {}).get("dependencies", []))), None)
            if next_task:
                starts.append(next_task)
            else:
                sprint.update(queueState="paused", pauseReason="No task can run; review dependencies.")
        machine.write_json(machine.product_path(state, api.project_id), product)
    for task in starts:
        key = "sprint-start-" + task["taskId"]
        result = api.handle(dict(protocol=dict(major=1, minor=2), requestId=key, projectId=api.project_id, actorId=api.actor_id,
                                 operation="tasks.start", target=task["taskId"], expectedVersion=task["version"], ownerEpoch=None, payload={}))
        if result["outcome"] != "recorded":
            with machine.Locked(state):
                product = machine.product_of(state, api.project_id)
                for sprint in product["sprints"]:
                    if task["taskId"] in sprint["taskIds"]:
                        sprint.update(queueState="paused", pauseReason="Start was refused: " + result["outcome"])
                machine.write_json(machine.product_path(state, api.project_id), product)
