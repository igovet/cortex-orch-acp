//! Prompt-free ACP observation. Transport errors are outcomes, never permission to replay.
use super::*;
use cortex_engine::wire;
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::{
    io::{AsyncReadExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
};
#[path = "config_catalog_codex.rs"]
mod codex;
#[path = "config_catalog_install.rs"]
mod install;
#[path = "config_catalog_observation.rs"]
mod observation;
use crate::machine::config_action::opencode_engine as oc;
use crate::machine::mcp_redact as redaction;

// Unqualified package candidates retain Python before effects (`codex::retained` names
// why). Unresolved two-argument launches, including Codex packages Python certainly
// cannot resolve (`codex::unverified`), behave like direct launches in the Python oracle.
// OS-keyring observations always retain the provider; missing helper infrastructure
// does not establish ImportError.
pub(super) fn admitted(env: &RouteEnvironment, r: &Value) -> bool {
    let Some(a) = r["launch_command"].as_array() else {
        return false;
    };
    let engine = r["engine"].as_str().unwrap();
    if !matches!(engine, "codex-acp" | "claude-agent-acp" | "opencode")
        || a.is_empty()
        // OpenCode is the exact `(binary, "acp")` launch; its install is verified
        // at launch and a pre-probe rejection is a failed row instead.
        || engine == "opencode"
            && (a.len() != 2 || a[1] != oc::ACP || super::opencode::rejected(env, r))
        || engine != "opencode"
            && install::candidate(a).is_some()
            && if engine == "codex-acp" {
                codex::retained(env, r).is_some()
            } else {
                install::claude(env, r).is_none()
            }
        || a.iter().any(|v| v.as_str().unwrap().contains('\0'))
        || !(r["auth_mode"] == "native_account"
            && r["credential_ref"]["kind"] == "engine_native_dir"
            || r["auth_mode"] == "api_key" && r["credential_ref"]["kind"] == "secret_file")
    {
        return false;
    }
    for root in [env.config.join("cortex"), env.state.join("cortex")] {
        if !root
            .parent()
            .is_some_and(|p| p.is_dir() && p.canonicalize().is_ok_and(|real| real == p))
        {
            return false;
        }
        if root.exists() && private_directory(&root, false).is_err() {
            return false;
        }
        if root
            .ancestors()
            .any(|p| p != env.home && p.join(".git").exists())
        {
            return false;
        }
        if let Ok(Some(bytes)) = reader::read_plain(&root.join(".gitignore"), true) {
            if bytes != b"*\n" {
                return false;
            }
        } else if root.join(".gitignore").exists() {
            return false;
        }
    }
    // Noncanonical references are qualified failed attempts. Do not inspect their targets.
    let expected = credential_path(env, r);
    if r["credential_ref"]["path"].as_str() == expected.to_str() {
        if r["credential_ref"]["kind"] == "engine_native_dir" {
            if !expected.is_dir() || !expected.canonicalize().is_ok_and(|p| p == expected) {
                return false;
            }
        } else if let Some(parent) = expected.parent() {
            if parent.exists()
                && (!parent.canonicalize().is_ok_and(|p| p == parent)
                    || private_directory(parent, false).is_err())
            {
                return false;
            }
        }
    }
    Path::new(a[0].as_str().unwrap()).is_absolute()
}
/// Why a record that no native producer launches is a failed attempt, in the order
/// Python reaches its failure (`catalog.refresh.perform_refresh`).
pub(super) fn unlaunched_reason(env: &RouteEnvironment, r: &Value) -> &'static str {
    let command = r["launch_command"].as_array();
    if super::home_read_only(env, r) {
        "engine_home_read_only"
    } else if command.is_some_and(Vec::is_empty) {
        "engine_unreachable:IndexError"
    } else if command.is_some_and(|a| {
        a.iter()
            .any(|v| v.as_str().is_some_and(|s| s.contains('\0')))
    }) {
        "engine_unreachable:ValueError"
    } else if r["credential_ref"]["kind"] == "os_keyring"
        && command.is_some_and(|a| a[0].as_str().is_some_and(|s| s.starts_with('/')))
    {
        // The OS keyring is never opened here: no credential state is observed.
        "engine_unreachable:keyring_unobservable"
    } else if command.is_some_and(|a| {
        r["engine"] != "opencode"
            && install::candidate(a).is_some()
            && (r["engine"] == "codex-acp" && codex::retained(env, r).is_some()
                || r["engine"] == "claude-agent-acp" && install::claude(env, r).is_none())
    }) {
        // Python's install verifier decides this shape; no certain outcome is claimed.
        "engine_unreachable:native_install_unverified"
    } else {
        "engine_unreachable:PortableError"
    }
}

/// Whether a task-session launch of registry record `r` pins the native Codex
/// launcher. A launch that is not install-shaped starts unchanged, as
/// Python's `_resolve_launch_install` does for its fake-engine doubles.
pub(super) fn codex_install_shaped(r: &Value) -> bool {
    r["engine"] == "codex-acp"
        && r["launch_command"]
            .as_array()
            .is_some_and(|a| install::candidate(a).is_some())
}
/// `CODEX_PATH` for an install-shaped task-session launch, prepared when the
/// engine starts. A bundled binary that cannot be located refuses the launch,
/// never starts it without the launcher: Rust runs no engine jail that keeps
/// the owner's `config.toml` read-only.
pub(super) fn codex_session_launcher(
    env: &RouteEnvironment,
    r: &Value,
) -> std::result::Result<PathBuf, &'static str> {
    codex::session_launcher(env, r).map_err(|_| "codex_launcher_unavailable")
}
fn credential_path(env: &RouteEnvironment, r: &Value) -> PathBuf {
    if r["credential_ref"]["kind"] == "secret_file" {
        return env
            .config
            .join("cortex/secrets")
            .join(r["id"].as_str().unwrap());
    }
    if r["credential_ref"]["owned"] == false {
        return env.home.join(match r["engine"].as_str() {
            Some("codex-acp") => ".codex",
            Some("opencode") => ".local/share/opencode",
            _ => ".claude",
        });
    }
    engine_home(env, r)
}
fn engine_home(env: &RouteEnvironment, r: &Value) -> PathBuf {
    env.state
        .join("cortex/connections")
        .join(r["id"].as_str().unwrap())
        .join(match r["engine"].as_str() {
            Some("codex-acp") => "codex-home",
            Some("opencode") => "opencode",
            _ => "claude-config",
        })
}

