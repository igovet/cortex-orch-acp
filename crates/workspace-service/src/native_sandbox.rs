//! Landlock host boundary for worker commands (Guarded, Development and the
//! read-only roles) and contained external MCP servers.
//!
//! The process keeps the real filesystem at its real paths, the owner's tools,
//! PATH and HOME, and the shared network. No mount, PID, user or network
//! namespace is created and nothing is copied. Landlock only restricts:
//! - writes (create, change, truncate, remove, rename, link) to the granted
//!   trees, and
//! - every access (read, list below, execute, write) to a private/secret set,
//!   when the policy names one.
//!
//! Landlock is allow-list only. A directory that directly holds a hidden or
//! read-only entry therefore cannot carry a hierarchical grant of its own: its
//! existing children are granted one by one and the directory keeps only the
//! rights that cannot reach the protected entry (listing, execution, creating
//! entries). Entry names below such a directory stay listable. A hidden entry or
//! a Cortex store path inside an open write tree (a writer's checkout) is not
//! protected at all (see [`Sandbox::host`]); an explicit read-only entry there
//! (an owner write denial) is, with that cost for its parent directories. Landlock does not restrict chmod/chown,
//! already open descriptors, Unix sockets or signals; it is a write and secret
//! boundary for trusted local work, not hostile-code isolation.
use std::{
    ffi::{CString, OsString},
    fs::File,
    io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    path::{Path, PathBuf},
    sync::Arc,
};

const EXECUTE: u64 = 1 << 0;
const WRITE_FILE: u64 = 1 << 1;
const READ_FILE: u64 = 1 << 2;
const READ_DIR: u64 = 1 << 3;
const MAKE_DIR: u64 = 1 << 7;
const MAKE_REG: u64 = 1 << 8;
const MAKE_SOCK: u64 = 1 << 9;
const MAKE_FIFO: u64 = 1 << 10;
const MAKE_SYM: u64 = 1 << 12;
const REFER: u64 = 1 << 13;
const TRUNCATE: u64 = 1 << 14;
const IOCTL_DEV: u64 = 1 << 15;
const READ: u64 = EXECUTE | READ_FILE | READ_DIR;
/// Rights Landlock accepts on a non-directory rule.
const FILE_RIGHTS: u64 = EXECUTE | WRITE_FILE | READ_FILE | TRUNCATE | IOCTL_DEV;
/// Write rights a directory holding a protected entry may still carry: they let a
/// command create entries there, never change, remove or replace existing ones.
const CREATE: u64 = MAKE_REG | MAKE_DIR | MAKE_SYM | MAKE_SOCK | MAKE_FIFO | REFER;
/// Bound on the project walk that finds secret files.
const PROJECT_ENTRIES: usize = 2_000_000;

