//! `worker-history`: the worker tab/card reader, ported from the `PYTHON`
//! script of `integrations/paseo/server/worker-history.ts` with the same
//! argv, queries, precedence rules and output bytes.
use crate::common::{
    dict, printed, process_identity_absent, route_selection, sum_tokens, usage_fallback_refs,
};
use crate::py::{self, Crash, Py};
use crate::scrub::scrub;
use crate::store::{self, placeholders, query, query_one, Fail, Row, Sv};
use regex::Regex;
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// Worker-tab bound for one attempt's own summary text.
const SUMMARY_MAX_CHARS: usize = 16000;
/// Worker-tab bound for the assignment brief sent to one attempt.
const PROMPT_MAX_CHARS: usize = 32000;

const INTERRUPTED_EN: &str = "Interrupted when the host connection closed; no result yet. \
The next message reconciles this worker from saved state.";
const INTERRUPTED_RU: &str = "Прервано закрытием соединения с хостом; результата ещё нет. \
Следующее сообщение восстановит этого исполнителя из сохранённого состояния.";

const EVENT_FIELDS: [&str; 17] = [
    "activity_ref",
    "input",
    "output",
    "status",
    "error",
    "diff",
    "tool_kind",
    "title",
    "locations",
    "content",
    "raw_input",
    "raw_output",
    "message_id",
    "payload_offloaded",
    "large_output_id",
    "sha256",
    "byte_count",
];
const WORKER_EVENT_TYPES: [&str; 5] = [
    "worker_result",
    "worker_question",
    "assignment_failed",
    "assignment_cancelled",
    "worker_activity",
];

struct Args {
    root: String,
    task_id: String,
    attempt_id: Option<String>,
    cursor: i64,
    limit: i64,
    assignment_cursor: String,
    assignment_ref: Option<String>,
    compact: bool,
}

fn non_empty(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
}

fn parse(argv: &[String]) -> Option<Args> {
    let arg = |i: usize| argv.get(i).map(String::as_str);
    let int_or = |text: &str, default: i64| -> Option<i64> {
        if text.is_empty() {
            Some(default)
        } else {
            py::parse_int(text)
        }
    };
    Some(Args {
        root: arg(0)?.to_owned(),
        task_id: arg(1)?.to_owned(),
        attempt_id: non_empty(arg(2)?),
        cursor: int_or(arg(3)?, 0)?,
        limit: int_or(arg(4)?, 200)?.clamp(1, 200),
        assignment_cursor: arg(5)?.to_owned(),
        assignment_ref: non_empty(arg(6)?),
        compact: arg(7) == Some("1"),
    })
}

/// Run the reader for its argv (after the subcommand): `(stdout, exit code)`.
pub fn run(argv: &[String]) -> (String, i32) {
    let Some(args) = parse(argv) else {
        return (String::new(), 1);
    };
    match store::read(&args.root, |db| read(db, &args)) {
        Ok(Output::Printed(text)) => (text, 0),
        Err(Fail::Sql) => (
            "{\"assignments\": [], \"available\": false, \"truncated\": false, \"nextCursor\": null, \
\"message\": \"Worker history could not be read.\"}\n"
                .into(),
            0,
        ),
        Err(Fail::Crash) => (String::new(), 1),
    }
}

enum Output {
    Printed(String),
}

fn has(tables: &[String], name: &str) -> bool {
    tables.iter().any(|t| t == name)
}

/// Python `set` of the string refs a host disconnect interrupted.
struct RefSet(Vec<String>);

