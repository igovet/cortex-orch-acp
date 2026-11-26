//! Runtime-kept verification commands follow the current plan. A command that
//! pins a file to a literal sha256 is a snapshot of that file at one moment. When
//! a later accepted change legitimately altered the file, the snapshot is stale:
//! enforcing it contradicts the approved state, so the runtime retires it
//! (`VERIFICATION_REVISED` with `superseded`) instead of asking the owner.
//!
//! Retired: (a) of two commands pinning the same path to different hashes, the
//! older one (no content satisfies both); (b) a pinned command that passed on an
//! earlier revision and failed on the latest verification, now that the approving
//! review binds the changed file. A pinned command that never passed and does not
//! contradict another stays enforced.
use super::event_tx;
use crate::CoreResult;
use rusqlite::Transaction;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

fn is_hex64(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

/// `(paths, sha256)` a command pins: exactly one hash literal beside a hashing
/// call and at least one file path. Anything else is not a pin.
pub(super) fn pin(argv: &Value) -> Option<(BTreeSet<String>, String)> {
    let text = argv
        .as_array()?
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    let lower = text.to_ascii_lowercase();
    if !(lower.contains("sha256") || lower.contains("hashlib")) {
        return None;
    }
    let mut hashes = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for token in text.split(|c: char| !(c.is_ascii_alphanumeric() || "_./-".contains(c))) {
        if is_hex64(token) {
            hashes.insert(token.to_ascii_lowercase());
        } else if token.len() > 3
            && !token.starts_with('-')
            && token.rsplit_once('.').is_some_and(|(stem, ext)| {
                !stem.is_empty() && (1..=5).contains(&ext.len()) && ext.bytes().all(|b| b.is_ascii_alphabetic())
            })
            && token.contains('/')
        {
            paths.insert(token.trim_start_matches("./").to_owned());
        }
    }
    if hashes.len() != 1 || paths.is_empty() {
        return None;
    }
    Some((paths, hashes.into_iter().next()?))
}

/// Retire stale pinned commands from the plan before verification runs.
/// Returns the retired commands with their reasons.
pub fn retire_superseded_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Vec<Value>> {
    // A task without a plan (a sprint planning task, whose deliverable is the
    // reviewed sprint plan) has no verification commands to retire.
    let raw: Option<String> = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let Some(raw) = raw else { return Ok(Vec::new()) };
    let mut plan: Value = serde_json::from_str(&raw)?;
    let commands = plan["verification"].as_array().cloned().unwrap_or_default();
    let pins: Vec<Option<(BTreeSet<String>, String)>> = commands.iter().map(pin).collect();
    if pins.iter().all(Option::is_none) {
        return Ok(Vec::new());
    }
    // History: when each command first entered the plan, whether it ever passed,
    // and which commands the latest failure left unresolved.
    let mut first: BTreeMap<String, i64> = BTreeMap::new();
    let mut passed: BTreeSet<String> = BTreeSet::new();
    let mut failed_last: BTreeSet<String> = BTreeSet::new();
    let mut stmt = db.prepare(
        "SELECT id,type,payload_json FROM runtime_events WHERE task_id=? AND type IN
         ('PLAN_ACCEPTED','VERIFICATION_REVISED','VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id",
    )?;
    let rows = stmt
        .query_map([task], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, kind, raw) in rows {
        let v: Value = serde_json::from_str(&raw)?;
        match kind.as_str() {
            "PLAN_ACCEPTED" | "VERIFICATION_REVISED" => {
                let list = if kind == "PLAN_ACCEPTED" { &v["plan"]["verification"] } else { &v["verification"] };
                for argv in list.as_array().into_iter().flatten() {
                    first.entry(argv.to_string()).or_insert(id);
                }
            }
            _ => {
                for c in v["checks"].as_array().into_iter().flatten() {
                    if c["complete"] == true && c["exit_code"] == 0 {
                        passed.insert(c["argv"].to_string());
                    }
                }
                if kind == "VERIFICATION_FAILED" {
                    failed_last = super::completion::unresolved_commands(&v, &Value::Null)
                        .iter()
                        .map(Value::to_string)
                        .collect();
                }
            }
        }
    }
    // Newest = latest entry into the plan, then later position.
    let age = |i: usize| (first.get(&commands[i].to_string()).copied().unwrap_or(0), i);
    let mut retire: BTreeMap<usize, &'static str> = BTreeMap::new();
    for i in 0..commands.len() {
        let Some((pi, si)) = &pins[i] else { continue };
        for j in 0..commands.len() {
            let Some((pj, sj)) = &pins[j] else { continue };
            if i != j && si != sj && pi.intersection(pj).next().is_some() && age(i) < age(j) {
                retire.insert(i, "contradicted_by_newer_pinned_check");
            }
        }
        let key = commands[i].to_string();
        if passed.contains(&key) && failed_last.contains(&key) {
            retire.entry(i).or_insert("file_changed_by_accepted_work_since_pin");
        }
    }
    if retire.is_empty() || retire.len() >= commands.len() {
        return Ok(Vec::new());
    }
    let superseded: Vec<Value> = retire
        .iter()
        .map(|(i, reason)| json!({"argv":commands[*i],"reason":reason}))
        .collect();
    let kept: Vec<Value> = commands
        .iter()
        .enumerate()
        .filter(|(i, _)| !retire.contains_key(i))
        .map(|(_, c)| c.clone())
        .collect();
    plan["verification"] = json!(kept);
    db.execute(
        "UPDATE runtime_tasks SET plan_json=? WHERE task_id=?",
        (plan.to_string(), task),
    )?;
    event_tx(
        db,
        task,
        "VERIFICATION_REVISED",
        json!({"verification":kept,"dropped_failed":[],"superseded":superseded}),
    )?;
    Ok(superseded)
}
