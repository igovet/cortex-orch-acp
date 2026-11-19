//! Persistent, read-only desktop provider. The Python writer remains authoritative.
//! All facts in a reply are read in one SQLite snapshot on a blocking executor.
use crate::{
    error::refuse,
    events::{EventStream, StreamFrame, Subscription},
    CoreError, CoreResult, Store,
};
use rusqlite::{Connection, OptionalExtension, Params};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Mutex,
};
use workspace_protocol::{
    core::{CoreCommand, CoreQuery, NativeBody, Protocol, Version},
    events::{DeltaBatch, DeltaOperation, EntityChange, EntityKind, PublicEntity, StreamKey},
    read::{QueryReply, ReadPage, ReadReply, TaskProjection},
};

pub(super) fn row(db: &Connection, sql: &str, params: impl Params) -> CoreResult<Option<Value>> {
    let mut stmt = db.prepare(sql)?;
    let names = stmt
        .column_names()
        .map(|s| s.to_string())
        .collect::<Vec<_>>();
    Ok(stmt
        .query_row(params, |r| {
            let mut value = serde_json::Map::new();
            for (i, name) in names.iter().enumerate() {
                use rusqlite::types::ValueRef;
                value.insert(
                    name.clone(),
                    match r.get_ref(i)? {
                        ValueRef::Null => Value::Null,
                        ValueRef::Integer(n) => json!(n),
                        ValueRef::Real(n) => json!(n),
                        ValueRef::Text(s) => json!(String::from_utf8_lossy(s)),
                        ValueRef::Blob(_) => Value::Null,
                    },
                );
            }
            Ok(Value::Object(value))
        })
        .optional()?)
}
fn field<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v[key].as_str()
}
fn ver(n: i64) -> CoreResult<Version> {
    Version::new(n).map_err(CoreError::Refused)
}
fn number(v: &Value, key: &str) -> i64 {
    v[key].as_i64().unwrap_or(0)
}
fn lifecycle(state: &str) -> (&'static str, &'static str) {
    match state {
        "CREATED" => ("planning", "starting"),
        "PLANNING" | "PLAN_READY" => ("planning", "running"),
        "EXECUTING" => ("execution", "running"),
        "REVIEWING" => ("review", "running"),
        "VERIFYING" => ("verification", "running"),
        "RECONCILING" | "ANSWER_READY" => ("finalizing", "running"),
        "WAITING_USER" => ("waiting", "waiting_user"),
        "RECOVERING" => ("recovery", "recovering"),
        "BLOCKED" => ("blocked", "blocked"),
        "FAILED" => ("failed", "failed"),
        "CANCELLED" => ("stopped", "stopped"),
        "COMPLETED" => ("result", "result"),
        "PAUSED" => ("paused", "paused"),
        _ => ("unknown", "unknown"),
    }
}
pub(crate) const CLOSE_UNCONFIRMED: &str = "tasks.close_unconfirmed";
pub(crate) const CLOSED_UNCONFIRMED: &str = "owner_closed_unconfirmed";
pub(crate) const CONTRADICTORY_EVIDENCE: &str = "contradictory_evidence";

// Internal request identities are not accepted as client request IDs. The receipt
// belongs to the Workspace task and execution pointer, not to a disputed runtime owner.
pub(crate) fn unconfirmed_close_key(task: &str) -> String {
    // The wire Identity allows any string up to 128 characters.
    format!("/close-unconfirmed/{task}/{:0>129}", "")
}

pub fn task_records(
    db: &Connection,
    project: &str,
    root: &str,
    id: &str,
) -> CoreResult<Option<TaskProjection>> {
    task(db, project, root, id)
}

