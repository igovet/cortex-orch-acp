//! Advanced command policy shared by the three bubblewrap profiles.
use super::*;
use std::{
    collections::{BTreeSet, HashMap},
    ffi::{CString, OsStr},
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    sync::{mpsc, Arc, Mutex},
    thread::JoinHandle,
};

const SENSITIVE: &[&str] = &[
    ".ssh",
    ".gnupg",
    ".aws",
    ".azure",
    ".kube",
    ".docker",
    ".password-store",
    ".pki",
    ".mozilla",
    ".thunderbird",
    ".config",
    ".local",
    ".claude",
    ".codex",
    ".cortex",
    ".git",
    ".paseo",
    ".hermes",
    ".cortex-dev",
];
const CACHES: &[&str] = &[
    ".cache",
    ".npm",
    ".yarn/berry/cache",
    ".pnpm-store",
    ".local/share/pnpm/store",
    ".bun/install/cache",
    ".cargo/registry",
    ".cargo/git",
    ".cargo/.package-cache",
    ".cargo/.package-cache-mutate",
    ".cargo/.global-cache",
    ".m2/repository",
    ".gradle/caches",
    ".gradle/wrapper/dists",
    ".gradle/daemon",
    ".gradle/native",
    ".gradle/.tmp",
    ".ivy2/cache",
    ".nuget/packages",
    "go/pkg/mod",
    ".local/share/virtualenv",
    ".node-gyp",
    ".ccache",
    ".docker/buildx/activity",
    ".docker/buildx/refs",
];
const CACHE_ENV: &[&str] = &[
    "XDG_CACHE_HOME",
    "npm_config_cache",
    "NPM_CONFIG_CACHE",
    "npm_config_store_dir",
    "PIP_CACHE_DIR",
    "UV_CACHE_DIR",
    "YARN_CACHE_FOLDER",
    "GOMODCACHE",
    "GOCACHE",
    "PRE_COMMIT_HOME",
    "BUN_INSTALL_CACHE_DIR",
    "CCACHE_DIR",
];
const GIT_GUARDED: &[&str] = &[
    "config",
    "config.worktree",
    "commondir",
    "gitdir",
    "hooks",
    "remotes",
    "branches",
];

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
    if let Ok(exe) = std::env::current_exe() {
        paths.push(exe);
    }
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
    if let Ok(git) = git_dirs(root) {
        controls.extend(git);
    } else {
        return true;
    }
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
    ]
    .iter()
    .any(|n| name == *n)
}

pub(super) fn home(
    s: &mut Sandbox,
    root: &Path,
    profile: Profile,
    protected: &[PathBuf],
) -> io::Result<()> {
    let home = env_path(&s.environment, "HOME").filter(|p| p.is_absolute() && p.is_dir());
    let Some(home) = home else {
        if profile == Profile::ReadOnly {
            for key in CACHE_ENV {
                if let Some(p) =
                    env_path(&s.environment, key).filter(|p| p.is_absolute() && p.is_dir())
                {
                    let real = p.canonicalize()?;
                    if real != Path::new("/")
                        && !overlaps(&real, root)
                        && !protected.iter().any(|p| {
                            overlaps(&real, p)
                                || p.canonicalize().is_ok_and(|p| overlaps(&real, &p))
                        })
                    {
                        s.mount("--bind", &real, &real)?;
                    }
                }
            }
        }
        return Ok(());
    };
    let real_home = home.canonicalize()?;
    if profile != Profile::Development {
        s.mount("--ro-bind", &real_home, &real_home)?;
    }
    for name in [".cache", ".npm"] {
        let p = home.join(name);
        if fs::symlink_metadata(&p).is_err_and(|e| e.kind() == io::ErrorKind::NotFound) {
            let _ = fs::DirBuilder::new().mode(0o700).create(p);
        }
    }
    let mut candidates = Vec::new();
    if profile == Profile::Guarded {
        for e in fs::read_dir(&home)? {
            let e = e?;
            let name = e.file_name();
            let name = name.to_string_lossy();
            if (name.starts_with('.') || name == "go")
                && !SENSITIVE.contains(&name.as_ref())
                && e.file_type()?.is_dir()
            {
                candidates.push((e.path(), false));
            }
        }
        let share = home.join(".local/share");
        if share.is_dir() && !share.symlink_metadata()?.file_type().is_symlink() {
            for e in fs::read_dir(share)? {
                let e = e?;
                if e.file_type()?.is_dir()
                    && !["keyrings", "applications", "autostart", "systemd"]
                        .iter()
                        .any(|n| e.file_name() == *n)
                {
                    candidates.push((e.path(), false));
                }
            }
        }
        let state = home.join(".local/state");
        if state.is_dir() && !state.symlink_metadata()?.file_type().is_symlink() {
            candidates.push((state, false));
        }
    } else if profile == Profile::ReadOnly {
        candidates.extend(CACHES.iter().map(|n| (home.join(n), false)));
        candidates.extend(
            CACHE_ENV
                .iter()
                .filter_map(|key| env_path(&s.environment, key))
                .filter(|p| p.is_absolute())
                .map(|p| (p, true)),
        );
    }
    for (p, relocated) in candidates {
        let Ok(real) = p.canonicalize() else { continue };
        let meta = fs::metadata(&real)?;
        if (!meta.is_dir() && (relocated || !meta.is_file()))
            || real == Path::new("/")
            || overlaps(&real, root)
            || real_home.starts_with(&real)
            || protected.iter().any(|p| {
                if profile == Profile::ReadOnly {
                    overlaps(&real, p) || p.canonicalize().is_ok_and(|p| overlaps(&real, &p))
                } else {
                    real.starts_with(p) || p.canonicalize().is_ok_and(|p| real.starts_with(p))
                }
            })
            || relocated
                && SENSITIVE
                    .iter()
                    .any(|n| overlaps(&real, &real_home.join(n)))
        {
            continue;
        }
        s.mount("--bind", &real, &real)?;
    }
    Ok(())
}

