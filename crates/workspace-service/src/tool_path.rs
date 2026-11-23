//! The owner's tool search path: the directories a program name is looked up in for
//! sandboxed worker commands, the final verifier, the program-availability probe,
//! uncontained MCP servers and machine diagnostics.
//!
//! The service is started by the session manager, whose `PATH` does not contain what
//! the owner's interactive shell adds (version managers such as nvm, `~/.local/bin`,
//! `~/.cargo/bin` and so on). At start, and when the owner asks, the service runs the
//! owner's own login shell once, interactively (so `.bashrc`/`.zshrc`/`config.fish`
//! run as in a terminal), with no terminal, stdin closed, a minimal environment, cwd
//! = home and a hard timeout that kills the whole process group, and reads only
//! `PATH` between per-run markers. If that fails, the inherited `PATH` plus existing
//! well-known per-user tool directories are used and the reason is recorded.
//!
//! Every entry is validated (absolute, existing directory, not world-writable without
//! the sticky bit, not in Cortex private state), de-duplicated in order and capped.
//! Owner-added extra directories follow the detected ones. A command launched later
//! reads the current value, so a change applies at the next command.
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs,
    io::Read,
    os::unix::{
        ffi::OsStrExt,
        fs::{MetadataExt, OpenOptionsExt},
        process::CommandExt,
    },
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{OnceLock, RwLock},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// Always searched last when the detected path lacks them.
const SYSTEM: [&str; 3] = ["/usr/local/bin", "/usr/bin", "/bin"];
const MAX_ENTRIES: usize = 64;
const MAX_BYTES: usize = 16 * 1024;
pub(crate) const MAX_EXTRA: usize = 16;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(5);
const OUTPUT_WINDOW: usize = 64 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Source {
    /// Not read yet: the service is still reading the shell.
    Pending,
    Shell,
    Fallback,
}
impl Source {
    fn name(self) -> &'static str {
        match self {
            Source::Pending => "pending",
            Source::Shell => "shell",
            Source::Fallback => "fallback",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Rejected {
    pub entry: String,
    pub reason: &'static str,
}

#[derive(Clone, Debug)]
pub(crate) struct Detected {
    pub directories: Vec<PathBuf>,
    pub source: Source,
    pub shell: Option<String>,
    /// Why the fallback was used (`timeout`, `no_marker`, `spawn_failed`, ...).
    pub reason: Option<&'static str>,
    pub read_at: u64,
    pub rejected: Vec<Rejected>,
}

struct State {
    home: PathBuf,
    private: Vec<PathBuf>,
    detected: Detected,
    extra: Vec<PathBuf>,
}
static STATE: OnceLock<RwLock<State>> = OnceLock::new();
/// One shell read at a time; a second request waits for and reuses nothing else.
static READING: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Begin with the inherited path and well-known directories, then read the owner's
/// shell in the background. Never fails and never delays service start.
pub fn start(state_dir: &Path) {
    let home = owner_home();
    let private = private_roots(state_dir, &home);
    let extra = saved_extra(state_dir);
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let mut detected = fallback(&home, &inherited, &private, "shell_read_pending");
    detected.source = Source::Pending;
    let fresh = STATE
        .set(RwLock::new(State {
            home,
            private,
            detected,
            extra: Vec::new(),
        }))
        .is_ok();
    if !fresh {
        return;
    }
    set_extra_paths(&extra);
    let _ = std::thread::Builder::new()
        .name("tool-path-read".into())
        .spawn(|| {
            reread();
        });
}

/// Whether the service started the tool search path (false in the per-command
/// supervisor, which receives the resolved value in its own `PATH`).
pub(crate) fn started() -> bool {
    STATE.get().is_some()
}

/// The resolved search path, or `None` where the service did not start it.
pub(crate) fn current() -> Option<OsString> {
    let state = STATE.get()?.read().ok()?;
    std::env::join_paths(resolved_in(&state).into_iter().map(|(p, _)| p)).ok()
}

/// This process's environment with `PATH` replaced by the command search path
/// ([`for_commands`]).
pub(crate) fn environment() -> BTreeMap<OsString, OsString> {
    let mut environment: BTreeMap<OsString, OsString> = std::env::vars_os().collect();
    if let Some(path) = for_commands() {
        environment.insert("PATH".into(), path);
    }
    if started() {
        environment.insert(COMMAND_EXTRAS.into(), command_extras());
    }
    environment
}

pub(crate) const COMMAND_EXTRAS: &str = "CORTEX_COMMAND_EXTRA_TOOL_DIRS";

/// Only owner-selected directories add general-purpose host executables to the sparse view.
pub(crate) fn command_extras() -> OsString {
    let extra = STATE.get().and_then(|state| state.read().ok()).map(|state| {
        state.extra.iter().filter(|p| valid(p, &state.private).is_ok()).cloned().collect::<Vec<_>>()
    }).unwrap_or_default();
    std::env::join_paths(extra).unwrap_or_default()
}

/// Search-path directories with where each came from: `shell`, `fallback`, `extra`
/// or `system`. Without a started state, the inherited `PATH`.
pub(crate) fn directories() -> Vec<(PathBuf, &'static str)> {
    match STATE.get().and_then(|s| s.read().ok()) {
        Some(state) => resolved_in(&state),
        None => std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .filter(|p| p.is_absolute())
            .map(|p| (p, "inherited"))
            .collect(),
    }
}

fn resolved_in(state: &State) -> Vec<(PathBuf, &'static str)> {
    let detected_origin = match state.detected.source {
        Source::Shell => "shell",
        _ => "fallback",
    };
    let tagged = state
        .detected
        .directories
        .iter()
        .map(|p| (p.clone(), detected_origin))
        .chain(state.extra.iter().map(|p| (p.clone(), "extra")))
        // The usual per-user tool folders the shell path lacks (startup files that set
        // a toolchain up only for some terminals, say) fill gaps after the owner's own.
        .chain(usual(&state.home).into_iter().map(|p| (p, "fallback")))
        .chain(SYSTEM.iter().map(|p| (PathBuf::from(p), "system")));
    let mut seen = BTreeSet::new();
    let mut bytes = 0;
    let mut out = Vec::new();
    for (p, origin) in tagged {
        // Validate again at use: a directory may have changed since it was read.
        let Ok(real) = valid(&p, &state.private) else {
            continue;
        };
        if !seen.insert(real) {
            continue;
        }
        bytes += p.as_os_str().len() + 1;
        if out.len() >= MAX_ENTRIES || bytes > MAX_BYTES {
            break;
        }
        out.push((p, origin));
    }
    out
}

/// The first directory holding an executable `name`, with its origin.
pub(crate) fn locate(name: &str) -> Option<(PathBuf, &'static str)> {
    use std::os::unix::fs::PermissionsExt;
    directories().into_iter().find_map(|(dir, origin)| {
        let candidate = dir.join(name);
        fs::metadata(&candidate)
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            .then_some((candidate, origin))
    })
}

/// Read the owner's shell again now (outside any lock) and keep the result.
pub(crate) fn reread() -> Value {
    let Some(lock) = STATE.get() else {
        return snapshot();
    };
    let _one = READING.lock();
    let (home, private) = match lock.read() {
        Ok(s) => (s.home.clone(), s.private.clone()),
        Err(_) => return snapshot(),
    };
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    let detected = detect(&owner_shell(), &home, &inherited, &private, CAPTURE_TIMEOUT);
    if let Ok(mut s) = lock.write() {
        s.detected = detected;
    }
    snapshot()
}

/// Replace the owner's extra directories (already validated and persisted).
pub(crate) fn set_extra_paths(extra: &[String]) {
    if let Some(lock) = STATE.get() {
        if let Ok(mut s) = lock.write() {
            let private = s.private.clone();
            s.extra = extra
                .iter()
                .take(MAX_EXTRA)
                .map(PathBuf::from)
                .filter(|p| valid(p, &private).is_ok())
                .collect();
        }
    }
}

/// Validate one owner-added directory. The result is the stored form.
pub(crate) fn check_extra(entry: &str, projects: &[PathBuf]) -> Result<String, &'static str> {
    let private = match STATE.get().and_then(|s| s.read().ok()) {
        Some(s) => s.private.clone(),
        None => private_roots(Path::new("/nonexistent"), &owner_home()),
    };
    let path = PathBuf::from(entry);
    let real = valid(&path, &private)?;
    if projects
        .iter()
        .any(|r| real.starts_with(r) || r.canonicalize().is_ok_and(|r| real.starts_with(r)))
    {
        return Err("project_folder");
    }
    if real == Path::new("/") {
        return Err("invalid");
    }
    Ok(match entry.trim_end_matches('/') {
        "" => "/".into(),
        trimmed => trimmed.to_owned(),
    })
}

/// The state shown in settings and diagnostics.
pub(crate) fn snapshot() -> Value {
    let Some(state) = STATE.get().and_then(|s| s.read().ok()) else {
        return json!({"source":"unavailable","detected":[],"extra":[],"resolved":[],"rejected":[]});
    };
    let d = &state.detected;
    json!({
        "source": d.source.name(),
        "shell": d.shell,
        "reason": d.reason,
        "readAt": d.read_at,
        "detected": d.directories,
        "rejected": d.rejected.iter().map(|r| json!({"entry":r.entry,"reason":r.reason})).collect::<Vec<_>>(),
        "extra": state.extra,
        "resolved": resolved_in(&state).into_iter().map(|(p, o)| json!({"directory":p,"origin":o})).collect::<Vec<_>>(),
    })
}

fn owner_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| passwd().map(|(home, _)| home))
        .unwrap_or_else(|| PathBuf::from("/nonexistent"))
}

