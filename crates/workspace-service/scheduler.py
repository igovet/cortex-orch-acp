"""Window-independent queues in the service's owning ACP child; never replay starts."""
import json
import os
import time

import machine_tools as machine

# A result also depends on the project's files, which no event announces; this bounds how old that reading can be.
RESULT_MAX_AGE = 60.0
# How long a project may go without a full pass, for the same reason.
FULL_PASS_EVERY = 60.0


def query(api, op, target=None, **fields):
    return api.handle(dict(protocol=dict(major=1, minor=2), requestId="scheduler-read", projectId=api.project_id,
                           operation=op, target=target, **fields))


def _stat(path):
    try:
        seen = os.stat(path)
    except OSError:
        return None
    return seen.st_mtime_ns, seen.st_size


class Gate:
    """Says when a pass over the queues could find something new. A pass that cannot is not run.

    Everything a pass reads is covered: the runtime's rows move the project's version and journal,
    the owner's records live in the product file, and a pass's own writes show up as one more
    pass that then finds nothing to do. Files only: no database connection while nothing moved.
    """

    def __init__(self, api, state, *, clock=time.monotonic):
        self.api, self.state, self.clock = api, state, clock
        self.seen = None
        self.pending = None
        self.passed_at = None

    def _files(self):
        database = str(self.api.store.path)
        return _stat(database), _stat(database + "-wal"), _stat(machine.product_path(self.state, self.api.project_id))

    def _positions(self):
        with self.api.store.connection() as db:
            project = db.execute("SELECT version FROM workspace_projects WHERE project_id=?", (self.api.project_id,)).fetchone()
            journal = db.execute("SELECT high_watermark FROM workspace_outbox_state WHERE project_id=?", (self.api.project_id,)).fetchone()
        return project[0] if project else None, journal[0] if journal else 0

    def due(self, busy=False):
        """Read before the pass, so a change during the pass is seen by the next one."""
        now = self.clock()
        files = self._files()
        if self.seen is not None and not busy and self.passed_at is not None and now - self.passed_at < FULL_PASS_EVERY:
            if files == self.seen[0]:
                return False
            positions = self._positions()
            if positions == self.seen[1] and files[2] == self.seen[0][2]:
                # A write to some other record: nothing here depends on it.
                self.seen = (files, positions)
                return False
        else:
            positions = self._positions()
        self.pending = (files, positions)
        return True

    def done(self):
        self.seen, self.pending, self.passed_at = self.pending, None, self.clock()


def _result_of(api, task):
    """A result needs a walk of the project and its evidence, so an unchanged task is not read twice in a row."""
    held = api.__dict__.setdefault("_scheduler_results", {})
    known = held.get(task["taskId"])
    if known and known[0] == (task["executionId"], task["version"]) and time.monotonic() - known[1] < RESULT_MAX_AGE:
        return known[2]
    execution = query(api, "executions.get", task["executionId"])
    result = None
    if execution.get("resultId"):
        result = query(api, "results.get", execution["resultId"])
        if result["executionId"] != task["executionId"] or result["taskId"] != task["taskId"]:
            raise ValueError("scheduler_result_binding")
    held[task["taskId"]] = ((task["executionId"], task["version"]), time.monotonic(), result)
    return result


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
    # Only what this pass would act on: a closed task's result is settled and costs a walk of the project to read.
    glance = machine.product_of(state, api.project_id)
    planning = {s.get("planningTaskId") for s in glance["sprints"] if s["planning"] in ("planning", "question")}
    for task in tasks:
        if task["phase"] == "result" and task["executionId"]:
            record = glance["tasks"].get(task["taskId"], {})
            if (record.get("status") == "done" or record.get("purged")) and task["taskId"] not in planning:
                continue
            result = _result_of(api, task)
            if result is not None:
                results[task["taskId"]] = result
    starts = []
    with machine.Locked(state):
        product = machine.product_of(state, api.project_id)
        before = json.dumps(product, sort_keys=True)
        # Integration/owner closure is durable. Removing a merged checkout makes
        # its old evidence unavailable; that must not reopen Done or block Archive.
        for record in product["tasks"].values():
            if (record.get("status") == "done" or record.get("purged")) and record.get("automaticReady"):
                record["automaticReady"] = False
        for task_id, result in results.items():
            record = product["tasks"].setdefault(task_id, {})
            if record.get("status") == "done" or record.get("purged"):
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
            if sprint.get("archivedAt"):
                continue
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
        # Rewriting identical bytes would look like a change to the next pass and to any reader.
        if json.dumps(product, sort_keys=True) != before:
            machine.write_json(machine.product_path(state, api.project_id), product)
    for task in starts:
        # Close/Pause and a selected start serialize on the same owner-record lock.
        with machine.Locked(state):
            current = machine.product_of(state, api.project_id)
            owner = next((s for s in current["sprints"] if task["taskId"] in s["taskIds"]), None)
            if not owner or owner.get("archivedAt") or owner["queueState"] != "running":
                continue
            task = query(api, "tasks.get", task["taskId"])
            if task["executionId"] is not None:
                continue
            key = "sprint-start-" + task["taskId"] + "-" + task["version"]
            result = api.handle(dict(protocol=dict(major=1, minor=2), requestId=key, projectId=api.project_id, actorId=api.actor_id,
                                     operation="tasks.start", target=task["taskId"], expectedVersion=task["version"], ownerEpoch=None, payload={}))
        if result["outcome"] != "recorded":
            with machine.Locked(state):
                product = machine.product_of(state, api.project_id)
                for sprint in product["sprints"]:
                    if task["taskId"] in sprint["taskIds"]:
                        sprint.update(queueState="paused", pauseReason="Start was refused: " + result["outcome"])
                machine.write_json(machine.product_path(state, api.project_id), product)
