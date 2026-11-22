//! The runtime's generic loop guard. A task must never repeat the same thing without
//! progress: every repeated situation (a stop, an owner decision, an infrastructure
//! interruption) gets a stable key, and each sighting is recorded as `LOOP_OBSERVED`
//! `{key, kind, node, count, rungs_used, rung | action}`. The count window starts at
//! the owner's newest input that can change the situation — `TASK_REOPENED`,
//! `USER_STEERING`, `AUTOMATIC_CAP_RENEWED`, or an answer given while this key was the
//! current loop — and never at `AUTOMATIC_RESUMED`, `PLAN_ACCEPTED` or a new plan
//! revision, which every cycle passes through. When the caller breaks a loop it
//! records `LOOP_BROKEN {key, kind, action, limits, evidence}`.
use super::event_tx;
use crate::CoreResult;
use rusqlite::Connection;
use serde_json::{json, Value};

pub const OBSERVED: &str = "LOOP_OBSERVED";
pub const BROKEN: &str = "LOOP_BROKEN";

/// Failure classes of infrastructure, environment and owner events. They are never
/// the model's failure: they never escalate a node, never take a rung of the strategy
/// ladder and never cause a replan.
pub const INFRASTRUCTURE_CLASSES: [&str; 9] = [
    "engine_failure",
    "transport_failure",
    "worker_lost",
    "authorization_loss",
    "capacity_unavailable",
    "owner_paused",
    "runtime_restart",
    "sandbox_unavailable",
    "verifier_environment",
];

/// Whether `class` is one of [`INFRASTRUCTURE_CLASSES`].
pub fn infrastructure_class(class: &str) -> bool {
    INFRASTRUCTURE_CLASSES.contains(&class)
}

/// The infrastructure classes as an SQL list literal (`'a','b',…`), for queries that
/// exclude them.
pub fn infrastructure_sql() -> String {
    INFRASTRUCTURE_CLASSES
        .iter()
        .map(|c| format!("'{c}'"))
        .collect::<Vec<_>>()
        .join(",")
}

/// A subject reduced to what stays the same when a situation repeats: lowercase,
/// whitespace collapsed, absolute paths masked as `<path>`, long hexadecimal
/// identifiers as `<hex>` and every other digit run as `#` (so timestamps, counters,
/// question and report ids do not make a repeat look new). Bounded to 2000 chars.
pub fn normalize(subject: &str) -> String {
    let text: String = subject
        .chars()
        .take(4000)
        .collect::<String>()
        .to_lowercase();
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let boundary = i == 0 || chars[i - 1].is_whitespace() || "\"'(=[,:".contains(chars[i - 1]);
        if c == '/' && boundary && chars.get(i + 1).is_some_and(|n| !n.is_whitespace()) {
            while i < chars.len() && !chars[i].is_whitespace() && !"\"'),]".contains(chars[i]) {
                i += 1;
            }
            out.push_str("<path>");
            continue;
        }
        if c.is_ascii_alphanumeric() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_alphanumeric() {
                i += 1;
            }
            let run: String = chars[start..i].iter().collect();
            let hex = run.len() >= 8
                && run.chars().all(|c| c.is_ascii_hexdigit())
                && run.chars().any(|c| c.is_ascii_digit());
            if hex {
                out.push_str("<hex>");
            } else {
                let mut digits = false;
                for ch in run.chars() {
                    if ch.is_ascii_digit() {
                        if !digits {
                            out.push('#');
                        }
                        digits = true;
                    } else {
                        digits = false;
                        out.push(ch);
                    }
                }
            }
            continue;
        }
        if c.is_whitespace() {
            if !out.ends_with(' ') && !out.is_empty() {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    out.trim().chars().take(2000).collect()
}

/// The node a repeat is about, without the review-fix prefixes (`_fix_<n>_`) the
/// runtime adds each review cycle.
pub fn node_base(node: &str) -> &str {
    let mut node = node;
    while let Some((n, rest)) = node.strip_prefix("_fix_").and_then(|r| r.split_once('_')) {
        if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
            break;
        }
        node = rest;
    }
    node
}

/// The stable key of a repeated situation: a SHA-256 of
/// `kind | node_base(node) | normalize(subject)` (24 hex chars).
pub fn key(kind: &str, node: &str, subject: &str) -> String {
    let material = format!(
        "{kind}|{}|{}",
        normalize(node_base(node)),
        normalize(subject)
    );
    crate::digest(material.as_bytes())[..24].to_owned()
}

