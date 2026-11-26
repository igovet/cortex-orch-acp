//! Review repair and bounded rebinds. Python: orchestration/state.py:697-762,
//! 1799-1848,2020-2075. Findings never widen an implementer's effective scope.
use super::{
    completion::EvidenceSnapshot, domain, event_tx, MAX_REVIEW_CYCLES, MAX_REVIEW_REBINDS,
};
use crate::{
    error::refuse,
    scheduler::policy::{canonical, effective_scope_tx, read_only},
    CoreError, CoreResult, TaskState,
};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
/// A refusal is a committed result, not an error that rolls back the new binding.
/// The outer runtime request ledger permanently retains this key's refusal.
pub fn preflight_result_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: &str,
    key: &str,
    e: &EvidenceSnapshot,
) -> CoreResult<Option<Value>> {
    let stage: Option<String> = db
        .query_row(
            "SELECT stage FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
            (task, assignment),
            |r| r.get(0),
        )
        .optional()?;
    if stage.as_deref() != Some("review") {
        return Ok(None);
    }
    let current: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
        WHERE n.task_id=? AND n.assignment_ref=? AND n.revision=t.revision AND n.status='running'
        AND n.result_json IS NULL AND t.state='REVIEWING')",
        (task, assignment),
        |r| r.get(0),
    )?;
    if !current {
        return Ok(None);
    }
    // What the review binds: its task files' bytes, or the whole checkout for a
    // packet recorded before task-file bindings or one with too many task files
    // (`completion::review_bound_files`).
    // An unrelated file another actor changes never re-binds a task-file binding.
    let (d, _) = super::completion::review_digest_tx(db, task, assignment, e).map_err(|e| {
        if e.code() == "project_revision_unavailable" {
            CoreError::Refused("review_revision_unavailable")
        } else {
            e
        }
    })?;
    let d = d.as_str();
    let rows = db
        .prepare(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED'
        AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id",
        )?
        .query_map((task, assignment), |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let bindings = rows
        .into_iter()
        .map(|s| Ok(serde_json::from_str::<Value>(&s)?))
        .collect::<CoreResult<Vec<_>>>()?;
    if bindings.is_empty() {
        return refuse("review_revision_unavailable");
    }
    let refused_unstable: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='REVIEW_REVISION_UNSTABLE'
         AND json_extract(payload_json,'$.assignment_ref')=? AND json_extract(payload_json,'$.refused_request_key')=?)",
        (task, assignment, key),
        |r| r.get(0),
    )?;
    if refused_unstable || bindings.iter().any(|b| b["refused_request_key"] == key) {
        return Ok(Some(
            json!({"outcome":"refused","error":"review_resubmit_required"}),
        ));
    }
    if bindings.last().unwrap()["revision_digest"] == d
        && crate::workspace::worktrees::receipt_matches(db, task, bindings.last().unwrap(), true)? {
        return Ok(None);
    }
    // A change by another actor outside the task's own files (a whole-checkout
    // binding, no writer of this task since the review first bound) is marked on the
    // refusal's record: it is charged to the runtime, never to the reviewer
    // (`checkout_drift_tx`).
    let first: i64 = db.query_row(
        "SELECT coalesce(min(id),0) FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED'
         AND json_extract(payload_json,'$.assignment_ref')=?",
        (task, assignment),
        |r| r.get(0),
    )?;
    let external = super::completion::review_bound_files(db, task, assignment)?.is_none()
        && !task_wrote_since(db, task, first)?;
    let tag = |mut v: Value| {
        if external {
            v["drift"] = json!("external");
        }
        v
    };
    if bindings.len() > MAX_REVIEW_REBINDS {
        // Past the rebind budget the project counts as unstable while it keeps
        // changing between submissions. Once a submission finds the same digest as
        // the previous unstable refusal (nothing changed in between), the review is
        // re-bound to that state like an ordinary rebind; without that, every later
        // submission compared with the stale binding and could never clear.
        let last: Option<String> = db
            .query_row(
                "SELECT json_extract(payload_json,'$.observed_digest') FROM runtime_events
                 WHERE task_id=? AND type='REVIEW_REVISION_UNSTABLE'
                 AND json_extract(payload_json,'$.assignment_ref')=?
                 AND id>(SELECT max(id) FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED'
                   AND json_extract(payload_json,'$.assignment_ref')=?)
                 ORDER BY id DESC LIMIT 1",
                (task, assignment, task, assignment),
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        if last.as_deref() != Some(d) {
            event_tx(
                db,
                task,
                "REVIEW_REVISION_UNSTABLE",
                tag(json!({"assignment_ref":assignment,"observed_digest":d,
                    "bound_digest":bindings.last().unwrap()["revision_digest"],"refused_request_key":key})),
            )?;
            return Ok(Some(
                json!({"outcome":"refused","error":"review_project_revision_unstable"}),
            ));
        }
    }
    event_tx(
        db,
        task,
        "REVIEW_STARTED",
        tag(super::completion::tag_checkout(json!({"assignment_ref":assignment,"revision_digest":d,
        "rebound_from":bindings.last().unwrap()["revision_digest"],"rebind":bindings.len(),"refused_request_key":key,
        "after_unstable":bindings.len() > MAX_REVIEW_REBINDS}), e)),
    )?;
    Ok(Some(
        json!({"outcome":"refused","error":"review_project_revision_changed"}),
    ))
}
/// fnmatch-like * and ? for scope matching; literal directory scopes include
/// their descendants. Bracket patterns conservatively fall back to all fixes.
fn matches(path: &str, scope: &str) -> bool {
    let path = canonical(path);
    let scope = canonical(scope);
    if scope == "." || path == scope || path.starts_with(&(scope.clone() + "/")) {
        return true;
    }
    let p: Vec<_> = path.chars().collect();
    let s: Vec<_> = scope.chars().collect();
    let mut dp = vec![false; p.len() + 1];
    dp[0] = true;
    for c in s {
        let mut next = vec![false; p.len() + 1];
        if c == '*' {
            next[0] = dp[0];
        }
        for i in 1..=p.len() {
            next[i] = if c == '*' {
                dp[i] || next[i - 1]
            } else {
                (c == '?' || c == p[i - 1]) && dp[i - 1]
            };
        }
        dp = next;
    }
    dp[p.len()]
}
/// A test file wherever it lives: under a `tests`/`test` folder, or named like a
/// test (`*.test.*`, `*.spec.*`, `*_test.*`, `test_*`).
fn test_file(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    path.split('/')
        .rev()
        .skip(1)
        .any(|d| d == "tests" || d == "test" || d == "__tests__")
        || name.contains(".test.")
        || name.contains(".spec.")
        || name.contains("_test.")
        || name.starts_with("test_")
}

