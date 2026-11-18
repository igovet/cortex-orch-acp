//! Runtime-only certification. Python oracle: orchestration/state.py:246-298,
//! 1861-1946,1993-2018,2104-2150,2244-2297. No model-provided pass flag
//! can complete a task. Observations must come from the daemon's effect ports.
use super::{
    domain, event_tx, plan, request_tx, waits, MAX_REVIEW_CYCLES, MAX_VERIFICATION_REPAIRS,
};
use crate::{
    error::refuse,
    ownership::{self, OwnerEpoch},
    ports::{CommandObservation, ProcessObservation},
    CoreError, CoreResult, TaskState,
};
use rusqlite::{OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// Non-deserializable effect snapshot, collected outside the SQLite transaction.
/// The host must use safe artifact reads and fresh project/process observations.
#[derive(Default)]
pub struct EvidenceSnapshot {
    pub project_digest: Option<String>,
    /// Metadata digest (type, mode, size, mtime, link target) of Git-ignored files.
    /// Absent when they could not be observed; a review waiver then cannot be granted.
    pub ignored_digest: Option<String>,
    pub head: Option<String>,
    /// A deliverable entry (a file past a safety bound, a special file) was recorded by
    /// size/mtime/mode only; restoring the mtime would defeat it, so no review waiver.
    /// Ignored files are not deliverable: only `ignored_digest` equality governs them.
    pub metadata_only: bool,
    /// A per-file or total content limit was reached during the observation.
    pub cap_reached: bool,
    /// The observation could not cover everything (e.g. a nested checkout); no waiver.
    pub incomplete: bool,
    pub artifacts: BTreeMap<String, String>,
    pub workers: BTreeMap<String, ProcessObservation>,
}
fn digest(snapshot: &EvidenceSnapshot) -> CoreResult<&str> {
    snapshot
        .project_digest
        .as_deref()
        .filter(|s| s.len() == 64 && s.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or(CoreError::Refused("project_revision_unavailable"))
}
/// Non-shell baseline of the retained evidence classifier. Inline shell
/// programs remain conservatively unverifiable until their status flow is
/// proved; a script file is an ordinary project check, not an inline program.
pub fn trivial_command(argv: &[String]) -> bool {
    fn base(s: &str) -> &str {
        s.rsplit('/').next().unwrap_or(s)
    }
    fn assignment(s: &str) -> bool {
        s.split_once('=').is_some_and(|(k, _)| {
            !k.is_empty()
                && k.bytes().enumerate().all(|(i, b)| {
                    b == b'_' || b.is_ascii_alphabetic() || i > 0 && b.is_ascii_digit()
                })
        })
    }
    let mut w = argv;
    loop {
        let Some(head) = w.first() else { return true };
        if assignment(head) {
            w = &w[1..];
            continue;
        }
        match base(head) {
            "env" => {
                w = &w[1..];
                while let Some(s) = w.first() {
                    if s.starts_with('-') || assignment(s) {
                        let n = if ["-u", "--unset", "-C", "--chdir"].contains(&s.as_str()) {
                            2
                        } else {
                            1
                        };
                        w = w.get(n..).unwrap_or(&[]);
                    } else {
                        break;
                    }
                }
            }
            "timeout" | "nice" => {
                let timeout = base(head) == "timeout";
                w = &w[1..];
                while let Some(s) = w.first().filter(|s| s.starts_with('-')) {
                    let n = if ["-s", "--signal", "-k", "--kill-after", "-n", "--adjustment"]
                        .contains(&s.as_str())
                    {
                        2
                    } else {
                        1
                    };
                    w = w.get(n..).unwrap_or(&[]);
                }
                if timeout {
                    w = w.get(1..).unwrap_or(&[]);
                }
            }
            "nohup" | "command" | "exec" | "time" => {
                w = &w[1..];
                while w.first().is_some_and(|s| s.starts_with('-')) {
                    w = &w[1..];
                }
            }
            _ => break,
        }
    }
    let Some(head) = w.first() else { return true };
    let program = base(head);
    if [
        ":", "[", "ag", "basename", "cat", "cd", "date", "df", "dirname", "du", "echo", "env",
        "export", "fd", "file", "find", "grep", "head", "hostname", "id", "less", "ls", "more",
        "printenv", "printf", "pwd", "readlink", "realpath", "rg", "set", "sleep", "sort", "stat",
        "tail", "tee", "test", "tr", "tree", "true", "type", "uname", "uniq", "wc", "which",
        "whoami", "yes", "exit", "return", "",
    ]
    .contains(&program)
    {
        return true;
    }
    if program == "git" {
        let mut rest = &w[1..];
        while let Some(s) = rest.first().filter(|s| s.starts_with('-')) {
            let n = if ["-C", "-c", "--git-dir", "--work-tree"].contains(&s.as_str()) {
                2
            } else {
                1
            };
            rest = rest.get(n..).unwrap_or(&[]);
        }
        return match rest.first().map(String::as_str) {
            None => true,
            Some("diff") => !rest
                .iter()
                .any(|s| ["--check", "--exit-code", "--quiet"].contains(&s.as_str())),
            Some(s) => [
                "branch",
                "config",
                "describe",
                "log",
                "ls-files",
                "remote",
                "rev-parse",
                "show",
                "status",
            ]
            .contains(&s),
        };
    }
    if ["sh", "bash", "dash", "zsh", "ksh"].contains(&program) {
        for word in &w[1..] {
            if word.starts_with('-') && !word.starts_with("--") {
                if word[1..].contains('c') {
                    return true;
                }
            } else if !word.starts_with("--") && !word.starts_with('+') {
                break;
            }
        }
    }
    false
}
#[cfg(test)]
mod evidence_policy_tests {
    use super::*;
    #[test]
    fn wrappers_searches_git_and_script_files_classify_by_real_command() {
        for argv in [
            vec!["/usr/bin/rg", "needle", "src"],
            vec![
                "env", "-u", "SECRET", "A=1", "timeout", "-k", "1", "30", "nice", "-n", "2", "ls",
            ],
            vec!["git", "-C", "project", "status"],
            vec!["git", "diff", "--stat"],
            vec!["sh", "-c", "pytest; echo masked"],
            vec!["sh", "-c", "exit 0"],
            vec!["printf", "passed"],
        ] {
            assert!(
                trivial_command(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
                "{argv:?}"
            );
        }
        for argv in [
            vec!["timeout", "180", "python3", "-m", "pytest"],
            vec!["env", "A=1", "cargo", "test"],
            vec!["git", "-c", "core.fsmonitor=false", "diff", "--check"],
            vec!["/bin/sh", "scripts/check.sh"],
            vec!["npm", "run", "test"],
        ] {
            assert!(
                !trivial_command(&argv.iter().map(|s| s.to_string()).collect::<Vec<_>>()),
                "{argv:?}"
            );
        }
    }
}
fn latest(db: &Transaction<'_>, task: &str, kind: &str) -> CoreResult<Option<(i64, Value)>> {
    let row:Option<(i64,String)>=db.query_row("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type=? ORDER BY id DESC LIMIT 1",
        (task,kind),|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    row.map(|(id, s)| Ok((id, serde_json::from_str(&s)?)))
        .transpose()
}
pub fn execution_baseline_tx(
    db: &Transaction<'_>,
    task: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    if state != TaskState::PlanReady {
        return refuse("execution_not_expected");
    }
    let d = digest(snapshot)?;
    event_tx(
        db,
        task,
        "EXECUTION_BASELINE",
        json!({"revision":revision,"kind":"observed","revision_digest":d,"head":snapshot.head,
            "ignored_digest":snapshot.ignored_digest,
            "metadata_only":snapshot.metadata_only,"cap_reached":snapshot.cap_reached,"incomplete":snapshot.incomplete}),
    )?;
    domain::state_tx(db, task, TaskState::Executing, None)
}
/// First baseline since reopen, not latest baseline, prevents a replan hiding
/// earlier unreviewed work. A fix cycle that already has a reviewer is re-reviewed.
pub fn add_review_tx(
    db: &Transaction<'_>,
    task: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    if state != TaskState::Executing {
        return refuse("review_not_expected");
    }
    let nodes=db.prepare("SELECT node_id,stage,status FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid")?
        .query_map((task,revision),|r|Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?
        .collect::<Result<Vec<_>,_>>()?;
    if nodes.is_empty() || nodes.iter().any(|n| n.2 != "completed") {
        return refuse("execution_incomplete");
    }
    let observed = digest(snapshot).ok();
    let mut waiver = None;
    // Metadata-only or incomplete observations can be forged (mtime restored), so they
    // never support a waiver; the review then runs.
    let waivable = !snapshot.metadata_only && !snapshot.incomplete;
    if waivable && !nodes.iter().any(|n| n.1 == "review") {
        if let Some(d) = observed {
            let baseline:Option<String>=db.query_row("SELECT payload_json FROM runtime_events WHERE task_id=? AND type='EXECUTION_BASELINE'
                AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=? AND type='TASK_REOPENED'),0) ORDER BY id LIMIT 1",
                (task,task),|r|r.get(0)).optional()?;
            if let Some(s) = baseline {
                let b: Value = serde_json::from_str(&s)?;
                if b["kind"] == "observed"
                    && b["metadata_only"] != true
                    && b["incomplete"] != true
                    && b["revision_digest"] == d
                    && b["head"] == json!(snapshot.head)
                    && snapshot.ignored_digest.is_some()
                    && b["ignored_digest"] == json!(snapshot.ignored_digest)
                {
                    waiver = Some(
                        json!({"reason":"no_project_changes","reference":"execution_baseline","reference_revision":b["revision"]}),
                    );
                }
            }
            if waiver.is_none() {
                let approved:Option<i64>=db.query_row("SELECT revision FROM runtime_nodes WHERE task_id=? AND stage='review'
                    AND status='completed' AND role='worker-code-reviewer' AND json_extract(result_json,'$.verdict')='APPROVE'
                    AND json_extract(result_json,'$.project_digest')=? AND json_extract(result_json,'$.ignored_digest')=?
                    ORDER BY rowid DESC LIMIT 1",(task,d,snapshot.ignored_digest.as_deref().unwrap_or("")),|r|r.get(0)).optional()?;
                if let Some(r) = approved {
                    waiver = Some(
                        json!({"reason":"no_changes_since_approved_review","reference":"approved_review","reference_revision":r}),
                    );
                }
            }
        }
    }
    if let Some(mut w) = waiver {
        w["revision"] = json!(revision);
        w["revision_digest"] = json!(observed);
        w["head"] = json!(snapshot.head);
        event_tx(db, task, "REVIEW_NOT_REQUIRED", w)?;
        return domain::state_tx(db, task, TaskState::Verifying, None);
    }
    let cycle: i64 = db.query_row(
        "SELECT review_cycle+1 FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    if cycle > MAX_REVIEW_CYCLES as i64 {
        domain::stop_tx(db, task, "review_cycle_limit")?;
        return Ok(());
    }
    db.execute("INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status)
        VALUES (?,?,?,'worker-code-reviewer','Independently review the objective, implementation and retained reports.',?,'[\"**\"]','review','pending')",
        (task,revision,format!("_review_{cycle}"),json!(nodes.iter().map(|n|&n.0).collect::<Vec<_>>()).to_string()))?;
    // The reviewer routes as review work at the highest complexity of the work it
    // reviews; it names no provider or model (independence is chosen by routing).
    let accepted: Option<String> = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let accepted: Value = serde_json::from_str(accepted.as_deref().unwrap_or("{}"))?;
    let rank = |c: &str| ["low", "normal", "high"].iter().position(|v| *v == c);
    let complexity = nodes
        .iter()
        .filter(|n| n.1 == "execution")
        .filter_map(|n| {
            let routing = plan::node_routing(&accepted, &n.0);
            routing["complexity"].as_str().and_then(rank)
        })
        .max()
        .map_or("normal", |i| ["low", "normal", "high"][i]);
    plan::record_node_routing_tx(
        db,
        task,
        &format!("_review_{cycle}"),
        &json!({"work_kind":"review","complexity":complexity}),
    )?;
    db.execute(
        "UPDATE runtime_tasks SET review_cycle=? WHERE task_id=?",
        (cycle, task),
    )?;
    domain::state_tx(db, task, TaskState::Reviewing, None)
}
pub fn start_review_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<()> {
    if domain::task_tx(db, task)?.0 != TaskState::Reviewing {
        return refuse("review_not_expected");
    }
    let current: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
        WHERE n.task_id=? AND n.assignment_ref=? AND n.revision=t.revision AND n.stage='review'
        AND n.role='worker-code-reviewer' AND n.status='running')",
        (task, assignment),
        |r| r.get(0),
    )?;
    if !current {
        return refuse("stale_assignment_result");
    }
    let bound: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED'
        AND json_extract(payload_json,'$.assignment_ref')=?)",
        (task, assignment),
        |r| r.get(0),
    )?;
    if bound {
        return refuse("review_already_bound");
    }
    event_tx(
        db,
        task,
        "REVIEW_STARTED",
        json!({"assignment_ref":assignment,"revision_digest":digest(snapshot)?}),
    )?;
    Ok(())
}
/// Result publication/ownership is checked in the transaction. A report is not
/// acceptance, and finishing a node never releases an engine reservation.
enum Independence {
    Independent(Value),
    Unverified(Value),
    Warning(Value),
    Refused,
}
fn review_independence_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: &str,
    revision: i64,
) -> CoreResult<Independence> {
    let route = |sql: &str, params: &[&dyn rusqlite::ToSql]| -> CoreResult<Vec<Value>> {
        Ok(db
            .prepare(sql)?
            .query_map(params, |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
            .iter()
            .filter_map(|s| serde_json::from_str(s).ok())
            .collect())
    };
    let own = route(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_ROUTE_SELECTED'
        AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
        &[&task, &assignment],
    )?;
    let reviewed = route(
        "SELECT e.payload_json FROM runtime_events e WHERE e.task_id=? AND e.type='NATIVE_ROUTE_SELECTED'
        AND json_extract(e.payload_json,'$.revision')=? AND json_extract(e.payload_json,'$.node') IN
        (SELECT node_id FROM runtime_nodes WHERE task_id=e.task_id AND revision=? AND stage='execution') ORDER BY e.id",
        &[&task, &revision, &revision],
    )?;
    let (Some(own), false) = (own.first(), reviewed.is_empty()) else {
        return Ok(Independence::Unverified(
            json!({"status":"unverified","reason":"route_not_recorded"}),
        ));
    };
    let model = own["model"].as_str().unwrap_or("");
    let models: BTreeSet<&str> = reviewed
        .iter()
        .filter_map(|r| r["model"].as_str())
        .collect();
    if model.is_empty() || models.is_empty() {
        return Ok(Independence::Unverified(
            json!({"status":"unverified","reason":"model_not_recorded"}),
        ));
    }
    let engines: BTreeSet<&str> = reviewed
        .iter()
        .filter_map(|r| r["route_engine"].as_str())
        .collect();
    let same_engine = own["route_engine"]
        .as_str()
        .is_some_and(|e| engines.contains(e));
    if !models.contains(model) {
        return Ok(Independence::Independent(
            json!({"status":"independent","model":model,"same_engine_as_reviewed":same_engine}),
        ));
    }
    let warning = &own["decision"]["independence"];
    if warning["status"] == "warning" {
        let mut facts = json!({"status":"warning","model":model,
            "same_engine_as_reviewed":same_engine,"warning":warning["warning"]});
        if warning["mode"].is_string() {
            facts["mode"] = warning["mode"].clone();
        }
        return Ok(Independence::Warning(facts));
    }
    Ok(Independence::Refused)
}
pub fn record_result_tx(
    db: &Transaction<'_>,
    task: &str,
    assignment: &str,
    result: &Value,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    if !matches!(state, TaskState::Executing | TaskState::Reviewing) {
        return refuse("stale_assignment_result");
    }
    waits::check_mutation_tx(db, task, assignment)?;
    let row:Option<(i64,String,String,Option<String>)>=db.query_row("SELECT revision,stage,status,result_json FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
        (task,assignment),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
    let (rev, stage, status, old) = row.ok_or(CoreError::Refused("stale_assignment_result"))?;
    if rev != revision {
        return refuse("stale_assignment_result");
    }
    if let Some(old) = old {
        let mut old: Value = serde_json::from_str(&old)?;
        if let Some(o) = old.as_object_mut() {
            o.remove("report_digest");
            o.remove("project_digest");
            o.remove("ignored_digest");
        }
        return if old == *result {
            Ok(())
        } else {
            refuse("conflicting_assignment_result")
        };
    }
    if status != "running" {
        return refuse("assignment_not_running");
    }
    let report = result["report_ref"]
        .as_str()
        .ok_or(CoreError::Refused("assignment_report_missing"))?;
    let attempt: Option<Option<String>> = db
        .query_row(
            "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
            (task, assignment),
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    let attempt = attempt.ok_or(CoreError::Refused("assignment_report_missing"))?;
    let published:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM reports r JOIN task_events e ON e.task_id=r.task_id
        WHERE r.task_id=? AND r.id=? AND e.assignment_ref IN (?,?) AND e.event_type='worker_result' AND e.superseded=0
        AND json_extract(e.payload_json,'$.report_ref')=r.id)",(task,report,assignment,&attempt),|r|r.get(0))?;
    if !published {
        return refuse("assignment_report_mismatch");
    }
    let artifact = snapshot
        .artifacts
        .get(report)
        .filter(|s| s.len() == 64)
        .ok_or(CoreError::Refused("assignment_report_missing"))?;
    let mut result = result.clone();
    if !result.is_object() {
        return refuse("invalid_assignment_result");
    }
    result["report_digest"] = json!(artifact);
    if let Some(digest) = &snapshot.project_digest {
        result["project_digest"] = json!(digest);
    }
    if let Some(digest) = &snapshot.ignored_digest {
        result["ignored_digest"] = json!(digest);
    }
    // A reviewer that requests changes may cite a failing check as the finding;
    // that is a completed review that starts a fix cycle, not a failed reviewer.
    // Only an explicit assignment_outcome=failed marks the review itself failed.
    // The same holds for BLOCK and INCOMPLETE: the verdict, not the cited check, routes it.
    let requests_changes = stage == "review"
        && ["REQUEST_CHANGES", "BLOCK", "INCOMPLETE"]
            .contains(&result["verdict"].as_str().unwrap_or(""));
    let failed = result["assignment_outcome"] == "failed"
        || (!requests_changes
            && result["checks"].as_array().is_some_and(|c| {
                c.iter()
                    .any(|c| c["outcome"] == "failed" && !super::worker_checks::resolved(c))
            }));
    // Passed worker checks are not promoted into final verification evidence.
    if stage == "review" {
        if !["APPROVE", "REQUEST_CHANGES", "BLOCK", "INCOMPLETE"]
            .contains(&result["verdict"].as_str().unwrap_or(""))
        {
            return refuse("review_verdict_required");
        }
        // An incomplete review names what it did not cover; only that verdict does.
        let uncovered = result["not_covered"]
            .as_array()
            .is_some_and(|a| !a.is_empty());
        if (result["verdict"] == "INCOMPLETE") != uncovered {
            return refuse("review_not_covered_required");
        }
        let start: String = db
            .query_row(
                "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED'
            AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
                (task, assignment),
                |r| r.get(0),
            )
            .optional()?
            .ok_or(CoreError::Refused("review_revision_unavailable"))?;
        let start: Value = serde_json::from_str(&start)?;
        if start["assignment_ref"] != assignment || start["revision_digest"] != digest(snapshot)? {
            return refuse("review_resubmit_required");
        }
        // Independence rests on the recorded routes: a reviewer on the model of the
        // work it judges is refused unless routing found no other model and said so.
        // A reviewer is never the attempt that executed work (hard guard); routing
        // evidence adds model independence on top of it.
        let same_attempt: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM delegation_provisioning p JOIN runtime_nodes n
            ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref
            WHERE p.task_id=? AND n.stage='execution' AND p.attempt_id=?)",
            (task, &attempt),
            |r| r.get(0),
        )?;
        if same_attempt {
            return refuse("review_not_independent");
        }
        match review_independence_tx(db, task, assignment, revision)? {
            Independence::Independent(facts) => {
                result["review_independence"] = facts;
            }
            // Independence that cannot be established is shown like a known
            // violation, never as a silent field.
            Independence::Warning(facts) | Independence::Unverified(facts) => {
                event_tx(
                    db,
                    task,
                    "REVIEW_INDEPENDENCE_WARNING",
                    json!({"assignment_ref":assignment,"independence":facts}),
                )?;
                result["review_independence"] = facts;
            }
            Independence::Refused => return refuse("review_not_independent"),
        }
        result["project_digest"] = json!(digest(snapshot)?);
    }
    db.execute(
        "UPDATE runtime_nodes SET status=?,result_json=? WHERE task_id=? AND assignment_ref=?",
        (
            if failed { "failed" } else { "completed" },
            result.to_string(),
            task,
            assignment,
        ),
    )?;
    domain::revoke_tx(db, task, Some(assignment))?;
    domain::graph_attempt_tx(
        db,
        task,
        assignment,
        if failed { "failed" } else { "reported" },
        if failed { Some("worker_failed") } else { None },
    )?;
    event_tx(
        db,
        task,
        if failed {
            "WORKER_FAILED"
        } else {
            "WORKER_COMPLETED"
        },
        json!({"assignment_ref":assignment,"attempt_id":attempt,"result":result}),
    )?;
    Ok(())
}
fn review_binding_tx(db: &Transaction<'_>, task: &str) -> CoreResult<(i64, String)> {
    let (_, revision) = domain::task_tx(db, task)?;
    let review: Option<(String, Option<String>)> = db
        .query_row(
            "SELECT status,result_json FROM runtime_nodes
        WHERE task_id=? AND revision=? AND stage='review' ORDER BY rowid DESC LIMIT 1",
            (task, revision),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    if let Some((status, result)) = review {
        let v: Value = serde_json::from_str(&result.unwrap_or_default())
            .map_err(|_| CoreError::Refused("completion_review_missing"))?;
        if status != "completed" || v["verdict"] != "APPROVE" {
            return refuse("completion_review_missing");
        }
        // The approval's WORKER_COMPLETED event must follow its REVIEW_STARTED.
        let event:Option<(i64,String)>=db.query_row("SELECT e.id,e.payload_json FROM runtime_events e
            WHERE e.task_id=? AND e.type='WORKER_COMPLETED' AND json_extract(e.payload_json,'$.result.report_ref')=?
            ORDER BY e.id DESC LIMIT 1",(task,v["report_ref"].as_str()),|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        let (id, _) = event.ok_or(CoreError::Refused("completion_review_missing"))?;
        return Ok((
            id,
            v["project_digest"]
                .as_str()
                .ok_or(CoreError::Refused("completion_review_missing"))?
                .into(),
        ));
    }
    let (id, v) = latest(db, task, "REVIEW_NOT_REQUIRED")?
        .ok_or(CoreError::Refused("completion_review_missing"))?;
    if v["revision"] != revision {
        return refuse("completion_review_missing");
    }
    Ok((
        id,
        v["revision_digest"]
            .as_str()
            .ok_or(CoreError::Refused("completion_review_missing"))?
            .into(),
    ))
}
/// Host preflight before any command effect; retain_check repeats these gates.
pub fn verification_preflight_tx(
    db: &Transaction<'_>,
    task: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<()> {
    if domain::task_tx(db, task)?.0 != TaskState::Verifying {
        return refuse("verification_not_expected");
    }
    let (_, reviewed) = review_binding_tx(db, task)?;
    if reviewed != digest(snapshot)? {
        return refuse("verification_review_revision_changed");
    }
    let active: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND kind='worker' AND state!='released')", [task], |r| r.get(0))?;
    if active {
        return refuse("workers_still_live_or_unknown");
    }
    Ok(())
}
/// Runtime CommandExecutor returns this observation; no model JSON is accepted.
/// A receipt belongs to exactly one argv, revision and observed digest.
pub fn retain_check_tx(
    db: &Transaction<'_>,
    task: &str,
    owner: &OwnerEpoch,
    key: &str,
    argv: &[String],
    observed: &CommandObservation,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<Value> {
    ownership::check_tx(db, task, owner)?;
    let (state, revision) = domain::task_tx(db, task)?;
    if state != TaskState::Verifying || observed.receipt_ref.is_empty() || argv.is_empty() {
        return refuse("verification_not_expected");
    }
    let (review_event, reviewed) = review_binding_tx(db, task)?;
    if reviewed != digest(snapshot)? {
        return refuse("verification_review_revision_changed");
    }
    let value = json!({"argv":argv,"complete":observed.complete,"exit_code":observed.exit_code,
        "receipt_ref":observed.receipt_ref,"revision":revision,"revision_digest":reviewed,"owner_epoch":owner.epoch,"review_event":review_event});
    request_tx(
        db,
        task,
        "runtime-verifier",
        key,
        "verification_command",
        &value,
        || Ok(value.clone()),
    )
}
pub fn record_verification_tx(
    db: &Transaction<'_>,
    task: &str,
    receipt_keys: &[String],
    snapshot: &EvidenceSnapshot,
) -> CoreResult<bool> {
    let (state, revision) = domain::task_tx(db, task)?;
    if state != TaskState::Verifying {
        return refuse("verification_not_expected");
    }
    let (review_event, reviewed) = review_binding_tx(db, task)?;
    if reviewed != digest(snapshot)? {
        return refuse("verification_review_revision_changed");
    }
    let plan: String = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let plan: Value = serde_json::from_str(&plan)?;
    let commands = plan["verification"]
        .as_array()
        .ok_or(CoreError::Refused("verification_commands_missing"))?;
    if commands.is_empty() {
        return refuse("verification_commands_missing");
    }
    let mut checks = Vec::new();
    let mut ids = BTreeSet::new();
    let mut passed = commands.len() == receipt_keys.len();
    for (index, key) in receipt_keys.iter().enumerate() {
        let s:Option<String>=db.query_row("SELECT result_json FROM runtime_requests WHERE task_id=? AND actor='runtime-verifier'
            AND request_key=? AND operation='verification_command'",(task,key),|r|r.get(0)).optional()?;
        let v: Value =
            serde_json::from_str(&s.ok_or(CoreError::Refused("verification_receipt_missing"))?)?;
        let receipt = v["receipt_ref"]
            .as_str()
            .ok_or(CoreError::Refused("verification_receipt_missing"))?;
        if !ids.insert(receipt.to_owned()) {
            return refuse("verification_receipt_replayed");
        }
        let reused: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_events e, json_each(e.payload_json,'$.checks') c
            WHERE e.task_id=? AND e.type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED')
            AND json_extract(c.value,'$.receipt_ref')=?)",
            (task, receipt),
            |r| r.get(0),
        )?;
        if reused {
            return refuse("verification_receipt_replayed");
        }
        let epoch: i64 = db.query_row(
            "SELECT epoch FROM task_owners WHERE task_id=?",
            [task],
            |r| r.get(0),
        )?;
        passed &= v["complete"] == true
            && v["exit_code"] == 0
            && v["revision"] == revision
            && v["revision_digest"] == reviewed
            && v["review_event"] == review_event
            && v["owner_epoch"] == epoch
            && commands.get(index) == Some(&v["argv"]);
        checks.push(v);
    }
    event_tx(
        db,
        task,
        if passed {
            "VERIFICATION_PASSED"
        } else {
            "VERIFICATION_FAILED"
        },
        json!({"revision":revision,"revision_digest":reviewed,"review_event":review_event,"checks":checks}),
    )?;
    if passed {
        domain::state_tx(db, task, TaskState::Reconciling, None)?;
    } else {
        let failures: i64 = db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='VERIFICATION_FAILED'",
            [task],
            |r| r.get(0),
        )?;
        if failures <= MAX_VERIFICATION_REPAIRS as i64 {
            domain::state_tx(db, task, TaskState::Planning, None)?;
        } else {
            domain::stop_tx(db, task, "verification_failed")?;
        }
    }
    Ok(passed)
}
pub fn submit_answer_tx(db: &Transaction<'_>, task: &str, text: &str) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    let informational:bool=db.query_row("SELECT plan_json IS NULL
        AND NOT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=?)
        AND NOT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator')
        FROM runtime_tasks WHERE task_id=?",(task,task,task),|r|r.get(0))?;
    if state != TaskState::Planning || revision != 0 || !informational || text.trim().is_empty() {
        return refuse("direct_answer_not_allowed");
    }
    db.execute(
        "UPDATE runtime_tasks SET synthesis=? WHERE task_id=?",
        (text, task),
    )?;
    event_tx(db, task, "ANSWER_SUBMITTED", json!({"text":text}))?;
    domain::state_tx(db, task, TaskState::AnswerReady, None)
}
/// Retain dropped failed commands until an exact successful rerun, or a real
/// answer to a task-global question asked after the removal. Worker-local answers
/// reset retry budgets but cannot certify these obligations (state.py:137-165).
pub fn dropped_failures_tx(db: &Transaction<'_>, task: &str) -> CoreResult<Vec<Value>> {
    let rows=db.prepare("SELECT id,type,payload_json FROM runtime_events WHERE task_id=? AND id>coalesce(
        (SELECT max(id) FROM runtime_events WHERE task_id=? AND type='PLAN_ACCEPTED'),0)
        AND type IN ('VERIFICATION_REVISED','VERIFICATION_PASSED','USER_QUESTION_REQUESTED','USER_ANSWER_RECEIVED') ORDER BY id")?
        .query_map((task,task),|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
    let mut outstanding: BTreeMap<String, (Value, i64)> = BTreeMap::new();
    let mut questions = BTreeMap::new();
    for (id, kind, s) in rows {
        let v: Value = serde_json::from_str(&s)?;
        match kind.as_str() {
            "VERIFICATION_REVISED" => {
                if let Some(checks) = v["dropped_failed"].as_array() {
                    for argv in checks {
                        outstanding.insert(argv.to_string(), (argv.clone(), id));
                    }
                }
            }
            "VERIFICATION_PASSED" => {
                if let Some(checks) = v["checks"].as_array() {
                    for c in checks {
                        if c["complete"] == true && c["exit_code"] == 0 {
                            outstanding.remove(&c["argv"].to_string());
                        }
                    }
                }
            }
            "USER_QUESTION_REQUESTED" => {
                questions.insert(v["question_id"].to_string(), id);
            }
            "USER_ANSWER_RECEIVED" => {
                let asked = questions
                    .get(&v["question_id"].to_string())
                    .copied()
                    .unwrap_or(0);
                outstanding.retain(|_, (_, removed)| *removed >= asked);
            }
            _ => {}
        }
    }
    Ok(outstanding.into_values().map(|(v, _)| v).collect())
}
/// Runtime alone calls this after reconciling processes and safe artifact reads.
pub fn complete_tx(
    db: &Transaction<'_>,
    task: &str,
    snapshot: &EvidenceSnapshot,
) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    if !matches!(state, TaskState::Reconciling | TaskState::AnswerReady) {
        return refuse("completion_not_expected");
    }
    let (blocker, synthesis): (Option<String>, Option<String>) = db.query_row(
        "SELECT blocker,synthesis FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if blocker.is_some() || synthesis.as_deref().is_none_or(|s| s.trim().is_empty()) {
        return refuse("completion_synthesis_or_blocker");
    }
    let stopped: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_stopped WHERE task_id=?)",
        [task],
        |r| r.get(0),
    )?;
    if stopped {
        return refuse("completion_task_stopped");
    }
    let pending:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM pending_questions WHERE task_id=? AND status='open')
        OR EXISTS(SELECT 1 FROM task_events WHERE task_id=? AND event_type IN ('user_message','coordinator_clarification') AND consumed_by IS NULL AND superseded=0)",
        (task,task),|r|r.get(0))?;
    // An owner message still waiting for the coordinator is never completed past.
    if pending
        || !super::steering::pending(db, task)?.is_empty()
        || !super::steering::undelivered(db, task)?.is_empty()
        || waits::projection_tx(db, task)?
            .iter()
            .any(|w| ["open", "ready", "resuming"].contains(&w["status"].as_str().unwrap_or("")))
    {
        return refuse("completion_pending_input");
    }
    let attempts=db.prepare("SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator'")?
        .query_map([task],|r|r.get::<_,Option<String>>(0))?.collect::<Result<Vec<_>,_>>()?;
    for a in attempts {
        if a.as_ref().and_then(|a| snapshot.workers.get(a))
            != Some(&ProcessObservation::ConfirmedExited)
        {
            return refuse("workers_still_live_or_unknown");
        }
    }
    let capacity:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND kind='worker' AND state!='released')",
        [task],|r|r.get(0))?;
    if capacity {
        return refuse("workers_still_live_or_unknown");
    }
    let informational:bool=db.query_row("SELECT revision=0 AND plan_json IS NULL AND NOT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=?)\n            AND NOT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator') FROM runtime_tasks WHERE task_id=?",(task,task,task),|r|r.get(0))?;
    if state == TaskState::AnswerReady && informational {
        let informational:bool=db.query_row("SELECT plan_json IS NULL AND NOT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=?)
            AND NOT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator')
            FROM runtime_tasks WHERE task_id=?",(task,task,task),|r|r.get(0))?;
        if revision != 0 || !informational {
            return refuse("completion_answer_invariant");
        }
        let answer = latest(db, task, "ANSWER_SUBMITTED")?
            .ok_or(CoreError::Refused("completion_answer_missing"))?
            .1;
        if answer["text"] != json!(synthesis) {
            return refuse("completion_answer_missing");
        }
    } else {
        let nodes=db.prepare("SELECT status,result_json,assignment_ref FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid")?
            .query_map((task,revision),|r|Ok((r.get::<_,String>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?)))?.collect::<Result<Vec<_>,_>>()?;
        if nodes.is_empty()
            || nodes
                .iter()
                .any(|(s, r, _)| s != "completed" || r.is_none())
        {
            return refuse("completion_nodes_incomplete");
        }
        if state == TaskState::AnswerReady {
            if !super::reconciliation::direct_answer_allowed(db, task)? {
                return refuse("completion_answer_invariant");
            }
            let answer = latest(db, task, "ANSWER_SUBMITTED")?
                .ok_or(CoreError::Refused("completion_answer_missing"))?
                .1;
            if answer["text"] != json!(synthesis) {
                return refuse("completion_answer_missing");
            }
        }
        for (_, r, assignment) in nodes {
            let v: Value = serde_json::from_str(&r.unwrap())?;
            if let Some(checks) = v["checks"].as_array() {
                let channel: Option<String> = db.query_row(
                    "SELECT c.channel_id FROM trusted_channels c JOIN delegation_provisioning p ON p.task_id=c.task_id AND p.attempt_id=c.attempt_id WHERE c.task_id=? AND p.idempotency_key=? ORDER BY c.rowid DESC LIMIT 1",
                    (task,assignment.as_deref()), |r|r.get(0)).optional()?;
                let mut runs = match channel {
                    Some(channel) => super::worker_checks::load_runs(db, task, &channel)?,
                    None => Vec::new(),
                };
                super::worker_checks::observe_outputs(db, task, &mut runs, &snapshot.artifacts)?;
                let current =
                    super::worker_checks::evaluate(checks, &runs, v["project_digest"].as_str())?;
                for (check, now) in checks.iter().zip(&current) {
                    if super::worker_checks::resolved(check) {
                        if !super::worker_checks::resolved(now)
                            || ["closed_by", "retracted_by", "superseded_by"]
                                .iter()
                                .any(|k| check[*k] != now[*k])
                        {
                            return refuse("completion_unresolved_worker_check");
                        }
                        continue;
                    }
                    if matches!(check["outcome"].as_str(), Some("not_run" | "unverified")) {
                        if !super::reconciliation::disclosed_gap(db, task, check)? {
                            return refuse("completion_unresolved_worker_check");
                        }
                        continue;
                    }
                    if check["outcome"] != "passed" {
                        return refuse("completion_unresolved_worker_check");
                    }
                    if now["outcome"] != "passed" {
                        return refuse("completion_worker_check_unverified");
                    }
                    let reference = check["tool_call_ref"]
                        .as_str()
                        .ok_or(CoreError::Refused("completion_worker_check_unverified"))?;
                    let receipt: Option<String> = db.query_row("SELECT CASE WHEN r.operation='outside_command' THEN json_extract(r.result_json,'$.reply.structuredContent') ELSE r.result_json END FROM runtime_requests r
                        JOIN trusted_channels c ON c.channel_id=r.actor
                        JOIN delegation_provisioning p ON p.task_id=c.task_id AND p.attempt_id=c.attempt_id
                        WHERE r.task_id=? AND p.idempotency_key=? AND r.operation IN ('command_run','outside_command')
                        AND coalesce(json_extract(r.result_json,'$.tool_call_ref'),json_extract(r.result_json,'$.reply.structuredContent.tool_call_ref'))=?",
                        (task, assignment.as_deref(), reference), |r| r.get(0)).optional()?;
                    let receipt: Value = serde_json::from_str(
                        &receipt.ok_or(CoreError::Refused("completion_worker_check_unverified"))?,
                    )?;
                    // A worker's `checks[].command` is descriptive text, never proof: the
                    // cited receipt's own argv is the run. Submission accepts the check on
                    // that receipt alone, so completion never refuses a verified pass because
                    // the description differs (for example an abbreviated `python3 -c "..."`);
                    // older stores retain such text in their node results.
                    if receipt["complete"] != true
                        || receipt["exit_code"] != 0
                        || !receipt["failure"].is_null()
                        || (check["evidence_strength"] == "verified"
                            && (receipt["revision_digest"] != check["revision_digest"]
                                || receipt["revision_digest"]
                                    .as_str()
                                    .is_none_or(|s| s.len() != 64)))
                        || serde_json::from_value::<Vec<String>>(receipt["argv"].clone())
                            .map(|a| trivial_command(&a))
                            .unwrap_or(true)
                        || (receipt["output_truncated"] == true
                            && receipt["large_output_id"].is_null())
                    {
                        return refuse("completion_worker_check_unverified");
                    }
                    if let Some(id) = receipt["large_output_id"].as_str() {
                        let hash:Option<String>=db.query_row("SELECT source_ref FROM large_outputs WHERE task_id=? AND id=? AND truncated=0",
                            (task,id),|r|r.get(0)).optional()?;
                        if hash.is_none() || snapshot.artifacts.get(id) != hash.as_ref() {
                            return refuse("completion_worker_output_changed");
                        }
                    }
                }
            }
            let report = v["report_ref"]
                .as_str()
                .ok_or(CoreError::Refused("completion_artifact_changed_or_missing"))?;
            let exists: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM reports WHERE task_id=? AND id=?)",
                (task, report),
                |r| r.get(0),
            )?;
            if !exists
                || snapshot.artifacts.get(report).map(String::as_str) != v["report_digest"].as_str()
                || v["report_digest"].as_str().is_none()
            {
                return refuse("completion_artifact_changed_or_missing");
            }
        }
        if !dropped_failures_tx(db, task)?.is_empty() {
            return refuse("completion_dropped_failed_check");
        }
        let (review_id, reviewed) = review_binding_tx(db, task)?;
        let latest: Option<(i64, String, String)> = db
            .query_row(
                "SELECT id,type,payload_json FROM runtime_events WHERE task_id=?
            AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1",
                [task],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let (id, kind, s) = latest.ok_or(CoreError::Refused("completion_verification_missing"))?;
        let v: Value = serde_json::from_str(&s)?;
        if kind != "VERIFICATION_PASSED"
            || id <= review_id
            || v["revision"] != revision
            || v["review_event"] != review_id
        {
            return refuse("completion_verification_missing");
        }
        if v["revision_digest"] != reviewed || digest(snapshot)? != reviewed {
            return refuse("completion_project_revision_changed");
        }
    }
    domain::state_tx(db, task, TaskState::Completed, None)
}
