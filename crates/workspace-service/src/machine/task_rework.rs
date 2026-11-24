//! Deliberate owner rework, separate from the late-message and result-review paths.
use super::{
    context, git,
    native::*,
    settings::{self, Lock},
};
use cortex_core::{
    workspace::control::{self, Binding, ReworkDisposition},
    CoreError, CoreResult, Store,
};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::{collections::BTreeSet, path::Path};

pub(crate) fn error(code: &'static str) -> Refused {
    let message = match code {
        "task_archived" => "Restore the task first.",
        "task_not_closed" => "Only a closed task can be returned for rework.",
        "stale_version" | "stale_epoch" => "The task changed. Refresh it before sending.",
        "merge_in_progress" => "Finish or abort the current merge first.",
        "rework_pending" => "A task returned for rework is not closed yet.",
        "invalid_message" => "Write a rework message first, up to 32000 characters.",
        "invalid_attachments" | "invalid_attachment" => "The attachments could not be read.",
        "write_rule_conflict" => "One message cannot both allow and deny writes to the same path.",
        "rework_target_unavailable" => "The task's target branch is unavailable.",
        "rework_target_not_checked_out" => {
            "Check out the task's target branch in the project folder first."
        }
        "rework_target_changed" => "The target branch changed. Refresh before sending.",
        "worktree_unavailable" => {
            "Cortex cannot verify the original task checkout or its retained branch. The task was not reopened."
        }
        "native_execution_binding_invalid" | "contradictory_evidence" => {
            "Cortex cannot confirm which execution to reopen."
        }
        "lost_worker_termination_unconfirmed" | "native_shutdown_unconfirmed" => {
            "Wait for every worker of this task to stop first."
        }
        "native_outside_outcome_unknown" => {
            "A previous action has an unknown outcome. Check the task activity first."
        }
        "project_inactive" => "Activate the project before returning a task for rework.",
        "native_task_route_unavailable" | "native_route_unavailable" => {
            "The task's coordinator could not be started. Check its model settings and try again."
        }
        "payload_conflict" => "This send already has different content.",
        _ => "The task could not be returned for rework.",
    };
    refuse(code, message)
}

fn closed(record: &Value) -> bool {
    matches!(
        record["status"].as_str(),
        Some("done" | "closed_unverified")
    )
}

fn archived(record: &Value) -> bool {
    record["archive"].is_object()
}

fn active_merge(record: &Value) -> bool {
    let merge = &record["merge"];
    merge.is_object()
        && !(merge["finished"] == true
            && merge["gitState"] == "completed"
            && record["conflict"] == "none")
}

fn normalize(record: &mut Value, event: Option<i64>) -> bool {
    let Some(event) = event.map(|id| id.to_string()) else {
        return false;
    };
    if record["reworkEvent"] == event {
        return false;
    }
    if !record.is_object() {
        *record = json!({});
    }
    for key in [
        "status",
        "doneAt",
        "closedAs",
        "alreadyInTargetBranch",
        "notVerifiedByCortex",
        "acceptance",
        "automaticReady",
        "acceptedRevision",
        "workspaceDigest",
        "integration",
        "work",
        "worktreeCleanup",
        "archiveQuestion",
        "archiveAskAfter",
        "merge",
        "conflict",
        "result",
        "reviewActions",
        "comparisonBaseRevision",
        "baseRevision",
        "reworkAvailable",
        "reworkUnavailableReason",
        "reworkWarnings",
        "reworkVersion",
        "reworkOwnerEpoch",
        "reworkExecutionId",
    ] {
        record.as_object_mut().unwrap().remove(key);
    }
    record["reworkEvent"] = json!(event);
    true
}

fn reopen_sprint(product: &mut Value, task: &str) {
    let id = product["tasks"][task]["sprintId"].clone();
    if id.is_null() {
        return;
    }
    for sprint in product["sprints"].as_array_mut().into_iter().flatten() {
        if sprint["id"] != id {
            continue;
        }
        if sprint["queueState"] == "complete" || sprint["automaticQueue"]["taskId"] == task {
            sprint["queueState"] = json!("paused");
            sprint["pauseReason"] = json!("A task was returned for rework.");
            sprint.as_object_mut().unwrap().remove("automaticQueue");
        }
    }
}

fn started_dependents(product: &Value, task: &str, started: &BTreeSet<String>) -> bool {
    let id = &product["tasks"][task]["sprintId"];
    if id.is_null() {
        return false;
    }
    let Some(members) = product["sprints"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|s| s["id"] == *id)
        .and_then(|s| s["taskIds"].as_array())
    else {
        return false;
    };
    // Walk reverse edges, including transitive dependents, without trusting the
    // saved graph to be acyclic. A task itself is never its own warning.
    let mut reached = BTreeSet::from([task]);
    let mut pending = vec![task];
    while let Some(parent) = pending.pop() {
        for id in members.iter().filter_map(Value::as_str) {
            if !reached.contains(id)
                && product["tasks"][id]["dependencies"]
                    .as_array()
                    .is_some_and(|deps| deps.iter().any(|dep| dep == parent))
            {
                if started.contains(id) {
                    return true;
                }
                reached.insert(id);
                pending.push(id);
            }
        }
    }
    false
}