/// Open every component without following aliases, retaining an O_PATH mount source.
pub(super) fn pin(path: &Path) -> io::Result<File> {
    if !path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
    {
        return Err(unavailable("invalid_mount_path"));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_PATH | libc::O_DIRECTORY)
        .open("/")?;
    for c in path.components().skip(1) {
        let name = CString::new(c.as_os_str().as_encoded_bytes())
            .map_err(|_| unavailable("invalid_mount_path"))?;
        let fd = unsafe {
            libc::openat(
                file.as_raw_fd(),
                name.as_ptr(),
                libc::O_PATH | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        file = unsafe { File::from_raw_fd(fd) };
        if file.metadata()?.file_type().is_symlink() {
            return Err(unavailable("mount_source_changed"));
        }
    }
    Ok(file)
}
fn query(root: &Path, args: &[&str]) -> io::Result<Option<String>> {
    let mut c = Command::new("/usr/bin/git");
    c.args(["-C"])
        .arg(root)
        .args(["--no-optional-locks", "-c", "core.fsmonitor=false"])
        .args(args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    if args.first() == Some(&"config") {
        c.env_remove("GIT_CONFIG_GLOBAL");
        if let Some(home) = std::env::var_os("HOME") {
            c.env("HOME", home);
        }
    }
    let output = s_output(&mut c)?;
    Ok(output.filter(|v| v.len() < 1024 * 1024))
}
fn s_output(c: &mut Command) -> io::Result<Option<String>> {
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
pub(super) fn git_dirs(root: &Path) -> io::Result<Vec<PathBuf>> {
    if !root.join(".git").try_exists()? {
        return Ok(vec![]);
    }
    if root
        .join(".git")
        .symlink_metadata()?
        .file_type()
        .is_symlink()
    {
        return Err(unavailable("git_metadata_alias"));
    }
    let mut dirs = Vec::new();
    for key in ["--absolute-git-dir", "--git-common-dir"] {
        let text = query(root, &["rev-parse", key])?
            .ok_or_else(|| unavailable("git_context_unavailable"))?;
        let p = PathBuf::from(text);
        let p = if p.is_absolute() { p } else { root.join(p) }.canonicalize()?;
        pin(&p)?;
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    Ok(dirs)
}
pub(super) fn hooks(root: &Path) -> io::Result<Option<PathBuf>> {
    let Some(text) = query(root, &["config", "--get", "core.hooksPath"])? else {
        return Ok(None);
    };
    let p = PathBuf::from(text);
    let p = if p.is_absolute() { p } else { root.join(p) };
    Ok(if p.is_dir() && p != root {
        Some(p.canonicalize()?)
    } else {
        None
    })
}

/// `dirs` is this preparation's own [`git_dirs`] answer (each directory was pinned).
pub(super) fn git(
    s: &mut Sandbox,
    root: &Path,
    dirs: &[PathBuf],
    profile: Profile,
    whole: bool,
) -> io::Result<()> {
    if !whole || profile == Profile::ReadOnly {
        for p in dirs {
            s.mount("--ro-bind", p, p)?;
        }
        // LFS can write even during Git inspection.
        s.git_config(&[format!("lfs.storage={}/git-lfs", s.scratch.display())]);
        return Ok(());
    }
    let mut writable = dirs.to_vec();
    let mut readonly = BTreeSet::new();
    let mut watched = Vec::new();
    let development = profile == Profile::Development;
    if development {
        let mut pending: Vec<_> = dirs.iter().map(|p| p.join("modules")).collect();
        let mut count = 0;
        while let Some(p) = pending.pop() {
            if let Ok(m) = p.symlink_metadata() {
                if !m.is_dir() {
                    continue;
                }
                count += 1;
                if count > 100_000 {
                    return Err(unavailable("git_tree_limit"));
                }
                if p.join("HEAD").is_file() && p.join("config").is_file() {
                    writable.push(p.clone());
                    pending.push(p.join("modules"));
                } else {
                    for e in fs::read_dir(p)? {
                        let e = e?;
                        if e.file_type()?.is_dir() {
                            pending.push(e.path());
                        }
                    }
                }
            }
        }
    }
    for p in &writable {
        for name in ["worktrees", "modules"] {
            let q = p.join(name);
            if fs::symlink_metadata(&q).is_ok() {
                readonly.insert(q);
            } else if development {
                watched.push(q);
            }
        }
        for name in GIT_GUARDED
            .iter()
            .copied()
            .chain(development.then_some("info"))
        {
            if name == "gitdir" && Some(p) == dirs.last() && !development {
                continue;
            }
            let q = p.join(name);
            if fs::symlink_metadata(&q).is_ok() {
                readonly.insert(q);
            } else {
                watched.push(q);
            }
        }
    }
    if let Some(h) = hooks(root)? {
        readonly.insert(h);
    }
    writable.sort_by_key(|p| p.components().count());
    let mut mounted = BTreeSet::new();
    for p in &writable {
        for guard in &readonly {
            if p.starts_with(guard) && p != guard && mounted.insert(guard.clone()) {
                s.mount("--ro-bind", guard, guard)?;
            }
        }
        s.mount("--bind", p, p)?;
    }
    for p in readonly.difference(&mounted) {
        s.mount("--ro-bind", p, p)?;
    }
    if root.join(".git").is_file() {
        s.mount("--ro-bind", &root.join(".git"), &root.join(".git"))?;
    }
    s.guard = Some(GitGuard::new(watched)?);
    if development {
        s.git_config(&[
            "core.hooksPath=/dev/null".into(),
            "core.fsmonitor=false".into(),
        ]);
    }
    Ok(())
}

/// Same retained policy as Python: absent Git controls are swept during and after
/// the command, not an atomic hostile-process barrier. Parent descriptors never follow links.
pub(super) struct GitGuard {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    paths: Arc<Vec<(File, CString)>>,
    removed: Arc<Mutex<Vec<String>>>,
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
fn remove_at(parent: &File, name: &CString) -> io::Result<()> {
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd >= 0 {
        let folder = unsafe { File::from_raw_fd(fd) };
        for e in fs::read_dir(format!("/proc/self/fd/{}", folder.as_raw_fd()))? {
            let e = e?;
            let leaf = CString::new(e.file_name().as_encoded_bytes()).unwrap();
            remove_at(&folder, &leaf)?;
        }
        if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR) } == 0 {
            return Ok(());
        }
    } else if unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), 0) } == 0 {
        return Ok(());
    }
    let e = io::Error::last_os_error();
    if e.kind() == io::ErrorKind::NotFound {
        Ok(())
    } else {
        Err(e)
    }
}
impl GitGuard {
    fn new(paths: Vec<PathBuf>) -> io::Result<Self> {
        let mut entries = Vec::new();
        for p in paths {
            let parent = pin(p.parent().ok_or_else(|| unavailable("git_guard_parent"))?)?;
            let name = CString::new(p.file_name().unwrap().as_encoded_bytes()).unwrap();
            entries.push((parent, name));
        }
        let paths = Arc::new(entries);
        let removed = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();
        let p = paths.clone();
        let r = removed.clone();
        let thread = std::thread::spawn(move || {
            while rx.recv_timeout(Duration::from_millis(500))
                == Err(mpsc::RecvTimeoutError::Timeout)
            {
                Self::sweep(&p, &r);
            }
        });
        Ok(Self {
            stop: Some(tx),
            thread: Some(thread),
            paths,
            removed,
        })
    }
    fn sweep(paths: &[(File, CString)], removed: &Mutex<Vec<String>>) {
        for (parent, leaf) in paths {
            let mut m = std::mem::MaybeUninit::<libc::stat>::uninit();
            if unsafe {
                libc::fstatat(
                    parent.as_raw_fd(),
                    leaf.as_ptr(),
                    m.as_mut_ptr(),
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            } == 0
                && remove_at(parent, leaf).is_ok()
            {
                removed
                    .lock()
                    .unwrap()
                    .push(leaf.to_string_lossy().into_owned());
            }
        }
    }
    pub(super) fn finish(&mut self) -> Vec<String> {
        if let Some(tx) = self.stop.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        Self::sweep(&self.paths, &self.removed);
        self.removed.lock().unwrap().clone()
    }
}
impl Drop for GitGuard {
    fn drop(&mut self) {
        self.finish();
    }
}

#[derive(Clone)]
struct Link {
    path: PathBuf,
    meta: fs::Metadata,
    owner: PathBuf,
}
fn version(m: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64, u32, u32, u32) {
    (
        m.dev(),
        m.ino(),
        m.size(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
        m.mode(),
        m.uid(),
        m.gid(),
    )
}
/// One complete traversal of the project tree at preparation time, shared by the
/// private-entry view and the hard-link policy (formerly two separate walks).
/// Nothing is cached between commands: a link count can change without touching any
/// project directory (a new link made from outside the project), so only a fresh
/// scan of every writable regular file is exact. Directory types come from the
/// directory listing itself; only regular files inside the write set are `fstatat`ed
/// (relative to their open directory), and directories are listed in parallel.
pub(super) struct Scan {
    /// Every entry named like a private control (`.git`, `.cortex`, `.cortex-dev`,
    /// the two dev JSON files) at any depth. Such entries are never descended.
    pub(super) private: Vec<PathBuf>,
    /// Canonical targets of private-named symbolic links.
    private_targets: Vec<PathBuf>,
    /// Writable, undenied regular files with more than one link.
    links: Vec<Link>,
    pub(super) entries: usize,
}
const TREE_LIMIT: usize = 2_000_000;
pub(super) fn scan(root: &Path, writes: &[PathBuf], denied: &[PathBuf]) -> io::Result<Scan> {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Condvar,
    };
    // Each denial is resolved once, not once per candidate file.
    let mut denied_all = denied.to_vec();
    denied_all.extend(denied.iter().filter_map(|d| d.canonicalize().ok()));
    struct Queue {
        pending: Vec<PathBuf>,
        active: usize,
        error: Option<io::Error>,
    }
    let queue = Mutex::new(Queue {
        pending: vec![root.to_owned()],
        active: 0,
        error: None,
    });
    let wake = Condvar::new();
    let count = AtomicUsize::new(0);
    let visit = |dir: &Path, found: &mut Scan, children: &mut Vec<PathBuf>| -> io::Result<()> {
        for e in fs::read_dir(dir)? {
            let e = e?;
            if count.fetch_add(1, Ordering::Relaxed) >= TREE_LIMIT {
                return Err(unavailable("project_tree_limit"));
            }
            let kind = e.file_type()?;
            let p = e.path();
            if private(&e.file_name()) {
                if kind.is_symlink() {
                    let target = p.canonicalize()?;
                    if target == root {
                        return Err(unavailable("control_path_overlaps_project"));
                    }
                    found.private_targets.push(target);
                }
                found.private.push(p);
                continue;
            }
            if kind.is_dir() {
                children.push(p);
            } else if kind.is_file() && writes.iter().any(|w| p.starts_with(w)) {
                let meta = e.metadata()?;
                if meta.is_dir() {
                    children.push(p);
                } else if meta.is_file()
                    && meta.nlink() > 1
                    && !denied_all.iter().any(|d| p.starts_with(d))
                {
                    let owner = p
                        .strip_prefix(root)
                        .unwrap()
                        .components()
                        .next()
                        .map(|c| root.join(c.as_os_str()))
                        .unwrap();
                    found.links.push(Link {
                        path: p,
                        meta,
                        owner,
                    });
                }
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
/// Standalone traversal plus policy, for callers without a prepared [`Scan`].
#[cfg(test)]
pub(super) fn links(
    root: &Path,
    profile: Profile,
    writes: &[PathBuf],
    denied: &[PathBuf],
    before_write: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<Vec<PathBuf>> {
    let scan = scan(root, writes, denied)?;
    links_from(&scan, root, profile, before_write)
}
/// Development detaches external aliases; Guarded/ReadOnly keep them read-only.
/// Internal aliases are safe only if every directory entry is writable.
pub(super) fn links_from(
    scan: &Scan,
    root: &Path,
    profile: Profile,
    before_write: &mut dyn FnMut() -> io::Result<()>,
) -> io::Result<Vec<PathBuf>> {
    let mut groups: HashMap<(u64, u64), Vec<Link>> = HashMap::new();
    for link in &scan.links {
        groups
            .entry((link.meta.dev(), link.meta.ino()))
            .or_default()
            .push(link.clone());
    }
    let private_targets = &scan.private_targets;
    for links in groups.values_mut() {
        links.retain(|l| !private_targets.iter().any(|p| l.path.starts_with(p)));
    }
    groups.retain(|_, links| !links.is_empty());
    let ignored = if profile == Profile::Development {
        ignored_directories(root)?
    } else {
        vec![]
    };
    let mut bulky = HashMap::<PathBuf, usize>::new();
    for links in groups.values() {
        if links.len() as u64 != links[0].meta.nlink() {
            for link in links {
                if let Some(p) = ignored.iter().find(|p| link.path.starts_with(p)) {
                    *bulky.entry(p.clone()).or_default() += 1;
                }
            }
        }
    }
    let bulky: BTreeSet<_> = bulky
        .into_iter()
        .filter(|(_, n)| *n > 100_000)
        .map(|(p, _)| p)
        .collect();
    let mut detaches = Vec::new();
    let mut ro = Vec::new();
    let mut by_owner = HashMap::<PathBuf, usize>::new();
    for links in groups.values() {
        if links.len() as u64 == links[0].meta.nlink() {
            continue;
        }
        for l in links {
            if bulky.iter().any(|p| l.path.starts_with(p)) {
                continue;
            }
            if profile == Profile::Development {
                detaches.push(l.clone());
            } else {
                *by_owner.entry(l.owner.clone()).or_default() += 1;
                ro.push(l.path.clone());
            }
        }
    }
    let mut demoted = bulky;
    demoted.extend(
        by_owner
            .into_iter()
            .filter(|(_, n)| *n > 256)
            .map(|(p, _)| p),
    );
    materialize(&detaches, before_write)?;
    let mut overlays: Vec<_> = demoted.iter().cloned().collect();
    overlays.extend(
        ro.into_iter()
            .filter(|p| !demoted.iter().any(|d| p.starts_with(d))),
    );
    Ok(overlays)
}
fn materialize(links: &[Link], before_write: &mut dyn FnMut() -> io::Result<()>) -> io::Result<()> {
    let total = links.iter().try_fold(0u64, |n, l| {
        n.checked_add(l.meta.len())
            .ok_or_else(|| unavailable("command_hardlink_materialization_limit"))
    })?;
    if total > 2 * 1024 * 1024 * 1024 {
        return Err(unavailable("command_hardlink_materialization_limit"));
    }
    if links.is_empty() {
        return Ok(());
    }
    before_write()?;
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut versions = HashMap::new();
    for l in links {
        let parent = pin(l.path.parent().unwrap())?;
        let leaf = CString::new(l.path.file_name().unwrap().as_encoded_bytes()).unwrap();
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                leaf.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(unavailable("command_hardlink_changed"));
        }
        let mut source = unsafe { File::from_raw_fd(fd) };
        let before = source.metadata()?;
        let key = (before.dev(), before.ino());
        if version(&before) != *versions.get(&key).unwrap_or(&version(&l.meta)) {
            return Err(unavailable("command_hardlink_changed"));
        }
        let temp = CString::new(format!(
            ".cortex-unlink-{}",
            crate::native_effects::identity().map_err(|_| unavailable("scratch_identity"))?
        ))
        .unwrap();
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                temp.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                0o600,
            )
        };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut target = unsafe { File::from_raw_fd(fd) };
        let effect = (|| {
            if unsafe { libc::ioctl(target.as_raw_fd(), 0x40049409u64, source.as_raw_fd()) } < 0 {
                let e = io::Error::last_os_error();
                if ![
                    Some(libc::EXDEV),
                    Some(libc::EOPNOTSUPP),
                    Some(libc::ENOTTY),
                    Some(libc::EINVAL),
                ]
                .contains(&e.raw_os_error())
                {
                    return Err(e);
                }
                let mut remaining = before.len();
                let mut buf = vec![0; 1024 * 1024];
                while remaining > 0 {
                    if Instant::now() > deadline {
                        return Err(unavailable("command_hardlink_materialization_timeout"));
                    }
                    let cap = remaining.min(buf.len() as u64) as usize;
                    let n = source.read(&mut buf[..cap])?;
                    if n == 0 {
                        return Err(unavailable("command_hardlink_changed"));
                    }
                    target.write_all(&buf[..n])?;
                    remaining -= n as u64;
                }
            }
            let m = target.metadata()?;
            if (m.uid(), m.gid()) != (before.uid(), before.gid())
                && unsafe { libc::fchown(target.as_raw_fd(), before.uid(), before.gid()) } != 0
            {
                return Err(io::Error::last_os_error());
            }
            copy_xattrs(&source, &target)?;
            target.set_permissions(fs::Permissions::from_mode(before.mode() & 0o7777))?;
            let times = [
                libc::timespec {
                    tv_sec: before.atime(),
                    tv_nsec: before.atime_nsec(),
                },
                libc::timespec {
                    tv_sec: before.mtime(),
                    tv_nsec: before.mtime_nsec(),
                },
            ];
            if unsafe { libc::futimens(target.as_raw_fd(), times.as_ptr()) } != 0 {
                return Err(io::Error::last_os_error());
            }
            target.sync_all()?;
            let current = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_NOFOLLOW)
                .open(format!(
                    "/proc/self/fd/{}/{}",
                    parent.as_raw_fd(),
                    leaf.to_string_lossy()
                ))?
                .metadata()?;
            if version(&current) != version(&before)
                || version(&source.metadata()?) != version(&before)
            {
                return Err(unavailable("command_hardlink_changed"));
            }
            if Instant::now() > deadline {
                return Err(unavailable("command_hardlink_materialization_timeout"));
            }
            if unsafe {
                libc::renameat(
                    parent.as_raw_fd(),
                    temp.as_ptr(),
                    parent.as_raw_fd(),
                    leaf.as_ptr(),
                )
            } != 0
            {
                return Err(io::Error::last_os_error());
            }
            // Sync an actual directory handle; O_PATH cannot fsync.
            File::open(format!("/proc/self/fd/{}", parent.as_raw_fd()))?.sync_all()?;
            versions.insert(key, version(&source.metadata()?));
            Ok(())
        })();
        if effect.is_err() {
            unsafe {
                libc::unlinkat(parent.as_raw_fd(), temp.as_ptr(), 0);
            }
        }
        effect?;
    }
    Ok(())
}
fn copy_xattrs(source: &File, target: &File) -> io::Result<()> {
    let n = unsafe { libc::flistxattr(source.as_raw_fd(), std::ptr::null_mut(), 0) };
    if n < 0 {
        return Err(io::Error::last_os_error());
    }
    let mut names = vec![0u8; n as usize];
    if n > 0
        && unsafe { libc::flistxattr(source.as_raw_fd(), names.as_mut_ptr().cast(), names.len()) }
            != n
    {
        return Err(unavailable("command_hardlink_changed"));
    }
    for name in names.split(|b| *b == 0).filter(|n| !n.is_empty()) {
        let name = CString::new(name).unwrap();
        let n =
            unsafe { libc::fgetxattr(source.as_raw_fd(), name.as_ptr(), std::ptr::null_mut(), 0) };
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut bytes = vec![0u8; n as usize];
        if unsafe {
            libc::fgetxattr(
                source.as_raw_fd(),
                name.as_ptr(),
                bytes.as_mut_ptr().cast(),
                bytes.len(),
            )
        } != n
            || unsafe {
                libc::fsetxattr(
                    target.as_raw_fd(),
                    name.as_ptr(),
                    bytes.as_ptr().cast(),
                    bytes.len(),
                    0,
                )
            } != 0
        {
            return Err(unavailable("command_hardlink_changed"));
        }
    }
    Ok(())
}
