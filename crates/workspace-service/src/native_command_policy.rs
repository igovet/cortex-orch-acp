//! Advanced command policy shared by the three bubblewrap profiles.
use super::*;
use std::{
    collections::{BTreeSet, HashMap},
    ffi::{CString, OsStr},
    fs::File,
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, PermissionsExt},
    },
};

pub(super) fn env_path(env: &BTreeMap<OsString, OsString>, key: &str) -> Option<PathBuf> {
    env.get(OsStr::new(key))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}
pub(super) fn controls(
    root: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> io::Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let home = env_path(env, "HOME").filter(|p| p.is_absolute());
    if let Some(home) = &home {
        paths.extend(
            [
                ".codex",
                ".claude",
                ".paseo",
                ".gitconfig",
                ".git-credentials",
            ]
            .map(|n| home.join(n)),
        );
    }
    for (key, fallback, names) in [
        (
            "XDG_CONFIG_HOME",
            ".config",
            &["cortex", "Paseo", "opencode"][..],
        ),
        ("XDG_STATE_HOME", ".local/state", &["cortex"][..]),
        ("XDG_DATA_HOME", ".local/share", &["opencode"][..]),
    ] {
        let Some(base) = env_path(env, key).or_else(|| home.as_ref().map(|h| h.join(fallback)))
        else {
            continue;
        };
        if !base.is_absolute() {
            return Err(unavailable("invalid_control_path"));
        }
        paths.extend(names.iter().map(|n| base.join(n)));
    }
    for key in ["CODEX_HOME", "CLAUDE_CONFIG_DIR"] {
        if let Some(p) = env_path(env, key) {
            if !p.is_absolute() || !p.exists() {
                return Err(unavailable("invalid_control_path"));
            }
            paths.push(p);
        }
    }
    let logical = cortex_core::ProjectKey::new(root, cortex_core::StoreChannel::Release)
        .map_err(|_| unavailable("project_context_conflict"))?;
    paths.extend([
        logical.root().join(".cortex"),
        logical.root().join(".cortex-dev"),
    ]);
    Ok(paths)
}
pub(crate) fn protected_target(root: &Path, path: &Path) -> bool {
    protected_target_environment(root, path, &std::env::vars_os().collect())
}
pub(super) fn protected_target_environment(
    root: &Path,
    path: &Path,
    env: &BTreeMap<OsString, OsString>,
) -> bool {
    let Ok(mut controls) = controls(root, env) else {
        return true;
    };
    let Ok(git) = pin(root).and_then(|checkout| git_inputs(root, &checkout)) else {
        return true;
    };
    controls.extend(git.into_iter().map(|(path, _)| path));
    controls
        .iter()
        .any(|p| path.starts_with(p) || p.canonicalize().is_ok_and(|real| path.starts_with(real)))
}

/// fnmatchcase semantics: '*' includes '/', with literal directory-prefix grants.
pub(super) fn matches(path: &str, pattern: &str) -> bool {
    if [".", "**", "**/*"].contains(&pattern)
        || path == pattern
        || path.starts_with(&format!("{}/", pattern.trim_end_matches('/')))
        || pattern.strip_suffix("/**") == Some(path)
    {
        return true;
    }
    let (Ok(path), Ok(pattern)) = (CString::new(path), CString::new(pattern)) else {
        return false;
    };
    unsafe { libc::fnmatch(pattern.as_ptr(), path.as_ptr(), 0) == 0 }
}
pub(super) fn expand(root: &Path, paths: &[PathBuf]) -> io::Result<Vec<PathBuf>> {
    let mut result = BTreeSet::new();
    for p in paths {
        let text = p.to_string_lossy();
        if !text.contains(['*', '?', '[']) {
            result.insert(p.clone());
            continue;
        }
        // Only project-relative denial patterns are expanded, never the host tree.
        let relative = p
            .strip_prefix(root)
            .map_err(|_| unavailable("invalid_denial"))?;
        let pattern = relative
            .to_str()
            .ok_or_else(|| unavailable("invalid_denial"))?;
        let mut pending = vec![root.to_owned()];
        let mut count = 0;
        while let Some(dir) = pending.pop() {
            for e in fs::read_dir(dir)? {
                count += 1;
                if count > 2_000_000 {
                    return Err(unavailable("project_tree_limit"));
                }
                let e = e?;
                let p = e.path();
                let name = p.strip_prefix(root).unwrap().to_string_lossy();
                if matches(&name, pattern) {
                    result.insert(p.clone());
                }
                if e.file_type()?.is_dir() && !private(&e.file_name()) {
                    pending.push(p);
                }
            }
        }
    }
    Ok(result.into_iter().collect())
}
pub(super) fn private(name: &OsStr) -> bool {
    [
        ".git",
        ".cortex",
        ".cortex-dev",
        ".cortex-dev-owner.json",
        ".cortex-workspace-dev.json",
        ".codex",
        ".claude",
        ".ssh",
        ".gnupg",
        ".aws",
        ".azure",
        ".kube",
        ".docker",
        ".git-credentials",
        ".gitconfig",
        ".credentials.json",
        ".claude.json",
        ".netrc",
        ".npmrc",
        ".pypirc",
        ".env",
        "cortex.sqlite3",
        "cortex.sqlite3-wal",
        "cortex.sqlite3-shm",
        "cortex.sqlite3-journal",
    ]
    .iter()
    .any(|n| name == *n)
        || name.as_encoded_bytes().starts_with(b".env.")
            && ![b".example".as_slice(), b".sample", b".template"]
                .iter()
                .any(|suffix| name.as_encoded_bytes().ends_with(suffix))
        || opencode_database(name)
}

