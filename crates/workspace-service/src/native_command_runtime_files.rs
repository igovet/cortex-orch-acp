//! Fixed runtime grants for the sparse command view. PATH and argv
//! cannot request arbitrary filesystem mounts.
use super::{policy, unavailable, Sandbox};
use std::{
    env,
    ffi::OsString,
    fs,
    io::{self, Read},
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) const HOME: &str = "/run/cortex-command";
const USR: &[&str] = &[
    "/usr/bin",
    "/usr/lib",
    "/usr/lib64",
    "/usr/libexec",
    "/usr/share/nodejs",
    "/usr/share/git-core",
    "/usr/share/perl",
    "/usr/share/pkgconfig",
    "/usr/include",
];
// Exact system files, with narrowly bounded distribution-specific aliases.
const SYSTEM_FILES: &[(&str, &[&str])] = &[
    ("/etc/ld.so.cache", &["/etc"]),
    ("/etc/ssl/certs/ca-certificates.crt", &["/etc/ssl/certs"]),
    ("/etc/resolv.conf", &["/etc", "/run/systemd/resolve", "/run/resolvconf",
        "/run/NetworkManager"]),
    ("/etc/hosts", &["/etc"]),
    ("/etc/nsswitch.conf", &["/etc"]),
    ("/etc/localtime", &["/etc", "/usr/share/zoneinfo"]),
];

fn overlaps(a: &Path, b: &Path) -> bool {
    a.starts_with(b) || b.starts_with(a)
}

#[derive(Clone, Copy)]
enum InputKind {
    Directory,
    File,
    Executable,
    ExtraExecutable,
}

impl InputKind {
    fn accepts(self, meta: &fs::Metadata) -> bool {
        match self {
            Self::Directory => meta.is_dir(),
            Self::File => meta.is_file(),
            Self::Executable => meta.is_file() && meta.mode() & 0o111 != 0,
            Self::ExtraExecutable => {
                meta.is_file() && meta.mode() & 0o111 != 0 && meta.nlink() == 1
            }
        }
    }
}

impl Sandbox {
    fn optional_runtime_input(
        &mut self,
        source: &Path,
        target: &Path,
        grant: (&[&Path], InputKind),
        root: &Path,
        protected: &[PathBuf],
    ) -> io::Result<bool> {
        let real = match source.canonicalize() {
            Ok(real) => real,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(e) => return Err(e),
        };
        let (bases, kind) = grant;
        if matches!(kind, InputKind::ExtraExecutable) && real != source
            || !bases.iter().any(|base| real != *base && real.starts_with(base))
            || overlaps(&real, root)
            || overlaps(target, root)
            || protected
                .iter()
                .any(|p| overlaps(&real, p) || p.canonicalize().is_ok_and(|p| overlaps(&real, &p)))
        {
            return Err(unavailable("runtime_input_overlap_or_alias"));
        }
        // Check and mount the same inode. In particular do not resolve a
        // tool-directory entry a second time after checking its target.
        let pin = Arc::new(policy::pin(&real)?);
        let meta = pin.metadata()?;
        if !kind.accepts(&meta) || meta.mode() & 0o022 != 0 {
            return Err(unavailable("unsafe_runtime_input"));
        }
        cortex_engine::filesystem_view::bind_pinned(
            &mut self.args, &mut self.pins, pin, target, true,
        )?;
        Ok(true)
    }

