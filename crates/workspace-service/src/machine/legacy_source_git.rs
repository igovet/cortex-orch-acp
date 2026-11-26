//! Legacy Git file selection. Only fixed ls-files runs, with a private HOME under
//! a Landlock ruleset (scratch-only writes, excluded entries hidden). Unavailable Git retains the legacy whole-tree fallback.
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
/// Hides every private store or Git directory name of the tree from the selection's
/// view (`EXCLUDED_TOP` at any depth, without looking inside one). A large tree is read
/// by several threads and its masks are added in the order the ordered walk adds them
/// (depth first, each directory in its read order), so the view is the same; when that
/// walk would stop (an unreadable entry, the entry bound or the deadline) it is the
/// ordered walk that runs and decides.
/// The excluded entries the listing must not read, hidden by the Landlock ruleset.
fn mask(root: &Path, remaining: &mut usize, deadline: Instant) -> io::Result<Vec<PathBuf>> {
    let Some(mut found) = mask_parallel(root, *remaining, deadline, MASK_WALKERS) else {
        let mut hidden = Vec::new();
        mask_ordered(&mut hidden, root, remaining, deadline)?;
        return Ok(hidden);
    };
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found.into_iter().map(|(_, path, _)| path).collect())
}
/// Threads of one parallel mask walk.
const MASK_WALKERS: usize = 8;
/// One mask: where the ordered walk meets it (the read index of each entry on the way),
/// its path and whether it is a directory. None when the ordered walk would stop.
type Mask = (Vec<u32>, PathBuf, bool);
fn mask_parallel(
    root: &Path,
    limit: usize,
    deadline: Instant,
    threads: usize,
) -> Option<Vec<Mask>> {
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Condvar, Mutex,
    };
    type Job = (PathBuf, Vec<u32>);
    // Directories waiting to be read, and how many are being read right now.
    let queue: Mutex<(Vec<Job>, usize)> = Mutex::new((vec![(root.to_owned(), Vec::new())], 0));
    let ready = Condvar::new();
    let seen = AtomicUsize::new(0);
    let failed = AtomicBool::new(false);
    let found = Mutex::new(Vec::new());
    // Set under the queue lock, so a walker about to wait always sees it or is woken.
    let fail = || {
        let _queue = queue.lock();
        failed.store(true, Ordering::SeqCst);
        ready.notify_all();
    };
    let next = || -> Option<Job> {
        let mut q = queue.lock().ok()?;
        loop {
            if failed.load(Ordering::SeqCst) {
                return None;
            }
            if let Some(job) = q.0.pop() {
                q.1 += 1;
                return Some(job);
            }
            if q.1 == 0 {
                return None;
            }
            q = ready.wait(q).ok()?;
        }
    };
    let done = |children: Vec<Job>| match queue.lock() {
        Ok(mut q) => {
            q.0.extend(children);
            q.1 -= 1;
            ready.notify_all();
        }
        Err(_) => {
            failed.store(true, Ordering::SeqCst);
            ready.notify_all();
        }
    };
    std::thread::scope(|scope| {
        for _ in 0..threads.max(1) {
            scope.spawn(|| {
                let mut masks = Vec::new();
                while let Some((dir, at)) = next() {
                    let mut children = Vec::new();
                    let read = (|| -> Option<()> {
                        for (index, entry) in fs::read_dir(&dir).ok()?.enumerate() {
                            // The ordered walk's bound: it stops before the entry past it.
                            if seen.fetch_add(1, Ordering::SeqCst) >= limit
                                || Instant::now() >= deadline
                            {
                                return None;
                            }
                            let entry = entry.ok()?;
                            let kind = entry.file_type().ok()?;
                            let mut place = at.clone();
                            place.push(u32::try_from(index).ok()?);
                            if super::legacy_source::EXCLUDED_TOP
                                .iter()
                                .any(|n| entry.file_name() == *n)
                            {
                                if kind.is_dir() {
                                    masks.push((place, entry.path(), true));
                                } else if !kind.is_symlink() {
                                    masks.push((place, entry.path(), false));
                                }
                            } else if kind.is_dir() {
                                children.push((entry.path(), place));
                            }
                        }
                        Some(())
                    })();
                    if read.is_none() {
                        fail();
                        break;
                    }
                    done(children);
                }
                match found.lock() {
                    Ok(mut found) => found.extend(masks),
                    Err(_) => fail(),
                }
            });
        }
    });
    if failed.load(Ordering::SeqCst) {
        return None;
    }
    found.into_inner().ok()
}
fn mask_ordered(
    hidden: &mut Vec<PathBuf>,
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
            if !kind.is_symlink() {
                hidden.push(entry.path());
            }
        } else if kind.is_dir() {
            mask_ordered(hidden, &entry.path(), remaining, deadline)?;
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
        let hidden = mask(root, &mut 2_000_000, Instant::now() + Duration::from_secs(20))?;
        // The real host under Landlock: writes only to the private scratch and
        // devices; the excluded entries and the live Cortex stores are unreadable.
        let mut writes = crate::native_sandbox::devices();
        writes.push(scratch.0.clone());
        let mut hidden = hidden;
        hidden.extend(crate::native_sandbox::live_stores(root));
        let rules = std::sync::Arc::new(crate::native_sandbox::Sandbox::host(
            &crate::native_sandbox::HostPolicy { writes, hidden, ..Default::default() },
        )?);
        let mut command = Command::new("/usr/bin/git");
        command
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("LANG", "C.UTF-8")
            .env("GIT_DIR", &dirs[0])
            .env("GIT_WORK_TREE", root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_TERMINAL_PROMPT", "0")
            .current_dir(root)
            .args([
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
        for name in ["HOME", "TMPDIR", "TMP", "TEMP"] {
            command.env(name, &scratch.0);
        }
        unsafe {
            command.pre_exec(move || {
                if libc::setpgid(0, 0) != 0 {
                    return Err(io::Error::last_os_error());
                }
                rules.enter()
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
