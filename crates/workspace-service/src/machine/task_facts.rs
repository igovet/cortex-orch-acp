//! Read-only owner task facts. No scheduler ownership or task mutation on this path.
use super::{context, git::Git, native::*};
use cortex_core::{CoreError, OpenMode, Store};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::path::Path;

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
pub(crate) async fn native_observation(root: &Path) -> Option<String> {
    crate::native_effects::observe(root.to_owned())
        .await
        .ok()
        .and_then(|e| e.project_digest)
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
        actions["reason"] = json!("This task failed before it produced a result. Request changes to try again.");
        actions["unblockAction"] = json!("request_changes");
    } else if status == Some("COMPLETED") {
        actions = match result_evidence(store, root, project, execution, native, own_writes(record)) {
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
    let __t=std::time::Instant::now();
    let Ok(Some(store)) = context::open(root, OpenMode::LiveReadOnly) else {
        return (unavailable(), true);
    };
    eprintln!("perf-sec review_open_ms={}", __t.elapsed().as_millis());
    let __r = record_actions(
        &store,
        root,
        project,
        execution,
        Some(status),
        record,
        native,
    )
    .unwrap_or_else(|_| (unavailable(), true)); eprintln!("perf-sec review_total_ms={}", __t.elapsed().as_millis()); __r
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
            native_observation(root).await
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
) -> Result<Value> {
    let (actions, work, warnings, _) =
        result_evidence(store, root, project, execution, native, own_writes(record))?;
    if actions["digest"] != *wanted || warnings {
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
    if actions["accept"] != true {
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
    record["sprintId"].as_str().is_some_and(|s| !s.is_empty()) && record["checkout"]["kind"] == "main"
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
        .read(|db| result_evidence_db(db, store, root, project, execution, native, own_writes))
        .map_err(context::core)
}
pub(super) fn result_evidence_db(
    db: &rusqlite::Connection,
    store: &Store,
    root: &Path,
    project: &str,
    execution: &str,
    native: Option<&str>,
    own_writes: bool,
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
    let (reason, unblock) = if status != "COMPLETED" {
        (Some("Wait for Cortex to finish this result."), Some("wait"))
    } else if crate::native_owner_controls::receipts_partial(db, &task)? {
        // The receipt set is larger than one read presents; Accept is never decided from part of it.
        (Some(crate::native_owner_controls::RECEIPTS_PARTIAL_REASON), Some("request_changes"))
    } else if evidence.warnings {
        (Some("Result evidence is unavailable or the files changed. Request changes and run fresh checks."),Some("request_changes"))
    } else if evidence.work && !evidence.approved {
        (
            Some("Independent review is not approved. Request changes to complete review."),
            Some("request_changes"),
        )
    } else if evidence.work && (!evidence.verified || evidence.observed.is_none()) {
        (
            Some("Run the current runtime verification checks before accepting work."),
            Some("rerun_checks"),
        )
    } else if !evidence.work && !answer {
        (
            Some("This result has no recorded answer or work evidence. Request a new result."),
            Some("request_changes"),
        )
    } else if evidence.unexplained {
        (Some("Run the unpassed checks first, or request changes. Only recorded technical blockers may be acknowledged."),Some("rerun_checks"))
    } else {
        (None, None)
    };
    let id = format!("wr_{execution}");
    let (drifted, scoped, notes) =
        (evidence.drifted, evidence.scoped, std::mem::take(&mut evidence.review_warnings));
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
    Ok(actions)
}
#[cfg(test)]
#[path = "owner_facts_perf.rs"]
mod owner_facts_perf;
