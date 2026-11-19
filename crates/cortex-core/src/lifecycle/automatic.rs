//! Runtime-owned continuation for a task admitted by the Automatic sprint queue.
//! Events are additive in format 12. No existing task acquires the policy on open.
use super::{domain, event_tx, waits};
use crate::{CoreError, CoreResult, TaskState};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

pub const POLICY: &str = "automatic_continuation";
/// Ask a senior consultant on the fourth recovery, then allow four further repairs.
const CONSULT_AT: i64 = 4;
const HARD_CAP: i64 = 8;
const PENDING: &str = "automatic_continuation_pending";
const PAUSED: &str = "automatic_continuation_paused";
pub const COORDINATOR_NOTE: &str = "Automatic sprint policy: do not ask the owner how to handle runtime failures, repairs, retries or approvals. Ask only for decisions about the work itself. Keep using the normal permission and access routes. Never waive required review or verification.";

pub(crate) fn bind_tx(db: &Transaction<'_>, task: &str) -> CoreResult<()> {
    event_tx(db, task, "AUTOMATIC_POLICY_BOUND", json!({"policy":POLICY}))?;
    Ok(())
}

pub fn enabled(db: &Connection, task: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_POLICY_BOUND'
                AND actor='runtime' AND json_extract(payload_json,'$.policy')=?)",
        (task, POLICY),
        |r| r.get(0),
    )?)
}

fn count(db: &Connection, task: &str) -> CoreResult<i64> {
    Ok(db.query_row(
        "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_CONTINUATION'",
        [task],
        |r| r.get(0),
    )?)
}

fn latest(db: &Connection, task: &str, kind: &str) -> CoreResult<Option<(i64, Value)>> {
    let row = db.query_row(
        "SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type=? ORDER BY id DESC LIMIT 1",
        (task, kind),
        |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)),
    ).optional()?;
    row.map(|(id, raw)| Ok((id, serde_json::from_str(&raw)?)))
        .transpose()
}

#[derive(Debug)]
pub struct Owed {
    pub event: i64,
    pub evidence: Value,
    pub external: bool,
}

/// Only our exact hold can be resumed. Owner pause, cancellation, real questions and
/// unknown-outcome recovery have separate authority and are never overridden.
pub fn owed(db: &Connection, task: &str) -> CoreResult<Option<Owed>> {
    let hold: Option<String> = db
        .query_row(
            "SELECT blocker FROM runtime_tasks WHERE task_id=? AND state='RECOVERING'",
            [task],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if !matches!(hold.as_deref(), Some(PENDING | PAUSED)) || !enabled(db, task)? {
        return Ok(None);
    }
    let paused = hold.as_deref() == Some(PAUSED);
    let kind = if paused {
        "AUTOMATIC_PAUSED"
    } else {
        "AUTOMATIC_CONTINUATION"
    };
    Ok(latest(db, task, kind)?.and_then(|(event, evidence)| {
        let external = paused && evidence["pause_kind"] == "external_blocker";
        (!paused || external).then_some(Owed {
            event,
            evidence,
            external,
        })
    }))
}

pub fn held(db: &Connection, task: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_tasks WHERE task_id=? AND state='RECOVERING' AND blocker IN (?,?))",
        (task, PENDING, PAUSED), |r| r.get(0),
    )?)
}

pub fn projection(db: &Connection, task: &str) -> CoreResult<Option<Value>> {
    if !enabled(db, task)? {
        return Ok(None);
    }
    let last = latest(db, task, "AUTOMATIC_CONTINUATION")?;
    let paused: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_tasks WHERE task_id=? AND state='RECOVERING' AND blocker=?)",
        (task, PAUSED), |r| r.get(0),
    )?;
    let pause = if paused {
        latest(db, task, "AUTOMATIC_PAUSED")?.map(|(id, v)| {
            json!({
                "eventId":id.to_string(),"kind":v["pause_kind"],"reason":v["reason"]
            })
        })
    } else {
        None
    };
    Ok(Some(json!({
        "policy":POLICY, "continuationCount":count(db, task)?,
        "lastContinuationReason":last.map(|(_, v)| v["reason"].clone()), "pause":pause,
    })))
}

