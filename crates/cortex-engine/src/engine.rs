//! ACP v1 worker connection, correlation and supervised process lifetime.
//! Python oracle: src/cortex_runtime/engine/client.py:159-237,
//! process.py:917-1299, session_manager.py:802-902,1082-1284,
//! permissions.py:370-501. Core owns permission decisions and durable identity.
use crate::{environment::LaunchSpec, wire, Error, FutureResult, Result};
use cortex_core::ports::ProcessObservation;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, BufReader},
    process::Command,
    sync::{mpsc, oneshot, watch, Mutex as AsyncMutex},
    task::JoinHandle,
};

/// Policy belongs to Core, not engine-native options or model JSON.
pub trait PermissionPolicy: Send + Sync + 'static {
    fn authority(&self) -> FutureResult<'_, ()>;
    fn decide<'a>(
        &'a self,
        session: &'a str,
        request_id: &'a Value,
        turn_generation: u64,
        request: &'a Value,
    ) -> FutureResult<'a, PermissionDecision>;
}
pub enum PermissionDecision {
    Cancelled,
    Selected(String),
}
#[derive(Debug)]
pub enum Event {
    Update(Value),
    UnknownNotification,
    LateOrDuplicateReply,
    TransportClosed,
}
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;
struct Inner {
    writer: AsyncMutex<tokio::process::ChildStdin>,
    pending: Pending,
    next: AtomicU64,
    closed: AtomicBool,
    shutdown: watch::Sender<bool>,
    sessions: Mutex<HashMap<String, Arc<AtomicU8>>>,
    creation_unknown: AtomicBool,
    initialized: AtomicU8,
    prompt_caps: Mutex<Value>,
    generations: Mutex<HashMap<String, Arc<AtomicU64>>>,
    /// Request id of the outstanding `session/new`. Its reply registers the new
    /// session in the reader itself, before the reply is delivered: an engine may
    /// send that session's first update right behind the reply (claude-agent-acp
    /// sends `available_commands_update` on a 0 ms timer), and the reader treats
    /// an update for an unregistered session as a protocol violation.
    session_new: Mutex<Option<u64>>,
    /// The session id the reader registered for that reply, taken by `start_session`.
    registered: Mutex<Option<String>>,
    diagnostics: Diagnostics,
}
/// Bounded tail of the engine's stderr kept for start-failure diagnostics.
pub const STDERR_TAIL: usize = 8192;
/// Secret-free facts about an engine process, retained so a failure that closes
/// the transport can be diagnosed afterwards. The stderr tail is bounded and
/// redacted when read ([`redact`]); nothing here is ever sent to a model.
#[derive(Clone, Default)]
pub struct Diagnostics(Arc<Mutex<DiagnosticState>>);
#[derive(Default)]
struct DiagnosticState {
    tail: std::collections::VecDeque<u8>,
    bytes: u64,
    exit: Option<String>,
    exited_unprompted: bool,
    transport_close: Option<&'static str>,
}
/// One read of [`Diagnostics`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DiagnosticSnapshot {
    /// The last [`STDERR_TAIL`] bytes of stderr, lossily decoded and redacted.
    pub stderr_tail: String,
    /// Every stderr byte the process wrote.
    pub stderr_bytes: u64,
    /// `exit:<code>` or `signal:<number>` once the process was reaped.
    pub exit: Option<String>,
    /// The process exited before the host asked it to stop.
    pub exited_unprompted: bool,
    /// Why the host's ACP transport closed, if it did: `eof`, a protocol
    /// violation the reader refused, a failed write, or a refused authority.
    pub transport_close: Option<&'static str>,
}
impl Diagnostics {
    pub fn snapshot(&self) -> DiagnosticSnapshot {
        let s = self.0.lock().unwrap();
        let (a, b) = s.tail.as_slices();
        DiagnosticSnapshot {
            stderr_tail: redact(&String::from_utf8_lossy(&[a, b].concat())),
            stderr_bytes: s.bytes,
            exit: s.exit.clone(),
            exited_unprompted: s.exited_unprompted,
            transport_close: s.transport_close,
        }
    }
    fn closed(&self, reason: &'static str) {
        let mut s = self.0.lock().unwrap();
        if s.transport_close.is_none() {
            s.transport_close = Some(reason);
        }
    }
    fn stderr(&self, chunk: &[u8]) {
        let mut s = self.0.lock().unwrap();
        s.bytes += chunk.len() as u64;
        s.tail.extend(chunk);
        let excess = s.tail.len().saturating_sub(STDERR_TAIL);
        s.tail.drain(..excess);
    }
}
/// Redacts what could be a credential from engine diagnostics: values after a
/// credential-like key (`token=`, `Authorization: Bearer ...`, `"apiKey":"..."`),
/// known secret prefixes (`sk-`, `ghp_`, JWTs, ...) and any long mixed
/// letter/digit run that is not a UUID. Control characters other than newline
/// and tab become `?`. Paths and ordinary words stay readable.
pub fn redact(text: &str) -> String {
    const KEYS: &[&str] = &[
        "token",
        "secret",
        "password",
        "passwd",
        "apikey",
        "api_key",
        "api-key",
        "authorization",
        "auth",
        "cookie",
        "credential",
        "credentials",
        "bearer",
        "key",
    ];
    fn word(c: char) -> bool {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '+' | '=' | '.' | '~')
    }
    fn uuid(t: &str) -> bool {
        let parts: Vec<_> = t.split('-').collect();
        parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12])
            && parts
                .iter()
                .all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
    }
    fn secret_like(t: &str) -> bool {
        let t = t.trim_matches(|c| matches!(c, '.' | '=' | '-' | '_'));
        let lower = t.to_ascii_lowercase();
        let prefixed = [
            "sk-",
            "sk_",
            "ghp_",
            "gho_",
            "ghs_",
            "github_pat_",
            "xox",
            "akia",
            "eyj",
        ]
        .iter()
        .any(|p| lower.starts_with(p) && t.len() >= p.len() + 8);
        let long = t.len() >= 32
            && t.chars().any(|c| c.is_ascii_digit())
            && t.chars().any(|c| c.is_ascii_alphabetic())
            && !uuid(t);
        prefixed || long
    }
    let clean: String = text
        .chars()
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                '?'
            } else {
                c
            }
        })
        .collect();
    let chars: Vec<char> = clean.chars().collect();
    let mut out = String::with_capacity(clean.len());
    let mut i = 0;
    // After a credential-like key, the next value (up to a delimiter) is redacted.
    let mut redact_next = false;
    while i < chars.len() {
        if !word(chars[i]) {
            out.push(chars[i]);
            if chars[i] == '\n' {
                redact_next = false;
            }
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && word(chars[i]) {
            i += 1;
        }
        let token: String = chars[start..i].iter().collect();
        // `key=value` in one token: the key stays; the value is redacted after a
        // credential-like key or when it looks like a secret itself.
        if let Some((key, value)) = token.split_once('=') {
            let keyed = KEYS.iter().any(|k| key.to_ascii_lowercase().ends_with(k));
            out.push_str(if redact_next || secret_like(key) {
                "[redacted]"
            } else {
                key
            });
            out.push('=');
            if value.is_empty() {
                redact_next = keyed;
            } else {
                out.push_str(if keyed || secret_like(value) {
                    "[redacted]"
                } else {
                    value
                });
                redact_next = false;
            }
            continue;
        }
        let lower = token.to_ascii_lowercase();
        let is_key = KEYS.iter().any(|k| lower.ends_with(k)) && token.len() <= 40;
        if redact_next && !is_key || secret_like(&token) {
            out.push_str("[redacted]");
            redact_next = false;
        } else {
            out.push_str(&token);
            redact_next = is_key
                && chars[i..]
                    .iter()
                    .take_while(|c| !word(**c) && **c != '\n')
                    .any(|c| matches!(c, ':' | '='));
            // `Authorization: Bearer <value>`: the scheme word passes the key on.
            if lower == "bearer" || lower == "basic" {
                redact_next = true;
            }
        }
    }
    out
}
/// Prompt content kinds the engine advertised at initialize.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PromptCapabilities {
    pub image: bool,
    pub embedded_context: bool,
}
/// Ready=0, active=1, uncertain=2. Dropping a prompt never unlocks the session.
pub struct Session {
    id: String,
    state: Arc<AtomicU8>,
    config: Mutex<Value>,
    generation: Arc<AtomicU64>,
}
impl Session {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn config_options(&self) -> Value {
        self.config.lock().unwrap().clone()
    }
    /// Observation only: cancellation must still await the exact prompt reply.
    pub fn prompt_active(&self) -> bool {
        self.state.load(Ordering::SeqCst) == 1
    }
}
pub struct Engine {
    inner: Arc<Inner>,
    events: AsyncMutex<mpsc::Receiver<Event>>,
    process: JoinHandle<Result<i32>>,
    reader: JoinHandle<()>,
    monitor: JoinHandle<()>,
    stderr: JoinHandle<u64>,
    exit: watch::Receiver<Option<i32>>,
    pid: u32,
}
fn close(inner: &Inner) {
    inner.closed.store(true, Ordering::SeqCst);
    for (_, tx) in inner.pending.lock().unwrap().drain() {
        let _ = tx.send(Err(Error::Closed));
    }
}
struct SendGuard<'a> {
    inner: &'a Inner,
    armed: bool,
}
impl Drop for SendGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            close(self.inner);
            let _ = self.inner.shutdown.send(true);
        }
    }
}
struct PendingGuard {
    pending: Pending,
    id: u64,
}
impl Drop for PendingGuard {
    fn drop(&mut self) {
        self.pending.lock().unwrap().remove(&self.id);
    }
}
struct TurnGuard(Arc<AtomicU8>);
impl Drop for TurnGuard {
    fn drop(&mut self) {
        if matches!(self.0.load(Ordering::SeqCst), 1 | 3) {
            self.0.store(2, Ordering::SeqCst);
        }
    }
}
impl Inner {
    async fn send(&self, v: Value) -> Result<()> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        let mut guard = SendGuard {
            inner: self,
            armed: true,
        };
        let r = tokio::time::timeout(Duration::from_secs(5), async {
            wire::write_frame(&mut *self.writer.lock().await, &v).await
        })
        .await
        .map_err(|_| {
            self.diagnostics.closed("write_timeout");
            Error::Unknown
        })?;
        guard.armed = false;
        if r.is_err() {
            self.diagnostics.closed("write_failed");
            close(self);
            let _ = self.shutdown.send(true);
        }
        r
    }
    /// Registers the session a `session/new` reply names, in the reader, before
    /// the reply reaches its caller. A duplicate id is left unregistered, which
    /// `start_session` refuses as a protocol error.
    fn register_created(&self, id: u64, reply: &Value) {
        {
            let mut outstanding = self.session_new.lock().unwrap();
            if *outstanding != Some(id) {
                return;
            }
            *outstanding = None;
        }
        let Some(sid) = reply["sessionId"].as_str().filter(|s| !s.is_empty()) else {
            return;
        };
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.contains_key(sid) {
            return;
        }
        sessions.insert(sid.to_owned(), Arc::new(AtomicU8::new(0)));
        self.generations
            .lock()
            .unwrap()
            .insert(sid.to_owned(), Arc::new(AtomicU64::new(0)));
        *self.registered.lock().unwrap() = Some(sid.to_owned());
    }
    async fn send_permission(
        &self,
        id: Value,
        mut outcome: Value,
        state: Arc<AtomicU8>,
        generation: Arc<AtomicU64>,
        expected: u64,
    ) -> Result<()> {
        let mut guard = SendGuard {
            inner: self,
            armed: true,
        };
        let r = tokio::time::timeout(Duration::from_secs(5), async {
            let mut writer = self.writer.lock().await;
            if self.closed.load(Ordering::SeqCst) {
                return Err(Error::Closed);
            }
            if state.load(Ordering::SeqCst) != 1 || generation.load(Ordering::SeqCst) != expected {
                outcome = json!({"outcome":"cancelled"});
            }
            wire::write_frame(
                &mut *writer,
                &json!({"jsonrpc":"2.0","id":id,"result":{"outcome":outcome}}),
            )
            .await
        })
        .await
        .map_err(|_| Error::Unknown)?;
        if r.is_ok() {
            guard.armed = false
        }
        r
    }
    async fn call(&self, method: &str, params: Value, deadline: Option<Duration>) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        if id > 9_007_199_254_740_991 {
            return Err(Error::Busy);
        }
        let (tx, rx) = oneshot::channel();
        {
            let mut p = self.pending.lock().unwrap();
            if p.len() >= 1024 {
                return Err(Error::Busy);
            }
            p.insert(id, tx);
        }
        let _guard = PendingGuard {
            pending: self.pending.clone(),
            id,
        };
        if method == "session/new" {
            *self.registered.lock().unwrap() = None;
            *self.session_new.lock().unwrap() = Some(id);
        }
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        match deadline {
            Some(d) => tokio::time::timeout(d, rx)
                .await
                .map_err(|_| Error::Unknown)?
                .map_err(|_| Error::Closed)?,
            None => rx.await.map_err(|_| Error::Closed)?,
        }
    }
}
impl Engine {
    pub async fn launch(spec: LaunchSpec, policy: Arc<dyn PermissionPolicy>) -> Result<Self> {
        Self::launch_with_model_instructions(spec, policy, None).await
    }
    pub async fn launch_with_model_instructions(
        spec: LaunchSpec,
        policy: Arc<dyn PermissionPolicy>,
        instructions: Option<&std::path::Path>,
    ) -> Result<Self> {
        policy.authority().await?;
        let mut env = spec.launch_environment()?;
        if let Some(path) = instructions {
            if spec.engine != crate::environment::EngineKind::Codex || !path.is_absolute() {
                return Err(Error::Invalid);
            }
            let mut config: Value =
                serde_json::from_str(&crate::environment::codex_thread_config())
                    .map_err(|_| Error::Invalid)?;
            config["model_instructions_file"] = json!(path);
            env.insert(crate::environment::CODEX_CONFIG.into(), config.to_string());
        }
        let mut child = Command::new(&spec.executable)
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()?;
        let pid = child.id().ok_or(Error::Io)?;
        let stdin = child.stdin.take().ok_or(Error::Io)?;
        let stdout = child.stdout.take().ok_or(Error::Io)?;
        let mut stderr = child.stderr.take().ok_or(Error::Io)?;
        let (shutdown, mut stop) = watch::channel(false);
        let (exited, exit) = watch::channel(None);
        let inner = Arc::new(Inner {
            writer: AsyncMutex::new(stdin),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next: AtomicU64::new(1),
            closed: AtomicBool::new(false),
            shutdown,
            sessions: Mutex::new(HashMap::new()),
            creation_unknown: AtomicBool::new(false),
            initialized: AtomicU8::new(0),
            prompt_caps: Mutex::new(Value::Null),
            generations: Mutex::new(HashMap::new()),
            session_new: Mutex::new(None),
            registered: Mutex::new(None),
            diagnostics: Diagnostics::default(),
        });
        let process_inner = inner.clone();
        let process = tokio::spawn(async move {
            let status = tokio::select! {
                status=child.wait()=>{
                    process_inner.diagnostics.0.lock().unwrap().exited_unprompted=true;
                    status
                },
                _=stop.changed()=>{
                    // A process whose stdout just ended is usually exiting by itself: a
                    // short grace keeps its own exit status for diagnostics.
                    let eof = process_inner.diagnostics.0.lock().unwrap().transport_close==Some("eof");
                    let own = if eof {
                        tokio::time::timeout(Duration::from_millis(250),child.wait()).await.ok()
                    } else {
                        child.try_wait().transpose()
                    };
                    match own {
                        Some(s)=>{
                            process_inner.diagnostics.0.lock().unwrap().exited_unprompted=true;
                            s
                        }
                        None=>{
                            // Only this freshly owned process group; never a caller-supplied PID.
                            unsafe{libc::kill(-(pid as i32),libc::SIGTERM);}
                            match tokio::time::timeout(Duration::from_secs(2),child.wait()).await {
                                Ok(s)=>s,
                                Err(_)=>{unsafe{libc::kill(-(pid as i32),libc::SIGKILL);}child.wait().await}
                            }
                        }
                    }
                }
            }?;
            // Reap the leader before reporting process exit. Kill lingering pipe holders.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let code = status.code().unwrap_or(-1);
            {
                use std::os::unix::process::ExitStatusExt;
                process_inner.diagnostics.0.lock().unwrap().exit = Some(match status.code() {
                    Some(code) => format!("exit:{code}"),
                    None => format!("signal:{}", status.signal().unwrap_or(0)),
                });
            }
            process_inner.diagnostics.closed("process_exited");
            close(&process_inner);
            let _ = exited.send(Some(code));
            Ok(code)
        });
        let stderr_diagnostics = inner.diagnostics.clone();
        let stderr = tokio::spawn(async move {
            let mut count = 0;
            let mut buf = [0; 8192];
            while let Ok(n) = stderr.read(&mut buf).await {
                if n == 0 {
                    break;
                }
                count += n as u64;
                stderr_diagnostics.stderr(&buf[..n]);
            }
            count
        });
        let monitor_inner = inner.clone();
        let monitor_policy = policy.clone();
        let monitor = tokio::spawn(async move {
            while !monitor_inner.closed.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if monitor_policy.authority().await.is_err() {
                    monitor_inner.diagnostics.closed("authority_refused");
                    close(&monitor_inner);
                    let _ = monitor_inner.shutdown.send(true);
                    break;
                }
            }
        });
        let (events, event_rx) = mpsc::channel(256);
        let reader_inner = inner.clone();
        let reader = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            let mut permissions = tokio::task::JoinSet::new();
            let mut request_ids = HashSet::new();
            let reason: &'static str = loop {
                let message = tokio::select! {
                    msg=wire::read_frame(&mut reader)=>msg,
                    Some(_)=permissions.join_next(), if !permissions.is_empty()=>continue,
                };
                let v = match message {
                    Ok(Some(v)) => v,
                    Ok(None) => break "eof",
                    Err(Error::FrameTooLarge) => break "frame_too_large",
                    Err(_) => break "invalid_frame",
                };
                if v["jsonrpc"] != "2.0" {
                    break "not_jsonrpc_2";
                }
                if let Some(method) = v["method"].as_str() {
                    if let Some(id) = v.get("id") {
                        // Do not approve a repeated engine action, including on another session.
                        let valid_id = id.is_string() || id.as_i64().is_some();
                        if !valid_id
                            || request_ids.len() >= 4096
                            || !request_ids.insert(id.to_string())
                        {
                            break "invalid_or_repeated_request_id";
                        }
                        let sid = v["params"]["sessionId"].as_str().unwrap_or("").to_owned();
                        let state = reader_inner.sessions.lock().unwrap().get(&sid).cloned();
                        if method == "session/request_permission"
                            && permissions.len() < 32
                            && state
                                .as_ref()
                                .is_some_and(|s| s.load(Ordering::SeqCst) == 1)
                        {
                            let inner = reader_inner.clone();
                            let policy = policy.clone();
                            let id = id.clone();
                            let params = v["params"].clone();
                            let state = state.unwrap();
                            let generation = reader_inner
                                .generations
                                .lock()
                                .unwrap()
                                .get(&sid)
                                .cloned()
                                .unwrap();
                            let expected = generation.load(Ordering::SeqCst);
                            permissions.spawn(async move {
                                // Core owns manual permission waits and cancellation.
                                let decision = policy.decide(&sid, &id, expected, &params).await;
                                let mut outcome = json!({"outcome":"cancelled"});
                                if state.load(Ordering::SeqCst) == 1
                                    && generation.load(Ordering::SeqCst) == expected
                                {
                                    if let Ok(PermissionDecision::Selected(option)) = decision {
                                        if params["options"].as_array().is_some_and(|opts| {
                                            opts.iter().any(|o| o["optionId"] == option)
                                        }) {
                                            outcome =
                                                json!({"outcome":"selected","optionId":option});
                                        }
                                    }
                                }
                                let _ = inner
                                    .send_permission(id, outcome, state, generation, expected)
                                    .await;
                            });
                        } else {
                            let response = if method == "session/request_permission" {
                                json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"cancelled"}}})
                            } else {
                                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Unsupported client request"}})
                            };
                            if reader_inner.send(response).await.is_err() {
                                break "write_failed";
                            }
                        }
                    } else {
                        let event = if method == "session/update" {
                            let sid = v["params"]["sessionId"].as_str().unwrap_or("");
                            if !reader_inner.sessions.lock().unwrap().contains_key(sid) {
                                break "update_for_unknown_session";
                            }
                            Event::Update(v["params"].clone())
                        } else {
                            Event::UnknownNotification
                        };
                        if events.try_send(event).is_err() {
                            break "event_queue_full";
                        }
                    }
                } else {
                    let Some(id) = v["id"].as_u64() else {
                        break "invalid_reply_id";
                    };
                    if v.get("result").is_some() == v.get("error").is_some() {
                        break "malformed_reply";
                    }
                    let tx = reader_inner.pending.lock().unwrap().remove(&id);
                    if let Some(tx) = tx {
                        let result = if let Some(e) = v.get("error") {
                            Err(Error::Rpc(e["code"].as_i64().unwrap_or(-32603)))
                        } else {
                            reader_inner.register_created(id, &v["result"]);
                            Ok(v["result"].clone())
                        };
                        let _ = tx.send(result);
                    } else if events.try_send(Event::LateOrDuplicateReply).is_err() {
                        break "event_queue_full";
                    }
                }
            };
            reader_inner.diagnostics.closed(reason);
            permissions.abort_all();
            while permissions.join_next().await.is_some() {}
            close(&reader_inner);
            let _ = events.try_send(Event::TransportClosed);
            // EOF/overflow is not process absence: request supervised termination.
            let _ = reader_inner.shutdown.send(true);
        });
        Ok(Self {
            inner,
            events: AsyncMutex::new(event_rx),
            process,
            reader,
            monitor,
            stderr,
            exit,
            pid,
        })
    }
    pub fn pid(&self) -> u32 {
        self.pid
    }
    /// The process's retained diagnostics. The handle stays readable after
    /// `shutdown`, which completes the stderr tail and exit status.
    pub fn diagnostics(&self) -> Diagnostics {
        self.inner.diagnostics.clone()
    }
    /// Resolves once the engine process has been observed to exit (or never, while it lives).
    pub async fn exited(&self) {
        let mut exit = self.exit.clone();
        while exit.borrow_and_update().is_none() {
            if exit.changed().await.is_err() {
                break;
            }
        }
    }
    pub fn observation(&self) -> ProcessObservation {
        if self.exit.borrow().is_some() {
            ProcessObservation::ConfirmedExited
        } else if self.inner.closed.load(Ordering::SeqCst) {
            ProcessObservation::Unknown
        } else {
            ProcessObservation::Alive
        }
    }
    pub async fn initialize(&self, deadline: Duration) -> Result<Value> {
        self.inner
            .initialized
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| Error::Busy)?;
        let r = self
            .inner
            .call(
                "initialize",
                json!({"protocolVersion":1,"clientCapabilities":{}}),
                Some(deadline),
            )
            .await?;
        if r["protocolVersion"] != 1 {
            return Err(Error::Protocol);
        }
        *self.inner.prompt_caps.lock().unwrap() =
            r["agentCapabilities"]["promptCapabilities"].clone();
        self.inner.initialized.store(2, Ordering::SeqCst);
        Ok(r)
    }
    /// `agentCapabilities.promptCapabilities` observed at initialize. An absent
    /// field is all-false per ACP: text and resource_link only.
    pub fn prompt_capabilities(&self) -> PromptCapabilities {
        let v = self.inner.prompt_caps.lock().unwrap();
        PromptCapabilities {
            image: v["image"] == true,
            embedded_context: v["embeddedContext"] == true,
        }
    }
    /// Only secret-free relay MCP entries are accepted; credentials stay in broker memory.
    pub async fn start_session(
        &self,
        cwd: &std::path::Path,
        servers: Vec<crate::broker::RelayEntry>,
        deadline: Duration,
    ) -> Result<Session> {
        self.start_session_with_meta(cwd, servers, None, deadline)
            .await
    }
    /// Metadata is produced only by the trusted, version-qualified instruction adapter.
    pub async fn start_session_with_meta(
        &self,
        cwd: &std::path::Path,
        servers: Vec<crate::broker::RelayEntry>,
        meta: Option<Value>,
        deadline: Duration,
    ) -> Result<Session> {
        if self.inner.initialized.load(Ordering::SeqCst) != 2 {
            return Err(Error::Busy);
        }
        if !cwd.is_absolute() {
            return Err(Error::Invalid);
        }
        let mut names = HashSet::new();
        for entry in &servers {
            let value = serde_json::to_value(entry).map_err(|_| Error::Invalid)?;
            if !names.insert(value["name"].as_str().unwrap_or("").to_owned()) {
                return Err(Error::Invalid);
            }
        }
        let mut params = json!({"cwd":cwd,"mcpServers":servers});
        if let Some(meta) = meta {
            if !meta.is_object() {
                return Err(Error::Invalid);
            }
            params["_meta"] = meta;
        }
        if self.inner.creation_unknown.swap(true, Ordering::SeqCst) {
            return Err(Error::Busy);
        }
        let r = self
            .inner
            .call("session/new", params, Some(deadline))
            .await?;
        let id = r["sessionId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(Error::Protocol)?
            .to_owned();
        // The reader registered exactly this new id before delivering the reply
        // (`register_created`); an id it had seen before was left unregistered.
        if self.inner.registered.lock().unwrap().take().as_deref() != Some(id.as_str()) {
            return Err(Error::Protocol);
        }
        let state = self
            .inner
            .sessions
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or(Error::Protocol)?;
        let generation = self
            .inner
            .generations
            .lock()
            .unwrap()
            .get(&id)
            .cloned()
            .ok_or(Error::Protocol)?;
        self.inner.creation_unknown.store(false, Ordering::SeqCst);
        Ok(Session {
            id,
            state,
            config: Mutex::new(r.get("configOptions").cloned().unwrap_or(json!([]))),
            generation,
        })
    }
    fn owns(&self, s: &Session) -> bool {
        self.inner
            .sessions
            .lock()
            .unwrap()
            .get(&s.id)
            .is_some_and(|state| Arc::ptr_eq(state, &s.state))
    }
    /// No model-turn timeout and no replay. A dropped future poisons the session.
    pub async fn prompt(&self, s: &Session, text: &str) -> Result<Value> {
        self.prompt_content(s, vec![json!({"type":"text","text":text})])
            .await
    }
    /// Content is prepared by the trusted host from verified stored input bytes.
    pub async fn prompt_content(&self, s: &Session, blocks: Vec<Value>) -> Result<Value> {
        if blocks.is_empty()
            || blocks
                .iter()
                .any(|b| !matches!(b["type"].as_str(), Some("text" | "image" | "resource")))
        {
            return Err(Error::Invalid);
        }
        if !self.owns(s) {
            return Err(Error::Forbidden);
        }
        s.state
            .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| Error::Busy)?;
        s.generation.fetch_add(1, Ordering::SeqCst);
        let _guard = TurnGuard(s.state.clone());
        let r = self
            .inner
            .call(
                "session/prompt",
                json!({"sessionId":s.id,"prompt":blocks}),
                None,
            )
            .await?;
        if !matches!(
            r["stopReason"].as_str(),
            Some("end_turn" | "max_tokens" | "max_turn_requests" | "refusal" | "cancelled")
        ) {
            return Err(Error::Protocol);
        }
        s.state.store(0, Ordering::SeqCst);
        Ok(r)
    }
    pub async fn cancel(&self, s: &Session) -> Result<()> {
        if !self.owns(s) {
            return Err(Error::Forbidden);
        }
        // Invalidate outstanding approvals immediately. Only a prompt reply restores ready.
        s.generation.fetch_add(1, Ordering::SeqCst);
        s.state.store(2, Ordering::SeqCst);
        self.inner
            .send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":s.id}}))
            .await
    }
    /// Configure only an actually observed option/value and confirm the returned state.
    /// A route parameter named by category (`thought_level`/`effort`) is sent under
    /// the session's one advertised `thought_level` option id (codex-acp
    /// `reasoning_effort`, claude-agent-acp `effort`), as Python's option mappings do.
    /// A timeout, partial response or dropped call leaves the session unusable.
    pub async fn configure(
        &self,
        s: &Session,
        option: &str,
        value: &str,
        deadline: Duration,
    ) -> Result<Value> {
        if !self.owns(s) {
            return Err(Error::Forbidden);
        }
        let observed = s.config_options();
        let option = advertised_option(&observed, option).ok_or(Error::Invalid)?;
        let option = option.as_str();
        fn supports(options: &Value, option: &str, value: &str) -> bool {
            options.as_array().is_some_and(|a| {
                a.iter().any(|o| {
                    o["id"] == option
                        && o["options"].as_array().is_some_and(|a| {
                            a.iter().any(|v| {
                                v["value"] == value
                                    || v["options"]
                                        .as_array()
                                        .is_some_and(|a| a.iter().any(|v| v["value"] == value))
                            })
                        })
                })
            })
        }
        if !supports(&observed, option, value) {
            return Err(Error::Invalid);
        }
        s.state
            .compare_exchange(0, 3, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| Error::Busy)?;
        s.generation.fetch_add(1, Ordering::SeqCst);
        let _guard = TurnGuard(s.state.clone());
        let r = self
            .inner
            .call(
                "session/set_config_option",
                json!({"sessionId":s.id,"configId":option,"value":value}),
                Some(deadline),
            )
            .await?;
        if !r["configOptions"].as_array().is_some_and(|a| {
            a.iter()
                .any(|o| o["id"] == option && o["currentValue"] == value)
        }) {
            return Err(Error::Protocol);
        }
        *s.config.lock().unwrap() = r["configOptions"].clone();
        s.state.store(0, Ordering::SeqCst);
        Ok(r)
    }
    pub async fn next_event(&self) -> Option<Event> {
        self.events.lock().await.recv().await
    }
    /// Drain a bounded snapshot after a reply. Notifications preceding that
    /// reply were queued by the same reader before resolving its request.
    pub async fn drain_events(&self) -> Vec<Event> {
        let mut queue = self.events.lock().await;
        let count = queue.len();
        (0..count).filter_map(|_| queue.try_recv().ok()).collect()
    }
    pub async fn shutdown(mut self) -> Result<i32> {
        self.inner.diagnostics.closed("host_shutdown");
        close(&self.inner);
        let _ = self.inner.shutdown.send(true);
        let result = (&mut self.process).await.map_err(|_| Error::Io)?;
        self.monitor.abort();
        let _ = (&mut self.monitor).await;
        (&mut self.reader).await.map_err(|_| Error::Io)?;
        (&mut self.stderr).await.map_err(|_| Error::Io)?;
        result
    }
}
/// The advertised config id for a route parameter: an exact advertised id, else
/// the unique advertised `thought_level` option for an effort parameter. Never invented.
fn advertised_option(options: &Value, parameter: &str) -> Option<String> {
    let options = options.as_array()?;
    if options.iter().any(|o| o["id"] == parameter) {
        return Some(parameter.to_owned());
    }
    if !matches!(parameter, "thought_level" | "effort") {
        return None;
    }
    let mut effort = options
        .iter()
        .filter(|o| o["category"] == "thought_level" && o["type"] == "select");
    match (effort.next(), effort.next()) {
        (Some(o), None) => o["id"].as_str().map(str::to_owned),
        _ => None,
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        close(&self.inner);
        let _ = self.inner.shutdown.send(true);
    }
}
impl Drop for Inner {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}
