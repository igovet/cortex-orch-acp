//! Owner presentation actions, not scheduler transitions or result certification.
use super::{context, git::Git, native::*, retention, task_facts_git};
use cortex_core::{CoreError, CoreResult, OpenMode};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::path::Path;

pub fn supported(action: &str) -> bool {
    matches!(
        action,
        "merged" | "close" | "resolution_started"
    )
}
pub(super) fn unsettled() -> Refused {
    refuse(
        "stop_required",
        "Stop the task and wait for every worker to stop first.",
    )
}

pub(super) fn close_paused_external(
    root: &Path,
    project: &str,
    state: &Path,
    task: &str,
    request: &Value,
) -> Result<Value> {
    if request.as_object().is_none_or(|params| {
        params.keys().any(|key| {
            !matches!(
                key.as_str(),
                "action" | "taskId" | "executionId" | "expectedVersion" | "requestId" | "confirmed"
            )
        })
    }) {
        return Err(refuse(
            "invalid_request",
            "Refresh the task before closing.",
        ));
    }
    if request["confirmed"] != true {
        return Err(refuse(
            "confirmation_required",
            "Confirm completion outside Cortex.",
        ));
    }
    let mut product = super::native::product(state, project)?;
    let record = &product["tasks"][task];
    let current = record["sprintId"].as_str().map(str::to_owned);
    if truth(&record["archive"])
        || truth(&record["purged"])
        || truth(&record["merge"])
        || record
            .get("conflict")
            .is_some_and(|conflict| conflict != "none")
    {
        return Err(refuse(
            "external_close_unavailable",
            "Restore the task or abort its active merge before closing.",
        ));
    }
    let store = context::open(root, OpenMode::RuntimeValidate)?
        .ok_or_else(|| refuse("invalid_task", "Unknown task."))?;
    let path = product_path(state, project)?;
    cortex_core::workspace::control::close_paused_external(
        &store,
        project,
        &crate::actor(),
        task,
        request,
        || {
            let mut memberships = product["sprints"]
                .as_array_mut()
                .into_iter()
                .flatten()
                .filter(|sprint| {
                    sprint["taskIds"]
                        .as_array()
                        .is_some_and(|ids| ids.iter().any(|id| id == task))
                });
            let sprint = memberships.next();
            if memberships.next().is_some()
                || current.as_deref() != sprint.as_ref().and_then(|sprint| sprint["id"].as_str())
            {
                return Err(CoreError::Refused("sprint_membership_changed"));
            }
            if let Some(sprint) = sprint.filter(|sprint| {
                matches!(sprint["queueState"].as_str(), Some("running" | "paused"))
            }) {
                let saved = sprint
                    .as_object_mut()
                    .ok_or(CoreError::Refused("state_unreadable"))?;
                saved.insert("queueState".into(), json!("paused"));
                saved.insert("pauseReason".into(), json!("Paused by you"));
                for key in ["pauseKind", "questionExecutions", "automaticQueue"] {
                    saved.remove(key);
                }
                super::settings::write_json(&path, &product)
                    .map_err(|error| CoreError::Refused(error.code))?;
            }
            Ok(())
        },
    )
    .map_err(|error| match error {
        CoreError::ExternalStopPending { role, reference } => {
            let who = reference.map_or(role.clone(), |reference| format!("{role} ({reference})"));
            refuse(
                "external_stop_pending",
                format!("Waiting for confirmed exit of {who}. Check again after it stops."),
            )
        }
        CoreError::Refused("stale_version") => refuse(
            "stale_version",
            "The task changed. Refresh it before closing.",
        ),
        CoreError::Refused("task_not_paused") => {
            refuse("task_not_paused", "Pause the task before closing it.")
        }
        CoreError::Refused("sprint_membership_changed") => refuse(
            "sprint_membership_changed",
            "Refresh the task to confirm its sprint.",
        ),
        _ => settle_error(error),
    })
}

