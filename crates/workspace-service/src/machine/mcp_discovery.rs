//! Owner pre-save observation only: initialize/tools-list, never tools/call or persistence.
//! One native operation, with the retained nested result envelope and refusal phases.
#[cfg(test)]
#[path = "mcp_discovery_tests.rs"]
mod tests;
use super::{mcp_redact, native::*, project_config, workers::truthy};
use crate::config_read_model::mcp as model;
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
    time::Instant,
};
type Fallible<T> = std::result::Result<T, &'static str>;
struct Spec {
    name: String,
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    reads: Vec<PathBuf>,
    writes: Vec<PathBuf>,
    runtime_env: Vec<String>,
    network: bool,
    env_order: Vec<String>,
    /// The owner's explicit opt-out: no Landlock/seccomp and no grant checks.
    uncontained: bool,
}
fn get<'a>(v: &'a Value, key: &str) -> Fallible<&'a Value> {
    if !v.is_object() {
        return Err("AttributeError");
    }
    Ok(v.get(key).unwrap_or(&Value::Null))
}
fn required<'a>(v: &'a Value, key: &str) -> Fallible<&'a Value> {
    if !v.is_object() {
        return Err("TypeError");
    }
    v.get(key).ok_or("KeyError")
}
fn text(v: &Value) -> Fallible<String> {
    v.as_str().map(str::to_owned).ok_or("PortableError")
}
fn strings(v: &Value) -> Fallible<Vec<String>> {
    v.as_array()
        .ok_or("PortableError")?
        .iter()
        .map(|v| {
            text(v).and_then(|s| {
                if s.contains('\0') {
                    Err("PortableError")
                } else {
                    Ok(s)
                }
            })
        })
        .collect()
}
fn configuration(server: &Value, config: &crate::Config) -> Fallible<Spec> {
    let folder = project_config::owner_folder(config).map_err(|_| "PortableError")?;
    let saved = project_config::read_plain(&folder.join("mcp-servers.json"), true)
        .map_err(|_| "PortableError")?
        .map(|b| serde_json::from_slice::<Value>(&b).map_err(|_| "PortableError"))
        .transpose()?
        .unwrap_or(json!({}));
    let name = required(server, "name")?;
    let registry = if truthy(&saved) {
        get(&saved, "servers")?.clone()
    } else {
        json!({})
    };
    let record = if truthy(&registry) {
        get(&registry, name.as_str().ok_or("TypeError")?)?.clone()
    } else {
        json!({})
    };
    let old = if truthy(&record) {
        get(&record, "env")?.clone()
    } else {
        json!({})
    };
    let mut old_env = BTreeMap::new();
    if let Some(a) = old.as_array() {
        for item in a {
            old_env.insert(
                text(required(item, "name")?)?,
                text(required(item, "value")?)?,
            );
        }
    } else if let Some(m) = old.as_object() {
        for (k, v) in m {
            old_env.insert(k.clone(), text(v)?);
        }
    } else if truthy(&old) {
        return Err("TypeError");
    }
    let mut env = BTreeMap::new();
    let mut env_order = vec![];
    let e = get(server, "env")?;
    if truthy(e) {
        for item in e.as_array().ok_or("AttributeError")? {
            if get(item, "action")? == "remove" {
                continue;
            }
            let key = text(required(item, "name")?)?;
            let value = if get(item, "action")? == "keep" && truthy(get(item, "stored")?) {
                old_env.get(&key).ok_or("KeyError")?.clone()
            } else {
                text(item.get("value").unwrap_or(&json!("")))?
            };
            if key.is_empty() || key.contains('\0') || value.contains('\0') {
                return Err("PortableError");
            }
            if !env.contains_key(&key) {
                env_order.push(key.clone());
            }
            env.insert(key, value);
        }
    }
    // The editor's value, else the saved entry's, else the saved registry default.
    let uncontained = match server.get("containment").filter(|v| !v.is_null()) {
        Some(v) if model::containment_valid(v) => v == "none",
        Some(_) => return Err("PortableError"),
        None => model::uncontained(
            if record.is_object() {
                &record
            } else {
                &Value::Null
            },
            saved.get("containment").unwrap_or(&Value::Null),
        ),
    };
    let command = text(required(server, "command")?)?;
    let name = text(name)?;
    if name.is_empty() || name == "cortex" || command.is_empty() || command.contains('\0') {
        return Err("PortableError");
    }
    let args = strings(server.get("args").unwrap_or(&json!([])))?;
    let paths = |key| -> Fallible<Vec<PathBuf>> {
        strings(server.get(key).unwrap_or(&json!([])))?
            .into_iter()
            .map(|s| {
                let p = PathBuf::from(s);
                if !p.is_absolute() {
                    Err("PortableError")
                } else {
                    Ok(p)
                }
            })
            .collect()
    };
    let reads = paths("readPaths")?;
    let writes = paths("writePaths")?;
    let network = server
        .get("network")
        .unwrap_or(&Value::Bool(false))
        .as_bool()
        .ok_or("PortableError")?;
    let runtime_env = strings(server.get("runtimeDirEnv").unwrap_or(&json!([])))?;
    let re = regex::Regex::new(r"^[A-Z_][A-Z0-9_]*$").unwrap();
    let mut seen = std::collections::BTreeSet::new();
    if runtime_env.iter().any(|s| {
        !re.is_match(s)
            || ["PATH", "HOME", "TMPDIR", "XDG_RUNTIME_DIR"].contains(&s.as_str())
            || !seen.insert(s)
    }) {
        return Err("PortableError");
    }
    Ok(Spec {
        name,
        command,
        args,
        env,
        reads,
        writes,
        runtime_env,
        network,
        env_order,
        uncontained,
    })
}
fn unsupported(phase: &str, error: &str) -> Value {
    json!({"ok":false,"code":"not_supported","message":"Could not list tools in the MCP sandbox. Check the executable, grants and environment, then retry.","manual":true,"diagnostic":format!("{phase}:{error}")})
}
fn timeout_reply() -> Value {
    json!({"ok":false,"code":"timeout","message":"The server did not list its tools before the deadline. Its processes were stopped. Retry or edit the connection."})
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> std::io::Result<Self> {
        let p = PathBuf::from("/tmp").join(format!(
            "cw-mcp-{}",
            crate::native_effects::identity().map_err(|_| std::io::Error::other("identity"))?
        ));
        fs::DirBuilder::new().mode(0o700).create(&p)?;
        Ok(Self(p))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
struct Process {
    child: Child,
    pid: i32,
}
impl Drop for Process {
    fn drop(&mut self) {
        // The process group includes descendants even if the server closes stdio.
        if self.pid > 0 {
            unsafe {
                libc::kill(-self.pid, libc::SIGKILL);
            }
        }
        let _ = self.child.start_kill();
    }
}
fn grant(path: &Path, root: &Path) -> bool {
    if !path.is_absolute()
        || !path.exists()
        || path.starts_with(root)
        || root.starts_with(path)
        || path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        || path.ancestors().any(|p| {
            p.symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
        })
    {
        return false;
    }
    let private = [
        ".git",
        ".cortex",
        ".cortex-dev",
        ".codex",
        ".claude",
        ".ssh",
        ".gnupg",
        ".cortex-dev-owner.json",
        ".cortex-workspace-dev.json",
    ];
    if path
        .components()
        .any(|c| private.iter().any(|s| c.as_os_str() == *s))
    {
        return false;
    }
    // Refuse protected descendants, links and unbounded grant trees rather than
    // granting their parent recursively to Landlock.
    let mut pending = vec![path.to_path_buf()];
    let mut count = 0;
    while let Some(p) = pending.pop() {
        count += 1;
        if count > 50000 {
            return false;
        }
        let Ok(m) = p.symlink_metadata() else {
            return false;
        };
        if m.file_type().is_symlink()
            || p.file_name()
                .is_some_and(|n| private.iter().any(|s| n == *s))
        {
            return false;
        }
        if m.is_dir() {
            let Ok(entries) = fs::read_dir(&p) else {
                return false;
            };
            for e in entries {
                let Ok(e) = e else {
                    return false;
                };
                pending.push(e.path());
            }
        } else if !m.is_file() {
            return false;
        }
    }
    true
}
struct Peer<'a> {
    process: &'a mut Process,
    buffer: Vec<u8>,
}
impl Peer<'_> {
    async fn send(&mut self, method: &str, id: Option<u64>, params: Option<Value>) -> Fallible<()> {
        let mut m = json!({"jsonrpc":"2.0","method":method});
        if let Some(id) = id {
            m["id"] = json!(id);
        }
        if let Some(p) = params {
            m["params"] = p;
        }
        let mut b = serde_json::to_vec(&m).map_err(|_| "ValueError")?;
        b.push(b'\n');
        self.process
            .child
            .stdin
            .as_mut()
            .ok_or("ValueError")?
            .write_all(&b)
            .await
            .map_err(|_| "BrokenPipeError")
    }
    async fn receive(&mut self, id: u64) -> Fallible<Value> {
        for _ in 0..32 {
            while !self.buffer.contains(&b'\n') {
                let mut chunk = [0; 65536];
                let n = self
                    .process
                    .child
                    .stdout
                    .as_mut()
                    .ok_or("ValueError")?
                    .read(&mut chunk)
                    .await
                    .map_err(|_| "OSError")?;
                if n == 0 {
                    return Err("ValueError");
                }
                self.buffer.extend_from_slice(&chunk[..n]);
                if self.buffer.len() > 262144 {
                    return Err("ValueError");
                }
            }
            let end = self.buffer.iter().position(|b| *b == b'\n').unwrap();
            let row = self.buffer.drain(..=end).collect::<Vec<_>>();
            let m: Value = serde_json::from_slice(&row).map_err(|_| "JSONDecodeError")?;
            let o = m.as_object().ok_or("TypeError")?;
            let Some(received) = o.get("id") else {
                continue;
            };
            if !(received.as_u64() == Some(id)
                || received.as_f64() == Some(id as f64)
                || id == 1 && *received == Value::Bool(true))
                || o.contains_key("error")
            {
                return Err("ValueError");
            }
            return o.get("result").cloned().ok_or("KeyError");
        }
        Err("ValueError")
    }
    async fn listing(&mut self) -> Fallible<Vec<Value>> {
        self.send("initialize",Some(1),Some(json!({"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"cortex-workspace","version":"1.0.6"}}))).await?;
        let init = self.receive(1).await?;
        if !["2024-11-05", "2025-03-26", "2025-06-18", "2025-11-25"]
            .contains(&get(&init, "protocolVersion")?.as_str().unwrap_or(""))
        {
            return Err("ValueError");
        }
        self.send("notifications/initialized", None, None).await?;
        let mut tools = vec![];
        let mut cursor = Value::Null;
        let mut seen = std::collections::BTreeSet::new();
        for page in 0..20 {
            self.send(
                "tools/list",
                Some(2 + page),
                Some(if truthy(&cursor) {
                    json!({"cursor":cursor})
                } else {
                    json!({})
                }),
            )
            .await?;
            let reply = self.receive(2 + page).await?;
            let batch = get(&reply, "tools")?.as_array().ok_or("ValueError")?;
            if tools.len() + batch.len() > 1000 {
                return Err("ValueError");
            }
            tools.extend(batch.iter().cloned());
            cursor = get(&reply, "nextCursor")?.clone();
            if !truthy(&cursor) {
                return Ok(tools);
            }
            let c = cursor.as_str().ok_or("ValueError")?;
            if !seen.insert(c.to_owned()) {
                return Err("ValueError");
            }
        }
        Err("ValueError")
    }
}
fn project(spec: &Spec, tools: Vec<Value>) -> Fallible<Value> {
    let name = regex::Regex::new(r"^[A-Za-z0-9_.:-]{1,128}$").unwrap();
    let mut rows = vec![];
    for tool in tools {
        let raw = get(&tool, "name")?;
        let Some(n) = raw.as_str().filter(|s| name.is_match(s)) else {
            return Err("ValueError");
        };
        let raw = get(&tool, "description")?;
        let mut description = if truthy(raw) {
            mcp_redact::pystr(raw)
        } else {
            String::new()
        }
        .chars()
        .take(1000)
        .collect::<String>();
        let mut n = n.to_owned();
        for value in spec
            .env_order
            .iter()
            .map(|k| &spec.env[k])
            .filter(|s| !s.is_empty())
        {
            n = n.replace(value, "[redacted]");
            description = description.replace(value, "[redacted]");
        }
        let a = get(&tool, "annotations")?;
        let a = if truthy(a) { a.clone() } else { json!({}) };
        let hint = if get(&a, "readOnlyHint")? == true {
            json!("read")
        } else if get(&a, "destructiveHint")? == true {
            json!("destructive")
        } else {
            Value::Null
        };
        rows.push(json!({"name":n,"description":mcp_redact::scrub(&description),"hint":hint}));
    }
    Ok(
        json!({"ok":true,"observation":{"at":now_ms(),"name":spec.name,"version":"reported","tools":rows}}),
    )
}
pub(super) async fn discover(server: &Value, config: &crate::Config) -> Value {
    discover_with_timeout(server, config, Duration::from_secs(15)).await
}
async fn discover_with_timeout(server: &Value, config: &crate::Config, timeout: Duration) -> Value {
    let spec = match configuration(server, config) {
        Ok(s) => s,
        Err(e) => return unsupported("configuration", e),
    };
    let scratch = match Scratch::new() {
        Ok(s) => s,
        Err(_) => return unsupported("configuration", "OSError"),
    };
    let root = scratch.0.join("project");
    let runtime = scratch.0.join("runtime");
    if fs::create_dir(&root)
        .and_then(|_| fs::DirBuilder::new().mode(0o700).create(&runtime))
        .is_err()
    {
        return unsupported("configuration", "OSError");
    }
    // An uncontained server's grants are not enforced, so they are not checked
    // and no Landlock/seccomp policy (or its per-file walk) is prepared.
    let sandbox = if spec.uncontained {
        None
    } else {
        if spec
            .reads
            .iter()
            .chain(&spec.writes)
            .any(|p| !grant(p, &root))
        {
            return json!({"ok":false,"code":"path_refused","message":"A sandbox grant is private, missing or a symlink. Correct it before retrying."});
        }
        match crate::native_sandbox::Sandbox::external(
            &root,
            &runtime,
            &spec.reads,
            &spec.writes,
            spec.network,
        ) {
            Ok(s) => Some(s),
            Err(_) => return unsupported("sandbox", "OSError"),
        }
    };
    let home = std::env::var_os("HOME").unwrap_or_default();
    let path = if spec.uncontained {
        crate::native_mcp::owner_path(Path::new(&spec.command), Path::new(&home))
    } else {
        "/usr/bin:/bin".into()
    };
    let mut command = Command::new(&spec.command);
    command
        .args(&spec.args)
        .current_dir(&root)
        .env_clear()
        .env("PATH", path)
        .env("HOME", &home)
        .envs(&spec.env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if spec.uncontained {
        // As at session start: the owner's runtime directory, no private ones.
        if let Some(r) = std::env::var_os("XDG_RUNTIME_DIR").filter(|r| Path::new(r).is_absolute())
        {
            command.env("XDG_RUNTIME_DIR", r);
        }
    } else {
        command
            .env("TMPDIR", &runtime)
            .env("XDG_RUNTIME_DIR", &runtime);
        for k in &spec.runtime_env {
            command.env(k, &runtime);
        }
    }
    unsafe {
        command.pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            match &sandbox {
                Some(sandbox) => sandbox.enter(),
                None => Ok(()),
            }
        });
    }
    let deadline = Instant::now() + timeout;
    let child = match command.spawn() {
        Ok(c) => c,
        Err(e) => {
            return unsupported(
                "spawn",
                match e.kind() {
                    std::io::ErrorKind::NotFound => "FileNotFoundError",
                    std::io::ErrorKind::PermissionDenied => "PermissionError",
                    _ => "OSError",
                },
            )
        }
    };
    let pid = child.id().unwrap() as i32;
    let mut process = Process { child, pid };
    let result = tokio::time::timeout_at(
        deadline,
        Peer {
            process: &mut process,
            buffer: vec![],
        }
        .listing(),
    )
    .await;
    // Kill and reap before releasing grants/scratch or reporting any observation.
    unsafe {
        libc::kill(-process.pid, libc::SIGKILL);
    }
    let _ = process.child.wait().await;
    process.pid = 0;
    match result {
        Err(_) => timeout_reply(),
        Ok(Err(e)) => unsupported("listing", e),
        Ok(Ok(tools)) => project(&spec, tools).unwrap_or_else(|e| unsupported("projection", e)),
    }
}
