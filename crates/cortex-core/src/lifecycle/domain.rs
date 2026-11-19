//! Native lifecycle side effects. All functions use the caller's writer transaction.
//! Python oracle: orchestration/state.py:327-422,1579-1588,2152-2192;
//! orchestration/task_pause.py:18-81; orchestration/policy.py:142-163.
use super::{
    event_tx, request_tx, waits, MAX_AUTONOMOUS_COORDINATOR_RETURNS, MAX_COORDINATOR_RETURNS,
};
use crate::{
    error::refuse,
    ownership::{self, OwnerEpoch},
    CoreError, CoreResult, TaskState,
};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub fn task_tx(db: &Transaction<'_>, task: &str) -> CoreResult<(TaskState, i64)> {
    let row: Option<(String, i64)> = db
        .query_row(
            "SELECT state,revision FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (state, revision) = row.ok_or(CoreError::Refused("unknown_task"))?;
    Ok((state.parse().map_err(CoreError::Refused)?, revision))
}
pub(crate) fn state_tx(
    db: &Transaction<'_>,
    task: &str,
    target: TaskState,
    blocker: Option<&str>,
) -> CoreResult<()> {
    let (source, _) = task_tx(db, task)?;
    db.execute(
        "UPDATE runtime_tasks SET state=?,blocker=?,updated_at=? WHERE task_id=?",
        (target.as_str(), blocker, crate::now(), task),
    )?;
    event_tx(
        db,
        task,
        "TASK_STATE_CHANGED",
        json!({"previous":source,"state":target,"blocker":blocker}),
    )?;
    Ok(())
}
pub(crate) fn revoke_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: Option<&str>,
) -> CoreResult<()> {
    db.execute("UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND role='worker' AND revoked_at IS NULL
        AND (? IS NULL OR attempt_id IN (SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?))",
        (crate::now(),task,assignment,task,assignment))?;
    Ok(())
}
/// Service restart is not a process wait or permission to replay effects.
/// Keep questions, waits and nodes intact; only the execution's recovery state changes.
pub fn native_recovery_required_tx(
    db: &Transaction<'_>,
    task: &str,
    reason: &str,
) -> CoreResult<()> {
    let state = task_tx(db, task)?.0;
    if state.terminal() {
        return Ok(());
    }
    let target = if matches!(state, TaskState::Paused | TaskState::WaitingUser) {
        state
    } else {
        TaskState::Recovering
    };
    // An unresolved outcome-unknown barrier is evidence, not a reason that a later
    // refusal may overwrite. Only an explicit owner Retry clears it.
    let current: Option<String> = db.query_row(
        "SELECT blocker FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let reason = match current.as_deref() {
        Some(existing) if existing.ends_with("outcome_unknown") => existing.to_owned(),
        _ => reason.to_owned(),
    };
    state_tx(db, task, target, Some(&reason))
}