/// The owner's login shell from the password database, then `SHELL`.
fn owner_shell() -> PathBuf {
    passwd()
        .map(|(_, shell)| shell)
        .filter(|s| s.is_absolute() && s.is_file())
        .or_else(|| {
            std::env::var_os("SHELL")
                .map(PathBuf::from)
                .filter(|s| s.is_absolute() && s.is_file())
        })
        .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}

fn passwd() -> Option<(PathBuf, PathBuf)> {
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut result = std::ptr::null_mut();
    let code = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            entry.as_mut_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut result,
        )
    };
    if code != 0 || result.is_null() {
        return None;
    }
    let entry = unsafe { entry.assume_init() };
    let text = |p: *const libc::c_char| {
        (!p.is_null()).then(|| {
            PathBuf::from(OsStr::from_bytes(
                unsafe { std::ffi::CStr::from_ptr(p) }.to_bytes(),
            ))
        })
    };
    Some((text(entry.pw_dir)?, text(entry.pw_shell)?))
}

/// Cortex's own private state: never a source of programs.
fn private_roots(state_dir: &Path, home: &Path) -> Vec<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let state = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/state"));
    let mut roots = vec![state_dir.to_owned()];
    for base in [config, state] {
        for name in ["cortex", "cortex-workspace", "cortex-workspace-dev"] {
            roots.push(base.join(name));
        }
    }
    roots.retain(|p| p.is_absolute());
    roots
}