pub(super) fn run(
    env: &RouteEnvironment,
    r: &Value,
    fence: impl FnOnce() -> Result<()>,
) -> Result<Value> {
    run_guarded(env, r, fence)
}

/// Retain a publication fence's guard until the immutable snapshot is durable.
pub(super) fn run_guarded<G>(
    env: &RouteEnvironment,
    r: &Value,
    fence: impl FnOnce() -> Result<G>,
) -> Result<Value> {
    // The machine lane already enters a Tokio runtime. Own and join a separate
    // observation thread rather than nesting block_on or detaching an engine.
    let observed = std::thread::scope(|scope| {
        scope
            .spawn(|| {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|_| "engine_unreachable:RuntimeError".to_owned())?;
                runtime.block_on(observe(env, r))
            })
            .join()
    })
    .map_err(|_| unknown())?;
    match observed {
        Err(reason) if reason == "machine_outcome_unknown" => Err(unknown()),
        Err(reason) if reason == "native_catalog_stderr_unqualified" =>
            Err(unavailable("The engine emitted unqualified diagnostic output. This connection published no snapshot; earlier connection effects may remain. Re-read Settings; no automatic replay is safe.")),
        Err(reason) => Ok(json!({"connection_id":r["id"],"success":false,"reason":reason})),
        Ok(parsed) => {
            if parsed.models.is_empty() {
                return Ok(json!({"connection_id":r["id"],"success":false,"reason":"no_model_options_advertised"}));
            }
            if !observation::unicode::safe_name(r["name"].as_str().unwrap()) {
                // `catalog.labels.assert_safe_connection_name` inside `build_catalog`.
                return Ok(json!({"connection_id":r["id"],"success":false,
                    "reason":"invalid_catalog:unsafe_connection_label"}));
            }
            let snapshot = match observation::snapshot(r, parsed) {
                Ok(snapshot) => snapshot,
                Err(error) if error.code.starts_with("invalid_catalog:") =>
                    return Ok(json!({"connection_id":r["id"],"success":false,"reason":error.code})),
                Err(error) => return Err(error),
            };
            let _guard = fence()?;
            publish(env, r["id"].as_str().unwrap(), &snapshot)?;
            Ok(json!({"connection_id":r["id"],"success":true,"reason":null}))
        }
    }
}

struct Peer {
    child: Child,
    input: ChildStdin,
    output: BufReader<ChildStdout>,
    next: u64,
    logs: Logs,
    eof: bool,
    deadline: tokio::time::Instant,
    stderr: tokio::task::JoinHandle<std::io::Result<u64>>,
}
impl Peer {
    async fn call(&mut self, method: &str, params: Value) -> std::result::Result<Value, String> {
        let id = self.next;
        self.next += 1;
        let call = async {
            wire::write_frame(
                &mut self.input,
                &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            )
            .await
            .map_err(|_| "engine_unreachable:EngineTransportClosed".to_owned())?;
            loop {
                let msg = match wire::read_frame(&mut self.output)
                    .await
                    .map_err(|_| "engine_unreachable:EngineTransportClosed".to_owned())?
                {
                    Some(msg) => msg,
                    None => {
                        self.eof = true;
                        self.logs.row(
                            "transport_eof",
                            json!({"alive":self.child.try_wait().ok().flatten().is_none()}),
                        );
                        return Err("engine_unreachable:EngineTransportClosed".into());
                    }
                };
                if msg.get("method").is_some() {
                    // Match Python's no-op callback: no filesystem, permission or tool replies.
                    continue;
                }
                if msg["id"] != id {
                    if msg["id"].is_u64() {
                        self.logs
                            .row("unknown_response_id", json!({"request_id":msg["id"]}));
                    }
                    continue;
                }
                if let Some(error) = msg.get("error") {
                    let auth = error["code"] == -32000
                        || error["code"].is_null()
                            && error["message"].as_str().is_some_and(|s| {
                                s.trim().eq_ignore_ascii_case("authentication required")
                            })
                        || error["message"].is_null()
                            && error["code"].as_str().is_some_and(|s| {
                                s.trim().eq_ignore_ascii_case("authentication required")
                            });
                    return Err(if auth {
                        "authorization_required"
                    } else {
                        "engine_unreachable:EngineProtocolError"
                    }
                    .into());
                }
                return Ok(msg["result"].clone());
            }
        };
        match tokio::time::timeout_at(self.deadline, call).await {
            Ok(result) => result,
            Err(_) => {
                self.logs.row(
                    "timeout_alive",
                    json!({"alive":self.child.try_wait().ok().flatten().is_none()}),
                );
                Err("engine_unreachable:EngineTimeout".into())
            }
        }
    }
    async fn close(mut self) -> std::result::Result<(), String> {
        let pid = self.child.id();
        drop(self.input);
        let status = match tokio::time::timeout(Duration::from_millis(100), self.child.wait()).await
        {
            Ok(status) => status,
            Err(_) => {
                if let Some(pid) = pid {
                    unsafe {
                        libc::kill(-(pid as i32), libc::SIGTERM);
                    }
                }
                match tokio::time::timeout(Duration::from_secs(5), self.child.wait()).await {
                    Ok(status) => status,
                    Err(_) => {
                        if let Some(pid) = pid {
                            unsafe {
                                libc::kill(-(pid as i32), libc::SIGKILL);
                            }
                        }
                        tokio::time::timeout(Duration::from_secs(5), self.child.wait())
                            .await
                            .map_err(|_| "machine_outcome_unknown".to_owned())?
                    }
                }
            }
        };
        if let Ok(status) = status {
            use std::os::unix::process::ExitStatusExt;
            self.logs.row(
                "process_exited",
                json!({"exit_code":status.code().unwrap_or_else(|| -status.signal().unwrap_or(9))}),
            );
        } else {
            return Err("machine_outcome_unknown".into());
        }
        if !self.eof {
            let eof = tokio::time::timeout(Duration::from_secs(1), async {
                loop {
                    match wire::read_frame(&mut self.output).await {
                        Ok(None) => return true,
                        Ok(Some(_)) => continue,
                        Err(_) => return false,
                    }
                }
            })
            .await
            .unwrap_or(false);
            if !eof {
                return Err("machine_outcome_unknown".into());
            }
            self.logs.row("transport_eof", json!({"alive":false}));
        }
        match tokio::time::timeout(Duration::from_secs(1), self.stderr).await {
            Ok(Ok(Ok(0))) => Ok(()),
            Ok(Ok(Ok(_))) => Err("native_catalog_stderr_unqualified".into()),
            _ => Err("machine_outcome_unknown".into()),
        }
    }
}
async fn observe(
    env: &RouteEnvironment,
    r: &Value,
) -> std::result::Result<observation::Observation, String> {
    observe_with(env, r, Duration::from_secs(20)).await
}
/// Spawn the engine process and its stderr drain: the observation's one `Peer`.
fn start_peer(
    mut c: Command,
    logs: Logs,
    secret: Option<String>,
    budget: Duration,
) -> std::result::Result<Peer, String> {
    let mut child = c.spawn().map_err(|e| {
        match e.kind() {
            std::io::ErrorKind::NotFound => "engine_unreachable:FileNotFoundError",
            std::io::ErrorKind::PermissionDenied => "engine_unreachable:PermissionError",
            _ => "engine_unreachable:OSError",
        }
        .to_owned()
    })?;
    let input = child.stdin.take().unwrap();
    let output = BufReader::new(child.stdout.take().unwrap());
    let mut stderr = child.stderr.take().unwrap();
    let stderr_logs = logs.clone();
    let stderr = tokio::spawn(async move {
        let mut bytes = [0; 8192];
        let mut line = Vec::new();
        let mut overflow = false;
        loop {
            let n = stderr.read(&mut bytes).await?;
            if n == 0 {
                if !line.is_empty() && !overflow {
                    stderr_logs.stderr_line(&String::from_utf8_lossy(&line), secret.as_deref())?;
                }
                // An over-bound diagnostic is not parity-qualified and cannot be replayed.
                return Ok::<_, std::io::Error>(u64::from(overflow));
            }
            for &b in &bytes[..n] {
                if line.len() < 64 * 1024 {
                    line.push(b);
                } else {
                    overflow = true;
                }
                if b == b'\n' {
                    if !overflow {
                        stderr_logs
                            .stderr_line(&String::from_utf8_lossy(&line), secret.as_deref())?;
                    }
                    line.clear();
                }
            }
        }
    });
    Ok(Peer {
        child,
        input,
        output,
        next: 1,
        logs,
        eof: false,
        stderr,
        deadline: tokio::time::Instant::now() + budget,
    })
}
#[cfg(test)]
pub(crate) async fn fixture_catalogue(
    env: &RouteEnvironment,
    record: &Value,
) -> std::result::Result<(), String> {
    let observed = observe_with(env, record, Duration::from_secs(25)).await?;
    if observed.models.is_empty() {
        return Err("fixture_catalogue_has_no_models".into());
    }
    Ok(())
}