/// Runtime-built input to the same consultant node and packet builder used by
/// a normal escalation. Pending until that path records CONSULTATION_REQUIRED.
pub(super) fn consultation(db: &Connection, task: &str) -> CoreResult<Option<Value>> {
    let Some((id, request)) = latest(db, task, "AUTOMATIC_CONSULTATION_REQUESTED")? else {
        return Ok(None);
    };
    let scheduled: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='CONSULTATION_REQUIRED' AND id>?)",
        (task, id), |r| r.get(0),
    )?;
    if scheduled {
        return Ok(None);
    }
    let stop = request["continuation_event"]
        .as_i64()
        .ok_or(CoreError::Refused("automatic_evidence_missing"))?;
    let raw: String = db.query_row(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND id=? AND type='AUTOMATIC_CONTINUATION'",
        (task, stop), |r| r.get(0),
    )?;
    let evidence: Value = serde_json::from_str(&raw)?;
    let reports: Vec<_> = evidence["reports"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["reportId"].as_str())
        .take(16)
        .collect();
    Ok(Some(
        json!({"operation":"escalation_request", "revision":request["revision"], "automatic":true,
        "proposal":{"question":"What should the coordinator change to clear the retained process failure without waiving review, checks or permissions?",
            "reason":format!("Automatic continuation {}: {} (runtime event {stop}; evidence {})",
                CONSULT_AT, request["reason"], evidence["events"]), "report_ids":reports}}),
    ))
}

/// A coordinator cannot turn a failed or unobserved mandatory check into a
/// passing result by omitting it from a repair plan or verification revision.
pub(super) fn require_failed_checks(
    db: &Connection,
    task: &str,
    proposed: &Value,
) -> CoreResult<()> {
    if !enabled(db, task)? {
        return Ok(());
    }
    let dropped: bool = db.query_row(
        "SELECT EXISTS(
            SELECT 1 FROM runtime_events e, json_each(e.payload_json,'$.commands') c
            WHERE e.task_id=? AND e.type='VERIFICATION_FAILED'
              AND NOT EXISTS (SELECT 1 FROM json_each(?) p WHERE p.value=c.value)
        )",
        (task, proposed.to_string()),
        |r| r.get(0),
    )?;
    if dropped {
        return crate::error::refuse("automatic_required_check_missing");
    }
    Ok(())
}

pub fn route_blocked(reason: &str) -> bool {
    matches!(
        reason,
        "native_route_authorization_unavailable"
            | "native_task_route_unavailable"
            | "no_available_route"
            | "pinned_route_unavailable"
            | "user_requested_route_unavailable"
            | "portable_coordinator_unavailable"
            | "engine_unavailable"
            | "engine_quota_exhausted"
            | "engine_authentication_required"
            | "authorization_required"
            | "connection_unavailable"
            | "native_route_executable_unavailable"
    )
}

fn evidence(db: &Connection, task: &str, revision: i64, reason: &str) -> CoreResult<Value> {
    let events = db.prepare(
        "SELECT id,type FROM runtime_events WHERE task_id=?1 AND type IN (
          'VERIFICATION_FAILED','NATIVE_DRIVER_STOPPED','NATIVE_SESSION_START_FAILED',
          'NATIVE_WORKER_ROUTE_BLOCKED','REVIEW_RERUN','ASSIGNMENT_RETRIED',
          'NATIVE_RECOVERY_REQUIRED','REPLAN_REQUEST_ACCEPTED','CONSULTATION_REQUIRED',
          'WORKER_FAILED','NATIVE_PROCESS_SETTLED','NATIVE_PROCESS_UNCONFIRMED')
         AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='AUTOMATIC_RESUMED'),0)
         ORDER BY id DESC LIMIT 24",
    )?.query_map([task], |r| Ok(json!({"id":r.get::<_, i64>(0)?, "type":r.get::<_, String>(1)?})))?
        .collect::<Result<Vec<_>,_>>()?;
    let reports = db.prepare(
        "SELECT node_id,assignment_ref,json_extract(result_json,'$.report_ref')
         FROM runtime_nodes WHERE task_id=? AND revision=? AND result_json IS NOT NULL ORDER BY rowid",
    )?.query_map((task,revision), |r| Ok(json!({"node":r.get::<_, String>(0)?,
        "assignmentRef":r.get::<_, Option<String>>(1)?, "reportId":r.get::<_, Option<String>>(2)?})))?
        .collect::<Result<Vec<_>,_>>()?;
    let mut details =
        json!({"reason":reason,"revision":revision,"events":events,"reports":reports});
    let worker: Option<String> = db.query_row(
        "SELECT payload_json FROM runtime_events WHERE task_id=?1 AND type='NATIVE_WORKER_ROUTE_BLOCKED'
            AND json_extract(payload_json,'$.revision')=?2 AND json_extract(payload_json,'$.reason')=?3
            AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='AUTOMATIC_RESUMED'),0)
         ORDER BY id DESC LIMIT 1",
        (task,revision,reason), |r| r.get(0),
    ).optional()?;
    if let Some(worker) = worker {
        details["worker_route"] = serde_json::from_str(&worker)?;
    }
    if reason == "engine_authentication_required" {
        let auth = db.query_row(
            "SELECT id,json_extract(payload_json,'$.connection'),created_at
             FROM runtime_events WHERE task_id=?1 AND type='NATIVE_AUTOMATIC_AUTH_BLOCKED'
             AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='AUTOMATIC_RESUMED'),0)
             ORDER BY id DESC LIMIT 1",
            [task], |r| Ok(json!({"event":r.get::<_, i64>(0)?,
                "connection":r.get::<_, String>(1)?, "at":r.get::<_, String>(2)?})),
        ).optional()?;
        details["authentication"] = json!(auth);
    }
    Ok(details)
}

