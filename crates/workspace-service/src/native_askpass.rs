//! The owner's password for `sudo` in a host command (`SUDO_ASKPASS`).
//!
//! Every host command (an `outside_command`, including a `command_run` the runtime
//! rerouted to the host) gets one private [`Channel`]: a 0700 directory holding a
//! Unix socket, a `sudo` shim placed first on `PATH` (it runs the real `sudo` with
//! `-A`) and an `askpass` script. When `sudo` needs the password it runs the askpass
//! script, which is this binary in [`ASKPASS_MODE`]. The helper connects to the
//! socket; the service checks the caller (this very program, started by a `sudo`
//! process running as root for this user, not traced), opens an owner prompt
//! "Password for sudo" naming the command, and waits. The prompt is listed by
//! `askpass.list` and answered by `askpass.answer` on the desktop or, unless the
//! owner turned it off, on a paired phone. The answer goes to the helper only, which
//! writes it to `sudo` through its private pipe and exits.
//!
//! The password is never stored, logged, put in an event, a receipt or command
//! output, and never shown to a model. It is bound to one prompt, held in memory only
//! while that prompt is answered, and overwritten with zeros once written. A prompt
//! ends when it is answered, cancelled, expires ([`TTL`]), its helper goes away or its
//! command ends; then the helper exits non-zero and `sudo` fails as it would without
//! a password. `sudo -n` never asks. This is a same-user boundary: hostile code
//! already running as the owner is not contained by it (see SECURITY.md).
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::{self, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// This binary as `sudo`'s askpass program.
pub const ASKPASS_MODE: &str = "--sudo-askpass";
/// This binary as the `sudo` shim first on a host command's `PATH`.
pub const SHIM_MODE: &str = "--sudo-shim";
const SOCKET_ENV: &str = "CORTEX_ASKPASS_SOCKET";
const TOKEN_ENV: &str = "CORTEX_ASKPASS_TOKEN";
const SHIM_DIR_ENV: &str = "CORTEX_SUDO_SHIM_DIR";
/// How long one prompt stays open.
pub const TTL: Duration = Duration::from_secs(600);
/// Longest password accepted (sudo reads one line).
pub const PASSWORD_BYTES: usize = 1024;
const PROMPT_CHARS: usize = 200;
const COMMAND_CHARS: usize = 600;

/// The answer to one prompt. Never cloned, printed or serialized.
pub struct Secret(Vec<u8>);
impl Secret {
    /// Takes the bytes of `text` without copying them.
    pub fn new(text: String) -> Self {
        Self(text.into_bytes())
    }
    fn zeroed(len: usize) -> Self {
        Self(vec![0; len])
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}
impl Drop for Secret {
    fn drop(&mut self) {
        for byte in self.0.iter_mut() {
            // SAFETY: a valid, aligned, exclusively borrowed byte.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
        std::sync::atomic::compiler_fence(Ordering::SeqCst);
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

/// How the owner answered one prompt.
pub enum Answer {
    Password(Secret),
    Cancel,
}

struct Open {
    channel: String,
    view: Value,
    expires: u128,
    answer: tokio::sync::oneshot::Sender<Answer>,
}
/// Open prompts of this process: their public view and the way to answer them. Never
/// their answers.
static OPEN: Mutex<BTreeMap<String, Open>> = Mutex::new(BTreeMap::new());
/// Whether a paired phone may send a sudo password or a sign-in token (the owner's
/// setting "Passwords and tokens from phones"; stored as `phoneAnswers`).
static PHONE: AtomicBool = AtomicBool::new(true);
static SETTINGS: Mutex<Option<PathBuf>> = Mutex::new(None);
/// Test fixtures only: accept a helper that is not started by a root `sudo` and
/// skip its stdout check. No production path sets it.
static TRUST_ANY_CALLER: AtomicBool = AtomicBool::new(false);

fn open_prompts() -> std::sync::MutexGuard<'static, BTreeMap<String, Open>> {
    OPEN.lock().unwrap_or_else(|e| e.into_inner())
}
fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
fn random_hex(bytes: usize) -> io::Result<String> {
    let mut buffer = vec![0u8; bytes];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut buffer)?;
    Ok(buffer.iter().map(|b| format!("{b:02x}")).collect())
}
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}
fn printable(text: &str, limit: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(limit)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Test fixtures: trust a helper whatever started it, and let it print to a shared
/// pipe. Process-wide; the production binary never calls it.
#[doc(hidden)]
pub fn trust_any_caller_for_tests() {
    TRUST_ANY_CALLER.store(true, Ordering::SeqCst);
}

/// Reads the owner's phone setting from `<state>/askpass.json` (absent: allowed).
pub(crate) fn configure(state: &Path) {
    let path = state.join("askpass.json");
    let allowed = std::fs::read(&path)
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v["phoneAnswers"].as_bool())
        .unwrap_or(true);
    PHONE.store(allowed, Ordering::SeqCst);
    *SETTINGS.lock().unwrap_or_else(|e| e.into_inner()) = Some(path);
}
/// Whether a paired phone may answer a prompt with a password, or send a sign-in
/// token for an owner action (`native_owner_actions`).
pub fn phone_allowed() -> bool {
    PHONE.load(Ordering::SeqCst)
}
/// The owner changes the phone setting; it is kept in a private file.
pub(crate) fn set_phone_allowed(allowed: bool) -> io::Result<()> {
    let path = SETTINGS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let Some(path) = path {
        let temp = path.with_extension(format!("json.{}", random_hex(6)?));
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&temp)?;
        file.write_all(json!({ "phoneAnswers": allowed }).to_string().as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temp, &path)?;
    }
    PHONE.store(allowed, Ordering::SeqCst);
    Ok(())
}

/// Every open prompt, newest last, without anything secret.
pub fn pending() -> Vec<Value> {
    let now = now_ms();
    open_prompts()
        .values()
        .filter(|o| o.expires > now)
        .map(|o| o.view.clone())
        .collect()
}
/// Answers one open prompt. The prompt closes whatever happens next.
pub fn answer(prompt: &str, answer: Answer) -> Result<(), &'static str> {
    let open = open_prompts().remove(prompt).ok_or("unknown_sudo_prompt")?;
    if open.expires <= now_ms() {
        return Err("unknown_sudo_prompt");
    }
    let password = matches!(answer, Answer::Password(_));
    let (root, task) = (
        open.view["projectRoot"].as_str().map(PathBuf::from),
        open.view["taskId"].as_str().map(str::to_owned),
    );
    open.answer
        .send(answer)
        .map_err(|_| "unknown_sudo_prompt")?;
    // The task's open sudo owner action (if any) is answered by this password.
    if let (true, Some(root), Some(task)) = (password, root, task) {
        crate::native_owner_actions::sudo_answered(&root, &task);
    }
    Ok(())
}
/// `askpass.answer` on the service socket: `{promptId, password}` or
/// `{promptId, cancel: true}`, nothing else. The password string is moved, never
/// copied, into the answer.
pub(crate) fn answer_request(mut params: Value) -> io::Result<Value> {
    let invalid = || crate::local::failure("invalid_params");
    let map = params.as_object_mut().ok_or_else(invalid)?;
    let id = map
        .get("promptId")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(invalid)?
        .to_owned();
    let (len, cancel) = (map.len(), map.get("cancel").cloned());
    let reply = match (len, cancel, map.get_mut("password")) {
        (2, Some(Value::Bool(true)), None) => {
            answer(&id, Answer::Cancel).map_err(crate::local::failure)?;
            json!({"outcome":"cancelled"})
        }
        (2, None, Some(password)) => {
            let Value::String(text) = password.take() else {
                return Err(invalid());
            };
            let secret = Secret::new(text);
            if secret.bytes().is_empty()
                || secret.bytes().len() > PASSWORD_BYTES
                || secret
                    .bytes()
                    .iter()
                    .any(|b| matches!(b, b'\n' | b'\r' | 0))
            {
                return Err(crate::local::failure("invalid_sudo_password"));
            }
            answer(&id, Answer::Password(secret)).map_err(crate::local::failure)?;
            json!({"outcome":"answered"})
        }
        _ => return Err(invalid()),
    };
    Ok(reply)
}

