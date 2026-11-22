//! Runtime-owned continuation of every task (the no-stop policy). A stop of the
//! runtime's own making (a failed worker, a failed check, a review that made no
//! progress, an unknown effect outcome, a degenerate model turn, an exhausted step
//! window) never parks the task for the owner: the runtime records an
//! `AUTOMATIC_CONTINUATION` with the next rung of a strategy ladder and continues
//! with a fresh plan. Only a route the host cannot qualify waits (`external_blocker`),
//! and the runtime resumes it by itself. `AUTOMATIC_POLICY_BOUND` still marks a task
//! an Automatic sprint started (its finish and file-tool rules); the continuation
//! itself applies to every task. Events are additive in format 12.
use super::{domain, event_tx, obstacles, waits};
use crate::{CoreError, CoreResult, TaskState};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

pub const POLICY: &str = "automatic_continuation";
const PENDING: &str = "automatic_continuation_pending";
const PAUSED: &str = "automatic_continuation_paused";
/// The strategy ladder. Each stop fingerprint (reason, classification detail,
/// blocking items, failed checks; see `stop_tx`) takes the first rung it has not used
/// yet since the owner last changed the situation, skipping a rung that would repeat
/// the previous attempt's routes; once every rung was used the loop guard breaks the
/// loop (`LOOP_BROKEN`) instead of cycling:
/// - `repair`: a fresh plan that repairs the recorded failure on the current routes;
/// - `escalate_route`: workers run on escalated routes (a stronger tier, preferably
///   the other engine than the failed attempt) from here on;
/// - `consult`: the senior consultant is asked and its advice joins the next plan;
/// - `reassign`: the failed work goes to another role or a different decomposition,
///   and the coordinator itself moves to another qualified model when one exists.
///
/// Escalated routing applies to the revision after an escalating rung and to the work
/// that fell short (`node_escalated`).
pub const STRATEGIES: [&str; 4] = ["repair", "escalate_route", "consult", "reassign"];
/// Model-facing note for every coordinator session.
pub const COORDINATOR_NOTE: &str = "Runtime no-stop policy: the runtime never parks this task for runtime failures, retries, caps or unknown effect outcomes; it records them and continues the task with a changed strategy. Do not ask the owner how to handle runtime failures, repairs, retries, limits or approvals. Ask only for decisions about the work itself, and keep working on everything the answer does not block. When the runtime reports effects with an unknown outcome, re-verify them from the files, Git state and receipts before redoing them. Keep using the normal permission and access routes. Never waive required review or verification.";

/// The rung of the strategy ladder for continuation `count` (1-based) of a stop
/// without a fingerprint: the plain cycle over [`STRATEGIES`].
pub fn strategy_for(count: i64) -> &'static str {
    STRATEGIES[((count.max(1) - 1) % STRATEGIES.len() as i64) as usize]
}

/// The coordinator instruction for one rung of the ladder.
pub fn strategy_instruction(strategy: &str) -> &'static str {
    match strategy {
        "escalate_route" => "Change approach: the runtime now routes workers on escalated routes (a stronger model, preferably the other engine than the failed attempt). Plan the repair so that it does not repeat the failed steps unchanged.",
        "consult" => "Change approach: the runtime adds a senior consultant to the next plan. Follow its advice; do not repeat an approach that already failed.",
        "reassign" => "Change approach: give the failed work to a different role able to do it, or decompose it differently (smaller, independently checkable steps). Do not assign the same objective to the same role again.",
        "ask_owner" => "A decision only the owner can make blocks the work. Call user_question_request now with one question and concrete options; do not plan, retry or escalate the blocked work before the owner answers.",
        "retry_unblocked" => "The access or service that blocked this work is available again. Retry the blocked work with a fresh plan; do not change an approach that failed only because of it.",
        _ => "Repair the recorded failure with a fresh plan. Read the cited reports first and change what failed.",
    }
}

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

/// Continuations that used a rung of the ladder: a `technical` stop (or one recorded
/// before stops were classified). An `ask_owner` continuation uses none.
fn count(db: &Connection, task: &str) -> CoreResult<i64> {
    Ok(db.query_row(
        "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_CONTINUATION'
         AND coalesce(json_extract(payload_json,'$.class'),'technical')='technical'",
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
    /// An obstacle pause (`pause_kind: obstacle`): the ids of its owner actions and
    /// external waits that are still open. The runtime rechecks them itself and
    /// resumes only once this is empty; no model is called meanwhile.
    pub waiting: Vec<Value>,
}

/// The pause kind of a task parked behind access, an outage or both.
pub const OBSTACLE_PAUSE: &str = "obstacle";

/// Only our exact hold can be resumed. Owner pause, cancellation and real questions
/// have separate authority and are never overridden. A pause an earlier version
/// recorded for the owner (`continuation_cap`, `owner_blocker`, `outcome_unknown`)
/// is owed like any continuation: the runtime resumes it by itself.
pub fn owed(db: &Connection, task: &str) -> CoreResult<Option<Owed>> {
    let hold: Option<String> = db
        .query_row(
            "SELECT blocker FROM runtime_tasks WHERE task_id=? AND state='RECOVERING'",
            [task],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if !matches!(hold.as_deref(), Some(PENDING | PAUSED)) {
        // A loop question the owner answered: the runtime applies the answer itself.
        if let Some((event, mut evidence, answer)) = loop_answered(db, task)? {
            let waiting = apply_loop_answer(&mut evidence, answer);
            return Ok(Some(Owed {
                event,
                evidence,
                external: false,
                waiting,
            }));
        }
        return Ok(None);
    }
    let paused = hold.as_deref() == Some(PAUSED);
    let kind = if paused {
        "AUTOMATIC_PAUSED"
    } else {
        "AUTOMATIC_CONTINUATION"
    };
    let Some((event, mut evidence)) = latest(db, task, kind)? else {
        return Ok(None);
    };
    // A broken loop continues only after its wait (`not_before`): until then the
    // runtime waits without any model call.
    let backoff: Vec<Value> = evidence["not_before"]
        .as_i64()
        .filter(|t| *t > unix_now())
        .map(|t| json!({"backoff_until":t}))
        .into_iter()
        .collect();
    if paused && evidence["pause_kind"] == INFRASTRUCTURE_PAUSE {
        // An infrastructure loop the runtime waits out; it resumes on its own rung.
        return Ok(Some(Owed {
            event,
            evidence,
            external: false,
            waiting: backoff,
        }));
    }
    if paused && evidence["pause_kind"] == OBSTACLE_PAUSE {
        let ids = evidence["obstacles"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let waiting = super::obstacles::still_open(db, task, &ids)?;
        return Ok(Some(Owed {
            event,
            evidence,
            external: false,
            waiting,
        }));
    }
    let external = paused && evidence["pause_kind"] == "external_blocker";
    if paused && !external {
        // A retired owner pause continues on the escalated part of the ladder.
        let strategy = evidence["strategy"]
            .as_str()
            .unwrap_or("escalate_route")
            .to_owned();
        evidence["strategy"] = json!(strategy);
        evidence["instruction"] = json!(strategy_instruction(&strategy));
    }
    Ok(Some(Owed {
        event,
        evidence,
        external,
        waiting: if external { Vec::new() } else { backoff },
    }))
}

/// Whether the strategy ladder reached escalated routing for this task since it was
/// last reopened (sticky: later rungs keep escalated routes).
pub fn escalated(db: &Connection, task: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1
            AND (type='AUTOMATIC_CONTINUATION' AND json_extract(payload_json,'$.strategy') IN ('escalate_route','consult','reassign')
                 OR type='AUTOMATIC_PAUSED' AND json_extract(payload_json,'$.pause_kind') IN ('continuation_cap','owner_blocker'))
            AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0))",
        [task],
        |r| r.get(0),
    )?)
}

