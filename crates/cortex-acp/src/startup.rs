//! Startup-only host operations. Never called by ACP/model arguments.
use cortex_core::{store::locks, CoreError, CoreResult};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use workspace_service::{config_read_model::ConfigurationReader, native_routes::RouteEnvironment};

const CONTRACT_VERSION: i64 = 1;
const MAX_JSON: u64 = 4 * 1024 * 1024;
const PRESET: &str = include_str!("../../../policy/model-settings-default.json");

/// Resolve raw host HOME/XDG roots exactly once for this compiled agent channel.
/// Runtime environment variables cannot switch a compiled binary's channel.
/// Explicit routing overrides and native-account HOME remain owner-selected.
pub fn channel_environment(mut environment: RouteEnvironment) -> RouteEnvironment {
    let channel = workspace_protocol::channel::current();
    if channel.is_dev() {
        environment.config = channel.config_dir(&environment.config);
        environment.state = channel.state_dir(&environment.state).join("state");
    }
    environment
}

/// Project configuration identity matches the compiled incoming agent.
pub fn project_store_name() -> &'static str {
    if workspace_protocol::channel::current().is_dev() {
        ".cortex-dev"
    } else {
        ".cortex"
    }
}

/// The operator can select a relocated payload, never a protocol/model caller.
pub fn runtime_root() -> PathBuf {
    std::env::var_os("CORTEX_ACP_RUNTIME_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}
fn failure(code: &'static str) -> CoreError {
    CoreError::Refused(code)
}
fn load_manifest(path: &Path) -> Result<Value, String> {
    let raw = fs::read_to_string(path).map_err(|e| {
        format!(
            "pinned dependency manifest is unreadable at {}: {e}",
            path.display()
        )
    })?;
    let v: Value = serde_json::from_str(&raw).map_err(|e| {
        format!(
            "pinned dependency manifest at {} is not valid JSON: {e}",
            path.display()
        )
    })?;
    if !v.is_object() || v["schema_version"] != 1 {
        return Err(format!(
            "pinned dependency manifest at {} has an unrecognized schema_version.",
            path.display()
        ));
    }
    Ok(v)
}
pub(crate) fn repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::String(s) => {
            let quote = if s.contains('\'') && !s.contains('"') {
                '"'
            } else {
                '\''
            };
            let mut out = String::new();
            out.push(quote);
            for c in s.chars() {
                match c {
                    '\\' => out.push_str("\\\\"),
                    '\n' => out.push_str("\\n"),
                    '\r' => out.push_str("\\r"),
                    '\t' => out.push_str("\\t"),
                    c if c == quote => {
                        out.push('\\');
                        out.push(c);
                    }
                    c if c.is_control() => out.push_str(&format!("\\x{:02x}", c as u32)),
                    c => out.push(c),
                }
            }
            out.push(quote);
            out
        }
        _ => v.to_string(),
    }
}
fn findings(manifest: &Value) -> Vec<String> {
    let mut errors = vec![];
    if manifest["portable_contract_version"] != CONTRACT_VERSION {
        errors.push(format!(
            "cortex_acp: pinned dependency portable_contract_version mismatch: pinned {} does not match the supported portable contract version {CONTRACT_VERSION}. Repair: The checkout is inconsistent with its own pinned manifest; reinstall/resync this exact revision rather than editing either value by hand.",
            repr(&manifest["portable_contract_version"])));
    }
    errors
}
/// Only the portable contract version applies to this native entry.
pub async fn verify_pinned(root: &Path) -> Result<(), Vec<String>> {
    let manifest = load_manifest(&root.join("pinned-dependencies.json"))
        .map_err(|e| vec![format!("cortex_acp: pinned dependency manifest error: {e}")])?;
    let errors = findings(&manifest);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Python `datetime.now(timezone.utc).isoformat()`: microseconds, omitted
/// entirely when they are exactly zero.
pub(crate) fn python_now() -> String {
    python_isoformat(&cortex_core::now())
}
fn python_isoformat(rfc3339_micros: &str) -> String {
    rfc3339_micros.replacen(".000000+", "+", 1)
}
fn create_absent(path: &Path, bytes: &[u8]) -> CoreResult<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = path.with_file_name(format!(
        ".startup-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)?;
    let result = (|| -> CoreResult<()> {
        file.write_all(bytes)?;
        file.sync_all()?;
        match fs::hard_link(&temporary, path) {
            Ok(()) => locks::sync_directory(path.parent().unwrap()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e.into()),
        }
    })();
    fs::remove_file(temporary)?;
    result
}
/// The private owner configuration root and its ignore file: Python
/// `registry.ensure_private_dir(root, root=root)`, never replacing owner bytes.
/// Missing ancestors are created 0755 (before umask) and are never changed;
/// the root itself is created 0700 or repaired to 0700 (an existing 0755
/// root included), then must be owner-only and owned by this user. A symlink
/// or non-directory component, or a project marker in an ancestor other than
/// `home`, refuses.
pub(crate) fn owner_root(folder: &Path, home: &Path) -> CoreResult<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    if !folder.is_absolute() {
        return Err(failure("relative_storage_root"));
    }
    let parts: Vec<_> = folder.components().collect();
    // `_assert_not_nested_in_project_workspace`: every ancestor below `/`.
    let mut current = PathBuf::from(parts[0].as_os_str());
    for part in parts.iter().take(parts.len().saturating_sub(1)).skip(1) {
        current.push(part);
        if current != home
            && [".git", ".cortex", ".cortex-dev"]
                .iter()
                .any(|marker| current.join(marker).symlink_metadata().is_ok())
        {
            return Err(failure("connection_storage_under_project"));
        }
    }
    let mut current = PathBuf::from(parts[0].as_os_str());
    for (index, part) in parts.iter().enumerate().skip(1) {
        current.push(part);
        let managed = index + 1 >= parts.len();
        let info = match current.symlink_metadata() {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Another process starting at the same time may create it first;
                // its folder gets the same checks as one that already existed.
                match fs::DirBuilder::new()
                    .mode(if managed { 0o700 } else { 0o755 })
                    .create(&current)
                {
                    Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(e.into()),
                    _ => {}
                }
                current.symlink_metadata()?
            }
            other => other?,
        };
        if info.file_type().is_symlink() || !info.is_dir() {
            return Err(failure("unsafe_storage"));
        }
        if managed {
            fs::set_permissions(&current, fs::Permissions::from_mode(0o700))?;
            let info = current.symlink_metadata()?;
            if info.uid() != unsafe { libc::getuid() } || info.mode() & 0o077 != 0 {
                return Err(failure("unsafe_storage"));
            }
        }
    }
    // `_ensure_root_gitignore`: exactly `*\n`, never overwritten.
    let ignore = folder.join(".gitignore");
    create_absent(&ignore, b"*\n")?;
    let mut text = Vec::new();
    locks::open_private(&ignore, false, true)?
        .take(3)
        .read_to_end(&mut text)?;
    if text != b"*\n" {
        return Err(failure("unsafe_storage"));
    }
    Ok(())
}
/// Absent-only public presets and a default profile for matching working files.
/// Owner bytes, including malformed files and symlinks, are never replaced.
pub fn seed_defaults(environment: &RouteEnvironment) -> CoreResult<()> {
    // Reuse the shared root validator before any creation.
    ConfigurationReader::new(environment.clone())?;
    let template: Value = serde_json::from_str(PRESET)?;
    let folder = environment.config.join("cortex");
    owner_root(&folder, &environment.home)?;
    // Python `_owner_config_lock`: a blocking, unbounded `flock`.
    let _lock =
        locks::AccessLock::acquire(&folder.join("connections.lock"), true, true, Duration::MAX)?;
    let mut defaults = json!({"coordinator":template["coordinator"]});
    if let Some(language) = template.get("language") {
        defaults["language"] = language.clone();
    }
    for (name, payload) in [
        ("routing.json", template["routing"].clone()),
        ("defaults.json", json!({"defaults":defaults})),
        (
            "mcp-servers.json",
            json!({"version":1,"servers":template.get("mcp_servers").cloned().unwrap_or(json!({}))}),
        ),
    ] {
        let path = folder.join(name);
        if path.symlink_metadata().is_ok() {
            continue;
        }
        let mut bytes = serde_json::to_vec_pretty(&payload)?;
        bytes.push(b'\n');
        create_absent(&path, &bytes)?;
    }
    let profile = folder.join("routing-profiles.json");
    if profile.symlink_metadata().is_err() {
        let mut sections = [None, None, None, None];
        for (index, name) in [
            "routing.json",
            "defaults.json",
            "mcp-servers.json",
            "connections.json",
        ]
        .iter()
        .enumerate()
        {
            let path = folder.join(name);
            if path.symlink_metadata().is_ok() {
                let mut bytes = Vec::new();
                locks::open_private(&path, false, true)?
                    .take(MAX_JSON + 1)
                    .read_to_end(&mut bytes)?;
                if bytes.len() as u64 > MAX_JSON {
                    return Err(failure("native_route_json_too_large"));
                }
                sections[index] = Some(bytes);
            }
        }
        if let Some(bytes) =
            workspace_service::native_runtime::seed_default_profile(&sections, &python_now())?
        {
            create_absent(&profile, &bytes)?;
        }
    }
    Ok(())
}
/// Python `follow_approved_adapters` prints each committed change itself.
pub(crate) fn follow_and_report(
    folder: &Path,
    host: &crate::adapters::AdapterHost,
) -> CoreResult<()> {
    for event in crate::adapters::follow_approved_adapters(folder, host)? {
        eprintln!(
            "cortex: approved adapter {} {} -> {}",
            event.engine, event.old_version, event.new_version
        );
    }
    Ok(())
}
/// Python `_ensure_first_run_configuration`: never fails startup. The
/// background pass is detached like Python's daemon thread and ends with the
/// process; the returned handle lets a host-local caller wait for it.
pub fn first_run(environment: &RouteEnvironment) -> Option<std::thread::JoinHandle<()>> {
    if seed_defaults(environment).is_err() {
        eprintln!("cortex: native routing defaults deferred");
    }
    if !off("CORTEX_ENGINE_FOLLOW") {
        let host = crate::adapters::AdapterHost::from_process(&environment.home, &runtime_root());
        if follow_and_report(&environment.config.join("cortex"), &host).is_err() {
            eprintln!("cortex: approved adapter selection deferred");
        }
    }
    if off("CORTEX_ENGINE_UPDATE") {
        return None;
    }
    let pass = crate::automatic::FirstRun::from_process(environment, &runtime_root());
    std::thread::Builder::new()
        .name("cortex-first-run".into())
        .spawn(move || pass.run())
        .ok()
}
pub(crate) fn off(key: &str) -> bool {
    std::env::var(key).is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no"
        )
    })
}

