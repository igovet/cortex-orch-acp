//! Retention and two-phase sprint deletion. Worktrees are never force-removed.
use super::{
    context,
    git::Git,
    native::*,
    settings::{self, Lock},
};
use cortex_core::{CoreError, OpenMode};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
const DAY: u64 = 86_400_000;
fn invalid() -> Refused {
    refuse(
        "state_unreadable",
        "Saved task or sprint state is unreadable.",
    )
}
/// One owner-readable sentence for a task that sprint deletion could not remove yet:
/// the task's title (never its internal id or a raw refusal code) and what to do.
fn deletion_blocker(
    store: &cortex_core::Store,
    project: &str,
    task: &str,
    record: &Value,
    code: &str,
) -> String {
    let title = record["title"]
        .as_str()
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_owned)
        .or_else(|| {
            store
                .read(|db| {
                    Ok(db
                        .query_row(
                            "SELECT description FROM workspace_tasks WHERE project_id=? AND task_id=?",
                            (project, task),
                            |r| r.get::<_, String>(0),
                        )
                        .optional()?)
                })
                .ok()
                .flatten()
        })
        .map(|t| {
            let line = t.lines().next().unwrap_or("").trim().to_owned();
            if line.chars().count() > 80 {
                format!("{}…", line.chars().take(79).collect::<String>())
            } else {
                line
            }
        })
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| "One task".to_owned());
    match code {
        "active_execution" => format!(
            "“{title}” is still running, or Cortex is still waiting to confirm that its engine processes stopped. Stop or close the task and wait until it shows as stopped; if it stays like this, reopen Cortex while no other agent app works in this project folder, so Cortex can confirm the stop."
        ),
        "unauthorized" | "not_found" => format!(
            "“{title}” no longer belongs to this project's saved state, so Cortex cannot confirm that it is safe to delete. Refresh the project."
        ),
        _ => format!("“{title}” could not be deleted yet."),
    }
}
fn tasks_mut(product: &mut Value) -> Result<&mut serde_json::Map<String, Value>> {
    product["tasks"].as_object_mut().ok_or_else(invalid)
}
pub(super) async fn task_path(root: &Path, record: &Value, git: &mut Git) -> Result<PathBuf> {
    if record["checkout"]["kind"]
        .as_str()
        .is_none_or(|s| s == "main")
    {
        return Ok(root.to_owned());
    }
    let path = string(&record["checkout"], "path")?;
    let branch = string(&record["checkout"], "branch")?;
    if !git
        .trees(root)
        .await?
        .iter()
        .any(|t| t["path"] == path && t["branch"] == branch)
    {
        return Err(refuse(
            "unknown_checkout",
            "The task worktree is missing or its branch changed.",
        ));
    }
    Ok(PathBuf::from(path))
}
pub(super) async fn unmerged(root: &Path, record: &Value, git: &mut Git) -> Result<bool> {
    if record["integration"]["worktreeCleanup"] == "removed"
        && record["checkout"]["path"]
            .as_str()
            .is_some_and(|p| !p.is_empty() && !Path::new(p).exists())
    {
        return Ok(false);
    }
    let path = task_path(root, record, git).await?;
    if git.is_repo(&path).await? {
        let head = &record["integration"]["sourceHead"];
        if super::workers::truthy(head)
            && json!(git.out(&path, &["rev-parse", "HEAD"]).await?.trim()) != *head
        {
            return Ok(true);
        }
        if !git.clean(&path).await? {
            return Ok(true);
        }
    }
    Ok(!matches!(
        record["work"].as_str(),
        Some("merged" | "discard_confirmed")
    ))
}
async fn observation(root: &Path) -> Result<String> {
    crate::native_effects::observe(root.to_owned())
        .await
        .map_err(context::core)?
        .project_digest
        .ok_or_else(|| refuse("workspace_unobservable", "The work could not be observed."))
}
pub(super) async fn purge(
    root: &Path,
    project: &str,
    state: &Path,
    product: &mut Value,
    task: &str,
    git: &mut Git,
) -> Result<u64> {
    purge_impl(root, project, state, product, task, git, false).await
}
pub(super) async fn purge_owner(
    root: &Path,
    project: &str,
    state: &Path,
    product: &mut Value,
    task: &str,
    git: &mut Git,
) -> Result<u64> {
    purge_impl(root, project, state, product, task, git, true).await
}
fn owner_purge_error(error: cortex_core::CoreError) -> Refused {
    if matches!(
        error.code(),
        "not_found" | "active_execution" | "unauthorized"
    ) {
        refuse(
            error.code(),
            "This task is still active, unknown, or no longer belongs to this project.",
        )
    } else {
        context::core(error)
    }
}
async fn purge_impl(
    root: &Path,
    project: &str,
    state: &Path,
    product: &mut Value,
    task: &str,
    git: &mut Git,
    owner: bool,
) -> Result<u64> {
    let record = product["tasks"][task].clone();
    if !super::workers::truthy(&record["archive"]) {
        return Err(refuse(
            "not_archived",
            "Only archived tasks can be deleted.",
        ));
    }
    let discarded = if record["work"] == "discard_confirmed" {
        let path = task_path(root, &record, git).await?;
        if owner {
            // Retained owner decisions bind to the legacy manifest, not the
            // distinct native Core verification digest. Unobservable is no consent.
            super::legacy_source::capture(&path)
                .ok()
                .flatten()
                .is_some_and(|sha| record["discardRevision"] == sha)
        } else {
            observation(&path)
                .await
                .ok()
                .is_some_and(|sha| record["discardRevision"] == sha)
        }
    } else {
        false
    };
    if unmerged(root, &record, git).await? && !discarded {
        product["tasks"][task]["archiveQuestion"] = json!(true);
        product["tasks"][task]["archive"] = Value::Null;
        settings::write_json(&product_path(state, project)?, product)?;
        return Err(refuse(
            "unmerged_work",
            "This task has unmerged work. Decide whether it may be discarded first.",
        ));
    }
    let result = if let Some(store) = context::open(root, OpenMode::RuntimeValidate)? {
        let exists = store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM workspace_tasks WHERE project_id=? AND task_id=?)",
                    (project, task),
                    |r| r.get::<_, bool>(0),
                )?)
            })
            .map_err(context::core)?;
        if exists || owner {
            store
                .purge_workspace_task(project, task, true)
                .map_err(|e| {
                    if owner {
                        owner_purge_error(e)
                    } else {
                        context::core(e)
                    }
                })?
        } else {
            json!({"vacuum_status":"not_needed","reclaimed_bytes":0})
        }
    } else {
        json!({"vacuum_status":"not_needed","reclaimed_bytes":0})
    };
    let mut retained = Value::Null;
    if record["checkout"]["kind"] == "worktree"
        && record["integration"]["worktreeCleanup"] != "removed"
    {
        let path = task_path(root, &record, git).await?;
        if path.exists() {
            retained = json!(path);
            if record["checkout"]["createdByWorkspace"] == true
                && path != root
                && git.clean(&path).await?
            {
                if git
                    .out(
                        root,
                        &[
                            "worktree",
                            "remove",
                            "--",
                            path.to_str().ok_or_else(invalid)?,
                        ],
                    )
                    .await
                    .is_ok()
                {
                    retained = Value::Null;
                }
            }
        }
    }
    let reclamation = if owner {
        result
            .as_object()
            .ok_or_else(invalid)?
            .iter()
            .filter(|(k, _)| {
                ["vacuum_status", "reclaimed_bytes", "vacuum_error"].contains(&k.as_str())
            })
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<serde_json::Map<_, _>>()
            .into()
    } else {
        result.clone()
    };
    product["tasks"][task] = json!({"purged":true,"reclamation":reclamation});
    if !retained.is_null() {
        product["tasks"][task]["retainedWorktree"] = retained;
    }
    product["messages"]
        .as_array_mut()
        .ok_or_else(invalid)?
        .retain(|v| v["taskId"] != task);
    product["comments"]
        .as_object_mut()
        .ok_or_else(invalid)?
        .remove(task);
    for s in product["sprints"].as_array_mut().ok_or_else(invalid)? {
        s["taskIds"]
            .as_array_mut()
            .ok_or_else(invalid)?
            .retain(|v| v != task);
    }
    Ok(result["reclaimed_bytes"].as_u64().unwrap_or(0))
}
pub async fn cleanup(state: &Path, projects: &Value, git: &mut Git) -> Result<Value> {
    cleanup_at(state, projects, git, now_ms()).await
}
async fn cleanup_at(state: &Path, projects: &Value, git: &mut Git, at: u64) -> Result<Value> {
    let settings = settings::machine(state)?;
    let _lock = Lock::new(state, "machine.lock")?;
    let (mut removed, mut skipped, mut asked, mut freed, mut failed) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    for project in projects.as_array().ok_or_else(invalid)? {
        let root = Path::new(string(project, "root")?);
        let id = string(project, "id")?;
        crate::project_guard::check(root)?;
        let mut product = context::product(root, state, id, git).await?;
        let ids = product["tasks"]
            .as_object()
            .ok_or_else(invalid)?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for task in ids {
            let record = product["tasks"][&task].clone();
            let archive = record["archive"]["at"].as_u64();
            if archive.is_some_and(|a| {
                at.saturating_sub(a) >= settings["retentionDays"].as_u64().unwrap_or(30) * DAY
            }) && record["purgeBlocker"].is_null()
            {
                match purge(root, id, state, &mut product, &task, git).await {
                    Ok(n) => {
                        freed += n;
                        removed += 1;
                        failed += u64::from(!matches!(
                            product["tasks"][&task]["reclamation"]["vacuum_status"].as_str(),
                            Some("completed" | "not_needed")
                        ));
                    }
                    Err(e) => {
                        product["tasks"][&task]["purgeBlocker"] = json!(e.message);
                        skipped += 1;
                        asked += u64::from(e.code == "unmerged_work");
                    }
                }
            } else if record["status"] == "done"
                && record["archive"].is_null()
                && record["doneAt"].as_u64().is_some_and(|a| {
                    at.saturating_sub(a) >= settings["autoArchiveDays"].as_u64().unwrap_or(7) * DAY
                })
            {
                if !unmerged(root, &record, git).await? {
                    product["tasks"][&task]["archive"] = json!({"at":at,"previousStatus":"done"});
                } else if record["archiveAskAfter"].as_u64().unwrap_or(0) <= at {
                    product["tasks"][&task]["archiveQuestion"] = json!(true);
                    asked += 1;
                }
            }
        }
        settings::write_json(&product_path(state, id)?, &product)?;
    }
    let run = json!({"at":at,"removed":removed,"skipped":skipped,"asked":asked,"freedMb":freed as f64/(1024.0*1024.0),"reclamationFailed":failed});
    let mut runs = settings::read_json(&state.join("machine-runs.json"), json!({}))?;
    runs["cleanup"] = run.clone();
    settings::write_json(&state.join("machine-runs.json"), &runs)?;
    Ok(run)
}
pub async fn delete_sprint(
    root: &Path,
    project: &str,
    state: &Path,
    p: &Value,
    git: &mut Git,
) -> Result<Value> {
    if p["confirmed"] != true {
        return Err(refuse(
            "confirmation_required",
            "Confirm before deleting the sprint and its tasks.",
        ));
    }
    let _lock = Lock::new(state, "machine.lock")?;
    let mut product = context::product(root, state, project, git).await?;
    let id = string(p, "sprintId")?;
    let index = product["sprints"]
        .as_array()
        .ok_or_else(invalid)?
        .iter()
        .position(|s| s["id"] == id);
    let Some(index) = index else {
        return product["deletedSprints"]
            .get(id)
            .cloned()
            .ok_or_else(|| refuse("unknown_sprint", "Unknown sprint."));
    };
    let sprint = product["sprints"][index].clone();
    let mut members = Vec::<String>::new();
    for task in sprint
        .get("deleteTaskIds")
        .unwrap_or(&sprint["taskIds"])
        .as_array()
        .ok_or_else(invalid)?
    {
        let task = task.as_str().ok_or_else(invalid)?.to_owned();
        if !members.contains(&task) {
            members.push(task);
        }
    }
    for (task, record) in product["tasks"].as_object().ok_or_else(invalid)? {
        if record["sprintId"] == id && !members.contains(task) {
            members.push(task.clone());
        }
    }
    if let Some(task) = sprint["planningTaskId"].as_str() {
        if !members.contains(&task.to_owned()) {
            members.push(task.to_owned());
        }
    }
    if members.iter().any(|task| {
        !product["tasks"][task]["sprintId"].is_null() && product["tasks"][task]["sprintId"] != id
    }) {
        return Err(refuse(
            "membership_changed",
            "A task now belongs to another sprint. Refresh and retry.",
        ));
    }
    let s = &mut product["sprints"][index];
    s["deleting"] = json!(true);
    s["queueState"] = json!("paused");
    if s["archivedAt"].is_null() {
        s["archivedAt"] = json!(now_ms());
    }
    s["pauseReason"] = json!("Deletion pending — retry Delete sprint");
    s["deleteTaskIds"] = json!(members);
    settings::write_json(&product_path(state, project)?, &product)?;
    let store = context::open(root, OpenMode::RuntimeValidate)?;
    let executions = if let Some(store) = &store {
        store.write(|db|{
        let mut ids=Vec::new();
        for task in &members {
            let row:Option<Option<String>>=db.query_row("SELECT execution_id FROM workspace_tasks WHERE project_id=? AND task_id=?",(project,task),|r|r.get(0)).optional()?;
            let Some(execution)=row else {continue;};
            let raw:Option<String>=db.query_row("SELECT options_json FROM workspace_task_options WHERE task_id=?",[task],|r|r.get(0)).optional()?;
            let mut options:Value=serde_json::from_str(raw.as_deref().unwrap_or("{}"))?;
            if !options.is_object() {return Err(CoreError::Refused("state_unreadable"));}
            options["deletingSprint"]=json!(id);
            db.execute("INSERT INTO workspace_task_options VALUES (?,?) ON CONFLICT(task_id) DO UPDATE SET options_json=excluded.options_json",(task,options.to_string()))?;
            if let Some(execution)=execution {
                let state:Option<Option<String>>=db.query_row("SELECT r.state FROM workspace_executions e LEFT JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE e.execution_id=?",[&execution],|r|r.get(0)).optional()?;
                if !matches!(state.flatten().as_deref(),Some("COMPLETED"|"FAILED"|"CANCELLED")) {ids.push(execution);}
            }
        }
        Ok(ids)
    }).map_err(context::core)?
    } else {
        Vec::new()
    };
    if p["action"] == "delete_prepare" {
        return Ok(json!({"done":false,"executionIds":executions}));
    }
    let mut kept = sprint["keptWorktrees"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut deleted = sprint["deletedTaskIds"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !sprint["planningAttempt"].is_null() && sprint["deletePlannerStopped"] != true {
        return Ok(
            json!({"done":false,"reason":"Waiting for the sprint planner to stop. Retry Delete sprint.","taskIds":deleted,"keptPaths":kept}),
        );
    }
    let mut blockers = Vec::new();
    for task in &members {
        if deleted.contains(&json!(task)) {
            continue;
        }
        let record = product["tasks"][task].clone();
        let path = if record["checkout"]["kind"] == "worktree" {
            record["checkout"]["path"].as_str().map(PathBuf::from)
        } else {
            None
        };
        if let Some(path) = &path {
            if !kept.contains(&json!(path)) {
                kept.push(json!(path));
            }
        }
        product["sprints"][index]["keptWorktrees"] = json!(kept);
        settings::write_json(&product_path(state, project)?, &product)?;
        if let Some(store) = &store {
            let exists=store.read(|db|Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM workspace_tasks WHERE project_id=? AND task_id=?)",(project,task),|r|r.get::<_,bool>(0))?)).map_err(context::core)?;
            if exists {
                if let Err(e) = store.purge_workspace_task(project, task, false) {
                    blockers.push(deletion_blocker(store, project, task, &record, e.code()));
                    continue;
                }
            }
        }
        if let Some(path) = &path {
            if !path.exists() {
                kept.retain(|p| p != &json!(path));
            } else if record["checkout"]["createdByWorkspace"] == true && path != root {
                if let Ok(verified) = task_path(root, &record, git).await {
                    if git.clean(&verified).await?
                        && git
                            .out(
                                root,
                                &[
                                    "worktree",
                                    "remove",
                                    "--",
                                    verified.to_str().ok_or_else(invalid)?,
                                ],
                            )
                            .await
                            .is_ok()
                    {
                        kept.retain(|p| p != &json!(path));
                    }
                }
            }
        }
        tasks_mut(&mut product)?.insert(task.clone(), json!({"purged":true}));
        product["messages"]
            .as_array_mut()
            .ok_or_else(invalid)?
            .retain(|v| v["taskId"] != *task);
        product["comments"]
            .as_object_mut()
            .ok_or_else(invalid)?
            .remove(task);
        deleted.push(json!(task));
        product["sprints"][index]["keptWorktrees"] = json!(kept);
        product["sprints"][index]["deletedTaskIds"] = json!(deleted);
        settings::write_json(&product_path(state, project)?, &product)?;
    }
    if !blockers.is_empty() {
        return Ok(
            json!({"done":false,"reason":format!("Not every task could be deleted yet. {} Retry Delete sprint.",blockers.join(" ")),"taskIds":deleted,"keptPaths":kept}),
        );
    }
    let result = json!({"done":true,"taskIds":members,"keptPaths":kept});
    product["sprints"]
        .as_array_mut()
        .ok_or_else(invalid)?
        .remove(index);
    if product.get("deletedSprints").is_none() {
        product["deletedSprints"] = json!({});
    }
    product["deletedSprints"][id] = result.clone();
    settings::write_json(&product_path(state, project)?, &product)?;
    Ok(result)
}
