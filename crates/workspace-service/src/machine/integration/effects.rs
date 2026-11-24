//! Integration effects after admission and the durable intent (task_git.py `integrate`).
//! No reset, stash, force removal or implicit push. Every failure after the first Git
//! effect is final: nothing is delegated or retried.
use super::conflicts;
use crate::machine::{
    git::{full_commit_id, valid_branch, Git},
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
use tokio::io::AsyncReadExt;

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
    pub automatic: bool,
    pub already_landed: bool,
    pub owned_worktree: bool,
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
    "changesBaseRevision",
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
    if plan.automatic {
        record["integration"]["automatic"] = json!(true);
        record["integration"]["committed"] = json!(!plan.already_landed);
    }
}
fn fresh(plan: &Plan<'_>) -> Value {
    json!({"resultId":plan.verified["resultId"],"version":plan.verified["version"]})
}
pub(super) async fn apply(plan: &Plan<'_>, product: &mut Value, git: &mut Git) -> Result<Value> {
    let path = plan.path;
    if plan.automatic {
        // Do not land on a branch or checkout that changed during admission.
        let branch = git.out(path, &["branch", "--show-current"]).await?;
        let observed =
            crate::machine::legacy_source::capture(path).map_err(crate::machine::context::core)?;
        if branch.trim() != plan.target
            || observed.as_deref() != product["tasks"][plan.task]["acceptedRevision"].as_str()
        {
            return Err(refuse(
                "workspace_drift",
                "The checkout changed before automatic finish. No commit was attempted.",
            ));
        }
    }
    if !plan.already_landed && !(plan.owned_worktree && plan.method == "squash") {
        commit_source(plan, git).await?;
    }
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
        if plan.owned_worktree && plan.method == "squash" {
            return Err(refuse("review_required", "The target changed. Update the task and rerun review before squash."));
        }
        return update_from_target(plan, product, &source_head, git).await;
    }
    if plan.method == "pr" {
        return pull_request(plan, product, &source_head, git).await;
    }
    squash(plan, product, &source_head, git).await
}
/// The target moved on: bring it into the task branch first. The target is untouched.
async fn update_from_target(
    plan: &Plan<'_>,
    product: &mut Value,
    source_head: &str,
    git: &mut Git,
) -> Result<Value> {
    let path = plan.path;
    let target_revision = super::update::target_revision(path, plan.target, git).await?;
    let before = crate::machine::task_merge::abort_snapshot(path, git).await?;
    let merged = git
        .success(
            path,
            &[
                "merge",
                "--no-autostash",
                "--no-edit",
                "-m",
                "Update task branch from target",
                "--",
                &target_revision,
            ],
        )
        .await?;
    if !merged {
        let (marker, other) = git.merge_marker(path).await?;
        if other || !marker.try_exists()? {
            return Err(refuse(
                "git_failed",
                "Git could not update the task branch. Inspect it before retrying.",
            ));
        }
        let record = &product["tasks"][plan.task];
        let merge = conflicts::retain(path, plan.source, plan.target, record, source_head, &target_revision, before, git).await?;
        let record = &mut product["tasks"][plan.task];
        record["requiresFreshResult"] = fresh(plan);
        record["conflict"] = json!("conflict");
        record["merge"] = merge;
        record["acceptance"] = Value::Null;
        record["automaticReady"] = json!(false);
        save(plan, product)?;
        return Err(refuse(
            "merge_conflict",
            "The conflict is in the task worktree. The target was not changed.",
        ));
    }
    super::update::verify_merged(path, &target_revision, git).await?;
    let record = &mut product["tasks"][plan.task];
    record["changesBaseRevision"] = json!(target_revision);
    record["requiresFreshResult"] = fresh(plan);
    record["acceptance"] = Value::Null;
    record["automaticReady"] = json!(false);
    save(plan, product)?;
    Err(refuse(
        "review_required",
        "The task branch was updated from the target. Rerun checks and review it before merging.",
    ))
}
async fn pull_request(
    plan: &Plan<'_>,
    product: &mut Value,
    source_head: &str,
    git: &mut Git,
) -> Result<Value> {
    let path = plan.path;
    let record = &product["tasks"][plan.task];
    if super::truth(&record["prAttempt"]) || record["integration"]["method"] == "pr" {
        return Err(refuse(
            "unknown_outcome",
            "A pull request was already attempted. Check GitHub before creating another.",
        ));
    }
    if let Some(accepted) = record["acceptance"]["committedDigest"].as_str() {
        if super::super::task_facts::native_observation(path)
            .await
            .as_deref()
            != Some(accepted)
        {
            return Err(refuse(
                "stale_result",
                "This result or its workspace changed. Rerun checks and review the new result.",
            ));
        }
    }
    let remote_ref = format!("refs/heads/{}", plan.source);
    let witness = &product["originPushWitnesses"][&remote_ref];
    if !valid_branch(plan.source)
        || !full_commit_id(source_head)
        || witness["commit"] != source_head
        || witness["ref"] != remote_ref
    {
        return Err(push_required());
    }
    let repository = origin_repository(path, git).await?;
    let push_url = git
        .out(path, &["remote", "get-url", "--push", "--all", "origin"])
        .await
        .map_err(|_| invalid_pr_remote())?;
    if GitHubRepository::from_remote(&push_url).as_ref() != Some(&repository) {
        return Err(invalid_pr_remote());
    }
    let repo_arg = repository.qualified_name();
    if witness["repository"] != repo_arg {
        return Err(push_required());
    }
    // Only branches accepted by confirmed Push can reach this lookup. Check
    // the exact ref and its repository-qualified API self link as well as the
    // SHA, so a redirect or a different ref is not evidence for this branch.
    let remote = repository
        .get(path, &format!("git/ref/heads/{}", plan.source))
        .await
        .ok_or_else(push_required)?;
    if remote["ref"] != remote_ref
        || remote["object"]["type"] != "commit"
        || remote["object"]["sha"] != source_head
        || !remote["url"]
            .as_str()
            .is_some_and(|url| repository.matches_ref_url(url, plan.source))
    {
        return Err(push_required());
    }

    // The provider can still move the branch between this read and creation.
    // Persist the attempt before dispatch and never replay a mutation on doubt.
    product["tasks"][plan.task]["prAttempt"] =
        json!({"state":"unknown","target":plan.target,"head":plan.source});
    save(plan, product)?;
    let title = plan.message.lines().next().unwrap_or("");
    let body = plan.params["body"].as_str().unwrap_or("");
    let mut command = repository.github_command(path);
    command.args([
        "pr",
        "create",
        "--repo",
        &repo_arg,
        "--base",
        plan.target,
        "--head",
        plan.source,
        "--title",
        title,
        "--body",
        body,
    ]);
    if super::truth(&plan.params["draft"]) {
        command.arg("--draft");
    }
    let printed = match github_output(command).await {
        Ok(Some(output)) => output,
        Err(_) => {
            return Err(refuse(
                "unknown_outcome",
                "GitHub did not answer. Check your pull requests before retrying.",
            ))
        }
        Ok(None) => return Err(refuse(
            "gh_failed",
            "GitHub CLI could not create the pull request. Check gh login and repository access.",
        )),
    };
    let reported = std::str::from_utf8(&printed)
        .ok()
        .and_then(|text| text.lines().rfind(|line| !line.trim().is_empty()))
        .and_then(|line| repository.pull_request_url(line.trim()));
    let Some(url) = reported else {
        return Err(refuse(
            "pull_request_unsafe",
            "GitHub CLI did not return a valid pull request address for this repository. Check GitHub before retrying.",
        ));
    };
    let number = url.rsplit('/').next().unwrap_or("");
    let expected_repository = format!("{}/{}", repository.owner, repository.repo);
    if !repository
        .get(path, &format!("pulls/{number}"))
        .await
        .is_some_and(|pr| {
            let matches_repo = |side: &str| {
                pr[side]["repo"]["full_name"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(&expected_repository))
            };
            matches_repo("head")
                && matches_repo("base")
                && pr["head"]["ref"] == plan.source
                && pr["head"]["sha"] == source_head
                && pr["base"]["ref"] == plan.target
                && pr["state"] == "open"
                && pr["html_url"] == url
        })
    {
        return Err(refuse(
            "pull_request_unsafe",
            "GitHub did not confirm the expected open pull request and commit. Check GitHub before retrying.",
        ));
    }
    product["tasks"][plan.task]["integration"] = json!({"method":"pr","targetBranch":plan.target,
        "digest":plan.params["digest"],"title":plan.message,"url":url});
    save(plan, product)?;
    Ok(json!({"done":true}))
}

