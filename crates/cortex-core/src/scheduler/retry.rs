//! Reconciled lost-worker retries. Python: orchestration/recovery.py:110-252;
//! engine/retry_policy.py:134-155. No prompt/process action runs in this module.
use crate::{
    error::refuse,
    lifecycle::{domain, event_tx, waits},
    ports::ProcessObservation,
    CoreError, CoreResult, TaskState,
};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};
/// Why an attempt ended, when it is not the model's failure: the engine, the
/// transport, the session vanishing without the model ending its turn, a lost
/// login, no capacity, the owner pausing, the runtime restarting, the sandbox not
/// being prepared, or the verifier's environment. A retry after one of these never
/// escalates the node, never takes a ladder rung and never replans.
pub const INFRASTRUCTURE_CLASSES: [&str; 9] = [
    "engine_failure",
    "transport_failure",
    "worker_lost",
    "authorization_loss",
    "capacity_unavailable",
    "owner_paused",
    "runtime_restart",
    "sandbox_unavailable",
    "verifier_environment",
];
/// Whether a recorded failure class is infrastructure (see [`INFRASTRUCTURE_CLASSES`]).
/// Every other class (`result_refused`, `degenerate_turn`, `indeterminate`, ...) is
/// the attempt's own quality and may escalate its node.
pub fn infrastructure_class(class: &str) -> bool {
    INFRASTRUCTURE_CLASSES.contains(&class)
}
/// Infrastructure retries of one node lineage (the node and its review fixes, in
/// every plan revision since the task was last reopened) that run on the ordinary
/// backoff. After this many the node keeps waiting on a long, growing delay
/// ([`INFRASTRUCTURE_WAIT_FIRST`]) instead of stopping: no rung, no replan.
pub const INFRASTRUCTURE_RETRY_CAP: usize = 6;
/// The first delay of an exhausted lineage's infrastructure wait, in seconds; it
/// doubles with every further loss up to [`INFRASTRUCTURE_WAIT_MAX`].
pub const INFRASTRUCTURE_WAIT_FIRST: f64 = 300.0;
pub const INFRASTRUCTURE_WAIT_MAX: f64 = 3600.0;
/// Classes that are not a failure of the node at all (the owner paused, the runtime
/// restarted): retried at once and never counted toward [`INFRASTRUCTURE_RETRY_CAP`].
fn interruption(class: &str) -> bool {
    matches!(class, "owner_paused" | "runtime_restart")
}
/// The node a review fix `_fix_<n>_<node>` repairs, else the node itself.
pub fn node_base(node: &str) -> &str {
    node.strip_prefix("_fix_")
        .and_then(|rest| rest.split_once('_'))
        .filter(|(n, _)| n.parse::<u32>().is_ok())
        .map(|(_, base)| base)
        .unwrap_or(node)
}
/// The infrastructure retries already counted for `node`'s lineage since the task
/// was last reopened, across plan revisions.
pub fn lineage_infrastructure_retries(
    db: &Connection,
    task: &str,
    node: &str,
) -> CoreResult<usize> {
    let base = node_base(node);
    let rows = db
        .prepare(
            "SELECT json_extract(payload_json,'$.node'),coalesce(json_extract(payload_json,'$.failure_class'),'')
             FROM runtime_events WHERE task_id=?1 AND type='ASSIGNMENT_RETRIED'
             AND json_extract(payload_json,'$.failed_at') IS NOT NULL
             AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0)",
        )?
        .query_map([task], |r| {
            Ok((r.get::<_, Option<String>>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .iter()
        .filter(|(n, class)| {
            n.as_deref().is_some_and(|n| node_base(n) == base)
                && infrastructure_class(class)
                && !interruption(class)
        })
        .count())
}
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
/// Whether a lost worker's dispatched action has a known end, so that a fresh
/// attempt of its node may run. A complete receipt has one. A settled receipt has
/// one when the runtime observed how its process ended (the supervisor's final
/// status: an exit code, a signal, an owner stop or a timeout), when it provably
/// never launched (`launched: false`, outcome `not_started`), when the supervisor
/// finished but its output could not be verified (`supervisor_finished`), or when a
/// read-only command's PID namespace ended with its supervisor
/// (`supervisor_gone_pid_namespace`: it could not write). A receipt settled only on
/// proof that its process is gone, without an observed end (outcome `unknown`:
/// an earlier boot, an empty process group, an owner acknowledgement), and a
/// receipt that is not settled, still block: the action may have taken effect,
/// and repeating the node could repeat it. The coordinator decides those.
pub fn action_outcome_known(receipt: &Value) -> bool {
    if receipt["complete"] == true || receipt["status"] == "complete" {
        return true;
    }
    if receipt["settled"] != true
        || receipt["outcome_unknown"] == true
        || receipt["failure"] == "command_outcome_unknown"
    {
        return false;
    }
    match receipt["outcome"].as_str() {
        None => true,
        Some("not_started") => receipt["launched"] == false,
        Some("unknown") => {
            receipt["exit_code"].is_i64()
                || matches!(
                    receipt["settlement_evidence"].as_str(),
                    Some("supervisor_finished" | "supervisor_gone_pid_namespace")
                )
        }
        Some(_) => false,
    }
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
    let infrastructure = infrastructure_class(class) && !route;
    let ordinary = retries
        .iter()
        .filter(|r| {
            r["route_failure"] != true
                && r["failed_at"].is_null()
                && r["auth_retry"] != true
                && r["owner_wait_continuation"] != true
                && r["obstacle_retry"] != true
        })
        .count();
    let mut retry = json!({"node":node,"revision":revision,"retry":retries.len()+1,"previous_assignment":assignment,
        "observed_session":if provision.is_some(){"absent"}else{"not_provisioned"},"failure_class":class});
    // Why the attempt ended, as the runtime observed it (engine exit, transport close,
    // runtime cancel, owner pause, restart, a bounded redacted stderr tail).
    if failure["ending"].is_object() {
        retry["ending"] = failure["ending"].clone();
    }
    if continuing {
        retry["owner_wait_continuation"] = json!(true);
    } else if route {
        let routed = retries
            .iter()
            .filter(|r| r["route_failure"] == true)
            .count();
        if auth {
            return refuse("engine_authentication_required");
        }
        retry["route_failure"] = json!(true);
        if routed >= INFRASTRUCTURE_RETRY_CAP {
            // No capacity for this node is an outage, never the model's failure: the
            // node waits on a growing delay without a rung (no worker_retry_limit).
            let delay = infrastructure_wait(routed - INFRASTRUCTURE_RETRY_CAP);
            retry["infrastructure_wait"] = json!(true);
            retry["retry_not_before"] = json!(observed.now + delay);
            if routed == INFRASTRUCTURE_RETRY_CAP {
                loop_broken_tx(db, task, &node, class, routed + 1, delay)?;
            }
        }
    } else if infrastructure && interruption(class) {
        // The owner paused or the runtime restarted: not a failure of the node.
        retry["failed_at"] = json!(observed.now);
        retry["retry_not_before"] = json!(observed.now);
        retry["infrastructure_retry"] = json!(0);
    } else if infrastructure {
        let mut times = retries
            .iter()
            .filter_map(|r| r["failed_at"].as_f64())
            .collect::<Vec<_>>();
        times.push(observed.now);
        times.sort_by(f64::total_cmp);
        let decision = windowed_retry(&times, observed.now, observed.window)?;
        let lineage = lineage_infrastructure_retries(db, task, &node)?;
        retry["failed_at"] = json!(observed.now);
        retry["infrastructure_retry"] = json!(decision.retry_number);
        retry["lineage_infrastructure_retry"] = json!(lineage + 1);
        if lineage >= INFRASTRUCTURE_RETRY_CAP {
            // The lineage's ordinary infrastructure budget is spent (across plan
            // revisions, not only inside one window): an infrastructure wait without
            // a rung or a replan, rechecked by the runtime when the delay ends.
            let delay = infrastructure_wait(lineage - INFRASTRUCTURE_RETRY_CAP);
            retry["infrastructure_wait"] = json!(true);
            retry["retry_not_before"] = json!(observed.now + delay);
            if lineage == INFRASTRUCTURE_RETRY_CAP {
                loop_broken_tx(db, task, &node, class, lineage + 1, delay)?;
            }
        } else {
            // A streak that kept failing for a whole window waits the longest
            // ordinary delay; it is never turned into the model's failure.
            let delay = if decision.may_retry {
                decision.delay
            } else {
                observed.window.max_delay
            };
            retry["retry_not_before"] = json!(observed.now + delay);
        }
    } else if ordinary >= 2 {
        return refuse("worker_retry_limit");
    }
    if auth {
        retry["auth_retry"] = json!(true);
    }
    if let Some(attempt) = observed.attempt {
        let receipts = db
            .prepare(
                "SELECT r.request_key,r.operation,r.result_json FROM runtime_requests r JOIN trusted_channels c ON c.channel_id=r.actor
            WHERE r.task_id=? AND c.attempt_id=? AND r.operation IN
            ('command_run','project_write','project_edit','project_mkdir','project_delete','project_skill_link','draft_write','external_call')
            AND coalesce(json_extract(r.result_json,'$.complete'),0)!=1 AND coalesce(json_extract(r.result_json,'$.status'),'')!='complete'",
            )?
            .query_map((task, attempt), |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut reverify = Vec::new();
        for (request, operation, raw) in receipts {
            // An unparsable receipt is never a known outcome.
            let Ok(receipt) = serde_json::from_str::<Value>(&raw) else {
                return refuse("lost_worker_action_outcome_unknown");
            };
            if action_outcome_known(&receipt) {
                continue;
            }
            // After a service restart the runtime settled what the gone session left
            // open for re-verification (`settled_by: runtime_reverify`): the same node
            // resumes and its fresh attempt re-verifies these before redoing anything.
            if class == "runtime_restart" && receipt["settled_by"] == "runtime_reverify" {
                let mut item = json!({"request":request,"operation":operation});
                for key in ["path", "argv", "cwd", "tool_call_ref"] {
                    if !receipt[key].is_null() {
                        item[key] = receipt[key].clone();
                    }
                }
                reverify.push(item);
                continue;
            }
            return refuse("lost_worker_action_outcome_unknown");
        }
        if !reverify.is_empty() {
            retry["reverify"] = json!(reverify);
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
/// The delay of the `n`-th (0-based) loss after a lineage's infrastructure budget
/// is spent: [`INFRASTRUCTURE_WAIT_FIRST`] doubling up to [`INFRASTRUCTURE_WAIT_MAX`].
pub fn infrastructure_wait(n: usize) -> f64 {
    (INFRASTRUCTURE_WAIT_FIRST * 2f64.powi(n.min(16) as i32)).min(INFRASTRUCTURE_WAIT_MAX)
}
/// The loop key of a repeated failure: `kind | node lineage | subject`.
pub fn loop_key(kind: &str, node: &str, subject: &str) -> String {
    crate::digest(format!("{kind}|{}|{subject}", node_base(node)).as_bytes())
}
/// `LOOP_BROKEN` for a lineage whose infrastructure budget is spent: the node keeps
/// waiting on the runtime's growing delay, without a rung, a replan or a model call
/// until the delay ends.
fn loop_broken_tx(
    db: &Transaction<'_>,
    task: &str,
    node: &str,
    class: &str,
    count: usize,
    delay: f64,
) -> CoreResult<()> {
    event_tx(
        db,
        task,
        "LOOP_BROKEN",
        json!({"key":loop_key("worker_lost",node,class),"kind":"worker_lost","node":node,"count":count,
            "action":"infrastructure_wait","limits":[],"failure_class":class,"retry_delay":delay}),
    )?;
    Ok(())
}
/// Record why an attempt of `assignment` ended without an accepted result, as the
/// `assignment_failed` event [`retry_lost_tx`] classifies its retry by: `cause` (what
/// the runtime observed), its failure `class` (see [`INFRASTRUCTURE_CLASSES`]; the
/// rest are quality classes) and the bounded `ending` facts (exit status or signal,
/// transport close, runtime cancel, stderr tail). Once per attempt; an attempt with
/// a recorded worker result records nothing.
pub fn record_attempt_ending_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: &str,
    attempt: &str,
    cause: &str,
    class: &str,
    ending: &Value,
) -> CoreResult<bool> {
    let published: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_events WHERE task_id=? AND event_type='worker_result'
         AND superseded=0 AND assignment_ref IN (?,?))",
        (task, assignment, attempt),
        |r| r.get(0),
    )?;
    if published {
        return Ok(false);
    }
    let payload = json!({"error":cause,"failure_class":class,"attempt":attempt,"ending":ending});
    let changed = db.execute(
        "INSERT OR IGNORE INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at)
         VALUES (?,'assignment_failed',?,?,?,'native-runtime',?)",
        (
            task,
            assignment,
            format!("attempt-ended:{attempt}"),
            payload.to_string(),
            crate::now(),
        ),
    )?;
    if changed == 1 {
        event_tx(
            db,
            task,
            "ATTEMPT_ENDED",
            json!({"assignment_ref":assignment,"attempt":attempt,"cause":cause,"failure_class":class,"ending":ending}),
        )?;
    }
    Ok(changed == 1)
}
/// An infrastructure pause of the task (`AUTOMATIC_PAUSED`, `pause_kind:
/// infrastructure`): no rung, no replan and no model call; the runtime rechecks it
/// itself and the no-progress notice tells the owner. `details` carries the reason,
/// the loop key and what repeated.
pub fn infrastructure_pause_tx(db: &Transaction<'_>, task: &str, details: Value) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    if state.terminal() || matches!(state, TaskState::Paused | TaskState::WaitingUser) {
        return Ok(());
    }
    let mut details = details;
    details["pause_kind"] = json!("infrastructure");
    details["revision"] = json!(revision);
    details["previous"] = json!(state);
    event_tx(db, task, "AUTOMATIC_PAUSED", details)?;
    domain::state_tx(
        db,
        task,
        TaskState::Recovering,
        Some("automatic_continuation_paused"),
    )?;
    Ok(())
}
