//! Display-only native ACP activity. Never tool authority or engine context.
use cortex_core::{channels::Credential, store::executor::StoreExecutor, CoreError, CoreResult};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::sync::Arc;

fn clipped(s: &str, limit: usize) -> String {
    let mut chars = s.chars();
    let mut text: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        text.push_str("\n[Content clipped]");
    }
    text
}
/// Credential-redacted and bounded: a selected input is display text, never authority.
fn redacted(s: &str, limit: usize) -> String {
    crate::machine::redact_display_text(&clipped(s, limit))
}
const PATHS: [&str; 4] = ["file_path", "filePath", "path", "absolutePath"];
const QUERIES: [&str; 3] = ["query", "pattern", "q"];
const REFS: [&str; 5] = ["report_id", "report_ref", "draft_id", "source_ref", "skill"];
const EDIT_TEXT: [&str; 7] = [
    "old_string",
    "oldString",
    "oldText",
    "new_string",
    "newString",
    "newText",
    "content",
];
/// Report, draft and result bodies are private: never an edit diff in display history.
const PRIVATE: [&str; 4] = [
    "create_draft",
    "draft_write",
    "write_report",
    "result_submit",
];
const EDITS: [&str; 7] = [
    "project_edit",
    "project_write",
    "outside_write",
    "edit",
    "write",
    "multiedit",
    "apply_patch",
];
const KINDS: [&str; 10] = [
    "read",
    "edit",
    "delete",
    "move",
    "search",
    "execute",
    "think",
    "fetch",
    "switch_mode",
    "other",
];
fn operation(title: &str) -> String {
    let name = title.rsplit("__").next().unwrap_or("");
    name.rsplit('.').next().unwrap_or("").trim().to_lowercase()
}
/// The few arguments a row names (path, command, query, skill or report) and,
/// for a non-private edit, its replacement text. Every other key is dropped.
fn arguments(raw: &Value, edit: bool) -> Value {
    let mut out = json!({});
    let Some(raw) = raw.as_object() else {
        return out;
    };
    let strings = |v: &Value| {
        v.as_array()
            .filter(|a| a.len() <= 256 && a.iter().all(Value::is_string))
            .map(|a| {
                a.iter()
                    .map(|s| json!(redacted(s.as_str().unwrap_or(""), 4000)))
                    .collect::<Vec<_>>()
            })
    };
    for (key, value) in raw {
        let key = key.as_str();
        let picked = if PATHS.contains(&key) || REFS.contains(&key) {
            value.as_str().map(|s| json!(clipped(s, 4096)))
        } else if matches!(key, "command" | "cmd" | "argv") || QUERIES.contains(&key) {
            value
                .as_str()
                .map(|s| json!(redacted(s, 4000)))
                .or_else(|| strings(value).map(Value::from))
        } else if edit && EDIT_TEXT.contains(&key) {
            value.as_str().map(|s| json!(redacted(s, 16_000)))
        } else if edit && key == "edits" {
            value.as_array().map(|a| {
                json!(a
                    .iter()
                    .take(64)
                    .filter_map(Value::as_object)
                    .map(|e| {
                        let mut one = json!({});
                        for k in ["old_string", "new_string"] {
                            if let Some(s) = e.get(k).and_then(Value::as_str) {
                                one[k] = json!(redacted(s, 16_000));
                            }
                        }
                        one
                    })
                    .collect::<Vec<_>>())
            })
        } else {
            None
        };
        if let Some(v) = picked {
            out[key] = v;
        }
    }
    out
}
/// The selected input in the shape the engine sent it: plain arguments, a JSON
/// string of them, an `{input}` wrapper or an MCP `{server, tool, arguments}` call.
fn input(raw: &Value, edit: bool) -> Option<Value> {
    let parsed;
    let raw = match raw.as_str() {
        Some(s) => {
            parsed = serde_json::from_str::<Value>(s).ok()?;
            &parsed
        }
        None => raw,
    };
    let out = if let (Some(server), Some(tool), true) = (
        raw["server"].as_str(),
        raw["tool"].as_str(),
        raw["arguments"].is_object(),
    ) {
        json!({"server":clipped(server, 256),"tool":clipped(tool, 256),"arguments":arguments(&raw["arguments"], edit)})
    } else if raw["input"].is_object() {
        json!({"input":arguments(&raw["input"], edit)})
    } else {
        arguments(raw, edit)
    };
    let empty = out.as_object().is_some_and(|m| m.is_empty())
        || out
            .get("input")
            .is_some_and(|v| v.as_object().is_some_and(|m| m.is_empty()));
    (!empty).then_some(out)
}
/// A code fence an engine wraps an error text in (claude-agent-acp sends
/// "```\n…\n```"), removed so the text reads as written.
fn unfenced(text: &str) -> &str {
    let t = text.trim();
    let Some(rest) = t.strip_prefix("```") else {
        return t;
    };
    let body = rest.split_once('\n').map_or("", |(_, b)| b);
    body.trim_end().strip_suffix("```").unwrap_or(body).trim()
}
/// Why a call was refused, in the runtime's own words: the `message` of a
/// `{code, message}` refusal (`content_refusal`), else, for a Cortex tool only,
/// its error text. An engine tool's output is never retained. Bounded, redacted.
fn failure(update: &Value, cortex: bool) -> Option<String> {
    let raw = &update["rawOutput"];
    let refused = update["status"] == "failed"
        || raw["isError"] == true
        || raw["result"]["isError"] == true
        || raw["error"].is_string()
        || raw["error"]["message"].is_string();
    if !refused {
        return None;
    }
    let mut texts: Vec<&str> = Vec::new();
    for blocks in [
        &update["content"],
        &raw["content"],
        &raw["result"]["content"],
    ] {
        for b in blocks
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .take(16)
        {
            texts.extend(
                [&b["content"]["text"], &b["text"]]
                    .iter()
                    .filter_map(|v| v.as_str()),
            );
        }
    }
    texts.extend(
        [raw, &raw["error"], &raw["error"]["message"]]
            .iter()
            .filter_map(|v| v.as_str()),
    );
    let structured = [
        &raw["structuredContent"],
        &raw["result"]["structuredContent"],
    ]
    .into_iter()
    .find_map(|v| v["message"].as_str().map(str::to_owned));
    let message = structured
        .or_else(|| {
            texts.iter().find_map(|t| {
                let v: Value = serde_json::from_str(unfenced(t)).ok()?;
                v["message"]
                    .as_str()
                    .filter(|m| !m.trim().is_empty())
                    .map(str::to_owned)
            })
        })
        .or_else(|| {
            cortex
                .then(|| texts.iter().map(|t| unfenced(t)).find(|t| !t.is_empty()))
                .flatten()
                .map(str::to_owned)
        })?;
    Some(redacted(message.trim(), 1000))
}
/// What a row shows of one tool update: its kind, input selections, file
/// locations and, for a non-private edit, the engine's own diff blocks.
/// `prior` is the title and kind of the call's retained announcement: an engine
/// (claude-agent-acp) sends the input and result in later untitled updates.
fn selected(update: &Value, safe: &mut Value, prior: &Value) {
    let own = update["kind"].as_str().filter(|k| KINDS.contains(k));
    if let Some(kind) = own {
        safe["kind"] = json!(kind);
    }
    let kind = own.or_else(|| prior["kind"].as_str().filter(|k| KINDS.contains(k)));
    // Edit text and diffs need the call's title, its own or the one retained for
    // it: an untitled call could belong to a private report or draft call.
    let title = update["title"].as_str().or_else(|| prior["title"].as_str());
    let operation = operation(title.unwrap_or(""));
    let private = title.is_none() || PRIVATE.contains(&operation.as_str());
    let edit = !private && (kind == Some("edit") || EDITS.contains(&operation.as_str()));
    if !private {
        let cortex =
            title.is_some_and(|t| t.contains("mcp__cortex__") || t.contains("mcp.cortex."));
        if let Some(reason) = failure(update, cortex) {
            safe["failure"] = json!(reason);
        }
    }
    if let Some(raw) = update.get("rawInput").and_then(|v| input(v, edit)) {
        safe["rawInput"] = raw;
    }
    if let Some(a) = update["locations"].as_array() {
        let locations = a
            .iter()
            .take(16)
            .filter_map(|l| {
                let path = l["path"].as_str().filter(|p| !p.is_empty())?;
                let mut one = json!({"path":clipped(path, 4096)});
                if let Some(line) = l["line"].as_u64() {
                    one["line"] = json!(line);
                }
                Some(one)
            })
            .collect::<Vec<_>>();
        if !locations.is_empty() {
            safe["locations"] = json!(locations);
        }
    }
    if private {
        return;
    }
    if let Some(a) = update["content"].as_array() {
        // Only diff blocks: text and terminal content (read bodies, outputs) stay out.
        let diffs = a
            .iter()
            .filter(|b| b["type"] == "diff")
            .take(16)
            .filter_map(|b| {
                let path = b["path"].as_str()?;
                let mut one = json!({"type":"diff","path":clipped(path, 4096)});
                for k in ["oldText", "newText"] {
                    if let Some(s) = b[k].as_str() {
                        one[k] = json!(redacted(s, 16_000));
                    }
                }
                Some(one)
            })
            .collect::<Vec<_>>();
        if !diffs.is_empty() {
            safe["content"] = json!(diffs);
        }
    }
}
enum Display {
    /// Text plus the engine's own message identity when it supplied one.
    Message(String, Option<String>),
    Update(Value),
}
fn display(
    params: &Value,
    session: &str,
    attempt: &str,
    role: &str,
    prior: &Value,
) -> CoreResult<Option<Display>> {
    if params["sessionId"] != session {
        return Err(CoreError::Refused("native_activity_session_mismatch"));
    }
    let update = &params["update"];
    Ok(match update["sessionUpdate"].as_str() {
        Some("agent_message_chunk") if update["content"]["type"] == "text" => {
            update["content"]["text"].as_str().map(|s| {
                let id = update["messageId"]
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= 256)
                    .map(str::to_owned);
                Display::Message(clipped(s, 16_000), id)
            })
        }
        Some(kind @ ("tool_call" | "tool_call_update")) => {
            let Some(call) = update["toolCallId"]
                .as_str()
                .filter(|s| !s.is_empty() && s.len() <= 512)
            else {
                return Ok(None);
            };
            let mut safe = json!({"sessionUpdate":kind,"toolCallId":format!("{attempt}:{call}")});
            if let Some(title) = update["title"].as_str() {
                // A shell call's title is its command line: redact it like the command.
                safe["title"] = json!(format!("{role}: {}", redacted(title, 1000)));
            }
            if let Some(status) = update["status"]
                .as_str()
                .filter(|s| matches!(*s, "pending" | "in_progress" | "completed" | "failed"))
            {
                safe["status"] = json!(status);
            }
            // A row names what the call worked on: a bounded, redacted selection
            // of its input, its locations and an edit's diff. Outputs, thoughts,
            // permission envelopes, read bodies and private report/draft bodies
            // are deliberately not copied into display history.
            selected(update, &mut safe, prior);
            Some(Display::Update(safe))
        }
        // The runtime's own marker of a completed compaction (see `record_update`):
        // a divider in the Workspace conversation. The call-style identity keeps a
        // worker's compaction on that worker's page.
        Some("compaction_update") if update["status"] == "completed" => update["compactionId"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 512)
            .map(|id| {
                Display::Update(json!({"sessionUpdate":"compaction_update",
                    "toolCallId":format!("{attempt}:compaction:{id}"),"status":"completed"}))
            }),
        _ => None,
    })
}
/// The title and kind this task's display history retained for an untitled
/// update's call, read only when the update carries something to select.
fn prior_call(
    db: &rusqlite::Connection,
    task: &str,
    attempt: &str,
    update: &Value,
) -> CoreResult<Value> {
    let selectable = ["rawInput", "content", "rawOutput"]
        .iter()
        .any(|k| !update[*k].is_null());
    let call = update["toolCallId"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 512);
    let (true, Some(call), None) = (
        update["sessionUpdate"] == "tool_call_update" && selectable,
        call,
        update.get("title"),
    ) else {
        return Ok(json!({}));
    };
    // The call's announcement (`tool_call`) carries its title and kind. It is recent, so each
    // session's history is read backwards along its (session, seq) index and stops there.
    let sessions = db
        .prepare("SELECT session_id FROM acp_host_sessions WHERE task_id=? LIMIT 3")?
        .query_map([task], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let id = format!("{attempt}:{call}");
    let mut found: Option<String> = None;
    for session in &sessions {
        found = db.query_row(
            "SELECT update_json FROM host_updates WHERE session_id=? AND json_valid(update_json)
               AND json_extract(update_json,'$.toolCallId')=? AND json_extract(update_json,'$.sessionUpdate')='tool_call'
               AND json_type(update_json,'$.title')='text' ORDER BY seq DESC LIMIT 1",
            (session, &id), |r| r.get(0)).optional()?;
        if found.is_some() {
            break;
        }
    }
    let v: Value = found
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(json!({}));
    Ok(json!({"title":v["title"],"kind":v["kind"]}))
}
/// Records one engine update in display history. A message chunk is not committed on
/// its own: its row is a deferred writer job ([`StoreExecutor::write_deferred`]) that
/// commits within 250 ms in one transaction with the other pending display rows, and
/// always before any later ordinary write (a tool update, the turn's settle event, an
/// owner message). Each chunk still runs the same statement in the same order, so seq
/// allocation, message identities and the turn boundary are decided exactly as for a
/// chunk committed at once. Only display text is at risk: a process killed before the
/// commit loses at most that window's streamed chunks, never authority or lifecycle state.
pub(crate) async fn record(
    executor: &Arc<StoreExecutor>,
    credential: &Credential,
    engine_session: &str,
    params: Value,
) -> CoreResult<()> {
    let credential = credential.clone();
    let engine_session = engine_session.to_owned();
    // Classification needs no store: an update with no display row writes nothing.
    match display(&params, &engine_session, "", "", &json!({}))? {
        None => Ok(()),
        Some(Display::Message(text, engine_id)) => {
            executor.write_deferred(move |db| {
                record_tx(
                    db,
                    &credential,
                    &engine_session,
                    &Value::Null,
                    Some((text, engine_id)),
                ).map(|_| ())
            })?;
            Ok(())
        }
        Some(Display::Update(_)) => executor
            .write(move |db| record_tx(db, &credential, &engine_session, &params, None).map(|_| ()))?
            .await
            .map_err(|_| CoreError::Refused("native_host_closed"))?,
    }
}
/// Only an acknowledged transcript write can attest to a live steering reply.
/// Blank and non-message updates still follow the ordinary display path.
pub(crate) async fn record_steering_reply(
    executor: &StoreExecutor,
    credential: &Credential,
    engine_session: &str,
    handoff: i64,
    params: &Value,
) -> CoreResult<Option<i64>> {
    let Some(Display::Message(text, engine_id)) =
        display(params, engine_session, "", "", &json!({}))?
    else {
        return Ok(None);
    };
    if text.trim().is_empty() {
        return Ok(None);
    }
    let credential = credential.clone();
    let engine_session = engine_session.to_owned();
    executor.write(move |db| {
        let identity = cortex_core::channels::authenticate(db, &credential.channel_id, credential.secret(), true)?;
        let Some((session, seq)) = record_tx(
            db, &credential, &engine_session, &Value::Null, Some((text, engine_id)),
        )? else {
            return Ok(None);
        };
        cortex_core::lifecycle::event_tx(db, &identity.task, "NATIVE_STEERING_REPLY_CHUNK", json!({
            "channel_id":credential.channel_id, "handoff":handoff, "display_session":session, "seq":seq,
        })).map(Some)
    })?.await.map_err(|_| CoreError::Refused("native_host_closed"))?
}

/// One display row in every display session of the bound task: `message` is a
/// streamed message's text and engine identity; otherwise `params` is the update.
fn record_tx(
    db: &rusqlite::Transaction<'_>,
    credential: &Credential,
    engine_session: &str,
    params: &Value,
    message: Option<(String, Option<String>)>,
) -> CoreResult<Option<(String, i64)>> {
    // Final updates may arrive after result_submit revoked the channel. Bind
    // them to the persisted exact attempt/session/current epoch, not readiness
    // or task state. An old owner can never append into a replacement session.
    let rows = db
        .prepare(
            "SELECT c.task_id,c.attempt_id,p.role_id FROM trusted_channels c
                JOIN task_owners o ON o.task_id=c.task_id AND o.epoch=c.owner_epoch
                JOIN delegation_provisioning p ON p.channel_id=c.channel_id AND p.task_id=c.task_id
                    AND p.attempt_id=c.attempt_id AND p.owner_epoch=c.owner_epoch
                WHERE c.channel_id=? AND p.engine_session_ref=?",
        )?
        .query_map((&credential.channel_id, &engine_session), |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    if rows.len() != 1 {
        return Err(CoreError::Refused("native_activity_binding_invalid"));
    }
    let (task, attempt, role) = &rows[0];
    let item = match message {
        Some((text, engine_id)) => Display::Message(text, engine_id),
        None => {
            let prior = prior_call(db, task, attempt, &params["update"])?;
            let Some(item) = display(params, engine_session, attempt, role, &prior)? else {
                return Ok(None);
            };
            item
        }
    };
    // Every chunk the engine emits is recorded in every display session (the
    // Workspace `cortex-native` and the incoming ACP `cortex-rust-acp`), also
    // after the coordinator's question commit: a message is never cut short.
    let sessions = db.prepare("SELECT s.session_id FROM acp_host_sessions s
            WHERE s.task_id=? AND (s.client_info=? OR json_extract(s.client_info,'$.name')='cortex-rust-acp')")?
            .query_map((task,r#"{"name":"cortex-native"}"#),|r|r.get::<_,String>(0))?
            .collect::<Result<Vec<_>,_>>()?;
    if sessions.len() > 2 {
        return Err(CoreError::Refused("native_activity_binding_ambiguous"));
    }
    // The compaction divider is not an ACP v1 session update: incoming ACP sessions
    // replay their stored updates to the client verbatim, so only the Workspace
    // session keeps it.
    let workspace_only =
        matches!(&item, Display::Update(u) if u["sessionUpdate"] == "compaction_update");
    let mut receipt = None;
    for session in &sessions {
        if workspace_only
            && !db.query_row(
                "SELECT client_info=? FROM acp_host_sessions WHERE session_id=?",
                (r#"{"name":"cortex-native"}"#, session),
                |r| r.get::<_, bool>(0),
            )?
        {
            continue;
        }
        let seq: i64 = db.query_row(
            "SELECT coalesce(max(seq),-1)+1 FROM (
            SELECT seq FROM host_transcript WHERE session_id=? UNION ALL
            SELECT seq FROM host_updates WHERE session_id=?)",
            (session, session),
            |r| r.get(0),
        )?;
        match &item {
            Display::Message(text, engine_id) => {
                // One streamed message is one identity: chunks that follow each
                // other directly in this attempt share the first chunk's id.
                let own = format!("{attempt}:");
                // A chunk continues the previous message only within the same engine
                // turn: a turn this channel settled after that chunk starts a new one.
                let previous: Option<Option<String>> = db.query_row(
                    "SELECT h.message_id FROM host_transcript h WHERE h.session_id=? AND h.seq=? AND h.kind='agent_message'
                     AND NOT EXISTS(SELECT 1 FROM runtime_events e WHERE e.task_id=? AND e.type='NATIVE_TURN_SETTLED'
                         AND json_extract(e.payload_json,'$.channel_id')=? AND e.created_at>=h.created_at)",
                    (session, seq - 1, task, &credential.channel_id), |r| r.get(0)).optional()?;
                let message_id = match (engine_id, previous.flatten()) {
                    (Some(id), _) => format!("{own}{id}"),
                    (None, Some(id)) if id.starts_with(&own) => id,
                    _ => format!("{own}{session}:{seq}"),
                };
                if db.execute("INSERT INTO host_transcript(session_id,seq,kind,text,message_id,created_at) VALUES (?,?,'agent_message',?,?,?)",
                    (session,seq,text,message_id,cortex_core::now()))? == 1 && receipt.is_none() {
                    receipt = Some((session.clone(), seq));
                }
            }
            Display::Update(update) => {
                db.execute(
                    "INSERT INTO host_updates VALUES (?,?,?,?)",
                    (session, seq, update.to_string(), cortex_core::now()),
                )?;
            }
        }
    }
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cortex_core::{
        channels::{self, Principal},
        lifecycle::{self, TaskMachine},
        OpenMode, ProjectKey, Store, StoreChannel,
    };
    /// A coordinator channel bound to engine session `wire` and one Workspace display
    /// session `display`, served by a real store executor.
    struct Streaming {
        root: std::path::PathBuf,
        key: ProjectKey,
        executor: Option<Arc<StoreExecutor>>,
        credential: Credential,
    }
    impl Streaming {
        fn new() -> Self {
            use std::os::unix::fs::PermissionsExt;
            let root = std::env::temp_dir().join(format!(
                "activity-stream-{}",
                crate::native_effects::identity().unwrap()
            ));
            std::fs::create_dir(&root).unwrap();
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
            let key = ProjectKey::new(&root, StoreChannel::Dev).unwrap();
            let store = Store::open(key.clone(), OpenMode::Initialize).unwrap();
            let owner = TaskMachine::new(&store)
                .create("t", "stream", "owner")
                .unwrap();
            let credential = store
                .write(|db| {
                    db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'", [])?;
                    let c = channels::issue_tx(db, "t", &owner, Principal::Coordinator)?;
                    channels::bind_session_tx(db, &c, "wire")?;
                    db.execute("INSERT INTO acp_host_sessions(session_id,project_root,task_id,client_info,client_capabilities,mcp_servers,created_at)
                        VALUES ('display',?,'t','{\"name\":\"cortex-native\"}','{}','[]',?)", (root.to_str(), cortex_core::now()))?;
                    Ok(c)
                })
                .unwrap();
            drop(store);
            let executor = Some(Arc::new(StoreExecutor::open(key.clone()).unwrap()));
            Self {
                root,
                key,
                executor,
                credential,
            }
        }
        fn executor(&self) -> &Arc<StoreExecutor> {
            self.executor.as_ref().unwrap()
        }
        async fn chunk(&self, text: &str) {
            let update = json!({"sessionId":"wire","update":{"sessionUpdate":"agent_message_chunk",
                "content":{"type":"text","text":text}}});
            record(self.executor(), &self.credential, "wire", update)
                .await
                .unwrap();
        }
        async fn tool(&self, call: &str) {
            let update = json!({"sessionId":"wire","update":{"sessionUpdate":"tool_call",
                "toolCallId":call,"title":"Read","status":"pending"}});
            record(self.executor(), &self.credential, "wire", update)
                .await
                .unwrap();
        }
        /// Every display row, read by an ordinary writer job: deferred rows commit first.
        async fn rows(&self) -> Vec<(i64, String, Option<String>)> {
            self.executor()
                .write(|db| {
                    let rows = db
                        .prepare(
                            "SELECT seq,coalesce(text,update_json),message_id FROM (
                        SELECT seq,text,NULL AS update_json,message_id FROM host_transcript WHERE session_id='display'
                        UNION ALL SELECT seq,NULL,json_extract(update_json,'$.toolCallId'),NULL FROM host_updates WHERE session_id='display')
                        ORDER BY seq",
                        )?
                        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(rows)
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap()
        }
    }
    impl Drop for Streaming {
        fn drop(&mut self) {
            self.executor.take();
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    /// Streamed chunks no longer commit one by one: they wait in deferred writes that
    /// commit together, still one row per chunk with the identity a chunk committed at
    /// once would get, and every later ordinary write (here a tool update) finds them
    /// committed and ordered before itself.
    #[tokio::test]
    async fn streamed_chunks_commit_deferred_in_order_before_the_next_update() {
        let s = Streaming::new();
        for _ in 0..200 {
            s.chunk("ab").await;
        }
        s.tool("read-1").await;
        s.chunk("after").await;
        let attempt: String = s
            .executor()
            .read(|db| {
                Ok(db.query_row("SELECT attempt_id FROM trusted_channels", [], |r| r.get(0))?)
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        let rows = s.rows().await;
        assert_eq!(rows.len(), 202, "{rows:?}");
        let first = Some(format!("{attempt}:display:0"));
        for (seq, row) in rows[..200].iter().enumerate() {
            assert_eq!(*row, (seq as i64, "ab".into(), first.clone()));
        }
        assert_eq!(rows[200], (200, format!("{attempt}:read-1"), None));
        // A row after another update starts a new message, exactly as before.
        assert_eq!(
            rows[201],
            (201, "after".into(), Some(format!("{attempt}:display:201")))
        );
    }
    /// The settle event of a turn is an ordinary write: the turn's streamed text commits
    /// before it, so the next turn's first chunk starts a new message.
    #[tokio::test]
    async fn a_settled_turn_ends_the_streamed_message_and_its_text_commits_first() {
        let s = Streaming::new();
        s.chunk("first ").await;
        s.chunk("turn").await;
        let channel = s.credential.channel_id.clone();
        s.executor()
            .write(move |db| {
                lifecycle::event_tx(
                    db,
                    "t",
                    "NATIVE_TURN_SETTLED",
                    json!({"channel_id":channel,"stop_reason":"end_turn"}),
                )?;
                // Committed in this very transaction's view: the turn's text came first.
                Ok(db.query_row(
                    "SELECT group_concat(text,'') FROM (SELECT text FROM host_transcript WHERE session_id='display' ORDER BY seq)",
                    [],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .unwrap()
            .await
            .unwrap()
            .map(|text| assert_eq!(text, "first turn"))
            .unwrap();
        s.chunk("second").await;
        let rows = s.rows().await;
        assert_eq!(rows.len(), 3, "{rows:?}");
        assert_eq!(rows[0].2, rows[1].2, "one message within the turn");
        assert_ne!(rows[1].2, rows[2].2, "a new turn starts a new message");
        assert_eq!(rows[2].1, "second");
    }
    /// Deferred text commits on its own within the window, and on executor shutdown.
    /// Only a process killed inside the window loses it (display text only).
    #[tokio::test]
    async fn deferred_text_commits_after_the_window_and_on_shutdown() {
        let mut s = Streaming::new();
        s.chunk("windowed").await;
        tokio::time::sleep(cortex_core::store::executor::DEFER_WINDOW * 4).await;
        let count = |executor: &Arc<StoreExecutor>| {
            executor
                .read(|db| {
                    Ok(
                        db.query_row("SELECT count(*) FROM host_transcript", [], |r| {
                            r.get::<_, i64>(0)
                        })?,
                    )
                })
                .unwrap()
        };
        assert_eq!(count(s.executor()).await.unwrap().unwrap(), 1);
        s.chunk("at shutdown").await;
        let executor = s.executor.take().unwrap();
        tokio::task::spawn_blocking(move || drop(executor))
            .await
            .unwrap();
        let store = Store::open(s.key.clone(), OpenMode::Initialize).unwrap();
        let texts: Vec<String> = store
            .read(|db| {
                Ok(db
                    .prepare("SELECT text FROM host_transcript ORDER BY seq")?
                    .query_map([], |r| r.get(0))?
                    .collect::<Result<_, _>>()?)
            })
            .unwrap();
        assert_eq!(texts, ["windowed", "at shutdown"]);
    }
    #[test]
    fn only_a_completed_compaction_marker_becomes_a_divider_update() {
        let wrap = |u| json!({"sessionId":"s","update":u});
        let Some(Display::Update(u)) = display(
            &wrap(json!({"sessionUpdate":"compaction_update",
            "compactionId":"k","status":"completed","summary":[{"type":"text","text":"private"}]})),
            "s",
            "a",
            "r",
            &json!({}),
        )
        .unwrap() else {
            panic!("no divider")
        };
        assert_eq!(
            u,
            json!({"sessionUpdate":"compaction_update","toolCallId":"a:compaction:k","status":"completed"})
        );
        for update in [
            json!({"sessionUpdate":"compaction_update","compactionId":"k","status":"in_progress"}),
            json!({"sessionUpdate":"compaction_update","status":"completed"}),
            json!({"sessionUpdate":"compaction_summary_chunk","compactionId":"k","content":{"type":"text","text":"private"}}),
        ] {
            assert!(display(&wrap(update), "s", "a", "r", &json!({}))
                .unwrap()
                .is_none());
        }
    }
    #[test]
    fn only_public_messages_and_minimal_tool_activity_are_projected() {
        let wrap = |u| json!({"sessionId":"s","update":u});
        assert!(display(&wrap(json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"private"}})), "s","a","r",&json!({})).unwrap().is_none());
        let item = display(&wrap(json!({"sessionUpdate":"tool_call","toolCallId":"c","title":"Read",
            "status":"completed","rawInput":{"secret":"private"},"rawOutput":"private","content":[{"text":"private"}]})), "s","a","r",&json!({})).unwrap().unwrap();
        let Display::Update(v) = item else { panic!() };
        assert_eq!(v["toolCallId"], "a:c");
        assert!(!v.to_string().contains("private"));
        assert!(display(&wrap(json!({})), "other", "a", "r", &json!({})).is_err());
        let Display::Message(text, _) = display(&wrap(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"😀".repeat(16_001)}})),"s","a","r",&json!({})).unwrap().unwrap() else { panic!() };
        assert!(text.ends_with("[Content clipped]"));
    }

    /// Tool calls in the shapes claude-agent-acp 0.107 and codex-acp send them
    /// (Cortex MCP and native tools). Retained rows used to keep only the title.
    fn engine_calls() -> Vec<Value> {
        vec![
            json!({"sessionUpdate":"tool_call","toolCallId":"read","status":"pending","title":"mcp__cortex__project_read","kind":"other",
                "rawInput":{"path":"src/app.ts","offset":1,"limit":40,"line_numbers":true},
                "content":[{"type":"content","content":{"type":"text","text":"PRIVATE READ BODY"}}]}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"read","status":"completed","rawOutput":"PRIVATE READ BODY",
                "content":[{"type":"content","content":{"type":"text","text":"PRIVATE READ BODY"}}]}),
            json!({"sessionUpdate":"tool_call","toolCallId":"skill","status":"pending","title":"mcp__cortex__read_skill","kind":"other","rawInput":{"skill":"orchestrator-protocol"}}),
            json!({"sessionUpdate":"tool_call","toolCallId":"plan","status":"pending","title":"mcp__cortex__plan_submit","kind":"other",
                "rawInput":{"plan":{"nodes":[{"goal":"PRIVATE PLAN"}]},"request_key":"PRIVATE-KEY"}}),
            json!({"sessionUpdate":"tool_call","toolCallId":"edit","status":"pending","title":"mcp__cortex__project_edit","kind":"other",
                "rawInput":{"path":"src/a.ts","edits":[{"old_string":"let a = 1;","new_string":"let a = 2;","replace_all":false}],"request_key":"PRIVATE-KEY"}}),
            json!({"sessionUpdate":"tool_call","toolCallId":"write","status":"pending","title":"mcp__cortex__project_write","kind":"other",
                "rawInput":{"path":"notes.md","content":"hello\nworld","expected_sha256":"PRIVATE-HASH","request_key":"PRIVATE-KEY"}}),
            json!({"sessionUpdate":"tool_call","toolCallId":"cmd","status":"pending","title":"mcp__cortex__command_run","kind":"other",
                "rawInput":{"argv":["git","status","--short"],"request_key":"PRIVATE-KEY","timeout_seconds":30}}),
            json!({"sessionUpdate":"tool_call","toolCallId":"bash","status":"pending","title":"curl -H 'Authorization: Bearer abcdefgh12345678' https://x.test","kind":"execute",
                "rawInput":{"command":"curl -H 'Authorization: Bearer abcdefgh12345678' https://x.test","description":"PRIVATE DESCRIPTION"}}),
            json!({"sessionUpdate":"tool_call","toolCallId":"native-edit","status":"pending","title":"Edit src/b.ts","kind":"edit",
                "rawInput":{"file_path":"/p/src/b.ts","old_string":"old line","new_string":"new line"},
                "locations":[{"path":"/p/src/b.ts","line":3}],
                "content":[{"type":"diff","path":"/p/src/b.ts","oldText":"old line","newText":"new line"}]}),
            json!({"sessionUpdate":"tool_call","toolCallId":"grep","status":"pending","title":"grep \"TODO\" src","kind":"search","rawInput":{"pattern":"TODO","path":"src","output_mode":"PRIVATE MODE"}}),
            json!({"sessionUpdate":"tool_call","toolCallId":"codex-read","status":"pending","title":"mcp.cortex.project_read","kind":"other",
                "rawInput":{"server":"cortex","tool":"project_read","arguments":{"path":"src/c.ts","PRIVATE":"x"}}}),
            json!({"sessionUpdate":"tool_call","toolCallId":"report","status":"pending","title":"mcp__cortex__draft_write","kind":"edit",
                "rawInput":{"draft_id":"d_1","content":"PRIVATE REPORT","request_key":"PRIVATE-KEY"},
                "content":[{"type":"diff","path":"report.md","newText":"PRIVATE REPORT"}]}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"report","content":[{"type":"diff","path":"report.md","newText":"PRIVATE REPORT"}],
                "rawInput":{"content":"PRIVATE REPORT"}}),
        ]
    }
    /// claude-agent-acp 0.107 announces a call with its title and no input, then
    /// sends the input and the result in untitled updates (seen in a retained
    /// live store); codex-acp 2.6 reports an MCP result as `rawOutput`.
    fn split_calls() -> Vec<Value> {
        let refusal = json!({"code":"plan_invalid","recorded":false,"message":"No plan was recorded (plan_invalid). Correct the proposal and submit it again."}).to_string();
        vec![
            json!({"sessionUpdate":"tool_call","toolCallId":"claude-write","status":"pending","title":"mcp__cortex__project_write","kind":"other"}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"claude-write","rawInput":{"path":"ARCHITECTURE.md","content":"# Architecture\n\nModules.","request_key":"PRIVATE-KEY"}}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"claude-write","status":"completed"}),
            json!({"sessionUpdate":"tool_call","toolCallId":"claude-plan","status":"pending","title":"mcp__cortex__plan_submit","kind":"other"}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"claude-plan","rawInput":{"plan":{"nodes":[{"goal":"PRIVATE PLAN"}]}}}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"claude-plan","status":"failed",
                "content":[{"type":"content","content":{"type":"text","text":format!("```\n{refusal}\n```")}}]}),
            json!({"sessionUpdate":"tool_call","toolCallId":"claude-draft","status":"pending","title":"mcp__cortex__draft_write","kind":"edit"}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"claude-draft","rawInput":{"draft_id":"d_2","content":"PRIVATE DRAFT"},"status":"failed",
                "content":[{"type":"content","content":{"type":"text","text":"```\n{\"code\":\"x\",\"message\":\"PRIVATE DRAFT\"}\n```"}}]}),
            json!({"sessionUpdate":"tool_call","toolCallId":"claude-bash","status":"pending","title":"npm test","kind":"execute","rawInput":{"command":"npm test"}}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"claude-bash","status":"failed",
                "content":[{"type":"content","content":{"type":"text","text":"```console\nPRIVATE OUTPUT\n```"}}],"rawOutput":"PRIVATE OUTPUT"}),
            json!({"sessionUpdate":"tool_call","toolCallId":"codex-plan","status":"in_progress","title":"mcp.cortex.plan_submit","kind":"execute",
                "rawInput":{"server":"cortex","tool":"plan_submit","arguments":{"plan":"PRIVATE PLAN"}}}),
            json!({"sessionUpdate":"tool_call_update","toolCallId":"codex-plan","status":"failed",
                "rawOutput":{"result":{"content":[{"type":"text","text":refusal}],"isError":true},"error":null}}),
        ]
    }
    fn displayed_after(update: Value, prior: &Value) -> Value {
        match display(
            &json!({"sessionId":"s","update":update}),
            "s",
            "xa_1",
            "orchestrator",
            prior,
        )
        .unwrap()
        {
            Some(Display::Update(v)) => v,
            _ => panic!("not an update"),
        }
    }
    fn displayed(update: Value) -> Value {
        displayed_after(update, &json!({}))
    }
    /// What `record` retains for each update in order, the call's first titled
    /// retained row standing in for `prior_call`.
    fn retained(updates: Vec<Value>) -> Vec<Value> {
        let mut rows: Vec<Value> = Vec::new();
        for u in updates {
            let first = rows.iter().find(|r| {
                r["toolCallId"] == format!("xa_1:{}", u["toolCallId"].as_str().unwrap())
                    && r["title"].is_string()
            });
            let prior = if u.get("title").is_none() {
                first
                    .map(|r| json!({"title":r["title"],"kind":r["kind"]}))
                    .unwrap_or(json!({}))
            } else {
                json!({})
            };
            rows.push(displayed_after(u, &prior));
        }
        rows
    }

    #[test]
    fn an_untitled_update_keeps_its_calls_edit_and_refusal_but_never_private_text() {
        let rows = retained(split_calls());
        let all = serde_json::to_string(&rows).unwrap();
        assert!(!all.contains("PRIVATE"), "{all}");
        // The write's text arrives in an untitled update; the retained title says it is a project write.
        assert_eq!(
            rows[1]["rawInput"],
            json!({"path":"ARCHITECTURE.md","content":"# Architecture\n\nModules."})
        );
        let reason =
            "No plan was recorded (plan_invalid). Correct the proposal and submit it again.";
        assert_eq!(
            rows[5]["failure"], reason,
            "claude-agent-acp fences the refusal JSON"
        );
        assert_eq!(
            rows[11]["failure"], reason,
            "codex-acp reports it as rawOutput"
        );
        assert!(
            rows[7].get("failure").is_none()
                && rows[7]
                    .get("rawInput")
                    .map_or(true, |r| r.get("content").is_none())
        );
        assert!(
            rows[9].get("failure").is_none(),
            "an engine tool's own output is not a refusal"
        );
        // Without the retained title the same update stays private.
        let blind = displayed(split_calls()[1].clone());
        assert_eq!(blind["rawInput"], json!({"path":"ARCHITECTURE.md"}));
        assert!(displayed(split_calls()[5].clone()).get("failure").is_none());
        // A refusal text is bounded.
        let long = displayed_after(
            json!({"sessionUpdate":"tool_call_update","toolCallId":"x","status":"failed",
            "content":[{"type":"content","content":{"type":"text","text":json!({"message":"m".repeat(5000)}).to_string()}}]}),
            &json!({"title":"orchestrator: mcp__cortex__plan_submit"}),
        );
        assert!(long["failure"].as_str().unwrap().chars().count() < 1100);
    }

    #[test]
    fn prior_call_reads_the_announcement_of_the_same_task_call() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        db.execute_batch("CREATE TABLE acp_host_sessions(session_id TEXT, task_id TEXT);
            CREATE TABLE host_updates(session_id TEXT, seq INTEGER, update_json TEXT, created_at TEXT);
            INSERT INTO acp_host_sessions VALUES ('s','task'),('other','elsewhere');").unwrap();
        for (session, seq, row) in [
            (
                "other",
                0,
                json!({"toolCallId":"xa_1:c","sessionUpdate":"tool_call","title":"worker-x: mcp__cortex__draft_write","kind":"edit"}),
            ),
            (
                "s",
                1,
                json!({"toolCallId":"xa_1:c","sessionUpdate":"tool_call_update"}),
            ),
            (
                "s",
                2,
                json!({"toolCallId":"xa_1:c","sessionUpdate":"tool_call","title":"worker-x: mcp__cortex__project_write","kind":"other"}),
            ),
            (
                "s",
                3,
                json!({"toolCallId":"xa_1:c","sessionUpdate":"tool_call_update","title":"later"}),
            ),
        ] {
            db.execute(
                "INSERT INTO host_updates VALUES (?,?,?,'now')",
                (session, seq, row.to_string()),
            )
            .unwrap();
        }
        let untitled =
            json!({"sessionUpdate":"tool_call_update","toolCallId":"c","rawInput":{"path":"a"}});
        assert_eq!(
            prior_call(&db, "task", "xa_1", &untitled).unwrap(),
            json!({"title":"worker-x: mcp__cortex__project_write","kind":"other"})
        );
        // Nothing to select, a titled update or another attempt: no lookup.
        assert_eq!(
            prior_call(
                &db,
                "task",
                "xa_1",
                &json!({"sessionUpdate":"tool_call_update","toolCallId":"c","status":"completed"})
            )
            .unwrap(),
            json!({})
        );
        assert_eq!(prior_call(&db, "task", "xa_1", &json!({"sessionUpdate":"tool_call_update","toolCallId":"c","title":"t","rawInput":{}})).unwrap(), json!({}));
        assert_eq!(
            prior_call(&db, "task", "xa_2", &untitled).unwrap(),
            json!({"title":null,"kind":null})
        );
    }

    #[test]
    fn tool_rows_keep_their_target_and_edit_but_never_private_input() {
        let rows: Vec<Value> = engine_calls().into_iter().map(displayed).collect();
        let all = serde_json::to_string(&rows).unwrap();
        assert!(!all.contains("PRIVATE"), "{all}");
        assert!(!all.contains("abcdefgh12345678"), "{all}");
        assert_eq!(rows[0]["title"], "orchestrator: mcp__cortex__project_read");
        assert_eq!(rows[0]["kind"], "other");
        assert_eq!(rows[0]["rawInput"], json!({"path":"src/app.ts"}));
        assert!(rows[1].get("rawInput").is_none() && rows[1].get("content").is_none());
        assert_eq!(
            rows[2]["rawInput"],
            json!({"skill":"orchestrator-protocol"})
        );
        assert!(
            rows[3].get("rawInput").is_none(),
            "a plan body is not a row target"
        );
        assert_eq!(
            rows[4]["rawInput"],
            json!({"path":"src/a.ts","edits":[{"old_string":"let a = 1;","new_string":"let a = 2;"}]})
        );
        assert_eq!(
            rows[5]["rawInput"],
            json!({"path":"notes.md","content":"hello\nworld"})
        );
        assert_eq!(
            rows[6]["rawInput"],
            json!({"argv":["git","status","--short"]})
        );
        assert!(rows[7]["rawInput"]["command"]
            .as_str()
            .unwrap()
            .starts_with("curl -H 'Authorization: "));
        assert_eq!(rows[8]["rawInput"]["old_string"], "old line");
        assert_eq!(
            rows[8]["locations"],
            json!([{"path":"/p/src/b.ts","line":3}])
        );
        assert_eq!(
            rows[8]["content"],
            json!([{"type":"diff","path":"/p/src/b.ts","oldText":"old line","newText":"new line"}])
        );
        assert_eq!(rows[9]["rawInput"], json!({"pattern":"TODO","path":"src"}));
        assert_eq!(
            rows[10]["rawInput"],
            json!({"server":"cortex","tool":"project_read","arguments":{"path":"src/c.ts"}})
        );
        assert_eq!(rows[11]["rawInput"], json!({"draft_id":"d_1"}));
        assert!(rows[12].get("content").is_none() && rows[12].get("rawInput").is_none());
        // Bounds hold for every selected text.
        let long = displayed(
            json!({"sessionUpdate":"tool_call","toolCallId":"big","title":"Write a.txt","kind":"edit",
            "rawInput":{"file_path":"a.txt","content":"x".repeat(40_000)}}),
        );
        assert!(long["rawInput"]["content"]
            .as_str()
            .unwrap()
            .ends_with("[Content clipped]"));
        assert!(long.to_string().len() < 17_000);
    }

    /// The retained rows read back through the Workspace history projection as
    /// rows that name their file, skill, command or edit.
    #[test]
    fn retained_activity_projects_paths_skills_commands_and_diffs() {
        use cortex_core::{
            lifecycle::TaskMachine, projections::ProjectionReader, OpenMode, ProjectKey, Store,
            StoreChannel,
        };
        use std::os::unix::fs::DirBuilderExt;
        use workspace_protocol::{
            core::{CoreQuery, Page},
            read::QueryReply,
        };
        let dir = std::env::temp_dir().join(format!(
            "cortex-native-activity-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&dir).unwrap();
        let key = ProjectKey::new(&dir, StoreChannel::Dev).unwrap();
        let store = Store::open(key.clone(), OpenMode::Initialize).unwrap();
        TaskMachine::new(&store)
            .create("runtime", "goal", "owner")
            .unwrap();
        let root = dir.to_str().unwrap().to_owned();
        let rows: Vec<String> = retained(engine_calls().into_iter().chain(split_calls()).collect())
            .iter()
            .map(Value::to_string)
            .collect();
        store.write(move |db| {
            db.execute("INSERT INTO workspace_projects VALUES ('p',?,'owner','2026-11-17T00:00:00Z',1)", [&root])?;
            db.execute("INSERT INTO acp_host_sessions VALUES ('session',?,'runtime','{\"name\":\"cortex-native\"}','{}','[]','2026-11-17T00:00:00Z',NULL)", [&root])?;
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,execution_id,created_at) VALUES ('task','p','Description',1,'execution','2026-11-17T00:00:00Z')", [])?;
            db.execute("INSERT INTO workspace_executions VALUES ('execution','task','runtime','session',1,1,'2026-11-17T00:00:00Z')", [])?;
            for (seq, row) in rows.iter().enumerate() {
                db.execute("INSERT INTO host_updates VALUES ('session',?,?,'2026-11-17T00:00:00Z')", rusqlite::params![seq as i64, row])?;
            }
            Ok(())
        }).unwrap();
        let reader = ProjectionReader::new(
            Store::open(key, OpenMode::LiveReadOnly).unwrap(),
            "p".into(),
            "owner".into(),
        )
        .unwrap();
        let QueryReply::History(page) = reader
            .query(CoreQuery::History(Page {
                target: "task".into(),
                cursor: None,
                limit: 100,
            }))
            .unwrap()
            .data
        else {
            panic!()
        };
        let _ = std::fs::remove_dir_all(&dir);
        let steps: Vec<Value> = page
            .items
            .iter()
            .filter_map(|r| {
                r.text
                    .as_deref()?
                    .strip_prefix("Workspace step: ")
                    .map(|s| serde_json::from_str(s).unwrap())
            })
            .collect();
        let call = |id: &str| {
            steps
                .iter()
                .rev()
                .find(|s| s["callId"] == format!("xa_1:{id}"))
                .cloned()
                .unwrap_or_else(|| panic!("{id}: {steps:?}"))
        };
        let all = serde_json::to_string(&page).unwrap();
        assert!(!all.contains("PRIVATE"), "{all}");
        assert_eq!(call("read")["path"], "src/app.ts");
        assert_eq!(call("read")["status"], "completed");
        assert_eq!(call("skill")["ref"], "orchestrator-protocol");
        assert_eq!(call("edit")["path"], "src/a.ts");
        assert_eq!(
            (
                call("edit")["oldText"].as_str(),
                call("edit")["newText"].as_str()
            ),
            (Some("let a = 1;"), Some("let a = 2;"))
        );
        assert_eq!(call("write")["newText"], "hello\nworld");
        assert_eq!(call("cmd")["command"], "git status --short");
        assert_eq!(call("native-edit")["path"], "/p/src/b.ts");
        assert_eq!(call("native-edit")["newText"], "new line");
        assert_eq!(call("grep")["query"], "TODO");
        assert_eq!(call("codex-read")["path"], "src/c.ts");
        assert!(call("report").get("newText").is_none());
        // The engine shapes that split a call: the write's text and both refusals read back.
        assert_eq!(
            (
                call("claude-write")["path"].as_str(),
                call("claude-write")["newText"].as_str()
            ),
            (Some("ARCHITECTURE.md"), Some("# Architecture\n\nModules."))
        );
        let reason =
            "No plan was recorded (plan_invalid). Correct the proposal and submit it again.";
        assert_eq!(call("claude-plan")["failure"], reason);
        assert_eq!(call("claude-plan")["status"], "failed");
        assert_eq!(call("codex-plan")["failure"], reason);
        assert!(call("claude-bash").get("failure").is_none());
        assert!(call("claude-draft").get("failure").is_none());
    }
}
