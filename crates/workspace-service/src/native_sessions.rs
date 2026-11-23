//! Native session composition. Desktop activation does not select this host yet.
//! One StoreExecutor owns its project lease; never compose it beside a child scheduler.
use cortex_core::{
    channels::{self, Credential, Identity, Principal},
    lifecycle::{
        self,
        completion::EvidenceSnapshot,
        domain,
        runtime::{RuntimeAction, RuntimeRequest},
        waits,
    },
    ownership::OwnerEpoch,
    ports::ProcessObservation,
    scheduler::{self, Reservation, ReservationKind},
    store::executor::StoreExecutor,
    CoreError, CoreResult, ProjectKey, TaskState,
};
use cortex_engine::{
    broker::Broker,
    engine::{Engine, Event, PermissionDecision, PermissionPolicy, Session},
    environment::LaunchSpec,
    gateway::{Channel, Credentials, Gateway, GatewayCore},
    Error, FutureResult,
};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};

/// Durable, secret-free record of each optional external server this session
/// runs without: the configured name and a stable reason code.
async fn record_withheld(
    executor: &Arc<StoreExecutor>,
    credential: &Credential,
    withheld: &[crate::native_mcp::Withheld],
) -> CoreResult<()> {
    if withheld.is_empty() {
        return Ok(());
    }
    let c = credential.clone();
    let rows = withheld
        .iter()
        .map(|w| (w.server.clone(), w.reason))
        .collect::<Vec<_>>();
    receive(executor.write(move |db| {
        let i = channels::authenticate(db, &c.channel_id, c.secret(), false)?;
        for (server, reason) in &rows {
            lifecycle::event_tx(
                db,
                &i.task,
                "NATIVE_EXTERNAL_MCP_WITHHELD",
                json!({"channel_id":c.channel_id,"server":server,"reason":reason}),
            )?;
        }
        Ok(())
    })?)
    .await
}
/// One failed session start attempt, as `record_start_failure` retains it.
struct StartFailure {
    attempt: u32,
    phase: &'static str,
    error: String,
    role: String,
    engine: &'static str,
    /// The task stop code for a final failure; `None` for an attempt that starts once more.
    blocker: Option<&'static str>,
    diagnostics: Option<cortex_engine::engine::DiagnosticSnapshot>,
    /// Withheld servers no event records yet (final failure only).
    withheld: Vec<(String, &'static str)>,
}
/// `NATIVE_SESSION_START_RETRIED` for a first attempt that starts once more,
/// `NATIVE_SESSION_START_FAILED` for the final one. When an engine process ran,
/// its exit status, transport close reason and stderr size go into the event and
/// its bounded, redacted stderr tail into a private file next to the task's
/// other runtime files, which the event names with its digest. A file that
/// cannot be written is reported in the event, never silently omitted, and
/// never stops the failure from being recorded.
async fn record_start_failure(
    executor: &Arc<StoreExecutor>,
    credential: &Credential,
    failure: StartFailure,
) -> CoreResult<()> {
    let channel = credential.channel_id.clone();
    receive(executor.publication(move |store| {
        let task: String = store.read(|db| {
            Ok(db.query_row(
                "SELECT task_id FROM trusted_channels WHERE channel_id=?",
                [&channel],
                |r| r.get(0),
            )?)
        })?;
        let diagnostics = failure.diagnostics.as_ref().map(|d| {
            let mut v = json!({"exit":d.exit,"exited_unprompted":d.exited_unprompted,
                "transport_close":d.transport_close,"stderr_bytes":d.stderr_bytes});
            match write_start_diagnostics(store, &task, &channel, &failure, d) {
                Ok((file, sha256, bytes)) => {
                    v["file"] = json!(file);
                    v["sha256"] = json!(sha256);
                    v["file_bytes"] = json!(bytes);
                }
                Err(e) => v["file_error"] = json!(e.code()),
            }
            v
        });
        store.write(|db| {
            let mut payload = json!({"channel_id":channel,"phase":failure.phase,"error":failure.error,
                "attempt":failure.attempt,"role":failure.role,"engine":failure.engine,"diagnostics":diagnostics});
            let kind = match failure.blocker {
                Some(blocker) => {
                    for (server, reason) in &failure.withheld {
                        lifecycle::event_tx(
                            db,
                            &task,
                            "NATIVE_EXTERNAL_MCP_WITHHELD",
                            json!({"channel_id":channel,"server":server,"reason":reason}),
                        )?;
                    }
                    payload["blocker"] = json!(blocker);
                    "NATIVE_SESSION_START_FAILED"
                }
                None => "NATIVE_SESSION_START_RETRIED",
            };
            lifecycle::event_tx(db, &task, kind, payload)?;
            Ok(())
        })
    })?)
    .await
}
/// Writes `<store>/<task>/.native-diagnostics/<channel>-start-<attempt>.log`
/// (0600, never replaced). Returns its name, SHA-256 and size.
fn write_start_diagnostics(
    store: &cortex_core::store::Store,
    task: &str,
    channel: &str,
    failure: &StartFailure,
    d: &cortex_engine::engine::DiagnosticSnapshot,
) -> CoreResult<(String, String, usize)> {
    use sha2::{Digest, Sha256};
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    if channel.is_empty()
        || !channel
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        || task.is_empty()
        || task.contains(['/', '\0'])
        || task == "."
        || task == ".."
    {
        return Err(CoreError::Refused("invalid_channel"));
    }
    let task_dir = store.key().directory().join(task);
    cortex_core::store::locks::private_directory(&task_dir, true)?;
    let dir = task_dir.join(".native-diagnostics");
    cortex_core::store::locks::private_directory(&dir, true)?;
    let name = format!("{channel}-start-{}.log", failure.attempt);
    let body = format!(
        "engine: {}\nrole: {}\nattempt: {}\nphase: {}\nerror: {}\nexit: {}\nexited_unprompted: {}\n\
         transport_close: {}\nstderr_bytes: {}\nstderr_tail (redacted, at most {} bytes):\n{}\n",
        failure.engine,
        failure.role,
        failure.attempt,
        failure.phase,
        failure.error,
        d.exit.as_deref().unwrap_or("unknown"),
        d.exited_unprompted,
        d.transport_close.unwrap_or("open"),
        d.stderr_bytes,
        cortex_engine::engine::STDERR_TAIL,
        d.stderr_tail,
    );
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dir.join(&name))?;
    file.write_all(body.as_bytes())?;
    file.sync_all()?;
    std::fs::File::open(&dir)?.sync_all()?;
    Ok((
        name,
        format!("{:x}", Sha256::digest(body.as_bytes())),
        body.len(),
    ))
}
/// Owner-visible timeline note for each withheld server, bound to this exact
/// engine session like any other display-only activity row.
async fn note_withheld(
    executor: &Arc<StoreExecutor>,
    credential: &Credential,
    session: &str,
    withheld: &[crate::native_mcp::Withheld],
) -> CoreResult<()> {
    for w in withheld {
        let update = json!({"sessionId":session,"update":{"sessionUpdate":"tool_call",
            "toolCallId":format!("cortex-external-mcp-withheld:{}", w.server),
            "title":format!("External MCP server {} was not attached ({}); this session continues without it", w.server, w.reason),
            "kind":"other","status":"failed"}});
        crate::native_activity::record(executor, credential, session, update).await?;
    }
    Ok(())
}
/// The engine's own display name for the model a session runs on, read from the model option the
/// engine advertised (the same list `session/new` returned). An alias entry such as the Claude
/// adapter's `default` is named "Default (recommended)" and describes the concrete model it
/// resolves to ("Opus 5.5"): that reads "Default (Opus 5.5)". None when the engine names nothing
/// better than the id.
pub(crate) fn model_label(options: &Value, model: &str) -> Option<String> {
    fn find<'a>(list: &'a Value, model: &str) -> Option<&'a Value> {
        list.as_array()?.iter().find_map(|o| {
            if o["value"] == model {
                Some(o)
            } else {
                find(&o["options"], model)
            }
        })
    }
    let option = options
        .as_array()?
        .iter()
        .find(|o| o["id"] == "model" || o["category"] == "model")?;
    let entry = find(&option["options"], model)?;
    let name = entry["name"]
        .as_str()
        .map(str::trim)
        .filter(|n| !n.is_empty() && *n != model)?;
    let description = entry["description"]
        .as_str()
        .map(str::trim)
        .filter(|d| !d.is_empty() && d.chars().count() <= 40 && !d.contains('\n') && *d != name);
    let alias = name.to_ascii_lowercase().starts_with("default");
    Some(match description {
        Some(d) if alias => format!("Default ({d})"),
        _ => name.to_owned(),
    })
}
fn engine_error(_: CoreError) -> Error {
    Error::Forbidden
}
async fn receive<T>(reply: tokio::sync::oneshot::Receiver<CoreResult<T>>) -> CoreResult<T> {
    reply
        .await
        .map_err(|_| CoreError::Refused("session_host_closed"))?
}
fn view(i: &Identity) -> Channel {
    Channel {
        task: i.task.clone(),
        attempt: i.attempt.clone(),
        owner_epoch: i.epoch as u64,
        principal: i.principal.clone(),
        role: i.role.clone(),
        profile: i.profile.clone(),
        stage: i.stage.clone(),
    }
}
fn same(i: &Identity, c: &Channel) -> bool {
    i.task == c.task
        && i.attempt == c.attempt
        && i.epoch as u64 == c.owner_epoch
        && i.principal == c.principal
        && i.role == c.role
        && i.profile == c.profile
        && i.stage == c.stage
}
/// Trusted in-process surface, not model/desktop JSON authority. The embedding
/// service must validate installed routes/accounts and qualified instructions.
/// Optional trusted owner-client decision transport. Core still authenticates
/// the exact task/channel/session before and after the asynchronous decision.
pub trait ClientPermissions: Send + Sync {
    fn decide<'a>(
        &'a self,
        session: &'a str,
        request: &'a Value,
    ) -> FutureResult<'a, PermissionDecision>;
}
pub struct NativeHost {
    pub(super) workspace_state: Option<PathBuf>,
    pub(super) mcp: crate::native_mcp::Registry,
    pub(super) routes: std::sync::RwLock<Option<Arc<dyn crate::native_runtime::NativeRoutes>>>,
    pub(super) permissions: std::sync::RwLock<Option<Arc<dyn ClientPermissions>>>,
    pub(super) executor: Arc<StoreExecutor>,
    pub(super) root: PathBuf,
    pub(super) driver: tokio::sync::Mutex<()>,
    pub(super) commands: crate::native_commands::Running,
    pub(super) capacity_changed: Arc<tokio::sync::Notify>,
    pub(super) route_holds: Arc<crate::native_route_hold::Holds>,
}
impl NativeHost {
    /// The trusted owner-client decision transport for native permission cards.
    pub fn bind_permissions(&self, provider: Arc<dyn ClientPermissions>) {
        *self.permissions.write().unwrap() = Some(provider);
    }
    /// Construct and finally drop outside a reactor: StoreExecutor joins its lanes.
    /// The owner fence is acquired before initialization, with no fallback on refusal.
    pub fn open(key: ProjectKey) -> CoreResult<Self> {
        Self::open_with_mcp(key, crate::native_mcp::Registry::default())
    }
    pub fn open_with_mcp(key: ProjectKey, mcp: crate::native_mcp::Registry) -> CoreResult<Self> {
        Self::open_inner(key, mcp, false, false)
    }
    /// Only the service recovery composition may take over legacy tasks. The
    /// executor acquires both lifetime fences before opening the writable store.
    pub(crate) fn open_for_recovery(
        key: ProjectKey,
        mcp: crate::native_mcp::Registry,
    ) -> CoreResult<Self> {
        Self::open_inner(key, mcp, true, false)
    }
    /// Recovery open for one of several same-project incoming agents: the
    /// scheduler lease is handed over while no task is unfinished.
    pub(crate) fn open_shared_for_recovery(
        key: ProjectKey,
        mcp: crate::native_mcp::Registry,
    ) -> CoreResult<Self> {
        Self::open_inner(key, mcp, true, true)
    }
    fn open_inner(
        key: ProjectKey,
        mcp: crate::native_mcp::Registry,
        recover: bool,
        shared: bool,
    ) -> CoreResult<Self> {
        let root = key.root().to_owned();
        let executor = Arc::new(if shared {
            StoreExecutor::open_shared(key)?
        } else {
            StoreExecutor::open(key)?
        });
        // No legacy task is silently adopted. A stopped legacy process does not
        // make its unfinished tasks native or prove that their effects settled.
        let legacy_active = executor.read(|db| {
            Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM runtime_tasks t WHERE t.state NOT IN ('COMPLETED','CANCELLED','FAILED')
                 AND NOT EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=t.task_id AND e.type IN ('TASK_CREATED','NATIVE_TASK_ADOPTED')
                 AND json_extract(e.payload_json,'$.execution_host')='rust'))",
                [], |r| r.get::<_, bool>(0),
            )?)
        })?.blocking_recv().map_err(|_| CoreError::Refused("session_host_closed"))??;
        if legacy_active && !recover {
            return Err(CoreError::Refused("legacy_runtime_tasks_active"));
        }
        // No engine request outlives the process that held it: an owner decision
        // still open from an earlier run is withdrawn, never answered into the void.
        // A shared (ACP) store may have another live process; its decisions stay.
        if !shared {
            executor
                .write(|db| {
                    cortex_core::permission_decisions::withdraw_all_open_tx(db, "service_restarted")
                })?
                .blocking_recv()
                .map_err(|_| CoreError::Refused("session_host_closed"))??;
        }
        Ok(Self {
            workspace_state: None,
            mcp,
            routes: std::sync::RwLock::new(None),
            permissions: std::sync::RwLock::new(None),
            executor,
            root,
            driver: tokio::sync::Mutex::new(()),
            commands: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
            capacity_changed: Arc::new(tokio::sync::Notify::new()),
            route_holds: Default::default(),
        })
    }
    pub async fn create(
        &self,
        task: String,
        goal: String,
        owner: String,
    ) -> CoreResult<OwnerEpoch> {
        let root = self.root.clone();
        receive(self.executor.write(move |db| {
            if task.is_empty() || goal.trim().is_empty() { return Err(CoreError::Refused("invalid_task")); }
            db.execute("INSERT INTO tasks VALUES (?,?,?)", (&task,root.to_str(),cortex_core::now()))?;
            db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES (?,'PLANNING',?,?,?)",
                (&task,&goal,cortex_core::now(),cortex_core::now()))?;
            let epoch=cortex_core::ownership::acquire_tx(db,&task,&owner,None)?;
            lifecycle::event_tx(db,&task,"TASK_CREATED",json!({"execution_host":"rust"}))?;
            cortex_core::routing::snapshot_tx(db, &task, &root)?;
            lifecycle::event_tx(db,&task,"TASK_STATE_CHANGED",json!({"previous":"CREATED","state":"PLANNING"}))?;
            Ok(epoch)
        })?).await
    }
    /// Trusted desktop binding; no new owner, engine, or legacy-task adoption.
    pub async fn bind_workspace(
        &self,
        project: String,
        actor: String,
        draft: String,
        task: String,
        owner: OwnerEpoch,
    ) -> CoreResult<String> {
        receive(self.executor.publication(move |store| {
            cortex_core::workspace::bind_native_execution(
                store, &project, &actor, &draft, &task, &owner,
            )
        })?)
        .await
    }
    pub async fn owner_action(
        &self,
        task: String,
        owner: OwnerEpoch,
        key: String,
        action: RuntimeAction,
    ) -> CoreResult<Value> {
        // An owner selection is never runtime completion certification.
        if !matches!(
            action,
            RuntimeAction::Cancel { .. } | RuntimeAction::Interrupt | RuntimeAction::Pause { .. }
        ) {
            return Err(CoreError::Refused("owner_action_refused"));
        }
        receive(self.executor.write(move |db| {
            cortex_core::ownership::check_tx(db, &task, &owner)?;
            let actor = format!("native-owner:{}", owner.owner);
            let payload = json!({"action":action,"owner_epoch":owner.epoch});
            lifecycle::request_tx(db, &task, &actor, &key, "native_owner_action", &payload, || {
                let reply = lifecycle::runtime::execute_tx(db, &RuntimeRequest {
                    task:task.clone(),owner,key:key.clone(),action,evidence:EvidenceSnapshot::default(),
                })?;
                // Revocation shares the receipt: replay must not fence a new session.
                db.execute("UPDATE trusted_channels SET revoked_at=? WHERE task_id=? AND revoked_at IS NULL",
                    (cortex_core::now(), &task))?;
                Ok(reply)
            })
        })?)
        .await
    }
    pub async fn answer(
        &self,
        task: String,
        owner: OwnerEpoch,
        key: String,
        question: String,
        answer: String,
    ) -> CoreResult<Value> {
        receive(self.executor.write(move |db| {
            cortex_core::ownership::check_tx(db,&task,&owner)?;
            lifecycle::request_tx(db,&task,&owner.owner,&key,"native_answer",
                &json!({"question":question,"answer":answer,"epoch":owner.epoch}),|| {
                    Ok(json!({"recorded":waits::answer_tx(db,&task,&question,&answer,"text",None)?}))
                })
        })?).await
    }
    pub async fn state(&self, task: String) -> CoreResult<TaskState> {
        receive(
            self.executor
                .write(move |db| Ok(domain::task_tx(db, &task)?.0))?,
        )
        .await
    }
    /// Trusted reconciliation after a worker channel has been revoked or the
    /// service restarted. Reading a retained receipt never replays publication,
    /// restarts a worker or certifies its result.
    pub async fn result_receipt(
        &self,
        task: String,
        owner: OwnerEpoch,
        assignment: String,
        key: String,
    ) -> CoreResult<Option<Value>> {
        receive(self.executor.write(move |db| {
            cortex_core::ownership::check_tx(db, &task, &owner)?;
            let mut query = db.prepare(
                "SELECT r.result_json FROM runtime_requests r
                 JOIN trusted_channels c ON c.channel_id=r.actor AND c.task_id=r.task_id
                 JOIN delegation_provisioning p ON p.channel_id=c.channel_id AND p.task_id=c.task_id
                 WHERE r.task_id=? AND p.idempotency_key=? AND r.request_key=?
                 AND r.operation='result_submit' ORDER BY c.issued_at",
            )?;
            let rows = query
                .query_map((&task, &assignment, &key), |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            if rows.len() > 1 {
                return Err(CoreError::Refused("native_result_receipt_ambiguous"));
            }
            rows.into_iter()
                .next()
                .map(|s| Ok(serde_json::from_str(&s)?))
                .transpose()
        })?)
        .await
    }
    pub async fn dispatch(
        &self,
        task: String,
        owner: OwnerEpoch,
        who: SessionRole,
        route: SessionRoute,
    ) -> CoreResult<SessionDriver> {
        self.dispatch_with_wait(task, owner, who, route, None).await
    }

    pub(super) async fn dispatch_with_wait(
        &self,
        task: String,
        owner: OwnerEpoch,
        who: SessionRole,
        route: SessionRoute,
        wait: Option<WorkerLaunch>,
    ) -> CoreResult<SessionDriver> {
        self.dispatch_managed(task, owner, who, route, wait, None).await
    }

    pub(super) async fn dispatch_managed(
        &self,
        task: String,
        owner: OwnerEpoch,
        who: SessionRole,
        route: SessionRoute,
        mut wait: Option<WorkerLaunch>,
        mut managed: Option<(&mut crate::native_route_hold::Attempt, LaunchInterrupts)>,
    ) -> CoreResult<SessionDriver> {
        use crate::native_route_hold::{Admission, LIMITED};
        if route.launch.cwd != self.root
            || route.connection.is_empty()
            || route.connection_limit == 0
            || route.instructions.trim().is_empty()
            || route.model.is_empty()
            || route.deadline.is_zero()
        {
            return Err(CoreError::Refused("invalid_session_route"));
        }
        let engine_kind = route.launch.engine;
        let limit = route.connection_limit;
        let assignment = match &who {
            SessionRole::Worker { assignment } => Some(assignment.clone()),
            SessionRole::Coordinator => None,
        };
        if let Some(wait) = &wait {
            wait.ticket.as_ref().unwrap().select(
                &route.connection, managed.as_ref().map(|(attempt, _)| attempt.key()),
            );
        }
        let mut queued = false;
        let mut provider_wait: Option<String> = None;
        let hold_changed = &self.route_holds.changed;
        enum Decision<T> {
            Waiting,
            RecheckRoute,
            Reserved(T),
        }
        let admitted = loop {
            let changed = self.capacity_changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            let route_wake = hold_changed.notified();
            tokio::pin!(route_wake);
            route_wake.as_mut().enable();
            if let Some(error) = managed.as_ref().and_then(|(_, watches)| watches.interruption())
                .or_else(|| wait.as_ref().and_then(WorkerLaunch::interruption))
            {
                break Err(error);
            }
            let admission = managed.as_mut().map_or(Admission::Ready, |(attempt, _)| {
                let first = wait.as_ref().is_none_or(|w| {
                    w.ticket.as_ref().unwrap().first_for_route(attempt.key())
                });
                attempt.admit(first)
            });
            let (reason, until) = match admission {
                Admission::Ready => (None, None),
                Admission::Waiting { reason, until } => (Some(reason), until),
            };
            if let Some(wait) = &wait {
                // A provider-held route must not head-of-line block another model
                // sharing its connection. The ticket keeps its original position.
                wait.ticket.as_ref().unwrap().set_held(reason.is_some());
            }
            let wait_id = if reason.is_some() && provider_wait.is_none() {
                match crate::identity() {
                    Ok(id) => Some(id),
                    Err(_) => break Err(CoreError::Refused("native_wait_identity_unavailable")),
                }
            } else {
                provider_wait.clone()
            };
            let wait_id_for_tx = wait_id.clone();
            let already_waiting = provider_wait.is_some();
            let eligible = wait.as_ref().is_none_or(|w| {
                w.ticket.as_ref().unwrap().first_on(&route.connection)
            });
            let (task, owner, assignment, connection) =
                (task.clone(), owner.clone(), assignment.clone(), route.connection.clone());
            let interrupts = managed.as_ref().map(|(_, w)| w.clone())
                .or_else(|| wait.as_ref().map(|w| w.interrupts.clone()));
            let model = route.model.clone();
            let gate = managed.as_ref().map(|(attempt, _)| attempt.reservation_gate());
            let result = receive(self.executor.write(move |db| {
                let principal = match &assignment {
                    Some(assignment) => Principal::Worker { assignment },
                    None => Principal::Coordinator,
                };
                if let Some(interrupts) = &interrupts {
                    if let Some(error) = interrupts.interruption() {
                        return Err(error);
                    }
                    cortex_core::ownership::check_tx(db, &task, &owner)?;
                    if let Some(assignment) = assignment.as_deref() {
                        if !db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
                         WHERE n.task_id=? AND n.assignment_ref=? AND n.revision=t.revision
                         AND n.status='running' AND n.result_json IS NULL
                         AND ((t.state='EXECUTING' AND n.stage='execution') OR (t.state='REVIEWING' AND n.stage='review'))
                         AND NOT EXISTS(SELECT 1 FROM task_stopped WHERE task_id=n.task_id))",
                        (&task, assignment), |row| row.get::<_, bool>(0),
                    )? {
                            return Err(CoreError::Refused("native_task_interrupted"));
                        }
                    } else if !db.query_row(
                        "SELECT EXISTS(SELECT 1 FROM runtime_tasks t WHERE task_id=? AND state IN ('PLANNING','RECONCILING')
                         AND NOT EXISTS(SELECT 1 FROM task_stopped s WHERE s.task_id=t.task_id))",
                        [&task], |row| row.get::<_, bool>(0),
                    )? {
                        return Err(CoreError::Refused("native_task_interrupted"));
                    }
                }
                if let Some(reason) = reason {
                    if !already_waiting {
                        lifecycle::event_tx(db, &task, "NATIVE_PROVIDER_ROUTE_WAITING", json!({
                            "wait_id":wait_id_for_tx, "assignment_ref":assignment, "owner_epoch":owner.epoch,
                            "reason":reason, "connection_id":connection, "model":model
                        }))?;
                    }
                    return Ok(Decision::Waiting);
                }
                if let (Some(_), Some(assignment)) = (&interrupts, assignment.as_deref()) {
                    let available = scheduler::worker_capacity_available(db, &task, &connection, limit)?;
                    if !available {
                        // No channel, provisioning intent or process exists for a waiter.
                        // A wait reopens (visibly) after an earlier one of this assignment ended.
                        if !capacity_wait_open(db, &task, assignment)? {
                            lifecycle::event_tx(db, &task, "NATIVE_WORKER_CAPACITY_WAITING", json!({
                                "assignment_ref":assignment,"reason":"waiting_for_engine_capacity","connection_id":connection,"limit":limit
                            }))?;
                        }
                    }
                    if !available || !eligible {
                        return Ok(Decision::Waiting);
                    }
                }
                // A refusal may have landed since the async check. Re-enter the
                // wait loop with no channel or reservation if admission changed.
                let _route_admission = match &gate {
                    Some(gate) => match gate.lock_ready() {
                        Some(guard) => Some(guard),
                        None => return Ok(Decision::RecheckRoute),
                    },
                    None => None,
                };
                let credential = channels::issue_tx(db, &task, &owner, principal)?;
                let identity =
                    channels::authenticate(db, &credential.channel_id, credential.secret(), false)?;
                cortex_engine::schema::catalogue(
                    &identity.role,
                    identity.profile.as_deref(),
                    identity.stage.as_deref(),
                )
                .map_err(|_| CoreError::Refused("unknown_session_role"))?;
                let mut instructions = crate::native_instructions::compose(
                    identity.profile.as_deref().unwrap_or("orchestrator"),
                    identity.stage.as_deref(),
                    engine_kind,
                )?;
                let automatic = lifecycle::automatic::enabled(db, &task)?;
                if identity.role == "coordinator" {
                    // The no-stop policy holds for every task, not only Automatic ones.
                    instructions.push_str("\n\n");
                    instructions.push_str(lifecycle::automatic::COORDINATOR_NOTE);
                    if automatic {
                        instructions.push_str(" Have execution workers use Cortex project file tools for changes that must be included in the task's local finish.");
                    }
                } else if automatic && identity.stage.as_deref() == Some("execution") {
                    instructions.push_str("\n\nAutomatic sprint: make the task's final file changes through Cortex project file tools so the runtime retains attributable write receipts. Do not stage or commit changes yourself; automatic finish only commits changes it can attribute to this task.");
                }
                let reservation = scheduler::reserve_tx(
                    db,
                    &Reservation {
                        task: &task,
                        connection: &connection,
                        assignment: identity.assignment.as_deref(),
                        attempt: &identity.attempt,
                        kind: if identity.role == "worker" {
                            ReservationKind::Worker
                        } else {
                            ReservationKind::Coordinator
                        },
                        connection_limit: limit,
                    },
                )?
                .ok_or(CoreError::Refused("engine_capacity_exhausted"))?;
                // The visible wait ends in the same transaction that reserves the slot.
                if let (Some(_), Some(assignment)) = (&interrupts, assignment.as_deref()) {
                    if capacity_wait_open(db, &task, assignment)? {
                        lifecycle::event_tx(db, &task, "NATIVE_WORKER_CAPACITY_WAIT_ENDED", json!({
                            "assignment_ref":assignment,"outcome":"admitted","reservation_id":reservation
                        }))?;
                    }
                }
                if interrupts.is_some() {
                    // Also close an observational wait left by an earlier driver
                    // of this claim (for example on service restart).
                    let mut query = db.prepare(
                        "SELECT json_extract(e.payload_json,'$.wait_id') FROM runtime_events e
                         WHERE e.task_id=?1 AND e.type='NATIVE_PROVIDER_ROUTE_WAITING'
                         AND json_extract(e.payload_json,'$.assignment_ref') IS ?2
                         AND NOT EXISTS(SELECT 1 FROM runtime_events x WHERE x.task_id=e.task_id AND x.id>e.id
                           AND x.type='NATIVE_PROVIDER_ROUTE_WAIT_ENDED'
                           AND json_extract(x.payload_json,'$.wait_id')=json_extract(e.payload_json,'$.wait_id'))",
                    )?;
                    let waits = query.query_map((&task, &assignment), |row| row.get::<_, String>(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    for id in waits {
                        lifecycle::event_tx(db, &task, "NATIVE_PROVIDER_ROUTE_WAIT_ENDED", json!({
                            "wait_id":id, "outcome":"admitted", "reservation_id":reservation
                        }))?;
                    }
                }
                Ok(Decision::Reserved((credential, reservation, instructions, automatic)))
            })?)
            .await;
            match result {
                Ok(Decision::RecheckRoute) => continue,
                Ok(Decision::Waiting) if reason.is_some() => {
                    provider_wait = wait_id;
                    let (_, interrupts) = managed.as_mut().expect("provider hold is managed");
                    // Wake at expiry, on probe settlement, or on interruption. The
                    // same bounded recheck as local capacity catches durable-only changes.
                    let recheck = tokio::time::Instant::now() + Duration::from_secs(1);
                    tokio::select! {
                        _ = &mut route_wake => {},
                        _ = &mut changed => {},
                        _ = interrupts.changed() => {},
                        _ = tokio::time::sleep_until(until.map_or(recheck, |at| at.min(recheck))) => {},
                    }
                }
                Ok(Decision::Waiting) => {
                    queued = true;
                    let Some(wait) = &mut wait else {
                        unreachable!("unqueued dispatch never returns a wait");
                    };
                    tokio::select! {
                        _ = &mut changed => {},
                        _ = &mut route_wake => {},
                        _ = wait.interrupts.changed() => {},
                        // Reconcile release and authority from other store writers too.
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                    }
                }
                Ok(Decision::Reserved(admitted)) => break Ok(admitted),
                Err(error) => break Err(error),
            }
        };
        if let Some(wait) = &mut wait {
            // The transaction committed before the next contender can reserve.
            drop(wait.ticket.take());
        }
        if let (Err(error), true, Some(assignment)) = (&admitted, queued, assignment.clone()) {
            let (task, outcome) = (task.clone(), error.code().to_owned());
            let closed = async {
                receive(self.executor.write(move |db| {
                    if capacity_wait_open(db, &task, &assignment)? {
                        lifecycle::event_tx(db, &task, "NATIVE_WORKER_CAPACITY_WAIT_ENDED", json!({
                            "assignment_ref":assignment,"outcome":outcome
                        }))?;
                    }
                    Ok(())
                })?)
                .await
            }
            .await;
            // The interruption that ended the wait stays the reported cause; an
            // unrecorded end is only observational (the projection also requires a
            // live claim of the active stage of an unstopped task).
            if let Err(record) = closed {
                eprintln!(
                    "workspace-service: the end of an engine capacity wait was not recorded ({})",
                    record.code()
                );
            }
        }
        if let (Err(error), Some(id)) = (&admitted, provider_wait) {
            let (task, outcome) = (task.clone(), error.code());
            if let Err(record) = async {
                receive(self.executor.write(move |db| {
                    lifecycle::event_tx(db, &task, "NATIVE_PROVIDER_ROUTE_WAIT_ENDED", json!({
                        "wait_id":id, "outcome":outcome
                    }))?;
                    Ok(())
                })?).await
            }.await {
                eprintln!("workspace-service: provider wait end was not recorded ({})", record.code());
            }
        }
        let (credential, reservation, instructions, automatic) = admitted?;
        if let Some(error) = managed.as_ref().and_then(|(_, watches)| watches.interruption())
            .or_else(|| wait.as_ref().and_then(WorkerLaunch::interruption))
        {
            self.settle_unstarted(credential, reservation).await?;
            return Err(error);
        }
        let policy = Arc::new(CorePolicy {
            workspace_state: self.workspace_state.clone(),
            commands: self.commands.clone(),
            routes: self.routes.read().unwrap().clone(),
            permissions: self.permissions.read().unwrap().clone(),
            executor: self.executor.clone(),
            credential: credential.clone(),
            root: self.root.clone(),
            protected_roots: vec![
                route.launch.credential_directory.clone(),
                route
                    .launch
                    .executable
                    .parent()
                    .unwrap_or(&route.launch.executable)
                    .to_path_buf(),
            ],
            engine: engine_kind,
            mcp_servers: std::sync::Mutex::new(Vec::new()),
            activity: Default::default(),
        });
        let credentials = || {
            Credentials::new(
                credential.channel_id.clone(),
                self.root.clone(),
                credential.secret().into(),
            )
        };
        credentials().map_err(|_| CoreError::Refused("invalid_channel"))?;
        let external_core = Arc::new(crate::native_mcp::CorePort {
            executor: self.executor.clone(),
            credential: credential.clone(),
            root: self.root.clone(),
            registry: self.mcp.clone(),
            engine: engine_kind,
        });
        let c = credential.clone();
        let authority = receive(self.executor.read(move |db| {
            let i = channels::authenticate(db, &c.channel_id, c.secret(), false)?;
            let scopes = if let Some(a) = &i.assignment {
                let raw: String = db.query_row(
                    "SELECT scope_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
                    (&i.task, a),
                    |r| r.get(0),
                )?;
                serde_json::from_str::<Vec<PathBuf>>(&raw)?
            } else {
                vec![]
            };
            Ok((view(&i), scopes))
        })?)
        .await;
        let (identity, scopes) = match authority {
            Ok(authority) => authority,
            Err(error) => {
                self.settle_unstarted(credential, reservation).await?;
                if let Some(interrupted) = wait.as_ref().and_then(WorkerLaunch::interruption) {
                    return Err(interrupted);
                }
                return Err(error);
            }
        };
        if let Some(error) = wait.as_ref().and_then(WorkerLaunch::interruption) {
            self.settle_unstarted(credential, reservation).await?;
            return Err(error);
        }
        // The owner-facing stop names the role whose session could not start.
        let blocker = if identity.role == "coordinator" {
            "coordinator_session_start_failed"
        } else {
            "native_session_start_failed"
        };
        let instructions = format!("{instructions}\n\n{}", route.instructions);
        let project_context = crate::native_skills::project_files(
            &identity.role,
            identity.profile.as_deref(),
            identity.stage.as_deref(),
        )
        .then(|| crate::native_skills::section(&self.root))
        .flatten()
        .unwrap_or_default();
        let mut attempt: u32 = 0;
        let (session, engine, broker, external) = loop {
            attempt += 1;
            let mut broker = None;
            let mut external = Vec::new();
            let mut withheld = Vec::new();
            let mut withheld_recorded = false;
            let mut engine: Option<Engine> = None;
            let mut phase = "broker";
            // The session is bound to its channel only after it is configured: until
            // then no MCP call can run (tools need a bound session), so a start that
            // fails before binding has had no effect and may start once more.
            let mut bound = false;
            let started = async {
                let gateway = Gateway::new(policy.clone(), credentials().map_err(|_| Error::Invalid)?);
                broker = Some(Broker::new(&route.broker_parent, Arc::new(gateway))?);
                phase = "external_mcp";
                self.mcp
                    .attach(
                        &identity,
                        engine_kind,
                        &self.root,
                        &scopes,
                        &route.broker_parent,
                        &route.relay,
                        credentials,
                        external_core.clone(),
                        &mut external,
                        &mut withheld,
                    )
                    .await?;
                phase = "launch";
                let mut launch = route.launch.clone();
                // Through Cortex's native Codex launcher the relays are App Server config
                // layers, not ACP servers: an ACP server list would replace the layer that
                // turns the owner's own Codex MCP servers off (see `CodexLauncher`).
                let by_launcher = match launch.codex_launcher.take() {
                    Some(launcher) => {
                        let mut relays = vec![broker.as_ref().unwrap().entry("cortex", &route.relay)?];
                        relays.extend(external.iter().map(|a| a.entry()));
                        launch.codex_launcher = Some(launcher.with_mcp_servers(relays));
                        true
                    }
                    None => false,
                };
                // A confined engine reaches its relays' brokers only through these
                // directories, shown read-only in its view; nothing else of the store.
                launch.expose(
                    std::iter::once(broker.as_ref().unwrap().directory().to_path_buf())
                        .chain(external.iter().filter_map(|a| a.broker_directory())),
                );
                let instruction_file = (engine_kind == cortex_engine::environment::EngineKind::Codex)
                    .then(|| broker.as_ref().unwrap().write_model_instructions(&instructions)).transpose()?;
                engine = Some(Engine::launch_with_model_instructions(launch, policy.clone(), instruction_file.as_deref()).await?);
                let e = engine.as_ref().unwrap();
                let b = broker.as_ref().unwrap();
                phase = "broker_bind";
                b.bind_engine(e.pid())?;
                for a in &external {
                    a.bind(e.pid())?;
                }
                phase = "initialize";
                e.initialize(route.deadline).await?;
                // An optional server that already exited (for example, one refused by
                // its own startup checks inside the containment) is withheld rather
                // than handed to the engine as a dead relay.
                phase = "external_mcp";
                let mut index = 0;
                while index < external.len() {
                    if external[index].exited() {
                        let a = external.remove(index);
                        let server = a.name().unwrap_or_default();
                        a.shutdown().await?;
                        withheld.push(crate::native_mcp::Withheld {
                            server,
                            reason: "server_exited_before_session",
                        });
                    } else {
                        index += 1;
                    }
                }
                *policy.mcp_servers.lock().unwrap() =
                    external.iter().filter_map(|a| a.name()).collect();
                phase = "session_new";
                let servers = if by_launcher {
                    vec![]
                } else {
                    let mut servers = vec![b.entry("cortex", &route.relay)?];
                    servers.extend(external.iter().map(|a| a.entry()));
                    servers
                };
                // Every Claude session starts without native shell/edit/subagent
                // tools, bypass, hooks, plugins, skills or other MCP configuration.
                let meta = (engine_kind == cortex_engine::environment::EngineKind::Claude)
                    .then(|| {
                        let mut meta = crate::native_engine_policy::claude_session_meta();
                        meta["systemPrompt"] = json!(&instructions);
                        meta
                    });
                let s = e
                    .start_session_with_meta(&self.root, servers, meta, route.deadline)
                    .await?;
                phase = "configure";
                e.configure(&s, "model", &route.model, route.deadline)
                    .await?;
                for (key, value) in &route.parameters {
                    if key != "mode" { e.configure(&s, key, value, route.deadline).await?; }
                }
                phase = "access_mode";
                let c = credential.clone();
                let (access,read_only) = receive(self.executor.read(move |db| {
                    let i = channels::authenticate(db,&c.channel_id,c.secret(),false)?;
                    Ok((cortex_core::access::current(db,&i.task)?,crate::native_engine_policy::read_only(&i)))
                }).map_err(engine_error)?).await.map_err(engine_error)?;
                // The engine mode depends on the channel's capability, never on the
                // access mode: no session runs in a mode that skips the engine's
                // permission requests. Codex follows the same routes as the other
                // engines (owner decision 2026-11-22).
                let mode = crate::native_engine_policy::native_mode(
                    engine_kind, read_only, &s.config_options())?;
                e.configure(&s,"mode",mode,route.deadline).await?;
                if engine_kind == cortex_engine::environment::EngineKind::Codex
                    && !crate::native_engine_policy::codex_native_confirmed(&s.config_options(), mode) {
                    return Err(Error::Refused("restricted_mode_unavailable"));
                }
                phase = "bind_session";
                let c = credential.clone();
                let sid = s.id().to_owned();
                bound = true;
                receive(
                    self.executor
                        .write(move |db| channels::bind_session_tx(db, &c, &sid))
                        .map_err(engine_error)?,
                )
                .await
                .map_err(engine_error)?;
                record_withheld(&self.executor, &credential, &withheld)
                    .await
                    .map_err(engine_error)?;
                withheld_recorded = true;
                // Display-only: the durable event above is the record, so a missing
                // display binding never stops the session.
                let _ = note_withheld(&self.executor, &credential, s.id(), &withheld).await;
                phase = "access_mode";
                let c = credential.clone();
                let selected = mode.to_owned();
                // The engine and model this session runs on, as the engine confirmed them in
                // `configure` (each reply must echo the exact value). The desktop shows a
                // worker's route from this record only, so shown and used cannot differ.
                let model_label = model_label(&s.config_options(), &route.model);
                let mut configured = json!({"channel_id":c.channel_id,"session":s.id(),
                    "engine":match engine_kind {
                        cortex_engine::environment::EngineKind::Codex => "codex-acp",
                        cortex_engine::environment::EngineKind::Claude => "claude-agent-acp",
                        cortex_engine::environment::EngineKind::OpenCode => "opencode",
                    },
                    "model":route.model,
                    "parameters":route.parameters.iter().filter(|(k,_)|k.as_str()!="mode").collect::<std::collections::BTreeMap<_,_>>()});
                // The engine's name for the model (an alias such as `default` reads with the
                // model it resolves to); absent when the engine names nothing better than the id.
                if let Some(label) = model_label { configured["model_label"] = json!(label); }
                let s_id = s.id().to_owned();
                receive(self.executor.write(move |db| {
                    let i = channels::authenticate(db,&c.channel_id,c.secret(),false)?;
                    lifecycle::event_tx(db,&i.task,"NATIVE_ACCESS_MODE_CONFIRMED",
                        json!({"channel_id":c.channel_id,"mode":selected,"access":access,"session":i.session}))?;
                    configured["engine_session_ref"] = json!(format!("native:{}:{}", i.attempt, s_id));
                    lifecycle::event_tx(db,&i.task,"NATIVE_SESSION_CONFIGURED",configured)?;
                    Ok(())
                }).map_err(engine_error)?).await.map_err(engine_error)?;
                let c = credential.clone();
                receive(self.executor.write(move|db| {
                    let i=channels::authenticate(db,&c.channel_id,c.secret(),true)?;
                    let replaced:bool=db.query_row("SELECT count(*)>1 FROM delegation_provisioning WHERE task_id=? AND role_id='orchestrator' AND engine_session_ref IS NOT NULL",[&i.task],|r|r.get(0))?;
                    cortex_core::recovery_context::generation_tx(db,&c,if i.role=="coordinator" && replaced {"session_replacement"} else {"session_open"},None)?;
                    Ok(())
                }).map_err(engine_error)?).await.map_err(engine_error)?;
                Ok::<_, Error>(s)
            }
            .await;
            let error = match started {
                Ok(s) => break (s, engine.unwrap(), broker.unwrap(), external),
                Err(error) => error,
            };
            // Only a completed owned wait releases capacity. Unknown effects
            // stay reserved/ambiguous; there is no automatic launch replay.
            // `Engine::launch` fails with `invalid_arguments` or
            // `authority_refused` only before it spawns (policy, launcher
            // preparation, launch environment); every post-spawn failure is
            // `io_error`. No process ever existed, so its absence is
            // trivially confirmed and recovery must not wait on it. A
            // `broker` failure happens before any process (engine or
            // external server) is started.
            let never_spawned = engine.is_none()
                && (phase == "broker"
                    || (phase == "launch" && matches!(error, Error::Invalid | Error::Forbidden)));
            let diagnostics = engine.as_ref().map(Engine::diagnostics);
            let mut observation = if let Some(e) = engine.take() {
                if e.shutdown().await.is_ok() {
                    ProcessObservation::ConfirmedExited
                } else {
                    ProcessObservation::Unknown
                }
            } else if never_spawned {
                ProcessObservation::ConfirmedExited
            } else {
                ProcessObservation::Unknown
            };
            let broker_settled = if let Some(b) = broker.take() {
                b.shutdown().await.is_ok()
            } else {
                true
            };
            for a in external {
                if a.shutdown().await.is_err() {
                    observation = ProcessObservation::Unknown;
                }
            }
            // Per-server failures are withheld, not errors: an external_mcp
            // error now means a launched server's exit was not confirmed.
            if phase == "external_mcp" {
                observation = ProcessObservation::Unknown;
            }
            // One bounded retry: a transport that closed while configuring an unbound
            // session whose process (and relays) confirmably exited. A fresh process,
            // broker and relays start under the same channel; both attempts are recorded.
            let retry = attempt == 1
                && !bound
                && matches!(error, Error::Closed | Error::Io)
                && matches!(phase, "configure" | "access_mode")
                && observation == ProcessObservation::ConfirmedExited;
            // The catalogue producer recognizes this same ACP authentication-required
            // response during session discovery. Unclassified transport failures keep
            // their generic startup diagnostic; no inference from engine text.
            let explicit_auth =
                automatic && phase == "session_new" && matches!(error, Error::Rpc(-32000));
            let blocker = if automatic && observation != ProcessObservation::ConfirmedExited {
                "native_session_outcome_unknown"
            } else if explicit_auth {
                "engine_authentication_required"
            } else {
                blocker
            };
            let provider_limit = (managed.is_some() && observation == ProcessObservation::ConfirmedExited
                && broker_settled && !bound && phase == "session_new")
                .then(|| error.provider_limit()).flatten();
            let failure = StartFailure {
                attempt,
                phase,
                // A refusal with a stable code (`restricted_mode_unavailable`, ...)
                // records that code; it is static and secret-free.
                error: match &error {
                    _ if provider_limit.is_some() => LIMITED.into(),
                    Error::Refused(code) => (*code).to_owned(),
                    other => other.to_string(),
                },
                role: identity.role.clone(),
                engine: engine_kind.name(),
                blocker: (!retry && provider_limit.is_none()).then_some(blocker),
                diagnostics: diagnostics.filter(|_| provider_limit.is_none()).map(|d| d.snapshot()),
                withheld: if withheld_recorded {
                    vec![]
                } else {
                    withheld
                        .iter()
                        .map(|w| (w.server.clone(), w.reason))
                        .collect()
                },
            };
            if retry {
                record_start_failure(&self.executor, &credential, failure).await?;
                continue;
            }
            if let (Some((attempt, _)), Some(limit)) = (&mut managed, provider_limit) {
                attempt.limited(limit, credential.channel_id.clone());
            }
            let recorded = record_start_failure(&self.executor, &credential, failure).await;
            settle(&self.executor, credential.clone(), reservation, observation, provider_limit).await?;
            if observation == ProcessObservation::ConfirmedExited {
                self.capacity_changed.notify_waiters();
            }
            recorded?;
            if explicit_auth && observation == ProcessObservation::ConfirmedExited {
                let t = identity.task.clone();
                let connection = route.connection.clone();
                receive(self.executor.write(move |db| {
                    lifecycle::event_tx(
                        db,
                        &t,
                        "NATIVE_AUTOMATIC_AUTH_BLOCKED",
                        json!({"connection":connection}),
                    )?;
                    Ok(())
                })?)
                .await?;
            }
            if provider_limit.is_some() {
                return Err(CoreError::Refused(LIMITED));
            }
            return Err(CoreError::Refused(blocker));
        };
        Ok(SessionDriver {
            executor: self.executor.clone(),
            capacity_changed: self.capacity_changed.clone(),
            credential,
            reservation,
            engine,
            broker,
            external,
            session,
            instructions,
            project_context,
            engine_kind,
            policy,
            delivered_attachments: std::sync::Mutex::new(std::collections::HashSet::new()),
            refusal_is_degenerate: std::sync::atomic::AtomicBool::new(false),
            dispatched_prompts: std::sync::atomic::AtomicI64::new(0),
            provider_refusal: Default::default(),
        })
    }

    async fn settle_unstarted(
        &self,
        credential: Credential,
        reservation: String,
    ) -> CoreResult<()> {
        settle(&self.executor, credential, reservation, ProcessObservation::ConfirmedExited, None).await?;
        self.capacity_changed.notify_waiters();
        Ok(())
    }
}

