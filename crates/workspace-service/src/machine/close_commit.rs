//! A task of an Automatic sprint that the owner closes as is is committed the way the
//! owner would by hand: its own recorded paths, at their current bytes, become one local
//! commit on the project folder's current branch, with a message saying it was closed
//! as is and not verified. Nothing else is staged or committed, nothing is pushed.
//! Without this every as-is close left its files uncommitted, and the next tasks'
//! change lists and reviews carried them.
use super::{
    context,
    git::Git,
    integration::REPO_BUSY,
    settings::Lock,
};
use cortex_core::OpenMode;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::path::Path;

/// Paths the task recorded writing or deleting through Cortex's own file tools.
fn recorded_paths(root: &Path, project: &str, task: &str) -> Vec<String> {
    let Ok(Some(store)) = context::open(root, OpenMode::LiveReadOnly) else {
        return Vec::new();
    };
    store
        .read(|db| {
            let runtime: Option<String> = db
                .query_row(
                    "SELECT e.runtime_task_id FROM workspace_tasks t JOIN workspace_executions e
                     ON e.execution_id=t.execution_id WHERE t.project_id=? AND t.task_id=?",
                    (project, task),
                    |r| r.get(0),
                )
                .optional()?;
            let Some(runtime) = runtime else {
                return Ok(Vec::new());
            };
            let rows = db
                .prepare(
                    "SELECT DISTINCT json_extract(payload_json,'$.path') FROM runtime_events
                     WHERE task_id=? AND json_valid(payload_json) AND (type='NATIVE_PROJECT_WRITTEN'
                        OR (type='NATIVE_PROJECT_CHANGED' AND json_extract(payload_json,'$.operation')='project_delete')
                        OR (type='TOOL_COMPLETED' AND json_extract(payload_json,'$.operation') IN
                            ('project_write','project_edit','project_delete')))",
                )?
                .query_map([&runtime], |r| r.get::<_, Option<String>>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows.into_iter().flatten().collect())
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|p: &String| {
            !p.is_empty()
                && !p.starts_with('/')
                && !p.starts_with('-')
                && p.split('/').all(|part| !part.is_empty() && part != "." && part != "..")
                && !matches!(p.split('/').next(), Some(".git" | ".cortex" | ".cortex-dev"))
        })
        .collect()
}

/// Reports the commit outcome for an eligible Automatic close. An ineligible close
/// has no commit outcome; a commit error never fails the already recorded close.
pub(super) async fn commit(
    root: &Path,
    project: &str,
    state: &Path,
    task: &str,
    git: &mut Git,
) -> Option<Value> {
    match run(root, project, state, task, git).await {
        Ok(outcome) => outcome,
        Err(error) => Some(json!({"committed":false,"reason":error.code})),
    }
}

async fn run(
    root: &Path,
    project: &str,
    state: &Path,
    task: &str,
    git: &mut Git,
) -> super::native::Result<Option<Value>> {
    let product = super::native::product(state, project)?;
    let record = &product["tasks"][task];
    let automatic = product["sprints"].as_array().into_iter().flatten().any(|s| {
        s["id"] == record["sprintId"] && super::product::automatic_sprint(s)
    });
    if record["status"] != "done"
        || record["closedAs"] != "as_is"
        || !automatic
        || record["checkout"]["kind"] != "main"
    {
        return Ok(None);
    }
    let skipped = |reason: &str| Ok(Some(json!({"committed":false,"reason":reason})));
    if !git.is_repo(root).await? || !git.has_history(root).await? {
        return skipped("no_git_history");
    }
    let branch = git.out(root, &["branch", "--show-current"]).await?.trim().to_owned();
    if branch.is_empty() {
        return skipped("detached_checkout");
    }
    let paths = recorded_paths(root, project, task);
    if paths.is_empty() {
        return skipped("no_recorded_paths");
    }
    // The same order as every other holder of both: the repository lock is taken
    // here, after the close released the machine lock.
    let _repo = Lock::long(
        state,
        &super::git::repo_lock(root),
        "integration_in_progress",
        REPO_BUSY,
    )?;
    let literal: Vec<String> = paths.iter().map(|p| format!(":(literal){p}")).collect();
    let mut status = vec!["status", "--porcelain", "-z", "--"];
    status.extend(literal.iter().map(String::as_str));
    if git.out(root, &status).await?.trim_matches('\0').trim().is_empty() {
        return skipped("nothing_to_commit");
    }
    let title = record["title"].as_str().unwrap_or(task).trim();
    let message = format!("Close task as is (not verified): {title}");
    let mut add = vec!["add", "-A", "--"];
    add.extend(literal.iter().map(String::as_str));
    git.out(root, &add).await?;
    let mut commit = vec!["commit", "--only", "-m", &message, "--"];
    commit.extend(literal.iter().map(String::as_str));
    if let Err(error) = git.out(root, &commit).await {
        // Leave the index as it was for these paths: nothing is half-committed.
        let mut reset = vec!["reset", "-q", "--"];
        reset.extend(literal.iter().map(String::as_str));
        let _ = git.out(root, &reset).await;
        return Err(error);
    }
    let head = git.out(root, &["rev-parse", "HEAD"]).await?.trim().to_owned();
    Ok(Some(json!({"committed":true,"branch":branch,"commit":head})))
}
