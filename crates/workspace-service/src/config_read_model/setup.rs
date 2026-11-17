//! Passive adapter/setup display discovery. Never launch, install, log in or refresh.
use super::{
    adapter_manifest, adapter_version, matches, ConfigurationReader, SettingsRead, ENGINES,
};
use crate::native_routes::{read_bytes, RouteEnvironment};
use cortex_core::{CoreError, CoreResult};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
};

/// Trusted host inputs, not socket/model fields. bundle is only this runtime's
/// channel-local engines root; None means a checkout without bundled engines.
pub struct SetupEnvironment {
    pub data: PathBuf,
    pub path: Vec<PathBuf>,
    pub node_override: Option<PathBuf>,
    pub bundle: Option<PathBuf>,
}
fn refuse<T>() -> CoreResult<T> {
    Err(CoreError::Refused("setup_discovery_unreadable"))
}
fn absolute(p: &Path) -> CoreResult<()> {
    if !p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
        return refuse();
    }
    Ok(())
}
fn parents(p: &Path) -> CoreResult<()> {
    absolute(p)?;
    for p in p.ancestors() {
        match fs::symlink_metadata(p) {
            Ok(m) if m.is_dir() && !m.is_symlink() => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            _ => return refuse(),
        }
    }
    Ok(())
}
fn regular(p: &Path, executable: bool) -> bool {
    fs::metadata(p).is_ok_and(|m| m.is_file() && (!executable || m.mode() & 0o111 != 0))
}
fn directories(root: &Path) -> CoreResult<Vec<PathBuf>> {
    parents(root)?;
    let entries = match fs::read_dir(root) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(_) => return refuse(),
    };
    let mut result = vec![];
    for (i, e) in entries.enumerate() {
        if i >= 512 {
            return refuse();
        }
        let p = e?.path();
        if p.file_name()
            .is_some_and(|s| s.as_encoded_bytes().starts_with(b"."))
        {
            continue;
        }
        let m = fs::symlink_metadata(&p)?;
        if m.is_symlink() {
            return refuse();
        }
        if m.is_dir() {
            result.push(p);
        }
    }
    result.sort();
    Ok(result)
}
fn version_key(s: &str) -> Vec<(usize, String)> {
    if !matches(
        r"^[0-9]+\.[0-9]+\.[0-9]+(?:\+sdk\.[0-9]+\.[0-9]+\.[0-9]+)?$",
        s,
    ) {
        return vec![];
    }
    s.replace("+sdk.", ".")
        .split('.')
        .map(|s| {
            let s = s.trim_start_matches('0');
            (s.len(), s.to_owned())
        })
        .collect()
}
fn qualified(engine: &str, version: &str) -> bool {
    adapter_version(engine, version)["qualified"] == true
}
struct Adapter {
    version: String,
    manifest: PathBuf,
    entry: PathBuf,
    manifest_digest: String,
}
fn select(entries: Vec<PathBuf>, engine: &str) -> CoreResult<Option<Adapter>> {
    let package = format!("@agentclientprotocol/{engine}");
    let mut candidates = vec![];
    for root in entries {
        let manifest = root
            .join("node_modules")
            .join(&package)
            .join("package.json");
        parents(manifest.parent().unwrap())?;
        let Some(bytes) = read_bytes(&manifest, false)? else {
            continue;
        };
        // A malformed/ambiguous manifest is not an installed adapter.
        let Some((version, bin)) = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .as_ref()
            .and_then(adapter_manifest)
        else {
            continue;
        };
        let package = manifest.parent().unwrap();
        let Ok(entry) = package.join(bin).canonicalize() else {
            continue;
        };
        if !entry.starts_with(package) || !regular(&entry, false) {
            continue;
        }
        candidates.push((
            (
                qualified(engine, &version),
                version_key(&version),
                version_key(root.file_name().unwrap_or_default().to_str().unwrap_or("")),
            ),
            Adapter {
                version,
                manifest,
                entry,
                manifest_digest: format!("{:x}", Sha256::digest(&bytes)),
            },
        ));
    }
    // Python max keeps the first candidate on equal keys.
    let mut best = None;
    for candidate in candidates {
        if best.as_ref().is_none_or(|(key, _)| candidate.0 > *key) {
            best = Some(candidate);
        }
    }
    Ok(best.map(|(_, a)| a))
}
// Readability/stability observation, not an approved pin or instruction qualification.
fn stable_digest(p: &Path) -> CoreResult<String> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(p)?;
    let before = f.metadata()?;
    const MAX: u64 = 512 * 1024 * 1024;
    if !before.is_file() || before.len() > MAX {
        return refuse();
    }
    let identity = |m: &fs::Metadata| {
        (
            m.dev(),
            m.ino(),
            m.len(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    let mut hash = Sha256::new();
    let mut n = 0;
    let mut handle = &f;
    let mut buf = [0u8; 65536];
    loop {
        let len = handle.read(&mut buf)?;
        if len == 0 {
            break;
        }
        n += len as u64;
        if n > MAX {
            return refuse();
        }
        hash.update(&buf[..len]);
    }
    let observed_digest = format!("{:x}", hash.finalize());
    if identity(&before) != identity(&f.metadata()?) || n != before.len() {
        return refuse();
    }
    Ok(observed_digest)
}
fn trusted(p: &Path, home: &Path) -> bool {
    p.is_absolute()
        && p.canonicalize().is_ok_and(|real| real == p)
        && ![".local/share/app.codeg", ".config/app.codeg"]
            .iter()
            .any(|r| p.starts_with(home.join(r)))
}
fn dependency(package: &Path, name: &str, probe: &str) -> Option<PathBuf> {
    let mut roots = vec![package.join("node_modules")];
    roots.extend(
        package
            .ancestors()
            .filter(|p| p.file_name().is_some_and(|s| s == "node_modules"))
            .map(Path::to_path_buf),
    );
    roots
        .into_iter()
        .map(|r| r.join(name))
        .find(|p| regular(&p.join(probe), false))
}
fn verify_bundle(a: &Adapter, engine: &str, node: &Path, home: &Path) -> CoreResult<bool> {
    if ![node, a.entry.as_path(), a.manifest.as_path()]
        .iter()
        .all(|p| trusted(p, home))
        || !regular(node, true)
    {
        return Ok(false);
    }
    stable_digest(&a.entry)?;
    if stable_digest(&a.manifest)? != a.manifest_digest {
        return refuse();
    }
    if engine == "codex-acp" {
        let package = a.manifest.parent().unwrap();
        let Some(dep) = dependency(package, "@openai/codex", "package.json") else {
            return Ok(false);
        };
        let Some(bytes) = read_bytes(&dep.join("package.json"), false)? else {
            return Ok(false);
        };
        let doc: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if !doc["version"].as_str().is_some_and(|s| !s.is_empty()) {
            return Ok(false);
        }
        let (name, triple) = match std::env::consts::ARCH {
            "x86_64" => ("@openai/codex-linux-x64", "x86_64-unknown-linux-musl"),
            "aarch64" => ("@openai/codex-linux-arm64", "aarch64-unknown-linux-musl"),
            _ => return refuse(),
        };
        let probe = format!("vendor/{triple}/bin/codex");
        let Some(dep) = dependency(package, name, &probe) else {
            return Ok(false);
        };
        let binary = dep.join(probe);
        if !trusted(&binary, home) || !regular(&binary, true) {
            return Ok(false);
        }
        stable_digest(&binary)?;
    }
    Ok(true)
}
fn automatic(config: &Path, engine: &str) -> CoreResult<(Value, Value)> {
    let Some(bytes) = read_bytes(&config.join(format!("first-run-{engine}.json")), true)? else {
        return Ok((Value::Null, Value::Null));
    };
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let Some(phase) = body["phase"].as_str() else {
        return Ok((json!("setup_failed"), Value::Null));
    };
    let phases = [
        "downloading",
        "connecting",
        "refreshing_models",
        "ready",
        "download_failed",
        "login_required",
        "opted_out",
        "catalog_failed",
        "setup_failed",
        "downloaded",
    ];
    if !phases.contains(&phase) {
        return Ok((json!("setup_failed"), Value::Null));
    }
    if ["downloading", "connecting", "refreshing_models"].contains(&phase) {
        // Noncreating, nonblocking probe of the existing cooperative setup lock.
        let lock = config.join("automatic-setup.lock");
        let file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open(&lock)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok((json!("interrupted"), Value::Null))
            }
            Err(_) => return refuse(),
        };
        let m = file.metadata()?;
        if !m.is_file()
            || m.uid() != unsafe { libc::getuid() }
            || m.mode() & 0o077 != 0
            || m.nlink() != 1
        {
            return refuse();
        }
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            return Ok((json!("interrupted"), Value::Null));
        }
        if std::io::Error::last_os_error().kind() != std::io::ErrorKind::WouldBlock {
            return refuse();
        }
    }
    let reason = if phase == "catalog_failed"
        && body["reason"]
            .as_str()
            .is_some_and(|s| matches(r"^[a-z][a-z0-9_]{0,63}(?::[A-Za-z][A-Za-z0-9_]{0,63})?$", s))
    {
        body["reason"].clone()
    } else {
        Value::Null
    };
    Ok((json!(phase), reason))
}
impl ConfigurationReader {
    /// The discovered adapter of an npm engine and the node launching it
    /// (Python `discover_engine`: the verified channel bundle, else owned installs).
    fn adapter(
        &self,
        env: &SetupEnvironment,
        engine: &str,
        node: Option<&PathBuf>,
    ) -> CoreResult<Option<(Adapter, PathBuf)>> {
        if let Some(bundle) = &env.bundle {
            let node = bundle.join("node/bin/node");
            if regular(&node, true) {
                if let Some(a) = select(vec![bundle.to_owned()], engine)? {
                    if qualified(engine, &a.version)
                        && verify_bundle(&a, engine, &node, &self.environment.home)?
                    {
                        return Ok(Some((a, node)));
                    }
                }
            }
        }
        let Some(node) = node else {
            return Ok(None);
        };
        Ok(select(
            directories(&env.data.join("cortex/engines").join(engine))?,
            engine,
        )?
        .map(|a| (a, node.clone())))
    }
    fn setup_node(&self, env: &SetupEnvironment) -> CoreResult<Option<PathBuf>> {
        parents(&env.data)?;
        RouteEnvironment {
            home: self.environment.home.clone(),
            config: env.data.clone(),
            state: env.data.clone(),
            routing_override: None,
        }
        .validate()?;
        for p in &env.path {
            absolute(p)?;
        }
        if let Some(p) = &env.node_override {
            absolute(p)?;
        }
        if let Some(p) = &env.bundle {
            parents(p)?;
        }
        Ok(env
            .node_override
            .clone()
            .or_else(|| {
                env.path
                    .iter()
                    .map(|p| p.join("node"))
                    .find(|p| regular(p, true))
            })
            .and_then(|p| p.canonicalize().ok())
            .filter(|p| regular(p, true)))
    }
    /// `(version, [node, entry])` of an npm engine's discovered adapter, the
    /// launch command Python's `discover_engine` reports. Nothing is executed.
    pub(crate) fn setup_launch(
        &self,
        env: &SetupEnvironment,
        engine: &str,
    ) -> CoreResult<Option<(String, Vec<String>)>> {
        let node = self.setup_node(env)?;
        Ok(self.adapter(env, engine, node.as_ref())?.map(|(a, node)| {
            (
                a.version,
                vec![
                    node.to_string_lossy().into_owned(),
                    a.entry.to_string_lossy().into_owned(),
                ],
            )
        }))
    }
    /// Complete prompt-free display discovery. No command/credential bytes returned.
    pub fn setup_discovery(&self, env: &SetupEnvironment) -> CoreResult<SettingsRead> {
        parents(&env.data)?;
        // Apply the existing project-storage fence to the additional XDG data root.
        RouteEnvironment {
            home: self.environment.home.clone(),
            config: env.data.clone(),
            state: env.data.clone(),
            routing_override: None,
        }
        .validate()?;
        for p in &env.path {
            absolute(p)?;
        }
        if let Some(p) = &env.node_override {
            absolute(p)?;
        }
        if let Some(p) = &env.bundle {
            parents(p)?;
        }
        let (registry, management) = self.registry_snapshot()?;
        let node = env
            .node_override
            .clone()
            .or_else(|| {
                env.path
                    .iter()
                    .map(|p| p.join("node"))
                    .find(|p| regular(p, true))
            })
            .and_then(|p| p.canonicalize().ok())
            .filter(|p| regular(p, true));
        let mut engines = vec![];
        let mut diagnostics = vec![];
        for engine in ENGINES {
            let optional = engine == "opencode";
            let found = if optional {
                None
            } else {
                self.adapter(env, engine, node.as_ref())?.map(|(a, _)| a)
            };
            let native = self.environment.home.join(match engine {
                "codex-acp" => ".codex",
                "claude-agent-acp" => ".claude",
                _ => ".local/share/opencode",
            });
            let marker = super::native_login_marker(engine, &native);
            if marker == "invalid" {
                return refuse();
            }
            let login = marker == "configured";
            let connected = management["connections"]
                .as_array()
                .unwrap()
                .iter()
                .find(|r| r["engine"] == engine)
                .map(|r| r["id"].clone());
            let status = if connected.is_some() {
                "connected"
            } else if optional {
                "discovery_unavailable"
            } else if found.is_none() {
                "not_downloaded"
            } else if login {
                "ready_to_connect"
            } else {
                "downloaded_no_login"
            };
            if optional {
                diagnostics.push(json!({"engine":engine,"source":"setup_discovery","code":"version_probe_disabled"}));
            }
            let (automatic, reason) = if optional {
                (Value::Null, Value::Null)
            } else {
                automatic(&self.environment.config.join("cortex"), engine)?
            };
            engines.push(json!({"engine":engine,"status":status,"version":found.as_ref().map(|a|a.version.clone()),
                "native_login_detected":login,"already_connected":connected,"qualified":found.as_ref().is_some_and(|a|qualified(engine,&a.version)),
                "install_supported":!optional,"automatic_setup":automatic,"automatic_setup_reason":reason,
                "auto_connect_disabled":registry["auto_connect_disabled"].as_array().is_some_and(|a|a.iter().any(|v|v==engine))}));
        }
        Ok(SettingsRead {
            view: json!({"surface":"connections","revision":management["revision"],"engines":engines}),
            diagnostics,
        })
    }

    /// Replace setup, Workspace, catalog and management seams before projection.
    pub fn settings_with_setup_readers(
        &self,
        observations: &Value,
        now: f64,
        project: Option<(&Path, &Path)>,
        workspace_state: &Path,
        setup: &SetupEnvironment,
    ) -> CoreResult<SettingsRead> {
        if !observations.is_object() {
            return Err(CoreError::Refused("invalid_settings_observation"));
        }
        let discovery = self.setup_discovery(setup)?;
        let mut o = observations.clone();
        o["discovery"] = discovery.view;
        let mut result = self.settings_with_workspace_readers(&o, now, project, workspace_state)?;
        result.diagnostics.extend(discovery.diagnostics);
        Ok(result)
    }
}