/// Whether a worker dispatch of `node` in plan `revision` routes escalated. Only the
/// work that fell short escalates, never the whole task (owner 2026-11-21: once the
/// ladder escalated, every later worker of a long task ran on the senior models):
/// - this node (or, for a review fix `_fix_<n>_<node>`, the node it fixes) failed
///   its own work since the task was last reopened; a failure that names another
///   role's work, an owner decision or missing access is not the model's, nor is one
///   the infrastructure caused (an infrastructure failure class, or a sandbox that
///   could not be prepared: [`infrastructure_failure_sql`]);
/// - a quality retry of this node in this revision (not a retry of an
///   infrastructure class, [`super::loop_guard::INFRASTRUCTURE_CLASSES`], nor a
///   route, authorization, owner-wait or obstacle retry);
/// - the fix of a second or later review cycle (the previous fix was rejected);
/// - the plan revision accepted right after a ladder rung that escalates routes.
/// A retry after an infrastructure failure (a lost session, a restart) is not one.
pub fn node_escalated(db: &Connection, task: &str, node: &str, revision: i64) -> CoreResult<bool> {
    let (cycle, base) = match node.strip_prefix("_fix_").and_then(|r| r.split_once('_')) {
        Some((n, base)) => (n.parse::<i64>().unwrap_or(1), base),
        None => (0, node),
    };
    if cycle >= 2 {
        return Ok(true);
    }
    let failed: bool = db.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM runtime_events f JOIN runtime_events c
            ON c.task_id=f.task_id AND c.type='ASSIGNMENT_CREATED'
           AND json_extract(c.payload_json,'$.assignment_ref')=json_extract(f.payload_json,'$.assignment_ref')
         WHERE f.task_id=?1 AND f.type='WORKER_FAILED'
           AND json_extract(c.payload_json,'$.node') IN (?2,?3)
           AND coalesce(json_extract(f.payload_json,'$.result.blocker.kind'),'') NOT IN ('other_role_work','owner_decision','missing_access')
           AND NOT {infra_failure}
           AND f.id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0))",
            infra_failure = infrastructure_failure_sql()
        ),
        (task, node, base),
        |r| r.get(0),
    )?;
    if failed {
        return Ok(true);
    }
    // A quality retry of this node in this revision (any retry that is not an
    // infrastructure, route, authorization, owner-wait or obstacle retry).
    let retried: bool = db.query_row(
        &format!("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND type='ASSIGNMENT_RETRIED'
           AND json_extract(payload_json,'$.node') IN (?2,?3) AND json_extract(payload_json,'$.revision')=?4
           AND coalesce(json_extract(payload_json,'$.failure_class'),'') NOT IN ({infra})
           AND coalesce(json_extract(payload_json,'$.route_failure'),0)!=1
           AND coalesce(json_extract(payload_json,'$.owner_wait_continuation'),0)!=1
           AND coalesce(json_extract(payload_json,'$.obstacle_retry'),0)!=1)",
            infra = super::loop_guard::infrastructure_sql()
        ),
        (task, node, base, revision),
        |r| r.get(0),
    )?;
    if retried {
        return Ok(true);
    }
    // The first plan revision whose execution began after the latest escalating rung.
    let after_rung: Option<i64> = db
        .query_row(
            "SELECT json_extract(b.payload_json,'$.revision') FROM runtime_events b
             WHERE b.task_id=?1 AND b.type='EXECUTION_BASELINE' AND b.id>(
               SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='AUTOMATIC_CONTINUATION'
                 AND json_extract(payload_json,'$.strategy') IN ('escalate_route','consult','reassign')
                 AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0))
             ORDER BY b.id LIMIT 1",
            [task],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(after_rung == Some(revision))
}

/// The engine an escalated dispatch of `node` (or, failing that, of the same `role`)
/// should move away from: the newest recorded route of that node or role in any plan
/// revision. A fresh plan reuses node ids or roles, so the failed attempt's engine is
/// known even when this revision has no earlier dispatch.
pub fn escalation_engine(
    db: &Connection,
    task: &str,
    node: &str,
    role: &str,
) -> CoreResult<Option<String>> {
    Ok(db
        .query_row(
            "SELECT json_extract(payload_json,'$.engine') FROM runtime_events WHERE task_id=? AND type='NATIVE_ROUTE_SELECTED'
             AND (json_extract(payload_json,'$.node')=? OR json_extract(payload_json,'$.decision.role')=?)
             ORDER BY id DESC LIMIT 1",
            (task, node, role),
            |r| r.get(0),
        )
        .optional()?
        .flatten())
}

pub fn held(db: &Connection, task: &str) -> CoreResult<bool> {
    let held: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_tasks WHERE task_id=? AND state='RECOVERING' AND blocker IN (?,?))",
        (task, PENDING, PAUSED), |r| r.get(0),
    )?;
    // A continuation whose loop question the owner answered is still the runtime's.
    Ok(held || loop_answered(db, task)?.is_some())
}

pub fn projection(db: &Connection, task: &str) -> CoreResult<Option<Value>> {
    if !enabled(db, task)? && count(db, task)? == 0 && !held(db, task)? {
        return Ok(None);
    }
    let last = latest(db, task, "AUTOMATIC_CONTINUATION")?;
    let strategy = last.as_ref().map(|(_, v)| v["strategy"].clone());
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
        "strategy":strategy,
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
        "proposal":{"question":"What should the coordinator change so that the task finishes, given the recorded failures and the approaches already tried, without waiving review, checks or permissions?",
            "reason":format!("Automatic continuation {}: {} (runtime event {stop}; evidence {})",
                request["count"].as_i64().unwrap_or(evidence["count"].as_i64().unwrap_or(0)),
                request["reason"], evidence["events"]), "report_ids":reports}}),
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
            WHERE e.task_id=?1 AND e.type='VERIFICATION_FAILED'
              AND NOT EXISTS (SELECT 1 FROM json_each(?2) p WHERE p.value=c.value)
              AND NOT EXISTS (SELECT 1 FROM json_each(?3) a WHERE a.value=c.value)
        )",
        (
            task,
            proposed.to_string(),
            json!(owner_accepted_commands(db, task)?).to_string(),
        ),
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

/// Dispatched effects of this task whose outcome is not known (no complete or settled
/// receipt), as the coordinator's uncertainty record. Bounded to 32 entries.
pub fn unknown_effects(db: &Connection, task: &str) -> CoreResult<Vec<Value>> {
    let rows = db
        .prepare(
            "SELECT request_key,operation,result_json FROM runtime_requests WHERE task_id=?
            AND operation IN ('outside_command','outside_write','command_run','project_write','project_edit',
                'project_mkdir','project_delete','project_skill_link','draft_write','external_call')
            AND json_extract(result_json,'$.complete') IS NOT 1
            AND json_extract(result_json,'$.status') IS NOT 'complete'
            AND json_extract(result_json,'$.settled') IS NOT 1 ORDER BY rowid LIMIT 32",
        )?
        .query_map([task], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .map(|(request, operation, raw)| {
            let receipt: Value = raw
                .as_deref()
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or(Value::Null);
            let mut item = json!({"request":request,"operation":operation});
            for key in ["path", "argv", "cwd", "tool_call_ref"] {
                if !receipt[key].is_null() {
                    item[key] = receipt[key].clone();
                }
            }
            item
        })
        .collect())
}

const COMMAND_OPERATIONS: [&str; 2] = ["outside_command", "command_run"];

/// Before new work runs, the runtime settles what it can about effects with an
/// unknown outcome instead of holding the task for the owner. A write, edit, call or
/// other synchronous effect of a session that is now confirmed gone cannot still be
/// running: its receipt is settled with outcome `unknown`
/// (`settled_by: runtime_reverify`) and listed for the coordinator, which re-verifies
/// it from the files, Git state and receipts before redoing anything. A command whose
/// receipt is still unsettled may still run: the caller waits for the host to settle
/// it with process evidence (`native_command_outcome_pending`), never the owner.
/// Returns the uncertainty record (empty when every effect is known).
pub fn settle_unknown_effects_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Vec<Value>> {
    let unknown = unknown_effects(db, task)?;
    if unknown
        .iter()
        .any(|e| COMMAND_OPERATIONS.contains(&e["operation"].as_str().unwrap_or("")))
    {
        return Err(CoreError::Refused("native_command_outcome_pending"));
    }
    for effect in &unknown {
        let raw: Option<String> = db.query_row(
            "SELECT result_json FROM runtime_requests WHERE task_id=? AND request_key=? AND operation=?",
            (task, effect["request"].as_str(), effect["operation"].as_str()),
            |r| r.get(0),
        )?;
        let mut receipt: Value = raw
            .as_deref()
            .and_then(|s| serde_json::from_str(s).ok())
            .filter(Value::is_object)
            .unwrap_or_else(|| json!({}));
        receipt["settled"] = json!(true);
        receipt["outcome"] = json!("unknown");
        receipt["settled_by"] = json!("runtime_reverify");
        receipt["settledAt"] = json!(crate::now());
        db.execute(
            "UPDATE runtime_requests SET result_json=? WHERE task_id=? AND request_key=? AND operation=?",
            (
                receipt.to_string(),
                task,
                effect["request"].as_str(),
                effect["operation"].as_str(),
            ),
        )?;
    }
    if !unknown.is_empty() {
        event_tx(
            db,
            task,
            "RUNTIME_UNCERTAINTY_RECORDED",
            json!({"effects":unknown,"settled_by":"runtime_reverify",
                "instruction":"These effects may or may not have taken place. Re-verify each one from the files, Git state and receipts before redoing it; never replay it blindly."}),
        )?;
    }
    Ok(unknown)
}

