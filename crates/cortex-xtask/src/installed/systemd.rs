//! No-paid real systemd lifecycle in one privileged, disposable container (dev channel).
//!
//! No host buses, cgroups, home, credentials or daemon sockets are mounted. The privilege is for
//! the container's PID 1 only; this is not hostile-code isolation.

use super::seed_large::rpc;
use super::util::*;
use super::Cli;
use crate::json::{parse, Res, Value};
use regex::Regex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PACKAGE: &str = "cortex-workspace-dev";
const UNIT: &str = "cortex-workspace-dev.service";

fn outcome(step: &str, details: Vec<(&str, Value)>) {
    let mut pairs = vec![("status", s("passed"))];
    pairs.extend(details);
    emit(step, pairs);
}

fn depends_names(archive: &Path) -> Res<Vec<String>> {
    let fields = run_ok(&["dpkg-deb", "-f", &archive.to_string_lossy(), "Depends"])?.stdout;
    let name = Regex::new(r"^[a-z0-9.+-]+").unwrap();
    fields
        .split(',')
        .map(|p| name.find(p.trim()).map(|m| m.as_str().to_string()).ok_or_else(|| format!("unparsable Depends entry {p:?}")))
        .collect()
}

pub fn host(cli: &Cli) -> Res<()> {
    let artifacts = cli.path("artifacts").ok_or("--artifacts is required")?.canonicalize().map_err(|e| e.to_string())?;
    let archive = std::fs::read_dir(&artifacts)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{PACKAGE}_")) && n.to_string_lossy().ends_with(".deb")))
        .ok_or("no dev archive in --artifacts")?;
    let mut names = depends_names(&archive)?;
    let prior = match cli.path("prior-archive") {
        Some(p) => {
            let p = p.canonicalize().map_err(|e| e.to_string())?;
            require(run_ok(&["dpkg-deb", "-f", &p.to_string_lossy(), "Package"])?.stdout.trim() == PACKAGE, "prior archive is not a dev archive")?;
            names.extend(depends_names(&p)?);
            Some(p)
        }
        None => None,
    };
    let names: BTreeSet<String> = names.into_iter().collect();
    let image = format!("cortex-systemd-check:{}", &sha_file(&archive)?[..16]);
    let harness = crate::proc::TempDir::new("systemd-harness-")?;
    std::fs::copy(std::env::current_exe().map_err(|e| e.to_string())?, harness.0.join("cortex-xtask")).map_err(|e| e.to_string())?;
    chmod(&harness.0.join("cortex-xtask"), 0o755)?;
    chmod(&harness.0, 0o755)?;
    {
        let context = crate::proc::TempDir::new("systemd-")?;
        let packages: Vec<&str> = names.iter().map(String::as_str).collect();
        write(
            &context.0.join("Dockerfile"),
            &format!(
                "FROM {BASE}\nENV container=docker\nRUN apt-get update && apt-get install -y --no-install-recommends systemd-sysv dbus-user-session xvfb {} && rm -rf /var/lib/apt/lists/*\nSTOPSIGNAL SIGRTMIN+3\nCMD [\"/sbin/init\"]\n",
                packages.join(" ")
            ),
        )?;
        run(&["docker", "build", "-q", "-t", &image, &context.0.to_string_lossy()], Duration::from_secs(1800))?;
    }
    let mut argv: Vec<String> = ["docker", "run", "--detach", "--privileged", "--cgroupns=private", "--network=none", "--tmpfs", "/run", "--tmpfs", "/run/lock", "--tmpfs", "/tmp"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    argv.extend(["--mount".into(), format!("type=bind,source={},target=/archives,readonly", artifacts.display())]);
    argv.extend(["--mount".into(), format!("type=bind,source={},target=/harness,readonly", harness.0.display())]);
    if let Some(p) = &prior {
        argv.extend(["--mount".into(), format!("type=bind,source={},target=/prior/owner.deb,readonly", p.display())]);
    }
    argv.push(image);
    let refs: Vec<&str> = argv.iter().map(String::as_str).collect();
    let id = run_ok(&refs)?.stdout.trim().to_string();
    let result = (|| -> Res<()> {
        wait("systemd_not_ready", || {
            let c = capture(Command::new("docker").args(["exec", &id, "systemctl", "is-system-running"]), Duration::from_secs(10))?;
            Ok(["running", "degraded"].contains(&c.stdout.trim()))
        })?;
        crate::proc::run(Command::new("docker").args(["exec", &id, "/harness/cortex-xtask", "check-systemd-inside"]), Duration::from_secs(900))
    })();
    let removed = run_ok(&["docker", "rm", "--force", &id]);
    result?;
    removed?;
    outcome("owned_systemd_container_removed", vec![]);
    Ok(())
}

/// One service request as the disposable desktop user; prints the whole reply.
pub fn rpc_client(method: &str, params: &str) -> Res<()> {
    let socket = Path::new(&std::env::var("XDG_RUNTIME_DIR").map_err(|_| "XDG_RUNTIME_DIR unset")?).join(PACKAGE).join("service.sock");
    let params = parse(params)?;
    match rpc(&socket, method, params) {
        Ok(result) => println!("{}", line(&obj(vec![("id", s("systemd")), ("result", result)]))),
        Err(e) => println!("{}", line(&obj(vec![("id", s("systemd")), ("error", s(e))]))),
    }
    Ok(())
}

pub fn inside() -> Res<()> {
    let comm = std::fs::read_to_string("/proc/1/comm").unwrap_or_default();
    require(Path::new("/.dockerenv").exists() && comm.trim() == "systemd" && unsafe { libc::getuid() } == 0, "internal mode requires a disposable systemd container")?;
    let archive = std::fs::read_dir("/archives")
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{PACKAGE}_")) && n.to_string_lossy().ends_with(".deb")))
        .ok_or("archive missing")?;
    let archive_text = archive.to_string_lossy().into_owned();
    run_ok(&["useradd", "--create-home", "--shell", "/bin/sh", "smoke"])?;
    let uid = run_ok(&["id", "-u", "smoke"])?.stdout.trim().to_string();
    let home = Path::new("/home/smoke");
    let runtime = PathBuf::from(format!("/run/user/{uid}"));
    let user = |args: &[&str]| -> Vec<String> {
        let mut v: Vec<String> = ["runuser", "-u", "smoke", "--", "env"].iter().map(|s| s.to_string()).collect();
        v.push(format!("HOME={}", home.display()));
        v.push(format!("XDG_RUNTIME_DIR={}", runtime.display()));
        v.push(format!("DBUS_SESSION_BUS_ADDRESS=unix:path={}", runtime.join("bus").display()));
        v.extend(args.iter().map(|s| s.to_string()));
        v
    };
    let ctl = |args: &[&str]| -> Res<Captured> {
        let mut a = vec!["systemctl", "--user"];
        a.extend(args);
        let c = user(&a);
        capture(Command::new(&c[0]).args(&c[1..]), Duration::from_secs(10))
    };
    let active = || -> bool { ctl(&["is-active", "--quiet", UNIT]).map(|c| c.code == 0).unwrap_or(false) };
    let pid = || -> Res<i64> {
        let r = ctl(&["show", "--property=MainPID", "--value", UNIT])?;
        require(r.code == 0, "MainPID unavailable")?;
        r.stdout.trim().parse().map_err(|_| "MainPID is not numeric".to_string())
    };
    let wait_for = |label: &str, mut test: Box<dyn FnMut() -> bool + '_>| -> Res<()> {
        let end = Instant::now() + Duration::from_secs(30);
        while Instant::now() < end {
            if test() {
                return Ok(());
            }
            sleep_ms(200);
        }
        Err(label.to_string())
    };
    let install = |path: &str, upgrade: bool, reinstall: bool| -> Res<()> {
        let mut c = Command::new("apt");
        c.args(["install", "--yes"]);
        if reinstall {
            c.arg("--reinstall");
        }
        c.arg(path);
        let r = capture(&mut c, Duration::from_secs(300))?;
        print!("{}{}", r.stdout, r.stderr);
        require(r.code == 0, "apt_install_failed")?;
        if upgrade {
            require(!format!("{}{}", r.stdout, r.stderr).to_lowercase().contains("downgrad"), "unexpected_downgrade")?;
            require(Regex::new(r"Upgrading:\s+1").unwrap().is_match(&r.stdout), "apt_did_not_report_upgrade")?;
            outcome("plain_apt_install_is_upgrade", vec![("downgrade_warning", b(false))]);
        }
        Ok(())
    };
    let remove = || run_ok(&["dpkg", "--remove", PACKAGE]).map(|_| ());
    let request = |method: &str, params: Value| -> Res<Value> {
        let me = "/harness/cortex-xtask";
        let c = user(&[me, "check-systemd-rpc", method, &line(&params)]);
        let r = capture(Command::new(&c[0]).args(&c[1..]), Duration::from_secs(90))?;
        require(r.code == 0, &format!("rpc_failed: {method}: {}", tail(&r.stderr)))?;
        let reply = parse(&r.stdout)?;
        require(reply.get("id").and_then(Value::as_str) == Some("systemd") && reply.get("error").is_none(), &format!("{method} refused: {}", reply.get("error").map(line).unwrap_or_default()))?;
        reply.get("result").cloned().ok_or_else(|| "rpc reply without result".to_string())
    };
    let project_comes_back = |project: &str, after: &str| -> Res<()> {
        // Only reads. Nothing here asks the service to start the project again.
        let end = Instant::now() + Duration::from_secs(90);
        let mut last = Value::Null;
        while Instant::now() < end {
            if let Ok(status) = request("projects.status", obj(vec![("projectId", s(project))])) {
                if matches!(status.get("ready"), Some(Value::Bool(true))) {
                    outcome(&format!("activated_project_ready_{after}"), vec![("state", status.get("state").cloned().unwrap_or(Value::Null))]);
                    return Ok(());
                }
                last = status;
            }
            sleep_ms(500);
        }
        Err(format!("activated_project_not_ready_{after}: {}", line(&last)))
    };
    run_ok(&["loginctl", "enable-linger", "smoke"])?;
    wait_for("user_bus_missing", Box::new(|| runtime.join("bus").exists()))?;
    wait_for("user_default_target_missing", Box::new(|| ctl(&["is-active", "--quiet", "default.target"]).map(|c| c.code == 0).unwrap_or(false)))?;
    outcome("real_systemd_pid1_and_lingering_user", vec![]);
    install(&archive_text, false, false)?;
    let wants = Path::new("/usr/lib/systemd/user/default.target.wants").join(UNIT);
    require(std::fs::symlink_metadata(&wants).map(|m| m.file_type().is_symlink()).unwrap_or(false) && wants.canonicalize().map(|p| p.is_file()).unwrap_or(false), "default.target.wants link missing")?;
    outcome("clean_install_with_default_wants", vec![]);
    run_ok(&["systemctl", "restart", &format!("user@{uid}.service")])?;
    wait_for("login_default_target_did_not_start_unit", Box::new(|| active()))?;
    require(pid()? > 0, "no service process")?;
    let socket_path = runtime.join(PACKAGE).join("service.sock");
    let is_socket = |p: &Path| std::os::unix::fs::FileTypeExt::is_socket(&std::fs::metadata(p).map(|m| m.file_type()).unwrap_or_else(|_| std::fs::metadata("/").unwrap().file_type()));
    wait_for("login_service_socket_missing", Box::new(|| is_socket(&socket_path)))?;
    outcome("login_time_default_target_start", vec![]);

    require(ctl(&["stop", UNIT])?.code == 0, "stop failed")?;
    let mut display = Command::new("Xvfb").args([":99", "-screen", "0", "1280x800x24", "-nolisten", "tcp"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?;
    let mut app: Option<Child> = None;
    let stage1 = (|| -> Res<()> {
        wait_for("display_not_ready", Box::new(|| is_socket(Path::new("/tmp/.X11-unix/X99"))))?;
        let c = user(&["env", "DISPLAY=:99", "WEBKIT_DISABLE_COMPOSITING_MODE=1", &format!("/usr/bin/{PACKAGE}")]);
        app = Some(Command::new(&c[0]).args(&c[1..]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|e| e.to_string())?);
        wait_for("desktop_did_not_start_unit", Box::new(|| active()))?;
        wait_for("app_service_socket_missing", Box::new(|| is_socket(&socket_path)))?;
        require(app.as_mut().unwrap().try_wait().map_err(|e| e.to_string())?.is_none() && pid()? > 0, "desktop exited early")?;
        outcome("app_driven_start_in_running_session", vec![]);
        Ok(())
    })();
    for child in [app.as_mut(), Some(&mut display)].into_iter().flatten() {
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end && child.try_wait().ok().flatten().is_none() {
            sleep_ms(50);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
    stage1?;

    let before = pid()?;
    install(&archive_text, false, true)?;
    wait_for("upgrade_did_not_restart_running_unit", Box::new(|| active() && pid().unwrap_or(before) != before))?;
    outcome("active_manager_upgrade_restart", vec![]);
    let sentinel = home.join("preserved-data");
    write(&sentinel, "preserve")?;
    remove()?;
    wait_for("removal_did_not_stop_unit", Box::new(|| !active() && !socket_path.exists()))?;
    require(read_to_string(&sentinel)? == "preserve", "removal changed preserved data")?;
    outcome("active_manager_removal_stop_preserves_data", vec![]);

    // Reproduce the owner's earlier two-binary archive shape, with no runtime, maintainer
    // scripts or default.target.wants link. This is not the original archive.
    let stage = Path::new("/tmp/legacy");
    run_ok(&["dpkg-deb", "-R", &archive_text, "/tmp/legacy"])?;
    std::fs::remove_dir_all(Path::new("/tmp/legacy/usr/lib").join(PACKAGE)).map_err(|e| e.to_string())?;
    std::fs::remove_file(stage.join("usr/lib/systemd/user/default.target.wants").join(UNIT)).map_err(|e| e.to_string())?;
    for script in ["postinst", "prerm"] {
        std::fs::remove_file(stage.join("DEBIAN").join(script)).map_err(|e| e.to_string())?;
    }
    let control = stage.join("DEBIAN/control");
    let body = read_to_string(&control)?;
    let rewritten: Vec<String> = body.lines().map(|l| if l.starts_with("Version:") { "Version: 1.0.6+sha256.f3eee9da173725e1".to_string() } else { l.to_string() }).collect();
    write(&control, &(rewritten.join("\n") + "\n"))?;
    run_ok(&["dpkg-deb", "--build", "/tmp/legacy", "/tmp/legacy.deb"])?;
    let prior = Path::new("/prior/owner.deb");
    let have_prior = prior.is_file();
    install(if have_prior { "/prior/owner.deb" } else { "/tmp/legacy.deb" }, false, false)?;
    let mut prior_pid = None;
    let mut project_id = String::new();
    if have_prior {
        require(ctl(&["daemon-reload"])?.code == 0, "daemon-reload failed")?;
        require(ctl(&["start", UNIT])?.code == 0, "start failed")?;
        wait_for("prior_archive_unit_inactive", Box::new(|| active()))?;
        wait_for("prior_archive_socket_missing", Box::new(|| is_socket(&socket_path)))?;
        prior_pid = Some(pid()?);
        // The owner's condition: a project activated by the earlier build, then a plain upgrade.
        let upgraded = home.join("upgraded-project");
        std::fs::create_dir(&upgraded).map_err(|e| e.to_string())?;
        run_ok(&["chown", "smoke:smoke", &upgraded.to_string_lossy()])?;
        let registered = request("projects.register", obj(vec![("root", s(upgraded.to_string_lossy())), ("displayName", s("Upgraded"))]))?;
        project_id = registered.get("projectId").and_then(Value::as_str).ok_or("no project identity")?.to_string();
        request("projects.activate", obj(vec![("projectId", s(&project_id))]))?;
        outcome("prior_archive_project_activated", vec![]);
    }
    install(&archive_text, true, false)?;
    require(ctl(&["start", UNIT])?.code == 0, "start failed")?;
    wait_for("legacy_upgrade_unit_inactive", Box::new(|| active()))?;
    if let Some(p) = prior_pid {
        wait_for("exact_archive_upgrade_did_not_restart", Box::new(move || pid().unwrap_or(p) != p))?;
    }
    wait_for("legacy_upgrade_socket_missing", Box::new(|| is_socket(&socket_path)))?;
    if prior_pid.is_some() {
        project_comes_back(&project_id, "after_upgrade")?;
        // The unit's crash-loop guard allows five starts a minute; this harness alone made more.
        // A restart or login is a later, separate owner event, so the counter starts afresh.
        require(ctl(&["reset-failed", UNIT])?.code == 0, "reset-failed failed")?;
        let before = pid()?;
        let restarted = ctl(&["restart", UNIT])?;
        require(restarted.code == 0, &format!("unit_restart_failed: {}", tail(&restarted.stderr)))?;
        wait_for("service_restart_did_not_restart", Box::new(|| active() && pid().unwrap_or(before) != before))?;
        project_comes_back(&project_id, "after_service_restart")?;
        require(ctl(&["reset-failed", UNIT])?.code == 0, "reset-failed failed")?;
        let before = pid()?;
        run_ok(&["systemctl", "restart", &format!("user@{uid}.service")])?;
        wait_for("login_did_not_start_unit", Box::new(|| runtime.join("bus").exists() && active() && pid().unwrap_or(before) != before))?;
        project_comes_back(&project_id, "after_login")?;
    }
    require(Path::new("/usr/lib").join(PACKAGE).join("runtime/runtime-payload.json").is_file(), "bundled runtime missing after upgrade")?;
    require(read_to_string(&sentinel)? == "preserve", "upgrade changed preserved data")?;
    outcome(
        if have_prior { "owner_archive_upgrade" } else { "owner_version_legacy_shape_upgrade" },
        vec![
            ("exact_original_archive", b(have_prior)),
            ("running_prior_unit_restarted", b(prior_pid.is_some())),
            ("prior_sha256", if have_prior { s(sha_file(prior)?) } else { Value::Null }),
        ],
    );
    remove()?;
    wait_for("final_removal_unit_active", Box::new(|| !active()))?;
    outcome("systemd_lifecycle_complete", vec![("paid_calls", n(0))]);
    Ok(())
}
