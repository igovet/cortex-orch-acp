//! Same-user, Unix-only local service. Cortex alone owns its store and public projections.
#[cfg(not(target_os = "linux"))]
compile_error!("workspace-service requires Linux peer credentials and parent-death supervision");
pub mod config_read_dispatch;
pub mod config_read_model;
pub mod config_refresh;
mod core_host;
mod directories;
mod directory_search;
#[cfg(test)]
#[path = "../tests/support/hermetic.rs"]
mod hermetic;
pub mod incoming;
pub mod incoming_history;
pub mod incoming_prompt;
mod launch;
mod local;
mod machine;
pub use machine::{settings_api, update_user_defaults};
/// Trusted same-binary maintenance worker, not a socket or model operation.
#[doc(hidden)]
pub async fn maintenance_worker(state: &Path, id: &str) -> io::Result<()> {
    machine::maintenance::worker(state, id)
        .await
        .map_err(|e| io::Error::other(e.code))
}
mod native_access;
mod native_activity;
mod native_attachments;
mod native_batch;
#[doc(hidden)]
pub mod native_codex_runtime;
mod native_command_handles;
mod native_command_sandbox;
#[doc(hidden)]
pub mod native_command_supervisor;
mod native_commands;
mod native_context_tools;
mod native_controls;
mod native_documents;
mod native_driver;
mod native_effects;
mod native_engine_policy;
mod native_instructions;
pub mod native_mcp;
mod native_owner_controls;
mod native_owner_decisions;
mod native_pipeline;
mod native_project;
mod native_project_ops;
mod native_recovery;
mod native_results;
pub mod native_routes;
pub mod native_runtime;
mod native_sandbox;
pub mod native_sessions;
mod native_skills;
mod native_sources;
mod native_tool_errors;
mod notifier;
mod project_guard;
mod remote;
pub mod remote_policy;
mod session_bus;
pub mod short_text_engine;
mod terminal;
pub mod tool_path;
mod voice;
pub use launch::LaunchSpec;
use local::{canonical_directory, failure, uid};
pub use local::{connect, read_frame, write_frame};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::UnixStream,
    sync::{broadcast, watch, Mutex},
    task::JoinSet,
    time::timeout,
};