fn evidence(db: &Connection, task: &str, revision: i64, reason: &str) -> CoreResult<Value> {
    let events = db.prepare(
        "SELECT id,type FROM runtime_events WHERE task_id=?1 AND type IN (
          'VERIFICATION_FAILED','NATIVE_DRIVER_STOPPED','NATIVE_SESSION_START_FAILED',
          'NATIVE_WORKER_ROUTE_BLOCKED','REVIEW_RERUN','ASSIGNMENT_RETRIED',
          'NATIVE_RECOVERY_REQUIRED','REPLAN_REQUEST_ACCEPTED','CONSULTATION_REQUIRED',
          'WORKER_FAILED','NATIVE_PROCESS_SETTLED','NATIVE_PROCESS_UNCONFIRMED','WORKER_STOPPED_BY_OWNER',
          'REVIEW_DEAD_END_ROUTED','REVIEW_NO_PROGRESS','NATIVE_TURN_DEGENERATE')
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

/// An item the reviewer reported it cannot reach with its tools (`tool_unavailable`)
/// in at least two dead-end reviews since the owner last renewed the budget, steered
/// the task or reopened it, while the newest dead end still names it. A resumed
/// continuation does not reset the count: every repeat passes through one. It is
/// evidence only: the continuation then takes the `reassign` rung when that rung is
/// still unused for the stop. Items compare by their name before any ` (` or `: `
/// detail.
pub fn repeated_review_dead_end(db: &Connection, task: &str) -> CoreResult<Option<(String, i64)>> {
    let since: i64 = db.query_row(
        "SELECT coalesce(max(id),0) FROM runtime_events WHERE task_id=?
         AND type IN ('AUTOMATIC_CAP_RENEWED','USER_STEERING','TASK_REOPENED')",
        [task],
        |r| r.get(0),
    )?;
    let ends = db
        .prepare(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_DEAD_END_ROUTED'
             AND id>? AND json_valid(payload_json) ORDER BY id DESC LIMIT 64",
        )?
        .query_map((task, since), |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .map(|raw| serde_json::from_str::<Value>(raw).unwrap_or(Value::Null))
        .collect::<Vec<_>>();
    let unreachable = |event: &Value| -> Vec<String> {
        event["blocked"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["blocker"] == "tool_unavailable")
            .filter_map(|b| b["item"].as_str().map(dead_end_item))
            .filter(|item| !item.is_empty())
            .collect()
    };
    let Some(newest) = ends.first() else {
        return Ok(None);
    };
    for item in unreachable(newest) {
        let count = ends
            .iter()
            .filter(|e| {
                unreachable(e)
                    .iter()
                    .any(|i| i.to_lowercase() == item.to_lowercase())
            })
            .count() as i64;
        if count >= 2 {
            return Ok(Some((item, count)));
        }
    }
    Ok(None)
}
/// A dead-end item's name without the reviewer's detail: `change_set (host could not …)`
/// and `change_set: changed paths …` both name `change_set`.
fn dead_end_item(item: &str) -> String {
    let end = [item.find(" ("), item.find(": ")]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(item.len());
    item[..end].trim().chars().take(160).collect()
}

/// Sightings of one infrastructure interruption (same reason and class, no real
/// progress in between) after which the runtime stops relaunching at once and waits
/// without model calls (`AUTOMATIC_PAUSED {pause_kind: infrastructure}`).
const INFRA_LOOP: i64 = 3;
/// The pause kind of an infrastructure loop the runtime waits out by itself.
pub const INFRASTRUCTURE_PAUSE: &str = "infrastructure";
/// Model-facing note of a continuation that takes no rung: the attempt was
/// interrupted by the infrastructure, the host or the owner, not by the work.
const INFRA_NOTE: &str = "The previous attempt was interrupted by the infrastructure, the host or the owner (recorded below), not by the work. Continue on the same approach; do not change a plan only because of the interruption.";

fn unix_now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// The first event of the current attempt: the newest resumed continuation, accepted
/// plan, reopening or steering.
fn attempt_start(db: &Connection, task: &str) -> CoreResult<i64> {
    Ok(db.query_row(
        "SELECT coalesce(max(id),0) FROM runtime_events WHERE task_id=?
           AND type IN ('AUTOMATIC_RESUMED','PLAN_ACCEPTED','TASK_REOPENED','USER_STEERING')",
        [task],
        |r| r.get(0),
    )?)
}

/// SQL predicate on a `WORKER_FAILED` event `f`: the failure is the infrastructure's,
/// not the model's. Either it carries an infrastructure failure class, or the worker
/// named its tools unreachable (`tool_unavailable`) while the command sandbox could
/// not be prepared: its blocker names `sandbox_unavailable` or
/// `git_context_unavailable`, or a command receipt of its assignment records a
/// sandbox cause (`sandbox_unavailable:…`, `native_command_sandbox_unavailable`).
pub(crate) fn infrastructure_failure_sql() -> String {
    let infra = super::loop_guard::infrastructure_sql();
    format!(
        "(coalesce(json_extract(f.payload_json,'$.failure_class'),'') IN ({infra})
          OR json_extract(f.payload_json,'$.result.blocker.kind')='tool_unavailable' AND (
            lower(coalesce(json_extract(f.payload_json,'$.result.blocker.detail'),'')) LIKE '%sandbox_unavailable%'
            OR lower(coalesce(json_extract(f.payload_json,'$.result.blocker.detail'),'')) LIKE '%git_context_unavailable%'
            OR EXISTS(SELECT 1 FROM runtime_events s WHERE s.task_id=f.task_id AND s.type='NATIVE_COMMAND_FINISHED'
                 AND s.id<f.id
                 AND s.id>coalesce((SELECT max(c2.id) FROM runtime_events c2 WHERE c2.task_id=f.task_id
                     AND c2.type='ASSIGNMENT_CREATED' AND c2.id<f.id
                     AND json_extract(c2.payload_json,'$.assignment_ref')=json_extract(f.payload_json,'$.assignment_ref')),f.id)
                 AND (coalesce(json_extract(s.payload_json,'$.receipt.observation.cause'),'') LIKE 'sandbox_unavailable%'
                      OR json_extract(s.payload_json,'$.receipt.observation.failure')='native_command_sandbox_unavailable'))))"
    )
}

/// Stop reasons that are verdicts about the work (a check, a review, a worker's own
/// result, the coordinator's progress). An owner pause during the attempt does not
/// explain them.
fn work_verdict(reason: &str) -> bool {
    reason.starts_with("review_")
        || reason.starts_with("verification_")
        || reason.starts_with("completion_")
        || reason.starts_with("unresolved_")
        || matches!(
            reason,
            "worker_failed"
                | "coordinator_no_progress_limit"
                | "native_turn_degenerate"
                | obstacles::OWNER_DECISION_STOP
        )
}

/// The infrastructure class of a stop the model did not cause, if it is one
/// ([`super::loop_guard::INFRASTRUCTURE_CLASSES`]): an unknown or unconfirmed effect,
/// a service restart or interrupted turn, the owner's pause or stop, a session the
/// engine could not start, a retry limit reached only by infrastructure retries, a
/// worker failure the sandbox caused, or a verification whose every failing command
/// failed for its environment (no complete output, a timeout, exit 126 or 127). Such
/// a stop never takes a rung of the ladder.
fn infrastructure_stop(
    db: &Connection,
    task: &str,
    revision: i64,
    reason: &str,
) -> CoreResult<Option<&'static str>> {
    let attempt = attempt_start(db, task)?;
    let owner_paused: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='OWNER_TASK_PAUSED' AND id>?)",
        (task, attempt),
        |r| r.get(0),
    )?;
    let interrupted = if owner_paused {
        "owner_paused"
    } else {
        "runtime_restart"
    };
    if reason.ends_with("outcome_unknown")
        || reason.ends_with("unconfirmed")
        || matches!(
            reason,
            "native_process_exit_settled"
                | "native_service_interrupted"
                | "native_recovery_failed"
                | "storage_unavailable"
                | "native_host_closed"
        )
    {
        return Ok(Some(interrupted));
    }
    Ok(match reason {
        "worker_stopped_by_owner" => Some("owner_paused"),
        "supervisor_spawn_failed" | "native_session_start_failed" => Some("engine_failure"),
        "worker_retry_limit" => retry_limit_class(db, task, revision)?,
        "worker_failed" => {
            let sql = format!(
                "SELECT count(*),coalesce(sum(CASE WHEN {} THEN 1 ELSE 0 END),0) FROM runtime_events f
                 WHERE f.task_id=? AND f.type='WORKER_FAILED' AND f.id>?",
                infrastructure_failure_sql()
            );
            let (all, infra): (i64, i64) =
                db.query_row(&sql, (task, attempt), |r| Ok((r.get(0)?, r.get(1)?)))?;
            (all > 0 && all == infra).then_some("sandbox_unavailable")
        }
        "verification_failed" => verifier_environment(db, task)?.then_some("verifier_environment"),
        other if owner_paused && !work_verdict(other) => Some("owner_paused"),
        _ => None,
    })
}

/// A retry limit reached only by infrastructure or route retries in this revision (no
/// node had two quality retries): the class of the newest retry.
fn retry_limit_class(
    db: &Connection,
    task: &str,
    revision: i64,
) -> CoreResult<Option<&'static str>> {
    let infra = super::loop_guard::infrastructure_sql();
    let quality: bool = db.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED'
               AND json_extract(payload_json,'$.revision')=?
               AND coalesce(json_extract(payload_json,'$.failure_class'),'') NOT IN ({infra})
               AND coalesce(json_extract(payload_json,'$.route_failure'),0)!=1
               AND coalesce(json_extract(payload_json,'$.owner_wait_continuation'),0)!=1
               AND coalesce(json_extract(payload_json,'$.obstacle_retry'),0)!=1
             GROUP BY json_extract(payload_json,'$.node') HAVING count(*)>=2)"
        ),
        (task, revision),
        |r| r.get(0),
    )?;
    if quality {
        return Ok(None);
    }
    let newest: Option<(Option<String>, bool)> = db
        .query_row(
            "SELECT json_extract(payload_json,'$.failure_class'),coalesce(json_extract(payload_json,'$.route_failure'),0)=1
             FROM runtime_events WHERE task_id=? AND type='ASSIGNMENT_RETRIED' AND json_extract(payload_json,'$.revision')=?
             ORDER BY id DESC LIMIT 1",
            (task, revision),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(match newest {
        Some((_, true)) => Some("capacity_unavailable"),
        Some((Some(class), false)) => super::loop_guard::INFRASTRUCTURE_CLASSES
            .iter()
            .find(|c| **c == class)
            .copied(),
        _ => None,
    })
}

