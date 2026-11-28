//! Audits the exact successful skill body selected for return by the gateway.
use cortex_core::{
    channels::{self, Credential, Identity},
    lifecycle, CoreError, CoreResult,
};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};

const OP: &str = "native_skill_delivery";

pub(super) enum Delivery {
    Inline(Value),
    Deferred { skill: Value, output: Value },
    Readback(Value),
}

pub(super) fn body_metadata(reply: &Value) -> Option<Value> {
    let body = reply["body"].as_str()?;
    let (id, source) = if reply["source"] == "project" {
        // Do not disclose project skill names or workspace paths in the journal.
        let canonical = reply["path"].as_str()?;
        (
            format!(
                "project-sha256:{}",
                crate::native_project::digest(canonical.as_bytes())
            ),
            "project",
        )
    } else {
        let id = reply["skill"].as_str()?;
        let source = if crate::native_instructions::is_profile(id) {
            "bundled_profile"
        } else {
            "bundled"
        };
        (id.to_owned(), source)
    };
    let sha256 = crate::native_project::digest(body.as_bytes());
    Some(
        json!({"skill_id":id,"source":source,"version":format!("sha256:{sha256}"),
        "sha256":sha256,"bytes":body.len()}),
    )
}

pub(super) fn output_page(reply: &Value) -> Option<Delivery> {
    (reply["large_output_id"].as_str()?.starts_with("lo_s_") && reply["integrity_verified"] == true)
        .then(|| Delivery::Readback(json!({
            "large_output_id":reply["large_output_id"],"offset":reply["offset"],
            "byte_count":reply["byte_count"],"integrity_verified":true,"truncated":reply["truncated"],
            "source_kind":reply["source_kind"],"next_offset":reply["next_offset"]
        })))
}

fn append(
    db: &Transaction<'_>,
    c: &Credential,
    i: &Identity,
    key: &str,
    event: &str,
    receipt: &Value,
) -> CoreResult<()> {
    let digest = crate::native_project::digest(&serde_json::to_vec(receipt)?);
    let n = db.execute(
        "INSERT INTO runtime_requests(task_id,request_key,actor,operation,digest,result_json)
         VALUES (?,?,?,?,?,?) ON CONFLICT(task_id,actor,request_key) DO NOTHING",
        (
            &i.task,
            key,
            &c.channel_id,
            OP,
            &digest,
            receipt.to_string(),
        ),
    )?;
    if n == 0 {
        let valid: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_requests
             WHERE task_id=? AND request_key=? AND actor=? AND operation=? AND digest=?)",
            (&i.task, key, &c.channel_id, OP, &digest),
            |r| r.get(0),
        )?;
        if !valid {
            return Err(CoreError::Refused("skill_delivery_receipt_collision"));
        }
        return Ok(());
    }
    lifecycle::event_tx(db, &i.task, event, receipt.clone())?;
    Ok(())
}

fn scoped(c: &Credential, i: &Identity, mut skill: Value, delivery: &str) -> Value {
    skill["task_id"] = json!(i.task);
    skill["channel_id"] = json!(c.channel_id);
    skill["assignment_ref"] = json!(i.assignment);
    skill["attempt"] = json!(i.attempt);
    skill["session"] = json!(i.session);
    skill["delivery"] = json!(delivery);
    skill["evidence"] = json!("gateway_response");
    skill
}

pub(super) fn record(
    db: &Transaction<'_>,
    c: &Credential,
    expected: &Identity,
    actions: Vec<Delivery>,
) -> CoreResult<()> {
    let i = channels::authenticate(db, &c.channel_id, c.secret(), true)?;
    if &i != expected {
        return Err(CoreError::Refused("channel_identity_changed"));
    }
    for action in actions {
        match action {
            Delivery::Inline(skill) => {
                let receipt = scoped(c, &i, skill, "inline");
                let key = format!(
                    "native-skill:inline:{}",
                    crate::native_project::digest(&serde_json::to_vec(&receipt)?)
                );
                append(db, c, &i, &key, "NATIVE_SKILL_DELIVERED", &receipt)?;
            }
            Delivery::Deferred { skill, output } => {
                let output_id = output["large_output_id"]
                    .as_str()
                    .ok_or(CoreError::Refused("skill_delivery_invalid_output"))?;
                let mut receipt = scoped(c, &i, skill, "deferred");
                receipt["large_output_id"] = json!(output_id);
                receipt["output_sha256"] = output["result_sha256"].clone();
                receipt["output_bytes"] = output["byte_count"].clone();
                append(
                    db,
                    c,
                    &i,
                    &format!("native-skill:offer:{output_id}"),
                    "NATIVE_SKILL_DEFERRED",
                    &receipt,
                )?;
            }
            Delivery::Readback(output) => readback(db, c, &i, &output)?,
        }
    }
    Ok(())
}

fn readback(db: &Transaction<'_>, c: &Credential, i: &Identity, page: &Value) -> CoreResult<()> {
    let (Some(id), Some(offset), Some(total)) = (
        page["large_output_id"].as_str(),
        page["offset"].as_u64(),
        page["byte_count"].as_u64(),
    ) else {
        return Ok(());
    };
    if page["integrity_verified"] != true
        || page["truncated"] != false
        || page["source_kind"] != "tool_result"
        || offset >= total
    {
        return Ok(());
    }
    let offer: Option<(String, String)> = db
        .query_row(
            "SELECT r.result_json,l.source_ref FROM runtime_requests r JOIN large_outputs l
           ON l.task_id=r.task_id AND l.id=? AND l.owner_scope=r.actor
           WHERE r.task_id=? AND r.actor=? AND r.operation=? AND r.request_key=?",
            (
                id,
                &i.task,
                &c.channel_id,
                OP,
                format!("native-skill:offer:{id}"),
            ),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((offer, hash)) = offer else {
        return Ok(());
    };
    let mut receipt: Value = serde_json::from_str(&offer)?;
    if receipt["attempt"] != i.attempt
        || receipt["assignment_ref"] != json!(i.assignment)
        || receipt["session"] != json!(i.session)
        || receipt["output_sha256"] != hash
        || receipt["output_bytes"].as_u64() != Some(total)
    {
        return Err(CoreError::Refused("skill_delivery_invalid_output"));
    }
    let progress_key = format!("native-skill:progress:{id}");
    let stored: Option<(String, String, i64)> = db.query_row(
        "SELECT operation,digest,json_extract(result_json,'$.next_offset') FROM runtime_requests
           WHERE task_id=? AND actor=? AND request_key=?",
        (&i.task, &c.channel_id, &progress_key),
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional()?;
    let next = match stored {
        Some((op, digest, next)) if op == OP && digest == hash && next >= 0 => next as u64,
        Some(_) => return Err(CoreError::Refused("skill_delivery_receipt_collision")),
        None => 0,
    };
    let end = page["next_offset"].as_u64().unwrap_or(total);
    if offset != next || end <= offset || end > total {
        return Ok(());
    }
    db.execute(
        "INSERT INTO runtime_requests(task_id,request_key,actor,operation,digest,result_json)
         VALUES (?,?,?,?,?,?) ON CONFLICT(task_id,actor,request_key) DO UPDATE SET result_json=excluded.result_json",
        (&i.task, &progress_key, &c.channel_id, OP, &hash, json!({"next_offset":end}).to_string()),
    )?;
    if end == total {
        receipt["delivery"] = json!("deferred_readback");
        append(
            db,
            c,
            i,
            &format!("native-skill:readback:{id}"),
            "NATIVE_SKILL_DELIVERED",
            &receipt,
        )?;
    }
    Ok(())
}
