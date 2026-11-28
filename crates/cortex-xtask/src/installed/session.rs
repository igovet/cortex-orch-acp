//! The desktop-user half of the installed gate: private display/compositor, the packaged
//! application launched through its own desktop entry, and real pointer/keyboard input.

use super::channel::Channel;
use super::config::{coordinator_visible, profiles_visible, EFFORT, MODEL, PROFILE_NAMES};
use super::input::{Input, Node};
use super::native;
use super::require_container;
use super::util::*;
use crate::json::{parse, Res, Value};
use std::os::unix::fs::{FileTypeExt, MetadataExt};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const JOURNEY: &str = "/home/acceptance/journey";

fn sidebar_visible(nodes: &[Node]) -> bool {
    // Home also has an Inbox link; only the named navigation landmark is the sidebar.
    nodes
        .iter()
        .any(|n| n.role == "landmark" && n.name == "Workspace" && n.w > 0 && n.h > 0)
}

pub fn require_startup_health(nodes: &[Node]) -> Res<()> {
    let forbidden = [
        "Not connected to the service",
        "data is loading",
        "could not be loaded",
        "Task details unavailable",
        "Waiting for sprint task details",
        "Queue paused: This sprint",
    ];
    let bad = nodes.iter().any(|n| {
        n.name == "Retry"
            || forbidden
                .iter()
                .any(|w| format!("{} {}", n.name, n.text).contains(w))
    });
    require(!bad, "startup showed a connection failure before readiness")
}

fn require_full_worker(input: &Input) -> Res<()> {
    require(
        input.has("general activity", &["landmark"])?,
        "worker activity pane missing",
    )?;
    // The worker stream has no search or Follow strip: following resumes at the end on its own.
    require(
        !input.has("Find in this activity", &[])?,
        "removed worker search strip returned",
    )?;
    require(
        !input.has("Show full history", &[])?,
        "intermediate worker view returned",
    )
}

fn app_alive(package: &str) -> bool {
    let uid = unsafe { libc::getuid() };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return false;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_string_lossy().chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let dir = entry.path();
        let Ok(meta) = std::fs::metadata(&dir) else {
            continue;
        };
        if meta.uid() != uid {
            continue;
        }
        if let Ok(exe) = std::fs::read_link(dir.join("exe")) {
            let base = exe
                .file_name()
                .map(|b| b.to_string_lossy().into_owned())
                .unwrap_or_default();
            if base == package || base.starts_with("WebKit") {
                return true;
            }
        }
    }
    false
}

/// A framed `service.status` read as this desktop user, proving the service survived the close.
fn service_status(channel: &Channel) -> Res<()> {
    use std::io::{Read, Write};
    let mut stream =
        UnixStream::connect(&channel.socket).map_err(|e| format!("service socket: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    #[repr(C)]
    #[derive(Default)]
    struct Cred {
        pid: i32,
        uid: u32,
        gid: u32,
    }
    let mut cred = Cred::default();
    let mut len = std::mem::size_of::<Cred>() as libc::socklen_t;
    use std::os::fd::AsRawFd;
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut Cred as *mut _,
            &mut len,
        )
    };
    require(
        rc == 0 && cred.pid > 0 && cred.uid == unsafe { libc::getuid() },
        "wrong service peer",
    )?;
    let request = line(&obj(vec![
        ("jsonrpc", s("2.0")),
        ("id", n(1)),
        ("method", s("service.status")),
        ("params", obj(vec![])),
    ]));
    let mut frame = (request.len() as u32).to_be_bytes().to_vec();
    frame.extend(request.as_bytes());
    stream.write_all(&frame).map_err(|e| e.to_string())?;
    let mut head = [0u8; 4];
    stream
        .read_exact(&mut head)
        .map_err(|_| "service health reply truncated".to_string())?;
    let size = u32::from_be_bytes(head) as usize;
    require(size > 0 && size <= 1_048_576, "service health reply size")?;
    let mut body = vec![0u8; size];
    stream
        .read_exact(&mut body)
        .map_err(|_| "service health reply truncated".to_string())?;
    let reply = parse(&String::from_utf8_lossy(&body))?;
    require(
        reply.get("id").and_then(Value::as_i64) == Some(1)
            && reply.get("result").is_some()
            && reply.get("error").is_none(),
        "unhealthy service reply",
    )
}