impl RefSet {
    /// `set.add(ref)`: a list/dict is unhashable; other non-string refs can
    /// never equal a string ref, so they need not be kept.
    fn add(&mut self, value: &Py) -> Result<(), Crash> {
        match value {
            Py::List(_) | Py::Dict(_) => Err(Crash),
            Py::Str(s) => {
                if !self.0.contains(s) {
                    self.0.push(s.clone());
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
    fn contains(&self, value: &Sv) -> bool {
        value
            .as_text()
            .is_some_and(|s| self.0.iter().any(|r| r == s))
    }
}

fn task_language(db: &Connection, tables: &[String], task_id: &Sv) -> Result<&'static str, Fail> {
    if has(tables, "task_language") {
        if let Some(row) = query_one(
            db,
            "select language from task_language where task_id=?",
            &[task_id.clone()],
        )? {
            match row.get("language")?.as_text() {
                Some("en") => return Ok("en"),
                Some("ru") => return Ok("ru"),
                _ => {}
            }
        }
    }
    for row in query(
        db,
        "select payload_json from task_events where task_id=? and event_type='user_message' order by id desc limit 20",
        &[task_id.clone()],
    )? {
        let Ok(payload) = row.get("payload_json")?.loads() else {
            continue;
        };
        if !payload.is_dict() {
            continue; // AttributeError, caught
        }
        if let Some(text) = payload.get("content").as_str().filter(|s| !s.is_empty()) {
            let letters: Vec<char> = text.chars().filter(|c| py::is_alpha(*c)).collect();
            let cyrillic = letters.iter().filter(|c| ('\u{400}'..='\u{4ff}').contains(*c)).count();
            let ru = !letters.is_empty() && (cyrillic as f64) / (letters.len() as f64) >= 0.30;
            return Ok(if ru { "ru" } else { "en" });
        }
    }
    Ok("en")
}

/// `project_checks(checks, legacy)`: one result's checks for display, or
/// `None` when absent or malformed.
fn project_checks(checks: &Py, legacy: bool) -> Option<Py> {
    let Py::List(items) = checks else {
        return None;
    };
    if !items.iter().all(|c| {
        c.is_dict() && c.get("name").as_str().is_some() && c.get("outcome").as_str().is_some()
    }) {
        return None;
    }
    let text = |value: &str| {
        if legacy {
            scrub(value)
        } else {
            value.to_owned()
        }
    };
    let nonblank = |c: &Py, key: &str| {
        c.get(key)
            .as_str()
            .filter(|s| !py::strip(s).is_empty())
            .map(str::to_owned)
    };
    Some(Py::List(
        items
            .iter()
            .map(|c| {
                let mut check = dict(vec![
                    (
                        "name",
                        Py::str(text(c.get("name").as_str().unwrap_or_default())),
                    ),
                    ("outcome", c.get("outcome").clone()),
                ]);
                if let Some(blocker) = nonblank(c, "blocker") {
                    check.set("blocker", Py::str(text(py::prefix(&blocker, 600))));
                }
                for (key, field) in [
                    ("closedBy", "closed_by"),
                    ("claimedOutcome", "claimed_outcome"),
                    ("evidenceGap", "evidence_gap"),
                ] {
                    if let Some(value) = nonblank(c, field) {
                        check.set(key, Py::str(value));
                    }
                }
                check
            })
            .collect(),
    ))
}

/// `assignment_parts(request)`: (title block, brief, acceptance).
fn assignment_parts(request: &Py) -> Option<(String, String, String)> {
    let brief = request.get("brief").as_str().filter(|s| !s.is_empty())?;
    let acceptance = request.get("acceptance").as_str()?;
    let title_block = match request.get("title").as_str() {
        Some(title) if !title.is_empty() && !brief.contains(title) => format!("{title}\n\n"),
        _ => String::new(),
    };
    Some((title_block, brief.to_owned(), acceptance.to_owned()))
}

fn scrub_leaves(value: &Py) -> Py {
    match value {
        Py::Str(s) => Py::str(scrub(s)),
        Py::List(items) => Py::List(items.iter().map(scrub_leaves).collect()),
        Py::Dict(entries) => Py::Dict(
            entries
                .iter()
                .map(|(k, v)| (k.clone(), scrub_leaves(v)))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// `scrub_brief(brief)`: scrub a JSON-object brief's string values, keeping its structure.
fn scrub_brief(brief: &str) -> Result<String, Crash> {
    match py::loads(brief) {
        Some(parsed) if parsed.is_dict() => py::dumps(&scrub_leaves(&parsed)),
        _ => Ok(scrub(brief)),
    }
}

/// `attempt_prompt(...)`: the assignment text sent to one work attempt.
fn attempt_prompt(
    db: &Connection,
    task_id: &Sv,
    attempt: &Row,
    requests: &[Py],
    refs: &[Sv],
    active: bool,
) -> Result<Py, Fail> {
    let attempt_id = attempt.get("attempt_id")?.to_py()?;
    let first = attempt.get("attempt_no")?.eq_int(1);
    let mut candidates = Vec::new();
    for payload in requests {
        let bound = payload.get("work_attempt_ref");
        if py::eq(bound, &attempt_id) || (first && !bound.truthy()) {
            if let Some(parts) = assignment_parts(payload) {
                candidates.push(parts);
            }
        }
    }
    if candidates.is_empty() {
        return Ok(dict(vec![("state", Py::str("unavailable"))]));
    }
    let mut params = vec![task_id.clone()];
    params.extend(refs.iter().cloned());
    let mut markers: Vec<(String, Sv)> = Vec::new();
    for row in query(
        db,
        &format!(
            "select payload_json,created_at from task_events where task_id=? and event_type='progress' \
             and assignment_ref in ({}) and json_valid(payload_json) and \
             json_extract(payload_json,'$.kind')='worker_brief_sent' order by id",
            placeholders(refs.len())
        ),
        &params,
    )? {
        let Ok(marker) = row.get("payload_json")?.loads() else {
            continue;
        };
        let Some(sha) = marker.is_dict().then(|| marker.get("assignment_sha256").as_str()).flatten() else {
            continue;
        };
        let bound = marker.get("work_attempt_ref");
        if !(bound.is_null() || py::eq(bound, &attempt_id)) {
            continue;
        }
        markers.push((sha.to_owned(), row.get("created_at")?.clone()));
    }
    let digest = |parts: &(String, String, String)| {
        let text = format!("{}{}\n\nAcceptance: {}", parts.0, parts.1, parts.2);
        format!("{:x}", Sha256::digest(text.as_bytes()))
    };
    let mut chosen = None;
    let mut sent_at = Sv::Null;
    for (sha, created_at) in &markers {
        chosen = candidates.iter().find(|parts| digest(parts) == *sha);
        if chosen.is_some() {
            sent_at = created_at.clone();
            break;
        }
    }
    if !markers.is_empty() && chosen.is_none() {
        return Ok(dict(vec![("state", Py::str("mismatch"))]));
    }
    let (title_block, brief, acceptance) =
        chosen.unwrap_or(&candidates[candidates.len() - 1]).clone();
    let brief = scrub_brief(&brief)?;
    let state = if sent_at.truthy() {
        "sent"
    } else if active {
        "queued"
    } else {
        "unrecorded"
    };
    let mut out = dict(vec![
        ("state", Py::str(state)),
        ("brief", Py::str(brief.clone())),
        ("acceptance", Py::str(scrub(&acceptance))),
    ]);
    if sent_at.truthy() {
        out.set("sentAt", sent_at.to_py()?);
    }
    if !title_block.is_empty() {
        out.set("title", Py::str(scrub(py::strip(&title_block))));
    }
    if py::len(&brief) > PROMPT_MAX_CHARS {
        out.set(
            "brief",
            Py::str(format!("{}…", py::prefix(&brief, PROMPT_MAX_CHARS - 1))),
        );
        out.set("truncated", Py::Bool(true));
    }
    Ok(out)
}

fn mcp_name() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)^mcp(?:__|\.)([^.]+?)(?:__|\.)([\w-]+)").expect("mcp pattern")
    })
}

fn read_verb() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)^(read|view|cat)(?:[\s\x1c-\x1f]|$)").expect("read pattern"))
}

