//! `acp-smoke`: the no-paid startup smoke harness for the production incoming ACP agent.
//!
//! Spawns the real `cortex-acp` binary against a fresh temporary project directory
//! and a fresh temporary `HOME`/`XDG_CONFIG_HOME`/`XDG_STATE_HOME`, with a minimal,
//! explicitly built process environment (never a copy of this process's environment),
//! so no host credential or unrelated variable reaches the agent. It never sends a
//! prompt that could reach a paid model and never reads real user connection or
//! catalog state: only the temporary directories created here.
//!
//! By default no connection exists, so the agent must advertise no `model` option.
//! `--fixture-connection` additionally seeds one clearly fake API-key connection
//! (fake key text) in the temporary roots, entirely outside the user's directories.
//!
//! Exit 0 iff every check passed; a skipped check is reported as `skip`, never as a
//! pass. The JSON summary goes to `--report PATH`, else to stderr; stdout carries
//! nothing from this harness. Known limits: this harness observes only the ACP
//! stream on stdout plus the exit code and the agent's stderr for diagnostics, so
//! a stray write ahead of the first protocol frame is indistinguishable from client
//! misbehaviour here, and it has no JSON-schema validator, so frame shapes are
//! checked field by field and the `schema:*` conformance check is reported `skip`.
use crate::acp_wire::{AgentSession, EXIT_TIMEOUT, READ_TIMEOUT};
use serde_json::{json, Value};
use std::fs;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

struct Report {
    checks: Vec<Value>,
}

impl Report {
    fn record(&mut self, name: &str, ok: bool, detail: Option<Value>) {
        let mut entry = json!({"name": name, "status": if ok { "pass" } else { "fail" }});
        if let Some(detail) = detail {
            entry["detail"] = detail;
        }
        self.checks.push(entry);
    }
    fn skip(&mut self, name: &str, reason: &str) {
        self.checks
            .push(json!({"name": name, "status": "skip", "detail": reason}));
    }
    fn all_passed(&self) -> bool {
        self.checks
            .iter()
            .all(|c| matches!(c["status"].as_str(), Some("pass" | "skip")))
    }
}

fn root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}

fn cortex_version(root: &Path) -> String {
    fs::read_to_string(root.join("package-identity.json"))
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v["version"].as_str().map(String::from))
        .unwrap_or_else(|| "unknown".into())
}

/// A temp root with no stray `.git`/`.cortex` marker directly inside it, because the
/// connection registry (correctly) refuses any ancestor that contains such a marker.
fn safe_tmp_base() -> PathBuf {
    let env_tmp = std::env::var("TMPDIR").ok();
    for candidate in [
        env_tmp.as_deref(),
        Some("/var/tmp"),
        Some(std::env::temp_dir().to_str().unwrap_or("/tmp")),
    ]
    .into_iter()
    .flatten()
    {
        let path = Path::new(candidate);
        let writable = path.is_dir()
            && fs::metadata(path)
                .map(|m| m.permissions().mode() & 0o200 != 0)
                .unwrap_or(false);
        if writable && !path.join(".git").exists() && !path.join(".cortex").exists() {
            return path.to_path_buf();
        }
    }
    std::env::temp_dir()
}

fn unique_dir(base: &Path, prefix: &str) -> Result<PathBuf, String> {
    let token = fs::read_to_string("/proc/sys/kernel/random/uuid").map_err(|e| e.to_string())?;
    let path = base.join(format!("{prefix}{}", token.trim().replace('-', "")));
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&path)
        .map_err(|e| format!("cannot create {}: {e}", path.display()))?;
    path.canonicalize().map_err(|e| e.to_string())
}

/// Delete one of this run's own temporary directories, including read-only directories a store creates.
fn remove_tree(path: &Path) -> std::io::Result<()> {
    fn writable(path: &Path) {
        if let Ok(meta) = fs::symlink_metadata(path) {
            if meta.is_dir() {
                let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
                if let Ok(entries) = fs::read_dir(path) {
                    for entry in entries.flatten() {
                        writable(&entry.path());
                    }
                }
            }
        }
    }
    writable(path);
    fs::remove_dir_all(path)
}