pub(super) fn task(
    db: &Connection,
    project: &str,
    root: &str,
    id: &str,
) -> CoreResult<Option<TaskProjection>> {
    let Some(t) = row(
        db,
        "SELECT * FROM workspace_tasks WHERE project_id=? AND task_id=?",
        [project, id],
    )?
    else {
        return Ok(None);
    };
    let mut phase = "not_started";
    let mut state = "not_started";
    let mut runtime_state = None;
    let mut reason = None;
    let mut at = field(&t, "created_at").map(str::to_owned);
    let mut warnings = Vec::new();
    let mut route = None;
    let mut automatic_continuation = None;
    if let Some(execution) = field(&t, "execution_id") {
        phase = "unknown";
        state = "unknown";
        let e = row(
            db,
            "SELECT * FROM workspace_executions WHERE execution_id=? AND task_id=?",
            [execution, id],
        )?;
        if let Some(e) = e {
            let runtime = field(&e, "runtime_task_id").unwrap_or("");
            let r = row(db,"SELECT r.*,t.project_root FROM runtime_tasks r JOIN tasks t ON t.id=r.task_id WHERE r.task_id=?", [runtime])?;
            let session = row(
                db,
                "SELECT * FROM acp_host_sessions WHERE session_id=?",
                [field(&e, "acp_session_id").unwrap_or("")],
            )?;
            let owner = row(
                db,
                "SELECT epoch FROM task_owners WHERE task_id=?",
                [runtime],
            )?;
            let start = row(
                db,
                "SELECT state FROM workspace_starts WHERE execution_id=?",
                [execution],
            )?;
            let valid_binding = session.as_ref().is_some_and(|s| {
                field(s, "project_root") == Some(root) && field(s, "task_id") == Some(runtime)
            });
            let epoch = owner.as_ref().map(|o| number(o, "epoch")).unwrap_or(0);
            if let Some(r) = &r {
                let raw = field(r, "state").unwrap_or("");
                let mapped = lifecycle(raw);
                let valid = valid_binding
                    && field(r, "project_root") == Some(root)
                    && epoch == number(&e, "owner_epoch")
                    && number(r, "revision") >= 0
                    && !(raw == "COMPLETED" && field(r, "blocker").is_some_and(|s| !s.is_empty()))
                    && !start
                        .as_ref()
                        .is_some_and(|s| field(s, "state") == Some("unknown"));
                if valid {
                    (phase, state) = mapped;
                    automatic_continuation = crate::lifecycle::automatic::projection(db, runtime)?;
                    if automatic_continuation
                        .as_ref()
                        .is_some_and(|v| v["pause"].is_object())
                    {
                        (phase, state) = ("paused", "paused");
                    }
                    // Close task commits the owner's cancellation at admission with this
                    // exact blocker; its finish receipt (and OWNER_TASK_RESOLVED) can still
                    // be refused while an engine exit is unconfirmed. The closed task is the
                    // owner's decision either way, never a run that is waiting for them.
                    // So is Finish chosen on the runtime's stop question (or "finish the
                    // task" given as its answer): `USER_FORCE_FINISHED` closes it as is.
                    if raw=="CANCELLED" && (matches!(field(r, "blocker"), Some("owner_resolved_without_certification" | "user_force_finished"))
                        || row(db,"SELECT 1 FROM runtime_events WHERE task_id=? AND type IN ('OWNER_TASK_RESOLVED','USER_FORCE_FINISHED') LIMIT 1",[runtime])?.is_some()) {
                        warnings.push("owner_resolved_without_certification".into());
                    }
                } else {
                    warnings.push("contradictory_evidence".into());
                }
                if mapped.0 != "unknown" {
                    runtime_state = Some(raw.to_owned());
                }
                at = field(r, "updated_at").map(str::to_owned);
                if state != "unknown" {
                    reason = row(
                        db,
                        "SELECT wait_reason FROM task_orchestration WHERE task_id=?",
                        [runtime],
                    )?
                    .and_then(|v| field(&v, "wait_reason").map(str::to_owned));
                    if let Some(pause) = automatic_continuation
                        .as_ref()
                        .and_then(|v| v["pause"]["reason"].as_str())
                    {
                        reason = Some(pause.to_owned());
                    }
                    if reason.is_none() && raw == "RECOVERING" {
                        reason = native_recovery_reason(db, runtime, r)?;
                        if reason.is_none() {
                            if let Some(hold) = native_exit_hold(db, runtime)? {
                                warnings.push(hold.into());
                            }
                        }
                    }
                    // A step queued behind another task's files waits, visibly, for
                    // that task to release them; it is not stopped.
                    if reason.is_none() && matches!(raw, "EXECUTING" | "REVIEWING") {
                        if let Some(wait) =
                            files_wait(db, runtime, field(r, "project_root").unwrap_or(""))?
                        {
                            reason = Some(wait);
                            warnings.push("waiting_for_files".into());
                        }
                    }
                }
            } else if valid_binding
                && epoch >= number(&e, "owner_epoch")
                && start
                    .as_ref()
                    .is_some_and(|s| matches!(field(s, "state"), Some("claimed" | "dispatched")))
            {
                let base = row(db, "SELECT project_root FROM tasks WHERE id=?", [runtime])?;
                if base
                    .as_ref()
                    .is_some_and(|v| field(v, "project_root") == Some(root))
                {
                    (phase, state) = ("planning", "starting");
                    if row(db,"SELECT task_id FROM task_stopped WHERE task_id=?", [runtime])?.is_some() {
                        (phase,state)=("stopped","stopped");
                    } else if let Some(notice)=row(db,"SELECT payload_json FROM task_events WHERE task_id=? AND event_type='progress' AND json_valid(payload_json) AND json_extract(payload_json,'$.message_key') IN ('engine_retry.launch','engine_retry.gave_up','engine_retry.recovered') ORDER BY id DESC LIMIT 1", [runtime])? {
                        let value: Value=serde_json::from_str(field(&notice,"payload_json").unwrap_or("{}"))?;
                        match field(&value,"message_key") {
                            Some("engine_retry.launch") => warnings.push("engine_launch_retrying".into()),
                            Some("engine_retry.gave_up") => { (phase,state)=("blocked","blocked"); warnings.push("engine_launch_failed".into()); },
                            _ => (),
                        }
                        if !warnings.is_empty() { reason=field(&value,"note").map(|s|s.chars().take(1000).collect()); }
                    }
                }
            }
            route = observed_route(db, runtime)?;
        }
        if state == "unknown" && warnings.is_empty() {
            warnings.push("contradictory_evidence".into());
        }
        let close = row(
            db,
            "SELECT json_extract(w.receipt_json,'$.closedAt') AS closed_at
             FROM workspace_requests w JOIN workspace_projects p USING(project_id)
             WHERE w.project_id=? AND p.project_root=? AND w.actor_id=p.activated_by
             AND w.request_id=? AND w.operation=?
             AND json_extract(w.receipt_json,'$.executionId')=?",
            (
                project,
                root,
                unconfirmed_close_key(id),
                CLOSE_UNCONFIRMED,
                execution,
            ),
        )?;
        if let Some(closed_at) = close.as_ref().and_then(|r| field(r, "closed_at")) {
            // Close the card, not a process we cannot safely identify.
            (phase, state) = ("unknown", "unknown");
            runtime_state = None;
            reason = None;
            at = Some(closed_at.to_owned());
            route = None;
            automatic_continuation = None;
            warnings = vec![CONTRADICTORY_EVIDENCE.into(), CLOSED_UNCONFIRMED.into()];
        }
    }
    let mut result = TaskProjection {
        task_id: id.into(),
        description: field(&t, "description").unwrap_or("").into(),
        title: None,
        title_availability: "not_connected".into(),
        version: ver(number(&t, "version"))?,
        execution_id: field(&t, "execution_id").map(str::to_owned),
        phase: phase.into(),
        runtime_state,
        execution_state: state.into(),
        wait_reason: reason,
        last_confirmed_at: at,
        coordinator_route: route,
        automatic_continuation,
        warnings,
        description_availability: None,
    };
    if serde_json::to_vec(&result)?.len() > 58_000 {
        result.description.clear();
        result.description_availability = Some("unavailable".into());
        result
            .warnings
            .push("description_unavailable:page_limit".into());
    }
    Ok(Some(result))
}
/// The project's tasks whose runtime record is the owner's close as is (Finish on the
/// runtime's stop question, or a Close task whose saved disposition was not written),
/// with when the runtime last confirmed it in milliseconds. It is the same rule the task
/// projection applies (`owner_resolved_without_certification` on a bound, consistent
/// CANCELLED record), so the owner's records and the board can never disagree.
pub fn owner_closed(
    db: &Connection,
    project: &str,
    root: &str,
) -> CoreResult<Vec<(String, Option<i64>)>> {
    let mut query = db.prepare(
        "SELECT t.task_id FROM workspace_tasks t
         JOIN workspace_executions e ON e.execution_id=t.execution_id AND e.task_id=t.task_id
         JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
         WHERE t.project_id=? AND r.state='CANCELLED' ORDER BY t.sequence LIMIT 10001",
    )?;
    let ids = query
        .query_map([project], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    if ids.len() > 10000 {
        return Err(CoreError::Refused("machine_reply_too_large"));
    }
    let mut out = Vec::new();
    for id in ids {
        let Some(t) = task(db, project, root, &id)? else {
            continue;
        };
        if t.phase == "stopped"
            && t.runtime_state.as_deref() == Some("CANCELLED")
            && t.warnings
                .iter()
                .any(|w| w == "owner_resolved_without_certification")
            && !t.warnings.iter().any(|w| w == "contradictory_evidence")
        {
            let at = t
                .last_confirmed_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|d| d.timestamp_millis());
            out.push((id, at));
        }
    }
    Ok(out)
}
/// Native hosts never write task_orchestration. A recovery with a recorded
/// blocker states its reason; one whose owned processes all settled is an
/// ended turn waiting for the owner. Anything else stays unconfirmed.
/// The recorded wait of a ready step for another unfinished task's files, as one
/// sentence naming that task and the files, while no later plan, owner message or
/// claimed step has ended it.
fn files_wait(db: &Connection, runtime: &str, root: &str) -> CoreResult<Option<String>> {
    let Some(w) = row(
        db,
        "SELECT json_extract(e.payload_json,'$.holder_task') AS holder,json_extract(e.payload_json,'$.paths') AS paths,
            coalesce((SELECT d.description FROM workspace_tasks d JOIN workspace_executions x USING(task_id) WHERE x.runtime_task_id=h.task_id),h.goal) AS title
         FROM runtime_events e JOIN runtime_tasks h ON h.task_id=json_extract(e.payload_json,'$.holder_task')
         WHERE e.task_id=?1 AND e.type='NATIVE_WRITE_CONFLICT_WAITING' AND h.state NOT IN ('COMPLETED','CANCELLED','FAILED')
         AND e.id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type IN
            ('PLAN_ACCEPTED','USER_STEERING','USER_ANSWER_RECEIVED','ASSIGNMENT_CREATED')),0)
         ORDER BY e.id DESC LIMIT 1",
        [runtime],
    )?
    else {
        return Ok(None);
    };
    let first = field(&w, "title")
        .unwrap_or("")
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    let title: String = if first.chars().count() > 60 {
        format!("{}…", first.chars().take(60).collect::<String>())
    } else {
        first.to_owned()
    };
    let paths: Vec<String> = serde_json::from_str::<Value>(field(&w, "paths").unwrap_or("[]"))
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(Value::as_str)
        .map(|p| {
            p.strip_prefix(root)
                .map(|s| s.trim_start_matches('/'))
                .unwrap_or(p)
                .to_owned()
        })
        .collect();
    let mut files = paths.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    if paths.len() > 3 {
        files.push_str(&format!(" and {} more", paths.len() - 3));
    }
    Ok(Some(if files.is_empty() {
        format!("Waiting for the task “{title}” to release the files this step needs")
    } else {
        format!("Waiting for the task “{title}” to release {files}")
    }))
}
fn native_recovery_reason(db: &Connection, runtime: &str, r: &Value) -> CoreResult<Option<String>> {
    let native = row(
        db,
        "SELECT 1 AS native FROM runtime_events WHERE task_id=? AND type IN ('TASK_CREATED','NATIVE_TASK_ADOPTED')
         AND json_extract(payload_json,'$.execution_host')='rust' LIMIT 1",
        [runtime],
    )?
    .is_some();
    if !native {
        return Ok(None);
    }
    if let Some(blocker) = field(r, "blocker").filter(|b| !b.is_empty()) {
        return Ok(Some(blocker.chars().take(1000).collect()));
    }
    let live = row(
        db,
        "SELECT 1 AS live WHERE EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=?1 AND status!='terminated')
         OR EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=?1 AND state!='released')",
        [runtime],
    )?
    .is_some();
    Ok((!live).then(|| "turn_interrupted".to_owned()))
}
/// Why a native recovery without a recorded reason is still not an ended turn: some
/// owned process of the task is not confirmed gone. The task stays unconfirmed; this
/// only names which of two observable cases holds, so the owner is never left without
/// a way out:
///
/// - `process_exit_pending`: a session is still being stopped (its provisioning or
///   reservation is still live). Its shutdown observation settles it, and the settled
///   reservation republishes the task, so the runtime decides by itself.
/// - `process_exit_unconfirmed`: every remaining row is this host's own recorded
///   unconfirmed exit (`ambiguous`). The running service re-checks it on its
///   scheduler passes with startup recovery's process evidence, and a settlement
///   republishes the task. While that evidence stays inconclusive only the owner can
///   end the task (Close as is, which replays nothing).
fn native_exit_hold(db: &Connection, runtime: &str) -> CoreResult<Option<&'static str>> {
    let native = row(
        db,
        "SELECT 1 AS native FROM runtime_events WHERE task_id=? AND type IN ('TASK_CREATED','NATIVE_TASK_ADOPTED')
         AND json_extract(payload_json,'$.execution_host')='rust' LIMIT 1",
        [runtime],
    )?
    .is_some();
    if !native {
        return Ok(None);
    }
    let Some(held) = row(
        db,
        "SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=?1 AND status NOT IN ('terminated','ambiguous'))
            OR EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=?1 AND state NOT IN ('released','ambiguous')) AS live,
         EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=?1 AND status!='terminated')
            OR EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=?1 AND state!='released') AS held",
        [runtime],
    )?
    else {
        return Ok(None);
    };
    Ok(
        match (number(&held, "held") != 0, number(&held, "live") != 0) {
            (false, _) => None,
            (true, true) => Some("process_exit_pending"),
            (true, false) => Some("process_exit_unconfirmed"),
        },
    )
}
fn observed_route(
    db: &Connection,
    runtime: &str,
) -> CoreResult<Option<workspace_protocol::read::RouteLabel>> {
    let Some(c)=row(db,"SELECT c.*,x.route_model FROM configuration_revisions c JOIN configuration_route_extras x ON x.revision_id=c.id AND x.task_id=c.task_id WHERE c.task_id=? AND c.state='active' AND c.confirmation_evidence_ref IS NOT NULL ORDER BY c.revision DESC LIMIT 1", [runtime])? else {return Ok(None);};
    if field(&c, "confirmation_evidence_ref").is_none_or(str::is_empty) {
        return Ok(None);
    }
    if row(db,"SELECT 1 FROM configuration_revisions WHERE task_id=? AND revision>? AND state IN ('pending_next_use','indeterminate') LIMIT 1", rusqlite::params![runtime,number(&c,"revision")])?.is_some() {return Ok(None);}
    let current = row(
        db,
        "SELECT coordinator_attempt_ref FROM task_orchestration WHERE task_id=?",
        [runtime],
    )?;
    let audit = if let Some(attempt) = current
        .as_ref()
        .and_then(|v| field(v, "coordinator_attempt_ref"))
        .filter(|s| !s.is_empty())
    {
        let a=row(db,"SELECT * FROM attempt_route_audit WHERE task_id=? AND attempt_id=? ORDER BY recorded_at DESC,rowid DESC LIMIT 1", [runtime,attempt])?;
        if !a.as_ref().is_some_and(|a| {
            field(a, "model") == field(&c, "route_model")
                && field(a, "engine") == field(&c, "route_engine")
                && field(a, "connection_id") == field(&c, "route_connection_id")
                && field(a, "config_revision_id") == field(&c, "id")
        }) {
            return Ok(None);
        }
        a
    } else {
        row(db,"SELECT * FROM attempt_route_audit WHERE task_id=? AND role='coordinator' AND config_revision_id=? AND model=? ORDER BY recorded_at DESC,rowid DESC LIMIT 1", [runtime,field(&c,"id").unwrap_or(""),field(&c,"route_model").unwrap_or("")])?
    };
    let Some(model) = field(&c, "route_model").filter(|s| !s.is_empty() && *s != "unknown") else {
        return Ok(None);
    };
    let detail: Value = serde_json::from_str(
        audit
            .as_ref()
            .and_then(|a| field(a, "decision_detail_json"))
            .unwrap_or("{}"),
    )
    .unwrap_or(json!({}));
    let parameters: Value =
        serde_json::from_str(field(&c, "parameters").unwrap_or("{}")).unwrap_or(json!({}));
    let effort = ["effort", "reasoning_effort", "thought_level"]
        .iter()
        .find_map(|k| field(&parameters, k).filter(|s| !s.is_empty()))
        .unwrap_or("");
    let effort = match effort {
        "engine_default" => "".into(),
        "low" => "Low".into(),
        "medium" => "Medium".into(),
        "high" => "High".into(),
        "xhigh" => "Extra high".into(),
        "minimal" => "Minimal".into(),
        "max" => "Max".into(),
        "none" => "None".into(),
        s => s.replace('_', " "),
    };
    Ok(Some(workspace_protocol::read::RouteLabel {
        model: field(&detail, "model_name")
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .unwrap_or(format!("{model} (alias)")),
        effort,
    }))
}

