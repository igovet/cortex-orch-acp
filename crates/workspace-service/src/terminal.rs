//! Local owner's interactive terminals. Never used by the model gateway.
use crate::local::{canonical_directory, failure};
use portable_pty::{native_pty_system, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, VecDeque},
    io,
    path::Path,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use workspace_protocol::terminal::{Request, OUTPUT_BYTES, READ_BYTES, SESSION_LIMIT};

#[derive(Default)]
pub struct Manager {
    sessions: BTreeMap<String, Arc<Mutex<Pty>>>,
}
struct Pty {
    id: String,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn portable_pty::Child + Send + Sync>,
    pid: i32,
    output: VecDeque<u8>,
    end: u64,
    exited: bool,
    closed: bool,
}
fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        cols,
        rows,
        pixel_width: 0,
        pixel_height: 0,
    }
}
impl Pty {
    fn spawn(root: &Path, cols: u16, rows: u16) -> io::Result<Self> {
        let pair = native_pty_system()
            .openpty(size(cols, rows))
            .map_err(|_| failure("terminal_unavailable"))?;
        let shell = CommandBuilder::new_default_prog().get_shell();
        if !Path::new(&shell).is_absolute() {
            return Err(failure("terminal_shell_unavailable"));
        }
        let mut command = CommandBuilder::new(&shell);
        command.arg("-i");
        command.cwd(root);
        command.env("TERM", "xterm-256color");
        let fd = pair
            .master
            .as_raw_fd()
            .ok_or_else(|| failure("terminal_unavailable"))?;
        // One nonblocking master is used for input and output. No unbounded writer queue.
        unsafe {
            let flags = libc::fcntl(fd, libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
                return Err(failure("terminal_unavailable"));
            }
        }
        let id = crate::identity()?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|_| failure("terminal_shell_unavailable"))?;
        let pid = child
            .process_id()
            .ok_or_else(|| failure("terminal_unavailable"))? as i32;
        drop(pair.slave);
        Ok(Self {
            id,
            master: pair.master,
            child,
            pid,
            output: VecDeque::with_capacity(OUTPUT_BYTES),
            end: 0,
            exited: false,
            closed: false,
        })
    }
    fn drain(&mut self) {
        let mut bytes = [0u8; 4096];
        // A continuously writing shell cannot starve another request.
        for _ in 0..16 {
            let n = unsafe {
                libc::read(
                    self.master.as_raw_fd().unwrap(),
                    bytes.as_mut_ptr().cast(),
                    bytes.len(),
                )
            };
            if n <= 0 {
                break;
            }
            for byte in &bytes[..n as usize] {
                if self.output.len() == OUTPUT_BYTES {
                    self.output.pop_front();
                }
                self.output.push_back(*byte);
            }
            self.end += n as u64;
        }
        // Observe exit without reaping: retaining the shell PID prevents session-ID
        // reuse before cleanup of foreground and background process groups.
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        if unsafe {
            libc::waitid(
                libc::P_PID,
                self.pid as u32,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        } == 0
            && unsafe { info.si_pid() } != 0
        {
            self.exited = true;
        }
    }
    fn snapshot(&mut self, after: u64) -> io::Result<Value> {
        self.drain();
        if after > self.end {
            return Err(failure("terminal_cursor_ahead"));
        }
        let start = self.end - self.output.len() as u64;
        let at = after.max(start);
        let data: Vec<u8> = self
            .output
            .iter()
            .skip((at - start) as usize)
            .take(READ_BYTES)
            .copied()
            .collect();
        Ok(
            json!({"sessionId":self.id,"data":data,"next":at+data.len() as u64,
            "truncated":after<start,"exited":self.exited}),
        )
    }
    fn write(&mut self, data: &str) -> io::Result<Value> {
        self.drain();
        if self.exited {
            return Err(failure("terminal_exited"));
        }
        let n = unsafe {
            libc::write(
                self.master.as_raw_fd().unwrap(),
                data.as_ptr().cast(),
                data.len(),
            )
        };
        if n < 0 {
            return Err(failure("terminal_input_refused"));
        }
        // A short write is explicit: the caller must never replay already accepted bytes.
        Ok(json!({"written":n}))
    }
    fn signal(&self, signal: i32) {
        // Interactive jobs have their own process groups. Signal every group in our
        // dedicated session, not merely the shell's group. This is cleanup, not containment.
        let mut groups = std::collections::BTreeSet::new();
        groups.insert(self.pid);
        if let Ok(entries) = std::fs::read_dir("/proc") {
            for entry in entries.flatten() {
                if let Ok(stat) = std::fs::read_to_string(entry.path().join("stat")) {
                    if let Some((_, fields)) = stat.rsplit_once(") ") {
                        let fields: Vec<_> = fields.split_whitespace().take(4).collect();
                        if fields.len() == 4 && fields[3].parse::<i32>() == Ok(self.pid) {
                            if let Ok(group) = fields[2].parse::<i32>() {
                                groups.insert(group);
                            }
                        }
                    }
                }
            }
        }
        for group in groups {
            if group > 0 {
                unsafe {
                    libc::kill(-group, signal);
                }
            }
        }
    }
    async fn close(&mut self) {
        self.closed = true;
        self.signal(libc::SIGTERM);
        tokio::time::sleep(Duration::from_millis(150)).await;
        self.signal(libc::SIGKILL);
        // Reap only after the final signal so the session identity cannot be reused.
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                self.exited = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
impl Drop for Pty {
    fn drop(&mut self) {
        if !self.closed {
            self.signal(libc::SIGKILL);
            let _ = self.child.wait();
        }
    }
}
impl Manager {
    pub async fn call(
        &mut self,
        project: &str,
        root: &Path,
        request: Request,
    ) -> io::Result<Value> {
        if let Request::Open(p) = request {
            // Recheck the registered locator; the caller never supplies a cwd or executable.
            crate::project_guard::check(root)?;
            let current = canonical_directory(root)?;
            if current != root {
                return Err(failure("terminal_project_changed"));
            }
            if let Some(pty) = self.sessions.get(project) {
                return pty.lock().await.snapshot(0);
            }
            if self.sessions.len() >= SESSION_LIMIT {
                return Err(failure("terminal_session_limit"));
            }
            let pty = Arc::new(Mutex::new(Pty::spawn(root, p.cols, p.rows)?));
            let weak = Arc::downgrade(&pty);
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    let Some(pty) = weak.upgrade() else { break };
                    let mut pty = pty.lock().await;
                    if pty.closed {
                        break;
                    }
                    pty.drain();
                }
            });
            self.sessions.insert(project.into(), pty.clone());
            return pty.lock().await.snapshot(0);
        }
        let id = match &request {
            Request::Read(p) => &p.session_id,
            Request::Write(p) => &p.session_id,
            Request::Resize(p) => &p.session_id,
            Request::Close(p) => &p.session_id,
            _ => unreachable!(),
        };
        let pty = self
            .sessions
            .get(project)
            .ok_or_else(|| failure("terminal_session_unknown"))?
            .clone();
        let mut pty = pty.lock().await;
        if &pty.id != id {
            return Err(failure("terminal_session_unknown"));
        }
        match request {
            Request::Read(p) => pty.snapshot(p.after),
            Request::Write(p) => pty.write(&p.data),
            Request::Resize(p) => {
                pty.master
                    .resize(size(p.cols, p.rows))
                    .map_err(|_| failure("terminal_resize_failed"))?;
                Ok(json!({"resized":true}))
            }
            Request::Close(_) => {
                pty.close().await;
                self.sessions.remove(project);
                Ok(json!({"closed":true}))
            }
            _ => unreachable!(),
        }
    }
    pub async fn remove(&mut self, project: &str) {
        if let Some(pty) = self.sessions.remove(project) {
            pty.lock().await.close().await;
        }
    }
    pub async fn stop(&mut self) {
        for (_, pty) in std::mem::take(&mut self.sessions) {
            pty.lock().await.close().await;
        }
    }
}