/// One transcript event projected for the worker tab, or `None` when it is
/// not worker chat.
fn project_event(ev: &Row) -> Result<Option<Py>, Fail> {
    let payload = ev
        .get("payload_json")?
        .loads()
        .unwrap_or_else(|_| Py::dict());
    let event_type = ev.get("event_type")?;
    let event_kind = if payload.is_dict() {
        payload.get("kind")
    } else {
        &py::NONE
    };
    if !WORKER_EVENT_TYPES.iter().any(|t| event_type.eq_str(t))
        && !py::eq_str(event_kind, "worker_activity")
    {
        return Ok(None);
    }
    if !payload.is_dict() {
        return Err(Fail::Crash); // payload.get on a non-dict
    }
    let mut message = &py::NONE;
    for key in ["message", "summary", "text", "response", "reason", "detail"] {
        message = payload.get(key);
        if message.truthy() {
            break;
        }
    }
    let mut parts = Vec::new();
    for part in [
        payload.get("title"),
        payload.get("status"),
        message,
        payload.get("error"),
    ] {
        if part.is_null() || py::eq_str(part, "") {
            continue;
        }
        parts.push(match part.as_str() {
            Some(s) => s.to_owned(),
            None => py::dumps(part)?,
        });
    }
    let text = if parts.is_empty() {
        event_type.to_str()
    } else {
        parts.join(" · ")
    };
    let mut event = dict(vec![
        ("id", ev.get("id")?.to_py()?),
        ("type", event_type.to_py()?),
        ("createdAt", ev.get("created_at")?.to_py()?),
        ("text", Py::str(scrub(&text))),
    ]);
    for field in EVENT_FIELDS {
        let value = payload.get(field);
        if value.is_null() {
            continue;
        }
        event.set(
            field,
            match value.as_str() {
                Some(s) => Py::str(scrub(s)),
                None => value.clone(),
            },
        );
    }
    let title = event.get("title");
    let read_title = if title.truthy() {
        py::to_str(title)
    } else {
        String::new()
    };
    let empty = Py::dict();
    let read_args = if event.get("raw_input").is_dict() {
        event.get("raw_input")
    } else {
        &empty
    };
    let read_name = match read_args.get("tool").as_str() {
        Some(tool) if py::eq_str(read_args.get("server"), "cortex") => format!("mcp.cortex.{tool}"),
        _ => read_title.clone(),
    };
    let found = mcp_name().captures(&read_name);
    let named = |n: &str| n == "project_read" || n == "read_skill";
    let cortex_read = found
        .as_ref()
        .is_some_and(|m| m[1].to_lowercase() == "cortex" && named(&m[2].to_lowercase()))
        || named(&read_name.to_lowercase());
    let read_command = read_args
        .get_opt("command")
        .unwrap_or_else(|| read_args.get("cmd"));
    let tool_kind = event.get("tool_kind");
    let shell_row = !py::eq_str(tool_kind, "read")
        && (py::eq_str(tool_kind, "execute")
            || py::eq_str(tool_kind, "terminal")
            || read_command.as_str().is_some_and(|command| {
                !py::strip(command).is_empty()
                    && py::normalize_spaces(&read_title) == py::normalize_spaces(command)
            }));
    let read_only_row = py::eq_str(tool_kind, "read")
        || cortex_read
        || (found.is_none() && !shell_row && read_verb().is_match(&read_title));
    if read_only_row {
        for key in ["output", "raw_output", "content", "error", "diff"] {
            event.remove(key);
        }
        event.set("text", Py::str(read_title));
    }
    Ok(Some(event))
}

fn status_is(value: &Py, words: &[&str]) -> bool {
    let text = py::to_str(value).to_lowercase();
    words.contains(&text.as_str())
}

const NON_TERMINAL: [&str; 6] = [
    "planned",
    "running",
    "in_progress",
    "working",
    "ready",
    "provisioning",
];
const ACTIVE: [&str; 9] = [
    "planned",
    "pending",
    "queued",
    "running",
    "in_progress",
    "working",
    "ready",
    "provisioning",
    "starting",
];

