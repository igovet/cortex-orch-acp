//! Inline native worker publication. Runs only on the owned StoreExecutor lane;
//! no SQLite transaction spans filesystem publication. A durable payload binding
//! precedes publication; the final event, node result and receipt commit together.
use cortex_core::{
    channels::{self, Credential, Identity},
    lifecycle::{
        self,
        completion::{self, EvidenceSnapshot},
        waits,
    },
    store::documents::{Publication, Published, Publisher},
    CoreError, CoreResult, Store,
};
use serde_json::{json, Value};

fn text<'a>(v: &'a Value, field: &str) -> CoreResult<&'a str> {
    v[field]
        .as_str()
        .ok_or(CoreError::Refused("invalid_native_result"))
}
/// Project only data retained in a failed result, in both owner worker readers.
pub(super) fn add_failed_result_fields(result: &mut Value, recorded: &Value) {
    if recorded["assignment_outcome"] != "failed" {
        return;
    }
    result["outcome"] = json!("failed");
    if recorded["objective_status"] == "partial" {
        result["objectiveStatus"] = json!("partial");
        let remaining: Vec<_> = recorded["remaining"].as_array().into_iter().flatten()
            .filter_map(Value::as_str).filter(|text| !text.trim().is_empty())
            .take(16).map(|text| text.chars().take(500).collect::<String>()).collect();
        if !remaining.is_empty() {
            result["remaining"] = json!(remaining);
        }
    }
    if let Some(detail) = recorded["blocker"]["detail"].as_str().filter(|text| !text.trim().is_empty()) {
        result["blockerDetail"] = json!(detail.chars().take(1000).collect::<String>());
    }
}
#[cfg(test)]
fn validate_result(
    store: &Store,
    c: &Credential,
    i: &Identity,
    args: &Value,
    evidence: &EvidenceSnapshot,
) -> CoreResult<Vec<Value>> {
    validate_result_with(store, c, i, args, evidence, &Relief::default())
}
/// What the runtime relaxes for a submission after the same correctable refusal
/// repeated [`SAME_REFUSAL_LIMIT`] times in one attempt (see [`relieve`]): it never
/// turns a failed or unverified check into a pass, only records what stays open.
#[derive(Default)]
struct Relief {
    /// Unread required predecessor reports are disclosed as an `unverified` limit
    /// instead of refused.
    disclose_unread_reports: bool,
    /// Runtime limits (`outcome: unverified`, `source: runtime`) recorded with the result.
    limits: Vec<Value>,
}
fn validate_result_with(
    store: &Store,
    c: &Credential,
    i: &Identity,
    args: &Value,
    evidence: &EvidenceSnapshot,
    relief: &Relief,
) -> CoreResult<Vec<Value>> {
    if i.role != "worker" || !matches!(i.stage.as_deref(), Some("execution" | "review")) {
        return Err(CoreError::Refused("worker_required"));
    }
    let report = text(args, "report")?;
    if report.trim().is_empty() || report.contains("Cortex draft ID:") {
        return Err(CoreError::Refused("native_inline_report_required"));
    }
    // Only a report template's own guidance comment is left-over guidance; any other
    // HTML comment (quoted markup under review, for example) is ordinary content.
    if cortex_core::assignment_brief::guidance_remaining(report) {
        return Err(CoreError::Refused("draft_guidance_remaining"));
    }
    if text(args, "summary")?.trim().is_empty() {
        return Err(CoreError::Refused("native_summary_required"));
    }
    let title = title(args)?;
    if title.trim().is_empty() || title.chars().count() > 200 {
        return Err(CoreError::Refused("invalid_publication"));
    }
    use cortex_core::lifecycle::worker_checks;
    let submitted = args["checks"]
        .as_array()
        .ok_or(CoreError::Refused("invalid_native_result"))?;
    let mut runs = store.read(|db| worker_checks::load_runs(db, &i.task, &c.channel_id))?;
    let logical = store.key().root();
    if store.read(|db| Ok(crate::native_project::identity_root(db, i)? != logical))? {
        let checkout = crate::native_project::checkout_binding(store, i)?
            .ok_or(CoreError::Refused("worktree_binding_invalid"))?;
        runs.retain(|run| {
            run.receipt.get("checkout") == Some(&checkout)
                && run.receipt["cwd"].as_str().map(std::path::Path::new).is_some_and(|cwd| {
                    checkout["root"].as_str().is_some_and(|root| cwd.starts_with(root))
                        && !cwd.components().any(|part| part == std::path::Component::ParentDir)
                })
        });
    }
    // A failed run that an earlier submission of this attempt cited and this one omits is
    // carried over by the runtime as a failed check (closed by an exact passing rerun when
    // one exists); it never refuses the submission.
    let presented: Vec<(String, Option<String>)> = store.read(|db| Ok(db.prepare(
        "SELECT json_extract(payload_json,'$.ref'),json_extract(payload_json,'$.name')
         FROM runtime_events WHERE task_id=? AND type='CHECK_FAILURE_PRESENTED'
         AND json_extract(payload_json,'$.attempt')=? ORDER BY id"
    )?.query_map((&i.task, &i.attempt), |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<Vec<_>, _>>()?))?;
    let mut all: Vec<Value> = submitted.to_vec();
    let first_carried = all.len();
    for (r, name) in &presented {
        let cited = submitted.iter().any(|c| {
            matches!(c["outcome"].as_str(), Some("passed" | "failed")) && c["tool_call_ref"] == *r
        });
        let Some(run) = runs.iter().find(|run| run.receipt["tool_call_ref"] == *r) else { continue };
        if cited || all[first_carried..].iter().any(|c| c["tool_call_ref"] == *r) {
            continue;
        }
        let program = run.receipt["argv"][0].as_str().unwrap_or("command");
        let program: String = program.rsplit('/').next().unwrap_or(program).chars().take(40).collect();
        let name = name.clone().unwrap_or_else(|| format!("{program} (carried from an earlier submission)"));
        all.push(json!({"name": name,
            "outcome": "failed", "tool_call_ref": r}));
    }
    let relevant = worker_checks::relevant_references(&all, &runs);
    for run in &mut runs {
        if !run.receipt["tool_call_ref"]
            .as_str()
            .is_some_and(|r| relevant.contains(r))
        {
            continue;
        }
        if let Some(id) = run.receipt["large_output_id"].as_str() {
            let output = crate::native_commands::read_output(
                store,
                i,
                c,
                &json!({"large_output_id":id,"limit":1}),
            );
            if let Ok(output) = output {
                let v = &output["structuredContent"];
                run.output_verified = v["integrity_verified"] == true
                    && v["truncated"] == false
                    && v["byte_count"] == run.receipt["byte_count"]
                    && v["exit_status"]["exit_code"] == run.receipt["exit_code"]
                    && v["failure"] == run.receipt["failure"];
            }
        }
    }
    // A cited pass whose run did not observe the exact submitted checkout may still be
    // bound to its own work set: the host compares that set's files now with the
    // stable observation retained across the run (`native_work_set`).
    let partial_checkout = evidence.incomplete || evidence.cap_reached || evidence.metadata_only;
    let checkout_digest = if partial_checkout { None } else { evidence.project_digest.as_deref() };
    let mut digests = std::collections::BTreeMap::new();
    for run in &mut runs {
        let reference = run.receipt["tool_call_ref"].as_str().unwrap_or("");
        let cited = submitted
            .iter()
            .any(|c| c["outcome"] == "passed" && c["tool_call_ref"] == reference);
        let exact = run.receipt["revision_digest"]
            .as_str()
            .is_some_and(|d| d.len() == 64 && Some(d) == checkout_digest);
        if !cited || exact || reference.is_empty() || run.operation != "command_run" {
            continue;
        }
        if let Some(assignment) = i.assignment.as_deref() {
            run.work_set = crate::native_work_set::binding_at_submission(
                store,
                &i.task,
                assignment,
                reference,
                &mut digests,
            )?.filter(|binding| binding.get("checkout") == run.receipt.get("checkout")
                && (binding.get("checkout").is_none() || binding["cwd"] == run.receipt["cwd"]));
        }
    }
    // Retain only this attempt's observed failures/pending receipts, including
    // citations whose explicit correction will subsequently be refused.
    let presented_now: Vec<_> = submitted
        .iter()
        .filter(|c| matches!(c["outcome"].as_str(), Some("passed" | "failed")))
        .filter_map(|c| Some((c["tool_call_ref"].as_str()?, c["name"].as_str().unwrap_or("check"))))
        .filter(|(reference, _)| {
            let matched: Vec<_> = runs
                .iter()
                .filter(|r| r.receipt["tool_call_ref"] == *reference)
                .collect();
            matched.len() == 1 && matched[0].presented_failure()
        })
        .collect();
    if !presented_now.is_empty() {
        store.write(|db| {
        channels::authenticate(db, &c.channel_id, c.secret(), true)?;
        for (reference, name) in &presented_now {
                    let exists:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='CHECK_FAILURE_PRESENTED' AND json_extract(payload_json,'$.attempt')=? AND json_extract(payload_json,'$.ref')=?)",
                        (&i.task,&i.attempt,reference),|r|r.get(0))?;
                    if !exists { lifecycle::event_tx(db,&i.task,"CHECK_FAILURE_PRESENTED",json!({"attempt":i.attempt,"ref":reference,"name":check_text(name, 200)}))?; }
        }
        Ok(())
    })?;
    }
    // Complete diagnostic observations stay in the command history. A
    // diagnostic citation never becomes a verified pass or an open check.
    let diagnostics: std::collections::BTreeSet<_> = runs
        .iter()
        .filter(|run| run.diagnostic_result())
        .filter_map(|run| run.receipt["tool_call_ref"].as_str())
        .filter(|reference| {
            runs.iter().filter(|run| run.receipt["tool_call_ref"] == *reference).take(2).count() == 1
        })
        .collect();
    let mut index = 0;
    all.retain_mut(|c| {
        let carried = index >= first_carried;
        index += 1;
        if matches!(c["outcome"].as_str(), Some("passed" | "failed"))
            && c["tool_call_ref"].as_str().is_some_and(|r| diagnostics.contains(r)) {
            return false;
        }
        if carried && !runs.iter().any(|run| run.receipt["tool_call_ref"] == c["tool_call_ref"] && run.verified_failure()) {
            c["outcome"] = json!("unverified");
            c["blocker"] = json!("The earlier submission cited this check without a retained, verified failing receipt.");
        }
        true
    });
    let checked_failure = args["assignment_outcome"] == "failed"
        && lifecycle::review::valid_blocker(&args["blocker"]["kind"])
        && args["blocker"]["detail"].as_str().is_some_and(|detail| !detail.trim().is_empty());
    let mut checks = match worker_checks::evaluate(&all, &runs, checkout_digest) {
        Ok(checks) => checks,
        Err(error) if matches!(error.code(), "invalid_check_retraction" | "invalid_check_supersession") => {
            // Assess the original observations without the requested correction.
            // A failure report can retain these facts without granting that correction.
            for check in &mut all {
                if let Some(fields) = check.as_object_mut() {
                    fields.remove("invocation_error");
                    fields.remove("command_correction");
                }
            }
            let checks = worker_checks::evaluate(&all, &runs, checkout_digest)?;
            if !checked_failure {
                refusal_details(&c.channel_id, error.code(), unresolved_check_items(&checks, &runs));
                return Err(error);
            }
            checks
        }
        Err(error) => return Err(error),
    };
    // Partial or failed whole-checkout observation cannot certify any check.
    // Each claimed pass must instead have independently verified scope evidence.
    if partial_checkout && !checked_failure {
        let claimed = submitted.iter().filter(|check| check["outcome"] == "passed").count();
        let verified = checks.iter().filter(|check| check["outcome"] == "passed"
            && check["evidence_strength"] == "verified"
            && check["revision_binding"]["kind"] == "work_set").count();
        if claimed != verified {
            refusal_details(&c.channel_id, "project_revision_unavailable", unresolved_check_items(&checks, &runs));
            return Err(CoreError::Refused("project_revision_unavailable"));
        }
    }
    if let Some(assignment) = i.assignment.as_deref() {
        store.read(|db| worker_checks::attribute_foreign(db, &i.task, assignment, &mut checks, &runs))?;
    }
    // A reviewer that does not approve (REQUEST_CHANGES, BLOCK or INCOMPLETE) may cite
    // an open failed check as evidence for its verdict; only APPROVE cannot.
    let review_requests_changes = i.stage.as_deref() == Some("review")
        && args["verdict"].as_str().is_some_and(|v| {
            ["REQUEST_CHANGES", "BLOCK", "INCOMPLETE"]
                .iter()
                .any(|x| v.eq_ignore_ascii_case(x))
        });
    for check in &checks {
        if check["outcome"] == "failed"
            && !worker_checks::resolved(check)
            && args["assignment_outcome"] != "failed"
            && !review_requests_changes
        {
            refusal_details(
                &c.channel_id,
                "open_failed_check",
                unresolved_check_items(&checks, &runs),
            );
            return Err(CoreError::Refused("open_failed_check"));
        }
        if matches!(check["outcome"].as_str(), Some("not_run" | "unverified"))
            && check["blocker"]
                .as_str()
                .is_none_or(|s| s.trim().is_empty())
        {
            refusal_details(&c.channel_id, "check_blocker_required", unresolved_check_items(&checks, &runs));
            return Err(CoreError::Refused("check_blocker_required"));
        }
    }
    // A worker finishes its own assignment. A part it can still do, or a failure
    // without a named dead end, is refused with exactly what is outstanding; the
    // channel stays open and the same session continues.
    if let Some((code, missing)) = outstanding(store, c, i, args)? {
        if !(code == "review_predecessor_reports_required" && relief.disclose_unread_reports) {
            if matches!(code, "assignment_blocker_required" | "objective_partial_blocker_required") {
                let mut details = unresolved_check_items(&checks, &runs);
                details.extend(missing);
                refusal_details(&c.channel_id, code, details);
            }
            return Err(CoreError::Refused(code));
        }
    }
    Ok(checks)
}

