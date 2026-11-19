//! Read-only owner task facts. No scheduler ownership or task mutation on this path.
use super::{context, git::Git, native::*};
use cortex_core::{CoreError, OpenMode, Store};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{os::unix::ffi::OsStrExt, path::Path};

fn waiting() -> Value {
    json!({"accept":false,"requestChanges":false,"reason":"Wait for Cortex to finish this result.",
        "unblockAction":"wait","acknowledgeOpenChecks":false})
}
fn unavailable() -> Value {
    let mut actions = waiting();
    actions["reason"] = json!("This result binding is unavailable. Refresh or reopen the project before requesting a new result.");
    actions["unblockAction"] = json!("refresh");
    actions
}
/// A closed card: nothing is still being finished, and nothing is offered.
fn closed() -> Value {
    let mut actions = waiting();
    actions["reason"] = json!("This task is closed.");
    actions["unblockAction"] = Value::Null;
    actions
}
/// A stopped task has no result to review.
fn stopped() -> Value {
    let mut actions = waiting();
    actions["reason"] = json!("This task stopped without a result to review.");
    actions["unblockAction"] = Value::Null;
    actions
}
/// A finished result an earlier version of Cortex produced. It is shown read-only:
/// its review and check evidence are not in this version's form, so it is never
/// acceptable here. Request changes reopens the task for a fresh, reviewable result.
/// It keeps the same result identity as any result (the task's latest event), with a
/// placeholder digest that makes no claim about the files.
fn earlier_version(version: i64, execution: &str) -> Value {
    let mut actions = waiting();
    actions["requestChanges"] = json!(true);
    actions["reason"] = json!("This result is from an earlier version of the app and is shown read-only. Request changes for a fresh result you can review and accept.");
    actions["unblockAction"] = json!("request_changes");
    actions["earlierVersion"] = json!(true);
    let id = format!("wr_{execution}");
    actions["digest"] = json!(format!("result:{id}:{version}"));
    actions["resultId"] = json!(id);
    actions["version"] = json!(version.to_string());
    actions
}
/// The result identity (latest event) of a finished task the retired Python host
/// created, or None for a task this host created. A task whose execution is not bound to its
/// current owner epoch is never presented as an earlier result: its binding is invalid.
fn earlier_version_task(store: &Store, execution: &str) -> Option<i64> {
    store
        .read(|db| {
            Ok(db
                .query_row(
                    "SELECT coalesce((SELECT max(id) FROM runtime_events WHERE task_id=e.runtime_task_id),1)
                     FROM workspace_executions e JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
                     WHERE e.execution_id=? AND r.state='COMPLETED'
                     AND EXISTS(SELECT 1 FROM task_owners o WHERE o.task_id=e.runtime_task_id AND o.epoch=e.owner_epoch)
                     AND NOT EXISTS(SELECT 1 FROM runtime_events v WHERE v.task_id=e.runtime_task_id
                        AND v.type='TASK_CREATED' AND json_extract(v.payload_json,'$.execution_host')='rust')",
                    [execution],
                    |r| r.get::<_, i64>(0),
                )
                .optional()?)
        })
        .ok()
        .flatten()
}
/// Legacy task options may name a worktree checkout without a path. The product
/// projection resolves it from the project's real worktrees (the old transport
/// name or the `cw/<slug>-<hash>` branch); a task with no such worktree keeps the
/// transport-name path, which is only a prospective location that nothing here
/// creates or deletes. An ambiguous match is refused by `context::product`.
pub(super) fn materialized(root: &Path, _project: &str) -> Result<()> {
    // Opening still enforces the project/store binding (`project_storage_mismatch`)
    // that callers translate before any effect.
    context::open(root, OpenMode::LiveReadOnly)?;
    Ok(())
}
/// The native verifier's digest domain for the project folder. None is an
/// unobservable workspace, never a pass.
/// A fresh `native_effects::observe_digest` of `root`. Acceptance, automatic
/// acceptance and integration decide on this; presentation reads use
/// `recent_observation`.
pub(crate) async fn native_observation(root: &Path) -> Option<String> {
    crate::native_effects::observe_digest(root.to_owned())
        .await
        .ok()
}
/// `recent_observation` for presentation reads only.
pub(crate) async fn presented_observation(root: &Path) -> Option<String> {
    recent_observation(root.to_owned()).await.ok()
}
/// How long a presentation read may reuse a folder observation whose change signal
/// (`folder_signal`) has not moved: review actions in a product read are at most this old.
pub(super) const RECENT: std::time::Duration = std::time::Duration::from_secs(30);
/// One reused observation: the change signal taken before it, and when.
struct Recent {
    signal: String,
    digest: String,
    at: std::time::Instant,
}
type Slots = std::collections::HashMap<
    std::path::PathBuf,
    std::sync::Arc<tokio::sync::Mutex<Option<Recent>>>,