/// Whether this assignment's latest engine capacity wait has not ended yet.
fn capacity_wait_open(db: &rusqlite::Connection, task: &str, assignment: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=?1
         AND e.type='NATIVE_WORKER_CAPACITY_WAITING' AND json_extract(e.payload_json,'$.assignment_ref')=?2
         AND NOT EXISTS(SELECT 1 FROM runtime_events x WHERE x.task_id=e.task_id AND x.id>e.id
            AND x.type='NATIVE_WORKER_CAPACITY_WAIT_ENDED' AND json_extract(x.payload_json,'$.assignment_ref')=?2))",
        (task, assignment),
        |row| row.get(0),
    )?)
}

/// A batch registers claims before polling any of their futures. Unresolved
/// predecessors can be on this connection; a known different route never holds it.
#[derive(Default)]
pub(super) struct WorkerLaunchOrder {
    queue: std::sync::Mutex<std::collections::VecDeque<(String, Option<LaunchSelection>)>>,
}

struct LaunchSelection {
    connection: String,
    route: Option<crate::native_route_hold::RouteKey>,
    held: bool,
}
impl WorkerLaunchOrder {
    pub(super) fn push(
        self: &Arc<Self>,
        assignment: String,
        changed: Arc<tokio::sync::Notify>,
    ) -> LaunchTicket {
        self.queue
            .lock()
            .unwrap()
            .push_back((assignment.clone(), None));
        LaunchTicket {
            order: self.clone(),
            assignment,
            changed,
        }
    }
}
pub(super) struct LaunchTicket {
    order: Arc<WorkerLaunchOrder>,
    assignment: String,
    changed: Arc<tokio::sync::Notify>,
}
impl LaunchTicket {
    pub(super) fn order(&self) -> Arc<WorkerLaunchOrder> {
        self.order.clone()
    }

