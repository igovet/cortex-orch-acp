//! Native Settings "Download engine" for Codex and Claude. The oracle is the
//! retained `management.install_engine` with `engine_update.install_pinned_engine`
//! (background=False) and `discover_connections`, within the machine request deadline.
//!
//! The pinned-install steps duplicate `crates/cortex-acp/src/automatic.rs`
//! (`install_pin`): that crate depends on this one, so the code cannot move there
//! without a dependency cycle. Only Cortex's manifest pin is ever installed.
use super::opencode_engine;
use super::*;
use crate::config_read_model::SetupEnvironment;
use crate::config_read_model::{adapter_version, native_login_marker, ConfigurationReader};
use crate::native_routes::RouteEnvironment;
use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Read,
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::PathBuf,
    process::Stdio,
    time::{Duration, Instant},
};

#[cfg(test)]
#[path = "config_engine_install_tests.rs"]
mod tests;

const CLAUDE_SDK_PACKAGE: &str = "@anthropic-ai/claude-agent-sdk";
const CLAUDE_SDK_OVERRIDES: [(&str, &str); 1] = [("0.87.0", "0.3.293")];
/// The current version and one to fall back to.
const KEEP: usize = 2;
// Leave time to kill/reap npm and return its refusal before the client deadline.
// Queueing and pre-install work consume the same absolute request budget.
const CLEANUP_RESERVE: Duration = Duration::from_secs(1);
pub(super) const TIMEOUT: Duration = super::super::HELPER_DEADLINE.saturating_sub(CLEANUP_RESERVE);

pub(super) fn npm_deadline(request_deadline: Instant) -> Instant {
    (Instant::now() + TIMEOUT).min(request_deadline - CLEANUP_RESERVE)
}

/// Trusted host inputs: the retained helper's environment, never socket fields.
pub(super) struct Host {
    pub route: RouteEnvironment,
    pub data: PathBuf,
    pub path: Option<OsString>,
    pub bundle: Option<PathBuf>,
    pub manifest: PathBuf,
}