/// What a prompt names: the task, worker and command it belongs to.
#[derive(Clone)]
pub(crate) struct Meta {
    pub task: String,
    pub task_title: Option<String>,
    pub worker: String,
    pub project_root: PathBuf,
    pub argv: Vec<String>,
    pub cwd: PathBuf,
}

struct State {
    id: String,
    token: String,
    exe: PathBuf,
    ttl: Duration,
    trusted: bool,
    meta: Meta,
    outcomes: Mutex<Vec<&'static str>>,
}
impl State {
    fn record(&self, outcome: &'static str) {
        let mut outcomes = self.outcomes.lock().unwrap_or_else(|e| e.into_inner());
        if outcomes.len() < 16 {
            outcomes.push(outcome);
        }
    }
}

/// One host command's private askpass route. Dropping it closes every prompt it
/// opened and removes its directory.
pub(crate) struct Channel {
    dir: PathBuf,
    state: Arc<State>,
    accept: tokio::task::JoinHandle<()>,
}
impl Channel {
    /// Opens the route for one host command. Needs a Tokio runtime.
    pub(crate) fn open(meta: Meta) -> io::Result<Self> {
        Self::open_with(meta, TTL, TRUST_ANY_CALLER.load(Ordering::SeqCst))
    }
    /// `trusted` skips the caller and stdout checks: test fixtures only.
    pub(crate) fn open_with(meta: Meta, ttl: Duration, trusted: bool) -> io::Result<Self> {
        let exe = std::env::current_exe()?;
        let exe_text = exe
            .to_str()
            .filter(|t| !t.contains('\'') && !t.ends_with(" (deleted)"))
            .ok_or_else(|| io::Error::other("askpass_helper_unavailable"))?
            .to_owned();
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| {
                p.is_absolute()
                    && std::fs::metadata(p).is_ok_and(|m| {
                        m.is_dir() && m.uid() == unsafe { libc::geteuid() } && m.mode() & 0o077 == 0
                    })
            })
            .unwrap_or_else(std::env::temp_dir);
        let id = random_hex(16)?;
        let dir = base.join(format!("cw-askpass-{}", &id[..16]));
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let made = (|| -> io::Result<(tokio::net::UnixListener, String)> {
            let write = |name: &str, mode: &str| -> io::Result<()> {
                let mut file = std::fs::OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o700)
                    .custom_flags(libc::O_NOFOLLOW)
                    .open(dir.join(name))?;
                file.write_all(format!("#!/bin/sh\nexec '{exe_text}' {mode} \"$@\"\n").as_bytes())
            };
            write("sudo", SHIM_MODE)?;
            write("askpass", ASKPASS_MODE)?;
            let listener = tokio::net::UnixListener::bind(dir.join("socket"))?;
            Ok((listener, random_hex(32)?))
        })();
        let (listener, token) = match made {
            Ok(made) => made,
            Err(e) => {
                let _ = std::fs::remove_dir_all(&dir);
                return Err(e);
            }
        };
        let state = Arc::new(State {
            id,
            token,
            exe,
            ttl,
            trusted,
            meta,
            outcomes: Mutex::new(Vec::new()),
        });
        let serving = state.clone();
        let accept = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let state = serving.clone();
                tokio::spawn(async move {
                    let outcome = serve(stream, &state).await;
                    state.record(outcome);
                });
            }
        });
        Ok(Self { dir, state, accept })
    }
    /// The environment that routes `sudo` in the command through this channel.
    pub(crate) fn env(&self) -> Vec<(&'static str, OsString)> {
        let mut path = OsString::from(self.dir.as_os_str());
        if let Some(rest) = std::env::var_os("PATH").filter(|p| !p.is_empty()) {
            path.push(":");
            path.push(rest);
        } else {
            path.push(":/usr/local/bin:/usr/bin:/bin");
        }
        vec![
            ("PATH", path),
            ("SUDO_ASKPASS", self.dir.join("askpass").into_os_string()),
            (SOCKET_ENV, self.dir.join("socket").into_os_string()),
            (TOKEN_ENV, OsString::from(&self.state.token)),
            (SHIM_DIR_ENV, self.dir.clone().into_os_string()),
        ]
    }
    /// How each prompt of this command ended, in order (no answers).
    pub(crate) fn outcomes(&self) -> Vec<&'static str> {
        self.state
            .outcomes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }
}
impl Drop for Channel {
    fn drop(&mut self) {
        self.accept.abort();
        open_prompts().retain(|_, o| o.channel != self.state.id);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// `sudo` as the first word of a host command's argv also asks through the channel.
pub(crate) fn with_askpass(argv: &[String]) -> Vec<String> {
    let name = argv.first().map(String::as_str).unwrap_or("");
    if name == "sudo" || name.ends_with("/sudo") {
        let mut out = vec![argv[0].clone()];
        out.extend(sudo_arguments(&argv[1..]));
        out
    } else {
        argv.to_vec()
    }
}

/// `sudo`'s arguments with `-A` first, unless they already choose how (or whether)
/// to read a password, or only ask for help, the version or to forget credentials.
pub fn sudo_arguments<S: AsRef<str> + Clone + From<&'static str>>(args: &[S]) -> Vec<S> {
    let keep = args
        .iter()
        .map(AsRef::as_ref)
        .take_while(|a| a.starts_with('-') && *a != "--")
        .any(|a| {
            matches!(
                a,
                "-n" | "--non-interactive"
                    | "-S"
                    | "--stdin"
                    | "-A"
                    | "--askpass"
                    | "-h"
                    | "--help"
                    | "-V"
                    | "--version"
                    | "-K"
                    | "--remove-timestamp"
            ) || (!a.starts_with("--") && a[1..].chars().any(|c| matches!(c, 'n' | 'S' | 'A')))
        });
    if keep {
        return args.to_vec();
    }
    let mut out = Vec::with_capacity(args.len() + 1);
    out.push(S::from("-A"));
    out.extend(args.iter().cloned());
    out
}

/// The first `sudo` on `path` outside the shim's own directory.
fn real_sudo(path: &OsString, shim: Option<&Path>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(path)
        .filter(|dir| dir.is_absolute() && shim.is_none_or(|s| dir != s))
        .map(|dir| dir.join("sudo"))
        .find(|candidate| {
            std::fs::metadata(candidate)
                .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        })
}

/// The `sudo` shim: runs the real `sudo` with `-A` inserted (see [`sudo_arguments`]).
fn shim(args: Vec<OsString>) -> i32 {
    use std::os::unix::process::CommandExt;
    let shim = std::env::var_os(SHIM_DIR_ENV).map(PathBuf::from);
    let Some(sudo) = real_sudo(
        &std::env::var_os("PATH").unwrap_or_default(),
        shim.as_deref(),
    ) else {
        eprintln!("sudo: command not found");
        return 127;
    };
    let text: Option<Vec<String>> = args.iter().map(|a| a.to_str().map(str::to_owned)).collect();
    let args: Vec<OsString> = match text {
        Some(text) => sudo_arguments(&text)
            .into_iter()
            .map(OsString::from)
            .collect(),
        None => {
            let mut out = vec![OsString::from("-A")];
            out.extend(args);
            out
        }
    };
    let error = std::process::Command::new(sudo).args(args).exec();
    eprintln!("sudo: {}", error.kind());
    126
}

/// Runs a mode of this binary when asked to, and exits. Called first in `main`.
pub fn dispatch() {
    let mut args = std::env::args_os().skip(1);
    let mode = args.next();
    match mode.as_deref().and_then(|m| m.to_str()) {
        Some(ASKPASS_MODE) => std::process::exit(helper(args.collect())),
        Some(SHIM_MODE) => std::process::exit(shim(args.collect())),
        _ => {}
    }
}

/// The askpass helper: asks the service and writes the password to stdout (sudo's
/// private pipe), or exits non-zero.
fn helper(args: Vec<OsString>) -> i32 {
    use std::os::fd::FromRawFd;
    let prompt = args
        .first()
        .map(|a| printable(&a.to_string_lossy(), PROMPT_CHARS))
        .unwrap_or_default();
    let (Some(socket), Ok(token)) = (std::env::var_os(SOCKET_ENV), std::env::var(TOKEN_ENV)) else {
        return 1;
    };
    match ask(Path::new(&socket), &token, &prompt, true) {
        Ok(secret) => {
            // Unbuffered: no copy stays in a stdout buffer.
            let mut out = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(1) });
            let written = out
                .write_all(secret.bytes())
                .and_then(|_| out.write_all(b"\n"));
            drop(secret);
            if written.is_ok() {
                0
            } else {
                1
            }
        }
        Err(e) => {
            eprintln!("sudo password prompt: {e}");
            1
        }
    }
}