/// Records a stop and fences the old revision. It does not prove that a child
/// exited, or give the next coordinator any authority over unresolved effects.
pub(super) fn stop_tx(db: &Transaction<'_>, task: &str, reason: &str) -> CoreResult<TaskState> {
    if held(db, task)? {
        return Ok(TaskState::Recovering);
    }
    let (state, revision) = domain::task_tx(db, task)?;
    if state == TaskState::WaitingUser
        || waits::projection_tx(db, task)?
            .iter()
            .any(|w| matches!(w["status"].as_str(), Some("open" | "ready" | "resuming")))
    {
        // A real user/access decision is already pending. Preserve its exact wait.
        return Ok(state);
    }
    let mut details = evidence(db, task, revision, reason)?;
    let count = count(db, task)?;
    let exhausted = budget_used(db, task)? >= HARD_CAP;
    let unknown = reason.ends_with("outcome_unknown") || reason.ends_with("unconfirmed");
    let pause = if unknown {
        Some("outcome_unknown")
    } else if exhausted {
        Some("continuation_cap")
    } else if route_blocked(reason) {
        Some("external_blocker")
    } else {
        None
    };
    waits::cancel_tx(db, task, None, "automatic_stop", "cancelled")?;
    domain::cancel_nodes_tx(db, task, "automatic_stop")?;
    domain::revoke_tx(db, task, None)?;
    if let Some(kind) = pause {
        details["pause_kind"] = json!(kind);
        event_tx(db, task, "AUTOMATIC_PAUSED", details)?;
        domain::state_tx(db, task, TaskState::Recovering, Some(PAUSED))?;
    } else {
        details["count"] = json!(count + 1);
        let id = event_tx(db, task, "AUTOMATIC_CONTINUATION", details)?;
        if count + 1 == CONSULT_AT {
            event_tx(
                db,
                task,
                "AUTOMATIC_CONSULTATION_REQUESTED",
                json!({
                    "continuation_event":id, "reason":reason, "revision":revision,
                }),
            )?;
        }
        domain::state_tx(db, task, TaskState::Recovering, Some(PENDING))?;
    }
    Ok(TaskState::Recovering)
}

/// Classify a crash whose effects cannot be reconciled. Retain all existing
/// attempt/wait evidence and never automatically retry an unknown side effect.
pub fn pause_unsettled_tx(db: &Transaction<'_>, task: &str, reason: &str) -> CoreResult<bool> {
    let (state, revision) = domain::task_tx(db, task)?;
    if !enabled(db, task)?
        || state.terminal()
        || matches!(state, TaskState::Paused | TaskState::WaitingUser)
        || waits::projection_tx(db, task)?
            .iter()
            .any(|w| matches!(w["status"].as_str(), Some("open" | "ready" | "resuming")))
    {
        return Ok(false);
    }
    if held(db, task)?
        && latest(db, task, "AUTOMATIC_PAUSED")?
            .is_some_and(|(_, v)| v["pause_kind"] == "outcome_unknown" && v["reason"] == reason)
    {
        return Ok(true);
    }
    let mut details = evidence(db, task, revision, reason)?;
    details["pause_kind"] = json!("outcome_unknown");
    event_tx(db, task, "AUTOMATIC_PAUSED", details)?;
    domain::state_tx(db, task, TaskState::Recovering, Some(PAUSED))?;
    Ok(true)
}

/// Stop reasons that name nothing but an owned process whose exit was not confirmed.
const EXIT_UNCONFIRMED: [&str; 2] = [
    "native_shutdown_unconfirmed",
    "lost_worker_termination_unconfirmed",
];

