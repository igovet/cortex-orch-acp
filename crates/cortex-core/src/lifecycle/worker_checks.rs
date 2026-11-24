//! Worker check history is evidence, not a model-provided certification.
//! Parity reference: delegation/evidence.py current_outcomes/accept_checks.
use super::{completion::trivial_command, probe_correction};
use crate::{CoreError, CoreResult};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};

#[derive(Clone, Debug)]
pub struct Run {
    pub operation: String,
    pub receipt: Value,
    pub order: i64,
    pub output_verified: bool,
    /// A runtime-verified work-set binding of this run (`work_set_binding`): the
    /// run's work set was unchanged across the run and is unchanged at submission.
    /// Only the host's own observation (or, at completion, the recorded result of
    /// that observation) sets it; never a model argument.
    pub work_set: Option<Value>,
}
impl Run {
    fn reference(&self) -> &str {
        self.receipt["tool_call_ref"].as_str().unwrap_or("")
    }
    /// The run was bound to the whole checkout and that checkout is the submitted one.
    fn exact(&self, digest: Option<&str>) -> bool {
        self.receipt["revision_digest"]
            .as_str()
            .is_some_and(|d| d.len() == 64 && Some(d) == digest)
    }
    fn identity(&self) -> bool {
        self.receipt["argv"]
            .as_array()
            .is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_string))
            && self.receipt["cwd"].is_string()
            && self.receipt["profile"].is_string()
    }
    /// A receipt the runtime itself settled as provably never launched: its payload
    /// was never released to the sandbox (`settle_unrecorded_tx`, `command_not_started`).
    /// No process ran, so it cannot be an unknown later run of a check: it neither
    /// blocks closing an earlier failure nor counts as a later run of the same command.
    /// A settled receipt with an unknown outcome is not this and still blocks.
    fn never_launched(&self) -> bool {
        self.receipt["settled"] == true
            && self.receipt["launched"] == false
            && self.receipt["outcome"] == "not_started"
            && self.receipt["failure"] == "command_not_started"
            && self.receipt["exit_code"].is_null()
    }
    pub(super) fn same(&self, other: &Self) -> bool {
        !self.never_launched()
            && !other.never_launched()
            && self.identity()
            && other.identity()
            && self.receipt["argv"] == other.receipt["argv"]
            && self.receipt["cwd"] == other.receipt["cwd"]
            && self.receipt.get("checkout") == other.receipt.get("checkout")
            && (self.operation != other.operation
                || self.receipt["profile"] == other.receipt["profile"])
    }
    /// The same intent as `other`: the same argv in the same working directory, on
    /// any profile or operation. Only used to let a later verified pass of a later
    /// revision supersede an earlier revision's failed check
    /// (`repair_checks::require_prior_worker_repairs`), never inside one attempt.
    pub(super) fn same_intent(&self, other: &Self) -> bool {
        !self.never_launched()
            && !other.never_launched()
            && self.identity()
            && other.identity()
            && self.receipt["argv"] == other.receipt["argv"]
            && self.receipt["cwd"] == other.receipt["cwd"]
            && self.receipt.get("checkout") == other.receipt.get("checkout")
    }
    pub fn verified_failure(&self) -> bool {
        self.complete() && self.receipt["exit_code"].as_i64().is_some_and(|c| c != 0)
    }
    pub fn presented_failure(&self) -> bool {
        self.verified_failure()
            || self.receipt["complete"] != true
            || !self.receipt["failure"].is_null()
            || self.receipt["exit_code"].as_i64().is_none()
    }
    fn complete(&self) -> bool {
        self.receipt["complete"] == true
            && self.receipt["failure"].is_null()
            && self.receipt["exit_code"].as_i64().is_some()
            && self.output_verified
    }
}

