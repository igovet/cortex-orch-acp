//! Completed-work controls, including exact legacy source-bound verification.
//! Shapes the port cannot assess make the whole result's evidence unavailable
//! (warnings, no acceptance, no pass); they never select another interpreter.
use super::task_facts::safe_bytes;
use cortex_core::{CoreError, CoreResult, Store};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

pub(super) struct Evidence {
    pub work: bool,
    pub warnings: bool,
    pub approved: bool,
    pub acknowledge: bool,
    pub verified: bool,
    pub observed: Option<String>,
    pub unexplained: bool,
    pub open_counts: [u64; 3],
    /// The recorded verification digest no longer matches the project folder.
    pub drifted: bool,
    /// The folder changed since verification, but only outside the files this
    /// project-folder sprint task recorded writing; those still hold its content.
    pub scoped: bool,
    /// Visible notes that do not make the evidence unavailable (same-model review,
    /// self-reported checks the runtime's own passing verification supersedes).
    pub review_warnings: Vec<String>,
    /// The open checks behind `unexplained`, named (`open_note`), in order. Native tasks
    /// only: a retained task keeps the wording its recorded projection had.
    pub blocking: Vec<String>,
    pub not_run_checks: Vec<Value>,
}
/// At most this many superseded self-reported checks are named one by one.
const NOTED_CLAIMS: usize = 10;
/// One self-reported check that does not block acceptance because the runtime's own
/// verification of this exact version passed, in plain words. Bounded.
fn superseded_note(stage: &str, check: &Value, outcome: &str) -> String {
    let who = if stage == "review" {
        "Reviewer"
    } else {
        "Worker"
    };
    let name: String = check["name"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("unnamed check")
        .chars()
        .take(160)
        .collect();
    let why = match check["evidence_gap"].as_str() {
        Some("revision_mismatch") => {
            "it ran before the final edits, so that run does not count".to_owned()
        }
        Some("trivial_command") => {
            "its command only shows something and does not test the claim".to_owned()
        }
        Some(_) => "the runtime could not confirm the command run it cited".to_owned(),
        None if outcome == "not_run" => match check["blocker"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            Some(blocker) => format!(
                "not run: {}",
                blocker
                    .chars()
                    .take(200)
                    .collect::<String>()
                    .trim_end_matches('.')
            ),
            None => "not run".to_owned(),
        },
        None => "not confirmed by a recorded command run".to_owned(),
    };
    format!("{who} check \u{201c}{name}\u{201d}: {why}.")
}
/// A skipped check is display evidence only, never a rerun target or a pass.
fn not_run_row(
    name: &str,
    command: Option<&Value>,
    source: &str,
    policy: bool,
    manual: bool,
    reason: &str,
    technical: bool,
) -> Value {
    let mut row = json!({
        "name": name,
        "outcome": "not_run",
        "source": source,
        "kind": if policy { "policy" } else if manual { "manual" } else { "automated" },
        "detail": reason,
        "rerunnable": false,
    });
    if let Some(command) = command.filter(|argv| nonempty_command(argv)) {
        row["command"] = command.clone();
    }
    if technical {
        row["technicalReason"] = json!(reason);
    }
    row
}
fn outcome_words(outcome: &str) -> &'static str {
    match outcome {
        "failed" => "failed",
        "not_run" => "was not run",
        "unverified" => "is not confirmed by a recorded run",
        _ => "has no usable outcome",
    }
}
/// One open check that keeps Accept off, in plain words and bounded, so the reason
/// names what blocks instead of a generic instruction (`Evidence::blocking`).
fn open_note(stage: &str, check: &Value, outcome: &str) -> String {
    let who = if stage == "review" {
        "Reviewer"
    } else {
        "Worker"
    };
    let name: String = check["name"]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("unnamed check")
        .chars()
        .take(160)
        .collect();
    format!(
        "{who} check \u{201c}{name}\u{201d} {}.",
        outcome_words(outcome)
    )
}
/// Whether every file this task recorded writing (Cortex project writes and their
/// reconciliations, before its verification event `before`) still holds exactly the
/// content it wrote, and nothing of the task could have changed the folder unrecorded
/// in a way this cannot see (a command still without its end). Fail-closed: no
/// recorded write, an unreadable/unsafe/oversized file or any malformed record is false.
fn own_writes_unchanged(
    db: &Connection,
    task: &str,
    root: &std::path::Path,
    before: i64,
) -> CoreResult<bool> {
    let mut q = db.prepare(
        "SELECT payload_json FROM runtime_events WHERE task_id=? AND type IN ('NATIVE_PROJECT_WRITTEN','NATIVE_PROJECT_RECONCILED') AND id<? ORDER BY id LIMIT 1001",
    )?;
    let rows = q
        .query_map((task, before), |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if rows.is_empty() || rows.len() > 1000 {
        return Ok(false);
    }
    let mut written = std::collections::BTreeMap::new();
    for raw in rows {
        let Ok(value) = serde_json::from_str::<Value>(&raw) else {
            return Ok(false);
        };
        let Some(path) = value["path"].as_str() else {
            return Ok(false);
        };
        // The latest record of a path says what the file holds; it must name a content digest.
        written.insert(path.to_owned(), value["sha256"].clone());
    }
    let Some(written) = written
        .into_iter()
        .map(|(path, sha)| {
            valid_digest(&sha).then(|| (path, sha.as_str().unwrap_or_default().to_owned()))
        })
        .collect::<Option<Vec<_>>>()
    else {
        return Ok(false);
    };
    let unfinished: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events s WHERE s.task_id=? AND s.type='NATIVE_COMMAND_STARTED'
            AND NOT EXISTS(SELECT 1 FROM runtime_events f WHERE f.task_id=s.task_id AND f.type='NATIVE_COMMAND_FINISHED'
                AND json_extract(f.payload_json,'$.receipt.tool_call_ref')=json_extract(s.payload_json,'$.ref')))",
        [task],
        |r| r.get(0),
    )?;
    if unfinished {
        return Ok(false);
    }
    for (path, sha) in written {
        match safe_bytes(root, &path, 64 * 1024 * 1024) {
            Ok(data) if format!("{:x}", Sha256::digest(&data)) == sha => {}
            _ => return Ok(false),
        }
    }
    Ok(true)
}
fn count_open(counts: &mut [u64; 3], outcome: &str, manual: bool) {
    if !manual {
        if let Some(index) = ["failed", "not_run", "unverified"]
            .iter()
            .position(|s| *s == outcome)
        {
            counts[index] += 1;
        }
    }
}
/// Internal marker for retained evidence this reader cannot interpret (malformed
/// JSON shapes, counts beyond its bounds). `evaluate` turns it into explicit
/// unavailable evidence; it is never a pass and never leaves this module family.
pub(super) const DEGRADED: &str = "task_facts_evidence_degraded";
pub(super) fn degraded<T>() -> CoreResult<T> {
    Err(CoreError::Refused(DEGRADED))
}
fn pending<T>() -> CoreResult<T> {
    degraded()
}
fn commands(value: &Value) -> CoreResult<&[Value]> {
    value
        .as_array()
        .map(Vec::as_slice)
        .ok_or(CoreError::Refused(DEGRADED))
}
/// Which of one provisioned node's claimed checks the native assessor
/// (`cortex_core::lifecycle::worker_checks`, the same one completion uses) re-derives
/// as withdrawn, mirroring Python `active_checks`: a failed claim is omitted only when
/// the fresh assessment says "retracted" with the same `retracted_by` the claim
/// persisted. The attempt's runs come from its trusted channels; output evidence is
/// bound to that attempt's own non-truncated large outputs. Any gap (no provisioning,
/// no channel, an assessor refusal, unbound output) leaves the claim a failed check.
const RESOLUTIONS: [&str; 3] = ["retracted_by", "closed_by", "superseded_by"];
fn assessor_resolved(check: &Value) -> bool {
    cortex_core::lifecycle::worker_checks::resolved(check)
}
pub(crate) fn withdrawn(
    db: &Connection,
    task: &str,
    assignment: Option<&str>,
    result: &Value,
    checks: &[Value],
) -> Vec<bool> {
    let none = vec![false; checks.len()];
    if !checks
        .iter()
        .any(|c| c["outcome"] == "failed" && RESOLUTIONS.iter().any(|k| c[*k].is_string()))
    {
        return none;
    }
    let attempt = (|| -> CoreResult<Option<(String, Option<String>)>> {
        let row: Option<String> = db
            .query_row(
                "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
                (task, assignment),
                |r| r.get(0),
            )
            .optional()?;
        let Some(attempt) = row else { return Ok(None) };
        let channel: Option<String> = db
            .query_row(
                "SELECT c.channel_id FROM trusted_channels c JOIN delegation_provisioning p ON p.task_id=c.task_id AND p.attempt_id=c.attempt_id WHERE c.task_id=? AND p.idempotency_key=? ORDER BY c.rowid DESC LIMIT 1",
                (task, assignment),
                |r| r.get(0),
            )
            .optional()?;
        Ok(Some((attempt, channel)))
    })();
    let Ok(Some((attempt, Some(channel)))) = attempt else {
        return none;
    };
    let derived = (|| -> CoreResult<Vec<Value>> {
        use cortex_core::lifecycle::worker_checks as assessor;
        let mut runs = assessor::load_runs(db, task, &channel)?;
        let mut artifacts = std::collections::BTreeMap::new();
        for run in &runs {
            if let Some(id) = run.receipt["large_output_id"].as_str() {
                if let Some(source) = attempt_output(db, task, id, &attempt)? {
                    artifacts.insert(id.to_owned(), source);
                }
            }
        }
        assessor::observe_outputs(db, task, &mut runs, &artifacts)?;
        // Like completion: a run bound to its runtime-verified work set keeps that
        // binding, so a later passing run of a project-part check still closes the
        // earlier failure here (owner store 2026-11-23).
        assessor::accept_recorded_bindings(db, task, checks, &mut runs)?;
        assessor::evaluate(checks, &runs, result["project_digest"].as_str())
    })();
    let Ok(now) = derived else { return none };
    checks
        .iter()
        .zip(&now)
        .map(|(claimed, fresh)| {
            // Retracted, closed by a later passing run of the same check, or superseded by a
            // corrected command: each counts only when the fresh assessment derives the very
            // same reference the claim persisted, and no other.
            claimed["outcome"] == "failed"
                && assessor_resolved(fresh)
                && RESOLUTIONS.iter().any(|k| claimed[*k].is_string())
                && RESOLUTIONS.iter().all(|k| claimed[*k] == fresh[*k])
        })
        .collect()
}
/// The recorded content hash of one complete large output of `attempt`, as the result's
/// completion bound it. A retained (legacy) output is scoped to the attempt's assignment
/// and is taken as recorded. A native one is scoped to the channel that ran the command,
/// one of that attempt's trusted channels; like completion, its stored file must still
/// hash to the recorded digest and size. Anything else (another attempt's output, a
/// truncated or missing file, an unsafe name) is None: the run's output stays unverified.
fn attempt_output(
    db: &Connection,
    task: &str,
    id: &str,
    attempt: &str,
) -> CoreResult<Option<String>> {
    let row: Option<(Option<String>, String, i64, bool)> = db
        .query_row(
            "SELECT l.source_ref,l.filename,l.byte_count,
                EXISTS(SELECT 1 FROM delegation_provisioning p WHERE p.task_id=l.task_id AND p.idempotency_key=l.owner_scope AND p.attempt_id=?3)
             FROM large_outputs l WHERE l.task_id=?1 AND l.id=?2 AND l.truncated=0 AND (
                EXISTS(SELECT 1 FROM delegation_provisioning p WHERE p.task_id=l.task_id AND p.idempotency_key=l.owner_scope AND p.attempt_id=?3)
                OR EXISTS(SELECT 1 FROM trusted_channels c WHERE c.task_id=l.task_id AND c.channel_id=l.owner_scope AND c.attempt_id=?3))",
            (task, id, attempt),
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    let Some((Some(source), file, size, assignment_scoped)) = row else {
        return Ok(None);
    };
    if assignment_scoped {
        return Ok(Some(source));
    }
    let Some(directory) = db.path().and_then(|p| std::path::Path::new(p).parent()) else {
        return Ok(None);
    };
    if file.is_empty()
        || file.contains('/')
        || file.contains("..")
        || task.contains('/')
        || task.contains("..")
    {
        return Ok(None);
    }
    let path = directory.join(task).join(".native-output").join(&file);
    let Ok(mut opened) = cortex_core::store::locks::open_private(&path, false, true) else {
        return Ok(None);
    };
    Ok(match crate::native_commands::fingerprint(&mut opened) {
        Ok((hash, count)) if hash == source && count as i64 == size => Some(source),
        _ => None,
    })
}
/// Paid-probe words in the command a cited gateway receipt recorded. The
/// receipt's own strength is not assessed here, so any such mention is treated as
/// a policy-blocked disclosure (stricter than the legacy assessor, never looser).
fn receipt_paid(db: &Connection, task: &str, reference: &str) -> CoreResult<bool> {
    let mut q = db.prepare(
        "SELECT result_json FROM runtime_requests WHERE task_id=? AND instr(result_json,?)>0 LIMIT 21",
    )?;
    let rows = q
        .query_map((task, reference), |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if rows.len() > 20 || rows.iter().map(String::len).sum::<usize>() > 8 * 1024 * 1024 {
        return Ok(true);
    }
    for raw in rows {
        let Ok(saved) = serde_json::from_str::<Value>(&raw) else {
            return Ok(true);
        };
        if paid_command(&saved["receipt"]["argv"]) || paid_command(&saved["argv"]) {
            return Ok(true);
        }
    }
    Ok(false)
}
fn nonempty_command(value: &Value) -> bool {
    value.as_array().is_some_and(|a| {
        !a.is_empty()
            && a.iter().all(|w| {
                w.as_str()
                    .is_some_and(|s| !s.is_empty() && !s.contains('\0'))
            })
    })
}
fn policy(check: &Value) -> bool {
    // A limitation the runtime itself disclosed (an unread file, an earlier failed
    // check, an owner-accepted gap) is never a policy restriction, whatever words its
    // explanation uses: on 2026-11-21 "after repeated coverage refusals" matched
    // "refus" and held an Automatic sprint for the owner.
    if check["source"] == "runtime" {
        return paid_command(&check["command"]);
    }
    let text = check["blocker"].as_str().unwrap_or("").to_lowercase();
    [
        "paid",
        "authoriz",
        "approval",
        "policy",
        "consent",
        "permission",
        "denied",
        "refus",
        "prohibit",
    ]
    .iter()
    .any(|word| text.contains(word))
        || paid_command(&check["command"])
}
/// The repository's explicit no-paid reporting convention. Only a skipped
/// observation outside the runtime verification plan can be informational.
fn optional_paid_observation(check: &Value, expected: &[Value]) -> bool {
    let command = &check["command"];
    check["outcome"] == "not_run"
        && check["tool_call_ref"].is_null()
        && (command.is_null() || (paid_command(command) && !expected.contains(command)))
        && check["blocker"].as_str().is_some_and(unrequested_paid)
}
/// The documented blocker of a paid live check the owner did not request, in English
/// or Russian, optionally followed by more detail ("... for this recovery; ..."):
/// on 2026-11-22 an added clause turned the convention into an owner hold.
fn unrequested_paid(reason: &str) -> bool {
    let reason = reason.trim().to_lowercase();
    [
        "paid live check not requested by the owner",
        "платная живая проверка не запрошена владельцем",
    ]
    .iter()
    .any(|phrase| {
        reason.strip_prefix(phrase).is_some_and(|rest| {
            rest.is_empty() || rest.starts_with(['.', ';', ',', ':', ' ', '(', '—', '-'])
        })
    })
}
fn paid_command(value: &Value) -> bool {
    value.as_array().is_some_and(|a| {
        a.windows(2).any(|pair| {
            pair[0].as_str().and_then(|path| path.rsplit('/').next())
                == Some("cortex-paseo-tools.sh")
                && matches!(pair[1].as_str(), Some("probe-codex" | "probe-claude"))
        }) || a.iter().filter_map(Value::as_str).any(|w| {
            w.contains("cortex_codex_probe.py")
                || w.contains("cortex_claude_probe.py")
                || w == "--i-authorize-paid-call"
        })
    })
}
pub(super) fn valid_digest(value: &Value) -> bool {
    value.as_str().is_some_and(|s| {
        s.len() == 64
            && s.bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    })
}
/// Native hosts record `native_effects::observe` digests of the project folder, a
/// different domain from Python's `capture_revision`. The caller observes that
/// folder before reading (`native`); None is an unobservable native workspace.
/// Either domain binds exact content, so a native task matches either one.
/// `own_writes`: a native project-folder sprint task, whose result goes stale only
/// when a file it recorded writing changed (later tasks write the same folder).
pub(super) fn evaluate(
    db: &Connection,
    store: &Store,
    task: &str,
    revision: i64,
    native: Option<&str>,
    own_writes: bool,
) -> CoreResult<Evidence> {
    match evaluate_exact(db, store, task, revision, native, own_writes) {
        Err(CoreError::Refused(DEGRADED)) => {
            let work: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND revision=?)",
                (task, revision),
                |r| r.get(0),
            )?;
            Ok(Evidence {
                work,
                warnings: true,
                approved: false,
                acknowledge: false,
                verified: false,
                observed: None,
                unexplained: false,
                open_counts: [0; 3],
                drifted: false,
                scoped: false,
                review_warnings: vec![],
                blocking: vec![],
                not_run_checks: vec![],
            })
        }
        other => other,
    }
}
fn evaluate_exact(
    db: &Connection,
    store: &Store,
    task: &str,
    revision: i64,
    native: Option<&str>,
    own_writes: bool,
) -> CoreResult<Evidence> {
    let native_task: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type IN ('TASK_CREATED','NATIVE_TASK_ADOPTED')
            AND json_extract(payload_json,'$.execution_host')='rust')",
        [task],
        |r| r.get(0),
    )?;
    let attempts = super::task_facts_attempts::Attempts::read(db, store, task, revision)?;
    let plan: Option<String> = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let plan: Value = serde_json::from_str(plan.as_deref().unwrap_or("{}"))?;
    let empty = Value::Array(vec![]);
    let expected = commands(plan.get("verification").unwrap_or(&empty))?;
    let verification: Option<(i64,String,String)> = db.query_row("SELECT id,type,payload_json FROM runtime_events WHERE task_id=? AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1",
        [task], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
    let work: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND revision=?)",
        (task, revision),
        |r| r.get(0),
    )?;
    let mut warnings = false;
    let mut count = 0;
    let mut acknowledge = false;
    let mut verified = false;
    let mut observed = None;
    let mut unexplained = false;
    let mut open_counts = [0; 3];
    let mut drifted = false;
    let mut scoped = false;
    let mut review_warnings = Vec::new();
    let mut blocking = Vec::new();
    let mut not_run_checks = Vec::new();
    let final_closures = super::task_facts_rerun::final_closures(db, store, task, revision)?;
    if let Some((id, kind, raw)) = &verification {
        let value: Value = serde_json::from_str(raw)?;
        let current = value["revision"] == revision;
        let current_result = current
            && db.query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='TASK_REOPENED' AND id>?)",
                (task, *id),
                |r| r.get::<_, bool>(0),
            )?;
        let bound = current && valid_digest(&value["revision_digest"]);
        let checks = commands(value.get("checks").unwrap_or(&empty))?;
        let mut all_passed = true;
        for i in 0..expected.len().max(checks.len()) {
            let check = checks.get(i);
            let outcome = match check {
                None => "not_run",
                Some(c)
                    if !current || expected.get(i) != c.get("argv") || c["complete"] != true =>
                {
                    "unverified"
                }
                Some(c)
                    if c["exit_code"].as_i64() == Some(0)
                        && bound
                        && c["revision_digest"] == value["revision_digest"] =>
                {
                    "passed"
                }
                Some(c) if c["exit_code"].as_i64().is_some_and(|n| n != 0) => "failed",
                _ => "unverified",
            };
            let attempt = attempts
                .get(Some("runtime-verification"), i)
                .filter(|attempt| attempt.id > *id);
            let outcome = attempt.map(|a| a.outcome).unwrap_or(outcome);
            let manual = expected.get(i).is_none() && matches!(outcome, "not_run" | "unverified");
            all_passed &= outcome == "passed";
            let policy = expected.get(i).is_some_and(paid_command);
            let open = policy || outcome != "passed";
            count_open(
                &mut open_counts,
                if policy { "not_run" } else { outcome },
                manual,
            );
            acknowledge |= !manual && open;
            let unaccounted = !manual
                && open
                && !policy
                && (outcome != "not_run"
                    || attempts
                        .get(Some("runtime-verification"), i)
                        .filter(|a| a.id > *id)
                        .is_none());
            if current_result && outcome == "not_run" {
                let command = expected
                    .get(i)
                    .or_else(|| check.and_then(|c| c.get("argv")));
                let name = command
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ");
                let reason = attempt
                    .and_then(|a| a.blocker.as_deref())
                    .or_else(|| check.and_then(|c| c["blocker"].as_str()))
                    .filter(|reason| !reason.trim().is_empty())
                    .unwrap_or(if policy {
                        "This command is restricted by verification policy."
                    } else {
                        "No execution was recorded for this runtime check."
                    });
                not_run_checks.push(not_run_row(
                    &name,
                    command,
                    "runtime",
                    policy,
                    manual,
                    reason,
                    !unaccounted,
                ));
            }
            if open && !manual && native_task {
                blocking.push(format!(
                    "Runtime check {} {}.",
                    i + 1,
                    outcome_words(outcome)
                ));
            }
            unexplained |= unaccounted;
            count += 1;
        }
        verified = current && kind == "VERIFICATION_PASSED" && bound && all_passed;
        warnings |= current && kind == "VERIFICATION_PASSED" && !verified;
        if bound {
            let has_workspace: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='task_workspaces')",
                [], |r| r.get(0))?;
            let workspace: Option<String> = if has_workspace {
                db.query_row(
                    "SELECT workspace_root FROM task_workspaces WHERE task_id=?",
                    [task],
                    |r| r.get(0),
                )
                .optional()?
            } else {
                None
            };
            let root: String = match workspace {
                Some(root) => root,
                None => db.query_row("SELECT project_root FROM tasks WHERE id=?", [task], |r| {
                    r.get(0)
                })?,
            };
            // A native task adopted after Python verification keeps a legacy digest. A
            // verification the native runtime recorded (any of a task it created, or one
            // after the adoption) is in the native domain only: a legacy capture of the
            // folder could never match it, so it is not taken (it walks the whole folder).
            let legacy_domain: bool = !native_task
                || db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='NATIVE_TASK_ADOPTED' AND id>?)",
                    (task, *id),
                    |r| r.get(0),
                )?;
            // The checkout the verification ran in: `checkout_digest` when it was bound
            // to the task's own files (its `revision_digest` is then that binding).
            let checkout = value
                .get("checkout_digest")
                .unwrap_or(&value["revision_digest"]);
            let actual = if native_task && native.is_some_and(|d| *checkout == d) {
                native.map(str::to_owned)
            } else if native_task && !legacy_domain {
                None
            } else if native_task {
                super::legacy_source::capture(std::path::Path::new(&root)).unwrap_or(None)
            } else {
                super::legacy_source::capture(std::path::Path::new(&root))?
            };
            if actual.as_deref() == checkout.as_str() {
                observed = actual;
            } else if native_task
                && own_writes
                && native.is_some()
                && verified
                && own_writes_unchanged(db, task, std::path::Path::new(&root), *id)?
            {
                // A project-folder sprint task shares the folder with the tasks after
                // it: their writes leave this result as verified while every file it
                // wrote still holds what it wrote. Its identity stays the verified one.
                observed = checkout.as_str().map(str::to_owned);
                scoped = true;
            } else {
                warnings = true;
                drifted = true;
            }
        }
    } else if work {
        count += expected.len();
        open_counts[1] += expected
            .iter()
            .filter(|argv| nonempty_command(argv))
            .count() as u64;
        acknowledge |= expected
            .iter()
            .any(|argv| argv.as_array().is_some_and(|a| !a.is_empty()));
    }
    let mut q = db.prepare("SELECT stage,assignment_ref,result_json FROM runtime_nodes WHERE task_id=? AND revision=? ORDER BY rowid LIMIT 1001")?;
    let nodes = q
        .query_map((task, revision), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if nodes.len() > 1000 {
        return pending();
    }
    if nodes
        .iter()
        .map(|n| n.2.as_deref().unwrap_or("").len())
        .sum::<usize>()
        > 8 * 1024 * 1024
    {
        return pending();
    }
    let has_review = nodes.iter().any(|n| n.0 == "review");
    let mut approved = false;
    let settled = native_task && verified && observed.is_some();
    let mut superseded = Vec::new();
    for (stage, assignment, raw) in nodes {
        let result: Value = serde_json::from_str(raw.as_deref().unwrap_or("{}"))?;
        let checks = commands(result.get("checks").unwrap_or(&empty))?;
        let provision: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?)",
            (task,assignment.as_deref()),|r|r.get(0))?;
        let paid_receipts = if provision {
            let mut paid = std::collections::BTreeSet::new();
            for check in checks {
                // A cited gateway receipt can make a pass/failure policy-blocked
                // through its recorded command; look at that command directly.
                if check["outcome"] == "passed" || check["outcome"] == "failed" {
                    if let Some(reference) = check["tool_call_ref"]
                        .as_str()
                        .filter(|s| s.starts_with("gateway:"))
                    {
                        let authorized: bool = db.query_row(
                            "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='TOOL_AUTHORIZED' AND json_extract(payload_json,'$.tool_call_ref')=?)",
                            (task,reference), |r|r.get(0))?;
                        // Like `_policy_blocker(argv, check)`, a policy blocker on the claim
                        // also makes an authorized receipt's pass/failure not_run.
                        if authorized && (policy(check) || receipt_paid(db, task, reference)?) {
                            paid.insert(reference.to_owned());
                        }
                    }
                }
            }
            paid
        } else {
            Default::default()
        };
        let report: Option<String> = db
            .query_row(
                "SELECT filename FROM reports WHERE task_id=? AND id=?",
                (task, result["report_ref"].as_str()),
                |r| r.get(0),
            )
            .optional()?;
        let actual = match report {
            Some(name) => {
                // Match the legacy single-component publication namespace.
                if task.contains('/') || name.contains('/') {
                    None
                } else {
                    let path = format!("{task}/{name}");
                    // A report beyond the bound is unverifiable evidence (warnings), not a
                    // reason to select another reader.
                    match safe_bytes(&store.key().directory(), &path, 64 * 1024 * 1024) {
                        Ok(data) => Some(format!("{:x}", Sha256::digest(&data))),
                        Err(_) => None,
                    }
                }
            }
            None => None,
        };
        let matches = actual
            .as_deref()
            .is_some_and(|d| result["report_digest"] == d);
        warnings |= !matches;
        if stage == "review" {
            // A review that ran on the reviewed work's own model is shown with a
            // visible warning. The runtime recorded it knowingly (no other model was
            // available or selected); it does not make the evidence unavailable.
            // Any other unreadable independence status stays unavailable evidence.
            match result["review_independence"]["status"].as_str() {
                None | Some("independent") => {}
                Some("warning" | "unverified") => review_warnings.push(
                    result["review_independence"]["warning"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("The reviewer's independence from the work it reviewed is not confirmed.")
                        .chars()
                        .take(500)
                        .collect(),
                ),
                Some(_) => warnings = true,
            }
            // An INCOMPLETE review whose unreachable parts the runtime disclosed as
            // unverified limits is the review the runtime continued to verification
            // with; its limits stay notes (never passes). Treating it as unapproved
            // held an Automatic sprint for the owner after the task had completed
            // (owner store 2026-11-22).
            let disclosed =
                result["verdict"] == "INCOMPLETE" && result["review_limit_disclosed"] == true;
            approved =
                matches && (matches!(result["verdict"].as_str(), Some("APPROVE")) || disclosed);
        }
        let omitted = if provision {
            withdrawn(db, task, assignment.as_deref(), &result, checks)
        } else {
            vec![false; checks.len()]
        };
        for (index, check) in checks.iter().enumerate() {
            // Python `active_checks`: a freshly re-derived withdrawal is omitted
            // (the index of later checks is unchanged).
            if omitted[index] {
                continue;
            }
            let original = check["outcome"].as_str().unwrap_or("unknown");
            let outcome = match original {
                "passed"
                    if !matches || check["tool_call_ref"].as_str().is_none_or(str::is_empty) =>
                {
                    "unverified"
                }
                "passed" | "failed" | "not_run" | "unverified" | "not_applicable" => original,
                _ => "unknown",
            };
            let target = matches!(original, "not_run" | "unverified");
            let receipt_policy = check["tool_call_ref"]
                .as_str()
                .is_some_and(|r| paid_receipts.contains(r));
            let outcome = if target && policy(check) || receipt_policy {
                "not_run"
            } else if final_closures
                .get(&(assignment.clone(), index))
                .is_some_and(|start| {
                    attempts
                        .get(assignment.as_deref(), index)
                        .is_none_or(|a| a.id <= *start)
                })
            {
                if matches {
                    "passed"
                } else {
                    outcome
                }
            } else {
                attempts
                    .get(assignment.as_deref(), index)
                    .map(|a| {
                        if a.closed && !matches {
                            outcome
                        } else {
                            a.outcome
                        }
                    })
                    .unwrap_or(outcome)
            };
            let manual = if receipt_policy {
                false
            } else if target {
                !nonempty_command(&check["command"]) && !policy(check)
            } else {
                matches!(outcome, "not_run" | "unverified")
            };
            let policy_gap = (target && policy(check)) || receipt_policy;
            // A native result whose runtime verification of this exact version passed
            // on the unchanged folder: a self-reported check that is only unconfirmed
            // or not run (never a failure, never a policy-blocked one) is a visible
            // note, not an open check. Runtime verification and independent review
            // decide; a superseded claim cannot be rerun from the result anyway.
            // A limitation the runtime itself recorded (for example changes it could not
            // list for review) is not a self-reported claim: it stays a manual check.
            let superseded_by_verification = settled
                && matches!(outcome, "not_run" | "unverified")
                && !policy_gap
                && check["source"] != "runtime";
            let attempt = attempts.get(assignment.as_deref(), index);
            let optional_policy = settled
                && matches
                && attempt.is_none()
                && !receipt_policy
                && optional_paid_observation(check, expected);
            if outcome == "not_run" {
                let attempt_reason = attempt.and_then(|a| a.blocker.as_deref());
                let reason = attempt_reason
                    .or_else(|| check["blocker"].as_str().filter(|s| !s.trim().is_empty()))
                    .unwrap_or(if policy_gap {
                        "This check is restricted by verification policy."
                    } else {
                        "No execution was recorded for this check."
                    });
                not_run_checks.push(not_run_row(
                    check["name"].as_str().unwrap_or(""),
                    check.get("command"),
                    if attempt_reason.is_some() {
                        "runtime"
                    } else {
                        "worker"
                    },
                    policy_gap,
                    manual,
                    reason,
                    superseded_by_verification || policy_gap || attempt_reason.is_some(),
                ));
            }
            if outcome == "unverified" && manual && check["source"] == "runtime" {
                // A limitation the runtime recorded is shown to the owner as a manual check
                // with its reason; it is never a pass and never an open check.
                let mut row = not_run_row(
                    check["name"].as_str().unwrap_or(""),
                    None,
                    "runtime",
                    false,
                    true,
                    check["blocker"]
                        .as_str()
                        .or_else(|| check["detail"].as_str())
                        .unwrap_or("The runtime could not confirm this."),
                    false,
                );
                row["outcome"] = json!("unverified");
                not_run_checks.push(row);
            }
            if superseded_by_verification {
                superseded.push(superseded_note(&stage, check, outcome));
                count += 1;
                continue;
            }
            if optional_policy {
                count += 1;
                continue;
            }
            let open = !matches!(outcome, "passed" | "not_applicable") && !manual;
            count_open(&mut open_counts, outcome, manual);
            acknowledge |= open;
            let unaccounted = open
                && !(target && policy(check))
                && !receipt_policy
                && (outcome != "not_run" || attempts.get(assignment.as_deref(), index).is_none());
            if open && native_task {
                blocking.push(open_note(&stage, check, outcome));
            }
            unexplained |= unaccounted;
            count += 1;
        }
    }
    if work && !has_review {
        let waived: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='REVIEW_NOT_REQUIRED' AND json_extract(payload_json,'$.revision')=?)",
            (task,revision),|r|r.get(0))?;
        approved |= waived;
    }
    if count > 100 {
        warnings = true;
        acknowledge = false;
    }
    if !superseded.is_empty() {
        review_warnings.push(format!(
            "The runtime's own verification of this version passed. {} self-reported check{} it does not cover {} shown for information and do{} not block acceptance:",
            superseded.len(),
            if superseded.len() == 1 { "" } else { "s" },
            if superseded.len() == 1 { "is" } else { "are" },
            if superseded.len() == 1 { "es" } else { "" },
        ));
        let more = superseded.len().saturating_sub(NOTED_CLAIMS);
        review_warnings.extend(superseded.into_iter().take(NOTED_CLAIMS));
        if more > 0 {
            review_warnings.push(format!(
                "And {more} more self-reported check{}.",
                if more == 1 { "" } else { "s" }
            ));
        }
    }
    Ok(Evidence {
        work,
        warnings,
        approved,
        acknowledge,
        verified,
        observed,
        unexplained,
        open_counts,
        drifted,
        scoped,
        review_warnings,
        blocking,
        not_run_checks,
    })
}

