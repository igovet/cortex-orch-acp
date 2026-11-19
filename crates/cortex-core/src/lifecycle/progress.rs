//! Persisted dispatch-progress budget of a native driver.
use crate::{CoreError, CoreResult};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};

/// Dispatch steps one driver may spend between owner actions.
pub const PROGRESS_BUDGET: i64 = 128;
/// Spend one step. The count survives a service restart. It starts over only on an
/// owner action (steering, answer, interrupt or continue), after a driver stop, or on
/// reopen. Exhaustion refuses with `native_progress_limit` and changes nothing.
pub fn spend_tx(db: &Transaction<'_>, task: &str, budget: i64) -> CoreResult<()> {
    let epoch: i64 = db.query_row(
        "SELECT coalesce(max(id),0) FROM runtime_events WHERE task_id=? AND type IN
        ('USER_STEERING','USER_ANSWER_RECEIVED','WORKER_USER_ANSWER_RECEIVED','TURN_INTERRUPTED',
         'NATIVE_DRIVER_STOPPED','TASK_REOPENED','AUTOMATIC_RESUMED')",
        [task],
        |r| r.get(0),
    )?;
    let saved: Option<String> = db
        .query_row(
            "SELECT result_json FROM runtime_requests WHERE task_id=? AND actor='native-progress' AND request_key='budget'",
            [task],
            |r| r.get(0),
        )
        .optional()?;
    let used = saved
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .filter(|v| v["epoch"] == epoch)
        .and_then(|v| v["used"].as_i64())
        .unwrap_or(0);
    if used >= budget {
        return Err(CoreError::Refused("native_progress_limit"));
    }
    db.execute(
        "INSERT INTO runtime_requests VALUES (?,'budget','native-progress','native_progress_budget','-',?)
         ON CONFLICT(task_id,actor,request_key) DO UPDATE SET result_json=excluded.result_json",
        (task, json!({"epoch":epoch,"used":used + 1}).to_string()),
    )?;
    Ok(())
}
