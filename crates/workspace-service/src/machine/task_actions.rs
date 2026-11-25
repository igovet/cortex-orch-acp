//! Native draft and owner actions. Git integration has its own admission/effect boundary.
use super::{
    context,
    git::Git,
    native::*,
    product as private_product,
    settings::{self, Lock},
};
use cortex_core::{
    workspace::edits::{self, Change},
    OpenMode, Store,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::path::Path;

/// Every action is native. A rename title that is not a string, boolean,
/// integer or null (float, array, object) is an explicit `invalid_request`
/// refusal inside `run`; no request is delegated.
pub fn supported(_p: &Value) -> bool {
    true
}
fn edit_error(code: &'static str) -> Refused {
    refuse(
        code,
        "The task started or changed. Refresh before editing; your draft was not saved.",
    )
}
fn contract() -> Refused {
    refuse("machine_tool_failed", "ContractError")
}
fn store(root: &Path) -> Result<Store> {
    match context::open(root, OpenMode::RuntimeValidate)? {
        Some(store) => Ok(store),
        None => Store::open(context::key(root)?, OpenMode::Initialize).map_err(context::core),
    }
}
pub(super) fn require_consistent_records(root: &Path, project: &str, task: &str) -> Result<()> {
    if let Some(store) = context::open(root, OpenMode::LiveReadOnly)? {
        let records = store
            .read(|db| {
                cortex_core::projections::task_records(db, project, &root.to_string_lossy(), task)
            })
            .map_err(context::core)?;
        if records.is_some_and(|t| {
            t.warnings.iter().any(|w| {
                matches!(
                    w.as_str(),
                    "contradictory_evidence" | "owner_closed_unconfirmed"
                )
            })
        }) {
            return Err(refuse(
                "contradictory_evidence",
                "The task's records disagree. Only the explicit close without verification is available.",
            ));
        }
    }
    Ok(())
}
/// Take the private disposition without depending on Git, worktree availability,
/// or a consistent runtime projection. Only an existing task can be archived.
pub(super) async fn run_archive(
    root: &Path,
    project: &str,
    state: &Path,
    p: &Value,
    runtime: Option<&std::sync::Arc<crate::native_runtime::Runtime>>,
) -> Result<Value> {
    object_params(p)?;
    let task = task_id(p)?;
    // Start, Resume and automatic continuation normally serialize here. If a
    // long control owns the lane, still shelve the task without claiming it stopped.
    let lane = match runtime {
        Some(runtime) => tokio::time::timeout(
            std::time::Duration::from_secs(2), runtime.controls.lock(),
        ).await.ok(),
        None => None,
    };
    let _lock = Lock::new(state, "machine.lock")?;
    let mut product = super::native::product(state, project)?;
    let saved = &product["tasks"][task];
    if truth(&saved["purged"]) {
        return Err(refuse("invalid_task", "Unknown task."));
    }
    if truth(&saved["archive"]) {
        return Err(refuse("already_archived", "Already in the Archive."));
    }

    let mut observed = Value::Null;
    let mut owner_closed = json!({"tasks":{}});
    let mut quiet = false;
    match context::open(root, OpenMode::LiveReadOnly) {
        Ok(Some(store)) => {
            if let Ok((fact, stopped)) = store.read(|db| {
                let fact = cortex_core::projections::task_records(db, project, &root.to_string_lossy(), task)?;
                let stopped = match runtime_of(db, project, task)? {
                    Some(runtime) => cortex_core::lifecycle::automatic::require_processes_gone(db, &runtime).is_ok(),
                    None => !db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM workspace_starts WHERE task_id=?)",
                        [task], |row| row.get::<_, bool>(0),
                    )?,
                };
                Ok((serde_json::to_value(fact)?, stopped))
            }) {
                observed = fact;
                quiet = stopped;
            }
            if saved["status"].is_null() {
                let _ = context::owner_closed(&store, project, &mut owner_closed);
            }
        }
        // A genuinely absent store has no durable start or execution to stop.
        // Do not infer that from a failed read or while control may be in flight.
        Ok(None) => quiet = runtime.is_none() || lane.is_some(),
        Err(_) => {}
    }
    if !saved.is_object() && observed.is_null() {
        return Err(refuse("invalid_task", "Unknown task."));
    }

    let warned = |tag: &str| observed["warnings"].as_array()
        .is_some_and(|items| items.iter().any(|item| item == tag));
    let owner_closed = &owner_closed["tasks"][task];
    let status = if warned("owner_closed_unconfirmed") {
        "closed_unverified"
    } else if warned("process_exit_pending") && !warned("cancelled_worker_stopping") {
        "stopping"
    } else if warned("contradictory_evidence") || warned("process_exit_unconfirmed") {
        "unknown"
    } else if let Some(saved) = saved["status"].as_str().or(owner_closed["status"].as_str()) {
        saved
    } else {
        match observed["phase"].as_str() {
            Some("not_started") => "draft",
            Some("planning" | "execution" | "review" | "verification" | "finalizing") => "working",
            Some("waiting" | "blocked") => "needs_you",
            Some("recovery") if observed["waitReason"].is_string() => "needs_you",
            Some("paused") => "paused",
            Some("failed") => "failed",
            Some("stopped") => "cancelled",
            Some("result") => "review",
            _ => "unknown",
        }
    };
    let idle_phase = matches!(observed["phase"].as_str(),
        Some("not_started" | "paused" | "failed" | "stopped" | "result"));
    let stop_unconfirmed = if quiet && (status == "done" || idle_phase) {
        false
    } else if let (Some(runtime), Some(_), Some(execution)) =
        (runtime, lane.as_ref(), observed["executionId"].as_str())
    {
        !runtime.pause_for_archive(project, execution).await
    } else {
        true
    };
    let status = status.to_owned();
    let record = &mut product["tasks"][task];
    if !record.is_object() {
        *record = json!({});
    }
    if status == "done" && record["status"].is_null() {
        for (key, value) in owner_closed.as_object().into_iter().flatten() {
            record[key] = value.clone();
        }
    }
    super::task_lifecycle::archive(record, now_ms(), &status);
    if status != "done" {
        // Distinguish a manual archive from a historical sprint-closure archive.
        record["archive"]["unfinished"] = json!(true);
    }
    if stop_unconfirmed {
        record["archive"]["stopUnconfirmed"] = json!(true);
    }
    let reply = json!({"done":true,"reclamation":record["reclamation"]});
    settings::write_json(&product_path(state, project)?, &product)?;
    Ok(reply)
}