/// A reviewer's report is not complete merely because its result mentions it.
/// Every report the review packet requires (`assignment_brief::review_reports`: the
/// work of this review's round and the preceding review of the revision, never every
/// historical report) must have a successful page receipt whose `next_cursor` is
/// null on this exact channel and attempt, unless an INCOMPLETE result names it in
/// `not_covered` with its dead end. Returns the unread report ids, which a refusal
/// names while the same worker session stays alive, and the required reports that
/// cannot be read at all (no published edition, or the runtime's own read of it
/// fails other than by a busy lock) with their reason: those are dead ends retained
/// on the result, never a refusal the reviewer could not clear.
pub(super) fn predecessor_reading(
    store: &Store,
    c: &Credential,
    i: &Identity,
    args: &Value,
) -> CoreResult<(Vec<String>, Vec<Value>)> {
    let Some(assignment) = i.assignment.as_deref() else {
        return Ok((Vec::new(), Vec::new()));
    };
    let blocked = lifecycle::review::blocked_items(args);
    let (unread, mut unreadable) = store.read(|db| {
        use rusqlite::OptionalExtension;
        let node: Option<String> = db
            .query_row(
                "SELECT n.node_id FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
                 WHERE n.task_id=? AND n.assignment_ref=? AND n.revision=t.revision",
                (&i.task, assignment),
                |r| r.get(0),
            )
            .optional()?;
        let Some(node) = node else {
            return Ok((Vec::new(), Vec::new()));
        };
        let reports = cortex_core::assignment_brief::review_reports(db, &i.task, &node)?;
        let unreadable: Vec<Value> = reports["unavailable"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| r["report_id"].as_str())
            .filter(|id| !blocked.contains(*id))
            .map(|id| json!({"report_id":id,"reason":"unavailable"}))
            .collect();
        let mut unread = Vec::new();
        for report in reports["required"].as_array().into_iter().flatten() {
            let Some(id) = report["report_id"].as_str() else {
                continue;
            };
            if blocked.contains(id) {
                continue;
            }
            let read: bool = db.query_row(
                "SELECT EXISTS(
                   SELECT 1 FROM runtime_events
                   WHERE task_id=? AND type='REPORT_PAGE_READ'
                     AND json_extract(payload_json,'$.channel_id')=?
                     AND json_extract(payload_json,'$.assignment_ref')=?
                     AND json_extract(payload_json,'$.attempt')=?
                     AND json_extract(payload_json,'$.report_id')=?
                     AND json_type(payload_json,'$.next_cursor')='null'
                 )",
                (&i.task, &c.channel_id, assignment, &i.attempt, id),
                |r| r.get(0),
            )?;
            if !read {
                unread.push(id.to_owned());
            }
        }
        Ok((unread, unreadable))
    })?;
    // An unread report the runtime itself cannot read is a dead end, not work the
    // reviewer can still do; a busy lock is transient and stays required.
    let publisher = Publisher::new(store);
    let mut missing = Vec::new();
    for id in unread {
        match publisher.read_report(&i.task, &id, None, 1) {
            Err(e) if e.code() != "busy" => {
                unreadable.push(json!({"report_id":id,"reason":e.code()}));
            }
            _ => missing.push(id),
        }
    }
    Ok((missing, unreadable))
}
/// What this submission leaves undone that the worker can still do: the refusal
/// code with the exact outstanding items, checked before anything is published.
/// The same rule binds the recorded result (`completion::outstanding_tx`).
pub(super) fn outstanding(
    store: &Store,
    c: &Credential,
    i: &Identity,
    args: &Value,
) -> CoreResult<Option<(&'static str, Vec<String>)>> {
    // Without an assignment binding nothing can be recorded anyway; the failure
    // blocker rule still applies to the arguments alone.
    let assignment = i.assignment.as_deref().unwrap_or("");
    let stage = i.stage.as_deref().unwrap_or("execution");
    if stage == "execution" {
        let missing = store.read(|db| required_reports_unread(store, db, c, i))?;
        if !missing.is_empty() {
            return Ok(Some(("required_reports_unread", missing)));
        }
    }
    if let Some(found) =
        store.read(|db| completion::outstanding_tx(db, &i.task, assignment, stage, args))?
    {
        return Ok(Some(found));
    }
    if stage == "review" {
        let (unread, _) = predecessor_reading(store, c, i, args)?;
        if !unread.is_empty() {
            return Ok(Some(("review_predecessor_reports_required", unread)));
        }
    }
    Ok(None)
}