struct Procs(Vec<Child>);

impl Drop for Procs {
    fn drop(&mut self) {
        for child in self.0.iter_mut().rev() {
            if child.try_wait().ok().flatten().is_none() {
                unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
                let end = std::time::Instant::now() + Duration::from_secs(10);
                while std::time::Instant::now() < end && child.try_wait().ok().flatten().is_none() {
                    sleep_ms(50);
                }
                if child.try_wait().ok().flatten().is_none() {
                    let _ = child.kill();
                }
                let _ = child.wait();
            }
        }
    }
}

fn unit_substate(unit: &str) -> Res<String> {
    let done = run_ok(&[
        "systemctl",
        "--user",
        "show",
        unit,
        "-p",
        "SubState",
        "--value",
    ])?;
    Ok(done.stdout.trim().to_string())
}

pub fn run(channel: &Channel, backend: &str, phase: &str, close_cycles: u32) -> Res<()> {
    require_container(true)?;
    let package = channel.package.clone();
    let unit = channel.unit.clone();
    let logs = PathBuf::from("/evidence").join(format!("{backend}-{phase}"));
    std::fs::create_dir(&logs).map_err(|e| e.to_string())?;
    let mut procs = Procs(Vec::new());
    let slot: std::cell::OnceCell<Input> = std::cell::OnceCell::new();
    let desktop_entry = format!("/usr/share/applications/{package}.desktop");
    let result = (|| -> Res<()> {
        let runtime_dir = std::env::var("XDG_RUNTIME_DIR").map_err(|_| "XDG_RUNTIME_DIR unset")?;
        let spawn = |procs: &mut Procs, argv: &[&str], name: &str| -> Res<usize> {
            let sink = std::fs::File::create(logs.join(format!("{name}.log")))
                .map_err(|e| e.to_string())?;
            let err = sink.try_clone().map_err(|e| e.to_string())?;
            let child = Command::new(argv[0])
                .args(&argv[1..])
                .stdin(Stdio::null())
                .stdout(sink)
                .stderr(err)
                .spawn()
                .map_err(|e| format!("{}: {e}", argv[0]))?;
            procs.0.push(child);
            Ok(procs.0.len() - 1)
        };
        if backend == "x11" {
            std::env::set_var("DISPLAY", ":99");
            std::env::set_var("GDK_BACKEND", "x11");
            spawn(
                &mut procs,
                &[
                    "Xvfb",
                    ":99",
                    "-screen",
                    "0",
                    "1920x1200x24",
                    "-nolisten",
                    "tcp",
                ],
                "display",
            )?;
            wait("X server", || {
                Ok(std::fs::metadata("/tmp/.X11-unix/X99")
                    .map(|m| m.file_type().is_socket())
                    .unwrap_or(false))
            })?;
            spawn(&mut procs, &["openbox"], "wm")?;
            sleep_ms(1000);
        } else {
            std::env::set_var("WAYLAND_DISPLAY", "cw-acceptance");
            std::env::set_var("GDK_BACKEND", "wayland");
            spawn(
                &mut procs,
                &[
                    "mutter",
                    "--headless",
                    "--wayland",
                    "--no-x11",
                    "--virtual-monitor",
                    "1920x1200",
                    "--wayland-display",
                    "cw-acceptance",
                ],
                "compositor",
            )?;
            wait("Wayland compositor", || {
                Ok(
                    std::fs::metadata(Path::new(&runtime_dir).join("cw-acceptance"))
                        .map(|m| m.file_type().is_socket())
                        .unwrap_or(false),
                )
            })?;
        }
        let _ = slot.set(Input::new(backend)?);
        let input = slot.get().ok_or("input unavailable")?;
        // gio launch reads the installed desktop entry and its real Exec command.
        run_inherit(&["gio", "launch", &desktop_entry], Duration::from_secs(120))?;
        if phase == "delayed-start" || phase == "cached-board" {
            wait_secs("service demonstrably not ready", 5, || {
                Ok(unit_substate(&unit)? == "start-pre")
            })?;
            emit(
                "service_not_ready_observed",
                vec![("backend", s(backend)), ("phase", s(phase))],
            );
        }
        let renamed = matches!(phase, "large-board" | "cached-board");
        let machine_label = if renamed { "Local Connected" } else { "This machine Connected" };
        let startup_samples = std::cell::Cell::new(0i64);
        let startup_ready = |window_only: bool| -> Res<bool> {
            let nodes = input.nodes()?;
            if ["cold-start", "delayed-start", "large-board", "cached-board"].contains(&phase) {
                require_startup_health(&nodes)?;
                startup_samples.set(startup_samples.get() + 1);
            }
            Ok(nodes.iter().any(|n| {
                if window_only {
                    n.name == "Sidebar"
                } else {
                    n.name == machine_label
                }
            }))
        };
        wait_secs("installed desktop window", 60, || startup_ready(true))?;
        if phase == "cached-board" {
            require_startup_health(&input.nodes()?)?;
            require(
                input.seen("Large board saved task")?,
                "cached sprint did not paint before service readiness",
            )?;
            emit(
                "cached_board_before_service_ready",
                vec![("backend", s(backend))],
            );
        }
        if backend == "x11" {
            input.click("Maximize", &[], false)?;
        } else {
            input.key("super+Up")?;
        }
        sleep_ms(1000);
        wait("healthy connected machine", || startup_ready(false))?;
        if renamed {
            emit("saved_machine_name_visible_on_startup", vec![("backend", s(backend)), ("phase", s(phase))]);
        }
        wait("maximized native frame", || {
            Ok(input
                .nodes()?
                .iter()
                .any(|n| n.role == "frame" && n.w == 1920 && n.h == 1200))
        })?;
        let click = |name: &str, expect: &str, roles: &[&str], contains: bool| -> Res<()> {
            input.click(name, roles, contains)?;
            wait(&format!("response to {name}"), || {
                if name == "Inbox" {
                    input.has("Inbox", &["heading"])
                } else {
                    input.seen(expect)
                }
            })?;
            emit(
                "real_input",
                vec![
                    ("control", s(name)),
                    ("response", s(expect)),
                    ("backend", s(backend)),
                    ("phase", s(phase)),
                ],
            );
            Ok(())
        };
        if phase == "cold-start" || phase == "incident" {
            click("Settings", "Back to workspace", &["link"], false)?;
            click("Orchestration", "Coordinator", &["link"], false)?;
            click("Profiles", "Saved", &["link"], false)?;
            wait("six profiles with default active in Settings", || {
                Ok(profiles_visible(&input.nodes()?))
            })?;
            // Coordinator is a group inside Defaults, not a navigation tab. Returning through
            // the Defaults link keeps the check on the owner-visible settings route.
            click("Defaults", "Exact default route", &["link"], false)?;
            wait("saved coordinator model and effort in Settings", || {
                Ok(coordinator_visible(&input.nodes()?))
            })?;
            emit(
                "orchestration_settings_visible",
                vec![
                    ("backend", s(backend)),
                    ("phase", s(phase)),
                    ("channel", s(&channel.name)),
                    ("active", s("default")),
                    ("profiles", n(PROFILE_NAMES.len() as i64)),
                    ("coordinator", s(MODEL)),
                    ("effort", s(EFFORT)),
                ],
            );
            click("Back to workspace", "Inbox", &["link"], false)?;
        }
        if phase == "cold-start" || phase == "delayed-start" {
            emit(
                "startup_connected_without_retry",
                vec![
                    ("backend", s(backend)),
                    ("phase", s(phase)),
                    ("samples", n(startup_samples.get())),
                ],
            );
        } else if phase == "large-board" || phase == "cached-board" {
            wait("large project visible", || {
                input.seen("Large board fixture")
            })?;
            if input.has("Show tasks of Large board fixture", &[])? {
                input.click("Show tasks of Large board fixture", &[], false)?;
            }
            click(
                "Large saved sprint",
                "Large board saved task",
                &["link"],
                true,
            )?;
            wait("saved sprint task cards", || {
                let nodes = input.nodes()?;
                require_startup_health(&nodes)?;
                Ok(nodes
                    .iter()
                    .any(|n| n.both().contains("Large board saved task")))
            })?;
            for _ in 0..10 {
                require_startup_health(&input.nodes()?)?;
                sleep_ms(150);
            }
            let visible = input
                .nodes()?
                .iter()
                .filter(|n| n.role == "link" && n.name.contains("Large board saved task"))
                .count();
            require(visible > 0, "no sprint task links visible")?;
            emit(
                "large_board_visible_without_startup_banner",
                vec![
                    ("backend", s(backend)),
                    ("phase", s(phase)),
                    ("fixture_sprint_tasks", n(31)),
                    ("visible_task_links", n(visible as i64)),
                ],
            );
        } else if phase == "native-journey" {
            native::journey(input, backend, channel)?;
        } else if phase == "incident" {
            for name in ["Incident fixture", "Other fixture", "Unavailable fixture"] {
                wait(&format!("registered project {name}"), || input.seen(name))?;
            }
            emit(
                "all_projects_visible",
                vec![
                    ("backend", s(backend)),
                    ("count", n(3)),
                    ("unreadable_store", b(true)),
                ],
            );
            click("Inbox", "Inbox", &["link"], false)?;
            require(input.has("Inbox", &["heading"])?, "Inbox failed to render")?;
            click("Archive", "Filter archived tasks", &["link"], false)?;
            click("Settings", "Back to workspace", &["link"], false)?;
            click("Back to workspace", "Inbox", &["link"], false)?;
        } else {
            journey_phase(input, &mut procs, &logs, backend, phase, channel, &click)?;
        }
        input.stable("Sidebar")?;
        require(
            sidebar_visible(&input.nodes()?),
            "sidebar must be visible before the hide action",
        )?;
        input.click("Sidebar", &[], false)?;
        wait("sidebar hidden", || Ok(!sidebar_visible(&input.nodes()?)))?;
        input.click("Sidebar", &[], false)?;
        wait("sidebar restored", || Ok(sidebar_visible(&input.nodes()?)))?;
        input.closing.set(true);
        input.click("Close window", &[], false)?;
        wait("desktop process exited", || Ok(!app_alive(&package)))?;
        wait("window removed", || Ok(input.nodes()?.is_empty()))?;
        // Reopen the actual installed desktop entry without changing its profile or service.
        for cycle in 2..=close_cycles {
            input.closing.set(false);
            run_inherit(&["gio", "launch", &desktop_entry], Duration::from_secs(120))?;
            wait_secs("reopened desktop window", 60, || input.has("Sidebar", &[]))?;
            wait("reopened connected machine", || {
                input.has(machine_label, &[])
            })?;
            if backend == "wayland" {
                // Establish global hit-test geometry again: Wayland exposes no window position.
                input.key("super+Up")?;
                wait("reopened maximized native frame", || {
                    Ok(input
                        .nodes()?
                        .iter()
                        .any(|n| n.role == "frame" && n.w == 1920 && n.h == 1200))
                })?;
            }
            input.closing.set(true);
            input.click("Close window", &[], false)?;
            wait("reopened desktop process exited", || {
                Ok(!app_alive(&package))
            })?;
            wait("reopened window removed", || Ok(input.nodes()?.is_empty()))?;
            emit(
                "repeated_close",
                vec![
                    ("backend", s(backend)),
                    ("phase", s(phase)),
                    ("cycle", n(cycle as i64)),
                    ("process_exited", b(true)),
                ],
            );
        }
        service_status(channel)?;
        emit(
            "own_close_button",
            vec![
                ("backend", s(backend)),
                ("phase", s(phase)),
                ("process_exited", b(true)),
                ("service_responded", b(true)),
            ],
        );
        Ok(())
    })();
    if let Err(error) = &result {
        let children: Vec<Value> = procs
            .0
            .iter_mut()
            .map(|p| {
                obj(vec![
                    ("pid", n(p.id() as i64)),
                    (
                        "exit_code",
                        p.try_wait()
                            .ok()
                            .flatten()
                            .and_then(|s| s.code())
                            .map(|c| n(c as i64))
                            .unwrap_or(Value::Null),
                    ),
                ])
            })
            .collect();
        emit(
            "failure_native_state",
            vec![
                ("backend", s(backend)),
                ("phase", s(phase)),
                ("children", arr(children)),
                ("error", s(error)),
            ],
        );
        if let Some(input) = slot.get() {
            if let Ok(nodes) = input.nodes_checked(false) {
                let tree = arr(nodes.iter().map(Node::json).collect());
                let _ = std::fs::write(
                    logs.join("failure-tree.json"),
                    crate::json::dumps(
                        &tree,
                        &crate::json::Format {
                            sort_keys: false,
                            ensure_ascii: true,
                            indent: Some(2),
                            compact: false,
                        },
                    )
                    .unwrap_or_default(),
                );
            }
        }
    }
    drop(procs);
    result
}