fn count_tasks(project: &Path, channel: &str) -> Result<i64, String> {
    let store = project
        .join(if channel == "dev" {
            ".cortex-dev"
        } else {
            ".cortex"
        })
        .join("cortex.sqlite3");
    if !store.exists() {
        return Ok(0);
    }
    let connection =
        rusqlite::Connection::open_with_flags(&store, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|e| format!("cannot open the task store read-only: {e}"))?;
    connection
        .query_row("SELECT count(*) FROM tasks", [], |row| row.get(0))
        .map_err(|e| format!("task count unavailable: {e}"))
}

/// Seed one fake API-key connection and an exact eligible fake route policy below one config root.
fn seed_fixture_connection(config_root: &Path) -> Result<(), String> {
    let cortex = config_root.join("cortex");
    let secrets = cortex.join("secrets");
    for dir in [config_root, cortex.as_path(), secrets.as_path()] {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| e.to_string())?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
    }
    let id = "cn_5a0e00000001";
    let secret = secrets.join(format!("{id}.key"));
    write_private(&secret, "fake-smoke-secret-key")?;
    let registry = json!({
        "schema_version": 1,
        "connections": {id: {
            "id": id, "name": "Smoke Fixture", "engine": "codex-acp", "provider": "openai",
            "auth_mode": "api_key", "key_kind": "openai_api_key", "endpoint": "cortex-smoke:fixture",
            "launch_command": null, "credential_ref": {"kind": "secret_file", "path": secret},
            "approved_fingerprint": "smoke-fixture", "state": "configured", "qualified": false,
            "created_at": "2000-01-01T00:00:00Z", "updated_at": "2000-01-01T00:00:00Z"
        }},
        "override_history": {},
    });
    write_private(
        &cortex.join("connections.json"),
        &format!("{}\n", serde_json::to_string_pretty(&registry).unwrap()),
    )?;
    let routing = json!({
        "model_policy_version": 2,
        "enabled": {"codex-acp": ["smoke-placeholder-model"]},
        "models": {"codex-acp": {
            "auto": ["smoke-placeholder-model"],
            "tiers": {"smoke-placeholder-model": "light"},
            "policy": {"smoke-placeholder-model": {}},
        }},
    });
    write_private(
        &cortex.join("routing.json"),
        &format!("{}\n", serde_json::to_string_pretty(&routing).unwrap()),
    )
}

fn write_private(path: &Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())
}

fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

fn run_smoke(
    root: &Path,
    binary: &Path,
    fixture: bool,
    report: &mut Report,
    keep: bool,
) -> Result<Value, String> {
    let base = safe_tmp_base();
    let project = unique_dir(&base, "cortex-acp-smoke-project-")?;
    let home = unique_dir(&base, "cortex-acp-smoke-home-")?;
    let outcome = smoke(root, binary, fixture, report, &project, &home);
    if keep {
        eprintln!(
            "acp-smoke: kept {} and {}",
            project.display(),
            home.display()
        );
    } else {
        for path in [&project, &home] {
            if let Err(error) = remove_tree(path) {
                eprintln!("acp-smoke: could not remove {}: {error}", path.display());
            }
        }
    }
    outcome
}

