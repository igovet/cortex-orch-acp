//! Finish or abort retained merges. Intent persistence precedes Git; uncertain effects never delegate.
use super::{
    git::Git,
    ledger,
    native::*,
    retention,
    settings::{self, Lock},
    workers::truthy,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const IN_PROGRESS: &str = "This task's merge is being finished or aborted. Wait for it to finish.";
/// The merge fields of this task's record, plus the Git facts the read added (kept as
/// the retained overlay did), into the saved product as it is now. Never gives up
/// waiting for the machine lock: the Git effect already happened.
fn save(state: &Path, project: &str, task: &str, product: &Value) -> Result<()> {
    settings::save_task(
        state,
        project,
        task,
        &product["tasks"][task],
        &[
            "conflict",
            "merge",
            "acceptance",
            "resolvedBy",
            "status",
            "requiresFreshResult",
            "changesBaseRevision",
            "automaticReady",
        ],
    )?;
    let _lock = Lock::wait(state, "machine.lock")?;
    let mut saved = super::native::product(state, project)?;
    let mut changed = false;
    for key in ["hasGit", "ghAvailable"] {
        if let Some(value) = product.get(key).filter(|v| saved.get(key) != Some(*v)) {
            saved[key] = value.clone();
            changed = true;
        }
    }
    if changed {
        settings::write_json(&product_path(state, project)?, &saved)?;
    }
    Ok(())
}
pub(super) const FRESH_CHECKS_REASON: &str =
    "This merge needs a fresh independent review and runtime verification. Request fresh checks for the resolved revision.";

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Observed {
    Stopped,
    Completed(String),
    AtSource,
    Other,
}

pub(super) async fn checkout(
    root: &Path,
    project: &str,
    task: &str,
    record: &Value,
    git: &mut Git,
) -> Result<PathBuf> {
    match super::context::owned_checkout(root, project, task)? {
        Some(worktree) => Ok(worktree.path),
        None => retention::task_path(root, record, git).await,
    }
}

pub(super) fn manual_ready(root: &Path, project: &str, task: &str, record: &Value) -> Result<()> {
    if !record["merge"]["conflictId"].is_string() {
        return Ok(());
    }
    require_settled(root, project, task)
}

fn require_settled(root: &Path, project: &str, task: &str) -> Result<()> {
    use rusqlite::OptionalExtension;
    let Some(store) = super::context::open(root, cortex_core::OpenMode::LiveReadOnly)? else {
        return Ok(());
    };
    store
        .read(|db| {
            let runtime: Option<String> = db
                .query_row(
                    "SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e
             ON w.execution_id=e.execution_id AND w.task_id=e.task_id
             WHERE w.project_id=? AND w.task_id=?",
                    (project, task),
                    |row| row.get(0),
                )
                .optional()?;
            if let Some(runtime) = runtime {
                super::task_lifecycle::settled(db, root, project, task)?;
                cortex_core::lifecycle::automatic::require_settled_execution(db, &runtime)?;
            }
            Ok(())
        })
        .map_err(super::task_lifecycle::settle_error)
}

pub(super) fn manual_blocked(record: &Value) -> bool {
    record["conflict"] == "resolving"
        || record["merge"]["gitState"] == "unknown"
        || matches!(
            record["merge"]["handoff"]["state"].as_str(),
            Some("dispatch_unknown" | "active" | "unknown")
        )
}

pub(super) async fn observe(path: &Path, merge: &Value, git: &mut Git) -> Result<Observed> {
    let (Some(source), Some(target), Some(branch)) = (
        merge["sourceRevision"].as_str(),
        merge["targetRevision"].as_str(),
        merge["into"].as_str(),
    ) else {
        return Ok(Observed::Other);
    };
    if git.out(path, &["branch", "--show-current"]).await?.trim() != branch {
        return Ok(Observed::Other);
    }
    let head = git.out(path, &["rev-parse", "--verify", "HEAD"]).await?;
    let head = head.trim();
    let (marker, other_operation) = git.merge_marker(path).await?;
    if other_operation {
        return Ok(Observed::Other);
    }
    let present = match tokio::fs::symlink_metadata(&marker).await {
        Ok(metadata) if metadata.is_file() => true,
        Ok(_) => return Ok(Observed::Other),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    if present {
        // Unlike rev-parse MERGE_HEAD this rejects a multi-parent (octopus) merge.
        use tokio::io::AsyncReadExt;
        let mut contents = Vec::new();
        tokio::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(marker)
            .await?
            .take(256)
            .read_to_end(&mut contents)
            .await?;
        if head == source
            && (contents == target.as_bytes()
                || contents.strip_suffix(b"\n") == Some(target.as_bytes()))
        {
            return Ok(Observed::Stopped);
        }
    } else if !super::integration::conflicts::has_unmerged(path, git).await? {
        if head == source {
            return Ok(Observed::AtSource);
        }
        let parents = git.out(path, &["show", "-s", "--format=%P", head]).await?;
        if parents.split_whitespace().eq([source, target]) {
            return Ok(Observed::Completed(head.to_owned()));
        }
    }
    Ok(Observed::Other)
}

/// An old record lacks a trustworthy merge identity. Only the owner can finish or
/// abort it in Git. This action confirms that no Git operation remains in the
/// original branch and forgets the overlay; it never adopts current Git identities.
async fn require_legacy_merge_closed(path: &Path, merge: &Value, git: &mut Git) -> Result<()> {
    let branch = merge["into"]
        .as_str()
        .filter(|branch| !branch.is_empty())
        .ok_or_else(unsafe_merge)?;
    if git.out(path, &["branch", "--show-current"]).await?.trim() != branch {
        return Err(refuse(
            "unsafe_conflict",
            "Open the saved task branch in its checkout before clearing the old merge record.",
        ));
    }
    git.out(path, &["rev-parse", "--verify", "HEAD"]).await?;
    let (marker, other_operation) = git.merge_marker(path).await?;
    let marker_absent = match tokio::fs::symlink_metadata(marker).await {
        Ok(_) => false,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(error) => return Err(error.into()),
    };
    if !marker_absent
        || other_operation
        || super::integration::conflicts::has_unmerged(path, git).await?
    {
        return Err(refuse(
            "legacy_merge_still_open",
            "A Git operation or unmerged entries remain. Inspect the task checkout in Git and finish or abort the operation there before clearing this old merge record.",
        ));
    }
    Ok(())
}

/// Exact abort can reuse evidence only if deliverable bytes and the staged index
/// both match. Git reporting the same HEAD alone is not enough.
pub(super) async fn abort_snapshot(path: &Path, git: &mut Git) -> Result<Value> {
    let source = path.to_owned();
    let source = tokio::task::spawn_blocking(move || super::legacy_source::capture(&source))
        .await
        .ok()
        .and_then(|r| r.ok())
        .flatten();
    let Some(source) = source else {
        return Ok(Value::Null);
    };
    Ok(
        match git.bytes(path, &["ls-files", "--stage", "-z"]).await {
            Ok(index) => json!({"source":source,"index":format!("{:x}", Sha256::digest(index))}),
            Err(_) => Value::Null,
        },
    )
}

pub(super) async fn exact_abort(path: &Path, merge: &Value, git: &mut Git) -> Result<bool> {
    let before = &merge["before"]["abortSnapshot"];
    Ok(before.is_object() && *before == abort_snapshot(path, git).await?)
}

pub(super) fn clear_abort(record: &mut Value, exact: bool, after: Value) {
    if exact {
        record["requiresFreshResult"] = record["merge"]["before"]["requiresFreshResult"].clone();
    } else if after.is_object() {
        record["requiresFreshResult"]["after"] = after;
    }
    record["conflict"] = json!("none");
    record["merge"] = Value::Null;
    record["acceptance"] = Value::Null;
    record["automaticReady"] = json!(false);
}

pub(super) fn result_floor(root: &Path, project: &str, task: &str) -> Result<Value> {
    use rusqlite::OptionalExtension;
    let Some(store) = super::context::open(root, cortex_core::OpenMode::LiveReadOnly)? else {
        return Ok(Value::Null);
    };
    store
        .read(|db| {
            Ok(db
                .query_row(
                    "SELECT r.task_id,r.revision FROM workspace_tasks w JOIN workspace_executions e
         ON e.execution_id=w.execution_id AND e.task_id=w.task_id
         JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
         WHERE w.project_id=? AND w.task_id=? AND r.revision>=0",
                    (project, task),
                    |r| Ok(json!({"task":r.get::<_,String>(0)?,"revision":r.get::<_,i64>(1)?})),
                )
                .optional()?
                .unwrap_or(Value::Null))
        })
        .map_err(super::context::core)
}

/// Appending an event can advance the result version without re-reviewing the tree.
/// Require a later runtime plan in addition to ordinary current evidence.
pub(super) fn fresh_needed(
    db: &rusqlite::Connection,
    project: &str,
    execution: &str,
    record: &Value,
) -> cortex_core::CoreResult<bool> {
    use rusqlite::OptionalExtension;
    if record["merge"]["finished"] != true
        && (record["merge"]["conflictId"].is_string()
            || (record["merge"].is_object()
                && matches!(
                    record["conflict"].as_str(),
                    Some("conflict" | "failed" | "resolving")
                )))
    {
        return Ok(true);
    }
    let checks = &record["merge"]["freshChecks"];
    let floor = if checks.is_object() {
        &checks["after"]
    } else if let Some(floor) = record["requiresFreshResult"].get("after") {
        floor
    } else {
        return Ok(false);
    };
    let (Some(task), Some(revision)) = (floor["task"].as_str(), floor["revision"].as_i64()) else {
        return Ok(true);
    };
    let actual = db
        .query_row(
            "SELECT r.revision FROM workspace_executions e JOIN workspace_tasks w
         ON w.execution_id=e.execution_id AND e.task_id=w.task_id
         JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
         WHERE w.project_id=? AND e.execution_id=? AND r.task_id=?",
            (project, execution, task),
            |r| r.get::<_, i64>(0),
        )
        .optional()?;
    Ok(actual.is_none_or(|current| current <= revision))
}

pub(super) fn finished(record: &mut Value, revision: String, by: &str, after: Value) {
    record["changesBaseRevision"] = record["merge"]["targetRevision"].clone();
    record["conflict"] = json!("none");
    record["resolvedBy"] = json!(by);
    record["acceptance"] = Value::Null;
    record["automaticReady"] = json!(false);
    record.as_object_mut().unwrap().remove("status");
    record["merge"]["gitState"] = json!("completed");
    record["merge"]["finished"] = json!(true);
    record["merge"]["finishedAt"] = json!(now_ms());
    record["merge"]["freshChecks"] =
        json!({"state":"not_requested","revision":revision,"after":after});
}

/// Run at most one normal merge commit. The caller marks the effect uncertain
/// before a commit hook can run and persists the result under the repository lock.
pub(super) async fn complete(
    path: &Path,
    merge: &Value,
    git: &mut Git,
    observed: Observed,
    effects: &mut bool,
) -> Result<String> {
    match observed {
        Observed::Completed(revision) => Ok(revision),
        Observed::Stopped => {
            if super::integration::conflicts::has_unmerged(path, git).await?
                || !git.success(path, &["diff", "--cached", "--check"]).await?
            {
                return Err(refuse(
                    "unresolved_conflict",
                    "Resolve and stage every conflict without markers first.",
                ));
            }
            if observe(path, merge, git).await? != Observed::Stopped {
                return Err(unsafe_merge());
            }
            *effects = true;
            git.out(path, &["commit", "-m", "Resolve task merge conflicts"])
                .await?;
            match observe(path, merge, git).await? {
                Observed::Completed(revision) => Ok(revision),
                _ => Err(unknown()),
            }
        }
        _ => Err(unsafe_merge()),
    }
}

pub(super) fn unsafe_merge() -> Refused {
    refuse("unsafe_conflict", "Git no longer shows the saved task and target commits. Inspect the task checkout before continuing.")
}

fn unknown() -> Refused {
    refuse("integration_unknown", "The merge may have changed Git or task state. Inspect Git before requesting a new action; nothing was retried.")
}
pub async fn run(
    root: &Path,
    project: &str,
    task: &str,
    state: &Path,
    product: &mut Value,
    p: &Value,
    git: &mut Git,
    lock: Lock,
) -> Result<Value> {
    let record = &product["tasks"][task];
    let abort = p["action"] == "abort_merge";
    let discard_legacy = match p.get("discardLegacy") {
        None => false,
        Some(value) if abort && value == true => true,
        Some(_) => {
            return Err(refuse(
                "invalid_request",
                "Clearing an old merge record requires explicit confirmation.",
            ));
        }
    };
    if discard_legacy && record["merge"]["conflictId"].is_string() {
        return Err(refuse(
            "stale_conflict",
            "This merge has a saved identity. Refresh the task and use the normal merge controls.",
        ));
    }
    if !discard_legacy && manual_blocked(record) {
        return Err(refuse(
            "busy",
            "Stop Cortex and wait for the task to stop before resolving manually.",
        ));
    }
    if truthy(&record["checkout"])
        && (!record["checkout"].is_object() || !record["checkout"]["kind"].is_string())
    {
        return Err(refuse("machine_tool_failed", "TypeError"));
    }
    let path = checkout(root, project, task, record, git).await?;
    if !truthy(&record["merge"]) || record["merge"]["finished"] == true {
        return Err(refuse(
            "no_conflict",
            "There is no retained merge conflict for this task.",
        ));
    }
    if discard_legacy {
        require_settled(root, project, task)?;
    } else {
        manual_ready(root, project, task, record)?;
    }
    if (!abort || discard_legacy) && !record["merge"].is_object() {
        return Err(refuse("machine_tool_failed", "AttributeError"));
    }
    // The Git merge commit (and its hooks) or abort runs after the machine lock is
    // released; this task's other owner actions are refused until it settles.
    let claim = settings::claim(state, project, task, IN_PROGRESS, &lock)?;
    let intent =
        match ledger::Intent::begin_locked(state, "product.task", project, Some(root), p, &lock)? {
            ledger::Begin::Replay(reply) => return super::task_resolution::replay(reply),
            ledger::Begin::Run(intent) => intent,
        };
    drop(lock);
    let mut effects = false;
    let outcome: Result<Value> = async {
        let merge = &product["tasks"][task]["merge"];
        if discard_legacy {
            let after = result_floor(root, project, task)?;
            require_legacy_merge_closed(&path, merge, git).await?;
            effects = true;
            let record = &mut product["tasks"][task];
            // There is no pre-merge snapshot to justify restoring old evidence, even
            // if the owner aborted and Git returned to the original tree.
            clear_abort(record, false, after);
            record["resolvedBy"] = Value::Null;
            record.as_object_mut().unwrap().remove("status");
        } else if abort {
            match observe(&path, merge, git).await? {
                Observed::Stopped => {
                    effects = true;
                    git.out(&path, &["merge", "--abort"]).await?;
                    if observe(&path, merge, git).await? != Observed::AtSource {
                        return Err(unknown());
                    }
                }
                Observed::AtSource => {}
                _ => return Err(unsafe_merge()),
            }
            let exact = exact_abort(&path, merge, git).await?;
            let after = if exact {
                Value::Null
            } else {
                result_floor(root, project, task)?
            };
            effects = true;
            clear_abort(&mut product["tasks"][task], exact, after);
        } else {
            let after = result_floor(root, project, task)?;
            let observed = observe(&path, merge, git).await?;
            let revision = complete(&path, merge, git, observed, &mut effects).await?;
            effects = true;
            finished(&mut product["tasks"][task], revision, "you", after);
        }
        save(state, project, task, product)?;
        Ok(json!({"done":true,"reclamation":product["tasks"][task]["reclamation"]}))
    }
    .await;
    let reply = envelope(outcome.map_err(|e| if effects { unknown() } else { e }));
    intent.finish(state, &reply).map_err(|_| unknown())?;
    drop(claim);
    super::task_resolution::replay(reply)
}
