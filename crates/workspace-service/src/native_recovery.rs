//! Restart from durable checkpoints, never from an inferred process outcome.
use crate::native_runtime::Runtime;
use cortex_core::{
    lifecycle::{self, domain, waits},
    ownership::{self, OwnerEpoch},
    CoreError, CoreResult, TaskState,
};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Arc};

/// Runs one task's recovery writes inside a savepoint. A failure rolls back only
/// that task's writes and is returned to the caller, so one damaged or legacy task
/// can never fail the recovery (and the opening) of the whole project.
fn isolated<T>(
    db: &rusqlite::Transaction<'_>,
    body: impl FnOnce() -> CoreResult<T>,
) -> CoreResult<T> {
    db.execute_batch("SAVEPOINT native_recovery_task")?;
    match body() {
        Ok(value) => {
            db.execute_batch("RELEASE native_recovery_task")?;
            Ok(value)
        }
        Err(error) => {
            db.execute_batch("ROLLBACK TO native_recovery_task; RELEASE native_recovery_task")?;
            Err(error)
        }
    }
}

/// Whether the task has a runtime record. Runtime events reference it, so a task the
/// retired Python host ran before runtime records existed can never carry one.
fn runtime_record(db: &rusqlite::Transaction<'_>, task: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_tasks WHERE task_id=?)",
        [task],
        |r| r.get(0),
    )?)
}

/// Records a settlement on the task's history when it has one. A task without a
/// runtime record keeps the evidence on the settled rows themselves (the terminated
/// provisioning status and the reservation's release observation).
fn settled_event_tx(db: &rusqlite::Transaction<'_>, task: &str, payload: Value) -> CoreResult<()> {
    if runtime_record(db, task)? {
        lifecycle::event_tx(db, task, "NATIVE_PROCESS_SETTLED", payload)?;
    }
    Ok(())
}

/// The process group and start time (`/proc/<pid>/stat` fields 5 and 22).
fn process_stat(pid: i64) -> Option<(i64, String)> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<&str> = stat
        .get(stat.rfind(')')? + 1..)?
        .split_whitespace()
        .collect();
    Some((fields.get(2)?.parse().ok()?, (*fields.get(19)?).to_owned()))
}

/// Whether a process recorded in this boot may still run: its exact process (pid with
/// the recorded start time) or any member of its process group. Anything that cannot
/// be observed counts as running.
pub(crate) fn recorded_process_alive(pid: i64, pgid: i64, start: &str) -> bool {
    if pid <= 0 {
        return true;
    }
    if process_stat(pid).is_some_and(|(_, started)| started == start) {
        return true;
    }
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return true;
    };
    pgid > 0
        && entries.flatten().any(|e| {
            e.file_name()
                .to_str()
                .and_then(|n| n.parse::<i64>().ok())
                .and_then(process_stat)
                .is_some_and(|(group, _)| group == pgid)
        })
}

/// Process records of the retired Python host never received an end time when its
/// boot ended, so they kept old tasks "running" forever (Delete refused them). A
/// record from an earlier boot cannot name a live process; a record from this boot is
/// ended only when neither its exact process nor any member of its process group
/// still exists. The row itself keeps the evidence (`termination_reason`).
fn settle_registry_tx(
    db: &rusqlite::Transaction<'_>,
    boot: &str,
    failures: &mut Vec<(String, String)>,
) -> CoreResult<()> {
    let rows = db
        .prepare(
            "SELECT task_id,attempt_id,pid,pgid,start_ticks,boot_id FROM attempt_process_registry
             WHERE terminated_at IS NULL ORDER BY task_id,rowid",
        )?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut per_task: BTreeMap<String, Vec<(String, &'static str)>> = BTreeMap::new();
    for (task, attempt, pid, pgid, start, recorded_boot) in rows {
        let reason = if recorded_boot != boot {
            "previous_boot"
        } else if !recorded_process_alive(pid, pgid, &start) {
            "process_gone"
        } else {
            continue;
        };
        per_task.entry(task).or_default().push((attempt, reason));
    }
    for (task, ended) in per_task {
        let done = isolated(db, || {
            for (attempt, reason) in &ended {
                db.execute(
                    "UPDATE attempt_process_registry SET terminated_at=?,termination_reason=?
                     WHERE attempt_id=? AND task_id=? AND terminated_at IS NULL",
                    (cortex_core::now(), reason, attempt, &task),
                )?;
            }
            Ok(())
        });
        if let Err(error) = done {
            failures.push((task, error.code().to_owned()));
        }
    }
    Ok(())
}

/// The retired Python host left `failed`/`in_progress` worker rows whose session was
/// never created: no engine session, no process record, and either every capacity
/// reservation of the attempt already released by that host's own process
/// observation, or (`failed`) no capacity at all. No process of such an attempt can
/// run. This host never leaves a row like that: it releases a reservation and
/// terminates its provisioning row in one transaction, and never writes `failed`.
fn settle_released_unstarted_tx(
    db: &rusqlite::Transaction<'_>,
    failures: &mut Vec<(String, String)>,
) -> CoreResult<()> {
    let rows = db
        .prepare(
            "SELECT p.task_id,p.idempotency_key,p.status,p.attempt_id FROM delegation_provisioning p
             WHERE p.status!='terminated' AND p.engine_session_ref IS NULL
             AND NOT EXISTS(SELECT 1 FROM attempt_process_registry a WHERE a.attempt_id=p.attempt_id)
             AND NOT EXISTS(SELECT 1 FROM capacity_reservations r WHERE r.attempt_id=p.attempt_id AND r.state!='released')
             AND (EXISTS(SELECT 1 FROM capacity_reservations r WHERE r.attempt_id=p.attempt_id)
                OR p.status='failed')
             ORDER BY p.task_id,p.rowid",
        )?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut per_task: BTreeMap<String, Vec<(String, String, Option<String>)>> = BTreeMap::new();
    for (task, key, status, attempt) in rows {
        per_task
            .entry(task)
            .or_default()
            .push((key, status, attempt));
    }
    for (task, rows) in per_task {
        let done = isolated(db, || {
            for (key, _, _) in &rows {
                db.execute(
                    "UPDATE delegation_provisioning SET status='terminated',updated_at=?
                     WHERE task_id=? AND idempotency_key=? AND status!='terminated'",
                    (cortex_core::now(), &task, key),
                )?;
            }
            let settled: Vec<Value> = rows
                .iter()
                .map(|(key, status, attempt)| {
                    json!({"delegation":key,"attempt":attempt,"previous_status":status})
                })
                .collect();
            settled_event_tx(
                db,
                &task,
                json!({"count":settled.len(),"delegations":settled,
                    "evidence":"no_engine_session_or_process_and_capacity_released"}),
            )
        });
        if let Err(error) = done {
            failures.push((task, error.code().to_owned()));
        }
    }
    Ok(())
}

/// A delegation left non-terminated (e.g. `ambiguous` by the retired Python host)
/// whose every registered process has terminated or belongs to an earlier boot
/// cannot have a live process: settle it with that evidence. A delegation with no
/// registered process is left to the other rules (fail-closed).
fn settle_registered_tx(
    db: &rusqlite::Transaction<'_>,
    boot: &str,
    failures: &mut Vec<(String, String)>,
) -> CoreResult<()> {
    let gone = db
        .prepare(
            "SELECT p.task_id,p.idempotency_key,p.status FROM delegation_provisioning p
             WHERE p.status!='terminated' AND p.attempt_id IS NOT NULL
             AND EXISTS(SELECT 1 FROM attempt_process_registry r WHERE r.attempt_id=p.attempt_id)
             AND NOT EXISTS(SELECT 1 FROM attempt_process_registry r WHERE r.attempt_id=p.attempt_id
                 AND r.terminated_at IS NULL AND r.boot_id=?)
             ORDER BY p.task_id,p.rowid",
        )?
        .query_map([boot], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut per_task: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for (task, key, status) in gone {
        per_task.entry(task).or_default().push((key, status));
    }
    for (task, rows) in per_task {
        let done = isolated(db, || {
            for (key, _) in &rows {
                db.execute(
                    "UPDATE delegation_provisioning SET status='terminated',updated_at=? WHERE idempotency_key=? AND task_id=?",
                    (cortex_core::now(), key, &task),
                )?;
            }
            let settled: Vec<Value> = rows
                .iter()
                .map(|(key, status)| json!({"delegation":key,"previous_status":status}))
                .collect();
            settled_event_tx(
                db,
                &task,
                json!({"count":settled.len(),"delegations":settled,
                    "evidence":"registered_processes_terminated_or_previous_boot"}),
            )
        });
        if let Err(error) = done {
            failures.push((task, error.code().to_owned()));
        }
    }
    Ok(())
}

/// A finished desktop task the retired Python host ran was never adopted (adoption
/// only ever took unfinished tasks), so every owner action on it (Request changes,
/// a message, Continue, Retry, Close) was refused as an invalid binding. Adopt it the
/// same way, without replaying anything: a new owner epoch fences the old host and the
/// execution follows it. Only a task whose execution binding is otherwise valid (same
/// owner epoch, session and project root) is adopted.
fn adopt_finished_tx(
    db: &rusqlite::Transaction<'_>,
    failures: &mut Vec<(String, String)>,
) -> CoreResult<()> {
    let rows = db
        .prepare(
            "SELECT r.task_id,r.state,o.owner_ref,o.epoch FROM runtime_tasks r
             JOIN task_owners o ON o.task_id=r.task_id
             JOIN tasks base ON base.id=r.task_id
             JOIN workspace_executions e ON e.runtime_task_id=r.task_id AND e.owner_epoch=o.epoch
             JOIN acp_host_sessions h ON h.session_id=e.acp_session_id AND h.task_id=r.task_id
                AND h.project_root=base.project_root
             WHERE r.state IN ('COMPLETED','CANCELLED','FAILED')
             AND NOT EXISTS(SELECT 1 FROM runtime_events v WHERE v.task_id=r.task_id
                AND v.type IN ('TASK_CREATED','NATIVE_TASK_ADOPTED')
                AND json_extract(v.payload_json,'$.execution_host')='rust')
             ORDER BY r.rowid",
        )?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (task, state, owner, epoch) in rows {
        let done = isolated(db, || {
            let adopted = ownership::acquire_tx(db, &task, "native-recovery", Some(epoch))?;
            lifecycle::event_tx(
                db,
                &task,
                "NATIVE_TASK_ADOPTED",
                json!({"execution_host":"rust","previous_host":"python","previous_owner":owner,
                    "previous_epoch":epoch,"owner_epoch":adopted.epoch,"replayed":false,
                    "finished":true,"state":state}),
            )?;
            db.execute(
                "UPDATE workspace_executions SET owner_epoch=?,version=version+1
                 WHERE runtime_task_id=? AND owner_epoch=?",
                (adopted.epoch, &task, epoch),
            )?;
            Ok(())
        });
        if let Err(error) = done {
            failures.push((task, error.code().to_owned()));
        }
    }
    Ok(())
}

/// Releases one attempt's delegation rows and capacity reservations with an
/// explicit observation. Only called with positive evidence that no process of
/// the attempt can still run.
fn release_attempt_tx(
    db: &rusqlite::Transaction<'_>,
    task: &str,
    attempt: &str,
    observation: &str,
) -> CoreResult<()> {
    db.execute(
        "UPDATE delegation_provisioning SET status='terminated',updated_at=?
         WHERE task_id=? AND attempt_id=? AND status!='terminated'",
        (cortex_core::now(), task, attempt),
    )?;
    db.execute(
        "UPDATE capacity_reservations SET state='released',release_observation=?,updated_at=?
         WHERE task_id=? AND attempt_id=? AND state!='released'",
        (observation, cortex_core::now(), task, attempt),
    )?;
    Ok(())
}

/// A session start that failed before its engine was launched never had an engine
/// process. Earlier builds recorded such a failure as an unknown observation, so the
/// attempt stayed `ambiguous` forever and every later control, continuation and
/// deletion of the task was refused. The durable evidence is the session's own
/// `NATIVE_SESSION_START_FAILED` event, bound to the exact channel:
///
/// - phase `broker`: nothing was started at all;
/// - phase `launch` or `external_mcp` with `authority_refused`/`invalid_arguments`:
///   a policy refusal before the engine launch. `Engine::launch` refuses that way
///   only before it spawns; the current host attaches external servers without
///   failing (a server is withheld instead), so an `external_mcp` refusal is the
///   earlier hosts' pre-launch attachment refusal, after which that host had
///   already stopped any external server it started (process-group kill).
///
/// The session must never have been bound (`engine_session_ref` absent) and the
/// channel must have no other activity. Any other start failure (for example an
/// `io_error`, or any phase after the launch) keeps the attempt fail-closed.
fn settle_unlaunched_tx(
    db: &rusqlite::Transaction<'_>,
    failures: &mut Vec<(String, String)>,
) -> CoreResult<()> {
    let rows = db
        .prepare(
            "SELECT p.task_id,p.idempotency_key,p.status,p.attempt_id,
                json_extract(e.payload_json,'$.phase'),json_extract(e.payload_json,'$.error')
             FROM delegation_provisioning p
             JOIN runtime_events e ON e.task_id=p.task_id AND e.type='NATIVE_SESSION_START_FAILED'
                AND json_extract(e.payload_json,'$.channel_id')=p.channel_id
             WHERE p.status!='terminated' AND p.channel_id IS NOT NULL AND p.attempt_id IS NOT NULL
             AND p.engine_session_ref IS NULL
             AND (json_extract(e.payload_json,'$.phase')='broker'
                OR (json_extract(e.payload_json,'$.phase') IN ('launch','external_mcp')
                    AND json_extract(e.payload_json,'$.error') IN ('authority_refused','invalid_arguments')))
             AND (SELECT count(*) FROM runtime_events f WHERE f.task_id=p.task_id
                AND f.type='NATIVE_SESSION_START_FAILED'
                AND json_extract(f.payload_json,'$.channel_id')=p.channel_id)=1
             AND NOT EXISTS(SELECT 1 FROM runtime_events x WHERE x.task_id=p.task_id
                AND json_extract(x.payload_json,'$.channel_id')=p.channel_id
                AND x.type NOT IN ('NATIVE_SESSION_START_FAILED','NATIVE_EXTERNAL_MCP_WITHHELD'))
             ORDER BY p.rowid",
        )?
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut per_task: BTreeMap<
        String,
        Vec<(String, String, String, Option<String>, Option<String>)>,
    > = BTreeMap::new();
    for (task, key, status, attempt, phase, error) in rows {
        per_task
            .entry(task)
            .or_default()
            .push((key, status, attempt, phase, error));
    }
    for (task, rows) in per_task {
        let done = isolated(db, || {
            let mut settled = Vec::new();
            for (key, status, attempt, phase, error) in &rows {
                release_attempt_tx(db, &task, attempt, "never_launched")?;
                settled.push(json!({"delegation":key,"attempt":attempt,
                    "previous_status":status,"phase":phase,"error":error}));
            }
            settled_event_tx(
                db,
                &task,
                json!({"count":settled.len(),"delegations":settled,
                    "evidence":"session_start_failed_before_engine_launch"}),
            )
        });
        if let Err(error) = done {
            failures.push((task, error.code().to_owned()));
        }
    }
    Ok(())
}

/// Whether `cmdline` (NUL-separated) is an ACP engine adapter Cortex launches, a
/// process of its session (the Codex App Server Cortex runs, or anything handed a
/// session's MCP relay endpoint, which names a `cortex-mcp-` broker directory), or
/// OpenCode.
fn engine_command(cmdline: &[u8]) -> bool {
    cmdline
        .split(|b| *b == 0)
        .filter(|a| !a.is_empty())
        .any(|arg| {
            let arg = String::from_utf8_lossy(arg);
            arg.contains("codex-acp")
                || arg.contains("claude-agent-acp")
                || arg.contains("--native-codex-runtime")
                || arg.contains("/cortex-mcp-")
                || std::path::Path::new(arg.as_ref())
                    .file_name()
                    .is_some_and(|n| n == "opencode")
        })
}

/// Whether any process of this user other than this service could be an engine
/// session (or one of its processes) working in one of `roots`. Every engine session
/// is launched with the project root as its working directory (`dispatch` refuses any
/// other), and its relays and App Server inherit it. A process whose command line or
/// working directory cannot be read is counted (fail-closed); a process that exited
/// or is a zombie cannot act and is not. An unreadable process table counts.
fn engine_process_in(proc_root: &std::path::Path, roots: &[std::path::PathBuf]) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return true;
    };
    let me = std::process::id();
    let uid = unsafe { libc::geteuid() };
    let gone = |e: &std::io::Error| {
        e.kind() == std::io::ErrorKind::NotFound || e.raw_os_error() == Some(libc::ESRCH)
    };
    for entry in entries.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        if pid == me {
            continue;
        }
        match entry.metadata() {
            Ok(m) if m.uid() != uid => continue,
            Ok(_) => {}
            Err(e) if gone(&e) => continue,
            Err(_) => return true,
        }
        let cmdline = match std::fs::read(entry.path().join("cmdline")) {
            Ok(c) => c,
            Err(e) if gone(&e) => continue,
            Err(_) => return true,
        };
        // Empty: a zombie or a kernel thread.
        if cmdline.is_empty() || !engine_command(&cmdline) {
            continue;
        }
        match std::fs::read_link(entry.path().join("cwd")) {
            Ok(cwd) if !roots.iter().any(|r| cwd.starts_with(r)) => continue,
            Err(e) if gone(&e) => continue,
            _ => return true,
        }
    }
    false
}

