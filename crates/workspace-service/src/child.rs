use crate::local::{failure, FRAME_BYTES};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    path::PathBuf,
    process::Stdio,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::{ChildStdin, Command},
    sync::{broadcast, mpsc, oneshot, watch, Mutex},
    time::timeout,
};

/// Bounds the memory of given-up requests; an older late reply then ends the child as before.
const ABANDONED_LIMIT: usize = 256;
fn increment(number: &str) -> String {
    let mut bytes = number.as_bytes().to_vec();
    for digit in bytes.iter_mut().rev() {
        if *digit < b'9' {
            *digit += 1;
            return String::from_utf8(bytes).unwrap();
        }
        *digit = b'0';
    }
    bytes.insert(0, b'1');
    String::from_utf8(bytes).unwrap()
}
/// Supplied only by the release/native host, never from local client JSON.
/// This is a trusted same-user process boundary, not hostile-code isolation.
#[derive(Clone)]
pub struct LaunchSpec {
    executable: PathBuf,
    arguments: Vec<String>,
    pub(crate) dev_config: Option<PathBuf>,
    tools: Option<PathBuf>,
}
/// The environment every process this service starts runs with: never PYTHONPATH, loader
/// injection, engine flags or credentials, and in dev always the channel's own directories.
pub(crate) fn runtime_environment(command: &mut Command, config: &crate::Config) -> io::Result<()> {
    command.env_clear();
    for key in [
        "HOME",
        "PATH",
        "LANG",
        "LC_ALL",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    if workspace_protocol::channel::current().is_dev() {
        // Config/state live below the channel's private registry root. HOME is unchanged:
        // native engine accounts remain in the user's own native directories.
        for (key, leaf) in [
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_DATA_HOME", "data"),
        ] {
            let directory = if key == "XDG_CONFIG_HOME" {
                config
                    .launch
                    .dev_config
                    .clone()
                    .unwrap_or_else(|| config.state_dir.join(leaf))
            } else {
                config.state_dir.join(leaf)
            };
            crate::local::private_tree(&directory)?;
            command.env(key, directory);
        }
        command.env("XDG_RUNTIME_DIR", &config.runtime_dir);
    }
    command.env("CORTEX_WORKSPACE_CHANNEL", if workspace_protocol::channel::current().is_dev() { "dev" } else { "release" });
    command.env("CORTEX_WORKSPACE_STATE_DIR", &config.state_dir);
    command.env("PYTHONDONTWRITEBYTECODE", "1");
    Ok(())
}
impl LaunchSpec {
    pub fn new(executable: PathBuf, arguments: Vec<String>) -> io::Result<Self> {
        if !executable.is_absolute() || !executable.is_file() {
            return Err(failure("explicit_executable_required"));
        }
        Ok(Self {
            executable: executable.canonicalize()?,
            arguments,
            dev_config: None,
            tools: None,
        })
    }
    /// The bundled machine helper. Without it the service offers no machine facts or actions.
    pub fn with_tools(mut self, tools: PathBuf) -> io::Result<Self> {
        if !tools.is_absolute() || !tools.is_file() {
            return Err(failure("explicit_tools_required"));
        }
        self.tools = Some(tools.canonicalize()?);
        Ok(self)
    }
    pub(crate) fn tools(&self) -> Option<(&PathBuf, &PathBuf)> {
        self.tools.as_ref().map(|tools| (&self.executable, tools))
    }
}
pub struct Child {
    input: Mutex<Option<ChildStdin>>,
    refusals: mpsc::Sender<Value>,
    pending: Mutex<HashMap<u64, oneshot::Sender<io::Result<Value>>>>,
    /// Requests given up on. Their late replies are dropped, not read as a broken protocol.
    abandoned: Mutex<VecDeque<u64>>,
    /// The first thing that ended this child.
    ended: std::sync::Mutex<Option<String>>,
    next: AtomicU64,
    pub valid: AtomicBool,
    pub ready: AtomicBool,
    pub exited: AtomicBool,
    pub sessions: Mutex<HashSet<String>>,
    stop: Mutex<Option<oneshot::Sender<()>>>,
    stopped: watch::Receiver<bool>,
    request_timeout: Duration,
    project: String,
    events: broadcast::Sender<Value>,
    cursor: Mutex<(String, String)>,
    poller: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// Wakes a journal poll that has backed off, because someone just asked this child to do something.
    activity: tokio::sync::Notify,
}
/// A quiet project is asked less and less often, down to this.
const JOURNAL_POLL_FIRST: Duration = Duration::from_millis(500);
const JOURNAL_POLL_LAST: Duration = Duration::from_secs(4);
impl Child {
    pub async fn spawn(
        config: &crate::Config,
        root: &std::path::Path,
        project: &str,
        actor: &str,
        events: broadcast::Sender<Value>,
    ) -> io::Result<Arc<Self>> {
        let mut cmd = Command::new(&config.launch.executable);
        cmd.args(&config.launch.arguments)
            .args(["--project-id", project, "--actor-id", actor])
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        runtime_environment(&mut cmd, config)?;
        let parent = unsafe { libc::getpid() };
        unsafe {
            cmd.pre_exec(move || {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent {
                    return Err(failure("service_parent_lost"));
                }
                Ok(())
            });
        }
        let mut process = cmd.spawn()?;
        let input = process.stdin.take();
        let output = process.stdout.take().unwrap();
        let stderr = process.stderr.take().unwrap();
        let (stop_tx, stop_rx) = oneshot::channel();
        let (done_tx, stopped) = watch::channel(false);
        let (refusals, mut refusal_rx) = mpsc::channel(64);
        let child = Arc::new(Self {
            refusals,
            input: Mutex::new(input),
            pending: Mutex::new(HashMap::new()),
            abandoned: Mutex::new(VecDeque::new()),
            ended: std::sync::Mutex::new(None),
            next: AtomicU64::new(1),
            valid: AtomicBool::new(true),
            ready: AtomicBool::new(false),
            exited: AtomicBool::new(false),
            sessions: Mutex::new(HashSet::new()),
            stop: Mutex::new(Some(stop_tx)),
            stopped,
            request_timeout: config.request_timeout,
            project: project.into(),
            events,
            cursor: Mutex::new((String::new(), String::new())),
            poller: Mutex::new(None),
            activity: tokio::sync::Notify::new(),
        });
        let refusal_child = Arc::downgrade(&child);
        let refusal_task = tokio::spawn(async move {
            while let Some(message) = refusal_rx.recv().await {
                let Some(child) = refusal_child.upgrade() else {
                    break;
                };
                if child.send(message).await.is_err() {
                    child.invalidate("host_reply_write_failed").await;
                    break;
                }
            }
        });
        // Drain stderr independently, without retaining or publishing private logs.
        let stderr_task = tokio::spawn(async move {
            let mut reader = stderr;
            let mut buf = [0; 8192];
            loop {
                match reader.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    _ => (),
                }
            }
        });
        let out_child = child.clone();
        let stdout_task = tokio::spawn(async move {
            let mut reader = BufReader::new(output);
            loop {
                let mut bytes = Vec::new();
                let read = (&mut reader)
                    .take((FRAME_BYTES + 1) as u64)
                    .read_until(b'\n', &mut bytes)
                    .await;
                if !matches!(read,Ok(n) if n>0 && n<=FRAME_BYTES && bytes.last()==Some(&b'\n')) {
                    out_child
                        .invalidate("child_stdout_closed_or_oversized")
                        .await;
                    // No more decoding after a broken boundary, but never block the
                    // owned process on a full stdout pipe while it awaits explicit stop.
                    let _ = tokio::io::copy(&mut reader, &mut tokio::io::sink()).await;
                    break;
                }
                let message = serde_json::from_slice::<Value>(&bytes);
                if let Ok(message) = message {
                    if out_child.receive(message).await.is_ok() {
                        continue;
                    }
                }
                out_child.invalidate("child_protocol_error").await;
                // Keep draining even after a malformed frame until the owner stops the child.
            }
        });
        let owner = child.clone();
        let grace = config.shutdown_timeout;
        tokio::spawn(async move {
            tokio::select! {
                _ = process.wait() => (),
                _ = stop_rx => {
                    let graceful=async {
                        let sessions:Vec<_>=owner.sessions.lock().await.iter().cloned().collect();
                        for session in sessions {
                            owner.notify("session/cancel",json!({"sessionId":session})).await?;
                        }
                        // Only explicit service shutdown (not client detach) closes this pipe.
                        owner.input.lock().await.take();
                        process.wait().await.map(|_|())
                    };
                    if !matches!(timeout(grace,graceful).await,Ok(Ok(()))) {
                        let _=process.start_kill(); let _=process.wait().await;
                    }
                }
            }
            owner.exited.store(true, Ordering::SeqCst);
            owner.invalidate("child_exited").await;
            stdout_task.abort();
            stderr_task.abort();
            refusal_task.abort();
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            let _ = refusal_task.await;
            let _ = done_tx.send(true);
        });
        Ok(child)
    }
    /// Whether this process can still serve its project. A slow child is healthy.
    pub fn healthy(&self) -> bool {
        self.valid.load(Ordering::SeqCst) && !self.exited.load(Ordering::SeqCst)
    }
    pub fn failure_reason(&self) -> Option<String> {
        self.ended.lock().ok()?.clone()
    }
    pub fn reason(&self) -> String {
        self.failure_reason().unwrap_or_else(|| "child_exited".into())
    }
    pub async fn invalidate(&self, reason: &str) {
        if let Ok(mut ended) = self.ended.lock() {
            ended.get_or_insert_with(|| reason.into());
        }
        self.valid.store(false, Ordering::SeqCst);
        self.ready.store(false, Ordering::SeqCst);
        for (_, sender) in self.pending.lock().await.drain() {
            let _ = sender.send(Err(failure("unknown_outcome")));
        }
        let _ = self
            .events
            .send(json!({"projectId":self.project,"type":"resync_required","reason":reason}));
    }
    async fn receive(&self, message: Value) -> io::Result<()> {
        if message["jsonrpc"] != "2.0" {
            return Err(failure("invalid_jsonrpc"));
        }
        if message.get("method").is_some() {
            let method = message["method"]
                .as_str()
                .ok_or_else(|| failure("invalid_method"))?;
            if message.get("id").is_some() {
                // The slice advertises no host filesystem, terminal or permission capability.
                // Refuse rather than granting an unimplemented host request.
                self.refusals.try_send(json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":"host_capability_unavailable"}}))
                    .map_err(|_|failure("host_reply_queue_overflow"))?;
            } else if method == "session/update" || method == "_cortex/workspace/update" {
                let _ = self.events.send(
                    json!({"projectId":self.project,"type":"invalidation","notification":message}),
                );
            } else {
                return Err(failure("unknown_notification"));
            }
            return Ok(());
        }
        let id = message["id"]
            .as_u64()
            .ok_or_else(|| failure("invalid_reply_id"))?;
        if message.get("result").is_some() == message.get("error").is_some() {
            return Err(failure("invalid_reply"));
        }
        let sender = self.pending.lock().await.remove(&id);
        let Some(sender) = sender else {
            let mut abandoned = self.abandoned.lock().await;
            let Some(index) = abandoned.iter().position(|known| *known == id) else {
                return Err(failure("unmatched_reply"));
            };
            abandoned.remove(index);
            return Ok(());
        };
        let result = if message.get("error").is_some() {
            // Do not leak remote diagnostic bodies into service errors.
            let code = match message["error"]["message"].as_str() {
                Some("stale_version") => "stale_version",
                Some("stale_epoch") => "stale_epoch",
                Some("task_paused") => "task_paused",
                Some("pause_settling") => "pause_settling",
                Some("task_not_paused") => "task_not_paused",
                Some("task_not_resumable") => "task_not_resumable",
                Some("execution_session_unavailable") => "execution_session_unavailable",
                Some("report_unavailable") => "report_unavailable",
                _ => "acp_request_refused",
            };
            Err(failure(code))
        } else {
            Ok(message["result"].clone())
        };
        let _ = sender.send(result);
        Ok(())
    }
    async fn send(&self, message: Value) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(&message).map_err(|_| failure("invalid_json"))?;
        if bytes.len() + 1 > FRAME_BYTES {
            return Err(failure("frame_too_large"));
        }
        bytes.push(b'\n');
        timeout(self.request_timeout, async {
            let mut input = self.input.lock().await;
            input
                .as_mut()
                .ok_or_else(|| failure("child_stopped"))?
                .write_all(&bytes)
                .await
        })
        .await
        .map_err(|_| failure("child_write_timeout"))?
    }
    pub async fn notify(&self, method: &str, params: Value) -> io::Result<()> {
        self.send(json!({"jsonrpc":"2.0","method":method,"params":params}))
            .await
    }
    pub async fn request(&self, method: &str, params: Value) -> io::Result<Value> {
        if !self.valid.load(Ordering::SeqCst) {
            return Err(failure("child_unavailable"));
        }
        if !matches!(params.get("requestId").and_then(Value::as_str), Some("service-events" | "service-read")) {
            self.activity.notify_one();
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending.lock().await;
            if pending.len() >= 64 {
                drop(pending);
                self.invalidate("request_queue_overflow").await;
                return Err(failure("resync_required"));
            }
            pending.insert(id, tx);
        }
        if self
            .send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await
            .is_err()
        {
            self.invalidate("child_write_failed").await;
            return Err(failure("unknown_outcome"));
        }
        match timeout(self.request_timeout, rx).await {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => Err(failure("unknown_outcome")),
            Err(_) => {
                // A long synchronous step is not a dead child. This one request stays unknown and
                // is never resent; the journal poller resnapshots once the child answers again.
                if self.pending.lock().await.remove(&id).is_some() {
                    let mut abandoned = self.abandoned.lock().await;
                    if abandoned.len() >= ABANDONED_LIMIT {
                        abandoned.pop_front();
                    }
                    abandoned.push_back(id);
                }
                self.ready.store(false, Ordering::SeqCst);
                let _ = self.events.send(json!({"projectId":self.project,"type":"resync_required","reason":"child_slow"}));
                Err(failure("unknown_outcome"))
            }
        }
    }
    /// A prompt turn outlives every request deadline, so its reply is awaited without one and a
    /// long turn never invalidates the child. The reply is routed and dropped.
    pub async fn prompt(&self, params: Value) -> io::Result<()> {
        if !self.valid.load(Ordering::SeqCst) {
            return Err(failure("child_unavailable"));
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        self.send(json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":params}))
            .await?;
        let _ = rx.await;
        Ok(())
    }
    pub async fn install_snapshot(&self, snapshot: &Value) -> io::Result<()> {
        workspace_protocol::validate("ProjectSnapshot", snapshot)
            .map_err(|_| failure("invalid_snapshot"))?;
        if snapshot["projectId"] != self.project {
            return Err(failure("snapshot_project_mismatch"));
        }
        let cursor = snapshot["eventCursor"]
            .as_str()
            .ok_or_else(|| failure("snapshot_cursor_missing"))?;
        *self.cursor.lock().await = (
            cursor.into(),
            snapshot["highWatermark"].as_str().unwrap().into(),
        );
        Ok(())
    }
    pub async fn start_polling(self: &Arc<Self>) {
        let weak = Arc::downgrade(self);
        *self.poller.lock().await = Some(tokio::spawn(async move {
            let mut wait = JOURNAL_POLL_FIRST;
            loop {
                let Some(woken) = weak.upgrade() else {
                    break;
                };
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = woken.activity.notified() => wait = JOURNAL_POLL_FIRST,
                }
                drop(woken);
                let Some(child) = weak.upgrade() else {
                    break;
                };
                if !child.valid.load(Ordering::SeqCst) {
                    break;
                }
                if !child.ready.load(Ordering::SeqCst) {
                    // Comes back by itself, with or without a window open to ask for it.
                    let query = json!({"protocol":{"major":1,"minor":0},"requestId":"service-read",
                        "projectId":child.project,"operation":"project.snapshot","target":null});
                    if let Ok(snapshot) = child.request("_cortex/workspace", query).await {
                        if child.install_snapshot(&snapshot).await.is_ok() && child.healthy() {
                            child.ready.store(true, Ordering::SeqCst);
                            let _ = child.events.send(json!({"projectId":child.project,"type":"project_state"}));
                        }
                    }
                    continue;
                }
                // Serialize cursor advancement with resnapshot, not with unrelated requests.
                let mut position = child.cursor.lock().await;
                let query = json!({"protocol":{"major":1,"minor":0},"requestId":"service-events",
                    "projectId":child.project,"operation":"events.page","target":null,"cursor":position.0,"limit":100});
                let page = child.request("_cortex/workspace", query).await;
                let page = match page {
                    Ok(page) if workspace_protocol::validate("EventPage", &page).is_ok() => page,
                    _ => {
                        child.ready.store(false, Ordering::SeqCst);
                        let _=child.events.send(json!({"projectId":child.project,"type":"resync_required","reason":"journal_unavailable"}));
                        continue;
                    }
                };
                let mut last = position.1.clone();
                let mut consistent = true;
                for event in page["items"].as_array().unwrap() {
                    if event["sequence"] != increment(&last)
                        || event.get("projectId").is_some_and(|p| p != &child.project)
                    {
                        consistent = false;
                        break;
                    }
                    last = event["sequence"].as_str().unwrap().into();
                }
                let Some(cursor) = page["eventCursor"].as_str() else {
                    child.ready.store(false, Ordering::SeqCst);
                    continue;
                };
                if !consistent || (page["nextCursor"].is_null() && page["highWatermark"] != last) {
                    child.ready.store(false, Ordering::SeqCst);
                    let _=child.events.send(json!({"projectId":child.project,"type":"resync_required","reason":"journal_gap"}));
                    continue;
                }
                *position = (cursor.into(), last);
                if !page["items"].as_array().unwrap().is_empty() {
                    let _ = child.events.send(
                        json!({"projectId":child.project,"type":"public_events","page":page}),
                    );
                    wait = JOURNAL_POLL_FIRST;
                } else {
                    wait = (wait * 2).min(JOURNAL_POLL_LAST);
                }
            }
        }));
    }
    pub async fn stop(&self) {
        if let Some(poller) = self.poller.lock().await.take() {
            poller.abort();
            let _ = poller.await;
        }
        if let Some(sender) = self.stop.lock().await.take() {
            let _ = sender.send(());
        }
        let mut done = self.stopped.clone();
        while !*done.borrow_and_update() {
            if done.changed().await.is_err() {
                break;
            }
        }
    }
}