fn private_json(path: &Path) -> CoreResult<Value> {
    let file = locks::open_private(path, false, true)?;
    let mut bytes = Vec::new();
    file.take(MAX_JSON + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > MAX_JSON {
        return Err(failure("native_route_json_too_large"));
    }
    Ok(serde_json::from_slice(&bytes)?)
}
/// Lookup descriptions only after the shared reader has validated every snapshot,
/// connection identity, route fingerprint and newest-file selection.
fn cached_metadata(
    environment: &RouteEnvironment,
    connections: &Value,
) -> CoreResult<BTreeMap<String, Value>> {
    let mut descriptions = BTreeMap::new();
    for id in connections["connections"].as_object().unwrap().keys() {
        let folder = environment.state.join("cortex/catalog").join(id);
        if !folder.exists() {
            continue;
        }
        let mut files = fs::read_dir(folder)?.collect::<Result<Vec<_>, _>>()?;
        files.retain(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("cs_") && n.ends_with(".json"))
        });
        files.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));
        for file in files {
            let doc = private_json(&file.path())?;
            let entries = doc["entries"]
                .as_array()
                .ok_or_else(|| failure("invalid_catalog_snapshot"))?;
            if entries.is_empty()
                && !doc["unavailable_reason"].is_null()
                && doc["unavailable_reason"] != "no_models_observed"
            {
                continue;
            }
            for e in entries {
                if let Some(id) = e["route"]["id"].as_str() {
                    descriptions.insert(
                        id.into(),
                        json!({
                            "description":e["model_description"],
                            "parameters":e["route"]["supported_parameters"]
                        }),
                    );
                }
            }
            break;
        }
    }
    Ok(descriptions)
}
/// Paseo selector value -> ordered, unique thought levels. No models are seeded.
/// File/native-marker validation reuses the shared native refresh; keyring cannot
/// be qualified here and refuses instead of publishing a false empty catalogue.
pub fn model_efforts(environment: &RouteEnvironment) -> CoreResult<Value> {
    let reader = ConfigurationReader::new(environment.clone())?;
    let connections = reader.connections()?;
    if connections["connections"]
        .as_object()
        .unwrap()
        .values()
        .any(|r| r["credential_ref"]["kind"] == "os_keyring")
    {
        return Err(failure("native_model_efforts_keyring_unavailable"));
    }
    let observation = reader.connection_management()?;
    let ids: Vec<_> = connections["connections"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    if !ids.is_empty() {
        workspace_service::config_refresh::refresh_connections(
            environment,
            &ids,
            observation["revision"].as_str().unwrap(),
            &cortex_core::now(),
            Duration::from_secs(1),
        )?;
    }
    let catalog = reader.catalog_selector(None)?;
    let descriptions = cached_metadata(environment, &connections)?;
    efforts(&catalog, &descriptions)
}
fn efforts(catalog: &Value, descriptions: &BTreeMap<String, Value>) -> CoreResult<Value> {
    let entries = catalog["entries"]
        .as_array()
        .ok_or_else(|| failure("invalid_catalog_snapshot"))?;
    let routes = catalog["routes"]["routes"]
        .as_array()
        .ok_or_else(|| failure("invalid_catalog_snapshot"))?;
    let model_id =
        regex::Regex::new(r"^[a-z][a-z0-9.]*(?:-[a-z0-9.]+)+(?:\[[a-z0-9]+\])?$").unwrap();
    let entries: Vec<_> = entries
        .iter()
        .filter(|entry| {
            routes
                .iter()
                .any(|r| r["route_id"] == entry["route_id"] && r["in_selector"] == true)
        })
        .collect();
    let mut selected = vec![];
    for entry in &entries {
        let id = entry["route_id"].as_str().unwrap();
        let mut model = entry["model"].as_str().unwrap();
        if model.eq_ignore_ascii_case("default") {
            let Some(d) = descriptions.get(id).and_then(|m| m["description"].as_str()) else {
                continue;
            };
            if !model_id.is_match(d)
                || entries
                    .iter()
                    .any(|e| e["connection_id"] == entry["connection_id"] && e["model"] == d)
            {
                continue;
            }
            model = d;
        }
        selected.push((entry, model));
    }
    let route_ids: BTreeSet<_> = selected
        .iter()
        .map(|(e, _)| e["route_id"].as_str().unwrap())
        .collect();
    let mut result = json!({});
    const STRENGTH: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];
    for (entry, model) in &selected {
        let key = if selected.iter().filter(|(_, m)| m == model).count() == 1
            && !route_ids.contains(model)
        {
            model
        } else {
            entry["route_id"].as_str().unwrap()
        };
        let raw = descriptions.get(entry["route_id"].as_str().unwrap());
        let values: Vec<_> = if let Some(pairs) = raw.and_then(|m| m["parameters"].as_array()) {
            // The settings read model filters an older engine_default spelling.
            // Python's ACP selector filters only the literal default value.
            pairs
                .iter()
                .filter(|p| p[0] == "thought_level")
                .filter_map(|p| p[1].as_str())
                .collect()
        } else {
            entry["efforts"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(Value::as_str)
                .collect()
        };
        let mut efforts: Vec<_> = values
            .into_iter()
            .filter(|v| !v.eq_ignore_ascii_case("default"))
            .collect();
        efforts.sort_by_key(|v| {
            (
                STRENGTH
                    .iter()
                    .position(|s| *s == v.to_ascii_lowercase())
                    .unwrap_or(STRENGTH.len()),
                v.to_ascii_lowercase(),
            )
        });
        efforts.dedup();
        result[key] = json!(efforts);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn timestamps_omit_a_zero_microsecond_like_python() {
        assert_eq!(
            python_isoformat("2026-11-14T23:00:00.000000+00:00"),
            "2026-11-14T23:00:00+00:00"
        );
        assert_eq!(
            python_isoformat("2026-11-14T23:00:00.000100+00:00"),
            "2026-11-14T23:00:00.000100+00:00"
        );
        assert!(
            regex::Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(\.\d{6})?\+00:00$")
                .unwrap()
                .is_match(&python_now())
        );
    }
    #[test]
    fn owner_root_repairs_modes_like_ensure_private_dir() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("cortex-owner-root-{}", std::process::id()));
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        fs::create_dir_all(root.join("existing")).unwrap();
        fs::set_permissions(root.join("existing"), fs::Permissions::from_mode(0o755)).unwrap();
        owner_root(&root.join("existing"), &root).unwrap();
        assert_eq!(mode(&root.join("existing")), 0o700);
        assert_eq!(fs::read(root.join("existing/.gitignore")).unwrap(), b"*\n");
        // Missing ancestors are ordinary directories; only the root is private.
        owner_root(&root.join("fresh/config/cortex"), &root).unwrap();
        assert_eq!(mode(&root.join("fresh/config/cortex")), 0o700);
        assert_eq!(mode(&root.join("fresh/config")) & 0o700, 0o700);
        std::os::unix::fs::symlink(root.join("existing"), root.join("link")).unwrap();
        assert!(owner_root(&root.join("link"), &root).is_err());
        fs::write(root.join("existing/.gitignore"), b"other").unwrap();
        assert!(owner_root(&root.join("existing"), &root).is_err());
        fs::create_dir_all(root.join("project/.git")).unwrap();
        assert!(matches!(
            owner_root(&root.join("project/config/cortex"), &root),
            Err(CoreError::Refused("connection_storage_under_project"))
        ));
        // The effective HOME may itself be a repository.
        owner_root(&root.join("project/config/cortex"), &root.join("project")).unwrap();
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn manifest_findings_are_ordered_and_fail_closed() {
        // Fields the native entry does not own are ignored, never enforced.
        assert!(
            findings(&json!({"retired_field":[99, 0],"portable_contract_version":1})).is_empty()
        );
        assert!(findings(&json!({"portable_contract_version":1})).is_empty());
        let errors = findings(&json!({"portable_contract_version":2}));
        assert_eq!(errors.len(), 1);
        assert!(errors[0].contains("portable_contract_version mismatch: pinned 2"));
        assert_eq!(findings(&json!({})).len(), 1);
    }
    #[test]
    fn efforts_use_selector_values_and_python_strength_order() {
        let catalog = json!({"entries":[
            {"route_id":"xr_a","connection_id":"cn_a","model":"shared","efforts":["high","default","low","high"]},
            {"route_id":"xr_b","connection_id":"cn_b","model":"shared","efforts":[]},
            {"route_id":"xr_c","connection_id":"cn_a","model":"unique","efforts":["zeta","medium"]},
            {"route_id":"xr_d","connection_id":"cn_a","model":"denied","efforts":["max"]}
        ],"routes":{"routes":[
            {"route_id":"xr_a","in_selector":true},{"route_id":"xr_b","in_selector":true},
            {"route_id":"xr_c","in_selector":true},{"route_id":"xr_d","in_selector":false}
        ]}});
        assert_eq!(
            efforts(&catalog, &BTreeMap::new()).unwrap(),
            json!({"xr_a":["low","high"],"xr_b":[],"unique":["medium","zeta"]})
        );
    }
}