#[cfg(test)]
thread_local! {
    /// Per-thread count of contained-server preparations, observed by unit tests only.
    pub(crate) static WALKS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn unavailable(reason: &str) -> io::Error {
    io::Error::other(format!("native_command_sandbox_unavailable: {reason}"))
}

/// The Landlock ABI of the running kernel (0 when Landlock is absent or disabled).
pub(crate) fn abi() -> i64 {
    let v = unsafe {
        libc::syscall(
            libc::SYS_landlock_create_ruleset,
            std::ptr::null::<u8>(),
            0usize,
            1u32,
        )
    };
    v.max(0)
}

fn supported(abi: i64) -> u64 {
    let mut all = (1 << 13) - 1;
    if abi >= 2 {
        all |= REFER;
    }
    if abi >= 3 {
        all |= TRUNCATE;
    }
    if abi >= 5 {
        all |= IOCTL_DEV;
    }
    all
}

/// What one process may write and which private paths it may not touch at all.
#[derive(Clone, Debug, Default)]
pub(crate) struct HostPolicy {
    /// Trees the process may change: create, write, truncate, remove and rename in.
    /// Hidden and read-only entries inside them stay protected.
    pub writes: Vec<PathBuf>,
    /// Trees the process may change as a whole (a writer's checkout): hidden and
    /// read-only entries inside them are not protected by Landlock.
    pub open_writes: Vec<PathBuf>,
    /// Trees that stay unchangeable even inside `writes` (they remain readable).
    pub read_only: Vec<PathBuf>,
    /// Trees the process may neither read, list below, execute nor change.
    pub hidden: Vec<PathBuf>,
}

#[repr(C)]
struct Ruleset {
    fs: u64,
}
#[repr(C, packed)]
struct PathRule {
    access: u64,
    parent: i32,
}

/// A prepared Landlock ruleset, entered in the child between fork and exec.
pub struct Sandbox {
    rules: OwnedFd,
    handled: u64,
}

/// Existing paths, resolved to the objects Landlock rules attach to.
fn canonical(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = paths
        .iter()
        .filter(|p| p.is_absolute())
        .filter_map(|p| p.canonicalize().ok())
        .collect();
    out.sort();
    out.dedup();
    out
}

impl Sandbox {
    /// Prepares the ruleset for `policy`. Fails closed when the kernel has no Landlock.
    pub(crate) fn host(policy: &HostPolicy) -> io::Result<Self> {
        let abi = abi();
        if abi < 1 {
            return Err(unavailable("landlock_unavailable"));
        }
        let all = supported(abi);
        let write = all & !READ;
        let open = canonical(&policy.open_writes);
        // Landlock is allow-list only: an entry inside an open write tree could be
        // protected only by withholding the tree's own grant, which would make every
        // file created directly in it (the checkout root next to `.cortex` or
        // `.env`) unusable. Such entries are not protected here; the callers' other
        // guards (the live-store argv seal, deletion consent) still apply.
        let inside = |p: &PathBuf| open.iter().any(|w| p.starts_with(w));
        let mut hidden = canonical(&policy.hidden);
        hidden.retain(|p| !inside(p));
        // Explicit read-only entries (owner write denials) stay protected even inside
        // an open tree, at the cost described in the module documentation for their
        // parent directories; Cortex store paths there are left to the argv seal.
        let in_store = |p: &PathBuf| {
            open.iter().any(|w| {
                p.strip_prefix(w).is_ok_and(|r| {
                    r.components().any(|c| c.as_os_str() == ".cortex" || c.as_os_str() == ".cortex-dev")
                })
            })
        };
        let read_only: Vec<PathBuf> = canonical(&policy.read_only)
            .into_iter()
            .filter(|p| !inside(p) || !in_store(p))
            .collect();
        let narrowed: Vec<PathBuf> = read_only.iter().filter(|p| inside(p)).cloned().collect();
        // An open write tree inside a hidden one stays hidden.
        let open: Vec<PathBuf> = open
            .iter()
            .filter(|w| !hidden.iter().any(|h| w.starts_with(h)))
            .cloned()
            .collect();
        // Without a hidden set reads are not handled at all: every path stays readable.
        let handled = if hidden.is_empty() { write } else { all };
        let ruleset = Ruleset { fs: handled };
        let fd = unsafe {
            libc::syscall(
                libc::SYS_landlock_create_ruleset,
                &ruleset,
                std::mem::size_of::<Ruleset>(),
                0u32,
            )
        };
        if fd < 0 {
            return Err(unavailable("landlock_unavailable"));
        }
        let s = Self {
            rules: unsafe { OwnedFd::from_raw_fd(fd as i32) },
            handled,
        };
        if !hidden.is_empty() {
            s.grant_except(Path::new("/"), READ, &hidden, READ_DIR | EXECUTE)?;
        }
        let mut fixed = hidden.clone();
        fixed.extend(read_only.iter().cloned());
        for path in canonical(&policy.writes) {
            s.grant_except(&path, write, &fixed, write & CREATE)?;
        }
        for path in open {
            s.grant_except(&path, write, &narrowed, write & CREATE)?;
        }
        Ok(s)
    }

    /// Grants `rights` on `path` and everything below it, except below `excluded`.
    /// A directory holding an excluded entry keeps only `ancestor` rights and its
    /// other children are granted individually.
    fn grant_except(
        &self,
        path: &Path,
        rights: u64,
        excluded: &[PathBuf],
        ancestor: u64,
    ) -> io::Result<()> {
        if excluded.iter().any(|x| path.starts_with(x)) {
            return Ok(());
        }
        let inner: Vec<PathBuf> = excluded
            .iter()
            .filter(|x| x.starts_with(path))
            .cloned()
            .collect();
        if inner.is_empty() {
            return self.allow(path, rights);
        }
        self.allow(path, ancestor)?;
        let entries = match std::fs::read_dir(path) {
            Ok(e) => e,
            // The owner's own permissions already deny this directory: no rule
            // grants anything below it, and nothing is refused.
            Err(e) if matches!(e.raw_os_error(), Some(libc::EACCES | libc::ENOENT)) => {
                return Ok(())
            }
            Err(e) => return Err(e),
        };
        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) if matches!(e.raw_os_error(), Some(libc::EACCES | libc::ENOENT)) => {
                    continue
                }
                Err(e) => return Err(e),
            };
            // A symlink is resolved through its target's own rules.
            if entry.file_type().is_ok_and(|t| t.is_symlink()) {
                continue;
            }
            self.grant_except(&entry.path(), rights, &inner, ancestor)?;
        }
        Ok(())
    }

    fn allow(&self, path: &Path, access: u64) -> io::Result<()> {
        let c = CString::new(path.as_os_str().as_bytes())?;
        let fd = unsafe { libc::open(c.as_ptr(), libc::O_PATH | libc::O_CLOEXEC) };
        if fd < 0 {
            let e = io::Error::last_os_error();
            // Removed while preparing, or unreachable with the owner's permissions.
            if matches!(e.raw_os_error(), Some(libc::EACCES | libc::ENOENT)) {
                return Ok(());
            }
            return Err(e);
        }
        let file = unsafe { File::from_raw_fd(fd) };
        let mut access = access & self.handled;
        if !file.metadata()?.is_dir() {
            access &= FILE_RIGHTS;
        }
        if access == 0 {
            return Ok(());
        }
        let rule = PathRule {
            access,
            parent: file.as_raw_fd(),
        };
        if unsafe {
            libc::syscall(
                libc::SYS_landlock_add_rule,
                self.rules.as_raw_fd(),
                1,
                &rule,
                0,
            )
        } < 0
        {
            return Err(unavailable("landlock_rule_failed"));
        }
        Ok(())
    }

    /// Only called in Command::pre_exec. No allocation or locks after fork.
    pub unsafe fn enter(&self) -> io::Result<()> {
        if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if unsafe { libc::syscall(libc::SYS_landlock_restrict_self, self.rules.as_raw_fd(), 0) } < 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// A contained external MCP server: the real filesystem readable except the
    /// owner's secrets, Cortex stores and the project's secret files; writes only
    /// to its explicit grants and its private runtime directory. The network
    /// stays shared.
    pub fn external(
        root: &Path,
        home: &Path,
        runtime: &Path,
        writes: &[PathBuf],
    ) -> io::Result<Self> {
        #[cfg(test)]
        WALKS.with(|w| w.set(w.get() + 1));
        let mut hidden = secrets(Some(home), &std::env::vars_os().collect());
        hidden.extend(live_stores(root));
        hidden.extend(indexed_project_secrets(root)?);
        // The server's private runtime directory may sit inside a hidden store
        // directory: hide that directory's other entries instead, so the grant is usable.
        let mut split = Vec::new();
        hidden.retain(|h| {
            if !runtime.starts_with(h) || runtime == h {
                return true;
            }
            let mut dir = h.clone();
            while let Ok(entries) = std::fs::read_dir(&dir) {
                let next = entries.flatten().map(|e| e.path()).fold(None, |next, p| {
                    if runtime.starts_with(&p) {
                        Some(p)
                    } else {
                        split.push(p);
                        next
                    }
                });
                match next {
                    Some(p) if p != runtime => dir = p,
                    _ => break,
                }
            }
            false
        });
        hidden.extend(split);
        let mut granted = writes.to_vec();
        granted.push(runtime.to_path_buf());
        granted.extend(devices());
        Self::host(&HostPolicy {
            writes: granted,
            hidden,
            ..Default::default()
        })
    }
}