/// One entry's validity; the canonical directory on success.
pub(crate) fn valid(entry: &Path, private: &[PathBuf]) -> Result<PathBuf, &'static str> {
    let bytes = entry.as_os_str().as_bytes();
    if bytes.is_empty() || !entry.is_absolute() {
        return Err("relative");
    }
    if bytes.contains(&b':')
        || bytes.contains(&b'\n')
        || bytes.contains(&0)
        || entry
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err("invalid");
    }
    let real = entry.canonicalize().map_err(|_| "missing")?;
    let meta = fs::metadata(&real).map_err(|_| "missing")?;
    if !meta.is_dir() {
        return Err("not_a_directory");
    }
    if meta.mode() & 0o002 != 0 && meta.mode() & 0o1000 == 0 {
        return Err("world_writable");
    }
    let private_component = |p: &Path| {
        p.components().any(|c| {
            let n = c.as_os_str();
            n == ".cortex" || n == ".cortex-dev"
        })
    };
    if private_component(entry)
        || private_component(&real)
        || private.iter().any(|r| {
            entry.starts_with(r)
                || real.starts_with(r)
                || r.canonicalize().is_ok_and(|r| real.starts_with(r))
        })
    {
        return Err("private_state");
    }
    Ok(real)
}

/// Validate, de-duplicate (by real directory, first wins) and cap a list of entries.
pub(crate) fn normalise(
    entries: impl IntoIterator<Item = String>,
    private: &[PathBuf],
) -> (Vec<PathBuf>, Vec<Rejected>) {
    let mut seen = BTreeSet::new();
    let mut kept = Vec::new();
    let mut rejected = Vec::new();
    let mut bytes = 0;
    for entry in entries {
        let path = PathBuf::from(&entry);
        match valid(&path, private) {
            Err(reason) => {
                if rejected.len() < MAX_ENTRIES {
                    rejected.push(Rejected {
                        entry: entry.chars().take(512).collect(),
                        reason,
                    });
                }
            }
            Ok(real) => {
                if !seen.insert(real) {
                    continue;
                }
                bytes += entry.len() + 1;
                if kept.len() >= MAX_ENTRIES || bytes > MAX_BYTES {
                    if rejected.len() < MAX_ENTRIES {
                        rejected.push(Rejected {
                            entry: entry.chars().take(512).collect(),
                            reason: "limit",
                        });
                    }
                    continue;
                }
                kept.push(path);
            }
        }
    }
    (kept, rejected)
}