/// The newest failed verification failed only for its environment: every failing
/// command has no complete output (a timeout, output not captured), or exited 126 or
/// 127 (not executable, not found on the verifier's path).
fn verifier_environment(db: &Connection, task: &str) -> CoreResult<bool> {
    let Some((_, failed)) = latest(db, task, "VERIFICATION_FAILED")? else {
        return Ok(false);
    };
    let failing: Vec<&Value> = failed["checks"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| !(c["complete"] == true && c["exit_code"] == 0))
        .collect();
    Ok(!failing.is_empty()
        && failing.iter().all(|c| {
            c["complete"] != true
                || matches!(c["exit_code"].as_i64(), Some(126 | 127))
                || c["failure"]
                    .as_str()
                    .is_some_and(|f| f.contains("timeout") || f.contains("timed_out"))
        }))
}

/// The newest review dead end of `revision` (or, failing that, of the attempt).
fn dead_end_event(db: &Connection, task: &str, revision: i64) -> CoreResult<Option<Value>> {
    let raw: Option<String> = db
        .query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=?1 AND type='REVIEW_DEAD_END_ROUTED'
               AND (json_extract(payload_json,'$.revision')=?2 OR id>?3)
             ORDER BY (json_extract(payload_json,'$.revision')=?2) DESC, id DESC LIMIT 1",
            (task, revision, attempt_start(db, task)?),
            |r| r.get(0),
        )
        .optional()?;
    Ok(raw.and_then(|r| serde_json::from_str(&r).ok()))
}

fn blocked_items(event: &Value) -> Vec<Value> {
    event["blocked"].as_array().cloned().unwrap_or_default()
}

/// The fingerprint of a stop: its reason, its classification detail, the items that
/// block it (review dead ends by item name), the failed checks (argv and exit code)
/// and the failed workers (node and blocker kind), normalized by the loop guard. The
/// checkout digest and the revision are left out: they change on every attempt.
/// Returns the failed node (diagnostic), the subject and the normalized items.
fn stop_fingerprint(
    db: &Connection,
    task: &str,
    revision: i64,
    reason: &str,
    detail: &str,
) -> CoreResult<(String, String, Vec<String>)> {
    let mut parts = vec![reason.to_owned()];
    if !detail.is_empty() && detail != reason {
        parts.push(detail.to_owned());
    }
    let mut node = String::new();
    let mut items = Vec::new();
    match reason {
        "review_incomplete" => {
            if let Some(event) = dead_end_event(db, task, revision)? {
                let names: Vec<String> = blocked_items(&event)
                    .iter()
                    .filter_map(|b| b["item"].as_str().map(dead_end_item))
                    .collect();
                items =
                    super::loop_guard::items(&names.iter().map(String::as_str).collect::<Vec<_>>());
                node = "review".into();
            }
        }
        "verification_failed" => {
            if let Some((_, failed)) = latest(db, task, "VERIFICATION_FAILED")? {
                let mut checks: Vec<String> = failed["checks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|c| !(c["complete"] == true && c["exit_code"] == 0))
                    .map(|c| format!("{} => {}", c["argv"], c["exit_code"]))
                    .collect();
                checks.sort();
                items = checks;
            }
        }
        "worker_failed" => {
            let rows = db
                .prepare(
                    "SELECT coalesce(n.node_id,''),coalesce(json_extract(f.payload_json,'$.result.blocker.kind'),'')
                     FROM runtime_events f LEFT JOIN runtime_nodes n
                       ON n.task_id=f.task_id AND n.assignment_ref=json_extract(f.payload_json,'$.assignment_ref')
                     WHERE f.task_id=? AND f.type='WORKER_FAILED' AND f.id>? ORDER BY f.id",
                )?
                .query_map((task, attempt_start(db, task)?), |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut failed: Vec<String> = rows
                .iter()
                .map(|(n, k)| format!("{}:{k}", super::loop_guard::node_base(n)))
                .collect();
            failed.sort();
            failed.dedup();
            node = rows.first().map(|(n, _)| n.clone()).unwrap_or_default();
            items = failed;
        }
        r if r.starts_with("review_") => node = "review".into(),
        _ => {}
    }
    if !items.is_empty() {
        parts.push(items.join(", "));
    }
    Ok((node, parts.join(" | "), items))
}

/// The routes of the stopped revision cannot change on `escalate_route`: every worker
/// of the revision already ran escalated, or had a single candidate route.
fn routes_unchanged(db: &Connection, task: &str, revision: i64) -> CoreResult<bool> {
    let (all, fixed): (i64, i64) = db.query_row(
        "SELECT count(*),coalesce(sum(CASE WHEN json_extract(payload_json,'$.decision.escalated')=1
                 OR coalesce(json_array_length(payload_json,'$.decision.candidates'),0)<=1 THEN 1 ELSE 0 END),0)
         FROM runtime_events WHERE task_id=? AND type='NATIVE_ROUTE_SELECTED' AND json_extract(payload_json,'$.revision')=?",
        (task, revision),
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    Ok(all > 0 && all == fixed)
}

/// The rung of the newest continuation that took one, since the task was reopened
/// (`repair` when none did): an interrupted attempt resumes on it.
fn current_rung(db: &Connection, task: &str) -> CoreResult<&'static str> {
    let strategy: Option<String> = db
        .query_row(
            "SELECT json_extract(payload_json,'$.strategy') FROM runtime_events WHERE task_id=?1
               AND type='AUTOMATIC_CONTINUATION' AND json_extract(payload_json,'$.strategy') IN ('repair','escalate_route','consult','reassign')
               AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0)
             ORDER BY id DESC LIMIT 1",
            [task],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(STRATEGIES
        .iter()
        .find(|s| Some(**s) == strategy.as_deref())
        .copied()
        .unwrap_or("repair"))
}

fn loop_json(history: &super::loop_guard::History, kind: &str) -> Value {
    json!({"key":history.key,"kind":kind,"count":history.count,"rungs_used":history.rungs_used,
        "breaks":history.breaks,"history":history.events})
}

