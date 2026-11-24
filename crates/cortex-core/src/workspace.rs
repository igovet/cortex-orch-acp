//! Authenticated desktop drafts and trusted native execution bindings. No engine effects.
//! Wire-1 compatibility is deliberate: the retained adapter shares this receipt ledger.
use crate::{error::refuse, store::append_public_tx, CoreError, CoreResult, Store};
#[path = "workspace_control.rs"]
pub mod control;
#[path = "workspace_edits.rs"]
pub mod edits;
#[path = "workspace_worktrees.rs"]
pub mod worktrees;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};

/// Bind only persisted, explicitly native runtime facts. This is a trusted host
/// hook, never model JSON authority and never an engine/session launch.
pub fn bind_native_execution(
    store: &Store,
    project: &str,
    actor: &str,
    draft: &str,
    task: &str,
    owner: &crate::ownership::OwnerEpoch,
) -> CoreResult<String> {
    store.write(|db| bind_native_execution_tx(db, store, project, actor, draft, task, owner))
}
fn bind_native_execution_tx(
    db: &rusqlite::Transaction<'_>,
    store: &Store,
    project: &str,
    actor: &str,
    draft: &str,
    task: &str,
    owner: &crate::ownership::OwnerEpoch,
) -> CoreResult<String> {
    crate::ownership::check_tx(db, task, owner)?;
    let root = store.key().root().to_string_lossy();
    let row: Option<(Option<String>, i64)> = db
        .query_row(
            "SELECT t.execution_id,t.version FROM workspace_tasks t
             JOIN workspace_projects p USING(project_id)
             WHERE t.task_id=? AND p.project_id=? AND p.project_root=? AND p.activated_by=?",
            (draft, project, root.as_ref(), actor),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (existing, version) = row.ok_or(CoreError::Refused("unauthorized"))?;
    let native: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM tasks t JOIN runtime_tasks r ON t.id=r.task_id
             WHERE t.id=? AND t.project_root=? AND EXISTS(SELECT 1 FROM runtime_events e
             WHERE e.task_id=t.id AND e.type IN ('TASK_CREATED','NATIVE_TASK_ADOPTED') AND json_extract(e.payload_json,'$.execution_host')='rust'))",
            (task, root.as_ref()), |r| r.get(0))?;
    if !native || draft == task {
        return refuse("native_binding_evidence_invalid");
    }
    if let Some(execution) = existing {
        let valid: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM workspace_executions e JOIN acp_host_sessions s ON s.session_id=e.acp_session_id
                 WHERE e.execution_id=? AND e.task_id=? AND e.runtime_task_id=? AND e.owner_epoch=?
                 AND s.task_id=e.runtime_task_id AND s.project_root=?)",
                (&execution,draft,task,owner.epoch,root.as_ref()), |r| r.get(0))?;
        return if valid {
            Ok(execution)
        } else {
            refuse("native_binding_conflict")
        };
    }
    let used: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspace_executions WHERE runtime_task_id=?)",
        [task],
        |r| r.get(0),
    )?;
    if used {
        return refuse("native_binding_conflict");
    }
    let execution = format!("we_{}", uuid::Uuid::new_v4().simple());
    // This is a logical display session, distinct from every real child ACP
    // session retained in delegation_provisioning. It is never sent to an engine.
    let session = format!("native_{}", uuid::Uuid::new_v4().simple());
    db.execute(
        "INSERT INTO acp_host_sessions VALUES (?,?,?,?,'{}','[]',?,NULL)",
        (
            &session,
            root.as_ref(),
            task,
            r#"{"name":"cortex-native"}"#,
            crate::now(),
        ),
    )?;
    db.execute(
        "INSERT INTO workspace_executions VALUES (?,?,?,?,?,1,?)",
        (&execution, draft, task, &session, owner.epoch, crate::now()),
    )?;
    db.execute(
        "UPDATE workspace_tasks SET execution_id=?,version=version+1 WHERE task_id=?",
        (&execution, draft),
    )?;
    db.execute(
        "UPDATE workspace_projects SET version=version+1 WHERE project_id=?",
        [project],
    )?;
    append_public_tx(db, project, &execution, "execution", 1)?;
    append_public_tx(db, project, draft, "task", version + 1)?;
    Ok(execution)
}

/// Reconcile a previously committed Start even if route qualification/capacity changed.
pub fn replay_native_start(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
) -> CoreResult<Option<Value>> {
    replay_native_start_with_policy(store, project, actor, command, None)
}