fn smoke(
    root: &Path,
    binary: &Path,
    fixture: bool,
    report: &mut Report,
    project: &Path,
    home: &Path,
) -> Result<Value, String> {
    if project.starts_with(root) {
        return Err("refusing to use the repository checkout as an ACP project root".into());
    }
    let (config_home, state_home) = (home.join("config"), home.join("state"));
    for dir in [&config_home, &state_home] {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let env: Vec<(String, String)> = [
        (
            "PATH",
            std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into()),
        ),
        ("HOME", home.display().to_string()),
        ("XDG_CONFIG_HOME", config_home.display().to_string()),
        ("XDG_STATE_HOME", state_home.display().to_string()),
        (
            "LANG",
            std::env::var("LANG")
                .ok()
                .filter(|l| !l.is_empty())
                .unwrap_or_else(|| "C.UTF-8".into()),
        ),
        // No engine download into this disposable HOME.
        ("CORTEX_ENGINE_UPDATE", "off".into()),
        ("CORTEX_ACP_RUNTIME_ROOT", root.display().to_string()),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    if fixture {
        // A compiled binary has a channel, not an env-selected one: seed both possible roots.
        let channels: Value = serde_json::from_str(
            &fs::read_to_string(root.join("crates/workspace-protocol/channels.json"))
                .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        let namespace = channels["dev"]["config_namespace"]
            .as_str()
            .ok_or("channels.json lacks the dev config namespace")?;
        seed_fixture_connection(&config_home)?;
        seed_fixture_connection(&config_home.join(namespace))?;
    }
    report.skip(
        "schema:frames",
        "no JSON-schema validator in this tool; frame fields are checked individually",
    );

    let mut session = AgentSession::spawn(&[binary.display().to_string()], &env, project)?;
    let mut protocol_version = Value::Null;
    let walked = (|| -> Result<(), String> {
        session.send(&request(
            1,
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {}, "clientInfo": {"name": "cortex-acp-smoke", "version": "1"}}),
        ))?;
        let init = session.recv(READ_TIMEOUT)?;
        let result = &init["result"];
        protocol_version = result["protocolVersion"].clone();
        let channel = result["_meta"]["cortexChannel"].as_str().unwrap_or("");
        report.record(
            "initialize:compiled_channel",
            matches!(channel, "release" | "dev"),
            Some(json!(channel)),
        );
        report.record(
            "initialize:protocol_version_1",
            protocol_version == json!(1),
            Some(protocol_version.clone()),
        );
        let caps = &result["agentCapabilities"];
        report.record(
            "initialize:loadSession_true",
            caps["loadSession"] == json!(true),
            Some(caps.clone()),
        );
        report.record(
            "initialize:image_prompt_capability",
            caps["promptCapabilities"]
                == json!({"image": true, "audio": false, "embeddedContext": true}),
            Some(caps["promptCapabilities"].clone()),
        );

        session.send(&request(
            2,
            "session/new",
            json!({"cwd": project, "mcpServers": []}),
        ))?;
        let created = session.recv(READ_TIMEOUT)?;
        if created.get("error").is_some() {
            // A failed session/new never gets its available_commands_update; fail now with the real error.
            return Err(format!(
                "session/new returned an error: {}",
                created["error"]
            ));
        }
        let session_id = created["result"]["sessionId"].as_str().map(String::from);
        report.record(
            "session/new:has_session_id",
            session_id.as_deref().is_some_and(|s| !s.is_empty()),
            None,
        );
        let options = created["result"]["configOptions"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let ids: std::collections::BTreeSet<&str> =
            options.iter().filter_map(|o| o["id"].as_str()).collect();
        let language_and_access = ids == ["access_mode", "language"].into_iter().collect();
        if fixture {
            // The fixture is API-key based; native session routes currently require native-account auth.
            report.record(
                "session/new:native_api_key_fixture_not_routable",
                language_and_access,
                Some(json!(options)),
            );
        } else {
            report.record(
                "session/new:no_model_option_without_connections",
                !ids.contains("model") && !ids.contains("thought_level") && language_and_access,
                Some(json!(options)),
            );
            let access = options.iter().find(|o| o["id"] == "access_mode");
            let values: std::collections::BTreeSet<&str> = access
                .and_then(|o| o["options"].as_array())
                .into_iter()
                .flatten()
                .filter_map(|o| o["value"].as_str())
                .collect();
            report.record(
                "session/new:access_mode_option_shape",
                access.is_some_and(|o| o["currentValue"] == "development")
                    && values == ["auto", "development", "guarded"].into_iter().collect(),
                Some(access.cloned().unwrap_or(Value::Null)),
            );
        }
        let commands = session.recv(READ_TIMEOUT)?;
        let names: std::collections::BTreeSet<String> = commands["params"]["update"]
            ["availableCommands"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["name"].as_str().map(String::from))
            .collect();
        report.record(
            "available_commands:cortex_status_and_setup_present",
            names.contains("cortex-status") && names.contains("cortex-setup"),
            Some(json!(names)),
        );

        let session_id = session_id.ok_or("no session id to prompt")?;
        session.send(&request(3, "session/prompt", json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "/cortex-status"}]})))?;
        let chunk = session.recv(READ_TIMEOUT)?;
        report.record(
            "cortex_status:agent_message_chunk",
            chunk["params"]["update"]["sessionUpdate"] == "agent_message_chunk",
            None,
        );
        let status = session.recv(READ_TIMEOUT)?;
        report.record(
            "cortex_status:end_turn",
            status["result"]["stopReason"] == "end_turn",
            Some(status.clone()),
        );
        let channel_name = if channel == "dev" { "dev" } else { "release" };
        match count_tasks(project, channel_name) {
            Ok(count) => report.record(
                "cortex_status:no_task_created",
                count == 0,
                Some(json!(count)),
            ),
            Err(error) => report.record("cortex_status:no_task_created", false, Some(json!(error))),
        }

        // A normal prompt must fail closed with no hidden provider call.
        session.send(&request(4, "session/prompt", json!({"sessionId": session_id, "prompt": [{"type": "text", "text": "Please write a haiku."}]})))?;
        let refused = session.recv(READ_TIMEOUT)?;
        let error = &refused["error"];
        report.record(
            "normal_prompt:orchestration_unavailable",
            error["code"] == -32001
                && error["message"] == "orchestration_unavailable"
                && error["data"]["reason"] == "no_configuration_revision",
            Some(error.clone()),
        );
        Ok(())
    })();
    let code = session.close(EXIT_TIMEOUT);
    report.record("process:clean_exit_0", code == Some(0), Some(json!(code)));
    walked?;
    Ok(json!({"cortex_version": cortex_version(root), "protocol_version": protocol_version}))
}

