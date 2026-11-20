//! Installed Git argv only. Inspection overrides filters per command; writes retain hooks.
use super::{
    files::revision,
    native::*,
    settings::{self, Lock},
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::Command,
};
pub(super) mod listing;
#[cfg(test)]
mod pull_tests;
mod snapshot_reuse;
const MAX_DIFF: usize = 400000;
const MAX_TREE: usize = 20000;
const MAX_UNTRACKED: usize = 2000;
#[derive(Clone)]
pub struct Git {
    configs: BTreeMap<PathBuf, Vec<String>>,
    runtime_config: Option<crate::Config>,
}
struct Output {
    bytes: Vec<u8>,
    ok: bool,
    text: String,
    error: String,
}
fn too_large() -> Refused {
    refuse(
        "git_output_too_large",
        "Git output exceeds the bounded inspection limit.",
    )
}
/// Why Git refused a command, from its own (C locale) error text, as a code and what to do next. Only the
/// classification leaves this function: stderr can name remote URLs, hook output or credentials.
pub(super) fn failure(verb: &str, stderr: &str) -> Refused {
    // Only the network commands are classified. Every other command keeps `git_failed`, which
    // callers such as the squash integration rely on to restore the index after a refused merge
    // or commit, and whose output (a hook's, say) can contain any of the words below.
    if !matches!(verb, "push" | "pull" | "fetch") {
        return refuse(
            "git_failed",
            "Git refused this operation. Inspect the checkout in Git; no automatic retry was made.",
        );
    }
    let e = stderr.to_ascii_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| e.contains(n));
    let nothing = match verb {
        "push" => " Nothing was pushed.",
        "pull" => " Nothing was pulled.",
        "fetch" => " Nothing was fetched.",
        _ => " Nothing changed.",
    };
    let next = |why: &str, todo: &str| format!("{why}{nothing} {todo}");
    if has(&[
        "protected branch",
        "gh006",
        "not allowed to push code to protected",
        "not allowed to force push code to a protected",
    ]) {
        return refuse(
            "push_protected_branch",
            format!("The remote protects this branch and refused the push.{nothing} Push to another branch and open a pull request, or ask the repository owner to allow it."),
        );
    }
    if has(&[
        "hook declined",
        "pre-receive hook",
        "pre-push hook",
        "hook rejected",
    ]) {
        return refuse(
            "git_hook_rejected",
            format!("A Git hook rejected this operation.{nothing} Read the hook's message by running the same command in the terminal (Ctrl+J), fix what it reports, then try again."),
        );
    }
    if verb == "push"
        && has(&[
            "(fetch first)",
            "(non-fast-forward)",
            "non-fast-forward",
            "tip of your current branch is behind",
        ])
    {
        return refuse(
            "push_not_fast_forward",
            next("The remote branch has commits this checkout does not have, so the push would not be a fast-forward.", "Fetch, then choose Pull… to merge or rebase the remote commits (or do it in the terminal, Ctrl+J), and push again. Cortex never force-pushes."),
        );
    }
    if verb == "pull"
        && has(&[
            "you have unstaged changes",
            "your index contains uncommitted changes",
            "would be overwritten by merge",
            "would be overwritten by checkout",
            "untracked working tree files would be",
        ])
    {
        return refuse(
            "dirty_checkout",
            next(
                "Pulling would overwrite files in this checkout that are not committed.",
                "Commit them, or move them out of the way, then pull again. Cortex does not stash.",
            ),
        );
    }
    if has(&[
        "not possible to fast-forward",
        "diverging branches",
        "have diverged",
        "need to specify how to reconcile",
    ]) {
        return refuse(
            "pull_not_fast_forward",
            next(
                "The local and remote histories diverged, so Git cannot fast-forward this branch.",
                "Choose Pull… to merge or rebase the remote commits, then push.",
            ),
        );
    }
    if has(&[
        "no tracking information",
        "has no upstream branch",
        "no upstream configured",
        "no such ref was fetched",
    ]) {
        return refuse(
            "upstream_required",
            next("This branch has no upstream.", "Set one in the terminal (git push -u <remote> <branch> or git branch --set-upstream-to), then try again."),
        );
    }
    if has(&[
        "authentication failed",
        "permission denied (publickey",
        "could not read username",
        "could not read password",
        "terminal prompts disabled",
        "invalid username or password",
        "the requested url returned error: 403",
        "the requested url returned error: 401",
        "permission to ",
    ]) {
        return refuse(
            "git_auth_failed",
            next("The remote did not accept this machine's credentials.", "Check your Git login or SSH key for this remote in the terminal (Ctrl+J), then try again."),
        );
    }
    if has(&[
        "could not resolve host",
        "connection refused",
        "connection timed out",
        "network is unreachable",
        "unable to access",
        "could not read from remote repository",
        "does not appear to be a git repository",
        "repository not found",
        "failed to connect",
    ]) {
        return refuse(
            "remote_unreachable",
            next(
                "The remote could not be reached or does not exist.",
                "Check the network and the remote's address in Git settings, then try again.",
            ),
        );
    }
    refuse(
        "git_failed",
        "Git refused this operation. Inspect the checkout in Git; no automatic retry was made.",
    )
}
/// A pull or another merge or rebase is stopped in the checkout.
const OPERATION_OPEN: &str = "A merge or rebase is stopped in this checkout. Continue or abort it first; nothing was pulled.";
const NO_OPERATION: &str =
    "No merge or rebase is in progress in this checkout. Refresh Git status.";
