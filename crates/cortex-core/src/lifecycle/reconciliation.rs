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
    if entries.is_empty() {
        return Ok(text.into());
    }
    if authored_disclosure(text) {
        return refuse("synthesis_disclosure_in_text");
    }
    let language: Option<String> = db.query_row("SELECT coalesce(
        (SELECT json_extract(payload_json,'$.settings.language') FROM runtime_events WHERE task_id=? AND type='NATIVE_TASK_SETTINGS' ORDER BY id DESC LIMIT 1),
        (SELECT language FROM task_language WHERE task_id=?))", (task,task), |r|r.get(0))?;
    let (heading, label) = if language.as_deref() == Some("ru") {
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
            json!({"text":text,"not_verified":args["not_verified"]}),
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
/// Files whose content differs from the first execution baseline since the task was
/// last reopened. None when that baseline metadata is missing or not that baseline.
fn changed_files(
    db: &Connection,
    task: &str,
    current: Option<&BTreeMap<String, String>>,
    current_ignored: Option<&str>,
) -> CoreResult<Option<Vec<String>>> {
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
    // The manifest lists only files Git would track or show. Changes to ignored files
    // are covered by their own metadata digest; unless it is observed and unchanged,
    // the list cannot account for every change.
    if current_ignored.is_none() || baseline["ignored_digest"].as_str() != current_ignored {
        return Ok(None);
    }
    let before: BTreeMap<String, String> = serde_json::from_str(&manifest)?;
    let names: BTreeSet<&String> = before.keys().chain(current.keys()).collect();
    Ok(Some(
        names
            .into_iter()
            .filter(|n| before.get(*n) != current.get(*n))
            .cloned()
            .collect(),
    ))
}
/// Changed files listed in a review brief; the rest are counted by folder.
pub const REVIEW_PACKET_FILES: usize = 200;
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
    if let (Some(changed), Some(current)) = (&manifest, current) {
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
    let total = files.len();
    let mut ordered: Vec<_> = files.iter().collect();
    if total > REVIEW_PACKET_FILES {
        // Keep directly observed task writes in view when the baseline delta is large.
        ordered.sort_by_key(|(path, _)| !last_writes.contains_key(*path));
    }
    let listed: Vec<Value> = ordered
        .iter()
        .copied()
        .take(REVIEW_PACKET_FILES)
        .map(|(path, (change, by))| {
            let mut file = json!({"path":path,"change":change,"by":by});
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
    let mut changed = json!({"files":listed,"total":total,"complete":complete,
        "source":if complete {"execution baseline manifest and recorded writes"} else {"recorded writes only"}});
    if !complete {
        changed["note"] = json!("The folder could not be compared with the execution baseline, so files changed by commands are not listed; inspect the predecessor reports and the folder for them.");
    }
    if total > listed.len() {
        changed["more"] = json!({"count":total - listed.len(),
            "by_folder":folders.iter().map(|(f, n)| json!({"folder":f,"files":n})).collect::<Vec<_>>(),
            "how":"read these folders with project_read to see the rest"});
    }
    let boundary = json!({"baseline_event":baseline_event,
        "recorded_task_paths":last_writes.len(),
        "guidance":"Use the objective and task write receipts across plans to focus your review. A matching hash binds the observed file bytes to the latest recorded write. For changed or unobserved bytes, inspect the relevant file and predecessor reports. Manifest-only paths have no confirmed writer. A whole-checkout diff may include unrelated work."});
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
    let outside: Vec<&String> = files
        .keys()
        .filter(|p| {
            !scope
                .iter()
                .any(|s| crate::scheduler::policy::overlaps(s, p))
        })
        .collect();
    Ok(
        json!({"changed_files":changed,"change_boundary":boundary,"predecessor_reports":reports,"read_scope":scope,
        "read_scope_covers_change":outside.is_empty(),"outside_read_scope":outside}),
    )
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
    if existing.is_some() || blocked || review_cycle != 1 || !gaps(db, task)?.is_empty() {
        return Ok(None);
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
    let Some(changed) = changed_files(db, task, current, current_ignored)? else {
        return Ok(None);
    };
    if changed.iter().any(|f| {
        f.contains('`')
            || f.chars()
                .any(|c| hidden_character(c) || matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}'))
    }) {
        return Ok(None);
    }
    let language: Option<String> = db.query_row("SELECT coalesce(
        (SELECT json_extract(payload_json,'$.settings.language') FROM runtime_events WHERE task_id=? AND type='NATIVE_TASK_SETTINGS' ORDER BY id DESC LIMIT 1),
        (SELECT language FROM task_language WHERE task_id=?))", (task,task), |r|r.get(0))?;
    let (done, summary_label, files_label) = if language.as_deref() == Some("ru") {
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
        json!({"text":text,"source":"retained-worker-summaries","summaries":summaries.len(),"changed_files":changed.len()}),
    )?;
    Ok(Some(text))
}
pub fn direct_answer_allowed(db: &Connection, task: &str) -> CoreResult<bool> {
    if super::automatic::enabled(db, task)? {
        return Ok(false);
    }
    let (revision,informational):(i64,bool)=db.query_row("SELECT revision,plan_json IS NULL
        AND NOT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=?)
        AND NOT EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND role_id!='orchestrator')
        FROM runtime_tasks WHERE task_id=?",(task,task,task),|r|Ok((r.get(0)?,r.get(1)?)))?;
    if revision == 0 && informational {
        return Ok(true);
    }
    let active:bool=db.query_row("SELECT EXISTS(SELECT 1 FROM runtime_nodes WHERE task_id=? AND status IN ('pending','running'))",[task],|r|r.get(0))?;
    if active {
        return Ok(false);
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
        AND s.type='STEERING_APPLIED' WHERE v.task_id=? AND v.type='VERIFICATION_PASSED' AND v.id>?
        AND json_extract(v.payload_json,'$.revision')=? AND NOT EXISTS(SELECT 1 FROM runtime_events f
            WHERE f.task_id=v.task_id AND f.id>v.id AND f.type='VERIFICATION_FAILED'))",(task,boundary,revision),|r|r.get(0))?;
    Ok(release || steered || stopped_release)
}
pub fn submit_answer_tx(
    db: &Transaction<'_>,
    task: &str,
    actor: &str,
    key: &str,
    args: &Value,
) -> CoreResult<Value> {
    request_tx(db, task, actor, key, "answer_submit", args, || {
        if domain::task_tx(db, task)?.0 != TaskState::Planning || !direct_answer_allowed(db, task)?
        {
            return refuse("direct_answer_not_allowed");
        }
        let text = disclosed(
            db,
            task,
            args["text"]
                .as_str()
                .ok_or(CoreError::Refused("synthesis_text_required"))?,
            &args["not_verified"],
        )?;
        db.execute(
            "UPDATE runtime_tasks SET synthesis=? WHERE task_id=?",
            (&text, task),
        )?;
        event_tx(
            db,
            task,
            "ANSWER_SUBMITTED",
            json!({"text":text,"not_verified":args["not_verified"]}),
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
        let commands = plan::verification(&args["verification"])?;
        let (state, revision) = domain::task_tx(db, task)?;
        let last:Option<(String,String)>=db.query_row("SELECT type,payload_json FROM runtime_events WHERE task_id=?
            AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED','PLAN_ACCEPTED') ORDER BY id DESC LIMIT 1",
            [task],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
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
        let failed: Value = serde_json::from_str(&raw)?;
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
