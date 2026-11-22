//! Local squash index recovery. No fallback is possible after dispatch.
use super::{git::Git, native::*};
use serde_json::Value;
use std::{
    fs,
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

pub(super) fn replay(reply: Value) -> Result<Value> {
    if reply["ok"] == true {
        return Ok(reply["result"].clone());
    }
    // Only fixed internal codes/messages are read from the private intent.
    let code = match reply["code"].as_str() {
        Some("payload_conflict") => "payload_conflict",
        Some("outcome_unknown") => "outcome_unknown",
        Some("integration_failed") => "integration_failed",
        Some("integration_conflict") => "integration_conflict",
        Some("integration_unknown") => "integration_unknown",
        Some("index_restore_failed") => "index_restore_failed",
        Some("git_timeout") => "git_timeout",
        Some("git_failed") => "git_failed",
        Some("merge_conflict") => "merge_conflict",
        Some("review_required") => "review_required",
        Some("private_index") => "private_index",
        Some("push_required") => "push_required",
        Some("pull_request_unsafe") => "pull_request_unsafe",
        Some("unknown_outcome") => "unknown_outcome",
        Some("gh_failed") => "gh_failed",
        Some("invalid_request") => "invalid_request",
        _ => "machine_tool_failed",
    };
    Err(refuse(
        code,
        reply["message"]
            .as_str()
            .unwrap_or("Inspect Git before retrying integration."),
    ))
}
pub(super) struct IndexSnapshot {
    path: PathBuf,
    bytes: Option<Vec<u8>>,
    head: String,
}
impl IndexSnapshot {
    pub(super) async fn capture(root: &Path, git: &mut Git) -> Result<Self> {
        let path = PathBuf::from(
            git.out(
                root,
                &["rev-parse", "--path-format=absolute", "--git-path", "index"],
            )
            .await?
            .trim(),
        );
        let bytes = match fs::symlink_metadata(&path) {
            Ok(m) if m.is_file() && m.nlink() == 1 => Some(super::task_facts::safe_bytes(
                path.parent()
                    .ok_or_else(|| refuse("invalid_path", "Invalid index path."))?,
                path.file_name()
                    .and_then(|s| s.to_str())
                    .ok_or_else(|| refuse("invalid_path", "Invalid index name."))?,
                16 * 1024 * 1024,
            )?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            _ => {
                return Err(refuse(
                    "index_restore_failed",
                    "The target index cannot be safely retained.",
                ))
            }
        };
        let head = git
            .out(root, &["rev-parse", "HEAD"])
            .await?
            .trim()
            .to_owned();
        Ok(Self { path, bytes, head })
    }
    pub(super) fn restore(&self) -> Result<()> {
        let lock = self.path.with_file_name(format!(
            "{}.lock",
            self.path.file_name().unwrap().to_string_lossy()
        ));
        let result = (|| -> std::io::Result<()> {
            let mut stream = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW)
                .open(&lock)?;
            if let Some(bytes) = &self.bytes {
                stream.write_all(bytes)?;
            }
            stream.sync_all()?;
            drop(stream);
            if self.bytes.is_some() {
                fs::rename(&lock, &self.path)?;
            } else {
                if self.path.exists() {
                    fs::remove_file(&self.path)?;
                }
                fs::remove_file(&lock)?;
            }
            Ok(())
        })();
        result.map_err(|_| refuse("index_restore_failed", "Integration stopped and the previous staging selection could not be restored. Close other Git operations, inspect staged files in Changes, then retry. Your file contents were not discarded."))
    }
}
pub(super) async fn squash(
    root: &Path,
    source: &str,
    message: &str,
    snapshot: IndexSnapshot,
    git: &mut Git,
) -> Result<()> {
    match git.out(root, &["merge", "--squash", "--", source]).await {
        Ok(_) => {}
        // Timeout/unobservable effects leave the intent unresolved and index alone.
        Err(e) if e.code != "git_failed" => return Err(e),
        Err(_) => return stopped_merge(root, snapshot, git).await,
    }
    let committed: Result<()> = async {
        if !git.success(root, &["diff", "--cached", "--quiet"]).await? {
            git.out(root, &["commit", "-m", message]).await?;
        }
        Ok(())
    }
    .await;
    match committed {
        Ok(()) => Ok(()),
        Err(e) if e.code != "git_failed" => Err(e),
        Err(_) => {
            unchanged_head(root, &snapshot, git).await?;
            snapshot.restore()?;
            Err(refuse("integration_failed", "Git could not finish the target commit. Its previous staging selection was restored; the task commit is retained. Inspect the target checkout in Changes and fix its Git identity or commit hooks, then commit the retained changes there and refresh the task. Do not repeat Merge while the target has uncommitted changes."))
        }
    }
}
pub(super) async fn unchanged_head(
    root: &Path,
    snapshot: &IndexSnapshot,
    git: &mut Git,
) -> Result<()> {
    if git.out(root, &["rev-parse", "HEAD"]).await?.trim() != snapshot.head {
        return Err(refuse("integration_unknown", "A commit was recorded before integration stopped. Inspect Git history and refresh the task before continuing; do not repeat the merge blindly."));
    }
    Ok(())
}
/// `git merge --squash` failed. Unmerged index entries mean a real content
/// conflict that already rewrote the target working tree; anything else is a
/// refusal Git makes before touching files (for example files it would overwrite).
async fn stopped_merge(root: &Path, snapshot: IndexSnapshot, git: &mut Git) -> Result<()> {
    unchanged_head(root, &snapshot, git).await?;
    let unmerged = git
        .out(root, &["ls-files", "--unmerged", "-z"])
        .await
        .map_err(|e| match e.code {
            "git_timeout" => e,
            _ => refuse("integration_unknown", "Integration stopped and the target checkout state could not be confirmed. Inspect the target checkout in Changes and refresh the task before continuing; do not repeat Merge blindly."),
        })?;
    if unmerged.is_empty() {
        snapshot.restore()?;
        return Err(refuse("integration_failed", "Git refused to combine the task changes with the target checkout, so nothing was committed. Its previous staging selection was restored; the task commit is retained. Inspect the target checkout in Changes for uncommitted or untracked files the task would overwrite, move them yourself, then refresh the task before merging again."));
    }
    // A squash merge writes no MERGE_HEAD, so `git merge --abort` refuses here.
    // `reset --merge` undoes the attempted merge: conflict markers, staged task
    // files and SQUASH_MSG. The target was verified clean before the merge.
    // Cleanliness is confirmed before the exact index restore, which stays last:
    // status may refresh the index stat cache.
    let clean =
        git.success(root, &["reset", "--merge"]).await? && git.clean(root).await.unwrap_or(false);
    if clean {
        unchanged_head(root, &snapshot, git).await?;
        snapshot.restore()?;
        return Err(refuse("integration_conflict", "The task changes conflict with newer changes on the target branch, so nothing was committed. The target checkout was returned to its last commit with no merge in progress, and the task commit is retained. Update the task with the target branch's changes, resolve the conflict there, then merge again."));
    }
    Err(refuse("integration_conflict", "The task changes conflict with newer changes on the target branch, so nothing was committed, but Git could not fully undo the attempted merge: the target checkout may still contain conflict markers or staged task files. Inspect the target checkout in Changes and return it to its last commit yourself, then refresh the task. The task commit is retained."))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture_git(root: &Path, a: &[&str]) {
        let out = std::process::Command::new("git")
            .args(a)
            .current_dir(root)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .output()
            .unwrap();
        assert!(out.status.success(), "fixture git {a:?}");
    }
    fn observe(root: &Path, a: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(a)
            .current_dir(root)
            .output()
            .unwrap();
        assert!(out.status.success(), "observe git {a:?}");
        String::from_utf8(out.stdout).unwrap()
    }
    /// main: a.txt=base; task: a.txt=task plus b.txt; main then diverges.
    fn repo(name: &str, target_a: Option<&str>) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "cw-squash-{name}-{}",
            super::super::settings::unique()
        ));
        fs::create_dir(&root).unwrap();
        fixture_git(&root, &["init", "-q", "-b", "main"]);
        for (k, v) in [
            ("user.name", "Fixture"),
            ("user.email", "fixture@example.invalid"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", "/dev/null"),
        ] {
            fixture_git(&root, &["config", k, v]);
        }
        fs::write(root.join("a.txt"), "base\n").unwrap();
        fixture_git(&root, &["add", "."]);
        fixture_git(&root, &["commit", "-q", "-m", "Base"]);
        fixture_git(&root, &["checkout", "-q", "-b", "task"]);
        fs::write(root.join("a.txt"), "task\n").unwrap();
        fs::write(root.join("b.txt"), "task file\n").unwrap();
        fixture_git(&root, &["add", "."]);
        fixture_git(&root, &["commit", "-q", "-m", "Task"]);
        fixture_git(&root, &["checkout", "-q", "main"]);
        if let Some(content) = target_a {
            fs::write(root.join("a.txt"), content).unwrap();
            fixture_git(&root, &["commit", "-q", "-a", "-m", "Target"]);
        }
        root
    }
    #[tokio::test]
    async fn real_squash_conflict_is_undone_and_reported_as_a_conflict() {
        let root = repo("conflict", Some("target\n"));
        let head = observe(&root, &["rev-parse", "HEAD"]);
        let index = fs::read(root.join(".git/index")).unwrap();
        let mut git = Git::new();
        let snapshot = IndexSnapshot::capture(&root, &mut git).await.unwrap();
        assert_eq!(snapshot.bytes.as_deref(), Some(&index[..]));
        let error = squash(&root, "task", "Integrate", snapshot, &mut git)
            .await
            .unwrap_err();
        assert_eq!(error.code, "integration_conflict");
        assert!(error
            .message
            .contains("returned to its last commit with no merge in progress"));
        assert!(!error.message.contains("hooks"));
        // Exact prior staging selection; checked first because a plain status refreshes it.
        assert_eq!(fs::read(root.join(".git/index")).unwrap(), index);
        // The checkout matches the message: last commit, nothing staged or unmerged.
        assert_eq!(observe(&root, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            observe(&root, &["status", "--porcelain", "--untracked-files=all"]),
            ""
        );
        assert_eq!(observe(&root, &["ls-files", "--unmerged"]), "");
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "target\n");
        assert!(!root.join("b.txt").exists());
        for state in ["SQUASH_MSG", "MERGE_MSG", "MERGE_HEAD", "AUTO_MERGE"] {
            assert!(!root.join(".git").join(state).exists(), "{state} retained");
        }
        assert_eq!(
            observe(&root, &["log", "-1", "--format=%s", "task"]).trim(),
            "Task"
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn refused_squash_without_conflict_leaves_target_files_alone() {
        // Untracked target file the task would overwrite: Git refuses before any write.
        let root = repo("refused", None);
        fs::write(root.join("b.txt"), "user file\n").unwrap();
        let head = observe(&root, &["rev-parse", "HEAD"]);
        let mut git = Git::new();
        let snapshot = IndexSnapshot::capture(&root, &mut git).await.unwrap();
        let error = squash(&root, "task", "Integrate", snapshot, &mut git)
            .await
            .unwrap_err();
        assert_eq!(error.code, "integration_failed");
        assert!(error.message.contains("refused to combine"));
        assert!(!error.message.contains("hooks"));
        assert_eq!(observe(&root, &["rev-parse", "HEAD"]), head);
        assert_eq!(observe(&root, &["status", "--porcelain"]), "?? b.txt\n");
        assert_eq!(fs::read_to_string(root.join("a.txt")).unwrap(), "base\n");
        assert_eq!(
            fs::read_to_string(root.join("b.txt")).unwrap(),
            "user file\n"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
