//! Explicit external MCP grants, durable reservation port and no automatic replay.
//! Python oracle: engine/external_proxy.py:105-287; mcp_bridge.py:178-245.
use crate::{
    broker::Handler,
    gateway::{Channel, Credentials},
    wire, Error, FutureResult, Result,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, BufReader},
    process::{ChildStdin, ChildStdout, Command},
    sync::{watch, Mutex},
    task::JoinHandle,
};

/// Registry policy, never inferred from MCP annotations. Core narrows coordinator
/// and read-only profiles; consultant channels must receive no external server.
pub struct Grants {
    pub name: String,
    pub allowed_tools: BTreeSet<String>,
    pub always_load: BTreeSet<String>,
    pub read_only_tools: BTreeSet<String>,
    pub coordinator_tools: Option<BTreeSet<String>>,
}
impl Grants {
    pub fn narrow(&mut self, channel: &Channel) -> Result<()> {
        let read_only = matches!(
            channel.profile.as_deref(),
            Some(
                "worker-architect"
                    | "worker-ux-designer"
                    | "worker-code-reviewer"
                    | "worker-explorer"
                    | "worker-planner"
                    | "worker-security-auditor"
                    | "worker-accessibility-auditor"
                    | "worker-database-architect"
                    | "worker-performance-engineer"
                    | "worker-build-verification"
            )
        ) || channel.stage.as_deref() == Some("review");
        if matches!(
            channel.profile.as_deref(),
            Some("worker-senior-consultant" | "senior-consultant")
        ) {
            return Err(Error::Forbidden);
        }
        crate::schema::catalogue(
            &channel.role,
            channel.profile.as_deref(),
            channel.stage.as_deref(),
        )?;
        if read_only || channel.role == "coordinator" {
            self.allowed_tools
                .retain(|n| self.read_only_tools.contains(n));
        }
        if channel.role == "coordinator" {
            let lookup = self
                .coordinator_tools
                .as_ref()
                .unwrap_or(&self.read_only_tools);
            self.allowed_tools.retain(|n| lookup.contains(n));
        }
        if self.allowed_tools.is_empty() {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
pub enum Reservation {
    Dispatch,
    Replay(Value),
    Unknown,
}
/// Implementations MUST persist authorization+reservation before Dispatch, bind
/// request identity to channel/server/id/digest, and persist a complete response
/// before returning it. No DB transaction may span the external process call.
pub trait ExternalCore: Send + Sync + 'static {
    fn authorize<'a>(
        &'a self,
        credentials: &'a Credentials,
        server: &'a str,
    ) -> FutureResult<'a, Channel>;
    fn reserve<'a>(
        &'a self,
        credentials: &'a Credentials,
        channel: &'a Channel,
        server: &'a str,
        id: Value,
        digest: String,
    ) -> FutureResult<'a, Reservation>;
    fn reserve_tool<'a>(
        &'a self,
        credentials: &'a Credentials,
        channel: &'a Channel,
        server: &'a str,
        id: Value,
        digest: String,
        _tool: &'a str,
        _read_only: bool,
    ) -> FutureResult<'a, Reservation> {
        self.reserve(credentials, channel, server, id, digest)
    }
    fn complete<'a>(
        &'a self,
        credentials: &'a Credentials,
        server: &'a str,
        id: Value,
        reply: Value,
    ) -> FutureResult<'a, ()>;
}
/// Trusted service-prepared command and explicit registry environment. No ambient
/// environment is inherited; channel credentials must stay in this proxy.
pub struct ExternalLaunch {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub environment: BTreeMap<String, String>,
}
impl ExternalLaunch {
    fn validate(&self) -> Result<()> {
        if !self.executable.is_absolute()
            || !self.executable.is_file()
            || !self.cwd.is_absolute()
            || !self.cwd.is_dir()
        {
            return Err(Error::Invalid);
        }
        if self.args.iter().any(|s| s.contains('\0'))
            || self.environment.iter().any(|(k, v)| {
                k.is_empty()
                    || k.contains(['=', '\0'])
                    || k.starts_with("CORTEX_CHANNEL_")
                    || v.contains('\0')
            })
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }
}
/// Host-prepared OS policy. Called only after fork, before exec; implementations
/// must use async-signal-safe operations, without allocation or application locks.
pub trait Containment: Send + Sync {
    unsafe fn enter(&self) -> std::io::Result<()>;
}
struct Transport {
    input: ChildStdin,
    output: BufReader<ChildStdout>,
}
pub struct ExternalProxy {
    credentials: Arc<Credentials>,
    core: Arc<dyn ExternalCore>,
    grants: Grants,
    io: Mutex<Transport>,
    stop: watch::Sender<bool>,
    closed: Arc<AtomicBool>,
    process: Option<JoinHandle<Result<()>>>,
    monitor: Option<JoinHandle<()>>,
    stderr_tail: Arc<std::sync::Mutex<String>>,
}
impl ExternalProxy {
    /// Launches without an OS containment policy. Only for a registry command the
    /// owner explicitly configured to run uncontained; this adapter grants no OS
    /// access of its own and still enforces the tool grants and channel binding.
    pub async fn launch(
        spec: ExternalLaunch,
        credentials: Credentials,
        core: Arc<dyn ExternalCore>,
        grants: Grants,
    ) -> Result<Self> {
        Self::launch_inner(spec, credentials, core, grants, None).await
    }
    pub async fn launch_contained(
        spec: ExternalLaunch,
        credentials: Credentials,
        core: Arc<dyn ExternalCore>,
        grants: Grants,
        containment: Arc<dyn Containment>,
    ) -> Result<Self> {
        Self::launch_inner(spec, credentials, core, grants, Some(containment)).await
    }
    async fn launch_inner(
        spec: ExternalLaunch,
        credentials: Credentials,
        core: Arc<dyn ExternalCore>,
        mut grants: Grants,
        containment: Option<Arc<dyn Containment>>,
    ) -> Result<Self> {
        if grants.name.is_empty()
            || matches!(grants.name.as_str(), "cortex" | "worker" | "coordinator")
            || grants.allowed_tools.is_empty()
        {
            return Err(Error::Invalid);
        }
        let channel = core.authorize(&credentials, &grants.name).await?;
        grants.narrow(&channel)?;
        spec.validate()?;
        let mut command = Command::new(&spec.executable);
        if let Some(containment) = containment {
            unsafe {
                command.pre_exec(move || containment.enter());
            }
        }
        let mut child = command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(spec.environment)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()?;
        let pid = child.id().ok_or(Error::Io)?;
        let input = child.stdin.take().ok_or(Error::Io)?;
        let output = BufReader::new(child.stdout.take().ok_or(Error::Io)?);
        let mut stderr = child.stderr.take().ok_or(Error::Io)?;
        let (stop, mut stopping) = watch::channel(false);
        let closed = Arc::new(AtomicBool::new(false));
        let closing = closed.clone();
        let tail: Arc<std::sync::Mutex<String>> = Arc::default();
        let drained = tail.clone();
        let process = tokio::spawn(async move {
            let drain = tokio::spawn(async move {
                let mut b = [0u8; 8192];
                while let Ok(n) = stderr.read(&mut b).await {
                    if n == 0 {
                        break;
                    }
                    // Only the newest bytes are kept, for a secret-free exit diagnostic.
                    if let Ok(mut t) = drained.lock() {
                        t.push_str(&String::from_utf8_lossy(&b[..n]));
                        if t.len() > 4096 {
                            let cut = t.len() - 4096;
                            let cut = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(t.len());
                            t.drain(..cut);
                        }
                    }
                }
            });
            let status = tokio::select! {
                status=child.wait()=>status,
                _=stopping.changed()=>{
                    unsafe{libc::kill(-(pid as i32),libc::SIGKILL);}
                    child.wait().await
                }
            };
            closing.store(true, Ordering::SeqCst);
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            drain.await.map_err(|_| Error::Io)?;
            status?;
            Ok(())
        });
        let credentials = Arc::new(credentials);
        let c = credentials.clone();
        let policy = core.clone();
        let name = grants.name.clone();
        let kill = stop.clone();
        let done = closed.clone();
        let monitor = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(50)).await;
                if done.load(Ordering::SeqCst) {
                    break;
                }
                if policy.authorize(&c, &name).await.is_err() {
                    let _ = kill.send(true);
                    break;
                }
            }
        });
        Ok(Self {
            credentials,
            core,
            grants,
            io: Mutex::new(Transport { input, output }),
            stop,
            closed,
            process: Some(process),
            monitor: Some(monitor),
            stderr_tail: tail,
        })
    }
    /// The newest non-empty stderr line of the server, made safe to show: control
    /// characters dropped, long or credential-like words replaced, 160 characters at most.
    pub fn stderr_line(&self) -> Option<String> {
        let text = self.stderr_tail.lock().ok()?;
        let line = text.lines().rev().map(str::trim).find(|l| !l.is_empty())?;
        // A credential-looking word hides itself and the word after it.
        let mut hide_next = false;
        let safe = line
            .split_whitespace()
            .map(|w| {
                let l = w.to_ascii_lowercase();
                let sensitive = w.len() > 48
                    || ["token", "secret", "password", "bearer", "key=", "auth"].iter().any(|k| l.contains(k));
                let hidden = sensitive || hide_next;
                hide_next = sensitive;
                if hidden {
                    "<redacted>".to_owned()
                } else {
                    w.chars().filter(|c| !c.is_control()).collect()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        Some(safe.chars().take(160).collect())
    }
    /// The server's stderr shows it waits for the owner to sign in (nothing is returned from it).
    pub fn needs_sign_in(&self) -> bool {
        self.stderr_tail.lock().ok().is_some_and(|t| sign_in_need(&t).is_some())
    }
    /// True once the contained server process has exited or this proxy was fenced.
    /// An observed exit only; `false` is not evidence that the server is healthy.
    pub fn exited(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
    async fn exchange(&self, request: &Value) -> Result<Option<Value>> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Closed);
        }
        let mut io = self.io.lock().await;
        wire::write_frame(&mut io.input, request).await?;
        let Some(id) = request.get("id") else {
            return Ok(None);
        };
        loop {
            let reply = wire::read_frame(&mut io.output)
                .await?
                .ok_or(Error::Closed)?;
            if reply["jsonrpc"] != "2.0" {
                return Err(Error::Protocol);
            }
            if reply.get("method").is_some() {
                // No server-initiated sampling, elicitation, roots or hidden tools.
                if let Some(id) = reply.get("id") {
                    wire::write_frame(&mut io.input,&json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Runtime gateway does not grant this method"}})).await?;
                }
                continue;
            }
            if reply.get("id") != Some(id)
                || reply.get("result").is_some() == reply.get("error").is_some()
            {
                return Err(Error::Protocol);
            }
            return Ok(Some(reply));
        }
    }
    async fn dispatch(&self, request: Value) -> Result<Option<Value>> {
        let channel = self
            .core
            .authorize(&self.credentials, &self.grants.name)
            .await?;
        let method = request["method"].as_str().ok_or(Error::Invalid)?;
        let id = request.get("id").cloned();
        if request["jsonrpc"] != "2.0"
            || id
                .as_ref()
                .is_some_and(|v| !v.is_string() && v.as_i64().is_none())
        {
            return Err(Error::Invalid);
        }
        let empty = match method {
            "resources/list" => Some("resources"),
            "resources/templates/list" => Some("resourceTemplates"),
            "prompts/list" => Some("prompts"),
            _ => None,
        };
        if let Some(key) = empty {
            return Ok(id.map(|id| json!({"jsonrpc":"2.0","id":id,"result":{key:[]}})));
        }
        if !matches!(
            method,
            "initialize"
                | "notifications/initialized"
                | "ping"
                | "tools/list"
                | "tools/call"
                | "notifications/cancelled"
        ) {
            return Err(Error::Forbidden);
        }
        if id.is_none()
            && !matches!(
                method,
                "notifications/initialized" | "notifications/cancelled"
            )
        {
            return Err(Error::Forbidden);
        }
        if method == "tools/call" {
            let tool = request["params"]["name"].as_str().ok_or(Error::Invalid)?;
            if !self.grants.allowed_tools.contains(tool) {
                return Err(Error::Forbidden);
            }
            let digest = format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&request["params"]).map_err(|_| Error::Invalid)?)
            );
            match self
                .core
                .reserve_tool(
                    &self.credentials,
                    &channel,
                    &self.grants.name,
                    id.clone().unwrap(),
                    digest,
                    tool,
                    self.grants.read_only_tools.contains(tool),
                )
                .await?
            {
                Reservation::Replay(reply) => return Ok(Some(reply)),
                Reservation::Unknown => return Err(Error::Unknown),
                Reservation::Dispatch => {}
            }
        }
        // A dropped future after dispatch fences this connection; never retry.
        let mut guard = ExchangeGuard {
            stop: &self.stop,
            closed: &self.closed,
            armed: true,
        };
        let mut reply = tokio::time::timeout(Duration::from_secs(300), self.exchange(&request))
            .await
            .map_err(|_| Error::Unknown)??;
        self.core
            .authorize(&self.credentials, &self.grants.name)
            .await?;
        if method == "tools/list" {
            let tools = reply
                .as_mut()
                .and_then(|v| v["result"]["tools"].as_array_mut())
                .ok_or(Error::Protocol)?;
            let mut names = BTreeSet::new();
            tools.retain(|t| {
                t["name"]
                    .as_str()
                    .is_some_and(|n| self.grants.allowed_tools.contains(n))
            });
            for t in tools {
                let name = t["name"].as_str().ok_or(Error::Protocol)?.to_owned();
                if !names.insert(name.clone()) || !t["inputSchema"].is_object() {
                    return Err(Error::Protocol);
                }
                if self.grants.always_load.contains(&name) {
                    if !t["_meta"].is_object() {
                        t["_meta"] = json!({})
                    }
                    t["_meta"]["anthropic/alwaysLoad"] = json!(true);
                }
            }
        }
        if method == "tools/call" {
            self.core
                .complete(
                    &self.credentials,
                    &self.grants.name,
                    id.unwrap(),
                    reply.clone().ok_or(Error::Protocol)?,
                )
                .await?;
        }
        guard.armed = false;
        Ok(reply)
    }
    pub async fn shutdown(mut self) -> Result<()> {
        self.closed.store(true, Ordering::SeqCst);
        let _ = self.stop.send(true);
        if let Some(m) = self.monitor.take() {
            m.abort();
            let _ = m.await;
        }
        if let Some(p) = self.process.take() {
            p.await.map_err(|_| Error::Io)??
        }
        Ok(())
    }
}
struct ExchangeGuard<'a> {
    stop: &'a watch::Sender<bool>,
    closed: &'a AtomicBool,
    armed: bool,
}
impl Drop for ExchangeGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.closed.store(true, Ordering::SeqCst);
            let _ = self.stop.send(true);
        }
    }
}
impl Handler for ExternalProxy {
    fn handle<'a>(&'a self, request: Value) -> FutureResult<'a, Option<Value>> {
        Box::pin(async move {
            let id = request.get("id").cloned();
            match self.dispatch(request).await{
                Ok(reply)=>Ok(reply),
                Err(error)=>Ok(id.map(|id|json!({"jsonrpc":"2.0","id":id,"error":{"code":-32001,"message":error.to_string()}})))
            }
        })
    }
}
impl Drop for ExternalProxy {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(m) = self.monitor.take() {
            m.abort()
        }
    }
}

