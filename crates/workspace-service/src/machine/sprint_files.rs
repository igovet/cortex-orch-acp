//! The files the owner gave a sprint reach every task created from it.
//!
//! Planning reads the sprint's own attachments and those of its planning
//! conversation (answers and messages); a task created from the plan gets the same
//! files as its own draft attachments, so its coordinator and workers read them as
//! sources (images stay images). A draft that was created without them is brought
//! up to date, and a task that already runs is told by one queued boundary message
//! carrying the files, delivered at its next turn.
use super::{
    attachment_files::{inline_for_delivery, is_reference},
    context,
    native::*,
    settings,
};
use cortex_core::Store;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::path::Path;

/// The sprint's files as saved references: its own, its conversation's and the
/// planning task's answer files, each once.
pub(crate) fn references(product: &Value, sprint: &Value) -> Vec<Value> {
    let mut out: Vec<Value> = Vec::new();
    let planner = sprint["planningTaskId"].as_str().unwrap_or("");
    let lists = std::iter::once(&sprint["attachments"])
        .chain(sprint["conversation"].as_array().into_iter().flatten().map(|c| &c["attachments"]))
        .chain(std::iter::once(&product["answerFiles"][planner]["attachments"]));
    for list in lists {
        for item in list.as_array().into_iter().flatten() {
            let same = |o: &Value| o["name"] == item["name"] && o["sha256"] == item["sha256"] && o["data"] == item["data"];
            if item["name"].is_string() && !out.iter().any(same) {
                out.push(item.clone());
            }
        }
    }
    out
}

/// The same files with their bodies, in the draft attachment wire form. A file whose
/// body is gone is left out.
pub(crate) fn wire(state: &Path, refs: &[Value]) -> Vec<Value> {
    let mut holder = json!({"attachments":refs});
    inline_for_delivery(state, &mut holder);
    holder["attachments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| {
            Some(json!({"name":a["name"].as_str()?,"mimeType":a["mimeType"].as_str()?,"data":a["data"].as_str()?}))
        })
        .collect()
}

fn has(files: &[Value], f: &Value) -> bool {
    files.iter().any(|o| o["name"] == f["name"] && o["data"] == f["data"])
}

/// Bring every member of `sprint_id` up to date. Called with machine.lock held;
/// answers true when the saved product changed (a message was queued).
pub(crate) fn backfill(
    store: &Store,
    state: &Path,
    project: &str,
    product: &mut Value,
    sprint_id: &str,
) -> Result<bool> {
    let Some(sprint) = product["sprints"].as_array().and_then(|s| s.iter().find(|s| s["id"] == sprint_id)).cloned() else {
        return Ok(false);
    };
    let refs = references(product, &sprint);
    if refs.is_empty() || sprint["materializedVersion"].is_null() {
        return Ok(false);
    }
    let files = wire(state, &refs);
    if files.is_empty() {
        return Ok(false);
    }
    let members: Vec<String> = sprint["taskIds"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.as_str().map(str::to_owned))
        .collect();
    let running = store
        .write(|db| {
            let mut running = Vec::new();
            for task in &members {
                let row: Option<(String, Option<String>, Option<String>)> = db
                    .query_row(
                        "SELECT o.options_json,w.execution_id,r.state FROM workspace_tasks w
                         JOIN workspace_task_options o USING(task_id)
                         LEFT JOIN workspace_executions e ON e.execution_id=w.execution_id
                         LEFT JOIN runtime_tasks r ON r.task_id=e.runtime_task_id
                         WHERE w.task_id=? AND w.project_id=?",
                        (task, project),
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .optional()?;
                let Some((encoded, execution, state)) = row else { continue };
                let mut options: Value = serde_json::from_str(&encoded)?;
                let mut have: Vec<Value> = options["attachments"].as_array().cloned().unwrap_or_default();
                let missing: Vec<&Value> = files.iter().filter(|f| !has(&have, f)).collect();
                if missing.is_empty() {
                    continue;
                }
                if execution.is_none() {
                    have.extend(missing.into_iter().cloned());
                    if cortex_core::input::capture_from_wire(&json!(have)).is_err() {
                        continue;
                    }
                    options["attachments"] = json!(have);
                    db.execute(
                        "UPDATE workspace_task_options SET options_json=? WHERE task_id=?",
                        (serde_json::to_string(&options)?, task),
                    )?;
                } else if !matches!(state.as_deref(), Some("COMPLETED" | "FAILED" | "CANCELLED") | None) {
                    running.push(task.clone());
                }
            }
            Ok(running)
        })
        .map_err(context::core)?;
    let mut changed = false;
    let digest = {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        for f in &files {
            h.update(f["name"].as_str().unwrap_or("").as_bytes());
            h.update(f["data"].as_str().unwrap_or("").as_bytes());
        }
        h.finalize().iter().take(6).map(|b| format!("{b:02x}")).collect::<String>()
    };
    let queued: Vec<Value> = refs.iter().filter(|r| is_reference(r)).cloned().collect();
    if queued.is_empty() {
        return Ok(false);
    }
    for task in running {
        let id = format!("sprintfiles-{task}-{digest}");
        let rows = product["messages"].as_array_mut();
        let Some(rows) = rows else { continue };
        if rows.iter().any(|m| m["id"] == id) {
            continue;
        }
        rows.push(json!({"id":id,"taskId":task,"workerId":null,
            "text":"Sprint files: the owner attached these files to the sprint (its description or planning conversation). They are part of this task's sources; read them before continuing.",
            "mode":"queue","delivery":"queued","at":std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64),"deliveredAt":null,"failure":null,
            "attachments":queued}));
        changed = true;
    }
    if changed {
        settings::write_json(&product_path(state, project)?, product)?;
    }
    Ok(changed)
}