    pub(super) fn extra_tools(
        &mut self,
        root: &Path,
        protected: &[PathBuf],
    ) -> io::Result<Vec<PathBuf>> {
        let mut directories = Vec::new();
        let configured = self
            .environment
            .get(std::ffi::OsStr::new(crate::tool_path::COMMAND_EXTRAS))
            .cloned()
            .unwrap_or_default();
        for (index, raw) in env::split_paths(&configured)
            .filter(|p| !p.as_os_str().is_empty())
            .enumerate()
        {
            if index >= crate::tool_path::MAX_EXTRA {
                return Err(unavailable("extra_tool_directory_limit"));
            }
            let dir = crate::tool_path::valid(&raw, protected)
                .map_err(|_| unavailable("unsafe_extra_tool_directory"))?;
            if overlaps(&dir, root) || protected.iter().any(|p| overlaps(&dir, p)) {
                return Err(unavailable("unsafe_extra_tool_directory"));
            }
            let dest = Path::new(HOME).join("tools").join(index.to_string());
            for (count, entry) in fs::read_dir(&dir)?.enumerate() {
                if count >= 4096 {
                    return Err(unavailable("extra_tool_entry_limit"));
                }
                let entry = entry?;
                let meta = entry.path().symlink_metadata()?;
                if meta.is_file()
                    && meta.nlink() == 1
                    && meta.mode() & 0o111 != 0
                    && !entry.file_name().as_encoded_bytes().starts_with(b".")
                {
                    self.optional_runtime_input(
                        &entry.path(),
                        &dest.join(entry.file_name()),
                        (&[&dir], InputKind::ExtraExecutable),
                        root,
                        protected,
                    )?;
                }
            }
            directories.push(dest);
        }
        Ok(directories)
    }

    pub(super) fn runtime_inputs(&mut self, root: &Path, protected: &[PathBuf]) -> io::Result<()> {
        self.runtime_inputs_at(root, protected, Path::new(HOME))
    }

