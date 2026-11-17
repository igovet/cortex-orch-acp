//! Exact legacy manifest bytes, including sandboxed Git file selection.
//! This is not the native Core project digest. Non-UTF-8 names are carried as raw
//! bytes and ordered like Python's surrogateescape strings.
use cortex_core::CoreResult;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, OpenOptionsExt},
        },
    },
    path::Path,
    time::{Duration, Instant},
};
pub(super) const EXCLUDED_TOP: &[&str] = &[
    ".git",
    ".cortex",
    ".cortex-dev",
    ".cortex-dev-owner.json",
    ".cortex-workspace-dev.json",
];
const EXCLUDED_DIR: &[&str] = &[
    "node_modules",
    ".venv",
    "venv",
    "__pycache__",
    "Pods",
    ".gradle",
    ".expo",
    ".next",
    ".turbo",
    ".pytest_cache",
    ".mypy_cache",
];
#[derive(Debug)]
enum ScanError {
    Unavailable,
}
impl From<io::Error> for ScanError {
    fn from(_: io::Error) -> Self {
        Self::Unavailable
    }
}
type ScanResult<T> = Result<T, ScanError>;
struct Budget {
    files: usize,
    bytes: u64,
    max_files: usize,
    max_bytes: u64,
    deadline: Instant,
    listed: Option<super::legacy_source_git::Listed>,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            files: 0,
            bytes: 0,
            max_files: 50_000,
            max_bytes: 512 * 1024 * 1024,
            deadline: Instant::now() + Duration::from_secs(20),
            listed: None,
        }
    }
}
impl Budget {
    fn time(&self) -> ScanResult<()> {
        if Instant::now() > self.deadline {
            Err(ScanError::Unavailable)
        } else {
            Ok(())
        }
    }
}
fn same(a: &Metadata, b: &Metadata) -> bool {
    (
        a.dev(),
        a.ino(),
        a.mode(),
        a.len(),
        a.mtime(),
        a.mtime_nsec(),
        a.ctime(),
        a.ctime_nsec(),
    ) == (
        b.dev(),
        b.ino(),
        b.mode(),
        b.len(),
        b.mtime(),
        b.mtime_nsec(),
        b.ctime(),
        b.ctime_nsec(),
    )
}
fn open_at(
    parent: &File,
    name: &std::ffi::OsStr,
    directory: bool,
    expected: &Metadata,
) -> ScanResult<File> {
    let name = std::ffi::CString::new(name.as_bytes()).map_err(|_| ScanError::Unavailable)?;
    let flags = libc::O_RDONLY
        | libc::O_CLOEXEC
        | libc::O_NOFOLLOW
        | libc::O_NONBLOCK
        | if directory { libc::O_DIRECTORY } else { 0 };
    let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags) };
    if fd < 0 {
        return Err(ScanError::Unavailable);
    }
    let file = unsafe { File::from_raw_fd(fd) };
    let actual = file.metadata()?;
    if actual.dev() != expected.dev()
        || actual.ino() != expected.ino()
        || if directory {
            !actual.is_dir()
        } else {
            !actual.is_file()
        }
    {
        return Err(ScanError::Unavailable);
    }
    Ok(file)
}
fn metadata_hash(size: u64, inode: u64, mtime_ns: i128, ctime_ns: i128) -> String {
    format!(
        "stat:{:x}",
        Sha256::digest(format!("{size}:{inode}:{mtime_ns}:{ctime_ns}").as_bytes())
    )
}
fn hash_file(
    file: &mut File,
    budget: &mut Budget,
    path: &str,
    hook: &mut impl FnMut(&str, &str),
) -> ScanResult<String> {
    let before = file.metadata()?;
    if before.len() > 1024 * 1024 || budget.bytes.saturating_add(before.len()) > budget.max_bytes {
        return Ok(metadata_hash(
            before.len(),
            before.ino(),
            before.mtime() as i128 * 1_000_000_000 + before.mtime_nsec() as i128,
            before.ctime() as i128 * 1_000_000_000 + before.ctime_nsec() as i128,
        ));
    }
    let mut hash = Sha256::new();
    let mut size = 0;
    let mut buf = vec![0; 1024 * 1024];
    loop {
        budget.time()?;
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        size += read as u64;
        budget.bytes += read as u64;
        if size > 64 * 1024 * 1024 || budget.bytes > budget.max_bytes {
            return Err(ScanError::Unavailable);
        }
        hash.update(&buf[..read]);
        hook("read", path);
    }
    if !same(&before, &file.metadata()?) {
        return Err(ScanError::Unavailable);
    }
    Ok(format!("{:x}", hash.finalize()))
}
fn record(kind: &[u8], path: &[u8], size: &[u8], tail: &[u8]) -> Vec<u8> {
    let mut out = kind.to_vec();
    for part in [path, size, tail] {
        out.push(b'\t');
        out.extend_from_slice(part);
    }
    out
}
/// Python orders surrogateescape strings by code point: valid UTF-8 as itself and
/// every undecodable byte as U+DC00 plus the byte.
fn python_order(bytes: &[u8]) -> Vec<u32> {
    let mut out = Vec::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        out.extend(chunk.valid().chars().map(|c| c as u32));
        out.extend(chunk.invalid().iter().map(|b| 0xDC00 + *b as u32));
    }
    out
}
fn walk(
    folder: &File,
    prefix: &[u8],
    budget: &mut Budget,
    records: &mut Vec<Vec<u8>>,
    hook: &mut impl FnMut(&str, &str),
) -> ScanResult<()> {
    // Python recurses once per level and would exhaust its interpreter stack in a
    // tree this deep. An explicit unobservable result is the bounded native outcome.
    if prefix.iter().filter(|b| **b == b'/').count() >= 128 {
        return Err(ScanError::Unavailable);
    }
    let before = folder.metadata()?;
    // /proc resolves only our owned descriptor. Every child open is relative
    // to it, no-follow, with device/inode and post-scan state checks.
    for entry in fs::read_dir(format!("/proc/self/fd/{}", folder.as_raw_fd()))? {
        budget.time()?;
        let entry = entry?;
        let name = entry.file_name();
        let mut path = prefix.to_vec();
        if !prefix.is_empty() {
            path.push(b'/');
        }
        path.extend_from_slice(name.as_bytes());
        // Excluded names are ASCII; a non-UTF-8 name can never equal one.
        let name_text = name.to_str().unwrap_or("");
        if prefix.is_empty() && EXCLUDED_TOP.contains(&name_text) {
            continue;
        }
        let st = fs::symlink_metadata(entry.path())?;
        if st.is_dir() && EXCLUDED_DIR.contains(&name_text) {
            continue;
        }
        if budget
            .listed
            .as_ref()
            .is_some_and(|list| !list.includes(&path, st.is_dir()))
        {
            continue;
        }
        budget.files += 1;
        if budget.listed.is_none() && budget.files > budget.max_files {
            return Err(ScanError::Unavailable);
        }
        let shown = String::from_utf8_lossy(&path).into_owned();
        hook("entry", &shown);
        if st.is_file() {
            let mut file = open_at(folder, &name, false, &st)?;
            if path == b".claude/scheduled_tasks.lock" {
                continue;
            }
            let digest = hash_file(&mut file, budget, &shown, hook)?;
            records.push(record(
                b"f",
                &path,
                st.len().to_string().as_bytes(),
                digest.as_bytes(),
            ));
        } else if st.is_symlink() {
            let target = fs::read_link(entry.path())?;
            records.push(record(b"l", &path, b"-", target.as_os_str().as_bytes()));
        } else if st.is_dir() {
            records.push(record(b"d", &path, b"-", b"-"));
            let child = open_at(folder, &name, true, &st)?;
            walk(&child, &path, budget, records, hook)?;
        } else {
            let kind = match st.mode() & libc::S_IFMT {
                libc::S_IFIFO => "p",
                libc::S_IFSOCK => "s",
                libc::S_IFBLK | libc::S_IFCHR => "dev",
                _ => "o",
            };
            records.push(record(kind.as_bytes(), &path, b"-", b"-"));
        }
    }
    hook("scanned", &String::from_utf8_lossy(prefix));
    if !same(&before, &folder.metadata()?) {
        return Err(ScanError::Unavailable);
    }
    Ok(())
}
fn scan(
    root: &Path,
    budget: &mut Budget,
    hook: &mut impl FnMut(&str, &str),
) -> ScanResult<(String, Vec<Vec<u8>>)> {
    // Match the legacy fallback if Git, its metadata or the sandbox is unavailable.
    // Never replace it with an unsandboxed command or a partial successful list.
    let selecting = Instant::now();
    // An oversized selection or an exhausted mask walk has no bounded successful
    // list: the source is reported unobservable, never partially identified.
    budget.listed = super::legacy_source_git::listed(root).map_err(|_| ScanError::Unavailable)?;
    // Python starts the tree budget after Git selection, including its timeout.
    // Preserve a caller's stricter test budget rather than resetting it blindly.
    budget.deadline += selecting.elapsed();
    if budget.listed.is_some() {
        budget.deadline = Instant::now() + Duration::from_secs(300);
    }
    scan_selected(root, budget, hook)
}
fn scan_selected(
    root: &Path,
    budget: &mut Budget,
    hook: &mut impl FnMut(&str, &str),
) -> ScanResult<(String, Vec<Vec<u8>>)> {
    let folder = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(root)?;
    let mut records = vec![];
    walk(&folder, b"", budget, &mut records, hook)?;
    records.sort_by_cached_key(|r| python_order(r));
    let mut digest = Sha256::new();
    for record in &records {
        digest.update(record);
        digest.update(b"\n");
    }
    Ok((format!("{:x}", digest.finalize()), records))
}
/// None is a complete unavailable observation, never a partial digest.
/// Unobservable sources (very deep trees, oversized Git selections, races) are
/// explicit None, never drift or an absent source.
pub(super) fn capture(root: &Path) -> CoreResult<Option<String>> {
    match scan(root, &mut Budget::default(), &mut |_, _| {}) {
        Ok((digest, _)) => Ok(Some(digest)),
        Err(ScanError::Unavailable) => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::symlink,
        sync::atomic::{AtomicU64, Ordering},
    };
    struct Root(std::path::PathBuf);
    impl Root {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = Path::new("/tmp").join(format!(
                "cx-source-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn offline_oracle_manifest_bytes_match() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/legacy-source.json")).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let root = Root::new();
            for entry in case["entries"].as_array().unwrap() {
                let path = root.0.join(entry["path"].as_str().unwrap());
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                match entry["kind"].as_str().unwrap() {
                    "d" => fs::create_dir_all(path).unwrap(),
                    "f" => fs::write(path, entry["text"].as_str().unwrap()).unwrap(),
                    "l" => symlink(entry["target"].as_str().unwrap(), path).unwrap(),
                    "p" => {
                        let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                    }
                    _ => panic!("bad fixture"),
                }
            }
            let (digest, manifest) = scan(&root.0, &mut Budget::default(), &mut |_, _| {}).unwrap();
            assert_eq!(
                serde_json::json!(text(&manifest)),
                case["expected"]["manifest"],
                "{}",
                case["name"]
            );
            assert_eq!(digest, case["expected"]["dirty_digest"], "{}", case["name"]);
        }
        for case in fixture["metadata"].as_array().unwrap() {
            assert_eq!(
                metadata_hash(
                    case["size"].as_u64().unwrap(),
                    case["inode"].as_u64().unwrap(),
                    case["mtime"].as_str().unwrap().parse().unwrap(),
                    case["ctime"].as_str().unwrap().parse().unwrap()
                ),
                case["expected"].as_str().unwrap()
            );
        }
    }
    fn text(records: &[Vec<u8>]) -> Vec<String> {
        records
            .iter()
            .map(|r| String::from_utf8(r.clone()).unwrap())
            .collect()
    }
    fn populate(root: &Path, entries: &serde_json::Value) {
        for entry in entries.as_array().unwrap() {
            let path = root.join(entry["path"].as_str().unwrap());
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            match entry["kind"].as_str().unwrap() {
                "d" => fs::create_dir_all(path).unwrap(),
                "f" => fs::write(path, entry["text"].as_str().unwrap()).unwrap(),
                "l" => symlink(entry["target"].as_str().unwrap(), path).unwrap(),
                "p" => {
                    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
                    assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                }
                _ => panic!("bad fixture"),
            }
        }
    }
    fn git(root: &Path, args: &[&str]) {
        let result = std::process::Command::new("/usr/bin/git")
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .current_dir(root)
            .args(args)
            .output()
            .unwrap();
        assert!(result.status.success(), "fixture Git failed");
    }
    #[test]
    fn offline_oracle_git_manifests_and_live_fallback_match() {
        use super::super::legacy_source_git::{listed, Listed};
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/legacy-git-source.json"))
                .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let base = Root::new();
            let mut root = base.0.join("repo");
            fs::create_dir(&root).unwrap();
            git(&root, &["init", "-b", "main"]);
            populate(&root, &case["entries"]);
            git(&root, &["add", "source.txt"]);
            if case["name"] == "linked-worktree" {
                git(
                    &root,
                    &[
                        "-c",
                        "user.name=Fixture",
                        "-c",
                        "user.email=fixture@example.invalid",
                        "-c",
                        "commit.gpgsign=false",
                        "-c",
                        "core.hooksPath=/dev/null",
                        "commit",
                        "-m",
                        "fixture",
                    ],
                );
                let tree = base.0.join("tree");
                git(
                    &root,
                    &["worktree", "add", "-b", "task", tree.to_str().unwrap()],
                );
                root = tree;
                populate(&root, &case["entries"]);
            }
            let raw = case["listed"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_u64().unwrap() as u8)
                .collect::<Vec<_>>();
            let mut budget = Budget {
                listed: Some(Listed::parse(&raw).unwrap()),
                max_files: 0,
                ..Budget::default()
            };
            let (digest, manifest) = scan_selected(&root, &mut budget, &mut |_, _| {}).unwrap();
            assert_eq!(digest, case["selected"]["dirty_digest"], "{}", case["name"]);
            assert_eq!(
                serde_json::json!(text(&manifest)),
                case["selected"]["manifest"]
            );
            let (digest, manifest) =
                scan_selected(&root, &mut Budget::default(), &mut |_, _| {}).unwrap();
            assert_eq!(digest, case["fallback"]["dirty_digest"], "{}", case["name"]);
            assert_eq!(
                serde_json::json!(text(&manifest)),
                case["fallback"]["manifest"]
            );

            // A planted monitor/filter must never execute, even on the real transport.
            let sentinel = base.0.join("must-not-exist");
            let poison = format!("!touch {}", sentinel.display());
            git(&root, &["config", "core.fsmonitor", &poison]);
            git(&root, &["config", "filter.poison.clean", &poison]);
            let before_index = fs::read(if case["name"] == "linked-worktree" {
                base.0.join("repo/.git/worktrees/tree/index")
            } else {
                root.join(".git/index")
            })
            .unwrap();
            let selection = listed(&root).unwrap();
            let expected = if selection.is_some() {
                "selected"
            } else {
                eprintln!(
                    "Git source sandbox unavailable: testing real bounded whole-tree fallback"
                );
                "fallback"
            };
            assert_eq!(
                capture(&root).unwrap().unwrap(),
                case[expected]["dirty_digest"]
            );
            assert!(!sentinel.exists());
            let after_index = fs::read(if case["name"] == "linked-worktree" {
                base.0.join("repo/.git/worktrees/tree/index")
            } else {
                root.join(".git/index")
            })
            .unwrap();
            assert_eq!(before_index, after_index);
        }
        assert!(Listed::parse(b"no-terminator").is_err());
        // Non-UTF-8 and never-matching names parse like the legacy reader.
        let list = Listed::parse(b"\xff/x\0../outside\0").unwrap();
        assert!(list.includes(b"\xff/x", false));
        assert!(list.includes(b"\xff", true));
        assert!(!list.includes(b"\xff", false));
    }
    #[test]
    fn unusable_git_metadata_falls_back_without_opening_fifos_or_links() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/legacy-source.json")).unwrap();
        let case = &fixture["cases"][1];
        for poison in [
            ".git",
            "pointer",
            "link",
            "HEAD",
            "index",
            "config",
            "packed-refs",
            "info/exclude",
            "commondir",
        ] {
            let root = Root::new();
            populate(&root.0, &case["entries"]);
            if poison == "pointer" {
                fs::write(root.0.join(".git"), "invalid pointer").unwrap();
            } else if poison == "link" {
                symlink("missing", root.0.join(".git")).unwrap();
            } else {
                let target = if poison == ".git" {
                    root.0.join(".git")
                } else {
                    let target = root.0.join(".git").join(poison);
                    fs::create_dir_all(target.parent().unwrap()).unwrap();
                    target
                };
                let target = std::ffi::CString::new(target.as_os_str().as_bytes()).unwrap();
                assert_eq!(unsafe { libc::mkfifo(target.as_ptr(), 0o600) }, 0);
            }
            assert!(
                super::super::legacy_source_git::listed(&root.0)
                    .unwrap()
                    .is_none(),
                "{poison}"
            );
            assert_eq!(
                capture(&root.0).unwrap().unwrap(),
                case["expected"]["dirty_digest"],
                "{poison}"
            );
        }
    }
    #[test]
    fn metadata_and_retained_boundaries_are_explicit() {
        use std::os::unix::ffi::OsStringExt;
        let root = Root::new();
        let path = root.0.join("large");
        let file = File::create(&path).unwrap();
        file.set_len(1024 * 1024 + 1).unwrap();
        let st = file.metadata().unwrap();
        let expected = metadata_hash(
            st.len(),
            st.ino(),
            st.mtime() as i128 * 1_000_000_000 + st.mtime_nsec() as i128,
            st.ctime() as i128 * 1_000_000_000 + st.ctime_nsec() as i128,
        );
        let (digest, records) = scan(&root.0, &mut Budget::default(), &mut |_, _| {}).unwrap();
        assert_eq!(
            text(&records),
            vec![format!("f\tlarge\t{}\t{expected}", st.len())]
        );
        assert_eq!(capture(&root.0).unwrap().unwrap(), digest);
        file.set_len(1024 * 1024).unwrap();
        let (_, records) = scan(&root.0, &mut Budget::default(), &mut |_, _| {}).unwrap();
        assert!(!text(&records)[0].contains("stat:"));
        let mut budget = Budget {
            max_bytes: 0,
            ..Budget::default()
        };
        let (_, records) = scan(&root.0, &mut budget, &mut |_, _| {}).unwrap();
        assert!(text(&records)[0].contains("stat:"));
        // Non-UTF-8 names and targets are ported byte for byte, ordered like Python's
        // surrogateescape strings (U+DC80.. sorts after U+00E9 but before U+E000).
        let root = Root::new();
        for name in [
            vec![0xffu8],
            "\u{e9}".as_bytes().to_vec(),
            "\u{e000}".as_bytes().to_vec(),
            b"a".to_vec(),
        ] {
            fs::write(root.0.join(std::ffi::OsString::from_vec(name)), "source").unwrap();
        }
        symlink(
            std::ffi::OsString::from_vec(vec![b't', 0xfe]),
            root.0.join("link"),
        )
        .unwrap();
        let (digest, records) = scan(&root.0, &mut Budget::default(), &mut |_, _| {}).unwrap();
        let paths = records
            .iter()
            .map(|r| r.split(|b| *b == b'\t').nth(1).unwrap().to_vec())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            vec![
                b"a".to_vec(),
                "\u{e9}".as_bytes().to_vec(),
                vec![0xff],
                "\u{e000}".as_bytes().to_vec(),
                b"link".to_vec()
            ]
        );
        assert!(records.iter().any(|r| r.ends_with(&[b't', 0xfe])));
        assert_eq!(capture(&root.0).unwrap().unwrap(), digest);

        let root = Root::new();
        let mut path = root.0.clone();
        for _ in 0..130 {
            path.push("d");
            fs::create_dir(&path).unwrap();
        }
        // Too deep to scan: an explicit unobservable source, never a digest.
        assert!(capture(&root.0).unwrap().is_none());
    }
    #[test]
    fn source_races_and_limits_never_return_a_partial_digest() {
        for phase in ["entry-file", "entry-fifo", "entry-dir", "read", "scanned"] {
            let root = Root::new();
            fs::write(root.0.join("file"), "old").unwrap();
            fs::create_dir(root.0.join("dir")).unwrap();
            fs::write(root.0.join("dir/child"), "child").unwrap();
            let mut fired = false;
            let result = scan(&root.0, &mut Budget::default(), &mut |at, path| {
                if fired {
                    return;
                }
                match (phase, at, path) {
                    ("entry-file", "entry", "file") => {
                        fs::rename(root.0.join("file"), root.0.join("old")).unwrap();
                        symlink("old", root.0.join("file")).unwrap();
                    }
                    ("entry-fifo", "entry", "file") => {
                        fs::remove_file(root.0.join("file")).unwrap();
                        let path =
                            std::ffi::CString::new(root.0.join("file").as_os_str().as_bytes())
                                .unwrap();
                        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
                    }
                    ("entry-dir", "entry", "dir") => {
                        fs::rename(root.0.join("dir"), root.0.join("old-dir")).unwrap();
                        symlink("old-dir", root.0.join("dir")).unwrap();
                    }
                    ("read", "read", "file") => {
                        fs::write(root.0.join("file"), "changed").unwrap();
                    }
                    ("scanned", "scanned", "") => {
                        fs::write(root.0.join("added"), "new").unwrap();
                    }
                    _ => return,
                }
                fired = true;
            });
            assert!(fired);
            assert!(
                matches!(result, Err(ScanError::Unavailable)),
                "{phase}: {result:?}"
            );
        }
        let root = Root::new();
        fs::write(root.0.join("file"), "text").unwrap();
        let mut budget = Budget {
            max_files: 0,
            ..Budget::default()
        };
        assert!(scan(&root.0, &mut budget, &mut |_, _| {}).is_err());
        let mut budget = Budget {
            deadline: Instant::now() - Duration::from_secs(1),
            ..Budget::default()
        };
        assert!(scan(&root.0, &mut budget, &mut |_, _| {}).is_err());
        assert!(capture(&root.0.join("missing")).unwrap().is_none());
        symlink(&root.0, root.0.join("alias")).unwrap();
        assert!(capture(&root.0.join("alias")).unwrap().is_none());
        fs::create_dir(root.0.join(".git")).unwrap();
        // Unusable Git metadata falls back to the same bounded whole-tree observer.
        assert!(capture(&root.0).unwrap().is_some());
    }
}
