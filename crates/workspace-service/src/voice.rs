//! One local microphone lease, independent of projects and model sessions.
use crate::{local::failure, machine::settings};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs::{self, OpenOptions},
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    process::Command,
    sync::{Mutex, mpsc},
    task::JoinHandle,
};
use workspace_protocol::voice::{Language, Request};
const LEASE: Duration = Duration::from_secs(10);
struct Live {
    page: Value,
    read: Instant,
}
struct Session {
    id: String,
    request: String,
    download_only: bool,
    live: Arc<Mutex<Live>>,
    control: mpsc::Sender<&'static str>,
    task: JoinHandle<()>,
}
#[derive(Default)]
pub struct Manager {
    session: Option<Session>,
    recent: VecDeque<String>,
}
fn terminal(page: &Value) -> bool {
    matches!(
        page["phase"].as_str(),
        Some("ready" | "error" | "cancelled")
    )
}
fn runtime(config: &crate::Config) -> io::Result<(PathBuf, PathBuf)> {
    let root = config
        .launch
        .runtime_root()
        .ok_or_else(|| failure("voice_unavailable"))?;
    let installed = root.parent().ok_or_else(|| failure("voice_unavailable"))?;
    let bundle = installed.join("voice/voice.cjs");
    let node = installed.join("engines/node/bin/node");
    if bundle.is_file() && node.is_file() {
        return Ok((node, bundle));
    }
    let helper = root.join("packaging/voice/voice.cjs");
    let node = ["/usr/bin/node", "/usr/local/bin/node"]
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file());
    if helper.is_file()
        && helper
            .parent()
            .unwrap()
            .join("node_modules/sherpa-onnx-node")
            .is_dir()
    {
        if let Some(node) = node {
            return Ok((node, helper));
        }
    }
    Err(failure("voice_unavailable"))
}
#[derive(Deserialize)]
struct Model {
    name: String,
    bytes: u64,
    files: std::collections::BTreeMap<String, String>,
}

fn catalogue() -> &'static std::collections::BTreeMap<String, Model> {
    static MODELS: std::sync::OnceLock<std::collections::BTreeMap<String, Model>> =
        std::sync::OnceLock::new();
    MODELS.get_or_init(|| {
        serde_json::from_str(include_str!("../../../packaging/voice/model.json"))
            .expect("packaged voice catalogue")
    })
}

fn model_directory(state: &Path, model: &Model) -> PathBuf {
    state.join("voice").join(&model.name)
}

fn model_phase(state: &Path, model: &Model) -> &'static str {
    let directory = model_directory(state, model);
    match fs::symlink_metadata(&directory) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => return "not_downloaded",
        Ok(m) if m.is_dir() => {}
        _ => return "integrity_failed",
    }
    if model.files.iter().all(|(name, sha)| {
        let verified = || -> io::Result<bool> {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(directory.join(name))?;
            let meta = file.metadata()?;
            if !meta.is_file() || meta.len() > model.bytes {
                return Ok(false);
            }
            let mut hash = Sha256::new();
            let size = io::copy(&mut file.take(meta.len() + 1), &mut hash)?;
            Ok(size == meta.len() && format!("{:x}", hash.finalize()) == *sha)
        };
        verified().unwrap_or(false)
    }) {
        "ready"
    } else {
        "integrity_failed"
    }
}

fn preference(state: &Path) -> PathBuf {
    state.join("voice-settings.json")
}

fn language(state: &Path) -> io::Result<Language> {
    let value = settings::read_json(&preference(state), json!(Language::default()))
        .map_err(|_| failure("voice_settings_unavailable"))?;
    serde_json::from_value(value).map_err(|_| failure("voice_settings_unavailable"))
}

impl Manager {
    async fn busy(&self) -> bool {
        match &self.session {
            Some(s) => !terminal(&s.live.lock().await.page),
            None => false,
        }
    }

    async fn set_language(&self, state: &Path, language: Language) -> io::Result<Value> {
        if self.busy().await {
            return Err(failure("voice_busy"));
        }
        settings::write_json(&preference(state), &json!(language))
            .map_err(|_| failure("voice_settings_unavailable"))?;
        Ok(json!({"language":language}))
    }

