//! Click the shipped desktop window the way a person does, and fail if a click changes nothing.
//!
//! Every click is a real pointer event from the X server (XTEST through xdotool), so it passes
//! through hit-testing, CSS pointer rules, overlays, inert regions and the page's own handlers
//! exactly like a mouse. Nothing is dispatched into the page. AT-SPI is only read, to find where
//! a control is and to see what the screen shows after the click. No screenshot is taken.
//!
//! Each scenario runs the installed binaries with a disposable HOME, XDG_RUNTIME_DIR, session bus
//! and X server, and leaves nothing running. Needs Xvfb, xdotool, dbus-run-session and the AT-SPI
//! library on the host.

use super::ffi::{Acc, COORD_SCREEN};
use super::util::*;
use super::Cli;
use crate::json::{dumps, parse, Format, Res, Value};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CLICKABLE: [&str; 6] = [
    "push button",
    "button",
    "toggle button",
    "link",
    "page tab",
    "menu item",
];

type Row = (String, String, i64, i64, i64, i64);

fn tree(desktop_binary: &str) -> Vec<Row> {
    let mut found = Vec::new();
    let Ok(desk) = Acc::desktop() else {
        return found;
    };
    let mut stack = Vec::new();
    if let Ok(count) = desk.child_count() {
        for i in 0..count {
            if let Ok(Some(app)) = desk.child(i) {
                if app.name().unwrap_or_default() == desktop_binary {
                    stack.push(app);
                    break;
                }
            }
        }
    }
    while let Some(node) = stack.pop() {
        if found.len() >= 5000 {
            break;
        }
        let step = (|| -> Res<()> {
            if node.states()?[1] {
                let (x, y, w, h) = node.extents(COORD_SCREEN).unwrap_or((0, 0, 0, 0));
                found.push((
                    node.role_name()?,
                    node.name()?,
                    x as i64,
                    y as i64,
                    w as i64,
                    h as i64,
                ));
            }
            for i in (0..node.child_count()?).rev() {
                if let Some(c) = node.child(i)? {
                    stack.push(c);
                }
            }
            Ok(())
        })();
        let _ = step;
    }
    found
}

fn shown(role: &str, name: &str, nodes: &[Row]) -> bool {
    nodes.iter().any(|r| r.0 == role && r.1 == name)
}

fn wait_for(desktop: &str, test: &dyn Fn(&[Row]) -> bool, seconds: u64) -> Vec<Row> {
    let end = Instant::now() + Duration::from_secs(seconds);
    loop {
        let nodes = tree(desktop);
        if test(&nodes) || Instant::now() > end {
            return nodes;
        }
        sleep_ms(250);
    }
}

fn channel_entry(root: &Path, name: &str) -> Res<Value> {
    let all = parse(&read_to_string(
        &root.join("crates/workspace-protocol/channels.json"),
    )?)?;
    all.get(name)
        .cloned()
        .ok_or_else(|| format!("unknown channel {name}"))
}

fn text(v: &Value, key: &str) -> Res<String> {
    v.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("channel field {key} missing"))
}

/// A registered project that is not running, whose saved plan names two tasks the runtime has never listed.
fn seed_plan_without_runtime_tasks(home: &Path, channel: &Value) -> Res<()> {
    let state = home
        .join(".local/state")
        .join(text(channel, "state_namespace")?);
    let project = format!("wp_{}1", "0".repeat(31));
    let folder = home.join("fixture-project");
    std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(state.join("product")).map_err(|e| e.to_string())?;
    chmod(&state, 0o700)?;
    chmod(&state.join("product"), 0o700)?;
    let registry = arr(vec![obj(vec![
        ("projectId", s(&project)),
        ("canonicalRoot", s(folder.to_string_lossy())),
        ("displayName", s("Fixture project")),
        ("activation", Value::Null),
    ])]);
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
        (
            "taskIds",
            arr(vec![s("wt_not_loaded_1"), s("wt_not_loaded_2")]),
        ),
        ("queueState", s("idle")),
        ("pauseReason", Value::Null),
    ]);
    let product = obj(vec![("tasks", obj(vec![])), ("sprints", arr(vec![sprint]))]);
    for (path, value) in [
        (state.join("projects.json"), registry),
        (
            state.join("product").join(format!("{project}.json")),
            product,
        ),
    ] {
        write(&path, &line(&value))?;
        chmod(&path, 0o600)?;
    }
    Ok(())
}

