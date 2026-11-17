//! A full task session through the real `cortex-acp` entry (its coordinator,
//! then a worker) never writes project trust into the owner's native Codex
//! config. The fake codex-acp starts its App Server once per engine process,
//! as the real adapter's ephemeral title thread does, and the fake App Server
//! persists trust for any thread whose cwd lacks a trust layer, as
//! codex-cli 0.164.0 does. Offline only: no account, network or model call.
#[path = "../../cortex-engine/src/bin/fake-engine.rs"]
mod peer;
#[path = "support/codex_runtime.rs"]
mod support;
use cortex_engine::wire;
use serde_json::{json, Value};
use std::{
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{
    io::BufReader,
    process::{ChildStdin, ChildStdout},
};

const EXE: &str = env!("CARGO_BIN_EXE_cortex-acp");
const MODEL: &str = "fixture-model";

fn private_json(path: &Path, value: Value) {
    std::fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
async fn send(input: &mut ChildStdin, id: i64, method: &str, params: Value) {
    wire::write_frame(
        input,
        &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
    )
    .await
    .unwrap();
}
async fn response(output: &mut BufReader<ChildStdout>, id: i64) -> Value {
    loop {
        let frame = tokio::time::timeout(Duration::from_secs(30), wire::read_frame(output))
            .await
            .expect("agent reply timeout")
            .unwrap()
            .expect("agent closed");
        if frame["id"] == id && frame.get("method").is_none() {
            return frame;
        }
    }
}

async fn task_session_leaves_owner_codex_config_byte_identical() {
    let f = support::Fixture::new("acp-task-trust");
    f.serve_task_sessions(&std::env::current_exe().unwrap(), MODEL);
    let config = f.0.join("home/.codex/config.toml");
    let baseline = b"# owner config\nsandbox_mode = \"danger-full-access\"\n";
    std::fs::write(&config, baseline).unwrap();
    let project = f.0.join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("work.txt"), "before").unwrap();
    // The fake worker records that it was prompted, then holds its turn.
    std::fs::write(project.join("scenario.txt"), "hold").unwrap();
    assert!(std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .current_dir(&project)
        .status()
        .unwrap()
        .success());
    let stderr = f.0.join("agent.stderr");
    let mut command = tokio::process::Command::from(f.command(EXE));
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&stderr).unwrap())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let diagnostics = || std::fs::read_to_string(&stderr).unwrap_or_default();
    // First run registers the pinned install-shaped connection and its catalogue.
    let deadline = Instant::now() + Duration::from_secs(90);
    while !f.finished() && Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let status = f.status().expect("first-run status");
    assert_eq!(status["phase"], "ready", "{status}: {}", diagnostics());
    assert!(f.models().iter().any(|m| m == MODEL), "{:?}", f.models());
    let registry_path = {
        let found = f.find("connections.json");
        assert_eq!(found.len(), 1, "{found:?}");
        found[0].clone()
    };
    let registry: Value = serde_json::from_slice(&std::fs::read(&registry_path).unwrap()).unwrap();
    let connections = registry["connections"].as_object().unwrap();
    assert_eq!(connections.len(), 1, "{registry}");
    let (id, record) = connections.iter().next().unwrap();
    assert_eq!(record["engine"], "codex-acp");
    assert_eq!(record["launch_command"].as_array().map(Vec::len), Some(2));
    let folder = registry_path.parent().unwrap();
    private_json(
        &folder.join("routing.json"),
        json!({"model_policy_version":2,"enabled":{"codex-acp":[MODEL]},
            "models":{"codex-acp":{"auto":[MODEL],"tiers":{MODEL:["light","standard","deep"]}}},
            "pin":{"*":format!("codex-acp/{MODEL}")}}),
    );
    private_json(
        &folder.join("defaults.json"),
        json!({"defaults":{"route_connection_id":id,"route_model":MODEL,"parameters":{}}}),
    );
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    send(
        &mut input,
        1,
        "initialize",
        json!({"protocolVersion":1,"clientCapabilities":{},"clientInfo":{"name":"trust-fixture"}}),
    )
    .await;
    assert_eq!(
        response(&mut output, 1).await["result"]["protocolVersion"],
        1
    );
    send(
        &mut input,
        2,
        "session/new",
        json!({"cwd":project,"mcpServers":[]}),
    )
    .await;
    let session = response(&mut output, 2).await;
    let sid = session["result"]["sessionId"]
        .as_str()
        .unwrap_or_else(|| panic!("{session}: {}", diagnostics()))
        .to_owned();
    send(
        &mut input,
        3,
        "session/prompt",
        json!({"sessionId":sid,"prompt":[{"type":"text","text":"Implement fixture"}]}),
    )
    .await;
    // The task keeps running while its worker holds; keep the agent's stdout
    // drained. A question card means the task stopped before any worker.
    let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen = stopped.clone();
    let drain = tokio::spawn(async move {
        let mut frames = Vec::new();
        while let Ok(Some(frame)) = wire::read_frame(&mut output).await {
            if frame["method"] == "session/request_permission" {
                seen.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            frames.push(frame);
        }
        frames
    });
    let entered = project.join("worker-entered");
    let deadline = Instant::now() + Duration::from_secs(90);
    while !entered.exists()
        && !stopped.load(std::sync::atomic::Ordering::SeqCst)
        && Instant::now() < deadline
    {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let observed = std::fs::read(&config).unwrap();
    let starts =
        std::fs::read_to_string(f.0.join("home/.codex/thread-starts.log")).unwrap_or_default();
    unsafe {
        libc::kill(child.id().unwrap() as i32, libc::SIGTERM);
    }
    drop(input);
    if tokio::time::timeout(Duration::from_secs(20), child.wait())
        .await
        .is_err()
    {
        child.kill().await.unwrap();
    }
    let frames = drain.await.unwrap();
    // Plan admission preflights verification in the read-only command
    // sandbox, so a worker is dispatched only where namespaces work; the
    // coordinator's sessions are covered on every host.
    let workers = namespaces_available();
    assert!(
        entered.exists() || !workers,
        "no worker was prompted; agent stderr: {}; frames: {frames:?}",
        diagnostics()
    );
    assert_eq!(
        String::from_utf8_lossy(&observed),
        String::from_utf8_lossy(baseline),
        "the owner's Codex config changed"
    );
    // The coordinator's and the worker's App Servers both ran in the project
    // with the non-persistent trust layer; no thread started without one.
    let project = project.canonicalize().unwrap();
    let in_project = starts
        .lines()
        .filter(|l| {
            l.split_once(' ')
                .is_some_and(|(_, cwd)| Path::new(cwd) == project)
        })
        .count();
    assert!(in_project >= if workers { 2 } else { 1 }, "{starts}");
    assert!(
        starts.lines().all(|l| l.starts_with("trusted ")),
        "{starts}"
    );
    if !workers {
        println!("namespace-restricted host: coordinator sessions asserted; the worker session requires real namespaces");
    }
}
fn namespaces_available() -> bool {
    let available = std::process::Command::new("/usr/bin/bwrap")
        .args([
            "--die-with-parent",
            "--unshare-pid",
            "--ro-bind",
            "/",
            "/",
            "--proc",
            "/proc",
            "--",
            "/bin/true",
        ])
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success());
    assert!(
        available || std::env::var_os("CORTEX_REQUIRE_NAMESPACE_FIXTURES").is_none(),
        "the worker journey requires real namespaces"
    );
    available
}

fn main() {
    if std::env::args()
        .nth(1)
        .is_some_and(|arg| arg.starts_with("desktop-"))
    {
        peer::main();
        return;
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(task_session_leaves_owner_codex_config_byte_identical());
    println!("codex task sessions: every App Server carried non-persistent trust; owner Codex config byte-identical");
}
