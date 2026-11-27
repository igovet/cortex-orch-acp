//! The container half of the rendered journey: a disposable account on a private session bus, the private display (X11 with Openbox or
//! headless Mutter), the native service with the no-paid peer, and the shipped desktop binary with the request-file probe.

use super::journey::*;
use super::pretty_line;
use super::session::*;
use crate::installed::channel::Channel;
use crate::installed::native;
use crate::installed::util::*;
use crate::installed::Cli;
use crate::json::{Res, Value};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub(super) const PROJECTS: [&str; 5] = ["journey", "second", "terminal", "voice", "attachments"];
const STAGES: [&str; 9] = [
    "common",
    "lifecycle",
    "baseline",
    "controls",
    "wayland",
    "terminal",
    "voice",
    "attachments",
    "all",
];

/// What each process really did, read before cleanup ends them.
pub(super) fn native_evidence(j: &mut J, error: &str) -> Value {
    let status = j.st.app_status();
    let alive = j.st.app.is_some() && status.is_none();
    let answers = if alive {
        Some(j.page_answers_now())
    } else {
        None
    };
    let unexpected = j.unexpected_terminations();
    let all = web_terminations(&j.st.probe_dir);
    let by_api = all.as_arr().map_or(0, |a| {
        a.iter()
            .filter(|r| r.as_str() == Some("terminated_by_api"))
            .count()
    });
    let logs = j.st.scratch.join("logs");
    let mut launched: Vec<PathBuf> = std::fs::read_dir(&logs)
        .map(|rd| {
            rd.flatten()
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("app-"))
                })
                .collect()
        })
        .unwrap_or_default();
    launched.sort();
    let service = j.st.service.as_mut().map(|c| c.try_wait().ok().flatten());
    let compositor =
        j.st.compositor
            .as_mut()
            .map(|c| c.try_wait().ok().flatten());
    obj(vec![
        (
            "failure_class",
            failure_class(
                j.st.app.is_some() && status.is_some(),
                unexpected.as_arr().is_some_and(|a| !a.is_empty()),
                compositor.as_ref().is_some_and(Option::is_some),
                error,
            ),
        ),
        (
            "app",
            if j.st.app.is_some() {
                exit_status(status)
            } else {
                Value::Null
            },
        ),
        (
            "app_processes",
            j.st.app
                .as_ref()
                .map_or_else(|| arr(vec![]), |a| owned_processes(a.id() as i32)),
        ),
        ("page_answers", answers.map_or(Value::Null, Value::Bool)),
        ("web_process_terminations", unexpected),
        ("requested_closes", n(j.st.closes as i64)),
        ("terminated_by_api", n(by_api as i64)),
        ("compositor", compositor.map_or(Value::Null, exit_status)),
        ("service", service.map_or(Value::Null, exit_status)),
        (
            "app_stderr",
            launched.last().map_or(Value::Null, |p| stderr_summary(p)),
        ),
        (
            "compositor_stderr",
            stderr_summary(&logs.join("compositor.stderr")),
        ),
        ("app_launches", n(launched.len() as i64)),
        (
            "largest_app_stderr_bytes",
            n(launched
                .iter()
                .filter_map(|p| p.metadata().ok())
                .map(|m| m.len() as i64)
                .max()
                .unwrap_or(0)),
        ),
        ("cores", core_files(&j.st.scratch.join("cores"))),
        (
            "core_limit",
            n(unsafe {
                let mut limit = libc::rlimit {
                    rlim_cur: 0,
                    rlim_max: 0,
                };
                libc::getrlimit(libc::RLIMIT_CORE, &mut limit);
                limit.rlim_cur as i64
            }),
        ),
    ])
}

