//! Legacy Git file selection. Only fixed ls-files runs, in a no-home, no-network
//! bubblewrap view. Unavailable Git retains the legacy whole-tree fallback.
use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::unix::{
        fs::{DirBuilderExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
pub(super) struct Listed {
    files: BTreeSet<Vec<u8>>,
    dirs: BTreeSet<Vec<u8>>,
}
impl Listed {
    pub(super) fn includes(&self, path: &[u8], directory: bool) -> bool {
        path == b".gitignore"
            || if directory {
                self.dirs.contains(path)
            } else {
                self.files.contains(path)
            }
    }
    pub(super) fn parse(bytes: &[u8]) -> Result<Self, ()> {
        if !bytes.is_empty() && bytes.last() != Some(&0) {
            return Err(());
        }
        let mut files = BTreeSet::new();
        let mut dirs = BTreeSet::new();
        // Like the legacy reader, any byte path is accepted; names that cannot
        // match a real entry (absolute, dot components) simply never select one.
        for raw in bytes.split(|b| *b == 0).filter(|b| !b.is_empty()) {
            files.insert(raw.to_vec());
            for (i, b) in raw.iter().enumerate() {
                if *b == b'/' {
                    dirs.insert(raw[..i].to_vec());
                }
            }
        }
        Ok(Self { files, dirs })
    }
}
fn directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)
}
fn small(path: &Path) -> io::Result<String> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("not_regular"));
    }
    let mut bytes = Vec::new();
    file.take(4097).read_to_end(&mut bytes)?;
    if bytes.len() > 4096 {
        return Err(io::Error::other("pointer_too_large"));
    }
    String::from_utf8(bytes).map_err(io::Error::other)
}
fn metadata(root: &Path) -> io::Result<Vec<PathBuf>> {
    let dot = root.join(".git");
    let st = dot.symlink_metadata()?;
    let gitdir = if st.is_dir() {
        dot
    } else if st.is_file() {
        let pointer = small(&dot)?;
        root.join(
            pointer
                .trim()
                .strip_prefix("gitdir:")
                .ok_or_else(|| io::Error::other("pointer"))?
                .trim(),
        )
    } else {
        return Err(io::Error::other("metadata_type"));
    };
    directory(&gitdir)?;
    let mut dirs = vec![gitdir];
    let common = dirs[0].join("commondir");
    match common.symlink_metadata() {
        Ok(st) if st.is_file() => {
            let target = small(&common)?;
            let path = dirs[0].join(target.trim());
            directory(&path)?;
            dirs.push(path);
        }
        Ok(_) => return Err(io::Error::other("common_type")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    for dir in &dirs {
        for name in ["HEAD", "index", "config", "packed-refs", "info/exclude"] {
            match dir.join(name).symlink_metadata() {
                Ok(st) if st.is_file() => {}
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                _ => return Err(io::Error::other("metadata_type")),
            }
        }
    }
    dirs.into_iter().map(|p| p.canonicalize()).collect()
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> io::Result<Self> {
        let id =
            crate::native_effects::identity().map_err(|_| io::Error::other("scratch_identity"))?;
        let path = Path::new("/tmp").join(format!("cx-git-source-{id}"));
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn mask(
    command: &mut Command,
    root: &Path,
    remaining: &mut usize,
    deadline: Instant,
) -> io::Result<()> {
    for entry in fs::read_dir(root)? {
        if *remaining == 0 || Instant::now() >= deadline {
            return Err(io::Error::new(io::ErrorKind::FileTooLarge, "mask_limit"));
        }
        *remaining -= 1;
        let entry = entry?;
        let kind = entry.file_type()?;
        if super::legacy_source::EXCLUDED_TOP
            .iter()
            .any(|n| entry.file_name() == *n)
        {
            if kind.is_dir() {
                command
                    .arg("--tmpfs")
                    .arg(entry.path())
                    .arg("--remount-ro")
                    .arg(entry.path());
            } else if !kind.is_symlink() {
                command.arg("--ro-bind").arg("/dev/null").arg(entry.path());
            }
        } else if kind.is_dir() {
            mask(command, &entry.path(), remaining, deadline)?;
        }
    }
    Ok(())
}
// Err is an oversized or unterminated selection; the caller reports the source
// unobservable rather than assigning a different identity.
pub(super) fn listed(root: &Path) -> Result<Option<Listed>, ()> {
    if directory(root).is_err() {
        return Ok(None);
    }
    let Ok(dirs) = metadata(root) else {
        return Ok(None);
    };
    let run = || -> io::Result<Option<Vec<u8>>> {
        directory(root)?;
        let scratch = Scratch::new()?;
        let output = scratch.0.join("stdout");
        let stdout = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)?;
        let mut command = Command::new("/usr/bin/bwrap");
        command.env_clear().env("PATH", "/usr/bin:/bin").args([
            "--die-with-parent",
            "--unshare-pid",
            "--unshare-ipc",
            "--unshare-net",
            "--clearenv",
            "--setenv",
            "PATH",
            "/usr/local/bin:/usr/bin:/bin",
            "--setenv",
            "LANG",
            "C.UTF-8",
        ]);
        for name in ["HOME", "TMPDIR", "TMP", "TEMP"] {
            command.arg("--setenv").arg(name).arg(&scratch.0);
        }
        for name in ["/usr", "/bin", "/sbin", "/lib", "/lib64"] {
            let path = Path::new(name);
            if path.symlink_metadata().is_ok_and(|m| m.is_symlink()) {
                command.arg("--symlink").arg(fs::read_link(path)?).arg(path);
            } else if path.is_dir() {
                command.arg("--ro-bind").arg(path).arg(path);
            }
        }
        for name in ["/etc/ld.so.cache", "/etc/ld.so.conf", "/etc/localtime"] {
            if Path::new(name).is_file() {
                command.arg("--ro-bind").arg(name).arg(name);
            }
        }
        fs::create_dir(scratch.0.join("var-tmp"))?;
        command.args(["--proc", "/proc", "--dev", "/dev", "--bind", "/tmp", "/tmp"]);
        command
            .arg("--bind")
            .arg(scratch.0.join("var-tmp"))
            .arg("/var/tmp");
        command.arg("--bind").arg(&scratch.0).arg(&scratch.0);
        command.arg("--ro-bind").arg(root).arg(root);
        mask(
            &mut command,
            root,
            &mut 2_000_000,
            Instant::now() + Duration::from_secs(20),
        )?;
        for dir in &dirs {
            command.arg("--ro-bind").arg(dir).arg(dir);
        }
        command
            .arg("--setenv")
            .arg("GIT_DIR")
            .arg(&dirs[0])
            .arg("--setenv")
            .arg("GIT_WORK_TREE")
            .arg(root)
            .args([
                "--setenv",
                "GIT_CONFIG_NOSYSTEM",
                "1",
                "--setenv",
                "GIT_CONFIG_GLOBAL",
                "/dev/null",
                "--setenv",
                "GIT_TERMINAL_PROMPT",
                "0",
            ])
            .arg("--chdir")
            .arg(root)
            .args([
                "--",
                "/usr/bin/git",
                "--no-optional-locks",
                "-c",
                "core.fsmonitor=false",
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
            ])
            .arg(format!("lfs.storage={}/lfs", scratch.0.display()))
            .args([
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
                "--",
                ".",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(stdout);
        unsafe {
            command.pre_exec(|| {
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn()?;
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut oversized = false;
        let status = loop {
            oversized |= fs::metadata(&output).is_ok_and(|m| m.len() > 64 * 1024 * 1024);
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline && !oversized => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => {
                    unsafe {
                        libc::kill(-(child.id() as i32), libc::SIGKILL);
                    }
                    let _ = child.wait();
                    break None;
                }
            }
        };
        if oversized {
            return Err(io::Error::new(io::ErrorKind::FileTooLarge, "list_limit"));
        }
        if status.is_some_and(|s| s.success()) {
            let mut bytes = Vec::new();
            File::open(output)?
                .take(64 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            Ok(Some(bytes))
        } else {
            Ok(None)
        }
    };
    match run() {
        Ok(Some(bytes)) if bytes.len() <= 64 * 1024 * 1024 => Listed::parse(&bytes).map(Some),
        Ok(Some(_)) => Err(()),
        Err(e) if e.kind() == io::ErrorKind::FileTooLarge => Err(()),
        _ => Ok(None),
    }
}
