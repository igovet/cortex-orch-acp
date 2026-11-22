//! Opt-in for trusted native launches of a privately extracted Dev package.
use crate::local::{canonical_directory, failure, uid};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::{self, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Component, Path, PathBuf},
};
use workspace_protocol::channel::{self, Channel};

const PREFIX: &str = "CORTEX_WORKSPACE_DEV_EXTRACTED_PREFIX";
const ROOTS: [(&str, &str); 5] = [
    ("XDG_CONFIG_HOME", "config"),
    ("XDG_STATE_HOME", "state"),
    ("XDG_RUNTIME_DIR", "run"),
    ("XDG_CACHE_HOME", "cache"),
    ("XDG_DATA_HOME", "data"),
];

/// Shared by the service and desktop so neither falls back to the installed instance
/// on a malformed opt-in. Release never interprets this environment variable.
pub struct DevExtraction(PathBuf);

impl DevExtraction {
    pub fn from_env() -> io::Result<Option<Self>> {
        Self::resolve(channel::current(), 0, |name| std::env::var_os(name))
    }

    fn resolve(
        channel: &Channel,
        system_uid: u32,
        env: impl Fn(&str) -> Option<OsString>,
    ) -> io::Result<Option<Self>> {
        if !channel.is_dev() {
            return Ok(None);
        }
        let Some(prefix) = env(PREFIX).map(PathBuf::from) else {
            return Ok(None);
        };
        let prefix = canonical_directory(&prefix).map_err(|_| invalid())?;
        if prefix.parent().is_none() {
            return Err(invalid());
        }
        // A private directory can still be renamed through an unsafe parent. Permit
        // sticky shared parents (such as /tmp), but not arbitrary writable parents.
        for ancestor in prefix.ancestors().skip(1) {
            let meta = fs::symlink_metadata(ancestor).map_err(|_| invalid())?;
            if !meta.is_dir()
                || meta.uid() != system_uid && meta.uid() != uid()
                || meta.mode() & 0o022 != 0 && meta.mode() & 0o1000 == 0
            {
                return Err(invalid());
            }
        }
        private_root(&prefix)?;
        for (var, leaf) in ROOTS {
            let expected = prefix.join(leaf);
            if env(var).map(PathBuf::from).as_ref() != Some(&expected) {
                return Err(invalid());
            }
            private_root(&expected)?;
        }
        // Refuse a redirected registry, config namespace, or socket directory before
        // the service or desktop can create/read anything under those names.
        for path in [
            channel.config_dir(&prefix.join("config")),
            channel.state_dir(&prefix.join("state")),
            channel.runtime_dir(&prefix.join("run")),
        ] {
            match fs::symlink_metadata(&path) {
                Ok(_) => private_root(&path)?,
                Err(err) if err.kind() == io::ErrorKind::NotFound => (),
                Err(_) => return Err(invalid()),
            }
        }
        Ok(Some(Self(prefix)))
    }

    pub fn runtime_dir(&self) -> PathBuf {
        channel::current().runtime_dir(&self.0.join("run"))
    }

    /// Only the actual executable in this Dev extraction can enable qualification.
    fn qualification_binary(&self, expected: &str) -> io::Result<()> {
        self.validate_qualification_binary(channel::current(), expected, &std::env::current_exe()?)
    }

    fn validate_qualification_binary(
        &self,
        channel: &Channel,
        expected: &str,
        exe: &Path,
    ) -> io::Result<()> {
        let bin = self.0.join("usr/bin");
        let target = bin.join(expected);
        if !channel.is_dev() || exe != target {
            return Err(invalid());
        }
        for directory in [self.0.join("usr"), bin] {
            if !owned_entry(&directory)?.is_dir() {
                return Err(invalid());
            }
        }
        require_file(&target, true)
    }

    /// Trusted native desktop startup only. None must not allow an extracted View PR
    /// to fall through to an installed browser opener.
    pub fn qualification_opener(&self) -> io::Result<Option<OpenerCapture>> {
        let Some(directory) = std::env::var_os("CORTEX_WORKSPACE_QUALIFICATION_OPENER_DIR") else {
            return Ok(None);
        };
        self.qualification_binary(&channel::current().desktop_binary)?;
        Ok(Some(OpenerCapture(PathBuf::from(directory))))
    }

    /// Same-process no-task check; the supplied project must be the disposable
    /// launcher's fixed fixture, not an owner project or a socket request.
    pub async fn qualification_verifier(&self, projects: &Path) -> io::Result<String> {
        self.qualification_binary(&channel::current().service_binary)?;
        if projects != self.0.join("projects") {
            return Err(invalid());
        }
        private_root(projects)?;
        let fixture = projects.join(".qualification-verifier");
        private_root(&fixture)?;
        crate::native_effects::qualification_verifier(fixture)
            .await
            .map_err(|e| failure(e.code()))
    }