/// The wrapper: a scratch tree and a private session bus, then the session below inside it.
pub fn inside(cli: &Cli) -> Res<i32> {
    super::baseline_requested(cli)?;
    super::peer_timing_requested(cli)?;
    if cli.flag("scratch").is_some() {
        return session(cli);
    }
    if let Some(only) = cli.flag("only") {
        require(
            STAGES.contains(&only),
            &format!("--only is one of {}", STAGES.join(", ")),
        )?;
    }
    let tmp = crate::proc::TempDir::new("cw-native-")?;
    // The account's own home, with the default config and state locations below it, as the installed gate's account has.
    for name in [
        "home",
        "home/.config",
        "home/.local",
        "home/.local/state",
        "home/.cache",
        "runtime",
    ] {
        std::fs::create_dir_all(tmp.0.join(name)).map_err(|e| e.to_string())?;
        chmod(&tmp.0.join(name), 0o700)?;
    }
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let home = tmp.0.join("home");
    let mut command = Command::new("dbus-run-session");
    command
        .env_clear()
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env("LANG", "C.UTF-8")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_RUNTIME_DIR", tmp.0.join("runtime"))
        .args(["--"])
        .arg(&me)
        .arg("check-rendered-inside")
        .arg("--artifacts")
        .arg(cli.flag("artifacts").unwrap_or("/artifacts"))
        .arg("--checkout")
        .arg(cli.flag("checkout").unwrap_or("/checkout"))
        .arg("--scratch")
        .arg(&tmp.0)
        .stderr(Stdio::null())
        .process_group(0);
    for name in ["only", "journey-wayland", "baseline", "peer-timing"] {
        if let Some(value) = cli.flag(name) {
            command.args([format!("--{name}"), value.to_string()]);
        }
    }
    let mut wrapper = command
        .spawn()
        .map_err(|e| format!("dbus-run-session: {e}"))?;
    let end = Instant::now() + Duration::from_secs(2400);
    loop {
        if let Some(status) = wrapper.try_wait().map_err(|e| e.to_string())? {
            return Ok(status.code().unwrap_or(1));
        }
        if Instant::now() > end {
            unsafe { libc::killpg(wrapper.id() as i32, libc::SIGKILL) };
            let _ = wrapper.wait();
            return Err("journey deadline exceeded".into());
        }
        sleep_ms(200);
    }
}

fn git_fixture(repository: &Path) -> Res<()> {
    write(
        &repository.join("README.md"),
        "# Journey\n\nA disposable project.\n",
    )?;
    write(&repository.join("a.txt"), "Disposable project notes.\n")?;
    for args in [
        vec!["init", "-q", "-b", "main"],
        vec!["add", "README.md", "work.txt"],
        vec![
            "-c",
            "user.name=Audit",
            "-c",
            "user.email=audit@example.invalid",
            "commit",
            "-q",
            "-m",
            "Journey history",
        ],
    ] {
        let mut command = Command::new("git");
        command.args(&args).current_dir(repository);
        let done = capture(&mut command, Duration::from_secs(30))?;
        require(
            done.code == 0,
            &format!("git fixture failed: {}", tail(&done.stderr)),
        )?;
    }
    Ok(())
}