/// The answer frame: one byte (`P` password, `C` closed with a reason), a 16-bit
/// big-endian length and that many bytes.
fn read_answer(stream: &mut std::os::unix::net::UnixStream) -> io::Result<Secret> {
    let mut head = [0u8; 3];
    stream.read_exact(&mut head)?;
    let len = u16::from_be_bytes([head[1], head[2]]) as usize;
    if len > PASSWORD_BYTES {
        return Err(io::Error::other("invalid_answer"));
    }
    let mut body = Secret::zeroed(len);
    stream.read_exact(&mut body.0)?;
    match head[0] {
        b'P' if len > 0 => Ok(body),
        b'C' => Err(io::Error::other(printable(
            &String::from_utf8_lossy(body.bytes()),
            64,
        ))),
        _ => Err(io::Error::other("invalid_answer")),
    }
}

/// The helper's side of one prompt. `hardened` is the real helper process: it makes
/// itself undumpable (no same-user tracing or memory reads) and, when the service
/// asks, refuses a stdout pipe that another readable process of this user holds.
pub fn ask(socket: &Path, token: &str, prompt: &str, hardened: bool) -> io::Result<Secret> {
    let mut stream = std::os::unix::net::UnixStream::connect(socket)?;
    stream.set_read_timeout(Some(TTL + Duration::from_secs(60)))?;
    let mut check = [0u8; 1];
    stream.read_exact(&mut check)?;
    let strict = match check[0] {
        b'S' => true,
        b'R' => false,
        _ => return Err(io::Error::other("caller_refused")),
    };
    if hardened {
        if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } != 0 {
            return Err(io::Error::other("helper_hardening_failed"));
        }
        if tracer(std::process::id()) != Some(0) {
            return Err(io::Error::other("helper_traced"));
        }
        if strict {
            private_stdout()?;
        }
    }
    let mut request = json!({ "token": token, "prompt": prompt }).to_string();
    request.push('\n');
    stream.write_all(request.as_bytes())?;
    read_answer(&mut stream)
}