fn with_refs(first: &Sv, refs: &[Sv]) -> Vec<Sv> {
    let mut params = vec![first.clone()];
    params.extend(refs.iter().cloned());
    params
}

fn text_params(first: &Sv, second: Option<&str>, refs: &[String]) -> Vec<Sv> {
    let mut params = vec![first.clone()];
    if let Some(second) = second {
        params.push(Sv::text(second));
    }
    params.extend(refs.iter().map(|r| Sv::text(r.clone())));
    params
}

/// `[... for check in result.get('checks', []) if check.get('outcome') != 'passed']`
/// of a failed runtime node, with Python's exceptions on malformed checks.
fn unresolved_checks(result: &Py) -> Result<Vec<String>, Crash> {
    let checks = match result.get_opt("checks") {
        None => return Ok(Vec::new()),
        Some(value) => value.iterate()?,
    };
    let mut out = Vec::new();
    for check in checks {
        if py::eq_str(check.get_or_crash("outcome")?, "passed") {
            continue;
        }
        let name = match check.get_opt("name") {
            None => "check".to_owned(),
            Some(value) => value.as_str().ok_or(Crash)?.to_owned(),
        };
        let outcome = match check.get_opt("outcome") {
            None => "unverified".to_owned(),
            Some(value) => value.as_str().ok_or(Crash)?.to_owned(),
        };
        out.push(format!("{name}: {outcome}"));
    }
    Ok(out)
}