pub fn replay_native_start_with_policy(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    policy: Option<NativeExecutionPolicy>,
) -> CoreResult<Option<Value>> {
    let digest = start_digest(command, None, None, policy)?;
    store.read(|db| {
        let binding: Option<(String,String)> = db.query_row(
            "SELECT project_root,activated_by FROM workspace_projects WHERE project_id=?",
            [project], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        if binding != Some((store.key().root().to_string_lossy().into_owned(),actor.into())) {
            return refuse("unauthorized");
        }
        let request = command["requestId"].as_str().ok_or(CoreError::Refused("invalid_request"))?;
        let prior: Option<(String,String)> = db.query_row(
            "SELECT digest,receipt_json FROM workspace_requests WHERE project_id=? AND actor_id=? AND request_id=?",
            (project,actor,request), |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        if prior.is_none() {
            worktrees::check_request(db, project, actor, request, &digest, true)?;
        }
        prior.map(|(old,encoded)| {
            if old != digest { return refuse("payload_conflict"); }
            Ok(serde_json::from_str(&encoded)?)
        }).transpose()
    })
}

/// Read saved bytes from the same activated project/task options as Start.
pub fn draft_attachments(
    store: &Store,
    project: &str,
    actor: &str,
    task: &str,
) -> CoreResult<Value> {
    store.read(|db| {
        let encoded: Option<String> = db
            .query_row(
                "SELECT o.options_json FROM workspace_task_options o
             JOIN workspace_tasks t USING(task_id) JOIN workspace_projects p USING(project_id)
             WHERE t.task_id=? AND p.project_id=? AND p.project_root=? AND p.activated_by=?",
                (
                    task,
                    project,
                    store.key().root().to_string_lossy().as_ref(),
                    actor,
                ),
                |r| r.get(0),
            )
            .optional()?;
        let options: Value =
            serde_json::from_str(&encoded.ok_or(CoreError::Refused("task_not_found"))?)?;
        let files = options
            .get("attachments")
            .cloned()
            .unwrap_or_else(|| json!([]));
        workspace_protocol::validate("Attachments", &files)
            .map_err(|_| CoreError::Refused("native_attachments_unavailable"))?;
        Ok(files)
    })
}

/// Owner choices saved with a desktop draft (`{model, effort, mode, language}`),
/// validated with the same `CreatePayload` contract as Python `control.apply_settings`.
fn saved_settings(options: &Value) -> CoreResult<Option<Value>> {
    let Some(settings) = options.get("settings") else {
        return Ok(None);
    };
    workspace_protocol::validate(
        "CreatePayload",
        &json!({"description":"options","settings":settings}),
    )
    .map_err(|_| CoreError::Refused("native_task_settings_invalid"))?;
    Ok(Some(settings.clone()))
}

/// Read a draft's saved settings from the same activated project/task options as
/// Start, so the host can qualify the chosen route before any admission effect.
/// A missing draft is `None`: admission reports the authoritative refusal.
pub fn draft_settings(
    store: &Store,
    project: &str,
    actor: &str,
    task: &str,
) -> CoreResult<Option<Value>> {
    store.read(|db| {
        let encoded: Option<String> = db
            .query_row(
                "SELECT o.options_json FROM workspace_task_options o
             JOIN workspace_tasks t USING(task_id) JOIN workspace_projects p USING(project_id)
             WHERE t.task_id=? AND p.project_id=? AND p.project_root=? AND p.activated_by=?",
                (
                    task,
                    project,
                    store.key().root().to_string_lossy().as_ref(),
                    actor,
                ),
                |r| r.get(0),
            )
            .optional()?;
        match encoded {
            Some(encoded) => saved_settings(&serde_json::from_str(&encoded)?),
            None => Ok(None),
        }
    })
}

/// A registered desktop host reads the saved kind; a request never supplies
/// checkout authority or selects a filesystem path.
pub fn draft_worktree_selected(store: &Store, project: &str, actor: &str, task: &str) -> CoreResult<bool> {
    store.read(|db| Ok(db.query_row(
        "SELECT json_extract(o.options_json,'$.checkout.kind') IS 'worktree'
         FROM workspace_task_options o JOIN workspace_tasks t USING(task_id)
         JOIN workspace_projects p USING(project_id)
         WHERE t.task_id=? AND p.project_id=? AND p.project_root=? AND p.activated_by=?",
        (task,project,store.key().root().to_str(),actor), |r| r.get(0),
    ).optional()?.unwrap_or(false)))
}

/// Trusted host option; deliberately not deserializable from commands, plans or tools.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeExecutionPolicy {
    AutomaticContinuation,
    /// A read-only sprint planning task (see `lifecycle::sprint_planning`).
    SprintPlanning,
}

fn start_digest(
    command: &Value,
    client: Option<(&str, &Value)>,
    capture: Option<&crate::input::Capture>,
    policy: Option<NativeExecutionPolicy>,
) -> CoreResult<String> {
    let mut input = match client {
        Some((session, settings)) => {
            json!({"command":command,"session":session,"settings":settings,"capture":capture.map(|c|c.fingerprint())})
        }
        None => command.clone(),
    };
    if policy == Some(NativeExecutionPolicy::AutomaticContinuation) {
        input = json!({"start":input, "policy":crate::lifecycle::automatic::POLICY});
    }
    if policy == Some(NativeExecutionPolicy::SprintPlanning) {
        input = json!({"start":input, "policy":crate::lifecycle::sprint_planning::POLICY});
    }
    Ok(crate::digest(&serde_json::to_vec(&input)?))
}

/// Atomic desktop admission. Provisioning occurs only after this committed receipt.
pub fn start_native(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
) -> CoreResult<Value> {
    start_native_client(store, project, actor, command, None)
}

/// Trusted ingress admission. The client session and initial settings commit with
/// the same native Start receipt, before the shared host may provision a process.
pub fn start_native_client(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    client: Option<(&str, &Value)>,
) -> CoreResult<Value> {
    start_native_input(store, project, actor, command, client, None)
}
pub fn start_native_input(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    client: Option<(&str, &Value)>,
    capture: Option<&crate::input::Capture>,
) -> CoreResult<Value> {
    start_native_input_with_policy(store, project, actor, command, client, capture, None)
}

pub fn start_native_input_with_policy(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    client: Option<(&str, &Value)>,
    capture: Option<&crate::input::Capture>,
    policy: Option<NativeExecutionPolicy>,
) -> CoreResult<Value> {
    admit_native_start(store, project, actor, command, client, capture, policy, None, None)
}

/// The first execution of a never-started Workspace task closed from the board.
struct StartRework<'a> {
    request: &'a Value,
    capture: Option<&'a crate::input::Capture>,
    history: Value,
}

