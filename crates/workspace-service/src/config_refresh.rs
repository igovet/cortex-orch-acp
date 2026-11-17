//! Explicit host-only filesystem-credential refresh. Never called by display reads.
//! Cooperative registry arbitration, not credential/account or executable validation.
use crate::{
    config_read_model::{native_login_marker, parse_connections, route_fingerprint},
    native_routes::{read_bytes, RouteEnvironment},
};
use cortex_core::{
    store::locks::{private_directory, AccessLock},
    CoreError, CoreResult,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

fn revision(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
fn refused<T>(code: &'static str) -> CoreResult<T> {
    Err(CoreError::Refused(code))
}
fn safe_directory(path: &Path) -> bool {
    path.ancestors()
        .all(|p| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()))
        && fs::symlink_metadata(path)
            .is_ok_and(|m| m.uid() == unsafe { libc::getuid() } && m.mode() & 0o077 == 0)
}
fn expected_path(env: &RouteEnvironment, r: &Value) -> Option<std::path::PathBuf> {
    let id = r["id"].as_str()?;
    let engine = r["engine"].as_str()?;
    match r["credential_ref"]["kind"].as_str()? {
        "secret_file" => Some(env.config.join("cortex/secrets").join(id)),
        "engine_native_dir" => {
            let owned = r["credential_ref"]["owned"] == true;
            let suffix = match (engine, owned) {
                ("codex-acp", true) => "codex-home",
                ("claude-agent-acp", true) => "claude-config",
                ("opencode", true) => "opencode",
                ("codex-acp", false) => ".codex",
                ("claude-agent-acp", false) => ".claude",
                ("opencode", false) => ".local/share/opencode",
                _ => return None,
            };
            Some(if owned {
                env.state.join("cortex/connections").join(id).join(suffix)
            } else {
                env.home.join(suffix)
            })
        }
        _ => None,
    }
}
fn collides(r: &Value, records: &serde_json::Map<String, Value>) -> bool {
    let reference = &r["credential_ref"];
    // Referenced client-native directories intentionally share one per engine.
    if reference["kind"] == "engine_native_dir" && reference["owned"] == false {
        return false;
    }
    records.values().any(|other| {
        other["id"] != r["id"]
            && other["credential_ref"]["path"] == reference["path"]
            && !(other["credential_ref"]["kind"] == "engine_native_dir"
                && other["credential_ref"]["owned"] == false)
    })
}
pub(crate) fn derive(
    env: &RouteEnvironment,
    r: &Value,
    records: &serde_json::Map<String, Value>,
) -> &'static str {
    let Some(expected) = expected_path(env, r) else {
        return "invalid";
    };
    if r["credential_ref"]["path"].as_str() != expected.to_str() || collides(r, records) {
        return "invalid"; // Never stat a noncanonical or colliding credential target.
    }
    if r["credential_ref"]["kind"] == "engine_native_dir" {
        return native_login_marker(r["engine"].as_str().unwrap(), &expected);
    }
    if !safe_directory(expected.parent().unwrap()) {
        return "invalid";
    }
    match fs::symlink_metadata(expected) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "authorization_required",
        Ok(m)
            if m.is_file()
                && !m.file_type().is_symlink()
                && m.nlink() == 1
                && m.uid() == unsafe { libc::getuid() }
                && m.mode() & 0o077 == 0 =>
        {
            if m.len() == 0 {
                "authorization_required"
            } else {
                "configured"
            }
        }
        _ => "invalid",
    }
}
fn fingerprint_matches(r: &Value) -> bool {
    let launch = r["launch_command"].as_array().map(|a| {
        a.iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect::<Vec<_>>()
    });
    r["approved_fingerprint"]
        == route_fingerprint(
            r["engine"].as_str().unwrap(),
            r["provider"].as_str().unwrap(),
            r["endpoint"].as_str(),
            launch.as_deref(),
        )
}
fn replace(
    path: &Path,
    before: &[u8],
    bytes: &[u8],
    finish: impl FnOnce(&Path) -> CoreResult<()>,
) -> CoreResult<()> {
    if bytes.len() > 4 * 1024 * 1024 {
        return refused("registry_too_large");
    }
    let parent = path.parent().unwrap();
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let tmp = parent.join(format!(
        ".connections.json.{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let mut renamed = false;
    let mut created = false;
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&tmp)?;
        created = true;
        file.write_all(bytes)?;
        file.sync_all()?;
        // Refuse noncooperating replacements detected before commit, too.
        if read_bytes(path, true)?.as_deref() != Some(before) {
            return refused("revision_conflict");
        }
        fs::rename(&tmp, path)?;
        renamed = true;
        finish(parent)
    })();
    // Only remove our own successfully-created temporary file.
    if created && !renamed {
        let _ = fs::remove_file(&tmp);
    }
    if renamed && result.is_err() {
        refused("machine_outcome_unknown")
    } else {
        result
    }
}