fn push_required() -> Refused {
    refuse(
        "push_required",
        "Workspace could not verify a confirmed push of this exact commit to origin. Check the remote and push this branch from Git before creating the pull request.",
    )
}

/// Bound both the local wait and captured output; never expose CLI diagnostics
/// or replay a call whose effect is unknown. Dropping the child terminates it.
async fn github_output(
    mut command: tokio::process::Command,
) -> std::result::Result<Option<Vec<u8>>, tokio::time::error::Elapsed> {
    tokio::time::timeout(Duration::from_secs(25), async {
        let mut child = command.spawn().ok()?;
        let mut stdout = child.stdout.take()?.take(256 * 1024 + 1);
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).await.ok()?;
        if bytes.len() > 256 * 1024 || !child.wait().await.ok()?.success() {
            return None;
        }
        Some(bytes)
    })
    .await
}

fn invalid_pr_remote() -> Refused {
    refuse(
        "pull_request_unsafe",
        "Configure origin to fetch and push to the same single GitHub repository before creating a pull request.",
    )
}

pub(super) async fn origin_repository(path: &Path, git: &mut Git) -> Result<GitHubRepository> {
    let remote = git
        .out(path, &["remote", "get-url", "--all", "origin"])
        .await
        .map_err(|_| invalid_pr_remote())?;
    GitHubRepository::from_remote(&remote).ok_or_else(invalid_pr_remote)
}