fn read(db: &Connection, args: &Args) -> Result<Output, Fail> {
    let tables = store::tables(db)?;
    if ![
        "tasks",
        "work_assignments",
        "work_assignment_attempts",
        "task_events",
    ]
    .iter()
    .all(|t| has(&tables, t))
    {
        return Ok(Output::Printed(
            "{\"assignments\": [], \"available\": false, \"truncated\": false, \"nextCursor\": null, \
\"message\": \"Worker history is unavailable in this workspace store.\"}\n"
                .into(),
        ));
    }
    let task_rows = query(
        db,
        "select id from tasks where id=? and project_root=? order by id",
        &[Sv::text(args.task_id.clone()), Sv::text(args.root.clone())],
    )?;
    let has_route_audit = has(&tables, "attempt_route_audit");
    let has_provisioning = has(&tables, "delegation_provisioning");
    let has_process_registry = has(&tables, "attempt_process_registry");
    let attempt_filter = args.attempt_id.as_deref();
    let mut assignments: Vec<Py> = Vec::new();
    let mut event_ids: Vec<i64> = Vec::new();
    let mut truncated = false;
    let mut more_assignments = false;
    for task in &task_rows {
        let task_id = task.get("id")?.clone();
        let mut disconnected = RefSet(Vec::new());
        for row in query(
            db,
            "select event_type,assignment_ref,payload_json from task_events where task_id=? and \
             ((event_type='progress' and payload_json like '%host_disconnected%') or \
             (event_type='assignment_failed' and payload_json like '%recovery_worker_lost%'))",
            &[task_id.clone()],
        )? {
            let Ok(payload) = row.get("payload_json")?.loads() else {
                continue;
            };
            if !payload.is_dict() {
                continue;
            }
            if row.get("event_type")?.eq_str("assignment_failed") {
                let named = payload.get("attempt_id");
                let reference = if named.truthy() {
                    named.clone()
                } else {
                    row.get("assignment_ref")?.to_py()?
                };
                if py::eq_str(payload.get("error"), "recovery_worker_lost") && reference.truthy() {
                    disconnected.add(&reference)?;
                }
                continue;
            }
            if !py::eq_str(payload.get("kind"), "host_disconnected") {
                continue;
            }
            let workers = payload.get("interrupted_workers");
            if workers.truthy() {
                for worker in workers.iterate()? {
                    let reference = if worker.is_dict() {
                        worker.get("attempt_id").clone()
                    } else {
                        Py::Null
                    };
                    if reference.truthy() {
                        disconnected.add(&reference)?;
                    }
                }
            }
        }
        let interrupted_reason = match task_language(db, &tables, &task_id)? {
            "ru" => INTERRUPTED_RU,
            _ => INTERRUPTED_EN,
        };
        let rows = if let Some(assignment_ref) = args.assignment_ref.as_deref() {
            let rows = query(
                db,
                "select a.* from work_assignments a where a.task_id=? and
                (a.assignment_id=? or a.idempotency_key=? or exists (
                    select 1 from task_events e where e.task_id=a.task_id
                    and e.event_type='delegation_requested' and e.assignment_ref=?
                    and (case when json_valid(e.payload_json) then
                        json_extract(e.payload_json, '$.plan_assignment_ref')=a.assignment_id
                        or json_extract(e.payload_json, '$.work_attempt_ref') in
                            (select attempt_id from work_assignment_attempts where assignment_id=a.assignment_id)
                        else 0 end)
                )) order by a.assignment_id limit 2",
                &[
                    task_id.clone(),
                    Sv::text(assignment_ref),
                    Sv::text(assignment_ref),
                    Sv::text(assignment_ref),
                ],
            )?;
            if rows.len() > 1 {
                return Ok(Output::Printed(
                    "{\"assignments\": [], \"available\": false, \"truncated\": false, \"nextCursor\": null, \
\"nextAssignmentCursor\": null, \"message\": \"Worker assignment identity is ambiguous.\"}\n"
                        .into(),
                ));
            }
            rows
        } else {
            query(
                db,
                "select * from work_assignments where task_id=? and assignment_id>? order by assignment_id limit 101",
                &[task_id.clone(), Sv::text(args.assignment_cursor.clone())],
            )?
        };
        let mut rows = rows;
        let page_more = rows.len() > 100;
        rows.truncate(100);
        more_assignments |= page_more;
        truncated |= page_more;
        for a in &rows {
            let assignment_id = a.get("assignment_id")?.clone();
            let idempotency_key = a.get("idempotency_key")?.clone();
            let mut title: Option<String> = None;
            let mut role: Option<String> = None;
            let title_rows = query(
                db,
                "select assignment_ref,payload_json from task_events where task_id=?
                and event_type='delegation_requested' and (assignment_ref in (?,?) or
                    (case when json_valid(payload_json) then
                        json_extract(payload_json, '$.plan_assignment_ref')=? or
                        json_extract(payload_json, '$.work_attempt_ref') in
                            (select attempt_id from work_assignment_attempts where assignment_id=?)
                        else 0 end)) order by id",
                &[
                    task_id.clone(),
                    assignment_id.clone(),
                    idempotency_key.clone(),
                    assignment_id.clone(),
                    assignment_id.clone(),
                ],
            )?;
            let mut selected_request: Option<&Row> = None;
            for r in &title_rows {
                if r.get("assignment_ref")?
                    .eq_opt(args.assignment_ref.as_deref())
                {
                    selected_request = Some(r);
                    break;
                }
            }
            let mut selected_attempt: Py = args.attempt_id.clone().map(Py::Str).unwrap_or(Py::Null);
            if let Some(request) = selected_request {
                let payload = request
                    .get("payload_json")?
                    .loads()
                    .unwrap_or_else(|_| Py::dict());
                let bound = if payload.is_dict() {
                    payload.get("work_attempt_ref").clone()
                } else {
                    Py::Null
                };
                if bound.truthy() {
                    if attempt_filter.is_some_and(|id| !py::eq_str(&bound, id)) {
                        continue;
                    }
                    selected_attempt = bound;
                }
            }
            for title_row in &title_rows {
                let (candidate_title, candidate_role) = match title_row.get("payload_json")?.loads()
                {
                    Ok(payload) if payload.is_dict() => {
                        (payload.get("title").clone(), payload.get("role").clone())
                    }
                    _ => (Py::Null, Py::Null),
                };
                if title.is_none() {
                    if let Some(t) = crate::common::nonblank(&candidate_title) {
                        title = Some(py::strip(t).to_owned());
                    }
                }
                if role.is_none() {
                    if let Some(r) = crate::common::nonblank(&candidate_role) {
                        let r = py::strip(r);
                        role = Some(r.strip_prefix("worker-").unwrap_or(r).to_owned());
                    }
                }
            }
            let attempts = if selected_attempt.truthy() {
                query(
                    db,
                    "select * from work_assignment_attempts where assignment_id=? and attempt_id=? order by attempt_no, attempt_id",
                    &[assignment_id.clone(), Sv::from_py(&selected_attempt)?],
                )?
            } else {
                query(
                    db,
                    "select * from work_assignment_attempts where assignment_id=? order by attempt_no, attempt_id",
                    &[assignment_id.clone()],
                )?
            };
            let mut attempt_out: Vec<Py> = Vec::new();
            let mut latest_route: Option<Py> = None;
            for at in &attempts {
                let at_id = at.get("attempt_id")?.clone();
                if attempt_filter.is_some_and(|id| !at_id.eq_str(id)) {
                    continue;
                }
                let at_id_py = at_id.to_py()?;
                let mut refs: Vec<Sv> = vec![at_id.clone()];
                let mut execution_refs: Vec<String> = Vec::new();
                if has_provisioning {
                    for request_row in &title_rows {
                        let payload = request_row
                            .get("payload_json")?
                            .loads()
                            .unwrap_or_else(|_| Py::dict());
                        if !py::eq(payload.get_or_crash("work_attempt_ref")?, &at_id_py) {
                            continue;
                        }
                        let request_ref = request_row.get("assignment_ref")?.clone();
                        refs.push(request_ref.clone());
                        if let Some(provisioning) = query_one(
                            db,
                            "select attempt_id from delegation_provisioning where task_id=? and idempotency_key=?",
                            &[task_id.clone(), request_ref],
                        )? {
                            if let Some(id) = provisioning.get("attempt_id")?.as_text().filter(|s| !s.is_empty()) {
                                execution_refs.push(id.to_owned());
                            }
                        }
                    }
                    refs.extend(execution_refs.iter().map(|r| Sv::text(r.clone())));
                }
                if at.get("attempt_no")?.eq_int(1) {
                    refs.push(assignment_id.clone());
                    refs.push(idempotency_key.clone());
                }
                let marks = placeholders(refs.len());
                let mut params = with_refs(&task_id, &refs);
                params.push(Sv::Int(args.cursor));
                params.push(Sv::Int(args.limit + 1));
                let mut evs = query(
                    db,
                    &format!(
                        "select id,event_type,created_at,payload_json from task_events where task_id=? and \
                         assignment_ref in ({marks}) and id>? and (event_type in ('worker_result','worker_question',\
                         'assignment_failed','assignment_cancelled','worker_activity') or (json_valid(payload_json) \
                         and json_extract(payload_json,'$.kind')='worker_activity')) and not (json_valid(payload_json) \
                         and coalesce(json_extract(payload_json,'$.activity_ref'),'')='context' and \
                         coalesce(json_extract(payload_json,'$.kind'),'')='worker_activity') order by id limit ?"
                    ),
                    &params,
                )?;
                let more = evs.len() as i64 > args.limit;
                if more {
                    evs.truncate(args.limit as usize);
                }
                truncated |= more;
                let mut event_out = Vec::new();
                for ev in &evs {
                    if let Some(event) = project_event(ev)? {
                        if let Sv::Int(id) = ev.get("id")? {
                            event_ids.push(*id);
                        }
                        event_out.push(event);
                    }
                }
                let terminal_rows = query(
                    db,
                    &format!(
                        "select id,event_type,created_at from task_events where task_id=? and assignment_ref in ({marks}) \
                         and event_type in ('worker_result','assignment_failed','assignment_cancelled') order by id"
                    ),
                    &with_refs(&task_id, &refs),
                )?;
                let priority = |kind: &str| if kind == "worker_result" { 2 } else { 1 };
                let mut terminal_type: Option<String> = None;
                let mut completed_at: Option<Sv> = None;
                for row in &terminal_rows {
                    let kind = row.get("event_type")?.to_str();
                    if terminal_type
                        .as_deref()
                        .is_none_or(|current| priority(&kind) >= priority(current))
                    {
                        terminal_type = Some(kind);
                        completed_at = Some(row.get("created_at")?.clone());
                    }
                }
                let mut process_absent = false;
                if has_process_registry && !execution_refs.is_empty() {
                    let process_rows = query(
                        db,
                        &format!(
                            "select * from attempt_process_registry where task_id=? and role=? and attempt_id in ({})",
                            placeholders(execution_refs.len())
                        ),
                        &text_params(&task_id, Some("worker"), &execution_refs),
                    )?;
                    let mut distinct = execution_refs.clone();
                    distinct.sort();
                    distinct.dedup();
                    process_absent =
                        process_rows.len() == distinct.len() && !process_rows.is_empty() && {
                            let mut all = true;
                            for row in &process_rows {
                                if !process_identity_absent(row)? {
                                    all = false;
                                    break;
                                }
                            }
                            all
                        };
                }
                let mut attempt_summary: Option<String> = None;
                let mut attempt_route: Option<Py> = None;
                let mut runtime_authoritative = false;
                if has_route_audit {
                    let route_sql = "select engine,model,decision_detail_json from attempt_route_audit \
                                     where task_id=? and attempt_id=? order by recorded_at desc, id desc limit 1";
                    let mut route_row = None;
                    for reference in &execution_refs {
                        if let Some(row) = query_one(
                            db,
                            route_sql,
                            &[task_id.clone(), Sv::text(reference.clone())],
                        )? {
                            route_row = Some(row);
                            break;
                        }
                    }
                    if route_row.is_none() {
                        route_row = query_one(db, route_sql, &[task_id.clone(), at_id.clone()])?;
                    }
                    if let Some(row) = route_row {
                        let engine = row.get("engine")?;
                        let model = row.get("model")?;
                        if engine.as_text().is_some_and(|s| !s.is_empty())
                            && model.as_text().is_some_and(|s| !s.is_empty())
                        {
                            let raw = row.get("decision_detail_json")?;
                            let detail = if raw.truthy() {
                                raw.loads().unwrap_or_else(|_| Py::dict())
                            } else {
                                Py::dict()
                            };
                            let field = |key: &str| {
                                let value = if detail.is_dict() {
                                    detail.get(key)
                                } else {
                                    &py::NONE
                                };
                                match value.as_str() {
                                    Some(s) if !s.is_empty() => Py::str(s),
                                    _ => Py::Null,
                                }
                            };
                            let mut route = dict(vec![
                                ("provider", engine.to_py()?),
                                ("model", model.to_py()?),
                                ("modelName", field("model_name")),
                                ("effort", field("effort")),
                            ]);
                            if let Some(selection) = route_selection(&detail)? {
                                route.set("selection", selection);
                            }
                            latest_route = Some(route.clone());
                            attempt_route = Some(route);
                        }
                    }
                }
                let failure = at.get("failure_reason")?;
                let failure = match failure.as_text() {
                    Some(s) => Py::str(scrub(s)),
                    None => failure.to_py()?,
                };
                let mut result = dict(vec![
                    ("id", at_id_py.clone()),
                    ("attemptNo", at.get("attempt_no")?.to_py()?),
                    ("status", at.get("status")?.to_py()?),
                    ("failureReason", failure),
                    ("events", Py::List(event_out)),
                ]);
                match terminal_type.as_deref() {
                    Some("worker_result") => result.set("status", Py::str("completed")),
                    Some("assignment_failed") => result.set("status", Py::str("failed")),
                    Some("assignment_cancelled") => result.set("status", Py::str("cancelled")),
                    _ => {}
                }
                let mut interrupted = false;
                let mut lost = execution_refs
                    .iter()
                    .any(|r| disconnected.contains(&Sv::text(r.clone())))
                    || refs.iter().any(|r| disconnected.contains(r));
                let negative_terminal = matches!(
                    terminal_type.as_deref(),
                    Some("assignment_failed" | "assignment_cancelled")
                );
                if lost && negative_terminal {
                    interrupted = true;
                    result.set("status", Py::str("interrupted"));
                    result.set("failureReason", Py::str(interrupted_reason));
                }
                let non_terminal = status_is(result.get("status"), &NON_TERMINAL);
                if non_terminal && !lost && has_process_registry && !execution_refs.is_empty() {
                    let owners: Vec<String> = execution_refs
                        .iter()
                        .map(|r| format!("acp_host:{r}"))
                        .collect();
                    let owner_rows = query(
                        db,
                        &format!(
                            "select * from attempt_process_registry where task_id=? and role=? and attempt_id in ({})",
                            placeholders(owners.len())
                        ),
                        &text_params(&task_id, Some("acp_host"), &owners),
                    )?;
                    lost = !owner_rows.is_empty() && {
                        let mut all = true;
                        for row in &owner_rows {
                            if !process_identity_absent(row)? {
                                all = false;
                                break;
                            }
                        }
                        all
                    };
                }
                if non_terminal && lost {
                    interrupted = true;
                    result.set("status", Py::str("interrupted"));
                    if !result.get("failureReason").truthy() {
                        result.set("failureReason", Py::str(interrupted_reason));
                    }
                } else if process_absent && non_terminal {
                    result.set("status", Py::str("failed"));
                    if !result.get("failureReason").truthy() {
                        result.set(
                            "failureReason",
                            Py::str("Worker process exited before reporting a result."),
                        );
                    }
                }
                if has(&tables, "runtime_nodes") && !execution_refs.is_empty() && !interrupted {
                    let runtime_rows = query(
                        db,
                        &format!(
                            "select n.status,n.result_json from runtime_nodes n join delegation_provisioning p on \
                             p.task_id=n.task_id and p.idempotency_key=n.assignment_ref where n.task_id=? and \
                             p.attempt_id in ({})",
                            placeholders(execution_refs.len())
                        ),
                        &text_params(&task_id, None, &execution_refs),
                    )?;
                    runtime_authoritative = !runtime_rows.is_empty();
                    if runtime_rows.len() == 1 {
                        let status = runtime_rows[0].get("status")?;
                        let raw = runtime_rows[0].get("result_json")?;
                        result.set("status", status.to_py()?);
                        let parsed = if raw.is_null() {
                            Py::dict()
                        } else if raw.truthy() {
                            raw.loads().unwrap_or_else(|_| Py::dict())
                        } else {
                            py::loads("{}").unwrap_or_else(Py::dict)
                        };
                        let parsed = if parsed.is_dict() { parsed } else { Py::dict() };
                        if let Some(checks) = project_checks(parsed.get("checks"), false) {
                            result.set("checks", checks);
                        }
                        if !args.compact {
                            if let Some(summary) = crate::common::nonblank(parsed.get("summary")) {
                                attempt_summary = Some(summary.to_owned());
                            }
                        }
                        if status.eq_str("failed") {
                            let unresolved = unresolved_checks(&parsed)?;
                            let prefix = if raw.is_null() {
                                "Worker failed. "
                            } else {
                                "Worker failed. Runtime did not accept worker checks. "
                            };
                            result.set(
                                "failureReason",
                                Py::str(scrub(&format!("{prefix}{}", unresolved.join("; ")))),
                            );
                        }
                    } else if runtime_rows.len() > 1 {
                        result.set("status", Py::str("failed"));
                        result.set(
                            "failureReason",
                            Py::str(
                                "Ambiguous runtime assignment identity; outcome is unverified.",
                            ),
                        );
                    }
                }
                if let Some(row) = query_one(
                    db,
                    &format!(
                        "select payload_json from task_events where task_id=? and assignment_ref in ({marks}) and \
                         event_type='progress' and json_valid(payload_json) and \
                         json_extract(payload_json,'$.activity_ref')='context' and \
                         json_extract(payload_json,'$.kind')='worker_activity' order by id desc limit 1"
                    ),
                    &with_refs(&task_id, &refs),
                )? {
                    let payload = row.get("payload_json")?.loads().map_err(|_| Fail::Crash)?;
                    let content = payload.get_or_crash("content")?;
                    if content.is_dict()
                        && ["used", "size"]
                            .iter()
                            .all(|k| content.get(k).is_int() && content.get(k).int_non_negative())
                        && content.get("size").int_positive()
                    {
                        result.set(
                            "context",
                            dict(vec![("used", content.get("used").clone()), ("size", content.get("size").clone())]),
                        );
                    }
                }
                if has(&tables, "usage_records") && !execution_refs.is_empty() {
                    let usage_refs = if has_route_audit {
                        usage_fallback_refs(db, &task_id, &execution_refs)?
                    } else {
                        execution_refs.clone()
                    };
                    let usage_rows = query(
                        db,
                        &format!(
                            "select basis,tokens_json,stop_reason,failed from usage_records where task_id=? and attempt_id in ({})",
                            placeholders(usage_refs.len())
                        ),
                        &text_params(&task_id, None, &usage_refs),
                    )?;
                    if let Some(tokens) = sum_tokens(&usage_rows)? {
                        result.set("tokens", tokens);
                    }
                }
                let legacy_summary_needed = !args.compact && attempt_summary.is_none();
                let legacy_checks_needed = !runtime_authoritative && !result.contains("checks");
                if terminal_type.as_deref() == Some("worker_result")
                    && (legacy_summary_needed || legacy_checks_needed)
                {
                    let row = query_one(
                        db,
                        &format!(
                            "select payload_json from task_events where task_id=? and assignment_ref in ({marks}) and \
                             event_type='worker_result' order by id desc limit 1"
                        ),
                        &with_refs(&task_id, &refs),
                    )?;
                    let payload = match &row {
                        Some(row) => row
                            .get("payload_json")?
                            .loads()
                            .unwrap_or_else(|_| Py::dict()),
                        None => Py::dict(),
                    };
                    let payload = if payload.is_dict() {
                        payload
                    } else {
                        Py::dict()
                    };
                    if legacy_summary_needed {
                        if let Some(summary) = crate::common::nonblank(payload.get("summary")) {
                            attempt_summary = Some(summary.to_owned());
                        }
                    }
                    if legacy_checks_needed {
                        if let Some(checks) = project_checks(payload.get("checks"), true) {
                            result.set("checks", checks);
                        }
                    }
                }
                if let Some(summary) = attempt_summary {
                    let mut summary = scrub(py::strip(&summary));
                    if py::len(&summary) > SUMMARY_MAX_CHARS {
                        summary = format!("{}…", py::prefix(&summary, SUMMARY_MAX_CHARS - 1));
                        result.set("summaryTruncated", Py::Bool(true));
                    }
                    result.set("summary", Py::str(summary));
                }
                if let Some(Py::List(checks)) = result.get_opt("checks") {
                    let open_issues = checks
                        .iter()
                        .filter(|c| {
                            !py::eq_str(c.get("outcome"), "passed") && !c.contains("closedBy")
                        })
                        .count();
                    if py::eq_str(result.get("status"), "completed") && open_issues > 0 {
                        result.set("status", Py::str("completed_with_issues"));
                    }
                    if args.compact {
                        result.set("openIssues", Py::int(open_issues as i64));
                        result.remove("checks");
                    }
                }
                if let Some(completed_at) = completed_at {
                    if !completed_at.is_null() {
                        result.set("completedAt", completed_at.to_py()?);
                    }
                }
                if let Some(route) = attempt_route {
                    result.set("route", route);
                }
                if !args.compact && (args.attempt_id.is_some() || args.assignment_ref.is_some()) {
                    let mut requests = Vec::new();
                    for request_row in &title_rows {
                        if let Ok(payload) = request_row.get("payload_json")?.loads() {
                            if payload.is_dict() {
                                requests.push(payload);
                            }
                        }
                    }
                    let active = status_is(result.get("status"), &ACTIVE);
                    let prompt = attempt_prompt(db, &task_id, at, &requests, &refs, active)?;
                    result.set("prompt", prompt);
                }
                attempt_out.push(result);
            }
            if !attempt_out.is_empty() || args.assignment_ref.is_some() {
                let public_ref = match (selected_request, title_rows.last()) {
                    (Some(request), _) => request.get("assignment_ref")?.to_py()?,
                    (None, Some(last)) => last.get("assignment_ref")?.to_py()?,
                    (None, None) => idempotency_key.to_py()?,
                };
                let status = match attempt_out.last() {
                    Some(last) => last.get("status").clone(),
                    None => {
                        let status = a.get("status")?;
                        if status.truthy() {
                            status.to_py()?
                        } else {
                            Py::str("pending")
                        }
                    }
                };
                let mut assignment = dict(vec![
                    ("taskId", task_id.to_py()?),
                    ("id", assignment_id.to_py()?),
                    ("assignmentRef", public_ref),
                    ("nodeKey", a.get("node_key")?.to_py()?),
                    ("status", status),
                    ("attempts", Py::List(attempt_out)),
                ]);
                if let Some(title) = title {
                    assignment.set("title", Py::str(title));
                }
                if let Some(role) = role {
                    assignment.set("role", Py::str(role));
                }
                if let Some(route) = latest_route {
                    assignment.set("route", route);
                }
                assignments.push(assignment);
            }
        }
    }
    let next_cursor = match (truncated, event_ids.iter().max()) {
        (true, Some(id)) => Py::int(*id),
        _ => Py::Null,
    };
    let next_assignment_cursor = match (more_assignments, assignments.last()) {
        (true, Some(last)) => last.get("id").clone(),
        _ => Py::Null,
    };
    Ok(Output::Printed(printed(&dict(vec![
        ("assignments", Py::List(assignments)),
        ("available", Py::Bool(true)),
        ("truncated", Py::Bool(truncated)),
        ("nextCursor", next_cursor),
        ("nextAssignmentCursor", next_assignment_cursor),
    ]))?))
}
