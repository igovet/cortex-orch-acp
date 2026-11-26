//! Owner-only external MCP registry and per-channel attachment. A server is
//! contained (Landlock: the real filesystem readable except the owner's secrets
//! and Cortex stores, writes only to its grants) unless the owner explicitly sets
//! `"containment": "none"` on it or as the registry default.
//! Configuration and environment values stay in memory, never task reports.
use crate::{native_routes::RouteEnvironment, native_sandbox::Sandbox};
use cortex_core::{CoreError, CoreResult};
use cortex_engine::{
    broker::{Broker, RelayEntry},
    environment::EngineKind,
    external::{Containment, ExternalCore, ExternalLaunch, ExternalProxy, Grants},
    gateway::{Channel, Credentials},
    Error,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::fs::DirBuilderExt,
    path::{Path, PathBuf},
    sync::Arc,
};

fn invalid() -> CoreError {
    CoreError::Refused("invalid_mcp_registry")
}
/// Where the owner's MCP registry for one project lives: `cortex_dir` is the
/// owner's Cortex configuration directory (the folder that holds the global
/// `mcp-servers.json`). The folder name is a digest of the project's logical
/// root (a linked checkout shares its main project's registry), so it never
/// sits in the project tree and its contents cannot be changed by the project.
pub fn project_registry_path(cortex_dir: &Path, root: &Path) -> CoreResult<PathBuf> {
    use sha2::{Digest, Sha256};
    let logical = crate::native_runtime::key(root)?;
    let digest = Sha256::digest(logical.root().as_os_str().as_encoded_bytes());
    let name: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    Ok(cortex_dir
        .join("projects")
        .join(name)
        .join("mcp-servers.json"))
}
fn strings(v: Option<&Value>) -> CoreResult<Vec<String>> {
    match v {
        None | Some(Value::Null) => Ok(vec![]),
        Some(v) => v
            .as_array()
            .ok_or_else(invalid)?
            .iter()
            .map(|v| {
                v.as_str()
                    .filter(|s| !s.contains('\0'))
                    .map(str::to_owned)
                    .ok_or_else(invalid)
            })
            .collect(),
    }
}
fn subset(values: &[String], allowed: &[String]) -> CoreResult<()> {
    if values.iter().any(|v| !allowed.contains(v)) {
        return Err(invalid());
    }
    Ok(())
}
fn filter(v: Option<&Value>, allowed: &[&str]) -> CoreResult<Option<Vec<String>>> {
    let Some(v) = v else { return Ok(None) };
    let s = strings(Some(v))?;
    if s.is_empty()
        || s.iter().any(|v| !allowed.contains(&v.as_str()))
        || s.iter().collect::<BTreeSet<_>>().len() != s.len()
    {
        return Err(invalid());
    }
    Ok(Some(s))
}
#[derive(Clone)]
struct Server {
    name: String,
    command: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    allowed: Vec<String>,
    readonly: Vec<String>,
    lookup: Option<Vec<String>>,
    eager: Vec<String>,
    reads: Vec<PathBuf>,
    writes: Vec<PathBuf>,
    runtime_env: Vec<String>,
    /// Accepted for compatibility; every server shares the host network.
    #[allow(dead_code)]
    network: bool,
    roles: Option<Vec<String>>,
    engines: Option<Vec<String>>,
    containment: Mode,
}
/// How an external server runs. `Uncontained` is an explicit owner opt-out: the
/// server runs with the owner's full user authority and its path grants and
/// network flag are not enforced (every server shares the host network). Tool grants and the relay still apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Contained,
    Uncontained,
}
fn mode(v: Option<&Value>, default: Mode) -> CoreResult<Mode> {
    match v {
        None => Ok(default),
        Some(v) => match v.as_str() {
            Some("contained") => Ok(Mode::Contained),
            Some("none") => Ok(Mode::Uncontained),
            _ => Err(invalid()),
        },
    }
}
impl Server {
    fn grants(&self) -> Grants {
        Grants {
            name: self.name.clone(),
            allowed_tools: self.allowed.iter().cloned().collect(),
            read_only_tools: self.readonly.iter().cloned().collect(),
            coordinator_tools: self.lookup.as_ref().map(|v| v.iter().cloned().collect()),
            always_load: self.eager.iter().cloned().collect(),
        }
    }
}
#[derive(Clone, Default)]
pub struct Registry {
    servers: Vec<Server>,
    /// Machine entries the owner switched off (`enabled: false`) that are otherwise valid:
    /// a project may switch one on for itself, and only through this list.
    dormant: Vec<Server>,
    home: PathBuf,
    /// The Cortex process's own XDG_RUNTIME_DIR, passed only to uncontained servers.
    owner_runtime: Option<PathBuf>,
    /// Where this registry was loaded from, so each session start re-reads the owner files
    /// (a Settings save or file edit applies to the next session without a service restart).
    source: Option<(RouteEnvironment, PathBuf)>,
}
impl Registry {
    /// No project-local file and no writes or seeding. Explicit environment supports
    /// channel-specific configuration and fully disposable tests.
    pub fn load(env: &RouteEnvironment) -> CoreResult<Self> {
        env.validate()?;
        let raw = crate::native_routes::read(&env.config.join("cortex/mcp-servers.json"), true)?;
        let mut registry = Self::parse(raw.as_ref(), env.home.clone())?;
        registry.owner_runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute());
        Ok(registry)
    }
    /// The owner-global registry plus the owner's registry for the project at
    /// `root` (see [`project_registry_path`]). The project file lives in Cortex's
    /// own owner-private configuration, never in the project tree, so project
    /// contents cannot install an executable capability.
    pub fn load_for_project(env: &RouteEnvironment, root: &Path) -> CoreResult<Self> {
        let registry = Self::load(env)?;
        let path = project_registry_path(&env.config.join("cortex"), root)?;
        let raw = crate::native_routes::read(&path, true)?;
        let mut registry = registry.with_project(raw.as_ref())?;
        registry.source = Some((env.clone(), root.to_path_buf()));
        Ok(registry)
    }
    /// Effective servers for one project: every global server, adjusted by the project
    /// file. A project entry in `servers` replaces the machine entry of that name as a
    /// whole (even when the project entry is disabled). `overrides` maps a machine server
    /// name to `false` (off for this project) or `true` (on for this project, using the
    /// machine entry unchanged, including a machine entry that is switched off). Project
    /// entries pass the same validation, grants and containment as machine ones. A
    /// malformed project file refuses; it never falls back to the machine registry, which
    /// could re-enable a server the project switched off.
    pub fn with_project(mut self, raw: Option<&Value>) -> CoreResult<Self> {
        let Some(raw) = raw else { return Ok(self) };
        let overrides = match raw.get("overrides") {
            None | Some(Value::Null) => BTreeMap::new(),
            Some(v) => v
                .as_object()
                .ok_or_else(invalid)?
                .iter()
                .map(|(k, v)| match v.as_bool() {
                    Some(on) if !k.is_empty() && !["cortex", "coordinator", "worker"].contains(&k.as_str()) => Ok((k.clone(), on)),
                    _ => Err(invalid()),
                })
                .collect::<CoreResult<BTreeMap<_, _>>>()?,
        };
        let own = Self::parse(Some(raw), self.home.clone())?;
        let named: BTreeSet<&str> = raw
            .get("servers")
            .or_else(|| raw.get("mcpServers"))
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|m| m.keys().map(String::as_str))
            .collect();
        if overrides.keys().any(|k| named.contains(k.as_str())) {
            return Err(invalid());
        }
        let mut dormant = std::mem::take(&mut self.dormant);
        let mut servers = std::mem::take(&mut self.servers);
        servers.retain(|s| !named.contains(s.name.as_str()) && overrides.get(&s.name) != Some(&false));
        for (name, on) in &overrides {
            if *on {
                if let Some(at) = dormant.iter().position(|s| &s.name == name) {
                    servers.push(dormant.remove(at));
                }
            }
        }
        servers.extend(own.servers);
        self.servers = servers;
        Ok(self)
    }
    /// The registry as the owner files read now; `None` for one not loaded from files.
    pub(crate) fn reloaded(&self) -> CoreResult<Option<Self>> {
        self.source
            .as_ref()
            .map(|(env, root)| Self::load_for_project(env, root))
            .transpose()
    }
    fn parse(raw: Option<&Value>, home: PathBuf) -> CoreResult<Self> {
        let mut servers = vec![];
        let mut dormant = vec![];
        if let Some(raw) = raw {
            // Registry-wide default for entries without their own `containment`.
            let default = mode(raw.get("containment"), Mode::Contained)?;
            let entries = raw
                .get("servers")
                .or_else(|| raw.get("mcpServers"))
                .and_then(Value::as_object)
                .ok_or_else(invalid)?;
            for (name, spec) in entries {
                if name.is_empty()
                    || ["cortex", "coordinator", "worker"].contains(&name.as_str())
                    || !spec.is_object()
                {
                    return Err(invalid());
                }
                if spec.get("enabled") == Some(&Value::Bool(false)) {
                    // Kept apart, so a project can switch it on. An entry that would not
                    // load if enabled is dropped here and cannot be switched on.
                    let mut on = spec.clone();
                    on["enabled"] = Value::Bool(true);
                    let mut single = serde_json::Map::new();
                    single.insert(name.clone(), on);
                    let mut one = serde_json::json!({ "servers": single });
                    if let Some(default) = raw.get("containment") {
                        one["containment"] = default.clone();
                    }
                    if let Ok(one) = Self::parse(Some(&one), home.clone()) {
                        dormant.extend(one.servers);
                    }
                    continue;
                }
                let command = spec["command"]
                    .as_str()
                    .filter(|s| !s.is_empty() && !s.contains('\0'))
                    .ok_or_else(invalid)?
                    .to_owned();
                let args = strings(spec.get("args"))?;
                let allowed = strings(spec.get("allowed_tools"))?;
                if allowed.is_empty() || allowed.iter().any(String::is_empty) {
                    return Err(CoreError::Refused(
                        "external_mcp_explicit_tool_grants_required",
                    ));
                }
                let readonly = strings(spec.get("read_only_tools"))?;
                subset(&readonly, &allowed)?;
                let lookup = spec
                    .get("coordinator_tools")
                    .map(|v| strings(Some(v)))
                    .transpose()?;
                if let Some(v) = &lookup {
                    subset(v, &readonly)?;
                }
                let eager = strings(spec.get("always_load_tools"))?;
                subset(&eager, &allowed)?;
                let runtime_env = strings(spec.get("runtime_dir_env"))?;
                if runtime_env.iter().collect::<BTreeSet<_>>().len() != runtime_env.len()
                    || runtime_env.iter().any(|v| !env_name(v) || reserved(v))
                {
                    return Err(invalid());
                }
                let paths = |key| -> CoreResult<Vec<PathBuf>> {
                    strings(spec.get(key))?
                        .into_iter()
                        .map(|v| {
                            let p = PathBuf::from(v);
                            if !p.is_absolute() {
                                Err(invalid())
                            } else {
                                Ok(p)
                            }
                        })
                        .collect()
                };
                let mut environment = BTreeMap::new();
                if let Some(e) = spec.get("env").filter(|v| !v.is_null()) {
                    if let Some(e) = e.as_object() {
                        for (k, v) in e {
                            environment
                                .insert(k.clone(), v.as_str().ok_or_else(invalid)?.to_owned());
                        }
                    } else if let Some(e) = e.as_array() {
                        for v in e {
                            environment.insert(
                                v["name"].as_str().ok_or_else(invalid)?.into(),
                                v["value"].as_str().ok_or_else(invalid)?.into(),
                            );
                        }
                    } else {
                        return Err(invalid());
                    }
                }
                if environment.iter().any(|(k, v)| {
                    k.is_empty()
                        || k.contains(['=', '\0'])
                        || reserved(k)
                        || k.starts_with("CORTEX_CHANNEL_")
                        || v.contains('\0')
                }) {
                    return Err(invalid());
                }
                let network = spec
                    .get("network")
                    .map(|v| v.as_bool().ok_or_else(invalid))
                    .transpose()?
                    .unwrap_or(false);
                servers.push(Server {
                    name: name.clone(),
                    command,
                    args,
                    env: environment,
                    allowed,
                    readonly,
                    lookup,
                    eager,
                    reads: paths("read_paths")?,
                    writes: paths("write_paths")?,
                    runtime_env,
                    network,
                    roles: filter(spec.get("roles"), &["coordinator", "worker"])?,
                    engines: filter(
                        spec.get("engines"),
                        &["codex-acp", "claude-agent-acp", "opencode"],
                    )?,
                    containment: mode(spec.get("containment"), default)?,
                });
            }
        }
        Ok(Self {
            servers,
            dormant,
            home,
            owner_runtime: None,
            source: None,
        })
    }
    /// Client duplicates cannot replace the owner's executable, secrets or grants.
    /// Like Python `load_mcp_servers`/`host_mcp_servers`, entries no engine is ever
    /// handed are accepted and ignored: a later duplicate name, `cortex` or a role
    /// name, and an entry without a stdio `command` (an `http`/`sse` server such as
    /// the one Paseo injects into every session). An unconfigured stdio executable,
    /// which Python would forward, is still refused.
    pub fn validate_client(&self, entries: &Value) -> CoreResult<()> {
        if entries.is_null() {
            return Ok(());
        }
        let entries = entries.as_array().ok_or_else(invalid)?;
        let mut seen = BTreeSet::new();
        for e in entries {
            let name = e["name"].as_str().ok_or_else(invalid)?;
            let stdio = e["command"].as_str().is_some_and(|c| !c.is_empty());
            if !seen.insert(name)
                || ["cortex", "coordinator", "worker"].contains(&name)
                || !stdio
                || self.servers.iter().any(|s| s.name == name)
            {
                continue;
            }
            return Err(CoreError::Refused("external_mcp_server_unconfigured"));
        }
        Ok(())
    }
    /// Attaches every eligible server. A server whose grants, worker scopes,
    /// containment, launch or relay cannot be prepared is withheld from this
    /// session and reported in `withheld`; it never runs with less containment
    /// than its registry entry names (an explicit `"containment": "none"`).
    /// Only an unconfirmed cleanup of an already launched server is an error.
    pub(crate) async fn attach(
        &self,
        channel: &Channel,
        engine: EngineKind,
        root: &Path,
        scopes: &[PathBuf],
        parent: &Path,
        relay: &Path,
        credentials: impl Fn() -> Result<Credentials, Error>,
        core: Arc<dyn ExternalCore>,
        result: &mut Vec<Attachment>,
        withheld: &mut Vec<Withheld>,
    ) -> Result<(), Error> {
        if matches!(
            channel.profile.as_deref(),
            Some("worker-senior-consultant" | "senior-consultant")
        ) {
            return Ok(());
        }
        // Re-read the owner registry now: a host opened before the owner switched a server on
        // (or off) for this project must not keep serving its start-up snapshot. An unreadable
        // file attaches nothing and says so; it never falls back to the old snapshot.
        let current;
        let this = match self.reloaded() {
            Ok(Some(fresh)) => {
                current = fresh;
                &current
            }
            Ok(None) => self,
            Err(_) => {
                    withheld.push(Withheld {
                        server: "(mcp registry)".into(),
                        reason: "invalid_mcp_registry",
                        detail: None,
                    });
                    return Ok(());
                }
        };
        for server in &this.servers {
            if server
                .roles
                .as_ref()
                .is_some_and(|v| !v.iter().any(|r| r == &channel.role))
                || server
                    .engines
                    .as_ref()
                    .is_some_and(|v| !v.iter().any(|e| e == engine.name()))
            {
                continue;
            }
            let mut grants = server.grants();
            if grants.narrow(channel).is_err() {
                continue;
            }
            // Missing paths are not advertised, matching the Python attachment filter.
            // An uncontained server's path grants are not enforced, so they are ignored.
            if server.containment == Mode::Contained
                && server
                    .reads
                    .iter()
                    .chain(&server.writes)
                    .any(|p| !p.exists())
            {
                continue;
            }
            let before = result.len();
            let launched = this
                .attach_one(
                    server,
                    grants,
                    channel,
                    root,
                    scopes,
                    parent,
                    relay,
                    &credentials,
                    core.clone(),
                    result,
                )
                .await;
            if let Err(reason) = launched {
                // attach_one pushes its attachment before any process can exist.
                if result.len() > before {
                    let a = result.pop().unwrap();
                    if a.shutdown().await.is_err() {
                        // Process absence is unconfirmed: the session start fails
                        // and its cleanup records an unknown observation.
                        return Err(Error::Io);
                    }
                }
                withheld.push(Withheld {
                    server: server.name.clone(),
                    reason,
                    detail: None,
                });
            }
        }
        Ok(())
    }
    async fn attach_one(
        &self,
        server: &Server,
        grants: Grants,
        channel: &Channel,
        root: &Path,
        scopes: &[PathBuf],
        parent: &Path,
        relay: &Path,
        credentials: &impl Fn() -> Result<Credentials, Error>,
        core: Arc<dyn ExternalCore>,
        result: &mut Vec<Attachment>,
    ) -> Result<(), &'static str> {
        // As the owner's terminal finds it, contained or not.
        let executable = resolve(&server.command)
            .or_else(|e| owner_resolve(&server.command).ok_or(e))
            .map_err(|_| "executable_unavailable")?;
        let runtime = parent.join(format!(
            "external-runtime-{}",
            crate::native_effects::identity().map_err(|_| "runtime_directory_unavailable")?
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&runtime)
            .map_err(|_| "runtime_directory_unavailable")?;
        result.push(Attachment {
            broker: None,
            proxy: None,
            runtime,
            entry: None,
            tools: grants.allowed_tools.len(),
        });
        let attachment = result.last_mut().unwrap();
        let (launch, sandbox) = self.prepare(
            server,
            executable,
            channel,
            root,
            scopes,
            &attachment.runtime,
        )?;
        let credentials = credentials().map_err(|_| "launch_failed")?;
        let proxy = Arc::new(
            match sandbox {
                Some(sandbox) => {
                    ExternalProxy::launch_contained(
                        launch,
                        credentials,
                        core,
                        grants,
                        Arc::new(sandbox),
                    )
                    .await
                }
                // The owner's explicit opt-out: the registry's own executable,
                // still behind the channel-bound relay and in its own process group.
                None => ExternalProxy::launch(launch, credentials, core, grants).await,
            }
            .map_err(|_| "launch_failed")?,
        );
        attachment.proxy = Some(proxy.clone());
        let broker = Broker::new(parent, proxy).map_err(|_| "relay_unavailable")?;
        attachment.entry = Some(
            broker
                .entry(&server.name, relay)
                .map_err(|_| "relay_unavailable")?,
        );
        attachment.broker = Some(broker);
        Ok(())
    }
    /// The launch specification and, for a contained server, its prepared
    /// sandbox. An uncontained server gets no Landlock policy, so its
    /// path grants, worker scopes and the per-file project walk are skipped.
    fn prepare(
        &self,
        server: &Server,
        executable: PathBuf,
        channel: &Channel,
        root: &Path,
        scopes: &[PathBuf],
        runtime: &Path,
    ) -> Result<(ExternalLaunch, Option<Sandbox>), &'static str> {
        let mut environment = server.env.clone();
        environment.insert("HOME".into(), self.home.to_string_lossy().into());
        environment.insert("LANG".into(), "C.UTF-8".into());
        let sandbox = match server.containment {
            // As the owner runs it outside Cortex: the owner's PATH and runtime
            // directory; TMPDIR and `runtime_dir_env` (containment's only writable
            // place) are not applied, like the path grants.
            Mode::Uncontained => {
                environment.insert("PATH".into(), owner_path(&executable, &self.home));
                if let Some(r) = self.owner_runtime.as_ref().and_then(|r| r.to_str()) {
                    environment.insert("XDG_RUNTIME_DIR".into(), r.into());
                }
                None
            }
            Mode::Contained => {
                // The owner's tool search path: the real host tools stay visible.
                environment.insert("PATH".into(), owner_path(&executable, &self.home));
                environment.insert("TMPDIR".into(), runtime.to_string_lossy().into());
                for k in std::iter::once("XDG_RUNTIME_DIR")
                    .chain(server.runtime_env.iter().map(String::as_str))
                {
                    environment.insert(k.into(), runtime.to_string_lossy().into());
                }
                Some(self.contain(server, channel, root, scopes, runtime)?)
            }
        };
        Ok((
            ExternalLaunch {
                executable,
                args: server.args.clone(),
                cwd: root.into(),
                environment,
            },
            sandbox,
        ))
    }
    /// A grant may not be, contain or sit inside Cortex private state or the
    /// owner's Cortex configuration. Checked on the grant itself, its direct
    /// children and well-known private locations, never by walking the tree: a
    /// runtime install (for example Node under NVM) legitimately holds many
    /// symlinks and ordinary files, and the sandbox enforces what a symlink may reach.
    fn guard(&self, p: &Path) -> Result<(), Error> {
        guard_grant(p, &self.home)
    }
    fn contain(
        &self,
        server: &Server,
        channel: &Channel,
        root: &Path,
        scopes: &[PathBuf],
        runtime: &Path,
    ) -> Result<Sandbox, &'static str> {
        let reads = server
            .reads
            .iter()
            .map(|p| grant(p, root))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "grant_path_refused")?;
        let mut writes = server
            .writes
            .iter()
            .map(|p| grant(p, root))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| "grant_path_refused")?;
        if channel.role == "worker"
            && !cortex_core::scheduler::policy::read_only(
                channel.profile.as_deref().unwrap_or(""),
                channel.stage.as_deref().unwrap_or(""),
            )
        {
            for scope in scopes {
                let p = root.join(scope);
                safe(&p).map_err(|_| "worker_scope_refused")?;
                if !p.starts_with(root) || p == root {
                    return Err("worker_scope_refused");
                }
                // A scope the worker has not created yet (or a pattern) names no
                // existing object: Landlock cannot grant it without widening to a
                // parent, so the server simply does not receive it.
                match p.symlink_metadata() {
                    Ok(_) => writes.push(p),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(_) => return Err("worker_scope_refused"),
                }
            }
        }
        // Reject protected descendants rather than granting them via a parent directory.
        for p in reads.iter().chain(&writes) {
            self.guard(p).map_err(|_| "protected_path_refused")?;
        }
        // Read grants are validated like write grants; every non-secret path is readable.
        let _ = reads;
        Sandbox::external(root, &self.home, runtime, &writes)
            .map_err(|_| "containment_unavailable")
    }
}
/// An eligible server this session runs without, and a stable, secret-free reason.
pub(crate) struct Withheld {
    pub server: String,
    pub reason: &'static str,
    /// The server's last stderr line (secret-filtered), when it exited on its own.
    pub detail: Option<String>,
}
pub(crate) struct CorePort {
    pub executor: Arc<cortex_core::store::executor::StoreExecutor>,
    pub credential: cortex_core::channels::Credential,
    pub root: PathBuf,
    pub registry: Registry,
    pub engine: EngineKind,
}
impl CorePort {
    fn credentials(&self, c: &Credentials) -> Result<(), Error> {
        if c.project != self.root
            || c.channel_id != self.credential.channel_id
            || c.secret() != self.credential.secret()
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}
impl ExternalCore for CorePort {
    fn authorize<'a>(
        &'a self,
        c: &'a Credentials,
        server: &'a str,
    ) -> cortex_engine::FutureResult<'a, Channel> {
        Box::pin(async move {
            self.credentials(c)?;
            let credential = self.credential.clone();
            let i = self
                .executor
                .read(move |db| {
                    cortex_core::channels::authenticate(
                        db,
                        &credential.channel_id,
                        credential.secret(),
                        false,
                    )
                })
                .map_err(|_| Error::Forbidden)?
                .await
                .map_err(|_| Error::Closed)?
                .map_err(|_| Error::Forbidden)?;
            let channel = Channel {
                task: i.task,
                attempt: i.attempt,
                owner_epoch: i.epoch as u64,
                principal: i.principal,
                role: i.role,
                profile: i.profile,
                stage: i.stage,
            };
            let spec = self
                .registry
                .servers
                .iter()
                .find(|s| s.name == server)
                .ok_or(Error::Forbidden)?;
            if spec
                .roles
                .as_ref()
                .is_some_and(|v| !v.contains(&channel.role))
                || spec
                    .engines
                    .as_ref()
                    .is_some_and(|v| !v.iter().any(|s| s == self.engine.name()))
            {
                return Err(Error::Forbidden);
            }
            spec.grants().narrow(&channel)?;
            Ok(channel)
        })
    }
    fn reserve<'a>(
        &'a self,
        _: &'a Credentials,
        _: &'a Channel,
        _: &'a str,
        _: Value,
        _: String,
    ) -> cortex_engine::FutureResult<'a, cortex_engine::external::Reservation> {
        Box::pin(async { Err(Error::Forbidden) })
    }
    fn reserve_tool<'a>(
        &'a self,
        c: &'a Credentials,
        expected: &'a Channel,
        server: &'a str,
        id: Value,
        digest: String,
        tool: &'a str,
        read_only: bool,
    ) -> cortex_engine::FutureResult<'a, cortex_engine::external::Reservation> {
        Box::pin(async move {
            let channel = self.authorize(c, server).await?;
            if channel.task != expected.task
                || channel.attempt != expected.attempt
                || channel.owner_epoch != expected.owner_epoch
                || channel.principal != expected.principal
                || channel.role != expected.role
                || channel.profile != expected.profile
                || channel.stage != expected.stage
            {
                return Err(Error::Forbidden);
            }
            let spec = self
                .registry
                .servers
                .iter()
                .find(|s| s.name == server)
                .ok_or(Error::Forbidden)?;
            let mut grants = spec.grants();
            grants.narrow(&channel)?;
            if !grants.allowed_tools.contains(tool)
                || read_only != grants.read_only_tools.contains(tool)
            {
                return Err(Error::Forbidden);
            }
            let credential = self.credential.clone();
            let server = server.to_owned();
            let tool = tool.to_owned();
            let result = self
                .executor
                .write(move |db| {
                    cortex_core::external::reserve_tx(
                        db,
                        &credential,
                        &server,
                        &id,
                        &digest,
                        &tool,
                        read_only,
                    )
                })
                .map_err(|_| Error::Forbidden)?
                .await
                .map_err(|_| Error::Closed)?
                .map_err(|_| Error::Forbidden)?;
            Ok(match result {
                cortex_core::external::Reservation::Dispatch => {
                    cortex_engine::external::Reservation::Dispatch
                }
                cortex_core::external::Reservation::Replay(v) => {
                    cortex_engine::external::Reservation::Replay(v)
                }
                cortex_core::external::Reservation::Unknown => {
                    cortex_engine::external::Reservation::Unknown
                }
            })
        })
    }
    fn complete<'a>(
        &'a self,
        c: &'a Credentials,
        server: &'a str,
        id: Value,
        reply: Value,
    ) -> cortex_engine::FutureResult<'a, ()> {
        Box::pin(async move {
            self.authorize(c, server).await?;
            let credential = self.credential.clone();
            let server = server.to_owned();
            self.executor
                .write(move |db| {
                    cortex_core::external::complete_tx(db, &credential, &server, &id, &reply)
                })
                .map_err(|_| Error::Forbidden)?
                .await
                .map_err(|_| Error::Closed)?
                .map_err(|_| Error::Forbidden)
        })
    }
}
fn env_name(s: &str) -> bool {
    let mut b = s.bytes();
    b.next()
        .is_some_and(|v| v == b'_' || v.is_ascii_uppercase())
        && b.all(|v| v == b'_' || v.is_ascii_uppercase() || v.is_ascii_digit())
}
fn reserved(s: &str) -> bool {
    ["PATH", "HOME", "TMPDIR", "XDG_RUNTIME_DIR"].contains(&s)
}
fn private(p: &Path) -> bool {
    p.components().any(|c| {
        [
            ".git",
            ".cortex",
            ".cortex-dev",
            ".codex",
            ".claude",
            ".ssh",
            ".gnupg",
            ".cortex-dev-owner.json",
            ".cortex-workspace-dev.json",
        ]
        .iter()
        .any(|v| c.as_os_str() == *v)
    })
}
fn safe(p: &Path) -> Result<(), Error> {
    if !p.is_absolute()
        || private(p)
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
        || p.ancestors().any(|p| {
            p.symlink_metadata()
                .is_ok_and(|m| m.file_type().is_symlink())
        })
    {
        return Err(Error::Forbidden);
    }
    Ok(())
}
fn grant(p: &Path, root: &Path) -> Result<PathBuf, Error> {
    safe(p)?;
    if p.starts_with(root) || root.starts_with(p) {
        return Err(Error::Forbidden);
    }
    Ok(p.to_path_buf())
}
/// Bounded protected-path decision for one contained-server grant.
fn guard_grant(p: &Path, home: &Path) -> Result<(), Error> {
    safe(p)?;
    let config = home.join(".config/cortex");
    let data = home.join(".local/share/cortex");
    let state = home.join(".local/state/cortex");
    // Inside the owner's Cortex configuration, or containing it or the stores.
    if p.starts_with(&config)
        || [&config, &data, &state].iter().any(|d| d.starts_with(p))
    {
        return Err(Error::Forbidden);
    }
    // A directory that directly holds private state (live stores, VCS, keys).
    if p.is_dir() {
        for e in std::fs::read_dir(p)? {
            if private(Path::new(&e?.file_name())) {
                return Err(Error::Forbidden);
            }
        }
    }
    Ok(())
}
/// PATH for an uncontained server: the registry executable's own directory
/// (for example a Node install's `bin`), the owner's user bin directories and
/// the system ones. Built, never inherited from the Cortex process.
pub(crate) fn owner_path(executable: &Path, home: &Path) -> String {
    let mut dirs: Vec<PathBuf> = vec![];
    dirs.extend(executable.parent().map(Path::to_path_buf));
    // The owner's tool search path, as their terminal has it (where the service read it).
    if crate::tool_path::started() {
        dirs.extend(crate::tool_path::directories().into_iter().map(|(p, _)| p));
    }
    dirs.extend([home.join(".local/bin"), home.join("bin")]);
    dirs.extend(["/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
    let mut seen = BTreeSet::new();
    dirs.into_iter()
        .filter_map(|d| d.to_str().map(str::to_owned))
        .filter(|d| !d.is_empty() && !d.contains(':') && seen.insert(d.clone()))
        .collect::<Vec<_>>()
        .join(":")
}
/// A bare command name on the owner's tool search path (uncontained servers only).
fn owner_resolve(command: &str) -> Option<PathBuf> {
    if command.is_empty() || command.contains('/') || !crate::tool_path::started() {
        return None;
    }
    crate::tool_path::locate(command).map(|(p, _)| p)
}
fn resolve(command: &str) -> Result<PathBuf, Error> {
    let path = Path::new(command);
    let p = if path.is_absolute() {
        path.into()
    } else {
        ["/usr/bin", "/bin"]
            .iter()
            .map(|d| Path::new(d).join(path))
            .find(|p| p.is_file())
            .ok_or(Error::Invalid)?
    };
    if !p.is_file() {
        return Err(Error::Invalid);
    }
    Ok(p)
}
impl Containment for Sandbox {
    unsafe fn enter(&self) -> std::io::Result<()> {
        unsafe { Sandbox::enter(self) }
    }
}
pub(crate) struct Attachment {
    broker: Option<Broker>,
    proxy: Option<Arc<ExternalProxy>>,
    runtime: PathBuf,
    entry: Option<RelayEntry>,
    /// How many tools the owner's grants allow this session on the server.
    tools: usize,
}
impl Attachment {
    /// Secret-free record of what this session is offered: the name and allowed-tool count.
    pub(crate) fn offered(&self) -> Option<Value> {
        Some(serde_json::json!({"server": self.name()?, "tools": self.tools}))
    }
}
/// Durable `NATIVE_EXTERNAL_MCP_OFFERED` event: the servers a session actually runs with, by
/// name and tool count only (never environment, arguments or paths). Nothing when none.
pub(crate) async fn record_offered(
    executor: &Arc<cortex_core::store::executor::StoreExecutor>,
    credential: &cortex_core::channels::Credential,
    attachments: &[Attachment],
) -> CoreResult<()> {
    let servers: Vec<Value> = attachments.iter().filter_map(Attachment::offered).collect();
    if servers.is_empty() {
        return Ok(());
    }
    let c = credential.clone();
    executor
        .write(move |db| {
            let i = cortex_core::channels::authenticate(db, &c.channel_id, c.secret(), false)?;
            cortex_core::lifecycle::event_tx(
                db,
                &i.task,
                "NATIVE_EXTERNAL_MCP_OFFERED",
                serde_json::json!({"channel_id":c.channel_id,"servers":servers}),
            )?;
            Ok(())
        })?
        .await
        .map_err(|_| CoreError::Refused("session_host_closed"))?
}
impl Attachment {
    pub fn entry(&self) -> RelayEntry {
        self.entry.as_ref().unwrap().clone()
    }
    /// The private directory of this server's relay broker, which a confined
    /// engine's view must show for the relay to reach it.
    pub fn broker_directory(&self) -> Option<PathBuf> {
        self.broker.as_ref().map(|b| b.directory().to_path_buf())
    }
    /// Observed exit of the contained server before or during this session.
    pub fn exited(&self) -> bool {
        self.proxy.as_ref().is_some_and(|p| p.exited())
    }
    pub fn stderr_line(&self) -> Option<String> {
        self.proxy.as_ref().and_then(|p| p.stderr_line())
    }
    pub fn name(&self) -> Option<String> {
        serde_json::to_value(self.entry.as_ref()?).ok()?["name"]
            .as_str()
            .map(str::to_owned)
    }
    pub fn bind(&self, pid: u32) -> Result<(), Error> {
        self.broker.as_ref().unwrap().bind_engine(pid)
    }
    pub async fn shutdown(mut self) -> Result<(), Error> {
        if let Some(b) = self.broker.take() {
            b.shutdown().await?;
        }
        if let Some(p) = self.proxy.take() {
            Arc::try_unwrap(p)
                .map_err(|_| Error::Busy)?
                .shutdown()
                .await?;
        }
        Ok(())
    }
}
impl Drop for Attachment {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.runtime);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn registry_grants_and_filters_are_explicit() {
        let raw = json!({"servers":{"s":{"command":"/bin/true","allowed_tools":["read","write"],
            "read_only_tools":["read"],"coordinator_tools":["read"],"always_load_tools":["read"],
            "env":[{"name":"TOKEN","value":"fixture"}],"roles":["worker"],"engines":["codex-acp"]}}});
        let registry = Registry::parse(Some(&raw), "/nonexistent".into()).unwrap();
        // Accepted and never attached, like Python: owned names keep the owner's
        // spec; reserved names, duplicates and non-stdio servers are ignored.
        for entries in [
            json!([{"name":"s","command":"/never-execute"}]),
            json!([{"type":"http","name":"paseo","url":"http://127.0.0.1:1/mcp",
                "headers":[{"name":"Authorization","value":"Bearer fixture"}]}]),
            json!([{"type":"sse","name":"events","url":"http://127.0.0.1:1/sse"}]),
            json!([{"name":"other"}]),
            json!([{"name":"other","command":""}]),
            json!([{"name":"cortex","command":"/never-execute"}]),
            json!([{"name":"worker","command":"/never-execute"}]),
            json!([{"name":"s"},{"name":"s","command":"/never-execute"}]),
        ] {
            assert!(registry.validate_client(&entries).is_ok(), "{entries}");
        }
        for entries in [
            json!([{"name":"other","command":"/never-execute"}]),
            json!([{"type":"http","name":"paseo","url":"http://127.0.0.1:1/mcp"},
                {"name":"other","command":"/never-execute"}]),
            json!([{"command":"/never-execute"}]),
            json!({}),
        ] {
            assert!(registry.validate_client(&entries).is_err(), "{entries}");
        }
        assert_eq!(
            registry
                .validate_client(&json!([{"name":"other","command":"/never-execute"}]))
                .unwrap_err()
                .code(),
            "external_mcp_server_unconfigured"
        );
        for (key, value) in [
            ("allowed_tools", json!([])),
            ("read_only_tools", json!(["unknown"])),
            ("coordinator_tools", json!(["write"])),
            ("always_load_tools", json!(["unknown"])),
            ("roles", json!(["worker", "worker"])),
            ("engines", json!(["unknown"])),
            ("runtime_dir_env", json!(["HOME"])),
            ("runtime_dir_env", json!(["BAD-NAME"])),
            ("runtime_dir_env", json!(["STATE", "STATE"])),
            ("read_paths", json!(["relative"])),
            ("network", json!("true")),
            ("env", json!({"CORTEX_CHANNEL_SECRET":"fixture"})),
            ("env", json!({"HOME":"/other"})),
        ] {
            let mut invalid = raw.clone();
            invalid["servers"]["s"][key] = value;
            assert!(
                Registry::parse(Some(&invalid), "/nonexistent".into()).is_err(),
                "{key}"
            );
        }
        assert!(Registry::parse(
            Some(&json!({"servers":{"disabled":{"enabled":false}}})),
            "/none".into()
        )
        .unwrap()
        .servers
        .is_empty());
    }
    fn names(registry: &Registry) -> Vec<(String, String)> {
        let mut v: Vec<_> = registry
            .servers
            .iter()
            .map(|s| (s.name.clone(), s.command.clone()))
            .collect();
        v.sort();
        v
    }
    fn global() -> Registry {
        let tools = json!(["read"]);
        Registry::parse(
            Some(&json!({"servers":{
                "a":{"command":"/bin/a","allowed_tools":tools},
                "b":{"command":"/bin/b","allowed_tools":tools},
                "c":{"command":"/bin/c","allowed_tools":tools,"containment":"none"}}})),
            "/nonexistent".into(),
        )
        .unwrap()
    }
    #[test]
    fn project_registry_adds_overrides_and_disables_machine_servers() {
        let tools = json!(["read"]);
        let project = json!({"servers":{
            "b":{"command":"/bin/b-project","allowed_tools":tools},
            "c":{"command":"/bin/c-project","allowed_tools":tools,"enabled":false},
            "d":{"command":"/bin/d","allowed_tools":tools}},"overrides":{"a":false}});
        let merged = global().with_project(Some(&project)).unwrap();
        // The project entry replaces the machine entry as a whole; a disabled project
        // entry hides the machine one; an `overrides` false hides a machine server.
        assert_eq!(
            names(&merged),
            [("b".into(), "/bin/b-project".into()), ("d".into(), "/bin/d".into())]
        );
        // Another project (no file) and an empty project file keep the machine registry.
        let all = ["a", "b", "c"].map(|n| (n.to_owned(), format!("/bin/{n}")));
        assert_eq!(names(&global().with_project(None).unwrap()), all);
        assert_eq!(
            names(&global().with_project(Some(&json!({"servers":{}}))).unwrap()),
            all
        );
        // An override naming a server the machine does not have is harmless.
        assert_eq!(
            names(&global().with_project(Some(&json!({"servers":{},"overrides":{"zzz":false,"yyy":true}}))).unwrap()),
            all
        );
        // Client-supplied entries are validated against the effective registry.
        assert!(merged.validate_client(&json!([{"name":"d","command":"/never"}])).is_ok());
        assert_eq!(
            merged
                .validate_client(&json!([{"name":"a","command":"/never"}]))
                .unwrap_err()
                .code(),
            "external_mcp_server_unconfigured"
        );
    }
    #[test]
    fn a_project_can_switch_on_a_machine_server_that_is_switched_off() {
        let tools = json!(["read"]);
        let machine = Registry::parse(
            Some(&json!({"containment":"none","servers":{
                "kling":{"command":"/bin/kling","allowed_tools":tools,"enabled":false},
                "broken":{"command":"/bin/broken","enabled":false},
                "on":{"command":"/bin/on","allowed_tools":tools}}})),
            "/nonexistent".into(),
        )
        .unwrap();
        let list = |r: &Registry| names(r).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(list(&machine), ["on"]);
        let project = |overrides: Value| json!({"servers":{},"overrides":overrides});
        let kling = machine.clone().with_project(Some(&project(json!({"kling":true})))).unwrap();
        assert_eq!(list(&kling), ["kling", "on"]);
        // The machine entry is used unchanged, including its containment default.
        assert_eq!(kling.servers.iter().find(|s| s.name == "kling").unwrap().containment, Mode::Uncontained);
        // A machine entry that would not load is never switched on by a project.
        let broken = machine.clone().with_project(Some(&project(json!({"broken":true,"kling":true,"on":false})))).unwrap();
        assert_eq!(list(&broken), ["kling"]);
    }
    #[test]
    fn project_registry_keeps_the_same_validation_and_containment_rules() {
        let tools = json!(["read"]);
        let entry = json!({"command":"/bin/d","allowed_tools":tools});
        for (project, why) in [
            (json!({"servers":{"d":{"command":"/bin/d"}}}), "explicit tool grants"),
            (json!({"servers":{"d":{"command":"/bin/d","allowed_tools":[]}}}), "empty grants"),
            (json!({"servers":{"cortex":entry}}), "reserved name"),
            (json!({"servers":{"d":entry},"overrides":{"cortex":false}}), "reserved override"),
            (json!({"servers":{"d":entry},"overrides":{"a":"no"}}), "override shape"),
            (json!({"servers":{"d":entry},"overrides":["a"]}), "override container"),
            (json!({"servers":{"d":entry},"overrides":{"d":false}}), "override and project entry"),
            (json!({"servers":{"d":entry},"containment":"maybe"}), "containment value"),
            (json!({"servers":{"d":{"command":"/bin/d","allowed_tools":tools,"read_only_tools":["x"]}}}), "subset"),
            (json!({"overrides":{"a":false}}), "no servers object"),
        ] {
            assert!(global().with_project(Some(&project)).is_err(), "{why}");
        }
        // The project file's registry default applies to project entries only.
        let merged = global()
            .with_project(Some(&json!({"containment":"none","servers":{"d":entry,
                "e":{"command":"/bin/e","allowed_tools":tools,"containment":"contained"}}})))
            .unwrap();
        let mode = |n: &str| merged.servers.iter().find(|s| s.name == n).unwrap().containment;
        assert_eq!(mode("a"), Mode::Contained);
        assert_eq!(mode("c"), Mode::Uncontained);
        assert_eq!(mode("d"), Mode::Uncontained);
        assert_eq!(mode("e"), Mode::Contained);
        // Without an explicit opt-out a project server is contained.
        let merged = global().with_project(Some(&json!({"servers":{"d":entry}}))).unwrap();
        assert_eq!(
            merged.servers.iter().find(|s| s.name == "d").unwrap().containment,
            Mode::Contained
        );
    }
    #[test]
    fn a_session_in_project_a_gets_its_server_and_project_b_does_not() {
        use std::os::unix::fs::PermissionsExt;
        let base = scratch("project-registry");
        let private = |p: &Path| {
            std::fs::create_dir_all(p).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
        };
        let (a, b) = (base.join("a"), base.join("b"));
        for root in [&a, &b] {
            std::fs::create_dir_all(root).unwrap();
        }
        let (a, b) = (a.canonicalize().unwrap(), b.canonicalize().unwrap());
        let config = base.join("config");
        private(&config);
        private(&config.join("cortex"));
        private(&config.join("cortex/projects"));
        let write = |path: &Path, body: Value| {
            private(path.parent().unwrap());
            std::fs::write(path, serde_json::to_vec(&body).unwrap()).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        };
        let tools = json!(["read"]);
        write(
            &config.join("cortex/mcp-servers.json"),
            json!({"version":1,"servers":{"shared":{"command":"/bin/shared","allowed_tools":tools},
                "kling":{"command":"/bin/kling","allowed_tools":tools}}}),
        );
        let env = RouteEnvironment {
            home: base.clone(),
            config: config.clone(),
            state: base.join("state"),
            routing_override: None,
        };
        // Machine-wide Kling is moved to project A: switched off in B, defined for A.
        write(
            &project_registry_path(&config.join("cortex"), &a).unwrap(),
            json!({"version":1,"servers":{},"overrides":{}}),
        );
        write(
            &project_registry_path(&config.join("cortex"), &b).unwrap(),
            json!({"version":1,"servers":{"extra":{"command":"/bin/extra","allowed_tools":tools}},
                "overrides":{"kling":false}}),
        );
        let in_a = Registry::load_for_project(&env, &a).unwrap();
        let in_b = Registry::load_for_project(&env, &b).unwrap();
        let list = |r: &Registry| names(r).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        assert_eq!(list(&in_a), ["kling", "shared"]);
        assert_eq!(list(&in_b), ["extra", "shared"]);
        // A project with no file sees the machine registry; the machine load is unchanged.
        let other = base.join("c");
        std::fs::create_dir_all(&other).unwrap();
        let other = other.canonicalize().unwrap();
        assert_eq!(list(&Registry::load_for_project(&env, &other).unwrap()), ["kling", "shared"]);
        assert_eq!(list(&Registry::load(&env).unwrap()), ["kling", "shared"]);
        // The file sits outside the project tree and differs per project.
        let (fa, fb) = (
            project_registry_path(&config.join("cortex"), &a).unwrap(),
            project_registry_path(&config.join("cortex"), &b).unwrap(),
        );
        assert_ne!(fa, fb);
        assert!(!fa.starts_with(&a) && fa.starts_with(config.join("cortex/projects")));
        // A malformed project file refuses; it never falls back to the machine registry.
        write(&fa, json!({"servers":[]}));
        assert!(Registry::load_for_project(&env, &a).is_err());
        // A project file that is not private is refused as well.
        write(&fa, json!({"servers":{}}));
        std::fs::set_permissions(&fa, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(Registry::load_for_project(&env, &a).is_err());
        std::fs::remove_dir_all(&base).unwrap();
    }
    #[test]
    fn a_registry_loaded_for_a_project_sees_a_later_override_without_a_restart() {
        use std::os::unix::fs::PermissionsExt;
        let base = scratch("project-reload");
        let private = |p: &Path| {
            std::fs::create_dir_all(p).unwrap();
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
        };
        let root = base.join("p");
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let config = base.join("config");
        private(&config);
        private(&config.join("cortex"));
        private(&config.join("cortex/projects"));
        let write = |path: &Path, body: Value| {
            private(path.parent().unwrap());
            std::fs::write(path, serde_json::to_vec(&body).unwrap()).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
        };
        write(
            &config.join("cortex/mcp-servers.json"),
            json!({"version":1,"servers":{"kling":{"command":"/bin/kling","enabled":false,
                "containment":"none","allowed_tools":["read"]}}}),
        );
        let env = RouteEnvironment {
            home: base.clone(),
            config: config.clone(),
            state: base.join("state"),
            routing_override: None,
        };
        let list = |r: &Registry| names(r).into_iter().map(|(n, _)| n).collect::<Vec<_>>();
        let host = Registry::load_for_project(&env, &root).unwrap();
        assert!(list(&host).is_empty());
        let file = project_registry_path(&config.join("cortex"), &root).unwrap();
        write(&file, json!({"version":1,"servers":{},"overrides":{"kling":true}}));
        // The host's start-up snapshot is stale; the session start re-reads the files.
        assert!(list(&host).is_empty());
        assert_eq!(list(&host.reloaded().unwrap().unwrap()), ["kling"]);
        write(&file, json!({"servers":[]}));
        assert!(host.reloaded().is_err());
        assert!(Registry::default().reloaded().unwrap().is_none());
        std::fs::remove_dir_all(&base).unwrap();
    }
    #[test]
    fn containment_is_explicit_with_a_contained_default() {
        let server = json!({"command":"/bin/true","allowed_tools":["read"]});
        let parse = |raw: Value| Registry::parse(Some(&raw), "/nonexistent".into());
        let modes = |raw: Value| {
            let r = parse(raw).unwrap();
            r.servers
                .iter()
                .map(|s| (s.name.clone(), s.containment))
                .collect::<Vec<_>>()
        };
        // Absent everywhere: contained, unchanged for every existing registry.
        assert_eq!(
            modes(json!({"servers":{"a":server}})),
            [("a".into(), Mode::Contained)]
        );
        let mut none = server.clone();
        none["containment"] = json!("none");
        let mut contained = server.clone();
        contained["containment"] = json!("contained");
        assert_eq!(
            modes(json!({"servers":{"a":none,"b":server}})),
            [
                ("a".into(), Mode::Uncontained),
                ("b".into(), Mode::Contained)
            ]
        );
        // The registry default applies to entries without their own value only.
        assert_eq!(
            modes(json!({"containment":"none","servers":{"a":server,"b":contained}})),
            [
                ("a".into(), Mode::Uncontained),
                ("b".into(), Mode::Contained)
            ]
        );
        assert_eq!(
            modes(json!({"containment":"contained","servers":{"a":server}})),
            [("a".into(), Mode::Contained)]
        );
        for value in [
            json!("off"),
            json!("None"),
            json!(""),
            json!(true),
            json!(false),
            json!(null),
            json!(["none"]),
        ] {
            let mut bad = server.clone();
            bad["containment"] = value.clone();
            assert_eq!(
                parse(json!({"servers":{"a":bad}})).err().map(|e| e.code()),
                Some("invalid_mcp_registry"),
                "{value}"
            );
            assert_eq!(
                parse(json!({"containment":value,"servers":{"a":server}}))
                    .err()
                    .map(|e| e.code()),
                Some("invalid_mcp_registry"),
                "{value}"
            );
        }
    }
    fn worker() -> Channel {
        Channel {
            task: "t".into(),
            attempt: "a".into(),
            owner_epoch: 1,
            principal: "p".into(),
            role: "worker".into(),
            profile: Some("worker-backend-dev".into()),
            stage: Some("execution".into()),
        }
    }
    fn scratch(name: &str) -> PathBuf {
        let base = std::env::temp_dir().join(format!(
            "native-mcp-{name}-{}-{}",
            std::process::id(),
            crate::native_effects::identity().unwrap()
        ));
        std::fs::create_dir_all(base.join("project/src")).unwrap();
        std::fs::create_dir_all(base.join("runtime")).unwrap();
        std::fs::write(base.join("project/src/file"), "x").unwrap();
        base
    }
    #[test]
    fn uncontained_launch_skips_the_walk_and_uses_the_owner_environment() {
        let base = scratch("uncontained");
        let (root, runtime) = (base.join("project"), base.join("runtime"));
        let home = base.join("home");
        // Grants that a contained launch refuses (protected, missing, inside the
        // project) and a scope: none of them is evaluated for an uncontained server.
        let raw = json!({"containment":"none","servers":{"s":{
            "command":"/bin/true","allowed_tools":["read"],
            "read_paths":[home.join(".ssh"), root.join("src")],
            "write_paths":["/nonexistent/cortex-grant"],
            "runtime_dir_env":["STATE_DIR"],"env":{"TOKEN":"fixture"}}}});
        let mut registry = Registry::parse(Some(&raw), home.clone()).unwrap();
        registry.owner_runtime = Some("/run/user/fixture".into());
        let server = &registry.servers[0];
        let before = crate::native_sandbox::WALKS.with(|w| w.get());
        let (launch, sandbox) = registry
            .prepare(
                server,
                "/usr/bin/true".into(),
                &worker(),
                &root,
                &["src".into(), "../escape".into()],
                &runtime,
            )
            .unwrap();
        assert!(sandbox.is_none());
        assert_eq!(crate::native_sandbox::WALKS.with(|w| w.get()), before);
        let env = &launch.environment;
        assert_eq!(env["HOME"], home.to_str().unwrap());
        let path = format!(
            "/usr/bin:{}/.local/bin:{}/bin:/usr/local/bin:/bin",
            home.display(),
            home.display()
        );
        assert_eq!(env["PATH"], path);
        assert_eq!(env["TOKEN"], "fixture");
        // The owner's runtime directory; the private one is containment-only.
        assert_eq!(env["XDG_RUNTIME_DIR"], "/run/user/fixture");
        assert!(!env.contains_key("TMPDIR") && !env.contains_key("STATE_DIR"));
        assert!(!env.keys().any(|k| k.starts_with("CORTEX_CHANNEL_")));
        assert_eq!(launch.executable, Path::new("/usr/bin/true"));
        assert_eq!(launch.cwd, root);
        std::fs::remove_dir_all(&base).unwrap();
    }
    #[test]
    fn contained_default_still_walks_and_refuses_unsafe_grants() {
        let base = scratch("contained");
        let (root, runtime) = (base.join("project"), base.join("runtime"));
        let raw = json!({"servers":{"s":{"command":"/bin/true","allowed_tools":["read"]}}});
        let registry = Registry::parse(Some(&raw), base.join("home")).unwrap();
        let before = crate::native_sandbox::WALKS.with(|w| w.get());
        let prepared = registry.prepare(
            &registry.servers[0],
            "/usr/bin/true".into(),
            &worker(),
            &root,
            &["src".into()],
            &runtime,
        );
        assert_eq!(crate::native_sandbox::WALKS.with(|w| w.get()), before + 1);
        match prepared {
            Ok((launch, sandbox)) => {
                assert!(sandbox.is_some());
                // The owner's tool search path, as an uncontained server has it.
                assert_eq!(
                    launch.environment["PATH"],
                    owner_path(Path::new("/usr/bin/true"), &base.join("home"))
                );
                assert_eq!(launch.environment["TMPDIR"], runtime.to_str().unwrap());
                assert_eq!(
                    launch.environment["XDG_RUNTIME_DIR"],
                    runtime.to_str().unwrap()
                );
            }
            // A host without Landlock ABI 3 withholds the server; never uncontained.
            Err(reason) => assert_eq!(reason, "containment_unavailable"),
        }
        // A grant inside the project is refused before any walk.
        let raw = json!({"servers":{"s":{"command":"/bin/true","allowed_tools":["read"],
            "read_paths":[root.join("src")]}}});
        let registry = Registry::parse(Some(&raw), base.join("home")).unwrap();
        assert_eq!(
            registry
                .prepare(
                    &registry.servers[0],
                    "/usr/bin/true".into(),
                    &worker(),
                    &root,
                    &[],
                    &runtime
                )
                .err(),
            Some("grant_path_refused")
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
    #[test]
    fn contained_grants_with_symlinks_and_auth_dirs_attach_but_private_state_is_refused() {
        use std::os::unix::fs::symlink;
        let base = scratch("nvm");
        let (root, runtime, home) = (
            base.join("project"),
            base.join("runtime"),
            base.join("home"),
        );
        // An NVM-like install: symlinked bin entries, node_modules/.bin links, dotfiles.
        let node = home.join(".nvm/versions/node/v1");
        std::fs::create_dir_all(node.join("bin")).unwrap();
        std::fs::create_dir_all(node.join("lib/node_modules/npm/bin")).unwrap();
        std::fs::create_dir_all(node.join("lib/node_modules/mcp-remote/node_modules/.bin"))
            .unwrap();
        std::fs::write(node.join("lib/node_modules/npm/bin/npm-cli.js"), "x").unwrap();
        std::fs::write(node.join("lib/node_modules/npm/.npmrc"), "x").unwrap();
        symlink("../lib/node_modules/npm/bin/npm-cli.js", node.join("bin/npm")).unwrap();
        symlink(
            "/nonexistent/outside",
            node.join("lib/node_modules/mcp-remote/node_modules/.bin/x"),
        )
        .unwrap();
        let kling = home.join(".local/share/cortex/mcp/kling");
        std::fs::create_dir_all(kling.join("auth")).unwrap();
        std::fs::create_dir_all(kling.join("npm-cache")).unwrap();
        std::fs::write(kling.join("auth/credentials"), "x").unwrap();
        let decide = |reads: Vec<PathBuf>, writes: Vec<PathBuf>| {
            let raw = json!({"servers":{"s":{"command":"/bin/true","allowed_tools":["read"],
                "read_paths":reads,"write_paths":writes,"network":true}}});
            let registry = Registry::parse(Some(&raw), home.clone()).unwrap();
            registry
                .prepare(
                    &registry.servers[0],
                    "/usr/bin/true".into(),
                    &worker(),
                    &root,
                    &[],
                    &runtime,
                )
                .map(|_| ())
                .err()
        };
        // Attached, or withheld only because this host lacks Landlock.
        let ok = decide(vec![node.clone()], vec![kling.clone()]);
        assert!(
            matches!(ok, None | Some("containment_unavailable")),
            "{ok:?}"
        );
        // Private state is still refused.
        std::fs::create_dir_all(home.join("proj/.cortex")).unwrap();
        for bad in [
            home.join("proj"),
            home.join(".config/cortex"),
            home.clone(),
            home.join(".local/share"),
        ] {
            std::fs::create_dir_all(&bad).unwrap();
            assert_eq!(
                decide(vec![bad.clone()], vec![]),
                Some("protected_path_refused"),
                "{bad:?}"
            );
        }
        assert_eq!(
            decide(vec![home.join("proj/.cortex")], vec![]),
            Some("grant_path_refused")
        );
        assert_eq!(decide(vec![root.clone()], vec![]), Some("grant_path_refused"));
        std::fs::remove_dir_all(&base).unwrap();
    }
    /// Owner dry-check, never in the default suite: loads the owner's real registry
    /// read-only (`CORTEX_OWNER_MCP_PROBE_HOME=<owner home>`) and prints, per server,
    /// the attach/withhold decision of the contained path. Nothing is launched.
    #[test]
    #[ignore = "owner dry-check: reads the owner's registry, launches nothing"]
    fn owner_registry_decision() {
        let home = PathBuf::from(std::env::var("CORTEX_OWNER_MCP_PROBE_HOME").expect("owner home"));
        let env = RouteEnvironment {
            config: home.join(".config"),
            state: home.join(".local/state"),
            home,
            routing_override: None,
        };
        let registry = Registry::load(&env).unwrap();
        let base = scratch("owner-decision");
        for server in &registry.servers {
            let decision = match resolve(&server.command) {
                Err(_) => Err("executable_unavailable"),
                Ok(executable) => registry
                    .prepare(
                        server,
                        executable,
                        &worker(),
                        &base.join("project"),
                        &[],
                        &base.join("runtime"),
                    )
                    .map(|_| ()),
            };
            println!(
                "decision server={} containment={:?} -> {}",
                server.name,
                server.containment,
                decision.map_or_else(|r| format!("withhold {r}"), |()| "attach".into())
            );
        }
        std::fs::remove_dir_all(&base).unwrap();
    }
    /// Owner-run probe, never part of the default suite: loads the owner's real
    /// registry read-only through this loader and runs one configured server
    /// through the uncontained launch path (`initialize` and `tools/list` only).
    /// `CORTEX_OWNER_MCP_PROBE=<server>`, `CORTEX_OWNER_MCP_PROBE_ROOT=<project>` and
    /// `CORTEX_OWNER_MCP_PROBE_HOME=<owner home>` and `CORTEX_OWNER_MCP_PROBE_RUNTIME`
    /// (the owner's XDG_RUNTIME_DIR): test processes get a private HOME and XDG tree.
    #[tokio::test]
    #[ignore = "owner probe: reads the owner's registry and runs a real server"]
    async fn owner_registry_probe() {
        struct Port;
        impl ExternalCore for Port {
            fn authorize<'a>(
                &'a self,
                _: &'a Credentials,
                _: &'a str,
            ) -> cortex_engine::FutureResult<'a, Channel> {
                Box::pin(async { Ok(worker()) })
            }
            fn reserve<'a>(
                &'a self,
                _: &'a Credentials,
                _: &'a Channel,
                _: &'a str,
                _: Value,
                _: String,
            ) -> cortex_engine::FutureResult<'a, cortex_engine::external::Reservation> {
                Box::pin(async { Err(Error::Forbidden) })
            }
            fn complete<'a>(
                &'a self,
                _: &'a Credentials,
                _: &'a str,
                _: Value,
                _: Value,
            ) -> cortex_engine::FutureResult<'a, ()> {
                Box::pin(async { Err(Error::Forbidden) })
            }
        }
        use cortex_engine::broker::Handler;
        let name = std::env::var("CORTEX_OWNER_MCP_PROBE").expect("server name");
        let root = PathBuf::from(std::env::var("CORTEX_OWNER_MCP_PROBE_ROOT").expect("project"));
        let home = PathBuf::from(std::env::var("CORTEX_OWNER_MCP_PROBE_HOME").expect("owner home"));
        let env = RouteEnvironment {
            config: home.join(".config"),
            state: home.join(".local/state"),
            home,
            routing_override: None,
        };
        let view = crate::config_read_model::ConfigurationReader::new(env.clone())
            .unwrap()
            .mcp_management()
            .unwrap();
        let mut registry = Registry::load(&env).unwrap();
        // The owner's runtime directory, as the service process would pass it.
        registry.owner_runtime =
            std::env::var_os("CORTEX_OWNER_MCP_PROBE_RUNTIME").map(PathBuf::from);
        println!(
            "registry default={} servers={:?} management_valid={:?}",
            view["containment"],
            registry
                .servers
                .iter()
                .map(|s| (s.name.as_str(), s.containment))
                .collect::<Vec<_>>(),
            view["servers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| (s["name"].as_str().unwrap(), s["valid"] == true))
                .collect::<Vec<_>>()
        );
        let server = registry.servers.iter().find(|s| s.name == name).unwrap();
        let base = scratch("owner-probe");
        let runtime = base.join("runtime");
        // As in attach_one: a private 0700 runtime directory.
        use std::os::unix::fs::PermissionsExt;
        for p in [&base, &runtime] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let executable = resolve(&server.command).unwrap();
        let walks = crate::native_sandbox::WALKS.with(|w| w.get());
        let started = std::time::Instant::now();
        let (launch, sandbox) = registry
            .prepare(
                server,
                executable.clone(),
                &worker(),
                &root,
                &["src".into()],
                &runtime,
            )
            .unwrap();
        let uncontained_prepare = started.elapsed();
        assert!(sandbox.is_none());
        assert_eq!(crate::native_sandbox::WALKS.with(|w| w.get()), walks);
        let started = std::time::Instant::now();
        let proxy = ExternalProxy::launch(
            launch,
            Credentials::new(
                "ch_0123456789ab".into(),
                root.clone(),
                "owner-probe-fixture-0001".into(),
            )
            .unwrap(),
            Arc::new(Port),
            server.grants(),
        )
        .await
        .unwrap();
        let init = proxy
            .handle(
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
                "protocolVersion":"2025-06-18","capabilities":{},
                "clientInfo":{"name":"cortex-owner-probe","version":"1.0.6"}}}),
            )
            .await
            .unwrap()
            .unwrap();
        proxy
            .handle(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
            .await
            .unwrap();
        let list = proxy
            .handle(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
            .await
            .unwrap()
            .unwrap();
        let handshake = started.elapsed();
        let tools: Vec<_> = list["result"]["tools"]
            .as_array()
            .unwrap_or_else(|| panic!("tools/list failed: {}", list["error"]))
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_owned())
            .collect();
        println!(
            "server={name} protocol={} serverInfo.name={} granted_tools_listed={} {:?} uncontained_prepare={:?} launch+handshake={:?}",
            init["result"]["protocolVersion"],
            init["result"]["serverInfo"]["name"],
            tools.len(),
            tools,
            uncontained_prepare,
            handshake
        );
        assert!(!tools.is_empty());
        proxy.shutdown().await.unwrap();
        // The cost the opt-out removes: the same server prepared contained.
        let mut contained = server.clone();
        contained.containment = Mode::Contained;
        let started = std::time::Instant::now();
        let walked = registry.prepare(
            &contained,
            executable,
            &worker(),
            &root,
            &["src".into()],
            &runtime,
        );
        println!(
            "contained prepare (per-file walk) of {}: {:?}, outcome={}",
            root.display(),
            started.elapsed(),
            match &walked {
                Ok(_) => "prepared",
                Err(reason) => reason,
            }
        );
        std::fs::remove_dir_all(&base).unwrap();
    }
    /// Owner-run contained probe, never in the default suite: launches one real
    /// registry server inside its prepared containment, captures its stderr and
    /// sends `initialize` and `tools/list` only (never a tool call). Prints the
    /// argv, environment names (never values) and the server's stderr.
    /// Same variables as `owner_registry_probe`.
    #[test]
    #[ignore = "owner probe: reads the owner's registry and runs a real contained server"]
    fn owner_contained_probe() {
        use std::io::{BufRead, Read, Write};
        use std::os::unix::{fs::PermissionsExt, process::CommandExt};
        let name = std::env::var("CORTEX_OWNER_MCP_PROBE").expect("server name");
        let root = PathBuf::from(std::env::var("CORTEX_OWNER_MCP_PROBE_ROOT").expect("project"));
        let home = PathBuf::from(std::env::var("CORTEX_OWNER_MCP_PROBE_HOME").expect("owner home"));
        let env = RouteEnvironment {
            config: home.join(".config"),
            state: home.join(".local/state"),
            home,
            routing_override: None,
        };
        let mut registry = Registry::load(&env).unwrap();
        // A project registry file (`CORTEX_OWNER_MCP_PROBE_PROJECT_REGISTRY`), read only.
        if let Some(file) = std::env::var_os("CORTEX_OWNER_MCP_PROBE_PROJECT_REGISTRY") {
            let raw: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
            registry = registry.with_project(Some(&raw)).unwrap();
        }
        let mut forced = registry.servers.iter().find(|s| s.name == name).unwrap().clone();
        let open = std::env::var("CORTEX_OWNER_MCP_PROBE_MODE").as_deref() == Ok("none");
        if open {
            forced.containment = Mode::Uncontained;
        }
        let server = &forced;
        let base = scratch("owner-contained");
        let runtime = base.join("runtime");
        for p in [&base, &runtime] {
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let (launch, sandbox) = registry
            .prepare(server, resolve(&server.command).unwrap(), &worker(), &root, &[], &runtime)
            .unwrap();
        let sandbox = sandbox.filter(|_| !open);
        println!(
            "argv={:?} env_names={:?} path={}",
            launch.args,
            launch.environment.keys().collect::<Vec<_>>(),
            launch.environment["PATH"]
        );
        let mut command = std::process::Command::new(&launch.executable);
        if let Some(sandbox) = sandbox {
            unsafe { command.pre_exec(move || sandbox.enter()) };
        }
        let mut child = command
            .args(&launch.args)
            .current_dir(&launch.cwd)
            .env_clear()
            .envs(&launch.environment)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut stderr = child.stderr.take().unwrap();
        let errors = std::thread::spawn(move || {
            let mut text = String::new();
            let _ = stderr.read_to_string(&mut text);
            text
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let mut out = std::io::BufReader::new(child.stdout.take().unwrap());
        std::thread::spawn(move || {
            let mut line = String::new();
            while out.read_line(&mut line).unwrap_or(0) > 0 {
                let _ = tx.send(std::mem::take(&mut line));
            }
        });
        let deadline = std::time::Duration::from_secs(120);
        let mut ask = |request: Value| {
            writeln!(stdin, "{request}").unwrap();
            stdin.flush().unwrap();
            let id = request.get("id").cloned();
            while id.is_some() {
                match rx.recv_timeout(deadline) {
                    Ok(line) => {
                        let v: Value = serde_json::from_str(line.trim()).unwrap_or(Value::Null);
                        if v.get("id") == id.as_ref() {
                            return Some(v);
                        }
                    }
                    Err(_) => return None,
                }
            }
            None
        };
        let init = ask(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{
            "protocolVersion":"2025-06-18","capabilities":{},
            "clientInfo":{"name":"cortex-owner-probe","version":"1.0.6"}}}));
        let list = init.as_ref().and_then(|_| {
            ask(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
            ask(json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}))
        });
        let status = child.try_wait().ok().flatten();
        println!(
            "initialize_ok={} serverInfo.name={} tools_listed={:?} exit={status:?}",
            init.is_some(),
            init.as_ref().map_or(Value::Null, |v| v["result"]["serverInfo"]["name"].clone()),
            list.as_ref().and_then(|v| v["result"]["tools"].as_array().map(Vec::len)),
        );
        unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
        let _ = child.kill();
        let _ = child.wait();
        let text = errors.join().unwrap();
        // Stderr of the server (npm/mcp-remote diagnostics); the owner reviews it
        // for secrets before sharing.
        println!("stderr<<\n{}\n>>", text.chars().take(4000).collect::<String>());
        std::fs::remove_dir_all(&base).unwrap();
    }
}