fn failed() -> Refused {
    refuse(
        "engine_install_failed",
        "Pinned engine installation failed; retry after checking npm/network.",
    )
}
fn unavailable() -> Refused {
    refuse(
        "engine_install_failed",
        "Pinned installation is unavailable, busy or failed verification.",
    )
}
fn package(engine: &str) -> &'static str {
    if engine == "codex-acp" {
        "@agentclientprotocol/codex-acp"
    } else {
        "@agentclientprotocol/claude-agent-acp"
    }
}
fn qualified(engine: &str, version: &str) -> bool {
    adapter_version(engine, version)["qualified"] == true
}
/// Python `discovery.is_pinned`.
fn pinned(engine: &str, version: &str) -> bool {
    qualified(
        engine,
        if engine == "claude-agent-acp" {
            version.split("+sdk.").next().unwrap()
        } else {
            version
        },
    )
}
/// Python `discovery.version_key`; numeric parts compare as (length, digits).
fn version_key(text: &str) -> Vec<(usize, String)> {
    if !crate::config_read_model::matches(
        r"^[0-9]+\.[0-9]+\.[0-9]+(?:\+sdk\.[0-9]+\.[0-9]+\.[0-9]+)?$",
        text,
    ) {
        return vec![];
    }
    text.replace("+sdk.", ".")
        .split('.')
        .map(|part| {
            let part = part.trim_start_matches('0');
            (part.len(), part.to_owned())
        })
        .collect()
}
fn executable(path: &Path) -> bool {
    std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .is_ok_and(|p| unsafe { libc::access(p.as_ptr(), libc::X_OK) } == 0)
}
/// Python `shutil.which(name, path=PATH)` as run by the helper (cwd `/`);
/// an unset PATH is `os.defpath`.
fn which(name: &str, path: Option<&OsString>) -> Option<PathBuf> {
    let default = OsString::from("/bin:/usr/bin");
    std::env::split_paths(path.unwrap_or(&default))
        .map(|dir| dir.join(name))
        .find(|candidate| {
            let absolute = Path::new("/").join(candidate);
            absolute.exists() && executable(&absolute) && !absolute.is_dir()
        })
}
/// Python `Path.resolve()` from cwd `/`; a broken path keeps its lexical form.
fn resolve(path: &Path) -> PathBuf {
    let absolute = Path::new("/").join(path);
    absolute.canonicalize().unwrap_or(absolute)
}
/// Python `_declared_bin`: `(version, bin)` declared by the manifest itself.
fn declared_bin(manifest: &Path) -> Option<(String, String)> {
    let text = fs::read_to_string(manifest).ok()?;
    let data: Value = serde_json::from_str(&text).ok()?;
    let version = data["version"].as_str().filter(|v| !v.is_empty())?;
    let target = match &data["bin"] {
        Value::String(s) => s.as_str(),
        Value::Object(m) if m.len() == 1 => m.values().next()?.as_str()?,
        _ => return None,
    };
    (!target.is_empty()).then(|| (version.to_owned(), target.to_owned()))
}
/// Python `_discover_in([entry])`: the declared version of a real, contained bin.
fn discover_one(entry: &Path, package: &str) -> Option<String> {
    let manifest = entry
        .join("node_modules")
        .join(package)
        .join("package.json");
    if !manifest.is_file() {
        return None;
    }
    let (version, target) = declared_bin(&manifest)?;
    let directory = manifest.parent()?.canonicalize().ok()?;
    let bin = manifest.parent()?.join(target).canonicalize().ok()?;
    (bin.starts_with(&directory) && bin.is_file()).then_some(version)
}
fn remove_tree(path: &Path) {
    // shutil.rmtree(ignore_errors=True): a link or file is never removed.
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
    {
        let _ = fs::remove_dir_all(path);
    }
}
/// Python `_subdirs`: non-dot directories (links followed), sorted by name.
fn subdirectories(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return vec![];
    };
    let mut out: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && !p
                    .file_name()
                    .unwrap_or_default()
                    .as_encoded_bytes()
                    .starts_with(b".")
        })
        .collect();
    out.sort();
    out
}
fn name(path: &Path) -> &str {
    path.file_name().and_then(|n| n.to_str()).unwrap_or("")
}