fn kill(child: &mut Child) {
    if child.try_wait().ok().flatten().is_none() {
        unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end && child.try_wait().ok().flatten().is_none() {
            sleep_ms(50);
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Inside `dbus-run-session`: start the installed service and window, then click the journey.
pub fn inner(cli: &Cli) -> Res<()> {
    let p = |i: usize| cli.positional.get(i).cloned().unwrap_or_default();
    let (prefix, base, app, fixture) = (
        PathBuf::from(p(0)),
        PathBuf::from(p(2)),
        PathBuf::from(p(3)),
        p(4),
    );
    let channel = channel_entry(&PathBuf::from(p(5)), &p(1))?;
    let (desktop, service, runtime_ns, package) = (
        text(&channel, "desktop_binary")?,
        text(&channel, "service_binary")?,
        text(&channel, "runtime_namespace")?,
        text(&channel, "package")?,
    );
    let runtime_dir = std::env::var("XDG_RUNTIME_DIR").map_err(|_| "XDG_RUNTIME_DIR unset")?;
    std::fs::create_dir_all(Path::new(&runtime_dir).join(&runtime_ns))
        .map_err(|e| e.to_string())?;
    chmod(&Path::new(&runtime_dir).join(&runtime_ns), 0o700)?;
    let runtime = prefix.join("lib").join(&package).join("runtime");
    let log = |name: &str| std::fs::File::create(base.join(name)).map_err(|e| e.to_string());
    let mut procs: Vec<Child> = Vec::new();
    procs.push(
        Command::new(prefix.join("bin").join(&service))
            .args(["/", "/usr/bin/python3"])
            .arg(&runtime)
            .stdout(log("service.log")?)
            .stderr(log("service.err")?)
            .spawn()
            .map_err(|e| e.to_string())?,
    );
    procs.push(
        Command::new(&app)
            .env("GDK_BACKEND", "x11")
            .env_remove("WAYLAND_DISPLAY")
            .stdout(log("app.log")?)
            .stderr(log("app.err")?)
            .spawn()
            .map_err(|e| e.to_string())?,
    );
    let outcome = (|| -> Value {
        let ready = |n: &[Row]| {
            shown("link", "Inbox", n)
                && n.iter().any(|r| r.1 == "This machine Connected")
                && (fixture.is_empty() || n.iter().any(|r| r.1.contains(&fixture)))
        };
        let nodes = wait_for(&desktop, &ready, 60);
        if !ready(&nodes) {
            return obj(vec![
                ("ok", b(false)),
                (
                    "steps",
                    arr(vec![obj(vec![
                        ("step", s("window")),
                        ("ok", b(false)),
                        (
                            "reason",
                            s("no sidebar, no connected machine or no fixture on screen"),
                        ),
                    ])]),
                ),
            ]);
        }
        // The service and the page settle (projects, plan) before the first click, as they would for a person.
        sleep_ms(4000);
        let journey: [(&str, Option<(&str, &str)>); 5] = [
            ("Inbox", Some(("heading", "Inbox"))),
            ("Archive", Some(("entry", "Filter archived tasks"))),
            ("Settings", Some(("link", "Back to workspace"))),
            ("Back to workspace", Some(("link", "Inbox"))),
            ("Sidebar", None),
        ];
        let (mut steps, mut ok) = (Vec::new(), true);
        for (label, expect) in journey {
            let nodes = tree(&desktop);
            let target = nodes
                .iter()
                .find(|r| r.1 == label && CLICKABLE.contains(&r.0.as_str()) && r.4 > 0 && r.5 > 0)
                .map(|r| (r.2, r.3, r.4, r.5));
            let Some((x0, y0, w, h)) = target else {
                steps.push(obj(vec![
                    ("step", s(label)),
                    ("ok", b(false)),
                    ("reason", s("control not on screen")),
                ]));
                ok = false;
                continue;
            };
            let (x, y) = (x0 + w / 2, y0 + h / 2);
            let click = Command::new("xdotool")
                .args(["mousemove", "--sync", &x.to_string(), &y.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
                && {
                    sleep_ms(200);
                    Command::new("xdotool")
                        .args(["click", "1"])
                        .status()
                        .map(|s| s.success())
                        .unwrap_or(false)
                };
            if !click {
                return obj(vec![
                    ("ok", b(false)),
                    ("steps", arr(steps)),
                    ("reason", s("xdotool failed")),
                ]);
            }
            let passed = match expect {
                None => !shown(
                    "link",
                    "Inbox",
                    &wait_for(&desktop, &|n| !shown("link", "Inbox", n), 5),
                ),
                Some((role, name)) => shown(
                    role,
                    name,
                    &wait_for(&desktop, &|n| shown(role, name, n), 5),
                ),
            };
            let mut step = obj(vec![
                ("step", s(label)),
                ("at", arr(vec![n(x), n(y)])),
                ("ok", b(passed)),
            ]);
            if !passed {
                let wanted = expect
                    .map(|(r, nm)| format!("{r} {nm}"))
                    .unwrap_or_else(|| "hidden sidebar".into());
                step.set("reason", s(format!("after the click no {wanted}")));
            }
            steps.push(step);
            ok &= passed;
        }
        obj(vec![("ok", b(ok)), ("steps", arr(steps))])
    })();
    for child in procs.iter_mut().rev() {
        kill(child);
    }
    println!("RESULT {}", line(&outcome));
    Ok(())
}

fn scenario(
    name: &str,
    prefix: &Path,
    channel: &Value,
    channel_name: &str,
    root: &Path,
    display: &str,
    app: &Path,
) -> Res<Value> {
    let base = crate::proc::TempDir::new("cw-real-input-")?;
    let (home, run) = (base.0.join("home"), base.0.join("run"));
    std::fs::create_dir(&home).map_err(|e| e.to_string())?;
    std::fs::create_dir(&run).map_err(|e| e.to_string())?;
    chmod(&run, 0o700)?;
    let mut fixture = String::new();
    if name == "plan without runtime tasks" {
        seed_plan_without_runtime_tasks(&home, channel)?;
        fixture = "Fixture project".into();
    }
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new("dbus-run-session");
    cmd.env_clear()
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", &run)
        .env("DISPLAY", display)
        .env("PATH", "/usr/local/bin:/usr/bin:/bin")
        .env(
            "LANG",
            std::env::var("LANG").unwrap_or_else(|_| "C.UTF-8".into()),
        )
        .current_dir(&base.0)
        .args(["--"])
        .arg(&me)
        .arg("check-real-input-inner")
        .arg(prefix)
        .arg(channel_name)
        .arg(&base.0)
        .arg(app)
        .arg(&fixture)
        .arg(root);
    let out = capture(&mut cmd, Duration::from_secs(300))?;
    let mut result = match out.stdout.lines().find_map(|l| l.strip_prefix("RESULT ")) {
        Some(text) => parse(text)?,
        None => obj(vec![
            ("ok", b(false)),
            (
                "steps",
                arr(vec![obj(vec![
                    ("step", s("run")),
                    ("ok", b(false)),
                    ("reason", s(format!("no result, exit {}", out.code))),
                ])]),
            ),
        ]),
    };
    // Counted markers only: logs can carry private paths, so no body is retained.
    let app_log = std::fs::read_to_string(base.0.join("app.log")).unwrap_or_default();
    result.set("app_panics", n(app_log.matches("panicked").count() as i64));
    let mut row = obj(vec![("scenario", s(name))]);
    for (k, v) in result.as_obj().unwrap_or(&[]) {
        row.set(k, v.clone());
    }
    Ok(row)
}

pub fn host(cli: &Cli, root: &Path) -> Res<i32> {
    let prefix = cli.path("prefix").unwrap_or_else(|| PathBuf::from("/usr"));
    let channel_name = cli.flag("channel").unwrap_or("dev").to_string();
    let channel = channel_entry(root, &channel_name)?;
    let app = cli.path("app").unwrap_or_else(|| {
        prefix
            .join("bin")
            .join(text(&channel, "desktop_binary").unwrap_or_default())
    });
    let app = app
        .canonicalize()
        .map_err(|e| format!("desktop binary: {e}"))?;
    for tool in ["Xvfb", "xdotool", "dbus-run-session"] {
        if crate::proc::which(tool).is_none() {
            eprintln!("missing {tool}");
            return Ok(2);
        }
    }
    let mut fds = [0i32; 2];
    require(unsafe { libc::pipe(fds.as_mut_ptr()) } == 0, "pipe failed")?;
    let mut xvfb = Command::new("Xvfb")
        .args([
            "-displayfd",
            &fds[1].to_string(),
            "-screen",
            "0",
            "1600x1000x24",
            "-nolisten",
            "tcp",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    unsafe { libc::close(fds[1]) };
    let mut buf = [0u8; 32];
    let got = unsafe { libc::read(fds[0], buf.as_mut_ptr() as *mut _, 32) };
    unsafe { libc::close(fds[0]) };
    let display = format!(
        ":{}",
        String::from_utf8_lossy(&buf[..got.max(0) as usize]).trim()
    );
    let mut results = Vec::new();
    let mut failure = None;
    for name in ["fresh home", "plan without runtime tasks"] {
        match scenario(name, &prefix, &channel, &channel_name, root, &display, &app) {
            Ok(r) => results.push(r),
            Err(e) => {
                failure = Some(e);
                break;
            }
        }
    }
    kill(&mut xvfb);
    if let Some(e) = failure {
        return Err(e);
    }
    println!(
        "{}",
        dumps(
            &arr(results.clone()),
            &Format {
                sort_keys: false,
                ensure_ascii: true,
                indent: Some(1),
                compact: false
            }
        )?
    );
    let good = results.iter().all(|r| {
        matches!(r.get("ok"), Some(Value::Bool(true)))
            && r.get("app_panics").and_then(Value::as_i64) == Some(0)
    });
    Ok(if good { 0 } else { 1 })
}