/// Whether a server's stderr shows it waits for the owner to sign in (OAuth), and the
/// sign-in address when one is printed. The address is returned only as a plain, bounded
/// `https` URL (no whitespace or control characters, at most 2048 bytes) taken from a line
/// that asks to authorize or sign in, or from an address that itself names `oauth` or
/// `authorize`. It is an in-memory value for the owner's browser, never a log or report.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SignInNeed {
    pub url: Option<String>,
}
/// Reads server stderr text for a sign-in requirement (see [`SignInNeed`]); `None` when it shows none.
pub fn sign_in_need(text: &str) -> Option<SignInNeed> {
    let mut url = None;
    let mut phrase = false;
    for line in text.lines() {
        let lower = line.to_ascii_lowercase();
        let asks = ["authorize this client", "please authorize", "sign-in for this server", "sign in for this server",
            "waiting for authorization", "authentication required", "please sign in", "please log in", "visit the following url", "open the following url"]
            .iter()
            .any(|k| lower.contains(k));
        phrase |= asks;
        for word in line.split_whitespace() {
            let word = word.trim_matches(|c: char| matches!(c, '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']' | ',' | ';' | '.'));
            let Some(rest) = word.strip_prefix("https://") else { continue };
            let host_ok = rest
                .split(['/', '?', '#'])
                .next()
                .is_some_and(|h| !h.is_empty() && h.bytes().all(|b| b.is_ascii_alphanumeric() || b".-:".contains(&b)));
            let w = word.to_ascii_lowercase();
            let names_auth = w.contains("oauth") || w.contains("authorize");
            if host_ok && word.len() <= 2048 && !word.chars().any(|c| c.is_control()) && (asks || names_auth) {
                url = Some(word.to_owned());
                phrase = true;
            }
        }
    }
    phrase.then_some(SignInNeed { url })
}

/// A server started only to learn whether it is ready, and to hold a sign-in open: the
/// service sends one MCP `initialize` and watches for the answer. A server that waits for
/// the owner to sign in answers only after the sign-in completes, so keeping this process
/// alive lets its own local OAuth callback finish. It carries no channel and no tool grant.
pub struct SignInProbe {
    ready: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    tail: Arc<std::sync::Mutex<String>>,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}
impl SignInProbe {
    pub async fn launch(spec: ExternalLaunch, containment: Option<Arc<dyn Containment>>) -> Result<Self> {
        spec.validate()?;
        let mut command = Command::new(&spec.executable);
        if let Some(containment) = containment {
            unsafe {
                command.pre_exec(move || containment.enter());
            }
        }
        let mut child = command
            .args(&spec.args)
            .current_dir(&spec.cwd)
            .env_clear()
            .envs(spec.environment)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()?;
        let pid = child.id().ok_or(Error::Io)?;
        let mut input = child.stdin.take().ok_or(Error::Io)?;
        let mut output = BufReader::new(child.stdout.take().ok_or(Error::Io)?);
        let mut stderr = child.stderr.take().ok_or(Error::Io)?;
        let (stop, mut stopping) = watch::channel(false);
        let ready = Arc::new(AtomicBool::new(false));
        let closed = Arc::new(AtomicBool::new(false));
        let tail: Arc<std::sync::Mutex<String>> = Arc::default();
        let (r, c, t) = (ready.clone(), closed.clone(), tail.clone());
        let task = tokio::spawn(async move {
            let drain = tokio::spawn(async move {
                let mut b = [0u8; 8192];
                while let Ok(n) = stderr.read(&mut b).await {
                    if n == 0 {
                        break;
                    }
                    if let Ok(mut text) = t.lock() {
                        text.push_str(&String::from_utf8_lossy(&b[..n]));
                        if text.len() > 16384 {
                            let cut = text.len() - 16384;
                            let cut = (cut..text.len()).find(|i| text.is_char_boundary(*i)).unwrap_or(text.len());
                            text.drain(..cut);
                        }
                    }
                }
            });
            let ask = json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"cortex-sign-in-check","version":"1"}}});
            let talk = async {
                wire::write_frame(&mut input, &ask).await.ok()?;
                loop {
                    let reply = wire::read_frame(&mut output).await.ok()??;
                    if reply.get("method").is_none() && reply.get("id") == Some(&json!(1)) {
                        return reply.get("result").is_some().then_some(());
                    }
                }
            };
            tokio::select! {
                answered = talk => {
                    if answered.is_some() {
                        r.store(true, Ordering::SeqCst);
                    }
                }
                _ = child.wait() => {}
                _ = stopping.changed() => {}
            }
            c.store(true, Ordering::SeqCst);
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = child.wait().await;
            let _ = drain.await;
        });
        Ok(Self { ready, closed, tail, stop, task: Some(task) })
    }
    /// The server answered `initialize`: it is signed in and working.
    pub fn ready(&self) -> bool {
        self.ready.load(Ordering::SeqCst)
    }
    /// The process has ended (or was stopped).
    pub fn exited(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
    /// What the server's stderr says about signing in so far.
    pub fn sign_in(&self) -> Option<SignInNeed> {
        sign_in_need(&self.tail.lock().ok()?)
    }
    /// Ends the process and its group and waits for it.
    pub async fn shutdown(mut self) {
        let _ = self.stop.send(true);
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for SignInProbe {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

#[cfg(test)]
mod sign_in_tests {
    use super::sign_in_need;
    #[test]
    fn mcp_remote_authorization_line_gives_the_address() {
        let text = "[1] Using transport\nPlease authorize this client by visiting: https://kling.ai/oauth/authorize?state=abc&x=1\n";
        let need = sign_in_need(text).unwrap();
        assert_eq!(need.url.as_deref(), Some("https://kling.ai/oauth/authorize?state=abc&x=1"));
    }
    #[test]
    fn an_oauth_address_alone_and_a_phrase_alone_count() {
        assert!(sign_in_need("open https://id.example/oauth/authorize?c=1 now").unwrap().url.is_some());
        assert_eq!(sign_in_need("This instance is running the sign-in for this server").unwrap().url, None);
    }
    #[test]
    fn ordinary_output_and_unsafe_addresses_do_not_count() {
        assert!(sign_in_need("listening on https://example.com/docs\nready").is_none());
        assert!(sign_in_need("").is_none());
        let n = sign_in_need("Please authorize: http://x.test/oauth/authorize and https://bad_host/a").unwrap();
        assert_eq!(n.url, None);
    }
}
