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
    let d = e
        .project_digest
        .as_deref()
        .filter(|s| s.len() == 64)
        .ok_or(CoreError::Refused("review_revision_unavailable"))?;
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
    if bindings.iter().any(|b| b["refused_request_key"] == key) {
        return Ok(Some(
            json!({"outcome":"refused","error":"review_resubmit_required"}),
        ));
    }
    if bindings.last().unwrap()["revision_digest"] == d {
        return Ok(None);
    }
    if bindings.len() > MAX_REVIEW_REBINDS {
        return Ok(Some(
            json!({"outcome":"refused","error":"review_project_revision_unstable"}),
        ));
    }
    event_tx(
        db,
        task,
        "REVIEW_STARTED",
        json!({"assignment_ref":assignment,"revision_digest":d,
        "rebound_from":bindings.last().unwrap()["revision_digest"],"rebind":bindings.len(),"refused_request_key":key}),
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
/// A concrete finding with an owning execution node and an in-scope file can be
/// repaired immediately, even in an INCOMPLETE review; the fresh review after the
/// fix covers the new bytes.
fn actionable_findings(result: &Value) -> bool {
    let Some(findings) = result["findings"].as_array() else {
        return false;
    };
    !findings.is_empty()
        && findings.iter().all(|finding| {
            finding.is_object()
                && finding["cross_cutting"] == false
                && finding["node"]
                    .as_str()
                    .is_some_and(|node| !node.trim().is_empty())
                && finding["fix"]
                    .as_str()
                    .is_some_and(|fix| !fix.trim().is_empty())
                && finding["files"]
                    .as_array()
                    .is_some_and(|files| !files.is_empty())
        })
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
/// Whether this review shows no progress over the previous completed review of the
/// same revision (the one whose findings the last fixes addressed): the reviewed
/// bytes are unchanged, or the same findings came back. `None` when this is the
/// first review of the revision or something changed.
fn no_progress_tx(
    db: &Transaction<'_>,
    task: &str,
    revision: i64,
    review: &str,
    result: &Value,
) -> CoreResult<Option<&'static str>> {
    let previous: Option<String> = db
        .query_row(
            "SELECT result_json FROM runtime_nodes WHERE task_id=? AND revision=? AND stage='review'
             AND status='completed' AND node_id!=? AND result_json IS NOT NULL ORDER BY rowid DESC LIMIT 1",
            (task, revision, review),
            |r| r.get(0),
        )
        .optional()?;
    let Some(previous) = previous else {
        return Ok(None);
    };
    let previous: Value = serde_json::from_str(&previous)?;
    if !matches!(
        previous["verdict"].as_str(),
        Some("REQUEST_CHANGES" | "INCOMPLETE")
    ) {
        return Ok(None);
    }
    let digest = |v: &Value| {
        v["project_digest"]
            .as_str()
            .filter(|d| d.len() == 64)
            .map(str::to_owned)
    };
    if digest(result).is_some() && digest(result) == digest(&previous) {
        return Ok(Some("unchanged_files"));
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
        return Ok(Some("repeated_findings"));
    }
    Ok(None)
}
/// The parts of the change a review reports it did not cover, as given in its
/// `not_covered`.
fn uncovered(result: &Value) -> Vec<Value> {
    result["not_covered"]
        .as_array()
        .cloned()
        .unwrap_or_default()
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
    // The review of a submitted sprint plan (see `sprint_planning`): findings return the
    // plan to its coordinator for a revision within the same bounded rules.
    let plan_review = super::sprint_planning::is_plan_review(&review);
    match result["verdict"].as_str() {
        Some("APPROVE") => return domain::state_tx(db, task, TaskState::Verifying, None),
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
        // of the same bytes would meet the same dead end, so none is scheduled: the
        // stop goes the task's run-mode way (Automatic: coordinator, then the senior
        // consultant, then the owner; otherwise the owner's stop question). A legacy
        // result without blockers, or a BLOCK naming no problem, takes the same stop.
        Some("INCOMPLETE" | "BLOCK") => {
            let blocked: Vec<Value> = uncovered(&result)
                .into_iter()
                .map(|gap| json!({"item":gap["item"],"blocker":gap["blocker"],"reason":gap["reason"]}))
                .collect();
            event_tx(
                db,
                task,
                "REVIEW_DEAD_END_ROUTED",
                json!({"review":review,"revision":revision,"report_ref":result["report_ref"],
                    "verdict":result["verdict"],"blocked":blocked}),
            )?;
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
    if let Some(reason) = no_progress_tx(db, task, revision, &review, &result)?
        .filter(|reason| !(plan_review && *reason == "unchanged_files"))
    {
        event_tx(
            db,
            task,
            "REVIEW_NO_PROGRESS",
            json!({"review":review,"revision":revision,"reason":reason,"report_ref":result["report_ref"]}),
        )?;
        domain::stop_tx(db, task, "review_no_progress")?;
        return Ok(());
    }
    let cycle: i64 = db.query_row(
        "SELECT review_cycle FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    if cycle >= MAX_REVIEW_CYCLES as i64 {
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
    let mut fixes = Vec::new();
    for (id, role, _, _, _) in &rows {
        if fallback.is_none() && !grouped.contains_key(id) {
            continue;
        }
        let scope = &scopes[id];
        let role = if !read_only(role, "execution") {
            role.as_str()
        } else if scope
            .iter()
            .all(|p| p.starts_with("docs/") || p == "docs" || p.ends_with(".md"))
        {
            "worker-technical-writer"
        } else if scope
            .iter()
            .all(|p| p.starts_with("tests/") || p == "tests")
        {
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
