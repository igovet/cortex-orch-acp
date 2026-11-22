//! No engine, shell peer, network, account, keyring or installed state.
#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, BufRead, Write},
    os::unix::{fs::PermissionsExt, process::ExitStatusExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn ignored(signal: i32) -> bool {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        assert_eq!(libc::sigaction(signal, std::ptr::null(), &mut action), 0);
        action.sa_sigaction == libc::SIG_IGN
    }
}
fn peer() -> i32 {
    let argv: Vec<String> = std::env::args().collect();
    let phase = if argv.ends_with(&["features".into(), "list".into()]) {
        "features"
    } else if argv.ends_with(&["mcp".into(), "list".into(), "--json".into()]) {
        "mcp"
    } else if argv.last().map(String::as_str) == Some("app-server") {
        "app-server"
    } else {
        return 99;
    };
    let case = std::env::var("CW_FAKE_CODEX").unwrap();
    let row = json!({
        "pid":std::process::id(),
        "argv":argv,
        "cwd":std::env::current_dir().unwrap(),
        "env":std::env::vars().collect::<std::collections::BTreeMap<_,_>>(),
        "sigpipe":ignored(libc::SIGPIPE),
        "sigxfsz":ignored(libc::SIGXFSZ)
    });
    let mut out = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(std::env::var("CW_TRANSCRIPT").unwrap())
        .unwrap();
    writeln!(out, "{row}").unwrap();
    if phase == "features" {
        if case == "timeout" {
            std::thread::sleep(std::time::Duration::from_secs(30));
        }
        if case == "feature-sigterm" {
            unsafe {
                libc::raise(libc::SIGTERM);
            }
        }
        if case == "nonzero" {
            eprintln!("unretained synthetic probe detail");
            return 9;
        }
        if case == "invalid-utf8" {
            io::stdout().write_all(&[255]).unwrap();
            return 0;
        }
        for name in [
            "multi_agent",
            "multi_agent_v2",
            "plugins",
            "shell_tool",
            "view_image",
        ] {
            if case == "missing-feature" && name == "plugins" {
                continue;
            }
            let enabled = if case == "enabled-feature" && name == "plugins" {
                "true"
            } else {
                "false"
            };
            if case == "unicode-whitespace" {
                print!("{name}\u{1f}stable\u{2003}{enabled}\u{85}");
            } else {
                println!("{name} stable {enabled}");
            }
        }
        if case == "last-feature-wins" {
            println!("plugins stable true");
        }
        if case == "last-feature-disabled" {
            println!("plugins stable false");
        }
        if case == "large-stderr" {
            io::stderr().write_all(&vec![b'x'; 128 * 1024]).unwrap();
        }
    } else if phase == "mcp" {
        let overridden = argv.iter().any(|a| a.starts_with("mcp_servers."));
        let value: Value = match case.as_str() {
            "empty" => json!([]),
            "malformed" => {
                println!("not JSON");
                return 0;
            }
            "wrong-shape" => json!({}),
            "null-entry" => json!([null]),
            "missing-enabled" => json!([{"name":"alpha"}]),
            "numeric-enabled" => json!([{"name":"alpha","enabled":1}]),
            "null-name" => json!([{"name":null,"enabled":false}]),
            "empty-name" => json!([{"name":"","enabled":false}]),
            "duplicate-name" => {
                json!([{"name":"alpha","enabled":false},{"name":"alpha","enabled":false}])
            }
            "unicode-name" => json!([{"name":"模型","enabled":false}]),
            "injection-name" => json!([{"name":"x.y","enabled":false}]),
            "enabled-after" if overridden => json!([{"name":"alpha","enabled":true}]),
            "second-malformed" if overridden => {
                println!("not JSON");
                return 0;
            }
            "changed-second-list" if overridden => json!([]),
            // Codex's own layering: `name.enabled=false` turns a configured server off and an
            // inline `name={...}` table defines one ("relay-dropped": a Codex that ignored it).
            "relays" | "relay-dropped" => {
                let layers: Vec<&str> = argv
                    .iter()
                    .filter_map(|a| a.strip_prefix("mcp_servers."))
                    .collect();
                let mut rows =
                    vec![json!({"name":"z-9","enabled":!layers.contains(&"z-9.enabled=false")})];
                for layer in &layers {
                    if let Some((name, table)) = layer.split_once('=') {
                        if table.starts_with('{') && case == "relays" {
                            rows.push(json!({"name":name,"enabled":true}));
                        }
                    }
                }
                json!(rows)
            }
            _ => {
                json!([{"name":"z-9","enabled":!overridden},{"name":"A_1","enabled":false,"ignored":"value"}])
            }
        };
        println!("{value}");
    } else {
        println!("PID={}", std::process::id());
        eprintln!("synthetic final stderr");
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let request: Value = serde_json::from_str(&line.unwrap()).unwrap();
            assert_eq!(request["method"], "initialize");
            println!(
                "{}",
                json!({"jsonrpc":"2.0","id":request["id"],"result":{}})
            );
        }
        match case.as_str() {
            "exit-7" => return 7,
            "sigterm" | "sigint" | "sigpipe" => unsafe {
                let signal = match case.as_str() {
                    "sigterm" => libc::SIGTERM,
                    "sigint" => libc::SIGINT,
                    _ => libc::SIGPIPE,
                };
                libc::raise(signal);
            },
            _ => (),
        }
    }
    0
}
/// What one run of a launcher produced: exit (negative for a signal), stdout, stderr, and one
/// transcript row per process the launcher started.
type Outcome = (i64, Vec<u8>, Vec<u8>, Vec<Value>);
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
fn run(root: &Path, path: &Path, case: &str, argv: &[Value], locale: &Value) -> Outcome {
    run_with(root, path, case, argv, locale, &[])
}
fn run_with(
    root: &Path,
    path: &Path,
    case: &str,
    argv: &[Value],
    locale: &Value,
    extra: &[(&str, &str)],
) -> Outcome {
    let transcript = root.join("transcript");
    let _ = fs::remove_file(&transcript);
    let cwd = root.join("work space");
    let _ = fs::create_dir(&cwd);
    let mut command = Command::new(path);
    command
        .args(argv.iter().map(|a| a.as_str().unwrap()))
        .current_dir(&cwd)
        .env_clear()
        .env("HOME", root)
        .env("XDG_STATE_HOME", root)
        .env("XDG_CONFIG_HOME", root)
        .env("CODEX_HOME", root)
        .env("PATH", root)
        .env("LANG", "C.UTF-8")
        .env("CW_FAKE_CODEX", case)
        .env("CW_TRANSCRIPT", &transcript)
        .env("CODEX_API_KEY", "synthetic-offline")
        .env("CORTEX_CHANNEL_SECRET", "synthetic-not-a-real-channel")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in locale.as_object().unwrap() {
        match value.as_str() {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    for (key, value) in extra {
        command.env(key, value);
    }
    let start = Instant::now();
    let mut child = command.spawn().unwrap();
    let pid = child.id();
    // A launcher that refuses exits without reading stdin; its outcome is checked below.
    if let Err(error) = child
        .stdin
        .take()
        .unwrap()
        .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{}}\n")
    {
        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe, "{error}");
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(child.wait_with_output());
    });
    let output = match receiver.recv_timeout(Duration::from_secs(25)) {
        Ok(output) => output.unwrap(),
        Err(_) => {
            unsafe { libc::kill(pid as i32, libc::SIGKILL) };
            panic!("launcher did not finish: {case}");
        }
    };
    let mut rows: Vec<Value> = fs::read_to_string(&transcript)
        .map(|text| {
            text.lines()
                .map(|l| serde_json::from_str(l).unwrap())
                .collect()
        })
        .unwrap_or_default();
    for row in &mut rows {
        let peer = row
            .as_object_mut()
            .unwrap()
            .remove("pid")
            .unwrap()
            .as_i64()
            .unwrap();
        if row["argv"].as_array().unwrap().last().unwrap() == "app-server" {
            assert_eq!(peer, pid as i64, "final peer must replace the launcher PID");
        }
        assert_eq!(
            unsafe { libc::kill(peer as i32, 0) },
            -1,
            "launcher left an unreaped fake child"
        );
    }
    let mut out = output.stdout;
    if out.windows(4).any(|w| w == b"PID=") {
        let prefix = format!("PID={pid}\n");
        assert!(
            out.starts_with(prefix.as_bytes()),
            "final peer must replace launcher PID"
        );
        out.splice(..prefix.len() - 1, *b"PID=@PID@");
    }
    if case == "timeout" {
        let elapsed = start.elapsed().as_secs_f64();
        assert!(
            (19.0..=24.0).contains(&elapsed),
            "real 20-second check deadline"
        );
    }
    let status = output
        .status
        .code()
        .map(i64::from)
        .or_else(|| output.status.signal().map(|s| -i64::from(s)))
        .unwrap();
    (status, out, output.stderr, rows)
}
/// Rows as the golden data records them: the run's private root is a placeholder.
fn normalized(root: &Path, rows: &[Value]) -> Value {
    serde_json::from_str(
        &json!(rows)
            .to_string()
            .replace(root.to_str().unwrap(), "@ROOT@"),
    )
    .unwrap()
}
/// The native launcher against the frozen transcript of the reference launcher it replaced:
/// exit/stdout/stderr/argv/cwd/env/signals for every case. The one intended difference is the
/// non-persistent trust layer the native App Server argv carries for its own cwd.
fn golden_comparisons(root: &Path, binary: &Path, native: &Path) -> usize {
    let golden: Vec<Value> =
        serde_json::from_str(include_str!("fixtures/codex-launcher-golden.json")).unwrap();
    assert_eq!(golden.len(), 39);
    fs::create_dir_all(root.join("work space")).unwrap();
    let physical = fs::canonicalize(root.join("work space")).unwrap();
    let physical = physical.to_str().unwrap();
    assert!(!physical.contains('"') && !physical.contains('\\'));
    let trust = [
        json!("-c"),
        json!(format!(
            "projects={{\"{physical}\"={{trust_level=\"trusted\"}}}}"
        )),
    ];
    let _ = binary;
    for entry in &golden {
        fs::create_dir_all(root.join("work space")).unwrap();
        let case = entry["case"].as_str().unwrap();
        let argv = entry["argv"].as_array().unwrap();
        let (status, out, err, mut rows) = run(root, native, case, argv, &entry["locale"]);
        for row in &mut rows {
            // The service bounds every child's build parallelism to half the CPUs
            // (`launch::default_build_parallelism`); the value depends on the host.
            if let Some(env) = row["env"].as_object_mut() {
                for key in ["CARGO_BUILD_JOBS", "CMAKE_BUILD_PARALLEL_LEVEL"] {
                    if let Some(jobs) = env.remove(key) {
                        assert!(
                            jobs.as_str()
                                .and_then(|j| j.parse::<u32>().ok())
                                .is_some_and(|j| j >= 2),
                            "{key}={jobs}"
                        );
                    }
                }
            }
            let args = row["argv"].as_array_mut().unwrap();
            if args.last().unwrap() == "app-server" {
                let n = args.len();
                assert_eq!(
                    args[n - 3..n - 1],
                    trust,
                    "native App Server trust layer missing"
                );
                args.drain(n - 3..n - 1);
            }
        }
        let what = format!("{case} {argv:?} {}", entry["locale"]);
        assert_eq!(json!(status), entry["status"], "{what}: exit status");
        assert_eq!(hex(&out), entry["out"].as_str().unwrap(), "{what}: stdout");
        assert_eq!(hex(&err), entry["err"].as_str().unwrap(), "{what}: stderr");
        assert_eq!(normalized(root, &rows), entry["rows"], "{what}: transcript");
    }
    golden.len()
}
/// A session's MCP relays reach App Server as `-c` layers beside the layers that turn the owner's
/// configured servers off: codex-acp then sends no `mcp_servers` table that could replace them.
/// The hand-over variable never reaches App Server; a malformed, colliding or unhonoured relay
/// refuses the launch.
fn session_relays(root: &Path, native: &Path) {
    const KEY: &str = "CORTEX_CODEX_MCP_SERVERS";
    let relays = json!([
        {"name":"cortex","command":"/opt/cortex relay","args":["/proc/1/fd/3/cortex-mcp-0/broker.sock","quote\"d\\"],"env":[]},
        {"name":"codebase-memory-mcp","command":"/opt/relay","args":[],"env":[{"name":"MODE","value":"relay\u{1}"}]}
    ])
    .to_string();
    let thread = cortex_engine::environment::codex_thread_config();
    let (status, _, err, rows) = run_with(
        root,
        native,
        "relays",
        &[json!("app-server")],
        &json!({}),
        &[(KEY, &relays), ("CODEX_CONFIG", &thread)],
    );
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&err));
    let peer = rows
        .iter()
        .find(|r| r["argv"].as_array().unwrap().last().unwrap() == "app-server")
        .unwrap();
    let argv: Vec<&str> = peer["argv"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a.as_str().unwrap())
        .collect();
    let layer = |text: &str| argv.windows(2).any(|w| w == ["-c", text]);
    assert!(layer("mcp_servers.z-9.enabled=false"), "{argv:?}");
    assert!(
        layer(
            r#"mcp_servers.cortex={command="/opt/cortex relay",args=["/proc/1/fd/3/cortex-mcp-0/broker.sock","quote\"d\\"],env={}}"#
        ),
        "{argv:?}"
    );
    assert!(
        layer(
            r#"mcp_servers.codebase-memory-mcp={command="/opt/relay",args=[],env={"MODE"="relay\u0001"}}"#
        ),
        "{argv:?}"
    );
    assert!(layer("features.plugins=false"), "{argv:?}");
    assert!(
        peer["env"].get(KEY).is_none(),
        "the relay list leaked into App Server's environment"
    );
    for row in &rows {
        assert!(
            row["env"].get(KEY).is_none(),
            "a Codex check saw the relay list"
        );
    }
    let refused_with = |case: &str, extra: &[(&str, &str)]| {
        let (status, out, err, rows) = run_with(
            root,
            native,
            case,
            &[json!("app-server")],
            &json!({}),
            extra,
        );
        assert_eq!(
            (status, out, err),
            (
                65,
                vec![],
                b"cortex codex native feature/MCP check failed\n".to_vec()
            ),
            "{case} {extra:?}"
        );
        assert!(rows
            .iter()
            .all(|r| r["argv"].as_array().unwrap().last().unwrap() != "app-server"));
    };
    let refused =
        |case: &str, value: &str| refused_with(case, &[(KEY, value), ("CODEX_CONFIG", &thread)]);
    refused("relay-dropped", &relays);
    // A session thread's `features` table replaces the App Server's `-c` feature
    // layers: a session without codex-acp's thread feature config (or with one that
    // turns a feature back on or adds anything) never starts.
    refused_with("relays", &[(KEY, &relays)]);
    for bad in [
        json!({"features":{"plugins":false}}).to_string(),
        thread.replace("\"plugins\":false", "\"plugins\":true"),
        thread.replace("\"plugins\":false", "\"plugins\":\"false\""),
        json!({"features":serde_json::from_str::<Value>(&thread).unwrap()["features"],"mcp_servers":{}}).to_string(),
        json!({"features":serde_json::from_str::<Value>(&thread).unwrap()["features"].as_object().unwrap().clone()
            .into_iter().chain([("memories".to_owned(), json!(false))]).collect::<serde_json::Map<_,_>>()}).to_string(),
        "not JSON".to_owned(),
    ] {
        refused_with("relays", &[(KEY, &relays), ("CODEX_CONFIG", &bad)]);
        // Outside a session the config may be absent, never wrong.
        refused_with("success", &[("CODEX_CONFIG", &bad)]);
    }
    let (status, _, err, _) = run_with(
        root,
        native,
        "success",
        &[json!("app-server")],
        &json!({}),
        &[("CODEX_CONFIG", &thread)],
    );
    assert_eq!(status, 0, "{}", String::from_utf8_lossy(&err));
    // A relay named like one of the owner's servers would inherit that entry's other fields.
    refused(
        "relays",
        &json!([{"name":"z-9","command":"/opt/relay","args":[],"env":[]}]).to_string(),
    );
    for bad in [
        "not JSON".to_owned(),
        json!({}).to_string(),
        json!([{"name":"x.y","command":"/opt/relay","args":[],"env":[]}]).to_string(),
        json!([{"name":"cortex","command":"relative","args":[],"env":[]}]).to_string(),
        json!([{"name":"cortex","command":"/opt/relay","args":[1],"env":[]}]).to_string(),
        json!([{"name":"cortex","command":"/opt/relay","args":[]}]).to_string(),
        json!([{"name":"a","command":"/opt/r","args":[],"env":[]},{"name":"a","command":"/opt/r","args":[],"env":[]}]).to_string(),
    ] {
        refused("relays", &bad);
    }
}
/// Native-only integrity fences reject unsafe on-disk artifacts.
fn integrity_fences(root: &Path, binary: &Path, native: &Path) {
    let _ = binary;
    let refused = || {
        let (status, out, err, rows) =
            run(root, native, "success", &[json!("app-server")], &json!({}));
        assert_eq!(
            (status, out, err, rows.len()),
            (
                65,
                vec![],
                b"cortex codex native feature/MCP check failed\n".to_vec(),
                0
            )
        );
    };
    let body = fs::read(native).unwrap();
    fs::write(native, [body.as_slice(), b"\n"].concat()).unwrap();
    refused();
    fs::write(native, &body).unwrap();
    fs::set_permissions(native, fs::Permissions::from_mode(0o750)).unwrap();
    refused();
    fs::set_permissions(native, fs::Permissions::from_mode(0o700)).unwrap();
    let alias: PathBuf = root.join("hardlink");
    fs::hard_link(native, &alias).unwrap();
    refused();
    fs::remove_file(&alias).unwrap();
    let target = root.join("native-target");
    fs::rename(native, &target).unwrap();
    std::os::unix::fs::symlink(&target, native).unwrap();
    refused();
    assert_eq!(fs::read(&target).unwrap(), body);
    fs::remove_file(native).unwrap();
    fs::rename(&target, native).unwrap();
}
fn main() {
    if std::env::var_os("CW_FAKE_CODEX").is_some() {
        std::process::exit(peer());
    }
    let root = std::env::temp_dir().join(format!("cw-codex-launch-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
    let result = std::panic::catch_unwind(|| {
        // Quotes/backslashes/spaces test JSON payloads, not shell quoting.
        let binary = root.join("fake peer-'\\-codex");
        fs::copy(std::env::current_exe().unwrap(), &binary).unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        for path in [
            "/path with space/service".to_owned(),
            format!("/{}", "x".repeat(250)),
            "relative".to_owned(),
        ] {
            assert!(
                workspace_service::native_codex_runtime::artifact(Path::new(&path), &binary)
                    .is_err()
            );
        }
        let artifact = root.join("native-runtime");
        let body = workspace_service::native_codex_runtime::artifact(
            Path::new(env!("CARGO_BIN_EXE_workspace-service")),
            &binary,
        )
        .unwrap();
        fs::write(&artifact, body).unwrap();
        fs::set_permissions(&artifact, fs::Permissions::from_mode(0o700)).unwrap();
        let count = golden_comparisons(&root, &binary, &artifact);
        integrity_fences(&root, &binary, &artifact);
        session_relays(&root, &artifact);
        println!("{count} native launcher execution comparisons passed; native PATH contains no interpreter");
    });
    fs::remove_dir_all(&root).unwrap();
    if let Err(error) = result {
        std::panic::resume_unwind(error);
    }
}