fn start_rework_digest(request: &Value) -> CoreResult<String> {
    Ok(crate::digest(&serde_json::to_vec(
        &json!({"operation":"workspace_draft_rework","request":request}),
    )?))
}

pub fn initial_rework_replay(
    store: &Store, project: &str, actor: &str, request: &Value,
) -> CoreResult<Option<Value>> {
    let root = store.key().root().to_string_lossy();
    store.read(|db| {
        let prior: Option<(String,String)> = db.query_row(
            "SELECT r.digest,r.receipt_json FROM workspace_requests r JOIN workspace_projects p USING(project_id)
             WHERE r.project_id=? AND p.project_root=? AND p.activated_by=? AND r.actor_id=? AND r.request_id=?",
            (project, &*root, actor, actor, request["requestId"].as_str()), |r| Ok((r.get(0)?,r.get(1)?)),
        ).optional()?;
        prior.map(|(digest,body)| {
            if digest != start_rework_digest(request)? {
                return refuse("payload_conflict");
            }
            Ok(serde_json::from_str(&body)?)
        }).transpose()
    })
}

/// Only the Workspace owner adapter supplies closed-draft evidence, after
/// checking its archive and disposition under both its Git and machine locks.
pub fn start_native_rework(
    store: &Store,
    project: &str,
    actor: &str,
    request: &Value,
    capture: Option<&crate::input::Capture>,
    history: Value,
    machine_root: Option<&std::path::Path>,
) -> CoreResult<Value> {
    let capture = capture.filter(|c| !c.blocks.is_empty());
    if request["action"] != "rework" || request["projectId"] != project
        || !request["executionId"].is_null() || !request["ownerEpoch"].is_null()
    {
        return refuse("invalid_request");
    }
    if request.get("capture") != capture.map(|c| c.fingerprint()).as_ref() {
        return refuse("invalid_input_capture");
    }
    control::checked_owner_text(request)?;
    let command = json!({"protocol":{"major":1,"minor":8},"operation":"tasks.start",
        "projectId":project,"actorId":actor,"target":request["taskId"],
        "requestId":request["requestId"],"expectedVersion":request["expectedVersion"],
        "ownerEpoch":null,"payload":{}});
    let rework = Some(StartRework { request, capture, history });
    if let Some(machine_root) = machine_root {
        return start_worktree(store, project, actor, &command, machine_root, rework);
    }
    admit_native_start(store, project, actor, &command, None, None, None, None, rework)
}

