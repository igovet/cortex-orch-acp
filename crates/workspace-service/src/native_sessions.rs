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
    let name = entry["name"].as_str().map(str::trim).filter(|n| !n.is_empty() && *n != model)?;
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
    pub(super) mcp: crate::native_mcp::Registry,
    pub(super) routes: std::sync::RwLock<Option<Arc<dyn crate::native_runtime::NativeRoutes>>>,
    pub(super) permissions: std::sync::RwLock<Option<Arc<dyn ClientPermissions>>>,
    pub(super) executor: Arc<StoreExecutor>,
    pub(super) root: PathBuf,
    pub(super) driver: tokio::sync::Mutex<()>,
    pub(super) commands: crate::native_commands::Running,
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
            mcp,
            routes: std::sync::RwLock::new(None),
            permissions: std::sync::RwLock::new(None),
            executor,
            root,
            driver: tokio::sync::Mutex::new(()),
            commands: Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new())),
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
        if route.launch.cwd != self.root
            || route.connection.is_empty()
            || route.connection_limit == 0
            || route.instructions.trim().is_empty()
            || route.model.is_empty()
            || route.deadline.is_zero()
        {
            return Err(CoreError::Refused("invalid_session_route"));
        }
        let connection = route.connection.clone();
        let limit = route.connection_limit;
        let engine_kind = route.launch.engine;
        let (credential, reservation, instructions) = receive(self.executor.write(move |db| {
            let credential = channels::issue_tx(
                db,
                &task,
                &owner,
                match &who {
                    SessionRole::Coordinator => Principal::Coordinator,
                    SessionRole::Worker { assignment } => Principal::Worker { assignment },
                },
            )?;
            let identity =
                channels::authenticate(db, &credential.channel_id, credential.secret(), false)?;
            cortex_engine::schema::catalogue(
                &identity.role,
                identity.profile.as_deref(),
                identity.stage.as_deref(),
            )
            .map_err(|_| CoreError::Refused("unknown_session_role"))?;
            let instructions = crate::native_instructions::compose(
                identity.profile.as_deref().unwrap_or("orchestrator"),
                identity.stage.as_deref(),
                engine_kind,
            )?;
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
            Ok((credential, reservation, instructions))
        })?)
        .await?;
        let policy = Arc::new(CorePolicy {
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
        let (identity, scopes) = receive(self.executor.read(move |db| {
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
        .await?;
        // The owner-facing stop names the role whose session could not start.
        let blocker = if identity.role == "coordinator" {
            "coordinator_session_start_failed"
        } else {
            "native_session_start_failed"
        };
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
                engine = Some(Engine::launch(launch, policy.clone()).await?);
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
                    .then(crate::native_engine_policy::claude_session_meta);
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
                // permission requests, and a read-only channel without a read-only
                // or asking mode does not start.
                let mode = crate::native_engine_policy::native_mode(
                    engine_kind, read_only, &s.config_options())?;
                e.configure(&s,"mode",mode,route.deadline).await?;
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
                receive(self.executor.write(move |db| {
                    let i = channels::authenticate(db,&c.channel_id,c.secret(),false)?;
                    lifecycle::event_tx(db,&i.task,"NATIVE_ACCESS_MODE_CONFIRMED",
                        json!({"channel_id":c.channel_id,"mode":selected,"access":access,"session":i.session}))?;
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
            if let Some(b) = broker.take() {
                let _ = b.shutdown().await;
            }
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
            let failure = StartFailure {
                attempt,
                phase,
                // A refusal with a stable code (`restricted_mode_unavailable`, ...)
                // records that code; it is static and secret-free.
                error: match &error {
                    Error::Refused(code) => (*code).to_owned(),
                    other => other.to_string(),
                },
                role: identity.role.clone(),
                engine: engine_kind.name(),
                blocker: (!retry).then_some(blocker),
                diagnostics: diagnostics.map(|d| d.snapshot()),
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
            let recorded = record_start_failure(&self.executor, &credential, failure).await;
            settle(&self.executor, credential.clone(), reservation, observation).await?;
            recorded?;
            return Err(CoreError::Refused(blocker));
        };
        Ok(SessionDriver {
            executor: self.executor.clone(),
            credential,
            reservation,
            engine,
            broker,
            external,
            session,
            instructions: format!(
                "{instructions}{}\n\n{}",
                // Only roles served project_read see the project's own skills.
                crate::native_skills::project_files(
                    &identity.role,
                    identity.profile.as_deref(),
                    identity.stage.as_deref()
                )
                .then(|| crate::native_skills::section(&self.root))
                .flatten()
                .unwrap_or_default(),
                route.instructions
            ),
            engine_kind,
            policy,
            delivered_attachments: std::sync::Mutex::new(std::collections::HashSet::new()),
        })
    }
}
pub enum SessionRole {
    Coordinator,
    Worker { assignment: String },
}
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
pub struct SessionDriver {
    executor: Arc<StoreExecutor>,
    credential: Credential,
    reservation: String,
    engine: Engine,
    broker: Broker,
    external: Vec<crate::native_mcp::Attachment>,
    session: Session,
    instructions: String,
    engine_kind: cortex_engine::environment::EngineKind,
    policy: Arc<CorePolicy>,
    /// Retained attachment bodies already delivered to this engine session.
    delivered_attachments: std::sync::Mutex<std::collections::HashSet<String>>,
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
        let prompt = format!("{}\n\n{}\n\n{}", self.instructions, capsule, text);
        let task = self
            .policy
            .check(true)
            .await
            .map_err(engine_error_to_core)?
            .task;
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
        let pending = self.engine.prompt_content(&self.session, blocks);
        tokio::pin!(pending);
        let mut updates = 0;
        let mut compacted = false;
        let result = loop {
            tokio::select! {
                result=&mut pending=>break result,
                event=self.engine.next_event()=>{
                    match event {
                        Some(Event::Update(params))=>{
                            compacted |= self.record_update(params).await?;
                            updates+=1;
                        }
                        Some(Event::TransportClosed)|None=>break Err(Error::Closed),
                        _=>{}
                    }
                }
            }
        };
        // The prompt reply may win select while earlier notifications remain
        // queued. Persist those too; never count a reply as activity evidence.
        for event in self.engine.drain_events().await {
            if let Event::Update(params) = event {
                compacted |= self.record_update(params).await?;
                updates += 1;
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
        receive(self.executor.write(move |db| {
            // A valid tool may already have advanced or revoked this channel.
            let task: String = db.query_row(
                "SELECT task_id FROM trusted_channels WHERE channel_id=?",
                [&credential.channel_id],
                |r| r.get(0),
            )?;
            lifecycle::event_tx(
                db,
                &task,
                "NATIVE_TURN_SETTLED",
                json!({"channel_id":credential.channel_id,"stop_reason":recorded}),
            )?;
            Ok(())
        })?)
        .await?;
        Ok(Turn {
            stop_reason: reason,
            updates,
            compacted,
        })
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
            return receive(self.executor.write(move|db|{
                let i=channels::authenticate(db,&c.channel_id,c.secret(),true)?;
                let session=format!("native:{}:{}",i.attempt,i.session.as_deref().unwrap());
                let seen:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM compaction_events_seen WHERE engine_session_ref=? AND evidence_id=? AND generation_id IS NOT NULL)",(&session,&evidence),|r|r.get(0))?;
                if phase!="completed" {
                    db.execute("INSERT OR IGNORE INTO compaction_events_seen VALUES (?,?,?,?,?,NULL)",(&session,&evidence,&i.task,&phase,cortex_core::now()))?;
                    return Ok(false);
                }
                cortex_core::recovery_context::generation_tx(db,&c,"observed_compaction",Some(&evidence))?;
                Ok(!seen)
            })?).await;
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
    pub async fn cancel(&self) -> CoreResult<()> {
        self.engine
            .cancel(&self.session)
            .await
            .map_err(|_| CoreError::Refused("native_cancel_outcome_unknown"))
    }
    pub(crate) async fn wait_state(&self) -> CoreResult<Option<String>> {
        let c = self.credential.clone();
        receive(self.executor.write(move |db| {
            let i: (String, String) = db.query_row(
                "SELECT task_id,idempotency_key FROM delegation_provisioning WHERE channel_id=?",
                [&c.channel_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            if let Some(state) =
                lifecycle::skill_proposals::continuation(db, &i.0, &c.channel_id, false)?
            {
                return Ok(Some(state));
            }
            Ok(waits::current_tx(db, &i.0, &i.1)?
                .and_then(|w| w["status"].as_str().map(str::to_owned)))
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
        let delivery = receive(self.executor.write(move |db| {
            let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
            let assignment = i.assignment.ok_or(CoreError::Refused("worker_required"))?;
            waits::claim_tx(db, &i.task, &assignment, &session)
        })?)
        .await?;
        let Some(delivery) = delivery else {
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
            let current = channels::authenticate(db, &c.channel_id, c.secret(), true).is_ok();
            let failure = if delivered && current || recorded {
                None
            } else {
                Some("answer_delivery_unknown")
            };
            let finished = waits::finish_tx(
                db,
                &task,
                delivery["wait"]["wait_id"].as_str().unwrap(),
                delivery["wait"]["delivery_id"].as_str().unwrap(),
                failure,
            )?;
            Ok(finished && failure.is_none())
        })?)
        .await?;
        if !settled {
            return Err(CoreError::Refused("native_answer_delivery_unconfirmed"));
        }
        Ok(true)
    }
    pub async fn shutdown(self) -> CoreResult<()> {
        let Self {
            executor,
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
        settle(&executor, credential, reservation, observation).await?;
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
            use cortex_engine::permission::{self, Action, Fallback};
            let c = self.credential.clone();
            let expected_session = session.to_owned();
            let session = session.to_owned();
            let id = request_id.clone();
            let tool = permission::tool_name(request).map(str::to_owned);
            let kind = permission::tool_kind(request).map(str::to_owned);
            let incoming = receive(self.executor.write(move |db| {
                let i=channels::authenticate(db,&c.channel_id,c.secret(),true)?;
                if i.session.as_deref()!=Some(&session) {return Err(CoreError::Refused("session_mismatch"));}
                let mut q=db.prepare("SELECT session_id FROM acp_host_sessions WHERE task_id=?
                    AND json_extract(client_info,'$.name')='cortex-rust-acp'")?;
                let sessions=q.query_map([&i.task],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
                if sessions.len()>1 {return Err(CoreError::Refused("incoming_session_binding_conflict"));}
                lifecycle::event_tx(db,&i.task,"NATIVE_PERMISSION_REQUESTED",
                    json!({"channel_id":c.channel_id,"session":session,"request_id":id,"turn_generation":generation,
                        "tool":tool,"kind":kind}))?;
                Ok((sessions.into_iter().next(),cortex_core::access::current(db,&i.task)?))
            }).map_err(engine_error)?).await.map_err(engine_error)?;
            let (incoming, access) = incoming;
            let i = self.check(true).await?;
            let read_only = i.role != "worker"
                || cortex_core::scheduler::policy::read_only(
                    i.profile.as_deref().unwrap_or(""),
                    i.stage.as_deref().unwrap_or(""),
                );
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
            let result = self.call(credentials, channel, name, arguments).await;
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
    /// `read_batch`: up to eight allowlisted evidence reads, preflighted together,
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
            let member = self.call(credentials, channel, &tool, call["arguments"].clone());
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
                let data = reply["structuredContent"].clone();
                let bytes = serde_json::to_vec(&data).unwrap_or_default();
                let size = bytes.len();
                if size <= crate::native_batch::ITEM_BYTES {
                    item["status"] = json!("ok");
                    item["result"] = data;
                } else if tool == "read_large_output" {
                    item["status"] = json!("error");
                    item["error"] = crate::native_tool_errors::reply("batch_result_limit")
                        ["structuredContent"]
                        .clone();
                } else {
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
                                crate::native_commands::defer_json_over(
                                    store,
                                    &i,
                                    data,
                                    crate::native_batch::ITEM_BYTES,
                                )
                            })
                            .map_err(tool_error)?,
                    )
                    .await
                    .map_err(tool_error)?;
                    item["status"] = json!("deferred");
                    item["large_output_id"] = deferred["large_output_id"].clone();
                    item["byte_count"] = deferred["byte_count"].clone();
                    item["result_sha256"] = json!(crate::native_project::digest(&bytes));
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
    /// Boxed so a read_batch member can re-enter the same dispatch.
    fn call<'a>(
        &'a self,
        credentials: &'a Credentials,
        channel: &'a Channel,
        name: &'a str,
        arguments: Value,
    ) -> FutureResult<'a, Value> {
        Box::pin(self.dispatch_call(credentials, channel, name, arguments))
    }
    async fn dispatch_call(
        &self,
        credentials: &Credentials,
        channel: &Channel,
        name: &str,
        arguments: Value,
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
                let result = receive(
                    self.executor
                        .publication(move |store| {
                            let i = store.read(|db| {
                                channels::authenticate(db, &c.channel_id, c.secret(), true)
                            })?;
                            if !same(&i, &expected) {
                                return Err(CoreError::Refused("channel_identity_changed"));
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
                return crate::native_commands::execute(
                    self.executor.clone(),
                    self.credential.clone(),
                    self.root.clone(),
                    self.permissions.clone(),
                    self.protected_roots.clone(),
                    self.commands.clone(),
                    &name,
                    arguments,
                )
                .await
                .map_err(tool_error);
            }
            if crate::native_project::supports(&name) || crate::native_sources::supports(&name) {
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
                                crate::native_project::execute(store, &i, &c, &name, &arguments)
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
                            // Every refusal of an authenticated worker's result is retained
                            // by its code alone, so the owner can see why no result exists.
                            if let Err(error) = &submitted {
                                let _ = crate::native_results::record_refusal(
                                    store,
                                    &c,
                                    &i,
                                    error.code(),
                                );
                            }
                            // A structural refusal names exactly the missing report sections.
                            let missing = match &submitted {
                                Err(error)
                                    if error.code()
                                        == cortex_core::assignment_brief::SECTIONS_MISSING =>
                                {
                                    Some(crate::native_results::missing_sections(
                                        store, &i, &arguments,
                                    ))
                                }
                                _ => None,
                            };
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