/// A completed INCOMPLETE review of `revision` (not a sprint plan review) on a task in
/// REVIEWING: its dead ends can be disclosed as limits. Returns the review node.
fn disclosable_review(
    db: &Connection,
    task: &str,
    revision: i64,
) -> CoreResult<Option<(String, Value)>> {
    let state: String = db.query_row(
        "SELECT state FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    if state != TaskState::Reviewing.as_str() {
        return Ok(None);
    }
    let row: Option<(String, String, Option<String>)> = db
        .query_row(
            "SELECT node_id,status,result_json FROM runtime_nodes
             WHERE task_id=? AND revision=? AND stage='review' ORDER BY rowid DESC LIMIT 1",
            (task, revision),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((node, status, Some(raw))) = row else {
        return Ok(None);
    };
    let result: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    if status != "completed"
        || result["verdict"] != "INCOMPLETE"
        || super::sprint_planning::is_plan_review(&node)
    {
        return Ok(None);
    }
    Ok(Some((node, result)))
}

/// Disclose a review's dead ends as `unverified` runtime limits on its result
/// (`review_limit_disclosed`, `REVIEW_LIMIT_DISCLOSED`) and continue to verification.
/// The review is not waived: everything it could read was reviewed, and the final
/// result must disclose every limit.
fn disclose_review_tx(
    db: &Transaction<'_>,
    task: &str,
    revision: i64,
    review: (String, Value),
    blocked: &[Value],
    key: &str,
    why: &str,
) -> CoreResult<Vec<Value>> {
    let (node, mut result) = review;
    let limits: Vec<Value> = blocked
        .iter()
        .map(|b| {
            let item: String = b["item"].as_str().unwrap_or("an item").chars().take(200).collect();
            let reason: String = b["reason"].as_str().unwrap_or("").chars().take(400).collect();
            let detail = if reason.is_empty() { String::new() } else { format!(" ({reason})") };
            let blocker = match b["blocker"].as_str() {
                Some("owner_decision") => format!(
                    "The owner already decided that {item} is not reviewed{detail}; it is not covered by the review."
                ),
                Some("tool_unavailable") => format!(
                    "The reviewer's tools could not reach {item}{detail}; it is not covered by the review."
                ),
                _ => format!(
                    "The review could not cover {item}{detail}, and every strategy of the runtime's ladder met the same dead end; it is not covered by the review."
                ),
            };
            json!({"name":format!("Review did not cover {item}"),"outcome":"unverified",
                "blocker":blocker,"source":"runtime"})
        })
        .collect();
    match result["checks"].as_array_mut() {
        Some(checks) => checks.extend(limits.iter().cloned()),
        None => result["checks"] = json!(limits),
    }
    result["review_limit_disclosed"] = json!(true);
    db.execute(
        "UPDATE runtime_nodes SET result_json=? WHERE task_id=? AND revision=? AND node_id=?",
        (result.to_string(), task, revision, &node),
    )?;
    event_tx(
        db,
        task,
        "REVIEW_LIMIT_DISCLOSED",
        json!({"review":node,"revision":revision,"report_ref":result["report_ref"],"limits":limits,
            "loop_key":key,"reason":why}),
    )?;
    domain::state_tx(db, task, TaskState::Verifying, None)?;
    Ok(limits)
}

/// Records a stop as a continuation on the strategy ladder and fences the old
/// revision. It never parks the task for the owner. Only a route the host cannot
/// qualify waits (`external_blocker`), and the runtime resumes it by itself.
///
/// The ladder is chosen per stop fingerprint ([`stop_fingerprint`], loop guard kind
/// `stop`): the rung is the first one this fingerprint has not used since the owner
/// last changed the situation (a rung whose routes would equal the previous
/// attempt's is skipped). Once every rung was used for one fingerprint the loop is
/// broken (`LOOP_BROKEN`) instead of cycling: review dead ends are disclosed as
/// `unverified` limits and the task continues to verification; anything else that
/// cannot be disclosed (a failing check, repeated findings) is put to the owner as
/// one runtime question without any model call ([`loop_question_tx`]). An infrastructure, host or owner interruption
/// ([`infrastructure_stop`]) takes no rung: it resumes on the current rung, and when
/// the same interruption repeats without progress the runtime waits it out
/// (`pause_kind: infrastructure`). An owner decision the owner already answered for
/// the same items is not asked again: its items are disclosed instead.
///
/// It does not prove that a child exited: resuming still requires every owned
/// process to be gone, and effects with an unknown outcome are re-verified by the
/// next coordinator ([`settle_unknown_effects_tx`]), never replayed blindly.
pub(super) fn stop_tx(db: &Transaction<'_>, task: &str, reason: &str) -> CoreResult<TaskState> {
    use super::loop_guard::{self as guard, Verdict};
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
    let unknown = reason.ends_with("outcome_unknown") || reason.ends_with("unconfirmed");
    if unknown {
        // Recorded for the coordinator; the effects are settled for re-verification
        // when the continuation resumes, once every owned process is gone.
        details["uncertain"] = json!(true);
        details["unknown_effects"] = json!(unknown_effects(db, task)?);
    }
    let dead_end = if reason == "review_incomplete" {
        repeated_review_dead_end(db, task)?
    } else {
        None
    };
    if let Some((item, count)) = &dead_end {
        details["review_dead_end"] = json!({"item":item,"count":count});
    }
    // Why the attempt failed decides what happens next: missing access waits for the
    // owner's action, an outage for the runtime's own recheck, an owner decision for
    // the owner's answer, an interruption resumes on the same rung; only a failure of
    // the work takes the next rung of the ladder.
    let (found, node) = if route_blocked(reason) {
        (None, None)
    } else {
        let (found, node) = obstacles::classify_stop(db, task, revision, reason)?;
        (Some(found), node)
    };
    let class = found.as_ref().map(|f| f.class);
    let infra = if class == Some(obstacles::TECHNICAL) {
        infrastructure_stop(db, task, revision, reason)?
    } else {
        None
    };
    let drift_assignment: Option<String> =
        match (found.as_ref().map(|f| f.rule), node.as_deref()) {
            (Some("checkout_drift"), Some(review)) => db
                .query_row(
                    "SELECT assignment_ref FROM runtime_nodes WHERE task_id=? AND revision=? AND node_id=?",
                    (task, revision, review),
                    |r| r.get(0),
                )
                .optional()?
                .flatten(),
            _ => None,
        };
    let classified = |obstacle: Option<&String>| -> Option<Value> {
        found.as_ref().map(|found| {
            let mut c = found.to_json();
            c["node"] = json!(node);
            c["revision"] = json!(revision);
            c["stop"] = json!(reason);
            c["obstacle"] = json!(obstacle);
            if let Some(infra) = infra {
                c["infrastructure"] = json!(infra);
            }
            // The failed review behind checkout drift is retried by its assignment
            // once the wait ends (`obstacles::retry_unblocked_tx`).
            if found.rule == "checkout_drift" {
                c["assignment_ref"] = json!(drift_assignment);
            }
            c
        })
    };
    // A failure of the work: one sighting of its fingerprint, and the rung (or the
    // loop break) it takes.
    let mut skip: Vec<&str> = Vec::new();
    let mut ladder = None;
    if class == Some(obstacles::TECHNICAL) && infra.is_none() {
        let detail = found.as_ref().map(|f| f.detail.as_str()).unwrap_or("");
        let (failed, subject, items) = stop_fingerprint(db, task, revision, reason, detail)?;
        if routes_unchanged(db, task, revision)? {
            skip.push("escalate_route");
        }
        // A degenerate turn never runs on the same model again.
        if reason == "native_turn_degenerate" {
            skip.push("repair");
        }
        let review = if reason == "review_incomplete" {
            disclosable_review(db, task, revision)?
        } else {
            None
        };
        // A review whose dead ends are only items the owner already decided about and
        // items its tools cannot reach is disclosed at once: the owner is never asked
        // the same question twice, and no strategy reaches those items.
        let answered = found
            .as_ref()
            .is_some_and(|f| f.rule == "owner_decision_answered")
            && review.is_some()
            && blocked_items(&dead_end_event(db, task, revision)?.unwrap_or(Value::Null))
                .iter()
                .all(|b| {
                    b["blocker"] == obstacles::OWNER_DECISION || b["blocker"] == "tool_unavailable"
                });
        // A dead end that repeats goes to another role first.
        let prefer = dead_end.is_some().then_some("reassign");
        let (history, verdict, _) = guard::observe_tx(
            db,
            task,
            "stop",
            "",
            &subject,
            json!({"stop":reason,"failed_node":failed,"items":items}),
            false,
            |h| match guard::next_rung(&STRATEGIES, h, &skip, prefer) {
                _ if answered => Verdict::Break { action: "disclose" },
                Some(rung) => Verdict::Continue { rung },
                None if review.is_some() => Verdict::Break { action: "disclose" },
                None => Verdict::Break {
                    action: "ask_owner",
                },
            },
        )?;
        if verdict == (Verdict::Break { action: "disclose" }) {
            if let (Some(review), Some(c)) = (review, classified(None)) {
                event_tx(db, task, "FAILURE_CLASSIFIED", c)?;
                let blocked =
                    blocked_items(&dead_end_event(db, task, revision)?.unwrap_or(Value::Null));
                let limits = disclose_review_tx(
                    db,
                    task,
                    revision,
                    review,
                    &blocked,
                    &history.key,
                    if answered {
                        "owner_decision_answered"
                    } else {
                        "ladder_exhausted"
                    },
                )?;
                guard::broken_tx(
                    db,
                    task,
                    &history,
                    "stop",
                    "disclose",
                    json!(limits),
                    json!({"stop":reason,"items":items}),
                )?;
                return Ok(TaskState::Verifying);
            }
        }
        ladder = Some((history, verdict, items));
    }
    let obstacle = match &found {
        Some(found) => obstacles::open_obstacle_tx(db, task, found, node.as_deref(), revision)?,
        None => None,
    };
    if let Some(c) = classified(obstacle.as_ref()) {
        details["class"] = json!(c["class"]);
        event_tx(db, task, "FAILURE_CLASSIFIED", c.clone())?;
        if class != Some(obstacles::TECHNICAL) {
            details["classification"] = c;
        }
    }
    waits::cancel_tx(db, task, None, "automatic_stop", "cancelled")?;
    domain::cancel_nodes_tx(db, task, "automatic_stop")?;
    domain::revoke_tx(db, task, None)?;
    if route_blocked(reason) {
        details["pause_kind"] = json!("external_blocker");
        // The resumed plan routes away from the blocked route.
        details["strategy"] = json!("escalate_route");
        event_tx(db, task, "AUTOMATIC_PAUSED", details)?;
        domain::state_tx(db, task, TaskState::Recovering, Some(PAUSED))?;
        return Ok(TaskState::Recovering);
    }
    match class {
        Some(obstacles::ACCESS | obstacles::EXTERNAL_OUTAGE) => {
            // No rung is used: the task waits, without model calls, until the owner's
            // action is resolved or the host answers, then retries the blocked work.
            details["pause_kind"] = json!(OBSTACLE_PAUSE);
            details["obstacles"] = json!(obstacle.into_iter().collect::<Vec<_>>());
            details["resume_state"] = json!("PLANNING");
            // A review that failed only because another actor kept changing the
            // checkout is run again by the runtime once the wait ends, without a
            // coordinator turn (`resume_tx`).
            if found.as_ref().is_some_and(|f| f.rule == "checkout_drift") {
                if let Some(review) = node.as_deref() {
                    details["resume_state"] = json!("EXECUTING");
                    details["resume_review"] = json!(review);
                }
            }
            details["strategy"] = json!("retry_unblocked");
            details["instruction"] = json!(strategy_instruction("retry_unblocked"));
            event_tx(db, task, "AUTOMATIC_PAUSED", details)?;
            domain::state_tx(db, task, TaskState::Recovering, Some(PAUSED))?;
            return Ok(TaskState::Recovering);
        }
        Some(obstacles::OWNER_DECISION) => {
            // No rung is used: the next coordinator turn asks the owner.
            let decisions = obstacles::open_decisions(db, task)?;
            let detail = found.as_ref().map(|f| f.detail.clone()).unwrap_or_default();
            details["strategy"] = json!("ask_owner");
            details["instruction"] = json!(obstacles::ask_owner_instruction(&detail));
            details["decisions"] = json!(decisions
                .iter()
                .map(|d| d["id"].clone())
                .collect::<Vec<_>>());
            details["switch_coordinator"] = json!(false);
            event_tx(db, task, "AUTOMATIC_CONTINUATION", details)?;
            domain::state_tx(db, task, TaskState::Recovering, Some(PENDING))?;
            return Ok(TaskState::Recovering);
        }
        _ => {}
    }
    if let Some(infra) = infra {
        // No rung is used: the attempt resumes on the rung it was on. The same
        // interruption repeating without progress is waited out without model calls.
        let rung = current_rung(db, task)?;
        let (history, verdict, _) = guard::observe_tx(
            db,
            task,
            "infrastructure",
            "",
            &format!("{reason} | {infra}"),
            json!({"stop":reason,"class":infra}),
            true,
            |h| {
                if h.count >= INFRA_LOOP {
                    Verdict::Break {
                        action: "infrastructure_wait",
                    }
                } else {
                    Verdict::Continue { rung: "resume" }
                }
            },
        )?;
        details["class"] = json!("infrastructure");
        details["infrastructure"] = json!(infra);
        details["count"] = json!(count);
        details["strategy"] = json!(rung);
        details["instruction"] = json!(format!("{INFRA_NOTE} {}", strategy_instruction(rung)));
        details["switch_coordinator"] = json!(false);
        details["loop_key"] = json!(history.key);
        details["loop"] = loop_json(&history, "infrastructure");
        if matches!(verdict, Verdict::Break { .. }) {
            let wait = guard::backoff_seconds(history.breaks + 1);
            details["pause_kind"] = json!(INFRASTRUCTURE_PAUSE);
            details["not_before"] = json!(unix_now() + wait);
            details["backoff_seconds"] = json!(wait);
            guard::broken_tx(
                db,
                task,
                &history,
                "infrastructure",
                "infrastructure_wait",
                json!([]),
                json!({"stop":reason,"class":infra,"backoff_seconds":wait}),
            )?;
            event_tx(db, task, "AUTOMATIC_PAUSED", details)?;
            domain::state_tx(db, task, TaskState::Recovering, Some(PAUSED))?;
            return Ok(TaskState::Recovering);
        }
        event_tx(db, task, "AUTOMATIC_CONTINUATION", details)?;
        domain::state_tx(db, task, TaskState::Recovering, Some(PENDING))?;
        return Ok(TaskState::Recovering);
    }
    let next = count + 1;
    let strategy = match &ladder {
        Some((_, Verdict::Continue { rung }, _)) => *rung,
        // Every rung was used and nothing can be disclosed: the owner decides; the
        // attempt after the answer starts a new round of the ladder.
        Some((_, Verdict::Break { .. }, _)) => "repair",
        None => strategy_for(next),
    };
    details["count"] = json!(next);
    details["strategy"] = json!(strategy);
    details["instruction"] = json!(strategy_instruction(strategy));
    details["switch_coordinator"] =
        json!(reason == "native_turn_degenerate" || strategy == "reassign");
    if let Some((history, verdict, items)) = &ladder {
        details["loop_key"] = json!(history.key);
        details["loop"] = loop_json(history, "stop");
        if matches!(verdict, Verdict::Break { .. }) {
            details["switch_coordinator"] = json!(false);
            return loop_question_tx(db, task, revision, reason, history, items, details);
        }
    }
    let id = event_tx(db, task, "AUTOMATIC_CONTINUATION", details)?;
    if strategy == "consult" {
        event_tx(
            db,
            task,
            "AUTOMATIC_CONSULTATION_REQUESTED",
            json!({
                "continuation_event":id, "reason":reason, "revision":revision, "count":next,
            }),
        )?;
    }
    domain::state_tx(db, task, TaskState::Recovering, Some(PENDING))?;
    Ok(TaskState::Recovering)
}

/// Fixed ids of the loop question's answers, in option order.
pub const LOOP_OPTIONS: [&str; 4] = [
    "retry_now",
    "other_approach",
    "accept_unverified",
    "retry_later",
];
const LOOP_OPTIONS_EN: [&str; 4] = [
    "Retry now: start a new round of every strategy",
    "Another approach: describe in your answer what to do differently",
    "Accept unverified: record this as not verified and finish the task",
    "Retry later: try again automatically in 30 minutes",
];
const LOOP_OPTIONS_RU: [&str; 4] = [
    "Повторить сейчас: начать новый круг всех стратегий",
    "Другой подход: опишите в ответе, что сделать иначе",
    "Принять без проверки: отметить как непроверенное и завершить задачу",
    "Повторить позже: автоматически через 30 минут",
];
/// The wait of a `retry_later` answer, without model calls.
pub const RETRY_LATER_SECONDS: i64 = 30 * 60;

/// The task's name for the owner: the first line of its description (or goal).
pub(crate) fn task_name(db: &Connection, task: &str) -> CoreResult<String> {
    let text: Option<String> = db
        .query_row(
            "SELECT coalesce(
                (SELECT t.description FROM workspace_tasks t JOIN workspace_executions e USING(task_id)
                 WHERE e.runtime_task_id=?1 LIMIT 1),
                (SELECT json_extract(payload_json,'$.goal') FROM runtime_events
                 WHERE task_id=?1 AND type='NATIVE_START_ADMITTED' ORDER BY id LIMIT 1))",
            [task],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let line = text
        .as_deref()
        .and_then(|t| t.lines().map(str::trim).find(|l| !l.is_empty()))
        .unwrap_or(task);
    let mut name: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        name.push('…');
    }
    Ok(name)
}