#[derive(Clone)]
pub struct Config {
    /// Usually $XDG_RUNTIME_DIR/cortex-workspace; must be private and same-user.
    pub runtime_dir: PathBuf,
    /// Durable private locator registry, separate from every project's .cortex.
    pub state_dir: PathBuf,
    pub allowed_root: PathBuf,
    pub launch: LaunchSpec,
    pub request_timeout: Duration,
    pub shutdown_timeout: Duration,
    pub event_capacity: usize,
}
impl Config {
    pub fn launch_config_root(&self) -> Option<PathBuf> {
        self.launch.dev_config.clone()
    }
    /// Production/native-host defaults; test instances may supply disposable paths.
    /// Missing XDG_RUNTIME_DIR fails closed, never falling back to a public /tmp socket.
    pub fn for_user(allowed_root: PathBuf, mut launch: LaunchSpec) -> io::Result<Self> {
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .ok_or_else(|| failure("runtime_directory_unavailable"))?;
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
            .ok_or_else(|| failure("state_directory_unavailable"))?;
        if workspace_protocol::channel::current().is_dev() {
            let config = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .ok_or_else(|| failure("config_directory_unavailable"))?;
            launch.dev_config = Some(workspace_protocol::channel::current().config_dir(&config));
        }
        Ok(Self {
            runtime_dir: workspace_protocol::channel::current()
                .runtime_dir(&PathBuf::from(runtime)),
            state_dir: workspace_protocol::channel::current().state_dir(&state),
            allowed_root,
            launch,
            request_timeout: Duration::from_secs(30),
            shutdown_timeout: Duration::from_secs(3),
            event_capacity: 128,
        })
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Activation {
    actor_id: String,
    recorded_at: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Project {
    project_id: String,
    canonical_root: PathBuf,
    display_name: String,
    activation: Option<Activation>,
}
/// Why an activated project has no working child, and when the service tries again.
struct Failure {
    reason: String,
    attempts: u32,
    retry_at: tokio::time::Instant,
}
struct State {
    projects: Vec<Project>,
    /// A runtime being started. The registry stays available while it opens.
    activating: BTreeSet<String>,
    failures: BTreeMap<String, Failure>,
    native: BTreeMap<String, Arc<native_runtime::Runtime>>,
}
/// How often the service looks for an activated project with no working child.
const SUPERVISE_EVERY: Duration = Duration::from_secs(1);
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MOST: Duration = Duration::from_secs(60);
/// Concurrent socket connections the service serves at once; later callers wait to be accepted.
pub const CONNECTION_LIMIT: usize = 64;
pub struct Service {
    endpoint: local::Endpoint,
    config: Config,
    state: Mutex<State>,
    native_activation: tokio::sync::RwLock<()>,
    stopping: AtomicBool,
    shutdown: watch::Sender<bool>,
    events: broadcast::Sender<Value>,
    /// Wakes the sprint loop for one project after a change that can move a sprint on.
    sprint_wake: native_driver::sprints::SprintWake,
    helper: machine::Helper,
    searches: directories::Searches,
    core: Arc<core_host::Registry>,
    terminals: Mutex<terminal::Manager>,
    voice: Mutex<voice::Manager>,
    /// The embedded remote-access host, once the real service started it.
    remote: std::sync::OnceLock<remote::Remote>,
    /// Desktop notifications for owner moments, raised by the service itself (see `notifier`).
    notifier: notifier::Notifier,
}
fn identity() -> io::Result<String> {
    let mut bytes = [0; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(format!(
        "wp_{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}
fn actor() -> String {
    format!("uid:{}", uid())
}
fn project_path(root: &Path, path: &Path) -> io::Result<PathBuf> {
    let path = canonical_directory(path).map_err(folder_failure)?;
    if !path.starts_with(root)
        || path
            .components()
            .any(|p| p.as_os_str() == ".cortex" || p.as_os_str() == ".cortex-dev")
    {
        return Err(failure("project_path_refused"));
    }
    Ok(path)
}
/// The filesystem's answer in words the owner can be told. Anything else stays what it was.
fn folder_failure(error: io::Error) -> io::Error {
    match error.kind() {
        io::ErrorKind::NotFound => failure("project_folder_missing"),
        io::ErrorKind::PermissionDenied => failure("project_folder_unreadable"),
        io::ErrorKind::NotADirectory => failure("project_folder_not_directory"),
        _ if error.to_string() == "directory_required" => failure("project_folder_not_directory"),
        _ => error,
    }
}
fn writable(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::CString::new(path.as_os_str().as_bytes())
        .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::W_OK) } == 0)
}
impl Service {
    pub async fn bind(mut config: Config) -> io::Result<Arc<Self>> {
        if config.event_capacity == 0
            || config.event_capacity > 1024
            || config.request_timeout.is_zero()
            || config.shutdown_timeout.is_zero()
        {
            return Err(failure("invalid_limits"));
        }
        let endpoint = local::Endpoint::bind(&config.runtime_dir)?;
        config.state_dir = local::private_tree(&config.state_dir)?;
        config.allowed_root = canonical_directory(&config.allowed_root)?;
        if config
            .allowed_root
            .components()
            .any(|p| p.as_os_str() == ".cortex" || p.as_os_str() == ".cortex-dev")
        {
            return Err(failure("private_root_refused"));
        }
        if config
            .state_dir
            .components()
            .chain(config.runtime_dir.components())
            .any(|p| p.as_os_str() == ".cortex" || p.as_os_str() == ".cortex-dev")
        {
            return Err(failure("private_root_refused"));
        }
        let projects = local::read_registry(&config.state_dir.join("projects.json"))?;
        let mut ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for p in &projects {
            // A project's folder can be away (a disk not mounted yet, a deleted folder). That is
            // that project's state, shown on its page. It never keeps the whole machine down.
            if !ids.insert(p.project_id.clone())
                || !paths.insert(p.canonical_root.clone())
                || p.activation.as_ref().is_some_and(|a| a.actor_id != actor())
            {
                return Err(failure("invalid_registry"));
            }
            workspace_protocol::validate("Identity", &json!(p.project_id))
                .map_err(|_| failure("invalid_registry"))?;
        }
        let (shutdown, _) = watch::channel(false);
        let (events, _) = broadcast::channel(config.event_capacity);
        let service = Arc::new(Self {
            endpoint,
            config,
            state: Mutex::new(State {
                projects,
                activating: BTreeSet::new(),
                failures: BTreeMap::new(),
                native: BTreeMap::new(),
            }),
            native_activation: tokio::sync::RwLock::new(()),
            stopping: AtomicBool::new(false),
            shutdown,
            events,
            sprint_wake: Default::default(),
            helper: machine::Helper::default(),
            searches: directories::Searches::default(),
            core: Arc::new(core_host::Registry::default()),
            terminals: Mutex::new(terminal::Manager::default()),
            voice: Mutex::new(voice::Manager::default()),
            remote: std::sync::OnceLock::new(),
            notifier: Default::default(),
        });
        Ok(service)
    }
    fn accepting(&self) -> io::Result<()> {
        if self.stopping.load(Ordering::SeqCst) {
            Err(failure("service_stopping"))
        } else {
            Ok(())
        }
    }
    pub fn socket_path(&self) -> &Path {
        &self.endpoint.path
    }
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }
    /// Starts the embedded remote-access host (off until the owner enables it; the flag
    /// persists). A host that cannot start leaves the service running and `remote.status`
    /// reporting why. Called once by the real service; bare test services have no host.
    pub async fn start_remote(&self) {
        let relay = workspace_remote::default_relay().map(str::to_owned);
        self.start_remote_with(Vec::new(), relay).await
    }
    /// [`Service::start_remote`] on explicit bind addresses and an explicit default relay
    /// (tests bind loopback only and name no relay, so they never reach the network).
    #[doc(hidden)]
    pub async fn start_remote_with(
        &self,
        bind_addrs: Vec<std::net::SocketAddr>,
        default_relay: Option<String>,
    ) {
        if self.remote.get().is_some() {
            return;
        }
        let started = remote::start(
            &self.config.state_dir,
            &self.endpoint.path,
            bind_addrs,
            default_relay,
        )
        .await;
        if let remote::Remote::Running(host) = &started {
            // An open settings page rereads remote access whenever the host's state moves.
            let mut changes = host.subscribe();
            let events = self.events.clone();
            tokio::spawn(async move {
                while changes.changed().await.is_ok() {
                    let _ = events.send(json!({"type":"remote_state"}));
                }
            });
        }
        let _ = self.remote.set(started);
    }
    pub async fn projects(&self) -> Value {
        json!(self.state.lock().await.projects)
    }
    fn engine_setup(&self) -> Value {
        use std::os::unix::fs::MetadataExt;
        let base = self.config.launch.dev_config.clone().or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        });
        let mut statuses = serde_json::Map::new();
        for engine in ["codex-acp", "claude-agent-acp"] {
            let phase = base
                .as_ref()
                .and_then(|base| {
                    let directory = local::canonical_directory(&base.join("cortex")).ok()?;
                    let path = directory.join(format!("first-run-{engine}.json"));
                    let info = std::fs::symlink_metadata(&path).ok()?;
                    if !info.is_file()
                        || info.uid() != uid()
                        || info.mode() & 0o077 != 0
                        || info.len() > 8192
                    {
                        return None;
                    }
                    let value: Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
                    let phase = value["phase"].as_str()?;
                    match phase {
                        "downloading" | "connecting" | "refreshing_models" | "ready"
                        | "download_failed" | "login_required" | "opted_out" | "catalog_failed"
                        | "setup_failed" | "downloaded" => Some(phase.to_owned()),
                        _ => None,
                    }
                })
                .unwrap_or_else(|| "not_started".into());
            statuses.insert(engine.into(), json!(phase));
        }
        Value::Object(statuses)
    }
    pub async fn status(&self) -> Value {
        json!({"stopping":self.stopping.load(Ordering::SeqCst),"actorId":actor(),
            "channel":workspace_protocol::channel::current().name,"application":workspace_protocol::channel::current().title,
            "engines":self.engine_setup(),"notifications":self.notifier.status(),"transport":"unix","protocol":{"major":1,"minor":0}})
    }
    pub async fn project_status(&self, id: &str) -> io::Result<Value> {
        let state = self.state.lock().await;
        if !state.projects.iter().any(|p| p.project_id == id) {
            return Err(failure("project_not_found"));
        }
        if state.native.contains_key(id) {
            return Ok(
                json!({"ready":true,"state":"ready","recovery":"fresh","executionHost":"rust",
                "capabilities":native_runtime::capabilities()?}),
            );
        }
        let failed = state.failures.get(id).map(|f| f.reason.as_str());
        Ok(if state.activating.contains(id) {
            json!({"ready":false,"state":"starting","recovery":"unknown","reason":failed})
        } else if let Some(reason) = failed {
            json!({"ready":false,"state":"failed","recovery":"unknown","reason":reason})
        } else {
            json!({"ready":false,"state":"inactive","recovery":"unknown","reason":"project_inactive_or_activation_unavailable"})
        })
    }
    fn home_path(&self, path: &Path) -> io::Result<PathBuf> {
        let path = match path.strip_prefix("~") {
            Ok(rest) => std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| failure("absolute_path_required"))?
                .join(rest),
            Err(_) => path.to_path_buf(),
        };
        if !path.is_absolute() {
            return Err(failure("absolute_path_required"));
        }
        Ok(path)
    }
    fn private_path(&self, candidate: &Path) -> bool {
        !candidate.starts_with(&self.config.allowed_root)
            || candidate.starts_with(&self.config.state_dir)
            || candidate.starts_with(self.endpoint.path.parent().unwrap_or(Path::new("/")))
            || candidate
                .components()
                .any(|p| p.as_os_str() == ".cortex" || p.as_os_str() == ".cortex-dev")
    }
    /// The real folder a request names. Links are followed here, once, so everything later
    /// works on one exact root; the private stores and read-only trees stay out of reach.
    fn project_folder(&self, path: &Path, create: bool) -> io::Result<PathBuf> {
        let path = self.home_path(path)?;
        let refused = |candidate: &Path| self.private_path(candidate);
        if create && !path.exists() {
            if path
                .components()
                .any(|p| matches!(p, std::path::Component::ParentDir))
            {
                return Err(failure("invalid_path"));
            }
            let mut existing = path.as_path();
            while !existing.exists() {
                existing = existing
                    .parent()
                    .ok_or_else(|| failure("project_folder_missing"))?;
            }
            if refused(
                &existing
                    .canonicalize()
                    .map_err(folder_failure)?
                    .join(path.strip_prefix(existing).unwrap_or(Path::new(""))),
            ) {
                return Err(failure("project_path_refused"));
            }
            std::fs::create_dir_all(&path).map_err(folder_failure)?;
        }
        let real = path.canonicalize().map_err(folder_failure)?;
        if !real.is_dir() {
            return Err(failure("project_folder_not_directory"));
        }
        if refused(&real) {
            return Err(failure("project_path_refused"));
        }
        if !writable(&real) {
            return Err(failure("project_folder_not_writable"));
        }
        project_path(&self.config.allowed_root, &real)
    }
    /// A new empty folder inside one that exists, refused when the name is already taken. The
    /// parent's links are resolved once, like any other project path.
    fn new_folder(&self, path: &Path) -> io::Result<PathBuf> {
        let path = self.home_path(path)?;
        let (parent, name) = path
            .parent()
            .zip(path.file_name())
            .ok_or_else(|| failure("invalid_path"))?;
        if path
            .components()
            .any(|p| matches!(p, std::path::Component::ParentDir))
        {
            return Err(failure("invalid_path"));
        }
        let parent = parent.canonicalize().map_err(folder_failure)?;
        if !parent.is_dir() {
            return Err(failure("project_folder_not_directory"));
        }
        let folder = parent.join(name);
        if self.private_path(&folder) {
            return Err(failure("project_path_refused"));
        }
        if !writable(&parent) {
            return Err(failure("project_folder_not_writable"));
        }
        std::fs::create_dir(&folder).map_err(|error| match error.kind() {
            io::ErrorKind::AlreadyExists => failure("project_folder_exists"),
            io::ErrorKind::PermissionDenied => failure("project_folder_not_writable"),
            _ => folder_failure(error),
        })?;
        Ok(folder)
    }
    pub async fn register(&self, path: &Path, name: &str) -> io::Result<Value> {
        self.register_folder(path, name, false).await
    }
    pub async fn register_folder(
        &self,
        path: &Path,
        name: &str,
        create: bool,
    ) -> io::Result<Value> {
        self.accepting()?;
        if name.is_empty() || name.chars().count() > 256 {
            return Err(failure("invalid_display_name"));
        }
        self.register_root(self.project_folder(path, create)?, name)
            .await
    }
    pub async fn register_new_folder(&self, path: &Path, name: &str) -> io::Result<Value> {
        self.accepting()?;
        if name.is_empty() || name.chars().count() > 256 {
            return Err(failure("invalid_display_name"));
        }
        let created = self.new_folder(path)?;
        let registered = match self.project_folder(&created, false) {
            Ok(root) => self.register_root(root, name).await,
            Err(error) => Err(error),
        };
        // A folder made for a project that was refused is not left behind.
        if registered.is_err() {
            let _ = std::fs::remove_dir(&created);
        }
        registered
    }
    async fn register_root(&self, canonical_root: PathBuf, name: &str) -> io::Result<Value> {
        project_guard::check(&canonical_root)?;
        let mut state = self.state.lock().await;
        self.accepting()?;
        if let Some(p) = state
            .projects
            .iter()
            .find(|p| p.canonical_root == canonical_root)
        {
            return Ok(json!(p));
        }
        project_guard::reserve(&canonical_root)?;
        let detached = local::read_registry(&self.config.state_dir.join("removed-projects.json"))?;
        let previous = detached.iter().find(|p| p.canonical_root == canonical_root);
        let p = Project {
            project_id: match previous {
                Some(p) => p.project_id.clone(),
                None => identity()?,
            },
            canonical_root,
            display_name: name.into(),
            activation: None,
        };
        let mut updated = state.projects.clone();
        updated.push(p.clone());
        local::save_registry(&self.config.state_dir.join("projects.json"), &updated)?;
        state.projects = updated;
        Ok(json!(p))
    }
    pub async fn rename(&self, id: &str, name: &str) -> io::Result<Value> {
        self.accepting()?;
        if name.trim().is_empty() || name.chars().count() > 256 {
            return Err(failure("invalid_display_name"));
        }
        let mut state = self.state.lock().await;
        let mut updated = state.projects.clone();
        let project = updated
            .iter_mut()
            .find(|p| p.project_id == id)
            .ok_or_else(|| failure("project_not_found"))?;
        project.display_name = name.into();
        let renamed = json!(project);
        local::save_registry(&self.config.state_dir.join("projects.json"), &updated)?;
        state.projects = updated;
        Ok(renamed)
    }
    pub async fn remove(&self, id: &str) -> io::Result<Value> {
        self.accepting()?;
        {
            let mut state = self.state.lock().await;
            if !state.projects.iter().any(|p| p.project_id == id) {
                return Err(failure("project_not_found"));
            }
            let mut detached =
                local::read_registry(&self.config.state_dir.join("removed-projects.json"))?;
            if !detached.iter().any(|p| p.project_id == id) {
                detached.push(
                    state
                        .projects
                        .iter()
                        .find(|p| p.project_id == id)
                        .unwrap()
                        .clone(),
                );
                local::save_registry(
                    &self.config.state_dir.join("removed-projects.json"),
                    &detached,
                )?;
            }
            let updated: Vec<_> = state
                .projects
                .iter()
                .filter(|p| p.project_id != id)
                .cloned()
                .collect();
            local::save_registry(&self.config.state_dir.join("projects.json"), &updated)?;
            state.projects = updated;
            state.failures.remove(id);
        }
        self.terminals.lock().await.remove(id).await;
        Ok(json!({"outcome":"removed"}))
    }
    pub async fn activate(&self, id: &str) -> io::Result<Value> {
        self.activate_inner(id).await
    }
    /// Claims the start of this project's runtime and records the owner's intent. The lock is held
    /// only for this bookkeeping, never across the runtime opening.
    async fn prepare_activation(&self, id: &str) -> io::Result<Result<PathBuf, Value>> {
        let mut state = self.state.lock().await;
        self.accepting()?;
        let index = state
            .projects
            .iter()
            .position(|p| p.project_id == id)
            .ok_or_else(|| failure("project_not_found"))?;
        if !state.activating.insert(id.into()) {
            return Err(failure("project_activation_running"));
        }
        let mut prepare = || {
            let root = project_path(
                &self.config.allowed_root,
                &state.projects[index].canonical_root,
            )?;
            project_guard::check(&root)?;
            if state.native.contains_key(id) {
                return Ok(Err(json!({"projectId":id,"alreadyActive":true})));
            }
            project_guard::activate(&root)?;
            // Persist intent before opening, so an interrupted start is not mistaken for inactivity.
            if state.projects[index].activation.is_none() {
                let mut updated = state.projects.clone();
                updated[index].activation = Some(Activation {
                    actor_id: actor(),
                    recorded_at: SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .map_err(|_| failure("clock_error"))?
                        .as_secs(),
                });
                local::save_registry(&self.config.state_dir.join("projects.json"), &updated)?;
                state.projects = updated;
            }
            Ok(Ok(root))
        };
        let result: io::Result<Result<PathBuf, Value>> = prepare();
        if !matches!(result, Ok(Ok(_))) {
            state.activating.remove(id);
        }
        result
    }
    fn note_failure(state: &mut State, id: &str, reason: String) {
        let attempts = state
            .failures
            .get(id)
            .map_or(0, |f| f.attempts)
            .saturating_add(1);
        let wait = RETRY_FIRST
            .saturating_mul(1u32 << attempts.min(6))
            .min(RETRY_MOST);
        state.failures.insert(
            id.into(),
            Failure {
                reason,
                attempts,
                retry_at: tokio::time::Instant::now() + wait,
            },
        );
    }
    async fn activate_inner(&self, id: &str) -> io::Result<Value> {
        self.accepting()?;
        let root = match self.prepare_activation(id).await {
            Ok(Ok(root)) => root,
            Ok(Err(active)) => return Ok(active),
            Err(error) => {
                if !matches!(
                    error.to_string().as_str(),
                    "project_activation_running" | "project_not_found" | "service_stopping"
                ) {
                    let mut state = self.state.lock().await;
                    Self::note_failure(&mut state, id, error.to_string());
                    drop(state);
                    self.project_changed(id);
                }
                return Err(error);
            }
        };
        self.project_changed(id);
        {
            // Stop must observe every owner opened before its settlement boundary,
            // without holding the machine state mutex over store recovery.
            let _activation = self.native_activation.read().await;
            self.accepting()?;
            let opened = {
                let id = id.to_owned();
                let routes = self
                    .config
                    .launch
                    .native_routes
                    .clone()
                    .unwrap_or_else(|| Arc::new(native_runtime::UnqualifiedRoutes));
                let maintenance_state = self.config.state_dir.clone();
                tokio::task::spawn_blocking(move || {
                    native_runtime::Runtime::open(&root, &id, routes)
                        .map(|r| r.with_maintenance_state(maintenance_state))
                })
                .await
                .map_err(|_| failure("storage_unavailable"))?
                .map_err(|e| failure(e.code()))
            };
            let opened = match opened {
                Ok(runtime) => {
                    let runtime = Arc::new(runtime);
                    match runtime.recover().await {
                        // An answered stop question whose relaunch a restart interrupted
                        // is resumed, never left in RECOVERING. A failure to relaunch
                        // leaves an explicit, stoppable task, not a failed project.
                        Ok(()) => {
                            let _ = runtime.continue_owed_tasks().await;
                            Ok(runtime)
                        }
                        Err(error) => Err(failure(error.code())),
                    }
                }
                Err(error) => Err(error),
            };
            let mut state = self.state.lock().await;
            state.activating.remove(id);
            match opened {
                Ok(runtime) => {
                    state.native.insert(id.into(), runtime);
                    state.failures.remove(id);
                    drop(state);
                    // A project that just opened gets its first sprint pass now, not on some later wake.
                    self.sprint_wake.project(id);
                    self.project_changed(id);
                    return Ok(json!({"projectId":id,"executionHost":"rust","recovery":"fresh"}));
                }
                Err(e) => {
                    Self::note_failure(&mut state, id, e.to_string());
                    drop(state);
                    self.project_changed(id);
                    return Err(e);
                }
            }
        }
    }
    /// A project gained or lost its runtime. Windows reread the machine, which stays connected.
    fn project_changed(&self, id: &str) {
        let _ = self
            .events
            .send(json!({"projectId":id,"type":"project_state"}));
    }
    /// Starts what an earlier activation authorized and the service does not have running: after
    /// a restart or a folder coming back. Never a task, only its runtime.
    async fn recover(self: &Arc<Self>) {
        let now = tokio::time::Instant::now();
        let due: Vec<_> = {
            let state = self.state.lock().await;
            let activated: Vec<_> = state
                .projects
                .iter()
                .filter(|p| p.activation.is_some())
                .map(|p| p.project_id.clone())
                .collect();
            let mut due = vec![];
            for id in activated {
                if state.activating.contains(&id)
                    || state.native.contains_key(&id)
                    || state.failures.get(&id).is_some_and(|f| f.retry_at > now)
                {
                    continue;
                }
                due.push(id);
            }
            due
        };
        for id in due {
            let service = self.clone();
            tokio::spawn(async move {
                let _ = service.activate_inner(&id).await;
            });
        }
    }
    async fn supervise(self: Arc<Self>) {
        let mut shutdown = self.shutdown.subscribe();
        while !*shutdown.borrow_and_update() {
            self.recover().await;
            tokio::select! {
                _ = shutdown.changed() => (),
                _ = tokio::time::sleep(SUPERVISE_EVERY) => (),
            }
        }
    }
    pub async fn workspace(&self, mut request: Value) -> io::Result<Value> {
        self.accepting()?;
        let command = request.get("payload").is_some();
        if command {
            request["actorId"] = json!(actor());
            request["payload"] = self.resolved_attachments(request["payload"].take()).await?;
        }
        workspace_protocol::validate(if command { "Command" } else { "Query" }, &request)
            .map_err(|_| failure("invalid_workspace_request"))?;
        if request["protocol"]["major"] != 1 {
            return Err(failure("version_mismatch"));
        }
        let project = request["projectId"]
            .as_str()
            .ok_or_else(|| failure("invalid_project"))?;
        let native = self.state.lock().await.native.get(project).cloned();
        if let Some(native) = native {
            if !command && request["operation"] == "history.page" {
                // The same core projection `native.query` serves.
                let reply = self.native_query(json!({"protocol":{"major":2,"mapping":2},
                    "requestId":request["requestId"],"projectId":project,"operation":"history.page",
                    "payload":{"target":request["target"],"cursor":request.get("cursor").cloned().unwrap_or(Value::Null),
                        "limit":request.get("limit").cloned().unwrap_or(json!(100))}})).await?;
                return Ok(reply["data"]["value"].clone());
            }
            return native.command(project.to_owned(), request).await;
        }
        Err(self.inactive(project).await)
    }
    /// Why a project has no native runtime: unknown, or registered but not running.
    async fn inactive(&self, project: &str) -> io::Error {
        let state = self.state.lock().await;
        let Some(locator) = state.projects.iter().find(|p| p.project_id == project) else {
            return failure("project_not_found");
        };
        match project_guard::check(&locator.canonical_root) {
            Ok(()) => failure("project_inactive"),
            Err(error) => error,
        }
    }
    pub async fn receive_event(
        &self,
        receiver: &mut broadcast::Receiver<Value>,
    ) -> io::Result<Value> {
        match receiver.recv().await {
            Ok(event) => Ok(event),
            Err(broadcast::error::RecvError::Lagged(_)) => {
                Ok(json!({"type":"resync_required","reason":"subscription_overflow"}))
            }
            Err(_) => Err(failure("subscription_closed")),
        }
    }
    pub async fn cancel_execution(&self, id: &str, execution: &str) -> io::Result<Value> {
        self.accepting()?;
        let native = self.state.lock().await.native.get(id).cloned();
        if let Some(native) = native {
            return native.stop_execution(id.into(), execution.into()).await;
        }
        Err(self.inactive(id).await)
    }
    /// Owner files travel as saved references (`product.attachment`). Core and the engine
    /// prompt get the bodies read here, at this moment; inline bytes are refused here.
    async fn resolved_attachments(&self, mut value: Value) -> io::Result<Value> {
        let state = self.config.state_dir.clone();
        tokio::task::spawn_blocking(move || {
            machine::attachment_files::resolve_all(&state, &mut value)
                .map(|()| value)
                .map_err(|e| failure(e.code))
        })
        .await
        .map_err(|_| failure("machine_tool_failed"))?
    }
    /// A message for the bound session goes down the same prompt path as the owner's first one.
    /// The reply comes when that turn ends, so what is confirmed here is delivery, not completion.
    pub async fn prompt_execution(
        &self,
        id: &str,
        execution: &str,
        text: &str,
        request_id: Option<&str>,
        attachments: Value,
    ) -> io::Result<Value> {
        self.accepting()?;
        let native = self.state.lock().await.native.get(id).cloned();
        if let Some(native) = native {
            // Files are checked and decoded once here and travel as a trusted capture; the
            // request itself never carries file bytes.
            let capture = match attachments.as_array() {
                Some(items) if items.is_empty() => None,
                _ => Some(
                    cortex_core::input::capture_from_wire(
                        &self
                            .resolved_attachments(json!({"attachments":attachments}))
                            .await?["attachments"],
                    )
                    .map_err(|e| failure(e.code()))?,
                ),
            };
            return native.control_input(id.into(), json!({"projectId":id,"executionId":execution,"action":"prompt","text":text,"attachments":[],"requestId":match request_id {Some(key)=>key.to_owned(),None=>format!("owner-{}",identity()?)}}), capture).await;
        }
        Err(self.inactive(id).await)
    }
    pub async fn control_execution(&self, params: Value) -> io::Result<Value> {
        self.accepting()?;
        let project = params["projectId"]
            .as_str()
            .ok_or_else(|| failure("invalid_project"))?;
        params["executionId"]
            .as_str()
            .ok_or_else(|| failure("invalid_execution"))?;
        let native = self.state.lock().await.native.get(project).cloned();
        if let Some(native) = native {
            return native.control(project.to_owned(), params).await;
        }
        Err(self.inactive(project).await)
    }
    pub async fn stop(&self) -> io::Result<()> {
        if self.stopping.swap(true, Ordering::SeqCst) {
            let mut shutdown = self.shutdown.subscribe();
            while !*shutdown.borrow_and_update() {
                if shutdown.changed().await.is_err() {
                    break;
                }
            }
            return Ok(());
        }
        self.finish_stop().await
    }
    async fn finish_stop(&self) -> io::Result<()> {
        // Remote devices lose their tunnel first; the persisted on/off flag is kept.
        if let Some(remote::Remote::Running(host)) = self.remote.get() {
            host.shutdown().await;
        }
        self.voice.lock().await.stop().await;
        self.terminals.lock().await.stop().await;
        let _activations = self.native_activation.write().await;
        let native: Vec<_> = self.state.lock().await.native.values().cloned().collect();
        for runtime in native {
            runtime.shutdown().await;
        }
        let _ = self.shutdown.send_replace(true);
        Ok(())
    }
    pub async fn serve(self: Arc<Self>) -> io::Result<()> {
        let mut shutdown = self.shutdown.subscribe();
        let mut connections = JoinSet::new();
        let supervisor = tokio::spawn(self.clone().supervise());
        let native_events = tokio::spawn(self.clone().native_events());
        // Native projects have no child queue loop; this plans and runs their sprints.
        let native_sprints = tokio::spawn(self.clone().native_sprints());
        loop {
            if *shutdown.borrow() {
                break;
            }
            tokio::select! {
                _=shutdown.changed()=>(),
                // At the connection bound the next caller waits in the listen backlog until a
                // connection ends. Accepting and dropping it would read as a lost service to a
                // client whose own busy reads hold the other connections.
                accepted=self.endpoint.listener.accept(),if connections.len()<CONNECTION_LIMIT=>{
                    let (stream,_)=accepted?;
                    if local::authorize(&stream).is_err() { continue; }
                    let service=self.clone();
                    connections.spawn(async move { let _=service.client(stream).await; });
                }
                _=connections.join_next(),if !connections.is_empty()=>(),
            }
        }
        supervisor.abort();
        native_events.abort();
        native_sprints.abort();
        connections.abort_all();
        while connections.join_next().await.is_some() {}
        // Abort requests alone do not drop task-owned Arcs or the endpoint lock.
        // Observe both background owners before reporting the server settled.
        let _ = supervisor.await;
        let _ = native_events.await;
        Ok(())
    }
    async fn client(self: Arc<Self>, stream: UnixStream) -> io::Result<()> {
        local::authorize(&stream)?;
        let (mut reader, mut writer) = stream.into_split();
        let mut subscription = None;
        loop {
            let message = {
                // read_exact is not cancellation-safe after consuming a frame prefix.
                // Keep the same future alive while subscription events are delivered.
                let frame = read_frame(&mut reader);
                tokio::pin!(frame);
                loop {
                    if let Some(receiver) = subscription.as_mut() {
                        tokio::select! {
                            message=&mut frame=>break message?,
                            event=self.receive_event(receiver)=>{
                                let event=event?;
                                timeout(self.config.request_timeout,write_frame(&mut writer,&json!({"jsonrpc":"2.0","method":"service.event","params":event})))
                                    .await.map_err(|_|failure("client_write_timeout"))??;
                            }
                        }
                    } else {
                        break frame.await?;
                    }
                }
            };
            let parsed = serde_json::from_value::<Request>(message);
            let request = match parsed {
                Ok(r)
                    if r.jsonrpc == "2.0"
                        && (r.id.is_string() || r.id.is_i64() || r.id.is_u64()) =>
                {
                    r
                }
                _ => return Err(failure("invalid_request")),
            };
            let stopping = request.method == "service.stop" && request.params == json!({});
            let result = match request.method.as_str() {
                "service.subscribe" if request.params == json!({}) => {
                    self.accepting()?;
                    subscription = Some(self.subscribe());
                    Ok(json!({"subscribed":true}))
                }
                _ => self.dispatch(&request.method, request.params).await,
            };
            if stopping && result.is_ok() && !self.stopping.swap(true, Ordering::SeqCst) {
                // Shutdown belongs to the service, even if the requester disappears
                // before receiving its acknowledgement. Refuse new work immediately.
                let service = self.clone();
                tokio::spawn(async move {
                    let _ = service.finish_stop().await;
                });
            }
            let response = match result {
                Ok(value) => json!({"jsonrpc":"2.0","id":request.id,"result":value}),
                Err(error) => {
                    json!({"jsonrpc":"2.0","id":request.id,"error":{"code":-32000,"message":error.to_string()}})
                }
            };
            timeout(
                self.config.request_timeout,
                write_frame(&mut writer, &response),
            )
            .await
            .map_err(|_| failure("client_write_timeout"))??;
            if stopping {
                return Ok(());
            }
        }
    }
    async fn dispatch(&self, method: &str, params: Value) -> io::Result<Value> {
        self.accepting()?;
        match method {
            "service.status" if params == json!({}) => Ok(self.status().await),
            "service.stop" if params == json!({}) => Ok(json!({"outcome":"stop_requested"})),
            "service.capabilities" if params == json!({}) => Ok(
                json!({"channel":workspace_protocol::channel::current().name,"application":workspace_protocol::channel::current().title,"methods":["native.query","native.board","service.status","service.capabilities","service.subscribe","service.stop","projects.list","projects.register","projects.rename","projects.remove","projects.activate","projects.status","workspace","executions.cancel","executions.prompt","executions.control","machine.call","directories.search","directories.cancel","terminal.open","terminal.read","terminal.write","terminal.resize","terminal.close","voice.start","voice.read","voice.stop","voice.cancel","remote.status","remote.enable","remote.pairing.start","remote.pairing.cancel","remote.pairing.decide","remote.devices.revoke","remote.relay.set","notifications.report","notifications.opened","notifications.take"],"machine":true,"permissions":false,"sessionOptions":false,
                    // The native host runs every task in the project folder (no task worktrees).
                    "taskWorktrees":false}),
            ),
            "projects.list" if params == json!({}) => Ok(self.projects().await),
            "projects.register" => {
                let p: Registration =
                    serde_json::from_value(params).map_err(|_| failure("invalid_params"))?;
                if p.new_folder {
                    self.register_new_folder(&p.root, &p.display_name).await
                } else {
                    self.register_folder(&p.root, &p.display_name, p.create)
                        .await
                }
            }
            method if method.starts_with("directories.") => {
                let request = workspace_protocol::directories::parse(method, params)
                    .map_err(|_| failure("invalid_directory_search"))?;
                self.directories(request).await
            }
            "projects.rename" => {
                let p: Rename =
                    serde_json::from_value(params).map_err(|_| failure("invalid_params"))?;
                self.rename(&p.project_id, &p.display_name).await
            }
            "projects.remove" => {
                let p: Removal =
                    serde_json::from_value(params).map_err(|_| failure("invalid_params"))?;
                if !p.confirmed {
                    return Err(failure("confirmation_required"));
                }
                self.remove(&p.project_id).await
            }
            "projects.activate" | "projects.status" => {
                let p: ProjectParam =
                    serde_json::from_value(params).map_err(|_| failure("invalid_params"))?;
                match method {
                    "projects.activate" => self.activate(&p.project_id).await,
                    _ => self.project_status(&p.project_id).await,
                }
            }
            // Owner-only: the remote tunnel's policy refuses every `remote.*` method.
            method if method.starts_with("remote.") => {
                remote::call(self.remote.get(), method, params).await
            }
            method if method.starts_with("voice.") => {
                let request = workspace_protocol::voice::parse(method, params)
                    .map_err(|_| failure("invalid_voice_request"))?;
                self.voice.lock().await.call(&self.config, request).await
            }
            method if method.starts_with("terminal.") => {
                let request = workspace_protocol::terminal::parse(method, params)
                    .map_err(|_| failure("invalid_terminal_request"))?;
                let project = match &request {
                    workspace_protocol::terminal::Request::Open(p) => &p.project_id,
                    workspace_protocol::terminal::Request::Read(p) => &p.project_id,
                    workspace_protocol::terminal::Request::Write(p) => &p.project_id,
                    workspace_protocol::terminal::Request::Resize(p) => &p.project_id,
                    workspace_protocol::terminal::Request::Close(p) => &p.project_id,
                };
                // Hold registry ownership through the operation to serialize removal/open.
                let state = self.state.lock().await;
                let project = project.clone();
                let root = &state
                    .projects
                    .iter()
                    .find(|p| p.project_id == project)
                    .ok_or_else(|| failure("project_not_found"))?
                    .canonical_root;
                let mut terminals = self.terminals.lock().await;
                self.accepting()?;
                terminals.call(&project, root, request).await
            }
            "native.query" | "native.board" => self.native_query(params).await,
            "workspace" => self.workspace(params).await,
            "executions.control" => self.control_execution(params).await,
            "executions.cancel" => {
                let p: ExecutionParam =
                    serde_json::from_value(params).map_err(|_| failure("invalid_params"))?;
                self.cancel_execution(&p.project_id, &p.execution_id).await
            }
            "executions.prompt" => {
                let p: PromptParam =
                    serde_json::from_value(params).map_err(|_| failure("invalid_params"))?;
                self.prompt_execution(
                    &p.project_id,
                    &p.execution_id,
                    &p.text,
                    p.request_id.as_deref(),
                    p.attachments,
                )
                .await
            }
            "machine.call" => {
                // An owner action on the board (accept, close, a sprint control) can resolve an
                // owner moment without a runtime event; the notifier reads that project again.
                let project = matches!(
                    params["op"].as_str(),
                    Some("product.task" | "product.sprint" | "product.integrate")
                )
                .then(|| params["projectId"].as_str().map(str::to_owned))
                .flatten();
                let call = serde_json::from_value(params).map_err(|_| failure("invalid_params"))?;
                let reply = self.machine(call).await;
                if let Some(project) = project {
                    self.notifier.touch(&project);
                }
                reply
            }
            method if method.starts_with("notifications.") => {
                self.notifications_call(method, params).await
            }
            _ => Err(failure("unsupported_method")),
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    jsonrpc: String,
    id: Value,
    method: String,
    params: Value,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Registration {
    root: PathBuf,
    display_name: String,
    #[serde(default)]
    create: bool,
    #[serde(default)]
    new_folder: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Rename {
    project_id: String,
    display_name: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Removal {
    project_id: String,
    confirmed: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProjectParam {
    project_id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ExecutionParam {
    project_id: String,
    execution_id: String,
}
fn empty_attachments() -> Value {
    json!([])
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PromptParam {
    project_id: String,
    execution_id: String,
    text: String,
    #[serde(default = "empty_attachments")]
    attachments: Value,
    #[serde(default)]
    request_id: Option<String>,
}
