//! Owner route binding. No discovery, login, defaults seeding or Python.
//! Selection and consent use owner policy and retained direct-user evidence.
//! The only owner write is an explicit pre-task language default (Python parity).
//! The startup-only first-run catalogue facade publishes observed snapshots only.
mod consent;
#[cfg(test)]
mod consent_tests;
mod grammar;
mod grammar_data;
mod policy;
pub(crate) use policy::{coordinator_models_valid, tier_efforts_valid, COORDINATOR_MODELS};
mod presentation;
pub(crate) use presentation::russian_note;
mod quota;
pub mod short_text;
use cortex_core::{CoreError, CoreResult};
use cortex_engine::environment::{CodexLauncher, EngineKind, LaunchSpec};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::OpenOptions,
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

/// Shared legacy policy-label vocabulary; editor reads do not authorize routes.
pub(crate) fn reserved_policy_word(word: &str) -> bool {
    use grammar_data::{MEANS_WORDS, NEGATION_WORDS, REQUEST_WORDS, WORK_VERBS};
    ["the", "a", "an", "model", "модели", "моделью"].contains(&word)
        || [REQUEST_WORDS, WORK_VERBS, MEANS_WORDS, NEGATION_WORDS]
            .iter()
            .any(|p| grammar::re(&format!("^(?:{p})$")).is_match(word))
}
fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(bytes))
}
/// An engine's pure alias entry (the Claude adapter's `default`) names no concrete model, so it is
/// never offered, listed in a catalogue or chosen by a route decision: every route is a concrete model.
pub(crate) fn is_alias_model(id: &str) -> bool {
    id.eq_ignore_ascii_case("default")
}
pub(crate) fn is_concrete_effort(id: &str) -> bool {
    let id = id.trim();
    !id.is_empty() && !id.eq_ignore_ascii_case("default") && !id.eq_ignore_ascii_case("engine_default")
}

/// The Cortex-owned launch mode a Codex engine advertises once the worker permission profile is
/// configured. It describes how Cortex launches the engine, not which route this is, so it never
/// enters a route identity: hardening or loosening the launch must keep every saved id valid.
fn launch_detail(pair: &(String, String)) -> bool {
    pair.0 == "mode" && pair.1 == crate::native_engine_policy::CODEX_WORKER_PROFILE_MODE
}
fn route_hash(engine: &Value, connection_id: &Value, model: &Value, pairs: &[(String, String)]) -> String {
    let body = json!({
        "engine": engine, "connection_id": connection_id,
        "model": model, "supported_parameters": pairs,
    });
    format!("xr_{}", &digest(&serde_json::to_vec(&body).unwrap())[..24])
}
/// Python `portable.identity.route_fingerprint` over a stored catalog route, minus launch details.
/// Pairs are sorted, never deduplicated: claude-agent-acp advertises several
/// `model_config` on/off selects, so a real Claude route repeats those pairs.
pub(crate) fn catalog_route_id(
    engine: &Value,
    connection_id: &Value,
    model: &Value,
    mut pairs: Vec<(String, String)>,
) -> String {
    pairs.retain(|p| !launch_detail(p));
    pairs.sort();
    route_hash(engine, connection_id, model, &pairs)
}
/// The stable id of a stored route when its stored id is that id or the id an earlier build
/// derived over every advertised pair (launch details included); `None`: not this route's id.
pub(crate) fn accepted_route_id(
    stored: &Value,
    engine: &Value,
    connection_id: &Value,
    model: &Value,
    mut pairs: Vec<(String, String)>,
) -> Option<String> {
    let stable = catalog_route_id(engine, connection_id, model, pairs.clone());
    if stored == &json!(stable) {
        return Some(stable);
    }
    pairs.sort();
    (stored == &json!(route_hash(engine, connection_id, model, &pairs))).then_some(stable)
}
/// Every id the retained snapshots of one connection ever gave each model, for resolving a saved
/// id the current catalogue no longer carries. Bounded: the newest snapshots only.
pub(crate) fn retained_route_ids(state: &Path, connection_id: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let directory = state.join("cortex/catalog").join(connection_id);
    let Ok(read) = std::fs::read_dir(&directory) else { return out };
    let mut files: Vec<_> = read
        .flatten()
        .filter(|f| f.file_name().to_str().is_some_and(|s| s.starts_with("cs_") && s.ends_with(".json")))
        .collect();
    files.sort_by_key(|f| std::cmp::Reverse(f.metadata().and_then(|m| m.modified()).ok()));
    for file in files.into_iter().take(64) {
        let Ok(bytes) = std::fs::read(file.path()) else { continue };
        if bytes.len() as u64 > MAX_JSON {
            continue;
        }
        let Ok(snapshot) = serde_json::from_slice::<Value>(&bytes) else { continue };
        for entry in snapshot["entries"].as_array().into_iter().flatten() {
            let r = &entry["route"];
            if let (Some(model), Some(id)) = (r["model"].as_str(), r["id"].as_str()) {
                if r["connection_id"] == connection_id {
                    out.entry(model.to_owned()).or_default().insert(id.to_owned());
                }
            }
        }
    }
    out
}
const MAX_JSON: u64 = 4 * 1024 * 1024;
/// Internal inputs and refusal details for a native worker dispatch or recovery probe.
#[doc(hidden)]
#[derive(Default)]
pub struct WorkerRouteContext {
    pub(crate) held: Vec<crate::native_route_hold::RouteKey>,
    pub(crate) block: Value,
}

