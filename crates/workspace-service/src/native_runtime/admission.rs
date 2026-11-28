//! Native Start and command admission.
use super::*;

impl Runtime {
    pub(crate) async fn start_queued(
        self: &Arc<Self>,
        project: String,
        request: Value,
        automatic: bool,
        expected: crate::machine::sprint_live::Expectation,
    ) -> io::Result<Value> {
        use cortex_core::workspace::NativeExecutionPolicy;
        if request["operation"] != "tasks.start" {
            return Err(failure("native_operation_unavailable"));
        }
        self.command_with_policy(
            project,
            request,
            None,
            automatic.then_some(NativeExecutionPolicy::AutomaticContinuation),
            Some(expected),
        )
        .await
    }
    /// Start a sprint's read-only planning task: bound in the Start transaction.
    pub(crate) async fn start_sprint_planning(
        self: &Arc<Self>,
        project: String,
        request: Value,
    ) -> io::Result<Value> {
        use cortex_core::workspace::NativeExecutionPolicy;
        if request["operation"] != "tasks.start" {
            return Err(failure("native_operation_unavailable"));
        }
        self.command_with_policy(
            project,
            request,
            None,
            Some(NativeExecutionPolicy::SprintPlanning),
            None,
        )
        .await
    }
    pub(crate) async fn client_command(
        self: &Arc<Self>,
        project: String,
        request: Value,
        client: Option<(String, Value, Option<cortex_core::input::Capture>)>,
    ) -> io::Result<Value> {
        self.command_with_policy(project, request, client, None, None)
            .await
    }
    async fn command_with_policy(
        self: &Arc<Self>,
        project: String,
        request: Value,
        client: Option<(String, Value, Option<cortex_core::input::Capture>)>,
        policy: Option<cortex_core::workspace::NativeExecutionPolicy>,
        queued: Option<crate::machine::sprint_live::Expectation>,
    ) -> io::Result<Value> {
        if *self.stop.borrow() {
            return Err(failure("service_stopping"));
        }
        let host = self.host.clone();
        let operation = request["operation"].as_str().unwrap_or("");
        if operation == "capabilities.get" {
            return capabilities();
        }
        if operation == "tasks.create" {
            return self.create_draft(project, request, None).await;
        }
        if matches!(operation, "decisions.list" | "decisions.get") {
            return self.decisions(project, request).await;
        }
        if operation == "decisions.answer" {
            return self.answer_command(project, request).await;
        }
        if operation == "reports.read" {
            let report = request["target"].as_str().unwrap_or("").to_owned();
            let cursor = request["cursor"].as_str().map(str::to_owned);
            let limit = request["limit"].as_u64().unwrap_or(4000).clamp(1, 4000) as usize;
            let value = host
                .executor
                .publication(move |store| {
                    cortex_core::workspace::read_report_page(
                        store,
                        &project,
                        &report,
                        cursor.as_deref(),
                        limit,
                    )
                })
                .map_err(err)?
                .await
                .map_err(|_| failure("native_host_closed"))?
                .map_err(err)?;
            return workspace_protocol::validate("ReportPage", &value)
                .map(|_| value)
                .map_err(|_| failure("invalid_report_page"));
        }
        if operation != "tasks.start" {
            return Err(failure("native_operation_unavailable"));
        }
        let _control = self.controls.lock().await;
        let replay_project = project.clone();
        let replay_request = request.clone();
        if let Some(receipt) = host
            .executor
            .publication({
                let check_replay = client.is_none() && queued.is_none();
                move |store| {
                    if !check_replay {
                        return Ok(None);
                    }
                    cortex_core::workspace::replay_native_start_with_policy(
                        store,
                        &replay_project,
                        &actor(),
                        &replay_request,
                        policy,
                    )
                }
            })
            .map_err(err)?
            .await
            .map_err(|_| failure("native_host_closed"))?
            .map_err(err)?
        {
            return Ok(receipt);
        }
        // Candidate parsing is deliberately not authorization. Refuse before admission/effects.
        // A desktop draft's saved model/effort is qualified exactly as its coordinator
        // turns will route it (`task_route`), never against the owner default alone.
        let saved = if client.is_none() {
            let (p, t) = (
                project.clone(),
                request["target"].as_str().unwrap_or("").to_owned(),
            );
            host.executor
                .publication(move |store| {
                    cortex_core::workspace::draft_settings(store, &p, &actor(), &t)
                })
                .map_err(err)?
                .await
                .map_err(|_| failure("native_host_closed"))?
                .map_err(err)?
        } else {
            None
        };
        // A task's own route never needs a resolvable owner default (incoming
        // clients carry their settings; a desktop draft its saved ones).
        let pin_settings = saved
            .clone()
            .or_else(|| client.as_ref().map(|(_, settings, _)| settings.clone()));
        let (coordinator, pin) = start_coordinator(&*self.routes, &host.root, pin_settings.as_ref()).map_err(err)?;
        let mut jobs = self.jobs.lock().await;
        if *self.stop.borrow() {
            return Err(failure("service_stopping"));
        }
        jobs.retain(|j| !j.is_finished());
        if jobs.len() >= 32 {
            return Err(failure("native_capacity_unavailable"));
        }
        // Own admission before awaiting SQLite. Losing a window/request must not
        // strand a committed Start or cause a retry to launch a second process.
        let (send, reply) = tokio::sync::oneshot::channel();
        let runtime = self.clone();
        jobs.push(tokio::spawn(async move {
            let maintenance = runtime.host.workspace_state.clone();
            let admitted = async {
                host.executor
                    .publication(move |store| {
                        let _guard = maintenance
                            .as_deref()
                            .map(crate::machine::maintenance::admission)
                            .transpose()
                            .map_err(|e| CoreError::Refused(e.code))?;
                        if client.is_none()
                            && cortex_core::workspace::draft_worktree_selected(
                                store, &project, &actor(), request["target"].as_str().unwrap_or(""),
                            )?
                        {
                            // Only the Automatic continuation policy may start in a worktree, and only
                            // for a member of a sprint that shares one worktree (checked by Core).
                            if policy.is_some() && policy != Some(cortex_core::workspace::NativeExecutionPolicy::AutomaticContinuation) {
                                return Err(CoreError::Refused("native_worktree_unavailable"));
                            }
                            let state = maintenance.as_deref()
                                .ok_or(CoreError::Refused("native_worktree_unavailable"))?;
                            return crate::machine::start_worktree(store, state, &project, &actor(), &request, queued.as_ref(), policy);
                        }
                        let start = || cortex_core::workspace::start_native_input_with_policy(
                            store, &project, &actor(), &request,
                            client.as_ref().map(|(session, settings, _)| (session.as_str(), settings)),
                            client.as_ref().and_then(|(_, _, capture)| capture.as_ref()),
                            policy,
                        );
                        if let Some(state) = maintenance.as_deref() {
                            crate::machine::task_rework::with_plain_start_guard(
                                state, &project, &request, queued.as_ref().map(|expected| (store, expected)), start,
                            )
                        } else if queued.is_some() {
                            Err(CoreError::Refused(crate::machine::sprint_live::STALE))
                        } else {
                            start()
                        }
                    })
                    .map_err(err)?
                    .await
                    .map_err(|_| failure("native_host_closed"))?
                    .map_err(err)
            }
            .await;
            match admitted {
                Err(error) => {
                    let _ = send.send(Err(error));
                }
                Ok(admitted) => {
                    if admitted["fresh"] != true {
                        let _ = send.send(Ok(admitted["receipt"].clone()));
                        return;
                    }
                    let task = admitted["task"].as_str().unwrap().to_owned();
                    let owner = OwnerEpoch {
                        owner: admitted["owner"]["owner"].as_str().unwrap().into(),
                        epoch: admitted["owner"]["epoch"].as_i64().unwrap(),
                    };
                    let goal = admitted["goal"].as_str().unwrap().to_owned();
                    // Pin the resolved coordinator route with the task's start.
                    if let Some(pin) = pin {
                        let t = task.clone();
                        if let Ok(reply) = runtime.host.executor.write(move |db| {
                            cortex_core::lifecycle::event_tx(
                                db,
                                &t,
                                "NATIVE_COORDINATOR_ROUTE",
                                pin,
                            )?;
                            Ok(())
                        }) {
                            let _ = reply.await;
                        }
                    }
                    // Resolve the committed binding before preflight. On a lost
                    // checkout the driver handles recovery; never observe main for it.
                    if let Ok(checkout) = runtime.host.execution_checkout(&task, &owner).await {
                        crate::native_preflight::record(
                            &runtime.host.executor, &task, checkout.path(),
                        ).await;
                    }
                    match runtime.archive_fenced(&task, true).await {
                        Ok(false) => {}
                        Ok(true) => {
                            let _ = send.send(Err(failure("task_archived")));
                            return;
                        }
                        Err(error) => {
                            let _ = send.send(Err(err(error)));
                            return;
                        }
                    }
                    let (cancel, cancelled) = watch::channel(false);
                    let (done, finished) = watch::channel(false);
                    runtime.install_pump(task.clone(), cancel, finished).await;
                    let _ = send.send(Ok(admitted["receipt"].clone()));
                    if let Err(error) = runtime
                        .drive(&task, &owner, &goal, coordinator, cancelled.clone(), None)
                        .await
                    {
                        if *cancelled.borrow() || *runtime.stop.borrow() {
                            done.send_replace(true);
                            return;
                        }
                        runtime
                            .stop_after_error(&task, &owner, error, cancelled.clone())
                            .await;
                    }
                    runtime.driver_ended(&task, &cancelled).await;
                    done.send_replace(true);
                }
            }
        }));
        drop(jobs);
        reply
            .await
            .map_err(|_| failure("native_start_outcome_unknown"))?
    }
}