/// The developer for product code a review finding names: frontend sources to the
/// frontend developer, scripts and CI to DevOps, a mix of frontend and other code
/// to the full-stack developer, anything else to the backend developer.
fn code_role(files: &[String]) -> &'static str {
    let ext = |p: &str| p.rsplit_once('.').map_or("", |(_, e)| e).to_owned();
    let frontend = |p: &String| {
        [
            "ts", "tsx", "js", "jsx", "mjs", "css", "scss", "html", "vue", "svelte",
        ]
        .contains(&ext(p).as_str())
    };
    let ops = |p: &String| {
        p.starts_with("scripts/")
            || p.starts_with(".github/")
            || p.rsplit('/')
                .next()
                .is_some_and(|n| n.starts_with("Dockerfile"))
            || ext(p) == "sh"
    };
    let code: Vec<&String> = files
        .iter()
        .filter(|p| !p.ends_with(".md") && !p.starts_with("docs/") && !test_file(p))
        .collect();
    if !code.is_empty() && code.iter().all(|p| ops(p)) {
        "worker-devops-engineer"
    } else if !code.is_empty() && code.iter().all(|p| frontend(p)) {
        "worker-frontend-dev"
    } else if code.iter().any(|p| frontend(p)) {
        "worker-fullstack-dev"
    } else {
        "worker-backend-dev"
    }
}

