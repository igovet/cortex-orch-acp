//! The prepared confinement ([`Confinement`]) a confined engine or helper starts
//! in: an OS policy the child enters between fork and exec (for example a
//! Landlock ruleset), the environment values it requires and any directories it
//! reaches at another path. No mount namespace is created. Callers choose and
//! validate the policy; this module never interprets argv or PATH.
use std::{
    collections::BTreeMap,
    ffi::OsStr,
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::external::Containment;

/// One prepared confinement for one engine or helper process. There is no
/// unconfined form: a caller that cannot prepare one does not start the process.
#[derive(Clone)]
pub struct Confinement {
    containment: Arc<dyn Containment>,
    environment: BTreeMap<String, String>,
    mapped: Vec<(PathBuf, PathBuf)>,
}
impl Confinement {
    pub fn new(
        containment: Arc<dyn Containment>,
        environment: BTreeMap<String, String>,
        mapped: Vec<(PathBuf, PathBuf)>,
    ) -> Self {
        Self {
            containment,
            environment,
            mapped,
        }
    }
    /// Values the confined process must receive instead of the host's.
    pub fn environment(&self) -> &BTreeMap<String, String> {
        &self.environment
    }
    /// The path at which the confined process reaches a host `path` inside one of
    /// the mapped directories; any other path is unchanged.
    pub fn view_path(&self, path: &Path) -> PathBuf {
        for (host, view) in &self.mapped {
            if let Ok(rest) = path.strip_prefix(host) {
                return view.join(rest);
            }
        }
        path.to_owned()
    }
    /// `executable args...` at its real path, entering the policy just before exec.
    pub fn command<I, S>(&self, executable: &Path, args: I) -> std::process::Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut command = std::process::Command::new(executable);
        command.args(args);
        let containment = self.containment.clone();
        unsafe {
            command.pre_exec(move || containment.enter());
        }
        command
    }
}

/// Service policy that prepares the confinement of one engine launch when it
/// actually starts, after its launch environment (including a prepared Codex
/// launcher) exists. `inputs` are additional host files the engine must read
/// (its model instructions, the prepared launcher).
pub trait Confiner: Send + Sync + 'static {
    fn confine(
        &self,
        spec: &crate::environment::LaunchSpec,
        environment: &BTreeMap<String, String>,
        inputs: &[PathBuf],
    ) -> std::io::Result<Confinement>;
}
