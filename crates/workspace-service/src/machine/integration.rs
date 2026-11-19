//! Integration admission and effects: squash merge, pull request and same-branch
//! commit. Admission refuses before any effect; the durable intent precedes Git and
//! nothing is ever delegated to another runtime.
mod conflicts;
mod effects;
mod update;
use super::{
    context,
    git::Git,
    native::*,
    settings::{self, Lock},
    task_facts_git,
    workers::truthy as truth,
};
/// Another owner action on a task whose integration is running.
pub(super) const IN_PROGRESS: &str =
    "This task is being merged or its pull request is being created. Wait for it to finish.";
/// Another Git-changing owner action of the same project (a merge, pull request,
/// same-branch commit or merge finish) while one runs.
pub(super) const REPO_BUSY: &str =
    "Another merge, pull request or commit in this project is still running. Try again when it finishes.";
use cortex_core::OpenMode;
use serde_json::{json, Value};
use std::path::Path;

/// Saved state or request JSON of a shape the current app never produces.
fn malformed() -> Refused {
    refuse(
        "invalid_request",
        "This integration request or the saved task state is malformed. Refresh the task and try again.",
    )
}
fn stale() -> Refused {
    refuse(
        "stale_result",
        "Review and accept the current result before finishing.",
    )
}
fn unavailable(message: &str) -> Refused {
    refuse("result_unavailable", message)
}

/// Automatic may skip Git only when the task-owned view is complete and empty.
/// In particular, `alreadyInTarget` alone is not enough: missing facts and
/// repair paths must remain owner-visible rather than being treated as a no-op.
fn task_already_landed(facts: &Value) -> bool {
    facts["alreadyInTarget"] == true
        && facts["pendingPaths"]
            .as_array()
            .is_some_and(|paths| paths.is_empty())
        && !facts["repairPaths"].is_array()
}
/// The task's current runtime result, bound to the `digest` the owner reviewed: only a
/// completed execution (no worker still running, nothing paused or failed) whose
/// evidence still matches. `incomplete` is what the owner reads otherwise.
async fn verified_result(
    root: &Path,
    project: &str,
    task: &str,
    digest: &Value,
    record: &Value,
    incomplete: &str,
    automatic: bool,
) -> Result<Value> {
    let Some(store) = context::open(root, OpenMode::LiveReadOnly)? else {
        return Err(unavailable(
            "Activate Cortex and review its runtime result first.",
        ));
    };
    // Exactly the result-state part of retained _facts: a quiet/failed/paused
    // execution is not a completed result. No synthesis/check/receipt is inferred.
    let completed = store
        .read(|db| {
            Ok(db.query_row(
                "SELECT EXISTS(
             SELECT 1 FROM workspace_executions e JOIN workspace_tasks t USING(task_id)
             JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
             JOIN tasks base ON base.id=r.task_id
             JOIN acp_host_sessions s ON s.session_id=e.acp_session_id
             JOIN task_owners o ON o.task_id=r.task_id
             WHERE t.project_id=? AND e.task_id=? AND e.execution_id=t.execution_id
             AND r.state='COMPLETED' AND r.revision>=0 AND (r.blocker IS NULL OR r.blocker='')
             AND base.project_root=? AND s.project_root=base.project_root
             AND s.task_id=r.task_id AND o.epoch=e.owner_epoch
             AND NOT EXISTS(SELECT 1 FROM workspace_starts w
                 WHERE w.execution_id=e.execution_id AND w.state='unknown'))",
                (project, task, root.to_string_lossy().as_ref()),
                |row| row.get::<_, bool>(0),
            )?)
        })
        .map_err(context::core)?;
    if !completed {
        return Err(unavailable(incomplete));
    }
    let execution = store
        .read(|db| {
            Ok(db.query_row(
                "SELECT execution_id FROM workspace_tasks WHERE project_id=? AND task_id=?",
                (project, task),
                |r| r.get::<_, String>(0),
            )?)
        })
        .map_err(context::core)?;
    let native = super::task_facts::native_observation(root).await;
    super::task_facts::integration_result(
        &store,
        root,
        project,
        &execution,
        digest,
        native.as_deref(),
        record,
        automatic,
    )
}
/// The recorded address of the pull request created for a task, for the desktop's own
/// opener: only an `https` address on the host of this repository's `origin` remote,
/// checked again now against the remote as it is configured today.
pub async fn pull_request_address(
    root: &Path,
    project: &str,
    state: &Path,
    params: &Value,
    git: &mut Git,
) -> Result<Value> {
    let task = params["pullRequest"].as_str().ok_or_else(malformed)?;
    let saved = super::native::product(state, project)?;
    let integration = &saved["tasks"][task]["integration"];
    if integration["method"] != "pr" {
        return Err(refuse(
            "no_pull_request",
            "No pull request was created for this task from this app.",
        ));
    }
    let Some(url) = integration["url"].as_str() else {
        return Err(refuse(
            "pull_request_unknown",
            "GitHub CLI did not report this pull request's address, so it was not recorded. Find it on GitHub.",
        ));
    };
    let remote = git
        .out(root, &["remote", "get-url", "origin"])
        .await
        .unwrap_or_default();
    if effects::pull_request_url(url, &remote).as_deref() != Some(url) {
        return Err(refuse(
            "pull_request_unsafe",
            "The recorded pull request address is not on this repository's remote host, so it was not opened.",
        ));
    }
    Ok(serde_json::json!({ "url": url }))
}
pub async fn run(
    root: &Path,
    project: &str,
    state: &Path,
    params: &Value,
    git: &mut Git,
) -> Result<Value> {
    integrate(root, project, state, params, git, false).await
}

