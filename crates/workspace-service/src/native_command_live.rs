//! Host-visible profiles for ordinary worker commands and runtime verification.
//! The scoped snapshot uses the separate sandbox policy.
#[path = "native_command_live_policy.rs"]
pub(crate) mod policy;
#[path = "native_command_ssh.rs"]
mod ssh;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs, io,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) use crate::native_command_sandbox::Profile;

pub(crate) struct Sandbox {
    binary: PathBuf,
    args: Vec<OsString>,
    scratch: PathBuf,
    environment: BTreeMap<OsString, OsString>,
    pins: Vec<std::sync::Arc<fs::File>>,
    guard: Option<policy::GitGuard>,
    overlay: bool,
    auto: bool,
    ssh_environment: BTreeMap<OsString, OsString>,
}

fn unavailable(reason: &str) -> io::Error {
    io::Error::other(format!("native_command_sandbox_unavailable: {reason}"))
}

impl Sandbox {
    pub(crate) fn prepare(
        root: &Path,
        profile: Profile,
        writes: &[PathBuf],
        protected: &[PathBuf],
        network: bool,
    ) -> io::Result<Self> {
        Self::with_environment(
            root,
            profile,
            writes,
            protected,
            network,
            // The owner's tool search path replaces the session manager's PATH.
            crate::tool_path::environment(),
        )
    }

    pub(crate) fn with_environment(
        root: &Path,
        profile: Profile,
        writes: &[PathBuf],
        protected: &[PathBuf],
        network: bool,
        environment: BTreeMap<OsString, OsString>,
    ) -> io::Result<Self> {
        Self::with_environment_authorized(
            root,
            profile,
            writes,
            protected,
            network,
            environment,
            &mut || Ok(()),
        )
    }

    /// `timings` receives per-stage durations (milliseconds) and counts, also when
    /// preparation fails: numbers only, never paths or names.
    pub(crate) fn prepare_authorized(
        root: &Path,
        profile: Profile,
        writes: &[PathBuf],
        protected: &[PathBuf],
        before_write: &mut dyn FnMut() -> io::Result<()>,
        timings: &mut BTreeMap<&'static str, u64>,
    ) -> io::Result<Self> {
        Self::with_environment_timed(
            root,
            profile,
            writes,
            protected,
            true,
            // In the per-command supervisor this is the PATH the service resolved for it.
            crate::tool_path::environment(),
            before_write,
            timings,
        )
    }

