//! Engine turn delivery, retry, and outcome evidence.
use super::*;
use crate::native_sessions::LaunchInterrupts;
use crate::native_route_hold::{Attempt as RouteAttempt, RouteKey, LIMITED};

struct TurnMarker {
    state_and_revision: (String, i64),
    has_synthesis: bool,
}

impl Runtime {
    /// One runtime turn with the degenerate-turn watchdog (see
    /// `native_sessions::degenerate`). A coordinator turn that degenerated without
    /// changing the task's state or plan revision runs once more, with the same
    /// prompt, in a fresh engine session; when that degenerates too, the coordinator
    /// moves to another qualified model (`NATIVE_COORDINATOR_ESCALATED`, preferably
    /// the other engine) and runs the turn there. Only when no other model exists, or
    /// it degenerates as well, does the turn end with `native_turn_degenerate`, which
    /// the no-stop policy turns into a continuation on an escalated rung, never an
    /// owner hold. A worker's degenerate turn ends with its node still open and its
    /// session terminated, so the lost-worker path replaces it in a fresh session on
    /// an escalated route (another model or engine) at the next step, every time.
    pub(super) async fn turn(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        role: SessionRole,
        route: SessionRoute,
        prompt: &str,
        cancelled: watch::Receiver<bool>,
    ) -> CoreResult<()> {
        const DEGENERATE: &str = "native_turn_degenerate";
        let assignment = match &role {
            SessionRole::Worker { assignment } => Some(assignment.clone()),
            SessionRole::Coordinator | SessionRole::ActiveSteering | SessionRole::OwnerDecision { .. } => None,
        };
        let retry = route.clone();
        let before = self.turn_marker(task).await?;
        match self
            .turn_once(task, owner, role, route, prompt, cancelled.clone(), None)
            .await
        {
            Err(error) if error.code() == DEGENERATE => {}
            other => return self.keep_committed_turn(task, &before, other).await,
        }
        if assignment.is_some() {
            // The replacement routes as an escalated redispatch of the same node.
            return Ok(());
        }
        // A degenerate turn that still changed the task (a plan, a question) stands;
        // the driver continues from that state as after any other turn.
        if self.turn_marker(task).await?.state_and_revision != before.state_and_revision {
            return Ok(());
        }
        match self
            .turn_once(
                task,
                owner,
                SessionRole::Coordinator,
                retry.clone(),
                prompt,
                cancelled.clone(),
                None,
            )
            .await
        {
            Err(error) if error.code() == DEGENERATE => {}
            other => return self.keep_committed_turn(task, &before, other).await,
        }
        if self.turn_marker(task).await?.state_and_revision != before.state_and_revision {
            return Ok(());
        }
        // Never the same model a third time: move the coordinator to another one.
        let current = self.coordinator_route(task, Some(retry)).await?;
        let Some(alternate) = self
            .escalate_coordinator(task, &current, DEGENERATE)
            .await?
        else {
            return Err(CoreError::Refused(DEGENERATE));
        };
        match self
            .turn_once(
                task,
                owner,
                SessionRole::Coordinator,
                alternate,
                prompt,
                cancelled,
                None,
            )
            .await
        {
            Err(error) if error.code() == DEGENERATE => Err(CoreError::Refused(DEGENERATE)),
            other => self.keep_committed_turn(task, &before, other).await,
        }
    }

    /// A tool can commit a plan, result or question before the prompt reply is
    /// lost. Keep that checkpoint rather than stopping or replaying the turn.
    async fn keep_committed_turn(
        &self,
        task: &str,
        before: &TurnMarker,
        result: CoreResult<()>,
    ) -> CoreResult<()> {
        if matches!(&result, Err(error) if error.code() == "native_turn_outcome_unknown") {
            let after = self.turn_marker(task).await?;
            // Submitting the final synthesis can commit without changing state or revision.
            if after.state_and_revision != before.state_and_revision
                || (!before.has_synthesis && after.has_synthesis)
            {
                return Ok(());
            }
        }
        result
    }

