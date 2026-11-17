//! Revalidate literal-gap reruns from retained runtime evidence, never display claims.
use cortex_core::{CoreResult, Store};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::Path;

fn pending<T>() -> CoreResult<T> {
    super::task_facts_work::degraded()
}
fn exact_json(v: &Value) -> bool {
    match v {
        Value::Number(n) => n.is_i64() || n.is_u64(),
        Value::Array(a) => a.iter().all(exact_json),
        Value::Object(m) => m.values().all(exact_json),
        _ => true,
    }
}
fn events(
    db: &Connection,
    task: &str,
    kind: &str,
    field: &str,
    value: &Value,
) -> CoreResult<Vec<(i64, Value)>> {
    // Only internal, fixed field names reach this query. Three rows suffice to
    // reject a duplicate without scanning an unbounded retained history.
    let sql = format!("SELECT id,payload_json FROM runtime_events WHERE task_id=? AND actor='runtime' AND type=? AND json_extract(payload_json,'$.{field}')=? LIMIT 3");
    let mut q = db.prepare(&sql)?;
    let rows = q
        .query_map((task, kind, value.as_str()), |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if rows.iter().map(|r| r.1.len()).sum::<usize>() > 8 * 1024 * 1024 {
        return pending();
    }
    rows.into_iter()
        .map(|(id, s)| Ok((id, serde_json::from_str(&s)?)))
        .collect()
}

fn task_root(db: &Connection, task: &str) -> CoreResult<String> {
    let has_workspace: bool = db.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='task_workspaces')",
        [],
        |r| r.get(0),
    )?;
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
    Ok(root)
}
pub(super) fn source(db: &Connection, task: &str) -> CoreResult<Option<String>> {
    super::legacy_source::capture(Path::new(&task_root(db, task)?))
}