/// Only the authenticated attempt's channels may be passed here. Keep pending
/// requests too: a later unknown identity must prevent closing an old failure.
pub fn load_runs(db: &Connection, task: &str, channel: &str) -> CoreResult<Vec<Run>> {
    // Request intents are inserted serially before launch. Use their single
    // order domain even for pending requests without a started event.
    let mut q = db.prepare(
        "SELECT r.operation,r.result_json,r.rowid
        FROM runtime_requests r WHERE r.task_id=? AND r.actor IN
          (SELECT channel_id FROM trusted_channels WHERE task_id=? AND
            (channel_id=? OR (attempt_id IS NOT NULL AND attempt_id=
              (SELECT attempt_id FROM trusted_channels WHERE channel_id=?))))
        AND r.operation IN ('command_run','outside_command') ORDER BY 3,r.rowid",
    )?;
    let rows = q
        .query_map((task, task, channel, channel), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let runs = rows.into_iter()
        .map(|(operation, raw, order)| {
            let saved: Value = serde_json::from_str(&raw)?;
            let receipt = if operation == "outside_command" {
                saved["reply"]["structuredContent"].clone()
            } else {
                saved
            };
            Ok(Run {
                operation,
                receipt,
                order,
                output_verified: false,
                work_set: None,
            })
        })
        .collect::<CoreResult<Vec<_>>>()?;
    let mut matched = Vec::new();
    for run in runs {
        if crate::workspace::worktrees::receipt_matches(db, task, &run.receipt, false)? {
            matched.push(run);
        } else if run.receipt["complete"] != true && run.receipt["checkout"].is_null() {
            // A not-yet-observed intent is not proof of any checkout.
            matched.push(run);
        }
    }
    Ok(matched)
}

/// At most this many entries (scope entries plus written paths) form a work set;
/// a larger one is not recorded and its runs bind only to the whole checkout.
pub const WORK_SET_LIMIT: usize = 256;
/// A scope entry that names the whole project (or something outside it) leaves no
/// narrower set: `overlaps` treats an empty literal prefix as everything.
fn whole_project(entry: &str) -> bool {
    let entry = crate::scheduler::policy::canonical(entry);
    let prefix = entry[..entry.find(['*', '?', '[']).unwrap_or(entry.len())]
        .trim_end_matches('/')
        .to_owned();
    prefix.is_empty()
        || prefix == "."
        || entry.starts_with('/')
        || entry.split('/').any(|part| part == "..")
}
fn written_paths(
    db: &Connection,
    task: &str,
    assignment: &str,
    after: Option<i64>,
) -> CoreResult<Vec<String>> {
    // Every channel of this assignment (all its attempts): project_write/edit and
    // the file operations (`NATIVE_PROJECT_CHANGED`) record their path.
    let rows = db
        .prepare(
            "SELECT DISTINCT json_extract(e.payload_json,'$.path') FROM runtime_events e
             JOIN trusted_channels c ON c.task_id=e.task_id
               AND c.channel_id=json_extract(e.payload_json,'$.channel_id')
             WHERE e.task_id=? AND c.assignment_id=? AND e.id>?
               AND e.type IN ('NATIVE_PROJECT_WRITTEN','NATIVE_PROJECT_CHANGED')
               AND length(json_extract(e.payload_json,'$.path'))>0",
        )?
        .query_map((task, assignment, after.unwrap_or(0)), |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(rows)
}
/// The work set of an assignment's command, taken in the transaction that records
/// the command's start: its declared scope entries plus every path its own tools
/// (project_write/edit, delete, mkdir) wrote so far. `None` when that set is the
/// whole project (a `.`/`**` scope), too large, or there is no assignment: such a run
/// binds only to the whole checkout, as before.
pub fn work_set_tx(
    db: &Connection,
    task: &str,
    assignment: Option<&str>,
    scope: &[String],
) -> CoreResult<Option<Vec<String>>> {
    let Some(assignment) = assignment else {
        return Ok(None);
    };
    if scope.iter().any(|s| whole_project(s)) {
        return Ok(None);
    }
    let mut set: std::collections::BTreeSet<String> = scope
        .iter()
        .map(|s| crate::scheduler::policy::canonical(s))
        .collect();
    for path in written_paths(db, task, assignment, None)? {
        if whole_project(&path) {
            return Ok(None);
        }
        set.insert(crate::scheduler::policy::canonical(&path));
    }
    if set.is_empty() || set.len() > WORK_SET_LIMIT {
        return Ok(None);
    }
    Ok(Some(set.into_iter().collect()))
}
/// Whether a path belongs to a work set (the scheduler's conservative overlap).
pub fn in_work_set(path: &str, work_set: &[String]) -> bool {
    work_set
        .iter()
        .any(|entry| crate::scheduler::policy::overlaps(path, entry))
}
/// Retain the work-set observation of one settled command run: the work-set digest
/// before launch and after the process ended. Written by the host only.
pub fn record_work_set_tx(
    db: &Connection,
    task: &str,
    reference: &str,
    assignment: &str,
    work_set: &[String],
    start: &str,
    end: &str,
) -> CoreResult<()> {
    let started: Option<(i64, String)> = db
        .query_row(
            "SELECT id,payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_COMMAND_STARTED'
             AND json_extract(payload_json,'$.ref')=? ORDER BY id LIMIT 1",
            (task, reference),
            |r| Ok((r.get(0)?,r.get(1)?)),
        )
        .optional()?;
    let Some((started, payload)) = started else {
        return Ok(());
    };
    let seen: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='NATIVE_CHECK_WORK_SET'
         AND json_extract(payload_json,'$.ref')=?)",
        (task, reference),
        |r| r.get(0),
    )?;
    if seen {
        return Ok(());
    }
    let payload: Value = serde_json::from_str(&payload)?;
    let mut event = json!({"ref":reference,"assignment_ref":assignment,"work_set":work_set,
        "started_event":started,"start_digest":start,"end_digest":end,"stable":start==end});
    if let Some(checkout) = payload.get("checkout") {
        event["checkout"] = checkout.clone();
        event["cwd"] = payload["cwd"].clone();
    }
    super::event_tx(db, task, "NATIVE_CHECK_WORK_SET", event)?;
    Ok(())
}
/// The retained work-set observation of `reference`, when it is stable (the work
/// set had the same digest before launch and after the process ended).
pub fn work_set_record(db: &Connection, task: &str, reference: &str) -> CoreResult<Option<Value>> {
    let rows = db
        .prepare(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='NATIVE_CHECK_WORK_SET'
             AND json_extract(payload_json,'$.ref')=? LIMIT 2",
        )?
        .query_map((task, reference), |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let [raw] = rows.as_slice() else {
        return Ok(None);
    };
    let v: Value = serde_json::from_str(raw)?;
    let valid = v["stable"] == true
        && v["end_digest"]
            .as_str()
            .is_some_and(|d| d.len() == 64 && d.bytes().all(|b| b.is_ascii_hexdigit()))
        && v["start_digest"] == v["end_digest"]
        && v["work_set"]
            .as_array()
            .is_some_and(|a| !a.is_empty() && a.iter().all(Value::is_string))
        && v["started_event"].is_i64();
    Ok(valid.then_some(v))
}
/// Whether this assignment's own tools wrote a path outside `work_set` after the
/// run started: that later work is not what the check ran against.
pub fn wrote_outside_after(
    db: &Connection,
    task: &str,
    assignment: &str,
    started: i64,
    work_set: &[String],
) -> CoreResult<bool> {
    Ok(written_paths(db, task, assignment, Some(started))?
        .iter()
        .any(|p| !in_work_set(p, work_set)))
}
/// The binding a verified work-set run gives its passed check.
pub fn work_set_binding(record: &Value) -> Value {
    let mut binding = json!({"kind":"work_set","work_set":record["work_set"],"digest":record["end_digest"]});
    if let Some(checkout) = record.get("checkout") {
        binding["checkout"] = checkout.clone();
        binding["cwd"] = record["cwd"].clone();
    }
    binding
}
/// At completion, re-establish each recorded work-set binding from the runtime's own
/// records: the submission-time comparison was made by the host and recorded with
/// the result; the observation it compared must still be the retained stable one.
pub fn accept_recorded_bindings(
    db: &Connection,
    task: &str,
    checks: &[Value],
    runs: &mut [Run],
) -> CoreResult<()> {
    for check in checks {
        let binding = &check["revision_binding"];
        if binding["kind"] != "work_set" || check["evidence_strength"] != "verified" {
            continue;
        }
        let Some(reference) = check["tool_call_ref"].as_str() else {
            continue;
        };
        let Some(record) = work_set_record(db, task, reference)? else {
            continue;
        };
        if work_set_binding(&record) != *binding {
            continue;
        }
        for run in runs.iter_mut().filter(|r| r.reference() == reference) {
            if binding.get("checkout") == run.receipt.get("checkout")
                && (binding.get("checkout").is_none() || binding["cwd"] == run.receipt["cwd"])
            {
                run.work_set = Some(binding.clone());
            }
        }
    }
    Ok(())
}

/// Only cited commands and their matching history need full output hashing.
/// Unrelated identities still remain in history, including unknown identities.
pub fn relevant_references(checks: &[Value], runs: &[Run]) -> std::collections::BTreeSet<String> {
    let cited: std::collections::BTreeSet<_> = checks
        .iter()
        .filter(|c| matches!(c["outcome"].as_str(), Some("passed" | "failed")))
        .filter_map(|c| c["tool_call_ref"].as_str())
        .collect();
    let identities: Vec<_> = runs
        .iter()
        .filter(|r| cited.contains(r.reference()))
        .collect();
    runs.iter()
        .filter(|r| cited.contains(r.reference()) || identities.iter().any(|c| r.same(c)))
        .map(|r| r.reference().to_owned())
        .collect()
}

/// Snapshot hashes are runtime observations, never model-supplied links.
pub fn observe_outputs(
    db: &Connection,
    task: &str,
    runs: &mut [Run],
    artifacts: &std::collections::BTreeMap<String, String>,
) -> CoreResult<()> {
    for run in runs {
        let Some(id) = run.receipt["large_output_id"].as_str() else {
            continue;
        };
        if !artifacts.contains_key(id) {
            continue;
        }
        let row:Option<(String,i64,bool,Option<i64>,Option<String>)>=db.query_row(
            "SELECT source_ref,byte_count,truncated,exit_code,failure FROM large_outputs WHERE task_id=? AND id=?",
            (task,id),|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).optional()?;
        if let Some((hash, count, truncated, code, failure)) = row {
            run.output_verified = artifacts.get(id) == Some(&hash)
                && !truncated
                && run.receipt["byte_count"].as_i64() == Some(count)
                && run.receipt["exit_code"].as_i64() == code
                && run.receipt["failure"] == json!(failure);
        }
    }
    Ok(())
}

#[derive(Clone)]
struct Assessment {
    run: Option<usize>,
    strength: &'static str,
    gap: Option<&'static str>,
}
fn assess(check: &Value, runs: &[Run], digest: Option<&str>, duplicate: bool) -> Assessment {
    let weak = |gap| Assessment {
        run: None,
        strength: "weak",
        gap: Some(gap),
    };
    if !matches!(check["outcome"].as_str(), Some("passed" | "failed")) {
        return Assessment {
            run: None,
            strength: "none",
            gap: None,
        };
    }
    let Some(reference) = check["tool_call_ref"].as_str().filter(|s| !s.is_empty()) else {
        return weak("tool_call_ref_missing");
    };
    if duplicate {
        return weak("ref_already_cited");
    }
    let found: Vec<_> = runs
        .iter()
        .enumerate()
        .filter(|(_, r)| r.reference() == reference)
        .collect();
    if found.len() != 1 {
        return weak(if found.is_empty() {
            "tool_call_ref_unknown"
        } else {
            "ambiguous_ref"
        });
    }
    let (index, run) = found[0];
    let gap = if !run.complete() {
        Some("call_incomplete")
    } else if check["outcome"] == "passed" && run.receipt["exit_code"] != 0 {
        Some("nonzero_exit")
    } else if check["outcome"] == "failed" && run.receipt["exit_code"] == 0 {
        Some("zero_exit")
    } else if !run.identity() {
        Some("command_unrecorded")
    } else if check["outcome"] == "passed"
        && serde_json::from_value::<Vec<String>>(run.receipt["argv"].clone())
            .map(|a| trivial_command(&a))
            .unwrap_or(true)
    {
        Some("trivial_command")
    } else if check["outcome"] == "passed" && !run.exact(digest) && run.work_set.is_none() {
        // Neither the whole submitted checkout nor a runtime-verified work set
        // (`work_set_binding`) is what this run observed.
        Some("revision_mismatch")
    } else {
        None
    };
    Assessment {
        run: Some(index),
        strength: if gap.is_none() { "verified" } else { "weak" },
        gap,
    }
}

/// Recompute, including all uncited later runs. Retain failed history and derive
/// closure/retraction links; never accept a link supplied by the model.
pub fn evaluate(checks: &[Value], runs: &[Run], digest: Option<&str>) -> CoreResult<Vec<Value>> {
    let mut cited = std::collections::BTreeSet::new();
    let assessments: Vec<_> = checks
        .iter()
        .map(|c| {
            let duplicate = c["tool_call_ref"]
                .as_str()
                .is_some_and(|r| !cited.insert(r.to_owned()));
            assess(c, runs, digest, duplicate)
        })
        .collect();
    let mut out = Vec::new();
    for (check, a) in checks.iter().zip(&assessments) {
        let mut c = check.clone();
        for field in [
            "closed_by",
            "retracted_by",
            "superseded_by",
            "current_outcome",
            "evidence_gap",
            "claimed_outcome",
            "revision_digest",
            "revision_binding",
        ] {
            c.as_object_mut()
                .ok_or(CoreError::Refused("invalid_native_result"))?
                .remove(field);
        }
        c["evidence_strength"] = json!(a.strength);
        if let Some(gap) = a.gap {
            c["evidence_gap"] = json!(gap)
        }
        if check["outcome"] == "passed" {
            let mut gap = a.gap;
            if a.strength == "verified" {
                let run = &runs[a.run.unwrap()];
                c["revision_digest"] = run.receipt["revision_digest"].clone();
                // What the pass is bound to: the exact submitted checkout, or only
                // the run's work set (its scope and own writes; other files may differ).
                c["revision_binding"] = match (&run.work_set, run.exact(digest)) {
                    (_, true) => json!({"kind":"checkout"}),
                    (Some(binding), false) => binding.clone(),
                    (None, false) => Value::Null,
                };
                if let Some(latest) = runs.iter().filter(|r| r.same(run)).max_by_key(|r| r.order) {
                    if latest.order > run.order {
                        if !latest.complete() {
                            gap = Some("later_run_unresolved")
                        } else if latest.receipt["exit_code"] != 0 {
                            gap = Some("later_run_failed")
                        }
                    }
                }
            }
            if let Some(gap) = gap {
                c["outcome"] = json!(if matches!(gap, "nonzero_exit" | "later_run_failed") {
                    "failed"
                } else {
                    "unverified"
                });
                c["claimed_outcome"] = json!("passed");
                c["evidence_gap"] = json!(gap);
                c["evidence_strength"] = json!(if c["outcome"] == "unverified" {
                    "none"
                } else {
                    assess(&c, runs, digest, false).strength
                });
                if c["outcome"] == "unverified" {
                    c["blocker"] =
                        json!(format!("Runtime kept this passed claim unverified: {gap}."));
                }
            }
        } else if check["outcome"] == "failed" && a.strength == "verified" {
            let run = &runs[a.run.unwrap()];
            let latest = runs.iter().filter(|r| r.same(run)).max_by_key(|r| r.order);
            if let Some(latest) = latest
                .filter(|r| r.order > run.order && r.complete() && r.receipt["exit_code"] == 0)
            {
                let cited: Vec<_> = checks
                    .iter()
                    .zip(&assessments)
                    .filter(|(c, a)| {
                        c["outcome"] == "passed"
                            && c["name"] == check["name"]
                            && c["tool_call_ref"] == latest.reference()
                            && a.strength == "verified"
                    })
                    .collect();
                if cited.len() == 1
                    && !runs
                        .iter()
                        .any(|r| r.order > run.order && !r.identity() && !r.never_launched())
                {
                    c["closed_by"] = json!(latest.reference());
                }
            }
        }
        out.push(c);
    }
    for (index, check) in checks.iter().enumerate() {
        let a = &assessments[index];
        if check["probe_withdrawn"]
            .as_str()
            .is_some_and(|s| !s.trim().is_empty())
            && check["outcome"] == "failed"
            && a.strength == "verified"
            && out[index]["closed_by"].is_null()
        {
            out[index]["retracted_by"] = json!("probe_withdrawn");
            out[index]["current_outcome"] = json!("retracted");
        }
        for (field, derived, code) in [
            (
                "invocation_error",
                "retracted_by",
                "invalid_check_retraction",
            ),
            (
                "command_correction",
                "superseded_by",
                "invalid_check_supersession",
            ),
        ] {
            let Some(request) = check.get(field) else {
                continue;
            };
            let target = checks
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c["outcome"] == "passed"
                        && c["name"] == check["name"]
                        && c["tool_call_ref"] == request["corrected_by"]
                })
                .collect::<Vec<_>>();
            let uses = checks
                .iter()
                .flat_map(|c| ["invocation_error", "command_correction"].map(move |f| &c[f]))
                .filter(|r| r.is_object() && r["corrected_by"] == request["corrected_by"])
                .count();
            let eligible = || -> Option<&str> {
                if check["outcome"] != "failed"
                    || a.strength != "verified"
                    || target.len() != 1
                    || uses != 1
                    || request["reason"]
                        .as_str()
                        .is_none_or(|s| s.trim().is_empty())
                {
                    return None;
                }
                let (t, _) = target[0];
                if assessments[t].strength != "verified" || out[t]["outcome"] != "passed" {
                    return None;
                }
                let bad = &runs[a.run?];
                let good = &runs[assessments[t].run?];
                if bad.order >= good.order
                    || bad.operation != good.operation
                    || bad.receipt["cwd"] != good.receipt["cwd"]
                    || bad.receipt["profile"] != good.receipt["profile"]
                    || !bad.receipt["runtime_verification"].is_null()
                    || !good.receipt["runtime_verification"].is_null()
                    || runs.iter().any(|r| {
                        r.order > bad.order
                            && ((!r.identity() && !r.never_launched())
                                || (r.same(bad) && (!r.complete() || r.receipt["exit_code"] != 0)))
                    })
                {
                    return None;
                }
                let argv =
                    serde_json::from_value::<Vec<String>>(bad.receipt["argv"].clone()).ok()?;
                let corrected =
                    serde_json::from_value::<Vec<String>>(good.receipt["argv"].clone()).ok()?;
                let output = bad.receipt["output"].as_str()?;
                if bad.receipt["byte_count"].as_u64() != Some(output.len() as u64) {
                    return None;
                }
                if !probe_correction::eligible(
                    &argv,
                    &corrected,
                    output,
                    bad.receipt["exit_code"].as_i64()?,
                    field == "command_correction",
                ) {
                    return None;
                }
                Some(good.reference())
            };
            let reference = eligible().ok_or(CoreError::Refused(code))?;
            for key in ["closed_by", "retracted_by", "superseded_by"] {
                out[index].as_object_mut().unwrap().remove(key);
            }
            out[index][derived] = json!(reference);
            out[index]["current_outcome"] = json!(if field == "command_correction" {
                "superseded"
            } else {
                "retracted"
            });
        }
    }
    Ok(out)
}