/// A merge or rebase that stopped at conflicts, as the owner reads it.
fn stopped_at(operation: &Value) -> Refused {
    let n = operation["conflicts"].as_array().map_or(0, Vec::len);
    let kind = if operation["kind"] == "rebase" {
        "rebase"
    } else {
        "merge"
    };
    refuse(
        "pull_conflict",
        format!("The {kind} stopped at {n} conflicting file{}. Nothing was stashed or reset. Resolve {}, mark {} resolved and Continue, or Abort to return to where the checkout was before the pull.", if n == 1 { "" } else { "s" }, if n == 1 { "it" } else { "them" }, if n == 1 { "it" } else { "them" }),
    )
}
fn args(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}
impl Git {
    pub fn new() -> Self {
        Self {
            configs: BTreeMap::new(),
            runtime_config: None,
        }
    }
    pub fn for_service(config: &crate::Config) -> Self {
        Self {
            configs: BTreeMap::new(),
            runtime_config: Some(config.clone()),
        }
    }
    /// `None` when stdout exceeds `limit`; the child is killed as soon as that is known.
    async fn raw_bounded(
        &self,
        path: &Path,
        argv: &[String],
        input: Option<Vec<u8>>,
        limit: usize,
    ) -> Result<Option<Output>> {
        let mut cmd = Command::new("git");
        cmd.env_clear();
        for key in [
            "HOME",
            "PATH",
            "LANG",
            "LC_ALL",
            "XDG_CONFIG_HOME",
            "XDG_STATE_HOME",
            "XDG_CACHE_HOME",
            "XDG_RUNTIME_DIR",
        ] {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        if let Some(config) = &self.runtime_config {
            crate::launch::runtime_environment(&mut cmd, config)?;
        }
        cmd.args(["-c", "core.quotepath=false", "-c", "color.ui=false"])
            .args(argv)
            .current_dir(path)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("LC_ALL", "C")
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_COMMON_DIR")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            // A missing working directory fails the spawn with the same NotFound as a missing
            // `git`; only the latter means Git is not installed.
            if e.kind() == std::io::ErrorKind::NotFound && !path.is_dir() {
                refuse(
                    "checkout_missing",
                    "This checkout folder no longer exists. Prune it in Git (git worktree prune).",
                )
            } else if e.kind() == std::io::ErrorKind::NotFound {
                refuse("git_unavailable", "Git is not installed on this machine.")
            } else {
                e.into()
            }
        })?;
        let mut stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        // Each stream stops at its own bound and the first overflow ends the read at once:
        // waiting on a child that is blocked writing into a pipe nobody drains any more
        // only ends in the timeout below.
        async fn bounded<R: tokio::io::AsyncRead + Unpin>(
            reader: R,
            limit: usize,
        ) -> std::result::Result<Vec<u8>, Option<std::io::Error>> {
            let mut data = Vec::new();
            reader
                .take(limit as u64 + 1)
                .read_to_end(&mut data)
                .await
                .map_err(Some)?;
            if data.len() > limit {
                return Err(None);
            }
            Ok(data)
        }
        let writer = tokio::spawn(async move {
            if let Some(bytes) = input {
                stdin.write_all(&bytes).await?;
            }
            stdin.shutdown().await
        });
        let run = async {
            let read = tokio::try_join!(bounded(stdout, limit), bounded(stderr, 65536));
            #[cfg(test)]
            snapshot_reuse::record(path, argv, read.as_ref().map_or(limit + 1, |r| r.0.len()));
            let (out, err) = match read {
                Ok(read) => read,
                Err(problem) => {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                    writer.abort();
                    return match problem {
                        Some(error) => Err(error.into()),
                        None => Ok(None),
                    };
                }
            };
            let status = child.wait().await?;
            writer
                .await
                .map_err(|_| refuse("git_failed", "Git input could not be written."))??;
            Ok(Some(Output {
                ok: status.success(),
                text: String::from_utf8_lossy(&out).into_owned(),
                bytes: out,
                error: String::from_utf8_lossy(&err).into_owned(),
            }))
        };
        tokio::time::timeout(Duration::from_secs(25),run).await.map_err(|_|refuse("git_timeout","Git did not answer in time; a write may have completed. Inspect before retrying."))?
    }
    async fn raw(&self, path: &Path, argv: &[String], input: Option<Vec<u8>>) -> Result<Output> {
        self.raw_bounded(path, argv, input, workspace_protocol::FRAME_BYTES)
            .await?
            .ok_or_else(too_large)
    }
    async fn run(
        &mut self,
        path: &Path,
        argv: Vec<String>,
        check: bool,
        input: Option<Vec<u8>>,
    ) -> Result<Output> {
        self.run_bounded(path, argv, check, input, workspace_protocol::FRAME_BYTES)
            .await?
            .ok_or_else(too_large)
    }
    async fn run_bounded(
        &mut self,
        path: &Path,
        argv: Vec<String>,
        check: bool,
        input: Option<Vec<u8>>,
        limit: usize,
    ) -> Result<Option<Output>> {
        let changes = snapshot_reuse::mutating(&argv);
        let verb = argv.first().cloned().unwrap_or_default();
        let full = self.prepared(path, argv).await?;
        let found = self.raw_bounded(path, &full, input, limit).await;
        if changes {
            // Whatever the outcome, a Git-changing command ends every reused snapshot.
            snapshot_reuse::invalidate();
        }
        let Some(out) = found? else {
            return Ok(None);
        };
        if check && !out.ok {
            // Never return stderr: remote URLs, hook output or credentials can occur there.
            return Err(failure(&verb, &out.error));
        }
        Ok(Some(out))
    }
    /// The complete argv of one command: inspection commands carry the per-repository
    /// overrides that disable hooks, file monitors and every configured filter driver.
    async fn prepared(&mut self, path: &Path, argv: Vec<String>) -> Result<Vec<String>> {
        let verb = argv.first().map(String::as_str).unwrap_or("");
        let read = matches!(
            verb,
            "status"
                | "diff"
                | "log"
                | "show"
                | "ls-files"
                | "check-attr"
                | "check-ignore"
                | "rev-list"
        );
        let mut full = Vec::new();
        if read {
            if !self.configs.contains_key(path) {
                let found = self
                    .raw(
                        path,
                        &args(&[
                            "config",
                            "--null",
                            "--name-only",
                            "--get-regexp",
                            r"^filter\.",
                        ]),
                        None,
                    )
                    .await?;
                if !found.ok && !found.error.is_empty() {
                    return Err(refuse(
                        "git_failed",
                        "Git filter settings could not be inspected.",
                    ));
                }
                let drivers: BTreeSet<_> = found
                    .text
                    .split('\0')
                    .filter_map(|s| s.rsplit_once('.').map(|(k, _)| k))
                    .collect();
                let mut config = args(&[
                    "-c",
                    "core.hooksPath=/dev/null",
                    "-c",
                    "core.fsmonitor=false",
                    "-c",
                    "core.untrackedCache=false",
                ]);
                for driver in drivers {
                    for (key, value) in [
                        ("clean", ""),
                        ("smudge", ""),
                        ("process", ""),
                        ("required", "false"),
                    ] {
                        config.extend(["-c".to_owned(), format!("{driver}.{key}={value}")]);
                    }
                }
                self.configs.insert(path.to_path_buf(), config);
            }
            full.extend(self.configs[path].clone());
        }
        full.push(verb.to_owned());
        if matches!(verb, "diff" | "log" | "show") {
            full.extend(args(&["--no-ext-diff", "--no-textconv"]));
        }
        full.extend(argv.into_iter().skip(1));
        Ok(full)
    }
    pub(super) async fn out(&mut self, path: &Path, a: &[&str]) -> Result<String> {
        Ok(self.run(path, args(a), true, None).await?.text)
    }
    pub(super) async fn bytes(&mut self, path: &Path, a: &[&str]) -> Result<Vec<u8>> {
        Ok(self.run(path, args(a), true, None).await?.bytes)
    }
    async fn checked(&mut self, path: &Path, a: Vec<String>) -> Result<()> {
        self.run(path, a, true, None).await?;
        Ok(())
    }
    pub(super) async fn success(&mut self, path: &Path, a: &[&str]) -> Result<bool> {
        Ok(self.run(path, args(a), false, None).await?.ok)
    }
    pub async fn is_repo(&mut self, path: &Path) -> Result<bool> {
        let out = match self
            .run(
                path,
                args(&["rev-parse", "--is-inside-work-tree"]),
                false,
                None,
            )
            .await
        {
            Err(e) if e.code == "git_unavailable"
                && crate::native_effects::no_checkout_marker(path) =>
            {
                return Ok(false);
            }
            result => result?,
        };
        if out.ok {
            return Ok(out.text.trim() == "true");
        }
        if out.error.contains("not a git repository")
            && fs::symlink_metadata(path.join(".git"))
                .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
        {
            Ok(false)
        } else {
            Err(refuse(
                "git_failed",
                "Repository availability could not be confirmed.",
            ))
        }
    }
    /// Whether a repository has any commit to integrate into. A freshly
    /// initialized repository has no branch at all (`HEAD` names an unborn
    /// branch), so no target branch can exist until its first commit.
    pub(super) async fn has_history(&mut self, path: &Path) -> Result<bool> {
        if self
            .success(path, &["rev-parse", "--verify", "--quiet", "HEAD"])
            .await?
        {
            return Ok(true);
        }
        Ok(!self
            .out(
                path,
                &[
                    "for-each-ref",
                    "--count=1",
                    "--format=%(refname)",
                    "refs/heads",
                ],
            )
            .await?
            .trim()
            .is_empty())
    }
    pub(super) async fn trees(&mut self, root: &Path) -> Result<Vec<Value>> {
        let text = self.out(root, &["worktree", "list", "--porcelain"]).await?;
        let mut result = Vec::new();
        let mut entry = json!({});
        for line in text.lines().chain(std::iter::once("")) {
            if line.is_empty() {
                if entry.get("path").is_some() {
                    result.push(entry);
                }
                entry = json!({});
            } else if let Some(path) = line.strip_prefix("worktree ") {
                entry["path"] = json!(path);
            } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
                entry["branch"] = json!(branch);
            } else if line == "detached" {
                entry["branch"] = json!("");
            }
        }
        Ok(result)
    }
    pub async fn resolve(&mut self, root: &Path, project: &str, wanted: &str) -> Result<PathBuf> {
        if !self.is_repo(root).await? {
            return if wanted == format!("{project}:main") {
                Ok(root.to_path_buf())
            } else {
                Err(refuse(
                    "unknown_checkout",
                    "This folder has no separate Git worktrees.",
                ))
            };
        }
        for (i, t) in self.trees(root).await?.iter().enumerate() {
            let path = t["path"].as_str().unwrap_or("");
            if checkout_id(project, path, i == 0) == wanted {
                return Ok(PathBuf::from(path));
            }
        }
        Err(refuse(
            "unknown_checkout",
            "That checkout is not part of this project.",
        ))
    }
    pub async fn filters(
        &mut self,
        path: &Path,
        names: &[String],
    ) -> Result<BTreeMap<String, String>> {
        if names.is_empty() {
            return Ok(BTreeMap::new());
        }
        let input = format!("{}\0", names.join("\0")).into_bytes();
        let out = self
            .run(
                path,
                args(&["check-attr", "-z", "--stdin", "filter"]),
                true,
                Some(input),
            )
            .await?;
        Ok(out
            .text
            .split('\0')
            .collect::<Vec<_>>()
            .chunks_exact(3)
            .filter(|p| p[2] != "unspecified" && p[2] != "unset")
            .map(|p| (p[0].to_owned(), p[2].to_owned()))
            .collect())
    }
    /// Which of `paths` (relative to `path`) Git ignores, from one `check-ignore` read.
    /// A tracked file is never reported ignored.
    pub(super) async fn ignored(
        &mut self,
        path: &Path,
        paths: &[String],
    ) -> Result<BTreeSet<String>> {
        if paths.is_empty() {
            return Ok(BTreeSet::new());
        }
        let input = format!("{}\0", paths.join("\0")).into_bytes();
        let out = self
            .run(
                path,
                args(&["check-ignore", "-z", "--stdin"]),
                false,
                Some(input),
            )
            .await?;
        // Exit 1 with nothing on stderr only means that none is ignored.
        if !out.ok && !out.error.is_empty() {
            return Err(refuse(
                "git_failed",
                "Git could not say which files it ignores.",
            ));
        }
        Ok(out
            .text
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect())
    }
    /// What the snapshot's tree leaves out, without descending into it: every ignored file and
    /// top-most ignored folder, and every untracked folder (also empty ones), as `(path, folder,
    /// ignored)`. `None` when Git's answer is larger than `limit` bytes.
    pub(super) async fn outside_tree(
        &mut self,
        path: &Path,
        limit: usize,
    ) -> Result<Option<Vec<(String, bool, bool)>>> {
        let mut found = Vec::new();
        for ignored in [true, false] {
            let mut a = args(&[
                "ls-files",
                "-z",
                "--others",
                "--exclude-standard",
                "--directory",
            ]);
            if ignored {
                a.push("--ignored".into());
            }
            let Some(out) = self.run_bounded(path, a, true, None, limit).await? else {
                return Ok(None);
            };
            for name in out.text.split('\0').filter(|s| !s.is_empty()) {
                let (name, folder) = match name.strip_suffix('/') {
                    Some(n) => (n, true),
                    None => (name, false),
                };
                // Untracked files are in the tree already; only their folders are added.
                if (ignored || folder) && !name.split('/').any(|p| RESERVED.contains(&p)) {
                    found.push((name.to_owned(), folder, ignored));
                }
            }
        }
        Ok(Some(found))
    }
    async fn listed(&mut self, path: &Path, directories: bool) -> Result<Vec<String>> {
        let mut a = args(&["ls-files", "-z", "--others", "--exclude-standard"]);
        if directories {
            a.extend(args(&["--directory", "--no-empty-directory"]));
        }
        for n in RESERVED {
            a.push(format!("--exclude={n}/"));
        }
        Ok(self
            .run(path, a, true, None)
            .await?
            .text
            .split('\0')
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect())
    }
    async fn untracked(&mut self, path: &Path) -> Result<(Vec<String>, BTreeSet<String>)> {
        let names = self.listed(path, false).await?;
        if names.len() <= MAX_UNTRACKED {
            return Ok((names, BTreeSet::new()));
        }
        let folders: BTreeSet<_> = self
            .listed(path, true)
            .await?
            .into_iter()
            .filter_map(|n| n.strip_suffix('/').map(str::to_owned))
            .collect();
        let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut loose = Vec::new();
        for name in names {
            let mut at = 0;
            let owner = name
                .split('/')
                .take(name.split('/').count().saturating_sub(1))
                .find_map(|p| {
                    at += p.len();
                    let prefix = &name[..at];
                    at += 1;
                    if folders.contains(prefix) {
                        Some(prefix.to_owned())
                    } else {
                        None
                    }
                });
            if let Some(owner) = owner {
                groups.entry(owner).or_default().push(name);
            } else {
                loose.push(name);
            }
        }
        let mut collapsed: BTreeSet<_> = groups.keys().cloned().collect();
        let mut room = MAX_UNTRACKED as isize - loose.len() as isize - groups.len() as isize;
        let mut groups = groups.into_iter().collect::<Vec<_>>();
        groups.sort_by(|a, b| (a.1.len(), &a.0).cmp(&(b.1.len(), &b.0)));
        for (folder, members) in groups {
            if members.len() as isize - 1 > room {
                break;
            }
            room -= members.len() as isize - 1;
            loose.extend(members);
            collapsed.remove(&folder);
        }
        loose.extend(collapsed.iter().cloned());
        if loose.len() > MAX_UNTRACKED {
            return Err(refuse("repo_too_large", "More than 2,000 untracked entries remain after directory collapsing. Inspect Git before selecting files."));
        }
        loose.sort();
        Ok((loose, collapsed))
    }
    /// Tracked changes as `(stage, letter, path)`, without diffs and without untracked files.
    async fn tracked_changes(
        &mut self,
        path: &Path,
    ) -> Result<Vec<(&'static str, &'static str, String)>> {
        let raw = self
            .out(
                path,
                &["status", "--porcelain=v2", "-z", "--untracked-files=no"],
            )
            .await?;
        let entries = raw.split('\0').collect::<Vec<_>>();
        let mut items = Vec::new();
        let mut i = 0;
        while i < entries.len() {
            let line = entries[i];
            i += 1;
            if line.is_empty() {
                continue;
            }
            let kind = line.as_bytes()[0];
            if kind == b'u' {
                items.push((
                    "unstaged",
                    "M",
                    line.splitn(11, ' ').last().unwrap_or("").to_owned(),
                ));
            } else if kind == b'1' || kind == b'2' {
                let parts = line
                    .splitn(if kind == b'2' { 10 } else { 9 }, ' ')
                    .collect::<Vec<_>>();
                if parts.len() < 3 {
                    continue;
                }
                if kind == b'2' {
                    i += 1;
                }
                let xy = parts[1].as_bytes();
                let name = parts.last().unwrap().to_string();
                for (n, stage) in [(0, "staged"), (1, "unstaged")] {
                    if xy.get(n).is_some_and(|v| *v != b'.') {
                        let letter = match xy[n] {
                            b'A' => "A",
                            b'D' => "D",
                            _ => "M",
                        };
                        items.push((stage, letter, name.clone()));
                    }
                }
            }
        }
        items.retain(|(_, _, n)| !RESERVED.contains(&n.split('/').next().unwrap_or("")));
        Ok(items)
    }
    /// True when `status` would list nothing. Never builds a diff, so a working tree with a
    /// large diff answers as fast as `git status` does.
    pub async fn clean(&mut self, path: &Path) -> Result<bool> {
        if !self.tracked_changes(path).await?.is_empty() {
            return Ok(false);
        }
        match self.untracked(path).await {
            Ok((mut names, _)) => {
                names.retain(|n| !RESERVED.contains(&n.split('/').next().unwrap_or("")));
                Ok(names.is_empty())
            }
            // More untracked entries than can be listed: certainly not clean.
            Err(refused) if refused.code == "repo_too_large" => Ok(false),
            Err(refused) => Err(refused),
        }
    }
    /// True when tracked files differ from HEAD, staged or not. Untracked files do not
    /// count: Git itself refuses a merge or rebase that would overwrite one.
    pub(super) async fn tracked_dirty(&mut self, path: &Path) -> Result<bool> {
        Ok(!self.tracked_changes(path).await?.is_empty())
    }
    /// A merge or rebase stopped in this checkout, read from Git's own state files: its
    /// kind, the files still in conflict and, for a rebase, the commit and branch it
    /// started from. `null` when neither is in progress. Reads only.
    pub(super) async fn operation(&mut self, path: &Path) -> Result<Value> {
        let found = self
            .run(
                path,
                args(&[
                    "rev-parse",
                    "--path-format=absolute",
                    "--git-path",
                    "MERGE_HEAD",
                    "--git-path",
                    "rebase-merge",
                    "--git-path",
                    "rebase-apply",
                ]),
                false,
                None,
            )
            .await?;
        let places: Vec<PathBuf> = found.text.lines().map(PathBuf::from).collect();
        if !found.ok || places.len() != 3 {
            return Ok(Value::Null);
        }
        // `rebase-apply` without `rebasing` is `git am`, which is not a pull.
        let rebase = if places[1].is_dir() {
            Some(&places[1])
        } else if places[2].join("rebasing").exists() {
            Some(&places[2])
        } else {
            None
        };
        let kind = match (rebase, places[0].is_file()) {
            (Some(_), _) => "rebase",
            (None, true) => "merge",
            (None, false) => return Ok(Value::Null),
        };
        let listed = self
            .out(path, &["diff", "--name-only", "--diff-filter=U", "-z"])
            .await?;
        let conflicts: Vec<&str> = listed.split('\0').filter(|n| !n.is_empty()).collect();
        let mut found = json!({"kind":kind,"conflicts":conflicts});
        if let Some(dir) = rebase {
            let read = |name: &str| {
                fs::read_to_string(dir.join(name))
                    .ok()
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
            };
            found["origHead"] = json!(read("orig-head"));
            found["branch"] = json!(read("head-name").map(|h| h
                .strip_prefix("refs/heads/")
                .map(str::to_owned)
                .unwrap_or(h)));
            found["onto"] = json!(read("onto"));
        }
        Ok(found)
    }
    pub async fn status(&mut self, path: &Path) -> Result<Vec<Value>> {
        let mut items = self.tracked_changes(path).await?;
        items.extend(
            self.untracked(path)
                .await?
                .0
                .into_iter()
                .map(|name| ("untracked", "U", name)),
        );
        items.retain(|(_, _, n)| !RESERVED.contains(&n.split('/').next().unwrap_or("")));
        let filters = self
            .filters(
                path,
                &items.iter().map(|(_, _, n)| n.clone()).collect::<Vec<_>>(),
            )
            .await?;
        let mut scope = args(&["--", "."]);
        scope.extend(filters.keys().map(|n| format!(":(literal,exclude){n}")));
        let mut plain_args = args(&["diff", "--unified=3"]);
        plain_args.extend(scope.clone());
        let mut cached_args = args(&["diff", "--cached", "--unified=3"]);
        cached_args.extend(scope.clone());
        // Hunks are shown only within MAX_DIFF; a larger diff is stopped at that bound and
        // the files keep their numstat counts without hunks.
        let plain = self
            .run_bounded(path, plain_args, true, None, MAX_DIFF)
            .await?
            .map(|o| o.text);
        let cached = match plain {
            Some(_) => self
                .run_bounded(path, cached_args, true, None, MAX_DIFF)
                .await?
                .map(|o| o.text),
            None => None,
        };
        let (plain, cached, keep) = match (plain, cached) {
            (Some(p), Some(c)) if p.len() + c.len() <= MAX_DIFF => (p, c, true),
            _ => (String::new(), String::new(), false),
        };
        let mut budget = MAX_DIFF.saturating_sub(plain.len() + cached.len());
        let unstaged = if keep {
            parse_diff(&plain, false)
        } else {
            BTreeMap::new()
        };
        let staged = if keep {
            parse_diff(&cached, true)
        } else {
            BTreeMap::new()
        };
        let mut counts = BTreeMap::new();
        for (prefix, stage) in [
            (args(&["diff", "--numstat", "-z"]), "unstaged"),
            (args(&["diff", "--cached", "--numstat", "-z"]), "staged"),
        ] {
            let mut a = prefix;
            a.extend(scope.clone());
            let text = self.run(path, a, true, None).await?.text;
            for row in text.split('\0') {
                let parts = row.split('\t').collect::<Vec<_>>();
                if parts.len() == 3 {
                    counts.insert(
                        (stage, parts[2].to_owned()),
                        (
                            parts[0].parse::<u64>().unwrap_or(0),
                            parts[1].parse::<u64>().unwrap_or(0),
                        ),
                    );
                }
            }
        }
        let mut result = Vec::new();
        for (stage, letter, name) in items {
            let mut record = if stage == "staged" {
                staged.get(&name)
            } else {
                unstaged.get(&name)
            }
            .cloned()
            .unwrap_or_else(|| json!({"add":0,"del":0,"hunks":[]}));
            if stage == "untracked" && !filters.contains_key(&name) {
                let file = path.join(&name);
                if fs::symlink_metadata(&file).is_ok_and(|m| {
                    m.is_file() && !m.file_type().is_symlink() && m.len() <= MAX_DIFF as u64
                }) {
                    let data = fs::read(&file)?;
                    if let Ok(text) = std::str::from_utf8(&data) {
                        if !text.contains('\0') && !text.is_empty() {
                            let lines = text.lines().collect::<Vec<_>>();
                            record["add"] = json!(lines.len());
                            if keep && data.len() <= budget {
                                budget -= data.len();
                                record["hunks"] = json!([{"header":format!("@@ -0,0 +1,{} @@",lines.len()),"staged":false,
                                    "lines":lines.iter().enumerate().map(|(i,l)|json!({"kind":"add","old":null,"new":i+1,"text":l})).collect::<Vec<_>>()}]);
                            }
                        }
                    }
                }
            }
            if let Some((add, del)) = counts.get(&(stage, name.clone())) {
                record["add"] = json!(add);
                record["del"] = json!(del);
            }
            record["path"] = json!(name);
            record["status"] = json!(letter);
            record["stage"] = json!(stage);
            if let Some(filter) = filters.get(&name) {
                record["filter"] = json!(filter);
            }
            result.push(record);
        }
        Ok(result)
    }
    pub(super) async fn default_branch(&mut self, root: &Path) -> Result<String> {
        let head = self
            .run(
                root,
                args(&["symbolic-ref", "--short", "refs/remotes/origin/HEAD"]),
                false,
                None,
            )
            .await?
            .text;
        if !head.trim().is_empty() {
            return Ok(head
                .trim()
                .split_once('/')
                .map_or(head.trim(), |(_, b)| b)
                .to_owned());
        }
        let branches = self
            .out(
                root,
                &["for-each-ref", "--format=%(refname:short)", "refs/heads"],
            )
            .await?;
        let names = branches.lines().collect::<Vec<_>>();
        Ok(["main", "master"]
            .into_iter()
            .find(|n| names.contains(n))
            .or_else(|| names.first().copied())
            .unwrap_or("")
            .to_owned())
    }
    async fn commits(&mut self, root: &Path) -> Result<Vec<Value>> {
        if !self
            .success(root, &["rev-parse", "--verify", "--quiet", "HEAD"])
            .await?
        {
            return Ok(Vec::new());
        }
        let upstream = self
            .success(root, &["rev-parse", "--verify", "--quiet", "@{u}"])
            .await?;
        let unpushed = if upstream {
            self.out(root, &["rev-list", "@{u}..HEAD"]).await?
        } else {
            String::new()
        };
        let text = self
            .out(
                root,
                &[
                    "log",
                    "-n",
                    "100",
                    "--date=relative",
                    "--format=%x1e%H%x00%h%x00%s%x00%an%x00%ar%x00%D",
                ],
            )
            .await?;
        // Commits are immutable: their `--shortstat` is computed once per repository and
        // commit, never again for each snapshot (it diffs every listed commit).
        let hashes = text
            .split('\x1e')
            .filter_map(|b| b.split('\0').next())
            .filter(|h| !h.trim().is_empty())
            .map(|h| h.trim().to_owned())
            .collect::<Vec<_>>();
        let stats = self.commit_stats(root, &hashes).await?;
        let mut rows = Vec::new();
        for block in text.split('\x1e').filter(|b| !b.trim().is_empty()) {
            let (head, _) = block.split_once('\n').unwrap_or((block, ""));
            let f = head.split('\0').collect::<Vec<_>>();
            if f.len() != 6 {
                continue;
            }
            let (files, add, del) = stats.get(f[0]).copied().unwrap_or_default();
            let tags = f[5]
                .split(", ")
                .filter_map(|r| r.strip_prefix("tag: "))
                .collect::<Vec<_>>();
            let branch = f[5]
                .split(", ")
                .find(|r| !r.is_empty() && !r.starts_with("tag: ") && !r.starts_with("origin/"))
                .unwrap_or("")
                .rsplit(" -> ")
                .next()
                .unwrap_or("");
            rows.push(json!({"sha":f[1],"message":f[2],"author":f[3],"when":f[4],"branch":branch,"taskId":null,"tags":tags,
                "files":files,"add":add,"del":del,"pushed":upstream&&!unpushed.lines().any(|l|l==f[0]),
                "graph":{"paths":[{"cls":"g1","d":"M 12 0 V 34"}],"dot":{"cls":"d1","x":12}}}));
        }
        Ok(rows)
    }
    /// `(files, insertions, deletions)` of each commit as `git log --shortstat` reports
    /// it, from the bounded per-repository cache; only commits not seen before are
    /// diffed, split over a few parallel `git log --no-walk` reads.
    async fn commit_stats(
        &mut self,
        root: &Path,
        hashes: &[String],
    ) -> Result<BTreeMap<String, (u64, u64, u64)>> {
        let (mut found, missing) = snapshot_reuse::known_stats(root, hashes);
        if missing.is_empty() {
            return Ok(found);
        }
        let lanes = missing.len().min(8);
        let mut jobs = Vec::new();
        for lane in 0..lanes {
            let mut a = args(&[
                "log",
                "--no-walk=unsorted",
                "--format=%x1e%H",
                "--shortstat",
            ]);
            // Round robin: the newest (often largest) commits land in different reads.
            a.extend(missing.iter().skip(lane).step_by(lanes).cloned());
            a.push("--".into());
            let full = self.prepared(root, a).await?;
            let git = self.clone();
            let path = root.to_path_buf();
            jobs.push(tokio::spawn(async move {
                git.raw_bounded(&path, &full, None, workspace_protocol::FRAME_BYTES)
                    .await
            }));
        }
        let mut computed = BTreeMap::new();
        for job in jobs {
            let out = job
                .await
                .map_err(|_| refuse("git_failed", "Commit statistics could not be read."))??
                .ok_or_else(too_large)?;
            if !out.ok {
                return Err(refuse("git_failed","Git refused this operation. Inspect the checkout in Git; no automatic retry was made."));
            }
            for block in out.text.split('\x1e').filter(|b| !b.trim().is_empty()) {
                let (hash, tail) = block.split_once('\n').unwrap_or((block, ""));
                computed.insert(hash.trim().to_owned(), shortstat(tail));
            }
        }
        // A requested commit absent from the reply is shown without counts and not kept.
        snapshot_reuse::keep_stats(root, &computed);
        for hash in missing {
            found.insert(
                hash.clone(),
                computed.get(&hash).copied().unwrap_or_default(),
            );
        }
        Ok(found)
    }
    /// True only when the `diff --unified=3 <revisions> -- .` of a task (filtered paths
    /// excluded) is certainly larger than `MAX_DIFF`, decided by a cheaper bounded read.
    /// Rename detection pairs only added and deleted paths, so the patch of each path
    /// modified on both sides appears unchanged in the full diff: those patches alone,
    /// read with `--no-renames` (no rename detection over a large history) and stopped at
    /// the same bound, already exceeding it settles the answer. False, so the full read
    /// decides, whenever they fit or cannot be read.
    async fn diff_certainly_over(&mut self, path: &Path, revisions: &[String]) -> Result<bool> {
        let mut a = args(&[
            "diff",
            "--name-only",
            "-z",
            "--no-renames",
            "--diff-filter=M",
        ]);
        a.extend(revisions.iter().cloned());
        a.extend(args(&["--", "."]));
        let Some(names) = self
            .run_bounded(path, a, false, None, workspace_protocol::FRAME_BYTES)
            .await?
            .filter(|o| o.ok)
        else {
            return Ok(false);
        };
        let names = names
            .text
            .split('\0')
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if names.is_empty() {
            return Ok(false);
        }
        let filters = self.filters(path, &names).await?;
        let mut a = args(&["diff", "--unified=3", "--no-renames", "--diff-filter=M"]);
        a.extend(revisions.iter().cloned());
        a.extend(args(&["--", "."]));
        a.extend(filters.keys().map(|n| format!(":(literal,exclude){n}")));
        Ok(self
            .run_bounded(path, a, false, None, MAX_DIFF)
            .await?
            .is_none())
    }
    /// A repeated snapshot with unchanged task context and filesystem signals within
    /// `task_facts::RECENT` uses the bounded reuse slot. Git snapshots also key on
    /// refs, worktree list, fetch marker and origin; a Git-changing command ends reuse.
    pub async fn snapshot(
        &mut self,
        root: &Path,
        project: &str,
        state: &Path,
        context: Option<&Value>,
    ) -> Result<Value> {
        let product = match context {
            Some(value) => value.clone(),
            None => product(state, project)?,
        };
        let slot = snapshot_reuse::slot(root, project);
        let mut held = slot.lock().await;
        // The key is taken before the reads: a change during them moves the next key.
        let key = self.snapshot_key(root, project, &product).await?;
        if let (Some(key), Some(seen)) = (&key, held.as_ref()) {
            if seen.key == *key && seen.at.elapsed() < super::task_facts::RECENT {
                return Ok(seen.value.clone());
            }
        }
        *held = None;
        let at = std::time::Instant::now();
        let value = self.snapshot_fresh(root, project, &product).await?;
        if let Some(key) = key {
            *held = Some(snapshot_reuse::Seen {
                key,
                at,
                value: value.clone(),
            });
        }
        Ok(value)
    }
    /// The cheap reuse key of a snapshot, or None when a filesystem signal is
    /// unavailable (for example, when a file changed just now).
    async fn snapshot_key(
        &mut self,
        root: &Path,
        project: &str,
        product: &Value,
    ) -> Result<Option<String>> {
        use sha2::Digest;
        use super::task_facts::{plain_folder_signal, PlainSignal};

        let generation = snapshot_reuse::generation();
        let is_repo = self.is_repo(root).await?;
        let mut h = sha2::Sha256::new();
        // Only what `snapshot_fresh` reads of the task context: a new message, comment
        // or task timestamp does not end reuse.
        let tasks = product["tasks"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.iter())
            .map(|(task, r)| {
                let gone = r["checkout"]["path"]
                    .as_str()
                    .is_some_and(|p| !Path::new(p).exists());
                (
                    task.clone(),
                    json!([
                        r["checkout"],
                        r["baseRevision"],
                        r["changesBaseRevision"],
                        r["integration"]["worktreeCleanup"],
                        r["integration"]["sourceHead"],
                        gone
                    ]),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        for part in [
            format!("{generation}\0{project}\0{}", root.display()),
            serde_json::to_string(&json!([tasks, product["drafts"]])).unwrap_or_default(),
        ] {
            h.update(part.as_bytes());
            h.update(b"\0\x1e");
        }
        if !is_repo {
            let root = root.to_owned();
            let signal = tokio::task::spawn_blocking(move || {
                plain_folder_signal(&root, PlainSignal::Snapshot)
            })
            .await
            .ok()
            .flatten();
            return Ok(signal.map(|signal| {
                h.update(b"plain-folder\0");
                h.update(signal.as_bytes());
                format!("{:x}", h.finalize())
            }));
        }
        let refs = self
            .out(
                root,
                &[
                    "for-each-ref",
                    "--format=%(refname)%00%(objectname)%00%(symref)%00%(upstream)",
                ],
            )
            .await?;
        let trees = self.out(root, &["worktree", "list", "--porcelain"]).await?;
        let remote = self
            .run(root, args(&["remote", "get-url", "origin"]), false, None)
            .await?
            .text;
        let marker = self
            .run(
                root,
                args(&["rev-parse", "--git-path", "FETCH_HEAD"]),
                false,
                None,
            )
            .await?
            .text;
        let fetched = fs::metadata(root.join(marker.trim()))
            .ok()
            .and_then(|m| m.modified().ok());
        for part in [refs, trees.clone(), remote, format!("{fetched:?}")] {
            h.update(part.as_bytes());
            h.update(b"\0\x1e");
        }
        // Every checkout is signalled at once: each costs a few listings and stats.
        let signals = trees
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
            .map(|path| {
                let path = PathBuf::from(path);
                tokio::task::spawn_blocking(move || {
                    if path.is_dir() {
                        super::task_facts::folder_signal(&path)
                    } else {
                        Some("absent".to_owned())
                    }
                })
            })
            .collect::<Vec<_>>();
        for signal in signals {
            let Some(signal) = signal.await.ok().flatten() else {
                return Ok(None);
            };
            h.update(signal.as_bytes());
            h.update(b"\0");
        }
        Ok(Some(format!("{:x}", h.finalize())))
    }
    async fn snapshot_fresh(
        &mut self,
        root: &Path,
        project: &str,
        product: &Value,
    ) -> Result<Value> {
        let main = format!("{project}:main");
        let mut snapshot = json!({"projectId":project,"hasGit":false,"remote":"","tree":[],"treeTruncated":false,"sources":{},
            "commits":[],"branches":[],"checkouts":[],"taskDiffs":{},"fetchedAt":null,"openBranch":"","defaultBranch":""});
        if !self.is_repo(root).await? {
            let tree = folder_tree(root)?;
            snapshot["treeTruncated"] = json!(tree.len() > MAX_TREE);
            snapshot["tree"] = json!(tree.into_iter().take(MAX_TREE).collect::<Vec<_>>());
            snapshot["checkouts"] = json!([checkout(
                &main,
                project,
                "main",
                "",
                root,
                None,
                "",
                "",
                0,
                0,
                vec![],
                ""
            )]);
            snapshot["checkouts"][0]["label"] = json!("Project folder");
            for task in product["tasks"]
                .as_object()
                .into_iter()
                .flat_map(|m| m.keys())
            {
                snapshot["taskDiffs"][task] = json!({"files":[],"reason":"This folder has no Git history. Review files in Code; result acceptance is still available."});
            }
            return Ok(snapshot);
        }
        snapshot["hasGit"] = json!(true);
        // The commit list (statistics of commits not seen before are its slow part) is
        // read alongside the checkouts; it only reads, and only from `root`.
        let commits = {
            let mut git = self.clone();
            let root = root.to_path_buf();
            tokio::spawn(async move { git.commits(&root).await })
        };
        let main_branch = self
            .out(root, &["branch", "--show-current"])
            .await?
            .trim()
            .to_owned();
        let target = self.default_branch(root).await?;
        snapshot["openBranch"] = json!(main_branch);
        snapshot["defaultBranch"] = json!(target);
        snapshot["remote"] = json!(clean_remote(
            &self
                .run(root, args(&["remote", "get-url", "origin"]), false, None)
                .await?
                .text
        ));
        let refs = self
            .out(
                root,
                &[
                    "for-each-ref",
                    "--format=%(refname:short)%00%(upstream:short)%00%(upstream:track,nobracket)",
                    "refs/heads",
                ],
            )
            .await?;
        let mut upstream = BTreeMap::new();
        let mut branches = Vec::new();
        for line in refs.lines() {
            let f = line.split('\0').collect::<Vec<_>>();
            if f.len() < 3 {
                continue;
            }
            let ahead = track(f[2], "ahead");
            let behind = track(f[2], "behind");
            upstream.insert(f[0].to_owned(), (f[1].to_owned(), ahead, behind));
            branches
                .push(json!({"name":f[0],"where":f[1],"task":null,"ahead":ahead,"behind":behind}));
        }
        snapshot["branches"] = json!(branches);
        let trees = self.trees(root).await?;
        let mut checkouts = Vec::new();
        // One snapshot inspects each checkout once and each task baseline once: tasks
        // of one folder, often several on the same base revision, share the reads.
        let mut statuses: BTreeMap<PathBuf, Vec<Value>> = BTreeMap::new();
        let mut task_diffs: BTreeMap<(PathBuf, Vec<String>, bool), Value> = BTreeMap::new();
        for (i, t) in trees.iter().enumerate() {
            let path = Path::new(t["path"].as_str().unwrap_or(""));
            let branch = t["branch"].as_str().unwrap_or("");
            let id = checkout_id(project, &path.to_string_lossy(), i == 0);
            let task = product["tasks"].as_object().and_then(|m| {
                m.iter()
                    .find(|(_, r)| r["checkout"]["path"] == t["path"])
                    .map(|(k, _)| k.as_str())
            });
            let (up, mut ahead, mut behind) = upstream.get(branch).cloned().unwrap_or_default();
            let files = if path.is_dir() {
                let found = self.status(path).await?;
                statuses.insert(path.to_path_buf(), found.clone());
                found
            } else {
                Vec::new()
            };
            if up.is_empty() && !target.is_empty() && !branch.is_empty() && branch != target {
                let count = self
                    .run(
                        // Branch refs are shared by every worktree; a deleted (prunable) worktree
                        // folder must not stop the snapshot.
                        root,
                        vec![
                            "rev-list".into(),
                            "--left-right".into(),
                            "--count".into(),
                            format!("{target}...{branch}"),
                        ],
                        false,
                        None,
                    )
                    .await?;
                if count.ok {
                    let f = count.text.split_whitespace().collect::<Vec<_>>();
                    if f.len() == 2 {
                        behind = f[0].parse().unwrap_or(0);
                        ahead = f[1].parse().unwrap_or(0);
                    }
                }
            }
            // A merge or rebase stopped here (a pull's, a task update's or the owner's own in
            // the terminal), so every screen shows it and offers its way out.
            let operation = if path.is_dir() {
                self.operation(path).await?
            } else {
                Value::Null
            };
            let mut record = checkout(
                &id,
                project,
                if i == 0 { "main" } else { "worktree" },
                branch,
                path,
                task,
                if up.is_empty() { &target } else { &up },
                &up,
                ahead,
                behind,
                files,
                product["drafts"][&id].as_str().unwrap_or(""),
            );
            record["operation"] = operation;
            checkouts.push(record);
        }
        snapshot["checkouts"] = json!(checkouts);
        // Target updates move only the Changes baseline, not the original task base
        // used for attribution and integration ancestry.
        for (task, record) in product["tasks"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.iter())
        {
            let Some(base) = record["changesBaseRevision"]
                .as_str()
                .filter(|s| valid_sha(s))
                .or_else(|| record["baseRevision"].as_str())
            else {
                continue;
            };
            let removed = record["integration"]["worktreeCleanup"] == "removed"
                && record["checkout"]["path"]
                    .as_str()
                    .is_some_and(|p| !Path::new(p).exists());
            let mut path = root;
            if !removed && record["checkout"]["kind"] == "worktree" {
                let candidate = record["checkout"]["path"].as_str().unwrap_or("");
                if !trees
                    .iter()
                    .any(|t| t["path"] == candidate && t["branch"] == record["checkout"]["branch"])
                {
                    return Err(refuse(
                        "unknown_checkout",
                        "The task worktree is missing or its branch changed.",
                    ));
                }
                path = Path::new(candidate);
            }
            let mut revisions = vec![base.to_owned()];
            if removed {
                revisions.push(
                    record["integration"]["sourceHead"]
                        .as_str()
                        .filter(|s| valid_sha(s))
                        .ok_or_else(|| {
                            refuse(
                                "unknown_checkout",
                                "Removed worktree has no retained source revision.",
                            )
                        })?
                        .to_owned(),
                );
            }
            let key = (path.to_path_buf(), revisions.clone(), removed);
            if let Some(seen) = task_diffs.get(&key) {
                snapshot["taskDiffs"][task] = seen.clone();
                continue;
            }
            if self.diff_certainly_over(path, &revisions).await? {
                let value = json!({"files":[],"reason":"The task diff exceeds the display limit; review it in Git."});
                snapshot["taskDiffs"][task] = value.clone();
                task_diffs.insert(key, value);
                continue;
            }
            let mut a = vec!["diff".into(), "--unified=3".into()];
            a.extend(revisions.clone());
            let mut names_args = args(&["diff", "--name-only", "-z"]);
            names_args.extend(revisions);
            let names = self.run(path, names_args, true, None).await?.text;
            let filters = self
                .filters(
                    path,
                    &names
                        .split('\0')
                        .filter(|n| !n.is_empty())
                        .map(str::to_owned)
                        .collect::<Vec<_>>(),
                )
                .await?;
            a.extend(args(&["--", "."]));
            a.extend(filters.keys().map(|n| format!(":(literal,exclude){n}")));
            let Some(diff) = self
                .run_bounded(path, a, true, None, MAX_DIFF)
                .await?
                .map(|o| o.text)
            else {
                let value = json!({"files":[],"reason":"The task diff exceeds the display limit; review it in Git."});
                snapshot["taskDiffs"][task] = value.clone();
                task_diffs.insert(key, value);
                continue;
            };
            let mut files = parse_diff(&diff, false)
                .into_iter()
                .map(|(name, mut data)| {
                    data["path"] = json!(name);
                    data["status"] = json!("M");
                    data
                })
                .collect::<Vec<_>>();
            let status = match statuses.get(path) {
                Some(seen) => seen.clone(),
                None => {
                    let found = self.status(path).await?;
                    statuses.insert(path.to_path_buf(), found.clone());
                    found
                }
            };
            for mut file in status {
                if !removed && file["stage"] == "untracked" {
                    file.as_object_mut().unwrap().remove("stage");
                    files.push(file);
                }
            }
            let value = json!({"files":files});
            snapshot["taskDiffs"][task] = value.clone();
            task_diffs.insert(key, value);
        }
        let (untracked, collapsed) = self.untracked(root).await?;
        let tracked = self.out(root, &["ls-files", "-z", "--cached"]).await?;
        let tree: BTreeSet<_> = tracked
            .split('\0')
            .map(str::to_owned)
            .chain(untracked)
            .filter(|n| {
                !n.is_empty()
                    && !collapsed.contains(n)
                    && !RESERVED.contains(&n.split('/').next().unwrap_or(""))
            })
            .collect();
        snapshot["treeTruncated"] = json!(tree.len() > MAX_TREE);
        snapshot["tree"] = json!(tree.into_iter().take(MAX_TREE).collect::<Vec<_>>());
        snapshot["commits"] = json!(commits
            .await
            .map_err(|_| refuse("git_failed", "The commit list could not be read."))??);
        let marker = self
            .run(
                root,
                args(&["rev-parse", "--git-path", "FETCH_HEAD"]),
                false,
                None,
            )
            .await?
            .text;
        if !marker.trim().is_empty() {
            if let Ok(m) = fs::metadata(root.join(marker.trim())) {
                if let Ok(t) = m.modified() {
                    snapshot["fetchedAt"] = json!(t
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as u64);
                }
            }
        }
        while serde_json::to_vec(&snapshot)
            .map_err(|_| refuse("repo_too_large", "Invalid repository snapshot."))?
            .len()
            > FRAME_BUDGET
        {
            let mut removed = false;
            for c in snapshot["checkouts"].as_array_mut().unwrap() {
                for f in c["files"].as_array_mut().unwrap() {
                    if f["hunks"].as_array().is_some_and(|h| !h.is_empty()) {
                        f["hunks"] = json!([]);
                        removed = true;
                    }
                }
            }
            if removed {
                continue;
            }
            let tree = snapshot["tree"].as_array_mut().unwrap();
            if tree.len() < 100 {
                return Err(refuse(
                    "repo_too_large",
                    "This project is too large to read in one pass.",
                ));
            }
            tree.truncate(tree.len() / 2);
            snapshot["treeTruncated"] = json!(true);
        }
        Ok(snapshot)
    }
    pub async fn configured_remote(&mut self, path: &Path, selected: &str) -> Result<String> {
        let text = self.out(path, &["remote"]).await?;
        let names = text.lines().collect::<Vec<_>>();
        if !selected.starts_with('-') && names.contains(&selected) {
            return Ok(selected.to_owned());
        }
        let mut found = Vec::new();
        if !selected.is_empty() {
            for name in names {
                let url = self.out(path, &["remote", "get-url", "--", name]).await?;
                if clean_remote(&url) == selected {
                    found.push(name);
                }
            }
        }
        if found.len() > 1 {
            return Err(refuse(
                "ambiguous_remote",
                "More than one remote has that URL. Push by remote name in a terminal.",
            ));
        }
        if found.len() != 1 || found[0].starts_with('-') {
            return Err(refuse("unknown_remote", "That remote is not configured."));
        }
        Ok(found[0].to_owned())
    }
    async fn hunk(
        &mut self,
        path: &Path,
        name: &str,
        index: usize,
        cached: bool,
    ) -> Result<Vec<u8>> {
        let mut a = args(&["diff", "--unified=3"]);
        if cached {
            a.push("--cached".into());
        }
        a.extend(args(&["--", name]));
        let diff = self.run(path, a, true, None).await?.text;
        let Some((header, body)) = diff.split_once("\n@@") else {
            return Err(refuse(
                "stale_hunk",
                "That change moved. Look again and retry.",
            ));
        };
        let body = format!("@@{body}");
        let hunks = body.split("\n@@").collect::<Vec<_>>();
        let hunk = hunks
            .get(index)
            .ok_or_else(|| refuse("stale_hunk", "That change moved. Look again and retry."))?;
        Ok(format!(
            "{header}\n{}{}\n",
            if index == 0 { "" } else { "@@" },
            hunk.trim_end_matches('\n')
        )
        .into_bytes())
    }
    /// Pull into the checkout: `ff` (the default) only fast-forwards; `merge` and
    /// `rebase` bring in remote commits the local branch lacks, as the owner chose. Never
    /// stashes, resets or force-pushes; a pull that stops at a conflict stays stopped for
    /// the owner, with Continue and an exact Abort.
    async fn pull(&mut self, path: &Path, p: &Value) -> Result<()> {
        let mode = p["mode"].as_str().unwrap_or("ff");
        if !matches!(mode, "ff" | "merge" | "rebase") {
            return Err(refuse(
                "unsupported_action",
                "Choose merge or rebase for this pull.",
            ));
        }
        if !self.operation(path).await?.is_null() {
            return Err(refuse("operation_in_progress", OPERATION_OPEN));
        }
        if self
            .out(path, &["branch", "--show-current"])
            .await?
            .trim()
            .is_empty()
        {
            return Err(refuse("detached_checkout", "This checkout is not on a branch (detached HEAD), so there is nothing to pull into. Check out a branch first."));
        }
        if !self
            .success(path, &["rev-parse", "--verify", "--quiet", "@{u}"])
            .await?
        {
            return Err(refuse(
                "upstream_required",
                "No upstream is configured. Configure a remote and tracking branch before pulling.",
            ));
        }
        if mode == "ff" {
            return self.checked(path, args(&["pull", "--ff-only"])).await;
        }
        // An abort restores HEAD, the index and every tracked file exactly only when none
        // of them had uncommitted changes; Git keeps untracked files and refuses to
        // overwrite one.
        if self.tracked_dirty(path).await? {
            return Err(refuse("dirty_checkout", "This checkout has uncommitted changes. Commit them first; Cortex does not stash, and a merge or rebase over uncommitted changes could not be aborted exactly. Nothing was pulled."));
        }
        // Command-line options win over pull.rebase, pull.ff and the autostash settings.
        let argv: &[&str] = if mode == "merge" {
            &["pull", "--no-rebase", "--ff", "--no-edit", "--no-autostash"]
        } else {
            &["pull", "--rebase", "--no-autostash"]
        };
        let out = self.run(path, args(argv), false, None).await?;
        let stopped = self.operation(path).await?;
        if !stopped.is_null() {
            return Err(stopped_at(&stopped));
        }
        if !out.ok {
            return Err(failure("pull", &out.error));
        }
        Ok(())
    }
    /// A merge a task's update or integration left in its checkout is finished or aborted
    /// from the task, which keeps its record in step.
    async fn held_by_task(
        &mut self,
        root: &Path,
        project: &str,
        state: &Path,
        path: &Path,
    ) -> Result<()> {
        let saved = product(state, project)?;
        let here = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        for record in saved["tasks"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.values())
        {
            let retained = record["merge"].is_object()
                && record["merge"]["finished"] != true
                && matches!(
                    record["conflict"].as_str(),
                    Some("conflict" | "resolving" | "failed")
                );
            let at = match record["checkout"]["kind"].as_str() {
                Some("worktree") => record["checkout"]["path"]
                    .as_str()
                    .map(|p| fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p))),
                _ => Some(root.clone()),
            };
            if retained && at.as_deref() == Some(here.as_path()) {
                return Err(refuse(
                    "task_merge",
                    format!(
                        "This merge belongs to the task “{}”. Finish or abort it from the task.",
                        record["title"].as_str().unwrap_or("")
                    ),
                ));
            }
        }
        Ok(())
    }
    /// Back to exactly where the checkout was before the pull: HEAD, the index and the
    /// tracked files. Checked afterwards; anything else is reported, never retried.
    async fn pull_abort(&mut self, path: &Path, p: &Value) -> Result<()> {
        let operation = self.operation(path).await?;
        if operation.is_null() {
            return Err(refuse("no_operation", NO_OPERATION));
        }
        if p["confirmed"] != true {
            return Err(refuse(
                "confirmation_required",
                "Confirm before aborting the pull.",
            ));
        }
        let head = self.out(path, &["rev-parse", "HEAD"]).await?;
        let (expected, branch) = if operation["kind"] == "rebase" {
            let orig = operation["origHead"]
                .as_str()
                .filter(|s| valid_sha(s))
                .ok_or_else(|| refuse("git_state_unknown", "Git's record of where this rebase started is missing, so Cortex cannot check an abort. Inspect it in the terminal (Ctrl+J)."))?
                .to_owned();
            self.checked(path, args(&["rebase", "--abort"])).await?;
            (orig, operation["branch"].as_str().map(str::to_owned))
        } else {
            self.checked(path, args(&["merge", "--abort"])).await?;
            (head.trim().to_owned(), None)
        };
        let now = self.out(path, &["rev-parse", "HEAD"]).await?;
        let on = self.out(path, &["branch", "--show-current"]).await?;
        if now.trim() != expected
            || branch.is_some_and(|b| b != on.trim())
            || !self.operation(path).await?.is_null()
            || self.tracked_dirty(path).await?
        {
            return Err(refuse("git_state_unknown", "Git aborted, but this checkout is not exactly where it was before the pull. Inspect it in the terminal (Ctrl+J); nothing else was changed or retried."));
        }
        Ok(())
    }
    /// Finish a stopped pull once every conflicted file is resolved and marked resolved:
    /// the merge commit, or the rest of the rebase, which may stop again at its next
    /// conflict.
    async fn pull_continue(&mut self, path: &Path) -> Result<()> {
        let operation = self.operation(path).await?;
        if operation.is_null() {
            return Err(refuse("no_operation", NO_OPERATION));
        }
        let left = operation["conflicts"].as_array().map_or(0, Vec::len);
        if left > 0 {
            return Err(refuse(
                "unresolved_conflict",
                format!(
                    "{left} file{} still in conflict. Resolve {}, mark {} resolved, then continue.",
                    if left == 1 { " is" } else { "s are" },
                    if left == 1 { "it" } else { "them" },
                    if left == 1 { "it" } else { "them" }
                ),
            ));
        }
        // Only leftover markers block; whitespace that the remote commits brought is theirs.
        let check = self
            .run(path, args(&["diff", "--cached", "--check"]), false, None)
            .await?;
        if check.text.contains("leftover conflict marker") {
            return Err(refuse("conflict_markers", "A file marked resolved still contains conflict markers (<<<<<<<, =======, >>>>>>>). Edit it, mark it resolved again, then continue."));
        }
        if operation["kind"] == "merge" {
            let head = self.out(path, &["rev-parse", "HEAD"]).await?;
            let merge_head = self.out(path, &["rev-parse", "MERGE_HEAD"]).await?;
            self.checked(
                path,
                args(&["-c", "core.editor=true", "commit", "--no-edit"]),
            )
            .await?;
            let parents = self
                .out(path, &["show", "-s", "--format=%P", "HEAD"])
                .await?;
            // Hooks stay active; success alone cannot show a hook did not replace HEAD.
            if parents.split_whitespace().collect::<Vec<_>>()
                != vec![head.trim(), merge_head.trim()]
                || !self.operation(path).await?.is_null()
            {
                return Err(refuse("git_state_unknown", "Git made the merge commit, but it is not the commit Cortex expected. Inspect the checkout in the terminal (Ctrl+J); nothing was retried."));
            }
            return Ok(());
        }
        let out = self
            .run(
                path,
                args(&["-c", "core.editor=true", "rebase", "--continue"]),
                false,
                None,
            )
            .await?;
        let again = self.operation(path).await?;
        if !again.is_null() {
            if again["conflicts"].as_array().is_some_and(|c| !c.is_empty()) {
                return Err(stopped_at(&again));
            }
            return Err(refuse("rebase_stopped", "Git stopped the rebase again without a conflict, for example because a commit became empty. Inspect it in the terminal (Ctrl+J), or abort the pull."));
        }
        if !out.ok {
            return Err(failure("rebase", &out.error));
        }
        Ok(())
    }
    pub async fn action(
        &mut self,
        root: &Path,
        project: &str,
        state: &Path,
        p: &Value,
    ) -> Result<Value> {
        let action = string(p, "action")?;
        // The owner's own action, Git or not (a draft, an untracked file removed), is
        // never followed by a reused snapshot from before it.
        let _invalidate = snapshot_reuse::OnDrop;
        // Slow external Git effects must not hold the product/configuration writer lock.
        let git_lock = repo_lock(root);
        // Network and hook-running actions name themselves: a second Git action on this
        // project is then refused at once with that reason instead of waiting.
        let _lock = match action {
            "fetch" | "pull" | "push" => Lock::long(state, &git_lock, "git_in_progress",
                "A fetch, pull or push of this project is still running. Try again when it finishes.")?,
            "commit" | "revert_commit" | "git_init" => Lock::long(state, &git_lock, "git_in_progress",
                "A commit in this project is still running. Try again when it finishes.")?,
            "pull_continue" | "pull_abort" => Lock::long(state, &git_lock, "git_in_progress",
                "A pull of this project is still being continued or aborted. Try again when it finishes.")?,
            _ => Lock::new(state, &git_lock)?,
        };
        if action == "git_init" {
            if self.is_repo(root).await? || fs::symlink_metadata(root.join(".git")).is_ok() {
                return Err(refuse(
                    "repository_exists",
                    "This project already has a Git repository.",
                ));
            }
            self.checked(root, args(&["init"])).await?;
            return Ok(json!({"done":true,"hasGit":true}));
        }
        if !self.is_repo(root).await? {
            return Err(refuse("git_required","This folder has no Git repository. Initialize Git to use Git operations; tasks and review do not require it."));
        }
        if action == "fetch" {
            self.checked(root, args(&["fetch", "--prune"])).await?;
            return Ok(json!({"done":true}));
        }
        if action == "branch" {
            let name = string(p, "name")?;
            if !valid_branch(name)
                || name.starts_with('-')
                || !self
                    .success(root, &["check-ref-format", "--branch", name])
                    .await?
            {
                return Err(refuse("invalid_branch", "That is not a valid branch name."));
            }
            let mut a = args(&["branch", name]);
            if let Some(at) = p["at"].as_str().filter(|s| valid_sha(s)) {
                a.push(at.into());
            }
            self.checked(root, a).await?;
            return Ok(json!({"done":true}));
        }
        let wanted = p["checkoutId"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| format!("{project}:main"));
        let path = self.resolve(root, project, &wanted).await?;
        match action {
            "draft" => {
                let _state_lock = Lock::new(state, "machine.lock")?;
                let mut product = product(state, project)?;
                product["drafts"][&wanted] = json!(p["message"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(8000)
                    .collect::<String>());
                settings::write_json(&product_path(state, project)?, &product)?;
            }
            "stage" => {
                let name = relative(string(p, "path")?)?;
                if let Some(h) = p["hunk"].as_u64() {
                    let staged = p["staged"] == true;
                    let patch = self.hunk(&path, name, h as usize, !staged).await?;
                    let mut a = args(&["apply", "--cached"]);
                    if !staged {
                        a.push("-R".into());
                    }
                    self.run(&path, a, true, Some(patch)).await?;
                } else {
                    self.checked(
                        &path,
                        if p["staged"] == true {
                            args(&["add", "--", name])
                        } else {
                            args(&["restore", "--staged", "--", name])
                        },
                    )
                    .await?;
                }
            }
            "revert" => {
                if p["confirmed"] != true {
                    return Err(refuse(
                        "confirmation_required",
                        "Confirm before discarding changes.",
                    ));
                }
                let name = relative(string(p, "path")?)?;
                if let Some(h) = p["hunk"].as_u64() {
                    let patch = self.hunk(&path, name, h as usize, false).await?;
                    self.run(&path, args(&["apply", "-R"]), true, Some(patch))
                        .await?;
                } else if !self
                    .success(&path, &["ls-files", "--error-unmatch", "--", name])
                    .await?
                {
                    // unlink the link itself, never follow an untracked symlink.
                    let target = path.join(name);
                    let parent = fs::canonicalize(target.parent().unwrap())?;
                    if !parent.starts_with(fs::canonicalize(&path)?) {
                        return Err(refuse(
                            "invalid_path",
                            "That path is not inside this checkout.",
                        ));
                    }
                    if fs::symlink_metadata(&target).is_ok_and(|m| !m.is_dir()) {
                        fs::remove_file(target)?;
                    }
                } else {
                    self.checked(
                        &path,
                        args(&[
                            "restore",
                            "--source=HEAD",
                            "--staged",
                            "--worktree",
                            "--",
                            name,
                        ]),
                    )
                    .await?;
                }
            }
            "commit" => {
                let staged = self
                    .out(&path, &["diff", "--cached", "--name-only", "-z"])
                    .await?;
                if staged
                    .split('\0')
                    .any(|n| RESERVED.contains(&n.split('/').next().unwrap_or("")))
                {
                    return Err(refuse(
                        "private_index",
                        "Unstage Workspace helper data before committing.",
                    ));
                }
                let message = string(p, "message")?;
                if message.trim().is_empty() {
                    return Err(refuse("empty_message", "Write a commit message first."));
                }
                if staged.is_empty() {
                    let names: BTreeSet<_> = self
                        .status(&path)
                        .await?
                        .iter()
                        .filter_map(|f| f["path"].as_str().map(str::to_owned))
                        .collect();
                    if !names.is_empty() {
                        let mut a = args(&["add", "-A", "--"]);
                        // Literal pathspecs preserve filenames containing glob/pathspec syntax.
                        for name in names {
                            relative(&name)?;
                            a.push(format!(":(literal){name}"));
                        }
                        self.checked(&path, a).await?;
                    }
                }
                let mut a = args(&["commit", "-m", message]);
                if p["amend"] == true {
                    a.push("--amend".into());
                }
                self.checked(&path, a).await?;
                let _state_lock = Lock::new(state, "machine.lock")?;
                let mut product = product(state, project)?;
                product["drafts"].as_object_mut().unwrap().remove(&wanted);
                settings::write_json(&product_path(state, project)?, &product)?;
            }
            "pull" => self.pull(&path, p).await?,
            "pull_continue" | "pull_abort" => {
                self.held_by_task(root, project, state, &path).await?;
                if action == "pull_abort" {
                    self.pull_abort(&path, p).await?;
                } else {
                    self.pull_continue(&path).await?;
                }
            }
            "push" => {
                let branch = p["branch"].as_str().unwrap_or("");
                if p["confirmed"] != true || !valid_branch(branch) {
                    return Err(refuse(
                        "confirmation_required",
                        "Confirm the remote and branch first.",
                    ));
                }
                let remote = self
                    .configured_remote(&path, p["remote"].as_str().unwrap_or(""))
                    .await?;
                self.checked(&path, args(&["push", "--", &remote, branch]))
                    .await?;
            }
            "revert_commit" => {
                let sha = p["sha"].as_str().unwrap_or("");
                if p["confirmed"] != true || !valid_sha(sha) {
                    return Err(refuse(
                        "confirmation_required",
                        "Confirm before reverting a commit.",
                    ));
                }
                if !self
                    .success(&path, &["diff", "--cached", "--quiet"])
                    .await?
                {
                    return Err(refuse(
                        "dirty_index",
                        "Commit or unstage your selected changes before reverting a commit.",
                    ));
                }
                self.checked(&path, args(&["revert", "--no-commit", sha]))
                    .await?;
                self.checked(
                    &path,
                    args(&["commit", "-m", &format!("Revert commit {sha}")]),
                )
                .await?;
            }
            "delete_worktree" => {
                if wanted == format!("{project}:main") || p["confirmed"] != true {
                    return Err(refuse(
                        "confirmation_required",
                        "Only a confirmed separate worktree can be deleted.",
                    ));
                }
                if !self.clean(&path).await? {
                    return Err(refuse(
                        "dirty_worktree",
                        "Commit or move the work yourself first; cleanup never forces deletion.",
                    ));
                }
                self.checked(
                    root,
                    args(&["worktree", "remove", "--", &path.to_string_lossy()]),
                )
                .await?;
            }
            _ => {
                return Err(refuse(
                    "unsupported_action",
                    "This machine does not support that yet.",
                ))
            }
        }
        Ok(json!({"done":true}))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn git(root: &Path, a: &[&str]) {
        assert!(std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(a)
            .current_dir(root)
            .output()
            .unwrap()
            .status
            .success());
    }
    /// A failing Git command's stderr, as the service runs Git (C locale, no prompts).
    fn refused_stderr(root: &Path, a: &[&str]) -> String {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(a)
            .current_dir(root)
            .env("LC_ALL", "C")
            .env("GIT_TERMINAL_PROMPT", "0")
            .output()
            .unwrap();
        assert!(!out.status.success(), "{a:?} succeeded");
        String::from_utf8_lossy(&out.stderr).into_owned()
    }
    /// Push and pull refusals name their cause and the next step from real Git output, never the output itself.
    #[test]
    fn git_refusals_are_classified_from_real_git_output() {
        let base = std::env::temp_dir().join(format!("cw-git-refusals-{}", settings::unique()));
        let origin = base.join("origin.git");
        let (a, b) = (base.join("a"), base.join("b"));
        fs::create_dir_all(&base).unwrap();
        git(
            &base,
            &[
                "init",
                "-q",
                "--bare",
                "-b",
                "main",
                origin.to_str().unwrap(),
            ],
        );
        for clone in [&a, &b] {
            git(
                &base,
                &[
                    "clone",
                    "-q",
                    origin.to_str().unwrap(),
                    clone.to_str().unwrap(),
                ],
            );
            git(clone, &["checkout", "-q", "-B", "main"]);
        }
        fs::write(a.join("a.txt"), "a\n").unwrap();
        git(&a, &["add", "."]);
        git(&a, &["commit", "-q", "-m", "a"]);
        git(&a, &["push", "-q", "-u", "origin", "main"]);
        git(&b, &["pull", "-q", "origin", "main"]);
        git(&b, &["branch", "-q", "--set-upstream-to=origin/main"]);
        // Both sides move on: the histories diverge.
        fs::write(a.join("a.txt"), "a2\n").unwrap();
        git(&a, &["commit", "-q", "-am", "a2"]);
        git(&a, &["push", "-q"]);
        fs::write(b.join("b.txt"), "b\n").unwrap();
        git(&b, &["add", "."]);
        git(&b, &["commit", "-q", "-m", "b"]);
        let push = failure(
            "push",
            &refused_stderr(&b, &["push", "--", "origin", "main"]),
        );
        assert_eq!(push.code, "push_not_fast_forward", "{}", push.message);
        assert!(
            push.message.contains("Nothing was pushed") && push.message.contains("terminal"),
            "{}",
            push.message
        );
        git(&b, &["fetch", "-q"]);
        let pull = failure("pull", &refused_stderr(&b, &["pull", "--ff-only"]));
        assert_eq!(pull.code, "pull_not_fast_forward", "{}", pull.message);
        assert!(pull.message.contains("diverged"), "{}", pull.message);
        git(&b, &["checkout", "-q", "-b", "loose"]);
        let upstream = failure("pull", &refused_stderr(&b, &["pull", "--ff-only"]));
        assert_eq!(upstream.code, "upstream_required", "{}", upstream.message);
        // A server-side hook refusal, and one that reports a protected branch as GitHub does.
        let hook = origin.join("hooks/pre-receive");
        for (script, code) in [
            ("#!/bin/sh\necho 'policy says no' >&2\nexit 1\n", "git_hook_rejected"),
            ("#!/bin/sh\necho 'GH006: Protected branch update failed for refs/heads/main.' >&2\nexit 1\n", "push_protected_branch"),
        ] {
            fs::write(&hook, script).unwrap();
            fs::set_permissions(&hook, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            fs::write(a.join("a.txt"), format!("{code}\n")).unwrap();
            git(&a, &["commit", "-q", "-am", code]);
            let refused = failure("push", &refused_stderr(&a, &["push", "--", "origin", "main"]));
            assert_eq!(refused.code, code, "{}", refused.message);
            assert!(!refused.message.contains("policy says no") && !refused.message.contains("GH006"));
        }
        let gone = failure(
            "push",
            &refused_stderr(
                &a,
                &[
                    "push",
                    "--",
                    base.join("missing.git").to_str().unwrap(),
                    "main",
                ],
            ),
        );
        assert_eq!(gone.code, "remote_unreachable", "{}", gone.message);
        // Authentication needs a real server; these are Git's own messages for it.
        for text in [
            "remote: Invalid username or password.\nfatal: Authentication failed for 'https://example.invalid/o/r.git/'\n",
            "git@example.invalid: Permission denied (publickey).\nfatal: Could not read from remote repository.\n",
            "fatal: could not read Username for 'https://example.invalid': terminal prompts disabled\n",
        ] {
            let auth = failure("fetch", text);
            assert_eq!(auth.code, "git_auth_failed", "{text}");
            assert!(!auth.message.contains("example.invalid"));
        }
        assert_eq!(
            failure("commit", "error: something else\n").code,
            "git_failed"
        );
        // Outside push, pull and fetch nothing is classified, whatever the output says.
        for verb in ["commit", "merge", "revert", "status"] {
            assert_eq!(
                failure(
                    verb,
                    "remote: pre-receive hook declined\nfatal: Authentication failed\n"
                )
                .code,
                "git_failed"
            );
        }
        fs::remove_dir_all(base).unwrap();
    }
    /// The snapshot without the relative commit times (`when`, e.g. "3 seconds ago"):
    /// two separate reads of an unchanged checkout differ only there as the clock moves.
    fn timeless(snapshot: &Value) -> Value {
        match snapshot {
            Value::Object(map) => Value::Object(
                map.iter()
                    .filter(|(k, _)| k.as_str() != "when")
                    .map(|(k, v)| (k.clone(), timeless(v)))
                    .collect(),
            ),
            Value::Array(items) => Value::Array(items.iter().map(timeless).collect()),
            other => other.clone(),
        }
    }
    /// A worktree whose folder the owner deleted (`git worktree list` marks it prunable) must
    /// not fail the whole snapshot, and must never be reported as "Git is not installed".
    #[tokio::test]
    async fn a_deleted_worktree_folder_keeps_the_snapshot_and_is_not_missing_git() {
        let base = std::env::temp_dir().join(format!("cw-gone-tree-{}", settings::unique()));
        let root = base.join("repo");
        let gone = base.join("gone");
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        git(
            &root,
            &[
                "worktree",
                "add",
                "-q",
                "-b",
                "side",
                gone.to_str().unwrap(),
            ],
        );
        fs::remove_dir_all(&gone).unwrap();
        let state = base.join("state");
        fs::create_dir_all(&state).unwrap();
        let mut g = Git::new();
        let snapshot = g
            .snapshot(&root, "p", &state, Some(&json!({"tasks":{}})))
            .await
            .unwrap();
        assert_eq!(
            snapshot["checkouts"].as_array().unwrap().len(),
            2,
            "{snapshot}"
        );
        let refused = g.out(&gone, &["status"]).await.unwrap_err();
        assert_eq!(refused.code, "checkout_missing");
        fs::remove_dir_all(base).unwrap();
    }
    /// Tasks of one folder on one base revision share a single diff and status read,
    /// and each still shows the whole task diff, untracked files included.
    #[tokio::test]
    async fn tasks_on_one_base_share_the_snapshot_reads() {
        let base = std::env::temp_dir().join(format!("cw-shared-base-{}", settings::unique()));
        let root = base.join("repo");
        fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        let head = String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(&root)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_owned();
        fs::write(root.join("a.txt"), "a\nb\n").unwrap();
        fs::write(root.join("new.txt"), "n\n").unwrap();
        let state = base.join("state");
        fs::create_dir_all(&state).unwrap();
        let task = |kind: &str| json!({"baseRevision":head,"checkout":{"kind":kind}});
        let product = json!({"tasks":{"t1":task("main"),"t2":task("main"),"t3":{"checkout":{"kind":"main"}}}});
        let snapshot = Git::new()
            .snapshot(&root, "p", &state, Some(&product))
            .await
            .unwrap();
        let diffs = &snapshot["taskDiffs"];
        assert_eq!(diffs["t1"], diffs["t2"], "{snapshot}");
        assert!(diffs.get("t3").is_none(), "{snapshot}");
        let paths: BTreeSet<_> = diffs["t1"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["path"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            paths,
            BTreeSet::from(["a.txt".to_owned(), "new.txt".to_owned()])
        );
        assert_eq!(
            snapshot["checkouts"][0]["files"].as_array().unwrap().len(),
            2,
            "{snapshot}"
        );
        fs::remove_dir_all(base).unwrap();
    }
    fn head(root: &Path) -> String {
        let out = std::process::Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root)
            .output()
            .unwrap();
        String::from_utf8(out.stdout).unwrap().trim().to_owned()
    }
    fn fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = std::env::temp_dir().join(format!("cw-{tag}-{}", settings::unique()));
        let root = base.join("repo");
        let state = base.join("state");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&state).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        (base, root, state)
    }
    fn task_on(base: &str) -> Value {
        json!({"tasks":{"t1":{"baseRevision":base,"checkout":{"kind":"main"}}}})
    }
    const OVER: &str = "The task diff exceeds the display limit; review it in Git.";
    /// A task diff whose modified paths alone exceed the display limit is decided from
    /// that cheaper read, stopped at the bound: the full diff (with rename detection) is
    /// never run, and nothing is read past the bound.
    #[tokio::test]
    async fn an_over_limit_task_diff_is_decided_without_reading_it() {
        let (base, root, state) = fixture("over-limit");
        let lines = 120_000;
        let body = |tag: &str| {
            (0..lines)
                .map(|i| format!("{tag}{i}\n"))
                .collect::<String>()
        };
        fs::write(root.join("big.txt"), body("o")).unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        let start = head(&root);
        fs::write(root.join("big.txt"), body("n")).unwrap();
        git(&root, &["commit", "-q", "-am", "rewrite"]);
        let snapshot = Git::new()
            .snapshot(&root, "p", &state, Some(&task_on(&start)))
            .await
            .unwrap();
        assert_eq!(
            snapshot["taskDiffs"]["t1"],
            json!({"files":[],"reason":OVER}),
            "{snapshot}"
        );
        let reads = snapshot_reuse::reads(&root);
        let task_reads = |renames: bool| {
            reads
                .iter()
                .filter(|(a, _)| a.contains(&"--unified=3".to_owned()) && a.contains(&start))
                .filter(|(a, _)| a.contains(&"--no-renames".to_owned()) != renames)
                .map(|r| r.1)
                .collect::<Vec<_>>()
        };
        assert_eq!(task_reads(true), Vec::<usize>::new(), "{reads:?}");
        assert_eq!(task_reads(false), vec![MAX_DIFF + 1], "{reads:?}");
        assert!(
            reads.iter().all(|(_, bytes)| *bytes <= MAX_DIFF + 1),
            "{reads:?}"
        );
        fs::remove_dir_all(base).unwrap();
    }
    /// A diff the cheaper read cannot rule out (here an added file) is read only up to
    /// the display limit: the read stops (the child is killed) at the bound, same reason.
    #[tokio::test]
    async fn a_task_diff_read_stops_at_the_display_limit() {
        let (base, root, state) = fixture("stop-at-limit");
        fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        let start = head(&root);
        fs::write(root.join("big.txt"), "x\n".repeat(300_000)).unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "big"]);
        let snapshot = Git::new()
            .snapshot(&root, "p", &state, Some(&task_on(&start)))
            .await
            .unwrap();
        assert_eq!(
            snapshot["taskDiffs"]["t1"],
            json!({"files":[],"reason":OVER}),
            "{snapshot}"
        );
        let reads = snapshot_reuse::reads(&root);
        let full = reads
            .iter()
            .filter(|(a, _)| a.contains(&"--unified=3".to_owned()) && a.contains(&start))
            .filter(|(a, _)| !a.contains(&"--no-renames".to_owned()))
            .collect::<Vec<_>>();
        assert_eq!(full.len(), 1, "{reads:?}");
        assert_eq!(full[0].1, MAX_DIFF + 1, "{reads:?}");
        fs::remove_dir_all(base).unwrap();
    }
    /// Commit counts equal `git log --shortstat` (root, rename and merge commits
    /// included) and are computed once per commit: a repeated list reruns no statistics,
    /// and a new commit computes only its own.
    #[tokio::test]
    async fn commit_statistics_are_computed_once_per_commit() {
        let (base, root, _state) = fixture("commit-stats");
        fs::write(root.join("a.txt"), "a\nb\nc\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "root"]);
        git(&root, &["mv", "a.txt", "b.txt"]);
        git(&root, &["commit", "-q", "-m", "rename"]);
        git(&root, &["checkout", "-q", "-b", "side"]);
        fs::write(root.join("s.txt"), "s\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "side"]);
        git(&root, &["checkout", "-q", "main"]);
        fs::write(root.join("m.txt"), "m\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "main"]);
        git(&root, &["merge", "-q", "--no-ff", "-m", "merge", "side"]);
        git(&root, &["commit", "-q", "--allow-empty", "-m", "empty"]);
        let old = std::process::Command::new("git")
            .args([
                "log",
                "--no-ext-diff",
                "--no-textconv",
                "-n",
                "100",
                "--format=%x1e%H",
                "--shortstat",
            ])
            .current_dir(&root)
            .output()
            .unwrap();
        let expected = String::from_utf8(old.stdout)
            .unwrap()
            .split('\x1e')
            .filter(|b| !b.trim().is_empty())
            .map(|b| {
                let (hash, tail) = b.split_once('\n').unwrap_or((b, ""));
                let (files, add, del) = shortstat(tail);
                (hash[..7].to_owned(), json!([files, add, del]))
            })
            .collect::<Vec<_>>();
        let counted = |rows: &[Value]| {
            rows.iter()
                .map(|r| {
                    (
                        r["sha"].as_str().unwrap()[..7].to_owned(),
                        json!([r["files"], r["add"], r["del"]]),
                    )
                })
                .collect::<Vec<_>>()
        };
        let stat_reads = || {
            snapshot_reuse::reads(&root)
                .into_iter()
                .filter(|(a, _)| a.contains(&"--shortstat".to_owned()))
                .map(|(a, _)| a)
                .collect::<Vec<_>>()
        };
        let mut g = Git::new();
        let first = g.commits(&root).await.unwrap();
        assert_eq!(counted(&first), expected);
        assert!(
            expected.iter().any(|(_, c)| c == &json!([0, 0, 0])),
            "{expected:?}"
        );
        let before = stat_reads().len();
        assert!(before >= 1);
        // `when` is the commit's age in words, so it may tick between the two reads.
        let timeless = |mut rows: Vec<Value>| {
            for r in &mut rows {
                r.as_object_mut().unwrap().remove("when");
            }
            rows
        };
        assert_eq!(
            timeless(g.commits(&root).await.unwrap()),
            timeless(first.clone())
        );
        assert_eq!(
            stat_reads().len(),
            before,
            "known commits were diffed again"
        );
        git(&root, &["commit", "-q", "--allow-empty", "-m", "next"]);
        let next = head(&root);
        let again = g.commits(&root).await.unwrap();
        assert_eq!(again.len(), first.len() + 1);
        let reads = stat_reads();
        assert_eq!(reads.len(), before + 1, "{reads:?}");
        let hashes = reads[before]
            .iter()
            .filter(|a| a.len() == 40)
            .collect::<Vec<_>>();
        assert_eq!(hashes, vec![&next], "{reads:?}");
        fs::remove_dir_all(base).unwrap();
    }
    /// An unchanged checkout is served from the reuse slot without rereading Git status,
    /// diffs, the tree or the log; the owner's own action through the service ends reuse at once.
    #[tokio::test]
    async fn an_unchanged_snapshot_is_reused_until_an_owner_action() {
        snapshot_reuse::isolated(check_snapshot_reuse()).await;
    }

    #[tokio::test]
    async fn plain_folder_snapshot_reuses_tree_until_metadata_changes() {
        snapshot_reuse::isolated(async {
            let (base, root, state) = fixture("plain-reuse");
            fs::remove_dir_all(root.join(".git")).unwrap();
            fs::write(root.join("first"), "a").unwrap();
            let dependency = root.join("node_modules/dep");
            fs::create_dir_all(&dependency).unwrap();
            tokio::time::sleep(Duration::from_millis(3100)).await;
            let context = json!({"tasks":{}});
            let mut git = Git::new();
            let first = git.snapshot(&root, "p", &state, Some(&context)).await.unwrap();
            assert_eq!(first["tree"], json!(["first"]));

            // Reusing a distinct cache entry proves folder_tree was not used again.
            let slot = snapshot_reuse::slot(&root, "p");
            slot.lock().await.as_mut().unwrap().value["tree"] = json!(["cached"]);
            fs::write(dependency.join("ignored"), "not in the snapshot").unwrap();
            let second = git.snapshot(&root, "p", &state, Some(&context)).await.unwrap();
            assert_eq!(second["tree"], json!(["cached"]));

            fs::rename(root.join("first"), root.join("second")).unwrap();
            let changed = git.snapshot(&root, "p", &state, Some(&context)).await.unwrap();
            assert_eq!(changed["tree"], json!(["second"]));
            fs::remove_dir_all(base).unwrap();
        })
        .await;
    }

    async fn check_snapshot_reuse() {
        let (base, root, state) = fixture("reuse");
        fs::write(root.join("a.txt"), "a\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        let start = head(&root);
        fs::write(root.join("a.txt"), "a\nb\n").unwrap();
        fs::write(root.join("new.txt"), "n\n").unwrap();
        // `folder_signal` refuses to vouch for anything changed in the last two seconds.
        tokio::time::sleep(Duration::from_millis(3100)).await;
        let context = task_on(&start);
        let status_reads = || {
            snapshot_reuse::reads(&root)
                .iter()
                .filter(|(a, _)| a.contains(&"status".to_owned()))
                .count()
        };
        let mut g = Git::new();
        let first = g
            .snapshot(&root, "p", &state, Some(&context))
            .await
            .unwrap();
        let fresh = status_reads();
        assert!(fresh > 0);
        let second = g
            .snapshot(&root, "p", &state, Some(&context))
            .await
            .unwrap();
        assert_eq!(second, first);
        assert_eq!(
            status_reads(),
            fresh,
            "an unchanged snapshot was read again"
        );
        // A changed task context is not served from reuse.
        let other = json!({"tasks":{}});
        let changed = g.snapshot(&root, "p", &state, Some(&other)).await.unwrap();
        assert_ne!(timeless(&changed), timeless(&first));
        assert!(status_reads() > fresh);
        // A fresh read, so its relative commit times may have moved on since `first`.
        let reread = g
            .snapshot(&root, "p", &state, Some(&context))
            .await
            .unwrap();
        assert_eq!(timeless(&reread), timeless(&first));
        let reused = status_reads();
        // Served from reuse: byte-identical to the read it reuses, times included.
        assert_eq!(
            g.snapshot(&root, "p", &state, Some(&context))
                .await
                .unwrap(),
            reread
        );
        assert_eq!(status_reads(), reused);
        // An owner action that changes nothing the key sees still ends reuse...
        g.action(&root, "p", &state, &json!({"action":"draft","message":"m"}))
            .await
            .unwrap();
        g.snapshot(&root, "p", &state, Some(&context))
            .await
            .unwrap();
        assert!(
            status_reads() > reused,
            "a snapshot from before an owner action was reused"
        );
        // ...and staging shows at once.
        g.action(
            &root,
            "p",
            &state,
            &json!({"action":"stage","path":"a.txt","staged":true}),
        )
        .await
        .unwrap();
        let staged = g
            .snapshot(&root, "p", &state, Some(&context))
            .await
            .unwrap();
        let stages = staged["checkouts"][0]["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| {
                (
                    f["path"].as_str().unwrap().to_owned(),
                    f["stage"].as_str().unwrap().to_owned(),
                )
            })
            .collect::<Vec<_>>();
        assert!(
            stages.contains(&("a.txt".to_owned(), "staged".to_owned())),
            "{staged}"
        );
        fs::remove_dir_all(base).unwrap();
    }
    /// A working tree whose diff is larger than a frame must neither stall until the Git
    /// timeout nor fail the snapshot: hunks are dropped, counts stay, and the cleanliness
    /// check used by archive never builds the diff at all.
    #[tokio::test]
    async fn oversized_diff_degrades_promptly() {
        let root = std::env::temp_dir().join(format!("cw-big-diff-{}", settings::unique()));
        fs::create_dir(&root).unwrap();
        git(&root, &["init", "-q"]);
        let lines = workspace_protocol::FRAME_BYTES / 40 + 1000;
        let body = |tag: &str| {
            (0..lines)
                .map(|i| format!("{tag} line {i:08} padding padding\n"))
                .collect::<String>()
        };
        fs::write(root.join("big.txt"), body("old")).unwrap();
        fs::write(root.join("small.txt"), "a\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        fs::write(root.join("big.txt"), body("new")).unwrap();
        let started = std::time::Instant::now();
        let mut g = Git::new();
        assert!(!g.clean(&root).await.unwrap());
        let files = g.status(&root).await.unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0]["path"], "big.txt");
        assert_eq!(files[0]["add"], lines as u64);
        assert_eq!(files[0]["del"], lines as u64);
        assert_eq!(files[0]["hunks"], json!([]));
        let refused = g.out(&root, &["diff", "--unified=3"]).await.unwrap_err();
        assert_eq!(refused.code, "git_output_too_large");
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        git(&root, &["checkout", "--", "big.txt"]);
        assert!(g.clean(&root).await.unwrap());
        fs::write(root.join("new.txt"), "n\n").unwrap();
        assert!(!g.clean(&root).await.unwrap());
        fs::remove_dir_all(root).unwrap();
    }
    #[tokio::test]
    async fn ssh_display_url_resolves_to_live_name_without_network() {
        let root = std::env::temp_dir().join(format!("cw-ssh-resolution-{}", settings::unique()));
        fs::create_dir(&root).unwrap();
        for a in [
            vec!["init"],
            vec![
                "remote",
                "add",
                "origin",
                "git@example.invalid:owner/repo.git",
            ],
        ] {
            assert!(std::process::Command::new("git")
                .args(a)
                .current_dir(&root)
                .output()
                .unwrap()
                .status
                .success());
        }
        assert_eq!(
            Git::new()
                .configured_remote(&root, "git@example.invalid:owner/repo.git")
                .await
                .unwrap(),
            "origin"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
fn valid_branch(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 200
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b))
}
fn valid_sha(sha: &str) -> bool {
    (4..=64).contains(&sha.len())
        && sha
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn clean_remote(url: &str) -> String {
    let text = url.trim();
    if let Some((scheme, rest)) = text.split_once("://") {
        let (authority, tail) = rest.split_once('/').unwrap_or((rest, ""));
        let host = authority.rsplit('@').next().unwrap_or(authority);
        return format!(
            "{scheme}://{host}{}{tail}",
            if rest.contains('/') { "/" } else { "" }
        );
    }
    text.to_owned()
}
fn checkout_id(project: &str, path: &str, main: bool) -> String {
    if main {
        format!("{project}:main")
    } else {
        format!("{project}:wt:{}", &revision(path.as_bytes())[..16])
    }
}
fn track(text: &str, kind: &str) -> u64 {
    text.split_once(&format!("{kind} "))
        .and_then(|(_, s)| s.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok())
        .unwrap_or(0)
}
#[allow(clippy::too_many_arguments)]
fn checkout(
    id: &str,
    project: &str,
    kind: &str,
    branch: &str,
    path: &Path,
    task: Option<&str>,
    base: &str,
    up: &str,
    ahead: u64,
    behind: u64,
    files: Vec<Value>,
    draft: &str,
) -> Value {
    json!({"id":id,"projectId":project,"kind":kind,"branch":branch,"path":path,"taskId":task,"label":null,"base":base,"upstream":up,
        "ahead":ahead,"behind":behind,"behindOf":{},"files":files,"draft":draft,"sources":{},"updateConflict":null})
}
/// Folders the tree of a folder without Git does not walk into. Code lists them on demand, marked ignored.
pub(super) const UNWALKED: [&str; 3] = ["node_modules", ".venv", "__pycache__"];
fn folder_tree(root: &Path) -> Result<Vec<String>> {
    folder_tree_with(root, &mut Vec::new())
}
/// The tree of a folder without Git; `skipped` receives each folder not walked into.
pub(super) fn folder_tree_with(root: &Path, skipped: &mut Vec<String>) -> Result<Vec<String>> {
    let mut pending = vec![root.to_path_buf()];
    let mut tree = Vec::new();
    while let Some(path) = pending.pop() {
        let mut entries = fs::read_dir(path)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let name = e.file_name();
            let name = name.to_string_lossy();
            if RESERVED.contains(&name.as_ref()) {
                continue;
            }
            let meta = e.file_type()?;
            if meta.is_dir() && !UNWALKED.contains(&name.as_ref()) {
                pending.push(e.path());
            } else if meta.is_dir() {
                skipped.push(
                    e.path()
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            } else {
                tree.push(
                    e.path()
                        .strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            if tree.len() > MAX_TREE {
                tree.sort();
                return Ok(tree);
            }
        }
    }
    tree.sort();
    Ok(tree)
}
pub fn parse_diff(text: &str, staged: bool) -> BTreeMap<String, Value> {
    let mut files = BTreeMap::<String, Value>::new();
    let mut current = None::<String>;
    let mut previous = None::<String>;
    let mut hunk = None::<usize>;
    let mut old = 0u64;
    let mut new = 0u64;
    for line in text.split('\n') {
        if line.starts_with("diff --git ") {
            current = None;
            previous = None;
            hunk = None;
        } else if let Some(name) = line.strip_prefix("+++ b/") {
            current = Some(name.split('\t').next().unwrap_or(name).to_owned());
        } else if let Some(name) = line.strip_prefix("--- a/") {
            previous = Some(name.split('\t').next().unwrap_or(name).to_owned());
        } else if line.starts_with("@@ ") {
            let parts = line.split_whitespace().collect::<Vec<_>>();
            if parts.len() < 4 {
                continue;
            }
            let Some(a) = parts[1]
                .strip_prefix('-')
                .and_then(|s| s.split(',').next()?.parse::<u64>().ok())
            else {
                continue;
            };
            let Some(b) = parts[2]
                .strip_prefix('+')
                .and_then(|s| s.split(',').next()?.parse::<u64>().ok())
            else {
                continue;
            };
            let Some(key) = current.clone().or_else(|| previous.clone()) else {
                continue;
            };
            let record = files
                .entry(key.clone())
                .or_insert_with(|| json!({"hunks":[],"add":0,"del":0}));
            let hunks = record["hunks"].as_array_mut().unwrap();
            hunks.push(json!({"header":line,"lines":[],"staged":staged}));
            hunk = Some(hunks.len() - 1);
            current = Some(key);
            old = a;
            new = b;
        } else if let (Some(key), Some(index)) = (&current, hunk) {
            if line.is_empty() || ![" ", "+", "-"].contains(&&line[..1]) {
                continue;
            }
            let kind = match line.as_bytes()[0] {
                b'+' => "add",
                b'-' => "del",
                _ => "ctx",
            };
            let record = files.get_mut(key).unwrap();
            record["hunks"][index]["lines"]
                .as_array_mut()
                .unwrap()
                .push(
                    json!({"kind":kind,"old":if kind=="add"{None}else{Some(old)},
                "new":if kind=="del"{None}else{Some(new)},"text":&line[1..]}),
                );
            if kind != "add" {
                old += 1;
            }
            if kind != "del" {
                new += 1;
            }
            if kind != "ctx" {
                record[kind] = json!(record[kind].as_u64().unwrap_or(0) + 1);
            }
        }
    }
    files
}
/// `(files, insertions, deletions)` from one `--shortstat` line; zeros when absent.
fn shortstat(tail: &str) -> (u64, u64, u64) {
    let mut files = 0;
    let mut add = 0;
    let mut del = 0;
    for field in tail.trim().split(',') {
        let n = field
            .split_whitespace()
            .next()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        if field.contains("file") {
            files = n;
        } else if field.contains("insertion") {
            add = n;
        } else if field.contains("deletion") {
            del = n;
        }
    }
    (files, add, del)
}
/// The per-repository lock every Git-changing owner action of a project takes:
/// Git actions here, integration and merge finish/abort. Changes of one index and
/// checkout set never interleave.
pub(crate) fn repo_lock(root: &Path) -> String {
    format!(
        "git-{}.lock",
        &revision(root.as_os_str().as_encoded_bytes())[..16]
    )
}