/// Device and pseudo-file trees every confined process writes (`/dev/null`, ttys,
/// shared memory, `/proc/self`). Device permissions still apply.
pub(crate) fn devices() -> Vec<PathBuf> {
    ["/dev", "/proc"].map(PathBuf::from).to_vec()
}

/// The owner's private keys and credential stores below HOME (and the engines'
/// configured homes): never readable by a confined process. SSH configuration,
/// known hosts and public keys stay readable; the SSH agent socket is not a file grant.
pub(crate) fn secrets(
    home: Option<&Path>,
    env: &std::collections::BTreeMap<OsString, OsString>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let var = |k: &str| {
        env.get(std::ffi::OsStr::new(k))
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    if let Some(home) = home.filter(|h| h.is_absolute()) {
        let ssh = home.join(".ssh");
        if let Ok(entries) = std::fs::read_dir(&ssh) {
            for e in entries.flatten() {
                let name = e.file_name();
                let name = name.to_string_lossy();
                let public = name.ends_with(".pub")
                    || name == "config"
                    || name.starts_with("known_hosts")
                    || name.starts_with("authorized_keys")
                    || name == "config.d";
                if !public {
                    out.push(e.path());
                }
            }
        }
        out.extend(
            [
                ".gnupg/private-keys-v1.d",
                ".gnupg/secring.gpg",
                ".aws/credentials",
                ".aws/sso/cache",
                ".azure",
                ".kube/config",
                ".docker/config.json",
                ".password-store",
                ".local/share/keyrings",
                ".netrc",
                ".git-credentials",
                ".npmrc",
                ".pypirc",
                ".cargo/credentials",
                ".cargo/credentials.toml",
                ".config/gh/hosts.yml",
                ".config/hub",
                ".config/gcloud",
                ".codex/auth.json",
                ".claude/.credentials.json",
                ".claude.json",
                ".local/share/opencode/auth.json",
                ".config/cortex",
                ".local/state/cortex",
                ".paseo",
                ".mozilla",
                ".thunderbird",
                ".config/google-chrome",
                ".config/chromium",
                ".config/BraveSoftware",
            ]
            .map(|n| home.join(n)),
        );
    }
    if let Some(p) = var("CODEX_HOME") {
        out.push(p.join("auth.json"));
    }
    if let Some(p) = var("CLAUDE_CONFIG_DIR") {
        out.push(p.join(".credentials.json"));
    }
    if let Some(p) = var("XDG_CONFIG_HOME") {
        out.extend(["cortex", "gh/hosts.yml"].map(|n| p.join(n)));
    }
    if let Some(p) = var("XDG_STATE_HOME") {
        out.push(p.join("cortex"));
    }
    out
}

/// The project's live Cortex stores and private Dev registrations, at the logical
/// project root (a linked worktree resolves to its main checkout).
pub(crate) fn live_stores(root: &Path) -> Vec<PathBuf> {
    let logical = cortex_core::ProjectKey::new(root, cortex_core::StoreChannel::Release)
        .map(|k| k.root().to_path_buf())
        .unwrap_or_else(|_| root.to_path_buf());
    let mut out = Vec::new();
    for base in [logical.as_path(), root] {
        out.extend(
            [
                ".cortex",
                ".cortex-dev",
                ".cortex-dev-owner.json",
                ".cortex-workspace-dev.json",
            ]
            .map(|n| base.join(n)),
        );
    }
    out.sort();
    out.dedup();
    out
}

/// A secret file by name: `.env` and `.env.*` (except example, sample and
/// template files), package-manager and Git credential files. Classified only on
/// names below the checkout root, never on the checkout's own absolute path.
pub(crate) fn secret_name(name: &std::ffi::OsStr) -> bool {
    let n = name.as_bytes();
    (n == b".env"
        || n.starts_with(b".env.")
            && ![b".example".as_slice(), b".sample", b".template", b".dist"]
                .iter()
                .any(|s| n.ends_with(s)))
        || [
            b".netrc".as_slice(),
            b".npmrc",
            b".pypirc",
            b".git-credentials",
            b".credentials.json",
        ]
        .contains(&n)
}

/// Secret files inside the checkout. The walk skips Git metadata, Cortex state and
/// bulk build/dependency trees, and fails closed beyond a fixed entry bound.
pub(crate) fn project_secrets(root: &Path) -> io::Result<Vec<PathBuf>> {
    const SKIP: &[&str] = &[
        ".git",
        ".cortex",
        ".cortex-dev",
        "target",
        "node_modules",
        ".venv",
        "venv",
        ".workspace-build",
    ];
    let mut out = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    let mut count = 0;
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            count += 1;
            if count > PROJECT_ENTRIES {
                return Err(unavailable("project_tree_limit"));
            }
            let name = e.file_name();
            let Ok(kind) = e.file_type() else { continue };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                if !SKIP.iter().any(|s| name == *s) {
                    pending.push(e.path());
                }
            } else if secret_name(&name) {
                out.push(e.path());
            }
        }
    }
    Ok(out)
}

