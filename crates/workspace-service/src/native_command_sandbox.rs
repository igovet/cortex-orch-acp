//! Brokered command filesystem/process views, using the production bubblewrap mechanism.
//! The actual checkout is read-only; writes stay in separate private paths.
//! No unsandboxed or Landlock-only fallback. This is not hostile-code isolation.
#[path = "native_command_policy.rs"]
pub(crate) mod policy;
#[path = "native_command_scoped.rs"]
mod scoped;
#[path = "native_command_runtime_files.rs"]
mod runtime_files;
#[cfg(test)]
#[path = "native_command_git.rs"]
mod git;
pub(crate) const CODEX_SCRATCH: &str = "codex_scratch";
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs, io,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Profile {
    ReadOnly,
    Guarded,
    Development,
}

pub(crate) struct Sandbox {
    binary: PathBuf,
    args: Vec<OsString>,
    scratch: PathBuf,
    environment: BTreeMap<OsString, OsString>,
    pins: Vec<std::sync::Arc<fs::File>>,
    scoped_root: Option<PathBuf>,
    git_context: Option<(PathBuf, PathBuf)>,
}

fn unavailable(reason: &str) -> io::Error {
    io::Error::other(format!("native_command_sandbox_unavailable: {reason}"))
}

impl Sandbox {
    pub(crate) fn prepare_scoped(root: &Path, protected: &[PathBuf]) -> io::Result<Self> {
        let binary = ["/usr/bin/bwrap", "/bin/bwrap"]
            .into_iter()
            .map(PathBuf::from)
            .find(|p| p.is_file())
            .ok_or_else(|| unavailable("bubblewrap_missing"))?;
        Self::scoped(binary, root, protected)
    }

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
        _writes: &[PathBuf],
        protected: &[PathBuf],
        network: bool,
        environment: BTreeMap<OsString, OsString>,
        _before_write: &mut dyn FnMut() -> io::Result<()>,
        timings: &mut BTreeMap<&'static str, u64>,
    ) -> io::Result<Self> {
        let root = root.canonicalize()?;
        if ["/", "/proc", "/dev", "/run"].iter().any(|p| {
            root == Path::new(p) || (*p != "/" && root.starts_with(p))
        }) {
            return Err(unavailable("invalid_project_mount"));
        }
        let temp = Path::new("/tmp").canonicalize()?;
        if temp.starts_with(&root) || Path::new("/var/tmp").starts_with(&root) {
            return Err(unavailable("scratch_overlaps_checkout"));
        }
        if policy::env_path(&environment, "HOME")
            .filter(|home| home.is_absolute())
            .and_then(|home| home.canonicalize().ok())
            .is_some_and(|home| home.starts_with(&root))
        {
            return Err(unavailable("project_overlaps_owner_home"));
        }
        let scratch = temp.join(format!(
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
            scoped_root: None,
            git_context: None,
        };
        s.args.push("--die-with-parent".into());
        if profile == Profile::ReadOnly {
            s.args.extend(["--unshare-pid", "--proc", "/proc", "--remount-ro", "/proc"].map(OsString::from));
        }
        // In a shared PID namespace, host procfs is not a project input. Do
        // not mount it into the view; the existing network and IPC stay shared.
        if !network {
            s.args.push("--unshare-net".into());
        }
        s.args.extend(
            ["--clearenv", "--cap-drop", "ALL", "--dev", "/dev",
             "--setenv", "LANG", "C.UTF-8"]
                .map(OsString::from),
        );
        for path in ["/tmp", "/var/tmp"] {
            s.args.extend(
                ["--perms", "01777", "--size", "1073741824", "--tmpfs", path]
                    .map(OsString::from),
            );
        }
        // Large build artifacts go to a fresh, owner-only directory on disk,
        // not the host HOME, a shared cache, or RAM-backed temp.
        s.mount("--bind", &s.scratch.clone(), Path::new(runtime_files::HOME))?;
        let controls = Instant::now();
        let mut hidden = policy::controls(&root, &s.environment)?;
        hidden.extend(policy::expand(&root, protected)?);
        if hidden.iter().any(|p| root.starts_with(p)) {
            return Err(unavailable("protected_project_mount"));
        }
        s.runtime_inputs(&root, &hidden)?;
        let extra = s.extra_tools(&root, &hidden)?;
        s.project_inputs(&root, &hidden, controls, timings)?;
        s.environment = BTreeMap::from([
            ("HOME".into(), runtime_files::HOME.into()),
            ("CARGO_HOME".into(), format!("{}/.cargo", runtime_files::HOME).into()),
            ("RUSTUP_HOME".into(), format!("{}/.rustup", runtime_files::HOME).into()),
            ("PATH".into(), runtime_files::path(&root, extra).ok_or_else(|| unavailable("command_path_invalid"))?),
            ("TMPDIR".into(), "/tmp".into()),
            ("TMP".into(), "/tmp".into()),
            ("TEMP".into(), "/tmp".into()),
            ("XDG_CACHE_HOME".into(), format!("{}/cache", runtime_files::HOME).into()),
            ("CARGO_TARGET_DIR".into(), format!("{}/target", runtime_files::HOME).into()),
            ("GIT_CONFIG_NOSYSTEM".into(), "1".into()),
            ("GIT_CONFIG_GLOBAL".into(), "/dev/null".into()),
            ("GIT_OPTIONAL_LOCKS".into(), "0".into()),
        ]);
        s.git_config(&[
            "core.hooksPath=/dev/null".into(),
            "core.fsmonitor=false".into(),
            format!("lfs.storage={}/git-lfs", runtime_files::HOME),
        ]);
        s.args
            .extend(["--remount-ro".into(), "/".into(), "--chdir".into(), root.into_os_string()]);
        let probe = Instant::now();
        let probed = s.probe();
        timings.insert("probe_ms", probe.elapsed().as_millis() as u64);
        probed?;
        Ok(s)
    }

    /// The read-only checkout with its audited Git inputs and private masks,
    /// shared by every command view and by a confined engine's working directory.
    fn project_inputs(
        &mut self,
        root: &Path,
        hidden: &[PathBuf],
        controls: Instant,
        timings: &mut BTreeMap<&'static str, u64>,
    ) -> io::Result<()> {
        let checkout = std::sync::Arc::new(policy::pin(root)?);
        let mut git = policy::git_inputs(root, &checkout)?;
        self.git_context = git.first().zip(git.last()).map(|((dir, _), (common, _))| {
            (dir.clone(), common.clone())
        });
        let mut registrations = Vec::new();
        for (path, _) in &git {
            if hidden.iter().any(|h| {
                path.starts_with(h) || h.canonicalize().is_ok_and(|h| path.starts_with(h))
            }) {
                return Err(unavailable("unverified_git_input"));
            }
            let worktrees = path.join("worktrees");
            if worktrees.symlink_metadata().is_ok_and(|m| m.is_dir()) {
                registrations.push(worktrees);
            }
        }
        git.sort_by_key(|(path, _)| path.components().count());
        timings.insert("controls_ms", controls.elapsed().as_millis() as u64);

        // Keep the actual worktree read-only, including ignored paths. Tests
        // and builds use separate per-run locations without changing their inputs.
        let walked = Instant::now();
        let tree = policy::scan(root, &checkout, hidden, &git, &registrations)?;
        timings.insert("tree_walk_ms", walked.elapsed().as_millis() as u64);
        timings.insert("tree_entries", tree.entries as u64);
        cortex_engine::filesystem_view::bind_pinned(
            &mut self.args, &mut self.pins, checkout, root, true,
        )?;
        // Bind the audited Git roots before masking: no later metadata bind
        // may re-expose a private descendant. Only the current linked
        // worktree's pinned directory is admitted through the registration mask.
        for (path, pin) in &git {
            cortex_engine::filesystem_view::bind_pinned(
                &mut self.args, &mut self.pins, pin.clone(), path, true,
            )?;
            let worktrees = path.join("worktrees");
            if registrations.contains(&worktrees) {
                self.args.extend(["--tmpfs".into(), worktrees.into_os_string()]);
            }
        }
        for path in &registrations {
            self.args.extend(["--remount-ro".into(), path.as_os_str().into()]);
        }
        let containing_mount = |path: &Path| {
            git.iter().rev().find_map(|(base, _)| path.starts_with(base).then_some(base.as_path()))
                .or_else(|| path.starts_with(root).then_some(root))
        };
        let mut masks = std::collections::BTreeSet::new();
        for path in tree.private.iter().chain(&tree.private_targets).chain(hidden) {
            if let Ok(real) = path.canonicalize() {
                if root.starts_with(&real) || git.iter().any(|(p, _)| p.starts_with(&real)) {
                    return Err(unavailable("protected_project_mount"));
                }
                if let Some(base) = containing_mount(&real) {
                    if !registrations.iter().any(|p| p.starts_with(base) && real.starts_with(p)) {
                        masks.insert(real);
                    }
                }
            }
        }
        let aliases = policy::external_link_paths(&tree, &masks);
        masks.extend(aliases.into_iter().map(Path::to_path_buf));
        let mut mounted: Vec<PathBuf> = Vec::new();
        for path in masks {
            if !mounted.iter().any(|p| path.starts_with(p)) {
                let base = containing_mount(&path).ok_or_else(|| unavailable("unsafe_private_input"))?;
                self.hide_project_input(base, &path)?;
                mounted.push(path);
            }
        }
        Ok(())
    }

    fn mount(&mut self, kind: &str, source: &Path, target: &Path) -> io::Result<()> {
        let real = source.canonicalize()?;
        let read_only = match kind {
            "--bind" => false,
            "--ro-bind" => true,
            _ => return Err(unavailable("invalid_mount_kind")),
        };
        cortex_engine::filesystem_view::bind(&mut self.args, &mut self.pins, &real, target, read_only)
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
    fn hide_project_input(&mut self, root: &Path, path: &Path) -> io::Result<()> {
        // A symlink outside the project has no target in this view; the scan
        // separately identifies private alias targets that are in the project.
        let meta = match path.symlink_metadata() {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        if meta.is_symlink() {
            return Ok(());
        }
        let path = path.canonicalize()?;
        if !path.starts_with(root) || path == root {
            return Err(unavailable("unsafe_private_input"));
        }
        if meta.is_dir() {
            self.args.extend([
                "--tmpfs".into(),
                path.as_os_str().into(),
                "--remount-ro".into(),
                path.as_os_str().into(),
            ]);
        } else {
            self.mount("--ro-bind", Path::new("/dev/null"), &path)?;
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
            let mapped;
            let cwd = if let Some(root) = &self.scoped_root {
                let relative = cwd
                    .strip_prefix(root)
                    .map_err(|_| unavailable("outside_cwd"))?;
                mapped = Path::new("/workspace").join(relative);
                &mapped
            } else {
                let root = Path::new(self.args.last().ok_or_else(|| unavailable("missing_cwd"))?);
                if !cwd.starts_with(root)
                    || cwd.components().any(|c| matches!(c, std::path::Component::ParentDir))
                {
                    return Err(unavailable("outside_cwd"));
                }
                cwd
            };
            command
                .args(&self.args[..self.args.len() - 2])
                .arg("--chdir")
                .arg(cwd);
        } else {
            command.args(&self.args);
        }
        command.env_clear().stdin(Stdio::null());
        use std::os::{fd::AsRawFd, unix::process::CommandExt};
        let mut pins = self.pins.clone();
        if self.scoped_root.is_some() {
            let filter = fs::File::open(self.scratch.join("seccomp"))?;
            command.arg("--seccomp").arg(filter.as_raw_fd().to_string());
            pins.push(std::sync::Arc::new(filter));
        }
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
        for (key, value) in &self.environment {
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

/// A confined engine's working directory: the same read-only checkout, Git
/// inputs and private masks as a command view (`home` names the engine's own
/// HOME so its controls are masked when they lie inside the checkout). The view
/// arguments and pinned sources are appended to the caller's view.
#[cfg(test)]
pub(crate) fn engine_project_view(
    root: &Path,
    home: &Path,
    args: &mut Vec<OsString>,
    pins: &mut Vec<std::sync::Arc<fs::File>>,
) -> io::Result<()> {
    let root = root.canonicalize()?;
    if ["/", "/proc", "/dev", "/run", "/usr", "/etc"]
        .iter()
        .any(|p| root == Path::new(p) || (*p != "/" && root.starts_with(p)))
    {
        return Err(unavailable("invalid_project_mount"));
    }
    // No scratch: this view part never owns a private directory to clean up.
    let mut view = Sandbox {
        binary: PathBuf::new(),
        args: Vec::new(),
        scratch: PathBuf::new(),
        environment: BTreeMap::from([(OsString::from("HOME"), home.as_os_str().to_owned())]),
        pins: Vec::new(),
        scoped_root: None,
        git_context: None,
    };
    let hidden = policy::controls(&root, &view.environment)?;
    if hidden.iter().any(|p| root.starts_with(p)) {
        return Err(unavailable("protected_project_mount"));
    }
    view.project_inputs(&root, &hidden, Instant::now(), &mut BTreeMap::new())?;
    args.append(&mut view.args);
    pins.append(&mut view.pins);
    Ok(())
}

#[cfg(test)]
#[path = "native_command_policy_tests.rs"]
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
        // Probe namespaces independently of project grants, without giving
        // even the probe a view of the host root or its environment.
        let mut probe = Command::new("/usr/bin/bwrap");
        probe.env_clear().args([
            "--die-with-parent",
            "--unshare-net",
            "--unshare-pid",
            "--proc",
            "/proc",
        ]);
        for name in ["/usr/bin", "/usr/lib", "/usr/lib64", "/bin", "/lib", "/lib64"] {
            let path = Path::new(name);
            if let Ok(link) = path.read_link() {
                probe.arg("--symlink").arg(link).arg(path);
            } else if path.is_dir() {
                probe.arg("--ro-bind").arg(path).arg(path);
            }
        }
        let available = probe
            .args(["--", "/usr/bin/true"])
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
                script.lines().map(|line| {
                    line.trim_start().strip_prefix("! ")
                        .map_or_else(|| line.to_owned(), |command| format!("denied {command}"))
                }).collect::<Vec<_>>().join("\n")
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
        let home = f.0.join("home");
        let db = home.join(".local/share/opencode/opencode.db");
        fs::create_dir_all(db.parent().unwrap()).unwrap();
        let conn = rusqlite::Connection::open(&db).unwrap();
        conn.execute_batch(
            "CREATE TABLE credential (v TEXT); INSERT INTO credential VALUES ('fixture');
             CREATE TABLE session_message (v TEXT); INSERT INTO session_message VALUES ('fixture');",
        ).unwrap();
        fs::copy(&db, root.join("opencode.db")).unwrap();
        fs::hard_link(f.0.join("outside"), root.join("outside-alias")).unwrap();
        let marker = format!("private-{}", crate::native_effects::identity().unwrap());
        let strong = capability();
        for profile in [Profile::ReadOnly, Profile::Guarded, Profile::Development] {
            let result = f.prepare(profile, &[], &[]);
            if !strong {
                let error = result.err().expect("unavailable mechanism accepted");
                assert!(error.to_string().starts_with("native_command_sandbox_unavailable:"), "{error}");
                assert_eq!(fs::read_to_string(root.join("source")).unwrap(), "unchanged");
                eprintln!("{profile:?}: sandbox unavailable; verified only pre-launch refusal");
                continue;
            }
            let s = result.unwrap();
            let scratch = s.scratch().to_owned();
            let outside = f.0.join("outside");
            assert!(s.command_at(&["/bin/true".into()], Some(&f.0)).is_err());
            run(&s, r#"
                test "$(cat source)" = unchanged
                ! test -r "$1"
                ! test -r "$2"
                test ! -s opencode.db
                test ! -s outside-alias
                ! test -r .cortex-dev/private
                ! touch .cortex-dev/new
                test -z "${SSH_AUTH_SOCK:-}"
                mkdir -p "$XDG_CACHE_HOME" "$CARGO_TARGET_DIR"
                echo cache > "$XDG_CACHE_HOME/one"
                echo build > "$CARGO_TARGET_DIR/one"
                ! sh -c 'echo wrong > source'
                ! touch created
                echo tmp > "$TMPDIR/$3"
                echo vartmp > "/var/tmp/$3"
            "#, &[&outside, &db, Path::new(&marker)]);
            assert!(!Path::new("/var/tmp").join(&marker).exists());
            assert!(!Path::new("/tmp").join(&marker).exists());
            assert_eq!(fs::read_to_string(root.join("source")).unwrap(), "unchanged");
            assert!(!root.join("created").exists());
            drop(s);
            assert!(!scratch.exists());
        }
        drop(conn);
    }

    #[test]
    fn a_checkout_cannot_grant_the_owner_home() {
        let f = Fixture::new();
        let root = f.root();
        let environment = BTreeMap::from([("HOME".into(), root.clone().into_os_string())]);
        let error = Sandbox::with_binary(
            "/bin/false".into(), &root, Profile::Guarded, &[], &[], false, environment,
        ).err().expect("fixture HOME was accepted as a checkout");
        assert!(error.to_string().ends_with("project_overlaps_owner_home"));
    }

    #[test]
    fn explicit_tools_and_private_build_outputs() {
        let f = Fixture::new();
        let root = f.root();
        let tool_dir = f.0.join("extra");
        let ambient = f.0.join("ambient");
        fs::create_dir(&tool_dir).unwrap();
        fs::create_dir(&ambient).unwrap();
        for (dir, file) in [(&tool_dir, "fixture-build"), (&ambient, "ambient-tool")] {
            fs::write(dir.join(file), "#!/bin/sh\nprintf 'fixture-build\\n'\n").unwrap();
            fs::set_permissions(dir.join(file), fs::Permissions::from_mode(0o755)).unwrap();
        }
        fs::write(tool_dir.join("not-a-tool"), "not visible").unwrap();
        let environment = BTreeMap::from([
            ("HOME".into(), f.0.join("home").into_os_string()),
            ("PATH".into(), ambient.into_os_string()),
            (crate::tool_path::COMMAND_EXTRAS.into(), tool_dir.into_os_string()),
            ("SSH_AUTH_SOCK".into(), "/not-forwarded".into()),
        ]);
        let result = Sandbox::with_environment(&root, Profile::ReadOnly, &[], &[], false, environment);
        if !capability() {
            assert!(result.is_err());
            eprintln!("verifier tool-input fixture: refusal only");
            return;
        }
        let s = result.unwrap();
        run(&s, r#"
            test "$(fixture-build)" = fixture-build
            ! command -v ambient-tool
            ! test -e "$(dirname "$(command -v fixture-build)")/not-a-tool"
            test -z "${SSH_AUTH_SOCK:-}"
            mkdir -p "$CARGO_TARGET_DIR"
            printf 'int main(void) { return 0; }\n' > "$TMPDIR/main.c"
            cc -o "$CARGO_TARGET_DIR/program" "$TMPDIR/main.c"
            "$CARGO_TARGET_DIR/program"
        "#, &[]);
    }

    #[test]
    fn pinned_rust_build_uses_private_tool_state_and_outputs() {
        rust_build_uses_private_tool_state_and_outputs(true);
    }

    #[test]
    fn unpinned_rust_build_uses_private_default_and_outputs() {
        rust_build_uses_private_tool_state_and_outputs(false);
    }

    fn rust_build_uses_private_tool_state_and_outputs(pinned: bool) {
        let f = Fixture::new();
        let root = f.root();
        let owner_home = std::env::var_os("CARGO_HOME")
            .map(PathBuf::from)
            .filter(|path| path.file_name().is_some_and(|name| name == ".cargo"))
            .and_then(|path| path.parent().map(Path::to_path_buf))
            .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
            .expect("Rust fixture needs the provisioned toolchain owner home");
        fs::create_dir(root.join("src")).unwrap();
        if pinned {
            fs::write(
                root.join("rust-toolchain.toml"),
                include_str!("../../../rust-toolchain.toml"),
            )
            .unwrap();
        }
        fs::write(root.join("Cargo.toml"),
            "[package]\nname = \"sparse-rust-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
        fs::write(root.join("Cargo.lock"),
            "version = 4\n\n[[package]]\nname = \"sparse-rust-fixture\"\nversion = \"0.1.0\"\n").unwrap();
        fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        let environment = BTreeMap::from([
            ("HOME".into(), owner_home.into_os_string()),
            ("CARGO_HOME".into(), "/unforwarded-fixture-cargo".into()),
            ("RUSTUP_HOME".into(), "/unforwarded-fixture-rustup".into()),
        ]);
        let result = Sandbox::with_environment(&root, Profile::ReadOnly, &[], &[], false, environment);
        if !capability() {
            assert!(result.is_err());
            eprintln!("Rust fixture (pinned={pinned}): namespace refusal only");
            return;
        }
        let s = result.unwrap();
        if !pinned {
            run(
                &s,
                r#"
                    test ! -e rust-toolchain
                    test ! -e rust-toolchain.toml
                    test -z "${RUSTUP_TOOLCHAIN:-}"
                    test -s "$RUSTUP_HOME/settings.toml"
                    case "$(rustup show active-toolchain)" in
                        *" (default)") ;;
                        *) exit 98 ;;
                    esac
                "#,
                &[],
            );
        }
        run(&s, r#"
            test "$CARGO_HOME" = "$HOME/.cargo"
            test "$RUSTUP_HOME" = "$HOME/.rustup"
            test "$(command -v cargo)" = "$CARGO_HOME/bin/cargo"
            test ! -r "$CARGO_HOME/credentials.toml"
            HOME="$TMPDIR" cargo build --locked --offline
            "$CARGO_TARGET_DIR/debug/sparse-rust-fixture"
        "#, &[]);
        assert!(!root.join("target").exists());
    }

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
        // Only this launch's private, randomly named scratch; never a caller path.
        let _ = policy::cleanup_scratch(&self.scratch);
    }
}
