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
fn output(v: &Value) -> String {
    if let Some(s) = v.as_str() {
        return s.into();
    }
    if let Some(a) = v.as_array() {
        return a
            .iter()
            .map(output)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
    }
    for k in ["output", "text", "stdout", "formatted_output", "content"] {
        if let Some(s) = v.get(k) {
            return output(s);
        }
    }
    String::new()
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
        let text = output(
            update
                .get("rawOutput")
                .or_else(|| update.get("raw_output"))
                .or_else(|| update.get("output"))
                .unwrap_or(&Value::Null),
        );
        d["output"] = json!(if text.is_empty() {
            output(&update["content"])
        } else {
            text
        });
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
fn display(
    db: &Connection,
    session: &str,
    seq: i64,
    raw: &str,
) -> CoreResult<Option<(String, String, Option<String>)>> {
    let mut u: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return Ok(Some(("unknown".into(), String::new(), None))),
    };
    let kind = field(&u, "sessionUpdate").unwrap_or("");
    if !matches!(kind, "plan" | "tool_call" | "tool_call_update")
        || hidden(field(&u, "title").unwrap_or(""))
    {
        return Ok(None);
    }
    if let Some(call) = field(&u, "toolCallId").map(str::to_owned) {
        let prior=db.prepare("SELECT update_json FROM host_updates WHERE session_id=? AND seq<=? AND json_valid(update_json) AND json_extract(update_json,'$.toolCallId')=? ORDER BY seq DESC LIMIT 100")?
            .query_map(rusqlite::params![session,seq,call],|r|r.get::<_,String>(0))?.collect::<Result<Vec<_>,_>>()?;
        let mut merged = json!({});
        if prior.len() >= 100 {
            if let Some(initial)=row(db,"SELECT update_json FROM host_updates WHERE session_id=? AND seq<=? AND json_valid(update_json) AND json_extract(update_json,'$.toolCallId')=? ORDER BY seq LIMIT 1",rusqlite::params![session,seq,call])? {
                merge(&mut merged,&serde_json::from_str::<Value>(field(&initial,"update_json").unwrap_or("{}"))?);
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
/// The break a reader sees between two messages whose streamed rows follow each
/// other. Display history records a reply one chunk per row and the client joins
/// adjacent chunks back into one message, so without it the last sentence of one
/// message (say the coordinator's) ran straight into the next (a worker's).
const PARAGRAPH: &str = "\n\n";
/// Whether this agent chunk begins a different message than the agent chunk
/// right before it: both carry an identity and the identities differ. A chunk
/// after a user message, or one with no recorded identity, is left as written.
fn starts_message(db: &Connection, session: &str, seq: i64, message: Option<&str>) -> CoreResult<bool> {
    let Some(message) = message else { return Ok(false) };
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
/// with no delegation record) stays the coordinator's, so an older history reads as it always did.
fn from_worker(db: &Connection, task: &str, message: Option<&str>) -> CoreResult<bool> {
    let Some(attempt) = message.and_then(|m| m.split_once(':')).map(|(a, _)| a).filter(|a| !a.is_empty()) else { return Ok(false) };
    let exists: bool = db.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='delegation_provisioning')", [], |r| r.get(0))?;
    if !exists {
        return Ok(false);
    }
    let roles = db
        .prepare("SELECT DISTINCT role_id FROM delegation_provisioning WHERE task_id=? AND attempt_id=?")?
        .query_map(rusqlite::params![task, attempt], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(!roles.is_empty() && roles.iter().all(|r| r != "orchestrator"))
}
pub(super) fn page(db: &Connection, project: &str, p: &Page) -> CoreResult<ReadPage<HistoryRow>> {
    let e=row(db,"SELECT e.acp_session_id FROM workspace_executions e JOIN workspace_tasks t USING(task_id) WHERE t.project_id=? AND t.task_id=?", [project,&p.target])?.ok_or(CoreError::Refused("not_found"))?;
    let session = field(&e, "acp_session_id").unwrap_or("");
    let binding = crate::digest(
        serde_json::to_string(&json!([project, p.target, session, p.limit]))?.as_bytes(),
    );
    let sql="SELECT seq,kind,text,NULL AS update_json,created_at,message_id FROM host_transcript WHERE session_id=? UNION ALL SELECT seq,'update',NULL,update_json,created_at,NULL FROM host_updates WHERE session_id=?";
    let maximum: i64 = db.query_row(
        &format!("SELECT coalesce(max(seq),-1) FROM ({sql})"),
        [session, session],
        |r| r.get(0),
    )?;
    let (high, after) = if let Some(c) = &p.cursor {
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
    let rows = db
        .prepare(&format!(
            "SELECT * FROM ({sql}) WHERE seq>? AND seq<=? ORDER BY seq LIMIT ?"
        ))?
        .query_map(
            rusqlite::params![session, session, after, high, p.limit + 1],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, Option<String>>(2)?,
                    r.get::<_, Option<String>>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                ))
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    let mut items = Vec::new();
    let mut consumed = after;
    let mut bytes = 256;
    for (seq, kind, text, update, at, message) in rows.iter().take(p.limit as usize) {
        // A worker's or reviewer's narration is not the task's conversation: it is read on that worker's own page. A row
        // left out is consumed like a hidden update, so paging never stalls on it.
        if kind == "agent_message" && from_worker(db, &p.target, message.as_deref())? {
            consumed = *seq;
            continue;
        }
        // A worker's tool calls (call identity `{attempt}:{call}`) belong to its own page, like its narration.
        if kind == "update" {
            let call = update
                .as_deref()
                .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                .and_then(|u| field(&u, "toolCallId").map(str::to_owned));
            if from_worker(db, &p.target, call.as_deref())? {
                consumed = *seq;
                continue;
            }
        }
        let (kind, text, status) = if kind == "update" {
            let Some((kind, text, status)) =
                display(db, session, *seq, update.as_deref().unwrap_or(""))?
            else {
                consumed = *seq;
                continue;
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
                            starts_message(db, session, *seq, message.as_deref())
                                .map(|new| if new { format!("{PARAGRAPH}{t}") } else { t.clone() })
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
        let size = serde_json::to_vec(&item)?.len();
        if bytes + size > 58_000 && !items.is_empty() {
            break;
        }
        if size > 58_000 {
            item.text = None;
            item.availability = "unavailable".into();
            item.reason = Some("content_exceeds_page_limit".into());
        }
        bytes += serde_json::to_vec(&item)?.len() + 1;
        items.push(item);
        consumed = *seq;
    }
    let next_cursor = if !rows.is_empty() && consumed < high {
        Some(format!("nh2.{binding}.{high}.{consumed}"))
    } else {
        None
    };
    Ok(ReadPage {
        items,
        next_cursor,
        high_watermark: ver(high + 1)?,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(
            "CREATE TABLE workspace_tasks(task_id TEXT, project_id TEXT);
             CREATE TABLE workspace_executions(execution_id TEXT, task_id TEXT, acp_session_id TEXT);
             CREATE TABLE host_transcript(session_id TEXT, seq INTEGER, kind TEXT, text TEXT, message_id TEXT, created_at TEXT);
             CREATE TABLE host_updates(session_id TEXT, seq INTEGER, update_json TEXT, created_at TEXT);
             CREATE TABLE delegation_provisioning(task_id TEXT, attempt_id TEXT, role_id TEXT);
             INSERT INTO delegation_provisioning VALUES ('t','xa_c','orchestrator'),('t','xa_w','worker-mobile-dev');
             INSERT INTO workspace_tasks VALUES ('t','p');
             INSERT INTO workspace_executions VALUES ('e','t','s');",
        )
        .unwrap();
        db
    }
    fn texts(db: &Connection, limit: u16) -> Vec<String> {
        let mut out = Vec::new();
        let mut cursor = None;
        loop {
            let p = page(db, "p", &Page { target: "t".into(), cursor, limit }).unwrap();
            out.extend(p.items.into_iter().map(|r| format!("{}|{}", r.kind, r.text.unwrap_or_default())));
            cursor = p.next_cursor;
            if cursor.is_none() {
                return out;
            }
        }
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
            db.execute("INSERT INTO host_transcript VALUES ('s',?,?,?,?,'now')", rusqlite::params![seq, kind, text, id]).unwrap();
        }
        db.execute("INSERT INTO host_updates VALUES ('s',5,?,'now')", [json!({"sessionUpdate":"tool_call","toolCallId":"c","title":"Read a.ts","kind":"read","rawInput":{"path":"a.ts"}}).to_string()]).unwrap();
        let expected = [
            "user|Index it", "assistant|Индексирую `ARCH", "assistant|ITECTURE.md`.",
            "user|Next", "assistant|Unidentified", "assistant| chunk.",
        ];
        let all = texts(&db, 100);
        assert_eq!(all.iter().filter(|t| !t.starts_with("tool|")).cloned().collect::<Vec<_>>(), expected);
        // A page boundary between the two messages keeps the break.
        assert_eq!(texts(&db, 3).iter().filter(|t| !t.starts_with("tool|")).cloned().collect::<Vec<_>>(), expected);
    }

    /// A worker's narration is left out of the task history. A history written before attempts were recorded (no delegation table, or an attempt the task never recorded) keeps
    /// every message as the coordinator's.
    #[test]
    fn a_message_is_a_workers_only_when_its_attempt_is_a_recorded_non_orchestrator_role() {
        let db = fixture();
        for (seq, id) in [(0, Some("xa_w:m")), (1, Some("xa_unknown:m")), (2, Some("synthesis:s:2")), (3, None)] {
            db.execute("INSERT INTO host_transcript VALUES ('s',?,'agent_message','t',?,'now')", rusqlite::params![seq, id]).unwrap();
        }
        let kinds = |db: &Connection| texts(db, 100).into_iter().map(|t| t.split('|').next().unwrap().to_owned()).collect::<Vec<_>>();
        assert_eq!(kinds(&db), ["assistant", "assistant", "assistant"]);
        db.execute_batch("DROP TABLE delegation_provisioning").unwrap();
        assert_eq!(kinds(&db), ["assistant"; 4]);
    }

    /// Conversation shows only the coordinator: tool calls of a worker attempt are left out, the coordinator's stay.
    #[test]
    fn conversation_omits_worker_tool_calls_and_keeps_the_coordinators() {
        let db = fixture();
        for (seq, call) in [(0, "xa_c:one"), (1, "xa_w:two"), (2, "xa_c:three"), (3, "xa_w:four")] {
            let update = json!({"sessionUpdate":"tool_call","toolCallId":call,"title":"Read a.ts","kind":"read","rawInput":{"path":"a.ts"}});
            db.execute("INSERT INTO host_updates VALUES ('s',?,?,'now')", rusqlite::params![seq, update.to_string()]).unwrap();
        }
        let calls = texts(&db, 2)
            .iter()
            .filter_map(|t| t.strip_prefix("tool|Workspace step: ").map(|s| serde_json::from_str::<Value>(s).unwrap()["callId"].as_str().unwrap().to_owned()))
            .collect::<Vec<_>>();
        assert_eq!(calls, ["xa_c:one", "xa_c:three"]);
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
            .filter_map(|t| t.strip_prefix("tool|Workspace step: ").map(|s| serde_json::from_str(s).unwrap()))
            .collect();
        let last = |id: &str| steps.iter().rev().find(|s| s["callId"] == id).unwrap().clone();
        assert_eq!(last("plan")["failure"], "No plan was recorded (plan_invalid).");
        assert_eq!(last("plan")["status"], "failed");
        assert!(last("draft").get("failure").is_none());
    }
}
