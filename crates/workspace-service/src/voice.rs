//! One local microphone lease, independent of projects and model sessions.
use crate::local::failure;
use serde_json::{json, Value};
use std::{collections::VecDeque, io, path::PathBuf, process::Stdio, sync::Arc, time::{Duration, Instant}};
use tokio::{io::{AsyncBufReadExt, AsyncWriteExt, BufReader}, process::Command, sync::{mpsc, Mutex}, task::JoinHandle};
use workspace_protocol::voice::Request;
const LEASE: Duration = Duration::from_secs(10);
struct Live { page: Value, read: Instant }
struct Session {
    id: String, request: String, live: Arc<Mutex<Live>>,
    control: mpsc::Sender<&'static str>, task: JoinHandle<()>,
}
#[derive(Default)]
pub struct Manager { session: Option<Session>, recent: VecDeque<String> }
fn terminal(page: &Value) -> bool { matches!(page["phase"].as_str(), Some("ready" | "error" | "cancelled")) }
fn runtime(config: &crate::Config) -> io::Result<(PathBuf, PathBuf)> {
    let (_, tools) = config.launch.tools().ok_or_else(|| failure("voice_unavailable"))?;
    let root = tools.parent().and_then(|p| p.parent()).and_then(|p| p.parent()).ok_or_else(|| failure("voice_unavailable"))?;
    let installed = root.parent().ok_or_else(|| failure("voice_unavailable"))?;
    let bundle = installed.join("voice/voice.cjs");
    let node = installed.join("engines/node/bin/node");
    if bundle.is_file() && node.is_file() { return Ok((node, bundle)); }
    let helper = root.join("packaging/voice/voice.cjs");
    let node = ["/usr/bin/node", "/usr/local/bin/node"].iter().map(PathBuf::from).find(|p| p.is_file());
    if helper.is_file() && helper.parent().unwrap().join("node_modules/sherpa-onnx-node").is_dir() {
        if let Some(node) = node { return Ok((node, helper)); }
    }
    Err(failure("voice_unavailable"))
}
impl Manager {
    pub async fn call(&mut self, config: &crate::Config, request: Request) -> io::Result<Value> {
        if let Request::Start(p) = request {
            if let Some(s) = &self.session {
                let mut live = s.live.lock().await;
                if s.request == p.request_id { live.read = Instant::now(); return Ok(live.page.clone()); }
                if !terminal(&live.page) { return Err(failure("voice_busy")); }
            }
            if self.recent.contains(&p.request_id) { return Err(failure("voice_request_expired")); }
            self.stop().await;
            let (node, helper) = runtime(config)?;
            let mut command = Command::new(node);
            command.arg(helper).arg(&config.state_dir).current_dir("/").env_clear();
            // Keep the actual desktop audio session, not the dev channel's synthetic runtime directory.
            for key in ["HOME", "LANG", "XDG_RUNTIME_DIR"] {
                if let Some(value) = std::env::var_os(key) { command.env(key, value); }
            }
            command.env("PATH", "/usr/bin:/bin");
            command.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).kill_on_drop(true);
            let parent = unsafe { libc::getpid() };
            unsafe { command.pre_exec(move || {
                if libc::setpgid(0, 0) != 0 || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::getppid() != parent { return Err(failure("voice_parent_lost")); }
                Ok(())
            }); }
            let mut child = command.spawn().map_err(|_| failure("voice_unavailable"))?;
            let pid = child.id().unwrap() as i32;
            let mut input = child.stdin.take().unwrap();
            let output = child.stdout.take().unwrap();
            let id = crate::identity()?;
            let page = json!({"sessionId":id,"phase":"starting","progress":0,"text":"","seconds":0,"error":""});
            let live = Arc::new(Mutex::new(Live { page, read: Instant::now() }));
            let updates = live.clone();
            let (control, mut commands) = mpsc::channel(4);
            let task = tokio::spawn(async move {
                let mut lines = BufReader::new(output).lines();
                let mut timer = tokio::time::interval(Duration::from_millis(500));
                let started = Instant::now();
                let mut stopping: Option<Instant> = None;
                loop {
                    tokio::select! {
                        command = commands.recv() => {
                            match command {
                                Some("stop") => {
                                    if stopping.is_none() {
                                        if input.write_all(b"stop\n").await.is_err() { break; }
                                        stopping = Some(Instant::now());
                                    }
                                }
                                _ => { let mut l = updates.lock().await; l.page["phase"] = json!("cancelled"); l.page["text"] = json!(""); break; }
                            }
                        }
                        line = lines.next_line() => {
                            let Some(line) = line.ok().flatten() else { break; };
                            if line.len() > 65536 { break; }
                            let Ok(value) = serde_json::from_str::<Value>(&line) else { break; };
                            if !valid_update(&value) { break; }
                            let mut l = updates.lock().await;
                            if l.page["phase"] == "cancelled" { break; }
                            for key in ["phase", "progress", "text", "seconds", "error"] { l.page[key] = value[key].clone(); }
                            if terminal(&l.page) { break; }
                            if stopping.is_some() { l.page["phase"] = json!("stopping"); }
                        }
                        _ = timer.tick() => {
                            let l = updates.lock().await;
                            if l.read.elapsed() > LEASE || started.elapsed() > Duration::from_secs(600)
                                || stopping.is_some_and(|s| s.elapsed() > Duration::from_secs(15)) { break; }
                        }
                    }
                }
                // Kill the entire owned group, including the native capture process, and reap the worker.
                unsafe { libc::kill(-pid, libc::SIGKILL); }
                let _ = child.kill().await; let _ = child.wait().await;
                let mut l = updates.lock().await;
                if !terminal(&l.page) {
                    l.page["phase"] = json!("error"); l.page["error"] = json!("voice_session_lost");
                }
            });
            self.recent.push_back(p.request_id.clone());
            if self.recent.len() > 64 { self.recent.pop_front(); }
            self.session = Some(Session { id, request: p.request_id, live, control, task });
        } else {
            let (id, action) = match &request {
                Request::Read(p) => (&p.session_id, None), Request::Stop(p) => (&p.session_id, Some("stop")),
                Request::Cancel(p) => (&p.session_id, Some("cancel")), _ => unreachable!(),
            };
            let s = self.session.as_ref().filter(|s| &s.id == id).ok_or_else(|| failure("voice_session_unknown"))?;
            let mut l = s.live.lock().await; l.read = Instant::now();
            if let Some(action) = action {
                if (!terminal(&l.page) && l.page["phase"] != "stopping") || action == "cancel" {
                    if action == "cancel" { l.page["phase"] = json!("cancelled"); l.page["text"] = json!(""); }
                    else { l.page["phase"] = json!("stopping"); }
                    let _ = s.control.try_send(action);
                }
            }
        }
        Ok(self.session.as_ref().unwrap().live.lock().await.page.clone())
    }
    pub async fn stop(&mut self) {
        if let Some(s) = self.session.take() {
            let _ = s.control.send("cancel").await;
            let _ = s.task.await;
        }
    }
}
fn valid_update(p: &Value) -> bool {
    matches!(p["phase"].as_str(), Some("downloading" | "loading" | "recording" | "ready" | "error"))
        && p["text"].as_str().is_some_and(|s| s.chars().count() <= 16000)
        && p["progress"].as_f64().is_some_and(|n| (0.0..=100.0).contains(&n))
        && p["seconds"].as_f64().is_some_and(|n| (0.0..=100.0).contains(&n))
        && p["error"].as_str().is_some_and(|s| s.len() <= 80)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_updates() {
        assert!(valid_update(&json!({"phase":"recording","text":"привет","progress":100,"seconds":2,"error":""})));
        assert!(!valid_update(&json!({"phase":"recording","text":"x".repeat(16001),"progress":100,"seconds":2,"error":""})));
        assert!(!valid_update(&json!({"phase":"sending","text":"","progress":100,"seconds":2,"error":""})));
    }
}
