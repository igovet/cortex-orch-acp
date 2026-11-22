//! Scratch-only command view for managed Codex. No live checkout inode is mounted.
use super::{policy, unavailable, Sandbox};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File},
    io::{self, Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    },
    path::Path,
};

const MAX_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ENTRIES: usize = 65_536;
const MAX_DEPTH: usize = 64;

impl Sandbox {
    pub(super) fn scoped(
        binary: std::path::PathBuf,
        root: &Path,
        protected: &[std::path::PathBuf],
    ) -> io::Result<Self> {
        // Do not let a checkout containing the staging directory make staging
        // itself a project write (or recursively copy our own staging files).
        let temp = Path::new("/tmp").canonicalize()?;
        if temp.starts_with(root) || crate::native_engine_policy::project_private(root) {
            return Err(unavailable("scratch_overlaps_checkout"));
        }
        let scratch = temp.join(format!(
            "cortex-scoped-command-{}",
            crate::native_effects::identity().map_err(|_| unavailable("scratch_identity"))?
        ));
        fs::DirBuilder::new().mode(0o700).create(&scratch)?;
        let mut s = Self {
            binary,
            args: Vec::new(),
            scratch,
            environment: BTreeMap::new(),
            pins: Vec::new(),
            guard: None,
            overlay: false,
            ssh_environment: BTreeMap::new(),
            scoped_root: Some(root.to_path_buf()),
        };
        // Probe the actual view before doing bounded input staging. This does
        // not run the proposed command or any executable from the checkout.
        s.args.extend(
            [
                "--die-with-parent",
                "--new-session",
                "--unshare-user",
                "--disable-userns",
                "--unshare-net",
                "--unshare-ipc",
                "--unshare-pid",
                "--unshare-uts",
                "--cap-drop",
                "ALL",
                "--clearenv",
                "--setenv",
                "PATH",
                "/usr/bin:/bin",
                "--setenv",
                "LANG",
                "C.UTF-8",
                "--setenv",
                "HOME",
                "/tmp",
                "--setenv",
                "TMPDIR",
                "/tmp",
                "--setenv",
                "CARGO_TARGET_DIR",
                "/tmp/target",
                "--proc",
                "/proc",
                "--dev",
                "/dev",
                "--size",
                "1073741824",
                "--tmpfs",
                "/tmp",
                "--remount-ro",
                "/proc",
            ]
            .map(OsString::from),
        );
        // System executables/libraries only; no host HOME, /run, /tmp,
        // service connections, writable device mounts or environment grants.
        for p in [
            "/usr/bin",
            "/usr/lib",
            "/usr/lib64",
            "/bin",
            "/lib",
            "/lib64",
        ] {
            let path = Path::new(p);
            if path.is_symlink() {
                let link = fs::read_link(path)?;
                if !path.canonicalize()?.starts_with("/usr") {
                    return Err(unavailable("invalid_system_tool_path"));
                }
                s.args
                    .extend(["--symlink".into(), link.into_os_string(), p.into()]);
            } else if path.is_dir() {
                if path.starts_with(root) || root.starts_with(path) {
                    return Err(unavailable("system_mount_overlaps_checkout"));
                }
                s.mount("--ro-bind", path, path)?;
            }
        }
        let filter = s.scratch.join("seccomp");
        let mut file = File::create(&filter)?;
        let (filter_code, n) = crate::native_sandbox::syscall_filter(false);
        for ins in &filter_code[..n] {
            file.write_all(&ins.code.to_ne_bytes())?;
            file.write_all(&[ins.jt, ins.jf])?;
            file.write_all(&ins.k.to_ne_bytes())?;
        }
        drop(file);
        let staged = s.scratch.join("input");
        fs::create_dir(&staged)?;
        s.mount("--ro-bind", &staged, Path::new("/workspace"))?;
        s.args.extend([
            "--remount-ro".into(),
            "/".into(),
            "--chdir".into(),
            "/workspace".into(),
        ]);
        s.probe()?;
        let root_fd = policy::pin(root)?;
        let (mut entries, mut bytes) = (MAX_ENTRIES, MAX_BYTES);
        copy_dir(
            &root_fd,
            root,
            &staged,
            protected,
            0,
            &mut entries,
            &mut bytes,
        )?;
        Ok(s)
    }
}