/// Read the shell; on any failure, the fallback with its reason.
pub(crate) fn detect(
    shell: &Path,
    home: &Path,
    inherited: &OsStr,
    private: &[PathBuf],
    timeout: Duration,
) -> Detected {
    let name = shell
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    match capture(shell, home, inherited, timeout) {
        Ok(path) => {
            let (directories, rejected) = normalise(
                path.split(':').filter(|e| !e.is_empty()).map(str::to_owned),
                private,
            );
            Detected {
                directories,
                source: Source::Shell,
                shell: Some(name),
                reason: None,
                read_at: now_ms(),
                rejected,
            }
        }
        Err(reason) => {
            let mut d = fallback(home, inherited, private, reason);
            d.shell = Some(name);
            d
        }
    }
}

/// Shell flags that run startup files as an interactive login shell does.
fn invocation(shell: &Path, begin: &str, end: &str) -> Result<Vec<String>, &'static str> {
    let name = shell
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // Markers are split across arguments so an echo of the command text never matches.
    let (a, b) = begin.split_at(begin.len() / 2);
    let (c, d) = end.split_at(end.len() / 2);
    match name.as_str() {
        "bash" | "zsh" | "sh" | "dash" | "ksh" | "mksh" => Ok(vec![
            "-l".into(),
            "-i".into(),
            "-c".into(),
            format!("printf '\\n%s%s%s%s%s\\n' '{a}' '{b}' \"$PATH\" '{c}' '{d}'"),
        ]),
        "fish" => Ok(vec![
            "--login".into(),
            "--interactive".into(),
            "--command".into(),
            format!("printf '\\n%s%s%s%s%s\\n' '{a}' '{b}' (string join : $PATH) '{c}' '{d}'"),
        ]),
        _ => Err("unsupported_shell"),
    }
}

/// The last complete marked value in noisy output, if any.
pub(crate) fn parse(output: &[u8], begin: &str, end: &str) -> Option<String> {
    let text = String::from_utf8_lossy(output);
    let start = text.rfind(begin)? + begin.len();
    let length = text[start..].find(end)?;
    let value = &text[start..start + length];
    (!value.contains('\n') && !value.is_empty()).then(|| value.to_owned())
}

