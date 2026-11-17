//! Writer-executor entry point for trusted runtime/owner adapters. These types
//! deliberately do not implement Deserialize. Transport authorization belongs
//! to the daemon; a JSON payload cannot construct runtime completion authority.
use super::{
    completion::{self, EvidenceSnapshot},
    domain, request_tx,
};
use crate::{
    error::refuse,
    ownership::{self, OwnerEpoch},
    store::executor::StoreExecutor,
    CoreResult, TaskState,
};
use rusqlite::Transaction;
use serde::Serialize;
use serde_json::{json, Value};
#[derive(Serialize)]
pub enum RuntimeAction {
    Cancel { reason: String },
    Stop { blocker: String },
    StopAssignment { assignment: String, reason: String },
    Interrupt,
    Pause { resume: bool, expected_version: i64 },
    Recover { target: TaskState },
    ReturnToCoordinator { reason: String },
    BeginExecution,
    AddReview,
    AdvanceReview,
    StartReview { assignment: String },
    RecordResult { assignment: String, result: Value },
    RecordVerification { receipt_keys: Vec<String> },
    SubmitAnswer { text: String },
    SubmitSynthesis { text: String },
    Complete,
}
pub struct RuntimeRequest {
    pub task: String,
    pub owner: OwnerEpoch,
    pub key: String,
    pub action: RuntimeAction,
    pub evidence: EvidenceSnapshot,
}
pub fn execute_tx(db: &Transaction<'_>, r: &RuntimeRequest) -> CoreResult<Value> {
    ownership::check_tx(db, &r.task, &r.owner)?;
    // Pause has its own replay ledger, including the displayed version.
    if let RuntimeAction::Pause {
        resume,
        expected_version,
    } = r.action
    {
        return domain::owner_pause_tx(db, &r.task, &r.owner, &r.key, resume, expected_version);
    }
    let payload = json!({"action":r.action,"owner_epoch":r.owner.epoch});
    request_tx(
        db,
        &r.task,
        &r.owner.owner,
        &r.key,
        "runtime_action",
        &payload,
        || {
            let t = &r.task;
            let e = &r.evidence;
            match &r.action {
                RuntimeAction::Cancel { reason } => domain::cancel_task_tx(db, t, reason)?,
                RuntimeAction::Stop { blocker } => {
                    domain::stop_tx(db, t, blocker)?;
                }
                RuntimeAction::StopAssignment { assignment, reason } => {
                    domain::stop_assignment_tx(db, t, assignment, reason)?
                }
                RuntimeAction::Interrupt => domain::interrupt_tx(db, t)?,
                RuntimeAction::Recover { target } => domain::settle_recovery_tx(db, t, *target)?,
                RuntimeAction::ReturnToCoordinator { reason } => {
                    return Ok(json!({"returned":domain::return_to_coordinator_tx(db,t,reason)?}))
                }
                RuntimeAction::BeginExecution => completion::execution_baseline_tx(db, t, e)?,
                RuntimeAction::AddReview => completion::add_review_tx(db, t, e)?,
                RuntimeAction::AdvanceReview => super::review::advance_tx(db, t)?,
                RuntimeAction::StartReview { assignment } => {
                    completion::start_review_tx(db, t, assignment, e)?
                }
                RuntimeAction::RecordResult { assignment, result } => {
                    if let Some(refusal) =
                        super::review::preflight_result_tx(db, t, assignment, &r.key, e)?
                    {
                        return Ok(refusal);
                    }
                    completion::record_result_tx(db, t, assignment, result, e)?;
                }
                RuntimeAction::RecordVerification { receipt_keys } => {
                    return Ok(
                        json!({"passed":completion::record_verification_tx(db,t,receipt_keys,e)?}),
                    )
                }
                RuntimeAction::SubmitAnswer { text } => completion::submit_answer_tx(db, t, text)?,
                RuntimeAction::SubmitSynthesis { text } => {
                    if domain::task_tx(db, t)?.0 != TaskState::Reconciling || text.trim().is_empty()
                    {
                        return refuse("synthesis_not_expected");
                    }
                    db.execute(
                        "UPDATE runtime_tasks SET synthesis=? WHERE task_id=?",
                        (text, t),
                    )?;
                    super::event_tx(db, t, "SYNTHESIS_SUBMITTED", json!({"text":text}))?;
                }
                RuntimeAction::Complete => completion::complete_tx(db, t, e)?,
                RuntimeAction::Pause { .. } => unreachable!(),
            }
            Ok(json!({"outcome":"recorded","state":domain::task_tx(db,t)?.0}))
        },
    )
}
pub fn submit(
    executor: &StoreExecutor,
    request: RuntimeRequest,
) -> CoreResult<tokio::sync::oneshot::Receiver<CoreResult<Value>>> {
    executor.write(move |db| execute_tx(db, &request))
}
