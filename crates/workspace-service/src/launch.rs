use crate::local::failure;
use std::{io, path::PathBuf, sync::Arc};
use tokio::process::Command;

/// Supplied only by the release/native host, never from local client JSON.
/// This is a trusted same-user process boundary, not hostile-code isolation.
#[derive(Clone)]
pub struct LaunchSpec {
    runtime_root: Option<PathBuf>,
    pub(crate) extracted_dev: bool,
    pub(crate) dev_config: Option<PathBuf>,
    /// The host's own state root, when it is not `<state_dir>/state` (the settings API).
    pub(crate) dev_state: Option<PathBuf>,
    pub(crate) native_routes: Option<Arc<dyn crate::native_runtime::NativeRoutes>>,
}
/// Build-parallelism variables a task's builds inherit, set to half of the
/// machine's CPUs (at least two) unless the owner already set them. Task builds
/// (Rust, CMake) otherwise start one job per CPU, and a linker per job: on
/// 2026-11-21 ten parallel links of about 1 GiB each saturated an owner's disk and
/// memory and froze the desktop. The owner's own value always wins.
pub const BUILD_PARALLELISM: [&str; 2] = ["CARGO_BUILD_JOBS", "CMAKE_BUILD_PARALLEL_LEVEL"];
pub fn default_build_parallelism() {
    let cpus = std::thread::available_parallelism().map_or(2, |n| n.get());
    let jobs = (cpus / 2).max(2).to_string();
    for key in BUILD_PARALLELISM {
        if std::env::var_os(key).is_none() {
            // Called first in `main`, before any thread is started.
            std::env::set_var(key, &jobs);
        }
    }
}
/// The environment every process this service starts runs with: never loader
/// injection, engine flags or credentials, and in dev always the channel's own directories.
pub(crate) fn runtime_environment(command: &mut Command, config: &crate::Config) -> io::Result<()> {
    command.env_clear();
    for key in [
        "HOME",
        "PATH",
        "LANG",
        "LC_ALL",
        "XDG_CONFIG_HOME",
        "XDG_STATE_HOME",
        "XDG_CACHE_HOME",
        "XDG_RUNTIME_DIR",
        BUILD_PARALLELISM[0],
        BUILD_PARALLELISM[1],
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    if workspace_protocol::channel::current().is_dev() {
        // Config/state live below the channel's private registry root. HOME is unchanged:
        // native engine accounts remain in the user's own native directories.
        for (key, leaf) in [
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_DATA_HOME", "data"),
        ] {
            let directory = if key == "XDG_CONFIG_HOME" {
                config
                    .launch
                    .dev_config
                    .clone()
                    .unwrap_or_else(|| config.state_dir.join(leaf))
            } else if key == "XDG_STATE_HOME" {
                crate::native_routes::RouteEnvironment::workspace_dev_state(
                    &config.state_dir,
                    config.launch.dev_state.as_deref(),
                )
            } else {
                config.state_dir.join(leaf)
            };
            crate::local::private_tree(&directory)?;
            command.env(key, directory);
        }
        command.env("XDG_RUNTIME_DIR", &config.runtime_dir);
    }
    command.env(
        "CORTEX_WORKSPACE_CHANNEL",
        if workspace_protocol::channel::current().is_dev() {
            "dev"
        } else {
            "release"
        },
    );
    command.env("CORTEX_WORKSPACE_STATE_DIR", &config.state_dir);
    Ok(())
}
impl LaunchSpec {
    /// Trusted native asset root.
    pub fn native(runtime_root: PathBuf) -> io::Result<Self> {
        if !runtime_root.is_absolute() || !runtime_root.is_dir() {
            return Err(failure("explicit_runtime_root_required"));
        }
        Ok(Self {
            runtime_root: Some(runtime_root.canonicalize()?),
            extracted_dev: false,
            dev_config: None,
            dev_state: None,
            native_routes: None,
        })
    }
    /// Keep inherited routing outside a validated, privately extracted Dev run.
    pub fn with_dev_extraction(mut self, _: &crate::dev_extraction::DevExtraction) -> Self {
        self.extracted_dev = true;
        self
    }
    /// A host with no bundled asset root: registry, machine and native-runtime operations only.
    pub fn bare() -> Self {
        Self {
            runtime_root: None,
            extracted_dev: false,
            dev_config: None,
            dev_state: None,
            native_routes: None,
        }
    }
    pub(crate) fn runtime_root(&self) -> Option<&std::path::Path> {
        self.runtime_root.as_deref()
    }
    pub(crate) fn bundled_engines(&self) -> Option<PathBuf> {
        let runtime = self.runtime_root()?;
        let package = runtime.parent()?;
        (runtime.file_name()? == "runtime"
            && package.file_name()? == workspace_protocol::channel::current().package.as_str())
        .then(|| package.join("engines"))
        .filter(|p| p.is_dir())
    }
}
