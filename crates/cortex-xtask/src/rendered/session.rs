//! The infrastructure of the rendered journey: the private display (Openbox or headless Mutter), the native service, the shipped
//! desktop binary with the request-file probe, and the audits that read it. The journey itself is `flow.rs`.


use super::{pretty_block, pretty_line, AUDIT, POPOVERS, PROBE};
use crate::installed::input::Input;
use crate::installed::util::*;
use crate::installed::Cli;
use crate::json::{parse, Res, Value};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const SCREEN: (i64, i64) = (1920, 1200);
pub(super) const MONITOR: (i64, i64) = (1920, 1200);
pub(super) const WAYLAND_NAME: &str = "cw-full-wayland";
// The accepted stylesheet's values, read again from the app itself on every audit.
const TITLEBAR_H: f64 = 40.0;
const CHEVRON_MAX: f64 = 34.0;
const SHADOW_BLUR_MAX: f64 = 8.0;
/// The interface face of the design (docs/cortex-workspace/DESIGN.md): bundled by `packages/workspace-ui` `fonts.css` and named first by its `--font` token.
pub const DESIGN_FONT: &str = "Noto Sans Variable";
// What a dying process leaves on stderr, counted by name. Message bodies are never retained.
const STDERR_MARKERS: [&str; 16] = ["panic", "Segmentation", "Aborted", "core dumped", "CRITICAL", "WARNING", "EGL", "MESA", "Error reading events from display", "Broken pipe", "Lost connection", "Protocol error", "bwrap", "Operation not permitted", "out of memory", "WebProcess"];
const WEB_TERMINATION: [&str; 3] = ["crashed", "exceeded_memory_limit", "terminated_by_api"];

pub(super) struct Run {
    pub failures: Vec<String>,
    pub progress_step: String,
    pub passed: i64,
}

pub(super) fn outcome(run: &mut Run, step: &str, status: &str, details: Vec<(&str, Value)>) {
    if status == "passed" {
        run.progress_step = step.into();
        run.passed += 1;
    }
    let mut v = obj(vec![("step", s(step)), ("status", s(status))]);
    for (k, x) in details {
        v.set(k, x);
    }
    println!("{}", pretty_line(&v));
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

pub(super) fn fault(run: &mut Run, step: &str, details: Vec<(&str, Value)>) {
    run.failures.push(step.into());
    outcome(run, step, "failed", details);
}

pub(super) fn num(v: &Value, path: &str) -> f64 {
    let mut cur = Some(v);
    for part in path.split('.') {
        cur = cur.and_then(|c| match part.parse::<usize>() {
            Ok(i) => c.as_arr().and_then(|a| a.get(i)),
            Err(_) => c.get(part),
        });
    }
    match cur {
        Some(Value::Num(text)) => text.parse().unwrap_or(f64::NAN),
        _ => f64::NAN,
    }
}

pub(super) fn at<'a>(v: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = Some(v);
    for part in path.split('.') {
        cur = cur.and_then(|c| match part.parse::<usize>() {
            Ok(i) => c.as_arr().and_then(|a| a.get(i)),
            Err(_) => c.get(part),
        });
    }
    cur
}

pub(super) fn text<'a>(v: &'a Value, path: &str) -> &'a str {
    at(v, path).and_then(Value::as_str).unwrap_or("")
}

pub(super) fn flag(v: &Value, path: &str) -> bool {
    at(v, path).is_some_and(Value::truthy)
}

pub(super) fn count(v: &Value, path: &str) -> usize {
    at(v, path).and_then(Value::as_arr).map_or(0, <[Value]>::len)
}

/// Names every regression in one audit; an empty list means the screen passed.
pub fn judge(report: &Value) -> Vec<String> {
    let mut wrong = Vec::new();
    let (vw, vh) = (num(report, "page.viewport.0"), num(report, "page.viewport.1"));
    for part in ["html", "body", "root"] {
        if text(report, &format!("page.{part}")) != text(report, "page.surface") {
            wrong.push(format!("{part}_background_is_not_the_theme_surface"));
        }
    }
    for part in ["rootBox", "htmlBox", "bodyBox"] {
        let p = |k: &str| num(report, &format!("page.{part}.{k}"));
        if (p("x"), p("y"), p("w"), p("h")) != (0.0, 0.0, vw, vh) {
            wrong.push(format!("{part}_does_not_fill_the_viewport"));
        }
    }
    if text(report, "page.bodyMargin") != "0px" {
        wrong.push("body_has_a_margin".into());
    }
    if num(report, "page.scroll.0") > vw || num(report, "page.scroll.1") > vh {
        wrong.push("page_scrolls".into());
    }
    let bar = at(report, "chrome.barBox").filter(|b| b.truthy());
    if bar.is_none_or(|_| num(report, "chrome.barBox.y") != 0.0 || num(report, "chrome.barBox.h") != TITLEBAR_H || num(report, "chrome.barBox.w") != vw) {
        wrong.push("title_band_geometry".into());
    }
    if count(report, "chrome.outside") > 0 {
        wrong.push("control_outside_title_band".into());
    }
    if count(report, "chrome.overlaps") > 0 {
        wrong.push("title_band_controls_overlap".into());
    }
    if num(report, "chrome.winGroups") != 1.0 || num(report, "chrome.winButtons") != 3.0 || flag(report, "chrome.winHidden") {
        wrong.push("window_controls_are_not_one_real_group".into());
    }
    if at(report, "chrome.split").is_some_and(Value::truthy)
        && (count(report, "chrome.split.parts") != 2 || num(report, "chrome.split.parts.1.w") > CHEVRON_MAX || text(report, "chrome.split.display") == "grid")
    {
        wrong.push("split_chevron_stretched".into());
    }
    for (path, name) in [("light", "light_background_in_dark_theme"), ("native", "unstyled_native_control"), ("dashes", "dash_stands_for_missing_data"), ("clipped", "control_text_clipped")] {
        if count(report, path) > 0 {
            wrong.push(name.into());
        }
    }
    if num(report, "maxShadow") > SHADOW_BLUR_MAX {
        wrong.push("strong_shadow".into());
    }
    if num(report, "ui.plainIcons") > 0.0 {
        wrong.push("icon_lost_its_mask".into());
    }
    if text(report, "ui.userSelect") != "text" {
        wrong.push("page_text_is_not_selectable".into());
    }
    let faces: Vec<&str> = at(report, "fonts.faces").and_then(Value::as_obj).map(|o| o.iter().map(|(k, _)| k.as_str()).collect()).unwrap_or_default();
    let loaded: Vec<&str> = at(report, "fonts.loaded").and_then(Value::as_arr).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
    if faces.contains(&"Cousine") && !loaded.iter().any(|f| f.starts_with("Cousine")) {
        wrong.push("code_font_not_loaded".into());
    }
    if text(report, "fonts.stack") != DESIGN_FONT || !flag(report, "fonts.check") {
        wrong.push("design_font_not_first".into());
    }
    if !loaded.iter().any(|f| f.starts_with(DESIGN_FONT)) {
        wrong.push("design_font_not_loaded".into());
    }
    if num(report, "fonts.widths.design") == num(report, "fonts.widths.sans") {
        wrong.push("design_font_not_rendering".into());
    }
    if faces.iter().any(|f| *f != DESIGN_FONT && *f != "Cousine") {
        wrong.push("text_in_a_fallback_face".into());
    }
    wrong
}