/// Match the retained _facts/require_settled boundary, including stopped launches.
/// The caller can hold its immediate transaction through the overlay replacement.
pub(crate) fn settled(db: &Connection, root: &Path, project: &str, task: &str) -> CoreResult<()> {
    let row = db
        .query_row(
            "SELECT e.runtime_task_id,e.acp_session_id,e.execution_id,e.owner_epoch
         FROM workspace_executions e JOIN workspace_tasks t USING(task_id)
         WHERE t.task_id=? AND t.project_id=?",
            (task, project),
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((runtime, session, execution, epoch)) = row else {
        return Ok(());
    };
    let owner: Option<i64> = db
        .query_row(
            "SELECT epoch FROM task_owners WHERE task_id=?",
            [&runtime],
            |r| r.get(0),
        )
        .optional()?;
    let session: Option<(String, Option<String>)> = db
        .query_row(
            "SELECT project_root,task_id FROM acp_host_sessions WHERE session_id=?",
            [session],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let base: Option<String> = db
        .query_row(
            "SELECT project_root FROM tasks WHERE id=?",
            [&runtime],
            |r| r.get(0),
        )
        .optional()?;
    let start: Option<String> = db
        .query_row(
            "SELECT state FROM workspace_starts WHERE execution_id=?",
            [execution],
            |r| r.get(0),
        )
        .optional()?;
    let state: Option<(String,i64,Option<String>)> = db.query_row(
        "SELECT r.state,r.revision,r.blocker FROM runtime_tasks r JOIN tasks t ON t.id=r.task_id WHERE r.task_id=?",
        [&runtime], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let bound = base.as_deref() == root.to_str()
        && session.as_ref().is_some_and(|(p, t)| {
            Some(p.as_str()) == root.to_str() && t.as_deref() == Some(runtime.as_str())
        });
    let quiet = match state {
        Some((state, revision, blocker)) => {
            bound
                && owner == Some(epoch)
                && revision >= 0
                && !(state == "COMPLETED" && blocker.is_some_and(|s| !s.is_empty()))
                && start.as_deref() != Some("unknown")
                && matches!(
                    state.as_str(),
                    "PAUSED" | "CANCELLED" | "COMPLETED" | "FAILED"
                )
        }
        None => {
            bound
                && owner.is_some_and(|n| n >= epoch)
                && matches!(start.as_deref(), Some("claimed" | "dispatched"))
                && db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM task_stopped WHERE task_id=?)",
                    [&runtime],
                    |r| r.get::<_, bool>(0),
                )?
        }
    };
    let live: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND status='running')",
        [&runtime],
        |r| r.get(0),
    )?;
    if !quiet || live {
        return Err(CoreError::Refused("stop_required"));
    }
    Ok(())
}
pub(super) fn settle_error(e: CoreError) -> Refused {
    match e.code() {
        "stop_required" | "lost_worker_termination_unconfirmed" => unsettled(),
        "native_outside_outcome_unknown" => refuse(e.code(), "A dispatched action still has an unknown outcome. Inspect the task activity before handing its folder to the next task."),
        "bound_external_close_required" => refuse(e.code(), "Refresh the paused task and confirm external completion for its current execution."),
        _ => context::core(e),
    }
}
pub(super) fn external_settled(
    db: &Connection,
    root: &Path,
    project: &str,
    task: &str,
) -> CoreResult<()> {
    let paused_native: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspace_tasks t
           JOIN workspace_executions e ON e.task_id=t.task_id AND e.execution_id=t.execution_id
           JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
           WHERE t.project_id=? AND t.task_id=? AND r.state='PAUSED'
             AND EXISTS(SELECT 1 FROM runtime_events v WHERE v.task_id=r.task_id
                 AND v.type IN ('TASK_CREATED','NATIVE_TASK_ADOPTED')
                 AND json_extract(v.payload_json,'$.execution_host')='rust'))",
        (project, task),
        |row| row.get(0),
    )?;
    if paused_native {
        return Err(CoreError::Refused("bound_external_close_required"));
    }
    settled(db, root, project, task)
}

