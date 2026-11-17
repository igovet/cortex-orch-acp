//! `paid-probe codex|claude`: the only paid engine probes.
//!
//! One explicitly authorized, bounded ACP turn against one named native engine
//! adapter in a disposable workspace. Nothing runs without BOTH
//! `--i-authorize-paid-call` and a non-empty `--approval-ref`: without them the
//! command validates its arguments, prints a refusal and exits 2 before reading
//! any file beyond argv, creating a workspace, starting a process or touching
//! the network. `--plan` prints the exact steps and exits 0 without any
//! authorization and without executing anything.
//!
//! The probe observes one configured engine turn only. No worker cycle, runtime
//! completion or GUI behaviour is claimed, no transcript or secret is recorded,
//! and every permission request the adapter makes is cancelled. This tool is
//! never run by the automated suite (the unit tests below cover the
//! refusal and `--plan` paths, the only paths tests may reach).
use crate::acp_wire::AgentSession;
use serde_json::json;
use std::fs;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const TURN_SECONDS: u64 = 180;
const PROMPT: &str = "One bounded qualification prompt: reply with the single word ok.";

fn plan(engine: &str) -> Vec<String> {
    let scope = if engine == "codex" {
        "codex-acp"
    } else {
        "claude-agent-acp"
    };
    vec![
        format!("1. Require --adapter to be an absolute path of an executable {scope} adapter file; it is never searched on PATH."),
        "2. Create a disposable temporary workspace outside this checkout and outside any Git or .cortex project.".into(),
        "3. Start the adapter with a minimal environment (PATH, the real HOME for the native account login, LANG); no other variable is inherited.".into(),
        "4. initialize (ACP v1), then session/new for the disposable workspace; with --model, require that the session advertises that exact model option value, and refuse before any prompt when it does not.".into(),
        format!("5. Send exactly one bounded prompt and observe the stop reason for at most {TURN_SECONDS}s; cancel every permission request; answer any other agent request with a method-not-found error."),
        "6. Write a record of the actual stop reason and requested model to --output (no transcript, no secret, no environment).".into(),
        "7. Close stdin and stop the adapter, then remove the disposable workspace. This is not a full orchestration or GUI qualification.".into(),
    ]
}

struct Args {
    authorize: bool,
    approval_ref: Option<String>,
    adapter: Option<PathBuf>,
    model: Option<String>,
    output: Option<PathBuf>,
    plan: bool,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut args = Args { authorize: false, approval_ref: None, adapter: None, model: None, output: None, plan: false };
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--i-authorize-paid-call" => args.authorize = true,
            "--plan" => args.plan = true,
            "--approval-ref" | "--adapter" | "--model" | "--output" => {
                let value = it.next().ok_or_else(|| format!("{arg} needs a value"))?.clone();
                match arg.as_str() {
                    "--approval-ref" => args.approval_ref = Some(value),
                    "--adapter" => args.adapter = Some(PathBuf::from(value)),
                    "--model" => args.model = Some(value),
                    _ => args.output = Some(PathBuf::from(value)),
                }
            }
            other => return Err(format!("unrecognized argument {other:?}")),
        }
    }
    Ok(args)
}

pub fn run(engine: &str, argv: &[String]) -> (String, i32) {
    let label = format!("paid-probe {engine}");
    let args = match parse(argv) {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{label}: {error}");
            return (String::new(), 2);
        }
    };
    if args.plan {
        let mut text = format!("{label} --plan (no authorization, nothing executed):\n");
        for step in plan(engine) {
            text.push_str(&format!("  {step}\n"));
        }
        return (text, 0);
    }
    if !args.authorize || args.approval_ref.as_deref().is_none_or(|r| r.trim().is_empty()) {
        eprintln!(
            "{label}: refusing to run without both --i-authorize-paid-call and a non-empty --approval-ref; exiting before any \
             file read, workspace creation, process start or network activity. Use --plan to see the steps."
        );
        return (String::new(), 2);
    }
    for (flag, present) in [("--adapter", args.adapter.is_some()), ("--output", args.output.is_some())] {
        if !present {
            eprintln!("{label}: {flag} is required once authorized.");
            return (String::new(), 2);
        }
    }
    match authorized(engine, &args) {
        Ok(path) => (format!("{label}: wrote {}\n", path.display()), 0),
        Err(error) => {
            eprintln!("{label}: {error}");
            (String::new(), 1)
        }
    }
}

fn workspace() -> Result<PathBuf, String> {
    let token = fs::read_to_string("/proc/sys/kernel/random/uuid").map_err(|e| e.to_string())?;
    let path = Path::new("/var/tmp").join(format!("cortex-probe-{}", token.trim().replace('-', "")));
    fs::DirBuilder::new().mode(0o700).create(&path).map_err(|e| format!("cannot create the workspace: {e}"))?;
    path.canonicalize().map_err(|e| e.to_string())
}

