//! Which deliverable paths changed while one task command ran, and the current
//! state of the files a review binds.
//!
//! A gateway file tool leaves a write receipt naming its path; a command does not.
//! So that a review can require the task's own changes and nothing else, the host
//! lists the checkout's dirty deliverable paths before a writable command is
//! launched and again after its process ended (`git status --porcelain=v2 -z
//! --untracked-files=all`: tracked changes and untracked files, never Git-ignored
//! ones, each with its status record plus size, mtime, ctime, mode and inode).
//! Clean files are neither visited nor hashed. A path whose entry differs between
//! the two listings changed during the run and is recorded on the command's
//! `NATIVE_COMMAND_FINISHED` observation (`changed_paths`), even when another actor
//! changed it at the same time. A folder outside any repository is walked instead
//! (every file's metadata). A failed or oversized listing, a nested checkout, or
//! too many changed paths records `changed_paths_complete: false` with the reason:
//! the review of that task then requires every changed file and binds the whole
//! checkout, as before. A read-only command, or one that provably never started,
//! records an empty, complete list without listing anything.
//!
//! The limit is exact: a path changed and restored to the same status record and
//! metadata within one run is invisible, as are Git-ignored files (not deliverable)
//! and changes inside a nested checkout (which make the record incomplete).
use cortex_core::{
    lifecycle::completion::{self, EvidenceSnapshot},
    store::executor::StoreExecutor,
    CoreError, CoreResult,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    hash::{Hash, Hasher},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};

/// A listing with more dirty paths than this is not kept.
const PATH_LIMIT: usize = 200_000;
/// Bytes of one `git status` listing; a longer one is not kept.
const OUTPUT_LIMIT: usize = 64 << 20;
/// Changed paths recorded for one command; more make its record incomplete.
pub(crate) const CHANGED_LIMIT: usize = 5_000;
/// Bytes hashed for the files one observation binds; past it a file is identified
/// by its metadata (including ctime, which restoring an mtime cannot reset).
const BINDING_BYTES: u64 = 256 << 20;

/// The dirty deliverable paths of the checkout, each with a fingerprint of its
/// status record and metadata, or why they could not be listed.
pub(crate) type Snapshot = Result<BTreeMap<String, u64>, &'static str>;

enum Listing {
    Git { prefix: Vec<u8>, status: Vec<u8> },
    Plain(Vec<u8>),
}

fn listing_error(e: &CoreError) -> &'static str {
    if e.code() == "project_unobservable_too_many_paths" {
        "too_many_paths"
    } else {
        "listing_failed"
    }
}

/// List the checkout's dirty deliverable paths with their fingerprints.
pub(crate) async fn snapshot(root: PathBuf) -> Snapshot {
    let listing = if crate::native_effects::no_repository(&root).await {
        let r = root.clone();
        match tokio::task::spawn_blocking(move || crate::native_effects::plain_paths(&r)).await {
            Ok(Ok(paths)) => Listing::Plain(paths),
            Ok(Err(e)) => return Err(listing_error(&e)),
            Err(_) => return Err("listing_failed"),
        }
    } else {
        let prefix =
            crate::native_effects::git_with_limit(&root, &["rev-parse", "--show-prefix"], 1 << 16)
                .await
                .map_err(|e| listing_error(&e))?;
        let status = crate::native_effects::git_with_limit(
            &root,
            &[
                "-c",
                "core.quotePath=false",
                "status",
                "--porcelain=v2",
                "-z",
                "--untracked-files=all",
                "--no-renames",
                "--ignore-submodules=none",
                "--",
                ".",
                ":(exclude).cortex",
                ":(exclude).cortex-dev",
                ":(exclude).cortex-dev-owner.json",
                ":(exclude).cortex-workspace-dev.json",
            ],
            OUTPUT_LIMIT,
        )
        .await
        .map_err(|e| listing_error(&e))?;
        let prefix = prefix.strip_suffix(b"\n").unwrap_or(&prefix).to_vec();
        Listing::Git { prefix, status }
    };
    tokio::task::spawn_blocking(move || fingerprints(&root, listing))
        .await
        .unwrap_or(Err("listing_failed"))
}