    /// Language servers use a small tmpfs as HOME. The normal command
    /// profile leaves the generated default in its own bound scratch.
    pub(super) fn runtime_inputs_at(
        &mut self,
        root: &Path,
        protected: &[PathBuf],
        view: &Path,
    ) -> io::Result<()> {
        // System executables, loaders, headers and named package data. Do not
        // include /usr/local, /etc, /run or any containing home or cache root.
        for name in USR {
            let path = Path::new(name);
            self.optional_runtime_input(path, path, (&[Path::new("/usr")], InputKind::Directory), root, protected)?;
        }
        for (name, bases) in SYSTEM_FILES {
            let path = Path::new(name);
            let bases: Vec<_> = bases.iter().map(Path::new).collect();
            self.optional_runtime_input(path, path, (&bases, InputKind::File), root, protected)?;
        }
        // On merged-/usr Debian systems these executable links go through
        // individual /etc/alternatives entries. Never mount that directory.
        for name in ["cc", "c++", "awk"] {
            let path = Path::new("/etc/alternatives").join(name);
            if Path::new("/usr/bin")
                .join(name)
                .read_link()
                .is_ok_and(|target| target == path)
            {
                // Keep the target's executable identity. Binding a regular file
                // over this alias makes GCC resolve its own path in /etc instead
                // of /usr/bin, and it no longer finds its helpers and plugins.
                let target = path.canonicalize()?;
                if target.parent() != Some(Path::new("/usr/bin"))
                    || !InputKind::Executable.accepts(&target.metadata()?)
                {
                    return Err(unavailable("invalid_system_tool_path"));
                }
                self.args.extend([
                    "--symlink".into(),
                    target.into_os_string(),
                    path.into_os_string(),
                ]);
            }
        }
        for (name, usr) in [
            ("/bin", "/usr/bin"),
            ("/lib", "/usr/lib"),
            ("/lib64", "/usr/lib64"),
        ] {
            let path = Path::new(name);
            if path.is_symlink() {
                if path.canonicalize()? != Path::new(usr).canonicalize()? {
                    return Err(unavailable("invalid_system_tool_path"));
                }
                self.args
                    .extend(["--symlink".into(), usr.into(), name.into()]);
            } else {
                self.optional_runtime_input(path, path, (&[Path::new("/")], InputKind::Directory), root, protected)?;
            }
        }

        let Some(home) = policy::env_path(&self.environment, "HOME").filter(|p| p.is_absolute())
        else {
            return Ok(());
        };
        let home = home.canonicalize()?;
        if home == Path::new("/") {
            return Err(unavailable("invalid_toolchain_home"));
        }
        // Known rustup/cargo layout only; in particular do not bind .cargo or
        // .rustup, settings, credentials, package locks, or redirected caches.
        let cargo = home.join(".cargo");
        let rustup = home.join(".rustup");
        let mut rust = false;
        for name in [
            "cargo",
            "rustc",
            "rustdoc",
            "rustfmt",
            "cargo-fmt",
            "cargo-clippy",
            "clippy-driver",
            "rustup",
        ] {
            let rel = Path::new("bin").join(name);
            rust |= self.optional_runtime_input(
                &cargo.join(&rel),
                &view.join(".cargo").join(&rel),
                (&[&cargo], InputKind::Executable),
                root,
                protected,
            )?;
        }
        if rust {
            for name in ["cache", "index", "src"] {
                let rel = Path::new("registry").join(name);
                self.optional_runtime_input(
                    &cargo.join(&rel),
                    &view.join(".cargo").join(&rel),
                    (&[&cargo], InputKind::Directory),
                    root,
                    protected,
                )?;
            }
            let toolchains = rustup.join("toolchains");
            match fs::read_dir(&toolchains) {
                Ok(entries) => {
                    let mounted = pin_toolchains(self, entries, &toolchains, view, root, protected)?;
                    if let Some(name) = rustup_default(&rustup, &mounted, root, protected) {
                        // A generated default (not RUSTUP_TOOLCHAIN) lets explicit
                        // +toolchain and project toolchain files keep precedence.
                        let private = self.scratch.join(".rustup");
                        fs::create_dir(&private)?;
                        let settings = private.join("settings.toml");
                        fs::write(&settings, format!("version = \"12\"\ndefault_toolchain = \"{name}\"\n"))?;
                        if view != Path::new(HOME) {
                            let file = Arc::new(fs::File::open(settings)?);
                            self.args.extend([
                                "--file".into(),
                                file.as_raw_fd().to_string().into(),
                                view.join(".rustup/settings.toml").into_os_string(),
                            ]);
                            self.pins.push(file);
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

fn pin_toolchains(
    sandbox: &mut Sandbox,
    entries: fs::ReadDir,
    source: &Path,
    view: &Path,
    root: &Path,
    protected: &[PathBuf],
) -> io::Result<Vec<OsString>> {
    let mut mounted = Vec::new();
    for (index, entry) in entries.enumerate() {
        if index >= 64 {
            return Err(unavailable("rust_toolchain_limit"));
        }
        let entry = entry?;
        if entry.file_type()?.is_dir()
            && sandbox.optional_runtime_input(
                &entry.path(),
                &view.join(".rustup/toolchains").join(entry.file_name()),
                (&[source], InputKind::Directory),
                root,
                protected,
            )?
        {
            mounted.push(entry.file_name());
        }
    }
    Ok(mounted)
}

/// Extract only the top-level default from rustup's own serialized settings.
/// Do not bind the file, inherit overrides, or add a mount based on its value.
fn rustup_default(
    rustup: &Path,
    mounted: &[OsString],
    root: &Path,
    protected: &[PathBuf],
) -> Option<String> {
    if mounted.is_empty() {
        return None;
    }
    let path = rustup.join("settings.toml");
    if overlaps(&path, root)
        || protected.iter().any(|p| {
            overlaps(&path, p) || p.canonicalize().is_ok_and(|p| overlaps(&path, &p))
        })
    {
        return None;
    }

    // Pin without following any symlink. Check the same inode we will read,
    // rejecting hardlinks and non-regular files before opening it for reading.
    let pin = policy::pin(&path).ok()?;
    let meta = pin.metadata().ok()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & 0o022 != 0
    {
        return None;
    }
    const MAX_SETTINGS_BYTES: u64 = 64 * 1024;
    let mut settings = String::new();
    fs::File::open(format!("/proc/self/fd/{}", pin.as_raw_fd()))
        .ok()?
        .take(MAX_SETTINGS_BYTES + 1)
        .read_to_string(&mut settings)
        .ok()?;
    if settings.len() as u64 > MAX_SETTINGS_BYTES {
        return None;
    }
    let mut entries = settings
        .lines()
        .map(str::trim)
        .take_while(|line| !line.starts_with('['))
        .filter_map(|line| line.strip_prefix("default_toolchain = "));
    let name = entries.next()?.strip_prefix('"')?.strip_suffix('"')?;
    if entries.next().is_some()
        || !name.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
        || !mounted.iter().any(|entry| entry == name)
    {
        return None;
    }
    Some(name.to_owned())
}

/// Project-local program lookup is preserved; arbitrary host PATH entries
/// remain invisible even when a command knows their absolute spelling.
pub(super) fn path(root: &Path, extra: Vec<PathBuf>) -> Option<OsString> {
    let mut paths: Vec<_> = [".venv/bin", "venv/bin", "node_modules/.bin"]
        .into_iter()
        .map(|name| root.join(name))
        .filter(|p| p.is_dir())
        .collect();
    paths.extend(extra);
    paths.extend([
        Path::new(HOME).join(".cargo/bin"),
        "/usr/bin".into(),
        "/bin".into(),
    ]);
    env::join_paths(paths).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native_command_sandbox::tests::Fixture;
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn rustup_default_requires_an_installed_name_and_safe_settings_file() {
        let f = Fixture::new();
        let root = f.root();
        let rustup = f.0.join("home/.rustup");
        fs::create_dir(&rustup).unwrap();
        let settings = rustup.join("settings.toml");
        let mounted = ["stable-x86_64-unknown-linux-gnu".into()];
        let configured = |protected: &[PathBuf]| rustup_default(&rustup, &mounted, &root, protected);
        let valid = "version = \"12\"\ndefault_toolchain = \"stable-x86_64-unknown-linux-gnu\"\n";
        assert_eq!(configured(&[]), None);
        fs::write(&settings, format!("{valid}[overrides]\n\"/fixture\" = \"nightly\"\n")).unwrap();
        assert_eq!(configured(&[]).as_deref(), mounted[0].to_str());
        assert_eq!(configured(std::slice::from_ref(&settings)), None);
        for invalid in [
            format!("[overrides]\n{valid}"),
            format!("{valid}{valid}"),
            format!("{valid}{}", " ".repeat(64 * 1024)),
            "default_toolchain = \"not-installed\"\n".into(),
            "default_toolchain = \"../stable\"\n".into(),
        ] {
            fs::write(&settings, invalid).unwrap();
            assert_eq!(configured(&[]), None);
        }
        fs::remove_file(&settings).unwrap();
        let outside = f.0.join("outside");
        fs::write(&outside, valid).unwrap();
        symlink(&outside, &settings).unwrap();
        assert_eq!(configured(&[]), None);
        fs::remove_file(&settings).unwrap();
        fs::hard_link(&outside, &settings).unwrap();
        assert_eq!(configured(&[]), None);
    }

    #[test]
    fn runtime_grants_reject_aliases_protected_sources_and_wrong_types() {
        let f = Fixture::new();
        let root = f.root();
        let approved = f.0.join("approved");
        let scratch = f.0.join("scratch");
        fs::create_dir(&approved).unwrap();
        fs::create_dir(&scratch).unwrap();
        let allowed = approved.join("compiler");
        let alias = approved.join("alias");
        let data = approved.join("data");
        let directory = approved.join("directory");
        fs::create_dir(&directory).unwrap();
        fs::write(&allowed, "synthetic executable").unwrap();
        fs::set_permissions(&allowed, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(&data, "synthetic non-tool").unwrap();
        symlink(f.0.join("outside"), &alias).unwrap();
        let mut sandbox = Sandbox {
            binary: "/bin/false".into(),
            args: Vec::new(),
            scratch,
            environment: Default::default(),
            pins: Vec::new(),
            scoped_root: None,
            git_context: None,
        };
        let target = Path::new("/run/cortex-command/tools/compiler");
        assert!(sandbox.optional_runtime_input(
            &allowed, target, (&[&approved], InputKind::ExtraExecutable), &root, &[],
        ).unwrap());
        assert_eq!(sandbox.pins.len(), 1);
        for (source, kind, protected) in [
            (alias.as_path(), InputKind::File, vec![]),
            (directory.as_path(), InputKind::File, vec![]),
            (data.as_path(), InputKind::ExtraExecutable, vec![]),
            (allowed.as_path(), InputKind::ExtraExecutable, vec![allowed.clone()]),
        ] {
            assert!(sandbox.optional_runtime_input(
                source, target, (&[&approved], kind), &root, &protected,
            ).is_err());
        }
        assert_eq!(sandbox.pins.len(), 1);
    }
}
