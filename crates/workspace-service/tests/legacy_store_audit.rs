//! Opt-in audit of existing project stores through the service's own open, recovery,
//! read and control paths. Ignored by default and inert without `AUDIT_ROOT`.
//!
//! It must only ever see a disposable copy of a store: `AUDIT_ROOT` has to contain the
//! marker file `.cortex-audit-copy`, which no real project has. The usual way to run it is
//! inside a private mount namespace in which the copy is bind-mounted over the project's
//! original path (stores bind their absolute project root), so the real project is hidden
//! for the whole run. Every engine route resolves to an executable that does not exist, so
//! no engine, model or account can start. Output is one JSON document (`AUDIT_OUT`) with
//! identities, states, codes and counts only: no message bodies, prompts or report text.
//!
//! Environment:
//! - `AUDIT_MODE`: `inspect` (non-mutating opens) or `service` (activation, recovery, reads,
//!   optional controls).
//! - `AUDIT_ROOT`, `AUDIT_OUT`; `AUDIT_CHANNEL` (`release`/`dev`, inspect mode only).
//! - service mode: `AUDIT_STATE` (private disposable service state directory, may already
//!   hold a copied `product/<id>.json`), `AUDIT_PROJECT_ID`, optional `AUDIT_NAME`, and
//!   `AUDIT_CONTROLS` (comma list, applied in order per started task: `message`, `resume`, `retry`,
//!   `chain` (message, answer the stop question, message), `stop_close_delete`; plus
//!   `delete_sprint` (every sprint first) and `restart` (a second service on the same store,
//!   then message, Close and Delete sprint)). `AUDIT_PRODUCT_SRC` optionally names a directory
//!   whose `<projectId>.json` product overlay is copied into `AUDIT_STATE`; `AUDIT_SKIP_READS`
//!   skips the window reads.
//! - A default (dev-channel) test build serves `.cortex-dev`: a copied release store is placed
//!   there, and the copy gets the dev ownership marker `project_guard::reserve` would write.
#[path = "support/hermetic.rs"]
mod hermetic;
use cortex_core::{CoreResult, OpenMode, ProjectKey, Store, StoreChannel};
use cortex_engine::environment::{EngineKind, LaunchSpec as EngineLaunch};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use workspace_service::{
    native_runtime::NativeRoutes, native_sessions::SessionRoute, Config, LaunchSpec, Service,
};

const MARKER: &str = ".cortex-audit-copy";
const NO_ENGINE: &str = "/nonexistent/cortex-audit-no-engine";

/// Every role and task resolves, so controls reach admission, but nothing can launch.
struct NoEngine;
fn session(root: &Path) -> SessionRoute {
    SessionRoute {
        launch: EngineLaunch {
            engine: EngineKind::Codex,
            executable: NO_ENGINE.into(),
            args: vec![],
            cwd: root.into(),
            native_home: root.join(".cortex-audit-home"),
            credential_directory: root.join(".cortex-audit-home"),
            base_environment: BTreeMap::new(),
            codex_launcher: None,
            confinement: None,
        },
        relay: NO_ENGINE.into(),
        broker_parent: root.join(".cortex"),
        connection: "audit-no-engine".into(),
        connection_limit: 4,
        model: "audit-no-model".into(),
        parameters: BTreeMap::new(),
        instructions: "Audit: no engine.".into(),
        deadline: Duration::from_secs(2),
    }
}
impl NativeRoutes for NoEngine {
    fn route(&self, root: &Path, _: &str) -> CoreResult<SessionRoute> {
        Ok(session(root))
    }
    fn task_route(&self, root: &Path, _: &Value) -> CoreResult<SessionRoute> {
        Ok(session(root))
    }
}

fn var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|v| !v.is_empty())
}
fn short(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('~');
    }
    out
}
fn code(e: &std::io::Error) -> String {
    e.to_string()
}
fn channel(name: &str) -> StoreChannel {
    if name == "dev" {
        StoreChannel::Dev
    } else {
        StoreChannel::Release
    }
}

#[test]
#[ignore = "opt-in: needs AUDIT_* and a disposable store copy"]
fn legacy_store_audit() {
    let Some(root) = var("AUDIT_ROOT").map(PathBuf::from) else {
        return;
    };
    assert!(
        root.join(MARKER).is_file(),
        "AUDIT_ROOT is not a marked disposable copy; refusing to touch it"
    );
    let out = PathBuf::from(var("AUDIT_OUT").expect("AUDIT_OUT"));
    let report = match var("AUDIT_MODE").as_deref() {
        Some("inspect") => inspect(&root),
        Some("service") => tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap()
            .block_on(service(&root)),
        other => panic!("unknown AUDIT_MODE {other:?}"),
    };
    std::fs::write(out, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
}

fn inspect(root: &Path) -> Value {
    let ch = channel(var("AUDIT_CHANNEL").as_deref().unwrap_or("release"));
    let mut report = json!({"mode":"inspect","root":root});
    for (label, mode) in [
        ("inspectImmutable", OpenMode::InspectImmutable),
        ("liveReadOnly", OpenMode::LiveReadOnly),
    ] {
        let opened = ProjectKey::new(root, ch).and_then(|k| Store::open(k, mode));
        report[label] = match opened {
            Err(e) => json!({"ok":false,"code":e.code()}),
            Ok(store) => {
                let counts = store.read(|db| {
                    let n = |sql: &str| match db.query_row(sql, [], |r| r.get::<_, i64>(0)) {
                        Ok(n) => json!(n),
                        Err(e) => json!(format!("error:{}", short(&e.to_string(), 120))),
                    };
                    let format: Value = db
                        .query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))
                        .map(Value::from)
                        .unwrap_or(Value::Null);
                    Ok(json!({"userVersion":format,
                        "runtimeTasks":n("SELECT count(*) FROM runtime_tasks"),
                        "workspaceTasks":n("SELECT count(*) FROM workspace_tasks"),
                        "events":n("SELECT count(*) FROM runtime_events")}))
                });
                match counts {
                    Ok(c) => json!({"ok":true,"counts":c}),
                    Err(e) => json!({"ok":true,"readCode":e.code()}),
                }
            }
        };
    }
    report
}