    fn select(&self, connection: &str, route: Option<&crate::native_route_hold::RouteKey>) {
        let mut queue = self.order.queue.lock().unwrap();
        let (_, selection) = queue.iter_mut().find(|(a, _)| a == &self.assignment).unwrap();
        *selection = Some(LaunchSelection { connection: connection.into(), route: route.cloned(), held: false });
        self.changed.notify_waiters();
    }

    fn set_held(&self, held: bool) {
        let mut queue = self.order.queue.lock().unwrap();
        let (_, selection) = queue.iter_mut().find(|(a, _)| a == &self.assignment).unwrap();
        let selection = selection.as_mut().unwrap();
        if selection.held != held {
            selection.held = held;
            self.changed.notify_waiters();
        }
    }

    fn first_for_route(&self, route: &crate::native_route_hold::RouteKey) -> bool {
        for (assignment, selection) in self.order.queue.lock().unwrap().iter() {
            if assignment == &self.assignment {
                return true;
            }
            if selection.as_ref().is_none_or(|s| s.route.as_ref() == Some(route)) {
                return false;
            }
        }
        false
    }
    fn first_on(&self, connection: &str) -> bool {
        for (assignment, route) in self.order.queue.lock().unwrap().iter() {
            if assignment == &self.assignment {
                return true;
            }
            if route.as_ref().is_none_or(|s| !s.held && s.connection == connection) {
                return false;
            }
        }
        false
    }
}
impl Drop for LaunchTicket {
    fn drop(&mut self) {
        self.order
            .queue
            .lock()
            .unwrap()
            .retain(|(a, _)| a != &self.assignment);
        self.changed.notify_waiters();
    }
}
type StopWatch = tokio::sync::watch::Receiver<bool>;

#[derive(Clone)]
pub(super) struct LaunchInterrupts {
    pub service: StopWatch,
    pub batch: StopWatch,
    pub worker: StopWatch,
}
impl LaunchInterrupts {
    fn interruption(&self) -> Option<CoreError> {
        let code = if *self.service.borrow() || self.service.has_changed().is_err() {
            "native_service_stopped"
        } else if *self.batch.borrow() || self.batch.has_changed().is_err() {
            "native_task_interrupted"
        } else if *self.worker.borrow() || self.worker.has_changed().is_err() {
            "native_worker_stopped_before_launch"
        } else {
            return None;
        };
        Some(CoreError::Refused(code))
    }

    async fn changed(&mut self) {
        tokio::select! {
            _ = self.service.changed() => {},
            _ = self.batch.changed() => {},
            _ = self.worker.changed() => {},
        }
    }
}
pub(super) struct WorkerLaunch {
    ticket: Option<LaunchTicket>,
    interrupts: LaunchInterrupts,
}
impl WorkerLaunch {
    pub(super) fn new(
        ticket: LaunchTicket,
        service: StopWatch,
        batch: StopWatch,
        worker: StopWatch,
    ) -> Self {
        Self {
            ticket: Some(ticket),
            interrupts: LaunchInterrupts {
                service,
                batch,
                worker,
            },
        }
    }

    fn interruption(&self) -> Option<CoreError> {
        self.interrupts.interruption()
    }
}
#[derive(Clone)]
pub enum SessionRole {
    Coordinator,
    Worker { assignment: String },
}
#[derive(Clone)]
pub struct SessionRoute {
    pub launch: LaunchSpec,
    pub relay: PathBuf,
    pub broker_parent: PathBuf,
    pub connection: String,
    pub connection_limit: u16,
    pub model: String,
    pub parameters: std::collections::BTreeMap<String, String>,
    pub instructions: String,
    pub deadline: Duration,
}
pub struct Turn {
    pub stop_reason: String,
    pub updates: usize,
    pub compacted: bool,
}
/// An open writing-worker turn: its identity, its start event and the checkout's
/// dirty paths before the prompt (`SessionDriver::open_writer_turn`).
struct WriterTurn {
    turn: String,
    since: i64,
    before: crate::native_change_set::Snapshot,
}
/// How often an active worker prompt checks whether its authority was revoked.
const REVOCATION_POLL: Duration = Duration::from_secs(1);
/// How long a revoked worker turn may keep running to end by itself (its closing
/// reply after its own accepted replan or escalation) before it is cancelled.
const REVOKED_FINISH_GRACE: Duration = Duration::from_secs(10);
/// How long a revoked worker's engine has to answer `session/cancel` with its prompt
/// reply before the turn ends as outcome-unknown and the session is terminated. ACP
/// agents reply once they have aborted in-flight tool calls, normally within seconds;
/// the margin covers a slow abort while staying far below a model's own run time.
const REVOKED_CANCEL_GRACE: Duration = Duration::from_secs(25);
pub(crate) enum SessionWait {
    Open(Option<Duration>),
    Ready,
    None,
}

