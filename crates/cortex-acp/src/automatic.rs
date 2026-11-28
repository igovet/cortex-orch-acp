//! Startup background first-run pass, ported from Python
//! `connections.automatic` and `connections.engine_update` (the oracle).
//!
//! One pass per process under `automatic-setup.lock` (a concurrent pass skips).
//! For each npm engine it installs the manifest pin beside the current version
//! only when absent (npm in a private staging directory, verified, then renamed;
//! never npm latest, never a downgrade), discovers the qualified install,
//! registers a missing native-account connection only when an existing native
//! login marker is present and the owner did not opt out, and then observes that
//! connection's models once without a model prompt. Diagnostics go only to
//! `first-run-<engine>.json` and stderr. A failure is isolated per engine and is
//! retried only by a later startup; no uncertain effect is replayed.
use crate::{
    adapters::{self, AdapterHost, PACKAGES},
    registry::Registry,
};
use cortex_core::{store::locks, CoreError, CoreResult};
use serde_json::{json, Value};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use workspace_service::{
    config_read_model::{native_login_marker, route_fingerprint},
    native_routes::{observe_first_run_catalog, valid_first_run_catalog_entry, RouteEnvironment},
};

const LOCK: &str = "automatic-setup.lock";
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
/// The current version and one to fall back to.
const KEEP: usize = 2;
const CLAUDE_SDK_PACKAGE: &str = "@anthropic-ai/claude-agent-sdk";
/// 0.87.0 pins an SDK that predates newer models; this exact refresh was
/// inspected independently (Python `_CLAUDE_SDK_OVERRIDES`).
const CLAUDE_SDK_OVERRIDES: [(&str, &str); 1] = [("0.87.0", "0.3.293")];
const MAX_JSON: u64 = 4 * 1024 * 1024;

/// Trusted host inputs only (process environment and the payload root).
pub struct FirstRun {
    pub environment: RouteEnvironment,
    pub host: AdapterHost,
    /// Directory holding `pinned-dependencies.json`.
    pub manifest_root: PathBuf,
    /// `CORTEX_ENGINE_FOLLOW` is not off.
    pub follow: bool,
}

fn failure(code: &'static str) -> CoreError {
    CoreError::Refused(code)
}
fn safe_reason(reason: &str) -> bool {
    regex::Regex::new(r"^[a-z][a-z0-9_]{0,63}(?::[A-Za-z][A-Za-z0-9_]{0,63})?$")
        .unwrap()
        .is_match(reason)
}
/// Python `json.dumps({"phase": ...})` with its default separators.
fn status_body(phase: &str, reason: Option<&str>) -> String {
    let quoted = |s: &str| serde_json::to_string(s).unwrap();
    match reason.filter(|r| phase == "catalog_failed" && safe_reason(r)) {
        Some(reason) => format!(
            "{{\"phase\": {}, \"reason\": {}}}",
            quoted(phase),
            quoted(reason)
        ),
        None => format!("{{\"phase\": {}}}", quoted(phase)),
    }
}
fn failed_status_body(phase: &str, stage: &str, reason: &str) -> String {
    let mut status = json!({"phase":phase,"stage":stage});
    if safe_reason(reason) {
        status["reason"] = json!(reason);
    }
    status.to_string()
}
fn random_hex() -> CoreResult<String> {
    let mut bytes = [0u8; 6];
    fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}