/// A Popen return code as evidence: still running, a normal exit, or the signal that ended it.
pub fn exit_status(code: Option<std::process::ExitStatus>) -> Value {
    use std::os::unix::process::ExitStatusExt;
    match code {
        None => obj(vec![("state", s("running"))]),
        Some(status) => match (status.code(), status.signal()) {
            (Some(c), _) => obj(vec![("state", s("exited")), ("exit_code", n(c as i64))]),
            (None, Some(sig)) => obj(vec![("state", s("signalled")), ("signal", s(signal_name(sig)))]),
            _ => obj(vec![("state", s("exited"))]),
        },
    }
}

pub(super) fn signal_name(sig: i32) -> String {
    match sig {
        1 => "SIGHUP", 2 => "SIGINT", 3 => "SIGQUIT", 4 => "SIGILL", 6 => "SIGABRT", 7 => "SIGBUS", 8 => "SIGFPE", 9 => "SIGKILL", 11 => "SIGSEGV", 13 => "SIGPIPE", 15 => "SIGTERM",
        other => return other.to_string(),
    }
    .to_string()
}

/// Size and marker counts of a process's stderr, never its lines.
pub fn stderr_summary(path: &Path) -> Value {
    let Ok(raw) = std::fs::read(path) else { return obj(vec![("bytes", n(0)), ("lines", n(0)), ("markers", obj(vec![]))]) };
    let data = String::from_utf8_lossy(&raw).into_owned();
    let mut markers = obj(vec![]);
    for word in STDERR_MARKERS {
        let found = data.matches(word).count();
        if found > 0 {
            markers.set(word, n(found as i64));
        }
    }
    obj(vec![("bytes", n(raw.len() as i64)), ("lines", n(data.matches('\n').count() as i64)), ("markers", markers)])
}

/// Each core a dead process left, by name and size.
pub fn core_files(directory: &Path) -> Value {
    let mut found: Vec<(String, u64)> = std::fs::read_dir(directory)
        .map(|rd| rd.flatten().filter(|e| e.file_name().to_string_lossy().starts_with("core")).map(|e| (e.file_name().to_string_lossy().into_owned(), e.metadata().map(|m| m.len()).unwrap_or(0))).collect())
        .unwrap_or_default();
    found.sort();
    arr(found.into_iter().map(|(name, bytes)| obj(vec![("name", s(name)), ("bytes", n(bytes as i64))])).collect())
}

/// Every web-process-terminated WebKit reported to the probe, in order.
pub fn web_terminations(directory: &Path) -> Value {
    let text = std::fs::read_to_string(directory.join("web-process.log")).unwrap_or_default();
    arr(text.split_whitespace().map(|w| w.parse::<usize>().ok().and_then(|i| WEB_TERMINATION.get(i)).map_or_else(|| s(w), |name| s(*name))).collect())
}

/// Name and state of this process group's members and of any WebKit process, from /proc. Never a command line.
pub fn owned_processes(group: i32) -> Value {
    let mut found = Vec::new();
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) else { continue };
            let (Some(open), Some(close)) = (stat.find('('), stat.rfind(')')) else { continue };
            let fields: Vec<&str> = stat[close + 2..].split_whitespace().collect();
            if fields.len() > 2 && (fields[2].parse::<i32>().ok() == Some(group) || stat[open + 1..close].starts_with("WebKit")) {
                found.push(format!("{}:{}", &stat[open + 1..close], fields[0]));
            }
        }
    }
    found.sort();
    arr(found.into_iter().map(s).collect())
}

/// Which of the separate things went wrong, strongest evidence first. A compositor death takes a Wayland client with it, so it
/// is named before the app.
pub fn failure_class(app_exited: bool, terminated: bool, compositor_exited: bool, error: &str) -> Value {
    if compositor_exited {
        s("compositor_exited")
    } else if app_exited {
        s("app_exited")
    } else if terminated {
        s("web_process_terminated")
    } else if !error.is_empty() {
        s("journey_step_failed")
    } else {
        Value::Null
    }
}

pub(super) fn stop(child: &mut Option<Child>) {
    let Some(mut process) = child.take() else { return };
    unsafe { libc::killpg(process.id() as i32, libc::SIGTERM) };
    let end = Instant::now() + Duration::from_secs(8);
    while Instant::now() < end && process.try_wait().ok().flatten().is_none() {
        sleep_ms(50);
    }
    if process.try_wait().ok().flatten().is_none() {
        unsafe { libc::killpg(process.id() as i32, libc::SIGKILL) };
    }
    let _ = process.wait();
}

/// The request-file probe as a shared object: the one the image compiled when it was built, else compiled here (an image that
/// still carries a compiler).
pub(super) fn probe_library(scratch: &Path) -> Res<PathBuf> {
    let prebuilt = Path::new(super::PREBUILT).join("probe.so");
    if prebuilt.is_file() {
        return Ok(prebuilt);
    }
    let source = scratch.join("probe.c");
    write(&source, PROBE)?;
    let shim = scratch.join("probe.so");
    let flags = run(&["pkg-config", "--cflags", "--libs", "webkit2gtk-4.1", "gtk+-3.0"], Duration::from_secs(20))?.stdout;
    let mut gcc = vec!["gcc".to_string(), "-shared".into(), "-fPIC".into(), "-O1".into(), "-w".into(), "-o".into(), shim.to_string_lossy().into_owned(), source.to_string_lossy().into_owned()];
    gcc.extend(flags.split_whitespace().map(String::from));
    run(&gcc.iter().map(String::as_str).collect::<Vec<_>>(), Duration::from_secs(120))?;
    Ok(shim)
}

pub(super) fn channel_entry(checkout: &Path, name: &str) -> Res<Value> {
    let text = read_to_string(&checkout.join("crates/workspace-protocol/channels.json"))?;
    parse(&text)?.get(name).cloned().ok_or_else(|| format!("channel {name} is not defined"))
}

pub(super) fn xdotool(args: &[&str]) -> Res<String> {
    let mut all = vec!["xdotool"];
    all.extend_from_slice(args);
    Ok(run(&all, Duration::from_secs(30))?.stdout)
}