pub struct SessionDriver {
    executor: Arc<StoreExecutor>,
    capacity_changed: Arc<tokio::sync::Notify>,
    credential: Credential,
    reservation: String,
    engine: Engine,
    broker: Broker,
    external: Vec<crate::native_mcp::Attachment>,
    session: Session,
    instructions: String,
    project_context: String,
    engine_kind: cortex_engine::environment::EngineKind,
    policy: Arc<CorePolicy>,
    /// Retained attachment bodies already delivered to this engine session.
    delivered_attachments: std::sync::Mutex<std::collections::HashSet<String>>,
    /// Whether an ACP `refusal` stop ends a turn as degenerate (set for runtime-driven
    /// turns; see [`SessionDriver::treat_refusal_as_degenerate`]).
    refusal_is_degenerate: std::sync::atomic::AtomicBool,
    dispatched_prompts: std::sync::atomic::AtomicI64,
    provider_refusal: std::sync::Mutex<Option<cortex_engine::ProviderLimit>>,
}

/// A dispatched prompt is banked once, even when an outer runtime select drops
/// the prompt future. Queue on drop, before any subsequent session shutdown
/// write, without awaiting the accounting transaction on the turn's path.
struct PromptUsage<'a> {
    executor: &'a StoreExecutor,
    runtime: tokio::runtime::Handle,
    task: String,
    attempt: String,
    session: String,
    call: std::sync::OnceLock<(String, i64)>,
    reason: Option<String>,
    tokens: Option<Value>,
    failed: bool,
}

impl Drop for PromptUsage<'_> {
    fn drop(&mut self) {
        let Some((id, sequence)) = self.call.take() else {
            return;
        };
        let (task, attempt, session) = (
            std::mem::take(&mut self.task),
            std::mem::take(&mut self.attempt),
            std::mem::take(&mut self.session),
        );
        let reason = self.reason.take();
        let (basis, tokens) = match self.tokens.take() {
            Some(tokens) => ("engine_reported", tokens.to_string()),
            None => ("unknown", "{}".into()),
        };
        let failed = self.failed;
        match self.executor.write(move |db| {
            db.execute(
                "INSERT INTO usage_records(id,task_id,attempt_id,engine_session_ref,
                 call_sequence,basis,tokens_json,stop_reason,failed,created_at)
                 VALUES (?,?,?,?,?,?,?,?,?,?)",
                (
                    id,
                    task,
                    attempt,
                    session,
                    sequence,
                    basis,
                    tokens,
                    reason,
                    failed,
                    cortex_core::now(),
                ),
            )?;
            Ok(())
        }) {
            Ok(receipt) => {
                self.runtime.spawn(async move {
                    if let Err(error) = receive(receipt).await {
                        usage_write_diagnostic(&error);
                    }
                });
            }
            Err(error) => usage_write_diagnostic(&error),
        }
    }
}

fn usage_write_diagnostic(error: &CoreError) {
    use std::io::Write as _;
    let _ = writeln!(
        std::io::stderr(),
        "workspace-service: prompt usage was not recorded ({})",
        error.code()
    );
}

impl SessionDriver {
    fn root_for_input(&self) -> PathBuf {
        let channel = workspace_protocol::channel::current();
        self.policy.root.join(if channel.is_dev() {
            ".cortex-dev"
        } else {
            ".cortex"
        })
    }
    pub fn session_id(&self) -> &str {
        self.session.id()
    }
    fn provider_refusal(&self) -> Option<(
        cortex_engine::ProviderLimit, Arc<std::sync::atomic::AtomicBool>,
    )> {
        self.provider_refusal.lock().unwrap().map(|limit| (limit, self.policy.activity.clone()))
    }