/// Provision a saved worktree draft before admitting it. Only a checkout-aware
/// desktop host may call this entry point. It must hold its logical repository
/// Git lock through the whole call. Incoming ACP and automatic Sprint starts
/// must continue to use the non-provisioning entry point.
pub fn start_native_worktree(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    machine_root: &std::path::Path,
) -> CoreResult<Value> {
    start_worktree(store, project, actor, command, machine_root, None)
}

fn start_worktree(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    machine_root: &std::path::Path,
    rework: Option<StartRework<'_>>,
) -> CoreResult<Value> {
    workspace_protocol::validate("Command", command)
        .map_err(|_| CoreError::Refused("invalid_workspace_request"))?;
    if command["operation"] != "tasks.start" || command["projectId"] != project
        || command["actorId"] != actor || command["protocol"]["major"] != 1
    {
        return refuse("unauthorized");
    }
    let request = command["requestId"].as_str().ok_or(CoreError::Refused("invalid_request"))?;
    let draft = command["target"].as_str().ok_or(CoreError::Refused("invalid_target"))?;
    let digest = match &rework {
        Some(r) => start_rework_digest(r.request)?,
        None => start_digest(command, None, None, None)?,
    };
    let reserve = worktrees::reserve(store, project, actor, request, draft, &digest, command)?;
    use worktrees::Reservation::*;
    let (plan, witness) = match reserve {
        Replay(receipt) => return Ok(json!({"receipt":receipt,"fresh":false})),
        Refused(error) => return refuse(error),
        Observed(plan, witness) => {
            if plan.verify_initial(store.key().root(), &witness).is_err() {
                worktrees::mark(store, project, actor, request, "uncertain", "worktree_uncertain")?;
                return refuse("worktree_uncertain");
            }
            (plan, witness)
        }
        New(title) => {
            let plan = match worktrees::plan(store.key().root(), machine_root, project, draft, &title) {
                Ok(plan) => plan,
                Err(error) => {
                    worktrees::mark(store, project, actor, request, "failed-known", error.code())?;
                    return Err(error);
                }
            };
            worktrees::save_plan(store, project, actor, request, &plan)?;
            let witness = match plan.provision(store.key().root()) {
                Ok(witness) => witness,
                Err(_) => {
                    worktrees::mark(store, project, actor, request, "uncertain", "worktree_uncertain")?;
                    return refuse("worktree_uncertain");
                }
            };
            worktrees::save_witness(store, project, actor, request, &witness)?;
            (plan, witness)
        }
    };
    admit_native_start(store, project, actor, command, None, None, None, Some((&plan, &witness)), rework)
}

