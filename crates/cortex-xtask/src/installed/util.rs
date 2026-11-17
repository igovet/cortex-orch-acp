//! Shared helpers for the installed-package gate: JSON evidence lines, bounded subprocesses
//! and deadline polling. Everything here runs on the host or inside the disposable container.

use crate::json::{dumps, Format, Res, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const BASE: &str = "ubuntu:26.04@sha256:6a06111c3b0a8600feadb270aec771dc239bf407b25625f93f44ab00e11f4d3c";

pub fn s(x: impl Into<String>) -> Value {
    Value::str(x)
}
pub fn n(x: i64) -> Value {
    Value::int(x)
}
pub fn b(x: bool) -> Value {
    Value::Bool(x)
}
pub fn arr(items: Vec<Value>) -> Value {
    Value::Arr(items)
}
pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut v = Value::obj();
    for (k, x) in pairs {
        v.set(k, x);
    }
    v
}
pub fn opt_s(x: Option<String>) -> Value {
    x.map(Value::Str).unwrap_or(Value::Null)
}

const LINE: Format = Format { sort_keys: false, ensure_ascii: true, indent: None, compact: false };
const CANON: Format = Format { sort_keys: true, ensure_ascii: true, indent: None, compact: true };

/// Python `json.dumps(value)` (`", "` and `": "` separators, ASCII).
pub fn line(value: &Value) -> String {
    dumps(value, &LINE).unwrap_or_else(|e| format!("{{\"step\": \"evidence_error\", \"error\": {:?}}}", e))
}

/// `json.dumps(value, sort_keys=True, separators=(",", ":"))`
pub fn canon(value: &Value) -> String {
    dumps(value, &CANON).unwrap_or_default()
}

pub fn emit(step: &str, pairs: Vec<(&str, Value)>) {
    let mut v = obj(vec![("step", s(step))]);
    for (k, x) in pairs {
        v.set(k, x);
    }
    println!("{}", line(&v));
    use std::io::Write;
    let _ = std::io::stdout().flush();
}

pub fn sha_file(path: &Path) -> Res<String> {
    crate::fsutil::file_digest(path)
}

pub fn sha_bytes(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

pub struct Captured {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Run to completion with a deadline, capturing both streams (drained concurrently).
pub fn capture(command: &mut Command, timeout: Duration) -> Res<Captured> {
    let label = format!("{:?}", command.get_program());
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|e| format!("cannot start {label}: {e}"))?;
    let mut out = child.stdout.take().unwrap();
    let mut err = child.stderr.take().unwrap();
    // A daemonizing grandchild may keep the pipes open after the child exits, so the readers
    // report over channels and are abandoned after a bounded grace period.
    let (tx1, rx1) = std::sync::mpsc::channel();
    let (tx2, rx2) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut out, &mut v);
        let _ = tx1.send(v);
    });
    std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = std::io::Read::read_to_end(&mut err, &mut v);
        let _ = tx2.send(v);
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(st) => break st,
            None if start.elapsed() > timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{label} timed out"));
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    };
    let so = rx1.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    let se = rx2.recv_timeout(Duration::from_secs(5)).unwrap_or_default();
    Ok(Captured {
        code: status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&so).into_owned(),
        stderr: String::from_utf8_lossy(&se).into_owned(),
    })
}

/// Run; a non-zero exit is an error naming the program and exit code only.
pub fn run(argv: &[&str], timeout: Duration) -> Res<Captured> {
    let mut c = Command::new(argv[0]);
    c.args(&argv[1..]);
    let done = capture(&mut c, timeout)?;
    if done.code != 0 {
        return Err(format!(
            "{} failed (exit {}): {}",
            Path::new(argv[0]).file_name().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default(),
            done.code,
            tail(&done.stderr)
        ));
    }
    Ok(done)
}

pub fn run_ok(argv: &[&str]) -> Res<Captured> {
    run(argv, Duration::from_secs(120))
}

pub fn tail(text: &str) -> String {
    let t = text.trim();
    let start = t.len().saturating_sub(400);
    let start = (start..=t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(0);
    t[start..].to_string()
}

pub fn read_to_string(path: &Path) -> Res<String> {
    fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn write(path: &Path, text: &str) -> Res<()> {
    fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn chmod(path: &Path, mode: u32) -> Res<()> {
    crate::proc::chmod(path, mode)
}

pub fn chown_tree(path: &Path, owner: &str) -> Res<()> {
    run_ok(&["chown", "-R", owner, &path.to_string_lossy()]).map(|_| ())
}

/// Poll until `test` yields a value; an `Err` from `test` fails immediately (as an assertion would).
pub fn wait_with<T>(label: &str, seconds: u64, mut test: impl FnMut() -> Res<Option<T>>) -> Res<T> {
    let end = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < end {
        if let Some(value) = test()? {
            if Instant::now() <= end {
                return Ok(value);
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    Err(format!("deadline: {label}"))
}

pub fn wait(label: &str, mut test: impl FnMut() -> Res<bool>) -> Res<()> {
    wait_with(label, 30, || Ok(test()?.then_some(())))
}

pub fn wait_secs(label: &str, seconds: u64, mut test: impl FnMut() -> Res<bool>) -> Res<()> {
    wait_with(label, seconds, || Ok(test()?.then_some(())))
}

pub fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

pub fn mono_ns() -> i64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) };
    t.tv_sec as i64 * 1_000_000_000 + t.tv_nsec as i64
}

pub fn require(cond: bool, message: &str) -> Res<()> {
    if cond {
        Ok(())
    } else {
        Err(message.to_string())
    }
}

/// Run with the parent's stdio inherited (the desktop child it launches keeps logging there).
pub fn run_inherit(argv: &[&str], timeout: Duration) -> Res<()> {
    let mut c = Command::new(argv[0]);
    c.args(&argv[1..]);
    crate::proc::run(&mut c, timeout)
}
