//! Local Git effects and filesystem witnesses for native task worktrees.
use crate::{error::refuse, CoreError, CoreResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    io::Read,
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt},
        process::CommandExt,
    },
    path::{Component, Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Worktree {
    pub id: String,
    pub path: PathBuf,
    pub branch: String,
    pub base_sha: String,
    common: PathBuf,
    // A recreated checkout starts at the retained tip, not at its comparison base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retained_head: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Witness {
    admin: PathBuf,
    tree: (u64, u64),
    admin_identity: (u64, u64),
    common_identity: (u64, u64),
    gitfile_identity: (u64, u64),
}

fn git(root: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_ALLOW_PROTOCOL", "")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .args([
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "core.fsmonitor=false",
        ])
        .args([
            "-c",
            "core.attributesFile=/dev/null",
            "-c",
            "core.untrackedCache=false",
        ])
        .args([
            "-c",
            "core.sparseCheckout=false",
            "-c",
            "index.sparse=false",
        ])
        .arg("-C")
        .arg(root);
    cmd
}

fn run(cmd: &mut Command) -> CoreResult<Output> {
    const OUTPUT_LIMIT: usize = 4 * 1024 * 1024;
    let mut child = cmd
        .stdout(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|_| CoreError::Refused("worktree_git_unavailable"))?;
    let mut pipe = child.stdout.take().expect("stdout was piped");
    let mut status = None;
    let result = (|| -> CoreResult<Output> {
        use std::os::fd::AsRawFd;
        if unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, libc::O_NONBLOCK) } == -1 {
            return refuse("worktree_git_failed");
        }
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut stdout = Vec::new();
        let mut buf = [0; 8192];
        let mut eof = false;
        loop {
            if !eof {
                match pipe.read(&mut buf) {
                    Ok(0) => eof = true,
                    Ok(n) if stdout.len() + n <= OUTPUT_LIMIT => {
                        stdout.extend_from_slice(&buf[..n])
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    _ => return refuse("worktree_git_failed"),
                }
            }
            if status.is_none() {
                status = child
                    .try_wait()
                    .map_err(|_| CoreError::Refused("worktree_git_failed"))?;
            }
            if let Some(status) = status.filter(|_| eof) {
                return Ok(Output {
                    status,
                    stdout,
                    stderr: Vec::new(),
                });
            }
            if Instant::now() >= deadline {
                return refuse("worktree_git_timeout");
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    })();
    if result.is_err() && status.is_none() {
        // Only this private Git process group and any builtin child.
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.wait();
    }
    result
}

fn checked(cmd: &mut Command) -> CoreResult<Vec<u8>> {
    let out = run(cmd)?;
    if !out.status.success() {
        return refuse("worktree_git_failed");
    }
    Ok(out.stdout)
}

fn line(root: &Path, args: &[&str]) -> CoreResult<String> {
    let bytes = checked(git(root).args(args))?;
    let raw = bytes
        .strip_suffix(b"\n")
        .ok_or(CoreError::Refused("worktree_git_failed"))?;
    let text = std::str::from_utf8(raw).map_err(|_| CoreError::Refused("worktree_git_failed"))?;
    if text.is_empty() || text.contains(['\n', '\r', '\0']) {
        return refuse("worktree_git_failed");
    }
    Ok(text.to_owned())
}

fn slug(text: &str) -> String {
    let mut result = String::new();
    let mut separator = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            if separator && !result.is_empty() {
                result.push('-');
            }
            result.push(c.to_ascii_lowercase());
            if result.len() >= 40 {
                break;
            }
            separator = false;
        } else {
            separator = true;
        }
    }
    if result.is_empty() {
        "task".into()
    } else {
        result
    }
}

