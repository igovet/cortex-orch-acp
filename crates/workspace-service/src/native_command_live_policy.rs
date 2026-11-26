//! Command policy shared by the Landlock command profiles: control paths, the
//! owner's tool caches, project-relative denial patterns and Git directories.
use super::*;
use std::{
    collections::BTreeSet,
    ffi::{CString, OsStr},
    fs::{File, OpenOptions},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
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
pub(super) fn env_path(env: &BTreeMap<OsString, OsString>, key: &str) -> Option<PathBuf> {
    env.get(OsStr::new(key))
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}
pub(crate) fn controls(
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
/// A private or secret entry name anywhere in a checkout: Git and Cortex state,
/// engine and key directories, credential files, `.env` files and live databases.
pub(crate) fn private(name: &OsStr) -> bool {
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
        ".gitconfig",
        ".claude.json",
        "cortex.sqlite3",
        "cortex.sqlite3-wal",
        "cortex.sqlite3-shm",
        "cortex.sqlite3-journal",
        "opencode.db",
        "opencode.db-wal",
        "opencode.db-shm",
        "opencode.db-journal",
    ]
    .iter()
    .any(|n| name == *n)
        || crate::native_sandbox::secret_name(name)
}


/// The owner's tool and cache directories a profile may write: for Guarded and
/// Development every non-sensitive dot directory of HOME (and `go`), the
/// application data below `~/.local/share` (not keyrings or desktop
/// registrations) and `~/.local/state`; for the read-only roles only the
/// package and build caches.
pub(super) fn tool_caches(
    env: &BTreeMap<OsString, OsString>,
    root: &Path,
    profile: Profile,
) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let home = env_path(env, "HOME").filter(|p| p.is_absolute() && p.is_dir());
    let relocated: Vec<PathBuf> = CACHE_ENV
        .iter()
        .filter_map(|key| env_path(env, key))
        .filter(|p| p.is_absolute() && p.is_dir())
        .collect();
    let Some(home) = home else {
        if profile == Profile::ReadOnly {
            out.extend(relocated);
        }
        return Ok(out);
    };
    if profile == Profile::ReadOnly {
        out.extend(CACHES.iter().map(|n| home.join(n)));
        out.extend(relocated.into_iter().filter(|p| {
            !SENSITIVE.iter().any(|n| {
                let s = home.join(n);
                p.starts_with(&s) || s.starts_with(p)
            })
        }));
    } else {
        for e in fs::read_dir(&home)? {
            let e = e?;
            let name = e.file_name();
            let name = name.to_string_lossy();
            if (name.starts_with('.') || name == "go")
                && !SENSITIVE.contains(&name.as_ref())
                && e.file_type()?.is_dir()
            {
                out.push(e.path());
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
                    out.push(e.path());
                }
            }
        }
        out.push(home.join(".local/state"));
        out.extend(relocated);
    }
    // A cache that is, contains or sits in the checkout is not a separate grant.
    let real_root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    out.retain(|p| {
        p.canonicalize().is_ok_and(|real| {
            real != Path::new("/") && !real.starts_with(&real_root) && !real_root.starts_with(&real)
                && home.canonicalize().map(|h| h != real && !h.starts_with(&real)).unwrap_or(true)
        })
    });
    Ok(out)
}

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
/// The git directories of `root` read from the files git itself uses (`.git` as a
/// directory, or a linked worktree's `gitdir:` pointer and its `commondir`), for when
/// `git rev-parse` cannot answer (for example a foreign-owner checkout). Read-only:
/// nothing is executed and every returned directory is pinned by the caller.
pub(super) fn parsed_git_dirs(root: &Path) -> io::Result<Vec<PathBuf>> {
    let dot = root.join(".git");
    let absolute = |base: &Path, text: &str| -> io::Result<PathBuf> {
        let p = PathBuf::from(text.trim());
        if p.is_absolute() { p } else { base.join(p) }.canonicalize()
    };
    let meta = dot.symlink_metadata()?;
    let git = if meta.is_dir() {
        dot.canonicalize()?
    } else if meta.is_file() {
        let text = fs::read_to_string(&dot)?;
        let pointer = text
            .lines()
            .find_map(|l| l.strip_prefix("gitdir:"))
            .ok_or_else(|| unavailable("git_context_unavailable"))?;
        absolute(root, pointer)?
    } else {
        return Err(unavailable("git_context_unavailable"));
    };
    let mut dirs = vec![git.clone()];
    if let Ok(common) = fs::read_to_string(git.join("commondir")) {
        let common = absolute(&git, &common)?;
        if !dirs.contains(&common) {
            dirs.push(common);
        }
    }
    Ok(dirs)
}
pub(crate) fn git_dirs(root: &Path) -> io::Result<Vec<PathBuf>> {
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
        let Some(text) = query(root, &["rev-parse", key])? else {
            // git refused (ownership, damaged config): read the same facts from its files.
            dirs = parsed_git_dirs(root)?;
            for p in &dirs {
                pin(p)?;
            }
            return Ok(dirs);
        };
        let p = PathBuf::from(text);
        let p = if p.is_absolute() { p } else { root.join(p) }.canonicalize()?;
        pin(&p)?;
        if !dirs.contains(&p) {
            dirs.push(p);
        }
    }
    Ok(dirs)
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