    /// Drain ACP updates concurrently. Chat/end_turn never become tool authority.
    pub async fn prompt(&self, text: &str) -> CoreResult<Turn> {
        self.policy
            .check(true)
            .await
            .map_err(|_| CoreError::Refused("channel_authority_refused"))?;
        let c = self.credential.clone();
        let (delivery, capsule) = receive(
            self.executor
                .write(move |db| cortex_core::recovery_context::delivery_tx(db, &c))?,
        )
        .await?;
        let mut prompt = format!("{}\n\n{capsule}\n\n{text}", self.project_context);
        if self.engine_kind == cortex_engine::environment::EngineKind::OpenCode {
            prompt = format!("{}\n\n{prompt}", self.instructions);
        }
        let identity = self
            .policy
            .check(true)
            .await
            .map_err(engine_error_to_core)?;
        let worker = identity.role == "worker";
        let writer = worker && !crate::native_engine_policy::read_only(&identity);
        let mut usage = PromptUsage {
            executor: &self.executor,
            runtime: tokio::runtime::Handle::current(),
            session: format!("native:{}:{}", identity.attempt, self.session.id()),
            task: identity.task.clone(),
            attempt: identity.attempt,
            call: std::sync::OnceLock::new(),
            reason: None,
            tokens: None,
            failed: true,
        };
        let task = identity.task;
        let root = self.root_for_input();
        let stored = receive(self.executor.read(move|db|{
            use std::io::Read;
            let mut q=db.prepare("SELECT b.kind,b.mime,b.uri,b.sha256,b.size_bytes,b.body_ref,b.name FROM source_blocks b
                JOIN source_revisions r ON r.report_id=b.report_id WHERE b.task_id=? AND b.availability='stored'
                ORDER BY r.revision,b.block_index")?;
            let rows=q.query_map([&task],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,
                r.get::<_,Option<String>>(2)?,r.get::<_,String>(3)?,r.get::<_,i64>(4)?,r.get::<_,String>(5)?,r.get::<_,Option<String>>(6)?)))?
                .collect::<Result<Vec<_>,_>>()?;
            let mut content=Vec::new();let mut total=0usize;
            for (kind,mime,uri,hash,size,name,label) in rows {
                if name.contains('/')||name=="."||name==".."||size<0 {return Err(CoreError::Refused("input_body_invalid"));}
                let path=root.join(&task).join(".source_blocks").join(&name);
                let file=cortex_core::store::locks::open_private(&path,false,true)?;
                let mut bytes=Vec::new();
                file.take((cortex_core::input::MAX_BYTES+1) as u64).read_to_end(&mut bytes)?;
                total+=bytes.len();
                use sha2::{Digest,Sha256};
                if total>cortex_core::input::MAX_BYTES||bytes.len()!=size as usize||format!("{:x}",Sha256::digest(&bytes))!=hash {
                    return Err(CoreError::Refused("input_body_invalid"));
                }
                content.push(crate::native_attachments::Stored{kind,mime,uri,name:label,sha256:hash,key:name,bytes});
            }
            Ok(content)
        })?).await?;
        let caps = self.engine.prompt_capabilities();
        let (attachments, fresh) = {
            let sent = self.delivered_attachments.lock().unwrap();
            crate::native_attachments::prompt_blocks(&stored, &sent, caps, self.engine_kind)
        };
        let mut blocks = vec![json!({"type":"text","text":prompt})];
        blocks.extend(attachments);
        let c = self.credential.clone();
        let transmitted = delivery.clone();
        let (bundle, brief) = (self.instructions.clone(), text.to_owned());
        receive(self.executor.write(move|db|{
            let i=channels::authenticate(db,&c.channel_id,c.secret(),true)?;
            let session=format!("native:{}:{}",i.attempt,i.session.as_deref().unwrap());
            db.execute("UPDATE capsule_deliveries SET state='continuation_observed',updated_at=?
                WHERE engine_session_ref=? AND state='delivery_confirmed'",(cortex_core::now(),&session))?;
            db.execute("UPDATE capsule_deliveries SET state='transmitted',updated_at=? WHERE id=? AND state='built'",(cortex_core::now(),&transmitted))?;
            // What this prompt gave the engine stays verifiable afterwards (digests only).
            let tools=cortex_engine::schema::catalogue(&i.role,i.profile.as_deref(),i.stage.as_deref()).unwrap_or_default();
            let catalogue=crate::native_project::digest(&serde_json::to_vec(&tools)?);
            cortex_core::recovery_context::record_delivery_tx(db,&c,&transmitted,&bundle,&capsule,&brief,&catalogue)?;
            Ok(())
        })?).await?;
        // A writing worker's engine may change files without a receipt: its turn is
        // bracketed by dirty-path listings (`native_change_set`).
        let window = if writer {
            Some(self.open_writer_turn().await?)
        } else {
            None
        };
        let call = &usage.call;
        let session_ref = &usage.session;
        let pending = self
            .engine
            .prompt_content_observed(&self.session, blocks, || {
                let sequence = self
                    .dispatched_prompts
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                    + 1;
                let id = format!(
                    "{}{session_ref}:{sequence}",
                    crate::notifier::progress::NATIVE_USAGE_PREFIX,
                );
                let _ = call.set((id, sequence));
            });
        tokio::pin!(pending);
        let mut updates = 0;
        let mut compacted = false;
        // A worker turn whose authority the runtime has revoked can do nothing more,
        // yet the engine may keep generating for minutes while the attempt's capacity
        // stays reserved and recovery waits for it. Once the revocation is observed on
        // an active prompt, ask the engine to cancel; a prompt that does not reply
        // within the grace ends here with its outcome unknown, and the caller's
        // shutdown terminates and settles the session process.
        let revocation = tokio::time::sleep(REVOCATION_POLL);
        tokio::pin!(revocation);
        let mut cancel_sent = false;
        let mut revoked_since: Option<tokio::time::Instant> = None;
        // The degenerate-turn watchdog (see `degenerate`) reads every agent message of
        // this turn as it streams. A trip records which rule fired, cancels the turn
        // through the same bounded cancel-and-grace path as a revoked worker, and keeps
        // the rest of the degenerate text out of the conversation.
        let mut watch = degenerate::Watch::default();
        let mut tripped = false;
        let result = loop {
            tokio::select! {
                result=&mut pending=>break result,
                event=self.engine.next_event()=>{
                    match event {
                        Some(Event::Update(params))=>{
                            updates+=1;
                            if !tripped && params["sessionId"] == self.session.id() {
                                if let Some(trip) = watch.observe(&params["update"]) {
                                    tripped = true;
                                    self.record_degenerate(&trip, worker).await?;
                                    if self.prompt_active() {
                                        let _ = self.engine.cancel(&self.session).await;
                                    }
                                    cancel_sent = true;
                                    revocation.as_mut().reset(tokio::time::Instant::now() + REVOKED_CANCEL_GRACE);
                                    continue;
                                }
                            }
                            if tripped && degenerate::agent_text(&params["update"]) {
                                continue;
                            }
                            compacted |= self.record_update(params).await?;
                        }
                        Some(Event::TransportClosed)|None=>break Err(Error::Closed),
                        _=>{}
                    }
                }
                _=&mut revocation, if worker || cancel_sent=>{
                    if cancel_sent {
                        break Err(Error::Closed);
                    }
                    // Cancelling before the prompt is active would poison the session.
                    // A revoked turn first gets a short chance to end by itself: the
                    // revocation is often the turn's own accepted request (a replan or
                    // escalation), and its closing reply should not be cut off.
                    let next = if self.prompt_active() && self.authority_revoked().await {
                        let since = *revoked_since.get_or_insert_with(tokio::time::Instant::now);
                        if since.elapsed() >= REVOKED_FINISH_GRACE {
                            let _ = self.engine.cancel(&self.session).await;
                            cancel_sent = true;
                            REVOKED_CANCEL_GRACE
                        } else {
                            REVOCATION_POLL
                        }
                    } else {
                        REVOCATION_POLL
                    };
                    revocation.as_mut().reset(tokio::time::Instant::now() + next);
                }
            }
        };
        if let Ok(reply) = &result {
            usage.reason = reply["stopReason"].as_str().map(str::to_owned);
            usage.tokens = crate::notifier::progress::prompt_reported_tokens(&reply["usage"]);
        }
        // The prompt reply may win select while earlier notifications remain
        // queued. Persist those too; never count a reply as activity evidence.
        for event in self.engine.drain_events().await {
            if let Event::Update(params) = event {
                updates += 1;
                if !tripped && params["sessionId"] == self.session.id() {
                    if let Some(trip) = watch.observe(&params["update"]) {
                        tripped = true;
                        self.record_degenerate(&trip, worker).await?;
                        continue;
                    }
                }
                if tripped && degenerate::agent_text(&params["update"]) {
                    continue;
                }
                compacted |= self.record_update(params).await?;
            }
        }
        // A degenerate turn is never settled as an ordinary reply, whether the engine
        // answered the cancel or the grace ran out; the caller retries it once in a
        // fresh session (see `native_runtime`).
        // A turn that did not end normally may still act after this point: its
        // change record is incomplete.
        self.settle_writer_turn(window.as_ref(), !tripped && result.is_ok())
            .await?;
        if tripped {
            return Err(CoreError::Refused("native_turn_degenerate"));
        }
        if let Some(limit) = result.as_ref().err().and_then(Error::provider_limit_before_work) {
            use std::sync::atomic::Ordering::SeqCst;
            // Replaying a prompt after output or effects could repeat an action.
            // Only the first direct refusal, without native/gateway activity or
            // attached external servers, is eligible to start fresh.
            if self.dispatched_prompts.load(SeqCst) == 1
                && !self.policy.activity.load(SeqCst)
                && self.external.is_empty()
            {
                *self.provider_refusal.lock().unwrap() = Some(limit);
            }
        }
        let result = result.map_err(|_| CoreError::Refused("native_turn_outcome_unknown"))?;
        self.delivered_attachments.lock().unwrap().extend(fresh);
        receive(self.executor.write(move|db| {
            db.execute("UPDATE capsule_deliveries SET state='delivery_confirmed',updated_at=? WHERE id=? AND state='transmitted'",(cortex_core::now(),&delivery))?;
            Ok(())
        })?).await?;
        let reason = result["stopReason"].as_str().unwrap().to_owned();
        let credential = self.credential.clone();
        let recorded = reason.clone();
        let usage_id = usage.call.get().map(|(id, _)| id.clone());
        receive(self.executor.write(move |db| {
            // A valid tool may already have advanced or revoked this channel.
            let task: String = db.query_row(
                "SELECT task_id FROM trusted_channels WHERE channel_id=?",
                [&credential.channel_id],
                |r| r.get(0),
            )?;
            // `writer` says whether the turn kept a change record
            // (`NATIVE_WRITER_TURN_CHANGES`).
            lifecycle::event_tx(
                db,
                &task,
                "NATIVE_TURN_SETTLED",
                json!({"channel_id":credential.channel_id,"stop_reason":recorded,"writer":writer,"usage_id":usage_id}),
            )?;
            Ok(())
        })?)
        .await?;
        // An ACP `refusal` stop is the model declining the turn (claude-agent-acp reports
        // a refused turn with it). In a runtime-driven turn it counts as degenerate.
        if reason == "refusal"
            && self
                .refusal_is_degenerate
                .load(std::sync::atomic::Ordering::SeqCst)
        {
            self.record_degenerate(&degenerate::Trip::refusal(), worker)
                .await?;
            return Err(CoreError::Refused("native_turn_degenerate"));
        }
        usage.failed = false;
        Ok(Turn {
            stop_reason: reason,
            updates,
            compacted,
        })
    }
    /// Open a writing worker's turn window: a start record naming the turn, then the
    /// checkout's dirty paths before the prompt is sent.
    async fn open_writer_turn(&self) -> CoreResult<WriterTurn> {
        let turn = crate::native_effects::identity()?[..24].to_owned();
        let c = self.credential.clone();
        let t = turn.clone();
        let since = receive(self.executor.write(move |db| {
            let task: String = db.query_row(
                "SELECT task_id FROM trusted_channels WHERE channel_id=?",
                [&c.channel_id],
                |r| r.get(0),
            )?;
            lifecycle::event_tx(
                db,
                &task,
                "NATIVE_WRITER_TURN_STARTED",
                json!({"channel_id":c.channel_id,"turn":t}),
            )
        })?)
        .await?;
        let before = crate::native_change_set::snapshot(self.policy.root.clone()).await;
        Ok(WriterTurn {
            turn,
            since,
            before,
        })
    }
    /// Close a writing turn's observation window. OpenCode retains native
    /// sandbox writes. Codex native tools cannot write; attached servers still
    /// need conservative attribution until their own routes prove containment.
    /// Claude retains its existing callback-based attribution.
    async fn settle_writer_turn(&self, window: Option<&WriterTurn>, ended: bool) -> CoreResult<()> {
        let Some(window) = window else {
            return Ok(());
        };
        let record = if ended {
            let after = crate::native_change_set::snapshot(self.policy.root.clone()).await;
            crate::native_change_set::record(&window.before, &after)
        } else {
            crate::native_change_set::incomplete("turn_outcome_unknown")
        };
        let c = self.credential.clone();
        let (turn, since) = (window.turn.clone(), window.since);
        use cortex_engine::environment::EngineKind;
        let engine = self.engine_kind;
        receive(self.executor.write(move |db| {
            let task: String = db.query_row(
                "SELECT task_id FROM trusted_channels WHERE channel_id=?",
                [&c.channel_id],
                |r| r.get(0),
            )?;
            let native = match engine {
                EngineKind::OpenCode => true,
                EngineKind::Codex => true,
                EngineKind::Claude => db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND id>?
                     AND type IN ('NATIVE_PERMISSION_REQUESTED','NATIVE_PERMISSION_AUTO_ALLOWED')
                     AND json_extract(payload_json,'$.channel_id')=?
                     AND coalesce(json_extract(payload_json,'$.tool'),'') NOT LIKE 'mcp\\_\\_cortex\\_\\_%' ESCAPE '\\'
                     AND coalesce(json_extract(payload_json,'$.kind'),'') NOT IN ('read','search','fetch','think','switch_mode'))",
                    (&task, since, &c.channel_id),
                    |r| r.get::<_, bool>(0),
                )?,
            };
            let mut payload = json!({"channel_id":c.channel_id,"turn":turn,"native_mutation":native});
            crate::native_change_set::add(&mut payload, &record);
            lifecycle::event_tx(db, &task, "NATIVE_WRITER_TURN_CHANGES", payload)?;
            Ok(())
        })?)
        .await
    }
    async fn record_update(&self, params: Value) -> CoreResult<bool> {
        if params["sessionId"] != self.session.id() {
            return Err(CoreError::Refused("native_activity_session_mismatch"));
        }
        let update = &params["update"];
        let signal = matches!(
            update["sessionUpdate"].as_str(),
            Some("tool_call" | "tool_call_update")
        ) && (update["_meta"]["contextCompaction"].is_object()
            || self.engine_kind == cortex_engine::environment::EngineKind::Claude
                && update["_meta"]["claudeCode"]["toolName"] == "compact");
        if signal {
            let evidence = update["toolCallId"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 512)
                .ok_or(CoreError::Refused("compaction_identity_missing"))?
                .to_owned();
            let phase = update["status"].as_str().unwrap_or("unknown").to_owned();
            let bound = self.credential.clone();
            let wire_session = self.session.id().to_owned();
            let retired=receive(self.executor.read(move|db|Ok(db.query_row(
                "SELECT EXISTS(SELECT 1 FROM trusted_channels c JOIN task_owners o ON o.task_id=c.task_id AND o.epoch=c.owner_epoch
                    JOIN delegation_provisioning p ON p.channel_id=c.channel_id AND p.attempt_id=c.attempt_id
                    WHERE c.channel_id=? AND p.engine_session_ref=? AND c.revoked_at IS NOT NULL)",
                (&bound.channel_id,&wire_session),|r|r.get::<_,bool>(0))?))?).await?;
            if retired {
                return Ok(false);
            }
            let c = self.credential.clone();
            let id = evidence.clone();
            let fresh = receive(self.executor.write(move|db|{
                let i=channels::authenticate(db,&c.channel_id,c.secret(),true)?;
                let session=format!("native:{}:{}",i.attempt,i.session.as_deref().unwrap());
                let seen:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM compaction_events_seen WHERE engine_session_ref=? AND evidence_id=? AND generation_id IS NOT NULL)",(&session,&evidence),|r|r.get(0))?;
                if phase!="completed" {
                    db.execute("INSERT OR IGNORE INTO compaction_events_seen VALUES (?,?,?,?,?,NULL)",(&session,&evidence,&i.task,&phase,cortex_core::now()))?;
                    return Ok(false);
                }
                cortex_core::recovery_context::generation_tx(db,&c,"observed_compaction",Some(&evidence))?;
                Ok(!seen)
            })?).await?;
            if fresh {
                // One divider in the Workspace conversation per completed compaction. It is
                // display history only: a failure to record it never fails the turn.
                let marker = json!({"sessionId":self.session.id(),"update":{
                    "sessionUpdate":"compaction_update","compactionId":id,"status":"completed"}});
                let _ = crate::native_activity::record(
                    &self.executor,
                    &self.credential,
                    self.session.id(),
                    marker,
                )
                .await;
            }
            return Ok(fresh);
        }
        crate::native_activity::record(&self.executor, &self.credential, self.session.id(), params)
            .await?;
        Ok(false)
    }
    pub(crate) async fn needs_compaction_continuation(&self) -> CoreResult<bool> {
        let c = self.credential.clone();
        receive(self.executor.write(move |db| {
            let Ok(i) = channels::authenticate(db, &c.channel_id, c.secret(), true) else {
                return Ok(false);
            };
            let state = domain::task_tx(db, &i.task)?.0;
            Ok(if i.role == "coordinator" {
                matches!(state, TaskState::Planning | TaskState::Reconciling)
            } else {
                lifecycle::waits::current_tx(db, &i.task, i.assignment.as_deref().unwrap())?
                    .is_none()
            })
        })?)
        .await
    }
    /// Resolves when this session's engine process has exited.
    pub(crate) async fn process_exited(&self) {
        self.engine.exited().await
    }
    /// The engine process's retained, secret-free diagnostics (exit status or signal,
    /// transport close, bounded redacted stderr tail). The handle stays readable after
    /// [`Self::shutdown`], which completes them.
    pub(crate) fn diagnostics(&self) -> cortex_engine::engine::Diagnostics {
        self.engine.diagnostics()
    }
    /// This session's channel id.
    pub(crate) fn channel_id(&self) -> &str {
        &self.credential.channel_id
    }
    /// `delivered`: the guidance prompt reached the engine (it was active, even if a
    /// later guidance message cancelled it); `preempted`: a newer guidance replaced it.
    pub(crate) async fn guidance_delivery(
        &self,
        request: Value,
        delivered: bool,
        preempted: bool,
    ) -> CoreResult<()> {
        let channel = self.credential.channel_id.clone();
        receive(self.executor.write(move |db| {
            let task: String = db.query_row(
                "SELECT task_id FROM trusted_channels WHERE channel_id=?",
                [&channel],
                |r| r.get(0),
            )?;
            lifecycle::event_tx(
                db,
                &task,
                "NATIVE_GUIDANCE_SETTLED",
                json!({"request":request["requestId"],"delivered":delivered,"preempted":preempted}),
            )?;
            Ok(())
        })?)
        .await
    }
    pub(crate) fn prompt_active(&self) -> bool {
        self.session.prompt_active()
    }
    /// Count an ACP `refusal` stop as a degenerate turn, as the runtime's own turn loop
    /// does. Off by default, so a direct driver caller still sees the stop reason.
    pub(crate) fn treat_refusal_as_degenerate(&self) {
        self.refusal_is_degenerate
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }
    /// Durable record of a degenerate turn: the rule, the engine message id and the
    /// message size, never its text. A worker's record names its node and plan
    /// revision, which bound the runtime's single retry of that node.
    async fn record_degenerate(&self, trip: &degenerate::Trip, worker: bool) -> CoreResult<()> {
        let channel = self.credential.channel_id.clone();
        let session = self.session.id().to_owned();
        let trip = trip.clone();
        receive(self.executor.write(move |db| {
            let (task, assignment): (String, Option<String>) = db.query_row(
                "SELECT task_id,assignment_id FROM trusted_channels WHERE channel_id=?",
                [&channel],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let node: Option<(String, i64)> = match &assignment {
                Some(a) => db
                    .query_row(
                        "SELECT node_id,revision FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
                        (&task, a),
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .optional()?,
                None => None,
            };
            lifecycle::event_tx(
                db,
                &task,
                "NATIVE_TURN_DEGENERATE",
                json!({"channel_id":channel,"session":session,
                    "role":if worker {"worker"} else {"coordinator"},
                    "assignment_ref":assignment,"node":node.as_ref().map(|n|&n.0),"revision":node.as_ref().map(|n|n.1),
                    "rule":trip.rule,"message_id":trip.message_id,"chars":trip.chars,
                    "repeated_share":trip.repeated_share}),
            )?;
            Ok(())
        })?)
        .await
    }
    /// The runtime durably withdrew this worker channel's authority: the channel is
    /// revoked or its owner epoch replaced. Never true for a coordinator channel, and
    /// an unreadable store is not taken as a revocation. Checked on the writer lane,
    /// which is serialized with the revoking transaction and is the lane the turn's own
    /// activity records already wait on.
    async fn authority_revoked(&self) -> bool {
        let channel = self.credential.channel_id.clone();
        let Ok(reply) = self.executor.write(move |db| {
            Ok(db
                .query_row(
                    "SELECT c.revoked_at IS NOT NULL OR o.epoch IS NOT c.owner_epoch
                     FROM trusted_channels c LEFT JOIN task_owners o ON o.task_id=c.task_id
                     WHERE c.channel_id=? AND c.role='worker'",
                    [&channel],
                    |r| r.get::<_, bool>(0),
                )
                .optional()?
                .unwrap_or(false))
        }) else {
            return false;
        };
        receive(reply).await.unwrap_or(false)
    }
    pub async fn cancel(&self) -> CoreResult<()> {
        self.engine
            .cancel(&self.session)
            .await
            .map_err(|_| CoreError::Refused("native_cancel_outcome_unknown"))
    }
    /// Resume the worker with its terminal receipts after a pending command reply
    /// or premature submission. The supervisor owns the command deadline.
    pub(crate) async fn command_continuation(
        &self,
        after: i64,
    ) -> CoreResult<Option<(i64, String)>> {
        let channel = self.credential.channel_id.clone();
        let pending = receive(self.executor.read(move |db| {
            // A successful result or a revoked channel already closed this assignment.
            let mut events = db.prepare(
                "SELECT e.id,e.type,e.payload_json,c.task_id FROM trusted_channels c
                 JOIN runtime_nodes n ON n.task_id=c.task_id AND n.assignment_ref=c.assignment_id
                 JOIN runtime_events e ON e.task_id=c.task_id
                 WHERE c.channel_id=? AND c.revoked_at IS NULL AND n.status='running' AND n.result_json IS NULL
                   AND e.id>? AND e.type IN ('NATIVE_COMMAND_PENDING_REPLY','WORKER_RESULT_REFUSED')
                   AND json_extract(e.payload_json,'$.channel_id')=c.channel_id
                   AND json_extract(e.payload_json,'$.attempt')=c.attempt_id ORDER BY e.id")?;
            let mut rows = events.query((&channel, after))?;
            let mut cursor = after;
            let mut task = None;
            let mut refs = std::collections::BTreeSet::new();
            let mut refusal_code: Option<(String, Vec<String>)> = None;
            while let Some(row) = rows.next()? {
                cursor = row.get(0)?;
                task = Some(row.get::<_, String>(3)?);
                let kind: String = row.get(1)?;
                let detail: Value = serde_json::from_str(&row.get::<_, String>(2)?)?;
                if kind == "NATIVE_COMMAND_PENDING_REPLY" {
                    refs.insert(detail["ref"].as_str().ok_or(CoreError::Refused("invalid_command_handle"))?.to_owned());
                } else if detail["code"] == "command_not_settled" {
                    refs.extend(serde_json::from_value::<Vec<String>>(detail["command_refs"].clone())?);
                } else {
                    refusal_code = detail["code"].as_str().map(|code| {
                        let missing = serde_json::from_value::<Vec<String>>(detail["missing"].clone())
                            .unwrap_or_default();
                        (code.to_owned(), missing)
                    });
                }
            }
            drop(rows);
            // The turn ended with this assignment still open on this channel and no
            // result: the worker has not finished, so the same session is asked to.
            let open: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM trusted_channels c
                 JOIN runtime_nodes n ON n.task_id=c.task_id AND n.assignment_ref=c.assignment_id
                 WHERE c.channel_id=? AND c.revoked_at IS NULL AND n.status='running' AND n.result_json IS NULL)",
                [&channel], |r| r.get(0),
            )?;
            let latest: i64 = db.query_row("SELECT coalesce(max(id),0) FROM runtime_events", [], |r| r.get(0))?;
            Ok((cursor, refs, task, refusal_code, open, latest))
        })?).await?;
        let (id, refs, task, refusal_code, open, latest) = pending;
        let Some(task) = task else {
            return Ok(open.then(|| (latest, String::from(
                "Your turn ended, but this assignment has no recorded result yet. Finish it in this session: complete every part you can within your role and permissions, then submit the complete result. Report a part as not done only for a genuine dead end, with its blocker as the result_submit schema describes."
            ))));
        };
        for reference in &refs {
            let done = self
                .policy
                .commands
                .lock()
                .await
                .get(reference)
                .filter(|command| command.task == task)
                .map(|command| command.done.clone());
            if let Some(mut done) = done {
                let _ = done.wait_for(|settled| *settled).await;
            }
        }
        let correction = refusal_code.map(|(code, missing)| {
            let mut message = format!(
                "Your result submission was refused with `{code}`. Correct the reported issue, rerun any failed check exactly, and submit the complete result again. Do not claim success from an incomplete or missing check."
            );
            if !missing.is_empty() {
                message.push_str(&format!(
                    " Still outstanding, to finish in this session: {}.",
                    missing.join("; ")
                ));
            }
            message
        });
        if refs.is_empty() {
            return Ok(correction.map(|message| (id, message)));
        }
        let channel = self.credential.channel_id.clone();
        let receipts = receive(self.executor.read(move |db| {
            let mut receipts = Vec::new();
            for reference in refs {
                let receipt: Option<String> = db.query_row(
                    "SELECT json_object('ref',?, 'exit_code',json_extract(result_json,'$.exit_code'),
                    'failure',json_extract(result_json,'$.failure')) FROM runtime_requests
                    WHERE task_id=? AND actor=? AND operation='command_run' AND json_extract(result_json,'$.tool_call_ref')=?
                    AND json_extract(result_json,'$.settled')=1",
                    (&reference, &task, &channel, &reference),
                    |r| r.get(0),
                )
                .optional()?;
                let Some(raw) = receipt else {
                    return Err(CoreError::Refused("lost_worker_action_outcome_unknown"));
                };
                receipts.push(serde_json::from_str::<Value>(&raw)?);
            }
            Ok(receipts)
        })?)
        .await?;
        let mut message = format!(
            "Commands launched during your turn have now settled. Their final receipts: {}. Inspect their output with command_status and submit the result with the observed outcomes.",
            json!(receipts)
        );
        if let Some(correction) = correction {
            message.push('\n');
            message.push_str(&correction);
        }
        Ok(Some((id, message)))
    }
    /// Return the event cursor immediately before a worker prompt starts. A
    /// continuation must only consume a refusal produced by that prompt, never
    /// one retained from an earlier turn on the same channel.
    pub(crate) async fn event_cursor(&self) -> CoreResult<i64> {
        receive(self.executor.read(|db| {
            Ok(
                db.query_row("SELECT coalesce(max(id),0) FROM runtime_events", [], |r| {
                    r.get(0)
                })?,
            )
        })?)
        .await
    }
    pub(crate) async fn wait_state(&self) -> CoreResult<SessionWait> {
        let c = self.credential.clone();
        let session = self.session.id().to_owned();
        receive(self.executor.write(move |db| {
            let (task, assignment, coordinator): (String, String, bool) = db.query_row(
                "SELECT task_id,idempotency_key,role_id='orchestrator' FROM delegation_provisioning WHERE channel_id=?",
                [&c.channel_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )?;
            if coordinator {
                let state = lifecycle::skill_proposals::continuation(db, &task, &c.channel_id, false)?;
                return Ok(match state.as_deref() {
                    Some("ready") => SessionWait::Ready,
                    Some("open") => SessionWait::Open(None),
                    _ => SessionWait::None,
                });
            }
            let Some(mut wait) = waits::current_tx(db, &task, &assignment)? else {
                return Ok(SessionWait::None);
            };
            if wait["channel_id"] != c.channel_id || wait["engine_session_ref"] != session {
                return Ok(SessionWait::None);
            }
            let retry = scheduler::policy::poll_access_tx(db, &mut wait, &cortex_core::now())?;
            Ok(match wait["status"].as_str() {
                Some("open") => SessionWait::Open(retry),
                Some("ready") => SessionWait::Ready,
                _ => SessionWait::None,
            })
        })?)
        .await
    }
    pub async fn continue_answer(&self) -> CoreResult<bool> {
        let c = self.credential.clone();
        let session = self.session.id().to_owned();
        let coordinator = self.credential.clone();
        let next = receive(self.executor.write(move |db| {
            let i =
                channels::authenticate(db, &coordinator.channel_id, coordinator.secret(), true)?;
            if i.role == "coordinator" {
                lifecycle::skill_proposals::continuation(db, &i.task, &coordinator.channel_id, true)
            } else {
                Ok(None)
            }
        })?)
        .await?;
        if let Some(text) = next {
            self.prompt(&text).await?;
            return Ok(true);
        }
        let claimed = receive(self.executor.write(move |db| {
            let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
            let assignment = i.assignment.ok_or(CoreError::Refused("worker_required"))?;
            // The channel's last request before the answer is sent: any later one was
            // made by the engine in the answer's own turn (it makes none between turns).
            let before: i64 = db.query_row(
                "SELECT coalesce(max(rowid),0) FROM runtime_requests WHERE task_id=? AND actor=?",
                (&i.task, &c.channel_id),
                |r| r.get(0),
            )?;
            Ok(waits::claim_tx(db, &i.task, &assignment, &session)?.map(|d| (d, before)))
        })?)
        .await?;
        let Some((delivery, before)) = claimed else {
            return Ok(false);
        };
        let text = delivery["answer"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| delivery["answer"].to_string());
        let result = self.prompt(&text).await;
        let c = self.credential.clone();
        let delivered = result
            .as_ref()
            .is_ok_and(|turn| turn.stop_reason == "end_turn");
        let settled = receive(self.executor.write(move |db| {
            let task: String = db.query_row(
                "SELECT task_id FROM trusted_channels WHERE channel_id=?",
                [&c.channel_id],
                |r| r.get(0),
            )?;
            // A channel may be revoked by result_submit before ACP replies. Only
            // this attempt's durable result after this claimed delivery proves it
            // consumed the answer; transport loss by itself remains unknown.
            let recorded = db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_requests r
                JOIN trusted_channels c ON c.channel_id=r.actor AND c.task_id=r.task_id
                JOIN task_owners o ON o.task_id=c.task_id AND o.epoch=c.owner_epoch
                JOIN runtime_events e ON e.task_id=c.task_id AND e.type IN ('WORKER_COMPLETED','WORKER_FAILED')
                AND json_extract(e.payload_json,'$.attempt_id')=c.attempt_id
                WHERE r.task_id=? AND r.actor=? AND r.operation='result_submit'
                AND e.id>(SELECT max(id) FROM runtime_events WHERE task_id=r.task_id AND type='WORKER_CONTINUATION_STARTED'
                AND json_extract(payload_json,'$.delivery_id')=?))",
                    (&task,&c.channel_id,delivery["wait"]["delivery_id"].as_str()), |r|r.get::<_,bool>(0))?;
            // The engine acted in the answer's turn (a tool call through this channel
            // after the answer was sent): it received the answer, even when that call
            // (a replan request, a stop) ended the turn before ACP replied.
            let acted: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND rowid>?)",
                (&task, &c.channel_id, before),
                |r| r.get(0),
            )?;
            let current = channels::authenticate(db, &c.channel_id, c.secret(), true).is_ok();
            let failure = if delivered && current || recorded || acted {
                None
            } else {
                Some("answer_delivery_unknown")
            };
            let (wait_id, delivery_id) = (
                delivery["wait"]["wait_id"].as_str().unwrap(),
                delivery["wait"]["delivery_id"].as_str().unwrap(),
            );
            let finished = waits::finish_tx(db, &task, wait_id, delivery_id, failure)?;
            // A call made in the answer's own turn can end the assignment before ACP
            // replies: a replan request cancels it, and the cancellation closes this
            // delivery's wait as `cancelled`. With the answer proven received, the
            // runtime's own close is that delivery's outcome, never an unconfirmed one.
            let closed = !finished
                && failure.is_none()
                && waits::projection_tx(db, &task)?.iter().any(|w| {
                    w["wait_id"] == wait_id
                        && w["delivery_id"] == delivery_id
                        && w["status"] == "cancelled"
                });
            Ok((finished || closed) && failure.is_none())
        })?)
        .await?;
        // A degenerate answer turn is retried like any other degenerate turn.
        if let Err(error) = &result {
            if error.code() == "native_turn_degenerate" {
                return Err(CoreError::Refused("native_turn_degenerate"));
            }
        }
        if !settled {
            return Err(CoreError::Refused("native_answer_delivery_unconfirmed"));
        }
        Ok(true)
    }
    pub async fn shutdown(self) -> CoreResult<()> {
        self.shutdown_managed(None).await
    }

    pub(super) async fn shutdown_managed(
        self,
        attempt: Option<&mut crate::native_route_hold::Attempt>,
    ) -> CoreResult<()> {
        let refusal = attempt.as_ref().and_then(|_| self.provider_refusal());
        let Self {
            executor,
            capacity_changed,
            credential,
            reservation,
            engine,
            broker,
            external,
            ..
        } = self;
        let mut observation = if engine.shutdown().await.is_ok() {
            ProcessObservation::ConfirmedExited
        } else {
            ProcessObservation::Unknown
        };
        let broker_result = broker.shutdown().await;
        let mut external_ok = true;
        for a in external {
            external_ok &= a.shutdown().await.is_ok();
        }
        if !external_ok {
            observation = ProcessObservation::Unknown;
        }
        let provider_limit = refusal.and_then(|(limit, activity)| {
            (observation == ProcessObservation::ConfirmedExited && broker_result.is_ok()
                && !activity.load(std::sync::atomic::Ordering::SeqCst))
                .then_some(limit)
        });
        if let (Some(attempt), Some(limit)) = (attempt, provider_limit) {
            attempt.limited(limit, credential.channel_id.clone());
        }
        settle(&executor, credential, reservation, observation, provider_limit).await?;
        if observation == ProcessObservation::ConfirmedExited {
            capacity_changed.notify_waiters();
        }
        if observation != ProcessObservation::ConfirmedExited
            || broker_result.is_err()
            || !external_ok
        {
            return Err(CoreError::Refused("native_shutdown_unconfirmed"));
        }
        Ok(())
    }
}
async fn settle(
    executor: &StoreExecutor,
    c: Credential,
    reservation: String,
    observation: ProcessObservation,
    provider_limit: Option<cortex_engine::ProviderLimit>,
) -> CoreResult<()> {
    receive(executor.write(move |db| {
        channels::revoke_tx(db, &c.channel_id)?;
        db.execute("UPDATE capsule_deliveries SET state='superseded',reason='session_settled',updated_at=?
            WHERE engine_session_ref IN (SELECT 'native:'||attempt_id||':'||engine_session_ref
                FROM delegation_provisioning WHERE channel_id=?) AND state IN ('built','transmitted','delivery_confirmed')",
            (cortex_core::now(),&c.channel_id))?;
        scheduler::settle_tx(db, &reservation, observation)?;
        db.execute(
            "UPDATE delegation_provisioning SET status=?,updated_at=? WHERE channel_id=?",
            (
                if observation == ProcessObservation::ConfirmedExited {
                    "terminated"
                } else {
                    "ambiguous"
                },
                cortex_core::now(),
                &c.channel_id,
            ),
        )?;
        if let Some(limit) = provider_limit {
            let task: String = db.query_row(
                "SELECT task_id FROM trusted_channels WHERE channel_id=?",
                [&c.channel_id], |row| row.get(0),
            )?;
            lifecycle::event_tx(db, &task, "NATIVE_PROVIDER_ROUTE_REFUSED", json!({
                "channel_id":c.channel_id,
                "reason":match limit.kind {
                    cortex_engine::LimitKind::Rate => "provider_rate_limited",
                    cortex_engine::LimitKind::Quota => "provider_quota_limited",
                },
                "retry_after_seconds":limit.retry_after_seconds, "resets_at":limit.resets_at_unix_seconds
            }))?;
        }
        // A removed receiver or interrupted prompt cannot leave a durable queue
        // looking deliverable after its exact worker process has settled.
        let pending=db.prepare("SELECT q.task_id,json_extract(q.payload_json,'$.request') FROM runtime_events q
            JOIN delegation_provisioning p ON p.task_id=q.task_id AND p.channel_id=?
            WHERE q.type='NATIVE_GUIDANCE_QUEUED' AND json_extract(q.payload_json,'$.worker') IN (p.idempotency_key,p.attempt_id)
            AND NOT EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=q.task_id AND e.type='NATIVE_GUIDANCE_SETTLED'
                AND json_extract(e.payload_json,'$.request')=json_extract(q.payload_json,'$.request'))")?
            .query_map([&c.channel_id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?)))?
            .collect::<Result<Vec<_>,_>>()?;
        for (task,request) in pending {
            lifecycle::event_tx(db,&task,"NATIVE_GUIDANCE_SETTLED",json!({"request":request,"delivered":false}))?;
        }
        Ok(())
    })?)
    .await
}
/// Concrete default-deny composition. Unsupported tools are not advertised.
struct CorePolicy {
    workspace_state: Option<PathBuf>,
    commands: crate::native_commands::Running,
    routes: Option<Arc<dyn crate::native_runtime::NativeRoutes>>,
    permissions: Option<Arc<dyn ClientPermissions>>,
    executor: Arc<StoreExecutor>,
    credential: Credential,
    root: PathBuf,
    protected_roots: Vec<PathBuf>,
    engine: cortex_engine::environment::EngineKind,
    /// External MCP servers attached to this session, set by the start attempt
    /// that launched it before the session is bound.
    mcp_servers: std::sync::Mutex<Vec<String>>,
    activity: Arc<std::sync::atomic::AtomicBool>,
}
impl CorePolicy {
    async fn check(&self, ready: bool) -> cortex_engine::Result<Identity> {
        let c = self.credential.clone();
        receive(
            self.executor
                .read(move |db| channels::authenticate(db, &c.channel_id, c.secret(), ready))
                .map_err(engine_error)?,
        )
        .await
        .map_err(engine_error)
    }
}
/// The allow option of a request the access mode allows because it names a
/// channel-bound MCP tool (rule `channel_mcp`), from the same classification the
/// decision path applies; `None` for every other request, including a channel tool
/// whose request offers no allow option.
fn auto_channel_allow(
    request: &Value,
    i: &Identity,
    access: &str,
    engine: cortex_engine::environment::EngineKind,
    root: &std::path::Path,
    mcp_servers: &[String],
) -> Option<String> {
    use cortex_engine::permission::{self, Action};
    let read_only = i.role != "worker"
        || cortex_core::scheduler::policy::read_only(
            i.profile.as_deref().unwrap_or(""),
            i.stage.as_deref().unwrap_or(""),
        );
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|h| h.is_absolute());
    let class = |p: &std::path::Path| crate::native_engine_policy::path_class(root, p);
    let verdict = permission::classify(
        request,
        &permission::Facts {
            access,
            read_only,
            consultant: matches!(
                i.profile.as_deref(),
                Some("worker-senior-consultant" | "senior-consultant")
            ),
            runtime_answers_native: engine != cortex_engine::environment::EngineKind::OpenCode,
            root,
            home: home.as_deref(),
            mcp_servers,
            class: &class,
        },
    );
    (matches!(verdict.action, Action::Allow) && verdict.rule == "channel_mcp")
        .then(|| permission::allow_option(request))
        .flatten()
}
impl PermissionPolicy for CorePolicy {
    fn authority(&self) -> FutureResult<'_, ()> {
        Box::pin(async move { self.check(false).await.map(|_| ()) })
    }
    fn decide<'a>(
        &'a self,
        session: &'a str,
        request_id: &'a Value,
        generation: u64,
        request: &'a Value,
    ) -> FutureResult<'a, PermissionDecision> {
        Box::pin(async move {
            self.activity.store(true, std::sync::atomic::Ordering::SeqCst);
            use cortex_engine::permission::{self, Action, Fallback};
            let c = self.credential.clone();
            let expected_session = session.to_owned();
            let session = session.to_owned();
            let id = request_id.clone();
            let tool = permission::tool_name(request).map(str::to_owned);
            let kind = permission::tool_kind(request).map(str::to_owned);
            let (owned, root, engine) = (request.clone(), self.root.clone(), self.engine);
            let mcp_servers = self.mcp_servers.lock().unwrap().clone();
            let incoming = receive(self.executor.write(move |db| {
                let i=channels::authenticate(db,&c.channel_id,c.secret(),true)?;
                if i.session.as_deref()!=Some(&session) {return Err(CoreError::Refused("session_mismatch"));}
                let mut q=db.prepare("SELECT session_id FROM acp_host_sessions WHERE task_id=?
                    AND json_extract(client_info,'$.name')='cortex-rust-acp'")?;
                let sessions=q.query_map([&i.task],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
                if sessions.len()>1 {return Err(CoreError::Refused("incoming_session_binding_conflict"));}
                let access=cortex_core::access::current(db,&i.task)?;
                // A channel-bound tool (the Cortex gateway or an attached external server,
                // both re-checking the channel and operation inside the call) is allowed by
                // the access mode alone. It is the bulk of all requests, so its request and
                // allowance are one record, written in this one transaction with the channel,
                // session and access mode it was decided under. Every other request keeps
                // its request record here and its decision record once decided.
                if let Some(option) = auto_channel_allow(&owned, &i, &access, engine, &root, &mcp_servers) {
                    lifecycle::event_tx(db,&i.task,"NATIVE_PERMISSION_AUTO_ALLOWED",
                        json!({"channel_id":c.channel_id,"session":session,"request_id":id,"turn_generation":generation,
                            "tool":tool,"kind":kind,"outcome":{"outcome":"selected","optionId":option},
                            "rule":"channel_mcp","decider":format!("mode:{access}")}))?;
                    return Ok(Err(option));
                }
                lifecycle::event_tx(db,&i.task,"NATIVE_PERMISSION_REQUESTED",
                    json!({"channel_id":c.channel_id,"session":session,"request_id":id,"turn_generation":generation,
                        "tool":tool,"kind":kind}))?;
                Ok(Ok((sessions.into_iter().next(),access)))
            }).map_err(engine_error)?).await.map_err(engine_error)?;
            let (incoming, access) = match incoming {
                Ok(pending) => pending,
                Err(option) => return Ok(PermissionDecision::Selected(option)),
            };
            let i = self.check(true).await?;
            let read_only = crate::native_engine_policy::read_only(&i);
            // claude-agent-acp (`default`) asks for every tool it does not run itself and
            // codex-acp (`read-only`/`workspace-write`) asks to leave its sandbox; the
            // runtime answers both itself. OpenCode keeps the client route.
            let answers = self.engine != cortex_engine::environment::EngineKind::OpenCode;
            let class =
                |p: &std::path::Path| crate::native_engine_policy::path_class(&self.root, p);
            let home = std::env::var_os("HOME")
                .map(std::path::PathBuf::from)
                .filter(|h| h.is_absolute());
            let mcp_servers = self.mcp_servers.lock().unwrap().clone();
            let verdict = permission::classify(
                request,
                &permission::Facts {
                    access: &access,
                    read_only,
                    consultant: matches!(
                        i.profile.as_deref(),
                        Some("worker-senior-consultant" | "senior-consultant")
                    ),
                    runtime_answers_native: answers,
                    root: &self.root,
                    home: home.as_deref(),
                    mcp_servers: &mcp_servers,
                    class: &class,
                },
            );
            let refuse = || match permission::reject_option(request) {
                Some(option) if answers => PermissionDecision::Selected(option),
                _ => PermissionDecision::Cancelled,
            };
            // Who decided: the owner on a card, the access mode for an allowance,
            // the runtime for a policy refusal or a card nobody could answer.
            let mut decider = "runtime".to_owned();
            let mut owner_outcome: Option<(&str, String, Value)> = None;
            let decision = match verdict.action {
                Action::Allow => {
                    decider = format!("mode:{access}");
                    permission::allow_option(request)
                        .map(PermissionDecision::Selected)
                        .unwrap_or_else(refuse)
                }
                Action::Refuse => refuse(),
                Action::Cancel => PermissionDecision::Cancelled,
                Action::Ask { fallback, .. } => {
                    // The card names role, operation, path and reason and offers only
                    // Allow once / Deny: on the incoming ACP client, or as an owner
                    // decision of a Workspace task. The engine's request waits for it.
                    let who = i.profile.clone().unwrap_or_else(|| i.role.clone());
                    let card = permission::owner_card(request, &who, verdict.rule);
                    let details = crate::native_owner_decisions::details(
                        request,
                        &who,
                        verdict.rule,
                        &access,
                    );
                    let request_ref = format!(
                        "{}#{}",
                        request["toolCall"]["toolCallId"].as_str().unwrap_or(""),
                        request_id
                    );
                    let provider = self.permissions.clone();
                    match crate::native_owner_decisions::ask(
                        &self.executor,
                        &self.credential,
                        provider,
                        incoming,
                        &request_ref,
                        &card,
                        details,
                    )
                    .await
                    .map_err(engine_error)?
                    {
                        crate::native_owner_decisions::Owner::Allowed => {
                            decider = "owner".to_owned();
                            owner_outcome = Some(("allow_once", request_ref, card.clone()));
                            permission::allow_option(&card)
                                .map(PermissionDecision::Selected)
                                .unwrap_or_else(refuse)
                        }
                        crate::native_owner_decisions::Owner::Denied => {
                            decider = "owner".to_owned();
                            owner_outcome = Some(("reject_once", request_ref, card.clone()));
                            refuse()
                        }
                        crate::native_owner_decisions::Owner::Withdrawn(reason) => {
                            decider = format!("withdrawn:{reason}");
                            owner_outcome = Some(("cancelled", request_ref, card.clone()));
                            PermissionDecision::Cancelled
                        }
                        crate::native_owner_decisions::Owner::Unavailable => {
                            decider = "no_owner_client".to_owned();
                            match fallback {
                                Fallback::Refuse => refuse(),
                                Fallback::Cancel => PermissionDecision::Cancelled,
                            }
                        }
                    }
                }
            };
            let rule = verdict.rule;

            // Late responses cannot authorize a revoked/paused/replaced channel.
            let i = self.check(true).await?;
            if i.session.as_deref() != Some(expected_session.as_str()) {
                return Err(Error::Forbidden);
            }
            let c = self.credential.clone();
            let cancelled = matches!(&decision, PermissionDecision::Cancelled);
            let outcome = match &decision {
                PermissionDecision::Cancelled => json!({"outcome":"cancelled"}),
                PermissionDecision::Selected(id) => json!({"outcome":"selected","optionId":id}),
            };
            receive(self.executor.write(move|db|{
                let i=channels::authenticate(db,&c.channel_id,c.secret(),true)?;
                if i.session.as_deref()!=Some(expected_session.as_str()) {return Err(CoreError::Refused("session_mismatch"));}
                if cortex_core::access::current(db,&i.task)?!=access {return Err(CoreError::Refused("access_mode_changed"));}
                lifecycle::event_tx(db,&i.task,if cancelled {"NATIVE_PERMISSION_CANCELLED"}else{"NATIVE_PERMISSION_DECIDED"},
                    json!({"channel_id":c.channel_id,"outcome":outcome,"turn_generation":generation,"rule":rule,"decider":decider}))?;
                // An owner-decided request also leaves its permission receipt.
                if let Some((host_outcome,reference,card))=&owner_outcome {
                    cortex_core::permission_decisions::receipt_tx(db,&i.task,&i.attempt,reference,
                        card["toolCall"]["title"].as_str().unwrap_or(""),host_outcome)?;
                }
                Ok(())
            }).map_err(engine_error)?).await.map_err(engine_error)?;
            Ok(decision)
        })
    }
}
impl GatewayCore for CorePolicy {
    fn supports(&self, name: &str) -> bool {
        matches!(
            name,
            "read_skill"
                | "read_batch"
                | "ask_question"
                | "user_question_request"
                | "answer_submit"
                | "result_submit"
                | "plan_submit"
                | "list_route_options"
                | "synthesis_submit"
                | "verification_submit"
                | "replan_request"
                | "scope_request"
                | "escalation_request"
        ) || crate::native_documents::supports(name)
            || crate::native_access::supports(name)
            || crate::native_sprint_plan::supports(name)
            || crate::native_sources::supports(name)
            || crate::native_project::supports(name)
            || crate::native_project_ops::supports(name)
            || crate::native_context_tools::supports(name)
            || crate::native_commands::supports(name)
    }
    fn supports_channel(&self, channel: &Channel, name: &str) -> bool {
        crate::native_instructions::supports(channel.stage.as_deref(), name)
    }
    fn authenticate<'a>(&'a self, credentials: &'a Credentials) -> FutureResult<'a, Channel> {
        Box::pin(async move {
            if credentials.project != self.root
                || credentials.channel_id != self.credential.channel_id
                || credentials.secret() != self.credential.secret()
            {
                return Err(Error::Forbidden);
            }
            // Discovery during session/new is allowed; calls require ready below.
            self.check(false).await.map(|i| view(&i))
        })
    }
    fn execute<'a>(
        &'a self,
        credentials: &'a Credentials,
        channel: &'a Channel,
        name: &'a str,
        arguments: Value,
        _request_id: Value,
    ) -> FutureResult<'a, Value> {
        // Enforcement is unchanged; a non-authority failure after channel
        // authentication is reported as a structured, correctable tool error.
        Box::pin(async move {
            self.activity.store(true, std::sync::atomic::Ordering::SeqCst);
            let result = self
                .dispatch_call(credentials, channel, name, arguments, false)
                .await;
            // result_submit keeps its own classification (native_results::correctable):
            // its other refusals stay named refusals, never a retryable tool error.
            if name == "result_submit" {
                result
            } else {
                crate::native_tool_errors::finish(result)
            }
        })
    }
}
impl CorePolicy {
    /// Channel authentication that keeps the precise refusal code.
    async fn identity(&self, ready: bool) -> cortex_engine::Result<Identity> {
        let c = self.credential.clone();
        receive(
            self.executor
                .read(move |db| channels::authenticate(db, &c.channel_id, c.secret(), ready))
                .map_err(tool_error)?,
        )
        .await
        .map_err(tool_error)
    }
    /// `read_batch`: up to eight batchable read-only reads, preflighted together,
    /// then executed one at a time through the same handler a direct call uses,
    /// with channel authority rechecked between items.
    async fn batch(
        &self,
        credentials: &Credentials,
        channel: &Channel,
        arguments: Value,
    ) -> cortex_engine::Result<Value> {
        use crate::native_tool_errors::{item_error, reply_at};
        let offered = cortex_engine::schema::catalogue(
            &channel.role,
            channel.profile.as_deref(),
            channel.stage.as_deref(),
        )?
        .into_iter()
        .filter(|t| {
            let n = t["name"].as_str().unwrap_or("");
            self.supports(n) && self.supports_channel(channel, n)
        })
        .collect::<Vec<_>>();
        if let Err((code, field)) = crate::native_batch::preflight(&offered, &arguments) {
            return Ok(reply_at(code, &field));
        }
        let calls = arguments["calls"].as_array().cloned().unwrap_or_default();
        let started = std::time::Instant::now();
        let (mut results, mut executed) = (Vec::new(), 0usize);
        for call in calls {
            let (id, tool) = (
                call["id"].clone(),
                call["tool"].as_str().unwrap_or("").to_owned(),
            );
            if started.elapsed() >= crate::native_batch::ADMISSION {
                results.push(json!({"id":id,"tool":tool,"status":"not_run","reason":"batch_budget_exhausted"}));
                continue;
            }
            // Authority loss is not an item failure: it withholds every body.
            let i = self.identity(true).await?;
            if !same(&i, channel) {
                return Err(Error::Refused("channel_identity_changed"));
            }
            executed += 1;
            // Box the recursive dispatch: direct dispatch can itself call batch.
            let member = Box::pin(self.dispatch_call(
                credentials,
                channel,
                &tool,
                call["arguments"].clone(),
                true,
            ));
            let reply = crate::native_tool_errors::finish(member.await)?;
            let i = self.identity(true).await?;
            if !same(&i, channel) {
                return Err(Error::Refused("channel_identity_changed"));
            }
            let mut item = json!({"id":id,"tool":tool});
            if reply["isError"] == true {
                item["status"] = json!("error");
                item["error"] = item_error(&reply);
            } else if reply["content"]
                .as_array()
                .is_some_and(|c| c.iter().any(|b| b["type"] != "text"))
            {
                item["status"] = json!("error");
                item["error"] = crate::native_tool_errors::reply("batch_multimodal_unsupported")
                    ["structuredContent"]
                    .clone();
            } else {
                let mut data = reply["structuredContent"].clone();
                let mut bytes = serde_json::to_vec(&data)
                    .map_err(CoreError::from)
                    .map_err(tool_error)?;
                if bytes.len() <= crate::native_batch::ITEM_BYTES {
                    item["status"] = json!("ok");
                    item["result"] = data;
                } else if tool == "read_large_output" {
                    item["status"] = json!("error");
                    item["error"] = crate::native_tool_errors::reply("batch_result_limit")
                        ["structuredContent"]
                        .clone();
                } else {
                    // A deferred project read didn't persist the proposed coverage.
                    // Keep the page, but don't advertise an overwrite precondition.
                    if let Some(fields) = data.as_object_mut().filter(|_| tool == "project_read") {
                        fields.remove("read_coverage_complete");
                        fields.remove("full_content_sha256");
                        bytes = serde_json::to_vec(&data)
                            .map_err(CoreError::from)
                            .map_err(tool_error)?;
                    }
                    let result_sha256 = crate::native_project::digest(&bytes);
                    let c = self.credential.clone();
                    let expected = channel.clone();
                    let deferred = receive(
                        self.executor
                            .publication(move |store| {
                                let i = store.read(|db| {
                                    channels::authenticate(db, &c.channel_id, c.secret(), true)
                                })?;
                                if !same(&i, &expected) {
                                    return Err(CoreError::Refused("channel_identity_changed"));
                                }
                                // Already classified as deferred.
                                crate::native_commands::defer_json_over(store, &i, data, 0)
                            })
                            .map_err(tool_error)?,
                    )
                    .await
                    .map_err(tool_error)?;
                    item["status"] = json!("deferred");
                    item["large_output_id"] = deferred["large_output_id"].clone();
                    item["byte_count"] = deferred["byte_count"].clone();
                    item["result_sha256"] = json!(result_sha256);
                }
            }
            results.push(item);
        }
        // A deferred item ran and is retained under its large_output_id: a normal outcome.
        // Only an item error or an item that did not run makes the MCP result an error.
        let complete = results
            .iter()
            .all(|r| r["status"] == "ok" || r["status"] == "deferred");
        let returned = results.iter().filter(|r| r["status"] == "ok").count();
        let deferred = results.iter().filter(|r| r["status"] == "deferred").count();
        let value = json!({"outcome":if complete {"complete"} else {"incomplete"},"results":results,
            "executed_count":executed,"returned_count":returned,"deferred_count":deferred});
        if serde_json::to_vec(&value)
            .map(|b| b.len())
            .unwrap_or(usize::MAX)
            > crate::native_batch::RESULT_BYTES
        {
            return Ok(crate::native_tool_errors::reply("result_too_large"));
        }
        Ok(
            json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":!complete}),
        )
    }
    async fn dispatch_call(
        &self,
        credentials: &Credentials,
        channel: &Channel,
        name: &str,
        arguments: Value,
        batched: bool,
    ) -> cortex_engine::Result<Value> {
        {
            if credentials.project != self.root
                || credentials.channel_id != self.credential.channel_id
                || credentials.secret() != self.credential.secret()
            {
                return Err(Error::Forbidden);
            }
            self.identity(false).await?;
            if !self.supports(name) || !self.supports_channel(channel, name) {
                return Err(Error::Refused("tool_not_offered"));
            }
            if name == "read_batch" {
                return self.batch(credentials, channel, arguments).await;
            }
            let c = self.credential.clone();
            let expected = channel.clone();
            let name = name.to_owned();
            // Codex uses the same effect routes as the other engines (owner
            // decision 2026-11-22); the strict scoped route is not used.
            let scoped = false;
            if crate::native_project_ops::supports(&name) {
                let i = self.identity(true).await?;
                if !same(&i, &expected) {
                    return Err(Error::Refused("channel_identity_changed"));
                }
                return crate::native_tool_errors::outcome(
                    crate::native_project_ops::execute(
                        self.executor.clone(),
                        self.credential.clone(),
                        self.root.clone(),
                        self.permissions.clone(),
                        &name,
                        arguments,
                        scoped,
                    )
                    .await,
                );
            }
            if name == "check_preflight" {
                let i = self.identity(true).await?;
                if !same(&i, &expected) {
                    return Err(Error::Refused("channel_identity_changed"));
                }
                return crate::native_tool_errors::outcome(
                    crate::native_context_tools::preflight(self.root.clone(), &arguments)
                        .await
                        .map(crate::native_sources::reply),
                );
            }
            if crate::native_context_tools::supports(&name) {
                let state = self.workspace_state.clone();
                let recipient = self.root.clone();
                let result = receive(
                    self.executor
                        .publication(move |store| {
                            let i = store.read(|db| {
                                channels::authenticate(db, &c.channel_id, c.secret(), true)
                            })?;
                            if !same(&i, &expected) {
                                return Err(CoreError::Refused("channel_identity_changed"));
                            }
                            if matches!(name.as_str(), "sprint_graph" | "sprint_task_result") {
                                return crate::native_context_tools::sprint_read(
                                    store,
                                    &c,
                                    &i,
                                    state.as_deref(),
                                    &recipient,
                                    &name,
                                    &arguments,
                                );
                            }
                            if name == "task_get_context" {
                                crate::native_context_tools::task_context(store, &i, &c.channel_id)
                            } else {
                                crate::native_context_tools::capsule(store, &i, &arguments)
                            }
                        })
                        .map_err(tool_error)?,
                )
                .await;
                return crate::native_tool_errors::outcome(
                    result.map(crate::native_sources::reply),
                );
            }
            if crate::native_commands::supports(&name) {
                let key = arguments["request_key"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let result = crate::native_commands::execute(
                    self.executor.clone(),
                    self.credential.clone(),
                    self.root.clone(),
                    self.permissions.clone(),
                    self.protected_roots.clone(),
                    self.commands.clone(),
                    &name,
                    arguments,
                    scoped,
                )
                .await;
                if let Err(error) = &result {
                    crate::native_commands::record_failure(
                        &self.executor,
                        &self.credential,
                        &key,
                        "command_call",
                        error,
                    )
                    .await;
                }
                return result.map_err(tool_error);
            }
            if crate::native_project::supports(&name) || crate::native_sources::supports(&name) {
                let root = self.root.clone();
                let result = receive(
                    self.executor
                        .publication(move |store| {
                            let i = store.read(|db| {
                                channels::authenticate(db, &c.channel_id, c.secret(), true)
                            })?;
                            if !same(&i, &expected) {
                                return Err(CoreError::Refused("channel_identity_changed"));
                            }
                            if crate::native_project::supports(&name) {
                                crate::native_project::execute(
                                    store, &i, &c, &name, &arguments, batched, &root, scoped,
                                )
                            } else {
                                crate::native_sources::execute(store, &i, &name, &arguments)
                            }
                        })
                        .map_err(tool_error)?,
                )
                .await;
                // Project and source failures were already tool errors; they now
                // carry the structured code, field and corrective hint.
                return Ok(match result {
                    Ok(v) => crate::native_tool_errors::enrich(v),
                    Err(e) => crate::native_tool_errors::reply(e.code()),
                });
            }
            if crate::native_sprint_plan::supports(&name) {
                return crate::native_sprint_plan::execute(
                    self.executor.clone(),
                    self.credential.clone(),
                    arguments,
                )
                .await
                .map_err(tool_error);
            }
            if crate::native_access::supports(&name) {
                return crate::native_access::execute(
                    self.executor.clone(),
                    self.credential.clone(),
                    self.root.clone(),
                    self.permissions.clone(),
                    self.protected_roots.clone(),
                    self.commands.clone(),
                    &name,
                    arguments,
                    scoped,
                )
                .await
                .map_err(tool_error);
            }
            if crate::native_documents::supports(&name) {
                let value = receive(
                    self.executor
                        .publication(move |store| {
                            let i = store.read(|db| {
                                channels::authenticate(db, &c.channel_id, c.secret(), true)
                            })?;
                            if !same(&i, &expected) {
                                return Err(CoreError::Refused("channel_identity_changed"));
                            }
                            crate::native_documents::execute(store, &c, &i, &name, &arguments)
                        })
                        .map_err(tool_error)?,
                )
                .await
                .map_err(tool_error)?;
                return Ok(json!({"content":[{"type":"text","text":value.to_string()}],
                    "structuredContent":value,"isError":false}));
            }
            if name == "list_route_options" {
                if channel.role != "coordinator" {
                    return Err(Error::Refused("tool_refused"));
                }
                let routes = self
                    .routes
                    .clone()
                    .ok_or(Error::Refused("route_options_unavailable"))?;
                let root = self.root.clone();
                let value = receive(
                    self.executor
                        .read(move |db| {
                            let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
                            if !same(&i, &expected) {
                                return Err(CoreError::Refused("channel_identity_changed"));
                            }
                            if arguments != json!({}) {
                                return Err(CoreError::Refused("invalid_arguments"));
                            }
                            let evidence = cortex_core::routing::evidence(db, &i.task, &root)?;
                            routes.menu(&root, &evidence)
                        })
                        .map_err(tool_error)?,
                )
                .await
                .map_err(tool_error)?;
                return Ok(
                    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false}),
                );
            }
            if name == "read_skill" {
                let value = receive(
                    self.executor
                        .read(move |db| {
                            let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
                            if !same(&i, &expected) {
                                return Err(CoreError::Refused("channel_identity_changed"));
                            }
                            let tools = cortex_engine::schema::catalogue(
                                &i.role,
                                i.profile.as_deref(),
                                i.stage.as_deref(),
                            )
                            .map_err(engine_error_to_core)?;
                            let tool = tools
                                .iter()
                                .find(|t| t["name"] == "read_skill")
                                .ok_or(CoreError::Refused("tool_refused"))?;
                            let valid =
                                cortex_engine::schema::validate(&tool["inputSchema"], &arguments)
                                    .is_ok()
                                    && arguments["skill"].is_string();
                            let profile = i.profile.unwrap_or_else(|| "orchestrator".into());
                            let project = crate::native_skills::project_files(
                                &i.role,
                                Some(profile.as_str()).filter(|_| i.role == "worker"),
                                i.stage.as_deref(),
                            );
                            Ok((valid, profile, project, arguments))
                        })
                        .map_err(tool_error)?,
                )
                .await
                .map_err(tool_error)?;
                // Channel authority is settled above; a skill that cannot be read is a
                // precise, correctable tool error, never a bare authority refusal.
                let (valid, profile, project, arguments) = value;
                let skill = arguments["skill"].as_str().unwrap_or("");
                return Ok(if valid {
                    crate::native_skills::read(&self.root, &profile, skill, project)
                } else {
                    crate::native_skills::invalid(skill, &profile, project)
                });
            }
            if name == "verification_submit" {
                let i = self.identity(true).await?;
                let task = i.task;
                let actor = c.channel_id.clone();
                let key = arguments["request_key"]
                    .as_str()
                    .ok_or(Error::Invalid)?
                    .to_owned();
                let replay=receive(self.executor.read(move|db|Ok(db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?)",(&task,&actor,&key),|r|r.get::<_,bool>(0))?)).map_err(tool_error)?).await.map_err(tool_error)?;
                if !replay {
                    if let Err(error) = crate::native_effects::verification_preflight(
                        self.root.clone(),
                        &arguments["verification"],
                    )
                    .await
                    {
                        return Ok(content_refusal("verification", error));
                    }
                }
            }
            if name == "plan_submit" {
                let i = self.identity(true).await?;
                if !same(&i, &expected) {
                    return Err(Error::Refused("channel_identity_changed"));
                }
                let task = i.task;
                let actor = c.channel_id.clone();
                let key = arguments["request_key"]
                    .as_str()
                    .ok_or(Error::Invalid)?
                    .to_owned();
                let replay=receive(self.executor.read(move|db|Ok(db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?)",
                    (&task,&actor,&key),|r|r.get::<_,bool>(0))?)).map_err(tool_error)?).await.map_err(tool_error)?;
                if !replay {
                    if let Err(error) =
                        crate::native_effects::plan_preflight(self.root.clone(), &arguments["plan"])
                            .await
                    {
                        return Ok(content_refusal("plan", error));
                    }
                }
            }
            if name == "result_submit" {
                let evidence = if channel.stage.as_deref() == Some("review")
                    || arguments["checks"]
                        .as_array()
                        .is_some_and(|checks| checks.iter().any(|c| c["outcome"] == "passed"))
                {
                    crate::native_effects::observe(self.root.clone())
                        .await
                        .map_err(tool_error)?
                } else {
                    EvidenceSnapshot::default()
                };
                let outcome = receive(
                    self.executor
                        .publication(move |store| {
                            let i = store.read(|db| {
                                channels::authenticate(db, &c.channel_id, c.secret(), true)
                            })?;
                            if !same(&i, &expected) {
                                return Err(CoreError::Refused("channel_identity_changed"));
                            }
                            let submitted = (|| {
                                let tools = cortex_engine::schema::catalogue(
                                    &i.role,
                                    i.profile.as_deref(),
                                    i.stage.as_deref(),
                                )
                                .map_err(engine_error_to_core)?;
                                let tool = tools
                                    .iter()
                                    .find(|t| t["name"] == name)
                                    .ok_or(CoreError::Refused("tool_refused"))?;
                                cortex_engine::schema::validate(&tool["inputSchema"], &arguments)
                                    .map_err(|_| CoreError::Refused("invalid_arguments"))?;
                                crate::native_results::submit(store, &c, &i, &arguments, evidence)
                            })();
                            // A refusal names exactly what is missing: report sections, or
                            // the items the worker still has to do in this same session.
                            let missing = match &submitted {
                                Err(error) => crate::native_results::refusal_missing(
                                    store,
                                    &c,
                                    &i,
                                    &arguments,
                                    error.code(),
                                ),
                                _ => None,
                            };
                            // Every refusal of an authenticated worker's result is retained
                            // by its code and named items, so the owner can see why no
                            // result exists and a continuation can repeat what is open.
                            if let Err(error) = &submitted {
                                let _ = crate::native_results::record_refusal(
                                    store,
                                    &c,
                                    &i,
                                    error.code(),
                                    missing.as_deref(),
                                );
                            }
                            Ok((submitted, missing))
                        })
                        .map_err(tool_error)?,
                )
                .await
                .map_err(tool_error)?;
                let (outcome, missing) = outcome;
                return match outcome {
                    Ok(value) => Ok(json!({"content":[{"type":"text","text":value.to_string()}],
                        "structuredContent":value,"isError":false})),
                    // A correctable content refusal goes back to the worker with its code
                    // and correction, so it can submit again in the same turn.
                    Err(error) if crate::native_results::correctable(error.code()) => Ok(
                        crate::native_results::refusal_reply_with(error.code(), missing),
                    ),
                    Err(error) => Err(tool_error(error)),
                };
            }
            let routing = self.routes.clone();
            let root = self.root.clone();
            let synthesis = matches!(name.as_str(), "synthesis_submit" | "answer_submit");
            let value = receive(
                self.executor
                    .write(move |db| {
                        let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
                        if !same(&i, &expected) {
                            return Err(CoreError::Refused("channel_identity_changed"));
                        }
                        let tools = cortex_engine::schema::catalogue(
                            &i.role,
                            i.profile.as_deref(),
                            i.stage.as_deref(),
                        )
                        .map_err(engine_error_to_core)?;
                        let tool = tools
                            .iter()
                            .find(|t| t["name"] == name)
                            .ok_or(CoreError::Refused("tool_refused"))?;
                        cortex_engine::schema::validate(&tool["inputSchema"], &arguments)
                            .map_err(engine_error_to_core)?;
                        if name == "synthesis_submit" && i.role == "coordinator" {
                            return synthesis_reply(db, &i.task, &arguments);
                        }
                        let key = arguments["request_key"]
                            .as_str()
                            .ok_or(CoreError::Refused("invalid_request_key"))?;
                        match name.as_str() {
                            "plan_submit" if i.role == "coordinator" => {
                                let replay:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?)",
                                    (&i.task,&c.channel_id,key),|r|r.get(0))?;
                                let hints=if !replay {
                                    let evidence=cortex_core::routing::evidence(db,&i.task,&root)?;
                                    if let Some(routes)=routing {routes.previews(&root,&arguments["plan"],&evidence)?}else{json!([])}
                                }else{json!([])};
                                lifecycle::plan::accept_routed_tx(db,&i.task,&c.channel_id,key,&arguments["plan"],&hints)
                            },
                            "user_question_request" if i.role == "coordinator" => {
                                lifecycle::skill_proposals::request(db, &root, &i.task, &c.channel_id, key, &arguments)
                            }
                            "ask_question" if i.role == "worker" => {
                                let forms = if let Some(a) = arguments["questions"].as_array() {
                                    a.clone()
                                } else {
                                    vec![arguments.clone()]
                                };
                                // A batch member's optional short label (its tab title).
                                let topics = forms
                                    .iter()
                                    .map(|f| f["topic"].as_str().map(str::to_owned))
                                    .collect::<Vec<_>>();
                                let forms = forms
                                    .into_iter()
                                    .map(|f| waits::QuestionForm {
                                        question: f["question"].as_str().unwrap().into(),
                                        options: f["options"]
                                            .as_array()
                                            .unwrap()
                                            .iter()
                                            .map(|s| s.as_str().unwrap().into())
                                            .collect(),
                                    })
                                    .collect::<Vec<_>>();
                                waits::request_tx_bound_topics(db, &i.task, &c.channel_id, key, &forms, &topics)
                            }
                            "verification_submit" if i.role=="coordinator" => lifecycle::reconciliation::submit_verification_tx(db,&i.task,&c.channel_id,key,&arguments),
                            "replan_request" | "scope_request" | "escalation_request" => lifecycle::domain::propose_replan_tx(db,&i.task,&c.channel_id,key,&name,&arguments),
                            "answer_submit" if i.role == "coordinator" => {
                                gap_reply(db, &i.task, lifecycle::reconciliation::submit_answer_tx(db,&i.task,&c.channel_id,key,&arguments))
                            }
                            _ => Err(CoreError::Refused("tool_refused")),
                        }
                    })
                    .map_err(tool_error)?,
            )
            .await
            .map_err(tool_error)?;
            if synthesis {
                return Ok(value);
            }
            Ok(
                json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":false}),
            )
        }
    }
}
/// A Core failure inside an authenticated tool call keeps its precise code.
fn tool_error(error: CoreError) -> Error {
    Error::Refused(error.code())
}
/// A plan or verification the runtime refuses before recording anything is a correctable
/// content error, not an authority failure: the coordinator gets the exact code and submits
/// a corrected proposal instead of giving up on `authority_refused`.
fn content_refusal(what: &str, error: CoreError) -> Value {
    let code = error.code();
    let hint = match code {
        "verification_trivial_command" => " A verification command only prints, reads, lists or searches (echo, cat, ls, grep, rg, find, test, true, read-only git and the like), so its exit status cannot prove the work. Use the project's documented check (test runner, build, linter, type checker) or another command that exits nonzero when the result is wrong.",
        "verification_executable_unavailable" => " A verification program was not found on the command search path that worker commands and the final verifier use (the project's own tool directories, then the owner's PATH); use a program that is installed there.",
        "read_only_scope_not_writable" => " A read-only step names a path that does not exist and that no dependent writer creates.",
        _ => "",
    };
    let value = json!({
        "code": code,
        "recorded": false,
        "message": format!("No {what} was recorded ({code}).{hint} Correct the proposal and submit it again."),
    });
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":true})
}
fn engine_error_to_core(_: Error) -> CoreError {
    CoreError::Refused("invalid_native_tool")
}