struct Model {
    stream: EventStream,
    versions: BTreeMap<String, Version>,
    project_version: Version,
}
pub struct ProjectionReader {
    store: Store,
    project: String,
    actor: String,
    model: Mutex<Model>,
}
impl ProjectionReader {
    pub fn new(store: Store, project: String, actor: String) -> CoreResult<Self> {
        let (high, versions) = store.read(|db| {
            project_row(
                db,
                &project,
                &actor,
                store.key().root().to_str().unwrap_or(""),
            )?;
            let (high, _) = position(db, &project)?;
            let versions = db
                .prepare("SELECT task_id,version FROM workspace_tasks WHERE project_id=?")?
                .query_map([&project], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .map(|(id, v)| Ok((id, ver(v)?)))
                .collect::<CoreResult<_>>()?;
            Ok((high, versions))
        })?;
        Ok(Self {
            store,
            project: project.clone(),
            actor,
            model: Mutex::new(Model {
                stream: EventStream::new(
                    StreamKey::Project {
                        project_id: project,
                    },
                    high,
                ),
                versions,
                project_version: ver(0)?,
            }),
        })
    }
    pub fn query(&self, query: CoreQuery) -> CoreResult<ReadReply> {
        query.validate_body().map_err(CoreError::Refused)?;
        // Serialize stream publication and snapshot creation: neither can race past the other's watermark.
        let mut model = self
            .model
            .lock()
            .map_err(|_| CoreError::Refused("storage_unavailable"))?;
        self.poll_model(&mut model)?;
        let generation = model.stream.generation().to_owned();
        self.store.read(|db| {
            let p=project_row(db,&self.project,&self.actor,self.store.key().root().to_str().unwrap_or(""))?;
            let (high,_)=position(db,&self.project)?;
            if high!=model.stream.high_watermark() {return refuse("snapshot_expired");}
            let version=ver(number(&p,"version"))?;
            let data=match query {
                CoreQuery::TasksBoard(target)=>{
                    if target.id!=self.project {return refuse("invalid_project");}
                    let ids=db.prepare("SELECT task_id FROM workspace_tasks WHERE project_id=? ORDER BY sequence LIMIT 1001")?
                        .query_map([&self.project],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
                    if ids.len()>1000 {return refuse("board_too_large");}
                    let items=ids.iter().map(|id|task(db,&self.project,self.store.key().root().to_str().unwrap_or(""),id)?
                        .ok_or(CoreError::Refused("snapshot_expired"))).collect::<CoreResult<Vec<_>>>()?;
                    QueryReply::Board(ReadPage {items,next_cursor:None,high_watermark:high,tail_cursor:None,prev_cursor:None})
                },
                CoreQuery::ProjectSnapshot(page)|CoreQuery::TasksPage(page)=>{
                    if page.target!=self.project {return refuse("invalid_project");}
                    let binding=crate::digest(serde_json::to_string(&json!([generation,self.project,version,high,page.limit]))?.as_bytes());
                    let after=parse_cursor(page.cursor.as_deref(),"nt2",&binding)?;
                    let rows=db.prepare("SELECT sequence,task_id FROM workspace_tasks WHERE project_id=? AND sequence>? ORDER BY sequence LIMIT ?")?
                        .query_map(rusqlite::params![self.project,after,page.limit+1],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?)))?
                        .collect::<Result<Vec<_>,_>>()?;
                    let mut items=Vec::new(); let mut consumed=after; let mut bytes=0;
                    for (seq,id) in rows.iter().take(page.limit as usize) {
                        let t=task(db,&self.project,self.store.key().root().to_str().unwrap_or(""),id)?.ok_or(CoreError::Refused("snapshot_expired"))?;
                        let size=serde_json::to_vec(&t)?.len()+1;
                        if !items.is_empty() && bytes+size>58_000 {break;}
                        bytes+=size;
                        items.push(t); consumed=*seq;
                    }
                    let next_cursor=if rows.len()>items.len(){Some(format!("nt2.{binding}.{consumed}"))}else{None};
                    QueryReply::Tasks(ReadPage {items,next_cursor,high_watermark:high,tail_cursor:None,prev_cursor:None})
                },
                CoreQuery::TaskGet(t)=>{
                    let value=task(db,&self.project,self.store.key().root().to_str().unwrap_or(""),&t.id)?.ok_or(CoreError::Refused("not_found"))?;
                    QueryReply::Task(value)
                },
                CoreQuery::TaskState(t)=>QueryReply::State(task(db,&self.project,self.store.key().root().to_str().unwrap_or(""),&t.id)?.ok_or(CoreError::Refused("not_found"))?),
                CoreQuery::History(p)=>QueryReply::History(history::page(db,&self.project,&p)?),
                CoreQuery::HistoryEarlier(p)=>QueryReply::History(history::earlier(db,&self.project,&p)?),
                _=>return refuse("unsupported_operation"),
            };
            let reply=ReadReply {protocol:Protocol::default(),generation,project_id:self.project.clone(),version,high_watermark:high,data};
            reply.validate().map_err(CoreError::Refused)?; Ok(reply)
        })
    }
    pub fn subscribe(&self, generation: &str, after: Version) -> CoreResult<Subscription> {
        let mut model = self
            .model
            .lock()
            .map_err(|_| CoreError::Refused("storage_unavailable"))?;
        Ok(model.stream.subscribe(generation, after))
    }
    pub fn poll(&self) -> CoreResult<()> {
        let mut model = self
            .model
            .lock()
            .map_err(|_| CoreError::Refused("storage_unavailable"))?;
        self.poll_model(&mut model)
    }
    fn poll_model(&self, model: &mut Model) -> CoreResult<()> {
        let after = model.stream.high_watermark();
        let (high,changes)=self.store.read(|db| {
            project_row(db,&self.project,&self.actor,self.store.key().root().to_str().unwrap_or(""))?;
            let (high,floor)=position(db,&self.project)?;
            if after<floor || after>high {return refuse("resync_required");}
            if high==after {return Ok((high,Vec::new()));}
            let entries=db.prepare("SELECT sequence,entity_id,kind FROM workspace_outbox WHERE project_id=? AND sequence>? ORDER BY sequence LIMIT 10001")?
                .query_map(rusqlite::params![self.project,after.get()],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?
                .collect::<Result<Vec<_>,_>>()?;
            if entries.len()>10000 || entries.first().map(|r|r.0)!=Some(after.get()+1) || entries.last().map(|r|r.0)!=Some(high.get()) || entries.windows(2).any(|w| w[1].0!=w[0].0+1) {return refuse("resync_required");}
            let ids=entries.iter().filter(|r|r.2=="task").map(|r|r.1.clone()).collect::<BTreeSet<_>>();
            let mut changes=Vec::new();
            for id in ids {
                let base_version=model.versions.get(&id).copied().unwrap_or(ver(0)?);
                let current=task(db,&self.project,self.store.key().root().to_str().unwrap_or(""),&id)?;
                let (new_version,operation)=match current {
                    Some(t) if t.version>base_version=>(t.version,DeltaOperation::Upsert(PublicEntity::TaskProjection(t))),
                    Some(_)=>continue,
                    None=>(ver(base_version.get().checked_add(1).ok_or(CoreError::Refused("resync_required"))?)?,DeltaOperation::Remove),
                };
                changes.push(EntityChange {kind:EntityKind::Task,id,base_version,new_version,operation});
            }
            if changes.is_empty() {
                changes.push(EntityChange {kind:EntityKind::Project,id:self.project.clone(),base_version:model.project_version,new_version:high,operation:DeltaOperation::Invalidate});
            }
            Ok((high,changes))
        })?;
        if high == after {
            return Ok(());
        }
        let batch = DeltaBatch {
            generation: model.stream.generation().into(),
            stream_key: StreamKey::Project {
                project_id: self.project.clone(),
            },
            from_sequence: ver(after.get() + 1)?,
            through_sequence: high,
            changes,
        };
        model.stream.publish(batch.clone())?;
        for c in batch.changes {
            if c.kind == EntityKind::Task {
                model.versions.insert(c.id, c.new_version);
            } else if c.kind == EntityKind::Project {
                model.project_version = c.new_version;
            }
        }
        Ok(())
    }
    pub fn drain(subscription: &Subscription) -> Vec<StreamFrame> {
        std::iter::from_fn(|| subscription.next()).collect()
    }
}
impl crate::ports::CoreMachinePort for ProjectionReader {
    fn query(&self, q: CoreQuery) -> CoreResult<ReadReply> {
        self.query(q)
    }
    fn command(&self, _: CoreCommand) -> CoreResult<workspace_protocol::core::MutationReceipt> {
        refuse("read_only_provider")
    }
}
fn project_row(db: &Connection, project: &str, actor: &str, root: &str) -> CoreResult<Value> {
    let p = row(
        db,
        "SELECT * FROM workspace_projects WHERE project_id=?",
        [project],
    )?
    .ok_or(CoreError::Refused("project_inactive"))?;
    if field(&p, "project_root") != Some(root) || field(&p, "activated_by") != Some(actor) {
        return refuse("unauthorized");
    }
    Ok(p)
}
fn position(db: &Connection, project: &str) -> CoreResult<(Version, Version)> {
    let p = row(
        db,
        "SELECT high_watermark,retained_floor FROM workspace_outbox_state WHERE project_id=?",
        [project],
    )?;
    Ok(match p {
        Some(p) => (
            ver(number(&p, "high_watermark"))?,
            ver(number(&p, "retained_floor"))?,
        ),
        None => (ver(0)?, ver(0)?),
    })
}
pub(super) fn parse_cursor(cursor: Option<&str>, prefix: &str, binding: &str) -> CoreResult<i64> {
    let Some(cursor) = cursor else {
        return Ok(0);
    };
    let fields: Vec<_> = cursor.split('.').collect();
    if fields.len() != 3 || fields[0] != prefix || fields[1] != binding {
        return refuse("snapshot_expired");
    }
    let value = fields[2]
        .parse::<i64>()
        .map_err(|_| CoreError::Refused("invalid_cursor"))?;
    if value < 0 || value.to_string() != fields[2] {
        return refuse("invalid_cursor");
    }
    Ok(value)
}
mod history;
pub use history::{tool_step, tool_step_from};

#[cfg(test)]
mod latency_diagnostic {
    use super::*;
    #[test]
    #[ignore = "explicit read-only measurement of an authorized database copy"]
    fn copied_store_board_and_eager_history() {
        let path = std::env::var("CORTEX_BENCH_COPY").expect("authorized copy path required");
        let db =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
        db.pragma_update(None, "query_only", "ON").unwrap();
        let (project, root): (String, String) = db
            .query_row(
                "SELECT project_id,project_root FROM workspace_projects LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let ids = db
            .prepare("SELECT task_id FROM workspace_tasks WHERE project_id=? ORDER BY sequence")
            .unwrap()
            .query_map([&project], |r| r.get::<_, String>(0))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let start = std::time::Instant::now();
        let items = ids
            .iter()
            .map(|id| task(&db, &project, &root, id).unwrap().unwrap())
            .collect::<Vec<_>>();
        let board = start.elapsed();
        let bytes = serde_json::to_vec(&items).unwrap().len();
        let mut pages = 0;
        let mut rows = 0;
        for t in items.iter().filter(|t| t.execution_id.is_some()) {
            let mut cursor = None;
            loop {
                let p = history::page(
                    &db,
                    &project,
                    &workspace_protocol::core::Page {
                        target: t.task_id.clone(),
                        cursor,
                        limit: 100,
                    },
                )
                .unwrap();
                pages += 1;
                rows += p.items.len();
                cursor = p.next_cursor;
                if cursor.is_none() {
                    break;
                }
                assert!(pages < 10000);
            }
        }
        eprintln!("copied_store tasks={} bytes={} board_ms={:.3} eager_board_plus_history_ms={:.3} history_pages={} visible_history_rows={}",
            items.len(),bytes,board.as_secs_f64()*1000.,start.elapsed().as_secs_f64()*1000.,pages,rows);
    }
}
#[cfg(test)]
mod files_wait_tests {
    use crate::{lifecycle, store::Store};
    use serde_json::json;
    /// A step queued behind another unfinished task's files reads as waiting for that
    /// task by name, until the holder finishes or this task claims a step.
    #[test]
    fn a_step_queued_behind_another_tasks_files_names_the_holder() {
        let root = std::env::temp_dir().join(format!("files-wait-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let store = Store::open(
            crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap(),
            crate::OpenMode::Initialize,
        )
        .unwrap();
        let machine = lifecycle::TaskMachine::new(&store);
        machine.create("t", "g", "o").unwrap();
        machine
            .create("h", "Format all tests\nwith the new style", "o")
            .unwrap();
        let base = root.to_string_lossy().into_owned();
        store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1", [])?;
                lifecycle::event_tx(db, "t", "ASSIGNMENT_CREATED", json!({}))?;
                lifecycle::event_tx(
                    db,
                    "t",
                    "NATIVE_WRITE_CONFLICT_WAITING",
                    json!({"node":"reset","holder_task":"h","paths":[format!("{base}/test/a.js")]}),
                )?;
                assert_eq!(
                    super::files_wait(db, "t", &base)?.as_deref(),
                    Some("Waiting for the task “Format all tests” to release test/a.js")
                );
                db.execute(
                    "UPDATE runtime_tasks SET state='COMPLETED' WHERE task_id='h'",
                    [],
                )?;
                assert_eq!(super::files_wait(db, "t", &base)?, None);
                db.execute(
                    "UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id='h'",
                    [],
                )?;
                lifecycle::event_tx(db, "t", "ASSIGNMENT_CREATED", json!({}))?;
                assert_eq!(super::files_wait(db, "t", &base)?, None);
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