async fn observe_with(
    env: &RouteEnvironment,
    r: &Value,
    budget: Duration,
) -> std::result::Result<observation::Observation, String> {
    if r["engine"] == "opencode" {
        return observe_opencode(env, r, budget).await;
    }
    let pinned = if install::candidate(r["launch_command"].as_array().unwrap()).is_some() {
        if r["engine"] == "claude-agent-acp" {
            Some(install::claude(env, r).ok_or_else(|| "machine_outcome_unknown".to_owned())?)
        } else {
            None
        }
    } else {
        None
    };
    // Re-classified at launch, as Python resolves its install per refresh.
    let codex = if r["engine"] == "codex-acp"
        && install::candidate(r["launch_command"].as_array().unwrap()).is_some()
        && !codex::unverified(r)
    {
        Some(codex::resolve(env, r).ok_or_else(|| "machine_outcome_unknown".to_owned())?)
    } else {
        None
    };
    let temp = Scratch::new().map_err(|_| "engine_unreachable:OSError".to_owned())?;
    let command = r["launch_command"].as_array().unwrap();
    let logs = Logs::new(env).map_err(|_| "engine_unreachable:PortableError".to_owned())?;
    let artifacts = codex.as_ref().map(|p| p.prepare(env, r)).transpose()?;
    super::ensure_ignore(&env.config.join("cortex"))
        .map_err(|_| "engine_unreachable:PortableError".to_owned())?;
    let expected = credential_path(env, r);
    if r["credential_ref"]["path"].as_str() != expected.to_str() {
        return Err("engine_unreachable:PortableError".into());
    }
    let secret = if r["auth_mode"] == "api_key" {
        let bytes = reader::read_plain(&expected, true)
            .map_err(|_| "engine_unreachable:PortableError".to_owned())?
            .ok_or_else(|| "engine_unreachable:PortableError".to_owned())?;
        let text = String::from_utf8(bytes)
            .map_err(|_| "engine_unreachable:UnicodeDecodeError".to_owned())?;
        Some(
            text.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
                .to_owned(),
        )
    } else {
        None
    };
    let home = if r["credential_ref"]["kind"] == "engine_native_dir" {
        expected
    } else {
        engine_home(env, r)
    };
    let mut args: Vec<String> = command[1..]
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    if let Some((entry, _)) = &artifacts {
        args[0] = entry
            .to_str()
            .ok_or_else(|| "machine_outcome_unknown".to_owned())?
            .to_owned();
    }
    use sha2::{Digest, Sha256};
    let joined = std::iter::once(command[0].as_str().unwrap())
        .chain(args.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("\u{1f}");
    logs.row(
        "starting",
        json!({"command":{"argc":command.len(),"argv0":command[0],
        "argv_hash":format!("{:x}",Sha256::digest(joined.as_bytes()))[..16]},"cwd":temp.0}),
    );
    let mut c = Command::new(command[0].as_str().unwrap());
    c.args(args)
        .current_dir(&temp.0)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for key in [
        "PATH", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "TMPDIR", "USER", "LOGNAME",
    ] {
        if let Ok(mut value) = std::env::var(key) {
            if key == "PATH" {
                value = value
                    .split(':')
                    .filter(|p| p.starts_with('/'))
                    .collect::<Vec<_>>()
                    .join(":");
            }
            c.env(key, value);
        }
    }
    c.env("HOME", &env.home)
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env(
            if r["engine"] == "codex-acp" {
                "CODEX_HOME"
            } else {
                "CLAUDE_CONFIG_DIR"
            },
            &home,
        );
    if let Some((_, wrapper)) = &artifacts {
        // codex-acp copies its own `features` into every thread; without this the observation
        // session would start the owner's Codex plugin servers (see `codex_thread_config`).
        c.env("CODEX_PATH", wrapper).env("NO_BROWSER", "1").env(
            cortex_engine::environment::CODEX_CONFIG,
            cortex_engine::environment::codex_thread_config(),
        );
    }
    if let Some(secret) = &secret {
        let key = match r["key_kind"].as_str() {
            Some("openai_api_key") => "CODEX_API_KEY",
            Some("anthropic_api_key") => "ANTHROPIC_API_KEY",
            Some("claude_oauth_token") => "CLAUDE_CODE_OAUTH_TOKEN",
            _ => return Err("engine_unreachable:PortableError".into()),
        };
        if secret.contains('\0') {
            return Err("engine_unreachable:ValueError".into());
        }
        c.env(key, secret);
    }
    if r["engine"] == "claude-agent-acp" {
        c.env("CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT", "0")
            .env("MCP_TOOL_TIMEOUT", "2147483647");
    }
    // Preserve only the owner's validated SSH agent socket, as Python does.
    if let Ok(path) = std::env::var("SSH_AUTH_SOCK") {
        use std::os::unix::fs::{FileTypeExt, MetadataExt};
        if path.starts_with('/')
            && !path.contains(['\0', '\n'])
            && fs::metadata(&path)
                .is_ok_and(|m| m.file_type().is_socket() && m.uid() == unsafe { libc::getuid() })
        {
            c.env("SSH_AUTH_SOCK", path);
        }
    }
    // No channel credentials or arbitrary ambient environment are forwarded.
    unsafe {
        c.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    if pinned.as_ref().is_some_and(|p| !p.unchanged())
        || codex.as_ref().is_some_and(|p| !p.unchanged())
    {
        return Err("engine_unreachable:PortableError".into());
    }
    #[cfg(test)]
    crate::native_account_fixture::on_exec(c.as_std_mut(), "catalogue", &env.home)
        .map_err(|_| "fixture_profile_unavailable".to_owned())?;
    let mut peer = start_peer(c, logs, secret, budget)?;
    let result = async {
        peer.call(
            "initialize",
            json!({"protocolVersion":1,"clientCapabilities":{}}),
        )
        .await?;
        let result = peer
            .call("session/new", json!({"cwd":temp.0,"mcpServers":[]}))
            .await?;
        let raw = result["configOptions"].clone();
        if raw.is_null() || raw.as_array().is_some_and(Vec::is_empty) {
            return Err("no_config_options_advertised".into());
        }
        if r["engine"] == "claude-agent-acp" {
            observation::model_efforts(&mut peer, &result, raw).await
        } else {
            observation::parse(&raw)
        }
    }
    .await;
    peer.close().await?;
    let mut result = result;
    if let (Ok(parsed), Some(install)) = (&mut result, &codex) {
        if !parsed.models.is_empty() {
            parsed.enrich(codex::ceiling(install, budget).await?);
        }
    }
    result.map_err(|reason| {
        if reason.starts_with("engine_unreachable:") && super::home_read_only(env, r) {
            "engine_home_read_only".into()
        } else {
            reason
        }
    })
}

/// The prompt-free OpenCode catalog read (`catalog.refresh._read_catalog_from_engine`
/// for `opencode`): a verified install launched only inside the engine jail with a
/// disposable working directory and no project grant.
async fn observe_opencode(
    env: &RouteEnvironment,
    r: &Value,
    budget: Duration,
) -> std::result::Result<observation::Observation, String> {
    let portable = || "engine_unreachable:PortableError".to_owned();
    let command: Vec<String> = r["launch_command"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_owned())
        .collect();
    let path = std::env::var_os("PATH");
    // An install that cannot be resolved and verified is never launched.
    let binary =
        oc::verified_install(&command, &env.home, path.as_ref()).map_err(|_| portable())?;
    let bwrap = oc::isolating_bwrap(path.as_ref()).ok_or_else(portable)?;
    let temp = Scratch::new().map_err(|_| "engine_unreachable:OSError".to_owned())?;
    let (workspace, jail_tmp) = (temp.0.join("workspace"), temp.0.join("jail-tmp"));
    for directory in [&workspace, &jail_tmp] {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .map_err(|_| "engine_unreachable:OSError".to_owned())?;
    }
    let logs = Logs::new(env).map_err(|_| portable())?;
    super::ensure_ignore(&env.config.join("cortex")).map_err(|_| portable())?;
    let expected = credential_path(env, r);
    if r["credential_ref"]["path"].as_str() != expected.to_str()
        || r["credential_ref"]["kind"] != "engine_native_dir"
        || expected.file_name().and_then(|n| n.to_str()) != Some("opencode")
    {
        return Err(portable());
    }
    use sha2::{Digest, Sha256};
    let joined = command.join("\u{1f}");
    logs.row(
        "starting",
        json!({"command":{"argc":command.len(),"argv0":command[0],
        "argv_hash":format!("{:x}",Sha256::digest(joined.as_bytes()))[..16]},"cwd":workspace}),
    );
    let launch = oc::launch(
        &bwrap,
        &binary,
        &env.home,
        &expected,
        &workspace,
        &jail_tmp,
        &oc::base_environment(),
    )
    .map_err(|_| "engine_unreachable:OSError".to_owned())?;
    logs.row(
        "opencode_owner_providers",
        json!({"imported":launch.imported,"skipped":launch.skipped}),
    );
    let mut c = Command::from(launch.command);
    c.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    unsafe {
        c.pre_exec(|| {
            if libc::setsid() < 0 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut peer = start_peer(c, logs, None, budget)?;
    let result = async {
        let initialized = peer
            .call(
                "initialize",
                json!({"protocolVersion":1,"clientCapabilities":{}}),
            )
            .await?;
        if !initialized["agentInfo"]["version"]
            .as_str()
            .is_some_and(oc::qualified)
        {
            return Err("engine_unreachable:RuntimeError".to_owned());
        }
        let result = peer
            .call("session/new", json!({"cwd":workspace,"mcpServers":[]}))
            .await?;
        let raw = result["configOptions"].clone();
        if raw.is_null() || raw.as_array().is_some_and(Vec::is_empty) {
            return Err("no_config_options_advertised".into());
        }
        observation::model_efforts(&mut peer, &result, raw).await
    }
    .await;
    peer.close().await?;
    let parsed = result.map_err(|reason| {
        if reason.starts_with("engine_unreachable:") && super::home_read_only(env, r) {
            "engine_home_read_only".to_owned()
        } else {
            reason
        }
    })?;
    // Models alone cannot establish that Cortex's required agent is usable.
    if !parsed.models.is_empty() && !parsed.offers("mode", oc::AGENT) {
        return Err("opencode_cortex_mode_not_advertised".into());
    }
    Ok(parsed)
}

#[derive(Clone)]
struct Logs {
    directory: PathBuf,
    id: String,
}
impl Logs {
    fn new(env: &RouteEnvironment) -> Result<Self> {
        let root = env.state.join("cortex");
        let base = root.join("engine-logs");
        let id = format!("eirt_{}", random_id().map_err(|_| unknown())?);
        let directory = base.join(&id);
        for path in [&root, &base, &directory] {
            private_directory(path, true).map_err(|_| unknown())?;
        }
        super::ensure_ignore(&root)?;
        use std::os::unix::fs::OpenOptionsExt;
        for name in ["observations.jsonl", "stderr.log"] {
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(directory.join(name))
                .map_err(|_| unknown())?;
        }
        Ok(Self { directory, id })
    }
    fn stderr_line(&self, text: &str, secret: Option<&str>) -> std::io::Result<()> {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        if secret.is_some_and(|s| {
            s.lines()
                .any(|part| !part.is_empty() && text.contains(part))
        }) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unqualified diagnostic",
            ));
        }
        let mut scrubbed = redaction::scrub(text);
        if scrubbed.chars().count() > 400 {
            scrubbed = format!(
                "{}...[truncated_from:{}_chars]",
                scrubbed.chars().take(400).collect::<String>(),
                text.chars().count()
            );
        }
        let bytes = format!("{}\n", scrubbed.trim_end_matches('\n')).into_bytes();
        let mut file = fs::OpenOptions::new()
            .append(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.directory.join("stderr.log"))?;
        let size = file.metadata()?.len();
        if size < 256 * 1024 {
            file.write_all(&bytes[..bytes.len().min((256 * 1024 - size) as usize)])?;
        }
        Ok(())
    }
    fn row(&self, event: &str, mut fields: Value) {
        use std::{io::Write, os::unix::fs::OpenOptionsExt};
        fields["event"] = json!(event);
        fields["engine_instance_id"] = json!(self.id);
        fields["time_ns"] = json!(std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64);
        // Values are fixed lifecycle metadata only; never raw stderr, argv arguments or RPC errors.
        if let Ok(mut file) = fs::OpenOptions::new()
            .append(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(self.directory.join("observations.jsonl"))
        {
            if let Ok(meta) = file.metadata() {
                if meta.len() < 256 * 1024 {
                    let _ = writeln!(file, "{}", fields);
                }
            }
        }
    }
}
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> std::io::Result<Self> {
        let root = std::env::temp_dir().join(format!("cortex-catalog-refresh-{}", random_id()?));
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new().mode(0o700).create(&root)?;
        Ok(Self(root))
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture {
        root: PathBuf,
        env: RouteEnvironment,
        case: Value,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("cw-publish-{}", random_id().unwrap()));
            fs::create_dir(&root).unwrap();
            let env = RouteEnvironment {
                home: root.join("home"),
                config: root.join("config"),
                state: root.join("state"),
                routing_override: None,
            };
            for p in [
                &env.home,
                &env.config,
                &env.config.join("cortex"),
                &env.state,
                &root.join("bin"),
                &env.home.join(".codex"),
            ] {
                use std::os::unix::fs::PermissionsExt;
                fs::create_dir(p).unwrap();
                fs::set_permissions(p, fs::Permissions::from_mode(0o700)).unwrap();
            }
            let corpus: Value = serde_json::from_str(include_str!(
                "../../tests/fixtures/config-catalog-producer-python.json"
            ))
            .unwrap();
            let case = corpus["cases"][0].clone();
            Self { root, env, case }
        }
        fn record(&self) -> Value {
            let bytes = self.case["initial"]["config/cortex/connections.json"]["content"]
                .as_str()
                .unwrap()
                .replace("@ROOT@", self.root.to_str().unwrap());
            serde_json::from_str::<Value>(&bytes).unwrap()["connections"]["cn_000000000001"].clone()
        }
        fn snapshot(&self) -> Value {
            let files = self.case["after"].as_object().unwrap();
            let (_, item) = files
                .iter()
                .find(|(p, _)| p.contains("/cs_") && p.ends_with(".json"))
                .unwrap();
            serde_json::from_str(item["content"].as_str().unwrap()).unwrap()
        }
        fn peer(&self) {
            use std::os::unix::fs::PermissionsExt;
            let script = self.case["peer"]
                .as_str()
                .unwrap()
                .replace("@ROOT@", self.root.to_str().unwrap());
            fs::write(self.root.join("bin/peer"), script).unwrap();
            fs::set_permissions(
                self.root.join("bin/peer"),
                fs::Permissions::from_mode(0o700),
            )
            .unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
    #[test]
    fn producer_claude_install_content_fence_rejects_drift() {
        let f = Fixture::new();
        let package = f.env.home.join("engines/claude");
        fs::create_dir_all(package.join("dist")).unwrap();
        let manifest = package.join("package.json");
        let entry = package.join("dist/index.js");
        fs::write(
            &manifest,
            r#"{"version":"0.31.1","bin":{"claude-agent-acp":"dist/index.js"}}"#,
        )
        .unwrap();
        fs::write(&entry, "// synthetic").unwrap();
        f.peer();
        let mut r = f.record();
        r["engine"] = json!("claude-agent-acp");
        r["launch_command"] = json!([f.root.join("bin/peer"), entry]);
        // Ordinary Node binaries exceed the small manifest bound. Hash them in chunks.
        let node = f.root.join("bin/peer");
        let mut executable = fs::read(&node).unwrap();
        executable.resize(5 * 1024 * 1024, b' ');
        fs::write(&node, executable).unwrap();
        let pin = install::claude(&f.env, &r).unwrap();
        assert!(pin.unchanged());
        fs::write(&manifest, "{}").unwrap();
        assert!(!pin.unchanged());
        assert!(install::claude(&f.env, &r).is_none());
        assert!(!f.root.join("bin/seen").exists());
    }
    #[test]
    fn producer_publication_never_replaces_and_post_commit_failure_remains_unknown() {
        let f = Fixture::new();
        let snapshot = f.snapshot();
        publish(&f.env, "cn_000000000001", &snapshot).unwrap();
        let path = f
            .env
            .state
            .join("cortex/catalog/cn_000000000001")
            .join(format!(
                "{}.json",
                snapshot["snapshot_id"].as_str().unwrap()
            ));
        let before = fs::read(&path).unwrap();
        let mut collision = snapshot.clone();
        collision["entries"] = json!([]);
        assert_eq!(
            publish(&f.env, "cn_000000000001", &collision)
                .unwrap_err()
                .code,
            "machine_outcome_unknown"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
        let mut second = snapshot.clone();
        second["snapshot_id"] = json!("cs_000000000002");
        assert_eq!(
            publish_with(&f.env, "cn_000000000001", &second, |_| Err(
                std::io::Error::other("injected sync failure")
            ))
            .unwrap_err()
            .code,
            "machine_outcome_unknown"
        );
        let directory = path.parent().unwrap();
        assert!(directory.join("cs_000000000002.json").exists());
        assert_eq!(
            fs::read_dir(directory).unwrap().count(),
            2,
            "staging names must be cleaned"
        );
        assert_eq!(fs::read(&path).unwrap(), before);
    }
    #[test]
    fn producer_timeout_is_bounded_reaped_and_never_publishes() {
        let f = Fixture::new();
        f.peer();
        fs::write(
            f.root.join("bin/peer"),
            "#!/bin/sh\nwhile IFS= read -r line; do :; done\n",
        )
        .unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let started = std::time::Instant::now();
        let result = runtime.block_on(observe_with(&f.env, &f.record(), Duration::from_millis(30)));
        assert!(matches!(result,Err(ref reason) if reason == "engine_unreachable:EngineTimeout"));
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!f.env.state.join("cortex/catalog").exists());
        for instance in fs::read_dir(f.env.state.join("cortex/engine-logs")).unwrap() {
            let text =
                fs::read_to_string(instance.unwrap().path().join("observations.jsonl")).unwrap();
            let rows: Vec<Value> = text
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            assert!(rows
                .iter()
                .any(|r| r["event"] == "timeout_alive" && r["alive"] == true));
            assert!(rows
                .iter()
                .any(|r| r["event"] == "process_exited" && r["exit_code"] == 0));
            assert!(rows.iter().any(|r| r["event"] == "transport_eof"));
        }
    }
    #[test]
    fn producer_overbound_stderr_never_publishes_or_replays() {
        let f = Fixture::new();
        f.peer();
        let path = f.root.join("bin/peer");
        let script = fs::read_to_string(&path)
            .unwrap()
            .replace("#!/bin/sh", "#!/bin/sh\nprintf '%070000d' 0 >&2");
        fs::write(&path, script).unwrap();
        let error = run(&f.env, &f.record(), || Ok(())).unwrap_err();
        assert_eq!(error.code, "config_action_unavailable");
        assert!(!f.env.state.join("cortex/catalog").exists());
        assert_eq!(
            fs::read_to_string(f.root.join("bin/seen"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }
    #[test]
    fn producer_known_secret_echo_is_not_retained_even_across_truncation() {
        let f = Fixture::new();
        let logs = Logs::new(&f.env).unwrap();
        for secret in ["short", "first\nsecond", &"xy-".repeat(400)] {
            assert!(logs.stderr_line(secret, Some(secret)).is_err());
        }
        assert_eq!(fs::read(logs.directory.join("stderr.log")).unwrap(), b"");
    }
    #[test]
    fn producer_stderr_is_scrubbed_before_private_retention() {
        let f = Fixture::new();
        f.peer();
        let path = f.root.join("bin/peer");
        let script = fs::read_to_string(&path)
            .unwrap()
            .replace("#!/bin/sh", "#!/bin/sh\nprintf 'synthetic diagnostic' >&2");
        fs::write(&path, script).unwrap();
        let reply = run(&f.env, &f.record(), || Ok(())).unwrap();
        assert_eq!(reply["success"], true);
        assert!(f.env.state.join("cortex/catalog").exists());
        assert_eq!(
            fs::read_to_string(f.root.join("bin/seen"))
                .unwrap()
                .lines()
                .count(),
            2
        );
        for instance in fs::read_dir(f.env.state.join("cortex/engine-logs")).unwrap() {
            assert_eq!(
                fs::read(instance.unwrap().path().join("stderr.log")).unwrap(),
                b"synthetic diagnostic\n"
            );
        }
    }
    /// A fake OpenCode: `--version` for the sandboxed probe, `acp` for the jailed read.
    fn fake_opencode(f: &Fixture, version: &str, mode: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let binary = f.env.home.join(".opencode/bin/opencode");
        fs::create_dir_all(binary.parent().unwrap()).unwrap();
        let script = format!(
            r##"#!/bin/sh
if [ "$1" = "--version" ]; then echo "opencode {version}"; exit 0; fi
[ "$1" = acp ] || exit 90
[ -n "$OPENCODE_CONFIG_CONTENT" ] || exit 91
[ "$OPENCODE_DISABLE_PROJECT_CONFIG" = 1 ] || exit 92
[ "$XDG_DATA_HOME" = "{data}" ] || exit 93
[ -e "$HOME/.config/opencode/owner-secret.txt" ] && exit 96
prompt=$(printf %s "$OPENCODE_CONFIG_CONTENT" | sed -n 's/.*{{file:\([^}}]*\)}}.*/\1/p')
[ -r "$prompt" ] || exit 97
[ -z "${{CORTEX_CHANNEL_SECRET+x}}" ] || exit 98
touch "{seen}"
while IFS= read -r line; do
 id=${{line#*\"id\"}}
 id=${{id#*:}}
 id=${{id%%,*}}
 id=${{id%%\}}*}}
 id=${{id# }}
 case "$line" in
 *session/prompt*) exit 95 ;;
 *initialize*) printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"agentInfo":{{"name":"opencode","version":"{version}"}}}}}}' ;;
 *session/new*) printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"sessionId":"s1","configOptions":[{{"id":"mode","name":"Mode","type":"select","category":"mode","currentValue":"{mode}","options":[{{"value":"{mode}","name":"Cortex"}}]}},{{"id":"model","name":"Model","type":"select","category":"model","currentValue":"m-a","options":[{{"value":"m-a","name":"Model A"}},{{"value":"m-b","name":"Model B"}}]}}]}}}}' ;;
 *session/set_config_option*)
 case "$line" in
 *m-b*) cur=m-b ;;
 *) cur=m-a ;;
 esac
 printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{"configOptions":[{{"id":"mode","name":"Mode","type":"select","category":"mode","currentValue":"{mode}","options":[{{"value":"{mode}","name":"Cortex"}}]}},{{"id":"model","name":"Model","type":"select","category":"model","currentValue":"'"$cur"'","options":[{{"value":"m-a","name":"Model A"}},{{"value":"m-b","name":"Model B"}}]}}]}}}}' ;;
 *) printf '%s\n' '{{"jsonrpc":"2.0","id":'"$id"',"result":{{}}}}' ;;
 esac