pub(super) fn external(p: &Value) -> bool {
    p["action"] == "close" && matches!(p["choice"].as_str(), Some("external" | "already_in_target"))
}
pub(super) fn close_precondition(record: &Value, p: &Value) -> Result<()> {
    if !matches!(
        p["choice"].as_str(),
        Some("as_is" | "cancel" | "external" | "already_in_target")
    ) || p["confirmed"] != true
    {
        return Err(refuse(
            "confirmation_required",
            "Choose how to close the task and confirm.",
        ));
    }
    if record["status"] == "done" && record["closedAs"] == "external" {
        return Err(refuse("already_closed", "This task was already closed outside Cortex."));
    }
    if external(p) {
        if record["status"] == "done" {
            return Err(refuse(
                "already_closed",
                "This task is already closed. Refresh its state.",
            ));
        }
        if record.get("conflict").is_some_and(|v| v != "none") {
            return Err(refuse(
                "merge_in_progress",
                "Abort the active merge before closing the task.",
            ));
        }
    }
    Ok(())
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
/// The explicit destructive choice does not stop an active run, acknowledge an
/// unknown effect, or certify verification. Only settled, owned checkouts qualify.
pub(super) async fn close_with_cleanup(
    root: &Path,
    project: &str,
    state: &Path,
    task: &str,
    product: &mut Value,
    p: &Value,
    git: &mut Git,
) -> Result<Value> {
    use cortex_core::workspace::worktrees::{finish_cleanup, queue_close_tx, Cleanup};
    if p["confirmed"] != true {
        return Err(refuse("confirmation_required", "Confirm closing and deleting the task worktree."));
    }
    let store = context::open(root, OpenMode::RuntimeValidate)?
        .ok_or_else(|| refuse("worktree_not_owned", "Cortex did not create a worktree for this task."))?;
    let record = &mut product["tasks"][task];
    if truth(&record["archive"]) || truth(&record["purged"]) || truth(&record["merge"]) {
        return Err(refuse("close_worktree_unavailable", "The task is archived, removed, or has a merge in progress."));
    }
    if record["status"] != "done" {
        // Reuse the close-as-is presentation, without its optional acknowledgement
        // of an unknown effect or any mutation of the runtime's state machine.
        apply(root, project, task, record, &json!({"action":"close","choice":"as_is","confirmed":true}), git).await?;
    }
    store.write(|db| {
        queue_close_tx(db, project, task)?;
        super::settings::write_json(
            &product_path(state, project).map_err(|e| CoreError::Refused(e.code))?,
            product,
        ).map_err(|e| CoreError::Refused(e.code))?;
        Ok(())
    }).map_err(|e| match e.code() {
        "active_execution" => unsettled(),
        "worktree_not_owned" => refuse(e.code(), "Cortex did not create a worktree for this task."),
        _ => settle_error(e),
    })?;
    let cleanup = finish_cleanup(&store, project, task, Cleanup::Close)
        .ok().flatten().unwrap_or_else(|| json!({"status":"pending","reason":"worktree_cleanup_unconfirmed"}));
    Ok(json!({"done":true,"worktreeCleanup":cleanup}))
}

pub async fn apply(
    root: &Path,
    project: &str,
    task: &str,
    record: &mut Value,
    p: &Value,
    git: &mut Git,
) -> Result<()> {
    let at = now_ms();
    match p["action"].as_str().unwrap_or("") {
        "merged" => {
            record["status"] = json!("done");
            record["work"] = json!("merged");
            record["doneAt"] = json!(at);
            record["integration"] = p["integration"].clone();
        }
        "close" => {
            let choice = p["choice"].as_str().unwrap_or("");
            close_precondition(record, p)?;
            if choice == "cancel" {
                close_cancelled(record, at);
                return Ok(());
            }
            if external(p) {
                if let Some(store) = context::open(root, OpenMode::LiveReadOnly)? {
                    store
                        .read(|db| external_settled(db, root, project, task))
                        .map_err(settle_error)?;
                }
                if choice == "already_in_target" {
                    let facts = task_facts_git::read_for_finish(root, record, task, git).await?;
                    let Some(target) = facts["targetBranch"]
                        .as_str()
                        .filter(|target| !target.is_empty() && facts["alreadyInTarget"] == true)
                    else {
                        return Err(refuse(
                            "containment_changed",
                            "The target changed. Refresh before closing; nothing was merged.",
                        ));
                    };
                    // Containment records where to restart, not whether the result was accepted.
                    record["alreadyInTargetBranch"] = json!(target);
                }
                for (k, v) in [
                    ("notVerifiedByCortex", json!(true)),
                    ("acceptance", Value::Null),
                    ("integration", Value::Null),
                    ("automaticReady", json!(false)),
                    ("work", json!("unmerged")),
                ] {
                    record[k] = v;
                }
                record["closedAs"] = json!("external");
            } else {
                // An old automatic acceptance cannot turn a changed worktree into a
                // verified close. Keep the task's evidence honest when the owner uses
                // Close as is after the result became stale.
                let result_matches_workspace = record["workspaceDigest"]
                    .as_str()
                    .zip(record["result"]["digest"].as_str())
                    .is_none_or(|(workspace, result)| workspace == result);
                let acceptance_matches_workspace = record["acceptance"]["digest"]
                    .as_str()
                    .zip(record["workspaceDigest"].as_str())
                    .is_none_or(|(accepted, workspace)| accepted == workspace);
                let checked = record["status"] == "ready_to_merge"
                    && (truth(&record["acceptance"]) || truth(&record["automaticReady"]))
                    && result_matches_workspace
                    && acceptance_matches_workspace;
                record["notVerifiedByCortex"] = json!(choice == "as_is" && !checked);
                record["closedAs"] = json!(choice);
            }
            record["status"] = json!("done");
            record["doneAt"] = json!(at);
        }
        "resolution_started" => {
            // Keep the old action an explicit refusal, not a second write path.
            retention::task_path(root, record, git).await?;
            if !truth(&record["merge"]) {
                return Err(refuse("no_conflict", "There is no retained merge conflict for this task."));
            }
            return Err(refuse("handoff_required", "Use the owning execution's handoff_merge action."));
        }
        _ => {
            return Err(refuse(
                "unsupported_action",
                "This machine does not support that yet.",
            ))
        }
    }
    Ok(())
}

/// Cancellation closes the presentation only, without accepting a result or deleting history.
pub(super) fn close_cancelled(record: &mut Value, at: u64) {
    record["notVerifiedByCortex"] = json!(false);
    record["closedAs"] = json!("cancel");
    record["status"] = json!("done");
    record["doneAt"] = json!(at);
}

/// The archive disposition never discards work or changes the result.
pub(super) fn archive(record: &mut Value, at: u64, previous_status: &str) {
    record["archive"] = json!({"at":at,"previousStatus":previous_status});
    record["archiveQuestion"] = json!(false);
    record.as_object_mut().unwrap().remove("archiveAskAfter");
}
