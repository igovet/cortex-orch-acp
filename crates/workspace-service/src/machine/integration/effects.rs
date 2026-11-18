//! Integration effects after admission and the durable intent (task_git.py `integrate`).
//! No reset, stash, force removal or implicit push. Every failure after the first Git
//! effect is final: nothing is delegated or retried.
use super::conflicts;
use crate::machine::{
    git::Git,
    integration_squash::{self, IndexSnapshot},
    native::*,
    settings,
};
use serde_json::{json, Value};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

pub(super) struct Plan<'a> {
    pub root: &'a Path,
    pub project: &'a str,
    pub state: &'a Path,
    pub task: &'a str,
    pub path: &'a Path,
    pub source: &'a str,
    pub target: &'a str,
    pub method: &'a str,
    pub message: &'a str,
    pub params: &'a Value,
    pub verified: &'a Value,
    pub pending_paths: Vec<String>,
    pub target_tree: Option<PathBuf>,
}

fn helper(name: &str) -> bool {
    RESERVED.contains(&name.split('/').next().unwrap_or(""))
}
/// Workspace helper data staged by an older app must never be committed.
pub(super) async fn private_index(path: &Path, git: &mut Git) -> Result<()> {
    // NUL-separated: the default quoted form hides non-ASCII names from `helper`.
    let staged = git
        .out(path, &["diff", "--cached", "--name-only", "-z"])
        .await?;
    if staged.split('\0').any(helper) {
        return Err(refuse(
            "private_index",
            "Unstage Workspace helper data before committing.",
        ));
    }
    Ok(())
}
/// Literal paths also handle pathspec metacharacters; helper data is excluded.
async fn stage_work(path: &Path, git: &mut Git) -> Result<()> {
    private_index(path, git).await?;
    let mut names = std::collections::BTreeSet::new();
    for args in [
        &["diff", "--name-only", "-z"][..],
        &["ls-files", "--others", "--exclude-standard", "-z"][..],
    ] {
        for name in git.out(path, args).await?.split('\0') {
            if !name.is_empty() && !helper(name) {
                names.insert(name.to_owned());
            }
        }
    }
    let names: Vec<_> = names.into_iter().collect();
    for chunk in names.chunks(256) {
        let literal: Vec<String> = chunk.iter().map(|n| format!(":(literal){n}")).collect();
        let mut argv = vec!["add", "-A", "--"];
        argv.extend(literal.iter().map(String::as_str));
        git.out(path, &argv).await?;
    }
    Ok(())
}
fn unknown_record() -> Refused {
    refuse("integration_unknown", "Git finished but the integration record could not be saved. Inspect the target history and refresh; do not repeat Merge blindly.")
}
/// The task fields integration writes; no other field of the saved record is touched.
const OWNED: &[&str] = &[
    "status",
    "work",
    "doneAt",
    "integration",
    "requiresFreshResult",
    "conflict",
    "merge",
    "acceptance",
    "automaticReady",
    "prAttempt",
];
/// Only this task's record changes; the rest of the saved product is re-read under a
/// short machine lock, so owner edits made while Git ran are kept.
fn save(plan: &Plan<'_>, product: &Value) -> Result<()> {
    settings::save_task(
        plan.state,
        plan.project,
        plan.task,
        &product["tasks"][plan.task],
        OWNED,
    )
    .map_err(|_| unknown_record())
}
fn failed_commit(method: &str) -> Refused {
    refuse(
        "integration_failed",
        format!("Git could not save the task changes. The previous staging selection was restored and your files are unchanged. Check your Git identity and commit hooks in this checkout, fix the reported local problem in Git, then retry {}", if method == "commit" { "Commit." } else { "Merge." }),
    )
}
/// Commit the task's own changes in the source checkout, restoring the exact prior
/// staging selection if Git refuses. Timeouts stay unresolved with the intent.
async fn commit_source(plan: &Plan<'_>, git: &mut Git) -> Result<()> {
    let path = plan.path;
    let snapshot = IndexSnapshot::capture(path, git).await?;
    let work: Result<()> = async {
        if plan.method == "commit" {
            // --only does not sweep already-staged unrelated paths into this commit.
            private_index(path, git).await?;
            let literal: Vec<String> = plan
                .pending_paths
                .iter()
                .map(|p| format!(":(literal){p}"))
                .collect();
            let mut add = vec!["add", "-A", "--"];
            add.extend(literal.iter().map(String::as_str));
            git.out(path, &add).await?;
            let mut commit = vec!["commit", "--only", "-m", plan.message, "--"];
            commit.extend(literal.iter().map(String::as_str));
            git.out(path, &commit).await?;
        } else {
            stage_work(path, git).await?;
            if !git.success(path, &["diff", "--cached", "--quiet"]).await? {
                git.out(path, &["commit", "-m", plan.message]).await?;
            }
        }
        Ok(())
    }
    .await;
    match work {
        Ok(()) => Ok(()),
        Err(e) if e.code != "git_failed" => Err(e),
        Err(_) => {
            integration_squash::unchanged_head(path, &snapshot, git).await?;
            snapshot.restore()?;
            Err(failed_commit(plan.method))
        }
    }
}
fn done(record: &mut Value, method: &str, plan: &Plan<'_>, source_head: &str) {
    record["status"] = json!("done");
    record["work"] = json!("merged");
    record["doneAt"] = json!(now_ms());
    record["integration"] = json!({"method":method,"targetBranch":plan.target,
        "digest":plan.params["digest"],"message":plan.message,"sourceHead":source_head});
}
fn fresh(plan: &Plan<'_>) -> Value {
    json!({"resultId":plan.verified["resultId"],"version":plan.verified["version"]})
}
pub(super) async fn apply(plan: &Plan<'_>, product: &mut Value, git: &mut Git) -> Result<Value> {
    commit_source(plan, git).await?;
    let path = plan.path;
    let source_head = git
        .out(path, &["rev-parse", "HEAD"])
        .await?
        .trim()
        .to_owned();
    if plan.method == "commit" {
        done(
            &mut product["tasks"][plan.task],
            "commit",
            plan,
            &source_head,
        );
        save(plan, product)?;
        return Ok(json!({"done":true}));
    }
    if plan.source != plan.target
        && !git
            .success(
                path,
                &["merge-base", "--is-ancestor", plan.target, plan.source],
            )
            .await?
    {
        return update_from_target(plan, product, git).await;
    }
    if plan.method == "pr" {
        return pull_request(plan, product, git).await;
    }
    squash(plan, product, &source_head, git).await
}
/// The target moved on: bring it into the task branch first. The target is untouched.
async fn update_from_target(plan: &Plan<'_>, product: &mut Value, git: &mut Git) -> Result<Value> {
    let path = plan.path;
    let merged = git
        .success(
            path,
            &[
                "merge",
                "-m",
                "Update task branch from target",
                "--",
                plan.target,
            ],
        )
        .await?;
    if !merged {
        if git
            .out(path, &["diff", "--name-only", "--diff-filter=U"])
            .await?
            .trim()
            .is_empty()
        {
            return Err(refuse(
                "git_failed",
                "Git could not update the task branch. Inspect it before retrying.",
            ));
        }
        let record = product["tasks"][plan.task].clone();
        let merge = conflicts::build(path, plan.source, plan.target, &record, git).await?;
        let record = &mut product["tasks"][plan.task];
        record["requiresFreshResult"] = fresh(plan);
        record["conflict"] = json!("conflict");
        record["merge"] = merge;
        record["acceptance"] = Value::Null;
        save(plan, product)?;
        return Err(refuse(
            "merge_conflict",
            "The conflict is in the task worktree. The target was not changed.",
        ));
    }
    let record = &mut product["tasks"][plan.task];
    record["requiresFreshResult"] = fresh(plan);
    record["acceptance"] = Value::Null;
    record["automaticReady"] = json!(false);
    save(plan, product)?;
    Err(refuse(
        "review_required",
        "The task branch was updated from the target. Rerun checks and review it before merging.",
    ))
}
async fn pull_request(plan: &Plan<'_>, product: &mut Value, git: &mut Git) -> Result<Value> {
    let path = plan.path;
    let record = &product["tasks"][plan.task];
    if super::truth(&record["prAttempt"]) || record["integration"]["method"] == "pr" {
        return Err(refuse(
            "unknown_outcome",
            "A pull request was already attempted. Check GitHub before creating another.",
        ));
    }
    // No push is hidden here. gh requires the branch to have been pushed explicitly.
    if !git
        .success(path, &["rev-parse", "--verify", "@{u}"])
        .await?
        || !git
            .out(path, &["rev-list", "@{u}..HEAD"])
            .await?
            .trim()
            .is_empty()
    {
        return Err(refuse(
            "push_required",
            "Push this task branch from Git first, then create the pull request.",
        ));
    }
    product["tasks"][plan.task]["prAttempt"] =
        json!({"state":"unknown","target":plan.target,"head":plan.source});
    save(plan, product)?;
    let title = plan.message.lines().next().unwrap_or("");
    let body = plan.params["body"].as_str().unwrap_or("");
    let mut command = tokio::process::Command::new("gh");
    command
        .args([
            "pr",
            "create",
            "--base",
            plan.target,
            "--head",
            plan.source,
            "--title",
            title,
            "--body",
            body,
        ])
        .current_dir(path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if super::truth(&plan.params["draft"]) {
        command.arg("--draft");
    }
    let outcome = match command.spawn() {
        Err(_) => return Err(refuse(
            "gh_failed",
            "GitHub CLI could not create the pull request. Check gh login and repository access.",
        )),
        Ok(mut child) => tokio::time::timeout(Duration::from_secs(25), child.wait()).await,
    };
    match outcome {
        Err(_) => {
            return Err(refuse(
                "unknown_outcome",
                "GitHub did not answer. Check your pull requests before retrying.",
            ))
        }
        Ok(Ok(status)) if status.success() => {}
        Ok(_) => return Err(refuse(
            "gh_failed",
            "GitHub CLI could not create the pull request. Check gh login and repository access.",
        )),
    }
    product["tasks"][plan.task]["integration"] = json!({"method":"pr","targetBranch":plan.target,
        "digest":plan.params["digest"],"title":plan.message});
    save(plan, product)?;
    Ok(json!({"done":true}))
}
async fn squash(
    plan: &Plan<'_>,
    product: &mut Value,
    source_head: &str,
    git: &mut Git,
) -> Result<Value> {
    let mut temporary: Option<PathBuf> = None;
    let tree = match &plan.target_tree {
        Some(tree) => tree.clone(),
        None => {
            let folder = plan
                .state
                .join("merge-targets")
                .join(plan.project)
                .join(plan.task);
            if let Some(parent) = folder.parent() {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)?;
            }
            // An empty leftover is harmless; anything else is not ours to remove.
            if folder.exists() && std::fs::remove_dir(&folder).is_err() {
                return Err(refuse(
                    "integration_failed",
                    "A temporary target checkout from an earlier attempt is still present. Inspect it in Git, remove it yourself, then retry.",
                ));
            }
            let text = folder.to_string_lossy().into_owned();
            git.out(plan.root, &["worktree", "add", &text, plan.target])
                .await?;
            temporary = Some(folder.clone());
            folder
        }
    };
    let snapshot = IndexSnapshot::capture(&tree, git).await?;
    let result = integration_squash::squash(&tree, plan.source, plan.message, snapshot, git).await;
    // A clean temporary checkout is removed without force; a dirty or uncertain one stays.
    if let Err(e) = result {
        if e.code == "integration_failed" || e.code == "integration_conflict" {
            remove_temp(plan, &temporary, git).await;
        }
        return Err(e);
    }
    done(
        &mut product["tasks"][plan.task],
        "squash",
        plan,
        source_head,
    );
    save(plan, product)?;
    remove_temp(plan, &temporary, git).await;
    // The merge is recorded before optional cleanup: cleanup failure cannot undo it.
    if plan.params["deleteWorktree"] == true
        && product["tasks"][plan.task]["checkout"]["kind"] == "worktree"
    {
        let mut cleanup = "retained";
        // An unreadable status keeps the worktree; it never turns the recorded merge into a failure.
        let clean = git.clean(plan.path).await.unwrap_or(false);
        if plan.source != plan.target && clean {
            let text = plan.path.to_string_lossy().into_owned();
            let _ = git
                .out(plan.root, &["worktree", "remove", "--", &text])
                .await;
            if !plan.path.exists() {
                cleanup = "removed";
            }
        }
        product["tasks"][plan.task]["integration"]["worktreeCleanup"] = json!(cleanup);
        // The merge is already saved as done; an unsaved cleanup note leaves it unreported.
        let _ = save(plan, product);
    }
    Ok(json!({"done":true}))
}
async fn remove_temp(plan: &Plan<'_>, temporary: &Option<PathBuf>, git: &mut Git) {
    if let Some(folder) = temporary {
        let text = folder.to_string_lossy().into_owned();
        let _ = git
            .out(plan.root, &["worktree", "remove", "--", &text])
            .await;
    }
}