pub fn run(argv: &[String]) -> (String, i32) {
    let mut fixture = false;
    let mut keep = false;
    let mut report_path: Option<PathBuf> = None;
    let mut binary: Option<PathBuf> = None;
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--fixture-connection" => fixture = true,
            "--keep-temp" => keep = true,
            "--report" | "--rust-binary" => {
                let Some(value) = it.next() else {
                    eprintln!("acp-smoke: {arg} needs a value");
                    return (String::new(), 2);
                };
                if arg == "--report" {
                    report_path = Some(PathBuf::from(value));
                } else {
                    binary = Some(PathBuf::from(value));
                }
            }
            _ => {
                eprintln!("usage: acp-smoke [--rust-binary ABS_PATH] [--fixture-connection] [--report PATH] [--keep-temp]");
                return (String::new(), 2);
            }
        }
    }
    let root = root();
    let binary = binary.unwrap_or_else(|| root.join("target/debug/cortex-acp"));
    if !binary.is_absolute() {
        eprintln!("acp-smoke: --rust-binary must be an absolute path");
        return (String::new(), 2);
    }
    if !binary.is_file() {
        eprintln!("acp-smoke: {} is not built; run `cargo build --locked --offline -p cortex-acp` or pass --rust-binary", binary.display());
        return (String::new(), 2);
    }
    let mut report = Report { checks: Vec::new() };
    let mut versions = json!({"cortex_version": cortex_version(&root), "protocol_version": null});
    match run_smoke(&root, &binary, fixture, &mut report, keep) {
        Ok(found) => versions = found,
        Err(error) => report.record("harness:fatal", false, Some(json!(error))),
    }
    let run_id = fs::read_to_string("/proc/sys/kernel/random/uuid")
        .map(|u| u.trim().to_string())
        .unwrap_or_default();
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0);
    let summary = json!({
        "entry": {"kind": "rust", "argv": [binary]}, "versions": versions, "checks": report.checks,
        "all_passed": report.all_passed(), "run_id": run_id, "timestamp": timestamp,
    });
    let text = serde_json::to_string_pretty(&summary).unwrap_or_default();
    match &report_path {
        Some(path) => {
            let written = path
                .parent()
                .map_or(Ok(()), fs::create_dir_all)
                .and_then(|()| fs::write(path, format!("{text}\n")));
            if let Err(error) = written {
                eprintln!("acp-smoke: cannot write {}: {error}", path.display());
                return (String::new(), 1);
            }
        }
        None => eprintln!("{text}"),
    }
    if !report.all_passed() {
        let failed: Vec<String> = report
            .checks
            .iter()
            .filter(|c| c["status"] == "fail")
            .map(|c| format!("{}: {}", c["name"].as_str().unwrap_or(""), c["detail"]))
            .collect();
        eprintln!(
            "acp-smoke: FAILED ({} check(s)): {}",
            failed.len(),
            failed.join("; ")
        );
    }
    (String::new(), i32::from(!report.all_passed()))
}
