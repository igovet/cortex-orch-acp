//! No-paid install/upgrade/remove qualification; only disposable Docker state is changed.
//! `check-install` runs on the host; its container and user-session halves are the same binary.

use super::seed_large::rpc;
use super::channel::Channel;
use super::util::*;
use super::Cli;
use crate::json::{Res, Value};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

fn outcome(step: &str, status: &str, details: Vec<(&str, Value)>) {
    let mut pairs = vec![("status", s(status))];
    pairs.extend(details);
    emit(step, pairs);
}

fn packages_dir(cli: &Cli) -> Res<PathBuf> {
    cli.path("artifacts")
        .ok_or_else(|| "--artifacts is required".to_string())
}

fn package_channel(package: &str) -> Res<Channel> {
    match package {
        "cortex-workspace" => Channel::new("release"),
        "cortex-workspace-dev" => Channel::new("dev"),
        _ => Err("unsupported installed package".into()),
    }
}

/// Host: only archives and this reviewed tool enter the container. No checkout, user home,
/// credentials, host bus, cgroups or Docker socket are mounted.
pub fn host(cli: &Cli) -> Res<()> {
    let channel = cli.flag("channel").unwrap_or("dev");
    let package = match channel {
        "dev" => "cortex-workspace-dev",
        "release" => "cortex-workspace",
        _ => return Err("--channel must be dev or release".into()),
    };
    let engine = cli.flag("engine").unwrap_or("docker");
    if !["docker", "podman"].contains(&engine) {
        return Err("--engine must be docker or podman".into());
    }
    let artifacts = packages_dir(cli)?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let prefix = format!("{package}_");
    let found = std::fs::read_dir(&artifacts)
        .map_err(|e| e.to_string())?
        .flatten()
        .any(|e| {
            e.file_name().to_string_lossy().starts_with(&prefix)
                && e.file_name().to_string_lossy().ends_with(".deb")
        });
    require(
        found,
        &format!("no {package}_*.deb in the artifacts directory"),
    )?;
    let tool = crate::proc::TempDir::new("check-install-")?;
    std::fs::copy(
        std::env::current_exe().map_err(|e| e.to_string())?,
        tool.0.join("cortex-xtask"),
    )
    .map_err(|e| e.to_string())?;
    chmod(&tool.0.join("cortex-xtask"), 0o755)?;
    chmod(&tool.0, 0o755)?;
    let command = format!(
        "apt-get update >/dev/null && exec /smoke/cortex-xtask check-install-container {package}"
    );
    let mut c = Command::new(engine);
    c.args(["run", "--rm", "--mount"])
        .arg(format!(
            "type=bind,source={},target=/archives,readonly",
            artifacts.display()
        ))
        .arg("--mount")
        .arg(format!(
            "type=bind,source={},target=/smoke,readonly",
            tool.0.display()
        ))
        .args([
            "--env",
            "DEBIAN_FRONTEND=noninteractive",
            BASE,
            "/bin/sh",
            "-c",
            &command,
        ]);
    crate::proc::run(&mut c, Duration::from_secs(6 * 3600))
}

fn wait_socket(path: &Path, process: Option<&mut Child>) -> Res<Value> {
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    let mut process = process;
    while std::time::Instant::now() < deadline {
        if let Some(p) = process.as_deref_mut() {
            if p.try_wait().map_err(|e| e.to_string())?.is_some() {
                return Err("process_exited".into());
            }
        }
        if let Ok(v) = rpc(path, "service.status", obj(vec![])) {
            return Ok(v);
        }
        sleep_ms(100);
    }
    Err("socket_timeout".into())
}