/// Each listed path's fingerprint: its status record (Git) and its metadata.
fn fingerprints(root: &Path, listing: Listing) -> Snapshot {
    let mut entries: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    match listing {
        Listing::Plain(paths) => {
            for path in paths.split(|b| *b == 0).filter(|p| !p.is_empty()) {
                entries.push((path.to_vec(), Vec::new()));
            }
        }
        Listing::Git { prefix, status } => {
            for record in status.split(|b| *b == 0).filter(|r| !r.is_empty()) {
                // `1 XY sub mH mI mW hH hI path`, `u XY sub m1 m2 m3 mW h1 h2 h3 path`,
                // `? path`; renames are off, ignored files are not listed.
                let fields = match record.first() {
                    Some(b'1') => 9,
                    Some(b'u') => 11,
                    Some(b'?') => 2,
                    Some(b'#') => continue,
                    _ => return Err("listing_failed"),
                };
                let mut parts = record.splitn(fields, |b| *b == b' ');
                let mut head = Vec::new();
                for _ in 0..fields - 1 {
                    head.extend_from_slice(parts.next().ok_or("listing_failed")?);
                    head.push(b' ');
                }
                let path = parts.next().ok_or("listing_failed")?;
                // A submodule's own changes are not listed here.
                if head.get(5) == Some(&b'S') && record[0] != b'?' {
                    return Err("nested_checkout");
                }
                let Some(path) = path.strip_prefix(prefix.as_slice()) else {
                    continue;
                };
                entries.push((path.to_vec(), head));
            }
        }
    }
    if entries.len() > PATH_LIMIT {
        return Err("too_many_paths");
    }
    let mut out = BTreeMap::new();
    for (path, record) in entries {
        // A directory entry is a nested checkout whose files are not listed.
        if path.ends_with(b"/") {
            return Err("nested_checkout");
        }
        let path = String::from_utf8(path).map_err(|_| "listing_failed")?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        record.hash(&mut hasher);
        metadata(root, &path).hash(&mut hasher);
        out.insert(path, hasher.finish());
    }
    Ok(out)
}

