//! Native connection discovery as the product performs it: the service's agent child runs the
//! startup first-run pass, which registers `connections.json` entries only for an existing native
//! login marker and records `first-run-<engine>.json`. This module observes those files; it never
//! writes a registry itself and never calls a model.

use super::util::*;
use crate::json::{parse, Res, Value};
use std::path::{Path, PathBuf};

pub const ENGINES: [(&str, &str, &str); 2] = [("codex-acp", ".codex", "auth.json"), ("claude-agent-acp", ".claude", ".credentials.json")];

/// Synthetic owned login markers; never a real account.
pub fn write_markers(home: &Path) -> Res<()> {
    for (_, dir, marker) in ENGINES {
        let native = home.join(dir);
        std::fs::create_dir_all(&native).map_err(|e| e.to_string())?;
        chmod(&native, 0o700)?;
        write(&native.join(marker), "{}")?;
        chmod(&native.join(marker), 0o600)?;
    }
    Ok(())
}

pub fn registry(config: &Path) -> Res<Vec<(String, Value)>> {
    let path = config.join("connections.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let doc = parse(&read_to_string(&path)?)?;
    let items = doc.get("connections").and_then(Value::as_obj).ok_or("registry has no connections object")?;
    Ok(items.to_vec())
}

#[allow(dead_code)]
pub fn status_phase(config: &Path, engine: &str) -> Option<String> {
    let text = std::fs::read_to_string(config.join(format!("first-run-{engine}.json"))).ok()?;
    parse(&text).ok()?.get("phase").and_then(Value::as_str).map(str::to_string)
}

/// Ask the service's own engine check, exactly as the Settings "recheck" action does. A synthetic
/// login marker is not a login, so the native CLI check normally refuses with
/// `native_login_required`: that proves the qualified bundled adapter and its native binary were
/// found and executed, and the registry must stay empty for that engine. A runtime that instead
/// accepts the marker must have registered exactly one configured, unowned native-account
/// connection launching the bundled Node. Any other answer is a hard failure.
pub fn recheck_without_real_login(socket: &Path, config: &Path, package: &str) -> Res<Vec<(String, String)>> {
    let node = format!("/usr/lib/{package}/engines/node/bin/node");
    let mut seen = Vec::new();
    for (engine, _, _) in ENGINES {
        let params = obj(vec![("op", s("engine.connection")), ("params", obj(vec![("action", s("recheck")), ("engine", s(engine))]))]);
        let reply = match super::seed_large::rpc(socket, "machine.call", params) {
            Ok(v) => line(&v),
            Err(e) => e,
        };
        let mine: Vec<(String, Value)> = registry(config)?.into_iter().filter(|(_, r)| r.get("engine").and_then(Value::as_str) == Some(engine)).collect();
        let answer = if reply.contains("native_login_required") {
            require(mine.is_empty(), &format!("{engine}: a connection exists although the native login check refused"))?;
            "native_login_required"
        } else if reply.contains("connectionId") {
            require(mine.len() == 1, &format!("{engine}: the check accepted the marker but registered {} connections", mine.len()))?;
            let r = &mine[0].1;
            require(matches!(r.get("credential_ref").and_then(|c| c.get("owned")), Some(Value::Bool(false))), &format!("{engine}: connection claims ownership of the native login"))?;
            require(r.get("state").and_then(Value::as_str) == Some("configured"), &format!("{engine}: connection is not configured"))?;
            require(r.get("auth_mode").and_then(Value::as_str) == Some("native_account"), &format!("{engine}: connection is not a native account"))?;
            let launch = r.get("launch_command").and_then(Value::as_arr).and_then(|a| a.first()).and_then(Value::as_str);
            require(launch == Some(node.as_str()), &format!("{engine}: connection does not launch the bundled Node"))?;
            "registered_configured_unowned"
        } else {
            return Err(format!("{engine}: unexpected recheck answer: {}", tail(&reply)));
        };
        seen.push((engine.to_string(), answer.to_string()));
    }
    Ok(seen)
}

pub fn config_dir(home: &Path, package: &str) -> PathBuf {
    let base = home.join(".config");
    (if package.ends_with("-dev") { base.join(package) } else { base }).join("cortex")
}
