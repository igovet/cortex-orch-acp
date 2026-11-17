//! Durable external-call identity. Never hold a transaction across process IO.
use crate::{
    channels::{self, Credential},
    lifecycle::event_tx,
    CoreError, CoreResult,
};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};

pub enum Reservation {
    Dispatch,
    Replay(Value),
    Unknown,
}
fn key(server: &str, id: &Value) -> CoreResult<String> {
    if server.is_empty() || server == "cortex" || !(id.is_string() || id.as_i64().is_some()) {
        return Err(CoreError::Refused("external_mcp_identity_invalid"));
    }
    Ok(format!("external:{}", json!([server, id])))
}
pub fn reserve_tx(
    db: &Transaction<'_>,
    c: &Credential,
    server: &str,
    id: &Value,
    digest: &str,
    tool: &str,
    read_only: bool,
) -> CoreResult<Reservation> {
    let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
    let key = key(server, id)?;
    let old: Option<(String,String,String)> = db.query_row(
        "SELECT operation,digest,result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?",
        (&i.task,&c.channel_id,&key),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    if let Some((op, d, saved)) = old {
        if op != "external_call" || d != digest {
            return Err(CoreError::Refused("external_mcp_request_identity_conflict"));
        }
        let saved: Value = serde_json::from_str(&saved)?;
        return Ok(if saved["complete"] == true {
            Reservation::Replay(saved["receipt"].clone())
        } else {
            Reservation::Unknown
        });
    }
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) || tool.is_empty() {
        return Err(CoreError::Refused("external_mcp_identity_invalid"));
    }
    db.execute(
        "INSERT INTO runtime_requests VALUES (?,?,?,?,?,?)",
        (
            &i.task,
            &key,
            &c.channel_id,
            "external_call",
            digest,
            json!({"complete":false,"read_only":read_only}).to_string(),
        ),
    )?;
    event_tx(
        db,
        &i.task,
        "EXTERNAL_TOOL_AUTHORIZED",
        json!({"channel_id":c.channel_id,"server":server,"tool":tool,"request_id":id,"digest":digest}),
    )?;
    Ok(Reservation::Dispatch)
}
pub fn complete_tx(
    db: &Transaction<'_>,
    c: &Credential,
    server: &str,
    id: &Value,
    reply: &Value,
) -> CoreResult<()> {
    let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
    if reply.get("id") != Some(id)
        || reply["jsonrpc"] != "2.0"
        || reply.get("result").is_some() == reply.get("error").is_some()
    {
        return Err(CoreError::Refused(
            "external_mcp_response_identity_mismatch",
        ));
    }
    let key = key(server, id)?;
    let saved:Option<String>=db.query_row("SELECT result_json FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=? AND operation='external_call'",
        (&i.task,&c.channel_id,&key),|r|r.get(0)).optional()?;
    let saved: Value = serde_json::from_str(
        &saved.ok_or(CoreError::Refused("external_mcp_reservation_missing"))?,
    )?;
    if saved["complete"] == true {
        return Err(CoreError::Refused("external_mcp_already_complete"));
    }
    db.execute(
        "UPDATE runtime_requests SET result_json=? WHERE task_id=? AND actor=? AND request_key=?",
        (
            json!({"complete":true,"receipt":reply}).to_string(),
            &i.task,
            &c.channel_id,
            &key,
        ),
    )?;
    event_tx(
        db,
        &i.task,
        "EXTERNAL_TOOL_RESULT",
        json!({"channel_id":c.channel_id,"server":server,"request_id":id}),
    )?;
    Ok(())
}
