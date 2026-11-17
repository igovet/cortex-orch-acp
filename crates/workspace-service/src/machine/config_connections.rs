//! Filesystem metadata validation and non-secret removal, selected before effects.
#[cfg(test)]
#[path = "config_connections_tests.rs"]
mod tests;
use super::{registry, registry_json::RegistryJson, *};
use crate::machine::config_read;
use std::path::Path;
#[path = "config_secret_remove.rs"]
mod secret;

// Only the retained provider can observe the selected Python keyring backend.
// Provider absence is not evidence of an import failure or credential state.
fn portable(record: &Value, operation: &str) -> bool {
    match operation {
        "validate" => !keyring::reference(record),
        "remove" => true,
        _ => true,
    }
}
fn defaults(folder: &Path) -> Result<(Option<Vec<u8>>, Value)> {
    let bytes = reader::read_plain(&folder.join("defaults.json"), true)?;
    let (value, _, _) =
        crate::config_read_model::defaults_document(bytes.as_deref()).map_err(|_| {
            refuse(
                "invalid_settings",
                "defaults.json cannot be read, so Cortex cannot tell whether the connection is the default route. Repair defaults.json and retry; nothing was removed.",
            )
        })?;
    Ok((bytes, value))
}
/// Validation of an OS-keyring credential needs the host keyring backend, which this
/// service links none of and never opens. That one shape is an explicit refusal before
/// any effect, never a credential state (see `config_keyring`).
fn keyring_unobservable(config: &crate::Config, p: &Value) -> bool {
    if p["operation"] != "validate" {
        return false;
    }
    let inspect = || -> Result<bool> {
        let folder = reader::owner_folder(config)?;
        let bytes = reader::read_plain(&folder.join("connections.json"), true)?;
        let parsed = RegistryJson::parse(bytes.as_deref())?;
        let normalized = registry::normalize(&parsed.value)?;
        // A stale revision is refused first, before any keyring lookup.
        if p["expected_revision"].as_str() != Some(&write::token(bytes.as_deref())) {
            return Ok(false);
        }
        let record = &normalized["connections"][p["connection_id"].as_str().unwrap_or("")];
        Ok(!portable(record, "validate"))
    };
    // Unsafe/malformed registry admission is still native fail-closed; never replay.
    inspect().unwrap_or(false)
}
pub(super) fn update(
    config: &crate::Config,
    p: &Value,
    now: &str,
    persist: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<Value> {
    update_with(config, p, now, None, persist)
}
/// `backend` is only ever a test fake; production has no native keyring backend.
fn update_with(
    config: &crate::Config,
    p: &Value,
    now: &str,
    backend: Option<&dyn keyring::Backend>,
    persist: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<Value> {
    if backend.is_none() && keyring_unobservable(config, p) {
        return Err(unavailable(
            "OS keyring storage is not supported by this build, so a keyring credential cannot be validated. Store the key in a secret file instead (replace the connection with a secret-file credential). Nothing was changed.",
        ));
    }
    let folder = reader::owner_folder(config)?;
    crate::local::private_tree(&folder)?;
    ensure_ignore(&folder)?;
    let _lock = write::owner_lock(&folder)?;
    let path = folder.join("connections.json");
    let before = reader::read_plain(&path, true)?;
    // The retained validate/remove wrapper checks revision before parsing.
    write::check(
        "connections",
        p["expected_revision"].as_str().unwrap(),
        &write::token(before.as_deref()),
    )?;
    let input = RegistryJson::parse(before.as_deref())?;
    let mut state = registry::normalize(&input.value)?;
    let original = input.bytes(&state);
    let id = p["connection_id"].as_str().unwrap();
    let op = p["operation"].as_str().unwrap();
    let record = state["connections"].get(id).cloned().ok_or_else(|| {
        refuse(
            "unknown_connection",
            "connection_id names no registered connection.",
        )
    })?;
    if !portable(&record, op) && backend.is_none() {
        return Err(unavailable(
            "OS keyring storage is not supported by this build, so a keyring credential cannot be validated. Store the key in a secret file instead. Nothing was written.",
        ));
    }
    let mut notes = vec![];
    let mut observed_defaults = None;
    if op == "validate" {
        let new_state = if keyring::reference(&record) {
            keyring::state(
                backend.expect("keyring backend checked before validation"),
                &record,
            )
        } else {
            let mut environment = config_read::environment(config)?;
            environment.config = folder.parent().unwrap().to_owned();
            crate::config_refresh::derive(
                &environment,
                &record,
                state["connections"].as_object().unwrap(),
            )
        };
        if record["state"] != new_state {
            state["connections"][id]["state"] = json!(new_state);
            state["connections"][id]["updated_at"] = json!(now);
        }
    } else {
        let (bytes, value) = defaults(&folder)?;
        if value["route_connection_id"] == id {
            notes.push("user_default_route_uses_removed_connection");
        }
        observed_defaults = Some(bytes);
        state["connections"].as_object_mut().unwrap().remove(id);
        if let Some(history) = state["override_history"].as_object_mut() {
            history.remove(id);
        }
        // A registry that never opted out of an engine has no list yet.
        let mut disabled: std::collections::BTreeSet<String> = state["auto_connect_disabled"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect();
        disabled.insert(record["engine"].as_str().unwrap().to_owned());
        state["auto_connect_disabled"] = json!(disabled);
    }
    let bytes = input.bytes(&state);
    let changed = bytes != original;
    let actual = if changed {
        Some(bytes.as_slice())
    } else {
        before.as_deref()
    };
    let mut reply = connection_management_view(&state, &write::token(actual), actual.is_some());
    reply["changed"] = json!(true);
    reply["notes"] = json!(notes);
    if bytes.len() > FRAME_BUDGET || serde_json::to_vec(&reply).unwrap().len() > FRAME_BUDGET {
        return Err(refuse(
            "machine_reply_too_large",
            "Settings exceed the native write limit.",
        ));
    }
    write::check(
        "connections",
        &write::token(before.as_deref()),
        &write::token(reader::read_plain(&path, true)?.as_deref()),
    )?;
    if let Some(expected) = observed_defaults {
        write::check(
            "user_defaults",
            &write::token(expected.as_deref()),
            &write::token(reader::read_plain(&folder.join("defaults.json"), true)?.as_deref()),
        )?;
    }
    let removed_secret = op == "remove" && secret::unlink(&folder, &record)?;
    if changed {
        if let Err(error) = persist(&path, &bytes) {
            // Python also deletes before registry commit and cannot restore this
            // secret without reading its contents. Never claim rollback or replay.
            if removed_secret {
                return Err(refuse("machine_outcome_unknown",
                    "Secret was removed but the registry update could not be confirmed. Inspect settings before retrying."));
            }
            return Err(error);
        }
    }
    Ok(reply)
}