/// A path's metadata identity, `absent`, or `unobservable`.
fn metadata(root: &Path, path: &str) -> String {
    match root.join(path).symlink_metadata() {
        Ok(m) => format!(
            "meta:{:o}:{}:{}.{}:{}.{}:{}:{}",
            m.mode(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
            m.dev(),
            m.ino()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "absent".into(),
        Err(_) => "unobservable".into(),
    }
}

/// What changed in a checkout after a launch instant, judged by inode change times
/// (ctime, which no unprivileged process can set back). The "before" observations of
/// a command (project digest and dirty listing) run beside its launch; when one was
/// still running when the payload was released, this tells whether what it saw can
/// still be the checkout as it was at launch.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Touched {
    /// Deliverable paths (tracked, or untracked and not ignored) changed since.
    pub files: BTreeSet<String>,
    /// Directories (relative, `""` for the checkout root) in which an entry was
    /// created, removed or renamed since.
    pub dirs: BTreeSet<String>,
    /// Git's index, HEAD, current ref or exclude file changed since.
    pub git: bool,
    /// Why the sweep cannot vouch for anything (a nested checkout, too many entries,
    /// a failed read): a fixed reason code.
    pub structure: Option<&'static str>,
}

impl Touched {
    pub(crate) fn quiet(&self) -> bool {
        self.files.is_empty() && self.dirs.is_empty() && !self.git && self.structure.is_none()
    }

    /// Whether a listed path may have changed: touched itself, in a touched directory,
    /// or any path once Git's own state moved.
    fn covers(&self, path: &str) -> bool {
        self.git
            || self.files.contains(path)
            || self.dirs.contains(path.rsplit_once('/').map_or("", |(dir, _)| dir))
    }
}

/// Entries one `touched_since` sweep visits before it gives up (as unprovable).
const SWEEP_LIMIT: usize = 300_000;
/// The kernel stamps ctime from a coarse clock (one tick, at most 10 ms, behind):
/// a change made right after the launch instant can carry a slightly earlier stamp.
const CLOCK_MARGIN: std::time::Duration = std::time::Duration::from_millis(50);

/// Every deliverable path, every directory outside wholly ignored trees and Git's
/// index, HEAD and current ref, checked for a ctime at or after `since`.
pub(crate) async fn touched_since(root: PathBuf, since: std::time::SystemTime) -> Touched {
    let unprovable = |reason| Touched { structure: Some(reason), ..Default::default() };
    let since = since.checked_sub(CLOCK_MARGIN).unwrap_or(since);
    let Ok(since) = since.duration_since(std::time::UNIX_EPOCH) else {
        return unprovable("clock_unavailable");
    };
    let since = (since.as_secs() as i64, since.subsec_nanos() as i64);
    const EXCLUDE: [&str; 6] = [
        "--",
        ".",
        ":(exclude).cortex",
        ":(exclude).cortex-dev",
        ":(exclude).cortex-dev-owner.json",
        ":(exclude).cortex-workspace-dev.json",
    ];
    let git = |args: &'static [&'static str]| {
        let root = root.clone();
        async move {
            let mut argv = args.to_vec();
            argv.extend(EXCLUDE);
            crate::native_effects::git_with_limit(&root, &argv, OUTPUT_LIMIT).await
        }
    };
    let (paths, pruned, controls) = if crate::native_effects::no_repository(&root).await {
        (None, BTreeSet::new(), Vec::new())
    } else {
        let listed = git(&["ls-files", "--cached", "--others", "--exclude-standard", "-z"]).await;
        let ignored = git(&[
            "ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z",
        ])
        .await;
        let (Ok(listed), Ok(ignored)) = (listed, ignored) else {
            return unprovable("sweep_failed");
        };
        let pruned: BTreeSet<Vec<u8>> = ignored
            .split(|c| *c == 0)
            .filter_map(|p| p.strip_suffix(b"/"))
            .filter(|p| !p.is_empty())
            .map(<[u8]>::to_vec)
            .collect();
        let mut wanted = vec!["rev-parse", "--git-path", "index", "--git-path", "HEAD",
            "--git-path", "packed-refs", "--git-path", "info/exclude"];
        let reference = crate::native_effects::git_with_limit(&root, &["symbolic-ref", "-q", "HEAD"], 1 << 16)
            .await
            .ok()
            .and_then(|r| String::from_utf8(r).ok())
            .map(|r| r.trim_end_matches('\n').to_owned())
            .filter(|r| r.starts_with("refs/") && !r.contains(".."));
        if let Some(r) = &reference {
            wanted.extend(["--git-path", r]);
        }
        let Ok(out) = crate::native_effects::git_with_limit(&root, &wanted, 1 << 16).await else {
            return unprovable("sweep_failed");
        };
        let mut controls: Vec<PathBuf> = String::from_utf8_lossy(&out)
            .lines()
            .map(|l| root.join(l))
            .collect();
        if reference.is_some() {
            if let Some(parent) = controls.last().and_then(|p| p.parent()).map(Path::to_path_buf) {
                controls.push(parent);
            }
        }
        (Some(listed), pruned, controls)
    };
    tokio::task::spawn_blocking(move || sweep(&root, since, paths, &pruned, &controls))
        .await
        .unwrap_or_else(|_| unprovable("sweep_failed"))
}