/// The roots an engine of these tasks could work in: the project and every task's
/// own project root and workspace, each as recorded and canonical.
fn engine_roots_tx(
    db: &rusqlite::Transaction<'_>,
    root: &std::path::Path,
) -> CoreResult<Vec<std::path::PathBuf>> {
    let mut roots: Vec<std::path::PathBuf> = vec![root.to_path_buf()];
    let recorded = db
        .prepare("SELECT project_root FROM tasks UNION SELECT workspace_root FROM task_workspaces")?
        .query_map([], |r| r.get::<_, Option<String>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    roots.extend(
        recorded
            .into_iter()
            .flatten()
            .filter(|r| !r.is_empty())
            .map(Into::into),
    );
    let canonical: Vec<_> = roots.iter().filter_map(|r| r.canonicalize().ok()).collect();
    roots.extend(canonical);
    roots.sort();
    roots.dedup();
    Ok(roots)
}

/// A service that dies (for example by SIGKILL) leaves its running attempts open:
/// delegation `ready`/`ambiguous`, reservation `reserved`/`ambiguous`, and no process
/// record (this host records engine processes only through these rows), so their
/// exit could never be confirmed in the same boot and every later message, Stop,
/// Resume and Delete of the task was refused. Recovery runs only under this
/// project's exclusive scheduler lease, before any session of the project starts, so
/// the service that drove these attempts is gone. When no engine process can be
/// observed working in the project ([`engine_process_in`]), none of those attempts
/// can still run: each is settled as terminated with that evidence and its task is
/// returned so recovery holds it as interrupted instead of driving it on its own.
/// While a candidate process exists, the rows stay fail-closed (the boot rule below
/// still applies). Attempts with a process record are left to the registry rules,
/// and tasks without a runtime record are left as they were.
fn settle_without_engine_tx(
    db: &rusqlite::Transaction<'_>,
    root: &std::path::Path,
    engine_proc: &std::path::Path,
    failures: &mut Vec<(String, String)>,
) -> CoreResult<std::collections::BTreeSet<String>> {
    let open = db
        .prepare(
            "SELECT task_id,attempt_id FROM (
                SELECT task_id,attempt_id FROM delegation_provisioning
                    WHERE status!='terminated' AND attempt_id IS NOT NULL
                UNION SELECT task_id,attempt_id FROM capacity_reservations WHERE state!='released') o
             WHERE EXISTS(SELECT 1 FROM runtime_tasks t WHERE t.task_id=o.task_id)
             AND NOT EXISTS(SELECT 1 FROM attempt_process_registry a WHERE a.attempt_id=o.attempt_id)
             ORDER BY 1,2",
        )?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    if open.is_empty() || !no_engine_process_tx(db, root, engine_proc)? {
        return Ok(Default::default());
    }
    settle_without_engine_attempts_tx(
        db,
        open,
        "service_gone_and_no_engine_process_in_project",
        failures,
    )
}

/// The process evidence both settlements of an attempt without a process record rely
/// on: no engine process ([`engine_process_in`]) works in the project or any task's
/// root. Fail-closed: anything unreadable counts as such a process.
fn no_engine_process_tx(
    db: &rusqlite::Transaction<'_>,
    root: &std::path::Path,
    engine_proc: &std::path::Path,
) -> CoreResult<bool> {
    Ok(!engine_process_in(engine_proc, &engine_roots_tx(db, root)?))
}

/// Settles each `(task, attempt)` once [`no_engine_process_tx`] holds: the delegation
/// becomes `terminated`, the reservation `released` with observation
/// `no_engine_process`, and each task gets one `NATIVE_PROCESS_SETTLED` event naming
/// `evidence`. One task's failed writes roll back alone. Returns the settled tasks.
fn settle_without_engine_attempts_tx(
    db: &rusqlite::Transaction<'_>,
    open: Vec<(String, String)>,
    evidence: &str,
    failures: &mut Vec<(String, String)>,
) -> CoreResult<std::collections::BTreeSet<String>> {
    let mut settled_tasks = std::collections::BTreeSet::new();
    let mut per_task: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (task, attempt) in open {
        per_task.entry(task).or_default().push(attempt);
    }
    for (task, attempts) in per_task {
        let done = isolated(db, || {
            let mut settled = Vec::new();
            for attempt in &attempts {
                release_attempt_tx(db, &task, attempt, "no_engine_process")?;
                settled.push(json!({"attempt":attempt}));
            }
            lifecycle::event_tx(
                db,
                &task,
                "NATIVE_PROCESS_SETTLED",
                json!({"count":settled.len(),"delegations":settled,"evidence":evidence}),
            )?;
            Ok(())
        });
        match done {
            Ok(()) => {
                settled_tasks.insert(task);
            }
            Err(error) => failures.push((task, error.code().to_owned())),
        }
    }
    Ok(settled_tasks)
}

/// The attempts a running service may settle by itself: each one's every unsettled
/// row is a recorded unconfirmed exit (`ambiguous`, so nothing of it is still being
/// started or stopped), it has no process record (those are the registry rules',
/// at startup), its task has a runtime record, and it is bound to its task's current
/// owner epoch (a provisioning row or channel exists, and none names another epoch).
fn unconfirmed_attempts(db: &rusqlite::Connection) -> CoreResult<Vec<(String, String)>> {
    Ok(db
        .prepare(
            "SELECT o.task_id,o.attempt_id FROM (
                SELECT task_id,attempt_id FROM delegation_provisioning
                    WHERE status='ambiguous' AND attempt_id IS NOT NULL
                UNION SELECT task_id,attempt_id FROM capacity_reservations WHERE state='ambiguous') o
             JOIN task_owners w ON w.task_id=o.task_id
             WHERE EXISTS(SELECT 1 FROM runtime_tasks t WHERE t.task_id=o.task_id)
             AND NOT EXISTS(SELECT 1 FROM attempt_process_registry a WHERE a.attempt_id=o.attempt_id)
             AND NOT EXISTS(SELECT 1 FROM delegation_provisioning p WHERE p.attempt_id=o.attempt_id
                AND p.status NOT IN ('terminated','ambiguous'))
             AND NOT EXISTS(SELECT 1 FROM capacity_reservations r WHERE r.attempt_id=o.attempt_id
                AND r.state NOT IN ('released','ambiguous'))
             AND (EXISTS(SELECT 1 FROM delegation_provisioning p WHERE p.attempt_id=o.attempt_id AND p.task_id=o.task_id)
                OR EXISTS(SELECT 1 FROM trusted_channels c WHERE c.attempt_id=o.attempt_id AND c.task_id=o.task_id))
             AND NOT EXISTS(SELECT 1 FROM delegation_provisioning p WHERE p.attempt_id=o.attempt_id
                AND p.owner_epoch!=w.epoch)
             AND NOT EXISTS(SELECT 1 FROM trusted_channels c WHERE c.attempt_id=o.attempt_id
                AND c.owner_epoch!=w.epoch)
             ORDER BY 1,2",
        )?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?)
}