fn nonce() -> String {
    let mut bytes = [0u8; 12];
    if fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_err()
    {
        bytes[..8].copy_from_slice(&now_ms().to_le_bytes());
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Run `shell` once and return the `PATH` it reports.
pub(crate) fn capture(
    shell: &Path,
    home: &Path,
    inherited: &OsStr,
    timeout: Duration,
) -> Result<String, &'static str> {
    use std::os::fd::AsRawFd;
    let id = nonce();
    let begin = format!("__CORTEX_TOOL_PATH_BEGIN_{id}__");
    let end = format!("__CORTEX_TOOL_PATH_END_{id}__");
    let args = invocation(shell, &begin, &end)?;
    if !home.is_dir() {
        return Err("home_unavailable");
    }
    let mut command = Command::new(shell);
    command
        .args(&args)
        .env_clear()
        .env("HOME", home)
        .env("SHELL", shell)
        .env("PATH", inherited)
        .env("TERM", "dumb")
        .env(
            "LANG",
            std::env::var_os("LANG").unwrap_or_else(|| "C.UTF-8".into()),
        )
        .current_dir(home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for key in [
        "USER",
        "LOGNAME",
        "XDG_RUNTIME_DIR",
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
    ] {
        if let Some(v) = std::env::var_os(key) {
            command.env(key, v);
        }
    }
    unsafe {
        // A new session: no controlling terminal, and one process group to kill.
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|_| "spawn_failed")?;
    let pid = child.id() as libc::pid_t;
    let mut stdout = child.stdout.take().ok_or("spawn_failed")?;
    let fd = stdout.as_raw_fd();
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let deadline = Instant::now() + timeout;
    let mut output = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut exited = false;
    let mut drain = |output: &mut Vec<u8>| loop {
        match stdout.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                output.extend_from_slice(&buffer[..n]);
                if output.len() > 2 * OUTPUT_WINDOW {
                    output.drain(..output.len() - OUTPUT_WINDOW);
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    };
    while Instant::now() < deadline {
        drain(&mut output);
        // Observe exit without reaping, so the group id cannot be reused before the kill.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let r = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if r == 0 && unsafe { info.si_pid() } == pid {
            exited = true;
            break;
        }
        if r != 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drain(&mut output);
    // Whatever the startup files left running in this session ends with the read.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let _ = child.wait();
    if !exited {
        return Err("timeout");
    }
    parse(&output, &begin, &end).ok_or("no_marker")
}

/// The inherited path plus well-known per-user tool directories that exist.
pub(crate) fn fallback(
    home: &Path,
    inherited: &OsStr,
    private: &[PathBuf],
    reason: &'static str,
) -> Detected {
    let mut entries: Vec<String> = std::env::split_paths(inherited)
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|e| !e.is_empty())
        .collect();
    entries.extend(
        usual(home)
            .into_iter()
            .map(|p| p.to_string_lossy().into_owned()),
    );
    let (directories, rejected) = normalise(entries, private);
    Detected {
        directories,
        source: Source::Fallback,
        shell: None,
        reason: Some(reason),
        read_at: now_ms(),
        rejected,
    }
}

/// The usual per-user tool directories that exist, in search order: `~/.local/bin`,
/// `~/.cargo/bin`, `~/bin`, the active nvm and fnm Node versions, Volta, Bun, Deno,
/// Go, pnpm's home and the pyenv/asdf shims. Not yet validated.
pub(crate) fn usual(home: &Path) -> Vec<PathBuf> {
    let mut known = vec![
        home.join(".local/bin"),
        home.join(".cargo/bin"),
        home.join("bin"),
    ];
    known.extend(nvm_default(home));
    known.extend(fnm_default(home));
    known.extend([
        home.join(".volta/bin"),
        home.join(".bun/bin"),
        home.join(".deno/bin"),
        home.join("go/bin"),
        std::env::var_os("PNPM_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/share/pnpm")),
        home.join(".pyenv/shims"),
        home.join(".asdf/shims"),
    ]);
    known.retain(|p| p.is_dir());
    known
}

/// fnm's default version's `bin` (`aliases/default` in fnm's directory: `FNM_DIR`,
/// else `$XDG_DATA_HOME/fnm`, `~/.local/share/fnm` or the older `~/.fnm`).
pub(crate) fn fnm_default(home: &Path) -> Option<PathBuf> {
    let data = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    std::env::var_os("FNM_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .into_iter()
        .chain([data.join("fnm"), home.join(".fnm")])
        .map(|dir| dir.join("aliases/default/bin"))
        .find(|p| p.is_dir())
}

/// The search path for commands launched from this process. Where the service started
/// the tool search path, that resolved value; elsewhere (an incoming ACP agent, whose
/// `PATH` is its client's) the inherited `PATH`, then the usual per-user tool
/// directories it lacks, then the system folders, validated like every other entry.
pub(crate) fn for_commands() -> Option<OsString> {
    if started() {
        return current();
    }
    let home = owner_home();
    let inherited = std::env::var_os("PATH").unwrap_or_default();
    inherited_with_usual(
        &home,
        &inherited,
        &private_roots(Path::new("/nonexistent"), &home),
    )
}

/// `inherited`, then the usual per-user tool directories and system folders it lacks.
pub(crate) fn inherited_with_usual(
    home: &Path,
    inherited: &OsStr,
    private: &[PathBuf],
) -> Option<OsString> {
    let entries = std::env::split_paths(inherited)
        .chain(usual(home))
        .chain(SYSTEM.iter().map(PathBuf::from))
        .map(|p| p.to_string_lossy().into_owned())
        .filter(|e| !e.is_empty());
    let (directories, _) = normalise(entries, private);
    std::env::join_paths(directories).ok()
}

/// nvm's default version's `bin`, from `~/.nvm/alias/default`. (`NVM_DIR` is set by
/// the shell startup files, so the service never has it.)
pub(crate) fn nvm_default(home: &Path) -> Option<PathBuf> {
    let base = home.join(".nvm");
    let versions = base.join("versions/node");
    let mut alias = fs::read_to_string(base.join("alias/default"))
        .ok()?
        .trim()
        .to_owned();
    let numeric = |s: &str| {
        s.trim_start_matches('v')
            .starts_with(|c: char| c.is_ascii_digit())
    };
    // One level of named alias (`default` -> `lts/iron` -> `22.x`, for example).
    if !numeric(&alias) && !alias.contains("..") && !alias.is_empty() {
        if let Ok(next) = fs::read_to_string(base.join("alias").join(&alias)) {
            alias = next.trim().to_owned();
        }
    }
    if alias.is_empty() {
        return None;
    }
    let wanted = alias.trim_start_matches('v').to_owned();
    // A name nvm resolves itself (`node`, `stable`, `lts/*`): the newest installed.
    let any = !numeric(&alias);
    let mut best: Option<(Vec<u64>, PathBuf)> = None;
    for e in fs::read_dir(&versions).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(number) = name.strip_prefix('v') else {
            continue;
        };
        if !any && number != wanted && !number.starts_with(&format!("{wanted}.")) {
            continue;
        }
        let key: Vec<u64> = number.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        if best.as_ref().is_none_or(|(k, _)| key > *k) {
            best = Some((key, e.path().join("bin")));
        }
    }
    best.map(|(_, p)| p).filter(|p| p.is_dir())
}

/// The saved extra directories from `machine.json`, bounded and without following links.
pub(crate) fn saved_extra(state_dir: &Path) -> Vec<String> {
    let Ok(mut file) = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(state_dir.join("machine.json"))
    else {
        return vec![];
    };
    let mut data = Vec::new();
    if file
        .by_ref()
        .take(4 * 1024 * 1024 + 1)
        .read_to_end(&mut data)
        .is_err()
        || data.len() > 4 * 1024 * 1024
    {
        return vec![];
    }
    serde_json::from_slice::<Value>(&data)
        .ok()
        .and_then(|v| v["toolPathExtra"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(str::to_owned))
        .take(MAX_EXTRA)
        .collect()
}

#[cfg(test)]
#[path = "tool_path_tests.rs"]
pub(crate) mod tests;