pub(super) fn session(cli: &Cli) -> Res<i32> {
    let scratch = PathBuf::from(cli.flag("scratch").unwrap_or_default());
    let artifacts = PathBuf::from(cli.flag("artifacts").unwrap_or("/artifacts"));
    let checkout = PathBuf::from(cli.flag("checkout").unwrap_or("/checkout"));
    let wayland = cli.flag("journey-wayland").is_some();
    let only = cli.flag("only").filter(|o| *o != "all").map(String::from);
    let channel_json = channel_entry(&checkout, "dev")?;
    let home = scratch.join("home");
    let mut run_ = Run {
        failures: vec![],
        progress_step: String::new(),
        passed: 0,
    };
    let webkit = run(
        &["dpkg-query", "-W", "-f=${Version}", "libwebkit2gtk-4.1-0"],
        Duration::from_secs(20),
    )
    .map(|c| c.stdout.trim().to_string())
    .unwrap_or_default();
    outcome(
        &mut run_,
        "runtime_versions",
        "passed",
        vec![
            ("webkitgtk", s(webkit)),
            ("application", s("1.0.6")),
            ("tauri", s("2.10.1")),
            (
                "display",
                s(if wayland {
                    "private native Wayland with Mutter; software compositing"
                } else {
                    "private Xvfb/X11 with Openbox; software compositing"
                }),
            ),
            ("images", s("none")),
        ],
    );
    let cores = scratch.join("cores");
    std::fs::create_dir_all(&cores).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(scratch.join("logs")).map_err(|e| e.to_string())?;
    // Every child inherits this directory and limit, so a crash leaves a core here.
    std::env::set_current_dir(&cores).map_err(|e| e.to_string())?;
    unsafe {
        let mut limit = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        libc::getrlimit(libc::RLIMIT_CORE, &mut limit);
        limit.rlim_cur = limit.rlim_max;
        libc::setrlimit(libc::RLIMIT_CORE, &limit);
    }
    let channel = Channel::new("dev")?;
    let engine = artifacts.join("native-fake-engine");
    require(
        engine.is_file(),
        "native-fake-engine is not in the artifacts directory",
    )?;
    let peer_timing = super::peer_timing_requested(cli)?;
    let engine = if peer_timing {
        super::baseline::timed_peer(&scratch, &checkout, &engine)?
    } else {
        engine
    };
    for name in PROJECTS {
        // The coordinator question scenario belongs to the journey project; the others take the engine's plain path.
        let scenario = if name == "journey" {
            "reconcile-question"
        } else {
            ""
        };
        let root =
            native::prepare_with(&home, &channel, name, scenario, &engine.to_string_lossy())?;
        git_fixture(&root)?;
    }
    let probe_dir = scratch.join("probe");
    std::fs::create_dir_all(&probe_dir).map_err(|e| e.to_string())?;
    let shim = probe_library(&scratch)?;
    let mut st = Stack::new(
        scratch.clone(),
        probe_dir,
        shim,
        artifacts.clone(),
        checkout,
        channel_json,
        wayland,
    );
    st.baseline = super::baseline_requested(cli)?;
    st.peer_timing = peer_timing;
    let mut j = J {
        run: run_,
        st,
        home,
        channel,
        keep: Vec::new(),
        only,
    };
    let result = super::flow::drive(&mut j);
    let error = result.as_ref().err().cloned().unwrap_or_default();
    let mut exit = 0;
    if !error.is_empty() {
        let mut v = obj(vec![
            ("step", s("journey_incomplete")),
            ("status", s("failed")),
            ("reason", s(error.as_str())),
            ("last_step", s(j.run.progress_step.as_str())),
            ("steps_passed", n(j.run.passed)),
        ]);
        v.set("at", arr(vec![]));
        if let Some(failure) = j.st.observer.as_ref().and_then(|o| o.diagnostic()) {
            v.set("observer_failure", s(failure));
        }
        println!("{}", pretty_line(&v));
        let evidence = native_evidence(&mut j, &error);
        outcome(
            &mut j.run,
            "native_process_evidence",
            "diagnostic",
            outcome_pairs(&evidence),
        );
        super::flow::failure_diagnostics(&mut j);
        exit = 1;
    } else if let Err(e) = super::flow::finish(&mut j) {
        println!(
            "{}",
            pretty_line(&obj(vec![
                ("step", s("journey_incomplete")),
                ("status", s("failed")),
                ("reason", s(e.as_str()))
            ]))
        );
        exit = 1;
    }
    if !j.run.failures.is_empty() {
        exit = 1;
    }
    j.release_clipboards();
    // Shut down the observed client before its private relay, then the real service.
    stop(&mut j.st.app);
    drop(j.st.observer.take());
    for child in [
        &mut j.st.service,
        &mut j.st.pulse,
        &mut j.st.compositor,
        &mut j.st.wm,
        &mut j.st.xvfb,
    ] {
        stop(child);
    }
    outcome(&mut j.run, "owned_process_cleanup", "passed", vec![]);
    Ok(exit)
}