fn refuse<T>(code: &'static str) -> CoreResult<T> {
    Err(CoreError::Refused(code))
}
/// Validate a cached first-run row using the shared catalogue evidence contract
/// and Python's recomputed ExecutionRoute identity. This grants no route authority.
pub fn valid_first_run_catalog_entry(entry: &Value, connection_id: &str) -> bool {
    let route = &entry["route"];
    let Ok(pairs) =
        serde_json::from_value::<Vec<(String, String)>>(route["supported_parameters"].clone())
    else {
        return false;
    };
    if !crate::machine::valid_cached_entry(entry, connection_id)
        || !crate::config_read_model::matches(r"^cn_[a-f0-9]{12}$", connection_id)
        || ["engine", "model"]
            .iter()
            .any(|key| !route[key].as_str().is_some_and(|s| !s.is_empty()))
    {
        return false;
    }
    accepted_route_id(
        &route["id"],
        &route["engine"],
        &route["connection_id"],
        &route["model"],
        pairs,
    )
    .is_some()
}
/// Startup-only first-run model observation of one registered connection
/// (Python `connections.automatic.refresh_connection`), never a model prompt.
/// `Ok(None)`: no native outcome is claimed for this launch shape. The reply is
/// a fixed `{connection_id, success, reason}` row, never engine text.
pub fn observe_first_run_catalog(
    environment: &RouteEnvironment,
    connection_id: &str,
) -> CoreResult<Option<Value>> {
    crate::machine::config_action::catalog_refresh::observe_first_run(environment, connection_id)
        .map_err(CoreError::Refused)
}
/// Explicit roots let service channel isolation and disposable tests use the same loader.
/// These paths are trusted host inputs, never socket/model fields.
#[derive(Clone)]
pub struct RouteEnvironment {
    pub home: PathBuf,
    pub config: PathBuf,
    pub state: PathBuf,
    pub routing_override: Option<PathBuf>,
}
impl RouteEnvironment {
    pub fn for_user() -> CoreResult<Self> {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(CoreError::Refused("native_route_home_unavailable"))?;
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".config"));
        let state = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"));
        Ok(Self {
            home,
            config,
            state,
            routing_override: std::env::var_os("CORTEX_ROUTING")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from),
        })
    }
    pub fn for_workspace(config: &crate::Config) -> CoreResult<Self> {
        let mut environment = Self::for_user()?;
        if workspace_protocol::channel::current().is_dev() {
            environment.config = config
                .launch_config_root()
                .unwrap_or_else(|| config.state_dir.join("config"));
            environment.state = config
                .launch
                .dev_state
                .clone()
                .unwrap_or_else(|| config.state_dir.join("state"));
            if config.launch.extracted_dev {
                environment.routing_override = None;
            }
        }
        Ok(environment)
    }
    pub(crate) fn validate(&self) -> CoreResult<()> {
        for path in [&self.home, &self.config, &self.state] {
            if !path.is_absolute()
                || path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return refuse("relative_storage_root");
            }
        }
        // User connection storage must never be project-local, even through links.
        for path in [&self.config, &self.state] {
            // Missing first-run config/state roots are not a transport failure.
            // Validate their nearest existing ancestor without creating anything.
            let mut existing = path.as_path();
            while !existing.try_exists()? {
                existing = existing
                    .parent()
                    .ok_or(CoreError::Refused("native_route_settings_unreadable"))?;
            }
            let real = existing.canonicalize()?;
            for ancestor in real.ancestors() {
                if ancestor == self.home {
                    continue;
                }
                if [".git", ".cortex", ".cortex-dev"]
                    .iter()
                    .any(|m| ancestor.join(m).symlink_metadata().is_ok())
                {
                    return refuse("connection_storage_under_project");
                }
            }
        }
        Ok(())
    }
}
pub(crate) fn read(path: &Path, private: bool) -> CoreResult<Option<Value>> {
    read_bytes(path, private)?
        .map(|bytes| {
            serde_json::from_slice(&bytes)
                .map_err(|_| CoreError::Refused("native_route_settings_invalid"))
        })
        .transpose()
}
/// One bounded descriptor read, also used for byte-exact management revisions.
pub(crate) fn read_bytes(path: &Path, private: bool) -> CoreResult<Option<Vec<u8>>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return refuse("native_route_settings_unreadable"),
    };
    if private {
        private_parents(path)?;
    }
    let info = file.metadata()?;
    if !info.is_file()
        || info.len() > MAX_JSON
        || (private
            && (info.uid() != unsafe { libc::getuid() }
                || info.mode() & 0o077 != 0
                || info.nlink() != 1))
    {
        return refuse("native_route_settings_unreadable");
    }
    let mut bytes = Vec::new();
    file.take(MAX_JSON + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON {
        return refuse("native_route_settings_unreadable");
    }
    Ok(Some(bytes))
}
/// The newest Codex rollout limits of a connection: its native directory (the
/// owner's own Codex runs) and its private `CODEX_HOME`, where its confined
/// sessions keep their rollouts (`view`). Read only; nothing is created.
pub(crate) fn codex_quota(environment: &RouteEnvironment, connection: &str, home: &Path) -> Value {
    let native = quota::codex(home);
    if connection.is_empty()
        || !connection
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return native;
    }
    let private = quota::codex(
        &environment
            .state
            .join("cortex/engine-private")
            .join(connection)
            .join("codex-home"),
    );
    let observed = |w: &Value| {
        ["primary", "secondary"]
            .iter()
            .filter_map(|n| w[n]["observed_at"].as_f64())
            .fold(f64::NEG_INFINITY, f64::max)
    };
    if private.as_object().is_some_and(|m| !m.is_empty())
        && (native.as_object().is_none_or(|m| m.is_empty()) || observed(&private) > observed(&native))
    {
        private
    } else {
        native
    }
}
fn private_parents(path: &Path) -> CoreResult<()> {
    for parent in path.parent().into_iter().flat_map(Path::ancestors) {
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(parent)
            .map_err(|_| CoreError::Refused("native_route_settings_unreadable"))?;
        let info = dir.metadata()?;
        if info.uid() != unsafe { libc::getuid() } || info.mode() & 0o077 != 0 {
            return refuse("native_route_settings_unreadable");
        }
        if parent.file_name().is_some_and(|s| s == "cortex") {
            return Ok(());
        }
    }
    refuse("native_route_settings_unreadable")
}
fn strings(v: &Value) -> CoreResult<Vec<String>> {
    v.as_array()
        .ok_or(CoreError::Refused("invalid_routing_overrides"))?
        .iter()
        .map(|x| {
            x.as_str()
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .ok_or(CoreError::Refused("invalid_routing_overrides"))
        })
        .collect()
}
fn contains(v: &Value, s: &str) -> bool {
    v.as_array().is_some_and(|a| a.iter().any(|x| x == s))
}
fn identity(s: &str, prefix: &str) -> bool {
    s.strip_prefix(prefix).is_some_and(|tail| {
        !tail.is_empty() && tail.len() <= 64 && tail.bytes().all(|c| c.is_ascii_alphanumeric())
    })
}
#[derive(Clone)]
struct Candidate {
    connection: Value,
    entry: Value,
}
impl Candidate {
    fn engine(&self) -> &str {
        self.entry["route"]["engine"].as_str().unwrap_or("")
    }
    fn model(&self) -> &str {
        self.entry["route"]["model"].as_str().unwrap_or("")
    }
    fn connection_id(&self) -> &str {
        self.connection["id"].as_str().unwrap_or("")
    }
    fn route_id(&self) -> &str {
        self.entry["route"]["id"].as_str().unwrap_or("")
    }
}
/// A resolved exact identity, still subject to observed ACP configuration at dispatch.
/// Credentials are references only and this type deliberately has no Debug/Serialize.
pub struct SelectedRoute {
    candidate: Candidate,
    pub parameters: BTreeMap<String, String>,
    pub basis: &'static str,
    pub decision: Value,
    environment: RouteEnvironment,
}
impl SelectedRoute {
    pub fn connection(&self) -> &str {
        self.candidate.connection_id()
    }
    pub fn model(&self) -> &str {
        self.candidate.model()
    }
    pub fn route_id(&self) -> &str {
        self.candidate.route_id()
    }
    pub fn engine(&self) -> &str {
        self.candidate.engine()
    }
    pub fn connection_limit(&self) -> u16 {
        self.candidate.connection["max_concurrent_sessions"]
            .as_u64()
            .unwrap_or(4) as u16
    }
    /// Build only the owner-approved argv, never a shell command or PATH fallback.
    pub fn launch(&self, root: &Path) -> CoreResult<LaunchSpec> {
        let command = strings(&self.candidate.connection["launch_command"])?;
        let executable = command
            .first()
            .map(PathBuf::from)
            .ok_or(CoreError::Refused("connection_unavailable"))?;
        let executable = executable_path(&executable)?;
        let credential = &self.candidate.connection["credential_ref"];
        if credential["kind"] != "engine_native_dir" {
            return refuse("native_route_auth_unsupported");
        }
        let credential_directory = credential["path"]
            .as_str()
            .map(PathBuf::from)
            .ok_or(CoreError::Refused("invalid_credential_ref"))?;
        if !credential_directory.is_absolute() || !credential_directory.is_dir() {
            return refuse("authorization_required");
        }
        let engine = match self.engine() {
            "codex-acp" => EngineKind::Codex,
            "claude-agent-acp" => EngineKind::Claude,
            "opencode" => EngineKind::OpenCode,
            _ => return refuse("native_route_engine_unsupported"),
        };
        let launch = LaunchSpec {
            engine,
            executable,
            args: command[1..].to_vec(),
            cwd: root.to_owned(),
            native_home: self.environment.home.clone(),
            credential_directory,
            base_environment: std::env::vars()
                .filter(|(k, _)| {
                    [
                        "PATH", "LANG", "LC_ALL", "LC_CTYPE", "TZ", "TMPDIR", "USER", "LOGNAME",
                    ]
                    .contains(&k.as_str())
                })
                .collect(),
            codex_launcher: None,
            // The filesystem-view experiment is exercised only by synthetic
            // fixtures. Normal sessions retain the installed engine route.
            confinement: None,
        };
        launch
            .environment()
            .map_err(|_| CoreError::Refused("native_route_environment_invalid"))?;
        Ok(launch)
    }
    /// The launch an engine session actually starts (coordinator, worker,
    /// review, verification or planning). An install-shaped codex-acp launch
    /// also pins Cortex's native Codex launcher as `CODEX_PATH`, as Python's
    /// `EngineProcess` does, so every App Server gets non-persistent trust for
    /// its own working folder and never writes the owner's `config.toml`.
    /// Like Python's install verification, the launcher is prepared (and an
    /// unlocatable bundled Codex refused) only when the engine starts, so
    /// resolving a route for selection or presentation stays effect-free.
    pub fn session_launch(&self, root: &Path) -> CoreResult<LaunchSpec> {
        use crate::machine::config_action::catalog_refresh as catalog;
        let mut launch = self.launch(root)?;
        if launch.engine == EngineKind::Codex
            && catalog::codex_install_shaped(&self.candidate.connection)
        {
            let environment = self.environment.clone();
            let record = self.candidate.connection.clone();
            launch.codex_launcher = Some(CodexLauncher::new(move || {
                catalog::codex_session_launcher(&environment, &record)
                    .map_err(|reason| cortex_engine::Error::InvalidField { field: "codex_launcher".into(), reason })
            }));
        }
        Ok(launch)
    }
}
#[cfg(test)]
pub(crate) fn fixture_session_launch(
    environment: RouteEnvironment,
    root: &Path,
    record: Value,
) -> CoreResult<LaunchSpec> {
    let entry = json!({"route":{"engine":record["engine"]}});
    let launch = SelectedRoute {
        candidate: Candidate {
            connection: record,
            entry,
        },
        parameters: BTreeMap::new(),
        basis: "offline_fixture",
        decision: Value::Null,
        environment,
    }
    .session_launch(root)?;
    Ok(launch)
}