>;
static OBSERVED: std::sync::LazyLock<std::sync::Mutex<Slots>> =
    std::sync::LazyLock::new(Default::default);
/// `native_effects::observe_digest` of `root` for presentation reads (product facts and
/// review actions). Reads of one folder share one observation at a time, and a read
/// reuses the last one while the folder's change signal is unchanged and it is younger
/// than `RECENT`: an open window rereads facts after every streamed update, and
/// hashing a large folder each time kept the machine busy while nothing changed.
/// Accepting a result and the native verifier never use this; they observe fresh.
pub(crate) async fn recent_observation(
    root: std::path::PathBuf,
) -> cortex_core::CoreResult<String> {
    let slot = OBSERVED
        .lock()
        .map_err(|_| CoreError::Refused("project_revision_unavailable"))?
        .entry(root.clone())
        .or_default()
        .clone();
    let mut held = slot.lock().await;
    let at = std::time::Instant::now();
    let probe = root.clone();
    let signal = tokio::task::spawn_blocking(move || folder_signal(&probe))
        .await
        .ok()
        .flatten();
    if let (Some(signal), Some(seen)) = (&signal, held.as_ref()) {
        if seen.signal == *signal && seen.at.elapsed() < RECENT {
            return Ok(seen.digest.clone());
        }
    }
    *held = None;
    let digest = crate::native_effects::observe_digest(root).await?;
    // The signal was taken first: a change during the observation moves the next one.
    *held = signal.map(|signal| Recent {
        signal,
        digest: digest.clone(),
        at,
    });
    Ok(digest)
}
/// A cheap change signal of one Git working folder: HEAD, the index entries, the
/// identity, size and change times of every tracked and unignored untracked file (the
/// private stores excluded), and the same metadata of every directory holding such a
/// file, of the folder itself and of each untracked directory Git lists as a whole
/// (`--directory`, which includes empty ones). Creating or removing an empty directory
/// changes the change time of the directory holding it, so it moves the signal. One
/// bound remains: an empty directory created or removed inside a directory that holds
/// only empty directories (no file Git lists, and not itself listed as a whole) is not
/// seen, so a presentation read can reuse the older digest for up to `RECENT`.
/// Acceptance and integration never reuse it (`native_observation` is fresh).
/// None, so never reused, for a folder without Git, a nested checkout or repository,
/// more than 100,000 paths, or any file or directory changed in the last two seconds
/// (a timestamp that coarse could hide a second write).
pub(super) fn folder_signal(root: &Path) -> Option<String> {
    use sha2::Digest;
    use std::os::unix::fs::MetadataExt;
    const EXCLUDE: [&str; 4] = [
        ":(exclude).cortex",
        ":(exclude).cortex-dev",
        ":(exclude).cortex-dev-owner.json",
        ":(exclude).cortex-workspace-dev.json",
    ];
    let head = signal_git(root, &["rev-parse", "HEAD", "--symbolic-full-name", "HEAD"])?;
    let mut stage = vec!["ls-files", "--stage", "-z", "--", "."];
    stage.extend(EXCLUDE);
    let index = signal_git(root, &stage)?;
    let mut others = vec![
        "ls-files",
        "--others",
        "--exclude-standard",
        "-z",
        "--",
        ".",
    ];
    others.extend(EXCLUDE);
    let untracked = signal_git(root, &others)?;
    let mut whole = vec![
        "ls-files",
        "--others",
        "--exclude-standard",
        "--directory",
        "-z",
        "--",
        ".",
    ];
    whole.extend(EXCLUDE);
    let listed_dirs = signal_git(root, &whole)?;
    let mut h = sha2::Sha256::new();
    h.update(&head);
    h.update(b"\0index\0");
    h.update(&index);
    h.update(b"\0untracked\0");
    h.update(&untracked);
    let names = index
        .split(|b| *b == 0)
        .filter_map(|e| e.splitn(2, |b| *b == b'\t').nth(1))
        .chain(untracked.split(|b| *b == 0))
        .filter(|n| !n.is_empty());
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    let mut count = 0usize;
    // The folder, every directory holding a listed file and each directory Git lists
    // as a whole: an entry created or removed in one changes its change time.
    let mut dirs: std::collections::BTreeSet<&[u8]> = [&b""[..]].into();
    dirs.extend(
        listed_dirs
            .split(|b| *b == 0)
            .filter_map(|d| d.strip_suffix(b"/"))
            .filter(|d| !d.is_empty()),
    );
    for name in names {
        count += 1;
        if count > 100_000 {
            return None;
        }
        let mut parent = name;
        while let Some(cut) = parent.iter().rposition(|b| *b == b'/') {
            parent = &parent[..cut];
            if !dirs.insert(parent) {
                break;
            }
        }
        let path = root.join(std::ffi::OsStr::from_bytes(name));
        h.update(name);
        match std::fs::symlink_metadata(&path) {
            Ok(m) => {
                if m.is_dir() || m.mtime() >= now - 2 || m.ctime() >= now - 2 {
                    return None;
                }
                for v in [m.dev(), m.ino(), m.mode() as u64, m.size()] {
                    h.update(v.to_be_bytes());
                }
                for v in [m.mtime(), m.mtime_nsec(), m.ctime(), m.ctime_nsec()] {
                    h.update(v.to_be_bytes());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => h.update(b"missing"),
            Err(_) => return None,
        }
    }
    h.update(b"\0directories\0");
    for dir in dirs {
        let m = std::fs::symlink_metadata(root.join(std::ffi::OsStr::from_bytes(dir))).ok()?;
        if m.mtime() >= now - 2 || m.ctime() >= now - 2 {
            return None;
        }
        h.update(dir);
        h.update(b"\0");
        for v in [
            m.ino(),
            m.mtime() as u64,
            m.mtime_nsec() as u64,
            m.ctime() as u64,
            m.ctime_nsec() as u64,
        ] {
            h.update(v.to_be_bytes());
        }
    }
    Some(format!("{:x}", h.finalize()))
}
/// One bounded read-only Git listing for `folder_signal`: no hooks, filters, file
/// monitor, optional locks or global configuration; None unless it completes in time.
pub(super) fn signal_git(root: &Path, args: &[&str]) -> Option<Vec<u8>> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    const LIMIT: u64 = 32 * 1024 * 1024;
    let mut child = Command::new("/usr/bin/git")
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=/dev/null",
        ])
        .args(args)
        .current_dir(root)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", "/nonexistent")
        .env("LANG", "C.UTF-8")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut data = Vec::new();
        (&mut stdout)
            .take(LIMIT + 1)
            .read_to_end(&mut data)
            .ok()
            .map(|_| data)
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(5))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let data = reader.join().ok().flatten()?;
    (status?.success() && (data.len() as u64) <= LIMIT).then_some(data)
}
/// Review actions of one saved task record, in Python `_facts` order, and whether
/// its result is work rather than an informational answer.
fn record_actions(
    store: &Store,
    root: &Path,
    project: &str,
    execution: &str,
    status: Option<&str>,
    record: &Value,
    native: Option<&str>,
) -> Result<(Value, bool)> {
    let mut actions = waiting();
    let mut work = true;
    if record["status"] == "done" {
        actions = closed();
    } else if status == Some("CANCELLED") {
        actions = stopped();
    } else if status == Some("FAILED") {
        // An owner message reopens a failed task for a fresh plan.
        actions = stopped();
        actions["requestChanges"] = json!(true);
        actions["reason"] =
            json!("This task failed before it produced a result. Request changes to try again.");
        actions["unblockAction"] = json!("request_changes");
        // Outside a queue the owner decides. In Hybrid and Automatic the scheduler
        // sends the run back to its agent first and holds the queue with its own decision.
        actions["nextActor"] = json!("owner");
        actions["ownerActions"] = json!(["ask_agent", "close_as_is"]);
        actions["asks"] = json!(["Ask the agent to try again. It continues from the recorded state, and review and verification run again before the task can finish."]);
    } else if status == Some("COMPLETED") {
        actions = match result_evidence(store, root, project, execution, native, own_writes(record))
        {
            Ok((value, evidence, ..)) => {
                work = evidence;
                value
            }
            Err(_) => match earlier_version_task(store, execution) {
                Some(version) => earlier_version(version, execution),
                None => unavailable(),
            },
        };
        if record["requiresFreshResult"]
            == json!({"resultId":actions["resultId"],"version":actions["version"].as_str().unwrap_or("")})
        {
            actions["accept"] = json!(false);
            actions["reason"] = json!("The source changed after this result. Request changes for fresh checks and review.");
            actions["unblockAction"] = json!("request_changes");
        }
    }
    Ok((actions, work))
}
/// The native owner's read has no retained provider: the same review actions as
/// `read`, and whether the result is work (not an answer). A fact the port does
/// not cover yet makes that result unavailable for acceptance, never accepted.
pub(crate) fn native_review(
    root: &Path,
    project: &str,
    execution: &str,
    status: &str,
    record: &Value,
    native: Option<&str>,
) -> (Value, bool) {
    let Ok(Some(store)) = context::open(root, OpenMode::LiveReadOnly) else {
        return (unavailable(), true);
    };
    record_actions(
        &store,
        root,
        project,
        execution,
        Some(status),
        record,
        native,
    )
    .unwrap_or_else(|_| (unavailable(), true))
}
/// `native_review` of each completed `(execution, record)` of one read, in order. The
/// reviews share one observation of the project folder per read (see
/// `legacy_source::memoized`) instead of walking it twice per result, and reuse a
/// recent one of an unchanged folder (`legacy_source::memoized_recent`).
pub(crate) fn native_reviews(
    root: &Path,
    project: &str,
    completed: &[(String, Value)],
    native: Option<&str>,
) -> Vec<(Value, bool)> {
    super::legacy_source::memoized_recent(|| {
        completed
            .iter()
            .map(|(execution, record)| {
                native_review(root, project, execution, "COMPLETED", record, native)
            })
            .collect()
    })
}
pub async fn read(root: &Path, state: &Path, project: &str, git: &mut Git) -> Result<Value> {
    materialized(root, project)?;
    let mut product = context::product(root, state, project, git).await?;
    if let Some(store) = context::open(root, OpenMode::LiveReadOnly)? {
        let rows = store.read(|db| {
            let mut q = db.prepare("SELECT t.task_id,t.version,e.execution_id,r.state FROM workspace_tasks t
                JOIN workspace_executions e ON e.execution_id=t.execution_id
                LEFT JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE t.project_id=? ORDER BY t.sequence LIMIT 10001")?;
            let rows = q.query_map([project], |r| Ok((r.get::<_,String>(0)?,r.get::<_,i64>(1)?,
                r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?)))?.collect::<std::result::Result<Vec<_>,_>>()?;
            if rows.len()>10000 {return Err(CoreError::Refused("machine_reply_too_large"));}
            Ok(rows)
        }).map_err(context::core)?;
        let native = if rows.iter().any(|r| r.3.as_deref() == Some("COMPLETED")) {
            presented_observation(root).await
        } else {
            None
        };
        for (task, _version, execution, status) in rows {
            let record = &mut product["tasks"][&task];
            if !record.is_object() {
                *record = json!({});
            }
            let (mut actions, _) = record_actions(
                &store,
                root,
                project,
                &execution,
                status.as_deref(),
                record,
                native.as_deref(),
            )?;
            // Native-only presentation facts; the saved-state read keeps its recorded shape.
            if let Some(map) = actions.as_object_mut() {
                map.remove("workspaceChanged");
                map.remove("reviewWarnings");
                map.remove("notRunChecks");
                map.remove("ownWritesOnly");
            }
            record["reviewActions"] = actions;
        }
    }
    integration_facts(root, &mut product, git).await?;
    if serde_json::to_vec(&product)
        .map_err(|_| refuse("state_unreadable", "Invalid state."))?
        .len()
        > FRAME_BUDGET
    {
        return Err(refuse(
            "machine_reply_too_large",
            "Project facts exceed the reply limit.",
        ));
    }
    Ok(product)
}
/// Local integration facts of every task waiting in Ready to merge.
/// Reading never promotes a conflict or marks a task done.
async fn integration_facts(root: &Path, product: &mut Value, git: &mut Git) -> Result<()> {
    let tasks = product["tasks"]
        .as_object()
        .ok_or_else(|| refuse("state_unreadable", "Saved tasks are unreadable."))?
        .iter()
        .filter(|(_, v)| v["status"] == "ready_to_merge")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect::<Vec<_>>();
    for (task, record) in tasks {
        product["tasks"][&task]["integrationFacts"] =
            super::task_facts_git::read(root, &record, &task, git).await?;
    }
    Ok(())
}
/// The saved task records the native owner read starts from: the private overlay
/// with each task's store-owned options (checkout and its current branch, settings,
/// base revision, a saved draft title) and the local integration facts of tasks in
/// Ready to merge. A task created since the overlay was last written is presented
/// from its store options, never as a record without a checkout.
#[cfg(test)]
pub(crate) async fn native_saved(
    root: &Path,
    state: &Path,
    project: &str,
    git: &mut Git,
) -> Result<Value> {
    materialized(root, project)?;
    let mut product = context::product(root, state, project, git).await?;
    integration_facts(root, &mut product, git).await?;
    Ok(product)
}
/// `native_saved` for the service's product reads: the Ready-to-merge integration facts
/// come from one background inspection per folder (`task_facts_presented`). With
/// `defer` the read never waits for it; without, it answers as `native_saved` does.
pub(crate) async fn native_presented(
    root: &Path,
    state: &Path,
    project: &str,
    git: &mut Git,
    defer: bool,
) -> Result<Value> {
    materialized(root, project)?;
    let mut product = context::product(root, state, project, git).await?;
    super::task_facts_presented::apply(root, &mut product, git, defer).await?;
    Ok(product)
}
pub(super) fn safe_bytes(root: &Path, name: &str, limit: usize) -> Result<Vec<u8>> {
    use std::{
        ffi::CString,
        fs::File,
        io::Read,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::MetadataExt,
        },
    };
    if name.is_empty()
        || name
            .split('/')
            .any(|v| v.is_empty() || v == "." || v == "..")
    {
        return Err(refuse("invalid_path", "Invalid evidence path."));
    }
    let mut folder = File::open(root)?;
    let mut leaf = None;
    let parts = name.split('/').collect::<Vec<_>>();
    for (i, part) in parts.iter().enumerate() {
        let name =
            CString::new(*part).map_err(|_| refuse("invalid_path", "Invalid evidence path."))?;
        let fd = unsafe {
            libc::openat(
                folder.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY
                    | libc::O_CLOEXEC
                    | libc::O_NOFOLLOW
                    | libc::O_NONBLOCK
                    | if i + 1 == parts.len() {
                        0
                    } else {
                        libc::O_DIRECTORY
                    },
            )
        };
        if fd < 0 {
            return Err(refuse(
                "evidence_unavailable",
                "Evidence could not be read without following links.",
            ));
        }
        if i + 1 == parts.len() {
            leaf = Some((folder.try_clone()?, name));
        }
        folder = unsafe { File::from_raw_fd(fd) };
    }
    let before = folder.metadata()?;
    if !before.is_file() || before.len() > limit as u64 {
        return Err(refuse(
            "evidence_unavailable",
            "Evidence exceeds its bound.",
        ));
    }
    let mut data = Vec::new();
    (&mut folder)
        .take(limit as u64 + 1)
        .read_to_end(&mut data)?;
    let after = folder.metadata()?;
    let identity = |m: &std::fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.mode(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    let (parent, name) = leaf.expect("validated nonempty path");
    let current_fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if current_fd < 0 {
        return Err(refuse(
            "evidence_unavailable",
            "Evidence changed during reading.",
        ));
    }
    let current = unsafe { File::from_raw_fd(current_fd) };
    if data.len() > limit
        || identity(&before) != identity(&after)
        || identity(&after) != identity(&current.metadata()?)
    {
        return Err(refuse(
            "evidence_unavailable",
            "Evidence changed during reading.",
        ));
    }
    Ok(data)
}
pub(super) fn integration_result(
    store: &Store,
    root: &Path,
    project: &str,
    execution: &str,
    wanted: &Value,
    native: Option<&str>,
    record: &Value,
    automatic: bool,
) -> Result<Value> {
    let (actions, work, warnings, counts) =
        result_evidence(store, root, project, execution, native, own_writes(record))?;
    if actions["digest"] != *wanted
        || warnings
        || (automatic && actions["version"] != record["acceptance"]["version"])
    {
        return Err(refuse(
            "stale_result",
            "This result or its workspace changed. Rerun checks and review the new result.",
        ));
    }
    if !work {
        return Err(refuse(
            "checks_required",
            "This is an informational answer, not verified code for merging.",
        ));
    }
    let owner_consented_open_checks = automatic
        && record["acceptance"]["finishConsent"].is_object()
        && record["acceptance"]["open"]["failed"] == counts[0]
        && record["acceptance"]["open"]["not_run"] == counts[1]
        && record["acceptance"]["open"]["unverified"] == counts[2];
    if actions["accept"] != true
        || (automatic && counts != [0, 0, 0] && !owner_consented_open_checks)
    {
        return Err(refuse(
            "checks_required",
            actions["reason"]
                .as_str()
                .unwrap_or("Result evidence is unavailable."),
        ));
    }
    Ok(actions)
}
/// A task of a sprint that works in the project folder itself: the tasks after it
/// write the same folder, so its result is compared on the files it wrote.
pub(super) fn own_writes(record: &Value) -> bool {
    record["sprintId"].as_str().is_some_and(|s| !s.is_empty())
        && record["checkout"]["kind"] == "main"
}
fn result_evidence(
    store: &Store,
    root: &Path,
    project: &str,
    execution: &str,
    native: Option<&str>,
    own_writes: bool,
) -> Result<(Value, bool, bool, [u64; 3])> {
    store
        .read(|db| {
            result_evidence_db(
                db, store, root, project, execution, native, own_writes, false,
            )
        })
        .map_err(context::core)
}
#[allow(clippy::too_many_arguments)]
pub(super) fn result_evidence_db(
    db: &rusqlite::Connection,
    store: &Store,
    root: &Path,
    project: &str,
    execution: &str,
    native: Option<&str>,
    own_writes: bool,
    automatic: bool,
) -> cortex_core::CoreResult<(Value, bool, bool, [u64; 3])> {
    let row:Option<(String,String,Option<String>,i64)>=db.query_row(
            "SELECT r.task_id,r.state,r.synthesis,coalesce((SELECT max(id) FROM runtime_events WHERE task_id=r.task_id),1)
            FROM workspace_tasks t JOIN workspace_executions e USING(task_id)
            JOIN runtime_tasks r ON r.task_id=e.runtime_task_id JOIN tasks base ON base.id=r.task_id
            JOIN acp_host_sessions s ON s.session_id=e.acp_session_id
            JOIN task_owners o ON o.task_id=r.task_id AND o.epoch=e.owner_epoch
            WHERE t.project_id=? AND e.execution_id=? AND e.execution_id=t.execution_id AND base.project_root=? AND s.project_root=base.project_root
            AND s.task_id=r.task_id AND r.revision>=0 AND (r.blocker IS NULL OR r.blocker='')
            AND NOT EXISTS(SELECT 1 FROM workspace_starts WHERE execution_id=e.execution_id AND state='unknown')",
            (project,execution,root.to_string_lossy().as_ref()),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let (task, status, synthesis, version) = row.ok_or(CoreError::Refused("result_unavailable"))?;
    if synthesis.as_deref().is_none_or(str::is_empty) {
        return Err(CoreError::Refused("result_unavailable"));
    }
    // The legacy projection withholds a result text beyond 32,000 characters with a
    // content-limit warning; keep that visible unavailability and never accept it.
    let oversized = synthesis
        .as_deref()
        .is_some_and(|s| s.chars().count() > 32000);
    let revision: i64 = db.query_row(
        "SELECT revision FROM runtime_tasks WHERE task_id=?",
        [&task],
        |r| r.get(0),
    )?;
    let mut evidence =
        super::task_facts_work::evaluate(db, store, &task, revision, native, own_writes)?;
    evidence.warnings |= oversized;
    let answer: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='ANSWER_SUBMITTED')",
        [&task],
        |r| r.get(0),
    )?;
    // Which open checks keep Accept off, named, so the card says what blocks.
    let named_blockers = (!evidence.blocking.is_empty()).then(|| {
        let shown = evidence.blocking.iter().take(3).cloned().collect::<Vec<_>>().join(" ");
        let more = evidence.blocking.len().saturating_sub(3);
        format!(
            "{shown}{} Rerun {} or request changes. Only a recorded technical blocker may be acknowledged.",
            if more > 0 { format!(" And {more} more.") } else { String::new() },
            if evidence.blocking.len() == 1 { "that check" } else { "those checks" },
        )
    });
    let acknowledgement = (automatic && evidence.acknowledge).then(|| {
        let items = evidence
            .blocking
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        format!("{items} Open the task to review the recorded checks or close it as is.")
            .trim()
            .to_owned()
    });
    let (reason, unblock) = if status != "COMPLETED" {
        (Some("Wait for Cortex to finish this result."), Some("wait"))
    } else if crate::native_owner_controls::receipts_partial(db, &task)? {
        // The receipt set is larger than one read presents; Accept is never decided from part of it.
        (
            Some(crate::native_owner_controls::RECEIPTS_PARTIAL_REASON),
            Some("request_changes"),
        )
    } else if evidence.warnings {
        (Some("Result evidence is unavailable or the files changed. Request changes and run fresh checks."),Some("request_changes"))
    } else if evidence.work && !evidence.approved {
        (
            Some("Independent review is not approved. Request changes to complete review."),
            Some("request_changes"),
        )
    } else if evidence.work && (!evidence.verified || evidence.observed.is_none()) {
        (
            Some(
                if automatic {
                    named_blockers.as_deref()
                } else {
                    None
                }
                .unwrap_or("Run the current runtime verification checks before accepting work."),
            ),
            Some("rerun_checks"),
        )
    } else if !evidence.work && !answer {
        (
            Some("This result has no recorded answer or work evidence. Request a new result."),
            Some("request_changes"),
        )
    } else if evidence.unexplained {
        (Some(named_blockers.as_deref().unwrap_or("Run the unpassed checks first, or request changes. Only recorded technical blockers may be acknowledged.")),Some("rerun_checks"))
    } else if let Some(message) = acknowledgement.as_deref() {
        (Some(message), None)
    } else {
        (None, None)
    };
    let id = format!("wr_{execution}");
    let (drifted, scoped, notes) = (
        evidence.drifted,
        evidence.scoped,
        std::mem::take(&mut evidence.review_warnings),
    );
    let mut actions = (
        json!({"accept":reason.is_none(),"requestChanges":status=="COMPLETED","reason":reason,"unblockAction":unblock,
            "acknowledgeOpenChecks":evidence.acknowledge,"resultId":id,"version":version.to_string(),"digest":evidence.observed.unwrap_or_else(||format!("result:{id}:{version}"))}),
        evidence.work,
        evidence.warnings,
        evidence.open_counts,
    );
    // Present only when they apply, so an ordinary result keeps its recorded shape.
    if drifted {
        actions.0["workspaceChanged"] = json!(true);
    }
    // The folder changed only outside this task's own recorded writes: the
    // presentation compares its result with those files, not the whole folder.
    if scoped {
        actions.0["ownWritesOnly"] = json!(true);
    }
    if !notes.is_empty() {
        actions.0["reviewWarnings"] = json!(notes);
    }
    if !evidence.not_run_checks.is_empty() {
        actions.0["notRunChecks"] = json!(evidence.not_run_checks);
    }
    Ok(actions)
}
#[cfg(test)]
#[path = "owner_facts_perf.rs"]
mod owner_facts_perf;
#[cfg(test)]
mod recent_tests {
    use super::*;
    fn git(root: &Path, a: &[&str]) {
        assert!(std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid"])
            .args(a)
            .current_dir(root)
            .output()
            .unwrap()
            .status
            .success());
    }
    fn settle() {
        std::thread::sleep(std::time::Duration::from_millis(3100));
    }
    /// Acceptance and integration observe fresh even right after a cached presentation
    /// read, and the reuse signal itself moves when an empty directory appears.
    #[tokio::test]
    async fn an_empty_directory_after_a_cached_read_is_seen_by_acceptance_and_the_signal() {
        let root =
            std::env::temp_dir().join(format!("cw-fresh-{}", super::super::settings::unique()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("src/a.txt"), "a\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "base"]);
        settle();
        let cached = recent_observation(root.clone()).await.unwrap();
        let signal = folder_signal(&root).unwrap();
        for (n, empty) in ["empty", "src/nested-empty"].into_iter().enumerate() {
            std::fs::create_dir(root.join(empty)).unwrap();
            // The decision path never reuses: it sees the new directory at once.
            let fresh = native_observation(&root).await.unwrap();
            assert_ne!(fresh, cached, "{empty}");
            assert_eq!(
                fresh,
                crate::native_effects::observe_digest(root.clone())
                    .await
                    .unwrap()
            );
            settle();
            // The presentation signal moved too, so the cached digest is not reused.
            assert_ne!(
                folder_signal(&root).as_deref(),
                Some(signal.as_str()),
                "{empty}"
            );
            assert_eq!(
                recent_observation(root.clone()).await.unwrap(),
                fresh,
                "{empty} {n}"
            );
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
    /// A presentation observation is reused only while the folder's change signal holds:
    /// a fresh write is never trusted, and any content change gives a new digest.
    #[tokio::test]
    async fn recent_observation_reuses_only_an_unchanged_folder() {
        let root =
            std::env::temp_dir().join(format!("cw-recent-{}", super::super::settings::unique()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        git(&root, &["init", "-q", "-b", "main"]);
        std::fs::write(root.join("src/a.txt"), "a\n").unwrap();
        git(&root, &["add", "."]);
        git(&root, &["commit", "-q", "-m", "init"]);
        std::fs::write(root.join("loose.txt"), "u\n").unwrap();
        // Written moments ago: too recent to vouch for its content.
        assert_eq!(folder_signal(&root), None);
        settle();
        let signal = folder_signal(&root).expect("a settled folder has a signal");
        assert_eq!(folder_signal(&root).as_deref(), Some(signal.as_str()));
        let first = recent_observation(root.clone()).await.unwrap();
        assert_eq!(
            first,
            crate::native_effects::observe_digest(root.clone())
                .await
                .unwrap()
        );
        assert_eq!(recent_observation(root.clone()).await.unwrap(), first);
        // A rewrite of an untracked file is a change, even before it settles.
        std::fs::write(root.join("loose.txt"), "v\n").unwrap();
        let changed = recent_observation(root.clone()).await.unwrap();
        assert_ne!(changed, first);
        assert_eq!(
            changed,
            crate::native_effects::observe_digest(root.clone())
                .await
                .unwrap()
        );
        settle();
        assert_ne!(folder_signal(&root).as_deref(), Some(signal.as_str()));
        // A staged change moves the signal as well.
        git(&root, &["add", "loose.txt"]);
        settle();
        let staged = folder_signal(&root).unwrap();
        std::fs::remove_file(root.join("loose.txt")).unwrap();
        assert_ne!(folder_signal(&root).as_deref(), Some(staged.as_str()));
        // No Git: never reused.
        let plain = root.join("src");
        std::fs::remove_dir_all(root.join(".git")).unwrap();
        assert_eq!(folder_signal(&plain), None);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