/// Shared by the board, detailed product read, and changed-task admission.
/// Read-only fall back to the durable event if the private overlay save was lost.
pub(super) fn project(
    store: &Store,
    project: &str,
    product: &mut Value,
    decorate: bool,
) -> Result<()> {
    let root = store.key().root().to_string_lossy();
    store.read(|db| {
        let mut query = db.prepare(
            "SELECT t.task_id,t.version,t.execution_id,e.runtime_task_id,r.state FROM workspace_tasks t
             JOIN workspace_projects p USING(project_id)
             LEFT JOIN workspace_executions e ON e.execution_id=t.execution_id AND e.task_id=t.task_id
             LEFT JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
             WHERE t.project_id=? AND p.project_root=? AND p.activated_by=?
             ORDER BY t.sequence LIMIT 10001",
        )?;
        let rows = query.query_map((project, &*root, crate::actor()), |r| Ok((
            r.get::<_, String>(0)?, r.get::<_, i64>(1)?,
            r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?,
            r.get::<_, Option<String>>(4)?,
        )))?.collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.len() > 10000 { return Err(CoreError::Refused("machine_reply_too_large")); }
        for (task, _, _, runtime, _) in &rows {
            if let Some(runtime) = runtime {
                normalize_on(db, project, product, task, runtime)?;
            }
        }
        if !decorate { return Ok(()); }
        let started = rows.iter().filter(|r| r.2.is_some()).map(|r| r.0.clone()).collect();
        for (task, version, execution, runtime, state) in rows {
            let record = &product["tasks"][&task];
            let recorded_close = match &execution {
                Some(execution) => control::unconfirmed_close_current(db, project, &root, &task, execution)?,
                None => false,
            };
            let closed = closed(record) || recorded_close;
            let resolved = match (&state, &runtime) {
                (Some(state), Some(runtime)) if state == "COMPLETED" => control::owner_resolution_current(db, runtime)?,
                _ => false,
            };
            if !closed && !archived(record) && record["reworkEvent"].is_null() && !resolved
                && !matches!(state.as_deref(), Some("CANCELLED" | "FAILED"))
            {
                continue;
            }
            let decision = if archived(record) {
                Err(CoreError::Refused("task_archived"))
            } else if record["purged"] == true {
                Err(CoreError::Refused("invalid_task"))
            } else if active_merge(record) {
                Err(CoreError::Refused("merge_in_progress"))
            } else {
                match (&execution, &runtime) {
                    (Some(execution), Some(runtime)) =>
                        control::rework_binding_for_execution(db, &root, project, &crate::actor(), execution)
                            .and_then(|b| control::rework_allowed(db, runtime, closed, false).map(|_| Some(b.owner.epoch))),
                    (None, None) if closed => Ok(None),
                    _ => Err(CoreError::Refused("native_execution_binding_invalid")),
                }
            };
            let warns = decision.is_ok() && started_dependents(product, &task, &started);
            let record = &mut product["tasks"][&task];
            record["reworkAvailable"] = json!(decision.is_ok());
            record["reworkWarnings"] = if warns { json!(["dependents_started"]) } else { json!([]) };
            match decision {
                Ok(epoch) => {
                    record["reworkUnavailableReason"] = Value::Null;
                    record["reworkVersion"] = json!(version.to_string());
                    record["reworkOwnerEpoch"] = json!(epoch.map(|n| n.to_string()));
                    record["reworkExecutionId"] = json!(execution);
                }
                Err(CoreError::Refused(code)) => {
                    record["reworkUnavailableReason"] = json!(code);
                    for key in ["reworkVersion", "reworkOwnerEpoch", "reworkExecutionId"] {
                        record.as_object_mut().unwrap().remove(key);
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }).map_err(context::core)
}

pub(super) fn open_rework(task: &Value) -> bool {
    task["reworkEvent"].is_string() && !closed(task) && task["purged"] != true
}

pub(super) fn normalize_on(
    db: &Connection,
    project: &str,
    product: &mut Value,
    task: &str,
    runtime: &str,
) -> CoreResult<()> {
    let event = control::owner_rework_event(db, runtime)?;
    let record = &mut product["tasks"][task];
    if normalize(record, event) {
        let (base, target, reset_changes_base): (Option<String>, Option<String>, bool) = db.query_row(
            "SELECT json_extract(payload_json,'$.baseRevision'),json_extract(payload_json,'$.targetBranch'),
                    COALESCE(json_extract(payload_json,'$.resetChangesBase'),0)
             FROM runtime_events WHERE id=? AND task_id=?",
            (event, runtime), |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        if reset_changes_base {
            record.as_object_mut().unwrap().remove("changesBaseRevision");
        }
        if let Some(base) = base {
            record["baseRevision"] = json!(base);
            if let Some(target) = target {
                record["targetBranch"] = json!(target);
            }
        }
        if let Some(tree) = cortex_core::workspace::worktrees::owned_on(db, project, task)? {
            context::set_worktree(record, &tree);
        }
        reopen_sprint(product, task);
    }
    Ok(())
}

/// Old overlay-only dispositions did not record a runtime close event. Do not
/// let a plain owner message reopen one of those, including archived tasks.
pub(crate) fn with_plain_prompt_guard<T>(
    store: &Store,
    state: &Path,
    project: &str,
    execution: &str,
    action: impl FnOnce() -> CoreResult<T>,
) -> CoreResult<T> {
    let _lock = Lock::new(state, "machine.lock").map_err(|e| CoreError::Refused(e.code))?;
    let mut product = product(state, project).map_err(|e| CoreError::Refused(e.code))?;
    store.read(|db| {
        let root = store.key().root().to_string_lossy();
        let b =
            control::rework_binding_for_execution(db, &root, project, &crate::actor(), execution)?;
        normalize_on(db, project, &mut product, &b.draft, &b.task)?;
        let record = &product["tasks"][&b.draft];
        if closed(record)
            || archived(record)
            || control::unconfirmed_close_current(db, project, &root, &b.draft, execution)?
            || control::owner_resolution_current(db, &b.task)?
        {
            return Err(CoreError::Refused("task_not_resumable"));
        }
        Ok(())
    })?;
    action()
}

pub(crate) fn with_plain_start_guard<T>(
    state: &Path,
    project: &str,
    request: &Value,
    queued: Option<(&Store, &super::sprint_live::Expectation)>,
    start: impl FnOnce() -> CoreResult<T>,
) -> CoreResult<T> {
    let _lock = Lock::new(state, "machine.lock").map_err(|e| CoreError::Refused(e.code))?;
    let task = request["target"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_task"))?;
    let saved = product(state, project).map_err(|e| CoreError::Refused(e.code))?;
    if let Some((store, expected)) = queued {
        super::sprint_live::check_start(store, &saved, project, expected, task)?;
    }
    if closed(&saved["tasks"][task]) || archived(&saved["tasks"][task]) {
        return Err(CoreError::Refused("task_not_resumable"));
    }
    start()
}

pub(crate) fn preflight(
    store: &Store,
    state: &Path,
    project: &str,
    request: &Value,
) -> CoreResult<(Option<Binding>, Option<Value>)> {
    let _lock = Lock::new(state, "machine.lock").map_err(|e| CoreError::Refused(e.code))?;
    let mut product = product(state, project).map_err(|e| CoreError::Refused(e.code))?;
    let root = store.key().root().to_string_lossy();
    let task = request["taskId"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_task"))?;
    if archived(&product["tasks"][task]) {
        return Err(CoreError::Refused("task_archived"));
    }
    if request["executionId"].is_null() {
        let prior = cortex_core::workspace::initial_rework_replay(
            store,
            project,
            &crate::actor(),
            request,
        )?;
        if prior.is_none() {
            let record = &product["tasks"][task];
            if !closed(record) || record["purged"] == true {
                return Err(CoreError::Refused("task_not_closed"));
            }
            if active_merge(record) {
                return Err(CoreError::Refused("merge_in_progress"));
            }
            store.read(|db| {
                let (version, execution): (i64, Option<String>) = db.query_row(
                    "SELECT t.version,t.execution_id FROM workspace_tasks t JOIN workspace_projects p USING(project_id)
                     WHERE t.project_id=? AND t.task_id=? AND p.project_root=? AND p.activated_by=?",
                    (project, task, &*root, crate::actor()), |r| Ok((r.get(0)?, r.get(1)?)),
                ).optional()?.ok_or(CoreError::Refused("invalid_task"))?;
                if version.to_string() != request["expectedVersion"] || execution.is_some() {
                    return Err(CoreError::Refused("stale_version"));
                }
                Ok(())
            })?;
        }
        return Ok((None, prior));
    }
    store.read(|db| {
        let b = control::rework_binding(db, &root, project, &crate::actor(), request)?;
        let replay = control::replay_rework(db, &b, request)?;
        if replay.is_none() {
            normalize_on(db, project, &mut product, &b.draft, &b.task)?;
            let record = &product["tasks"][&b.draft];
            if b.version.to_string() != request["expectedVersion"] {
                return Err(CoreError::Refused("stale_version"));
            }
            if record["purged"] == true {
                return Err(CoreError::Refused("invalid_task"));
            }
            if active_merge(record) {
                return Err(CoreError::Refused("merge_in_progress"));
            }
            let closed = closed(record)
                || control::unconfirmed_close_current(
                    db,
                    project,
                    &root,
                    &b.draft,
                    request["executionId"].as_str().unwrap(),
                )?;
            control::rework_allowed(db, &b.task, closed, archived(record))?;
        }
        Ok((Some(b), replay))
    })
}

pub(crate) fn admit(
    store: &Store,
    state: &Path,
    project: &str,
    request: &Value,
    capture: Option<&cortex_core::input::Capture>,
) -> Result<(Value, Option<Binding>)> {
    let _repo = Lock::long(
        state,
        &git::repo_lock(store.key().root()),
        "git_in_progress",
        "Cortex is preparing the checkout for rework.",
    )?;
    let lock = Lock::new(state, "machine.lock")?;
    let mut product = product(state, project)?;
    let root = store.key().root().to_string_lossy();
    let task = string(request, "taskId")?;
    let actor = crate::actor();
    settings::idle(state, project, task, &lock)?;
    let binding = if request["executionId"].is_null() {
        None
    } else {
        Some(
            store
                .read(|db| {
                    let b = control::rework_binding(db, &root, project, &actor, request)?;
                    normalize_on(db, project, &mut product, &b.draft, &b.task)?;
                    Ok(b)
                })
                .map_err(|e| error(e.code()))?,
        )
    };
    let record = &product["tasks"][task];
    if archived(record) {
        return Err(error("task_archived"));
    }
    if record["purged"] == true {
        return Err(error("invalid_task"));
    }
    if active_merge(record) {
        return Err(error("merge_in_progress"));
    }
    let history = json!({"status":record["status"],"closedAs":record["closedAs"],
        "doneAt":record["doneAt"],"notVerifiedByCortex":record["notVerifiedByCortex"],
        "acceptance":record["acceptance"],"integration":record["integration"],
        "alreadyInTargetBranch":record["alreadyInTargetBranch"],
        "worktreeCleanup":record["worktreeCleanup"],"baseRevision":record["baseRevision"]});
    let (mut receipt, b) = if let Some(b) = binding {
        let contained_target = record["alreadyInTargetBranch"].as_str();
        let integrated = contained_target.is_some()
            || record["work"] == "merged"
            || (record["integration"].is_object() && record["integration"]["method"] != "pr");
        let target = contained_target
            .or_else(|| record["integration"]["targetBranch"].as_str())
            .or_else(|| record["targetBranch"].as_str())
            .or_else(|| {
                (record["checkout"]["kind"] == "main")
                    .then(|| record["checkout"]["branch"].as_str())
                    .flatten()
            })
            .filter(|name| !name.is_empty());
        let directory = if cortex_core::workspace::worktrees::owned(store, project, &b.draft)
            .map_err(|e| error(e.code()))?
            .is_some()
        {
            super::worktree_directory(state).map_err(|e| error(e.code()))?
        } else {
            std::path::PathBuf::new()
        };
        let receipt = control::rework(
            store,
            project,
            &actor,
            request,
            capture,
            ReworkDisposition {
                archived: false,
                closed: closed(record),
                integrated,
                target,
                history,
                worktree_root: &directory,
            },
        )
        .map_err(|e| error(e.code()))?;
        (receipt, b)
    } else {
        if !closed(record) {
            return Err(error("task_not_closed"));
        }
        let needs_worktree =
            cortex_core::workspace::draft_worktree_selected(store, project, &actor, task)
                .map_err(|e| error(e.code()))?;
        let directory = needs_worktree
            .then(|| super::worktree_directory(state))
            .transpose()
            .map_err(|e| error(e.code()))?;
        let started = cortex_core::workspace::start_native_rework(
            store,
            project,
            &actor,
            request,
            capture,
            history,
            directory.as_deref(),
        )
        .map_err(|e| error(e.code()))?;
        let receipt = started["receipt"].clone();
        if started["fresh"] != true {
            return Ok((receipt, None));
        }
        let execution = string(&receipt, "target")?;
        let b = store
            .read(|db| control::binding(db, &root, project, &actor, execution))
            .map_err(|e| error(e.code()))?;
        (receipt, b)
    };
    // Core is authoritative. If the file cannot be replaced, reads still mask
    // the old disposition using the committed event. Do not claim the send failed.
    let saved = store
        .read(|db| normalize_on(db, project, &mut product, &b.draft, &b.task))
        .map_err(context::core)
        .and_then(|()| product_path(state, project))
        .and_then(|path| settings::write_json(&path, &product));
    if saved.is_err() {
        receipt["overlayPending"] = json!(true);
    }
    Ok((receipt, Some(b)))
}
