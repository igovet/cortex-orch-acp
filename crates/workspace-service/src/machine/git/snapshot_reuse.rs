//! Bounded in-process reuse for `Git::snapshot`: the last snapshot of each project and
//! the immutable `--shortstat` counts of commits already seen. A Git-changing command
//! run through the service, and every owner Git action, advances one generation that
//! is part of every snapshot key, so no snapshot read before it is served after it.
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, LazyLock, Mutex,
    },
};
/// Projects whose last snapshot is kept; more clears the idle slots.
const SLOTS: usize = 16;
/// Repositories whose commit counts are kept; more starts over.
const REPOSITORIES: usize = 32;
/// Commits kept per repository; more starts that repository over.
const COMMITS: usize = 4096;
pub(super) struct Seen {
    pub key: String,
    pub at: std::time::Instant,
    pub value: Value,
}
type Slot = Arc<tokio::sync::Mutex<Option<Seen>>>;
#[derive(Default)]
struct SnapshotCache {
    generation: AtomicU64,
    slots: Mutex<HashMap<(PathBuf, String), Slot>>,
}
static SNAPSHOTS: LazyLock<SnapshotCache> = LazyLock::new(Default::default);

#[cfg(test)]
tokio::task_local! {
    static TEST_SNAPSHOTS: SnapshotCache;
}

/// Keep both generation and slot eviction private to the reuse regression:
/// unrelated Git tests run in parallel and legitimately invalidate the shared cache.
#[cfg(test)]
pub(super) async fn isolated<F: std::future::Future>(test: F) -> F::Output {
    TEST_SNAPSHOTS.scope(SnapshotCache::default(), test).await
}

fn with_cache<T>(f: impl Fn(&SnapshotCache) -> T) -> T {
    #[cfg(test)]
    if let Ok(value) = TEST_SNAPSHOTS.try_with(&f) {
        return value;
    }
    f(&SNAPSHOTS)
}
type Stats = HashMap<PathBuf, HashMap<String, (u64, u64, u64)>>;
static STATS: LazyLock<Mutex<Stats>> = LazyLock::new(Default::default);
pub(super) fn generation() -> u64 {
    with_cache(|cache| cache.generation.load(Ordering::SeqCst))
}
pub(super) fn invalidate() {
    with_cache(|cache| cache.generation.fetch_add(1, Ordering::SeqCst));
}
/// Advances the generation when dropped: at the end of an owner action, success or not.
pub(super) struct OnDrop;
impl Drop for OnDrop {
    fn drop(&mut self) {
        invalidate();
    }
}
/// The one reuse slot of `(root, project)`. Snapshots of one project are taken one at
/// a time, so a burst of identical requests runs Git once and the rest reuse it.
pub(super) fn slot(root: &Path, project: &str) -> Slot {
    with_cache(|cache| {
        let mut slots = cache.slots.lock().unwrap_or_else(|e| e.into_inner());
        let key = (root.to_path_buf(), project.to_owned());
        if slots.len() >= SLOTS && !slots.contains_key(&key) {
            slots.retain(|_, s| Arc::strong_count(s) > 1);
        }
        slots.entry(key).or_default().clone()
    })
}
/// The known counts among `hashes`, and the hashes still to compute (in order).
pub(super) fn known_stats(
    root: &Path,
    hashes: &[String],
) -> (BTreeMap<String, (u64, u64, u64)>, Vec<String>) {
    let stats = STATS.lock().unwrap_or_else(|e| e.into_inner());
    let seen = stats.get(root);
    let mut found = BTreeMap::new();
    let mut missing = Vec::new();
    for hash in hashes {
        match seen.and_then(|s| s.get(hash)) {
            Some(v) => {
                found.insert(hash.clone(), *v);
            }
            None if !missing.contains(hash) => missing.push(hash.clone()),
            None => {}
        }
    }
    (found, missing)
}
pub(super) fn keep_stats(root: &Path, computed: &BTreeMap<String, (u64, u64, u64)>) {
    let mut stats = STATS.lock().unwrap_or_else(|e| e.into_inner());
    if stats.len() >= REPOSITORIES && !stats.contains_key(root) {
        stats.clear();
    }
    let kept = stats.entry(root.to_path_buf()).or_default();
    if kept.len() + computed.len() > COMMITS {
        kept.clear();
    }
    kept.extend(computed.iter().map(|(k, v)| (k.clone(), *v)));
}
/// True unless `argv` is a command form known to only read: anything else (and
/// anything unknown) ends snapshot reuse when it runs.
pub(super) fn mutating(argv: &[String]) -> bool {
    let a = argv.iter().map(String::as_str).collect::<Vec<_>>();
    let positional = a.iter().skip(1).filter(|s| !s.starts_with('-')).count();
    match a.first().copied().unwrap_or("") {
        "status" | "diff" | "log" | "show" | "ls-files" | "ls-tree" | "check-attr"
        | "check-ignore" | "check-ref-format" | "rev-list" | "rev-parse" | "for-each-ref"
        | "show-ref" | "cat-file" | "merge-base" => false,
        "symbolic-ref" => positional > 1,
        "worktree" => a.get(1) != Some(&"list"),
        "branch" => a.get(1) != Some(&"--show-current"),
        "remote" => !(a.len() == 1 || a.get(1) == Some(&"get-url") || a.get(1) == Some(&"-v")),
        "config" => !a
            .iter()
            .any(|s| matches!(*s, "--get" | "--get-all" | "--get-regexp" | "--list" | "-l")),
        _ => true,
    }
}
#[cfg(test)]
static READS: LazyLock<Mutex<Vec<(PathBuf, Vec<String>, usize)>>> = LazyLock::new(Default::default);
/// Test record of each Git command run below `path`: its argv and stdout bytes read
/// (the bound plus one when the read stopped at the bound).
#[cfg(test)]
pub(super) fn record(path: &Path, argv: &[String], bytes: usize) {
    READS
        .lock()
        .unwrap()
        .push((path.to_path_buf(), argv.to_vec(), bytes));
}
#[cfg(test)]
pub(super) fn reads(below: &Path) -> Vec<(Vec<String>, usize)> {
    READS
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.0.starts_with(below))
        .map(|r| (r.1.clone(), r.2))
        .collect()
}