/// Backoff of the live re-check while its evidence stays inconclusive. A changed set
/// of unconfirmed attempts is looked at at once.
#[derive(Default, Debug)]
pub(crate) struct ExitRecheck {
    attempts: Vec<(String, String)>,
    next: Option<std::time::Instant>,
    delay: std::time::Duration,
}
const RECHECK_FIRST: std::time::Duration = std::time::Duration::from_secs(15);
const RECHECK_MAX: std::time::Duration = std::time::Duration::from_secs(600);

/// Backoff of the live re-settle of orphaned command receipts, with the same bounds
/// as the exit re-check. A changed set of receipts is looked at at once.
#[derive(Default, Debug)]
pub(crate) struct CommandRecheck {
    rows: Vec<String>,
    next: Option<std::time::Instant>,
    delay: std::time::Duration,
}

impl Runtime {
    /// Live re-settle of `command_run`/`outside_command` receipts left unsettled
    /// (a lost call, channel or supervisor), on the sprint scheduler's passes. Commands
    /// this service still runs or observes are skipped; each other receipt is settled
    /// only on proof that its process is gone ([`crate::native_command_handles::resettle_row`]),
    /// never as a success. While some stay open the check backs off (15 s, doubling to
    /// 10 min). Returns how many receipts were settled.
    pub(crate) async fn resettle_orphaned_commands(self: &Arc<Self>) -> CoreResult<usize> {
        if *self.stop.borrow() {
            return Ok(0);
        }
        let busy: std::collections::BTreeSet<String> =
            self.host.commands.lock().await.keys().cloned().collect();
        let skip = busy.clone();
        let rows: Vec<String> = self
            .host
            .executor
            .read(move |db| {
                Ok(crate::native_command_handles::orphans(db)?
                    .into_iter()
                    .filter(|(_, _, _, r)| {
                        !r.as_deref().is_some_and(|r| {
                            skip.contains(r) || crate::native_commands::in_flight(r)
                        })
                    })
                    .map(|(t, a, k, _)| format!("{t}\u{0}{a}\u{0}{k}"))
                    .collect())
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        {
            let mut recheck = self
                .command_recheck
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            if rows.is_empty() {
                *recheck = CommandRecheck::default();
                return Ok(0);
            }
            if recheck.rows != rows {
                *recheck = CommandRecheck {
                    rows: rows.clone(),
                    ..CommandRecheck::default()
                };
            }
            if recheck
                .next
                .is_some_and(|at| std::time::Instant::now() < at)
            {
                return Ok(0);
            }
        }
        let (settled, left) = self
            .host
            .executor
            .publication(move |store| crate::native_command_handles::resettle(store, &busy, false))?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        let mut recheck = self
            .command_recheck
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if left.is_empty() {
            *recheck = CommandRecheck::default();
        } else {
            recheck.rows = left;
            recheck.delay = if recheck.delay.is_zero() {
                RECHECK_FIRST
            } else {
                (recheck.delay * 2).min(RECHECK_MAX)
            };
            recheck.next = Some(std::time::Instant::now() + recheck.delay);
        }
        Ok(settled)
    }
}

/// A broker directory (`cortex-mcp-` and 24 hex digits) of a service that died is
/// never removed by that service. One no socket of this system is bound in (as
/// `/proc/net/unix` lists) and that has not changed for a minute belongs to no live
/// broker: it is removed. Nothing is removed when the socket table is unreadable.
fn remove_stale_brokers(directory: &std::path::Path) {
    use std::os::unix::fs::MetadataExt;
    let Ok(sockets) = std::fs::read_to_string("/proc/net/unix") else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let uid = unsafe { libc::geteuid() };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(token) = name.strip_prefix("cortex-mcp-") else {
            continue;
        };
        if token.len() != 24
            || !token
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            continue;
        }
        let Ok(meta) = entry.path().symlink_metadata() else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age >= std::time::Duration::from_secs(60));
        if !meta.is_dir() || meta.uid() != uid || !old || sockets.contains(&format!("/{name}/")) {
            continue;
        }
        if let Err(error) = std::fs::remove_dir_all(entry.path()) {
            eprintln!(
                "workspace-service: a stale broker directory was kept ({})",
                error.kind()
            );
        }
    }
}

/// An attempt that recovery could not confirm is stamped once per boot with this
/// boot's identity (`NATIVE_PROCESS_UNCONFIRMED`). An attempt is only ever driven by
/// the service that provisioned it and is never relaunched, so any process it had
/// existed during the stamped boot at the latest. When a later recovery runs in a
/// different boot, no such process can still run: the attempt is settled with that
/// evidence. Within the same boot it stays fail-closed unless no engine process works
/// in the project any more ([`settle_without_engine_tx`]). The stamp is a runtime event,
/// so a task without a runtime record is never stamped and its rows stay unsettled.
fn settle_previous_boot_tx(
    db: &rusqlite::Transaction<'_>,
    boot: &str,
    failures: &mut Vec<(String, String)>,
) -> CoreResult<()> {
    let open = db
        .prepare(
            "SELECT task_id,attempt_id FROM delegation_provisioning
                WHERE status!='terminated' AND attempt_id IS NOT NULL
             UNION SELECT task_id,attempt_id FROM capacity_reservations WHERE state!='released'
             ORDER BY 1,2",
        )?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let stamped = |task: &str, attempt: &str, same_boot: bool| -> CoreResult<bool> {
        Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_events e, json_each(json_extract(e.payload_json,'$.attempts')) a
                WHERE e.task_id=? AND e.type='NATIVE_PROCESS_UNCONFIRMED' AND a.value=?
                AND (json_extract(e.payload_json,'$.boot_id')=?)=?)",
            (task, attempt, boot, same_boot),
            |r| r.get(0),
        )?)
    };
    let mut per_task: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (task, attempt) in open {
        per_task.entry(task).or_default().push(attempt);
    }
    for (task, attempts) in per_task {
        let done = isolated(db, || {
            if !runtime_record(db, &task)? {
                return Ok(());
            }
            let mut settled = Vec::new();
            let mut stamp = Vec::new();
            for attempt in attempts {
                if stamped(&task, &attempt, false)? {
                    release_attempt_tx(db, &task, &attempt, "previous_boot")?;
                    settled.push(json!({"attempt":attempt}));
                } else if !stamped(&task, &attempt, true)? {
                    stamp.push(attempt);
                }
            }
            if !settled.is_empty() {
                lifecycle::event_tx(
                    db,
                    &task,
                    "NATIVE_PROCESS_SETTLED",
                    json!({"count":settled.len(),"delegations":settled,
                        "evidence":"unconfirmed_attempt_from_previous_boot"}),
                )?;
            }
            if !stamp.is_empty() {
                lifecycle::event_tx(
                    db,
                    &task,
                    "NATIVE_PROCESS_UNCONFIRMED",
                    json!({"boot_id":boot,"attempts":stamp}),
                )?;
            }
            Ok(())
        });
        if let Err(error) = done {
            failures.push((task, error.code().to_owned()));
        }
    }
    Ok(())
}

/// The hold recovery records for a command an earlier host started
/// (`COMMAND_PROCESS_STARTED`) with no recorded exit.
const LEGACY_EFFECT: &str = "legacy_effect_outcome_unknown";

/// Earlier hosts recorded a command's start and exit as two events; a service that
/// died between them left the start without its exit, and recovery held the task
/// for it on every restart, forever. Such a start is settled, and its missing exit
/// recorded once (`COMMAND_PROCESS_EXITED` with `recovered: true` and its
/// `evidence`), when real evidence shows the process can no longer run:
/// - its own receipt (same task, channel and call reference) is complete
///   (`receipt_complete`, with the receipt's exit code) or settled
///   (`receipt_settled`; its outcome stays what the receipt says, never a success);
/// - or the runtime recorded an event in another boot after the start
///   (`previous_boot`): a process of an earlier boot cannot run in this one. Its
///   outcome is not known (`outcome_not_recorded`, no exit code).
///
/// A start without a call reference, or with none of this evidence, stays unknown.
fn settle_legacy_starts_tx(
    db: &rusqlite::Transaction<'_>,
    task: &str,
    boot: Option<&str>,
) -> CoreResult<usize> {
    use rusqlite::OptionalExtension;
    let starts = db
        .prepare(
            "SELECT i.id,i.actor,json_extract(i.payload_json,'$.tool_call_ref') FROM runtime_events i
             WHERE i.task_id=?1 AND i.type='COMMAND_PROCESS_STARTED'
             AND json_type(i.payload_json,'$.tool_call_ref')='text'
             AND NOT EXISTS(SELECT 1 FROM runtime_events f WHERE f.task_id=i.task_id AND f.id>i.id
                AND f.type='COMMAND_PROCESS_EXITED'
                AND json_extract(f.payload_json,'$.tool_call_ref')=json_extract(i.payload_json,'$.tool_call_ref'))
             ORDER BY i.id",
        )?
        .query_map([task], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut settled = 0;
    for (start, actor, reference) in starts {
        let receipt: Option<String> = db
            .query_row(
                "SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=?
                 AND operation IN ('command_run','outside_command') AND json_valid(result_json)
                 AND ?3 IN (json_extract(result_json,'$.tool_call_ref'),json_extract(result_json,'$.receipt.tool_call_ref'))
                 AND (json_extract(result_json,'$.complete') IS 1 OR json_extract(result_json,'$.status') IS 'complete'
                    OR json_extract(result_json,'$.settled') IS 1)
                 ORDER BY rowid DESC LIMIT 1",
                (task, &actor, &reference),
                |r| r.get(0),
            )
            .optional()?;
        let exit = if let Some(raw) = receipt {
            let receipt: Value = serde_json::from_str(&raw)?;
            let field = |name: &str| {
                Some(&receipt[name])
                    .filter(|v| !v.is_null())
                    .or_else(|| Some(&receipt["receipt"][name]).filter(|v| !v.is_null()))
                    .cloned()
                    .unwrap_or(Value::Null)
            };
            let complete = receipt["complete"] == true || receipt["status"] == "complete";
            let exit_code = field("exit_code");
            let failure = match field("failure") {
                Value::Null if !complete || !exit_code.is_i64() => json!("outcome_not_recorded"),
                failure => failure,
            };
            json!({"exit_code":exit_code,"failure":failure,
                "evidence":if complete {"receipt_complete"} else {"receipt_settled"}})
        } else {
            let earlier_boot = match boot {
                Some(boot) => db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_events b WHERE b.id>?1 AND (
                        (b.type='NATIVE_PROCESS_UNCONFIRMED'
                            AND coalesce(json_extract(b.payload_json,'$.boot_id'),'') NOT IN ('',?2))
                        OR (b.type='NATIVE_COMMAND_PROCESS'
                            AND coalesce(json_extract(b.payload_json,'$.process_identity.boot_id'),'') NOT IN ('',?2))))",
                    (start, boot),
                    |r| r.get::<_, bool>(0),
                )?,
                None => false,
            };
            if !earlier_boot {
                continue;
            }
            json!({"exit_code":null,"failure":"outcome_not_recorded","evidence":"previous_boot"})
        };
        let mut payload = exit;
        payload["tool_call_ref"] = json!(reference);
        payload["recovered"] = json!(true);
        payload["start_event"] = json!(start);
        lifecycle::event_tx(db, task, "COMMAND_PROCESS_EXITED", payload)?;
        settled += 1;
    }
    Ok(settled)
}

