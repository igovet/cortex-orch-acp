//! `paseo-dev`: run a Paseo CLI command, or the Paseo Electron desktop (`gui`),
//! against the explicitly isolated dev home (default `~/.paseo-dev`).
//!
//! Guards preserved from the wrapper's contract: the production home `~/.paseo` and
//! the production Electron profile `~/.config/Paseo` are refused; a command may not
//! override `--home` or select `--host`; every inherited `PASEO_*` variable is dropped
//! (a wrapper started from inside a Paseo process must not target that installation);
//! Cortex's own XDG roots live inside the isolated home; a long-lived Paseo process is
//! refused inside a Cortex command sandbox; a Git-sourced plugin install or update
//! needs `--installed-runtime`. Removed with the Python runtime: `--wire-trace`,
//! `route-preflight` and `binding`.
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};

const DEFAULT_PASEO: &str = "/opt/Paseo/resources/bin/paseo";
const DEFAULT_GUI: &str = "/opt/Paseo/Paseo";
const DAEMON_LAUNCHES: [&[&str]; 4] = [
    &["start"],
    &["onboard"],
    &["daemon", "start"],
    &["daemon", "run"],
];
const ENGINE_ACCOUNT_DIRS: [&str; 2] = [".codex", ".claude"];
/// Tests only: an alternate mountinfo file to inspect.
pub const MOUNTINFO_OVERRIDE: &str = "CORTEX_PASEO_DEV_MOUNTINFO";
const INSTALLED_RUNTIME_HELP: &str = "do not pass this checkout as CORTEX_PASEO_SOURCE_ROOT, so a Git-installed plugin \
launches the runtime its own install staged; required for Git plugin install/update and for the daemon start that serves them";
const USAGE: &str = "usage: paseo-dev [--paseo ABS_PATH] [--home ABS_PATH] [--installed-runtime] [--check] -- <paseo command...>
       paseo-dev gui [--paseo ABS_PATH] [--home ABS_PATH] [--user-data-dir ABS_PATH] [--port N] [--xvfb] [--installed-runtime]
  --installed-runtime: do not pass this checkout as CORTEX_PASEO_SOURCE_ROOT (for Git plugin install/update and the daemon that serves it)";

type Res<T> = Result<T, String>;

fn user_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/nonexistent"))
}

/// Python `Path.resolve(strict=False)`: follow existing links, keep a lexical tail for what does not exist.
fn resolve(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normal.pop();
            }
            Component::CurDir => {}
            other => normal.push(other.as_os_str()),
        }
    }
    let mut existing = normal.as_path();
    let mut tail: Vec<&std::ffi::OsStr> = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            return tail.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                tail.push(name);
                existing = parent;
            }
            _ => return normal,
        }
    }
}

fn resolved_absolute(text: &str, flag: &str) -> Res<PathBuf> {
    let expanded = match text.strip_prefix("~/") {
        Some(rest) => user_home().join(rest),
        None if text == "~" => user_home(),
        None => PathBuf::from(text),
    };
    if !expanded.is_absolute() {
        return Err(format!("{flag} must be an absolute path"));
    }
    Ok(resolve(&expanded))
}

fn checkout_root() -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    root.canonicalize().unwrap_or(root)
}

/// Keep a dev Paseo process out of the user's normal Cortex XDG state and out of any
/// Paseo installation it may have been started from.
pub fn isolated_environment(home: &Path, checkout_runtime: bool) -> BTreeMap<OsString, OsString> {
    let mut env: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    for key in [
        "CORTEX_DEV_WIRE_TRACE",
        "CORTEX_DEV_TRACE_HOME",
        "CORTEX_PASEO_SOURCE_ROOT",
    ] {
        env.remove(OsString::from(key).as_os_str());
    }
    env.retain(|key, _| !key.to_string_lossy().starts_with("PASEO_"));
    let xdg = home.join("cortex-xdg");
    env.insert("XDG_CONFIG_HOME".into(), xdg.join("config").into());
    env.insert("XDG_STATE_HOME".into(), xdg.join("state").into());
    if checkout_runtime {
        env.insert("CORTEX_PASEO_SOURCE_ROOT".into(), checkout_root().into());
    }
    env
}

fn installs_git_plugin(words: &[String]) -> bool {
    if words.len() >= 2 && words[0] == "plugin" && words[1] == "update" {
        return true;
    }
    if words.len() < 2 || words[0] != "plugin" || words[1] != "install" {
        return false;
    }
    match words[2..].iter().find(|w| !w.starts_with('-')) {
        Some(source) => {
            let expanded = source
                .strip_prefix("~/")
                .map_or_else(|| PathBuf::from(source), |rest| user_home().join(rest));
            !expanded.is_dir()
        }
        None => false,
    }
}

