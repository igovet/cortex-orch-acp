//! Confirmed two-file reset and shared public-preset payload/snapshot helpers.
use crate::machine::{
    config_action::ensure_ignore, native::*, project_config as reader, project_config_save as write,
};
use serde_json::{json, Value};
use std::path::Path;

#[cfg(test)]
#[path = "config_reset_tests.rs"]
mod tests;

const NAMES: [&str; 4] = ["routing", "defaults", "mcp_servers", "routing_profiles"];
const FILES: [&str; 4] = [
    "routing.json",
    "defaults.json",
    "mcp-servers.json",
    "routing-profiles.json",
];
pub(super) const TEMPLATE: &str = include_str!("../../../../policy/model-settings-default.json");

fn pretty(value: write::Ordered) -> Vec<u8> {
    let mut bytes = value.pretty();
    bytes.push(b'\n');
    bytes
}
pub(super) fn snapshot(folder: &Path) -> Result<([Option<Vec<u8>>; 4], Value)> {
    let mut bytes = [None, None, None, None];
    let mut files = json!({});
    for (i, file) in FILES.iter().enumerate() {
        bytes[i] = reader::read_plain(&folder.join(file), true)?;
        files[NAMES[i]] = json!({"revision":write::token(bytes[i].as_deref()),
            "status":if bytes[i].is_some() {"existing"} else {"absent"}});
    }
    let revisions = pretty(write::Ordered::Object(
        (0..2)
            .map(|i| {
                (
                    NAMES[i].into(),
                    write::Ordered::Scalar(json!(write::token(bytes[i].as_deref()))),
                )
            })
            .collect(),
    ));
    Ok((
        bytes,
        json!({"surface":"default_preset", "revision":write::token(Some(&revisions)), "files":files}),
    ))
}
pub(super) fn payloads() -> Result<[Vec<u8>; 2]> {
    // Build-owned public asset, never a socket-selected file. Keep its insertion
    // order: the Python preset serializer does not sort object keys.
    let template: write::Ordered = serde_json::from_str(TEMPLATE).map_err(|_| {
        refuse(
            "invalid_default_preset",
            "The bundled preset failed validation; nothing was replaced.",
        )
    })?;
    let write::Ordered::Object(mut fields) = template else {
        return Err(refuse(
            "invalid_default_preset",
            "The bundled preset failed validation; nothing was replaced.",
        ));
    };
    let mut take = |key: &str| -> Result<write::Ordered> {
        let i = fields.iter().position(|(k, _)| k == key).ok_or_else(|| {
            refuse(
                "invalid_default_preset",
                "The bundled preset failed validation; nothing was replaced.",
            )
        })?;
        Ok(fields.remove(i).1)
    };
    let routing = take("routing")?;
    let coordinator = take("coordinator")?;
    let mut defaults = vec![("coordinator".into(), coordinator)];
    if let Some((_, language)) = fields.into_iter().find(|(k, _)| k == "language") {
        defaults.push(("language".into(), language));
    }
    Ok([
        pretty(routing),
        pretty(write::Ordered::Object(vec![(
            "defaults".into(),
            write::Ordered::Object(defaults),
        )])),
    ])
}
pub(super) fn combined_revision(bytes: &[Option<Vec<u8>>; 4]) -> String {
    let body = pretty(write::Ordered::Object(
        (0..2)
            .map(|i| {
                (
                    NAMES[i].into(),
                    write::Ordered::Scalar(json!(write::token(bytes[i].as_deref()))),
                )
            })
            .collect(),
    ));
    write::token(Some(&body))
}
fn stamp() -> Result<String> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| {
            refuse(
                "storage_error",
                "Owner configuration storage refused the operation.",
            )
        })?
        .as_nanos()
        .to_string())
}
pub(super) fn reset(config: &crate::Config, params: &Value) -> Result<Value> {
    reset_with(config, params, stamp, write::replace)
}
/// The management API's reset: confirmation and `expected` were checked by
/// the caller, and `expected` is the caller's own revision, never re-read.
pub(super) fn reset_expected(config: &crate::Config, expected: &str) -> Result<Value> {
    replace_with(config, expected, stamp, write::replace)
}
fn reset_with(
    config: &crate::Config,
    params: &Value,
    stamp: impl FnOnce() -> Result<String>,
    persist: impl FnMut(&Path, &[u8]) -> Result<()>,
) -> Result<Value> {
    let folder = reader::owner_folder(config)?;
    // The machine helper re-reads the preset revision, ignoring the supplied
    // expected_revision, *before* confirmation. Preserve that public contract.
    let (_, observed) = snapshot(&folder)?;
    if params["confirm"] != true {
        return Err(refuse(
            "confirmation_required",
            "Send confirm: true to replace both user preset files.",
        ));
    }
    replace_with(
        config,
        observed["revision"].as_str().unwrap(),
        stamp,
        persist,
    )
}
fn replace_with(
    config: &crate::Config,
    expected: &str,
    stamp: impl FnOnce() -> Result<String>,
    mut persist: impl FnMut(&Path, &[u8]) -> Result<()>,
) -> Result<Value> {
    let payloads = payloads()?;
    let folder = reader::owner_folder(config)?;
    crate::local::private_tree(&folder)?;
    ensure_ignore(&folder)?;
    let _lock = write::owner_lock(&folder)?;
    let (before, mut reply) = snapshot(&folder)?;
    write::check(
        "default_preset",
        expected,
        reply["revision"].as_str().unwrap(),
    )?;
    let stamp = stamp()?;
    let mut backups = json!({});
    for i in 0..2 {
        if before[i].is_some() {
            backups[NAMES[i]] = json!(folder.join(format!("{}.cortex-backup-{stamp}", FILES[i])));
        }
        reply["files"][NAMES[i]] =
            json!({"revision":write::token(Some(&payloads[i])), "status":"existing"});
    }
    let revisions = pretty(write::Ordered::Object(
        (0..2)
            .map(|i| {
                (
                    NAMES[i].into(),
                    write::Ordered::Scalar(json!(write::token(Some(&payloads[i])))),
                )
            })
            .collect(),
    ));
    reply["revision"] = json!(write::token(Some(&revisions)));
    reply["changed"] = json!((0..2).any(|i| before[i].as_deref() != Some(payloads[i].as_slice())));
    reply["backups"] = backups.clone();
    // Budget the complete reply before the first backup/replacement.
    if serde_json::to_vec(&reply).unwrap().len() > FRAME_BUDGET {
        return Err(refuse(
            "machine_reply_too_large",
            "Settings exceed the native write limit.",
        ));
    }
    for i in 0..2 {
        if let Some(body) = &before[i] {
            let path = Path::new(backups[NAMES[i]].as_str().unwrap());
            // A clock collision must not destroy an earlier recovery copy.
            if path.symlink_metadata().is_ok() {
                return Err(refuse(
                    "unsafe_storage",
                    "A preset backup already exists; nothing was replaced.",
                ));
            }
            persist(path, body)?;
        }
    }
    for i in 0..2 {
        if let Err(error) = persist(&folder.join(FILES[i]), &payloads[i]) {
            // A failed directory sync may follow a completed replacement. Do not
            // guess which bytes landed or launch another provider.
            if error.code == "machine_outcome_unknown" {
                return Err(error);
            }
            for j in (0..i).rev() {
                let path = folder.join(FILES[j]);
                let restored = if let Some(body) = &before[j] {
                    persist(&path, body)
                } else {
                    std::fs::remove_file(&path)
                        .and_then(|_| std::fs::File::open(&folder)?.sync_all())
                        .map_err(Refused::from)
                };
                if restored.is_err() {
                    return Err(refuse(
                        "preset_rollback_failed",
                        "Preset reset and rollback failed; restore the private backups.",
                    ));
                }
            }
            return Err(error);
        }
    }
    Ok(reply)
}