/// One enabled server as a sign-in check starts it: no channel and no tool grant.
pub(crate) struct ProbeTarget {
    pub name: String,
    /// Stable identity of the entry (name, command, arguments), never its environment.
    pub fingerprint: String,
    pub launch: ExternalLaunch,
    pub sandbox: Option<Sandbox>,
}
impl Registry {
    /// The enabled servers of this (project-adjusted) registry, prepared exactly as a
    /// session would start them, for the sign-in check. `runtime` is a private scratch
    /// directory the contained servers use as their temporary place.
    pub(crate) fn probe_targets(&self, root: &Path, runtime: &Path) -> Vec<ProbeTarget> {
        let channel = Channel {
            task: "sign-in-check".into(),
            attempt: "sign-in-check".into(),
            owner_epoch: 0,
            principal: "sign-in-check".into(),
            role: "coordinator".into(),
            profile: None,
            stage: None,
        };
        let mut out = Vec::new();
        for server in &self.servers {
            if server.containment == Mode::Contained
                && server.reads.iter().chain(&server.writes).any(|p| !p.exists())
            {
                continue;
            }
            let Ok(executable) = resolve(&server.command).or_else(|e| match server.containment {
                Mode::Uncontained => owner_resolve(&server.command).ok_or(e),
                Mode::Contained => Err(e),
            }) else {
                continue;
            };
            let Ok((launch, sandbox)) = self.prepare(server, executable, &channel, root, &[], runtime) else {
                continue;
            };
            let fingerprint = {
                use sha2::{Digest, Sha256};
                let mut h = Sha256::new();
                h.update(server.name.as_bytes());
                h.update([0]);
                h.update(server.command.as_bytes());
                for a in &server.args {
                    h.update([0]);
                    h.update(a.as_bytes());
                }
                format!("{}:{:x}", server.name, h.finalize())[..server.name.len() + 17].to_owned()
            };
            out.push(ProbeTarget { name: server.name.clone(), fingerprint, launch, sandbox });
        }
        out
    }
}
impl Attachment {
    /// The server's stderr shows it waits for the owner to sign in.
    pub fn needs_sign_in(&self) -> bool {
        self.proxy.as_ref().is_some_and(|p| p.needs_sign_in())
    }
}