pub struct RouteResolver {
    environment: RouteEnvironment,
    rules: Value,
    defaults: Value,
    candidates: Vec<Candidate>,
}
impl RouteResolver {
    /// First-existing-file precedence, including the channel's project store.
    /// All reads are bounded. The caller must additionally enforce task/project
    /// ceilings, availability and quota before authorizing a dispatch.
    pub fn load(
        root: &Path,
        store_directory: &Path,
        environment: RouteEnvironment,
    ) -> CoreResult<Self> {
        environment.validate()?;
        if !root.is_absolute() || !store_directory.is_absolute() {
            return refuse("relative_storage_root");
        }
        let mut paths = vec![store_directory.join("routing.json")];
        if let Some(path) = &environment.routing_override {
            paths.push(path.clone());
        }
        paths.push(environment.config.join("cortex/routing.json"));
        let mut rules = None;
        for path in paths {
            if let Some(value) = read(&path, false)? {
                rules = Some(value);
                break;
            }
        }
        let rules = rules.ok_or(CoreError::Refused("model_policy_migration_required"))?;
        if rules.get("model_policy_version").is_none() {
            return refuse("model_policy_migration_required");
        }
        if rules["model_policy_version"] != 2 {
            return refuse("model_policy_version_unsupported");
        }
        validate_rules(&rules)?;
        policy::validate_policy(&rules)?;
        let defaults = read(&environment.config.join("cortex/defaults.json"), true)?
            .unwrap_or(json!({"defaults":{}}));
        let registry = read(&environment.config.join("cortex/connections.json"), true)?
            .ok_or(CoreError::Refused("connection_unavailable"))?;
        if registry["schema_version"] != 1 {
            return refuse("native_connection_registry_invalid");
        }
        let records = registry["connections"]
            .as_object()
            .ok_or(CoreError::Refused("native_connection_registry_invalid"))?;
        let mut candidates = Vec::new();
        for (id, record) in records {
            if !identity(id, "cn_") || record["id"] != *id {
                return refuse("native_connection_registry_invalid");
            }
            if record["auth_mode"] != "native_account"
                || record["qualified"] != true
                || record["state"] != "configured"
            {
                continue;
            }
            if !record["max_concurrent_sessions"].is_null()
                && !record["max_concurrent_sessions"]
                    .as_u64()
                    .is_some_and(|n| (1..=64).contains(&n))
            {
                return refuse("native_connection_registry_invalid");
            }
            let fingerprint = digest(&serde_json::to_vec(&json!({
                "engine":record["engine"],"provider":record["provider"],"endpoint":record["endpoint"],
                "launch_command":record["launch_command"]
            }))?);
            if record["approved_fingerprint"] != fingerprint {
                return refuse("untrusted_project_override");
            }
            let directory = environment.state.join("cortex/catalog").join(id);
            let mut files = match std::fs::read_dir(&directory) {
                Ok(files) => files.take(4097).collect::<Result<Vec<_>, _>>()?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => return refuse("native_route_settings_unreadable"),
            };
            if files.len() > 4096 {
                return refuse("native_route_catalog_too_large");
            }
            files.retain(|f| {
                f.file_name().to_str().is_some_and(|s| {
                    s.ends_with(".json") && identity(s.trim_end_matches(".json"), "cs_")
                })
            });
            files.sort_by_key(|f| std::cmp::Reverse(f.metadata().and_then(|m| m.modified()).ok()));
            for file in files {
                let snapshot = read(&file.path(), true)?
                    .ok_or(CoreError::Refused("invalid_catalog_snapshot"))?;
                let entries = snapshot["entries"]
                    .as_array()
                    .ok_or(CoreError::Refused("invalid_catalog_snapshot"))?;
                if entries.is_empty()
                    && !snapshot["unavailable_reason"].is_null()
                    && snapshot["unavailable_reason"] != "no_models_observed"
                {
                    continue;
                }
                for entry in entries {
                    let r = &entry["route"];
                    if entry["connection_id"] != *id
                        || r["connection_id"] != *id
                        || r["engine"] != record["engine"]
                        || entry["catalog_snapshot_id"] != snapshot["snapshot_id"]
                        || file.path().file_stem().and_then(|s| s.to_str())
                            != snapshot["snapshot_id"].as_str()
                    {
                        return refuse("invalid_catalog_snapshot");
                    }
                    let pairs: Vec<(String, String)> =
                        serde_json::from_value(r["supported_parameters"].clone())
                            .map_err(|_| CoreError::Refused("invalid_catalog_snapshot"))?;
                    let Some(stable) =
                        accepted_route_id(&r["id"], &r["engine"], &json!(id), &r["model"], pairs)
                    else {
                        return refuse("invalid_catalog_snapshot");
                    };
                    // A snapshot saved before aliases were dropped may still list one: it is
                    // never a candidate, so no route decision selects it or sends it to an engine.
                    if r["model"].as_str().is_some_and(is_alias_model) {
                        continue;
                    }
                    let facts = entry["evidence"]
                        .as_object()
                        .ok_or(CoreError::Refused("invalid_catalog_snapshot"))?;
                    if facts.values().any(|p| p["source"] == "seed")
                        || (facts.contains_key("authorization_required")
                            && !facts.contains_key("configured"))
                        || entry["evidence"]["catalog_obtained"]["source"] != "discovery"
                    {
                        continue;
                    }
                    let mut entry = entry.clone();
                    entry["route"]["id"] = json!(stable);
                    candidates.push(Candidate {
                        connection: record.clone(),
                        entry,
                    });
                }
                break;
            }
        }
        Ok(Self {
            environment,
            rules,
            defaults,
            candidates,
        })
    }
    fn allowed(&self, c: &Candidate, worker: bool) -> bool {
        let e = c.engine();
        let m = c.model();
        contains(&self.rules["enabled"][e], m)
            && ![m.to_owned(), format!("{e}/{m}"), format!("{e}/*")]
                .iter()
                .any(|s| contains(&self.rules["deny"], s))
            && (!worker
                || self.rules.get("providers").is_none()
                || contains(&self.rules["providers"], e))
    }
    fn selected(
        &self,
        c: &Candidate,
        parameters: Value,
        basis: &'static str,
    ) -> CoreResult<SelectedRoute> {
        let mut parameters: BTreeMap<String, String> = serde_json::from_value(parameters)
            .map_err(|_| CoreError::Refused("unsupported_dependent_setting"))?;
        let pairs: Vec<(String, String)> =
            serde_json::from_value(c.entry["route"]["supported_parameters"].clone())
                .map_err(|_| CoreError::Refused("invalid_catalog_snapshot"))?;
        if matches!(basis, "owner_default" | "owner_task")
            && !parameters
                .get("thought_level")
                .is_some_and(|v| is_concrete_effort(v))
        {
            parameters.remove("thought_level");
            if let Some(effort) =
                presentation::nearest_effort(&presentation::efforts(c), self.settings_effort())
            {
                parameters.insert("thought_level".into(), effort.into());
            }
        }
        // A coordinator stays inside the profile: its effort is limited to the
        // union of its model's tier ranges, a saved or inherited level moved to
        // the nearest allowed one (the lower on a tie). Only a level the route
        // offers is moved; an effort the model does not support at all is still
        // refused below (`unsupported_dependent_setting`), never substituted.
        if matches!(basis, "owner_default" | "owner_task") {
            if let Some(allowed) = self.coordinator_efforts(c) {
                if allowed.is_empty() {
                    return refuse("native_task_route_unavailable");
                }
                if let Some(effort) = parameters
                    .get("thought_level")
                    .filter(|e| {
                        !allowed.contains(e)
                            && pairs.contains(&("thought_level".to_owned(), (*e).clone()))
                    })
                    .cloned()
                {
                    let nearest = presentation::nearest_effort(&allowed, Some(&effort))
                        .unwrap_or_default()
                        .to_owned();
                    parameters.insert("thought_level".into(), nearest);
                }
            }
        }
        if parameters
            .iter()
            .any(|(k, v)| !pairs.contains(&(k.clone(), v.clone())))
        {
            return refuse("unsupported_dependent_setting");
        }
        Ok(SelectedRoute {
            candidate: c.clone(),
            parameters,
            basis,
            decision: json!({}),
            environment: self.environment.clone(),
        })
    }
    fn settings_effort(&self) -> Option<&str> {
        let d = &self.defaults["defaults"];
        d.get("parameters")
            .or_else(|| d["coordinator"].get("parameters"))
            .and_then(|p| p["thought_level"].as_str())
            .filter(|v| is_concrete_effort(v))
    }
    /// Portable or saved exact coordinator defaults. Providers restrict workers,
    /// not a manually configured coordinator. Ambiguity never picks the first.
    pub fn coordinator(&self) -> CoreResult<SelectedRoute> {
        let d = &self.defaults["defaults"];
        let portable = &d["coordinator"];
        let matches: Vec<_> = self
            .candidates
            .iter()
            .filter(|c| {
                self.allowed(c, false)
                    && self.coordinator_eligible(c)
                    && if portable.is_object() {
                        portable["engine"] == c.engine() && portable["model"] == c.model()
                    } else {
                        d["route_connection_id"] == c.connection_id()
                            && d["route_model"] == c.model()
                    }
            })
            .collect();
        if matches.len() != 1 {
            return refuse(if matches.is_empty() {
                "portable_coordinator_unavailable"
            } else {
                "portable_coordinator_ambiguous"
            });
        }
        let parameters = d
            .get("parameters")
            .or_else(|| portable.get("parameters"))
            .cloned()
            .unwrap_or(json!({}));
        self.selected(matches[0], parameters, "owner_default")
    }
    /// An explicit per-task coordinator choice stays inside existing owner eligibility.
    /// A genuine route change drops route-specific parameters. An omitted
    /// effort inherits the Settings preference, narrowed to the new route's
    /// advertised levels; only the same route keeps its other parameters.
    /// An exact route id (a provider/cross-account choice) binds across
    /// connections and does not need a resolvable default; a bare model name
    /// stays on the default's connection, else it must name one route.
    pub(crate) fn task_coordinator(&self, settings: &Value) -> CoreResult<SelectedRoute> {
        let base = self.coordinator();
        let resolved = settings["model"].as_str().map(|m| self.current_route_id(m));
        let chosen = resolved.as_deref().filter(|s| !s.is_empty());
        let allowed: Vec<_> = self
            .candidates
            .iter()
            .filter(|c| self.allowed(c, false) && self.coordinator_eligible(c))
            .collect();
        let matches: Vec<_> = match (chosen, &base) {
            (Some(m), _) if allowed.iter().any(|c| c.route_id() == m) => {
                allowed.into_iter().filter(|c| c.route_id() == m).collect()
            }
            (Some(m), Ok(base)) => allowed
                .into_iter()
                .filter(|c| c.connection_id() == base.connection() && c.model() == m)
                .collect(),
            (Some(m), Err(_)) => allowed.into_iter().filter(|c| c.model() == m).collect(),
            (None, Ok(base)) => allowed
                .into_iter()
                .filter(|c| c.route_id() == base.route_id())
                .collect(),
            (None, Err(e)) => return Err(CoreError::Refused(e.code())),
        };
        if matches.len() != 1 {
            return refuse("native_task_route_unavailable");
        }
        let mut parameters = match &base {
            Ok(base) if matches[0].route_id() == base.route_id() => base.parameters.clone(),
            _ => BTreeMap::new(),
        };
        if let Some(effort) = settings["effort"].as_str().filter(|s| is_concrete_effort(s)) {
            parameters.insert("thought_level".into(), effort.into());
        }
        self.selected(matches[0], serde_json::to_value(parameters)?, "owner_task")
    }
    /// A saved route id the catalogue no longer carries, resolved to the one current route of the
    /// same connection and model that an earlier snapshot gave that id. Anything else (a current id,
    /// a bare model name, an id no retained snapshot knows, or an ambiguous one) comes back unchanged.
    pub(crate) fn current_route_id(&self, saved: &str) -> String {
        if !saved.starts_with("xr_") || self.candidates.iter().any(|c| c.route_id() == saved) {
            return saved.to_owned();
        }
        let mut found = BTreeSet::new();
        let mut seen = BTreeSet::new();
        for c in &self.candidates {
            if !seen.insert(c.connection_id().to_owned()) {
                continue;
            }
            let history = retained_route_ids(&self.environment.state, c.connection_id());
            for other in self.candidates.iter().filter(|o| o.connection_id() == c.connection_id()) {
                if history.get(other.model()).is_some_and(|ids| ids.contains(saved)) {
                    found.insert(other.route_id().to_owned());
                }
            }
        }
        if found.len() == 1 { found.into_iter().next().unwrap() } else { saved.to_owned() }
    }
    /// A coordinator resolved at task start, kept as its exact route identity.
    fn pin(&self, settings: &Value) -> CoreResult<Value> {
        let selected = self.task_coordinator(settings)?;
        Ok(json!({"route_id":selected.route_id(),"parameters":selected.parameters}))
    }
    /// The pinned coordinator route again, independent of the current owner default.
    /// An explicit effort replaces only the thought level.
    fn pinned(&self, pin: &Value, effort: Option<&str>) -> CoreResult<SelectedRoute> {
        let id = pin["route_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(CoreError::Refused("native_task_route_unavailable"))?;
        let id = self.current_route_id(id);
        let id = id.as_str();
        let matches: Vec<_> = self
            .candidates
            .iter()
            .filter(|c| self.allowed(c, false) && c.route_id() == id && self.coordinator_eligible(c))
            .collect();
        if matches.len() != 1 {
            return refuse("native_task_route_unavailable");
        }
        let mut parameters: BTreeMap<String, String> =
            serde_json::from_value(pin["parameters"].clone()).unwrap_or_default();
        if let Some(effort) = effort.filter(|v| is_concrete_effort(v)) {
            parameters.insert("thought_level".into(), effort.into());
        }
        self.selected(matches[0], serde_json::to_value(parameters)?, "owner_task")
    }
    /// Another qualified coordinator route than `connection`/`model`, as a pin: an
    /// eligible owner route with quota left, on another connection (engine or account)
    /// first, then the least used. Used by the no-stop policy to change approach.
    fn alternate_pin(&self, connection: &str, model: &str) -> CoreResult<Option<Value>> {
        let mut options = Vec::new();
        for c in self.candidates.iter().filter(|c| {
            self.allowed(c, false)
                && self.coordinator_eligible(c)
                && !c.route_id().is_empty()
                && !(c.connection_id() == connection && c.model() == model)
        }) {
            let usage = self.usage(c)?;
            if usage.is_none_or(|v| v < 100.0) {
                options.push((c.connection_id() == connection, usage.unwrap_or(0.0), c));
            }
        }
        options.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.total_cmp(&b.1)));
        Ok(options
            .first()
            .map(|(_, _, c)| json!({"route_id":c.route_id(),"parameters":{}})))
    }
    /// Context-free worker selection is used only for preflight. Dispatch supplies retained evidence.
    pub fn worker(&self, role: &str) -> CoreResult<SelectedRoute> {
        self.choose(
            role,
            &json!({}),
            &cortex_core::routing::Evidence::default(),
            false,
            None,
        )
    }
}
fn validate_rules(r: &Value) -> CoreResult<()> {
    let object = r
        .as_object()
        .ok_or(CoreError::Refused("invalid_routing_overrides"))?;
    if object.contains_key("tiers") && r["tiers"].as_object().is_none_or(|m| !m.is_empty()) {
        return refuse("invalid_routing_overrides");
    }
    for key in ["deny", "auto_allow", "providers"] {
        if let Some(v) = r.get(key) {
            strings(v)?;
        }
    }
    if let Some(v) = r.get("providers") {
        if v.as_array().is_none_or(Vec::is_empty) {
            return refuse("invalid_routing_overrides");
        }
    }
    for key in ["enabled", "models", "pin", "engine_affinity"] {
        if r.get(key).is_some_and(|v| !v.is_object()) {
            return refuse("invalid_routing_overrides");
        }
    }
    if let Some(pins) = r["pin"].as_object() {
        if pins
            .values()
            .any(|p| !p.as_str().is_some_and(|s| s.contains('/')))
        {
            return refuse("invalid_routing_overrides");
        }
    }
    if let Some(enabled) = r["enabled"].as_object() {
        for v in enabled.values() {
            strings(v)?;
        }
    }
    if let Some(models) = r["models"].as_object() {
        for engine in models.values() {
            if !engine.is_object() {
                return refuse("invalid_routing_overrides");
            }
            if let Some(auto) = engine.get("auto") {
                for model in strings(auto)? {
                    if !valid_tiers(&engine["tiers"][&model]) {
                        return refuse("policy_missing");
                    }
                }
            }
            if let Some(tiers) = engine["tiers"].as_object() {
                if tiers.values().any(|v| !valid_tiers(v)) {
                    return refuse("invalid_routing_overrides");
                }
            }
        }
    }
    Ok(())
}
/// Resolve an approved command without executing it. Relative paths containing a
/// slash never resolve against a project; bare names use absolute PATH entries.
pub fn executable_path(command: &Path) -> CoreResult<PathBuf> {
    let candidates = if command.is_absolute() {
        vec![command.to_owned()]
    } else if command.components().count() == 1 {
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .filter(|p| {
                p.is_absolute()
                    && !p
                        .components()
                        .any(|c| matches!(c, std::path::Component::ParentDir))
            })
            .map(|p| p.join(command))
            .collect()
    } else {
        return refuse("native_route_executable_unavailable");
    };
    for path in candidates {
        let Ok(real) = path.canonicalize() else {
            continue;
        };
        if !real.is_file() {
            continue;
        }
        let Ok(c) = std::ffi::CString::new(real.as_os_str().as_encoded_bytes()) else {
            continue;
        };
        if unsafe { libc::access(c.as_ptr(), libc::X_OK) } == 0 {
            return Ok(real);
        }
    }
    refuse("native_route_executable_unavailable")
}