#[allow(clippy::too_many_arguments)]
fn admit_native_start(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    client: Option<(&str, &Value)>,
    capture: Option<&crate::input::Capture>,
    policy: Option<NativeExecutionPolicy>,
    worktree: Option<(&worktrees::Worktree, &worktrees::Witness)>,
    rework: Option<StartRework<'_>>,
) -> CoreResult<Value> {
    workspace_protocol::validate("Command", command)
        .map_err(|_| CoreError::Refused("invalid_workspace_request"))?;
    if command["operation"] != "tasks.start"
        || command["projectId"] != project
        || command["actorId"] != actor
        || command["protocol"]["major"] != 1
    {
        return refuse("unauthorized");
    }
    let request = command["requestId"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_request"))?;
    let draft = command["target"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_target"))?;
    let digest = match &rework {
        Some(r) => start_rework_digest(r.request)?,
        None => start_digest(command, client, capture, policy)?,
    };
    store.write(|db| {
        let binding: Option<(String,String)> = db.query_row(
            "SELECT project_root,activated_by FROM workspace_projects WHERE project_id=?",
            [project], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        if binding != Some((store.key().root().to_string_lossy().into_owned(), actor.into())) {
            return refuse("unauthorized");
        }
        let prior: Option<(String,String)> = db.query_row(
            "SELECT digest,receipt_json FROM workspace_requests WHERE project_id=? AND actor_id=? AND request_id=?",
            (project,actor,request), |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((old, encoded)) = prior {
            return if old == digest { Ok(json!({"receipt":serde_json::from_str::<Value>(&encoded)?,"fresh":false})) }
                else { refuse("payload_conflict") };
        }
        if worktree.is_none() {
            worktrees::check_request(db, project, actor, request, &digest, false)?;
        }
        let row: Option<(String,i64,Option<String>,String)> = db.query_row(
            "SELECT t.description,t.version,t.execution_id,o.options_json FROM workspace_tasks t
             JOIN workspace_task_options o USING(task_id) WHERE t.project_id=? AND t.task_id=?",
            (project,draft), |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let (goal,version,existing,options) = row.ok_or(CoreError::Refused("task_not_found"))?;
        if command["expectedVersion"] != version.to_string() { return refuse("stale_version"); }
        if existing.is_some() { return refuse("busy"); }
        if !command["ownerEpoch"].is_null() { return refuse("stale_owner_epoch"); }
        let mut options: Value = serde_json::from_str(&options)?;
        if options.get("deletingSprint").is_some() {return refuse("sprint_deleting");}
        match (options["checkout"]["kind"].as_str(), worktree) {
            (Some("worktree"), Some((plan, _))) => {
                options["checkout"] = plan.projection();
                options["baseRevision"] = json!(plan.base_sha);
            }
            (None | Some("main"), None) => {
                if worktrees::has_unresolved(db, project, draft)? {
                    return refuse("worktree_uncertain");
                }
            }
            (_, Some(_)) => return refuse("worktree_binding_invalid"),
            _ => return refuse("native_worktree_unavailable"),
        }
        // Saved owner settings are honoured as in Python `_dispatch_workspace_start`;
        // the host qualified their route before admission. Incoming clients bring
        // their own session settings instead.
        let has_files = options.get("attachments").is_some_and(|v| v.as_array().is_none_or(|a| !a.is_empty()));
        if client.is_some() && (options.get("settings").is_some() || has_files) {
            return refuse("native_start_options_unavailable");
        }
        // A desktop draft's files are decoded and checked in this same transaction, so
        // the retained bodies are exactly the draft version being started.
        let draft_capture = if has_files { Some(crate::input::capture_from_wire(&options["attachments"])?) } else { None };
        let saved = if client.is_none() { saved_settings(&options)? } else { None };
        let task = format!("t_{}",uuid::Uuid::new_v4().simple());
        // Start does not hold the whole checkout: structured writers are
        // admitted by their effective write scope and fenced at the effect.
        // File bodies stay in private retention; the event keeps only their descriptors.
        let mut event_options = options.clone();
        if let Some(m) = event_options.as_object_mut() { m.remove("attachments"); }
        db.execute("INSERT INTO tasks VALUES (?,?,?)", (&task,store.key().root().to_str(),crate::now()))?;
        db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES (?,'PLANNING',?,?,?)",
            (&task,&goal,crate::now(),crate::now()))?;
        let owner = crate::ownership::acquire_tx(db,&task,actor,None)?;
        crate::lifecycle::event_tx(db,&task,"TASK_CREATED",json!({"execution_host":"rust"}))?;
        if policy == Some(NativeExecutionPolicy::AutomaticContinuation) {
            crate::lifecycle::automatic::bind_tx(db, &task)?;
        }
        if policy == Some(NativeExecutionPolicy::SprintPlanning) {
            crate::lifecycle::sprint_planning::bind_tx(db, &task)?;
        }
        crate::routing::snapshot_tx(db, &task, store.key().root())?;
        crate::lifecycle::event_tx(db,&task,"TASK_STATE_CHANGED",json!({"previous":"CREATED","state":"PLANNING"}))?;
        crate::lifecycle::event_tx(db,&task,"NATIVE_START_ADMITTED",json!({"draft":draft,"request":request,"goal":goal,"options":event_options}))?;
        if let Some((plan, witness)) = worktree {
            worktrees::bind_tx(db, project, actor, request, draft, version, &task, plan, witness)?;
            db.execute("UPDATE workspace_task_options SET options_json=? WHERE task_id=?",
                (options.to_string(), draft))?;
        }
        let execution = bind_native_execution_tx(db,store,project,actor,draft,&task,&owner)?;
        if let Some(settings) = &saved {
            let session: String = db.query_row("SELECT acp_session_id FROM workspace_executions WHERE execution_id=?",
                [&execution], |r| r.get(0))?;
            let mode = settings["mode"].as_str().ok_or(CoreError::Refused("native_task_settings_invalid"))?;
            crate::access::switch_tx(db, &task, mode, &session, "acp_pre_task_selection")?;
            crate::lifecycle::event_tx(db,&task,"NATIVE_TASK_SETTINGS",
                json!({"settings":settings,"activation":"initial"}))?;
        }
        if let Some(capture) = &draft_capture {
            // The draft's files become the task's first source revision, committed
            // with the admission; the first engine prompt of every session carries them.
            let session: String = db.query_row("SELECT acp_session_id FROM workspace_executions WHERE execution_id=?",
                [&execution], |r| r.get(0))?;
            capture.persist(store, db, &task, &session, &goal)?;
        }
        if let Some((session, settings)) = client {
            let changed = db.execute(
                "UPDATE acp_host_sessions SET task_id=? WHERE session_id=? AND project_root=?
                 AND task_id IS NULL AND json_extract(client_info,'$.name')='cortex-rust-acp'",
                (&task,session,store.key().root().to_str()))?;
            if changed != 1 { return refuse("incoming_session_binding_conflict"); }
            let mode = crate::access::session(db, session)?;
            let explicit: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM acp_session_access_defaults WHERE session_id=? AND selected_mode IS NOT NULL)",
                [session], |r|r.get(0))?;
            crate::access::switch_tx(db, &task, &mode, session,
                if explicit {"acp_pre_task_selection"} else {"runtime_session_default"})?;
            crate::lifecycle::event_tx(db,&task,"NATIVE_TASK_SETTINGS",
                json!({"settings":settings,"activation":"initial"}))?;
            let seq:i64=db.query_row("SELECT coalesce(max(seq),-1)+1 FROM (
                SELECT seq FROM host_transcript WHERE session_id=? UNION ALL
                SELECT seq FROM host_updates WHERE session_id=?)", (session,session),|r|r.get(0))?;
            let attachments=match capture {Some(c)=>c.persist(store,db,&task,session,&goal)?,None=>json!([])};
            db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,attachments_json,created_at) VALUES (?,?,'user_message',?,?,?)",
                (session,seq,&goal,attachments.to_string(),crate::now()))?;
        }
        // The owner's language is recorded at task start (task_language) for its readers.
        crate::assignment_brief::record_language_tx(db,&task)?;
        db.execute("INSERT INTO workspace_starts(execution_id,task_id,project_id,actor_id,request_id,source_origin,source_event_id,state,created_at)
            VALUES (?,?,?,?,?,?,?,'claimed',?)",
            (&execution,draft,project,actor,request,"native-desktop",format!("native:{execution}"),crate::now()))?;
        let mut reply = receipt(command,"recorded",&execution,1);
        if let Some(rework) = rework {
            let b = control::binding(db, &store.key().root().to_string_lossy(), project, actor, &execution)?;
            let event = crate::lifecycle::event_tx(db, &task, "TASK_REOPENED", json!({
                "origin":"owner_rework","previous":"NOT_STARTED","revision":0,
                "request":request,"closed":rework.history,"baseRevision":options["baseRevision"]
            }))?;
            control::owner_message_tx(store, db, &b, rework.request, rework.capture, crate::TaskState::Planning, false)?;
            reply["done"] = json!(true);
            reply["state"] = json!("PLANNING");
            reply["reworkEvent"] = json!(event.to_string());
            reply["baseRevision"] = options["baseRevision"].clone();
        }
        db.execute("INSERT INTO workspace_requests VALUES (?,?,?,?,?,?)",
            (project,actor,request,"tasks.start",digest,serde_json::to_string(&reply)?))?;
        Ok(json!({"receipt":reply,"fresh":true,"task":task,"owner":{"owner":owner.owner,"epoch":owner.epoch},"goal":goal}))
    })
}

