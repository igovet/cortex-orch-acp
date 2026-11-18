//! Offline install + real Cortex launch/ACP initialize. Never sends a prompt or logs in.

use super::discovery::{self, config_dir};
use super::seed_large::rpc;
use super::util::*;
use super::Cli;
use crate::json::{parse, Res, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

fn depends_names(archive: &Path) -> Res<(String, Vec<String>)> {
    let fields = run_ok(&["dpkg-deb", "-f", &archive.to_string_lossy(), "Depends"])?.stdout;
    let re = regex::Regex::new(r"^[a-z0-9.+-]+").unwrap();
    let names = fields
        .split(',')
        .map(|p| {
            re.find(p.trim())
                .map(|m| m.as_str().to_string())
                .ok_or_else(|| "unparsable Depends".to_string())
        })
        .collect::<Res<Vec<_>>>()?;
    Ok((fields, names))
}

pub fn host(cli: &Cli) -> Res<()> {
    let archive = cli
        .path("archive")
        .ok_or("--archive is required")?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let package = run_ok(&["dpkg-deb", "-f", &archive.to_string_lossy(), "Package"])?
        .stdout
        .trim()
        .to_string();
    require(
        package == "cortex-workspace-dev" || package == "cortex-workspace",
        "unexpected package",
    )?;
    let (fields, names) = depends_names(&archive)?;
    require(
        !regex::Regex::new(r"\b(nodejs|npm)\b")
            .unwrap()
            .is_match(&fields),
        "archive depends on a system Node",
    )?;
    let context = crate::proc::TempDir::new("cortex-offline-install-")?;
    std::fs::copy(&archive, context.0.join("application.deb")).map_err(|e| e.to_string())?;
    std::fs::copy(
        std::env::current_exe().map_err(|e| e.to_string())?,
        context.0.join("cortex-xtask"),
    )
    .map_err(|e| e.to_string())?;
    chmod(&context.0.join("cortex-xtask"), 0o755)?;
    write(
        &context.0.join("Dockerfile"),
        &format!(
            "FROM {BASE}\nRUN apt-get update && apt-get install -y --no-install-recommends {} && rm -rf /var/lib/apt/lists/*\nCOPY application.deb /application.deb\nCOPY cortex-xtask /check\n",
            names.join(" ")
        ),
    )?;
    let tag = format!("cortex-offline-engines:{}", &sha_file(&archive)?[..16]);
    run(
        &[
            "docker",
            "build",
            "-q",
            "-t",
            &tag,
            &context.0.to_string_lossy(),
        ],
        Duration::from_secs(1800),
    )?;
    let command = format!(
        "dpkg -i /application.deb >/dev/null && useradd --create-home smoke && runuser -u smoke -- env HOME=/home/smoke PATH=/usr/bin:/bin /check check-bundled-engines-session {package}"
    );
    run_inherit(
        &[
            "docker",
            "run",
            "--rm",
            "--network",
            "none",
            "--entrypoint",
            "sh",
            &tag,
            "-c",
            &command,
        ],
        Duration::from_secs(600),
    )?;
    println!(
        "{}",
        line(&obj(vec![
            (
                "archive_bytes",
                n(std::fs::metadata(&archive)
                    .map_err(|e| e.to_string())?
                    .len() as i64)
            ),
            ("package", s(package))
        ]))
    );
    Ok(())
}

fn acp_initialize(argv: &[String], cwd: &Path) -> Res<()> {
    let mut child: Child = Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .env("CORTEX_ENGINE_UPDATE", "off")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("adapter launch: {e}"))?;
    let mut stdin = child.stdin.take().unwrap();
    let request = line(&obj(vec![
        ("jsonrpc", s("2.0")),
        ("id", n(1)),
        ("method", s("initialize")),
        (
            "params",
            obj(vec![
                ("protocolVersion", n(1)),
                ("clientCapabilities", obj(vec![])),
                (
                    "clientInfo",
                    obj(vec![
                        ("name", s("cortex-offline-install-check")),
                        ("version", s("1.0.6")),
                    ]),
                ),
            ]),
        ),
    ]));
    writeln!(stdin, "{request}").map_err(|e| e.to_string())?;
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for l in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(v) = parse(&l) {
                if v.get("id").and_then(Value::as_i64) == Some(1) {
                    let _ = tx.send(v);
                    return;
                }
            }
        }
    });
    let reply = rx.recv_timeout(Duration::from_secs(30));
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    let reply = reply.map_err(|_| "adapter did not answer initialize".to_string())?;
    require(
        reply.get("error").is_none()
            && reply
                .get("result")
                .and_then(|r| r.get("protocolVersion"))
                .and_then(Value::as_i64)
                == Some(1),
        "adapter initialize refused",
    )
}

