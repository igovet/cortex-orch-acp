//! Owner decisions for native engine permission requests of a Workspace task.
//!
//! A request the access mode's decision table sends to the owner, when no
//! incoming ACP client can answer it, is kept as one `pending_questions` row
//! (`dedup_key` prefixed [`DEDUP_PREFIX`], options Allow once / Deny) and a
//! `NATIVE_PERMISSION_DECISION_OPENED` event with its details (role, operation,
//! paths, reason, access mode). The engine's request waits while the row is
//! open; the owner's answer (`answer_tx`) or a withdrawal (`withdraw_tx`) closes
//! it. No store format change: the existing row and event columns carry it all.
use crate::{lifecycle, CoreError, CoreResult};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

pub const DEDUP_PREFIX: &str = "native_permission:";
pub const ALLOW: &str = "Allow once";
pub const DENY: &str = "Deny";
pub const ALLOWED: &str = "Allowed once";
pub const DENIED: &str = "Denied";
const SOURCE: &str = "owner_permission";

/// Whether the task is a native Workspace task (bound to a desktop execution).
pub fn workspace_task_tx(db: &Connection, task: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM workspace_executions WHERE runtime_task_id=?)",
        [task],
        |r| r.get(0),
    )?)
}
/// Opens one decision; returns its question id.
pub fn open_tx(
    db: &Connection,
    task: &str,
    assignment: Option<&str>,
    channel: &str,
    request: &str,
    prompt: &str,
    details: &Value,
) -> CoreResult<String> {
    let now = crate::now();
    let id = format!(
        "np_{}",
        &crate::digest(format!("{task}:{channel}:{request}:{now}").as_bytes())[..32]
    );
    db.execute(
        "INSERT INTO pending_questions(id,task_id,assignment_ref,dedup_key,text,options_json,status,created_at)
         VALUES (?,?,?,?,?,?,'open',?)",
        (
            &id,
            task,
            assignment,
            format!("{DEDUP_PREFIX}{channel}:{request}:{id}"),
            prompt,
            json!([ALLOW, DENY]).to_string(),
            &now,
        ),
    )?;
    let mut payload = details.clone();
    payload["question_id"] = json!(id);
    payload["channel_id"] = json!(channel);
    lifecycle::event_tx(db, task, "NATIVE_PERMISSION_DECISION_OPENED", payload)?;
    Ok(id)
}
/// The row's status and answer text.
pub fn state_tx(db: &Connection, id: &str) -> CoreResult<Option<(String, Option<String>)>> {
    Ok(db
        .query_row(
            "SELECT status,answer_text FROM pending_questions WHERE id=? AND dedup_key LIKE 'native_permission:%'",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?)
}
/// Withdraws an open decision with a stated reason; false when it was not open.
pub fn withdraw_tx(db: &Connection, task: &str, id: &str, reason: &str) -> CoreResult<bool> {
    let changed = db.execute(
        "UPDATE pending_questions SET status='withdrawn',answered_at=? WHERE id=? AND task_id=? AND status='open'
         AND dedup_key LIKE 'native_permission:%'",
        (crate::now(), id, task),
    )?;
    if changed == 1 {
        lifecycle::event_tx(
            db,
            task,
            "NATIVE_PERMISSION_DECISION_WITHDRAWN",
            json!({"question_id":id,"reason":reason}),
        )?;
    }
    Ok(changed == 1)
}
/// Withdraws every open decision (a service start: no engine request survived).
pub fn withdraw_all_open_tx(db: &Connection, reason: &str) -> CoreResult<usize> {
    let open = db
        .prepare(
            "SELECT id,task_id FROM pending_questions WHERE status='open' AND dedup_key LIKE 'native_permission:%'",
        )?
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut n = 0;
    for (id, task) in open {
        if withdraw_tx(db, &task, &id, reason)? {
            n += 1;
        }
    }
    Ok(n)
}
/// Records the owner's answer; false when the decision is no longer open.
pub fn answer_tx(db: &Connection, task: &str, id: &str, allow: bool) -> CoreResult<bool> {
    let text = if allow { ALLOWED } else { DENIED };
    let changed = db.execute(
        "UPDATE pending_questions SET status='answered',answer_text=?,answer_source=?,answered_at=?
         WHERE id=? AND task_id=? AND status='open' AND dedup_key LIKE 'native_permission:%'",
        (text, SOURCE, crate::now(), id, task),
    )?;
    if changed == 1 {
        lifecycle::event_tx(
            db,
            task,
            "NATIVE_PERMISSION_DECISION_ANSWERED",
            json!({"question_id":id,"allowed":allow}),
        )?;
    }
    Ok(changed == 1)
}
/// The decision's details (its opening event) and, for a withdrawn one, why.
pub fn details_tx(db: &Connection, task: &str, id: &str) -> CoreResult<(Value, Option<String>)> {
    let opened: Option<String> = db
        .query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_PERMISSION_DECISION_OPENED'
             AND json_extract(payload_json,'$.question_id')=? ORDER BY id DESC LIMIT 1",
            (task, id),
            |r| r.get(0),
        )
        .optional()?;
    let reason: Option<String> = db
        .query_row(
            "SELECT json_extract(payload_json,'$.reason') FROM runtime_events WHERE task_id=?
             AND type='NATIVE_PERMISSION_DECISION_WITHDRAWN' AND json_extract(payload_json,'$.question_id')=?
             ORDER BY id DESC LIMIT 1",
            (task, id),
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let details = opened
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(CoreError::from)?
        .unwrap_or(Value::Null);
    Ok((details, reason))
}
/// The owner-facing sentence for a withdrawal reason.
pub fn withdrawn_sentence(reason: Option<&str>, status: &str) -> &'static str {
    match reason {
        Some("service_restarted") => {
            "The service restarted before you answered; the engine's request no longer exists."
        }
        Some("engine_request_ended") => "The worker's engine stopped waiting before you answered.",
        Some("task_paused") => "The task was paused before you answered.",
        Some("task_stopped") => "The task was stopped before you answered.",
        Some(_) => "The worker's session ended before you answered.",
        None if status == "superseded" => "The task asked something else before you answered.",
        None => "The task was stopped before you answered.",
    }
}
/// One receipt of an owner-decided native permission request.
pub fn receipt_tx(
    db: &Connection,
    task: &str,
    attempt: &str,
    tool_call_ref: &str,
    subject: &str,
    outcome: &str,
) -> CoreResult<()> {
    let subject: String = subject.chars().take(2000).collect();
    db.execute(
        "INSERT OR IGNORE INTO permission_receipts(task_id,attempt_id,tool_call_ref,command_sha256,command_text,host_outcome,created_at)
         VALUES (?,?,?,?,?,?,?)",
        (
            task,
            attempt,
            tool_call_ref,
            crate::digest(subject.as_bytes()),
            &subject,
            outcome,
            crate::now(),
        ),
    )?;
    Ok(())
}
