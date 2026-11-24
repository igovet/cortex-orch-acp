//! Incoming ACP v1 transport. All task effects belong to workspace-service's
//! shared native host; this crate never opens a Store or constructs a scheduler.
pub mod adapters;
pub mod automatic;
mod commands;
mod frames;
mod pyjson;
mod python_protocol;
mod registry;
pub mod startup;
mod task_commands;
use cortex_engine::{engine::PermissionDecision, wire, FutureResult};
/// Python `MAX_TRACKED_REQUEST_IDS`: the most recent ids kept for the
/// duplicate `session/prompt` check; older ids are forgotten, not refused.
const MAX_TRACKED_REQUEST_IDS: usize = 10_000;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufRead, AsyncWrite},
    sync::{mpsc, oneshot, watch, Mutex},
    task::JoinSet,
};
use workspace_service::{
    incoming::IncomingHost, native_runtime::NativeRoutes, native_sessions::ClientPermissions,
};

fn invalid(message: &str) -> io::Error {
    io::Error::other(message.to_owned())
}
fn string<'a>(v: &'a Value, k: &str) -> io::Result<&'a str> {
    v[k].as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| invalid("invalid_params"))
}
struct PendingPermission {
    session: String,
    /// Question ids of a Cortex question card; empty for engine permissions.
    questions: Vec<String>,
    send: oneshot::Sender<Value>,
}
/// The question ids a Cortex question card carries (one, or a batch's members).
fn card_questions(body: &Value) -> Vec<String> {
    if body["_meta"]["cortexQuestion"] != true {
        return Vec::new();
    }
    let input = &body["toolCall"]["rawInput"];
    match input["questions"].as_array() {
        Some(members) => members
            .iter()
            .filter_map(|m| m["questionId"].as_str().map(str::to_owned))
            .collect(),
        None => input["questionId"]
            .as_str()
            .map(|q| vec![q.to_owned()])
            .unwrap_or_default(),
    }
}
struct Permissions {
    output: mpsc::Sender<Value>,
    next: AtomicU64,
    pending: Mutex<HashMap<String, PendingPermission>>,
}
impl Permissions {
    async fn request(&self, sid: &str, mut body: Value) -> io::Result<Value> {
        body["sessionId"] = json!(sid);
        let id = format!(
            "cortex-permission-{}",
            self.next.fetch_add(1, Ordering::Relaxed)
        );
        let (send, reply) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.len() >= 128 {
                return Err(invalid("permission_capacity"));
            }
            pending.insert(
                id.clone(),
                PendingPermission {
                    session: sid.into(),
                    questions: card_questions(&body),
                    send,
                },
            );
        }
        if self.output.send(json!({"jsonrpc":"2.0","id":id,"method":"session/request_permission","params":body})).await.is_err(){
            self.pending.lock().await.remove(&id);
            return Err(invalid("client_closed"));
        }
        // Human decisions have no transport deadline. Keep the correlation until
        // a response, card retirement, session cancellation or connection shutdown.
        let result = reply.await;
        self.pending.lock().await.remove(&id);
        result.map_err(|_| invalid("permission_cancelled"))
    }
    async fn question(
        &self,
        sid: &str,
        body: Value,
        cancelled: &watch::Receiver<u64>,
    ) -> io::Result<Value> {
        // A card derived before cancel must not open a fresh wait after cancel
        // cleared the older pending map. Keep the prompt's admission generation.
        if cancelled.has_changed().unwrap_or(true) {
            return Err(invalid("permission_cancelled"));
        }
        let mut changed = cancelled.clone();
        let result = tokio::select! {
            biased;
            _ = changed.changed() => Err(invalid("permission_cancelled")),
            result = self.request(sid, body) => result,
        };
        if cancelled.has_changed().unwrap_or(true)
            || result
                .as_ref()
                .is_err_and(|e| e.to_string() == "permission_cancelled")
        {
            self.cancel(sid).await;
            return Err(invalid("permission_cancelled"));
        }
        result
    }
    async fn cancel(&self, sid: &str) {
        self.pending.lock().await.retain(|_, p| p.session != sid);
    }
    /// Whether this session shows an open card for this question.
    async fn shows_question(&self, sid: &str, question: &str) -> bool {
        self.pending
            .lock()
            .await
            .values()
            .any(|p| p.session == sid && p.questions.iter().any(|q| q == question))
    }
    /// Retire only the card of an answered question; engine permission
    /// requests and other cards of the session stay open.
    async fn retire_question(&self, sid: &str, question: &str) {
        let mut pending = self.pending.lock().await;
        let retired: Vec<_> = pending
            .iter()
            .filter(|(_, p)| p.session == sid && p.questions.iter().any(|q| q == question))
            .map(|(id, _)| id.clone())
            .collect();
        for id in retired {
            if let Some(p) = pending.remove(&id) {
                // A not-selected outcome ends that card's wait without the
                // cancellation path that clears every card of the session.
                let _ = p.send.send(json!({"outcome":{"outcome":"cancelled"}}));
            }
        }
    }
    async fn respond(&self, frame: &Value) {
        if let Some(id) = frame["id"].as_str() {
            if let Some(p) = self.pending.lock().await.remove(id) {
                let _ = p
                    .send
                    .send(frame.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }
}
impl ClientPermissions for Permissions {
    fn decide<'a>(
        &'a self,
        sid: &'a str,
        request: &'a Value,
    ) -> FutureResult<'a, PermissionDecision> {
        Box::pin(async move {
            let result = self.request(sid, request.clone()).await;
            let Ok(result) = result else {
                return Ok(PermissionDecision::Cancelled);
            };
            if result["outcome"]["outcome"] != "selected" {
                return Ok(PermissionDecision::Cancelled);
            }
            let Some(id) = result["outcome"]["optionId"].as_str() else {
                return Ok(PermissionDecision::Cancelled);
            };
            // Persistent grants cannot be synthesized by this adapter. Only an
            // exact one-shot choice offered for this request is eligible.
            if request["options"].as_array().is_some_and(|a| {
                a.iter().any(|o| {
                    o["optionId"] == id
                        && matches!(o["kind"].as_str(), Some("allow_once" | "reject_once"))
                })
            }) {
                Ok(PermissionDecision::Selected(id.into()))
            } else {
                Ok(PermissionDecision::Cancelled)
            }
        })
    }
}
#[cfg(test)]
mod cancellation_tests {
    use super::*;
    fn permissions() -> (Arc<Permissions>, mpsc::Receiver<Value>) {
        let (output, read) = mpsc::channel(8);
        (
            Arc::new(Permissions {
                output,
                next: AtomicU64::new(0),
                pending: Mutex::new(HashMap::new()),
            }),
            read,
        )
    }
    #[tokio::test]
    async fn cancelled_prompt_never_opens_a_late_question_and_new_prompt_can_ask() {
        let (p, mut client) = permissions();
        let (cancel, old) = watch::channel(0);
        cancel.send_replace(1);
        assert!(p.question("s", json!({}), &old).await.is_err());
        assert!(client.try_recv().is_err());
        assert!(p.pending.lock().await.is_empty());
        let fresh = cancel.subscribe();
        let request = p.question("s", json!({}), &fresh);
        tokio::pin!(request);
        let frame = tokio::select! { _=&mut request=>panic!("question ended without a client reply"),frame=client.recv()=>frame.unwrap() };
        p.respond(&json!({"id":frame["id"],"result":{"outcome":{"outcome":"selected","optionId":"answer"}}})).await;
        assert_eq!(request.await.unwrap()["outcome"]["optionId"], "answer");
    }
    #[tokio::test(start_paused = true)]
    async fn delayed_question_response_keeps_its_correlation() {
        let (p, mut client) = permissions();
        let (_cancel, generation) = watch::channel(0);
        let body = json!({"_meta":{"cortexQuestion":true},
            "toolCall":{"rawInput":{"questionId":"q"}}});
        let request = p.question("s", body, &generation);
        tokio::pin!(request);
        let frame = tokio::select! { _=&mut request=>panic!("question ended without a client reply"),frame=client.recv()=>frame.unwrap() };
        tokio::time::advance(Duration::from_secs(31)).await;
        assert!(tokio::time::timeout(Duration::from_millis(1), &mut request)
            .await
            .is_err());
        assert!(p.shows_question("s", "q").await);
        p.respond(&json!({"id":frame["id"],"result":{"outcome":{"outcome":"selected","optionId":"answer"}}})).await;
        assert_eq!(request.await.unwrap()["outcome"]["optionId"], "answer");
        assert!(p.pending.lock().await.is_empty());
    }
    #[test]
    fn language_of_question_fallback_does_not_create_a_selectable_answer() {
        let decision = json!({"questionId":"exact-id","prompt":"Точный текст?"});
        let response = json!({"outcome":{"outcome":"selected","optionId":"cortex-question-1"}});
        for (ru, label) in [
            (false, "Reply in chat or type an answer under Other…"),
            (true, "Ответьте в чате или введите ответ в поле «Другое»…"),
        ] {
            let card = crate::question_card(&decision, &[], ru);
            assert_eq!(card["toolCall"]["title"], "Точный текст?");
            assert_eq!(card["options"][0]["name"], label);
            assert_eq!(card["options"][0]["optionId"], "cortex-question-1");
            assert_eq!(crate::card_choice(&response, &[]), None);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn delayed_engine_permission_response_keeps_its_correlation() {
        let (p, mut client) = permissions();
        let body = json!({"options":[{"optionId":"allow","kind":"allow_once"}]});
        let request = p.decide("s", &body);
        tokio::pin!(request);
        let frame = tokio::select! { _=&mut request=>panic!("permission ended without a client reply"),frame=client.recv()=>frame.unwrap() };
        tokio::time::advance(Duration::from_secs(31)).await;
        assert!(tokio::time::timeout(Duration::from_millis(1), &mut request)
            .await
            .is_err());
        p.respond(&json!({"id":frame["id"],"result":{"outcome":{"outcome":"selected","optionId":"allow"}}})).await;
        assert!(
            matches!(request.await.unwrap(), PermissionDecision::Selected(id) if id == "allow")
        );
        assert!(p.pending.lock().await.is_empty());
    }
    #[tokio::test(start_paused = true)]
    async fn cancel_interrupts_open_question_and_fences_every_later_card_of_that_prompt() {
        let (p, mut client) = permissions();
        let (cancel, generation) = watch::channel(0);
        let request = p.question("s", json!({}), &generation);
        tokio::pin!(request);
        let frame = tokio::select! { _=&mut request=>panic!("question ended without a client reply"),frame=client.recv()=>frame.unwrap() };
        tokio::time::advance(Duration::from_secs(31)).await;
        assert!(tokio::time::timeout(Duration::from_millis(1), &mut request)
            .await
            .is_err());
        cancel.send_replace(1);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), &mut request)
                .await
                .unwrap()
                .is_err()
        );
        drop(request);
        assert!(p.pending.lock().await.is_empty());
        p.respond(&json!({"id":frame["id"],"result":{"outcome":{"outcome":"selected","optionId":"stale"}}})).await;
        assert!(p.question("s", json!({}), &generation).await.is_err());
        assert!(client.try_recv().is_err());
    }
}
/// Python `DEFAULT_HOST_CANCEL_GRACE_SECONDS`.
const HOST_CANCEL_GRACE: Duration = Duration::from_secs(5);
/// Python `_CancelSteeringHold`: Paseo sends `session/cancel` before every
/// follow-up prompt, and ACP cannot tell that from a Stop click. The cancelled
/// turn ends at once without pausing its task; a prompt on the same session
/// within the grace window is steering, and a lone cancel pauses on expiry.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Hold {
    #[default]
    Idle,
    /// A cancel reached a running Paseo prompt.
    Armed,
    /// That prompt's turn ended; the grace window with this token is open.
    Waiting(u64),
}
struct ClientSession {
    host: Arc<IncomingHost>,
    busy: AtomicBool,
    cancellation: watch::Sender<u64>,
    configuration: Mutex<()>,
    paseo: bool,
    hold: std::sync::Mutex<(Hold, u64)>,
}
impl ClientSession {
    fn new(host: Arc<IncomingHost>, paseo: bool) -> Self {
        Self {
            host,
            busy: AtomicBool::new(false),
            cancellation: watch::channel(0).0,
            configuration: Mutex::new(()),
            paseo,
            hold: std::sync::Mutex::new((Hold::Idle, 0)),
        }
    }
    fn with_hold<T>(&self, f: impl FnOnce(&mut (Hold, u64)) -> T) -> T {
        f(&mut self.hold.lock().unwrap_or_else(|e| e.into_inner()))
    }
    /// `session/cancel` of a running Paseo prompt holds the cancel.
    fn arm_hold(&self) {
        if self.paseo && self.busy.load(Ordering::Acquire) {
            self.with_hold(|h| {
                if h.0 == Hold::Idle {
                    h.0 = Hold::Armed;
                }
            });
        }
    }
    /// A turn observing a held cancel ends without pausing its task.
    fn hold_turn(&self) -> bool {
        self.with_hold(|h| {
            if h.0 != Hold::Armed {
                return false;
            }
            h.1 += 1;
            h.0 = Hold::Waiting(h.1);
            true
        })
    }
    /// After the prompt replied: the open grace window, if any. An armed hold
    /// whose prompt ended without a held turn is dropped.
    fn settle_hold(&self) -> Option<u64> {
        self.with_hold(|h| match h.0 {
            Hold::Waiting(token) => Some(token),
            _ => {
                h.0 = Hold::Idle;
                None
            }
        })
    }
    /// A prompt on the cancelled session turns an open grace window into
    /// steering; a cancel of this prompt itself stays armed.
    fn steer_hold(&self) {
        self.with_hold(|h| {
            if matches!(h.0, Hold::Waiting(_)) {
                h.0 = Hold::Idle;
            }
        });
    }
    /// Whether a cancelled Paseo prompt is still releasing this session.
    fn hold_pending(&self) -> bool {
        self.with_hold(|h| h.0 != Hold::Idle)
    }
    /// Grace window over without a follow-up prompt.
    fn expire_hold(&self, token: u64) -> bool {
        self.with_hold(|h| {
            let expired = h.0 == Hold::Waiting(token);
            if expired {
                h.0 = Hold::Idle;
            }
            expired
        })
    }
    /// Release the prompt lane; `Some` names a held cancel's grace window.
    fn release(&self) -> Option<u64> {
        self.busy.store(false, Ordering::Release);
        self.settle_hold()
    }
    /// After the reply: a lone held cancel pauses once its window expires.
    async fn expire_after_grace(&self, sid: &str, token: Option<u64>) {
        if let Some(token) = token {
            tokio::time::sleep(HOST_CANCEL_GRACE).await;
            if self.expire_hold(token) {
                let _ = self.host.cancel(sid).await;
            }
        }
    }
    async fn config_options(&self, sid: &str) -> io::Result<Value> {
        Ok(client_options(
            self.host.config_options(sid).await?,
            self.paseo,
        ))
    }
    fn project_update(&self, mut value: Value) -> Value {
        if value["sessionUpdate"] == "config_option_update" {
            value["configOptions"] = client_options(value["configOptions"].take(), self.paseo);
        }
        value
    }
}
fn client_options(mut options: Value, paseo: bool) -> Value {
    if let Some(items) = options.as_array_mut() {
        items.retain(|o| !paseo || o["id"] != "access_mode");
        for option in items {
            match option["id"].as_str() {
                Some("language") => {
                    option["category"] = json!(if paseo { "mode" } else { "other" });
                    if paseo {
                        option["name"] = json!("Coordinator language");
                    }
                }
                Some("access_mode") => option["category"] = json!("mode"),
                _ => {}
            }
        }
    }
    options
}
struct Agent {
    routes: Arc<dyn NativeRoutes>,
    hosts: HashMap<PathBuf, Arc<IncomingHost>>,
    sessions: HashMap<String, Arc<ClientSession>>,
    legacy_sessions: HashSet<String>,
    info: Value,
    capabilities: Value,
    initialized: bool,
    output: mpsc::Sender<Value>,
    permissions: Arc<Permissions>,
    seen: HashSet<String>,
    seen_order: VecDeque<String>,
    /// Detached per-session catalogue refreshes; aborted, never awaited, at EOF.
    refreshes: JoinSet<()>,
}
impl Agent {
    async fn send(&self, v: Value) -> io::Result<()> {
        self.output
            .send(v)
            .await
            .map_err(|_| invalid("client_closed"))
    }
    async fn host(&mut self, params: &Value) -> io::Result<Arc<IncomingHost>> {
        let root = PathBuf::from(string(params, "cwd")?);
        if !root.is_absolute() || !root.is_dir() || root.canonicalize()? != root {
            return Err(invalid("invalid_cwd"));
        }
        if let Some(host) = self.hosts.get(&root) {
            return Ok(host.clone());
        }
        if self.hosts.len() >= 16 {
            return Err(invalid("project_capacity"));
        }
        let routes = self.routes.clone();
        let path = root.clone();
        let host = Arc::new(
            tokio::task::spawn_blocking(move || IncomingHost::open(&path, routes))
                .await
                .map_err(|_| invalid("host_open_failed"))??,
        );
        host.bind_permissions(self.permissions.clone());
        self.hosts.insert(root, host.clone());
        Ok(host)
    }
    /// Python `self.sessions`: every session this connection has open or loaded.
    fn known_sessions(&self) -> Vec<String> {
        self.sessions
            .keys()
            .chain(self.legacy_sessions.iter())
            .cloned()
            .collect()
    }
    fn session(&self, p: &Value) -> io::Result<(String, Arc<ClientSession>)> {
        let sid = string(p, "sessionId")?;
        if self.legacy_sessions.contains(sid) {
            return Err(invalid("legacy_session_read_only"));
        }
        Ok((
            sid.into(),
            self.sessions
                .get(sid)
                .cloned()
                .ok_or_else(|| invalid("unknown_session"))?,
        ))
    }
    async fn dispatch(&mut self, frame: Value, jobs: &mut JoinSet<()>) -> io::Result<()> {
        // Python `handle_message`; the frame reader already answered malformed
        // lines, a wrong `jsonrpc` and a non-string `method`.
        let id = frame.get("id").cloned();
        let Some(method) = frame["method"].as_str() else {
            if id.is_some() && (frame.get("result").is_some() || frame.get("error").is_some()) {
                self.permissions.respond(&frame).await;
            } else if let Some(id) = id {
                self.send(error(id, -32600, "Missing or invalid \"method\"."))
                    .await?;
            }
            return Ok(());
        };
        if method != "initialize" && !self.initialized {
            if let Some(id) = id {
                self.send(error(
                    id,
                    -32600,
                    "Call initialize before any other method.",
                ))
                .await?;
            }
            return Ok(());
        }
        let p = frame.get("params").cloned().unwrap_or(json!({}));
        if let Some(id) = &id {
            // Python `_track_request_id`: every id is recorded (exact JSON
            // text keeps types distinct) but only a resent prompt refuses.
            let key = id.to_string();
            if self.seen.insert(key.clone()) {
                self.seen_order.push_back(key);
                if self.seen_order.len() > MAX_TRACKED_REQUEST_IDS {
                    if let Some(old) = self.seen_order.pop_front() {
                        self.seen.remove(&old);
                    }
                }
            } else if method == "session/prompt" {
                return self
                    .send(error(id.clone(), -32600, "duplicate_request_id"))
                    .await;
            }
        }
        let response=async {
            if method!="initialize" && !self.initialized {return Err(invalid("not_initialized"));}
            if !p.is_object(){return Err(invalid("invalid_params"));}
            if id.is_none() && method!="session/cancel" {return Err(invalid("request_id_required"));}
            python_protocol::validate(method,&p)?;
            match method {
                "initialize"=>{
                    if !p["protocolVersion"].is_i64() && !p["protocolVersion"].is_u64(){return Err(invalid("initialize requires an integer protocolVersion."));}
                    self.info=p.get("clientInfo").cloned().unwrap_or(json!({}));
                    self.capabilities=p.get("clientCapabilities").cloned().unwrap_or(json!({}));
                    self.initialized=true;
                    Ok(json!({"protocolVersion":1,"agentCapabilities":{"loadSession":true,
                        "promptCapabilities":{"image":true,"audio":false,"embeddedContext":true},
                        "mcpCapabilities":{"http":false,"sse":false}},"authMethods":[],
                        "agentInfo":{"name":"cortex","version":env!("CARGO_PKG_VERSION")},"_meta":{"cortexChannel":workspace_protocol::channel::current().name}}))
                },
                "authenticate"=>Err(invalid("Method 'authenticate' is not supported.")),
                "session/new"|"session/load"=>{
                    if self.sessions.len()>=256{return Err(invalid("session_capacity"));}
                    if p.get("mcpServers").is_some_and(|v|!v.is_null()&&!v.is_array()){
                        return Err(invalid("invalid_mcp_servers"));
                    }
                    if p.get("mcpServers").and_then(Value::as_array).is_some_and(|a|!a.is_empty()) {
                        let registry=self.routes.mcp_registry().map_err(|e|invalid(e.code()))?;
                        registry.validate_client(&p["mcpServers"]).map_err(|e|invalid(e.code()))?;
                    }
                    if method=="session/load" {
                        let root=PathBuf::from(string(&p,"cwd")?);
                        if !root.is_absolute() || !root.canonicalize().is_ok_and(|real|real==root) {return Err(invalid("invalid_cwd"));}
                        let sid=string(&p,"sessionId")?.to_owned();
                        let s=sid.clone();
                        let history=tokio::task::spawn_blocking(move||workspace_service::incoming_history::LegacyHistory::open(&root,&s))
                            .await.map_err(|_|invalid("history_reader_closed"))??;
                        if let Some(history)=history {
                            let history=Arc::new(history);
                            let mut after=-1;
                            loop {
                                let h=history.clone();
                                let rows=tokio::task::spawn_blocking(move||h.page(after)).await.map_err(|_|invalid("history_reader_closed"))??;
                                if rows.is_empty(){break;}
                                for row in rows {after=row["seq"].as_i64().unwrap();let mut replay=row["update"].clone(); replay["_meta"]["cortexReplay"]=json!({"sessionId":sid,"sequences":[after],"textBytes":replay["content"]["text"].as_str().map(str::len).unwrap_or(0)});self.send(update(&sid,replay)).await?;}
                            }
                            self.legacy_sessions.insert(sid);
                            // Python `_session_load` options, read-only: the task's
                            // latest live revision (none for a task-less session).
                            let paseo=self.info["name"].as_str().is_some_and(|n|n.to_lowercase()=="paseo");
                            let h=history.clone();
                            let mut settings=tokio::task::spawn_blocking(move||h.selection()).await.map_err(|_|invalid("history_reader_closed"))??;
                            settings["combined"]=json!(paseo);
                            if settings["language"].is_null() {
                                let routes=self.routes.clone();
                                settings["language"]=json!(tokio::task::spawn_blocking(move||routes.default_language()).await
                                    .map_err(|_|invalid("route_reader_closed"))?.unwrap_or_else(||"auto".into()));
                            }
                            let root=PathBuf::from(string(&p,"cwd")?);
                            let options=client_options(workspace_service::incoming::presentation(self.routes.clone(),root,settings.clone()).await?,paseo);
                            let mut result=json!({"configOptions":options});
                            result["modes"]=if paseo {workspace_service::incoming::modes(settings["mode"].as_str().unwrap_or("guarded"))} else {Value::Null};
                            return Ok(result);
                        }
                    }
                    let host=self.host(&p).await?;
                    host.validate_mcp_servers(&p["mcpServers"])?;
                    let sid=if method=="session/new"{host.recover().await?;host.new_session(self.info.clone(),self.capabilities.clone()).await?}
                        else{let s=string(&p,"sessionId")?.to_owned();host.session(&s).await?;host.recover_legacy().await?;s};
                    if self.sessions.get(&sid).is_some_and(|s|s.busy.load(Ordering::Acquire)){return Err(invalid("session_busy"));}
                    let paseo=self.info["name"].as_str().is_some_and(|n|n.to_lowercase()=="paseo");
                    host.bind_client(&sid,paseo,method=="session/load");
                    self.sessions.insert(sid.clone(),Arc::new(ClientSession::new(host.clone(),paseo)));
                    if method=="session/load" {
                        let mut after=-1;
                        loop {
                            let rows=host.history(&sid,after).await?;
                            if rows.is_empty(){break;}
                            for row in rows {after=row["seq"].as_i64().unwrap();let mut replay=self.sessions[&sid].project_update(row["update"].clone()); replay["_meta"]["cortexReplay"]=json!({"sessionId":sid,"sequences":[after],"textBytes":replay["content"]["text"].as_str().map(str::len).unwrap_or(0)});self.send(update(&sid,replay)).await?;}
                        }
                    }
                    let options=client_options(host.config_options(&sid).await?,paseo);
                    let mut result=json!({"configOptions":options});
                    if method=="session/new" {result["sessionId"]=json!(sid);}
                    // Python's load reply always carries `modes` (null outside Paseo).
                    if paseo {result["modes"]=host.modes(&sid).await?;} else if method=="session/load" {result["modes"]=Value::Null;}
                    if let Some(id)=id.clone(){self.send(reply(id,result)).await?;}
                    // Python `commands.AVAILABLE_COMMANDS`, restricted to the
                    // commands this host implements, in Python's order and copy.
                    self.send(update(&sid,json!({"sessionUpdate":"available_commands_update","availableCommands":task_commands::available()}))).await?;
                    if method=="session/new" {
                        // Python `_start_catalog_refresh`: after the reply, one
                        // detached prompt-free refresh per registered connection;
                        // only a published observation sends fresh options.
                        while self.refreshes.try_join_next().is_some() {}
                        for connection in host.connection_ids().await.unwrap_or_default() {
                            if self.refreshes.len()>=64 {break;}
                            let (session,out,sid)=(self.sessions[&sid].clone(),self.output.clone(),sid.clone());
                            self.refreshes.spawn(async move {
                                if session.host.refresh_catalog(&connection).await {
                                    if let Ok(options)=session.config_options(&sid).await {
                                        let _=out.send(update(&sid,json!({"sessionUpdate":"config_option_update","configOptions":options}))).await;
                                    }
                                }
                            });
                        }
                    }
                    return Ok(Value::Null);
                },
                "session/prompt"=>{
                    let (sid,session)=self.session(&p)?;
                    let blocks=p["prompt"].clone();
                    let Some(id)=id.clone() else {return Err(invalid("prompt_requires_request"));};
                    let output=self.output.clone();
                    let permissions=self.permissions.clone();
                    let known=self.known_sessions();
                    let name=commands::recognize(&blocks).map(|r|r.0);
                    if name==Some("cortex-kill") {
                        // Python handles a command stop as control input, even
                        // while a turn or question is in flight, with its own token.
                        jobs.spawn(async move{
                            let (_held,cancelled)=watch::channel(0);
                            let result=prompt(&sid,&id,&session,blocks,&output,&permissions,cancelled,known).await;
                            let _=output.send(respond(id,result)).await;
                        });
                        return Ok(Value::Null);
                    }
                    if session.busy.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err(){
                        if session.hold_pending() && !matches!(name,Some("cortex-clear")) {
                            // Python: the prompt Paseo sends after its cancel waits for
                            // the cancelled turn to end, then runs as steering.
                            jobs.spawn(async move{
                                let deadline=tokio::time::Instant::now()+Duration::from_secs(30);
                                while session.busy.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err() {
                                    if tokio::time::Instant::now()>=deadline {
                                        let _=output.send(respond(id,Err(invalid(task_commands::BUSY)))).await;
                                        return;
                                    }
                                    tokio::time::sleep(Duration::from_millis(20)).await;
                                }
                                let cancelled=session.cancellation.subscribe();
                                let result=prompt(&sid,&id,&session,blocks,&output,&permissions,cancelled,known).await;
                                let held=session.release();
                                let _=output.send(respond(id,result)).await;
                                session.expire_after_grace(&sid,held).await;
                            });
                            return Ok(Value::Null);
                        }
                        match name {
                            Some("cortex-clear")=>return Err(invalid(task_commands::CLEAR_BUSY)),
                            Some("cortex-stop"|"cortex-finish")=>{}
                            // Python: typed text answers the running turn's open question.
                            None|Some("cortex-answer")=>{
                                if steer_busy(&sid,&session,&blocks,&self.permissions).await? {
                                    return Ok(json!({"stopReason":"end_turn"}));
                                }
                                return Err(invalid(task_commands::BUSY));
                            }
                            _=>return Err(invalid(task_commands::BUSY)),
                        }
                        // Python: an explicit terminal command interrupts the running
                        // turn, waits up to 30 s for it to release, then runs.
                        session.cancellation.send_modify(|generation|*generation=generation.wrapping_add(1));
                        self.permissions.cancel(&sid).await;
                        jobs.spawn(async move{
                            let _=session.host.cancel(&sid).await;
                            let deadline=tokio::time::Instant::now()+Duration::from_secs(30);
                            while session.busy.compare_exchange(false,true,Ordering::AcqRel,Ordering::Acquire).is_err() {
                                if tokio::time::Instant::now()>=deadline {
                                    let _=output.send(respond(id,Err(invalid(task_commands::BUSY)))).await;
                                    return;
                                }
                                tokio::time::sleep(Duration::from_millis(50)).await;
                            }
                            let cancelled=session.cancellation.subscribe();
                            let result=prompt(&sid,&id,&session,blocks,&output,&permissions,cancelled,known).await;
                            session.busy.store(false,Ordering::Release);
                            let _=output.send(respond(id,result)).await;
                        });
                        return Ok(Value::Null);
                    }
                    let cancelled=session.cancellation.subscribe();
                    jobs.spawn(async move{
                        let result=prompt(&sid,&id,&session,blocks,&output,&permissions,cancelled,known).await;
                        let held=session.release();
                        let _=output.send(respond(id,result)).await;
                        session.expire_after_grace(&sid,held).await;
                    });
                    return Ok(Value::Null);
                },
                "session/cancel"=>{
                    let (sid,session)=self.session(&p)?;
                    session.arm_hold();
                    session.cancellation.send_modify(|generation|*generation=generation.wrapping_add(1));
                    self.permissions.cancel(&sid).await;
                    // The prompt follower owns its actual execution session.
                    // In particular, a separate-task turn must never pause sid's
                    // unrelated bound task. Idle cancellation has no task effect.
                    return Ok(Value::Null);
                },
                "session/set_config_option"|"session/set_mode"=>{
                    let (sid,session)=self.session(&p)?;
                    let set_mode=method=="session/set_mode";
                    let key=if set_mode{"access_mode"}else{string(&p,"configId")?}.to_owned();
                    let value=string(&p,if method=="session/set_mode"{"modeId"}else{"value"})?.to_owned();
                    let out=self.output.clone();
                    let permissions=self.permissions.clone();
                    let id=id.clone();
                    jobs.spawn(async move {
                      let _serial=session.configuration.lock().await;
                      let mut notifications=Vec::new();
                      let result=async {
                    if !set_mode && session.paseo && key=="access_mode" {
                        // Paseo receives access as `modes`, not as an option.
                        return Err(invalid("unknown_config_option"));
                    }
                    if key=="cortex_question_answer"{
                        let a:Value=serde_json::from_str(&value).map_err(|_|invalid("invalid_question_answer"))?;
                        session.host.answer_payload(&sid,&a).await?;
                        permissions.cancel(&sid).await;
                    }else if key=="cortex_stop_command" {
                        let a:Value=serde_json::from_str(&value).map_err(|_|invalid("invalid_command_stop"))?;
                        session.host.control(&sid,json!({"action":"command_stop","toolCallRef":string(&a,"toolCallRef")?,
                            "reason":string(&a,"reason")?,"requestId":string(&a,"requestKey")?})).await?;
                    }else if key=="cortex_queued_message"{
                        let a:Value=serde_json::from_str(&value).map_err(|_|invalid("invalid_queued_message"))?;
                        let text=prompt_text(&a["content"])?;
                        permissions.cancel(&sid).await;
                        session.host.queued_message(&sid,string(&a,"clientMessageId")?,&text).await?;
                    }else{
                        let options=if key=="access_mode" {session.host.select_access(&sid,&value,if set_mode {"acp_set_mode"} else {"acp_config_option"}).await?} else {session.host.configure(&sid,&key,&value).await?};
                        if set_mode {notifications.push(update(&sid,json!({"sessionUpdate":"current_mode_update","currentModeId":value})));}
                        notifications.push(update(&sid,session.project_update(json!({"sessionUpdate":"config_option_update","configOptions":options}))));
                    }
                    Ok::<_,io::Error>(if set_mode {json!({})} else {json!({"configOptions":session.config_options(&sid).await?})})
                      }.await;
                      let success=result.is_ok();
                      // Python `_session_set_config_option`: a refused selection is
                      // followed by the unchanged options, read before the reply.
                      if let Err(e)=&result {
                          let native=e.to_string();
                          if !set_mode && !key.starts_with("cortex_") && native!="unknown_session" && python_protocol::portable_selection(&native).is_none() {
                              if let Ok(options)=session.config_options(&sid).await {
                                  notifications.push(update(&sid,json!({"sessionUpdate":"config_option_update","configOptions":options})));
                              }
                          }
                      }
                      if let Some(id)=id {let frame=match result{Ok(v)=>reply(id,v),Err(e)=>{
                          let native=e.to_string();
                          match python_protocol::portable_selection(&native) {
                              Some((code,remediation))=>json!({"jsonrpc":"2.0","id":id,"error":{"code":-32602,"message":remediation,
                                  "data":{"code":code,"remediation":remediation}}}),
                              None=>error(id,-32602,&python_protocol::config_message(&native,set_mode,&key,&value)),
                          }
                      }};let _=out.send(frame).await;}
                      if success || !notifications.is_empty() {for notification in notifications {if out.send(notification).await.is_err(){break;}}}
                    });
                    return Ok(Value::Null);
                },
                // Like standalone legacy ingress, an external ACP client has
                // no service-activated Workspace API. Do not invent a new wire
                // shape under the existing extension method names.
                "_cortex/workspace_control"=>Err(invalid("project_inactive")),
                "_cortex/workspace"=>{
                    workspace_protocol::validate(if p.get("payload").is_some(){"Command"}else{"Query"},&p)
                        .map_err(|_|invalid("invalid_workspace_request"))?;
                    Err(invalid("workspace_not_activated"))
                },
                _=>Err(invalid("method_not_found")),
            }
        }.await;
        match response {
            Ok(v) if v.is_null() => Ok(()),
            Ok(v) => {
                if let Some(id) = id {
                    self.send(reply(id, v)).await?;
                }
                Ok(())
            }
            Err(e) => {
                if let Some(id) = id {
                    self.send(python_protocol::request_error(id, method, &e)).await?;
                }
                Ok(())
            }
        }
    }
}
/// Python `_session_prompt` on a busy session: text (or `/cortex-answer <n>
/// <text>`) answers an open question while the turn keeps running. A worker
/// question is answered and its typed text recorded; a coordinator question
/// only through Paseo's question card (`CORTEX_PASEO_QUESTION_UI=1`), as
/// Python's card waiter does. `Ok(false)`: refuse as busy.
async fn steer_busy(
    sid: &str,
    session: &ClientSession,
    blocks: &Value,
    permissions: &Permissions,
) -> io::Result<bool> {
    let text = python_protocol::prompt_text(blocks);
    if text.is_empty() {
        return Ok(false);
    }
    let recognized = commands::recognize(blocks);
    let decisions = session.host.decisions(sid).await?;
    let open: Vec<&Value> = decisions["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|d| d["status"] == "pending")
        .collect();
    let (chosen, answer) = match &recognized {
        Some((_, argument)) => match task_commands::answer(argument) {
            Some((index, answer)) if index <= open.len() => (Some(open[index - 1]), answer),
            _ => return Err(invalid(task_commands::BUSY_ANSWER_USAGE)),
        },
        None if open.len() > 1 => return Err(invalid(task_commands::BUSY_MULTIPLE)),
        // Attachments travel separately; the answer is the typed text itself.
        None => {
            let typed = blocks
                .as_array()
                .into_iter()
                .flatten()
                .filter(|b| b["type"] == "text")
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n");
            (
                open.first().copied(),
                if typed.trim().is_empty() { text } else { typed },
            )
        }
    };
    let Some(chosen) = chosen else {
        return Ok(false);
    };
    let question = chosen["questionId"]
        .as_str()
        .ok_or_else(|| invalid("invalid_question"))?;
    let labels: Vec<&str> = chosen["options"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|o| o["label"].as_str())
        .collect();
    let value = task_commands::typed_choice(&answer, &labels);
    if !chosen["batchId"].is_null() {
        // A batch is answered only as a whole (`cortex_question_answer`);
        // Python's per-member answer has no native counterpart.
        return Err(invalid("question_batch_requires_complete_answer"));
    }
    if chosen["scope"] != "task" {
        // Recorded before delivery, like Python's typed worker answer.
        session
            .host
            .record(
                sid,
                json!({"sessionUpdate":"user_message_chunk","content":{"type":"text","text":answer}}),
            )
            .await?;
    } else if recognized.is_some()
        || std::env::var("CORTEX_PASEO_QUESTION_UI").as_deref() != Ok("1")
        || !permissions.shows_question(sid, question).await
    {
        return Ok(false);
    }
    // Images and resources typed with the answer are retained as the task's next
    // source revision, so the engine prompt that delivers the answer carries them.
    if blocks
        .as_array()
        .is_some_and(|a| a.iter().any(|b| b["type"] != "text"))
    {
        session
            .host
            .retain_answer_attachments(sid, blocks.clone())
            .await?;
    }
    session.host.answer(sid, question, &value).await?;
    permissions.retire_question(sid, question).await;
    Ok(true)
}
fn prompt_text(blocks: &Value) -> io::Result<String> {
    let a = blocks
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or_else(|| invalid("invalid_prompt"))?;
    let mut text = String::new();
    for block in a {
        if block["type"] != "text" {
            return Err(invalid("native_prompt_content_unavailable"));
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(
            block["text"]
                .as_str()
                .ok_or_else(|| invalid("invalid_prompt"))?,
        );
    }
    if text.trim().is_empty() || text.chars().count() > 32_000 {
        return Err(invalid("invalid_prompt"));
    }
    Ok(text)
}
/// What a recognized command does after Python's `_run_command_body` parsed it.
enum Command {
    /// One recorded reply, never an engine turn.
    Reply(String),
    /// A reply Python sends without recording it (`/cortex-new-task` usage).
    Unrecorded(String),
    /// An ordinary prompt turn with this exact text (harvest, skill management).
    Turn(String),
    /// A separate task the session never binds.
    NewTask(String),
    /// Deliver this answer to an open question, then follow the turn.
    Answer(String, String),
}
fn reply_text(text: impl Into<String>) -> io::Result<Command> {
    Ok(Command::Reply(text.into()))
}
/// Python `_run_command_body`. Store and lifecycle effects stay behind the
/// native host; text, usage errors and refusals are Python's.
async fn command(
    sid: &str,
    session: &ClientSession,
    (command, argument): (&str, String),
    out: &mpsc::Sender<Value>,
    known: &[String],
) -> io::Result<Command> {
    let host = &session.host;
    let options = || async { Ok::<_, io::Error>(session.config_options(sid).await?) };
    let task = || async {
        Ok::<_, io::Error>(host.session(sid).await?["task"].as_str().map(str::to_owned))
    };
    let reply = match command {
        "cortex-kill" => {
            let (reference, reason) = match task_commands::kill(&argument) {
                Ok(stop) => stop,
                Err(text) => return reply_text(text),
            };
            if task().await?.is_none() {
                return reply_text(task_commands::refused("command_stop_no_task"));
            }
            let key = format!("slash:{}", wp_identity()?);
            let request = json!({"action":"command_stop","toolCallRef":reference,"reason":reason,"requestId":key});
            return reply_text(match host.control(sid, request).await {
                Ok(_) => task_commands::KILL_REQUESTED.into(),
                Err(e) => task_commands::refused(&e.to_string()),
            });
        }
        "cortex-clear" => {
            let Some(clear) = task_commands::parse_clear(&argument, task_commands::now_micros())
            else {
                return reply_text(task_commands::CLEAR_USAGE);
            };
            let Ok(found) = host
                .clear_preview(sid, clear.cutoff.clone(), clear.force, known.to_vec())
                .await
            else {
                return reply_text(task_commands::CLEAR_REFUSED);
            };
            let count = |k: &str| found[k].as_u64().unwrap_or_default();
            let (tasks, sessions, skipped) = (count("tasks"), count("sessions"), count("skipped"));
            return reply_text(if !clear.confirm {
                task_commands::dry_run(&clear, tasks, sessions, skipped)
            } else if tasks == 0 && sessions == 0 {
                task_commands::confirmed_nothing(&clear, skipped)
            } else {
                task_commands::CLEAR_DELETE_UNAVAILABLE.into()
            });
        }
        "cortex-stop" | "cortex-finish" => {
            let finish = command == "cortex-finish";
            if finish && !argument.is_empty() {
                return reply_text(task_commands::FINISH_USAGE);
            }
            if task().await?.is_none() {
                return reply_text(if finish {
                    task_commands::FINISH_NONE
                } else {
                    task_commands::STOP_NONE
                });
            }
            let request = if finish {
                json!({"action":"close_task","confirmed":true})
            } else {
                json!({"action":"stop"})
            };
            match host.control(sid, request).await {
                Ok(_) => {}
                // A turn this command interrupted already paused the task.
                Err(e) if !finish && e.to_string() == "already_paused" => {}
                Err(_) => {
                    return Err(invalid(if finish {
                        "Failed to close the task."
                    } else {
                        "Failed to stop the task."
                    }))
                }
            }
            return reply_text(if finish {
                task_commands::FINISH_CONFIRMED
            } else {
                task_commands::STOP_CONFIRMED
            });
        }
        "cortex-answer" => {
            let parsed = task_commands::answer(&argument);
            let (Some((index, text)), Some(_)) = (parsed, task().await?) else {
                return reply_text(task_commands::ANSWER_USAGE);
            };
            let decisions = host.decisions(sid).await?;
            let open: Vec<_> = decisions["items"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|d| d["status"] == "pending")
                .filter_map(|d| d["questionId"].as_str())
                .collect();
            if open.is_empty() {
                return reply_text(task_commands::ANSWER_NONE_OPEN);
            }
            return match open.get(index - 1) {
                Some(question) => Ok(Command::Answer((*question).into(), text)),
                None => reply_text(task_commands::ANSWER_UNKNOWN_INDEX),
            };
        }
        "cortex-new-task" => {
            return Ok(if argument.is_empty() {
                Command::Unrecorded(task_commands::NEW_TASK_USAGE.into())
            } else {
                Command::NewTask(argument)
            })
        }
        "skill-management" => {
            return match task_commands::skill_management(&argument) {
                Some(text) => Ok(Command::Turn(text)),
                None => reply_text(task_commands::SKILL_USAGE),
            }
        }
        "harvest" | "harvest-refresh" => {
            let settings = host.settings(sid).await?;
            let language = settings["language"].as_str().unwrap_or("en");
            return Ok(Command::Turn(task_commands::harvest(
                command == "harvest-refresh",
                &argument,
                language,
                task().await?.is_some(),
            )));
        }
        "cortex-status" => {
            let states = host.connection_states().await?;
            return reply_text(commands::status(
                &options().await?,
                &commands::connection_summary(&states),
            ));
        }
        "cortex-setup" => return reply_text(commands::setup(&host.connection_states().await?)),
        "cortex-language" => match commands::language(&argument) {
            Some(value) => ("language", value),
            None => return reply_text("Usage: /cortex-language auto|en|ru"),
        },
        "cortex-model" => {
            let entries = host.listing().await?;
            if entries.is_empty() {
                return reply_text("No model is selectable yet. Use /cortex-setup to configure an authorized engine connection.");
            }
            match commands::model(&argument, &entries) {
                Some(route) => ("route", route),
                None => {
                    return reply_text(format!(
                        "No matching model for {}. Use /cortex-status to see the current options.",
                        startup::repr(&json!(argument))
                    ))
                }
            }
        }
        "cortex-effort" => {
            if argument.is_empty() {
                return reply_text("/cortex-effort requires a value. Use /cortex-status to see the current options.");
            }
            if !options()
                .await?
                .as_array()
                .is_some_and(|a| a.iter().any(|o| o["id"] == "thought_level"))
            {
                return reply_text("No thought level is selectable for the current model. Use /cortex-status to inspect options, or /cortex-setup if no model is available.");
            }
            ("thought_level", argument)
        }
        _ => return Err(invalid("native_slash_command_unavailable")),
    };
    let (id, value) = reply;
    let _serial = session.configuration.lock().await;
    let result = host.configure(sid, id, &value).await;
    if result
        .as_ref()
        .is_err_and(|e| python_protocol::portable_selection(&e.to_string()).is_some())
    {
        // Python lets this resolution error escape the command body.
        return Err(invalid("Internal error while processing this prompt."));
    }
    let shown = match &result {
        Ok(options) => client_options(options.clone(), session.paseo),
        Err(_) => options().await?,
    };
    out.send(update(
        sid,
        json!({"sessionUpdate":"config_option_update","configOptions":shown}),
    ))
    .await
    .map_err(|_| invalid("client_closed"))?;
    reply_text(match result {
        Ok(_) => format!(
            "{command} set to {} (pending — applies to the next run).",
            startup::repr(&json!(value))
        ),
        Err(e) => {
            let key = if id == "route" { "model" } else { id };
            format!(
                "Could not apply {command}: {}",
                python_protocol::config_message(&e.to_string(), false, key, &value)
            )
        }
    })
}
/// The same random request identity form the native host uses.
fn wp_identity() -> io::Result<String> {
    use std::io::Read;
    let mut bytes = [0; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
#[allow(clippy::too_many_arguments)]
async fn prompt(
    sid: &str,
    request: &Value,
    session: &ClientSession,
    blocks: Value,
    out: &mpsc::Sender<Value>,
    permissions: &Permissions,
    cancelled: watch::Receiver<u64>,
    known: Vec<String>,
) -> io::Result<Value> {
    let Some(recognized) = commands::recognize(&blocks) else {
        return turn(sid, session, blocks, out, permissions, cancelled).await;
    };
    // Python records the command prompt before running it, except a command
    // stop, which is control input handled outside the prompt lane.
    if recognized.0 != "cortex-kill" {
        let typed = blocks[0]["text"].as_str().unwrap_or_default();
        session
            .host
            .record(
                sid,
                json!({"sessionUpdate":"user_message_chunk","content":{"type":"text","text":typed}}),
            )
            .await?;
    }
    let identity = request
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| request.to_string());
    let message = format!("assistant:command:{sid}:{identity}");
    let chunk = |body: String| json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":body},"messageId":message});
    match command(sid, session, recognized, out, &known).await? {
        Command::Reply(body) => {
            if cancelled.has_changed().unwrap_or(true) {
                return Ok(json!({"stopReason":"cancelled"}));
            }
            let chunk = chunk(body);
            session.host.record(sid, chunk.clone()).await?;
            out.send(update(sid, chunk))
                .await
                .map_err(|_| invalid("client_closed"))?;
            Ok(json!({"stopReason":"end_turn"}))
        }
        Command::Unrecorded(body) => {
            out.send(update(sid, chunk(body)))
                .await
                .map_err(|_| invalid("client_closed"))?;
            Ok(json!({"stopReason":"end_turn"}))
        }
        Command::Turn(text) => {
            let blocks = json!([{"type":"text","text":text}]);
            turn(sid, session, blocks, out, permissions, cancelled).await
        }
        Command::NewTask(text) => new_task(sid, &message, session, &text, out, cancelled).await,
        Command::Answer(question, text) => {
            let after = cursor(sid, session).await?;
            session.steer_hold();
            session.host.answer(sid, &question, &text).await?;
            follow(sid, session, out, permissions, cancelled, after).await
        }
    }
}
/// The last durable public sequence already shown for this session.
async fn cursor(sid: &str, session: &ClientSession) -> io::Result<i64> {
    let mut after = -1;
    loop {
        let rows = session.host.history(sid, after).await?;
        let Some(last) = rows.last() else {
            return Ok(after);
        };
        after = last["seq"].as_i64().unwrap();
    }
}
/// Python `_run_prompt_body`: admit one ordinary prompt, then follow it.
async fn turn(
    sid: &str,
    session: &ClientSession,
    blocks: Value,
    out: &mpsc::Sender<Value>,
    permissions: &Permissions,
    cancelled: watch::Receiver<u64>,
) -> io::Result<Value> {
    let after = cursor(sid, session).await?;
    if cancelled.has_changed().unwrap_or(true) {
        return Ok(json!({"stopReason":"cancelled"}));
    }
    // Python `_resolve_cancel_hold`: this prompt steers a held cancel's task.
    session.steer_hold();
    // A task a vanished peer process left unfinished is recovered before new work.
    session.host.recover().await?;
    if let Err(e) = session.host.start_prompt(sid, blocks.clone()).await {
        let native = e.to_string();
        if !python_protocol::route_unavailable(&native) {
            return Err(e);
        }
        // Python records the prompt in the session transcript before its
        // orchestration refuses it; the native host admitted nothing.
        session
            .host
            .record(
                sid,
                json!({"sessionUpdate":"user_message_chunk","content":{"type":"text",
                    "text":python_protocol::prompt_text(&blocks)}}),
            )
            .await?;
        return Err(invalid(&format!("orchestration_unavailable:{native}")));
    }
    // A cancel during admission is observed by `follow` before anything else.
    follow(sid, session, out, permissions, cancelled, after).await
}
/// Python `_run_cortex_new_task`: start a separate task, stream only its
/// coordinator text under the command's message id, then name the task.
async fn new_task(
    sid: &str,
    message: &str,
    session: &ClientSession,
    text: &str,
    out: &mpsc::Sender<Value>,
    cancelled: watch::Receiver<u64>,
) -> io::Result<Value> {
    if cancelled.has_changed().unwrap_or(true) {
        return Ok(json!({"stopReason":"cancelled"}));
    }
    let host = &session.host;
    let hidden = host.new_task(sid, text).await.map_err(|e| {
        if python_protocol::route_unavailable(&e.to_string()) {
            invalid("new_task_unavailable")
        } else {
            e
        }
    })?;
    let mut after = -1;
    let mut paused = false;
    let state = loop {
        if !paused && cancelled.has_changed().unwrap_or(true) {
            paused = true;
            host.cancel(&hidden).await?;
        }
        let rows = host.history(&hidden, after).await?;
        for row in &rows {
            after = row["seq"].as_i64().unwrap();
            let update = &row["update"];
            if let ("agent_message_chunk", Some(text)) = (
                update["sessionUpdate"].as_str().unwrap_or_default(),
                update["content"]["text"].as_str(),
            ) {
                if !paused {
                    out.send(update_chunk(sid, text, message))
                        .await
                        .map_err(|_| invalid("client_closed"))?;
                }
            }
        }
        let state = host.session(&hidden).await?;
        if matches!(
            state["state"].as_str(),
            Some("COMPLETED" | "CANCELLED" | "PAUSED" | "FAILED")
        ) {
            host.settled(&hidden).await?;
            if rows.is_empty() {
                break state;
            }
            continue;
        }
        // Nothing answers a question card of a task no session shows: the
        // coordinator's turn has ended once it waits on the owner.
        let waiting = host.decisions(&hidden).await?["items"]
            .as_array()
            .is_some_and(|a| a.iter().any(|d| d["status"] == "pending"));
        if waiting && rows.is_empty() {
            break state;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let task = state["task"].as_str().unwrap_or_default();
    out.send(update_chunk(
        sid,
        &task_commands::new_task_created(task),
        message,
    ))
    .await
    .map_err(|_| invalid("client_closed"))?;
    Ok(
        json!({"stopReason":if paused || matches!(state["state"].as_str(),Some("PAUSED"|"CANCELLED")){"cancelled"}else{"end_turn"}}),
    )
}
fn update_chunk(sid: &str, text: &str, message: &str) -> Value {
    update(
        sid,
        json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text},"messageId":message}),
    )
}
/// Follow the durable public stream of an admitted turn to its native end.
async fn follow(
    sid: &str,
    session: &ClientSession,
    out: &mpsc::Sender<Value>,
    permissions: &Permissions,
    cancelled: watch::Receiver<u64>,
    mut after: i64,
) -> io::Result<Value> {
    let mut questions = HashSet::new();
    let mut paused = false;
    loop {
        // Cancellation belongs to this admitted prompt, not the transport's
        // invoking session (which can be following a separate task instead).
        if !paused && cancelled.has_changed().unwrap_or(true) {
            paused = true;
            if session.hold_turn() {
                // Python `_CancelSteeringHold`: a Paseo cancel ends this turn at
                // once; its task and workers continue until the grace expires.
                return Ok(json!({"stopReason":"cancelled"}));
            }
            // `/cortex-stop` can pause the task between the host's state check and
            // its pause; that already ends this turn as cancelled.
            match session.host.cancel(sid).await {
                Err(e) if e.to_string() == "already_paused" => {}
                other => other?,
            }
        }
        // Drain the durable public stream before inspecting terminal state.
        let rows = session.host.history(sid, after).await?;
        for row in &rows {
            after = row["seq"].as_i64().unwrap();
            if row["update"]["sessionUpdate"] != "user_message_chunk" {
                out.send(update(sid, session.project_update(row["update"].clone())))
                    .await
                    .map_err(|_| invalid("client_closed"))?;
            }
        }
        let state = session.host.session(sid).await?;
        if matches!(
            state["state"].as_str(),
            Some("COMPLETED" | "CANCELLED" | "PAUSED" | "FAILED")
        ) {
            // A completed task is evidence from Core, never inferred from ACP end_turn.
            // Likewise cancellation waits for observed termination, not only Pause.
            session.host.settled(sid).await?;
            if !rows.is_empty() {
                continue;
            }
            return Ok(
                json!({"stopReason":if matches!(state["state"].as_str(),Some("PAUSED"|"CANCELLED")){"cancelled"}else{"end_turn"}}),
            );
        }
        let ru = state["language"] == "ru";
        let decisions = session.host.decisions(sid).await?;
        if let Some(items) = decisions["items"].as_array() {
            for d in items {
                if d["status"] == "pending" && !d["batchId"].is_null() {
                    let batch = d["batchId"]
                        .as_str()
                        .ok_or_else(|| invalid("invalid_question_batch"))?;
                    if !questions.insert(batch.into()) {
                        continue;
                    }
                    // Python `elicit_batch` members carry the optional `topic` tab label.
                    let members:Vec<_>=items.iter().filter(|m|m["batchId"]==batch).map(|m| {
                        let mut v=json!({"questionId":m["questionId"],"question":m["prompt"],
                            "options":m["options"].as_array().unwrap().iter().map(|o|o["label"].clone()).collect::<Vec<_>>()});
                        if m["topic"].is_string() {v["topic"]=m["topic"].clone();}
                        v}).collect();
                    // Python `elicit_batch` option ids, which the Paseo plugin's
                    // question form validates: `cortex-batch:<question>:<n>`.
                    let options: Vec<_> = items
                        .iter()
                        .filter(|m| m["batchId"] == batch)
                        .flat_map(|m| {
                            m["options"].as_array().unwrap().iter().enumerate().map(move |(i,o)|
                            json!({"optionId":format!("cortex-batch:{}:{}",m["questionId"].as_str().unwrap_or_default(),i+1),"name":o["label"],"kind":"allow_once"}))
                        })
                        .collect();
                    let body = json!({"toolCall":{"toolCallId":format!("cortex-question-batch:{batch}"),"kind":"other",
                        "title":if ru { "Ответьте на связанные вопросы" } else { "Answer related questions" },"rawInput":{"__cortex_question":true,"batchId":batch,
                        "questionId":d["questionId"],"questions":members}},"options":options,"_meta":{"cortexQuestion":true}});
                    // ACP returns a single option only. A batch needs the complete
                    // typed config submission, not that one-option response.
                    let _ = permissions.question(sid, body, &cancelled).await;
                    continue;
                }
                if d["status"] == "pending" && questions.insert(d["questionId"].to_string()) {
                    let labels: Vec<&str> = d["options"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .filter_map(|o| o["label"].as_str())
                        .collect();
                    let body = question_card(d, &labels, ru);
                    let answer = permissions.question(sid, body, &cancelled).await;
                    if let Some(label) = answer.ok().and_then(|a| card_choice(&a, &labels)) {
                        session
                            .host
                            .answer(sid, d["questionId"].as_str().unwrap(), label)
                            .await?;
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
/// Python `_PaseoQuestionElicitor.elicit`: the Paseo plugin recognises a
/// question card only by this marker, tool call id and option-id shape. Paseo
/// needs at least one action, so an option-less question offers Python's
/// instructional action, which is never an answer.
fn question_card(d: &Value, labels: &[&str], ru: bool) -> Value {
    let shown: Vec<&str> = if labels.is_empty() {
        vec![if ru {
            "Ответьте в чате или введите ответ в поле «Другое»…"
        } else {
            "Reply in chat or type an answer under Other…"
        }]
    } else {
        labels.to_vec()
    };
    let options: Vec<_> = shown
        .iter()
        .enumerate()
        .map(|(i, label)| json!({"optionId":format!("cortex-question-{}",i+1),"name":label,"kind":"allow_once"}))
        .collect();
    let question = d["prompt"].as_str().unwrap_or_default();
    let question: String = question.chars().take(4000).collect();
    json!({"toolCall":{"toolCallId":format!("cortex-question:{}",d["questionId"].as_str().unwrap_or_default()),
        "kind":"other","title":question,
        "rawInput":{"__cortex_question":true,"questionId":d["questionId"],"question":question}},
        "options":options,"_meta":{"cortexQuestion":true}})
}
/// The offered label a selected card option names, if any.
fn card_choice<'a>(answer: &Value, labels: &[&'a str]) -> Option<&'a str> {
    if answer["outcome"]["outcome"] != "selected" {
        return None;
    }
    let index: usize = answer["outcome"]["optionId"]
        .as_str()?
        .strip_prefix("cortex-question-")?
        .parse()
        .ok()?;
    labels.get(index.checked_sub(1)?).copied()
}
fn reply(id: Value, result: Value) -> Value {
    json!({"jsonrpc":"2.0","id":id,"result":result})
}
fn respond(id: Value, result: io::Result<Value>) -> Value {
    match result {
        Ok(v) => reply(id, v),
        Err(e) => python_protocol::prompt_error(id, &e),
    }
}
fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})
}
fn update(sid: &str, update: Value) -> Value {
    json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":sid,"update":update}})
}

/// Run one client connection; EOF cancels/reaps owned runtime jobs before releasing
/// any project owner. The bounded writer lane never interleaves JSON frames.
pub async fn serve<R, W>(input: R, output: W, routes: Arc<dyn NativeRoutes>) -> io::Result<()>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    serve_until(input, output, routes, std::future::pending()).await
}
/// Host shutdown follows the same settlement path as client EOF.
pub async fn serve_until<R, W, F>(
    mut input: R,
    mut output: W,
    routes: Arc<dyn NativeRoutes>,
    shutdown: F,
) -> io::Result<()>
where
    R: AsyncBufRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
    F: std::future::Future<Output = ()>,
{
    tokio::pin!(shutdown);
    // read_frame owns partial bytes and is not cancellation-safe. A dedicated
    // bounded lane reads each frame once, even when another request completes.
    let (input_tx, mut incoming) = mpsc::channel::<frames::Inbound>(4);
    let reader = tokio::spawn(async move {
        while let Some(frame) = frames::read(&mut input).await {
            if input_tx.send(frame).await.is_err() {
                break;
            }
        }
    });
    let (tx, mut rx) = mpsc::channel::<Value>(128);
    let mut writer = tokio::spawn(async move {
        while let Some(v) = rx.recv().await {
            wire::write_frame(&mut output, &v)
                .await
                .map_err(|_| invalid("client_write_failed"))?;
        }
        Ok::<_, io::Error>(())
    });
    let permissions = Arc::new(Permissions {
        output: tx.clone(),
        next: AtomicU64::new(1),
        pending: Mutex::new(HashMap::new()),
    });
    let mut agent = Agent {
        routes,
        hosts: HashMap::new(),
        sessions: HashMap::new(),
        legacy_sessions: HashSet::new(),
        info: json!({}),
        capabilities: json!({}),
        initialized: false,
        output: tx,
        permissions,
        seen: HashSet::new(),
        seen_order: VecDeque::new(),
        refreshes: JoinSet::new(),
    };
    let mut jobs = JoinSet::new();
    let mut writer_result = None;
    let result = loop {
        tokio::select! {
            frame=incoming.recv()=>{
                match frame {
                    Some(Ok(frame))=>{
                        if jobs.len()>=512{break Err(invalid("request_capacity"));}
                        if let Err(e)=agent.dispatch(Value::Object(frame),&mut jobs).await{break Err(e);}
                    },
                    None=>break Ok(()),
                    // Python replies to a malformed line and keeps reading.
                    Some(Err(e))=>if let Err(e)=agent.send(e.reply()).await{break Err(e);},
                }
            },
            _=&mut shutdown=>break Ok(()),
            Some(_)=jobs.join_next()=>{},
            result=&mut writer=>{
                writer_result=Some(result);
                break Err(invalid("client_write_failed"));
            },
        }
    };
    reader.abort();
    let _ = reader.await;
    agent.permissions.pending.lock().await.clear();
    for host in agent.hosts.values() {
        host.shutdown().await;
    }
    jobs.abort_all();
    while jobs.join_next().await.is_some() {}
    // A refresh still observing ends with this process (Python's detached
    // refresh process outlives it); it must not hold the writer or a session open.
    agent.refreshes.abort_all();
    while agent.refreshes.join_next().await.is_some() {}
    agent.sessions.clear();
    let hosts = std::mem::take(&mut agent.hosts);
    drop(agent);
    // StoreExecutor joins its threads only off the async reactor.
    tokio::task::spawn_blocking(move || drop(hosts))
        .await
        .map_err(|_| invalid("host_shutdown_failed"))?;
    match writer_result {
        Some(result) => result,
        None => writer.await,
    }
    .map_err(|_| invalid("writer_failed"))??;
    result
}