    /// Move the task's coordinator to another qualified model than `current`
    /// (preferably another engine or account), recorded as
    /// `NATIVE_COORDINATOR_ESCALATED`. Later coordinator turns of the task use it
    /// until the owner chooses a model again. `None` when no other model qualifies.
    pub(crate) async fn escalate_coordinator(
        &self,
        task: &str,
        current: &SessionRoute,
        reason: &str,
    ) -> CoreResult<Option<SessionRoute>> {
        let Some(pin) = self
            .routes
            .alternate_coordinator_pin(&self.host.root, current)?
        else {
            return Ok(None);
        };
        let route = match self.routes.pinned_route(&self.host.root, &pin, None) {
            Ok(route) => route,
            Err(_) => return Ok(None),
        };
        let t = task.to_owned();
        let record = json!({"pin":pin,"reason":reason,"from":{"connection":current.connection,"model":current.model},
            "to":{"connection":route.connection,"model":route.model}});
        self.host
            .executor
            .write(move |db| {
                cortex_core::lifecycle::event_tx(db, &t, "NATIVE_COORDINATOR_ESCALATED", record)?;
                Ok(())
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        Ok(Some(route))
    }
    /// Read the durable checkpoint indicators together, without loading synthesis text.
    async fn turn_marker(&self, task: &str) -> CoreResult<TurnMarker> {
        let t = task.to_owned();
        self.host
            .executor
            .read(move |db| {
                Ok(db.query_row(
                    "SELECT state,revision,synthesis IS NOT NULL FROM runtime_tasks WHERE task_id=?",
                    [&t],
                    |r| {
                        Ok(TurnMarker {
                            state_and_revision: (r.get(0)?, r.get(1)?),
                            has_synthesis: r.get(2)?,
                        })
                    },
                )?)
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
    pub(super) async fn turn_once(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        role: SessionRole,
        route: SessionRoute,
        prompt: &str,
        cancelled: watch::Receiver<bool>,
        launch: Option<LaunchTicket>,
    ) -> CoreResult<()> {
        self.turn_once_scoped(task, owner, role, route, prompt, cancelled, launch, None, None)
            .await.map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn turn_once_scoped(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        mut role: SessionRole,
        mut route: SessionRoute,
        prompt: &str,
        cancelled: watch::Receiver<bool>,
        mut launch: Option<LaunchTicket>,
        merge_handoff: Option<&str>,
        steering: Option<SteeringPrompt>,
    ) -> CoreResult<TurnOutcome> {
        // The task's settings must still be readable before any turn starts.
        self.task_settings(task).await?;
        let active_steering = matches!(role, SessionRole::ActiveSteering);
        let owner_decision = matches!(role, SessionRole::OwnerDecision { .. });
        let coordinator = active_steering || owner_decision || matches!(role, SessionRole::Coordinator);
        if coordinator {
            route = self.coordinator_route(task, Some(route)).await?;
            if owner_decision {
                route.deadline = route.deadline.min(self.routes.owner_decision_timeout(&self.host.root));
            }
            let t = task.to_owned();
            if self
                .host
                .executor
                .read(move |db| cortex_core::lifecycle::automatic::enabled(db, &t))?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??
            {
                self.routes
                    .automatic_coordinator_health(&self.host.root, &route)?;
            }
        }
        // The owner's language (an explicit en/ru setting, otherwise the goal's language)
        // is recorded in task_language and stated per role: owner-facing text follows it,
        // worker-to-worker material stays English.
        let t = task.to_owned();
        let language = self
            .host
            .executor
            .write(move |db| cortex_core::assignment_brief::record_language_tx(db, &t))?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))??;
        route.instructions.push('\n');
        route.instructions.push_str(&if coordinator {
            crate::native_instructions::coordinator_language_directive(language)
        } else {
            format!("Owner-selected response language: {language}. user_question_language: {language}. It applies only to \
a question to the owner (question text and option labels). Write reports, report headings, summaries and every other message in English.")
        });
        let mut prompt = prompt.to_owned();
        if self.host.workspace_state.is_some() {
            let t = task.to_owned();
            let contract = self.host.executor.read(move |db| {
                cortex_core::deliverables::runtime_contract(db, &t)
            })?.await.map_err(|_| CoreError::Refused("native_host_closed"))??;
            if let Some(contract) = contract {
                if let Ok(mut brief) = serde_json::from_str::<serde_json::Map<String, Value>>(&prompt) {
                    brief.insert("sprint_deliverables".into(), contract);
                    prompt = Value::Object(brief).to_string();
                } else {
                    prompt.push_str(&format!("\n\nMaterialized sprint input and output commitments.\n{contract}"));
                }
            }
        }
        if coordinator && !active_steering && !owner_decision {
            if let Some(state) = self.host.workspace_state.clone() {
                let t = task.to_owned();
                let root = self.host.root.clone();
                let summary = self
                    .host
                    .executor
                    .read(move |db| {
                        crate::machine::sprint_graph::Scope::load(db, &state, &root, &t)?
                            .map(|scope| scope.start(db))
                            .transpose()
                    })?
                    .await
                    .map_err(|_| CoreError::Refused("native_host_closed"))?;
                match summary {
                    Ok(Some(summary)) => prompt.push_str(&format!("\n\nSprint dependencies (Workspace records). Use sprint_graph to explore the graph and sprint_task_result to read a member’s outcome. For each prerequisite your work uses, read its relevant reports through the final page, check their result bindings, and verify inputs in this checkout. A settled card alone does not establish that its result or files are available here. Treat missing, stale, or partial evidence as unresolved and raise a blocker if work needs it.\n{summary}")),
                    Ok(None) => {},
                    Err(error) => prompt.push_str(&format!("\n\nSprint context unavailable ({}).", error.code())),
                }
                // An owner change of a sprint member: settle reviewed revisions and
                // point the coordinator at an open impact check (`sprint_impact`).
                let (t, state) = (task.to_owned(), self.host.workspace_state.clone());
                let routes = self.routes.clone();
                if let Some(state) = state {
                    match self.host.executor.publication_wait(move |store| {
                        let root = store.key().root().to_owned();
                        crate::machine::sprint_revision::coordinator_turn(store, &state, &t,
                            &|c| routes.sprint_route(&root, c).ok().flatten())
                    }).await {
                        Ok(note) if !note.is_empty() => prompt.push_str(&format!("\n\n{note}")),
                        Ok(_) => {}
                        Err(error) => prompt.push_str(&format!("\n\nSprint impact check unavailable ({}).", error.code())),
                    }
                }
            }
            let t = task.to_owned();
            let instruction = self
                .host
                .executor
                .read(move |db| {
                    cortex_core::lifecycle::skill_proposals::planning_instruction(db, &t)
                })?
                .await
                .map_err(|_| CoreError::Refused("native_host_closed"))??;
            if !instruction.is_empty() {
                prompt.push_str(&format!("\n\n{instruction}"));
            }
        }
        let order = launch.as_ref().map(LaunchTicket::order);
        loop {
            let mut attempt = RouteAttempt::new(
                self.host.route_holds.clone(),
                RouteKey(route.launch.engine.name(), route.connection.clone(), route.model.clone()),
            );
            let result = self.turn_once_attempt(
                task, owner, role.clone(), route.clone(), &prompt, cancelled.clone(),
                launch.take(), &mut attempt, merge_handoff, steering,
            ).await;
            if active_steering || owner_decision || merge_handoff.is_some() || !matches!(&result, Err(error) if error.code() == LIMITED) {
                return result;
            }
            if *self.stop.borrow() || *cancelled.borrow() {
                return Ok(TurnOutcome::Ordinary);
            }
            let channel = attempt.refused_channel.take().expect("confirmed provider refusal");
            if let SessionRole::Worker { assignment } = &mut role {
                let t = task.to_owned();
                let o = owner.clone();
                let old = assignment.clone();
                let result = self.host.executor.write(move |db| {
                    cortex_core::scheduler::replace_provider_refused_claim_tx(db, &t, &o, &old, &channel)
                })?.await.map_err(|_| CoreError::Refused("native_host_closed"))?;
                match result {
                    Ok(new) => {
                        if let Ok(mut brief) = serde_json::from_str::<Value>(&prompt) {
                            if brief.is_object() && brief["assignment_ref"] == json!(*assignment) {
                                brief["assignment_ref"] = json!(new);
                                prompt = brief.to_string();
                            }
                        }
                        if let Ok(mut brief) = serde_json::from_str::<Value>(&prompt) {
                            if brief["assignment_ref"].as_str() == Some(assignment.as_str()) {
                                brief["assignment_ref"] = json!(new);
                                prompt = brief.to_string();
                            }
                        }
                        *assignment = new;
                        let order = order.clone().unwrap_or_default();
                        launch = Some(order.push(assignment.clone(), self.host.capacity_changed.clone()));
                    }
                    // A durable pause, replan, or result wins over a retry.
                    Err(error) if error.code() == "native_provider_retry_claim_changed" => return Ok(TurnOutcome::Ordinary),
                    Err(error) => return Err(error),
                }
            }
        }
    }

    async fn worker_fact_memory(
        &self, task: &str, assignment: &str, checkout: &std::path::Path,
    ) -> Option<Value> {
        use cortex_core::strategy::memory;
        let (task, assignment, checkout) =
            (task.to_owned(), assignment.to_owned(), checkout.to_owned());
        self.host.executor.read(move |db| {
            let scope: String = db.query_row(
                "SELECT scope_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
                (&task, &assignment),
                |r| r.get(0),
            )?;
            let scope = cortex_core::scheduler::policy::effective_scope_tx(
                db, &task, Some(&assignment), &scope,
            )?;
            memory::select(
                db, &task, &scope, crate::native_project::scope_covers,
                |path| crate::native_project::memory_sha256(&checkout, path),
                memory::BRIEF_BYTES,
            )
        }).ok()?.await.ok()?.ok().flatten()
    }

    async fn turn_once_attempt(
        &self,
        task: &str,
        owner: &OwnerEpoch,
        role: SessionRole,
        route: SessionRoute,
        prompt: &str,
        mut cancelled: watch::Receiver<bool>,
        launch: Option<LaunchTicket>,
        attempt: &mut RouteAttempt,
        merge_handoff: Option<&str>,
        steering: Option<SteeringPrompt>,
    ) -> CoreResult<TurnOutcome> {
        if self.archive_fenced(task, true).await? {
            return Err(CoreError::Refused("task_archived"));
        }
        let assignment = match &role {
            SessionRole::Worker { assignment } => Some(assignment.clone()),
            _ => None,
        };
        let is_worker = assignment.is_some();
        let decision = match &role {
            SessionRole::OwnerDecision { obstacle } =>
                Some((obstacle.clone(), tokio::time::Instant::now() + route.deadline)),
            _ => None,
        };
        let active_steering = matches!(role, SessionRole::ActiveSteering);
        // Register owner guidance/stop before dispatch: dispatch already reports the
        // worker ready, and a message sent at that moment must reach it.
        let (sender, mut guidance) = tokio::sync::mpsc::channel(8);
        let (worker_stop, mut worker_cancelled) = watch::channel(false);
        let (worker_done, worker_finished) = watch::channel(false);
        if let Some(a) = &assignment {
            self.guidance.lock().await.insert(a.clone(), sender);
            self.worker_stops
                .lock()
                .await
                .insert(a.clone(), (worker_stop, worker_finished));
        }
        let mut stop = self.stop.subscribe();
        let launch = launch.map(|ticket| {
            WorkerLaunch::new(ticket, stop.clone(), cancelled.clone(), worker_cancelled.clone())
        });
        let driver = match self
            .host
            .dispatch_managed(task.into(), owner.clone(), role, route, launch, Some((attempt, LaunchInterrupts {
                service: stop.clone(),
                batch: cancelled.clone(),
                worker: worker_cancelled.clone(),
            })))
            .await
        {
            Ok(driver) => driver,
            Err(error) => {
                if let Some(a) = &assignment {
                    self.guidance.lock().await.remove(a);
                    self.worker_stops.lock().await.remove(a);
                }
                match error.code() {
                    // No launch to settle. The batch reconciles the current task state.
                    "native_task_interrupted" => return Ok(if active_steering {
                        TurnOutcome::SteeringUnconfirmed
                    } else {
                        TurnOutcome::Ordinary
                    }),
                    "native_worker_stopped_before_launch" => {
                        worker_done.send_replace(true);
                        return Ok(TurnOutcome::Ordinary);
                    }
                    _ => return Err(error),
                }
            }
        };
        driver.treat_refusal_as_degenerate();
        if let Some(input) = steering {
            let confirmed = if *stop.borrow_and_update() || *cancelled.borrow() {
                Ok(false)
            } else {
                tokio::select! {
                    biased;
                    _ = stop.changed() => Ok(false),
                    _ = cancelled.changed() => Ok(false),
                    _ = tokio::time::sleep_until(input.deadline) => driver.expire_steering().await,
                    reply = driver.prompt_active_steering(prompt, input) => reply.map(|reply| reply.steering_confirmed),
                }
            };
            // Neither the prompt deadline nor a worker ending can drop the owned
            // shutdown. It observes process exit or records an ambiguous settlement.
            let settled = driver.shutdown_managed(None).await;
            if matches!(confirmed, Ok(true)) {
                attempt.succeeded();
            }
            settled?;
            return confirmed.map(|confirmed| if confirmed {
                TurnOutcome::SteeringConfirmed
            } else {
                TurnOutcome::SteeringUnconfirmed
            });
        }
        if let Some((obstacle, deadline)) = decision {
            let result = if *stop.borrow_and_update() || *cancelled.borrow() {
                Err(CoreError::Refused("native_task_interrupted"))
            } else {
                tokio::select! {
                    biased;
                    _ = stop.changed() => Err(CoreError::Refused("native_service_stopped")),
                    _ = cancelled.changed() => Err(CoreError::Refused("native_task_interrupted")),
                    _ = tokio::time::sleep_until(deadline) => Err(CoreError::Refused("owner_decision_turn_timeout")),
                    result = driver.prompt(prompt) => result.map(|_| ()),
                }
            };
            // Fence a late tool call in the writer transaction before awaiting
            // process settlement. A decision already committed wins this race.
            let fallback = if !*stop.borrow() && !*cancelled.borrow() {
                let reason = match &result {
                    Ok(()) => "no_decision",
                    Err(error) if error.code() == "owner_decision_turn_timeout" => "timeout",
                    Err(_) => "turn_failed",
                };
                self.fallback_owner_decision(task, owner, &obstacle, reason).await
            } else {
                Ok(false)
            };
            if result.is_ok() {
                attempt.succeeded();
            }
            driver.shutdown_managed(None).await?;
            fallback?;
            return Ok(TurnOutcome::Ordinary);
        }
        // How the turn ended, as the runtime observed it; recorded for a worker below.
        let turn: CoreResult<&'static str> = if *stop.borrow_and_update() || *cancelled.borrow() {
            Err(CoreError::Refused("native_service_stopped"))
        } else if *worker_cancelled.borrow() {
            Ok("owner_stopped")
        } else {
            tokio::select! {
                result = async {
                    let mut handled_refusal = if is_worker { driver.event_cursor().await? } else { 0 };
                    let mut continuations = 0;
                    let mut worker_prompt = None;
                    if let (Some(assignment), Ok(mut brief)) = (&assignment, serde_json::from_str::<Value>(prompt)) {
                        if brief["stage"] == "execution" {
                            if brief["assignment_ref"] != *assignment {
                                return Err(CoreError::Refused("native_worker_brief_assignment_mismatch"));
                            }
                            let retained_memory = brief.as_object_mut()
                                .and_then(|fields| fields.remove("fact_memory")).is_some();
                            if let Some(memory) = self.worker_fact_memory(task, assignment, driver.memory_checkout()).await {
                                brief["fact_memory"] = memory;
                                worker_prompt = Some(brief.to_string());
                            } else if retained_memory {
                                worker_prompt = Some(brief.to_string());
                            }
                            let (t, owner) = (task.to_owned(), owner.clone());
                            self.host.executor.write(move |db| {
                                cortex_core::ownership::check_tx(db, &t, &owner)?;
                                cortex_core::lifecycle::event_tx(db, &t, "NATIVE_WORKER_BRIEF_RETAINED",
                                    json!({"brief":brief})).map(|_| ())
                            })?.await.map_err(|_| CoreError::Refused("native_host_closed"))??;
                        }
                    }
                    self.guided_prompt(&driver, worker_prompt.as_deref().unwrap_or(prompt), &mut guidance, is_worker).await?;
                    if let Some(request) = merge_handoff {
                        let (t, request, channel) = (
                            task.to_owned(), request.to_owned(), driver.channel_id().to_owned(),
                        );
                        self.host.executor.write(move |db| {
                            cortex_core::lifecycle::event_tx(db, &t, "WORKSPACE_MERGE_HANDOFF_DELIVERED",
                                json!({"request":request,"channel_id":channel})).map(|_| ())
                        })?.await.map_err(|_| CoreError::Refused("native_host_closed"))??;
                    }
                    attempt.succeeded();
                    loop {
                        let wake = self.wake.notified();
                        tokio::pin!(wake);
                        wake.as_mut().enable();
                        use crate::native_sessions::SessionWait;
                        match driver.wait_state().await? {
                            SessionWait::Ready => { driver.continue_answer().await?; },
                            SessionWait::Open(retry) => {
                                #[cfg(test)]
                                if is_worker {
                                    self.host.guidance_question_wait.notify_one();
                                }
                                tokio::select! {
                                    _ = &mut wake => {},
                                    _ = tokio::time::sleep(retry.unwrap_or_default()), if retry.is_some() => {},
                                    // A worker process that dies while its question is open is
                                    // noticed now, not when the answer arrives. The turn ends
                                    // and the exited process is settled; the lost-worker path
                                    // then replaces the assignment and retires its wait.
                                    _ = driver.process_exited(), if is_worker => return Ok("process_exited"),
                                    next = driver.coordinator_guidance(true), if is_worker => {
                                        driver.forward_coordinator_guidance(next?).await?;
                                    },
                                    Some(message) = guidance.recv() => {
                                        let result = driver.prompt(&crate::native_owner_controls::guidance_prompt(&message)).await;
                                        driver.guidance_delivery(message, result.is_ok(), false).await?;
                                        result?;
                                    }
                                }
                            },
                            _ if is_worker && continuations < 2 => {
                                let next=tokio::select!{
                                    result=driver.command_continuation(handled_refusal)=>result?,
                                    _=driver.process_exited()=>return Ok("process_exited"),
                                };
                                let Some((id,message))=next else {break;};
                                handled_refusal=id;
                                continuations+=1;
                                self.guided_prompt(&driver,&message,&mut guidance,true).await?;
                            }
                            _ => break,
                        }
                    }
                    Ok("turn_ended")
                } => result,
                _ = stop.changed() => Err(CoreError::Refused("native_service_stopped")),
                _ = cancelled.changed() => Err(CoreError::Refused("native_task_interrupted")),
                _ = worker_cancelled.changed() => Ok("owner_stopped"),
            }
        };
        // Always observe the owned wait, including prompt failure or service shutdown.
        if let Some(a) = &assignment {
            self.guidance.lock().await.remove(a);
        }
        let diagnostics = driver.diagnostics();
        let channel = driver.channel_id().to_owned();
        let retryable_prompt = matches!(&turn, Err(error) if error.code() == "native_turn_outcome_unknown");
        let settled = driver.shutdown_managed(retryable_prompt.then_some(&mut *attempt)).await;
        if settled.is_ok() {
            worker_done.send_replace(true);
        }
        if let Some(a) = assignment.as_ref().filter(|_| attempt.refused_channel.is_none()) {
            // Why this attempt ended, for the lost-worker path's classification.
            let cause = match &turn {
                Ok(cause) => *cause,
                Err(error) => error.code(),
            };
            if let Err(error) = self
                .record_worker_ending(task, a, &channel, cause, diagnostics.snapshot())
                .await
            {
                eprintln!(
                    "workspace-service: the end of a worker attempt was not recorded ({})",
                    error.code()
                );
            }
        }
        if let Some(a) = assignment {
            self.worker_stops.lock().await.remove(&a);
        }
        settled?;
        if attempt.refused_channel.is_some() && !*worker_cancelled.borrow() {
            return Err(CoreError::Refused(LIMITED));
        }
        // Worker endings are consumed by the worker retry path. A coordinator
        // interruption instead reaches the task driver, unless the turn committed
        // a checkpoint (checked by `keep_committed_turn`).
        if let Err(e) = turn {
            if !is_worker || e.code() != "native_turn_outcome_unknown" {
                return Err(e);
            }
        }
        Ok(TurnOutcome::Ordinary)
    }
    /// Records why a worker attempt that has no accepted result ended
    /// (`scheduler::retry::record_attempt_ending_tx`): the turn's end as the runtime
    /// observed it, with the engine's exit status or signal, transport close, a runtime
    /// cancel of a permission, an owner pause and a short redacted stderr tail. A turn
    /// that ended after refused results is the attempt's own quality
    /// (`result_refused`), as is a degenerate one (`degenerate_turn`); a process that
    /// vanished without the model ending its turn is `worker_lost`; the owner's pause
    /// or stop is `owner_paused`, a service stop `runtime_restart`, a command sandbox
    /// that could not be prepared `sandbox_unavailable`.
    async fn record_worker_ending(
        &self,
        task: &str,
        assignment: &str,
        channel: &str,
        cause: &str,
        diagnostics: cortex_engine::engine::DiagnosticSnapshot,
    ) -> CoreResult<()> {
        let (t, a, ch, cause) = (
            task.to_owned(),
            assignment.to_owned(),
            channel.to_owned(),
            cause.to_owned(),
        );
        self.host
            .executor
            .write(move |db| {
                use rusqlite::OptionalExtension;
                let Some(attempt) = db
                    .query_row(
                        "SELECT attempt_id FROM trusted_channels WHERE task_id=? AND channel_id=?",
                        (&t, &ch),
                        |r| r.get::<_, Option<String>>(0),
                    )
                    .optional()?
                    .flatten()
                else {
                    return Ok(());
                };
                let open: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND assignment_ref=?
                     AND result_json IS NULL AND status IN ('running','failed'))",
                    (&t, &a),
                    |r| r.get(0),
                )?;
                if !open {
                    return Ok(());
                }
                let facts = worker_ending_facts(db, &t, &ch, &attempt)?;
                let class = worker_ending_class(&cause, &facts);
                let mut ending = facts;
                ending["cause"] = json!(cause);
                ending["exit"] = json!(diagnostics.exit);
                ending["exited_unprompted"] = json!(diagnostics.exited_unprompted);
                ending["transport_close"] = json!(diagnostics.transport_close);
                ending["stderr_bytes"] = json!(diagnostics.stderr_bytes);
                ending["stderr_tail"] = json!(stderr_tail(&diagnostics.stderr_tail));
                cortex_core::scheduler::retry::record_attempt_ending_tx(
                    db, &t, &a, &attempt, &cause, class, &ending,
                )?;
                Ok(())
            })?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?
    }
}