/// A restart that finds a task at the same point this many times without progress
/// (`boot_recovery` loop key: reason and plan revision) pauses it for the runtime's
/// own recheck instead of resuming it again.
const BOOT_RECOVERY_LIMIT: i64 = 3;
/// Records `NATIVE_RECOVERY_REQUIRED` once per restart burst: a later restart that
/// finds the task held for the same reason in the same state, with nothing recorded
/// for it since but recovery bookkeeping, records nothing again.
fn recovery_required_once_tx(
    db: &rusqlite::Transaction<'_>,
    task: &str,
    required: Value,
) -> CoreResult<()> {
    use rusqlite::OptionalExtension;
    let latest: Option<(i64, String)> = db
        .query_row(
            "SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_RECOVERY_REQUIRED'
             ORDER BY id DESC LIMIT 1",
            [task],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((id, raw)) = latest {
        let previous: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        let quiet: bool = db.query_row(
            "SELECT NOT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND id>? AND type NOT IN
                ('NATIVE_PROCESS_SETTLED','NATIVE_PROCESS_UNCONFIRMED','NATIVE_GUIDANCE_SETTLED',
                 'NATIVE_CONTROL_OUTCOME_UNKNOWN','NATIVE_START_RECONCILED','TASK_STATE_CHANGED'))",
            (task, id),
            |r| r.get(0),
        )?;
        if quiet
            && previous["reason"] == required["reason"]
            && previous["previous"] == required["previous"]
            && previous["continued"] == required["continued"]
        {
            return Ok(());
        }
    }
    lifecycle::event_tx(db, task, "NATIVE_RECOVERY_REQUIRED", required)?;
    Ok(())
}
/// How a restart found a task: resumed on its rung, paused for a recheck, or left to
/// the ordinary recovery rules.
enum RestartResume {
    Resumed,
    Paused,
    NotApplicable,
}
/// A service restart is infrastructure, never the model's failure. A task whose plan
/// was being planned, executed or reviewed, whose every owned process is confirmed
/// gone, resumes in the same state on the same ladder rung without a new plan
/// revision: what the gone sessions left with an unknown outcome is settled once for
/// re-verification (`settled_by: runtime_reverify`), each lost attempt is recorded as
/// ended by the restart (`runtime_restart`, so its node is retried at once without
/// escalation), and the fresh attempt re-verifies those effects. A restart that finds
/// the task at the same point [`BOOT_RECOVERY_LIMIT`] times without progress pauses
/// it (`pause_kind: infrastructure`) instead. A command still unsettled, unconfirmed
/// processes or any other unknown keep the ordinary recovery rules.
fn restart_resume_tx(
    db: &rusqlite::Transaction<'_>,
    task: &str,
    state: TaskState,
    reason: &str,
    boot: Option<&str>,
) -> CoreResult<RestartResume> {
    if lifecycle::automatic::require_processes_gone(db, task).is_err() {
        return Ok(RestartResume::NotApplicable);
    }
    let (_, revision) = domain::task_tx(db, task)?;
    // Resumes at this same point (the plan revision; the reason label differs between
    // restarts of one crash loop) since the task last progressed or the owner spoke.
    let resumed: i64 = db.query_row(
        "SELECT count(*) FROM runtime_events WHERE task_id=?1 AND type='NATIVE_RECOVERY_RESUMED'
           AND json_extract(payload_json,'$.restart_resume')=1 AND json_extract(payload_json,'$.revision')=?2
           AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type IN
             ('TASK_PROGRESS','TASK_REOPENED','USER_STEERING','USER_ANSWER_RECEIVED','OWNER_TASK_RESUMED')),0)",
        (task, revision),
        |r| r.get(0),
    )?;
    let key = cortex_core::scheduler::retry::loop_key(
        "boot_recovery",
        "",
        &format!("{}|{revision}", state.as_str()),
    );
    if resumed >= BOOT_RECOVERY_LIMIT {
        lifecycle::event_tx(
            db,
            task,
            "LOOP_BROKEN",
            json!({"key":key,"kind":"boot_recovery","node":null,"count":resumed + 1,
                "action":"infrastructure_pause","limits":[],"reason":reason,"revision":revision}),
        )?;
        cortex_core::scheduler::retry::infrastructure_pause_tx(
            db,
            task,
            json!({"reason":"boot_recovery_loop","restart_reason":reason,"key":key,"count":resumed + 1}),
        )?;
        return Ok(RestartResume::Paused);
    }
    let uncertainty = match lifecycle::automatic::settle_unknown_effects_tx(db, task) {
        Ok(uncertainty) => uncertainty,
        Err(CoreError::Refused("native_command_outcome_pending")) => {
            return Ok(RestartResume::NotApplicable)
        }
        Err(error) => return Err(error),
    };
    // Each attempt the restart ended without a result: retried as `runtime_restart`.
    let lost = db
        .prepare(
            "SELECT n.assignment_ref,p.attempt_id FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
             JOIN delegation_provisioning p ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
             WHERE n.task_id=? AND n.revision=t.revision AND n.status='running' AND n.result_json IS NULL
             AND p.status='terminated' AND p.attempt_id IS NOT NULL",
        )?
        .query_map([task], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    for (assignment, attempt) in &lost {
        cortex_core::scheduler::retry::record_attempt_ending_tx(
            db,
            task,
            assignment,
            attempt,
            "service_restarted",
            "runtime_restart",
            &json!({"cause":"service_restarted","restart_reason":reason,"boot_id":boot}),
        )?;
    }
    if resumed > 0 {
        lifecycle::event_tx(
            db,
            task,
            "LOOP_OBSERVED",
            json!({"key":key,"kind":"boot_recovery","node":null,"count":resumed + 1}),
        )?;
    }
    lifecycle::event_tx(
        db,
        task,
        "NATIVE_RECOVERY_RESUMED",
        json!({"state":state,"restart_resume":true,"restart_reason":reason,"revision":revision,
            "uncertainty":uncertainty,"lost_attempts":lost.len(),"replayed":false}),
    )?;
    Ok(RestartResume::Resumed)
}

