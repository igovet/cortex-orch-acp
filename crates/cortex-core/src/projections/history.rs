//! Allowlisted display projection; never return read bodies, thoughts or raw envelopes.
use super::{field, row, ver};
use crate::{error::refuse, CoreError, CoreResult};
use rusqlite::{Connection, OptionalExtension};
use serde_json::{json, Value};
use workspace_protocol::{
    core::Page,
    read::{HistoryRow, ReadPage},
};

fn hidden(title: &str) -> bool {
    let name = title
        .trim()
        .rsplit("__")
        .next()
        .unwrap_or("")
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();
    matches!(
        name.as_str(),
        "agent_thought_chunk"
            | "thought"
            | "thinking"
            | "reasoning"
            | "wait"
            | "wait_agent"
            | "task_notification"
            | "usage_update"
            | "current_mode_update"
            | "config_option_update"
            | "available_commands_update"
            | "session_info_update"
            | "turn_started"
            | "turn_complete"
    )
}
fn merge(old: &mut Value, new: &Value) {
    if let (Some(old), Some(new)) = (old.as_object_mut(), new.as_object()) {
        for (k, v) in new {
            if v.is_null() {
                continue;
            }
            if matches!(k.as_str(), "rawInput" | "raw_input" | "input") {
                if v.as_object().is_some_and(|m| m.is_empty()) || v.as_str() == Some("") {
                    continue;
                }
                if let Some(prior) = old.get_mut(k).filter(|o| o.is_object() && v.is_object()) {
                    merge(prior, v);
                    continue;
                }
            }
            old.insert(k.clone(), v.clone());
        }
    }
}
fn shell_word(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_@%+=:,./-".contains(&b))
    {
        s.into()
    } else {
        format!("'{}'", s.replace('\'', "'\"'\"'"))
    }
}
fn output_text(v: &Value) -> Option<String> {
    if let Some(s) = v.as_str() {
        return Some(s.into());
    }
    if let Some(a) = v.as_array() {
        let mut parts = a.iter().filter_map(output_text);
        let mut text = parts.next()?;
        for part in parts.filter(|s| !s.is_empty()) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&part);
        }
        return Some(text);
    }
    ["output", "text", "stdout", "formatted_output", "content"]
        .into_iter()
        .find_map(|k| v.get(k))
        .and_then(output_text)
}