fn strings(v: &Value) -> CoreResult<Vec<String>> {
    v.as_array()
        .ok_or(CoreError::Refused("invalid_review_findings"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or(CoreError::Refused("invalid_review_findings"))
        })
        .collect()
}
/// Why a review could not cover one part of the change itself. These are the only
/// dead ends: anything else the reviewer can still do inside its own assignment.
pub const COVERAGE_BLOCKERS: &[&str] = &[
    "missing_access",
    "owner_decision",
    "other_role_work",
    "tool_unavailable",
];
/// The same categories are the dead ends a worker may name for a failed assignment.
pub fn valid_blocker(kind: &Value) -> bool {
    kind.as_str()
        .is_some_and(|k| COVERAGE_BLOCKERS.contains(&k))
}
/// The `not_covered` parts of an INCOMPLETE review that name no dead end. The
/// reviewer can still cover them, so the result is not final: the runtime refuses
/// it with exactly these items and the same review session continues.
pub fn unfinished_coverage(result: &Value) -> Vec<String> {
    if !result["verdict"]
        .as_str()
        .is_some_and(|v| v.eq_ignore_ascii_case("INCOMPLETE"))
    {
        return Vec::new();
    }
    result["not_covered"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|gap| !valid_blocker(&gap["blocker"]))
        .map(|gap| gap["item"].as_str().unwrap_or("").to_owned())
        .collect()
}
/// Parts an INCOMPLETE review declared blocked by a dead end, by their item text.
pub fn blocked_items(result: &Value) -> BTreeSet<String> {
    if !result["verdict"]
        .as_str()
        .is_some_and(|v| v.eq_ignore_ascii_case("INCOMPLETE"))
    {
        return BTreeSet::new();
    }
    result["not_covered"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|gap| valid_blocker(&gap["blocker"]))
        .filter_map(|gap| gap["item"].as_str().map(str::to_owned))
        .collect()
}
/// A BLOCK must name the confirmed problems that block.
pub fn block_without_findings(result: &Value) -> bool {
    result["verdict"]
        .as_str()
        .is_some_and(|v| v.eq_ignore_ascii_case("BLOCK"))
        && result["findings"].as_array().is_none_or(Vec::is_empty)
}
/// Any concrete finding of an INCOMPLETE review is repaired first, a cross-cutting
/// one included (it takes the conservative fix of every plan task); the fresh
/// review after the fix covers the new bytes. An INCOMPLETE review with findings
/// never reaches verification.
fn actionable_findings(result: &Value) -> bool {
    let Some(findings) = result["findings"].as_array() else {
        return false;
    };
    !findings.is_empty()
        && findings.iter().all(|finding| {
            finding.is_object()
                && finding["fix"]
                    .as_str()
                    .is_some_and(|fix| !fix.trim().is_empty())
                && finding["files"]
                    .as_array()
                    .is_some_and(|files| !files.is_empty())
        })
}
/// The first plan revision of the current task lineage: the one after the newest
/// reopen, or the first revision when the task was never reopened. Budgets and
/// no-progress checks that span revisions count from here, never from a plan
/// acceptance or a continuation.
pub(crate) fn lineage_start_tx(db: &rusqlite::Connection, task: &str) -> CoreResult<i64> {
    Ok(db.query_row(
        "SELECT coalesce((SELECT json_extract(payload_json,'$.revision')+1 FROM runtime_events
           WHERE task_id=? AND type='TASK_REOPENED' ORDER BY id DESC LIMIT 1),0)",
        [task],
        |r| r.get(0),
    )?)
}
/// The id of the newest event that starts a fresh window for repeat detection: a
/// reopen or an owner message (0 when there is none).
pub(crate) fn repeat_window_tx(db: &rusqlite::Connection, task: &str) -> CoreResult<i64> {
    Ok(db.query_row(
        "SELECT coalesce(max(id),0) FROM runtime_events WHERE task_id=? AND type IN ('TASK_REOPENED','USER_STEERING')",
        [task],
        |r| r.get(0),
    )?)
}
/// A stable loop key: the digest of `kind`, the node without its fix-round prefix
/// and the normalized subject.
pub(crate) fn loop_key(kind: &str, node: &str, subject: &str) -> String {
    let mut base = node;
    while let Some(rest) = base.strip_prefix("_fix_") {
        base = rest.split_once('_').map_or(rest, |(_, r)| r);
    }
    let base = if base.starts_with("_review_") {
        "_review"
    } else {
        base
    };
    crate::digest(format!("{kind}|{base}|{}", normalize_text(subject)).as_bytes())
}
/// Lowercased text with whitespace collapsed.
pub(crate) fn normalize_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
/// A dead-end item's name without the reviewer's detail: `change_set (host could not …)`
/// and `change_set: changed paths …` both name `change_set`.
pub fn dead_end_item(item: &str) -> String {
    let end = [item.find(" ("), item.find(": ")]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(item.len());
    item[..end]
        .trim()
        .chars()
        .take(160)
        .collect::<String>()
        .to_lowercase()
}
/// The file names a dead-end item or decision text mentions: the last component of
/// every path-like token (`a/b/c.json, d.json and e/f.mjs` names `c.json`, `d.json`
/// and `f.mjs`), lowercased. Rewording an item ("the three twins" split into one item
/// per file) keeps its names.
pub(crate) fn named_files(text: &str) -> BTreeSet<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, ',' | ';' | '(' | ')' | '`' | '"' | '\''))
        .map(|t| t.trim_matches(|c: char| matches!(c, '.' | ':' | '[' | ']' | '«' | '»')))
        .filter(|t| {
            t.contains('/')
                || t.rsplit_once('.').is_some_and(|(stem, ext)| {
                    !stem.is_empty()
                        && (1..=5).contains(&ext.len())
                        && ext.chars().all(|c| c.is_ascii_alphanumeric())
                        && ext.chars().any(|c| c.is_ascii_alphabetic())
                })
        })
        .filter_map(|t| t.trim_end_matches('/').rsplit('/').next())
        .filter(|t| t.len() > 2 && !t.starts_with("pq_"))
        .map(str::to_lowercase)
        .collect()
}
/// Owner question ids (`pq_` and 32 hexadecimal digits) cited in `text`.
pub(crate) fn cited_questions(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("pq_") {
        let id: String = rest[at + 3..]
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .collect();
        if id.len() == 32 {
            out.push(format!("pq_{id}"));
        }
        rest = &rest[at + 3..];
    }
    out
}
/// The owner's recorded answer text to question `id`, when it was answered.
pub(crate) fn answer_text_tx(
    db: &rusqlite::Connection,
    task: &str,
    id: &str,
) -> CoreResult<Option<String>> {
    Ok(db
        .query_row(
            "SELECT coalesce(answer_text,'') FROM pending_questions WHERE task_id=? AND id=? AND status='answered'",
            (task, id),
            |r| r.get(0),
        )
        .optional()?)
}
/// The answer to the first owner question answered after event `after`: its text,
/// or an empty string when the answer event names no recorded answer text.
pub(crate) fn answer_after_tx(
    db: &rusqlite::Connection,
    task: &str,
    after: i64,
) -> CoreResult<Option<String>> {
    let id: Option<Option<String>> = db
        .query_row(
            "SELECT json_extract(payload_json,'$.question_id') FROM runtime_events WHERE task_id=? AND id>?
             AND type IN ('USER_ANSWER_RECEIVED','WORKER_USER_ANSWER_RECEIVED') ORDER BY id LIMIT 1",
            (task, after),
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = id else { return Ok(None) };
    Ok(Some(match id {
        Some(id) => answer_text_tx(db, task, &id)?.unwrap_or_default(),
        None => String::new(),
    }))
}
/// Whether an `owner_decision` dead end of a review is one the owner already decided,
/// and the owner's answer when so. It is decided when its text cites an owner question
/// that was answered, or when an earlier review of this task (since the newest reopen
/// or owner message) routed the same item as an `owner_decision` dead end and an owner
/// answer followed. The same item is matched after rewording: the same item name
/// without the reviewer's detail ([`dead_end_item`]), or only file names the earlier
/// decided items already named ([`named_files`]); owner store 2026-11-21: three
/// generated twins routed as one item were split into one item per file by the next
/// review, and the owner was asked again.
pub fn decided_owner_item_tx(
    db: &rusqlite::Connection,
    task: &str,
    item: &str,
    reason: &str,
) -> CoreResult<Option<String>> {
    if item.trim().is_empty() {
        return Ok(None);
    }
    for id in cited_questions(&format!("{item}\n{reason}")) {
        if let Some(answer) = answer_text_tx(db, task, &id)? {
            return Ok(Some(answer));
        }
    }
    let since = repeat_window_tx(db, task)?;
    let rows = db
        .prepare(
            "SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_DEAD_END_ROUTED'
             AND id>? AND json_valid(payload_json) ORDER BY id DESC LIMIT 64",
        )?
        .query_map((task, since), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    let name = dead_end_item(item);
    let files = named_files(item);
    let mut decided_files = BTreeSet::new();
    let mut answer = None;
    for (id, raw) in rows {
        let Some(found) = answer_after_tx(db, task, id)? else {
            continue;
        };
        let event: Value = serde_json::from_str(&raw)?;
        for b in event["blocked"].as_array().into_iter().flatten() {
            if b["blocker"] != "owner_decision" {
                continue;
            }
            let earlier = b["item"].as_str().unwrap_or_default();
            if !earlier.trim().is_empty() && dead_end_item(earlier) == name {
                return Ok(Some(found));
            }
            decided_files.extend(named_files(earlier));
            answer.get_or_insert_with(|| found.clone());
        }
    }
    if !files.is_empty() && files.is_subset(&decided_files) {
        return Ok(answer);
    }
    Ok(None)
}
/// Review refusal codes caused by the project changing under a running review.
pub const DRIFT_REFUSALS: &[&str] = &[
    "review_project_revision_changed",
    "review_project_revision_unstable",
    "review_resubmit_required",
];
/// Whether a writer of this task changed project files after event `after`: a
/// gateway file write, a command whose run changed paths, or a writing turn that
/// changed paths.
fn task_wrote_since(db: &rusqlite::Connection, task: &str, after: i64) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?1 AND id>?2 AND json_valid(payload_json) AND (
            type IN ('NATIVE_PROJECT_WRITTEN','NATIVE_PROJECT_CHANGED')
         OR type='NATIVE_WRITER_TURN_CHANGES' AND coalesce(json_array_length(json_extract(payload_json,'$.changed_paths')),0)>0
         OR type='NATIVE_COMMAND_FINISHED'
            AND coalesce(json_array_length(json_extract(payload_json,'$.receipt.observation.changed_paths')),0)>0))",
        (task, after),
        |r| r.get(0),
    )?)
}
/// The decision for review refusals caused by another actor changing the checkout
/// outside the task's own files (infrastructure, not the reviewer's failure). It
/// applies when the review binds the whole checkout (attribution was incomplete, see
/// `completion::review_bound_files`), no writer of this task changed a file since the
/// review first bound its revision, and the reviewer's assignment was refused for a
/// changed or unstable revision. A task-file binding only re-binds when one of the
/// task's own files changed, which the reviewer must re-read, so it never qualifies.
/// `Some` carries the evidence: such refusals, and a review that ends because of
/// them, are retried on the runtime's budget, never the reviewer's (no review cycle,
/// no rung of the strategy ladder).
pub fn checkout_drift_tx(
    db: &rusqlite::Connection,
    task: &str,
    assignment: &str,
) -> CoreResult<Option<Value>> {
    if super::completion::review_bound_files(db, task, assignment)?.is_some() {
        return Ok(None);
    }
    let first: Option<i64> = db.query_row(
        "SELECT min(id) FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED'
         AND json_extract(payload_json,'$.assignment_ref')=?",
        (task, assignment),
        |r| r.get(0),
    )?;
    let Some(first) = first else { return Ok(None) };
    let refusals = db
        .prepare(
            "SELECT json_extract(payload_json,'$.code') FROM runtime_events WHERE task_id=? AND type='WORKER_RESULT_REFUSED'
             AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id",
        )?
        .query_map((task, assignment), |r| r.get::<_, Option<String>>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let drift = refusals
        .iter()
        .filter(|c| c.as_deref().is_some_and(|c| DRIFT_REFUSALS.contains(&c)))
        .count();
    if drift == 0 || task_wrote_since(db, task, first)? {
        return Ok(None);
    }
    let rebinds: i64 = db.query_row(
        "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED'
         AND json_extract(payload_json,'$.assignment_ref')=? AND json_extract(payload_json,'$.rebind') IS NOT NULL",
        (task, assignment),
        |r| r.get(0),
    )?;
    Ok(Some(
        json!({"cause":"external_drift","charged":"runtime","binding":"checkout",
        "drift_refusals":drift,"rebinds":rebinds,
        "last_refusal":refusals.last().cloned().flatten()}),
    ))
}
/// One finding's identity: the plan task, its files and the requested fix, with
/// case and spacing normalized.
fn finding_key(finding: &Value) -> (String, Vec<String>, String) {
    let mut files: Vec<String> = finding["files"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|f| f.as_str().map(canonical))
        .collect();
    files.sort();
    let fix = finding["fix"]
        .as_str()
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    (
        finding["node"].as_str().unwrap_or("").to_owned(),
        files,
        fix,
    )
}
/// A finding's identity across revisions: its files and requested fix. A replan may
/// rename the plan task that owns the same files, so the node is left out.
fn finding_subject(finding: &Value) -> (Vec<String>, String) {
    let (_, files, fix) = finding_key(finding);
    (files, fix)
}
fn finding_subjects(result: &Value) -> BTreeSet<(Vec<String>, String)> {
    result["findings"]
        .as_array()
        .into_iter()
        .flatten()
        .map(finding_subject)
        .collect()
}
/// The loop key of a review's findings set (`review_findings`).
fn findings_key(result: &Value) -> String {
    let subject = finding_subjects(result)
        .into_iter()
        .map(|(files, fix)| format!("{}:{fix}", files.join(",")))
        .collect::<Vec<_>>()
        .join("\n");
    loop_key("review_findings", "_review", &subject)
}
/// Whether a completed review asked for changes with findings to repair.
fn requests_changes(result: &Value) -> bool {
    result["verdict"] == "REQUEST_CHANGES"
        || result["verdict"] == "INCOMPLETE" && actionable_findings(result)
}
/// Completed reviews of the task lineage ([`lineage_start_tx`]) other than `review`
/// of `revision`, newest first, of the same kind (plan reviews or change reviews).
fn lineage_reviews_tx(
    db: &Transaction<'_>,
    task: &str,
    revision: i64,
    review: &str,
) -> CoreResult<Vec<(i64, Value)>> {
    let plan_review = super::sprint_planning::is_plan_review(review);
    let start = lineage_start_tx(db, task)?;
    let rows = db
        .prepare(
            "SELECT revision,node_id,result_json FROM runtime_nodes WHERE task_id=? AND stage='review'
             AND status='completed' AND result_json IS NOT NULL AND revision>=? AND revision<=?
             AND NOT (revision=? AND node_id=?) ORDER BY revision DESC, rowid DESC LIMIT 256",
        )?
        .query_map((task, start, revision, revision, review), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut out = Vec::new();
    for (rev, node, raw) in rows {
        // A plan review compares only with plan reviews of its own revision.
        if super::sprint_planning::is_plan_review(&node) != plan_review
            || plan_review && rev != revision
        {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<Value>(&raw) {
            out.push((rev, v));
        }
    }
    Ok(out)
}
/// Whether this review shows no progress over the previous completed review (the one
/// whose findings the last fixes addressed): within the revision the reviewed bytes
/// are unchanged, or, in this revision or an earlier one of the task lineage, the same
/// findings (files and fix) came back. Comparing across revisions matters because a
/// continuation after a stop plans a new revision: without it, identical findings in
/// each new revision repeated the whole fix cycle forever. `None` when this is the
/// first review or something changed; otherwise the reason and the previous
/// review's revision.
fn no_progress_tx(
    db: &Transaction<'_>,
    task: &str,
    revision: i64,
    review: &str,
    result: &Value,
) -> CoreResult<Option<(&'static str, i64)>> {
    let Some((previous_revision, previous)) = lineage_reviews_tx(db, task, revision, review)?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    if !matches!(
        previous["verdict"].as_str(),
        Some("REQUEST_CHANGES" | "INCOMPLETE")
    ) {
        return Ok(None);
    }
    if previous_revision != revision {
        let current = finding_subjects(result);
        if !current.is_empty() && current == finding_subjects(&previous) {
            return Ok(Some(("repeated_findings", previous_revision)));
        }
        return Ok(None);
    }
    // What each review bound: its task files when both have that binding (another
    // actor's edits elsewhere are not progress), otherwise the whole checkout.
    let field = if result["review_digest"].is_string() && previous["review_digest"].is_string() {
        "review_digest"
    } else {
        "project_digest"
    };
    let digest = |v: &Value| {
        v[field]
            .as_str()
            .filter(|d| d.len() == 64)
            .map(str::to_owned)
    };
    if digest(result).is_some() && digest(result) == digest(&previous) {
        return Ok(Some(("unchanged_files", revision)));
    }
    let keys = |v: &Value| -> BTreeSet<(String, Vec<String>, String)> {
        v["findings"]
            .as_array()
            .into_iter()
            .flatten()
            .map(finding_key)
            .collect()
    };
    let current = keys(result);
    if !current.is_empty() && current == keys(&previous) {
        return Ok(Some(("repeated_findings", revision)));
    }
    Ok(None)
}
/// The review-cycle budget of the task lineage. `review_cycle` counts the reviews of
/// one plan revision (it names review and fix nodes and bounds one revision's
/// reviews); a new plan revision restarts that count but not this budget. Once
/// [`MAX_REVIEW_CYCLES`] change-requesting reviews were recorded in the lineage
/// (this one included), a further fix round is granted while this review raises a
/// finding no earlier review of the lineage raised (same files and normalized fix):
/// new problems are progress. It is refused when every finding was already raised
/// before (the work is going around), and after [`MAX_LINEAGE_REVIEW_CYCLES`] rounds
/// in any case, so reworded repeats cannot cycle forever. Counting findings instead
/// stopped a task whose three reviews each found one different problem (owner store
/// 2026-11-22); before any budget, one task ran nine review-cycle ladders with
/// eighteen change requests (2026-11-21).
/// `Some((rounds, previous, current))` when the budget refuses another round.
fn lineage_budget_exhausted_tx(
    db: &Transaction<'_>,
    task: &str,
    revision: i64,
    review: &str,
    result: &Value,
) -> CoreResult<Option<(usize, usize, usize)>> {
    let earlier: Vec<Value> = lineage_reviews_tx(db, task, revision, review)?
        .into_iter()
        .map(|(_, v)| v)
        .filter(requests_changes)
        .collect();
    let rounds = earlier.len() + 1;
    if rounds < MAX_REVIEW_CYCLES {
        return Ok(None);
    }
    let current = finding_subjects(result);
    let seen: BTreeSet<_> = earlier.iter().flat_map(finding_subjects).collect();
    let previous = earlier.first().map_or(0, |p| finding_subjects(p).len());
    let new_problem = current.iter().any(|f| !seen.contains(f));
    if new_problem && rounds < MAX_LINEAGE_REVIEW_CYCLES {
        return Ok(None);
    }
    Ok(Some((rounds, previous, current.len())))
}
/// Change-requesting reviews one task lineage may run while each raises a new problem.
const MAX_LINEAGE_REVIEW_CYCLES: usize = 12;
/// The parts of the change a review reports it did not cover, as given in its
/// `not_covered`.
fn uncovered(result: &Value) -> Vec<Value> {
    result["not_covered"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}
/// A review that could not reach some items with its tools, and found nothing to
/// repair: those items become disclosed unverified limits of the review, appended
/// to its result as `unverified` runtime checks that the final result must disclose
/// (`REVIEW_LIMIT_DISCLOSED`), and the task continues to verification. The review
/// itself is not waived: everything it could read was reviewed, and its verdict and
/// binding stay those of that review.
fn disclose_review_limits_tx(
    db: &Transaction<'_>,
    task: &str,
    revision: i64,
    review: &str,
    result: &Value,
    blocked: &[Value],
) -> CoreResult<()> {
    let limits: Vec<Value> = blocked
        .iter()
        .map(|b| {
            let item = b["item"].as_str().unwrap_or("an item").chars().take(200).collect::<String>();
            let reason = b["reason"].as_str().unwrap_or("").chars().take(400).collect::<String>();
            if b["blocker"] == "owner_decision" || b["owner_accepted"] == true {
                return json!({"name":format!("Review did not read {item}"),"outcome":"unverified",
                    "blocker":format!("The owner decided that {item} is not reviewed{}; it is not covered by the review.",
                        if reason.is_empty() { String::new() } else { format!(" ({reason})") }),
                    "source":"runtime"});
            }
            json!({"name":format!("Review could not read {item}"),"outcome":"unverified",
                "blocker":if reason.is_empty() {
                    format!("The reviewer's tools could not reach {item}; it is not covered by the review.")
                } else {
                    format!("The reviewer's tools could not reach {item} ({reason}); it is not covered by the review.")
                },
                "source":"runtime"})
        })
        .collect();
    let mut result = result.clone();
    match result["checks"].as_array_mut() {
        Some(checks) => checks.extend(limits.iter().cloned()),
        None => result["checks"] = json!(limits),
    }
    result["review_limit_disclosed"] = json!(true);
    result["review_limit_owner_authorized"] = json!(true);
    db.execute(
        "UPDATE runtime_nodes SET result_json=? WHERE task_id=? AND revision=? AND node_id=?",
        (result.to_string(), task, revision, review),
    )?;
    event_tx(
        db,
        task,
        "REVIEW_LIMIT_DISCLOSED",
        json!({"review":review,"revision":revision,"report_ref":result["report_ref"],"limits":limits,
            "owner_authorized":true}),
    )?;
    domain::state_tx(db, task, TaskState::Verifying, None)
}
/// The verification commands of the plan whose approved stage missed the goal
/// (`REVIEW_GOAL_NOT_MET` after the latest accepted plan): they never ran, so they
/// are still owed by the next plan.
pub fn owed_goal_commands(db: &rusqlite::Connection, task: &str) -> CoreResult<Vec<Value>> {
    let owed: Option<String> = db
        .query_row(
            "SELECT json_extract(payload_json,'$.verification') FROM runtime_events WHERE task_id=?1
             AND type='REVIEW_GOAL_NOT_MET'
             AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='PLAN_ACCEPTED'),0)
             ORDER BY id DESC LIMIT 1",
            [task],
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    Ok(owed
        .and_then(|o| serde_json::from_str::<Value>(&o).ok())
        .and_then(|o| o.as_array().cloned())
        .unwrap_or_default())
}
/// Whether the owner accepted a review dead end of this task as not verified
/// (`LOOP_BROKEN` `accept_unverified` for a `review_incomplete` stop) since the
/// newest reopen or owner message: the owner's close-as-is for the unreachable parts.
fn owner_accepted_review_tx(db: &Transaction<'_>, task: &str) -> CoreResult<bool> {
    let since = repeat_window_tx(db, task)?;
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='LOOP_BROKEN' AND id>?
         AND json_extract(payload_json,'$.action')='accept_unverified'
         AND json_extract(payload_json,'$.evidence.stop')='review_incomplete')",
        (task, since),
        |r| r.get(0),
    )?)
}
/// The review that a follow-up review of the same revision replaced, when
/// `review` is such a follow-up (`REVIEW_RERUN`).
fn rerun_of_tx(db: &Transaction<'_>, task: &str, revision: i64, review: &str) -> CoreResult<bool> {
    Ok(db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='REVIEW_RERUN'
         AND json_extract(payload_json,'$.revision')=? AND json_extract(payload_json,'$.rerun')=?)",
        (task, revision, review),
        |r| r.get(0),
    )?)
}
/// One follow-up review of the same revision, narrowed to the parts the previous
/// review's tools could not reach, on another route and with another approach
/// (`REVIEW_RERUN`). Files the earlier review read in their current bytes stay
/// read (`reviewed_before`). False when the review budget of the revision is used.
fn narrow_review_tx(
    db: &Transaction<'_>,
    task: &str,
    revision: i64,
    review: &str,
    result: &Value,
    blocked: &[Value],
) -> CoreResult<bool> {
    let mut cycle: i64 = db.query_row(
        "SELECT review_cycle+1 FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    if cycle > MAX_REVIEW_CYCLES as i64 {
        return Ok(false);
    }
    while db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND revision=? AND node_id=?)",
        (task, revision, format!("_review_{cycle}")),
        |r| r.get::<_, bool>(0),
    )? {
        cycle += 1;
    }
    let deps: String = db.query_row(
        "SELECT dependencies_json FROM runtime_nodes WHERE task_id=? AND revision=? AND node_id=?",
        (task, revision, review),
        |r| r.get(0),
    )?;
    let mut deps: Vec<String> = serde_json::from_str(&deps)?;
    deps.push(review.to_owned());
    let mut objective = format!(
        "Follow-up review. The previous review ({review}, report {}) could not reach these parts of the change with its tools:\n",
        result["report_ref"].as_str().unwrap_or("")
    );
    for b in blocked {
        objective.push_str(&format!(
            "- {} ({})\n",
            b["item"].as_str().unwrap_or("unnamed part").chars().take(300).collect::<String>(),
            b["reason"].as_str().unwrap_or("tool unavailable").chars().take(400).collect::<String>()
        ));
    }
    objective.push_str("Cover exactly these parts first, with a different approach than the previous review (other read tools, line ranges, paging, a read-only command); files it read in their current bytes are marked reviewed_before. Then judge the whole work against the original goal in the review packet.");
    let objective: String = objective.chars().take(8000).collect();
    let node = format!("_review_{cycle}");
    db.execute("INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status)
        VALUES (?,?,?,'worker-code-reviewer',?,?,'[\"**\"]','review','pending')",
        (task, revision, &node, &objective, json!(deps).to_string()))?;
    let plan: Option<String> = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let plan: Value = serde_json::from_str(plan.as_deref().unwrap_or("{}"))?;
    let routing = super::plan::routing_fields(&super::plan::node_routing(&plan, review));
    super::plan::record_node_routing_tx(db, task, &node, &routing)?;
    db.execute(
        "UPDATE runtime_tasks SET review_cycle=? WHERE task_id=?",
        (cycle, task),
    )?;
    event_tx(
        db,
        task,
        "REVIEW_RERUN",
        json!({"review":review,"rerun":node,"revision":revision,
            "reason":"The previous review's tools could not reach some parts of the change; a follow-up review covers them with another approach.",
            "items":blocked.iter().map(|b| b["item"].clone()).collect::<Vec<_>>()}),
    )?;
    Ok(true)
}
/// Select the latest review's outcome. Approval enables final verification;
/// request-changes schedules bounded scoped fixes; BLOCK with findings asks about the
/// stop; INCOMPLETE (or a legacy BLOCK naming no finding) routes its named dead ends
/// as a stop without scheduling another review of the same revision.
pub fn advance_tx(db: &Transaction<'_>, task: &str) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    if state != TaskState::Reviewing {
        return refuse("review_not_expected");
    }
    let row: Option<(String, String, Option<String>)> = db
        .query_row(
            "SELECT node_id,status,result_json FROM runtime_nodes
        WHERE task_id=? AND revision=? AND stage='review' ORDER BY rowid DESC LIMIT 1",
            (task, revision),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (review, status, result) = row.ok_or(CoreError::Refused("review_not_expected"))?;
    if status != "completed" {
        return refuse("review_incomplete");
    }
    let result: Value =
        serde_json::from_str(&result.ok_or(CoreError::Refused("review_incomplete"))?)?;
    // The review of a proposed sprint revision settles only that revision.
    if super::sprint_impact::is_revision_review(&review) {
        return super::sprint_impact::review_done_tx(db, task, &review, &result);
    }
    // The review of a submitted sprint plan (see `sprint_planning`): findings return the
    // plan to its coordinator for a revision within the same bounded rules.
    let plan_review = super::sprint_planning::is_plan_review(&review);
    match result["verdict"].as_str() {
        Some("APPROVE") => {
            // The stage is approved, but an outcome of the goal is not delivered: no
            // verification. The coordinator plans the rest; the plan's verification
            // commands, which never ran, stay owed (`plan::required_verification`).
            let undelivered = if plan_review {
                Vec::new()
            } else {
                super::completion::undelivered_outcomes(db, task, &result)?
            };
            if !undelivered.is_empty() {
                let plan: Option<String> = db.query_row(
                    "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
                    [task],
                    |r| r.get(0),
                )?;
                let plan: Value = serde_json::from_str(plan.as_deref().unwrap_or("{}"))?;
                event_tx(
                    db,
                    task,
                    "REVIEW_GOAL_NOT_MET",
                    json!({"review":review,"revision":revision,"report_ref":result["report_ref"],
                        "undelivered":undelivered,"verification":plan["verification"]}),
                )?;
                event_tx(
                    db,
                    task,
                    "TASK_PROGRESS",
                    json!({"kind":"review_goal_not_met","node":review,"revision":revision}),
                )?;
                if !domain::return_to_coordinator_tx(db, task, "goal_not_met")? {
                    domain::stop_tx(db, task, "goal_not_met")?;
                }
                return Ok(());
            }
            event_tx(
                db,
                task,
                "TASK_PROGRESS",
                json!({"kind":"review_passed","node":review,"revision":revision}),
            )?;
            return domain::state_tx(db, task, TaskState::Verifying, None);
        }
        Some("BLOCK")
            if plan_review && result["findings"].as_array().is_some_and(|f| !f.is_empty()) => {}
        // A BLOCK with confirmed problems goes to the coordinator to address them.
        Some("BLOCK") if result["findings"].as_array().is_some_and(|f| !f.is_empty()) => {
            domain::stop_tx(db, task, "review_blocked")?;
            return Ok(());
        }
        // Concrete findings are repaired first; the review after the fix covers the
        // new bytes, including what this one could not reach.
        Some("INCOMPLETE") if actionable_findings(&result) => {}
        // A reviewer finishes its own review: result_submit refuses a part it can still
        // cover. What remains is a dead end it named with its blocker. Another reviewer
        // of the same bytes would meet the same dead end, so none is scheduled.
        // Items its tools cannot reach (`tool_unavailable`) and nothing else: the
        // runtime records them as disclosed unverified limits of the review and the
        // task continues to verification; the final result must disclose them. Any
        // other dead end (access, another role's work, a decision) is a continuation
        // that changes approach (no-stop policy), never a stop for the owner. A legacy
        // result without blockers, or a BLOCK naming no problem, takes the same route.
        Some("INCOMPLETE" | "BLOCK") => {
            // A follow-up review that again could not reach the same kind of part:
            // no further review can, so the owner decides (close it as is, or how
            // to make it reviewable). The item is routed as an owner decision.
            let narrowed = rerun_of_tx(db, task, revision, &review)?;
            let blocked: Vec<Value> = uncovered(&result)
                .into_iter()
                .map(|gap| {
                    if narrowed && gap["blocker"] == "tool_unavailable" {
                        json!({"item":gap["item"],"blocker":"owner_decision","reason":format!(
                            "{}. A follow-up review with another approach could not reach it either; the owner decides whether to close it as is, without review, or how to make it reviewable",
                            gap["reason"].as_str().unwrap_or("The reviewer's tools could not reach it").trim_end_matches('.'))})
                    } else {
                        json!({"item":gap["item"],"blocker":gap["blocker"],"reason":gap["reason"]})
                    }
                })
                .collect();
            event_tx(
                db,
                task,
                "REVIEW_DEAD_END_ROUTED",
                json!({"review":review,"revision":revision,"report_ref":result["report_ref"],
                    "verdict":result["verdict"],"blocked":blocked}),
            )?;
            // Retry only a review whose actionable blocker is access. Mixed
            // dead ends and concrete findings still take the ordinary route.
            if blocked.iter().all(|b| matches!(b["blocker"].as_str(), Some("missing_access" | "tool_unavailable"))) {
                if let Some(b) = blocked.iter().find(|b| b["blocker"] == "missing_access") {
                    let assignment: Option<String> = db.query_row(
                        "SELECT assignment_ref FROM runtime_nodes WHERE task_id=? AND revision=? AND node_id=?",
                        (task, revision, &review), |r| r.get(0),
                    )?;
                    let found = super::obstacles::classify(&super::obstacles::Signals {
                        blocker_kind: Some("missing_access".into()),
                        blocker_detail: Some(format!("{}: {}", b["item"].as_str().unwrap_or(""), b["reason"].as_str().unwrap_or(""))),
                        ..Default::default()
                    });
                    if let Some(assignment) = assignment {
                        if super::obstacles::park_review_access_tx(db, task, &review, &assignment, revision, found)? {
                            return Ok(());
                        }
                    }
                }
            }
            // An owner decision the owner already made is not asked again
            // (`decided_owner_item_tx`: the same item, also reworded, routed before
            // with an owner answer since, or a reason citing an answered question;
            // owner store 2026-11-21: the owner answered "close as is" ten times
            // while each closing review re-raised the same declined reads).
            // Only the owner closes an unreachable part as is: an owner decision the
            // owner already answered, or a review dead end the owner accepted as not
            // verified. Nothing else reaches verification from an INCOMPLETE review.
            let accepted = owner_accepted_review_tx(db, task)?;
            let mut reachable = !blocked.is_empty();
            let mut decided = Vec::new();
            let mut blocked = blocked;
            for b in &mut blocked {
                if accepted {
                    b["owner_accepted"] = json!(true);
                    continue;
                }
                let answer = if b["blocker"] == "owner_decision" {
                    decided_owner_item_tx(
                        db,
                        task,
                        b["item"].as_str().unwrap_or_default(),
                        b["reason"].as_str().unwrap_or_default(),
                    )?
                } else {
                    None
                };
                match answer {
                    Some(answer) => decided.push((b.clone(), answer)),
                    None => reachable = false,
                }
            }
            if !plan_review && result["verdict"] == "INCOMPLETE" && reachable {
                for (b, answer) in &decided {
                    let item = b["item"].as_str().unwrap_or_default();
                    event_tx(
                        db,
                        task,
                        "LOOP_BROKEN",
                        json!({"key":loop_key("owner_decision", &review, &dead_end_item(item)),
                            "kind":"owner_decision","node":review,"action":"disclosed_limit",
                            "answer":answer.chars().take(400).collect::<String>(),
                            "limits":[format!("Review did not read {}", item.chars().take(200).collect::<String>())]}),
                    )?;
                }
                return disclose_review_limits_tx(db, task, revision, &review, &result, &blocked);
            }
            // Parts only the reviewer's tools could not reach get one follow-up review
            // narrowed to them, on another route and with another approach. Parts the
            // owner already decided about are not reviewed again.
            let unreachable: Vec<Value> = blocked
                .iter()
                .filter(|b| !decided.iter().any(|(d, _)| d == *b))
                .cloned()
                .collect();
            if !plan_review
                && result["verdict"] == "INCOMPLETE"
                && !narrowed
                && !unreachable.is_empty()
                && unreachable.iter().all(|b| b["blocker"] == "tool_unavailable")
                && narrow_review_tx(db, task, revision, &review, &result, &unreachable)?
            {
                return Ok(());
            }
            domain::stop_tx(db, task, "review_incomplete")?;
            return Ok(());
        }
        Some("REQUEST_CHANGES") => {}
        _ => return refuse("review_verdict_required"),
    }
    // A fix and review round continues only on real progress. When the files did not
    // change since the previous review of this revision, or the review repeats the
    // same findings, another identical round would ping-pong: stop once instead and
    // let the run mode escalate (coordinator, senior consultant, owner).
    // A plan review sees unchanged project files by design; the plan itself changed
    // (an unchanged plan is refused at submission), so only repeated findings count.
    if let Some((reason, previous_revision)) = no_progress_tx(db, task, revision, &review, &result)?
        .filter(|(reason, _)| !(plan_review && *reason == "unchanged_files"))
    {
        let key = findings_key(&result);
        event_tx(
            db,
            task,
            "REVIEW_NO_PROGRESS",
            json!({"review":review,"revision":revision,"reason":reason,"report_ref":result["report_ref"],
                "previous_revision":previous_revision,"across_revisions":previous_revision!=revision,
                "key":key}),
        )?;
        if reason == "repeated_findings" {
            let count = 1 + lineage_reviews_tx(db, task, revision, &review)?
                .iter()
                .filter(|(_, v)| findings_key(v) == key)
                .count();
            event_tx(
                db,
                task,
                "LOOP_OBSERVED",
                json!({"key":key,"kind":"review_findings","node":review,"count":count}),
            )?;
        }
        domain::stop_tx(db, task, "review_no_progress")?;
        return Ok(());
    }
    if !plan_review {
        if let Some((rounds, previous, current)) =
            lineage_budget_exhausted_tx(db, task, revision, &review, &result)?
        {
            event_tx(
                db,
                task,
                "LOOP_OBSERVED",
                json!({"key":loop_key("review_cycle", "_review", ""),"kind":"review_cycle",
                    "node":review,"count":rounds,"previous_findings":previous,"findings":current}),
            )?;
            domain::stop_tx(db, task, "review_cycle_limit")?;
            return Ok(());
        }
    }
    let cycle: i64 = db.query_row(
        "SELECT review_cycle FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    // A large sprint plan converges over more rounds: past the usual budget its
    // review continues by itself while each round finds fewer problems than the
    // previous one (repeated findings already stopped above), up to a hard cap.
    let converging = plan_review
        && cycle < super::sprint_planning::MAX_PLAN_REVIEW_CYCLES
        && super::sprint_planning::fewer_findings_tx(db, task, &result)?;
    if cycle >= MAX_REVIEW_CYCLES as i64 && !converging {
        domain::stop_tx(db, task, "review_cycle_limit")?;
        return Ok(());
    }
    if plan_review {
        return super::sprint_planning::return_for_revision_tx(db, task, &review, &result);
    }
    let plan: Option<String> = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let plan: Value = serde_json::from_str(plan.as_deref().unwrap_or("{}"))?;
    let planned = plan["tasks"].as_array().map(|nodes| {
        nodes
            .iter()
            .filter_map(|n| n["id"].as_str().map(str::to_owned))
            .collect::<BTreeSet<_>>()
    });
    let rows = db
        .prepare(
            "SELECT node_id,role,scope_json,dependencies_json,assignment_ref FROM runtime_nodes
        WHERE task_id=? AND revision=? AND stage='execution' ORDER BY rowid",
        )?
        .query_map((task, revision), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let rows = rows
        .into_iter()
        .filter(|n| {
            planned.as_ref().map_or(!n.0.starts_with('_'), |p| {
                p.contains(&n.0) && n.0 != "_consultation"
            })
        })
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return refuse("execution_nodes_missing");
    }
    let findings = result["findings"].as_array();
    if findings.is_none_or(Vec::is_empty)
        && rows.len() > 1
        && domain::return_to_coordinator_tx(db, task, "review_changes_without_findings")?
    {
        event_tx(
            db,
            task,
            "REVIEW_CHANGES_REQUESTED",
            json!({"review":review,"fixes":[],"targeted":false,"fallback":"no_findings","returned":true}),
        )?;
        return Ok(());
    }
    let mut scopes = BTreeMap::new();
    let mut parents = BTreeMap::new();
    for (id, _, scope, deps, assignment) in &rows {
        scopes.insert(
            id.clone(),
            effective_scope_tx(db, task, assignment.as_deref(), scope)?,
        );
        parents.insert(id.clone(), strings(&serde_json::from_str::<Value>(deps)?)?);
    }
    let mut grouped: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    let mut fallback = if findings.is_none_or(Vec::is_empty) {
        Some("no_findings")
    } else {
        None
    };
    if let Some(findings) = findings {
        for f in findings {
            let node = f["node"].as_str().unwrap_or("");
            if f["cross_cutting"] != false {
                fallback = Some("cross_cutting");
                break;
            }
            let Some(scope) = scopes.get(node) else {
                fallback = Some("unknown_node");
                break;
            };
            let files = strings(&f["files"])?;
            if files.is_empty()
                || files.iter().any(|p| {
                    p.starts_with('/')
                        || p.split('/')
                            .any(|s| ["..", ".git", ".cortex", ".cortex-dev"].contains(&s))
                        || !scope.iter().any(|s| matches(p, s))
                })
            {
                fallback = Some("files_outside_scope");
                break;
            }
            grouped.entry(node.into()).or_default().push(f);
        }
    }
    let mut objectives = BTreeMap::new();
    if fallback.is_none() {
        for (node, items) in &grouped {
            let mut objective = format!(
                "Address these review findings within the assigned scope (review report: {}):\n",
                result["report_ref"].as_str().unwrap_or("")
            );
            for item in items {
                objective += &format!(
                    "- Files: {}\n  Fix: {}\n",
                    strings(&item["files"])?.join(", "),
                    item["fix"].as_str().unwrap_or("")
                );
            }
            if result["verdict"] == "INCOMPLETE" {
                let left = uncovered(&result);
                if !left.is_empty() {
                    objective.push_str("\nAfter this fix, recheck the following parts before publishing the next result:\n");
                    for item in left {
                        objective.push_str(&format!(
                            "- {} ({})\n",
                            item["item"].as_str().unwrap_or("unlisted part"),
                            item["reason"].as_str().unwrap_or("coverage gap")
                        ));
                    }
                }
            }
            if objective.chars().count() > 8000 {
                fallback = Some("findings_too_long");
                break;
            }
            objectives.insert(node.clone(), objective);
        }
    }
    let docs = |p: &String| p.starts_with("docs/") || p == "docs" || p.ends_with(".md");
    let tests = |p: &String| p.starts_with("tests/") || p == "tests";
    // The files the findings for one fix name: its own findings, or every finding
    // when the fix is not targeted.
    let named = |id: &str| -> CoreResult<(Vec<String>, BTreeSet<String>)> {
        let items: Vec<&Value> = match grouped.get(id) {
            Some(items) if fallback.is_none() => items.clone(),
            _ => findings.into_iter().flatten().collect(),
        };
        let mut files = Vec::new();
        let mut proposed = BTreeSet::new();
        for item in &items {
            files.extend(strings(&item["files"])?);
            // The reviewer's choice of specialist, kept only for a writing role.
            proposed.insert(
                item["role"]
                    .as_str()
                    .map(|r| format!("worker-{}", r.trim_start_matches("worker-")))
                    .filter(|r| !read_only(r, "execution"))
                    .unwrap_or_default(),
            );
        }
        Ok((files, proposed))
    };
    let mut fixes = Vec::new();
    for (id, role, _, _, _) in &rows {
        if fallback.is_none() && !grouped.contains_key(id) {
            continue;
        }
        let scope = &scopes[id];
        // A fix goes to the writing specialist every one of its findings proposes,
        // else to the specialist for the files they name (owner store 2026-11-21:
        // four documentation findings on a QA node were routed to QA, and the
        // coordinator had to replan them to a technical writer), else to the
        // node's own writing role.
        let (files, proposed) = named(id)?;
        let some = !files.is_empty();
        let unanimous = match proposed.len() {
            1 => proposed.first().filter(|r| !r.is_empty()).cloned(),
            _ => None,
        };
        let role = if let Some(chosen) = unanimous.as_deref() {
            chosen
        } else if some && files.iter().all(docs) {
            "worker-technical-writer"
        } else if some && files.iter().all(|p| test_file(p)) {
            "worker-qa-engineer"
        } else if some
            && (read_only(role, "execution")
                || ["worker-qa-engineer", "worker-technical-writer"].contains(&role.as_str()))
        {
            // Product code named on a testing or writing node goes to a developer.
            code_role(&files)
        } else if !read_only(role, "execution") {
            role.as_str()
        } else if scope.iter().all(docs) {
            "worker-technical-writer"
        } else if scope.iter().all(tests) {
            "worker-qa-engineer"
        } else {
            "worker-backend-dev"
        };
        let mut deps = vec![review.clone()];
        let mut ancestors = BTreeSet::new();
        let mut pending = parents.get(id).cloned().unwrap_or_default();
        while let Some(p) = pending.pop() {
            if ancestors.insert(p.clone()) {
                pending.extend(parents.get(&p).cloned().unwrap_or_default());
            }
        }
        for (parent, _, _, _, _) in &rows {
            let include = if fallback.is_some() {
                parents[id].contains(parent)
            } else {
                ancestors.contains(parent) && grouped.contains_key(parent)
            };
            if include {
                deps.push(format!("_fix_{cycle}_{parent}"));
            }
        }
        let objective = if fallback.is_some() {
            format!(
                "Address review findings within the assigned scope: {}",
                result["summary"].as_str().unwrap_or("")
            )
        } else {
            objectives[id].clone()
        };
        let fix = format!("_fix_{cycle}_{id}");
        db.execute("INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status)
            VALUES (?,?,?,?,?,?,?,'execution','pending')",(task,revision,&fix,role,objective,json!(deps).to_string(),json!(scope).to_string()))?;
        // The fix routes like the node it fixes (complexity, work kind, provider,
        // model and its reason), not with defaults.
        let inherited = super::plan::routing_fields(&super::plan::node_routing(&plan, id));
        super::plan::record_node_routing_tx(db, task, &fix, &inherited)?;
        fixes.push(fix);
    }
    event_tx(
        db,
        task,
        "REVIEW_CHANGES_REQUESTED",
        json!({"review":review,"fixes":fixes,"targeted":fallback.is_none(),"fallback":fallback}),
    )?;
    domain::state_tx(db, task, TaskState::Executing, None)
}
