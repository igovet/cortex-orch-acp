//! Reconciled lost-worker retries. Python: orchestration/recovery.py:110-252;
//! engine/retry_policy.py:134-155. No prompt/process action runs in this module.
use crate::{
    error::refuse,
    lifecycle::{domain, event_tx, waits},
    ports::ProcessObservation,
    CoreError, CoreResult, TaskState,
};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};
#[derive(Debug, Clone, Copy)]
pub struct RetryWindow {
    pub first_delay: f64,
    pub max_delay: f64,
    pub window: f64,
}
impl Default for RetryWindow {
    fn default() -> Self {
        Self {
            first_delay: 5.0,
            max_delay: 60.0,
            window: 600.0,
        }
    }
}
#[derive(Debug, PartialEq)]
pub struct WindowDecision {
    pub may_retry: bool,
    pub retry_number: usize,
    pub delay: f64,
}
pub fn windowed_retry(times: &[f64], now: f64, window: RetryWindow) -> CoreResult<WindowDecision> {
    if !now.is_finite()
        || window.first_delay < 0.0
        || window.max_delay < window.first_delay
        || window.window <= 0.0
        || !window.first_delay.is_finite()
        || !window.max_delay.is_finite()
        || !window.window.is_finite()
        || times.iter().any(|t| !t.is_finite() || *t > now)
        || times.windows(2).any(|p| p[0] > p[1])
    {
        return refuse("invalid_retry_window");
    }
    let mut start = times.len().saturating_sub(1);
    while start > 0 && times[start] - times[start - 1] <= window.window {
        start -= 1;
    }
    let number = (times.len() - start).max(1);
    let may_retry = times.is_empty() || (now - times[start]).max(0.0) < window.window;
    Ok(WindowDecision {
        may_retry,
        retry_number: number,
        delay: if may_retry {
            (window.first_delay * 2f64.powi((number - 1).min(1024) as i32)).min(window.max_delay)
        } else {
            0.0
        },
    })
}
/// Observation is for this exact provisioned attempt, collected by EngineDriver
/// before entering the transaction. Unknown is never absence.
pub struct RetryObservation<'a> {
    pub assignment: &'a str,
    pub attempt: Option<&'a str>,
    pub process: ProcessObservation,
    pub now: f64,
    pub window: RetryWindow,
}
pub fn retry_lost_tx(
    db: &Transaction<'_>,
    task: &str,
    observed: &RetryObservation<'_>,
) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    if !matches!(state, TaskState::Executing | TaskState::Reviewing) {
        return refuse("retry_not_expected");
    }
    let assignment = observed.assignment;
    let row:Option<(String,i64,Option<String>,String)>=db.query_row("SELECT node_id,revision,result_json,status FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
        (task,assignment),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let (node, rev, result, status) = row.ok_or(CoreError::Refused("stale_assignment"))?;
    if rev != revision || result.is_some() || !["running", "failed"].contains(&status.as_str()) {
        return refuse("lost_worker_result_changed");
    }
    let provision: Option<Option<String>> = db
        .query_row(
            "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
            (task, assignment),
            |r| r.get(0),
        )
        .optional()?;
    if provision.as_ref().map(|v| v.as_deref()) != observed.attempt.map(Some) {
        // None/None is the positively unprovisioned case.
        if !(provision.is_none() && observed.attempt.is_none()) {
            return refuse("retry_attempt_changed");
        }
    }
    if provision.is_some() && observed.process != ProcessObservation::ConfirmedExited {
        return refuse("lost_worker_termination_unconfirmed");
    }
    let published:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM task_events WHERE task_id=? AND event_type='worker_result' AND superseded=0
        AND assignment_ref IN (?,?))",(task,assignment,observed.attempt),|r|r.get(0))?;
    if published {
        return refuse("lost_worker_result_changed");
    }
    let mut wait = waits::current_tx(db, task, assignment)?;
    let continuing = match wait.as_ref() {
        Some(w) => waits::preserved_tx(db, w)?,
        None => false,
    };
    if continuing
        && !(wait.as_ref().unwrap()["status"] == "ready"
            && waits::valid_tx(db, wait.as_ref().unwrap())?)
    {
        return refuse("worker_wait_not_ready");
    }
    let failure: Option<String> = db
        .query_row(
            "SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref=?
        AND event_type='assignment_failed' AND superseded=0 ORDER BY id DESC LIMIT 1",
            (task, assignment),
            |r| r.get(0),
        )
        .optional()?;
    let failure: Value = serde_json::from_str(failure.as_deref().unwrap_or("{}"))?;
    let reason = failure["error"].as_str().unwrap_or("");
    let class = failure["failure_class"].as_str().unwrap_or(
        if reason.is_empty() || reason == "recovery_worker_lost" {
            "worker_lost"
        } else {
            ""
        },
    );
    if [
        "refusal",
        "provider_restriction",
        "cancellation",
        "no_available_route",
        "pinned_route_unavailable",
        "invalid_routing_overrides",
        "route_denied",
    ]
    .contains(&reason)
        || reason.starts_with("fallback_refused:") && class != "capacity_unavailable"
    {
        return refuse("worker_failure_not_retryable");
    }
    if reason.starts_with("engine_authentication_required:") {
        return refuse("engine_authentication_required");
    }
    let rows=db.prepare("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED'
        AND json_extract(payload_json,'$.node')=? AND json_extract(payload_json,'$.revision')=? ORDER BY id")?
        .query_map((task,&node,revision),|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
    let retries = rows
        .iter()
        .map(|s| Ok(serde_json::from_str::<Value>(s)?))
        .collect::<CoreResult<Vec<_>>>()?;
    let auth = class == "authorization_loss" || reason == "authorization_loss";
    if auth
        && (failure["auth_replay_safe"] != true || retries.iter().any(|r| r["auth_retry"] == true))
    {
        return refuse("engine_authentication_required");
    }
    let route = class == "capacity_unavailable" || failure["route_failure"] == true;
    let infrastructure = [
        "engine_failure",
        "transport_failure",
        "worker_lost",
        "authorization_loss",
    ]
    .contains(&class)
        && !route;
    let ordinary = retries
        .iter()
        .filter(|r| {
            r["route_failure"] != true
                && r["failed_at"].is_null()
                && r["auth_retry"] != true
                && r["owner_wait_continuation"] != true
        })
        .count();
    let mut retry = json!({"node":node,"revision":revision,"retry":retries.len()+1,"previous_assignment":assignment,
        "observed_session":if provision.is_some(){"absent"}else{"not_provisioned"},"failure_class":class});
    if continuing {
        retry["owner_wait_continuation"] = json!(true);
    } else if route {
        if retries
            .iter()
            .filter(|r| r["route_failure"] == true)
            .count()
            >= 6
        {
            return refuse("worker_retry_limit");
        }
        if auth {
            return refuse("engine_authentication_required");
        }
        retry["route_failure"] = json!(true);
    } else if infrastructure {
        let mut times = retries
            .iter()
            .filter_map(|r| r["failed_at"].as_f64())
            .collect::<Vec<_>>();
        times.push(observed.now);
        times.sort_by(f64::total_cmp);
        let decision = windowed_retry(&times, observed.now, observed.window)?;
        if !decision.may_retry {
            return refuse("worker_retry_limit");
        }
        retry["failed_at"] = json!(observed.now);
        retry["retry_not_before"] = json!(observed.now + decision.delay);
        retry["infrastructure_retry"] = json!(decision.retry_number);
    } else if ordinary >= 2 {
        return refuse("worker_retry_limit");
    }
    if auth {
        retry["auth_retry"] = json!(true);
    }
    if let Some(attempt) = observed.attempt {
        let unknown:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_requests r JOIN trusted_channels c ON c.channel_id=r.actor
            WHERE r.task_id=? AND c.attempt_id=? AND r.operation IN
            ('command_run','project_write','project_edit','project_mkdir','project_delete','project_skill_link','draft_write','external_call')
            AND coalesce(json_extract(r.result_json,'$.complete'),0)!=1 AND coalesce(json_extract(r.result_json,'$.status'),'')!='complete')",(task,attempt),|r|r.get(0))?;
        if unknown {
            return refuse("lost_worker_action_outcome_unknown");
        }
        // Release is justified by this exact confirmed process observation.
        db.execute("UPDATE capacity_reservations SET state='released',updated_at=?,release_observation='process_exited'
            WHERE task_id=? AND attempt_id=? AND state IN ('reserved','ambiguous')",(crate::now(),task,attempt))?;
    }
    domain::revoke_tx(db, task, Some(assignment))?;
    domain::graph_attempt_tx(
        db,
        task,
        assignment,
        "failed",
        Some(if continuing {
            "owner_wait_continued"
        } else {
            "recovered_replacement"
        }),
    )?;
    if continuing {
        let w = wait.as_mut().unwrap();
        w["status"] = json!("continued");
        waits::change_tx(db, w)?;
        event_tx(
            db,
            task,
            "WORKER_WAIT_CONTINUATION_SCHEDULED",
            json!({"wait":w,"node_id":node,"revision":revision}),
        )?;
    } else {
        waits::cancel_tx(
            db,
            task,
            Some(assignment),
            "recovered_replacement",
            "orphaned",
        )?;
    }
    let changed=db.execute("UPDATE runtime_nodes SET status='pending',assignment_ref=NULL WHERE task_id=? AND assignment_ref=? AND result_json IS NULL",
        (task,assignment))?;
    if changed != 1 {
        return refuse("lost_worker_result_changed");
    }
    event_tx(db, task, "ASSIGNMENT_RETRIED", retry)?;
    Ok(())
}
