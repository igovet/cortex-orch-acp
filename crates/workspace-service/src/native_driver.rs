//! Runtime-owned progress. Models can submit plans/results, never certification.
//! Embedding calls advance after a settled turn and process wait, or after reopen.
use crate::{native_effects, native_sessions::NativeHost};
use cortex_core::{
    lifecycle::{self, completion, domain, plan, review},
    ownership::{self, OwnerEpoch},
    ports::{CommandObservation, ProcessObservation},
    scheduler,
    store::documents::Publisher,
    CoreError, CoreResult, TaskState,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
async fn recv<T>(r: tokio::sync::oneshot::Receiver<CoreResult<T>>) -> CoreResult<T> {
    r.await
        .map_err(|_| CoreError::Refused("native_host_closed"))?
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
    let mut item = json!({"assignment_ref":assignment, "node":node, "role":role,
        "objective":objective, "scope":serde_json::from_str::<Value>(scope_json)?, "stage":stage});
    if let Value::Object(extra) =
        cortex_core::assignment_brief::extras(db, task, node, role, stage, language)?
    {
        item.as_object_mut().unwrap().extend(extra);
    }
    if let Some(answer) = lifecycle::obstacles::retry_decision_answer(
        db, task, node, domain::task_tx(db, task)?.1,
    )? {
        item["owner_decision_answer"] = json!(answer);
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
        recv(self.executor.write(move |db| {
            ownership::check_tx(db, &t, &o)?;
            let (state, revision) = domain::task_tx(db, &t)?;
            if state != TaskState::Executing || revision != expected_revision {
                return Ok(Vec::new());
            }
            // Publish localized obstacle state even when owner steering prevents a
            // refill; only the next claim must wait for that steering boundary.
            lifecycle::obstacles::classify_failed_tx(db, &t)?;
            if !lifecycle::steering::pending(db, &t)?.is_empty() {
                return Ok(Vec::new());
            }
            lifecycle::obstacles::retry_unblocked_tx(db, &t)?;
            if lifecycle::obstacles::unparked_failure(db, &t)? { return Ok(Vec::new()); }
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
        })?)
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
            let state = recv(self.executor.write(move |db| {
                ownership::check_tx(db, &t, &o)?;
                let state = domain::task_tx(db, &t)?.0;
                let waiting = matches!(state, TaskState::Reviewing | TaskState::Verifying)
                    && scheduler::policy::review_writer_tx(db, &t)?.is_some();
                Ok((state, lifecycle::automatic::held(db, &t)?, waiting))
            })?)
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
                    // The baseline also retains the project's per-file manifest, which
                    // lets a clean first-review path list the files the work changed.
                    // The same pass also gives the per-file manifest: the execution
                    // baseline keeps it, and a review packet lists the files changed since.
                    let checkout = self.execution_checkout(&task, &owner).await?;
                    let review_root = checkout.path().to_owned();
                    let (mut e, baseline_manifest, raw_manifest) =
                        native_effects::observe_review_manifest(review_root.clone()).await?;
                    e.checkout_evidence = checkout.evidence(owner.epoch);
                    let t = task.clone();
                    let o = owner.clone();
                    let step=recv(self.executor.write(move|db|{
                        ownership::check_tx(db,&t,&o)?;
                        if domain::task_tx(db,&t)?.0!=state{return Err(CoreError::Refused("native_state_changed"));}
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
                                if failed&&!active&&!domain::return_to_coordinator_tx(db,&t,"worker_failed")?{domain::stop_tx(db,&t,"worker_failed")?;}
                                // An owner-stopped worker is not a dead end: siblings keep
                                // running, and once nothing else is runnable the coordinator
                                // decides. Dependents of the stopped node never start silently.
                                else if !failed&&!active&&domain::owner_stopped_work_is_idle_tx(db,&t)?
                                    &&!domain::return_to_coordinator_tx(db,&t,"worker_stopped_by_owner")?{domain::stop_tx(db,&t,"worker_stopped_by_owner")?;}
                                else if !unfinished&&!active{
                                    if scheduler::policy::review_writer_tx(db,&t)?.is_some(){return Ok(json!({"state":state,"assignments":[],"writeConflict":true}));}
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
                        scheduler::claim_ready_tx(db,&t,&o,"native-host",&scheduler::policy::NativePolicy::default())?;
                        let rows=db.prepare("SELECT n.assignment_ref,n.node_id,n.role,n.objective,n.scope_json,n.stage FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
                            WHERE n.task_id=? AND n.revision=t.revision AND n.status='running'
                            AND NOT EXISTS(SELECT 1 FROM delegation_provisioning p WHERE p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref) ORDER BY n.rowid")?
                            .query_map([&t],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,String>(3)?,r.get::<_,String>(4)?,r.get::<_,String>(5)?)))?
                            .collect::<Result<Vec<_>,_>>()?;
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
                                crate::native_project::add_review_lines(&review_root,base.as_deref(),&mut packet);
                                completion::mark_reviewed_before(db,&t,&a,&mut packet)?;
                                item["review_packet"]=packet.clone();
                                // Persist the exact authoritative packet used for this
                                // reviewer.  Result submission must validate coverage
                                // against this packet, not against a model's recollection.
                                let recorded:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='REVIEW_PACKET' AND json_extract(payload_json,'$.assignment_ref')=?)",(&t,&a),|r|r.get(0))?;
                                if !recorded { lifecycle::event_tx(db,&t,"REVIEW_PACKET",serde_json::json!({"assignment_ref":a,"node":node,"packet":packet,"binding":binding}))?; }
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
                                // A sprint revision review reads a proposed graph, not the
                                // task's changes: no change packet binds it.
                                let bound:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED' AND json_extract(payload_json,'$.assignment_ref')=?)",
                                    (&t,&a),|r|r.get(0))?;
                                if !bound { completion::start_review_tx(db,&t,&a,&e)?; }
                            }
                            assignments.push(item);
                        }
                        // Keep the stage and graph intact while the driver waits.
                        if assignments.is_empty() && !active && !lifecycle::obstacles::parked(db,&t)?.is_empty() {
                            return Ok(json!({"state":current,"assignments":[],"obstacleWait":true}));
                        }
                        let conflict = assignments.is_empty() && scheduler::policy::write_conflict_tx(db,&t)?.is_some();
                        Ok(json!({"state":current,"revision":domain::task_tx(db,&t)?.1,"assignments":assignments,"writeConflict":conflict}))
                    })?).await?;
                    if step["continue"] == true {
                        continue;
                    }
                    return Ok(step);
                }
                TaskState::Verifying => {
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
    /// Runs the plan's verification commands once on the reviewed binding and records
    /// the outcome. An environmental outcome (`completion::verifier_environment`) is
    /// rerun once by the runtime itself (`VERIFICATION_RERUN`, no model call); if it
    /// fails the same way again, the verification becomes the runtime's own wait
    /// (`VERIFICATION_ENVIRONMENT_WAIT`, no replan and no ladder rung) and is run again
    /// after the next backoff step (1, 2, 5, 10 minutes), after which the runtime asks the
    /// owner one question. A real failure stays a failure; nothing
    /// unverified is recorded as passed.
    async fn verify_plan(&self, task: &str, owner: &OwnerEpoch) -> CoreResult<Verified> {
        // A verification waiting for its environment is not run, nor the project
        // observed, before its recheck is due.
        let t = task.to_owned();
        let wait = recv(
            self.executor
                .read(move |db| completion::verification_wait_tx(db, &t))?,
        )
        .await?;
        let now = unix_now()?;
        let rechecking = match &wait {
            Some(w) => {
                let due = w["retry_not_before"].as_f64().unwrap_or(0.0);
                if due > now {
                    return Ok(Verified::Waiting(std::time::Duration::from_secs_f64(
                        due - now,
                    )));
                }
                true
            }
            None => false,
        };
        // One observation anchors the review preflight, the recorded intent
        // digest, every post-command drift check and the final record.
        // Verification compares what the approving review bound: the task's own files
        // (identified in the observation) or the whole checkout.
        let checkout = self.execution_checkout(task, owner).await?;
        let root = checkout.path().to_owned();
        let mut e = native_effects::observe(root.clone()).await?;
        e.checkout_evidence = checkout.evidence(owner.epoch);
        crate::native_change_set::bind_task_files(&self.executor, &root, task, &mut e).await?;
        let e = std::sync::Arc::new(e);
        let digest = e.project_digest.clone();
        let t = task.to_owned();
        let o = owner.clone();
        let intent_digest = digest.clone();
        let preflight = std::sync::Arc::clone(&e);
        let run = format!("verify-{}", native_effects::identity()?);
        let intent=recv(self.executor.write(move|db|{
            ownership::check_tx(db,&t,&o)?;
            let (_,revision)=domain::task_tx(db,&t)?;
            // A run of this revision that started and never finished (a restart or crash
            // mid-run) has an unknown outcome: it is settled as unknown and verification
            // runs again, instead of every later verification of the revision being refused.
            completion::settle_interrupted_verification_tx(db,&t)?;
            match completion::verification_preflight_tx(db,&t,&preflight) {
                Err(e) if e.code()=="verification_review_revision_changed" => {
                    completion::review_changed_checkout_tx(db,&t,&preflight)?;
                    return Ok(None);
                }
                result => result?,
            }
            // A sprint planning task without commands: the runtime verifies its reviewed plan.
            if lifecycle::sprint_planning::enabled(db,&t)? {
                let p:Option<String>=db.query_row("SELECT plan_json FROM runtime_tasks WHERE task_id=?",[&t],|r|r.get(0))?;
                let commands=match p { Some(p)=>serde_json::from_str::<Value>(&p)?["verification"].as_array().map_or(0,Vec::len), None=>0 };
                if commands==0 {
                    lifecycle::sprint_planning::verify_tx(db,&t)?;
                    return Ok(None);
                }
            }
            let p:String=db.query_row("SELECT plan_json FROM runtime_tasks WHERE task_id=?",[&t],|r|r.get(0))?;
            let commands=plan::verification(&serde_json::from_str::<Value>(&p)?["verification"])?;
            let bound=completion::current_binding_digest(db,&t,&preflight)?;
            lifecycle::request_tx(db,&t,"native-verifier",&run,"native_verification_intent",
                &json!({"revision":revision,"digest":intent_digest,"binding_digest":bound,"commands":commands}),||Ok(json!({"revision":revision,"digest":intent_digest,"binding_digest":bound})))?;
            Ok(Some((commands,run,bound)))
        })?).await?;
        let Some((commands, run, bound)) = intent else {
            return Ok(Verified::Recorded);
        };
        let mut keys = Vec::new();
        let mut all_exits_observed = true;
        for (index, argv) in commands.into_iter().enumerate() {
            let key = format!("{run}:{index}");
            let (mut receipt, mut environment) = self
                .verification_run(task, owner, &run, &key, &argv, &bound, &digest)
                .await?;
            let mut key = key;
            let mut receipts = vec![receipt["tool_call_ref"].clone()];
            // The first environmental outcome of a fresh verification is rerun once,
            // exactly, by the runtime; a due recheck of a waiting verification is
            // itself that rerun.
            if let (Some(reason), false) = (environment, rechecking) {
                let (t, o, a, first) = (
                    task.to_owned(),
                    owner.clone(),
                    json!(argv),
                    receipt["tool_call_ref"].clone(),
                );
                recv(self.executor.write(move |db| {
                    ownership::check_tx(db, &t, &o)?;
                    completion::record_verification_rerun_tx(db, &t, &a, reason, &first)
                })?)
                .await?;
                key = format!("{run}:{index}:rerun");
                (receipt, environment) = self
                    .verification_run(task, owner, &run, &key, &argv, &bound, &digest)
                    .await?;
                receipts.push(receipt["tool_call_ref"].clone());
            }
            // Drift is a change of what the review bound: with a task-file binding,
            // another actor's edit elsewhere in the checkout is not drift.
            let current = self.execution_checkout(task, owner).await?;
            if current.path() != root { return Err(CoreError::Refused("worktree_binding_invalid")); }
            let mut fresh = native_effects::observe(root.clone()).await?;
            fresh.checkout_evidence = current.evidence(owner.epoch);
            crate::native_change_set::bind_task_files(&self.executor, &root, task, &mut fresh)
                .await?;
            let t = task.to_owned();
            let (fresh, fresh_bound) = recv(self.executor.read(move |db| {
                let d = completion::current_binding_digest(db, &t, &fresh).ok();
                Ok((fresh, d))
            })?)
            .await?;
            all_exits_observed &= receipt["observation"]["complete"] == true
                && receipt["observation"]["exit_code"].is_i64();
            if fresh_bound.as_deref() != Some(bound.as_str()) {
                // The reviewed bytes changed while the command ran: whatever it
                // observed (an unobserved exit is an unknown outcome, never a pass) does
                // not verify them. The run is settled and a fresh review binds the new
                // bytes.
                let outcome = if all_exits_observed {
                    "observed"
                } else {
                    "unknown"
                };
                let (t, o) = (task.to_owned(), owner.clone());
                recv(self.executor.write(move |db| {
                    ownership::check_tx(db,&t,&o)?;
                    lifecycle::request_tx(db,&t,"native-verifier-finish",&run,"native_verification_finished",
                        &json!({"last_output":key}),||Ok(json!({"settled":true,"reason":"verification_review_revision_changed","last_output":key,
                            "outcome":outcome,"settled_by":"runtime_reverify"})))?;
                    completion::review_changed_checkout_tx(db,&t,&fresh)
                })?).await?;
                return Ok(Verified::Recorded);
            }
            if let Some(reason) = environment {
                let (t, o, a) = (task.to_owned(), owner.clone(), json!(argv));
                let wait = recv(self.executor.write(move |db| {
                    ownership::check_tx(db, &t, &o)?;
                    lifecycle::request_tx(db,&t,"native-verifier-finish",&run,"native_verification_finished",
                        &json!({"last_output":key}),||Ok(json!({"settled":true,"reason":"verifier_environment","environment":reason,"last_output":key})))?;
                    completion::record_verification_wait_tx(db, &t, &a, reason, &json!(receipts), unix_now()?)
                })?)
                .await?;
                // After the 10-minute recheck the runtime asked the owner instead
                // (WAITING_USER): nothing more to wait for here.
                if wait["owner_question"].is_string() {
                    return Ok(Verified::Recorded);
                }
                let left = wait["retry_after_s"].as_u64().unwrap_or(60);
                return Ok(Verified::Waiting(std::time::Duration::from_secs(left)));
            }
            let t = task.to_owned();
            let o = owner.clone();
            keys.push(key.clone());
            recv(self.executor.write(move |db| {
                ownership::check_tx(db, &t, &o)?;
                let observed = CommandObservation {
                    complete: receipt["observation"]["complete"] == true,
                    exit_code: receipt["observation"]["exit_code"]
                        .as_i64()
                        .map(|v| v as i32),
                    receipt_ref: receipt["tool_call_ref"].as_str().unwrap().into(),
                };
                completion::retain_check_tx(db, &t, &o, &key, &argv, &observed, &fresh)?;
                Ok(())
            })?)
            .await?;
        }
        let t = task.to_owned();
        let o = owner.clone();
        recv(self.executor.write(move |db| {
            ownership::check_tx(db, &t, &o)?;
            completion::record_verification_tx(db, &t, &keys, &e)?;
            lifecycle::request_tx(
                db,
                &t,
                "native-verifier-finish",
                &run,
                "native_verification_finished",
                &json!({"keys":keys}),
                || Ok(json!({"settled":true})),
            )?;
            Ok(())
        })?)
        .await?;
        Ok(Verified::Recorded)
    }
    /// One run of one verification command under the run's intent: the receipt (its
    /// output retained under `key` as the runtime's own diagnostic) and whether its
    /// outcome was environmental (`completion::verifier_environment`), judged against
    /// the task's own worker runs of the same command on the same checkout.
    #[allow(clippy::too_many_arguments)]
    async fn verification_run(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        run: &str,
        key: &str,
        argv: &[String],
        bound: &str,
        digest: &Option<String>,
    ) -> CoreResult<(Value, Option<&'static str>)> {
        let mut receipt = match self.owner_verify(task, owner, argv).await {
            Ok(r) => r,
            Err(err) => {
                json!({"tool_call_ref":key,"argv":argv,"profile":"native-readonly-landlock-seccomp",
                "observation":{"complete":false,"failure":err.code(),"exit_code":null}})
            }
        };
        // The run binds what the verification binds; the checkout it ran in is
        // kept beside it.
        receipt["revision_digest"] = json!(bound);
        receipt["checkout_digest"] = json!(digest);
        receipt["owner_epoch"] = json!(owner.epoch);
        receipt["run"] = json!(run);
        let t = task.to_owned();
        let raw = receipt.clone();
        let run_key = run.to_owned();
        let output_key = key.to_owned();
        let digests = [bound.to_owned(), digest.clone().unwrap_or_default()];
        let argv = argv.to_vec();
        let environment = recv(self.executor.write(move|db|{
            // A command already dispatched under this intent is an observed
            // effect even if later state/epoch/project changes disqualify it.
            let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND actor='native-verifier' AND request_key=? AND operation='native_verification_intent')",(&t,&run_key),|r|r.get(0))?;
            if !exists{return Err(CoreError::Refused("verification_intent_missing"));}
            lifecycle::request_tx(db,&t,"native-verifier-output",&output_key,"native_command_output",&raw,||Ok(raw.clone()))?;
            let observation=&raw["observation"];
            let failed=observation["complete"]==true && observation["exit_code"].as_i64().is_some_and(|c|c!=0);
            let worker_passed=failed && completion::worker_passed_tx(db,&t,&argv,&[digests[0].as_str(),digests[1].as_str()],&raw)?;
            Ok(completion::verifier_environment(observation,worker_passed))
        })?).await?;
        Ok((receipt, environment))
    }
}

/// What one verification step left: an outcome recorded (passed, failed, or a
/// fresh review of changed bytes), or the runtime's own wait for the verifier's
/// environment with the time until its recheck.
pub(super) enum Verified {
    Recorded,
    Waiting(std::time::Duration),
}

fn unix_now() -> CoreResult<f64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| CoreError::Refused("clock_unavailable"))?
        .as_secs_f64())
}

/// D5: a native sprint must plan and run its tasks like single tasks do.
#[cfg(test)]
mod sprint_tests {
    use crate::{
        native_runtime::NativeRoutes, native_sessions::SessionRoute, Config, LaunchSpec, Service,
    };
    use cortex_core::CoreResult;
    use cortex_engine::environment::{EngineKind, LaunchSpec as EngineLaunch};
    use serde_json::{json, Value};
    use std::{
        collections::BTreeMap, os::unix::fs::PermissionsExt, path::Path, sync::Arc, time::Duration,
    };

    /// No-paid ACP peer (a POSIX shell script): a two-task plan only for a read-only planning turn.
    /// The deadline journey additionally gates its reply after two clarifications.
    fn peer_script() -> String {
        let plan = json!({"tasks": [
            {"key": "t1", "title": "First", "description": "Do the first part", "dependsOn": []},
            {"key": "t2", "title": "Second", "description": "Do the second part", "dependsOn": ["t1"]}]})
        .to_string();
        let plan = json!(plan).to_string();
        let items: Vec<_> = (1..=39)
            .map(|n| {
                let deps = if n > 1 {
                    vec![format!("t{}", n - 1)]
                } else {
                    vec![]
                };
                json!({"key":format!("t{n}"),"description":format!("Plan part {n}; verify its documented result."),"dependsOn":deps})
            })
            .collect();
        let large_plan = json!(json!({"tasks":items}).to_string());
        let detailed_items: Vec<_> = (1..=50)
            .map(|n| {
                let area = ["navigation", "editor", "activity", "search", "settings"][(n - 1) % 5];
                let description = format!(
                    "Implement the {area} behavior for scenario {n}. Record the current \
                     selection before applying the change, and preserve it when the user \
                     returns to this view or refreshes the underlying project. Keep the \
                     action available from the keyboard without changing the order of \
                     unrelated controls. Show the pending state beside the affected \
                     control, then replace it with the result or a readable reason. \
                     Acceptance: exercise a successful change and verify that the \
                     saved selection is restored after reopening the view. Cover a \
                     refusal with the selection intact, the previous content still \
                     visible, and a descriptive message that names the next available \
                     action. Check the same operation in a second project to verify \
                     that it does not reuse state from the first."
                );
                let dependencies: Vec<_> =
                    (n > 1).then(|| format!("t{}", n - 1)).into_iter().collect();
                json!({"key":format!("t{n}"), "title":format!("{area}: scenario {n}"),
                    "description":description, "dependsOn":dependencies})
            })
            .collect();
        let detailed_plan = json!({"tasks": detailed_items}).to_string();
        assert!(
            detailed_plan.chars().count() > 32_000,
            "fixture must exceed the old limit"
        );
        let detailed_plan = json!(detailed_plan);
        let first =
            json!(json!({"question":"Choose the scope?","options":["Two parts"]}).to_string());
        let second =
            json!(json!({"question":"Choose the order?","options":["Part one first"]}).to_string());
        let fixture = json!("Fixture coordinator.").to_string();
        let other = json!("not read-only").to_string();
        format!(
            r#"
mode=agent
config() {{
    printf '[{{"id":"model","type":"select","currentValue":"fixture-model","options":[{{"value":"fixture-model","name":"F"}}]}},{{"id":"mode","type":"select","currentValue":"%s","options":[{{"value":"agent","name":"agent"}},{{"value":"read-only","name":"read-only"}}]}}]' "$mode"
}}
update() {{
    printf '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"s1","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":%s}}}}}}}}\n' "$1"
}}
while IFS= read -r line; do
    id=${{line#*\"id\":}}
    id=${{id%%,*}}
    id=${{id%%\}}*}}
    case "$line" in
    *'"method":"initialize"'*)
        printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":1,"agentCapabilities":{{}}}}}}\n' "$id" ;;
    *'"method":"session/new"'*)
        printf '{{"jsonrpc":"2.0","id":%s,"result":{{"sessionId":"s1","configOptions":%s}}}}\n' "$id" "$(config)" ;;
    *'"method":"session/set_config_option"'*)
        case "$line" in
        *'"configId":"mode"'*)
            value=${{line#*\"value\":\"}}
            mode=${{value%%\"*}} ;;
        esac
        printf '{{"jsonrpc":"2.0","id":%s,"result":{{"configOptions":%s}}}}\n' "$id" "$(config)" ;;
    *'"method":"session/prompt"'*)
        case "$line" in
        *'Sprint owner load hold'*)
            printf 'held\n' >> "$0/load-entered"
            while [ ! -f "$0/load-release" ]; do /usr/bin/sleep .02; done
            update '{fixture}' ;;
        *'Sprint task t1'*)
            printf '%s\n' "$line" > "$0/.git/sprint-child-prompt"
            update '{fixture}' ;;
        *'Merge manifest:'*)
            printf '%s\n' "$line" >> "$0/.git/merge-handoff-prompts"
            while [ ! -f "$0/.git/release-merge-handoff" ]; do /usr/bin/sleep .02; done
            update '{fixture}' ;;
        *'Run a fresh independent review and runtime verification for the resolved task merge'*)
            printf '%s\n' "$line" >> "$0/.git/merge-fresh-check-prompts"
            update '{fixture}' ;;
        *'Automatic recovery. Inspect the current task'*)
            printf '%s\n' "$line" > "$0/.git/automatic-repair-prompt"
            printf 'repair\n' >> "$0/.git/automatic-repair-input"
            update '{fixture}' ;;
        *'You only plan user tasks'*)
            if [ "$mode" != read-only ]; then
                update '{other}'
            else
                case "$line" in
                *'Sprint deadline fixture'*)
                    case "$line" in
                    *'First retained answer'*'Second retained answer'*)
                        : > "$0/entered"
                        while [ ! -f "$0/release" ]; do /usr/bin/sleep .02; done
                        update '{large_plan}' ;;
                    *'First retained answer'*) update '{second}' ;;
                    *) update '{first}' ;;
                    esac ;;
                *'Sprint large plan fixture'*)
                    printf 'pass\n' >> "$0/passes"
                    update '{detailed_plan}' ;;
                *'Sprint compact retry fixture'*|*'Sprint oversized twice fixture'*)
                    printf 'pass\n' >> "$0/passes"
                    update '{detailed_plan}'
                    case "$line" in
                    *'Sprint compact retry fixture'*'planning reply exceeded'*) ;;
                    *)
                        padding=$(printf '%32768s' '')
                        for chunk in 1 2 3 4; do update "\"$padding\""; done ;;
                    esac ;;
                *) update '{plan}' ;;
                esac
            fi ;;
        *) update '{fixture}' ;;
        esac
        printf '{{"jsonrpc":"2.0","id":%s,"result":{{"stopReason":"end_turn"}}}}\n' "$id" ;;
    esac
done
"#
        )
    }
    pub(super) struct Peer(pub(super) Option<std::path::PathBuf>);
    impl NativeRoutes for Peer {
        fn route(&self, root: &Path, _: &str) -> CoreResult<SessionRoute> {
            if root.join(".git/refuse-merge-route").is_file() {
                return Err(cortex_core::CoreError::Refused("fixture_merge_route_refused"));
            }
            let load_route = root.join("load-route-enabled").is_file();
            Ok(SessionRoute {
                launch: EngineLaunch {
                    engine: EngineKind::Codex,
                    executable: "/bin/sh".into(),
                    args: vec![
                        "-c".into(),
                        peer_script(),
                        self.0.as_deref().unwrap_or(root).display().to_string(),
                    ],
                    cwd: root.into(),
                    native_home: root.into(),
                    credential_directory: root.into(),
                    base_environment: BTreeMap::new(),
                    codex_launcher: None,
                    confinement: None,
                },
                relay: "/bin/false".into(),
                broker_parent: root.join(if workspace_protocol::channel::current().is_dev() {
                    ".cortex-dev"
                } else {
                    ".cortex"
                }),
                connection: "fixture".into(),
                connection_limit: 4,
                model: if load_route {
                    "load-model".into()
                } else {
                    "fixture-model".into()
                },
                parameters: if load_route {
                    BTreeMap::from([("thought_level".into(), "high".into())])
                } else {
                    BTreeMap::new()
                },
                instructions: "No-paid fixture.".into(),
                deadline: Duration::from_secs(3),
            })
        }
    }
    pub(super) async fn call(service: &Service, op: &str, project: &str, params: Value) -> Value {
        let reply = service
            .machine(
                serde_json::from_value(json!({"op":op,"projectId":project,"params":params}))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(reply["ok"], true, "{op}: {reply}");
        reply["result"].clone()
    }
    pub(super) async fn sprint(service: &Service, project: &str, id: &Value) -> Value {
        call(service, "product.board", project, json!({})).await["sprints"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == *id)
            .unwrap()
            .clone()
    }
    async fn executions(service: &Service, project: &str) -> BTreeMap<String, bool> {
        let runtime = service
            .state
            .lock()
            .await
            .native
            .get(project)
            .unwrap()
            .clone();
        runtime
            .host
            .executor
            .read(|db| {
                let rows = db
                    .prepare("SELECT task_id,execution_id IS NOT NULL FROM workspace_tasks")?
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?
                    .collect::<Result<BTreeMap<_, _>, _>>()?;
                Ok(rows)
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap()
    }
    async fn wait<F: std::future::Future<Output = bool>>(what: &str, mut check: impl FnMut() -> F) {
        for _ in 0..240 {
            if check().await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
        panic!("timed out waiting for {what}");
    }
    fn create(project: &str, request: &str) -> Value {
        json!({"protocol":{"major":1,"minor":8},"projectId":project,"actorId":"forged","requestId":request,
            "operation":"tasks.create","target":project,"expectedVersion":"0","ownerEpoch":null,
            "payload":{"description":format!("Sprint task {request}"),"checkout":{"kind":"main"}}})
    }

    fn rule(tasks: Value) -> (&'static str, String) {
        let e = super::sprints::validate_tasks(&tasks).unwrap_err();
        (e.code, e.detail)
    }

    #[test]
    fn sprint_plan_rules_keep_only_content_fields_and_are_lenient_where_unambiguous() {
        use super::sprints::validate_tasks;
        let kept =
            validate_tasks(&json!([{"key":"a","description":"Do A","dependsOn":[],"extra":1}]))
                .unwrap();
        assert_eq!(
            kept,
            vec![json!({"key":"a","description":"Do A","dependsOn":[]})]
        );
        let missing = validate_tasks(&json!([{"key":"a","description":"A"}])).unwrap();
        assert_eq!(missing[0]["dependsOn"], json!([]));
        assert!(missing[0].get("complexity").is_none());
        let complex =
            validate_tasks(&json!([{"key":"a","description":"A","complexity":"high"}])).unwrap();
        assert_eq!(complex[0]["complexity"], "high");
        assert_eq!(
            rule(json!([{"key":"a","description":"A","complexity":"huge"}])).1,
            "task 1 (a): complexity must be low, normal or high"
        );
        let single = validate_tasks(
            &json!([{"key":"a","description":"A"},{"key":"b","description":"B","dependsOn":"a"}]),
        )
        .unwrap();
        assert_eq!(single[1]["dependsOn"], json!(["a"]));
        let title = validate_tasks(&json!([{"key":"a","title":"ж".repeat(300),"description":"A"}]))
            .unwrap()[0]["title"]
            .as_str()
            .unwrap()
            .to_owned();
        assert_eq!(title.chars().count(), 200);
        assert!(title.ends_with('…'));
        // A plan has no item count limit; only the whole plan's bytes are bounded.
        let items = |count: usize, size: usize| -> Value {
            (1..=count).map(|n| json!({"key":format!("t{n}"),"title":format!("Part {n}"),
                "description":"d".repeat(size),"dependsOn":if n > 1 { json!([format!("t{}", n - 1)]) } else { json!([]) }}))
                .collect::<Vec<_>>().into()
        };
        assert_eq!(validate_tasks(&items(400, 700)).unwrap().len(), 400);
        assert_eq!(rule(items(120, 5000)).0, "plan_too_large");
        // An incomplete plan keeps item rules and unique keys but leaves references for later parts.
        let open = json!([{"key":"a","description":"A","dependsOn":["later"]}]);
        assert!(super::sprints::check_tasks(&open, false).is_ok());
        assert_eq!(rule(open).0, "invalid_dependencies");
    }

    #[test]
    fn sprint_plan_rules_name_the_failing_task_and_rule() {
        let ok = |k: &str| json!({"key":k,"description":"D"});
        assert_eq!(
            rule(json!([ok("t1"), ok("t2"), {"key":"t3","description":"  "}])),
            ("invalid_task", "task 3 (t3): description is empty".into())
        );
        assert_eq!(
            rule(json!([ok("t1"), {"key":"t2"}])).1,
            "task 2 (t2): description is missing or not text"
        );
        assert_eq!(
            rule(json!([{"description":"D"}])).1,
            "task 1: key must be text of 1 to 100 characters"
        );
        assert_eq!(rule(json!([7])).1, "task 1: not an object");
        assert_eq!(
            rule(json!([ok("t1"), ok("t1")])).1,
            "task 2 (t1): duplicate key"
        );
        assert_eq!(
            rule(json!([{"key":"t1","description":"D","dependsOn":[1]}])).1,
            "task 1 (t1): dependsOn must be a list of task keys"
        );
        assert_eq!(
            rule(json!([{"key":"t1","description":"D","title":5}])).1,
            "task 1 (t1): title must be text"
        );
        assert_eq!(
            rule(json!([ok("t1"), {"key":"t2","description":"D","dependsOn":["t12"]}])),
            (
                "invalid_dependencies",
                "task 2 (t2) depends on unknown key t12".into()
            )
        );
        assert_eq!(
            rule(json!([{"key":"t1","description":"D","dependsOn":["t1"]}])).0,
            "invalid_dependencies"
        );
        assert_eq!(
            rule(
                json!([{"key":"a","description":"A","dependsOn":["b"]},{"key":"b","description":"B","dependsOn":["a"]}])
            ),
            (
                "invalid_dependencies",
                "task 1 (a) is part of or blocked by a dependency cycle".into()
            )
        );
        assert_eq!(
            rule(json!([])),
            ("tasks_required", "tasks: the list is empty".into())
        );
        assert_eq!(rule(json!({"x":1})).1, "tasks: missing or not a list");
    }

    /// The advertised schema and the runtime rules agree: the tool is offered to the
    /// coordinator only, and its schema accepts what the rules accept.
    #[test]
    fn sprint_plan_submit_is_a_coordinator_tool_with_the_plan_contract() {
        let coordinator = cortex_engine::schema::catalogue("coordinator", None, None).unwrap();
        let tool = coordinator
            .iter()
            .find(|t| t["name"] == "sprint_plan_submit")
            .expect("advertised");
        let args = json!({"request_key":"k","tasks":[{"key":"t1","title":"T","description":"D","dependsOn":[]}]});
        assert!(cortex_engine::schema::validate(&tool["inputSchema"], &args).is_ok());
        assert!(cortex_engine::schema::validate(
            &tool["inputSchema"],
            &json!({"request_key":"k","tasks":[]})
        )
        .is_err());
        assert!(super::sprints::validate_tasks(&args["tasks"]).is_ok());
        for profile in ["worker-explorer", "worker-backend-dev"] {
            let worker =
                cortex_engine::schema::catalogue("worker", Some(profile), Some("execution"))
                    .unwrap();
            assert!(
                !worker.iter().any(|t| t["name"] == "sprint_plan_submit"),
                "{profile}"
            );
        }
    }

    /// Cross-task reads are coordinator-only and expose bounded cursors. Workers
    /// must continue to use their assignment-local report tools.
    #[test]
    fn sprint_cross_task_reads_are_coordinator_only() {
        let coordinator = cortex_engine::schema::catalogue("coordinator", None, None).unwrap();
        let graph = coordinator
            .iter()
            .find(|t| t["name"] == "sprint_graph")
            .expect("sprint_graph advertised");
        assert!(
            cortex_engine::schema::validate(&graph["inputSchema"], &json!({"limit": 20})).is_ok()
        );
        let result = coordinator
            .iter()
            .find(|t| t["name"] == "sprint_task_result")
            .expect("sprint_task_result advertised");
        assert!(cortex_engine::schema::validate(
            &result["inputSchema"],
            &json!({"member_id": "task-a"})
        )
        .is_ok());
        assert!(cortex_engine::schema::validate(&result["inputSchema"], &json!({})).is_err());
        for profile in ["worker-explorer", "worker-backend-dev"] {
            let worker =
                cortex_engine::schema::catalogue("worker", Some(profile), Some("execution"))
                    .unwrap();
            assert!(!worker.iter().any(|t| t["name"] == "sprint_graph"));
            assert!(!worker.iter().any(|t| t["name"] == "sprint_task_result"));
        }
    }
    #[tokio::test(flavor = "multi_thread")]
    async fn native_sprint_plans_read_only_and_runs_its_queue_in_dependency_order() {
        let root = std::env::temp_dir().join(format!("ns-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let folder = root.join("project");
        std::fs::create_dir(&folder).unwrap();
        assert!(std::process::Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(&folder)
            .status()
            .unwrap()
            .success());
        let service = Service::bind_native(
            Config {
                runtime_dir: root.join("runtime"),
                state_dir: root.join("state"),
                allowed_root: root.clone(),
                launch: LaunchSpec::bare(),
                request_timeout: Duration::from_secs(5),
                shutdown_timeout: Duration::from_secs(2),
                event_capacity: 64,
            },
            Arc::new(Peer(None)),
        )
        .await
        .unwrap();
        let registered = service.register(&folder, "Sprint").await.unwrap();
        let id = registered["projectId"].as_str().unwrap().to_owned();
        service.activate(&id).await.unwrap();
        let sprint_params = |text: &str| json!({"action":"create","description":text,"runMode":"hybrid","checkout":{"kind":"main"}});
        // Planning claimed before the service (re)started is never silently rerun.
        let stale = call(
            &service,
            "product.sprint",
            &id,
            sprint_params("Planned before restart"),
        )
        .await["sprintId"]
            .clone();
        let runner = tokio::spawn(service.clone().serve());
        wait("the interrupted planning to be cancelled", || async {
            sprint(&service, &id, &stale).await["planning"] == "cancelled"
        })
        .await;
        assert!(sprint(&service, &id, &stale).await["conversation"]
            .to_string()
            .contains("Planning was interrupted"));
        // A sprint created while the service runs is planned by a read-only planning task.
        let fresh = call(
            &service,
            "product.sprint",
            &id,
            sprint_params("Build the two parts"),
        )
        .await["sprintId"]
            .clone();
        wait("the planning task to start", || async {
            sprint(&service, &id, &fresh).await["planningTaskId"].is_string()
        })
        .await;
        let planning = sprint(&service, &id, &fresh).await["planningTaskId"]
            .as_str()
            .unwrap()
            .to_owned();
        // No-stop policy: a turn that ends without a plan is a continuation (RECOVERING).
        wait("the planning coordinator's turn to settle", || async {
            let runtime = service.state.lock().await.native.get(&id).unwrap().clone();
            let t = planning.clone();
            runtime.host.executor.read(move |db| Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id
                 JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE w.task_id=?
                 AND r.state IN ('WAITING_USER','FAILED','CANCELLED','BLOCKED','RECOVERING')
                 AND NOT EXISTS(SELECT 1 FROM capacity_reservations c WHERE c.task_id=r.task_id AND c.state!='released'))",
                [&t], |r| r.get::<_, bool>(0))?)).unwrap().await.unwrap().unwrap()
        })
        .await;
        // The coordinator's reviewed plan (sprint_plan_submit) and the task's completion.
        let runtime = service.state.lock().await.native.get(&id).unwrap().clone();
        let t = planning.clone();
        runtime.host.executor.write(move |db| {
            let task: String = db.query_row(
                "SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id WHERE w.task_id=?",
                [&t], |r| r.get(0))?;
            assert!(cortex_core::lifecycle::sprint_planning::enabled(db, &task)?);
            cortex_core::lifecycle::event_tx(db, &task, "SPRINT_PLAN_SUBMITTED", json!({"revision":0,"tasks":[
                {"key":"t1","title":"First","description":"Do the first part","dependsOn":[]},
                {"key":"t2","title":"Second","description":"Do the second part","dependsOn":["t1"]}]}))?;
            db.execute("UPDATE pending_questions SET status='cancelled' WHERE task_id=? AND status='open'", [&task])?;
            db.execute("UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL,synthesis='Planned.' WHERE task_id=?", [&task])?;
            cortex_core::lifecycle::event_tx(db, &task, "TASK_STATE_CHANGED", json!({"state":"COMPLETED"}))?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
        drop(runtime);
        wait("the planning task's plan to be proposed", || async {
            sprint(&service, &id, &fresh).await["planning"] == "ready"
        })
        .await;
        let planned = sprint(&service, &id, &fresh).await;
        assert_eq!(planned["plan"]["version"], 1);
        let items = planned["plan"]["items"].as_array().unwrap().clone();
        assert_eq!(items.len(), 2);
        assert_eq!(items[1]["dependsOn"], json!(["t1"]));
        // Materialize exactly like the desktop: create, join the sprint, then run it.
        let mut created: Vec<Value> = Vec::new();
        for (n, item) in items.iter().enumerate() {
            let task = service
                .workspace(create(&id, &format!("t{n}")))
                .await
                .unwrap()["target"]
                .clone();
            let deps: Vec<Value> = item["dependsOn"]
                .as_array()
                .unwrap()
                .iter()
                .map(|_| created[0].clone())
                .collect();
            call(
                &service,
                "product.task",
                &id,
                json!({"action":"sprint","taskId":task,"sprintId":fresh,"dependencies":deps}),
            )
            .await;
            created.push(task);
        }
        call(
            &service,
            "product.sprint",
            &id,
            json!({"action":"patch","sprintId":fresh,
            "patch":{"planning":"created","materializedVersion":1,"taskIds":created}}),
        )
        .await;
        call(
            &service,
            "product.sprint",
            &id,
            json!({"action":"patch","sprintId":fresh,
            "patch":{"queueState":"running","pauseReason":null}}),
        )
        .await;
        let (first, second) = (
            created[0].as_str().unwrap().to_owned(),
            created[1].as_str().unwrap().to_owned(),
        );
        wait("the queue to start the first task", || async {
            executions(&service, &id).await.get(&first) == Some(&true)
        })
        .await;
        assert_eq!(
            executions(&service, &id).await.get(&second),
            Some(&false),
            "dependency started early"
        );
        // Settle the first task as a completed answer; acceptance advances the queue.
        wait("the first task's coordinator turns to settle", || async {
            let runtime = service.state.lock().await.native.get(&id).unwrap().clone();
            runtime.host.executor.read(|db| Ok(db.query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM capacity_reservations WHERE state!='released')
                 AND EXISTS(SELECT 1 FROM runtime_tasks WHERE state IN ('WAITING_USER','FAILED','CANCELLED','BLOCKED','RECOVERING'))",
                [], |r| r.get::<_, bool>(0))?)).unwrap().await.unwrap().unwrap()
        })
        .await;
        let runtime = service.state.lock().await.native.get(&id).unwrap().clone();
        let t = first.clone();
        runtime.host.executor.write(move |db| {
            let runtime_task: String = db.query_row(
                "SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id WHERE w.task_id=?",
                [&t], |r| r.get(0))?;
            db.execute("UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL,synthesis='Queue fixture answer.' WHERE task_id=?", [&runtime_task])?;
            cortex_core::lifecycle::event_tx(db, &runtime_task, "ANSWER_SUBMITTED", json!({"text":"Queue fixture answer."}))?;
            cortex_core::lifecycle::event_tx(db, &runtime_task, "VERIFICATION_PASSED", json!({"fixture":true}))?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
        wait("the queue to start the dependent task", || async {
            executions(&service, &id).await.get(&second) == Some(&true)
        })
        .await;
        wait("the dependent prompt to be recorded", || async {
            folder.join(".git/sprint-child-prompt").is_file()
        })
        .await;
        let child_prompt =
            std::fs::read_to_string(folder.join(".git/sprint-child-prompt")).unwrap();
        assert!(
            child_prompt.contains(&first),
            "dependent prompt omitted predecessor {first}"
        );
        let board = call(&service, "product.board", &id, json!({})).await;
        assert_eq!(board["tasks"][first.as_str()]["status"], "ready_to_merge");
        assert_eq!(
            board["tasks"][first.as_str()]["acceptance"]["automatic"],
            true
        );
        service.stop().await.unwrap();
        let _ = runner.await;
        drop(runtime);
        drop(service);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn service_runtime_binds_cross_task_graph_to_the_authenticated_member() {
        let root = std::env::temp_dir().join(format!("ns-graph-{}", crate::identity().unwrap()));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let folder = root.join("project");
        std::fs::create_dir(&folder).unwrap();
        assert!(std::process::Command::new("/usr/bin/git")
            .args(["init", "-q"])
            .current_dir(&folder)
            .status()
            .unwrap()
            .success());
        let service = Service::bind_native(
            Config {
                runtime_dir: root.join("runtime"),
                state_dir: root.join("state"),
                allowed_root: root.clone(),
                launch: LaunchSpec::bare(),
                request_timeout: Duration::from_secs(5),
                shutdown_timeout: Duration::from_secs(2),
                event_capacity: 64,
            },
            Arc::new(Peer(None)),
        )
        .await
        .unwrap();
        let id = service.register(&folder, "Graph").await.unwrap()["projectId"]
            .as_str()
            .unwrap()
            .to_owned();
        service.activate(&id).await.unwrap();
        let sprint_id = call(&service, "product.sprint", &id, json!({"action":"create","description":"Graph fixture","runMode":"manual","checkout":{"kind":"main"}})).await["sprintId"].clone();
        let a = service.workspace(create(&id, "graph-a")).await.unwrap()["target"].clone();
        let b = service.workspace(create(&id, "graph-b")).await.unwrap()["target"].clone();
        call(
            &service,
            "product.task",
            &id,
            json!({"action":"sprint","taskId":a,"sprintId":sprint_id,"dependencies":[]}),
        )
        .await;
        call(
            &service,
            "product.task",
            &id,
            json!({"action":"sprint","taskId":b,"sprintId":sprint_id,"dependencies":[a]}),
        )
        .await;
        call(&service, "product.sprint", &id, json!({"action":"patch","sprintId":sprint_id,"patch":{"planning":"created","materializedVersion":1,"taskIds":[a,b]}})).await;
        let runtime = service.state.lock().await.native.get(&id).unwrap().clone();
        let request = json!({"protocol":{"major":1,"minor":8},"requestId":"graph-start","projectId":id,"actorId":"forged","operation":"tasks.start","target":a,"expectedVersion":"1","ownerEpoch":null,"payload":{}});
        let start = service.workspace(request).await.unwrap();
        assert_eq!(start["outcome"], "recorded", "start refused: {start}");
        let runtime_task = runtime
            .host
            .executor
            .read({
                let a = a.as_str().unwrap().to_owned();
                move |db| {
                    Ok(db.query_row(
                        "SELECT runtime_task_id FROM workspace_executions WHERE task_id=?",
                        [&a],
                        |r| r.get::<_, String>(0),
                    )?)
                }
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let state = root.join("state");
        let graph_result = runtime
            .host
            .executor
            .read({
                let state = state.clone();
                let runtime_task = runtime_task.clone();
                move |db| {
                    let scope =
                        crate::machine::sprint_graph::Scope::load(db, &state, &runtime_task)?
                            .expect("sprint scope");
                    let graph = scope.graph(&json!({"limit":10}))?;
                    let foreign = scope.member("outside").unwrap_err().code();
                    Ok((graph, foreign.to_owned()))
                }
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let graph = graph_result.0;
        assert_eq!(graph["tasks"].as_array().unwrap().len(), 2);
        assert_eq!(graph_result.1, "task_outside_sprint");
        service.stop().await.unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    // Keep Automatic repair classification scenarios in their own file so the
    // scheduler tests and the classifier can evolve independently.
    #[cfg(test)]
    pub(crate) mod automatic_repair_tests {
        include!("automatic_repair_tests.rs");
    }
}

/// Native sprint planning and queue. Python ran both in the project child's
/// queue loop (the retired Python launcher's planning and scheduler ticks); a native
/// project has no child, so without this nothing ever planned or ran a sprint.
/// Sprint records are read and changed only through the owner's product calls.
pub(crate) mod sprints {
    use crate::{actor, machine::sprint_live::{self, Snapshot}, native_runtime::Runtime, Service};
    use serde_json::{json, Value};
    use std::{
        collections::{BTreeSet, HashMap},
        io,
        sync::Arc,
        time::Duration,
    };

    const PASS_EVERY: Duration = Duration::from_secs(2);
    // A plan has no item count limit; it arrives in parts. Each
    // `sprint_plan_submit` call carries at most 128 KiB of UTF-8 JSON, and the
    // whole assembled plan at most 512 KiB: about a thousand items of 500
    // characters, kept in one bounded review prompt and well inside the 8 MiB
    // Workspace frame limit (shared with history and other state).
    pub(crate) const MAX_PART_BYTES: usize = 128 * 1024;
    const MAX_PLAN_BYTES: usize = 512 * 1024;
    /// The draft description limit of `tasks.create`.
    const MAX_REQUEST_CHARS: usize = 32_000;

    /// A rejected plan: a stable `code` plus a short owner-safe `detail`
    /// naming the failing task (1-based index and sanitized key) and rule. The
    /// detail never contains raw model prose.
    #[derive(Debug, PartialEq, Clone)]
    pub(crate) struct PlanError {
        pub(crate) code: &'static str,
        pub(crate) detail: String,
    }
    fn fail(code: &'static str, detail: impl Into<String>) -> PlanError {
        PlanError {
            code,
            detail: detail.into(),
        }
    }
    /// Model-chosen keys are shown only in a short, restricted form.
    fn safe_key(key: &str) -> String {
        key.chars()
            .filter(|c| c.is_alphanumeric() || "-_.:".contains(*c))
            .take(40)
            .collect()
    }
    fn task_label(index: usize, item: &Value) -> String {
        match item["key"].as_str().map(safe_key).filter(|k| !k.is_empty()) {
            Some(key) => format!("task {index} ({key})"),
            None => format!("task {index}"),
        }
    }

    /// The sprint plan rules, shared by `sprint_plan_submit` and the sprint. Only
    /// the plan's content fields survive.
    pub(crate) fn validate_tasks(value: &Value) -> Result<Vec<Value>, PlanError> {
        check_tasks(value, true)
    }
    /// The same rules over a plan assembled so far: every item's own rules and
    /// unique keys always; dependencies, cycles and the whole-plan size only once
    /// the plan is `complete` (a later part may define a key an earlier one names).
    pub(crate) fn check_tasks(value: &Value, complete: bool) -> Result<Vec<Value>, PlanError> {
        let items = value
            .as_array()
            .ok_or_else(|| fail("tasks_required", "tasks: missing or not a list"))?;
        if items.is_empty() {
            return Err(fail("tasks_required", "tasks: the list is empty"));
        }
        let mut keys = BTreeSet::new();
        let mut kept = Vec::new();
        for (n, item) in items.iter().enumerate() {
            let label = task_label(n + 1, item);
            if !item.is_object() {
                return Err(fail("invalid_task", format!("{label}: not an object")));
            }
            let key = match item.get("key").and_then(Value::as_str) {
                Some(k) if (1..=100).contains(&k.chars().count()) => k,
                _ => {
                    return Err(fail(
                        "invalid_task",
                        format!("{label}: key must be text of 1 to 100 characters"),
                    ))
                }
            };
            if !keys.insert(key.to_owned()) {
                return Err(fail("invalid_task", format!("{label}: duplicate key")));
            }
            match item.get("description").and_then(Value::as_str) {
                Some(d) if d.trim().is_empty() => {
                    return Err(fail(
                        "invalid_task",
                        format!("{label}: description is empty"),
                    ))
                }
                Some(d) if d.trim().chars().count() > MAX_REQUEST_CHARS => {
                    return Err(fail(
                        "invalid_task",
                        format!("{label}: description is longer than 32000 characters"),
                    ))
                }
                Some(_) => {}
                None => {
                    return Err(fail(
                        "invalid_task",
                        format!("{label}: description is missing or not text"),
                    ))
                }
            }
            let deps: Vec<Value> = match item.get("dependsOn") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::String(s)) => vec![Value::String(s.clone())],
                Some(Value::Array(a)) if a.iter().all(Value::is_string) => a.clone(),
                Some(_) => {
                    return Err(fail(
                        "invalid_task",
                        format!("{label}: dependsOn must be a list of task keys"),
                    ))
                }
            };
            let title = match item.get("title") {
                None | Some(Value::Null) => None,
                Some(Value::String(t)) if t.chars().count() > 200 => {
                    let mut cut: String = t.chars().take(199).collect();
                    cut.push('…');
                    Some(Value::String(cut))
                }
                Some(t @ Value::String(_)) => Some(t.clone()),
                Some(_) => {
                    return Err(fail("invalid_task", format!("{label}: title must be text")))
                }
            };
            let complexity = match item.get("complexity") {
                None | Some(Value::Null) => None,
                Some(Value::String(c)) if ["low", "normal", "high"].contains(&c.as_str()) => {
                    Some(json!(c))
                }
                Some(_) => {
                    return Err(fail(
                        "invalid_task",
                        format!("{label}: complexity must be low, normal or high"),
                    ))
                }
            };
            let mut clean = serde_json::Map::new();
            clean.insert("key".into(), json!(key));
            if let Some(title) = title {
                clean.insert("title".into(), title);
            }
            clean.insert("description".into(), item["description"].clone());
            clean.insert("dependsOn".into(), Value::Array(deps));
            if let Some(complexity) = complexity {
                clean.insert("complexity".into(), complexity);
            }
            kept.push(Value::Object(clean));
        }
        if json!(kept).to_string().len() > MAX_PLAN_BYTES {
            return Err(fail(
                "plan_too_large",
                format!("the whole plan exceeds {MAX_PLAN_BYTES} UTF-8 bytes; use shorter task descriptions without dropping required scope, acceptance criteria or owner decisions, or split the work into smaller sprints"),
            ));
        }
        if !complete {
            return Ok(kept);
        }
        for (n, item) in kept.iter().enumerate() {
            for d in item["dependsOn"].as_array().unwrap() {
                let d = d.as_str().unwrap();
                if !keys.contains(d) {
                    return Err(fail(
                        "invalid_dependencies",
                        format!(
                            "{} depends on unknown key {}",
                            task_label(n + 1, item),
                            safe_key(d)
                        ),
                    ));
                }
            }
        }
        let mut settled = BTreeSet::new();
        let mut pending: Vec<(usize, &Value)> = kept.iter().enumerate().collect();
        while !pending.is_empty() {
            let (ready, rest): (Vec<_>, Vec<_>) = pending.into_iter().partition(|(_, i)| {
                i["dependsOn"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|d| settled.contains(d.as_str().unwrap()))
            });
            if ready.is_empty() {
                let (n, item) = rest[0];
                return Err(fail(
                    "invalid_dependencies",
                    format!(
                        "{} is part of or blocked by a dependency cycle",
                        task_label(n + 1, item)
                    ),
                ));
            }
            for (_, item) in ready {
                settled.insert(item["key"].as_str().unwrap().to_owned());
            }
            pending = rest;
        }
        Ok(kept)
    }
    /// Owner-visible text: a stable code and the short precise reason.
    fn parse_failure(error: &PlanError) -> String {
        match error.code {
            "requirements_too_large" => "The plan and original owner requirements exceed the task description limit. Shorten the sprint input without dropping required decisions.".to_owned(),
            "plan_too_large" => "The plan was too long. If this repeats, split the work into smaller sprints.".to_owned(),
            code => format!("The planning task submitted an invalid plan ({code}): {}.", error.detail),
        }
    }

    /// What the planning task is asked to do. The sprint description and the
    /// conversation are the owner's request; the plan's shape is the advertised
    /// `sprint_plan_submit` schema, never restated here.
    pub(crate) fn planning_request(
        sprint: &Value,
        retry: Option<PlanningFailure>,
    ) -> Result<String, String> {
        let language = match sprint["settings"]["language"].as_str() {
            Some("ru") => "Russian",
            Some("en") => "English",
            _ => "the language of the description",
        };
        let mut text = format!(
            "Sprint planning (read-only). Study this project and plan the sprint described below.\n\
             - This task is read-only: read and inspect the project, run read-only discovery and, \
             when the study needs it, plan only read-only specialists such as explorer, architect, \
             planner or auditors. Change no file and implement nothing.\n\
             - Ask the owner with your question tool when an essential decision is missing, and \
             always when the description explicitly requires a question before implementation. \
             Never decide on the owner's behalf.\n\
             - Base the plan on what you found and submit it with sprint_plan_submit. For a large \
             plan, submit small ordered parts as you work (part 1, 2, and so on). Use final:false \
             on each intermediate part and final:true on the last. Follow the tool's advertised \
             limits. Keep each item compact while \
             preserving scope, dependencies, owner decisions and acceptance criteria. Do not echo \
             the full plan in prose.\n\
             - The runtime verifies and independently reviews the plan; after approval finish the \
             task. The owner then approves the plan and creates the tasks.\n\
             - Write in {language}."
        );
        if let Some(limit) = retry {
            text.push_str(&format!(
                "\n- Retry strategy: {} Start with small ordered sprint_plan_submit parts from \
                 the outset, even if the whole plan might fit in one call. Submit the first part \
                 as soon as its items are ready; do not wait to draft the whole plan. Keep each \
                 item compact without dropping requirements.",
                limit.reason()
            ));
        }
        text.push_str("\n\nSprint description:\n");
        text.push_str(sprint["description"].as_str().unwrap_or(""));
        let earlier: Vec<String> = sprint["conversation"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let who = match c["kind"].as_str() {
                    Some("question") => "Question",
                    Some("answer") => "Owner answer",
                    Some("message") => "Owner message",
                    _ => return None,
                };
                c["text"].as_str().map(|t| format!("{who}: {t}"))
            })
            .collect();
        if !earlier.is_empty() {
            text.push_str("\n\nEarlier sprint conversation:\n");
            text.push_str(&earlier.join("\n"));
        }
        let length = text.chars().count();
        if length > MAX_REQUEST_CHARS {
            return Err(format!("The planning request is too long ({length} characters; limit {MAX_REQUEST_CHARS}, including instructions and saved conversation). Shorten the sprint description before retrying."));
        }
        Ok(text)
    }
    /// The sprint's saved files, as the draft's own attachments.
    fn planning_files(sprint: &Value) -> Vec<Value> {
        sprint["attachments"]
            .as_array()
            .into_iter()
            .flatten()
            .chain(
                sprint["conversation"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .flat_map(|c| c["attachments"].as_array().into_iter().flatten()),
            )
            .filter_map(|a| {
                Some(
                    json!({"name":a["name"].as_str()?,"mimeType":a["mimeType"].as_str()?,
                    "data":a["data"].as_str()?}),
                )
            })
            .collect()
    }
    /// A sole planned task keeps the original owner requirements as a safeguard
    /// against a shortened summary; oversized combined descriptions refuse.
    pub(crate) fn finalize_items(
        sprint: &Value,
        mut items: Vec<Value>,
    ) -> Result<Vec<Value>, String> {
        if items.len() == 1 {
            let mut context = format!("\n\nOriginal owner requirements (take precedence over this task summary; never decide an explicitly requested owner question yourself; ask it before implementation unless the owner already answered):\n{}",
                sprint["description"].as_str().unwrap_or(""));
            let follow: Vec<&str> = sprint["conversation"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|c| c["kind"] == "answer" || c["kind"] == "message")
                .filter_map(|c| c["text"].as_str())
                .collect();
            if !follow.is_empty() {
                context.push_str(&format!("\nOwner follow-up:\n{}", follow.join("\n")));
            }
            let description = format!("{}{context}", items[0]["description"].as_str().unwrap());
            if description.chars().count() > MAX_REQUEST_CHARS {
                return Err(parse_failure(&fail("requirements_too_large", "")));
            }
            items[0]["description"] = json!(description);
        }
        Ok(items)
    }

    /// Only exact runtime codes enter this allowlist; engine text and generic
    /// cancellation/refusal codes never supply a cause.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    pub(crate) enum PlanningFailure {
        OwnerStopped,
        PartSize,
        TotalSize,
        CapacityTimeout,
        StepLimit,
        NoProgressLimit,
        #[default]
        Unknown,
    }

    impl PlanningFailure {
        pub(crate) fn code(self) -> &'static str {
            match self {
                Self::OwnerStopped => "planning_stopped_by_owner",
                Self::PartSize => "plan_part_too_large",
                Self::TotalSize => "plan_too_large",
                Self::CapacityTimeout => "scheduler_capacity_timeout",
                Self::StepLimit => "native_progress_limit",
                Self::NoProgressLimit => "coordinator_no_progress_limit",
                Self::Unknown => "planning_task_stopped",
            }
        }

        pub(crate) fn kind(self) -> &'static str {
            match self {
                Self::OwnerStopped => "owner_stopped",
                Self::Unknown => "unknown",
                _ => "limit",
            }
        }

        pub(crate) fn reason(self) -> &'static str {
            match self {
                Self::OwnerStopped => "You stopped the planning task.",
                Self::PartSize => "The planning task ended after a plan part was rejected for exceeding the size limit.",
                Self::TotalSize => "The planning task ended after the assembled plan was rejected for exceeding the total size limit.",
                Self::CapacityTimeout => "The planning task timed out waiting for runtime capacity.",
                Self::StepLimit => "The planning task reached the runtime's step limit.",
                Self::NoProgressLimit => "The planning task reached the runtime's limit for turns without progress.",
                Self::Unknown => "The planning task ended without a known cause.",
            }
        }

        pub(crate) fn is_limit(self) -> bool {
            self.kind() == "limit"
        }
    }

    fn recorded_planning_failure(
        db: &rusqlite::Connection,
        task: &str,
        state: &str,
        blocker: Option<&str>,
    ) -> cortex_core::CoreResult<PlanningFailure> {
        use rusqlite::OptionalExtension;
        let transition: Option<(i64, Option<String>)> = db.query_row(
            "SELECT id,json_extract(payload_json,'$.blocker') FROM runtime_events
             WHERE id=(SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_STATE_CHANGED')
             AND actor='runtime' AND json_extract(payload_json,'$.state')=?2",
            (task, state),
            |r| Ok((r.get(0)?, r.get(1)?)),
        ).optional()?;
        let Some((event, recorded)) = transition.filter(|(_, code)| code.as_deref() == blocker) else {
            return Ok(PlanningFailure::Unknown);
        };
        Ok(match recorded.as_deref() {
            Some("owner_resolved_without_certification" | "user_force_finished") if state == "CANCELLED" => {
                PlanningFailure::OwnerStopped
            }
            Some("scheduler_capacity_timeout") => PlanningFailure::CapacityTimeout,
            Some("native_progress_limit") => PlanningFailure::StepLimit,
            Some("coordinator_no_progress_limit") => PlanningFailure::NoProgressLimit,
            // Generic cancellation carries no owner attribution. Only use a
            // size refusal that was not superseded in the final planning phase.
            None | Some("cancelled") => {
                let code: Option<String> = db.query_row(
                    "SELECT CASE WHEN type=?3 THEN json_extract(payload_json,'$.code') END
                     FROM runtime_events
                     WHERE task_id=?1 AND id<?2 AND actor='runtime' AND type IN (
                         ?3,?4,?5,'TASK_STATE_CHANGED','NATIVE_TURN_DEGENERATE','USER_ANSWER_RECEIVED'
                     )
                     ORDER BY id DESC LIMIT 1",
                    (task, event, crate::native_sprint_plan::REJECTED,
                        cortex_core::lifecycle::sprint_planning::PART,
                        cortex_core::lifecycle::sprint_planning::SUBMITTED),
                    |r| r.get(0),
                ).optional()?.flatten();
                match code.as_deref() {
                    Some("plan_part_too_large") => PlanningFailure::PartSize,
                    Some("plan_too_large") => PlanningFailure::TotalSize,
                    _ => PlanningFailure::Unknown,
                }
            }
            _ => PlanningFailure::Unknown,
        })
    }

    /// The runtime facts of a planning task's current execution.
    #[derive(Debug, Default, Clone)]
    pub(crate) struct PlanningFact {
        pub(crate) execution: Option<String>,
        pub(crate) runtime_task: Option<String>,
        pub(crate) state: Option<String>,
        failure: PlanningFailure,
        /// The event that completed the current run (or the latest event of a completed one).
        pub(crate) completion: Option<i64>,
        pub(crate) plan: Option<(i64, Value)>,
    }
    pub(crate) async fn planning_fact(runtime: &Runtime, task: &str) -> io::Result<PlanningFact> {
        let task = task.to_owned();
        runtime
            .host
            .executor
            .read(move |db| {
                use rusqlite::OptionalExtension;
                let row = db
                    .query_row(
                        "SELECT w.execution_id,e.runtime_task_id,r.state,r.blocker FROM workspace_tasks w
                         LEFT JOIN workspace_executions e ON e.execution_id=w.execution_id
                         LEFT JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE w.task_id=?",
                        [&task],
                        |r| Ok((r.get(0)?, r.get::<_, Option<String>>(1)?,
                            r.get::<_, Option<String>>(2)?, r.get::<_, Option<String>>(3)?)),
                    )
                    .optional()?;
                let Some((execution, runtime_task, state, blocker)) = row else {
                    return Ok(PlanningFact::default());
                };
                let mut fact = PlanningFact {
                    execution,
                    runtime_task: runtime_task.clone(),
                    state: state.clone(),
                    ..PlanningFact::default()
                };
                if let Some(rt) = runtime_task {
                    if state.as_deref() == Some("COMPLETED") {
                        fact.completion = db.query_row(
                            "SELECT coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_STATE_CHANGED'
                                AND json_extract(payload_json,'$.state')='COMPLETED'),(SELECT max(id) FROM runtime_events WHERE task_id=?1))",
                            [&rt],
                            |r| r.get(0),
                        )?;
                    }
                    if let Some(state @ ("FAILED" | "CANCELLED")) = state.as_deref() {
                        fact.failure = recorded_planning_failure(db, &rt, state, blocker.as_deref())?;
                    }
                    fact.plan = cortex_core::lifecycle::sprint_planning::latest(db, &rt)?;
                }
                Ok(fact)
            })
            .map_err(|e| crate::failure(e.code()))?
            .await
            .map_err(|_| crate::failure("native_host_closed"))?
            .map_err(|e| crate::failure(e.code()))
    }
    /// What the sprint does next with its planning task. Pure, so every branch is tested.
    #[derive(Debug, PartialEq)]
    pub(crate) enum PlanningStep {
        Wait,
        Ready {
            items: Vec<Value>,
            run: Value,
        },
        Correct {
            text: String,
            run: Value,
        },
        Cancel {
            note: String,
            code: &'static str,
            run: Value,
        },
        Continue {
            text: String,
            run: Value,
        },
        Terminal {
            failure: PlanningFailure,
            run: Value,
        },
        Replace {
            retry: Option<PlanningFailure>,
        },
        /// A finished turn without a new plan keeps the current proposal (the owner asked
        /// something and the coordinator answered).
        Keep {
            run: Value,
        },
    }
    fn planning_start_reason(code: &str) -> String {
        match code {
            "maintenance_busy" => "Maintenance was running when the planning task was starting.",
            "service_stopping" | "native_host_closed" => {
                "The service was stopping when the planning task was starting."
            }
            "native_capacity_unavailable" => "Too many tasks are running to start planning now.",
            _ => "The planning task could not start. Check the selected model.",
        }
        .to_owned()
    }
    const CORRECTION: &str = "The sprint plan is still missing: this planning task finished without a plan recorded by sprint_plan_submit. Submit the plan with sprint_plan_submit based on what you studied (ask the owner first if an essential decision is missing), then finish the task again.";
    const CONTINUE: &str = "Continue planning this sprint: submit the plan with sprint_plan_submit based on what you studied, then finish the task.";
    pub(crate) fn planning_step(sprint: &Value, fact: &PlanningFact) -> PlanningStep {
        let Some(rt) = fact.runtime_task.as_deref() else {
            return PlanningStep::Wait;
        };
        let prior = &sprint["planningRun"];
        let fresh = prior["runtimeTask"] != rt;
        let handled = if fresh {
            Value::Null
        } else {
            prior["handled"].clone()
        };
        let outcome = if fresh {
            ""
        } else {
            prior["outcome"].as_str().unwrap_or("")
        };
        let corrections = if fresh {
            0
        } else {
            prior["corrections"].as_i64().unwrap_or(0)
        };
        let consumed = if fresh {
            0
        } else {
            prior["planEvent"].as_i64().unwrap_or(0)
        };
        let resumed = sprint["planning"] == "planning";
        let run = |handled: Value, outcome: &str, corrections: i64, plan: i64| {
            json!({"runtimeTask":rt,"handled":handled,"outcome":outcome,
                "corrections":corrections,"planEvent":plan})
        };
        match fact.state.as_deref() {
            Some("COMPLETED") => {
                let completion = json!(fact.completion.unwrap_or(0));
                if handled == completion {
                    // Already answered: a plan waits for the owner or a correction for the task.
                    // A cancelled run the owner resumed asks the same task to continue.
                    return if outcome == "cancelled" && resumed {
                        PlanningStep::Continue {
                            text: CONTINUE.into(),
                            run: run(completion, "continued", 0, consumed),
                        }
                    } else {
                        PlanningStep::Wait
                    };
                }
                match &fact.plan {
                    Some((event, tasks)) if *event > consumed => {
                        match validate_tasks(tasks)
                            .map_err(|e| parse_failure(&e))
                            .and_then(|items| finalize_items(sprint, items))
                        {
                            Ok(items) => PlanningStep::Ready {
                                items,
                                run: run(completion, "ready", 0, *event),
                            },
                            Err(note) => PlanningStep::Cancel {
                                note,
                                code: "plan_rejected",
                                run: run(completion, "cancelled", corrections, *event),
                            },
                        }
                    }
                    _ if !sprint["plan"].is_null() && sprint["plan"]["items"].is_array() => {
                        PlanningStep::Keep {
                            run: run(completion, "ready", 0, consumed),
                        }
                    }
                    _ if corrections < 1 => PlanningStep::Correct {
                        text: CORRECTION.into(),
                        run: run(completion, "corrected", corrections + 1, consumed),
                    },
                    _ => PlanningStep::Cancel {
                        note: "The planning task finished twice without a sprint plan.".into(),
                        code: "plan_missing",
                        run: run(completion, "cancelled", corrections, consumed),
                    },
                }
            }
            Some("FAILED" | "CANCELLED") => {
                let stopped = json!(format!("stopped:{rt}"));
                if handled != stopped {
                    PlanningStep::Terminal {
                        failure: fact.failure,
                        run: run(stopped, "cancelled", corrections, consumed),
                    }
                } else if resumed {
                    PlanningStep::Replace {
                        retry: fact.failure.is_limit().then_some(fact.failure),
                    }
                } else {
                    PlanningStep::Wait
                }
            }
            _ => PlanningStep::Wait,
        }
    }

    /// Background jobs of a pass; dropping the pass aborts them. Planning itself is
    /// an ordinary runtime task, so no planner job runs here.
    #[derive(Default)]
    struct Jobs(
        HashMap<(String, String), tokio::task::JoinHandle<()>>,
        HashMap<String, HashMap<String, u8>>,
    );
    impl Drop for Jobs {
        fn drop(&mut self) {
            for job in self.0.values() {
                job.abort();
            }
        }
    }
    #[cfg(test)]
    mod budget_tests {
        use super::*;
        use crate::{
            native_driver::sprint_tests::{call, sprint, Peer},
            Config, LaunchSpec,
        };
        use std::{
            os::unix::fs::{DirBuilderExt, PermissionsExt},
            path::Path,
        };

        /// A disposable fixture directory.
        struct Scratch(std::path::PathBuf);
        impl Drop for Scratch {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }

        async fn planning_fixture(name: &str) -> (Scratch, Arc<Service>, Arc<Runtime>, String) {
            let root = std::env::temp_dir().join(format!("{name}-{}", crate::identity().unwrap()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .unwrap();
            let scratch = Scratch(root.clone());
            let folder = root.join("project");
            std::fs::create_dir(&folder).unwrap();
            assert!(std::process::Command::new("/usr/bin/git")
                .args(["init", "-q"])
                .current_dir(&folder)
                .status()
                .unwrap()
                .success());
            let service = Service::bind_native(
                Config {
                    runtime_dir: root.join("runtime"),
                    state_dir: root.join("state"),
                    allowed_root: root.clone(),
                    launch: LaunchSpec::bare(),
                    request_timeout: Duration::from_secs(5),
                    shutdown_timeout: Duration::from_secs(2),
                    event_capacity: 64,
                },
                Arc::new(Peer(None)),
            )
            .await
            .unwrap();
            let registered = service.register(&folder, name).await.unwrap();
            let project = registered["projectId"].as_str().unwrap().to_owned();
            service.activate(&project).await.unwrap();
            let runtime = service.state.lock().await.native[&project].clone();
            (scratch, service, runtime, project)
        }

        /// A sprint created exactly as the desktop creates one.
        async fn task_sprint(service: &Service, project: &str, params: Value) -> Value {
            call(service, "product.sprint", project, params).await["sprintId"].clone()
        }

        /// The planning task's current runtime task, once the fixture coordinator's turn settled.
        async fn settled(runtime: &Runtime, draft: &str) -> String {
            let draft = draft.to_owned();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
            loop {
                let t = draft.clone();
                let task: Option<String> = runtime.host.executor.read(move |db| {
                    use rusqlite::OptionalExtension;
                    Ok(db.query_row("SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id
                        JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE w.task_id=?
                        AND r.state IN ('WAITING_USER','FAILED','CANCELLED','BLOCKED','RECOVERING','COMPLETED')
                        AND NOT EXISTS(SELECT 1 FROM capacity_reservations c WHERE c.task_id=r.task_id AND c.state!='released')",
                        [&t], |r| r.get(0)).optional()?)
                }).unwrap().await.unwrap().unwrap();
                if let Some(task) = task {
                    return task;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the fixture coordinator did not settle"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        /// The fixture's stand-in for a coordinator that submitted `plan` (if any) through
        /// sprint_plan_submit and then finished through the ordinary completion gate.
        async fn complete(runtime: &Runtime, task: &str, plan: Option<Value>) {
            let task = task.to_owned();
            runtime.host.executor.write(move |db| {
                if let Some(tasks) = plan {
                    cortex_core::lifecycle::event_tx(db, &task, "SPRINT_PLAN_SUBMITTED", json!({"revision":0,"tasks":tasks}))?;
                }
                db.execute("UPDATE pending_questions SET status='cancelled' WHERE task_id=? AND status='open'", [&task])?;
                db.execute("UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL,synthesis='Planned.' WHERE task_id=?", [&task])?;
                cortex_core::lifecycle::event_tx(db, &task, "TASK_STATE_CHANGED", json!({"previous":"PLANNING","state":"COMPLETED","blocker":null}))?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
        }

        async fn draft_text(runtime: &Runtime, draft: &str) -> String {
            let draft = draft.to_owned();
            runtime
                .host
                .executor
                .read(move |db| {
                    Ok(db.query_row(
                        "SELECT description FROM workspace_tasks WHERE task_id=?",
                        [&draft],
                        |r| r.get(0),
                    )?)
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        }

        fn two_parts() -> Value {
            json!([{"key":"t1","title":"First","description":"Do the first part","dependsOn":[]},
                {"key":"t2","title":"Second","description":"Do the second part","dependsOn":["t1"]}])
        }

        async fn native_board(service: &Service, project: &str) -> Value {
            let reply = service
                .native_query(json!({"protocol":{"major":2,"mapping":2},
                    "requestId":"read-planning-board","projectId":project,
                    "operation":"tasks.board","payload":{"id":project}}))
                .await
                .unwrap();
            assert_eq!(reply["data"]["type"], "board");
            reply["data"]["value"]["items"].clone()
        }

        async fn native_board_task(service: &Service, project: &str, task: &str) -> Value {
            native_board(service, project)
                .await
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["taskId"] == task)
                .expect("task on the native board")
                .clone()
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn sprint_planner_identity_is_public_with_the_draft_before_the_sprint_link() {
            let (_scratch, service, runtime, project) = planning_fixture("planner-identity").await;
            let sprint_id = task_sprint(
                &service,
                &project,
                json!({"action":"create","description":"Plan a change"}),
            )
            .await;
            let id = sprint_id.as_str().unwrap();
            assert_eq!(native_board(&service, &project).await, json!([]));

            let create = json!({"protocol":{"major":1,"minor":8},"requestId":"planner-draft",
                "projectId":project,"actorId":actor(),"operation":"tasks.create","target":project,
                "expectedVersion":"0","ownerEpoch":null,
                "payload":{"description":"Plan a change","checkout":{"kind":"main"}}});
            // This is the driver's create step, deliberately without the later CAS or Start.
            let first = runtime
                .create_draft(project.clone(), create.clone(), Some(id.to_owned()))
                .await
                .unwrap();
            assert_eq!(first["outcome"], "recorded");
            assert!(sprint(&service, &project, &sprint_id).await["planningTaskId"].is_null());
            let draft = first["target"].as_str().unwrap();
            let row = native_board_task(&service, &project, draft).await;
            assert_eq!(row["planningSprintId"], id);
            assert_eq!(row["version"], "1");
            assert!(row["executionId"].is_null());

            let replay = runtime
                .create_draft(project.clone(), create.clone(), Some(id.to_owned()))
                .await
                .unwrap();
            assert_eq!(replay, first);
            let conflict = runtime
                .command(project.clone(), create.clone())
                .await
                .unwrap();
            assert_eq!(conflict["outcome"], "payload_conflict");

            let mut ordinary = create;
            ordinary["requestId"] = json!("ordinary-draft");
            ordinary["payload"]["planningSprintId"] = json!(id);
            let forged = runtime
                .command(project.clone(), ordinary.clone())
                .await
                .unwrap_err();
            assert_eq!(forged.to_string(), "invalid_workspace_request");
            ordinary["payload"]
                .as_object_mut()
                .unwrap()
                .remove("planningSprintId");
            let ordinary = runtime.command(project.clone(), ordinary).await.unwrap();
            assert_eq!(ordinary["outcome"], "recorded");
            let task = ordinary["target"].as_str().unwrap();
            assert!(native_board_task(&service, &project, task)
                .await
                .get("planningSprintId")
                .is_none());
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_sprint_plans_in_a_read_only_task_revises_on_request_and_approval_closes_it() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-task").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(&service, &project,
                json!({"action":"create","description":"Build the two parts","runMode":"hybrid","checkout":{"kind":"main"}})).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let started = sprint(&service, &project, &id).await;
            let draft = started["planningTaskId"]
                .as_str()
                .expect("a planning task")
                .to_owned();
            assert_eq!(started["planning"], "planning");
            assert_eq!(
                native_board_task(&service, &project, &draft).await["planningSprintId"],
                id
            );
            // A task in the project folder, asked to study and plan read-only.
            let text = draft_text(&runtime, &draft).await;
            assert!(text.starts_with("Sprint planning (read-only)"), "{text}");
            assert!(text.contains("Build the two parts"), "{text}");
            assert!(text.contains("sprint_plan_submit"), "{text}");
            let task = settled(&runtime, &draft).await;
            let t = task.clone();
            let bound = runtime
                .host
                .executor
                .read(move |db| Ok(cortex_core::lifecycle::sprint_planning::enabled(db, &t)?))
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            assert!(bound, "the planning task is bound read-only at Start");
            // It is the sprint's planning task, never one of its members.
            let board = call(&service, "product.board", &project, json!({})).await;
            assert!(board["tasks"][&draft]["sprintId"].is_null());
            assert_eq!(sprint(&service, &project, &id).await["taskIds"], json!([]));
            // A second pass while the task works changes nothing and creates nothing.
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(
                sprint(&service, &project, &id).await["planningTaskId"],
                draft.as_str()
            );

            // The completed task's plan becomes the sprint's proposal.
            complete(&runtime, &task, Some(two_parts())).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let ready = sprint(&service, &project, &id).await;
            assert_eq!(ready["planning"], "ready", "{ready}");
            assert_eq!(ready["plan"]["version"], 1);
            assert_eq!(ready["plan"]["items"].as_array().unwrap().len(), 2);
            assert_eq!(ready["plan"]["items"][1]["dependsOn"], json!(["t1"]));
            assert_eq!(ready["history"][0], ready["plan"]);
            // Nothing is created or started by planning.
            assert_eq!(ready["taskIds"], json!([]));
            assert_eq!(ready["queueState"], "idle");
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(sprint(&service, &project, &id).await["plan"]["version"], 1);

            // The owner asks for changes in the planning conversation: the same task revises the plan.
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"patch","sprintId":id,"patch":{"planning":"planning"}}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(
                sprint(&service, &project, &id).await["planning"],
                "planning"
            );
            let mut revised = two_parts();
            revised.as_array_mut().unwrap().push(
                json!({"key":"t3","title":"Third","description":"Document both parts","dependsOn":["t2"]}));
            complete(&runtime, &task, Some(revised)).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let revised = sprint(&service, &project, &id).await;
            assert_eq!(revised["planning"], "ready", "{revised}");
            assert_eq!(revised["plan"]["version"], 2);
            assert_eq!(revised["plan"]["items"].as_array().unwrap().len(), 3);
            assert_eq!(revised["history"].as_array().unwrap().len(), 2);
            // A question answered without a new plan keeps the current proposal.
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"patch","sprintId":id,"patch":{"planning":"planning"}}),
            )
            .await;
            complete(&runtime, &task, None).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let kept = sprint(&service, &project, &id).await;
            assert_eq!(kept["planning"], "ready", "{kept}");
            assert_eq!(kept["plan"], revised["plan"]);

            // Approval materializes the plan; the planning task closes as done.
            call(&service, "product.sprint", &project,
                json!({"action":"patch","sprintId":id,"patch":{"planning":"created","materializedVersion":2,"taskIds":[]}})).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let board = call(&service, "product.board", &project, json!({})).await;
            assert_eq!(
                board["tasks"][&draft]["status"], "done",
                "{}",
                board["tasks"][&draft]
            );
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_missing_plan_is_asked_for_once_then_planning_is_cancelled_and_resumes() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-correct").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(
                &service,
                &project,
                json!({"action":"create","description":"Plan the cleanup"}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let draft = sprint(&service, &project, &id).await["planningTaskId"]
                .as_str()
                .unwrap()
                .to_owned();
            let task = settled(&runtime, &draft).await;
            complete(&runtime, &task, None).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let corrected = sprint(&service, &project, &id).await;
            assert_eq!(corrected["planning"], "planning", "{corrected}");
            assert_eq!(
                corrected["planningRun"]["outcome"], "corrected",
                "{corrected}"
            );
            assert_eq!(corrected["planningRun"]["corrections"], 1);
            // The correction went to the same task, as an ordinary message.
            let t = task.clone();
            let delivered: bool = runtime.host.executor.read(move |db| {
                Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND payload_json LIKE '%sprint plan is still missing%')",
                    [&t], |r| r.get(0))?)
            }).unwrap().await.unwrap().unwrap();
            assert!(delivered, "the correction reaches the planning task");
            let again = settled(&runtime, &draft).await;
            assert_eq!(again, task, "the same planning task continues");
            complete(&runtime, &task, None).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let cancelled = sprint(&service, &project, &id).await;
            assert_eq!(cancelled["planning"], "cancelled", "{cancelled}");
            assert!(cancelled["plan"].is_null());
            let note = cancelled["conversation"]
                .as_array()
                .unwrap()
                .last()
                .unwrap()["text"]
                .as_str()
                .unwrap()
                .to_owned();
            assert!(
                note.contains("finished twice without a sprint plan"),
                "{note}"
            );
            assert!(note.contains("Resume planning"), "{note}");
            let event = cancelled["conversation"]
                .as_array()
                .unwrap()
                .last()
                .unwrap();
            assert_eq!(event["kind"], "event");
            assert!(event["at"].as_u64().is_some_and(|at| at > 0));
            assert_eq!(
                cancelled["planningFailure"],
                json!({"text":note,"step":"run",
                "code":"plan_missing","at":event["at"]})
            );
            // Another pass leaves the cancelled sprint as it is.
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(
                sprint(&service, &project, &id).await["planning"],
                "cancelled"
            );
            // Resume planning asks the same task to continue.
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"patch","sprintId":id,"patch":{"planning":"planning"}}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let resumed = sprint(&service, &project, &id).await;
            assert_eq!(resumed["planning"], "planning", "{resumed}");
            assert!(resumed.get("planningFailure").is_none());
            assert_eq!(resumed["planningRun"]["outcome"], "continued");
            assert_eq!(resumed["planningTaskId"], draft.as_str());
            settled(&runtime, &draft).await;
            complete(&runtime, &task, Some(two_parts())).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(sprint(&service, &project, &id).await["planning"], "ready");
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn sprint_planning_diagnostics_preserve_saved_work_and_owner_cancel_is_neutral() {
            let (_scratch, service, runtime, project) =
                planning_fixture("sprint-diagnostics").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(
                &service,
                &project,
                json!({"action":"create","description":"Keep the saved request"}),
            )
            .await;
            let sprint_id = id.as_str().unwrap();
            let plan =
                json!({"version":1,"items":[{"key":"saved","description":"Previously proposed"}]});
            service
                .sprint_patch(&project, sprint_id, json!({"plan":plan,"history":[plan]}))
                .await
                .unwrap();
            service
                .sprint_note(&project, sprint_id, "answer", "Keep this answer")
                .await
                .unwrap();
            let before = sprint(&service, &project, &id).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, true)
                .await
                .unwrap();
            let failed = sprint(&service, &project, &id).await;
            assert_eq!(failed["planning"], "cancelled");
            let conversation = failed["conversation"].as_array().unwrap();
            assert_eq!(conversation.len(), 2);
            assert_eq!(conversation[0], before["conversation"][0]);
            assert_eq!(conversation[1]["text"], failed["planningFailure"]["text"]);
            for key in ["description", "attachments", "plan", "history"] {
                assert_eq!(failed[key], before[key], "{key}");
            }

            service
                .sprint_patch(&project, sprint_id, json!({"planning":"planning"}))
                .await
                .unwrap();
            assert!(sprint(&service, &project, &id)
                .await
                .get("planningFailure")
                .is_none());
            for field in ["planningFailure", "planningWarning"] {
                let patch = json!({(field):{"text":"Not a service diagnostic"}});
                let request = serde_json::from_value(json!({
                    "op":"product.sprint","projectId":project,
                    "params":{"action":"patch","sprintId":sprint_id,"patch":patch}
                }))
                .unwrap();
                let reply = service.machine(request).await.unwrap();
                assert_eq!(reply["ok"], false);
                assert_eq!(reply["code"], "invalid_sprint");
            }
            service
                .sprint_patch(&project, sprint_id, json!({"planning":"cancelled"}))
                .await
                .unwrap();
            // A failure from an in-flight pass cannot overwrite the owner's stop.
            service
                .cancel_planning(
                    &project,
                    sprint_id,
                    "start",
                    "planning_start_failed",
                    "Planning did not start.",
                    None,
                )
                .await
                .unwrap();
            let stopped = sprint(&service, &project, &id).await;
            assert_eq!(stopped["planning"], "cancelled");
            assert!(stopped.get("planningFailure").is_none());
            assert_eq!(stopped["conversation"], failed["conversation"]);
            service.stop().await.unwrap();
        }

        async fn linked_unstarted(
            service: &Service,
            runtime: &Arc<Runtime>,
            project: &str,
            id: &Value,
        ) -> String {
            let created = runtime.command(
                project.to_owned(),
                json!({"protocol":{"major":1,"minor":8},"requestId":"fixture-create",
                    "projectId":project,"actorId":actor(),"operation":"tasks.create","target":project,
                    "expectedVersion":"0","ownerEpoch":null,"payload":{"description":"Old draft"}}),
            ).await.unwrap();
            assert_eq!(created["outcome"], "recorded", "{created}");
            let draft = created["target"].as_str().unwrap().to_owned();
            call(
                service,
                "product.sprint",
                project,
                json!({"action":"patch","sprintId":id,
                "patch":{"planningTaskId":draft,"planningRun":{"outcome":"fixture"}}}),
            )
            .await;
            draft
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn planning_draft_accepts_product_close_as_cancelled() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-close").await;
            let id = task_sprint(
                &service,
                &project,
                json!({"action":"create","description":"Check closure"}),
            )
            .await;
            let task = linked_unstarted(&service, &runtime, &project, &id).await;

            call(
                &service,
                "product.task",
                &project,
                json!({"action":"close","taskId":task,"choice":"cancel","confirmed":true}),
            )
            .await;

            let board = call(&service, "product.board", &project, json!({})).await;
            assert_eq!(board["tasks"][&task]["status"], "done");
            assert_eq!(board["tasks"][&task]["closedAs"], "cancel");
            assert_eq!(draft_text(&runtime, &task).await, "Old draft");
            service.stop().await.unwrap();
        }

        async fn assert_retired_planner(
            service: &Service,
            project: &str,
            task: &str,
            expected_phase: &str,
        ) {
            let board = call(service, "product.board", project, json!({})).await;
            let record = &board["tasks"][task];
            assert_eq!(record["retiredPlanner"], true, "{record}");
            assert_eq!(record["closedAs"], "cancel");
            assert_eq!(record["status"], "done");
            assert_ne!(record["purged"], true);
            let native = native_board_task(service, project, task).await;
            assert_eq!(native["phase"], expected_phase);

            // Even a pending decision from a stale read is not an ordinary task
            // notification. This exercises the projection that feeds the bus.
            let decisions = HashMap::from([(
                task.to_owned(),
                vec![json!({"status":"pending","decisionId":"pending-fixture"})],
            )]);
            assert!(crate::notifier::items(project, &[native], &board, &decisions, &[])
                .iter()
                .all(|item| item.place.sprint || item.place.id != task));
        }

        async fn recover_sprint(service: &Service, project: &str, params: Value) -> Value {
            let request = serde_json::from_value(json!({
                "op":"product.sprint","projectId":project,"params":params
            }))
            .unwrap();
            service.machine(request).await.unwrap()
        }

        async fn started_draft(
            service: &Service,
            runtime: &Runtime,
            project: &str,
            id: &Value,
        ) -> String {
            let saved = sprint(service, project, id).await;
            let task = saved["planningTaskId"].as_str().expect("linked draft");
            assert!(planning_fact(runtime, task)
                .await
                .unwrap()
                .execution
                .is_some());
            task.to_owned()
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn refused_planning_start_unlinks_the_draft_and_owner_retry_starts_a_new_one() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-retry").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(
                &service,
                &project,
                json!({"action":"create","description":"Current owner description"}),
            )
            .await;
            let old = linked_unstarted(&service, &runtime, &project, &id).await;

            // Start with an outdated version: the runtime refuses without committing execution.
            let refused = runtime
                .start_sprint_planning(
                    project.clone(),
                    json!({"protocol":{"major":1,"minor":8},"requestId":"fixture-start-stale",
                    "projectId":project,"actorId":actor(),"operation":"tasks.start","target":old,
                    "expectedVersion":"2","ownerEpoch":null,"payload":{}}),
                )
                .await;
            assert_eq!(refused.as_ref().unwrap_err().to_string(), "stale_version");
            service
                .finish_planning_start(&project, &runtime, id.as_str().unwrap(), &old, refused)
                .await
                .unwrap();

            let failed = sprint(&service, &project, &id).await;
            assert_eq!(failed["planning"], "cancelled");
            assert!(failed["planningTaskId"].is_null());
            assert!(failed["planningRun"].is_null());
            assert_eq!(failed["planningFailure"]["code"], "planning_start_failed");
            assert_retired_planner(&service, &project, &old, "not_started").await;
            assert_eq!(
                failed["conversation"][0]["text"],
                failed["planningFailure"]["text"]
            );
            assert_eq!(draft_text(&runtime, &old).await, "Old draft");
            assert!(planning_fact(&runtime, &old)
                .await
                .unwrap()
                .execution
                .is_none());
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(
                sprint(&service, &project, &id).await,
                failed,
                "no automatic retry"
            );

            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"note","sprintId":id,
                "kind":"answer","text":"Use option A"}),
            )
            .await;
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"patch","sprintId":id,
                "patch":{"planning":"planning"}}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let new = started_draft(&service, &runtime, &project, &id).await;
            assert_ne!(new, old);
            let text = draft_text(&runtime, &new).await;
            assert!(
                text.contains("Current owner description")
                    && text.contains("Owner answer: Use option A")
            );
            assert!(sprint(&service, &project, &id).await["planningFailure"].is_null());
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn retry_recovers_an_old_failed_sprint_with_a_linked_unstarted_draft() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-stuck").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(
                &service,
                &project,
                json!({"action":"create","description":"Legacy sprint"}),
            )
            .await;
            let old = linked_unstarted(&service, &runtime, &project, &id).await;
            service
                .cancel_planning(
                    &project,
                    id.as_str().unwrap(),
                    "start",
                    "planning_start_refused",
                    "Legacy failure.",
                    None,
                )
                .await
                .unwrap();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(
                sprint(&service, &project, &id).await["planningTaskId"],
                old.as_str()
            );

            for _ in 0..2 {
                call(
                    &service,
                    "product.sprint",
                    &project,
                    json!({"action":"patch","sprintId":id,
                    "patch":{"planning":"planning"}}),
                )
                .await;
            }
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let fresh = started_draft(&service, &runtime, &project, &id).await;
            assert_ne!(fresh, old);
            assert_retired_planner(&service, &project, &old, "not_started").await;
            assert_eq!(draft_text(&runtime, &old).await, "Old draft");
            assert!(planning_fact(&runtime, &old)
                .await
                .unwrap()
                .execution
                .is_none());
            assert!(sprint(&service, &project, &id).await["planningRetry"].is_null());
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn planning_message_recovers_a_failed_sprint_and_passes_the_saved_note() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-change").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(
                &service,
                &project,
                json!({"action":"create","description":"Keep the original"}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let old = started_draft(&service, &runtime, &project, &id).await;
            let runtime_task = settled(&runtime, &old).await;
            runtime.host.executor.write(move |db| {
                db.execute("UPDATE pending_questions SET status='cancelled' WHERE task_id=? AND status='open'", [&runtime_task])?;
                db.execute("UPDATE runtime_tasks SET state='FAILED',blocker='fixture_failure' WHERE task_id=?", [&runtime_task])?;
                cortex_core::lifecycle::event_tx(db, &runtime_task, "TASK_STATE_CHANGED", json!({"state":"FAILED","blocker":"fixture_failure"}))?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let failed = sprint(&service, &project, &id).await;
            assert_eq!(failed["planningTaskId"], old.as_str());
            assert_eq!(failed["planning"], "cancelled");
            let params = json!({"action":"recover_planning_message","sprintId":id,
                "text":"Use the smaller version", "attachments":[]});
            let reply = recover_sprint(&service, &project, params.clone()).await;
            assert_eq!(reply["result"], json!({"done":true}), "{reply}");
            let accepted = sprint(&service, &project, &id).await;
            assert_eq!(accepted["planning"], "planning");
            assert!(accepted["planningTaskId"].is_null());
            assert!(accepted["planningFailure"].is_null());
            assert_retired_planner(&service, &project, &old, "failed").await;
            let repeated = recover_sprint(&service, &project, params).await;
            assert_eq!(repeated["code"], "planning_recovery_unavailable");
            assert_eq!(sprint(&service, &project, &id).await, accepted);

            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let fresh = started_draft(&service, &runtime, &project, &id).await;
            assert_ne!(fresh, old);
            let text = draft_text(&runtime, &fresh).await;
            assert!(
                text.contains("Keep the original")
                    && text.contains("Owner message: Use the smaller version")
            );
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn recovery_message_refuses_to_replace_a_live_planner() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-live-guard").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(
                &service,
                &project,
                json!({"action":"create","description":"Keep the live task"}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = started_draft(&service, &runtime, &project, &id).await;
            let runtime_task = settled(&runtime, &task).await;
            runtime
                .host
                .executor
                .write(move |db| {
                    db.execute(
                        "UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?",
                        [&runtime_task],
                    )?;
                    Ok(())
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            service
                .cancel_planning(
                    &project,
                    id.as_str().unwrap(),
                    "continue",
                    "planning_continue_failed",
                    "Fixture failure.",
                    None,
                )
                .await
                .unwrap();
            let before = sprint(&service, &project, &id).await;
            let reply = recover_sprint(
                &service,
                &project,
                json!({"action":"recover_planning_message","sprintId":id,"text":"Do not detach"}),
            )
            .await;
            assert_eq!(reply["code"], "planning_execution_live", "{reply}");
            assert_eq!(sprint(&service, &project, &id).await, before);
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn replacing_an_overlong_request_preserves_saved_work_and_starts_planning() {
            let (_scratch, service, runtime, project) =
                planning_fixture("sprint-edit-request").await;
            let mut jobs = Jobs::default();
            let files = call(&service, "product.attachment", &project, json!({
                "action":"upload","files":[{"name":"owner.txt","mimeType":"text/plain","data":"YQ=="}]
            })).await;
            let id = task_sprint(
                &service,
                &project,
                json!({"action":"create",
                "description":format!("old request {}", "x".repeat(31_900)), "attachments":files}),
            )
            .await;
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"note","sprintId":id,
                "kind":"answer","text":"Preserved answer"}),
            )
            .await;
            let plan = json!({"version":1,"items":two_parts()});
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"patch","sprintId":id,
                "patch":{"plan":plan,"history":[plan]}}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let failed = sprint(&service, &project, &id).await;
            assert_eq!(
                failed["planningFailure"]["code"],
                "planning_request_invalid"
            );
            assert!(failed["planningFailure"]["text"]
                .as_str()
                .unwrap()
                .contains("limit 32000"));

            // An older failure may still link a draft without a native planner marker.
            let old = linked_unstarted(&service, &runtime, &project, &id).await;
            let failed = sprint(&service, &project, &id).await;
            let reply = recover_sprint(
                &service,
                &project,
                json!({
                    "action":"replace_planning_request","sprintId":id,"description":" "
                }),
            )
            .await;
            assert_eq!(reply["code"], "invalid_sprint");
            assert_eq!(sprint(&service, &project, &id).await, failed);

            let reply = recover_sprint(&service, &project, json!({
                "action":"replace_planning_request","sprintId":id,"description":"Short new request"
            })).await;
            assert_eq!(reply["result"], json!({"done":true}), "{reply}");
            let edited = sprint(&service, &project, &id).await;
            assert_eq!(edited["description"], "Short new request");
            assert!(edited["planningFailure"].is_null());
            assert_retired_planner(&service, &project, &old, "not_started").await;
            for key in ["attachments", "conversation", "plan", "history"] {
                assert_eq!(edited[key], failed[key], "{key}");
            }

            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = started_draft(&service, &runtime, &project, &id).await;
            let text = draft_text(&runtime, &task).await;
            assert!(
                text.contains("Short new request")
                    && text.contains("Owner answer: Preserved answer")
            );
            assert!(!text.contains("old request"));
            assert_eq!(
                runtime.draft_attachments(project, task).await.unwrap()[0]["name"],
                "owner.txt"
            );
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn ambiguous_planning_start_keeps_a_committed_execution_link() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-lost-reply").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(
                &service,
                &project,
                json!({"action":"create","description":"Started planning"}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let draft = started_draft(&service, &runtime, &project, &id).await;

            // Drive the same readback used on a lost Start reply after a real commit.
            service
                .finish_planning_start(
                    &project,
                    &runtime,
                    id.as_str().unwrap(),
                    &draft,
                    Err(crate::failure("native_host_closed")),
                )
                .await
                .unwrap();
            let saved = sprint(&service, &project, &id).await;
            assert_eq!(saved["planningTaskId"], draft.as_str());
            assert_eq!(saved["planning"], "planning");
            assert!(saved["planningFailure"].is_null());
            let board = call(&service, "product.board", &project, json!({})).await;
            assert!(board["tasks"][&draft]["retiredPlanner"].is_null());

            let runtime_task = settled(&runtime, &draft).await;
            complete(&runtime, &runtime_task, Some(two_parts())).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(sprint(&service, &project, &id).await["planning"], "ready");
            assert_eq!(
                sprint(&service, &project, &id).await["planningTaskId"],
                draft.as_str()
            );
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn sprint_planning_warning_is_once_per_streak_and_clears_on_success() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-warning").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(
                &service,
                &project,
                json!({"action":"create","description":"Check a stalled sprint"}),
            )
            .await;
            let sprint_id = id.as_str().unwrap();
            service
                .sprint_patch(&project, sprint_id, json!({"planning":"question"}))
                .await
                .unwrap();
            // Inject pass results, while exercising the real warning writer and board read.
            for attempt in 1..=5 {
                let snapshot = sprint(&service, &project, &id).await;
                service
                    .observe_planning_pass(
                        &project,
                        &snapshot,
                        jobs.1.entry(project.clone()).or_default(),
                        Err(crate::failure("planning_unavailable")),
                    )
                    .await;
                let saved = sprint(&service, &project, &id).await;
                assert_eq!(saved["planning"], "question");
                assert_eq!(
                    saved["conversation"].as_array().unwrap().len(),
                    usize::from(attempt >= 3)
                );
                if attempt >= 3 {
                    let note = &saved["conversation"][0];
                    assert_eq!(note["kind"], "event");
                    assert!(note["at"].as_u64().is_some_and(|at| at > 0));
                    assert_eq!(
                        saved["planningWarning"],
                        json!({"text":note["text"],"step":"internal",
                        "code":"planning_pass_failed","at":note["at"],"count":3})
                    );
                } else {
                    assert!(saved.get("planningWarning").is_none());
                }
            }
            // This older saved sprint has no planning task; a real pass succeeds without starting one.
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let recovered = sprint(&service, &project, &id).await;
            assert!(recovered.get("planningWarning").is_none());
            assert_eq!(recovered["planning"], "question");
            assert_eq!(recovered["conversation"].as_array().unwrap().len(), 1);
            assert!(!jobs.1[&project].contains_key(sprint_id));
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(sprint(&service, &project, &id).await, recovered);
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_failed_planning_warning_write_is_retried_while_the_streak_continues() {
            let (scratch, service, runtime, project) =
                planning_fixture("sprint-warning-retry").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(
                &service,
                &project,
                json!({"action":"create","description":"Check a stalled sprint"}),
            )
            .await;
            let sprint_id = id.as_str().unwrap();
            service
                .sprint_patch(&project, sprint_id, json!({"planning":"question"}))
                .await
                .unwrap();
            let product_path = scratch
                .0
                .join("state/product")
                .join(format!("{project}.json"));
            for attempt in 1..=6 {
                let snapshot = sprint(&service, &project, &id).await;
                // The third pass's warning write meets an unreadable store, which is
                // restored before the next pass while planning passes keep failing.
                let saved_bytes = (attempt == 3).then(|| {
                    let bytes = std::fs::read(&product_path).unwrap();
                    std::fs::write(&product_path, b"{").unwrap();
                    bytes
                });
                service
                    .observe_planning_pass(
                        &project,
                        &snapshot,
                        jobs.1.entry(project.clone()).or_default(),
                        Err(crate::failure("planning_unavailable")),
                    )
                    .await;
                if let Some(bytes) = saved_bytes {
                    std::fs::write(&product_path, bytes).unwrap();
                }
                let saved = sprint(&service, &project, &id).await;
                assert_eq!(saved["planning"], "question");
                let events = saved["conversation"].as_array().unwrap();
                if attempt <= 3 {
                    assert!(events.is_empty(), "pass {attempt}");
                    assert!(saved.get("planningWarning").is_none(), "pass {attempt}");
                } else {
                    // Written on the next failed pass, once for the whole streak.
                    assert_eq!(events.len(), 1, "pass {attempt}");
                    assert_eq!(events[0]["kind"], "event");
                    assert_eq!(
                        saved["planningWarning"],
                        json!({"text":events[0]["text"],"step":"internal",
                        "code":"planning_pass_failed","at":events[0]["at"],"count":3})
                    );
                }
            }
            // A successful pass clears the active warning; the event stays.
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let recovered = sprint(&service, &project, &id).await;
            assert!(recovered.get("planningWarning").is_none());
            assert_eq!(recovered["conversation"].as_array().unwrap().len(), 1);
            assert!(!jobs.1[&project].contains_key(sprint_id));
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn an_old_saved_sprint_resumes_into_a_planning_task_and_close_stops_it() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-old").await;
            let mut jobs = Jobs::default();
            // Saved by the old version: a description-only planning run without a task,
            // interrupted by the restart. It is never silently rerun.
            let id = task_sprint(
                &service,
                &project,
                json!({"action":"create","description":"Old sprint"}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, true)
                .await
                .unwrap();
            let old = sprint(&service, &project, &id).await;
            assert_eq!(old["planning"], "cancelled");
            assert_eq!(old["planningFailure"]["step"], "restart");
            assert_eq!(old["planningFailure"]["code"], "planning_interrupted");
            assert!(old["planningTaskId"].is_null());
            assert!(old["conversation"]
                .to_string()
                .contains("Planning was interrupted"));
            // An old open planning question waits for the owner; nothing runs for it.
            call(&service, "product.sprint", &project,
                json!({"action":"note","sprintId":id,"kind":"question","text":"Which part first?","options":["A","B"]})).await;
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"patch","sprintId":id,"patch":{"planning":"question"}}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert!(sprint(&service, &project, &id).await["planningTaskId"].is_null());
            // The owner answers as before: the answer is kept and planning resumes in a task.
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"note","sprintId":id,"kind":"answer","text":"Part A first"}),
            )
            .await;
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"patch","sprintId":id,"patch":{"planning":"planning"}}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let resumed = sprint(&service, &project, &id).await;
            let draft = resumed["planningTaskId"]
                .as_str()
                .expect("a planning task")
                .to_owned();
            assert_eq!(
                native_board_task(&service, &project, &draft).await["planningSprintId"],
                id
            );
            let text = draft_text(&runtime, &draft).await;
            assert!(
                text.contains("Old sprint") && text.contains("Owner answer: Part A first"),
                "{text}"
            );
            assert!(text.contains("Question: Which part first?"), "{text}");
            // Closing the sprint stops its planning task through the execution control.
            let task = settled(&runtime, &draft).await;
            runtime
                .host
                .executor
                .write({
                    let task = task.clone();
                    move |db| {
                        db.execute(
                            "UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?",
                            [&task],
                        )?;
                        Ok(())
                    }
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"close","sprintId":id,"confirmed":true}),
            )
            .await;
            assert!(!sprint(&service, &project, &id).await["archivedAt"].is_null());
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let t = task.clone();
            let stopped: bool = runtime.host.executor.read(move |db| {
                Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type LIKE '%PAUSE%')
                    OR EXISTS(SELECT 1 FROM task_stopped WHERE task_id=?)", [&t, &t], |r| r.get(0))?)
            }).unwrap().await.unwrap().unwrap();
            assert!(stopped, "closing the sprint stops its planning task");
            service.stop().await.unwrap();
        }

        /// One `sprint_plan_submit` call of the planning coordinator, after channel checks.
        async fn submit(runtime: &Runtime, task: &str, args: Value) -> Value {
            let task = task.to_owned();
            runtime
                .host
                .executor
                .write(move |db| {
                    crate::native_sprint_plan::submit_in_tx(db, &task, "coordinator-channel", &args)
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap()["structuredContent"]
                .clone()
        }


        #[test]
        fn sprint_plan_large_request_prompt_directs_ordered_parts_without_prose() {
            let request = planning_request(
                &json!({"description":"Plan at least fifty-five items",
                    "conversation":[{"kind":"answer","text":"Keep the shared dependency"}]}),
                None,
            )
            .unwrap();
            for fragment in [
                "submit small ordered parts as you work",
                "part 1, 2, and so on",
                "final:false on each intermediate part and final:true on the last",
                "advertised limits",
                "scope, dependencies, owner decisions and acceptance criteria",
                "Do not echo the full plan in prose.",
                "Owner answer: Keep the shared dependency",
            ] {
                assert!(request.contains(fragment), "missing {fragment}");
            }
            assert!(!request.contains("Retry strategy:"));
        }

        async fn sprint_plan_set_state(
            runtime: &Runtime,
            task: &str,
            state: &'static str,
            blocker: Option<&'static str>,
        ) {
            let task = task.to_owned();
            runtime.host.executor.write(move |db| {
                let previous: String = db.query_row(
                    "SELECT state FROM runtime_tasks WHERE task_id=?", [&task], |r| r.get(0),
                )?;
                db.execute(
                    "UPDATE pending_questions SET status='cancelled' WHERE task_id=? AND status='open'",
                    [&task],
                )?;
                db.execute("UPDATE runtime_tasks SET state=?,blocker=? WHERE task_id=?",
                    (state, blocker, &task))?;
                cortex_core::lifecycle::event_tx(db, &task, "TASK_STATE_CHANGED",
                    json!({"state":state,"previous":previous,"blocker":blocker}))?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
        }

        async fn sprint_plan_reject_large_part(runtime: &Runtime, task: &str) {
            sprint_plan_set_state(runtime, task, "PLANNING", None).await;
            let tasks: Vec<_> = (0..32).map(|n| json!({
                "key":format!("t{n}"),"title":format!("Item {n}"),"description":"x".repeat(4096),
            })).collect();
            let refusal = submit(runtime, task,
                json!({"request_key":"oversize","part":1,"final":false,"tasks":tasks})).await;
            assert_eq!(refusal["code"], "plan_part_too_large");
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn sprint_plan_unknown_failure_stays_unknown() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-unknown").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(&service, &project,
                json!({"action":"create","description":"Plan a sprint"})).await;
            service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
            let draft = started_draft(&service, &runtime, &project, &id).await;
            let task = settled(&runtime, &draft).await;
            // A later accepted part supersedes an earlier size refusal. A raw
            // blocker containing timeout-like text is never projected verbatim.
            sprint_plan_reject_large_part(&runtime, &task).await;
            let accepted = submit(&runtime, &task,
                json!({"request_key":"corrected","part":1,"final":false,"tasks":two_parts()})).await;
            assert_eq!(accepted["recorded"], true);
            sprint_plan_set_state(&runtime, &task, "FAILED", None).await;
            assert_eq!(planning_fact(&runtime, &draft).await.unwrap().failure, PlanningFailure::Unknown);
            sprint_plan_set_state(&runtime, &task, "FAILED", Some("timeout: raw engine detail")).await;
            service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
            let saved = sprint(&service, &project, &id).await;
            assert_eq!(saved["planningFailure"]["kind"], "unknown");
            assert_eq!(saved["planningFailure"]["recommendedAction"], "retry");
            assert_eq!(saved["planningFailure"]["consecutiveLimitReplacementFailures"], 0);
            let text = saved["planningFailure"]["text"].as_str().unwrap();
            assert!(text.contains("without a known cause") && !text.contains("timeout"), "{text}");
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn sprint_plan_terminal_failure_reason_is_classified_from_exact_runtime_blockers() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-blockers").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(&service, &project,
                json!({"action":"create","description":"Plan a sprint"})).await;
            service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
            let draft = started_draft(&service, &runtime, &project, &id).await;
            let task = settled(&runtime, &draft).await;
            sprint_plan_set_state(&runtime, &task, "FAILED", Some("scheduler_capacity_timeout")).await;
            assert_eq!(planning_fact(&runtime, &draft).await.unwrap().failure, PlanningFailure::CapacityTimeout);
            sprint_plan_reject_large_part(&runtime, &task).await;
            // Explicit owner action outranks the plan refusal.
            let t = task.clone();
            runtime.host.executor.write(move |db| {
                cortex_core::lifecycle::domain::cancel_task_tx(db, &t, "user_force_finished")
            }).unwrap().await.unwrap().unwrap();
            service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
            let saved = sprint(&service, &project, &id).await;
            assert_eq!(saved["planningFailure"]["kind"], "owner_stopped");
            assert_eq!(saved["planningFailure"]["text"], "You stopped the planning task.");
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn sprint_plan_retry_changes_strategy_and_repeated_limit_recommends_change_request() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-limit-retry").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(&service, &project,
                json!({"action":"create","description":"Keep the owner's large request"})).await;
            service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
            let draft = started_draft(&service, &runtime, &project, &id).await;
            let task = settled(&runtime, &draft).await;
            service.sprint_note(&project, id.as_str().unwrap(), "answer", "Keep option A").await.unwrap();
            sprint_plan_reject_large_part(&runtime, &task).await;
            sprint_plan_set_state(&runtime, &task, "FAILED", None).await;
            service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
            let failed = sprint(&service, &project, &id).await;
            assert_eq!(failed["planning"], "cancelled");
            assert_eq!(failed["planningFailure"]["kind"], "limit");
            assert_eq!(failed["planningFailure"]["code"], "plan_part_too_large");
            assert_eq!(failed["planningFailure"]["recommendedAction"], "retry");
            assert_eq!(failed["planningFailure"]["consecutiveLimitReplacementFailures"], 0);

            let retry = json!({"action":"patch","sprintId":id,"patch":{"planning":"planning"}});
            assert_eq!(recover_sprint(&service, &project, retry.clone()).await["ok"], true);
            service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
            let replacement = started_draft(&service, &runtime, &project, &id).await;
            assert_ne!(draft, replacement);
            let new_text = draft_text(&runtime, &replacement).await;
            assert!(new_text.contains(PlanningFailure::PartSize.reason()));
            assert!(new_text.contains("from the outset, even if the whole plan might fit in one call"));
            assert!(new_text.contains("Owner answer: Keep option A"));
            let new_task = settled(&runtime, &replacement).await;
            sprint_plan_reject_large_part(&runtime, &new_task).await;
            sprint_plan_set_state(&runtime, &new_task, "FAILED", None).await;
            service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
            let repeated = sprint(&service, &project, &id).await;
            assert_eq!(repeated["planning"], "cancelled");
            assert_eq!(repeated["planningFailure"]["recommendedAction"], "change_request");
            assert_eq!(repeated["planningFailure"]["consecutiveLimitReplacementFailures"], 1);
            let refusal = recover_sprint(&service, &project, retry).await;
            assert_eq!(refusal["code"], "planning_change_request_required");
            service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
            assert_eq!(sprint(&service, &project, &id).await, repeated);

            let unchanged = recover_sprint(&service, &project,
                json!({"action":"replace_planning_request","sprintId":id,"description":"Keep the owner's large request"})).await;
            assert_eq!(unchanged["code"], "planning_change_request_required");
            let change = recover_sprint(&service, &project,
                json!({"action":"replace_planning_request","sprintId":id,"description":"Plan only the first milestone"})).await;
            assert_eq!(change["ok"], true, "{change}");
            service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
            let changed = started_draft(&service, &runtime, &project, &id).await;
            assert_ne!(changed, replacement);
            let text = draft_text(&runtime, &changed).await;
            assert!(text.contains("Plan only the first milestone"));
            assert!(!text.contains("Retry strategy:"));
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread")]
        async fn a_120_task_plan_in_three_parts_is_assembled_validated_reviewed_and_approved() {
            let (_scratch, service, runtime, project) = planning_fixture("sprint-parts").await;
            let mut jobs = Jobs::default();
            let id = task_sprint(&service, &project,
                json!({"action":"create","description":"A large sprint","runMode":"manual","checkout":{"kind":"main"}})).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let draft = sprint(&service, &project, &id).await["planningTaskId"]
                .as_str()
                .unwrap()
                .to_owned();
            let task = settled(&runtime, &draft).await;
            // The coordinator's turn: planning again, no worker running.
            let t = task.clone();
            runtime.host.executor.write(move |db| {
                db.execute("UPDATE pending_questions SET status='cancelled' WHERE task_id=? AND status='open'", [&t])?;
                db.execute("UPDATE runtime_tasks SET state='PLANNING',blocker=NULL WHERE task_id=?", [&t])?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            // Items t1..t120; t10 depends on t90, which only the third part defines.
            let part = |range: std::ops::RangeInclusive<usize>, cycle: bool| -> Value {
                range.map(|n| {
                    let deps = match n {
                        10 => json!(["t90"]),
                        90 if cycle => json!(["t10"]),
                        n if n > 1 && n != 11 => json!([format!("t{}", n - 1)]),
                        _ => json!([]),
                    };
                    json!({"key":format!("t{n}"),"title":format!("Item {n}"),"description":format!("Do item {n}"),"dependsOn":deps})
                }).collect::<Vec<_>>().into()
            };
            let first = submit(
                &runtime,
                &task,
                json!({"request_key":"p1","part":1,"final":false,"tasks":part(1..=40, false)}),
            )
            .await;
            assert_eq!(
                first,
                json!({"recorded":true,"part":1,"final":false,"tasks":40})
            );
            let early = submit(&runtime, &task, json!({"request_key":"p3-early","part":3,"final":true,"tasks":part(81..=120, false)})).await;
            assert_eq!(early["code"], "sprint_plan_part_out_of_order", "{early}");
            let second = submit(
                &runtime,
                &task,
                json!({"request_key":"p2","part":2,"final":false,"tasks":part(41..=80, false)}),
            )
            .await;
            assert_eq!(second["tasks"], 80);
            // A key repeated across parts and a cycle across parts are refused, naming the task.
            let mut repeated = part(81..=120, false);
            repeated[0]["key"] = json!("t5");
            let duplicate = submit(
                &runtime,
                &task,
                json!({"request_key":"p3-dup","part":3,"final":true,"tasks":repeated}),
            )
            .await;
            assert_eq!(
                (duplicate["code"].as_str(), duplicate["detail"].as_str()),
                (Some("invalid_task"), Some("task 81 (t5): duplicate key"))
            );
            let cycle = submit(&runtime, &task, json!({"request_key":"p3-cycle","part":3,"final":true,"tasks":part(81..=120, true)})).await;
            assert_eq!(cycle["code"], "invalid_dependencies", "{cycle}");
            assert_eq!(
                cycle["detail"],
                "task 10 (t10) is part of or blocked by a dependency cycle"
            );
            // The corrected final part completes the whole plan and schedules its review.
            let done = submit(
                &runtime,
                &task,
                json!({"request_key":"p3","part":3,"final":true,"tasks":part(81..=120, false)}),
            )
            .await;
            assert_eq!(
                done,
                json!({"recorded":true,"tasks":120,"review":"scheduled"})
            );
            let t = task.clone();
            let (state, objective): (String, String) = runtime.host.executor.read(move |db| {
                Ok(db.query_row("SELECT t.state,n.objective FROM runtime_tasks t JOIN runtime_nodes n USING(task_id)
                    WHERE t.task_id=? AND n.node_id='_plan_review_1'", [&t], |r| Ok((r.get(0)?, r.get(1)?)))?)
            }).unwrap().await.unwrap().unwrap();
            assert_eq!(state, "REVIEWING");
            assert!(
                objective.contains("\"t1\"") && objective.contains("\"t120\""),
                "the review covers the whole plan"
            );
            // A later part without a first part belongs to no plan.
            let orphan = submit(
                &runtime,
                &task,
                json!({"request_key":"orphan","part":2,"final":true,"tasks":part(1..=1, false)}),
            )
            .await;
            assert_eq!(orphan["code"], "sprint_plan_part_out_of_order");
            // Approved, verified and completed: the sprint proposes all 120 tasks.
            complete(&runtime, &task, None).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let ready = sprint(&service, &project, &id).await;
            assert_eq!(ready["planning"], "ready", "{}", ready["conversation"]);
            let items = ready["plan"]["items"].as_array().unwrap();
            assert_eq!(items.len(), 120);
            assert_eq!(items[9]["dependsOn"], json!(["t90"]));
            assert_eq!(items[119]["key"], "t120");
            call(&service, "product.sprint", &project,
                json!({"action":"patch","sprintId":id,"patch":{"planning":"created","materializedVersion":1,"taskIds":[]}})).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let board = call(&service, "product.board", &project, json!({})).await;
            assert_eq!(board["tasks"][&draft]["status"], "done");
            service.stop().await.unwrap();
        }

        #[test]
        fn planning_steps_follow_the_planning_task() {
            let fact = |state: &str, completion: i64, plan: Option<(i64, Value)>| PlanningFact {
                execution: Some("e".into()),
                runtime_task: Some("rt".into()),
                state: Some(state.into()),
                failure: PlanningFailure::Unknown,
                completion: Some(completion),
                plan,
            };
            let base = json!({"description":"Two parts","conversation":[],"planning":"planning","plan":null});
            // Still working: nothing to do.
            assert_eq!(
                planning_step(&base, &fact("EXECUTING", 0, None)),
                PlanningStep::Wait
            );
            // Finished with a valid plan.
            let PlanningStep::Ready { items, run } =
                planning_step(&base, &fact("COMPLETED", 9, Some((5, two_parts()))))
            else {
                panic!()
            };
            assert_eq!(items.len(), 2);
            assert_eq!(
                (run["handled"].clone(), run["planEvent"].clone()),
                (json!(9), json!(5))
            );
            // An invalid plan (only reachable from older data) is never offered: precise reason.
            let cycle = json!([{"key":"a","description":"A","dependsOn":["b"]},{"key":"b","description":"B","dependsOn":["a"]}]);
            let PlanningStep::Cancel { note, .. } =
                planning_step(&base, &fact("COMPLETED", 9, Some((5, cycle))))
            else {
                panic!()
            };
            assert_eq!(note, "The planning task submitted an invalid plan (invalid_dependencies): task 1 (a) is part of or blocked by a dependency cycle.");
            // Finished without a plan: one correction, then cancellation.
            let PlanningStep::Correct { run, .. } =
                planning_step(&base, &fact("COMPLETED", 9, None))
            else {
                panic!()
            };
            let mut corrected = base.clone();
            corrected["planningRun"] = run;
            assert_eq!(
                planning_step(&corrected, &fact("COMPLETED", 9, None)),
                PlanningStep::Wait
            );
            let PlanningStep::Cancel { run, .. } =
                planning_step(&corrected, &fact("COMPLETED", 12, None))
            else {
                panic!()
            };
            let mut cancelled = corrected.clone();
            cancelled["planningRun"] = run;
            cancelled["planning"] = json!("cancelled");
            assert_eq!(
                planning_step(&cancelled, &fact("COMPLETED", 12, None)),
                PlanningStep::Wait
            );
            cancelled["planning"] = json!("planning");
            assert!(matches!(
                planning_step(&cancelled, &fact("COMPLETED", 12, None)),
                PlanningStep::Continue { .. }
            ));
            // A consumed plan is not offered twice; a turn without a new plan keeps the proposal.
            let mut ready = base.clone();
            ready["plan"] = json!({"version":1,"items":two_parts()});
            ready["planningRun"] = json!({"runtimeTask":"rt","handled":9,"outcome":"ready","corrections":0,"planEvent":5});
            assert_eq!(
                planning_step(&ready, &fact("COMPLETED", 9, Some((5, two_parts())))),
                PlanningStep::Wait
            );
            assert!(matches!(
                planning_step(&ready, &fact("COMPLETED", 14, Some((5, two_parts())))),
                PlanningStep::Keep { .. }
            ));
            assert!(matches!(
                planning_step(&ready, &fact("COMPLETED", 14, Some((13, two_parts())))),
                PlanningStep::Ready { .. }
            ));
            // A stopped task cancels planning once; Resume planning then starts a new task.
            let PlanningStep::Terminal { run, .. } =
                planning_step(&base, &fact("FAILED", 0, None))
            else {
                panic!()
            };
            let mut stopped = base.clone();
            stopped["planningRun"] = run;
            stopped["planning"] = json!("cancelled");
            assert_eq!(
                planning_step(&stopped, &fact("FAILED", 0, None)),
                PlanningStep::Wait
            );
            stopped["planning"] = json!("planning");
            assert!(matches!(
                planning_step(&stopped, &fact("FAILED", 0, None)),
                PlanningStep::Replace { .. }
            ));
            // A new execution of the task is observed afresh.
            let mut other = ready.clone();
            other["planningRun"]["runtimeTask"] = json!("old");
            assert!(matches!(
                planning_step(&other, &fact("COMPLETED", 9, Some((5, two_parts())))),
                PlanningStep::Ready { .. }
            ));
        }

        #[test]
        fn a_sole_planned_task_keeps_the_owner_requirements() {
            let sprint = json!({"description":"Ask me before choosing the database.",
                "conversation":[{"kind":"answer","text":"Use SQLite"},{"kind":"event","text":"x"}]});
            let items = finalize_items(
                &sprint,
                vec![json!({"key":"t1","description":"Build it","dependsOn":[]})],
            )
            .unwrap();
            let text = items[0]["description"].as_str().unwrap();
            assert!(text.starts_with("Build it\n\nOriginal owner requirements"));
            assert!(text.contains("Ask me before choosing the database."));
            assert!(text.ends_with("Owner follow-up:\nUse SQLite"));
            let long = json!({"description":"x".repeat(32_000),"conversation":[]});
            assert!(finalize_items(
                &long,
                vec![json!({"key":"t1","description":"D","dependsOn":[]})]
            )
            .is_err());
            let two = finalize_items(&sprint, two_parts().as_array().unwrap().clone()).unwrap();
            assert_eq!(two[0]["description"], "Do the first part");
        }

        #[tokio::test]
        async fn an_automatic_queue_start_binds_the_runtime_policy() {
            let root =
                std::env::temp_dir().join(format!("auto-queue-{}", crate::identity().unwrap()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .unwrap();
            let _scratch = Scratch(root.clone());
            let folder = root.join("project");
            std::fs::create_dir(&folder).unwrap();
            let service = Service::bind_native(
                Config {
                    runtime_dir: root.join("runtime"),
                    state_dir: root.join("state"),
                    allowed_root: root.clone(),
                    launch: LaunchSpec::bare(),
                    request_timeout: Duration::from_secs(5),
                    shutdown_timeout: Duration::from_secs(2),
                    event_capacity: 64,
                },
                Arc::new(Peer(None)),
            )
            .await
            .unwrap();
            let registered = service.register(&folder, "Automatic queue").await.unwrap();
            let project = registered["projectId"].as_str().unwrap().to_owned();
            service.activate(&project).await.unwrap();
            let runtime = service.state.lock().await.native[&project].clone();
            let id = call(&service, "product.sprint", &project,
                json!({"action":"create","description":"An automatic sprint","runMode":"automatic"}))
                .await["sprintId"].clone();
            let draft = service
                .workspace(json!({"protocol":{"major":1,"minor":8},
                "requestId":"draft","projectId":project,"actorId":actor(),
                "operation":"tasks.create","target":project,"expectedVersion":"0","ownerEpoch":null,
                "payload":{"description":"Implement the plan","checkout":{"kind":"main"}}}))
                .await
                .unwrap()["target"]
                .clone();
            call(
                &service,
                "product.task",
                &project,
                json!({"action":"sprint","taskId":draft,"sprintId":id,"dependencies":[]}),
            )
            .await;
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"patch","sprintId":id,
                "patch":{"planning":"created","queueState":"running","taskIds":[draft]}}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut Jobs::default(), false)
                .await
                .unwrap();
            let tasks = service.native_tasks(&project).await.unwrap();
            assert_eq!(
                tasks[draft.as_str().unwrap()]["automaticContinuation"]["policy"],
                cortex_core::lifecycle::automatic::POLICY
            );
            service.stop().await.unwrap();
        }

        #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
        async fn sprint_owner_writes_survive_sustained_contention() {
            let (_scratch, folder, service, runtime, project, sprint_id, _) =
                automatic_fixture("dev", "main", "main").await;
            let create = |request: String, description: &str| {
                json!({"protocol":{"major":1,"minor":8},"requestId":request,
                    "projectId":project,"actorId":actor(),"operation":"tasks.create",
                    "target":project,"expectedVersion":"0","ownerEpoch":null,
                    "payload":{"description":description,"checkout":{"kind":"main"}}})
            };
            let mut live = Vec::new();
            for index in 0..4 {
                let draft = service
                    .workspace(create(format!("load-live-{index}"), "Sprint owner load hold"))
                    .await
                    .unwrap();
                let task = draft["target"].as_str().unwrap().to_owned();
                call(&service, "product.task", &project, json!({
                    "action":"sprint","taskId":task,"sprintId":sprint_id,"dependencies":[]
                })).await;
                assert_eq!(service.workspace(json!({"protocol":{"major":1,"minor":8},
                    "requestId":format!("load-start-{index}"),"projectId":project,"actorId":actor(),
                    "operation":"tasks.start","target":task,"expectedVersion":draft["entityVersion"],
                    "ownerEpoch":null,"payload":{}})).await.unwrap()["outcome"], "recorded");
                let bound = task.clone();
                // An exact link retry after Start confirms the committed membership
                // without resetting the task or touching its active execution.
                call(&service, "product.task", &project, json!({
                    "action":"sprint","taskId":task,"sprintId":sprint_id,"dependencies":[]
                })).await;
                live.push(runtime.host.executor.read(move |db| {
                    Ok(db.query_row("SELECT execution_id FROM workspace_tasks WHERE task_id=?",
                        [bound], |row| row.get::<_, String>(0))?)
                }).unwrap().await.unwrap().unwrap());
            }
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if std::fs::read_to_string(folder.join("load-entered"))
                        .is_ok_and(|s| s.lines().count() == live.len()) {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.expect("four real sprint turns held");

            // Offer a different model/effort for later turns while each live peer
            // is still on the original fixture route.
            std::fs::write(folder.join("load-route-enabled"), "").unwrap();
            let settings = json!({"model":"load-model","effort":"high","mode":"guarded","language":"auto"});
            assert_eq!(runtime.routes.task_route(&folder, &settings).unwrap().model, "load-model");

            // Hold the lane *before* a transaction starts. Fill its backlog with
            // read-only jobs so a refusal here cannot originate in SQLite locking.
            let writer = &runtime.host.executor;
            let controls = runtime.controls.lock().await;
            let generic_actions = service.helper.actions.clone().try_acquire_many_owned(4).unwrap();
            let (entered, observed) = tokio::sync::oneshot::channel();
            let (release, gate) = std::sync::mpsc::channel();
            let blocker = writer.publication(move |_| {
                entered.send(()).unwrap();
                gate.recv_timeout(Duration::from_secs(12)).unwrap();
                Ok(())
            }).unwrap();
            observed.await.unwrap();
            let mut queued = Vec::new();
            for _ in 0..129 {
                match writer.publication(|s| s.read(|db| Ok(db.query_row("SELECT 1", [], |r| r.get::<_, i64>(0))?))) {
                    Ok(job) => queued.push(job),
                    Err(error) => {
                        assert_eq!(error.code(), "busy");
                        break;
                    }
                }
            }
            assert_eq!(queued.len(), 128, "the physical writer queue is full");
            let key = crate::native_runtime::key(&folder).unwrap();
            let unlocked = tokio::task::spawn_blocking(move || {
                let store = cortex_core::Store::open(key, cortex_core::OpenMode::RuntimeValidate).unwrap();
                store.write(|_| Ok(())).unwrap();
            });
            tokio::time::timeout(Duration::from_millis(500), unlocked)
                .await.expect("SQLite permits BEGIN IMMEDIATE while executor is full").unwrap();

            let start = tokio::time::Instant::now();
            let last_background_ticket = writer.submitted();
            let mut requests = tokio::task::JoinSet::new();
            for (index, execution) in live.iter().enumerate() {
                let mut update = json!({"projectId":project,"executionId":execution,
                    "requestId":format!("load-settings-{index}"),"action":"task_settings","settings":settings});
                // First change the model, then change only effort on this same active task.
                update["settings"]["effort"] = json!("");
                let draft = create(format!("load-new-{index}"), "Created during the held sprint");
                let svc = service.clone();
                let (project_id, sprint) = (project.clone(), sprint_id.clone());
                requests.spawn(async move {
                    let preflight = call(&svc, "product.read", &project_id, json!({})).await;
                    let ready = preflight["sprints"].as_array().unwrap().iter()
                        .any(|row| row["id"] == sprint && row["planning"] == "created");
                    assert!(ready, "desktop sprint preflight must succeed while saturated");
                    let result = svc.workspace(draft.clone()).await;
                    if let Ok(receipt) = &result {
                        if receipt["outcome"] == "recorded" {
                            call(&svc, "product.task", &project_id, json!({
                                "action":"sprint","taskId":receipt["target"],"sprintId":sprint,"dependencies":[]
                            })).await;
                        }
                    }
                    vec![(false, draft, result, start.elapsed())]
                });
                let svc = service.clone();
                requests.spawn(async move {
                    let result = svc.control_execution(update.clone()).await;
                    let mut effort = update.clone();
                    effort["requestId"] = json!(format!("load-effort-{index}"));
                    effort["settings"]["effort"] = json!("high");
                    let model = (true, update, result, start.elapsed());
                    let result = svc.control_execution(effort.clone()).await;
                    vec![model, (true, effort, result, start.elapsed())]
                });
            }
            tokio::time::timeout(Duration::from_secs(1), async {
                while writer.submitted() < last_background_ticket + 5 {
                    tokio::task::yield_now().await;
                }
            }).await.expect("four draft jobs and the first settings lookup admitted while full");
            let admitted_in = start.elapsed();
            let ordinary = writer.publication_wait(|_| panic!("refusal must not execute"));
            let ordinary: cortex_core::CoreResult<()> = ordinary.await;
            assert_eq!(ordinary.unwrap_err().code(), "busy");
            assert!(start.elapsed() >= Duration::from_secs(2));
            assert_eq!(requests.len(), 8);
            let held_for = start.elapsed();
            release.send(()).unwrap();
            blocker.await.unwrap().unwrap();
            for job in queued {
                assert_eq!(job.await.unwrap().unwrap(), 1);
            }
            // Replenished background work also progresses without releasing the unrelated
            // project control lock or generic machine action class.
            let sibling = writer.publication(|store| store.write(|db| {
                db.execute_batch("CREATE TEMP TABLE owner_load_background(done); INSERT INTO owner_load_background VALUES(1);")?;
                Ok(1)
            })).unwrap();
            assert_eq!(sibling.await.unwrap().unwrap(), 1);
            let mut replies = Vec::new();
            let mut counts = [0; 2];
            let mut busy = [0; 2];
            let mut max = [Duration::ZERO; 2];
            tokio::time::timeout(Duration::from_secs(2), async {
                while let Some(reply) = requests.join_next().await {
                    for reply in reply.unwrap() {
                        let lane = usize::from(reply.0);
                        counts[lane] += usize::from(reply.2.is_ok());
                        busy[lane] += usize::from(reply.2.as_ref().is_err_and(|e| e.to_string() == "owner_queue_busy"));
                        max[lane] = max[lane].max(reply.3);
                        replies.push(reply);
                    }
                }
            }).await.expect("bounded owner acknowledgement after the writer was released");
            std::fs::write(folder.join("load-release"), "").unwrap();
            let peer_start = tokio::time::Instant::now();
            let peers = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let executions = live.clone();
                    let settled = writer.read(move |db| {
                        Ok(db.query_row("SELECT count(DISTINCT w.execution_id)
                            FROM workspace_executions w JOIN runtime_events e ON e.task_id=w.runtime_task_id
                            WHERE w.execution_id IN (?1, ?2, ?3, ?4) AND e.type='NATIVE_TURN_SETTLED'
                              AND json_extract(e.payload_json,'$.stop_reason')='end_turn'",
                            rusqlite::params_from_iter(executions), |row| row.get::<_, i64>(0))?)
                    }).unwrap().await.unwrap().unwrap();
                    if settled == live.len() as i64 {
                        break settled;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.expect("all live sibling turns settle durably with unrelated controls still held");
            eprintln!("sprint owner load: {peers}/4 ACP peers settled in {:?}", peer_start.elapsed());
            drop(generic_actions);
            drop(controls);
            assert!(max.iter().all(|elapsed| *elapsed < held_for + Duration::from_secs(2)));
            eprintln!("sprint owner load: admitted={admitted_in:?}; saturated={held_for:?}; \
                background=128+1; create/settings replies={counts:?}; busy={busy:?}; max latency={max:?}");
            let stored = writer.read(|db| {
                Ok((
                    db.query_row("SELECT count(*) FROM workspace_requests WHERE request_id LIKE 'load-new-%'", [], |r| r.get::<_, i64>(0))?,
                    db.query_row("SELECT count(*) FROM runtime_events WHERE type='NATIVE_TASK_SETTINGS'
                        AND json_extract(payload_json,'$.settings.model') = 'load-model'", [], |r| r.get::<_, i64>(0))?,
                ))
            }).unwrap().await.unwrap().unwrap();
            eprintln!("sprint owner load: persisted new drafts and task-local settings={stored:?}");
            let errors: Vec<_> = replies.iter().filter_map(|reply| reply.2.as_ref().err()).collect();
            assert_eq!(counts, [4, 8], "{errors:?}");
            assert_eq!(stored, (4, 8));

            // Exact duplicates acknowledge the retained original and do not repeat it.
            let retry_target = replies.iter().find(|reply| reply.0).unwrap().1.clone();
            for (settings, request, result, _) in replies {
                let receipt = result.unwrap();
                assert_eq!(receipt["outcome"], "recorded", "{receipt}");
                if settings {
                    assert_eq!(service.control_execution(request.clone()).await.unwrap(), receipt);
                    let execution = request["executionId"].as_str().unwrap().to_owned();
                    let persisted = writer.read(move |db| {
                        let mut q = db.prepare("SELECT e.payload_json FROM runtime_events e
                            JOIN workspace_executions w ON w.runtime_task_id=e.task_id
                            WHERE w.execution_id=? AND e.type='NATIVE_TASK_SETTINGS' ORDER BY e.id")?;
                        let rows = q.query_map([execution], |row| row.get::<_, String>(0))?
                            .collect::<rusqlite::Result<Vec<_>>>()?;
                        Ok(rows.into_iter().map(|s| serde_json::from_str::<Value>(&s)).collect::<Result<Vec<_>, _>>()?)
                    }).unwrap().await.unwrap().unwrap();
                    assert_eq!(persisted.len(), 2, "replay must not repeat a settings change");
                    assert_eq!(persisted[0]["settings"]["effort"], "");
                    assert_eq!(persisted[1]["settings"]["effort"], "high");
                    let index = usize::from(request["settings"]["effort"] == "high");
                    assert_eq!(persisted[index], json!({"settings":request["settings"],"activation":"next_use"}));
                    let mut conflicting = request;
                    conflicting["settings"]["language"] = json!("en");
                    assert_eq!(service.control_execution(conflicting).await.unwrap_err().to_string(),
                        "payload_conflict");
                } else {
                    assert_eq!(service.workspace(request.clone()).await.unwrap(), receipt);
                    let request_id = request["requestId"].as_str().unwrap().to_owned();
                    let persisted = writer.read(move |db| {
                        Ok(db.query_row("SELECT r.receipt_json FROM workspace_requests r
                            JOIN workspace_tasks t ON json_extract(r.receipt_json,'$.target')=t.task_id
                            WHERE r.request_id=? AND r.project_id=t.project_id", [request_id],
                            |row| row.get::<_, String>(0))?)
                    }).unwrap().await.unwrap().unwrap();
                    assert_eq!(serde_json::from_str::<Value>(&persisted).unwrap(), receipt);
                    let mut conflicting = request;
                    conflicting["payload"]["description"] = json!("Not a replay");
                    assert_eq!(service.workspace(conflicting).await.unwrap()["outcome"], "payload_conflict");
                    // The desktop links the replayed receipt again; membership is still unique.
                    call(&service, "product.task", &project, json!({
                        "action":"sprint","taskId":receipt["target"],"sprintId":sprint_id,"dependencies":[]
                    })).await;
                }
            }
            let current = sprint(&service, &project, &sprint_id).await;
            assert_eq!(current["taskIds"].as_array().unwrap().len(), 10);
            let drafts = writer.read(|db| {
                Ok(db.query_row("SELECT count(*) FROM workspace_tasks WHERE description='Created during the held sprint'",
                    [], |row| row.get::<_, i64>(0))?)
            }).unwrap().await.unwrap().unwrap();
            assert_eq!(drafts, 4, "duplicates must not create extra drafts");

            // If the separately persisted membership is refused, replay the same
            // create identity and link exactly that task once. Never create again.
            let request = create("load-partial-create".into(), "Retry the existing sprint draft");
            let draft = service.workspace(request.clone()).await.unwrap();
            let link = json!({"action":"sprint","taskId":draft["target"],"sprintId":sprint_id,"dependencies":[]});
            let full = service.helper.sprint_links.clone().try_acquire_many_owned(4).unwrap();
            let refused = tokio::time::timeout(Duration::from_millis(500), service.machine(
                serde_json::from_value(json!({"op":"product.task","projectId":project,"params":link})).unwrap()
            )).await.unwrap().unwrap_err();
            assert_eq!(refused.to_string(), "machine_busy");
            drop(full);
            assert!(!sprint(&service, &project, &sprint_id).await["taskIds"].as_array().unwrap().contains(&draft["target"]));
            assert_eq!(service.workspace(request).await.unwrap(), draft);
            call(&service, "product.task", &project, link.clone()).await;
            call(&service, "product.task", &project, link).await;
            let linked = sprint(&service, &project, &sprint_id).await;
            assert_eq!(linked["taskIds"].as_array().unwrap().len(), 11);
            assert_eq!(linked["taskIds"].as_array().unwrap().iter().filter(|id| **id == draft["target"]).count(), 1);

            // A failure writing the settings must also roll back the admission record.
            writer.write(|db| {
                db.execute_batch("CREATE TEMP TRIGGER load_abort_setting BEFORE INSERT ON runtime_events
                    WHEN NEW.type='NATIVE_TASK_SETTINGS' BEGIN SELECT RAISE(ABORT, 'fixture failure'); END")?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            let mut uncommitted = retry_target;
            uncommitted["requestId"] = json!("load-settings-unavailable");
            uncommitted["settings"]["model"] = json!("not-an-offered-model");
            assert_eq!(service.control_execution(uncommitted.clone()).await.unwrap_err().to_string(),
                "native_task_route_unavailable");
            uncommitted["settings"]["model"] = json!("load-model");
            uncommitted["requestId"] = json!("load-settings-rollback");
            uncommitted["settings"]["language"] = json!("en");
            let full = runtime.settings_slots.clone().try_acquire_many_owned(8).unwrap();
            assert_eq!(tokio::time::timeout(Duration::from_millis(500), service.control_execution(uncommitted.clone()))
                .await.unwrap().unwrap_err().to_string(), "owner_queue_busy");
            drop(full);
            assert!(service.control_execution(uncommitted.clone()).await.is_err());
            let rollback_count = writer.read(|db| {
                Ok(db.query_row("SELECT count(*) FROM runtime_requests WHERE request_key='load-settings-rollback'",
                    [], |row| row.get::<_, i64>(0))?)
            }).unwrap().await.unwrap().unwrap();
            assert_eq!(rollback_count, 0, "failed local change must not leave partial admission");
            writer.write(|db| {
                db.execute_batch("DROP TRIGGER load_abort_setting")?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            let retry = service.control_execution(uncommitted.clone()).await.unwrap();
            assert_eq!(retry["outcome"], "recorded");
            assert_eq!(service.control_execution(uncommitted).await.unwrap(), retry);
            eprintln!("sprint owner load: one refused link reconciled without a duplicate; \
                one settings rollback retried and replayed");
            service.stop().await.unwrap();
        }

        fn git_fixture(root: &Path, args: &[&str]) -> String {
            let out = std::process::Command::new("/usr/bin/git")
                .current_dir(root)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .args(args)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().into()
        }

        async fn automatic_fixture(
            target: &str,
            sprint_checkout: &str,
            task_checkout: &str,
        ) -> (
            Scratch,
            std::path::PathBuf,
            Arc<Service>,
            Arc<Runtime>,
            String,
            Value,
            Vec<String>,
        ) {
            let root = std::env::temp_dir()
                .join(format!("automatic-finish-{}", crate::identity().unwrap()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&root)
                .unwrap();
            let folder = root.join("project");
            std::fs::create_dir(&folder).unwrap();
            let scratch = Scratch(root.clone());
            git_fixture(&folder, &["init", "-q", "-b", "dev"]);
            git_fixture(&folder, &["config", "user.name", "Queue fixture"]);
            git_fixture(
                &folder,
                &["config", "user.email", "fixture@example.invalid"],
            );
            git_fixture(&folder, &["config", "commit.gpgsign", "false"]);
            std::fs::write(folder.join("work.txt"), "before\n").unwrap();
            git_fixture(&folder, &["add", "work.txt"]);
            git_fixture(&folder, &["commit", "-q", "-m", "Base"]);
            git_fixture(&folder, &["branch", "main"]);
            let service = Service::bind_native(
                Config {
                    runtime_dir: root.join("runtime"),
                    state_dir: root.join("state"),
                    allowed_root: root,
                    launch: LaunchSpec::bare(),
                    request_timeout: Duration::from_secs(5),
                    shutdown_timeout: Duration::from_secs(2),
                    event_capacity: 64,
                },
                Arc::new(Peer(None)),
            )
            .await
            .unwrap();
            let registered = service.register(&folder, "Queue fixture").await.unwrap();
            let project = registered["projectId"].as_str().unwrap().to_owned();
            service.activate(&project).await.unwrap();
            let runtime = service.state.lock().await.native[&project].clone();
            // These fixtures drive every scheduler pass themselves: the tick alone
            // continues a stopped task, never a background follower.
            runtime
                .followers
                .store(false, std::sync::atomic::Ordering::SeqCst);
            let id = call(
                &service,
                "product.sprint",
                &project,
                json!({
                    "action":"create","description":"Finish two tasks","runMode":"automatic",
                    "targetBranch":target,"checkout":{"kind":sprint_checkout}
                }),
            )
            .await["sprintId"]
                .clone();
            let mut tasks = Vec::<String>::new();
            for index in 0..2 {
                let created = service.workspace(json!({"protocol":{"major":1,"minor":8},
                    "requestId":format!("draft-{index}"),"projectId":project,"actorId":actor(),
                    "operation":"tasks.create","target":project,"expectedVersion":"0","ownerEpoch":null,
                    "payload":{"description":"Implement a part","checkout":{"kind":task_checkout}}
                })).await.unwrap();
                let task = created["target"].as_str().unwrap().to_owned();
                call(
                    &service,
                    "product.task",
                    &project,
                    json!({"action":"sprint","taskId":task,
                    "sprintId":id,"dependencies":tasks}),
                )
                .await;
                tasks.push(task);
            }
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"patch","sprintId":id,
                "patch":{"planning":"created","queueState":"running","taskIds":tasks}}),
            )
            .await;
            (scratch, folder, service, runtime, project, id, tasks)
        }

        #[tokio::test]
        async fn sprint_live_priority_fences_stale_queue_claims_and_decisions() {
            for (mode, checkout) in [("automatic", "main"), ("hybrid", "main"), ("hybrid", "worktree")] {
                let (scratch, folder, service, runtime, project, sprint_id, ids) =
                    automatic_fixture("dev", checkout, checkout).await;
                if checkout == "worktree" {
                    let reply = service.machine(serde_json::from_value(json!({
                        "op":"machine.update","params":{"patch":{"worktreeRoot":scratch.0.join("trees")}}
                    })).unwrap()).await.unwrap();
                    assert_eq!(reply["ok"], true, "{reply}");
                }
                let sprint_id = sprint_id.as_str().unwrap().to_owned();
                call(&service, "product.sprint", &project, json!({"action":"patch","sprintId":sprint_id,
                    "patch":{"runMode":mode,"materializedVersion":1}})).await;
                call(&service, "product.sprint", &project, json!({"action":"patch","sprintId":sprint_id,
                    "patch":{"queueState":"running"}})).await;
                let read = || call(&service, "product.sprint", &project,
                    json!({"action":"live_graph","sprintId":sprint_id}));
                let initial = read().await;
                assert_eq!(initial["order"]["notStarted"][0]["taskId"], ids[0]);
                assert_eq!(initial["order"]["notStarted"][0]["next"], true);
                let old = runtime.sprint_queue_snapshot(project.clone(), sprint_id.clone()).await.unwrap();

                let moved = call(&service, "product.sprint", &project, json!({
                    "action":"move_priority","sprintId":sprint_id,"taskId":ids[1],
                    "direction":"up","expectedGraphRevision":initial["graphRevision"],
                })).await;
                assert_eq!(moved["changed"], true);
                let moved = read().await;
                assert_eq!(moved["priority"], json!([ids[1], ids[0]]));
                assert_eq!(moved["members"][0]["taskId"], ids[0]);
                assert_eq!(moved["order"]["notStarted"][0]["taskId"], ids[0], "priority cannot bypass edges");
                let removed = call(&service, "product.sprint", &project, json!({
                    "action":"set_dependency","sprintId":sprint_id,"dependentTaskId":ids[1],
                    "predecessorTaskId":ids[0],"present":false,"expectedGraphRevision":moved["graphRevision"],
                })).await;
                assert_eq!(removed["changed"], true);
                service.start_queue_task(&project, &runtime, &old, &ids[0], mode == "automatic").await.unwrap();
                service.observed_pause(&project, &old, automatic_hold(
                    "settlement_blocked", Some(&ids[0]), "dependencies_blocked", "Old decision", &HoldFacts::default(),
                )).await.unwrap();
                service.complete_queue(&project, &old).await.unwrap();
                let fresh = read().await;
                assert_eq!(fresh["order"]["queueState"], "running");
                assert_eq!(fresh["order"]["notStarted"][0]["taskId"], ids[1]);
                assert_eq!(fresh["order"]["notStarted"][0]["next"], true);
                assert!(service.native_tasks(&project).await.unwrap()[&ids[0]]["executionId"].is_null());

                let newest = runtime.sprint_queue_snapshot(project.clone(), sprint_id.clone()).await.unwrap();
                service.start_queue_task(&project, &runtime, &newest, &ids[1], mode == "automatic").await.unwrap();
                let fact = service.native_tasks(&project).await.unwrap();
                let execution = fact[&ids[1]]["executionId"].clone();
                assert!(execution.is_string(), "{fact:?}");
                if checkout == "worktree" {
                    let trees = git_fixture(&folder, &["worktree", "list", "--porcelain"]);
                    assert!(trees.contains(scratch.0.join("trees").to_str().unwrap()), "{trees}");
                }
                service.start_queue_task(&project, &runtime, &newest, &ids[1], mode == "automatic").await.unwrap();
                let fact = service.native_tasks(&project).await.unwrap();
                assert_eq!(fact[&ids[1]]["executionId"], execution);
                assert!(fact[&ids[0]]["executionId"].is_null());
                let after = read().await;
                assert_eq!(after["order"]["queueState"], "running");
                assert_eq!(after["members"][1]["editable"], false);
                assert!(after["order"]["notStarted"].as_array().unwrap().iter().all(|r| r["next"] == false));
                for (revision, expected) in [(&fresh["graphRevision"], "stale_graph_revision"),
                    (&after["graphRevision"], "sprint_member_read_only")]
                {
                    let reply = service.machine(serde_json::from_value(json!({"op":"product.sprint","projectId":project,
                        "params":{"action":"set_dependency","sprintId":sprint_id,"dependentTaskId":ids[1],
                            "predecessorTaskId":ids[0],"present":true,"expectedGraphRevision":revision}})).unwrap()).await.unwrap();
                    assert_eq!(reply["code"], expected, "{reply}");
                }
                service.stop().await.unwrap();
            }
        }

        #[tokio::test]
        async fn sprint_live_projection_is_suggested_in_manual_and_never_starts_work() {
            let (_scratch, _folder, service, _runtime, project, id, ids) =
                automatic_fixture("dev", "main", "main").await;
            let read = || call(&service, "product.sprint", &project,
                json!({"action":"live_graph","sprintId":id}));
            assert_eq!(read().await["order"]["notStarted"][0]["next"], true);
            let runtime = service.state.lock().await.native.remove(&project).unwrap();
            let inactive = read().await;
            assert_eq!(inactive["order"]["notStarted"][0]["next"], false);
            assert_eq!(inactive["order"]["notStarted"][0]["conditional"], true);
            service.state.lock().await.native.insert(project.clone(), runtime);
            call(&service, "product.sprint", &project, json!({"action":"patch","sprintId":id,
                "patch":{"runMode":"manual","materializedVersion":1}})).await;
            let graph = call(&service, "product.sprint", &project, json!({"action":"live_graph","sprintId":id})).await;
            assert_eq!(graph["order"]["mode"], "manual");
            assert_eq!(graph["order"]["notStarted"][1]["taskId"], ids[1]);
            assert_eq!(graph["order"]["notStarted"][1]["rank"], 2);
            assert_eq!(graph["order"]["notStarted"][0]["next"], false);
            let facts = service.native_tasks(&project).await.unwrap();
            assert!(facts[&ids[0]]["executionId"].is_null() && facts[&ids[1]]["executionId"].is_null());
            // Explicit Manual Start is still task-local; the service does not require the suggested order.
            let reply = service.workspace(json!({"protocol":{"major":1,"minor":8},"requestId":"manual-b",
                "projectId":project,"actorId":actor(),"operation":"tasks.start","target":ids[1],
                "expectedVersion":facts[&ids[1]]["version"],"ownerEpoch":null,"payload":{}})).await.unwrap();
            assert_eq!(reply["outcome"], "recorded", "{reply}");
            assert!(service.native_tasks(&project).await.unwrap()[&ids[0]]["executionId"].is_null());
            service.stop().await.unwrap();
        }

        async fn settled_peer(runtime: &Runtime, draft: &str) -> String {
            let draft = draft.to_owned();
            let deadline = tokio::time::Instant::now() + Duration::from_secs(12);
            loop {
                let t = draft.clone();
                let task: Option<String> = runtime.host.executor.read(move |db| {
                    use rusqlite::OptionalExtension;
                    Ok(db.query_row("SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e USING(execution_id)
                        JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE w.task_id=?
                        AND r.state IN ('WAITING_USER','FAILED','CANCELLED','BLOCKED','RECOVERING')
                        AND NOT EXISTS(SELECT 1 FROM capacity_reservations c WHERE c.task_id=r.task_id AND c.state!='released')",
                        [&t], |r| r.get(0)).optional()?)
                }).unwrap().await.unwrap().unwrap();
                if let Some(task) = task {
                    return task;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "the fake ACP peer did not settle"
                );
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }

        // Inject the fake peer's completed result at the retained-result boundary.
        // Use the real checkout digest and an actual local check; none of this
        // unit fixture is live provider or release qualification.
        async fn verified_fixture(runtime: &Runtime, task: String, attributed: bool) {
            verified_fixture_at(runtime, task, attributed, b"reviewed changes\n").await;
        }

        async fn verified_fixture_at(
            runtime: &Runtime,
            task: String,
            attributed: bool,
            checked_bytes: &[u8],
        ) {
            use sha2::{Digest, Sha256};
            let root = &runtime.host.root;
            let bytes = b"reviewed changes\n";
            std::fs::write(root.join("work.txt"), bytes).unwrap();
            if checked_bytes != bytes {
                std::fs::write(root.join("work.txt"), checked_bytes).unwrap();
            }
            assert!(std::process::Command::new("/usr/bin/test")
                .current_dir(root)
                .args(["-s", "work.txt"])
                .status()
                .unwrap()
                .success());
            let sha = format!("{:x}", Sha256::digest(bytes));
            let digest = crate::native_effects::observe(root.clone())
                .await
                .unwrap()
                .project_digest
                .unwrap();
            runtime.host.executor.publication(move |store| {
                let dir = store.key().directory().join(&task);
                std::fs::create_dir_all(&dir)?;
                let work = "# Execution evidence\n\nThe fixture changed work.txt.\n";
                let review = "# Independent review\n\nThe fixture's change is approved.\n";
                std::fs::write(dir.join("work.md"), work)?;
                std::fs::write(dir.join("review.md"), review)?;
                store.write(|db| {
                    for (id, filename) in [("finish_work", "work.md"), ("finish_review", "review.md")] {
                        db.execute("INSERT INTO reports VALUES (?,?,'report',?,'now')", (id, &task, filename))?;
                    }
                    db.execute("INSERT INTO runtime_nodes VALUES (?,0,'fixture-execution','worker-backend-dev','Work','[]','[\"work.txt\"]','execution','completed','fixture-execution',?)",
                        (&task, json!({"report_ref":"finish_work","report_digest":format!("{:x}", Sha256::digest(work)),"checks":[]}).to_string()))?;
                    db.execute("INSERT INTO runtime_nodes VALUES (?,0,'fixture-review','worker-code-reviewer','Review','[\"fixture-execution\"]','[]','review','completed','fixture-review',?)",
                        (&task, json!({"report_ref":"finish_review","report_digest":format!("{:x}", Sha256::digest(review)),"verdict":"APPROVE","checks":[]}).to_string()))?;
                    if attributed {
                        db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,owner_epoch,issued_at)
                            VALUES ('fixture-write','hash',?,'fixture-worker','worker',1,'now')", [&task])?;
                        db.execute("INSERT INTO runtime_requests VALUES (?,'fixture-key','fixture-write','project_edit','digest',?)",
                            (&task, json!({"complete":true,
                                "reply":{"structuredContent":{"path":"work.txt","sha256":sha}}}).to_string()))?;
                        cortex_core::lifecycle::event_tx(db, &task, "NATIVE_PROJECT_WRITTEN",
                            json!({"channel_id":"fixture-write","path":"work.txt","sha256":sha}))?;
                    }
                    let argv = json!(["/usr/bin/test", "-s", "work.txt"]);
                    db.execute("UPDATE runtime_tasks SET state='COMPLETED', revision=0, blocker=NULL,
                        synthesis='Verified fixture result.', plan_json=? WHERE task_id=?",
                        (json!({"verification":[argv]}).to_string(), &task))?;
                    cortex_core::lifecycle::event_tx(db, &task, "VERIFICATION_PASSED",
                        json!({"revision":0,"revision_digest":digest,"checks":[{"argv":argv,"complete":true,
                            "exit_code":0,"revision_digest":digest}]}))?;
                    Ok(())
                })
            }).unwrap().await.unwrap().unwrap();
        }

        fn merge_request(project: &str, execution: &Value, conflict: &Value, action: &str, id: &str) -> Value {
            json!({"projectId":project,"executionId":execution,"conflictId":conflict,"action":action,"requestId":id})
        }

        async fn delivered_fixture_prompt(path: &Path) -> String {
            tokio::time::timeout(Duration::from_secs(10), async {
                loop {
                    if let Ok(text) = std::fs::read_to_string(path) {
                        if text.ends_with('\n') {
                            assert_eq!(text.lines().count(), 1, "request was dispatched more than once");
                            let body: Value = serde_json::from_str(text.trim_end()).unwrap();
                            break body["params"]["prompt"].as_array().unwrap()
                                .iter().filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("\n");
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.expect("the no-paid peer did not receive a coordinator prompt")
        }

        struct MergeFixture {
            _scratch: Scratch,
            folder: std::path::PathBuf,
            target: std::path::PathBuf,
            target_head: String,
            service: Arc<Service>,
            runtime: Arc<Runtime>,
            project: String,
            draft: String,
            task: String,
            execution: Value,
            conflict: Value,
            sprint_id: Value,
            jobs: Jobs,
            old_actions: Value,
        }

        async fn merge_handoff_fixture() -> MergeFixture {
            let (_scratch, folder, service, runtime, project, sprint_id, drafts) =
                automatic_fixture("main", "main", "main").await;
            let draft = &drafts[0];
            let mut jobs = Jobs::default();
            service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
            let task = settled_peer(&runtime, draft).await;
            let target = folder.with_file_name("target");
            git_fixture(&folder, &["worktree", "add", target.to_str().unwrap(), "main"]);
            std::fs::write(target.join("work.txt"), "Target conflict\n").unwrap();
            git_fixture(&target, &["commit", "-qam", "Target changes work"]);
            let target_head = git_fixture(&target, &["rev-parse", "HEAD"]);
            verified_fixture(&runtime, task.clone(), true).await;
            service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
            let board = call(&service, "product.board", &project, json!({})).await;
            let accepted = &board["tasks"][draft]["acceptance"];
            assert!(accepted["digest"].is_string(), "{accepted}");
            let old_actions = call(&service, "product.read", &project, json!({"taskIds":[draft]})).await
                ["tasks"][draft]["reviewActions"].clone();
            let integrate = service.machine(serde_json::from_value(json!({
                "op":"product.integrate","projectId":project,"params":{
                    "confirmed":true,"taskId":draft,"method":"squash","message":"Owner integrates",
                    "digest":accepted["digest"],"requestId":"integration-conflict"}
                })).unwrap()).await.unwrap();
            assert_eq!(integrate["code"], "merge_conflict", "{integrate}");
            let record = call(&service, "product.board", &project, json!({})).await["tasks"][draft].clone();
            assert_eq!(record["conflict"], "conflict", "{record}");
            assert!(record["merge"]["handoff"].is_null());
            assert!(!folder.join(".git/merge-handoff-prompts").exists());
            let conflict = record["merge"]["conflictId"].clone();
            let execution = service.native_tasks(&project).await.unwrap()[draft]["executionId"].clone();
            MergeFixture { _scratch, folder, target, target_head, service, runtime,
                project, draft: draft.clone(), task, execution, conflict, sprint_id, jobs, old_actions }
        }

        #[tokio::test]
        async fn owner_merge_handoff_sends_git_manifest_stop_restores_manual_finish_and_checks_request() {
            let MergeFixture { _scratch, folder, target, target_head, service, runtime, project,
                draft, task, execution, conflict, sprint_id, mut jobs, old_actions } = merge_handoff_fixture().await;
            let (draft, conflict) = (&draft, &conflict);
            let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
            let handoff = merge_request(&project, &execution, conflict, "handoff_merge", "explicit-handoff");
            let started = service.control_execution(handoff.clone()).await.unwrap();
            assert_eq!(started["conflict"], "resolving", "{started}");
            let prompt = delivered_fixture_prompt(&folder.join(".git/merge-handoff-prompts")).await;
            let manifest: Value = serde_json::from_str(prompt.lines()
                .find_map(|line| line.strip_prefix("Merge manifest: ")).expect("manifest on the wire")).unwrap();
            assert_eq!(manifest, json!({
                "sourceRevision":head,"targetRevision":target_head,
                "paths":["work.txt"],"complete":true,"truncated":false
            }));
            assert!(!prompt.contains("Target conflict\n"));
            let replay = service.control_execution(handoff.clone()).await.unwrap();
            assert_eq!(replay["merge"]["handoff"]["requestId"], "explicit-handoff");
            let mut changed = handoff;
            changed["text"] = json!("Different guidance with the same request ID");
            assert_eq!(service.control_execution(changed).await.unwrap_err().to_string(), "payload_conflict");
            let duplicate = merge_request(&project, &execution, conflict, "handoff_merge", "another-handoff");
            assert_eq!(service.control_execution(duplicate).await.unwrap_err().to_string(), "stop_required");

            service.cancel_execution(&project, execution.as_str().unwrap()).await.unwrap();
            let board = call(&service, "product.board", &project, json!({})).await;
            assert_eq!(board["tasks"][draft]["conflict"], "conflict", "{board}");
            assert_eq!(board["tasks"][draft]["merge"]["handoff"]["state"], "stopped");
            assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
            assert_eq!(git_fixture(&folder, &["rev-parse", "MERGE_HEAD"]), target_head);
            let unknown_prompt_count = std::fs::read_to_string(folder.join(".git/merge-handoff-prompts")).unwrap().lines().count();
            let inspected = service.control_execution(merge_request(
                &project, &execution, conflict, "inspect_merge", "inspect-stopped"
            )).await.unwrap();
            assert_eq!(inspected["merge"]["gitState"], "stopped");
            assert_eq!(unknown_prompt_count, 1);
            std::fs::write(folder.join("work.txt"), "Manually resolved\n").unwrap();
            git_fixture(&folder, &["add", "work.txt"]);
            call(&service, "product.task", &project, json!({
                "taskId":draft,"action":"finish_merge","requestId":"manual-finish"
            })).await;
            let resolved = git_fixture(&folder, &["rev-parse", "HEAD"]);
            let record = call(&service, "product.board", &project, json!({})).await["tasks"][draft].clone();
            assert_eq!(record["resolvedBy"], "you");
            assert_eq!(record["merge"]["freshChecks"]["revision"], resolved);
            assert_eq!(record["merge"]["freshChecks"]["state"], "not_requested");
            assert_eq!(record["acceptance"], Value::Null);
            assert_eq!(git_fixture(&target, &["rev-parse", "HEAD"]), target_head);
            // Nothing was sent on Finish or the queue pass while checks await the owner.
            service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap();
            assert_eq!(sprint(&service, &project, &sprint_id).await["automaticQueue"]["code"], "different_target");
            assert!(!folder.join(".git/merge-fresh-check-prompts").exists());
            let old = service.machine(serde_json::from_value(json!({"op":"product.task","projectId":project,
                "params":{"action":"accept","taskId":draft,"digest":old_actions["digest"],"version":old_actions["version"]}}))
                .unwrap()).await.unwrap();
            assert_eq!(old["ok"], false, "{old}");
            let checks = merge_request(&project, &execution, conflict, "request_merge_checks", "explicit-checks");
            let request = service.control_execution(checks.clone()).await.unwrap();
            assert_eq!(request["merge"]["freshChecks"]["state"], "requested");
            let fresh = delivered_fixture_prompt(&folder.join(".git/merge-fresh-check-prompts")).await;
            assert!(fresh.contains(&resolved));
            assert!(!fresh.contains("Merge manifest:"));
            assert_eq!(service.control_execution(checks).await.unwrap()["merge"]["freshChecks"]["state"], "requested");
            let count = runtime.host.executor.read(move |db| {
                Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='USER_STEERING'
                    AND json_extract(payload_json,'$.text') LIKE 'Run a fresh independent review%'", [&task],
                    |row| row.get::<_,i64>(0))?)
            }).unwrap().await.unwrap().unwrap();
            assert_eq!(count, 1);
            assert_eq!(git_fixture(&target, &["rev-parse", "HEAD"]), target_head);
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn owner_merge_handoff_settled_completion_finishes_the_exact_staged_merge() {
            let MergeFixture { _scratch, folder, target, target_head, service, runtime, project,
                draft, task, execution, conflict, .. } = merge_handoff_fixture().await;
            let source = git_fixture(&folder, &["rev-parse", "HEAD"]);
            let started = service.control_execution(merge_request(&project, &execution, &conflict,
                "handoff_merge", "complete-handoff")).await.unwrap();
            let request = started["merge"]["handoff"]["runtimeRequestId"].as_str().unwrap();
            delivered_fixture_prompt(&folder.join(".git/merge-handoff-prompts")).await;
            std::fs::write(folder.join("work.txt"), "Resolved and staged\n").unwrap();
            git_fixture(&folder, &["add", "work.txt"]);
            let staged_tree = git_fixture(&folder, &["write-tree"]);
            let mut updates = service.events.subscribe();

            // Keep coordinator authority until the real ACP reply is recorded.
            // Marking the task COMPLETED first can close the live ACP transport.
            // Fence the next driver step so the fixture can finish before it advances.
            let driver = runtime.host.driver.lock().await;
            std::fs::write(folder.join(".git/release-merge-handoff"), "").unwrap();
            tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    let (task, request) = (task.clone(), request.to_owned());
                    let delivered: bool = runtime.host.executor.read(move |db| {
                        Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events
                            WHERE task_id=? AND type='WORKSPACE_MERGE_HANDOFF_DELIVERED'
                            AND json_extract(payload_json,'$.request')=?)",
                            (&task, &request), |row| row.get(0))?)
                    }).unwrap().await.unwrap().unwrap();
                    if delivered {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }).await.expect("the exact merge handoff reply was not recorded");

            // Only the completed task phase is injected; the service finishes Git.
            let fixture_task = task.clone();
            runtime.host.executor.write(move |db| {
                db.execute("UPDATE pending_questions SET status='cancelled'
                    WHERE task_id=? AND status='open'", [&fixture_task])?;
                db.execute("UPDATE runtime_tasks SET state='COMPLETED', blocker=NULL,
                    synthesis='Staged the resolution.' WHERE task_id=?", [&fixture_task])?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            drop(driver);
            let observed = tokio::time::timeout(Duration::from_secs(30), async {
                loop {
                    let board = call(&service, "product.board", &project, json!({})).await;
                    let record = &board["tasks"][&draft];
                    if record["merge"]["finished"] == true {
                        break record.clone();
                    }
                    loop {
                        use tokio::sync::broadcast::error::RecvError;
                        match updates.recv().await {
                            Ok(update) if update["type"] == "project_state"
                                && update["projectId"] == project => break,
                            Ok(_) => {}
                            Err(RecvError::Lagged(_)) => break,
                            Err(RecvError::Closed) => panic!("workspace updates closed before merge settlement"),
                        }
                    }
                }
            }).await;
            let record = match observed {
                Ok(record) => record,
                Err(_) => {
                    let board = call(&service, "product.board", &project, json!({})).await;
                    let record = &board["tasks"][&draft];
                    let trace = runtime.host.executor.read(move |db| {
                        let phase: String = db.query_row("SELECT state FROM runtime_tasks WHERE task_id=?",
                            [&task], |row| row.get(0))?;
                        let mut stmt = db.prepare("SELECT type FROM runtime_events WHERE task_id=? ORDER BY id DESC LIMIT 20")?;
                        let events = stmt.query_map([&task], |row| row.get::<_,String>(0))?
                            .collect::<Result<Vec<_>,_>>()?;
                        Ok((phase, events))
                    }).unwrap().await.unwrap().unwrap();
                    panic!("merge did not settle: phase/events={trace:?}, conflict={}, handoff={}, Git={}",
                        record["conflict"], record["merge"]["handoff"]["state"], record["merge"]["gitState"]);
                }
            };
            assert_eq!(record["resolvedBy"], "cortex");
            assert_eq!(record["merge"]["handoff"]["state"], "completed");
            assert_eq!(record["merge"]["freshChecks"]["state"], "not_requested");
            assert_eq!(record["acceptance"], Value::Null);
            assert_eq!(git_fixture(&folder, &["show", "-s", "--format=%P", "HEAD"]),
                format!("{source} {target_head}"));
            assert_eq!(git_fixture(&folder, &["show", "-s", "--format=%T", "HEAD"]), staged_tree);
            assert_eq!(git_fixture(&target, &["rev-parse", "HEAD"]), target_head);
            assert!(!folder.join(".git/merge-fresh-check-prompts").exists());
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn owner_merge_handoff_refusal_leaves_the_exact_merge_available_to_abort() {
            let MergeFixture { _scratch, folder, target, target_head, service, project,
                draft, execution, conflict, .. } = merge_handoff_fixture().await;
            let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
            let mut invalid = merge_request(&project, &execution, &conflict, "handoff_merge", "rule");
            invalid["text"] = json!("deny-write: work.txt");
            assert_eq!(service.control_execution(invalid).await.unwrap_err().to_string(), "merge_guidance_write_rule");
            std::fs::write(folder.join(".git/refuse-merge-route"), "").unwrap();
            let refused = merge_request(&project, &execution, &conflict, "handoff_merge", "refused-handoff");
            assert!(service.control_execution(refused).await.is_err());
            let board = call(&service, "product.board", &project, json!({})).await;
            assert_eq!(board["tasks"][&draft]["conflict"], "conflict");
            assert_eq!(board["tasks"][&draft]["merge"]["handoff"]["state"], "refused");
            assert!(!folder.join(".git/merge-handoff-prompts").exists());
            call(&service, "product.task", &project,
                json!({"action":"abort_merge","taskId":draft,"requestId":"abort-refused"})).await;
            assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
            assert_eq!(git_fixture(&target, &["rev-parse", "HEAD"]), target_head);
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn owner_merge_handoff_inspection_of_a_replaced_operation_never_dispatches() {
            let MergeFixture { _scratch, folder, target, target_head, service, project,
                draft, execution, conflict, .. } = merge_handoff_fixture().await;
            git_fixture(&folder, &["merge", "--abort"]);
            git_fixture(&folder, &["commit", "--allow-empty", "-qm", "Unrelated commit"]);
            let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
            let inspected = service.control_execution(merge_request(&project, &execution, &conflict,
                "inspect_merge", "inspect-replaced")).await.unwrap();
            assert_eq!(inspected["merge"]["gitState"], "unknown");
            assert_eq!(inspected["conflict"], "failed");
            assert!(!folder.join(".git/merge-handoff-prompts").exists());
            let reply = service.machine(serde_json::from_value(json!({"op":"product.task","projectId":project,
                "params":{"action":"abort_merge","taskId":draft,"requestId":"abort-replaced"}}))
                .unwrap()).await.unwrap();
            assert_eq!(reply["ok"], false, "{reply}");
            assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
            assert_eq!(git_fixture(&target, &["rev-parse", "HEAD"]), target_head);
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn legacy_merge_external_abort_cannot_restore_the_old_result() {
            let MergeFixture {
                _scratch,
                folder,
                target,
                target_head,
                service,
                runtime,
                project,
                draft,
                task,
                old_actions,
                ..
            } = merge_handoff_fixture().await;
            let clear = |request| json!({
                "op":"product.task", "projectId":project,
                "params":{"action":"abort_merge","taskId":draft,"requestId":request,"discardLegacy":true}
            });
            let bound = service.machine(serde_json::from_value(clear("bound-merge")).unwrap())
                .await.unwrap();
            assert_eq!(bound["code"], "stale_conflict", "{bound}");

            // Pair the original branch-only overlay shape with a real owning execution.
            let product_path = _scratch.0.join("state/product").join(format!("{project}.json"));
            let mut product: Value = serde_json::from_slice(&std::fs::read(&product_path).unwrap()).unwrap();
            let record = &mut product["tasks"][&draft];
            let merge = record["merge"].as_object_mut().unwrap();
            for key in ["conflictId", "kind", "sourceRevision", "targetRevision", "gitState"] {
                assert!(merge.remove(key).is_some());
            }
            let before = merge.get_mut("before").unwrap().as_object_mut().unwrap();
            before.remove("abortSnapshot");
            before.remove("requiresFreshResult");
            // Model the old UI having started a resolution without a typed handoff.
            record["conflict"] = json!("resolving");
            std::fs::write(&product_path, product.to_string()).unwrap();

            let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
            git_fixture(&folder, &["merge", "--abort"]);
            let phase = |phase: &'static str| {
                let task = task.clone();
                runtime.host.executor.write(move |db| {
                    db.execute("UPDATE runtime_tasks SET state=? WHERE task_id=?", (phase, task))?;
                    Ok(())
                }).unwrap()
            };
            phase("REVIEWING").await.unwrap().unwrap();
            let refused = service.machine(serde_json::from_value(clear("review-in-progress")).unwrap())
                .await.unwrap();
            assert_eq!(refused["code"], "stop_required", "{refused}");
            assert_eq!(std::fs::read(&product_path).unwrap(), product.to_string().as_bytes());
            phase("COMPLETED").await.unwrap().unwrap();

            let request = clear("externally-aborted-legacy");
            let reply = service.machine(serde_json::from_value(request.clone()).unwrap()).await.unwrap();
            assert_eq!(reply["ok"], true, "{reply}");
            assert_eq!(service.machine(serde_json::from_value(request).unwrap()).await.unwrap(), reply);
            let product: Value = serde_json::from_slice(&std::fs::read(product_path).unwrap()).unwrap();
            let record = &product["tasks"][&draft];
            assert_eq!(record["conflict"], "none");
            assert_eq!(record["merge"], Value::Null);
            assert_eq!(record["acceptance"], Value::Null);
            assert_eq!(record["requiresFreshResult"]["after"], json!({"task":task,"revision":0}));
            assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
            assert_eq!(git_fixture(&target, &["rev-parse", "HEAD"]), target_head);
            assert!(!folder.join(".git/merge-handoff-prompts").exists());
            assert!(!folder.join(".git/merge-fresh-check-prompts").exists());

            let old = service.machine(serde_json::from_value(json!({
                "op":"product.task","projectId":project,"params":{
                    "action":"accept","taskId":draft,"digest":old_actions["digest"],
                    "version":old_actions["version"]
                }
            })).unwrap()).await.unwrap();
            assert_eq!(old["code"], "review_required", "{old}");
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_repairs_changed_accepted_result_from_saved_hold() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let draft = &tasks[0];
            let task = settled_peer(&runtime, draft).await;
            verified_fixture(&runtime, task.clone(), true).await;
            let accepted = call(
                &service,
                "product.task",
                &project,
                json!({"action":"auto_ready","taskId":draft}),
            )
            .await;
            assert_eq!(accepted["accepted"], true, "{accepted}");
            std::fs::write(folder.join("work.txt"), "changed since acceptance\n").unwrap();
            let expected = sprint(&service, &project, &id).await;
            service
                .automatic_pause(
                    &project,
                    &expected,
                    automatic_hold(
                        "settlement_blocked",
                        Some(draft),
                        "stale_result",
                        "The workspace changed.",
                        &HoldFacts::default(),
                    ),
                )
                .await
                .unwrap();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let after = sprint(&service, &project, &id).await;
            let board = call(&service, "product.board", &project, json!({})).await;
            let repairs: i64 = runtime.host.executor.read(move |db| {
                Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_REPAIR_REQUESTED'", [&task], |r| r.get(0))?)
            }).unwrap().await.unwrap().unwrap();
            assert_eq!(repairs, 1, "persisted hold: {after}");
            assert_eq!(after["queueState"], "running");
            assert!(board["tasks"][draft]["acceptance"].is_null());
            assert_ne!(board["tasks"][draft]["status"], "done");
            assert_eq!(git_fixture(&folder, &["log", "-1", "--format=%s"]), "Base");
            let peer_input = folder.join(".git/automatic-repair-input");
            tokio::time::timeout(Duration::from_secs(8), async {
                while !peer_input.exists() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("assigned coordinator receives the repair instruction");
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(std::fs::read_to_string(peer_input).unwrap(), "repair\n");
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn archive_slow_driver_records_an_unknown_pause() {
            let (_scratch, _folder, service, runtime, project, _id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            service.sprint_pass(&project, &runtime, &mut Jobs::default(), false).await.unwrap();
            let draft = &tasks[0];
            let task = settled_peer(&runtime, draft).await;
            let (cancel, cancelled) = tokio::sync::watch::channel(false);
            let (done, finished) = tokio::sync::watch::channel(false);
            runtime.install_pump(task.clone(), cancel, finished).await;

            // Keep the driver alive through the entire stop budget.
            tokio::time::timeout(Duration::from_secs(9), call(
                &service, "product.task", &project, json!({"action":"archive","taskId":draft}),
            )).await.expect("archive waits only a bounded time for the driver");
            assert!(*cancelled.borrow(), "the admitted Pause asked the driver to stop");
            assert!(!*done.borrow());
            let board = call(&service, "product.board", &project, json!({})).await;
            assert_eq!(board["tasks"][draft]["archive"]["stopUnconfirmed"], true);
            runtime.host.executor.read(move |db| {
                let (state, outcome, reason): (String, String, String) = db.query_row(
                    "SELECT t.state,json_extract(f.result_json,'$.outcome'),json_extract(f.result_json,'$.reason')
                     FROM runtime_tasks t JOIN runtime_requests i USING(task_id)
                     JOIN runtime_requests f USING(task_id,request_key)
                     WHERE t.task_id=? AND i.operation='native_desktop_control'
                     AND i.request_key LIKE 'archive-stop-%' AND f.actor='native-desktop-finish'",
                    [&task], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )?;
                assert_eq!(state, "PAUSED");
                assert_eq!(outcome, "unknown");
                assert_eq!(reason, "archive_stop_timeout");
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            done.send_replace(true);
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn archived_then_restored_result_refuses_stale_repair_dispatch() {
            use crate::machine::task_auto_repair::{Dispatch, RepairPlan, dispatch};
            let (_scratch, _folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            service.sprint_pass(&project, &runtime, &mut Jobs::default(), false).await.unwrap();
            let draft = &tasks[0];
            let task = settled_peer(&runtime, draft).await;
            verified_fixture(&runtime, task.clone(), true).await;
            let accepted = call(&service, "product.task", &project,
                json!({"action":"auto_ready","taskId":draft})).await;
            assert_eq!(accepted["accepted"], true, "{accepted}");
            let selected = call(&service, "product.read", &project, json!({"taskIds":[draft]})).await;
            let version = selected["tasks"][draft]["result"]["sourceVersion"].as_str().unwrap();
            let execution = service.native_tasks(&project).await.unwrap()[draft]["executionId"].clone();
            let expected = sprint(&service, &project, &id).await;
            for action in ["archive", "restore"] {
                call(&service, "product.task", &project, json!({"action":action,"taskId":draft})).await;
            }
            let state = &service.config.state_dir;
            let before = crate::machine::native_product(state, &project).unwrap();
            assert_eq!(before["tasks"][draft]["archiveHold"], "stopped");
            assert_eq!(before["tasks"][draft]["acceptance"]["automatic"], true);
            assert_eq!(sprint(&service, &project, &id).await, expected);
            let admission = dispatch(state, &runtime, &project, &expected, draft,
                execution.as_str().unwrap(), Some(version),
                RepairPlan { prompt: "stale repair".into(), fingerprint: "stale-archive-fixture".into() },
                false).await.unwrap();
            assert!(matches!(admission, Dispatch::Refused(None)));
            assert_eq!(crate::machine::native_product(state, &project).unwrap(), before);
            assert_eq!(repairs_of(&runtime, &task).await, 0);
            assert_eq!(service.native_tasks(&project).await.unwrap()[draft]["phase"], "result");
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn archived_and_restored_hybrid_members_leave_a_paused_sprint_idle() {
            let (_scratch, _folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            call(&service, "product.sprint", &project, json!({"action":"patch","sprintId":id,
                "patch":{"runMode":"hybrid","queueState":"paused"}})).await;
            let mut jobs = Jobs::default();
            // The fixture has poll candidates before they are shelved.
            assert!(service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap());
            for draft in &tasks {
                call(&service, "product.task", &project,
                    json!({"action":"archive","taskId":draft})).await;
            }
            call(&service, "product.task", &project,
                json!({"action":"restore","taskId":tasks[1]})).await;
            let current = sprint(&service, &project, &id).await;
            assert_eq!(current["queueState"], "paused");
            assert_eq!(current["runMode"], "hybrid");
            assert!(!service.sprint_pass(&project, &runtime, &mut jobs, false).await.unwrap());
            service.stop().await.unwrap();
        }

        async fn repairs_of(runtime: &Runtime, task: &str) -> i64 {
            let task = task.to_owned();
            runtime.host.executor.read(move |db| {
                Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_REPAIR_REQUESTED'", [&task], |r| r.get(0))?)
            }).unwrap().await.unwrap().unwrap()
        }

        async fn fail_run(runtime: &Runtime, task: &str, blocker: &str) {
            let (t, b) = (task.to_owned(), blocker.to_owned());
            runtime.host.executor.write(move |db| {
                db.execute("UPDATE runtime_tasks SET state='FAILED',blocker=? WHERE task_id=?", (&b, &t))?;
                // A failed run has no open question (the fake peer's Hybrid turn leaves one).
                db.execute("UPDATE pending_questions SET status='cancelled' WHERE task_id=? AND status='open'", [&t])?;
                cortex_core::lifecycle::event_tx(db, &t, "TASK_STATE_CHANGED", json!({"state":"FAILED","blocker":b}))?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
        }

        /// A run that failed with every effect known (here: an answer the runtime could
        /// not certify because review was missing) goes back to the agent by itself; the
        /// owner is asked only after the bounded repairs, and Allow sends it back once more.
        #[tokio::test]
        async fn automatic_sprint_sends_a_failed_run_back_to_the_agent_then_asks_the_owner() {
            let (_scratch, _folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            let blocker = "answer_not_certified:completion_review_missing";
            fail_run(&runtime, &task, blocker).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let after = sprint(&service, &project, &id).await;
            assert_eq!(repairs_of(&runtime, &task).await, 1, "{after}");
            assert_eq!(after["queueState"], "running", "{after}");
            assert!(after["automaticQueue"].is_null(), "{after}");
            // The same failure again: the deduplicated, bounded repair is spent, so the
            // owner gets one decision with explicit asks and actions.
            let task = settled_peer(&runtime, &tasks[0]).await;
            fail_run(&runtime, &task, blocker).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let held = sprint(&service, &project, &id).await;
            let hold = &held["automaticQueue"];
            assert_eq!(hold["code"], "task_failed", "{held}");
            assert_eq!(hold["nextActor"], "owner");
            assert_eq!(hold["ownerActions"], json!(["allow_repair", "close_as_is"]));
            assert!(!hold["asks"][0].as_str().unwrap().contains('_'), "{hold}");
            assert_eq!(repairs_of(&runtime, &task).await, 1);
            // Another pass without a decision changes nothing and dispatches nothing.
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(sprint(&service, &project, &id).await, held);
            assert_eq!(repairs_of(&runtime, &task).await, 1);
            // A stale decision is refused; the current one is recorded once.
            let stale = service
                .machine(
                    serde_json::from_value(json!({"op":"product.sprint","projectId":project,
                    "params":{"action":"allow","sprintId":id,"taskId":tasks[0],"code":"task_stopped","ownerAction":"allow_repair"}}))
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(stale["code"], "automatic_hold_changed", "{stale}");
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"allow","sprintId":id,"taskId":tasks[0],"code":"task_failed","ownerAction":"allow_repair"}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let resumed = sprint(&service, &project, &id).await;
            assert_eq!(repairs_of(&runtime, &task).await, 2, "{resumed}");
            assert_eq!(resumed["queueState"], "running", "{resumed}");
            assert!(resumed["automaticQueue"].is_null(), "{resumed}");
            service.stop().await.unwrap();
        }

        /// Hybrid follows the same next-actor rule: a failed run goes back to its agent,
        /// and a cancelled prerequisite holds only its dependent, named, with actions.
        #[tokio::test]
        async fn hybrid_sprint_repairs_a_failed_run_and_names_a_blocked_dependent() {
            let (_scratch, _folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"patch","sprintId":id,"patch":{"runMode":"hybrid"}}),
            )
            .await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            fail_run(
                &runtime,
                &task,
                "answer_not_certified:completion_nodes_incomplete",
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let after = sprint(&service, &project, &id).await;
            assert_eq!(repairs_of(&runtime, &task).await, 1, "{after}");
            assert_eq!(after["queueState"], "running", "{after}");
            // The owner cancels the first task: its dependent is held, by name.
            settled_peer(&runtime, &tasks[0]).await;
            call(
                &service,
                "product.task",
                &project,
                json!({"action":"close","taskId":tasks[0],"choice":"cancel","confirmed":true}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let held = sprint(&service, &project, &id).await;
            let hold = &held["automaticQueue"];
            assert_eq!(held["queueState"], "paused", "{held}");
            assert_eq!(hold["code"], "dependencies_blocked", "{held}");
            assert_eq!(hold["taskId"], tasks[1]);
            assert_eq!(hold["nextActor"], "owner");
            assert_eq!(hold["ownerActions"], json!(["close_as_is"]));
            assert!(
                hold["reason"].as_str().unwrap().contains("cancelled"),
                "{hold}"
            );
            // Closing the dependent clears the hold by itself; no Continue queue needed.
            call(
                &service,
                "product.task",
                &project,
                json!({"action":"close","taskId":tasks[1],"choice":"cancel","confirmed":true}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let done = sprint(&service, &project, &id).await;
            assert_eq!(done["queueState"], "complete", "{done}");
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_repairs_content_unknown_without_pausing_or_touching_unrelated_work(
        ) {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            // Capture the verified result with an unrelated staged file already
            // present. Automatic recovery must preserve it while reconciling the
            // task's recorded path.
            std::fs::write(folder.join("unrelated.txt"), "keep staged\n").unwrap();
            git_fixture(&folder, &["add", "unrelated.txt"]);
            verified_fixture_at(
                &runtime,
                task.clone(),
                true,
                b"edited independently before verification\n",
            )
            .await;
            let accepted = call(
                &service,
                "product.task",
                &project,
                json!({"action":"auto_ready","taskId":tasks[0]}),
            )
            .await;
            assert_eq!(accepted["accepted"], true, "{accepted}");

            // The recorded task file becomes unlocatable. Recovery must not commit
            // or drop the unrelated staged file.

            let facts = call(
                &service,
                "product.read",
                &project,
                json!({"taskIds":[tasks[0]]}),
            )
            .await;
            assert_eq!(
                facts["tasks"][&tasks[0]]["integrationFacts"]["reason"],
                "Task files changed again or their recorded content cannot be located. Finish in Git and use Done outside the app, or close as is."
            );

            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let repaired = sprint(&service, &project, &id).await;
            assert_eq!(repaired["queueState"], "running", "{repaired}");
            assert!(repaired["automaticQueue"].is_null(), "{repaired}");
            let repairs: i64 = runtime.host.executor.read(move |db| {
                Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_REPAIR_REQUESTED'", [&task], |r| r.get::<_, i64>(0))?)
            }).unwrap().await.unwrap().unwrap();
            assert_eq!(repairs, 1, "{repaired}");
            let staged = git_fixture(&folder, &["diff", "--cached", "--name-only"]);
            assert_eq!(staged, "unrelated.txt");
            let peer_input = folder.join(".git/automatic-repair-input");
            tokio::time::timeout(Duration::from_secs(8), async {
                while !peer_input.exists() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("assigned coordinator receives the repair instruction");
            let prompt = folder.join(".git/automatic-repair-prompt");
            tokio::time::timeout(Duration::from_secs(8), async {
                while !prompt.exists() {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("repair prompt is retained for the fixture");
            let prompt = std::fs::read_to_string(prompt).unwrap();
            assert!(prompt.contains("work.txt"), "{prompt}");
            assert!(prompt.contains("reconcile its recorded files"), "{prompt}");
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(std::fs::read_to_string(peer_input).unwrap(), "repair\n");
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_finishes_exact_task_paths_then_starts_the_dependent() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            let hook = folder.join(".git/hooks/pre-commit");
            std::fs::write(&hook, "#!/bin/sh\n: > .git/fixture-hook-ran\n").unwrap();
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::write(folder.join("unrelated.txt"), "keep staged\n").unwrap();
            git_fixture(&folder, &["add", "unrelated.txt"]);
            verified_fixture(&runtime, task, true).await;
            let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
            let wire: crate::machine::MachineCall = serde_json::from_value(json!({
                "op":"automatic.finish","projectId":project,"params":{"sprintId":id,"taskId":tasks[0]}
            })).unwrap();
            assert!(
                service.machine(wire).await.is_err(),
                "wire clients cannot invoke scheduler effects"
            );
            assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);

            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let board = call(&service, "product.board", &project, json!({})).await;
            let first = &board["tasks"][&tasks[0]];
            assert_eq!(first["status"], "done", "{first}");
            assert_eq!(first["acceptance"]["automatic"], true);
            assert_eq!(first["integration"]["automatic"], true);
            assert_eq!(first["integration"]["committed"], true);
            assert_eq!(first["integration"]["targetBranch"], "dev");
            assert_eq!(
                git_fixture(&folder, &["show", "--format=", "--name-only", "HEAD"]),
                "work.txt"
            );
            assert_eq!(
                git_fixture(&folder, &["diff", "--cached", "--name-only"]),
                "unrelated.txt"
            );
            assert!(folder.join(".git/fixture-hook-ran").exists());

            let landed = git_fixture(&folder, &["rev-parse", "HEAD"]);
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let projected = service.native_tasks(&project).await.unwrap();
            assert!(projected[&tasks[1]]["executionId"].is_string());
            assert_eq!(
                projected[&tasks[1]]["automaticContinuation"]["policy"],
                "automatic_continuation"
            );
            assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), landed);
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_skips_cancelled_and_archived_members_but_not_dependencies() {
            for archive in [false, true] {
                let (_scratch, folder, service, runtime, project, id, tasks) =
                    automatic_fixture("dev", "main", "main").await;
                let mut jobs = Jobs::default();
                service
                    .sprint_pass(&project, &runtime, &mut jobs, false)
                    .await
                    .unwrap();
                settled_peer(&runtime, &tasks[0]).await;
                call(
                    &service,
                    "product.task",
                    &project,
                    json!({
                        "action":"close","taskId":tasks[0],"choice":"cancel","confirmed":true
                    }),
                )
                .await;
                if archive {
                    call(
                        &service,
                        "product.task",
                        &project,
                        json!({"action":"archive","taskId":tasks[0]}),
                    )
                    .await;
                }
                let board = call(&service, "product.board", &project, json!({})).await;
                assert_eq!(board["tasks"][&tasks[0]]["closedAs"], "cancel");
                assert_eq!(board["tasks"][&tasks[0]]["archive"].is_object(), archive);

                // A third, independent task must run even while the original
                // second member cannot use the cancelled member as a prerequisite.
                let created = service.workspace(json!({"protocol":{"major":1,"minor":8},
                    "requestId":"independent-draft","projectId":project,"actorId":actor(),
                    "operation":"tasks.create","target":project,"expectedVersion":"0","ownerEpoch":null,
                    "payload":{"description":"Independent work","checkout":{"kind":"main"}}
                })).await.unwrap();
                let independent = created["target"].as_str().unwrap();
                call(
                    &service,
                    "product.task",
                    &project,
                    json!({"action":"sprint","taskId":independent,
                    "sprintId":id,"dependencies":[]}),
                )
                .await;
                call(
                    &service,
                    "product.sprint",
                    &project,
                    json!({"action":"patch","sprintId":id,
                    "patch":{"taskIds":[tasks[0],tasks[1],independent]}}),
                )
                .await;

                service
                    .sprint_pass(&project, &runtime, &mut jobs, false)
                    .await
                    .unwrap();
                let projection = service.native_tasks(&project).await.unwrap();
                assert!(projection[independent]["executionId"].is_string());
                assert!(projection[&tasks[1]]["executionId"].is_null());
                let other = settled_peer(&runtime, independent).await;
                verified_fixture(&runtime, other, true).await;
                service
                    .sprint_pass(&project, &runtime, &mut jobs, false)
                    .await
                    .unwrap();
                service
                    .sprint_pass(&project, &runtime, &mut jobs, false)
                    .await
                    .unwrap();
                let held = sprint(&service, &project, &id).await;
                assert_eq!(
                    held["automaticQueue"]["code"], "dependencies_blocked",
                    "{held}"
                );
                assert_eq!(held["automaticQueue"]["needsOwner"], true);
                service
                    .sprint_pass(&project, &runtime, &mut jobs, false)
                    .await
                    .unwrap();
                assert_eq!(sprint(&service, &project, &id).await, held);
                call(
                    &service,
                    "product.task",
                    &project,
                    json!({"action":"close","taskId":tasks[1],
                    "choice":"cancel","confirmed":true}),
                )
                .await;
                service
                    .sprint_pass(&project, &runtime, &mut jobs, false)
                    .await
                    .unwrap();
                assert_eq!(
                    sprint(&service, &project, &id).await["queueState"],
                    "running"
                );
                service
                    .sprint_pass(&project, &runtime, &mut jobs, false)
                    .await
                    .unwrap();
                assert_eq!(
                    sprint(&service, &project, &id).await["queueState"],
                    "complete"
                );
                assert!(
                    service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"]
                        .is_null()
                );
                assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "2");
                service.stop().await.unwrap();
            }
        }

        #[tokio::test]
        async fn automatic_sprint_waits_for_confirmed_cross_branch_integration() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("main", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            verified_fixture(&runtime, task, true).await;
            let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let projected = sprint(&service, &project, &id).await;
            assert_eq!(projected["queueState"], "paused", "{projected}");
            assert_eq!(projected["automaticQueue"]["kind"], "owner_integration");
            assert_eq!(projected["automaticQueue"]["needsOwner"], true);
            assert_eq!(projected["automaticQueue"]["taskId"], tasks[0]);
            assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
            assert_eq!(git_fixture(&folder, &["rev-parse", "main"]), head);
            assert!(
                service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_null()
            );
            let board = call(&service, "product.board", &project, json!({})).await;
            let first = &board["tasks"][&tasks[0]];
            assert_eq!(first["status"], "ready_to_merge", "{first}");
            assert_eq!(first["acceptance"]["automatic"], true);
            let result = call(
                &service,
                "product.integrate",
                &project,
                json!({"confirmed":true,
                "taskId":tasks[0],"method":"squash","message":"Owner-confirmed integration",
                "digest":first["acceptance"]["digest"],"requestId":"owner-finish"}),
            )
            .await;
            assert_eq!(result["done"], true, "{result}");
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(
                sprint(&service, &project, &id).await["queueState"],
                "running"
            );
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert!(
                service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_string()
            );
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_refuses_both_sprint_and_native_draft_worktrees() {
            for (sprint_checkout, task_checkout) in [("worktree", "main"), ("main", "worktree")] {
                let (_scratch, root, service, runtime, project, id, tasks) =
                    automatic_fixture("dev", sprint_checkout, task_checkout).await;
                service
                    .sprint_pass(&project, &runtime, &mut Jobs::default(), false)
                    .await
                    .unwrap();
                let projected = sprint(&service, &project, &id).await;
                assert_eq!(projected["queueState"], "paused", "{projected}");
                assert_eq!(projected["automaticQueue"]["kind"], "start_refused");
                assert_eq!(
                    projected["automaticQueue"]["code"],
                    "native_worktree_unavailable"
                );
                assert_eq!(projected["automaticQueue"]["needsOwner"], true);
                assert!(projected["pauseReason"]
                    .as_str()
                    .unwrap()
                    .contains("separate worktree"));
                assert_eq!(
                    git_fixture(&root, &["worktree", "list", "--porcelain"])
                        .lines()
                        .filter(|line| line.starts_with("worktree "))
                        .count(),
                    1
                );
                assert!(
                    service.native_tasks(&project).await.unwrap()[&tasks[0]]["executionId"]
                        .is_null()
                );
                service.stop().await.unwrap();
            }
        }

        #[tokio::test]
        async fn a_retired_core_pause_is_resumed_by_the_runtime_not_held_for_the_owner() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            let t = task.clone();
            // A pause an earlier version recorded for the owner (an attempt cap).
            runtime.host.executor.write(move |db| {
                db.execute("UPDATE runtime_tasks SET state='RECOVERING',blocker='automatic_continuation_paused' WHERE task_id=?", [&t])?;
                cortex_core::lifecycle::event_tx(db, &t, "AUTOMATIC_CONTINUATION", json!({"reason":"Retry checks"}))?;
                cortex_core::lifecycle::event_tx(db, &t, "AUTOMATIC_PAUSED",
                    json!({"pause_kind":"continuation_cap","reason":"The continuation limit was reached."}))?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            // No-stop policy: the runtime resumes it with a changed strategy by itself.
            let resumed = runtime.host.executor.read(move |db| {
                Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_RESUMED'",
                    [&task], |row| row.get::<_, i64>(0))?)
            }).unwrap().await.unwrap().unwrap();
            assert!(resumed >= 1, "{resumed}");
            let sprint = sprint(&service, &project, &id).await;
            assert_ne!(sprint["automaticQueue"]["needsOwner"], true, "{sprint}");
            assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "1");
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_rechecks_acceptance_when_verification_becomes_available() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            // A result phase alone is not verification evidence. The fixture
            // publishes the passing result only after the queue first refuses it.
            let t = task.clone();
            runtime
                .host
                .executor
                .write(move |db| {
                    db.execute(
                        "UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL WHERE task_id=?",
                        [&t],
                    )?;
                    Ok(())
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let held = sprint(&service, &project, &id).await;
            assert_eq!(
                held["automaticQueue"]["code"], "automatic_acceptance_blocked",
                "{held}"
            );
            assert_eq!(held["automaticQueue"]["needsOwner"], true);
            assert!(
                service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_null()
            );
            assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "1");

            verified_fixture(&runtime, task, true).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(
                sprint(&service, &project, &id).await["queueState"],
                "running"
            );
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "2");
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_requires_owner_finish_after_explicit_acceptance() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            verified_fixture(&runtime, task, true).await;
            let board = call(&service, "product.board", &project, json!({})).await;
            let facts = runtime
                .owner_facts(&project, board, json!({}))
                .await
                .unwrap();
            let review = &facts["product"]["tasks"][&tasks[0]]["reviewActions"];
            assert_eq!(review["accept"], true, "{review}");
            call(&service, "product.task", &project,
                json!({"action":"accept","taskId":tasks[0],"version":review["version"],"digest":review["digest"]})).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let held = sprint(&service, &project, &id).await;
            assert_eq!(
                held["automaticQueue"]["kind"], "owner_integration",
                "{held}"
            );
            assert_eq!(held["automaticQueue"]["code"], "owner_accepted");
            assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "1");
            call(&service, "product.integrate", &project,
                json!({"confirmed":true,"method":"commit","taskId":tasks[0],"digest":review["digest"],
                    "message":"Finish explicitly accepted work","requestId":"owner-commit"})).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(
                sprint(&service, &project, &id).await["queueState"],
                "running"
            );
            assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "2");
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_owner_finish_consent_finishes_and_advances() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            verified_fixture(&runtime, task.clone(), true).await;
            runtime.host.executor.write(move |db| {
                let checks = json!([{"name":"Staging deployment", "outcome":"not_run", "blocker":"Deployment is prohibited by policy."}]);
                db.execute("UPDATE runtime_nodes SET result_json=json_set(result_json,'$.checks',json(?))
                    WHERE task_id=? AND stage='review'", (checks.to_string(), &task))?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(
                sprint(&service, &project, &id).await["automaticQueue"]["code"],
                "automatic_acceptance_blocked"
            );
            let board = call(&service, "product.board", &project, json!({})).await;
            let facts = runtime
                .owner_facts(&project, board, json!({}))
                .await
                .unwrap();
            let review = &facts["product"]["tasks"][&tasks[0]]["reviewActions"];
            assert_eq!(review["acknowledgeOpenChecks"], true, "{review}");
            call(
                &service,
                "product.task",
                &project,
                json!({"action":"accept","taskId":tasks[0],"version":review["version"],
                    "digest":review["digest"],"acknowledgeOpenChecks":true,"automaticFinish":{"sprintId":id,"targetBranch":"dev"}}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let board = call(&service, "product.board", &project, json!({})).await;
            let first = &board["tasks"][&tasks[0]];
            assert_eq!(first["status"], "done", "{first}");
            assert_eq!(first["integration"]["automatic"], true);
            assert_eq!(first["acceptance"]["open"]["not_run"], 1);
            assert_eq!(first["integration"]["targetBranch"], "dev");
            assert_eq!(
                sprint(&service, &project, &id).await["queueState"],
                "running"
            );
            assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "2");
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert!(
                service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_string()
            );
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_owner_finish_consent_completes_manual_owner_acceptance() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            verified_fixture(&runtime, task, true).await;
            let board = call(&service, "product.board", &project, json!({})).await;
            let facts = runtime
                .owner_facts(&project, board, json!({}))
                .await
                .unwrap();
            let review = &facts["product"]["tasks"][&tasks[0]]["reviewActions"];
            call(
                &service,
                "product.task",
                &project,
                json!({"action":"accept","taskId":tasks[0],"version":review["version"],"digest":review["digest"]}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(
                sprint(&service, &project, &id).await["automaticQueue"]["code"],
                "owner_accepted"
            );
            let board = call(&service, "product.board", &project, json!({})).await;
            let acceptance = board["tasks"][&tasks[0]]["acceptance"].clone();
            call(
                &service,
                "product.task",
                &project,
                json!({"action":"accept","taskId":tasks[0],"version":acceptance["version"],
                    "digest":acceptance["digest"],"automaticFinish":{"sprintId":id,"targetBranch":"dev"}}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let board = call(&service, "product.board", &project, json!({})).await;
            assert_eq!(
                board["tasks"][&tasks[0]]["status"],
                "done",
                "task={:?} sprint={:?}",
                board["tasks"][&tasks[0]],
                sprint(&service, &project, &id).await
            );
            assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "2");
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_owner_finish_consent_rejects_stale_target_version_and_mode() {
            let (_scratch, _folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            let t = task.clone();
            runtime
                .host
                .executor
                .write(move |db| {
                    db.execute(
                        "UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL WHERE task_id=?",
                        [&t],
                    )?;
                    Ok(())
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            verified_fixture(&runtime, task, true).await;
            let board = call(&service, "product.board", &project, json!({})).await;
            let facts = runtime
                .owner_facts(&project, board, json!({}))
                .await
                .unwrap();
            let review = facts["product"]["tasks"][&tasks[0]]["reviewActions"].clone();
            let wrong_target = service
                .machine(
                    serde_json::from_value(json!({"op":"product.task","projectId":project,
                        "params":{"action":"accept","taskId":tasks[0],"version":review["version"],
                        "digest":review["digest"],"automaticFinish":{"sprintId":id,"targetBranch":"main"}}}))
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(wrong_target["ok"], false);
            assert_eq!(wrong_target["code"], "stale_result");
            let wrong_version = service
                .machine(
                    serde_json::from_value(json!({"op":"product.task","projectId":project,
                        "params":{"action":"accept","taskId":tasks[0],"version":"wrong",
                        "digest":review["digest"],"automaticFinish":{"sprintId":id,"targetBranch":"dev"}}}))
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(wrong_version["ok"], false);
            assert_eq!(wrong_version["code"], "stale_result");
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"patch","sprintId":id,"patch":{"runMode":"manual"}}),
            )
            .await;
            let wrong_mode = service
                .machine(
                    serde_json::from_value(json!({"op":"product.task","projectId":project,
                        "params":{"action":"accept","taskId":tasks[0],"version":review["version"],
                        "digest":review["digest"],"automaticFinish":{"sprintId":id,"targetBranch":"dev"}}}))
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(wrong_mode["ok"], false);
            assert_eq!(wrong_mode["code"], "automatic_finish_unavailable");
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_owner_close_as_is_releases_acceptance_hold() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            let t = task.clone();
            runtime
                .host
                .executor
                .write(move |db| {
                    db.execute(
                        "UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL WHERE task_id=?",
                        [&t],
                    )?;
                    Ok(())
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let held = sprint(&service, &project, &id).await;
            assert_eq!(held["queueState"], "paused", "{held}");
            assert_eq!(
                held["automaticQueue"]["code"],
                "automatic_acceptance_blocked"
            );
            let before = held["automaticQueue"]["reason"]
                .as_str()
                .unwrap()
                .to_owned();

            // The presentation command cannot release the shared folder while the
            // runtime still lacks the outcome of a dispatched write.
            let held_task = task.clone();
            runtime.host.executor.write(move |db| {
                db.execute("INSERT INTO runtime_requests VALUES (?, 'pending-check', 'fixture', 'project_edit','digest','{}')", [&held_task])?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            let premature = service
                .machine(
                    serde_json::from_value(json!({"op":"product.task","projectId":project,
                "params":{"action":"close","choice":"as_is","confirmed":true,"taskId":tasks[0]}}))
                    .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                premature["code"], "native_outside_outcome_unknown",
                "{premature}"
            );
            assert_eq!(
                sprint(&service, &project, &id).await["queueState"],
                "paused"
            );
            let held_task = task.clone();
            runtime.host.executor.write(move |db| {
                db.execute("UPDATE runtime_requests SET result_json=json_object('complete',json('true')) WHERE task_id=? AND request_key='pending-check'", [&held_task])?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();

            // Exercise the same owner close route exposed by the task screen. The
            // UI first resolves the bound execution, then persists the task's
            // closed disposition. Reusing its request id must not record a second
            // owner resolution.
            let projection = service.native_tasks(&project).await.unwrap();
            let execution = projection[&tasks[0]]["executionId"].clone();
            let expected_version = projection[&tasks[0]]["version"].clone();
            let close_request = json!({
                "projectId":project,
                "executionId":execution,
                "requestId":"owner-close-as-is",
                "action":"close_task",
                "confirmed":true,
                "expectedVersion":expected_version
            });
            let first_control = runtime
                .control(project.clone(), close_request.clone())
                .await
                .unwrap();
            assert_eq!(first_control["outcome"], "recorded");
            let config = service.config.clone();
            service.stop().await.unwrap();
            drop(runtime);
            drop(service);
            let service = Service::bind_native(config, Arc::new(Peer(None)))
                .await
                .unwrap();
            service.activate(&project).await.unwrap();
            let runtime = service.state.lock().await.native[&project].clone();
            let mut jobs = Jobs::default();
            let replay = runtime
                .control(project.clone(), close_request)
                .await
                .unwrap();
            assert_eq!(replay, first_control);
            let resolved_events = runtime
                .host
                .executor
                .read(move |db| {
                    Ok(db.query_row(
                        "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='OWNER_TASK_RESOLVED'",
                        [&task],
                        |row| row.get::<_, i64>(0),
                    )?)
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(resolved_events, 1);
            call(
                &service,
                "product.task",
                &project,
                json!({"action":"close","taskId":tasks[0],"choice":"as_is","confirmed":true}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let resumed = sprint(&service, &project, &id).await;
            assert_eq!(resumed["queueState"], "running", "{resumed}");
            assert!(resumed["automaticQueue"].is_null(), "{resumed}");
            assert_eq!(resumed["pauseReason"], Value::Null);
            assert_eq!(
                call(&service, "product.board", &project, json!({})).await["tasks"][&tasks[0]]
                    ["closedAs"],
                "as_is"
            );
            assert!(!before.is_empty());

            // The next pass starts the dependent once, and another pass does not
            // repeat the close or create a second execution for it.
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let first = service.native_tasks(&project).await.unwrap();
            assert!(first[&tasks[1]]["executionId"].is_string());
            let execution = first[&tasks[1]]["executionId"].clone();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let second = service.native_tasks(&project).await.unwrap();
            assert_eq!(second[&tasks[1]]["executionId"], execution);
            assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "1");
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_rechecks_finish_preconditions_without_dispatching_a_commit() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("later", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            verified_fixture(&runtime, task, true).await;
            let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let held = sprint(&service, &project, &id).await;
            assert_eq!(held["automaticQueue"]["code"], "unknown_branch", "{held}");

            git_fixture(&folder, &["branch", "later"]);
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let next = sprint(&service, &project, &id).await;
            assert_eq!(
                next["automaticQueue"]["kind"], "owner_integration",
                "{next}"
            );
            assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
            assert!(
                service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_null()
            );
            service.stop().await.unwrap();
        }

        /// A folder without Git has nothing to commit: a verified, automatically
        /// accepted result reaches Done by itself (closed in place with its checks
        /// kept), never a "close it yourself" hold, and the queue starts the next task.
        #[tokio::test]
        async fn automatic_sprint_finishes_in_a_folder_without_git_by_itself() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            std::fs::remove_dir_all(folder.join(".git")).unwrap();
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            verified_fixture(&runtime, task, true).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let board = call(&service, "product.board", &project, json!({})).await;
            let first = &board["tasks"][&tasks[0]];
            assert_eq!(first["status"], "done", "{first}");
            assert_eq!(first["closedAs"], "as_is");
            assert_eq!(first["notVerifiedByCortex"], false);
            assert_eq!(first["acceptance"]["automatic"], true);
            let held = sprint(&service, &project, &id).await;
            assert_eq!(held["queueState"], "running", "{held}");
            assert!(held["automaticQueue"].is_null(), "{held}");
            assert!(!folder.join(".git").exists(), "no repository is created");
            // The next pass starts the dependent task: nothing waited for the owner.
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert!(
                !service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_null()
            );
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_closes_already_landed_work_without_an_extra_commit() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            std::fs::write(folder.join("work.txt"), "reviewed changes\n").unwrap();
            git_fixture(&folder, &["add", "work.txt"]);
            git_fixture(&folder, &["commit", "-q", "-m", "Already landed"]);
            std::fs::write(folder.join("unrelated.txt"), "keep staged\n").unwrap();
            git_fixture(&folder, &["add", "unrelated.txt"]);
            let head = git_fixture(&folder, &["rev-parse", "HEAD"]);
            verified_fixture(&runtime, task, true).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let board = call(&service, "product.board", &project, json!({})).await;
            let first = &board["tasks"][&tasks[0]];
            assert_eq!(first["status"], "done", "{first}");
            assert_eq!(first["integration"]["automatic"], true);
            assert_eq!(first["integration"]["committed"], false);
            assert_eq!(git_fixture(&folder, &["rev-parse", "HEAD"]), head);
            assert_eq!(
                git_fixture(&folder, &["diff", "--cached", "--name-only"]),
                "unrelated.txt"
            );
            assert_eq!(
                sprint(&service, &project, &id).await["queueState"],
                "running"
            );
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_never_commits_unattributed_changes() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            verified_fixture(&runtime, task, false).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let held = sprint(&service, &project, &id).await;
            assert_eq!(held["queueState"], "paused", "{held}");
            assert_eq!(held["automaticQueue"]["code"], "task_content_unknown");
            assert_eq!(held["automaticQueue"]["needsOwner"], true);
            assert!(
                service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_null()
            );
            assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "1");
            assert_eq!(
                std::fs::read(folder.join("work.txt")).unwrap(),
                b"reviewed changes\n"
            );
            service.stop().await.unwrap();
        }

        #[tokio::test]
        async fn automatic_sprint_does_not_repeat_a_dispatched_failed_commit() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            let hook = folder.join(".git/hooks/pre-commit");
            std::fs::write(
                &hook,
                "#!/bin/sh\nprintf 'attempt\\n' >> .git/fixture-attempts\nexit 1\n",
            )
            .unwrap();
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
            std::fs::write(folder.join("unrelated.txt"), "keep staged\n").unwrap();
            git_fixture(&folder, &["add", "unrelated.txt"]);
            verified_fixture(&runtime, task, true).await;

            for _ in 0..2 {
                service
                    .sprint_pass(&project, &runtime, &mut jobs, false)
                    .await
                    .unwrap();
                let held = sprint(&service, &project, &id).await;
                assert_eq!(
                    held["automaticQueue"]["code"], "integration_failed",
                    "{held}"
                );
                assert_eq!(held["automaticQueue"]["needsOwner"], true);
                // Even an explicit queue restart cannot dispatch the same result's
                // integration a second time behind the durable intent.
                call(
                    &service,
                    "product.sprint",
                    &project,
                    json!({"action":"patch","sprintId":id,
                    "patch":{"queueState":"running","pauseReason":null}}),
                )
                .await;
            }
            assert_eq!(
                std::fs::read_to_string(folder.join(".git/fixture-attempts")).unwrap(),
                "attempt\n"
            );
            assert_eq!(git_fixture(&folder, &["rev-list", "--count", "HEAD"]), "1");
            assert_eq!(
                git_fixture(&folder, &["diff", "--cached", "--name-only"]),
                "unrelated.txt"
            );
            assert!(
                service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_null()
            );
            service.stop().await.unwrap();
        }

        async fn allow(service: &Service, project: &str, id: &Value, hold: &Value) -> Value {
            service
                .machine(
                    serde_json::from_value(json!({"op":"product.sprint","projectId":project,
                    "params":{"action":"allow","sprintId":id,"taskId":hold["taskId"],"code":hold["code"],
                        "holdId":hold["holdId"],"ownerAction":"allow_repair"}}))
                    .unwrap(),
                )
                .await
                .unwrap()
        }

        /// H2: a busy runtime decides nothing. Without consent no owner hold is written
        /// (the agent goes first on the next pass); with consent the consent is kept.
        #[tokio::test]
        async fn a_busy_runtime_keeps_agent_first_repair_and_the_owners_consent() {
            let (_scratch, _folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            fail_run(
                &runtime,
                &task,
                "answer_not_certified:completion_review_missing",
            )
            .await;
            let fact = service.native_tasks(&project).await.unwrap()[&tasks[0]].clone();
            let running = sprint(&service, &project, &id).await;
            {
                let _busy = runtime.controls.lock().await;
                let repair = service
                    .failed_repair(&project, &runtime, &running, &tasks[0], &fact)
                    .await
                    .unwrap();
                assert!(matches!(repair, Repair::Later), "busy is not a refusal");
            }
            assert_eq!(repairs_of(&runtime, &task).await, 0);
            assert_eq!(sprint(&service, &project, &id).await, running);
            // The owner allowed a repair; the runtime is busy when the queue acts on it.
            service
                .automatic_pause(
                    &project,
                    &running,
                    automatic_hold(
                        "settlement_blocked",
                        Some(&tasks[0]),
                        "task_failed",
                        "The last run of this task ended as failed.",
                        &HoldFacts {
                            repair: true,
                            ..HoldFacts::default()
                        },
                    ),
                )
                .await
                .unwrap();
            let held = sprint(&service, &project, &id).await;
            let reply = allow(&service, &project, &id, &held["automaticQueue"]).await;
            assert_eq!(reply["ok"], true, "{reply}");
            let allowed = sprint(&service, &project, &id).await;
            assert!(allowed["automaticQueue"]["allowedAt"].is_i64(), "{allowed}");
            {
                let _busy = runtime.controls.lock().await;
                let repair = service
                    .failed_repair(&project, &runtime, &allowed, &tasks[0], &fact)
                    .await
                    .unwrap();
                assert!(matches!(repair, Repair::Later), "busy is not a refusal");
            }
            assert_eq!(
                sprint(&service, &project, &id).await,
                allowed,
                "consent kept"
            );
            assert_eq!(repairs_of(&runtime, &task).await, 0);
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let resumed = sprint(&service, &project, &id).await;
            assert_eq!(repairs_of(&runtime, &task).await, 1, "{resumed}");
            assert_eq!(resumed["queueState"], "running", "{resumed}");
            assert!(resumed["automaticQueue"].is_null(), "{resumed}");
            service.stop().await.unwrap();
        }

        /// M4: a failed run with an effect whose outcome is unknown is never offered to
        /// the agent; the runtime's refusal reaches the owner's hold.
        #[tokio::test]
        async fn a_failed_run_with_an_unknown_effect_is_never_offered_to_the_agent() {
            let (_scratch, _folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            let t = task.clone();
            runtime.host.executor.write(move |db| {
                db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,owner_epoch,issued_at)
                    VALUES ('fixture-unknown','hash',?,'fixture-worker','worker',1,'now')", [&t])?;
                db.execute("INSERT INTO runtime_requests VALUES (?,'fixture-unknown-key','fixture-unknown','project_write','digest',?)",
                    (&t, json!({"complete":false}).to_string()))?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            fail_run(
                &runtime,
                &task,
                "answer_not_certified:completion_review_missing",
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let held = sprint(&service, &project, &id).await;
            let hold = &held["automaticQueue"];
            assert_eq!(hold["code"], "task_failed", "{held}");
            // A write whose outcome is unknown can never be settled: the owner decides,
            // in plain words, with only the close the service accepts.
            assert_eq!(hold["nextActor"], "owner");
            assert_eq!(hold["ownerActions"], json!(["close_as_is"]), "{hold}");
            assert_eq!(hold["reason"], UNSETTLED_REASON);
            assert_eq!(hold["asks"], json!([UNSETTLED_CLOSE_ASK]));
            assert_eq!(
                hold["settling"]["hold"]["asks"],
                json!([UNKNOWN_EFFECT_ASK])
            );
            assert_eq!(repairs_of(&runtime, &task).await, 0);
            // A command whose process still runs, which the runtime settles by itself
            // once it ends: the runtime acts next and the owner is asked nothing.
            let t = task.clone();
            runtime.host.executor.write(move |db| {
                db.execute("DELETE FROM runtime_requests WHERE task_id=? AND request_key='fixture-unknown-key'", [&t])?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            let child = live_command(&runtime, &task, "fixture-unknown-key").await;
            for _ in 0..2 {
                service
                    .sprint_pass(&project, &runtime, &mut jobs, false)
                    .await
                    .unwrap();
                let waiting = sprint(&service, &project, &id).await["automaticQueue"].clone();
                assert_eq!(waiting["nextActor"], "runtime", "{waiting}");
                assert_eq!(waiting["reason"], SETTLING_REASON);
                assert_eq!(waiting["ownerActions"], json!([]));
                assert_eq!(waiting["settling"]["since"], hold["settling"]["since"]);
            }
            // Once it settles the queue derives the failed run again: the agent repairs it.
            drop(child);
            let t = task.clone();
            runtime.host.executor.write(move |db| {
                db.execute("UPDATE runtime_requests SET result_json=? WHERE task_id=? AND request_key='fixture-unknown-key'",
                    (json!({"complete":false,"settled":true,"failure":"command_outcome_unknown"}).to_string(), &t))?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            for _ in 0..2 {
                service
                    .sprint_pass(&project, &runtime, &mut jobs, false)
                    .await
                    .unwrap();
            }
            let resumed = sprint(&service, &project, &id).await;
            assert!(resumed["automaticQueue"]["settling"].is_null(), "{resumed}");
            assert_eq!(repairs_of(&runtime, &task).await, 1, "{resumed}");
            service.stop().await.unwrap();
        }

        /// The owner's incident: a stale automatic acceptance whose task has an unknown
        /// effect. While the runtime can settle it nothing is offered; once it cannot,
        /// Close as is is offered and accepted (preflight and close), recording the
        /// effect as unknown, never as done.
        #[tokio::test]
        async fn an_unknown_effect_hold_offers_only_a_close_the_service_accepts() {
            let (_scratch, _folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            let t = task.clone();
            runtime
                .host
                .executor
                .write(move |db| {
                    db.execute(
                        "UPDATE runtime_tasks SET state='COMPLETED',blocker=NULL WHERE task_id=?",
                        [&t],
                    )?;
                    Ok(())
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
            let child = live_command(&runtime, &task, "pending-check").await;
            for _ in 0..2 {
                service
                    .sprint_pass(&project, &runtime, &mut jobs, false)
                    .await
                    .unwrap();
            }
            let waiting = sprint(&service, &project, &id).await["automaticQueue"].clone();
            assert_eq!(waiting["code"], "automatic_acceptance_blocked", "{waiting}");
            assert_eq!(waiting["nextActor"], "runtime", "{waiting}");
            assert_eq!(waiting["ownerActions"], json!([]));
            let close = |preflight: bool| {
                json!({"op":"product.task","projectId":project,"params":{"action":"close","choice":"as_is",
                    "confirmed":true,"taskId":tasks[0],"preflight":preflight}})
            };
            // Closing while the runtime can still settle the command is refused, by the
            // preflight as by the close.
            for preflight in [true, false] {
                let refused = service
                    .machine(serde_json::from_value(close(preflight)).unwrap())
                    .await
                    .unwrap();
                assert_eq!(
                    refused["code"], "native_outside_outcome_unknown",
                    "{refused}"
                );
            }
            // An effect the runtime can never confirm goes to the owner.
            let t = task.clone();
            runtime.host.executor.write(move |db| {
                db.execute("UPDATE runtime_requests SET operation='project_edit' WHERE task_id=? AND request_key='pending-check'", [&t])?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let held = sprint(&service, &project, &id).await["automaticQueue"].clone();
            assert_eq!(held["nextActor"], "owner", "{held}");
            assert_eq!(held["ownerActions"], json!(["close_as_is"]), "{held}");
            assert_eq!(held["reason"], UNSETTLED_REASON);
            assert_eq!(held["settling"]["since"], waiting["settling"]["since"]);
            let refused = allow(&service, &project, &id, &held).await;
            assert_eq!(refused["code"], "automatic_hold_changed", "{refused}");
            let checked = service
                .machine(serde_json::from_value(close(true)).unwrap())
                .await
                .unwrap();
            assert_eq!(checked["result"]["preflight"], true, "{checked}");
            let closed = service
                .machine(serde_json::from_value(close(false)).unwrap())
                .await
                .unwrap();
            assert_eq!(closed["ok"], true, "{closed}");
            let t = task.clone();
            let (settled_by, acknowledged): (String, i64) = runtime.host.executor.read(move |db| {
                Ok((db.query_row("SELECT json_extract(result_json,'$.settled_by') FROM runtime_requests WHERE task_id=? AND request_key='pending-check'", [&t], |r| r.get(0))?,
                    db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='OWNER_ACKNOWLEDGED_UNKNOWN_EFFECTS'", [&t], |r| r.get(0))?))
            }).unwrap().await.unwrap().unwrap();
            assert_eq!(
                (settled_by.as_str(), acknowledged),
                ("owner_close_as_is", 1)
            );
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let resumed = sprint(&service, &project, &id).await;
            assert!(resumed["automaticQueue"].is_null(), "{resumed}");
            service.stop().await.unwrap();
        }

        /// M1: Hybrid keeps a refused start for the owner's Try again instead of
        /// clearing it and retrying the start on every pass.
        #[tokio::test]
        async fn hybrid_keeps_a_refused_start_until_the_owner_tries_again() {
            let (_scratch, _folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"patch","sprintId":id,"patch":{"runMode":"hybrid"}}),
            )
            .await;
            let expected = sprint(&service, &project, &id).await;
            service
                .automatic_pause(
                    &project,
                    &expected,
                    automatic_hold(
                        "start_refused",
                        Some(&tasks[0]),
                        "maintenance_busy",
                        &start_reason("maintenance_busy"),
                        &HoldFacts::default(),
                    ),
                )
                .await
                .unwrap();
            let held = sprint(&service, &project, &id).await;
            assert_eq!(
                held["automaticQueue"]["ownerActions"],
                json!(["retry", "close_as_is"])
            );
            let mut jobs = Jobs::default();
            for _ in 0..2 {
                service
                    .sprint_pass(&project, &runtime, &mut jobs, false)
                    .await
                    .unwrap();
                assert_eq!(sprint(&service, &project, &id).await, held);
            }
            assert!(
                service.native_tasks(&project).await.unwrap()[&tasks[0]]["executionId"].is_null()
            );
            // Try again is Continue queue: the queue starts the task.
            call(
                &service,
                "product.sprint",
                &project,
                json!({"action":"patch","sprintId":id,"patch":{"queueState":"running","pauseReason":null}}),
            )
            .await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert!(
                service.native_tasks(&project).await.unwrap()[&tasks[0]]["executionId"].is_string()
            );
            service.stop().await.unwrap();
        }

        /// L5: the repaired result's acceptance is cleared before the runtime reopens
        /// it; a repair that does not start leaves the record exactly as it was.
        #[tokio::test]
        async fn a_repair_that_does_not_start_leaves_the_acceptance_as_it_was() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let draft = &tasks[0];
            let task = settled_peer(&runtime, draft).await;
            verified_fixture(&runtime, task.clone(), true).await;
            let accepted = call(
                &service,
                "product.task",
                &project,
                json!({"action":"auto_ready","taskId":draft}),
            )
            .await;
            assert_eq!(accepted["accepted"], true, "{accepted}");
            std::fs::write(folder.join("work.txt"), "changed since acceptance\n").unwrap();
            let expected = sprint(&service, &project, &id).await;
            let hold = automatic_hold(
                "settlement_blocked",
                Some(draft),
                "stale_result",
                "The workspace changed.",
                &HoldFacts::default(),
            );
            service
                .automatic_pause(&project, &expected, hold)
                .await
                .unwrap();
            let held = sprint(&service, &project, &id).await;
            let before =
                call(&service, "product.board", &project, json!({})).await["tasks"][draft].clone();
            let execution =
                service.native_tasks(&project).await.unwrap()[draft]["executionId"].clone();
            {
                let _busy = runtime.controls.lock().await;
                let repair = service
                    .automatic_repair(
                        &project,
                        &runtime,
                        &held,
                        draft,
                        &execution,
                        None,
                        "stale_result",
                        "The workspace changed.",
                        true,
                    )
                    .await
                    .unwrap();
                assert!(matches!(repair, Repair::Later));
            }
            let after =
                call(&service, "product.board", &project, json!({})).await["tasks"][draft].clone();
            assert_eq!(after["acceptance"], before["acceptance"]);
            assert_eq!(after["status"], before["status"]);
            assert_eq!(sprint(&service, &project, &id).await, held);
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let board = call(&service, "product.board", &project, json!({})).await;
            assert_eq!(repairs_of(&runtime, &task).await, 1);
            assert!(board["tasks"][draft]["acceptance"].is_null());
            service.stop().await.unwrap();
        }

        /// L3: a failed commit says nothing about a Commit retry no hold offers; the
        /// owner's consent sends it back to the agent for a fresh result.
        #[tokio::test]
        async fn a_failed_commit_offers_the_agent_repair_it_describes() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            let hook = folder.join(".git/hooks/pre-commit");
            std::fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
            std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o700)).unwrap();
            verified_fixture(&runtime, task.clone(), true).await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let held = sprint(&service, &project, &id).await;
            let hold = &held["automaticQueue"];
            assert_eq!(hold["code"], "integration_failed", "{held}");
            assert_eq!(hold["reason"], INTEGRATION_FAILED);
            assert_eq!(held["pauseReason"], INTEGRATION_FAILED);
            assert_eq!(
                hold["ownerActions"],
                json!(["allow_repair", "close_as_is"]),
                "{hold}"
            );
            std::fs::remove_file(&hook).unwrap();
            let reply = allow(&service, &project, &id, hold).await;
            assert_eq!(reply["ok"], true, "{reply}");
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let resumed = sprint(&service, &project, &id).await;
            assert_eq!(repairs_of(&runtime, &task).await, 1, "{resumed}");
            assert_eq!(resumed["queueState"], "running", "{resumed}");
            service.stop().await.unwrap();
        }

        /// L2/L7: every owner hold the scheduler can persist answers Allow only for
        /// itself, Try again releases it, and every runtime-actor hold is rechecked
        /// (released here, where nothing blocks) by the next pass.
        #[tokio::test]
        async fn every_hold_action_reaches_its_handler_and_every_runtime_hold_is_rechecked() {
            use crate::native_driver::sprint_tests::automatic_repair_tests::{
                HOLDS, OWNER_ACTIONS,
            };
            let (_scratch, _folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            // How the conversation card sends each action (see the card in the UI):
            // product.sprint allow, a sprint patch, a task control, the accept or close
            // flows, or a link to the place the owner acts.
            let routes = [
                ("allow_repair", "product.sprint allow"),
                ("retry", "product.sprint patch queueState running"),
                ("allow_retries", "task continue"),
                ("resume_task", "task continue"),
                ("confirm_and_continue", "task retry"),
                ("allow_finish", "accept with finish consent"),
                ("accept_result", "accept"),
                ("close_as_is", "product.task close"),
                ("confirm_integration", "task changes"),
                ("resolve_conflict", "task conflict"),
                ("change_run_mode", "sprint settings"),
            ];
            for action in OWNER_ACTIONS {
                assert!(
                    routes.iter().any(|(a, _)| a == action),
                    "{action} has no route"
                );
            }
            let facts = [
                HoldFacts::default(),
                HoldFacts {
                    repair: true,
                    ..HoldFacts::default()
                },
            ];
            for &(kind, code) in HOLDS {
                for task in [Some(tasks[0].as_str()), None] {
                    for facts in &facts {
                        let hold = automatic_hold(kind, task, code, "A plain reason.", facts);
                        let expected = sprint(&service, &project, &id).await;
                        service
                            .automatic_pause(&project, &expected, hold.clone())
                            .await
                            .unwrap();
                        let saved = sprint(&service, &project, &id).await["automaticQueue"].clone();
                        assert!(saved["holdId"].is_string(), "{saved}");
                        if hold["nextActor"] == BY_OWNER {
                            let offers = hold["ownerActions"]
                                .as_array()
                                .unwrap()
                                .iter()
                                .any(|a| a == "allow_repair");
                            // An older card's decision never applies to this hold.
                            let mut stale = saved.clone();
                            stale["holdId"] = json!("h_older");
                            let refused = allow(&service, &project, &id, &stale).await;
                            assert_eq!(refused["code"], "automatic_hold_changed", "{saved}");
                            let reply = allow(&service, &project, &id, &saved).await;
                            assert_eq!(reply["ok"] == true, offers, "{saved}: {reply}");
                            let after = sprint(&service, &project, &id).await;
                            assert_eq!(after["automaticQueue"]["allowedAt"].is_i64(), offers);
                            // Try again releases the hold through the sprint patch.
                            call(
                                &service,
                                "product.sprint",
                                &project,
                                json!({"action":"patch","sprintId":id,
                                "patch":{"queueState":"running","pauseReason":null}}),
                            )
                            .await;
                            assert!(
                                sprint(&service, &project, &id).await["automaticQueue"].is_null()
                            );
                        } else if task.is_some() || kind != "continuation" {
                            // A continuation hold always names its task.
                            service
                                .sprint_pass(&project, &runtime, &mut jobs, false)
                                .await
                                .unwrap();
                            let after = sprint(&service, &project, &id).await;
                            assert!(
                                after["automaticQueue"].is_null(),
                                "{kind}/{code} is never rechecked: {after}"
                            );
                            assert_eq!(after["queueState"], "running");
                        } else {
                            call(
                                &service,
                                "product.sprint",
                                &project,
                                json!({"action":"patch","sprintId":id,
                                "patch":{"queueState":"running","pauseReason":null}}),
                            )
                            .await;
                        }
                    }
                }
            }
            // No hold the scheduler persists names the agent: while an agent repair runs
            // the queue runs and no hold is saved (see `BY_AGENT`).
            assert!(
                service.native_tasks(&project).await.unwrap()[&tasks[0]]["executionId"].is_null()
            );
            service.stop().await.unwrap();

            // With an unknown effect present, every hold of a started task is decided
            // again: a command the runtime can settle is the runtime's (asks nothing),
            // any other effect is the owner's, and each action offered is accepted by its
            // handler. Allow a repair is never offered; Close as is passes its handler's
            // own preflight, including the settled-execution guard of an automatic close.
            let (_scratch, _folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            // A command whose process still runs (the runtime settles it once it ends),
            // then a write the runtime can never confirm.
            let child = live_command(&runtime, &task, "invariant-unknown").await;
            for settles in [true, false] {
                if !settles {
                    let t = task.clone();
                    runtime.host.executor.write(move |db| {
                        db.execute("UPDATE runtime_requests SET operation='project_edit',result_json='{\"complete\":false}'
                            WHERE task_id=? AND request_key='invariant-unknown'", [&t])?;
                        Ok(())
                    }).unwrap().await.unwrap().unwrap();
                }
                for &(kind, code) in HOLDS {
                    for facts in &facts {
                        let hold =
                            automatic_hold(kind, Some(&tasks[0]), code, "A plain reason.", facts);
                        let expected = sprint(&service, &project, &id).await;
                        service
                            .automatic_pause(&project, &expected, hold.clone())
                            .await
                            .unwrap();
                        let saved = sprint(&service, &project, &id).await["automaticQueue"].clone();
                        let actions: Vec<&str> = saved["ownerActions"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|a| a.as_str().unwrap())
                            .collect();
                        if kind == "start_refused" {
                            assert_eq!(saved["ownerActions"], hold["ownerActions"], "{saved}");
                        } else if settles {
                            assert_eq!(saved["nextActor"], BY_RUNTIME, "{kind}/{code}: {saved}");
                            assert_eq!(saved["needsOwner"], false);
                            assert_eq!(saved["reason"], SETTLING_REASON);
                            assert!(actions.is_empty() && saved["asks"] == json!([]), "{saved}");
                            // Rechecked on every pass, and kept while it stays unknown.
                            service
                                .sprint_pass(&project, &runtime, &mut jobs, false)
                                .await
                                .unwrap();
                            let kept =
                                sprint(&service, &project, &id).await["automaticQueue"].clone();
                            assert_eq!(kept["nextActor"], BY_RUNTIME, "{kind}/{code}: {kept}");
                            assert_eq!(kept["settling"]["hold"]["code"], code);
                        } else if saved["settling"].is_null() {
                            // No-stop policy: the task's continuation already settled the
                            // write for re-verification by its coordinator, so the hold is
                            // decided as if every effect were known.
                            let t = task.clone();
                            let settled_by: String = runtime.host.executor.read(move |db| {
                                Ok(db.query_row("SELECT coalesce(json_extract(result_json,'$.settled_by'),'') FROM runtime_requests
                                    WHERE task_id=? AND request_key='invariant-unknown'", [&t], |r| r.get(0))?)
                            }).unwrap().await.unwrap().unwrap();
                            assert_eq!(settled_by, "runtime_reverify", "{kind}/{code}: {saved}");
                            assert_eq!(
                                saved["reason"], "A plain reason.",
                                "{kind}/{code}: {saved}"
                            );
                        } else {
                            assert_eq!(saved["nextActor"], BY_OWNER, "{kind}/{code}: {saved}");
                            assert_eq!(saved["reason"], UNSETTLED_REASON, "{kind}/{code}: {saved}");
                            assert_eq!(actions.last(), Some(&"close_as_is"), "{saved}");
                            for action in &actions {
                                assert!(ACCEPTED_UNSETTLED.contains(action), "{action}: {saved}");
                                assert!(routes.iter().any(|(a, _)| a == action), "{action}");
                            }
                            let refused = allow(&service, &project, &id, &saved).await;
                            assert_eq!(refused["code"], "automatic_hold_changed", "{saved}");
                            let checked = service
                                .machine(
                                    serde_json::from_value(json!({"op":"product.task","projectId":project,
                                        "params":{"action":"close","choice":"as_is","confirmed":true,
                                            "taskId":tasks[0],"preflight":true}}))
                                    .unwrap(),
                                )
                                .await
                                .unwrap();
                            assert_eq!(checked["ok"], true, "{kind}/{code}: {checked}");
                        }
                        call(
                            &service,
                            "product.sprint",
                            &project,
                            json!({"action":"patch","sprintId":id,
                            "patch":{"queueState":"running","pauseReason":null}}),
                        )
                        .await;
                    }
                }
            }
            drop(child);
            service.stop().await.unwrap();
        }

        /// A command receipt the runtime cannot settle yet: the process group it
        /// recorded still runs. Dropping the returned child ends it.
        async fn live_command(runtime: &Runtime, task: &str, key: &str) -> tokio::process::Child {
            let child = tokio::process::Command::new("/bin/sleep")
                .arg("120")
                .process_group(0)
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let pid = child.id().unwrap();
            let (t, k) = (task.to_owned(), key.to_owned());
            runtime.host.executor.write(move |db| {
                let reference = format!("native-command:{k}");
                db.execute("INSERT INTO runtime_requests VALUES (?,?,'fixture-live','command_run','digest',?)",
                    (&t, &k, json!({"complete":false,"tool_call_ref":reference}).to_string()))?;
                cortex_core::lifecycle::event_tx(db, &t, "NATIVE_COMMAND_PROCESS",
                    json!({"ref":reference,"process_identity":{"pid":pid,"start_ticks":1}}))?;
                Ok(())
            }).unwrap().await.unwrap().unwrap();
            child
        }

        /// The settling decision itself: the runtime waits a bounded time for commands,
        /// any other unknown effect is the owner's at once, and a known one changes nothing.
        #[test]
        fn a_settling_hold_waits_a_bounded_time_and_keeps_the_replaced_decision() {
            let facts = HoldFacts {
                repair: true,
                ..HoldFacts::default()
            };
            let hold = automatic_hold(
                "settlement_blocked",
                Some("t"),
                "stale_result",
                "Changed.",
                &facts,
            );
            assert_eq!(hold["ownerActions"], json!(["allow_repair", "close_as_is"]));
            assert_eq!(settling_hold(hold.clone(), &Value::Null, None, 5), hold);
            let waiting = settling_hold(hold.clone(), &Value::Null, Some(Unsettled::Commands), 5);
            assert_eq!(waiting["nextActor"], BY_RUNTIME);
            assert_eq!(waiting["settling"]["since"], 5);
            assert_eq!(waiting["settling"]["hold"], hold);
            // The same hold decided again keeps its first wait and its replaced decision.
            let again = settling_hold(waiting.clone(), &waiting, Some(Unsettled::Commands), 6);
            assert_eq!(again, waiting);
            // No time cap: commands stay the runtime's to settle, however long it takes.
            let late = settling_hold(
                hold.clone(),
                &waiting,
                Some(Unsettled::Commands),
                5 + 24 * 60 * 60 * 1000,
            );
            assert_eq!(late["nextActor"], BY_RUNTIME);
            assert_eq!(late["ownerActions"], json!([]));
            assert_eq!(late["settling"]["hold"], hold);
            let never = settling_hold(hold.clone(), &Value::Null, Some(Unsettled::Effects), 5);
            assert_eq!(never["ownerActions"], json!(["close_as_is"]));
            assert_eq!(never["asks"], json!([UNSETTLED_CLOSE_ASK]));
            // Settled again: the replaced decision comes back unchanged.
            assert_eq!(settling_hold(never, &late, None, 9), hold);
            // An action the service accepts with an unknown effect stays offered first.
            let recovery = automatic_hold(
                "settlement_blocked",
                Some("t"),
                "task_recovery",
                "Stopped.",
                &HoldFacts::default(),
            );
            let owner = settling_hold(recovery, &Value::Null, Some(Unsettled::Effects), 5);
            assert_eq!(
                owner["ownerActions"],
                json!(["confirm_and_continue", "close_as_is"])
            );
            assert_eq!(owner["asks"].as_array().unwrap().len(), 2);
            // A refused start and a hold without a task are never changed.
            let start = automatic_hold(
                "start_refused",
                Some("t"),
                "refused",
                "No.",
                &HoldFacts::default(),
            );
            assert_eq!(
                settling_hold(start.clone(), &Value::Null, Some(Unsettled::Effects), 5),
                start
            );
            let none = automatic_hold(
                "settlement_blocked",
                None,
                "git_required",
                "No.",
                &HoldFacts::default(),
            );
            assert_eq!(
                settling_hold(none.clone(), &Value::Null, Some(Unsettled::Effects), 5),
                none
            );
        }
    }

    /// Legacy manual-queue pause wording, retained for saved holds.
    #[cfg(test)]
    pub(super) fn with_settle_failure<'a>(
        pause: Option<&'a str>,
        failed: Option<&'a str>,
    ) -> Option<&'a str> {
        match (pause, failed) {
            (Some(UNSETTLED), Some(why)) => Some(why),
            (pause, _) => pause,
        }
    }
    const UNSETTLED: &str = "A result has missing or unsuccessful checks.";
    /// The disposition a sprint member is settled by: its saved one, or, for a task the
    /// owner finished through the runtime alone (Finish on its stop question) with none
    /// saved, the owner's close as is, read from the same projection rule the board uses.
    pub(crate) fn member_disposition(
        record: &Value,
        task: Option<&Value>,
    ) -> (String, Option<String>) {
        if task.is_some_and(|t| warned(t, "owner_closed_unconfirmed")) {
            return ("done".into(), Some("unconfirmed".into()));
        }
        let status = record["status"].as_str().unwrap_or("").to_owned();
        let closed = record["closedAs"].as_str().map(str::to_owned);
        let resolved = task.is_some_and(|t| {
            t["phase"] == "stopped"
                && t["runtimeState"] == "CANCELLED"
                && t["warnings"].as_array().is_some_and(|w| {
                    w.iter()
                        .any(|w| w == "owner_resolved_without_certification")
                        && !w.iter().any(|w| w == "contradictory_evidence")
                })
        });
        if status.is_empty() && record["archive"].is_null() && resolved {
            return (
                "done".into(),
                Some(closed.unwrap_or_else(|| "as_is".into())),
            );
        }
        (status, closed)
    }
    #[cfg(test)]
    mod settle_tests {
        use super::*;

        #[test]
        fn a_closed_unconfirmed_member_is_skipped_but_does_not_finish_its_dependency() {
            let records = json!({"first":{"title":"First"},"second":{"dependencies":["first"]}});
            let tasks = HashMap::from([
                (
                    "first".into(),
                    json!({"phase":"unknown","warnings":["contradictory_evidence","owner_closed_unconfirmed"]}),
                ),
                ("second".into(), json!({"phase":"not_started"})),
            ]);
            let members = vec!["first".into(), "second".into()];
            let disposition = members
                .iter()
                .map(|id: &String| (id.clone(), member_disposition(&records[id], tasks.get(id))))
                .collect();
            for hybrid in [false, true] {
                assert!(matches!(
                    automatic_queue_step(&members[..1], &tasks, &records, &disposition, hybrid),
                    QueueStep::Complete
                ));
                assert!(matches!(
                    automatic_queue_step(&members, &tasks, &records, &disposition, hybrid),
                    QueueStep::Hold(_)
                ));
            }
        }
        #[test]
        fn a_failed_settle_names_itself_and_other_pauses_keep_their_reason() {
            let why = "Build could not be accepted automatically: The result could not be read. Open the task to accept it yourself.";
            assert_eq!(with_settle_failure(Some(UNSETTLED), Some(why)), Some(why));
            assert_eq!(with_settle_failure(Some(UNSETTLED), None), Some(UNSETTLED));
            assert_eq!(
                with_settle_failure(Some("This sprint is in manual mode."), Some(why)),
                Some("This sprint is in manual mode.")
            );
            assert_eq!(with_settle_failure(None, Some(why)), None);
        }
        #[test]
        fn a_member_finished_through_the_runtime_settles_as_closed_as_is() {
            let resolved = json!({"phase":"stopped","runtimeState":"CANCELLED","warnings":["owner_resolved_without_certification"]});
            let none = (String::new(), None);
            let as_is = ("done".to_owned(), Some("as_is".to_owned()));
            assert_eq!(member_disposition(&json!({}), Some(&resolved)), as_is);
            // A run the runtime cancelled by itself, or contradictory evidence, settles nothing.
            let plain = json!({"phase":"stopped","runtimeState":"CANCELLED","warnings":[]});
            assert_eq!(member_disposition(&json!({}), Some(&plain)), none);
            let mut odd = resolved.clone();
            odd["warnings"] = json!([
                "owner_resolved_without_certification",
                "contradictory_evidence"
            ]);
            assert_eq!(member_disposition(&json!({}), Some(&odd)), none);
            assert_eq!(member_disposition(&json!({}), None), none);
            // A saved disposition always wins.
            let saved = json!({"status":"done","closedAs":"cancel"});
            assert_eq!(
                member_disposition(&saved, Some(&resolved)),
                ("done".to_owned(), Some("cancel".to_owned()))
            );
        }
    }
    #[cfg(test)]
    mod hold_tests {
        use super::*;
        /// H1: only a recorded hold (or an exact legacy text) is a system hold. A failed
        /// settle text proves nothing: earlier versions wrote it over the owner's pause.
        #[test]
        fn an_owner_pause_is_never_a_system_hold_whatever_its_text() {
            let paused = |reason: &str| json!({"runMode":"hybrid","queueState":"paused","pauseReason":reason});
            for reason in [
                "Paused by you",
                "Build could not be accepted automatically: the check stopped before it finished.",
                "This sprint is in manual mode.",
            ] {
                assert!(!system_hold(&paused(reason)), "{reason}");
                assert!(!resumable_automatic_queue(&paused(reason)), "{reason}");
            }
            assert!(system_hold(&paused(
                "No task can run; review dependencies."
            )));
            let mut held = paused("Paused by you");
            held["automaticQueue"] = json!({"kind":"settlement_blocked"});
            assert!(system_hold(&held));
        }
        /// L1: an acceptance the finish cannot use never "continues by itself".
        #[test]
        fn an_unusable_acceptance_is_the_owners_decision_with_an_action() {
            let code = "automatic_acceptance_required";
            assert_eq!(first_actor("settlement_blocked", code), BY_OWNER);
            let plain = automatic_hold(
                "settlement_blocked",
                Some("t"),
                code,
                "r",
                &HoldFacts::default(),
            );
            assert_eq!(plain["nextActor"], BY_OWNER);
            assert_eq!(plain["ownerActions"], json!(["close_as_is"]));
            let repair = HoldFacts {
                repair: true,
                ..HoldFacts::default()
            };
            let offered = automatic_hold("settlement_blocked", Some("t"), code, "r", &repair);
            assert_eq!(
                offered["ownerActions"],
                json!(["allow_repair", "close_as_is"])
            );
        }
        /// M5: Hybrid offers its acceptance for a worktree result; Automatic's own
        /// finish still needs the project folder.
        #[test]
        fn hybrid_offers_acceptance_in_a_worktree_and_automatic_finish_needs_the_folder() {
            let record = |checkout: &str| {
                json!({"checkout":{"kind":checkout},"conflict":"none",
                    "reviewActions":{"acknowledgeOpenChecks":true}})
            };
            let hybrid = json!({"runMode":"hybrid"});
            let automatic = json!({"runMode":"automatic"});
            let code = "automatic_acceptance_blocked";
            assert!(finish_offered(&hybrid, &record("worktree"), code, false));
            assert!(finish_offered(&hybrid, &record("main"), code, false));
            assert!(!finish_offered(
                &automatic,
                &record("worktree"),
                code,
                false
            ));
            assert!(finish_offered(&automatic, &record("main"), code, false));
            assert!(!finish_offered(&hybrid, &record("worktree"), code, true));
            assert!(!finish_offered(
                &hybrid,
                &record("worktree"),
                "stale_result",
                false
            ));
            let mut facts = HoldFacts {
                finish: true,
                accept_only: true,
                ..HoldFacts::default()
            };
            facts.asks = vec!["Accept it.".into()];
            let hold = automatic_hold("settlement_blocked", Some("t"), code, "r", &facts);
            assert_eq!(
                hold["ownerActions"],
                json!(["accept_result", "close_as_is"])
            );
        }
        /// L6: the hold names the dependent the cancelled prerequisite actually blocks,
        /// not the first task waiting behind it.
        #[test]
        fn a_cancelled_prerequisite_holds_the_task_that_depends_on_it() {
            let members = ["schema", "api", "ui"].map(str::to_owned);
            // "ui" waits for "api"; "api" waits for the cancelled "schema".
            let records = json!({
                "schema":{"title":"Schema","closedAs":"cancel"},
                "api":{"title":"API","dependencies":["schema"]},
                "ui":{"title":"UI","dependencies":["api"]}
            });
            let tasks = ["api", "ui"]
                .into_iter()
                .map(|id| (id.to_owned(), json!({"phase":"not_started"})))
                .collect();
            let disposition: HashMap<String, (String, Option<String>)> = [(
                "schema".to_owned(),
                ("done".to_owned(), Some("cancel".to_owned())),
            )]
            .into_iter()
            .collect();
            // The waiting task is listed first: it must not be the one named.
            let order = ["ui", "api", "schema"].map(str::to_owned);
            for members in [&members[..], &order[..]] {
                let QueueStep::Hold(hold) =
                    automatic_queue_step(members, &tasks, &records, &disposition, false)
                else {
                    panic!("a cancelled prerequisite must hold the queue");
                };
                assert_eq!(hold["code"], "dependencies_blocked");
                assert_eq!(hold["taskId"], "api", "{hold}");
                let reason = hold["reason"].as_str().unwrap();
                assert!(
                    reason.contains("\u{201c}Schema\u{201d}") && reason.contains("cancelled"),
                    "{reason}"
                );
            }
        }
    }
    /// The outcome of one attempt to send a task back to its agent.
    enum Repair {
        /// The agent is working on it; no hold is persisted.
        Started,
        /// Not decidable in this pass (busy, unreadable, review still running).
        Later,
        /// No agent repair now; the facts for the owner's decision.
        Declined(HoldFacts),
    }
    /// The outcome of one automatic settle for one task.
    enum Settle {
        Accepted,
        Waiting(String),
        Deferred,
        Failed(String),
    }
    const WORKTREE_UNAVAILABLE: &str =
        "Automatic cannot start in a separate worktree. It runs only in the project folder.";
    const NOT_SENT_BACK_ASK: &str = "The agent could not be sent back to this task. Close it as is, or check its work details first.";
    const UNKNOWN_EFFECT_ASK: &str = "An action of this task may or may not have finished, so no agent is sent back to repeat it. Check the project folder, then close the task as is.";
    /// A failed commit, in words that match the actions the owner is offered.
    const INTEGRATION_FAILED: &str = "Git could not commit this task's changes. Nothing was committed and the files are unchanged. Check the Git identity and commit hooks of the project folder. A failed commit is never repeated as it is.";

    /// Why Automatic could not start a task, in plain words (never the refusal code).
    fn start_reason(code: &str) -> String {
        match code {
            "native_worktree_unavailable" => WORKTREE_UNAVAILABLE,
            "stale_version" | "stale_result" => "The task changed while the queue was starting it.",
            "maintenance_busy" => {
                "Maintenance was running when the queue tried to start this task."
            }
            "service_stopping" | "native_host_closed" => {
                "The service was stopping when the queue tried to start this task."
            }
            _ => "The queue could not start this task.",
        }
        .to_owned()
    }

    fn resumable_automatic_queue(sprint: &Value) -> bool {
        matches!(sprint["runMode"].as_str(), Some("automatic" | "hybrid"))
            && sprint["queueState"] == "paused"
            && system_hold(sprint)
    }

    /// A pause the scheduler made, rechecked on every pass. The owner's own pause
    /// ("Paused by you") and a mode switch are never lifted by the scheduler.
    fn system_hold(sprint: &Value) -> bool {
        if matches!(
            sprint["automaticQueue"]["kind"].as_str(),
            Some("continuation" | "owner_integration" | "settlement_blocked" | "start_refused")
        ) {
            return true;
        }
        // Hybrid pauses saved as free text by earlier versions, matched exactly. Nothing
        // writes these any more. A failed-settle text is not one of them: earlier
        // versions also wrote it over the owner's own pause, so it proves nothing.
        sprint["runMode"] == "hybrid"
            && sprint["automaticQueue"].is_null()
            && sprint["pauseReason"].as_str().is_some_and(|reason| {
                matches!(
                    reason,
                    "A sprint task is missing; review the queue."
                        | "A task was cancelled. Review the task before continuing the queue."
                        | "A task needs attention or has an unknown outcome."
                        | "A result has missing or unsuccessful checks."
                        | "Resolve the task conflict first."
                        | "No task can run; review dependencies."
                ) || reason.starts_with("Start was refused: ")
            })
    }

    /// The runtime itself rechecks the condition on every pass and continues on its own.
    pub(super) const BY_RUNTIME: &str = "runtime";
    /// The assigned agents fix it within their granted permissions; the runtime sends
    /// the work back by itself. No hold is persisted while an agent repair runs.
    pub(super) const BY_AGENT: &str = "agent";
    /// Only the owner can decide. The hold names what is asked and the actions.
    pub(super) const BY_OWNER: &str = "owner";

    /// What a hold's decision depends on beyond its kind and code.
    #[derive(Default, Clone, Debug)]
    pub(super) struct HoldFacts {
        /// A fresh agent repair is possible if the owner allows it.
        pub repair: bool,
        /// The owner can allow Automatic to finish the recorded result as it is.
        pub finish: bool,
        /// Hybrid: the owner's acceptance is all that is asked; merging stays theirs.
        pub accept_only: bool,
        /// Concrete items the owner is asked to allow, in plain language.
        pub asks: Vec<String>,
    }

    /// Who acts next on an Automatic queue hold, and which owner actions exist.
    /// This is the one place the decision is made: every hold the scheduler
    /// persists is built by `automatic_hold`, which calls it. An owner hold always
    /// has at least one action; the last one is always the separate Close as is.
    pub(super) fn hold_decision(
        kind: &str,
        code: &str,
        task: Option<&str>,
        facts: &HoldFacts,
    ) -> (&'static str, Vec<&'static str>) {
        let close = |mut actions: Vec<&'static str>| {
            if task.is_some() {
                actions.push("close_as_is");
            } else if actions.is_empty() {
                // A hold that names no task can still be checked again on request.
                actions.push("retry");
            }
            actions
        };
        let repair = || {
            if facts.repair {
                vec!["allow_repair"]
            } else {
                vec![]
            }
        };
        match (kind, code) {
            // No-stop policy: every continuation hold is the runtime's own. A route,
            // login or quota blocker resumes once the route qualifies (or another one
            // does); a pause an earlier version recorded for the owner (an attempt cap,
            // an owner blocker, an unknown outcome) is resumed by the runtime with a
            // changed strategy and the uncertainty recorded, never replayed blindly.
            ("continuation", _) => (BY_RUNTIME, vec![]),
            ("owner_integration", "owner_accepted") => (BY_OWNER, close(vec!["allow_finish"])),
            ("owner_integration", _) => (BY_OWNER, close(vec!["confirm_integration"])),
            ("start_refused", "native_worktree_unavailable") => {
                (BY_OWNER, close(vec!["change_run_mode"]))
            }
            ("start_refused", _) => (BY_OWNER, close(vec!["retry"])),
            // A missing record or a conflict being resolved: rechecked on every pass.
            ("settlement_blocked", "task_missing" | "conflict_resolving" | "queue_blocked")
                if code != "queue_blocked" || task.is_none() =>
            {
                (BY_RUNTIME, vec![])
            }
            // An acceptance the finish cannot use (saved without its accepted revision,
            // or made by hand) is never re-accepted by the runtime: the owner decides.
            ("settlement_blocked", "automatic_acceptance_required") => (BY_OWNER, close(repair())),
            ("settlement_blocked", "task_conflict") => (BY_OWNER, close(vec!["resolve_conflict"])),
            ("settlement_blocked", "task_paused") => (BY_OWNER, close(vec!["resume_task"])),
            ("settlement_blocked", "task_blocked" | "task_recovery") => {
                (BY_OWNER, close(vec!["confirm_and_continue"]))
            }
            // Agent work first; the owner is asked only after the bounded repairs.
            ("settlement_blocked", "task_failed") => (BY_OWNER, close(repair())),
            ("settlement_blocked", "automatic_acceptance_blocked") => {
                let mut actions = match (facts.finish, facts.accept_only) {
                    (true, true) => vec!["accept_result"],
                    (true, false) => vec!["allow_finish"],
                    _ => vec![],
                };
                actions.extend(repair());
                (BY_OWNER, close(actions))
            }
            (
                "settlement_blocked",
                "stale_result"
                | "workspace_drift"
                | "task_files_changed"
                | "task_content_unknown"
                | "review_required"
                | "integration_failed",
            ) => (BY_OWNER, close(repair())),
            // Stopped by the owner, unknown state, unknown finish outcome, or a
            // dependency that cannot finish: only the owner can settle it.
            (
                "settlement_blocked",
                "task_stopped"
                | "task_state_unknown"
                | "outcome_unknown"
                | "integration_outcome_unknown"
                | "dependencies_blocked"
                | "queue_blocked",
            ) => (BY_OWNER, close(vec![])),
            // A finish precondition outside the task (a branch, the target checkout,
            // Git): rechecked on every pass, and the owner can ask for it now.
            ("settlement_blocked", _) => (BY_OWNER, close(vec!["retry"])),
            _ => (BY_OWNER, close(vec!["retry"])),
        }
    }

    /// Who acts first on a hold condition. For agent-first conditions the scheduler
    /// sends the work back by itself (bounded, deduplicated) and persists a hold,
    /// decided by `hold_decision`, only when no agent repair can start.
    pub(super) fn first_actor(kind: &str, code: &str) -> &'static str {
        match (kind, code) {
            (
                "settlement_blocked",
                "task_failed"
                | "automatic_acceptance_blocked"
                | "stale_result"
                | "workspace_drift"
                | "task_files_changed"
                | "review_required",
            ) => BY_AGENT,
            _ => hold_decision(kind, code, None, &HoldFacts::default()).0,
        }
    }

    /// Whether the owner can let the recorded result finish (Automatic) or accept it
    /// (Hybrid) as it is. Only Automatic's own finish needs the project folder; a Hybrid
    /// acceptance works in a separate worktree too, and merging stays the owner's.
    pub(super) fn finish_offered(sprint: &Value, record: &Value, code: &str, unsure: bool) -> bool {
        let actions = &record["reviewActions"];
        code == "automatic_acceptance_blocked"
            && !unsure
            && (sprint["runMode"] == "hybrid" || record["checkout"]["kind"] == "main")
            && record["conflict"].as_str().is_none_or(|c| c == "none")
            && record["integration"].is_null()
            && (actions["accept"] == true || actions["acknowledgeOpenChecks"] == true)
    }

    /// The owner's plain-language request for the primary action, when the hold
    /// carries no more specific items.
    fn default_ask(action: &str) -> &'static str {
        match action {
            "allow_finish" => "Allow Automatic to finish this result with its recorded checks.",
            "accept_result" => "Accept this result with its recorded checks. The queue then continues; merging stays your decision.",
            "allow_repair" => "Let the agent fix this and publish a fresh result. Automatic checks it again and finishes the task when it passes.",
            "allow_retries" => "Allow the agent more automatic attempts. It continues from the recorded state.",
            "confirm_and_continue" => "Check the recorded work, then let the agent continue from the current state. Actions whose outcome is unknown are not repeated.",
            "resume_task" => "Resume the task you paused. Automatic then continues the queue.",
            "retry" => "Fix the cause named above, then let Automatic try again.",
            "confirm_integration" => "Confirm how to bring this result into the target branch.",
            "resolve_conflict" => "Resolve the conflict with the target branch.",
            "change_run_mode" => "Automatic runs only in the project folder. Switch this sprint to Hybrid or Manual to use a separate worktree.",
            _ => "Close the task as is. The queue then continues with the next task.",
        }
    }

    pub(super) fn automatic_hold(
        kind: &str,
        task: Option<&str>,
        code: &str,
        reason: &str,
        facts: &HoldFacts,
    ) -> Value {
        let (actor, actions) = hold_decision(kind, code, task, facts);
        let asks: Vec<String> = if actor != BY_OWNER {
            vec![]
        } else if facts.asks.is_empty() {
            vec![default_ask(actions.first().copied().unwrap_or("close_as_is")).to_owned()]
        } else {
            facts.asks.iter().take(10).cloned().collect()
        };
        json!({"kind":kind,"taskId":task,"code":code,"reason":reason,
            "needsOwner":actor == BY_OWNER,"nextActor":actor,"ownerActions":actions,"asks":asks})
    }

    /// Dispatched actions of a held task whose outcome is still unknown, the same
    /// receipts `require_known_effects` refuses on: supervised commands only, which
    /// the runtime settles by itself, or any other effect it can never confirm.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub(super) enum Unsettled {
        Commands,
        Effects,
    }
    pub(super) const SETTLING_REASON: &str = "Waiting for an unconfirmed command of this task to settle. Automatic continues by itself once it has.";
    pub(super) const UNSETTLED_REASON: &str = "An action of this task never confirmed whether it finished, and Cortex cannot find out by itself.";
    pub(super) const UNSETTLED_CLOSE_ASK: &str = "Check the project folder, then close the task as is. Nothing is repeated, and the unconfirmed action stays recorded as unknown, never as done.";
    /// The owner actions whose handlers accept a task with an unknown effect. Allow a
    /// repair, more attempts, and a finish or acceptance are refused or unsafe until
    /// it is known; Close as is records the owner's acknowledgement (see `task_actions`).
    pub(super) const ACCEPTED_UNSETTLED: &[&str] = &[
        "confirm_and_continue",
        "resume_task",
        "retry",
        "confirm_integration",
        "resolve_conflict",
        "change_run_mode",
        "close_as_is",
    ];

    /// The hold persisted while a task has an unknown effect. While the runtime can
    /// still settle it (supervised commands, with no time cap) the runtime
    /// acts next and the owner is asked nothing; otherwise the owner is offered only
    /// actions the service accepts. The decision it replaces is kept in `settling.hold`
    /// and comes back once the effect is known. A hold without a task, a refused start,
    /// and a task whose effects are known are unchanged.
    pub(super) fn settling_hold(
        hold: Value,
        prior: &Value,
        unsettled: Option<Unsettled>,
        now: i64,
    ) -> Value {
        let original = if hold["settling"]["hold"].is_object() {
            hold["settling"]["hold"].clone()
        } else {
            hold
        };
        let Some(unsettled) = unsettled else {
            return original;
        };
        if original["taskId"].as_str().is_none() || original["kind"] == "start_refused" {
            return original;
        }
        let since = prior["settling"]["since"]
            .as_i64()
            .filter(|_| prior["taskId"] == original["taskId"])
            .unwrap_or(now);
        // No-stop policy: the runtime keeps waiting for its own command evidence (no
        // time cap); only an effect it can never confirm reaches the owner here.
        let waiting = unsettled == Unsettled::Commands;
        let (actor, reason, actions, asks) = if waiting {
            (BY_RUNTIME, SETTLING_REASON, vec![], vec![])
        } else {
            let mut actions: Vec<&str> = original["ownerActions"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|a| *a != "close_as_is")
                .filter_map(|a| ACCEPTED_UNSETTLED.iter().copied().find(|k| *k == a))
                .collect();
            let mut asks: Vec<String> = actions.iter().map(|a| default_ask(a).to_owned()).collect();
            actions.push("close_as_is");
            asks.push(UNSETTLED_CLOSE_ASK.to_owned());
            (BY_OWNER, UNSETTLED_REASON, actions, asks)
        };
        json!({"kind":original["kind"],"taskId":original["taskId"],"code":original["code"],
            "reason":reason,"needsOwner":actor == BY_OWNER,"nextActor":actor,
            "ownerActions":actions,"asks":asks,
            "settling":{"since":since,"owner":actor == BY_OWNER,"hold":original}})
    }

    /// Codes the running queue derives again from the task once its effects are
    /// known, without repeating any effect. Any other settled hold is restored as it was.
    fn rederived(kind: &str, code: &str) -> bool {
        kind == "continuation"
            || kind == "settlement_blocked"
                && matches!(
                    code,
                    "task_failed"
                        | "task_blocked"
                        | "task_recovery"
                        | "task_stopped"
                        | "task_paused"
                        | "task_state_unknown"
                        | "dependencies_blocked"
                        | "task_missing"
                        | "conflict_resolving"
                        | "task_conflict"
                        | "queue_blocked"
                        | "archived_task"
                        | "stale_result"
                        | "workspace_drift"
                        | "task_files_changed"
                        | "review_required"
                        | "automatic_acceptance_blocked"
                )
    }

    /// Which unknown effects a sprint task's runtime has, read from the same receipts
    /// `require_known_effects` checks. The runtime re-settles every command receipt on its
    /// own passes (`resettle_orphaned_commands`) once nothing can still run it; a payload
    /// that outlives its supervisor keeps it open until the host settles it. Any
    /// other receipt (a write, an edit, a call) can never be settled by it.
    pub(super) async fn unsettled_effects(
        runtime: &Runtime,
        project: &str,
        task: &str,
    ) -> Option<Unsettled> {
        let (project, task) = (project.to_owned(), task.to_owned());
        let read = runtime.host.executor.read(move |db| {
            use rusqlite::OptionalExtension;
            let runtime: Option<String> = db
                .query_row(
                    "SELECT e.runtime_task_id FROM workspace_tasks w JOIN workspace_executions e
                     ON e.execution_id=w.execution_id AND e.task_id=w.task_id
                     WHERE w.project_id=? AND w.task_id=?",
                    (&project, &task),
                    |row| row.get(0),
                )
                .optional()?;
            let Some(runtime) = runtime else {
                return Ok(None);
            };
            Ok(db.query_row(
                "SELECT max(operation NOT IN ('command_run','outside_command'))
                 FROM runtime_requests WHERE task_id=?
                    AND operation IN ('outside_command','outside_write','command_run','project_write','project_edit',
                        'project_mkdir','project_delete','project_skill_link','draft_write','external_call')
                    AND json_extract(result_json,'$.complete') IS NOT 1
                    AND json_extract(result_json,'$.status') IS NOT 'complete'
                    AND json_extract(result_json,'$.settled') IS NOT 1",
                [&runtime],
                |row| row.get::<_, Option<bool>>(0),
            )?)
        });
        match read.ok()?.await {
            Ok(Ok(Some(true))) => Some(Unsettled::Effects),
            Ok(Ok(Some(false))) => Some(Unsettled::Commands),
            _ => None,
        }
    }

    /// The same hold with its decision recomputed. A saved hold from an earlier
    /// version (no `nextActor`) is rewritten on the next pass.
    fn hold_current(prior: &Value) -> bool {
        prior["nextActor"].is_string()
            && prior["ownerActions"].is_array()
            && prior["asks"].is_array()
    }

    fn continuation_hold(task: &str, t: &Value) -> Option<Value> {
        let held = &t["automaticContinuation"]["pause"];
        held.is_object().then(|| {
            let code = held["kind"].as_str().unwrap_or("outcome_unknown");
            let reason = match code {
                "continuation_cap" | "owner_blocker" => "The agent continues this task with a changed approach.",
                "outcome_unknown" => "An action of this task may or may not have finished. The agent checks it and continues; it is not repeated blindly.",
                _ => match held["reason"].as_str() {
                    Some("engine_authentication_required" | "authorization_required") => "The engine needs you to sign in again. Automatic continues by itself once it can connect.",
                    Some("engine_quota_exhausted") => "The engine usage limit is reached. Automatic continues by itself when the engine is available again.",
                    Some(cortex_core::lifecycle::automatic::NO_MODEL_LISTED_FOR_REVIEW) => "No model may review this work: no usable model has Review in its Preferred work. Add Review to a model in Settings > Orchestration > Engines and models; Automatic continues by itself once one can review.",
                    Some(cortex_core::lifecycle::automatic::NO_MODEL_LISTED_FOR_WORK_KIND) => "No usable model has this kind of work in its Preferred work. Add it to a model in Settings > Orchestration > Engines and models; Automatic continues by itself once one can do it.",
                    _ => "The engine or its route is not available right now. Automatic continues by itself once it is.",
                },
            };
            automatic_hold("continuation", Some(task), code, reason, &HoldFacts::default())
        })
    }

    /// Skipped members do not block the queue, but skipping is not completion
    /// evidence for their dependents.
    fn automatic_skip(record: &Value, disposition: Option<&(String, Option<String>)>) -> bool {
        record["purged"] == true
            || !record["archive"].is_null()
            || !record["archiveHold"].is_null()
            || disposition.is_some_and(|(_, closed)| {
                matches!(closed.as_deref(), Some("cancel" | "unconfirmed"))
            })
    }

    /// Whether the runtime's task projection carries this warning.
    fn warned(task: &Value, warning: &str) -> bool {
        task["warnings"]
            .as_array()
            .is_some_and(|w| w.iter().any(|v| v == warning))
    }

    #[cfg(test)]
    #[test]
    fn automatic_queue_skips_a_retained_archive_without_certifying_its_work() {
        let members = ["archived", "dependent", "independent"].map(str::to_owned);
        let tasks = ["dependent", "independent"]
            .into_iter()
            .map(|id| (id.to_owned(), json!({"phase":"not_started"})))
            .collect();
        let records = json!({
            "archived":{"archive":{"at":1}},
            "dependent":{"dependencies":["archived"]},
            "independent":{}
        });
        let mut records = records;
        let disposition = HashMap::from([("archived".into(), ("ready_to_merge".into(), None))]);
        records["archived"]["archive"]["unfinished"] = json!(true);
        for hybrid in [false, true] {
            assert!(matches!(
                automatic_queue_step(&members, &tasks, &records, &disposition, hybrid),
                QueueStep::Start("independent")
            ));
        }
        // Even a restored draft must not return to either automatic queue.
        records["archived"]["archive"] = Value::Null;
        records["archived"]["archiveHold"] = json!("draft");
        for hybrid in [false, true] {
            assert!(matches!(
                automatic_queue_step(&members, &tasks, &records, &disposition, hybrid),
                QueueStep::Start("independent")
            ));
        }
    }

    #[cfg(test)]
    #[test]
    fn a_stopped_member_names_its_actor_and_never_says_open_it() {
        let members = ["held".to_owned()];
        let records = json!({"held":{}});
        let expect = [
            ("failed", "task_failed"),
            ("blocked", "task_blocked"),
            ("stopped", "task_stopped"),
            ("paused", "task_paused"),
            ("unknown", "task_state_unknown"),
            ("recovery", "task_recovery"),
        ];
        for (phase, code) in expect {
            let tasks = [("held".to_owned(), json!({"phase":phase}))]
                .into_iter()
                .collect();
            let QueueStep::Hold(hold) =
                automatic_queue_step(&members, &tasks, &records, &HashMap::new(), false)
            else {
                panic!("{phase} must hold the queue");
            };
            assert_eq!(hold["code"], code);
            assert_eq!(hold["nextActor"], BY_OWNER, "{hold}");
            let reason = hold["reason"].as_str().unwrap();
            assert!(
                !reason.contains("Open") && !reason.contains('_'),
                "{reason}"
            );
            assert!(
                !hold["ownerActions"].as_array().unwrap().is_empty(),
                "{hold}"
            );
        }
        assert_eq!(first_actor("settlement_blocked", "task_failed"), BY_AGENT);
        // Waiting on the continuation the runtime already owns is no hold at all.
        let tasks = [(
            "held".to_owned(),
            json!({"phase":"recovery","waitReason":"automatic_continuation_pending"}),
        )]
        .into_iter()
        .collect();
        assert!(matches!(
            automatic_queue_step(&members, &tasks, &records, &HashMap::new(), false),
            QueueStep::Wait
        ));
        // A session still being stopped is the runtime's to decide: no hold.
        let tasks = [(
            "held".to_owned(),
            json!({"phase":"recovery","waitReason":null,"warnings":["process_exit_pending"]}),
        )]
        .into_iter()
        .collect();
        assert!(matches!(
            automatic_queue_step(&members, &tasks, &records, &HashMap::new(), false),
            QueueStep::Wait
        ));
        // An exit the host recorded as unconfirmed is never "I checked it, continue"
        // (continuing is refused until it is known): the owner can only close it as is.
        let tasks = [(
            "held".to_owned(),
            json!({"phase":"recovery","waitReason":null,"warnings":["process_exit_unconfirmed"]}),
        )]
        .into_iter()
        .collect();
        let QueueStep::Hold(hold) =
            automatic_queue_step(&members, &tasks, &records, &HashMap::new(), false)
        else {
            panic!("an unconfirmed exit must hold the queue");
        };
        assert_eq!(hold["code"], "task_state_unknown");
        assert_eq!(hold["nextActor"], BY_OWNER);
        assert_eq!(hold["ownerActions"], json!(["close_as_is"]));
    }

    enum QueueStep<'a> {
        Hold(Value),
        Complete,
        Wait,
        Start(&'a str),
    }

    #[cfg(test)]
    #[test]
    fn owner_merge_handoff_requires_explicit_checks_in_both_queue_modes() {
        let members = vec!["t".to_owned()];
        let tasks = HashMap::from([("t".to_owned(), json!({"phase":"result"}))]);
        for state in ["not_requested", "request_unknown"] {
            let records = json!({"t":{"conflict":"none","merge":{"freshChecks":{"state":state}}}});
            for hybrid in [false, true] {
                let QueueStep::Hold(hold) =
                    automatic_queue_step(&members, &tasks, &records, &HashMap::new(), hybrid)
                else {
                    panic!("fresh checks require the owner in both modes");
                };
                assert_eq!(hold["code"], "task_conflict");
                assert_eq!(hold["nextActor"], BY_OWNER);
                assert_eq!(hold["ownerActions"][0], "resolve_conflict");
            }
        }
    }

    fn awaiting_owner_merge_checks(record: &Value) -> bool {
        matches!(record["merge"]["freshChecks"]["state"].as_str(),
            Some("not_requested" | "request_unknown"))
            && record["reviewActions"]["accept"] != true
    }

    type Disposition = HashMap<String, (String, Option<String>)>;

    fn dependency_settled(id: &str, records: &Value, disposition: &Disposition, hybrid: bool) -> bool {
        disposition.get(id).is_some_and(|(status, closed)| {
            (status == "done" || hybrid && status == "ready_to_merge")
                && !matches!(closed.as_deref(), Some("cancel" | "unconfirmed"))
                && records[id]["purged"] != true
                && records[id]["archive"]["unfinished"] != true
                && records[id]["archiveHold"].is_null()
        })
    }

    fn dependencies_ready(record: &Value, done: impl Fn(&str) -> bool) -> bool {
        match record.get("dependencies") {
            None => true,
            Some(Value::Array(deps)) => deps.iter().all(|dep| dep.as_str().is_some_and(&done)),
            _ => false,
        }
    }

    #[cfg(test)]
    fn automatic_queue_step<'a>(
        members: &'a [String], tasks: &HashMap<String, Value>, records: &Value,
        disposition: &Disposition, hybrid: bool,
    ) -> QueueStep<'a> {
        queue_step_in_order(members, members, tasks, records, disposition, hybrid)
    }

    fn queue_step_in_order<'a>(
        members: &'a [String],
        priority: &'a [String],
        tasks: &HashMap<String, Value>,
        records: &Value,
        disposition: &Disposition,
        hybrid: bool,
    ) -> QueueStep<'a> {
        let done = |id: &str| dependency_settled(id, records, disposition, hybrid);
        let pending: Vec<_> = members
            .iter()
            .map(String::as_str)
            .filter(|id| !done(id) && !automatic_skip(&records[*id], disposition.get(*id)))
            .collect();
        let mut waiting = false;
        let none = HoldFacts::default();
        for &id in &pending {
            let Some(task) = tasks.get(id) else {
                return QueueStep::Hold(automatic_hold(
                    "settlement_blocked",
                    None,
                    "task_missing",
                    "A sprint task could not be read. Automatic checks again by itself.",
                    &none,
                ));
            };
            let record = &records[id];
            if matches!(record["merge"]["handoff"]["state"].as_str(), Some("unknown" | "dispatch_unknown"))
                || record["merge"]["gitState"] == "unknown"
            {
                return QueueStep::Hold(automatic_hold(
                    "settlement_blocked", Some(id), "task_conflict",
                    "Inspect the retained merge. Cortex will not repeat the handoff or touch the target.",
                    &none,
                ));
            }
            if awaiting_owner_merge_checks(record) && matches!(
                task["phase"].as_str(), Some("result" | "failed" | "paused" | "stopped")
            ) {
                return QueueStep::Hold(automatic_hold(
                    "settlement_blocked", Some(id), "task_conflict",
                    "This merge needs a request for fresh independent review and verification.",
                    &none,
                ));
            }
            match record["conflict"].as_str() {
                Some("resolving") => {
                    return QueueStep::Hold(automatic_hold(
                        "settlement_blocked",
                        Some(id),
                        "conflict_resolving",
                        "A conflict of this task is being resolved. Automatic continues when it is done.",
                        &none,
                    ));
                }
                Some("conflict" | "failed") => {
                    return QueueStep::Hold(automatic_hold(
                        "settlement_blocked",
                        Some(id),
                        "task_conflict",
                        "This task has a conflict with the target branch.",
                        &none,
                    ));
                }
                _ => {}
            }
            let (code, reason) = match task["phase"].as_str() {
                Some("not_started") => continue,
                // Repaired by the agent first (see `queue_pass`); held only after the bounded repairs.
                Some("failed") => ("task_failed", "The last run of this task ended as failed."),
                Some("blocked") => (
                    "task_blocked",
                    "The task could not continue: its engine or a required step did not start.",
                ),
                Some("stopped") => ("task_stopped", "This task was stopped before it finished."),
                Some("paused") => ("task_paused", "You paused this task."),
                // A session still being stopped: its shutdown observation decides it.
                Some("recovery") if warned(task, "process_exit_pending") => {
                    waiting = true;
                    continue;
                }
                // Continuing is refused while the exit is unknown. Each pass re-checks it
                // first (`resettle_unconfirmed_exits`) and, once settled, the task goes on
                // by itself; while the evidence stays inconclusive only Close as is (which
                // replays nothing) ends it.
                Some("recovery") if warned(task, "process_exit_unconfirmed") => (
                    "task_state_unknown",
                    "A process of this task did not confirm that it stopped. Nothing is repeated, and the task cannot continue until that is known.",
                ),
                Some("recovery") if task["waitReason"] != "automatic_continuation_pending" => (
                    "task_recovery",
                    "The task stopped and some of its actions may not have finished. They are not repeated automatically.",
                ),
                Some("unknown") | None => (
                    "task_state_unknown",
                    "How this task ended cannot be confirmed. Nothing is repeated automatically.",
                ),
                _ => {
                    waiting = true;
                    continue;
                }
            };
            return QueueStep::Hold(automatic_hold(
                "settlement_blocked",
                Some(id),
                code,
                reason,
                &none,
            ));
        }
        if waiting {
            return QueueStep::Wait;
        }
        if pending.is_empty() {
            return QueueStep::Complete;
        }
        let unmet = |id: &str| {
            records[id]["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|dep| !dep.as_str().is_some_and(done))
                .map(|dep| dep.as_str().unwrap_or_default().to_owned())
        };
        // A draft that a proposed sprint revision holds back waits for it, without a hold.
        let held = |id: &str| records[id]["revisionPending"] == true;
        let eligible: BTreeSet<_> = pending.iter().copied().filter(|id| !held(id)).collect();
        if let Some(id) = priority.iter().map(String::as_str)
            .find(|id| eligible.contains(id) && dependencies_ready(&records[*id], done))
        {
            return QueueStep::Start(id);
        }
        if pending.iter().any(|id| held(id)) {
            return QueueStep::Wait;
        }
        // Name the task that is actually stuck: one whose unfinished prerequisite this
        // queue will never run (cancelled, skipped or outside the sprint), not merely
        // the first task waiting behind it. A cycle falls back to the first pending task.
        let stuck = |id: &str| {
            records[id]["dependencies"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .find(|dep| !done(dep) && !pending.contains(dep))
                .map(str::to_owned)
        };
        let (blocked, dep) = match pending
            .iter()
            .copied()
            .find_map(|id| Some((id, stuck(id)?)))
        {
            Some((id, dep)) => (Some(id), Some(dep)),
            None => {
                let first = pending.first().copied();
                (first, first.and_then(unmet))
            }
        };
        let name = dep
            .as_deref()
            .and_then(|d| records[d]["title"].as_str())
            .filter(|t| !t.trim().is_empty())
            .map(|t| {
                format!(
                    "\u{201c}{}\u{201d}",
                    t.trim().chars().take(120).collect::<String>()
                )
            })
            .unwrap_or_else(|| "a task it depends on".to_owned());
        let cancelled = dep.as_deref().is_some_and(|d| {
            disposition
                .get(d)
                .is_some_and(|(_, closed)| closed.as_deref() == Some("cancel"))
                || records[d]["closedAs"] == "cancel"
        });
        let reason = if cancelled {
            format!(
                "This task depends on {name}, which was cancelled, so it cannot start by itself."
            )
        } else {
            format!("This task depends on {name}, which is not finished and cannot be finished by this queue.")
        };
        let facts = HoldFacts {
            asks: vec![format!(
                "Close this task as is or cancel it, because {name} will not finish. The queue then continues."
            )],
            ..HoldFacts::default()
        };
        QueueStep::Hold(automatic_hold(
            "settlement_blocked",
            blocked,
            "dependencies_blocked",
            &reason,
            &facts,
        ))
    }
    fn queue_disposition(snapshot: &Snapshot) -> Disposition {
        snapshot.roster.iter().map(|id| {
            let fact = snapshot.tasks.get(id);
            let mut disposition = member_disposition(&snapshot.records[id], fact);
            // Neither a missing binding nor a contradictory Core result proves settlement.
            if snapshot.projection_member(id).is_some_and(|(_, _, status, _)| status != "confirmed")
                && !matches!(disposition.1.as_deref(), Some("cancel" | "unconfirmed"))
            {
                disposition.0.clear();
            }
            (id.clone(), disposition)
        }).collect()
    }

    fn automatic_checkout_unavailable(sprint: &Value) -> bool {
        sprint["checkout"]["kind"].as_str().is_some_and(|kind| kind != "main")
    }

    fn observed_step<'a>(snapshot: &'a Snapshot, disposition: &Disposition, hybrid: bool) -> QueueStep<'a> {
        queue_step_in_order(&snapshot.roster, &snapshot.priority, &snapshot.tasks,
            &snapshot.records, disposition, hybrid)
    }

    pub(crate) fn selected_now<'a>(snapshot: &'a Snapshot) -> Option<&'a str> {
        let sprint = &snapshot.sprint;
        if sprint["queueState"] != "running" || !crate::machine::queue_driven_mode(sprint)
            || (!sprint["deleting"].is_null() && sprint["deleting"] != false)
        {
            return None;
        }
        let hybrid = sprint["runMode"] == "hybrid";
        let disposition = queue_disposition(snapshot);
        if !hybrid && (automatic_checkout_unavailable(sprint) || snapshot.roster.iter().any(|id| {
            !automatic_skip(&snapshot.records[id], disposition.get(id))
                && !dependency_settled(id, &snapshot.records, &disposition, false)
                && snapshot.tasks.get(id).is_some_and(|t| continuation_hold(id, t).is_some())
        })) {
            return None;
        }
        match observed_step(snapshot, &disposition, hybrid) {
            QueueStep::Start(id) => Some(id),
            _ => None,
        }
    }

    /// The first pick uses the queue decision itself. Future picks assume only
    /// successful settlement and never skip a current hold to promise a launch.
    pub(crate) fn live_execution_order(snapshot: &Snapshot) -> Value {
        let sprint = &snapshot.sprint;
        let mode = sprint["runMode"].as_str().filter(|m| matches!(*m, "hybrid" | "automatic")).unwrap_or("manual");
        let queue = sprint["queueState"].as_str().filter(|s| matches!(*s, "idle" | "running" | "paused" | "complete")).unwrap_or("idle");
        let closed = !sprint["archivedAt"].is_null()
            || (!sprint["deleting"].is_null() && sprint["deleting"] != false);
        let hybrid_settlement = mode != "automatic";
        let disposition = queue_disposition(snapshot);
        let mut working = Vec::new();
        let mut pending = BTreeSet::new();
        let mut unranked = HashMap::new();
        let mut simulated = BTreeSet::new();
        for id in &snapshot.roster {
            let (draft, claim, status, _) = snapshot.projection_member(id).unwrap();
            let record = &snapshot.records[id];
            let fact = snapshot.tasks.get(id);
            let phase = fact.and_then(|v| v["phase"].as_str());
            if dependency_settled(id, &snapshot.records, &disposition, hybrid_settlement) {
                simulated.insert(id.as_str());
            }
            if automatic_skip(record, disposition.get(id)) {
                continue;
            }
            if status != "confirmed" || phase == Some("unknown") {
                unranked.insert(id.as_str(), if status == "missing" { "task_missing" } else { "task_state_unknown" });
                continue;
            }
            if matches!(phase, Some("planning" | "execution" | "review" | "verification" | "finalizing")) {
                working.push((claim, id.as_str()));
            }
            if draft && record["revisionPending"] == true {
                unranked.insert(id.as_str(), "sprint_revision_pending");
            } else if draft {
                pending.insert(id.as_str());
            } else if fact.is_some_and(|t| t["executionId"].is_string())
                && matches!(phase, Some("planning" | "execution" | "review" | "verification" | "finalizing" |
                    "waiting" | "result"))
            {
                // The queue might get a successfully settled result before it can
                // start a draft. A failure, cancellation or hold is not assumed away.
                simulated.insert(id.as_str());
            }
        }
        working.sort_by_key(|&(claim, id)| (claim.is_none(), claim, id));
        let working: Vec<_> = working.into_iter().map(|(ordinal, id)| {
            json!({"taskId":id,"startOrder":ordinal.map(|x| x.to_string())})
        }).collect();
        let next_candidate = selected_now(snapshot);
        let step = observed_step(snapshot, &disposition, hybrid_settlement);
        let hold_code = if closed {
            Some("sprint_closed")
        } else if mode != "manual" && queue == "paused" {
            Some(if system_hold(sprint) { "queue_held" } else { "queue_paused" })
        } else if mode == "automatic" && automatic_checkout_unavailable(sprint) {
            Some("native_worktree_unavailable")
        } else if mode != "manual" && queue == "complete" {
            Some("queue_complete")
        } else if mode != "manual" && matches!(&step, QueueStep::Hold(hold) if hold["code"] != "dependencies_blocked") {
            Some("queue_blocked")
        } else {
            None
        };
        let mut not_started = Vec::new();
        let mut stop = hold_code;
        while stop.is_none() {
            // Prefer the exact live pick before simulating future successful
            // settlements; this avoids treating unsettled prerequisites as done now.
            let candidate = if not_started.is_empty() && next_candidate.is_some() {
                next_candidate
            } else {
                snapshot.priority.iter().map(String::as_str).find(|id| {
                    pending.contains(id) && dependencies_ready(&snapshot.records[*id], |d| simulated.contains(d))
                })
            };
            let Some(id) = candidate else { break; };
            if !pending.contains(id) { break; }
            if mode == "automatic" && snapshot.projection_member(id).is_some_and(|(_, _, _, checkout)| checkout == Some("worktree")) {
                unranked.insert(id, "native_worktree_unavailable");
                pending.remove(id);
                stop = Some("queue_blocked");
                break;
            }
            pending.remove(id);
            let is_next = not_started.is_empty() && next_candidate == Some(id);
            not_started.push(json!({"taskId":id,"rank":not_started.len()+1,
                "reason":null,"next":is_next,"conditional":mode != "manual" && !is_next}));
            simulated.insert(id);
        }
        for id in &snapshot.priority {
            let reason = unranked.get(id.as_str()).copied().or_else(|| {
                pending.contains(id.as_str()).then_some(stop.unwrap_or("dependencies_blocked"))
            });
            if let Some(reason) = reason {
                not_started.push(json!({"taskId":id,"rank":null,"reason":reason,"next":false,"conditional":false}));
            }
        }
        json!({"mode":mode,"queueState":queue,"working":working,"notStarted":not_started})
    }

    fn other(code: String) -> io::Error {
        io::Error::other(code)
    }
    /// Least time between two passes of one project, however often it is woken: no wake-up storm
    /// costs more than the fixed two-second loop this replaces did.
    pub(crate) const PASS_FLOOR: Duration = PASS_EVERY;
    /// A project with a sprint in motion (a queue running, planning, or results waiting for an
    /// automatic settle) is looked at this often even without an event, for a change no event
    /// reports (a runtime whose reader is not open yet, a settle that deferred). Others never are.
    pub(crate) const SAFETY_TICK: Duration = Duration::from_secs(30);

    /// Projects whose sprints should be looked at now, named by whatever changed them.
    #[derive(Default)]
    pub(crate) struct SprintWake {
        woken: std::sync::Mutex<BTreeSet<String>>,
        notify: tokio::sync::Notify,
    }
    impl SprintWake {
        /// Something that can move this project's sprints on changed.
        pub(crate) fn project(&self, project: &str) {
            self.woken
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(project.to_owned());
            self.notify.notify_one();
        }
        fn take(&self) -> BTreeSet<String> {
            std::mem::take(&mut *self.woken.lock().unwrap_or_else(|e| e.into_inner()))
        }
    }

    /// When each project's sprints are next passed. A project is passed once when it first
    /// appears (service start or activation), on an owner or loop change of its sprint or task
    /// records, on a runtime task event while it has a sprint in motion, and on the safety tick
    /// while it has one. A project with no sprint in motion costs nothing between changes.
    #[derive(Default, Debug)]
    pub(crate) struct Schedule {
        projects: std::collections::BTreeMap<String, Slot>,
    }
    #[derive(Default, Debug)]
    struct Slot {
        last: Option<tokio::time::Instant>,
        live: bool,
        pending: bool,
    }
    impl Schedule {
        /// The projects that exist now: a new one is due at once, a gone one is forgotten.
        pub(crate) fn known<'a>(&mut self, names: impl IntoIterator<Item = &'a str>) {
            let names: BTreeSet<&str> = names.into_iter().collect();
            self.projects.retain(|p, _| names.contains(p.as_str()));
            for name in names {
                self.projects.entry(name.to_owned()).or_insert(Slot {
                    pending: true,
                    ..Slot::default()
                });
            }
        }
        /// An owner or loop change of this project's sprint or task records.
        pub(crate) fn wake(&mut self, project: &str) {
            if let Some(slot) = self.projects.get_mut(project) {
                slot.pending = true;
            }
        }
        /// Whether the last pass of `project` found something in motion.
        pub(crate) fn live(&self, project: &str) -> bool {
            self.projects.get(project).is_some_and(|s| s.live)
        }
        /// A runtime task event: it matters only to a project with a sprint in motion.
        pub(crate) fn task_event(&mut self, project: &str) {
            if let Some(slot) = self.projects.get_mut(project).filter(|s| s.live) {
                slot.pending = true;
            }
        }
        /// Events were lost: every project with a sprint in motion is looked at again.
        pub(crate) fn lagged(&mut self) {
            for slot in self.projects.values_mut().filter(|s| s.live) {
                slot.pending = true;
            }
        }
        /// The projects to pass now, each no sooner than `PASS_FLOOR` after its last pass.
        pub(crate) fn due(&mut self, now: tokio::time::Instant) -> Vec<String> {
            let mut due = vec![];
            for (name, slot) in &mut self.projects {
                if slot.live && slot.last.is_some_and(|at| now >= at + SAFETY_TICK) {
                    slot.pending = true;
                }
                if slot.pending && slot.last.is_none_or(|at| now >= at + PASS_FLOOR) {
                    slot.pending = false;
                    due.push(name.clone());
                }
            }
            due
        }
        /// A pass of `project` ended; `live` says whether it has a sprint in motion.
        pub(crate) fn passed(&mut self, project: &str, live: bool, now: tokio::time::Instant) {
            if let Some(slot) = self.projects.get_mut(project) {
                slot.last = Some(now);
                slot.live = live;
            }
        }
        /// When the loop must look again without any wake: a deferred wake past its floor, or a
        /// safety tick. `None` when no project has anything pending or in motion.
        pub(crate) fn next(&self) -> Option<tokio::time::Instant> {
            self.projects
                .values()
                .filter_map(|slot| match (slot.pending, slot.live, slot.last) {
                    (true, _, None) => Some(tokio::time::Instant::now()),
                    (true, _, Some(at)) => Some(at + PASS_FLOOR),
                    (false, true, Some(at)) => Some(at + SAFETY_TICK),
                    _ => None,
                })
                .min()
        }
    }
    /// A service event that can move a sprint on: a runtime task change of that project.
    fn task_event(event: &Value) -> Option<&str> {
        let project = event["projectId"].as_str()?;
        match event["type"].as_str()? {
            "native_resync" => Some(project),
            "native_delta" => event["batch"]["changes"]
                .as_array()?
                .iter()
                .any(|c| c["kind"] == "task")
                .then_some(project),
            _ => None,
        }
    }
    #[cfg(test)]
    mod schedule_tests {
        use super::*;
        use serde_json::json;
        #[test]
        fn a_project_with_no_sprint_in_motion_costs_nothing_until_it_changes() {
            let mut s = Schedule::default();
            s.known(["p"]);
            let now = tokio::time::Instant::now();
            // Service start: one pass.
            assert_eq!(s.due(now), vec!["p".to_owned()]);
            s.passed("p", false, now);
            assert_eq!(s.next(), None, "nothing in motion: no timer at all");
            // Hours pass, runtime tasks change: nothing is read.
            let later = now + Duration::from_secs(3600);
            s.task_event("p");
            s.lagged();
            assert!(s.due(later).is_empty());
            assert_eq!(s.next(), None);
            // An owner change of a sprint (a run mode, Run): one pass.
            s.wake("p");
            assert_eq!(s.due(later), vec!["p".to_owned()]);
        }
        #[test]
        fn a_sprint_in_motion_wakes_on_each_event_and_the_safety_tick() {
            let mut s = Schedule::default();
            s.known(["p", "q"]);
            let now = tokio::time::Instant::now();
            assert_eq!(s.due(now).len(), 2);
            s.passed("p", true, now);
            s.passed("q", false, now);
            assert_eq!(s.next(), Some(now + SAFETY_TICK));
            // A task state change, a settle outcome, a sprint patch: each wakes p, none sooner than the floor.
            for wake in [
                Schedule::task_event as fn(&mut Schedule, &str),
                Schedule::wake,
            ] {
                let at = now + PASS_FLOOR * 10;
                s.passed("p", true, at - PASS_FLOOR * 2);
                wake(&mut s, "p");
                assert_eq!(s.due(at), vec!["p".to_owned()]);
                s.passed("p", true, at);
                wake(&mut s, "p");
                assert!(s.due(at + PASS_FLOOR / 2).is_empty(), "within the floor");
                assert_eq!(s.next(), Some(at + PASS_FLOOR));
                assert_eq!(s.due(at + PASS_FLOOR), vec!["p".to_owned()]);
                s.passed("p", true, at + PASS_FLOOR);
            }
            // q has nothing in motion: its task events are ignored.
            s.task_event("q");
            assert!(!s.due(now + PASS_FLOOR * 100).contains(&"q".to_owned()));
            // With no event, p is looked at on the safety tick only.
            let last = now + PASS_FLOOR * 100;
            s.passed("p", true, last);
            assert!(s
                .due(last + SAFETY_TICK - Duration::from_millis(1))
                .is_empty());
            assert_eq!(s.due(last + SAFETY_TICK), vec!["p".to_owned()]);
            s.passed("p", true, last + SAFETY_TICK);
            // Activation of a new project: passed at once; a removed one is forgotten.
            s.known(["p", "r"]);
            assert_eq!(s.due(last + SAFETY_TICK), vec!["r".to_owned()]);
        }
        #[test]
        fn only_runtime_task_changes_count_as_task_events() {
            let delta = |kind: &str| json!({"type":"native_delta","projectId":"p","batch":{"changes":[{"kind":kind}]}});
            assert_eq!(task_event(&delta("task")), Some("p"));
            assert_eq!(task_event(&delta("history")), None);
            assert_eq!(
                task_event(&json!({"type":"native_resync","projectId":"p"})),
                Some("p")
            );
            // The loop's own writes tell windows to refresh; they are not task events.
            assert_eq!(
                task_event(&json!({"type":"invalidation","projectId":"p"})),
                None
            );
        }
        #[test]
        fn a_wake_names_its_project_once() {
            let wake = SprintWake::default();
            wake.project("p");
            wake.project("p");
            wake.project("q");
            assert_eq!(wake.take().into_iter().collect::<Vec<_>>(), ["p", "q"]);
            assert!(wake.take().is_empty());
        }
    }
    impl Service {
        pub(crate) async fn native_sprints(self: Arc<Self>) {
            let mut shutdown = self.shutdown.subscribe();
            let mut events = self.events.subscribe();
            let mut jobs = Jobs::default();
            let mut seen = BTreeSet::new();
            let mut schedule = Schedule::default();
            while !*shutdown.borrow_and_update() {
                let native: Vec<(String, Arc<Runtime>)> = self
                    .state
                    .lock()
                    .await
                    .native
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                jobs.0.retain(|_, j| !j.is_finished());
                jobs.1
                    .retain(|project, _| native.iter().any(|(id, _)| id == project));
                schedule.known(native.iter().map(|(k, _)| k.as_str()));
                for project in self.sprint_wake.take() {
                    schedule.wake(&project);
                }
                let due: BTreeSet<String> = schedule
                    .due(tokio::time::Instant::now())
                    .into_iter()
                    .collect();
                for (project, runtime) in native.iter().filter(|(k, _)| due.contains(k)) {
                    let first = seen.insert(project.clone());
                    // One project's unreadable state must not stop the others; it is looked at again on the safety tick.
                    let live = self
                        .sprint_pass(project, runtime, &mut jobs, first)
                        .await
                        .unwrap_or(true)
                        || jobs.0.keys().any(|(p, _)| p == project);
                    schedule.passed(project, live, tokio::time::Instant::now());
                }
                let next = schedule.next();
                tokio::select! {
                    _ = shutdown.changed() => (),
                    _ = self.sprint_wake.notify.notified() => (),
                    event = events.recv() => match event {
                        Ok(event) => if let Some(project) = task_event(&event) {
                            // A session exit recorded as unconfirmed is worth a pass in any
                            // project (one read, no process scan); the pass re-checks it.
                            if !schedule.live(project) {
                                if let Some((_, runtime)) = native.iter().find(|(k, _)| k == project) {
                                    if runtime.unconfirmed_exits_pending().await.unwrap_or(false) {
                                        schedule.wake(project);
                                    }
                                }
                            }
                            schedule.task_event(project);
                        },
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => schedule.lagged(),
                        // No sender is left only while the service stops.
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => tokio::time::sleep(SAFETY_TICK).await,
                    },
                    _ = tokio::time::sleep_until(next.unwrap_or_else(tokio::time::Instant::now)), if next.is_some() => (),
                }
            }
        }
        /// One automatic settle. Never an error for the pass: a settle that could not
        /// finish is `Failed` with an owner-readable reason for that task only.
        async fn settle(
            &self,
            project: &str,
            task: &str,
            record: &Value,
            unattended: bool,
        ) -> Settle {
            let title = record["title"]
                .as_str()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or("A result");
            let failed = |why: &str| {
                let suffix = if unattended {
                    ""
                } else {
                    " Open the task to accept it yourself."
                };
                Settle::Failed(format!(
                    "{title} could not be accepted automatically: {why}{suffix}"
                ))
            };
            match self
                .product_call(
                    "product.task",
                    project,
                    json!({"taskId":task,"action":"auto_ready"}),
                )
                .await
            {
                Ok(r) if r["accepted"] == true => Settle::Accepted,
                Ok(r) if r["deferred"] == true => Settle::Deferred,
                Ok(r) if r["failed"].is_string() => failed(
                    r["reason"]
                        .as_str()
                        .unwrap_or("the result could not be read."),
                ),
                Ok(r) => Settle::Waiting(r["reason"].as_str().unwrap_or(UNSETTLED).to_owned()),
                Err(_) => failed("the check stopped before it finished."),
            }
        }

        /// Re-read the same owned result the desktop shows. The board deliberately
        /// omits its checks. Block only on a real owner decision or exhausted repair.
        /// The agent fixes what it can within its granted permissions: the bounded,
        /// deduplicated automatic repair first, then exactly one more repair per owner
        /// consent recorded on the hold. Never for an unknown effect or a partial read.
        /// `automatic` is false where only the owner may send the work back.
        #[allow(clippy::too_many_arguments)]
        async fn automatic_repair(
            &self,
            project: &str,
            runtime: &Arc<Runtime>,
            sprint: &Value,
            task: &str,
            execution: &Value,
            changed_paths: Option<&Value>,
            code: &str,
            reason: &str,
            automatic: bool,
        ) -> io::Result<Repair> {
            use crate::machine::task_auto_repair as repair;
            let Some(execution) = execution.as_str() else {
                return Ok(Repair::Declined(HoldFacts::default()));
            };
            let Ok(product) = self
                .product_call("product.read", project, json!({"taskIds":[task]}))
                .await
            else {
                // Read again on the next pass; an unreadable result is no owner decision.
                return Ok(Repair::Later);
            };
            let record = &product["tasks"][task];
            let result = &record["result"];
            let actions = &record["reviewActions"];
            if actions["pending"] == true {
                return Ok(Repair::Later);
            }
            let (Some(version), Some(checks)) = (
                result["sourceVersion"].as_str(),
                result["checks"].as_array(),
            ) else {
                return Ok(Repair::Declined(HoldFacts::default()));
            };
            let partial = result["checksPartial"] == true;
            let stale = actions["workspaceChanged"] == true
                || matches!(
                    code,
                    "stale_result" | "workspace_drift" | "task_files_changed" | "review_required"
                );
            let mut facts = HoldFacts {
                finish: finish_offered(sprint, record, code, partial || stale),
                ..HoldFacts::default()
            };
            if facts.finish {
                facts.asks = repair::finish_asks(checks);
            }
            if partial || actions["requestChanges"] != true {
                return Ok(Repair::Declined(facts));
            }
            let hold = &sprint["automaticQueue"];
            let consent = (sprint["queueState"] == "paused"
                && hold["taskId"] == task
                && hold["code"] == code
                && hold["nextActor"] == BY_OWNER)
                .then(|| hold["allowedAt"].as_i64())
                .flatten()
                .map(|at| at.to_string());
            let mut context = reason.to_owned();
            if let Some(paths) = changed_paths.and_then(Value::as_array) {
                let shown: Vec<_> = paths.iter().filter_map(Value::as_str).take(12).collect();
                context.push_str(&format!(
                    " Recorded task files (data): {}.",
                    shown.join(", ")
                ));
            }
            let owner_plan = |consent: &str| repair::owner_plan(&context, checks, stale, consent);
            let plan = match consent.as_deref() {
                Some(consent) => owner_plan(consent),
                None if automatic => match changed_paths {
                    Some(paths) => repair::reconcile_files(paths),
                    None => repair::plan(
                        actions,
                        checks,
                        result["warnings"]
                            .as_array()
                            .is_some_and(|items| !items.is_empty()),
                    ),
                },
                None => None,
            };
            let mut refused = repair::Dispatch::Refused(None);
            if let Some(plan) = plan {
                match repair::dispatch(
                    &self.config.state_dir,
                    runtime,
                    project,
                    sprint,
                    task,
                    execution,
                    Some(version),
                    plan,
                    consent.is_some(),
                )
                .await?
                {
                    repair::Dispatch::Started => return Ok(Repair::Started),
                    // Busy decides nothing: asked again next pass, consent kept.
                    repair::Dispatch::Busy => return Ok(Repair::Later),
                    other => refused = other,
                }
            }
            if refused.unknown_effect() {
                facts.asks.push(UNKNOWN_EFFECT_ASK.into());
            } else if consent.is_some() {
                facts.asks.push(NOT_SENT_BACK_ASK.into());
            } else {
                // An acceptance the owner made by hand is never replaced by an agent repair.
                facts.repair = owner_plan("offer").is_some()
                    && !(record["acceptance"].is_object()
                        && record["acceptance"]["automatic"] != true);
            }
            Ok(Repair::Declined(facts))
        }

        /// A run that ended as failed with every effect known goes back to the same
        /// coordinator, bounded like a result repair. Its review and verification run again.
        async fn failed_repair(
            &self,
            project: &str,
            runtime: &Arc<Runtime>,
            sprint: &Value,
            task: &str,
            fact: &Value,
        ) -> io::Result<Repair> {
            let Some(execution) = fact["executionId"].as_str() else {
                return Ok(Repair::Declined(HoldFacts::default()));
            };
            let hold = &sprint["automaticQueue"];
            let consent = (sprint["queueState"] == "paused"
                && hold["taskId"] == task
                && hold["code"] == "task_failed"
                && hold["nextActor"] == BY_OWNER)
                .then(|| hold["allowedAt"].as_i64())
                .flatten();
            let mut plan = crate::machine::task_auto_repair::failed_plan(
                fact["waitReason"].as_str(),
                execution,
            );
            if let Some(at) = consent {
                plan.fingerprint = format!("owner:{at}:{}", plan.fingerprint);
            }
            use crate::machine::task_auto_repair::Dispatch;
            let refused = match crate::machine::task_auto_repair::dispatch(
                &self.config.state_dir,
                runtime,
                project,
                sprint,
                task,
                execution,
                None,
                plan,
                consent.is_some(),
            )
            .await?
            {
                Dispatch::Started => return Ok(Repair::Started),
                // Busy decides nothing: asked again next pass, consent kept.
                Dispatch::Busy => return Ok(Repair::Later),
                refused => refused,
            };
            Ok(Repair::Declined(if refused.unknown_effect() {
                // An effect whose outcome is unknown is never replayed by an agent.
                HoldFacts {
                    asks: vec![UNKNOWN_EFFECT_ASK.into()],
                    ..HoldFacts::default()
                }
            } else if consent.is_some() {
                HoldFacts {
                    asks: vec![NOT_SENT_BACK_ASK.into()],
                    ..HoldFacts::default()
                }
            } else {
                HoldFacts {
                    repair: true,
                    asks: vec!["Let the agent finish this task again. It already used its automatic attempts. Review and verification run again before it can finish.".into()],
                    ..HoldFacts::default()
                }
            }))
        }

        /// Persist a hold, after the agent had its chance to fix a failed run.
        async fn hold_or_repair(
            &self,
            project: &str,
            runtime: &Arc<Runtime>,
            observed: &Snapshot,
            hold: Value,
        ) -> io::Result<()> {
            let (sprint, tasks) = (&observed.sprint, &observed.tasks);
            if hold["code"] == "task_failed" {
                if let Some((id, fact)) = hold["taskId"]
                    .as_str()
                    .and_then(|id| tasks.get(id).map(|f| (id, f)))
                {
                    match self
                        .failed_repair(project, runtime, sprint, id, fact)
                        .await?
                    {
                        Repair::Started | Repair::Later => return Ok(()),
                        Repair::Declined(facts) => {
                            let reason = hold["reason"].as_str().unwrap_or_default().to_owned();
                            return self
                                .observed_pause(project, observed,
                                    automatic_hold(
                                        "settlement_blocked",
                                        Some(id),
                                        "task_failed",
                                        &reason,
                                        &facts,
                                    ),
                                )
                                .await;
                        }
                    }
                }
            }
            self.observed_pause(project, observed, hold).await
        }
        async fn product_call(&self, op: &str, project: &str, params: Value) -> io::Result<Value> {
            // An automatic settle that accepted nothing changed nothing; it is retried on
            // every pass, so it must not make open windows refetch their facts.
            let settle = params["action"] == "auto_ready";
            let call = serde_json::from_value(json!({"op":op,"projectId":project,"params":params}))
                .map_err(|_| crate::failure("invalid_request"))?;
            let reply = self.machine(call).await?;
            if reply["ok"] != true {
                return Err(other(
                    reply["error"]["code"]
                        .as_str()
                        .unwrap_or("machine_refused")
                        .to_owned(),
                ));
            }
            // Owner state changed here in the background (a planner question, a plan, the
            // queue) has no runtime event of its own. Tell open windows at once, so it never
            // waits for their slow facts poll.
            if !matches!(op, "product.board" | "product.read")
                && !(settle && reply["result"]["accepted"] != true)
            {
                let _ = self
                    .events
                    .send(json!({"projectId":project,"type":"invalidation"}));
            }
            Ok(reply["result"].clone())
        }
        async fn sprint_patch(&self, project: &str, sprint: &str, patch: Value) -> io::Result<()> {
            self.product_call(
                "product.sprint",
                project,
                json!({"action":"patch","sprintId":sprint,"patch":patch}),
            )
            .await
            .map(|_| ())
        }
        async fn sprint_note(
            &self,
            project: &str,
            sprint: &str,
            kind: &str,
            text: &str,
        ) -> io::Result<()> {
            self.product_call(
                "product.sprint",
                project,
                json!({"action":"note","sprintId":sprint,"kind":kind,"text":text}),
            )
            .await
            .map(|_| ())
        }
        async fn sprint_pass(
            self: &Arc<Self>,
            project: &str,
            runtime: &Arc<Runtime>,
            jobs: &mut Jobs,
            first: bool,
        ) -> io::Result<bool> {
            // Exits a session could not confirm are looked at again first (bounded, with
            // backoff), so a task they alone held continues in this same pass.
            let exits_pending = runtime
                .resettle_unconfirmed_exits()
                .await
                .map_err(|e| crate::failure(e.code()))?;
            let automatic_active = runtime
                .continue_automatic_tasks()
                .await
                .map_err(|e| crate::failure(e.code()))?;
            let product = self
                .product_call("product.board", project, json!({}))
                .await?;
            let sprints = product["sprints"].as_array().into_iter().flatten();
            let ids: BTreeSet<_> = sprints.clone().filter_map(|s| s["id"].as_str()).collect();
            let failures = jobs.1.entry(project.to_owned()).or_default();
            failures.retain(|id, _| ids.contains(id.as_str()));
            for sprint in sprints {
                // One sprint's planning failure never stops the others.
                let result = self
                    .planning_pass(project, runtime, sprint, &product, first)
                    .await;
                self.observe_planning_pass(project, sprint, failures, result)
                    .await;
            }
            // Settle waiting results in queue-driven modes, even when the queue
            // is paused (for example just after a switch from Manual). Manual
            // sprints cost nothing here: no task read, no settle call, no folder scan.
            let automatic: Vec<&Value> = product["sprints"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|s| {
                    s["id"].is_string()
                        && s["runMode"] == "hybrid"
                        && crate::machine::queue_driven_mode(s)
                })
                .map(|s| &s["id"])
                .collect();
            let waiting: Vec<&String> = product["tasks"]
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(_, record)| {
                    automatic.contains(&&record["sprintId"])
                        && record["acceptance"].is_null()
                        && !automatic_skip(record, None)
                        && record["status"] != "done"
                })
                .map(|(task, _)| task)
                .collect();
            let waiting_any = !waiting.is_empty();
            if !waiting.is_empty() {
                let finished = self.native_tasks(project).await?;
                for task in waiting {
                    if finished.get(task).is_some_and(|t| t["phase"] == "result") {
                        let record = &product["tasks"][task.as_str()];
                        // A settle that could not finish is held, with its reason, by the
                        // queue's own pass. A paused queue without a hold is paused by the
                        // owner or a mode switch: its reason is never overwritten, so it is
                        // never mistaken for a system hold the scheduler may lift.
                        let _ = self.settle(project, task, record, false).await;
                    }
                }
            }
            for sprint in product["sprints"].as_array().into_iter().flatten() {
                if sprint["archivedAt"].is_null()
                    && (sprint["queueState"] == "running" || resumable_automatic_queue(sprint))
                {
                    // One sprint's unreadable state never stops the others.
                    let _ = self.queue_pass(project, runtime, sprint).await;
                }
            }
            // In motion: a queue running, planning under way, a result waiting for an automatic settle,
            // or a session exit still unconfirmed (looked at again on the safety tick). Anything else changes only through an owner or loop write, which wakes the loop itself.
            let sprints = product["sprints"].as_array().into_iter().flatten();
            let live = sprints.clone().any(|s| {
                s["archivedAt"].is_null()
                    && (s["queueState"] == "running"
                        || s["planning"] == "planning"
                        || resumable_automatic_queue(s))
            }) || waiting_any
                || automatic_active
                || exits_pending;
            Ok(live)
        }
        async fn observe_planning_pass(
            &self,
            project: &str,
            sprint: &Value,
            failures: &mut HashMap<String, u8>,
            result: io::Result<()>,
        ) {
            const WARNING_THRESHOLD: u8 = 3;
            let Some(id) = sprint["id"].as_str() else {
                return;
            };
            if result.is_ok() {
                failures.remove(id);
                if !sprint["planningWarning"].is_null() {
                    self.update_planning_warning(project, id, None).await;
                }
            } else {
                let count = failures.entry(id.to_owned()).or_default();
                if *count < WARNING_THRESHOLD {
                    *count += 1;
                    if *count == WARNING_THRESHOLD
                        && sprint["planningWarning"].is_null()
                        && !self.update_planning_warning(project, id, Some(*count)).await
                    {
                        // Not saved: stay just below the threshold so the next failed
                        // pass of this streak tries again. The writer adds its event only
                        // while no warning is saved, so the streak still gets one event.
                        *count = WARNING_THRESHOLD - 1;
                    }
                }
            }
        }

        /// Planning of one sprint: an ordinary read-only runtime task in the project
        /// folder, created and started by the service, observed here. Its validated
        /// plan becomes the sprint's proposal once the task completed.
        async fn planning_pass(
            &self,
            project: &str,
            runtime: &Arc<Runtime>,
            sprint: &Value,
            product: &Value,
            first: bool,
        ) -> io::Result<()> {
            let Some(id) = sprint["id"].as_str() else {
                return Ok(());
            };
            let closed = !sprint["archivedAt"].is_null()
                || sprint
                    .get("deleting")
                    .is_some_and(|d| !d.is_null() && *d != false);
            let task = sprint["planningTaskId"].as_str();
            let planning = sprint["planning"].as_str().unwrap_or("");
            if let Some(task) = task {
                let record = &product["tasks"][task];
                let fact = planning_fact(runtime, task).await?;
                let finished = matches!(
                    fact.state.as_deref(),
                    Some("COMPLETED" | "FAILED" | "CANCELLED") | None
                );
                if closed || planning == "cancelled" {
                    // Close or delete stops the planning task through its execution control,
                    // once per execution (a repeat replays the same stop). Stop in the sprint
                    // already paused it through the same control.
                    if closed && !finished {
                        if let Some(execution) = fact.execution.as_deref() {
                            runtime
                                .control(
                                    project.to_owned(),
                                    json!({"projectId":project,"executionId":execution,
                                        "requestId":format!("sprint-planning-stop-{execution}"),"action":"pause"}),
                                )
                                .await?;
                        }
                    }
                    return Ok(());
                }
                if planning == "created" {
                    // Approved: the planning task closes as done.
                    if finished && record["status"] != "done" && record["purged"] != true {
                        self.product_call(
                            "product.task",
                            project,
                            json!({"action":"close","taskId":task,"choice":"as_is","confirmed":true}),
                        )
                        .await?;
                    }
                    return Ok(());
                }
                if !matches!(planning, "planning" | "question" | "ready") {
                    return Ok(());
                }
                let retry = planning == "planning" && sprint["planningRetry"] == true;
                if fact.execution.is_none() {
                    if retry {
                        if self
                            .set_planning_task(project, id, Some(task.to_owned()), None, None)
                            .await?
                        {
                            let mut unlinked = sprint.clone();
                            unlinked["planningTaskId"] = Value::Null;
                            unlinked["planningRun"] = Value::Null;
                            return self.start_planning(project, runtime, &unlinked, None).await;
                        }
                        return Ok(());
                    }
                    // A prior create/link may have survived a restart or an unresolved
                    // Start reply. Without an owner retry, report it, never start again.
                    return self
                        .cancel_unstarted_planning(
                            project,
                            id,
                            task,
                            "planning_start_failed",
                            "The planning task did not start.",
                        )
                        .await;
                }
                if retry
                    && !self
                        .consume_planning_retry(project, id, task.to_owned())
                        .await?
                {
                    return Ok(());
                }
                return match planning_step(sprint, &fact) {
                    PlanningStep::Wait => Ok(()),
                    PlanningStep::Ready { items, run } => {
                        let version = sprint["plan"]["version"].as_i64().unwrap_or(0) + 1;
                        let plan = json!({"version":version,"items":items});
                        let mut history = sprint["history"].as_array().cloned().unwrap_or_default();
                        history.push(plan.clone());
                        self.sprint_patch(
                            project,
                            id,
                            json!({"planning":"ready","plan":plan,
                            "history":history,"planningRun":run}),
                        )
                        .await
                    }
                    PlanningStep::Keep { run } => {
                        self.sprint_patch(
                            project,
                            id,
                            json!({"planning":"ready","planningRun":run}),
                        )
                        .await
                    }
                    PlanningStep::Correct { text, run } | PlanningStep::Continue { text, run } => {
                        let execution = fact.execution.clone().unwrap_or_default();
                        let request = format!(
                            "sprint-plan-{}-{}",
                            run["runtimeTask"].as_str().unwrap_or(""),
                            run["handled"]
                        );
                        let sent = runtime
                            .control(project.to_owned(), json!({"projectId":project,"executionId":execution,
                                "action":"prompt","text":text,"attachments":[],"requestId":request}))
                            .await;
                        match sent {
                            Ok(reply) if reply["outcome"] == "recorded" => {
                                self.sprint_patch(project, id, json!({"planning":"planning","planningRun":run}))
                                    .await
                            }
                            _ => {
                                self.cancel_planning(
                                    project,
                                    id,
                                    "continue",
                                    "planning_continue_failed",
                                    "The planning task could not be asked to continue.",
                                    Some(json!({"runtimeTask":run["runtimeTask"],"handled":run["handled"],
                                        "outcome":"cancelled","corrections":run["corrections"],"planEvent":run["planEvent"]})),
                                )
                                .await
                            }
                        }
                    }
                    PlanningStep::Cancel { note, code, run } => {
                        self.cancel_planning(project, id, "run", code, &note, Some(run))
                            .await
                    }
                    PlanningStep::Terminal { failure, run } => {
                        self.record_planning_failure(
                            project,
                            id,
                            failure.reason().into(),
                            "run",
                            failure.code(),
                            Some(run),
                            None,
                            Some((task.to_owned(), failure)),
                        )
                        .await
                    }
                    PlanningStep::Replace { retry } => {
                        // Linking the new planner also retires the previous one.
                        self.sprint_note(
                            project, id, "event", "Planning continues in a new planning task.",
                        )
                        .await?;
                        self.start_planning(project, runtime, sprint, retry).await
                    }
                };
            }
            if closed || planning != "planning" {
                // A sprint saved by an older version without a planning task waits for
                // the owner's answer or Resume planning; nothing runs for it meanwhile.
                return Ok(());
            }
            if first {
                // An attempt claimed before a restart is never silently rerun.
                return self
                    .cancel_planning(
                        project,
                        id,
                        "restart",
                        "planning_interrupted",
                        "Planning was interrupted.",
                        None,
                    )
                    .await;
            }
            self.start_planning(project, runtime, sprint, None).await
        }
        async fn cancel_planning(
            &self,
            project: &str,
            id: &str,
            step: &'static str,
            code: &'static str,
            note: &str,
            run: Option<Value>,
        ) -> io::Result<()> {
            let text = format!("{note} Resume planning to try again.");
            self.record_planning_failure(project, id, text, step, code, run, None, None)
                .await
        }

        async fn cancel_unstarted_planning(
            &self,
            project: &str,
            id: &str,
            task: &str,
            code: &'static str,
            reason: &str,
        ) -> io::Result<()> {
            let text = format!("{reason} Resume planning to try again.");
            self.record_planning_failure(
                project, id, text, "start", code, None, Some(task.into()), None,
            )
            .await
        }

        /// Create the read-only planning draft in the project folder, record it on the
        /// sprint, then start it bound to the sprint planning policy.
        async fn start_planning(
            &self,
            project: &str,
            runtime: &Arc<Runtime>,
            sprint: &Value,
            retry: Option<PlanningFailure>,
        ) -> io::Result<()> {
            let id = sprint["id"].as_str().unwrap_or("");
            // Saved files are references; their bodies are read for this draft only.
            let state = self.config.state_dir.clone();
            let inlined = {
                let sprint = sprint.clone();
                tokio::task::spawn_blocking(move || {
                    let mut sprint = sprint;
                    crate::machine::attachment_files::inline_for_delivery(&state, &mut sprint);
                    sprint
                })
                .await
                .map_err(|_| crate::failure("planning_unavailable"))?
            };
            let description = match planning_request(&inlined, retry) {
                Ok(text) => text,
                Err(note) => {
                    return self
                        .cancel_planning(
                            project,
                            id,
                            "request",
                            "planning_request_invalid",
                            &note,
                            Some(Value::Null),
                        )
                        .await
                }
            };
            let mut payload = json!({"description":description,"checkout":{"kind":"main"}});
            let files = planning_files(&inlined);
            if !files.is_empty() {
                payload["attachments"] = json!(files);
            }
            let settings = &sprint["settings"];
            if let Some(model) = settings["model"].as_str().filter(|m| !m.is_empty()) {
                payload["settings"] = json!({"model":model,
                    "effort":settings["effort"].as_str().unwrap_or(""),
                    "mode":settings["mode"].as_str().filter(|m| cortex_core::access::MODES.contains(m)).unwrap_or("guarded"),
                    "language":settings["language"].as_str().filter(|l| matches!(*l, "auto" | "en" | "ru")).unwrap_or("auto")});
            }
            let request = format!("sprint-planning-{}", crate::identity()?);
            let created = runtime
                .create_draft(
                    project.to_owned(),
                    json!({"protocol":{"major":1,"minor":8},"requestId":request,"projectId":project,
                        "actorId":actor(),"operation":"tasks.create","target":project,
                        "expectedVersion":"0","ownerEpoch":null,"payload":payload}),
                    Some(id.to_owned()),
                )
                .await;
            let task = match created {
                Ok(receipt) if receipt["outcome"] == "recorded" => (
                    receipt["target"].as_str().map(str::to_owned),
                    receipt["entityVersion"].clone(),
                ),
                _ => (None, Value::Null),
            };
            let (Some(task), version) = task else {
                return self
                    .cancel_planning(
                        project,
                        id,
                        "create",
                        "planning_task_create_failed",
                        "The planning task could not be created. Check the selected model and attached files.",
                        Some(Value::Null),
                    )
                    .await;
            };
            if !self
                .set_planning_task(
                    project,
                    id,
                    sprint["planningTaskId"].as_str().map(str::to_owned),
                    Some(task.clone()),
                    retry,
                )
                .await?
            {
                return Ok(());
            }
            let started = runtime
                .start_sprint_planning(
                    project.to_owned(),
                    json!({"protocol":{"major":1,"minor":8},"requestId":format!("{request}-start"),
                        "projectId":project,"actorId":actor(),"operation":"tasks.start","target":task,
                        "expectedVersion":if version.is_null() { json!("1") } else { version },
                        "ownerEpoch":null,"payload":{}}),
                )
                .await;
            self.finish_planning_start(project, runtime, id, &task, started)
                .await
        }

        async fn finish_planning_start(
            &self,
            project: &str,
            runtime: &Runtime,
            id: &str,
            task: &str,
            started: io::Result<Value>,
        ) -> io::Result<()> {
            let (code, reason) = match started {
                Ok(receipt) if receipt["outcome"] == "recorded" => return Ok(()),
                Ok(receipt) => (
                    "planning_start_refused",
                    planning_start_reason(receipt["outcome"].as_str().unwrap_or("refused")),
                ),
                Err(error) => (
                    "planning_start_failed",
                    planning_start_reason(&error.to_string()),
                ),
            };
            // A lost reply is not proof that Start did not commit. An unreadable
            // runtime is not proof either; leave the link for the next pass to check.
            if planning_fact(runtime, task).await?.execution.is_some() {
                return Ok(());
            }
            self.cancel_unstarted_planning(project, id, task, code, &reason)
                .await
        }
        // A concurrent writer (a running task) can advance the outbox between pages or
        // faster than the per-read retry, which stales the snapshot-bound cursor; the whole
        // scan then restarts from the first page, a bounded number of times.
        async fn native_tasks(&self, project: &str) -> io::Result<HashMap<String, Value>> {
            let mut attempt = 0;
            loop {
                match self.native_tasks_scan(project).await {
                    Err(error) if error.to_string() == "snapshot_expired" && attempt < 20 => {
                        attempt += 1;
                        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    }
                    result => return result,
                }
            }
        }
        async fn native_tasks_scan(&self, project: &str) -> io::Result<HashMap<String, Value>> {
            let mut tasks = HashMap::new();
            let mut cursor = Value::Null;
            for _ in 0..200 {
                let page = self
                    .native_query(
                        json!({"protocol":{"major":2,"mapping":2},"requestId":"sprint-queue",
                        "projectId":project,"operation":"tasks.page",
                        "payload":{"target":project,"cursor":cursor,"limit":50}}),
                    )
                    .await?;
                let value = &page["data"]["value"];
                for item in value["items"].as_array().into_iter().flatten() {
                    if let Some(id) = item["taskId"].as_str() {
                        tasks.insert(id.to_owned(), item.clone());
                    }
                }
                cursor = value["nextCursor"].clone();
                if cursor.is_null() {
                    return Ok(tasks);
                }
            }
            Err(crate::failure("scheduler_task_read_budget"))
        }
        // Effect outcomes (including uncertain Git effects) retain their existing
        // durable hold handling; the queue-only decisions use a fresh-facts fence.
        async fn automatic_pause(
            &self,
            project: &str,
            sprint: &Value,
            pause: Value,
        ) -> io::Result<()> {
            self.save_pause(project, sprint, pause, None).await
        }

        async fn observed_pause(&self, project: &str, observed: &Snapshot, pause: Value) -> io::Result<()> {
            self.save_pause(project, &observed.sprint, pause, Some(&observed.queue_revision)).await
        }

        async fn save_pause(
            &self,
            project: &str,
            sprint: &Value,
            pause: Value,
            revision: Option<&str>,
        ) -> io::Result<()> {
            // Every hold the scheduler persists is checked against the task's unknown
            // effects here, so no hold offers an action its handler would refuse.
            let pause = match pause["taskId"].as_str() {
                Some(task) if pause.is_object() => {
                    let runtime = self.state.lock().await.native.get(project).cloned();
                    let unsettled = match runtime {
                        Some(runtime) => unsettled_effects(&runtime, project, task).await,
                        None => None,
                    };
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis() as i64);
                    settling_hold(pause, &sprint["automaticQueue"], unsettled, now)
                }
                _ => pause,
            };
            let reply = self
                .automatic_sprint_action(
                    project,
                    "automatic.queue",
                    json!({"expected":sprint,"queueRevision":revision,"pause":pause}),
                )
                .await?;
            if reply["ok"] != true {
                return Err(other(
                    reply["code"]
                        .as_str()
                        .unwrap_or("automatic_queue_unavailable")
                        .into(),
                ));
            }
            Ok(())
        }
        /// True when the Automatic-specific step held or changed the queue.
        /// Otherwise the ordinary sequential scheduler can look for a draft.
        async fn automatic_pass(
            &self,
            project: &str,
            runtime: &Arc<Runtime>,
            observed: &Snapshot,
            disposition: &Disposition,
        ) -> io::Result<bool> {
            let (sprint, tasks, records, members) = (&observed.sprint, &observed.tasks, &observed.records, &observed.roster);
            let settled = |task: &str| {
                automatic_skip(&records[task], disposition.get(task))
                    || disposition
                        .get(task)
                        .is_some_and(|(status, _)| status == "done")
            };
            if sprint["queueState"] == "paused" {
                let prior = &sprint["automaticQueue"];
                let kind = prior["kind"].as_str().unwrap_or_default();
                let code = prior["code"].as_str().unwrap_or_default();
                let task = prior["taskId"].as_str();
                if task.is_some_and(|id| !members.iter().any(|member| member == id) || settled(id))
                {
                    self.observed_pause(project, observed, Value::Null).await?;
                    return Ok(true);
                }
                // A hold saved before decisions were recorded on it is rewritten with
                // its current decision; the specific recheck runs on the next pass.
                let upgrade = || {
                    let reason = match (kind, prior["reason"].as_str()) {
                        ("start_refused", _) => start_reason(code),
                        (_, Some(reason)) if !reason.contains('_') => reason.to_owned(),
                        _ => "Automatic stopped at this task.".to_owned(),
                    };
                    automatic_hold(kind, task, code, &reason, &HoldFacts::default())
                };
                let legacy = !hold_current(prior)
                    && !matches!(
                        (kind, code),
                        (
                            "settlement_blocked",
                            "queue_blocked" | "dependencies_blocked" | "archived_task"
                        ) | ("continuation", _)
                    );
                if legacy {
                    self.observed_pause(project, observed, upgrade()).await?;
                    return Ok(true);
                }
                // A hold waiting on an unknown effect is rechecked first: it stays while
                // the effect is unknown (`automatic_pause` hands it to the owner once the
                // runtime can no longer settle it), and once it is known the queue derives
                // the condition again or the replaced decision comes back as it was.
                if prior["settling"]["hold"].is_object() {
                    let original = prior["settling"]["hold"].clone();
                    let unsettled = match task {
                        Some(id) => unsettled_effects(runtime, project, id).await,
                        None => None,
                    };
                    let next = if unsettled.is_none() && rederived(kind, code) {
                        Value::Null
                    } else {
                        original
                    };
                    self.observed_pause(project, observed, next).await?;
                    return Ok(true);
                }
                match (kind, code, task) {
                    ("start_refused", _, _) => {}
                    ("continuation", _, Some(id)) => {
                        // Rechecked against the task as read now: a continuation that no
                        // longer pauses (or no longer exists) releases its hold.
                        if let Some(t) = tasks.get(id) {
                            self.observed_pause(project, observed,
                                continuation_hold(id, t).unwrap_or(Value::Null),
                            )
                            .await?;
                        }
                    }
                    ("owner_integration", "owner_accepted", Some(id))
                        if records[id]["acceptance"]["finishConsent"].is_object() =>
                    {
                        // A manual Accept may later acquire explicit Automatic-finish
                        // consent. Re-enter the same validated finish path; without
                        // that consent, automatic_finish preserves this owner hold.
                        self.automatic_finish(
                            project,
                            runtime,
                            sprint,
                            id,
                            &tasks[id]["executionId"],
                            true,
                        )
                        .await?;
                    }
                    (
                        "settlement_blocked",
                        "queue_blocked"
                        | "dependencies_blocked"
                        | "archived_task"
                        | "task_missing"
                        | "conflict_resolving"
                        | "task_conflict"
                        | "task_paused"
                        | "task_blocked"
                        | "task_recovery"
                        | "task_failed"
                        | "task_stopped"
                        | "task_state_unknown",
                        _,
                    ) => {
                        // Observe current conditions, not an unconditional clear/re-pause loop.
                        let hold = members
                            .iter()
                            .filter(|id| !settled(id))
                            .find_map(|id| tasks.get(id).and_then(|t| continuation_hold(id, t)));
                        let next = hold.unwrap_or_else(|| {
                            match queue_step_in_order(members, &observed.priority, tasks, records, disposition, false)
                            {
                                QueueStep::Hold(hold) => hold,
                                _ => Value::Null,
                            }
                        });
                        if next["code"] == "task_failed" {
                            // A failed run held for the owner keeps its decision until the
                            // owner allows a repair or the run changes.
                            let same = next["taskId"] == prior["taskId"] && code == "task_failed";
                            if !same || prior["allowedAt"].is_i64() {
                                self.hold_or_repair(project, runtime, observed, next)
                                    .await?;
                            }
                        } else {
                            self.observed_pause(project, observed, next).await?;
                        }
                    }
                    ("settlement_blocked", "automatic_acceptance_blocked", Some(id)) => {
                        match tasks.get(id).and_then(|t| t["phase"].as_str()) {
                            Some("result")
                                if records[id]["conflict"].as_str().is_none_or(|c| {
                                    !matches!(c, "conflict" | "failed" | "resolving")
                                }) =>
                            {
                                let accepted = records[id]["status"] == "ready_to_merge"
                                    && records[id]["acceptance"].is_object();
                                if accepted {
                                    if records[id]["acceptance"]["finishConsent"].is_object() {
                                        self.automatic_finish(
                                            project,
                                            runtime,
                                            sprint,
                                            id,
                                            &tasks[id]["executionId"],
                                            true,
                                        )
                                        .await?;
                                    } else {
                                        self.observed_pause(project, observed, Value::Null).await?;
                                    }
                                } else {
                                    match self.settle(project, id, &records[id], true).await {
                                        Settle::Accepted => {
                                            // This pass changed the acceptance it just checked.
                                            // Resume against the unchanged sprint; the next
                                            // pass must inspect the newly accepted result.
                                            self.automatic_pause(project, sprint, Value::Null).await?
                                        }
                                        Settle::Deferred => return Ok(true),
                                        Settle::Waiting(reason) | Settle::Failed(reason) => {
                                            self.acceptance_blocked(
                                                project, runtime, observed, id, &reason,
                                            )
                                            .await?
                                        }
                                    }
                                }
                            }
                            Some(phase) if !matches!(phase, "result" | "unknown") => {
                                self.observed_pause(project, observed, Value::Null).await?;
                            }
                            _ => {}
                        }
                    }
                    ("settlement_blocked", _, Some(id))
                        if !matches!(code, "outcome_unknown" | "integration_outcome_unknown") =>
                    {
                        let phase = tasks.get(id).and_then(|t| t["phase"].as_str());
                        if phase.is_some_and(|phase| !matches!(phase, "result" | "unknown")) {
                            self.observed_pause(project, observed, Value::Null).await?;
                        } else if phase == Some("result") && code != "integration_failed" {
                            self.automatic_finish(
                                project,
                                runtime,
                                sprint,
                                id,
                                &tasks[id]["executionId"],
                                true,
                            )
                            .await?;
                        } else if phase == Some("result") && prior["allowedAt"].is_i64() {
                            // A failed commit is never retried as is. With the owner's
                            // consent the agent fixes its cause and publishes a fresh result.
                            let reason = prior["reason"].as_str().unwrap_or_default().to_owned();
                            if let Repair::Declined(facts) = self
                                .automatic_repair(
                                    project,
                                    runtime,
                                    sprint,
                                    id,
                                    &tasks[id]["executionId"],
                                    None,
                                    code,
                                    &reason,
                                    false,
                                )
                                .await?
                            {
                                self.observed_pause(project, observed,
                                    automatic_hold(kind, Some(id), code, &reason, &facts),
                                )
                                .await?;
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(true);
            }
            if automatic_checkout_unavailable(sprint) {
                self.observed_pause(project, observed,
                    automatic_hold(
                        "start_refused",
                        None,
                        "native_worktree_unavailable",
                        WORKTREE_UNAVAILABLE,
                        &HoldFacts::default(),
                    ),
                )
                .await?;
                return Ok(true);
            }
            for task in members.iter().filter(|id| !settled(id)) {
                if let Some(hold) = tasks.get(task).and_then(|t| continuation_hold(task, t)) {
                    self.observed_pause(project, observed, hold).await?;
                    return Ok(true);
                }
            }
            // Finish before the next draft can start. After an effect, use a
            // fresh board on the next pass instead of the old status/digest.
            for task in members.iter().filter(|id| !settled(id)) {
                let record = &records[task];
                if tasks.get(task).is_none_or(|t| t["phase"] != "result")
                    || awaiting_owner_merge_checks(record)
                    || matches!(
                        record["conflict"].as_str(),
                        Some("conflict" | "failed" | "resolving")
                    )
                {
                    continue;
                }
                let acceptance =
                    if record["status"] == "ready_to_merge" && record["acceptance"].is_object() {
                        Settle::Accepted
                    } else {
                        self.settle(project, task, record, true).await
                    };
                let blocked = match acceptance {
                    Settle::Accepted => None,
                    Settle::Deferred => return Ok(true),
                    Settle::Waiting(reason) | Settle::Failed(reason) => Some(reason),
                };
                if let Some(reason) = blocked {
                    self.acceptance_blocked(project, runtime, observed, task, &reason)
                        .await?;
                    return Ok(true);
                }
                self.automatic_finish(
                    project,
                    runtime,
                    sprint,
                    task,
                    &tasks[task]["executionId"],
                    false,
                )
                .await?;
                return Ok(true);
            }
            Ok(false)
        }

        async fn automatic_finish(
            &self,
            project: &str,
            runtime: &Arc<Runtime>,
            sprint: &Value,
            task: &str,
            execution: &Value,
            recheck: bool,
        ) -> io::Result<()> {
            let reply = self
                .automatic_sprint_action(
                    project,
                    "automatic.finish",
                    json!({
                        "sprintId":sprint["id"],"taskId":task,
                        "expectedHold":if recheck { sprint.clone() } else { Value::Null }
                    }),
                )
                .await;
            let (kind, code, reason) = match &reply {
                Ok(r) if r["result"]["done"] == true => {
                    return if recheck {
                        self.automatic_pause(project, sprint, Value::Null).await
                    } else {
                        Ok(())
                    };
                }
                Ok(r) if r["result"]["deferred"] == true => return Ok(()),
                Ok(r) if r["result"]["repairPaths"].is_array() => (
                    "settlement_blocked",
                    "task_files_changed",
                    "The recorded task files have changed. Review their current contents and publish a fresh result.",
                ),
                Ok(r) if r["result"]["ownerIntegration"] == true => (
                    "owner_integration",
                    r["result"]["code"].as_str().unwrap_or("different_target"),
                    r["result"]["reason"]
                        .as_str()
                        .unwrap_or("Confirm how to integrate into the target branch."),
                ),
                Ok(r)
                    if matches!(
                        r["code"].as_str(),
                        Some("machine_busy" | "integration_in_progress")
                    ) =>
                {
                    return Ok(())
                }
                Ok(r) => (
                    "settlement_blocked",
                    r["code"].as_str().unwrap_or("integration_unavailable"),
                    r["message"]
                        .as_str()
                        .unwrap_or("The automatic finish could not be confirmed."),
                ),
                Err(_) => (
                    "settlement_blocked",
                    "outcome_unknown",
                    "The automatic finish could not be confirmed. It was not retried.",
                ),
            };
            let mut facts = HoldFacts::default();
            // The machine's commit refusal asks for a Commit retry no hold offers.
            let reason = if code == "integration_failed" {
                INTEGRATION_FAILED
            } else {
                reason
            };
            // A changed or outdated result is agent work. Content Cortex cannot attribute
            // to the task, a failed commit, and an acceptance the finish cannot use go
            // back to the agent only with consent.
            if matches!(
                code,
                "stale_result"
                    | "workspace_drift"
                    | "task_files_changed"
                    | "review_required"
                    | "task_content_unknown"
                    | "integration_failed"
                    | "automatic_acceptance_required"
            ) {
                let automatic = first_actor(kind, code) == BY_AGENT;
                match self
                    .automatic_repair(
                        project,
                        runtime,
                        sprint,
                        task,
                        execution,
                        reply
                            .as_ref()
                            .ok()
                            .and_then(|r| r["result"].get("repairPaths")),
                        code,
                        reason,
                        automatic,
                    )
                    .await?
                {
                    Repair::Started | Repair::Later => return Ok(()),
                    Repair::Declined(declined) => facts = declined,
                }
            }
            self.automatic_pause(
                project,
                sprint,
                automatic_hold(kind, Some(task), code, reason, &facts),
            )
            .await
        }

        /// A result Automatic could not accept: the agent repairs it when it can,
        /// otherwise the owner is asked exactly what to allow.
        async fn acceptance_blocked(
            &self,
            project: &str,
            runtime: &Arc<Runtime>,
            observed: &Snapshot,
            task: &str,
            reason: &str,
        ) -> io::Result<()> {
            let (sprint, tasks) = (&observed.sprint, &observed.tasks);
            let code = "automatic_acceptance_blocked";
            match self
                .automatic_repair(
                    project,
                    runtime,
                    sprint,
                    task,
                    &tasks[task]["executionId"],
                    None,
                    code,
                    reason,
                    true,
                )
                .await?
            {
                Repair::Started | Repair::Later => Ok(()),
                Repair::Declined(facts) => {
                    self.observed_pause(project, observed,
                        automatic_hold("settlement_blocked", Some(task), code, reason, &facts),
                    )
                    .await
                }
            }
        }

        /// Scheduler tick for one running sprint. Starts are idempotent by
        /// request identity and never replayed; anything unclear pauses the queue.
        async fn queue_pass(
            &self,
            project: &str,
            runtime: &Arc<Runtime>,
            sprint: &Value,
        ) -> io::Result<()> {
            let Some(id) = sprint["id"].as_str() else { return Ok(()); };
            let observed = runtime.sprint_queue_snapshot(project.to_owned(), id.to_owned()).await?;
            let sprint = &observed.sprint;
            if !sprint["archivedAt"].is_null()
                || (!sprint["deleting"].is_null() && sprint["deleting"] != false)
                || (sprint["queueState"] != "running" && !resumable_automatic_queue(sprint))
            {
                return Ok(());
            }
            let disposition = queue_disposition(&observed);
            match sprint["runMode"].as_str() {
                Some("automatic") => {
                    if self.automatic_pass(project, runtime, &observed, &disposition).await? {
                        return Ok(());
                    }
                    match queue_step_in_order(&observed.roster, &observed.priority, &observed.tasks,
                        &observed.records, &disposition, false)
                    {
                        QueueStep::Hold(hold) => self.hold_or_repair(project, runtime, &observed, hold).await,
                        QueueStep::Complete => self.complete_queue(project, &observed).await,
                        QueueStep::Wait => Ok(()),
                        QueueStep::Start(task) =>
                            self.start_queue_task(project, runtime, &observed, task, true).await,
                    }
                }
                Some("hybrid") => self.hybrid_pass(project, runtime, &observed, disposition).await,
                _ => self.queue_transition(project, &observed, true).await,
            }
        }

        async fn complete_queue(&self, project: &str, observed: &Snapshot) -> io::Result<()> {
            self.queue_transition(project, observed, false).await
        }

        async fn queue_transition(&self, project: &str, observed: &Snapshot, pause_manual: bool) -> io::Result<()> {
            let reply = self.automatic_sprint_action(project, "automatic.queue", json!({
                "expected":observed.sprint,"queueRevision":observed.queue_revision,
                "complete":!pause_manual,"pauseManual":pause_manual,
            })).await?;
            if reply["ok"] != true {
                return Err(other(reply["code"].as_str().unwrap_or("automatic_queue_unavailable").to_owned()));
            }
            Ok(())
        }

        /// Hybrid uses the same decision as Automatic, but an accepted result
        /// settles its dependents and merging stays with the owner.
        async fn hybrid_pass(
            &self,
            project: &str,
            runtime: &Arc<Runtime>,
            observed: &Snapshot,
            disposition: Disposition,
        ) -> io::Result<()> {
            let (sprint, tasks, records, members) = (&observed.sprint, &observed.tasks, &observed.records, &observed.roster);
            let paused = sprint["queueState"] == "paused";
            if paused && !system_hold(sprint) {
                return Ok(());
            }
            let prior = &sprint["automaticQueue"];
            // A refused start waits for the owner's Try again (or a mode change), like
            // Automatic; it is released by itself only once its task no longer waits.
            if paused && prior["kind"] == "start_refused" {
                let gone = prior["taskId"].as_str().is_some_and(|task| {
                    !members.iter().any(|m| m == task)
                        || automatic_skip(&records[task], disposition.get(task))
                        || disposition
                            .get(task)
                            .is_some_and(|(s, _)| s == "done" || s == "ready_to_merge")
                        || tasks.get(task).is_some_and(|t| t["phase"] != "not_started")
                });
                return if gone {
                    self.observed_pause(project, observed, Value::Null).await
                } else {
                    Ok(())
                };
            }
            for task in members {
                let settled = disposition
                    .get(task)
                    .is_some_and(|(s, _)| s == "done" || s == "ready_to_merge");
                if settled
                    || automatic_skip(&records[task], disposition.get(task))
                    || tasks.get(task).is_none_or(|t| t["phase"] != "result")
                    || awaiting_owner_merge_checks(&records[task])
                    || records[task]["conflict"]
                        .as_str()
                        .is_some_and(|c| c != "none")
                {
                    continue;
                }
                match self.settle(project, task, &records[task], true).await {
                    Settle::Accepted => {
                        // Acceptance is a write; use fresh Core and overlay facts next pass.
                        return Ok(());
                    }
                    Settle::Deferred => return Ok(()),
                    Settle::Waiting(reason) | Settle::Failed(reason) => {
                        let code = "automatic_acceptance_blocked";
                        match self
                            .automatic_repair(
                                project,
                                runtime,
                                sprint,
                                task,
                                &tasks[task.as_str()]["executionId"],
                                None,
                                code,
                                &reason,
                                true,
                            )
                            .await?
                        {
                            Repair::Started | Repair::Later => return Ok(()),
                            Repair::Declined(mut facts) => {
                                facts.accept_only = true;
                                return self
                                    .observed_pause(project, observed,
                                        automatic_hold(
                                            "settlement_blocked",
                                            Some(task),
                                            code,
                                            &reason,
                                            &facts,
                                        ),
                                    )
                                    .await;
                            }
                        }
                    }
                }
            }
            match queue_step_in_order(members, &observed.priority, tasks, records, &disposition, true) {
                QueueStep::Hold(hold) => {
                    let same = hold["code"] == prior["code"] && hold["taskId"] == prior["taskId"];
                    if hold["code"] == "task_failed" && same && !prior["allowedAt"].is_i64() {
                        return Ok(());
                    }
                    self.hold_or_repair(project, runtime, observed, hold)
                        .await
                }
                QueueStep::Complete => {
                    self.complete_queue(project, observed)
                    .await
                }
                // A cleared system hold resumes the queue by itself.
                QueueStep::Wait | QueueStep::Start(_) if paused => {
                    self.observed_pause(project, observed, Value::Null).await
                }
                QueueStep::Wait => Ok(()),
                QueueStep::Start(task) => {
                    self.start_queue_task(project, runtime, observed, task, false)
                        .await
                }
            }
        }

        async fn start_queue_task(
            &self,
            project: &str,
            runtime: &Arc<Runtime>,
            observed: &Snapshot,
            task: &str,
            automatic: bool,
        ) -> io::Result<()> {
            let version = &observed.tasks[task]["version"];
            let request = json!({"protocol":{"major":1,"minor":8},
                "requestId":format!("sprint-start-{task}-{}",version.as_str().unwrap_or("0")),
                "projectId":project,"actorId":actor(),"operation":"tasks.start","target":task,
                "expectedVersion":version,"ownerEpoch":null,"payload":{}});
            let start = runtime.start_queued(project.to_owned(), request, automatic, observed.expectation()).await;
            let code = match &start {
                Ok(receipt) if receipt["outcome"] == "recorded" => return Ok(()),
                Ok(receipt) => receipt["outcome"].as_str().unwrap_or("refused").to_owned(),
                Err(error) => error.to_string(),
            };
            if code == sprint_live::STALE || code == "stale_version" {
                self.sprint_wake.project(project);
                return Ok(());
            }
            let reason = start_reason(&code);
            self.observed_pause(project, observed, automatic_hold(
                "start_refused", Some(task), &code, &reason, &HoldFacts::default(),
            )).await
        }
    }
}