/// Read-only production route provider. No discovery subprocess, login, seeding,
/// quota inference or model calls occur here. Unsupported policy fails closed.
pub struct ConfiguredRoutes {
    pub environment: RouteEnvironment,
    pub relay: PathBuf,
}
impl ConfiguredRoutes {
    fn registered(&self, field: impl Fn(&Value) -> String) -> Vec<String> {
        read(
            &self.environment.config.join("cortex/connections.json"),
            true,
        )
        .ok()
        .flatten()
        .and_then(|r| {
            r["connections"]
                .as_object()
                .map(|m| m.values().map(&field).collect())
        })
        .unwrap_or_default()
    }
    fn session(
        &self,
        root: &Path,
        selected: SelectedRoute,
    ) -> CoreResult<crate::native_sessions::SessionRoute> {
        let key = super::native_runtime::key(root)?;
        let launch = selected.session_launch(root)?;
        let relay = executable_path(&self.relay)?;
        Ok(crate::native_sessions::SessionRoute {
            launch, relay, broker_parent:key.directory(),connection:selected.connection().into(),
            connection_limit:selected.connection_limit(),model:selected.model().into(),parameters:selected.parameters.clone(),
            instructions:"Use the channel-served tools. Only the Rust runtime owns lifecycle, review, verification and completion.".into(),
            deadline:std::time::Duration::from_secs(30),
        })
    }
}
/// Python numbers are exact integers; never round a preserved one into a float.
fn exact_integers(bytes: &[u8]) -> bool {
    let (mut i, mut quoted, mut escape) = (0, false, false);
    while i < bytes.len() {
        let b = bytes[i];
        i += 1;
        if quoted {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else if b == b'"' {
            quoted = true;
        } else if b == b'-' || b.is_ascii_digit() {
            let start = i - 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || b".eE+-".contains(&bytes[i])) {
                i += 1;
            }
            let n = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
            if !n.contains(['.', 'e', 'E'])
                && n.parse::<i64>().is_err()
                && n.parse::<u64>().is_err()
            {
                return false;
            }
        }
    }
    true
}
/// Python `_apply_language_selection` before a task: the user-wide default is
/// rewritten as Python's sorted compact `defaults.json` (normalized ceiling) under
/// the shared `connections.lock`, after the same registry validation. The read
/// happens inside the lock; a failure changes nothing and is never retried.
pub(crate) fn persist_user_language(
    environment: &RouteEnvironment,
    language: Option<&str>,
) -> CoreResult<()> {
    persist_user_defaults(environment, |defaults| {
        defaults["language"] = json!(language);
        Ok(())
    })
}
/// Python `_apply_pre_task_selection`: the choice becomes the normal user-default
/// route (`route_id`, `route_connection_id`, `route_model`, `parameters`) and any
/// portable `coordinator` default is dropped. Other defaults and the normalized
/// ceiling are kept; the write shares `persist_user_language`'s lock and form.
pub(crate) fn persist_user_route(
    environment: &RouteEnvironment,
    selected: &SelectedRoute,
    parameters: &BTreeMap<String, String>,
) -> CoreResult<()> {
    persist_user_defaults(environment, |defaults| {
        let map = defaults
            .as_object_mut()
            .ok_or(CoreError::Refused("invalid_user_defaults"))?;
        map.remove("coordinator");
        map.insert("route_id".into(), json!(selected.route_id()));
        map.insert("route_connection_id".into(), json!(selected.connection()));
        map.insert("route_model".into(), json!(selected.model()));
        map.insert("parameters".into(), json!(parameters));
        Ok(())
    })
}
fn persist_user_defaults(
    environment: &RouteEnvironment,
    update: impl FnOnce(&mut Value) -> CoreResult<()>,
) -> CoreResult<()> {
    use cortex_core::store::locks;
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let folder = environment.config.join("cortex");
    locks::private_directory(&folder, false)?;
    let _lock = locks::AccessLock::acquire(
        &folder.join("connections.lock"),
        true,
        true,
        std::time::Duration::from_secs(5),
    )?;
    let registry = read(&folder.join("connections.json"), true)?;
    crate::config_read_model::parse_connections(registry.as_ref()).map_err(CoreError::Refused)?;
    let path = folder.join("defaults.json");
    let before = read_bytes(&path, true)?;
    if before.as_deref().is_some_and(|b| !exact_integers(b)) {
        return refuse("native_defaults_integer_unsupported");
    }
    let (mut defaults, ceiling, _) = crate::config_read_model::defaults_document(before.as_deref())
        .map_err(|_| CoreError::Refused("invalid_user_defaults"))?;
    update(&mut defaults)?;
    let body =
        crate::config_read_model::python_json(&json!({"defaults":defaults,"ceiling":ceiling}));
    let temporary = folder.join(format!(".defaults.json.{}.native", std::process::id()));
    let result = (|| -> CoreResult<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&temporary)?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)?;
        locks::sync_directory(&folder)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}
