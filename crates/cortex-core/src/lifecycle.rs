//! Authoritative transition graph. Parity: orchestration/state.py:56-81,182-206;
//! attempt graph: delegation/work_graph.py:551-595. No deserializable authority.
use crate::{
    error::refuse,
    ownership::{self, OwnerEpoch},
    AssignmentState, CoreResult, Store, TaskState,
};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};

pub mod completion;
pub mod domain;
pub mod plan;
pub mod progress;
mod probe_correction;
pub mod reconciliation;
pub mod review;
pub mod runtime;
pub mod skill_proposals;
pub mod waits;
pub mod worker_checks;

pub const MAX_NODES: usize = 64;
pub const MAX_REVIEW_CYCLES: usize = 3;
pub const MAX_REVIEW_REBINDS: usize = 3;
pub const MAX_VERIFICATION_REPAIRS: usize = 3;
pub const MAX_COORDINATOR_RETURNS: usize = 3;
pub const MAX_AUTONOMOUS_COORDINATOR_RETURNS: usize = 6;

/// Graph only; stage-specific evidence gates are separate and must not be skipped.
pub fn allowed(source: TaskState, target: TaskState) -> bool {
    use TaskState::*;
    if source == Paused {
        return target == Cancelled;
    }
    if source.terminal() {
        return false;
    }
    if matches!(target, Blocked | Failed | Cancelled | Recovering) {
        return true;
    }
    match source {
        Created => target == Planning,
        Planning => matches!(target, PlanReady | AnswerReady | WaitingUser | Verifying),
        AnswerReady => target == Completed,
        PlanReady => target == Executing,
        Executing => matches!(target, Reviewing | Verifying | WaitingUser),
        Reviewing => matches!(target, Executing | Verifying | WaitingUser),
        Verifying => matches!(target, Reconciling | Planning | Reviewing),
        Reconciling => matches!(target, Completed | WaitingUser | Reviewing),
        WaitingUser => matches!(target, Planning | Reconciling | Executing | Reviewing),
        Recovering => matches!(
            target,
            Planning | Executing | Reviewing | Verifying | Reconciling
        ),
        Blocked | Paused | Failed | Cancelled | Completed => false,
    }
}
pub fn attempt_allowed(source: AssignmentState, target: AssignmentState) -> bool {
    use AssignmentState::*;
    match source {
        Planned => matches!(target, Running | Reported | Failed | NeedsRevision),
        Running => matches!(target, Reported | Failed | NeedsRevision),
        Reported => target == NeedsRevision,
        _ => false,
    }
}
pub fn event_tx(db: &Connection, task: &str, kind: &str, payload: Value) -> CoreResult<i64> {
    db.execute("INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES (?,?,'runtime',?,?)",
        (task,kind,serde_json::to_string(&payload)?,crate::now()))?;
    Ok(db.last_insert_rowid())
}
fn waiver(db: &Connection, task: &str) -> CoreResult<bool> {
    Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events e JOIN runtime_tasks t USING(task_id)
        WHERE e.task_id=? AND e.type='REVIEW_NOT_REQUIRED' AND json_extract(e.payload_json,'$.revision')=t.revision
        AND NOT EXISTS(SELECT 1 FROM runtime_nodes n WHERE n.task_id=t.task_id AND n.revision=t.revision AND n.stage='review'))",
        [task],|r|r.get(0))?)
}
/// Trusted runtime supplies filesystem observations BEFORE the transaction.
/// Later domain ports must provide complete review/completion/question evidence.
pub trait TransitionGuards {
    fn before_transition(
        &self,
        db: &Transaction<'_>,
        task: &str,
        source: TaskState,
        target: TaskState,
    ) -> CoreResult<()>;
    fn baseline(&self) -> Option<Value> {
        None
    }
    fn stop_tx(
        &self,
        _db: &Transaction<'_>,
        _task: &str,
        _source: TaskState,
        _blocker: &str,
    ) -> CoreResult<()> {
        refuse("question_domain_unavailable")
    }
}
/// Safe foundation default: never claims unfinished domains are implemented.
pub struct FoundationGuards;
impl TransitionGuards for FoundationGuards {
    fn before_transition(
        &self,
        _db: &Transaction<'_>,
        _task: &str,
        source: TaskState,
        target: TaskState,
    ) -> CoreResult<()> {
        if target == TaskState::Completed {
            return refuse("completion_domain_unavailable");
        }
        if source == TaskState::PlanReady && target == TaskState::Executing {
            return refuse("execution_baseline_required");
        }
        Ok(())
    }
}
pub struct TaskMachine<'a> {
    store: &'a Store,
}
impl<'a> TaskMachine<'a> {
    pub fn new(store: &'a Store) -> Self {
        Self { store }
    }
    pub fn create(&self, id: &str, goal: &str, owner: &str) -> CoreResult<OwnerEpoch> {
        self.store.write(|db| {
            db.execute("INSERT INTO tasks VALUES (?,?,?)",(id,self.store.key().root().to_str(),crate::now()))?;
            db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES (?,'CREATED',?,?,?)",
                (id,goal,crate::now(),crate::now()))?;
            let owner=ownership::acquire_tx(db,id,owner,None)?;
            event_tx(db,id,"TASK_CREATED",json!({}))?;
            crate::routing::snapshot_tx(db,id,self.store.key().root())?;
            Ok(owner)
        })
    }
    pub fn transition(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        key: &str,
        target: TaskState,
        blocker: Option<&str>,
        guards: &dyn TransitionGuards,
    ) -> CoreResult<Value> {
        self.store.write(|db| {
            ownership::check_tx(db, task, owner)?;
            let payload = json!({"target":target,"blocker":blocker,"owner_epoch":owner.epoch});
            request_tx(db, task, &owner.owner, key, "transition", &payload, || {
                let source: String = db.query_row(
                    "SELECT state FROM runtime_tasks WHERE task_id=?",
                    [task],
                    |r| r.get(0),
                )?;
                let source: TaskState = source.parse().map_err(crate::CoreError::Refused)?;
                if source == TaskState::Paused && target != TaskState::Cancelled {
                    return refuse("task_paused");
                }
                if !allowed(source, target)
                    || source == TaskState::Executing
                        && target == TaskState::Verifying
                        && !waiver(db, task)?
                {
                    return refuse("invalid_state_transition");
                }
                guards.before_transition(db, task, source, target)?;
                // Completion has a dedicated runtime certification entry point.
                // A permissive adapter guard must never turn chat into acceptance.
                if target == TaskState::Completed {
                    return refuse("runtime_completion_required");
                }
                if target == TaskState::Cancelled {
                    domain::cancel_task_tx(db, task, blocker.unwrap_or("cancelled"))?;
                    return Ok(json!({"state":"CANCELLED","outcome":"recorded"}));
                }
                if source == TaskState::PlanReady && target == TaskState::Executing {
                    let baseline = guards
                        .baseline()
                        .ok_or(crate::CoreError::Refused("execution_baseline_required"))?;
                    event_tx(db, task, "EXECUTION_BASELINE", baseline)?;
                }
                if target == TaskState::Blocked {
                    domain::stop_tx(db, task, blocker.unwrap_or("unknown"))?;
                } else {
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
                }
                let actual: String = db.query_row(
                    "SELECT state FROM runtime_tasks WHERE task_id=?",
                    [task],
                    |r| r.get(0),
                )?;
                Ok(json!({"state":actual,"outcome":"recorded"}))
            })
        })
    }
    pub fn transition_attempt(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        key: &str,
        attempt: &str,
        target: AssignmentState,
        reason: Option<&str>,
    ) -> CoreResult<Value> {
        self.store.write(|db| {
            ownership::check_tx(db,task,owner)?;
            request_tx(db,task,&owner.owner,key,"transition_attempt",&json!({"attempt":attempt,"target":target,"reason":reason}),|| {
                let row:Option<(String,String)>=db.query_row("SELECT status,assignment_id FROM work_assignment_attempts WHERE task_id=? AND attempt_id=?",
                    (task,attempt),|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
                let (source,assignment)=row.ok_or(crate::CoreError::Refused("unknown_attempt"))?;
                let source:AssignmentState=serde_json::from_value(Value::String(source))?;
                if !attempt_allowed(source,target) || matches!(target,AssignmentState::Failed|AssignmentState::NeedsRevision) && reason.is_none_or(str::is_empty) {
                    return refuse("invalid_attempt_transition");
                }
                let failure=if matches!(target,AssignmentState::Failed|AssignmentState::NeedsRevision){reason}else{None};
                db.execute("UPDATE work_assignment_attempts SET status=?,failure_reason=? WHERE attempt_id=?",(target.as_str(),failure,attempt))?;
                db.execute("UPDATE work_assignments SET status=? WHERE assignment_id=?",(target.as_str(),&assignment))?;
                let result=json!({"attempt_id":attempt,"task_id":task,"status":target,"failure_reason":failure});
                event_tx(db,task,"ASSIGNMENT_STATE_CHANGED",result.clone())?;
                Ok(result)
            })
        })
    }
}
/// Receipt, state and event share the SAME caller-owned transaction. Canonical
/// serde maps are sorted; identity includes operation, actor, task and owner fence.
pub fn request_tx(
    db: &Transaction<'_>,
    task: &str,
    actor: &str,
    key: &str,
    operation: &str,
    payload: &Value,
    body: impl FnOnce() -> CoreResult<Value>,
) -> CoreResult<Value> {
    if key.is_empty() || key.chars().count() > 128 {
        return refuse("invalid_request_key");
    }
    let digest = crate::digest(&serde_json::to_vec(
        &json!({"operation":operation,"payload":payload}),
    )?);
    let old:Option<(String,String)>=db.query_row("SELECT digest,result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?",
        (task,actor,key),|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    if let Some((old, result)) = old {
        if old != digest {
            return refuse("payload_conflict");
        }
        return Ok(serde_json::from_str(&result)?);
    }
    let result = body()?;
    db.execute(
        "INSERT INTO runtime_requests VALUES (?,?,?,?,?,?)",
        (
            task,
            key,
            actor,
            operation,
            digest,
            serde_json::to_string(&result)?,
        ),
    )?;
    Ok(result)
}