// ----------------------------------------------------------------- sandbox

fn mountinfo() -> Vec<Vec<String>> {
    let path = std::env::var(MOUNTINFO_OVERRIDE)
        .ok()
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| "/proc/self/mountinfo".into());
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    String::from_utf8_lossy(&bytes)
        .lines()
        .map(|line| line.split(' ').map(String::from).collect::<Vec<_>>())
        .filter(|fields| fields.len() > 5)
        .collect()
}

/// A Cortex sandbox's scratch root mounted over /var/tmp (`/cortex-command-…/var-tmp`), if this process runs in one.
fn cortex_sandbox_var_tmp() -> Option<String> {
    let roots: Vec<String> = mountinfo()
        .into_iter()
        .filter(|f| f[4] == "/var/tmp")
        .map(|f| f[3].replace("\\040", " "))
        .collect();
    let last = roots.last()?;
    let pattern =
        regex::Regex::new(r"/cortex-(?:command|engine)-[^/]+/(?:tmp/)?var-tmp(?://deleted)?$")
            .ok()?;
    pattern.is_match(last).then(|| last.clone())
}

/// A native engine account directory whose effective mount is read-only.
fn read_only_engine_account() -> Option<String> {
    let home = user_home();
    let targets: Vec<String> = ENGINE_ACCOUNT_DIRS
        .iter()
        .map(|n| home.join(n).display().to_string())
        .collect();
    let mut effective: BTreeMap<String, bool> = BTreeMap::new();
    for fields in mountinfo() {
        let point = fields[4].replace("\\040", " ");
        if targets.contains(&point) {
            effective.insert(point, fields[5].split(',').any(|o| o == "ro"));
        }
    }
    targets
        .into_iter()
        .find(|t| effective.get(t).copied().unwrap_or(false))
}

fn refused_inside_cortex_sandbox(label: &str) -> bool {
    if let Some(scratch) = cortex_sandbox_var_tmp() {
        eprintln!(
            "{label}: refusing to start a long-lived Paseo process inside a Cortex sandbox (/var/tmp is its scratch {scratch}, deleted \
             when the command ends, after which every engine jail the daemon launches fails in bwrap); run it through outside_command or a normal terminal"
        );
        return true;
    }
    if let Some(account) = read_only_engine_account() {
        eprintln!(
            "{label}: refusing to start a long-lived Paseo process inside a Cortex sandbox ({account} is mounted read-only, but the \
             daemon's engines use the native accounts there; Codex cannot start with a read-only ~/.codex, so its model discovery fails); \
             run it through outside_command or a normal terminal"
        );
        return true;
    }
    false
}

// ------------------------------------------------------------------ parsing

struct Cli {
    paseo: String,
    home: String,
    check: bool,
    installed_runtime: bool,
    command: Vec<String>,
}

fn parse_cli(argv: &[String]) -> Res<Cli> {
    let mut cli = Cli {
        paseo: DEFAULT_PASEO.into(),
        home: user_home().join(".paseo-dev").display().to_string(),
        check: false,
        installed_runtime: false,
        command: Vec::new(),
    };
    let mut i = 0;
    while i < argv.len() {
        let arg = argv[i].as_str();
        match arg {
            "--" => {
                cli.command = argv[i + 1..].to_vec();
                return Ok(cli);
            }
            "--check" => cli.check = true,
            "--installed-runtime" => cli.installed_runtime = true,
            "--wire-trace" => return Err(
                "--wire-trace was removed with the Python runtime; it has no native replacement"
                    .into(),
            ),
            "--paseo" | "--home" => {
                i += 1;
                let value = argv
                    .get(i)
                    .ok_or_else(|| format!("{arg} needs a value"))?
                    .clone();
                if arg == "--paseo" {
                    cli.paseo = value;
                } else {
                    cli.home = value;
                }
            }
            _ if arg.starts_with("--paseo=") => cli.paseo = arg["--paseo=".len()..].into(),
            _ if arg.starts_with("--home=") => cli.home = arg["--home=".len()..].into(),
            _ if arg.starts_with('-') => return Err(format!("unrecognized option {arg}")),
            _ => {
                cli.command = argv[i..].to_vec();
                return Ok(cli);
            }
        }
        i += 1;
    }
    Ok(cli)
}