/// stdout is a pipe no readable process of this user (other than this one) holds:
/// only the root `sudo` that started this helper reads it.
fn private_stdout() -> io::Result<()> {
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(1, &mut stat) } != 0 || stat.st_mode & libc::S_IFMT != libc::S_IFIFO {
        return Err(io::Error::other("stdout_not_private_pipe"));
    }
    let target = format!("pipe:[{}]", stat.st_ino);
    let me = std::process::id().to_string();
    for entry in std::fs::read_dir("/proc")?.flatten() {
        let name = entry.file_name();
        let Some(pid) = name
            .to_str()
            .filter(|n| n.bytes().all(|b| b.is_ascii_digit()))
        else {
            continue;
        };
        if pid == me {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(entry.path().join("fd")) else {
            continue;
        };
        for fd in fds.flatten() {
            if std::fs::read_link(fd.path()).is_ok_and(|l| l.as_os_str() == target.as_str()) {
                return Err(io::Error::other("stdout_shared"));
            }
        }
    }
    Ok(())
}

fn status_field(pid: u32, field: &str) -> Option<String> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    text.lines().find_map(|l| {
        l.strip_prefix(field)?
            .strip_prefix(':')
            .map(|v| v.trim().to_owned())
    })
}
fn tracer(pid: u32) -> Option<u32> {
    status_field(pid, "TracerPid")?.parse().ok()
}