pub(super) async fn automatic(
    root: &Path,
    project: &str,
    state: &Path,
    params: &Value,
    git: &mut Git,
) -> Result<Value> {
    integrate(root, project, state, params, git, true).await
}

fn automatic_member(saved: &Value, task: &str, expected_sprint: &Value, recheck: &Value) -> bool {
    let record = &saved["tasks"][task];
    record["sprintId"] == *expected_sprint
        && record["archive"].is_null()
        && record["purged"] != true
        && !matches!(
            record["conflict"].as_str(),
            Some("conflict" | "failed" | "resolving")
        )
        && saved["sprints"].as_array().into_iter().flatten().any(|s| {
            s["id"] == *expected_sprint
                && super::product::automatic_sprint(s)
                && (if recheck.is_null() {
                    s["queueState"] == "running"
                } else {
                    s == recheck
                        && s["queueState"] == "paused"
                        && matches!(
                            s["automaticQueue"]["kind"].as_str(),
                            Some("settlement_blocked" | "owner_integration")
                        )
                        && s["automaticQueue"]["taskId"] == task
                        && s["automaticQueue"]["code"].as_str().is_some_and(|code| {
                            !matches!(
                                code,
                                "outcome_unknown"
                                    | "integration_outcome_unknown"
                                    | "integration_failed"
                            )
                        })
                })
                && s["taskIds"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id == task))
        })
}

/// Automatic finish in a folder without Git. The accepted result already is the
/// project folder and there is no repository to commit it to, so — like a result
/// whose changes already landed — it reaches Done without any Git effect: the task
/// is closed as is with its acceptance and checks kept (never Not verified). Every
/// gate before this point (current automatic acceptance, completed and verified
/// runtime result, no fresh-result requirement, unchanged project folder) has
/// passed; the saved record and the queue hold are checked again under the machine
/// lock, so a concurrent owner action wins and nothing is written twice.
fn finish_in_place(
    state: &Path,
    project: &str,
    task: &str,
    record: &Value,
    sprint: &Value,
    recheck: &Value,
) -> Result<Value> {
    let _lock = Lock::new(state, "machine.lock")?;
    let mut product = super::native::product(state, project)?;
    if product["tasks"][task] != *record {
        return Err(stale());
    }
    if !automatic_member(&product, task, sprint, recheck) {
        return Ok(json!({"deferred":true}));
    }
    let saved = &mut product["tasks"][task];
    saved["status"] = json!("done");
    saved["doneAt"] = json!(now_ms());
    saved["closedAs"] = json!("as_is");
    saved["notVerifiedByCortex"] = json!(false);
    settings::write_json(&product_path(state, project)?, &product)?;
    Ok(json!({"done":true,"inPlace":true}))
}