impl Host {
    pub(super) fn from_config(config: &crate::Config) -> Result<Self> {
        let route = super::super::config_read::environment(config)?;
        // The retained child environment: release has no XDG_DATA_HOME or
        // CORTEX_NODE_BINARY; dev uses only its channel-local data root.
        let data = if workspace_protocol::channel::current().is_dev() {
            config.state_dir.join("data")
        } else {
            route.home.join(".local/share")
        };
        let runtime = config
            .launch
            .runtime_root()
            .ok_or_else(|| refuse("config_action_unavailable", "No runtime manifest."))?;
        Ok(Self {
            data,
            path: std::env::var_os("PATH"),
            bundle: config.launch.bundled_engines(),
            manifest: runtime.join("pinned-dependencies.json"),
            route,
        })
    }
    /// A registry or login marker the native discovery cannot project.
    pub(super) fn unprojected(&self) -> Option<&'static str> {
        let home = &self.route.home;
        for (engine, dir) in [
            ("codex-acp", ".codex"),
            ("claude-agent-acp", ".claude"),
            ("opencode", ".local/share/opencode"),
        ] {
            if native_login_marker(engine, &home.join(dir)) == "invalid" {
                return Some("native_login_marker_unprojected");
            }
        }
        let folder = self.route.config.join("cortex");
        let parsed = reader::read_plain(&folder.join("connections.json"), true)
            .ok()
            .and_then(|bytes| registry_json::RegistryJson::parse(bytes.as_deref()).ok())
            .and_then(|parsed| registry::normalize(&parsed.value).ok());
        if parsed.is_none() {
            return Some("connection_registry_unprojected");
        }
        None
    }
    /// Python `discover_opencode` candidates in the helper environment (no
    /// `binary` is forwarded and CORTEX_OPENCODE_BINARY is not inherited).
    pub(super) fn opencode_candidate(&self) -> bool {
        self.route
            .home
            .join(".opencode/bin/opencode")
            .canonicalize()
            .is_ok()
            || which("opencode", self.path.as_ref()).is_some()
    }
    fn setup(&self, path: bool) -> SetupEnvironment {
        SetupEnvironment {
            data: self.data.clone(),
            path: if path {
                self.path
                    .as_ref()
                    .map(|p| {
                        std::env::split_paths(p)
                            .filter(|p| p.is_absolute())
                            .collect()
                    })
                    .unwrap_or_default()
            } else {
                vec![]
            },
            node_override: None,
            bundle: self.bundle.clone(),
        }
    }
    /// Python `discover_connections`. OpenCode has no candidate (admission), so
    /// its retained status is `not_downloaded` unless connected.
    /// `(version, launch command)` of an npm engine's discovered adapter.
    pub(super) fn launch(&self, engine: &str) -> Result<Option<(String, Vec<String>)>> {
        ConfigurationReader::new(self.route.clone())
            .and_then(|r| r.setup_launch(&self.setup(true), engine))
            .map_err(|e| {
                refuse(
                    e.code(),
                    "Native engine discovery refused; re-read Settings.",
                )
            })
    }
    pub(super) fn route(&self) -> &crate::native_routes::RouteEnvironment {
        &self.route
    }
    pub(super) fn discovery(&self) -> Result<Value> {
        let mut view = ConfigurationReader::new(self.route.clone())
            .and_then(|r| r.setup_discovery(&self.setup(true)))
            .map_err(|e| {
                refuse(
                    e.code(),
                    "Native engine discovery refused; re-read Settings.",
                )
            })?
            .view;
        // OpenCode's version is read by the sandboxed probe (`discover_opencode`).
        for engine in view["engines"].as_array_mut().into_iter().flatten() {
            if engine["engine"] == "opencode" && engine["status"] == "discovery_unavailable" {
                match opencode_engine::discover(&self.route.home, self.path.as_ref(), None) {
                    None => engine["status"] = json!("not_downloaded"),
                    Some(found) => {
                        engine["status"] = json!(if engine["native_login_detected"] == true {
                            "ready_to_connect"
                        } else {
                            "downloaded_no_login"
                        });
                        engine["qualified"] = json!(opencode_engine::qualified(&found.version));
                        engine["version"] = json!(found.version);
                    }
                }
            }
        }
        Ok(view)
    }
    /// Python `discover_bundled`: only the channel bundle, never PATH or owned trees.
    fn bundled(&self, engine: &str) -> Option<String> {
        self.bundle.as_ref()?;
        let view = ConfigurationReader::new(self.route.clone())
            .and_then(|r| r.setup_discovery(&self.setup(false)))
            .ok()?
            .view;
        view["engines"]
            .as_array()?
            .iter()
            .find(|e| e["engine"] == engine)?["version"]
            .as_str()
            .map(str::to_owned)
    }
    /// Python `engine_update.pinned_version`.
    fn pin(&self, engine: &str) -> Result<String> {
        let text = fs::read_to_string(&self.manifest).map_err(|_| failed())?;
        let manifest: Value = serde_json::from_str(&text).map_err(|_| failed())?;
        // Python `data.get("schema_version") != 1` (True and 1.0 equal 1).
        let schema = &manifest["schema_version"];
        if !manifest.is_object() || !(schema.as_f64() == Some(1.0) || *schema == true) {
            return Err(failed());
        }
        // `.get(key, {})` on a present non-object (null included) raised an
        // uncaught AttributeError: no JSON from the helper.
        let engines = manifest.get("engines");
        let entry = engines.and_then(|e| e.get(engine));
        if engines.is_some_and(|e| !e.is_object()) || entry.is_some_and(|e| !e.is_object()) {
            return Err(refuse("machine_tool_failed", UNCAUGHT_ATTRIBUTE_ERROR));
        }
        manifest["engines"][engine]["pinned_package_version"]
            .as_str()
            .filter(|v| !version_key(v).is_empty() && qualified(engine, v))
            .map(str::to_owned)
            .ok_or_else(failed)
    }
}

