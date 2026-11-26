//! Worker command profiles on the host (Guarded, Development, the read-only roles
//! and runtime verification). The command runs at its real working directory with
//! the real filesystem, the owner's tools, PATH and HOME and the shared network;
//! a Landlock ruleset (`native_sandbox`) entered between fork and exec restricts
//! its writes to the profile's scope and hides the private/secret set:
//!
//! - read-only roles: writes only to the private scratch (TMPDIR), the explicit
//!   write grants of runtime verification (ignored build directories) and the
//!   package/build caches; the owner's keys and credential stores, the project's
//!   secret files and the live Cortex stores are unreadable;
//! - Guarded: the checkout (or the granted paths in it), scratch and the owner's
//!   tool directories; the same hidden set;
//! - Development: as Guarded plus the shared `/tmp` and `/var/tmp`; only the live
//!   Cortex stores are hidden;
//! - Auto (only where a command still takes a profile): as Development with a
//!   writable `.cortex` directory whose live store files stay hidden.
//!
//! Control paths (engine and Cortex configuration, owner write denials, the
//! service executable) stay read-only in every profile. A writer's granted
//! checkout is an open write tree: entries inside it (`.cortex`, `.env`, owner
//! denials) are not protected by Landlock, which would otherwise make files
//! created directly in the checkout unusable; the live-store argv seal and
//! deletion consent still apply there. No namespace, mount, overlay or snapshot
//! is created.
#[path = "native_command_live_policy.rs"]
pub(crate) mod policy;
#[path = "native_command_ssh.rs"]
mod ssh;
use crate::native_sandbox::{self, HostPolicy};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs, io,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) use crate::native_command_sandbox::Profile;