/// Every rung of the ladder was used for one stop fingerprint and the failure cannot
/// be disclosed: the runtime itself asks the owner one question (no model call),
/// in the task's language, naming the task, the repeated failure, and how many full
/// rounds of the ladder were tried, with four answers of fixed id
/// ([`LOOP_OPTIONS`]). The continuation is recorded (`owner_question`) and the task
/// waits in WAITING_USER; the runtime applies the answer itself ([`owed`],
/// [`resume_tx`]). The same fingerprint asks again only after the chosen answer was
/// tried and every rung was used again; that question names its attempt number.
fn loop_question_tx(
    db: &Transaction<'_>,
    task: &str,
    revision: i64,
    reason: &str,
    history: &super::loop_guard::History,
    items: &[String],
    details: Value,
) -> CoreResult<TaskState> {
    let attempt: i64 = db.query_row(
        "SELECT count(*)+1 FROM runtime_events WHERE task_id=?1 AND type='LOOP_BROKEN'
           AND json_extract(payload_json,'$.key')=?2 AND json_extract(payload_json,'$.action')='ask_owner'
           AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0)",
        (task, &history.key),
        |r| r.get(0),
    )?;
    let ru = domain::task_language_tx(db, task)? == "ru";
    let name = task_name(db, task)?;
    let failure = domain::owner_readable_blocker(reason, ru);
    let detail: String = items
        .iter()
        .take(2)
        .map(|i| i.chars().take(120).collect::<String>())
        .collect::<Vec<_>>()
        .join("; ");
    let detail = if detail.is_empty() {
        String::new()
    } else {
        format!(" ({detail})")
    };
    let text = if ru {
        format!(
            "{}Задача «{name}»: {failure}{detail} — повторяется без продвижения. Все 4 стратегии (исправление, более сильные модели, старший консультант, другая роль) испробованы полностью {attempt} раз(а). Как продолжить?",
            if attempt > 1 { format!("Попытка {attempt}. ") } else { String::new() }
        )
    } else {
        format!(
            "{}Task \"{name}\": {failure}{detail} keeps repeating without progress. All 4 strategies (repair, stronger models, senior consultant, another role) were tried in full {attempt} time(s). How should it continue?",
            if attempt > 1 { format!("Attempt {attempt}. ") } else { String::new() }
        )
    };
    runtime_question_tx(
        db, task, revision, reason, history, items, details, &text, attempt, ru,
    )
}