fn sweep(
    root: &Path,
    since: (i64, i64),
    paths: Option<Vec<u8>>,
    pruned: &BTreeSet<Vec<u8>>,
    controls: &[PathBuf],
) -> Touched {
    use std::os::unix::ffi::OsStrExt;
    let newer = |m: &std::fs::Metadata| (m.ctime(), m.ctime_nsec()) >= since;
    let mut touched = Touched::default();
    for control in controls {
        match control.symlink_metadata() {
            Ok(m) if newer(&m) => touched.git = true,
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => touched.structure = Some("sweep_failed"),
        }
    }
    let git = paths.is_some();
    if let Some(paths) = &paths {
        for path in paths.split(|c| *c == 0).filter(|p| !p.is_empty()) {
            let Ok(name) = std::str::from_utf8(path) else {
                touched.structure = Some("sweep_failed");
                continue;
            };
            match root.join(name).symlink_metadata() {
                // A gitlink or nested checkout: its own files are not listed here.
                Ok(m) if m.is_dir() => touched.structure = Some("nested_checkout"),
                Ok(m) if newer(&m) => {
                    touched.files.insert(name.to_owned());
                }
                Ok(_) => {}
                // A tracked deletion shows on its directory's change time.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => touched.structure = Some("sweep_failed"),
            }
        }
    }
    // Every directory outside wholly ignored trees and the private stores: an entry
    // created, removed or renamed there changes the directory's own ctime.
    let mut pending = vec![(root.to_path_buf(), Vec::<u8>::new())];
    let mut seen = 0usize;
    while let Some((dir, rel)) = pending.pop() {
        match dir.symlink_metadata() {
            Ok(m) if newer(&m) => {
                touched.dirs.insert(String::from_utf8_lossy(&rel).into_owned());
            }
            Ok(_) => {}
            // Removed since it was listed: its parent's ctime moved too.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => {
                touched.structure = Some("sweep_failed");
                continue;
            }
        }
        let Ok(entries) = std::fs::read_dir(&dir) else {
            touched.structure = Some("sweep_failed");
            continue;
        };
        for entry in entries {
            let Ok(entry) = entry else {
                touched.structure = Some("sweep_failed");
                continue;
            };
            seen += 1;
            if seen > SWEEP_LIMIT {
                touched.structure = Some("too_many_paths");
                return touched;
            }
            let name = entry.file_name();
            let name = name.as_bytes();
            let top = rel.is_empty();
            let Ok(kind) = entry.file_type() else {
                touched.structure = Some("sweep_failed");
                continue;
            };
            if name == b".git" {
                if !top {
                    touched.structure = Some("nested_checkout");
                }
                continue;
            }
            if crate::native_effects::plain_listing_excludes(name, top, kind.is_dir()) {
                continue;
            }
            let mut path = rel.clone();
            if !top {
                path.push(b'/');
            }
            path.extend_from_slice(name);
            if kind.is_dir() {
                if !pruned.contains(&path) {
                    pending.push((entry.path(), path));
                }
            } else if !git {
                // A plain folder: every file is deliverable.
                match entry.metadata() {
                    Ok(m) if newer(&m) => {
                        touched.files.insert(String::from_utf8_lossy(&path).into_owned());
                    }
                    Ok(_) => {}
                    Err(_) => touched.structure = Some("sweep_failed"),
                }
            }
        }
    }
    touched
}

/// The instant a command's payload was released, shared by the observations taken
/// beside its launch (`Before`, the work-set digest).
#[derive(Clone, Default)]
pub(crate) struct Launch(std::sync::Arc<std::sync::Mutex<Option<std::time::SystemTime>>>);

impl Launch {
    /// Called immediately before the payload is released.
    pub(crate) fn released(&self) {
        let mut at = self.0.lock().unwrap_or_else(|e| e.into_inner());
        at.get_or_insert_with(std::time::SystemTime::now);
    }

    /// For an observation that just ended: `None` when it ended before the payload was
    /// released (it saw the checkout as at launch), else what changed since the release.
    pub(crate) async fn touched(&self, root: PathBuf) -> Option<Touched> {
        let released = *self.0.lock().unwrap_or_else(|e| e.into_inner());
        Some(touched_since(root, released?).await)
    }
}

/// A command's "before" observations (its project digest and its dirty listing),
/// taken beside its preparation and launch instead of ahead of them, so that the
/// payload never waits for them. When they ended before the payload was released
/// they are exactly what a blocking observation would have seen. Otherwise a
/// `touched_since` sweep from the release instant, run as soon as they end, decides:
/// a quiet checkout keeps them; a touched path drops the digest (the check binds no
/// revision, as when the digests before and after differ) and is attributed to the
/// command; a change the record cannot name makes the record incomplete.
pub(crate) struct Before(tokio::task::JoinHandle<Observed>);