/// Secret files of the checkout's top level only: one directory read, never a walk.
pub(crate) fn top_secrets(root: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(root)
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir() && !t.is_symlink()))
                .filter(|e| secret_name(&e.file_name()))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default()
}

/// The last `project_secrets` walk of each checkout in this process, and whether a
/// refresh is running.
type SecretIndex = std::collections::HashMap<PathBuf, (Arc<Vec<PathBuf>>, bool)>;
static SECRET_INDEX: std::sync::Mutex<Option<SecretIndex>> = std::sync::Mutex::new(None);

/// `project_secrets` without a walk on the caller's path: the checkout's last walk in
/// this process plus a fresh read of its top level, and a refresh of the walk in the
/// background for the next caller. Only the first call for a checkout in a process
/// walks before it returns (and fails closed as `project_secrets` does). A nested
/// secret file created after the last finished walk is hidden from the command after
/// the next one.
pub(crate) fn indexed_project_secrets(root: &Path) -> io::Result<Vec<PathBuf>> {
    let cached = {
        let mut index = SECRET_INDEX.lock().unwrap_or_else(|e| e.into_inner());
        let index = index.get_or_insert_with(Default::default);
        match index.get_mut(root) {
            Some((list, refreshing)) => {
                let start = !*refreshing;
                *refreshing = true;
                Some((list.clone(), start))
            }
            None => None,
        }
    };
    let Some((list, start)) = cached else {
        let list = project_secrets(root)?;
        let mut index = SECRET_INDEX.lock().unwrap_or_else(|e| e.into_inner());
        index
            .get_or_insert_with(Default::default)
            .insert(root.to_path_buf(), (Arc::new(list.clone()), false));
        return Ok(list);
    };
    if start {
        let root = root.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("cortex-secret-index".into())
            .spawn({
                let root = root.clone();
                move || {
                    let walked = project_secrets(&root);
                    let mut index = SECRET_INDEX.lock().unwrap_or_else(|e| e.into_inner());
                    let index = index.get_or_insert_with(Default::default);
                    match walked {
                        Ok(list) => {
                            index.insert(root, (Arc::new(list), false));
                        }
                        // A walk that fails closed is not kept: the next caller walks.
                        Err(_) => {
                            index.remove(&root);
                        }
                    }
                }
            });
        if spawned.is_err() {
            let mut index = SECRET_INDEX.lock().unwrap_or_else(|e| e.into_inner());
            index.get_or_insert_with(Default::default).remove(&root);
        }
    }
    let mut out = (*list).clone();
    out.extend(top_secrets(root));
    out.sort();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::process::CommandExt;

    /// Runs `script` with the real `/bin/sh` under `policy`. `None` when the kernel
    /// has no Landlock, so the caller reports the check as not run.
    pub(crate) fn run(policy: &HostPolicy, script: &str, args: &[&Path]) -> Option<bool> {
        let sandbox = match Sandbox::host(policy) {
            Ok(s) => std::sync::Arc::new(s),
            Err(e) => {
                assert!(e.to_string().contains("landlock_unavailable"), "{e}");
                return None;
            }
        };
        let mut command = std::process::Command::new("/bin/sh");
        command.arg("-c").arg(script).arg("probe").args(args);
        let entered = sandbox.clone();
        unsafe {
            command.pre_exec(move || entered.enter());
        }
        Some(command.status().unwrap().success())
    }

    pub(crate) fn fixture(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "landlock-{name}-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir_all(p.join("project/src")).unwrap();
        std::fs::create_dir_all(p.join("project/.cortex")).unwrap();
        std::fs::create_dir_all(p.join("home/.ssh")).unwrap();
        std::fs::create_dir_all(p.join("outside")).unwrap();
        std::fs::write(p.join("project/src/lib.rs"), "fn a() {}").unwrap();
        std::fs::write(p.join("project/.env"), "TOKEN=x").unwrap();
        std::fs::write(p.join("project/.cortex/cortex.sqlite3"), "db").unwrap();
        std::fs::write(p.join("home/.ssh/id_ed25519"), "key").unwrap();
        std::fs::write(p.join("home/.ssh/id_ed25519.pub"), "pub").unwrap();
        std::fs::write(p.join("home/.ssh/known_hosts"), "hosts").unwrap();
        p
    }

    #[test]
    fn writes_outside_the_scope_are_denied_and_secrets_are_hidden() {
        let f = fixture("scope");
        let (project, home) = (f.join("project"), f.join("home"));
        let mut hidden = secrets(Some(&home), &Default::default());
        hidden.extend(live_stores(&project));
        hidden.extend(project_secrets(&project).unwrap());
        assert!(hidden.contains(&project.join(".env")));
        assert!(!hidden.contains(&home.join(".ssh/known_hosts")));
        let mut writes = vec![project.join("src")];
        writes.extend(devices());
        let policy = HostPolicy { writes, hidden, ..Default::default() };
        let result = run(
            &policy,
            r#"set -e
deny() { if "$@" 2>/dev/null; then exit 9; fi; }
put() { echo x > "$1"; }
add() { echo x >> "$1"; }
put "$1/src/new.rs"; cat "$1/src/new.rs" >/dev/null
deny put "$3/x"
deny cat "$1/.env"
deny cat "$1/.cortex/cortex.sqlite3"
deny add "$1/.cortex/cortex.sqlite3"
deny cat "$2/.ssh/id_ed25519"
cat "$2/.ssh/id_ed25519.pub" "$2/.ssh/known_hosts" /etc/hosts >/dev/null
/usr/bin/env true; ls "$1" >/dev/null"#,
            &[&project, &home, &f.join("outside")],
        );
        std::fs::remove_dir_all(&f).unwrap();
        match result {
            Some(ok) => assert!(ok, "Landlock rules were not enforced as granted"),
            None => eprintln!("Landlock unavailable on this host: rule check not run"),
        }
    }

    /// After the first walk of a checkout, callers get its secret files without one:
    /// the last walk plus a fresh read of the top level, while the walk is refreshed
    /// in the background for the next caller.
    #[test]
    fn indexed_secrets_need_no_walk_after_the_first() {
        let f = fixture("index");
        let project = f.join("project").canonicalize().unwrap();
        std::fs::create_dir_all(project.join("svc")).unwrap();
        std::fs::write(project.join("svc/.env"), "TOKEN=x").unwrap();
        let first = indexed_project_secrets(&project).unwrap();
        assert!(first.contains(&project.join(".env")) && first.contains(&project.join("svc/.env")));
        std::fs::create_dir_all(project.join("api")).unwrap();
        std::fs::write(project.join("api/.netrc"), "machine x").unwrap();
        std::fs::write(project.join(".env.local"), "TOKEN=y").unwrap();
        let second = indexed_project_secrets(&project).unwrap();
        // The new top-level secret is hidden at once; the nested one after a refresh.
        assert!(second.contains(&project.join(".env.local")), "{second:?}");
        assert!(second.contains(&project.join("svc/.env")));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if indexed_project_secrets(&project).unwrap().contains(&project.join("api/.netrc")) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "the background refresh never landed");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(top_secrets(&project).len(), 2);
        std::fs::remove_dir_all(&f).unwrap();
    }

    #[test]
    fn without_a_hidden_set_every_path_stays_readable() {
        let f = fixture("reads");
        let project = f.join("project");
        let mut writes = vec![project.clone()];
        writes.extend(devices());
        let policy = HostPolicy {
            writes,
            read_only: vec![project.join(".cortex")],
            ..Default::default()
        };
        let result = run(
            &policy,
            r#"set -e
deny() { if "$@" 2>/dev/null; then exit 9; fi; }
put() { echo x > "$1"; }
add() { echo x >> "$1"; }
cat "$1/.cortex/cortex.sqlite3" "$2/.ssh/id_ed25519" >/dev/null
deny add "$1/.cortex/cortex.sqlite3"
add "$1/src/lib.rs"; echo z > /dev/null
deny put "$3/x""#,
            &[&project, &f.join("home"), &f.join("outside")],
        );
        std::fs::remove_dir_all(&f).unwrap();
        match result {
            Some(ok) => assert!(ok, "Landlock rules were not enforced as granted"),
            None => eprintln!("Landlock unavailable on this host: rule check not run"),
        }
    }

    #[test]
    fn a_worktree_path_containing_cortex_is_not_itself_private() {
        // Only names below the checkout root are classified, never its absolute path.
        let f = fixture("worktree");
        let worktree = f.join(".cortex/worktrees/task/checkout");
        std::fs::create_dir_all(worktree.join("src")).unwrap();
        std::fs::write(worktree.join("src/main.rs"), "fn main() {}").unwrap();
        assert!(project_secrets(&worktree).unwrap().is_empty());
        let mut writes = vec![worktree.clone()];
        writes.extend(devices());
        let policy = HostPolicy {
            writes,
            hidden: secrets(Some(&f.join("home")), &Default::default()),
            ..Default::default()
        };
        let result = run(
            &policy,
            r#"cat "$1/src/main.rs" >/dev/null && mkdir "$1/src/new" && echo x > "$1/src/new/x.rs""#,
            &[&worktree],
        );
        std::fs::remove_dir_all(&f).unwrap();
        if let Some(ok) = result {
            assert!(ok);
        }
    }
}