async fn integrate(
    root: &Path,
    project: &str,
    state: &Path,
    params: &Value,
    git: &mut Git,
    automatic: bool,
) -> Result<Value> {
    if !params.is_object() {
        return Err(malformed());
    }
    if !automatic && params["confirmed"] != true {
        return Err(refuse(
            "confirmation_required",
            "Confirm this Git action first.",
        ));
    }
    // One Git-changing owner action per repository, from admission to the recorded
    // outcome: a second one is refused at once, never interleaved or queued.
    let _repo = Lock::long(
        state,
        &super::git::repo_lock(root),
        "integration_in_progress",
        REPO_BUSY,
    )?;
    // Bringing the target into the task branch is its own admission, under the same
    // repository lock; it never touches the target.
    if !automatic && params["method"] == "update" {
        return update::run(root, project, state, params, git).await;
    }
    // Integrate reads the saved overlay, not the enriched product projection. The
    // checks below run without the machine lock; the record they read is checked
    // again under it before the durable intent.
    let saved = super::native::product(state, project)?;
    let tasks = saved["tasks"].as_object().ok_or_else(malformed)?;
    let record = match params["taskId"].as_str() {
        Some(task) => tasks.get(task).unwrap_or(&Value::Null),
        None if params["taskId"].is_null() => &Value::Null,
        None => return Err(malformed()),
    };
    let scheduler_sprint = &params["sprintId"];
    let recheck = &params["expectedHold"];
    let automatic_params;
    let params = if automatic {
        let task = params["taskId"].as_str().ok_or_else(malformed)?;
        if !scheduler_sprint.is_string()
            || !automatic_member(&saved, task, scheduler_sprint, recheck)
        {
            return Ok(json!({"deferred":true}));
        }
        if record["checkout"]["kind"] != "main" {
            return Err(refuse(
                "native_worktree_unavailable",
                "Automatic requires the project folder checkout.",
            ));
        }
        let accepted = &record["acceptance"];
        // A folder without Git has no branch: its default is never asked of Git.
        let effective_target = match record["targetBranch"].as_str().filter(|s| !s.is_empty()) {
            Some(target) => target.to_owned(),
            None if git.is_repo(root).await? => git.default_branch(root).await?,
            None => String::new(),
        };
        let finish_consent_matches = accepted["finishConsent"].is_object()
            && accepted["finishConsent"]["sprintId"] == *scheduler_sprint
            && accepted["finishConsent"]["targetBranch"] == effective_target
            && record["checkout"]["kind"] == "main";
        if record["status"] == "ready_to_merge"
            && accepted.is_object()
            && accepted["automatic"] != true
            && !finish_consent_matches
            && record["acceptedRevision"].is_string()
        {
            return Ok(json!({"ownerIntegration":true,"code":"owner_accepted",
                "reason":"You accepted this result. Open the task to confirm how to finish it."}));
        }
        if (accepted["automatic"] != true && !finish_consent_matches)
            || !record["acceptedRevision"].is_string()
        {
            return Err(refuse(
                "automatic_acceptance_required",
                "There is no current automatic acceptance of verified work.",
            ));
        }
        let identity = super::files::revision(
            json!([task, accepted["version"], accepted["digest"]])
                .to_string()
                .as_bytes(),
        );
        automatic_params = json!({"taskId":task,"method":"commit","digest":accepted["digest"],
            "message":"Finish verified Workspace task","requestId":format!("automatic-finish-{identity}")});
        &automatic_params
    } else {
        params
    };
    if !truth(record) {
        return Err(stale());
    }
    if !record.is_object() {
        return Err(malformed());
    }
    if record["status"] != "ready_to_merge" || !truth(&record["acceptance"]) {
        return Err(stale());
    }
    if !record["acceptance"].is_object()
        || !(params["digest"].is_null() || params["digest"].is_string())
        || !(record["acceptance"]["digest"].is_null() || record["acceptance"]["digest"].is_string())
    {
        return Err(malformed());
    }
    if record["acceptance"]["digest"] != params["digest"] {
        return Err(stale());
    }
    let task = params["taskId"].as_str().ok_or_else(malformed)?;
    // Unsupported legacy shapes retain their interpreter before inspection.
    let mut inspected = record.clone();
    if truth(&params["targetBranch"]) {
        inspected["targetBranch"] = params["targetBranch"].clone();
    }
    if truth(&inspected["targetBranch"]) && !inspected["targetBranch"].is_string()
        || truth(&record["baseRevision"]) && !record["baseRevision"].is_string()
        || truth(&record["checkout"]) && !record["checkout"].is_object()
        || record["checkout"].is_object() && !record["checkout"]["kind"].is_string()
    {
        return Err(malformed());
    }
    let facts = task_facts_git::read_for_finish(root, &inspected, task, git).await?;
    if !automatic && facts["alreadyInTarget"] == true {
        return Err(refuse(
            "already_in_target",
            format!(
                "Already in {}. Confirm Close task instead; no Git operation is needed.",
                facts["targetBranch"].as_str().ok_or_else(malformed)?
            ),
        ));
    }
    let verified = verified_result(
        root,
        project,
        task,
        &params["digest"],
        record,
        "Only a completed runtime result can be accepted.",
        automatic,
    )
    .await?;
    let path = super::retention::task_path(root, record, git).await?;
    if record["requiresFreshResult"]
        == serde_json::json!({"resultId":verified["resultId"],"version":verified["version"]})
    {
        return Err(refuse("review_required", "The source changed after this result. Run fresh checks and review the new result before integration."));
    }
    if truth(&record["acceptedRevision"]) {
        let current = super::legacy_source::capture(&path).map_err(context::core)?;
        if current.as_deref() != record["acceptedRevision"].as_str() {
            return Err(refuse(
                "workspace_drift",
                "Files changed after acceptance. Rerun checks and accept the new result.",
            ));
        }
    }
    if !git.is_repo(&path).await? {
        if automatic {
            return finish_in_place(state, project, task, record, scheduler_sprint, recheck);
        }
        return Err(refuse("git_required", "The accepted changes are already in the project folder. Close the task as-is, or initialize Git before using merge or PR operations."));
    }
    let source = git
        .out(&path, &["branch", "--show-current"])
        .await?
        .trim()
        .to_owned();
    let target = facts["targetBranch"]
        .as_str()
        .ok_or_else(malformed)?
        .to_owned();
    let target = target.as_str();
    if target.starts_with('-')
        || !git
            .success(root, &["check-ref-format", "--branch", target])
            .await?
    {
        return Err(refuse("invalid_branch", "Choose a valid target branch."));
    }
    if !git
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
    if let Some(base) = record["baseRevision"].as_str().filter(|s| !s.is_empty()) {
        if !git
            .success(&path, &["merge-base", "--is-ancestor", base, target])
            .await?
        {
            return Err(refuse("incompatible_target", "This target predates the task base. Choose a descendant branch, or integrate this patch manually."));
        }
    }
    let message = params["message"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| refuse("empty_message", "Write a commit message first."))?;
    if source.is_empty() {
        return Err(refuse(
            "detached_checkout",
            "A task must have a branch before merging.",
        ));
    }
    if !git
        .out(&path, &["diff", "--name-only", "--diff-filter=U"])
        .await?
        .trim()
        .is_empty()
    {
        return Err(refuse(
            "merge_conflict",
            "Resolve the task's conflict first.",
        ));
    }
    if automatic && source != target {
        return Ok(json!({"ownerIntegration":true,"reason":format!(
            "Finishing this task would integrate {source} into {target}. Confirm the merge or pull request."
        )}));
    }
    let method = params["method"].as_str().unwrap_or("");
    // Containment also recognizes historical commits. For Automatic, an empty
    // task-owned diff is a certified no-op even when the checkout contains
    // unrelated owner work. The facts are only produced after the runtime has
    // matched every recorded task path (or proved that the answer requested no
    // changes), so this does not turn unknown work into a successful finish.
    let already_landed = automatic && source == target && task_already_landed(&facts);
    if source == target {
        if method != "commit" {
            return Err(refuse("same_branch", format!("This task is already on {target}. Use Commit to this branch, or confirm Close task; there is no branch integration.")));
        }
        if automatic && facts["repairPaths"].is_array() {
            return Ok(
                json!({"done": false, "repairPaths": facts["repairPaths"], "reason": "The recorded task files have changed. Review their current contents and publish a fresh result."}),
            );
        }
        if facts["commitAvailable"] != true && !already_landed {
            return Err(refuse(
                "task_content_unknown",
                facts["reason"]
                    .as_str()
                    .unwrap_or("Refresh the task before committing."),
            ));
        }
    } else if method != "squash" && method != "pr" {
        return Err(refuse(
            "unsupported_action",
            "Choose squash merge or pull request.",
        ));
    }
    if method == "pr" && !super::settings::available("gh") {
        return Err(refuse(
            "gh_unavailable",
            "Install and sign in to GitHub CLI (gh) to create a pull request.",
        ));
    }
    let same = |a: &Path, b: &Path| {
        a == b
            || matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(a), Ok(b)) if a == b)
    };
    let target_tree = git
        .trees(root)
        .await?
        .iter()
        .find(|t| t["branch"] == target)
        .and_then(|t| t["path"].as_str())
        .map(std::path::PathBuf::from);
    if let Some(tree) = target_tree.as_deref().filter(|t| !same(t, &path)) {
        if !git.clean(tree).await? {
            return Err(refuse(
                "dirty_target",
                "The target checkout has uncommitted work. Commit or move it yourself first.",
            ));
        }
    }
    // Read-only recheck before the durable intent: helper data in the index, and
    // for a shared branch the exact task-owned paths about to be committed.
    if !already_landed {
        effects::private_index(&path, git).await?;
    }
    let mut pending_paths = Vec::new();
    if method == "commit" {
        let current = task_facts_git::read_for_finish(root, &inspected, task, git).await?;
        let unchanged = if already_landed {
            task_already_landed(&current)
        } else {
            current["commitAvailable"] == true && current["pendingPaths"] == facts["pendingPaths"]
        };
        if !unchanged {
            return Err(refuse(
                "task_content_unknown",
                "Task files changed. Refresh before committing.",
            ));
        }
        pending_paths = facts["pendingPaths"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|p| p.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
    }
    // The durable intent is committed before Git. Drop/cancellation leaves it
    // dispatched; neither a helper nor a retry may reinterpret that as failure.
    // The machine lock is held only for this admission. Git, hooks and GitHub run
    // after it is released; this task's other owner actions are refused meanwhile.
    let lock = Lock::new(state, "machine.lock")?;
    let mut product = super::native::product(state, project)?;
    let claim = settings::claim(state, project, task, IN_PROGRESS, &lock)?;
    if product["tasks"][task] != *record {
        return Err(stale());
    }
    if automatic && !automatic_member(&product, task, scheduler_sprint, recheck) {
        return Ok(json!({"deferred":true}));
    }
    let intent = match super::ledger::Intent::begin_locked(
        state,
        if automatic {
            "automatic.finish"
        } else {
            "product.integrate"
        },
        project,
        Some(root),
        params,
        &lock,
    )? {
        super::ledger::Begin::Run(intent) => intent,
        super::ledger::Begin::Replay(reply) => return super::integration_squash::replay(reply),
    };
    drop(lock);
    let plan = effects::Plan {
        root,
        project,
        state,
        task,
        path: &path,
        source: &source,
        target,
        method,
        message,
        params,
        verified: &verified,
        pending_paths,
        target_tree,
        automatic,
        already_landed,
    };
    let outcome = effects::apply(&plan, &mut product, git).await;
    // An overlay failure after Git is ambiguous, not safe to rerun.
    let reply = super::native::envelope(outcome);
    intent.finish(state, &reply)?;
    drop(claim);
    super::integration_squash::replay(reply)
}

#[cfg(test)]
mod tests;