/// The running stack: display, window manager, service, desktop binary and the probe's request directory.
pub(super) struct Stack {
    pub scratch: PathBuf,
    pub probe_dir: PathBuf,
    pub shim: PathBuf,
    pub artifacts: PathBuf,
    pub checkout: PathBuf,
    pub channel: Value,
    pub xvfb: Option<Child>,
    pub wm: Option<Child>,
    pub service: Option<Child>,
    pub app: Option<Child>,
    pub sequence: u32,
    pub launches: u32,
    /// Native Wayland under headless Mutter instead of X11 with Openbox.
    pub wayland: bool,
    pub compositor: Option<Child>,
    pub pulse: Option<Child>,
    /// Keyboard and pointer for this backend: XTEST through xdotool, or Mutter's remote-desktop session.
    pub input: Option<Input>,
    /// Window closes the harness asked for. The product ends the web process on each close, so exactly one
    /// `terminated_by_api` is accounted for per request.
    pub closes: u32,
    pub launch_theme: String,
    /// The Wayland socket name this stack's compositor owns, and whether its app exposes accessibility.
    pub wayland_name: String,
    pub a11y: bool,
}

impl Stack {
    pub(super) fn new(scratch: PathBuf, probe_dir: PathBuf, shim: PathBuf, artifacts: PathBuf, checkout: PathBuf, channel: Value, wayland: bool) -> Stack {
        Stack { scratch, probe_dir, shim, artifacts, checkout, channel, xvfb: None, wm: None, service: None, app: None, sequence: 0, launches: 0, wayland, compositor: None, pulse: None, input: None, closes: 0, launch_theme: "dark".into(), wayland_name: WAYLAND_NAME.into(), a11y: true }
    }
}

impl Stack {
    pub(super) fn ns(&self, key: &str) -> String {
        text(&self.channel, key).to_string()
    }