fn is_executable_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

// ---------------------------------------------------------------------- cli

fn run_cli(argv: &[String]) -> i32 {
    let label = "cortex_paseo_dev";
    let cli = match parse_cli(argv) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("{label}: {error}\n{USAGE}");
            return 2;
        }
    };
    let home = match resolved_absolute(&cli.home, "--home") {
        Ok(home) => home,
        Err(error) => {
            eprintln!("{label}: {error}");
            return 2;
        }
    };
    let production = resolve(&user_home().join(".paseo"));
    if home == production {
        eprintln!("{label}: refusing production Paseo home");
        return 2;
    }
    let paseo = PathBuf::from(&cli.paseo);
    if !paseo.is_absolute() || !is_executable_file(&paseo) {
        eprintln!("{label}: --paseo must name an executable absolute path");
        return 2;
    }
    let mut command = cli.command.clone();
    if command.first().is_some_and(|c| c == "--") {
        command.remove(0);
    }
    if command.iter().any(|c| {
        c == "--home" || c.starts_with("--home=") || c == "--host" || c.starts_with("--host=")
    }) {
        eprintln!("{label}: command may not override --home or select --host");
        return 2;
    }
    if cli.check {
        if !command.is_empty() {
            eprintln!("{label}: --check accepts no Paseo command");
            return 2;
        }
        let report =
            serde_json::json!({"paseo": paseo, "home": home, "production_home": production});
        println!("{report}");
        return 0;
    }
    if command.is_empty() {
        eprintln!("{label}: provide a Paseo command after --");
        return 2;
    }
    let first = command
        .iter()
        .position(|c| !c.starts_with('-'))
        .unwrap_or(command.len());
    let words = &command[first..];
    if installs_git_plugin(words) && !cli.installed_runtime {
        eprintln!(
            "{label}: a Git-sourced plugin install or update needs --installed-runtime, and the dev daemon must be started with \
             --installed-runtime too; otherwise the daemon launches this checkout instead of the installed runtime"
        );
        return 2;
    }
    if DAEMON_LAUNCHES.iter().any(|launch| {
        words.len() >= launch.len()
            && words[..launch.len()]
                .iter()
                .map(String::as_str)
                .eq(launch.iter().copied())
    }) && refused_inside_cortex_sandbox(label)
    {
        return 2;
    }
    let mut env = isolated_environment(&home, !cli.installed_runtime);
    // A wrapper invoked from an active Paseo agent must not target that agent;
    // the isolated home is explicit in both --home and the environment.
    env.insert("PASEO_HOME".into(), home.clone().into());
    match Command::new(&paseo)
        .arg("--home")
        .arg(&home)
        .args(&command)
        .env_clear()
        .envs(&env)
        .status()
    {
        Ok(status) => status
            .code()
            .unwrap_or(128 + std::os::unix::process::ExitStatusExt::signal(&status).unwrap_or(0)),
        Err(error) => {
            eprintln!("{label}: cannot start {}: {error}", paseo.display());
            1
        }
    }
}

// ---------------------------------------------------------------------- gui

static TERMINATED_BY: AtomicI32 = AtomicI32::new(0);

extern "C" fn on_signal(signal: libc::c_int) {
    TERMINATED_BY.store(signal, Ordering::SeqCst);
}

fn stop_process(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn terminate_group(child: &mut Child, timeout: Duration) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    let group = child.id() as libc::pid_t;
    // SAFETY: signalling a process group this wrapper created with setsid.
    unsafe { libc::killpg(group, libc::SIGTERM) };
    let start = Instant::now();
    while start.elapsed() < timeout {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    // SAFETY: as above.
    unsafe { libc::killpg(group, libc::SIGKILL) };
    let _ = child.wait();
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|d| Path::new(d).join(name))
        .find(|p| is_executable_file(p))
}

fn start_xvfb() -> Res<(Child, String)> {
    let xvfb = which("Xvfb").ok_or(
        "--xvfb requires Xvfb with -displayfd support; install Xvfb or run without --xvfb",
    )?;
    let mut child = Command::new(xvfb)
        .args([
            "-displayfd",
            "1",
            "-screen",
            "0",
            "1440x900x24",
            "-nolisten",
            "tcp",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot start Xvfb: {e}"))?;
    let stdout = child.stdout.take().ok_or("Xvfb has no stdout")?;
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        let _ = sender.send(line);
    });
    let Ok(line) = receiver.recv_timeout(Duration::from_secs(5)) else {
        stop_process(&mut child);
        return Err("Xvfb did not report a private display within 5 seconds".into());
    };
    let display = line.trim().to_string();
    if display.is_empty() || !display.bytes().all(|b| b.is_ascii_digit()) {
        stop_process(&mut child);
        return Err("Xvfb did not report a valid private display".into());
    }
    Ok((child, format!(":{display}")))
}