fn live(root: &Path) -> CoreResult<Store> {
    // The channel this service build serves (the default test build is Dev).
    let ch = if workspace_protocol::channel::current().is_dev() {
        StoreChannel::Dev
    } else {
        StoreChannel::Release
    };
    Store::open(ProjectKey::new(root, ch)?, OpenMode::LiveReadOnly)
}
/// Durable per-task facts, read with a separate live read-only connection.
fn db_tasks(root: &Path, since: i64) -> Value {
    let store = match live(root) {
        Ok(s) => s,
        Err(e) => return json!({"error":e.code()}),
    };
    let read = store.read(|db| {
        let max: i64 = db.query_row("SELECT coalesce(max(id),0) FROM runtime_events", [], |r| r.get(0)).unwrap_or(-1);
        let sect = |r: rusqlite::Result<Vec<Value>>| match r {
            Ok(v) => json!(v),
            Err(e) => json!({"queryError":short(&e.to_string(), 160)}),
        };
        let rows = sect((|| db
            .prepare(
                "SELECT t.task_id,t.state,t.blocker,t.revision,t.created_at,
                  EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=t.task_id AND e.type='TASK_CREATED'
                    AND json_extract(e.payload_json,'$.execution_host')='rust'),
                  EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=t.task_id AND e.type='NATIVE_TASK_ADOPTED'),
                  (SELECT w.task_id FROM workspace_executions x JOIN workspace_tasks w ON w.task_id=x.task_id WHERE x.runtime_task_id=t.task_id),
                  (SELECT x.execution_id FROM workspace_executions x WHERE x.runtime_task_id=t.task_id),
                  (SELECT group_concat(status||':'||n,',') FROM (SELECT status,count(*) n FROM delegation_provisioning WHERE task_id=t.task_id GROUP BY status)),
                  (SELECT group_concat(state||':'||n,',') FROM (SELECT state,count(*) n FROM capacity_reservations WHERE task_id=t.task_id GROUP BY state)),
                  (SELECT count(*) FROM attempt_process_registry WHERE task_id=t.task_id AND terminated_at IS NULL),
                  (SELECT count(*) FROM runtime_nodes WHERE task_id=t.task_id AND status='running'),
                  (SELECT count(*) FROM runtime_events WHERE task_id=t.task_id)
                 FROM runtime_tasks t ORDER BY t.rowid",
            )?
            .query_map([], |r| {
                Ok(json!({"runtimeTask":r.get::<_,String>(0)?,"state":r.get::<_,String>(1)?,
                    "blocker":r.get::<_,Option<String>>(2)?,"revision":r.get::<_,i64>(3)?,
                    "createdAt":r.get::<_,String>(4)?,"rustCreated":r.get::<_,bool>(5)?,
                    "adopted":r.get::<_,bool>(6)?,"draft":r.get::<_,Option<String>>(7)?,
                    "execution":r.get::<_,Option<String>>(8)?,
                    "provisioning":r.get::<_,Option<String>>(9)?,"reservations":r.get::<_,Option<String>>(10)?,
                    "liveProcesses":r.get::<_,i64>(11)?,"runningNodes":r.get::<_,i64>(12)?,
                    "events":r.get::<_,i64>(13)?}))
            })?
            .collect::<Result<Vec<_>, _>>())());
        // New events since `since`: type plus a reason-like field only.
        let events = sect((|| db
            .prepare(
                "SELECT id,task_id,type,json_extract(payload_json,'$.reason'),
                   json_type(payload_json,'$.previous'),json_extract(payload_json,'$.evidence')
                 FROM runtime_events WHERE id>? ORDER BY id LIMIT 5000",
            )?
            .query_map([since], |r| {
                Ok(json!({"id":r.get::<_,i64>(0)?,"task":r.get::<_,String>(1)?,"type":r.get::<_,String>(2)?,
                    "reason":r.get::<_,Option<String>>(3)?,"sync":r.get::<_,Option<String>>(4)?.is_some(),
                    "evidence":r.get::<_,Option<String>>(5)?}))
            })?
            .collect::<Result<Vec<_>, _>>())());
        let drafts = sect((|| db
            .prepare(
                "SELECT w.task_id,w.execution_id,length(w.description),w.version,
                   (SELECT state FROM workspace_starts s WHERE s.task_id=w.task_id)
                 FROM workspace_tasks w ORDER BY w.sequence",
            )?
            .query_map([], |r| {
                Ok(json!({"draft":r.get::<_,String>(0)?,"execution":r.get::<_,Option<String>>(1)?,
                    "descriptionChars":r.get::<_,i64>(2)?,"version":r.get::<_,i64>(3)?,
                    "start":r.get::<_,Option<String>>(4)?}))
            })?
            .collect::<Result<Vec<_>, _>>())());
        let projects = sect((|| db
            .prepare("SELECT project_id,project_root FROM workspace_projects")?
            .query_map([], |r| Ok(json!([r.get::<_, String>(0)?, r.get::<_, String>(1)?])))?
            .collect::<Result<Vec<_>, _>>())());
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").map(|b| b.trim().to_owned()).unwrap_or_default();
        // Why each unsettled delegation/reservation is unsettled (identities and codes only).
        let unsettled = sect((|| db
            .prepare(
                "SELECT p.task_id,p.idempotency_key,p.status,p.step,p.attempt_id IS NULL,p.engine_session_ref IS NULL,
                   p.channel_id IS NULL,p.error_code,
                   (SELECT group_concat(r.state||'/'||coalesce(r.release_observation,''),',') FROM capacity_reservations r WHERE r.attempt_id=p.attempt_id),
                   (SELECT count(*) FROM attempt_process_registry a WHERE a.attempt_id=p.attempt_id),
                   (SELECT count(*) FROM attempt_process_registry a WHERE a.attempt_id=p.attempt_id AND a.terminated_at IS NULL AND a.boot_id=?1),
                   (SELECT json_extract(e.payload_json,'$.phase')||'/'||json_extract(e.payload_json,'$.error') FROM runtime_events e
                      WHERE e.task_id=p.task_id AND e.type='NATIVE_SESSION_START_FAILED' AND json_extract(e.payload_json,'$.channel_id')=p.channel_id LIMIT 1),
                   (SELECT count(*) FROM runtime_events x WHERE x.task_id=p.task_id AND json_extract(x.payload_json,'$.channel_id')=p.channel_id),
                   (SELECT n.status FROM runtime_nodes n WHERE n.task_id=p.task_id AND n.assignment_ref=p.idempotency_key ORDER BY n.revision DESC LIMIT 1),
                   p.role_id
                 FROM delegation_provisioning p WHERE p.status!='terminated' ORDER BY p.rowid",
            )?
            .query_map([&boot], |r| {
                Ok(json!({"task":r.get::<_,String>(0)?,"delegation":short(&r.get::<_,String>(1)?,24),"status":r.get::<_,String>(2)?,
                    "step":r.get::<_,String>(3)?,"noAttempt":r.get::<_,bool>(4)?,"noSession":r.get::<_,bool>(5)?,
                    "noChannel":r.get::<_,bool>(6)?,"error":r.get::<_,Option<String>>(7)?,"reservations":r.get::<_,Option<String>>(8)?,
                    "registry":r.get::<_,i64>(9)?,"registryThisBoot":r.get::<_,i64>(10)?,"startFailed":r.get::<_,Option<String>>(11)?,
                    "channelEvents":r.get::<_,i64>(12)?,"node":r.get::<_,Option<String>>(13)?,"role":r.get::<_,String>(14)?}))
            })?
            .collect::<Result<Vec<_>, _>>())());
        let reservations = sect((|| db
            .prepare(
                "SELECT r.task_id,r.kind,r.state,r.release_observation,
                   EXISTS(SELECT 1 FROM delegation_provisioning p WHERE p.attempt_id=r.attempt_id),
                   (SELECT count(*) FROM attempt_process_registry a WHERE a.attempt_id=r.attempt_id)
                 FROM capacity_reservations r WHERE r.state!='released' ORDER BY r.rowid",
            )?
            .query_map([], |r| {
                Ok(json!({"task":r.get::<_,String>(0)?,"kind":r.get::<_,String>(1)?,"state":r.get::<_,String>(2)?,
                    "observation":r.get::<_,Option<String>>(3)?,"hasDelegation":r.get::<_,bool>(4)?,"registry":r.get::<_,i64>(5)?}))
            })?
            .collect::<Result<Vec<_>, _>>())());
        Ok(json!({"maxEvent":max,"tasks":rows,"newEvents":events,"drafts":drafts,"projects":projects,"unsettled":unsettled,"openReservations":reservations}))
    });
    read.unwrap_or_else(|e| json!({"error":e.code()}))
}
fn max_event(root: &Path) -> i64 {
    live(root)
        .and_then(|s| {
            s.read(|db| {
                Ok(
                    db.query_row("SELECT coalesce(max(id),0) FROM runtime_events", [], |r| {
                        r.get(0)
                    })?,
                )
            })
        })
        .unwrap_or(-1)
}
/// Wait until background drivers stop writing events (bounded).
async fn quiet(root: &Path, most: Duration) -> Value {
    let start = Instant::now();
    let mut last = max_event(root);
    let mut stable = Instant::now();
    while start.elapsed() < most {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let now = max_event(root);
        if now != last {
            last = now;
            stable = Instant::now();
        } else if stable.elapsed() >= Duration::from_secs(4) {
            return json!({"quiet":true,"waitedMs":start.elapsed().as_millis() as u64});
        }
    }
    json!({"quiet":false,"waitedMs":start.elapsed().as_millis() as u64})
}

