//! Coordinator proposals after runtime verification. No model pass flag is evidence.
use super::{domain, event_tx, plan, request_tx};
use crate::{error::refuse, CoreError, CoreResult, TaskState};
use rusqlite::{Connection, OptionalExtension, Transaction};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

fn normalized(s: &str) -> String {
    s.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn results(db: &Connection, task: &str) -> CoreResult<Vec<Value>> {
    Ok(db
        .prepare(
            "SELECT result_json FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
        WHERE n.task_id=? AND n.revision=t.revision AND n.result_json IS NOT NULL ORDER BY n.rowid",
        )?
        .query_map([task], |r| r.get::<_, String>(0))?
        .map(|r| Ok(serde_json::from_str(&r?)?))
        .collect::<CoreResult<Vec<_>>>()?)
}
/// A runtime pass closes a declared-command gap only on the current reviewed
/// revision. Command-less gaps remain disclosed; unrelated passes cannot hide them.
pub fn gaps(db: &Connection, task: &str) -> CoreResult<BTreeMap<String, String>> {
    let verified: Option<(String,String)> = db.query_row("SELECT type,payload_json FROM runtime_events
        WHERE task_id=? AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1",
        [task], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
    let verified: Value = verified
        .filter(|(kind, _)| kind == "VERIFICATION_PASSED")
        .map(|(_, s)| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or(Value::Null);
    let current: i64 = db.query_row(
        "SELECT revision FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let mut gaps = BTreeMap::new();
    for result in results(db, task)? {
        for check in result["checks"].as_array().into_iter().flatten() {
            if !matches!(check["outcome"].as_str(), Some("not_run" | "unverified")) {
                continue;
            }
            let closed = check["command"].as_array().is_some_and(|a| !a.is_empty())
                && verified["revision"] == current
                && verified["checks"].as_array().is_some_and(|a| {
                    a.iter().any(|c| {
                        c["argv"] == check["command"]
                            && c["complete"] == true
                            && c["exit_code"] == 0
                            && c["revision_digest"] == verified["revision_digest"]
                    })
                });
            if !closed {
                let name = check["name"]
                    .as_str()
                    .ok_or(CoreError::Refused("invalid_worker_check"))?;
                gaps.insert(normalized(name), name.into());
            }
        }
    }
    // A limit the runtime disclosed when it broke a loop (`LOOP_BROKEN.limits`, for
    // example on a node of an earlier plan revision) stays a gap of the final result
    // until the task is reopened, so it reaches Not verified after a replan too.
    let broken = db
        .prepare(
            "SELECT payload_json FROM runtime_events WHERE task_id=?1 AND type='LOOP_BROKEN'
             AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0)
             ORDER BY id",
        )?
        .query_map([task], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for raw in broken {
        let v: Value = serde_json::from_str(&raw)?;
        for limit in v["limits"].as_array().into_iter().flatten() {
            if !matches!(limit["outcome"].as_str(), Some("not_run" | "unverified")) {
                continue;
            }
            if let Some(name) = limit["name"].as_str().filter(|n| !n.trim().is_empty()) {
                gaps.insert(normalized(name), name.into());
            }
        }
    }
    Ok(gaps)
}
fn authored_disclosure(text: &str) -> bool {
    const HEADINGS: &[&str] = &[
        "not verified",
        "what is not verified",
        "what was not verified",
        "what was not checked",
        "what was not tested",
        "unverified",
        "unverified checks",
        "unverified items",
        "verification gaps",
        "not checked",
        "not tested",
        "checks not run",
        "tests not run",
        "skipped checks",
        "skipped tests",
        "не проверено",
        "что не проверено",
        "что не было проверено",
        "что осталось непроверенным",
        "непроверенное",
        "непроверенные проверки",
        "непроверенные пункты",
        "не выполнено",
        "невыполненные проверки",
        "не запускалось",
    ];
    let reserved = |line: &str| {
        HEADINGS.contains(
            &normalized(
                &line
                    .replace(['*', '_', '`'], "")
                    .trim_matches([' ', '\t', ':', '#']),
            )
            .as_str(),
        )
    };
    let mut fence: Option<(char, usize)> = None;
    let mut previous = "";
    for line in text.lines() {
        let trimmed = line.trim_start_matches(' ');
        let indentation = line.len() - trimmed.len();
        let marker = trimmed.chars().next().filter(|c| matches!(c, '`' | '~'));
        let count = marker.map_or(0, |c| trimmed.chars().take_while(|v| *v == c).count());
        if let Some((c, n)) = fence {
            if indentation <= 3
                && marker == Some(c)
                && count >= n
                && trimmed[count..].trim().is_empty()
            {
                fence = None;
            }
            continue;
        }
        if indentation <= 3 && count >= 3 {
            fence = Some((marker.unwrap(), count));
            previous = "";
            continue;
        }
        if indentation >= 4 || line.starts_with(['\t', '>']) {
            previous = "";
            continue;
        }
        let hash = trimmed.chars().take_while(|c| *c == '#').count();
        if indentation <= 3
            && (1..=6).contains(&hash)
            && trimmed[hash..].starts_with([' ', '\t'])
            && reserved(&trimmed[hash..])
        {
            return true;
        }
        if indentation <= 3
            && !trimmed.is_empty()
            && (trimmed.trim_end().chars().all(|c| c == '=')
                || trimmed.trim_end().chars().all(|c| c == '-'))
            && reserved(previous)
        {
            return true;
        }
        previous = line;
    }
    false
}
/// The names a `not_verified` item may carry: a recorded check of the task (any
/// revision since it was last reopened) that did not pass, or a limit the runtime
/// itself recorded. Anything else (a deliverable that was not made, a step that was
/// skipped) is not a check gap: it is goal work, refused `not_verified_not_a_check`.
fn check_names(db: &Connection, task: &str) -> CoreResult<BTreeSet<String>> {
    let mut names: BTreeSet<String> = gaps(db, task)?.into_keys().collect();
    let rows = db
        .prepare(
            "SELECT c.value FROM runtime_nodes n, json_each(n.result_json,'$.checks') c
             WHERE n.task_id=?1 AND n.result_json IS NOT NULL AND json_valid(n.result_json)
             AND n.revision>=coalesce((SELECT json_extract(payload_json,'$.revision')+1 FROM runtime_events
               WHERE task_id=?1 AND type='TASK_REOPENED' ORDER BY id DESC LIMIT 1),0)",
        )?
        .query_map([task], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for raw in rows {
        let check: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        if check["outcome"] != "passed" {
            if let Some(name) = check["name"].as_str() {
                names.insert(normalized(name));
            }
        }
    }
    let limits = db
        .prepare(
            "SELECT payload_json FROM runtime_events WHERE task_id=?1 AND type IN
               ('LOOP_BROKEN','REVIEW_LIMIT_DISCLOSED','REVIEW_CHANGE_SET_LIMITED','PRIOR_WORKER_CHECK_DISCLOSED')
             AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0)",
        )?
        .query_map([task], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for raw in limits {
        let event: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        for limit in event["limits"].as_array().into_iter().flatten() {
            if let Some(name) = limit["name"].as_str().or(limit.as_str()) {
                names.insert(normalized(name));
            }
        }
    }
    // The runtime's own verification commands, named by their argv.
    let failed = db
        .prepare(
            "SELECT c.value FROM runtime_events e, json_each(e.payload_json,'$.checks') c
             WHERE e.task_id=? AND e.type IN ('VERIFICATION_FAILED','VERIFICATION_PASSED')",
        )?
        .query_map([task], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for raw in failed {
        let check: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
        if let Some(argv) = check["argv"].as_array() {
            let words: Vec<&str> = argv.iter().filter_map(Value::as_str).collect();
            names.insert(normalized(&words.join(" ")));
            names.insert(normalized(&check["argv"].to_string()));
        }
    }
    Ok(names)
}
/// How a final result reports each goal outcome. `allow_not_done` is the owner's
/// decision to close the task as is (an answer to a question asked after
/// verification or after a runtime stop): only then may an outcome stay not done,
/// and the returned list keeps it for the disclosure.
///
/// - Plan outcomes in force: every id exactly once, no unknown id.
/// - A plan accepted before plans named outcomes (legacy): the coordinator names
///   the goal's outcomes here instead; at least one.
/// - No plan (a direct informational answer): optional.
pub fn final_outcomes(
    db: &Connection,
    task: &str,
    submitted: &Value,
    allow_not_done: bool,
) -> CoreResult<Vec<Value>> {
    let revision: i64 = db.query_row(
        "SELECT revision FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let planned = plan::task_outcomes(db, task)?;
    let list = match submitted {
        Value::Null => Vec::new(),
        Value::Array(list) => list.clone(),
        _ => return refuse("invalid_final_outcomes"),
    };
    if list.is_empty() && (planned.is_some() || revision > 0) {
        return refuse("final_outcomes_required");
    }
    let mut seen = BTreeSet::new();
    for item in &list {
        let id = plan::outcome_id(&item["id"]).ok_or(CoreError::Refused("invalid_final_outcomes"))?;
        if !seen.insert(id.to_owned()) {
            return refuse("invalid_final_outcomes");
        }
        match item["status"].as_str() {
            Some("done") => {
                if item["evidence"]
                    .as_str()
                    .is_none_or(|e| e.trim().is_empty())
                {
                    return refuse("final_outcome_evidence_required");
                }
            }
            Some("not_done") => {}
            _ => return refuse("invalid_final_outcomes"),
        }
    }
    if let Some(planned) = &planned {
        let ids: BTreeSet<String> = planned
            .iter()
            .filter_map(|o| o["id"].as_str().map(str::to_owned))
            .collect();
        if seen.iter().any(|id| !ids.contains(id)) {
            return refuse("final_outcome_unknown");
        }
        if ids.iter().any(|id| !seen.contains(id)) {
            return refuse("final_outcomes_required");
        }
    }
    if !allow_not_done && list.iter().any(|o| o["status"] != "done") {
        return refuse("completion_goal_not_met");
    }
    Ok(list)
}
/// The plan outcomes a final result left out or did not do (for refusals).
pub fn unmet_outcomes(db: &Connection, task: &str, submitted: &Value) -> CoreResult<Vec<Value>> {
    let planned = plan::task_outcomes(db, task)?.unwrap_or_default();
    let list = submitted.as_array().cloned().unwrap_or_default();
    let mut out: Vec<Value> = planned
        .iter()
        .filter(|o| {
            !list
                .iter()
                .any(|s| s["id"] == o["id"] && s["status"] == "done")
        })
        .map(|o| json!({"id":o["id"],"statement":o["statement"]}))
        .collect();
    for s in &list {
        if s["status"] != "done" && !out.iter().any(|o| o["id"] == s["id"]) {
            out.push(json!({"id":s["id"]}));
        }
    }
    Ok(out)
}
/// Whether the recorded final result of `kind` (`SYNTHESIS_SUBMITTED`,
/// `SYNTHESIS_RUNTIME_COMPOSED` or `ANSWER_SUBMITTED`) reports every goal outcome
/// done, or not done only by the owner's close-as-is decision. A plan and final
/// result both recorded before outcomes existed (legacy) pass unchanged.
pub fn goal_met(db: &Connection, task: &str, payload: &Value) -> CoreResult<bool> {
    let planned = plan::task_outcomes(db, task)?;
    let Some(list) = payload["outcomes"].as_array() else {
        return Ok(planned.is_none());
    };
    let owner = payload["owner_closed_as_is"] == true;
    if list.iter().any(|o| o["status"] != "done") && !owner {
        return Ok(false);
    }
    Ok(planned.unwrap_or_default().iter().all(|o| {
        list.iter()
            .any(|s| s["id"] == o["id"] && (s["status"] == "done" || owner))
    }))
}
pub fn disclosed(db: &Connection, task: &str, text: &str, entries: &Value) -> CoreResult<String> {
    if text.trim().is_empty() {
        return refuse("synthesis_text_required");
    }
    let empty = vec![];
    let entries = if entries.is_null() {
        &empty
    } else {
        entries
            .as_array()
            .ok_or(CoreError::Refused("invalid_not_verified"))?
    };
    let mut listed = BTreeSet::new();
    let mut groups: Vec<(String, String, Vec<String>)> = Vec::new();
    for item in entries {
        let blocker = item["blocker"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or(CoreError::Refused("synthesis_blocker_required"))?;
        let name = item["item"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .ok_or(CoreError::Refused("invalid_not_verified"))?;
        listed.insert(normalized(name));
        let label = item["description"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(name)
            .to_owned();
        let key = normalized(blocker);
        if let Some(g) = groups.iter_mut().find(|g| g.0 == key) {
            g.2.push(label);
        } else {
            groups.push((key, blocker.into(), vec![label]));
        }
    }
    if gaps(db, task)?.keys().any(|s| !listed.contains(s)) {
        return refuse("synthesis_gaps_undisclosed");
    }
    if !listed.is_empty() {
        let known = check_names(db, task)?;
        if listed.iter().any(|s| !known.contains(s)) {
            return refuse("not_verified_not_a_check");
        }
    }
    if entries.is_empty() {
        return Ok(text.into());
    }
    if authored_disclosure(text) {
        return refuse("synthesis_disclosure_in_text");
    }
    let (heading, label) = if domain::task_language_tx(db, task)? == "ru" {
        ("Не проверено", "причина")
    } else {
        ("Not verified", "blocker")
    };
    Ok(format!(
        "{}\n\n## {heading}\n\n{}",
        text.trim_end(),
        groups
            .into_iter()
            .map(|(_, blocker, names)| format!("- {} — {label}: {blocker}", names.join("; ")))
            .collect::<Vec<_>>()
            .join("\n")
    ))
}
pub fn submit_synthesis_tx(db: &Transaction<'_>, task: &str, args: &Value) -> CoreResult<Value> {
    if domain::task_tx(db, task)?.0 != TaskState::Reconciling {
        return refuse("synthesis_not_expected");
    }
    let text = disclosed(
        db,
        task,
        args["text"]
            .as_str()
            .ok_or(CoreError::Refused("synthesis_text_required"))?,
        &args["not_verified"],
    )?;
    // A final result only for a met goal: an outcome not done is continued
    // (replan_request) or put to the owner (user_question_request), never synthesized.
    let outcomes = final_outcomes(db, task, &args["outcomes"], false)?;
    let previous: Option<String> = db.query_row(
        "SELECT synthesis FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    if let Some(previous) = previous {
        if previous != text {
            return refuse("synthesis_conflict");
        }
    } else {
        db.execute(
            "UPDATE runtime_tasks SET synthesis=? WHERE task_id=?",
            (&text, task),
        )?;
        event_tx(
            db,
            task,
            "SYNTHESIS_SUBMITTED",
            json!({"text":text,"not_verified":args["not_verified"],"outcomes":outcomes}),
        )?;
    }
    Ok(json!({"recorded":true,"state":"RECONCILING"}))
}
const MAX_MANIFEST_FILES: usize = 20_000;
/// Final text of a runtime-composed synthesis never exceeds this many bytes.
pub const SYNTHESIS_BYTE_LIMIT: usize = 48 * 1024;
/// Total characters of quoted execution summaries.
pub const SUMMARY_CHAR_LIMIT: usize = 1_600;
const BASELINE_PHASE: &str = "execution_baseline";
fn baseline_attempt(task: &str) -> String {
    format!("baseline:{task}")
}
/// Retain the per-file content manifest observed with the execution baseline, so
/// the files changed by the work can later be listed. A project too large to keep
/// stores no manifest, and the fast path then stays with the coordinator.
pub fn record_baseline_manifest_tx(
    db: &Transaction<'_>,
    task: &str,
    project_digest: &str,
    manifest: &BTreeMap<String, String>,
) -> CoreResult<()> {
    let attempt = baseline_attempt(task);
    // The reference is the first baseline since the task was last reopened, so a
    // later baseline (after a resume or a replan) never replaces it.
    let first: Option<String> = db
        .query_row(
            "SELECT json_extract(payload_json,'$.revision_digest') FROM runtime_events WHERE task_id=? AND type='EXECUTION_BASELINE'
            AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=? AND type='TASK_REOPENED'),0)
            ORDER BY id LIMIT 1",
            (task, task),
            |r| r.get(0),
        )
        .optional()?
        .flatten();
    if first.as_deref() != Some(project_digest) {
        return Ok(());
    }
    db.execute(
        "DELETE FROM attempt_revisions WHERE attempt_id=? AND phase=?",
        (&attempt, BASELINE_PHASE),
    )?;
    if manifest.len() > MAX_MANIFEST_FILES {
        return Ok(());
    }
    db.execute(
        "INSERT INTO attempt_revisions(attempt_id,captured_at,phase,kind,dirty_digest,manifest_json) VALUES (?,?,?,'native',?,?)",
        (&attempt, crate::now(), BASELINE_PHASE, project_digest, serde_json::to_string(manifest)?),
    )?;
    Ok(())
}
/// How Git-ignored files compare with the execution baseline. They are not
/// deliverable content (build outputs, dependency folders, caches), so this never
/// decides whether the deliverable change set is complete; it only adds a note.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum IgnoredChange {
    Unchanged,
    Changed,
    Unobserved,
}
/// The deliverable files that differ from the execution baseline, and how the
/// ignored files compare.
struct ChangeSet {
    files: Vec<String>,
    ignored: IgnoredChange,
}
/// Files whose content differs from the first execution baseline since the task was
/// last reopened. None when that baseline metadata is missing or not that baseline.
fn changed_files(
    db: &Connection,
    task: &str,
    current: Option<&BTreeMap<String, String>>,
    current_ignored: Option<&str>,
) -> CoreResult<Option<ChangeSet>> {
    // No trustworthy manifest of the project now (incomplete or metadata-only
    // observation): the changed-file list cannot be built.
    let Some(current) = current else {
        return Ok(None);
    };
    let baseline: Option<String> = db
        .query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='EXECUTION_BASELINE'
            AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=? AND type='TASK_REOPENED'),0)
            ORDER BY id LIMIT 1",
            (task, task),
            |r| r.get(0),
        )
        .optional()?;
    let Some(baseline) = baseline else {
        return Ok(None);
    };
    let baseline: Value = serde_json::from_str(&baseline)?;
    let Some(digest) = baseline["revision_digest"]
        .as_str()
        .filter(|_| baseline["kind"] == "observed")
    else {
        return Ok(None);
    };
    let stored: Option<(Option<String>, Option<String>)> = db
        .query_row(
            "SELECT dirty_digest,manifest_json FROM attempt_revisions WHERE attempt_id=? AND phase=? ORDER BY rowid DESC LIMIT 1",
            (baseline_attempt(task), BASELINE_PHASE),
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((Some(stored_digest), Some(manifest))) = stored else {
        return Ok(None);
    };
    if stored_digest != digest {
        return Ok(None);
    }
    // Both manifests list every deliverable path (tracked and untracked, not ignored),
    // so their difference is the complete deliverable change set. Ignored files are
    // not deliverable: their metadata digest only says whether they changed. Writes
    // the task's own tools made under ignored paths are still listed by the packet
    // from their write receipts.
    let ignored = match (baseline["ignored_digest"].as_str(), current_ignored) {
        (Some(before), Some(now)) if before == now => IgnoredChange::Unchanged,
        (Some(_), Some(_)) => IgnoredChange::Changed,
        _ => IgnoredChange::Unobserved,
    };
    let before: BTreeMap<String, String> = serde_json::from_str(&manifest)?;
    let names: BTreeSet<&String> = before.keys().chain(current.keys()).collect();
    Ok(Some(ChangeSet {
        files: names
            .into_iter()
            .filter(|n| before.get(*n) != current.get(*n))
            .cloned()
            .collect(),
        ignored,
    }))
}
/// Changed files listed in a review brief; the rest are counted by folder.
pub const REVIEW_PACKET_FILES: usize = 200;
/// A review binds at most this many task files by their bytes; a larger change set
/// binds the whole checkout instead (the same bound as a check's work set).
pub const REVIEW_BINDING_FILES: usize = 20_000;
/// Whether the task recorded every path its own commands and writing turns changed,
/// and those paths with the plan node that changed them. A file a gateway tool
/// wrote has a write receipt; the host also lists the checkout's dirty paths around
/// each command (`NATIVE_COMMAND_FINISHED`, `changed_paths`) and around each turn of
/// a writing worker whose engine could change files without a receipt
/// (`NATIVE_WRITER_TURN_CHANGES`, `native_mutation`: an engine that writes inside
/// its own sandbox, or a native or external tool the runtime allowed in that turn).
/// A record counts from its start or finish after `since` (the execution baseline):
/// earlier changes are part of the baseline itself. `Err` names why attribution is
/// incomplete: a command or writing turn without a complete change record (an
/// earlier build, a recovered or unknown outcome, a failed or oversized listing),
/// or one started but never settled.
fn recorded_changes(
    db: &Connection,
    task: &str,
    since: Option<i64>,
) -> CoreResult<(BTreeMap<String, BTreeSet<String>>, Result<(), String>)> {
    let mut statement = db.prepare(
        "SELECT e.id,e.type,e.payload_json,n.node_id,n.stage FROM runtime_events e
         LEFT JOIN trusted_channels c ON c.task_id=e.task_id AND c.channel_id=json_extract(e.payload_json,'$.channel_id')
         LEFT JOIN runtime_nodes n ON n.task_id=c.task_id AND n.assignment_ref=c.assignment_id
         WHERE e.task_id=? AND e.type IN ('NATIVE_COMMAND_STARTED','NATIVE_COMMAND_FINISHED',
           'NATIVE_WRITER_TURN_STARTED','NATIVE_WRITER_TURN_CHANGES','NATIVE_TURN_SETTLED') ORDER BY e.id",
    )?;
    let mut rows = statement.query([task])?;
    let since = since.unwrap_or(0);
    // A record's plan node comes from its start event, which names the channel.
    let mut started: BTreeMap<(bool, String), (i64, Option<String>)> = BTreeMap::new();
    let mut finished: BTreeSet<(bool, String)> = BTreeSet::new();
    let mut paths: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut complete = Ok(());
    let mut fail = |id: i64, reason: &str| {
        if id > since && complete.is_ok() {
            complete = Err(reason.to_owned());
        }
    };
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let kind: String = row.get(1)?;
        let payload: Value = serde_json::from_str(&row.get::<_, String>(2)?)?;
        let node: Option<String> = row.get(3)?;
        let stage: Option<String> = row.get(4)?;
        let (turn, reference, observation) = match kind.as_str() {
            "NATIVE_COMMAND_STARTED" | "NATIVE_WRITER_TURN_STARTED" => {
                let turn = kind == "NATIVE_WRITER_TURN_STARTED";
                let key = if turn {
                    &payload["turn"]
                } else {
                    &payload["ref"]
                };
                if let Some(reference) = key.as_str() {
                    started.insert((turn, reference.to_owned()), (id, node));
                }
                continue;
            }
            // A turn of an execution worker settled by an earlier build, which kept
            // no record of whether its engine changed files.
            "NATIVE_TURN_SETTLED" => {
                if payload.get("writer").is_none() && stage.as_deref() == Some("execution") {
                    fail(id, "turn_without_change_record");
                }
                continue;
            }
            "NATIVE_WRITER_TURN_CHANGES" => (true, payload["turn"].as_str(), &payload),
            _ => (
                false,
                payload["receipt"]["tool_call_ref"].as_str(),
                &payload["receipt"]["observation"],
            ),
        };
        let reference = reference.map(|r| (turn, r.to_owned()));
        let (start, started_by) = reference
            .as_ref()
            .and_then(|r| started.get(r))
            .cloned()
            .unwrap_or((id, None));
        let node = node.or(started_by);
        if let Some(reference) = reference {
            finished.insert(reference);
        }
        // A writing turn whose engine could not change files without a receipt.
        if turn && observation["native_mutation"] == false {
            continue;
        }
        let changed = observation["changed_paths"].as_array();
        if observation["changed_paths_complete"] != true || changed.is_none() {
            let reason = observation["changed_paths_reason"]
                .as_str()
                .unwrap_or(if turn {
                    "turn_without_change_record"
                } else {
                    "command_without_change_record"
                });
            fail(id.max(start), reason);
            continue;
        }
        for path in changed.into_iter().flatten().filter_map(Value::as_str) {
            let entry = paths.entry(path.to_owned()).or_default();
            entry.extend(node.clone());
        }
    }
    for ((turn, reference), (id, _)) in &started {
        if !finished.contains(&(*turn, reference.clone())) {
            fail(
                *id,
                if *turn {
                    "turn_unsettled"
                } else {
                    "command_unsettled"
                },
            );
        }
    }
    Ok((paths, complete))
}
/// Whether review must assume the work changed the checkout. Use task-attributed
/// effects, not the shared checkout delta (which can belong to another writer).
pub(crate) fn has_checkout_changes(db: &Connection, task: &str) -> CoreResult<bool> {
    let written: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND
         type IN ('NATIVE_PROJECT_WRITTEN','NATIVE_PROJECT_CHANGED',
                  'VERIFICATION_REVISION_CHANGED','RUNTIME_UNCERTAINTY_RECORDED'))",
        [task],
        |r| r.get(0),
    )?;
    if written {
        return Ok(true);
    }
    let (paths, complete) = recorded_changes(db, task, None)?;
    Ok(!paths.is_empty() || complete.is_err())
}

/// `read_report` characters per page.
const REPORT_PAGE_CHARS: i64 = 4000;
/// What a reviewer is handed besides its objective: the files the work under review
/// changed, the predecessor reports with their size, and its read scope.
///
/// The change list joins two runtime records: the difference between the per-file
/// manifest of the first execution baseline since the task was last reopened and
/// `current` (complete, including changes made by commands, when both manifests are
/// trustworthy), and the file writes and changes the task's sessions recorded across all
/// plans, including work preceding a recheck (`NATIVE_PROJECT_WRITTEN`, `NATIVE_PROJECT_CHANGED`), which also
/// name the plan node that made them. Git is not used: project files may be
/// untracked, or the project may have no repository. Without a usable manifest the
/// list is the recorded writes only and says so (`complete: false`).
///
/// A listed file is `required` (the review must read it) when this task wrote it:
/// a write receipt, or a path one of the task's commands changed while it ran
/// (`changed_paths` of `NATIVE_COMMAND_FINISHED`). Other changed files are listed
/// as context only: a shared checkout holds other actors' uncommitted work. When a
/// command since the baseline has no complete change record, attribution is
/// incomplete: the required set stays the task's recorded changes, other files stay
/// context, and the approval discloses the gap as an unverified limit. The packet's
/// `review_binding` names what the review binds: the task's own files
/// (`task_files`, their paths) or, when there are too many, the whole checkout. The host keeps it with the recorded packet, not in the
/// reviewer's brief.
pub fn review_packet_tx(
    db: &Connection,
    task: &str,
    review_node: &str,
    current: Option<&BTreeMap<String, String>>,
    current_ignored: Option<&str>,
    content_hashes: &BTreeMap<String, String>,
) -> CoreResult<Value> {
    let baseline_event: Option<i64> = db.query_row(
        "SELECT min(id) FROM runtime_events WHERE task_id=?1 AND type='EXECUTION_BASELINE'
         AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND type='TASK_REOPENED'),0)",
        [task], |r| r.get(0))?;
    let mut files: BTreeMap<String, (String, BTreeSet<String>)> = BTreeMap::new();
    let mut last_writes = BTreeMap::new();
    let manifest = changed_files(db, task, current, current_ignored)?;
    if let (Some(ChangeSet { files: changed, .. }), Some(current)) = (&manifest, current) {
        let stored: Option<String> = db
            .query_row(
                "SELECT manifest_json FROM attempt_revisions WHERE attempt_id=? AND phase=? ORDER BY rowid DESC LIMIT 1",
                (baseline_attempt(task), BASELINE_PHASE),
                |r| r.get(0),
            )
            .optional()?;
        let before: BTreeMap<String, String> = stored
            .map(|s| serde_json::from_str(&s))
            .transpose()?
            .unwrap_or_default();
        for path in changed {
            let kind = match (before.contains_key(path), current.contains_key(path)) {
                (false, true) => "added",
                (true, false) => "deleted",
                _ => "modified",
            };
            files.insert(path.clone(), (kind.into(), BTreeSet::new()));
        }
    }
    // A reopened task can ask for fresh review of work from earlier plans.
    // Keep the latest concrete write per path alongside the current baseline delta.
    let mut statement = db.prepare(
        "SELECT e.id, json_extract(e.payload_json,'$.path'), coalesce(json_extract(e.payload_json,'$.operation'),'written'),
                json_extract(e.payload_json,'$.sha256'), n.node_id, n.revision
         FROM runtime_events e LEFT JOIN trusted_channels c ON c.task_id=e.task_id
           AND c.channel_id=json_extract(e.payload_json,'$.channel_id')
         LEFT JOIN runtime_nodes n ON n.task_id=c.task_id AND n.assignment_ref=c.assignment_id
         WHERE e.task_id=? AND e.type IN ('NATIVE_PROJECT_WRITTEN','NATIVE_PROJECT_CHANGED')
           AND length(json_extract(e.payload_json,'$.path'))>0 ORDER BY e.id")?;
    let mut rows = statement.query([task])?;
    while let Some(row) = rows.next()? {
        let path: String = row.get(1)?;
        let operation: String = row.get(2)?;
        let sha256: Option<String> = row.get(3)?;
        let node: Option<String> = row.get(4)?;
        let revision: Option<i64> = row.get(5)?;
        let matches_current = sha256
            .as_ref()
            .zip(content_hashes.get(&path))
            .map(|(written, observed)| written == observed);
        let entry = files
            .entry(path.clone())
            .or_insert_with(|| (operation, BTreeSet::new()));
        if let Some(author) = &node {
            entry.1.insert(author.clone());
        }
        last_writes.insert(
            path,
            json!({"event_id":row.get::<_,i64>(0)?, "node":node,
            "revision":revision,"sha256":sha256,"matches_current":matches_current}),
        );
    }
    // Paths the task's own commands changed while they ran. With a complete
    // baseline comparison they only name the writer of a listed change (a path a
    // command touched and that now equals the baseline has nothing to review);
    // without one they are listed like recorded writes.
    let (commanded, attribution) = recorded_changes(db, task, baseline_event)?;
    for (path, nodes) in &commanded {
        let entry = match files.get_mut(path) {
            Some(entry) => entry,
            None if manifest.is_none() => files
                .entry(path.clone())
                .or_insert_with(|| ("modified".into(), BTreeSet::new())),
            None => continue,
        };
        entry.1.extend(nodes.iter().cloned());
    }
    let attributed = |path: &str| last_writes.contains_key(path) || commanded.contains_key(path);
    let attribution_complete = attribution.is_ok();
    // Only the task's own changes are required, whatever else is uncommitted in the
    // checkout. An incomplete record (a command or turn whose changes could not be
    // listed) never turns other actors' files into this task's review obligation:
    // they stay context, and the gap is disclosed as an unverified limit.
    let required = |path: &str| attributed(path);
    let total = files.len();
    let mut ordered: Vec<_> = files.iter().collect();
    if total > REVIEW_PACKET_FILES {
        // Keep the task's own changes in view when the baseline delta is large.
        ordered.sort_by_key(|(path, _)| (!attributed(path), !last_writes.contains_key(*path)));
    }
    let listed: Vec<Value> = ordered
        .iter()
        .copied()
        .take(REVIEW_PACKET_FILES)
        .map(|(path, (change, by))| {
            let mut file = json!({"path":path,"change":change,"by":by,"required":required(path)});
            // Bind the review obligation to the host's current manifest.  The
            // reviewer still has to read the bytes through the channel; this
            // value is only used by the runtime to reject stale/partial reads.
            if let Some(current_sha256) = content_hashes.get(path) {
                file["current_sha256"] = json!(current_sha256);
            }
            if let Some(last) = last_writes.get(path) {
                file["last_write"] = last.clone();
            }
            file
        })
        .collect();
    let mut folders: BTreeMap<String, usize> = BTreeMap::new();
    for (path, _) in ordered.iter().skip(REVIEW_PACKET_FILES) {
        let folder = path.rsplit_once('/').map_or(".", |(d, _)| d);
        *folders.entry(folder.to_owned()).or_default() += 1;
    }
    let complete = manifest.is_some();
    let required_total = files.keys().filter(|p| required(p)).count();
    let mut changed = json!({"files":listed,"total":total,"complete":complete,
        "required":required_total,"attribution_complete":attribution_complete,
        "source":if complete {"execution baseline manifest and recorded writes"} else {"recorded writes only"}});
    if !complete {
        changed["note"] = json!(if attribution_complete {
            "The folder could not be compared with the execution baseline, so only this task's recorded writes and the files its own commands changed are listed; inspect the predecessor reports for other changes."
        } else {
            "The folder could not be compared with the execution baseline, so files changed by commands are not listed; inspect the predecessor reports and the folder for them."
        });
    }
    if let Err(reason) = &attribution {
        changed["attribution"] = json!({"reason":reason,
            "note":"Some of this task's commands or turns have no complete record of the files they changed. Files with a write receipt or a recorded command change are required; other changed files are context only. Inspect them if the objective or predecessor reports point there."});
    }
    match manifest.as_ref().map(|m| m.ignored) {
        Some(IgnoredChange::Changed) => {
            changed["ignored_files"] = json!("Build outputs and other git-ignored files changed; they are not deliverable and are not listed.");
        }
        Some(IgnoredChange::Unobserved) => {
            changed["ignored_files"] = json!("Git-ignored files (build outputs and the like) could not be compared with the execution baseline; they are not deliverable and are not listed.");
        }
        _ => {}
    }
    if total > listed.len() {
        let unlisted_required = ordered
            .iter()
            .skip(REVIEW_PACKET_FILES)
            .filter(|(p, _)| required(p))
            .count();
        changed["more"] = json!({"count":total - listed.len(),"required":unlisted_required,
            "by_folder":folders.iter().map(|(f, n)| json!({"folder":f,"files":n})).collect::<Vec<_>>(),
            "how":"read these folders with project_read to see the rest"});
    }
    let boundary = json!({"baseline_event":baseline_event,
        "recorded_task_paths":last_writes.len(),
        "command_changed_paths":commanded.len(),
        "guidance":"Use the objective and task write receipts across plans to focus your review. Files with required true are this task's own changes (a write receipt, or a path changed while one of its commands ran): read each of them. Files with required false changed in the checkout without a writer from this task; they are context only and never required. A matching hash binds the observed file bytes to the latest recorded write. For changed or unobserved bytes, inspect the relevant file and predecessor reports. Manifest-only paths have no confirmed writer. A whole-checkout diff may include unrelated work."});
    // What the review binds: the bytes of the task's own files, unless the set is
    // too large to hash, when it is the whole checkout. Other actors' edits never
    // make the review stale.
    let bound: Vec<&String> = files.keys().filter(|p| required(p)).collect();
    let binding = if bound.len() > REVIEW_BINDING_FILES {
        json!({"kind":"checkout","reason":"too_many_task_files"})
    } else {
        json!({"kind":"task_files","paths":bound})
    };
    let reports = db
        .prepare(
            "SELECT n.node_id,n.role,json_extract(n.result_json,'$.report_ref'),
                (SELECT size_bytes FROM editions d WHERE d.report_id=json_extract(n.result_json,'$.report_ref') ORDER BY d.sequence DESC LIMIT 1),
                (SELECT substr(title,1,200) FROM editions d WHERE d.report_id=json_extract(n.result_json,'$.report_ref') ORDER BY d.sequence DESC LIMIT 1),
                json_extract(n.result_json,'$.report_structure.status')
             FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
             WHERE n.task_id=? AND n.revision=t.revision AND n.stage!='review' AND n.status='completed'
             AND json_type(n.result_json,'$.report_ref')='text' ORDER BY n.rowid",
        )?
        .query_map([task], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?))
        })?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|(node, role, report, bytes, title, structure)| {
            let bytes = bytes.unwrap_or(0);
            let mut entry = json!({"report_id":report,"node":node,"role":role.trim_start_matches("worker-"),"title":title,"size_bytes":bytes,
                "pages_at_most":((bytes + REPORT_PAGE_CHARS - 1) / REPORT_PAGE_CHARS).max(1),"page_characters":REPORT_PAGE_CHARS});
            // A report accepted without its required sections is marked for the reviewer.
            if let Some(structure) = structure {
                entry["report_structure"] = json!(structure);
            }
            entry
        })
        .collect::<Vec<_>>();
    let scope: Option<String> = db
        .query_row(
            "SELECT n.scope_json FROM runtime_nodes n JOIN runtime_tasks t USING(task_id) WHERE n.task_id=? AND n.revision=t.revision AND n.node_id=?",
            (task, review_node),
            |r| r.get(0),
        )
        .optional()?;
    let scope: Vec<String> = scope
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or_default();
    // The read scope must cover what the review is required to read.
    let outside: Vec<&String> = files
        .keys()
        .filter(|p| required(p))
        .filter(|p| {
            !scope
                .iter()
                .any(|s| crate::scheduler::policy::overlaps(s, p))
        })
        .collect();
    Ok(
        json!({"changed_files":changed,"change_boundary":boundary,"predecessor_reports":reports,"read_scope":scope,
        "read_scope_covers_change":outside.is_empty(),"outside_read_scope":outside,"review_binding":binding,
        "goal":review_goal(db, task)?}),
    )
}
/// Most characters of the goal and of each owner message in a review packet.
const GOAL_CHARS: usize = 12_000;
const OWNER_MESSAGE_CHARS: usize = 2_000;
const OWNER_MESSAGES: usize = 12;
/// What the review judges the work against: the task's original goal verbatim, the
/// owner's messages and answers since (newest last), and the plan's goal outcomes.
/// An approval of the stage that leaves an outcome undelivered does not reach
/// verification (`review::advance_tx`).
fn review_goal(db: &Connection, task: &str) -> CoreResult<Value> {
    let goal: String = db.query_row(
        "SELECT goal FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let clip = |s: &str, n: usize| -> Value {
        let count = s.chars().count();
        if count <= n {
            json!({"text":s})
        } else {
            json!({"text":s.chars().take(n).collect::<String>(),"truncated":true,"characters":count,
                "source":"read_capsule constraints"})
        }
    };
    let mut messages = db
        .prepare(
            "SELECT * FROM (SELECT id,'steering',json_extract(payload_json,'$.text'),NULL FROM runtime_events
               WHERE task_id=?1 AND type='USER_STEERING' AND json_type(payload_json,'$.text')='text'
             UNION ALL SELECT e.id,'answer',q.answer_text,q.text FROM runtime_events e
               JOIN pending_questions q ON q.task_id=e.task_id AND q.id=json_extract(e.payload_json,'$.question_id')
               WHERE e.task_id=?1 AND e.type='USER_ANSWER_RECEIVED' AND q.answer_text IS NOT NULL)
             ORDER BY 1 DESC LIMIT ?2",
        )?
        .query_map((task, OWNER_MESSAGES as i64), |r| {
            Ok((
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    messages.reverse();
    let messages: Vec<Value> = messages
        .into_iter()
        .filter_map(|(kind, text, question)| {
            let mut m = clip(&text?, OWNER_MESSAGE_CHARS);
            m["kind"] = json!(kind);
            if let Some(q) = question {
                m["question"] = json!(q.chars().take(500).collect::<String>());
            }
            Some(m)
        })
        .collect();
    let mut out = json!({"original":clip(&goal, GOAL_CHARS),"owner_messages":messages,
        "outcomes":plan::task_outcomes(db, task)?,
        "guidance":"Judge the work against the original goal, the owner's later messages and every plan outcome, not only against the objectives of this plan's tasks. A stage the plan narrowed or deferred is not the goal: report each outcome's delivery in your result. An approval that leaves an outcome undelivered sends the task back to its coordinator instead of to verification."});
    if out["outcomes"].is_null() {
        out["outcomes_note"] = json!("This plan was accepted before plans named goal outcomes; judge against the original goal and the owner's messages.");
    }
    Ok(out)
}
/// Characters that are control or invisible formatting (Unicode Cc/Cf) but not
/// ordinary whitespace.
fn hidden_character(c: char) -> bool {
    (c.is_control() && !c.is_whitespace())
        || matches!(c, '\u{00AD}' | '\u{061C}' | '\u{180E}' | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}' | '\u{FFF9}'..='\u{FFFB}' | '\u{E0001}' | '\u{E0020}'..='\u{E007F}')
}
fn escape_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if "\\`*_{}[]<>#|~&".contains(c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}
/// Whether the current revision's executed plan only investigated: every
/// execution node is a read-only role. Its result is findings, not the change a
/// task may ask for, so the coordinator (not the runtime) decides what follows
/// (owner store 2026-11-23: a task that had to agree and implement a change was
/// closed after its read-only inspection).
pub fn investigation_only(db: &rusqlite::Connection, task: &str) -> CoreResult<bool> {
    let roles = db
        .prepare(
            "SELECT n.role,n.stage FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
             WHERE n.task_id=? AND n.revision=t.revision AND n.stage='execution'",
        )?
        .query_map([task], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(!roles.is_empty()
        && roles
            .iter()
            .all(|(role, stage)| crate::scheduler::policy::read_only(role, stage)))
}
/// Compose the final text without a coordinator turn on the clean first-review
/// path, record it and `SYNTHESIS_RUNTIME_COMPOSED`. Returns None, changing
/// nothing, whenever any condition of that path does not hold; the coordinator
/// then reconciles. `current` is the manifest of the project at this moment and
/// `current_ignored` its ignored-files digest.
pub fn compose_runtime_synthesis_tx(
    db: &Transaction<'_>,
    task: &str,
    current: Option<&BTreeMap<String, String>>,
    current_ignored: Option<&str>,
) -> CoreResult<Option<String>> {
    if domain::task_tx(db, task)?.0 != TaskState::Reconciling {
        return Ok(None);
    }
    let (review_cycle, existing): (i64, Option<String>) = db.query_row(
        "SELECT review_cycle,synthesis FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    // A repair path, any replan, a waived or repeated review, or open owner input
    // keeps the coordinator.
    let blocked: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=?
            AND type IN ('REPLAN_REQUEST_ACCEPTED','RETURNED_TO_COORDINATOR','TASK_REOPENED',
                'VERIFICATION_REVISED','VERIFICATION_FAILED','REVIEW_CHANGES_REQUESTED','REVIEW_NOT_REQUIRED'))
         OR EXISTS(SELECT 1 FROM pending_questions WHERE task_id=? AND status='open')",
        (task, task),
        |r| r.get(0),
    )?;
    if existing.is_some()
        || blocked
        || review_cycle != 1
        || !gaps(db, task)?.is_empty()
        || investigation_only(db, task)?
    {
        return Ok(None);
    }
    // The goal outcomes are done only on the approving reviewer's goal coverage;
    // anything less keeps the coordinator.
    let mut outcomes = Vec::new();
    if let Some(planned) = plan::task_outcomes(db, task)? {
        let review: Option<String> = db
            .query_row(
                "SELECT n.result_json FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
                 WHERE n.task_id=? AND n.revision=t.revision AND n.stage='review' AND n.status='completed'
                 ORDER BY n.rowid DESC LIMIT 1",
                [task],
                |r| r.get(0),
            )
            .optional()?
            .flatten();
        let review: Value = review
            .and_then(|r| serde_json::from_str(&r).ok())
            .unwrap_or(Value::Null);
        for outcome in planned {
            let delivered = review["goal_coverage"].as_array().is_some_and(|c| {
                c.iter()
                    .any(|g| g["id"] == outcome["id"] && g["delivered"] == true)
            });
            if review["verdict"] != "APPROVE" || !delivered {
                return Ok(None);
            }
            outcomes.push(json!({"id":outcome["id"],"status":"done",
                "evidence":review["report_ref"].as_str().unwrap_or("approving review")}));
        }
    }
    let rows = db
        .prepare(
            "SELECT n.result_json FROM runtime_nodes n JOIN runtime_tasks t USING(task_id)
        WHERE n.task_id=? AND n.revision=t.revision AND n.stage='execution' AND n.result_json IS NOT NULL ORDER BY n.rowid",
        )?
        .query_map([task], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut summaries = Vec::new();
    let mut total = 0;
    for row in rows {
        let result: Value = serde_json::from_str(&row)?;
        let flat = result["summary"]
            .as_str()
            .map(|s| s.split_whitespace().collect::<Vec<_>>().join(" "))
            .unwrap_or_default();
        if flat.is_empty() || flat.chars().any(hidden_character) {
            return Ok(None);
        }
        total += flat.chars().count();
        summaries.push(escape_markdown(&flat));
    }
    if summaries.is_empty() || total > SUMMARY_CHAR_LIMIT {
        return Ok(None);
    }
    // Ignored files are not deliverable: whether they changed never keeps the
    // coordinator, and they are not listed as changed files.
    let Some(ChangeSet { files: changed, .. }) = changed_files(db, task, current, current_ignored)?
    else {
        return Ok(None);
    };
    if changed.iter().any(|f| {
        f.contains('`')
            || f.chars()
                .any(|c| hidden_character(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
    }) {
        return Ok(None);
    }
    let (done, summary_label, files_label) = if domain::task_language_tx(db, task)? == "ru" {
        (
            "Работа выполнена.",
            "Сводки работы (как их передали исполнители):",
            "Изменённые файлы:",
        )
    } else {
        (
            "The work is done.",
            "Work summaries (as submitted by the workers):",
            "Changed files:",
        )
    };
    let mut text = format!("{done}\n\n{summary_label}\n");
    for s in &summaries {
        text += &format!("- {s}\n");
    }
    if !changed.is_empty() {
        text += &format!("\n{files_label}\n");
        for f in &changed {
            text += &format!("- `{f}`\n");
        }
    }
    let text = text.trim_end().to_owned();
    if text.len() > SYNTHESIS_BYTE_LIMIT {
        return Ok(None);
    }
    db.execute(
        "UPDATE runtime_tasks SET synthesis=? WHERE task_id=?",
        (&text, task),
    )?;
    event_tx(
        db,
        task,
        "SYNTHESIS_RUNTIME_COMPOSED",
        json!({"text":text,"source":"retained-worker-summaries","summaries":summaries.len(),"changed_files":changed.len(),
            "outcomes":if outcomes.is_empty() { Value::Null } else { json!(outcomes) }}),
    )?;
    Ok(Some(text))
}
pub fn direct_answer_allowed(db: &Connection, task: &str) -> CoreResult<bool> {
    Ok(answer_grounds(db, task)?.is_some())
}
/// Why a direct answer is allowed: `informational` (no plan yet), `owner_closed`
/// (the owner answered a question asked after verification or a runtime stop
/// question, deciding how the task ends) or `steered` (an owner message after a
/// passed verification returned the work to planning). None: not allowed.
pub fn answer_grounds(db: &Connection, task: &str) -> CoreResult<Option<&'static str>> {
    let allowed = |v: bool, why: &'static str| Ok(v.then_some(why));
    if super::automatic::enabled(db, task)? || domain::owner_rework_needs_plan(db, task)? {
        return Ok(None);
    }
    let (revision,informational):(i64,bool)=db.query_row("SELECT revision,plan_json IS NULL
        AND NOT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=?)
        AND NOT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator')
        FROM runtime_tasks WHERE task_id=?",(task,task,task),|r|Ok((r.get(0)?,r.get(1)?)))?;
    if revision == 0 && informational {
        return Ok(Some("informational"));
    }
    let active:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND status IN ('pending','running'))",[task],|r|r.get(0))?;
    if active {
        return Ok(None);
    }
    let boundary:i64=db.query_row("SELECT coalesce(max(id),0) FROM runtime_events WHERE task_id=?
        AND type IN ('PLAN_ACCEPTED','ASSIGNMENT_CREATED','REPLAN_REQUEST_ACCEPTED','VERIFICATION_REVISED')",[task],|r|r.get(0))?;
    let stopped_release:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events q JOIN pending_questions a
        ON a.task_id=q.task_id AND a.id=json_extract(q.payload_json,'$.question_id') AND a.status='answered'
        AND a.answer_source IN ('text','elicitation')
        WHERE q.task_id=? AND q.type='USER_QUESTION_REQUESTED' AND q.id>? AND json_extract(q.payload_json,'$.revision')=?
        AND json_extract(q.payload_json,'$.runtime_stop')=1
        AND lower(trim(a.answer_text)) NOT IN ('1','2','continue','finish','продолжить','завершить',
            'continue: resume planning and ask for a new plan','continue: let the coordinator decide how to proceed',
            'finish: close the task as is, without certifying unfinished work',
            'продолжить: координатор решит, как действовать дальше',
            'завершить: закрыть задачу как есть, без подтверждения незавершённой работы',
            -- SQLite lower() folds ASCII only: the Russian options also as offered.
            'Продолжить','Завершить','Продолжить: координатор решит, как действовать дальше',
            'Завершить: закрыть задачу как есть, без подтверждения незавершённой работы'))",
        (task,boundary,revision),|r|r.get(0))?;
    let release:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events q JOIN runtime_events a ON a.task_id=q.task_id
        AND a.type='USER_ANSWER_RECEIVED' AND json_extract(a.payload_json,'$.question_id')=json_extract(q.payload_json,'$.question_id')
        WHERE q.task_id=? AND q.type='USER_QUESTION_REQUESTED' AND q.id>? AND json_extract(q.payload_json,'$.revision')=?
        AND json_extract(q.payload_json,'$.after_verification')=1)",(task,boundary,revision),|r|r.get(0))?;
    let steered:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events v JOIN runtime_events s ON s.task_id=v.task_id AND s.id>v.id
        AND s.type='STEERING_APPLIED' AND json_type(s.payload_json,'$.channel_id') IS NULL
        WHERE v.task_id=? AND v.type='VERIFICATION_PASSED' AND v.id>?
        AND json_extract(v.payload_json,'$.revision')=? AND NOT EXISTS(SELECT 1 FROM runtime_events f
            WHERE f.task_id=v.task_id AND f.id>v.id AND f.type='VERIFICATION_FAILED'))",(task,boundary,revision),|r|r.get(0))?;
    if release || stopped_release {
        return allowed(true, "owner_closed");
    }
    allowed(steered, "steered")
}
pub fn submit_answer_tx(
    db: &Transaction<'_>,
    task: &str,
    actor: &str,
    key: &str,
    args: &Value,
    evidence: Option<&super::completion::EvidenceSnapshot>,
) -> CoreResult<Value> {
    request_tx(db, task, actor, key, "answer_submit", args, || {
        let (state, revision) = domain::task_tx(db, task)?;
        let grounds = answer_grounds(db, task)?;
        if state != TaskState::Planning || grounds.is_none() {
            return refuse("direct_answer_not_allowed");
        }
        if revision > 0 {
            match super::completion::review_binding_tx(db, task) {
                Ok((_, reviewed)) => {
                    let evidence = evidence.ok_or(CoreError::Refused("project_revision_unavailable"))?;
                    if super::completion::current_binding_digest(db, task, evidence)? != reviewed {
                        return refuse("answer_project_revision_changed");
                    }
                }
                // An owner-released answer may explain work stopped before approval.
                // Its existing completion gate retains the answer and records the gap.
                Err(CoreError::Refused("completion_review_missing")) => {}
                Err(error) => return Err(error),
            }
        }
        // Planned work: the answer reports every goal outcome. Only the owner's own
        // decision about how the task ends lets an outcome stay not done, and the
        // runtime then discloses it below.
        let owner_closed = grounds == Some("owner_closed");
        let outcomes = final_outcomes(db, task, &args["outcomes"], owner_closed)?;
        let mut text = disclosed(
            db,
            task,
            args["text"]
                .as_str()
                .ok_or(CoreError::Refused("synthesis_text_required"))?,
            &args["not_verified"],
        )?;
        let unmet: Vec<String> = outcomes
            .iter()
            .filter(|o| o["status"] != "done")
            .map(|o| {
                plan::task_outcomes(db, task)
                    .ok()
                    .flatten()
                    .and_then(|p| p.into_iter().find(|p| p["id"] == o["id"]))
                    .and_then(|p| p["statement"].as_str().map(str::to_owned))
                    .unwrap_or_else(|| o["id"].as_str().unwrap_or("").to_owned())
            })
            .collect();
        if !unmet.is_empty() {
            let heading = if domain::task_language_tx(db, task)? == "ru" {
                "Цель достигнута не полностью (решение владельца)"
            } else {
                "Goal not fully met (owner's decision)"
            };
            text = format!(
                "{}\n\n## {heading}\n\n{}",
                text.trim_end(),
                unmet
                    .iter()
                    .map(|s| format!("- {s}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        db.execute(
            "UPDATE runtime_tasks SET synthesis=? WHERE task_id=?",
            (&text, task),
        )?;
        event_tx(
            db,
            task,
            "ANSWER_SUBMITTED",
            json!({"text":text,"not_verified":args["not_verified"],
                "outcomes":if outcomes.is_empty() { Value::Null } else { json!(outcomes) },
                "owner_closed_as_is":owner_closed && !unmet.is_empty()}),
        )?;
        domain::state_tx(db, task, TaskState::AnswerReady, None)?;
        Ok(json!({"recorded":true,"state":"ANSWER_READY"}))
    })
}
pub fn submit_verification_tx(
    db: &Transaction<'_>,
    task: &str,
    actor: &str,
    key: &str,
    args: &Value,
) -> CoreResult<Value> {
    request_tx(db, task, actor, key, "verification_submit", args, || {
        // An empty procedure only when the owner accepted every command of the plan as
        // not verified (checked below).
        let empty = args["verification"]
            .as_array()
            .is_some_and(|a| a.is_empty());
        let commands = if empty {
            Vec::new()
        } else {
            plan::verification(&args["verification"])?
        };
        let (state, revision) = domain::task_tx(db, task)?;
        let last:Option<(String,String)>=db.query_row("SELECT type,payload_json FROM runtime_events WHERE task_id=?
            AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED','PLAN_ACCEPTED','VERIFICATION_ENVIRONMENT_WAIT') ORDER BY id DESC LIMIT 1",
            [task],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        // A verification its environment kept failing, after the owner answered the
        // runtime's question about it, is repaired the same way: the coordinator may
        // change the procedure. That command never failed on the work, so leaving it out
        // is not a dropped failed check (an accepted one stays a disclosed limit).
        let last = last.map(|(kind, raw)| {
            let asked = kind == "VERIFICATION_ENVIRONMENT_WAIT"
                && serde_json::from_str::<Value>(&raw)
                    .is_ok_and(|w| w["owner_question"].is_string());
            if asked {
                (
                    "VERIFICATION_FAILED".to_owned(),
                    json!({"unresolved_commands":[]}).to_string(),
                )
            } else {
                (kind, raw)
            }
        });
        let Some((kind, raw)) = last else {
            return refuse("verification_repair_not_expected");
        };
        let ready:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND revision=?)
            AND NOT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND revision=? AND status!='completed')",
            (task,revision,task,revision),|r|r.get(0))?;
        if state != TaskState::Planning || kind != "VERIFICATION_FAILED" || !ready {
            return refuse("verification_repair_not_expected");
        }
        if super::automatic::consultation(db, task)?.is_some() {
            return refuse("automatic_consultation_plan_required");
        }
        super::automatic::require_failed_checks(db, task, &args["verification"])?;
        if empty {
            return accept_all_unverified_tx(db, task, revision);
        }
        let failed: Value = serde_json::from_str(&raw)?;
        // The identical procedure may be resubmitted once with nothing changed (one
        // rerun of a possibly flaky check). Again, with still nothing changed since (no
        // project write, no worker or outside command that could have changed the
        // environment, no owner input, no new plan), it would only repeat the same
        // failure: refused. The runtime's own rerun of an environmental outcome never
        // comes through here.
        let current: Value = db
            .query_row(
                "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
                [task],
                |r| r.get::<_, String>(0),
            )
            .ok()
            .and_then(|p| serde_json::from_str::<Value>(&p).ok())
            .map(|p| p["verification"].clone())
            .unwrap_or(Value::Null);
        if args["verification"] == current {
            let repeated: i64 = db.query_row(
                "SELECT count(*) FROM runtime_events WHERE task_id=?1 AND type='VERIFICATION_REVISED'
                 AND json_extract(payload_json,'$.verification')=json(?2)
                 AND id>coalesce((SELECT max(id) FROM runtime_events WHERE task_id=?1 AND (type IN
                   ('NATIVE_PROJECT_WRITTEN','NATIVE_PROJECT_CHANGED','NATIVE_OUTSIDE_SETTLED','NATIVE_WRITER_TURN_CHANGES',
                    'USER_STEERING','USER_ANSWER_RECEIVED','OWNER_ACTION_RESOLVED','TASK_REOPENED','PLAN_ACCEPTED')
                   OR type='NATIVE_COMMAND_STARTED' AND json_extract(payload_json,'$.channel_id') IS NOT NULL)),0)",
                (task, current.to_string()),
                |r| r.get(0),
            )?;
            if repeated >= 1 {
                return refuse("verification_unchanged");
            }
        }
        let raw: String = db.query_row(
            "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| r.get(0),
        )?;
        let mut plan: Value = serde_json::from_str(&raw)?;
        // A failed check that is absent from the new list was dropped or replaced,
        // whatever the list length. It stays an open obligation until the exact
        // command passes or the owner answers a task-global question.
        let mut dropped = vec![];
        for argv in super::completion::unresolved_commands(&failed, &plan["verification"]) {
            if !args["verification"].as_array().unwrap().contains(&argv) && !dropped.contains(&argv)
            {
                dropped.push(argv);
            }
        }
        // A trivial command that did not already belong to the plan cannot stand in
        // for a failed check: it would pass without checking anything.
        if !dropped.is_empty() {
            let known = plan["verification"].as_array().cloned().unwrap_or_default();
            for argv in &commands {
                if !known.contains(&json!(argv)) && super::completion::trivial_command(argv) {
                    return refuse("verification_trivial_replacement");
                }
            }
        }
        plan["verification"] = args["verification"].clone();
        db.execute(
            "UPDATE runtime_tasks SET plan_json=? WHERE task_id=?",
            (plan.to_string(), task),
        )?;
        event_tx(
            db,
            task,
            "VERIFICATION_REVISED",
            json!({"verification":commands,"dropped_failed":dropped}),
        )?;
        domain::state_tx(db, task, TaskState::Verifying, None)?;
        Ok(json!({"recorded":true,"state":"VERIFYING"}))
    })
}
/// Every verification command of the plan was accepted by the owner as not verified
/// (`automatic::owner_accepted_commands`, an explicit owner decision after the
/// runtime's loop question): the plan keeps no command, nothing is run and nothing
/// is recorded as passed (no `VERIFICATION_PASSED`). `VERIFICATION_REVISED` names the
/// accepted commands (`accepted_unverified`) and the task reconciles; completion
/// still requires the approving review's unchanged binding, and the accepted limits
/// are gaps the final result must list under Not verified.
fn accept_all_unverified_tx(db: &Transaction<'_>, task: &str, revision: i64) -> CoreResult<Value> {
    let raw: String = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let mut plan: Value = serde_json::from_str(&raw)?;
    let current = plan["verification"].as_array().cloned().unwrap_or_default();
    let accepted = super::automatic::owner_accepted_commands(db, task)?;
    if current.is_empty() || current.iter().any(|argv| !accepted.contains(argv)) {
        return refuse("verification_required");
    }
    plan["verification"] = json!([]);
    db.execute(
        "UPDATE runtime_tasks SET plan_json=? WHERE task_id=?",
        (plan.to_string(), task),
    )?;
    event_tx(
        db,
        task,
        "VERIFICATION_REVISED",
        json!({"verification":[],"dropped_failed":[],"accepted_unverified":current,"revision":revision}),
    )?;
    domain::state_tx(db, task, TaskState::Verifying, None)?;
    domain::state_tx(db, task, TaskState::Reconciling, None)?;
    Ok(json!({"recorded":true,"state":"RECONCILING","accepted_unverified":current}))
}
/// An owner-released answer cannot certify unfinished work. Keep the answer,
/// but fail explicitly on the same work-certification gaps as the retained runtime.
pub fn close_answer_tx(
    db: &Transaction<'_>,
    task: &str,
    evidence: &super::completion::EvidenceSnapshot,
) -> CoreResult<()> {
    let (state, revision) = domain::task_tx(db, task)?;
    if state != TaskState::AnswerReady {
        return refuse("answer_not_expected");
    }
    match super::completion::complete_tx(db, task, evidence) {
        Ok(()) => Ok(()),
        Err(CoreError::Refused(gap))
            if [
                "completion_nodes_incomplete",
                "completion_artifact_changed_or_missing",
                "completion_review_missing",
                "completion_verification_missing",
                "completion_project_revision_changed",
                "completion_dropped_failed_check",
            ]
            .contains(&gap) =>
        {
            event_tx(
                db,
                task,
                "ANSWER_NOT_CERTIFIED",
                json!({"revision":revision,"reason":gap}),
            )?;
            domain::state_tx(
                db,
                task,
                TaskState::Failed,
                Some(&format!("answer_not_certified:{gap}")),
            )
        }
        Err(error) => Err(error),
    }
}
/// Completion still validates the reports, process waits, review and latest
/// runtime verification. Disclosing a gap never converts a failure to a pass.
pub fn disclosed_gap(db: &Connection, task: &str, check: &Value) -> CoreResult<bool> {
    let name = check["name"]
        .as_str()
        .ok_or(CoreError::Refused("invalid_worker_check"))?;
    if !gaps(db, task)?.contains_key(&normalized(name)) {
        return Ok(true);
    }
    let raw: Option<String> = db
        .query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=?
        AND type IN ('ANSWER_SUBMITTED','SYNTHESIS_SUBMITTED') ORDER BY id DESC LIMIT 1",
            [task],
            |r| r.get(0),
        )
        .optional()?;
    let v: Value = raw
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or(Value::Null);
    Ok(v["not_verified"].as_array().is_some_and(|a| {
        a.iter().any(|g| {
            g["item"]
                .as_str()
                .is_some_and(|s| normalized(s) == normalized(name))
                && g["blocker"].as_str().is_some_and(|s| !s.trim().is_empty())
        })
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{lifecycle, store::Store};
    fn store() -> (std::path::PathBuf, Store) {
        let root = std::env::temp_dir().join(format!("reconciliation-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let key = crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap();
        (
            root.clone(),
            Store::open(key, crate::OpenMode::Initialize).unwrap(),
        )
    }
    fn manifest(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }
    /// A baseline as an earlier build stored it when only the ignored listing failed:
    /// `incomplete` and no ignored digest, with the deliverable manifest retained.
    fn with_baseline(baseline: Value, ignored_case: impl FnOnce(&Store)) {
        let (root, store) = store();
        lifecycle::TaskMachine::new(&store)
            .create("t", "g", "o")
            .unwrap();
        store
            .write(|db| {
                db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=1 WHERE task_id='t'", [])?;
                lifecycle::event_tx(db, "t", "EXECUTION_BASELINE", baseline)?;
                record_baseline_manifest_tx(db, "t", "d0", &manifest(&[("a.txt", "1"), ("b.txt", "2")]))?;
                db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'_review_1','worker-code-reviewer','Review','[]','[\"**\"]','review','running','rv',NULL)", [])?;
                Ok(())
            })
            .unwrap();
        ignored_case(&store);
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
    fn packet(store: &Store, ignored: Option<&str>) -> Value {
        let current = manifest(&[("a.txt", "9"), ("b.txt", "2")]);
        store
            .read(|db| review_packet_tx(db, "t", "_review_1", Some(&current), ignored, &current))
            .unwrap()
    }
    #[test]
    fn ignored_changes_never_make_the_deliverable_change_set_incomplete() {
        with_baseline(
            json!({"kind":"observed","revision_digest":"d0","ignored_digest":"i0","revision":1}),
            |store| {
                let same = packet(store, Some("i0"));
                assert_eq!(same["changed_files"]["complete"], true);
                assert!(same["changed_files"]["ignored_files"].is_null());
                for ignored in [Some("i1"), None] {
                    let p = packet(store, ignored);
                    let files = &p["changed_files"];
                    assert_eq!(files["complete"], true, "{ignored:?}");
                    assert_eq!(files["total"], 1);
                    assert_eq!(files["files"][0]["path"], "a.txt");
                    assert!(files["note"].is_null());
                    assert!(files["ignored_files"]
                        .as_str()
                        .unwrap()
                        .contains("not deliverable and are not listed"));
                }
                let changed = packet(store, Some("i1"));
                assert!(changed["changed_files"]["ignored_files"]
                    .as_str()
                    .unwrap()
                    .starts_with("Build outputs and other git-ignored files changed"));
            },
        );
    }
    #[test]
    fn a_stored_baseline_incomplete_only_for_ignored_files_gives_a_complete_change_set() {
        with_baseline(
            json!({"kind":"observed","revision_digest":"d0","ignored_digest":null,
                "metadata_only":false,"cap_reached":false,"incomplete":true,"revision":1}),
            |store| {
                let p = packet(store, Some("i0"));
                assert_eq!(p["changed_files"]["complete"], true);
                assert_eq!(p["changed_files"]["files"][0]["path"], "a.txt");
                assert_eq!(p["changed_files"]["total"], 1);
            },
        );
        // Without a retained manifest (what such a baseline usually left behind) the
        // list stays the recorded writes only, and says so.
        with_baseline(
            json!({"kind":"observed","revision_digest":"d0","ignored_digest":null,"incomplete":true,"revision":1}),
            |store| {
                store
                    .write(|db| Ok(db.execute("DELETE FROM attempt_revisions", [])?))
                    .unwrap();
                let p = packet(store, Some("i0"));
                assert_eq!(p["changed_files"]["complete"], false);
                assert!(p["changed_files"]["note"].is_string());
            },
        );
    }
}
