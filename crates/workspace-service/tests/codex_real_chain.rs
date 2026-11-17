//! Real installed codex-acp + Codex App Server behind Cortex's native launcher, with a
//! fake API key, a disposable Codex home and the network cut off (`unshare -rn`). No
//! prompt is sent, so no model is ever called. Explicitly ignored unless
//! `CORTEX_REAL_CODEX_BUNDLE` names an installed engine bundle (for example
//! `/usr/lib/cortex-workspace/engines`); run with `--ignored`.
#[path = "support/hermetic.rs"]
mod hermetic;
use cortex_engine::environment::{CodexLauncher, EngineKind, LaunchSpec, CODEX_MCP_SERVERS};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn write(path: &Path, body: &str, mode: u32) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}
/// The owner-side Codex state the live defect came from, reduced to fixtures: an
/// installed, enabled plugin whose MCP server starts with a thread (as the owner's
/// computer-use REPL and app tools did) and one configured MCP server. Every server
/// is a marker script that records its name.
fn fixture(root: &Path) {
    let marker = root.join("marker.sh");
    write(
        &marker,
        &format!(
            "#!/bin/sh\necho \"$1\" >> '{}'\nexec sleep 60\n",
            root.join("started").display()
        ),
        0o755,
    );
    let market = root.join("market");
    write(
        &market.join(".agents/plugins/marketplace.json"),
        &json!({"name":"fixture-market","plugins":[{"name":"probe","source":{"source":"local","path":"./plugins/probe"},
            "policy":{"installation":"AVAILABLE","authentication":"ON_INSTALL"},"category":"Engineering"}]})
        .to_string(),
        0o644,
    );
    for dir in [
        market.join("plugins/probe"),
        root.join("codex/plugins/cache/fixture-market/probe/1.0.0"),
    ] {
        write(
            &dir.join(".codex-plugin/plugin.json"),
            &json!({"name":"probe","version":"1.0.0","mcpServers":"./.mcp.json"}).to_string(),
            0o644,
        );
        write(
            &dir.join(".mcp.json"),
            &json!({"mcpServers":{"plugin_probe":{"command":marker,"args":["plugin_probe"],"startup_timeout_sec":5}}})
                .to_string(),
            0o644,
        );
    }
    write(
        &root.join("codex/config.toml"),
        &format!(
            "[marketplaces.fixture-market]\nsource_type = \"local\"\nsource = \"{}\"\n\n\
             [plugins.\"probe@fixture-market\"]\nenabled = true\n\n\
             [mcp_servers.owner_own]\ncommand = \"{}\"\nargs = [\"owner_own\"]\n",
            market.display(),
            marker.display()
        ),
        0o600,
    );
    // A synthetic API key; the network is cut off, so it is never sent anywhere.
    write(
        &root.join("codex/auth.json"),
        &json!({"auth_mode":"apikey","OPENAI_API_KEY":"sk-synthetic-offline-not-a-key"})
            .to_string(),
        0o600,
    );
    fs::create_dir_all(root.join("home")).unwrap();
    fs::create_dir_all(root.join("project")).unwrap();
}
/// One codex-acp `initialize` + `session/new` (no prompt). Returns the names of the
/// MCP servers that started, observed until the `cortex` relay stand-in started
/// (plus a settle margin) or the deadline.
fn session(root: &Path, bundle: &Path, session_launch: bool) -> BTreeSet<String> {
    let _ = fs::remove_file(root.join("started"));
    let codex = bundle
        .join("node_modules/@openai/codex-linux-x64/vendor/x86_64-unknown-linux-musl/bin/codex");
    let launcher = root.join("codex-acp-runtime");
    write(
        &launcher,
        &workspace_service::native_codex_runtime::artifact(
            Path::new(env!("CARGO_BIN_EXE_workspace-service")),
            &codex,
        )
        .unwrap(),
        0o700,
    );
    let prepared = launcher.clone();
    let spec = LaunchSpec {
        engine: EngineKind::Codex,
        executable: bundle.join("node/bin/node"),
        args: vec![bundle
            .join("node_modules/@agentclientprotocol/codex-acp/dist/index.js")
            .to_string_lossy()
            .into_owned()],
        cwd: root.join("project"),
        native_home: root.join("home"),
        credential_directory: root.join("codex"),
        base_environment: BTreeMap::from([
            ("PATH".to_owned(), "/usr/bin:/bin".to_owned()),
            ("LANG".to_owned(), "C.UTF-8".to_owned()),
        ]),
        codex_launcher: Some(CodexLauncher::new(move || Ok(prepared.clone()))),
    };
    // Exactly the environment a Cortex session launch passes to codex-acp, plus the
    // relay hand-over (here one stand-in named like the Cortex relay).
    let mut env = spec.launch_environment().unwrap();
    if session_launch {
        env.insert(
            CODEX_MCP_SERVERS.into(),
            json!([{"name":"cortex","command":root.join("marker.sh"),"args":["cortex"],"env":[]}])
                .to_string(),
        );
    } else {
        // A launch without relays and without codex-acp's thread feature config: the
        // control that shows the fixture detects a thread that turns plugins back on.
        env.remove(cortex_engine::environment::CODEX_CONFIG);
    }
    let mut child = Command::new("unshare")
        .args(["-rn", "--"])
        .arg(&spec.executable)
        .args(&spec.args)
        .current_dir(&spec.cwd)
        .env_clear()
        .envs(&env)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .expect("unshare is required to cut the network off");
    let pid = child.id() as i32;
    let mut stdin = child.stdin.take().unwrap();
    let stdout = child.stdout.take().unwrap();
    let (lines, received) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if lines.send(line).is_err() {
                break;
            }
        }
    });
    writeln!(stdin, "{}", json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}})).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    let initialized = loop {
        match received.recv_timeout(Duration::from_millis(200)) {
            Ok(line) if line.contains("\"id\":1") => break true,
            _ if Instant::now() > deadline => break false,
            _ => {}
        }
    };
    let mut observed = BTreeSet::new();
    if initialized {
        writeln!(stdin, "{}", json!({"jsonrpc":"2.0","id":2,"method":"session/new","params":{"cwd":spec.cwd,"mcpServers":[]}})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(45);
        let mut settle: Option<Instant> = None;
        while Instant::now() < deadline && settle.is_none_or(|t| Instant::now() < t) {
            while received.try_recv().is_ok() {}
            std::thread::sleep(Duration::from_millis(200));
            observed = fs::read_to_string(root.join("started"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect();
            if settle.is_none()
                && (observed.contains("cortex") || observed.contains("plugin_probe"))
            {
                settle = Some(Instant::now() + Duration::from_secs(5));
            }
        }
    }
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let _ = child.wait();
    assert!(initialized, "codex-acp did not answer initialize");
    observed
}
#[test]
#[ignore = "needs CORTEX_REAL_CODEX_BUNDLE (an installed engine bundle) and unshare; run with --ignored"]
fn real_codex_session_starts_only_the_cortex_relay() {
    let bundle = PathBuf::from(
        std::env::var_os("CORTEX_REAL_CODEX_BUNDLE")
            .expect("CORTEX_REAL_CODEX_BUNDLE must name the installed engine bundle"),
    );
    let root = std::env::temp_dir().join(format!("cw-codex-real-chain-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let result = std::panic::catch_unwind(|| {
        fixture(&root);
        let control = session(&root, &bundle, false);
        assert!(
            control.contains("plugin_probe"),
            "the fixture must detect a thread that turns the owner's plugins back on: {control:?}"
        );
        let started = session(&root, &bundle, true);
        assert_eq!(
            started,
            BTreeSet::from(["cortex".to_owned()]),
            "a Cortex session started servers other than its relay"
        );
        println!("control without thread features started {control:?}; Cortex session started {started:?}");
    });
    let _ = fs::remove_dir_all(&root);
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