/// The caller is this program (same file as the shim names), not traced, started by a
/// `sudo` that runs as root for this user. Answers that `sudo`'s command line.
fn verify_caller(pid: u32, exe: &Path) -> Result<String, &'static str> {
    let caller = std::fs::metadata(format!("/proc/{pid}/exe")).map_err(|_| "caller_unreadable")?;
    let ours = std::fs::metadata(exe).map_err(|_| "helper_missing")?;
    if (caller.dev(), caller.ino()) != (ours.dev(), ours.ino()) {
        return Err("caller_not_helper");
    }
    if tracer(pid) != Some(0) {
        return Err("caller_traced");
    }
    let parent: u32 = status_field(pid, "PPid")
        .and_then(|p| p.parse().ok())
        .ok_or("caller_unreadable")?;
    let uids = status_field(parent, "Uid").ok_or("parent_unreadable")?;
    let uids: Vec<&str> = uids.split_whitespace().collect();
    let me = unsafe { libc::geteuid() }.to_string();
    let comm = std::fs::read_to_string(format!("/proc/{parent}/comm")).unwrap_or_default();
    if uids.len() < 2 || uids[0] != me || uids[1] != "0" || comm.trim() != "sudo" {
        return Err("caller_not_sudo");
    }
    let cmdline = std::fs::read(format!("/proc/{parent}/cmdline")).unwrap_or_default();
    let words: Vec<String> = cmdline
        .split(|b| *b == 0)
        .filter(|w| !w.is_empty())
        .map(|w| String::from_utf8_lossy(w).into_owned())
        .collect();
    Ok(printable(&words.join(" "), COMMAND_CHARS))
}