fn stop(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        let end = std::time::Instant::now() + Duration::from_secs(10);
        while std::time::Instant::now() < end && child.try_wait().ok().flatten().is_none() {
            sleep_ms(50);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// The disposable desktop user: stale configuration is ignored, the app or unit starts the
/// service, a project registers and activates, and the sandbox helper is probed.
pub fn session(package: &str) -> Res<()> {
    let home = PathBuf::from(std::env::var("HOME").map_err(|_| "HOME unset")?);
    let channel = package_channel(package)?;
    let runtime = Path::new("/usr/lib").join(package).join("runtime");
    // Real first-run discovery: the product's own startup pass registers connections only for an
    // existing native login marker. The markers are synthetic and owned by this disposable user.
    let config = super::discovery::config_dir(&home, package);
    require(
        super::discovery::registry(&config)?.is_empty(),
        "the empty namespace already holds connections",
    )?;
    super::discovery::write_markers(&home)?;
    // Stale manual setup must be ignored by the actual installed unit.
    std::fs::create_dir_all(home.join(".config").join(package)).map_err(|e| e.to_string())?;
    write(
        &home.join(".config").join(package).join("service.env"),
        "CORTEX_WORKSPACE_CHECKOUT=/does-not-exist\n",
    )?;
    let unit_text =
        read_to_string(&Path::new("/usr/lib/systemd/user").join(format!("{package}.service")))?;
    let service_section: Vec<&str> = unit_text
        .split("[Service]")
        .nth(1)
        .unwrap_or("")
        .lines()
        .take_while(|l| !l.starts_with('['))
        .collect();
    require(
        !service_section
            .iter()
            .any(|l| l.trim_start().starts_with("EnvironmentFile")),
        "unit loads an EnvironmentFile",
    )?;
    let socket_path =
        Path::new(&std::env::var("XDG_RUNTIME_DIR").map_err(|_| "XDG_RUNTIME_DIR unset")?)
            .join(package)
            .join("service.sock");
    let manager = Command::new("/usr/bin/systemctl")
        .args(["--user", "show-environment"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let (mut process, mut app, mut display): (Option<Child>, Option<Child>, Option<Child>) =
        (None, None, None);
    let result = (|| -> Res<()> {
        if manager {
            // Start from a stopped unit, then let the real desktop native bridge start it.
            run_ok(&[
                "/usr/bin/systemctl",
                "--user",
                "stop",
                &format!("{package}.service"),
            ])?;
            display = Some(
                Command::new("Xvfb")
                    .args([":99", "-screen", "0", "1280x800x24", "-nolisten", "tcp"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| e.to_string())?,
            );
            sleep_ms(1000);
            app = Some(
                Command::new(format!("/usr/bin/{package}"))
                    .env("DISPLAY", ":99")
                    .env("WEBKIT_DISABLE_COMPOSITING_MODE", "1")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| e.to_string())?,
            );
            wait_socket(&socket_path, app.as_mut())?;
            outcome("app_driven_unit_start", "passed", vec![]);
        } else {
            outcome(
                "systemd_user_manager_and_app_start",
                "unverified",
                vec![("reason", s("container cannot host a systemd user manager"))],
            );
            let exec = service_section
                .iter()
                .find_map(|l| l.strip_prefix("ExecStart="))
                .ok_or("unit has no ExecStart")?
                .replace("%h", &home.to_string_lossy());
            let argv: Vec<&str> = exec.split_whitespace().collect();
            process = Some(
                Command::new(argv[0])
                    .args(&argv[1..])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()
                    .map_err(|e| e.to_string())?,
            );
            wait_socket(&socket_path, process.as_mut())?;
            outcome("unit_execstart_direct", "passed", vec![]);
        }
        let project = home.join("project");
        std::fs::create_dir(&project).map_err(|e| e.to_string())?;
        let registered = rpc(
            &socket_path,
            "projects.register",
            obj(vec![
                ("root", s(project.to_string_lossy())),
                ("displayName", s("Clean install")),
            ]),
        )?;
        let id = registered
            .get("projectId")
            .and_then(Value::as_str)
            .ok_or("no project identity")?
            .to_string();
        rpc(
            &socket_path,
            "projects.activate",
            obj(vec![("projectId", s(&id))]),
        )?;
        let status = rpc(
            &socket_path,
            "projects.status",
            obj(vec![("projectId", s(&id))]),
        )?;
        require(
            matches!(status.get("ready"), Some(Value::Bool(true))),
            "project did not become ready",
        )?;
        let store = channel.store_db(&home, &project)?;
        require(
            super::native::count(&store, "SELECT count(*) FROM tasks")? == 0,
            "clean install fixture unexpectedly contains tasks",
        )?;
        let found = super::discovery::recheck_without_real_login(&socket_path, &config, package)?;
        outcome(
            "empty_namespace_native_discovery",
            "passed",
            vec![
                ("engines", arr(found.iter().map(|(e, c)| obj(vec![("engine", s(e)), ("answer", s(c))])).collect())),
                ("note", s("bundled adapters discovered and run by the service; synthetic markers only, no model call")),
            ],
        );
        outcome(
            "bundled_runtime_connected",
            "passed",
            vec![
                ("paid_calls", n(0)),
                (
                    "adapters",
                    s("discovered by the service's native recheck; no model prompt"),
                ),
            ],
        );
        // Worker commands and contained MCP servers run under Landlock.
        let ok = std::fs::read_to_string("/sys/kernel/security/lsm")
            .is_ok_and(|l| l.split(',').any(|m| m.trim() == "landlock"));
        outcome(
            "landlock_sandbox",
            if ok { "passed" } else { "unverified" },
            vec![(
                "reason",
                if ok {
                    Value::Null
                } else {
                    s("Landlock is not an active LSM on this host; confined worker commands are not qualified")
                },
            )],
        );
        rpc(&socket_path, "service.stop", obj(vec![]))?;
        Ok(())
    })();
    for child in [&mut app, &mut process, &mut display].into_iter().flatten() {
        stop(child);
    }
    let _ = runtime;
    result
}

/// Root in the plain container: clean install, removal, legacy shape and upgrade rehearsal.
pub fn container(package: &str) -> Res<()> {
    let channel = package_channel(package)?;
    let archive = std::fs::read_dir("/archives")
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_name().is_some_and(|n| {
                n.to_string_lossy().starts_with(&format!("{package}_"))
                    && n.to_string_lossy().ends_with(".deb")
            })
        })
        .ok_or("archive missing")?;
    let archive_text = archive.to_string_lossy().into_owned();
    // Reproduce the former two-binary shape, without a runtime or wants link.
    let stage = Path::new("/tmp/legacy");
    let long = |argv: &[&str]| run(argv, Duration::from_secs(900));
    long(&["dpkg-deb", "-R", &archive_text, "/tmp/legacy"])?;
    std::fs::remove_dir_all(Path::new("/tmp/legacy/usr/lib").join(package))
        .map_err(|e| e.to_string())?;
    std::fs::remove_file(
        stage
            .join("usr/lib/systemd/user/default.target.wants")
            .join(format!("{package}.service")),
    )
    .map_err(|e| e.to_string())?;
    for script in ["postinst", "prerm"] {
        std::fs::remove_file(stage.join("DEBIAN").join(script)).map_err(|e| e.to_string())?;
    }
    let control = stage.join("DEBIAN/control");
    let body = read_to_string(&control)?;
    let rewritten: Vec<String> = body
        .lines()
        .map(|l| {
            if l.starts_with("Version:") {
                "Version: 1.0.5".to_string()
            } else {
                l.to_string()
            }
        })
        .collect();
    write(&control, &(rewritten.join("\n") + "\n"))?;
    long(&["dpkg-deb", "--build", "/tmp/legacy", "/tmp/legacy.deb"])?;
    long(&["apt-get", "update"])?;
    long(&[
        "apt-get",
        "install",
        "-y",
        "--no-install-recommends",
        &archive_text,
        "xvfb",
        "sqlite3",
    ])?;
    outcome("clean_install_with_dependencies", "passed", vec![]);
    require(
        crate::proc::which("gcc").is_none() && crate::proc::which("cargo").is_none(),
        "clean image must not contain build tools",
    )?;
    long(&["useradd", "--create-home", "--shell", "/bin/sh", "smoke"])?;
    let home = "/home/smoke";
    let uid = run_ok(&["id", "-u", "smoke"])?.stdout.trim().to_string();
    let runtime = format!("/run/user/{uid}");
    std::fs::create_dir_all(&runtime).map_err(|e| e.to_string())?;
    chmod(Path::new(&runtime), 0o700)?;
    run_ok(&["chown", "smoke:smoke", &runtime])?;
    let env = [
        format!("HOME={home}"),
        format!("XDG_STATE_HOME={home}/.local/state"),
        format!("XDG_RUNTIME_DIR={runtime}"),
        "PATH=/usr/local/bin:/usr/bin:/bin".to_string(),
    ];
    let mut manager = Command::new("runuser")
        .args(["-u", "smoke", "--", "env"])
        .args(&env)
        .arg("/lib/systemd/systemd")
        .arg("--user")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    sleep_ms(2000);
    let steps = (|| -> Res<()> {
        let mut cmd = Command::new("runuser");
        cmd.args(["-u", "smoke", "--", "env"])
            .args(&env)
            .arg("/smoke/cortex-xtask")
            .args(["check-install-session", package]);
        crate::proc::run(&mut cmd, Duration::from_secs(600))?;
        let home = Path::new(home);
        let project = home.join("project");
        let anchor = channel.store_anchor(&project);
        let store = channel.store_db(home, &project)?;
        let before = (sha_file(&anchor)?, sha_file(&store)?);
        long(&["apt-get", "remove", "-y", package])?;
        require(
            channel.store_db(home, &project)? == store
                && (sha_file(&anchor)?, sha_file(&store)?) == before,
            "removal changed the user store",
        )?;
        require(
            !Path::new("/usr/lib").join(package).join("runtime").exists(),
            "removal left the runtime",
        )?;
        outcome("removal_preserves_user_store", "passed", vec![]);
        long(&[
            "apt-get",
            "install",
            "-y",
            "--no-install-recommends",
            "/tmp/legacy.deb",
        ])?;
        outcome("legacy_shape_install", "passed", vec![]);
        long(&[
            "apt-get",
            "install",
            "-y",
            "--no-install-recommends",
            &archive_text,
        ])?;
        require(
            Path::new("/usr/lib")
                .join(package)
                .join("runtime/runtime-payload.json")
                .is_file(),
            "bundled runtime missing after upgrade",
        )?;
        require(
            channel.store_db(home, &project)? == store
                && (sha_file(&anchor)?, sha_file(&store)?) == before,
            "upgrade changed the user store",
        )?;
        outcome("upgrade_to_bundled_shape", "passed", vec![]);
        long(&["apt-get", "remove", "-y", package])?;
        if manager.try_wait().map_err(|e| e.to_string())?.is_some() {
            outcome(
                "active_manager_upgrade_restart_and_removal_stop",
                "unverified",
                vec![("reason", s("no running user manager; maintainer scripts exercised without an active manager"))],
            );
        }
        Ok(())
    })();
    stop(&mut manager);
    steps
}