/// Engine assignment identity and work-graph attempt identity are distinct.
/// Resolve the explicit delegation event binding, never guess one from the other.
/// Python: recovery/worker_reconciliation.py:49-84.
pub(crate) fn graph_attempt_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: &str,
    status: &str,
    reason: Option<&str>,
) -> CoreResult<()> {
    let s: Option<String> = db
        .query_row(
            "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=?
        AND event_type='delegation_requested' ORDER BY id DESC LIMIT 1",
            (task, assignment),
            |r| r.get(0),
        )
        .optional()?;
    if let Some(s) = s {
        let v: Value = serde_json::from_str(&s)?;
        if let Some(attempt) = v["work_attempt_ref"].as_str().filter(|s| !s.is_empty()) {
            let changed=db.execute("UPDATE work_assignment_attempts SET status=?,failure_reason=? WHERE task_id=? AND attempt_id=? AND status IN ('planned','running')",
                (status,reason,task,attempt))?;
            if changed > 0 {
                db.execute("UPDATE work_assignments SET status=? WHERE task_id=? AND assignment_id=(
                    SELECT assignment_id FROM work_assignment_attempts WHERE task_id=? AND attempt_id=?)",
                    (status,task,task,attempt))?;
                event_tx(
                    db,
                    task,
                    "ASSIGNMENT_STATE_CHANGED",
                    json!({"attempt_id":attempt,"status":status,"failure_reason":reason}),
                )?;
            }
        }
    }
    Ok(())
}
pub fn cancel_nodes_tx(db: &Transaction<'_>, task: &str, reason: &str) -> CoreResult<Vec<String>> {
    let refs = db.prepare("SELECT assignment_ref FROM runtime_nodes WHERE task_id=? AND status IN ('pending','running')
        AND assignment_ref IS NOT NULL ORDER BY revision,node_id")?
        .query_map([task],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    db.execute("UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND status IN ('pending','running')",[task])?;
    for assignment in &refs {
        graph_attempt_tx(db, task, assignment, "failed", Some(reason))?;
    }
    if !refs.is_empty() {
        event_tx(
            db,
            task,
            "LIVE_ASSIGNMENTS_CANCELLED",
            json!({"assignment_refs":refs,"reason":reason}),
        )?;
    }
    Ok(refs)
}
/// The language the owner chose for this task's messages: the task settings' `en`/`ru`, and
/// for `auto` (or no setting) the language of the task's own goal text.
pub fn task_language_tx(db: &Transaction<'_>, task: &str) -> CoreResult<&'static str> {
    let chosen: Option<String> = db
        .query_row(
            "SELECT json_extract(payload_json,'$.settings.language') FROM runtime_events
             WHERE task_id=? AND type='NATIVE_TASK_SETTINGS' ORDER BY id DESC LIMIT 1",
            [task],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    match chosen.as_deref() {
        Some("ru") => return Ok("ru"),
        Some("en") => return Ok("en"),
        _ => {}
    }
    let goal: Option<String> = db
        .query_row(
            "SELECT coalesce(json_extract(payload_json,'$.goal'),'') FROM runtime_events
             WHERE task_id=? AND type='NATIVE_START_ADMITTED' ORDER BY id LIMIT 1",
            [task],
            |r| r.get(0),
        )
        .optional()?;
    let cyrillic = goal
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_alphabetic())
        .take(400)
        .fold((0, 0), |(ru, all), c| {
            (
                ru + usize::from(('\u{400}'..='\u{4ff}').contains(&c)),
                all + 1,
            )
        });
    Ok(if cyrillic.1 > 0 && cyrillic.0 * 2 >= cyrillic.1 {
        "ru"
    } else {
        "en"
    })
}
/// Plain-language form of runtime blockers whose raw code would otherwise reach
/// an owner question, in the task's language. An unknown blocker is named as an
/// internal stop with its code in parentheses, never as the bare code.
fn owner_readable_blocker(blocker: &str, ru: bool) -> String {
    let (en, ruw) = match blocker {
        "worker_stopped_by_owner" => (
            "a worker was stopped and the remaining planned work depends on it or has nothing left to run",
            "воркер был остановлен, а оставшаяся запланированная работа зависит от него или выполнять больше нечего",
        ),
        "native_session_start_failed" => ("a worker session could not start", "не удалось запустить сессию воркера"),
        "coordinator_session_start_failed" => ("the coordinator session could not start", "не удалось запустить сессию координатора"),
        "worker_failed" | "worker_failure_not_retryable" => ("a worker failed", "воркер завершился с ошибкой"),
        "worker_retry_limit" => ("a worker kept failing after the allowed retries", "воркер продолжал падать после всех разрешённых повторов"),
        "verification_failed" => ("the runtime verification failed", "проверка результата не прошла"),
        "review_blocked" => ("the independent review blocked the result", "независимое ревью заблокировало результат"),
        "review_cycle_limit" => ("the review cycle limit was reached", "достигнут предел циклов ревью"),
        "review_no_progress" => (
            "fixes and review stopped making progress",
            "исправления и ревью перестали давать результат",
        ),
        "review_incomplete" => (
            "the independent review could not cover the whole change",
            "независимое ревью не смогло охватить всё изменение",
        ),
        "verification_review_revision_changed" => (
            "the files changed after the review",
            "файлы изменились после ревью",
        ),
        "verification_revision_unavailable" | "review_revision_unavailable" => (
            "the state of the project files could not be read",
            "не удалось прочитать состояние файлов проекта",
        ),
        "project_revision_unavailable" => (
            "the project's Git repository could not be read",
            "не удалось прочитать Git-репозиторий проекта",
        ),
        b if b.starts_with("project_unobservable_") => (
            "the project files could not be observed completely",
            "не удалось полностью прочитать файлы проекта",
        ),
        "native_progress_limit" => ("the task reached its limit of steps", "задача достигла предела шагов"),
        "coordinator_no_progress_limit" => ("the coordinator stopped making progress", "координатор перестал продвигаться"),
        "no_runnable_nodes" => ("no planned step can run", "ни один запланированный шаг не может выполниться"),
        "scheduler_capacity_timeout" => ("no capacity became free for the next step in time", "для следующего шага вовремя не освободилась мощность"),
        // What held the step is stated from recorded facts by `stop_notice`, never guessed here.
        "native_continuation_unavailable" => ("no planned step could be started", "не удалось запустить ни один запланированный шаг"),
        "lost_worker_action_outcome_unknown" => (
            "a worker's turn ended without an accepted result, and whether some of its actions finished is not known",
            "ход воркера закончился без принятого результата, и неизвестно, завершились ли некоторые его действия",
        ),
        b if b.starts_with("lost_worker_") => ("a worker process was lost", "процесс воркера был потерян"),
        b if b.starts_with("completion_") => ("the work could not be confirmed as complete", "не удалось подтвердить, что работа завершена"),
        _ => ("", ""),
    };
    match (en.is_empty(), ru) {
        (false, false) => en.to_owned(),
        (false, true) => ruw.to_owned(),
        (true, false) => format!("an internal stop ({blocker})"),
        (true, true) => format!("внутренняя остановка ({blocker})"),
    }
}
/// The first line of the runtime stop question: what stopped the task.
pub fn stop_headline(blocker: &str, ru: bool) -> String {
    if ru {
        format!(
            "Задача остановлена: {}.",
            owner_readable_blocker(blocker, true)
        )
    } else {
        format!(
            "The task stopped: {}.",
            owner_readable_blocker(blocker, false)
        )
    }
}
/// The runtime stop question's options, in the task's language. Answers are matched against
/// both languages (see `waits::answer_tx`).
pub const STOP_OPTIONS_EN: [&str; 2] = [
    "Continue: let the coordinator decide how to proceed",
    "Finish: close the task as is, without certifying unfinished work",
];
pub const STOP_OPTIONS_RU: [&str; 2] = [
    "Продолжить: координатор решит, как действовать дальше",
    "Завершить: закрыть задачу как есть, без подтверждения незавершённой работы",
];
/// A cancelled node of the current revision that the owner stopped after the latest
/// return to the coordinator. Runtime cancellations (returns, stops) never count.
pub fn owner_stop_unhandled_tx(db: &Transaction<'_>, task: &str) -> CoreResult<bool> {
    let (_, revision) = task_tx(db, task)?;
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events e JOIN runtime_nodes n
            ON n.task_id=e.task_id AND n.assignment_ref=json_extract(e.payload_json,'$.assignment_ref')
         WHERE e.task_id=? AND e.type='WORKER_STOPPED_BY_OWNER' AND n.revision=? AND n.status='cancelled'
         AND e.id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=? AND type='RETURNED_TO_COORDINATOR'),0))",
        (task, revision, task),
        |r| r.get(0),
    )?)
}
/// True when the current revision has an owner-stopped (`cancelled`) execution
/// node, no node is running, and no pending node has all its dependencies
/// completed. Dependents of a cancelled node are never runnable.
pub fn owner_stopped_work_is_idle_tx(db: &Transaction<'_>, task: &str) -> CoreResult<bool> {
    let (_, revision) = task_tx(db, task)?;
    let rows = db
        .prepare(
            "SELECT node_id,stage,status,dependencies_json FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid",
        )?
        .query_map((task, revision), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if !owner_stop_unhandled_tx(db, task)? {
        return Ok(false);
    }
    if rows.iter().any(|n| n.2 == "running" || n.2 == "failed") {
        return Ok(false);
    }
    let nodes = rows
        .into_iter()
        .map(|(id, stage, status, deps)| {
            Ok(crate::scheduler::Node {
                id,
                stage,
                status,
                dependencies: serde_json::from_str(&deps)?,
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    Ok(crate::scheduler::dependency_ready(&nodes, "execution")?.is_empty())
}
/// Stop is not successful completion. Work-level stops first let the coordinator ask.
pub fn stop_tx(db: &Transaction<'_>, task: &str, blocker: &str) -> CoreResult<TaskState> {
    let (source, revision) = task_tx(db, task)?;
    if source.terminal() || source == TaskState::Paused {
        return refuse("task_not_stoppable");
    }
    if blocker.trim().is_empty() {
        return refuse("blocker_required");
    }
    if super::automatic::enabled(db, task)? {
        return super::automatic::stop_tx(db, task, blocker);
    }
    let work = [
        "verification_failed",
        "worker_failed",
        "worker_stopped_by_owner",
        "worker_retry_limit",
        "worker_failure_not_retryable",
        "review_cycle_limit",
        "review_no_progress",
        "review_blocked",
        "no_runnable_nodes",
        "scheduler_capacity_timeout",
        "lost_worker_",
        "verification_review_revision_changed",
        "verification_revision_unavailable",
        "review_revision_unavailable",
        "completion_",
    ]
    .iter()
    .any(|p| blocker.starts_with(p));
    let (asked, answered): (i64, i64) = db.query_row(
        "SELECT
        coalesce(max(CASE WHEN type='ASK_USER_REQUIRED' THEN id END),0),
        coalesce(max(CASE WHEN type='USER_ANSWER_RECEIVED' THEN id END),0)
        FROM runtime_events WHERE task_id=?",
        [task],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let returns = source != TaskState::WaitingUser && work && (asked == 0 || answered > asked);
    // The explanation reads the facts (which steps were pending, what held them)
    // before the stop cancels anything; the question it replaces is named so the
    // Conversation can say why that one was withdrawn.
    let ru = task_language_tx(db, task)? == "ru";
    let notice = if returns {
        None
    } else {
        Some(super::stop_notice::explain_tx(db, task, blocker, ru)?)
    };
    waits::cancel_tx(db, task, None, "task_stopped_for_question", "cancelled")?;
    cancel_nodes_tx(db, task, "task_stopped_for_question")?;
    revoke_tx(db, task, None)?;
    let Some(notice) = notice else {
        event_tx(db, task, "ASK_USER_REQUIRED", json!({"blocker":blocker}))?;
        if source == TaskState::Reconciling && blocker == "completion_dropped_failed_check" {
            // The verified revision is still current. Discard the refused synthesis
            // so its coordinator can ask about the removed check on that revision.
            db.execute(
                "UPDATE runtime_tasks SET synthesis=NULL WHERE task_id=?",
                [task],
            )?;
            return Ok(source);
        }
        state_tx(db, task, TaskState::Recovering, None)?;
        return Ok(TaskState::Recovering);
    };
    let supersedes = db
        .prepare(
            "SELECT id FROM pending_questions WHERE task_id=? AND status='open' ORDER BY rowid",
        )?
        .query_map([task], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    db.execute("UPDATE pending_questions SET status='superseded',answered_at=? WHERE task_id=? AND status='open'",
        (crate::now(),task))?;
    let id = format!("pq_{}", uuid::Uuid::new_v4().simple());
    let question = notice.text.clone();
    let options = json!(if ru { STOP_OPTIONS_RU } else { STOP_OPTIONS_EN });
    db.execute(
        "INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,created_at)
        VALUES (?,?,?,?,?,'open',?)",
        (
            &id,
            task,
            format!("runtime:stop:{id}"),
            &question,
            options.to_string(),
            crate::now(),
        ),
    )?;
    let mut payload = json!({"question_id":id,"question":question,"options":options,
        "resume_state":"RECOVERING","blocker":blocker,"revision":revision,"runtime_stop":true,"supersedes":supersedes});
    if let (Some(p), Value::Object(n)) = (payload.as_object_mut(), notice.payload()) {
        p.extend(n);
    }
    event_tx(db, task, "USER_QUESTION_REQUESTED", payload)?;
    state_tx(db, task, TaskState::WaitingUser, Some(blocker))?;
    Ok(TaskState::WaitingUser)
}
pub fn cancel_task_tx(db: &Transaction<'_>, task: &str, reason: &str) -> CoreResult<()> {
    let (state, _) = task_tx(db, task)?;
    if state.terminal() {
        return Ok(());
    }
    waits::cancel_tx(db, task, None, reason, "cancelled")?;
    cancel_nodes_tx(db, task, reason)?;
    revoke_tx(db, task, None)?;
    db.execute("UPDATE pending_questions SET status='cancelled',answered_at=? WHERE task_id=? AND status='open'",
        (crate::now(),task))?;
    state_tx(db, task, TaskState::Cancelled, Some(reason))
}
/// Turn cancellation does not erase an explicit owner PAUSED hold.
pub fn interrupt_tx(db: &Transaction<'_>, task: &str) -> CoreResult<()> {
    let (state, _) = task_tx(db, task)?;
    if state.terminal() || state == TaskState::Paused {
        return Ok(());
    }
    waits::cancel_tx(db, task, None, "interrupted", "cancelled")?;
    cancel_nodes_tx(db, task, "interrupted")?;
    revoke_tx(db, task, None)?;
    event_tx(db, task, "TURN_INTERRUPTED", json!({}))?;
    state_tx(db, task, TaskState::Recovering, None)
}
/// Python `policy.apply_steering`: an owner message to a COMPLETED or FAILED task
/// reopens it for a fresh plan. Old nodes and results stay in their revision; the
/// unsettled result text and blocker are cleared. A task the owner already closed
/// stays closed. Other states are unchanged.
pub fn reopen_tx(db: &Transaction<'_>, task: &str) -> CoreResult<bool> {
    let (state, revision) = task_tx(db, task)?;
    if !matches!(state, TaskState::Completed | TaskState::Failed) {
        return Ok(false);
    }
    let closed: bool = db.query_row(
        "SELECT coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='OWNER_TASK_RESOLVED'),0)
            > coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0)",
        [task],
        |r| r.get(0),
    )?;
    if closed {
        return refuse("task_not_resumable");
    }
    db.execute(
        "UPDATE runtime_tasks SET state='RECOVERING',synthesis=NULL,blocker=NULL,updated_at=? WHERE task_id=?",
        (crate::now(), task),
    )?;
    event_tx(
        db,
        task,
        "TASK_REOPENED",
        json!({"previous":state,"revision":revision}),
    )?;
    Ok(true)
}
/// A worker stop cancels only that worker's wait; its capacity is NOT released.
pub fn stop_assignment_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: &str,
    reason: &str,
) -> CoreResult<()> {
    let found: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND assignment_ref=?)",
        (task, assignment),
        |r| r.get(0),
    )?;
    if !found {
        return refuse("unknown_assignment");
    }
    waits::cancel_tx(db, task, Some(assignment), reason, "cancelled")?;
    revoke_tx(db, task, Some(assignment))?;
    let changed=db.execute("UPDATE runtime_nodes SET status='cancelled' WHERE task_id=? AND assignment_ref=? AND status IN ('pending','running')",
        (task,assignment))?;
    graph_attempt_tx(db, task, assignment, "failed", Some(reason))?;
    if changed > 0 {
        event_tx(
            db,
            task,
            "LIVE_ASSIGNMENTS_CANCELLED",
            json!({"assignment_refs":[assignment],"reason":reason}),
        )?;
        // Only this event marks an owner stop; the cancellations the runtime makes
        // when it returns to the coordinator carry no such mark.
        event_tx(
            db,
            task,
            "WORKER_STOPPED_BY_OWNER",
            json!({"assignment_ref":assignment,"reason":reason}),
        )?;
    }
    db.execute("INSERT OR IGNORE INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at)
        VALUES (?,'assignment_cancel_requested',?,?,?,'runtime',?)",
        (task,assignment,format!("stop:{assignment}"),json!({"reason":reason}).to_string(),crate::now()))?;
    Ok(())
}
pub fn pause_saved_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Option<Value>> {
    let row:Option<String>=db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='OWNER_TASK_PAUSED' ORDER BY id DESC LIMIT 1",
        [task],|r|r.get(0)).optional()?;
    row.map(|s| Ok(serde_json::from_str(&s)?)).transpose()
}
/// Owner adapter uses the displayed workspace task version. Unbound Core callers
/// use the task's event cursor, obtained with this same function.
pub fn pause_version_tx(db: &Transaction<'_>, task: &str) -> CoreResult<i64> {
    let v: Option<i64> = db
        .query_row(
            "SELECT t.version FROM workspace_tasks t JOIN workspace_executions e USING(task_id)
        WHERE e.runtime_task_id=?",
            [task],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match v {
        Some(v) => v,
        None => db.query_row(
            "SELECT coalesce(max(id),0) FROM runtime_events WHERE task_id=?",
            [task],
            |r| r.get(0),
        )?,
    })
}
/// Called only by the authenticated owner adapter, not model dispatch.
/// An urgent pause ignores stale displayed content; resume requires its exact version.
pub fn owner_pause_tx(
    db: &Transaction<'_>,
    task: &str,
    owner: &OwnerEpoch,
    key: &str,
    resume: bool,
    expected_version: i64,
) -> CoreResult<Value> {
    ownership::check_tx(db, task, owner)?;
    request_tx(
        db,
        task,
        &owner.owner,
        key,
        "owner_pause",
        &json!({"resume":resume,"version":expected_version,"epoch":owner.epoch}),
        || {
            let bound_epoch: Option<i64> = db
                .query_row(
                    "SELECT owner_epoch FROM workspace_executions WHERE runtime_task_id=?",
                    [task],
                    |r| r.get(0),
                )
                .optional()?;
            if bound_epoch.is_some_and(|epoch| epoch != owner.epoch) {
                return refuse("stale_epoch");
            }
            let (before, _) = task_tx(db, task)?;
            if before.terminal() {
                return refuse("task_not_resumable");
            }
            if resume && pause_version_tx(db, task)? != expected_version {
                return refuse("stale_version");
            }
            if resume && before == TaskState::Recovering {
                if let Some(event) = super::automatic::owner_cap(db, task)? {
                    super::automatic::require_known_effects(db, task)?;
                    // Keep the original hold until the host qualifies a route
                    // and actually admits the fresh planning turn.
                    return Ok(json!({"state":before,"outcome":"recorded","stopEvent":event}));
                }
            }
            let (target, blocker) = if resume {
                let saved =
                    pause_saved_tx(db, task)?.ok_or(CoreError::Refused("task_not_paused"))?;
                if before != TaskState::Paused {
                    return refuse("task_not_paused");
                }
                let target = saved["resumeState"]
                    .as_str()
                    .ok_or(CoreError::Refused("invalid_pause_record"))?
                    .parse()
                    .map_err(CoreError::Refused)?;
                // A barrier recorded while paused (restart with unknown outcomes) is
                // never erased by Resume: the task returns as RECOVERING, retryable.
                let held: Option<String> = db.query_row(
                    "SELECT blocker FROM runtime_tasks WHERE task_id=?",
                    [task],
                    |r| r.get(0),
                )?;
                let held = held.filter(|b| b.ends_with("outcome_unknown"));
                // A delivery that was claimed but never confirmed cannot be replayed.
                let delivery_unknown = waits::projection_tx(db, task)?
                    .iter()
                    .any(|w| w["status"] == "resuming");
                let barrier = match (held, delivery_unknown) {
                    (Some(b), _) => Some(b),
                    (None, true) => {
                        waits::recover_deliveries_tx(db, task)?;
                        Some("native_answer_delivery_outcome_unknown".to_owned())
                    }
                    (None, false) => None,
                };
                let target = if barrier.is_some() {
                    TaskState::Recovering
                } else {
                    target
                };
                event_tx(
                    db,
                    task,
                    "OWNER_TASK_RESUMED",
                    json!({"state":target,"questionIds":saved["questionIds"],"barrier":barrier}),
                )?;
                (
                    target,
                    barrier.or_else(|| saved["blocker"].as_str().map(str::to_owned)),
                )
            } else {
                if before == TaskState::Paused {
                    return refuse("already_paused");
                }
                let questions = db
                    .prepare("SELECT id FROM pending_questions WHERE task_id=? AND status='open'")?
                    .query_map([task], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                let mut ids = Vec::new();
                for mut w in waits::projection_tx(db, task)? {
                    if ["open", "ready"].contains(&w["status"].as_str().unwrap_or(""))
                        && waits::valid_tx(db, &w)?
                    {
                        w["owner_paused"] = json!(true);
                        w["continuation_required"] = json!(true);
                        waits::change_tx(db, &mut w)?;
                        db.execute("UPDATE trusted_channels SET revoked_at=? WHERE channel_id=? AND revoked_at IS NULL",
                        (crate::now(),w["channel_id"].as_str()))?;
                        ids.push(w["wait_id"].clone());
                    }
                }
                // Resume continues from exactly where the task was paused. Running
                // turns end at Pause; their nodes stay `running` and are continued
                // by the lost-worker retry / coordinator recovery of the driver.
                let resume_state = before;
                let blocker: Option<String> = db.query_row(
                    "SELECT blocker FROM runtime_tasks WHERE task_id=?",
                    [task],
                    |r| r.get(0),
                )?;
                event_tx(
                    db,
                    task,
                    "OWNER_TASK_PAUSED",
                    json!({"previous":before,"resumeState":resume_state,
                "blocker":blocker,"questionIds":questions,"waitIds":ids}),
                )?;
                (TaskState::Paused, blocker)
            };
            state_tx(db, task, target, blocker.as_deref())?;
            Ok(json!({"state":target,"outcome":"recorded"}))
        },
    )
}
/// The owner answered the runtime's own stop question (resume RECOVERING) and
/// nothing has run since. Python hands that answer to a new coordinator turn
/// (workspace_api._answer -> dispatch_answer); the stopped native driver is gone,
/// so the host must relaunch it or the task stays in RECOVERING forever.
pub fn stop_continuation_owed_tx(db: &Transaction<'_>, task: &str) -> CoreResult<bool> {
    let row: Option<(String, Option<String>)> = db
        .query_row(
            "SELECT state,blocker FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((state, blocker)) = row else {
        return Ok(false);
    };
    if state != TaskState::Recovering.as_str() || blocker.is_some_and(|b| !b.is_empty()) {
        return Ok(false);
    }
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events c WHERE c.task_id=? AND c.type='TASK_STATE_CHANGED'
            AND c.id=(SELECT max(id) FROM runtime_events WHERE task_id=c.task_id AND type='TASK_STATE_CHANGED')
            AND json_extract(c.payload_json,'$.previous')='WAITING_USER'
            AND json_extract(c.payload_json,'$.state')='RECOVERING'
            AND c.id>(SELECT coalesce(max(q.id),0) FROM runtime_events q WHERE q.task_id=c.task_id
                AND q.type='USER_QUESTION_REQUESTED' AND json_extract(q.payload_json,'$.runtime_stop')=1)
            AND EXISTS(SELECT 1 FROM runtime_events q WHERE q.task_id=c.task_id AND q.id<c.id
                AND q.type='USER_QUESTION_REQUESTED' AND json_extract(q.payload_json,'$.runtime_stop')=1)
            AND NOT EXISTS(SELECT 1 FROM runtime_events s WHERE s.task_id=c.task_id AND s.id>c.id
                AND s.type IN ('TURN_INTERRUPTED','NATIVE_COORDINATOR_CONTINUED')))",
        [task],
        |r| r.get(0),
    )?)
}
/// Explicit desktop continuation after this host observed every owned process exit.
/// Old nodes/results remain in their revision; the coordinator must submit a fresh plan.
pub fn native_continue_tx(db: &Transaction<'_>, task: &str) -> CoreResult<()> {
    let state = task_tx(db, task)?.0;
    if state.terminal() || state == TaskState::Paused {
        return refuse("task_not_resumable");
    }
    let unsettled: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND status!='terminated')
         OR EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND state!='released')",
        (task,task), |r| r.get(0))?;
    if unsettled {
        return refuse("lost_worker_termination_unconfirmed");
    }
    let cap = super::automatic::owner_cap(db, task)?;
    if cap.is_some() {
        super::automatic::require_known_effects(db, task)?;
    }
    interrupt_tx(db, task)?;
    db.execute("UPDATE pending_questions SET status=\'superseded\',answered_at=? WHERE task_id=? AND status=\'open\'", (crate::now(),task))?;
    settle_recovery_tx(db, task, TaskState::Planning)?;
    if let Some(event) = cap {
        event_tx(
            db,
            task,
            "AUTOMATIC_CAP_RENEWED",
            json!({"stop_event":event}),
        )?;
    }
    Ok(())
}
/// Pending captures survive restart; settled assignments are never cancelled twice.
pub fn recovery_cancellations_tx(
    db: &Transaction<'_>,
    task: &str,
) -> CoreResult<(i64, Vec<String>)> {
    let after: i64 = db.query_row(
        "SELECT coalesce((SELECT json_extract(payload_json,'$.through') FROM runtime_events
        WHERE task_id=? AND type='LIVE_CANCELLATIONS_SETTLED' ORDER BY id DESC LIMIT 1),0)",
        [task],
        |r| r.get(0),
    )?;
    let rows=db.prepare("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type='LIVE_ASSIGNMENTS_CANCELLED' AND id>? ORDER BY id")?
        .query_map((task,after),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
    let mut through = after;
    let mut refs = BTreeSet::new();
    for (id, payload) in rows {
        through = id;
        let v: Value = serde_json::from_str(&payload)?;
        for r in v["assignment_refs"]
            .as_array()
            .ok_or(CoreError::Refused("invalid_cancellation_capture"))?
        {
            refs.insert(
                r.as_str()
                    .ok_or(CoreError::Refused("invalid_cancellation_capture"))?
                    .to_owned(),
            );
        }
    }
    let unrecorded=db.prepare("SELECT n.assignment_ref FROM runtime_nodes n WHERE n.task_id=? AND n.status='cancelled'
        AND n.assignment_ref IS NOT NULL AND NOT EXISTS(SELECT 1 FROM task_events e WHERE e.task_id=n.task_id
            AND e.assignment_ref=n.assignment_ref AND e.event_type='assignment_cancelled') ORDER BY n.revision,n.node_id")?
        .query_map([task],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    let unrecorded = unrecorded
        .into_iter()
        .filter(|r| !refs.contains(r))
        .collect::<Vec<_>>();
    if !unrecorded.is_empty() {
        through = event_tx(
            db,
            task,
            "LIVE_ASSIGNMENTS_CANCELLED",
            json!({"assignment_refs":unrecorded,"reason":"unrecorded"}),
        )?;
        refs.extend(unrecorded);
    }
    Ok((through, refs.into_iter().collect()))
}
/// Effects have already been reconciled outside SQLite. Released reservations are
/// evidence of confirmed absence; unknown/alive retains the capture and its scope.
pub fn settle_recovery_tx(db: &Transaction<'_>, task: &str, target: TaskState) -> CoreResult<()> {
    if task_tx(db, task)?.0 != TaskState::Recovering
        || !super::allowed(TaskState::Recovering, target)
    {
        return refuse("recovery_not_expected");
    }
    let (through, refs) = recovery_cancellations_tx(db, task)?;
    for assignment in &refs {
        let unconfirmed:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM delegation_provisioning p
            WHERE p.task_id=? AND p.idempotency_key=? AND (NOT EXISTS(SELECT 1 FROM capacity_reservations r WHERE r.attempt_id=p.attempt_id)
            OR EXISTS(SELECT 1 FROM capacity_reservations r WHERE r.attempt_id=p.attempt_id AND r.state!='released')))",
            (task,assignment),|r|r.get(0))?;
        if unconfirmed {
            return refuse("lost_worker_termination_unconfirmed");
        }
        revoke_tx(db, task, Some(assignment))?;
        db.execute("INSERT OR IGNORE INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at)
            VALUES (?,'assignment_cancelled',?,?,'{}','runtime',?)",(task,assignment,format!("recovery:{assignment}"),crate::now()))?;
    }
    if through > 0 {
        event_tx(
            db,
            task,
            "LIVE_CANCELLATIONS_SETTLED",
            json!({"through":through,"assignment_refs":refs}),
        )?;
    }
    state_tx(db, task, target, None)
}
/// Model proposals are channel-authorized before calling. This changes the plan,
/// never filesystem access. Python: orchestration/policy.py:178-222.
pub fn propose_replan_tx(
    db: &Transaction<'_>,
    task: &str,
    actor: &str,
    key: &str,
    operation: &str,
    proposal: &Value,
) -> CoreResult<Value> {
    if !["replan_request", "scope_request", "escalation_request"].contains(&operation) {
        return refuse("invalid_replan_operation");
    }
    request_tx(db, task, actor, key, operation, proposal, || {
        let (state, revision) = task_tx(db, task)?;
        if state.terminal() || state == TaskState::Paused {
            return refuse("task_not_resumable");
        }
        if super::automatic::held(db, task)? {
            return refuse("automatic_continuation_pending");
        }
        let count: i64 = db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='REPLAN_REQUEST_ACCEPTED'
            AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=? AND type IN
                ('USER_STEERING','USER_ANSWER_RECEIVED','WORKER_USER_ANSWER_RECEIVED','AUTOMATIC_RESUMED')),0)",
            (task, task),
            |r| r.get(0),
        )?;
        if count >= 3 {
            if super::automatic::enabled(db, task)? {
                if stop_tx(db, task, "replan_limit")? != TaskState::Recovering {
                    return refuse("automatic_real_decision_pending");
                }
                return Ok(json!({"accepted":false,"state":"RECOVERING",
                    "decision":"automatic_continuation","scope_granted":false}));
            }
            return refuse("replan_limit_requires_user_input");
        }
        if operation == "scope_request" {
            let paths = proposal["scope"]
                .as_array()
                .ok_or(CoreError::Refused("invalid_scope"))?;
            if paths.is_empty()
                || paths.iter().any(|p| {
                    p.as_str().is_none_or(|s| {
                        s.is_empty()
                            || s.chars().count() > 512
                            || s.starts_with('/')
                            || s.contains(['\\', '\0'])
                            || s.split('/').any(|s| {
                                matches!(
                                    s,
                                    ".." | ".git"
                                        | ".cortex"
                                        | ".cortex-dev"
                                        | ".cortex-dev-owner.json"
                                        | ".cortex-workspace-dev.json"
                                )
                            })
                    })
                })
            {
                return refuse("invalid_scope");
            }
            // The owner's write denials in force (lifted ones excluded).
            let denials = super::steering::denials(db, task)?;
            if paths.iter().any(|p| {
                denials
                    .iter()
                    .any(|d| crate::scheduler::policy::overlaps(p.as_str().unwrap(), d))
            }) {
                return refuse("scope_conflicts_with_user_policy");
            }
        }
        waits::cancel_tx(db, task, None, "replanning", "cancelled")?;
        cancel_nodes_tx(db, task, "replanning")?;
        revoke_tx(db, task, None)?;
        state_tx(db, task, TaskState::Recovering, None)?;
        let receipt = json!({"accepted":true,"state":"RECOVERING","decision":if operation=="escalation_request"{"senior_consultation"}else{"replan"},"scope_granted":false});
        event_tx(
            db,
            task,
            "REPLAN_REQUEST_ACCEPTED",
            json!({"operation":operation,"proposal":proposal,"decision":receipt,"revision":revision}),
        )?;
        Ok(receipt)
    })
}
/// Bounds per plan/user guidance and across plans. A new plan alone cannot reset
/// the autonomous budget (state.py:2152-2192).
pub fn return_to_coordinator_tx(
    db: &Transaction<'_>,
    task: &str,
    reason: &str,
) -> CoreResult<bool> {
    let (state, revision) = task_tx(db, task)?;
    if state.terminal() || state == TaskState::Paused {
        return refuse("task_not_resumable");
    }
    let returned:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='RETURNED_TO_COORDINATOR'
        AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=? AND type IN ('PLAN_ACCEPTED','USER_ANSWER_RECEIVED','USER_STEERING')),0)",
        (task,task),|r|r.get(0))?;
    let autonomous:i64=db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='RETURNED_TO_COORDINATOR'
        AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=? AND type IN ('USER_STEERING','USER_ANSWER_RECEIVED','WORKER_USER_ANSWER_RECEIVED')),0)",
        (task,task),|r|r.get(0))?;
    if returned >= MAX_COORDINATOR_RETURNS as i64
        || autonomous >= MAX_AUTONOMOUS_COORDINATOR_RETURNS as i64
    {
        return Ok(false);
    }
    // Preserve the failed specialist's handoff at the routing boundary.  The
    // coordinator must receive more than the generic `worker_failed` reason:
    // role, objective, last report and the worker's exact blocker are already
    // durable facts on the node and let it choose a repair, consultation, or
    // owner question without blindly dispatching the same specialist again.
    let handoff: Option<Value> = db
        .query_row(
            "SELECT role,objective,assignment_ref,result_json,status
             FROM runtime_nodes
             WHERE task_id=? AND revision=? AND stage='execution'
             ORDER BY CASE status WHEN 'failed' THEN 0 WHEN 'cancelled' THEN 1 ELSE 2 END,
                      rowid DESC LIMIT 1",
            (task, revision),
            |r| {
                let role: String = r.get(0)?;
                let objective: String = r.get(1)?;
                let assignment: Option<String> = r.get(2)?;
                let raw: Option<String> = r.get(3)?;
                let status: String = r.get(4)?;
                let result: Value = raw
                    .as_deref()
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(Value::Null);
                Ok(json!({
                    "role": role,
                    "objective": objective,
                    "assignment_ref": assignment,
                    "status": status,
                    "report_ref": result["report_ref"],
                    "summary": result["summary"],
                    "blocker": result.get("blocker").cloned()
                        .or_else(|| result.get("error").cloned())
                        .or_else(|| result.get("reason").cloned()),
                }))
            },
        )
        .optional()?;
    let mut payload = json!({"reason":reason});
    if let Some(handoff) = handoff {
        payload["handoff"] = handoff;
    }
    event_tx(db, task, "RETURNED_TO_COORDINATOR", payload)?;
    waits::cancel_tx(db, task, None, "returned_to_coordinator", "cancelled")?;
    cancel_nodes_tx(db, task, "returned_to_coordinator")?;
    revoke_tx(db, task, None)?;
    state_tx(db, task, TaskState::Recovering, None)?;
    Ok(true)
}
