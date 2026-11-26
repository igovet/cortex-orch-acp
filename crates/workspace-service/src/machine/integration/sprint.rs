//! The one merge of a shared sprint worktree's branch into the sprint's target branch,
//! once every member has finished onto the sprint branch, and the cleanup of that
//! worktree afterwards. Members commit only their own files onto the sprint branch
//! through the ordinary same-branch commit; nothing here commits task files.
use super::{
    super::{
        context, git::Git, integration_squash::{self, IndexSnapshot}, native::*,
        settings::{self, Lock},
    },
    REPO_BUSY,
};
use serde_json::{json, Value};

/// `automatic`: the queue merges by itself, but only into the branch the project folder
/// has checked out; any other target stays an owner decision (the reply says so).
pub(in crate::machine) async fn run(
    root: &std::path::Path,
    project: &str,
    state: &std::path::Path,
    params: &Value,
    git: &mut Git,
    automatic: bool,
) -> Result<Value> {
    if !automatic && params["confirmed"] != true {
        return Err(refuse("confirmation_required", "Confirm this Git action first."));
    }
    let sprint_id = params["sprintId"].as_str().ok_or_else(|| refuse("invalid_sprint", "Unknown sprint."))?;
    let _repo = Lock::long(state, &super::super::git::repo_lock(root), "integration_in_progress", REPO_BUSY)?;
    let saved = super::super::native::product(state, project)?;
    let sprint = saved["sprints"]
        .as_array()
        .and_then(|all| all.iter().find(|s| s["id"] == sprint_id))
        .ok_or_else(|| refuse("unknown_sprint", "Unknown sprint."))?;
    if sprint["checkout"]["kind"] != "worktree" || sprint["checkout"]["shared"] != true {
        return Err(refuse("not_shared_worktree", "This sprint does not share one worktree."));
    }
    if sprint["merge"]["state"] == "merged" {
        return Ok(json!({"done":true,"merged":true,"alreadyMerged":true}));
    }
    if !sprint["archivedAt"].is_null() {
        return Err(refuse("sprint_closed", "This sprint is closed."));
    }
    let members: Vec<&str> = sprint["taskIds"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let live: Vec<&str> = members
        .iter()
        .copied()
        .filter(|t| saved["tasks"][t].is_object() && saved["tasks"][t]["archive"].is_null())
        .collect();
    if live.is_empty() || live.iter().any(|t| saved["tasks"][t]["status"] != "done") {
        return Err(refuse("sprint_not_finished", "Finish every task of the sprint before merging its branch."));
    }
    let mut checkout = None;
    for task in &members {
        if let Some(w) = context::owned_checkout(root, project, task)? {
            checkout = Some(w);
            break;
        }
    }
    let w = checkout.ok_or_else(|| refuse("worktree_unavailable", "The sprint worktree is not available."))?;
    let target = match sprint["targetBranch"].as_str().filter(|s| !s.is_empty()) {
        Some(t) => t.to_owned(),
        None => git.default_branch(root).await?,
    };
    if target.starts_with('-') || !git.success(root, &["check-ref-format", "--branch", &target]).await? {
        return Err(refuse("invalid_branch", "Choose a valid target branch."));
    }
    if !git.success(root, &["show-ref", "--verify", "--quiet", &format!("refs/heads/{target}")]).await? {
        return Err(refuse("unknown_branch", "The target branch must exist locally."));
    }
    if target == w.branch {
        return Err(refuse("same_branch", "The sprint branch is the target branch."));
    }
    if w.path.exists() {
        if !git.clean(&w.path).await? || !git.operation(&w.path).await?.is_null() {
            return Err(refuse("dirty_checkout", "The sprint worktree has uncommitted changes or a Git operation in progress. Finish it first."));
        }
    }
    let tree = git
        .trees(root)
        .await?
        .iter()
        .find(|t| t["branch"] == target)
        .and_then(|t| t["path"].as_str())
        .map(std::path::PathBuf::from);
    let Some(tree) = tree else {
        return Err(refuse("target_not_checked_out", "Check out the target branch in a folder first; the sprint branch is merged there."));
    };
    if automatic && tree != root {
        return Ok(json!({"ownerIntegration":true,"reason":format!(
            "Merging the sprint branch into {target} needs your confirmation: it is not the branch open in the project folder.")}));
    }
    context::require_idle_checkout(root, project, &tree)?;
    if !git.clean(&tree).await? || !git.operation(&tree).await?.is_null() {
        return Err(refuse("dirty_target", "The target checkout has uncommitted work or a Git operation in progress. Commit or move it first."));
    }
    let title = sprint["title"].as_str().filter(|t| !t.trim().is_empty()).unwrap_or("sprint");
    let message = params["message"].as_str().filter(|m| !m.trim().is_empty()).map(str::to_owned)
        .unwrap_or_else(|| format!("Merge sprint: {title}"));
    let commit = squash_branch(root, &tree, &target, &w.branch, &message, git).await?;
    {
        let lock = Lock::new(state, "machine.lock")?;
        let mut product = super::super::native::product(state, project)?;
        let recorded = product["sprints"]
            .as_array_mut()
            .and_then(|all| all.iter_mut().find(|s| s["id"] == sprint_id))
            .ok_or_else(|| refuse("unknown_sprint", "Unknown sprint."))?;
        recorded["merge"] = json!({"state":"merged","target":target,"branch":w.branch,"commit":commit,"at":now_ms()});
        settings::write_json(&product_path(state, project)?, &product)?;
        drop(lock);
    }
    let cleanup = release(root, project, &members);
    Ok(json!({"done":true,"merged":true,"commit":commit,"target":target,"worktreeCleanup":cleanup}))
}

/// Releases every member in turn; the last one removes the clean worktree. A retained
/// worktree never undoes the recorded merge.
fn release(root: &std::path::Path, project: &str, members: &[&str]) -> Value {
    use cortex_core::{workspace::worktrees::{self, Cleanup}, OpenMode};
    let mut status = json!("retained");
    for task in members {
        let outcome = (|| {
            let store = context::open(root, OpenMode::RuntimeValidate)?
                .ok_or_else(|| context::core(cortex_core::CoreError::Refused("worktree_not_owned")))?;
            store.write(|db| worktrees::queue_merge_cleanup_tx(db, project, task)).map_err(context::core)?;
            worktrees::finish_cleanup(&store, project, task, Cleanup::Close).map_err(context::core)
        })();
        if let Ok(Some(v)) = outcome {
            status = v["status"].clone();
        }
    }
    status
}

/// One squash commit of everything the sprint branch holds beyond the target, in the
/// target's checkout. Nothing is committed when the branch adds nothing; the commit id
/// (or null) is returned.
async fn squash_branch(
    root: &std::path::Path,
    tree: &std::path::Path,
    target: &str,
    branch: &str,
    message: &str,
    git: &mut Git,
) -> Result<Value> {
    let ahead = git
        .out(root, &["rev-list", "--count", &format!("refs/heads/{target}..refs/heads/{branch}")])
        .await?;
    if ahead.trim() == "0" {
        return Ok(Value::Null);
    }
    let snapshot = IndexSnapshot::capture(tree, git).await?;
    integration_squash::squash(tree, branch, message, snapshot, git).await?;
    Ok(json!(git.out(tree, &["rev-parse", "HEAD"]).await?.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &std::path::Path, args: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }

    /// Two members commit their own files onto the sprint branch (the same-branch commit);
    /// the sprint then lands on the target as one commit, and a repeat adds nothing.
    #[tokio::test]
    async fn the_sprint_branch_lands_on_the_target_as_one_commit_and_a_repeat_adds_none() {
        let base = std::env::temp_dir().join(format!("cw-sprint-merge-{}", settings::unique()));
        let root = base.join("repo");
        let tree = base.join("sprint-wt");
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        for (k, v) in [("user.name", "F"), ("user.email", "f@example.invalid"), ("commit.gpgsign", "false"), ("core.hooksPath", "/dev/null")] {
            git(&root, &["config", k, v]);
        }
        std::fs::write(root.join("base.txt"), "base\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "Base"]);
        git(&root, &["worktree", "add", "-q", "-b", "cw/sprint-abc", tree.to_str().unwrap()]);
        for (file, message) in [("first.txt", "First member"), ("second.txt", "Second member")] {
            std::fs::write(tree.join(file), format!("{file}\n")).unwrap();
            git(&tree, &["add", file]);
            git(&tree, &["commit", "-q", "-m", message]);
        }
        let mut g = Git::new();
        let commit = squash_branch(&root, &root, "main", "cw/sprint-abc", "Merge sprint: demo", &mut g).await.unwrap();
        assert_eq!(commit, json!(git(&root, &["rev-parse", "HEAD"])));
        assert_eq!(git(&root, &["rev-list", "--count", "main"]), "2", "base plus one sprint commit");
        assert_eq!(git(&root, &["log", "-1", "--format=%s"]), "Merge sprint: demo");
        assert_eq!(git(&root, &["show", "--name-only", "--format=", "HEAD"]).lines().collect::<Vec<_>>(), ["first.txt", "second.txt"]);
        assert_eq!(git(&root, &["status", "--porcelain"]), "");
        // The branch still holds its own two commits; landing it again changes nothing.
        let again = squash_branch(&root, &root, "main", "cw/sprint-abc", "Merge sprint: demo", &mut g).await.unwrap();
        let _ = again;
        assert_eq!(git(&root, &["rev-list", "--count", "main"]), "2");
        assert_eq!(git(&root, &["status", "--porcelain"]), "");
        let _ = std::fs::remove_dir_all(base);
    }
}