    pub(super) fn start_display(&mut self) -> Res<()> {
        let mut fds = [0i32; 2];
        require(unsafe { libc::pipe(fds.as_mut_ptr()) } == 0, "pipe failed")?;
        let child = Command::new("Xvfb")
            .args(["-displayfd", &fds[1].to_string(), "-screen", "0", &format!("{}x{}x24", SCREEN.0, SCREEN.1), "-nolisten", "tcp"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|e| format!("Xvfb: {e}"))?;
        unsafe { libc::close(fds[1]) };
        let mut buf = [0u8; 32];
        let got = unsafe { libc::read(fds[0], buf.as_mut_ptr() as *mut _, 32) };
        unsafe { libc::close(fds[0]) };
        self.xvfb = Some(child);
        let display = format!(":{}", String::from_utf8_lossy(&buf[..got.max(0) as usize]).trim());
        std::env::set_var("DISPLAY", display);
        std::env::set_var("GDK_BACKEND", "x11");
        std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        std::env::set_var("NO_AT_BRIDGE", "0");
        std::env::set_var("GTK_A11Y", "always");
        self.wm = Some(Command::new("openbox").arg("--sm-disable").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn().map_err(|e| format!("openbox: {e}"))?);
        sleep_ms(1500);
        Ok(())
    }

    pub(super) fn start_service(&mut self) -> Res<()> {
        let log = std::fs::OpenOptions::new().create(true).append(true).open(self.scratch.join("service.log")).map_err(|e| e.to_string())?;
        self.service = Some(
            Command::new(self.artifacts.join(self.ns("service_binary")))
                .arg(std::env::var("HOME").map_err(|_| "HOME unset")?)
                .arg(&self.checkout)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(log)
                .process_group(0)
                .spawn()
                .map_err(|e| format!("service: {e}"))?,
        );
        Ok(())
    }

    pub(super) fn socket(&self) -> PathBuf {
        Path::new(&std::env::var("XDG_RUNTIME_DIR").unwrap_or_default()).join(self.ns("runtime_namespace")).join("service.sock")
    }

    /// Writes the dark/light choice GTK reads with no settings daemon; WebKit derives prefers-color-scheme from it: the real System path.
    pub(super) fn gtk_theme(&self, theme: &str) -> Res<()> {
        let config = PathBuf::from(std::env::var("XDG_CONFIG_HOME").map_err(|_| "XDG_CONFIG_HOME unset")?).join("gtk-3.0");
        std::fs::create_dir_all(&config).map_err(|e| e.to_string())?;
        write(&config.join("settings.ini"), &format!("[Settings]\ngtk-theme-name={}\n", if theme == "dark" { "Adwaita-dark" } else { "Adwaita" }))?;
        if self.wayland {
            // On Wayland GTK takes the theme from GSettings.
            let keyfile = config.parent().ok_or("XDG_CONFIG_HOME has no parent")?.join("glib-2.0/settings/keyfile");
            std::fs::create_dir_all(keyfile.parent().ok_or("no keyfile directory")?).map_err(|e| e.to_string())?;
            let dark = theme == "dark";
            write(&keyfile, &format!("[org/gnome/desktop/interface]\ngtk-theme='{}'\ncolor-scheme='{}'\n", if dark { "Adwaita-dark" } else { "Adwaita" }, if dark { "prefer-dark" } else { "prefer-light" }))?;
        }
        Ok(())
    }

    /// A real headless GNOME compositor, its window rules and its pointer: the owner's session type. GTK takes the theme from GSettings there, as on the owner's desktop.
    /// The environment of a Wayland compositor and its client: no X display, the compositor's socket and GSettings keyfile; accessibility
    /// on for the journey, off for the frame/control stage.
    pub(super) fn apply_env(&self, command: &mut Command) {
        command.env("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
        if !self.wayland {
            return;
        }
        command.env_remove("DISPLAY").env("GDK_BACKEND", "wayland").env("WAYLAND_DISPLAY", &self.wayland_name).env("GSETTINGS_BACKEND", "keyfile");
        if self.a11y {
            command.env("GTK_A11Y", "always").env("NO_AT_BRIDGE", "0");
        } else {
            command.env_remove("GTK_A11Y").env_remove("NO_AT_BRIDGE");
        }
    }

    pub(super) fn start_wayland(&mut self) -> Res<()> {
        self.gtk_theme("dark")?;
        let logs = self.scratch.join("logs");
        std::fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
        let sink = std::fs::File::create(logs.join(if self.a11y { "compositor.stderr" } else { "stage-compositor.stderr" })).map_err(|e| e.to_string())?;
        let mut command = Command::new("mutter");
        command
            .args(["--headless", "--wayland", "--no-x11", "--virtual-monitor", &format!("{}x{}", MONITOR.0, MONITOR.1), "--wayland-display", &self.wayland_name])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(sink)
            .process_group(0);
        self.apply_env(&mut command);
        self.compositor = Some(command.spawn().map_err(|e| format!("mutter: {e}"))?);
        if self.a11y {
            // The journey's own helpers (the OS clipboard owner and reader) are clients of this compositor too.
            std::env::remove_var("DISPLAY");
            std::env::remove_var("GDK_BACKEND");
            std::env::set_var("WAYLAND_DISPLAY", &self.wayland_name);
        }
        let socket = Path::new(&std::env::var("XDG_RUNTIME_DIR").unwrap_or_default()).join(&self.wayland_name);
        let mut gone = None;
        let up = wait_secs("Wayland compositor", 20, || {
            gone = self.compositor.as_mut().and_then(|c| c.try_wait().ok().flatten());
            Ok(socket.exists() || gone.is_some())
        });
        up?;
        if let Some(status) = gone {
            return Err(format!("wayland_compositor_failed:{}", line(&exit_status(Some(status)))));
        }
        // The keyboard and pointer are created before GTK enumerates the seat.
        self.input = Some(Input::new("wayland")?);
        Ok(())
    }

    pub(super) fn launch(&mut self, theme: &str) -> Res<()> {
        self.launch_theme = theme.into();
        self.gtk_theme(theme)?;
        let _ = std::fs::remove_file(self.probe_dir.join("request.js"));
        self.launches += 1;
        let logs = self.scratch.join("logs");
        std::fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
        let sink = std::fs::File::create(logs.join(format!("app-{}.stderr", self.launches))).map_err(|e| e.to_string())?;
        let mut command = Command::new(self.artifacts.join(self.ns("desktop_binary")));
        self.apply_env(&mut command);
        command.env("LD_PRELOAD", &self.shim).env("CW_PROBE_DIR", &self.probe_dir).stdin(Stdio::null()).stdout(Stdio::null()).stderr(sink).process_group(0);
        self.app = Some(command.spawn().map_err(|e| format!("desktop binary: {e}"))?);
        Ok(())
    }

    pub(super) fn app_status(&mut self) -> Option<std::process::ExitStatus> {
        self.app.as_mut().and_then(|a| a.try_wait().ok().flatten())
    }

    /// One request through the probe: a script body (or an `@native`, `@focus`, `@resize W H` verb) and its answer.
    pub(super) fn js(&mut self, body: &str, seconds: u64) -> Res<String> {
        let answer = self.probe_dir.join("response.json");
        let _ = std::fs::remove_file(&answer);
        self.sequence += 1;
        let temporary = self.probe_dir.join(format!("request-{}.tmp", self.sequence));
        write(&temporary, body)?;
        std::fs::rename(&temporary, self.probe_dir.join("request.js")).map_err(|e| e.to_string())?;
        let end = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < end {
            if answer.exists() {
                let reply = read_to_string(&answer)?;
                if let Some(error) = reply.strip_prefix('!') {
                    return Err(format!("probe_script_failed:{}", error.chars().take(160).collect::<String>()));
                }
                return Ok(reply);
            }
            if let Some(status) = self.app_status() {
                return Err(format!("app_exited_during_request:{}", line(&exit_status(Some(status)))));
            }
            sleep_ms(30);
        }
        Err("probe_no_answer".into())
    }

    pub(super) fn query(&mut self, body: &str, seconds: u64) -> Res<Value> {
        let reply = self.js(body, seconds)?;
        parse(&reply)
    }

    pub(super) fn wait_for(&mut self, step: &str, seconds: u64, mut check: impl FnMut(&mut Stack) -> Res<bool>) -> Res<()> {
        let end = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < end {
            if check(self)? {
                return Ok(());
            }
            sleep_ms(50);
        }
        Err(step.to_string())
    }

    pub(super) fn loaded(&mut self) -> Res<()> {
        self.wait_for("page_not_ready", 30, |st| Ok(st.js("return document.readyState === 'complete' && !!document.querySelector('.titlebar') ? 'yes' : ''", 5).is_ok_and(|a| a == "yes")))?;
        self.js("await document.fonts.ready; return 'yes';", 10).map(|_| ())
    }

    pub(super) fn window_id(&mut self) -> Res<String> {
        let pid = self.app.as_ref().map(|a| a.id()).ok_or("no app")?;
        let found = xdotool(&["search", "--all", "--onlyvisible", "--pid", &pid.to_string(), "--name", &format!("^{}$", text(&self.channel, "title"))])?;
        let ids: Vec<&str> = found.lines().collect();
        require(ids.len() == 1, "owned_window_identity_ambiguous")?;
        Ok(ids[0].to_string())
    }
}

pub(super) fn geometry(window: &str) -> Res<[i64; 4]> {
    let out = xdotool(&["getwindowgeometry", "--shell", window])?;
    let pick = |key: &str| -> Res<i64> { out.lines().find_map(|l| l.strip_prefix(&format!("{key}="))).and_then(|v| v.trim().parse().ok()).ok_or_else(|| format!("geometry {key}")) };
    Ok([pick("X")?, pick("Y")?, pick("WIDTH")?, pick("HEIGHT")?])
}

pub(super) fn xprop(window: &str, name: &str) -> String {
    run(&["xprop", "-id", window, name], Duration::from_secs(10)).map(|c| c.stdout.trim().to_string()).unwrap_or_default()
}

pub(super) fn maximized(window: &str) -> bool {
    let state = xprop(window, "_NET_WM_STATE");
    state.contains("_NET_WM_STATE_MAXIMIZED_VERT") && state.contains("_NET_WM_STATE_MAXIMIZED_HORZ")
}

pub(super) fn pointer(window: &str, x: f64, y: f64, actions: &[&str]) -> Res<()> {
    let g = geometry(window)?;
    let mut args = vec!["mousemove".to_string(), (g[0] + x.round() as i64).to_string(), (g[1] + y.round() as i64).to_string()];
    args.extend(actions.iter().map(|a| a.to_string()));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    xdotool(&refs)?;
    sleep_ms(80);
    Ok(())
}

pub(super) fn drag(window: &str, start: (f64, f64), by: (i64, i64)) -> Res<()> {
    pointer(window, start.0, start.1, &[])?;
    xdotool(&["mousedown", "1"])?;
    sleep_ms(100);
    let g = geometry(window)?;
    let (ax, ay) = (g[0] + start.0.round() as i64, g[1] + start.1.round() as i64);
    for step in 1..=8 {
        xdotool(&["mousemove", &(ax + by.0 * step / 8).to_string(), &(ay + by.1 * step / 8).to_string()])?;
        sleep_ms(30);
    }
    xdotool(&["mouseup", "1"])?;
    sleep_ms(300);
    Ok(())
}

/// One audit: the page's numbers judged by `judge`; a failure names every broken rule.
pub(super) fn audit(run_: &mut Run, st: &mut Stack, label: &str, theme: &str) -> Res<Value> {
    sleep_ms(150);
    let report = st.query(AUDIT, 10)?;
    let mut wrong = judge(&report);
    if text(&report, "page.theme") != theme {
        wrong.push("theme_is_not_the_system_theme".into());
    }
    if !label.contains("settings") && count(&report, "themed") > 0 {
        wrong.push("theme_control_outside_settings".into());
    }
    if wrong.is_empty() {
        let g = |path: &str| at(&report, path).cloned().unwrap_or(Value::Null);
        outcome(
            run_,
            "visual_audit",
            "passed",
            vec![
                ("screen", s(label)),
                ("theme", g("page.theme")),
                ("surface", g("page.surface")),
                ("viewport", g("page.viewport")),
                ("root", g("page.rootBox")),
                ("bar_h", g("chrome.barBox.h")),
                ("bar_controls", g("chrome.controls")),
                ("win_buttons", g("chrome.winButtons")),
                ("faces", g("fonts.faces")),
                ("loaded", g("fonts.loaded")),
                ("shadow_blur_max", g("maxShadow")),
                ("strips", g("ui.strips")),
                ("main", g("ui.mainText")),
                ("classes_without_a_rule", g("unknown")),
            ],
        );
    } else {
        let g = |path: &str| at(&report, path).cloned().unwrap_or(Value::Null);
        fault(
            run_,
            "visual_audit",
            vec![
                ("screen", s(label)),
                ("wrong", arr(wrong.iter().map(|w| s(w.as_str())).collect())),
                ("light", g("light")),
                ("native", g("native")),
                ("dashes", g("dashes")),
                ("clipped", g("clipped")),
                ("themed", g("themed")),
                ("plain_icons", g("ui.plainIconDetail")),
                ("outside", g("chrome.outside")),
                ("overlaps", g("chrome.overlaps")),
                ("page", g("page")),
                ("chrome", g("chrome")),
                ("fonts", g("fonts")),
            ],
        );
    }
    Ok(report)
}

pub(super) fn popovers(run_: &mut Run, st: &mut Stack, label: &str) -> Res<()> {
    let results = st.query(POPOVERS, 20)?;
    let rows = results.as_arr().unwrap_or(&[]);
    let bad: Vec<&Value> = rows
        .iter()
        .filter(|p| !(flag(p, "supported") && flag(p, "open") && text(p, "display") != "none" && num(p, "rect.2") > 0.0 && num(p, "rect.3") > 0.0 && flag(p, "inside") && num(p, "items") > 0.0))
        .collect();
    if !bad.is_empty() || rows.is_empty() {
        fault(run_, "popovers", vec![("screen", s(label)), ("results", results.clone()), ("bad", arr(bad.iter().map(|p| at(p, "id").cloned().unwrap_or(Value::Null)).collect()))]);
    } else {
        let opened = arr(rows.iter().map(|p| obj(vec![("id", at(p, "id").cloned().unwrap_or(Value::Null)), ("rect", at(p, "rect").cloned().unwrap_or(Value::Null)), ("items", at(p, "items").cloned().unwrap_or(Value::Null))])).collect());
        outcome(run_, "popovers", "passed", vec![("screen", s(label)), ("opened", opened)]);
    }
    Ok(())
}

pub(super) fn nav(st: &mut Stack, label: &str) -> Res<()> {
    let wanted = crate::json::quote(label, true);
    let found = st.js(&format!("const el = [...document.querySelectorAll('.sidebar .nav')].find(e => e.textContent.trim().startsWith({wanted})); if (!el) return 'no'; el.click(); return 'yes';"), 10)?;
    require(found == "yes", &format!("nav_missing:{label}"))?;
    sleep_ms(200);
    Ok(())
}

pub(super) fn click_control(st: &mut Stack, window: &str, selector: &str) -> Res<()> {
    let centre = st.query(&format!("const r = document.querySelector({}).getBoundingClientRect(); return JSON.stringify([r.x + r.width / 2, r.y + r.height / 2]);", crate::json::quote(selector, true)), 10)?;
    pointer(window, num(&centre, "0"), num(&centre, "1"), &["click", "1"])?;
    sleep_ms(350);
    Ok(())
}

pub(super) fn band_blank(st: &mut Stack) -> Res<(f64, f64)> {
    let blank = st.query("const b = document.querySelector('.tb-main').getBoundingClientRect(), g = document.querySelector('.tb-main > .grow').getBoundingClientRect(); return JSON.stringify([g.x + g.width / 2, b.y + b.height / 2]);", 10)?;
    Ok((num(&blank, "0"), num(&blank, "1")))
}

pub(super) fn window_behaviour(run_: &mut Run, st: &mut Stack) -> Res<()> {
    let window = st.window_id()?;
    let hints = xprop(&window, "_MOTIF_WM_HINTS");
    let extents = xprop(&window, "_NET_FRAME_EXTENTS");
    let native = parse(&st.js("@native", 10)?)?;
    let background: Vec<f64> = (0..3).map(|i| num(&native, &format!("background.{i}"))).collect();
    if background != [24.0, 27.0, 26.0] {
        fault(run_, "window_background_is_not_the_surface", vec![("native", native.clone())]);
    }
    // `_MOTIF_WM_HINTS = flags, functions, decorations, ...`: decorations off in the third field.
    let fields: Vec<u64> = hints.split('=').nth(1).map(|t| t.split(',').filter_map(|p| u64::from_str_radix(p.trim().trim_start_matches("0x"), 16).ok()).collect()).unwrap_or_default();
    let no_frame = fields.len() >= 3 && fields[0] & 2 != 0 && fields[2] == 0;
    if !no_frame || flag(&native, "decorated") || line(at(&native, "top").unwrap_or(&Value::Null)) != line(at(&native, "web").unwrap_or(&Value::Null)) {
        fault(run_, "os_decoration_present", vec![("hints", s(hints.as_str())), ("extents", s(extents.as_str())), ("native", native.clone())]);
    } else {
        outcome(run_, "no_os_decoration", "passed", vec![("hints", s(hints.as_str())), ("extents", s(extents.as_str())), ("native", native.clone())]);
    }
    let start = geometry(&window)?;
    let view = st.query("return JSON.stringify([innerWidth, innerHeight]);", 10)?;
    if [start[2] as f64, start[3] as f64] != [num(&view, "0"), num(&view, "1")] {
        fault(run_, "webview_does_not_fill_window", vec![("window", arr(start.iter().map(|v| n(*v)).collect())), ("viewport", view)]);
    }
    // The real buttons, with the real pointer, through a real window manager.
    click_control(st, &window, ".win button[aria-label=\"Maximize\"]")?;
    let w = window.clone();
    st.wait_for("maximize_button", 15, |_| Ok(maximized(&w)))?;
    let full = geometry(&window)?;
    st.wait_for("restore_label", 15, |st| Ok(st.js("return document.querySelector('.win button:nth-child(2)').getAttribute('aria-label') === 'Restore' ? 'Restore' : ''", 10)? == "Restore"))?;
    let filled = st.query("const r = document.getElementById('root').getBoundingClientRect(); return JSON.stringify([r.width, r.height, innerWidth, innerHeight]);", 10)?;
    outcome(run_, "maximize", "passed", vec![("window", arr(full.iter().map(|v| n(*v)).collect())), ("root_and_viewport", filled.clone())]);
    if (full[2], full[3]) != SCREEN || (num(&filled, "0"), num(&filled, "1")) != (SCREEN.0 as f64, SCREEN.1 as f64) {
        fault(run_, "maximize_does_not_fill", vec![("window", arr(full.iter().map(|v| n(*v)).collect())), ("root", filled)]);
    }
    click_control(st, &window, ".win button[aria-label=\"Restore\"]")?;
    let w = window.clone();
    st.wait_for("restore_button", 15, |_| Ok(!maximized(&w)))?;
    outcome(run_, "restore", "passed", vec![("window", arr(geometry(&window)?.iter().map(|v| n(*v)).collect()))]);
    click_control(st, &window, ".win button[aria-label=\"Minimize\"]")?;
    let w = window.clone();
    st.wait_for("minimize_button", 15, |_| Ok(xprop(&w, "_NET_WM_STATE").contains("_NET_WM_STATE_HIDDEN")))?;
    outcome(run_, "minimize", "passed", vec![("state", s(xprop(&window, "_NET_WM_STATE")))]);
    let _ = xdotool(&["windowactivate", "--sync", &window]);
    let w = window.clone();
    st.wait_for("unminimize", 15, |_| Ok(!xprop(&w, "_NET_WM_STATE").contains("_NET_WM_STATE_HIDDEN")))?;

    xdotool(&["windowmove", &window, "100", "60"])?;
    xdotool(&["windowsize", &window, "1300", "860"])?;
    sleep_ms(400);
    let before = geometry(&window)?;
    let blank = band_blank(st)?;
    drag(&window, blank, (140, 90))?;
    let after = geometry(&window)?;
    outcome(run_, "band_drag", "passed", vec![("before", arr(before.iter().map(|v| n(*v)).collect())), ("after", arr(after.iter().map(|v| n(*v)).collect()))]);
    // The window follows from where the pointer is when the slop is crossed, so a quick drag lands a step short.
    if (after[0] - before[0] - 140).abs() > 30 || (after[1] - before[1] - 90).abs() > 30 || after[2..] != before[2..] {
        fault(run_, "band_drag_failed", vec![("before", arr(before.iter().map(|v| n(*v)).collect())), ("after", arr(after.iter().map(|v| n(*v)).collect()))]);
    }
    // A press after the drag must reach the page at once.
    let pressed = st.js("return document.querySelector('.tb-toggle').getAttribute('aria-pressed');", 10)?;
    click_control(st, &window, ".tb-toggle")?;
    if st.js("return document.querySelector('.tb-toggle').getAttribute('aria-pressed');", 10)? == pressed {
        fault(run_, "first_click_after_drag_lost", vec![]);
    }
    let ring = st.query("const a = document.activeElement, s = getComputedStyle(a); return JSON.stringify({ active: a.tagName, focusVisible: a.matches(':focus-visible'), outline: s.outlineStyle });", 10)?;
    if flag(&ring, "focusVisible") || text(&ring, "outline") != "none" {
        fault(run_, "focus_ring_stays_after_a_mouse_click", vec![("ring", ring)]);
    } else {
        outcome(run_, "no_focus_ring_after_mouse_click", "passed", vec![("active", at(&ring, "active").cloned().unwrap_or(Value::Null)), ("focusVisible", b(false)), ("outline", s("none"))]);
    }
    click_control(st, &window, ".tb-toggle")?;
    let held = geometry(&window)?;
    let toggle = st.query("const r = document.querySelector('.tb-toggle').getBoundingClientRect(); return JSON.stringify([r.x + r.width / 2, r.y + r.height / 2]);", 10)?;
    drag(&window, (num(&toggle, "0"), num(&toggle, "1")), (120, 60))?;
    if geometry(&window)?[..2] != held[..2] {
        fault(run_, "control_in_band_dragged_the_window", vec![("before", arr(held.iter().map(|v| n(*v)).collect()))]);
    } else {
        outcome(run_, "controls_do_not_drag", "passed", vec![("at", arr(held.iter().map(|v| n(*v)).collect()))]);
    }
    if st.js("return document.querySelector('.tb-toggle').getAttribute('aria-pressed');", 10)? != pressed {
        click_control(st, &window, ".tb-toggle")?;
    }
    pointer(&window, blank.0, blank.1, &["click", "--repeat", "2", "--delay", "90", "1"])?;
    let w = window.clone();
    st.wait_for("double_click_maximize", 15, |_| Ok(maximized(&w)))?;
    outcome(run_, "double_click_maximizes", "passed", vec![("window", arr(geometry(&window)?.iter().map(|v| n(*v)).collect()))]);
    let blank_full = band_blank(st)?;
    pointer(&window, blank_full.0, blank_full.1, &["click", "--repeat", "2", "--delay", "90", "1"])?;
    let w = window.clone();
    st.wait_for("double_click_restore", 15, |_| Ok(!maximized(&w)))?;
    sleep_ms(300);
    for (edge, by, expect) in [("right", (90i64, 0i64), (90i64, 0i64)), ("bottom", (0, 70), (0, 70))] {
        let now = geometry(&window)?;
        let start = if edge == "right" { ((now[2] - 2) as f64, now[3] as f64 / 2.0) } else { (now[2] as f64 / 2.0, (now[3] - 2) as f64) };
        drag(&window, start, by)?;
        let grown = geometry(&window)?;
        outcome(run_, "edge_resize", "passed", vec![("edge", s(edge)), ("before", arr(now.iter().map(|v| n(*v)).collect())), ("after", arr(grown.iter().map(|v| n(*v)).collect()))]);
        if (grown[2] - now[2] - expect.0).abs() > 6 || (grown[3] - now[3] - expect.1).abs() > 6 {
            fault(run_, "edge_resize_failed", vec![("edge", s(edge))]);
        }
    }
    let now = geometry(&window)?;
    drag(&window, (2.0, 2.0), (-30, -20))?;
    let grown = geometry(&window)?;
    outcome(run_, "edge_resize", "passed", vec![("edge", s("top_left")), ("before", arr(now.iter().map(|v| n(*v)).collect())), ("after", arr(grown.iter().map(|v| n(*v)).collect()))]);
    if grown[2] - now[2] < 20 || grown[3] - now[3] < 12 || grown[0] >= now[0] {
        fault(run_, "edge_resize_failed", vec![("edge", s("top_left"))]);
    }
    xdotool(&["windowsize", &window, "960", "540"])?;
    sleep_ms(500);
    audit(run_, st, "home at minimum size", "dark")?;
    xdotool(&["windowsize", &window, "1440", "900"])?;
    sleep_ms(400);
    Ok(())
}

/// Grab the scrollbar and the window edge of the real desktop window with real pointer input.
///
/// Runs inside the rendered-check image against a built window and service binary, on a private Xvfb with Openbox, a
/// disposable HOME and no network. Pointer input is XTEST through xdotool, so it passes through GTK and the undecorated-window
/// edge handlers exactly like a mouse. The same read-only probe as the rendered journey reports positions; it dispatches nothing.
pub fn scrollbar(cli: &Cli) -> Res<i32> {
    let artifacts = PathBuf::from(cli.flag("artifacts").unwrap_or("/artifacts"));
    let checkout = PathBuf::from(cli.flag("checkout").unwrap_or("/checkout"));
    let evidence = PathBuf::from(cli.flag("evidence").unwrap_or("/evidence"));
    let label = cli.positional.first().cloned().unwrap_or_else(|| "new".into());
    let scratch_arg = cli.flag("scratch");
    let Some(scratch) = scratch_arg else {
        // The wrapper: a private session bus and disposable directories, then this command again inside them.
        let tmp = crate::proc::TempDir::new("cw-scroll-")?;
        for name in ["home", "config", "state", "cache", "runtime"] {
            std::fs::create_dir(tmp.0.join(name)).map_err(|e| e.to_string())?;
            chmod(&tmp.0.join(name), 0o700)?;
        }
        let me = std::env::current_exe().map_err(|e| e.to_string())?;
        let status = Command::new("dbus-run-session")
            .env_clear()
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .env("LANG", "C.UTF-8")
            .env("HOME", tmp.0.join("home"))
            .env("XDG_CONFIG_HOME", tmp.0.join("config"))
            .env("XDG_STATE_HOME", tmp.0.join("state"))
            .env("XDG_CACHE_HOME", tmp.0.join("cache"))
            .env("XDG_RUNTIME_DIR", tmp.0.join("runtime"))
            .args(["--"])
            .arg(&me)
            .arg("rendered-scrollbar")
            .arg(&label)
            .arg("--artifacts")
            .arg(&artifacts)
            .arg("--checkout")
            .arg(&checkout)
            .arg("--evidence")
            .arg(&evidence)
            .arg("--scratch")
            .arg(&tmp.0)
            .status()
            .map_err(|e| e.to_string())?;
        return Ok(status.code().unwrap_or(1));
    };
    let scratch = PathBuf::from(scratch);
    let channel = channel_entry(&checkout, "dev")?;
    let probe_dir = scratch.join("probe");
    std::fs::create_dir_all(&probe_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(scratch.join("projects")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(scratch.join("logs")).map_err(|e| e.to_string())?;
    let shim = probe_library(&scratch)?;
    let mut st = Stack::new(scratch.clone(), probe_dir, shim, artifacts, checkout, channel, false);
    let mut results = obj(vec![("binary", s(label.as_str()))]);
    let outcome_ = (|| -> Res<()> {
        st.start_display()?;
        st.start_service()?;
        let socket = st.socket();
        wait_secs("service socket", 20, || Ok(socket.exists()))?;
        results.set("service_up", b(socket.exists()));
        st.launch("dark")?;
        st.wait_for("page_not_ready", 30, |st| Ok(st.js("return document.readyState === 'complete' && document.querySelector('.titlebar') ? 'yes' : ''", 15).is_ok_and(|a| a == "yes")))?;
        st.js("await document.fonts.ready; return 'yes';", 15)?;
        let window = st.window_id()?;
        xdotool(&["windowsize", "--sync", &window, "1100", "640"])?;
        xdotool(&["windowmove", &window, "100", "60"])?;
        sleep_ms(1000);
        let g = geometry(&window)?;
        results.set("window", arr(g.iter().map(|v| n(*v)).collect()));
        // The Settings link, then the longest settings page, by real clicks.
        let click = |st: &mut Stack, body: &str| -> Res<()> {
            let spot = st.query(body, 15)?;
            let g = geometry(&window)?;
            xdotool(&["mousemove", "--sync", &(g[0] + num(&spot, "0") as i64).to_string(), &(g[1] + num(&spot, "1") as i64).to_string()])?;
            sleep_ms(200);
            xdotool(&["click", "1"])?;
            sleep_ms(500);
            Ok(())
        };
        click(&mut st, "const el=document.querySelector('a[aria-label=\"Settings\"]');const r=el.getBoundingClientRect();return JSON.stringify([r.x+r.width/2,r.y+r.height/2])")?;
        st.wait_for("settings", 30, |st| Ok(st.js("return document.querySelector('.set-main') ? 'yes' : ''", 15).is_ok_and(|a| a == "yes")))?;
        click(&mut st, "const a=[...document.querySelectorAll('.set-nav a, .set-side a, nav a')].find(a=>/Shortcuts/.test(a.textContent));const r=a.getBoundingClientRect();return JSON.stringify([r.x+r.width/2,r.y+r.height/2])")?;
        st.wait_for("shortcuts", 30, |st| Ok(st.js("return document.querySelector('.set-main') && /Shortcuts/.test(document.querySelector('.set-main').textContent) ? 'yes' : ''", 15).is_ok_and(|a| a == "yes")))?;
        sleep_ms(500);
        // Test scaffolding only: this disposable page must be longer than the window. Input still comes from the real pointer.
        st.js("const f=document.createElement('div');f.style.height='2400px';document.querySelector('.set-main').append(f);return 'yes';", 15)?;
        sleep_ms(300);
        let state = "const el=document.querySelector('.set-main');const r=el.getBoundingClientRect();return JSON.stringify({top:el.scrollTop,sh:el.scrollHeight,ch:el.clientHeight,left:r.left,right:r.right,bar:el.offsetWidth-el.clientWidth,iw:innerWidth,ih:innerHeight,edge:getComputedStyle(document.documentElement).getPropertyValue('--ws-edge'),dpr:devicePixelRatio})";
        let before = st.query(state, 15)?;
        results.set("before", before.clone());
        require(num(&before, "sh") > num(&before, "ch") + 50.0, "page must scroll for this check")?;
        // The former driver also saved screenshots of the private screen as evidence; no check reads them, and this harness takes none.
        results.set("screenshots", s("not_taken"));
        // The thumb is 5px wide, 3px in from each side of the 11px bar: grab both sides of it.
        let bar_left = num(&before, "right") - num(&before, "bar");
        let thumb_top = num(&before, "ch") * num(&before, "top") / num(&before, "sh");
        let g0 = geometry(&window)?;
        let mut attempts = obj(vec![]);
        for (side, offset) in [("thumb-left", 3.0), ("thumb-right", num(&before, "bar") - 4.0)] {
            let s0 = st.query(state, 15)?;
            let w0 = geometry(&window)?;
            let (grab_x, grab_y) = ((g0[0] as f64 + bar_left + offset) as i64, g0[1] + 150 + thumb_top as i64);
            // A hand drifts sideways while dragging; a pure vertical move would hide an edge resize.
            pointer_abs(grab_x, grab_y)?;
            sleep_ms(200);
            xdotool(&["mousedown", "1"])?;
            sleep_ms(200);
            for i in 1..=12 {
                pointer_abs(grab_x + (-8) * i / 12, grab_y + 90 * i / 12)?;
                sleep_ms(30);
            }
            sleep_ms(200);
            xdotool(&["mouseup", "1"])?;
            sleep_ms(500);
            let s1 = st.query(state, 15)?;
            let w1 = geometry(&window)?;
            attempts.set(
                side,
                obj(vec![
                    ("scrollDelta", Value::Num(format!("{}", num(&s1, "top") - num(&s0, "top")))),
                    ("grabbedAt", n(grab_x - g0[0])),
                    ("windowFrom", arr(w0.iter().map(|v| n(*v)).collect())),
                    ("windowTo", arr(w1.iter().map(|v| n(*v)).collect())),
                    ("resized", b(w0 != w1)),
                ]),
            );
            pointer_abs(5, 5)?;
            sleep_ms(200);
            if w0 == w1 {
                // Back to the top for the next grab, by real wheel.
                xdotool(&["click", "--repeat", "20", "--delay", "10", "4"])?;
                sleep_ms(300);
            }
        }
        results.set("scrollbar", attempts.clone());
        // The right edge itself still resizes the window.
        let e0 = geometry(&window)?;
        let edge_x = e0[0] + e0[2] - 2;
        pointer_abs(edge_x, e0[1] + e0[3] / 2)?;
        sleep_ms(200);
        xdotool(&["mousedown", "1"])?;
        sleep_ms(200);
        for i in 1..=12 {
            pointer_abs(edge_x + 60 * i / 12, e0[1] + e0[3] / 2)?;
            sleep_ms(30);
        }
        sleep_ms(200);
        xdotool(&["mouseup", "1"])?;
        sleep_ms(500);
        let e1 = geometry(&window)?;
        results.set("edge_resize_right", obj(vec![("from", arr(e0.iter().map(|v| n(*v)).collect())), ("to", arr(e1.iter().map(|v| n(*v)).collect()))]));
        let ok = attempts.as_obj().unwrap_or(&[]).iter().all(|(_, a)| num(a, "scrollDelta") > 40.0 && !flag(a, "resized")) && (e1[2] - e0[2] >= 40);
        results.set("ok", b(ok));
        results.set("after", st.query(state, 15)?);
        Ok(())
    })();
    if let Err(error) = outcome_ {
        results.set("error", s(error));
        results.set("ok", b(false));
    }
    for child in [&mut st.app, &mut st.service, &mut st.wm, &mut st.xvfb] {
        stop(child);
    }
    let rendered = pretty_block(&results, 2);
    std::fs::create_dir_all(&evidence).map_err(|e| e.to_string())?;
    write(&evidence.join(format!("scrollbar-{label}.json")), &format!("{rendered}\n"))?;
    println!("{}", pretty_line(&results));
    Ok(if flag(&results, "ok") { 0 } else { 1 })
}

pub(super) fn pointer_abs(x: i64, y: i64) -> Res<()> {
    xdotool(&["mousemove", "--sync", &x.to_string(), &y.to_string()]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;

    #[test]
    fn failure_class_names_the_strongest_evidence() {
        assert_eq!(failure_class(true, true, true, "x").as_str(), Some("compositor_exited"));
        assert_eq!(failure_class(true, true, false, "x").as_str(), Some("app_exited"));
        assert_eq!(failure_class(false, true, false, "x").as_str(), Some("web_process_terminated"));
        assert_eq!(failure_class(false, false, false, "step failed").as_str(), Some("journey_step_failed"));
        assert!(matches!(failure_class(false, false, false, ""), Value::Null));
    }

    #[test]
    fn exit_status_names_the_signal() {
        assert_eq!(line(&exit_status(None)), line(&obj(vec![("state", s("running"))])));
        assert_eq!(text(&exit_status(Some(std::process::ExitStatus::from_raw(0))), "state"), "exited");
        let killed = exit_status(Some(std::process::ExitStatus::from_raw(11)));
        assert_eq!((text(&killed, "state"), text(&killed, "signal")), ("signalled", "SIGSEGV"));
    }

    #[test]
    fn stderr_summary_keeps_counts_and_never_the_text() {
        let dir = crate::proc::TempDir::new("rendered-test-").unwrap();
        let path = dir.0.join("app.stderr");
        std::fs::write(&path, "secret thing panic\nEGL EGL\n").unwrap();
        let summary = stderr_summary(&path);
        assert_eq!(num(&summary, "lines"), 2.0);
        assert_eq!(num(&summary, "markers.panic"), 1.0);
        assert_eq!(num(&summary, "markers.EGL"), 2.0);
        assert!(!line(&summary).contains("secret"));
        assert_eq!(num(&stderr_summary(&dir.0.join("missing")), "bytes"), 0.0);
    }

    #[test]
    fn web_terminations_are_read_in_order_and_cores_by_name() {
        let dir = crate::proc::TempDir::new("rendered-test-").unwrap();
        std::fs::write(dir.0.join("web-process.log"), "2\n0\n1\n").unwrap();
        assert_eq!(line(&web_terminations(&dir.0)), r#"["terminated_by_api", "crashed", "exceeded_memory_limit"]"#);
        std::fs::write(dir.0.join("core.123"), "xxxx").unwrap();
        assert_eq!(text(&core_files(&dir.0), "0.name"), "core.123");
        assert_eq!(num(&core_files(&dir.0), "0.bytes"), 4.0);
    }

    #[test]
    fn the_probe_listens_for_web_process_death() {
        assert!(PROBE.contains("g_signal_connect(web_view, \"web-process-terminated\""));
    }

    fn report(font: &str, theme_surface: &str) -> Value {
        parse(&format!(
            r#"{{"page":{{"theme":"dark","surface":"{theme_surface}","viewport":[1440,900],"html":"{theme_surface}","body":"{theme_surface}","root":"{theme_surface}","rootBox":{{"x":0,"y":0,"w":1440,"h":900}},"htmlBox":{{"x":0,"y":0,"w":1440,"h":900}},"bodyBox":{{"x":0,"y":0,"w":1440,"h":900}},"bodyMargin":"0px","scroll":[1440,900]}},
            "chrome":{{"barBox":{{"x":0,"y":0,"w":1440,"h":40}},"outside":[],"overlaps":[],"controls":8,"winGroups":1,"winButtons":3,"winHidden":false,"split":null}},
            "fonts":{{"stack":"{font}","faces":{{"{font}":12}},"loaded":["{font} 400"],"check":true,"widths":{{"design":120.5,"sans":114.25,"medium":122}}}},
            "ui":{{"plainIcons":0,"userSelect":"text"}},"light":[],"native":[],"dashes":[],"clipped":[],"maxShadow":4}}"#
        ))
        .unwrap()
    }

    #[test]
    fn a_sound_screen_passes_and_every_broken_rule_is_named() {
        assert!(judge(&report("Noto Sans Variable", "rgb(24, 27, 26)")).is_empty());
        // The interface face is Noto Sans Variable; the rule stays strict, so the face the design no longer uses is a regression.
        let wrong = judge(&report("Roboto Variable", "rgb(24, 27, 26)"));
        assert!(wrong.contains(&"design_font_not_first".to_string()) && wrong.contains(&"text_in_a_fallback_face".to_string()));
        let mut broken = report("Noto Sans Variable", "rgb(24, 27, 26)");
        broken.set("maxShadow", n(12));
        broken.set("dashes", arr(vec![s("span.x")]));
        let wrong = judge(&broken);
        assert!(wrong.contains(&"strong_shadow".to_string()) && wrong.contains(&"dash_stands_for_missing_data".to_string()));
    }
}
