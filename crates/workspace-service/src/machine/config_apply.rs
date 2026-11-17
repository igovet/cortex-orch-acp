//! Absent-only preset publication for qualified public-payload/current-file shapes.
use super::{preset, profiles, *};
use std::{fs, io::Write, os::unix::fs::OpenOptionsExt};
#[cfg(test)]
#[path = "config_apply_tests.rs"]
mod tests;

fn payloads() -> Result<[Vec<u8>; 3]> {
    let [routing, defaults] = preset::payloads()?;
    let mut template: write::Ordered = serde_json::from_str(preset::TEMPLATE).unwrap();
    let write::Ordered::Object(ref mut fields) = template else {
        unreachable!()
    };
    let servers = fields
        .iter()
        .find(|(k, _)| k == "mcp_servers")
        .map(|(_, v)| v.clone())
        .unwrap_or(write::Ordered::Object(vec![]));
    let mut mcp = write::Ordered::Object(vec![
        ("version".into(), write::Ordered::Scalar(json!(1))),
        ("servers".into(), servers),
    ])
    .pretty();
    mcp.push(b'\n');
    Ok([routing, defaults, mcp])
}
// A complete fsynced temporary inode is linked into place without replacement.
// A racing existing owner file is never overwritten.
fn create(path: &Path, bytes: &[u8]) -> Result<bool> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let temporary = path.with_file_name(format!(
        ".preset-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&temporary)?;
    let outcome = (|| -> Result<bool> {
        file.write_all(bytes)?;
        file.sync_all()?;
        match fs::hard_link(&temporary, path) {
            Ok(()) => {
                fs::File::open(path.parent().unwrap())?.sync_all().map_err(|_|refuse(
                    "machine_outcome_unknown", "Preset publication may have completed; inspect settings before retrying."))?;
                Ok(true)
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e.into()),
        }
    })();
    let cleanup = fs::remove_file(&temporary);
    if cleanup.is_err() {
        return Err(refuse(
            "machine_outcome_unknown",
            "Preset publication cleanup failed; inspect settings before retrying.",
        ));
    }
    outcome
}
pub(super) fn apply(config: &crate::Config) -> Result<Value> {
    apply_with(config, &cortex_core::now(), create)
}
fn apply_with(
    config: &crate::Config,
    now: &str,
    publish: impl FnMut(&Path, &[u8]) -> Result<bool>,
) -> Result<Value> {
    apply_inner(config, now, publish, true)
}
/// The management API's apply: an existing owner file is diagnosed like
/// `ensure_model_settings` does, never replaced.
pub(super) fn apply_management(config: &crate::Config) -> Result<Value> {
    apply_inner(config, &cortex_core::now(), create, true)
}
/// `ensure_model_settings`' status of one existing owner file, when it is
/// reproducible natively (`None` otherwise: a free-form loader message).
fn existing_status(index: usize, bytes: &[u8]) -> Option<String> {
    match index {
        0 => {
            let parsed: Value = serde_json::from_slice(bytes).ok()?;
            if !parsed.is_object() {
                return None;
            }
            Some(
                if parsed["model_policy_version"].is_null() {
                    "model_policy_migration_required"
                } else if parsed["model_policy_version"].as_i64() != Some(2) {
                    "model_policy_version_unsupported"
                } else if crate::machine::project_routing::valid(&parsed) {
                    "current"
                } else {
                    return None;
                }
                .into(),
            )
        }
        1 => {
            let (values, _, _) = match crate::config_read_model::defaults_document(Some(bytes)) {
                Ok(document) => document,
                Err(e) => return e["code"].as_str().map(str::to_owned),
            };
            let mut problems = write::user_defaults::Problems::default();
            let known: serde_json::Map<String, Value> = values
                .as_object()
                .into_iter()
                .flatten()
                .filter(|(k, _)| crate::config_read_model::DEFAULT_KEYS.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            write::user_defaults::default_patch(&Value::Object(known), &mut problems);
            if problems.finish().is_err() {
                return Some("invalid_configuration".into());
            }
            let route = ["route_id", "route_connection_id", "route_model"];
            Some(
                if !values["coordinator"].is_null() || route.iter().all(|k| truth(&values[*k])) {
                    "current"
                } else {
                    "coordinator_default_missing"
                }
                .into(),
            )
        }
        _ => {
            let path = Path::new("mcp-servers.json");
            let parsed: Value = serde_json::from_slice(bytes).ok()?;
            let observed = crate::config_read_model::mcp::observation(path, Some(bytes)).ok()?;
            if observed["ok"] != true {
                return observed["error"]["code"].as_str().map(str::to_owned);
            }
            let key = observed["result"]["root_key"].as_str()?;
            // The loader's verdict on every entry, as the management writer applies it.
            for (name, spec) in parsed[key].as_object()? {
                if name.is_empty() || name == "cortex" || !spec.is_object() {
                    return Some("invalid_mcp_registry".into());
                }
                if spec["enabled"] == false {
                    continue;
                }
                if let Some(problem) = crate::config_read_model::mcp::problem(name, spec, path) {
                    return problem["code"].as_str().map(str::to_owned);
                }
            }
            Some("current".into())
        }
    }
}
fn truth(v: &Value) -> bool {
    python_truthy(v)
}
fn apply_inner(
    config: &crate::Config,
    now: &str,
    mut publish: impl FnMut(&Path, &[u8]) -> Result<bool>,
    management: bool,
) -> Result<Value> {
    let folder = reader::owner_folder(config)?;
    crate::local::private_tree(&folder)?;
    ensure_ignore(&folder)?;
    let _lock = write::owner_lock(&folder)?;
    let mut statuses: [Option<String>; 3] = [None, None, None];
    if management {
        let bytes = reader::read_plain(&folder.join("connections.json"), true)?;
        let parsed = registry_json::RegistryJson::parse(bytes.as_deref())?;
        registry::normalize(&parsed.value)?;
        for (i, file) in ["routing.json", "defaults.json", "mcp-servers.json"]
            .iter()
            .enumerate()
        {
            if let Some(bytes) = reader::read_plain(&folder.join(file), true)? {
                statuses[i] = Some(existing_status(i, &bytes).ok_or_else(|| {
                    refuse(
                        "invalid_settings",
                        format!("The existing {file} is malformed in a way Cortex cannot classify. Repair or remove {file} and retry; nothing was changed."),
                    )
                })?);
            }
        }
    }
    let payloads = payloads()?;
    let (mut before, mut reply) = preset::snapshot(&folder)?;
    let mut settings = json!({});
    let names = ["routing", "defaults", "mcp_servers", "routing_profiles"];
    let files = [
        "routing.json",
        "defaults.json",
        "mcp-servers.json",
        "routing-profiles.json",
    ];
    let mut plans: Vec<(usize, Vec<u8>)> = vec![];
    for i in 0..3 {
        let missing = before[i].is_none();
        let status = if missing {
            "created".to_owned()
        } else if let Some(status) = statuses[i].take() {
            status
        } else if i == 0
            && serde_json::from_slice::<Value>(before[i].as_ref().unwrap()).unwrap() == json!({})
        {
            "model_policy_migration_required".to_owned()
        } else {
            "current".to_owned()
        };
        settings[names[i]] =
            json!({"path":folder.join(files[i]),"status":status,"changed":missing});
        if missing {
            before[i] = Some(payloads[i].clone());
            plans.push((i, payloads[i].clone()));
        }
    }
    let section_bytes = [
        before[0].clone(),
        before[1].clone(),
        before[2].clone(),
        reader::read_plain(&folder.join("connections.json"), true)?,
    ];
    // Python reports a refused profile seed as that file's status.
    let mut seed_refusal = None;
    let profile = if before[3].is_none() {
        match profiles::seed_default(&section_bytes, now) {
            Err(e) if management => {
                seed_refusal = Some(e.code);
                None
            }
            seeded => seeded?,
        }
    } else {
        None
    };
    let profile_status = if before[3].is_some() {
        "existing"
    } else if profile.is_some() {
        "created"
    } else {
        seed_refusal.unwrap_or("custom_settings")
    };
    settings[names[3]] =
        json!({"path":folder.join(files[3]),"status":profile_status,"changed":profile.is_some()});
    if let Some(body) = profile {
        before[3] = Some(body.clone());
        plans.push((3, body));
    }
    // Budget before publishing any setting; no post-effect delegation.
    for i in 0..4 {
        reply["files"][names[i]] = json!({"revision":write::token(before[i].as_deref()),
            "status":if before[i].is_some() {"existing"}else{"absent"}});
    }
    reply["revision"] = json!(preset::combined_revision(&before));
    reply["settings"] = settings;
    reply["changed"] = json!(!plans.is_empty());
    if serde_json::to_vec(&reply).unwrap().len() > FRAME_BUDGET {
        return Err(refuse(
            "machine_reply_too_large",
            "Settings exceed the native write limit.",
        ));
    }
    for (published, (index, body)) in plans.into_iter().enumerate() {
        let created = publish(&folder.join(files[index]), &body).map_err(|error| {
            if published > 0 { refuse("machine_outcome_unknown",
                "Some preset files were created before publication failed. Inspect settings before retrying.") } else { error }
        })?;
        if !created {
            return Err(refuse(
                "machine_outcome_unknown",
                "Preset publication met a concurrent owner file; inspect settings before retrying.",
            ));
        }
    }
    Ok(reply)
}