    /// Check layout and path safety, not archive provenance or content digests.
    pub fn validate_runtime(&self, requested: &Path) -> io::Result<()> {
        self.validate_layout(channel::current(), requested)
            .map_err(|_| invalid())
    }

    fn validate_layout(&self, channel: &Channel, requested: &Path) -> io::Result<()> {
        let usr = self.0.join("usr");
        let lib = usr.join("lib");
        let package = lib.join(&channel.package);
        let runtime = package.join("runtime");
        if !channel.is_dev() || requested != runtime {
            return Err(invalid());
        }
        let bin = usr.join("bin");
        for dir in [&usr, &lib, &package, &bin] {
            if !owned_entry(dir)?.is_dir() {
                return Err(invalid());
            }
        }
        for name in [&channel.service_binary, &channel.desktop_binary] {
            require_file(&bin.join(name), true)?;
        }
        let engines = package.join("engines");
        let voice = package.join("voice");
        for (tree, links) in [(&runtime, false), (&engines, true), (&voice, false)] {
            check_tree(tree, links)?;
        }
        for (path, executable) in [
            (runtime.join("runtime-payload.json"), false),
            (runtime.join("package-identity.json"), false),
            (engines.join("bundle-manifest.json"), false),
            (engines.join("node/bin/node"), true),
            (voice.join("bundle-manifest.json"), false),
            (voice.join("voice.cjs"), false),
        ] {
            require_file(&path, executable)?;
        }
        match fs::symlink_metadata(runtime.join(".git")) {
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            _ => Err(invalid()),
        }
    }
}

/// Digests written by the native View PR path, never handed to another program.
pub struct OpenerCapture(PathBuf);

impl OpenerCapture {
    pub fn record(&self, url: &str) -> io::Result<()> {
        let parent = canonical_directory(&self.0).map_err(|_| invalid())?;
        if parent != self.0 {
            return Err(invalid());
        }
        private_root(&parent)?;
        let mut file = fs::OpenOptions::new()
            .append(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(parent.join("opener.sha256"))?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.uid() != uid()
            || metadata.nlink() != 1
            || metadata.mode() & 0o777 != 0o600
        {
            return Err(invalid());
        }
        file.write_all(format!("{:x}\n", Sha256::digest(url.as_bytes())).as_bytes())
    }
}

fn invalid() -> io::Error {
    failure("dev_extraction_invalid")
}

fn private_root(path: &Path) -> io::Result<()> {
    let meta = owned_entry(path).map_err(|_| invalid())?;
    if !meta.is_dir() || meta.mode() & 0o777 != 0o700 {
        return Err(invalid());
    }
    Ok(())
}

fn owned_entry(path: &Path) -> io::Result<fs::Metadata> {
    let meta = fs::symlink_metadata(path)?;
    if meta.uid() != uid() || meta.mode() & 0o022 != 0 {
        return Err(invalid());
    }
    Ok(meta)
}

fn require_file(path: &Path, executable: bool) -> io::Result<()> {
    let meta = owned_entry(path)?;
    if !meta.is_file() || meta.nlink() != 1 || executable && meta.mode() & 0o100 == 0 {
        return Err(invalid());
    }
    Ok(())
}

fn check_tree(root: &Path, engine_links: bool) -> io::Result<()> {
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        if !owned_entry(&dir)?.is_dir() {
            return Err(invalid());
        }
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            let meta = fs::symlink_metadata(&path)?;
            if engine_links && meta.file_type().is_symlink() && meta.uid() == uid() {
                check_engine_link(root, &path)?;
            } else if meta.is_dir() {
                dirs.push(path);
            } else {
                require_file(&path, false)?;
            }
        }
    }
    Ok(())
}