/// Called only after channel identity, coordinator role and schema validation.
/// These two pre-mutation content errors are correctable, not authority failures.
fn synthesis_reply(
    db: &rusqlite::Transaction<'_>,
    task: &str,
    arguments: &Value,
) -> CoreResult<Value> {
    gap_reply(
        db,
        task,
        lifecycle::reconciliation::submit_synthesis_tx(db, task, arguments),
    )
}
/// `synthesis_submit` and `answer_submit` share their disclosure refusals; both
/// are refused before any mutation and return the current gap names.
fn gap_reply(
    db: &rusqlite::Transaction<'_>,
    task: &str,
    submitted: CoreResult<Value>,
) -> CoreResult<Value> {
    let (value, is_error) = match submitted {
        Ok(value) => (value, false),
        Err(CoreError::Refused("synthesis_gaps_undisclosed")) => {
            let names = lifecycle::reconciliation::gaps(db, task)?
                .into_values()
                .collect::<Vec<_>>();
            (
                json!({
                    "code":"synthesis_gaps_undisclosed",
                    "recorded":false,
                    "required_not_verified":names,
                    "message":"Nothing was recorded. List each current unresolved check in not_verified using its exact name and a concrete blocker; names from an older revision do not close current gaps. Correct the proposal and submit again."
                }),
                true,
            )
        }
        Err(CoreError::Refused("synthesis_disclosure_in_text")) => (
            json!({
                "code":"synthesis_disclosure_in_text",
                "recorded":false,
                "message":"Nothing was recorded. Put verification-gap disclosures only in not_verified and remove the duplicate disclosure section from text, then submit the corrected proposal."
            }),
            true,
        ),
        Err(error) => return Err(error),
    };
    Ok(json!({"content":[{"type":"text","text":value.to_string()}],
        "structuredContent":value,"isError":is_error}))
}
/// The degenerate-turn watchdog: rules over the agent text one turn streams, measured
/// against retained production sessions. Ordinary messages stay far from every bound
/// (largest ordinary message 4,748 characters, one legitimate consultant message
/// 13,121; largest repeated-shingle share 0.045), while the degenerate turns observed
/// echoed the runtime's own prompt text, looped on the same few words, or streamed
/// tens of kilobytes without acting.
pub(crate) mod degenerate {
    use serde_json::Value;
    use std::collections::HashSet;
    use std::hash::{Hash, Hasher};