pub(crate) struct Sandbox {
    root: PathBuf,
    scratch: PathBuf,
    environment: BTreeMap<OsString, OsString>,
    rules: Arc<native_sandbox::Sandbox>,
    ssh_environment: BTreeMap<OsString, OsString>,
    /// Worktree and managed commands may only run inside their checkout.
    confined_cwd: bool,
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
        Self::with_environment_timed(
            root,
            profile,
            writes,
            protected,
            network,
            environment,
            None,
            &mut || Ok(()),
            &mut BTreeMap::new(),
        )
    }

    /// `timings` receives per-stage durations (milliseconds) and counts, also when
    /// preparation fails: numbers only, never paths or names. `secrets` is the
    /// service's index of the checkout's secret files (`indexed_project_secrets`);
    /// without it the project is walked here.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn prepare_authorized(
        root: &Path,
        profile: Profile,
        writes: &[PathBuf],
        protected: &[PathBuf],
        secrets: Option<&[PathBuf]>,
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
            secrets,
            before_write,
            timings,
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
        secrets: Option<&[PathBuf]>,
        before_write: &mut dyn FnMut() -> io::Result<()>,
        timings: &mut BTreeMap<&'static str, u64>,
    ) -> io::Result<Self> {
        // Owner decision: every command shares the host network.
        let _ = network;
        let started = Instant::now();
        let result =
            Self::build(root, profile, writes, protected, environment, secrets, before_write, timings);
        timings.insert("preparation_ms", started.elapsed().as_millis() as u64);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn build(
        root: &Path,
        profile: Profile,
        writes: &[PathBuf],
        protected: &[PathBuf],
        mut environment: BTreeMap<OsString, OsString>,
        secrets: Option<&[PathBuf]>,
        before_write: &mut dyn FnMut() -> io::Result<()>,
        timings: &mut BTreeMap<&'static str, u64>,
    ) -> io::Result<Self> {
        let millis = |since: Instant| since.elapsed().as_millis() as u64;
        let root = root.canonicalize()?;
        if root == Path::new("/") {
            return Err(unavailable("invalid_project_mount"));
        }
        let home = policy::env_path(&environment, "HOME").filter(|p| p.is_absolute() && p.is_dir());
        let controls = Instant::now();
        let mut read_only = policy::expand(&root, protected)?;
        let controls_paths = policy::controls(&root, &environment)?;
        timings.insert("controls_ms", millis(controls));
        // The project write grants are open trees: Landlock cannot protect an entry
        // inside them without breaking files created directly in them.
        let mut project = Vec::new();
        let mut grants = Vec::new();
        for p in writes {
            let p = match p.canonicalize() {
                Ok(p) => p,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            };
            if !p.starts_with(&root) {
                return Err(unavailable("write_grant_outside_project"));
            }
            project.push(p);
        }
        // Control paths stay read-only, except inside the writer's open checkout
        // (only explicit owner denials narrow it; see native_sandbox).
        read_only.extend(controls_paths.into_iter().filter(|c| {
            let real = c.canonicalize().unwrap_or_else(|_| c.clone());
            !project.iter().any(|w| real.starts_with(w))
        }));
        grants.extend(policy::tool_caches(&environment, &root, profile)?);
        if profile.wide() {
            grants.extend(["/tmp", "/var/tmp"].map(PathBuf::from));
        }
        grants.extend(native_sandbox::devices());
        let mut hidden = native_sandbox::live_stores(&root);
        if profile == Profile::Auto {
            // The store directories are writable; their live files stay hidden.
            let dirs: Vec<PathBuf> = hidden
                .iter()
                .filter(|p| p.file_name().is_some_and(|n| n == ".cortex" || n == ".cortex-dev"))
                .cloned()
                .collect();
            hidden.retain(|p| !dirs.contains(p));
            read_only.retain(|p| !dirs.contains(p));
            for dir in dirs {
                if let Ok(entries) = fs::read_dir(&dir) {
                    hidden.extend(
                        entries
                            .flatten()
                            .map(|e| e.path())
                            .filter(|p| crate::native_engine_policy::live_store_file(p)),
                    );
                }
                grants.push(dir);
            }
        }
        if matches!(profile, Profile::ReadOnly | Profile::Guarded) {
            hidden.extend(native_sandbox::secrets(home.as_deref(), &environment));
            // A secret file inside an open write grant is not protected (see
            // native_sandbox): with the whole checkout granted none is looked for.
            // Otherwise the service's index is used, never a walk on this path.
            if !project.iter().any(|w| root.starts_with(w)) {
                let indexed = Instant::now();
                match secrets {
                    Some(listed) => {
                        hidden.extend(listed.iter().filter(|p| p.starts_with(&root)).cloned());
                        hidden.extend(native_sandbox::top_secrets(&root));
                    }
                    None => hidden.extend(native_sandbox::indexed_project_secrets(&root)?),
                }
                timings.insert("secrets_ms", millis(indexed));
            }
        }
        // The scratch is this preparation's only mutation; it needs live authority.
        before_write()?;
        let scratch = std::env::temp_dir().join(format!(
            "cortex-command-{}",
            crate::native_effects::identity().map_err(|_| unavailable("scratch_identity"))?
        ));
        fs::DirBuilder::new().mode(0o700).create(&scratch)?;
        grants.push(scratch.clone());
        let ruled = Instant::now();
        let rules = native_sandbox::Sandbox::host(&HostPolicy {
            // Auto (a profile only where a command still takes one) keeps its live
            // store files hidden even inside the checkout.
            writes: if profile == Profile::Auto { [grants, project.clone()].concat() } else { grants },
            open_writes: if profile == Profile::Auto { Vec::new() } else { project },
            read_only,
            hidden,
        });
        timings.insert("rules_ms", millis(ruled));
        let rules = match rules {
            Ok(r) => Arc::new(r),
            Err(e) => {
                let _ = policy::cleanup_scratch(&scratch);
                return Err(e);
            }
        };
        if profile == Profile::ReadOnly {
            let options = environment
                .get(std::ffi::OsStr::new("PYTEST_ADDOPTS"))
                .map(|v| v.to_string_lossy().into_owned())
                .unwrap_or_default();
            environment.insert(
                "PYTEST_ADDOPTS".into(),
                format!(
                    "{options} -o '{}'",
                    format!("cache_dir={}/pytest-cache", scratch.display())
                        .replace('\'', "'\"'\"'")
                )
                .into(),
            );
        }
        match &home {
            Some(home) => {
                environment.insert("HOME".into(), home.clone().into_os_string());
            }
            None => {
                environment.insert("HOME".into(), scratch.clone().into_os_string());
                environment.insert("XDG_CACHE_HOME".into(), scratch.join(".cache").into_os_string());
            }
        }
        for variable in ["TMPDIR", "TMP", "TEMP"] {
            environment.insert(variable.into(), scratch.clone().into_os_string());
        }
        environment.entry("LANG".into()).or_insert_with(|| "C.UTF-8".into());
        if let Some(path) = command_path(&root, &environment) {
            environment.insert("PATH".into(), path);
        }
        for name in [".venv", "venv"] {
            let p = root.join(name);
            if p.join("bin").is_dir() {
                environment.insert("VIRTUAL_ENV".into(), p.into_os_string());
                break;
            }
        }
        environment.insert("PYTHONDONTWRITEBYTECODE".into(), "1".into());
        let mut s = Self {
            root,
            scratch,
            environment,
            rules,
            ssh_environment: BTreeMap::new(),
            confined_cwd: false,
        };
        ssh::configure(&mut s, home.as_deref())?;
        let probe = Instant::now();
        let probed = s.probe();
        timings.insert("probe_ms", millis(probe));
        probed?;
        Ok(s)
    }

    /// Runs `/bin/true` under this exact ruleset, never the proposed payload.
    fn probe(&self) -> io::Result<()> {
        let mut command = self.command(&["/bin/true".into()])?;
        command.stdout(Stdio::null()).stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|_| unavailable("landlock_launch_failed"))?;
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match child.try_wait()? {
                Some(s) if s.success() => return Ok(()),
                Some(_) => return Err(unavailable("landlock_probe_failed")),
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

    /// The payload itself (resolved on the prepared PATH) at its real working
    /// directory, entering the Landlock ruleset just before exec.
    pub(crate) fn command_at(&self, argv: &[String], cwd: Option<&Path>) -> io::Result<Command> {
        if argv.is_empty() {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty_command"));
        }
        if let Some(cwd) = cwd {
            if self.confined_cwd
                && (!cwd.starts_with(&self.root)
                    || cwd.components().any(|c| matches!(c, std::path::Component::ParentDir)))
            {
                return Err(unavailable("outside_cwd"));
            }
        }
        let mut command = Command::new(&argv[0]);
        command
            .args(&argv[1..])
            .current_dir(cwd.unwrap_or(&self.root))
            .env_clear()
            .stdin(Stdio::null());
        use std::os::unix::process::CommandExt;
        let rules = self.rules.clone();
        unsafe {
            command.pre_exec(move || {
                libc::umask(0o022);
                rules.enter()
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
            let ssh_agent = upper == "SSH_AUTH_SOCK"
                && Path::new(value).is_absolute()
                && !value.as_encoded_bytes().contains(&b'\n')
                && fs::metadata(PathBuf::from(value))
                    .is_ok_and(|m| m.file_type().is_socket() && m.uid() == unsafe { libc::getuid() });
            // Display, audio, GPU and session-bus variables name sockets and drivers,
            // not credentials, so the credential name filter does not apply to them.
            let hardware = crate::native_command_sandbox::devices::hardware_variable(
                &key.to_string_lossy(),
            );
            let prepared = ["HOME", "TMPDIR", "TMP", "TEMP", "PATH", "LANG"]
                .iter()
                .any(|k| key == std::ffi::OsStr::new(k));
            if prepared || hardware || !secret(&key.to_string_lossy()) && (!auth || ssh_agent) {
                command.env(key, value);
            }
        }
        for (key, value) in &self.ssh_environment {
            command.env(key, value);
        }
        Ok(command)
    }

    pub(crate) fn scratch(&self) -> &Path {
        &self.scratch
    }

    pub(crate) fn confine_cwd(&mut self) {
        self.confined_cwd = true;
    }

    pub(crate) fn set_environment(&mut self, key: &str, value: OsString) {
        self.environment.insert(key.into(), value);
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        // Only this launch's private, randomly named scratch; never a caller path.
        let _ = policy::cleanup_scratch(&self.scratch);
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

#[cfg(test)]
#[path = "native_command_live_policy_tests.rs"]
mod advanced_tests;
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub(crate) struct Fixture(pub(crate) PathBuf);
    impl Fixture {
        pub(crate) fn new() -> Self {
            let p = std::env::temp_dir().join(format!(
                "landlock-fixture-{}",
                crate::native_effects::identity().unwrap()
            ));
            fs::create_dir_all(p.join("project/src")).unwrap();
            fs::write(p.join("project/source"), "unchanged").unwrap();
            fs::create_dir(p.join("project/.cortex-dev")).unwrap();
            fs::write(p.join("project/.cortex-dev/private"), "private-fixture").unwrap();
            fs::write(p.join("project/.env"), "TOKEN=fixture").unwrap();
            fs::write(p.join("outside"), "protected-fixture").unwrap();
            fs::create_dir_all(p.join("home/.cache")).unwrap();
            fs::create_dir_all(p.join("home/.cargo/bin")).unwrap();
            fs::create_dir_all(p.join("home/.ssh")).unwrap();
            fs::write(p.join("home/.ssh/id_ed25519"), "key").unwrap();
            fs::write(p.join("home/.ssh/known_hosts"), "hosts").unwrap();
            Self(p)
        }
        pub(crate) fn prepare(
            &self,
            profile: Profile,
            writes: &[PathBuf],
            protected: &[PathBuf],
        ) -> io::Result<Sandbox> {
            let mut environment = BTreeMap::new();
            environment.insert("HOME".into(), self.0.join("home").into_os_string());
            environment.insert("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into());
            Sandbox::with_environment(&self.root(), profile, writes, protected, false, environment)
        }
        pub(crate) fn root(&self) -> PathBuf {
            self.0.join("project").canonicalize().unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    /// Whether this kernel enforces Landlock; without it every preparation fails closed.
    pub(crate) fn capability() -> bool {
        crate::native_sandbox::abi() >= 1
    }
    pub(crate) fn run(s: &Sandbox, script: &str, args: &[&Path]) {
        let mut argv = vec![
            "/bin/sh".to_owned(),
            "-c".into(),
            format!(
                "deny() {{ if \"$@\" 2>/dev/null; then exit 9; fi; }}\nput() {{ echo x > \"$1\"; }}\nset -e\n{script}"
            ),
            "probe".into(),
        ];
        argv.extend(args.iter().map(|p| p.to_string_lossy().into_owned()));
        let out = s.command(&argv).unwrap().output().unwrap();
        assert!(
            out.status.success(),
            "{script}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    #[test]
    fn guarded_writes_only_its_scope_and_hides_secrets() {
        let f = Fixture::new();
        let root = f.root();
        let Ok(s) = f.prepare(Profile::Guarded, &[root.join("src")], &[]) else {
            assert!(!capability(), "Guarded must prepare when Landlock is available");
            eprintln!("Landlock unavailable: Guarded rule check not run");
            return;
        };
        run(
            &s,
            r#"put "$1/src/new.rs"; cat "$1/src/new.rs" "$1/source" >/dev/null
deny put "$1/source"
deny put "$2"
deny cat "$1/.env"
deny cat "$1/.cortex-dev/private"
deny cat "$3/.ssh/id_ed25519"
cat "$3/.ssh/known_hosts" >/dev/null
put "$3/.cache/tool-cache"
put "$TMPDIR/scratch"
test "$HOME" = "$3"
command -v git >/dev/null; ls /usr/bin >/dev/null"#,
            &[&root, &f.0.join("outside"), &f.0.join("home")],
        );
        assert_eq!(fs::read_to_string(root.join("source")).unwrap(), "unchanged");
    }

    /// Preparation never walks the project: a read-only command hides the secret
    /// files the service's index names plus the top level read now, and a writer whose
    /// grant is the whole checkout looks for none.
    #[test]
    fn preparation_takes_secrets_from_the_index_without_a_project_walk() {
        let f = Fixture::new();
        let root = f.root();
        fs::create_dir_all(root.join("svc")).unwrap();
        fs::write(root.join("svc/.env"), "TOKEN=nested").unwrap();
        fs::write(root.join("svc/plain"), "plain").unwrap();
        let mut timings = BTreeMap::new();
        let indexed = [root.join("svc/.env")];
        let prepared = Sandbox::prepare_authorized(
            &root,
            Profile::ReadOnly,
            &[],
            &[],
            Some(&indexed),
            &mut || Ok(()),
            &mut timings,
        );
        assert!(!timings.contains_key("tree_walk_ms"), "{timings:?}");
        assert!(!timings.contains_key("tree_entries"), "{timings:?}");
        let Ok(s) = prepared else {
            assert!(!capability());
            eprintln!("Landlock unavailable: indexed secret check not run");
            return;
        };
        run(
            &s,
            r#"deny cat "$1/svc/.env"
deny cat "$1/.env"
cat "$1/svc/plain" "$1/source" >/dev/null"#,
            &[&root],
        );
        drop(s);
        let mut timings = BTreeMap::new();
        let _whole = Sandbox::prepare_authorized(
            &root,
            Profile::Guarded,
            std::slice::from_ref(&root),
            &[],
            None,
            &mut || Ok(()),
            &mut timings,
        );
        assert!(!timings.contains_key("secrets_ms"), "{timings:?}");
    }

    #[test]
    fn read_only_roles_write_only_scratch_grants_and_caches() {
        let f = Fixture::new();
        let root = f.root();
        fs::create_dir(root.join("target")).unwrap();
        let Ok(s) = f.prepare(Profile::ReadOnly, &[root.join("target")], &[]) else {
            assert!(!capability());
            eprintln!("Landlock unavailable: read-only rule check not run");
            return;
        };
        run(
            &s,
            r#"deny put "$1/src/x"
deny put "$1/source"
put "$1/target/build"
put "$TMPDIR/x"
put "$2/.cache/x"
deny cat "$1/.env"
deny cat "$2/.ssh/id_ed25519"
cat "$1/source" >/dev/null"#,
            &[&root, &f.0.join("home")],
        );
    }

    #[test]
    fn development_writes_the_whole_checkout_and_shares_var_tmp() {
        let f = Fixture::new();
        let root = f.root();
        let Ok(s) = f.prepare(Profile::Development, &[root.clone()], &[]) else {
            assert!(!capability());
            eprintln!("Landlock unavailable: Development rule check not run");
            return;
        };
        run(
            &s,
            r#"put "$1/src/new.rs"
put "$1/new-top-level"; cat "$1/new-top-level" >/dev/null
mkdir "$1/newdir"; put "$1/newdir/x"
cat "$1/.env" "$2/.ssh/id_ed25519" >/dev/null
t=$(mktemp -p /var/tmp cortex-dev-probe.XXXXXX); rm -f "$t""#,
            &[&root, &f.0.join("home")],
        );
    }

    #[test]
    fn auto_writes_the_store_directory_but_not_live_store_files() {
        let f = Fixture::new();
        let root = f.root();
        let store = root.join(".cortex");
        fs::create_dir(&store).unwrap();
        fs::write(store.join("cortex.sqlite3"), "db").unwrap();
        fs::write(store.join("notes.md"), "notes").unwrap();
        fs::create_dir(store.join("plans")).unwrap();
        let Ok(s) = f.prepare(Profile::Auto, &[root.clone()], &[]) else {
            assert!(!capability());
            eprintln!("Landlock unavailable: Auto rule check not run");
            return;
        };
        run(
            &s,
            r#"put "$1/.cortex/notes.md"
put "$1/.cortex/plans/plan.md"
deny cat "$1/.cortex/cortex.sqlite3"
deny put "$1/.cortex/cortex.sqlite3""#,
            &[&root],
        );
        assert_eq!(fs::read_to_string(store.join("cortex.sqlite3")).unwrap(), "db");
    }

    #[test]
    fn preparation_fails_closed_without_running_the_payload() {
        let f = Fixture::new();
        let root = f.root();
        // A write grant outside the checkout is refused before any process starts.
        let e = f
            .prepare(Profile::Guarded, &[f.0.join("home")], &[])
            .err()
            .expect("outside grant refused");
        assert_eq!(
            e.to_string(),
            "native_command_sandbox_unavailable: write_grant_outside_project"
        );
        assert!(!root.join("payload-ran").exists());
    }

    #[test]
    fn worker_commands_find_programs_on_the_owners_shell_path() {
        let s = crate::tool_path::tests::Scratch::new("worker");
        let lib = s.dir("home/.nvm/versions/node/v24.1.0/lib");
        let bin = s.dir("home/.nvm/versions/node/v24.1.0/bin");
        fs::write(lib.join("tool.js"), "#!/bin/sh\necho fixture-node-tool\n").unwrap();
        fs::set_permissions(lib.join("tool.js"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("../lib/tool.js", bin.join("cortex-fixture-node")).unwrap();
        let root = s.dir("project");
        let planted = s.dir("project/tools");
        fs::write(planted.join("cortex-project-tool"), "#!/bin/sh\necho planted\n").unwrap();
        fs::set_permissions(
            planted.join("cortex-project-tool"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let home = s.0.join("home");
        let mut environment = BTreeMap::new();
        environment.insert("HOME".into(), home.into_os_string());
        environment.insert(
            "PATH".into(),
            std::env::join_paths([planted.clone(), bin.clone(), PathBuf::from("/usr/bin"), PathBuf::from("/bin")])
                .unwrap(),
        );
        let path = command_path(&root, &environment).unwrap();
        assert!(!std::env::split_paths(&path).any(|p| p == planted));
        if !capability() {
            eprintln!("Landlock unavailable: sandboxed launch not run");
            return;
        }
        for profile in [Profile::Guarded, Profile::Development, Profile::ReadOnly] {
            let writes = if profile == Profile::ReadOnly { vec![] } else { vec![root.clone()] };
            let sandbox =
                Sandbox::with_environment(&root, profile, &writes, &[], false, environment.clone())
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
            run(&sandbox, "deny command -v cortex-project-tool", &[]);
        }
        let sandbox =
            Sandbox::with_environment(&root, Profile::ReadOnly, &[], &[], false, environment)
                .unwrap();
        run(&sandbox, r#"deny put "$1/injected""#, &[&bin]);
    }
}
