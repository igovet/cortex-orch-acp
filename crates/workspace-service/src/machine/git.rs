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
const MAX_DIFF: usize = 400000;
const MAX_TREE: usize = 20000;
const MAX_UNTRACKED: usize = 2000;
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
                refuse("checkout_missing", "This checkout folder no longer exists. Prune it in Git (git worktree prune).")
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
        let verb = argv.first().map(String::as_str).unwrap_or("");
        let read = matches!(
            verb,
            "status" | "diff" | "log" | "show" | "ls-files" | "check-attr" | "rev-list"
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
        let Some(out) = self.raw_bounded(path, &full, input, limit).await? else {
            return Ok(None);
        };
        if check && !out.ok {
            // Never return stderr: remote URLs, hook output or credentials can occur there.
            return Err(refuse("git_failed","Git refused this operation. Inspect the checkout in Git; no automatic retry was made."));
        }
        Ok(Some(out))
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
        let out = self
            .run(
                path,
                args(&["rev-parse", "--is-inside-work-tree"]),
                false,
                None,
            )
            .await?;
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
    async fn tracked_changes(&mut self, path: &Path) -> Result<Vec<(&'static str, &'static str, String)>> {
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
                    "--shortstat",
                ],
            )
            .await?;
        let mut rows = Vec::new();
        for block in text.split('\x1e').filter(|b| !b.trim().is_empty()) {
            let (head, tail) = block.split_once('\n').unwrap_or((block, ""));
            let f = head.split('\0').collect::<Vec<_>>();
            if f.len() != 6 {
                continue;
            }
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
                self.status(path).await?
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
            checkouts.push(checkout(
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
            ));
        }
        snapshot["checkouts"] = json!(checkouts);
        // These are saved machine task baselines, not lifecycle/acceptance decisions.
        for (task, record) in product["tasks"]
            .as_object()
            .into_iter()
            .flat_map(|m| m.iter())
        {
            let Some(base) = record["baseRevision"].as_str() else {
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
            let Some(diff) = self.run_bounded(path, a, true, None, MAX_DIFF).await?.map(|o| o.text) else {
                snapshot["taskDiffs"][task] = json!({"files":[],"reason":"The task diff exceeds the display limit; review it in Git."});
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
            for mut file in self.status(path).await? {
                if !removed && file["stage"] == "untracked" {
                    file.as_object_mut().unwrap().remove("stage");
                    files.push(file);
                }
            }
            snapshot["taskDiffs"][task] = json!({"files":files});
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
        snapshot["commits"] = json!(self.commits(root).await?);
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
    pub async fn action(
        &mut self,
        root: &Path,
        project: &str,
        state: &Path,
        p: &Value,
    ) -> Result<Value> {
        let action = string(p, "action")?;
        // Slow external Git effects must not hold the product/configuration writer lock.
        let git_lock = format!(
            "git-{}.lock",
            &revision(root.as_os_str().as_encoded_bytes())[..16]
        );
        let _lock = Lock::new(state, &git_lock)?;
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
            "pull" => {
                if !self
                    .success(&path, &["rev-parse", "--verify", "--quiet", "@{u}"])
                    .await?
                {
                    return Err(refuse("upstream_required","No upstream is configured. Configure a remote and tracking branch before pulling."));
                }
                self.checked(&path, args(&["pull", "--ff-only"])).await?;
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
        git(&root, &["worktree", "add", "-q", "-b", "side", gone.to_str().unwrap()]);
        fs::remove_dir_all(&gone).unwrap();
        let state = base.join("state");
        fs::create_dir_all(&state).unwrap();
        let mut g = Git::new();
        let snapshot = g
            .snapshot(&root, "p", &state, Some(&json!({"tasks":{}})))
            .await
            .unwrap();
        assert_eq!(snapshot["checkouts"].as_array().unwrap().len(), 2, "{snapshot}");
        let refused = g.out(&gone, &["status"]).await.unwrap_err();
        assert_eq!(refused.code, "checkout_missing");
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
        let body = |tag: &str| (0..lines).map(|i| format!("{tag} line {i:08} padding padding\n")).collect::<String>();
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
        assert!(started.elapsed() < Duration::from_secs(10), "{:?}", started.elapsed());
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
fn folder_tree(root: &Path) -> Result<Vec<String>> {
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
            if meta.is_dir() && !["node_modules", ".venv", "__pycache__"].contains(&name.as_ref()) {
                pending.push(e.path());
            } else if !meta.is_dir() {
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
