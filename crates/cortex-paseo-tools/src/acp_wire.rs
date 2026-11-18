//! A minimal newline-delimited JSON-RPC peer over a child's stdio, for the
//! no-paid smoke harness and the paid-probe tools. stdout is drained by a
//! reader thread into a channel, so a read never blocks past its deadline.
use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

pub const READ_TIMEOUT: Duration = Duration::from_secs(10);
pub const EXIT_TIMEOUT: Duration = Duration::from_secs(15);

pub struct AgentSession {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Receiver<Option<String>>,
    stderr: Arc<Mutex<Vec<String>>>,
}

impl AgentSession {
    /// Start `argv[0]` with exactly `env` (nothing inherited) in `cwd`.
    pub fn spawn(
        argv: &[String],
        env: &[(String, String)],
        cwd: &Path,
    ) -> Result<AgentSession, String> {
        let mut child = Command::new(&argv[0])
            .args(&argv[1..])
            .env_clear()
            .envs(env.iter().cloned())
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", argv[0]))?;
        let stdout = child.stdout.take().ok_or("no stdout")?;
        let stderr = child.stderr.take().ok_or("no stderr")?;
        let (sender, lines) = channel();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(Some(line)).is_err() {
                    return;
                }
            }
            let _ = sender.send(None);
        });
        let captured = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&captured);
        thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                if let Ok(mut lines) = sink.lock() {
                    lines.push(line);
                }
            }
        });
        let stdin = child.stdin.take();
        Ok(AgentSession {
            child,
            stdin,
            lines,
            stderr: captured,
        })
    }

    pub fn send(&mut self, message: &Value) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("stdin is closed")?;
        writeln!(stdin, "{message}")
            .and_then(|()| stdin.flush())
            .map_err(|e| format!("write to agent failed: {e}"))
    }

    fn drained_stderr(&self) -> String {
        self.stderr.lock().map(|l| l.join("\n")).unwrap_or_default()
    }

    /// The next stdout line and its JSON; any malformed or missing line is an error.
    pub fn recv(&mut self, timeout: Duration) -> Result<Value, String> {
        match self.lines.recv_timeout(timeout) {
            Ok(Some(line)) => serde_json::from_str(&line)
                .map_err(|e| format!("non-JSON stdout line {line:?}: {e}")),
            Ok(None) | Err(RecvTimeoutError::Disconnected) => Err(format!(
                "agent closed stdout unexpectedly (stderr: {:?})",
                self.drained_stderr()
            )),
            Err(RecvTimeoutError::Timeout) => Err(format!(
                "timed out after {}s waiting for a stdout line (stderr so far: {:?})",
                timeout.as_secs(),
                self.drained_stderr()
            )),
        }
    }

    /// Close stdin, wait for exit, kill after `timeout`; the exit code (`None` for a signal).
    pub fn close(&mut self, timeout: Duration) -> Option<i32> {
        self.stdin = None;
        let start = Instant::now();
        loop {
            match self.child.try_wait() {
                Ok(Some(status)) => return status.code(),
                Ok(None) if start.elapsed() < timeout => thread::sleep(Duration::from_millis(20)),
                _ => {
                    let _ = self.child.kill();
                    return self.child.wait().ok().and_then(|s| s.code());
                }
            }
        }
    }
}