/// npm ships relative executable aliases in the engine tree. Resolve each
/// component without following directory links or allowing it to leave that tree.
fn check_engine_link(root: &Path, link: &Path) -> io::Result<()> {
    let mut target = link.parent().ok_or_else(invalid)?.to_path_buf();
    let relative = fs::read_link(link)?;
    let mut parts = relative.components().peekable();
    while let Some(part) = parts.next() {
        match part {
            Component::CurDir => (),
            Component::ParentDir if target != root => {
                target.pop();
            }
            Component::Normal(name) => target.push(name),
            _ => return Err(invalid()),
        }
        if parts.peek().is_some() && !owned_entry(&target)?.is_dir() {
            return Err(invalid());
        }
    }
    require_file(&target, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::BTreeMap,
        os::unix::fs::{symlink, DirBuilderExt, PermissionsExt},
        sync::atomic::{AtomicUsize, Ordering},
    };

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = PathBuf::from("/tmp").join(format!(
                "cortex-extracted-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::DirBuilder::new().mode(0o700).create(&path).unwrap();
            for (_, leaf) in ROOTS {
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(path.join(leaf))
                    .unwrap();
            }
            Self(path)
        }

        fn env(&self, name: &str) -> Option<OsString> {
            if name == PREFIX {
                return Some(self.0.clone().into_os_string());
            }
            ROOTS
                .iter()
                .find(|(var, _)| *var == name)
                .map(|(_, leaf)| self.0.join(leaf).into_os_string())
        }

        fn bundle(&self, dev: &Channel) -> PathBuf {
            let usr = self.0.join("usr");
            let package = usr.join("lib").join(&dev.package);
            for (file, mode) in [
                (usr.join("bin").join(&dev.desktop_binary), 0o755),
                (usr.join("bin").join(&dev.service_binary), 0o755),
                (package.join("runtime/runtime-payload.json"), 0o644),
                (package.join("runtime/package-identity.json"), 0o644),
                (package.join("engines/bundle-manifest.json"), 0o644),
                (package.join("engines/node/bin/node"), 0o755),
                (package.join("voice/bundle-manifest.json"), 0o644),
                (package.join("voice/voice.cjs"), 0o644),
            ] {
                fs::create_dir_all(file.parent().unwrap()).unwrap();
                fs::write(&file, b"fixture").unwrap();
                fs::set_permissions(file, fs::Permissions::from_mode(mode)).unwrap();
            }
            package.join("runtime")
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn system_uid() -> u32 {
        // Guarded test runners can map root to an overflow UID. Keep production fixed to
        // UID 0, but exercise the same ancestor policy with the fixture filesystem's root.
        fs::metadata("/").unwrap().uid()
    }

    fn channels() -> BTreeMap<String, Channel> {
        serde_json::from_str(include_str!("../../workspace-protocol/channels.json")).unwrap()
    }

    #[test]
    fn opt_in_is_dev_only_and_release_never_reads_it() {
        let table = channels();
        let fixture = Fixture::new();
        let root = fixture.bundle(&table["dev"]);
        assert!(
            DevExtraction::resolve(&table["dev"], system_uid(), |_| None)
                .unwrap()
                .is_none()
        );
        assert!(
            DevExtraction::resolve(&table["release"], system_uid(), |_| {
                panic!("release must not read extraction settings")
            })
            .unwrap()
            .is_none()
        );

        let extraction =
            DevExtraction::resolve(&table["dev"], system_uid(), |var| fixture.env(var))
                .unwrap()
                .unwrap();
        assert!(extraction
            .validate_layout(&table["release"], &root)
            .is_err());
        assert!(extraction.validate_layout(&table["dev"], &root).is_ok());
        assert!(extraction
            .validate_layout(
                &table["dev"],
                Path::new("/usr/lib/cortex-workspace-dev/runtime")
            )
            .is_err());
        if channel::current().is_dev() {
            assert!(extraction.validate_runtime(&root).is_ok());
            assert_eq!(
                extraction.runtime_dir(),
                fixture.0.join("run/cortex-workspace-dev")
            );
        } else {
            assert!(extraction.validate_runtime(&root).is_err());
        }
    }

    #[test]
    fn qualification_requires_the_actual_binary_in_the_dev_prefix() {
        let table = channels();
        let dev = &table["dev"];
        let fixture = Fixture::new();
        fixture.bundle(dev);
        let extraction = DevExtraction(fixture.0.clone());
        let binary = fixture.0.join("usr/bin").join(&dev.desktop_binary);
        assert!(extraction
            .validate_qualification_binary(dev, &dev.desktop_binary, &binary)
            .is_ok());
        assert!(extraction
            .validate_qualification_binary(&table["release"], &dev.desktop_binary, &binary)
            .is_err());
        assert!(extraction
            .validate_qualification_binary(dev, &dev.service_binary, &binary)
            .is_err());
        assert!(extraction
            .validate_qualification_binary(
                dev,
                &dev.desktop_binary,
                Path::new("/usr/bin/cortex-workspace-dev")
            )
            .is_err());
    }

    #[test]
    fn qualification_opener_records_one_exact_argument_and_refuses_unsafe_recorders() {
        let fixture = Fixture::new();
        let capture = OpenerCapture(fixture.0.clone());
        let record = fixture.0.join("opener.sha256");
        let url = "https://qualification.invalid/cortex/view-pr?exact=1&arg=2";
        assert!(
            capture.record(url).is_err(),
            "must not create a missing recorder"
        );
        fs::write(&record, "").unwrap();
        fs::set_permissions(&record, fs::Permissions::from_mode(0o400)).unwrap();
        assert!(capture.record(url).is_err(), "read-only recorder must fail");
        assert_eq!(fs::read(&record).unwrap(), b"");
        fs::set_permissions(&record, fs::Permissions::from_mode(0o600)).unwrap();
        capture.record(url).unwrap();
        assert_eq!(
            fs::read(&record).unwrap(),
            b"685d873971c915642fedc5b6341eb7a4a73bbc5f5dcb2f7209530c0c1d48cb0f\n"
        );
        fs::remove_file(&record).unwrap();
        let target = fixture.0.join("target");
        fs::write(&target, "").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        symlink(&target, &record).unwrap();
        assert!(capture.record(url).is_err(), "do not follow a symlink");
        assert_eq!(fs::read(&target).unwrap(), b"");
    }

    #[test]
    fn refuses_unset_or_shared_roots_and_redirected_registry() {
        let table = channels();
        let dev = &table["dev"];
        let fixture = Fixture::new();
        for var in ROOTS.map(|(var, _)| var) {
            assert!(DevExtraction::resolve(dev, system_uid(), |name| {
                if var == name {
                    return None;
                }
                fixture.env(name)
            })
            .is_err());
        }
        assert!(DevExtraction::resolve(dev, system_uid(), |name| {
            if name == "XDG_RUNTIME_DIR" {
                return Some("/run/user/1000".into());
            }
            fixture.env(name)
        })
        .is_err());
        for malformed in ["", "relative", "/"] {
            assert!(DevExtraction::resolve(dev, system_uid(), |name| {
                if name == PREFIX {
                    return Some(malformed.into());
                }
                fixture.env(name)
            })
            .is_err());
        }

        let alias = fixture.0.join("config/prefix-alias");
        symlink(&fixture.0, &alias).unwrap();
        assert!(DevExtraction::resolve(dev, system_uid(), |name| {
            if name == PREFIX {
                return Some(alias.clone().into_os_string());
            }
            fixture.env(name)
        })
        .is_err());

        let state = fixture.0.join("state");
        fs::set_permissions(&state, fs::Permissions::from_mode(0o770)).unwrap();
        assert!(DevExtraction::resolve(dev, system_uid(), |name| fixture.env(name)).is_err());
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        symlink(fixture.0.join("config"), dev.state_dir(&state)).unwrap();
        assert!(DevExtraction::resolve(dev, system_uid(), |name| fixture.env(name)).is_err());
    }

    #[test]
    fn refuses_malformed_payloads_and_allows_only_contained_engine_links() {
        let table = channels();
        let dev = &table["dev"];
        let fixture = Fixture::new();
        let root = fixture.bundle(dev);
        let extraction = DevExtraction::resolve(dev, system_uid(), |name| fixture.env(name))
            .unwrap()
            .unwrap();
        let validate = || extraction.validate_layout(dev, &root);
        let bin = root.parent().unwrap().join("engines/node/bin");
        let link = bin.join("alias");
        symlink("../bin/node", &link).unwrap();
        validate().unwrap();
        fs::remove_file(&link).unwrap();

        for dest in [
            "../../../runtime/runtime-payload.json",
            "/usr/bin/sh",
            "missing",
        ] {
            symlink(dest, &link).unwrap();
            assert!(validate().is_err());
            fs::remove_file(&link).unwrap();
        }

        for relative in ["link", ".git"] {
            let link = root.join(relative);
            symlink("runtime-payload.json", &link).unwrap();
            assert!(validate().is_err());
            fs::remove_file(link).unwrap();
        }
        let marker = root.join("runtime-payload.json");
        let hardlink = fixture.0.join("other-name");
        fs::hard_link(&marker, &hardlink).unwrap();
        assert!(validate().is_err());
        fs::remove_file(hardlink).unwrap();
        let other_tree = root.parent().unwrap().join("voice");
        fs::set_permissions(&other_tree, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(validate().is_err());
        fs::set_permissions(other_tree, fs::Permissions::from_mode(0o755)).unwrap();

        fs::set_permissions(&marker, fs::Permissions::from_mode(0o664)).unwrap();
        assert!(validate().is_err());
        fs::remove_file(marker).unwrap();
        assert!(validate().is_err());
    }
}