    fn with_environment_authorized(
        root: &Path,
        profile: Profile,
        writes: &[PathBuf],
        protected: &[PathBuf],
        network: bool,
        environment: BTreeMap<OsString, OsString>,
        before_write: &mut dyn FnMut() -> io::Result<()>,
    ) -> io::Result<Self> {
        Self::with_environment_timed(
            root,
            profile,
            writes,
            protected,
            network,
            environment,
            before_write,
            &mut BTreeMap::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn with_environment_timed(
        root: &Path,
        profile: Profile,
        writes: &[PathBuf],
        protected: &[PathBuf],
        network: bool,
        environment: BTreeMap<OsString, OsString>,
        before_write: &mut dyn FnMut() -> io::Result<()>,
        timings: &mut BTreeMap<&'static str, u64>,
    ) -> io::Result<Self> {
        let binary = ["/usr/bin/bwrap", "/bin/bwrap"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
            .ok_or_else(|| unavailable("bubblewrap_missing"))?;
        let started = Instant::now();
        let result = Self::with_binary_authorized(
            binary,
            root,
            profile,
            writes,
            protected,
            network,
            environment,
            before_write,
            timings,
        );
        timings.insert("preparation_ms", started.elapsed().as_millis() as u64);
        result
    }

    #[cfg(test)]
    fn with_binary(
        binary: PathBuf,
        root: &Path,
        profile: Profile,
        writes: &[PathBuf],
        protected: &[PathBuf],
        network: bool,
        environment: BTreeMap<OsString, OsString>,
    ) -> io::Result<Self> {
        Self::with_binary_authorized(
            binary,
            root,
            profile,
            writes,
            protected,
            network,
            environment,
            &mut || Ok(()),
            &mut BTreeMap::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn with_binary_authorized(
        binary: PathBuf,
        root: &Path,
        profile: Profile,
        writes: &[PathBuf],
        protected: &[PathBuf],
        network: bool,
        environment: BTreeMap<OsString, OsString>,
        before_write: &mut dyn FnMut() -> io::Result<()>,
        timings: &mut BTreeMap<&'static str, u64>,
    ) -> io::Result<Self> {
        let millis = |since: Instant| since.elapsed().as_millis() as u64;
        let root = root.canonicalize()?;
        let scratch = std::env::temp_dir().join(format!(
            "cortex-command-{}",
            crate::native_effects::identity().map_err(|_| unavailable("scratch_identity"))?
        ));
        fs::DirBuilder::new().mode(0o700).create(&scratch)?;
        let mut s = Self {
            binary,
            args: Vec::new(),
            scratch,
            environment,
            pins: Vec::new(),
            guard: None,
            overlay: false,
            auto: profile == Profile::Auto,
            ssh_environment: BTreeMap::new(),
        };
        fs::create_dir(s.scratch.join("var-tmp"))?;
        s.args.push("--die-with-parent".into());
        if profile == Profile::ReadOnly {
            s.args.push("--unshare-pid".into());
        }
        if !network {
            s.args.push("--unshare-net".into());
        }
        s.args.extend(
            [
                "--clearenv",
                "--setenv",
                "PATH",
                "/usr/local/bin:/usr/bin:/bin",
                "--setenv",
                "LANG",
                "C.UTF-8",
            ]
            .map(OsString::from),
        );
        // Only the filtered owner environment reaches the child.
        // HOME is intentionally the real owner's home; its cache grants are mounts.
        s.mount(
            if profile.wide() {
                "--bind"
            } else {
                "--ro-bind"
            },
            Path::new("/"),
            Path::new("/"),
        )?;
        s.args
            .extend(["--proc", "/proc", "--dev-bind", "/dev", "/dev"].map(OsString::from));
        s.mount("--bind", Path::new("/tmp"), Path::new("/tmp"))?;
        if !profile.wide() {
            s.mount("--bind", &s.scratch.join("var-tmp"), Path::new("/var/tmp"))?;
        }
        let home = s
            .environment
            .get(std::ffi::OsStr::new("HOME"))
            .map(PathBuf::from)
            .filter(|p| p.is_absolute() && p.is_dir());
        let controls = Instant::now();
        let mut protected = policy::expand(&root, protected)?;
        protected.extend(policy::controls(&root, &s.environment)?);
        let git_dirs = policy::git_dirs(&root)?;
        timings.insert("controls_ms", millis(controls));
        protected.extend(git_dirs.iter().cloned());
        policy::home(&mut s, &root, profile, &protected)?;
        for p in &protected {
            s.protect(p)?;
        }
        // A project beneath a writable host or /tmp is always remounted read-only first.
        let project_mount = s.args.len();
        s.mount("--ro-bind", &root, &root)?;
        if profile == Profile::ReadOnly {
            let overlay = Instant::now();
            s.discard_project_writes(project_mount, &root)?;
            timings.insert("overlay_probe_ms", millis(overlay));
            let options = s
                .environment
                .get(std::ffi::OsStr::new("PYTEST_ADDOPTS"))
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_default();
            s.environment.insert(
                "PYTEST_ADDOPTS".into(),
                format!(
                    "{options} -o '{}'",
                    format!("cache_dir={}/pytest-cache", s.scratch.display())
                        .replace('\'', "'\"'\"'")
                )
                .into(),
            );
        }
        let materialization_denials: Vec<_> = protected
            .iter()
            .filter(|p| !git_dirs.contains(p))
            .cloned()
            .collect();
        let walked = Instant::now();
        let tree = policy::scan(&root, writes, &materialization_denials)?;
        timings.insert("tree_walk_ms", millis(walked));
        timings.insert("tree_entries", tree.entries as u64);
        let hardlinks = Instant::now();
        let link_overlays = policy::links_from(&tree, &root, profile, before_write)?;
        timings.insert("hardlink_policy_ms", millis(hardlinks));
        for p in writes {
            let p = match p.canonicalize() {
                Ok(p) => p,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            if !p.starts_with(&root) {
                return Err(unavailable("write_grant_outside_project"));
            }
            s.mount("--bind", &p, &p)?;
        }
        // Private controls and owner denials always follow writable grants.
        s.private_entries(&tree.private)?;
        for p in &protected {
            if p.starts_with(&root) && !git_dirs.contains(p) {
                s.protect(p)?;
            }
        }
        let whole = writes.iter().any(|p| p == &root);
        let git = Instant::now();
        policy::git(&mut s, &root, &git_dirs, profile, whole)?;
        timings.insert("git_policy_ms", millis(git));
        for p in link_overlays {
            s.mount("--ro-bind", &p, &p)?;
        }
        // Scratch last: even when the caller's temp directory is under /var/tmp.
        s.mount("--bind", &s.scratch.clone(), &s.scratch.clone())?;
        for variable in ["HOME", "TMPDIR", "TMP", "TEMP"] {
            let value = if variable == "HOME" {
                home.as_ref().unwrap_or(&s.scratch)
            } else {
                &s.scratch
            };
            s.args.extend([
                OsString::from("--setenv"),
                variable.into(),
                value.as_os_str().into(),
            ]);
        }
        if home.is_none() {
            s.environment.insert(
                "XDG_CACHE_HOME".into(),
                s.scratch.join(".cache").into_os_string(),
            );
        }
        if let Some(path) = command_path(&root, &s.environment) {
            s.environment.insert("PATH".into(), path);
        }
        for name in [".venv", "venv"] {
            let p = root.join(name);
            if p.join("bin").is_dir() {
                s.environment
                    .insert("VIRTUAL_ENV".into(), p.into_os_string());
                break;
            }
        }
        s.environment
            .insert("PYTHONDONTWRITEBYTECODE".into(), "1".into());
        ssh::configure(&mut s, home.as_deref())?;
        s.args
            .extend([OsString::from("--chdir"), root.into_os_string()]);
        let probe = Instant::now();
        let probed = s.probe();
        timings.insert("probe_ms", millis(probe));
        probed?;
        Ok(s)
    }

    fn mount(&mut self, kind: &str, source: &Path, target: &Path) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let real = source.canonicalize()?;
        let file = std::sync::Arc::new(policy::pin(&real)?);
        let flag = match kind {
            "--bind" => "--bind-fd",
            "--ro-bind" => "--ro-bind-fd",
            _ => return Err(unavailable("invalid_mount_kind")),
        };
        self.args.extend([
            flag.into(),
            file.as_raw_fd().to_string().into(),
            target.as_os_str().into(),
        ]);
        self.pins.push(file);
        Ok(())
    }
    fn git_config(&mut self, values: &[String]) {
        let prior = self
            .environment
            .get(std::ffi::OsStr::new("GIT_CONFIG_COUNT"))
            .and_then(|n| n.to_str())
            .and_then(|n| n.parse::<usize>().ok())
            .unwrap_or(0);
        for (i, value) in values.iter().enumerate() {
            if let Some((key, value)) = value.split_once('=') {
                self.environment
                    .insert(format!("GIT_CONFIG_KEY_{}", prior + i).into(), key.into());
                self.environment.insert(
                    format!("GIT_CONFIG_VALUE_{}", prior + i).into(),
                    value.into(),
                );
            }
        }
        self.environment.insert(
            "GIT_CONFIG_COUNT".into(),
            (prior + values.len()).to_string().into(),
        );
    }
    fn discard_project_writes(&mut self, at: usize, root: &Path) -> io::Result<()> {
        let probe_root = self.scratch.join("overlay-probe");
        let upper = self.scratch.join("overlay-upper");
        let work = self.scratch.join("overlay-work");
        for p in [&probe_root, &upper, &work] {
            fs::create_dir(p)?;
        }
        let mut probe = Command::new(&self.binary);
        probe
            .args([
                "--die-with-parent",
                "--ro-bind",
                "/",
                "/",
                "--dev",
                "/dev",
                "--overlay-src",
            ])
            .arg(&probe_root)
            .arg("--overlay")
            .arg(&upper)
            .arg(&work)
            .arg(&probe_root)
            .args(["--", "/bin/true"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut child = probe.spawn()?;
        // Generous for a loaded host: a probe that merely ran slowly must not leave a
        // read-only command without its discarded project overlay.
        let deadline = Instant::now() + Duration::from_secs(20);
        let supported = loop {
            match child.try_wait()? {
                Some(status) => break status.success(),
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break false;
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        };
        if supported {
            let fd = self.args[at + 1].clone();
            let source = OsString::from(format!("/proc/self/fd/{}", fd.to_string_lossy()));
            // The overlay source is a path operand, not a consumed --bind-fd.
            // Keep its descriptor in bubblewrap\'s monitor, never the payload.
            self.args.extend([OsString::from("--sync-fd"), fd]);
            self.args.splice(
                at..at + 3,
                [
                    OsString::from("--overlay-src"),
                    source,
                    "--overlay".into(),
                    upper.into_os_string(),
                    work.into_os_string(),
                    root.as_os_str().into(),
                ],
            );
            self.overlay = true;
        }
        Ok(())
    }
    pub(crate) fn finish(&mut self) -> Vec<String> {
        self.guard.as_mut().map(|g| g.finish()).unwrap_or_default()
    }

    fn protect(&mut self, p: &Path) -> io::Result<()> {
        if !p.try_exists()? {
            return Ok(());
        }
        let real = p.canonicalize()?;
        if real == Path::new("/") {
            return Err(unavailable("control_path_overlaps_host"));
        }
        let store = p
            .file_name()
            .is_some_and(|n| n == ".cortex" || n == ".cortex-dev")
            && real.is_dir();
        if store && self.auto {
            // Auto may write the store directory (the owner allowed model writes
            // on request); the live store files stay out of reach: an empty,
            // unreadable file is bound over each, so a command can neither read the
            // runtime's private database nor write, remove or rename it. A live file
            // that does not exist yet cannot be masked without creating it.
            let sealed = sealed_file()?;
            for e in fs::read_dir(&real)? {
                let path = e?.path();
                if crate::native_engine_policy::live_store_file(&path) {
                    self.mount("--ro-bind", &sealed, &path)?;
                }
            }
        } else if store {
            self.args.extend([
                OsString::from("--tmpfs"),
                real.as_os_str().into(),
                "--remount-ro".into(),
                real.as_os_str().into(),
            ]);
        } else {
            self.mount("--ro-bind", &real, &real)?;
        }
        Ok(())
    }

    /// Hides every private-named entry the preparation scan found at any depth.
    fn private_entries(&mut self, entries: &[PathBuf]) -> io::Result<()> {
        for p in entries {
            if p.file_name()
                .is_some_and(|n| n == ".cortex-dev-owner.json" || n == ".cortex-workspace-dev.json")
            {
                self.mount("--ro-bind", Path::new("/dev/null"), p)?;
            } else {
                self.protect(p)?;
            }
        }
        Ok(())
    }

    /// Probe this exact mount/process view, never the proposed payload. No cached
    /// availability survives a changed host policy or a service restart.
    fn probe(&self) -> io::Result<()> {
        let mut command = self.command(&["/bin/true".into()])?;
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|_| unavailable("bubblewrap_launch_failed"))?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match child.try_wait()? {
                Some(s) if s.success() => return Ok(()),
                Some(_) => return Err(unavailable("namespace_or_mount_setup_failed")),
                None if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(unavailable("setup_timeout"));
                }
                None => std::thread::sleep(Duration::from_millis(10)),
            }
        }
    }

    pub(crate) fn command(&self, argv: &[String]) -> io::Result<Command> {
        self.command_at(argv, None)
    }

    pub(crate) fn command_at(&self, argv: &[String], cwd: Option<&Path>) -> io::Result<Command> {
        if argv.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty_command"));
        }
        let mut command = Command::new(&self.binary);
        // bwrap accepts only one --chdir. Replace the prepared root for an
        // explicitly selected cwd rather than appending a second option.
        if let Some(cwd) = cwd {
            command
                .args(&self.args[..self.args.len() - 2])
                .arg("--chdir")
                .arg(cwd);
        } else {
            command.args(&self.args);
        }
        command.env_clear().stdin(Stdio::null());
        use std::os::{fd::AsRawFd, unix::process::CommandExt};
        let pins = self.pins.clone();
        unsafe {
            command.pre_exec(move || {
                for file in &pins {
                    let fd = file.as_raw_fd();
                    if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                libc::umask(0o022);
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        // Like the retained command boundary, credentials/channel secrets are not inherited.
        let secret = |key: &str| {
            let key = key.to_ascii_uppercase();
            key.starts_with("CORTEX_")
                // Any *KEY name (OLLAMA_CLOUD_BACKUP_KEY, GPG_KEY, X_KEY_ID ...) is treated as a credential.
                || (!key.starts_with("GIT_CONFIG_")
                    && (key.ends_with("KEY") || key.ends_with("KEYS") || key.contains("_KEY_")))
                || [
                    "TOKEN",
                    "SECRET",
                    "PASSW",
                    "CREDENTIAL",
                    "API_KEY",
                    "APIKEY",
                    "PRIVATE_KEY",
                    "PRIVATEKEY",
                    "COOKIE",
                    "SESSION",
                ]
                .iter()
                .any(|part| key.contains(part))
        };
        for (key, value) in &self.environment {
            let upper = key.to_string_lossy().to_ascii_uppercase();
            let auth = upper.contains("AUTH")
                && !["GIT_AUTHOR_NAME", "GIT_AUTHOR_EMAIL", "GIT_AUTHOR_DATE"]
                    .contains(&upper.as_str());
            use std::os::unix::fs::{FileTypeExt, MetadataExt};
            unsafe extern "C" {
                fn getuid() -> std::ffi::c_uint;
            }
            let ssh_agent = upper == "SSH_AUTH_SOCK"
                && Path::new(value).is_absolute()
                && !value.as_encoded_bytes().contains(&b'\n')
                && fs::metadata(PathBuf::from(value))
                    .is_ok_and(|m| m.file_type().is_socket() && m.uid() == unsafe { getuid() });
            if !secret(&key.to_string_lossy())
                && (!auth || ssh_agent)
                && !["HOME", "TMPDIR", "TMP", "TEMP"]
                    .iter()
                    .any(|k| key == std::ffi::OsStr::new(k))
            {
                command.arg("--setenv").arg(key).arg(value);
            }
        }
        for (key, value) in &self.ssh_environment {
            command.arg("--setenv").arg(key).arg(value);
        }
        command.arg("--").args(argv);
        Ok(command)
    }

    #[cfg(test)]
    pub(crate) fn scratch(&self) -> &Path {
        &self.scratch
    }
}

/// The search path every sandboxed command resolves a bare program name on: the
/// project's own tool directories first, then the owner's inherited `PATH`. The
/// verifier's availability probe uses this same list, so a program a worker command
/// finds (for example a `/usr/local/bin` link into a version manager under the
/// read-only home) is not reported missing to the coordinator.
pub(crate) fn command_path(
    root: &Path,
    environment: &BTreeMap<OsString, OsString>,
) -> Option<OsString> {
    let inherited = environment
        .get(std::ffi::OsStr::new("PATH"))
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or("/usr/local/bin:/usr/bin:/bin".into());
    let mut paths = Vec::new();
    for name in [".venv/bin", "venv/bin", "node_modules/.bin"] {
        let p = root.join(name);
        if p.is_dir() {
            paths.push(p);
        }
    }
    // The owner's entries are validated again here: a project can never add a search
    // directory of its own beyond the fixed tool directories above (its folder and
    // its private `.cortex` state are refused), and neither can a world-writable,
    // non-sticky or missing directory. The sandbox's read-only root mount already
    // shows every remaining directory, so nothing extra is bound for them.
    let real_root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    let mut kept = 0;
    for p in std::env::split_paths(&inherited) {
        match crate::tool_path::valid(&p, &[]) {
            Ok(real) if !real.starts_with(&real_root) && !p.starts_with(root) => {
                paths.push(p);
                kept += 1;
            }
            _ => {}
        }
    }
    if kept == 0 {
        paths.extend(["/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    }
    std::env::join_paths(paths).ok()
}

/// Existing ignored directories only; never private paths or aliases. No hooks,
/// filters or filesystem monitors run during this Git catalogue query.
pub(crate) fn ignored_directories(root: &Path) -> io::Result<Vec<PathBuf>> {
    use std::os::unix::fs::OpenOptionsExt;
    let temp = std::env::temp_dir().join(format!(
        "cortex-ignored-{}",
        crate::native_effects::identity().map_err(|_| unavailable("scratch_identity"))?
    ));
    let mut file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    // Unlink only this owned spool; the open file remains private during the query.
    fs::remove_file(&temp)?;
    let mut child = Command::new("/usr/bin/git")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
            "--directory",
            "--",
            ".",
        ])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(file.try_clone()?)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let status = loop {
        match child.try_wait()? {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(unavailable("ignored_query_timeout"));
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let mut candidates = Vec::new();
    if status.success() {
        use std::io::{Read, Seek, SeekFrom};
        file.seek(SeekFrom::Start(0))?;
        let mut data = Vec::new();
        file.take(8 * 1024 * 1024 + 1).read_to_end(&mut data)?;
        if data.len() > 8 * 1024 * 1024 {
            return Err(unavailable("ignored_catalogue_limit"));
        }
        for raw in data.split(|b| *b == 0) {
            if raw.ends_with(b"/") {
                if let Ok(s) = std::str::from_utf8(raw) {
                    candidates.push(root.join(s.trim_end_matches('/')));
                }
            }
        }
    } else if !root.join(".git").exists() {
        for entry in fs::read_dir(root)? {
            let entry = entry?;
            if [
                "node_modules",
                ".venv",
                "venv",
                "target",
                "build",
                "dist",
                ".pytest_cache",
                "__pycache__",
            ]
            .iter()
            .any(|n| entry.file_name() == *n)
            {
                candidates.push(entry.path());
            }
        }
    } else {
        return Err(unavailable("ignored_query_failed"));
    }
    candidates.retain(|p| {
        p.is_dir()
            && p.strip_prefix(root).ok().is_some_and(|r| {
                r.components().all(|c| {
                    let name = c.as_os_str().to_string_lossy();
                    ![
                        ".git",
                        ".cortex",
                        ".cortex-dev",
                        ".cortex-dev-owner.json",
                        ".cortex-workspace-dev.json",
                    ]
                    .contains(&name.as_ref())
                })
            })
            && p.canonicalize().is_ok_and(|real| real == *p)
    });
    candidates.sort();
    candidates.dedup();
    Ok(candidates)
}
fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

#[cfg(test)]
#[path = "native_command_live_policy_tests.rs"]
mod advanced_tests;
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub(super) struct Fixture(pub(super) PathBuf);
    impl Fixture {
        pub(super) fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "namespace-fixture-{}",
                crate::native_effects::identity().unwrap()
            ));
            fs::create_dir(&p).unwrap();
            fs::create_dir(p.join("project")).unwrap();
            fs::write(p.join("project/source"), "unchanged").unwrap();
            fs::set_permissions(p.join("project/source"), fs::Permissions::from_mode(0o644))
                .unwrap();
            fs::create_dir(p.join("project/.cortex-dev")).unwrap();
            fs::write(p.join("project/.cortex-dev/private"), "private-fixture").unwrap();
            fs::write(p.join("outside"), "protected-fixture").unwrap();
            fs::create_dir_all(p.join("home/.cache")).unwrap();
            fs::create_dir_all(p.join("home/.cargo/bin")).unwrap();
            Self(p)
        }
        pub(super) fn prepare(
            &self,
            profile: Profile,
            writes: &[PathBuf],
            protected: &[PathBuf],
        ) -> io::Result<Sandbox> {
            let mut environment = BTreeMap::new();
            environment.insert("HOME".into(), self.0.join("home").into_os_string());
            Sandbox::with_environment(&self.root(), profile, writes, protected, false, environment)
        }
        pub(super) fn root(&self) -> PathBuf {
            self.0.join("project")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }
    pub(super) fn capability() -> bool {
        let available = Command::new("/usr/bin/bwrap")
            .args([
                "--die-with-parent",
                "--unshare-pid",
                "--ro-bind",
                "/",
                "/",
                "--proc",
                "/proc",
                "--",
                "/bin/true",
            ])
            .stdin(Stdio::null())
            .output()
            .is_ok_and(|o| o.status.success());
        if std::env::var_os("CORTEX_REQUIRE_NAMESPACE_FIXTURES").is_some() {
            assert!(
                available,
                "namespace fixtures required but bubblewrap cannot establish the real view"
            );
        }
        available
    }
    pub(super) fn run(s: &Sandbox, script: &str, args: &[&Path]) {
        let mut argv = vec![
            "/bin/sh".into(),
            "-ec".into(),
            format!(
                "denied() {{ if \"$@\"; then echo unexpected-success >&2; exit 97; fi; }}\n{}",
                script.replace("! ", "denied ")
            ),
            "fixture".into(),
        ];
        argv.extend(args.iter().map(|p| p.to_str().unwrap().into()));
        let out = s.command(&argv).unwrap().output().unwrap();
        assert!(
            out.status.success(),
            "fixture failed: status={:?}, stdout={}, stderr={}",
            out.status.code(),
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    #[test]
    fn namespace_views_real_fixtures_or_explicit_fail_closed() {
        let f = Fixture::new();
        let root = f.root();
        if !capability() {
            for profile in [Profile::ReadOnly, Profile::Guarded, Profile::Development] {
                let result = f.prepare(profile, &[root.clone()], &[]);
                let e = match result {
                    Err(e) => e,
                    Ok(_) => panic!("unavailable mechanism accepted"),
                };
                assert!(
                    e.to_string()
                        .starts_with("native_command_sandbox_unavailable:"),
                    "{e}"
                );
            }
            assert_eq!(
                fs::read_to_string(root.join("source")).unwrap(),
                "unchanged"
            );
            eprintln!(
                "namespace capability unavailable; all three profiles refused before payload launch"
            );
            return;
        }
        let marker = format!("private-{}", crate::native_effects::identity().unwrap());
        let host_marker = PathBuf::from("/var/tmp").join(&marker);
        assert!(!host_marker.exists());
        let pid_ns = fs::read_link("/proc/self/ns/pid").unwrap();
        let s = f.prepare(Profile::ReadOnly, &[], &[]).unwrap();
        let scratch = s.scratch().to_owned();
        let at = s
            .command_at(&["/bin/pwd".into()], Some(&root))
            .unwrap()
            .output()
            .unwrap();
        assert!(
            at.status.success(),
            "explicit cwd failed: {}",
            String::from_utf8_lossy(&at.stderr)
        );
        assert_eq!(
            String::from_utf8(at.stdout).unwrap().trim(),
            root.to_str().unwrap()
        );
        run(
            &s,
            r#"
            test "$(cat source)" = unchanged
            if test "$4" = overlay; then
                chmod 0600 source; touch -m source; echo discarded > source; mv source renamed
            else
                ! chmod 0600 source; ! touch -m source
                ! sh -c 'echo wrong > source'; ! mv source renamed
            fi
            ! cat .cortex-dev/private
            ! touch .cortex-dev/new
            test "$(readlink /proc/self/ns/pid)" != "$1"
            ! test -r "/proc/$2/environ"
            echo scratch > "$TMPDIR/file"
            chmod 0600 "$TMPDIR/file"
            touch -m "$TMPDIR/file"
            echo cached > "$HOME/.cache/value"
            chmod 0600 "$HOME/.cache/value"
            ! touch "$HOME/.cargo/bin/injected"
            echo private > "/var/tmp/$3"
            chmod 0600 "/var/tmp/$3"
        "#,
            &[
                &pid_ns,
                Path::new(&std::process::id().to_string()),
                Path::new(&marker),
                Path::new(if s.overlay { "overlay" } else { "readonly" }),
            ],
        );
        assert!(!host_marker.exists(), "literal /var/tmp leaked to host");
        assert_eq!(
            fs::read_to_string(scratch.join("var-tmp").join(&marker)).unwrap(),
            "private\n"
        );
        assert_eq!(
            fs::metadata(root.join("source"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o644
        );
        drop(s);
        assert!(!scratch.exists());
        let restarted = f.prepare(Profile::ReadOnly, &[], &[]).unwrap();
        run(
            &restarted,
            r#"! test -e "/var/tmp/$1""#,
            &[Path::new(&marker)],
        );
        drop(restarted);
        for profile in [Profile::Guarded, Profile::Development] {
            let s = f
                .prepare(profile, &[root.clone()], &[f.0.join("outside")])
                .unwrap();
            run(
                &s,
                r#"
                echo writable > output
                chmod 0600 output
                ! chmod 0600 "$1"
                ! sh -c 'echo wrong > "$1"' fixture "$1"
                ! cat .cortex-dev/private
                ! touch .cortex-dev/new
                ! chmod 0700 .cortex-dev
                mkdir -p "$TMPDIR/cache"
                echo cache > "$TMPDIR/cache/value"
                chmod 0600 "$TMPDIR/cache/value"
            "#,
                &[&f.0.join("outside")],
            );
            assert_eq!(
                fs::read_to_string(root.join("output")).unwrap(),
                "writable\n"
            );
            assert_eq!(
                fs::read_to_string(f.0.join("outside")).unwrap(),
                "protected-fixture"
            );
        }
        eprintln!(
            "real namespace fixtures passed: scratch, metadata, protected descendants, process view, restart"
        );
    }
    #[test]
    fn auto_writes_the_store_directory_but_not_live_store_files() {
        if !capability() {
            eprintln!("namespace capability unavailable; auto store view not exercised");
            return;
        }
        let f = Fixture::new();
        let root = f.root();
        for dir in [".cortex", ".cortex-dev"] {
            fs::create_dir_all(root.join(dir)).unwrap();
            for name in ["cortex.sqlite3", "cortex.sqlite3-wal", "cortex.sqlite3-shm", ".store.lock"] {
                fs::write(root.join(dir).join(name), "live").unwrap();
            }
            fs::write(root.join(dir).join("notes.md"), "old").unwrap();
        }
        let s = f.prepare(Profile::Auto, &[root.clone()], &[]).unwrap();
        run(
            &s,
            r#"
            for d in .cortex .cortex-dev; do
                echo new > $d/x
                echo changed > $d/notes.md
                mkdir $d/sub; echo n > $d/sub/y
                for n in cortex.sqlite3 cortex.sqlite3-wal cortex.sqlite3-shm .store.lock; do
                    ! sh -c 'echo bad >> "$1"' fixture $d/$n
                    ! rm $d/$n
                    ! mv $d/x $d/$n
                done
            done
        "#,
            &[],
        );
        for dir in [".cortex", ".cortex-dev"] {
            assert_eq!(fs::read_to_string(root.join(dir).join("x")).unwrap(), "new\n");
            assert_eq!(fs::read_to_string(root.join(dir).join("notes.md")).unwrap(), "changed\n");
            assert_eq!(fs::read_to_string(root.join(dir).join("cortex.sqlite3")).unwrap(), "live");
            assert_eq!(fs::read_to_string(root.join(dir).join(".store.lock")).unwrap(), "live");
        }
    }
    #[test]
    fn guarded_and_development_keep_the_store_directory_read_only() {
        if !capability() {
            eprintln!("namespace capability unavailable; store view not exercised");
            return;
        }
        let f = Fixture::new();
        let root = f.root();
        fs::create_dir(root.join(".cortex")).unwrap();
        fs::write(root.join(".cortex/cortex.sqlite3"), "live").unwrap();
        for profile in [Profile::Guarded, Profile::Development] {
            let s = f.prepare(profile, &[root.clone()], &[]).unwrap();
            let argv = ["/bin/sh".into(), "-c".into(), "echo x > .cortex/x".into()];
            let out = s.command(&argv).unwrap().output().unwrap();
            assert!(!out.status.success());
            assert!(String::from_utf8_lossy(&out.stderr).contains("Read-only file system"));
            assert!(!root.join(".cortex/x").exists());
        }
    }
    #[test]
    fn ignored_grants_relocated_caches_and_project_toolchain() {
        let f = Fixture::new();
        let root = f.root();
        fs::create_dir(root.join("cache")).unwrap();
        fs::create_dir(root.join("alias-target")).unwrap();
        fs::create_dir_all(root.join(".venv/bin")).unwrap();
        fs::write(root.join(".gitignore"), "cache/\nalias/\n").unwrap();
        std::os::unix::fs::symlink(root.join("alias-target"), root.join("alias")).unwrap();
        assert!(Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(&root)
            .status()
            .unwrap()
            .success());
        let grants = ignored_directories(&root).unwrap();
        assert_eq!(grants, vec![root.join("cache")]);
        fs::create_dir(f.0.join("relocated-cache")).unwrap();
        let mut environment = BTreeMap::new();
        environment.insert("HOME".into(), f.0.join("home").into_os_string());
        environment.insert(
            "UV_CACHE_DIR".into(),
            f.0.join("relocated-cache").into_os_string(),
        );
        environment.insert("CORTEX_CHANNEL_SECRET".into(), "never-forward".into());
        environment.insert("OLLAMA_CLOUD_BACKUP_KEY".into(), "never-forward".into());
        let result =
            Sandbox::with_environment(&root, Profile::ReadOnly, &grants, &[], false, environment);
        if !capability() {
            assert!(result.is_err());
            return;
        }
        let s = result.unwrap();
        run(
            &s,
            r#"
            touch cache/verification-output
            touch "$UV_CACHE_DIR/metadata"
            test "$VIRTUAL_ENV" = "$PWD/.venv"
            case "$PATH" in "$PWD/.venv/bin:"*) ;; *) exit 3;; esac
            test -z "${CORTEX_CHANNEL_SECRET:-}"
            test -z "${OLLAMA_CLOUD_BACKUP_KEY:-}"
            if test "$1" = overlay; then echo discarded > source; else ! touch source; fi
            ! touch .git/config
        "#,
            &[Path::new(if s.overlay { "overlay" } else { "readonly" })],
        );
        assert_eq!(
            fs::read_to_string(root.join("source")).unwrap(),
            "unchanged"
        );
        assert!(root.join("cache/verification-output").is_file());
        assert!(f.0.join("relocated-cache/metadata").is_file());
    }
    /// A worker command (Guarded and Development views) runs a program found only in a
    /// directory the owner's login shell adds, through a version-manager style link,
    /// while a search entry inside the project is never honoured.
    #[test]
    fn worker_commands_find_programs_on_the_owners_shell_path() {
        let s = crate::tool_path::tests::Scratch::new("worker");
        let lib = s.dir("home/.nvm/versions/node/v24.1.0/lib");
        let bin = s.dir("home/.nvm/versions/node/v24.1.0/bin");
        fs::write(lib.join("tool.js"), "#!/bin/sh\necho fixture-node-tool\n").unwrap();
        fs::set_permissions(lib.join("tool.js"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("../lib/tool.js", bin.join("cortex-fixture-node")).unwrap();
        let shell = crate::tool_path::tests::login_shell(
            &s,
            &format!("echo noisy banner\nexport PATH={}:$PATH\n", bin.display()),
        );
        let home = s.0.join("home");
        let detected = crate::tool_path::detect(
            &shell,
            &home,
            std::ffi::OsStr::new("/usr/local/bin:/usr/bin:/bin"),
            &[],
            Duration::from_secs(5),
        );
        assert_eq!(detected.source, crate::tool_path::Source::Shell);
        assert!(
            detected.directories.contains(&bin),
            "{:?}",
            detected.directories
        );
        let root = s.dir("project");
        let planted = s.dir("project/tools");
        fs::write(
            planted.join("cortex-project-tool"),
            "#!/bin/sh\necho planted\n",
        )
        .unwrap();
        fs::set_permissions(
            planted.join("cortex-project-tool"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let mut entries = vec![planted.clone()];
        entries.extend(detected.directories.clone());
        let mut environment = BTreeMap::new();
        environment.insert("HOME".into(), home.into_os_string());
        environment.insert("PATH".into(), std::env::join_paths(entries).unwrap());
        let path = command_path(&root, &environment).unwrap();
        assert!(!std::env::split_paths(&path).any(|p| p == planted));
        if !capability() {
            eprintln!(
                "skipped sandbox launch: bubblewrap cannot create user namespaces on this host"
            );
            return;
        }
        for profile in [Profile::Guarded, Profile::Development] {
            let sandbox = Sandbox::with_environment(
                &root,
                profile,
                &[root.clone()],
                &[],
                false,
                environment.clone(),
            )
            .unwrap();
            let out = sandbox
                .command(&["cortex-fixture-node".into()])
                .unwrap()
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{profile:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&out.stdout), "fixture-node-tool\n");
            run(&sandbox, "! command -v cortex-project-tool", &[]);
        }
        // The verifier's read-only view runs it too, and cannot write the tool directory.
        let sandbox =
            Sandbox::with_environment(&root, Profile::ReadOnly, &[], &[], false, environment)
                .unwrap();
        run(
            &sandbox,
            r#"test "$(cortex-fixture-node)" = fixture-node-tool; ! touch "$1/injected""#,
            &[&bin],
        );
    }
    /// Manual benchmark of command preparation on a real project, never run by default:
    /// `CORTEX_PREPARATION_BENCH_ROOT=<project> cargo test -p workspace-service --lib
    /// preparation_benchmark -- --ignored --nocapture`. It uses the read-only profile
    /// with the whole project as its write set, so every regular file is inspected
    /// for hard links, yet nothing in the project is ever written.
    #[test]
    #[ignore]
    fn preparation_benchmark() {
        let Some(root) = std::env::var_os("CORTEX_PREPARATION_BENCH_ROOT").map(PathBuf::from)
        else {
            return;
        };
        let root = root.canonicalize().unwrap();
        let mut environment = BTreeMap::new();
        environment.insert(
            "HOME".into(),
            std::env::var_os("HOME").unwrap_or_else(|| "/nonexistent".into()),
        );
        for round in 0..5 {
            let mut timings = BTreeMap::new();
            let started = Instant::now();
            let s = Sandbox::with_environment_timed(
                &root,
                Profile::ReadOnly,
                &[root.clone()],
                &[],
                false,
                environment.clone(),
                &mut || Ok(()),
                &mut timings,
            )
            .unwrap();
            eprintln!(
                "round {round}: full preparation {} ms, stages {timings:?}",
                started.elapsed().as_millis(),
            );
            drop(s);
        }
    }
    #[test]
    fn failed_setup_never_executes_payload() {
        let f = Fixture::new();
        let result = Sandbox::with_binary(
            "/bin/false".into(),
            &f.root(),
            Profile::ReadOnly,
            &[],
            &[],
            false,
            BTreeMap::new(),
        );
        let e = match result {
            Err(e) => e,
            Ok(_) => panic!("failed namespace setup accepted"),
        };
        assert_eq!(
            e.to_string(),
            "native_command_sandbox_unavailable: namespace_or_mount_setup_failed"
        );
        assert!(!f.root().join("payload-ran").exists());
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        self.finish();
        // Only this launch's private, randomly named scratch; never a caller path.
        let _ = policy::cleanup_scratch(&self.scratch);
    }
}

/// One empty file with no permissions, bound over a live store file in Auto
/// command sandboxes so its bytes are neither readable nor writable.
fn sealed_file() -> io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    static SEALED: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    if let Some(p) = SEALED.get() {
        return Ok(p.clone());
    }
    let dir = std::env::temp_dir().join(format!("cortex-sealed-{}", std::process::id()));
    fs::create_dir_all(&dir)?;
    let p = dir.join("sealed");
    if !p.exists() {
        fs::write(&p, b"")?;
    }
    fs::set_permissions(&p, fs::Permissions::from_mode(0o000))?;
    Ok(SEALED.get_or_init(|| p).clone())
}