fn select_debug_port(requested: Option<u16>) -> std::io::Result<u16> {
    let listener = TcpListener::bind(("127.0.0.1", requested.unwrap_or(0)))?;
    Ok(listener.local_addr()?.port())
}

fn hostname() -> String {
    let mut buffer = [0u8; 256];
    // SAFETY: the buffer is valid for its stated length.
    let status = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
    if status != 0 {
        return String::new();
    }
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

/// The live local PID holding Electron's profile lock, if any. Paseo requests the
/// single-instance lock keyed by its userData directory and quits with status 0 when
/// another instance holds it, so a second launch never exposes its own DevTools port.
fn live_singleton_owner(user_data_dir: &Path) -> Option<u32> {
    let target = fs::read_link(user_data_dir.join("SingletonLock")).ok()?;
    let target = target.to_string_lossy().into_owned();
    let (host, pid_text) = target.rsplit_once('-')?;
    if host != hostname() || pid_text.is_empty() || !pid_text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let pid: u32 = pid_text.parse().ok()?;
    // SAFETY: signal 0 only checks existence.
    let status = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if status == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM) {
        Some(pid)
    } else {
        None
    }
}

struct Gui {
    paseo: String,
    home: String,
    user_data_dir: String,
    port: Option<i64>,
    xvfb: bool,
    installed_runtime: bool,
}

fn parse_gui(argv: &[String]) -> Res<Gui> {
    let config = user_home().join(".config");
    let mut gui = Gui {
        paseo: DEFAULT_GUI.into(),
        home: user_home().join(".paseo-dev").display().to_string(),
        user_data_dir: config.join("Paseo-dev").display().to_string(),
        port: None,
        xvfb: false,
        installed_runtime: false,
    };
    let mut it = argv.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--xvfb" => gui.xvfb = true,
            "--installed-runtime" => gui.installed_runtime = true,
            "--paseo" | "--home" | "--user-data-dir" | "--port" => {
                let value = it
                    .next()
                    .ok_or_else(|| format!("{arg} needs a value"))?
                    .clone();
                match arg.as_str() {
                    "--paseo" => gui.paseo = value,
                    "--home" => gui.home = value,
                    "--user-data-dir" => gui.user_data_dir = value,
                    _ => {
                        gui.port = Some(
                            value
                                .parse()
                                .map_err(|_| format!("--port must be an integer (got {value})"))?,
                        )
                    }
                }
            }
            other => {
                return Err(format!(
                    "unrecognized option {other}\n  {INSTALLED_RUNTIME_HELP}"
                ))
            }
        }
    }
    Ok(gui)
}