/// Exact literal target identity; receipt-derived failed/passed targets retain
/// their interpreter. The result accompanies it for immutable report binding.
pub(super) fn target_identity(
    db: &Connection,
    task: &str,
    revision: i64,
    target: &Value,
) -> CoreResult<Option<(Value, Value)>> {
    let root = task_root(db, task)?;
    if !Path::new(&root).is_absolute()
        || root
            .split('/')
            .skip(1)
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        // No identity is established for a malformed root; nothing closes.
        return Ok(None);
    }
    let Some(index) = target["index"]
        .as_u64()
        .and_then(|i| usize::try_from(i).ok())
    else {
        return Ok(None);
    };
    if target["assignment"] == "runtime-verification" {
        let plan: Option<String> = db.query_row(
            "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| r.get(0),
        )?;
        let raw = plan.as_deref().unwrap_or("{}");
        if raw.len() > 8 * 1024 * 1024 {
            return Ok(None);
        }
        let plan: Value = serde_json::from_str(raw)?;
        let Some(commands) = plan["verification"].as_array() else {
            return Ok(None);
        };
        let Some(argv) = commands.get(index) else {
            return Ok(None);
        };
        // Non-integer numbers have no byte-exact legacy hash; they close nothing.
        if !exact_json(argv) {
            return Ok(None);
        }
        let original = json!({"revision":revision,"argv":argv});
        let hash = format!(
            "{:x}",
            Sha256::digest(super::worker_history::dump(&original, true).as_bytes())
        );
        return Ok(Some((
            json!({"assignment":"runtime-verification","index":index,"check_hash":hash,"argv":argv,"cwd":root}),
            Value::Null,
        )));
    }
    let mut q = db.prepare("SELECT result_json FROM runtime_nodes WHERE task_id=? AND revision=? AND assignment_ref IS ? LIMIT 3")?;
    let nodes = q
        .query_map((task, revision, target["assignment"].as_str()), |r| {
            r.get::<_, Option<String>>(0)
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if nodes.is_empty() {
        return Ok(None);
    }
    if nodes.len() != 1 {
        return Ok(None);
    }
    let raw = nodes[0].as_deref().unwrap_or("{}");
    if raw.len() > 8 * 1024 * 1024 {
        return Ok(None);
    }
    let result: Value = serde_json::from_str(raw)?;
    let Some(checks) = result.get("checks").and_then(Value::as_array) else {
        return Ok(None);
    };
    let Some(original) = checks.get(index) else {
        return Ok(None);
    };
    if !matches!(original["outcome"].as_str(), Some("not_run" | "unverified")) {
        return Ok(None);
    }
    let Some(mut original) = original.as_object().cloned() else {
        return Ok(None);
    };
    original.remove("closed_by");
    let original = Value::Object(original);
    if !exact_json(&original) {
        return Ok(None);
    }
    let hash = format!(
        "{:x}",
        Sha256::digest(super::worker_history::dump(&original, true).as_bytes())
    );
    let argv = original["command"]
        .as_array()
        .filter(|a| {
            !a.is_empty()
                && a.iter().all(|v| {
                    v.as_str()
                        .is_some_and(|s| !s.is_empty() && !s.contains('\0'))
                })
        })
        .cloned()
        .unwrap_or_default();
    Ok(Some((
        json!({"assignment":target["assignment"],"index":index,"check_hash":hash,"argv":argv,"cwd":root}),
        result,
    )))
}

pub(super) fn closed(
    db: &Connection,
    store: &Store,
    task: &str,
    revision: i64,
    row_id: i64,
    payload: &Value,
    check: &Value,
) -> CoreResult<bool> {
    let binding = &payload["binding"];
    let target = &binding["target"];
    if target["assignment"] == "runtime-verification" {
        return Ok(false);
    }
    let Some((identity, result)) = target_identity(db, task, revision, target)? else {
        return Ok(false);
    };
    let root = &identity["cwd"];
    if *target != identity
        || check["argv"] != identity["argv"]
        || !check["started_event_id"].as_i64().is_some_and(|id| id > 0)
        || check.get("cwd").unwrap_or(root) != root
    {
        return Ok(false);
    }
    let reference = &check["command_ref"];
    let starts = events(
        db,
        task,
        "CHECK_EXECUTION_STARTED",
        "command_ref",
        reference,
    )?;
    let mut expected = json!({"command_ref":reference,"argv":identity["argv"],"revision":revision,
        "revision_digest":binding["digest"],"index":0,"profile":check["profile"]});
    if check
        .get("cwd")
        .is_some_and(|v| v.as_str().is_some_and(|s| !s.is_empty()))
    {
        expected["cwd"] = check["cwd"].clone();
    }
    if starts.len() != 1 || starts[0].0 != check["started_event_id"] || starts[0].1 != expected {
        return Ok(false);
    }
    let requests = events(
        db,
        task,
        "WORKER_CHECK_REQUESTED",
        "request_key",
        &payload["request_key"],
    )?;
    let finishes = events(
        db,
        task,
        "CHECK_EXECUTION_FINISHED",
        "command_ref",
        reference,
    )?;
    if requests.len() != 1
        || requests[0].1["binding"] != *binding
        || !(requests[0].0 < starts[0].0 && starts[0].0 < row_id)
        || finishes.len() != 1
        || !(starts[0].0 < finishes[0].0 && finishes[0].0 < row_id)
    {
        return Ok(false);
    }
    let Some(fields) = check.as_object() else {
        return Ok(false);
    };
    if fields
        .iter()
        .any(|(k, v)| k != "output_tail" && finishes[0].1.get(k).unwrap_or(&Value::Null) != v)
    {
        return Ok(false);
    }
    if !output_valid(db, store, task, check, false)? {
        return Ok(false);
    }
    publication_matches(
        db,
        task,
        &target["assignment"],
        &result["report_ref"],
        &binding["digest"],
    )
}

fn publication_matches(
    db: &Connection,
    task: &str,
    assignment: &Value,
    report: &Value,
    digest: &Value,
) -> CoreResult<bool> {
    let provision: Option<String> = db
        .query_row(
            "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
            (task, assignment.as_str()),
            |r| r.get(0),
        )
        .optional()?;
    let Some(provision) = provision else {
        return Ok(false);
    };
    let Some(report) = report.as_str() else {
        return Ok(false);
    };
    let mut q = db.prepare(
        "SELECT kind,dirty_digest FROM attempt_revisions WHERE attempt_id=? AND phase=? LIMIT 3",
    )?;
    let rows = q
        .query_map((provision, format!("worker_publish:{report}")), |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, Option<String>>(1)?))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows.len() == 1 && rows[0].0 == "observed" && rows[0].1.as_deref() == digest.as_str())
}

