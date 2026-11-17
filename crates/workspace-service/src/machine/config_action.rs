//! Qualified native metadata, profile and preset actions. Other variants retain their provider.
use super::{native::*, project_config as reader, project_config_save as write};
use crate::config_read_model::connection_management_view;
#[path = "config_action_registry.rs"]
pub(super) mod registry;
#[path = "config_action_json.rs"]
pub(super) mod registry_json;
use serde_json::{json, Value};
use std::path::Path;
#[path = "config_apply.rs"]
mod apply;
#[path = "config_catalog_refresh.rs"]
pub(crate) mod catalog_refresh;
#[path = "config_connections.rs"]
mod connections;
#[path = "config_engine_install.rs"]
mod engine_install;
#[path = "config_install.rs"]
mod install;
#[path = "config_keyring.rs"]
mod keyring;
#[path = "config_opencode.rs"]
pub(crate) mod opencode_engine;
#[path = "config_preset.rs"]
mod preset;
#[path = "config_profiles.rs"]
mod profiles;

#[cfg(test)]
#[path = "config_action_tests.rs"]
mod tests;

pub(super) fn seed_default_profile(
    bytes: &[Option<Vec<u8>>; 4],
    now: &str,
) -> Result<Option<Vec<u8>>> {
    profiles::seed_default(bytes, now)
}

fn unavailable(message: &'static str) -> Refused {
    refuse("config_action_unavailable", message)
}
const ACTIONS: [&str; 7] = [
    "apply_default_preset",
    "reset_default_preset",
    "refresh",
    "connection_update",
    "install_engine",
    "onboard_opencode",
    "profile",
];
const PROFILE_VERBS: [&str; 7] = [
    "read",
    "create",
    "save",
    "update",
    "delete",
    "deactivate",
    "activate",
];
/// The retained wrapper's own refusals for a request it never dispatches:
/// an unknown action (an unhashable one fails its dict lookup with TypeError)
/// and an unknown profile verb, which the management API names with `repr`.
fn dispatch_refusal(params: &Value) -> Option<Refused> {
    let action = &params["action"];
    if action.is_array() || action.is_object() {
        return Some(refuse("machine_tool_failed", "TypeError"));
    }
    if !action.as_str().is_some_and(|a| ACTIONS.contains(&a)) {
        return Some(refuse(
            "unsupported_action",
            "This machine does not support that yet.",
        ));
    }
    let verb = &params["verb"];
    if action != "profile" || verb.as_str().is_some_and(|v| PROFILE_VERBS.contains(&v)) {
        return None;
    }
    if verb.is_array() || verb.is_object() {
        return Some(refuse("machine_tool_failed", "TypeError"));
    }
    // Floats and non-ASCII text have no exact Python `repr` here: the refusal
    // names the verb in its JSON form instead of delegating.
    let shown = python_scalar_repr(verb).unwrap_or_else(|| verb.to_string());
    Some(refuse(
        "unsupported_action",
        format!("Action {shown} is not supported for surface 'routing_profiles'."),
    ))
}
pub(super) fn action(
    config: &crate::Config,
    params: &Value,
    deadline: std::time::Instant,
) -> Result<Value> {
    object_params(params)?;
    if let Some(refused) = dispatch_refusal(params) {
        return Err(refused);
    }
    match params["action"].as_str() {
        Some("connection_update") => update(config, params, &cortex_core::now(), write::replace),
        Some("reset_default_preset") => preset::reset(config, params),
        Some("apply_default_preset") => apply::apply(config),
        Some("refresh") => management_refresh(config),
        Some("install_engine" | "onboard_opencode") if install::refusal(params).is_some() => {
            Err(install::refusal(params).unwrap())
        }
        Some("install_engine") => management_install(config, params, deadline),
        Some("onboard_opencode") => management_onboard_opencode(config, params),
        Some("profile") => management_profile(config, params),
        _ => Err(refuse(
            "unsupported_action",
            "This machine does not support that yet.",
        )),
    }
}
pub(super) fn ensure_ignore(folder: &Path) -> Result<()> {
    use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt};
    // Serialize native first-use setup: a peer must not inspect our newly created,
    // not-yet-flushed ignore file. Registry commits still use the shared owner lock.
    static SETUP: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _setup = SETUP.lock().map_err(|_| {
        refuse(
            "storage_error",
            "Owner configuration storage refused the operation.",
        )
    })?;
    let path = folder.join(".gitignore");
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
    {
        Ok(mut f) => {
            if f.write_all(b"*\n").and_then(|_| f.sync_all()).is_err() {
                let _ = std::fs::remove_file(&path);
                return Err(refuse(
                    "storage_error",
                    "Owner configuration storage refused the operation.",
                ));
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if reader::read_plain(&path, true)?.as_deref() != Some(b"*\n") {
                return Err(refuse("unsafe_storage", format!("{} already exists with unexpected content; refusing to overwrite it (it must contain exactly '*').", path.display())));
            }
            Ok(())
        }
        Err(_) => Err(refuse(
            "storage_error",
            "Owner configuration storage refused the operation.",
        )),
    }
}
// --- management-API entry points (`machine::settings_api`) -----------------------
// The same native implementations with their retained-provider admission made
// explicit: a shape the retained provider alone could answer refuses here.

