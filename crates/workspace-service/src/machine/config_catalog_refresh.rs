//! Catalog refresh admission, registry fences, observation and immutable publication.
use super::*;
use crate::{config_read_model::ConfigurationReader, native_routes::RouteEnvironment};
use cortex_core::store::locks::{private_directory, AccessLock};
use std::{ffi::CString, fs, time::Duration};
#[path = "config_catalog_opencode.rs"]
mod opencode;
#[path = "config_catalog_producer.rs"]
mod producer;
#[path = "config_catalog_startup.rs"]
pub(crate) mod startup;

fn state(
    config: &crate::Config,
) -> Result<(std::path::PathBuf, RouteEnvironment, Option<Vec<u8>>, Value)> {
    let folder = reader::owner_folder(config)?;
    let mut env = crate::machine::config_read::environment(config)?;
    env.config = folder.parent().unwrap().to_owned();
    let bytes = reader::read_plain(&folder.join("connections.json"), true)?;
    let input = registry_json::RegistryJson::parse(bytes.as_deref())?;
    let records = registry::normalize(&input.value)?;
    Ok((folder, env, bytes, records))
}
/// Two admitted connections must not share one credential location (Python skips
/// keyring references, which name no path). The first colliding connection id.
fn credential_collision(env: &RouteEnvironment, records: &Value) -> Option<String> {
    let all = records["connections"].as_object().unwrap();
    all.values().find_map(|r| {
        if !producer::admitted(env, r) || keyring::reference(r) {
            return None;
        }
        all.values()
            .any(|other| {
                other["id"] != r["id"]
                    && other["credential_ref"]["path"] == r["credential_ref"]["path"]
                    && !(r["credential_ref"]["owned"] == false
                        && other["credential_ref"]["owned"] == false
                        && other["credential_ref"]["kind"] == "engine_native_dir")
            })
            .then(|| r["id"].as_str().unwrap_or("").to_owned())
    })
}
fn collision_refusal(id: &str) -> Refused {
    refuse(
        "credential_location_shared",
        format!("Connection {id} shares its credential location with another connection; refreshing could mix their accounts. Give each connection its own credential location. Nothing was changed."),
    )
}
fn view(env: &RouteEnvironment) -> Result<Value> {
    ConfigurationReader::new(env.clone())
        .and_then(|r| r.catalog_selector(None))
        .map(|v| v["routes"].clone())
        .map_err(|e| {
            refuse(
                e.code(),
                "Native catalog projection refused; re-read Settings.",
            )
        })
}
fn home_read_only(env: &RouteEnvironment, r: &Value) -> bool {
    let path = if r["credential_ref"]["kind"] == "engine_native_dir" {
        std::path::PathBuf::from(r["credential_ref"]["path"].as_str().unwrap())
    } else {
        let suffix = match r["engine"].as_str().unwrap() {
            "codex-acp" => "codex-home",
            "claude-agent-acp" => "claude-config",
            _ => "opencode",
        };
        env.state
            .join("cortex/connections")
            .join(r["id"].as_str().unwrap())
            .join(suffix)
    };
    if !fs::metadata(&path).is_ok_and(|m| m.is_dir()) {
        return false;
    }
    let Ok(path) = CString::new(path.as_os_str().as_encoded_bytes()) else {
        return false;
    };
    // Same metadata-only classification as refresh._engine_home_read_only.
    unsafe {
        if libc::access(path.as_ptr(), libc::W_OK) != 0 {
            return true;
        }
        let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        libc::statvfs(path.as_ptr(), info.as_mut_ptr()) == 0
            && info.assume_init().f_flag & libc::ST_RDONLY != 0
    }
}
/// Whether a task session of `record` pins the native Codex launcher
/// (`producer::codex_install_shaped`).
pub(crate) fn codex_install_shaped(record: &Value) -> bool {
    producer::codex_install_shaped(record)
}
/// The native Codex launcher a task session pins as `CODEX_PATH`
/// (`producer::codex_session_launcher`).
pub(crate) fn codex_session_launcher(
    env: &RouteEnvironment,
    record: &Value,
) -> std::result::Result<std::path::PathBuf, &'static str> {
    producer::codex_session_launcher(env, record)
}
pub(super) fn refresh(config: &crate::Config) -> Result<Value> {
    refresh_with(config, Duration::from_secs(25), || {})
}
/// Python `detached_refresh.refresh_locked` for the per-session prompt-free
/// refresh: hold `<state>/cortex/catalog/<id>/.refresh.lock` without waiting, so
/// a refresh of the same connection already running in either entry is skipped.
pub(crate) fn observe_session_refresh(
    env: &RouteEnvironment,
    id: &str,
) -> std::result::Result<Option<Value>, &'static str> {
    let suffix = id.strip_prefix("cn_").unwrap_or_default();
    if suffix.len() != 12
        || !suffix
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err("refresh_lock_unavailable");
    }
    let root = env.state.join("cortex");
    let directory = root.join("catalog").join(id);
    for path in [&root, &root.join("catalog"), &directory] {
        private_directory(path, true).map_err(|_| "refresh_lock_unavailable")?;
    }
    let _held = AccessLock::acquire(&directory.join(".refresh.lock"), true, true, Duration::ZERO)
        .map_err(|e| match e {
        cortex_core::CoreError::Refused("busy") => "refresh_in_progress",
        _ => "refresh_lock_unavailable",
    })?;
    observe_first_run(env, id)
}
/// Startup first-run observation of one connection (Python
/// `connections.automatic.refresh_connection` -> `perform_refresh`, 20 seconds).
/// The record is re-read and normalized here. `Ok(None)`: this absolute launch
/// shape keeps Python's launch-time verifier, so no native outcome is claimed.
/// Python publishes without a registry fence; this port refuses publication if
/// the observed record changed meanwhile. Nothing is retried or replayed.
pub(crate) fn observe_first_run(
    env: &RouteEnvironment,
    id: &str,
) -> std::result::Result<Option<Value>, &'static str> {
    let path = env.config.join("cortex/connections.json");
    let load = || -> Result<Value> {
        let bytes = reader::read_plain(&path, true)?;
        let input = registry_json::RegistryJson::parse(bytes.as_deref())?;
        registry::normalize(&input.value)
    };
    let records = load().map_err(|e| e.code)?;
    let r = records["connections"]
        .get(id)
        .cloned()
        .ok_or("connection_not_found")?;
    if producer::admitted(env, &r) {
        return producer::run_guarded(env, &r, || first_run_fence(env, id, &r))
            .map(Some)
            .map_err(|e| e.code);
    }
    // The same pre-launch failures as the Settings refresh's non-admitted rows.
    Ok(Some(unlaunched(env, &r)))
}
fn first_run_fence(env: &RouteEnvironment, id: &str, observed: &Value) -> Result<AccessLock> {
    let folder = env.config.join("cortex");
    private_directory(&folder, false)
        .map_err(|e| refuse(e.code(), "Unsafe configuration root."))?;
    let lock = AccessLock::acquire(
        &folder.join("connections.lock"),
        true,
        true,
        Duration::from_secs(30),
    )
    .map_err(|e| refuse(e.code(), "The registry publication lock is busy or unsafe."))?;
    let bytes = reader::read_plain(&folder.join("connections.json"), true)?;
    let input = registry_json::RegistryJson::parse(bytes.as_deref())?;
    let current = registry::normalize(&input.value)?;
    if current["connections"].get(id) != Some(observed) {
        return Err(refuse(
            "machine_outcome_unknown",
            "The connection changed during first-run catalog observation; no automatic replay is safe.",
        ));
    }
    Ok(lock)
}
/// The outcome of a record no qualified producer launches.
fn unlaunched(env: &RouteEnvironment, r: &Value) -> Value {
    // EngineProcess refuses null/relative commands before environment/secret
    // construction; [] fails at command[0]. These are real failed attempts,
    // never successful observations or newly created snapshots. OpenCode
    // install rejections fail before EngineProcess creates logs or a jail.
    json!({"connection_id":r["id"],"success":false,"reason":producer::unlaunched_reason(env, r)})
}
/// The management refresh when no record would launch: the same failure
/// outcomes without a registry lock, because nothing is published (Python
/// creates no lock or ignore file for these rows). `None` when one launches.
pub(super) fn refresh_unlaunched(config: &crate::Config) -> Result<Option<Value>> {
    let (_, env, _, records) = state(config)?;
    let all = records["connections"].as_object().unwrap();
    if all.values().any(|r| producer::admitted(&env, r)) {
        return Ok(None);
    }
    let mut rows: Vec<_> = all.values().collect();
    rows.sort_by_key(|r| (r["engine"].as_str(), r["name"].as_str(), r["id"].as_str()));
    let outcomes: Vec<_> = rows.into_iter().map(|r| unlaunched(&env, r)).collect();
    let mut reply = view(&env)?;
    reply["refresh"] = json!(outcomes);
    Ok(Some(reply))
}
pub(super) fn refresh_with(
    config: &crate::Config,
    wait: Duration,
    before_lock: impl FnOnce(),
) -> Result<Value> {
    let (folder, env, before, _) = state(config)?;
    before_lock();
    private_directory(&folder, false)
        .map_err(|e| refuse(e.code(), "Unsafe configuration root."))?;
    reader::read_plain(&folder.join("connections.lock"), true)?
        .ok_or_else(|| unavailable("The owner lock disappeared; re-read Settings."))?;
    let _lock =
        AccessLock::acquire(&folder.join("connections.lock"), false, true, wait).map_err(|e| {
            refuse(
                if e.code() == "busy" {
                    "machine_busy"
                } else {
                    "unsafe_storage"
                },
                "The owner configuration lock is busy or unsafe; nothing was written.",
            )
        })?;
    let current = reader::read_plain(&folder.join("connections.json"), true)?;
    write::check(
        "connections",
        &write::token(before.as_deref()),
        &write::token(current.as_deref()),
    )?;
    let input = registry_json::RegistryJson::parse(current.as_deref())?;
    let records = registry::normalize(&input.value)?;
    if let Some(id) = credential_collision(&env, &records) {
        return Err(collision_refusal(&id));
    }
    let mut rows: Vec<_> = records["connections"]
        .as_object()
        .unwrap()
        .values()
        .collect();
    rows.sort_by_key(|r| (r["engine"].as_str(), r["name"].as_str(), r["id"].as_str()));
    let outcomes: Vec<_> = rows
        .into_iter()
        .map(|r| -> Result<Value> {
            if producer::admitted(&env, r) {
                return producer::run(&env, r, || {
                    let bytes = reader::read_plain(&folder.join("connections.json"), true)?;
                    if bytes != current {
                        return Err(refuse("machine_outcome_unknown",
                            "The registry changed during catalog observation. Re-read Settings; no automatic replay is safe."));
                    }
                    Ok(())
                });
            }
            Ok(unlaunched(&env, r))
        })
        .collect::<Result<_>>()?;
    let launched = records["connections"]
        .as_object()
        .unwrap()
        .values()
        .any(|r| producer::admitted(&env, r));
    let mut reply = view(&env).map_err(|error| if launched {
        refuse("machine_outcome_unknown","Catalog observation may have published snapshots but its final view failed. Re-read Settings; do not automatically replay.")
    } else {error})?;
    reply["refresh"] = json!(outcomes);
    if serde_json::to_vec(&reply).unwrap().len() > FRAME_BUDGET {
        if launched {
            return Err(refuse("machine_outcome_unknown","Catalog observation may have published snapshots but the reply exceeds its limit. Re-read Settings; do not automatically replay."));
        }
        return Err(refuse(
            "machine_reply_too_large",
            "Settings exceed the native read limit.",
        ));
    }
    let after = reader::read_plain(&folder.join("connections.json"), true)?;
    if launched && after != current {
        return Err(refuse("machine_outcome_unknown","The registry changed after catalog observation. Re-read Settings; do not automatically replay."));
    }
    write::check(
        "connections",
        &write::token(current.as_deref()),
        &write::token(after.as_deref()),
    )?;
    Ok(reply)
}