fn program(run: &Run) -> String {
    run.receipt["argv"][0]
        .as_str()
        .map(|p| p.rsplit('/').next().unwrap_or(p).chars().take(40).collect())
        .unwrap_or_else(|| "command".into())
}
fn gap_hint(gap: &str) -> &'static str {
    match gap {
        "revision_mismatch" => "it observed neither the submitted checkout nor an unchanged work set (this assignment's scope and written files changed during or after the run, or the run has no work-set record): rerun it unchanged after your last edit and cite the new run",
        "call_incomplete" => "it has no complete result or its retained output is not intact: inspect it with command_status, or rerun it",
        "trivial_command" => "its command cannot verify a check (a read, search, no-op or unproved inline shell): run the real check command directly",
        "later_run_failed" => "a later run of the same command failed",
        "later_run_unresolved" => "a later run of the same command has no result yet: inspect it with command_status",
        _ => "it is not a verified pass",
    }
}
/// What keeps each open failed check of an evaluated result open, as bounded items
/// for a refusal: the check's name, its `tool_call_ref`, the gap and what closes it.
/// Names a program and receipt references only, never argv text or output.
pub fn open_items(evaluated: &[Value], runs: &[Run]) -> Vec<String> {
    let name = |c: &Value| -> String {
        c["name"]
            .as_str()
            .unwrap_or("check")
            .chars()
            .take(80)
            .collect()
    };
    let mut items = Vec::new();
    for check in evaluated {
        if check["outcome"] != "failed" || resolved(check) {
            continue;
        }
        let reference = check["tool_call_ref"].as_str().unwrap_or("");
        let label = format!("{} [{}]", name(check), reference);
        if check["claimed_outcome"] == "passed" {
            let gap = check["evidence_gap"].as_str().unwrap_or("nonzero_exit");
            items.push(match gap {
                "later_run_failed" => format!("{label}: later_run_failed: a later run of the same command failed; fix the cause, rerun it and cite that latest passing run"),
                _ => format!("{label}: nonzero_exit: the cited run exited nonzero; fix the cause, rerun the same command and cite the passing run"),
            });
            continue;
        }
        if check["evidence_strength"] != "verified" {
            let gap = check["evidence_gap"]
                .as_str()
                .unwrap_or("unverified_failure");
            items.push(format!("{label}: {gap}: the failed check cites no verified failing run of this attempt; cite the failing command_run receipt, or rerun the check"));
            continue;
        }
        let Some(failed) = runs.iter().find(|r| r.reference() == reference) else {
            continue;
        };
        let later = runs
            .iter()
            .filter(|r| r.same(failed) && r.order > failed.order)
            .max_by_key(|r| r.order);
        let Some(latest) = later else {
            items.push(format!("{label}: no_passing_rerun: no later run of the same {} argv, cwd and profile; fix the cause, rerun exactly that command and cite the passing run as passed under the same name", program(failed)));
            continue;
        };
        let rerun = latest.reference();
        if !latest.complete() {
            items.push(format!("{label}: later_run_unresolved: the latest rerun [{rerun}] has no complete result; inspect it with command_status, or rerun"));
            continue;
        }
        if latest.receipt["exit_code"] != 0 {
            items.push(format!("{label}: later_run_failed: the latest rerun [{rerun}] failed too; fix the cause and rerun"));
            continue;
        }
        let cited: Vec<&Value> = evaluated
            .iter()
            .filter(|c| c["tool_call_ref"] == rerun && c["claimed_outcome"] != "failed")
            .collect();
        match cited.as_slice() {
            [] => items.push(format!("{label}: rerun_not_cited: the latest rerun [{rerun}] passed; cite it as a passed check named \"{}\"", name(check))),
            [c] if c["name"] != check["name"] => items.push(format!("{label}: rerun_named_differently: cite the passing rerun [{rerun}] under the same name \"{}\"", name(check))),
            [c] if c["outcome"] != "passed" => {
                let gap = c["evidence_gap"].as_str().unwrap_or("unverified");
                items.push(format!("{label}: rerun_{gap}: the passing rerun [{rerun}] does not verify: {}", gap_hint(gap)));
            }
            _ => items.push(format!("{label}: later_run_unrecorded: a later command run of this attempt has no recorded identity; rerun the check once more and cite that run")),
        }
    }
    items
}
pub fn resolved(check: &Value) -> bool {
    check["outcome"] == "failed"
        && ["closed_by", "retracted_by", "superseded_by"]
            .iter()
            .any(|k| check[*k].is_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    const DIGEST: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    fn run(reference: &str, order: i64, code: i64, argv: Value, output: &str) -> Run {
        Run {
            operation: "command_run".into(),
            order,
            output_verified: true,
            work_set: None,
            receipt: json!({"tool_call_ref":reference,"argv":argv,"cwd":"/fixture","profile":"development",
                "complete":true,"exit_code":code,"failure":null,"revision_digest":DIGEST,
                "output":output,"byte_count":output.len()}),
        }
    }
    fn check(outcome: &str, reference: &str) -> Value {
        json!({"name":"check","outcome":outcome,"tool_call_ref":reference})
    }
    fn python(source: &str) -> Value {
        json!(["python3", "-B", "-c", source])
    }
    fn evaluate_pair(checks: &[Value], runs: &[Run]) -> CoreResult<Vec<Value>> {
        evaluate(checks, runs, Some(DIGEST))
    }
    #[test]
    fn unrelated_output_is_not_hashed_but_matching_uncited_history_is() {
        let runs = vec![
            run("bad", 1, 1, json!(["cargo", "test"]), ""),
            run("rerun", 2, 0, json!(["cargo", "test"]), ""),
            run("unrelated", 3, 0, json!(["cargo", "build"]), ""),
        ];
        assert_eq!(
            relevant_references(&[check("failed", "bad")], &runs),
            ["bad".to_string(), "rerun".to_string()].into()
        );
    }
    // Ported from test_check_history / test_check_closure_fixes cross-operation cases.
    #[test]
    fn exact_latest_rerun_table() {
        for case in [
            "exact",
            "outside",
            "outside_reverse",
            "argv",
            "cwd",
            "profile",
            "name",
            "earlier",
            "uncited_failure",
            "unresolved",
            "unrecorded",
            "uncited_success",
            "duplicate",
            "missing_ref",
            "never_launched_between",
            "never_launched_same_after",
            "settled_unknown_between",
        ] {
            let bad = run("bad", 1, 1, json!(["cargo", "test"]), "failed");
            // What `settle_unrecorded_tx` leaves for a call whose payload never left
            // sandbox preparation: argv only, no cwd or profile, no exit.
            let unreleased = |reference: &str, order: i64, outcome: &str, failure: &str| Run {
                operation: "command_run".into(),
                order,
                output_verified: false,
                work_set: None,
                receipt: json!({"tool_call_ref":reference,"argv":["cargo","test"],"complete":false,
                    "settled":true,"exit_code":null,"failure":failure,"outcome":outcome,
                    "launched":if outcome == "not_started" {json!(false)} else {Value::Null},
                    "settlement_evidence":"payload_never_released"}),
            };
            let mut good = run("good", 2, 0, json!(["cargo", "test"]), "passed");
            let mut runs = vec![bad, good.clone()];
            let mut checks = vec![check("failed", "bad"), check("passed", "good")];
            match case {
                "outside" => runs[1].operation = "outside_command".into(),
                "outside_reverse" => runs[0].operation = "outside_command".into(),
                "argv" => runs[1].receipt["argv"] = json!(["cargo", "test", "other"]),
                "cwd" => runs[1].receipt["cwd"] = json!("/other"),
                "profile" => runs[1].receipt["profile"] = json!("guarded"),
                "name" => checks[1]["name"] = json!("other"),
                "earlier" => runs[1].order = 0,
                "uncited_failure" => {
                    good.order = 3;
                    good.receipt["tool_call_ref"] = json!("late");
                    good.receipt["exit_code"] = json!(1);
                    runs.push(good);
                }
                "unresolved" => {
                    good.order = 3;
                    good.receipt["tool_call_ref"] = json!("late");
                    good.receipt["complete"] = json!(false);
                    runs.push(good);
                }
                "unrecorded" => {
                    good.order = 3;
                    good.receipt["tool_call_ref"] = json!("late");
                    good.receipt["argv"] = Value::Null;
                    runs.push(good);
                }
                "uncited_success" => {
                    good.order = 3;
                    good.receipt["tool_call_ref"] = json!("new");
                    runs.push(good);
                }
                "duplicate" => checks.push(check("passed", "good")),
                "missing_ref" => checks[1]["tool_call_ref"] = json!("unknown"),
                // Observed: sandbox preparation timed out for a rerun, then the exact
                // rerun passed. The never-released call must not keep the failure open.
                "never_launched_between" => {
                    runs[1].order = 3;
                    runs.push(unreleased(
                        "unreleased",
                        2,
                        "not_started",
                        "command_not_started",
                    ));
                }
                // Even with a full identity, a never-launched later call is not a later run.
                "never_launched_same_after" => {
                    let mut late = unreleased("late", 3, "not_started", "command_not_started");
                    late.receipt["cwd"] = json!("/fixture");
                    late.receipt["profile"] = json!("development");
                    runs.push(late);
                }
                // A call that may have started with an unknown outcome still blocks.
                "settled_unknown_between" => {
                    runs[1].order = 3;
                    runs.push(unreleased(
                        "unknown",
                        2,
                        "unknown",
                        "spawn_or_capture_failed",
                    ));
                }
                _ => (),
            }
            let result = evaluate_pair(&checks, &runs).unwrap();
            assert_eq!(result[0]["outcome"], "failed", "{case}");
            assert_eq!(
                result[0]["closed_by"] == "good",
                matches!(
                    case,
                    "exact"
                        | "outside"
                        | "outside_reverse"
                        | "duplicate"
                        | "never_launched_between"
                        | "never_launched_same_after"
                ),
                "{case}"
            );
            if case == "never_launched_same_after" {
                assert_eq!(result[1]["outcome"], "passed", "{case}");
                assert!(result[1]["evidence_gap"].is_null(), "{case}");
            }
            assert_eq!(result[0]["evidence_strength"], "verified", "{case}");
            if case == "uncited_failure" {
                assert_eq!(result[1]["outcome"], "failed");
            }
            if case == "unresolved" {
                assert_eq!(result[1]["outcome"], "unverified");
            }
        }
    }
    #[test]
    fn evidence_strength_and_kept_claims_table() {
        for (case, outcome, strength, gap) in [
            ("valid", "passed", "verified", None),
            ("trivial", "unverified", "none", Some("trivial_command")),
            (
                "unknown",
                "unverified",
                "none",
                Some("tool_call_ref_unknown"),
            ),
            (
                "missing",
                "unverified",
                "none",
                Some("tool_call_ref_missing"),
            ),
            ("incomplete", "unverified", "none", Some("call_incomplete")),
            ("truncated", "unverified", "none", Some("call_incomplete")),
            ("digest", "unverified", "none", Some("revision_mismatch")),
            ("nonzero", "failed", "verified", Some("nonzero_exit")),
            ("not_run", "not_run", "none", None),
            ("unverified", "unverified", "none", None),
        ] {
            let mut runs = vec![run("one", 1, 0, json!(["cargo", "test"]), "ok")];
            let mut c = check("passed", "one");
            match case {
                "trivial" => runs[0].receipt["argv"] = json!(["env", "A=1", "ls"]),
                "unknown" => c["tool_call_ref"] = json!("foreign"),
                "missing" => c
                    .as_object_mut()
                    .unwrap()
                    .remove("tool_call_ref")
                    .map(|_| ())
                    .unwrap(),
                "incomplete" => runs[0].receipt["complete"] = json!(false),
                "truncated" => runs[0].output_verified = false,
                "digest" => runs[0].receipt["revision_digest"] = Value::Null,
                "nonzero" => runs[0].receipt["exit_code"] = json!(1),
                "not_run" | "unverified" => {
                    c["outcome"] = json!(case);
                    c["blocker"] = json!("policy");
                }
                _ => (),
            }
            let out = evaluate_pair(&[c], &runs).unwrap();
            assert_eq!(out[0]["outcome"], outcome, "{case}");
            assert_eq!(out[0]["evidence_strength"], strength, "{case}");
            assert_eq!(out[0]["evidence_gap"], json!(gap), "{case}");
        }
    }
    #[test]
    fn probe_withdrawn_retains_verified_failed_history() {
        for (case, eligible) in [
            ("verified", true),
            ("unknown", false),
            ("incomplete", false),
            ("zero_exit", false),
            ("empty", false),
        ] {
            let mut runs = vec![run(
                "one",
                1,
                1,
                json!(["curl", "--fail", "https://invalid.test"]),
                "404",
            )];
            let mut c = check("failed", "one");
            c["probe_withdrawn"] = json!("Expected exploratory 404");
            match case {
                "unknown" => runs.clear(),
                "incomplete" => runs[0].output_verified = false,
                "zero_exit" => runs[0].receipt["exit_code"] = json!(0),
                "empty" => c["probe_withdrawn"] = json!(" "),
                _ => (),
            }
            let out = evaluate_pair(&[c], &runs).unwrap();
            assert_eq!(out[0]["outcome"], "failed");
            assert_eq!(
                out[0]["retracted_by"] == "probe_withdrawn",
                eligible,
                "{case}"
            );
        }
    }
    const BAD: &str = "import re; re.search('(', 'subject')";
    const GOOD: &str = "import re; re.search('subject', 'subject')";
    const ERROR: &str =
        "Traceback:\n  File \"<string>\", line 1\nre.error: missing ), unterminated subpattern\n";
    #[test]
    fn supported_correction_shapes_table() {
        for (bad, good, output) in [
            (BAD, GOOD, ERROR),
            (
                "assert (",
                "assert True",
                "SyntaxError: '(' was never closed\n",
            ),
            (
                "items=['a', 'b']; assert len(items)==3",
                "items=['a', 'b']; assert len(items)==2",
                "File \"<string>\"\nAssertionError\n",
            ),
            (
                "items=[2, 1]; assert sorted(items)==[2, 1]; assert sum(items)==3",
                "items=[2, 1]; assert sorted(items)==[1, 2]; assert sum(items)==3",
                "File \"<string>\"\nAssertionError\n",
            ),
        ] {
            for (field, derived, current) in [
                ("invocation_error", "retracted_by", "retracted"),
                ("command_correction", "superseded_by", "superseded"),
            ] {
                let runs = vec![
                    run("bad", 1, 1, python(bad), output),
                    run("good", 2, 0, python(good), ""),
                ];
                let mut checks = vec![check("failed", "bad"), check("passed", "good")];
                checks[0][field] = json!({"reason":"my invocation mistake","corrected_by":"good"});
                let out = evaluate_pair(&checks, &runs).unwrap();
                assert_eq!(out[0]["outcome"], "failed", "{bad}");
                assert_eq!(out[0][derived], "good");
                assert_eq!(out[0]["current_outcome"], current);
                assert_eq!(out[0]["evidence_strength"], "verified");
                assert!(out[0]["closed_by"].is_null());
                // Derived links never authorize a later lost or changed observation.
                let mut lost = runs.clone();
                lost[1].output_verified = false;
                assert!(evaluate_pair(&out, &lost).is_err());
            }
        }
    }
    #[test]
    fn unsafe_corrections_table() {
        for case in [
            "foreign",
            "claimed",
            "earlier",
            "duplicate",
            "missing",
            "different_name",
            "incomplete",
            "truncated",
            "lost_bad_body",
            "lost_good_body",
            "later_failure",
            "later_unresolved",
            "different_profile",
            "different_operation",
            "runtime_verification",
            "empty_reason",
            "real_failure",
            "changed_assertion",
            "test_runner",
            "bad_expectation",
            "unrecorded",
        ] {
            let mut runs = vec![
                run("bad", 1, 1, python(BAD), ERROR),
                run("good", 2, 0, python(GOOD), ""),
            ];
            let mut checks = vec![check("failed", "bad"), check("passed", "good")];
            checks[0]["invocation_error"] = json!({"corrected_by":"good","reason":"mistake"});
            match case {
                "foreign" => runs.remove(1).output_verified = false,
                "claimed" => checks[1]["tool_call_ref"] = json!("unknown"),
                "earlier" => runs[1].order = 0,
                "duplicate" => checks.push(checks[1].clone()),
                "missing" => {
                    checks.pop();
                }
                "different_name" => checks[1]["name"] = json!("other"),
                "incomplete" => runs[1].receipt["complete"] = json!(false),
                "truncated" | "lost_good_body" => runs[1].output_verified = false,
                "lost_bad_body" => runs[0].output_verified = false,
                "different_profile" => runs[1].receipt["profile"] = json!("other"),
                "different_operation" => runs[1].operation = "outside_command".into(),
                "runtime_verification" => {
                    runs[0].receipt["runtime_verification"] = json!({"command_ref":"v"})
                }
                "empty_reason" => checks[0]["invocation_error"]["reason"] = json!(" "),
                "real_failure" => runs[0].receipt["argv"] = json!(["cargo", "test"]),
                "changed_assertion" => {
                    runs[0].receipt["argv"] = python("import re; re.search('x','x'); assert False");
                    runs[1].receipt["argv"] = python("import re; re.search('x','x'); assert True");
                }
                "test_runner" => {
                    runs[0].receipt["argv"] =
                        python("import unittest; import re; re.search('(','x')");
                    runs[1].receipt["argv"] =
                        python("import unittest; import re; re.search('x','x')");
                }
                "bad_expectation" => {
                    runs[0].receipt["argv"] = python("items=[1,2]; assert len(items)==3");
                    runs[1].receipt["argv"] = python("items=[1,2,3]; assert len(items)==3");
                    runs[0].receipt["output"] = json!("File \"<string>\"\nAssertionError\n");
                }
                "later_failure" | "later_unresolved" | "unrecorded" => {
                    let mut late = runs[0].clone();
                    late.order = 3;
                    late.receipt["tool_call_ref"] = json!("late");
                    if case == "later_unresolved" {
                        late.receipt["complete"] = json!(false);
                    }
                    if case == "unrecorded" {
                        late.receipt["argv"] = Value::Null;
                    }
                    runs.push(late);
                }
                _ => (),
            }
            assert_eq!(
                evaluate_pair(&checks, &runs).unwrap_err().code(),
                "invalid_check_retraction",
                "{case}"
            );
        }
    }
    #[test]
    fn argparse_supersession_is_only_a_startup_error() {
        for (output,code,allowed) in [
            ("usage: check.py [-h] action\ncheck.py: error: unrecognized arguments: --check\n",2,true),
            ("usage: check.py [-h] action\ncheck.py: error: the following arguments are required: action\n",2,true),
            ("project test failed\nusage: check.py\ncheck.py: error: unrecognized arguments: --check\n",2,false),
            ("usage: check.py\ncheck.py: error: unrecognized arguments: --check\n",1,false),
        ] {
            let runs=vec![run("bad",1,code,json!(["python3","-B","check.py","--check"]),output),run("good",2,0,json!(["python3","-B","check.py","check"]),"")];
            let mut checks=vec![check("failed","bad"),check("passed","good")];
            checks[0]["command_correction"]=json!({"corrected_by":"good","reason":"argument typo"});
            let out=evaluate_pair(&checks,&runs);
            assert_eq!(out.is_ok(),allowed,"{output}");
        }
    }

    /// Owner store 2026-11-19 (cortex store, one attempt): the exact rerun of a failed
    /// rendered-layout check passed three times while another task wrote Rust files
    /// elsewhere in the shared checkout; no run kept a checkout digest, the failure
    /// could never close and the attempt was replanned. A run whose own work set was
    /// unchanged across the run and at submission now verifies, bound to that set.
    #[test]
    fn a_work_set_binding_verifies_a_pass_and_closes_its_failure() {
        let binding =
            json!({"kind":"work_set","work_set":["packages/ui/**"],"digest":"b".repeat(64)});
        let bad = run("bad", 1, 1, json!(["npm", "test"]), "failed");
        let mut good = run("good", 2, 0, json!(["npm", "test"]), "passed");
        // The checkout changed during the run: no checkout digest.
        good.receipt["revision_digest"] = Value::Null;
        let checks = vec![check("failed", "bad"), check("passed", "good")];
        let unbound = evaluate_pair(&checks, &[bad.clone(), good.clone()]).unwrap();
        assert_eq!(unbound[1]["evidence_gap"], "revision_mismatch");
        assert!(unbound[0]["closed_by"].is_null());
        good.work_set = Some(binding.clone());
        let out = evaluate_pair(&checks, &[bad, good]).unwrap();
        assert_eq!(out[1]["outcome"], "passed");
        assert_eq!(out[1]["evidence_strength"], "verified");
        assert_eq!(out[1]["revision_binding"], binding);
        assert_eq!(out[0]["closed_by"], "good");
        // An exact checkout match is reported as such; a model cannot supply either.
        let mut claimed = check("passed", "one");
        claimed["revision_binding"] = binding.clone();
        let exact = evaluate_pair(
            &[claimed.clone()],
            &[run("one", 1, 0, json!(["cargo", "test"]), "")],
        )
        .unwrap();
        assert_eq!(exact[0]["revision_binding"], json!({"kind":"checkout"}));
        let mut stale = run("one", 1, 0, json!(["cargo", "test"]), "");
        stale.receipt["revision_digest"] = json!("c".repeat(64));
        let out = evaluate_pair(&[claimed], &[stale]).unwrap();
        assert_eq!(out[0]["evidence_gap"], "revision_mismatch");
        assert!(out[0]["revision_binding"].is_null());
    }
    /// `open_failed_check` named nothing (an empty `missing`), and its hint always
    /// said to rerun the same command, also when the passing rerun had already run
    /// and only its revision binding was missing.
    #[test]
    fn open_items_name_the_check_its_receipt_and_the_actual_gap() {
        let bad = run("bad", 1, 1, json!(["npm", "test"]), "failed");
        let mut good = run("good", 2, 0, json!(["npm", "test"]), "passed");
        let items = |checks: &[Value], runs: &[Run]| {
            open_items(&evaluate_pair(checks, runs).unwrap(), runs)
        };
        let failed = check("failed", "bad");
        assert!(items(&[failed.clone()], &[bad.clone()])[0]
            .starts_with("check [bad]: no_passing_rerun: no later run of the same npm argv"));
        assert!(items(&[failed.clone()], &[bad.clone(), good.clone()])[0]
            .starts_with("check [bad]: rerun_not_cited: the latest rerun [good] passed"));
        let mut other = check("passed", "good");
        other["name"] = json!("other");
        assert!(
            items(&[failed.clone(), other], &[bad.clone(), good.clone()])[0]
                .contains("rerun_named_differently")
        );
        good.receipt["revision_digest"] = Value::Null;
        let mismatch = items(
            &[failed.clone(), check("passed", "good")],
            &[bad.clone(), good.clone()],
        );
        assert!(mismatch[0].starts_with("check [bad]: rerun_revision_mismatch: the passing rerun [good] does not verify: it observed neither"), "{mismatch:?}");
        assert!(!mismatch[0].contains("no later run"));
        let mut late = run("late", 3, 1, json!(["npm", "test"]), "again");
        late.receipt["tool_call_ref"] = json!("late");
        assert!(
            items(&[failed.clone()], &[bad.clone(), good.clone(), late])[0]
                .contains("later_run_failed: the latest rerun [late] failed too")
        );
        // A passed claim of a nonzero run is an open failure with its own item.
        assert!(items(&[check("passed", "bad")], &[bad.clone()])[0]
            .starts_with("check [bad]: nonzero_exit:"));
        // A closed failure names nothing.
        let ok = run("ok", 2, 0, json!(["npm", "test"]), "passed");
        assert!(items(&[failed, check("passed", "ok")], &[bad, ok]).is_empty());
    }
    fn store() -> (std::path::PathBuf, crate::Store) {
        let root = std::env::temp_dir().join(format!("work-set-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let store = crate::Store::open(
            crate::ProjectKey::new(&root, crate::StoreChannel::Dev).unwrap(),
            crate::OpenMode::Initialize,
        )
        .unwrap();
        crate::lifecycle::TaskMachine::new(&store)
            .create("t", "work set", "owner")
            .unwrap();
        (root, store)
    }
    #[test]
    fn work_sets_are_scope_and_own_writes_and_bind_only_a_stable_record() {
        let (root, store) = store();
        store
            .write(|db| {
                for (channel, assignment) in [("own", "a"), ("sibling", "b")] {
                    db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,assignment_id,attempt_id,owner_epoch,issued_at)
                        VALUES (?,'fixture','t','thread','worker',?,'x',1,'now')", (channel, assignment))?;
                }
                let write = |channel: &str, path: &str| {
                    super::super::event_tx(db, "t", "NATIVE_PROJECT_WRITTEN", json!({"channel_id":channel,"path":path,"sha256":"0"}))
                };
                write("own", "docs/notes.md")?;
                write("sibling", "crates/other.rs")?;
                let scope = vec!["packages/ui/**".to_owned(), "./packages/domain".to_owned()];
                assert_eq!(
                    work_set_tx(db, "t", Some("a"), &scope)?,
                    Some(vec!["docs/notes.md".into(), "packages/domain".into(), "packages/ui/**".into()])
                );
                // The whole project, or no assignment, leaves no narrower set.
                for whole in [".", "**", "*.md", "/abs", "../up"] {
                    assert_eq!(work_set_tx(db, "t", Some("a"), &[whole.to_owned()])?, None, "{whole}");
                }
                assert_eq!(work_set_tx(db, "t", None, &scope)?, None);
                let set = work_set_tx(db, "t", Some("a"), &scope)?.unwrap();
                let started = super::super::event_tx(db, "t", "NATIVE_COMMAND_STARTED", json!({"ref":"r1"}))?;
                super::super::event_tx(db, "t", "NATIVE_COMMAND_STARTED", json!({"ref":"r2"}))?;
                record_work_set_tx(db, "t", "r1", "a", &set, &"d".repeat(64), &"d".repeat(64))?;
                record_work_set_tx(db, "t", "r2", "a", &set, &"d".repeat(64), &"e".repeat(64))?;
                // A record is written once; an unknown command has none.
                record_work_set_tx(db, "t", "r1", "a", &set, &"f".repeat(64), &"f".repeat(64))?;
                record_work_set_tx(db, "t", "unknown", "a", &set, &"d".repeat(64), &"d".repeat(64))?;
                let stable = work_set_record(db, "t", "r1")?.expect("stable record");
                assert_eq!(stable["end_digest"], "d".repeat(64));
                assert_eq!(stable["started_event"], started);
                assert!(work_set_record(db, "t", "r2")?.is_none(), "changed during the run");
                assert!(work_set_record(db, "t", "unknown")?.is_none());
                // A sibling's write elsewhere does not count; this assignment's does.
                assert!(!wrote_outside_after(db, "t", "a", started, &set)?);
                write("own", "packages/ui/src/a.ts")?;
                assert!(!wrote_outside_after(db, "t", "a", started, &set)?);
                write("own", "crates/late.rs")?;
                assert!(wrote_outside_after(db, "t", "a", started, &set)?);
                // Completion re-establishes only the recorded binding of that record.
                let binding = work_set_binding(&stable);
                let mut runs = vec![run("r1", 1, 0, json!(["npm", "test"]), "")];
                let mut recorded = check("passed", "r1");
                recorded["evidence_strength"] = json!("verified");
                recorded["revision_binding"] = json!({"kind":"work_set","work_set":["**"],"digest":"d".repeat(64)});
                accept_recorded_bindings(db, "t", &[recorded.clone()], &mut runs)?;
                assert!(runs[0].work_set.is_none(), "a forged binding");
                recorded["revision_binding"] = binding.clone();
                accept_recorded_bindings(db, "t", &[recorded], &mut runs)?;
                assert_eq!(runs[0].work_set, Some(binding));
                Ok(())
            })
            .unwrap();
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }
}