/// Items named in free text, reduced to their last path component and normalized:
/// `a/b/x.json, y.json and z.mjs` names `x.json`, `y.json` and `z.mjs`. Sorted and
/// deduplicated, so a reworded or regrouped list of the same items compares equal.
pub fn items(names: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = names
        .iter()
        .flat_map(|name| {
            name.split(", ")
                .flat_map(|p| p.split(" and "))
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .map(|part| {
            let part = part.trim().trim_end_matches('/');
            let last = part.rsplit('/').next().unwrap_or(part);
            normalize(last)
        })
        .filter(|s| !s.is_empty())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// What the guard knows about a key before this sighting.
#[derive(Debug, Clone)]
pub struct History {
    pub key: String,
    /// Sightings in the window, this one included.
    pub count: i64,
    /// Ladder rungs earlier sightings of this key took, in order.
    pub rungs_used: Vec<String>,
    /// Times this key's loop was already broken in the window.
    pub breaks: i64,
    /// Ids of the earlier `LOOP_OBSERVED` events of this key in the window.
    pub events: Vec<i64>,
    /// The first event id of the window (exclusive bound).
    pub since: i64,
}

/// What the caller does with a sighting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Continue on this rung of the strategy ladder (or `resume` for a sighting that
    /// takes no rung).
    Continue { rung: &'static str },
    /// The loop is broken with this action (`disclose`, `ask_owner`,
    /// `infrastructure_wait`).
    Break { action: &'static str },
}

/// The first event id of `key`'s window: the newest `TASK_REOPENED`, `USER_STEERING`,
/// `AUTOMATIC_CAP_RENEWED`, or owner answer given while this key was the current loop
/// (the newest `LOOP_OBSERVED` before the answer is this key's). With `progress`, also
/// the newest real progress other than an accepted plan (`TASK_PROGRESS` of a
/// completed node, a passed check, review or verification), for loops that only
/// repeat when nothing moves (infrastructure interruptions at the same point).
pub fn window_start(db: &Connection, task: &str, key: &str, progress: bool) -> CoreResult<i64> {
    let owner: i64 = db.query_row(
        "SELECT coalesce(max(e.id),0) FROM runtime_events e WHERE e.task_id=?1
           AND (e.type IN ('TASK_REOPENED','USER_STEERING','AUTOMATIC_CAP_RENEWED')
             OR e.type IN ('USER_ANSWER_RECEIVED','WORKER_USER_ANSWER_RECEIVED')
                AND (SELECT json_extract(o.payload_json,'$.key') FROM runtime_events o
                     WHERE o.task_id=?1 AND o.type='LOOP_OBSERVED' AND o.id<e.id
                     ORDER BY o.id DESC LIMIT 1)=?2)",
        (task, key),
        |r| r.get(0),
    )?;
    if !progress {
        return Ok(owner);
    }
    let moved: i64 = db.query_row(
        "SELECT coalesce(max(id),0) FROM runtime_events WHERE task_id=? AND type='TASK_PROGRESS'
           AND coalesce(json_extract(payload_json,'$.kind'),'')!='plan_accepted'",
        [task],
        |r| r.get(0),
    )?;
    Ok(owner.max(moved))
}

/// The history of `key` before a new sighting.
pub fn history(db: &Connection, task: &str, key: &str, progress: bool) -> CoreResult<History> {
    let since = window_start(db, task, key, progress)?;
    let rows = db
        .prepare(
            "SELECT id,json_extract(payload_json,'$.rung') FROM runtime_events
             WHERE task_id=? AND type='LOOP_OBSERVED' AND json_extract(payload_json,'$.key')=? AND id>?
             ORDER BY id",
        )?
        .query_map((task, key, since), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let breaks: i64 = db.query_row(
        "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='LOOP_BROKEN'
           AND json_extract(payload_json,'$.key')=? AND id>?",
        (task, key, since),
        |r| r.get(0),
    )?;
    Ok(History {
        key: key.to_owned(),
        count: rows.len() as i64 + 1,
        rungs_used: rows.iter().filter_map(|(_, r)| r.clone()).collect(),
        breaks,
        events: rows.iter().map(|(id, _)| *id).collect(),
        since,
    })
}

/// Record one sighting of a repeated situation. `decide` sees the key's history and
/// returns what happens; the sighting is recorded with its rung or action. Returns the
/// history, the verdict and the `LOOP_OBSERVED` event id.
#[allow(clippy::too_many_arguments)]
pub fn observe_tx(
    db: &Connection,
    task: &str,
    kind: &str,
    node: &str,
    subject: &str,
    extra: Value,
    progress: bool,
    decide: impl FnOnce(&History) -> Verdict,
) -> CoreResult<(History, Verdict, i64)> {
    let key = key(kind, node, subject);
    let history = history(db, task, &key, progress)?;
    let verdict = decide(&history);
    let mut payload = json!({"key":key,"kind":kind,"node":node_base(node),"count":history.count,
        "rungs_used":history.rungs_used,"subject":normalize(subject).chars().take(300).collect::<String>()});
    match &verdict {
        Verdict::Continue { rung } => payload["rung"] = json!(rung),
        Verdict::Break { action } => payload["action"] = json!(action),
    }
    if let (Some(target), Some(fields)) = (payload.as_object_mut(), extra.as_object()) {
        for (k, v) in fields {
            target.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    let id = event_tx(db, task, OBSERVED, payload)?;
    Ok((history, verdict, id))
}

/// Record that the loop of `history.key` was broken with `action`; `limits` are the
/// disclosed `unverified` runtime checks (empty for a wait), `evidence` what the
/// break rests on (the earlier sightings are always attached).
pub fn broken_tx(
    db: &Connection,
    task: &str,
    history: &History,
    kind: &str,
    action: &str,
    limits: Value,
    evidence: Value,
) -> CoreResult<i64> {
    event_tx(
        db,
        task,
        BROKEN,
        json!({"key":history.key,"kind":kind,"action":action,"limits":limits,
            "count":history.count,"rungs_used":history.rungs_used,
            "evidence":{"observed":history.events,"since":history.since,"detail":evidence}}),
    )
}

/// The first rung of `ladder` this key has not used yet, without the rungs in `skip`
/// (a rung that would repeat the previous attempt unchanged). `prefer` is taken first
/// when it is still unused. `None`: every rung was used for this key.
pub fn next_rung(
    ladder: &[&'static str],
    history: &History,
    skip: &[&str],
    prefer: Option<&'static str>,
) -> Option<&'static str> {
    let open = |r: &&'static str| !history.rungs_used.iter().any(|u| u == r) && !skip.contains(r);
    if let Some(p) = prefer.filter(|p| ladder.contains(p)).filter(|p| open(p)) {
        return Some(p);
    }
    ladder.iter().find(|r| open(r)).copied()
}

/// The wait before resuming number `breaks` (1-based) of an infrastructure loop the
/// runtime waits out: 5, 15, 30 and then 60 minutes, without model calls in between.
pub fn backoff_seconds(breaks: i64) -> i64 {
    match breaks {
        i64::MIN..=1 => 5 * 60,
        2 => 15 * 60,
        3 => 30 * 60,
        _ => 60 * 60,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalization_masks_what_changes_between_repeats() {
        assert_eq!(
            normalize("  Owner answers pq_643872de625f473c8e4c9c9a8c19aaf4 at 2026-11-21T17:39:08Z in /mnt/x/y.json "),
            "owner answers pq_<hex> at #-#-#t#:#:#z in <path>"
        );
        assert_eq!(normalize("Revision 36\n\tfailed"), "revision # failed");
        assert_eq!(node_base("_fix_2__fix_1_work"), "work");
        assert_eq!(node_base("_fix_x_work"), "_fix_x_work");
        assert_eq!(
            key("stop", "_fix_1_work", "A  B 12"),
            key("stop", "work", "a b 99")
        );
        assert_ne!(key("stop", "work", "a"), key("stop", "other", "a"));
        assert_eq!(
            items(&[
                "channels/c/x-wide.json, y-wide.json and z.mjs",
                "a/b/y-wide.json"
            ]),
            vec!["x-wide.json", "y-wide.json", "z.mjs"]
        );
        assert_eq!(backoff_seconds(1), 300);
        assert_eq!(backoff_seconds(9), 3600);
    }
}