/// What `Before` observed, after the guard was applied.
pub(crate) struct Observed {
    pub digest: Option<String>,
    pub listing: Option<Snapshot>,
    /// For an observation that overlapped the payload: what changed since its release.
    pub touched: Option<Touched>,
    /// Numbers and fixed codes only: how long the observation ran and how it ended.
    pub meta: Value,
}

impl Before {
    pub(crate) fn start<F>(launch: Launch, root: PathBuf, digest: F, listing: bool) -> Self
    where
        F: std::future::Future<Output = Option<String>> + Send + 'static,
    {
        Self(tokio::spawn(async move {
            let started = std::time::Instant::now();
            let (digest, listing) = tokio::join!(digest, async {
                if listing {
                    Some(snapshot(root.clone()).await)
                } else {
                    None
                }
            });
            let observe_ms = started.elapsed().as_millis() as u64;
            let swept = std::time::Instant::now();
            // Nothing observed (a scoped or trivial read-only command): nothing to vouch for.
            if digest.is_none() && listing.is_none() {
                return Observed { digest, listing, touched: None, meta: json!({"before_observation":"none"}) };
            }
            let Some(touched) = launch.touched(root).await else {
                return Observed {
                    digest,
                    listing,
                    touched: None,
                    meta: json!({"before_observation":"before_release","before_observation_ms":observe_ms}),
                };
            };
            let sweep_ms = swept.elapsed().as_millis() as u64;
            let state = if touched.quiet() { "overlapped" } else { "overlapped_changed" };
            let mut meta = json!({"before_observation":state,"before_observation_ms":observe_ms,
                "before_sweep_ms":sweep_ms});
            if let Some(reason) = touched.structure {
                meta["before_sweep_reason"] = json!(reason);
            }
            Observed {
                digest: digest.filter(|_| touched.quiet()),
                listing: match (listing, touched.structure) {
                    (Some(Ok(_)), Some(reason)) => Some(Err(reason)),
                    (listing, _) => listing,
                },
                touched: Some(touched),
                meta,
            }
        }))
    }

    /// The observation, once it (and its sweep) ended. A lost observation task is an
    /// unobserved digest and a failed listing, never a pass.
    pub(crate) async fn finish(self) -> Observed {
        self.0.await.unwrap_or_else(|_| Observed {
            digest: None,
            listing: Some(Err("listing_failed")),
            touched: None,
            meta: json!({"before_observation":"lost"}),
        })
    }
}

/// The change record of a command from its listings before launch and after its
/// process ended.
pub(crate) fn record(before: &Snapshot, after: &Snapshot) -> Value {
    record_with(before, after, None)
}

/// `record` for a "before" listing that overlapped the payload, attributing
/// conservatively what `touched` says may have changed since the release: touched
/// paths, listed paths in touched directories, and every listed path once Git's own
/// state moved.
pub(crate) fn record_with(before: &Snapshot, after: &Snapshot, touched: Option<&Touched>) -> Value {
    let (before, after) = match (before, after) {
        (Ok(before), Ok(after)) => (before, after),
        (Err(reason), _) | (_, Err(reason)) => return incomplete(reason),
    };
    let none = Touched::default();
    let touched = touched.unwrap_or(&none);
    let changed: Vec<&String> = before
        .keys()
        .chain(after.keys())
        .chain(touched.files.iter())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| touched.covers(path) || before.get(*path) != after.get(*path))
        .collect();
    if changed.len() > CHANGED_LIMIT {
        let mut value = incomplete("too_many_changed_paths");
        value["changed_paths_count"] = json!(changed.len());
        return value;
    }
    json!({"changed_paths":changed,"changed_paths_complete":true,"changed_paths_source":"listing"})
}
/// A complete record of no changes: a read-only or never-started command.
pub(crate) fn unchanged(source: &str) -> Value {
    json!({"changed_paths":[],"changed_paths_complete":true,"changed_paths_source":source})
}
/// An incomplete record: the review falls back to requiring every changed file.
pub(crate) fn incomplete(reason: &str) -> Value {
    json!({"changed_paths_complete":false,"changed_paths_reason":reason})
}
/// Add a change record to a command observation.
pub(crate) fn add(observation: &mut Value, record: &Value) {
    if let (Some(observation), Some(record)) = (observation.as_object_mut(), record.as_object()) {
        for (key, value) in record {
            observation.insert(key.clone(), value.clone());
        }
    }
}

