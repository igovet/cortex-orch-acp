//! Private Linux MCP broker. Python oracle: engine/broker.py:36-222,382-579.
//! Same-user/ancestry checks are not hostile-code isolation.
use crate::{wire, Error, FutureResult, Result};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::fd::AsRawFd,
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{UnixListener, UnixStream},
    sync::watch,
    task::JoinHandle,
};

pub trait Handler: Send + Sync + 'static {
    fn handle<'a>(&'a self, request: Value) -> FutureResult<'a, Option<Value>>;
}
impl Handler for crate::gateway::Gateway {
    fn handle<'a>(&'a self, request: Value) -> FutureResult<'a, Option<Value>> {
        Box::pin(async move { Ok(self.handle(request).await) })
    }
}
/// Only brokers construct ACP entries. No channel secret or relay token is serialized.
#[derive(Clone, Serialize)]
pub struct RelayEntry {
    name: String,
    command: PathBuf,
    args: Vec<String>,
    env: Vec<Value>,
}
impl RelayEntry {
    /// A service-owned stdio server that needs no broker, channel or secret (the
    /// side chat's read-only project files): an absolute existing executable,
    /// plain arguments and no environment.
    pub fn stdio(name: &str, command: &Path, args: Vec<String>) -> Result<Self> {
        if name.is_empty()
            || !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || matches!(name, "cortex" | "worker" | "coordinator")
            || !command.is_absolute()
            || !command.is_file()
            || args.iter().any(|a| a.contains('\0'))
        {
            return Err(Error::Invalid);
        }
        Ok(Self { name: name.into(), command: command.into(), args, env: vec![] })
    }
}