/// The host settled, with process evidence, the unconfirmed exits that held this task:
/// every owned process is now known to be gone and no dispatched effect is unknown
/// ([`require_settled_execution`]). A RECOVERING task stopped with no recorded reason,
/// with only such an exit as its reason, or paused (`outcome_unknown`) only for one
/// (including the restart's `native_session_outcome_unknown`, recorded only when its
/// processes were unsettled), is then an ordinary interrupted turn and continues like
/// any automatic stop: budgeted and never waiving review or verification. An owner
/// pause, a question or wait, or any other unknown outcome keeps its own authority.
/// Returns whether a continuation is now owed.
pub fn exit_settled_tx(db: &Transaction<'_>, task: &str) -> CoreResult<bool> {
    if !enabled(db, task)? {
        return Ok(false);
    }
    let Some(blocker) = db
        .query_row(
            "SELECT coalesce(blocker,'') FROM runtime_tasks WHERE task_id=? AND state='RECOVERING'",
            [task],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    else {
        return Ok(false);
    };
    let exit_only = match blocker.as_str() {
        "" => true,
        PAUSED => latest(db, task, "AUTOMATIC_PAUSED")?.is_some_and(|(_, v)| {
            v["pause_kind"] == "outcome_unknown"
                && v["reason"].as_str().is_some_and(|r| {
                    EXIT_UNCONFIRMED.contains(&r) || r == "native_session_outcome_unknown"
                })
        }),
        other => EXIT_UNCONFIRMED.contains(&other),
    };
    if !exit_only
        || waits::projection_tx(db, task)?
            .iter()
            .any(|w| matches!(w["status"].as_str(), Some("open" | "ready" | "resuming")))
        || require_settled_execution(db, task).is_err()
    {
        return Ok(false);
    }
    if !blocker.is_empty() {
        domain::state_tx(db, task, TaskState::Recovering, None)?;
    }
    stop_tx(db, task, "native_process_exit_settled")?;
    Ok(owed(db, task)?.is_some())
}

/// Replaces an unlaunched recovery with a route hold. The continuation budget
/// was already charged at the stop; a failed observation never charges it again.
pub fn route_pause_tx(
    db: &Transaction<'_>,
    task: &str,
    event: i64,
    reason: &str,
) -> CoreResult<()> {
    if let Some(owed) = owed(db, task)?.filter(|o| o.event == event && !o.external) {
        let mut details = owed.evidence;
        details["continuation_event"] = json!(event);
        details["process_reason"] = details["reason"].clone();
        details["reason"] = json!(reason);
        details["pause_kind"] = json!("external_blocker");
        event_tx(db, task, "AUTOMATIC_PAUSED", details)?;
        domain::state_tx(db, task, TaskState::Recovering, Some(PAUSED))?;
    }
    Ok(())
}

/// The owner can renew the bounded retry budget for this exact hold, never
/// for an unconfirmed effect or a route the scheduler must qualify.
pub(super) fn owner_cap(db: &Connection, task: &str) -> CoreResult<Option<i64>> {
    let cap = projection(db, task)?
        .filter(|value| value["pause"]["kind"] == "continuation_cap")
        .and_then(|value| value["pause"]["eventId"].as_str()?.parse().ok());
    Ok(cap)
}

fn budget_used(db: &Connection, task: &str) -> CoreResult<i64> {
    Ok(db.query_row(
        "SELECT count(*) FROM runtime_events WHERE task_id=?1 AND type='AUTOMATIC_CONTINUATION'
         AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1
             AND type='AUTOMATIC_CAP_RENEWED'),0)",
        [task],
        |r| r.get(0),
    )?)
}

/// Called by the owning host only, after qualifying the current route. A fresh
/// event identity avoids consuming the wrong recovery after a concurrent control.
pub fn resume_tx(db: &Transaction<'_>, task: &str, event: i64) -> CoreResult<Option<Value>> {
    let Some(owed) = owed(db, task)?.filter(|o| o.event == event) else {
        return Ok(None);
    };
    require_settled_execution(db, task)?;
    domain::settle_recovery_tx(db, task, TaskState::Planning)?;
    event_tx(
        db,
        task,
        "AUTOMATIC_RESUMED",
        json!({"stop_event":event,"evidence":owed.evidence}),
    )?;
    Ok(Some(owed.evidence))
}

/// Check the retained processes and dispatched effects before handing the folder to more work.
pub fn require_settled_execution(db: &Connection, task: &str) -> CoreResult<()> {
    let live: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND status!='terminated')
         OR EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND state!='released')",
        (task, task), |r| r.get(0),
    )?;
    if live {
        return Err(CoreError::Refused("lost_worker_termination_unconfirmed"));
    }
    require_known_effects(db, task)
}

pub(super) fn require_known_effects(db: &Connection, task: &str) -> CoreResult<()> {
    // A settled child does not settle a lost command/tool receipt. Use the
    // same retained effects as native boot recovery before issuing a new plan.
    let effects_unknown: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=?
            AND operation IN ('outside_command','outside_write','command_run','project_write','project_edit',
                'project_mkdir','project_delete','project_skill_link','draft_write','external_call')
            AND json_extract(result_json,'$.complete') IS NOT 1
            AND json_extract(result_json,'$.status') IS NOT 'complete'
            AND json_extract(result_json,'$.settled') IS NOT 1)",
        [task], |r| r.get(0),
    )?;
    if effects_unknown {
        return Err(CoreError::Refused("native_outside_outcome_unknown"));
    }
    Ok(())
}
