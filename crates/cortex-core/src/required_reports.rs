//! Required immutable reading is scoped to the current node and worker attempt.
use crate::{error::refuse, CoreError, CoreResult};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub(crate) fn validate(node: &Value) -> CoreResult<()> {
    let Some(required) = node.get("required_reports") else {
        return Ok(());
    };
    let entries = required
        .as_array()
        .filter(|v| !v.is_empty() && v.len() <= 8)
        .ok_or(CoreError::Refused("invalid_required_reports"))?;
    let mut seen = BTreeSet::new();
    for entry in entries {
        let fields = entry
            .as_object()
            .ok_or(CoreError::Refused("invalid_required_reports"))?;
        let valid = if let Some(source) = entry["from_node"].as_str() {
            fields
                .keys()
                .all(|k| matches!(k.as_str(), "from_node" | "section"))
                && node["dependencies"]
                    .as_array()
                    .is_some_and(|deps| deps.iter().any(|v| v == source))
        } else {
            fields
                .keys()
                .all(|k| matches!(k.as_str(), "report_id" | "sha256" | "section"))
                && entry["report_id"]
                    .as_str()
                    .and_then(|id| id.strip_prefix("r_"))
                    .is_some_and(|id| hex(id, 12))
                && entry["sha256"].as_str().is_some_and(|s| hex(s, 64))
        };
        if !valid || !seen.insert(entry.to_string()) {
            return refuse("invalid_required_reports");
        }
        if let Some(section) = entry.get("section") {
            let valid = section.as_object().is_some_and(|fields| fields.len() == 2)
                && section["start"]
                    .as_u64()
                    .zip(section["end"].as_u64())
                    .is_some_and(|(start, end)| start < end && end <= 1_000_000_000);
            if !valid {
                return refuse("invalid_required_reports");
            }
        }
    }
    Ok(())
}

fn hex(value: &str, len: usize) -> bool {
    value.len() == len
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn exact_edition(db: &Connection, task: &str, id: &str, sha: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT count(*)=1 AND min(e.digest)=?3 FROM reports r JOIN editions e ON e.report_id=r.id
         WHERE r.task_id=?1 AND r.id=?2 AND r.kind='report'",
        (task, id, sha),
        |r| r.get(0),
    )?)
}

pub(crate) fn validate_existing(db: &Connection, task: &str, plan: &Value) -> CoreResult<()> {
    for node in plan["tasks"].as_array().into_iter().flatten() {
        for entry in node["required_reports"].as_array().into_iter().flatten() {
            if let (Some(id), Some(sha)) = (entry["report_id"].as_str(), entry["sha256"].as_str()) {
                if !exact_edition(db, task, id, sha)? {
                    return refuse("required_report_unavailable");
                }
            }
        }
    }
    Ok(())
}