/// Read only through pinned descriptors; stage new ordinary files, not hardlinks
/// or live device/socket/pipe inodes. New entries cannot replace a pinned input.
fn copy_dir(
    folder: &File,
    from: &Path,
    into: &Path,
    protected: &[std::path::PathBuf],
    depth: usize,
    entries: &mut usize,
    bytes: &mut u64,
) -> io::Result<()> {
    let fd_path = |f: &File| format!("/proc/self/fd/{}", f.as_raw_fd());
    if depth >= MAX_DEPTH {
        return Err(unavailable("snapshot_limit"));
    }
    for entry in fs::read_dir(fd_path(folder))? {
        *entries = entries
            .checked_sub(1)
            .ok_or_else(|| unavailable("snapshot_limit"))?;
        let name = entry?.file_name();
        let relative = Path::new(&name);
        let source = from.join(relative);
        if name == ".git"
            || crate::native_engine_policy::project_private(relative)
            || protected.iter().any(|p| source.starts_with(p))
        {
            continue;
        }
        let Some(name) = name.to_str() else { continue };
        let fd = match crate::native_project::openat(folder, name, libc::O_PATH) {
            Ok(f) => f,
            Err(cortex_core::CoreError::Refused(
                "symlink_refused" | "project_mount_alias_refused",
            )) => continue,
            Err(cortex_core::CoreError::Io(e)) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(_) => return Err(unavailable("snapshot_input_unavailable")),
        };
        let metadata = fd.metadata()?;
        let dest = into.join(relative);
        if metadata.is_dir() {
            fs::create_dir(&dest)?;
            copy_dir(&fd, &source, &dest, protected, depth + 1, entries, bytes)?;
        } else if metadata.is_file() && metadata.nlink() == 1 {
            if metadata.len() > *bytes {
                return Err(unavailable("snapshot_limit"));
            }
            let mut input = File::open(fd_path(&fd))?.take(*bytes + 1);
            let mut output = File::create(&dest)?;
            let size = io::copy(&mut input, &mut output)?;
            *bytes = bytes
                .checked_sub(size)
                .ok_or_else(|| unavailable("snapshot_limit"))?;
            // Preserve ordinary executable bits; never copy set-ID bits or xattrs.
            output.set_permissions(fs::Permissions::from_mode(
                0o644 | (metadata.mode() & 0o111),
            ))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{os::unix::fs::symlink, path::PathBuf, process::Command};

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "codex-command-fixture-{}",
                crate::native_effects::identity().unwrap()
            ));
            fs::DirBuilder::new().mode(0o700).create(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn snapshot_has_no_live_inodes_or_private_entries() {
        let f = Fixture::new();
        let root = f.0.join("root");
        let stage = f.0.join("stage");
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir(&stage).unwrap();
        let input = root.join("src/normal");
        fs::write(&input, "ordinary").unwrap();
        fs::write(root.join(".git"), "gitdir: elsewhere").unwrap();
        fs::create_dir_all(root.join("src/.cortex-dev")).unwrap();
        fs::write(root.join("src/.cortex-dev/cortex.sqlite3"), "fixture").unwrap();
        fs::write(root.join(".cortex-dev-owner.json"), "fixture").unwrap();
        symlink(&input, root.join("alias")).unwrap();
        fs::hard_link(
            root.join("src/.cortex-dev/cortex.sqlite3"),
            root.join("hard"),
        )
        .unwrap();
        let fifo =
            std::ffi::CString::new(root.join("src/pipe").as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        copy_dir(
            &policy::pin(&root).unwrap(),
            &root,
            &stage,
            &[],
            0,
            &mut 20,
            &mut 200,
        )
        .unwrap();
        assert_eq!(
            fs::read_to_string(stage.join("src/normal")).unwrap(),
            "ordinary"
        );
        assert_ne!(
            fs::metadata(&input).unwrap().ino(),
            fs::metadata(stage.join("src/normal")).unwrap().ino()
        );
        for name in [
            ".git",
            "alias",
            "hard",
            "src/pipe",
            "src/.cortex-dev",
            ".cortex-dev-owner.json",
        ] {
            assert!(!stage.join(name).exists(), "{name}");
        }
        let refusal = copy_dir(
            &policy::pin(&root).unwrap(),
            &root,
            &stage,
            &[],
            0,
            &mut 0,
            &mut 200,
        )
        .unwrap_err();
        assert!(refusal.to_string().ends_with("snapshot_limit"));
    }

    // A required, non-skipping positive host observation when selected explicitly.
    // Nested CI jails need not offer the namespaces used by the real service.
    #[test]
    #[ignore = "requires a host that permits bubblewrap user, PID and network namespaces"]
    fn scoped_command_host_observation() {
        use cortex_core::{ProjectKey, StoreChannel};
        let f = Fixture::new();
        let root = f.0.join("project");
        let linked = f.0.join("linked");
        let other = f.0.join("other");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&other).unwrap();
        let git = |args: &[&str]| {
            assert!(
                Command::new("/usr/bin/git")
                    .args(args)
                    .current_dir(&root)
                    .env_clear()
                    .env("PATH", "/usr/bin:/bin")
                    .env("GIT_CONFIG_NOSYSTEM", "1")
                    .env("GIT_CONFIG_GLOBAL", "/dev/null")
                    .status()
                    .unwrap()
                    .success(),
                "{args:?}"
            );
        };
        git(&["init", "-q"]);
        fs::write(root.join("file"), "fixture").unwrap();
        git(&["add", "file"]);
        git(&[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "fixture",
        ]);
        git(&["worktree", "add", "-qb", "linked", linked.to_str().unwrap()]);
        fs::write(linked.join("unassigned"), "untouched").unwrap();
        let mut protected = Vec::new();
        for channel in [StoreChannel::Release, StoreChannel::Dev] {
            let key = ProjectKey::new(&linked, channel).unwrap();
            assert_eq!(key.root(), root);
            fs::create_dir_all(key.directory()).unwrap();
            protected.push(key.directory().join("cortex.sqlite3"));
            let path = ProjectKey::new(&other, channel)
                .unwrap()
                .directory()
                .to_path_buf();
            fs::create_dir_all(&path).unwrap();
            protected.push(path.join("cortex.sqlite3"));
        }
        for p in &protected {
            fs::write(p, "private fixture").unwrap();
        }
        for name in [".cortex-dev-owner.json", ".cortex-workspace-dev.json"] {
            let p = root.join(name);
            fs::write(&p, "private fixture").unwrap();
            protected.push(p);
        }
        symlink(&protected[0], linked.join("store-alias")).unwrap();
        let sandbox = Sandbox::prepare_scoped(&linked, &[])
            .expect("positive namespace observation unavailable");
        let mut args = vec![
            "/bin/sh".into(),
            "-ec".into(),
            String::from(
                r#"
            [ "$PWD" = /workspace ] && [ "$(cat file)" = fixture ]
            printf ordinary > "$TMPDIR/result"
            [ "$(cat "$TMPDIR/result")" = ordinary ]
            [ ! -e store-alias ] && [ ! -e .git ] && [ ! -e "$1" ]
            if (printf changed >/host-root) 2>/dev/null; then exit 25; fi
            if (printf changed > unassigned) 2>/dev/null; then exit 21; fi
            if (printf changed > file) 2>/dev/null; then exit 22; fi
            if chmod +x unassigned 2>/dev/null; then exit 23; fi
            for p in "$@"; do
                if (printf changed > "$p") 2>/dev/null; then exit 24; fi
            done
        "#,
            ),
            "sh".into(),
        ];
        args.extend(protected.iter().map(|p| p.to_str().unwrap().to_owned()));
        let result = sandbox.command(&args).unwrap().output().unwrap();
        assert!(
            result.status.success(),
            "exit {:?}: {}",
            result.status.code(),
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(fs::read_to_string(linked.join("file")).unwrap(), "fixture");
        assert_eq!(
            fs::read_to_string(linked.join("unassigned")).unwrap(),
            "untouched"
        );
        for p in protected {
            assert_eq!(fs::read_to_string(p).unwrap(), "private fixture");
        }
    }
}