/// Records the runtime's own loop question (`text`, already in the task's language)
/// with the fixed answers of [`LOOP_OPTIONS`], the continuation that waits on it and
/// the task's move to WAITING_USER; the answer is applied like a loop question's.
/// Also used for a verification its environment keeps failing
/// (`completion::record_verification_wait_tx`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn runtime_question_tx(
    db: &Transaction<'_>,
    task: &str,
    revision: i64,
    reason: &str,
    history: &super::loop_guard::History,
    items: &[String],
    mut details: Value,
    text: &str,
    attempt: i64,
    ru: bool,
) -> CoreResult<TaskState> {
    let options = if ru { LOOP_OPTIONS_RU } else { LOOP_OPTIONS_EN };
    let question = format!("pq_{}", uuid::Uuid::new_v4().simple());
    db.execute(
        "INSERT INTO pending_questions(id,task_id,dedup_key,text,options_json,status,created_at)
         VALUES (?,?,?,?,?,'open',?)",
        (
            &question,
            task,
            format!("runtime:loop:{}:{attempt}", history.key),
            &text,
            json!(options).to_string(),
            crate::now(),
        ),
    )?;
    details["owner_question"] = json!(question);
    details["loop_attempt"] = json!(attempt);
    let id = event_tx(db, task, "AUTOMATIC_CONTINUATION", details)?;
    super::loop_guard::broken_tx(
        db,
        task,
        history,
        "stop",
        "ask_owner",
        json!([]),
        json!({"stop":reason,"question_id":question,"attempt":attempt,"items":items}),
    )?;
    event_tx(
        db,
        task,
        "USER_QUESTION_REQUESTED",
        json!({"question_id":question,"question":text,"options":options,"option_ids":LOOP_OPTIONS,
            "resume_state":"RECOVERING","revision":revision,"blocker":reason,
            "runtime_loop":{"key":history.key,"attempt":attempt,"continuation_event":id,"stop":reason}}),
    )?;
    domain::state_tx(db, task, TaskState::WaitingUser, Some(reason))?;
    Ok(TaskState::WaitingUser)
}

/// The answer to a loop question: its option id and the owner's own words. An answer
/// that names no option is the owner's direction (`other_approach`).
fn loop_choice(answer: &str) -> (&'static str, String) {
    let first = answer.lines().next().unwrap_or("").trim();
    let rest = answer
        .split_once("\n\n")
        .map(|(_, r)| r.trim().to_owned())
        .unwrap_or_default();
    for (i, id) in LOOP_OPTIONS.iter().enumerate() {
        let number = (i + 1).to_string();
        if first == LOOP_OPTIONS_EN[i]
            || first == LOOP_OPTIONS_RU[i]
            || first == *id
            || first == number
        {
            return (id, rest);
        }
    }
    ("other_approach", answer.trim().to_owned())
}

/// A continuation waiting on its loop question whose question is no longer open: the
/// continuation event, the question id and the answer (`None` when the question was
/// superseded or cancelled), with the time it was answered (unix seconds).
fn loop_answered(
    db: &Connection,
    task: &str,
) -> CoreResult<Option<(i64, Value, Option<(String, i64)>)>> {
    let open: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_tasks WHERE task_id=? AND state='RECOVERING'
            AND coalesce(blocker,'')='')",
        [task],
        |r| r.get(0),
    )?;
    if !open {
        return Ok(None);
    }
    let Some((event, evidence)) = latest(db, task, "AUTOMATIC_CONTINUATION")? else {
        return Ok(None);
    };
    let Some(question) = evidence["owner_question"].as_str() else {
        return Ok(None);
    };
    let resumed: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_RESUMED'
            AND json_extract(payload_json,'$.stop_event')=?)",
        (task, event),
        |r| r.get(0),
    )?;
    // Only the transition the answer made (WAITING_USER to RECOVERING) right after
    // this continuation: a task that moved on by another route is not claimed.
    let answered_now: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events c WHERE c.task_id=?1 AND c.type='TASK_STATE_CHANGED'
            AND c.id=(SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_STATE_CHANGED')
            AND c.id>?2
            AND json_extract(c.payload_json,'$.previous')='WAITING_USER'
            AND json_extract(c.payload_json,'$.state')='RECOVERING')",
        (task, event),
        |r| r.get(0),
    )?;
    if resumed || !answered_now {
        return Ok(None);
    }
    let row: Option<(String, Option<String>, Option<String>)> = db
        .query_row(
            "SELECT status,answer_text,answered_at FROM pending_questions WHERE task_id=? AND id=?",
            (task, question),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let answer = match row {
        Some((status, _, _)) if status == "open" => return Ok(None),
        Some((status, Some(text), at)) if status == "answered" => {
            let at = at
                .as_deref()
                .and_then(|a| chrono::DateTime::parse_from_rfc3339(a).ok())
                .map(|t| t.timestamp())
                .unwrap_or_else(unix_now);
            Some((text, at))
        }
        _ => None,
    };
    Ok(Some((event, evidence, answer)))
}

/// The continuation evidence once the owner answered the loop question: the answer
/// (`owner_choice`), the coordinator's instruction for it, and for `retry_later` the
/// wait before the runtime continues.
fn apply_loop_answer(evidence: &mut Value, answer: Option<(String, i64)>) -> Vec<Value> {
    let (choice, words, at) = match answer {
        Some((text, at)) => {
            let (choice, words) = loop_choice(&text);
            (choice, words, at)
        }
        // A question another authority replaced: continue as on `retry_now`.
        None => ("retry_now", String::new(), unix_now()),
    };
    evidence["owner_choice"] = json!(choice);
    let round = "The owner was asked after every strategy failed the same way";
    let instruction = match choice {
        "other_approach" if !words.is_empty() => {
            evidence["owner_direction"] = json!(words);
            format!("{round} and gave this direction for the next plan: \"{words}\". Plan the work along it; do not repeat the approaches that failed.")
        }
        "other_approach" => format!("{round} and asked for a different approach. Choose one that none of the failed attempts used."),
        "accept_unverified" => format!("{round} and accepted the failure as a disclosed unverified limit. Do not repair it again; finish the remaining work and submit the final result. The runtime lists the limit under Not verified; never report it as passed."),
        _ => format!("{round} and asked to try again: a new round of strategies starts. {}", strategy_instruction("repair")),
    };
    // A verification its environment kept failing: the change is to the verification
    // procedure (`verification_submit`), not to the reviewed work.
    let instruction = match evidence["verification_environment"]["argv"].as_array() {
        Some(_) if evidence["reason"] == "verifier_environment" => format!(
            "{instruction} The runtime's verification command {} kept failing in the verifier's environment ({}), not in the work. {}",
            evidence["verification_environment"]["argv"],
            evidence["verification_environment"]["reason"].as_str().unwrap_or("environment"),
            match choice {
                "accept_unverified" => "Submit the remaining verification with verification_submit, without that command (an empty list when the owner accepted every command of the plan).",
                "other_approach" => "Submit a verification procedure the verifier can run (for example without a service its sandbox cannot reach) with verification_submit.",
                _ => "Submit the same verification again with verification_submit.",
            }
        ),
        _ => instruction,
    };
    evidence["instruction"] = json!(instruction);
    if choice == "retry_later" {
        let until = at + RETRY_LATER_SECONDS;
        evidence["not_before"] = json!(until);
        if until > unix_now() {
            return vec![json!({"backoff_until":until})];
        }
    }
    Vec::new()
}

