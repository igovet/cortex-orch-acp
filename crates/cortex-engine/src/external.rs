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
        let process = tokio::spawn(async move {
            let drain = tokio::spawn(async move {
                let mut b = [0u8; 8192];
                while let Ok(n) = stderr.read(&mut b).await {
                    if n == 0 {
                        break;
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
        })
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
