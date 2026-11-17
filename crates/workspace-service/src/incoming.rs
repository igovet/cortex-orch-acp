//! Trusted incoming-client facade over the same native runtime used by Desktop.
//! No ACP framing here, no alternative scheduler or legacy mutation fallback.
use crate::{
    actor, identity,
    native_runtime::{self, NativeRoutes, Runtime},
};
use cortex_core::{CoreError, CoreResult};
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, io, path::Path, sync::Arc};

const LANGUAGE_DESCRIPTION: &str =
    "Language for Cortex's own status/question/stop lines; auto detects from your messages.";
/// Python `configuration.access_mode.LABELS`/`DESCRIPTIONS`.
pub fn access_label(mode: &str) -> (&'static str, &'static str) {
    match mode {
        "development" => ("Development mode", "Workers can work anywhere on this computer without asking. A deletion still asks you first. Docker and arbitrary code can bypass filesystem masks."),
        "auto" => ("Auto mode", "Workers can work anywhere on this computer and nothing asks you, deletions included. .git/.cortex protection, role limits and MCP checks still apply."),
        _ => ("Guarded mode", "Workers work inside the project. Asks you before a deletion, a write outside the project or a command outside the sandbox."),
    }
}
/// Python `access_mode_state`: the ACP `modes` a Paseo client shows.
pub fn modes(current: &str) -> Value {
    json!({"currentModeId":current,
        "availableModes":cortex_core::access::MODES.map(|m|{let (name,description)=access_label(m);
            json!({"id":m,"name":name,"description":description})})})
}
/// Python `build_config_options` for one session's presentation settings:
/// the route options, then language and access. Also used, read-only, for
/// Python-created sessions loaded through the display facade.
pub async fn presentation(
    routes: Arc<dyn NativeRoutes>,
    root: std::path::PathBuf,
    settings: Value,
) -> io::Result<Value> {
    let s = settings.clone();
    let mut options = tokio::task::spawn_blocking(move || routes.options(&root, &s))
        .await
        .map_err(|_| io::Error::other("route_reader_closed"))?;
    let a = options
        .as_array_mut()
        .ok_or_else(|| io::Error::other("invalid_config_options"))?;
    // Python `build_language_option`/`build_access_mode_option`.
    let language = settings["language"]
        .as_str()
        .filter(|l| matches!(*l, "auto" | "en" | "ru"))
        .unwrap_or("auto");
    a.push(
        json!({"id":"language","name":"Language","type":"select","category":"other",
        "currentValue":language,"description":LANGUAGE_DESCRIPTION,"options":[
            {"value":"auto","name":"Auto (detect from your messages)"},
            {"value":"en","name":"English"},{"value":"ru","name":"Russian"}]}),
    );
    let mode = settings["mode"]
        .as_str()
        .filter(|m| cortex_core::access::MODES.contains(m))
        .unwrap_or("guarded");
    a.push(
        json!({"id":"access_mode","name":"Access","type":"select","category":"mode",
        "currentValue":mode,"description":access_label(mode).1,
        "options":cortex_core::access::MODES.map(|m|{let (name,description)=access_label(m);
            json!({"value":m,"name":name,"description":description})})}),
    );
    Ok(options)
}
/// The four persisted native task settings; presentation keys never leave here.
fn task_settings(settings: &Value) -> Value {
    json!({"model":settings["model"],"effort":settings["effort"],
        "mode":settings["mode"],"language":settings["language"]})
}
/// Per-client presentation state, in memory like Python's session dict: the
/// combined Paseo picker, a reload (no inherited default) and a pre-task
/// session-local route choice `(route, effort)` shown as pending.
#[derive(Clone, Default)]
struct ClientView {
    combined: bool,
    loaded: bool,
    pending: Option<(String, String)>,
    /// A first prompt whose Start did not complete: its text digest with the
    /// create and start request keys. A retry of the same prompt replays those
    /// idempotent requests and reuses the draft instead of creating another.
    first_prompt: Option<(String, String, String)>,
}

