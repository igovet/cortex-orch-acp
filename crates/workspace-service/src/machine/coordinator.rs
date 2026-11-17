//! Retained coordinator display pages. No scheduler, writes or engine calls.
use super::{
    worker_history::{at, bytes, detail, dump, py_str},
    workers::{object, query, route_label, truthy},
};
use cortex_core::{CoreError, CoreResult};
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const STEP: &str = "Workspace step: ";
fn digest(v: &Value) -> String {
    format!("{:x}", Sha256::digest(v.to_string().as_bytes()))
}
fn invalid<T>() -> CoreResult<T> {
    Err(CoreError::Refused("coordinator_invalid_cursor"))
}
fn hidden(v: &Value) -> bool {
    let name = v
        .as_str()
        .unwrap_or("")
        .trim()
        .rsplit("__")
        .next()
        .unwrap_or("")
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();
    super::worker_history::HIDDEN.contains(&name.as_str())
}
fn merge(old: &mut Value, new: &Value) {
    // page_tx's sparse updates preserve empty inputs, not empty display fields.
    for (k, v) in new.as_object().into_iter().flatten() {
        if v.is_null() {
            continue;
        }
        if ["rawInput", "raw_input", "input"].contains(&k.as_str()) {
            if truthy(&old[k]) && !truthy(v) {
                continue;
            }
            if old[k].is_object() && v.is_object() {
                enrich(&mut old[k], v);
                continue;
            }
        }
        old[k] = v.clone();
    }
}
fn enrich(old: &mut Value, new: &Value) {
    for (k, v) in new.as_object().into_iter().flatten() {
        if old[k].is_object() && v.is_object() {
            enrich(&mut old[k], v);
        } else {
            old[k] = v.clone();
        }
    }
}
fn splitlines(s: &str) -> Vec<String> {
    let mut lines = vec![];
    let mut line = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if matches!(
            c as u32,
            10 | 13 | 11 | 12 | 28 | 29 | 30 | 133 | 8232 | 8233
        ) {
            lines.push(std::mem::take(&mut line));
            if c == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
        } else {
            line.push(c);
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}
fn activity(
    db: &Connection,
    kind: &str,
    body: &str,
    seq: i64,
    stamp: &Value,
    selected: Option<&(Value, String)>,
    status: &Value,
) -> CoreResult<Value> {
    let preview = if body.len() > 8192 {
        let mut n = 8000;
        while !body.is_char_boundary(n) {
            n -= 1;
        }
        format!(
            "{}\n[Content clipped: first {n} of {} UTF-8 bytes]",
            &body[..n],
            body.len()
        )
    } else {
        body.into()
    };
    let mut item = json!({"sequence":seq,"kind":if kind=="user"{"message"}else{"note"},
        "text":preview,"at":at(db,stamp)?});
    if let Some((d, args)) = selected {
        item["kind"] = json!("tool");
        item["text"] = d
            .get("title")
            .filter(|v| truthy(v))
            .cloned()
            .unwrap_or(json!("Tool activity"));
        item["args"] = json!(args);
        item["error"] = json!(status == "failed");
        if truthy(&d["callId"]) {
            item["toolCallRef"] = d["callId"].clone();
        }
        if truthy(&d["command"]) {
            item["kind"] = json!("command");
            item["text"] = d["command"].clone();
            item["command"] = json!({"state":"native","exit":null,"took":null,
                "tail":d.get("output").cloned().unwrap_or(json!("")),"reason":null});
        } else if d["kind"] == "edit" || d.get("newText").is_some() {
            let old = splitlines(d["oldText"].as_str().unwrap_or(""));
            let new = splitlines(d["newText"].as_str().unwrap_or(""));
            item["kind"] = json!("edit");
            item["diff"] = json!({"add":new.len(),"del":old.len(),
                "lines":old.iter().map(|s|format!("-{s}")).chain(new.iter().map(|s|format!("+{s}"))).collect::<Vec<_>>()});
        }
    }
    Ok(item)
}
fn route(db: &Connection, runtime: &Value) -> CoreResult<Value> {
    let configs=query(db,"SELECT c.*,x.route_model FROM configuration_revisions c JOIN configuration_route_extras x ON x.revision_id=c.id AND x.task_id=c.task_id WHERE c.task_id=? AND c.state='active' AND c.confirmation_evidence_ref IS NOT NULL ORDER BY c.revision DESC LIMIT 1",&[runtime.clone()])?;
    let Some(c) = configs
        .first()
        .filter(|c| truthy(&c["confirmation_evidence_ref"]))
    else {
        return Ok(Value::Null);
    };
    if !query(db,"SELECT 1 FROM configuration_revisions WHERE task_id=? AND revision>? AND state IN ('pending_next_use','indeterminate') LIMIT 1",&[runtime.clone(),c["revision"].clone()])?.is_empty() {
        return Ok(Value::Null);
    }
    let mut audits=query(db,"SELECT * FROM attempt_route_audit WHERE task_id=? AND role='coordinator' AND config_revision_id=? AND model=? ORDER BY recorded_at DESC,rowid DESC LIMIT 1",&[runtime.clone(),c["id"].clone(),c["route_model"].clone()])?;
    let current = query(
        db,
        "SELECT coordinator_attempt_ref FROM task_orchestration WHERE task_id=?",
        &[runtime.clone()],
    )?;
    if let Some(current) = current
        .first()
        .filter(|v| truthy(&v["coordinator_attempt_ref"]))
    {
        audits=query(db,"SELECT * FROM attempt_route_audit WHERE task_id=? AND attempt_id=? ORDER BY recorded_at DESC,rowid DESC LIMIT 1",&[runtime.clone(),current["coordinator_attempt_ref"].clone()])?;
        if audits.first().is_none_or(|a| {
            a["model"] != c["route_model"]
                || a["engine"] != c["route_engine"]
                || a["connection_id"] != c["route_connection_id"]
                || a["config_revision_id"] != c["id"]
        }) {
            return Ok(Value::Null);
        }
    }
    let d = audits
        .first()
        .map(|a| object(a["decision_detail_json"].as_str().unwrap_or("")))
        .unwrap_or(json!({}));
    Ok(route_label(
        &c["route_model"],
        &d,
        &object(c["parameters"].as_str().unwrap_or("")),
    ))
}
enum Fit {
    Fits,
    TooLarge,
    Invalid,
}
fn page_fits(item: &Value, high: i64) -> Fit {
    match workspace_protocol::validate(
        "HistoryPage",
        &json!({"items":[item],"nextCursor":null,"highWatermark":high.to_string()}),
    ) {
        Ok(()) => Fit::Fits,
        Err(code) if code == "page_too_large" => Fit::TooLarge,
        // A malformed field is not a page-size problem: the row alone becomes an
        // explicit unavailable-content row (see `append`).
        Err(_) => Fit::Invalid,
    }
}
fn page(
    db: &Connection,
    project: &str,
    task: &str,
    session: &Value,
    cursor: &Value,
    runtime: &Value,
) -> CoreResult<Value> {
    let binding = digest(&json!([project, task, session, 100]))[..24].to_owned();
    // The coordinator page holds only the coordinator's own rows: a row whose identity
    // (`{attempt}:…` message id or tool call id) names a worker attempt of this task is
    // that worker's, read on its own page. Unprefixed rows and older stores stay here.
    let sql="SELECT seq,kind,text,NULL AS update_json,created_at,message_id AS ident FROM host_transcript WHERE session_id=? UNION ALL SELECT seq,'update',NULL,update_json,created_at,CASE WHEN json_valid(update_json) THEN json_extract(update_json,'$.toolCallId') END FROM host_updates WHERE session_id=?";
    const OWN: &str = "(typeof(ident)<>'text' OR instr(ident,':')=0 OR substr(ident,1,instr(ident,':')-1) NOT IN (SELECT value FROM json_each(?)))";
    let recorded: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='delegation_provisioning')",
        [],
        |r| r.get(0),
    )?;
    let attempts = json!(if recorded {
        query(
            db,
            "SELECT DISTINCT attempt_id FROM delegation_provisioning WHERE task_id=? AND role_id<>'orchestrator' AND attempt_id IS NOT NULL",
            &[runtime.clone()],
        )?
        .iter()
        .map(|r| r["attempt_id"].clone())
        .collect::<Vec<_>>()
    } else {
        vec![]
    })
    .to_string();
    let maximum: i64 = db.query_row(
        &format!("SELECT coalesce(max(seq),-1) FROM ({sql}) WHERE {OWN}"),
        rusqlite::params![session.as_str(), session.as_str(), attempts],
        |r| r.get(0),
    )?;
    let (mut after, mut high) = (-1, maximum);
    if !cursor.is_null() {
        let parts = cursor.as_str().unwrap_or("").split('.').collect::<Vec<_>>();
        if parts.len() != 4 || parts[0] != "wh1" || parts[1] != binding {
            return invalid();
        }
        let parse = |s: &str| s.parse::<i64>().ok().filter(|n| n.to_string() == s);
        let Some(h) = parse(parts[2]) else {
            return invalid();
        };
        let Some(a) = parse(parts[3]) else {
            return invalid();
        };
        if a < 0 || a > h || h > maximum {
            return invalid();
        }
        high = h;
        after = a;
    }
    let rows = query(
        db,
        &format!("SELECT * FROM ({sql}) WHERE seq>? AND seq<=? AND {OWN} ORDER BY seq LIMIT 101"),
        &[session.clone(), session.clone(), json!(after), json!(high), json!(attempts)],
    )?;
    // A sequence at the end of the integer range cannot be paged; the history is
    // reported unavailable with the existing reopen outcome.
    let watermark = match high.checked_add(1) {
        Some(w) => w,
        None => return invalid(),
    };
    let mut size =
        bytes(&json!({"items":[],"nextCursor":null,"highWatermark":watermark.to_string()}));
    let mut items = vec![];
    let mut consumed = after;
    for row in rows.iter().take(100) {
        let Some(seq) = row["seq"].as_i64() else {
            return invalid();
        };
        let Some(sequence) = seq.checked_add(1).filter(|s| *s > 0) else {
            return invalid();
        };
        let mut kind = if row["kind"] == "user_message" {
            "user"
        } else {
            "assistant"
        };
        let mut body = row["text"].as_str().map(str::to_owned);
        let mut selected = None;
        let mut status = Value::Null;
        if row["kind"] == "update" {
            let mut update: Value = serde_json::from_str(row["update_json"].as_str().unwrap_or(""))
                .unwrap_or(Value::Null);
            if update.is_object() {
                if truthy(&update["toolCallId"])
                    && matches!(
                        update["sessionUpdate"].as_str(),
                        Some("tool_call" | "tool_call_update")
                    )
                {
                    let prior=query(db,"SELECT update_json FROM host_updates WHERE session_id=? AND seq<=? AND json_extract(update_json,'$.toolCallId')=? ORDER BY seq DESC LIMIT 100",&[session.clone(),json!(seq),update["toolCallId"].clone()])?;
                    let mut merged = json!({});
                    let mut valid = true;
                    for r in prior.iter().rev() {
                        match serde_json::from_str::<Value>(r["update_json"].as_str().unwrap_or(""))
                        {
                            Ok(v) if v.is_object() => merge(&mut merged, &v),
                            _ => {
                                valid = false;
                                break;
                            }
                        }
                    }
                    if prior.len() >= 100 {
                        let first=query(db,"SELECT update_json FROM host_updates WHERE session_id=? AND seq<=? AND json_extract(update_json,'$.toolCallId')=? ORDER BY seq LIMIT 1",&[session.clone(),json!(seq),update["toolCallId"].clone()])?;
                        let mut initial = object(first[0]["update_json"].as_str().unwrap_or(""));
                        merge(&mut initial, &merged);
                        merged = initial;
                        merged["projection_partial"] = json!(true);
                    }
                    update = if valid { merged } else { Value::Null };
                }
            }
            if !update.is_object() {
                kind = "unknown";
                body = None;
            } else {
                let k = update["sessionUpdate"].as_str().unwrap_or("");
                if !matches!(k, "plan" | "tool_call" | "tool_call_update")
                    || hidden(&update["title"])
                {
                    consumed = seq;
                    continue;
                }
                if k == "plan" {
                    // Python iterates `update.get("entries", [])`: a missing, string or
                    // object value yields no entries; null/number/bool raise TypeError
                    // and the row becomes an unavailable-content row.
                    let entries_value = update.get("entries");
                    let entries = match entries_value {
                        Some(Value::Array(a)) => Some(a.as_slice()),
                        None | Some(Value::String(_)) | Some(Value::Object(_)) => Some(&[][..]),
                        _ => None,
                    };
                    let Some(entries) = entries else {
                        let item = make_item(
                            db,
                            session,
                            row,
                            sequence,
                            "unknown",
                            None,
                            Value::Null,
                            None,
                        )?;
                        if !append(&mut items, &mut size, item, watermark)? {
                            break;
                        }
                        consumed = seq;
                        continue;
                    };
                    body = Some(format!(
                        "Plan\n{}",
                        entries
                            .iter()
                            .filter(|e| e.is_object())
                            .map(|e| format!(
                                "{} · {}",
                                e.get("content").map(py_str).unwrap_or_default(),
                                e.get("status").map(py_str).unwrap_or("unknown".into())
                            ))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ));
                    kind = "system";
                } else {
                    status = update["status"].clone();
                    let (d, args) = detail(&update)?;
                    let title = update
                        .get("title")
                        .filter(|v| truthy(v))
                        .map(py_str)
                        .unwrap_or("Tool activity".into());
                    body = Some(
                        if (!["path", "command", "query", "newText"]
                            .iter()
                            .any(|k| d.get(*k).is_some())
                            && !truthy(&update["toolCallId"]))
                            || title.starts_with("Worker")
                        {
                            format!(
                                "{title}{}",
                                status
                                    .as_str()
                                    .map(|s| format!(" · {s}"))
                                    .unwrap_or_default()
                            )
                        } else if !truthy(&update["title"]) && k == "tool_call_update" {
                            // title is first in tool_detail's ordered JSON string.
                            let prefix = format!("{{\"title\": {}, ", dump(&d["title"], false));
                            // The title is always the first key; keep the text whole otherwise.
                            match args.strip_prefix(&prefix) {
                                Some(rest) => format!("{STEP}{{{rest}"),
                                None => format!("{STEP}{args}"),
                            }
                        } else {
                            // display_update restores the original title after
                            // tool_detail clipped selected fields to its budget.
                            let prefix = format!("{{\"title\": {}, ", dump(&d["title"], false));
                            let replacement =
                                format!("{{\"title\": {}, ", dump(&json!(title), false));
                            format!("{STEP}{}", args.replacen(&prefix, &replacement, 1))
                        },
                    );
                    selected = Some((d, args));
                    kind = "tool";
                }
            }
        }
        let item = make_item(db, session, row, sequence, kind, body, status, selected)?;
        if !append(&mut items, &mut size, item, watermark)? {
            break;
        }
        consumed = seq;
    }
    let more = !rows.is_empty() && consumed < high;
    let mut activities = vec![];
    for item in items {
        let a = if let Some(a) = item.get("activity") {
            a.clone()
        } else if let Some(body) = item["text"].as_str() {
            let selected = body.strip_prefix(STEP).map(|s| (object(s), s.to_owned()));
            activity(
                db,
                item["kind"].as_str().unwrap_or(""),
                body,
                item["sequence"].as_str().unwrap().parse().unwrap(),
                &item["recordedAt"],
                selected.as_ref(),
                &item["toolStatus"],
            )?
        } else {
            json!({"sequence":item["sequence"].as_str().unwrap().parse::<i64>().unwrap(),"text":"Content unavailable in the retained display history.","kind":"note","at":at(db,&item["recordedAt"])?})
        };
        activities.push(a);
    }
    Ok(
        json!({"activity":activities,"activityPartial":more,"activityCursor":more.then(||format!("wh1.{binding}.{high}.{consumed}"))}),
    )
}
fn make_item(
    db: &Connection,
    session: &Value,
    row: &Value,
    sequence: i64,
    kind: &str,
    body: Option<String>,
    status: Value,
    selected: Option<(Value, String)>,
) -> CoreResult<Value> {
    let mut item = json!({"historyId":format!("wh_{}",&digest(&json!([session,row["seq"]]))[..32]),
        "sequence":sequence.to_string(),"revision":"1","kind":kind,"text":body,"toolStatus":if status.is_string(){status.clone()}else{Value::Null},
        "recordedAt":row["created_at"],"availability":if body.is_some(){"available"}else{"unavailable"}});
    if let Some(body) = body {
        if body.chars().count() > 32000 {
            item["text"] = Value::Null;
            item["availability"] = json!("unavailable");
            item["reason"] = json!("content_exceeds_wire_limit");
        } else {
            item["activity"] = activity(
                db,
                kind,
                &body,
                sequence,
                &row["created_at"],
                selected.as_ref(),
                &status,
            )?;
        }
    }
    Ok(item)
}
fn append(
    items: &mut Vec<Value>,
    size: &mut usize,
    mut item: Value,
    high: i64,
) -> CoreResult<bool> {
    let len = bytes(&item);
    let fit = page_fits(&item, high);
    if !matches!(fit, Fit::Fits) || *size + len + if items.is_empty() { 0 } else { 2 } > 60000 - 256
    {
        if !items.is_empty() {
            return Ok(false);
        }
        item.as_object_mut().unwrap().remove("activity");
        let smaller = json!({"items":[item],"nextCursor":null,"highWatermark":high.to_string()});
        match page_fits(&item, high) {
            Fit::Invalid => {
                // Malformed retained fields: an explicit unavailable-content row that
                // keeps only the identity and position of the retained entry.
                item["text"] = Value::Null;
                item["toolStatus"] = Value::Null;
                item["availability"] = json!("unavailable");
                item["reason"] = json!("content_unreadable");
            }
            Fit::TooLarge => {
                item["text"] = Value::Null;
                item["availability"] = json!("unavailable");
                item["reason"] = json!("content_exceeds_page_limit");
            }
            Fit::Fits if bytes(&smaller) > 60000 - 256 => {
                item["text"] = Value::Null;
                item["availability"] = json!("unavailable");
                item["reason"] = json!("content_exceeds_page_limit");
            }
            Fit::Fits => {}
        }
    }
    *size += bytes(&item) + if items.is_empty() { 0 } else { 2 };
    items.push(item);
    Ok(true)
}
pub(super) fn read(db: &Connection, project: &str, params: &Value) -> CoreResult<Value> {
    let task = params["workerId"]
        .as_str()
        .unwrap()
        .strip_prefix("coordinator:")
        .unwrap();
    let empty = json!([]);
    let task_ids = params
        .get("taskIds")
        .filter(|v| truthy(v))
        .unwrap_or(&empty);
    // Python `task_id not in task_ids`: list equality, string substring or object key.
    let listed = match task_ids {
        Value::Array(a) => a.iter().any(|v| v == task),
        Value::String(s) => s.contains(task),
        Value::Object(m) => m.contains_key(task),
        _ => false,
    };
    if !listed {
        return Err(CoreError::Refused("coordinator_unknown_task"));
    }
    let rows=query(db,"SELECT e.* FROM workspace_executions e JOIN workspace_tasks t USING(task_id) WHERE t.project_id=? AND t.task_id=?",&[json!(project),json!(task)])?;
    let Some(e) = rows.first() else {
        return Ok(json!([]));
    };
    let states = query(
        db,
        "SELECT state FROM runtime_tasks WHERE task_id=?",
        &[e["runtime_task_id"].clone()],
    )?;
    let state = states
        .first()
        .and_then(|v| v["state"].as_str())
        .unwrap_or("");
    let status = match state {
        "COMPLETED" => "completed",
        "FAILED" => "failed",
        "CANCELLED" => "cancelled",
        "WAITING_USER" => "waiting_you",
        "BLOCKED" | "PAUSED" => "pending",
        "CREATED" | "PLANNING" | "PLAN_READY" | "EXECUTING" | "REVIEWING" | "VERIFYING"
        | "RECONCILING" | "ANSWER_READY" | "RECOVERING" => "working",
        _ => "unknown",
    };
    let mut item = json!({"id":params["workerId"],"taskId":task,"executionId":e["execution_id"],"role":"coordinator","lane":"execution",
        "status":status,"goal":"Coordinator for this task","now":null,"waitsFor":null,"scope":[],"route":route(db,&e["runtime_task_id"])?,
        "attempt":null,"elapsedMs":null,"runningSince":null,"context":null,"usage":null,"result":null,"messages":[]});
    item.as_object_mut().unwrap().extend(
        page(db, project, task, &e["acp_session_id"], &params["cursor"], &e["runtime_task_id"])?
            .as_object()
            .unwrap()
            .clone(),
    );
    let found = json!([item]);
    if bytes(&json!({"ok":true,"result":found})) > 786432 {
        return Err(CoreError::Refused("worker_snapshot_too_large"));
    }
    Ok(found)
}