fn authorized(engine: &str, args: &Args) -> Result<PathBuf, String> {
    let adapter = args.adapter.as_ref().ok_or("--adapter is required")?;
    let output = args.output.as_ref().ok_or("--output is required")?;
    let meta = fs::metadata(adapter).map_err(|e| format!("adapter {}: {e}", adapter.display()))?;
    if !adapter.is_absolute() || !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
        return Err("--adapter must be the absolute path of an executable file".into());
    }
    let home = std::env::var("HOME").map_err(|_| "HOME is not set; the native account login lives there")?;
    let work = workspace()?;
    let env: Vec<(String, String)> = vec![
        ("PATH".into(), std::env::var("PATH").unwrap_or_else(|_| "/usr/bin:/bin".into())),
        ("HOME".into(), home),
        ("LANG".into(), "C.UTF-8".into()),
    ];
    let result = turn(adapter, &env, &work, args.model.as_deref());
    let _ = fs::remove_dir_all(&work);
    let (stop_reason, model) = result?;
    let started_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let record = json!({
        "status": "ran", "engine": engine, "started_unix": started_at,
        "configuration": {"requested_model": model},
        "execution": {"exit_status": stop_reason},
        "note": "Single-turn transport probe only; full runtime orchestration and GUI behavior remain unverified.",
    });
    let text = serde_json::to_string_pretty(&record).map_err(|e| e.to_string())?;
    fs::write(output, text).map_err(|e| format!("cannot write {}: {e}", output.display()))?;
    Ok(output.clone())
}

fn turn(adapter: &Path, env: &[(String, String)], work: &Path, model: Option<&str>) -> Result<(Option<String>, Option<String>), String> {
    let mut session = AgentSession::spawn(&[adapter.display().to_string()], env, work)?;
    let outcome = (|| {
        let wait = Duration::from_secs(30);
        session.send(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"cortex-paid-probe","version":"1"}}}))?;
        let init = session.recv(wait)?;
        if init["result"]["protocolVersion"] != json!(1) {
            return Err("the adapter did not negotiate ACP protocol version 1".to_string());
        }
        session.send(&json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":work,"mcpServers":[]}}))?;
        let created = loop {
            let message = session.recv(wait)?;
            if message["id"] == json!(2) {
                break message;
            }
        };
        let session_id = created["result"]["sessionId"].as_str().ok_or("session/new returned no session id")?.to_string();
        if let Some(model) = model {
            let options = created["result"]["configOptions"].as_array().cloned().unwrap_or_default();
            let offered = options.iter().any(|o| {
                o["id"] == "model" && o["options"].as_array().into_iter().flatten().any(|v| v["value"] == model)
            });
            if !offered {
                return Err(format!("the session does not advertise model {model:?}; refusing before any prompt"));
            }
            session.send(&json!({"jsonrpc":"2.0","id":3,"method":"session/set_config_option","params":{"sessionId":session_id,"configId":"model","value":model}}))?;
            loop {
                let message = session.recv(wait)?;
                if message["id"] == json!(3) {
                    if message.get("error").is_some() {
                        return Err("the adapter refused the model selection; refusing before any prompt".to_string());
                    }
                    break;
                }
            }
        }
        session.send(&json!({"jsonrpc":"2.0","id":4,"method":"session/prompt","params":{"sessionId":session_id,"prompt":[{"type":"text","text":PROMPT}]}}))?;
        let deadline = Instant::now() + Duration::from_secs(TURN_SECONDS);
        loop {
            let left = deadline.checked_duration_since(Instant::now()).ok_or("the turn did not finish within its bound")?;
            let message = session.recv(left)?;
            if message["id"] == json!(4) && message.get("method").is_none() {
                return Ok(message["result"]["stopReason"].as_str().map(String::from));
            }
            if let (Some(id), Some(method)) = (message.get("id"), message["method"].as_str()) {
                let reply = if method == "session/request_permission" {
                    json!({"jsonrpc":"2.0","id":id,"result":{"outcome":{"outcome":"cancelled"}}})
                } else {
                    json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"method not found"}})
                };
                session.send(&reply)?;
            }
        }
    })();
    session.close(Duration::from_secs(10));
    outcome.map(|stop| (stop, model.map(String::from)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn refuses_without_both_authorization_flags() {
        for engine in ["codex", "claude"] {
            for given in [&["--adapter", "/bin/true", "--output", "/tmp/x"][..], &["--i-authorize-paid-call"], &["--approval-ref", "ref"], &["--i-authorize-paid-call", "--approval-ref", "  "]] {
                assert_eq!(run(engine, &args(given)).1, 2, "{given:?}");
            }
        }
    }

    #[test]
    fn plan_needs_no_authorization_and_executes_nothing() {
        let (text, code) = run("codex", &args(&["--plan"]));
        assert_eq!(code, 0);
        assert!(text.contains("nothing executed") && text.contains("codex-acp"));
        assert!(run("claude", &args(&["--plan"])).0.contains("claude-agent-acp"));
    }

    #[test]
    fn authorized_run_still_requires_adapter_and_output_before_anything_starts() {
        assert_eq!(run("codex", &args(&["--i-authorize-paid-call", "--approval-ref", "owner-request-x"])).1, 2);
    }

    #[test]
    fn an_unusable_adapter_is_refused_before_a_workspace_exists() {
        let given = args(&["--i-authorize-paid-call", "--approval-ref", "owner-request-x", "--adapter", "relative/adapter", "--output", "/tmp/never-written"]);
        assert_eq!(run("claude", &given).1, 1);
        assert!(!Path::new("/tmp/never-written").exists());
    }
}
