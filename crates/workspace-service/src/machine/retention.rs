//! Retention and two-phase sprint deletion. Worktrees are never force-removed.
use super::{
    context,
    git::Git,
    native::*,
    settings::{self, Lock},
};
use cortex_core::{
    workspace::worktrees::{self, Cleanup},
    CoreError, OpenMode,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};
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
    _state: &Path,
    product: &mut Value,
    task: &str,
    git: &mut Git,
) -> Result<u64> {
    purge_impl(root, project, product, task, git, Purge::Manual).await
}
pub(super) async fn purge_owner(
    root: &Path,
    project: &str,
    _state: &Path,
    product: &mut Value,
    task: &str,
    git: &mut Git,
) -> Result<u64> {
    purge_impl(root, project, product, task, git, Purge::Owner).await
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
enum Purge {
    Manual,
    Owner,
    Scheduled { at: u64, days: u64 },
}

async fn purge_impl(
    root: &Path,
    project: &str,
    product: &mut Value,
    task: &str,
    git: &mut Git,
    mode: Purge,
) -> Result<u64> {
    let owner = matches!(mode, Purge::Owner);
    let mut record = product["tasks"][task].clone();
    if !super::workers::truthy(&record["archive"]) {
        return Err(refuse(
            "not_archived",
            "Only archived tasks can be deleted.",
        ));
    }
    // Resolve removable checkout and validate the bookkeeping before Core
    // deletion. Post-commit Git failure only retains the checkout.
    if !product["messages"].is_array() || !product["comments"].is_object()
        || !product["sprints"].as_array().is_some_and(|s| s.iter().all(|s| s["taskIds"].is_array()))
    {
        return Err(invalid());
    }
    let store = if matches!(mode, Purge::Scheduled { .. }) {
        Some(cortex_core::Store::open_for_workspace_retention(context::key(root)?)
            .map_err(context::core)?)
    } else {
        context::open(root, OpenMode::RuntimeValidate)?
    };
    let (owned, committed_purge) = if let Some(store) = &store {
        let owned = worktrees::owned(store, project, task).map_err(context::core)?;
        let prior = worktrees::cleanup_outcome(store, project, task, Cleanup::Purge)
            .map_err(context::core)?;
        if let Some(binding) = &owned {
            record["checkout"] = json!({"kind":"worktree","path":binding.path,"branch":binding.branch});
            if worktrees::cleanup_outcome(store, project, task, Cleanup::Close)
                .map_err(context::core)?.is_some_and(|r| r["status"] == "removed")
            {
                record["integration"]["worktreeCleanup"] = json!("removed");
            }
        }
        (owned, prior)
    } else {
        (None, None)
    };
    let preflight = async {
        let removed = record["integration"]["worktreeCleanup"] == "removed";
        let scheduled = matches!(mode, Purge::Scheduled { .. });
        // A lost owned checkout cannot authorize discarding work. If the
        // disposition was already merged, however, an absent checkout must not
        // silently redirect retention to main or block reporting it as retained.
        if record["work"] == "merged" && owned.as_ref().is_some_and(|w| !w.path.exists()) {
            return Ok(());
        }
        if record["checkout"]["kind"] == "worktree"
            && (!removed || scheduled && record["checkout"]["path"].as_str()
                .is_none_or(|path| Path::new(path).try_exists().unwrap_or(true)))
        {
            let path = task_path(root, &record, git).await?;
            if scheduled && !git.clean(&path).await? {
                return Err(refuse("dirty_checkout", "A dirty task worktree was retained."));
            }
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
            // The task stays in the Archive with its work; nothing is changed or deleted.
            return Err(refuse(
                "unmerged_work",
                "This task has unmerged work. Decide whether it may be discarded first.",
            ));
        }
        Ok(())
    };
    if committed_purge.is_none() {
        if matches!(mode, Purge::Scheduled { .. }) {
            tokio::time::timeout(Duration::from_secs(2), preflight).await
                .map_err(|_| refuse("retention_busy", "The checkout could not be confirmed in time."))??;
        } else {
            preflight.await?;
        }
    }
    let result = if committed_purge.is_some() {
        json!({"deleted":true,"vacuum_status":"not_needed","reclaimed_bytes":0})
    } else if let Purge::Scheduled { at, days } = mode {
        store.as_ref().ok_or_else(invalid)?
            .purge_expired_workspace_archive(project, task, &record, at, days)
            .map_err(context::core)?
    } else if let Some(store) = &store {
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
    // A record's path/createdByWorkspace fields never authorize destruction.
    // The pending disposition in Core survived deletion of the task row.
    let cleanup = if let Some(store) = &store {
        match worktrees::finish_cleanup(store, project, task, Cleanup::Purge) {
            Ok(result) => result,
            Err(_) => Some(json!({"status":"pending","reason":"worktree_cleanup_unconfirmed"})),
        }
    } else {
        None
    }.or_else(|| {
        if record["checkout"]["kind"] != "worktree" || record["integration"]["worktreeCleanup"] == "removed" {
            return None;
        }
        Some(json!({"status":"retained","path":record["checkout"]["path"],"reason":"worktree_not_owned"}))
    });
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
    if let Some(cleanup) = cleanup {
        if cleanup["status"] == "retained" && !cleanup["path"].is_null() {
            product["tasks"][task]["retainedWorktree"] = cleanup["path"].clone();
        }
        product["tasks"][task]["worktreeCleanup"] = cleanup;
    }
    product["messages"]
        .as_array_mut()
        .ok_or_else(invalid)?
        .retain(|v| v["taskId"] != task);
    product["comments"]
        .as_object_mut()
        .ok_or_else(invalid)?
        .remove(task);
    super::attachment_files::forget_answers(product, task);
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
    let (mut removed, mut skipped, mut asked, mut freed, mut failed) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    for project in projects.as_array().ok_or_else(invalid)? {
        let root = Path::new(string(project, "root")?);
        let id = string(project, "id")?;
        crate::project_guard::check(root)?;
        // One project at a time: owner actions of the other projects never wait for
        // this pass. Each purge stays one journaled deletion under this lock.
        let _repo = Lock::new(state, &super::git::repo_lock(root))?;
        let _lock = Lock::new(state, "machine.lock")?;
        let mut product = context::product(root, state, id, git).await?;
        let ids = product["tasks"]
            .as_object()
            .ok_or_else(invalid)?
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for task in ids {
            // A task whose merge is running is left for a later pass.
            if settings::idle(state, id, &task, &_lock).is_err() {
                continue;
            }
            let record = product["tasks"][&task].clone();
            let days = settings["retentionDays"].as_u64().unwrap_or(30);
            let archive = &record["archive"];
            let expired = if archive["previousStatus"] == "draft" {
                // Not-started tasks have no completion clock. Only an explicit
                // task deletion can attempt to purge an archived draft.
                false
            } else if archive.get("retentionAt").is_some() {
                cortex_core::store::expired_closed_archive(&record, at, days)
            } else {
                archive["at"].as_u64().is_some_and(|a| at.saturating_sub(a) >= days * DAY)
            };
            if expired && record["purgeBlocker"].is_null()
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
            } else {
                archive_done(&mut product["tasks"][&task], at, &settings);
            }
        }
        settings::write_json(&product_path(state, id)?, &product)?;
    }
    let run = json!({"at":at,"removed":removed,"skipped":skipped,"asked":asked,"freedMb":freed as f64/(1024.0*1024.0),"reclamationFailed":failed});
    let _lock = Lock::new(state, "machine.lock")?;
    let mut runs = settings::read_json(&state.join("machine-runs.json"), json!({}))?;
    runs["cleanup"] = run.clone();
    settings::write_json(&state.join("machine-runs.json"), &runs)?;
    // Cleanup also removes attachment bodies nothing references any more.
    let _ = super::attachment_files::sweep_locked(state, SystemTime::now());
    Ok(run)
}
fn archive_done(record: &mut Value, at: u64, settings: &Value) -> bool {
    if record["status"] != "done" || !record["archive"].is_null()
        || !settings["autoArchiveDays"].as_u64().filter(|d| (1..=3650).contains(d))
            .zip(record["doneAt"].as_u64())
            .is_some_and(|(days, done)| at.saturating_sub(done) >= days * DAY)
    {
        return false;
    }
    super::task_lifecycle::archive(record, at, "done");
    true
}

/// A rotating cursor ensures repeatedly busy/refused archives cannot starve
/// other records. An attempted deletion is always settled before advancing.
const PURGES_PER_VISIT: usize = 4;

pub(crate) async fn scheduled_project(
    config: &crate::Config,
    root: &Path,
    id: &str,
    at: u64,
    cursor: &mut Option<String>,
    stopping: impl Fn() -> bool,
) -> Result<bool> {
    let state = &config.state_dir;
    crate::project_guard::check(root)?;
    let Some(_repo) = Lock::try_new(state, &super::git::repo_lock(root))? else { return Ok(false) };
    let Some(lock) = Lock::try_new(state, "machine.lock")? else { return Ok(false) };
    let settings = settings::machine(state)?;
    let mut git = Git::for_service(config);
    let mut product = tokio::time::timeout(Duration::from_secs(2), context::product(root, state, id, &mut git))
        .await.map_err(|_| refuse("retention_busy", "Project facts were not available for retention."))??;
    if stopping() {
        return Ok(false);
    }
    let path = product_path(state, id)?;
    let mut ids = product["tasks"].as_object().ok_or_else(invalid)?.keys().cloned().collect::<Vec<_>>();
    ids.sort_unstable();
    let mut changed = false;
    for task in &ids {
        if settings::idle(state, id, task, &lock).is_ok() {
            changed |= archive_done(&mut product["tasks"][task], at, &settings);
        }
    }
    if changed {
        settings::write_json(&path, &product)?;
    }
    if settings["automaticArchivePurge"] == true {
        if let Some(days) = settings["retentionDays"].as_u64() {
            let start = ids.partition_point(|id| cursor.as_ref().is_some_and(|c| id <= c));
            let mut attempted = 0;
            for task in ids[start..].iter().chain(&ids[..start]) {
                if stopping() {
                    break;
                }
                if !cortex_core::store::expired_closed_archive(&product["tasks"][task], at, days)
                    || settings::idle(state, id, task, &lock).is_err()
                {
                    continue;
                }
                *cursor = Some(task.clone());
                attempted += 1;
                let result = purge_impl(root, id, &mut product, task, &mut git, Purge::Scheduled { at, days }).await;
                if result.is_ok() {
                    settings::write_json(&path, &product)?;
                    changed = true;
                }
                if attempted == PURGES_PER_VISIT {
                    break;
                }
            }
        }
    }
    Ok(changed)
}

pub(crate) fn scheduled_attachments(state: &Path, at: SystemTime) {
    if let Ok(Some(_lock)) = Lock::try_new(state, "machine.lock") {
        let _ = super::attachment_files::sweep_locked(state, at);
    }
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
    let _repo = Lock::new(state, &super::git::repo_lock(root))?;
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
    // A member still being merged is never deleted under it; say so at once.
    for task in &members {
        settings::idle(state, project, task, &_lock)?;
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
        let cleanup = store.as_ref().and_then(|store| {
            worktrees::finish_cleanup(store, project, task, Cleanup::Purge)
                .unwrap_or_else(|_| Some(json!({"status":"pending","reason":"worktree_cleanup_unconfirmed"})))
        });
        if let Some(path) = &path {
            if !path.exists() || cleanup.as_ref().is_some_and(|outcome| {
                outcome["status"] == "removed" && outcome["path"] == json!(path)
            }) {
                kept.retain(|p| p != &json!(path));
            }
        }
        let mut tombstone = json!({"purged":true});
        if let Some(cleanup) = cleanup {
            tombstone["worktreeCleanup"] = cleanup;
        }
        tasks_mut(&mut product)?.insert(task.clone(), tombstone);
        product["messages"]
            .as_array_mut()
            .ok_or_else(invalid)?
            .retain(|v| v["taskId"] != *task);
        product["comments"]
            .as_object_mut()
            .ok_or_else(invalid)?
            .remove(task);
        super::attachment_files::forget_answers(&mut product, task);
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
    // The deleted sprint's files go with it, once nothing else references them.
    let _ = super::attachment_files::sweep_locked(state, SystemTime::now());
    Ok(result)
}