fn private_bytes(path: &Path) -> CoreResult<Option<Vec<u8>>> {
    if path.symlink_metadata().is_err() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    locks::open_private(path, false, true)?
        .take(MAX_JSON + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON {
        return Err(failure("native_first_run_json_too_large"));
    }
    Ok(Some(bytes))
}
/// Python `shutil.which("npm", path=PATH)`: an executable non-directory file.
fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/bin:/usr/bin".into());
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file() && adapters::executable(candidate))
}
fn remove_tree(path: &Path) {
    // Like shutil.rmtree(ignore_errors=True): never follow or remove a link.
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink())
    {
        let _ = fs::remove_dir_all(path);
    }
}
fn pinned(engine: &str, version: &str) -> bool {
    if engine == "claude-agent-acp" {
        return adapters::qualified(engine, version.split("+sdk.").next().unwrap());
    }
    adapters::qualified(engine, version)
}
/// Bounded npm run; stdin closed, output discarded, killed at the deadline.
fn run_npm(npm: &Path, args: &[String]) -> CoreResult<()> {
    let directory = npm.parent().unwrap_or(Path::new("/"));
    let path = std::env::var("PATH").unwrap_or_default();
    let mut child = Command::new(npm)
        .args(args)
        .env("PATH", format!("{}:{path}", directory.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return if status.success() {
                Ok(())
            } else {
                Err(failure("native_engine_install_failed"))
            };
        }
        if started.elapsed() >= INSTALL_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return Err(failure("native_engine_install_timeout"));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

impl FirstRun {
    pub fn from_process(environment: &RouteEnvironment, runtime_root: &Path) -> Self {
        Self {
            environment: environment.clone(),
            host: AdapterHost::from_process(&environment.home, runtime_root),
            manifest_root: runtime_root.to_path_buf(),
            follow: !crate::startup::off("CORTEX_ENGINE_FOLLOW"),
        }
    }
    fn root(&self) -> PathBuf {
        self.environment.config.join("cortex")
    }
    /// The pass never fails startup and never prints raw setup errors.
    pub fn run(&self) {
        let _ = self.pass();
    }
    fn pass(&self) -> CoreResult<()> {
        let root = self.root();
        crate::startup::owner_root(&root, &self.environment.home)?;
        // A busy or unsafe lock means another pass owns first-run setup.
        let Ok(_lock) = locks::AccessLock::acquire(&root.join(LOCK), true, true, Duration::ZERO)
        else {
            return Ok(());
        };
        // A malformed owner preset cannot prevent connection setup.
        let _ = crate::startup::seed_defaults(&self.environment);
        for (engine, package) in PACKAGES {
            // Kept for this engine's phases only. A reader cannot confuse an
            // abandoned receipt for one adapter with another adapter's work.
            let Ok(_engine_lock) = locks::AccessLock::acquire(
                &root.join(format!("automatic-setup-{engine}.lock")),
                true,
                true,
                Duration::ZERO,
            ) else {
                continue;
            };
            if let Err((stage, error)) = self.setup_engine(engine, package) {
                self.write_status(engine, &failed_status_body("setup_failed", stage, error.code()))?;
            }
        }
        Ok(())
    }
    fn status(&self, engine: &str, phase: &str, reason: Option<&str>) -> CoreResult<()> {
        self.write_status(engine, &status_body(phase, reason))
    }
    fn write_status(&self, engine: &str, body: &str) -> CoreResult<()> {
        let root = self.root();
        locks::private_directory(&root, false)?;
        adapters::atomic_private(
            &root.join(format!("first-run-{engine}.json")),
            body.as_bytes(),
        )
    }
    /// Python `read_status_reason`: a recorded, allowlisted failure code only.
    fn status_reason(&self, engine: &str) -> CoreResult<Option<String>> {
        let Some(bytes) = private_bytes(&self.root().join(format!("first-run-{engine}.json")))?
        else {
            return Ok(None);
        };
        let body: Value = match serde_json::from_slice(&bytes) {
            Ok(body) => body,
            Err(_) => return Ok(None),
        };
        if body["phase"] != "catalog_failed" {
            return Ok(None);
        }
        Ok(body["reason"]
            .as_str()
            .filter(|r| safe_reason(r))
            .map(str::to_owned))
    }
    /// Python `registry.read_registry`, including its owner-root repair.
    fn registry(&self) -> CoreResult<(Option<Vec<u8>>, Registry)> {
        let root = self.root();
        crate::startup::owner_root(&root, &self.environment.home)?;
        let bytes = adapters::read_registry(&root.join("connections.json"))?;
        let state = Registry::parse(bytes.as_deref())?;
        Ok((bytes, state))
    }
    fn setup_engine(&self, engine: &'static str, package: &str) -> Result<(), (&'static str, CoreError)> {
        let adapter = |error| ("installing_adapter", error);
        let connections = |error| ("reading_connections", error);
        let connect = |error| ("connecting", error);
        let saved_catalog = |error| ("checking_saved_catalog", error);
        let refresh = |error| ("refreshing_models", error);
        self.status(engine, "downloading", None).map_err(adapter)?;
        // An offline or failed update must not hide a usable owned copy.
        let installation = self.install_pin(engine, package);
        let Some((_, launch)) = adapters::discover(engine, package, &self.host)
            .filter(|(version, _)| adapters::qualified(engine, version))
        else {
            let reason = installation.as_ref().err().map_or("qualified_adapter_unavailable", CoreError::code);
            return self.write_status(engine, &failed_status_body("download_failed", "installing_adapter", reason))
                .map_err(adapter);
        };
        let (_, state) = self.registry().map_err(connections)?;
        let existing = state.first_of(engine);
        if existing.is_none() && state.opted_out(engine) {
            return self.status(engine, "opted_out", None).map_err(connections);
        }
        self.status(engine, "connecting", None).map_err(connect)?;
        let created = self.register(engine, &launch).map_err(connect)?;
        let id = match (&created, existing) {
            (Some(id), _) => id.clone(),
            (None, Some(id)) => id,
            (None, None) => {
                // Includes an owner removal racing this pass: the registry decides.
                let (_, state) = self.registry().map_err(connections)?;
                let phase = if state.opted_out(engine) {
                    "opted_out"
                } else {
                    "login_required"
                };
                return self.status(engine, phase, None).map_err(connect);
            }
        };
        if created.is_none() {
            if self.reuse_saved_catalog(&id).map_err(saved_catalog)? {
                return self.status(engine, "ready", None).map_err(saved_catalog);
            }
        }
        self.status(engine, "refreshing_models", None).map_err(refresh)?;
        let (phase, reason) = self.observe(&id);
        // Best effort first, then a required rewrite keeping the recorded code.
        let _ = self.status(engine, phase, reason.as_deref());
        let reason = self.status_reason(engine).map_err(refresh)?;
        self.status(engine, phase, reason.as_deref()).map_err(refresh)
    }
    fn reuse_saved_catalog(&self, id: &str) -> CoreResult<bool> {
        match self.has_snapshot(id) {
            // A malformed newest snapshot supplies no retained catalogue
            // evidence. Use the ordinary fresh first-run model observation.
            Err(CoreError::Json(_) | CoreError::Refused("invalid_catalog_snapshot")) => Ok(false),
            result => result,
        }
    }
    /// Python `sync_discovered_connections` + `create_referenced_connection(
    /// automatic=True)`: never touches an existing connection; the opt-out,
    /// existing-engine and login-marker checks repeat under the owner lock.
    fn register(&self, engine: &str, launch: &[String]) -> CoreResult<Option<String>> {
        // Python `sync_discovered_connections`: a portable read refusal
        // creates nothing.
        match self.registry() {
            Ok((_, state)) if state.first_of(engine).is_none() => {}
            Ok(_) | Err(CoreError::Refused(_)) => return Ok(None),
            Err(error) => return Err(error),
        }
        let provider = if engine == "codex-acp" {
            "openai"
        } else {
            "anthropic"
        };
        let fingerprint = route_fingerprint(engine, provider, None, Some(launch));
        let now = crate::startup::python_now();
        let root = self.root();
        let result =
            (|| -> CoreResult<Option<String>> {
                // Python `registry.transaction`: a blocking, unbounded `flock`.
                let _lock = locks::AccessLock::acquire(
                    &root.join("connections.lock"),
                    true,
                    true,
                    Duration::MAX,
                )?;
                let (bytes, mut state) = self.registry()?;
                // The transaction serializes the state before its checks, so an
                // unencodable registry fails here like Python's UnicodeEncodeError.
                state.to_json()?;
                if state.opted_out(engine) || state.first_of(engine).is_some() {
                    return Ok(None);
                }
                let native = self.environment.home.join(if engine == "codex-acp" {
                    ".codex"
                } else {
                    ".claude"
                });
                let marker = if engine == "codex-acp" {
                    "auth.json"
                } else {
                    ".credentials.json"
                };
                if !native.join(marker).is_file() {
                    return Ok(None);
                }
                let mut id = None;
                for _ in 0..32 {
                    let candidate = format!("cn_{}", random_hex()?);
                    if state.get(&candidate).is_none() {
                        id = Some(candidate);
                        break;
                    }
                }
                let id = id.ok_or_else(|| failure("identifier_unavailable"))?;
                state.insert(id.clone(), json!({
                "id": id, "name": format!("{engine} (discovered)"), "engine": engine,
                "provider": provider, "auth_mode": "native_account", "key_kind": null,
                "endpoint": null, "launch_command": launch,
                "credential_ref": {"kind": "engine_native_dir", "path": native, "owned": false},
                "approved_fingerprint": fingerprint,
                "state": native_login_marker(engine, &native), "qualified": true,
                "created_at": now, "updated_at": now, "max_concurrent_sessions": 4,
            }));
                let text = state.to_json()?;
                if bytes.as_deref() != Some(text.as_bytes()) {
                    adapters::atomic_private(&root.join("connections.json"), text.as_bytes())?;
                }
                Ok(Some(id))
            })();
        match result {
            // Python skips a refused (portable) creation and creates nothing.
            Err(CoreError::Refused(_)) => Ok(None),
            other => other,
        }
    }
    /// Python `catalog.store.latest_connection_snapshot(...) is not None`.
    fn has_snapshot(&self, id: &str) -> CoreResult<bool> {
        let directory = self.environment.state.join("cortex/catalog").join(id);
        if !directory.is_dir() {
            return Ok(false);
        }
        let mut files: Vec<_> = fs::read_dir(&directory)?
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                let name = adapters::file_name(p);
                name.starts_with("cs_") && name.ends_with(".json") && p.is_file()
            })
            .collect();
        files.sort_by_key(|p| std::cmp::Reverse(p.metadata().and_then(|m| m.modified()).ok()));
        for file in files {
            let Some(bytes) = private_bytes(&file)? else {
                continue;
            };
            let raw: Value = serde_json::from_slice(&bytes)?;
            let entries = match raw.get("entries") {
                None => vec![],
                Some(Value::Array(entries)) => entries.clone(),
                Some(_) => return Err(failure("invalid_catalog_snapshot")),
            };
            if !raw.is_object()
                || raw.get("snapshot_id").is_none()
                || raw.get("created_at").is_none()
                || entries
                    .iter()
                    .any(|e| !valid_first_run_catalog_entry(e, id))
            {
                return Err(failure("invalid_catalog_snapshot"));
            }
            let reason = &raw["unavailable_reason"];
            if entries.is_empty() && !reason.is_null() && reason != "no_models_observed" {
                continue;
            }
            return Ok(true);
        }
        Ok(false)
    }
    /// Python `refresh_connection`: ready, or a fixed failure code.
    fn observe(&self, id: &str) -> (&'static str, Option<String>) {
        // Python's state-root creation makes missing non-managed ancestors
        // (0755 before umask); native admission requires the state root parent.
        if let Err(error) = fs::create_dir_all(&self.environment.state) {
            return (
                "catalog_failed",
                Some(format!("refresh_error:{}", CoreError::from(error).code())),
            );
        }
        match observe_first_run_catalog(&self.environment, id) {
            Ok(Some(row)) if row["success"] == true => ("ready", None),
            Ok(Some(row)) => ("catalog_failed", row["reason"].as_str().map(str::to_owned)),
            Ok(None) => {
                // Python's launch-time verifier is not ported for this launch shape.
                // Retain an explicit unavailable outcome, never invoke a Python helper.
                eprintln!("cortex: native first-run catalog observation unavailable");
                (
                    "catalog_failed",
                    Some("native_catalog_unavailable".to_owned()),
                )
            }
            Err(error) => ("catalog_failed", Some(error.code().to_owned())),
        }
    }
    /// Python `engine_update.pinned_version`: the qualified manifest pin.
    fn pin(&self, engine: &str) -> CoreResult<String> {
        let text = fs::read_to_string(self.manifest_root.join("pinned-dependencies.json"))?;
        let manifest: Value = serde_json::from_str(&text)?;
        manifest["engines"][engine]["pinned_package_version"]
            .as_str()
            .filter(|v| !adapters::version_key(v).is_empty() && adapters::qualified(engine, v))
            .map(str::to_owned)
            .ok_or_else(|| failure("native_engine_pin_unqualified"))
    }
    /// Python `engine_update._install_engine(background=True)`: install the pin
    /// iff absent; `Some(version)` only for a newly published install.
    fn install_pin(&self, engine: &str, package: &str) -> CoreResult<Option<String>> {
        let version = self.pin(engine)?;
        if adapters::discover_bundled(engine, package, &self.host)
            .is_some_and(|(found, _)| found == version)
        {
            return Ok(None);
        }
        let Some(node) = self.host.node.as_ref() else {
            return Ok(None);
        };
        let beside = node.parent().map(|p| p.join("npm"));
        let Some(npm) = beside.filter(|p| p.is_file()).or_else(|| which("npm")) else {
            return Ok(None);
        };
        let root = self.host.owned.join(engine);
        fs::create_dir_all(&root)?;
        let lock = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .custom_flags(libc::O_CLOEXEC)
            .open(root.join(".lock"))?;
        // SAFETY: the descriptor is owned by `lock` for the whole install.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Ok(None); // another Cortex process is already updating
        }
        for stale in fs::read_dir(&root)?.filter_map(Result::ok) {
            if stale.file_name().as_encoded_bytes().starts_with(b".tmp-") {
                remove_tree(&stale.path());
            }
        }
        let sdk = (engine == "claude-agent-acp")
            .then(|| CLAUDE_SDK_OVERRIDES.iter().find(|(v, _)| *v == version))
            .flatten()
            .map(|(_, sdk)| *sdk);
        let name = match sdk {
            Some(sdk) => format!("{version}+sdk.{sdk}"),
            None => version.clone(),
        };
        if root.join(&name).exists() {
            return Ok(None);
        }
        let staging = root.join(format!(".tmp-{name}"));
        if let Some(sdk) = sdk {
            fs::create_dir(&staging)?;
            let overrides = format!(
                "{{\"private\": true, \"overrides\": {{\"{CLAUDE_SDK_PACKAGE}\": \"{sdk}\"}}}}"
            );
            fs::write(staging.join("package.json"), overrides)?;
        }
        let staging_text = staging.to_str().ok_or_else(|| failure("unsafe_storage"))?;
        // A failed npm leaves its staging for the next pass's cleanup, like Python.
        run_npm(
            &npm,
            &[
                "install".into(),
                "--prefix".into(),
                staging_text.into(),
                "--no-audit".into(),
                "--no-fund".into(),
                "--omit=dev".into(),
                "--no-save".into(),
                format!("{package}@{version}"),
            ],
        )?;
        let manifest = staging
            .join("node_modules")
            .join(package)
            .join("package.json");
        let node_text = node.to_str().ok_or_else(|| failure("unsafe_storage"))?;
        let verified = adapters::declared_bin(&manifest).is_some_and(|(v, _)| v == version)
            && adapters::discover_in(std::slice::from_ref(&staging), engine, package, node_text)
                .is_some()
            && sdk.is_none_or(|sdk| {
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
        if !verified {
            remove_tree(&staging);
            return Ok(None);
        }
        fs::rename(&staging, root.join(&name))?;
        // A pinned (qualified) install is never pruned: the two newest could
        // both be unqualified and leave the engine unlaunchable.
        let mut installs = adapters::subdirectories(&root);
        installs.sort_by_key(|p| adapters::version_key(adapters::file_name(p)));
        let keep: Vec<_> = installs.iter().rev().take(KEEP).cloned().collect();
        for old in &installs {
            if !keep.contains(old) && !pinned(engine, adapters::file_name(old)) {
                remove_tree(old);
            }
        }
        if adapters::qualified(engine, &version) && self.follow {
            let _ = crate::startup::follow_and_report(&self.root(), &self.host);
        }
        Ok(Some(version))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_matches_python_json_dumps() {
        assert_eq!(
            status_body("downloading", None),
            "{\"phase\": \"downloading\"}"
        );
        assert_eq!(
            status_body("catalog_failed", Some("engine_unreachable:EngineTransportClosed")),
            "{\"phase\": \"catalog_failed\", \"reason\": \"engine_unreachable:EngineTransportClosed\"}"
        );
        // A reason is recorded only for catalog failures, and only as a fixed code.
        assert_eq!(status_body("ready", Some("x")), "{\"phase\": \"ready\"}");
        assert_eq!(
            status_body("catalog_failed", Some("Free text!")),
            "{\"phase\": \"catalog_failed\"}"
        );
    }

    #[test]
    fn claude_sdk_builds_and_qualified_versions_are_pinned() {
        assert!(pinned("claude-agent-acp", "0.87.0+sdk.0.3.293"));
        assert!(pinned("codex-acp", "2.3.0") && pinned("codex-acp", "2.6.0"));
        assert!(pinned("claude-agent-acp", "0.107.0") && !pinned("claude-agent-acp", "0.107.2"));
        assert!(!pinned("codex-acp", "2.4.0"));
    }

    #[test]
    fn malformed_newest_catalog_uses_fresh_observation_and_unsafe_files_refuse() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = std::env::temp_dir().join(format!("first-run-catalog-{}", random_hex().unwrap()));
        let id = "cn_000000000001";
        let folder = root.join("cortex/catalog").join(id);
        fs::create_dir_all(&folder).unwrap();
        let environment = RouteEnvironment {
            home: root.clone(), config: root.join("config"), state: root.clone(), routing_override: None,
        };
        let pass = FirstRun::from_process(&environment, &root);
        let write = |name: &str, content: &[u8]| {
            let path = folder.join(name);
            fs::write(&path, content).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            path
        };
        // Current Codex reports the worker launch mode, which is not part of
        // the stable route identity. First-run accepts the current producer.
        let route = json!({"engine":"codex-acp","connection_id":id,"model":"fixture",
            "id":"xr_ce5fce3688a1434fe7ff36a2",
            "supported_parameters":[["mode","permission-profile:cortex-workspace"]]});
        let catalogue = json!({"snapshot_id":"cs_000000000001","created_at":"fixture","entries":[{
            "route":route,"connection_id":id,"catalog_snapshot_id":"cs_000000000001",
            "label":"Fixture","disambiguated_label":"Fixture","evidence":{}
        }]});
        let original = write("cs_000000000001.json", catalogue.to_string().as_bytes());
        fs::File::options().write(true).open(&original).unwrap()
            .set_times(fs::FileTimes::new().set_modified(std::time::SystemTime::UNIX_EPOCH)).unwrap();
        assert!(pass.reuse_saved_catalog(id).unwrap());
        let newest = write("cs_000000000002.json", b"{");
        assert!(!pass.reuse_saved_catalog(id).unwrap());
        write("cs_000000000002.json", br#"{"snapshot_id":"cs_000000000002","created_at":"fixture","entries":[{}]}"#);
        assert!(!pass.reuse_saved_catalog(id).unwrap());
        fs::remove_file(&newest).unwrap();
        let alias_target = write("not-a-catalog.json", b"{}");
        symlink(&alias_target, &newest).unwrap();
        assert!(pass.reuse_saved_catalog(id).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_setup_records_only_allowlisted_code_shapes() {
        let safe: Value = serde_json::from_str(&failed_status_body(
            "setup_failed", "checking_saved_catalog", "unsafe_storage",
        )).unwrap();
        assert_eq!(safe, json!({"phase":"setup_failed","stage":"checking_saved_catalog","reason":"unsafe_storage"}));
        let unsafe_reason: Value = serde_json::from_str(&failed_status_body(
            "setup_failed", "checking_saved_catalog", "/private/location",
        )).unwrap();
        assert!(unsafe_reason.get("reason").is_none());
    }
}