fn direct_service(package: &str, home: &Path) -> Res<Child> {
    let unit =
        read_to_string(&Path::new("/usr/lib/systemd/user").join(format!("{package}.service")))?;
    let exec = unit
        .lines()
        .find_map(|l| l.strip_prefix("ExecStart="))
        .ok_or("unit has no ExecStart")?
        .replace("%h", &home.to_string_lossy());
    let argv: Vec<&str> = exec.split_whitespace().collect();
    Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())
}

fn stop(child: &mut Child, socket: &Path) {
    let _ = rpc(socket, "service.stop", obj(vec![]));
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end && child.try_wait().ok().flatten().is_none() {
        sleep_ms(100);
    }
    let _ = child.kill();
    let _ = child.wait();
}

pub fn session(package: &str) -> Res<()> {
    let home = PathBuf::from(std::env::var("HOME").map_err(|_| "HOME unset")?);
    let tree = Path::new("/usr/lib").join(package).join("engines");
    require(
        crate::proc::which("node").is_none() && crate::proc::which("npm").is_none(),
        "a system Node or npm is on PATH",
    )?;
    let manifest = parse(&read_to_string(&tree.join("bundle-manifest.json"))?)?;
    let files = manifest
        .get("files")
        .and_then(Value::as_obj)
        .ok_or("bundle manifest has no files")?
        .to_vec();
    let verify = |label: &str| -> Res<()> {
        for (relative, expected) in &files {
            let path = tree.join(relative);
            let meta = std::fs::metadata(&path).map_err(|e| format!("{relative}: {e}"))?;
            let c = std::ffi::CString::new(path.to_string_lossy().as_bytes()).unwrap();
            require(
                meta.uid() == 0 && unsafe { libc::access(c.as_ptr(), libc::W_OK) } != 0,
                &format!("{label}: bundle file is not root-owned and read-only: {relative}"),
            )?;
            require(
                Some(sha_file(&path)?.as_str()) == expected.as_str(),
                &format!("{label}: bundle file changed: {relative}"),
            )?;
        }
        Ok(())
    };
    verify("before")?;
    let workspace = home.join("project");
    std::fs::create_dir(&workspace).map_err(|e| e.to_string())?;
    let runtime = home.join("run").to_string_lossy().into_owned();
    std::fs::create_dir_all(&runtime).map_err(|e| e.to_string())?;
    chmod(Path::new(&runtime), 0o700)?;
    std::env::set_var("XDG_RUNTIME_DIR", &runtime);
    std::env::set_var("CORTEX_ENGINE_UPDATE", "off");
    let socket = Path::new(&runtime).join(package).join("service.sock");
    let config = config_dir(&home, package);
    // The service's own native engine check finds each qualified bundled adapter and its native
    // binary; synthetic markers are not a login, so it must refuse and register nothing.
    discovery::write_markers(&home)?;
    let mut svc = direct_service(package, &home)?;
    let probe = (|| -> Res<Vec<(String, String)>> {
        wait_with("service socket", 25, || {
            Ok(rpc(&socket, "service.status", obj(vec![])).ok().map(|_| ()))
        })?;
        discovery::recheck_without_real_login(&socket, &config, package)
    })();
    stop(&mut svc, &socket);
    let probe = probe?;
    emit(
        "native_engine_discovery",
        vec![
            (
                "engines",
                arr(probe
                    .iter()
                    .map(|(e, c)| obj(vec![("engine", s(e)), ("answer", s(c))]))
                    .collect()),
            ),
            ("real_account_authentication", s("unverified")),
            ("model_refresh", s("not_run")),
        ],
    );
    let required: Vec<String> = manifest
        .get("required")
        .and_then(Value::as_arr)
        .ok_or("manifest without required list")?
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    let find = |suffix: &str| -> Res<String> {
        required
            .iter()
            .find(|r| r.ends_with(suffix))
            .cloned()
            .ok_or_else(|| format!("manifest lists no {suffix}"))
    };
    let node = tree.join("node/bin/node");
    for (engine, adapter, native) in [
        (
            "codex-acp",
            find("@agentclientprotocol/codex-acp/dist/index.js")?,
            find("/bin/codex")?,
        ),
        (
            "claude-agent-acp",
            find("@agentclientprotocol/claude-agent-acp/dist/index.js")?,
            find("/claude")?,
        ),
    ] {
        let launch = vec![
            node.to_string_lossy().into_owned(),
            tree.join(&adapter).to_string_lossy().into_owned(),
        ];
        acp_initialize(&launch, &workspace)?;
        emit(
            "adapter_initialize",
            vec![
                ("engine", s(engine)),
                ("initialize", s("passed")),
                ("prompts_sent", n(0)),
                ("bundled_node", b(true)),
            ],
        );
        let version = capture(
            Command::new(tree.join(&native)).arg("--version"),
            Duration::from_secs(20),
        )?;
        require(
            version.code == 0,
            &format!("{engine}: native --version failed"),
        )?;
    }
    verify("after")?;
    emit(
        "root_owned_bundle_unchanged",
        vec![
            ("network", s("none")),
            ("native_version_checks", s("passed")),
        ],
    );
    Ok(())
}
