//! `sprint_plan_submit`: the coordinator of a read-only sprint planning task
//! submits its plan through this dispatched, channel-verified tool, in one call or
//! in ordered parts. The argument contract lives in the advertised schema; the
//! runtime assembles the parts and validates the whole plan with the same rules
//! the sprint applies, and a refusal names the failing task and rule so the
//! coordinator can correct it in the same turn.
use crate::native_driver::sprints::{check_tasks, PlanError, MAX_PART_BYTES};
use cortex_core::{
    channels::{self, Credential},
    lifecycle::sprint_planning,
    store::executor::StoreExecutor,
    CoreError, CoreResult,
};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::sync::Arc;

pub(super) fn supports(name: &str) -> bool {
    name == "sprint_plan_submit"
}

async fn recv<T>(r: tokio::sync::oneshot::Receiver<CoreResult<T>>) -> CoreResult<T> {
    r.await
        .map_err(|_| CoreError::Refused("native_host_closed"))?
}

fn reply(value: Value, error: bool) -> Value {
    json!({"content":[{"type":"text","text":value.to_string()}],
        "structuredContent":value,"isError":error})
}
fn refused(error: PlanError) -> Value {
    reply(
        json!({"recorded":false,"code":error.code,"detail":error.detail,
            "hint":"Correct the named task and rule, then call sprint_plan_submit again with a new request_key."}),
        true,
    )
}

/// The MCP reply: a receipt, or a correctable refusal naming the task and rule.
pub(super) async fn execute(
    executor: Arc<StoreExecutor>,
    credential: Credential,
    arguments: Value,
) -> CoreResult<Value> {
    let c = credential;
    recv(executor.write(move |db| {
        let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
        if i.role != "coordinator" {
            return Err(CoreError::Refused("tool_refused"));
        }
        let tools = cortex_engine::schema::catalogue(&i.role, None, None)
            .map_err(|_| CoreError::Refused("tool_refused"))?;
        let tool = tools
            .iter()
            .find(|t| t["name"] == "sprint_plan_submit")
            .ok_or(CoreError::Refused("tool_refused"))?;
        cortex_engine::schema::validate(&tool["inputSchema"], &arguments)
            .map_err(|_| CoreError::Refused("invalid_arguments"))?;
        submit_in_tx(db, &i.task, &c.channel_id, &arguments)
    })?)
    .await
}

/// One authenticated call, after its schema check: a part is checked with every
/// part before it (one 1-based numbering over the whole plan), and the final part
/// checks and records the whole assembled plan.
pub(crate) fn submit_in_tx(
    db: &Transaction<'_>,
    task: &str,
    actor: &str,
    arguments: &Value,
) -> CoreResult<Value> {
    if !sprint_planning::enabled(db, task)? {
        return Err(CoreError::Refused("sprint_plan_not_expected"));
    }
    let key = arguments["request_key"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_request_key"))?;
    let part = arguments["part"].as_i64().unwrap_or(1);
    let last = arguments["final"].as_bool().unwrap_or(true);
    let tasks = &arguments["tasks"];
    // An exact retry replays its receipt whatever the draft became meanwhile.
    let replay: bool = db
        .query_row(
            "SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?",
            (task, actor, key),
            |r| r.get::<_, i64>(0),
        )
        .optional()?
        .is_some();
    if !replay {
        if tasks.to_string().len() > MAX_PART_BYTES {
            return Ok(refused(PlanError {
                code: "plan_part_too_large",
                detail: format!(
                    "this part exceeds {MAX_PART_BYTES} UTF-8 bytes; send fewer tasks per part"
                ),
            }));
        }
        let (next, so_far) = sprint_planning::draft(db, task)?;
        if part != 1 && part != next {
            return Ok(refused(PlanError {
                code: "sprint_plan_part_out_of_order",
                detail: format!("part {part} does not follow the plan being assembled; send part {next}, or part 1 to start over"),
            }));
        }
        let mut assembled = if part == 1 { Vec::new() } else { so_far };
        assembled.extend(tasks.as_array().cloned().unwrap_or_default());
        let checked = match check_tasks(&json!(assembled), last) {
            Ok(items) => items,
            Err(error) => return Ok(refused(error)),
        };
        if last {
            let value =
                sprint_planning::submit_tx(db, task, actor, key, arguments, &json!(checked))?;
            return Ok(reply(value, false));
        }
        let part_items = json!(checked[checked.len() - tasks.as_array().map_or(0, Vec::len)..]);
        let value =
            sprint_planning::record_part_tx(db, task, actor, key, arguments, part, &part_items)?;
        return Ok(reply(value, false));
    }
    let value = if last {
        sprint_planning::submit_tx(db, task, actor, key, arguments, &json!([]))?
    } else {
        sprint_planning::record_part_tx(db, task, actor, key, arguments, part, &json!([]))?
    };
    Ok(reply(value, false))
}