/// The current state of each path: content hash and mode, link target, `absent`,
/// or its metadata past the byte bound (or for a directory or unreadable entry).
pub(crate) fn identify(root: &Path, paths: BTreeSet<String>) -> BTreeMap<String, String> {
    let mut budget = BINDING_BYTES;
    paths
        .into_iter()
        .map(|path| {
            let state = crate::native_work_set::state(root, &path, &mut budget)
                .unwrap_or_else(|| metadata(root, &path));
            (path, state)
        })
        .collect()
}
/// Identify, in `snapshot`, the files the task's review binds
/// (`completion::bound_files_tx`), from inside a store transaction.
pub(crate) fn bind_task_files_tx(
    db: &rusqlite::Connection,
    root: &Path,
    task: &str,
    snapshot: &mut EvidenceSnapshot,
) -> CoreResult<()> {
    let paths = completion::bound_files_tx(db, task)?;
    snapshot.task_files.extend(identify(root, paths));
    Ok(())
}
/// `bind_task_files_tx` outside a transaction.
pub(crate) async fn bind_task_files(
    executor: &StoreExecutor,
    root: &Path,
    task: &str,
    snapshot: &mut EvidenceSnapshot,
) -> CoreResult<()> {
    let t = task.to_owned();
    let paths = executor
        .read(move |db| completion::bound_files_tx(db, &t))?
        .await
        .map_err(|_| CoreError::Refused("native_host_closed"))??;
    if paths.is_empty() {
        return Ok(());
    }
    let root = root.to_owned();
    let states = tokio::task::spawn_blocking(move || identify(&root, paths))
        .await
        .map_err(|_| CoreError::Refused("native_effect_unavailable"))?;
    snapshot.task_files.extend(states);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(root: &Path, args: &[&str]) {
        assert!(std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t"])
            .args(args)
            .current_dir(root)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success());
    }
    fn paths(record: &Value) -> Vec<String> {
        serde_json::from_value(record["changed_paths"].clone()).unwrap()
    }
    fn repository(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "native-change-{name}-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "fn a() {}").unwrap();
        std::fs::write(root.join("README.md"), "r").unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "base"]);
        std::fs::create_dir_all(root.join("target/debug")).unwrap();
        // Past the ctime clock margin: the fixture itself predates every launch below.
        std::thread::sleep(std::time::Duration::from_millis(120));
        root
    }

    /// The sweep from a release instant names a changed deliverable file, and reports
    /// a created entry or a moved index as a change it cannot name; ignored build
    /// output and changes before the instant are not changes.
    #[tokio::test]
    async fn a_sweep_names_what_changed_after_the_release_instant() {
        let root = repository("sweep");
        std::fs::write(root.join("target/debug/out"), "o").unwrap();
        let released = std::time::SystemTime::now() + std::time::Duration::from_millis(60);
        std::thread::sleep(std::time::Duration::from_millis(120));
        let quiet = touched_since(root.clone(), released).await;
        assert!(quiet.quiet(), "{quiet:?}");
        // Ignored build output written after the instant is not deliverable.
        std::fs::write(root.join("target/debug/out"), "o2").unwrap();
        std::fs::write(root.join("target/debug/more"), "m").unwrap();
        assert!(touched_since(root.clone(), released).await.quiet());
        std::fs::write(root.join("src/lib.rs"), "fn b() {}").unwrap();
        let edited = touched_since(root.clone(), released).await;
        assert_eq!(edited.files, BTreeSet::from(["src/lib.rs".to_owned()]));
        assert_eq!(edited.structure, None);
        std::fs::write(root.join("src/new.rs"), "new").unwrap();
        let created = touched_since(root.clone(), released).await;
        assert!(created.files.contains("src/new.rs"), "{created:?}");
        assert_eq!(created.dirs, BTreeSet::from(["src".to_owned()]));
        assert_eq!(created.structure, None);
        std::fs::remove_file(root.join("README.md")).unwrap();
        let removed = touched_since(root.clone(), released).await;
        assert_eq!(removed.dirs, BTreeSet::from(["".to_owned(), "src".to_owned()]));
        let staged = repository("staged");
        let released = std::time::SystemTime::now();
        std::fs::write(staged.join("README.md"), "staged").unwrap();
        git(&staged, &["add", "README.md"]);
        let moved = touched_since(staged.clone(), released).await;
        assert!(moved.git && moved.structure.is_none(), "{moved:?}");
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&staged).unwrap();
    }

    /// A "before" observation that ended before the release is kept exactly; one still
    /// running at the release holds only for a checkout untouched since, and a file the
    /// command changed meanwhile is attributed to it while the digest binds nothing.
    #[tokio::test(flavor = "current_thread")]
    async fn a_before_observation_beside_the_launch_never_binds_a_changed_checkout() {
        let root = repository("before");
        let digest = |root: &Path| {
            let root = root.to_path_buf();
            async move { crate::native_effects::observe_digest(root).await.ok() }
        };
        let pristine = digest(&root).await.unwrap();
        // Ended before the release: exactly the launch state, even if the payload
        // changes a file right after.
        let launch = Launch::default();
        let before = Before::start(launch.clone(), root.clone(), digest(&root), true);
        for _ in 0..200 {
            if before.0.is_finished() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        launch.released();
        std::fs::write(root.join("src/lib.rs"), "fn b() {}").unwrap();
        let observed = before.finish().await;
        assert_eq!(observed.digest.as_deref(), Some(pristine.as_str()));
        assert_eq!(observed.meta["before_observation"], "before_release");
        let after = snapshot(root.clone()).await;
        let r = record_with(observed.listing.as_ref().unwrap(), &after, observed.touched.as_ref());
        assert_eq!(paths(&r), ["src/lib.rs"]);
        git(&root, &["checkout", "-q", "--", "src/lib.rs"]);
        std::thread::sleep(std::time::Duration::from_millis(120));
        // Released before the observation ran (a current-thread runtime runs the
        // spawned observation only at the first await): the edit made after the
        // release is seen by the observation itself, so its digest must not bind.
        let launch = Launch::default();
        let before = Before::start(launch.clone(), root.clone(), digest(&root), true);
        launch.released();
        std::fs::write(root.join("README.md"), "edited by the command").unwrap();
        let observed = before.finish().await;
        assert_eq!(observed.digest, None);
        assert_eq!(observed.meta["before_observation"], "overlapped_changed");
        assert_eq!(
            observed.touched.as_ref().unwrap().files,
            BTreeSet::from(["README.md".to_owned()])
        );
        let after = snapshot(root.clone()).await;
        let r = record_with(observed.listing.as_ref().unwrap(), &after, observed.touched.as_ref());
        assert_eq!(r["changed_paths_complete"], true, "{r}");
        assert_eq!(paths(&r), ["README.md"]);
        // Released before the observation, nothing touched: the overlapped digest holds.
        std::thread::sleep(std::time::Duration::from_millis(120));
        let current = digest(&root).await.unwrap();
        let launch = Launch::default();
        let before = Before::start(launch.clone(), root.clone(), digest(&root), true);
        launch.released();
        let observed = before.finish().await;
        assert_eq!(observed.meta["before_observation"], "overlapped");
        assert_eq!(observed.digest.as_deref(), Some(current.as_str()));
        // Entries created and removed during the overlap: the created file and every
        // listed path in a changed directory are attributed (a tracked deletion the
        // overlapping listing already saw included); the digest binds nothing.
        std::thread::sleep(std::time::Duration::from_millis(120));
        let launch = Launch::default();
        let before = Before::start(launch.clone(), root.clone(), digest(&root), true);
        launch.released();
        std::fs::write(root.join("created.txt"), "c").unwrap();
        std::fs::remove_file(root.join("src/lib.rs")).unwrap();
        let observed = before.finish().await;
        assert_eq!(observed.digest, None);
        let after = snapshot(root.clone()).await;
        let r = record_with(observed.listing.as_ref().unwrap(), &after, observed.touched.as_ref());
        assert_eq!(r["changed_paths_complete"], true, "{r}");
        assert_eq!(paths(&r), ["README.md", "created.txt", "src/lib.rs"]);
        // Git's own state moving during the overlap attributes every listed path.
        std::thread::sleep(std::time::Duration::from_millis(120));
        let launch = Launch::default();
        let before = Before::start(launch.clone(), root.clone(), digest(&root), true);
        launch.released();
        git(&root, &["add", "created.txt"]);
        let observed = before.finish().await;
        assert!(observed.touched.as_ref().unwrap().git);
        let after = snapshot(root.clone()).await;
        let r = record_with(observed.listing.as_ref().unwrap(), &after, observed.touched.as_ref());
        assert_eq!(paths(&r), ["README.md", "created.txt", "src/lib.rs"]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[tokio::test]
    async fn a_command_record_names_exactly_the_paths_changed_while_it_ran() {
        let root = std::env::temp_dir().join(format!(
            "native-change-set-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join(".gitignore"), "target/\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "fn a() {}").unwrap();
        std::fs::write(root.join("README.md"), "r").unwrap();
        std::fs::write(root.join("gone.txt"), "g").unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "base"]);
        // Unrelated uncommitted work that exists before and stays unchanged.
        std::fs::write(root.join("README.md"), "someone else's edit").unwrap();
        for n in 0..50 {
            std::fs::write(root.join(format!("other-{n}.txt")), "x").unwrap();
        }
        let before = snapshot(root.clone()).await;
        assert_eq!(before.as_ref().unwrap().len(), 51);
        // What the command does: edit a clean file, add one, delete a tracked one,
        // further edit an already dirty one, and write ignored build output.
        std::fs::write(root.join("src/lib.rs"), "fn b() {}").unwrap();
        std::fs::write(root.join("src/new.rs"), "new").unwrap();
        std::fs::remove_file(root.join("gone.txt")).unwrap();
        std::fs::write(root.join("other-7.txt"), "changed again").unwrap();
        std::fs::create_dir_all(root.join("target")).unwrap();
        std::fs::write(root.join("target/out"), "o").unwrap();
        let after = snapshot(root.clone()).await;
        let r = record(&before, &after);
        assert_eq!(r["changed_paths_complete"], true);
        assert_eq!(
            paths(&r),
            ["gone.txt", "other-7.txt", "src/lib.rs", "src/new.rs"]
        );
        // Nothing changed: an empty, complete record.
        let again = snapshot(root.clone()).await;
        assert!(paths(&record(&after, &again)).is_empty());
        // A failed listing on either side is never a complete record.
        let failed: Snapshot = Err("listing_failed");
        for r in [record(&failed, &after), record(&after, &failed)] {
            assert_eq!(r["changed_paths_complete"], false);
            assert_eq!(r["changed_paths_reason"], "listing_failed");
            assert!(r["changed_paths"].is_null());
        }
        // A nested checkout's own files cannot be listed.
        std::fs::create_dir_all(root.join("vendor/dep")).unwrap();
        git(&root.join("vendor/dep"), &["init", "-q"]);
        std::fs::write(root.join("vendor/dep/x"), "x").unwrap();
        assert_eq!(snapshot(root.clone()).await, Err("nested_checkout"));
        std::fs::remove_dir_all(root.join("vendor")).unwrap();
        // A plain folder is walked: every file's metadata.
        let plain = root.with_extension("plain");
        std::fs::create_dir_all(plain.join("a")).unwrap();
        std::fs::write(plain.join("a/x"), "x").unwrap();
        let p0 = snapshot(plain.clone()).await;
        std::fs::write(plain.join("a/y"), "y").unwrap();
        assert_eq!(paths(&record(&p0, &snapshot(plain.clone()).await)), ["a/y"]);
        // Bound files are identified by content, so equal bytes are equal states.
        let states = identify(
            &root,
            ["src/lib.rs", "gone.txt"]
                .map(String::from)
                .into_iter()
                .collect(),
        );
        assert_eq!(states["gone.txt"], "absent");
        assert!(states["src/lib.rs"].starts_with("file:"));
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&plain).unwrap();
    }
}