fn absent(path: &Path) -> bool {
    path.symlink_metadata()
        .is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

// Walk each prefix, not a caller-provided canonicalization that follows a symlink
// at any missing mkdir component. Check the nearest existing directory for Git.
fn safe_parent(path: &Path, logical: &Path, create: bool) -> CoreResult<()> {
    if !path.is_absolute()
        || path == Path::new("/")
        || path.starts_with(logical)
        || path.to_str().is_none_or(|p| p.contains(['\n', '\r', '\0']))
        || path
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::RootDir))
    {
        return refuse("unsafe_worktree_root");
    }
    let mut current = PathBuf::new();
    let mut existing = PathBuf::new();
    for c in path.components() {
        current.push(c);
        match current.symlink_metadata() {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => existing = current.clone(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            _ => return refuse("unsafe_worktree_root"),
        }
        if current != Path::new("/") && !absent(&current.join(".git")) {
            return refuse("unsafe_worktree_root");
        }
    }
    if run(git(&existing).args(["rev-parse", "--git-dir"]))?
        .status
        .success()
    {
        return refuse("unsafe_worktree_root");
    }
    if create {
        // A provision starts only below a canonical root; verify again after mkdir.
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)
            .map_err(|_| CoreError::Refused("unsafe_worktree_root"))?;
        if path.canonicalize().ok().as_deref() != Some(path) {
            return refuse("unsafe_worktree_root");
        }
    }
    Ok(())
}

pub(crate) fn plan(
    root: &Path,
    machine: &Path,
    project: &str,
    draft: &str,
    title: &str,
) -> CoreResult<Worktree> {
    plan_at(root, machine, project, draft, title, None)
}

pub(super) fn rework_base(root: &Path, target: Option<&str>, in_place: bool) -> CoreResult<String> {
    let Some(target) = target else {
        return line(root, &["rev-parse", "--verify", "HEAD^{commit}"]);
    };
    let reference = format!("refs/heads/{target}");
    checked(git(root).args(["check-ref-format", &reference]))?;
    let head = line(root, &["rev-parse", "--verify", &format!("{reference}^{{commit}}")])
        .map_err(|_| CoreError::Refused("rework_target_unavailable"))?;
    if in_place && line(root, &["symbolic-ref", "--quiet", "HEAD"]).ok().as_deref() != Some(&reference) {
        return refuse("rework_target_not_checked_out");
    }
    Ok(head)
}

pub(super) fn plan_at(
    root: &Path,
    machine: &Path,
    project: &str,
    draft: &str,
    title: &str,
    base: Option<&str>,
) -> CoreResult<Worktree> {
    if machine == Path::new("/") {
        return refuse("unsafe_worktree_root");
    }
    let top = line(root, &["rev-parse", "--show-toplevel"]).map_err(|e| {
        if e.code() == "worktree_git_failed" {
            CoreError::Refused("worktree_git_required")
        } else {
            e
        }
    })?;
    if Path::new(&top) != root || !root.join(".git").is_dir() {
        return refuse("worktree_git_required");
    }
    let base_sha = match base {
        Some(sha) => sha.to_owned(),
        None => line(root, &["rev-parse", "--verify", "HEAD^{commit}"])
            .map_err(|_| CoreError::Refused("worktree_head_unborn"))?,
    };
    if !matches!(base_sha.len(), 40 | 64) || !base_sha.bytes().all(|c| c.is_ascii_hexdigit()) {
        return refuse("worktree_git_failed");
    }
    let common = git_path(root, "--git-common-dir")?;
    if common != root.join(".git")
        || identity(&common, true).is_err()
        || common.canonicalize().ok().as_ref() != Some(&common)
    {
        return refuse("worktree_git_metadata_unsafe");
    }
    let repo = crate::digest(common.as_os_str().as_encoded_bytes());
    let suffix = crate::digest(draft.as_bytes());
    let leaf = format!("{}-{}", slug(title), &suffix[..12]);
    let branch = format!("cw/{leaf}");
    let parent = machine.join(&repo[..16]);
    safe_parent(&parent, root, false)?;
    let path = parent.join(&leaf);
    checked(git(root).args(["check-ref-format", &format!("refs/heads/{branch}")]))?;
    let out = run(git(root).args([
        "show-ref",
        "--verify",
        "--quiet",
        &format!("refs/heads/{branch}"),
    ]))?;
    if !absent(&path) || out.status.success() {
        return refuse("worktree_collision");
    }
    if out.status.code() != Some(1) {
        return refuse("worktree_git_failed");
    }
    let path_digest = crate::digest(path.as_os_str().as_encoded_bytes());
    // Match the machine Git checkout catalogue, without using it as authority.
    let id = format!("{project}:wt:{}", &path_digest[..16]);
    Ok(Worktree {
        id,
        path,
        branch,
        base_sha,
        common,
        retained_head: None,
    })
}

