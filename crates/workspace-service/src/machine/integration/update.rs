//! Update from target: the owner brings the target branch's new commits into a finished
//! task's own branch with a merge. The target is never changed and no published history
//! is rewritten. A conflict stays in the task's worktree with the same conflict view,
//! Finish merge and Abort merge as an integration's; a clean update moves the task's
//! tree, so the result the owner reviewed is no longer the current one.
use super::{conflicts, malformed, stale, task_facts_git, truth, verified_result, Git, Lock};
use crate::machine::{native::*, settings};
use serde_json::{json, Value};
use std::path::Path;

/// The task fields an update writes; no other field of the saved record is touched.
const OWNED: &[&str] = &[
    "requiresFreshResult",
    "conflict",
    "merge",
    "acceptance",
    "automaticReady",
];
fn save(state: &Path, project: &str, task: &str, record: &Value) -> Result<()> {
    settings::save_task(state, project, task, record, OWNED).map_err(|_| {
        refuse("integration_unknown", "Git updated the task branch but the task record could not be saved. Refresh the task and inspect its branch; do not repeat the update blindly.")
    })
}

pub(super) async fn run(
    root: &Path,
    project: &str,
    state: &Path,
    params: &Value,
    git: &mut Git,
) -> Result<Value> {
    let task = params["taskId"].as_str().ok_or_else(malformed)?;
    let saved = product(state, project)?;
    let record = saved["tasks"].get(task).cloned().unwrap_or(Value::Null);
    if !record.is_object() {
        return Err(refuse("invalid_task", "Unknown task."));
    }
    if !matches!(record["status"].as_str(), Some("review" | "ready_to_merge")) {
        return Err(refuse(
            "not_finished",
            "Only a finished task in review or Ready to merge can be updated from its target.",
        ));
    }
    if truth(&record["conflict"]) && record["conflict"] != "none" {
        return Err(refuse(
            "merge_conflict",
            "This task has a conflict in progress. Finish or abort it first.",
        ));
    }
    if record["checkout"]["kind"] != "worktree" {
        return Err(refuse("same_branch", "This task works in the project folder, which other work shares, so it has no branch of its own to update."));
    }
    // A worker still running, a paused or failed run, or a result that changed since
    // the owner looked: nothing is merged under it.
    let verified = verified_result(
        root,
        project,
        task,
        &params["digest"],
        &record,
        "This task's workers are still running or its run did not complete. Update it once it has finished.",
        false,
    )
    .await?;
    let path = crate::machine::retention::task_path(root, &record, git).await?;
    let source = git
        .out(&path, &["branch", "--show-current"])
        .await?
        .trim()
        .to_owned();
    if source.is_empty() {
        return Err(refuse(
            "detached_checkout",
            "The task's worktree is not on a branch (detached HEAD). Check out its branch first.",
        ));
    }
    let facts = task_facts_git::read(root, &record, task, git).await?;
    let target = facts["targetBranch"]
        .as_str()
        .ok_or_else(malformed)?
        .to_owned();
    if target.starts_with('-')
        || !git
            .success(root, &["check-ref-format", "--branch", &target])
            .await?
        || !git
            .success(
                root,
                &[
                    "show-ref",
                    "--verify",
                    "--quiet",
                    &format!("refs/heads/{target}"),
                ],
            )
            .await?
    {
        return Err(refuse(
            "unknown_branch",
            "The target branch must exist locally.",
        ));
    }
    if source == target {
        return Err(refuse(
            "same_branch",
            format!("This task works on {target} itself; there is nothing to bring in."),
        ));
    }
    if !git.operation(&path).await?.is_null() {
        return Err(refuse(
            "operation_in_progress",
            "A merge or rebase is stopped in the task's worktree. Continue or abort it first.",
        ));
    }
    let behind = git
        .out(
            &path,
            &["rev-list", "--count", &format!("{source}..{target}")],
        )
        .await?;
    if behind.trim() == "0" {
        return Err(refuse(
            "up_to_date",
            format!(
                "The task's branch already contains every commit of {target}. Nothing was changed."
            ),
        ));
    }
    // Abort restores the worktree exactly only when no tracked file had uncommitted
    // changes; Git itself refuses to overwrite an untracked one.
    if git.tracked_dirty(&path).await? {
        return Err(refuse("dirty_checkout", "The task's worktree has uncommitted changes. Commit them first (Commit… in the Git menu); Cortex does not stash, and a merge over uncommitted changes could not be aborted exactly."));
    }
    super::effects::private_index(&path, git).await?;
    // The durable intent is committed before Git, under a short machine lock; this
    // task's other owner actions (accepting, merging) are refused until it settles.
    let lock = Lock::new(state, "machine.lock")?;
    let current = product(state, project)?;
    let claim = settings::claim(state, project, task, super::IN_PROGRESS, &lock)?;
    if current["tasks"][task] != record {
        return Err(stale());
    }
    let intent = match crate::machine::ledger::Intent::begin_locked(
        state,
        "product.integrate",
        project,
        Some(root),
        params,
        &lock,
    )? {
        crate::machine::ledger::Begin::Run(intent) => intent,
        crate::machine::ledger::Begin::Replay(reply) => {
            return crate::machine::integration_squash::replay(reply)
        }
    };
    drop(lock);
    let outcome = merge(
        state, project, task, &path, &source, &target, record, &verified, git,
    )
    .await;
    let reply = envelope(outcome);
    intent.finish(state, &reply)?;
    drop(claim);
    crate::machine::integration_squash::replay(reply)
}