/// Check commands the owner accepted as disclosed unverified limits after a loop
/// question (`LOOP_BROKEN` action `accept_unverified`, `accepted_commands`), since the
/// task was last reopened. A repair plan may leave them out; they stay gaps of the
/// final result (`reconciliation::gaps` reads `LOOP_BROKEN.limits`).
pub fn owner_accepted_commands(db: &Connection, task: &str) -> CoreResult<Vec<Value>> {
    let rows = db
        .prepare(
            "SELECT payload_json FROM runtime_events WHERE task_id=?1 AND type='LOOP_BROKEN'
               AND json_extract(payload_json,'$.action')='accept_unverified'
               AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0)",
        )?
        .query_map([task], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for raw in rows {
        let v: Value = serde_json::from_str(&raw)?;
        out.extend(
            v["accepted_commands"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
    }
    Ok(out)
}

/// Record what the owner's loop answer does before the resumed attempt: an accepted
/// failure becomes a disclosed `unverified` limit (`LOOP_BROKEN` action
/// `accept_unverified`, never a pass); any other answer starts a new round of the
/// ladder whose first rung is the resumed attempt.
fn record_loop_answer_tx(db: &Transaction<'_>, task: &str, evidence: &Value) -> CoreResult<()> {
    let Some(choice) = evidence["owner_choice"].as_str() else {
        return Ok(());
    };
    let key = evidence["loop_key"].as_str().unwrap_or_default().to_owned();
    let history = super::loop_guard::history(db, task, &key, false)?;
    if choice == "accept_unverified" {
        let reason = evidence["reason"].as_str().unwrap_or("");
        let failure = domain::owner_readable_blocker(reason, false);
        let commands: Vec<Value> = if reason == "verification_failed" {
            latest(db, task, "VERIFICATION_FAILED")?
                .map(|(_, v)| {
                    v["checks"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|c| !(c["complete"] == true && c["exit_code"] == 0))
                        .map(|c| c["argv"].clone())
                        .collect()
                })
                .unwrap_or_default()
        } else if reason == "verifier_environment" {
            // The command the verifier's environment kept failing.
            evidence["verification_environment"]["argv"]
                .as_array()
                .map(|_| vec![evidence["verification_environment"]["argv"].clone()])
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut name = format!("Accepted without verification: {failure}");
        if let Some(argv) = commands.first() {
            name.push_str(&format!(" ({argv})"));
        }
        let limit = json!({"name":name.chars().take(200).collect::<String>(),"outcome":"unverified",
            "blocker":format!("The owner accepted this repeated failure without verification (question {}) after every strategy of the ladder failed the same way; it is not verified.",
                evidence["owner_question"].as_str().unwrap_or("")),
            "source":"runtime"});
        event_tx(
            db,
            task,
            super::loop_guard::BROKEN,
            json!({"key":key,"kind":"stop","action":"accept_unverified","limits":[limit],
                "accepted_commands":commands,"count":history.count,"rungs_used":history.rungs_used,
                "evidence":{"question_id":evidence["owner_question"],"stop":reason}}),
        )?;
    } else {
        event_tx(
            db,
            task,
            super::loop_guard::OBSERVED,
            json!({"key":key,"kind":"stop","node":"","count":history.count,"rungs_used":history.rungs_used,
                "rung":"repair","owner_choice":choice,"question_id":evidence["owner_question"]}),
        )?;
    }
    Ok(())
}

/// A crash or refusal whose effects could not be reconciled becomes a continuation
/// that carries the uncertainty (`uncertain`, `unknown_effects`), for every task.
/// Existing attempt and wait evidence is retained; nothing is replayed. A paused or
/// waiting task, or one with an open wait, keeps its own authority (`false`).
pub fn continue_unsettled_tx(db: &Transaction<'_>, task: &str, reason: &str) -> CoreResult<bool> {
    let (state, _) = domain::task_tx(db, task)?;
    if state.terminal()
        || matches!(state, TaskState::Paused | TaskState::WaitingUser)
        || waits::projection_tx(db, task)?
            .iter()
            .any(|w| matches!(w["status"].as_str(), Some("open" | "ready" | "resuming")))
    {
        return Ok(false);
    }
    if held(db, task)? {
        return Ok(true);
    }
    stop_tx(db, task, reason)?;
    Ok(held(db, task)?)
}

/// Stop reasons that name nothing but an owned process whose exit was not confirmed.
const EXIT_UNCONFIRMED: [&str; 2] = [
    "native_shutdown_unconfirmed",
    "lost_worker_termination_unconfirmed",
];

/// The host settled, with process evidence, the unconfirmed exits that held this task:
/// every owned process is now known to be gone ([`require_processes_gone`]); effects
/// with an unknown outcome are re-verified when the continuation resumes. A RECOVERING task stopped with no recorded reason,
/// with only such an exit as its reason, or paused (`outcome_unknown`) only for one
/// (including the restart's `native_session_outcome_unknown`, recorded only when its
/// processes were unsettled), is then an ordinary interrupted turn and continues like
/// any runtime stop (every task, no cap), never waiving review or verification. An owner
/// pause, a question or wait, or any other unknown outcome keeps its own authority.
/// Returns whether a continuation is now owed.
pub fn exit_settled_tx(db: &Transaction<'_>, task: &str) -> CoreResult<bool> {
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
        || require_processes_gone(db, task).is_err()
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

/// A pause an earlier version recorded for the owner (an exhausted budget or an owner
/// blocker): its owner Resume or Continue still records `AUTOMATIC_CAP_RENEWED`. The
/// runtime resumes such a pause by itself as well ([`owed`]).
pub(super) fn owner_cap(db: &Connection, task: &str) -> CoreResult<Option<i64>> {
    let cap = projection(db, task)?
        .filter(|value| {
            matches!(
                value["pause"]["kind"].as_str(),
                Some("continuation_cap" | "owner_blocker")
            )
        })
        .and_then(|value| value["pause"]["eventId"].as_str()?.parse().ok());
    Ok(cap)
}

/// The plan's remaining work waits only for open owner actions or external waits
/// (`parked`: node, class, obstacle id): the task pauses as the runtime's own wait
/// (`pause_kind: obstacle`, `resume_state: EXECUTING`) without cancelling the plan or
/// using a rung of the ladder. [`resume_tx`] continues the same plan once every
/// obstacle closed.
pub(super) fn obstacle_pause_tx(
    db: &Transaction<'_>,
    task: &str,
    parked: &[(String, String, String)],
) -> CoreResult<()> {
    let (_, revision) = domain::task_tx(db, task)?;
    let mut ids: Vec<&str> = parked.iter().map(|p| p.2.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    let details = json!({"reason":"obstacle_wait","revision":revision,"pause_kind":OBSTACLE_PAUSE,
        "resume_state":"EXECUTING","obstacles":ids,
        "nodes":parked.iter().map(|p| json!({"node":p.0,"class":p.1,"obstacle":p.2})).collect::<Vec<_>>()});
    event_tx(db, task, "AUTOMATIC_PAUSED", details)?;
    domain::state_tx(db, task, TaskState::Recovering, Some(PAUSED))
}

/// Called by the owning host only, after qualifying the current route. A fresh
/// event identity avoids consuming the wrong recovery after a concurrent control.
pub fn resume_tx(db: &Transaction<'_>, task: &str, event: i64) -> CoreResult<Option<Value>> {
    let Some(owed) = owed(db, task)?.filter(|o| o.event == event) else {
        return Ok(None);
    };
    // An obstacle pause resumes only once its owner actions and external waits closed.
    if !owed.waiting.is_empty() {
        return Ok(None);
    }
    require_processes_gone(db, task)?;
    let uncertainty = settle_unknown_effects_tx(db, task)?;
    let mut evidence = owed.evidence;
    record_loop_answer_tx(db, task, &evidence)?;
    // A plan parked behind an obstacle continues where it stopped: the parked nodes
    // are retried at once and the coordinator is not called. Effects with an unknown
    // outcome, or a plan that changed meanwhile, go back to the coordinator instead.
    let (_, revision) = domain::task_tx(db, task)?;
    if evidence["pause_kind"] == OBSTACLE_PAUSE
        && evidence["resume_state"] == "EXECUTING"
        && evidence["revision"] == revision
        && uncertainty.is_empty()
    {
        let retried = obstacles::retry_unblocked_tx(db, task)?;
        // A review parked behind checkout drift runs again in REVIEWING; when it could
        // not be retried, the coordinator plans instead of an empty review stage.
        let review = evidence["resume_review"].as_str().map(str::to_owned);
        match review {
            Some(review) if retried.contains(&review) => {
                domain::settle_recovery_tx(db, task, TaskState::Reviewing)?;
                evidence["resume"] = json!("execution");
            }
            Some(_) => {
                domain::settle_recovery_tx(db, task, TaskState::Planning)?;
                evidence["strategy"] = json!("retry_unblocked");
                evidence["instruction"] = json!(strategy_instruction("retry_unblocked"));
            }
            None => {
                domain::settle_recovery_tx(db, task, TaskState::Executing)?;
                evidence["resume"] = json!("execution");
            }
        }
        evidence["retried"] = json!(retried);
    } else {
        domain::settle_recovery_tx(db, task, TaskState::Planning)?;
        if evidence["pause_kind"] == OBSTACLE_PAUSE {
            evidence["strategy"] = json!("retry_unblocked");
            evidence["instruction"] = json!(strategy_instruction("retry_unblocked"));
        }
    }
    if !uncertainty.is_empty() {
        evidence["uncertainty"] = json!(uncertainty);
    }
    event_tx(
        db,
        task,
        "AUTOMATIC_RESUMED",
        json!({"stop_event":event,"evidence":evidence}),
    )?;
    Ok(Some(evidence))
}

/// Check the retained processes and dispatched effects before handing the folder to more work.
pub fn require_settled_execution(db: &Connection, task: &str) -> CoreResult<()> {
    require_processes_gone(db, task)?;
    require_known_effects(db, task)
}

/// Every owned engine session of the task is confirmed gone (the host settled it
/// with process evidence). Until then the runtime waits; it never asks the owner.
pub fn require_processes_gone(db: &Connection, task: &str) -> CoreResult<()> {
    let live: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND status!='terminated')
         OR EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND state!='released')",
        (task, task), |r| r.get(0),
    )?;
    if live {
        return Err(CoreError::Refused("lost_worker_termination_unconfirmed"));
    }
    Ok(())
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

/// Continuations of the same stop as the newest one (its loop key) since the owner
/// last changed the situation ([`super::loop_guard::window_start`]); an accepted plan
/// or a resumed continuation does not reset it, so a plan-then-fail cycle is spaced
/// out too. A continuation recorded before stops had keys counts as before: since
/// the task last made progress or the owner last spoke. The host spaces a
/// continuation by this count (no cap, only a growing delay), so an engine that keeps
/// failing at once is retried at a bounded rate instead of in a tight loop.
pub fn idle_continuations(db: &Connection, task: &str) -> CoreResult<i64> {
    let key: Option<String> = db
        .query_row(
            "SELECT json_extract(payload_json,'$.loop_key') FROM runtime_events
             WHERE task_id=? AND type='AUTOMATIC_CONTINUATION' ORDER BY id DESC LIMIT 1",
            [task],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if let Some(key) = key {
        let since = super::loop_guard::window_start(db, task, &key, false)?;
        return Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_CONTINUATION'
               AND json_extract(payload_json,'$.loop_key')=? AND id>?",
            (task, &key, since),
            |r| r.get(0),
        )?);
    }
    Ok(db.query_row(
        "SELECT count(*) FROM runtime_events WHERE task_id=?1 AND type='AUTOMATIC_CONTINUATION'
         AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type IN
            ('PLAN_ACCEPTED','REVIEW_STARTED','VERIFICATION_PASSED','USER_STEERING','USER_ANSWER_RECEIVED','TASK_REOPENED')),0)",
        [task],
        |r| r.get(0),
    )?)
}
