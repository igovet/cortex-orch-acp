//! Display-only compatibility reader. Never constructs a scheduler or writable store.
use cortex_core::{CoreError, OpenMode, Store};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use std::{collections::HashSet, io, path::Path, sync::Mutex};

fn error(e: CoreError) -> io::Error {
    io::Error::other(e.code())
}
pub struct LegacyHistory {
    store: Store,
    session: String,
    high: i64,
    reads: Mutex<ReadFilter>,
}
#[derive(Default)]
struct ReadFilter {
    reads: HashSet<String>,
    explicit: HashSet<String>,
    shells: HashSet<String>,
}
impl ReadFilter {
    fn observe(&mut self, id: &str, v: &Value) {
        if id.starts_with("worker:") {
            return;
        }
        let mut title = v["title"].as_str().unwrap_or("").trim().to_lowercase();
        let args = v
            .get("rawInput")
            .or_else(|| v.get("raw_input"))
            .unwrap_or(&Value::Null);
        if args["server"] == "cortex" {
            if let Some(tool) = args["tool"].as_str() {
                title = tool.to_lowercase();
            }
        }
        let tool = title
            .strip_prefix("mcp__cortex__")
            .or_else(|| title.strip_prefix("mcp.cortex."))
            .unwrap_or(&title);
        let explicit = v["kind"] == "read"
            || ["project_read", "read_skill", "read_batch", "tools_discover"].contains(&tool);
        let command = args
            .get("command")
            .or_else(|| args.get("cmd"))
            .and_then(Value::as_str);
        let shell = v["kind"] == "execute"
            || v["kind"] == "terminal"
            || command.is_some_and(|c| {
                !c.trim().is_empty()
                    && c.split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .to_lowercase()
                        == title.split_whitespace().collect::<Vec<_>>().join(" ")
            });
        if explicit {
            self.explicit.insert(id.into());
            self.reads.insert(id.into());
        } else if !self.explicit.contains(id) && shell {
            self.reads.remove(id);
            self.shells.insert(id.into());
        } else if !self.shells.contains(id)
            && (["project_read", "read_skill"].contains(&tool)
                || ["read ", "view ", "cat "]
                    .iter()
                    .any(|p| title.starts_with(p)))
        {
            self.reads.insert(id.into());
        }
    }
}
impl LegacyHistory {
    pub fn open(root: &Path, session: &str) -> io::Result<Option<Self>> {
        let key = crate::native_runtime::key(root).map_err(error)?;
        if !key.database().try_exists()? {
            return Ok(None);
        }
        let store = Store::open(key, OpenMode::LiveReadOnly).map_err(error)?;
        let high = store
            .read(|db| {
                let info: Option<String> = db.query_row(
                "SELECT client_info FROM acp_host_sessions WHERE session_id=? AND project_root=?",
                (session, root.to_str()), |r|r.get(0)).optional()?;
                let Some(info) = info else {
                    return Ok(None);
                };
                let info: Value = serde_json::from_str(&info)?;
                if info["name"] == "cortex-rust-acp" {
                    return Ok(None);
                }
                let duplicates: bool = db.query_row(
                    "SELECT EXISTS(SELECT seq FROM (
                SELECT seq FROM host_transcript WHERE session_id=? UNION ALL
                SELECT seq FROM host_updates WHERE session_id=?) GROUP BY seq HAVING count(*)>1)",
                    (session, session),
                    |r| r.get(0),
                )?;
                if duplicates {
                    return Err(CoreError::Refused("incomplete_history"));
                }
                Ok(Some(db.query_row(
                    "SELECT coalesce(max(seq),-1) FROM (
                SELECT seq FROM host_transcript WHERE session_id=? UNION ALL
                SELECT seq FROM host_updates WHERE session_id=?)",
                    (session, session),
                    |r| r.get(0),
                )?))
            })
            .map_err(error)?;
        Ok(high.map(|high| Self {
            store,
            session: session.into(),
            high,
            reads: Mutex::new(ReadFilter::default()),
        }))
    }
    /// Python `_session_load` selection context, read-only: the task's latest
    /// live configuration revision, its language override and access mode, or
    /// for a task-less session only its access mode (no inherited default).
    pub fn selection(&self) -> io::Result<Value> {
        self.store
            .read(|db| {
                let task: Option<String> = db.query_row(
                    "SELECT task_id FROM acp_host_sessions WHERE session_id=?",
                    [&self.session],
                    |r| r.get(0),
                )?;
                let Some(task) = task else {
                    return Ok(json!({"legacy":true,"task":false,
                        "mode":cortex_core::access::session(db, &self.session)?}));
                };
                let language: Option<Option<String>> = db
                    .query_row(
                        "SELECT language FROM task_language WHERE task_id=?",
                        [&task],
                        |r| r.get(0),
                    )
                    .optional()?;
                let revision: Option<(String, String, i64, String)> = db
                    .query_row(
                        "SELECT route_id,state,revision,parameters FROM configuration_revisions
                         WHERE task_id=? AND state IN ('selected','pending_next_use','active')
                         ORDER BY revision DESC LIMIT 1",
                        [&task],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                    )
                    .optional()?;
                let mut selection = json!({"legacy":true,"task":true,
                    "mode":cortex_core::access::current(db, &task)?,"language":language.flatten()});
                if let Some((route, state, revision, parameters)) = revision {
                    selection["model"] = json!(route);
                    selection["state"] = json!(state);
                    selection["revision"] = json!(revision);
                    selection["parameters"] = serde_json::from_str(&parameters)?;
                }
                Ok(selection)
            })
            .map_err(error)
    }
    pub fn page(&self, after: i64) -> io::Result<Vec<Value>> {
        self.store.read(|db| {
            let mut query = db.prepare("SELECT seq,kind,text,attachments,message_id FROM (
                SELECT seq,kind,text,attachments_json AS attachments,message_id FROM host_transcript WHERE session_id=?
                UNION ALL SELECT seq,'update',update_json,'[]',NULL FROM host_updates WHERE session_id=?)
                WHERE seq>? AND seq<=? ORDER BY seq LIMIT 100")?;
            let rows = query.query_map((&self.session,&self.session,after,self.high),
                |r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,
                    r.get::<_,Option<String>>(3)?,r.get::<_,Option<String>>(4)?)))?
                .collect::<Result<Vec<_>,_>>()?;
            rows.into_iter().map(|(seq,kind,mut text,attachments,message_id)| {
                let mut update = if kind=="update" {
                    let mut value: Value=serde_json::from_str(&text)?;
                    if value["sessionUpdate"].as_str().is_none() { return Err(CoreError::Refused("incomplete_history")); }
                    // Legacy read bodies are not display history. Keep identity,
                    // target and status, including status-only updates on later pages.
                    if let Some(id)=value["toolCallId"].as_str().map(str::to_owned){
                        let mut reads=self.reads.lock().map_err(|_|CoreError::Refused("history_reader_closed"))?;
                        reads.observe(&id,&value);
                        if reads.reads.contains(&id)&&!id.starts_with("worker:"){
                            for key in ["content","text","raw_output","rawOutput","output","error","diff"]{
                                value.as_object_mut().unwrap().remove(key);
                            }
                        }
                    }
                    value
                } else {
                    if !["user_message","agent_message"].contains(&kind.as_str()) { return Err(CoreError::Refused("incomplete_history")); }
                    let a: Value=serde_json::from_str(attachments.as_deref().unwrap_or("[]"))?;
                    for item in a.as_array().ok_or(CoreError::Refused("incomplete_history"))? {
                        if !item.is_object() { return Err(CoreError::Refused("incomplete_history")); }
                        let label=item["name"].as_str().or(item["uri"].as_str()).or(item["kind"].as_str()).unwrap_or("None");
                        text.push_str(&format!("\n[attachment: {} ({}) source_ref={}]",label,
                            item["availability"].as_str().unwrap_or("None"),item["source_ref"].as_str().unwrap_or("None")));
                    }
                    json!({"sessionUpdate":if kind=="user_message"{"user_message_chunk"}else{"agent_message_chunk"},
                        "content":{"type":"text","text":text},"messageId":message_id.unwrap_or_else(||format!("replay:{}:{seq}",self.session))})
                };
                if !update["_meta"].is_object() { update["_meta"]=json!({}); }
                update["_meta"]["cortexReplay"]=json!({"sessionId":self.session,"sequences":[seq]});
                if update.to_string().len()>cortex_engine::wire::MAX_FRAME-2048{return Err(CoreError::Refused("history_frame_too_large"));}
                Ok(json!({"seq":seq,"update":update}))
            }).collect()
        }).map_err(error)
    }
}