done
"##,
            data = f.env.home.join(".local/share").display(),
            seen = f.root.join("opencode-seen").display(),
        );
        fs::write(&binary, script).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        binary.canonicalize().unwrap()
    }
    fn opencode_record(f: &Fixture, binary: &Path) -> Value {
        let native = f.env.home.join(".local/share/opencode");
        fs::create_dir_all(&native).unwrap();
        fs::write(native.join("auth.json"), "{}").unwrap();
        // The owner's own configuration must stay hidden from the jailed engine.
        let owner = f.env.home.join(".config/opencode");
        fs::create_dir_all(&owner).unwrap();
        fs::write(owner.join("owner-secret.txt"), "hidden").unwrap();
        json!({"id":"cn_0000000000a1","name":"OpenCode","engine":"opencode","provider":"opencode",
            "auth_mode":"native_account","key_kind":null,"endpoint":null,
            "launch_command":[binary,"acp"],
            "credential_ref":{"kind":"engine_native_dir","path":native,"owned":false},
            "approved_fingerprint":crate::config_read_model::route_fingerprint(
                "opencode","opencode",None,Some(&[binary.to_string_lossy().into_owned(),"acp".into()])),
            "state":"configured","qualified":true,
            "created_at":"2026-11-16T00:00:00.000000+00:00","updated_at":"2026-11-16T00:00:00.000000+00:00",
            "max_concurrent_sessions":4})
    }
    fn bwrap_usable() -> bool {
        oc::isolating_bwrap(std::env::var_os("PATH").as_ref()).is_some_and(|b| {
            std::process::Command::new(b)
                .args([
                    "--unshare-user",
                    "--unshare-pid",
                    "--unshare-ipc",
                    "--ro-bind",
                    "/",
                    "/",
                    "/bin/true",
                ])
                .status()
                .is_ok_and(|s| s.success())
        })
    }
    #[test]
    fn opencode_catalog_is_read_natively_inside_the_jail_without_leaking() {
        if !bwrap_usable() {
            eprintln!(
                "bubblewrap with user namespaces is unavailable: jailed OpenCode read not run"
            );
            return;
        }
        let f = Fixture::new();
        let binary = fake_opencode(&f, "2.0.19", "cortex");
        let record = opencode_record(&f, &binary);
        let registry = registry::normalize(&json!({"schema_version":1,
            "connections":{"cn_0000000000a1":record},"override_history":{},"auto_connect_disabled":[]}))
        .unwrap();
        let r = registry["connections"]["cn_0000000000a1"].clone();
        assert!(admitted(&f.env, &r));
        let reply = run(&f.env, &r, || Ok(())).unwrap();
        assert_eq!(reply["success"], true, "{reply}");
        assert!(f.root.join("opencode-seen").exists());
        let catalog = f.env.state.join("cortex/catalog/cn_0000000000a1");
        let snapshot = fs::read_dir(&catalog)
            .unwrap()
            .flatten()
            .find(|e| e.file_name().to_string_lossy().starts_with("cs_"))
            .expect("a published snapshot");
        let body: Value = serde_json::from_slice(&fs::read(snapshot.path()).unwrap()).unwrap();
        assert_eq!(body["entries"].as_array().unwrap().len(), 2);
        assert_eq!(body["entries"][0]["route"]["engine"], "opencode");
        // The owner's own configuration was never modified.
        assert_eq!(
            fs::read_to_string(f.env.home.join(".config/opencode/owner-secret.txt")).unwrap(),
            "hidden"
        );
    }
    #[test]
    fn opencode_refresh_reports_unqualified_version_missing_mode_and_untrusted_install() {
        if !bwrap_usable() {
            eprintln!(
                "bubblewrap with user namespaces is unavailable: jailed OpenCode read not run"
            );
            return;
        }
        for (version, mode, expected) in [
            ("1.9.0", "cortex", "engine_unreachable:PortableError"),
            ("2.0.19", "build", "opencode_cortex_mode_not_advertised"),
        ] {
            let f = Fixture::new();
            let binary = fake_opencode(&f, version, mode);
            let record = opencode_record(&f, &binary);
            let registry = registry::normalize(&json!({"schema_version":1,
                "connections":{"cn_0000000000a1":record},"override_history":{},"auto_connect_disabled":[]}))
            .unwrap();
            let r = registry["connections"]["cn_0000000000a1"].clone();
            let reply = run(&f.env, &r, || Ok(())).unwrap();
            assert_eq!(reply["success"], false, "{reply}");
            assert_eq!(reply["reason"], expected, "{version}/{mode}");
            assert!(!f.env.state.join("cortex/catalog/cn_0000000000a1").exists());
        }
    }
    fn registry_fixture(f: &Fixture, record: &Value) -> Value {
        use std::os::unix::fs::PermissionsExt;
        let registry = json!({
            "schema_version": 1, "connections": {record["id"].as_str().unwrap(): record},
            "override_history": {}, "auto_connect_disabled": [],
        });
        let path = f.env.config.join("cortex/connections.json");
        fs::write(&path, serde_json::to_vec(&registry).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
        registry
    }

    #[test]
    fn first_run_publication_holds_registry_lock_against_concurrent_removal() {
        let f = Fixture::new();
        f.peer();
        let mut registry = registry_fixture(&f, &f.record());
        let r = registry::normalize(&registry).unwrap()["connections"]["cn_000000000001"].clone();
        let id = r["id"].as_str().unwrap();
        let lock_path = f.env.config.join("cortex/connections.lock");
        let registry_path = f.env.config.join("cortex/connections.json");
        let catalog = f.env.state.join("cortex/catalog").join(id);
        // Assert the guard is still held at publication completion, independently
        // of how quickly the competing writer is scheduled after unlock.
        struct PublishedGuard {
            _lock: AccessLock,
            catalog: PathBuf,
        }
        impl Drop for PublishedGuard {
            fn drop(&mut self) {
                assert!(fs::read_dir(&self.catalog).unwrap().next().is_some());
            }
        }
        std::thread::scope(|scope| {
            let (start_tx, start_rx) = std::sync::mpsc::channel();
            let (blocked_tx, blocked_rx) = std::sync::mpsc::channel();
            let catalog = &catalog;
            let lock_path = &lock_path;
            let writer = scope.spawn(move || {
                start_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                let error = AccessLock::acquire(&lock_path, false, true, Duration::ZERO)
                    .err()
                    .expect("publication must exclude the registry writer");
                assert_eq!(error.code(), "busy");
                blocked_tx.send(()).unwrap();
                let _lock =
                    AccessLock::acquire(&lock_path, false, true, Duration::from_secs(5)).unwrap();
                assert!(fs::read_dir(&catalog).unwrap().next().is_some());
                registry["connections"].as_object_mut().unwrap().remove(id);
                fs::write(&registry_path, serde_json::to_vec(&registry).unwrap()).unwrap();
                fs::remove_dir_all(&catalog).unwrap();
            });
            let reply = run_guarded(&f.env, &r, || {
                let lock = super::super::first_run_fence(&f.env, id, &r)?;
                start_tx.send(()).unwrap();
                blocked_rx.recv_timeout(Duration::from_secs(5)).unwrap();
                Ok(PublishedGuard {
                    _lock: lock,
                    catalog: catalog.clone(),
                })
            })
            .unwrap();
            assert_eq!(reply["success"], true);
            writer.join().unwrap();
        });
        assert!(
            !catalog.exists(),
            "publication must not recreate a removed catalogue"
        );
    }

    #[test]
    fn first_run_changed_or_removed_registry_refuses_publication() {
        for removed in [false, true] {
            let f = Fixture::new();
            f.peer();
            let mut registry = registry_fixture(&f, &f.record());
            let r =
                registry::normalize(&registry).unwrap()["connections"]["cn_000000000001"].clone();
            let id = r["id"].as_str().unwrap();
            let error = run_guarded(&f.env, &r, || {
                let lock = AccessLock::acquire(
                    &f.env.config.join("cortex/connections.lock"),
                    true,
                    true,
                    Duration::ZERO,
                )
                .unwrap();
                if removed {
                    registry["connections"].as_object_mut().unwrap().remove(id);
                } else {
                    registry["connections"][id]["name"] = json!("changed");
                }
                fs::write(
                    f.env.config.join("cortex/connections.json"),
                    serde_json::to_vec(&registry).unwrap(),
                )
                .unwrap();
                drop(lock);
                super::super::first_run_fence(&f.env, id, &r)
            })
            .unwrap_err();
            assert_eq!(error.code, "machine_outcome_unknown");
            assert!(!f.env.state.join("cortex/catalog").exists());
            assert_eq!(
                fs::read_to_string(f.root.join("bin/seen"))
                    .unwrap()
                    .lines()
                    .count(),
                2
            );
        }
    }

    #[test]
    fn producer_revision_fence_after_real_peer_prevents_publication_without_replay() {
        let f = Fixture::new();
        f.peer();
        let r = f.record();
        assert!(admitted(&f.env, &r));
        let error = run(&f.env, &r, || Err(unknown())).unwrap_err();
        assert_eq!(error.code, "machine_outcome_unknown");
        assert!(!f.env.state.join("cortex/catalog").exists());
        let calls = fs::read_to_string(f.root.join("bin/seen")).unwrap();
        assert_eq!(calls.lines().count(), 2);
        assert!(!calls.contains("session/prompt"));
    }
}
fn random_id() -> std::io::Result<String> {
    let mut bytes = [0; 6];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn unknown() -> Refused {
    refuse("machine_outcome_unknown","Catalog observation or publication may have completed. Re-read Settings before retrying; no automatic replay is safe.")
}
fn publish(env: &RouteEnvironment, id: &str, snapshot: &Value) -> Result<()> {
    publish_with(env, id, snapshot, |directory| {
        fs::File::open(directory)?.sync_all()
    })?;
    prune(&env.state.join("cortex/catalog").join(id));
    Ok(())
}
/// Readers use only the newest snapshot; route identities are content-derived, never
/// bound to a snapshot id. Keep a short history so the directory cannot grow without
/// bound (readers refuse beyond 4096 entries). Best-effort: a failed removal is harmless.
const KEPT_SNAPSHOTS: usize = 16;
fn prune(directory: &Path) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    let mut dated: Vec<_> = entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|s| s.starts_with("cs_") && s.ends_with(".json"))
        })
        .filter_map(|e| {
            let m = fs::symlink_metadata(e.path()).ok()?;
            (m.is_file()).then(|| (m.modified().ok(), e.path()))
        })
        .collect();
    dated.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in dated.into_iter().skip(KEPT_SNAPSHOTS) {
        let _ = fs::remove_file(path);
    }
}
fn publish_with(
    env: &RouteEnvironment,
    id: &str,
    snapshot: &Value,
    finish: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<()> {
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};
    let root = env.state.join("cortex");
    let directory = root.join("catalog").join(id);
    for path in [&root, &root.join("catalog"), &directory] {
        private_directory(path, true).map_err(|_| unknown())?;
    }
    super::ensure_ignore(&root).map_err(|_| unknown())?;
    let bytes = serde_json::to_vec(snapshot).map_err(|_| unknown())?;
    if bytes.len() > FRAME_BUDGET {
        return Err(unknown());
    }
    let target = directory.join(format!(
        "{}.json",
        snapshot["snapshot_id"].as_str().unwrap()
    ));
    let staging = directory.join(format!(".{}.tmp", random_id().map_err(|_| unknown())?));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&staging)
        .map_err(|_| unknown())?;
    let result = (|| -> std::io::Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        // Atomic visibility without replacement, including random-ID collisions.
        #[cfg(target_os = "linux")]
        {
            let from = CString::new(staging.as_os_str().as_encoded_bytes())?;
            let to = CString::new(target.as_os_str().as_encoded_bytes())?;
            if unsafe {
                libc::renameat2(
                    libc::AT_FDCWD,
                    from.as_ptr(),
                    libc::AT_FDCWD,
                    to.as_ptr(),
                    libc::RENAME_NOREPLACE,
                )
            } != 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            fs::hard_link(&staging, &target)?;
            fs::remove_file(&staging)?;
        }
        finish(&directory)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&staging); // only our own unpublished staging name
        return Err(unknown());
    }
    Ok(())
}