/// Python `engine_update._install_engine(background=False)`.
fn install_pin(
    host: &Host,
    engine: &str,
    deadline: Instant,
    prepare: &dyn Fn(&mut tokio::process::Command) -> std::io::Result<()>,
) -> Result<String> {
    let version = host.pin(engine)?;
    if host.bundled(engine).as_deref() == Some(version.as_str()) {
        return Ok(version);
    }
    let package = package(engine);
    let node = which("node", host.path.as_ref()).map(|p| resolve(&p));
    let beside = node
        .as_ref()
        .and_then(|n| n.parent())
        .map(|p| p.join("npm"));
    let npm = beside
        .filter(|p| p.is_file())
        .or_else(|| which("npm", host.path.as_ref()));
    let (Some(npm), Some(_)) = (npm, node) else {
        return Err(unavailable());
    };
    let root = host.data.join("cortex/engines").join(engine);
    fs::create_dir_all(&root).map_err(|_| failed())?;
    let lock = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_CLOEXEC)
        .open(root.join(".lock"))
        .map_err(|_| failed())?;
    // SAFETY: `lock` owns the descriptor for the whole install.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(unavailable()); // another Cortex process is already updating
    }
    for stale in fs::read_dir(&root).map_err(|_| failed())?.flatten() {
        if stale.file_name().as_encoded_bytes().starts_with(b".tmp-") {
            remove_tree(&stale.path());
        }
    }
    let sdk = (engine == "claude-agent-acp")
        .then(|| CLAUDE_SDK_OVERRIDES.iter().find(|(v, _)| *v == version))
        .flatten()
        .map(|(_, sdk)| *sdk);
    let install = match sdk {
        Some(sdk) => format!("{version}+sdk.{sdk}"),
        None => version.clone(),
    };
    if root.join(&install).exists() {
        return discover_one(&root.join(&install), package)
            .filter(|found| *found == version)
            .ok_or_else(unavailable);
    }
    let staging = root.join(format!(".tmp-{install}"));
    if let Some(sdk) = sdk {
        fs::create_dir(&staging).map_err(|_| failed())?;
        let overrides = format!(
            "{{\"private\": true, \"overrides\": {{\"{CLAUDE_SDK_PACKAGE}\": \"{sdk}\"}}}}"
        );
        fs::write(staging.join("package.json"), overrides).map_err(|_| failed())?;
    }
    run_npm(
        &npm,
        host.path.as_ref(),
        &staging,
        &format!("{package}@{version}"),
        deadline,
        prepare,
    )?;
    let manifest = staging
        .join("node_modules")
        .join(package)
        .join("package.json");
    let sdk_matches = sdk.is_none_or(|sdk| {
        fs::read_to_string(
            staging
                .join("node_modules")
                .join(CLAUDE_SDK_PACKAGE)
                .join("package.json"),
        )
        .ok()
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .is_some_and(|m| m["version"] == sdk)
    });
    if !declared_bin(&manifest).is_some_and(|(v, _)| v == version)
        || discover_one(&staging, package).is_none()
        || !sdk_matches
    {
        remove_tree(&staging);
        return Err(unavailable());
    }
    fs::rename(&staging, root.join(&install)).map_err(|_| failed())?;
    // A pinned (qualified) install is never pruned.
    let mut installs = subdirectories(&root);
    installs.sort_by_key(|p| version_key(name(p)));
    let keep: Vec<_> = installs.iter().rev().take(KEEP).cloned().collect();
    for old in &installs {
        if !keep.contains(old) && !pinned(engine, name(old)) {
            remove_tree(old);
        }
    }
    Ok(version)
}
/// Python `_run(check=True)`: helper environment, npm's own directory first on
/// PATH, cwd `/`, output discarded, killed at the bound. Failures are OSError
/// or SubprocessError: the retained "retry after checking npm/network" refusal.
fn run_npm(
    npm: &Path,
    helper_path: Option<&OsString>,
    staging: &Path,
    spec: &str,
    deadline: Instant,
    prepare: &dyn Fn(&mut tokio::process::Command) -> std::io::Result<()>,
) -> Result<()> {
    if Instant::now() >= deadline {
        return Err(failed());
    }
    let mut command = tokio::process::Command::new(npm);
    prepare(&mut command).map_err(|_| failed())?;
    // `str(Path(npm).parent)`: a bare name's parent is ".".
    let directory = npm
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut path = OsString::from(directory);
    path.push(":");
    path.push(helper_path.cloned().unwrap_or_default());
    if Instant::now() >= deadline {
        return Err(failed());
    }
    let mut child = command
        .as_std_mut()
        .env("PATH", path)
        .arg("install")
        .arg("--prefix")
        .arg(staging)
        .args(["--no-audit", "--no-fund", "--omit=dev", "--no-save", spec])
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| failed())?;
    loop {
        if let Some(status) = child.try_wait().map_err(|_| failed())? {
            return if status.success() {
                Ok(())
            } else {
                Err(failed())
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(failed());
        }
        std::thread::sleep(
            Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
}
/// Python `automatic.record_status(engine, "downloaded")`: best effort only.
fn record_downloaded(host: &Host, engine: &str) {
    let folder = host.route.config.join("cortex");
    let _ = (|| -> Result<()> {
        crate::local::private_tree(&folder)?;
        ensure_ignore(&folder)?;
        write::replace(
            &folder.join(format!("first-run-{engine}.json")),
            b"{\"phase\": \"downloaded\"}",
        )
    })();
}
/// `management.onboard_opencode` after its opt-in, revision-syntax and name checks
/// (config_install.rs): the revision is checked under the owner lock, a trusted
/// OpenCode binary whose sandboxed `--version` is qualified is registered
/// referencing the owner's own login (reusing an identical connection), its state
/// is recomputed and its models are observed once.
pub(super) fn onboard_opencode(host: &Host, folder: &Path, p: &Value) -> Result<Value> {
    crate::local::private_tree(folder)?;
    ensure_ignore(folder)?;
    let path = folder.join("connections.json");
    let expected = p["expected_revision"].as_str().unwrap_or("");
    {
        let _lock = write::owner_lock(folder)?;
        let before = reader::read_plain(&path, true)?;
        write::check("connections", expected, &write::token(before.as_deref()))?;
    }
    let binary = p["binary"].as_str().map(PathBuf::from);
    let found = opencode_engine::discover(&host.route.home, host.path.as_ref(), binary.as_deref())
        .ok_or_else(|| refuse("opencode_not_found", "No trusted OpenCode binary reporting a version was found (looked at the given binary, $CORTEX_OPENCODE_BINARY, ~/.opencode/bin/opencode and PATH)."))?;
    // `opencode_install.verify_install`.
    opencode_engine::trusted_binary(&found.binary, &host.route.home).map_err(|code| {
        refuse(
            code,
            format!(
                "{} is not a trusted OpenCode binary.",
                found.binary.display()
            ),
        )
    })?;
    if !opencode_engine::qualified(&found.version) {
        return Err(refuse(
            "opencode_version_unqualified",
            format!("OpenCode {} is not a version Cortex qualified (2.x.x); install a qualified version or wait for Cortex to qualify this one.", found.version),
        ));
    }
    let launch = vec![
        found.binary.to_string_lossy().into_owned(),
        opencode_engine::ACP.to_owned(),
    ];
    let name = p["name"].as_str().unwrap_or("OpenCode");
    let native = host.route.home.join(".local/share/opencode");
    let (id, state) = {
        let _lock = write::owner_lock(folder)?;
        let before = reader::read_plain(&path, true)?;
        let input = registry_json::RegistryJson::parse(before.as_deref())?;
        let mut registry = registry::normalize(&input.value)?;
        let state = crate::config_read_model::native_login_marker("opencode", &native);
        let existing = registry["connections"]
            .as_object()
            .unwrap()
            .values()
            .find(|r| {
                r["engine"] == "opencode"
                    && r["provider"] == "opencode"
                    && r["auth_mode"] == "native_account"
                    && r["key_kind"].is_null()
                    && r["endpoint"].is_null()
                    && r["launch_command"] == json!(launch)
            })
            .map(|r| r["id"].as_str().unwrap().to_owned());
        let id = match existing {
            Some(id) => {
                // `onboarding.validate`: recompute the state, touching nothing else.
                if registry["connections"][&id]["state"] != state {
                    registry["connections"][&id]["state"] = json!(state);
                    registry["connections"][&id]["updated_at"] = json!(cortex_core::now());
                    let body = input.bytes(&registry);
                    if body.len() > FRAME_BUDGET {
                        return Err(refuse(
                            "machine_reply_too_large",
                            "Settings exceed the native write limit.",
                        ));
                    }
                    write::replace(&path, &body)?;
                }
                id
            }
            None => {
                let mut id = None;
                for _ in 0..32 {
                    let mut bytes = [0u8; 6];
                    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
                    let candidate: String = std::iter::once("cn_".to_owned())
                        .chain(bytes.iter().map(|b| format!("{b:02x}")))
                        .collect();
                    if registry["connections"].get(&candidate).is_none() {
                        id = Some(candidate);
                        break;
                    }
                }
                let id = id.ok_or_else(|| {
                    refuse(
                        "identifier_unavailable",
                        "Could not allocate a unique connection id; retry.",
                    )
                })?;
                let now = cortex_core::now();
                registry["connections"][&id] = json!({
                    "id": id, "name": name, "engine": "opencode", "provider": "opencode",
                    "auth_mode": "native_account", "key_kind": null, "endpoint": null,
                    "launch_command": launch,
                    "credential_ref": {"kind": "engine_native_dir", "path": native, "owned": false},
                    "approved_fingerprint": crate::config_read_model::route_fingerprint(
                        "opencode", "opencode", None, Some(launch.as_slice())),
                    "state": state, "qualified": true, "created_at": now, "updated_at": now,
                    "max_concurrent_sessions": 4,
                });
                let body = input.bytes(&registry);
                if body.len() > FRAME_BUDGET {
                    return Err(refuse(
                        "machine_reply_too_large",
                        "Settings exceed the native write limit.",
                    ));
                }
                write::replace(&path, &body)?;
                id
            }
        };
        (id, state)
    };
    // `refresh_connection`: observe models once; a failure keeps the connection.
    let phase = match catalog_refresh::observe_first_run(&host.route, &id) {
        Ok(Some(row)) if row["success"] == true => "ready",
        _ => "catalog_failed",
    };
    let after = reader::read_plain(&path, true)?;
    let input = registry_json::RegistryJson::parse(after.as_deref())?;
    let registry = registry::normalize(&input.value)?;
    let mut view =
        connection_management_view(&registry, &write::token(after.as_deref()), after.is_some());
    view["connection"] = view["connections"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|c| c["id"] == id)
        .cloned()
        .unwrap_or(Value::Null);
    view["changed"] = json!(true);
    view["catalog_status"] = json!(phase);
    view["opencode"] = json!({"version":found.version,
        "next_action":match state {
            "configured" => json!("refresh_catalog"),
            "authorization_required" => json!("opencode_auth_login"),
            "invalid" => json!("make_credential_dir_private"),
            _ => Value::Null,
        },
        "automatic_routing":"excluded_until_routing_opts_in"});
    Ok(view)
}
/// The non-refusal `install_engine` request (engine is codex-acp or claude-agent-acp).
pub(super) fn install(
    host: &Host,
    engine: &str,
    deadline: Instant,
    prepare: &dyn Fn(&mut tokio::process::Command) -> std::io::Result<()>,
) -> Result<Value> {
    let version = install_pin(host, engine, deadline, prepare)?;
    record_downloaded(host, engine);
    Ok(
        json!({"engine":engine,"version":version,"status":"installed",
        "discovery":host.discovery()?}),
    )
}
