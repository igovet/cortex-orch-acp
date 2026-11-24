//! Project-only settings read. No configuration writes, initialization or catalog discovery.
#[cfg(test)]
#[path = "project_config_tests.rs"]
mod tests;
use super::{context, native::*, project_routing};
use cortex_core::OpenMode;
use rusqlite::OptionalExtension;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

fn revision(data: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(data))
}
fn number(s: &str) -> u64 {
    s.strip_prefix("sha256:")
        .filter(|s| {
            s.len() >= 12
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
        .and_then(|s| u64::from_str_radix(&s[..12], 16).ok())
        .unwrap_or(0)
}
fn owner_config(config: &crate::Config) -> Result<PathBuf> {
    if let Some(root) = config.launch_config_root() {
        return Ok(root.join("cortex"));
    }
    if workspace_protocol::channel::current().is_dev() {
        return Ok(config.state_dir.join("config/cortex"));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    if !root.is_absolute() {
        return Err(refuse(
            "unsafe_storage",
            "'XDG_CONFIG_HOME' must be an absolute path.",
        ));
    }
    Ok(root.join("cortex"))
}
pub(super) fn owner_folder(config: &crate::Config) -> Result<PathBuf> {
    let folder = owner_config(config)?;
    if folder
        .components()
        .any(|p| matches!(p,std::path::Component::Normal(s) if s==".cortex" || s==".cortex-dev"))
    {
        return Err(refuse("connection_storage_under_project","Connection storage must be user-scoped under XDG_CONFIG_HOME/XDG_STATE_HOME, never inside a project's .cortex store."));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    for ancestor in folder.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            _ => {
                return Err(refuse(
                    "unsafe_storage",
                    format!("{} is not a real directory.", ancestor.display()),
                ))
            }
        }
        if ancestor != folder && ancestor != home {
            for marker in [".git", ".cortex", ".cortex-dev"] {
                if ancestor.join(marker).symlink_metadata().is_ok() {
                    return Err(refuse("connection_storage_under_project",format!("{} is nested under {}, which contains a '{marker}' project marker; connection storage must be user-scoped, never inside any project workspace.",folder.display(),ancestor.display())));
                }
            }
        }
    }
    Ok(folder)
}
pub(super) fn read_plain(path: &Path, private: bool) -> Result<Option<Vec<u8>>> {
    let file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
    {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => {
            return Err(refuse(
                "unsafe_storage",
                format!(
                    "{path_display} is an unsafe link or cannot be opened safely.",
                    path_display = path.display()
                ),
            ))
        }
    };
    let m = file.metadata()?;
    if !m.is_file()
        || (private
            && (m.uid() != unsafe { libc::getuid() } || m.nlink() != 1 || m.mode() & 0o077 != 0))
    {
        return Err(refuse(
            "unsafe_storage",
            format!("{} is an unsafe link/hardlink/mode file.", path.display()),
        ));
    }
    if m.len() > FRAME_BUDGET as u64 {
        return Err(refuse(
            "machine_reply_too_large",
            "Project settings exceed the native read limit.",
        ));
    }
    let mut bytes = Vec::new();
    (&file)
        .take(FRAME_BUDGET as u64 + 1)
        .read_to_end(&mut bytes)?;
    let after = file.metadata()?;
    if (
        m.dev(),
        m.ino(),
        m.size(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
        m.nlink(),
    ) != (
        after.dev(),
        after.ino(),
        after.size(),
        after.mtime(),
        after.mtime_nsec(),
        after.ctime(),
        after.ctime_nsec(),
        after.nlink(),
    ) {
        return Err(refuse(
            "unsafe_storage",
            "Configuration changed during its bounded read; refresh before retrying.",
        ));
    }
    if bytes.len() > FRAME_BUDGET {
        return Err(refuse(
            "machine_reply_too_large",
            "Project settings exceed the native read limit.",
        ));
    }
    Ok(Some(bytes))
}
fn invalid_ceiling() -> Refused {
    refuse(
        "invalid_ceiling_snapshot",
        "a stored ceiling snapshot has an unexpected shape.",
    )
}
fn nonnegative(v: &Value) -> bool {
    v.as_u64().is_some()
}
fn truth(v: &Value) -> bool {
    super::workers::truthy(v)
}
pub(super) fn ceiling(v: &Value) -> Result<Value> {
    let Some(m) = v.as_object() else {
        return Err(invalid_ceiling());
    };
    let required = [
        "allowed_routes",
        "allowed_connections",
        "allowed_data_recipients",
        "permission_level",
        "limits",
        "spend_limit",
        "spend_limit_enforceable",
        "fallback_policy",
    ];
    if required.iter().any(|k| !m.contains_key(*k)) {
        return Err(invalid_ceiling());
    }
    let f = &v["fallback_policy"];
    if !f.as_object().is_some_and(|m| {
        [
            "allowed_target_routes",
            "allowed_failure_classes",
            "budget",
            "data_recipient_consent",
        ]
        .iter()
        .all(|k| m.contains_key(*k))
    }) {
        return Err(invalid_ceiling());
    }
    let mut v = v.clone();
    for (key, fallback) in [
        ("allowed_routes", false),
        ("allowed_connections", false),
        ("allowed_data_recipients", false),
        ("allowed_target_routes", true),
        ("allowed_failure_classes", true),
    ] {
        let target = if fallback {
            &mut v["fallback_policy"][key]
        } else {
            &mut v[key]
        };
        if target.is_null() {
            continue;
        }
        let items: Vec<Value> = if let Some(s) = target.as_str() {
            s.chars().map(|c| json!(c.to_string())).collect()
        } else if let Some(a) = target.as_array() {
            a.clone()
        } else {
            return Err(invalid_ceiling());
        };
        if items.iter().any(|v| !v.is_string()) {
            return Err(invalid_ceiling());
        }
        let set = items
            .into_iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<std::collections::BTreeSet<_>>();
        *target = json!(set);
    }
    if v["fallback_policy"]["allowed_failure_classes"]
        .as_array()
        .is_some_and(|a| {
            a.iter().any(|v| {
                [
                    "cancellation",
                    "refusal",
                    "authorization_loss",
                    "provider_restriction",
                ]
                .iter()
                .any(|s| v == s)
            })
        })
    {
        return Err(refuse("fallback_denylisted_failure_class","A fallback_policy may never name cancellation, refusal, authorization_loss, or provider_restriction as an allowed failure class; those can never trigger fallback."));
    }
    if !v["fallback_policy"]["budget"].is_null() && !nonnegative(&v["fallback_policy"]["budget"]) {
        return Err(refuse(
            "invalid_field",
            "'budget' must be a non-negative integer or None.",
        ));
    }
    if !v["permission_level"].is_null()
        && !v["permission_level"]
            .as_str()
            .is_some_and(|s| ["READ_ONLY", "STANDARD", "ELEVATED"].contains(&s))
    {
        return Err(invalid_ceiling());
    }
    let Some(limits) = v["limits"].as_object() else {
        return Err(invalid_ceiling());
    };
    let unknown = limits
        .keys()
        .filter(|s| !["calls", "tokens", "concurrency", "attempts"].contains(&s.as_str()))
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        return Err(refuse("invalid_field", format!("'limits' keys must be a subset of ['attempts', 'calls', 'concurrency', 'tokens']; got unknown key(s) {}.", serde_json::to_string(&unknown).unwrap().replace('"', "'"))));
    }
    for (k, value) in limits {
        if !nonnegative(value) {
            return Err(refuse(
                "invalid_field",
                format!("limits['{k}'] must be a non-negative integer."),
            ));
        }
    }
    if !v["spend_limit"].is_null() && !truth(&v["spend_limit_enforceable"]) {
        return Err(refuse("unenforceable_spend_limit","A spend_limit may only be set when cost telemetry is enforceable for the selected route; unknown cost is never treated as zero. Remove spend_limit or select a route whose connection reports enforceable cost telemetry."));
    }
    if !v["spend_limit"].is_null() && !nonnegative(&v["spend_limit"]) {
        return Err(refuse(
            "invalid_field",
            "'spend_limit' must be a non-negative integer or None.",
        ));
    }
    let f = &v["fallback_policy"];
    Ok(
        json!({"allowedRoutes":v["allowed_routes"],"allowedConnections":v["allowed_connections"],
        "allowedRecipients":v["allowed_data_recipients"],"attempts":v["limits"]["attempts"],
        "failureClasses":f["allowed_failure_classes"],"targetRoutes":f["allowed_target_routes"],
        "budget":f["budget"],"consent":truth(&f["data_recipient_consent"]),
        "stored":{"permissionLevel":v["permission_level"],"calls":v["limits"]["calls"],"tokens":v["limits"]["tokens"],
        "concurrency":v["limits"]["concurrency"],"spendLimit":v["spend_limit"],"spendEnforceable":truth(&v["spend_limit_enforceable"])}}),
    )
}
fn validate_user(config: &crate::Config) -> Result<()> {
    let folder = owner_folder(config)?;
    match fs::symlink_metadata(&folder) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Ok(m) if m.is_dir() && !m.file_type().is_symlink() => (),
        _ => {
            return Err(refuse(
                "unsafe_storage",
                format!("{} is not a real directory.", folder.display()),
            ))
        }
    }
    let Some(bytes) = read_plain(&folder.join("defaults.json"), true)? else {
        return Ok(());
    };
    let v: Value = serde_json::from_slice(&bytes).map_err(|_| {
        refuse(
            "invalid_user_defaults",
            "defaults.json is not valid UTF-8 JSON with the expected shape.",
        )
    })?;
    if let Some(c) = v.get("ceiling") {
        ceiling(c)?;
    }
    Ok(())
}
pub(super) fn read(root: &Path, config: &crate::Config) -> Result<Value> {
    // A broken project routing file is visible as a resettable empty editor, with
    // its exact byte token. It must not select effective user/environment rules.
    // The legacy project editor is root-scoped, not an effective/logical-root
    // routing lookup. A linked checkout without its own store retains the same
    // missing-store refusal; never open another checkout just to fill an editor.
    let directory = root.join(if workspace_protocol::channel::current().is_dev() {
        ".cortex-dev"
    } else {
        ".cortex"
    });
    let path = directory.join("routing.json");
    let (exists, token, rules) = match read_plain(&path, false) {
        Ok(Some(bytes)) => {
            let r = serde_json::from_slice::<Value>(&bytes)
                .ok()
                .filter(project_routing::valid)
                .unwrap_or(json!({}));
            (true, revision(&bytes), r)
        }
        Ok(None) => (false, "absent".into(), json!({})),
        Err(e) if e.code == "unsafe_storage" => (true, "absent".into(), json!({})),
        Err(e) => return Err(e),
    };
    if let Some(bytes) = read_plain(&owner_folder(config)?.join("connections.json"), true)? {
        super::project_registry::validate(&bytes)?;
    }
    let key = context::key(root)?;
    if key.root() != root && !directory.join("cortex.sqlite3").is_file() {
        return Err(refuse(
            "project_store_missing",
            format!(
                "{} has no Cortex project store yet; open a task there first.",
                root.display()
            ),
        ));
    }
    if key.root() != root {
        return Err(refuse(
            "project_context_conflict",
            "The project store refused the request.",
        ));
    }
    let store = context::open(root, OpenMode::LiveReadOnly)?
        .ok_or_else(|| {
            refuse(
                "project_store_missing",
                format!(
                    "{} has no Cortex project store yet; open a task there first.",
                    root.display()
                ),
            )
        })?;
    let row=store.read(|db|{
        let row=db.query_row("SELECT
            CASE WHEN length(CAST(defaults AS BLOB))+length(CAST(ceiling AS BLOB))+length(CAST(updated_at AS BLOB)) <= ?2
            THEN defaults END,
            CASE WHEN length(CAST(defaults AS BLOB))+length(CAST(ceiling AS BLOB))+length(CAST(updated_at AS BLOB)) <= ?2
            THEN ceiling END,
            CASE WHEN length(CAST(defaults AS BLOB))+length(CAST(ceiling AS BLOB))+length(CAST(updated_at AS BLOB)) <= ?2
            THEN updated_at END
            FROM project_configuration_defaults WHERE project_root=?1",
            rusqlite::params![root.to_string_lossy().as_ref(),FRAME_BUDGET as i64],
            |r|Ok((r.get::<_,Option<String>>(0)?,r.get::<_,Option<String>>(1)?,r.get::<_,Option<String>>(2)?))).optional()?;
        match row {
            Some((Some(a),Some(b),Some(c)))=>Ok(Some((a,b,c))),
            Some(_)=>Err(cortex_core::CoreError::Refused("machine_reply_too_large")),
            None=>Ok(None),
        }
    }).map_err(context::core)?;
    validate_user(config)?;
    let mut value = json!({});
    if exists {
        value["routing"] = project_routing::view(&rules);
    }
    let ceiling_token = match row {
        None => "absent".into(),
        Some((defaults, raw, updated)) => {
            let c: Value = serde_json::from_str(&raw).map_err(|_| invalid_ceiling())?;
            value["ceiling"] = ceiling(&c)?;
            let _: Value = serde_json::from_str(&defaults)
                .map_err(|_| refuse("machine_tool_failed", "JSONDecodeError"))?;
            revision(
                serde_json::to_string(&json!([defaults, raw, updated]))
                    .unwrap()
                    .as_bytes(),
            )
        }
    };
    value["revisions"] = json!({"routing":number(&token),"project_ceiling":number(&ceiling_token)});
    let reply = json!({"value":value,"tokens":{"routing":token,"project_ceiling":ceiling_token}});
    if serde_json::to_vec(&reply).unwrap().len() > FRAME_BUDGET {
        return Err(refuse(
            "machine_reply_too_large",
            "Project settings exceed the native read limit.",
        ));
    }
    Ok(reply)
}