async fn native(
    service: &Service,
    project: &str,
    op: &str,
    payload: Value,
) -> Result<Value, String> {
    let q = json!({"protocol":{"major":2,"mapping":2},"requestId":format!("audit-{}", op.replace('.', "-")),
        "projectId":project,"operation":op,"payload":payload});
    for _ in 0..20 {
        match service.native_query(q.clone()).await {
            Ok(reply) => return Ok(reply),
            Err(e) if code(&e) == "snapshot_expired" => {
                tokio::time::sleep(Duration::from_millis(20)).await
            }
            Err(e) => return Err(code(&e)),
        }
    }
    Err("snapshot_never_stable".into())
}
async fn machine(
    service: &Service,
    op: &str,
    project: Option<&str>,
    params: Value,
) -> Result<Value, String> {
    let mut call = json!({"op":op,"params":params});
    if let Some(p) = project {
        call["projectId"] = json!(p);
    }
    let call = serde_json::from_value(call).map_err(|e| format!("bad_call:{e}"))?;
    match service.machine(call).await {
        Ok(reply) if reply["ok"] == true => Ok(reply["result"].clone()),
        Ok(reply) => Err(format!(
            "{}:{}",
            reply["code"].as_str().unwrap_or("refused"),
            short(reply["message"].as_str().unwrap_or(""), 160)
        )),
        Err(e) => Err(code(&e)),
    }
}
fn outcome(r: &Result<Value, String>) -> Value {
    match r {
        Ok(_) => json!({"ok":true}),
        Err(e) => json!({"ok":false,"code":e}),
    }
}
/// Shape-only description of a history page: counts per row kind and odd rows.
fn history_shape(rows: &[Value]) -> Value {
    let mut kinds = BTreeMap::<String, u64>::new();
    let mut odd = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let kind = row["kind"].as_str().unwrap_or("<none>").to_owned();
        *kinds.entry(kind.clone()).or_default() += 1;
        let empty_text = row
            .get("text")
            .is_some_and(|t| t.as_str().is_some_and(|s| s.trim().is_empty()));
        if row["kind"].is_null() || row["recordedAt"].is_null() || empty_text {
            odd.push(json!({"index":i,"kind":kind,"keys":row.as_object().map(|o|o.keys().cloned().collect::<Vec<_>>()),
                "emptyText":empty_text}));
        }
    }
    json!({"rows":rows.len(),"kinds":kinds,"odd":odd.into_iter().take(10).collect::<Vec<_>>()})
}