fn output_valid(
    db: &Connection,
    store: &Store,
    task: &str,
    check: &Value,
    strict: bool,
) -> CoreResult<bool> {
    let reference = &check["command_ref"];
    let mut q = db.prepare("SELECT id,owner_scope,source_kind,exit_code,failure,signal,truncated,filename,byte_count FROM large_outputs WHERE task_id=? AND source_ref=? LIMIT 3")?;
    let outputs = q
        .query_map((task, reference.as_str()), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<i64>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
                r.get::<_, i64>(6)?,
                r.get::<_, String>(7)?,
                r.get::<_, i64>(8)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if outputs.len() != 1 {
        return Ok(false);
    }
    let (id, scope, kind, exit, failure, signal, truncated, filename, size) = &outputs[0];
    if check["large_output_id"] != *id
        || scope != "coordinator"
        || kind != "terminal"
        || *exit != Some(0)
        || failure.as_ref().is_some_and(|s| !s.is_empty())
        || signal.as_ref().is_some_and(|s| !s.is_empty())
        || *truncated != 0
        || strict && (failure.is_some() || signal.is_some())
    {
        return Ok(false);
    }
    output_complete(db, store, task, id, filename, *size)
}

// Final verification closes only the exact recorded disclosure on the reviewed
// current source. It does not accept work or write derived closed_by fields.
pub(super) fn final_closures(
    db: &Connection,
    store: &Store,
    task: &str,
    revision: i64,
) -> CoreResult<std::collections::BTreeMap<(Option<String>, usize), i64>> {
    let mut closed = std::collections::BTreeMap::new();
    let latest: Option<(i64, String, String, String)> = db.query_row(
        "SELECT id,type,actor,payload_json FROM runtime_events WHERE task_id=? AND type IN ('VERIFICATION_PASSED','VERIFICATION_FAILED') ORDER BY id DESC LIMIT 1",
        [task], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))).optional()?;
    let Some((end, kind, actor, raw)) = latest else {
        return Ok(closed);
    };
    if kind != "VERIFICATION_PASSED" || actor != "runtime" {
        return Ok(closed);
    }
    if raw.len() > 8 * 1024 * 1024 {
        return Ok(closed);
    }
    let payload: Value = serde_json::from_str(&raw)?;
    let digest = &payload["revision_digest"];
    if payload["revision"] != revision || !super::task_facts_work::valid_digest(digest) {
        return Ok(closed);
    }
    let plan: Option<String> = db.query_row(
        "SELECT plan_json FROM runtime_tasks WHERE task_id=?",
        [task],
        |r| r.get(0),
    )?;
    let raw = plan.as_deref().unwrap_or("{}");
    if raw.len() > 8 * 1024 * 1024 {
        return Ok(closed);
    }
    let plan: Value = serde_json::from_str(raw)?;
    let Some(commands) = plan["verification"].as_array() else {
        return Ok(closed);
    };
    let Some(checks) = payload["checks"].as_array() else {
        return Ok(closed);
    };
    if commands.is_empty() || commands.len() != checks.len() {
        return Ok(closed);
    }
    let review: Option<Option<String>> = db.query_row(
        "SELECT result_json FROM runtime_nodes WHERE task_id=? AND revision=? AND stage='review' ORDER BY rowid DESC LIMIT 1",
        (task, revision), |r| r.get(0)).optional()?;
    let reviewed = if let Some(raw) = review {
        let raw = raw.as_deref().unwrap_or("{}");
        if raw.len() > 8 * 1024 * 1024 {
            return Ok(closed);
        }
        serde_json::from_str::<Value>(raw)?["project_digest"].clone()
    } else {
        let raw: Option<String> = db.query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='REVIEW_NOT_REQUIRED' AND json_extract(payload_json,'$.revision')=? ORDER BY id DESC LIMIT 1",
            (task, revision), |r| r.get(0)).optional()?;
        match raw {
            Some(raw) if raw.len() <= 8 * 1024 * 1024 => {
                serde_json::from_str::<Value>(&raw)?["revision_digest"].clone()
            }
            Some(_) => return Ok(closed),
            None => Value::Null,
        }
    };
    if reviewed != *digest || source(db, task)?.as_deref() != digest.as_str() {
        return Ok(closed);
    }
    let mut refs = std::collections::BTreeSet::new();
    let mut runs = Vec::new();
    for (index, (check, argv)) in checks.iter().zip(commands).enumerate() {
        let Some(reference) = check["command_ref"].as_str() else {
            return Ok(closed);
        };
        let prefix = format!("verification:{revision}:");
        let suffix = format!(":{index}");
        let serial = reference
            .strip_prefix(&prefix)
            .and_then(|s| s.strip_suffix(&suffix));
        // Python str.isdigit accepts non-ASCII digits that never match a legacy
        // verification serial; closing nothing is the fail-closed outcome.
        if serial.is_some_and(|s| !s.is_ascii()) {
            return Ok(closed);
        }
        if !serial.is_some_and(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_digit()))
            || !refs.insert(reference)
            || check["argv"] != *argv
            || check["complete"] != true
            || check["exit_code"].as_i64() != Some(0)
            || check["revision_digest"] != *digest
            || !check["failure"].is_null()
            || super::task_facts_attempts::truthy(&check["truncated"])
        {
            return Ok(closed);
        }
        let Some(start_id) = check["started_event_id"].as_i64().filter(|id| *id < end) else {
            return Ok(closed);
        };
        // Unlike literal reruns, final receipts count non-runtime duplicates too.
        let mut q = db.prepare("SELECT id,actor,payload_json FROM runtime_events WHERE task_id=? AND type='CHECK_EXECUTION_STARTED' AND json_extract(payload_json,'$.command_ref')=? LIMIT 3")?;
        let starts = q
            .query_map((task, reference), |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if starts.len() != 1 || starts[0].0 != start_id || starts[0].1 != "runtime" {
            return Ok(closed);
        }
        if starts[0].2.len() > 8 * 1024 * 1024 {
            return Ok(closed);
        }
        let mut binding = json!({"command_ref":reference,"argv":argv,"revision":revision,"revision_digest":digest,"index":index});
        if let Some(profile) = check.get("profile") {
            binding["profile"] = profile.clone();
        }
        if serde_json::from_str::<Value>(&starts[0].2)? != binding
            || !output_valid(db, store, task, check, true)?
        {
            return Ok(closed);
        }
        runs.push((argv, start_id));
    }
    let mut q = db.prepare("SELECT assignment_ref,stage,result_json FROM runtime_nodes WHERE task_id=? AND revision=? AND result_json IS NOT NULL ORDER BY rowid LIMIT 1001")?;
    let nodes = q
        .query_map((task, revision), |r| {
            Ok((
                r.get::<_, Option<String>>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    if nodes.len() > 1000 || nodes.iter().map(|r| r.2.len()).sum::<usize>() > 8 * 1024 * 1024 {
        return Ok(closed);
    }
    for (assignment, stage, raw) in nodes {
        let result: Value = serde_json::from_str(&raw)?;
        let assignment_value = assignment.as_ref().map_or(Value::Null, |s| json!(s));
        let completed = events(
            db,
            task,
            "WORKER_COMPLETED",
            "assignment_ref",
            &assignment_value,
        )?;
        if completed.len() != 1 {
            continue;
        }
        let completion = &completed[0].1;
        let recorded = &completion["result"];
        let author_matches = if stage == "review" {
            recorded["project_digest"] == *digest
        } else {
            let attempt: Option<String> = db.query_row(
                "SELECT attempt_id FROM delegation_provisioning WHERE task_id=? AND idempotency_key=?",
                (task, assignment.as_deref()), |r| r.get(0)).optional()?;
            attempt
                .as_deref()
                .is_some_and(|s| completion["attempt_id"] == s)
                && publication_matches(
                    db,
                    task,
                    &assignment_value,
                    &recorded["report_ref"],
                    digest,
                )?
        };
        if !author_matches {
            continue;
        }
        let Some(checks) = result["checks"].as_array() else {
            continue;
        };
        for (index, check) in checks.iter().enumerate() {
            if !matches!(check["outcome"].as_str(), Some("not_run" | "unverified")) {
                continue;
            }
            let Some(mut original) = check.as_object().cloned() else {
                continue;
            };
            original.remove("closed_by");
            if recorded["checks"].get(index) != Some(&Value::Object(original)) {
                continue;
            }
            let Some(argv) = check["command"]
                .as_array()
                .filter(|a| !a.is_empty() && a.iter().all(Value::is_string))
            else {
                continue;
            };
            if let Some((_, start)) = runs
                .iter()
                .rev()
                .find(|(command, _)| command.as_array() == Some(argv))
            {
                closed.insert((assignment.clone(), index), *start);
            }
        }
    }
    Ok(closed)
}

fn output_complete(
    db: &Connection,
    store: &Store,
    task: &str,
    id: &str,
    filename: &str,
    size: i64,
) -> CoreResult<bool> {
    use std::{
        ffi::CString,
        fs::File,
        io::Read,
        os::{
            fd::{AsRawFd, FromRawFd},
            unix::fs::MetadataExt,
        },
        path::Component,
    };
    let safe = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    };
    if !safe(task) || !safe(id) || filename != format!("{id}.bin") || size < 0 {
        return Ok(false);
    }
    // The body is streamed, never buffered; only an absurd size is unverifiable.
    if size > 256 * 1024 * 1024 {
        return Ok(false);
    }
    let root: String = db.query_row("SELECT project_root FROM tasks WHERE id=?", [task], |r| {
        r.get(0)
    })?;
    let root = Path::new(&root);
    if !root.is_absolute() || root.components().any(|c| matches!(c, Component::ParentDir)) {
        return Ok(false);
    }
    let directory = store.key().directory();
    let Some(channel) = directory.file_name().and_then(|s| s.to_str()) else {
        return Ok(false);
    };
    let channel = channel.to_owned();
    let read = || -> std::io::Result<bool> {
        let mut folder = File::open("/")?;
        let mut parts = root
            .components()
            .filter_map(|c| {
                if let Component::Normal(s) = c {
                    Some((s.to_os_string(), false))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        parts.extend([channel.as_str(), task, "large_outputs"].map(|s| (s.into(), true)));
        for (part, private) in parts {
            let name = CString::new(part.as_encoded_bytes())?;
            let fd = unsafe {
                libc::openat(
                    folder.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            if fd < 0 {
                return Ok(false);
            }
            folder = unsafe { File::from_raw_fd(fd) };
            let info = folder.metadata()?;
            if private && (info.uid() != unsafe { libc::getuid() } || info.mode() & 0o077 != 0) {
                return Ok(false);
            }
        }
        let name = CString::new(filename)?;
        let open = || unsafe {
            libc::openat(
                folder.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        let fd = open();
        if fd < 0 {
            return Ok(false);
        }
        let mut body = unsafe { File::from_raw_fd(fd) };
        let before = body.metadata()?;
        if !before.is_file()
            || before.uid() != unsafe { libc::getuid() }
            || before.nlink() != 1
            || before.mode() & 0o077 != 0
            || before.len() != size as u64
        {
            return Ok(false);
        }
        let mut remaining = size as u64;
        let mut buffer = [0; 65536];
        while remaining > 0 {
            let length = remaining.min(buffer.len() as u64) as usize;
            let n = body.read(&mut buffer[..length])?;
            if n == 0 {
                return Ok(false);
            }
            remaining -= n as u64;
        }
        if body.read(&mut buffer[..1])? != 0 {
            return Ok(false);
        }
        let signature = |m: &std::fs::Metadata| {
            (
                m.dev(),
                m.ino(),
                m.len(),
                m.mode(),
                m.uid(),
                m.nlink(),
                m.mtime(),
                m.mtime_nsec(),
                m.ctime(),
                m.ctime_nsec(),
            )
        };
        let fd = open();
        if fd < 0 {
            return Ok(false);
        }
        let current = unsafe { File::from_raw_fd(fd) };
        Ok(signature(&before) == signature(&body.metadata()?)
            && signature(&before) == signature(&current.metadata()?))
    };
    Ok(read().unwrap_or(false))
}