/// Serves one helper connection; answers how the prompt ended (never the answer).
async fn serve(mut stream: tokio::net::UnixStream, state: &State) -> &'static str {
    let Ok(credentials) = stream.peer_cred() else {
        return "refused:caller_unreadable";
    };
    if credentials.uid() != unsafe { libc::geteuid() } {
        return "refused:caller_other_user";
    }
    let Some(pid) = credentials.pid().and_then(|p| u32::try_from(p).ok()) else {
        return "refused:caller_unreadable";
    };
    let trusted = state.trusted;
    let sudo_command = if trusted {
        None
    } else {
        match verify_caller(pid, &state.exe) {
            Ok(command) => Some(command),
            Err(_) => return "refused:untrusted_caller",
        }
    };
    if stream
        .write_all(if trusted { b"R" } else { b"S" })
        .await
        .is_err()
    {
        return "withdrawn";
    }
    // One bounded request line: the token and sudo's prompt text.
    let mut line = Vec::new();
    let read = tokio::time::timeout(Duration::from_secs(10), async {
        let mut byte = [0u8; 1];
        while line.len() < 4096 {
            if stream.read(&mut byte).await? == 0 {
                break;
            }
            if byte[0] == b'\n' {
                return Ok::<_, io::Error>(true);
            }
            line.push(byte[0]);
        }
        Ok(false)
    })
    .await;
    if !matches!(read, Ok(Ok(true))) {
        return "refused:malformed_request";
    }
    let Ok(request) = serde_json::from_slice::<Value>(&line) else {
        return "refused:malformed_request";
    };
    if !request["token"]
        .as_str()
        .is_some_and(|t| same(t.as_bytes(), state.token.as_bytes()))
    {
        return "refused:token_mismatch";
    }
    if !trusted && tracer(pid) != Some(0) {
        return "refused:caller_traced";
    }
    let prompt = printable(request["prompt"].as_str().unwrap_or(""), PROMPT_CHARS);
    let id = match random_hex(16) {
        Ok(id) => id,
        Err(_) => return "refused:no_identity",
    };
    let opened = now_ms();
    let expires = opened + state.ttl.as_millis();
    let meta = &state.meta;
    let command = sudo_command
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| printable(&meta.argv.join(" "), COMMAND_CHARS));
    let view = json!({
        "promptId": id,
        "title": "Password for sudo",
        "prompt": prompt,
        "command": command,
        "argv": meta.argv.iter().take(32).map(|a| printable(a, 200)).collect::<Vec<_>>(),
        "cwd": meta.cwd,
        "taskId": meta.task,
        "task": meta.task_title.as_deref().map(|t| printable(t.lines().next().unwrap_or(""), 160)),
        "worker": meta.worker,
        "projectRoot": meta.project_root,
        "openedAt": opened as u64,
        "expiresAt": expires as u64,
    });
    let (sender, receiver) = tokio::sync::oneshot::channel();
    open_prompts().insert(
        id.clone(),
        Open {
            channel: state.id.clone(),
            view,
            expires,
            answer: sender,
        },
    );
    struct Close(String);
    impl Drop for Close {
        fn drop(&mut self) {
            open_prompts().remove(&self.0);
        }
    }
    let _close = Close(id);
    let mut probe = [0u8; 1];
    let (frame, outcome) = tokio::select! {
        answer = receiver => match answer {
            Ok(Answer::Password(secret)) => {
                let len = secret.bytes().len() as u16;
                let mut frame = Secret::zeroed(3 + secret.bytes().len());
                frame.0[0] = b'P';
                frame.0[1..3].copy_from_slice(&len.to_be_bytes());
                frame.0[3..].copy_from_slice(secret.bytes());
                (frame, "answered")
            }
            Ok(Answer::Cancel) => (closed("cancelled"), "cancelled"),
            Err(_) => (closed("withdrawn"), "withdrawn"),
        },
        _ = tokio::time::sleep(state.ttl) => (closed("expired"), "expired"),
        _ = stream.read(&mut probe) => return "withdrawn",
    };
    if stream.write_all(frame.bytes()).await.is_err() {
        return "withdrawn";
    }
    let _ = stream.shutdown().await;
    outcome
}
fn closed(reason: &str) -> Secret {
    let mut frame = Secret::zeroed(3 + reason.len());
    frame.0[0] = b'C';
    frame.0[1..3].copy_from_slice(&(reason.len() as u16).to_be_bytes());
    frame.0[3..].copy_from_slice(reason.as_bytes());
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Meta {
        Meta {
            task: "t1".into(),
            task_title: Some("Install the build tools\nmore".into()),
            worker: "worker-backend-dev".into(),
            project_root: PathBuf::from("/nonexistent/project"),
            argv: vec![
                "sudo".into(),
                "apt-get".into(),
                "install".into(),
                "make".into(),
            ],
            cwd: PathBuf::from("/nonexistent/project"),
        }
    }
    fn route(channel: &Channel) -> (PathBuf, String) {
        let env = channel.env();
        let get = |k: &str| env.iter().find(|(n, _)| *n == k).unwrap().1.clone();
        (
            PathBuf::from(get(SOCKET_ENV)),
            get(TOKEN_ENV).into_string().unwrap(),
        )
    }
    async fn wait_for(channel: &Channel) -> Value {
        for _ in 0..200 {
            if let Some(p) = pending().into_iter().find(|p| {
                p["taskId"] == "t1"
                    && open_prompts()
                        .get(p["promptId"].as_str().unwrap())
                        .is_some_and(|o| o.channel == channel.state.id)
            }) {
                return p;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("no prompt opened");
    }

    #[test]
    fn the_shim_inserts_dash_a_unless_sudo_already_chooses() {
        let args = |v: &[&'static str]| v.to_vec();
        assert_eq!(
            sudo_arguments(&args(&["apt", "install"])),
            args(&["-A", "apt", "install"])
        );
        assert_eq!(
            sudo_arguments(&args(&["-u", "root", "id"])),
            args(&["-A", "-u", "root", "id"])
        );
        for kept in [
            &["-n", "true"][..],
            &["--non-interactive", "true"],
            &["-S", "true"],
            &["-nE", "true"],
            &["-A", "true"],
            &["-V"],
            &["-K"],
            &["--help"],
        ] {
            assert_eq!(sudo_arguments(&args(kept)), args(kept), "{kept:?}");
        }
        // Options after the command belong to the command.
        assert_eq!(
            sudo_arguments(&args(&["ls", "-n"])),
            args(&["-A", "ls", "-n"])
        );
        assert_eq!(
            with_askpass(&["/usr/bin/sudo".into(), "true".into()]),
            vec!["/usr/bin/sudo".to_owned(), "-A".into(), "true".into()]
        );
        assert_eq!(with_askpass(&["true".into()]), vec!["true".to_owned()]);
    }

    #[test]
    fn the_real_sudo_is_the_first_outside_the_shim_directory() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("askpass-path-{}", random_hex(6).unwrap()));
        let (shim, fake) = (base.join("shim"), base.join("fake"));
        for dir in [&shim, &fake] {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("sudo"), "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(dir.join("sudo"), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let path = std::env::join_paths([&shim, &fake]).unwrap();
        assert_eq!(real_sudo(&path, Some(&shim)), Some(fake.join("sudo")));
        assert_eq!(real_sudo(&path, None), Some(shim.join("sudo")));
        std::fs::remove_dir_all(&base).unwrap();
    }

    /// A helper whose caller is not a root `sudo` (here: this test process) is refused
    /// before any prompt opens.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_untrusted_caller_opens_no_prompt() {
        let channel = Channel::open_with(meta(), TTL, false).unwrap();
        let (socket, token) = route(&channel);
        let asked = tokio::task::spawn_blocking(move || ask(&socket, &token, "Password:", false))
            .await
            .unwrap();
        assert!(asked.is_err());
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(channel.outcomes(), vec!["refused:untrusted_caller"]);
        assert!(!open_prompts()
            .values()
            .any(|o| o.channel == channel.state.id));
    }

    /// The owner's answer reaches the helper, and only it; the prompt names the
    /// command, task and worker and carries nothing secret; it closes once answered.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_password_answer_reaches_only_the_helper_and_closes_the_prompt() {
        let channel = Channel::open_with(meta(), TTL, true).unwrap();
        let (socket, token) = route(&channel);
        let wrong = token.replace(|c: char| c != '0', "0");
        let (s, w) = (socket.clone(), wrong);
        assert!(tokio::task::spawn_blocking(move || ask(&s, &w, "x", false))
            .await
            .unwrap()
            .is_err());
        let asked = tokio::task::spawn_blocking(move || {
            ask(&socket, &token, "[sudo] password for owner: ", false)
        });
        let prompt = wait_for(&channel).await;
        assert_eq!(prompt["title"], "Password for sudo");
        assert_eq!(prompt["prompt"], "[sudo] password for owner:");
        assert_eq!(prompt["command"], "sudo apt-get install make");
        assert_eq!(prompt["task"], "Install the build tools");
        assert_eq!(prompt["worker"], "worker-backend-dev");
        let id = prompt["promptId"].as_str().unwrap().to_owned();
        answer_request(json!({"promptId":id,"password":"fixture-secret-1"})).unwrap();
        let secret = asked.await.unwrap().unwrap();
        assert_eq!(secret.bytes(), b"fixture-secret-1");
        assert!(format!("{secret:?}").find("fixture").is_none());
        assert!(!pending().iter().any(|p| p["promptId"] == id.as_str()));
        assert!(!pending()
            .iter()
            .any(|p| p.to_string().contains("fixture-secret")));
        // Answering again finds nothing to answer.
        assert_eq!(
            answer_request(json!({"promptId":id,"password":"again"}))
                .unwrap_err()
                .to_string(),
            "unknown_sudo_prompt"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(
            channel.outcomes(),
            vec!["refused:token_mismatch", "answered"]
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn cancel_expiry_and_the_command_ending_fail_the_helper() {
        let channel = Channel::open_with(meta(), TTL, true).unwrap();
        let (socket, token) = route(&channel);
        let (s, t) = (socket.clone(), token.clone());
        let asked = tokio::task::spawn_blocking(move || ask(&s, &t, "Password:", false));
        let id = wait_for(&channel).await["promptId"]
            .as_str()
            .unwrap()
            .to_owned();
        answer_request(json!({"promptId":id,"cancel":true})).unwrap();
        assert_eq!(asked.await.unwrap().unwrap_err().to_string(), "cancelled");
        drop(channel);

        let channel = Channel::open_with(meta(), Duration::from_millis(300), true).unwrap();
        let (socket, token) = route(&channel);
        let asked = tokio::task::spawn_blocking(move || ask(&socket, &token, "Password:", false));
        wait_for(&channel).await;
        assert_eq!(asked.await.unwrap().unwrap_err().to_string(), "expired");
        // The helper hears "expired" before the service drops the prompt from its
        // list; under load that takes a moment (a verification run on 2026-11-21
        // failed on the immediate check). The prompt must close, promptly.
        let mut open = true;
        for _ in 0..200 {
            open = open_prompts()
                .values()
                .any(|o| o.channel == channel.state.id);
            if !open {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(!open, "the expired prompt stayed open");
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(channel.outcomes(), vec!["expired"]);
        drop(channel);

        let channel = Channel::open_with(meta(), TTL, true).unwrap();
        let (socket, token) = route(&channel);
        let dir = channel.dir.clone();
        let asked = tokio::task::spawn_blocking(move || ask(&socket, &token, "Password:", false));
        wait_for(&channel).await;
        drop(channel);
        assert!(asked.await.unwrap().is_err());
        assert!(!dir.exists());
    }

    #[test]
    fn an_answer_request_has_exactly_one_shape() {
        for bad in [
            json!({}),
            json!({"promptId":"ab"}),
            json!({"promptId":"ab","password":"x","cancel":true}),
            json!({"promptId":"ab","cancel":false}),
            json!({"promptId":"ab","password":7}),
            json!({"promptId":"not hex","password":"x"}),
            json!({"promptId":"ab","password":"x","extra":1}),
        ] {
            assert_eq!(
                answer_request(bad.clone()).unwrap_err().to_string(),
                "invalid_params",
                "{bad}"
            );
        }
        for bad in ["", "two\nlines", &"x".repeat(PASSWORD_BYTES + 1)] {
            assert_eq!(
                answer_request(json!({"promptId":"ab","password":bad}))
                    .unwrap_err()
                    .to_string(),
                "invalid_sudo_password"
            );
        }
        assert_eq!(
            answer_request(json!({"promptId":"ab","cancel":true}))
                .unwrap_err()
                .to_string(),
            "unknown_sudo_prompt"
        );
    }
}