pub struct DraftReply {
    pub receipt: Value,
    pub fresh: bool,
}
fn receipt(request: &Value, outcome: &str, target: &str, version: i64) -> Value {
    json!({"requestId":request["requestId"],"outcome":outcome,"target":target,
        "entityVersion":version.to_string(),"recordedAt":crate::now()})
}
/// The caller supplies transport-bound project/actor identities, never model authority.
/// Attachment-bearing creates stay on the retained adapter until native byte validation
/// and capture are qualified. Refusal happens before opening a transaction.
pub fn create_draft(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
) -> CoreResult<DraftReply> {
    create_draft_with_planner(store, project, actor, command, None)
}

/// Trusted service entry point; the marker is not a command payload field and
/// does not select an execution policy or link the task to the sprint's run.
pub fn create_sprint_planning_draft(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    sprint: &str,
) -> CoreResult<DraftReply> {
    workspace_protocol::core::identity(sprint).map_err(CoreError::Refused)?;
    create_draft_with_planner(store, project, actor, command, Some(sprint))
}

fn create_draft_with_planner(
    store: &Store,
    project: &str,
    actor: &str,
    command: &Value,
    planning_sprint: Option<&str>,
) -> CoreResult<DraftReply> {
    workspace_protocol::validate("Command", command)
        .map_err(|_| CoreError::Refused("invalid_workspace_request"))?;
    if command["protocol"]["major"] != 1
        || command["operation"] != "tasks.create"
        || command["projectId"] != project
        || command["actorId"] != actor
    {
        return refuse("unauthorized");
    }
    // Draft files are retained with the draft and checked against the wire limits
    // now, so a Start can never meet a body it would have to refuse.
    if let Some(files) = command["payload"].get("attachments") {
        crate::input::capture_from_wire(files)?;
    }
    let description = command["payload"]["description"]
        .as_str()
        .ok_or(CoreError::Refused("description_required"))?;
    if description.trim().is_empty() {
        return refuse("description_required");
    }
    let request = command["requestId"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_request"))?;
    // serde_json's default sorted map and compact UTF-8 encoding match storage.encode.
    // Keep the digest of ordinary creates unchanged for retained receipts. A
    // planner receipt is replayable only with the same trusted sprint identity.
    let digest = crate::digest(&match planning_sprint {
        Some(sprint) => serde_json::to_vec(&(command, sprint))?,
        None => serde_json::to_vec(command)?,
    });
    store.write(|db| {
        let binding: Option<(String,String)> = db.query_row(
            "SELECT project_root,activated_by FROM workspace_projects WHERE project_id=?",
            [project], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        if binding != Some((store.key().root().to_string_lossy().into_owned(), actor.into())) {
            return refuse("unauthorized");
        }
        let prior: Option<(String,String)> = db.query_row(
            "SELECT digest,receipt_json FROM workspace_requests WHERE project_id=? AND actor_id=? AND request_id=?",
            (project,actor,request), |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((old_digest, encoded)) = prior {
            let old: Value = serde_json::from_str(&encoded)?;
            let value = if old_digest == digest { old } else {
                receipt(command,"payload_conflict",old["target"].as_str().unwrap_or(project),
                    old["entityVersion"].as_str().and_then(|s| s.parse().ok()).unwrap_or(0))
            };
            return Ok(DraftReply {receipt:value,fresh:false});
        }
        let result = if command["target"] != project || !command["ownerEpoch"].is_null() {
            receipt(command,"invalid_request",command["target"].as_str().unwrap_or(project),0)
        } else {
            let id = format!("wt_{}",uuid::Uuid::new_v4().simple());
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES (?,?,?,?)",
                (&id,project,description,crate::now()))?;
            let mut options = command["payload"].clone();
            options.as_object_mut().unwrap().remove("description");
            if let Some(sprint) = planning_sprint {
                options["planningSprintId"] = json!(sprint);
            }
            db.execute("INSERT INTO workspace_task_options VALUES (?,?)",(&id,serde_json::to_string(&options)?))?;
            db.execute("UPDATE workspace_projects SET version=version+1 WHERE project_id=?",[project])?;
            append_public_tx(db,project,&id,"task",1)?;
            receipt(command,"recorded",&id,1)
        };
        db.execute("INSERT INTO workspace_requests VALUES (?,?,?,?,?,?)",
            (project,actor,request,"tasks.create",&digest,serde_json::to_string(&result)?))?;
        let version: i64 = db.query_row("SELECT version FROM workspace_projects WHERE project_id=?",
            [project], |r| r.get(0))?;
        append_public_tx(db,project,project,"unknown",version)?;
        Ok(DraftReply {receipt:result,fresh:true})
    })
}

/// One draft of a batch the service creates for a single owner action: an identity
/// the caller derives from what the draft is for, its request text and its
/// `tasks.create` options (no description, no attachments).
pub struct BatchDraft<'a> {
    pub task_id: &'a str,
    pub description: &'a str,
    pub options: &'a Value,
}

/// Creates every draft of one owner action inside the caller's transaction, so the
/// caller's own record of that action commits or rolls back together with them.
/// The project binding is checked as for `create_draft`, and each draft as a
/// `tasks.create` payload, before anything is written. An identity that already
/// names a draft of this project is kept as it is (a retry of the same batch); an
/// identity held by another project refuses the whole batch. Returns how many
/// drafts were created now.
pub fn create_drafts_tx(
    db: &rusqlite::Transaction<'_>,
    root: &str,
    project: &str,
    actor: &str,
    drafts: &[BatchDraft<'_>],
) -> CoreResult<usize> {
    let binding: Option<(String, String)> = db
        .query_row(
            "SELECT project_root,activated_by FROM workspace_projects WHERE project_id=?",
            [project],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if binding != Some((root.into(), actor.into())) {
        return refuse("unauthorized");
    }
    for draft in drafts {
        let mut payload = draft.options.clone();
        let fields = payload
            .as_object_mut()
            .ok_or(CoreError::Refused("invalid_workspace_request"))?;
        if fields.contains_key("description") || fields.contains_key("attachments") {
            return refuse("invalid_workspace_request");
        }
        fields.insert("description".into(), json!(draft.description));
        workspace_protocol::validate("CreatePayload", &payload)
            .map_err(|_| CoreError::Refused("invalid_workspace_request"))?;
        if draft.description.trim().is_empty() || draft.task_id.is_empty() {
            return refuse("description_required");
        }
    }
    let mut created = 0;
    for draft in drafts {
        let held: Option<String> = db
            .query_row(
                "SELECT project_id FROM workspace_tasks WHERE task_id=?",
                [draft.task_id],
                |r| r.get(0),
            )
            .optional()?;
        match held {
            Some(owner) if owner == project => continue,
            Some(_) => return refuse("payload_conflict"),
            None => {}
        }
        db.execute(
            "INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES (?,?,?,?)",
            (draft.task_id, project, draft.description, crate::now()),
        )?;
        db.execute(
            "INSERT INTO workspace_task_options VALUES (?,?)",
            (draft.task_id, serde_json::to_string(draft.options)?),
        )?;
        append_public_tx(db, project, draft.task_id, "task", 1)?;
        created += 1;
    }
    if created > 0 {
        db.execute(
            "UPDATE workspace_projects SET version=version+1 WHERE project_id=?",
            [project],
        )?;
        let version: i64 = db.query_row(
            "SELECT version FROM workspace_projects WHERE project_id=?",
            [project],
            |r| r.get(0),
        )?;
        append_public_tx(db, project, project, "unknown", version)?;
    }
    Ok(created)
}

/// Read-only desktop report page. The report must be a published `report` edition of a runtime
/// task that a workspace execution of this project is bound to; anything else (another project's
/// store, an unbound task, a pipeline, a draft) is `not_found`. Paging and the digest and cursor
/// binding are the same bounded reader the workers' `read_report` uses.
pub fn read_report_page(
    store: &Store,
    project: &str,
    report: &str,
    cursor: Option<&str>,
    limit: usize,
) -> CoreResult<Value> {
    let row: Option<(String, String, String)> = store.read(|db| {
        Ok(db
            .query_row(
                "SELECT r.task_id,r.kind,(SELECT e.title FROM editions e WHERE e.report_id=r.id ORDER BY e.sequence DESC LIMIT 1)
                 FROM reports r
                 WHERE r.id=? AND EXISTS(SELECT 1 FROM workspace_executions x JOIN workspace_tasks t ON t.task_id=x.task_id
                   WHERE x.runtime_task_id=r.task_id AND t.project_id=?)",
                (report, project),
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?)
    })?;
    let Some((task, kind, title)) = row else {
        return refuse("not_found");
    };
    if kind != "report" {
        return refuse("not_found");
    }
    let page =
        crate::store::documents::Publisher::new(store).read_report(&task, report, cursor, limit)?;
    Ok(
        json!({"reportId":report,"title":title,"sha256":page.sha256,"markdown":page.markdown,
        "totalCharacters":page.total_characters,"partial":page.next_cursor.is_some(),
        "nextCursor":page.next_cursor}),
    )
}