fn git_path(root: &Path, option: &str) -> CoreResult<PathBuf> {
    Ok(PathBuf::from(line(
        root,
        &["rev-parse", "--path-format=absolute", option],
    )?))
}

fn identity(path: &Path, directory: bool) -> CoreResult<(u64, u64)> {
    let m = path
        .symlink_metadata()
        .map_err(|_| CoreError::Refused("worktree_unavailable"))?;
    if if directory {
        !m.is_dir()
    } else {
        !m.is_file() || m.nlink() != 1
    } {
        return refuse("worktree_unavailable");
    }
    Ok((m.dev(), m.ino()))
}

impl Worktree {
    pub(crate) fn projection(&self) -> Value {
        json!({"id":self.id,"kind":"worktree","path":self.path,"branch":self.branch,
               "baseSha":self.base_sha,"createdByWorkspace":true})
    }

    fn observe(&self, logical: &Path) -> CoreResult<Witness> {
        let t = &self.path;
        let gitfile = t.join(".git");
        let tree = identity(t, true)?;
        let gitfile_identity = identity(&gitfile, false)?;
        if t.starts_with(logical)
            || t.canonicalize()? != *t
            || git_path(logical, "--git-common-dir")? != self.common
            || git_path(t, "--git-common-dir")? != self.common
            || Path::new(&line(t, &["rev-parse", "--show-toplevel"])?) != t
            || line(t, &["symbolic-ref", "--quiet", "HEAD"])?
                != format!("refs/heads/{}", self.branch)
        {
            return refuse("worktree_unavailable");
        }
        let admin = git_path(t, "--absolute-git-dir")?;
        if admin.parent() != Some(self.common.join("worktrees").as_path())
            || admin.canonicalize()? != admin
            || fs::read(admin.join("gitdir"))? != format!("{}\n", gitfile.display()).as_bytes()
        {
            return refuse("worktree_unavailable");
        }
        let common_identity = identity(&self.common, true)?;
        let admin_identity = identity(&admin, true)?;
        checked(git(t).args(["merge-base", "--is-ancestor", &self.base_sha, "HEAD"]))?;
        Ok(Witness {
            admin,
            tree,
            admin_identity,
            common_identity,
            gitfile_identity,
        })
    }

    pub(crate) fn verify(&self, logical: &Path, witness: &Witness) -> CoreResult<()> {
        if self.observe(logical).as_ref().ok() != Some(witness) {
            return refuse("worktree_unavailable");
        }
        Ok(())
    }

    fn initial_head(&self) -> &str {
        self.retained_head.as_deref().unwrap_or(&self.base_sha)
    }

    fn branch_head(&self, logical: &Path, witness: &Witness) -> CoreResult<String> {
        if git_path(logical, "--git-common-dir")? != self.common
            || self.common.canonicalize().ok().as_ref() != Some(&self.common)
            || identity(&self.common, true)? != witness.common_identity
        {
            return refuse("worktree_unavailable");
        }
        let reference = format!("refs/heads/{}", self.branch);
        checked(git(logical).args(["check-ref-format", &reference]))?;
        let symbolic = run(git(logical).args(["symbolic-ref", "--quiet", &reference]))?;
        if symbolic.status.code() != Some(1) {
            return refuse("worktree_unavailable");
        }
        let head = line(logical, &["show-ref", "--verify", "--hash", &reference])?;
        checked(git(logical).args(["merge-base", "--is-ancestor", &self.base_sha, &head]))?;
        Ok(head)
    }

    /// Use a new owned directory, but only the branch proved by the old binding
    /// and the exact tip recorded when Workspace removed that checkout.
    pub(super) fn restore_branch(
        mut self,
        logical: &Path,
        old: &Worktree,
        witness: &Witness,
        head: &str,
    ) -> CoreResult<Self> {
        if self.common != old.common
            || !absent(&old.path)
            || !absent(&witness.admin)
            || old.branch_head(logical, witness).as_deref().ok() != Some(head)
        {
            return refuse("worktree_unavailable");
        }
        self.branch = old.branch.clone();
        self.retained_head = Some(head.to_owned());
        Ok(self)
    }

