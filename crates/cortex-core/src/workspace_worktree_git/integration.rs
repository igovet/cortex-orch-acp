//! Bounded Git transport for a witnessed assignment checkout. No index or branch
//! changes on the target. The caller owns its runtime integration reservation.
use super::{checked, git, line, neutralize_filters, run};
use crate::{digest, error::refuse, store::owner_file_leases::RelativePath, CoreError, CoreResult};
use std::{
    collections::BTreeMap,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, Write},
    os::{
        fd::FromRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::Path,
    process::Stdio,
};

type Files = BTreeMap<String, (u32, String)>;
pub(in crate::workspace) const MAX_FILES: usize = 64;

pub(in crate::workspace) struct Transfer {
    patch: Vec<u8>,
    files: Files,
    original: Files,
}

impl Transfer {
    /// Only ordinary tracked files qualify in this transport. Scope and path
    /// safety are checked again before the runtime accepts the applied result.
    pub(in crate::workspace) fn observe(root: &Path, base: &str) -> CoreResult<Self> {
        if Path::new(&line(root, &["rev-parse", "--show-toplevel"])?) != root
            || line(root, &["rev-parse", "--verify", "HEAD^{commit}"])? != base
        {
            return refuse("assignment_worktree_conflict");
        }
        let mut cmd = git(root);
        neutralize_filters(root, &mut cmd)?;
        let untracked = checked(cmd.args(["ls-files", "--others", "--exclude-standard", "-z"]))?;
        if !untracked.is_empty() {
            return refuse("assignment_worktree_untracked_files");
        }
        let raw = diff(root, &["--raw", "-z"])?;
        let patch = diff(
            root,
            &[
                "--binary",
                "--full-index",
                "--src-prefix=a/",
                "--dst-prefix=b/",
            ],
        )?;
        let mut entries = raw.split(|byte| *byte == 0).peekable();
        let mut files = Files::new();
        let mut original = Files::new();
        while let Some(header) = entries.next() {
            if header.is_empty() && entries.peek().is_none() {
                break;
            }
            let header = std::str::from_utf8(header)
                .map_err(|_| CoreError::Refused("assignment_worktree_unobserved"))?;
            let fields = header.split_whitespace().collect::<Vec<_>>();
            if fields.len() != 5
                || fields[4] != "M"
                || !matches!(fields[0], ":100644" | ":100755")
                || !matches!(fields[1], "100644" | "100755")
                || files.len() >= MAX_FILES
            {
                return refuse("assignment_worktree_unsupported_patch");
            }
            let raw_path = entries
                .next()
                .and_then(|p| std::str::from_utf8(p).ok())
                .ok_or(CoreError::Refused("assignment_worktree_unobserved"))?;
            RelativePath::new(raw_path)?;
            if Path::new(raw_path).components().any(|part| {
                matches!(
                    part.as_os_str().to_str(),
                    Some(".gitattributes" | ".gitmodules" | ".gitignore")
                )
            }) {
                return refuse("assignment_worktree_unsupported_patch");
            }
            files.insert(raw_path.into(), regular_file(root, raw_path)?);
            let before =
                checked(git(root).args(["cat-file", "blob", &format!("{base}:{raw_path}")]))?;
            let executable = if fields[0] == ":100755" { 0o111 } else { 0 };
            original.insert(raw_path.into(), (executable, digest(&before)));
        }
        if files.is_empty() != patch.is_empty() {
            return refuse("assignment_worktree_unobserved");
        }
        Ok(Self {
            patch,
            files,
            original,
        })
    }

    pub(in crate::workspace) fn paths(&self) -> impl Iterator<Item = &str> {
        self.files.keys().map(String::as_str)
    }

    pub(in crate::workspace) fn digest(&self) -> String {
        digest(&self.patch)
    }

    pub(in crate::workspace) fn check(&self, target: &Path) -> CoreResult<()> {
        self.apply(target, true)
    }