#[allow(clippy::too_many_arguments)]
async fn merge(
    state: &Path,
    project: &str,
    task: &str,
    path: &Path,
    source: &str,
    target: &str,
    mut record: Value,
    verified: &Value,
    git: &mut Git,
) -> Result<Value> {
    let head = git.out(path, &["rev-parse", "HEAD"]).await?;
    let fresh = json!({"resultId":verified["resultId"],"version":verified["version"]});
    let message = format!("Merge {target} into {source}");
    let merged = git
        .success(
            path,
            &[
                "merge",
                "--no-autostash",
                "--no-edit",
                "-m",
                &message,
                "--",
                target,
            ],
        )
        .await?;
    let stopped = git.operation(path).await?;
    if !merged && !stopped.is_null() {
        let mut merge = conflicts::build(path, source, target, &record, git).await?;
        // What Abort merge puts back: the tree is then exactly the reviewed one again.
        merge["kind"] = json!("update");
        merge["before"]["requiresFreshResult"] = record["requiresFreshResult"].clone();
        let n = merge["files"].as_array().map_or(0, Vec::len);
        record["requiresFreshResult"] = fresh;
        record["conflict"] = json!("conflict");
        record["merge"] = merge;
        record["acceptance"] = Value::Null;
        save(state, project, task, &record)?;
        return Err(refuse(
            "merge_conflict",
            format!("Updating {source} from {target} stopped at {n} conflicting file{} in the task's worktree. Nothing was stashed or reset, and {target} was not changed. Resolve the conflict, or abort the update.", if n == 1 { "" } else { "s" }),
        ));
    }
    if !merged {
        // Refused before it started (a hook, an untracked file in the way): unchanged.
        if git.out(path, &["rev-parse", "HEAD"]).await? == head {
            return Err(refuse(
                "git_failed",
                "Git refused to update the task branch; nothing was changed. Inspect the task's worktree in Git, for example an untracked file in the way, then retry.",
            ));
        }
        return Err(refuse("integration_unknown", "Git may have changed the task branch. Inspect it in Git before requesting another action; nothing was retried."));
    }
    // The tree moved: the result the owner reviewed is not the current one any more.
    record["requiresFreshResult"] = fresh;
    record["acceptance"] = Value::Null;
    record["automaticReady"] = json!(false);
    save(state, project, task, &record)?;
    Ok(json!({"done":true,"updated":true}))
}
