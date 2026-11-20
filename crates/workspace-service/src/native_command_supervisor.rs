//! Private per-command supervisor. Only the inherited pipe can authorize launch;
//! restart observers read its durable status; they never execute the payload.
use crate::native_command_sandbox::{Profile, Sandbox};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead, Read, Write},
    os::unix::{fs::OpenOptionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

pub const MODE: &str = "--native-command-supervisor";
#[derive(Serialize, Deserialize)]
pub(crate) struct Spec {
    pub root: PathBuf,
    pub cwd: PathBuf,
    pub argv: Vec<String>,
    pub profile: String,
    pub writes: Vec<PathBuf>,
    pub protected: Vec<PathBuf>,
    pub output: PathBuf,
    pub status: PathBuf,
    pub stop: PathBuf,
    pub reference: String,
    pub timeout: u64,
}
pub(crate) fn identity(pid: u32) -> Option<Value> {
    let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let fields: Vec<_> = stat.rsplit_once(") ")?.1.split_whitespace().collect();
    if matches!(*fields.first()?, "Z" | "X") {
        return None;
    }
    Some(
        json!({"pid":pid,"boot_id":boot.trim(),"start_ticks":fields.get(19)?.parse::<u64>().ok()?}),
    )
}
fn private(path: &Path) -> io::Result<File> {
    cortex_core::store::locks::open_private(path, false, false)
        .map_err(|_| io::Error::other("command_handle_file_invalid"))
}
pub(crate) fn status(path: &Path) -> io::Result<Value> {
    let mut bytes = Vec::new();
    private(path)?.take(16385).read_to_end(&mut bytes)?;
    if bytes.len() > 16384 {
        return Err(io::Error::other("command_status_oversized"));
    }
    serde_json::from_slice(&bytes).map_err(io::Error::other)
}
fn publish(path: &Path, value: &Value) -> io::Result<()> {
    let temp = path.with_extension("pending");
    let mut f = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&temp)?;
    serde_json::to_writer(&mut f, value)?;
    f.sync_all()?;
    std::fs::rename(temp, path)?;
    File::open(path.parent().unwrap())?.sync_all()
}
/// The fixed reason code of a failed sandbox preparation: the sandbox's own
/// `native_command_sandbox_unavailable: <reason>` code, or the I/O error class.
pub(crate) fn preparation_cause(e: &io::Error) -> String {
    let text = e.to_string();
    match text.strip_prefix("native_command_sandbox_unavailable: ") {
        Some(reason)
            if !reason.is_empty()
                && reason.len() <= 64
                && reason.bytes().all(|b| b.is_ascii_lowercase() || b == b'_') =>
        {
            format!("sandbox_unavailable:{reason}")
        }
        _ if text == "command_launch_not_authorized" => "launch_not_authorized".into(),
        _ => format!("preparation_io_{:?}", e.kind()),
    }
}
fn handshake(input: &mut impl BufRead, message: Value) -> io::Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "{message}")?;
    out.flush()?;
    let mut answer = String::new();
    input.take(128).read_line(&mut answer)?;
    if answer != "go\n" {
        return Err(io::Error::other("command_launch_not_authorized"));
    }
    Ok(())
}
fn drain(
    mut pipe: impl Read + std::os::fd::AsRawFd + Send + 'static,
    file: Arc<Mutex<File>>,
    stop: Arc<AtomicBool>,
) -> std::thread::JoinHandle<io::Result<()>> {
    std::thread::spawn(move || {
        let flags = unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut bytes = [0; 8192];
        loop {
            if stop.load(Ordering::Acquire) {
                return Err(io::Error::other("output_incomplete"));
            }
            match pipe.read(&mut bytes) {
                Ok(0) => return Ok(()),
                Ok(n) => file.lock().unwrap().write_all(&bytes[..n])?,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    if stop.load(Ordering::Acquire) {
                        return Err(io::Error::other("output_incomplete"));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    })
}
/// Trusted binary entry, not an MCP/API operation. Configuration and each
/// preparation mutation require the live inherited authority pipe.
pub fn run() -> io::Result<()> {
    let mut input = io::BufReader::new(io::stdin().lock());
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(1024 * 1024 + 1)
        .read_until(b'\n', &mut bytes)?;
    if bytes.len() > 1024 * 1024 {
        return Err(io::Error::other("command_spec_oversized"));
    }
    let spec: Spec = serde_json::from_slice(&bytes)?;
    let profile = match spec.profile.as_str() {
        "read_only" => Profile::ReadOnly,
        "guarded" => Profile::Guarded,
        "development" => Profile::Development,
        _ => return Err(io::Error::other("command_profile_invalid")),
    };
    let mut timings = std::collections::BTreeMap::new();
    let prepared = Sandbox::prepare_authorized(
        &spec.root,
        profile,
        &spec.writes,
        &spec.protected,
        &mut || handshake(&mut input, json!({"authorize":true})),
        &mut timings,
    );
    let mut sandbox = match prepared {
        Ok(sandbox) => sandbox,
        Err(e) => {
            // Tell the service why nothing will launch: a fixed reason code and the
            // stage timings (numbers), never a path, an argument or host output.
            let mut out = io::stdout().lock();
            let _ = writeln!(
                out,
                "{}",
                json!({"failed":preparation_cause(&e),"timings":timings})
            );
            let _ = out.flush();
            return Err(e);
        }
    };
    let own = identity(std::process::id())
        .ok_or_else(|| io::Error::other("command_identity_unavailable"))?;
    publish(
        &spec.status,
        &json!({"ref":spec.reference,"supervisor":own,"state":"ready"}),
    )?;
    // The service durably binds this exact supervisor before granting launch.
    handshake(&mut input, json!({"ready":own,"timings":timings}))?;
    let launched_at = Instant::now();
    let output = Arc::new(Mutex::new(private(&spec.output)?));
    let mut command: Command = sandbox.command_at(&spec.argv, Some(&spec.cwd))?;
    command
        .current_dir(&spec.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    let mut failure = None;
    let mut code = None;
    let mut signal = None;
    match command.spawn() {
        Err(_) => failure = Some("spawn_failed"),
        Ok(mut child) => {
            let pid = child.id();
            // The payload leads its own process group. Recording it lets a later
            // observer prove the payload gone after this supervisor is lost.
            let _ = publish(
                &spec.status,
                &json!({"ref":spec.reference,"supervisor":own,"state":"ready","group":pid}),
            );
            let stopping = Arc::new(AtomicBool::new(false));
            let out = drain(
                child.stdout.take().unwrap(),
                output.clone(),
                stopping.clone(),
            );
            let err = drain(
                child.stderr.take().unwrap(),
                output.clone(),
                stopping.clone(),
            );
            let deadline = Instant::now() + Duration::from_secs(spec.timeout);
            let mut group_owned = true;
            loop {
                // Keep the exited leader unreaped until group cleanup. Its PID
                // cannot be recycled into an unrelated stop target in this gap.
                let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
                let waited = unsafe {
                    libc::waitid(
                        libc::P_PID,
                        pid,
                        info.as_mut_ptr(),
                        libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
                    )
                };
                if waited != 0 {
                    if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
                        continue;
                    }
                    failure = Some("wait_failed");
                    group_owned = false;
                    break;
                }
                if unsafe { info.assume_init().si_pid() } != 0 {
                    break;
                }
                if spec.stop.exists() {
                    failure = Some("owner_stopped");
                    break;
                }
                if Instant::now() >= deadline {
                    failure = Some("timeout");
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            if group_owned {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            match child.wait() {
                Ok(status) => {
                    use std::os::unix::process::ExitStatusExt;
                    if failure.is_none() {
                        code = status.code();
                    }
                    signal = status.signal();
                }
                Err(_) => failure = Some("wait_failed"),
            }
            let until = Instant::now() + Duration::from_secs(2);
            while !(out.is_finished() && err.is_finished()) && Instant::now() < until {
                std::thread::sleep(Duration::from_millis(10));
            }
            stopping.store(true, Ordering::Release);
            let out = out.join();
            let err = err.join();
            if !matches!(out, Ok(Ok(()))) || !matches!(err, Ok(Ok(()))) {
                failure = Some("output_incomplete");
            }
        }
    }
    if !sandbox.finish().is_empty() {
        output
            .lock()
            .unwrap()
            .write_all(b"\nCortex removed newly planted Git control paths.\n")?;
    }
    drop(sandbox);
    output.lock().unwrap().sync_all()?;
    drop(output);
    let (hash, count) = crate::native_commands::fingerprint(&mut private(&spec.output)?)
        .map_err(|_| io::Error::other("command_output_unavailable"))?;
    publish(
        &spec.status,
        &json!({"ref":spec.reference,"supervisor":own,"state":"finished",
        "exit_code":code,"signal":signal,"failure":failure,"hash":hash,"byte_count":count,
        "process_ms":launched_at.elapsed().as_millis() as u64}),
    )?;
    Ok(())
}