    pub(in crate::workspace) fn apply_to(&self, target: &Path) -> CoreResult<()> {
        self.apply(target, false)
    }

    pub(in crate::workspace) fn includes(&self, other: &Self) -> bool {
        other
            .files
            .iter()
            .all(|(path, bytes)| self.files.get(path) == Some(bytes))
    }

    pub(in crate::workspace) fn count(&self) -> usize {
        self.files.len()
    }

    pub(in crate::workspace) fn can_combine(&self, other: &Self) -> bool {
        self.count() + other.count() <= MAX_FILES
            && self.patch.len() + other.patch.len() <= 4 * 1024 * 1024
    }

    fn apply(&self, target: &Path, check: bool) -> CoreResult<()> {
        if self.patch.is_empty() {
            return Ok(());
        }
        for (path, before) in &self.original {
            if regular_file(target, path).ok().as_ref() != Some(before) {
                return refuse("assignment_worktree_conflict");
            }
        }
        // Anonymous input keeps the patch out of the project tree and avoids
        // blocking IPC while Git's deadline is running.
        let fd =
            unsafe { libc::memfd_create(c"cortex-assignment-patch".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return refuse("assignment_worktree_patch_unavailable");
        }
        let mut input = unsafe { File::from_raw_fd(fd) };
        input.write_all(&self.patch)?;
        input.rewind()?;
        let mut cmd = git(target);
        neutralize_filters(target, &mut cmd)?;
        cmd.args(["apply", "--whitespace=nowarn"]);
        if check {
            cmd.arg("--check");
        }
        let result = run(cmd.arg("-").stdin(Stdio::from(input)))?;
        if !result.status.success() {
            return refuse(if check {
                "assignment_worktree_conflict"
            } else {
                "assignment_worktree_outcome_unknown"
            });
        }
        Ok(())
    }
}

fn regular_file(root: &Path, relative: &str) -> CoreResult<(u32, String)> {
    let path = root.join(relative);
    if path.canonicalize()? != path {
        return refuse("assignment_worktree_unobserved");
    }
    let info = fs::symlink_metadata(&path)?;
    if !info.is_file() || info.nlink() != 1 || info.len() > 4 * 1024 * 1024 {
        return refuse("assignment_worktree_unsupported_patch");
    }
    let mut bytes = Vec::new();
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)?;
    let opened = file.metadata()?;
    file.take(4 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    if (opened.dev(), opened.ino(), opened.len()) != (info.dev(), info.ino(), info.len())
        || bytes.len() as u64 != info.len()
    {
        return refuse("assignment_worktree_unobserved");
    }
    Ok((info.permissions().mode() & 0o111, digest(&bytes)))
}

fn diff(root: &Path, flags: &[&str]) -> CoreResult<Vec<u8>> {
    let mut cmd = git(root);
    neutralize_filters(root, &mut cmd)?;
    checked(
        cmd.args(["diff", "--no-ext-diff", "--no-textconv", "--no-renames"])
            .args(flags)
            .args(["HEAD", "--"]),
    )
}

pub(in crate::workspace) fn clean_target_index(root: &Path) -> CoreResult<bool> {
    let mut cmd = git(root);
    neutralize_filters(root, &mut cmd)?;
    let output = run(cmd.args([
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--quiet",
        "--cached",
        "HEAD",
        "--",
    ]))?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => refuse("assignment_worktree_unobserved"),
    }
}

pub(in crate::workspace) fn index_tracks(root: &Path, paths: &[String]) -> CoreResult<bool> {
    let output = run(git(root)
        .arg("--literal-pathspecs")
        .args(["ls-files", "--error-unmatch", "-z", "--"])
        .args(paths))?;
    if !output.status.success() {
        return Ok(false);
    }
    let observed: std::collections::BTreeSet<_> = output
        .stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .collect();
    Ok(observed == paths.iter().map(|p| p.as_bytes()).collect())
}
