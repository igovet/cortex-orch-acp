//! Runtime-owned progress. Models can submit plans/results, never certification.
//! Embedding calls advance after a settled turn and process wait, or after reopen.
use crate::{native_effects, native_sessions::NativeHost};
use cortex_core::{
    lifecycle::{self, completion, domain, review},
    ownership::{self, OwnerEpoch},
    ports::ProcessObservation,
    scheduler,
    store::documents::Publisher,
    CoreError, CoreResult, TaskState,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

struct WorkspaceScheduling(bool);
impl scheduler::SchedulingPolicy for WorkspaceScheduling {
    fn may_claim(&self, _: &rusqlite::Transaction<'_>, _: &str, _: &str) -> CoreResult<bool> {
        Ok(true)
    }

    fn supports_assignment_worktrees(&self) -> bool {
        self.0
    }
}
async fn recv<T>(r: tokio::sync::oneshot::Receiver<CoreResult<T>>) -> CoreResult<T> {
    r.await
        .map_err(|_| CoreError::Refused("native_host_closed"))?
}
fn require_producer_outputs_tx(
    store: &cortex_core::Store,
    db: &rusqlite::Transaction<'_>,
    task: &str,
    completing: bool,
) -> CoreResult<bool> {
    let Some(issue) = cortex_core::deliverables::producer_issue(store, db, task, completing)? else {
        return Ok(true);
    };
    lifecycle::event_tx(db, task, "SPRINT_OUTPUT_REQUIRED", issue)?;
    const REASON: &str = "sprint_output_required";
    if !domain::return_to_coordinator_tx(db, task, REASON)? {
        domain::stop_tx(db, task, REASON)?;
    }
    Ok(false)
}

/// Shared by initial dispatch, retained claims on restart and execution refills.
fn assignment_brief(
    db: &rusqlite::Transaction<'_>,
    task: &str,
    assignment: &str,
    node: &str,
    role: &str,
    objective: &str,
    scope_json: &str,
    stage: &str,
) -> CoreResult<Value> {
    let language = cortex_core::assignment_brief::record_language_tx(db, task)?;
    assignment_brief_at_language(db, task, assignment, node, role, objective, scope_json, stage, language)
}

/// Same brief shape for a retained blocker predating worker brief snapshots.
pub(crate) fn assignment_brief_at_language(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
    node: &str,
    role: &str,
    objective: &str,
    scope_json: &str,
    stage: &str,
    language: &str,
) -> CoreResult<Value> {
    let mut item = json!({"assignment_ref":assignment, "node":node, "role":role,
        "objective":objective, "scope":serde_json::from_str::<Value>(scope_json)?, "stage":stage});
    if let Value::Object(extra) =
        cortex_core::assignment_brief::extras(db, task, node, role, stage, language)?
    {
        item.as_object_mut().unwrap().extend(extra);
    }
    let revision: i64 = db.query_row(
        "SELECT revision FROM runtime_tasks WHERE task_id=?",
        [task],
        |row| row.get(0),
    )?;
    if let Some(answer) = lifecycle::obstacles::retry_decision_answer(db, task, node, revision)? {
        item["owner_decision_answer"] = json!(answer);
    }
    if let Some(brief) = lifecycle::obstacles::retry_decision_brief(db, task, node, revision)? {
        item["coordinator_decision_clarification"] = json!({
            "text":brief,
            "authority":"Clarification only. The original assignment boundaries, owner restrictions and all explicit consent requirements still apply."
        });
    }
    Ok(item)
}
/// Replace a review packet's predecessor list with the bounded split the result
/// check enforces: `predecessor_reports` (required, each to its last page),
/// `optional_reports` (with `optional_reports_more` when clipped) and
/// `unavailable_reports`, plus the rule that tells the reviewer which is which.
pub(crate) fn review_reading(
    db: &rusqlite::Connection,
    task: &str,
    node: &str,
    packet: &mut Value,
) -> CoreResult<()> {
    let split = cortex_core::assignment_brief::review_reports(db, task, node)?;
    packet["predecessor_reports"] = split["required"].clone();
    packet["optional_reports"] = split["optional"].clone();
    if let Some(more) = split.get("optional_more") {
        packet["optional_reports_more"] = more.clone();
    }
    if split["unavailable"]
        .as_array()
        .is_some_and(|a| !a.is_empty())
    {
        packet["unavailable_reports"] = split["unavailable"].clone();
    }
    packet["predecessor_reading"] = json!(cortex_core::assignment_brief::REVIEW_READING_RULE);
    Ok(())
}
impl NativeHost {
    /// Claim only newly-ready execution nodes after one worker releases a slot.
    /// This deliberately avoids the full `advance_step` path, which observes the
    /// checkout and reconciles review state. The caller already has a live execution
    /// batch, so a refill must be a small, owner-checked transaction.
    pub(super) async fn refill_execution(
        &self,
        t: String,
        o: OwnerEpoch,
        expected_revision: i64,
    ) -> CoreResult<Vec<Value>> {
        let _driver = self.driver.lock().await;
        recv(self.executor.publication(move |store| store.write(|db| {
            ownership::check_tx(db, &t, &o)?;
            let (state, revision) = domain::task_tx(db, &t)?;
            if state != TaskState::Executing || revision != expected_revision {
                return Ok(Vec::new());
            }
            cortex_core::deliverables::continue_tx(store, db, &t)?;
            // Publish localized obstacle state even when owner steering prevents a
            // refill; only the next claim must wait for that steering boundary.
            lifecycle::obstacles::classify_failed_tx(db, &t)?;
            if !lifecycle::steering::pending(db, &t)?.is_empty() {
                return Ok(Vec::new());
            }
            lifecycle::obstacles::retry_unblocked_tx(db, &t)?;
            if lifecycle::obstacles::unparked_failure(db, &t)?
                && !lifecycle::obstacles::parked(db, &t)?.iter()
                    .any(|(_, class, _)| class == lifecycle::obstacles::OWNER_DECISION)
            {
                return Ok(Vec::new());
            }
            let claimed = scheduler::claim_ready_tx(
                db,
                &t,
                &o,
                "native-host",
                &scheduler::policy::NativePolicy::default(),
            )?;
            if !claimed.is_empty() {
                lifecycle::progress::spend_tx(db, &t, lifecycle::progress::PROGRESS_BUDGET)?;
            }
            let mut assignments = Vec::new();
            for assignment_ref in claimed {
                let (node, role, objective, scope): (String, String, String, String) = db.query_row(
                    "SELECT node_id,role,objective,scope_json FROM runtime_nodes WHERE task_id=? AND revision=? AND assignment_ref=? AND stage='execution' AND status='running'",
                    (&t, revision, &assignment_ref),
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?;
                assignments.push(assignment_brief(db, &t, &assignment_ref, &node, &role, &objective, &scope, "execution")?);
            }
            Ok(assignments)
        }))?)
        .await
    }
    /// Serialized host driver. Returns durable dispatch descriptions or a stopped
    /// state; never starts/replays an ambiguous process. No model-callable route.
    pub async fn advance(&self, task: String, owner: OwnerEpoch) -> CoreResult<Value> {
        self.advance_step(task, owner, false).await
    }
    /// `advance`; with `recheck_after_verification` a passed verification returns
    /// `{"recheck":true}` instead of reconciling at once, so the caller can take the
    /// owner's queued messages at that boundary first.
    pub async fn advance_step(
        &self,
        task: String,
        owner: OwnerEpoch,
        recheck_after_verification: bool,
    ) -> CoreResult<Value> {
        let driver = self.driver.lock().await;
        for _ in 0..8 {
            let t = task.clone();
            let o = owner.clone();
            let state = recv(self.executor.publication(move |store| store.write(|db| {
                ownership::check_tx(db, &t, &o)?;
                let state = domain::task_tx(db, &t)?.0;
                if matches!(state, TaskState::PlanReady | TaskState::Executing | TaskState::Reviewing
                    | TaskState::Verifying | TaskState::Reconciling | TaskState::AnswerReady)
                {
                    cortex_core::deliverables::continue_tx(store, db, &t)?;
                }
                let waiting = matches!(state, TaskState::Reviewing | TaskState::Verifying)
                    && scheduler::policy::review_writer_tx(db, &t)?.is_some();
                Ok((state, lifecycle::automatic::held(db, &t)?, waiting))
            }))?)
            .await?;
            let (state, automatic_hold, waiting) = state;
            if waiting {
                return Ok(json!({"state":state,"assignments":[],"writeConflict":true}));
            }
            if automatic_hold {
                return Ok(json!({"state":state,"assignments":[],"automaticHold":true}));
            }
            match state {
                TaskState::PlanReady | TaskState::Executing | TaskState::Reviewing => {
                    if state == TaskState::Executing {
                        let t = task.clone();
                        let o = owner.clone();
                        let machine_state = self.workspace_state.clone();
                        let integration = recv(self.executor.publication(move |store| {
                            crate::machine::settle_assignment_checkouts(store, machine_state.as_deref(), &t, &o)
                        })?).await;
                        match integration {
                            Ok(()) => {}
                            Err(error) if matches!(error.code(), "assignment_worktree_target_busy" | "assignment_worktree_not_settled") => {
                                return Ok(json!({"state":state,"assignments":[],"obstacleWait":true}));
                            }
                            Err(error) => return Err(error),
                        }
                    }
                    // PlanReady establishes the baseline and claims execution nodes.
                    // Executing can enter review and claim its reviewer in this same
                    // step, so both it and Reviewing need the raw review hashes.
                    let checkout = self.execution_checkout(&task, &owner).await?;
                    let review_root = checkout.path().to_owned();
                    let (mut e, baseline_manifest, raw_manifest) =
                        if state == TaskState::PlanReady {
                            let (snapshot, manifest) =
                                native_effects::observe_manifest(review_root.clone()).await?;
                            (snapshot, manifest, Default::default())
                        } else {
                            native_effects::observe_review_manifest(review_root.clone()).await?
                        };
                    e.checkout_evidence = checkout.evidence(owner.epoch);
                    let t = task.clone();
                    let o = owner.clone();
                    let scheduling = WorkspaceScheduling(self.workspace_state.is_some());
                    let step=recv(self.executor.publication(move |store| store.write(|db| {
                        ownership::check_tx(db,&t,&o)?;
                        if domain::task_tx(db,&t)?.0!=state{return Err(CoreError::Refused("native_state_changed"));}
                        cortex_core::deliverables::continue_tx(store, db, &t)?;
                        // A retained owned wait permits a fresh retry; missing or
                        // ambiguous process evidence never does. This also works
                        // after reopening midway through an unfinished review.
                        let stopped=db.prepare("SELECT n.assignment_ref,p.attempt_id FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
                            JOIN delegation_provisioning p ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
                            WHERE n.task_id=? AND n.revision=t.revision AND n.status='running' AND n.result_json IS NULL
                            AND p.status='terminated'")?
                            .query_map([&t],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?.collect::<Result<Vec<_>,_>>()?;
                        for (assignment,attempt) in stopped{
                            scheduler::retry::retry_lost_tx(db,&t,&scheduler::retry::RetryObservation{
                                assignment:&assignment,attempt:Some(&attempt),process:ProcessObservation::ConfirmedExited,
                                now:std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|CoreError::Refused("clock_unavailable"))?.as_secs_f64(),
                                window:crate::native_runtime::retry_window()
                            })?;
                        }
                        let active:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND kind=\'worker\' AND state!=\'released\')",[&t],|r|r.get(0))?;
                        match state{
                            TaskState::PlanReady=>{
                                // An owner message that arrived during the planning turn applies
                                // before the plan starts: back to the coordinator with it.
                                if lifecycle::steering::apply_pending_tx(db,&t)? {return Ok(json!({"continue":true}));}
                                completion::execution_baseline_tx(db,&t,&e)?;
                                if let (Some(digest),Some(manifest))=(e.project_digest.as_deref(),baseline_manifest.as_ref()){lifecycle::reconciliation::record_baseline_manifest_tx(db,&t,digest,manifest)?;}
                            },
                            TaskState::Executing=>{
                                // Each failed node is classified once. Access, an outage or an
                                // owner decision parks it (its dependents wait, siblings run); a
                                // resolved access action or ended outage retries it at once. Only
                                // an unparked (technical) failure takes the coordinator route.
                                lifecycle::obstacles::classify_failed_tx(db,&t)?;
                                lifecycle::obstacles::retry_unblocked_tx(db,&t)?;
                                let failed=lifecycle::obstacles::unparked_failure(db,&t)?;
                                let unfinished:bool=db.query_row(
                                    "SELECT EXISTS(SELECT 1 FROM runtime_nodes n JOIN runtime_tasks t USING(task_id) WHERE n.task_id=? AND n.revision=t.revision AND n.status!='completed')",
                                     [&t],|r|r.get(0))?;
                                // Do not abandon an unresolved decision or its pending
                                // question when a different node fails or is stopped.
                                // The current graph remains claimable until the decision settles.
                                let owner_stopped = !failed && !active && domain::owner_stopped_work_is_idle_tx(db, &t)?;
                                if !active && (failed || owner_stopped) {
                                    let decision_waiting = lifecycle::obstacles::parked(db, &t)?.iter()
                                        .any(|(_, class, _)| class == lifecycle::obstacles::OWNER_DECISION);
                                    if !decision_waiting {
                                        let reason = if failed { "worker_failed" } else { "worker_stopped_by_owner" };
                                        if !domain::return_to_coordinator_tx(db, &t, reason)? {
                                            domain::stop_tx(db, &t, reason)?;
                                        }
                                    }
                                }
                                else if !unfinished&&!active{
                                    if scheduler::policy::review_writer_tx(db,&t)?.is_some(){return Ok(json!({"state":state,"assignments":[],"writeConflict":true}));}
                                    if !require_producer_outputs_tx(store, db, &t, false)? {
                                        return Ok(json!({"continue":true}));
                                    }
                                    completion::add_review_tx(db,&t,&e)?;
                                }
                            },
                            TaskState::Reviewing=>{
                                if scheduler::policy::review_writer_tx(db,&t)?.is_some() {
                                    return Ok(json!({"state":state,"assignments":[],"writeConflict":true}));
                                }
                                let review_status:String=db.query_row("SELECT coalesce((SELECT n.status FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
                                    WHERE n.task_id=? AND n.revision=t.revision AND n.stage='review' ORDER BY n.rowid DESC LIMIT 1),'missing')",[&t],|r|r.get(0))?;
                                if !active{
                                    if review_status=="completed"{review::advance_tx(db,&t)?;}
                                    else if review_status=="failed"{
                                        if lifecycle::obstacles::park_failed_review_tx(db,&t)? {
                                            if !lifecycle::obstacles::retry_unblocked_tx(db,&t)?.is_empty() {
                                                // A pending review reserves the checkout again. Observe its
                                                // new manifest only after that reservation is committed.
                                                return Ok(json!({"continue":true}));
                                            }
                                        } else { domain::stop_tx(db,&t,"review_blocked")?; }
                                    }
                                    // An owner-stopped reviewer is not a dead end either.
                                    else if review_status=="cancelled"&&domain::owner_stop_unhandled_tx(db,&t)?&&!domain::return_to_coordinator_tx(db,&t,"worker_stopped_by_owner")?{domain::stop_tx(db,&t,"worker_stopped_by_owner")?;}
                                }
                            },_=>unreachable!()
                        }
                        let current=domain::task_tx(db,&t)?.0;
                        if !matches!(current,TaskState::Executing|TaskState::Reviewing){return Ok(json!({"continue":true}));}
                        let batch = scheduler::claim_ready_batch_tx(db,&t,&o,"native-host",&scheduling)?;
                        let mut rows=db.prepare("SELECT n.assignment_ref,n.node_id,n.role,n.objective,n.scope_json,n.stage FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
                            WHERE n.task_id=? AND n.revision=t.revision AND n.status='running'
                            AND NOT EXISTS(SELECT 1 FROM delegation_provisioning p WHERE p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref) ORDER BY n.rowid")?
                            .query_map([&t],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?)))?
                            .collect::<Result<Vec<_>,_>>()?;
                        if batch.measured_priority {
                            // Older undispatched claims retain their place ahead of
                            // this batch; its newly claimed nodes follow the DAG rank.
                            rows.sort_by_key(|(assignment, ..)| {
                                batch.assignments.iter().position(|claimed| claimed == assignment)
                            });
                        }
                        let mut assignments=Vec::new();
                        for (a,node,role,objective,scope,stage) in rows{
                            let mut item=assignment_brief(db,&t,&a,&node,&role,&objective,&scope,&stage)?;
                            // A reviewer is told exactly what the work changed and how much
                            // predecessor evidence there is to read.
                            if stage=="review" && !lifecycle::sprint_impact::is_revision_review(&node){
                                let mut packet=lifecycle::reconciliation::review_packet_tx(db,&t,&node,baseline_manifest.as_ref(),e.ignored_digest.as_deref(),&raw_manifest)?;
                                // What the review binds is kept with the recorded packet,
                                // not in the reviewer's brief.
                                let binding=packet.as_object_mut().and_then(|p|p.remove("review_binding")).unwrap_or(Value::Null);
                                // Required reading is bounded to this review's round
                                // (`assignment_brief::review_reports`); older reports are optional.
                                review_reading(db,&t,&node,&mut packet)?;
                                let base=crate::native_project::review_base(db,&t,&packet)?;
                                let bytes=crate::native_project::add_review_lines(&review_root,base.as_deref(),&mut packet);
                                let risk=cortex_core::strategy::review_strategy(db,&t,&mut packet,bytes)?;
                                completion::mark_reviewed_before(db,&t,&a,&mut packet)?;
                                item["review_packet"]=packet.clone();
                                // Persist the exact authoritative packet used for this
                                // reviewer.  Result submission must validate coverage
                                // against this packet, not against a model's recollection.
                                let recorded:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='REVIEW_PACKET' AND json_extract(payload_json,'$.assignment_ref')=?)",(&t,&a),|r|r.get(0))?;
                                if !recorded {
                                    lifecycle::event_tx(db,&t,"REVIEW_PACKET",serde_json::json!({"assignment_ref":a,"node":node,"packet":packet,"binding":binding}))?;
                                    review::escalate_for_observed_risk_tx(db,&t,risk)?;
                                }
                                // The review binds its packet's binding: the task files' current
                                // bytes, or the whole checkout.
                                let bound:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED' AND json_extract(payload_json,'$.assignment_ref')=?)",
                                    (&t,&a),|r|r.get(0))?;
                                if !bound{
                                    if let Some(paths)=completion::review_bound_files(db,&t,&a)? {
                                        e.task_files.extend(crate::native_change_set::identify(&review_root,paths.into_iter().collect()));
                                    }
                                    completion::start_review_tx(db,&t,&a,&e)?;
                                }
                            } else if stage=="review" {
                                // A sprint revision judges an immutable proposal, not the
                                // checkout. Its empty task-file binding tolerates unrelated edits.
                                let bound:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED' AND json_extract(payload_json,'$.assignment_ref')=?)",
                                    (&t,&a),|r|r.get(0))?;
                                if !bound {
                                    lifecycle::event_tx(db,&t,"REVIEW_PACKET",json!({"assignment_ref":a,"node":node,
                                        "packet":{"changed_files":{"files":[],"complete":true,"attribution_complete":true}},
                                        "binding":{"kind":"task_files","paths":[]}}))?;
                                    completion::start_review_tx(db,&t,&a,&e)?;
                                }
                            }
                            assignments.push(item);
                        }
                        // Keep the stage and graph intact while the driver waits.
                        if assignments.is_empty() && !active && !lifecycle::obstacles::parked(db,&t)?.is_empty() {
                            return Ok(json!({"state":current,"assignments":[],"obstacleWait":true}));
                        }
                        let conflict = assignments.is_empty() && scheduler::policy::write_conflict_tx(db,&t)?.is_some();
                        Ok(json!({"state":current,"revision":domain::task_tx(db,&t)?.1,"assignments":assignments,"writeConflict":conflict}))
                    }))?).await?;
                    if step["continue"] == true {
                        continue;
                    }
                    return Ok(step);
                }
                TaskState::Verifying => {
                    let t = task.clone();
                    let o = owner.clone();
                    let outputs_ready = recv(self.executor.publication(move |store| store.write(|db| {
                        ownership::check_tx(db, &t, &o)?;
                        if domain::task_tx(db, &t)?.0 != TaskState::Verifying {
                            return Ok(false);
                        }
                        require_producer_outputs_tx(store, db, &t, false)
                    }))?).await?;
                    if !outputs_ready {
                        continue;
                    }
                    if let Verified::Waiting(left) = self.verify_plan(&task, &owner).await? {
                        // The runtime's own wait for the verifier's environment: no
                        // model call. The progress loop comes back after a short pause
                        // (and takes owner messages meanwhile); other callers decide.
                        drop(driver);
                        if recheck_after_verification {
                            tokio::time::sleep(left.min(std::time::Duration::from_secs(5))).await;
                        }
                        return Ok(
                            json!({"state":"VERIFYING","assignments":[],"recheck":recheck_after_verification,
                            "verificationWait":true,"retryInSeconds":left.as_secs()}),
                        );
                    }
                    if recheck_after_verification {
                        return Ok(json!({"recheck":true}));
                    }
                    continue;
                }
                TaskState::AnswerReady | TaskState::Reconciling => {
                    let checkout = self.execution_checkout(&task, &owner).await?;
                    let root = checkout.path().to_owned();
                    let (mut e, manifest) = native_effects::observe_manifest(root.clone()).await?;
                    e.checkout_evidence = checkout.evidence(owner.epoch);
                    let ignored = e.ignored_digest.clone();
                    let t = task.clone();
                    let o = owner.clone();
                    let sprint_state = self.workspace_state.clone();
                    let sprint_routes = self.routes.read().unwrap().clone();
                    let completed=recv(self.executor.publication(move|store|{
                        let publisher=Publisher::new(store);
                        // A reviewed sprint revision is applied or handed to the owner first.
                        if let Some(state) = &sprint_state {
                            let root = store.key().root().to_owned();
                            let _ = crate::machine::sprint_revision::settle(store, state, &t,
                                &|c| sprint_routes.as_ref()?.sprint_route(&root, c).ok().flatten());
                        }
                        let rows=store.read(|db|{
                            let mut q=db.prepare("SELECT json_set(n.result_json,'$.assignment_ref',n.assignment_ref) FROM runtime_nodes n JOIN runtime_tasks t USING(task_id) WHERE n.task_id=? AND n.revision=t.revision AND n.result_json IS NOT NULL ORDER BY n.rowid")?;
                            let rows=q.query_map([&t],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
                            Ok(rows)
                        })?;
                        let mut e=e;
                        for row in rows{
                            let v:Value=serde_json::from_str(&row)?;
                            let report=v["report_ref"].as_str().ok_or(CoreError::Refused("assignment_report_missing"))?;
                            e.artifacts.insert(report.into(),publisher.read_report(&t,report,None,4000)?.sha256);
                            let history=store.read(|db| {
                                let channel:Option<String>=db.query_row("SELECT c.channel_id FROM trusted_channels c JOIN delegation_provisioning p ON p.task_id=c.task_id AND p.attempt_id=c.attempt_id WHERE c.task_id=? AND p.idempotency_key=? ORDER BY c.rowid DESC LIMIT 1",
                                    (&t,v["assignment_ref"].as_str()),|r|r.get(0)).optional()?;
                                match channel {Some(c)=>lifecycle::worker_checks::load_runs(db,&t,&c),None=>Ok(Vec::new())}
                            })?;
                            let mut checks=v["checks"].as_array().cloned().unwrap_or_default();
                            let relevant=lifecycle::worker_checks::relevant_references(&checks,&history);
                            checks.extend(history.into_iter().filter(|r|r.receipt["tool_call_ref"].as_str().is_some_and(|r|relevant.contains(r))).map(|r|r.receipt));
                            let mut seen=std::collections::BTreeSet::new();
                            for check in &checks {
                                if let Some(reference)=check["tool_call_ref"].as_str(){
                                    if !seen.insert(reference.to_owned()) {continue;}
                                    let output:Option<(String,String,String,i64)>=store.read(|db|Ok(db.query_row(
                                        "SELECT l.id,l.filename,l.source_ref,l.byte_count FROM runtime_requests r JOIN large_outputs l
                                        ON l.id=coalesce(json_extract(r.result_json,'$.large_output_id'),json_extract(r.result_json,'$.reply.structuredContent.large_output_id'))
                                        AND l.task_id=r.task_id AND l.owner_scope=r.actor WHERE r.task_id=? AND r.operation IN ('command_run','outside_command')
                                        AND coalesce(json_extract(r.result_json,'$.tool_call_ref'),json_extract(r.result_json,'$.reply.structuredContent.tool_call_ref'))=?",
                                        (&t,reference),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?))?;
                                    if let Some((id,file,expected,size))=output{
                                        if file.contains('/')||file.contains(".."){continue;}
                                        let Ok(mut file)=cortex_core::store::locks::open_private(&store.key().directory().join(&t).join(".native-output").join(file),false,false) else {continue};
                                        let Ok((hash,count))=crate::native_commands::fingerprint(&mut file) else {continue};
                                        // An unavailable unrelated run stays unknown; it must not
                                        // certify a link or masquerade as a successful later run.
                                        if hash==expected && count as i64==size { e.artifacts.insert(id,hash); }
                                    }
                                }
                            }
                        }
                        store.write(|db|{
                            ownership::check_tx(db,&t,&o)?;
                            let workers=db.prepare("SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator' AND status='terminated'")?
                                .query_map([&t],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
                            for a in workers{e.workers.insert(a,ProcessObservation::ConfirmedExited);}
                            // Completion compares what the approving review bound.
                            crate::native_change_set::bind_task_files_tx(db,&root,&t,&mut e)?;
                            // Owner messages recorded while this revision ran apply here, after its
                            // workers, review and verification: the coordinator replans with them
                            // instead of the result completing past them.
                            if lifecycle::steering::apply_pending_tx(db,&t)? {return Ok(false);}
                            if lifecycle::sprint_impact::pending(db, &t)?.is_none()
                                && !require_producer_outputs_tx(store, db, &t, true)? {
                                return Ok(true);
                            }
                            let saved:Option<String>=db.query_row("SELECT synthesis FROM runtime_tasks WHERE task_id=?",[&t],|r|r.get(0))?;
                            let synthesis=if let Some(saved)=saved { saved } else {
                                // Only the clean fast path may compose without a coordinator.
                                match lifecycle::reconciliation::compose_runtime_synthesis_tx(db,&t,manifest.as_ref(),ignored.as_deref())?{
                                    Some(text)=>text,
                                    None=>return Ok(false),
                                }
                            };
                            let certification = if state==TaskState::AnswerReady { lifecycle::reconciliation::close_answer_tx(db,&t,&e) }
                                else { completion::complete_tx(db,&t,&e) };
                            match certification {
                                Err(CoreError::Refused("completion_unresolved_prior_worker_check")) => {
                                    if !domain::return_to_coordinator_tx(db,&t,"unresolved_prior_worker_check")? {
                                        domain::stop_tx(db,&t,"unresolved_prior_worker_check")?;
                                    }
                                    return Ok(true); // Return the new durable state, without publishing synthesis.
                                }
                                // Input still waiting for the coordinator holds completion:
                                // each blocking item is named once, and the coordinator's next
                                // turn (which carries it in its capsule) delivers it, instead
                                // of the driver stopping into the strategy ladder.
                                // An owner change of a sprint member waits for its impact
                                // verdict: named once, the coordinator's next turn records it.
                                Err(CoreError::Refused("sprint_impact_pending")) => {
                                    lifecycle::sprint_impact::record_pending_tx(db,&t)?;
                                    return Ok(false);
                                }
                                Err(CoreError::Refused("completion_pending_input")) => {
                                    let items=completion::pending_input_tx(db,&t)?;
                                    let last:Option<String>=db.query_row("SELECT json_extract(payload_json,'$.items') FROM runtime_events
                                        WHERE task_id=? AND type='COMPLETION_INPUT_PENDING' ORDER BY id DESC LIMIT 1",[&t],|r|r.get(0)).optional()?;
                                    if last.and_then(|s|serde_json::from_str::<Value>(&s).ok()).as_ref()!=Some(&json!(items)) {
                                        lifecycle::event_tx(db,&t,"COMPLETION_INPUT_PENDING",json!({"items":items}))?;
                                    }
                                    return Ok(false);
                                }
                                result => result?,
                            }
                            let sessions=db.prepare("SELECT session_id FROM acp_host_sessions WHERE task_id=?
                                AND json_extract(client_info,'$.name')='cortex-rust-acp'")?
                                .query_map([&t],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
                            for session in sessions {
                                let mut seq:i64=db.query_row("SELECT coalesce(max(seq),-1)+1 FROM (
                                    SELECT seq FROM host_transcript WHERE session_id=? UNION ALL
                                    SELECT seq FROM host_updates WHERE session_id=?)",(&session,&session),|r|r.get(0))?;
                                let chars:Vec<_>=synthesis.chars().collect();
                                let message_id=format!("synthesis:{session}:{seq}");
                                for part in chars.chunks(16_000) {
                                    let text:String=part.iter().collect();
                                    db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,message_id,created_at) VALUES (?,?,'agent_message',?,?,?)",
                                        (&session,seq,text,&message_id,cortex_core::now()))?;
                                    seq+=1;
                                }
                            }
                            Ok(true)
                        })
                    })?).await?;
                    if completed {
                        let t = task.clone();
                        let state = recv(self.executor.write(move |db| domain::task_tx(db, &t))?)
                            .await?
                            .0;
                        return Ok(json!({"state":state}));
                    }
                    return Ok(json!({"state":"RECONCILING","coordinator":true}));
                }
                _ => return Ok(json!({"state":state})),
            }
        }
        Err(CoreError::Refused("native_driver_progress_limit"))
    }
}

mod verification;
use verification::Verified;

#[cfg(test)]
mod sprint_tests;

pub(crate) mod sprints;