async fn reads(service: &Service, project: &str, report: &mut Value) {
    report["projectStatus"] = match service.project_status(project).await {
        Ok(v) => json!({"ok":true,"state":v["state"],"ready":v["ready"],"reason":v["reason"]}),
        Err(e) => json!({"ok":false,"code":code(&e)}),
    };
    let board = native(service, project, "tasks.board", json!({"id":project})).await;
    report["board"] = match &board {
        Ok(v) => json!({"ok":true,"items":v["data"]["value"]["items"].as_array().map(Vec::len),
            "nextCursor":v["data"]["value"]["nextCursor"]}),
        Err(e) => json!({"ok":false,"code":e}),
    };
    let mut items = Vec::new();
    let mut cursor = Value::Null;
    let mut page_error = None;
    for _ in 0..100 {
        match native(
            service,
            project,
            "tasks.page",
            json!({"target":project,"cursor":cursor,"limit":50}),
        )
        .await
        {
            Ok(v) => {
                items.extend(
                    v["data"]["value"]["items"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default(),
                );
                cursor = v["data"]["value"]["nextCursor"].clone();
                if cursor.is_null() {
                    break;
                }
            }
            Err(e) => {
                page_error = Some(e);
                break;
            }
        }
    }
    report["tasksPage"] = json!({"items":items.len(),"error":page_error});
    let mut started = Vec::new();
    let mut per_task = Vec::new();
    for item in &items {
        let task = item["taskId"].as_str().unwrap_or("").to_owned();
        let execution = item["executionId"].as_str().map(str::to_owned);
        let title = item["title"]
            .as_str()
            .or(item["description"].as_str())
            .unwrap_or("");
        let mut t = json!({"taskId":task,"executionId":execution,"phase":item["phase"],
            "runtimeState":item["runtimeState"],"status":item["status"],"version":item["version"],
            "titleChars":title.chars().count(),"title":short(title.lines().next().unwrap_or(""), 48),
            "keys":item.as_object().map(|o|o.keys().cloned().collect::<Vec<_>>())});
        for op in ["tasks.get", "tasks.state"] {
            let r = native(service, project, op, json!({"id":task})).await;
            t[op] = match &r {
                Ok(v) => json!({"ok":true,"type":v["data"]["type"]}),
                Err(e) => json!({"ok":false,"code":e}),
            };
        }
        if let Some(execution) = &execution {
            started.push(task.clone());
            let mut rows = Vec::new();
            let mut cursor = Value::Null;
            let mut error = None;
            let mut pages = 0;
            for _ in 0..200 {
                pages += 1;
                match native(
                    service,
                    project,
                    "history.page",
                    json!({"target":task,"cursor":cursor,"limit":100}),
                )
                .await
                {
                    Ok(v) => {
                        if v["data"]["type"] != "history" {
                            error = Some(format!("type:{}", v["data"]["type"]));
                            break;
                        }
                        rows.extend(
                            v["data"]["value"]["items"]
                                .as_array()
                                .cloned()
                                .unwrap_or_default(),
                        );
                        cursor = v["data"]["value"]["nextCursor"].clone();
                        if cursor.is_null() {
                            break;
                        }
                    }
                    Err(e) => {
                        error = Some(e);
                        break;
                    }
                }
            }
            let mut shape = history_shape(&rows);
            shape["pages"] = json!(pages);
            shape["error"] = json!(error);
            shape["truncated"] = json!(!cursor.is_null());
            t["history"] = shape;
            let mut decisions = Vec::new();
            let mut cursor = Value::Null;
            let mut derror = None;
            for _ in 0..50 {
                let ask = json!({"protocol":{"major":1,"minor":8},"projectId":project,"requestId":"audit-decisions",
                    "operation":"decisions.list","target":execution,"cursor":cursor,"limit":100});
                match service.workspace(ask).await {
                    Ok(v) => {
                        decisions.extend(v["items"].as_array().cloned().unwrap_or_default());
                        cursor = v["nextCursor"].clone();
                        if cursor.is_null() {
                            break;
                        }
                    }
                    Err(e) => {
                        derror = Some(code(&e));
                        break;
                    }
                }
            }
            t["decisions"] = json!({"count":decisions.len(),"error":derror,
                "pending":decisions.iter().filter(|d|d["status"]=="pending").count(),
                "answerable":decisions.iter().filter(|d|d["allowedOperations"].as_array().is_some_and(|a|!a.is_empty())).count()});
        }
        per_task.push(t);
    }
    // Product reads the window makes for this project.
    let facts = machine(
        service,
        "product.facts",
        Some(project),
        json!({"taskIds":started}),
    )
    .await;
    report["productFacts"] = outcome(&facts);
    let product_read = machine(service, "product.read", Some(project), json!({})).await;
    report["productRead"] = outcome(&product_read);
    let board_read = machine(service, "product.board", Some(project), json!({})).await;
    report["productBoard"] = outcome(&board_read);
    let workers = machine(
        service,
        "workers",
        Some(project),
        json!({"taskIds":started}),
    )
    .await;
    report["workers"] = match &workers {
        Ok(v) => json!({"ok":true,"count":v.as_array().map(Vec::len)}),
        Err(e) => json!({"ok":false,"code":e}),
    };
    for (key, op, scoped) in [
        ("repoSnapshot", "repo.snapshot", true),
        ("projectConfig", "project.config.read", true),
        ("configRead", "config.read", false),
    ] {
        let r = machine(service, op, scoped.then_some(project), json!({})).await;
        report[key] = outcome(&r);
    }
    let overlay = facts
        .as_ref()
        .ok()
        .map(|f| f["product"].clone())
        .or_else(|| product_read.as_ref().ok().cloned())
        .unwrap_or(Value::Null);
    let worker_list = facts
        .as_ref()
        .ok()
        .and_then(|f| f["workers"].as_array().cloned())
        .unwrap_or_default();
    for t in &mut per_task {
        let id = t["taskId"].as_str().unwrap_or("").to_owned();
        let o = &overlay["tasks"][&id];
        let result = &o["result"];
        t["overlay"] = json!({
            "present":o.is_object(),"status":o["status"],"closedAs":o["closedAs"],"purged":o["purged"],
            "titleChars":o["title"].as_str().map(|s|s.chars().count()),
            "checkout":o["checkout"]["kind"],
            "result":if result.is_object() {json!({"kind":result["kind"],"version":result["version"],
                "sourceVersion":result["sourceVersion"],"digest":result["digest"].as_str().map(|d|short(d,16)),
                "acceptanceAvailable":result["acceptanceAvailable"],"checks":result["checks"].as_array().map(Vec::len),
                "warnings":result["warnings"].as_array().map(Vec::len)})} else {Value::Null},
            "acceptance":if o["acceptance"].is_object() {json!({"version":o["acceptance"]["version"],
                "digest":o["acceptance"]["digest"].as_str().map(|d|short(d,16))})} else {Value::Null},
            "reviewActions":o["reviewActions"].as_object().map(|a|a.iter()
                .filter(|(_,v)|v.is_boolean()||v.is_string()||v.is_number())
                .map(|(k,v)|(k.clone(),v.clone())).collect::<serde_json::Map<_,_>>()),
            "workspaceDigest":o["workspaceDigest"].as_str().map(|d|short(d,16)),
        });
        let mine: Vec<_> = worker_list
            .iter()
            .filter(|w| w["taskId"] == json!(id))
            .collect();
        t["workerRows"] = json!({"count":mine.len(),
            "statuses":mine.iter().map(|w|w["status"].clone()).collect::<Vec<_>>()});
    }
    report["sprints"] = overlay["sprints"]
        .as_array()
        .map(|s| {
            s.iter()
                .map(|s| {
                    json!({"id":s["id"],"taskIds":s["taskIds"],"queueState":s["queueState"],
                    "planning":s["planning"],"deletingSprint":s["deleting"]})
                })
                .collect::<Vec<_>>()
        })
        .map_or(Value::Null, Value::from);
    report["tasks"] = json!(per_task);
}

macro_rules! timed {
    ($f:expr) => {
        match tokio::time::timeout(Duration::from_secs(40), $f).await {
            Ok(Ok(v)) => json!({"ok":true,"outcome":v["outcome"],"state":v["state"]}),
            Ok(Err(e)) => json!({"ok":false,"code":code(&e)}),
            Err(_) => json!({"ok":false,"code":"audit_timeout"}),
        }
    };
}
async fn controls(
    service: &Service,
    project: &str,
    root: &Path,
    wanted: &[String],
    report: &mut Value,
) {
    // Current task identities and versions, as the window holds them.
    let mut items = Vec::new();
    let mut cursor = Value::Null;
    for _ in 0..100 {
        let Ok(v) = native(
            service,
            project,
            "tasks.page",
            json!({"target":project,"cursor":cursor,"limit":50}),
        )
        .await
        else {
            break;
        };
        items.extend(
            v["data"]["value"]["items"]
                .as_array()
                .cloned()
                .unwrap_or_default(),
        );
        cursor = v["data"]["value"]["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    let before = max_event(root);
    // The owner's Delete sprint goes first, exactly as the window sends it (it stops and
    // closes each member execution itself).
    if wanted.iter().any(|w| w == "delete_sprint") {
        let sprints = machine(service, "product.read", Some(project), json!({})).await;
        let mut out = Vec::new();
        for s in sprints
            .ok()
            .and_then(|p| p["sprints"].as_array().cloned())
            .unwrap_or_default()
        {
            let reply = machine(
                service,
                "product.sprint",
                Some(project),
                json!({"action":"delete","sprintId":s["id"],"confirmed":true}),
            )
            .await;
            out.push(json!({"sprint":s["id"],"members":s["taskIds"].as_array().map(Vec::len),
                "reply":match reply {Ok(v)=>json!({"done":v["done"],"reason":v["reason"].as_str().map(|r|short(r,300))}),
                    Err(e)=>json!({"error":e})}}));
        }
        report["sprintDeletes"] = json!(out);
        report["sprintDeleteWait"] = quiet(root, Duration::from_secs(30)).await;
    }
    let mut results = Vec::new();
    let version_of = |items: &[Value], task: &str| {
        items
            .iter()
            .find(|i| i["taskId"] == task)
            .and_then(|i| i["version"].as_str().map(str::to_owned))
    };
    for item in &items {
        let Some(execution) = item["executionId"].as_str() else {
            continue;
        };
        let task = item["taskId"].as_str().unwrap_or("").to_owned();
        let state = item["runtimeState"].as_str().unwrap_or("").to_owned();
        let terminal = matches!(state.as_str(), "CANCELLED" | "FAILED");
        let mut r = json!({"taskId":task,"stateBefore":state});
        for control in wanted {
            match control.as_str() {
                "message" if !terminal => {
                    r["message"] = timed!(service.prompt_execution(
                        project,
                        execution,
                        "Audit probe: continue.",
                        Some(&format!("audit-msg-{task}")),
                        json!([])
                    ));
                }
                "resume" if !terminal => {
                    r["resume"] = timed!(service.control_execution(json!({"projectId":project,
                        "executionId":execution,"requestId":format!("audit-resume-{task}"),"action":"resume",
                        "expectedVersion":item["version"]})));
                }
                "chain" if !terminal && state != "COMPLETED" => {
                    r["chain"] = chain(service, project, root, &task, execution, "a").await;
                }
                "retry" if !terminal => {
                    r["retry"] = timed!(service.control_execution(json!({"projectId":project,
                        "executionId":execution,"requestId":format!("audit-retry-{task}"),"action":"retry"})));
                }
                "stop_close_delete" => {
                    if !terminal && state != "COMPLETED" {
                        r["stop"] = timed!(service.control_execution(json!({"projectId":project,
                            "executionId":execution,"requestId":format!("audit-stop-{task}"),"action":"pause",
                            "expectedVersion":item["version"]})));
                    }
                    // Fresh version after Stop, as the window rereads it.
                    let mut now = Vec::new();
                    if let Ok(v) = native(service, project, "tasks.get", json!({"id":task})).await {
                        now.push(v["data"]["value"].clone());
                    }
                    let version = version_of(&now, &task)
                        .or_else(|| item["version"].as_str().map(str::to_owned));
                    if !terminal && state != "COMPLETED" {
                        r["closeExecution"] = timed!(service.control_execution(json!({"projectId":project,
                            "executionId":execution,"requestId":format!("audit-close-{task}"),"action":"close_task",
                            "confirmed":true,"expectedVersion":version})));
                    }
                    let close = machine(
                        service,
                        "product.task",
                        Some(project),
                        json!({"action":"close","taskId":task,"choice":"cancel","confirmed":true}),
                    )
                    .await;
                    r["closeCard"] = outcome(&close);
                    let archive = machine(
                        service,
                        "product.task",
                        Some(project),
                        json!({"action":"archive","taskId":task}),
                    )
                    .await;
                    r["archive"] = outcome(&archive);
                    let purge = machine(
                        service,
                        "product.task",
                        Some(project),
                        json!({"action":"purge","taskId":task,"confirmed":true}),
                    )
                    .await;
                    r["purge"] = outcome(&purge);
                }
                _ => {}
            }
        }
        results.push(r);
    }
    report["controlWait"] = quiet(root, Duration::from_secs(60)).await;
    report["controls"] = json!(results);
    report["afterControls"] = db_tasks(root, before);
}

/// The service requires private (owner-only) state files and directories.
fn private_tree(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(path).unwrap();
    let mode = if meta.is_dir() { 0o700 } else { 0o600 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    if meta.is_dir() {
        for entry in std::fs::read_dir(path).unwrap() {
            private_tree(&entry.unwrap().path());
        }
    }
}
async fn service(root: &Path) -> Value {
    let state = PathBuf::from(var("AUDIT_STATE").expect("AUDIT_STATE"));
    let mut project = var("AUDIT_PROJECT_ID").expect("AUDIT_PROJECT_ID");
    if project == "auto" {
        // The identity this store already binds to its root, else a fresh one.
        let known: Option<String> = live(root).ok().and_then(|s| {
            s.read(|db| {
                Ok(db
                    .query_row(
                        "SELECT project_id FROM workspace_projects WHERE project_root=?",
                        [root.to_string_lossy()],
                        |r| r.get(0),
                    )
                    .ok())
            })
            .ok()
            .flatten()
        });
        project = known.unwrap_or_else(|| {
            use sha2::Digest;
            let d = sha2::Sha256::digest(root.to_string_lossy().as_bytes());
            format!(
                "wp_{}",
                d[..16]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            )
        });
    }
    if let Some(src) = var("AUDIT_PRODUCT_SRC") {
        let from = PathBuf::from(src).join(format!("{project}.json"));
        if from.is_file() {
            std::fs::create_dir_all(state.join("product")).unwrap();
            std::fs::copy(&from, state.join("product").join(format!("{project}.json"))).unwrap();
        }
    }
    let name = var("AUDIT_NAME").unwrap_or_else(|| "Audit".into());
    let wanted: Vec<String> = var("AUDIT_CONTROLS")
        .map(|c| c.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let skip_reads = var("AUDIT_SKIP_READS").is_some();
    let mut report_marker = false;
    std::fs::create_dir_all(state.join("product")).unwrap();
    std::fs::write(
        state.join("projects.json"),
        serde_json::to_vec(
            &json!([{"projectId":project,"canonicalRoot":root,"displayName":name,
            "activation":{"actorId":format!("uid:{}", unsafe { libc::getuid() }),"recordedAt":1}}]),
        )
        .unwrap(),
    )
    .unwrap();
    private_tree(&state);
    // A dev-channel build only opens a store its dev ownership marker binds. A release store
    // copied in as `.cortex-dev` has none: bind the copy, exactly as `project_guard::reserve` would.
    if workspace_protocol::channel::current().is_dev() {
        use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
        let marker = root.join(".cortex-dev/owner.json");
        let m = std::fs::metadata(root.join(".cortex-dev")).unwrap();
        // A copied dev store has a new directory identity: rebind the copy's own marker.
        let bound = std::fs::read(&marker)
            .ok()
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok());
        if bound
            .as_ref()
            .is_some_and(|b| b["store"] != json!([m.dev(), m.ino()]))
        {
            std::fs::remove_file(&marker).unwrap();
        }
        if !marker.exists() {
            let body = json!({"channel":"dev","root":root,"uid":unsafe { libc::getuid() },"store":[m.dev(),m.ino()]});
            use std::io::Write;
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&marker)
                .unwrap()
                .write_all(&serde_json::to_vec(&body).unwrap())
                .unwrap();
            report_marker = true;
        }
    }
    let runtime = state.parent().unwrap().join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::set_permissions(
        &runtime,
        std::os::unix::fs::PermissionsExt::from_mode(0o700),
    )
    .unwrap();
    let config: Config = Config {
        runtime_dir: runtime,
        state_dir: state.clone(),
        allowed_root: PathBuf::from("/"),
        launch: LaunchSpec::bare(),
        request_timeout: Duration::from_secs(30),
        shutdown_timeout: Duration::from_secs(3),
        event_capacity: 1024,
    };
    let mut report = json!({"mode":"service","root":root,"projectId":project,"controls":wanted,
        "channel":workspace_protocol::channel::current().name});
    report["devMarkerSynthesized"] = json!(report_marker);
    let service = match Service::bind_native(config.clone(), Arc::new(NoEngine)).await {
        Ok(s) => s,
        Err(e) => {
            report["bind"] = json!({"ok":false,"code":code(&e)});
            return report;
        }
    };
    let pre = db_tasks(root, i64::MAX);
    let since = pre["maxEvent"].as_i64().unwrap_or(0);
    report["pre"] = pre;
    report["prePage"] = match native(
        &service,
        &project,
        "tasks.page",
        json!({"target":project,"cursor":null,"limit":50}),
    )
    .await
    {
        Ok(v) => json!({"ok":true,"items":v["data"]["value"]["items"].as_array().map(|a|a.iter()
            .map(|i|json!({"taskId":i["taskId"],"runtimeState":i["runtimeState"],"phase":i["phase"]})).collect::<Vec<_>>())}),
        Err(e) => json!({"ok":false,"code":e}),
    };
    let started = Instant::now();
    let activated =
        tokio::time::timeout(Duration::from_secs(120), service.activate(&project)).await;
    report["activate"] = match activated {
        Ok(Ok(v)) => json!({"ok":true,"reply":v,"ms":started.elapsed().as_millis() as u64}),
        Ok(Err(e)) => json!({"ok":false,"code":code(&e),"ms":started.elapsed().as_millis() as u64}),
        Err(_) => json!({"ok":false,"code":"audit_timeout"}),
    };
    report["afterActivateSync"] = db_tasks(root, since);
    report["recoveryWait"] = quiet(root, Duration::from_secs(90)).await;
    report["afterRecovery"] = db_tasks(root, since);
    if !skip_reads {
        reads(&service, &project, &mut report).await;
    }
    if !wanted.is_empty() {
        controls(&service, &project, root, &wanted, &mut report).await;
    }
    let _ = tokio::time::timeout(Duration::from_secs(20), service.stop()).await;
    if wanted.iter().any(|w| w == "restart") {
        // A service restart on the same store, then the owner's next message, Close and Delete sprint.
        drop(service);
        let before = max_event(root);
        match Service::bind_native(config, Arc::new(NoEngine)).await {
            Err(e) => report["restart"] = json!({"bind":code(&e)}),
            Ok(again) => {
                let activated =
                    tokio::time::timeout(Duration::from_secs(120), again.activate(&project)).await;
                let mut restart = json!({"activate":match activated {Ok(Ok(_))=>json!("ok"),Ok(Err(e))=>json!(code(&e)),Err(_)=>json!("audit_timeout")}});
                restart["wait"] = quiet(root, Duration::from_secs(60)).await;
                restart["recovery"] = db_tasks(root, before);
                let mut cursor = Value::Null;
                let mut items = Vec::new();
                for _ in 0..100 {
                    let Ok(v) = native(
                        &again,
                        &project,
                        "tasks.page",
                        json!({"target":project,"cursor":cursor,"limit":50}),
                    )
                    .await
                    else {
                        break;
                    };
                    items.extend(
                        v["data"]["value"]["items"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default(),
                    );
                    cursor = v["data"]["value"]["nextCursor"].clone();
                    if cursor.is_null() {
                        break;
                    }
                }
                let mut probes = Vec::new();
                for item in &items {
                    let (Some(execution), Some(task)) =
                        (item["executionId"].as_str(), item["taskId"].as_str())
                    else {
                        continue;
                    };
                    if matches!(
                        item["runtimeState"].as_str(),
                        Some("COMPLETED" | "CANCELLED" | "FAILED")
                    ) {
                        continue;
                    }
                    let message = timed!(again.prompt_execution(
                        &project,
                        execution,
                        "Audit probe: after restart.",
                        Some(&format!("audit-restart-{task}")),
                        json!([])
                    ));
                    quiet(root, Duration::from_secs(30)).await;
                    let after_message = observe(&again, &project, root, task).await;
                    let version = native(&again, &project, "tasks.get", json!({"id":task}))
                        .await
                        .ok()
                        .and_then(|v| v["data"]["value"]["version"].as_str().map(str::to_owned));
                    let close = timed!(again.control_execution(json!({"projectId":project,"executionId":execution,
                        "requestId":format!("audit-restart-close-{task}"),"action":"close_task","confirmed":true,"expectedVersion":version})));
                    quiet(root, Duration::from_secs(20)).await;
                    probes.push(json!({"taskId":task,"message":message,"afterMessage":after_message,"close":close,
                        "afterClose":observe(&again, &project, root, task).await}));
                }
                restart["probes"] = json!(probes);
                let sprints = machine(&again, "product.read", Some(&project), json!({})).await;
                let mut out = Vec::new();
                for s in sprints
                    .ok()
                    .and_then(|p| p["sprints"].as_array().cloned())
                    .unwrap_or_default()
                {
                    let reply = machine(
                        &again,
                        "product.sprint",
                        Some(&project),
                        json!({"action":"delete","sprintId":s["id"],"confirmed":true}),
                    )
                    .await;
                    out.push(json!({"sprint":s["id"],"reply":match reply {Ok(v)=>json!({"done":v["done"],"reason":v["reason"].as_str().map(|r|short(r,300))}),Err(e)=>json!({"error":e})}}));
                }
                restart["sprintDeletes"] = json!(out);
                restart["final"] = db_tasks(root, before);
                report["restart"] = restart;
                let _ = tokio::time::timeout(Duration::from_secs(20), again.stop()).await;
            }
        }
    }
    report
}

/// The task as the window reads it, plus its durable blocker.
async fn observe(service: &Service, project: &str, root: &Path, task: &str) -> Value {
    let page = native(service, project, "tasks.get", json!({"id":task})).await;
    let blocker = live(root).ok().and_then(|s| {
        s.read(|db| {
            Ok(db
                .query_row(
                    "SELECT r.state,r.blocker FROM workspace_tasks w JOIN workspace_executions e ON e.execution_id=w.execution_id
                     JOIN runtime_tasks r ON r.task_id=e.runtime_task_id WHERE w.task_id=?",
                    [task],
                    |r| Ok(json!({"state":r.get::<_,String>(0)?,"blocker":r.get::<_,Option<String>>(1)?})),
                )
                .ok())
        })
        .ok()
        .flatten()
    });
    json!({"page":page.map(|v|json!({"runtimeState":v["data"]["value"]["runtimeState"],"phase":v["data"]["value"]["phase"],
        "waitReason":v["data"]["value"]["waitReason"]})).unwrap_or_else(|e|json!({"error":e})),"durable":blocker})
}
/// Answer every pending, answerable question of one execution with free text, as the window does.
async fn answer_pending(service: &Service, project: &str, execution: &str, key: &str) -> Value {
    let ask = json!({"protocol":{"major":1,"minor":8},"projectId":project,"requestId":"audit-decisions",
        "operation":"decisions.list","target":execution,"cursor":null,"limit":100});
    let Ok(page) = service.workspace(ask).await else {
        return json!({"listed":false});
    };
    let mut out = Vec::new();
    for d in page["items"].as_array().cloned().unwrap_or_default() {
        if d["status"] != "pending"
            || !d["allowedOperations"]
                .as_array()
                .is_some_and(|a| a.iter().any(|o| o == "decisions.answer"))
        {
            continue;
        }
        let answers = json!([{"decisionId":d["decisionId"],"expectedVersion":d["version"],"questionId":d["questionId"],
            "optionId":null,"text":"Try again."}]);
        let command = json!({"protocol":{"major":1,"minor":8},"projectId":project,"actorId":"audit",
            "requestId":format!("audit-answer-{key}"),"operation":"decisions.answer",
            "target":if d["batchId"].is_null() {d["decisionId"].clone()} else {d["batchId"].clone()},
            "expectedVersion":d["version"],"ownerEpoch":d["ownerEpoch"],
            "payload":{"executionId":execution,"acpSessionId":d["acpSessionId"],"batchId":d["batchId"],"answers":answers}});
        out.push(
            match tokio::time::timeout(Duration::from_secs(40), service.workspace(command)).await {
                Ok(Ok(v)) => json!({"ok":true,"outcome":v["outcome"]}),
                Ok(Err(e)) => json!({"ok":false,"code":code(&e)}),
                Err(_) => json!({"ok":false,"code":"audit_timeout"}),
            },
        );
    }
    json!(out)
}
/// The owner's sequence on a stuck task: message, answer the stop question, message again.
async fn chain(
    service: &Service,
    project: &str,
    root: &Path,
    task: &str,
    execution: &str,
    key: &str,
) -> Value {
    let mut steps = Vec::new();
    let first = timed!(service.prompt_execution(
        project,
        execution,
        "Audit probe: continue.",
        Some(&format!("audit-chain1{key}-{task}")),
        json!([])
    ));
    quiet(root, Duration::from_secs(30)).await;
    steps.push(
        json!({"step":"message","reply":first,"after":observe(service, project, root, task).await}),
    );
    let answered = answer_pending(service, project, execution, &format!("{key}-{task}")).await;
    quiet(root, Duration::from_secs(30)).await;
    steps.push(json!({"step":"answer","reply":answered,"after":observe(service, project, root, task).await}));
    let second = timed!(service.prompt_execution(
        project,
        execution,
        "Audit probe: continue again.",
        Some(&format!("audit-chain2{key}-{task}")),
        json!([])
    ));
    quiet(root, Duration::from_secs(30)).await;
    steps.push(json!({"step":"message","reply":second,"after":observe(service, project, root, task).await}));
    json!(steps)
}
