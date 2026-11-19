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
    Ok(item)
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
            if state != TaskState::Executing || revision != expected_revision
                || !lifecycle::steering::pending(db, &t)?.is_empty() {
                return Ok(Vec::new());
            }
            let failed: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND revision=? AND status='failed')",
                (&t, revision), |r| r.get(0),
            )?;
            if failed { return Ok(Vec::new()); }
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
        let _driver = self.driver.lock().await;
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
                    let (e, baseline_manifest, raw_manifest) =
                        native_effects::observe_review_manifest(self.root.clone()).await?;
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
                                let (failed,unfinished):(bool,bool)=db.query_row(
                                    "SELECT EXISTS(SELECT 1 FROM runtime_nodes n JOIN runtime_tasks t USING(task_id) WHERE n.task_id=? AND n.revision=t.revision AND n.status='failed'),
                                     EXISTS(SELECT 1 FROM runtime_nodes n JOIN runtime_tasks t USING(task_id) WHERE n.task_id=? AND n.revision=t.revision AND n.status!='completed')",
                                     (&t,&t),|r|Ok((r.get(0)?,r.get(1)?)))?;
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
                                let review_status:String=db.query_row("SELECT coalesce((SELECT n.status FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
                                    WHERE n.task_id=? AND n.revision=t.revision AND n.stage='review' ORDER BY n.rowid DESC LIMIT 1),'missing')",[&t],|r|r.get(0))?;
                                if !active{
                                    if review_status=="completed"{review::advance_tx(db,&t)?;}
                                    else if review_status=="failed"{domain::stop_tx(db,&t,"review_blocked")?;}
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
                            if stage=="review"{
                                let bound:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED' AND json_extract(payload_json,'$.assignment_ref')=?)",
                                    (&t,&a),|r|r.get(0))?;
                                if !bound{completion::start_review_tx(db,&t,&a,&e)?;}
                            }
                            let mut item=assignment_brief(db,&t,&a,&node,&role,&objective,&scope,&stage)?;
                            // A reviewer is told exactly what the work changed and how much
                            // predecessor evidence there is to read.
                            if stage=="review"{
                                let packet=lifecycle::reconciliation::review_packet_tx(db,&t,&node,baseline_manifest.as_ref(),e.ignored_digest.as_deref(),&raw_manifest)?;
                                item["review_packet"]=packet.clone();
                                // Persist the exact authoritative packet used for this
                                // reviewer.  Result submission must validate coverage
                                // against this packet, not against a model's recollection.
                                let recorded:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='REVIEW_PACKET' AND json_extract(payload_json,'$.assignment_ref')=?)",(&t,&a),|r|r.get(0))?;
                                if !recorded { lifecycle::event_tx(db,&t,"REVIEW_PACKET",serde_json::json!({"assignment_ref":a,"node":node,"packet":packet}))?; }
                            }
                            assignments.push(item);
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
                    self.verify_plan(&task, &owner).await?;
                    if recheck_after_verification {
                        return Ok(json!({"recheck":true}));
                    }
                    continue;
                }
                TaskState::AnswerReady | TaskState::Reconciling => {
                    let (e, manifest) = native_effects::observe_manifest(self.root.clone()).await?;
                    let ignored = e.ignored_digest.clone();
                    let t = task.clone();
                    let o = owner.clone();
                    let completed=recv(self.executor.publication(move|store|{
                        let publisher=Publisher::new(store);
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
    async fn verify_plan(&self, task: &str, owner: &OwnerEpoch) -> CoreResult<()> {
        // One observation anchors the review preflight, the recorded intent
        // digest, every post-command drift check and the final record.
        let e = std::sync::Arc::new(native_effects::observe(self.root.clone()).await?);
        let digest = e.project_digest.clone();
        let t = task.to_owned();
        let o = owner.clone();
        let intent_digest = digest.clone();
        let preflight = std::sync::Arc::clone(&e);
        let run = format!("verify-{}", native_effects::identity()?);
        let intent=recv(self.executor.write(move|db|{
            ownership::check_tx(db,&t,&o)?;
            let (_,revision)=domain::task_tx(db,&t)?;
            let unfinished:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND operation='native_verification_intent'
                AND json_extract(result_json,'$.revision')=? AND NOT EXISTS(SELECT 1 FROM runtime_requests r WHERE r.task_id=? AND r.actor='native-verifier-finish' AND r.request_key=runtime_requests.request_key))",
                (&t,revision,&t),|r|r.get(0))?;
            if unfinished{return Err(CoreError::Refused("native_verification_outcome_unknown"));}
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
            lifecycle::request_tx(db,&t,"native-verifier",&run,"native_verification_intent",
                &json!({"revision":revision,"digest":intent_digest,"commands":commands}),||Ok(json!({"revision":revision,"digest":intent_digest})))?;
            Ok(Some((commands,run)))
        })?).await?;
        let Some((commands, run)) = intent else {
            return Ok(());
        };
        let mut keys = Vec::new();
        let mut all_exits_observed = true;
        for (index, argv) in commands.into_iter().enumerate() {
            let mut receipt = match self.owner_verify(task, owner, &argv).await {
                Ok(r) => r,
                Err(err) => {
                    json!({"tool_call_ref":format!("{run}:{index}"),"argv":argv,"profile":"native-readonly-landlock-seccomp",
                    "observation":{"complete":false,"failure":err.code(),"exit_code":null}})
                }
            };
            receipt["revision_digest"] = json!(digest);
            receipt["owner_epoch"] = json!(owner.epoch);
            receipt["run"] = json!(run);
            let t = task.to_owned();
            let key = format!("{run}:{index}");
            let raw = receipt.clone();
            let run_key = run.clone();
            let output_key = key.clone();
            recv(self.executor.write(move|db|{
                // A command already dispatched under this intent is an observed
                // effect even if later state/epoch/project changes disqualify it.
                let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND actor='native-verifier' AND request_key=? AND operation='native_verification_intent')",(&t,&run_key),|r|r.get(0))?;
                if !exists{return Err(CoreError::Refused("verification_intent_missing"));}
                lifecycle::request_tx(db,&t,"native-verifier-output",&output_key,"native_command_output",&raw,||Ok(raw.clone()))?;
                Ok(())
            })?).await?;
            let fresh = native_effects::observe(self.root.clone()).await?;
            all_exits_observed &= receipt["observation"]["complete"] == true
                && receipt["observation"]["exit_code"].is_i64();
            if fresh.project_digest != digest {
                if !all_exits_observed {
                    return Err(CoreError::Refused("native_verification_outcome_unknown"));
                }
                let (t, o) = (task.to_owned(), owner.clone());
                return recv(self.executor.write(move |db| {
                    ownership::check_tx(db,&t,&o)?;
                    lifecycle::request_tx(db,&t,"native-verifier-finish",&run,"native_verification_finished",
                        &json!({"last_output":key}),||Ok(json!({"settled":true,"reason":"verification_review_revision_changed","last_output":key})))?;
                    completion::review_changed_checkout_tx(db,&t,&fresh)
                })?).await;
            }
            let t = task.to_owned();
            let o = owner.clone();
            let key = format!("{run}:{index}");
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
        .await
    }
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
                },
                relay: "/bin/false".into(),
                broker_parent: root.join(if workspace_protocol::channel::current().is_dev() {
                    ".cortex-dev"
                } else {
                    ".cortex"
                }),
                connection: "fixture".into(),
                connection_limit: 4,
                model: "fixture-model".into(),
                parameters: BTreeMap::new(),
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
        wait("the planning coordinator's turn to settle", || async {
            let runtime = service.state.lock().await.native.get(&id).unwrap().clone();
            let t = planning.clone();
            runtime.host.executor.read(move |db| Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id
                 JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE w.task_id=?
                 AND r.state IN ('WAITING_USER','FAILED','CANCELLED','BLOCKED')
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
                 AND EXISTS(SELECT 1 FROM runtime_tasks WHERE state IN ('WAITING_USER','FAILED','CANCELLED','BLOCKED'))",
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
    use crate::{actor, native_runtime::Runtime, Service};
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
            let mut clean = serde_json::Map::new();
            clean.insert("key".into(), json!(key));
            if let Some(title) = title {
                clean.insert("title".into(), title);
            }
            clean.insert("description".into(), item["description"].clone());
            clean.insert("dependsOn".into(), Value::Array(deps));
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
    pub(crate) fn planning_request(sprint: &Value) -> Result<String, String> {
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
             - Base the plan on what you found and submit it with sprint_plan_submit. The runtime \
             verifies and independently reviews it; after approval finish the task. The owner then \
             approves the plan and creates the tasks.\n\
             - Write in {language}.\n\nSprint description:\n{}",
            sprint["description"].as_str().unwrap_or("")
        );
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
        if text.chars().count() > MAX_REQUEST_CHARS {
            return Err("The planning request is too long. Shorten the description or conversation before retrying.".into());
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

    /// The runtime facts of a planning task's current execution.
    #[derive(Debug, Default, Clone)]
    pub(crate) struct PlanningFact {
        pub(crate) execution: Option<String>,
        pub(crate) runtime_task: Option<String>,
        pub(crate) state: Option<String>,
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
                let row: Option<(Option<String>, Option<String>, Option<String>)> = db
                    .query_row(
                        "SELECT w.execution_id,e.runtime_task_id,r.state FROM workspace_tasks w
                         LEFT JOIN workspace_executions e ON e.execution_id=w.execution_id
                         LEFT JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE w.task_id=?",
                        [&task],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?;
                let Some((execution, runtime_task, state)) = row else {
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
            run: Value,
        },
        Continue {
            text: String,
            run: Value,
        },
        Replace {
            note: String,
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
                        run: run(completion, "cancelled", corrections, consumed),
                    },
                }
            }
            Some("FAILED" | "CANCELLED") => {
                let stopped = json!(format!("stopped:{rt}"));
                if handled != stopped {
                    PlanningStep::Cancel {
                        note: "The planning task stopped before it produced a plan.".into(),
                        run: run(stopped, "cancelled", corrections, consumed),
                    }
                } else if resumed {
                    PlanningStep::Replace {
                        note: "Planning continues in a new planning task.".into(),
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
    struct Jobs(HashMap<(String, String), tokio::task::JoinHandle<()>>);
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
            // An ordinary task in the project folder, asked to study and plan read-only.
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
            let PlanningStep::Cancel { run, .. } = planning_step(&base, &fact("FAILED", 0, None))
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
        async fn automatic_sprint_projects_a_core_pause_and_waits_for_its_release() {
            let (_scratch, folder, service, runtime, project, id, tasks) =
                automatic_fixture("dev", "main", "main").await;
            let mut jobs = Jobs::default();
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            let task = settled_peer(&runtime, &tasks[0]).await;
            let before = service.native_tasks(&project).await.unwrap()[&tasks[0]]
                ["automaticContinuation"]["continuationCount"]
                .as_u64()
                .unwrap();
            let t = task.clone();
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
            let held = sprint(&service, &project, &id).await;
            assert_eq!(held["queueState"], "paused", "{held}");
            assert_eq!(held["automaticQueue"]["kind"], "continuation");
            assert_eq!(held["automaticQueue"]["code"], "continuation_cap");
            assert_eq!(held["automaticQueue"]["needsOwner"], true);
            let projection = service.native_tasks(&project).await.unwrap();
            assert_eq!(
                projection[&tasks[0]]["automaticContinuation"]["continuationCount"],
                before + 1
            );
            assert_eq!(
                projection[&tasks[0]]["automaticContinuation"]["lastContinuationReason"],
                "Retry checks"
            );
            assert!(projection[&tasks[1]]["executionId"].is_null());
            // Exercise the actual owner Resume control, not a fixture-only
            // update that removes the blocker behind the scheduler's back.
            let receipt = runtime.control(project.clone(), json!({
                "projectId":project,"executionId":projection[&tasks[0]]["executionId"],
                "requestId":"resume-cap","action":"resume","expectedVersion":projection[&tasks[0]]["version"]
            })).await.unwrap();
            assert_eq!(receipt["outcome"], "recorded");
            let renewed = runtime.host.executor.read(move |db| {
                Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_CAP_RENEWED'",
                    [&task], |row| row.get::<_, i64>(0))?)
            }).unwrap().await.unwrap().unwrap();
            assert_eq!(renewed, 1);
            service
                .sprint_pass(&project, &runtime, &mut jobs, false)
                .await
                .unwrap();
            assert_eq!(
                sprint(&service, &project, &id).await["queueState"],
                "running"
            );
            assert!(
                service.native_tasks(&project).await.unwrap()[&tasks[1]]["executionId"].is_null()
            );
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
                db.execute("INSERT INTO runtime_requests VALUES (?,'fixture-unknown-key','fixture-unknown','command_run','digest',?)",
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
            assert_eq!(hold["nextActor"], "owner");
            assert_eq!(hold["ownerActions"], json!(["close_as_is"]), "{hold}");
            assert_eq!(hold["asks"], json!([UNKNOWN_EFFECT_ASK]));
            assert_eq!(repairs_of(&runtime, &task).await, 0);
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
        }
    }

    /// The queue's pause reason, naming a settle that could not finish instead of
    /// blaming the checks of a result that never reached them.
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
    pub(super) fn member_disposition(
        record: &Value,
        task: Option<&Value>,
    ) -> (String, Option<String>) {
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
            // A route, login or quota blocker: the runtime resumes once the route qualifies.
            ("continuation", "external_blocker") => (BY_RUNTIME, vec![]),
            ("continuation", "continuation_cap") => (BY_OWNER, close(vec!["allow_retries"])),
            // An effect whose outcome is unknown is never replayed automatically.
            ("continuation", _) => (BY_OWNER, close(vec!["confirm_and_continue"])),
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
                "continuation_cap" => "The agent used all of its automatic attempts for this task.",
                "outcome_unknown" => "An action of this task may or may not have finished. It is not repeated automatically.",
                _ => match held["reason"].as_str() {
                    Some("engine_authentication_required" | "authorization_required") => "The engine needs you to sign in again. Automatic continues by itself once it can connect.",
                    Some("engine_quota_exhausted") => "The engine usage limit is reached. Automatic continues by itself when the engine is available again.",
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
            || disposition.is_some_and(|(_, closed)| closed.as_deref() == Some("cancel"))
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
        assert!(matches!(
            automatic_queue_step(&members, &tasks, &records, &HashMap::new(), false),
            QueueStep::Start("independent")
        ));
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

    fn automatic_queue_step<'a>(
        members: &'a [String],
        tasks: &HashMap<String, Value>,
        records: &Value,
        disposition: &HashMap<String, (String, Option<String>)>,
        hybrid: bool,
    ) -> QueueStep<'a> {
        // In Hybrid an accepted result settles its dependents; merging stays the owner's.
        let done = |id: &str| {
            disposition.get(id).is_some_and(|(status, closed)| {
                (status == "done" || hybrid && status == "ready_to_merge")
                    && closed.as_deref() != Some("cancel")
                    && records[id]["purged"] != true
            })
        };
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
            match records[id]["conflict"].as_str() {
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
        if let Some(id) = pending.iter().copied().find(|id| unmet(id).is_none()) {
            return QueueStep::Start(id);
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
            sprint: &Value,
            tasks: &HashMap<String, Value>,
            hold: Value,
        ) -> io::Result<()> {
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
                                .automatic_pause(
                                    project,
                                    sprint,
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
            self.automatic_pause(project, sprint, hold).await
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
            _jobs: &mut Jobs,
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
            for sprint in product["sprints"].as_array().cloned().unwrap_or_default() {
                // One sprint's planning failure never stops the others.
                let _ = self
                    .planning_pass(project, runtime, &sprint, &product, first)
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
                        && record["purged"] != true
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
                    let _ = self.queue_pass(project, runtime, sprint, &product).await;
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
                                    "The planning task could not be asked to continue.",
                                    json!({"runtimeTask":run["runtimeTask"],"handled":run["handled"],
                                        "outcome":"cancelled","corrections":run["corrections"],"planEvent":run["planEvent"]}),
                                )
                                .await
                            }
                        }
                    }
                    PlanningStep::Cancel { note, run } => {
                        self.cancel_planning(project, id, &note, run).await
                    }
                    PlanningStep::Replace { note } => {
                        if record["status"] != "done" && record["purged"] != true {
                            let _ = self
                                .product_call(
                                    "product.task",
                                    project,
                                    json!({"action":"close","taskId":task,"choice":"cancel","confirmed":true}),
                                )
                                .await;
                        }
                        self.sprint_note(project, id, "event", &note).await?;
                        self.start_planning(project, runtime, sprint).await
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
                self.sprint_note(
                    project,
                    id,
                    "event",
                    "Planning was interrupted. Resume planning to try again.",
                )
                .await?;
                return self
                    .sprint_patch(project, id, json!({"planning":"cancelled"}))
                    .await;
            }
            self.start_planning(project, runtime, sprint).await
        }
        async fn cancel_planning(
            &self,
            project: &str,
            id: &str,
            note: &str,
            run: Value,
        ) -> io::Result<()> {
            let text = format!("{note} Resume planning to try again.");
            self.sprint_note(project, id, "event", &text).await?;
            self.sprint_patch(
                project,
                id,
                json!({"planning":"cancelled","planningRun":run}),
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
            let description = match planning_request(&inlined) {
                Ok(text) => text,
                Err(note) => return self.cancel_planning(project, id, &note, Value::Null).await,
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
                .command(
                    project.to_owned(),
                    json!({"protocol":{"major":1,"minor":8},"requestId":request,"projectId":project,
                        "actorId":actor(),"operation":"tasks.create","target":project,
                        "expectedVersion":"0","ownerEpoch":null,"payload":payload}),
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
                        "The planning task could not be created. Check the selected model and attached files.",
                        Value::Null,
                    )
                    .await;
            };
            self.sprint_patch(
                project,
                id,
                json!({"planningTaskId":task,"planningRun":null,"planning":"planning"}),
            )
            .await?;
            let started = runtime
                .start_sprint_planning(
                    project.to_owned(),
                    json!({"protocol":{"major":1,"minor":8},"requestId":format!("{request}-start"),
                        "projectId":project,"actorId":actor(),"operation":"tasks.start","target":task,
                        "expectedVersion":if version.is_null() { json!("1") } else { version },
                        "ownerEpoch":null,"payload":{}}),
                )
                .await;
            match started {
                Ok(receipt) if receipt["outcome"] == "recorded" => Ok(()),
                Ok(receipt) => {
                    let code = receipt["outcome"].as_str().unwrap_or("refused").to_owned();
                    self.cancel_planning(project, id, &planning_start_reason(&code), Value::Null)
                        .await
                }
                Err(error) => {
                    self.cancel_planning(
                        project,
                        id,
                        &planning_start_reason(&error.to_string()),
                        Value::Null,
                    )
                    .await
                }
            }
        }
        async fn native_tasks(&self, project: &str) -> io::Result<HashMap<String, Value>> {
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
        async fn automatic_pause(
            &self,
            project: &str,
            sprint: &Value,
            pause: Value,
        ) -> io::Result<()> {
            let reply = self
                .automatic_sprint_action(
                    project,
                    "automatic.queue",
                    json!({"expected":sprint,"pause":pause}),
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
            sprint: &Value,
            tasks: &HashMap<String, Value>,
            records: &Value,
            members: &[String],
            disposition: &HashMap<String, (String, Option<String>)>,
        ) -> io::Result<bool> {
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
                    self.automatic_pause(project, sprint, Value::Null).await?;
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
                    self.automatic_pause(project, sprint, upgrade()).await?;
                    return Ok(true);
                }
                match (kind, code, task) {
                    ("start_refused", _, _) => {}
                    ("continuation", _, Some(id)) => {
                        // Rechecked against the task as read now: a continuation that no
                        // longer pauses (or no longer exists) releases its hold.
                        if let Some(t) = tasks.get(id) {
                            self.automatic_pause(
                                project,
                                sprint,
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
                            match automatic_queue_step(members, tasks, records, disposition, false)
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
                                self.hold_or_repair(project, runtime, sprint, tasks, next)
                                    .await?;
                            }
                        } else {
                            self.automatic_pause(project, sprint, next).await?;
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
                                        self.automatic_pause(project, sprint, Value::Null).await?;
                                    }
                                } else {
                                    match self.settle(project, id, &records[id], true).await {
                                        Settle::Accepted => {
                                            self.automatic_pause(project, sprint, Value::Null)
                                                .await?
                                        }
                                        Settle::Deferred => return Ok(true),
                                        Settle::Waiting(reason) | Settle::Failed(reason) => {
                                            self.acceptance_blocked(
                                                project, runtime, sprint, tasks, id, &reason,
                                            )
                                            .await?
                                        }
                                    }
                                }
                            }
                            Some(phase) if !matches!(phase, "result" | "unknown") => {
                                self.automatic_pause(project, sprint, Value::Null).await?;
                            }
                            _ => {}
                        }
                    }
                    ("settlement_blocked", _, Some(id))
                        if !matches!(code, "outcome_unknown" | "integration_outcome_unknown") =>
                    {
                        let phase = tasks.get(id).and_then(|t| t["phase"].as_str());
                        if phase.is_some_and(|phase| !matches!(phase, "result" | "unknown")) {
                            self.automatic_pause(project, sprint, Value::Null).await?;
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
                                self.automatic_pause(
                                    project,
                                    sprint,
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
            if sprint["checkout"]["kind"]
                .as_str()
                .is_some_and(|kind| kind != "main")
            {
                self.automatic_pause(
                    project,
                    sprint,
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
                    self.automatic_pause(project, sprint, hold).await?;
                    return Ok(true);
                }
            }
            // Finish before the next draft can start. After an effect, use a
            // fresh board on the next pass instead of the old status/digest.
            for task in members.iter().filter(|id| !settled(id)) {
                let record = &records[task];
                if tasks.get(task).is_none_or(|t| t["phase"] != "result")
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
                    self.acceptance_blocked(project, runtime, sprint, tasks, task, &reason)
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
            sprint: &Value,
            tasks: &HashMap<String, Value>,
            task: &str,
            reason: &str,
        ) -> io::Result<()> {
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
                    self.automatic_pause(
                        project,
                        sprint,
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
            product: &Value,
        ) -> io::Result<()> {
            let id = sprint["id"].as_str().unwrap_or("");
            let tasks = self.native_tasks(project).await?;
            let records = &product["tasks"];
            let members: Vec<String> = sprint["taskIds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t.as_str().map(str::to_owned))
                .collect();
            let disposition: HashMap<String, (String, Option<String>)> = members
                .iter()
                .map(|t| (t.clone(), member_disposition(&records[t], tasks.get(t))))
                .collect();
            let mut status: HashMap<String, String> = disposition
                .iter()
                .map(|(t, (s, _))| (t.clone(), s.clone()))
                .collect();
            let closed_as = |t: &str| disposition.get(t).and_then(|d| d.1.clone());
            // Only a real acceptance settles a newly finished result, and only while the
            // sprint uses a queued mode (a Manual queue pauses below).
            let queue_driven = crate::machine::queue_driven_mode(sprint);
            let automatic = sprint["runMode"] == "automatic" && queue_driven;
            if automatic {
                if self
                    .automatic_pass(
                        project,
                        runtime,
                        sprint,
                        &tasks,
                        records,
                        &members,
                        &disposition,
                    )
                    .await?
                {
                    return Ok(());
                }
                return match automatic_queue_step(&members, &tasks, records, &disposition, false) {
                    QueueStep::Hold(hold) => {
                        self.hold_or_repair(project, runtime, sprint, &tasks, hold)
                            .await
                    }
                    QueueStep::Complete => {
                        self.sprint_patch(
                            project,
                            id,
                            json!({"queueState":"complete","pauseReason":null}),
                        )
                        .await
                    }
                    QueueStep::Wait => Ok(()),
                    QueueStep::Start(task) => {
                        self.start_queue_task(project, runtime, sprint, task, &tasks[task], true)
                            .await
                    }
                };
            }
            if queue_driven && sprint["runMode"] == "hybrid" {
                return self
                    .hybrid_pass(
                        project,
                        runtime,
                        sprint,
                        &tasks,
                        records,
                        &members,
                        disposition,
                    )
                    .await;
            }
            let mut failed: Option<String> = None;
            for task in &members {
                let done = matches!(
                    status.get(task).map(String::as_str),
                    Some("ready_to_merge" | "done")
                );
                if done || records[task]["purged"] == true {
                    continue;
                }
                if queue_driven && tasks.get(task).is_some_and(|t| t["phase"] == "result") {
                    match self.settle(project, task, &records[task], false).await {
                        Settle::Accepted => {
                            status.insert(task.clone(), "ready_to_merge".into());
                        }
                        // Machine state is in use right now: settle on the next pass
                        // instead of pausing the queue for an unsettled result.
                        Settle::Deferred => return Ok(()),
                        Settle::Failed(reason) => {
                            failed.get_or_insert(reason);
                        }
                        Settle::Waiting(_) => {}
                    }
                }
            }
            let phase = |task: &str| -> Option<String> {
                let t = tasks.get(task)?;
                let mut phase = t["phase"].as_str().unwrap_or("unknown").to_owned();
                // Owner completion settles dependencies, not verification.
                if matches!(
                    phase.as_str(),
                    "not_started" | "result" | "stopped" | "failed"
                ) && matches!(closed_as(task).as_deref(), Some("external" | "as_is"))
                    && status.get(task).is_some_and(|s| s == "done")
                {
                    phase = "result".into();
                }
                Some(phase)
            };
            let phases: Vec<Option<String>> = members.iter().map(|t| phase(t)).collect();
            let settled = |task: &str| match status.get(task).map(String::as_str) {
                Some("done") => true,
                Some("ready_to_merge") => true,
                _ => false,
            };
            let pause = if !queue_driven {
                Some("This sprint is in manual mode.")
            } else if phases.iter().any(Option::is_none) {
                Some("A sprint task is missing; review the queue.")
            } else if members
                .iter()
                .any(|t| closed_as(t).as_deref() == Some("cancel"))
            {
                Some("A task was cancelled. Review the task before continuing the queue.")
            } else if members.iter().zip(&phases).any(|(task, p)| {
                matches!(
                    p.as_deref(),
                    Some("failed" | "stopped" | "blocked" | "unknown" | "recovery")
                ) && !(p.as_deref() == Some("recovery")
                    && tasks[task]["waitReason"] == "automatic_continuation_pending")
            }) {
                Some("A task needs attention or has an unknown outcome.")
            } else if members
                .iter()
                .zip(&phases)
                .any(|(t, p)| p.as_deref() == Some("result") && !settled(t))
            {
                Some("A result has missing or unsuccessful checks.")
            } else if members.iter().any(|t| {
                matches!(
                    records[t]["conflict"].as_str(),
                    Some("conflict" | "failed" | "resolving")
                )
            }) {
                Some("Resolve the task conflict first.")
            } else {
                None
            };
            // A settle that could not finish says why, instead of blaming the checks.
            let pause = with_settle_failure(pause, failed.as_deref());
            if let Some(reason) = pause {
                return self
                    .sprint_patch(
                        project,
                        id,
                        json!({"queueState":"paused","pauseReason":reason}),
                    )
                    .await;
            }
            if members.iter().all(|t| settled(t)) {
                return self
                    .sprint_patch(
                        project,
                        id,
                        json!({"queueState":"complete","pauseReason":null}),
                    )
                    .await;
            }
            // A running or waiting task holds the queue; it never pauses by itself.
            if phases
                .iter()
                .flatten()
                .any(|p| p != "not_started" && p != "result")
            {
                return Ok(());
            }
            let next = members.iter().zip(&phases).find(|(t, p)| {
                p.as_deref() == Some("not_started")
                    && records[t.as_str()]["dependencies"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .all(|d| d.as_str().is_some_and(|d| settled(d)))
            });
            let Some((task, _)) = next else {
                return self
                    .sprint_patch(project, id, json!({"queueState":"paused","pauseReason":"No task can run; review dependencies."}))
                    .await;
            };
            self.start_queue_task(project, runtime, sprint, task, &tasks[task], false)
                .await
        }

        /// The Hybrid queue under the same single next-actor decision as Automatic.
        /// Results are accepted automatically and repaired by the agent when they can
        /// be; merging stays the owner's. An accepted result settles its dependents.
        /// The owner's own pause ("Paused by you") is never lifted here.
        #[allow(clippy::too_many_arguments)]
        async fn hybrid_pass(
            &self,
            project: &str,
            runtime: &Arc<Runtime>,
            sprint: &Value,
            tasks: &HashMap<String, Value>,
            records: &Value,
            members: &[String],
            mut disposition: HashMap<String, (String, Option<String>)>,
        ) -> io::Result<()> {
            let id = sprint["id"].as_str().unwrap_or("");
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
                    self.automatic_pause(project, sprint, Value::Null).await
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
                    || records[task]["conflict"]
                        .as_str()
                        .is_some_and(|c| c != "none")
                {
                    continue;
                }
                match self.settle(project, task, &records[task], true).await {
                    Settle::Accepted => {
                        disposition.insert(task.clone(), ("ready_to_merge".into(), None));
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
                                    .automatic_pause(
                                        project,
                                        sprint,
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
            match automatic_queue_step(members, tasks, records, &disposition, true) {
                QueueStep::Hold(hold) => {
                    let same = hold["code"] == prior["code"] && hold["taskId"] == prior["taskId"];
                    if hold["code"] == "task_failed" && same && !prior["allowedAt"].is_i64() {
                        return Ok(());
                    }
                    self.hold_or_repair(project, runtime, sprint, tasks, hold)
                        .await
                }
                QueueStep::Complete => {
                    self.sprint_patch(
                        project,
                        id,
                        json!({"queueState":"complete","pauseReason":null}),
                    )
                    .await
                }
                // A cleared system hold resumes the queue by itself.
                QueueStep::Wait | QueueStep::Start(_) if paused => {
                    self.automatic_pause(project, sprint, Value::Null).await
                }
                QueueStep::Wait => Ok(()),
                QueueStep::Start(task) => {
                    self.start_queue_task(project, runtime, sprint, task, &tasks[task], false)
                        .await
                }
            }
        }

        async fn start_queue_task(
            &self,
            project: &str,
            runtime: &Arc<Runtime>,
            sprint: &Value,
            task: &str,
            fact: &Value,
            automatic: bool,
        ) -> io::Result<()> {
            let id = sprint["id"].as_str().unwrap_or("");
            let version = fact["version"].clone();
            let request = json!({"protocol":{"major":1,"minor":8},
                "requestId":format!("sprint-start-{task}-{}",version.as_str().unwrap_or("0")),
                "projectId":project,"actorId":actor(),"operation":"tasks.start","target":task,
                "expectedVersion":version,"ownerEpoch":null,"payload":{}});
            let start = if automatic {
                runtime.start_automatic(project.to_owned(), request).await
            } else {
                runtime.command(project.to_owned(), request).await
            };
            let refused = match start {
                Ok(receipt) if receipt["outcome"] == "recorded" => None,
                Ok(receipt) => Some(receipt["outcome"].as_str().unwrap_or("refused").to_owned()),
                Err(error) => Some(error.to_string()),
            };
            if let Some(code) = refused {
                if automatic || sprint["runMode"] == "hybrid" {
                    let reason = start_reason(&code);
                    return self
                        .automatic_pause(
                            project,
                            sprint,
                            automatic_hold(
                                "start_refused",
                                Some(task),
                                &code,
                                &reason,
                                &HoldFacts::default(),
                            ),
                        )
                        .await;
                }
                self.sprint_patch(
                    project,
                    id,
                    json!({"queueState":"paused","pauseReason":format!("Start was refused: {code}")}),
                )
                .await?;
            }
            Ok(())
        }
    }
}
