//! Fixed allowlist, explicit native-account HOME, no channel credentials in engine env.
//! Python oracle: src/cortex_runtime/connections/environment.py:1-55,77-103,
//! including its pinned `CODEX_PATH`/`NO_BROWSER` pair (lines 162-230).
//! Install verification, pinned launch wrappers and credential selection are service policy.
use crate::{broker::RelayEntry, Error, Result};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EngineKind {
    Codex,
    Claude,
    OpenCode,
}
impl EngineKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "codex-acp",
            Self::Claude => "claude-agent-acp",
            Self::OpenCode => "opencode",
        }
    }
}
/// Codex only: service policy that writes Cortex's private native Codex
/// launcher when the engine actually starts, never while a route is only
/// resolved or presented. `LaunchSpec::launch_environment` exports the
/// returned path as `CODEX_PATH` with `NO_BROWSER=1` and [`CODEX_CONFIG`], so
/// codex-acp runs App Server through it and keeps its session threads' features
/// off; a preparation error refuses the launch.
///
/// A session's MCP relays travel with the launcher, never in ACP `session/new`:
/// codex-acp turns ACP servers into one `mcp_servers` table in each thread's
/// config, and Codex lets that table replace the launcher's own `mcp_servers`
/// layer, which is where the owner's configured servers are turned off. The
/// launcher defines these relays as App Server `-c` layers instead.
#[derive(Clone)]
pub struct CodexLauncher {
    prepare: Arc<dyn Fn() -> Result<PathBuf> + Send + Sync>,
    servers: Vec<RelayEntry>,
}
/// The secret-free relay entries a launcher hands to its App Server
/// (`native_codex_runtime` in the service reads and checks them).
pub const CODEX_MCP_SERVERS: &str = "CORTEX_CODEX_MCP_SERVERS";
/// codex-acp's own session config (a JSON object, read once at startup). codex-acp
/// sends a `features` table in every `thread/start` config; App Server lets that
/// table replace the whole `features` layer of its `-c` flags, so a feature the
/// launcher turns off there (plugins, whose MCP servers then start) is back on for
/// the thread. codex-acp spreads this config's `features` into that table, so the
/// same features stay off for every session thread. Codex itself never reads it.
pub const CODEX_CONFIG: &str = "CODEX_CONFIG";
/// The Codex features every Cortex-launched App Server and session thread runs
/// without: no subagents, plugins, hooks, apps or native shell/image tools.
pub const CODEX_DISABLED_FEATURES: &[&str] = &[
    "multi_agent",
    "multi_agent_v2",
    "plugins",
    "hooks",
    "apps",
    "shell_tool",
    "view_image",
];
/// The `CODEX_CONFIG` value of a launcher-backed Codex launch: exactly
/// [`CODEX_DISABLED_FEATURES`], each `false`.
pub fn codex_thread_config() -> String {
    let features: serde_json::Map<String, serde_json::Value> = CODEX_DISABLED_FEATURES
        .iter()
        .map(|name| ((*name).to_owned(), serde_json::Value::Bool(false)))
        .collect();
    serde_json::json!({ "features": features }).to_string()
}
impl CodexLauncher {
    pub fn new(prepare: impl Fn() -> Result<PathBuf> + Send + Sync + 'static) -> Self {
        Self {
            prepare: Arc::new(prepare),
            servers: vec![],
        }
    }
    /// The MCP relays this session's App Server starts itself.
    pub fn with_mcp_servers(mut self, servers: Vec<RelayEntry>) -> Self {
        self.servers = servers;
        self
    }
    /// Python `pinned_engine_vars`: an absolute path to an existing file.
    pub fn prepare(&self) -> Result<PathBuf> {
        let path = (self.prepare)()?;
        if !path.is_absolute()
            || path.to_str().is_none()
            || path.as_os_str().as_encoded_bytes().contains(&0)
            || !path.is_file()
        {
            return Err(Error::Invalid);
        }
        Ok(path)
    }
}
/// Constructed by service connection policy after install/account/isolation validation.
/// No arbitrary environment overrides or discovered executable fallback.
#[derive(Clone)]
pub struct LaunchSpec {
    pub engine: EngineKind,
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub native_home: PathBuf,
    pub credential_directory: PathBuf,
    pub base_environment: BTreeMap<String, String>,
    /// Codex only: the native Codex launcher prepared at launch.
    pub codex_launcher: Option<CodexLauncher>,
    /// The filesystem view the engine and all its descendants start in. Service
    /// routes always set it; a view that cannot be prepared refuses the launch.
    pub confinement: Option<ConfinementRequest>,
}
/// The service policy that prepares a launch's view, and the session's private
/// broker directories that view must show (read-only, at
/// [`crate::broker::VIEW_DIRECTORY`]).
#[derive(Clone)]
pub struct ConfinementRequest {
    pub confiner: Arc<dyn crate::filesystem_view::Confiner>,
    pub exposed: Vec<PathBuf>,
}
impl ConfinementRequest {
    pub fn new(confiner: Arc<dyn crate::filesystem_view::Confiner>) -> Self {
        Self {
            confiner,
            exposed: Vec::new(),
        }
    }
}
impl LaunchSpec {
    /// Show these private broker directories to a confined launch. No effect
    /// on an unconfined launch, which reaches them through their host paths.
    pub fn expose(&mut self, directories: impl IntoIterator<Item = PathBuf>) {
        if let Some(request) = &mut self.confinement {
            request.exposed.extend(directories);
        }
    }
    pub fn environment(&self) -> Result<BTreeMap<String, String>> {
        for p in [
            &self.executable,
            &self.cwd,
            &self.native_home,
            &self.credential_directory,
        ] {
            if !p.is_absolute()
                || p.to_str().is_none()
                || p.as_os_str().as_encoded_bytes().contains(&0)
            {
                return Err(Error::Invalid);
            }
        }
        if !self.executable.is_file()
            || !self.cwd.is_dir()
            || !self.native_home.is_dir()
            || !self.credential_directory.is_dir()
        {
            return Err(Error::Invalid);
        }
        if self.args.iter().any(|v| v.contains('\0'))
            || self.base_environment.values().any(|v| v.contains('\0'))
        {
            return Err(Error::Invalid);
        }
        if self.codex_launcher.is_some() && self.engine != EngineKind::Codex {
            return Err(Error::Invalid);
        }
        let mut env = BTreeMap::new();
        for key in [
            "PATH", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "TMPDIR", "USER", "LOGNAME",
        ] {
            if let Some(v) = self.base_environment.get(key) {
                let v = if key == "PATH" {
                    v.split(':')
                        .filter(|x| {
                            Path::new(x).is_absolute()
                                && !Path::new(x)
                                    .components()
                                    .any(|c| matches!(c, std::path::Component::ParentDir))
                        })
                        .collect::<Vec<_>>()
                        .join(":")
                } else {
                    v.clone()
                };
                env.insert(key.into(), v);
            }
        }
        env.insert(
            "HOME".into(),
            self.native_home.to_string_lossy().into_owned(),
        );
        let (key, path) = match self.engine {
            EngineKind::Codex => ("CODEX_HOME", self.credential_directory.as_path()),
            EngineKind::Claude => ("CLAUDE_CONFIG_DIR", self.credential_directory.as_path()),
            EngineKind::OpenCode => {
                if self
                    .credential_directory
                    .file_name()
                    .and_then(|s| s.to_str())
                    != Some("opencode")
                {
                    return Err(Error::Invalid);
                }
                (
                    "XDG_DATA_HOME",
                    self.credential_directory.parent().ok_or(Error::Invalid)?,
                )
            }
        };
        env.insert(key.into(), path.to_string_lossy().into_owned());
        Ok(env)
    }
    /// The environment a launch actually receives: `environment` plus the
    /// prepared Codex launcher, if any.
    pub fn launch_environment(&self) -> Result<BTreeMap<String, String>> {
        let mut env = self.environment()?;
        if let Some(launcher) = &self.codex_launcher {
            let path = launcher.prepare()?;
            env.insert("CODEX_PATH".into(), path.to_string_lossy().into_owned());
            env.insert("NO_BROWSER".into(), "1".into());
            env.insert(CODEX_CONFIG.into(), codex_thread_config());
            if !launcher.servers.is_empty() {
                let servers =
                    serde_json::to_string(&launcher.servers).map_err(|_| Error::Invalid)?;
                if servers.contains('\0') {
                    return Err(Error::Invalid);
                }
                env.insert(CODEX_MCP_SERVERS.into(), servers);
            }
        }
        Ok(env)
    }
}