fn task_id(p: &Value) -> Result<&str> {
    p["taskId"]
        .as_str()
        .filter(|s| {
            !s.is_empty()
                && s.len() <= 128
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_:.-".contains(&b))
        })
        .ok_or_else(|| refuse("invalid_task", "Unknown task."))
}

pub async fn run(
    root: &Path,
    project: &str,
    state: &Path,
    p: &Value,
    git: &mut Git,
) -> Result<Value> {
    object_params(p)?;
    let task = task_id(p)?;
    // `params or {}`: falsy params read as an empty request (invalid_task above).
    // Merge finish/abort and file choices change the index: one Git-changing action
    // per repository at a time, taken before the machine lock (the same order as
    // every other holder of both).
    let _repo = if matches!(
        p["action"].as_str(),
        Some("finish_merge" | "abort_merge" | "resolve_block")
    ) {
        Some(Lock::long(
            state,
            &super::git::repo_lock(root),
            "integration_in_progress",
            super::integration::REPO_BUSY,
        )?)
    } else if matches!(p["action"].as_str(), Some("close_remove_worktree" | "purge" | "clear_old")) {
        Some(Lock::new(state, &super::git::repo_lock(root))?)
    } else {
        None
    };
    // An automatic settle never waits for the machine lock: the background pass
    // simply tries again next time (`deferred`), so no owner ever sees it as busy.
    let _lock = if p["action"] == "auto_ready" {
        match Lock::try_new(state, "machine.lock")? {
            Some(lock) => lock,
            None => return Ok(json!({"accepted":false,"deferred":true})),
        }
    } else {
        Lock::new(state, "machine.lock")?
    };
    if matches!(
        p["action"].as_str(),
        Some("resolve_block" | "finish_merge" | "abort_merge")
    ) {
        if let Some(reply) = super::ledger::Intent::existing_locked(
            state,
            "product.task",
            project,
            Some(root),
            p,
            &_lock,
        )? {
            return super::task_resolution::replay(reply);
        }
    }
    // A merge or pull request of this task runs its Git effects outside the lock; any
    // other action on the task is refused at once, naming it, rather than waiting.
    if let Err(e) = settings::idle(state, project, task, &_lock) {
        if p["action"] == "auto_ready" {
            return Ok(json!({"accepted":false,"deferred":true}));
        }
        return Err(e);
    }
    // Check before any effect. Unmaterialized legacy names still need the retained projection.
    super::task_facts::materialized(root, project).or_else(|error| {
        if error.code == "project_storage_mismatch"
            && matches!(p["action"].as_str(), Some("accept" | "confirm_discard"))
        {
            // The retained boundary translates this storage refusal; do not reopen it.
            return Err(refuse("machine_tool_failed", "StoreError"));
        }
        if error.code == "project_storage_mismatch" && super::task_lifecycle::external(p) {
            // Core has refused the contradictory store; never inspect it by another route.
            // Preserve the retained close refusal ordering using private presentation only.
            let saved = super::native::product(state, project)?;
            super::task_lifecycle::close_precondition(&saved["tasks"][task], p)?;
            return Err(super::task_lifecycle::unsettled());
        }
        Err(error)
    })?;
    if p["action"] == "close_unconfirmed" {
        if p.as_object().is_none_or(|object| {
            object.keys().any(|key| {
                !matches!(
                    key.as_str(),
                    "action" | "taskId" | "executionId" | "expectedVersion" | "confirmed"
                )
            })
        }) {
            return Err(refuse(
                "invalid_request",
                "Refresh the task and confirm closing again.",
            ));
        }
        if p["confirmed"] != true {
            return Err(refuse(
                "confirmation_required",
                "Confirm closing the task as is.",
            ));
        }
        let version = serde_json::from_value(p["expectedVersion"].clone())
            .map_err(|_| refuse("stale_version", "Refresh the task and confirm again."))?;
        let execution = string(p, "executionId")?;
        let store = context::open(root, OpenMode::RuntimeValidate)?
            .ok_or_else(|| refuse("invalid_task", "Unknown task."))?;
        cortex_core::workspace::control::close_unconfirmed(
            &store,
            project,
            &crate::actor(),
            task,
            execution,
            version,
        )
        .map_err(|error| match error.code() {
            "stale_version" | "records_not_conflicting" => {
                refuse(error.code(), "The task changed. Refresh it before closing.")
            }
            _ => context::core(error),
        })?;
        return Ok(json!({"done":true}));
    }
    // Saved presentation is not authority to change a disputed task. Check
    // before reading the mutable overlay or dispatching any ordinary action.
    if p["action"] != "restore"
        || !truth(&super::native::product(state, project)?["tasks"][task]["archive"])
    {
        require_consistent_records(root, project, task)?;
    }
    let mut product = context::product(root, state, project, git).await?;
    if product["tasks"].get(task).is_none() {
        product["tasks"][task] = json!({});
    }
    let mut record = product["tasks"][task].clone();
    let action = p["action"].as_str().unwrap_or("");
    let close_as_is = action == "close" && p["choice"] == "as_is";
    let review_hold = product["sprints"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|sprint| {
            sprint["id"] == record["sprintId"]
                && private_product::automatic_sprint(sprint)
                && sprint["queueState"] == "paused"
                && sprint["automaticQueue"]["kind"] == "settlement_blocked"
                && sprint["automaticQueue"]["taskId"] == task
                // A hold waiting on an unknown effect keeps its code's guards.
                && (sprint["automaticQueue"]["needsOwner"] == true
                    || sprint["automaticQueue"]["settling"].is_object())
        })
        .and_then(|sprint| sprint["automaticQueue"]["code"].as_str());
    let result_is_stale = matches!(
        review_hold,
        Some("stale_result" | "workspace_drift" | "task_files_changed" | "task_content_unknown")
    ) || record["workspaceDigest"]
        .as_str()
        .zip(record["result"]["digest"].as_str())
        .is_some_and(|(current, result)| current != result)
        || record["result"]["warnings"]
            .as_array()
            .is_some_and(|items| items.iter().any(|warning| warning == "workspace_drift"));
    let stale_automatic_acceptance =
        close_as_is && record["acceptance"]["automatic"] == true && result_is_stale;
    let automatic_acceptance_close = stale_automatic_acceptance
        || close_as_is && review_hold == Some("automatic_acceptance_blocked");
    // The scheduler handed this task to the owner because the runtime cannot settle
    // an effect whose outcome is unknown (see `settling_hold` in the native driver).
    let owner_unsettled = product["sprints"]
        .as_array()
        .into_iter()
        .flatten()
        .any(|sprint| {
            sprint["id"] == record["sprintId"]
                && sprint["automaticQueue"]["taskId"] == task
                && sprint["automaticQueue"]["settling"]["owner"] == true
        });
    if action == "close" && p["preflight"] == true {
        // The owner's close is checked before the runtime records its resolution, so
        // a refusal never leaves the run closed and the task open. Nothing is written.
        super::task_lifecycle::close_precondition(&record, p)?;
        if automatic_acceptance_close {
            if let Some(store) = context::open(root, OpenMode::LiveReadOnly)? {
                store
                    .read(|db| match runtime_of(db, project, task)? {
                        Some(runtime) => close_settled(db, &runtime, owner_unsettled, false),
                        None => Ok(()),
                    })
                    .map_err(super::task_lifecycle::settle_error)?;
            }
        }
        return Ok(json!({"done":false,"preflight":true}));
    }
    let mut version = None;
    match action {
        "edit" => {
            let store = store(root)?;
            let owner = store
                .read(|db| {
                    Ok(db
                        .query_row(
                            "SELECT activated_by FROM workspace_projects WHERE project_id=?",
                            [project],
                            |r| r.get::<_, String>(0),
                        )
                        .optional()?)
                })
                .map_err(context::core)?;
            if owner.is_none()
                || record["status"] == "done"
                || truth(&record["archive"])
                || truth(&record["purged"])
            {
                return Err(refuse(
                    "not_editable",
                    "Only a saved task that has not started can be edited.",
                ));
            }
            workspace_protocol::validate("Identity", &json!(task)).map_err(|_| contract())?;
            workspace_protocol::validate("Decimal", &p["expectedVersion"])
                .map_err(|_| contract())?;
            let mut resolved = p["payload"].clone();
            let title = p["payload"]["title"]
                .as_str()
                .filter(|s| !s.trim().is_empty() && s.trim().chars().count() <= 200)
                .ok_or_else(|| edit_error("title_required"))?;
            // The edit names its files by reference; Core keeps its own copy of each body.
            if let Some(items) = resolved
                .get("attachments")
                .filter(|a| a.is_array())
                .cloned()
            {
                resolved["attachments"] =
                    super::attachment_files::resolve(state, &items).map_err(|e| {
                        edit_error(if e.code == "invalid_attachments" {
                            "invalid_attachment"
                        } else {
                            e.code
                        })
                    })?;
            }
            let payload = &resolved;
            let mut choices = payload.clone();
            choices
                .as_object_mut()
                .ok_or_else(contract)?
                .remove("title");
            workspace_protocol::validate("CreatePayload", &choices).map_err(|_| contract())?;
            if choices["description"]
                .as_str()
                .unwrap_or("")
                .trim()
                .is_empty()
            {
                return Err(edit_error("description_required"));
            }
            private_product::checked_attachments(
                choices.get("attachments").unwrap_or(&json!([])),
                true,
            )
            .map_err(|e| edit_error(e.code))?;
            version = edits::change(
                &store,
                project,
                task,
                Change::Edit {
                    expected: p["expectedVersion"].as_str().unwrap(),
                    payload,
                },
            )
            .map_err(|e| edit_error(e.code()))?;
            record["title"] = json!(title.trim());
            record["titleState"] = json!("user");
        }
        "settings" => {
            workspace_protocol::validate(
                "CreatePayload",
                &json!({"description":"settings","settings":p["settings"]}),
            )
            .map_err(|_| contract())?;
            edits::change(
                &store(root)?,
                project,
                task,
                Change::Settings(&p["settings"]),
            )
            .map_err(|e| {
                if e.code() == "busy" {
                    refuse(
                        "busy",
                        "Change a running task through its session, not a draft.",
                    )
                } else {
                    context::core(e)
                }
            })?;
        }
        "rename" => {
            let empty = json!("");
            let raw = p.get("title").unwrap_or(&empty);
            if !(raw.is_string()
                || raw.is_boolean()
                || raw.is_null()
                || raw.is_i64()
                || raw.is_u64())
            {
                return Err(refuse("invalid_request", "A task title must be text."));
            }
            let title = python_text(raw);
            let title = title.trim();
            if title.is_empty() {
                return Err(refuse("empty_title", "Write a title first."));
            }
            let title: String = title.chars().take(200).collect();
            // The title is presentation, not lifecycle: any task that is still kept can
            // be renamed, started or not. Only a cleared or deleted task cannot.
            if truth(&record["archive"]) || truth(&record["purged"]) {
                return Err(refuse(
                    "not_editable",
                    "A cleared or deleted task cannot be renamed.",
                ));
            }
            version = edits::change(&store(root)?, project, task, Change::Rename(&title))
                .map_err(context::core)?;
            record["title"] = json!(title);
            record["titleState"] = json!("user");
        }
        "target" => {
            record["targetBranch"] = p["branch"].clone();
            record["targetInherited"] = json!(false);
        }
        "reopen" => {
            // This private disposition cleanup is not authority to reopen Done.
            if matches!(record["status"].as_str(), Some("done" | "closed_unverified"))
                || truth(&record["archive"])
            {
                if let Some(store) = context::open(root, OpenMode::LiveReadOnly)? {
                    let exists = store.read(|db| Ok(db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM workspace_tasks WHERE project_id=? AND task_id=?)",
                        (project, task), |r| r.get::<_, bool>(0),
                    )?)).map_err(context::core)?;
                    if exists {
                        return Err(refuse("task_not_resumable", "Use Return for rework to reopen a closed task."));
                    }
                }
            }
            for key in ["status", "acceptance", "automaticReady"] {
                record.as_object_mut().unwrap().remove(key);
            }
        }
        "restore" => {
            if truth(&record["purged"]) {
                return Err(refuse("invalid_task", "Unknown task."));
            }
            if record["archive"]["unfinished"] == true {
                // Drafts remain manually startable, but are not queue candidates.
                // If Pause was unconfirmed, keep fencing all automatic relaunch.
                record["archiveHold"] = json!(if record["archive"]["stopUnconfirmed"] == true {
                    "unconfirmed"
                } else if record["archive"]["previousStatus"] == "draft" {
                    "draft"
                } else {
                    "stopped"
                });
            }
            if record["archive"].is_object() {
                // Return to the project, not to an irrevocably closed sprint.
                // Remove both membership links so a subsequent sprint delete
                // cannot delete a task the owner already restored.
                for sprint in product["sprints"].as_array_mut().into_iter().flatten() {
                    if sprint["archivedAt"].is_null() || sprint["deleting"] == true {
                        continue;
                    }
                    if sprint["id"] == record["sprintId"] {
                        record["sprintId"] = Value::Null;
                    }
                    if let Some(ids) = sprint["taskIds"].as_array_mut() {
                        ids.retain(|id| id != task);
                    }
                }
            }
            record["archive"] = Value::Null;
            record["purgeBlocker"] = Value::Null;
        }
        "sprint" => {
            let index = product["sprints"]
                .as_array()
                .and_then(|s| s.iter().position(|s| s["id"] == p["sprintId"]))
                .ok_or_else(|| refuse("unknown_sprint", "Unknown sprint."))?;
            let mut sprint = product["sprints"][index].clone();
            if truth(&sprint["archivedAt"]) {
                return Err(refuse(
                    "sprint_closed",
                    "This sprint is closed. Create the task outside it.",
                ));
            }
            let empty = json!([]);
            let deps = p.get("dependencies").filter(|v| truth(v)).unwrap_or(&empty);
            let members = sprint["taskIds"]
                .as_array()
                .ok_or_else(|| refuse("state_unreadable", "Invalid sprint state."))?
                .clone();
            if deps
                .as_array()
                .is_some_and(|a| a.iter().any(|d| d.is_array() || d.is_object()))
            {
                return Err(refuse("machine_tool_failed", "TypeError"));
            }
            let deps = deps
                .as_array()
                .filter(|deps| {
                    deps.iter()
                        .enumerate()
                        .all(|(i, d)| !deps[..i].contains(d) && members.contains(d) && d != task)
                })
                .ok_or_else(|| {
                    refuse(
                        "invalid_dependencies",
                        "Dependencies must be earlier tasks in this sprint.",
                    )
                })?;
            if truth(&record["sprintId"]) && record["sprintId"] != sprint["id"] {
                return Err(refuse(
                    "already_linked",
                    "This task already belongs to a sprint.",
                ));
            }
            if members.contains(&json!(task))
                && record.get("dependencies").unwrap_or(&empty) != &json!(deps)
            {
                return Err(refuse(
                    "already_linked",
                    "Existing task dependencies stay unchanged.",
                ));
            }
            // The plan item's own title names the task it became; an owner's title stays.
            let title = match p.get("title") {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if !s.trim().is_empty() => {
                    Some(s.trim().chars().take(200).collect::<String>())
                }
                Some(_) => return Err(refuse("invalid_request", "A task title must be text.")),
            };
            {
                record["sprintId"] = sprint["id"].clone();
                record["dependencies"] = json!(deps);
                record["planning"] = json!(truth(&p["planning"]));
                if let Some(title) = title.filter(|_| record["titleState"] != "user") {
                    record["title"] = json!(title);
                    record["titleState"] = json!("generated");
                }
                if !members.contains(&json!(task)) && !truth(&p["planning"]) {
                    sprint["taskIds"].as_array_mut().unwrap().push(json!(task));
                    if sprint["queueState"] == "complete" {
                        sprint["queueState"] = json!("paused");
                        sprint["pauseReason"] =
                            json!("New task added. Continue the queue when ready.");
                    }
                }
                if record.get("targetBranch").is_none() || truth(&record["targetInherited"]) {
                    record["targetBranch"] = sprint["targetBranch"].clone();
                    record["targetInherited"] = json!(true);
                }
            }
            product["sprints"][index] = sprint;
            product["tasks"][task] = record;
            if let Some(store) = context::open(root, OpenMode::RuntimeValidate)? {
                store
                    .write(|db| {
                        let row:Option<Option<String>>=db.query_row(
                        "SELECT execution_id FROM workspace_tasks WHERE project_id=? AND task_id=?",
                        (project,task),|r|r.get(0)).optional()?;
                        if row != Some(None) {
                            return Err(cortex_core::CoreError::Refused("not_editable"));
                        }
                        settings::write_json(
                            &product_path(state, project)
                                .map_err(|e| cortex_core::CoreError::Refused(e.code))?,
                            &product,
                        )
                        .map_err(|e| cortex_core::CoreError::Refused(e.code))?;
                        Ok(())
                    })
                    .map_err(|e| {
                        if e.code() == "not_editable" {
                            refuse(
                                "not_editable",
                                "Only a task that has not started can join a sprint.",
                            )
                        } else {
                            context::core(e)
                        }
                    })?;
            } else {
                settings::write_json(&product_path(state, project)?, &product)?;
            }
            return Ok(json!({"done":true}));
        }
        "resolve_block" => {
            return super::task_resolution::resolve_block(
                root,
                project,
                task,
                state,
                &mut product,
                p,
                git,
                &_lock,
            )
            .await;
        }
        "finish_merge" | "abort_merge" => {
            return super::task_merge::run(root, project, task, state, &mut product, p, git, _lock)
                .await;
        }
        "confirm_discard" => {
            super::task_review::confirm_discard(root, &mut record, git).await?;
        }
        "auto_ready" => {
            return super::task_review::automatic(root, project, task, state, &mut product, git)
                .await;
        }
        "accept" => {
            return super::task_review::accept(root, project, task, state, &mut product, p, git)
                .await;
        }
        "close_remove_worktree" => {
            return super::task_lifecycle::close_with_cleanup(root, project, state, task, &mut product, p, git).await;
        }
        "clear_old" | "purge" => {
            let at = now_ms();
            if action == "clear_old" {
                let cutoff = p["cutoff"].as_f64();
                let done = record["doneAt"].as_f64();
                if p["confirmed"] != true
                    || !cutoff.is_some_and(|n| n >= 0.0 && n <= at as f64)
                    || truth(&record["archive"])
                    || record["status"] != "done"
                    || !done
                        .zip(cutoff)
                        .is_some_and(|(done, cutoff)| done <= cutoff)
                    || super::retention::unmerged(root, &record, git).await?
                {
                    return Err(refuse("clear_preview_changed", "The task is no longer eligible. Refresh the preview; no deletion was attempted."));
                }
                record["archive"] = json!({"at":at,"previousStatus":"done"});
                product["tasks"][task] = record;
            } else if p["confirmed"] != true {
                return Err(refuse("confirmation_required", "Confirm before deleting."));
            }
            // The shared Core purge holds the exclusive access fence over durable
            // deletion intent and teardown. Never fall back after this boundary.
            super::retention::purge_owner(root, project, state, &mut product, task, git).await?;
            settings::write_json(&product_path(state, project)?, &product)?;
            let mut result = json!({"done":true,"reclamation":product["tasks"][task]["reclamation"]});
            if let Some(cleanup) = product["tasks"][task].get("worktreeCleanup") {
                result["worktreeCleanup"] = cleanup.clone();
            }
            return Ok(result);
        }
        action if super::task_lifecycle::supported(action) => {
            super::task_lifecycle::apply(root, project, task, &mut record, p, git).await?;
            if automatic_acceptance_close
                && action == "close"
                && record["acceptance"]["automatic"] == true
            {
                // The saved product record may be intentionally compact and omit
                // the hydrated result/workspace digests. Preserve the stale
                // automatic-acceptance fact explicitly across restart.
                record["notVerifiedByCortex"] = json!(true);
            }
        }
        _ => {
            return Err(refuse(
                "unsupported_action",
                "This machine does not support that yet.",
            ))
        }
    }
    product["tasks"][task] = record;
    if super::task_lifecycle::external(p) || automatic_acceptance_close {
        if let Some(store) = context::open(root, OpenMode::RuntimeValidate)? {
            // Recheck under Start arbitration and retain it through the private save.
            store
                .write(|db| {
                    super::task_lifecycle::settled(db, root, project, task)?;
                    if automatic_acceptance_close {
                        let runtime: String = db.query_row(
                            "SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e
                             ON e.execution_id=w.execution_id AND e.task_id=w.task_id WHERE w.project_id=? AND w.task_id=?",
                            (project, task), |row| row.get(0),
                        )?;
                        close_settled(db, &runtime, owner_unsettled, true)?;
                    }
                    settings::write_json(
                        &product_path(state, project)
                            .map_err(|e| cortex_core::CoreError::Refused(e.code))?,
                        &product,
                    )
                    .map_err(|e| cortex_core::CoreError::Refused(e.code))?;
                    Ok(())
                })
                .map_err(super::task_lifecycle::settle_error)?;
        } else if automatic_acceptance_close {
            return Err(super::task_lifecycle::unsettled());
        } else {
            settings::write_json(&product_path(state, project)?, &product)?;
        }
    } else {
        settings::write_json(&product_path(state, project)?, &product)?;
    }
    if action == "edit" {
        return Ok(json!({"done":true,"version":version}));
    }
    if action == "sprint" || super::task_lifecycle::external(p) {
        return Ok(json!({"done":true}));
    }
    let mut reply = json!({"done":true,"reclamation":product["tasks"][task]["reclamation"]});
    if let Some(version) = version {
        reply["version"] = json!(version);
    }
    Ok(reply)
}
fn runtime_of(
    db: &rusqlite::Connection,
    project: &str,
    task: &str,
) -> cortex_core::CoreResult<Option<String>> {
    Ok(db
        .query_row(
            "SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e
             ON e.execution_id=w.execution_id AND e.task_id=w.task_id WHERE w.project_id=? AND w.task_id=?",
            (project, task),
            |row| row.get(0),
        )
        .optional()?)
}
/// The settled-execution check of a Close as is that hands the folder to the next
/// task. When the runtime cannot settle an unknown effect and the owner was asked
/// (`acknowledge`), the owner's confirmed close is the same explicit acknowledgement
/// as the runtime's Retry: each receipt is recorded as settled with an unknown
/// outcome, never as success. A worker still being stopped is always refused.
fn close_settled(
    db: &rusqlite::Connection,
    runtime: &str,
    acknowledge: bool,
    write: bool,
) -> cortex_core::CoreResult<()> {
    match cortex_core::lifecycle::automatic::require_settled_execution(db, runtime) {
        Err(e) if acknowledge && e.code() == "native_outside_outcome_unknown" => {
            if write {
                let count = db.execute(
                    "UPDATE runtime_requests SET result_json=json_set(CASE WHEN json_valid(result_json) THEN result_json ELSE '{}' END,
                        '$.settled',1,'$.outcome_unknown',1,'$.settled_by','owner_close_as_is','$.settled_at',?)
                     WHERE task_id=? AND operation IN ('outside_command','outside_write','command_run','project_write','project_edit',
                        'project_mkdir','project_delete','project_skill_link','draft_write','external_call')
                        AND json_extract(result_json,'$.complete') IS NOT 1 AND json_extract(result_json,'$.status') IS NOT 'complete'
                        AND json_extract(result_json,'$.settled') IS NOT 1",
                    (cortex_core::now(), runtime),
                )?;
                cortex_core::lifecycle::event_tx(
                    db,
                    runtime,
                    "OWNER_ACKNOWLEDGED_UNKNOWN_EFFECTS",
                    json!({"count":count,"request":"close_as_is"}),
                )?;
                cortex_core::lifecycle::automatic::require_settled_execution(db, runtime)?;
            }
            Ok(())
        }
        other => other,
    }
}
fn truth(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
    }
}
fn python_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        _ => v.to_string(),
    }
}