/// Recovers one unfinished task from its durable checkpoints. Returns the task to
/// drive when nothing about it is unknown. A held task is recorded once: a restart
/// that finds it already held for the same reason adds no new event. `interrupted`:
/// this recovery settled the task's attempts because the service that ran them died
/// ([`settle_without_engine_tx`]); the task is held for the owner, never driven on.
#[allow(clippy::too_many_arguments)]
fn recover_task_tx(
    db: &rusqlite::Transaction<'_>,
    task: &str,
    state: &str,
    owner: String,
    epoch: i64,
    legacy: bool,
    interrupted: bool,
    boot: Option<&str>,
) -> CoreResult<Option<(String, OwnerEpoch, TaskState)>> {
    let state: TaskState = state.parse().map_err(CoreError::Refused)?;
    let owner = if legacy {
        let adopted = ownership::acquire_tx(db, &task, "native-recovery", Some(epoch))?;
        // Preserve creation provenance; ownership transfer is a separate event.
        lifecycle::event_tx(
            db,
            &task,
            "NATIVE_TASK_ADOPTED",
            json!({
                "execution_host":"rust","previous_host":"python","previous_owner":owner,
                "previous_epoch":epoch,"owner_epoch":adopted.epoch,"replayed":false}),
        )?;
        db.execute(
            "UPDATE workspace_executions SET owner_epoch=?,version=version+1
                WHERE runtime_task_id=? AND owner_epoch=?",
            (adopted.epoch, &task, epoch),
        )?;
        for mut w in waits::projection_tx(db, &task)? {
            if matches!(w["status"].as_str(), Some("open" | "ready" | "resuming"))
                && w["owner_epoch"] == epoch
            {
                w["owner_epoch"] = json!(adopted.epoch);
                waits::change_tx(db, &mut w)?;
            }
        }
        adopted
    } else {
        OwnerEpoch { owner, epoch }
    };
    // Even a committed result does not establish that its process exited.
    // Keep reservations occupied until a real process observation settles them.
    let unknown: bool = db.query_row(
        "SELECT
            EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND status!='terminated')
            OR EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND state!='released')",
        (&task, &task),
        |r| r.get(0),
    )?;
    settle_legacy_starts_tx(db, task, boot)?;
    let legacy_effect_unknown: bool = db.query_row("SELECT EXISTS(
            SELECT 1 FROM runtime_events i WHERE i.task_id=?
            AND ((i.type='CHECK_EXECUTION_STARTED' AND NOT EXISTS(
                SELECT 1 FROM runtime_events f WHERE f.task_id=i.task_id AND f.id>i.id
                AND f.type IN ('CHECK_EXECUTION_FINISHED','CHECK_EXECUTION_ABORTED')
                AND json_extract(f.payload_json,'$.command_ref')=json_extract(i.payload_json,'$.command_ref')))
            OR (i.type='COMMAND_PROCESS_STARTED' AND NOT EXISTS(
                SELECT 1 FROM runtime_events f WHERE f.task_id=i.task_id AND f.id>i.id
                AND f.type='COMMAND_PROCESS_EXITED'
                AND json_extract(f.payload_json,'$.tool_call_ref')=json_extract(i.payload_json,'$.tool_call_ref')))))",
            [&task], |r|r.get(0))?;
    let verification_unknown: bool = db.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM runtime_requests i JOIN runtime_tasks t USING(task_id)
            WHERE i.task_id=? AND i.operation='native_verification_intent'
            AND json_extract(i.result_json,'$.revision')=t.revision
            AND NOT EXISTS(SELECT 1 FROM runtime_requests f WHERE f.task_id=i.task_id
                AND f.actor='native-verifier-finish' AND f.request_key=i.request_key))",
        [&task],
        |r| r.get(0),
    )?;
    db.execute(
        "UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND revoked_at IS NULL",
        (cortex_core::now(), &task),
    )?;
    db.execute(
        "UPDATE delegation_provisioning SET status='ambiguous',updated_at=?
            WHERE task_id=? AND status!='terminated'",
        (cortex_core::now(), &task),
    )?;
    db.execute("UPDATE capacity_reservations SET state='ambiguous',
            release_observation='termination_unconfirmed',updated_at=? WHERE task_id=? AND state!='released'",
            (cortex_core::now(),&task))?;
    let delivery_unknown = waits::projection_tx(db, &task)?
        .iter()
        .any(|w| w["status"] == "resuming" || w["reason"] == "answer_continuation_unconfirmed");
    waits::recover_deliveries_tx(db, &task)?;
    for mut w in waits::projection_tx(db, &task)? {
        // A wait an earlier restart already marked lost is not re-recorded.
        if matches!(w["status"].as_str(), Some("open" | "ready"))
            && !(w["continuation_required"] == true && w["reason"] == "native_session_lost")
        {
            w["continuation_required"] = json!(true);
            w["reason"] = json!("native_session_lost");
            waits::change_tx(db, &mut w)?;
        }
    }
    // Guidance can no longer reach the exact old session. Never replay it.
    let guidance=db.prepare("SELECT json_extract(q.payload_json,'$.request') FROM runtime_events q
            WHERE q.task_id=? AND q.type='NATIVE_GUIDANCE_QUEUED'
            AND NOT EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=q.task_id
                AND e.type='NATIVE_GUIDANCE_SETTLED'
                AND json_extract(e.payload_json,'$.request')=json_extract(q.payload_json,'$.request'))")?
            .query_map([&task], |r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    for request in guidance {
        lifecycle::event_tx(
            db,
            &task,
            "NATIVE_GUIDANCE_SETTLED",
            json!({"request":request,"delivered":false,"reason":"service_restarted"}),
        )?;
    }
    let control_unknown: bool = db.query_row("SELECT EXISTS(
            SELECT 1 FROM runtime_requests i WHERE i.task_id=? AND i.operation='native_desktop_control'
            AND NOT EXISTS(SELECT 1 FROM runtime_requests f WHERE f.task_id=i.task_id
                AND f.actor='native-desktop-finish' AND f.request_key=i.request_key))",
            [&task], |r|r.get(0))?;
    if control_unknown {
        // Surface each unfinished admission once. Its finish slot closes as an
        // explicit unknown outcome bound to the current revision, so it cannot
        // force RECOVERING again on every later restart. The same request keeps
        // replaying as `native_control_outcome_unknown`; Retry clears the hold.
        let (_, revision) = domain::task_tx(db, &task)?;
        let pending = db
            .prepare(
                "SELECT i.request_key,i.digest FROM runtime_requests i
                WHERE i.task_id=? AND i.operation='native_desktop_control'
                AND NOT EXISTS(SELECT 1 FROM runtime_requests f WHERE f.task_id=i.task_id
                    AND f.actor='native-desktop-finish' AND f.request_key=i.request_key)",
            )?
            .query_map([&task], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (request, digest) in pending {
            lifecycle::event_tx(
                db,
                &task,
                "NATIVE_CONTROL_OUTCOME_UNKNOWN",
                json!({"request":request,"reason":"service_restarted","revision":revision}),
            )?;
            db.execute("INSERT INTO runtime_requests VALUES (?,?,'native-desktop-finish','native_desktop_finished',?,?)",
                    (&task,&request,&digest,json!({"outcome":"unknown","native":true,
                        "reason":"service_restarted","revision":revision,
                        "settledAt":cortex_core::now()}).to_string()))?;
        }
    }
    let outside_unknown:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=?
            AND operation IN ('outside_command','outside_write','command_run','project_write','project_edit','project_mkdir','project_delete','project_skill_link','draft_write','external_call') AND json_extract(result_json,'$.complete') IS NOT 1 AND json_extract(result_json,'$.status') IS NOT 'complete' AND json_extract(result_json,'$.settled') IS NOT 1)",
            [&task],|r|r.get(0))?;
    let held: Option<String> = db.query_row(
        "SELECT blocker FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    // An earlier restart held the task only for a legacy command start that the
    // evidence now settles ([`settle_legacy_starts_tx`]). That barrier no longer
    // stands: the task is held for what its facts still show, and otherwise as the
    // interrupted turn it is (the owner's message, Retry, Resume and Close work).
    let legacy_settled = held.as_deref() == Some(LEGACY_EFFECT) && !legacy_effect_unknown;
    let reason = if delivery_unknown {
        Some("native_answer_delivery_outcome_unknown")
    } else if legacy_effect_unknown {
        Some(LEGACY_EFFECT)
    } else if outside_unknown {
        Some("native_outside_outcome_unknown")
    } else if verification_unknown {
        Some("native_verification_outcome_unknown")
    } else if control_unknown {
        Some("native_control_outcome_unknown")
    } else if unknown
        || legacy_settled
        || (interrupted
            && (state == TaskState::WaitingUser
                || waits::projection_tx(db, &task)?
                    .iter()
                    .any(|w| matches!(w["status"].as_str(), Some("open" | "ready")))))
    {
        // An interrupted turn with a question or wait keeps that wait's authority (a
        // barrier); any other interrupted turn continues below.
        Some("native_session_outcome_unknown")
    } else {
        None
    };
    // A restart is infrastructure: a plan being planned, executed, reviewed or
    // verified resumes in its state on the same rung without a new revision once its
    // processes are confirmed gone ([`restart_resume_tx`]). A verification run the
    // restart interrupted is settled as unknown and rerun by the runtime when the
    // driver verifies again (`completion::settle_interrupted_verification_tx`), never
    // a continuation. Answers, legacy starts, desktop controls, questions and waits
    // keep their own rules.
    let restart_reason = match reason {
        Some(
            r @ ("native_session_outcome_unknown"
            | "native_outside_outcome_unknown"
            | "native_verification_outcome_unknown"),
        ) => Some(r),
        None if interrupted => Some("native_service_interrupted"),
        _ => None,
    };
    if let Some(restart_reason) = restart_reason.filter(|r| {
        !legacy
            && match state {
                TaskState::Planning | TaskState::Executing | TaskState::Reviewing => {
                    *r != "native_verification_outcome_unknown"
                }
                TaskState::Verifying => true,
                _ => false,
            }
    }) {
        let waiting = waits::projection_tx(db, &task)?
            .iter()
            .any(|w| matches!(w["status"].as_str(), Some("open" | "ready" | "resuming")));
        if !waiting {
            match restart_resume_tx(db, task, state, restart_reason, boot)? {
                RestartResume::Resumed => return Ok(Some((task.to_owned(), owner, state))),
                RestartResume::Paused => return Ok(None),
                RestartResume::NotApplicable => {}
            }
        }
    }
    if let Some(reason) = reason {
        // No-stop policy (every task): the unknown outcome becomes a continuation that
        // records the uncertainty for the next coordinator, which re-verifies it once
        // the host has settled the processes. Only a paused or waiting task, or one
        // with an open wait, keeps the barrier for its own authority.
        let was_held = lifecycle::automatic::held(db, task)?;
        if lifecycle::automatic::continue_unsettled_tx(db, task, reason)? {
            if !was_held {
                let mut required =
                    json!({"reason":reason,"previous":state,"replayed":false,"continued":true});
                if legacy_settled {
                    required["settled_barrier"] = json!(LEGACY_EFFECT);
                }
                recovery_required_once_tx(db, task, required)?;
            }
            return Ok(None);
        }
        // Already held for exactly this (the effective) reason in the state it
        // would be held in: an earlier restart recorded it; record nothing again.
        let effective = match held.as_deref() {
            Some(existing) if existing.ends_with("outcome_unknown") && !legacy_settled => existing,
            _ => reason,
        };
        let target = if matches!(state, TaskState::Paused | TaskState::WaitingUser) {
            state
        } else {
            TaskState::Recovering
        };
        if !(state == target && held.as_deref() == Some(effective)) {
            let mut required = json!({"reason":reason,"previous":state,"replayed":false});
            if legacy_settled {
                required["settled_barrier"] = json!(LEGACY_EFFECT);
                recovery_required_once_tx(db, task, required)?;
                domain::native_barrier_settled_tx(db, &task, LEGACY_EFFECT, reason)?;
            } else {
                recovery_required_once_tx(db, task, required)?;
                domain::native_recovery_required_tx(db, &task, reason)?;
            }
        }
    } else if interrupted && !matches!(state, TaskState::Paused | TaskState::WaitingUser) {
        domain::stop_tx(db, task, "native_service_interrupted")?;
    } else if matches!(
        state,
        TaskState::Planning
            | TaskState::PlanReady
            | TaskState::Executing
            | TaskState::Reviewing
            | TaskState::Verifying
            | TaskState::Reconciling
            | TaskState::WaitingUser
            | TaskState::AnswerReady
    ) {
        lifecycle::event_tx(db, &task, "NATIVE_RECOVERY_RESUMED", json!({"state":state}))?;
        return Ok(Some((task.to_owned(), owner, state)));
    } else if state == TaskState::Recovering && descriptive_command_hold_tx(db, task)? {
        // Earlier builds held a verified, synthesized result as interrupted when a
        // worker's descriptive check command differed from its receipt. Completion
        // no longer refuses that, so the held result is checked again with every
        // evidence gate; a real gap takes the coordinator's stop path. A refused
        // settle (an unconfirmed worker) leaves the hold exactly as it was.
        if isolated(db, || {
            domain::settle_recovery_tx(db, task, TaskState::Reconciling)
        })
        .is_ok()
        {
            lifecycle::event_tx(
                db,
                &task,
                "NATIVE_RECOVERY_RESUMED",
                json!({"state":TaskState::Reconciling,"reason":"completion_rechecked"}),
            )?;
            return Ok(Some((task.to_owned(), owner, TaskState::Reconciling)));
        }
    } else if state == TaskState::Recovering {
        // Earlier builds stopped a task for a work blocker (a lost worker whose retry
        // gave up, a failed check) into RECOVERING to wait for a Retry that no screen
        // offers, and nothing ever asked the owner. Ask now, with the runtime's own
        // stop question and its concrete answers, in the task's language.
        if let Some(blocker) = unasked_stop_tx(db, task)? {
            domain::stop_tx(db, task, &blocker)?;
        }
    }
    Ok(None)
}

/// The blocker of a work stop that returned the task to RECOVERING for the owner
/// (`ASK_USER_REQUIRED`) when nothing followed it: no coordinator turn, question,
/// answer, message, interruption or continuation, no open question and no held reason.
fn unasked_stop_tx(db: &rusqlite::Transaction<'_>, task: &str) -> CoreResult<Option<String>> {
    use rusqlite::OptionalExtension;
    Ok(db.query_row(
        "SELECT json_extract(a.payload_json,'$.blocker') FROM runtime_events a JOIN runtime_tasks t ON t.task_id=a.task_id
         WHERE a.task_id=?1 AND a.type='ASK_USER_REQUIRED' AND t.state='RECOVERING' AND coalesce(t.blocker,'')=''
         AND json_type(a.payload_json,'$.blocker')='text' AND trim(json_extract(a.payload_json,'$.blocker'))!=''
         AND a.id=(SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='ASK_USER_REQUIRED')
         AND NOT EXISTS(SELECT 1 FROM runtime_events x WHERE x.task_id=?1 AND x.id>a.id AND x.type IN
            ('USER_QUESTION_REQUESTED','USER_ANSWER_RECEIVED','USER_STEERING','TURN_INTERRUPTED','NATIVE_COORDINATOR_CONTINUED',
             'NATIVE_TASK_CONTINUED','NATIVE_RECOVERY_RESUMED','PLAN_ACCEPTED','OWNER_TASK_PAUSED','OWNER_TASK_RESUMED'))
         AND NOT EXISTS(SELECT 1 FROM pending_questions WHERE task_id=?1 AND status='open')",
        [task],
        |r| r.get::<_, String>(0),
    ).optional()?)
}

/// A recovery hold that only the retired descriptive-command completion gate set:
/// a synthesized result whose latest verification passed, held by exactly that code.
fn descriptive_command_hold_tx(db: &rusqlite::Transaction<'_>, task: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT t.blocker='completion_worker_check_unverified' AND coalesce(trim(t.synthesis),'')!=''
            AND (SELECT type FROM runtime_events WHERE task_id=t.task_id
                AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1)='VERIFICATION_PASSED'
         FROM runtime_tasks t WHERE t.task_id=?",
        [task],
        |r| r.get::<_, Option<bool>>(0),
    )?
    .unwrap_or(false))
}

pub(super) const SYSTEM_PROC: &str = "/proc";

impl Runtime {
    pub(crate) async fn recover(self: &Arc<Self>) -> CoreResult<()> {
        self.recover_from(std::path::Path::new(SYSTEM_PROC)).await?;
        // Continuations this recovery recorded (or found) run without a scheduler tick.
        self.follow_owed_continuations().await
    }

