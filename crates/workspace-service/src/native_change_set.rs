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

/// The change record of a command from its listings before launch and after its
/// process ended.
pub(crate) fn record(before: &Snapshot, after: &Snapshot) -> Value {
    let (before, after) = match (before, after) {
        (Ok(before), Ok(after)) => (before, after),
        (Err(reason), _) | (_, Err(reason)) => return incomplete(reason),
    };
    let changed: Vec<&String> = before
        .keys()
        .chain(after.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| before.get(*path) != after.get(*path))
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