/// Refresh selected filesystem-backed connections under the Python-compatible
/// connections.lock. Explicit host action only: not a socket/model operation.
/// Existing private roots/registry are required; no repair, setup, keyring, CLI,
/// quota refresh, fingerprint approval or Python replay is performed.
/// The caller supplies its exact read revision and a trusted UTC timestamp.
/// A post-rename durability failure returns machine_outcome_unknown: re-read,
/// never retry automatically. Returned states are not dispatch authorization.
pub fn refresh_connections(
    environment: &RouteEnvironment,
    ids: &[String],
    expected_revision: &str,
    now: &str,
    wait: Duration,
) -> CoreResult<Value> {
    environment.validate()?;
    if ids.is_empty()
        || ids.len() > 4096
        || ids.iter().collect::<BTreeSet<_>>().len() != ids.len()
        || ids.iter().any(|id| {
            !regex::Regex::new(r"^cn_[a-f0-9]{12}$")
                .unwrap()
                .is_match(id)
        })
        || wait > Duration::from_secs(25)
        || !regex::Regex::new(r"^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d{1,6})?(?:Z|\+00:00)$")
            .unwrap()
            .is_match(now)
    {
        return refused("invalid_refresh_request");
    }
    let root = environment.config.join("cortex");
    private_directory(&root, false)?;
    let _lock = AccessLock::acquire(&root.join("connections.lock"), true, true, wait)?;
    let path = root.join("connections.json");
    let bytes = read_bytes(&path, true)?.ok_or(CoreError::Refused("registry_missing"))?;
    if revision(&bytes) != expected_revision {
        return refused("revision_conflict");
    }
    let mut raw: Value =
        serde_json::from_slice(&bytes).map_err(|_| CoreError::Refused("invalid_registry"))?;
    let registry = parse_connections(Some(&raw)).map_err(CoreError::Refused)?;
    let records = registry["connections"].as_object().unwrap();
    // Complete preflight before any credential-target IO or registry mutation.
    for id in ids {
        let r = records
            .get(id)
            .ok_or(CoreError::Refused("connection_unavailable"))?;
        if r["credential_ref"]["kind"] == "os_keyring" {
            return refused("keyring_refresh_unsupported");
        }
        if !fingerprint_matches(r) {
            return refused("connection_fingerprint_changed");
        }
    }
    let mut changed = false;
    let mut states = Vec::new();
    for id in ids {
        let state = derive(environment, &records[id], records);
        if records[id]["state"] != state {
            raw["connections"][id]["state"] = json!(state);
            raw["connections"][id]["updated_at"] = json!(now);
            changed = true;
        }
        states.push(json!({"id":id,"state":state}));
    }
    let output = if changed {
        let bytes_out =
            serde_json::to_vec(&raw).map_err(|_| CoreError::Refused("invalid_registry"))?;
        replace(&path, &bytes, &bytes_out, |parent| {
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })?;
        bytes_out
    } else {
        bytes
    };
    Ok(json!({"changed":changed,"revision":revision(&output),"connections":states}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    #[test]
    fn replacement_failure_keeps_precommit_and_fences_postcommit() {
        let root = std::env::temp_dir().join(format!("cw-rf-unit-{}", std::process::id()));
        fs::create_dir(&root).unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
        let folder = root.join("cortex");
        fs::create_dir(&folder).unwrap();
        fs::set_permissions(&folder, fs::Permissions::from_mode(0o700)).unwrap();
        let path = folder.join("connections.json");
        fs::write(&path, b"before").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let error = replace(&path, b"stale", b"after", |_| {
            panic!("must not reach commit")
        })
        .unwrap_err();
        assert_eq!(error.code(), "revision_conflict");
        assert_eq!(fs::read(&path).unwrap(), b"before");
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        let error = replace(&path, b"before", b"after", |_| {
            refused("injected_directory_sync_failure")
        })
        .unwrap_err();
        assert_eq!(error.code(), "machine_outcome_unknown");
        assert_eq!(fs::read(&path).unwrap(), b"after");
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }
}