fn err(e: CoreError) -> io::Error {
    io::Error::other(e.code())
}
async fn recv<T>(r: tokio::sync::oneshot::Receiver<CoreResult<T>>) -> io::Result<T> {
    r.await
        .map_err(|_| io::Error::other("native_host_closed"))?
        .map_err(err)
}
pub struct IncomingHost {
    runtime: Arc<Runtime>,
    project: String,
    recovered: tokio::sync::OnceCell<()>,
    views: std::sync::Mutex<HashMap<String, ClientView>>,
}
impl IncomingHost {
    /// Call on a blocking thread. Core acquires its scheduler and owner fences
    /// before initialization. A busy desktop owner is refused, never duplicated.
    pub fn open(root: &Path, routes: Arc<dyn NativeRoutes>) -> io::Result<Self> {
        let key = native_runtime::key(root).map_err(err)?;
        if key.root() != root {
            return Err(io::Error::other("incoming_worktree_unavailable"));
        }
        if native_runtime::legacy(root).map_err(err)? {
            return Err(io::Error::other("legacy_project_requires_retained_adapter"));
        }
        let project = format!(
            "acp-{}",
            &format!("{:x}", Sha256::digest(root.as_os_str().as_encoded_bytes()))[..32]
        );
        Ok(Self {
            runtime: Arc::new(Runtime::open_shared(root, &project, routes).map_err(err)?),
            project,
            recovered: tokio::sync::OnceCell::new(),
            views: std::sync::Mutex::new(HashMap::new()),
        })
    }
    /// Bind the connected client's presentation to a new or reloaded session.
    /// Like Python's session dict, a reload in the same process keeps the
    /// session's pending choice; a session first seen by a reload has neither
    /// a pending choice nor an inherited default.
    pub fn bind_client(&self, sid: &str, combined: bool, loaded: bool) {
        let mut views = self.views.lock().unwrap();
        if let Some(view) = views.get_mut(sid) {
            view.combined = combined;
            return;
        }
        if views.len() >= 4096 {
            views.clear();
        }
        views.insert(
            sid.to_owned(),
            ClientView {
                combined,
                loaded,
                pending: None,
                first_prompt: None,
            },
        );
    }
    fn view(&self, sid: &str) -> ClientView {
        self.views
            .lock()
            .unwrap()
            .get(sid)
            .cloned()
            .unwrap_or_default()
    }
    pub fn validate_mcp_servers(&self, entries: &Value) -> io::Result<()> {
        self.runtime.host.mcp.validate_client(entries).map_err(err)
    }
    pub fn bind_permissions(&self, provider: Arc<dyn crate::native_sessions::ClientPermissions>) {
        *self.runtime.host.permissions.write().unwrap() = Some(provider);
    }
    pub async fn recover(&self) -> io::Result<()> {
        self.recovered
            .get_or_try_init(|| async { self.runtime.recover().await.map_err(err) })
            .await?;
        // This process retook the shared lease and found a task a vanished peer left
        // unfinished: recover it here, as a restart would.
        if self.runtime.host.executor.take_orphaned() {
            self.runtime.recover().await.map_err(err)?;
            let _ = self.runtime.continue_owed_tasks().await;
        }
        Ok(())
    }
    pub async fn new_session(&self, info: Value, capabilities: Value) -> io::Result<String> {
        self.new_session_with_mode(info, capabilities, "development".into())
            .await
    }
    async fn new_session_with_mode(
        &self,
        info: Value,
        capabilities: Value,
        mode: String,
    ) -> io::Result<String> {
        let sid = format!("acps_{}", identity()?);
        let result = sid.clone();
        let root = self.runtime.host.root.clone();
        recv(
            self.runtime
                .host
                .executor
                .write(move |db| {
                    // This marker distinguishes native ingress from retained Python sessions.
                    let info = json!({"name":"cortex-rust-acp","client":info});
                    db.execute(
                        "INSERT INTO acp_host_sessions VALUES (?,?,NULL,?,?,'[]',?,NULL)",
                        (
                            &sid,
                            root.to_str(),
                            info.to_string(),
                            capabilities.to_string(),
                            cortex_core::now(),
                        ),
                    )?;
                    db.execute(
                        "INSERT INTO acp_session_access_defaults(session_id,mode) VALUES (?,?)",
                        (&sid, &mode),
                    )?;
                    Ok(())
                })
                .map_err(err)?,
        )
        .await?;
        Ok(result)
    }
    pub async fn session(&self, sid: &str) -> io::Result<Value> {
        let sid = sid.to_owned();
        let root = self.runtime.host.root.clone();
        recv(self.runtime.host.executor.read(move |db| {
            let task: Option<Option<String>> = db.query_row(
                "SELECT task_id FROM acp_host_sessions WHERE session_id=? AND project_root=?
                 AND json_extract(client_info,'$.name')='cortex-rust-acp'",
                (&sid,root.to_str()), |r|r.get(0)).optional()?;
            let task = task.ok_or(CoreError::Refused("unknown_session"))?;
            if let Some(task) = task {
                let (execution,logical,version,epoch,state,synthesis):(String,String,i64,i64,String,Option<String>) = db.query_row(
                    "SELECT e.execution_id,e.acp_session_id,e.version,o.epoch,r.state,r.synthesis
                     FROM workspace_executions e JOIN workspace_tasks t USING(task_id)
                     JOIN runtime_tasks r ON r.task_id=e.runtime_task_id JOIN task_owners o ON o.task_id=r.task_id
                     WHERE e.runtime_task_id=? AND o.epoch=e.owner_epoch", [&task],
                    |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?)))?;
                Ok(json!({"task":task,"execution":execution,"logicalSession":logical,
                    "version":version.to_string(),"epoch":epoch.to_string(),"state":state,"synthesis":synthesis}))
            } else { Ok(json!({"task":null})) }
        }).map_err(err)?).await
    }
    /// Bounded sequence reads. Only public history for the transport-bound session.
    pub async fn history(&self, sid: &str, after: i64) -> io::Result<Vec<Value>> {
        self.session(sid).await?;
        let sid = sid.to_owned();
        recv(self.runtime.host.executor.read(move |db| {
            let mut q = db.prepare("SELECT seq,kind,text,attachments,message_id FROM (
                SELECT seq,kind,text,attachments_json AS attachments,message_id FROM host_transcript WHERE session_id=?
                UNION ALL SELECT seq,'update',update_json,'[]',NULL FROM host_updates WHERE session_id=?)
                WHERE seq>? ORDER BY seq LIMIT 100")?;
            let rows = q.query_map((&sid,&sid,after), |r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?,r.get::<_,Option<String>>(4)?)))?
                .collect::<Result<Vec<_>,_>>()?;
            rows.into_iter().map(|(seq,kind,mut text,attachments,message_id)| {
                let update = if kind=="update" { serde_json::from_str(&text)? } else {
                    let attachments:Value=serde_json::from_str(attachments.as_deref().unwrap_or("[]"))?;
                    for item in attachments.as_array().ok_or(CoreError::Refused("incomplete_history"))?{
                        let label=item["name"].as_str().or(item["uri"].as_str()).or(item["kind"].as_str()).unwrap_or("attachment");
                        text.push_str(&format!("\n[attachment: {} ({}) source_ref={}]",label,
                            item["availability"].as_str().unwrap_or("inaccessible"),item["source_ref"].as_str().unwrap_or("unknown")));
                    }
                    json!({"sessionUpdate":if kind=="user_message"{"user_message_chunk"}else{"agent_message_chunk"},
                        "content":{"type":"text","text":text},"messageId":message_id.unwrap_or_else(||format!("{sid}:{seq}"))})
                };
                Ok(json!({"seq":seq,"update":update}))
            }).collect()
        }).map_err(err)?).await
    }
    pub async fn settings(&self, sid: &str) -> io::Result<Value> {
        let binding = self.session(sid).await?;
        let view = self.view(sid);
        if binding["state"] != "COMPLETED" {
            if let Some(task) = binding["task"].as_str() {
                let mut settings = self.runtime.task_settings(task).await.map_err(err)?;
                if settings.is_object() {
                    settings["mode"] = json!(self.access_mode(sid).await?);
                    settings["task"] = json!(true);
                    settings["combined"] = json!(view.combined);
                    return Ok(settings);
                }
            }
        }
        let access = self.access_mode(sid).await?;
        let sid = sid.to_owned();
        let selected = recv(self.runtime.host.executor.read(move |db| {
            let value: Option<String> = db.query_row(
                "SELECT update_json FROM host_updates WHERE session_id=? AND json_extract(update_json,'$.sessionUpdate')='config_option_update'
                 ORDER BY seq DESC LIMIT 1", [&sid], |r|r.get(0)).optional()?;
            value.map(|s|serde_json::from_str::<Value>(&s).map_err(CoreError::from)).transpose()
        }).map_err(err)?).await?;
        let routes = self.runtime.routes.clone();
        let language = tokio::task::spawn_blocking(move || routes.default_language())
            .await
            .map_err(|_| io::Error::other("route_reader_closed"))?
            .unwrap_or_else(|| "auto".into());
        let mut settings = json!({"model":"","effort":"","language":language,"mode":access,
            "combined":view.combined,"loaded":view.loaded});
        // Only the language choice is durable session state; a route choice is
        // the in-memory pending selection, its default the saved user route.
        if let Some(v) = selected {
            if let Some(option) = v["configOptions"]
                .as_array()
                .and_then(|a| a.iter().find(|o| o["id"] == "language"))
            {
                settings["language"] = option["currentValue"].clone();
            }
        }
        if !binding["task"].is_null() {
            settings["task"] = json!(true);
        } else if let Some((route, effort)) = view.pending {
            settings["model"] = json!(route);
            settings["effort"] = json!(effort);
            settings["state"] = json!("pending_next_use");
        }
        Ok(settings)
    }
    pub async fn access_mode(&self, sid: &str) -> io::Result<String> {
        let binding = self.session(sid).await?;
        let sid = sid.to_owned();
        recv(
            self.runtime
                .host
                .executor
                .read(move |db| match binding["task"].as_str() {
                    Some(task) => cortex_core::access::current(db, task),
                    None => cortex_core::access::session(db, &sid),
                })
                .map_err(err)?,
        )
        .await
    }
    pub async fn modes(&self, sid: &str) -> io::Result<Value> {
        Ok(modes(&self.access_mode(sid).await?))
    }
    pub async fn select_access(&self, sid: &str, mode: &str, origin: &str) -> io::Result<Value> {
        if !cortex_core::access::MODES.contains(&mode)
            || !matches!(origin, "acp_set_mode" | "acp_config_option")
        {
            return Err(io::Error::other("unsupported_config_value"));
        }
        let b = self.session(sid).await?;
        let result_sid = sid.to_owned();
        let sid = sid.to_owned();
        let mode = mode.to_owned();
        let origin = origin.to_owned();
        recv(self.runtime.host.executor.write(move |db| {
            let task: Option<String> = db.query_row("SELECT task_id FROM acp_host_sessions WHERE session_id=?", [&sid], |r|r.get(0))?;
            if let Some(task) = task {
                let epoch = b["epoch"].as_str().and_then(|s|s.parse::<i64>().ok()).ok_or(CoreError::Refused("missing_ownership_epoch"))?;
                let actual:i64 = db.query_row("SELECT epoch FROM task_owners WHERE task_id=?", [&task], |r|r.get(0))?;
                if epoch != actual { return Err(CoreError::Refused("stale_owner_epoch")); }
                cortex_core::access::switch_tx(db,&task,&mode,&sid,&origin)?;
            } else {
                db.execute("INSERT INTO acp_session_access_defaults VALUES (?,'guarded',?,?)
                    ON CONFLICT(session_id) DO UPDATE SET selected_mode=excluded.selected_mode,selection_origin=excluded.selection_origin",
                    (&sid,&mode,&origin))?;
            }
            Ok(())
        }).map_err(err)?).await?;
        self.config_options(&result_sid).await
    }
    pub async fn config_options(&self, sid: &str) -> io::Result<Value> {
        let settings = self.settings(sid).await?;
        self.options_for(settings).await
    }
    async fn options_for(&self, settings: Value) -> io::Result<Value> {
        presentation(
            self.runtime.routes.clone(),
            self.runtime.host.root.clone(),
            settings,
        )
        .await
    }
    pub async fn configure(&self, sid: &str, id: &str, value: &str) -> io::Result<Value> {
        if id == "access_mode" {
            return self.select_access(sid, value, "acp_config_option").await;
        }
        let options = self.config_options(sid).await?;
        // `route` (an exact route id from `/cortex-model`) is not an option.
        if id != "route" {
            let option = options
                .as_array()
                .unwrap()
                .iter()
                .find(|o| o["id"] == id)
                .ok_or_else(|| io::Error::other("unknown_config_option"))?;
            if !option["options"]
                .as_array()
                .unwrap()
                .iter()
                .any(|o| o["value"] == value)
            {
                return Err(io::Error::other("unsupported_config_value"));
            }
        }
        let mut settings = self.settings(sid).await?;
        let previous = settings.clone();
        let b = self.session(sid).await?;
        let pre_task = b["task"].is_null();
        let route_option = matches!(id, "provider" | "model" | "thought_level" | "route");
        match id {
            "language" => settings["language"] = json!(value),
            _ if route_option => {
                // Python `_apply_config_selection`: provider/model choose an exact
                // route and reset its dependent parameters; an effort keeps it.
                let routes = self.runtime.routes.clone();
                let root = self.runtime.host.root.clone();
                let (s, i, v) = (settings.clone(), id.to_owned(), value.to_owned());
                let target = tokio::task::spawn_blocking(move || routes.select(&root, &s, &i, &v))
                    .await
                    .map_err(|_| io::Error::other("route_reader_closed"))?
                    .map_err(err)?;
                if pre_task && target["echo"] == true {
                    // A host echo of the saved/pending route is not a change.
                    return Ok(options);
                }
                settings["model"] = target["route"].clone();
                settings["effort"] = json!(if id == "thought_level" { value } else { "" });
            }
            _ => return Err(io::Error::other("unknown_config_option")),
        }
        if route_option {
            self.runtime
                .routes
                .task_route(&self.runtime.host.root, &settings)
                .map_err(err)?;
        }
        let options = if let Some(execution) = b["execution"]
            .as_str()
            .filter(|_| b["state"] != "COMPLETED")
        {
            self.runtime
                .control(
                    self.project.clone(),
                    json!({"projectId":self.project,"executionId":execution,
                "requestId":identity()?,"action":"task_settings","settings":task_settings(&settings)}),
                )
                .await?;
            self.config_options(sid).await?
        } else {
            if id == "language" && b["task"].is_null() {
                // Python persists a pre-task language as the user default first.
                let routes = self.runtime.routes.clone();
                let language = (value != "auto").then(|| value.to_owned());
                tokio::task::spawn_blocking(move || {
                    routes.persist_default_language(language.as_deref())
                })
                .await
                .map_err(|_| io::Error::other("route_reader_closed"))?
                .map_err(err)?;
            }
            if route_option && pre_task {
                // Python persists a pre-task route choice as the user-default
                // route first, then shows it as this session's pending choice.
                let routes = self.runtime.routes.clone();
                let root = self.runtime.host.root.clone();
                let option = if id == "route" { "model" } else { id }.to_owned();
                let chosen = settings.clone();
                tokio::task::spawn_blocking(move || {
                    routes.persist_default_route(&root, &previous, &chosen, &option)
                })
                .await
                .map_err(|_| io::Error::other("route_reader_closed"))?
                .map_err(err)?;
                let pending = (
                    settings["model"].as_str().unwrap_or_default().to_owned(),
                    settings["effort"].as_str().unwrap_or_default().to_owned(),
                );
                self.views
                    .lock()
                    .unwrap()
                    .entry(sid.to_owned())
                    .or_default()
                    .pending = Some(pending);
                settings["state"] = json!("pending_next_use");
            }
            if pre_task {
                self.options_for(settings).await?
            } else {
                self.config_options(sid).await?
            }
        };
        self.record(
            sid,
            json!({"sessionUpdate":"config_option_update","configOptions":options}),
        )
        .await?;
        Ok(options)
    }
    /// Selectable routes for `/cortex-model`, from the owner registry.
    pub async fn listing(&self) -> io::Result<Vec<Value>> {
        let routes = self.runtime.routes.clone();
        let root = self.runtime.host.root.clone();
        tokio::task::spawn_blocking(move || routes.listing(&root))
            .await
            .map_err(|_| io::Error::other("route_reader_closed"))
    }
    pub async fn connection_ids(&self) -> io::Result<Vec<String>> {
        let routes = self.runtime.routes.clone();
        tokio::task::spawn_blocking(move || routes.connection_ids())
            .await
            .map_err(|_| io::Error::other("route_reader_closed"))
    }
    /// One bounded prompt-free catalogue observation of a connection.
    pub async fn refresh_catalog(&self, connection: &str) -> bool {
        let routes = self.runtime.routes.clone();
        let connection = connection.to_owned();
        tokio::task::spawn_blocking(move || routes.refresh_catalog(&connection))
            .await
            .unwrap_or(false)
    }
    pub async fn connection_states(&self) -> io::Result<Vec<String>> {
        let routes = self.runtime.routes.clone();
        tokio::task::spawn_blocking(move || routes.connection_states())
            .await
            .map_err(|_| io::Error::other("route_reader_closed"))
    }
    pub async fn record(&self, sid: &str, update: Value) -> io::Result<()> {
        self.session(sid).await?;
        let sid = sid.to_owned();
        recv(self.runtime.host.executor.write(move |db| {
            let seq:i64=db.query_row("SELECT coalesce(max(seq),-1)+1 FROM (
                SELECT seq FROM host_transcript WHERE session_id=? UNION ALL SELECT seq FROM host_updates WHERE session_id=?)",
                (&sid,&sid),|r|r.get(0))?;
            db.execute("INSERT INTO host_updates VALUES (?,?,?,?)",(&sid,seq,update.to_string(),cortex_core::now()))?;
            Ok(())
        }).map_err(err)?).await
    }
    pub async fn start(&self, sid: &str, text: &str) -> io::Result<()> {
        self.start_input(sid, text, None).await
    }
    pub async fn start_prompt(&self, sid: &str, blocks: Value) -> io::Result<()> {
        let root = self.runtime.host.root.clone();
        let (text, capture) =
            tokio::task::spawn_blocking(move || crate::incoming_prompt::capture(&root, &blocks))
                .await
                .map_err(|_| io::Error::other("input_reader_closed"))??;
        self.start_input(sid, &text, Some(capture)).await
    }
    async fn start_input(
        &self,
        sid: &str,
        text: &str,
        capture: Option<cortex_core::input::Capture>,
    ) -> io::Result<()> {
        let b = self.session(sid).await?;
        if b["state"] == "COMPLETED" {
            return Err(io::Error::other("completed_task_requires_new_session"));
        }
        if !b["task"].is_null() {
            // Attachments of a follow-up become the task's next source
            // revision, admitted together with its steering text.
            let request = json!({"action":"incoming_prompt","text":text,
                "expectedVersion":b["version"],"projectId":self.project,
                "executionId":b["execution"],"requestId":identity()?});
            return self
                .runtime
                .control_input(self.project.clone(), request, capture)
                .await
                .map(|_| ());
        }
        let settings = task_settings(&self.settings(sid).await?);
        // Missing routes fail before a draft, runtime task or source is created.
        self.runtime
            .routes
            .task_route(&self.runtime.host.root, &settings)
            .map_err(err)?;
        let digest = format!("{:x}", Sha256::digest(text.as_bytes()));
        let (create_key, start_key) = {
            let mut views = self.views.lock().unwrap();
            match views.get_mut(sid) {
                Some(view) => match &view.first_prompt {
                    Some((d, create, start)) if *d == digest => (create.clone(), start.clone()),
                    _ => {
                        let keys = (identity()?, identity()?);
                        view.first_prompt = Some((digest, keys.0.clone(), keys.1.clone()));
                        keys
                    }
                },
                None => (identity()?, identity()?),
            }
        };
        let create = self.command(
            "tasks.create",
            &self.project,
            create_key,
            json!({"description":text,"checkout":{"kind":"main"}}),
        );
        let created = self.runtime.command(self.project.clone(), create).await?;
        let draft = created["target"]
            .as_str()
            .ok_or_else(|| io::Error::other("native_admission_unknown"))?;
        let start = self.command("tasks.start", draft, start_key, json!({}));
        self.runtime
            .client_command(
                self.project.clone(),
                start,
                Some((sid.into(), settings, capture)),
            )
            .await?;
        if let Some(view) = self.views.lock().unwrap().get_mut(sid) {
            view.first_prompt = None;
        }
        Ok(())
    }
    fn command(&self, operation: &str, target: &str, key: String, payload: Value) -> Value {
        json!({"protocol":{"major":1,"minor":8},"projectId":self.project,"actorId":actor(),"requestId":key,
            "operation":operation,"target":target,"expectedVersion":if operation=="tasks.create"{"0"}else{"1"},
            "ownerEpoch":null,"payload":payload})
    }
    pub async fn control(&self, sid: &str, mut request: Value) -> io::Result<Value> {
        let b = self.session(sid).await?;
        if b["task"].is_null() {
            return Err(io::Error::other("session_has_no_task"));
        }
        request["projectId"] = json!(self.project);
        request["executionId"] = b["execution"].clone();
        if request.get("requestId").is_none() {
            request["requestId"] = json!(identity()?);
        }
        self.runtime.control(self.project.clone(), request).await
    }
    pub async fn queued_message(
        &self,
        sid: &str,
        message_id: &str,
        text: &str,
    ) -> io::Result<Value> {
        if message_id.is_empty()
            || message_id.len() > 512
            || text.trim().is_empty()
            || text.chars().count() > 32_000
        {
            return Err(io::Error::other("invalid_message"));
        }
        let key = format!("queue-{:x}", Sha256::digest(message_id.as_bytes()));
        self.control(
            sid,
            json!({"action":"incoming_prompt","requestId":key,"text":text}),
        )
        .await
    }
    pub async fn cancel(&self, sid: &str) -> io::Result<()> {
        let b = self.session(sid).await?;
        if b["task"].is_null()
            || matches!(
                b["state"].as_str(),
                Some("COMPLETED" | "FAILED" | "CANCELLED" | "PAUSED")
            )
        {
            return Ok(());
        }
        match self.control(sid, json!({"action":"pause"})).await {
            Ok(_) => Ok(()),
            // A concurrent /cortex-stop paused the task between the state check
            // and this pause: the turn is cancelled either way.
            Err(e) if e.to_string() == "already_paused" => Ok(()),
            Err(e) => Err(e),
        }
    }
    pub async fn decisions(&self, sid: &str) -> io::Result<Value> {
        let b = self.session(sid).await?;
        if b["task"].is_null() {
            return Ok(json!({"items":[]}));
        }
        self.runtime
            .decisions(
                self.project.clone(),
                json!({"operation":"decisions.list","target":b["execution"],"limit":100}),
            )
            .await
    }
    /// Attachments typed with an answer to an open question: retained as the
    /// task's next source revision before the answer is delivered, so the
    /// answered turn's engine prompt carries them. Returns whether any were kept.
    pub async fn retain_answer_attachments(&self, sid: &str, blocks: Value) -> io::Result<bool> {
        let b = self.session(sid).await?;
        let task = b["task"]
            .as_str()
            .ok_or_else(|| io::Error::other("session_has_no_task"))?
            .to_owned();
        let root = self.runtime.host.root.clone();
        let (text, capture) =
            tokio::task::spawn_blocking(move || crate::incoming_prompt::capture(&root, &blocks))
                .await
                .map_err(|_| io::Error::other("input_reader_closed"))??;
        if capture.blocks.is_empty() {
            return Ok(false);
        }
        let session = sid.to_owned();
        self.runtime
            .host
            .executor
            .publication(move |s| {
                cortex_core::input::retain_for_answer(s, &task, &session, &text, &capture)
            })
            .map_err(err)?
            .await
            .map_err(|_| io::Error::other("native_host_closed"))?
            .map_err(err)?;
        Ok(true)
    }
    pub async fn answer(&self, sid: &str, question: &str, answer: &str) -> io::Result<()> {
        let b = self.session(sid).await?;
        let decisions = self.decisions(sid).await?;
        let d = decisions["items"]
            .as_array()
            .and_then(|a| a.iter().find(|d| d["questionId"] == question))
            .ok_or_else(|| io::Error::other("unknown_question"))?;
        // Version/epoch are revalidated by Core in the answer transaction.
        let mut command=self.command("decisions.answer",d["decisionId"].as_str().unwrap(),identity()?,
            json!({"executionId":b["execution"],"acpSessionId":b["logicalSession"],"batchId":null,
                "answers":[{"decisionId":d["decisionId"],"questionId":question,"expectedVersion":d["version"],"optionId":null,"text":answer}]}));
        command["ownerEpoch"] = b["epoch"].clone();
        command["expectedVersion"] = d["version"].clone();
        self.runtime
            .answer_command_from(self.project.clone(), command, Some(sid.to_owned()))
            .await
            .map(|_| ())
    }
    /// A complete batch is validated and committed by Core in one transaction.
    /// One ACP permission choice can never resolve a multi-question batch.
    pub async fn answer_payload(&self, sid: &str, payload: &Value) -> io::Result<()> {
        let object = payload
            .as_object()
            .ok_or_else(|| io::Error::other("invalid_question_answer"))?;
        if !object.contains_key("batchId") {
            if object
                .keys()
                .any(|k| !["questionId", "answer", "requestKey"].contains(&k.as_str()))
            {
                return Err(io::Error::other("invalid_question_answer"));
            }
            let q = payload["questionId"]
                .as_str()
                .ok_or_else(|| io::Error::other("invalid_question_answer"))?;
            let text = payload["answer"]
                .as_str()
                .ok_or_else(|| io::Error::other("invalid_question_answer"))?;
            return self.answer(sid, q, text).await;
        }
        if object.len() != 3
            || !object.contains_key("answers")
            || !object.contains_key("requestKey")
        {
            return Err(io::Error::other("invalid_question_answer"));
        }
        let batch = payload["batchId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| io::Error::other("invalid_question_answer"))?;
        let key = payload["requestKey"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 512)
            .ok_or_else(|| io::Error::other("invalid_question_answer"))?;
        let answers = payload["answers"]
            .as_array()
            .filter(|a| !a.is_empty() && a.len() <= 8)
            .ok_or_else(|| io::Error::other("invalid_question_answer"))?;
        let b = self.session(sid).await?;
        let decisions = self.decisions(sid).await?;
        let items = decisions["items"].as_array().unwrap();
        let mut converted = Vec::new();
        for a in answers {
            if a.as_object().is_none_or(|o| o.len() != 2) {
                return Err(io::Error::other("invalid_question_answer"));
            }
            let d = items
                .iter()
                .find(|d| d["batchId"] == batch && d["questionId"] == a["questionId"])
                .ok_or_else(|| io::Error::other("unknown_question"))?;
            let text = a["answer"]
                .as_str()
                .filter(|s| !s.trim().is_empty() && s.chars().count() <= 8000)
                .ok_or_else(|| io::Error::other("invalid_question_answer"))?;
            converted.push(
                json!({"decisionId":d["decisionId"],"questionId":d["questionId"],
                "expectedVersion":d["version"],"optionId":null,"text":text}),
            );
        }
        let mut command=self.command("decisions.answer",batch,key.into(),
            json!({"executionId":b["execution"],"acpSessionId":b["logicalSession"],"batchId":batch,"answers":converted}));
        command["ownerEpoch"] = b["epoch"].clone();
        command["expectedVersion"] = command["payload"]["answers"][0]["expectedVersion"].clone();
        self.runtime
            .answer_command_from(self.project.clone(), command, Some(sid.to_owned()))
            .await
            .map(|_| ())
    }
    /// Python `cleanup.clear_tasks(dry_run=True)` selection, read-only: tasks
    /// and task-less sessions of this project whose activity precedes `cutoff`.
    /// `sessions` are the connection's loaded sessions (soft protection unless
    /// `force`); the invoking session and its task are always kept.
    pub async fn clear_preview(
        &self,
        sid: &str,
        cutoff: String,
        force: bool,
        sessions: Vec<String>,
    ) -> io::Result<Value> {
        self.session(sid).await?;
        let sid = sid.to_owned();
        let root = self.runtime.host.root.to_string_lossy().into_owned();
        recv(self.runtime.host.executor.read(move |db| {
            let tables = db.prepare("SELECT name FROM sqlite_master WHERE type='table'")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<std::collections::HashSet<_>, _>>()?;
            let column = |sql: &str, p: &[&dyn rusqlite::ToSql]| -> CoreResult<Vec<String>> {
                Ok(db.prepare(sql)?.query_map(p, |r| r.get::<_, Option<String>>(0))?
                    .collect::<Result<Vec<_>, _>>()?.into_iter().flatten().collect())
            };
            let rows = column("SELECT id FROM (SELECT t.id,t.project_root,MAX(t.created_at,
                COALESCE((SELECT MAX(e.updated_at) FROM reports r JOIN editions e ON e.report_id=r.id WHERE r.task_id=t.id),t.created_at)) AS activity
                FROM tasks t) WHERE project_root=? AND activity<?", &[&root, &cutoff])?;
            let mut keep = std::collections::HashSet::new();
            if tables.contains("thread_bindings") {
                keep.extend(column("SELECT task_id FROM thread_bindings WHERE thread_id=?", &[&sid])?);
            }
            keep.extend(column("SELECT task_id FROM acp_host_sessions WHERE session_id=?", &[&sid])?);
            if !force {
                for session in &sessions {
                    keep.extend(column("SELECT task_id FROM acp_host_sessions WHERE session_id=?", &[session])?);
                }
                // Python `protected_active_tasks`: non-terminal or ambiguously live work.
                keep.extend(column("SELECT task_id FROM runtime_tasks WHERE state NOT IN ('COMPLETED','FAILED','CANCELLED')", &[])?);
                if tables.contains("capacity_reservations") {
                    keep.extend(column("SELECT task_id FROM capacity_reservations WHERE state!='released'", &[])?);
                }
                if tables.contains("attempt_process_registry") {
                    keep.extend(column("SELECT task_id FROM attempt_process_registry WHERE terminated_at IS NULL", &[])?);
                }
            }
            let pending = if tables.contains("pending_deletions") {
                column("SELECT d.task_id FROM pending_deletions d JOIN tasks t ON t.id=d.task_id WHERE t.project_root=?", &[&root])?
            } else { Vec::new() };
            if pending.iter().any(|t| keep.contains(t)) {
                return Err(CoreError::Refused("retention_active_conflict"));
            }
            let mut doomed: std::collections::HashSet<_> = pending.into_iter().collect();
            doomed.extend(rows.iter().filter(|t| !keep.contains(*t)).cloned());
            let mut session_keep: std::collections::HashSet<String> = [sid.clone()].into();
            if !force {
                session_keep.extend(sessions);
            }
            let mut activity = vec!["s.created_at".to_owned(), "COALESCE(s.closed_at,s.created_at)".to_owned()];
            for table in ["host_transcript", "host_updates"] {
                if tables.contains(table) {
                    activity.push(format!("COALESCE((SELECT MAX(created_at) FROM {table} WHERE session_id=s.session_id),s.created_at)"));
                }
            }
            let orphans = column(&format!("SELECT s.session_id FROM acp_host_sessions s
                WHERE s.project_root=? AND (s.task_id IS NULL OR NOT EXISTS (SELECT 1 FROM tasks t WHERE t.id=s.task_id))
                AND MAX({})<?", activity.join(",")), &[&root, &cutoff])?
                .into_iter().filter(|s| !session_keep.contains(s)).count();
            Ok(json!({"tasks":doomed.len(),"sessions":orphans,
                "skipped":rows.iter().filter(|t| keep.contains(*t)).count()}))
        }).map_err(err)?).await
    }
    /// Python `/cortex-new-task`: a separate task the invoking session never
    /// binds. A missing route refuses before any session, draft or task exists.
    pub async fn new_task(&self, parent: &str, text: &str) -> io::Result<String> {
        let parent_settings = self.settings(parent).await?;
        let mode = cortex_core::access::normalize(parent_settings["mode"].as_str().unwrap_or(""))
            .to_owned();
        // A separate task uses the saved route, but never a broader access mode
        // or a different language than the invoking client selected.
        let settings =
            json!({"model":"","effort":"","mode":mode,"language":parent_settings["language"]});
        self.runtime
            .routes
            .task_route(&self.runtime.host.root, &settings)
            .map_err(err)?;
        let hidden = self
            .new_session_with_mode(json!({"newTaskFrom":parent}), json!({}), mode)
            .await?;
        let options = self.options_for(settings).await?;
        self.record(
            &hidden,
            json!({"sessionUpdate":"config_option_update","configOptions":options}),
        )
        .await?;
        self.start(&hidden, text).await?;
        Ok(hidden)
    }
    /// A durable Pause alone is not evidence that a child process has exited.
    pub async fn settled(&self, sid: &str) -> io::Result<()> {
        let b = self.session(sid).await?;
        let Some(task) = b["task"].as_str() else {
            return Ok(());
        };
        self.runtime.settle_task(task).await.map_err(err)?;
        let task = task.to_owned();
        recv(self.runtime.host.executor.read(move|db|{
            let unknown:bool=db.query_row("SELECT
                EXISTS(SELECT 1 FROM capacity_reservations WHERE task_id=? AND state!='released')
                OR EXISTS(SELECT 1 FROM delegation_provisioning WHERE task_id=? AND status!='terminated')",
                (&task,&task),|r|r.get(0))?;
            if unknown {return Err(CoreError::Refused("native_shutdown_unconfirmed"));}
            Ok(())
        }).map_err(err)?).await
    }
    pub async fn shutdown(&self) {
        self.runtime.shutdown().await;
    }
}