pub(super) fn management_profile(config: &crate::Config, p: &Value) -> Result<Value> {
    profiles::action(config, p)
}
pub(super) fn management_connection(config: &crate::Config, p: &Value) -> Result<Value> {
    update(config, p, &cortex_core::now(), write::replace)
}
pub(super) fn management_preset_snapshot(config: &crate::Config) -> Result<Value> {
    preset::snapshot(&reader::owner_folder(config)?).map(|(_, view)| view)
}
pub(super) fn management_preset_reset(config: &crate::Config, expected: &str) -> Result<Value> {
    preset::reset_expected(config, expected)
}
pub(super) fn management_preset_apply(config: &crate::Config) -> Result<Value> {
    apply::apply_management(config)
}
/// The registered connection records (normalized), for refresh admission.
pub(super) fn management_records(config: &crate::Config) -> Result<Value> {
    let folder = reader::owner_folder(config)?;
    let bytes = reader::read_plain(&folder.join("connections.json"), true)?;
    let input = registry_json::RegistryJson::parse(bytes.as_deref())?;
    registry::normalize(&input.value)
}
pub(super) fn management_refresh(config: &crate::Config) -> Result<Value> {
    if let Some(reply) = catalog_refresh::refresh_unlaunched(config)? {
        return Ok(reply);
    }
    // A launching producer publishes snapshots under the shared owner lock. An
    // existing lock is used as is (no new ignore file just to refresh); an absent
    // one is created together with the private root, as the first writer would.
    let folder = reader::owner_folder(config)?;
    if reader::read_plain(&folder.join("connections.lock"), true)?.is_none() {
        crate::local::private_tree(&folder)?;
        ensure_ignore(&folder)?;
        drop(write::owner_lock(&folder)?);
    }
    catalog_refresh::refresh(config)
}
pub(super) fn management_install(
    config: &crate::Config,
    p: &Value,
    deadline: std::time::Instant,
) -> Result<Value> {
    if let Some(refused) = install::refusal(p) {
        return Err(refused);
    }
    let host = engine_install::Host::from_config(config)?;
    // An installed OpenCode only changes its own discovery row (see below).
    if let Some(reason) = host.unprojected() {
        return Err(refuse(
            "config_action_unavailable",
            format!("Engine installation needs a retained-only check here ({reason}). Nothing was installed."),
        ));
    }
    let reply = engine_install::install(
        &host,
        p["engine"].as_str().unwrap(),
        engine_install::npm_deadline(deadline),
        &|command| crate::launch::runtime_environment(command, config),
    )?;
    Ok(reply)
}
/// `discover_connections`; an installed OpenCode binary's version is read by the
/// sandboxed probe.
pub(super) fn management_discovery(config: &crate::Config) -> Result<Value> {
    let host = engine_install::Host::from_config(config)?;
    if let Some(reason) = host.unprojected() {
        return Err(refuse(
            "config_action_unavailable",
            format!("Engine discovery needs a retained-only check here ({reason}). Nothing was changed."),
        ));
    }
    host.discovery()
}
/// `onboard_connection` after its engine, opt-in and revision-syntax checks:
/// an existing connection of the engine is returned unchanged; otherwise the
/// discovered, qualified adapter with a native login is registered referencing
/// the owner's native account, then its models are observed once.
pub(super) fn management_onboard(
    config: &crate::Config,
    engine: &str,
    expected: &str,
) -> Result<Value> {
    let folder = reader::owner_folder(config)?;
    crate::local::private_tree(&folder)?;
    ensure_ignore(&folder)?;
    let path = folder.join("connections.json");
    let entry = |registry: &Value, bytes: Option<&[u8]>, id: &str| {
        let view = connection_management_view(registry, &write::token(bytes), bytes.is_some());
        let connection = view["connections"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|c| c["id"] == id)
            .cloned()
            .unwrap_or(Value::Null);
        (view, connection)
    };
    {
        let _lock = write::owner_lock(&folder)?;
        let before = reader::read_plain(&path, true)?;
        write::check("connections", expected, &write::token(before.as_deref()))?;
        let input = registry_json::RegistryJson::parse(before.as_deref())?;
        let registry = registry::normalize(&input.value)?;
        let existing = input.value["connections"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(id, _)| id.clone())
            .find(|id| registry["connections"][id]["engine"] == engine);
        if let Some(id) = existing {
            let (mut view, connection) = entry(&registry, before.as_deref(), &id);
            view["changed"] = json!(false);
            view["connection"] = connection;
            return Ok(view);
        }
    }
    let host = engine_install::Host::from_config(config)?;
    let field = json!({"field":"engine"});
    let (version, launch) = if engine == "opencode" {
        // `discover_opencode`: a trusted binary whose version the sandboxed probe reads.
        match opencode_engine::discover(&host.route().home, host.path.as_ref(), None) {
            Some(found) => (
                found.version,
                vec![
                    found.binary.to_string_lossy().into_owned(),
                    opencode_engine::ACP.to_owned(),
                ],
            ),
            None => {
                return Err(refuse(
                    "engine_not_downloaded",
                    "Install the supported engine before connecting it.",
                )
                .with(field))
            }
        }
    } else {
        match host.launch(engine)? {
            Some(found) => found,
            None => {
                return Err(refuse(
                    "engine_not_downloaded",
                    "Install the supported engine before connecting it.",
                )
                .with(field))
            }
        }
    };
    if crate::config_read_model::adapter_version(engine, &version)["qualified"] != true {
        return Err(refuse(
            "engine_version_unqualified",
            "Install a source-qualified adapter version.",
        )
        .with(field));
    }
    let route = host.route().clone();
    let native = route.home.join(match engine {
        "codex-acp" => ".codex",
        "opencode" => ".local/share/opencode",
        _ => ".claude",
    });
    let marker = match engine {
        "claude-agent-acp" => ".credentials.json",
        _ => "auth.json",
    };
    if !native.join(marker).is_file() {
        return Err(refuse(
            "native_login_required",
            "Log in with the engine's native account command first.",
        )
        .with(field));
    }
    let provider = match engine {
        "codex-acp" => "openai",
        "opencode" => "opencode",
        _ => "anthropic",
    };
    let id = {
        // `create_referenced_connection(reuse=True, expected_revision=...)`.
        let _lock = write::owner_lock(&folder)?;
        let before = reader::read_plain(&path, true)?;
        if write::token(before.as_deref()) != expected {
            return Err(refuse(
                "revision_conflict",
                "Connections changed; reload before retrying.",
            )
            .with(json!({"field":"expected_revision"})));
        }
        let input = registry_json::RegistryJson::parse(before.as_deref())?;
        let mut registry = registry::normalize(&input.value)?;
        let duplicate = registry["connections"]
            .as_object()
            .unwrap()
            .values()
            .find(|r| {
                r["engine"] == engine
                    && r["provider"] == provider
                    && r["auth_mode"] == "native_account"
                    && r["key_kind"].is_null()
                    && r["endpoint"].is_null()
                    && r["launch_command"] == json!(launch)
            })
            .map(|r| r["id"].as_str().unwrap().to_owned());
        match duplicate {
            Some(id) => id,
            None => {
                let mut id = None;
                for _ in 0..32 {
                    let mut bytes = [0u8; 6];
                    use std::io::Read;
                    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
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
                    "id": id, "name": engine, "engine": engine, "provider": provider,
                    "auth_mode": "native_account", "key_kind": null, "endpoint": null,
                    "launch_command": launch,
                    "credential_ref": {"kind": "engine_native_dir", "path": native, "owned": false},
                    "approved_fingerprint": crate::config_read_model::route_fingerprint(
                        engine, provider, None, Some(launch.as_slice())),
                    "state": crate::config_read_model::native_login_marker(engine, &native),
                    "qualified": true, "created_at": now, "updated_at": now,
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
        }
    };
    // `refresh_connection`: observe models once; failure keeps the connection.
    let (phase, reason) = match catalog_refresh::observe_first_run(&route, &id) {
        Ok(Some(row)) if row["success"] == true => ("ready", None),
        Ok(Some(row)) => ("catalog_failed", row["reason"].as_str().map(str::to_owned)),
        Ok(None) => (
            "catalog_failed",
            Some("native_catalog_unavailable".to_owned()),
        ),
        Err(code) => ("catalog_failed", Some(format!("refresh_error:{code}"))),
    };
    let quoted = |s: &str| serde_json::to_string(s).unwrap();
    let status = match &reason {
        Some(reason)
            if crate::config_read_model::matches(
                r"^[a-z][a-z0-9_]{0,63}(?::[A-Za-z][A-Za-z0-9_]{0,63})?$",
                reason,
            ) =>
        {
            format!(
                "{{\"phase\": {}, \"reason\": {}}}",
                quoted(phase),
                quoted(reason)
            )
        }
        _ => format!("{{\"phase\": {}}}", quoted(phase)),
    };
    // Diagnostics are best effort, never a failure of the committed connection.
    // Only the engines Cortex sets up itself keep a first-run status file.
    if engine != "opencode" {
        let _ = write::replace(
            &folder.join(format!("first-run-{engine}.json")),
            status.as_bytes(),
        );
    }
    let after = reader::read_plain(&path, true)?;
    let input = registry_json::RegistryJson::parse(after.as_deref())?;
    let registry = registry::normalize(&input.value)?;
    let (mut view, connection) = entry(&registry, after.as_deref(), &id);
    view["changed"] = json!(true);
    view["connection"] = connection;
    view["catalog_status"] = json!(phase);
    Ok(view)
}
/// The onboarding request refusals that precede any lock, registry or discovery.
pub(super) fn management_onboard_opencode_refusal(p: &Value) -> Option<Refused> {
    install::refusal(p)
}
/// Whether an OpenCode candidate binary is installed (it needs the namespace jail).
pub(super) fn management_opencode_candidate(config: &crate::Config) -> Result<bool> {
    Ok(engine_install::Host::from_config(config)?.opencode_candidate())
}
pub(super) fn management_onboard_opencode(config: &crate::Config, p: &Value) -> Result<Value> {
    if let Some(refused) = install::refusal(p) {
        return Err(refused);
    }
    let host = engine_install::Host::from_config(config)?;
    engine_install::onboard_opencode(&host, &reader::owner_folder(config)?, p)
}
fn validation(field: &str, message: &str) -> Refused {
    refuse(
        "invalid_configuration",
        format!("Correct the invalid setting(s); nothing was written. {field}: {message}"),
    )
    .with(
        json!({"field":field,"errors":[{"field":field,"code":"invalid_value","message":message}]}),
    )
}
fn update(
    config: &crate::Config,
    p: &Value,
    now: &str,
    persist: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<Value> {
    let expected = p["expected_revision"].as_str().unwrap_or("");
    if expected != "absent"
        && !(expected.len() == 71
            && expected.starts_with("sha256:")
            && expected[7..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    {
        return Err(refuse("invalid_expected_revision",
            "expected_revision must be the exact revision a read returned ('absent' or 'sha256:<64 hex>')."));
    }
    let id = p["connection_id"].as_str().unwrap_or("");
    if !crate::config_read_model::matches(r"^cn_[a-f0-9]{12}$", id) {
        return Err(refuse(
            "invalid_connection_id",
            "connection_id must be cn_<12 hex>.",
        ));
    }
    let op = p["operation"].as_str().unwrap_or("");
    match op {
        "rename" => {
            if !crate::config_read_model::clean(&p["name"], 120) {
                return Err(validation("name", "must be 1-120 printable characters"));
            }
        }
        "set_max_concurrent_sessions" => {
            if !p["max_concurrent_sessions"]
                .as_u64()
                .is_some_and(|n| (1..=64).contains(&n))
            {
                return Err(validation(
                    "max_concurrent_sessions",
                    "must be an integer from 1 through 64",
                ));
            }
        }
        "validate" | "remove" => return connections::update(config, p, now, persist),
        _ => {
            return Err(refuse(
                "unsupported_operation",
                "operation must be one of rename, set_max_concurrent_sessions, validate, remove.",
            ))
        }
    }
    let folder = reader::owner_folder(config)?;
    crate::local::private_tree(&folder)?;
    ensure_ignore(&folder)?;
    let _lock = write::owner_lock(&folder)?;
    let path = folder.join("connections.json");
    let before = reader::read_plain(&path, true)?;
    let input = registry_json::RegistryJson::parse(before.as_deref())?;
    let mut registry = registry::normalize(&input.value)?;
    let old_normalized = input.bytes(&registry);
    write::check("connections", expected, &write::token(before.as_deref()))?;
    let Some(record) = registry["connections"].get_mut(id) else {
        return Err(refuse(
            "unknown_connection",
            "connection_id names no registered connection.",
        ));
    };
    let key = if op == "rename" {
        "name"
    } else {
        "max_concurrent_sessions"
    };
    record[key] = p[key].clone();
    record["updated_at"] = json!(now);
    let body = input.bytes(&registry);
    // Budget the *complete* reply before committing. Never post-read/delegate
    // after effects: timeouts and uncertain durability do not authorize a replay.
    let changed_bytes = body != old_normalized;
    let actual = if changed_bytes {
        Some(body.as_slice())
    } else {
        before.as_deref()
    };
    let mut reply = connection_management_view(&registry, &write::token(actual), actual.is_some());
    reply["changed"] = json!(true);
    reply["notes"] = json!([]);
    if body.len() > FRAME_BUDGET || serde_json::to_vec(&reply).unwrap().len() > FRAME_BUDGET {
        return Err(refuse(
            "machine_reply_too_large",
            "Settings exceed the native write limit.",
        ));
    }
    let current = reader::read_plain(&path, true)?;
    write::check(
        "connections",
        &write::token(before.as_deref()),
        &write::token(current.as_deref()),
    )?;
    if changed_bytes {
        persist(&path, &body)?;
    }
    Ok(reply)
}