fn required_reports_unread(
    store: &Store, db: &rusqlite::Connection, c: &Credential, i: &Identity,
) -> CoreResult<Vec<String>> {
    let Some(assignment) = i.assignment.as_deref() else { return Ok(Vec::new()) };
    let required = cortex_core::required_reports::for_assignment(db, &i.task, assignment)?;
    let unread = cortex_core::required_reports::unread(db, &i.task, assignment, &c.channel_id, &i.attempt)?;
    let publisher = Publisher::new(store);
    let mut missing = Vec::new();
    for report in required {
        let (id, sha) = (report["report_id"].as_str(), report["sha256"].as_str());
        let error = match (id, sha) {
            (Some(id), Some(sha)) if report["available"] == true => {
                publisher.verify_immutable_report_on(db, &i.task, id, sha).err()
            }
            _ => Some(CoreError::Refused("required_report_unavailable")),
        };
        if error.is_some() || unread.contains(&report) {
            let name = id.or_else(|| report["from_node"].as_str()).unwrap_or("report");
            let detail = error.map_or_else(|| {
                if let (Some(start), Some(end)) = (report["section"]["start"].as_u64(), report["section"]["end"].as_u64()) {
                    format!("characters {start}..{end}")
                } else {
                    "whole report".to_owned()
                }
            }, |e| e.code().to_owned());
            missing.push(format!("{name} ({detail})"));
        }
    }
    Ok(missing)
}
/// The items of an open or omitted failed check, computed by `validate_result` from
/// the evaluation it refused (the checkout and work-set observations it used are not
/// repeatable afterwards), by channel. Taken once by `refusal_missing` in the same
/// serial publication step.
static REFUSAL_DETAILS: std::sync::Mutex<
    std::collections::BTreeMap<String, (&'static str, Vec<String>)>,
> = std::sync::Mutex::new(std::collections::BTreeMap::new());
const REFUSAL_ITEM_LIMIT: usize = 32;
fn check_text(text: &str, limit: usize) -> String {
    text.chars()
        .take(limit)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}
fn bounded_check_items(mut items: Vec<String>) -> Vec<String> {
    if items.len() > REFUSAL_ITEM_LIMIT {
        let omitted = items.len() - (REFUSAL_ITEM_LIMIT - 1);
        items.truncate(REFUSAL_ITEM_LIMIT - 1);
        items.push(format!("Omitted {omitted} additional unresolved items."));
    }
    items.into_iter().map(|item| check_text(&item, 640)).collect()
}
fn unresolved_check_items(checks: &[Value], runs: &[lifecycle::worker_checks::Run]) -> Vec<String> {
    let mut items = lifecycle::worker_checks::open_items(checks, runs);
    for check in checks {
        if !matches!(check["outcome"].as_str(), Some("unverified" | "not_run")) {
            continue;
        }
        let label = check_text(check["name"].as_str().unwrap_or("check"), 80);
        let reference = check_text(check["tool_call_ref"].as_str().unwrap_or("no receipt"), 160);
        let reason = match (check["outcome"].as_str(), check["evidence_gap"].as_str()) {
            (Some("not_run"), _) => "the check was not run",
            (_, Some("revision_mismatch")) => "the receipt does not bind to the current checkout or work set",
            (_, Some("call_incomplete" | "later_run_unresolved")) => "this or a later matching command has no complete result; inspect its status",
            (_, Some("trivial_command")) => "the cited command cannot establish a verification pass",
            (_, Some("tool_call_ref_missing" | "tool_call_ref_unknown" | "ambiguous_ref" | "ref_already_cited")) =>
                "the cited receipt is missing, ambiguous, duplicated or outside this attempt",
            _ => "the retained evidence does not verify the check",
        };
        items.push(format!("{label} [{reference}]: {reason}"));
    }
    items
}
fn names_check_items(code: &str) -> bool {
    FAILED_CHECK_CODES.contains(&code)
        || matches!(code, "project_revision_unavailable" | "check_blocker_required"
            | "assignment_blocker_required" | "objective_partial_blocker_required" | "command_not_settled")
}
fn pending_commands(
    db: &rusqlite::Connection, task: &str, channel: &str,
) -> CoreResult<Vec<Option<String>>> {
    let mut statement = db.prepare(
        "SELECT CASE WHEN json_type(result_json,'$.handle.reference')='text'
             THEN json_extract(result_json,'$.handle.reference') END FROM runtime_requests
         WHERE task_id=? AND actor=? AND operation='command_run'
         AND json_type(result_json,'$.handle')='object'
         AND json_extract(result_json,'$.settled') IS NOT 1 ORDER BY request_key LIMIT ?",
    )?;
    let rows = statement.query_map((task, channel, (REFUSAL_ITEM_LIMIT + 1) as i64), |row| row.get(0))?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}
fn refusal_details(channel: &str, code: &'static str, items: Vec<String>) {
    if let Ok(mut details) = REFUSAL_DETAILS.lock() {
        details.insert(channel.to_owned(), (code, bounded_check_items(items)));
    }
}
/// The items a refusal names in its `missing` field, recomputed from the refused
/// arguments: report sections, or what the worker has still to do.
pub(super) fn refusal_missing(
    store: &Store,
    c: &Credential,
    i: &Identity,
    args: &Value,
    code: &str,
) -> Option<Vec<String>> {
    if code == cortex_core::assignment_brief::SECTIONS_MISSING {
        return Some(missing_sections(store, i, args));
    }
    let detail = REFUSAL_DETAILS
        .lock()
        .ok()
        .and_then(|mut d| d.remove(&c.channel_id));
    if let Some((_, items)) = detail.filter(|(refused, items)| *refused == code && !items.is_empty()) {
        return Some(items);
    }
    if code == "review_project_revision_changed" {
        let assignment = i.assignment.as_deref()?;
        return store.read(|db| {
            let rebinds: i64 = db.query_row(
                "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='REVIEW_STARTED'
                 AND json_extract(payload_json,'$.assignment_ref')=?
                 AND json_extract(payload_json,'$.rebind') IS NOT NULL",
                (&i.task, assignment), |r| r.get(0),
            )?;
            if rebinds != 1 {
                return Ok(None);
            }
            let items: Vec<_> = completion::review_coverage_missing_tx(db, &i.task, assignment, args)?
                .into_iter()
                .filter_map(|m| Some(format!("{} ({})", m["path"].as_str()?, m["reason"].as_str()?)))
                .collect();
            Ok((!items.is_empty()).then_some(items))
        }).ok().flatten();
    }
    match outstanding(store, c, i, args) {
        Ok(Some((found, items))) if found == code && !items.is_empty() => Some(items),
        _ => None,
    }
}
#[cfg(test)]
mod evidence_tests {
    use super::*;
    use cortex_core::{
        lifecycle::{FoundationGuards, TaskMachine},
        OpenMode, ProjectKey, StoreChannel, TaskState,
    };
    use std::os::unix::fs::PermissionsExt;
    /// Owner store 2026-11-17: two `result_submit` calls failed with a bare Forbidden
    /// and nothing retained why. A content refusal now goes back to the worker with its
    /// code, and every refusal is retained by code alone, bound to the attempt.
    #[test]
    fn a_refused_result_is_retained_by_code_and_explained_to_the_worker() {
        let root = std::env::temp_dir().join(format!(
            "native-refusal-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        let owner = TaskMachine::new(&store)
            .create("t", "checks", "owner")
            .unwrap();
        TaskMachine::new(&store)
            .transition(
                "t",
                &owner,
                "planning",
                TaskState::Planning,
                None,
                &FoundationGuards,
            )
            .unwrap();
        let c = store
            .write(|db| channels::issue_tx(db, "t", &owner, channels::Principal::Coordinator))
            .unwrap();
        let i = Identity {
            read_only: false,
            task: "t".into(),
            attempt: "xa_fixture".into(),
            principal: "fixture".into(),
            epoch: 1,
            role: "worker".into(),
            profile: Some("worker-debugger".into()),
            stage: Some("execution".into()),
            assignment: Some("a".into()),
            session: None,
        };
        for code in ["open_failed_check", "omitted_failed_check"] {
            record_refusal(&store, &c, &i, code, None).unwrap();
        }
        let rows: Vec<String> = store
            .read(|db| {
                Ok(db.prepare("SELECT payload_json FROM runtime_events WHERE type='WORKER_RESULT_REFUSED' ORDER BY id")?
                    .query_map([], |r| r.get(0))?
                    .collect::<Result<Vec<_>, _>>()?)
            })
            .unwrap();
        assert_eq!(rows.len(), 2);
        let first: Value = serde_json::from_str(&rows[0]).unwrap();
        assert_eq!(
            first,
            json!({"channel_id":c.channel_id,"attempt":"xa_fixture","assignment_ref":"a","code":"open_failed_check"})
        );
        assert!(correctable("open_failed_check") && correctable("invalid_arguments"));
        assert!(!correctable("channel_identity_changed") && !correctable("worker_required"));
        let reply = refusal_reply_with("open_failed_check", None);
        assert_eq!(reply["isError"], true);
        assert_eq!(reply["structuredContent"]["code"], "open_failed_check");
        assert_eq!(reply["structuredContent"]["field"], "checks");
        assert_eq!(reply["structuredContent"]["recorded"], false);
        assert!(reply["structuredContent"]["message"]
            .as_str()
            .unwrap()
            .contains("The listed checks are open"));
        // A structural refusal names exactly the missing headings and is correctable.
        let reply = refusal_reply_with("report_sections_missing", Some(vec!["## Changes".into()]));
        assert_eq!(reply["structuredContent"]["missing"], json!(["## Changes"]));
        assert_eq!(reply["structuredContent"]["field"], "report");
        for code in [
            "report_sections_missing",
            "draft_guidance_remaining",
            "review_resubmit_required",
            "review_project_revision_changed",
            "review_project_revision_unstable",
            "review_predecessor_reports_required",
            "review_coverage_unfinished",
            "review_file_coverage_required",
            "review_block_findings_required",
            "review_not_covered_required",
            "assignment_blocker_required",
        ] {
            assert!(correctable(code), "{code}");
            assert!(
                !refusal_reply_with(code, None)["structuredContent"]["hint"]
                    .as_str()
                    .unwrap()
                    .is_empty(),
                "{code}"
            );
        }
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    /// Owner stores 2026-11-19: reviewers were refused until they had read every
    /// report of every earlier round. After three replans and six fix rounds a review
    /// requires only its round (the last fix and the review that asked for it); a
    /// reviewer that read those is accepted, and a required report nobody can read is
    /// a retained dead end, never a refusal that cannot be cleared.
    #[test]
    fn review_reading_requires_the_current_round_and_never_loops_on_unreadable_reports() {
        let root = std::env::temp_dir().join(format!(
            "native-review-reading-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        let owner = TaskMachine::new(&store)
            .create("t", "review", "owner")
            .unwrap();
        TaskMachine::new(&store)
            .transition(
                "t",
                &owner,
                "planning",
                TaskState::Planning,
                None,
                &FoundationGuards,
            )
            .unwrap();
        let c = store
            .write(|db| channels::issue_tx(db, "t", &owner, channels::Principal::Coordinator))
            .unwrap();
        let publisher = Publisher::new(&store);
        let publish = |key: &str| -> String {
            let d = publisher
                .create_draft("t", "worker", "general", key)
                .unwrap();
            publisher
                .write_draft(
                    "t",
                    "worker",
                    &d.draft_id,
                    &format!(
                        "Cortex draft ID: `{}`\n\n# Round\n\n{}\n",
                        d.draft_id,
                        "x".repeat(5000)
                    ),
                )
                .unwrap();
            publisher
                .publish(
                    "t",
                    "worker",
                    &d.draft_id,
                    key,
                    Publication {
                        title: "Round",
                        summary: "Round report",
                        author: "worker",
                        source_revision: 0,
                        artifacts: json!([]),
                    },
                )
                .unwrap()
                .report_id
        };
        // Revisions 1 to 4 (three replans) with 2, 2, 0 and 2 fix rounds; only the
        // current round's two reports are real publications.
        let live = [publish("review-2"), publish("fix-2")];
        let mut counter = 0u32;
        store
            .write(|db| {
                for (revision, rounds) in [(1i64, 2), (2, 2), (3, 0), (4, 2)] {
                    let mut ids: Vec<String> = Vec::new();
                    let mut add = |id: &str, stage: &str, deps: &[String], done: bool| -> CoreResult<()> {
                        counter += 1;
                        let report = match (revision, id) {
                            (4, "_review_2") => live[0].clone(),
                            (4, "_fix_2_work") => live[1].clone(),
                            _ => format!("r_{counter:012x}"),
                        };
                        let assignment = if revision == 4 && !done { "rv".to_owned() } else { format!("a{counter}") };
                        let result = done.then(|| json!({"report_ref":report}).to_string());
                        db.execute("INSERT INTO runtime_nodes VALUES ('t',?,?,'worker-backend-dev','o',?,'[\"src\"]',?,?,?,?)",
                            (revision, id, json!(deps).to_string(), stage, if done {"completed"} else {"running"}, assignment, result))?;
                        Ok(())
                    };
                    for id in ["work", "scan"] {
                        add(id, "execution", &[], true)?;
                        ids.push(id.into());
                    }
                    for k in 1..=rounds {
                        let review = format!("_review_{k}");
                        add(&review, "review", &ids.clone(), true)?;
                        ids.push(review.clone());
                        let fix = format!("_fix_{k}_work");
                        add(&fix, "execution", &[review], true)?;
                        ids.push(fix);
                    }
                    add(&format!("_review_{}", rounds + 1), "review", &ids.clone(), revision < 4)?;
                }
                db.execute("UPDATE runtime_tasks SET revision=4 WHERE task_id='t'", [])?;
                Ok(())
            })
            .unwrap();
        let i = Identity {
            read_only: false,
            task: "t".into(),
            attempt: "xa_review".into(),
            principal: "reviewer".into(),
            epoch: 1,
            role: "worker".into(),
            profile: Some("worker-code-reviewer".into()),
            stage: Some("review".into()),
            assignment: Some("rv".into()),
            session: None,
        };
        let args = json!({"verdict":"APPROVE","findings":[],"summary":"ok"});
        let found = outstanding(&store, &c, &i, &args).unwrap();
        assert_eq!(
            found,
            Some(("review_predecessor_reports_required", live.to_vec())),
            "only the current round is required"
        );
        let read = |report: &str, next: Value| {
            store
                .write(|db| {
                    lifecycle::event_tx(db, "t", "REPORT_PAGE_READ", json!({"channel_id":c.channel_id,
                        "assignment_ref":"rv","attempt":"xa_review","report_id":report,"next_cursor":next}))?;
                    Ok(())
                })
                .unwrap()
        };
        // A page that is not the last does not count.
        read(&live[0], json!("cr.next"));
        read(&live[1], Value::Null);
        assert_eq!(
            outstanding(&store, &c, &i, &args).unwrap(),
            Some(("review_predecessor_reports_required", vec![live[0].clone()]))
        );
        read(&live[0], Value::Null);
        assert_eq!(
            outstanding(&store, &c, &i, &args).unwrap(),
            None,
            "read reviewer accepted"
        );
        assert!(predecessor_reading(&store, &c, &i, &args)
            .unwrap()
            .1
            .is_empty());
        // The fix's report becomes unreadable: an edition whose file is gone, and the
        // review before it names a report that was never published.
        store
            .write(|db| {
                db.execute("INSERT INTO reports(id,task_id,kind,filename,created_at) VALUES ('r_eeeeeeeeeeee','t','report','r_eeeeeeeeeeee.md','0')", [])?;
                db.execute("INSERT INTO editions(report_id,title,summary,author,updated_at,digest,size_bytes) VALUES ('r_eeeeeeeeeeee','Gone','S','x','0','d',10)", [])?;
                db.execute("UPDATE runtime_nodes SET result_json=? WHERE task_id='t' AND revision=4 AND node_id='_fix_2_work'",
                    [json!({"report_ref":"r_eeeeeeeeeeee"}).to_string()])?;
                db.execute("UPDATE runtime_nodes SET result_json=? WHERE task_id='t' AND revision=4 AND node_id='_review_2'",
                    [json!({"report_ref":"r_dddddddddddd"}).to_string()])?;
                Ok(())
            })
            .unwrap();
        for _ in 0..3 {
            assert_eq!(
                outstanding(&store, &c, &i, &args).unwrap(),
                None,
                "no refusal loop"
            );
        }
        let (missing, unreadable) = predecessor_reading(&store, &c, &i, &args).unwrap();
        assert!(missing.is_empty());
        let ids: Vec<&str> = unreadable
            .iter()
            .map(|u| u["report_id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["r_dddddddddddd", "r_eeeeeeeeeeee"]);
        assert_eq!(unreadable[0]["reason"], "unavailable");
        assert!(unreadable[1]["reason"]
            .as_str()
            .is_some_and(|r| !r.is_empty() && r != "busy"));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn first_refusal_then_honest_failure_records_original_evidence() {
        let root = std::env::temp_dir().join(format!(
            "native-first-refusal-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        let owner = TaskMachine::new(&store).create("t", "checks", "owner").unwrap();
        let c = store.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'", [])?;
            db.execute("INSERT INTO runtime_nodes (task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status,assignment_ref)
                VALUES ('t',1,'test','worker-backend-dev','test','[]','[]','execution','running','a')", [])?;
            channels::issue_tx(db, "t", &owner, channels::Principal::Worker { assignment: "a" })
        }).unwrap();
        store.write(|db| channels::bind_session_tx(db, &c, "fixture-session")).unwrap();
        let i = store.read(|db| channels::authenticate(db, &c.channel_id, c.secret(), true)).unwrap();

        let dir = store.key().directory().join("t").join(".native-output");
        cortex_core::store::locks::private_directory(dir.parent().unwrap(), true).unwrap();
        cortex_core::store::locks::private_directory(&dir, true).unwrap();
        let mut output = cortex_core::store::locks::open_private(&dir.join("lo_failed.bin"), true, false).unwrap();
        use std::io::Write;
        output.write_all(b"private output").unwrap();
        output.sync_all().unwrap();
        let digest = crate::native_project::digest(b"private output");
        let reference = "native-command:failed";
        let receipt = json!({"tool_call_ref":reference,"argv":["cargo","test","private-arg"],
            "cwd":root,"profile":"development","complete":true,"settled":true,"exit_code":1,
            "failure":null,"large_output_id":"lo_failed","byte_count":14});
        store.write(|db| {
            db.execute("INSERT INTO large_outputs VALUES ('lo_failed','t',?,'terminal',?,'lo_failed.bin',14,0,1,NULL,NULL,'fixture')",
                (&c.channel_id,&digest))?;
            db.execute("INSERT INTO runtime_requests VALUES ('t','run',?,'command_run','bound',?)",
                (&c.channel_id,receipt.to_string()))?;
            Ok(())
        }).unwrap();
        let mut report = String::from("# Checks\n");
        let skeleton = cortex_core::assignment_brief::skeleton("worker-backend-dev", "execution");
        for section in skeleton["sections"].as_array().unwrap() {
            report.push_str(&format!("\n{}\n\nRecorded fixture check.\n", section["heading"].as_str().unwrap()));
        }
        let mut args = json!({"request_key":"submit","report":report,"summary":"One check is open.",
            "checks":[{"name":"tests","outcome":"failed","tool_call_ref":reference}]});
        let error = submit(&store, &c, &i, &args, EvidenceSnapshot::default()).unwrap_err();
        assert_eq!(error.code(), "open_failed_check");
        let items = refusal_missing(&store, &c, &i, &args, error.code()).unwrap();
        record_refusal(&store, &c, &i, error.code(), Some(&items)).unwrap();
        let first = refusal_reply_with(error.code(), Some(items));
        let rendered = first.to_string();
        assert!(rendered.contains(reference) && rendered.contains("no_passing_rerun"), "{first}");
        assert!(!rendered.contains("private-arg") && !rendered.contains("private output"));

        args["assignment_outcome"] = json!("failed");
        args["objective_status"] = json!("partial");
        args["remaining"] = json!(["Verify the write exactly once"]);
        args["blocker"] = json!({"kind":"owner_decision","detail":"A rerun may repeat an external write."});
        args["checks"] = json!([]);
        let receipt = submit(&store, &c, &i, &args, EvidenceSnapshot::default()).unwrap();
        assert_eq!(receipt["outcome"], "recorded");
        assert_eq!(receipt["certified"], false);
        let (status, result) = store.read(|db| {
            Ok(db.query_row("SELECT status,result_json FROM runtime_nodes WHERE assignment_ref='a'", [], |r|
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?)
        }).unwrap();
        assert_eq!(status, "failed");
        let result: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(result["report_ref"], receipt["report_ref"]);
        assert_eq!(result["checks"][0]["outcome"], "failed");
        assert_eq!(result["checks"][0]["tool_call_ref"], reference);
        assert_eq!(result["remaining"], args["remaining"]);
        assert_eq!(result["blocker"], args["blocker"]);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn passed_receipts_require_digest_nontrivial_command_unique_ref_and_intact_output() {
        let root = std::env::temp_dir().join(format!(
            "native-checks-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        let machine = TaskMachine::new(&store);
        let owner = machine.create("t", "checks", "owner").unwrap();
        machine
            .transition(
                "t",
                &owner,
                "planning",
                TaskState::Planning,
                None,
                &FoundationGuards,
            )
            .unwrap();
        let c = store
            .write(|db| channels::issue_tx(db, "t", &owner, channels::Principal::Coordinator))
            .unwrap();
        store
            .write(|db| channels::bind_session_tx(db, &c, "fixture-session"))
            .unwrap();
        let i = Identity {
            read_only: false,
            task: "t".into(),
            attempt: "fixture".into(),
            principal: "fixture".into(),
            epoch: 1,
            role: "worker".into(),
            profile: Some("worker-backend-dev".into()),
            stage: Some("execution".into()),
            assignment: None,
            session: None,
        };
        let dir = store.key().directory().join("t").join(".native-output");
        cortex_core::store::locks::private_directory(dir.parent().unwrap(), true).unwrap();
        cortex_core::store::locks::private_directory(&dir, true).unwrap();
        let path = dir.join("lo_fixture.bin");
        let mut file = cortex_core::store::locks::open_private(&path, true, false).unwrap();
        use std::io::Write;
        file.write_all(b"complete output").unwrap();
        file.sync_all().unwrap();
        let hash = crate::native_project::digest(b"complete output");
        let digest = "a".repeat(64);
        let receipt = json!({"tool_call_ref":"native-command:fixture","argv":["cargo","test"],
            "cwd":root,"profile":"development","complete":true,"settled":true,"exit_code":0,"failure":null,
            "revision_digest":digest,"large_output_id":"lo_fixture","byte_count":15});
        store.write(|db|{
            db.execute("INSERT INTO large_outputs VALUES ('lo_fixture','t',?,'terminal',?,'lo_fixture.bin',15,0,0,NULL,NULL,'fixture')",
                (&c.channel_id,&hash))?;
            db.execute("INSERT INTO runtime_requests VALUES ('t','command',?,'command_run','bound',?)",(&c.channel_id,receipt.to_string()))?;Ok(())
        }).unwrap();
        let args = json!({"report":"# Evidence","summary":"checked","checks":[{"name":"tests","outcome":"passed","tool_call_ref":"native-command:fixture"}]});
        let evidence = EvidenceSnapshot {
            project_digest: Some(digest.clone()),
            ..Default::default()
        };
        let checks = validate_result(&store, &c, &i, &args, &evidence).unwrap();
        assert_eq!(checks[0]["evidence_strength"], "verified");
        assert_eq!(checks[0]["revision_digest"], digest);
        let stale = EvidenceSnapshot {
            project_digest: Some("b".repeat(64)),
            ..Default::default()
        };
        assert_eq!(
            validate_result(&store, &c, &i, &args, &stale).unwrap()[0]["evidence_gap"],
            "revision_mismatch"
        );
        let mut duplicate = args.clone();
        duplicate["checks"]
            .as_array_mut()
            .unwrap()
            .push(args["checks"][0].clone());
        assert_eq!(
            validate_result(&store, &c, &i, &duplicate, &evidence).unwrap()[1]["evidence_gap"],
            "ref_already_cited"
        );
        let mut trivial = receipt.clone();
        trivial["argv"] = json!(["env", "A=1", "ls"]);
        store
            .write(|db| {
                db.execute(
                    "UPDATE runtime_requests SET result_json=? WHERE request_key='command'",
                    [trivial.to_string()],
                )?;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            validate_result(&store, &c, &i, &args, &evidence).unwrap()[0]["evidence_gap"],
            "trivial_command"
        );
        // Confirmed outside receipts have the same evidence rule and full-output binding.
        store.write(|db|{
            db.execute("UPDATE runtime_requests SET operation='outside_command',result_json=? WHERE request_key='command'",
                [json!({"complete":true,"reply":{"structuredContent":receipt}}).to_string()])?;Ok(())
        }).unwrap();
        assert_eq!(
            validate_result(&store, &c, &i, &args, &evidence).unwrap()[0]["evidence_strength"],
            "verified"
        );
        let mut diagnostic = receipt.clone();
        diagnostic["tool_call_ref"] = json!("native-command:diagnostic");
        diagnostic["argv"] = json!(["git", "diff", "--no-index", "--check", "/dev/null", "file"]);
        diagnostic["exit_code"] = json!(1);
        diagnostic["large_output_id"] = json!("lo_diagnostic");
        store.write(|db|{
            db.execute("INSERT INTO large_outputs VALUES ('lo_diagnostic','t',?,'terminal',?,'lo_fixture.bin',15,0,1,NULL,NULL,'fixture')",(&c.channel_id,&hash))?;
            db.execute("INSERT INTO runtime_requests VALUES ('t','diagnostic',?,'command_run','bound',?)",
                (&c.channel_id,diagnostic.to_string()))?;
            Ok(())
        }).unwrap();
        let mut diagnostic_args = args.clone();
        diagnostic_args["checks"] =
            json!([{"name":"inspect diff","outcome":"failed","tool_call_ref":"native-command:diagnostic"}]);
        assert!(validate_result(&store, &c, &i, &diagnostic_args, &evidence).unwrap().is_empty());
        assert!(!store.read(|db| Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE type='CHECK_FAILURE_PRESENTED'
             AND json_extract(payload_json,'$.ref')='native-command:diagnostic')",
            [], |r| r.get::<_, bool>(0),
        )?)).unwrap());

        // Persist a nonzero receipt, refuse it, reopen, then require its history.
        let mut failed_receipt = receipt.clone();
        failed_receipt["tool_call_ref"] = json!("native-command:failed");
        failed_receipt["exit_code"] = json!(1);
        failed_receipt["large_output_id"] = json!("lo_failed");
        let mut rerun = receipt.clone();
        rerun["tool_call_ref"] = json!("native-command:rerun");
        store.write(|db|{
            db.execute("INSERT INTO large_outputs VALUES ('lo_failed','t',?,'terminal',?,'lo_fixture.bin',15,0,1,NULL,NULL,'fixture')",(&c.channel_id,&hash))?;
            db.execute("INSERT INTO runtime_requests VALUES ('t','failed',?,'command_run','bound',?)",(&c.channel_id,failed_receipt.to_string()))?;
            Ok(())
        }).unwrap();
        let mut failed_args = args.clone();
        failed_args["checks"] =
            json!([{"name":"tests","outcome":"failed","tool_call_ref":"native-command:failed"}]);
        let mut invalid = failed_args.clone();
        invalid["checks"][0]["invocation_error"] =
            json!({"corrected_by":"unknown","reason":"not proven"});
        assert_eq!(
            validate_result(&store, &c, &i, &invalid, &evidence)
                .unwrap_err()
                .code(),
            "invalid_check_retraction"
        );
        assert_eq!(
            validate_result(&store, &c, &i, &failed_args, &evidence)
                .unwrap_err()
                .code(),
            "open_failed_check"
        );
        // The refusal names the open check, its receipt and what keeps it open.
        let items = refusal_missing(&store, &c, &i, &failed_args, "open_failed_check").unwrap();
        assert_eq!(items.len(), 1, "{items:?}");
        assert!(
            items[0].starts_with("tests [native-command:failed]: no_passing_rerun:"),
            "{items:?}"
        );
        // Taken once: a later unrelated refusal does not repeat it.
        assert_eq!(
            refusal_missing(&store, &c, &i, &failed_args, "open_failed_check"),
            None
        );
        let mut explicit_failure = failed_args.clone();
        explicit_failure["assignment_outcome"] = json!("failed");
        let mut reader = i.clone();
        reader.profile = Some("worker-explorer".into());
        // A failure is only a named dead end: without its blocker it is refused,
        // correctably, and the assignment stays active.
        let unnamed =
            validate_result(&store, &c, &reader, &explicit_failure, &evidence).unwrap_err();
        assert_eq!(unnamed.code(), "assignment_blocker_required");
        assert!(correctable(unnamed.code()));
        explicit_failure["blocker"] = json!({"kind":"other_role_work",
            "detail":"The failing test is in a module only the backend developer may change."});
        assert!(validate_result(&store, &c, &reader, &explicit_failure, &evidence).is_ok());
        let honest_failure = validate_result(&store, &c, &i, &explicit_failure, &evidence).unwrap();
        assert_eq!(honest_failure[0]["outcome"], "failed");
        drop(store);
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::RuntimeValidate,
        )
        .unwrap();
        store
            .write(|db| {
                db.execute(
                    "INSERT INTO runtime_requests VALUES ('t','rerun',?,'command_run','bound',?)",
                    (&c.channel_id, rerun.to_string()),
                )?;
                Ok(())
            })
            .unwrap();
        let mut success_args = args.clone();
        success_args["checks"][0]["tool_call_ref"] = json!("native-command:rerun");
        // The omitted earlier-cited failure is carried over by the runtime and closed by
        // the exact passing rerun (whatever the passed check is named), never refused.
        let carried = validate_result(&store, &c, &i, &success_args, &evidence).unwrap();
        assert_eq!(carried.len(), 2);
        assert_eq!(carried[1]["tool_call_ref"], "native-command:failed");
        assert_eq!(carried[1]["outcome"], "failed");
        assert_eq!(carried[1]["closed_by"], "native-command:rerun");
        failed_args["checks"]
            .as_array_mut()
            .unwrap()
            .push(success_args["checks"][0].clone());
        let history = validate_result(&store, &c, &i, &failed_args, &evidence).unwrap();
        assert_eq!(history[0]["outcome"], "failed");
        assert_eq!(history[0]["closed_by"], "native-command:rerun");
        assert_eq!(history[0]["evidence_strength"], "verified");
        assert_eq!(history[1]["outcome"], "passed");
        assert_eq!(
            store
                .read(|db| Ok(db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='CHECK_FAILURE_PRESENTED'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?))
                .unwrap(),
            1
        );
        // The retained failed citation must still be present after tampering.
        std::fs::write(path, "tampered").unwrap();
        assert_eq!(
            validate_result(&store, &c, &i, &failed_args, &evidence)
                .unwrap_err()
                .code(),
            "open_failed_check"
        );
        // The item names the actual gap (both runs share the tampered output here).
        let items = refusal_missing(&store, &c, &i, &failed_args, "open_failed_check").unwrap();
        assert!(
            items[0].starts_with("tests [native-command:failed]: call_incomplete:"),
            "{items:?}"
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    /// A pass whose run saw the shared checkout change elsewhere (another task's
    /// writes) verifies against its own unchanged work set, and only that.
    #[test]
    fn a_pass_binds_to_its_unchanged_work_set_while_other_files_change() {
        let root = std::env::temp_dir().join(format!(
            "native-work-set-submit-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir_all(root.join("ui")).unwrap();
        std::fs::create_dir_all(root.join("core")).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(root.join(".gitignore"), ".cortex-dev/\n").unwrap();
        std::fs::write(root.join("ui/a.ts"), "a").unwrap();
        std::fs::write(root.join("core/x.rs"), "x").unwrap();
        for args in [
            vec!["init", "-q"],
            vec!["add", "."],
            vec!["commit", "-qm", "base"],
        ] {
            assert!(std::process::Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&root)
                .status()
                .unwrap()
                .success());
        }
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        let machine = TaskMachine::new(&store);
        let owner = machine.create("t", "checks", "owner").unwrap();
        machine
            .transition(
                "t",
                &owner,
                "planning",
                TaskState::Planning,
                None,
                &FoundationGuards,
            )
            .unwrap();
        let c = store
            .write(|db| channels::issue_tx(db, "t", &owner, channels::Principal::Coordinator))
            .unwrap();
        let i = Identity {
            read_only: false,
            task: "t".into(),
            attempt: "fixture".into(),
            principal: "fixture".into(),
            epoch: 1,
            role: "worker".into(),
            profile: Some("worker-frontend-dev".into()),
            stage: Some("execution".into()),
            assignment: Some("a".into()),
            session: None,
        };
        let dir = store.key().directory().join("t").join(".native-output");
        cortex_core::store::locks::private_directory(dir.parent().unwrap(), true).unwrap();
        cortex_core::store::locks::private_directory(&dir, true).unwrap();
        let mut file =
            cortex_core::store::locks::open_private(&dir.join("lo_fixture.bin"), true, false)
                .unwrap();
        use std::io::Write;
        file.write_all(b"complete output").unwrap();
        file.sync_all().unwrap();
        let hash = crate::native_project::digest(b"complete output");
        let receipt = json!({"tool_call_ref":"native-command:fixture","argv":["npm","test"],
            "cwd":root,"profile":"development","complete":true,"settled":true,"exit_code":0,"failure":null,
            "revision_digest":"c".repeat(64),"large_output_id":"lo_fixture","byte_count":15});
        let set = vec!["ui/**".to_owned()];
        let observed = crate::native_work_set::digest(&root, &set).unwrap();
        store.write(|db|{
            db.execute("INSERT INTO large_outputs VALUES ('lo_fixture','t',?,'terminal',?,'lo_fixture.bin',15,0,0,NULL,NULL,'fixture')",
                (&c.channel_id,&hash))?;
            db.execute("INSERT INTO runtime_requests VALUES ('t','command',?,'command_run','bound',?)",(&c.channel_id,receipt.to_string()))?;
            lifecycle::event_tx(db, "t", "NATIVE_COMMAND_STARTED", json!({"ref":"native-command:fixture"}))?;
            lifecycle::worker_checks::record_work_set_tx(db, "t", "native-command:fixture", "a", &set, &observed, &observed)
        }).unwrap();
        let args = json!({"report":"# Evidence","summary":"checked","checks":[{"name":"tests","outcome":"passed","tool_call_ref":"native-command:fixture"}]});
        let evidence = EvidenceSnapshot {
            project_digest: Some("b".repeat(64)),
            ..Default::default()
        };
        // Another writer's file changes: the work set is unchanged even if a
        // full checkout observation cannot bind a revision.
        std::fs::write(root.join("core/x.rs"), "changed by another task").unwrap();
        let unavailable = EvidenceSnapshot { incomplete: true, ..Default::default() };
        let scoped = validate_result(&store, &c, &i, &args, &unavailable).unwrap();
        assert_eq!(scoped[0]["outcome"], "passed");
        assert_eq!(scoped[0]["revision_binding"]["kind"], "work_set");
        let mut forged = args.clone();
        forged["checks"][0]["tool_call_ref"] = json!("native-command:missing");
        for has_real_pass in [false, true] {
            if has_real_pass {
                forged["checks"].as_array_mut().unwrap().push(args["checks"][0].clone());
            }
            assert_eq!(
                validate_result(&store, &c, &i, &forged, &unavailable).unwrap_err().code(),
                "project_revision_unavailable"
            );
        }
        let items = refusal_missing(&store, &c, &i, &forged, "project_revision_unavailable").unwrap();
        assert!(items.iter().any(|item| item.contains("native-command:missing")));
        forged["assignment_outcome"] = json!("failed");
        forged["objective_status"] = json!("partial");
        forged["remaining"] = json!(["Verify the unbound check"]);
        forged["blocker"] = json!({"kind":"owner_decision","detail":"Rerunning this command may repeat an external write."});
        let checks = validate_result(&store, &c, &i, &forged, &unavailable).unwrap();
        assert_eq!(checks[0]["outcome"], "unverified");
        assert_eq!(checks[0]["claimed_outcome"], "passed");
        assert_eq!(checks[0]["tool_call_ref"], "native-command:missing");
        assert_eq!(checks[1]["outcome"], "passed");
        let partial = EvidenceSnapshot {
            project_digest: Some("c".repeat(64)),
            cap_reached: true,
            ..Default::default()
        };
        let scoped = validate_result(&store, &c, &i, &args, &partial).unwrap();
        assert_eq!(scoped[0]["revision_binding"]["kind"], "work_set");
        let checks = validate_result(&store, &c, &i, &args, &evidence).unwrap();
        assert_eq!(checks[0]["outcome"], "passed");
        assert_eq!(checks[0]["evidence_strength"], "verified");
        assert_eq!(
            checks[0]["revision_binding"],
            json!({"kind":"work_set","work_set":["ui/**"],"digest":observed})
        );
        // Another assignment's record never binds this one.
        let mut other = i.clone();
        other.assignment = Some("b".into());
        assert_eq!(
            validate_result(&store, &c, &other, &args, &evidence).unwrap()[0]["evidence_gap"],
            "revision_mismatch"
        );
        // A change inside the work set after the run unbinds it.
        std::fs::write(root.join("ui/a.ts"), "edited after the run").unwrap();
        assert_eq!(
            validate_result(&store, &c, &i, &args, &evidence).unwrap()[0]["evidence_gap"],
            "revision_mismatch"
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
/// A value is a report id only when it is exactly `r_` plus twelve lowercase
/// hexadecimal digits. Anything else, including prose that merely starts with
/// `r_` or has surrounding whitespace, is inline report text.
/// Content refusals of `result_submit` that the worker can correct and submit again.
/// Authority and identity refusals are not among them; they stay opaque to the engine.
pub(super) fn correctable(code: &str) -> bool {
    matches!(
        code,
        "command_not_settled"
            | "project_revision_unavailable"
            | "failed_check_repair_required"
            | "open_failed_check"
            | "omitted_failed_check"
            | "check_blocker_required"
            | "invalid_native_result"
            | "native_inline_report_required"
            | "native_summary_required"
            | "invalid_publication"
            | "invalid_check_retraction"
            | "invalid_check_supersession"
            | "result_report_not_owned"
            | "request_key_reused"
            | "invalid_arguments"
            | "draft_guidance_remaining"
            | "report_sections_missing"
            | "required_reports_unread"
            | "review_project_revision_changed"
            | "review_project_revision_unstable"
            | "review_resubmit_required"
            | "review_predecessor_reports_required"
            | "review_coverage_unfinished"
            | "review_file_coverage_required"
            | "review_block_findings_required"
            | "review_not_covered_required"
            | "assignment_blocker_required"
            | "objective_partial_blocker_required"
            | "review_goal_coverage_required"
    )
}
/// The report text a submission names: the inline Markdown, or the complete body of
/// a report published in this task (read page by page). `None` when the id names no
/// report of this task; ownership is refused later with `result_report_not_owned`.
fn submitted_report(store: &Store, i: &Identity, args: &Value) -> CoreResult<Option<String>> {
    let report = text(args, "report")?;
    if !is_report_id(report) {
        return Ok(Some(report.to_owned()));
    }
    // Only this principal's own publication is read, exactly as the result binds it.
    let owned: bool = store.read(|db| {
        Ok(db.query_row(
            "SELECT EXISTS(SELECT 1 FROM reports r JOIN drafts d ON d.published_report_id=r.id
             WHERE r.id=? AND r.task_id=? AND d.owner_thread_id=? AND r.kind='report')",
            (report, &i.task, &i.principal),
            |r| r.get(0),
        )?)
    })?;
    if !owned {
        return Err(CoreError::Refused("result_report_not_owned"));
    }
    let publisher = Publisher::new(store);
    let (mut body, mut cursor) = (String::new(), None::<String>);
    for _ in 0..256 {
        let page = match publisher.read_report(&i.task, report, cursor.as_deref(), 4000) {
            Ok(page) => page,
            Err(e) if e.code() == "not_found" => return Ok(None),
            Err(e) => return Err(e),
        };
        body.push_str(&page.markdown);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(Some(body)),
        }
    }
    Ok(Some(body))
}
/// The required report sections this submission lacks, in template order.
pub(super) fn missing_sections(store: &Store, i: &Identity, args: &Value) -> Vec<String> {
    match submitted_report(store, i, args) {
        Ok(Some(report)) => cortex_core::assignment_brief::missing_sections(
            &report,
            i.profile.as_deref().unwrap_or(""),
            i.stage.as_deref().unwrap_or("execution"),
        ),
        _ => Vec::new(),
    }
}
/// Presence check of the runtime-selected template's required `##` sections. A report
/// lacking one is refused (correctable) up to `STRUCTURE_REFUSALS` times per attempt;
/// after that it is accepted and the result carries `REPORT_STRUCTURE_INCOMPLETE`.
fn report_structure(store: &Store, i: &Identity, args: &Value) -> CoreResult<Option<Value>> {
    use cortex_core::assignment_brief::{
        SECTIONS_MISSING, STRUCTURE_INCOMPLETE, STRUCTURE_REFUSALS,
    };
    let missing = missing_sections(store, i, args);
    if missing.is_empty() {
        return Ok(None);
    }
    let refused: i64 = store.read(|db| {
        Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='WORKER_RESULT_REFUSED'
             AND json_extract(payload_json,'$.attempt')=? AND json_extract(payload_json,'$.code')=?",
            (&i.task, &i.attempt, SECTIONS_MISSING),
            |r| r.get(0),
        )?)
    })?;
    if refused < STRUCTURE_REFUSALS {
        return Err(CoreError::Refused(SECTIONS_MISSING));
    }
    Ok(Some(
        json!({"status":STRUCTURE_INCOMPLETE,"missing":missing,"refusals":refused}),
    ))
}
/// The worker-facing reply to a correctable refusal: its code and what to change.
/// Nothing was recorded or published; the channel stays active.
/// `missing` names the exact missing sections for `report_sections_missing`.
pub(super) fn refusal_reply_with(code: &str, missing: Option<Vec<String>>) -> Value {
    let hint = match code {
        "command_not_settled" => "The listed commands have no final receipt. Inspect each with command_status before claiming success. If settling or repeating it would be unsafe or outside your authority, submit assignment_outcome failed with blocker.kind and a concrete blocker.detail.",
        "project_revision_unavailable" => "The listed checks lack an exact current checkout or work-set binding. Inspect their receipts and rerun only if authorized and safe. If no safe repair exists, submit assignment_outcome failed with the concrete blocker. For partial work, add objective_status partial and list the unfinished parts in remaining.",
        "draft_guidance_remaining" => "The report still contains a report template's guidance comment line (<!-- ... -->). Replace it with the content it asks for.",
        "report_sections_missing" => "The report lacks required sections of its template (listed in missing). Add each as a level-two heading with its exact English text, keep the rest, and submit again; the request_key may stay the same.",
        "review_project_revision_changed" => "The project changed while you reviewed it; the review is now bound to the current state. Re-read what changed, then submit with a fresh request_key.",
        "review_project_revision_unstable" => "The project kept changing during the review, so nothing was recorded and the review stays bound to an earlier state. Submit again with a fresh request_key once the other writers have stopped: a submission that finds the project unchanged since this refusal re-binds the review to that state and is refused review_project_revision_changed; then re-read what changed and submit once more. If it never settles, submit INCOMPLETE naming the instability.",
        "review_resubmit_required" => "This request_key belongs to a refused submission of this review. Submit again with a fresh request_key.",
        "required_reports_unread" => "Read each listed report or section on this assignment with direct read_report calls. For a section use its start and read through the required end. Follow each returned next_cursor for a whole report. Then submit again. If the exact report is unavailable, tell the coordinator what is blocked.",
        "review_predecessor_reports_required" => "The refusal's inline_reads holds the current pages of the required reports the runtime read for you; they count as read, so submit again. Any report still listed in missing and not in inline_reads: read each required predecessor report listed in missing (your review packet's predecessor_reports; optional_reports are not required) through its final page with read_report. A report the runtime cannot read is not required and is retained on your result instead. This review stays active in this session; then submit again.",
        "review_coverage_unfinished" => "The not_covered parts listed in missing name no dead end, so you can still cover them. Review each one now in this session, then submit again; keep in not_covered only parts you cannot reach yourself, each with its blocker.",
        "review_file_coverage_required" => "The refusal's inline_reads holds the current pages of the changed files listed in missing that the runtime read for you on this channel; they count as read, so submit the same review again. For a file still listed in missing and not fully delivered (its entry has next_offset), or not delivered at all, read it yourself: the changed files listed in missing were not read completely in their current state. Read them with project_read in this session, up to 8 files per read_batch call as inline project_read items, and submit again (for a file with review_lines in the review packet, reading those line ranges of its current bytes is enough); a part you genuinely cannot reach goes in not_covered with its blocker under verdict INCOMPLETE. After repeated refusals the result is recorded with every file still unread disclosed as unverified.",
        "review_block_findings_required" => "A BLOCK must name each confirmed problem in findings. Name them, or finish the review and choose the verdict your evidence supports.",
        "review_not_covered_required" => "not_covered goes with verdict INCOMPLETE only, and INCOMPLETE needs at least one not_covered part with its blocker.",
        "assignment_blocker_required" => "A failed assignment needs blocker: the dead end (kind and concrete detail) you cannot clear within your role and permissions. If you can still finish the work, finish it and submit the completed result instead.",
        "objective_partial_blocker_required" => "objective_status partial (with the undone parts in remaining) is accepted only at a dead end, as assignment_outcome failed with its blocker. Finish the remaining parts listed in missing in this session and submit objective_status complete; only a part you genuinely cannot do goes back as a failed assignment.",
        "review_goal_coverage_required" => "An approval answers, in goal_coverage, every outcome of the review packet goal (listed in missing) exactly once: delivered true only when the reviewed work delivers it as the goal states it. Approving a stage that leaves an outcome undelivered is fine: report it as delivered false.",
        "open_failed_check" => "The listed checks are open. Inspect their receipts first. If authorized and safe, fix the cause and cite a verified passing rerun of the exact command under the same accepted scope. If a repeat risks side effects, needs owner action or is unavailable, retain the failed check and submit assignment_outcome failed with blocker.kind and a concrete blocker.detail. For partial work, set objective_status partial and name the unfinished parts in remaining.",
        "failed_check_repair_required" => "A project check failed. Repair the cause in your scope, rerun the exact same command, and submit the result again. Only a genuine dead end, submitted as a failure with its blocker, goes to the coordinator.",
        "omitted_failed_check" => "A failed check run that an earlier submission cited is missing (each is named in missing by its tool_call_ref). Cite each again as a failed check; a verified exact passing rerun cited as passed under the same name closes it. At a genuine dead end, submit assignment_outcome \"failed\" with its blocker.",
        "check_blocker_required" => "Give each listed not_run or unverified check a concrete blocker. At a dead end, submit assignment_outcome failed with a blocker describing why this attempt cannot proceed safely.",
        "native_inline_report_required" => "The report must be the complete Markdown report, or the exact report_id of a report published by this attempt.",
        "native_summary_required" => "The summary must not be empty.",
        "invalid_publication" => "The report title must be 1 to 200 characters.",
        "invalid_check_retraction" | "invalid_check_supersession" => "The cited correction is not eligible. It applies only to a failed check whose run is a mistaken invocation of your own (a standalone python/python3 -c probe for invocation_error, or a mistyped command line for command_correction), when corrected_by is the tool_call_ref of a passed check with exactly the same name, cited by no other correction, whose run is the same operation, cwd and profile, came later in this attempt and whose argv is a recognised correction of the failed one, with no other later unresolved or failed run of the failed command. A genuine nonzero observation (for example a git check-ignore that reports a path as not ignored) is a real failed check: cite it as failed without a correction, rerun the identical command to close it, or at a genuine dead end submit assignment_outcome \"failed\" with its blocker.",
        "result_report_not_owned" => "The report_id must name a report this attempt published.",
        "request_key_reused" => "Use a new request_key.",
        "invalid_arguments" => "The arguments do not match the advertised result_submit schema.",
        _ => "",
    };
    let mut value = json!({"code":code,"recorded":false,"hint":hint,
        "message":format!("No result was recorded ({code}). {hint} Correct the result and submit it again.")});
    let field = match code {
        "draft_guidance_remaining"
        | "report_sections_missing"
        | "native_inline_report_required"
        | "result_report_not_owned" => Some("report"),
        "review_project_revision_changed"
        | "review_project_revision_unstable"
        | "review_resubmit_required"
        | "request_key_reused" => Some("request_key"),
        "review_coverage_unfinished" | "review_not_covered_required" => Some("not_covered"),
        "review_block_findings_required" => Some("findings"),
        "assignment_blocker_required" => Some("blocker"),
        "objective_partial_blocker_required" => Some("objective_status"),
        "review_goal_coverage_required" => Some("goal_coverage"),
        "native_summary_required" => Some("summary"),
        "invalid_publication" => Some("report_title"),
        "open_failed_check"
        | "project_revision_unavailable"
        | "command_not_settled"
        | "omitted_failed_check"
        | "check_blocker_required"
        | "failed_check_repair_required"
        | "invalid_check_retraction"
        | "invalid_check_supersession" => Some("checks"),
        _ => None,
    };
    if let Some(field) = field {
        value["field"] = json!(field);
    }
    if let Some(missing) = missing.filter(|m| !m.is_empty()) {
        value["missing"] = json!(missing);
    }
    json!({"content":[{"type":"text","text":value.to_string()}],"structuredContent":value,"isError":true})
}
/// Retains why an authenticated worker's `result_submit` was refused: the refusal
/// code with the attempt binding and the outstanding items it named (paths, report
/// ids or part names), never arguments, report text or check output.
pub(super) fn record_refusal(
    store: &Store,
    c: &Credential,
    i: &Identity,
    code: &str,
    missing: Option<&[String]>,
) -> CoreResult<()> {
    store.write(|db| {
        let mut detail=json!({"channel_id":c.channel_id,"attempt":i.attempt,"assignment_ref":i.assignment,"code":code});
        if let Some(missing) = missing.filter(|m| !m.is_empty()) {
            detail["missing"] = json!(missing.iter().take(32).collect::<Vec<_>>());
        }
        if code == "command_not_settled" {
            let pending = pending_commands(db, &i.task, &c.channel_id)?;
            detail["command_refs"] = json!(pending.iter().take(REFUSAL_ITEM_LIMIT)
                .filter_map(|reference| reference.as_deref()).collect::<Vec<_>>());
            if pending.len() > REFUSAL_ITEM_LIMIT {
                detail["command_refs_truncated"] = json!(true);
            }
        }
        lifecycle::event_tx(db,&i.task,"WORKER_RESULT_REFUSED",detail)?;
        Ok(())
    })
}
fn is_report_id(value: &str) -> bool {
    value.strip_prefix("r_").is_some_and(|tail| {
        tail.len() == 12 && tail.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
    })
}
#[cfg(test)]
mod report_id_tests {
    use super::is_report_id;
    #[test]
    fn only_exact_ids_are_report_references() {
        assert!(is_report_id("r_0123456789ab"));
        for inline in [
            "r_check passed after the fix",
            "r_0123456789ab and notes",
            "r_0123456789ab\n",
            " r_0123456789ab",
            "r_0123456789AB",
            "r_0123456789a",
            "r_0123456789abc",
            "r_",
            "r_regression analysis\n\nDetails follow.",
            "# Report",
        ] {
            assert!(!is_report_id(inline), "{inline:?}");
        }
    }
}
fn title(args: &Value) -> CoreResult<&str> {
    if let Some(title) = args["report_title"].as_str() {
        return Ok(title);
    }
    Ok(text(args, "report")?
        .lines()
        .find_map(|line| line.strip_prefix("# "))
        .unwrap_or("Native worker report"))
}
/// A review rebinds to the current project revision when the project changes while
/// it runs (`preflight_result_tx`), and the reviewer is told to re-check the changed
/// project. Its recorded packet still names the bytes each changed file had at
/// delivery, so a complete read of the new bytes could never satisfy the coverage
/// check and every later verdict would be refused. Whenever the newest binding is
/// newer than the newest packet, the packet is recorded again with each listed
/// file's current bytes, read like `project_read` reads them. A file without such
/// bytes now (removed, a symlink) has nothing to read, like a deletion. Nothing else
/// in the packet changes, and an unchanged project never rewrites it.
fn refresh_rebound_review_packet_tx(
    db: &rusqlite::Transaction<'_>,
    root: &std::path::Path,
    task: &str,
    assignment: &str,
) -> CoreResult<()> {
    use rusqlite::OptionalExtension;
    let latest = |kind: &str| -> CoreResult<Option<(i64, String)>> {
        Ok(db
            .query_row(
                "SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type=?
                 AND json_extract(payload_json,'$.assignment_ref')=? ORDER BY id DESC LIMIT 1",
                (task, kind, assignment),
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?)
    };
    let (Some((bound, started)), Some((recorded, raw))) =
        (latest("REVIEW_STARTED")?, latest("REVIEW_PACKET")?)
    else {
        return Ok(());
    };
    // The first binding follows its packet (the packet names what it binds); only
    // a re-binding newer than the packet refreshes it.
    let rebound = serde_json::from_str::<Value>(&started)?["rebind"].is_number();
    if bound < recorded || !rebound {
        return Ok(());
    }
    let mut event: Value = serde_json::from_str(&raw)?;
    if let Some(files) = event["packet"]["changed_files"]["files"].as_array_mut() {
        for file in files {
            if file["change"] == "deleted" {
                continue;
            }
            let Some(path) = file["path"].as_str() else {
                continue;
            };
            file["current_sha256"] =
                crate::native_project::current_sha256(root, path).map_or(Value::Null, Value::from);
        }
    }
    let base = crate::native_project::review_base(db, task, &event["packet"])?;
    let bytes = crate::native_project::add_review_lines(root, base.as_deref(), &mut event["packet"]);
    if event["packet"]["review_strategy"].is_object() {
        let risk = cortex_core::strategy::review_strategy(db, task, &mut event["packet"], bytes)?;
        lifecycle::review::escalate_for_observed_risk_tx(db, task, risk)?;
    }
    lifecycle::completion::mark_reviewed_before(db, task, assignment, &mut event["packet"])?;
    event["refreshed_from"] = json!(recorded);
    lifecycle::event_tx(db, task, "REVIEW_PACKET", event)?;
    Ok(())
}
/// The same correctable refusal (same code, same named items) at most this many
/// times in one attempt. At the limit a disclosable refusal is recorded with its
/// items as `unverified` runtime limits, and a failed-check refusal as a failed
/// assignment (or, for a review, an INCOMPLETE verdict) whose blocker is the open
/// check, never as a pass. Stricter per-code bounds stay: report sections and
/// changed-file coverage (2).
pub(super) const SAME_REFUSAL_LIMIT: usize = 3;
/// Refusals that name a failed check the worker could not close.
const FAILED_CHECK_CODES: [&str; 5] = [
    "open_failed_check",
    "omitted_failed_check",
    "failed_check_repair_required",
    "invalid_check_retraction",
    "invalid_check_supersession",
];
fn copy_evidence(e: &EvidenceSnapshot) -> EvidenceSnapshot {
    EvidenceSnapshot {
        checkout_evidence: e.checkout_evidence.clone(),
        project_digest: e.project_digest.clone(),
        ignored_digest: e.ignored_digest.clone(),
        head: e.head.clone(),
        metadata_only: e.metadata_only,
        cap_reached: e.cap_reached,
        incomplete: e.incomplete,
        artifacts: e.artifacts.clone(),
        workers: e.workers.clone(),
        task_files: e.task_files.clone(),
    }
}
/// Only called after schema validation, on the serial writer lane. A correctable
/// refusal that already repeated identically ([`SAME_REFUSAL_LIMIT`]) is relieved
/// once ([`relieve`]): the submission is recorded with what stays open disclosed,
/// and `LOOP_BROKEN` names the loop. Every other outcome is the submission's own.
pub(super) fn submit(
    store: &Store,
    credential: &Credential,
    expected: &Identity,
    args: &Value,
    evidence: EvidenceSnapshot,
) -> CoreResult<Value> {
    let spare = copy_evidence(&evidence);
    let first = submit_once(
        store,
        credential,
        expected,
        args,
        evidence,
        &Relief::default(),
    );
    let code = match &first {
        Err(error) if correctable(error.code()) => error.code(),
        _ => return first,
    };
    let Ok(Some((relieved, relief, action, missing))) =
        relieve(store, credential, expected, args, code)
    else {
        return first;
    };
    let outcome = submit_once(store, credential, expected, &relieved, spare, &relief);
    if outcome.is_ok() {
        let attempt = expected.attempt.clone();
        let _ = store.write(|db| {
            let node: Option<String> = expected
                .assignment
                .as_deref()
                .map(|a| {
                    db.query_row(
                        "SELECT node_id FROM runtime_nodes WHERE task_id=? AND assignment_ref=?",
                        (&expected.task, a),
                        |r| r.get(0),
                    )
                })
                .transpose()
                .unwrap_or(None);
            let mut items = missing.clone();
            items.sort();
            let subject = format!("{code}|{}", items.join("|"));
            lifecycle::event_tx(
                db,
                &expected.task,
                "LOOP_BROKEN",
                json!({"key":cortex_core::scheduler::retry::loop_key("result_refusal",node.as_deref().unwrap_or(""),&subject),
                    "kind":"result_refusal","node":node,"attempt":attempt,"code":code,"count":SAME_REFUSAL_LIMIT,
                    "action":action,"limits":relief.limits,"missing":missing.iter().take(32).collect::<Vec<_>>()}),
            )?;
            Ok(())
        });
    }
    outcome
}
/// The relief for the `code` refusal of `args` once it is the
/// [`SAME_REFUSAL_LIMIT`]-th identical one of this attempt: the arguments to record,
/// what the runtime relaxes, the action (`disclosed` or `failed_assignment`) and the
/// named items. `None` below the limit or for a refusal nothing can relieve (a
/// malformed submission, a moving checkout, a command still running).
fn relieve(
    store: &Store,
    c: &Credential,
    i: &Identity,
    args: &Value,
    code: &'static str,
) -> CoreResult<Option<(Value, Relief, &'static str, Vec<String>)>> {
    // A checkout that another writer keeps changing (`review_project_revision_*`,
    // `drift: external`) is not the reviewer's to correct and never counts toward
    // this limit; neither does a command that is still running.
    if matches!(
        code,
        "review_project_revision_changed"
            | "review_project_revision_unstable"
            | "review_resubmit_required"
            | "command_not_settled"
    ) {
        return Ok(None);
    }
    let missing = refusal_missing(store, c, i, args, code);
    // The caller names these items in the refusal it records; keep them for it.
    if names_check_items(code) {
        if let Some(items) = &missing {
            refusal_details(&c.channel_id, code, items.clone());
        }
    }
    let items = missing.clone().unwrap_or_default();
    let named = if items.is_empty() {
        Value::Null
    } else {
        json!(items.iter().take(32).collect::<Vec<_>>())
    };
    let prior: Vec<String> = store.read(|db| {
        Ok(db
            .prepare(
                "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='WORKER_RESULT_REFUSED'
                 AND json_extract(payload_json,'$.attempt')=? AND json_extract(payload_json,'$.code')=?",
            )?
            .query_map((&i.task, &i.attempt, code), |r| r.get(0))?
            .collect::<Result<Vec<_>, _>>()?)
    })?;
    let same = prior
        .iter()
        .filter_map(|raw| serde_json::from_str::<Value>(raw).ok())
        .filter(|p| p["missing"] == named)
        .count();
    if same + 1 < SAME_REFUSAL_LIMIT {
        return Ok(None);
    }
    let review = i.stage.as_deref() == Some("review");
    let shown = |items: &[String]| -> String {
        let mut text = items
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        if items.len() > 10 {
            text.push_str(&format!("; and {} more", items.len() - 10));
        }
        text
    };
    let limit = |name: &str, blocker: String| json!({"name":name,"outcome":"unverified","blocker":blocker,"source":"runtime"});
    let mut relieved = args.clone();
    let mut relief = Relief::default();
    // A review part left open is recorded as not covered, with the reviewer's own
    // words, under a dead end the next plan owns (`other_role_work`).
    let close_gaps = |relieved: &mut Value| {
        if let Some(gaps) = relieved["not_covered"].as_array_mut() {
            for gap in gaps {
                if !lifecycle::review::valid_blocker(&gap["blocker"]) {
                    gap["blocker"] = json!("other_role_work");
                    if gap["reason"].as_str().is_none_or(|r| r.trim().is_empty()) {
                        gap["reason"] = json!("The reviewer named no dead end for this part.");
                    }
                }
            }
        }
    };
    let action = match code {
        "check_blocker_required" => {
            for check in relieved["checks"].as_array_mut().into_iter().flatten() {
                if matches!(check["outcome"].as_str(), Some("not_run" | "unverified"))
                    && check["blocker"]
                        .as_str()
                        .is_none_or(|s| s.trim().is_empty())
                {
                    check["blocker"] = json!("No blocker was given for this check; the runtime recorded it unverified after repeated identical refusals.");
                }
            }
            "disclosed"
        }
        "review_predecessor_reports_required" if review => {
            relief.disclose_unread_reports = true;
            relief.limits.push(limit(
                "Predecessor reports not read",
                format!("The reviewer did not read these required predecessor reports to their last page after repeated identical refusals: {}.", shown(&items)),
            ));
            "disclosed"
        }
        "review_coverage_unfinished"
        | "review_not_covered_required"
        | "review_block_findings_required"
            if review =>
        {
            let incomplete = |v: &Value| {
                v["verdict"]
                    .as_str()
                    .is_some_and(|v| v.eq_ignore_ascii_case("INCOMPLETE"))
            };
            let blocks = lifecycle::review::block_without_findings(&relieved);
            if blocks || !incomplete(&relieved) {
                // Never a pass: a BLOCK without findings, or a verdict that left parts
                // uncovered, is recorded as INCOMPLETE.
                relieved["verdict"] = json!("INCOMPLETE");
            }
            let mut gaps = relieved["not_covered"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if blocks {
                gaps.push(json!({"item":"Blocking problems the reviewer did not name","blocker":"other_role_work",
                    "reason":"The reviewer returned BLOCK without naming a finding after repeated identical refusals."}));
            }
            if gaps.is_empty() {
                gaps.push(json!({"item":"Parts the reviewer did not name","blocker":"other_role_work",
                    "reason":"The reviewer returned INCOMPLETE without naming what it did not cover after repeated identical refusals."}));
            }
            relieved["not_covered"] = json!(gaps);
            close_gaps(&mut relieved);
            let open: Vec<String> = relieved["not_covered"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|g| g["item"].as_str().map(str::to_owned))
                .collect();
            relief.limits.push(limit(
                "Review parts not covered",
                format!("The review was recorded INCOMPLETE after repeated identical refusals; these parts stay uncovered: {}.", shown(&open)),
            ));
            "disclosed"
        }
        // Never a pass: a partial objective is recorded as the failed assignment it is,
        // and an approval's unanswered goal outcomes as not delivered.
        "objective_partial_blocker_required" => {
            relieved["assignment_outcome"] = json!("failed");
            if relieved["remaining"].as_array().is_none_or(Vec::is_empty) {
                relieved["remaining"] = json!(["Parts of the objective the worker did not name"]);
            }
            if !lifecycle::review::valid_blocker(&relieved["blocker"]["kind"]) {
                relieved["blocker"] = json!({"kind":"other_role_work","detail":
                    "The worker reported its objective as partly done without naming a dead end; the runtime recorded the remaining work for the coordinator after repeated identical refusals."});
            }
            "failed_assignment"
        }
        "review_goal_coverage_required" if review => {
            let outcomes = store.read(|db| lifecycle::plan::task_outcomes(db, &i.task))?.unwrap_or_default();
            let mut coverage: Vec<Value> = relieved["goal_coverage"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|c| outcomes.iter().any(|o| o["id"] == c["id"]))
                .collect();
            for o in &outcomes {
                if !coverage.iter().any(|c| c["id"] == o["id"]) {
                    coverage.push(json!({"id":o["id"],"delivered":false,
                        "note":"The reviewer did not report this outcome after repeated identical refusals."}));
                }
            }
            relieved["goal_coverage"] = json!(coverage);
            "disclosed"
        }
        "assignment_blocker_required" => {
            let detail = relieved["blocker"]["detail"].as_str().filter(|d| !d.trim().is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| "The worker reported the assignment failed without naming a dead end; the runtime recorded the failure after repeated identical refusals.".into());
            relieved["blocker"] = json!({"kind":"other_role_work","detail":detail});
            "failed_assignment"
        }
        code if FAILED_CHECK_CODES.contains(&code) => {
            if code == "omitted_failed_check" {
                // The runtime cites the failed runs the submission left out, as failed.
                let refs: Vec<String> = items
                    .iter()
                    .filter_map(|item| {
                        item.strip_prefix('[')
                            .and_then(|rest| rest.split_once(']'))
                            .map(|(r, _)| r.to_owned())
                    })
                    .collect();
                if refs.is_empty() {
                    return Ok(None);
                }
                let checks = relieved["checks"]
                    .as_array_mut()
                    .ok_or(CoreError::Refused("invalid_native_result"))?;
                for reference in refs {
                    checks.push(json!({"name":format!("Earlier failed run {reference}"),"outcome":"failed","tool_call_ref":reference}));
                }
            }
            if matches!(
                code,
                "invalid_check_retraction" | "invalid_check_supersession"
            ) {
                for check in relieved["checks"].as_array_mut().into_iter().flatten() {
                    if let Some(check) = check.as_object_mut() {
                        check.remove("invocation_error");
                        check.remove("command_correction");
                    }
                }
            }
            let open = if items.is_empty() {
                "an open failed check".to_owned()
            } else {
                shown(&items)
            };
            if review {
                if relieved["verdict"]
                    .as_str()
                    .is_some_and(|v| v.eq_ignore_ascii_case("APPROVE"))
                {
                    relieved["verdict"] = json!("INCOMPLETE");
                    let mut gaps = relieved["not_covered"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default();
                    gaps.push(json!({"item":"Open failed check","blocker":"other_role_work",
                        "reason":format!("A failed check stayed open after repeated identical refusals: {open}.")}));
                    relieved["not_covered"] = json!(gaps);
                }
                close_gaps(&mut relieved);
            } else {
                relieved["assignment_outcome"] = json!("failed");
                let own = relieved["blocker"]["detail"]
                    .as_str()
                    .filter(|d| !d.trim().is_empty())
                    .map(str::to_owned);
                relieved["blocker"] = json!({"kind":"other_role_work","detail":format!(
                    "The open failed check could not be closed in this attempt after repeated identical refusals: {open}. Its repair belongs to the next plan.{}",
                    own.map(|o| format!(" Worker: {o}")).unwrap_or_default())});
            }
            "failed_assignment"
        }
        _ => return Ok(None),
    };
    Ok(Some((relieved, relief, action, items)))
}
/// Only called after schema validation, on the serial writer lane. Re-authenticate
/// before every phase; external epoch changes may fence a publication in progress.
/// Such a publication remains retained, but is never promoted to a node result.
fn submit_once(
    store: &Store,
    credential: &Credential,
    expected: &Identity,
    args: &Value,
    mut evidence: EvidenceSnapshot,
    relief: &Relief,
) -> CoreResult<Value> {
    let i = store
        .read(|db| channels::authenticate(db, &credential.channel_id, credential.secret(), true))?;
    if &i != expected {
        return Err(CoreError::Refused("channel_identity_changed"));
    }
    // Keep a partial fingerprint out of the persisted result. Completion
    // re-evaluates receipts against this result's retained project digest.
    if evidence.incomplete || evidence.cap_reached || evidence.metadata_only {
        evidence.project_digest = None;
    }
    let mut checks = validate_result_with(store, credential, &i, args, &evidence, relief)?;
    if args["assignment_outcome"] != "failed" {
        let pending = store.read(|db| pending_commands(db, &i.task, &credential.channel_id))?;
        if !pending.is_empty() {
            let mut items = unresolved_check_items(&checks, &[]);
            items.extend(pending.into_iter().map(|reference| match reference {
                Some(reference) => format!("Command [{}]: no settled receipt; inspect its current status", check_text(&reference, 160)),
                None => "An owned command has no settled receipt or retained reference.".to_owned(),
            }));
            refusal_details(&credential.channel_id, "command_not_settled", items);
            return Err(CoreError::Refused("command_not_settled"));
        }
    }
    // What the runtime recorded open after repeated identical refusals stays visible.
    checks.extend(relief.limits.iter().cloned());
    let assignment = i
        .assignment
        .as_deref()
        .ok_or(CoreError::Refused("worker_required"))?;
    // A review verdict is compared with what its review binds: the current bytes
    // of the task's own files (`native_change_set`), or the whole checkout.
    let review_root = if i.stage.as_deref() == Some("review") {
        let binding = crate::native_project::checkout_binding(store, &i)?;
        let root = binding.as_ref().and_then(|b| b["root"].as_str())
            .map(std::path::PathBuf::from).unwrap_or_else(|| store.key().root().to_path_buf());
        store.read(|db| {
            crate::native_change_set::bind_task_files_tx(db, &root, &i.task, &mut evidence)
        })?;
        Some(root)
    } else { None };
    let key = text(args, "request_key")?;
    let intent_actor = format!("native-publication:{}", credential.channel_id);
    // Conflict-check the complete payload before any file effect. This committed
    // intent makes a crash/retry deterministic, not a new report or changed claim.
    if let Some(refusal) = store.write(|db| {
        let current =
            channels::authenticate(db, &credential.channel_id, credential.secret(), true)?;
        if current != i {
            return Err(CoreError::Refused("channel_identity_changed"));
        }
        let refusal =
            lifecycle::review::preflight_result_tx(db, &i.task, assignment, key, &evidence)?;
        if let Some(root) = &review_root {
            refresh_rebound_review_packet_tx(db, root, &i.task, assignment)?;
        }
        Ok(refusal)
    })? {
        // The re-binding is committed; the worker gets a structured, correctable error
        // (re-check the changed project, submit with a fresh request_key).
        return Err(CoreError::Refused(match refusal["error"].as_str() {
            Some("review_project_revision_changed") => "review_project_revision_changed",
            Some("review_project_revision_unstable") => "review_project_revision_unstable",
            _ => "review_resubmit_required",
        }));
    }
    // The required report sections are checked once the assignment may record a result
    // (an open question still refuses first) and before the payload binds the key, so a
    // corrected resubmission with the same request_key is a new payload, not a conflict.
    store.write(|db| {
        channels::authenticate(db, &credential.channel_id, credential.secret(), true)?;
        waits::check_mutation_tx(db, &i.task, assignment)
    })?;
    let structure = report_structure(store, &i, args)?;
    let source_revision = store.write(|db| {
        channels::authenticate(db, &credential.channel_id, credential.secret(), true)?;
        waits::check_mutation_tx(db, &i.task, assignment)?;
        // An earlier tool on this channel may already own this key. Refuse
        // before publication, not after creating an otherwise unused artifact.
        let used: bool = db.query_row(
            "SELECT EXISTS(SELECT 1 FROM runtime_requests WHERE task_id=? AND actor=? AND request_key=?)",
            (&i.task, &credential.channel_id, key), |r| r.get(0),
        )?;
        if used {
            return Err(CoreError::Refused("request_key_reused"));
        }
        lifecycle::request_tx(
            db,
            &i.task,
            &intent_actor,
            key,
            "result_payload",
            args,
            || {
                let revision: i64 = db.query_row(
                    "SELECT coalesce(max(revision),0) FROM source_revisions WHERE task_id=?",
                    [&i.task],
                    |r| r.get(0),
                )?;
                Ok(json!({"source_revision":revision}))
            },
        )
    })?["source_revision"]
        .as_i64()
        .ok_or(CoreError::Refused("invalid_result_intent"))?;
    let publisher = Publisher::new(store);
    let report = if is_report_id(text(args, "report")?) {
        let id = text(args, "report")?;
        // A report reference is not authority: it must come from this exact
        // channel principal's publication in this task, not a sibling's result.
        store.read(|db| {
            Ok(db
                .query_row(
                    "SELECT r.id,e.digest,e.size_bytes,coalesce(p.source_revision,0),
                coalesce(p.artifacts,'[]') FROM reports r JOIN editions e ON e.report_id=r.id
                LEFT JOIN report_provenance p ON p.edition_sequence=e.sequence
                JOIN drafts d ON d.published_report_id=r.id
                WHERE r.id=? AND r.task_id=? AND d.owner_thread_id=? AND r.kind='report'
                ORDER BY e.sequence DESC LIMIT 1",
                    (id, &i.task, &i.principal),
                    |r| {
                        Ok(Published {
                            report_id: r.get(0)?,
                            sha256: r.get(1)?,
                            size_bytes: r.get::<_, i64>(2)? as u64,
                            source_revision: r.get(3)?,
                            artifacts: serde_json::from_str(&r.get::<_, String>(4)?)
                                .map_err(|_| rusqlite::Error::InvalidQuery)?,
                        })
                    },
                )
                .map_err(|_| CoreError::Refused("result_report_not_owned"))?)
        })?
    } else {
        // The runtime, not the model, selects the report template for the role and stage.
        let template = cortex_core::assignment_brief::template_for(
            i.profile.as_deref().unwrap_or(""),
            i.stage.as_deref().unwrap_or("execution"),
        );
        let draft = publisher.create_draft(&i.task, &i.principal, template, key)?;
        let published: bool = store.read(|db| Ok(db.query_row(
        "SELECT published_report_id IS NOT NULL FROM drafts WHERE id=? AND task_id=? AND owner_thread_id=?",
        (&draft.draft_id, &i.task, &i.principal), |r| r.get(0),
    )?))?;
        if !published {
            publisher.write_draft(
                &i.task,
                &i.principal,
                &draft.draft_id,
                &format!(
                    "Cortex draft ID: `{}`\n\n{}",
                    draft.draft_id,
                    text(args, "report")?
                ),
            )?;
        }
        store.read(|db| {
            channels::authenticate(db, &credential.channel_id, credential.secret(), true)
        })?;
        let report = publisher.publish(
            &i.task,
            &i.principal,
            &draft.draft_id,
            key,
            Publication {
                title: title(args)?,
                // The full untruncated result summary is retained on the result, not
                // squeezed into the catalogue's independent 320-character field.
                summary: "Native worker result; see the immutable report and structured checks.",
                author: i
                    .profile
                    .as_deref()
                    .ok_or(CoreError::Refused("worker_required"))?,
                source_revision,
                artifacts: json!([]),
            },
        )?;
        report
    };
    // Publisher verifies the complete file against the edition digest, even
    // though this read exposes only its bounded first page.
    let observed = publisher.read_report(&i.task, &report.report_id, None, 4000)?;
    evidence
        .artifacts
        .insert(report.report_id.clone(), observed.sha256);
    let mut result = args.clone();
    result["checks"] = json!(checks);
    let object = result
        .as_object_mut()
        .ok_or(CoreError::Refused("invalid_native_result"))?;
    object.remove("report");
    object.remove("report_title");
    object.remove("request_key");
    result["report_ref"] = json!(report.report_id);
    if let Some(verdict) = result["verdict"].as_str() {
        result["verdict"] = json!(verdict.to_ascii_uppercase());
    }
    // A required predecessor report nobody can read is retained on the review result
    // with its reason, for the coordinator; it never held the review back.
    if i.stage.as_deref() == Some("review") {
        let (_, unreadable) = predecessor_reading(store, credential, &i, args)?;
        if !unreadable.is_empty() {
            result["predecessor_reports_unreadable"] = json!(unreadable);
        }
    }
    let mut bound = json!({"report_ref":report.report_id,"attempt_id":i.attempt,"checks":checks});
    if let Some(structure) = &structure {
        // Visible to the reviewer and the coordinator with the result itself.
        result["report_structure"] = structure.clone();
        bound["report_structure"] = structure.clone();
    }
    store.write(|db| {
        let current = channels::authenticate(db, &credential.channel_id, credential.secret(), true)?;
        if current != i {
            return Err(CoreError::Refused("channel_identity_changed"));
        }
        lifecycle::request_tx(db, &i.task, &credential.channel_id, key, "result_submit", args, || {
            if !required_reports_unread(store, db, credential, &i)?.is_empty() {
                return Err(CoreError::Refused("required_reports_unread"));
            }
            // Binding is created by the authenticated host, not by report text.
            db.execute(
                "INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at)
                 VALUES (?,'worker_result',?,?,?,'native-runtime',?)",
                (&i.task, assignment, format!("native-result:{}:{key}", credential.channel_id),
                    bound.to_string(), cortex_core::now()),
            )?;
            completion::record_result_tx(db, &i.task, assignment, &result, &evidence)?;
            cortex_core::store::fact_memory::retain_result_tx(db, &i.task, assignment, &credential.channel_id, &result, crate::native_project::memory_source_path)?;
            let mut receipt = json!({"outcome":"recorded","report_ref":report.report_id,"assignment_ref":assignment,
                "certified":false});
            if let Some(structure) = &structure {
                receipt["report_structure"] = structure.clone();
            }
            Ok(receipt)
        })
    })
}

#[cfg(test)]
mod rebound_packet_tests {
    use super::*;
    use cortex_core::{lifecycle::TaskMachine, OpenMode, ProjectKey, StoreChannel};
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    fn sha(b: &[u8]) -> String {
        format!("{:x}", Sha256::digest(b))
    }
    fn packets(store: &Store) -> Vec<Value> {
        store
            .read(|db| {
                Ok(db
                    .prepare(
                        "SELECT payload_json FROM runtime_events WHERE task_id='t'
                         AND type='REVIEW_PACKET' ORDER BY id",
                    )?
                    .query_map([], |r| r.get::<_, String>(0))?
                    .map(|s| serde_json::from_str(&s.unwrap()).unwrap())
                    .collect())
            })
            .unwrap()
    }

    /// Owner store 2026-11-19: CHANGELOG.md and a document changed 25 s after a
    /// review packet was recorded. The review was rebound to the new project
    /// revision, but the packet kept the old file hashes, so every complete read
    /// of the current bytes was "missing or stale" and each verdict was refused.
    #[test]
    fn a_rebound_review_packet_names_the_current_bytes_of_its_changed_files() {
        let root = std::env::temp_dir().join(format!(
            "native-rebound-packet-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        TaskMachine::new(&store)
            .create("t", "review", "owner")
            .unwrap();
        std::fs::write(root.join("a.md"), "reviewed\n").unwrap();
        std::fs::write(root.join("gone.md"), "reviewed\n").unwrap();
        let old = sha(b"reviewed\n");
        let files = json!([
            {"path":"a.md","change":"modified","current_sha256":old},
            {"path":"gone.md","change":"added","current_sha256":old},
            {"path":"was.md","change":"deleted"},
        ]);
        let refresh = || {
            store
                .write(|db| refresh_rebound_review_packet_tx(db, &root, "t", "a1"))
                .unwrap()
        };
        store
            .write(|db| {
                lifecycle::event_tx(
                    db,
                    "t",
                    "REVIEW_STARTED",
                    json!({"assignment_ref":"a1","revision_digest":"r1"}),
                )?;
                lifecycle::event_tx(
                    db,
                    "t",
                    "REVIEW_PACKET",
                    json!({"assignment_ref":"a1",
                    "node":"_review_1","packet":{"changed_files":{"complete":true,"files":files}}}),
                )?;
                Ok(())
            })
            .unwrap();
        // The project changes while the review runs.
        std::fs::write(root.join("a.md"), "edited meanwhile\n").unwrap();
        std::fs::remove_file(root.join("gone.md")).unwrap();
        // Without a rebind the delivered packet stays as recorded.
        refresh();
        assert_eq!(packets(&store).len(), 1);

        store
            .write(|db| {
                lifecycle::event_tx(
                    db,
                    "t",
                    "REVIEW_STARTED",
                    json!({"assignment_ref":"a1",
                    "revision_digest":"r2","rebound_from":"r1","rebind":1}),
                )?;
                Ok(())
            })
            .unwrap();
        refresh();
        let all = packets(&store);
        assert_eq!(all.len(), 2);
        let files = &all[1]["packet"]["changed_files"]["files"];
        assert_eq!(files[0]["current_sha256"], sha(b"edited meanwhile\n"));
        assert_eq!(files[1]["current_sha256"], Value::Null);
        assert_eq!(files[2], json!({"path":"was.md","change":"deleted"}));
        assert_eq!(all[1]["assignment_ref"], "a1");
        assert_eq!(all[1]["packet"]["changed_files"]["complete"], true);
        // Recorded once per rebind, never rewritten while the project is unchanged.
        refresh();
        assert_eq!(packets(&store).len(), 2);
        let _ = std::fs::remove_dir_all(&root);
    }
}
#[cfg(test)]
mod refusal_limit_tests {
    use super::*;
    use cortex_core::{
        lifecycle::{FoundationGuards, TaskMachine},
        OpenMode, ProjectKey, StoreChannel, TaskState,
    };
    use std::os::unix::fs::PermissionsExt;
    fn fixture(stage: &str) -> (std::path::PathBuf, Store, Credential, Identity) {
        let root = std::env::temp_dir().join(format!(
            "native-refusal-limit-{}",
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let store = Store::open(
            ProjectKey::new(&root, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        let owner = TaskMachine::new(&store)
            .create("t", "goal", "owner")
            .unwrap();
        TaskMachine::new(&store)
            .transition(
                "t",
                &owner,
                "planning",
                TaskState::Planning,
                None,
                &FoundationGuards,
            )
            .unwrap();
        let c = store
            .write(|db| channels::issue_tx(db, "t", &owner, channels::Principal::Coordinator))
            .unwrap();
        let i = Identity {
            read_only: false,
            task: "t".into(),
            attempt: "xa_fixture".into(),
            principal: "fixture".into(),
            epoch: 1,
            role: "worker".into(),
            profile: Some(
                if stage == "review" {
                    "worker-code-reviewer"
                } else {
                    "worker-backend-dev"
                }
                .into(),
            ),
            stage: Some(stage.into()),
            assignment: Some("a".into()),
            session: None,
        };
        (root, store, c, i)
    }
    /// Owner stores (audit 2026-11-22): `review_coverage_unfinished`,
    /// `assignment_blocker_required`, `open_failed_check` and others were refused
    /// without bound inside one turn. The third identical refusal is relieved: a
    /// review is recorded INCOMPLETE with the parts disclosed, a failed check makes
    /// the assignment failed with that check as its blocker; never a pass.
    #[test]
    fn the_third_identical_refusal_is_relieved_never_as_a_pass() {
        let (root, store, c, i) = fixture("review");
        let args = json!({"verdict":"INCOMPLETE","report":"r","summary":"s","checks":[],
            "not_covered":[{"item":"channels/life-is-broken/README.md","reason":"not read yet"}]});
        let code = "review_coverage_unfinished";
        let items = vec!["channels/life-is-broken/README.md".to_string()];
        assert!(relieve(&store, &c, &i, &args, code).unwrap().is_none());
        record_refusal(&store, &c, &i, code, Some(&items)).unwrap();
        assert!(relieve(&store, &c, &i, &args, code).unwrap().is_none());
        // A different item set is a different loop.
        record_refusal(&store, &c, &i, code, Some(&["other".to_string()])).unwrap();
        assert!(relieve(&store, &c, &i, &args, code).unwrap().is_none());
        record_refusal(&store, &c, &i, code, Some(&items)).unwrap();
        let (relieved, relief, action, missing) =
            relieve(&store, &c, &i, &args, code).unwrap().unwrap();
        assert_eq!(action, "disclosed");
        assert_eq!(missing, items);
        assert_eq!(relieved["verdict"], "INCOMPLETE");
        assert_eq!(relieved["not_covered"][0]["blocker"], "other_role_work");
        assert_eq!(relief.limits[0]["outcome"], "unverified");
        assert_eq!(relief.limits[0]["source"], "runtime");
        assert!(lifecycle::review::unfinished_coverage(&relieved).is_empty());
        // An APPROVE carrying an open failed check becomes INCOMPLETE, never a pass.
        let approve = json!({"verdict":"APPROVE","report":"r","summary":"s","checks":[]});
        for _ in 0..2 {
            record_refusal(&store, &c, &i, "open_failed_check", None).unwrap();
        }
        let (relieved, _, action, _) = relieve(&store, &c, &i, &approve, "open_failed_check")
            .unwrap()
            .unwrap();
        assert_eq!(action, "failed_assignment");
        assert_eq!(relieved["verdict"], "INCOMPLETE");
        // Checkout drift is never the reviewer's to correct and is never relieved.
        for _ in 0..3 {
            record_refusal(&store, &c, &i, "review_project_revision_unstable", None).unwrap();
        }
        assert!(
            relieve(&store, &c, &i, &approve, "review_project_revision_unstable")
                .unwrap()
                .is_none()
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn an_execution_worker_with_an_open_failed_check_fails_with_that_check_as_blocker() {
        let (root, store, c, i) = fixture("execution");
        let args = json!({"report":"r","summary":"s","checks":[{"name":"tests","outcome":"failed","tool_call_ref":"f"}]});
        for _ in 0..2 {
            record_refusal(&store, &c, &i, "open_failed_check", None).unwrap();
        }
        let (relieved, _, action, _) = relieve(&store, &c, &i, &args, "open_failed_check")
            .unwrap()
            .unwrap();
        assert_eq!(action, "failed_assignment");
        assert_eq!(relieved["assignment_outcome"], "failed");
        assert_eq!(relieved["blocker"]["kind"], "other_role_work");
        assert_eq!(relieved["checks"][0]["outcome"], "failed");
        // A failed outcome without a dead end is recorded with one.
        let failed = json!({"report":"r","summary":"s","checks":[],"assignment_outcome":"failed"});
        for _ in 0..2 {
            record_refusal(&store, &c, &i, "assignment_blocker_required", None).unwrap();
        }
        let (relieved, _, _, _) = relieve(&store, &c, &i, &failed, "assignment_blocker_required")
            .unwrap()
            .unwrap();
        assert!(lifecycle::review::valid_blocker(
            &relieved["blocker"]["kind"]
        ));
        // A check without a blocker is recorded unverified with the runtime's disclosure.
        let unblocked =
            json!({"report":"r","summary":"s","checks":[{"name":"live","outcome":"not_run"}]});
        for _ in 0..2 {
            record_refusal(&store, &c, &i, "check_blocker_required", None).unwrap();
        }
        let (relieved, _, action, _) =
            relieve(&store, &c, &i, &unblocked, "check_blocker_required")
                .unwrap()
                .unwrap();
        assert_eq!(action, "disclosed");
        assert_eq!(relieved["checks"][0]["outcome"], "not_run");
        assert!(relieved["checks"][0]["blocker"]
            .as_str()
            .is_some_and(|b| !b.is_empty()));
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