/// Selected command text from a retained update. No text (including a terminal
/// reference or an empty content list) is distinct from captured text of length zero.
/// Callers must first check command identity and exclude private calls.
pub fn retained_command_text(update: &Value) -> Option<String> {
    let raw = ["rawOutput", "raw_output", "output"]
        .into_iter()
        .find_map(|key| update.get(key))
        .and_then(output_text);
    // Preserve the display's content fallback when there is no non-empty raw text.
    match raw {
        Some(text) if !text.is_empty() => Some(text),
        empty_or_missing => update
            .get("content")
            .and_then(output_text)
            .or(empty_or_missing),
    }
}
fn detail(update: &Value) -> Value {
    let mut raw = update
        .get("rawInput")
        .or_else(|| update.get("raw_input"))
        .or_else(|| update.get("input"))
        .cloned()
        .unwrap_or(json!({}));
    if let Some(s) = raw.as_str() {
        raw = serde_json::from_str(s).unwrap_or(json!({}));
    }
    if raw["server"] == "cortex" && raw["arguments"].is_object() {
        raw = raw["arguments"].clone();
    } else if raw["input"].is_object() {
        raw = raw["input"].clone();
    }
    let title = field(update, "title").unwrap_or("Tool activity");
    let kind = field(update, "tool_kind")
        .or_else(|| field(update, "kind"))
        .unwrap_or("other");
    let mut d = json!({"title":title,"callId":field(update,"toolCallId").or_else(||field(update,"activity_ref")).unwrap_or(""),"kind":kind});
    if let Some(s) = field(update, "status") {
        d["status"] = json!(s);
    }
    if let (Some(attempt), Some(reference)) = (
        field(&update["commandOutputRef"], "attemptId"),
        field(&update["commandOutputRef"], "toolCallRef"),
    ) {
        if matches!(title, "mcp__cortex__command_run" | "mcp__cortex__outside_command")
            && d["callId"] == format!("cortex-broker:{attempt}:{reference}")
        {
            d["commandOutputRef"] = json!({"attemptId":attempt,"toolCallRef":reference});
        }
    }
    for (target, keys) in [
        (
            "path",
            vec!["file_path", "filePath", "path", "absolutePath"],
        ),
        ("command", vec!["command", "cmd"]),
        ("query", vec!["query", "pattern", "q"]),
        (
            "ref",
            vec!["report_id", "report_ref", "draft_id", "source_ref", "skill"],
        ),
    ] {
        if let Some(s) = keys.iter().find_map(|k| field(&raw, k)) {
            d[target] = json!(s);
        }
    }
    if d.get("path").is_none() {
        if let Some(s) = field(&update["locations"][0], "path") {
            d["path"] = json!(s);
        }
    }
    for key in ["command", "argv"] {
        if d.get("command").is_none() {
            if let Some(a) = raw[key]
                .as_array()
                .filter(|a| a.iter().all(|v| v.is_string()))
            {
                d["command"] = json!(a
                    .iter()
                    .map(|v| shell_word(v.as_str().unwrap_or("")))
                    .collect::<Vec<_>>()
                    .join(" "));
            }
        }
    }
    let operation = title
        .rsplit("__")
        .next()
        .unwrap_or("")
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();
    let private = matches!(
        operation.as_str(),
        "create_draft" | "draft_write" | "write_report" | "result_submit"
    );
    if private && kind == "edit" {
        d["kind"] = json!("other");
    }
    // A refused call's reason as display history retained it (bounded and
    // redacted there), never for a private report or draft call.
    if let Some(s) = field(update, "failure").filter(|s| !private && !s.trim().is_empty()) {
        d["failure"] = json!(s.chars().take(1000).collect::<String>());
    }
    let edit = !private
        && (kind == "edit"
            || matches!(
                operation.as_str(),
                "project_edit"
                    | "project_write"
                    | "outside_write"
                    | "edit"
                    | "write"
                    | "multiedit"
                    | "apply_patch"
            ));
    if edit {
        for (target, keys) in [
            ("oldText", vec!["old_string", "oldString", "oldText"]),
            (
                "newText",
                vec!["new_string", "newString", "newText", "content"],
            ),
        ] {
            if let Some(s) = keys.iter().find_map(|k| field(&raw, k)) {
                d[target] = json!(s);
            }
        }
        if let Some(a) = raw["edits"].as_array() {
            d["oldText"] = json!(a
                .iter()
                .filter_map(|v| field(v, "old_string"))
                .collect::<Vec<_>>()
                .join("\n"));
            d["newText"] = json!(a
                .iter()
                .filter_map(|v| field(v, "new_string"))
                .collect::<Vec<_>>()
                .join("\n"));
        }
    }
    // Diff contents are selected only for non-private calls, never a report body.
    if !private {
        if let Some(a) = update["content"].as_array() {
            for b in a.iter().filter(|b| b["type"] == "diff") {
                for k in ["path", "oldText", "newText"] {
                    if let Some(s) = field(b, k) {
                        d[k] = json!(s);
                    }
                }
            }
        }
    }
    if d.get("command").is_some() && !private {
        if let Some(text) = retained_command_text(update) {
            d["output"] = json!(text);
        }
    }
    if update["projection_partial"] == true {
        d["notice"] = json!("Only the last 100 updates of this call are available in this view.");
    }
    if update["payload_offloaded"] == true {
        d["notice"] = json!("Only a clipped preview of this step was retained in this view.");
    }
    while serde_json::to_vec(&d).map_or(0, |b| b.len()) > 18000 {
        let key = d
            .as_object()
            .unwrap()
            .iter()
            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.len())))
            .max_by_key(|(_, n)| *n)
            .unwrap()
            .0;
        let s = d[&key].as_str().unwrap();
        d[&key] = json!(
            s.chars().take(s.chars().count() / 2).collect::<String>() + "\n[Content clipped]"
        );
    }
    d
}
/// Retained updates of one tool call that a display row merges: at most `PRIOR` of
/// them, newest first; past that bound, the call's first update is merged first.
const PRIOR: usize = 100;
/// The text of a compaction divider row (`kind` "system"); the Workspace draws it as a rule.
pub const COMPACTION_DIVIDER: &str = "Context compacted";
fn display(
    db: &Connection,
    session: &str,
    seq: i64,
    raw: &str,
) -> CoreResult<Option<(String, String, Option<String>)>> {
    display_with(raw, |call| prior_updates(db, session, seq, call))
}
/// A call's newest retained updates up to a row, and its first one. Both are served by the
/// `host_updates_by_call` index: through the session index alone, every tool row walked all
/// earlier updates of its session, so a whole conversation read grew with its length squared.
const PRIOR_SQL: &str = "SELECT update_json FROM host_updates WHERE session_id=? AND seq<=? AND json_valid(update_json) AND json_extract(update_json,'$.toolCallId')=? ORDER BY seq DESC LIMIT 100";
const FIRST_SQL: &str = "SELECT update_json FROM host_updates WHERE session_id=? AND seq<=? AND json_valid(update_json) AND json_extract(update_json,'$.toolCallId')=? ORDER BY seq LIMIT 1";
/// The retained updates of `call` up to `seq` that a display row merges (see `PRIOR`).
fn prior_updates(
    db: &Connection,
    session: &str,
    seq: i64,
    call: &str,
) -> CoreResult<(Vec<String>, Option<String>)> {
    let prior = db
        .prepare(PRIOR_SQL)?
        .query_map(rusqlite::params![session, seq, call], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let initial = if prior.len() >= PRIOR {
        row(db, FIRST_SQL, rusqlite::params![session, seq, call])?
            .and_then(|initial| field(&initial, "update_json").map(str::to_owned))
    } else {
        None
    };
    Ok((prior, initial))
}
/// `display` with the call's retained updates supplied by `prior` (given the call
/// identity): the newest `PRIOR` up to the row, newest first, and the call's first
/// update when that bound is reached.
fn display_with(
    raw: &str,
    prior: impl FnOnce(&str) -> CoreResult<(Vec<String>, Option<String>)>,
) -> CoreResult<Option<(String, String, Option<String>)>> {
    let mut u: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Ok(Some(("unknown".into(), String::new(), None))),
    };
    let kind = field(&u, "sessionUpdate").unwrap_or("");
    // The runtime's marker of a completed context compaction: a divider row.
    if kind == "compaction_update" {
        return Ok((field(&u, "status") == Some("completed"))
            .then(|| ("system".into(), COMPACTION_DIVIDER.into(), None)));
    }
    if !matches!(kind, "plan" | "tool_call" | "tool_call_update")
        || hidden(field(&u, "title").unwrap_or(""))
    {
        return Ok(None);
    }
    if let Some(call) = field(&u, "toolCallId").map(str::to_owned) {
        let (prior, initial) = prior(&call)?;
        let mut merged = json!({});
        if prior.len() >= PRIOR {
            if let Some(initial) = initial {
                merge(&mut merged, &serde_json::from_str::<Value>(&initial)?);
            }
        }
        for p in prior.iter().rev() {
            merge(&mut merged, &serde_json::from_str::<Value>(p)?);
        }
        if prior.len() >= 100 {
            merged["projection_partial"] = json!(true);
        }
        u = merged;
    }
    if hidden(field(&u, "title").unwrap_or("")) {
        return Ok(None);
    }
    let status = field(&u, "status").map(str::to_owned);
    if u["sessionUpdate"] == "plan" {
        let entries = u["entries"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|v| {
                        format!(
                            "{} · {}",
                            field(v, "content").unwrap_or(""),
                            field(v, "status").unwrap_or("unknown")
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default();
        return Ok(Some(("system".into(), format!("Plan\n{entries}"), None)));
    }
    let mut d = detail(&u);
    let title = field(&u, "title").unwrap_or("Tool activity");
    let text = if title.starts_with("Worker")
        || (!["path", "command", "query", "newText"]
            .iter()
            .any(|k| d.get(k).is_some())
            && u.get("toolCallId").is_none())
    {
        format!(
            "{title}{}",
            status
                .as_ref()
                .map(|s| format!(" · {s}"))
                .unwrap_or_default()
        )
    } else {
        if u["sessionUpdate"] == "tool_call_update" && u.get("title").is_none() {
            d.as_object_mut().unwrap().remove("title");
        }
        format!("Workspace step: {}", serde_json::to_string(&d)?)
    };
    Ok(Some(("tool".into(), text, status)))
}
/// One tool call of display history exactly as the task timeline presents it: the
/// call's retained updates up to `seq` merged into one selected "Workspace step"
/// (or its plain title), with its status. None for a hidden or non-tool update.
/// Other owner views (a worker's page) reuse it so both read the same rows the same way.
pub fn tool_step(
    db: &Connection,
    session: &str,
    seq: i64,
    raw: &str,
) -> CoreResult<Option<(String, Option<String>)>> {
    Ok(match display(db, session, seq, raw)? {
        Some((kind, text, status)) if kind == "tool" => Some((text, status)),
        _ => None,
    })
}
/// `tool_step` for the row `seq` of a call whose retained updates were already read in
/// one pass: `updates` are every retained update of call `call` up to and including the
/// row, oldest first (`json_valid` rows whose text `toolCallId` is `call`). The same text
/// and status as `tool_step`, without one session scan per call. A row whose own call
/// identity reads differently is looked up exactly as `tool_step` does.
pub fn tool_step_from(
    db: &Connection,
    session: &str,
    seq: i64,
    raw: &str,
    call: &str,
    updates: &[String],
) -> CoreResult<Option<(String, Option<String>)>> {
    let read = display_with(raw, |parsed| {
        if parsed != call {
            return prior_updates(db, session, seq, parsed);
        }
        let prior = updates
            .iter()
            .rev()
            .take(PRIOR)
            .cloned()
            .collect::<Vec<_>>();
        let initial = (prior.len() >= PRIOR)
            .then(|| updates.first().cloned())
            .flatten();
        Ok((prior, initial))
    })?;
    Ok(match read {
        Some((kind, text, status)) if kind == "tool" => Some((text, status)),
        _ => None,
    })
}
/// The break a reader sees between two messages whose streamed rows follow each
/// other. Display history records a reply one chunk per row and the client joins
/// adjacent chunks back into one message, so without it the last sentence of one
/// message (say the coordinator's) ran straight into the next (a worker's).
const PARAGRAPH: &str = "\n\n";
/// Whether this agent chunk begins a different message than the agent chunk
/// right before it: both carry an identity and the identities differ. A chunk
/// after a user message, or one with no recorded identity, is left as written.
fn starts_message(
    db: &Connection,
    session: &str,
    seq: i64,
    message: Option<&str>,
) -> CoreResult<bool> {
    let Some(message) = message else {
        return Ok(false);
    };
    let previous: Option<(String, Option<String>)> = db
        .query_row(
            "SELECT kind,message_id FROM host_transcript WHERE session_id=? AND seq<? ORDER BY seq DESC LIMIT 1",
            rusqlite::params![session, seq],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    Ok(matches!(previous, Some((kind, Some(id))) if kind == "agent_message" && id != message))
}
/// Whether an agent message was written by a worker or reviewer rather than the coordinator. A native message's identity is
/// `{attempt}:{engine message}`; only a message whose attempt the task recorded under a role other than the orchestrator
/// is attributed to a worker. Any other message (no identity, an older identity, the synthesis, an unknown attempt or a store
/// with no delegation record) stays the coordinator's, so an older history reads as it always did. `task` is the runtime task
/// the attempts are recorded under (`workspace_executions.runtime_task_id`), never the workspace task identity.
fn from_worker(db: &Connection, task: &str, message: Option<&str>) -> CoreResult<bool> {
    let Some(attempt) = message
        .and_then(|m| m.split_once(':'))
        .map(|(a, _)| a)
        .filter(|a| !a.is_empty())
    else {
        return Ok(false);
    };
    let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='delegation_provisioning')", [], |r| r.get(0))?;
    if !exists {
        return Ok(false);
    }
    let roles = db
        .prepare(
            "SELECT DISTINCT role_id FROM delegation_provisioning WHERE task_id=? AND attempt_id=?",
        )?
        .query_map(rusqlite::params![task, attempt], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(!roles.is_empty() && roles.iter().all(|r| r != "orchestrator"))
}
/// One source row as `(seq, kind, text, update, recorded at, message identity)`.
type Source = (
    i64,
    String,
    Option<String>,
    Option<String>,
    String,
    Option<String>,
);
/// The rows of `session` that one of `sqls` (one per display table, each through its (session, seq) index) selects,
/// merged by sequence: ascending, or descending for a backward read. Each statement takes the session, its two
/// position bounds `a` and `b` and the row limit.
fn sources(
    db: &Connection,
    sqls: [&str; 2],
    session: &str,
    a: i64,
    b: i64,
    limit: u16,
    descending: bool,
) -> CoreResult<Vec<Source>> {
    let mut rows = Vec::new();
    for sql in sqls {
        rows.extend(
            db.prepare(sql)?
                .query_map(rusqlite::params![session, a, b, limit + 1], |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, Option<String>>(3)?,
                        r.get::<_, String>(4)?,
                        r.get::<_, Option<String>>(5)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    rows.sort_by_key(|r| r.0);
    if descending {
        rows.reverse();
    }
    rows.truncate(limit as usize + 1);
    Ok(rows)
}
/// The display row one source row reads as, or None for a row the conversation leaves out (a worker's narration or
/// tool call, a hidden update). Every row reads the same whichever direction it is read in: what it merges (its
/// call's earlier updates, the message before it) is looked up by its own position.
fn display_row(
    db: &Connection,
    session: &str,
    runtime: &str,
    source: &Source,
) -> CoreResult<Option<HistoryRow>> {
    let (seq, kind, text, update, at, message) = source;
    // A worker's or reviewer's narration is not the task's conversation: it is read on that worker's own page. A row
    // left out is consumed like a hidden update, so paging never stalls on it.
    if kind == "agent_message" && from_worker(db, runtime, message.as_deref())? {
        return Ok(None);
    }
    // A worker's tool calls (call identity `{attempt}:{call}`) belong to its own page, like its narration.
    if kind == "update" {
        let call = update
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .and_then(|u| field(&u, "toolCallId").map(str::to_owned));
        if from_worker(db, runtime, call.as_deref())? {
            return Ok(None);
        }
    }
    let (kind, text, status) = if kind == "update" {
        let Some((kind, text, status)) =
            display(db, session, *seq, update.as_deref().unwrap_or(""))?
        else {
            return Ok(None);
        };
        let text = if kind == "unknown" { None } else { Some(text) };
        (kind, text, status)
    } else {
        (
            if kind == "user_message" {
                "user"
            } else {
                "assistant"
            }
            .into(),
            if kind == "agent_message" {
                text.as_ref()
                    .map(|t| {
                        starts_message(db, session, *seq, message.as_deref()).map(|new| {
                            if new {
                                format!("{PARAGRAPH}{t}")
                            } else {
                                t.clone()
                            }
                        })
                    })
                    .transpose()?
            } else {
                text.clone()
            },
            None,
        )
    };
    let mut item = HistoryRow {
        history_id: format!(
            "wh_{}",
            &crate::digest(serde_json::to_string(&json!([session, seq]))?.as_bytes())[..32]
        ),
        sequence: ver(seq + 1)?,
        revision: ver(1)?,
        kind,
        text,
        tool_status: status,
        recorded_at: at.clone(),
        availability: "available".into(),
        reason: None,
    };
    if item.text.is_none() {
        item.availability = "unavailable".into();
        item.reason = Some("invalid_history_content".into());
    }
    if item
        .text
        .as_ref()
        .is_some_and(|s| s.chars().count() > 32000)
    {
        item.text = None;
        item.availability = "unavailable".into();
        item.reason = Some("content_exceeds_wire_limit".into());
    }
    Ok(Some(item))
}
/// The bytes of one page's rows: the page stops before a row that would pass it, and a single row over it reads as
/// unavailable.
const PAGE_BYTES: usize = 58_000;
/// Adds `item` to a page holding `items` (`bytes` so far). False, and nothing added, when it would pass the bound.
fn fits(items: &mut Vec<HistoryRow>, bytes: &mut usize, mut item: HistoryRow) -> CoreResult<bool> {
    let size = serde_json::to_vec(&item)?.len();
    if *bytes + size > PAGE_BYTES && !items.is_empty() {
        return Ok(false);
    }
    if size > PAGE_BYTES {
        item.text = None;
        item.availability = "unavailable".into();
        item.reason = Some("content_exceeds_page_limit".into());
    }
    *bytes += serde_json::to_vec(&item)?.len() + 1;
    items.push(item);
    Ok(true)
}
/// The task's bound session, the runtime task its attempts are recorded under, and the page's cursor binding.
fn bound(db: &Connection, project: &str, p: &Page) -> CoreResult<(String, String, String)> {
    let e=row(db,"SELECT e.acp_session_id,e.runtime_task_id FROM workspace_executions e JOIN workspace_tasks t USING(task_id) WHERE t.project_id=? AND t.task_id=?", [project,&p.target])?.ok_or(CoreError::Refused("not_found"))?;
    let session = field(&e, "acp_session_id").unwrap_or("").to_owned();
    // Worker attempts are recorded under the runtime task, not the workspace task the page is asked for.
    let runtime = field(&e, "runtime_task_id").unwrap_or("").to_owned();
    let binding = crate::digest(
        serde_json::to_string(&json!([project, p.target, session, p.limit]))?.as_bytes(),
    );
    Ok((session, runtime, binding))
}
pub(super) fn page(db: &Connection, project: &str, p: &Page) -> CoreResult<ReadPage<HistoryRow>> {
    let (session, runtime, binding) = bound(db, project, p)?;
    let session = session.as_str();
    let maximum: i64 = db.query_row(MAXIMUM_SQL, [session, session], |r| r.get(0))?;
    let (high, after) = if let Some(c) = p.cursor.as_deref().filter(|c| c.starts_with("nt1.")) {
        // A forward (tail) cursor: every row written after the position it names, up to the newest row now.
        let fields: Vec<_> = c.split('.').collect();
        if fields.len() != 4 || fields[1] != binding {
            return refuse("invalid_cursor");
        }
        let after = fields[2]
            .parse::<i64>()
            .map_err(|_| CoreError::Refused("invalid_cursor"))?;
        if after.to_string() != fields[2] || after < -1 {
            return refuse("invalid_cursor");
        }
        // The rows a reader already holds are gone or were written again: it must read the whole history afresh.
        if after > maximum || anchor(db, session, after)? != fields[3] {
            return refuse("history_reset");
        }
        (maximum, after)
    } else if let Some(c) = &p.cursor {
        let fields: Vec<_> = c.split('.').collect();
        if fields.len() != 4 || fields[0] != "nh2" || fields[1] != binding {
            return refuse("invalid_cursor");
        }
        let high = fields[2]
            .parse::<i64>()
            .map_err(|_| CoreError::Refused("invalid_cursor"))?;
        let after = fields[3]
            .parse::<i64>()
            .map_err(|_| CoreError::Refused("invalid_cursor"))?;
        if high.to_string() != fields[2]
            || after.to_string() != fields[3]
            || after < 0
            || after > high
            || high > maximum
        {
            return refuse("invalid_cursor");
        }
        (high, after)
    } else {
        (maximum, -1)
    };
    // Each table is read on its own through its (session, seq) index, from the position on, and the two are merged in
    // sequence order: the cost is the page's, never the length of the history before it.
    let rows = sources(
        db,
        [TRANSCRIPT_SQL, UPDATES_SQL],
        session,
        after,
        high,
        p.limit,
        false,
    )?;
    let mut items = Vec::new();
    let mut consumed = after;
    let mut bytes = 256;
    for source in rows.iter().take(p.limit as usize) {
        if let Some(item) = display_row(db, session, &runtime, source)? {
            if !fits(&mut items, &mut bytes, item)? {
                break;
            }
        }
        consumed = source.0;
    }
    let next_cursor = if !rows.is_empty() && consumed < high {
        Some(format!("nh2.{binding}.{high}.{consumed}"))
    } else {
        None
    };
    // Everything up to `position` has been handed out (or deliberately left out); a later read continues after it.
    let position = if next_cursor.is_some() {
        consumed
    } else {
        high
    };
    let tail_cursor = Some(format!(
        "nt1.{binding}.{position}.{}",
        anchor(db, session, position)?
    ));
    Ok(ReadPage {
        items,
        next_cursor,
        high_watermark: ver(high + 1)?,
        tail_cursor,
        prev_cursor: None,
    })
}
/// The conversation read from its end backwards (`history.earlier`). Without a cursor: the newest `limit` source rows
/// of the session, with the forward cursor (`tailCursor`) at the newest row, exactly as a complete forward read would
/// end, so later rows are read forward as before. With a backward cursor (`prevCursor`, `np1.`): the `limit` source
/// rows before the row it names. Rows are returned in source order and read exactly as a forward read returns them;
/// `prevCursor` names the first row consumed while earlier rows remain. A cursor of another project, task, session or
/// page size is `invalid_cursor`; one whose row is gone or was written again is `history_reset`.
pub(super) fn earlier(
    db: &Connection,
    project: &str,
    p: &Page,
) -> CoreResult<ReadPage<HistoryRow>> {
    let (session, runtime, binding) = bound(db, project, p)?;
    let session = session.as_str();
    let maximum: i64 = db.query_row(MAXIMUM_SQL, [session, session], |r| r.get(0))?;
    let before = if let Some(c) = &p.cursor {
        let fields: Vec<_> = c.split('.').collect();
        if fields.len() != 4 || fields[0] != "np1" || fields[1] != binding {
            return refuse("invalid_cursor");
        }
        let before = fields[2]
            .parse::<i64>()
            .map_err(|_| CoreError::Refused("invalid_cursor"))?;
        if before.to_string() != fields[2] || before < 1 {
            return refuse("invalid_cursor");
        }
        if before > maximum || anchor(db, session, before)? != fields[3] {
            return refuse("history_reset");
        }
        before
    } else {
        maximum + 1
    };
    let rows = sources(
        db,
        [TRANSCRIPT_BEFORE_SQL, UPDATES_BEFORE_SQL],
        session,
        before,
        -1,
        p.limit,
        true,
    )?;
    let mut items = Vec::new();
    let mut consumed = before;
    let mut bytes = 256;
    for source in rows.iter().take(p.limit as usize) {
        if let Some(item) = display_row(db, session, &runtime, source)? {
            if !fits(&mut items, &mut bytes, item)? {
                break;
            }
        }
        consumed = source.0;
    }
    items.reverse();
    // `rows` holds the newest `limit + 1` rows before the position: a row it holds that was not consumed is an earlier one.
    let prev_cursor = if rows.iter().any(|r| r.0 < consumed) {
        Some(format!(
            "np1.{binding}.{consumed}.{}",
            anchor(db, session, consumed)?
        ))
    } else {
        None
    };
    let tail_cursor = if p.cursor.is_none() {
        Some(format!(
            "nt1.{binding}.{maximum}.{}",
            anchor(db, session, maximum)?
        ))
    } else {
        None
    };
    Ok(ReadPage {
        items,
        next_cursor: None,
        high_watermark: ver(maximum + 1)?,
        tail_cursor,
        prev_cursor,
    })
}
/// The newest display position of a session: each table's own maximum through its (session, seq) index.
const MAXIMUM_SQL: &str = "SELECT max(coalesce((SELECT max(seq) FROM host_transcript WHERE session_id=?1),-1),coalesce((SELECT max(seq) FROM host_updates WHERE session_id=?2),-1))";
const TRANSCRIPT_SQL: &str = "SELECT seq,kind,text,NULL,created_at,message_id FROM host_transcript WHERE session_id=?1 AND seq>?2 AND seq<=?3 ORDER BY seq LIMIT ?4";
const UPDATES_SQL: &str = "SELECT seq,'update',NULL,update_json,created_at,NULL FROM host_updates WHERE session_id=?1 AND seq>?2 AND seq<=?3 ORDER BY seq LIMIT ?4";
/// A backward read's rows of each table before a position (`?2`) and after `?3`, newest first.
const TRANSCRIPT_BEFORE_SQL: &str = "SELECT seq,kind,text,NULL,created_at,message_id FROM host_transcript WHERE session_id=?1 AND seq<?2 AND seq>?3 ORDER BY seq DESC LIMIT ?4";
const UPDATES_BEFORE_SQL: &str = "SELECT seq,'update',NULL,update_json,created_at,NULL FROM host_updates WHERE session_id=?1 AND seq<?2 AND seq>?3 ORDER BY seq DESC LIMIT ?4";
const ANCHOR_SQL: &str = "SELECT created_at FROM host_transcript WHERE session_id=?1 AND seq=?2 UNION ALL SELECT created_at FROM host_updates WHERE session_id=?1 AND seq=?2";
/// What a forward cursor checks its position against: the recorded time of the row at that position, so a history that
/// was removed or written again under the same positions is detected instead of continued. `0` before the first row.
fn anchor(db: &Connection, session: &str, position: i64) -> CoreResult<String> {
    if position < 0 {
        return Ok("0".into());
    }
    let at = db
        .prepare(ANCHOR_SQL)?
        .query_map(rusqlite::params![session, position], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(crate::digest(serde_json::to_string(&json!([position, at]))?.as_bytes())[..16].to_owned())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE workspace_tasks(task_id TEXT, project_id TEXT);
             CREATE TABLE workspace_executions(execution_id TEXT, task_id TEXT, runtime_task_id TEXT, acp_session_id TEXT);
             CREATE TABLE host_transcript(session_id TEXT, seq INTEGER, kind TEXT, text TEXT, message_id TEXT, created_at TEXT);
             CREATE TABLE host_updates(session_id TEXT, seq INTEGER, update_json TEXT, created_at TEXT);
             CREATE TABLE delegation_provisioning(task_id TEXT, attempt_id TEXT, role_id TEXT);
             INSERT INTO delegation_provisioning VALUES ('r','xa_c','orchestrator'),('r','xa_w','worker-mobile-dev'),('t','xa_t','worker-mobile-dev');
             INSERT INTO workspace_tasks VALUES ('t','p');
             INSERT INTO workspace_executions VALUES ('e','t','r','s');",
        )
        .unwrap();
        db
    }
    #[test]
    fn command_output_ref_on_conversation_is_a_namespaced_receipt_only() {
        let db = fixture();
        let reference = json!({"attemptId":"xa_c","toolCallRef":"native-command:fixture"});
        for (seq, call) in [(0, "cortex-broker:xa_c:native-command:fixture"), (1, "xa_c:native-command:fixture")] {
            let update = json!({"sessionUpdate":"tool_call","toolCallId":call,
                "title":"mcp__cortex__command_run","kind":"execute","status":"completed",
                "commandOutputRef":reference,"large_output_id":"never-projected",
                "rawOutput":"not a retained display field"});
            db.execute("INSERT INTO host_updates VALUES ('s',?,?,'now')",
                (seq, update.to_string())).unwrap();
        }
        let request = Page { target: "t".into(), cursor: None, limit: 20 };
        let result = page(&db, "p", &request).unwrap();
        let get = |n:usize| -> Value {
            serde_json::from_str(result.items[n].text.as_deref().unwrap().strip_prefix("Workspace step: ").unwrap()).unwrap()
        };
        assert_eq!(get(0)["commandOutputRef"], reference);
        assert!(get(0).get("output").is_none());
        assert!(get(0).get("large_output_id").is_none());
        assert!(get(1).get("commandOutputRef").is_none());
    }

    #[test]
    fn native_command_text_distinguishes_missing_from_captured_empty_in_feed_steps() {
        let db = fixture();
        for (index, (fields, expected)) in [
            (json!({}), None),
            (json!({"rawOutput":null,"content":[]}), None),
            (json!({"rawOutput":{"exit_code":0},"content":[{"type":"terminal","terminalId":"term"}]}), None),
            (json!({"rawOutput":""}), Some("")),
            (json!({"raw_output":{"stdout":""}}), Some("")),
            (json!({"content":[{"type":"content","content":{"type":"text","text":""}}]}), Some("")),
            (json!({"rawOutput":"","content":[{"type":"content","content":{"type":"text","text":"recorded"}}]}), Some("recorded")),
        ]
        .into_iter()
        .enumerate()
        {
            let call = format!("xa_c:exec-{index}");
            let initial = json!({"sessionUpdate":"tool_call","toolCallId":call,"title":"Bash",
                "kind":"execute","status":"in_progress","rawInput":{"command":"printf ''"}});
            let mut settled = json!({"sessionUpdate":"tool_call_update","toolCallId":call,
                "status":"completed"});
            settled
                .as_object_mut()
                .unwrap()
                .extend(fields.as_object().unwrap().clone());
            let first = (index * 2) as i64;
            let updates = [initial.to_string(), settled.to_string()];
            for (seq, update) in (first..).zip(&updates) {
                db.execute("INSERT INTO host_updates VALUES ('s',?,?,'now')", (seq, update))
                    .unwrap();
            }

            // The native worker page uses this batched entry point for the same step.
            let worker = tool_step_from(&db, "s", first + 1, &updates[1], &call, &updates)
                .unwrap()
                .unwrap();
            let items = page(
                &db,
                "p",
                &Page {
                    target: "t".into(),
                    cursor: None,
                    limit: 100,
                },
            )
            .unwrap()
            .items;
            let waiting = items[index * 2].text.as_deref().unwrap();
            let completed = items[index * 2 + 1].text.as_deref().unwrap();
            let decode = |step: &str| -> Value {
                serde_json::from_str(step.strip_prefix("Workspace step: ").unwrap()).unwrap()
            };
            assert_eq!(decode(waiting)["status"], "in_progress");
            assert!(decode(waiting).get("output").is_none(), "{waiting}");
            assert_eq!(completed, worker.0);
            assert_eq!(worker.1.as_deref(), Some("completed"));
            let done = decode(completed);
            assert_eq!(done["status"], "completed");
            assert_eq!(
                done.get("output"),
                expected.map(|s| json!(s)).as_ref(),
                "{done}"
            );
        }
    }

    /// The store's own schema, with one session `s` of task `t` holding `calls` tool calls of
    /// three updates each, one call of 150 updates, and one malformed update.
    fn retained(calls: usize) -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(crate::store::schema::SQL).unwrap();
        // Only the history tables matter here; their referenced task and session rows do not.
        db.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        db.execute_batch(
            "INSERT INTO workspace_tasks(task_id,project_id,description,version,execution_id,created_at) VALUES ('t','p','d',1,'e','now');
             INSERT INTO workspace_executions(execution_id,task_id,runtime_task_id,acp_session_id,owner_epoch,bound_at) VALUES ('e','t','r','s',1,'now');",
        )
        .unwrap();
        let tx = db.unchecked_transaction().unwrap();
        let mut seq = 0i64;
        let mut add = |update: String| {
            tx.execute(
                "INSERT INTO host_updates VALUES ('s',?,?,'now')",
                rusqlite::params![seq, update],
            )
            .unwrap();
            seq += 1;
        };
        add("not json".into());
        for n in 0..150 {
            add(json!({"sessionUpdate":if n == 0 {"tool_call"} else {"tool_call_update"},"toolCallId":"long","title":"Run tests","kind":"execute","status":"in_progress","content":[{"type":"content","content":{"type":"text","text":format!("line {n}")}}]}).to_string());
        }
        for i in 0..calls {
            for (n, status) in ["pending", "in_progress", "completed"]
                .into_iter()
                .enumerate()
            {
                add(json!({"sessionUpdate":if n == 0 {"tool_call"} else {"tool_call_update"},"toolCallId":format!("c{i}"),"title":"Read a.ts","kind":"read","status":status,"rawInput":{"path":format!("a{i}.ts")}}).to_string());
            }
        }
        drop(add);
        tx.commit().unwrap();
        db
    }

    /// Every tool row merges its call's earlier updates. Through the session index alone that
    /// lookup walked every earlier update of the session, so one late page of a long task took
    /// seconds and a whole conversation read grew with its length squared, holding the
    /// project's one read connection meanwhile. Both lookups must be served by the call index.
    #[test]
    fn a_rows_call_updates_are_read_through_the_call_index() {
        let db = retained(10);
        for sql in [PRIOR_SQL, FIRST_SQL] {
            let plan = db
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(rusqlite::params!["s", 1000, "c3"], |r| {
                    r.get::<_, String>(3)
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(
                plan.iter().any(|step| step.contains(
                    "USING INDEX host_updates_by_call (session_id=? AND <expr>=? AND seq<?)"
                )),
                "{sql}: {plan:?}"
            );
            assert!(
                !plan.iter().any(|step| step.contains("TEMP B-TREE")),
                "{sql}: {plan:?}"
            );
        }
    }

    /// The index changes how the rows are found, never which: a malformed update is still
    /// retained (the partial index never evaluates it) and every page reads as before.
    #[test]
    fn the_call_index_leaves_every_page_unchanged() {
        let indexed = retained(400);
        let plain = retained(400);
        plain
            .execute_batch("DROP INDEX host_updates_by_call")
            .unwrap();
        for limit in [100, 7] {
            let a = texts(&indexed, limit);
            assert_eq!(a, texts(&plain, limit));
            assert!(a.iter().any(|t| t.contains("\"callId\":\"long\"")), "{a:?}");
        }
    }
    fn texts(db: &Connection, limit: u16) -> Vec<String> {
        let mut out = Vec::new();
        let mut cursor = None;
        loop {
            let p = page(
                db,
                "p",
                &Page {
                    target: "t".into(),
                    cursor,
                    limit,
                },
            )
            .unwrap();
            out.extend(
                p.items
                    .into_iter()
                    .map(|r| format!("{}|{}", r.kind, r.text.unwrap_or_default())),
            );
            cursor = p.next_cursor;
            if cursor.is_none() {
                return out;
            }
        }
    }

    #[test]
    fn a_completed_compaction_reads_as_one_divider_row() {
        let db = fixture();
        db.execute(
            "INSERT INTO host_transcript VALUES ('s',0,'agent_message','Before.',NULL,'now')",
            [],
        )
        .unwrap();
        for (seq, update) in [
            (
                1,
                json!({"sessionUpdate":"compaction_update","toolCallId":"xa_c:compaction:k","status":"completed"}),
            ),
            (
                2,
                json!({"sessionUpdate":"compaction_update","toolCallId":"xa_c:compaction:k","status":"in_progress"}),
            ),
        ] {
            db.execute(
                "INSERT INTO host_updates VALUES ('s',?,?,'now')",
                rusqlite::params![seq, update.to_string()],
            )
            .unwrap();
        }
        db.execute(
            "INSERT INTO host_transcript VALUES ('s',3,'agent_message','After.',NULL,'now')",
            [],
        )
        .unwrap();
        assert_eq!(
            texts(&db, 100),
            [
                "assistant|Before.".to_owned(),
                format!("system|{COMPACTION_DIVIDER}"),
                "assistant|After.".to_owned(),
            ]
        );
    }

    /// A retained live store: the coordinator's last chunk ("ARCHITECTURE.md`.")
    /// was followed directly by a worker's first chunk ("I") and the two read as
    /// "…ARCHITECTURE.md.I'll start…".
    #[test]
    fn a_new_message_starts_a_new_paragraph_and_a_streamed_one_stays_whole() {
        let db = fixture();
        for (seq, kind, text, id) in [
            (0, "user_message", "Index it", None),
            (1, "agent_message", "Индексирую `ARCH", Some("xa_c:msg_1")),
            (2, "agent_message", "ITECTURE.md`.", Some("xa_c:msg_1")),
            (3, "agent_message", "I", Some("xa_w:msg_2")),
            (4, "agent_message", "'ll start.", Some("xa_w:msg_2")),
            (6, "agent_message", "After a tool.", Some("xa_w:msg_3")),
            (7, "user_message", "Next", None),
            (8, "agent_message", "Unidentified", None),
            (9, "agent_message", " chunk.", None),
        ] {
            db.execute(
                "INSERT INTO host_transcript VALUES ('s',?,?,?,?,'now')",
                rusqlite::params![seq, kind, text, id],
            )
            .unwrap();
        }
        db.execute("INSERT INTO host_updates VALUES ('s',5,?,'now')", [json!({"sessionUpdate":"tool_call","toolCallId":"c","title":"Read a.ts","kind":"read","rawInput":{"path":"a.ts"}}).to_string()]).unwrap();
        let expected = [
            "user|Index it",
            "assistant|Индексирую `ARCH",
            "assistant|ITECTURE.md`.",
            "user|Next",
            "assistant|Unidentified",
            "assistant| chunk.",
        ];
        let all = texts(&db, 100);
        assert_eq!(
            all.iter()
                .filter(|t| !t.starts_with("tool|"))
                .cloned()
                .collect::<Vec<_>>(),
            expected
        );
        // A page boundary between the two messages keeps the break.
        assert_eq!(
            texts(&db, 3)
                .iter()
                .filter(|t| !t.starts_with("tool|"))
                .cloned()
                .collect::<Vec<_>>(),
            expected
        );
    }

    /// A worker's narration is left out of the task history. A history written before attempts were recorded (no delegation table, or an attempt the task never recorded) keeps
    /// every message as the coordinator's. Attempts are looked up under the runtime task (`r`), which differs from the
    /// workspace task the page is read for (`t`): an attempt filed under the workspace identity names no worker of this run.
    #[test]
    fn a_message_is_a_workers_only_when_its_attempt_is_a_recorded_non_orchestrator_role() {
        let db = fixture();
        for (seq, id) in [
            (0, Some("xa_w:m")),
            (1, Some("xa_unknown:m")),
            (2, Some("synthesis:s:2")),
            (3, None),
            (4, Some("xa_t:m")),
            (5, Some("xa_c:m")),
        ] {
            db.execute(
                "INSERT INTO host_transcript VALUES ('s',?,'agent_message','t',?,'now')",
                rusqlite::params![seq, id],
            )
            .unwrap();
        }
        let kinds = |db: &Connection| {
            texts(db, 100)
                .into_iter()
                .map(|t| t.split('|').next().unwrap().to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(kinds(&db), ["assistant"; 5]);
        db.execute_batch("DROP TABLE delegation_provisioning")
            .unwrap();
        assert_eq!(kinds(&db), ["assistant"; 6]);
    }

    /// Conversation shows only the coordinator: tool calls of a worker attempt are left out, the coordinator's stay.
    #[test]
    fn conversation_omits_worker_tool_calls_and_keeps_the_coordinators() {
        let db = fixture();
        for (seq, call) in [
            (0, "xa_c:one"),
            (1, "xa_w:two"),
            (2, "xa_c:three"),
            (3, "xa_w:four"),
        ] {
            let update = json!({"sessionUpdate":"tool_call","toolCallId":call,"title":"Read a.ts","kind":"read","rawInput":{"path":"a.ts"}});
            db.execute(
                "INSERT INTO host_updates VALUES ('s',?,?,'now')",
                rusqlite::params![seq, update.to_string()],
            )
            .unwrap();
        }
        let calls = texts(&db, 2)
            .iter()
            .filter_map(|t| {
                t.strip_prefix("tool|Workspace step: ").map(|s| {
                    serde_json::from_str::<Value>(s).unwrap()["callId"]
                        .as_str()
                        .unwrap()
                        .to_owned()
                })
            })
            .collect::<Vec<_>>();
        assert_eq!(calls, ["xa_c:one", "xa_c:three"]);
    }

    /// On the store's own schema, where the workspace task (`t`) and the runtime task its attempts are recorded under (`r`)
    /// differ, a worker's narration and tool calls written into the coordinator's session are left out; the coordinator's stay.
    #[test]
    fn on_the_store_schema_a_workers_rows_are_found_by_the_runtime_task() {
        let db = retained(0);
        db.execute_batch(
            "DELETE FROM host_updates;
             INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) VALUES
               ('k1','r','xa_c','orchestrator','ready','ready',1,'0','0'),('k2','r','xa_w','worker-devops-engineer','ready','ready',1,'0','0');
             INSERT INTO host_transcript(session_id,seq,kind,text,message_id,created_at) VALUES
               ('s',0,'agent_message','coordinator','xa_c:m1','now'),('s',1,'agent_message','worker','xa_w:m2','now');",
        )
        .unwrap();
        for (seq, call) in [(2, "xa_c:one"), (3, "xa_w:two")] {
            let update = json!({"sessionUpdate":"tool_call","toolCallId":call,"title":"Run","kind":"execute","status":"in_progress","rawInput":{"command":"ls"}});
            db.execute(
                "INSERT INTO host_updates VALUES ('s',?,?,'now')",
                rusqlite::params![seq, update.to_string()],
            )
            .unwrap();
        }
        let all = texts(&db, 100);
        assert!(all.iter().any(|t| t == "assistant|coordinator"), "{all:?}");
        assert!(
            !all.iter()
                .any(|t| t.contains("worker") || t.contains("xa_w")),
            "{all:?}"
        );
        assert!(all.iter().any(|t| t.contains("xa_c:one")), "{all:?}");
    }

    #[test]
    fn a_refused_calls_retained_reason_reads_on_its_step_but_never_a_private_ones() {
        let db = fixture();
        for (seq, update) in [
            json!({"sessionUpdate":"tool_call","toolCallId":"plan","title":"orchestrator: mcp__cortex__plan_submit","kind":"other","status":"pending"}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"plan","status":"failed","failure":"No plan was recorded (plan_invalid)."}),
            json!({"sessionUpdate":"tool_call","toolCallId":"draft","title":"worker-x: mcp__cortex__draft_write","kind":"edit","status":"pending"}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"draft","status":"failed","failure":"PRIVATE"}),
        ]
        .into_iter()
        .enumerate()
        {
            db.execute("INSERT INTO host_updates VALUES ('s',?,?,'now')", rusqlite::params![seq as i64, update.to_string()]).unwrap();
        }
        let steps: Vec<Value> = texts(&db, 100)
            .iter()
            .filter_map(|t| {
                t.strip_prefix("tool|Workspace step: ")
                    .map(|s| serde_json::from_str(s).unwrap())
            })
            .collect();
        let last = |id: &str| {
            steps
                .iter()
                .rev()
                .find(|s| s["callId"] == id)
                .unwrap()
                .clone()
        };
        assert_eq!(
            last("plan")["failure"],
            "No plan was recorded (plan_invalid)."
        );
        assert_eq!(last("plan")["status"], "failed");
        assert!(last("draft").get("failure").is_none());
    }

    /// The store's schema with an empty session `s` of task `t` (runtime task `r`), whose coordinator attempt is `xa_c`
    /// and whose worker attempt is `xa_w`.
    fn session() -> Connection {
        let db = retained(0);
        db.execute_batch(
            "DELETE FROM host_updates;
             INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,owner_epoch,created_at,updated_at) VALUES
               ('k1','r','xa_c','orchestrator','ready','ready',1,'0','0'),('k2','r','xa_w','worker-devops-engineer','ready','ready',1,'0','0');",
        )
        .unwrap();
        db
    }
    fn at(seq: i64) -> String {
        format!("2026-11-17T10:00:00.{seq:06}+00:00")
    }
    /// One coordinator (or, with attempt `xa_w`, worker) chunk at `seq` of message `message`.
    fn say(db: &Connection, seq: i64, attempt: &str, message: i64, text: &str) {
        db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,message_id,created_at) VALUES ('s',?,'agent_message',?,?,?)",
            rusqlite::params![seq, text, format!("{attempt}:m{message}"), at(seq)]).unwrap();
    }
    fn step(db: &Connection, seq: i64, call: &str, status: &str) {
        let update = json!({"sessionUpdate":"tool_call_update","toolCallId":call,"title":"Read a.ts","kind":"read","status":status,"rawInput":{"path":"a.ts"}});
        db.execute(
            "INSERT INTO host_updates VALUES ('s',?,?,?)",
            rusqlite::params![seq, update.to_string(), at(seq)],
        )
        .unwrap();
    }
    /// `n` coordinator rows from `from` on: chunks of a message every ten rows, with a tool update every third row.
    fn grow(db: &Connection, from: i64, n: i64) {
        let tx = db.unchecked_transaction().unwrap();
        for seq in from..from + n {
            if seq % 3 == 2 {
                step(
                    &tx,
                    seq,
                    &format!("xa_c:call{}", seq / 30),
                    if seq % 2 == 0 {
                        "in_progress"
                    } else {
                        "completed"
                    },
                )
            } else {
                say(&tx, seq, "xa_c", seq / 10, &format!("part {seq} "))
            }
        }
        tx.commit().unwrap();
    }
    /// Pages from `cursor` until none is left: the rows in order and the forward cursor of the last page.
    fn follow(
        db: &Connection,
        limit: u16,
        mut cursor: Option<String>,
    ) -> CoreResult<(Vec<HistoryRow>, String)> {
        let mut rows = Vec::new();
        loop {
            let p = page(
                db,
                "p",
                &Page {
                    target: "t".into(),
                    cursor,
                    limit,
                },
            )?;
            assert!(p.items.len() <= limit as usize);
            rows.extend(p.items);
            cursor = p.next_cursor;
            if cursor.is_none() {
                return Ok((
                    rows,
                    p.tail_cursor
                        .expect("a history page names its forward cursor"),
                ));
            }
        }
    }
    fn ids(rows: &[HistoryRow]) -> Vec<String> {
        rows.iter().map(|r| r.history_id.clone()).collect()
    }
    fn said(rows: &[HistoryRow]) -> Vec<String> {
        rows.iter()
            .filter(|r| r.kind == "assistant")
            .map(|r| r.text.clone().unwrap_or_default())
            .collect()
    }

    /// A forward read returns exactly the rows written after the position it continues from, in order and paged, and a
    /// first read followed by forward reads holds what one whole read holds: nothing twice, nothing missing.
    #[test]
    fn a_forward_read_returns_exactly_the_rows_after_its_position() {
        let db = session();
        grow(&db, 0, 30);
        let (first, tail) = follow(&db, 7, None).unwrap();
        // Nothing new: an empty page that stays where it was.
        let (none, same) = follow(&db, 7, Some(tail.clone())).unwrap();
        assert!(none.is_empty());
        assert_eq!(same, tail);
        grow(&db, 30, 25);
        let (more, tail) = follow(&db, 7, Some(tail)).unwrap();
        assert!(
            more.iter().all(|r| r.sequence.get() > 30),
            "{:?}",
            ids(&more)
        );
        let (whole, whole_tail) = follow(&db, 7, None).unwrap();
        assert_eq!([ids(&first), ids(&more)].concat(), ids(&whole));
        assert_eq!(tail, whole_tail);
        // A row's later update reads as its own row of the same call, so the client merges it into the step on screen.
        assert!(more.iter().any(|r| r.kind == "tool"));
    }

    /// Every statement a forward read runs seeks the session's position through an index: no scan of the session, no
    /// sort of it.
    #[test]
    fn a_forward_reads_statements_seek_through_the_session_indexes() {
        let db = session();
        grow(&db, 0, 50);
        for sql in [TRANSCRIPT_SQL, UPDATES_SQL, ANCHOR_SQL, MAXIMUM_SQL] {
            let mut statement = db.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let count = statement.parameter_count();
            let values: Vec<rusqlite::types::Value> = (0..count)
                .map(|i| {
                    if i == 0 {
                        "s".to_owned().into()
                    } else {
                        10i64.into()
                    }
                })
                .collect();
            let plan = statement
                .query_map(rusqlite::params_from_iter(values), |r| {
                    r.get::<_, String>(3)
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(
                plan.iter().any(|s| s.starts_with("SEARCH host_")
                    && s.contains("INDEX")
                    && s.contains("session_id=?")),
                "{sql}: {plan:?}"
            );
            assert!(
                !plan
                    .iter()
                    .any(|s| s.starts_with("SCAN host_") || s.contains("TEMP B-TREE")),
                "{sql}: {plan:?}"
            );
        }
    }

    /// SQLite virtual-machine steps spent by `read`, counted by a progress callback on every step.
    fn steps<T>(db: &Connection, read: impl FnOnce() -> T) -> (u64, T) {
        use std::sync::atomic::{AtomicU64, Ordering};
        extern "C" fn tick(counter: *mut std::ffi::c_void) -> std::ffi::c_int {
            // SAFETY: the pointer is the `AtomicU64` below, alive until the callback is removed.
            unsafe { &*(counter as *const AtomicU64) }.fetch_add(1, Ordering::Relaxed);
            0
        }
        let counter = AtomicU64::new(0);
        // SAFETY: the handle belongs to `db`, used on this thread only; the callback is removed before `counter` drops.
        unsafe {
            rusqlite::ffi::sqlite3_progress_handler(
                db.handle(),
                1,
                Some(tick),
                &counter as *const _ as *mut _,
            )
        };
        let value = read();
        unsafe {
            rusqlite::ffi::sqlite3_progress_handler(db.handle(), 0, None, std::ptr::null_mut())
        };
        (counter.load(Ordering::Relaxed), value)
    }

    /// A forward read of the same ten new rows costs the same on a conversation of 200 rows as on one of 10,000: its work
    /// does not grow with the history before its position.
    #[test]
    fn a_forward_reads_cost_does_not_grow_with_the_history_before_it() {
        let mut cost = Vec::new();
        for n in [200, 10_000] {
            let db = session();
            grow(&db, 0, n);
            let (_, tail) = follow(&db, 100, None).unwrap();
            grow(&db, n, 10);
            let started = std::time::Instant::now();
            let (vm, read) = steps(&db, || follow(&db, 100, Some(tail.clone())).unwrap());
            eprintln!(
                "forward_read rows={n} new_rows={} vm_steps={vm} micros={}",
                read.0.len(),
                started.elapsed().as_micros()
            );
            assert_eq!(read.0.len(), 10);
            cost.push(vm);
        }
        // Deeper index pages cost a few more steps per seek; nothing proportional to the 50 times longer prefix.
        assert!(cost[1] <= cost[0] + cost[0] / 4, "{cost:?}");
    }

    /// A worker's rows in the coordinator's session are left out of a forward read, and its position still moves past
    /// them: the next read starts after them, not at them.
    #[test]
    fn a_forward_read_moves_past_a_workers_rows() {
        let db = session();
        grow(&db, 0, 10);
        let (_, tail) = follow(&db, 100, None).unwrap();
        for seq in 10..160 {
            say(&db, seq, "xa_w", 1, "worker");
        }
        let (rows, tail) = follow(&db, 100, Some(tail)).unwrap();
        assert!(rows.is_empty());
        assert_eq!(tail.split('.').nth(2), Some("159"));
        say(&db, 160, "xa_c", 99, "coordinator");
        let (vm, (rows, _)) = steps(&db, || follow(&db, 100, Some(tail)).unwrap());
        assert_eq!(said(&rows), ["\n\ncoordinator"]);
        // Only the one new row is read, never the 150 worker rows behind it again.
        assert!(vm < 2_000, "{vm}");
    }

    /// A history whose rows are gone, or were written again under the same positions, is refused as reset, and a cursor of
    /// another session, page size or project is refused as invalid: the reader reads the whole history again.
    #[test]
    fn a_forward_read_of_a_history_that_changed_underneath_is_refused() {
        let code = |db: &Connection, cursor: &str| {
            page(
                db,
                "p",
                &Page {
                    target: "t".into(),
                    cursor: Some(cursor.into()),
                    limit: 100,
                },
            )
            .unwrap_err()
            .code()
        };
        let db = session();
        grow(&db, 0, 20);
        let (_, tail) = follow(&db, 100, None).unwrap();
        db.execute_batch(
            "DELETE FROM host_transcript WHERE seq>=15; DELETE FROM host_updates WHERE seq>=15;",
        )
        .unwrap();
        assert_eq!(code(&db, &tail), "history_reset");
        grow(&db, 15, 5);
        db.execute(
            "UPDATE host_transcript SET created_at='2026-11-18T00:00:00.000000+00:00' WHERE seq=19",
            [],
        )
        .unwrap();
        assert_eq!(code(&db, &tail), "history_reset");
        let other = tail.replacen("nt1.", "nt1.0", 1);
        assert_eq!(code(&db, &other), "invalid_cursor");
        let (_, tail) = follow(&db, 100, None).unwrap();
        assert!(follow(&db, 100, Some(tail.clone())).is_ok());
        db.execute(
            "UPDATE workspace_executions SET acp_session_id='replacement'",
            [],
        )
        .unwrap();
        assert_eq!(code(&db, &tail), "invalid_cursor");
    }

    /// One backward page from `cursor` (none: the newest rows).
    fn back_page(
        db: &Connection,
        limit: u16,
        cursor: Option<String>,
    ) -> CoreResult<ReadPage<HistoryRow>> {
        earlier(
            db,
            "p",
            &Page {
                target: "t".into(),
                cursor,
                limit,
            },
        )
    }
    /// The whole history read backwards: each page as returned (oldest page first) and the first page's forward cursor.
    fn backwards(db: &Connection, limit: u16) -> (Vec<Vec<HistoryRow>>, String) {
        let first = back_page(db, limit, None).unwrap();
        let tail = first
            .tail_cursor
            .clone()
            .expect("the newest page names its forward cursor");
        let mut pages = Vec::new();
        let mut page = first;
        loop {
            assert!(page.items.len() <= limit as usize);
            assert!(page.next_cursor.is_none());
            let prev = page.prev_cursor.clone();
            pages.insert(0, page.items);
            let Some(cursor) = prev else {
                return (pages, tail);
            };
            page = back_page(db, limit, Some(cursor)).unwrap();
            assert!(
                page.tail_cursor.is_none(),
                "only the newest page names a forward cursor"
            );
        }
    }

    /// Read backwards, the newest rows come first and in source order, each page holds at most `limit` source rows, and
    /// the pages put together (transcript and update rows merged by position, worker rows and hidden updates left out)
    /// hold exactly what one forward read holds; the newest page's forward cursor is the forward read's own.
    #[test]
    fn a_backward_read_puts_together_exactly_the_forward_read() {
        let db = session();
        grow(&db, 0, 230);
        for seq in 230..260 {
            say(&db, seq, "xa_w", 7, "worker");
        }
        grow(&db, 260, 40);
        for limit in [100, 7, 1] {
            let (whole, whole_tail) = follow(&db, limit, None).unwrap();
            let (pages, tail) = backwards(&db, limit);
            for page in &pages {
                assert!(page
                    .windows(2)
                    .all(|w| w[0].sequence.get() < w[1].sequence.get()));
            }
            let joined: Vec<HistoryRow> = pages.concat();
            assert_eq!(ids(&joined), ids(&whole), "limit {limit}");
            let text = |rows: &[HistoryRow]| {
                rows.iter()
                    .map(|r| {
                        format!(
                            "{}|{}|{:?}",
                            r.kind,
                            r.text.clone().unwrap_or_default(),
                            r.tool_status
                        )
                    })
                    .collect::<Vec<_>>()
            };
            assert_eq!(text(&joined), text(&whole), "limit {limit}");
            assert_eq!(tail, whole_tail, "limit {limit}");
            assert!(
                joined.iter().any(|r| r.kind == "tool")
                    && joined.iter().any(|r| r.kind == "assistant")
            );
            assert!(!said(&joined).iter().any(|t| t.contains("worker")));
        }
        // The newest page is the end of the conversation: its last row is the newest display row.
        let newest = back_page(&db, 100, None).unwrap();
        assert_eq!(
            newest.items.last().unwrap().history_id,
            follow(&db, 100, None).unwrap().0.last().unwrap().history_id
        );
        assert!(newest.prev_cursor.is_some());
    }

    /// A long call's merged step, a malformed update and the store's own schema read the same backwards as forwards.
    #[test]
    fn a_backward_read_of_the_retained_store_matches_the_forward_read() {
        let db = retained(120);
        for limit in [100, 9] {
            let (pages, _) = backwards(&db, limit);
            let joined = pages.concat();
            let forward = texts(&db, limit);
            assert_eq!(
                joined
                    .iter()
                    .map(|r| format!("{}|{}", r.kind, r.text.clone().unwrap_or_default()))
                    .collect::<Vec<_>>(),
                forward
            );
        }
    }

    /// After the newest page, later rows arrive through its forward cursor exactly as after a forward read; an empty
    /// conversation reads as one empty page with no earlier rows.
    #[test]
    fn a_backward_reads_forward_cursor_continues_with_the_new_rows() {
        let db = session();
        let empty = back_page(&db, 100, None).unwrap();
        assert!(empty.items.is_empty() && empty.prev_cursor.is_none());
        grow(&db, 0, 120);
        let (_, tail) = backwards(&db, 100);
        grow(&db, 120, 15);
        let (more, _) = follow(&db, 100, Some(tail)).unwrap();
        assert!(!more.is_empty());
        assert!(
            more.iter().all(|r| r.sequence.get() > 120),
            "{:?}",
            ids(&more)
        );
    }

    /// A backward cursor is bound like a forward one: another page size, session or project is `invalid_cursor`, a
    /// forward cursor is not a backward one (nor the reverse), and rows gone or written again at its position are
    /// `history_reset`.
    #[test]
    fn a_backward_cursor_is_bound_and_checked() {
        let db = session();
        grow(&db, 0, 60);
        let prev = back_page(&db, 10, None).unwrap().prev_cursor.unwrap();
        let code = |db: &Connection, limit: u16, cursor: &str| {
            back_page(db, limit, Some(cursor.into()))
                .unwrap_err()
                .code()
        };
        assert_eq!(code(&db, 20, &prev), "invalid_cursor");
        assert_eq!(
            code(&db, 10, &prev.replacen("np1.", "np1.0", 1)),
            "invalid_cursor"
        );
        assert_eq!(
            code(&db, 10, &prev.replacen("np1.", "nh2.", 1)),
            "invalid_cursor"
        );
        let (_, tail) = follow(&db, 10, None).unwrap();
        assert_eq!(code(&db, 10, &tail), "invalid_cursor");
        let forward = page(
            &db,
            "p",
            &Page {
                target: "t".into(),
                cursor: Some(prev.clone()),
                limit: 10,
            },
        )
        .unwrap_err()
        .code();
        assert_eq!(forward, "invalid_cursor");
        let position: i64 = prev.split('.').nth(2).unwrap().parse().unwrap();
        for bad in [
            format!("{}", position + 1000),
            "-1".into(),
            "0".into(),
            format!("0{position}"),
        ] {
            let mut fields: Vec<String> = prev.split('.').map(str::to_owned).collect();
            fields[2] = bad;
            let c = code(&db, 10, &fields.join("."));
            assert!(c == "invalid_cursor" || c == "history_reset", "{c}");
        }
        assert!(back_page(&db, 10, Some(prev.clone())).is_ok());
        db.execute(
            "UPDATE host_transcript SET created_at='2026-11-18T00:00:00.000000+00:00' WHERE seq=?",
            [position],
        )
        .unwrap();
        db.execute(
            "UPDATE host_updates SET created_at='2026-11-18T00:00:00.000000+00:00' WHERE seq=?",
            [position],
        )
        .unwrap();
        assert_eq!(code(&db, 10, &prev), "history_reset");
        let prev = back_page(&db, 10, None).unwrap().prev_cursor.unwrap();
        db.execute_batch(
            "DELETE FROM host_transcript WHERE seq>=40; DELETE FROM host_updates WHERE seq>=40;",
        )
        .unwrap();
        assert_eq!(code(&db, 10, &prev), "history_reset");
        let prev = back_page(&db, 10, None).unwrap().prev_cursor.unwrap();
        db.execute(
            "UPDATE workspace_executions SET acp_session_id='replacement'",
            [],
        )
        .unwrap();
        assert_eq!(code(&db, 10, &prev), "invalid_cursor");
    }

    /// A backward read seeks its position through each table's (session, seq) index, and the newest page of a 10,000-row
    /// conversation costs about what it costs on a 200-row one.
    #[test]
    fn a_backward_read_seeks_and_does_not_grow_with_the_history() {
        let db = session();
        grow(&db, 0, 50);
        for sql in [TRANSCRIPT_BEFORE_SQL, UPDATES_BEFORE_SQL] {
            let plan = db
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap()
                .query_map(rusqlite::params!["s", 40, -1, 10], |r| {
                    r.get::<_, String>(3)
                })
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap();
            assert!(
                plan.iter().any(|s| s.starts_with("SEARCH host_")
                    && s.contains("INDEX")
                    && s.contains("session_id=?")),
                "{sql}: {plan:?}"
            );
            assert!(
                !plan
                    .iter()
                    .any(|s| s.starts_with("SCAN host_") || s.contains("TEMP B-TREE")),
                "{sql}: {plan:?}"
            );
        }
        let mut cost = Vec::new();
        for n in [200, 10_000] {
            let db = session();
            grow(&db, 0, n);
            let (vm, page) = steps(&db, || back_page(&db, 100, None).unwrap());
            eprintln!(
                "backward_read rows={n} items={} vm_steps={vm}",
                page.items.len()
            );
            assert!(page.prev_cursor.is_some());
            cost.push(vm);
        }
        assert!(cost[1] <= cost[0] + cost[0] / 4, "{cost:?}");
    }
}
