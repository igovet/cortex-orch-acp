//! The root half of one disposable scenario: install, preserve, launch phases, verify payload.

use super::channel::Channel;
use super::config;
use super::native;
use super::payload;
use super::util::*;
use crate::json::{Res, Value};
use regex::Regex;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

const HOME: &str = "/home/acceptance";

pub fn user_command(argv: &[&str]) -> Vec<String> {
    let mut v: Vec<String> = [
        "runuser",
        "-u",
        "acceptance",
        "--",
        "env",
        "HOME=/home/acceptance",
        "XDG_STATE_HOME=/home/acceptance/.local/state",
        "XDG_RUNTIME_DIR=/run/user/1001",
        "DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1001/bus",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    v.extend(argv.iter().map(|s| s.to_string()));
    v
}

fn refs(v: &[String]) -> Vec<&str> {
    v.iter().map(String::as_str).collect()
}

pub fn ctl(args: &[&str]) -> Res<String> {
    let mut argv = vec!["systemctl", "--user"];
    argv.extend(args);
    let cmd = user_command(&argv);
    let done = capture(
        std::process::Command::new(&cmd[0]).args(&cmd[1..]),
        Duration::from_secs(120),
    )?;
    if done.code != 0 {
        println!("{} {}", done.stdout, done.stderr);
        return Err(format!(
            "fixture systemctl {} failed (exit {})",
            args.join(" "),
            done.code
        ));
    }
    Ok(done.stdout.trim().to_string())
}

fn mkdir_private(path: &Path) -> Res<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// Three project identities, one with an unreadable store, plus a saved sprint.
fn seed(home: &Path, channel: &Channel) -> Res<PathBuf> {
    let state = channel.state(home);
    let project = format!("wp_{}1", "0".repeat(31));
    let folder = home.join("incident-project");
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    mkdir_private(&state.join("product"))?;
    chmod(&state, 0o700)?;
    let mut registry = vec![obj(vec![
        ("projectId", s(&project)),
        ("canonicalRoot", s(folder.to_string_lossy())),
        ("displayName", s("Incident fixture")),
        ("activation", Value::Null),
    ])];
    for (number, name) in [(2, "Other fixture"), (3, "Unavailable fixture")] {
        let root = home.join(format!("project-{number}"));
        std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        registry.push(obj(vec![
            ("projectId", s(format!("wp_{number:032x}"))),
            ("canonicalRoot", s(root.to_string_lossy())),
            ("displayName", s(name)),
            ("activation", Value::Null),
        ]));
    }
    // One registered project has an unreadable store. Its identity must still appear.
    let broken = home.join("project-3").join(&channel.store);
    mkdir_private(&broken)?;
    write(&broken.join("cortex.sqlite3"), "not a SQLite database")?;
    chmod(&broken.join("cortex.sqlite3"), 0o600)?;
    let registry_path = state.join("projects.json");
    write(&registry_path, &line(&arr(registry)))?;
    let sprint = obj(vec![
        ("id", s("sp_fixture")),
        ("projectId", s(&project)),
        ("description", s("Fixture sprint")),
        ("title", s("Fixture sprint")),
        ("runMode", s("automatic")),
        ("targetBranch", Value::Null),
        ("planning", s("created")),
        ("conversation", arr(vec![])),
        ("plan", Value::Null),
        ("materializedVersion", n(1)),
        ("taskIds", arr(vec![s("wt_missing_1"), s("wt_missing_2")])),
        ("queueState", s("idle")),
        ("pauseReason", Value::Null),
    ]);
    let product = state.join("product").join(format!("{project}.json"));
    write(
        &product,
        &line(&obj(vec![
            ("tasks", obj(vec![])),
            ("sprints", arr(vec![sprint])),
        ])),
    )?;
    chmod(&registry_path, 0o600)?;
    chmod(&product, 0o600)?;
    Ok(registry_path)
}

fn prepare_fixture_start(unit: &str) -> Res<()> {
    super::require_container(false)?;
    // These are deliberate test restarts, not crashes. Do not spend the product unit's
    // five-per-minute budget across unrelated fixture phases, or hide a failure.
    require(
        ctl(&["show", unit, "-p", "ActiveState", "--value"])? == "inactive",
        "fixture unit not stopped",
    )?;
    require(
        ctl(&["show", unit, "-p", "Result", "--value"])? == "success",
        "fixture unit did not stop cleanly",
    )?;
    ctl(&["reset-failed", unit])?;
    emit(
        "fixture_start_budget_reset",
        vec![("previous_result", s("success"))],
    );
    Ok(())
}

fn seed_large_board(home: &Path, channel: &Channel) -> Res<()> {
    let unit = &channel.unit;
    prepare_fixture_start(unit)?;
    let root = home.join("large-project");
    std::fs::create_dir(&root).map_err(|e| e.to_string())?;
    chown_tree(&root, "1001:1001")?;
    // The real service creates the tasks through its public workspace API; nothing is written
    // into the store behind its back until it has stopped.
    ctl(&["start", unit])?;
    wait_secs("service for large-board seeding", 60, || {
        Ok(ctl(&["is-active", unit])? == "active")
    })?;
    // The unit is Type=simple: active means started, not listening yet.
    wait_secs("service socket for large-board seeding", 60, || {
        Ok(channel.socket.exists())
    })?;
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let me = me.to_string_lossy().into_owned();
    let cmd = user_command(&[
        &me,
        "installed-seed-large",
        "--channel",
        &channel.name,
        "api",
    ]);
    crate::proc::run(
        std::process::Command::new(&cmd[0]).args(&cmd[1..]),
        Duration::from_secs(600),
    )?;
    ctl(&["stop", unit])?;
    prepare_fixture_start(unit)?;
    let cmd = user_command(&[
        &me,
        "installed-seed-large",
        "--channel",
        &channel.name,
        "store",
    ]);
    crate::proc::run(
        std::process::Command::new(&cmd[0]).args(&cmd[1..]),
        Duration::from_secs(600),
    )?;
    emit(
        "large_store_seeded",
        vec![
            ("tasks", n(68)),
            ("sprint_tasks", n(31)),
            ("history_rows", n(100000)),
        ],
    );
    Ok(())
}

fn install(path: &str) -> Res<Captured> {
    let done = capture(
        std::process::Command::new("apt").args(["install", "--yes", path]),
        Duration::from_secs(900),
    )?;
    println!("{}\n{}", done.stdout, done.stderr);
    if done.code != 0 {
        return Err(format!("apt failed (exit {})", done.code));
    }
    Ok(done)
}

fn git_commit(project: &Path, message: &str, file: &str) -> Res<()> {
    let p = project.to_string_lossy().into_owned();
    run_ok(&["git", "-C", &p, "init", "-q", "-b", "main"])?;
    run_ok(&["git", "-C", &p, "add", file])?;
    run_ok(&[
        "git",
        "-C",
        &p,
        "-c",
        "user.name=Acceptance",
        "-c",
        "user.email=acceptance@example.invalid",
        "commit",
        "-q",
        "-m",
        message,
    ])?;
    Ok(())
}

pub fn run(
    channel: &Channel,
    backend: &str,
    mode: &str,
    close_cycles: u32,
    python_allowed: bool,
    only: Option<&str>,
) -> Res<()> {
    super::require_container(false)?;
    let unit = channel.unit.clone();
    let package = channel.package.clone();
    let home = PathBuf::from(HOME);
    let me = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .to_string_lossy()
        .into_owned();
    let python_policy = |stage: &str| -> Res<()> {
        let present = capture(
            std::process::Command::new("sh").args(["-c", "command -v python3 >/dev/null 2>&1"]),
            Duration::from_secs(10),
        )?
        .code
            == 0;
        emit(
            "python_policy",
            vec![
                ("stage", s(stage)),
                ("present", b(present)),
                ("allowed_by_archive_depends", b(python_allowed)),
            ],
        );
        require(
            python_allowed || !present,
            &format!(
                "python3 is present in the gate container ({stage}) but no archive requires it"
            ),
        )
    };
    python_policy("image")?;
    run_ok(&[
        "useradd",
        "--uid",
        "1001",
        "--create-home",
        "--shell",
        "/bin/sh",
        "acceptance",
    ])?;
    wait("system bus", || {
        Ok(Path::new("/run/dbus/system_bus_socket").exists())
    })?;
    run_ok(&["loginctl", "enable-linger", "acceptance"])?;
    wait("lingering user bus", || {
        Ok(Path::new("/run/user/1001/bus").exists())
    })?;
    let expected = config::seed(&home, channel)?;
    chown_tree(&home, "1001:1001")?;
    let preserved = |stage: &str| -> Res<()> {
        emit(
            "orchestration_config_preserved",
            config::verify(&home, channel, &expected, stage)?,
        );
        Ok(())
    };
    preserved("seeded")?;
    let (mut before, mut registry, mut prior_hash) = (String::new(), PathBuf::new(), String::new());
    if mode == "upgrade" {
        install("/archives/prior.deb")?;
        preserved("prior-install")?;
        registry = seed(&home, channel)?;
        prior_hash = sha_file(&registry)?;
        chown_tree(&home, "1001:1001")?;
        run_ok(&["systemctl", "restart", "user@1001.service"])?;
        wait("prior unit", || Ok(ctl(&["is-active", &unit])? == "active"))?;
        before = ctl(&["show", "-p", "MainPID", "--value", &unit])?;
        preserved("before-upgrade")?;
    }
    let current = install("/archives/candidate.deb")?;
    preserved("candidate-install")?;
    python_policy("after-install")?;
    if mode == "upgrade" {
        let all = format!("{}{}", current.stdout, current.stderr).to_lowercase();
        require(!all.contains("downgrad"), "apt reported a downgrade")?;
        require(
            Regex::new(r"Upgrading:\s+1")
                .unwrap()
                .is_match(&current.stdout),
            "not an ordinary apt upgrade",
        )?;
        wait("upgrade restarted service", || {
            Ok(ctl(&["show", "-p", "MainPID", "--value", &unit])? != before)
        })?;
        require(
            sha_file(&registry)? == prior_hash,
            "upgrade changed the registered projects",
        )?;
        emit(
            "upgrade_registry_preserved",
            vec![("count", n(3)), ("sha256", s(&prior_hash))],
        );
    } else {
        // New lingering user's existing default.target predates package install.
        run_ok(&["systemctl", "restart", "user@1001.service"])?;
    }
    wait("installed unit", || {
        Ok(ctl(&["is-active", &unit])? == "active")
    })?;
    ctl(&["stop", &unit])?;
    let phase = |name: &str| -> Res<()> {
        let argv = user_command(&[
            &me,
            "installed-session",
            "--channel",
            &channel.name,
            "--close-cycles",
            &close_cycles.to_string(),
            backend,
            name,
        ]);
        let outcome = crate::proc::run(
            std::process::Command::new(&argv[0]).args(&argv[1..]),
            Duration::from_secs(600),
        );
        let project = home.join("journey");
        let marker = |f: &str| project.join(f).exists();
        let failed = outcome.is_err();
        if failed {
            let state = ctl(&[
                "show",
                &unit,
                "-p",
                "ActiveState",
                "-p",
                "SubState",
                "-p",
                "ExecMainStatus",
                "-p",
                "MainPID",
            ])
            .unwrap_or_default();
            emit("failed_service_state", vec![("state", s(state))]);
            let j = user_command(&["journalctl", "--user", "--unit", &unit, "--no-pager"]);
            if let Ok(d) = capture(
                std::process::Command::new(&j[0]).args(&j[1..]),
                Duration::from_secs(120),
            ) {
                let _ = std::fs::write("/evidence/failure-user-service.log", d.stdout);
            }
        }
        let fixture_error = std::fs::read_to_string(project.join("fixture-error")).ok();
        emit(
            "fake_markers",
            vec![
                ("phase", s(name)),
                ("answered", b(marker("answered"))),
                ("finish", b(marker("finish"))),
                ("fixture_error", opt_s(fixture_error)),
            ],
        );
        outcome?;
        require(
            ctl(&["is-active", &unit])? == "active",
            "service died when window closed",
        )?;
        emit(
            "service_survives_close",
            vec![("backend", s(backend)), ("phase", s(name))],
        );
        Ok(())
    };
    let save_journal = || -> Res<()> {
        let j = capture(
            std::process::Command::new("journalctl")
                .args([&format!("--user-unit={unit}"), "--no-pager"]),
            Duration::from_secs(120),
        )?;
        write(Path::new("/evidence/service.log"), &j.stdout)
    };
    if only.is_none() {
        // First GUI launch uses a fresh profile with the installed service stopped.
        phase("cold-start")?;
        preserved("first-launch")?;
        ctl(&["stop", &unit])?;
        let delay = home
            .join(".config/systemd/user")
            .join(format!("{unit}.d/startup-delay.conf"));
        let write_delay = || -> Res<()> {
            std::fs::create_dir_all(delay.parent().unwrap()).map_err(|e| e.to_string())?;
            write(&delay, "[Service]\nExecStartPre=/usr/bin/sleep 10\n")?;
            chown_tree(&home, "1001:1001")?;
            ctl(&["daemon-reload"])?;
            Ok(())
        };
        let drop_delay = || -> Res<()> {
            std::fs::remove_file(&delay).map_err(|e| e.to_string())?;
            ctl(&["daemon-reload"])?;
            Ok(())
        };
        write_delay()?;
        let outcome = phase("delayed-start");
        drop_delay()?;
        outcome?;
        ctl(&["stop", &unit])?;
        if mode == "clean" {
            seed(&home, channel)?;
            chown_tree(&home, "1001:1001")?;
        }
        ctl(&["start", &unit])?;
        let outcome = phase("incident");
        let journal = save_journal();
        outcome?;
        preserved("unreadable-project-launch")?;
        journal?;
        ctl(&["stop", &unit])?;
        // Preservation is established before switching to unrelated fake-engine fixtures.
        // Restore the empty test account config, never an owner configuration.
        for (name, _) in &expected {
            std::fs::remove_file(channel.config(&home).join(name)).map_err(|e| e.to_string())?;
        }
        emit(
            "orchestration_fixture_retired",
            vec![("channel", s(&channel.name))],
        );
        seed_large_board(&home, channel)?;
        phase("large-board")?;
        ctl(&["stop", &unit])?;
        prepare_fixture_start(&unit)?;
        write_delay()?;
        let outcome = phase("cached-board");
        drop_delay()?;
        outcome?;
        ctl(&["stop", &unit])?;
    }
    if only.is_some() {
        for (name, _) in &expected {
            std::fs::remove_file(channel.config(&home).join(name)).map_err(|e| e.to_string())?;
        }
    }
    prepare_fixture_start(&unit)?;
    std::fs::remove_dir_all(channel.state(&home)).map_err(|e| e.to_string())?;
    // Only fixture files in the disposable account are added. The package payload is never patched.
    // Both journeys run the shipped native host against the no-paid `native-fake-engine` peer; the
    // only service override is the allowed project root (the unit's own default is the filesystem root).
    let override_dir = home.join(".config/systemd/user").join(format!("{unit}.d"));
    std::fs::create_dir_all(&override_dir).map_err(|e| e.to_string())?;
    let exec = format!(
        "/usr/bin/{} {HOME} {}",
        channel.service,
        channel.runtime.display()
    );
    let fixture_conf = override_dir.join("fixture.conf");
    write(
        &fixture_conf,
        &format!("[Service]\nExecStart=\nExecStart={exec}\n"),
    )?;
    let journey = native::prepare(&home, channel, "journey", "reconcile-question")?;
    git_commit(&journey, "Disposable acceptance fixture", "work.txt")?;
    let native_project = native::prepare(&home, channel, "native-journey", "")?;
    git_commit(&native_project, "Disposable native fixture", "work.txt")?;
    chown_tree(&home, "1001:1001")?;
    ctl(&["daemon-reload"])?;
    ctl(&["start", &unit])?;
    if only != Some("native-journey") {
        let outcome = phase("journey");
        let journal = save_journal();
        outcome?;
        journal?;
        ctl(&["stop", &unit])?;
        prepare_fixture_start(&unit)?;
        ctl(&["start", &unit])?;
    }
    if only != Some("journey") {
        phase("native-journey")?;
    }
    // Compare directly: these archives do not carry a DEBIAN/md5sums file.
    let verified = payload::verify(Path::new("/archives/candidate.deb"), Path::new("/"))?;
    let report = obj(vec![
        ("files", n(verified.files)),
        ("links", n(verified.links)),
        (
            "mismatches",
            arr(verified.mismatches.iter().map(s).collect()),
        ),
    ]);
    write(
        Path::new("/evidence/payload.json"),
        &(crate::json::pretty_ascii(&report)?),
    )?;
    require(
        verified.files > 0 && verified.mismatches.is_empty(),
        &format!("installed payload differs: {}", line(&report)),
    )?;
    emit(
        "installed_payload_matches_archive",
        vec![
            ("files", n(verified.files)),
            ("links", n(verified.links)),
            ("mismatches", arr(vec![])),
        ],
    );
    emit(
        "installed_scenario_passed",
        vec![("backend", s(backend)), ("mode", s(mode))],
    );
    let _ = (&package, refs(&[]));
    Ok(())
}