#[derive(Debug, PartialEq, Eq)]
pub(in crate::machine) struct GitHubRepository {
    host: String,
    owner: String,
    repo: String,
}

impl GitHubRepository {
    pub(in crate::machine) fn from_remote(remote: &str) -> Option<Self> {
        // Git emits one effective URL per line. Reject ambiguity rather than
        // selecting the first URL from a multi-destination origin.
        let remote = remote.strip_suffix('\n').unwrap_or(remote);
        if remote.len() > 2048 || remote.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return None;
        }
        let (host, path) = match remote.split_once("://") {
            Some((scheme @ ("https" | "ssh"), rest)) => {
                let (authority, path) = rest.split_once('/')?;
                let authority = authority.rsplit('@').next()?;
                let host = if let Some((host, port)) = authority.split_once(':') {
                    match (scheme, port.parse::<u16>().ok()?) {
                        ("https", 443) | ("ssh", 1..=65535) => host,
                        _ => return None,
                    }
                } else {
                    authority
                };
                (host, path)
            }
            Some(_) => return None,
            None => {
                let (authority, path) = remote.split_once(':')?;
                (authority.rsplit('@').next()?, path)
            }
        };
        let (owner, repo) = path.split_once('/')?;
        let repo = repo.strip_suffix(".git").unwrap_or(repo);
        if !Self::valid_host(host) || !Self::valid_name(owner) || !Self::valid_name(repo) {
            return None;
        }
        Some(Self {
            host: host.to_ascii_lowercase(),
            owner: owner.to_ascii_lowercase(),
            repo: repo.to_ascii_lowercase(),
        })
    }

    fn valid_host(host: &str) -> bool {
        host.len() <= 253
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            })
    }

    fn valid_name(name: &str) -> bool {
        !name.is_empty()
            && !name.starts_with('.')
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    }

    pub(in crate::machine) fn qualified_name(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.repo)
    }

    fn github_command(&self, path: &Path) -> tokio::process::Command {
        let mut command = tokio::process::Command::new("gh");
        command
            .current_dir(path)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GH_PROMPT_DISABLED", "1")
            .env("LC_ALL", "C")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env_remove("GH_REPO")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        if std::env::var("GH_HOST").as_deref() != Ok(self.host.as_str()) {
            command.env_remove("GH_HOST");
        }
        command
    }

    async fn get(&self, path: &Path, suffix: &str) -> Option<Value> {
        let endpoint = format!("repos/{}/{}/{suffix}", self.owner, self.repo);
        let mut command = self.github_command(path);
        command.args([
            "api",
            "--method",
            "GET",
            "--hostname",
            &self.host,
            &endpoint,
        ]);
        let bytes = github_output(command).await.ok()??;
        serde_json::from_slice(&bytes).ok()
    }

    fn matches_ref_url(&self, url: &str, branch: &str) -> bool {
        let root = if self.host == "github.com" {
            "https://api.github.com/repos/".to_owned()
        } else {
            format!("https://{}/api/v3/repos/", self.host)
        };
        url.strip_prefix(&root)
            .and_then(|suffix| suffix.split_once("/git/refs/heads/"))
            .is_some_and(|(name, reference)| {
                name.eq_ignore_ascii_case(&format!("{}/{}", self.owner, self.repo))
                    && reference == branch
            })
    }

    /// Only the exact repository's HTTPS PR route can be saved or opened.
    pub(super) fn pull_request_url<'a>(&self, text: &'a str) -> Option<&'a str> {
        if text.len() > 2048 || text.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return None;
        }
        let (host, path) = text.strip_prefix("https://")?.split_once('/')?;
        let mut parts = path.split('/');
        let (owner, repo, kind, number) =
            (parts.next()?, parts.next()?, parts.next()?, parts.next()?);
        (host.eq_ignore_ascii_case(&self.host)
            && owner.eq_ignore_ascii_case(&self.owner)
            && repo.eq_ignore_ascii_case(&self.repo)
            && kind == "pull"
            && !number.is_empty()
            && number.len() <= 12
            && number.bytes().all(|b| b.is_ascii_digit())
            && parts.next().is_none())
        .then_some(text)
    }
}
#[cfg(test)]
mod address_tests {
    use super::GitHubRepository;

