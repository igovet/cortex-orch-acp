//! Standalone retained worker projections through Core's live read-only store.
//! These are display facts, never result acceptance or scheduler authority.
use super::{
    context,
    native::{refuse, Result},
    worker_history::{self, bytes, dump, History},
};
use cortex_core::{CoreError, CoreResult, OpenMode};
use rusqlite::{
    types::{Value as Sql, ValueRef},
    Connection,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

pub(super) fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64() != Some(0.0),
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}
pub(super) fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        _ => dump(v, false),
    }
}
pub(super) fn object(s: &str) -> Value {
    serde_json::from_str::<Value>(s)
        .ok()
        .filter(Value::is_object)
        .unwrap_or(json!({}))
}
pub(super) fn query(db: &Connection, sql: &str, args: &[Value]) -> CoreResult<Vec<Value>> {
    let args = args
        .iter()
        .map(|v| match v {
            Value::Null => Ok(Sql::Null),
            Value::String(s) => Ok(Sql::Text(s.clone())),
            Value::Bool(b) => Ok(Sql::Integer(i64::from(*b))),
            Value::Number(n) => n
                .as_i64()
                .map(Sql::Integer)
                .or_else(|| n.as_f64().filter(|_| !n.is_u64()).map(Sql::Real))
                .ok_or(CoreError::Refused("invalid_request")),
            // sqlite3 cannot bind a list or object either; the request is refused.
            _ => Err(CoreError::Refused("invalid_request")),
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let mut q = db.prepare(sql)?;
    let names = q.column_names().map(|s| s.to_string()).collect::<Vec<_>>();
    let rows = q
        .query_map(rusqlite::params_from_iter(args), |r| {
            let mut map = serde_json::Map::new();
            for (i, k) in names.iter().enumerate() {
                let v = match r.get_ref(i)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => json!(n),
                    ValueRef::Text(_) => json!(r.get::<_, String>(i)?),
                    _ => return Err(rusqlite::Error::InvalidQuery),
                };
                map.insert(k.clone(), v);
            }
            Ok(Value::Object(map))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    // The legacy reader has no row cap beyond each statement's own LIMIT; a result
    // beyond these bounds is refused with the bounded worker-content outcome.
    if rows.len() > 100_000 || rows.iter().map(bytes).sum::<usize>() > 64 * 1024 * 1024 {
        return Err(CoreError::Refused("worker_content_too_large"));
    }
    Ok(rows)
}
pub(super) fn read(root: &Path, project: &str, params: &Value) -> Result<Value> {
    let Some(store) = context::open(root, OpenMode::LiveReadOnly)? else {
        return Ok(json!([]));
    };
    store.read(|db| read_db(db, project, params)).map_err(|e| {
        let (code, message) = match e.code() {
            "coordinator_unknown_task" => {
                ("unknown_task", "This coordinator belongs to another task.")
            }
            "coordinator_invalid_cursor" => (
                "invalid_cursor",
                "This coordinator history is unavailable. Reopen the task.",
            ),
            "invalid_cursor" => (
                "invalid_cursor",
                "This activity page no longer belongs to this attempt. Reopen its history.",
            ),
            "worker_snapshot_too_large" => (
                "worker_snapshot_too_large",
                "Select fewer tasks to read this worker snapshot.",
            ),
            "worker_snapshot_tasks_limit" => (
                "worker_snapshot_too_large",
                "Select at most 100 tasks for a worker snapshot.",
            ),
            "worker_snapshot_assignments_limit" => (
                "worker_snapshot_too_large",
                "Select a specific worker to read its history.",
            ),
            "worker_snapshot_attempts_limit" => (
                "worker_snapshot_too_large",
                "Select a specific worker attempt to read its history.",
            ),
            "worker_content_too_large" => (
                "worker_content_too_large",
                "This worker metadata cannot fit a bounded activity page.",
            ),
            "invalid_request" => (
                "invalid_request",
                "Select tasks as a list of task identifiers.",
            ),
            "worker_content_no_room" => (
                "worker_content_too_large",
                "This worker's metadata leaves no room for an activity page.",
            ),
            _ => (
                e.code(),
                "The project store could not confirm this operation.",
            ),
        };
        refuse(code, message)
    })
}
pub(super) fn route_label(model: &Value, detail: &Value, parameters: &Value) -> Value {
    let Some(model) = model.as_str().filter(|s| !s.is_empty() && *s != "unknown") else {
        return Value::Null;
    };
    let label = detail["model_name"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{model} (alias)"));
    let effort = ["effort", "reasoning_effort", "thought_level"]
        .iter()
        .find_map(|k| parameters[*k].as_str().filter(|s| !s.is_empty()))
        .unwrap_or("");
    let effort = match effort {
        "engine_default" => "",
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "xhigh" => "Extra high",
        "minimal" => "Minimal",
        "max" => "Max",
        "none" => "None",
        s => s,
    };
    json!({"model":label,"effort":effort.replace('_'," ")})
}
fn routes(
    db: &Connection,
    runtime: &str,
    attempt: &Value,
    assignment: &Value,
) -> CoreResult<Vec<Value>> {
    let mut chain = vec![];
    let mut current = attempt.clone();
    while truthy(&current) {
        if chain.contains(&current) || chain.len() >= 50 {
            return Ok(vec![]);
        }
        chain.push(current.clone());
        let row=query(db,"SELECT predecessor_attempt_id FROM attempt_route_audit WHERE task_id=? AND attempt_id=? ORDER BY recorded_at DESC,rowid DESC LIMIT 1",&[json!(runtime),current])?;
        current = row
            .first()
            .map(|v| v["predecessor_attempt_id"].clone())
            .unwrap_or(Value::Null);
    }
    let mut found = vec![];
    for (i, attempt) in chain.into_iter().rev().enumerate() {
        let audit=query(db,"SELECT model,decision_detail_json FROM attempt_route_audit WHERE task_id=? AND attempt_id=? ORDER BY recorded_at DESC,rowid DESC LIMIT 1",&[json!(runtime),attempt.clone()])?;
        let confirmed=query(db,"SELECT payload_json FROM task_events WHERE task_id=? AND (assignment_ref=? OR assignment_ref IS NULL) AND event_type='progress' AND json_valid(payload_json) AND json_extract(payload_json,'$.kind')='worker_configuration_confirmed' AND json_extract(payload_json,'$.attempt_id')=? ORDER BY id DESC LIMIT 1",&[json!(runtime),assignment.clone(),attempt])?;
        let v = confirmed
            .first()
            .map(|r| object(r["payload_json"].as_str().unwrap_or("")))
            .unwrap_or(json!({}));
        let route = if truthy(&v["confirmation_evidence_ref"]) {
            let detail = audit
                .first()
                .filter(|a| a["model"] == v["model"])
                .map(|a| object(a["decision_detail_json"].as_str().unwrap_or("")))
                .unwrap_or(json!({}));
            route_label(&v["model"], &detail, &v["parameters"])
        } else {
            Value::Null
        };
        found.push(json!({"n":i+1,"route":route}));
    }
    Ok(found)
}
fn read_db(db: &Connection, project: &str, params: &Value) -> CoreResult<Value> {
    let worker = params.get("workerId").unwrap_or(&Value::Null);
    if worker
        .as_str()
        .is_some_and(|s| s.starts_with("coordinator:"))
    {
        return super::coordinator::read(db, project, params);
    }
    let empty = vec![];
    let ids = match params.get("taskIds").filter(|v| truthy(v)) {
        None => &empty,
        Some(Value::Array(a)) => a,
        _ => return Err(CoreError::Refused("invalid_request")),
    };
    if ids.len() > 100 {
        return Err(CoreError::Refused("worker_snapshot_tasks_limit"));
    }
    let mut found = vec![];
    for task in ids {
        // Only text identities exist; any other element matches no task.
        if !task.is_string() {
            continue;
        }
        if query(
            db,
            "SELECT 1 FROM workspace_tasks WHERE task_id=? AND project_id=?",
            &[task.clone(), json!(project)],
        )?
        .is_empty()
        {
            continue;
        }
        let executions=query(db,"SELECT execution_id,runtime_task_id FROM workspace_executions WHERE task_id=? ORDER BY rowid DESC LIMIT 1",&[task.clone()])?;
        let Some(execution) = executions.first() else {
            continue;
        };
        // An execution not yet bound to a runtime task has no workers to show.
        let Some(runtime) = execution["runtime_task_id"].as_str() else {
            continue;
        };
        let assignments=query(db,"SELECT * FROM work_assignments WHERE task_id=? AND (? IS NULL OR assignment_id=? OR assignment_id IN (SELECT assignment_id FROM work_assignment_attempts WHERE attempt_id=?)) ORDER BY created_at LIMIT 101",&[json!(runtime),worker.clone(),worker.clone(),worker.clone()])?;
        if assignments.len() > 100 {
            return Err(CoreError::Refused("worker_snapshot_assignments_limit"));
        }
        let mut work = vec![];
        for assignment in assignments {
            let attempts=query(db,"SELECT * FROM work_assignment_attempts WHERE assignment_id=? ORDER BY attempt_no LIMIT 1001",&[assignment["assignment_id"].clone()])?;
            let total = attempts
                .last()
                .map(|a| a["attempt_no"].clone())
                .unwrap_or(json!(0));
            if attempts.is_empty() {
                if worker.is_null() || *worker == assignment["assignment_id"] {
                    work.push((
                        assignment.clone(),
                        Value::Null,
                        assignment["assignment_id"].clone(),
                        total,
                    ));
                }
            } else {
                for (i, a) in attempts.iter().enumerate() {
                    let identity = if i + 1 == attempts.len() {
                        assignment["assignment_id"].clone()
                    } else {
                        a["attempt_id"].clone()
                    };
                    if worker.is_null() || *worker == identity {
                        work.push((assignment.clone(), a.clone(), identity, total.clone()));
                    }
                }
            }
            if work.len() > 100 {
                return Err(CoreError::Refused("worker_snapshot_attempts_limit"));
            }
        }
        for (assignment, attempt, identity, total) in work {
            let requested=query(db,"SELECT assignment_ref,payload_json FROM task_events WHERE task_id=? AND event_type='delegation_requested' AND json_valid(payload_json) AND json_extract(payload_json,'$.work_attempt_ref')=? ORDER BY id DESC LIMIT 1",&[json!(runtime),attempt["attempt_id"].clone()])?;
            let spec = requested
                .first()
                .map(|v| object(v["payload_json"].as_str().unwrap_or("")))
                .unwrap_or(json!({}));
            let provisions=query(db,"SELECT p.* FROM delegation_provisioning p JOIN task_events e ON e.task_id=p.task_id AND e.assignment_ref=p.idempotency_key WHERE p.task_id=? AND e.event_type='delegation_requested' AND json_extract(e.payload_json,'$.work_attempt_ref')=? ORDER BY p.created_at DESC LIMIT 1",&[json!(runtime),attempt["attempt_id"].clone()])?;
            let provision = provisions.first();
            let mut refs = if attempt.is_null() {
                vec![
                    assignment["assignment_id"].clone(),
                    assignment["idempotency_key"].clone(),
                ]
            } else {
                vec![attempt["attempt_id"].clone()]
            };
            if let Some(p) = provision {
                refs.extend([p["idempotency_key"].clone(), p["attempt_id"].clone()]);
            } else if let Some(r) = requested.first() {
                refs.push(r["assignment_ref"].clone());
            }
            let binding = format!(
                "{:x}",
                Sha256::digest(
                    dump(
                        &json!([project, task, assignment["assignment_id"], refs]),
                        true
                    )
                    .as_bytes()
                )
            )[..24]
                .to_owned();
            let marks = vec!["?"; refs.len()].join(",");
            let mut args = vec![json!(runtime)];
            args.extend(refs.clone());
            let terminal=query(db,&format!("SELECT event_type FROM task_events WHERE task_id=? AND assignment_ref IN ({marks}) AND event_type IN ('worker_result','assignment_failed','assignment_cancelled') ORDER BY id DESC LIMIT 1"),&args)?;
            let raw_status = if attempt.is_null() {
                &assignment["status"]
            } else {
                &attempt["status"]
            };
            let status = match terminal.first().and_then(|v| v["event_type"].as_str()) {
                Some("worker_result") => "completed",
                Some("assignment_failed") => "failed",
                Some("assignment_cancelled") => "cancelled",
                _ => match raw_status.as_str().unwrap_or("") {
                    "completed" | "accepted" | "reported" => "completed",
                    "failed" => "failed",
                    "cancelled" => "cancelled",
                    "running" | "active" | "dispatched" => "working",
                    "pending" | "ready" | "blocked" => "pending",
                    _ => "unknown",
                },
            };
            let role = spec["role"]
                .as_str()
                .unwrap_or(assignment["node_key"].as_str().unwrap_or(""));
            let results=query(db,&format!("SELECT payload_json FROM task_events WHERE task_id=? AND assignment_ref IN ({marks}) AND event_type='worker_result' ORDER BY id DESC LIMIT 1"),&args)?;
            let result = if let Some(r) = results.first() {
                let p = object(r["payload_json"].as_str().unwrap_or(""));
                // A checks field that is not a list cannot be read as checks. Show one
                // explicit unverified row, never an empty (clean-looking) list.
                let unreadable = [
                    json!({"name":"Worker checks unavailable","outcome":"unverified","blocker":"The recorded checks could not be read."}),
                ];
                let active: &[Value] = match p.get("checks") {
                    None => &[],
                    Some(Value::Array(a)) => a,
                    Some(_) => &unreadable,
                };
                // Display events omit probe reasons: revalidate the matching accepted
                // result with the native assessor, like Python `active_checks`. A
                // withdrawal that cannot be re-derived stays a displayed failed claim.
                let mut kept: Vec<Value> = active.to_vec();
                if let Some(provision) = provision {
                    let node=query(db,"SELECT result_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",&[json!(runtime),provision["idempotency_key"].clone()])?;
                    if let Some(n) = node.first().filter(|n| truthy(&n["result_json"])) {
                        let n = object(n["result_json"].as_str().unwrap_or(""));
                        if let Some(stored) = n.get("checks").and_then(Value::as_array) {
                            let identities = |a: &[Value]| {
                                a.iter()
                                    .map(|v| json!([v["name"], v["outcome"], v["tool_call_ref"]]))
                                    .collect::<Vec<_>>()
                            };
                            if identities(stored) == identities(active) {
                                let omitted = super::task_facts_work::withdrawn(
                                    db,
                                    runtime,
                                    provision["idempotency_key"].as_str(),
                                    &n,
                                    stored,
                                );
                                kept = stored
                                    .iter()
                                    .zip(omitted)
                                    .filter(|(_, w)| !w)
                                    .map(|(c, _)| c.clone())
                                    .collect();
                            }
                        }
                    }
                }
                let active: &[Value] = &kept;
                let checks=active.iter().filter(|v|v.is_object()).map(|c|json!({"name":c.get("name").cloned().unwrap_or(json!("")),"outcome":match c["outcome"].as_str(){Some(s @ ("passed"|"failed"|"not_run"|"unverified"))=>s,_=>"unverified"},"detail":if truthy(&c["blocker"]){c["blocker"].clone()}else{json!("")},"source":"worker"})).collect::<Vec<_>>();
                let summary = p
                    .get("summary")
                    .filter(|v| truthy(v))
                    .or_else(|| p.get("text").filter(|v| truthy(v)))
                    .map(worker_history::py_str)
                    .unwrap_or_default()
                    .chars()
                    .take(8000)
                    .collect::<String>();
                json!({"summary":summary,"checks":checks,"reportId":p["report_ref"]})
            } else {
                Value::Null
            };
            let route_attempts = match provision {
                Some(p) => routes(db, runtime, &p["attempt_id"], &p["idempotency_key"])?,
                None => vec![],
            };
            let goal = spec
                .get("title")
                .filter(|v| truthy(v))
                .unwrap_or(&assignment["node_key"]);
            let mut item = json!({"id":identity,"taskId":task,"executionId":execution["execution_id"],"role":role.strip_prefix("worker-").unwrap_or(role),
                "status":status,"lane":if role.contains("review"){"review"}else{"execution"},"goal":worker_history::py_str(goal).chars().take(400).collect::<String>(),
                "now":null,"waitsFor":null,"scope":[],"route":route_attempts.last().map(|a|&a["route"]),"routeAttempts":route_attempts,
                "attempt":if attempt.is_null(){Value::Null}else{json!({"n":attempt["attempt_no"],"of":total})},
                "elapsedMs":null,"runningSince":if status=="working"{worker_history::at(db,&assignment["created_at"])?}else{Value::Null},
                "context":null,"usage":null,"result":result});
            let metadata = bytes(&item);
            if metadata > 60000 {
                return Err(CoreError::Refused("worker_content_too_large"));
            }
            let snapshot = bytes(&json!(found
                .iter()
                .chain(std::iter::once(&item))
                .collect::<Vec<_>>()));
            let budget = (65536 - metadata - 2).min(524288usize.saturating_sub(snapshot));
            let page = History {
                runtime,
                refs: &refs,
                channel: provision.and_then(|p| p["channel_id"].as_str()),
                task: task.as_str().unwrap(),
                worker: identity.as_str().unwrap_or(""),
                status,
                binding: &binding,
            }
            .page(db, &params["cursor"], budget)?;
            if !worker.is_null()
                && page["activityPartial"] == true
                && page["activity"].as_array().is_some_and(Vec::is_empty)
            {
                return Err(CoreError::Refused("worker_content_no_room"));
            }
            item.as_object_mut()
                .unwrap()
                .extend(page.as_object().unwrap().clone());
            found.push(item);
        }
    }
    if bytes(&json!({"ok":true,"result":found})) > 786432 {
        return Err(CoreError::Refused("worker_snapshot_too_large"));
    }
    Ok(json!(found))
}