    /// Text only the runtime's own prompt carries (the recovery capsule header, the
    /// host's reminder wrapper, the runtime status line). An agent message quoting
    /// it is the model replaying its prompt as its reply.
    pub(crate) const ECHO_MARKERS: &[&str] = &[
        "Cortex retained-state recovery capsule",
        "<system-reminder>",
        "Runtime status:",
    ];
    /// A marker starting within this many characters of the message start is an echo
    /// (the observed replays began with a few stray characters before the header).
    pub(crate) const ECHO_OPENING: usize = 80;
    /// The repetition rule applies once a message has this many characters.
    pub(crate) const REPETITION_FLOOR: usize = 1_000;
    /// Share of sampled 16-character shingles already seen earlier in the message.
    pub(crate) const REPETITION_SHARE: f64 = 0.30;
    const SHINGLE: usize = 16;
    const STRIDE: usize = 4;
    /// One agent message with no tool call between its parts.
    pub(crate) const LENGTH_CEILING: usize = 16_000;

    #[derive(Debug, Clone, PartialEq)]
    pub(crate) struct Trip {
        pub rule: &'static str,
        pub message_id: Option<String>,
        pub chars: usize,
        pub repeated_share: Option<f64>,
    }
    impl Trip {
        pub(crate) fn refusal() -> Self {
            Trip {
                rule: "refusal",
                message_id: None,
                chars: 0,
                repeated_share: None,
            }
        }
    }