impl crate::native_runtime::NativeRoutes for ConfiguredRoutes {
    fn persist_default_language(&self, language: Option<&str>) -> CoreResult<bool> {
        persist_user_language(&self.environment, language)?;
        Ok(true)
    }
    fn persist_default_route(
        &self,
        root: &Path,
        previous: &Value,
        settings: &Value,
        option: &str,
    ) -> CoreResult<bool> {
        let key = super::native_runtime::key(root)?;
        let resolver = RouteResolver::load(root, &key.directory(), self.environment.clone())?;
        let selected = resolver.task_coordinator(settings)?;
        let parameters = if matches!(option, "model" | "provider") {
            // A host echo of the saved route is not a change (Python keeps the
            // configured values); a real model change keeps only the resolved
            // effort, not the previous model's other dependent parameters. Like
            // Python `_default_selection_from_user_defaults`, only a saved
            // `route_id` or a portable coordinator counts as a saved route.
            let d = &resolver.defaults["defaults"];
            let falsy = |v: &Value| match v {
                Value::Null => true,
                Value::Bool(b) => !b,
                Value::Number(n) => n.as_f64() == Some(0.0),
                Value::String(s) => s.is_empty(),
                Value::Array(a) => a.is_empty(),
                Value::Object(o) => o.is_empty(),
            };
            let saved = (!d["coordinator"].is_null() && falsy(&d["route_id"]))
                || d["route_id"].as_str().is_some_and(|s| !s.is_empty());
            if saved
                && resolver
                    .task_coordinator(previous)
                    .is_ok_and(|current| current.route_id() == selected.route_id())
            {
                return Ok(true);
            }
            selected.parameters.get("thought_level").map_or_else(BTreeMap::new, |effort| {
                BTreeMap::from([("thought_level".to_owned(), effort.clone())])
            })
        } else {
            selected.parameters.clone()
        };
        persist_user_route(&self.environment, &selected, &parameters)?;
        Ok(true)
    }
    fn default_language(&self) -> Option<String> {
        let view = crate::config_read_model::ConfigurationReader::new(self.environment.clone())
            .ok()?
            .user_defaults_management_observation()
            .ok()?;
        view["result"]["defaults"]["language"]
            .as_str()
            .filter(|v| matches!(*v, "en" | "ru"))
            .map(str::to_owned)
    }
    fn worker_route(
        &self,
        root: &Path,
        role: &str,
        node: &Value,
        evidence: &cortex_core::routing::Evidence,
        escalated: bool,
        previous: Option<&str>,
    ) -> CoreResult<crate::native_sessions::SessionRoute> {
        let key = super::native_runtime::key(root)?;
        let resolver = RouteResolver::load(root, &key.directory(), self.environment.clone())?;
        self.session(
            root,
            resolver.choose(role, node, evidence, escalated, previous)?,
        )
    }
    fn worker_selection(
        &self,
        root: &Path,
        role: &str,
        node: &Value,
        evidence: &cortex_core::routing::Evidence,
        escalated: bool,
        previous: Option<&str>,
        context: &mut WorkerRouteContext,
    ) -> CoreResult<(crate::native_sessions::SessionRoute, Value)> {
        let key = super::native_runtime::key(root)?;
        let resolver = RouteResolver::load(root, &key.directory(), self.environment.clone())?;
        let selected =
            resolver.choose_with_context(role, node, evidence, escalated, previous, context)?;
        let mut decision = selected.decision.clone();
        decision["route_engine"] = json!(selected.engine());
        Ok((self.session(root, selected)?, decision))
    }
    fn menu(&self, root: &Path, evidence: &cortex_core::routing::Evidence) -> CoreResult<Value> {
        let key = super::native_runtime::key(root)?;
        RouteResolver::load(root, &key.directory(), self.environment.clone())?.menu(evidence)
    }
    fn previews(
        &self,
        root: &Path,
        plan: &Value,
        evidence: &cortex_core::routing::Evidence,
    ) -> CoreResult<Value> {
        let key = super::native_runtime::key(root)?;
        RouteResolver::load(root, &key.directory(), self.environment.clone())?
            .previews(plan, evidence)
    }
    fn mcp_registry(&self) -> CoreResult<crate::native_mcp::Registry> {
        crate::native_mcp::Registry::load(&self.environment)
    }
    fn mcp_registry_for(&self, root: &Path) -> CoreResult<crate::native_mcp::Registry> {
        crate::native_mcp::Registry::load_for_project(&self.environment, root)
    }
    /// Python `build_config_options` provider/model/thought_level options over
    /// the qualified, launchable owner routes (see `presentation`).
    fn options(&self, root: &Path, settings: &Value) -> Value {
        let read = || -> CoreResult<Value> {
            let key = super::native_runtime::key(root)?;
            let resolver = RouteResolver::load(root, &key.directory(), self.environment.clone())?;
            Ok(json!(resolver.view(root, settings).options))
        };
        read().unwrap_or_else(|_| json!([]))
    }
    fn sprint_route(&self, root: &Path, complexity: &str) -> CoreResult<Option<Value>> {
        let key = super::native_runtime::key(root)?;
        RouteResolver::load(root, &key.directory(), self.environment.clone())?.sprint_route(complexity)
    }
    fn select(&self, root: &Path, settings: &Value, id: &str, value: &str) -> CoreResult<Value> {
        let key = super::native_runtime::key(root)?;
        RouteResolver::load(root, &key.directory(), self.environment.clone())?
            .select(root, settings, id, value)
    }
    fn listing(&self, root: &Path) -> Vec<Value> {
        super::native_runtime::key(root)
            .and_then(|key| RouteResolver::load(root, &key.directory(), self.environment.clone()))
            .map(|r| r.listing(root))
            .unwrap_or_default()
    }
    fn connection_states(&self) -> Vec<String> {
        self.registered(|c| c["state"].as_str().unwrap_or_default().to_owned())
    }
    fn connection_ids(&self) -> Vec<String> {
        self.registered(|c| c["id"].as_str().unwrap_or_default().to_owned())
    }
    fn authentication_observed_after(
        &self,
        root: &Path,
        connection: &str,
        after: &str,
    ) -> CoreResult<bool> {
        let key = super::native_runtime::key(root)?;
        let current = RouteResolver::load(root, &key.directory(), self.environment.clone())?;
        Ok(current.candidates.iter().any(|c| {
            c.connection_id() == connection
                && c.entry["evidence"]["configured"]["source"] == "discovery"
                && c.entry["evidence"]["configured"]["observed_at"]
                    .as_str()
                    .is_some_and(|at| at > after)
        }))
    }
    fn automatic_coordinator_health(
        &self,
        root: &Path,
        route: &crate::native_sessions::SessionRoute,
    ) -> CoreResult<()> {
        let key = super::native_runtime::key(root)?;
        let current = RouteResolver::load(root, &key.directory(), self.environment.clone())?;
        let candidate = current
            .candidates
            .iter()
            .find(|c| c.connection_id() == route.connection && c.model() == route.model)
            .ok_or(CoreError::Refused("native_task_route_unavailable"))?;
        if current.usage(candidate)?.is_some_and(|used| used >= 100.0) {
            return refuse("engine_quota_exhausted");
        }
        Ok(())
    }
    fn refresh_catalog(&self, connection: &str) -> bool {
        crate::machine::config_action::catalog_refresh::observe_session_refresh(
            &self.environment,
            connection,
        )
        .is_ok_and(|row| row.is_some_and(|r| r["success"] == true))
    }
    fn task_route(
        &self,
        root: &Path,
        settings: &Value,
    ) -> CoreResult<crate::native_sessions::SessionRoute> {
        // An explicit route choice does not need a resolvable owner default.
        let key = super::native_runtime::key(root)?;
        let resolver = RouteResolver::load(root, &key.directory(), self.environment.clone())?;
        self.session(root, resolver.task_coordinator(settings)?)
    }
    fn fallback_coordinator(
        &self,
        root: &Path,
        stored: &Value,
    ) -> CoreResult<crate::native_sessions::SessionRoute> {
        let key = super::native_runtime::key(root)?;
        let resolver = RouteResolver::load(root, &key.directory(), self.environment.clone())?;
        let mut same_model = stored.clone();
        if same_model["model"].as_str().is_some_and(|m| !m.is_empty()) {
            // Same model, effort moved to the nearest level its range allows.
            same_model["effort"] = json!("");
            if let Ok(selected) = resolver.task_coordinator(&same_model) {
                return self.session(root, selected);
            }
        }
        self.session(root, resolver.coordinator()?)
    }
    fn alternate_coordinator_pin(
        &self,
        root: &Path,
        current: &crate::native_sessions::SessionRoute,
    ) -> CoreResult<Option<Value>> {
        let key = super::native_runtime::key(root)?;
        let resolver = RouteResolver::load(root, &key.directory(), self.environment.clone())?;
        resolver.alternate_pin(&current.connection, &current.model)
    }
    fn coordinator_pin(&self, root: &Path, settings: &Value) -> CoreResult<Option<Value>> {
        let key = super::native_runtime::key(root)?;
        let resolver = RouteResolver::load(root, &key.directory(), self.environment.clone())?;
        Ok(Some(resolver.pin(settings)?))
    }
    fn pinned_route(
        &self,
        root: &Path,
        pin: &Value,
        effort: Option<&str>,
    ) -> CoreResult<crate::native_sessions::SessionRoute> {
        let key = super::native_runtime::key(root)?;
        let resolver = RouteResolver::load(root, &key.directory(), self.environment.clone())?;
        self.session(root, resolver.pinned(pin, effort)?)
    }
    fn route(&self, root: &Path, role: &str) -> CoreResult<crate::native_sessions::SessionRoute> {
        let key = super::native_runtime::key(root)?;
        let resolver = RouteResolver::load(root, &key.directory(), self.environment.clone())?;
        let selected = if role == "orchestrator" {
            resolver.coordinator()?
        } else {
            resolver.worker(role)?
        };
        let launch = selected.session_launch(root)?;
        let relay = executable_path(&self.relay)?;
        Ok(crate::native_sessions::SessionRoute {
            launch, relay, broker_parent: key.directory(),
            connection: selected.connection().into(),
            connection_limit: selected.connection_limit(),
            model: selected.model().into(),
            parameters: selected.parameters.clone(),
            instructions: "Use the channel-served tools. Only the Rust runtime owns lifecycle, review, verification and completion.".into(),
            deadline: std::time::Duration::from_secs(30),
        })
    }
}
fn valid_tiers(v: &Value) -> bool {
    let values = if let Some(s) = v.as_str() {
        vec![s]
    } else {
        match v.as_array() {
            Some(a) if !a.is_empty() => {
                match a.iter().map(Value::as_str).collect::<Option<Vec<_>>>() {
                    Some(v) => v,
                    None => return false,
                }
            }
            _ => return false,
        }
    };
    values
        .iter()
        .all(|s| ["flagship", "deep", "standard", "light"].contains(s))
}