    pub(crate) fn verify_initial(&self, logical: &Path, witness: &Witness) -> CoreResult<()> {
        self.verify(logical, witness)?;
        if line(&self.path, &["rev-parse", "HEAD"])? != self.initial_head() {
            return refuse("worktree_uncertain");
        }
        Ok(())
    }

    pub(crate) fn provision(&self, logical: &Path) -> CoreResult<Witness> {
        safe_parent(
            self.path
                .parent()
                .ok_or(CoreError::Refused("unsafe_worktree_root"))?,
            logical,
            true,
        )?;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(&self.path)
            .map_err(|_| CoreError::Refused("worktree_uncertain"))?;
        let created = identity(&self.path, true)?;
        // No checkout until the linked branch's effective config can be inspected.
        let mut add = git(logical);
        add.args(["worktree", "add", "--no-checkout"]);
        if self.retained_head.is_some() {
            // No force or branch creation: Git refuses a branch checked out
            // elsewhere. Verify the exact checkout and tip below.
            add.arg("--").arg(&self.path).arg(&self.branch);
        } else {
            add.args(["-b", &self.branch, "--"])
                .arg(&self.path)
                .arg(&self.base_sha);
        }
        if let Err(error) = checked(&mut add) {
            // Git can refuse an existing branch checked out elsewhere. Undo
            // only our still-empty reservation, never Git metadata or a branch.
            if self.retained_head.is_some() && identity(&self.path, true).ok() == Some(created) {
                let _ = fs::remove_dir(&self.path);
            }
            return Err(error);
        }
        if self.observe(logical)?.tree != created {
            return refuse("worktree_uncertain");
        }
        no_submounts(&self.path)?;
        let cmd = &mut git(&self.path);
        neutralize_filters(&self.path, cmd)?;
        checked(
            git(&self.path)
                .arg("--work-tree")
                .arg(&self.path)
                .args(["read-tree", self.initial_head()]),
        )?;
        checked(
            cmd.arg("--work-tree")
                .arg(&self.path)
                .args(["checkout-index", "--all"]),
        )?;
        if line(&self.path, &["rev-parse", "HEAD"])? != self.initial_head() {
            return refuse("worktree_uncertain");
        }
        let witness = self.observe(logical)?;
        if witness.tree != created {
            return refuse("worktree_uncertain");
        }
        Ok(witness)
    }
}

// Only Git builtins. Strip inherited config, suppress hooks/fsmonitor and override
// every effective filter key after onbranch/includeIf sees the *new* branch.
fn neutralize_filters(path: &Path, cmd: &mut Command) -> CoreResult<()> {
    let out = run(git(path).args([
        "config",
        "-z",
        "--name-only",
        "--get-regexp",
        r"^filter\..*\.(clean|smudge|process|required)$",
    ]))?;
    if out.status.code() == Some(1) && out.stdout.is_empty() {
        return Ok(());
    }
    if !out.status.success() {
        return refuse("worktree_git_failed");
    }
    for key in out.stdout.split(|b| *b == 0).filter(|k| !k.is_empty()) {
        let key =
            std::str::from_utf8(key).map_err(|_| CoreError::Refused("worktree_git_failed"))?;
        if key.contains(['=', '\n', '\r']) {
            return refuse("worktree_git_failed");
        }
        let value = if key.ends_with(".required") {
            "false"
        } else {
            ""
        };
        cmd.args(["-c", &format!("{key}={value}")]);
    }
    Ok(())
}

