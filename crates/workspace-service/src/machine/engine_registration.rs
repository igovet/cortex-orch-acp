//! Referenced-account registration only. Never owns or opens account credentials.
use super::{not_ready, unchecked, Result};
use crate::machine::{
    config_action::{ensure_ignore, registry, registry_json::RegistryJson},
    project_config as reader, project_config_save as write,
};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::Path,
};

pub(super) fn prepare(folder: &Path) -> Result<()> {
    // Match the retained explicit writer's mode repair without chmod of ancestors.
    let mut current = std::path::PathBuf::new();
    for part in folder.components() {
        current.push(part);
        if !current.exists() {
            fs::DirBuilder::new()
                .mode(if current == folder { 0o700 } else { 0o755 })
                .create(&current)?;
        }
        let meta = fs::symlink_metadata(&current)?;
        if !meta.is_dir() || meta.file_type().is_symlink() {
            return Err(not_ready());
        }
    }
    if fs::metadata(folder)?.uid() != unsafe { libc::getuid() } {
        return Err(not_ready());
    }
    fs::set_permissions(folder, fs::Permissions::from_mode(0o700))?;
    ensure_ignore(folder)
}
pub(super) fn candidate() -> Result<String> {
    let mut bytes = [0u8; 6];
    if unsafe { libc::getrandom(bytes.as_mut_ptr().cast(), bytes.len(), 0) } != 6 {
        return Err(not_ready());
    }
    Ok(format!(
        "cn_{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}
pub(super) fn register(folder: &Path, home: &Path, engine: &str, adapter: &Value) -> Result<Value> {
    register_with(
        folder,
        home,
        engine,
        adapter,
        &cortex_core::now(),
        candidate,
        write::replace,
    )
    .map_err(|e| match e.code {
        "machine_outcome_unknown" => e,
        "machine_tool_failed" => unchecked(),
        _ => not_ready(),
    })
}
fn register_with(
    folder: &Path,
    home: &Path,
    engine: &str,
    adapter: &Value,
    now: &str,
    mut id: impl FnMut() -> Result<String>,
    persist: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<Value> {
    let path = folder.join("connections.json");
    // Retain the read revision before entering the shared Python/native owner lock.
    let before = reader::read_plain(&path, true)?;
    registry::normalize(&RegistryJson::parse(before.as_deref())?.value)?;
    let expected = write::token(before.as_deref());
    prepare(folder)?;
    let _lock = write::owner_lock(folder)?;
    let bytes = reader::read_plain(&path, true)?;
    let input = RegistryJson::parse(bytes.as_deref())?;
    let mut registry = registry::normalize(&input.value)?;
    let original = input.bytes(&registry);
    write::check("connections", &expected, &write::token(bytes.as_deref()))?;
    let records = registry["connections"].as_object_mut().unwrap();
    let mut selected = None;
    for _ in 0..32 {
        let id = id()?;
        if !records.contains_key(&id) {
            selected = Some(id);
            break;
        }
    }
    let id = selected.ok_or_else(not_ready)?;
    let provider = if engine == "codex-acp" {
        "openai"
    } else {
        "anthropic"
    };
    let launch: Vec<_> = adapter
        .as_array()
        .ok_or_else(not_ready)?
        .iter()
        .map(|v| v.as_str().map(str::to_owned).ok_or_else(not_ready))
        .collect::<Result<_>>()?;
    let native = home.join(if engine == "codex-acp" {
        ".codex"
    } else {
        ".claude"
    });
    let name = if engine == "codex-acp" {
        "Codex native account"
    } else {
        "Claude native account"
    };
    let fingerprint =
        crate::config_read_model::route_fingerprint(engine, provider, None, Some(&launch));
    // Duplicate reuse intentionally preserves the existing state, timestamps and ref.
    let existing = input
        .connection_order
        .iter()
        .filter_map(|id| records.get(id))
        .find(|r| {
            r["engine"] == engine
                && r["provider"] == provider
                && r["auth_mode"] == "native_account"
                && r["key_kind"].is_null()
                && r["endpoint"].is_null()
                && r["launch_command"] == *adapter
        });
    let chosen = if let Some(existing) = existing {
        existing["id"].as_str().unwrap().to_owned()
    } else {
        records.insert(
            id.clone(),
            json!({
                "id":id, "name":name, "engine":engine, "provider":provider,
                "auth_mode":"native_account", "key_kind":null, "endpoint":null,
                "launch_command":adapter,
                "credential_ref":{"kind":"engine_native_dir","owned":false,"path":native},
                "approved_fingerprint":fingerprint,
                "state":crate::config_read_model::native_login_marker(engine, &native),
                "qualified":true, "created_at":now, "updated_at":now,
                "max_concurrent_sessions":4
            }),
        );
        id
    };
    let body = input.bytes(&registry);
    // No post-write projection or delegation can replay an uncertain effect.
    if body != original {
        write::check(
            "connections",
            &write::token(bytes.as_deref()),
            &write::token(reader::read_plain(&path, true)?.as_deref()),
        )?;
        persist(&path, &body)?;
    }
    Ok(json!({"connectionId":chosen,
        "message":"Native login checked. Refresh models to observe available routes."}))
}

#[cfg(test)]
#[path = "engine_registration_tests.rs"]
mod tests;