    #[test]
    fn pull_request_address_requires_the_exact_remote_repository() {
        let ok = "https://github.com/owner/repo/pull/42";
        for remote in [
            "https://github.com/owner/repo.git",
            "https://user@GitHub.com:443/Owner/Repo",
            "git@github.com:owner/repo.git",
            "ssh://git@github.com/owner/repo.git",
        ] {
            let repo = GitHubRepository::from_remote(remote).unwrap();
            assert_eq!(repo.qualified_name(), "github.com/owner/repo", "{remote}");
            assert_eq!(repo.pull_request_url(ok), Some(ok), "{remote}");
        }

        let repository = GitHubRepository::from_remote("git@github.com:owner/repo.git").unwrap();
        let mixed_case = "https://GitHub.com/Owner/Repo/pull/42";
        assert_eq!(repository.pull_request_url(mixed_case), Some(mixed_case));
        for url in [
            "http://github.com/owner/repo/pull/42",
            "https://evil.example/owner/repo/pull/42",
            "https://github.com/owner/other/pull/42",
            "https://github.com/another/repo/pull/42",
            "https://user:pw@github.com/owner/repo/pull/42",
            "https://github.com:8443/owner/repo/pull/42",
            "https://github.com/owner/repo/issues/42",
            "https://github.com/owner/repo/pull/42?x=1",
            "https://github.com/owner/repo/pull/42 x",
            "https://github.com/../repo/pull/42",
            "https://github.com/owner/repo/pull/42/extra",
            "javascript:alert(1)",
        ] {
            assert_eq!(repository.pull_request_url(url), None, "{url}");
        }
    }

    #[test]
    fn pull_request_origin_rejects_ambiguous_or_unsupported_remote_urls() {
        for remote in [
            "",
            "/srv/git/repo.git",
            "http://github.com/owner/repo.git",
            "https://github.com:8443/owner/repo.git",
            "https://github.com/owner/nested/repo.git",
            "https://github.com/owner/repo.git?query=1",
            "https://github.com/owner/repo.git\nhttps://github.com/owner/other.git\n",
        ] {
            assert!(GitHubRepository::from_remote(remote).is_none(), "{remote}");
        }
        let enterprise =
            GitHubRepository::from_remote("ssh://git@github.example:2222/Owner/Repo.git").unwrap();
        assert_eq!(enterprise.qualified_name(), "github.example/owner/repo");
    }
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
    if plan.owned_worktree {
        let branch = git.out(&tree, &["branch", "--show-current"]).await?;
        let source_common = git.out(plan.root, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).await?;
        let target_common = git.out(&tree, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).await?;
        if branch.trim() != plan.target || source_common.trim() != target_common.trim() || !git.clean(&tree).await? {
            return Err(refuse("unsafe_target", "The chosen target checkout changed. No squash was attempted."));
        }
    }
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
    if plan.owned_worktree && plan.params["deleteWorktree"] != false {
        let cleanup = finish_owned_cleanup(plan);
        let record = &mut product["tasks"][plan.task]["integration"];
        record["worktreeCleanup"] = cleanup["status"].clone();
        record["worktreeCleanupReason"] = cleanup["reason"].clone();
        let _ = save(plan, product);
        return Ok(json!({"done":true,"worktreeCleanup":cleanup}));
    }
    if !plan.owned_worktree && plan.params["deleteWorktree"] == true
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
fn finish_owned_cleanup(plan: &Plan<'_>) -> Value {
    use cortex_core::{OpenMode, CoreError, workspace::worktrees::{self, Cleanup}};
    let settled = (|| {
        let store = super::context::open(plan.root, OpenMode::RuntimeValidate)?
            .ok_or_else(|| super::context::core(CoreError::Refused("worktree_not_owned")))?;
        store.write(|db| worktrees::queue_merge_cleanup_tx(db, plan.project, plan.task))
            .map_err(super::context::core)?;
        worktrees::finish_cleanup(&store, plan.project, plan.task, Cleanup::Close)
            .map_err(super::context::core)?
            .ok_or_else(|| super::context::core(CoreError::Refused("worktree_cleanup_unconfirmed")))
    })();
    settled.unwrap_or_else(|error| json!({
        "status":"retained","path":plan.path,"reason":error.code
    }))
}

async fn remove_temp(plan: &Plan<'_>, temporary: &Option<PathBuf>, git: &mut Git) {
    if let Some(folder) = temporary {
        let text = folder.to_string_lossy().into_owned();
        let _ = git
            .out(plan.root, &["worktree", "remove", "--", &text])
            .await;
    }
}