fn run_gui(argv: &[String]) -> i32 {
    let label = "cortex_paseo_dev gui";
    let gui = match parse_gui(argv) {
        Ok(gui) => gui,
        Err(error) => {
            eprintln!("{label}: {error}");
            return 2;
        }
    };
    let home = match resolved_absolute(&gui.home, "--home") {
        Ok(home) => home,
        Err(error) => {
            eprintln!("{label}: {error}");
            return 2;
        }
    };
    if home == resolve(&user_home().join(".paseo")) {
        eprintln!("{label}: refusing production Paseo home");
        return 2;
    }
    let Ok(user_data_dir) = resolved_absolute(&gui.user_data_dir, "--user-data-dir") else {
        eprintln!("{label}: --user-data-dir must be an absolute path");
        return 2;
    };
    if user_data_dir == resolve(&user_home().join(".config").join("Paseo")) {
        eprintln!("{label}: refusing production Paseo Electron profile");
        return 2;
    }
    let paseo = PathBuf::from(&gui.paseo);
    if !paseo.is_absolute() || !is_executable_file(&paseo) {
        eprintln!(
            "{label}: --paseo must name an executable absolute path (got {})",
            gui.paseo
        );
        return 2;
    }
    let requested = match gui.port {
        None => None,
        Some(p) if (1..=65535).contains(&p) => Some(p as u16),
        Some(p) => {
            eprintln!("{label}: --port must be 1-65535 (got {p})");
            return 2;
        }
    };
    if let Some(owner) = live_singleton_owner(&user_data_dir) {
        eprintln!(
            "{label}: Paseo PID {owner} already holds {}; a second launch would quit at once without DevTools. Reuse or stop that instance first.",
            user_data_dir.display()
        );
        return 3;
    }
    let port = match select_debug_port(requested) {
        Ok(port) => port,
        Err(error) => {
            match requested {
                None => eprintln!("{label}: could not select a free loopback port: {error}"),
                Some(p) => eprintln!("{label}: --port {p} is unavailable on 127.0.0.1: {error}"),
            }
            return 2;
        }
    };
    let mut flags = vec![
        "--remote-debugging-address=127.0.0.1".to_string(),
        format!("--remote-debugging-port={port}"),
    ];
    if gui.xvfb {
        flags.push("--ozone-platform=x11".into());
    }
    if refused_inside_cortex_sandbox(label) {
        return 2;
    }
    let mut env = isolated_environment(&home, !gui.installed_runtime);
    env.insert("PASEO_HOME".into(), home.clone().into());
    env.insert(
        "PASEO_ELECTRON_USER_DATA_DIR".into(),
        user_data_dir.clone().into(),
    );
    env.insert("PASEO_ELECTRON_FLAGS".into(), flags.join(" ").into());
    // ELECTRON_RUN_AS_NODE would interfere with isolation.
    env.remove(OsString::from("ELECTRON_RUN_AS_NODE").as_os_str());
    let mut xvfb = None;
    let mut display = String::new();
    if gui.xvfb {
        match start_xvfb() {
            Ok((child, name)) => {
                env.insert("DISPLAY".into(), name.clone().into());
                display = name;
                xvfb = Some(child);
            }
            Err(error) => {
                eprintln!("{label}: {error}");
                return 2;
            }
        }
    }
    println!("{label}: launching isolated Paseo at {}", home.display());
    println!("  Chrome DevTools Protocol: http://127.0.0.1:{port}");
    if xvfb.is_some() {
        println!("  Private Xvfb display: {display}");
    }
    TERMINATED_BY.store(0, Ordering::SeqCst);
    // SAFETY: the handler only stores into an atomic.
    let (previous_term, previous_int) = unsafe {
        let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        (
            libc::signal(libc::SIGTERM, handler),
            libc::signal(libc::SIGINT, handler),
        )
    };
    let mut command = Command::new(&paseo);
    command.env_clear().envs(&env);
    // SAFETY: setsid in the forked child only; it detaches Electron into its own session and group.
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut code = 1;
    let mut electron = match command.spawn() {
        Ok(child) => Some(child),
        Err(error) => {
            eprintln!("{label}: cannot start {}: {error}", paseo.display());
            None
        }
    };
    if let Some(child) = electron.as_mut() {
        code = loop {
            if let Ok(Some(status)) = child.try_wait() {
                let owner = live_singleton_owner(&user_data_dir);
                if owner.is_some_and(|o| o != child.id()) {
                    eprintln!(
                        "{label}: Paseo exited because PID {} holds {}; no DevTools endpoint is available on port {port}.",
                        owner.unwrap_or(0),
                        user_data_dir.display()
                    );
                    break 3;
                }
                break status.code().unwrap_or(1);
            }
            let signal = TERMINATED_BY.load(Ordering::SeqCst);
            if signal != 0 {
                break 128 + signal;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
    }
    // SAFETY: restoring the handlers saved above.
    unsafe {
        libc::signal(libc::SIGTERM, previous_term);
        libc::signal(libc::SIGINT, previous_int);
    }
    if let Some(child) = electron.as_mut() {
        terminate_group(child, Duration::from_secs(5));
    }
    if let Some(child) = xvfb.as_mut() {
        stop_process(child);
    }
    code
}

pub fn run(argv: &[String]) -> (String, i32) {
    if matches!(
        argv.first().map(String::as_str),
        Some("route-preflight" | "binding")
    ) {
        eprintln!("cortex_paseo_dev: `{}` was removed with the Python runtime it read; it has no native replacement", argv[0]);
        return (String::new(), 2);
    }
    if matches!(argv.first().map(String::as_str), Some("-h" | "--help")) {
        return (format!("{USAGE}\n"), 0);
    }
    let code = if argv.first().map(String::as_str) == Some("gui") {
        run_gui(&argv[1..])
    } else {
        run_cli(argv)
    };
    (String::new(), code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn production_home_is_refused_for_cli_and_gui() {
        let prod = user_home().join(".paseo").display().to_string();
        assert_eq!(run(&args(&["--home", &prod, "--check"])).1, 2);
        // A symlink-free alias resolves to the same production path.
        let alias = format!("{}/x/../.paseo", user_home().display());
        assert_eq!(run(&args(&["--home", &alias, "--check"])).1, 2);
        assert_eq!(run(&args(&["gui", "--home", &prod])).1, 2);
        let profile = user_home().join(".config/Paseo").display().to_string();
        assert_eq!(run(&args(&["gui", "--user-data-dir", &profile])).1, 2);
    }

    #[test]
    fn relative_home_and_endpoint_overrides_are_refused() {
        assert_eq!(run(&args(&["--home", "relative", "--check"])).1, 2);
        let dir = std::env::temp_dir().join(format!("paseo-dev-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        // An existing no-op avoids ETXTBSY when a parallel test forks while
        // this fixture would otherwise have its executable open for writing.
        let paseo_arg = "/bin/true";
        let home_arg = dir.join("home").display().to_string();
        for forbidden in [
            &["--home", "/x"][..],
            &["--home=/x"],
            &["--host", "h"],
            &["--host=h"],
        ] {
            let mut given = vec!["--paseo", paseo_arg, "--home", &home_arg, "--"];
            given.push("status");
            given.extend(forbidden);
            assert_eq!(run(&args(&given)).1, 2, "{forbidden:?}");
        }
        assert_eq!(
            run(&args(&[
                "--paseo", paseo_arg, "--home", &home_arg, "--check", "--", "status"
            ]))
            .1,
            2
        );
        assert_eq!(
            run(&args(&[
                "--paseo", paseo_arg, "--home", &home_arg, "--", "status"
            ]))
            .1,
            0
        );
        assert_eq!(
            run(&args(&[
                "--paseo", paseo_arg, "--home", &home_arg, "--", "plugin", "update"
            ]))
            .1,
            2
        );
        assert_eq!(
            run(&args(&[
                "--paseo",
                paseo_arg,
                "--home",
                &home_arg,
                "--installed-runtime",
                "--",
                "plugin",
                "update"
            ]))
            .1,
            0
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn isolation_drops_inherited_paseo_state_and_roots_xdg_in_the_home() {
        std::env::set_var("PASEO_HOME", "/prod");
        std::env::set_var("PASEO_AGENT_ID", "agent");
        let env = isolated_environment(Path::new("/h"), true);
        assert!(!env
            .keys()
            .any(|k| k.to_string_lossy().starts_with("PASEO_")));
        assert_eq!(
            env[OsString::from("XDG_CONFIG_HOME").as_os_str()],
            OsString::from("/h/cortex-xdg/config")
        );
        assert!(env.contains_key(OsString::from("CORTEX_PASEO_SOURCE_ROOT").as_os_str()));
        assert!(!isolated_environment(Path::new("/h"), false)
            .contains_key(OsString::from("CORTEX_PASEO_SOURCE_ROOT").as_os_str()));
    }

    #[test]
    fn removed_python_modes_are_refused_not_ignored() {
        assert_eq!(run(&args(&["--wire-trace", "--", "start"])).1, 2);
        assert_eq!(run(&args(&["route-preflight"])).1, 2);
        assert_eq!(run(&args(&["binding"])).1, 2);
    }

    #[test]
    fn plugin_installs_are_classified() {
        assert!(installs_git_plugin(&args(&["plugin", "update"])));
        assert!(installs_git_plugin(&args(&[
            "plugin",
            "install",
            "github:x/y"
        ])));
        assert!(!installs_git_plugin(&args(&["plugin", "install", "/"])));
        assert!(!installs_git_plugin(&args(&["plugin", "ls"])));
    }

    #[test]
    fn a_private_xvfb_display_starts_and_stops_when_xvfb_is_installed() {
        if which("Xvfb").is_none() {
            return;
        }
        let (mut child, display) = start_xvfb().unwrap();
        assert!(display.starts_with(':') && display[1..].bytes().all(|b| b.is_ascii_digit()));
        stop_process(&mut child);
        assert!(child.try_wait().unwrap().is_some());
    }

    #[test]
    fn a_free_debug_port_is_selected_and_a_busy_one_is_rejected() {
        let busy = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = busy.local_addr().unwrap().port();
        assert!(select_debug_port(Some(port)).is_err());
        assert!(select_debug_port(None).unwrap() > 0);
    }
}