/// The fake-engine journey: project registration, drafts, OS transfer, questions, workers, Settings.
fn journey_phase(
    input: &Input,
    procs: &mut Procs,
    logs: &Path,
    backend: &str,
    _phase: &str,
    channel: &Channel,
    click: &dyn Fn(&str, &str, &[&str], bool) -> Res<()>,
) -> Res<()> {
    click("Add project", "Add project", &[], false)?;
    click(
        "Search for folder",
        "Search for folder",
        &["list item"],
        true,
    )?;
    input.fill("Search for folder", "/home/acceptance/journey")?;
    // The picker resolves the on-disk directory before selection. Its list is read again after
    // every typed character, and a press on a row that is replaced before the release is lost:
    // wait until the typed folder's own row is listed and holds still between two reads.
    let mut seen = None;
    wait("typed folder listed and still", || {
        let now = input
            .find("/home/acceptance/journey", &["list item"], true)?
            .map(|n| (n.x, n.y, n.w, n.h));
        let still = now.is_some() && now == seen;
        seen = now;
        Ok(still)
    })?;
    click("journey", "Activate", &["list item"], true)?;
    input.stable("Activate")?;
    click("Activate", "New task", &[], false)?;
    wait_secs("activation finished", 60, || {
        Ok(!input.has("Activate", &[])?)
    })?;
    click("New task", "Describe the task", &[], false)?;
    input.fill("Describe the task", "Installed input")?;
    procs.0.push(input.paste(" journey")?);
    wait("OS clipboard paste", || {
        input.seen("Installed input journey")
    })?;
    // The no-paid peer's reconciliation scenarios require the owner goal to carry this constraint.
    // Pasted, not typed: a keysym outside the keymap cannot be typed on every compositor.
    procs.0.push(input.paste(" Never lose constraint α")?);
    wait("constraint text pasted", || {
        input.seen("Never lose constraint α")
    })?;
    // The drag target exists only when the host offers attachments. When it reports them unavailable
    // the file drop cannot be exercised: that is recorded as not_applicable, never as a pass.
    if input.seen("Attachments are not available on this machine")? {
        emit(
            "os_transfer",
            vec![
                ("backend", s(backend)),
                ("clipboard", b(true)),
                ("drag_drop", s("not_applicable")),
                ("reason", s("host reports attachments unavailable")),
            ],
        );
    } else {
        // Keep source and destination visible together. Wayland has no client positioning API:
        // real WM tiling keys establish their places.
        if backend == "wayland" {
            input.key("super+Right")?;
            sleep_ms(600);
        }
        let target = input
            .find("Describe the task", &["entry"], false)?
            .ok_or("composer entry missing")?;
        let tx = target.x as f64
            + target.w as f64 / 2.0
            + if backend == "wayland" { 960.0 } else { 0.0 };
        let ty = target.y as f64 + target.h as f64 / 2.0;
        let (fixture, ready) = (logs.join("drop.txt"), logs.join("drag-ready"));
        write(&fixture, "Installed attachment fixture")?;
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        let log = std::fs::File::create(logs.join("drag.log")).map_err(|e| e.to_string())?;
        let log2 = log.try_clone().map_err(|e| e.to_string())?;
        let mut drag = Command::new(me)
            .arg("installed-drag-source")
            .arg(&fixture)
            .arg(&ready)
            .arg(backend)
            .stdout(log)
            .stderr(log2)
            .spawn()
            .map_err(|e| e.to_string())?;
        let (sx, sy);
        if backend == "wayland" {
            sleep_ms(1000);
            input.key("super+Left")?;
            sx = 480.0;
            sy = 600.0;
        } else {
            sleep_ms(1000);
            let found = run_ok(&[
                "xdotool",
                "search",
                "--name",
                "^Installed attachment fixture$",
            ])?;
            let wid = found
                .stdout
                .lines()
                .last()
                .ok_or("drag source window missing")?
                .to_string();
            let geometry = run_ok(&["xdotool", "getwindowgeometry", "--shell", &wid])?;
            let g: std::collections::HashMap<&str, i64> = geometry
                .stdout
                .lines()
                .filter_map(|l| l.split_once('='))
                .filter_map(|(k, v)| v.parse().ok().map(|v| (k, v)))
                .collect();
            sx = (g["X"] as f64) + (g["WIDTH"] as f64) / 2.0;
            sy = (g["Y"] as f64) + (g["HEIGHT"] as f64) / 2.0;
        }
        wait("visible OS drag source", || Ok(ready.exists()))?;
        input.move_to(sx, sy)?;
        input.button(true)?;
        sleep_ms(100);
        input.move_to(sx + 20.0, sy + 20.0)?;
        sleep_ms(200);
        for step in 1..=20 {
            let f = step as f64 / 20.0;
            input.move_to(sx + (tx - sx) * f, sy + (ty - sy) * f)?;
            sleep_ms(40);
        }
        sleep_ms(500);
        input.button(false)?;
        wait("real OS file drop", || input.seen("drop.txt"))?;
        unsafe { libc::kill(drag.id() as i32, libc::SIGTERM) };
        let _ = drag.wait();
        if backend == "wayland" {
            input.click_at(tx, ty)?;
            input.key("super+Up")?;
            sleep_ms(600);
        }
        emit(
            "os_transfer",
            vec![
                ("backend", s(backend)),
                ("clipboard", b(true)),
                ("drag_drop", b(true)),
            ],
        );
    }
    let pressed = std::time::Instant::now();
    click("Save draft", "Installed input journey", &[], false)?;
    // The description already contains the title before Save returns. It is not evidence of
    // navigation or a committed task projection.
    let mut samples = Vec::new();
    let mut last_sample = 0i64;
    let settled = wait("saved draft reconciled on the board", || {
        let form = input.has("Save draft", &[])?;
        let scope = input.has("Board scope: All work", &[])?;
        let updating = input.seen("Updating confirmed state")?;
        let at = pressed.elapsed().as_millis() as i64;
        if at - last_sample >= 500 {
            last_sample = at;
            samples.push(s(format!(
                "{at} form={} scope={} updating={}",
                form as u8, scope as u8, updating as u8
            )));
        }
        Ok(!form && scope && !updating)
    });
    let confirmed_ms = pressed.elapsed().as_millis() as i64;
    emit(
        "save_draft_confirmation",
        vec![
            ("backend", s(backend)),
            ("settled", b(settled.is_ok())),
            ("ms", n(confirmed_ms)),
            ("samples", arr(samples)),
        ],
    );
    if settled.is_err() {
        // What the service answers now, and its own diagnostics: a slow service, or a window that keeps rereading.
        super::probe::diagnose(&channel.socket, "save_draft_unconfirmed");
    }
    settled?;
    let card = wait_with("saved task link", 30, || {
        input.find("Installed input journey", &["link"], true)
    })?;
    input.stable(&card.name)?;
    click(
        "Installed input journey",
        "Task progress and details",
        &["link"],
        true,
    )?;
    wait("task conversation opened", || {
        input.has("Task progress and details", &["panel"])
    })?;
    // The native coordinator plans one backend worker; after verification it asks the owner how to
    // finish (the no-paid peer's `reconcile-question` scenario). Choosing an option only selects it.
    let root = Path::new(JOURNEY);
    let home = Path::new("/home/acceptance");
    // Start is the task composer's action button; the title band has no Start.
    input.click_in(super::input::TASK_COMPOSER, "Start")?;
    wait("response to Start", || input.seen("How should we finish?"))?;
    emit(
        "real_input",
        vec![
            ("control", s("Start")),
            ("response", s("How should we finish?")),
            ("backend", s(backend)),
            ("phase", s(_phase)),
        ],
    );
    let answers = |root: &Path| -> Res<i64> {
        native::count(
            &channel.store_db(home, root)?,
            "SELECT count(*) FROM runtime_events WHERE type='USER_ANSWER_RECEIVED'",
        )
    };
    require(
        answers(root)? == 0,
        "an answer was delivered before the owner chose",
    )?;
    // The delegated workers finished before the question; their link must stay exactly where it is
    // while the answer is submitted and the board reconciles. The task bar's bare "Work details"
    // link is there before any worker is, so a worker reaches the window only when that link
    // counts every worker the store records. The native owner read lists one worker per
    // runtime node with a delegation provisioning record of the task's current execution
    // (`owner_read` in crates/workspace-service/src/native_owner_controls.rs).
    let recorded = native::count(
        &channel.store_db(home, root)?,
        "SELECT count(*) FROM runtime_nodes n JOIN delegation_provisioning p ON p.task_id=n.task_id AND p.idempotency_key=n.assignment_ref \
         WHERE n.task_id=(SELECT e.runtime_task_id FROM workspace_tasks t JOIN workspace_executions e ON e.execution_id=t.execution_id ORDER BY t.rowid DESC LIMIT 1)",
    )?;
    let settled = format!("Work details {recorded} finished");
    emit(
        "fixture_workers_recorded",
        vec![
            ("backend", s(backend)),
            ("recorded", n(recorded)),
            ("expected_link", s(&settled)),
        ],
    );
    require(recorded >= 1, "no worker recorded before the question")?;
    wait_secs("worker reached the window", 60, || {
        Ok(input.find(&settled, &["link"], false)?.is_some())
    })
    .map_err(|e| {
        let shown = input
            .find("Work details", &["link"], true)
            .ok()
            .flatten()
            .map(|n| n.name)
            .unwrap_or_default();
        format!("{e}; expected link {settled:?}, task bar shows {shown:?}")
    })?;
    let worker = wait_with("worker link", 30, || input.find(&settled, &["link"], false))?;
    input.stable(&worker.name)?;
    input.click("Explain and finish Recommended", &["radio button"], false)?;
    require(
        answers(root)? == 0,
        "choosing an option alone delivered an answer",
    )?;
    input.click("Send answer", &[], false)?;
    input.stable(&worker.name)?;
    // The question text also stays in the transcript; the answer plane is what must leave.
    wait("answered question left the screen", || {
        Ok(!input.has("Send answer", &[])?)
    })?;
    wait_secs("answer delivered", 60, || Ok(answers(root)? >= 1))?;
    emit("fixture_worker_ready", vec![("backend", s(backend))]);
    click("Work details", "general", &["link"], true)?;
    input.click("Coordinator", &["page tab"], true)?;
    // The coordinator feed is selected first; the worker's full activity stream follows.
    wait("coordinator feed selected", || {
        input.has("Coordinator activity", &["landmark"])
    })?;
    click("general", "general activity", &["page tab"], true)?;
    require_full_worker(input)?;
    input.click("Conversation", &["link"], false)?;
    // Stop/Resume exactness and runtime-certified completion are asserted by the native journey
    // (second task); this task runs to its certified result.
    wait_secs("runtime result", 90, || {
        Ok(
            native::state(home, root, channel)?.as_deref() == Some("COMPLETED")
                && input.seen("Owner decision explained.")?,
        )
    })?;
    let mut pairs = vec![("backend", s(backend))];
    pairs.extend(native::evidence(home, root, channel, false)?);
    emit("journey_native_task", pairs);
    click("Review changes", "Changes", &["link"], false)?;
    wait("review content rendered", || {
        input.has("Changes", &["landmark"])
    })?;
    input.key("Escape")?;
    // Every reachable Settings navigation page is followed through a real link. SettingsNav
    // deliberately exposes page links (not ARIA tabs), including the orchestration children.
    click("Settings", "Back to workspace", &["link"], false)?;
    let pages = [
        ("Updates", "Updates"),
        ("Appearance", "Appearance"),
        ("Voice input", "Voice input"),
        ("Shortcuts", "Shortcuts"),
        ("All machines", "Machines"),
        ("Overview", "Open machine settings"),
        // The bare Orchestration link intentionally opens the Defaults page.
        ("Orchestration", "Defaults"),
        ("Defaults", "Defaults"),
        ("Coordinator models", "Coordinator models"),
        ("Models", "Models"),
        ("Engines and models", "Engines and models"),
        ("Project ceiling", "Project ceiling"),
        ("Routing", "Routing"),
        ("MCP servers", "MCP servers"),
        ("Profiles", "Profiles"),
        ("Connections", "Connections"),
        ("Worktrees", "Worktrees"),
        ("Storage and archive", "Storage and archive"),
        ("Diagnostics", "Diagnostics"),
        ("Remote access", "Remote access"),
    ];
    for (name, marker) in pages {
        input.click(name, &["link"], name == "All machines")?;
        wait(&format!("Settings content {name}"), || {
            input.has(marker, &["heading"])
        })?;
        input.stable(marker)?;
        emit(
            "settings_page",
            vec![("name", s(name)), ("marker", s(marker))],
        );
    }

    // Route preview and Files and sources are deliberately secondary links from
    // Routing, not SettingsNav entries. Exercise both links and their exact page
    // headings, then return through each page's real Routing link.
    input.click("Routing", &["link"], false)?;
    wait("Routing page before secondary links", || {
        input.has("Routing", &["heading"])
    })?;
    input.click("Preview routes", &["link"], false)?;
    wait("Route preview page", || {
        input.has("Route preview", &["heading"])
    })?;
    input.stable("Route preview")?;
    emit(
        "settings_page",
        vec![
            ("name", s("Preview routes")),
            ("marker", s("Route preview")),
        ],
    );
    input.click("Routing", &["link"], false)?;
    wait("Routing page before files link", || {
        input.has("Routing", &["heading"])
    })?;
    input.click("Files and sources", &["link"], false)?;
    wait("Files and sources page", || {
        input.has("Files and sources", &["heading"])
    })?;
    input.stable("Files and sources")?;
    emit(
        "settings_page",
        vec![
            ("name", s("Files and sources")),
            ("marker", s("Files and sources")),
        ],
    );
    click("Back to workspace", "Inbox", &["link"], false)?;
    Ok(())
}