pub struct Broker {
    endpoint: PathBuf,
    directory: PathBuf,
    // Pins the parent used by the short /proc endpoint for the broker lifetime.
    _parent: fs::File,
    root: Arc<Mutex<Option<(u32, u64)>>>,
    stop: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
    receipt: Arc<Mutex<ToolsReceipt>>,
}
fn identity(pid: u32) -> Result<(u32, u64)> {
    let text = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let tail = text.rsplit_once(')').ok_or(Error::Forbidden)?.1;
    let fields: Vec<_> = tail.split_whitespace().collect();
    Ok((
        fields
            .get(1)
            .ok_or(Error::Forbidden)?
            .parse()
            .map_err(|_| Error::Forbidden)?,
        fields
            .get(19)
            .ok_or(Error::Forbidden)?
            .parse()
            .map_err(|_| Error::Forbidden)?,
    ))
}
fn descendant(mut pid: u32, root: (u32, u64)) -> bool {
    if !identity(root.0).is_ok_and(|(_, start)| start == root.1) {
        return false;
    }
    for _ in 0..64 {
        if pid == root.0 {
            return true;
        }
        if pid <= 1 {
            return false;
        }
        match identity(pid) {
            Ok((parent, _)) => pid = parent,
            Err(_) => return false,
        }
    }
    false
}
/// Metadata only. Unobserved/closed/incomplete is never readiness evidence.
#[derive(Clone, Debug, Serialize)]
pub struct ToolsReceipt {
    pub generation: u64,
    pub state: String,
    pub tool_count: Option<usize>,
    pub tool_names: Vec<String>,
    pub schema_digests: BTreeMap<String, String>,
    pub missing_required_tools: Vec<String>,
    pub worker_result_submit_present: Option<bool>,
}
impl Default for ToolsReceipt {
    fn default() -> Self {
        Self {
            generation: 0,
            state: "not_observed".into(),
            tool_count: None,
            tool_names: vec![],
            schema_digests: BTreeMap::new(),
            missing_required_tools: vec!["create_draft".into(), "write_report".into()],
            worker_result_submit_present: None,
        }
    }
}
impl ToolsReceipt {
    fn observe(generation: u64, reply: &Value) -> Self {
        let mut r = Self {
            generation,
            ..Self::default()
        };
        let Some(tools) = reply["result"]["tools"].as_array() else {
            r.state = "incomplete".into();
            return r;
        };
        let mut malformed = reply["result"]
            .get("nextCursor")
            .is_some_and(|v| !v.is_null());
        let mut names = BTreeSet::new();
        let mut valid = BTreeSet::new();
        for t in tools {
            let Some(name) = t["name"].as_str() else {
                malformed = true;
                continue;
            };
            let okay = !name.is_empty()
                && names.insert(name.to_owned())
                && t["inputSchema"]["type"] == "object";
            if okay {
                valid.insert(name.to_owned());
            } else {
                malformed = true;
            }
            r.schema_digests.insert(
                name.into(),
                format!(
                    "{:x}",
                    Sha256::digest(serde_json::to_vec(&t["inputSchema"]).unwrap())
                ),
            );
        }
        r.tool_names = names.into_iter().collect();
        r.tool_count = Some(tools.len());
        r.missing_required_tools.retain(|n| !valid.contains(n));
        r.worker_result_submit_present = Some(valid.contains("result_submit"));
        r.state = if malformed || !r.missing_required_tools.is_empty() {
            "incomplete"
        } else {
            "complete"
        }
        .into();
        r
    }
}
struct ConnectionGuard {
    receipt: Arc<Mutex<ToolsReceipt>>,
    generation: u64,
}
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        let mut r = self.receipt.lock().unwrap();
        if r.generation == self.generation {
            r.state = "closed".into();
        }
    }
}
fn equal(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        diff |= (a.get(i).copied().unwrap_or(0) ^ b.get(i).copied().unwrap_or(0)) as usize
    }
    diff == 0
}
fn private_dir(parent: &Path) -> Result<(PathBuf, String)> {
    if !parent.is_absolute() {
        return Err(Error::Invalid);
    }
    let mut random = [0u8; 32];
    fs::File::open("/dev/urandom")?.read_exact(&mut random)?;
    let token = random
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect::<String>();
    let dir = parent.join(format!("cortex-mcp-{}", &token[..24]));
    std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
    Ok((dir, token))
}
use std::os::unix::fs::DirBuilderExt;
impl Broker {
    pub fn new(parent: &Path, handler: Arc<dyn Handler>) -> Result<Self> {
        let (directory, token) = private_dir(parent)?;
        let setup = (|| -> Result<(UnixListener, fs::File, PathBuf)> {
            let parent = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_PATH | libc::O_DIRECTORY | libc::O_CLOEXEC)
                .open(parent)?;
            // sockaddr_un has only 108 bytes, regardless of PATH_MAX. Keep the
            // socket in its private directory but address it through a pinned
            // parent fd, so long project/TMPDIR paths do not prevent startup.
            let endpoint = PathBuf::from(format!(
                "/proc/{}/fd/{}",
                std::process::id(),
                parent.as_raw_fd()
            ))
            .join(directory.file_name().ok_or(Error::Invalid)?)
            .join("broker.sock");
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(directory.join("relay.token"))?;
            f.write_all(token.as_bytes())?;
            let listener = UnixListener::bind(&endpoint)?;
            fs::set_permissions(&endpoint, fs::Permissions::from_mode(0o600))?;
            Ok((listener, parent, endpoint))
        })();
        let (listener, parent, endpoint) = match setup {
            Ok(l) => l,
            Err(e) => {
                let _ = fs::remove_dir_all(&directory);
                return Err(e);
            }
        };
        let root = Arc::new(Mutex::new(None));
        let (stop, mut stopped) = watch::channel(false);
        let roots = root.clone();
        let receipt = Arc::new(Mutex::new(ToolsReceipt::default()));
        let observation = receipt.clone();
        let task = tokio::spawn(async move {
            let mut clients = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    _=stopped.changed()=>break,
                    Some(_)=clients.join_next(),if !clients.is_empty()=>{},
                    accepted=listener.accept()=>{
                        let Ok((mut stream,_))=accepted else{break};
                        if clients.len()>=16{continue}
                        let handler=handler.clone();let roots=roots.clone();let token=token.clone();let observation=observation.clone();
                        clients.spawn(async move{
                            let peer=stream.peer_cred().map_err(|_|Error::Forbidden)?;
                            let root=(*roots.lock().unwrap()).ok_or(Error::Forbidden)?;
                            if peer.uid()!=unsafe{libc::geteuid()} || !peer.pid().is_some_and(|p|p>1&&descendant(p as u32,root)){return Err(Error::Forbidden)}
                            let auth=async{
                                let mut bytes=Vec::new();
                                loop{let b=stream.read_u8().await?;if b==b'\n'{break}if bytes.len()>=128{return Err(Error::Forbidden)}bytes.push(b)}
                                if !equal(&bytes,token.as_bytes()){return Err(Error::Forbidden)}
                                Ok(())
                            };
                            tokio::time::timeout(Duration::from_secs(5),auth).await.map_err(|_|Error::Unknown)??;
                            let generation={let mut r=observation.lock().unwrap();let g=r.generation+1;*r=ToolsReceipt{generation:g,..ToolsReceipt::default()};g};
                            let _guard=ConnectionGuard{receipt:observation.clone(),generation};
                            let (r,mut w)=stream.into_split();let mut r=tokio::io::BufReader::new(r);
                            while let Some(request)=wire::read_frame(&mut r).await?{
                                if !identity(root.0).is_ok_and(|(_,s)|s==root.1){return Err(Error::Forbidden)}
                                let listing=request["method"]=="tools/list";
                                if let Some(reply)=handler.handle(request).await?{
                                    wire::write_frame(&mut w,&reply).await?;
                                    if listing{let mut r=observation.lock().unwrap();if r.generation==generation&&(r.state=="not_observed"||reply.get("error").is_some()){*r=ToolsReceipt::observe(generation,&reply);}}
                                }
                            }
                            Ok::<(),Error>(())
                        });
                    }
                }
            }
            clients.abort_all();
            while clients.join_next().await.is_some() {}
        });
        Ok(Self {
            endpoint,
            directory,
            _parent: parent,
            root,
            stop,
            task: Some(task),
            receipt,
        })
    }
    /// Bind once, immediately after launching the owned engine; never accept a model PID.
    pub fn bind_engine(&self, pid: u32) -> Result<()> {
        if pid <= 1 {
            return Err(Error::Invalid);
        }
        let (_, start) = identity(pid)?;
        let mut root = self.root.lock().unwrap();
        if root.is_some() {
            return Err(Error::Busy);
        }
        *root = Some((pid, start));
        Ok(())
    }
    pub fn entry(&self, name: &str, relay: &Path) -> Result<RelayEntry> {
        if name.is_empty()
            || matches!(name, "worker" | "coordinator")
            || !relay.is_absolute()
            || !relay.is_file()
        {
            return Err(Error::Invalid);
        }
        Ok(RelayEntry {
            name: name.into(),
            command: relay.into(),
            args: vec![self.endpoint.to_string_lossy().into_owned()],
            env: vec![],
        })
    }
    /// Codex reads its base instructions from a session-private file for this broker's lifetime.
    pub fn write_model_instructions(&self, text: &str) -> Result<PathBuf> {
        let path = self.directory.join("model-instructions.md");
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)?;
        file.write_all(text.as_bytes())?;
        Ok(path)
    }
    pub fn endpoint(&self) -> &Path {
        &self.endpoint
    }
    /// The broker's private directory: its socket, relay token and the
    /// session's model instructions. A confined engine sees it, read-only, at
    /// [`VIEW_DIRECTORY`]`/<name>`.
    pub fn directory(&self) -> &Path {
        &self.directory
    }
    pub fn tools_receipt(&self) -> ToolsReceipt {
        self.receipt.lock().unwrap().clone()
    }
    pub async fn shutdown(mut self) -> Result<()> {
        let _ = self.stop.send(true);
        if let Some(task) = self.task.take() {
            task.await.map_err(|_| Error::Io)?
        }
        Ok(())
    }
}
impl Drop for Broker {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        if let Some(task) = self.task.take() {
            task.abort()
        }
        let _ = fs::remove_dir_all(&self.directory);
    }
}
/// Where a confined engine's filesystem view shows each of its session's broker
/// directories (read-only, by their own name). Only such a view creates it: the
/// host's `/run` is root-owned and has no such directory.
pub const VIEW_DIRECTORY: &str = "/run/cortex/mcp";
/// A relay endpoint names the broker through the service's pinned
/// `/proc/<pid>/fd/<fd>/cortex-mcp-<token>/broker.sock` path. Inside a confined
/// engine (its own PID namespace and procfs) that path names nothing, or another
/// process: the same broker directory is reached at [`VIEW_DIRECTORY`] instead.
fn view_endpoint(endpoint: &Path) -> Option<PathBuf> {
    use std::path::Component;
    let parts: Vec<Component> = endpoint.components().collect();
    let [Component::RootDir, Component::Normal(proc_), Component::Normal(pid), Component::Normal(fd), Component::Normal(number), Component::Normal(name), Component::Normal(socket)] =
        parts.as_slice()
    else {
        return None;
    };
    let digits = |s: &std::ffi::OsStr| {
        !s.is_empty() && s.as_encoded_bytes().iter().all(u8::is_ascii_digit)
    };
    if *proc_ != "proc" || *fd != "fd" || *socket != "broker.sock" || !digits(pid) || !digits(number)
    {
        return None;
    }
    let token = name.to_str()?.strip_prefix("cortex-mcp-")?;
    if token.len() != 24 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let directory = Path::new(VIEW_DIRECTORY).join(name);
    fs::symlink_metadata(&directory)
        .is_ok_and(|m| m.is_dir())
        .then(|| directory.join("broker.sock"))
}
/// Secret-free executable adapter: reads the owner-only, no-follow token file.
pub async fn relay(endpoint: &Path) -> Result<()> {
    if !endpoint.is_absolute() {
        return Err(Error::Invalid);
    }
    let mapped = view_endpoint(endpoint);
    let endpoint = mapped.as_deref().unwrap_or(endpoint);
    let dir = endpoint.parent().ok_or(Error::Invalid)?;
    let info = fs::symlink_metadata(dir)?;
    if !info.is_dir() || info.uid() != unsafe { libc::geteuid() } || info.mode() & 0o077 != 0 {
        return Err(Error::Forbidden);
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(dir.join("relay.token"))?;
    let info = file.metadata()?;
    if !info.is_file()
        || info.uid() != unsafe { libc::geteuid() }
        || info.mode() & 0o077 != 0
        || info.len() > 128
    {
        return Err(Error::Forbidden);
    }
    let mut token = String::new();
    Read::take(&mut file, 129).read_to_string(&mut token)?;
    if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::Forbidden);
    }
    let mut stream = UnixStream::connect(endpoint).await?;
    stream.write_all(format!("{token}\n").as_bytes()).await?;
    let (mut r, mut w) = stream.into_split();
    let mut stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    tokio::select! {
        result=tokio::io::copy(&mut stdin,&mut w)=>{result?;w.shutdown().await?;tokio::io::copy(&mut r,&mut stdout).await?;},
        result=tokio::io::copy(&mut r,&mut stdout)=>{result?;},
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_a_shown_broker_directory_replaces_the_service_endpoint() {
        let endpoint =
            Path::new("/proc/1234/fd/7/cortex-mcp-0123456789abcdef01234567/broker.sock");
        // The host has no view directory: the service path stays in use.
        if !Path::new(VIEW_DIRECTORY).exists() {
            assert_eq!(view_endpoint(endpoint), None);
        }
        for other in [
            "/proc/1234/fd/7/cortex-mcp-0123/broker.sock",
            "/proc/self/fd/7/cortex-mcp-0123456789abcdef01234567/broker.sock",
            "/proc/1234/fd/7/cortex-mcp-0123456789abcdef01234567/other.sock",
            "/tmp/cortex-mcp-0123456789abcdef01234567/broker.sock",
            "/proc/1234/fd/7/../cortex-mcp-0123456789abcdef01234567/broker.sock",
        ] {
            assert_eq!(view_endpoint(Path::new(other)), None, "{other}");
        }
    }
}
