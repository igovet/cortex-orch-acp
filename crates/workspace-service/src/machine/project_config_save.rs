//! Revision-locked project settings writer. No Python retry after admission.
mod ceiling;
pub(in crate::machine) mod management;
mod routing;
pub(in crate::machine) use routing::view::Ordered;
// Machine-wide sections share the same persistence and revision fences.
#[cfg(test)]
mod machine_save_tests;
pub(crate) mod user_defaults;
pub(crate) mod user_mcp;
pub(crate) mod user_routing;
pub(crate) use routing::view::{
    observation as routing_observation, valid_entry as valid_cached_entry,
};
#[cfg(test)]
mod tests;
mod validation;
use super::{context, native::*, project_config as reader, settings};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
};

pub(super) fn token(data: Option<&[u8]>) -> String {
    data.map(|b| format!("sha256:{:x}", Sha256::digest(b)))
        .unwrap_or("absent".into())
}
fn expected(params: &Value, surface: &str) -> Result<String> {
    if super::workers::truthy(&params["tokens"]) && !params["tokens"].is_object() {
        return Err(validation::object_error());
    }
    let v = &params["tokens"][surface];
    let v = if super::workers::truthy(v) {
        v.clone()
    } else {
        json!("absent")
    };
    let s = v.as_str().unwrap_or("");
    if s != "absent"
        && !(s.len() == 71
            && s.starts_with("sha256:")
            && s[7..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
    {
        return Err(refuse("invalid_expected_revision",
            "expected_revision must be the exact revision a read returned ('absent' or 'sha256:<64 hex>')."));
    }
    Ok(s.into())
}
pub(super) fn check(surface: &str, expected: &str, current: &str) -> Result<()> {
    if expected != current {
        return Err(refuse("revision_conflict",format!("The {surface} configuration changed after it was read (expected revision {expected}, current {current}); nothing was written. Re-read it and apply the change again."))
            .with(json!({"field":"expected_revision","current_revision":current})));
    }
    Ok(())
}
fn unknown() -> Refused {
    refuse("machine_outcome_unknown","The settings write may have completed. Re-read settings before retrying; no automatic replay is safe.")
}
pub(super) fn owner_lock(folder: &Path) -> Result<cortex_core::store::locks::AccessLock> {
    crate::local::private_tree(folder)?;
    cortex_core::store::locks::AccessLock::acquire(
        &folder.join("connections.lock"),
        true,
        true,
        std::time::Duration::from_secs(25),
    )
    .map_err(|e| {
        refuse(
            if e.code() == "busy" {
                "machine_busy"
            } else {
                "unsafe_storage"
            },
            "The owner configuration lock is busy or unsafe; nothing was written.",
        )
    })
}
fn safe_target(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Ok(m)
            if m.is_file()
                && !m.file_type().is_symlink()
                && m.nlink() == 1
                && m.uid() == unsafe { libc::getuid() } =>
        {
            Ok(())
        }
        _ => Err(refuse(
            "unsafe_storage",
            format!(
                "{} is a link, hardlink or foreign file; refusing to replace it.",
                path.display()
            ),
        )),
    }
}
pub(super) fn replace(path: &Path, bytes: &[u8]) -> Result<()> {
    replace_sync(path, bytes, |parent| {
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    })
}
fn replace_sync(path: &Path, bytes: &[u8], finish: impl FnOnce(&Path) -> Result<()>) -> Result<()> {
    if bytes.len() > FRAME_BUDGET {
        return Err(refuse(
            "machine_reply_too_large",
            "Project settings exceed the native write limit.",
        ));
    }
    safe_target(path)?;
    let parent = path.parent().unwrap();
    let mut name = std::ffi::OsString::from(".");
    name.push(path.file_name().unwrap());
    name.push(format!(".{}.tmp", settings::unique()));
    let tmp = parent.join(name);
    let mut changed = false;
    let result = (|| {
        let mut f = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        safe_target(path)?;
        fs::rename(&tmp, path)?;
        changed = true;
        finish(parent)?;
        Ok(())
    })();
    let _ = fs::remove_file(tmp);
    if changed && result.is_err() {
        Err(unknown())
    } else {
        result
    }
}
pub(super) fn save(root: &Path, config: &crate::Config, params: &Value) -> Result<Value> {
    // Preserve legacy preflight order (routing/registry, then store/user/ceiling)
    // even for an unsupported surface. This reader cannot create a second store.
    let current = reader::read_now(root, config)?;
    // A malformed owner defaults root is never an unrestricted user ceiling.
    ceiling::user(config)?;
    if !params.is_object() {
        return Err(validation::object_error());
    }
    match params["surface"].as_str() {
        Some("routing") => routing::save(root, config, params, &current),
        Some("project_ceiling") => ceiling::save(root, config, params),
        Some("mcp_servers") => user_mcp::save_project(root, config, params),
        _ => Err(refuse(
            "unsupported_action",
            "Only routing and ceilings can be overridden by a project.",
        )),
    }
}