pub(super) fn opencode_database(name: &OsStr) -> bool {
    ["opencode.db", "opencode.db-wal", "opencode.db-shm", "opencode.db-journal"]
        .iter()
        .any(|candidate| name == *candidate)
}

pub(super) use cortex_engine::filesystem_view::pin;
#[cfg(test)]
pub(super) fn s_output(c: &mut Command) -> io::Result<Option<String>> {
    use std::{fs::OpenOptions, os::unix::fs::OpenOptionsExt};

    // Spool instead of a pipe: neither a large reply nor a child can deadlock preparation.
    let name = std::env::temp_dir().join(format!(
        "cortex-git-query-{}",
        crate::native_effects::identity().map_err(|_| unavailable("scratch_identity"))?
    ));
    let mut spool = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&name)?;
    fs::remove_file(name)?;
    let mut child = c.stdout(spool.try_clone()?).spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match child.try_wait()? {
            Some(status) => {
                if !status.success() {
                    return Ok(None);
                }
                use std::io::{Seek, SeekFrom};
                spool.seek(SeekFrom::Start(0))?;
                let mut out = String::new();
                spool.take(1024 * 1024 + 1).read_to_string(&mut out)?;
                if out.len() > 1024 * 1024 {
                    return Err(unavailable("git_query_limit"));
                }
                return Ok(Some(out.trim_end_matches('\n').into()));
            }
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unavailable("git_query_timeout"));
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}
pub(super) fn git_inputs(root: &Path, checkout: &File) -> io::Result<Vec<(PathBuf, std::sync::Arc<File>)>> {
    let marker = root.join(".git");
    match marker.symlink_metadata() {
        Ok(m) if m.is_file() || m.is_dir() => {}
        Ok(_) => return Err(unavailable("git_metadata_alias")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    }
    let input = open_input(checkout, Path::new(".git"))?;
    let meta = input.metadata()?;
    if meta.is_dir() {
        return Ok(vec![(marker, std::sync::Arc::new(input))]);
    }
    // Resolve only the metadata pointer, not repository configuration. In
    // particular, discovering mounts must not follow a Git config include on
    // the host. The descriptor pins the checked, bounded regular file.
    if !meta.is_file() || meta.nlink() != 1 || meta.len() > 4096 {
        return Err(unavailable("unverified_git_input"));
    }
    let mut contents = String::new();
    File::open(format!("/proc/self/fd/{}", input.as_raw_fd()))?
        .take(4097)
        .read_to_string(&mut contents)?;
    let mut lines = contents.lines();
    let pointer = lines
        .next()
        .and_then(|line| line.strip_prefix("gitdir: "))
        .filter(|p| !p.is_empty() && contents.len() <= 4096 && lines.next().is_none())
        .ok_or_else(|| unavailable("unverified_git_input"))?;
    // ProjectKey validates the gitfile, commondir and backlink. Reopen the
    // selected worktree metadata beneath the pinned common directory; alias
    // and mount crossings are rejected just as in the checkout walk.
    let project = cortex_core::ProjectKey::new(root, cortex_core::StoreChannel::Release)
        .map_err(|_| unavailable("unverified_git_input"))?;
    let common = project.root().join(".git");
    let dir = root.join(pointer).canonicalize()?;
    if project.root() == root || dir.parent() != Some(common.join("worktrees").as_path()) {
        return Err(unavailable("unverified_git_input"));
    }
    let common_pin = open_input(&pin(project.root())?, Path::new(".git"))?;
    let dir_pin = open_input(&common_pin, dir.strip_prefix(&common).unwrap())?;
    if !common_pin.metadata()?.is_dir() || !dir_pin.metadata()?.is_dir() {
        return Err(unavailable("unverified_git_input"));
    }
    // The first entry supplies per-worktree state, the last shared state.
    Ok(vec![
        (dir, std::sync::Arc::new(dir_pin)),
        (common, std::sync::Arc::new(common_pin)),
    ])
}

fn open_input(folder: &File, path: &Path) -> io::Result<File> {
    let name = path.to_str().ok_or_else(|| unavailable("invalid_project_input"))?;
    crate::native_project::openat(folder, name, libc::O_PATH)
        .map_err(|e| match e.code() {
            "project_mount_alias_refused" => unavailable("project_mount_alias_refused"),
            _ => unavailable("project_input_changed_or_unavailable"),
        })
}

pub(super) fn cleanup_scratch(path: &Path) -> io::Result<()> {
    // Overlay work directories may have mode 000. Reopen/chmod only pinned,
    // owner-owned directories within this launch\'s scratch, never aliases.
    fn permit(file: File) -> io::Result<()> {
        let m = file.metadata()?;
        if m.is_dir() && m.uid() == unsafe { libc::getuid() } {
            fs::set_permissions(
                format!("/proc/self/fd/{}", file.as_raw_fd()),
                fs::Permissions::from_mode(0o700),
            )?;
            for e in fs::read_dir(format!("/proc/self/fd/{}", file.as_raw_fd()))? {
                let e = e?;
                if e.file_type()?.is_dir() {
                    let name = CString::new(e.file_name().as_encoded_bytes()).unwrap();
                    let fd = unsafe {
                        libc::openat(
                            file.as_raw_fd(),
                            name.as_ptr(),
                            libc::O_PATH | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                        )
                    };
                    if fd < 0 {
                        return Err(io::Error::last_os_error());
                    }
                    permit(unsafe { File::from_raw_fd(fd) })?;
                }
            }
        }
        Ok(())
    }
    permit(pin(path)?)?;
    fs::remove_dir_all(path)
}
#[derive(Clone)]
struct Link {
    path: PathBuf,
    meta: fs::Metadata,
}
/// Mount-time project discovery. No file data is copied or written.
pub(super) struct Scan {
    /// Private and special entries are never descended, except for the
    /// separately pinned and scanned Git metadata roots.
    pub(super) private: Vec<PathBuf>,
    /// Canonical targets of private-named symbolic links.
    pub(super) private_targets: Vec<PathBuf>,
    /// Visible regular-file candidates with more than one link.
    links: Vec<Link>,
    pub(super) entries: usize,
}
const TREE_LIMIT: usize = 2_000_000;
pub(super) fn scan(
    root: &Path,
    checkout: &File,
    hidden: &[PathBuf],
    git: &[(PathBuf, std::sync::Arc<File>)],
    registrations: &[PathBuf],
) -> io::Result<Scan> {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Condvar, Mutex,
    };
    let mut denied_all = hidden.to_vec();
    denied_all.extend(hidden.iter().filter_map(|d| d.canonicalize().ok()));
    struct Queue {
        pending: Vec<PathBuf>,
        active: usize,
        error: Option<io::Error>,
    }
    let queue = Mutex::new(Queue {
        pending: std::iter::once(root.to_owned())
            .chain(git.iter().map(|(path, _)| path.clone()))
            .collect(),
        active: 0,
        error: None,
    });
    let wake = Condvar::new();
    let count = AtomicUsize::new(0);
    let git_entry = root.join(".git");
    let visit = |dir: &Path, found: &mut Scan, children: &mut Vec<PathBuf>| -> io::Result<()> {
        let (base, anchor) = git
            .iter()
            .rev()
            .find(|(p, _)| dir.starts_with(p))
            .map_or((root, checkout), |(p, fd)| (p.as_path(), fd.as_ref()));
        let relative = dir.strip_prefix(base).map_err(|_| unavailable("invalid_project_input"))?;
        let folder = if relative.as_os_str().is_empty() {
            anchor.try_clone()?
        } else {
            open_input(anchor, relative)?
        };
        for e in fs::read_dir(format!("/proc/self/fd/{}", folder.as_raw_fd()))? {
            let e = e?;
            if count.fetch_add(1, Ordering::Relaxed) >= TREE_LIMIT {
                return Err(unavailable("project_tree_limit"));
            }
            let kind = e.file_type()?;
            let name = e.file_name();
            let p = dir.join(&name);
            if registrations.contains(&p) || git.iter().any(|(path, _)| *path == p) {
                if !open_input(&folder, Path::new(&name))?.metadata()?.is_dir() {
                    return Err(unavailable("project_input_changed_or_unavailable"));
                }
                continue;
            }
            // In a linked checkout the admitted gitfile still participates in
            // checkout alias checks; only its name gets an exception.
            let is_private = private(&name) && (p != git_entry || git.is_empty() || !kind.is_file());
            if kind.is_symlink() {
                if is_private {
                    match p.canonicalize() {
                        Ok(target) if target == root => {
                            return Err(unavailable("control_path_overlaps_project"));
                        }
                        Ok(target) => found.private_targets.push(target),
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e),
                    }
                }
                continue;
            }
            let meta = open_input(&folder, Path::new(&name))?.metadata()?;
            if meta.file_type() != kind {
                return Err(unavailable("project_input_changed_or_unavailable"));
            }
            if is_private || (!kind.is_file() && !kind.is_dir()) {
                found.private.push(p);
            } else if kind.is_dir() && !denied_all.iter().any(|d| p.starts_with(d)) {
                children.push(p);
            } else if kind.is_file() && meta.nlink() > 1
                && !denied_all.iter().any(|d| p.starts_with(d))
            {
                found.links.push(Link { path: p, meta });
            }
        }
        Ok(())
    };
    let worker = || -> Scan {
        let mut found = Scan {
            private: Vec::new(),
            private_targets: Vec::new(),
            links: Vec::new(),
            entries: 0,
        };
        let mut children = Vec::new();
        loop {
            let dir = {
                let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    if q.error.is_some() {
                        return found;
                    }
                    if let Some(dir) = q.pending.pop() {
                        q.active += 1;
                        break dir;
                    }
                    if q.active == 0 {
                        return found;
                    }
                    q = wake.wait(q).unwrap_or_else(|e| e.into_inner());
                }
            };
            let result = visit(&dir, &mut found, &mut children);
            let mut q = queue.lock().unwrap_or_else(|e| e.into_inner());
            q.active -= 1;
            match result {
                Ok(()) => q.pending.append(&mut children),
                Err(e) => {
                    q.error.get_or_insert(e);
                }
            }
            wake.notify_all();
        }
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, 8);
    let parts: Vec<Scan> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..threads).map(|_| scope.spawn(worker)).collect();
        handles
            .into_iter()
            .map(|h| h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)))
            .collect()
    });
    if let Some(e) = queue.into_inner().unwrap_or_else(|e| e.into_inner()).error {
        return Err(e);
    }
    let mut all = Scan {
        private: Vec::new(),
        private_targets: Vec::new(),
        links: Vec::new(),
        entries: count.into_inner(),
    };
    for mut part in parts {
        all.private.append(&mut part.private);
        all.private_targets.append(&mut part.private_targets);
        all.links.append(&mut part.links);
    }
    // A deterministic order independent of thread scheduling. Read-only binds come
    // first and private store directories (hidden by a read-only tmpfs) last, so a
    // later mount never targets a path inside an already-hidden store.
    all.private.sort_by_cached_key(|p| {
        (
            p.file_name()
                .is_some_and(|n| n == ".cortex" || n == ".cortex-dev"),
            p.clone(),
        )
    });
    all.links.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(all)
}
/// A group with fewer authorized names than its inode's link count may have
/// an alias outside the checkout or inside a hidden project subtree.
pub(super) fn external_link_paths<'a>(scan: &'a Scan, masks: &BTreeSet<PathBuf>) -> Vec<&'a Path> {
    let mut counts = HashMap::new();
    for link in &scan.links {
        // A protected symlink may have designated another visible candidate
        // as private. Do not count that candidate as authority for its aliases.
        if !link.path.ancestors().any(|p| masks.contains(p)) {
            *counts.entry((link.meta.dev(), link.meta.ino())).or_insert(0u64) += 1;
        }
    }
    scan.links
        .iter()
        .filter(|l| counts.get(&(l.meta.dev(), l.meta.ino())) != Some(&l.meta.nlink()))
        .map(|l| l.path.as_path())
        .collect()
}
