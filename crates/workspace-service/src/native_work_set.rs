//! Work-set observation of a command run, so a passed check binds to the work it
//! checked rather than to the whole shared checkout.
//!
//! A run's work set (`worker_checks::work_set_tx`) is its assignment's declared
//! scope plus every path that assignment's own tools wrote before the run started.
//! The host hashes the deliverable files of that set (Git-listed tracked and
//! untracked, not ignored, files under it, plus each literal entry) before launch
//! and again after the process ended and its receipt settled, and retains both
//! (`NATIVE_CHECK_WORK_SET`). At submission it hashes the set again: a stable record
//! with an unchanged digest, and no later write of that assignment outside the set,
//! verifies the pass as `revision_binding: {kind: "work_set"}`. Files outside the
//! set (another task's concurrent work) may differ; that is the binding's limit.
use cortex_core::{
    lifecycle::worker_checks, scheduler::policy, store::executor::StoreExecutor, CoreError,
    CoreResult, Store,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};

/// A work set with more files or bytes than this is not hashed: such a run binds
/// only to the whole checkout.
const FILE_LIMIT: usize = 20_000;
const BYTE_LIMIT: u64 = 256 << 20;

/// The directory under which an entry's files are listed: a literal entry itself,
/// or the directory part before a wildcard. `None` for the whole project.
fn listing_root(entry: &str) -> Option<String> {
    let entry = policy::canonical(entry);
    let root = match entry.find(['*', '?', '[']) {
        None => entry.clone(),
        Some(cut) => {
            let raw = &entry[..cut];
            match raw.strip_suffix('/') {
                Some(dir) => dir.to_owned(),
                None => raw.rsplit_once('/').map(|(d, _)| d.to_owned())?,
            }
        }
    };
    let root = root.trim_end_matches('/').to_owned();
    (!root.is_empty()
        && root != "."
        && !root.starts_with('/')
        && !root.split('/').any(|p| p == ".."))
    .then_some(root)
}
fn listed(root: &Path, roots: &BTreeSet<String>) -> Option<BTreeSet<String>> {
    let mut command = std::process::Command::new("git");
    command
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.quotePath=false",
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
            "--",
        ])
        .args(roots.iter().map(|r| format!(":(literal){r}")))
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let out = command.output().ok().filter(|o| o.status.success())?;
    out.stdout
        .split(|b| *b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| String::from_utf8(p.to_vec()).ok())
        .collect()
}
/// The state of one path: content hash and mode of a regular file, link target,
/// `absent`, or `None` when it cannot be bound (a nested checkout or directory
/// listed as a file, an unreadable entry).
pub(crate) fn state(root: &Path, path: &str, budget: &mut u64) -> Option<String> {
    let at = root.join(path);
    let meta = match at.symlink_metadata() {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some("absent".into()),
        Err(_) => return None,
    };
    let kind = meta.file_type();
    if kind.is_symlink() {
        let target = std::fs::read_link(&at).ok()?;
        return Some(format!("link:{}", target.to_string_lossy()));
    }
    if kind.is_dir() {
        return None;
    }
    if !kind.is_file() {
        return Some(format!("special:{:o}", meta.mode()));
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&at)
        .ok()?;
    let mut hasher = Sha256::new();
    let mut reader = file.take(*budget + 1);
    let mut buffer = [0u8; 65536];
    let mut read = 0u64;
    loop {
        let n = reader.read(&mut buffer).ok()?;
        if n == 0 {
            break;
        }
        read += n as u64;
        hasher.update(&buffer[..n]);
    }
    if read > *budget {
        return None;
    }
    *budget -= read;
    Some(format!(
        "file:{:o}:{:x}",
        meta.mode() & 0o777,
        hasher.finalize()
    ))
}
/// The digest of a work set's current deliverable files, or `None` when the set
/// cannot be bound (the whole project, no Git listing, a nested checkout, past the
/// file or byte bound). Blocking: call it off the async runtime.
pub(crate) fn digest(root: &Path, work_set: &[String]) -> Option<String> {
    let mut roots = BTreeSet::new();
    let mut literal = BTreeSet::new();
    for entry in work_set {
        roots.insert(listing_root(entry)?);
        if !entry.contains(['*', '?', '[']) {
            literal.insert(policy::canonical(entry));
        }
    }
    let mut paths: BTreeSet<String> = listed(root, &roots)?
        .into_iter()
        .filter(|p| worker_checks::in_work_set(p, work_set))
        .collect();
    // A written or literal path the listing does not name (ignored, deleted): its
    // own state, unless it is a directory whose deliverable files are listed.
    for path in literal {
        if !paths.contains(&path) && !root.join(&path).is_dir() {
            paths.insert(path);
        }
    }
    if paths.len() > FILE_LIMIT {
        return None;
    }
    let mut budget = BYTE_LIMIT;
    let mut states = BTreeMap::new();
    for path in paths {
        let state = state(root, &path, &mut budget)?;
        states.insert(path, state);
    }
    let mut hasher = Sha256::new();
    hasher.update(b"cortex-work-set-v1\n");
    for entry in work_set {
        hasher.update(entry.as_bytes());
        hasher.update(b"\n");
    }
    hasher.update(b"\0");
    for (path, state) in &states {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        hasher.update(state.as_bytes());
        hasher.update(b"\n");
    }
    Some(format!("{:x}", hasher.finalize()))
}
pub(crate) async fn observe(root: PathBuf, work_set: Vec<String>) -> Option<String> {
    tokio::task::spawn_blocking(move || digest(&root, &work_set))
        .await
        .ok()
        .flatten()
}
/// After a launched `command_run`, wait until its process ended and its receipt
/// settled, observe the work set again and retain both observations. An unsettled
/// receipt (an unknown outcome, a later re-settle) records nothing: its observation
/// could precede the end of the process.
#[allow(clippy::too_many_arguments)]
pub(crate) fn finish(
    executor: Arc<StoreExecutor>,
    running: crate::native_commands::Running,
    task: String,
    assignment: String,
    reference: String,
    root: PathBuf,
    work_set: Vec<String>,
    start: String,
) {
    tokio::spawn(async move {
        let done = running.lock().await.get(&reference).map(|r| r.done.clone());
        if let Some(mut done) = done {
            if done.wait_for(|d| *d).await.is_err() {
                return;
            }
        }
        let (t, r) = (task.clone(), reference.clone());
        let Ok(settled) = executor.read(move |db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND operation='command_run'
                 AND json_extract(result_json,'$.tool_call_ref')=? AND json_extract(result_json,'$.settled')=1)",
                (&t, &r),
                |row| row.get::<_, bool>(0),
            )?)
        }) else {
            return;
        };
        if !matches!(settled.await, Ok(Ok(true))) {
            return;
        }
        let Some(end) = observe(root, work_set.clone()).await else {
            return;
        };
        if let Ok(written) = executor.write(move |db| {
            worker_checks::record_work_set_tx(
                db,
                &task,
                &reference,
                &assignment,
                &work_set,
                &start,
                &end,
            )
        }) {
            let _ = written.await;
        }
    });
}
/// The verified work-set binding of a cited passed run at submission: its retained
/// observation is stable, belongs to this assignment, the set's files are unchanged
/// now, and this assignment wrote nothing outside the set since the run started.
pub(crate) fn binding_at_submission(
    store: &Store,
    task: &str,
    assignment: &str,
    reference: &str,
    digests: &mut BTreeMap<Vec<String>, Option<String>>,
) -> CoreResult<Option<Value>> {
    let Some(record) = store.read(|db| worker_checks::work_set_record(db, task, reference))? else {
        return Ok(None);
    };
    if record["assignment_ref"] != assignment {
        return Ok(None);
    }
    let work_set: Vec<String> = serde_json::from_value(record["work_set"].clone())
        .map_err(|_| CoreError::Refused("invalid_native_result"))?;
    let started = record["started_event"].as_i64().unwrap_or(i64::MAX);
    if store
        .read(|db| worker_checks::wrote_outside_after(db, task, assignment, started, &work_set))?
    {
        return Ok(None);
    }
    let root = store.key().root().to_path_buf();
    let current = digests
        .entry(work_set.clone())
        .or_insert_with(|| digest(&root, &work_set));
    Ok((current.as_deref() == record["end_digest"].as_str())
        .then(|| worker_checks::work_set_binding(&record)))
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
    #[test]
    fn a_work_set_digest_sees_only_its_own_files() {
        let root = std::env::temp_dir().join(format!(
            "native-work-set-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir_all(root.join("ui/src")).unwrap();
        std::fs::create_dir_all(root.join("core")).unwrap();
        std::fs::write(root.join(".gitignore"), "ui/out/\n").unwrap();
        std::fs::write(root.join("ui/src/a.ts"), "a").unwrap();
        std::fs::write(root.join("core/lib.rs"), "fn x() {}").unwrap();
        std::fs::write(root.join("notes.md"), "n").unwrap();
        git(&root, &["init", "-q"]);
        git(&root, &["add", "."]);
        git(&root, &["commit", "-qm", "base"]);
        let set = vec!["notes.md".to_owned(), "ui/**".to_owned()];
        let d0 = digest(&root, &set).unwrap();
        // Another writer's files and ignored output do not change it.
        std::fs::write(root.join("core/lib.rs"), "fn y() {}").unwrap();
        std::fs::write(root.join("core/new.rs"), "new").unwrap();
        std::fs::create_dir_all(root.join("ui/out")).unwrap();
        std::fs::write(root.join("ui/out/build.js"), "x").unwrap();
        assert_eq!(digest(&root, &set).unwrap(), d0);
        // Its own files do: edits, untracked additions, deletions, mode changes.
        for change in ["edit", "add", "delete", "mode"] {
            match change {
                "edit" => std::fs::write(root.join("ui/src/a.ts"), "b").unwrap(),
                "add" => std::fs::write(root.join("ui/src/b.ts"), "b").unwrap(),
                "delete" => std::fs::remove_file(root.join("notes.md")).unwrap(),
                _ => {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(
                        root.join("ui/src/a.ts"),
                        std::fs::Permissions::from_mode(0o755),
                    )
                    .unwrap()
                }
            }
            let changed = digest(&root, &set).unwrap();
            assert_ne!(changed, d0, "{change}");
        }
        // The same files under another set are a different binding.
        assert_ne!(
            digest(&root, &["ui/**".to_owned()]),
            digest(&root, &["ui".to_owned()])
        );
        // The whole project, or a folder outside any repository, cannot be bound.
        assert_eq!(digest(&root, &["**".to_owned()]), None);
        assert_eq!(digest(&root, &["*.md".to_owned()]), None);
        let plain = root.with_extension("plain");
        std::fs::create_dir_all(plain.join("ui")).unwrap();
        assert_eq!(digest(&plain, &["ui".to_owned()]), None);
        std::fs::remove_dir_all(&root).unwrap();
        std::fs::remove_dir_all(&plain).unwrap();
    }
}