    /// An agent text chunk (message or thought) of an ACP `session/update`.
    pub(crate) fn agent_text(update: &Value) -> bool {
        matches!(
            update["sessionUpdate"].as_str(),
            Some("agent_message_chunk" | "agent_thought_chunk")
        )
    }

    /// The current agent message of one turn. A message is the run of
    /// `agent_message_chunk` updates with one `messageId` (or, without ids, the run
    /// between tool calls); a tool call ends it.
    #[derive(Default)]
    pub(crate) struct Watch {
        id: Option<String>,
        chars: Vec<char>,
        seen: HashSet<u64>,
        sampled: usize,
        repeated: usize,
        /// Character positions where an echo marker starts in this message.
        echoes: std::collections::BTreeSet<usize>,
    }
    impl Watch {
        pub(crate) fn observe(&mut self, update: &Value) -> Option<Trip> {
            match update["sessionUpdate"].as_str() {
                Some("agent_message_chunk") => {}
                Some("tool_call" | "tool_call_update") => {
                    self.start(None);
                    return None;
                }
                _ => return None,
            }
            let text = update["content"]["text"].as_str()?;
            let id = update["messageId"].as_str().map(str::to_owned);
            if id.is_some() && id != self.id {
                self.start(id);
            }
            self.push(text)
        }
        fn start(&mut self, id: Option<String>) {
            *self = Watch {
                id,
                ..Watch::default()
            };
        }
        fn share(&self) -> f64 {
            if self.sampled == 0 {
                0.0
            } else {
                self.repeated as f64 / self.sampled as f64
            }
        }
        fn trip(&self, rule: &'static str) -> Trip {
            Trip {
                rule,
                message_id: self.id.clone(),
                chars: self.chars.len(),
                repeated_share: (rule == "repetition")
                    .then(|| (self.share() * 1000.0).round() / 1000.0),
            }
        }
        pub(crate) fn push(&mut self, text: &str) -> Option<Trip> {
            let start = self.chars.len();
            self.chars.extend(text.chars());
            // Only the new text, and the tail a marker could straddle, is searched.
            let longest = ECHO_MARKERS
                .iter()
                .map(|m| m.chars().count())
                .max()
                .unwrap_or(1);
            let from = start.saturating_sub(longest - 1);
            let window: String = self.chars[from..].iter().collect();
            for marker in ECHO_MARKERS {
                for (byte, _) in window.match_indices(marker) {
                    self.echoes.insert(from + window[..byte].chars().count());
                }
            }
            // A replayed prompt opens the message with a marker (after a few stray
            // characters) or repeats markers; a reply that quotes one once inside its
            // own prose (likely when the project is this runtime's own source) is not.
            if self.echoes.iter().any(|&at| at < ECHO_OPENING) || self.echoes.len() >= 2 {
                return Some(self.trip("echo"));
            }
            while self.sampled * STRIDE + SHINGLE <= self.chars.len() {
                let at = self.sampled * STRIDE;
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                self.chars[at..at + SHINGLE].hash(&mut hasher);
                if !self.seen.insert(hasher.finish()) {
                    self.repeated += 1;
                }
                self.sampled += 1;
            }
            if self.chars.len() >= REPETITION_FLOOR && self.share() >= REPETITION_SHARE {
                return Some(self.trip("repetition"));
            }
            if self.chars.len() >= LENGTH_CEILING {
                return Some(self.trip("length"));
            }
            None
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use serde_json::json;

        fn chunk(id: &str, text: &str) -> Value {
            json!({"sessionUpdate":"agent_message_chunk","messageId":id,"content":{"type":"text","text":text}})
        }
        /// Streams `text` in engine-sized chunks and returns the first trip.
        fn stream(watch: &mut Watch, id: &str, text: &str) -> Option<Trip> {
            let chars: Vec<char> = text.chars().collect();
            chars
                .chunks(23)
                .find_map(|c| watch.observe(&chunk(id, &c.iter().collect::<String>())))
        }
        /// Ordinary prose that never repeats a 16-character window.
        fn prose(chars: usize) -> String {
            const WORDS: &[&str] = &[
                "the", "runtime", "review", "found", "that", "worker", "report", "module",
                "change", "test", "passes", "after", "fixing", "schema", "session", "record",
                "event", "plan", "budget", "owner", "answer", "question", "file", "path", "scope",
                "result", "check", "command", "output", "capsule", "context", "engine", "model",
                "route", "store", "commit", "branch", "diff", "line", "error", "warning", "build",
                "cargo", "crate", "function", "value", "field", "type", "trait", "async", "await",
                "lock", "queue", "stream", "chunk", "message", "turn", "cancel", "grace", "retry",
            ];
            let mut out = String::new();
            let mut n: u64 = 7;
            while out.chars().count() < chars {
                n = n
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                out.push_str(WORDS[(n >> 33) as usize % WORDS.len()]);
                out.push_str(if n % 11 == 0 { ". " } else { " " });
            }
            out
        }

        #[test]
        fn an_echoed_capsule_reminder_or_status_line_trips_echo() {
            for text in [
                "Understood. Cortex retained-state recovery capsule. The following JSON is recorded data",
                "Done.\n<system-reminder>\nThe user's settings changed.\n</system-reminder>",
                "Plan accepted.\n\nRuntime status: PLANNING, revision 3",
            ] {
                let mut watch = Watch::default();
                let trip = stream(&mut watch, "m1", text).expect(text);
                assert_eq!(trip.rule, "echo");
                assert_eq!(trip.message_id.as_deref(), Some("m1"));
            }
            // A marker split across two chunks is still found.
            let mut watch = Watch::default();
            assert!(watch.observe(&chunk("m", "ok <system-")).is_none());
            assert_eq!(
                watch.observe(&chunk("m", "reminder> x")).unwrap().rule,
                "echo"
            );
        }
        #[test]
        fn a_marker_quoted_once_inside_prose_is_not_an_echo_but_a_repeated_one_is() {
            // A worker explaining this runtime's own source may quote a marker once.
            let quoted = format!(
                "{} The engine wraps hook output in a <system-reminder> tag, so the parser skips it. {}",
                prose(400),
                prose(200)
            );
            let mut watch = Watch::default();
            assert_eq!(stream(&mut watch, "m", &quoted), None);
            // The same marker repeated in one message is a replayed prompt.
            let repeated = format!("{quoted} Then another <system-reminder> block followed.");
            let mut watch = Watch::default();
            assert_eq!(stream(&mut watch, "m", &repeated).unwrap().rule, "echo");
            // The observed capsule replay opened with stray characters before the header.
            let mut watch = Watch::default();
            let replay = "oftware CoreDump\n\nCopyrightCopyright © Kagi\n\nCortex retained-state recovery capsule. {\"access_mode\":\"auto\"}";
            assert_eq!(stream(&mut watch, "m", replay).unwrap().rule, "echo");
        }
        #[test]
        fn a_looping_message_trips_repetition_but_ordinary_text_does_not() {
            // The observed loop: one handle repeated without end.
            let mut watch = Watch::default();
            let trip = stream(&mut watch, "m", &"guner@guner@".repeat(400)).unwrap();
            assert_eq!(trip.rule, "repetition");
            assert!(
                trip.chars >= REPETITION_FLOOR && trip.chars < 1_100,
                "{trip:?}"
            );
            assert!(trip.repeated_share.unwrap() >= REPETITION_SHARE);
            // A paragraph pasted again and again, as a stuck model repeats itself.
            let mut watch = Watch::default();
            let paragraph = prose(300);
            assert_eq!(
                stream(&mut watch, "m", &paragraph.repeat(12)).unwrap().rule,
                "repetition"
            );
            // A long ordinary message, with a little repeated markup, stays clear.
            let mut watch = Watch::default();
            let ordinary = format!(
                "{}\n| a | b |\n| --- | --- |\n| 1 | 2 |\n{}",
                prose(6_000),
                prose(6_000)
            );
            assert_eq!(stream(&mut watch, "m", &ordinary), None);
            assert!(watch.share() < 0.1, "{}", watch.share());
            // Under the floor even a short repeated phrase is left alone.
            let mut watch = Watch::default();
            assert_eq!(stream(&mut watch, "m", &"ok ".repeat(300)), None);
        }
        #[test]
        fn one_long_message_without_a_tool_call_trips_length() {
            let mut watch = Watch::default();
            // The 13,121-character consultant message is legitimate.
            assert_eq!(stream(&mut watch, "c", &prose(13_121)), None);
            let mut watch = Watch::default();
            let trip = stream(&mut watch, "m", &prose(20_000)).unwrap();
            assert_eq!(trip.rule, "length");
            assert!((LENGTH_CEILING..LENGTH_CEILING + 23).contains(&trip.chars));
            // A tool call between the parts ends the message: no single message is long.
            let mut watch = Watch::default();
            for _ in 0..3 {
                assert_eq!(stream(&mut watch, "m", &prose(9_000)), None);
                assert!(watch
                    .observe(&json!({"sessionUpdate":"tool_call","toolCallId":"t","status":"in_progress"}))
                    .is_none());
            }
            // So does a new message id.
            let mut watch = Watch::default();
            assert_eq!(stream(&mut watch, "a", &prose(9_000)), None);
            assert_eq!(stream(&mut watch, "b", &prose(9_000)), None);
        }
        #[test]
        fn only_agent_message_text_is_watched() {
            let mut watch = Watch::default();
            let thought = json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"<system-reminder>"}});
            assert!(watch.observe(&thought).is_none());
            let tool =
                json!({"sessionUpdate":"tool_call","toolCallId":"t","title":"Runtime status:"});
            assert!(watch.observe(&tool).is_none());
            assert!(agent_text(&thought));
            assert!(!agent_text(&tool));
        }
    }
}
