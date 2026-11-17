//! Private API-key creation. Three registry transactions; no secret read or replay.
use super::{refuse, registration, Result};
use crate::machine::{
    config_action::{registry, registry_json::RegistryJson},
    project_config as reader, project_config_save as write,
};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::Path,
};

#[path = "config_secret_remove.rs"]
mod secret;
#[cfg(test)]
#[path = "engine_add_key_tests.rs"]
mod tests;

fn failed() -> super::Refused {
    refuse(
        "connection_failed",
        "The connection could not be saved. Refresh before retrying; no key is shown.",
    )
}
fn unknown() -> super::Refused {
    refuse("machine_outcome_unknown", "Connection storage may have changed. Re-read settings before retrying; no automatic replay is safe.")
}
pub(super) fn validate(p: &Value) -> Result<()> {
    // Python strip includes ASCII information separators, which Rust trim omits.
    if !p["name"]
        .as_str()
        .is_some_and(|s| !trim(s).is_empty() && s.chars().count() <= 100)
    {
        return Err(refuse(
            "invalid_name",
            "Enter a connection name up to 100 characters.",
        ));
    }
    if !p["key"].as_str().is_some_and(|s| {
        !trim(s).is_empty() && s.chars().count() <= 16384 && !s.contains(['\n', '\r', '\0'])
    }) {
        return Err(refuse(
            "invalid_key",
            "Enter a non-empty, single-line API key.",
        ));
    }
    Ok(())
}
fn trim(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c))
}
pub(super) fn validate_revision(p: &Value) -> Result<&str> {
    p["revision"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            refuse(
                "revision_conflict",
                "Refresh connections before adding this connection.",
            )
        })
}
pub(super) fn add(folder: &Path, p: &Value, adapter: &Value) -> Result<Value> {
    add_with(
        folder,
        p,
        adapter,
        cortex_core::now,
        registration::candidate,
        write::replace,
        || Ok(()),
    )
}
fn transaction<T, F: FnMut(&Path, &[u8]) -> Result<()>>(
    folder: &Path,
    persist: &mut F,
    body: impl FnOnce(&mut Value, &mut F) -> Result<T>,
) -> Result<T> {
    registration::prepare(folder)?;
    let _lock = write::owner_lock(folder)?;
    let path = folder.join("connections.json");
    let before = reader::read_plain(&path, true)?;
    let input = RegistryJson::parse(before.as_deref())?;
    let mut state = registry::normalize(&input.value)?;
    let original = input.bytes(&state);
    let result = body(&mut state, persist)?;
    let bytes = input.bytes(&state);
    if bytes != original {
        if bytes.len() > super::super::native::FRAME_BUDGET {
            return Err(failed());
        }
        write::check(
            "connections",
            &write::token(before.as_deref()),
            &write::token(reader::read_plain(&path, true)?.as_deref()),
        )?;
        persist(&path, &bytes)?;
    }
    Ok(result)
}
fn add_with(
    folder: &Path,
    p: &Value,
    adapter: &Value,
    mut now: impl FnMut() -> String,
    mut candidate: impl FnMut() -> Result<String>,
    mut persist: impl FnMut(&Path, &[u8]) -> Result<()>,
    validate: impl FnOnce() -> Result<()>,
) -> Result<Value> {
    let expected = validate_revision(p)?;
    let engine = p["engine"].as_str().unwrap();
    let provider = if engine == "codex-acp" {
        "openai"
    } else {
        "anthropic"
    };
    let kind = if engine == "codex-acp" {
        "openai_api_key"
    } else {
        "anthropic_api_key"
    };
    let created = now();
    // Preserve create_connection's revision-before-allocation and directory-before-
    // duplicate order. No registry record exists for the caller until commit succeeds.
    let selected = transaction(folder, &mut persist, |state, _persist| {
        if write::token(reader::read_plain(&folder.join("connections.json"), true)?.as_deref()) != expected {
            return Err(refuse("revision_conflict", "Connections changed. Refresh before retrying."));
        }
        let records = state["connections"].as_object_mut().unwrap();
        let mut selected = None;
        for _ in 0..32 {
            let id = candidate()?;
            if !records.contains_key(&id) { selected = Some(id); break; }
        }
        let id = selected.ok_or_else(failed)?;
        let directory = folder.join("secrets");
        prepare_secrets(folder)?;
        let path = directory.join(&id);
        if records.values().any(|r| r["engine"] == engine && r["provider"] == provider
            && r["auth_mode"] == "api_key" && r["key_kind"] == kind
            && r["endpoint"].is_null() && r["launch_command"] == *adapter) {
            return Err(failed());
        }
        let launch: Vec<String> = adapter.as_array().ok_or_else(failed)?.iter()
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(failed)).collect::<Result<_>>()?;
        records.insert(id.clone(), json!({
            "id":id, "name":trim(p["name"].as_str().unwrap()), "engine":engine, "provider":provider,
            "auth_mode":"api_key", "key_kind":kind, "endpoint":null, "launch_command":adapter,
            "credential_ref":{"kind":"secret_file","path":path},
            "approved_fingerprint":crate::config_read_model::route_fingerprint(engine, provider, None, Some(&launch)),
            "state":file_state(&path), "qualified":true, "created_at":created, "updated_at":created,
            "max_concurrent_sessions":4
        }));
        Ok(id)
    }).map_err(|e| match e.code { "revision_conflict" | "machine_outcome_unknown" => e, _ => failed() })?;

    // Never run rollback after an uncertain commit. Known failures after successful
    // creation remove the new record/secret and persist the owner opt-out, as Python.
    let result = (|| {
        let updated = now();
        transaction(folder, &mut persist, |state, persist| {
            canonical(folder, &selected, &state["connections"][&selected])?;
            let path = folder.join("secrets").join(&selected);
            if state["connections"].as_object().unwrap().values().any(|r| {
                r["id"] != selected
                    && r["credential_ref"]["path"].as_str() == path.to_str()
                    && !(r["credential_ref"]["kind"] == "engine_native_dir"
                        && r["credential_ref"]["owned"] == false)
            }) {
                return Err(failed());
            }
            prepare_secrets(folder)?;
            safe_secret(&path)?;
            persist(&path, p["key"].as_str().unwrap().as_bytes())?;
            let record = state["connections"].get_mut(&selected).ok_or_else(failed)?;
            record["state"] = json!(file_state(&path));
            record["updated_at"] = json!(updated);
            Ok(())
        })?;
        validate()?;
        let checked = now();
        transaction(folder, &mut persist, |state, _persist| {
            let record = state["connections"].get_mut(&selected).ok_or_else(failed)?;
            canonical(folder, &selected, record)?;
            let derived = file_state(&folder.join("secrets").join(&selected));
            if record["state"] != derived {
                record["state"] = json!(derived);
                record["updated_at"] = json!(checked);
            }
            Ok(())
        })
    })();
    if let Err(e) = result {
        if e.code == "machine_outcome_unknown" {
            return Err(e);
        }
        let cleanup = transaction(folder, &mut persist, |state, _persist| {
            let record = state["connections"]
                .get(&selected)
                .ok_or_else(failed)?
                .clone();
            canonical(folder, &selected, &record)?;
            secret::unlink(folder, &record)?;
            state["connections"]
                .as_object_mut()
                .unwrap()
                .remove(&selected);
            state["override_history"]
                .as_object_mut()
                .unwrap()
                .remove(&selected);
            if state.get("auto_connect_disabled").is_none() {
                state["auto_connect_disabled"] = json!([]);
            }
            let disabled = state["auto_connect_disabled"].as_array_mut().unwrap();
            if !disabled.iter().any(|v| v == engine) {
                disabled.push(json!(engine));
            }
            disabled.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
            Ok(())
        });
        if cleanup.is_err() {
            return Err(unknown());
        }
        return Err(failed());
    }
    Ok(json!({"connectionId":selected,
        "message":"Key saved privately. Refresh models to observe the available catalogue."}))
}
fn canonical(folder: &Path, id: &str, record: &Value) -> Result<()> {
    if record["id"] != id
        || record["auth_mode"] != "api_key"
        || record["key_kind"].is_null()
        || record["credential_ref"]["kind"] != "secret_file"
        || record["credential_ref"]["path"].as_str() != folder.join("secrets").join(id).to_str()
    {
        return Err(failed());
    }
    Ok(())
}
fn prepare_secrets(folder: &Path) -> Result<()> {
    let directory = folder.join("secrets");
    match fs::DirBuilder::new().mode(0o700).create(&directory) {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => (),
        Err(e) => return Err(e.into()),
    }
    let m = fs::symlink_metadata(&directory)?;
    if !m.is_dir() || m.file_type().is_symlink() || m.uid() != unsafe { libc::getuid() } {
        return Err(failed());
    }
    fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
fn safe_secret(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(m)
            if m.is_file()
                && !m.file_type().is_symlink()
                && m.nlink() == 1
                && m.uid() == unsafe { libc::getuid() }
                && m.mode() & 0o077 == 0 =>
        {
            Ok(())
        }
        _ => Err(failed()),
    }
}
fn file_state(path: &Path) -> &'static str {
    if safe_secret(path).is_err() {
        return "invalid";
    }
    match fs::symlink_metadata(path) {
        Ok(m) if m.len() > 0 => "configured",
        _ => "authorization_required",
    }
}