    pub async fn call(&mut self, config: &crate::Config, request: Request) -> io::Result<Value> {
        match request {
            Request::Settings => {
                let state = config.state_dir.clone();
                let language = language(&state)?;
                let busy = self.busy().await;
                let models = tokio::task::spawn_blocking(move || {
                    Language::ALL.map(|lang| {
                        let model = &catalogue()[lang.as_str()];
                        json!({"language":lang,"name":model.name,"bytes":model.bytes,
                            "phase":model_phase(&state, model)})
                    })
                })
                .await
                .map_err(|_| failure("voice_settings_unavailable"))?;
                return Ok(json!({"language":language,"busy":busy,"models":models}));
            }
            Request::SetLanguage(p) => {
                return self.set_language(&config.state_dir, p.language).await;
            }
            _ => {}
        }
        let download_only = matches!(request, Request::Download(_));
        if let Request::Start(p) | Request::Download(p) = request {
            if let Some(s) = &self.session {
                let mut live = s.live.lock().await;
                if s.request == p.request_id {
                    if s.download_only != download_only {
                        return Err(failure("voice_request_expired"));
                    }
                    live.read = Instant::now();
                    return Ok(live.page.clone());
                }
                if !terminal(&live.page) {
                    return Err(failure("voice_busy"));
                }
            }
            if self.recent.contains(&p.request_id) {
                return Err(failure("voice_request_expired"));
            }
            self.stop().await;
            let language = language(&config.state_dir)?;
            let (node, helper) = runtime(config)?;
            let mut command = Command::new(node);
            command
                .arg(helper)
                .arg(&config.state_dir)
                .arg(language.as_str())
                .arg(if download_only { "download" } else { "record" })
                .current_dir("/")
                .env_clear();
            // Keep the actual desktop audio session, not the dev channel's synthetic runtime directory.
            for key in ["HOME", "LANG", "XDG_RUNTIME_DIR"] {
                if let Some(value) = std::env::var_os(key) {
                    command.env(key, value);
                }
            }
            command.env("PATH", "/usr/bin:/bin");
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .kill_on_drop(true);
            let parent = unsafe { libc::getpid() };
            unsafe {
                command.pre_exec(move || {
                    if libc::setpgid(0, 0) != 0
                        || libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0
                    {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::getppid() != parent {
                        return Err(failure("voice_parent_lost"));
                    }
                    Ok(())
                });
            }
            let mut child = command.spawn().map_err(|_| failure("voice_unavailable"))?;
            let pid = child.id().unwrap() as i32;
            let mut input = child.stdin.take().unwrap();
            let output = child.stdout.take().unwrap();
            let id = crate::identity()?;
            let page = json!({"sessionId":id,"language":language,"phase":"starting","progress":0,"text":"","seconds":0,"error":""});
            let live = Arc::new(Mutex::new(Live {
                page,
                read: Instant::now(),
            }));
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
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
                let _ = child.kill().await;
                let _ = child.wait().await;
                let mut l = updates.lock().await;
                if !terminal(&l.page) {
                    l.page["phase"] = json!("error");
                    l.page["error"] = json!("voice_session_lost");
                }
            });
            self.recent.push_back(p.request_id.clone());
            if self.recent.len() > 64 {
                self.recent.pop_front();
            }
            self.session = Some(Session {
                id,
                request: p.request_id,
                download_only,
                live,
                control,
                task,
            });
        } else {
            let (id, action) = match &request {
                Request::Read(p) => (&p.session_id, None),
                Request::Stop(p) => (&p.session_id, Some("stop")),
                Request::Cancel(p) => (&p.session_id, Some("cancel")),
                _ => unreachable!(),
            };
            let s = self
                .session
                .as_ref()
                .filter(|s| &s.id == id)
                .ok_or_else(|| failure("voice_session_unknown"))?;
            let mut l = s.live.lock().await;
            l.read = Instant::now();
            if let Some(action) = action {
                if (!terminal(&l.page) && l.page["phase"] != "stopping") || action == "cancel" {
                    if action == "cancel" {
                        l.page["phase"] = json!("cancelled");
                        l.page["text"] = json!("");
                    } else {
                        l.page["phase"] = json!("stopping");
                    }
                    let _ = s.control.try_send(action);
                }
            }
        }
        Ok(self
            .session
            .as_ref()
            .unwrap()
            .live
            .lock()
            .await
            .page
            .clone())
    }
    pub async fn stop(&mut self) {
        if let Some(s) = self.session.take() {
            let _ = s.control.send("cancel").await;
            let _ = s.task.await;
        }
    }
}
fn valid_update(p: &Value) -> bool {
    matches!(
        p["phase"].as_str(),
        Some("downloading" | "loading" | "recording" | "ready" | "error")
    ) && p["text"]
        .as_str()
        .is_some_and(|s| s.chars().count() <= 16000)
        && p["progress"]
            .as_f64()
            .is_some_and(|n| (0.0..=100.0).contains(&n))
        && p["seconds"]
            .as_f64()
            .is_some_and(|n| (0.0..=100.0).contains(&n))
        && p["error"].as_str().is_some_and(|s| s.len() <= 80)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_assets_without_a_retained_provider() {
        use std::{fs, os::unix::fs::PermissionsExt};
        let root = std::env::temp_dir().join(format!("cw-native-assets-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        let package = root.join(&workspace_protocol::channel::current().package);
        let assets = package.join("runtime");
        fs::create_dir_all(&assets).unwrap();
        fs::create_dir_all(package.join("engines/node/bin")).unwrap();
        fs::create_dir_all(package.join("voice")).unwrap();
        fs::write(package.join("voice/voice.cjs"), b"fixture").unwrap();
        fs::write(package.join("engines/node/bin/node"), b"fixture").unwrap();
        let launch = crate::LaunchSpec::native(assets.clone()).unwrap();
        assert_eq!(launch.bundled_engines(), Some(package.join("engines")));
        let config = crate::Config {
            runtime_dir: root.join("socket"),
            state_dir: root.join("state"),
            allowed_root: root.clone(),
            launch,
            request_timeout: Duration::from_secs(1),
            shutdown_timeout: Duration::from_secs(1),
            event_capacity: 16,
        };
        assert_eq!(
            runtime(&config).unwrap(),
            (
                package.join("engines/node/bin/node"),
                package.join("voice/voice.cjs")
            )
        );
        assert!(!assets.join("src").exists());
        assert!(!assets.join("crates").exists());
        let other = root.join("unselected-channel/runtime");
        fs::create_dir_all(&other).unwrap();
        assert!(
            crate::LaunchSpec::native(other)
                .unwrap()
                .bundled_engines()
                .is_none()
        );
    }
    #[tokio::test]
    async fn settings_select_separate_models_and_bind_the_active_worker() {
        use std::os::unix::fs::PermissionsExt;
        use workspace_protocol::voice::{Language, parse};

        let root =
            std::env::temp_dir().join(format!("cw-test-voice-{}", crate::identity().unwrap()));
        fs::create_dir(&root).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        let package = root.join(&workspace_protocol::channel::current().package);
        let assets = package.join("runtime");
        let node = package.join("engines/node/bin/node");
        fs::create_dir_all(&assets).unwrap();
        fs::create_dir_all(node.parent().unwrap()).unwrap();
        fs::create_dir_all(package.join("voice")).unwrap();
        fs::write(package.join("voice/voice.cjs"), b"fixture").unwrap();
        // The disposable worker echoes argv, not audio, and waits for cancellation.
        fs::write(
            &node,
            r#"#!/bin/sh
printf '{"phase":"recording","progress":100,"text":"%s/%s","seconds":1,"error":""}\n' "$3" "$4"
read -r ignored
"#,
        )
        .unwrap();
        fs::set_permissions(&node, fs::Permissions::from_mode(0o700)).unwrap();
        let config = crate::Config {
            runtime_dir: root.join("socket"),
            state_dir: root.join("state"),
            allowed_root: root.clone(),
            launch: crate::LaunchSpec::native(assets).unwrap(),
            request_timeout: Duration::from_secs(2),
            shutdown_timeout: Duration::from_secs(1),
            event_capacity: 16,
        };
        let request = |method, value| parse(method, value).unwrap();
        let mut manager = Manager::default();
        let initial = manager.call(&config, Request::Settings).await.unwrap();
        assert_eq!(initial["language"], "ru");
        assert!(!preference(&config.state_dir).exists());
        assert_eq!(initial["models"][0]["phase"], "not_downloaded");
        assert_eq!(initial["models"][1]["phase"], "not_downloaded");
        let ru = model_directory(&config.state_dir, &catalogue()["ru"]);
        let en = model_directory(&config.state_dir, &catalogue()["en"]);
        assert_ne!(en, ru);
        assert!(ru.ends_with("voice/sherpa-onnx-small-zipformer-ru-2024-09-18"));
        fs::create_dir_all(en).unwrap();
        let statuses = manager.call(&config, Request::Settings).await.unwrap();
        assert_eq!(statuses["models"][0]["phase"], "not_downloaded");
        assert_eq!(statuses["models"][1]["phase"], "integrity_failed");

        manager
            .set_language(&config.state_dir, Language::En)
            .await
            .unwrap();
        let mut manager = Manager::default(); // simulate restart before recording
        let active = manager
            .call(
                &config,
                request("voice.start", json!({"requestId":"en-record"})),
            )
            .await
            .unwrap();
        let id = active["sessionId"].as_str().unwrap();
        assert_eq!(active["language"], "en");
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let page = manager
                    .call(&config, request("voice.read", json!({"sessionId":id})))
                    .await
                    .unwrap();
                if page["phase"] == "recording" {
                    assert_eq!(page["text"], "en/record");
                    break;
                }
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
        })
        .await
        .unwrap();
        let error = manager
            .set_language(&config.state_dir, Language::Ru)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "voice_busy");
        let error = manager
            .call(
                &config,
                request("voice.download", json!({"requestId":"other"})),
            )
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "voice_busy");
        manager
            .call(&config, request("voice.cancel", json!({"sessionId":id})))
            .await
            .unwrap();
        manager
            .set_language(&config.state_dir, Language::Ru)
            .await
            .unwrap();
        let next = manager
            .call(
                &config,
                request("voice.download", json!({"requestId":"ru-download"})),
            )
            .await
            .unwrap();
        assert_eq!(next["language"], "ru");
        manager.stop().await;
    }

    #[test]
    fn bounds_updates() {
        assert!(valid_update(
            &json!({"phase":"recording","text":"привет","progress":100,"seconds":2,"error":""})
        ));
        assert!(!valid_update(
            &json!({"phase":"recording","text":"x".repeat(16001),"progress":100,"seconds":2,"error":""})
        ));
        assert!(!valid_update(
            &json!({"phase":"sending","text":"","progress":100,"seconds":2,"error":""})
        ));
    }
}