#[cfg(test)]
mod route_identity_tests {
    use super::*;
    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect()
    }
    /// Pairs the Codex engine advertised before and after the worker permission profile existed.
    fn codex(with_profile: bool) -> Vec<(String, String)> {
        let mut list = vec![
            ("mode", "agent"),
            ("mode", "agent-full-access"),
            ("mode", "read-only"),
            ("mode", "workspace-write"),
            ("model_config", "off"),
            ("model_config", "on"),
            ("thought_level", "high"),
            ("thought_level", "low"),
            ("thought_level", "max"),
            ("thought_level", "medium"),
            ("thought_level", "ultra"),
            ("thought_level", "xhigh"),
        ];
        if with_profile {
            list.insert(2, ("mode", "permission-profile:cortex-workspace"));
        }
        pairs(&list)
    }
    const GOLDEN: &str = "xr_5f46d413ac842d86e4b25f1a";
    const WITH_PROFILE_BUILD: &str = "xr_03485bd94a43755028acf08a";
    fn id(p: Vec<(String, String)>) -> String {
        catalog_route_id(&json!("codex-acp"), &json!("cn_8944f22487be"), &json!("gpt-6.2-sol"), p)
    }
    #[test]
    fn a_route_id_ignores_the_cortex_launch_profile_mode() {
        assert_eq!(id(codex(false)), GOLDEN);
        assert_eq!(id(codex(true)), GOLDEN);
        // Reordered input and other launch-independent pairs still change the id.
        let mut reversed = codex(true);
        reversed.reverse();
        assert_eq!(id(reversed), GOLDEN);
        let mut other = codex(false);
        other.push(("mode".into(), "permission-profile:other".into()));
        assert_ne!(id(other), GOLDEN);
    }
    #[test]
    fn an_id_derived_with_the_profile_mode_is_accepted_and_normalised() {
        let (e, c, m) = (json!("codex-acp"), json!("cn_8944f22487be"), json!("gpt-6.2-sol"));
        for stored in [GOLDEN, WITH_PROFILE_BUILD] {
            assert_eq!(
                accepted_route_id(&json!(stored), &e, &c, &m, codex(true)).as_deref(),
                Some(GOLDEN)
            );
        }
        assert_eq!(accepted_route_id(&json!("xr_000000000000000000000000"), &e, &c, &m, codex(true)), None);
    }
    #[test]
    fn retained_snapshots_map_an_old_id_to_its_model() {
        let state = std::env::temp_dir().join(format!("route-history-{}", std::process::id()));
        let directory = state.join("cortex/catalog/cn_8944f22487be");
        std::fs::create_dir_all(&directory).unwrap();
        let entry = |id: &str| json!({"route":{"id":id,"model":"gpt-6.2-sol","connection_id":"cn_8944f22487be"}});
        std::fs::write(
            directory.join("cs_000000000001.json"),
            json!({"entries":[entry("xr_aaaaaaaaaaaaaaaaaaaaaaaa")]}).to_string(),
        )
        .unwrap();
        let history = retained_route_ids(&state, "cn_8944f22487be");
        assert!(history["gpt-6.2-sol"].contains("xr_aaaaaaaaaaaaaaaaaaaaaaaa"));
        let _ = std::fs::remove_dir_all(&state);
    }
}
