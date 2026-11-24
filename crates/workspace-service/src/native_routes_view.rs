//! Per-child filesystem views for native engines and the helpers that start an
//! engine executable (Bubblewrap mount and PID namespaces).
//!
//! A view shows only: the system runtime (`/usr`, the loader cache, TLS roots and
//! name-resolution files), the engine's own install, Cortex's own executable, a
//! private HOME at the owner's HOME path (a fresh tmpfs per launch with the
//! connection's persistent private cache at `~/.cache`), the engine's native
//! sign-in in place, the working directory read-only with the command policy's
//! private masks, private `/tmp`, and the session's broker directories. It never
//! shows the rest of the owner's home, other projects, Cortex stores or other
//! sessions' brokers; descendants inherit it. The network and IPC namespaces stay
//! shared, as before. A view that cannot be prepared refuses the launch: there is
//! no unconfined fallback.
//!
//! Native sign-in stays where the engine keeps it and is never copied:
//! - Codex: a private `CODEX_HOME` at the native path, with the native
//!   `auth.json` and its `auth.lock` bound in (Codex rewrites `auth.json` in place
//!   under that lock) and `config.toml` read-only.
//! - Claude: the native config directory itself (its refresh lock directory and
//!   staged credential replacement live there), with every other existing entry
//!   replaced by a private per-connection substitute.
//! - OpenCode (engine and catalogue only): its data directory itself, for the
//!   combined sign-in/history database and its SQLite companions, with every other
//!   existing entry replaced by a private substitute; its own configuration files
//!   read-only.
//!
//! Substitutes keep the names of the entries they replace visible (contents and
//! changes are private); entries created after the view starts are the engine's
//! own and land in the real directory.
use cortex_engine::{
    environment::{ConfinementRequest, EngineKind, LaunchSpec},
    filesystem_view::{self, Confinement, Confiner},
};
use std::{
    collections::BTreeMap,
    ffi::{OsStr, OsString},
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

const BWRAP: [&str; 2] = ["/usr/bin/bwrap", "/bin/bwrap"];
const TMP_BYTES: &str = "2147483648";
/// Exact system files, each optional. Symlinked ones are bound from their target.
const SYSTEM_FILES: &[&str] = &[
    "/etc/ld.so.cache",
    "/etc/resolv.conf",
    "/etc/hosts",
    "/etc/host.conf",
    "/etc/gai.conf",
    "/etc/nsswitch.conf",
    "/etc/passwd",
    "/etc/group",
    "/etc/localtime",
    "/etc/os-release",
    "/etc/gitconfig",
];
/// TLS trust stores (distribution layouts) and the alternatives links that
/// `/usr/bin` names resolve through; each optional.
const SYSTEM_DIRECTORIES: &[&str] = &["/etc/ssl", "/etc/pki", "/etc/alternatives"];
/// Native Claude sign-in entries kept in place: the credentials, their staged
/// replacements and the refresh lock directory with its holder record.
const CLAUDE_KEEP: &[&str] = &[".credentials.json", ".oauth_refresh.lock"];
/// OpenCode's combined sign-in/history database, its SQLite companions and the
/// older separate sign-in file.
const OPENCODE_KEEP: &[&str] = &[
    "opencode.db",
    "opencode.db-wal",
    "opencode.db-shm",
    "opencode.db-journal",
    "auth.json",
];
const OPENCODE_CONFIG: &[&str] = &["opencode.json", "opencode.jsonc", "config.json"];

fn unavailable(reason: &str) -> io::Error {
    io::Error::other(format!("engine_confinement_unavailable: {reason}"))
}
fn uid() -> u32 {
    unsafe { libc::getuid() }
}
fn normal(path: &Path) -> bool {
    path.is_absolute()
        && !path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
}

/// Cortex's private per-connection engine state: the engine's `~/.cache`, its
/// private `CODEX_HOME` and the substitutes of hidden account entries. Never
/// inside a project.
pub(crate) fn private_root(state: &Path, connection: &str) -> io::Result<PathBuf> {
    if connection.is_empty()
        || connection.len() > 64
        || !connection
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
        || !normal(state)
    {
        return Err(unavailable("invalid_private_state"));
    }
    private_directory(&state.join("cortex/engine-private").join(connection))
}
fn private_directory(path: &Path) -> io::Result<PathBuf> {
    fs::DirBuilder::new()
        .mode(0o700)
        .recursive(true)
        .create(path)?;
    let real = path.canonicalize()?;
    let meta = fs::symlink_metadata(&real)?;
    if !meta.is_dir() || meta.uid() != uid() || meta.mode() & 0o022 != 0 {
        return Err(unavailable("engine_private_state_unsafe"));
    }
    Ok(real)
}
fn private_file(path: &Path) -> io::Result<PathBuf> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let meta = file.metadata()?;
    if !meta.is_file() || meta.uid() != uid() || meta.nlink() != 1 {
        return Err(unavailable("engine_private_state_unsafe"));
    }
    path.canonicalize()
}
/// An owner-owned regular file, reached through at most its own symlink.
fn owned_file(path: &Path) -> io::Result<Option<PathBuf>> {
    let real = match path.canonicalize() {
        Ok(real) => real,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let meta = fs::symlink_metadata(&real)?;
    if !meta.is_file() || meta.uid() != uid() {
        return Err(unavailable("unsafe_account_file"));
    }
    Ok(Some(real))
}

/// Where a confined process runs.
pub(crate) enum Workdir<'a> {
    /// A checkout: read-only with the command policy's private masks.
    Project(&'a Path),
    /// A private scratch directory of this launch, writable.
    Scratch(&'a Path),
}

/// Which sign-in a view shows.
#[derive(Clone, Copy)]
pub(crate) enum Account<'a> {
    /// The engine's native sign-in in place (`engine_native_dir`).
    Native(&'a Path),
    /// A Cortex-owned engine home, bound whole.
    Owned(&'a Path),
}

/// One view under construction. Arguments are appended in mount order.
pub(crate) struct Builder {
    program: PathBuf,
    args: Vec<OsString>,
    pins: Vec<Arc<File>>,
    environment: BTreeMap<String, String>,
    mapped: Vec<(PathBuf, PathBuf)>,
    granted: Vec<PathBuf>,
    excluded: Vec<PathBuf>,
}
impl Builder {
    /// The system runtime and private `/tmp`. Requires an unprivileged bubblewrap.
    pub(crate) fn new() -> io::Result<Self> {
        let program = BWRAP
            .iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
            .ok_or_else(|| unavailable("bubblewrap_missing"))?;
        if fs::metadata(&program)?.mode() & 0o6000 != 0 {
            return Err(unavailable("privileged_bubblewrap"));
        }
        let mut view = Self {
            program,
            args: Vec::new(),
            pins: Vec::new(),
            environment: BTreeMap::from([("TMPDIR".into(), "/tmp".into())]),
            mapped: Vec::new(),
            granted: vec![PathBuf::from("/usr")],
            excluded: Vec::new(),
        };
        view.flags([
            "--die-with-parent",
            "--unshare-pid",
            "--proc",
            "/proc",
            "--dev",
            "/dev",
            "--cap-drop",
            "ALL",
        ]);
        for path in ["/tmp", "/var/tmp"] {
            view.flags(["--perms", "01777", "--size", TMP_BYTES, "--tmpfs", path]);
        }
        view.bind(Path::new("/usr"), Path::new("/usr"), true)?;
        for (name, usr) in [
            ("/bin", "/usr/bin"),
            ("/sbin", "/usr/sbin"),
            ("/lib", "/usr/lib"),
            ("/lib32", "/usr/lib32"),
            ("/lib64", "/usr/lib64"),
        ] {
            let path = Path::new(name);
            match fs::read_link(path) {
                Ok(link) => {
                    if path.canonicalize()?.starts_with("/usr") {
                        view.args
                            .extend(["--symlink".into(), link.into_os_string(), name.into()]);
                    } else {
                        return Err(unavailable("unsupported_system_layout"));
                    }
                }
                Err(_) if path.is_dir() => {
                    if !Path::new(usr).exists() || path.canonicalize()? != Path::new(usr) {
                        view.bind(path, path, true)?;
                    }
                }
                Err(_) => {}
            }
        }
        for name in SYSTEM_FILES {
            if let Ok(real) = Path::new(name).canonicalize() {
                if fs::metadata(&real).is_ok_and(|m| m.is_file()) {
                    view.bind(&real, Path::new(name), true)?;
                }
            }
        }
        for name in SYSTEM_DIRECTORIES {
            let path = Path::new(name);
            if fs::symlink_metadata(path).is_ok_and(|m| m.is_dir()) {
                view.bind(path, path, true)?;
            }
        }
        Ok(view)
    }
    fn flags<const N: usize>(&mut self, flags: [&str; N]) {
        self.args.extend(flags.map(OsString::from));
    }
    /// `source` must be canonical; `target` is the absolute path inside the view.
    fn bind(&mut self, source: &Path, target: &Path, read_only: bool) -> io::Result<()> {
        if !normal(target) {
            return Err(unavailable("invalid_mount_target"));
        }
        if self.excluded.iter().any(|p| {
            source.starts_with(p) || p.starts_with(source)
                || target.starts_with(p) || p.starts_with(target)
        }) {
            return Err(unavailable("worktree_route_overlapping_mount"));
        }
        filesystem_view::bind(&mut self.args, &mut self.pins, source, target, read_only)
    }
    /// A private HOME at the owner's HOME path: a fresh owner-only tmpfs per
    /// launch (the mount points of later grants are created there and vanish
    /// with it) with the connection's persistent private cache at `~/.cache`.
    /// Engines keep their own sessions and settings in their account view.
    pub(crate) fn home(&mut self, home: &Path, private: &Path) -> io::Result<()> {
        if !normal(home)
            || home == Path::new("/")
            || ["/usr", "/etc", "/proc", "/dev", "/run", "/tmp", "/var/tmp"]
                .iter()
                .any(|p| home == Path::new(p) || Path::new(p).starts_with(home))
            || ["/usr", "/etc", "/proc", "/dev", "/run"]
                .iter()
                .any(|p| home.starts_with(p))
        {
            return Err(unavailable("invalid_home"));
        }
        self.args.extend([
            "--perms".into(),
            "0700".into(),
            "--size".into(),
            TMP_BYTES.into(),
            "--tmpfs".into(),
            home.as_os_str().to_owned(),
        ]);
        let cache = private_directory(&private.join("cache"))?;
        self.bind(&cache, &home.join(".cache"), false)
    }

    /// The engine's install: each absolute existing path of the approved argv
    /// (the executable and, for example, its entry script). A path inside a
    /// `node_modules` tree grants the outermost one; any other grants itself.
    /// Grants never contain HOME.
    pub(crate) fn install<'a>(
        &mut self,
        paths: impl IntoIterator<Item = &'a Path>,
        home: &Path,
    ) -> io::Result<()> {
        let home_real = home.canonicalize().unwrap_or_else(|_| home.to_owned());
        for path in paths {
            if !normal(path) {
                continue;
            }
            let Ok(real) = path.canonicalize() else {
                continue;
            };
            let grant = real
                .ancestors()
                .filter(|a| a.file_name() == Some(OsStr::new("node_modules")))
                .last()
                .unwrap_or(&real)
                .to_owned();
            if grant == Path::new("/")
                || home.starts_with(&grant)
                || home_real.starts_with(&grant)
            {
                return Err(unavailable("install_contains_home"));
            }
            self.read_only(&grant)?;
            // The approved spelling of a file resolves inside the view too
            // (a name inside an already shown tree resolves there).
            if path != real
                && !self.granted.iter().any(|g| path.starts_with(g))
                && fs::metadata(&real)?.is_file()
            {
                self.bind(&real, path, true)?;
            }
        }
        Ok(())
    }
    /// One more read-only file or directory at its own canonical path.
    pub(crate) fn read_only(&mut self, path: &Path) -> io::Result<()> {
        let real = path.canonicalize()?;
        if self.granted.iter().any(|g| real.starts_with(g)) {
            return Ok(());
        }
        let meta = fs::metadata(&real)?;
        if !meta.is_file() && !meta.is_dir() {
            return Err(unavailable("unsupported_install_entry"));
        }
        self.bind(&real, &real, true)?;
        self.granted.push(real);
        Ok(())
    }
    /// Cortex's own executable: the MCP relay and the native Codex launcher's
    /// interpreter.
    pub(crate) fn service_executable(&mut self) -> io::Result<()> {
        let exe = std::env::current_exe()?.canonicalize()?;
        self.read_only(&exe)
    }

    /// The engine's sign-in, as described in the module documentation.
    pub(crate) fn account(
        &mut self,
        engine: EngineKind,
        account: Account<'_>,
        private: &Path,
    ) -> io::Result<()> {
        match account {
            Account::Owned(directory) => {
                let real = private_directory(directory)?;
                let meta = fs::symlink_metadata(&real)?;
                if !meta.is_dir() || meta.uid() != uid() || !normal(directory) {
                    return Err(unavailable("unsafe_engine_home"));
                }
                self.bind(&real, directory, false)
            }
            Account::Native(directory) => {
                if !normal(directory) {
                    return Err(unavailable("invalid_account_directory"));
                }
                let real = directory.canonicalize()?;
                let meta = fs::symlink_metadata(&real)?;
                if !meta.is_dir() || meta.uid() != uid() {
                    return Err(unavailable("unsafe_account_directory"));
                }
                match engine {
                    EngineKind::Codex => self.codex(directory, &real, private),
                    EngineKind::Claude => {
                        self.bind(&real, directory, false)?;
                        let substitutes = private_directory(&private.join("claude-config"))?;
                        self.substitute(&real, directory, &substitutes, |name| {
                            CLAUDE_KEEP.iter().any(|keep| {
                                let name = name.as_encoded_bytes();
                                name.strip_prefix(keep.as_bytes())
                                    .is_some_and(|rest| rest.is_empty() || rest[0] == b'.')
                            })
                        })
                    }
                    EngineKind::OpenCode => {
                        // OpenCode later runs helper executables from `bin` outside any
                        // view: it always exists (as before), so a confined process
                        // only ever writes its private substitute.
                        let bin = real.join("bin");
                        if fs::symlink_metadata(&bin).is_err() {
                            fs::DirBuilder::new().mode(0o700).create(&bin)?;
                        }
                        self.bind(&real, directory, false)?;
                        let substitutes = private_directory(&private.join("opencode-data"))?;
                        self.substitute(&real, directory, &substitutes, |name| {
                            OPENCODE_KEEP.iter().any(|keep| name == *keep)
                        })
                    }
                }
            }
        }
    }
    /// OpenCode sessions read the owner's own OpenCode configuration files
    /// (providers and models), read-only. Other engines' configuration is either
    /// in their account view (Codex `config.toml`) or not loaded (Claude).
    pub(crate) fn engine_config(&mut self, engine: EngineKind, home: &Path) -> io::Result<()> {
        if engine != EngineKind::OpenCode {
            return Ok(());
        }
        let config = Path::new(".config/opencode");
        let Ok(real_home) = home.canonicalize() else {
            return Ok(());
        };
        for name in OPENCODE_CONFIG {
            if let Some(file) = owned_file(&real_home.join(config).join(name))? {
                self.bind(&file, &home.join(config).join(name), true)?;
            }
        }
        Ok(())
    }
    /// A directory at its own path: read-only, or writable for a private
    /// scratch directory of this launch.
    pub(crate) fn directory(&mut self, path: &Path, writable: bool) -> io::Result<()> {
        let real = path.canonicalize()?;
        let meta = fs::symlink_metadata(&real)?;
        if !meta.is_dir() || (writable && (meta.uid() != uid() || meta.mode() & 0o077 != 0)) {
            return Err(unavailable("unsafe_directory"));
        }
        self.bind(&real, path, !writable)
    }
    /// A writable private directory shown at another path (for example `/var/tmp`).
    pub(crate) fn directory_at(&mut self, path: &Path, target: &Path) -> io::Result<()> {
        let real = path.canonicalize()?;
        let meta = fs::symlink_metadata(&real)?;
        if !meta.is_dir() || meta.uid() != uid() || meta.mode() & 0o077 != 0 {
            return Err(unavailable("unsafe_directory"));
        }
        self.bind(&real, target, false)
    }
    /// Literal view arguments a caller already validated (descriptor-backed
    /// data files, an additional namespace flag).
    pub(crate) fn raw(&mut self, args: impl IntoIterator<Item = OsString>) {
        self.args.extend(args);
    }
    fn codex(&mut self, directory: &Path, real: &Path, private: &Path) -> io::Result<()> {
        let substitute = private_directory(&private.join("codex-home"))?;
        // Mount points of absent native files would read as empty files.
        for name in ["auth.json", "auth.lock", "config.toml"] {
            let leftover = substitute.join(name);
            if fs::symlink_metadata(&leftover).is_ok_and(|m| m.is_file() && m.len() == 0)
                && fs::symlink_metadata(real.join(name)).is_err()
            {
                fs::remove_file(&leftover)?;
            }
        }
        self.bind(&substitute, directory, false)?;
        if let Some(auth) = owned_file(&real.join("auth.json"))? {
            self.bind(&auth, &directory.join("auth.json"), false)?;
            // Codex creates this exact (empty) lock file itself before it rewrites
            // `auth.json`; it must be the native one, shared with native Codex.
            let lock = real.join("auth.lock");
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&lock)?;
            let meta = file.metadata()?;
            if !meta.is_file() || meta.uid() != uid() {
                return Err(unavailable("unsafe_account_file"));
            }
            self.bind(&lock.canonicalize()?, &directory.join("auth.lock"), false)?;
        }
        if let Some(config) = owned_file(&real.join("config.toml"))? {
            self.bind(&config, &directory.join("config.toml"), true)?;
        }
        Ok(())
    }
    /// Replace every existing entry of `real` (shown at `target`) that `keep`
    /// does not name with a private substitute of the same kind. Symlinks stay:
    /// they resolve only inside the view.
    fn substitute(
        &mut self,
        real: &Path,
        target: &Path,
        substitutes: &Path,
        keep: impl Fn(&OsStr) -> bool,
    ) -> io::Result<()> {
        let mut names = Vec::new();
        for (count, entry) in fs::read_dir(real)?.enumerate() {
            if count >= 4096 {
                return Err(unavailable("account_directory_too_large"));
            }
            names.push(entry?.file_name());
        }
        names.sort();
        let directories = private_directory(&substitutes.join("d"))?;
        let files = private_directory(&substitutes.join("f"))?;
        for name in names {
            if keep(&name) {
                continue;
            }
            let meta = match fs::symlink_metadata(real.join(&name)) {
                Ok(meta) => meta,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            let shown = target.join(&name);
            if meta.file_type().is_symlink() {
                continue;
            } else if meta.is_dir() {
                let source = private_directory(&directories.join(&name))?;
                self.bind(&source, &shown, false)?;
            } else if meta.is_file() {
                let source = private_file(&files.join(&name))?;
                self.bind(&source, &shown, false)?;
            } else {
                self.bind(Path::new("/dev/null"), &shown, true)?;
            }
        }
        Ok(())
    }

    /// The working directory and `--chdir`.
    pub(crate) fn workdir(&mut self, workdir: Workdir<'_>, home: &Path) -> io::Result<()> {
        let cwd = match workdir {
            Workdir::Project(root) => {
                let real = root.canonicalize()?;
                if home.starts_with(&real) || real == Path::new("/") {
                    return Err(unavailable("project_contains_home"));
                }
                crate::native_command_sandbox::engine_project_view(
                    &real,
                    home,
                    &mut self.args,
                    &mut self.pins,
                )?;
                root
            }
            Workdir::Scratch(directory) => {
                let real = directory.canonicalize()?;
                let meta = fs::symlink_metadata(&real)?;
                if !meta.is_dir() || meta.uid() != uid() || meta.mode() & 0o077 != 0 {
                    return Err(unavailable("unsafe_scratch"));
                }
                self.bind(&real, directory, false)?;
                directory
            }
        };
        if !normal(cwd) {
            return Err(unavailable("invalid_workdir"));
        }
        self.args.extend(["--chdir".into(), cwd.as_os_str().into()]);
        Ok(())
    }
    /// Additional read-only files the engine reads (its instructions file, the
    /// prepared Codex launcher). Files inside a shown broker directory are
    /// already visible there.
    pub(crate) fn inputs(&mut self, inputs: &[PathBuf]) -> io::Result<()> {
        for input in inputs {
            if self.mapped.iter().any(|(host, _)| input.starts_with(host)) {
                continue;
            }
            let real = owned_file(input)?.ok_or_else(|| unavailable("missing_launch_input"))?;
            if !normal(input) {
                return Err(unavailable("invalid_launch_input"));
            }
            self.bind(&real, input, true)?;
        }
        Ok(())
    }
    /// Show each private broker directory read-only at
    /// [`cortex_engine::broker::VIEW_DIRECTORY`]`/<name>`.
    pub(crate) fn brokers(&mut self, directories: &[PathBuf]) -> io::Result<()> {
        for directory in directories {
            let real = directory.canonicalize()?;
            let meta = fs::symlink_metadata(&real)?;
            let name = real
                .file_name()
                .filter(|n| n.as_encoded_bytes().starts_with(b"cortex-mcp-"))
                .ok_or_else(|| unavailable("invalid_broker_directory"))?
                .to_owned();
            if !meta.is_dir() || meta.uid() != uid() || meta.mode() & 0o077 != 0 {
                return Err(unavailable("unsafe_broker_directory"));
            }
            let shown = Path::new(cortex_engine::broker::VIEW_DIRECTORY).join(&name);
            // The precisely scoped per-channel relay directory is the only
            // approved grant from inside the logical project store.
            filesystem_view::bind(&mut self.args, &mut self.pins, &real, &shown, true)?;
            self.mapped.push((directory.clone(), shown.clone()));
            if real != *directory {
                self.mapped.push((real, shown));
            }
        }
        Ok(())
    }
    pub(crate) fn finish(self) -> Confinement {
        Confinement::new(
            self.program,
            self.args,
            self.pins,
            self.environment,
            self.mapped,
        )
    }
}

/// Debug builds only: Cortex's own no-model ACP test peer (`fake-engine`, or the
/// test executable itself) built beside the running test binary starts without
/// a view, and so does any engine of a debug process that already runs in a
/// nested user namespace, so the hermetic suite also runs inside a Cortex
/// command view. Release builds never take this branch.
pub(crate) fn unconfined_test_peer(executable: &Path) -> bool {
    // Crate unit tests drive helpers with scripted peers; their views are
    // exercised directly by this module's tests.
    cfg!(test) || beside_test_binary(executable) || nested_debug_build() || debug_opt_out()
}
/// Debug builds only: an integration test whose scripted peer reports through
/// fixture files outside any view (Python-parity catalogue captures) starts its
/// service with `CORTEX_TEST_UNCONFINED_ENGINES=1`. Release builds ignore it.
fn debug_opt_out() -> bool {
    cfg!(debug_assertions)
        && std::env::var_os("CORTEX_TEST_UNCONFINED_ENGINES").is_some_and(|v| v == "1")
}
/// Debug builds only: this process already runs in a non-initial user namespace
/// (for example a test suite inside a Cortex command view, where a further
/// unprivileged namespace is refused by design). Release builds never take it.
fn nested_debug_build() -> bool {
    cfg!(debug_assertions)
        && fs::read_to_string("/proc/self/uid_map").is_ok_and(|map| {
            map.split_whitespace().collect::<Vec<_>>() != ["0", "0", "4294967295"]
        })
}
fn beside_test_binary(executable: &Path) -> bool {
    #[cfg(debug_assertions)]
    {
        let (Ok(real), Ok(exe)) = (
            executable.canonicalize(),
            std::env::current_exe().and_then(|e| e.canonicalize()),
        ) else {
            return false;
        };
        if real == exe {
            return true;
        }
        let beside = exe.parent().into_iter().flat_map(|d| [Some(d), d.parent()]).flatten();
        real.file_name() == Some(OsStr::new("fake-engine"))
            && beside.into_iter().any(|d| real.parent() == Some(d))
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = executable;
        false
    }
}

/// The confinement of one routed engine session (task roles and short texts).
pub(crate) struct RouteConfiner {
    pub(crate) state: PathBuf,
    pub(crate) connection: String,
    excluded: Vec<PathBuf>,
}
impl RouteConfiner {
    pub(crate) fn request(state: &Path, connection: &str) -> ConfinementRequest {
        ConfinementRequest::new(Arc::new(Self {
            state: state.to_owned(),
            connection: connection.to_owned(),
            excluded: Vec::new(),
        }))
    }

    pub(crate) fn worktree(
        state: &Path, connection: &str, logical: &Path, cwd: &Path,
    ) -> cortex_core::CoreResult<ConfinementRequest> {
        let siblings = cwd.parent().ok_or(cortex_core::CoreError::Refused("worktree_binding_invalid"))?;
        let excluded = vec![logical.to_owned(), siblings.to_owned()];
        let system = ["/usr", "/etc", "/proc", "/dev", "/run"];
        if excluded.iter().any(|path| {
            system.iter().any(|p| path.starts_with(p) || Path::new(p).starts_with(path))
                || state.starts_with(path)
        }) {
            return Err(cortex_core::CoreError::Refused("worktree_route_overlapping_mount"));
        }
        Ok(ConfinementRequest::new(Arc::new(Self {
            state: state.to_owned(), connection: connection.to_owned(), excluded,
        })))
    }
}
impl Confiner for RouteConfiner {
    fn confine(
        &self,
        spec: &LaunchSpec,
        _: &BTreeMap<String, String>,
        inputs: &[PathBuf],
    ) -> io::Result<Confinement> {
        let private = private_root(&self.state, &self.connection)?;
        let mut view = Builder::new()?;
        // HOME is tmpfs; its cache, account, install and launcher are
        // separate grants which must not overlap any checkout.
        view.excluded = self.excluded.clone();
        view.home(&spec.native_home, &private)?;
        view.install(
            std::iter::once(spec.executable.as_path())
                .chain(spec.args.iter().map(Path::new)),
            &spec.native_home,
        )?;
        view.service_executable()?;
        view.account(
            spec.engine,
            Account::Native(&spec.credential_directory),
            &private,
        )?;
        view.engine_config(spec.engine, &spec.native_home)?;
        view.workdir(Workdir::Project(&spec.cwd), &spec.native_home)?;
        let exposed = spec
            .confinement
            .as_ref()
            .map(|r| r.exposed.clone())
            .unwrap_or_default();
        view.brokers(&exposed)?;
        view.inputs(inputs)?;
        Ok(view.finish())
    }
}

/// The view of one helper that starts an engine executable outside a session
/// (model catalogue, usage): the same system, install, private HOME and sign-in
/// as a session of that connection, a private writable scratch directory as its
/// working directory, `extra` install trees and `inputs` files read-only.
/// `None` only for the debug-build test peer (see [`unconfined_test_peer`]).
#[allow(clippy::too_many_arguments)]
pub(crate) fn helper(
    engine: EngineKind,
    state: &Path,
    connection: &str,
    home: &Path,
    executable: &Path,
    args: &[String],
    account: Account<'_>,
    scratch: &Path,
    extra: &[PathBuf],
    inputs: &[PathBuf],
) -> io::Result<Option<Confinement>> {
    if unconfined_test_peer(executable) {
        return Ok(None);
    }
    let private = private_root(state, connection)?;
    let mut view = Builder::new()?;
    view.home(home, &private)?;
    view.install(
        std::iter::once(executable).chain(args.iter().map(Path::new)),
        home,
    )?;
    for path in extra {
        view.read_only(path)?;
    }
    view.service_executable()?;
    view.account(engine, account, &private)?;
    view.workdir(Workdir::Scratch(scratch), home)?;
    view.inputs(inputs)?;
    Ok(Some(view.finish()))
}

#[cfg(test)]
#[path = "native_routes_view_tests.rs"]
mod tests;