pub fn selected(db: &Connection, task: &str, node: &str) -> CoreResult<Vec<Value>> {
    let (revision, raw): (i64, Option<String>) = db.query_row(
        "SELECT revision,plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let plan: Value = serde_json::from_str(&raw)?;
    let Some(entries) = plan["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|item| item["id"] == node)
        .and_then(|item| item["required_reports"].as_array())
    else {
        return Ok(Vec::new());
    };
    let mut result = Vec::with_capacity(entries.len());
    for entry in entries {
        let mut bound = entry.clone();
        let mut after = 0;
        if let Some(source) = entry["from_node"].as_str() {
            let identity: Option<(String, String, i64)> = db.query_row(
                "SELECT json_extract(n.result_json,'$.report_ref'),json_extract(n.result_json,'$.report_digest'),min(e.id)
                 FROM runtime_nodes n JOIN delegation_provisioning p ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
                 JOIN runtime_events e ON e.task_id=n.task_id AND e.type='WORKER_COMPLETED' AND e.actor='runtime'
                   AND json_extract(e.payload_json,'$.assignment_ref')=n.assignment_ref
                   AND json_extract(e.payload_json,'$.attempt_id')=p.attempt_id
                   AND json_extract(e.payload_json,'$.result.report_ref')=json_extract(n.result_json,'$.report_ref')
                   AND json_extract(e.payload_json,'$.result.report_digest')=json_extract(n.result_json,'$.report_digest')
                 WHERE n.task_id=? AND n.revision=? AND n.node_id=? AND n.stage='execution' AND n.status='completed'
                 HAVING count(*)=1",
                (task, revision, source), |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
            ).optional()?;
            if let Some((id, sha, event)) = identity {
                bound["report_id"] = json!(id);
                bound["sha256"] = json!(sha);
                after = event;
            }
        }
        let available = match (bound["report_id"].as_str(), bound["sha256"].as_str()) {
            (Some(id), Some(sha)) => exact_edition(db, task, id, sha)?,
            _ => false,
        };
        bound["binding_event"] = json!(after);
        bound["available"] = json!(available);
        result.push(bound);
    }
    Ok(result)
}

/// Same assignment, channel and attempt must cover the exact edition.
pub fn unread(
    db: &Connection,
    task: &str,
    assignment: &str,
    channel: &str,
    attempt: &str,
) -> CoreResult<Vec<Value>> {
    let mut missing = Vec::new();
    for required in for_assignment(db, task, assignment)? {
        if required["available"] != true
            || !read(db, task, assignment, channel, attempt, &required)?
        {
            missing.push(required);
        }
    }
    Ok(missing)
}

pub fn for_assignment(db: &Connection, task: &str, assignment: &str) -> CoreResult<Vec<Value>> {
    let node: Option<String> = db.query_row(
        "SELECT n.node_id FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
         WHERE n.task_id=? AND n.revision=t.revision AND n.assignment_ref=? AND n.stage='execution'",
        (task, assignment), |r| r.get(0),
    ).optional()?;
    node.map(|node| selected(db, task, &node))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn read(
    db: &Connection,
    task: &str,
    assignment: &str,
    channel: &str,
    attempt: &str,
    required: &Value,
) -> CoreResult<bool> {
    let mut statement = db.prepare(
        "SELECT payload_json FROM runtime_events WHERE task_id=?1 AND type='REPORT_PAGE_READ' AND actor='runtime' AND id>?2
         AND json_extract(payload_json,'$.assignment_ref')=?3 AND json_extract(payload_json,'$.channel_id')=?4
         AND json_extract(payload_json,'$.attempt')=?5 AND json_extract(payload_json,'$.report_id')=?6
         AND json_extract(payload_json,'$.sha256')=?7
         AND json_extract(payload_json,'$.required_inline')=1 ORDER BY id LIMIT 10001",
    )?;
    let mut rows = statement.query((
        task,
        required["binding_event"].as_i64(),
        assignment,
        channel,
        attempt,
        required["report_id"].as_str(),
        required["sha256"].as_str(),
    ))?;
    let section = required["section"]["start"]
        .as_u64()
        .zip(required["section"]["end"].as_u64());
    let mut ranges = Vec::new();
    let mut cursors = BTreeSet::new();
    let mut count = 0;
    while let Some(row) = rows.next()? {
        count += 1;
        if count > 10000 {
            return refuse("required_report_read_limit");
        }
        let raw: String = row.get(0)?;
        let page: Value = serde_json::from_str(&raw)?;
        let Some((start, end)) = page["start_character"]
            .as_u64()
            .zip(page["end_character"].as_u64())
        else {
            continue;
        };
        let Some(total) = page["total_characters"].as_u64() else {
            continue;
        };
        if start >= end || end > total {
            continue;
        }
        if let Some((_, required_end)) = section {
            if required_end <= total {
                ranges.push((start, end));
            }
        } else {
            let linked = match page["cursor"].as_str() {
                Some(cursor) => cursors.contains(&(cursor.to_owned(), start, total)),
                None => start == 0,
            };
            if linked {
                if let Some(next) = page["next_cursor"].as_str() {
                    cursors.insert((next.to_owned(), end, total));
                } else if end == total {
                    return Ok(true);
                }
            }
        }
    }
    if let Some((mut covered, required_end)) = section {
        ranges.sort_unstable();
        for (start, end) in ranges {
            if start > covered {
                break;
            }
            covered = covered.max(end);
            if covered >= required_end {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
