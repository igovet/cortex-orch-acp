"""Restore saved sprint drafts, never import another channel's runtime history."""
from copy import deepcopy

import machine_tools as machine
from cortex_runtime.workspace_api import storage, task_options
from cortex_runtime.workspace_contract import validate


def materialized(sprint, product, project):
    """Use the same dependency order as the port that saved taskIds."""
    ids = sprint.get("taskIds") or []
    version = sprint.get("materializedVersion")
    plans = [p for p in [sprint.get("plan"), *(sprint.get("history") or [])]
             if isinstance(p, dict) and p.get("version") == version]
    if not ids or version is None or not plans or any(p != plans[0] for p in plans):
        raise ValueError("saved_plan_unavailable")
    items = plans[0].get("items")
    if sprint.get("projectId") != project or not isinstance(items, list) or len(items) != len(ids):
        raise ValueError("saved_plan_membership")
    for task in ids:
        validate("Identity", task)
    if len(set(ids)) != len(ids) or len({i["key"] for i in items}) != len(items):
        raise ValueError("saved_plan_membership")
    ordered, done = [], set()
    while len(ordered) < len(items):
        item = next((i for i in items if i["key"] not in done
                     and all(d in done for d in i["dependsOn"])), None)
        if item is None:
            raise ValueError("saved_plan_dependencies")
        ordered.append(item)
        done.add(item["key"])
    keys = {item["key"]: task for item, task in zip(ordered, ids)}
    restored = []
    for item, task in zip(ordered, ids):
        record = product["tasks"].get(task) or {}
        if (record.get("sprintId") != sprint["id"]
                or record.get("dependencies", []) != [keys[d] for d in item["dependsOn"]]):
            raise ValueError("saved_plan_membership")
        options = {k: deepcopy(record[k]) for k in ("settings",) if k in record}
        options["checkout"] = {"kind": (record.get("checkout") or {}).get("kind", "main")}
        validate("CreatePayload", dict(description=item["description"], **options))
        if not item["description"].strip():
            raise ValueError("saved_plan_description")
        restored.append((task, item["description"], options, record))
    return restored


def restore(api, state):
    """Repair only exact saved members; existing rows and all runtime evidence stay intact."""
    with machine.Locked(state):
        product = machine.product_of(state, api.project_id)
        for sprint in product["sprints"]:
            ids = sprint.get("taskIds") or []
            with api.store.connection() as db:
                existing = {r[0] for r in db.execute("SELECT task_id FROM workspace_tasks WHERE project_id=?",
                                                    (api.project_id,))}
            missing = [t for t in ids if t not in existing
                       and not product["tasks"].get(t, {}).get("purged")]
            if not missing:
                continue
            try:
                candidates = materialized(sprint, product, api.project_id)
            except (ValueError, KeyError, TypeError):
                sprint.update(queueState="paused", pauseKind="attention",
                              pauseReason="Saved sprint tasks are missing and the original plan cannot be matched safely.")
                machine.write_json(machine.product_path(state, api.project_id), product)
                continue
            # Persist the pause BEFORE insertion: a crash must never auto-start recovered drafts.
            sprint.update(queueState="paused", pauseKind="attention",
                          pauseReason="Tasks restored from the saved sprint plan. Previous execution history is unavailable in this channel. Review before starting.")
            for task, _, _, record in candidates:
                if task in missing:
                    for key in ("status", "acceptance", "automaticReady", "readyVersion", "integration"):
                        record.pop(key, None)
                    record["restoredFromPlan"] = True
            machine.write_json(machine.product_path(state, api.project_id), product)

            def write(db):
                api._project(db)
                for task, description, options, _ in candidates:
                    if task not in missing:
                        continue
                    if db.execute("SELECT 1 FROM workspace_tasks WHERE task_id=?", (task,)).fetchone():
                        continue
                    if (db.execute("SELECT 1 FROM workspace_executions WHERE task_id=?", (task,)).fetchone()
                            or db.execute("SELECT 1 FROM workspace_starts WHERE task_id=?", (task,)).fetchone()):
                        raise ValueError("saved_task_has_execution")
                    db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES (?,?,?,?)",
                               (task, api.project_id, description, storage.now()))
                    task_options.save(db, task, options)
                    db.execute("UPDATE workspace_projects SET version=version+1 WHERE project_id=?", (api.project_id,))
                    storage.append_public_tx(db, project_id=api.project_id, entity_id=task, kind="task", version=1)
            api.store.immediate_write(write)