    /// Called once after acquiring the project scheduler lease, before publication.
    pub(super) async fn recover_from(
        self: &Arc<Self>,
        engine_proc: &std::path::Path,
    ) -> CoreResult<()> {
        self.host
            .executor
            .publication(|store| {
                crate::native_project::recover_files(store)?;
                crate::native_commands::recover_commands(store)
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        crate::native_command_handles::recover(self).await?;
        // No call of an earlier service can still act: settle every orphaned command
        // receipt whose process is provably gone, before the tasks' effects are judged.
        let busy: std::collections::BTreeSet<String> =
            self.host.commands.lock().await.keys().cloned().collect();
        if let Ok(reply) = self
            .host
            .executor
            .publication(move |store| crate::native_command_handles::resettle(store, &busy, true))
        {
            if let Ok(Err(error)) = reply.await {
                eprintln!(
                    "workspace-service: command receipts were not re-settled ({})",
                    error.code()
                );
            }
        }
        if let Ok(key) = crate::native_runtime::key(&self.host.root) {
            remove_stale_brokers(&key.directory());
        }
        let root = self.host.root.clone();
        let engine_proc = engine_proc.to_owned();
        // The retired Python host could leave a desktop start as `unknown` after its runtime
        // task had been created. Native starts are written atomically as `claimed` and never
        // `unknown`. When the execution is validly bound to an existing runtime task (same
        // owner epoch, session and project root), the start provably happened: record it.
        let (mut failures, interrupted) = self
            .host
            .executor
            .write(move |db| {
                let settled = db
                    .prepare(
                        "SELECT s.execution_id,e.runtime_task_id FROM workspace_starts s
                     JOIN workspace_executions e USING(execution_id)
                     JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
                     JOIN tasks base ON base.id=r.task_id
                     JOIN task_owners o ON o.task_id=r.task_id
                     JOIN acp_host_sessions h ON h.session_id=e.acp_session_id
                     WHERE s.state='unknown' AND e.owner_epoch=o.epoch AND h.task_id=r.task_id
                     AND h.project_root=base.project_root AND r.revision>=0",
                    )?
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
                    .collect::<Result<Vec<_>, _>>()?;
                let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                    .map(|b| b.trim().to_owned())
                    .unwrap_or_default();
                // Each rule settles per task; a task whose writes fail is rolled back
                // alone and reported, and the project still opens.
                let mut failures = Vec::new();
                if !boot.is_empty() {
                    settle_registry_tx(db, &boot, &mut failures)?;
                    settle_registered_tx(db, &boot, &mut failures)?;
                }
                settle_released_unstarted_tx(db, &mut failures)?;
                settle_unlaunched_tx(db, &mut failures)?;
                let interrupted = settle_without_engine_tx(db, &root, &engine_proc, &mut failures)?;
                if !boot.is_empty() {
                    settle_previous_boot_tx(db, &boot, &mut failures)?;
                }
                for (execution, task) in settled {
                    let done = isolated(db, || {
                        db.execute(
                            "UPDATE workspace_starts SET state='dispatched' WHERE execution_id=? AND state='unknown'",
                            [&execution],
                        )?;
                        lifecycle::event_tx(
                            db,
                            &task,
                            "NATIVE_START_RECONCILED",
                            json!({"execution_id":execution,"previous_state":"unknown",
                                "evidence":"bound_runtime_task","previous_host":"python"}),
                        )
                    });
                    if let Err(error) = done {
                        failures.push((task, error.code().to_owned()));
                    }
                }
                adopt_finished_tx(db, &mut failures)?;
                Ok((failures, interrupted))
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        let (tasks, failures) = self
            .host
            .executor
            .write(move |db| {
                // The boot this recovery runs in: evidence that a process of another one has ended.
                let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
                    .map(|b| b.trim().to_owned())
                    .ok()
                    .filter(|b| !b.is_empty());
                let rows = db
                    .prepare(
                        "SELECT t.task_id,t.state,o.owner_ref,o.epoch,
                NOT EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=t.task_id
                    AND e.type IN ('TASK_CREATED','NATIVE_TASK_ADOPTED')
                    AND json_extract(e.payload_json,'$.execution_host')='rust')
                FROM runtime_tasks t JOIN task_owners o USING(task_id)
                WHERE t.state NOT IN ('COMPLETED','CANCELLED','FAILED')
                ORDER BY t.rowid",
                    )?
                    .query_map([], |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, String>(2)?,
                            r.get::<_, i64>(3)?,
                            r.get::<_, bool>(4)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut ready = Vec::new();
                let mut unrecovered = Vec::new();
                for (task, state, owner, epoch, legacy) in rows {
                    // One task's recovery never fails the project: its writes roll back alone.
                    let held = interrupted.contains(&task);
                    match isolated(db, || {
                        recover_task_tx(
                            db,
                            &task,
                            &state,
                            owner,
                            epoch,
                            legacy,
                            held,
                            boot.as_deref(),
                        )
                    }) {
                        Ok(Some(driven)) => ready.push(driven),
                        Ok(None) => {}
                        Err(error) => unrecovered.push((task, error.code().to_owned())),
                    }
                }
                // A task whose recovery failed is held (never driven) with that reason, once.
                for (task, code) in &unrecovered {
                    let _ = isolated(db, || {
                        let held: Option<String> = db.query_row(
                            "SELECT blocker FROM runtime_tasks WHERE task_id=?",
                            [task],
                            |r| r.get(0),
                        )?;
                        if held.as_deref() == Some("native_recovery_failed") {
                            return Ok(());
                        }
                        // No-stop policy: continue it with a fresh plan when the store
                        // still accepts a continuation; otherwise hold it with the reason.
                        let continued = isolated(db, || {
                            if lifecycle::automatic::held(db, task)? {
                                return Ok(true);
                            }
                            domain::stop_tx(db, task, "native_recovery_failed")?;
                            lifecycle::automatic::held(db, task)
                        });
                        if continued.unwrap_or(false) {
                            return Ok(());
                        }
                        lifecycle::event_tx(
                            db,
                            task,
                            "NATIVE_RECOVERY_REQUIRED",
                            json!({"reason":"native_recovery_failed","code":code,"replayed":false}),
                        )?;
                        domain::native_recovery_required_tx(db, task, "native_recovery_failed")
                    });
                }
                failures.extend(unrecovered);
                Ok((ready, failures))
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        for (task, code) in failures {
            eprintln!("workspace-service: recovery left task {task} unsettled ({code})");
        }
        for (task, owner, state) in tasks {
            let (cancel, cancelled) = tokio::sync::watch::channel(false);
            let (done, finished) = tokio::sync::watch::channel(false);
            self.install_pump(task.clone(), cancel, finished).await;
            let runtime = self.clone();
            self.spawn_recovery(async move {
                let result = if state == TaskState::Planning {
                    runtime
                        .recover_planning(&task, &owner, cancelled.clone())
                        .await
                } else {
                    runtime.progress(&task, &owner, cancelled.clone()).await
                };
                if let Err(error) = result {
                    // The same path as on a live driver, for every task: the stop is a
                    // continuation on the next rung of the strategy ladder (no-stop
                    // policy), never an "interrupted" hold for the owner.
                    if !*cancelled.borrow() && !*runtime.stop.borrow() {
                        runtime
                            .stop_after_error(&task, &owner, error, cancelled.clone())
                            .await;
                    }
                }
                runtime.driver_ended(&task, &cancelled).await;
                done.send_replace(true);
            })
            .await;
        }
        Ok(())
    }
}

/// Live re-check of unconfirmed exits. A session whose shutdown this service could
/// not confirm leaves its rows `ambiguous` ([`crate::native_sessions`]); startup
/// recovery is no longer the only place that settles them.
impl Runtime {
    /// Called on the sprint scheduler's passes (its safety tick and wake-ups). Returns
    /// whether unconfirmed attempts remain to be looked at again.
    pub(crate) async fn resettle_unconfirmed_exits(self: &Arc<Self>) -> CoreResult<bool> {
        // Orphaned command receipts are re-settled on the same passes (own backoff).
        let _ = self.resettle_orphaned_commands().await;
        self.resettle_unconfirmed_exits_from(std::path::Path::new(SYSTEM_PROC))
            .await
    }

    /// Whether any attempt qualifies for the live re-check (one read, no process scan).
    pub(crate) async fn unconfirmed_exits_pending(&self) -> CoreResult<bool> {
        let attempts = self
            .host
            .executor
            .read(|db| unconfirmed_attempts(db))?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        Ok(!attempts.is_empty())
    }

    /// Settles [`unconfirmed_attempts`] with the same process evidence, writes and
    /// event as startup recovery's settlement without an engine
    /// ([`no_engine_process_tx`], [`settle_without_engine_attempts_tx`]; evidence
    /// `unconfirmed_exit_and_no_engine_process_in_project`), in one transaction that
    /// re-reads the rows. It never signals, kills or relaunches anything. While an
    /// engine process may still work in the project nothing changes and the check
    /// backs off (15 s, doubling to 10 min; a changed set of attempts is looked at at
    /// once). A settled task that no driver runs and that only that exit held, in
    /// Automatic, continues like any automatic stop
    /// ([`lifecycle::automatic::exit_settled_tx`]); any other task keeps its state and
    /// is an ordinary interrupted turn for its owner, and an answered stop question
    /// owed a relaunch gets it.
    pub(super) async fn resettle_unconfirmed_exits_from(
        self: &Arc<Self>,
        engine_proc: &std::path::Path,
    ) -> CoreResult<bool> {
        if *self.stop.borrow() {
            return Ok(false);
        }
        let attempts = self
            .host
            .executor
            .read(|db| unconfirmed_attempts(db))?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        {
            let mut recheck = self.exit_recheck.lock().unwrap_or_else(|e| e.into_inner());
            if attempts.is_empty() {
                *recheck = ExitRecheck::default();
                return Ok(false);
            }
            if recheck.attempts != attempts {
                *recheck = ExitRecheck {
                    attempts: attempts.clone(),
                    ..ExitRecheck::default()
                };
            }
            if recheck
                .next
                .is_some_and(|at| std::time::Instant::now() < at)
            {
                return Ok(true);
            }
        }
        // The serial lane of automatic continuation and owner controls: no driver of
        // these tasks is started by them while this pass decides.
        let serial = self.controls.lock().await;
        let mut driven = std::collections::BTreeSet::new();
        for (task, _) in &attempts {
            if self.driven(task).await {
                driven.insert(task.clone());
            }
        }
        let root = self.host.root.clone();
        let engine_proc = engine_proc.to_owned();
        // The attempts still unconfirmed after this pass, and whether any was settled.
        let (remaining, settled) = self
            .host
            .executor
            .write(move |db| {
                let open = unconfirmed_attempts(db)?;
                if open.is_empty() || !no_engine_process_tx(db, &root, &engine_proc)? {
                    return Ok((open, false));
                }
                let mut failures = Vec::new();
                let settled = settle_without_engine_attempts_tx(
                    db,
                    open,
                    "unconfirmed_exit_and_no_engine_process_in_project",
                    &mut failures,
                )?;
                for task in settled.iter().filter(|t| !driven.contains(*t)) {
                    // A refusal keeps the task exactly as it was, for its owner.
                    let _ = isolated(db, || lifecycle::automatic::exit_settled_tx(db, task));
                }
                Ok((unconfirmed_attempts(db)?, !settled.is_empty()))
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        drop(serial);
        let pending = !remaining.is_empty();
        {
            let mut recheck = self.exit_recheck.lock().unwrap_or_else(|e| e.into_inner());
            if pending {
                recheck.attempts = remaining;
                recheck.delay = if recheck.delay.is_zero() {
                    RECHECK_FIRST
                } else {
                    (recheck.delay * 2).min(RECHECK_MAX)
                };
                recheck.next = Some(std::time::Instant::now() + recheck.delay);
            } else {
                *recheck = ExitRecheck::default();
            }
        }
        if settled {
            let _ = self.continue_owed_tasks().await;
        }
        Ok(pending)
    }
}

#[cfg(test)]
#[path = "native_recovery_legacy_tests.rs"]
mod legacy_tests;

// Recovery unit tests exercise the same scanner against per-project proc entries.
#[cfg(test)]
pub(super) mod fixture {
    use super::*;
    use std::path::{Path, PathBuf};

    pub(super) fn engine_proc(project: &Path) -> PathBuf {
        let proc_root = project.join("fixture-proc");
        std::fs::create_dir_all(&proc_root).unwrap();
        proc_root
    }

    pub(crate) fn engine(project: &Path, cwd: &Path) -> PathBuf {
        let pid = if std::process::id() == 1 { "2" } else { "1" };
        let entry = engine_proc(project).join(pid);
        std::fs::create_dir(&entry).unwrap();
        std::fs::write(entry.join("cmdline"), b"codex-acp\0").unwrap();
        std::os::unix::fs::symlink(cwd, entry.join("cwd")).unwrap();
        entry
    }

    impl Runtime {
        pub(crate) async fn recover_fixture(self: &Arc<Self>) -> CoreResult<()> {
            self.recover_from(&engine_proc(&self.host.root)).await
        }
        /// One scheduler pass's re-check of unconfirmed exits, against the fixture table.
        pub(crate) async fn resettle_fixture(self: &Arc<Self>) -> CoreResult<bool> {
            self.resettle_unconfirmed_exits_from(&engine_proc(&self.host.root))
                .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{native_runtime::UnqualifiedRoutes, Config, LaunchSpec, Service};
    use std::{
        future::Future,
        os::unix::fs::PermissionsExt,
        task::{Context, Poll, Waker},
        time::Duration,
    };

    #[test]
    fn engine_process_probe_excludes_unrelated_roots_but_not_unreadable_cwd() {
        let scratch =
            std::env::temp_dir().join(format!("engine-probe-{}", crate::identity().unwrap()));
        let root = scratch.join("project");
        let entry = fixture::engine(&root, &scratch);
        let proc_root = fixture::engine_proc(&root);
        assert!(!engine_process_in(&proc_root, std::slice::from_ref(&root)));

        // A non-symlink cwd yields EINVAL, not process-disappeared evidence.
        let cwd = entry.join("cwd");
        std::fs::remove_file(&cwd).unwrap();
        std::fs::write(&cwd, b"not a symlink").unwrap();
        assert!(engine_process_in(&proc_root, std::slice::from_ref(&root)));
        assert!(engine_process_in(&proc_root.join("missing"), &[root]));
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[tokio::test]
    async fn legacy_adoption_is_fenced_and_never_replays_unknown_effects() {
        use cortex_core::{lifecycle::TaskMachine, ownership::SchedulerOwner, OpenMode, Store};
        let root =
            std::env::temp_dir().join(format!("legacy-adoption-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        crate::project_guard::reserve(&project).unwrap();
        let key = crate::native_runtime::key(&project).unwrap();
        let store = Store::open(key.clone(), OpenMode::Initialize).unwrap();
        for (task, state) in [
            ("planning", "PLANNING"),
            ("worker", "EXECUTING"),
            ("review", "REVIEWING"),
            ("verification", "VERIFYING"),
            ("command", "PLANNING"),
            ("claimed", "EXECUTING"),
            ("outside", "EXECUTING"),
            ("paused", "PAUSED"),
            ("terminal", "COMPLETED"),
        ] {
            TaskMachine::new(&store)
                .create(task, "Fixture", "python-owner")
                .unwrap();
            store.write(|db| {
                db.execute("UPDATE runtime_tasks SET state=? WHERE task_id=?",(state,task))?;
                if ["planning","worker","review"].contains(&task) {
                    db.execute("INSERT INTO delegation_provisioning
                        (idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at)
                        VALUES (?,?,?,'worker-backend-dev','ready','ready',1,'fixture','fixture')",
                        (task,task,task))?;
                    db.execute("INSERT INTO capacity_reservations VALUES (?,?,?,NULL,?,'worker','reserved','fixture','fixture',NULL)",
                        (task,"fake",task,task))?;
                }
                if task=="verification" {
                    lifecycle::event_tx(db,task,"CHECK_EXECUTION_STARTED",json!({"command_ref":"lost-check","revision":0}))?;
                }
                if task=="command" {
                    lifecycle::event_tx(db,task,"COMMAND_PROCESS_STARTED",json!({"tool_call_ref":"lost-command"}))?;
                }
                if task=="claimed" {
                    let mut wait=json!({"wait_id":"lost-delivery","task_id":task,"assignment_ref":"a",
                        "status":"resuming","owner_epoch":1,"delivery_id":"delivery"});
                    waits::change_tx(db,&mut wait)?;
                }
                if task=="outside" {
                    db.execute("INSERT INTO runtime_requests VALUES (?,'lost','channel','outside_write','fixture','{}')",[task])?;
                }
                Ok(())
            }).unwrap();
        }
        let service = Service::bind_native(
            Config {
                runtime_dir: root.join("runtime"),
                state_dir: root.join("state"),
                allowed_root: root.clone(),
                launch: LaunchSpec::bare(),
                request_timeout: Duration::from_secs(2),
                shutdown_timeout: Duration::from_secs(1),
                event_capacity: 32,
            },
            Arc::new(UnqualifiedRoutes),
        )
        .await
        .unwrap();
        let registered = service.register(&project, "Legacy").await.unwrap();
        let id = registered["projectId"].as_str().unwrap();
        let lease = SchedulerOwner::acquire(&key).unwrap();
        let engine_proc = fixture::engine_proc(&project);
        assert_eq!(
            service
                .activate_inner(id, &engine_proc)
                .await
                .unwrap_err()
                .to_string(),
            "scheduler_owner_busy"
        );
        store
            .read(|db| {
                assert_eq!(
                    db.query_row(
                        "SELECT count(*) FROM runtime_events WHERE type='NATIVE_TASK_ADOPTED'",
                        [],
                        |r| r.get::<_, i64>(0)
                    )?,
                    0
                );
                Ok(())
            })
            .unwrap();
        drop(lease);
        assert_eq!(
            service.activate_inner(id, &engine_proc).await.unwrap()["executionHost"],
            "rust"
        );
        service.stop().await.unwrap();
        store.read(|db| {
            for (task,reason) in [("planning","native_session_outcome_unknown"),("worker","native_session_outcome_unknown"),
                ("review","native_session_outcome_unknown"),("verification","legacy_effect_outcome_unknown"),
                ("command","legacy_effect_outcome_unknown"),("claimed","native_answer_delivery_outcome_unknown"),
                ("outside","native_outside_outcome_unknown")] {
                let (state,blocker,epoch):(String,String,i64)=db.query_row(
                    "SELECT t.state,t.blocker,o.epoch FROM runtime_tasks t JOIN task_owners o USING(task_id) WHERE t.task_id=?",
                    [task],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
                // No-stop policy: the unknown outcome is a continuation that carries it
                // for the coordinator, never a hold for the owner.
                assert_eq!(state,"RECOVERING");
                assert_eq!(blocker,"automatic_continuation_pending","{task}");
                assert_eq!(epoch,2);
                let continued:String=db.query_row(
                    "SELECT json_extract(payload_json,'$.reason') FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_CONTINUATION' ORDER BY id DESC LIMIT 1",
                    [task],|r|r.get(0))?;
                assert!(continued==reason || reason=="native_session_outcome_unknown" && continued=="native_service_interrupted","{task}: {continued}");
            }
            assert_eq!(db.query_row("SELECT count(*) FROM delegation_provisioning",[],|r|r.get::<_,i64>(0))?,3);
            // The host that ran them is gone and no engine works in the project: the
            // attempts are settled, and their tasks are held as interrupted, not driven.
            assert_eq!(db.query_row("SELECT count(*) FROM delegation_provisioning WHERE status='terminated'",[],|r|r.get::<_,i64>(0))?,3);
            assert_eq!(db.query_row("SELECT count(*) FROM capacity_reservations WHERE state='released' AND release_observation='no_engine_process'",[],|r|r.get::<_,i64>(0))?,3);
            assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_RECOVERY_RESUMED'",[],|r|r.get::<_,i64>(0))?,0);
            assert_eq!(db.query_row("SELECT state FROM runtime_tasks WHERE task_id='paused'",[],|r|r.get::<_,String>(0))?,"PAUSED");
            assert_eq!(db.query_row("SELECT epoch FROM task_owners WHERE task_id='terminal'",[],|r|r.get::<_,i64>(0))?,1);
            Ok(())
        }).unwrap();
        // A second recovery records neither another adoption nor a new epoch.
        service
            .state
            .lock()
            .await
            .native
            .get(id)
            .unwrap()
            .recover_fixture()
            .await
            .unwrap();
        store.read(|db| {
            assert_eq!(db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_TASK_ADOPTED'",[],|r|r.get::<_,i64>(0))?,8);
            assert!(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='WORKER_WAIT_CHANGED' AND json_extract(payload_json,'$.status')='orphaned')",[],|r|r.get::<_,bool>(0))?);
            Ok(())
        }).unwrap();
        drop(service);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn stop_fences_a_native_activation_waiting_to_open_its_owner() {
        let root =
            std::env::temp_dir().join(format!("native-activation-stop-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        let service = Service::bind_native(
            Config {
                runtime_dir: root.join("runtime"),
                state_dir: root.join("state"),
                allowed_root: root.clone(),
                launch: LaunchSpec::bare(),
                request_timeout: Duration::from_secs(2),
                shutdown_timeout: Duration::from_secs(1),
                event_capacity: 32,
            },
            Arc::new(UnqualifiedRoutes),
        )
        .await
        .unwrap();
        let registered = service.register(&project, "Late activation").await.unwrap();
        let id = registered["projectId"].as_str().unwrap();
        {
            // Hold the actual owner-opening boundary, then queue both public actions.
            // Polling gives deterministic order; elapsed sleep is not the evidence.
            let gate = service.native_activation.write().await;
            let activation = service.activate(id);
            tokio::pin!(activation);
            let mut cx = Context::from_waker(Waker::noop());
            assert!(matches!(activation.as_mut().poll(&mut cx), Poll::Pending));
            let stop = service.stop();
            tokio::pin!(stop);
            assert!(
                matches!(stop.as_mut().poll(&mut cx), Poll::Pending),
                "Stop returned while a native activation could still open an owner"
            );
            drop(gate);
            tokio::time::timeout(Duration::from_secs(2), async {
                let (activation, stop) = tokio::join!(&mut activation, &mut stop);
                assert_eq!(activation.unwrap_err().to_string(), "service_stopping");
                stop.unwrap();
            })
            .await
            .unwrap();
            assert!(service.state.lock().await.native.is_empty());
            assert!(
                !project
                    .join(if workspace_protocol::channel::current().is_dev() {
                        ".cortex-dev"
                    } else {
                        ".cortex"
                    })
                    .join("cortex.sqlite3")
                    .exists(),
                "late activation opened a database after Stop"
            );
            drop(activation);
            drop(stop);
        }
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test]
    async fn orphaned_command_receipts_settle_at_startup_and_on_live_passes() {
        use cortex_core::{lifecycle::TaskMachine, OpenMode, Store};
        let root =
            std::env::temp_dir().join(format!("command-orphans-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let project = root.join("project");
        std::fs::create_dir(&project).unwrap();
        crate::project_guard::reserve(&project).unwrap();
        let key = crate::native_runtime::key(&project).unwrap();
        let store = Store::open(key, OpenMode::Initialize).unwrap();
        TaskMachine::new(&store)
            .create("held", "Fixture", "owner")
            .unwrap();
        let orphan = |store: &Store, key: &str| {
            store
                .write(|db| {
                    db.execute("INSERT INTO runtime_requests VALUES ('held',?,'ended-channel','command_run','d',?)",
                        (key, json!({"complete":false,"tool_call_ref":format!("native-command:{key}"),
                            "argv":["npm","run","typecheck"]}).to_string()))?;
                    Ok(())
                })
                .unwrap()
        };
        let settled = |store: &Store, key: &str| {
            store
                .read(|db| {
                    Ok(db.query_row("SELECT json_extract(result_json,'$.settled') IS 1 FROM runtime_requests WHERE request_key=?",
                        [key], |r| r.get::<_, bool>(0))?)
                })
                .unwrap()
        };
        store
            .write(|db| {
                db.execute(
                    "UPDATE runtime_tasks SET state='PAUSED' WHERE task_id='held'",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        orphan(&store, "before-restart");
        let service = Service::bind_native(
            Config {
                runtime_dir: root.join("runtime"),
                state_dir: root.join("state"),
                allowed_root: root.clone(),
                launch: LaunchSpec::bare(),
                request_timeout: Duration::from_secs(2),
                shutdown_timeout: Duration::from_secs(1),
                event_capacity: 32,
            },
            Arc::new(UnqualifiedRoutes),
        )
        .await
        .unwrap();
        let registered = service.register(&project, "Orphans").await.unwrap();
        let id = registered["projectId"].as_str().unwrap();
        service
            .activate_inner(id, &fixture::engine_proc(&project))
            .await
            .unwrap();
        // Startup settles the receipt before the task's effects are judged.
        assert!(settled(&store, "before-restart"));
        let blocker: Option<String> = store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT blocker FROM runtime_tasks WHERE task_id='held'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_ne!(blocker.as_deref(), Some("native_outside_outcome_unknown"));
        let runtime = service.state.lock().await.native.get(id).unwrap().clone();
        // A receipt orphaned while the service runs (its call or channel ended).
        orphan(&store, "while-running");
        let flight = crate::native_commands::Flight::new("native-command:in-flight");
        orphan(&store, "in-flight");
        assert_eq!(runtime.resettle_orphaned_commands().await.unwrap(), 1);
        assert!(settled(&store, "while-running"));
        assert!(
            !settled(&store, "in-flight"),
            "a call still in flight is left alone"
        );
        drop(flight);
        assert_eq!(runtime.resettle_orphaned_commands().await.unwrap(), 1);
        assert!(settled(&store, "in-flight"));
        store
            .read(|db| {
                cortex_core::lifecycle::automatic::require_settled_execution(db, "held")?;
                assert_eq!(db.query_row("SELECT count(*) FROM runtime_requests WHERE json_extract(result_json,'$.failure')='command_not_started'
                    AND json_extract(result_json,'$.complete') IS 0",[],|r|r.get::<_,i64>(0))?, 3);
                Ok(())
            })
            .unwrap();
        assert_eq!(runtime.resettle_orphaned_commands().await.unwrap(), 0);
        drop(runtime);
        service.stop().await.unwrap();
        drop(service);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod restart_tests {
    use super::*;
    use cortex_core::{lifecycle::TaskMachine, OpenMode, ProjectKey, Store, StoreChannel};
    use std::os::unix::fs::PermissionsExt;
    fn empty_store() -> (std::path::PathBuf, Store) {
        let root = std::env::temp_dir().join(format!(
            "native-restart-resume-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        TaskMachine::new(&store)
            .create("t", "goal", "owner")
            .unwrap();
        (root, store)
    }
    fn store() -> (std::path::PathBuf, Store) {
        let (root, store) = empty_store();
        store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=4", [])?;
                // A worker the dead service ran, settled by this recovery: no result.
                db.execute("INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status,assignment_ref)
                    VALUES ('t',4,'impl','worker-backend-dev','work','[]','[\"src\"]','execution','running','a1')",[])?;
                db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,channel_id,engine_session_ref,owner_epoch,created_at,updated_at)
                    VALUES ('a1','t','x1','worker-backend-dev','ready','terminated','c1','es','1','now','now')",[])?;
                db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,assignment_id,attempt_id,owner_epoch,issued_at)
                    VALUES ('c1','fixture','t','thread','worker','a1','x1',1,'now')",[])?;
                // A write the gone session left without a final receipt.
                db.execute("INSERT INTO runtime_requests VALUES ('t','w1','c1','project_write','digest','{\"path\":\"src/lib.rs\"}')",[])?;
                Ok(())
            })
            .unwrap();
        (root, store)
    }
    fn count(store: &Store, kind: &str) -> i64 {
        store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE task_id='t' AND type=?",
                    [kind],
                    |r| r.get(0),
                )?)
            })
            .unwrap()
    }
    /// Owner stores 2026-11-21 13:54 (cortex 323970-323984, Youtube 49909): a restart
    /// turned every interrupted task into a ladder rung (`repair`) and a new plan. It
    /// resumes on the same rung now, in the same revision, with the gone session's
    /// open effects settled once for re-verification and its attempt ended as
    /// `runtime_restart`; a restart that finds the same point three times pauses the
    /// task for the runtime's own recheck instead (`boot_recovery`).
    #[test]
    fn a_restart_resumes_on_the_same_rung_and_a_crash_loop_pauses() {
        let (root, store) = store();
        let recover = |store: &Store| {
            store.write(|db| {
                recover_task_tx(
                    db,
                    "t",
                    "EXECUTING",
                    "owner".into(),
                    1,
                    false,
                    true,
                    Some("boot"),
                )
            })
        };
        let resumed = recover(&store).unwrap();
        assert!(matches!(resumed, Some((_, _, TaskState::Executing))));
        let (state, revision, settled, class): (String, i64, String, String) = store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT t.state,t.revision,(SELECT json_extract(result_json,'$.settled_by') FROM runtime_requests WHERE request_key='w1'),
                        (SELECT json_extract(payload_json,'$.failure_class') FROM task_events WHERE event_type='assignment_failed')
                     FROM runtime_tasks t WHERE task_id='t'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?)
            })
            .unwrap();
        assert_eq!(
            (state.as_str(), revision, settled.as_str(), class.as_str()),
            ("EXECUTING", 4, "runtime_reverify", "runtime_restart")
        );
        assert_eq!(count(&store, "AUTOMATIC_CONTINUATION"), 0);
        assert_eq!(count(&store, "NATIVE_RECOVERY_RESUMED"), 1);
        // Two more restarts at the same point resume again; the fourth pauses.
        for _ in 0..2 {
            assert!(recover(&store).unwrap().is_some());
        }
        assert_eq!(count(&store, "LOOP_OBSERVED"), 2);
        assert!(recover(&store).unwrap().is_none());
        let (state, pause): (String, String) = store
            .read(|db| {
                Ok(db.query_row(
                    "SELECT (SELECT state FROM runtime_tasks WHERE task_id='t'),
                        json_extract(payload_json,'$.pause_kind') FROM runtime_events WHERE type='AUTOMATIC_PAUSED'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            })
            .unwrap();
        assert_eq!(
            (state.as_str(), pause.as_str()),
            ("RECOVERING", "infrastructure")
        );
        assert_eq!(count(&store, "LOOP_BROKEN"), 1);
        assert_eq!(count(&store, "AUTOMATIC_CONTINUATION"), 0);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn the_sixth_coordinator_interruption_series_waits_across_restart_and_resumes_once() {
        use cortex_core::lifecycle::automatic;

        let (root, mut store) = empty_store();
        store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET state='PLANNING',revision=4", [])?;
                Ok(())
            })
            .unwrap();
        let reopen = || {
            Store::open(
                ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
                OpenMode::Initialize,
            )
            .unwrap()
        };
        let reason = "native_turn_outcome_unknown";
        // Two immediate continuations, followed by five cooldowns. Fast-forward
        // only the stored deadline, keeping the actual persisted policy history.
        for (index, delay) in [0, 0, 300, 900, 1800, 3600, 3600].into_iter().enumerate() {
            store
                .write(|db| {
                    assert_eq!(domain::stop_tx(db, "t", reason)?, TaskState::Recovering);
                    let owed = automatic::owed(db, "t")?.unwrap();
                    assert_eq!(owed.evidence["loop"]["count"], index + 1);
                    if delay > 0 {
                        assert_eq!(owed.evidence["backoff_seconds"], delay);
                        assert!(automatic::resume_tx(db, "t", owed.event)?.is_none());
                        db.execute(
                            "UPDATE runtime_events SET payload_json=json_set(payload_json,'$.not_before',0) WHERE id=?",
                            [owed.event],
                        )?;
                    } else {
                        assert!(owed.waiting.is_empty());
                    }
                    assert!(automatic::resume_tx(db, "t", owed.event)?.is_some());
                    Ok(())
                })
                .unwrap();
        }
        assert_eq!(count(&store, "AUTOMATIC_PAUSED"), 5);
        assert_eq!(count(&store, "USER_QUESTION_REQUESTED"), 0);
        drop(store);
        store = reopen();
        let question = store
            .write(|db| {
                // A newly interrupted turn's effect has no final receipt.
                db.execute(
                    "INSERT INTO runtime_requests VALUES ('t','last-write','c1','project_write','digest','{\"path\":\"src/lib.rs\"}')",
                    [],
                )?;
                lifecycle::event_tx(db, "t", "NATIVE_DRIVER_STOPPED", json!({"reason":reason}))?;
                assert_eq!(domain::stop_tx(db, "t", reason)?, TaskState::WaitingUser);
                assert_eq!(domain::stop_tx(db, "t", reason)?, TaskState::WaitingUser);
                assert!(automatic::owed(db, "t")?.is_none());
                let (id, text): (String, String) = db.query_row(
                    "SELECT id,text FROM pending_questions WHERE task_id='t' AND status='open'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                assert!(text.contains("the coordinator's turn was interrupted"), "{text}");
                assert!(text.contains("sixth series"), "{text}");
                let asked: String = db.query_row(
                    "SELECT payload_json FROM runtime_events WHERE task_id='t' AND type='USER_QUESTION_REQUESTED'",
                    [],
                    |r| r.get(0),
                )?;
                let asked: Value = serde_json::from_str(&asked)?;
                assert_eq!(asked["question_id"], id);
                assert_eq!(asked["question"], text);
                assert_eq!(asked["blocker"], reason);
                assert_eq!(asked["option_ids"], json!(["retry_now", "other_approach", "retry_later"]));
                Ok(id)
            })
            .unwrap();
        drop(store);
        store = reopen();
        store
            .write(|db| {
                // The startup recovery path must preserve this exact question;
                // even an unresolved effect cannot turn it back into a retry.
                recover_task_tx(
                    db,
                    "t",
                    "WAITING_USER",
                    "owner".into(),
                    1,
                    false,
                    true,
                    Some("boot-after-interruption"),
                )?;
                assert_eq!(domain::task_tx(db, "t")?, (TaskState::WaitingUser, 4));
                assert!(automatic::owed(db, "t")?.is_none());
                assert!(waits::answer_tx(db, "t", &question, "retry_now", "text", None)?);
                assert!(!waits::answer_tx(db, "t", &question, "retry_now", "text", None)?);
                let event = automatic::owed(db, "t")?.unwrap().event;
                let resumed = automatic::resume_tx(db, "t", event)?.unwrap();
                assert_eq!(resumed["owner_question"], question);
                assert_eq!(resumed["owner_choice"], "retry_now");
                assert_eq!(resumed["uncertainty"][0]["operation"], "project_write");
                assert!(automatic::resume_tx(db, "t", event)?.is_none());
                assert_eq!(domain::task_tx(db, "t")?, (TaskState::Planning, 4));
                let (outcome, settlement): (String, String) = db.query_row(
                    "SELECT json_extract(result_json,'$.outcome'),json_extract(result_json,'$.settled_by')
                     FROM runtime_requests WHERE request_key='last-write'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                assert_eq!((outcome.as_str(), settlement.as_str()), ("unknown", "runtime_reverify"));

                // The answer opens a fresh window without consuming a rung or a sighting.
                assert_eq!(domain::stop_tx(db, "t", reason)?, TaskState::Recovering);
                let owed = automatic::owed(db, "t")?.unwrap();
                assert_eq!(owed.evidence["loop"]["count"], 1);
                assert!(owed.waiting.is_empty());
                Ok(())
            })
            .unwrap();
        assert_eq!(count(&store, "AUTOMATIC_PAUSED"), 5);
        assert_eq!(count(&store, "AUTOMATIC_RESUMED"), 8);
        assert_eq!(count(&store, "USER_QUESTION_REQUESTED"), 1);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    /// A restart in the middle of the final verification left its run without a
    /// finish record; it used to become a ladder continuation (a replan). The task
    /// stays VERIFYING on the same rung, and the driver settles that run as unknown
    /// and reruns verification itself.
    #[test]
    fn an_interrupted_verification_stays_verifying_without_a_continuation() {
        let (root, store) = store();
        store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET state='VERIFYING'", [])?;
                db.execute("UPDATE runtime_nodes SET status='completed',result_json='{}'", [])?;
                db.execute("DELETE FROM runtime_requests", [])?;
                db.execute(
                    "INSERT INTO runtime_requests VALUES ('t','verify-1','native-verifier','native_verification_intent','digest','{\"revision\":4}')",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        let resumed = store
            .write(|db| {
                recover_task_tx(
                    db,
                    "t",
                    "VERIFYING",
                    "owner".into(),
                    1,
                    false,
                    false,
                    Some("boot"),
                )
            })
            .unwrap();
        assert!(matches!(resumed, Some((_, _, TaskState::Verifying))));
        let state: String = store
            .read(|db| Ok(db.query_row("SELECT state FROM runtime_tasks", [], |r| r.get(0))?))
            .unwrap();
        assert_eq!(state, "VERIFYING");
        assert_eq!(count(&store, "AUTOMATIC_CONTINUATION"), 0);
        assert_eq!(count(&store, "NATIVE_RECOVERY_REQUIRED"), 0);
        assert_eq!(count(&store, "NATIVE_RECOVERY_RESUMED"), 1);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    /// Owner stores: ~20 restarts re-flagged the same parked tasks (236
    /// `NATIVE_RECOVERY_REQUIRED`, up to 16 per task). One per restart burst now.
    #[test]
    fn recovery_required_is_recorded_once_per_burst() {
        let (root, store) = store();
        store
            .write(|db| {
                for _ in 0..3 {
                    recovery_required_once_tx(
                        db,
                        "t",
                        json!({"reason":"native_session_outcome_unknown","previous":"WAITING_USER","replayed":false}),
                    )?;
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(count(&store, "NATIVE_RECOVERY_REQUIRED"), 1);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