impl Worktree {
    pub(super) fn check_remove(&self, logical: &Path, witness: &Witness) -> CoreResult<()> {
        if absent(&self.path) {
            return refuse("worktree_missing");
        }
        self.verify(logical, witness)?;
        no_submounts(&self.path)?;
        const OCCUPIED: &[&str] = &[
            "locked",
            "MERGE_HEAD",
            "CHERRY_PICK_HEAD",
            "REVERT_HEAD",
            "rebase-apply",
            "rebase-merge",
            "sequencer",
            "BISECT_START",
            "index.lock",
            "HEAD.lock",
        ];
        if OCCUPIED
            .iter()
            .any(|name| !absent(&witness.admin.join(name)))
        {
            return refuse("worktree_occupied");
        }
        // Ignored and untracked entries are local work too. Do not let an
        // assume-unchanged or sparse index suppress the cleanliness witness.
        let index = checked(git(&self.path).args(["ls-files", "-v", "-z"]))?;
        let mut cmd = git(&self.path);
        neutralize_filters(&self.path, &mut cmd)?;
        let status = checked(cmd.env("GIT_OPTIONAL_LOCKS", "0").args([
            "status",
            "--porcelain=v1",
            "-z",
            "--ignored=matching",
            "--untracked-files=all",
            "--ignore-submodules=none",
        ]))?;
        if index
            .split(|b| *b == 0)
            .any(|r| !r.is_empty() && !r.starts_with(b"H "))
            || !status.is_empty()
        {
            return refuse("worktree_dirty");
        }
        self.verify(logical, witness)
    }

    /// Compensate only our newly witnessed, unchanged provision. Never force
    /// removal or delete a branch someone advanced after it was prepared.
    pub(super) fn abandon_initial(&self, logical: &Path, witness: &Witness) -> CoreResult<()> {
        self.verify_initial(logical, witness)?;
        self.remove(logical, witness)?;
        if self.retained_head.is_none() {
            checked(git(logical).args([
                "update-ref", "-d", &format!("refs/heads/{}", self.branch), &self.base_sha,
            ]))?;
        }
        Ok(())
    }

    pub(super) fn remove(&self, logical: &Path, witness: &Witness) -> CoreResult<()> {
        self.remove_with_head(logical, witness, false).map(|_| ())
    }

    pub(super) fn remove_with_head(
        &self,
        logical: &Path,
        witness: &Witness,
        keep_head: bool,
    ) -> CoreResult<Option<String>> {
        self.check_remove(logical, witness)?;
        let head = if keep_head {
            Some(self.branch_head(logical, witness)?)
        } else {
            None
        };
        let mut cmd = git(logical);
        neutralize_filters(logical, &mut cmd)?;
        neutralize_filters(&self.path, &mut cmd)?;
        checked(cmd.args(["worktree", "remove", "--"]).arg(&self.path))?;
        if !absent(&self.path) || !absent(&witness.admin) {
            return refuse("worktree_cleanup_unconfirmed");
        }
        // If an external Git writer moved the branch during removal, do not
        // certify either tip as the removed checkout's recoverable state.
        Ok(head.filter(|head| self.branch_head(logical, witness).ok().as_ref() == Some(head)))
    }
}

// Git's non-force cleanup is not a mount boundary. In particular a bind
// mount can have the same device number, so check this namespace's mount table.
fn no_submounts(path: &Path) -> CoreResult<()> {
    let info = fs::read("/proc/self/mountinfo")
        .map_err(|_| CoreError::Refused("worktree_mount_unobservable"))?;
    if info.is_empty() {
        return refuse("worktree_mount_unobservable");
    }
    for line in info.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let encoded = line
            .split(|b| *b == b' ')
            .nth(4)
            .ok_or(CoreError::Refused("worktree_mount_unobservable"))?;
        let mut decoded = Vec::new();
        let mut bytes = encoded.iter().copied();
        while let Some(b) = bytes.next() {
            if b == b'\\' {
                let mut n = 0u16;
                for _ in 0..3 {
                    let v = bytes
                        .next()
                        .filter(|v| (b'0'..=b'7').contains(v))
                        .ok_or(CoreError::Refused("worktree_mount_unobservable"))?;
                    n = n * 8 + u16::from(v - b'0');
                }
                decoded.push(
                    u8::try_from(n)
                        .map_err(|_| CoreError::Refused("worktree_mount_unobservable"))?,
                );
            } else {
                decoded.push(b);
            }
        }
        if Path::new(std::ffi::OsStr::from_bytes(&decoded)).starts_with(path) {
            return refuse("worktree_occupied");
        }
    }
    Ok(())
}
