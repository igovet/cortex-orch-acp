//! Offline configuration observations. Not a route authorizer or a machine dispatcher.
//! Explicit host roots only; never seeds, repairs or launches a process.
//! Setup display may nonblockingly probe an existing cooperative setup lock.
mod catalog;
mod connections;
mod defaults;
pub(crate) mod management;
pub(crate) mod mcp;
pub(crate) mod ordered;
mod profiles;
mod readers;
mod routing;
mod settings;
mod setup;
pub mod usage_refresh;
mod workspace;
use crate::native_routes::{read, RouteEnvironment};
pub use connections::{parse_connections, route_fingerprint};
use cortex_core::{CoreError, CoreResult};
pub(crate) use defaults::{defaults_document, DEFAULT_KEYS};
pub(crate) use management::canonical as python_json;
pub use profiles::parse_profiles;
pub(crate) use readers::management_view as connection_management_view;
pub use readers::SettingsRead;
use serde_json::{json, Value};
pub use settings::settings_view;
pub use setup::SetupEnvironment;
use std::{os::unix::fs::MetadataExt, path::Path};

/// Management profiles validate tokens more strictly than the project editor.
pub(crate) fn profile_routing_syntax(r: &Value) -> bool {
    fn token(v: &Value) -> bool {
        v.as_str()
            .is_some_and(|s| matches(r"^[^\s\x00-\x1f\x7f]{1,256}$", s))
    }
    fn exact(s: &str) -> bool {
        s.contains('/') && !s.starts_with('/') && !s.ends_with('/')
    }
    if r["pin"].as_object().is_some_and(|m| {
        m.iter().any(|(role, v)| {
            if !matches(r"^(\*|[A-Za-z0-9][A-Za-z0-9._-]{0,127})$", role) || !token(v) {
                return true;
            }
            let s = v.as_str().unwrap();
            let (route, effort) = s
                .split_once(':')
                .map(|(a, b)| (a, Some(b)))
                .unwrap_or((s, None));
            let Some((engine, model)) = route.split_once('/') else {
                return true;
            };
            !ENGINES.contains(&engine)
                || model.is_empty()
                || (engine == "opencode" && !exact(model))
                || effort.is_some_and(|s| !["low", "medium", "high", "xhigh", "max"].contains(&s))
        })
    }) {
        return false;
    }
    for key in ["deny", "auto_allow"] {
        if let Some(a) = r[key].as_array() {
            let mut seen = std::collections::BTreeSet::new();
            for v in a {
                if !token(v) || !seen.insert(v.to_string()) {
                    return false;
                }
                let s = v.as_str().unwrap();
                let pair = s.split_once('/');
                if s.contains('*') {
                    if !pair.is_some_and(|(e, m)| ENGINES.contains(&e) && m == "*") {
                        return false;
                    }
                } else if pair.is_some_and(|(e, m)| {
                    !ENGINES.contains(&e)
                        || m.is_empty()
                        || (e == "opencode" && key == "auto_allow" && !exact(m))
                }) {
                    return false;
                }
            }
        }
    }
    if r["tiers"]
        .as_object()
        .is_some_and(|m| m.keys().any(|s| !token(&json!(s))))
    {
        return false;
    }
    if r["providers"].as_array().is_some_and(|a| {
        a.iter()
            .map(Value::to_string)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != a.len()
    }) {
        return false;
    }
    if r["enabled"]["opencode"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| !v.as_str().is_some_and(exact)))
    {
        return false;
    }
    if let Some(config) = r["models"]["opencode"].as_object() {
        for key in ["auto", "tiers", "policy"] {
            let ids: Vec<&str> = if key == "auto" {
                config
                    .get(key)
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect()
            } else {
                config
                    .get(key)
                    .and_then(Value::as_object)
                    .into_iter()
                    .flat_map(|m| m.keys().map(String::as_str))
                    .collect()
            };
            if ids.iter().any(|s| !exact(s)) {
                return false;
            }
        }
    }
    true
}
pub type ModelResult<T> = Result<T, &'static str>;
pub(crate) const ENGINES: [&str; 3] = ["codex-acp", "claude-agent-acp", "opencode"];
pub(crate) fn matches(pattern: &str, s: &str) -> bool {
    regex::Regex::new(pattern).unwrap().is_match(s)
}
pub(crate) fn clean(v: &Value, limit: usize) -> bool {
    v.as_str().is_some_and(|s| {
        !s.trim().is_empty()
            && s.chars().count() <= limit
            && !s.chars().any(|c| c < ' ' || c == '\u{7f}')
    })
}
pub(crate) fn integer(v: &Value, max: u64) -> bool {
    v.as_u64().is_some_and(|v| v <= max)
}
/// No credential bytes are opened. The host must provide an already canonical,
/// trusted native directory; this observation does not validate a registry route.
pub fn native_login_marker(engine: &str, directory: &Path) -> &'static str {
    let marker = match engine {
        "codex-acp" | "opencode" => "auth.json",
        "claude-agent-acp" => ".credentials.json",
        _ => return "unavailable",
    };
    if !directory.is_absolute() {
        return "invalid";
    }
    match std::fs::symlink_metadata(directory) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return "authorization_required",
        Err(_) => return "invalid",
        Ok(_) => (),
    }
    if directory
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return "invalid";
    }
    for p in directory.ancestors() {
        if !std::fs::symlink_metadata(p).is_ok_and(|m| m.is_dir() && !m.file_type().is_symlink()) {
            return "invalid";
        }
    }
    if !directory
        .metadata()
        .is_ok_and(|m| m.uid() == unsafe { libc::getuid() } && m.mode() & 0o077 == 0)
    {
        return "invalid";
    }
    match std::fs::symlink_metadata(directory.join(marker)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "authorization_required",
        Ok(m) if m.is_file() && !m.file_type().is_symlink() => "configured",
        _ => "invalid",
    }
}
/// Version policy is an install marker, not byte/instruction/live qualification.
pub fn adapter_version(engine: &str, version: &str) -> Value {
    let qualified = match engine {
        "codex-acp" => ["1.12.0", "1.13.0", "2.0.1", "2.1.1", "2.3.0", "2.6.0"].contains(&version),
        "claude-agent-acp" => {
            ["0.78.0", "0.87.0", "0.88.0", "0.97.0", "0.107.0"].contains(&version)
        }
        "opencode" => matches(r"^2\.[0-9]+\.[0-9]+$", version),
        _ => false,
    };
    let pinned = if engine == "claude-agent-acp" {
        adapter_version_base(version.split("+sdk.").next().unwrap_or(version))
    } else {
        qualified && engine == "codex-acp"
    };
    json!({"qualified":qualified,"pinned":pinned})
}
/// Parse only the declared unambiguous package bin, never execute it.
pub fn adapter_manifest(value: &Value) -> Option<(String, String)> {
    let version = value["version"].as_str().filter(|s| !s.is_empty())?;
    let bin = &value["bin"];
    let target = if let Some(s) = bin.as_str() {
        s
    } else {
        let map = bin.as_object()?;
        if map.len() != 1 {
            return None;
        }
        map.values().next()?.as_str()?
    };
    if target.is_empty() {
        return None;
    }
    Some((version.to_owned(), target.to_owned()))
}
fn adapter_version_base(version: &str) -> bool {
    ["0.78.0", "0.87.0", "0.88.0", "0.97.0", "0.107.0"].contains(&version)
}
/// Filter an already captured cache, without refreshing it or estimating use.
pub fn usage_windows(engine: &str, windows: &Value, now: f64) -> Value {
    if !["codex-acp", "claude-agent-acp"].contains(&engine) {
        return json!([]);
    }
    let mut result = Vec::new();
    if let Some(map) = windows.as_object() {
        for (name, w) in map {
            if name.to_lowercase().contains("overage")
                || (engine == "claude-agent-acp" && name == "five_hour")
            {
                continue;
            }
            let Some(used) = w["used"].as_f64().filter(|n| (0.0..=100.0).contains(n)) else {
                continue;
            };
            let reset = w["resets_at"].as_f64();
            if reset.is_some_and(|n| n <= now) {
                continue;
            }
            let mut item = json!({"name":name,"usedPercent":used,"resetsAt":if reset.is_some() {w["resets_at"].clone()} else {Value::Null}});
            if w["window_minutes"].as_u64().is_some_and(|n| n > 0) {
                item["windowMinutes"] = w["window_minutes"].clone();
            }
            if w["observed_at"]
                .as_f64()
                .is_some_and(|n| n.is_finite() && n > 0.0)
            {
                item["observedAt"] = w["observed_at"].clone();
            }
            result.push(item);
        }
    }
    json!(result)
}
pub fn context_window(windows: &Value, model: Option<&str>) -> Option<u64> {
    windows[model.unwrap_or("")].as_u64().filter(|n| *n > 0)
}
/// Explicit-root read model; no environment access and no implicit fallback roots.
pub struct ConfigurationReader {
    environment: RouteEnvironment,
}
impl ConfigurationReader {
    pub fn new(environment: RouteEnvironment) -> CoreResult<Self> {
        environment.validate()?;
        Ok(Self { environment })
    }
    pub fn profiles(&self) -> CoreResult<Value> {
        let v = read(
            &self.environment.config.join("cortex/routing-profiles.json"),
            true,
        )?;
        parse_profiles(v.as_ref()).map_err(CoreError::Refused)
    }
    pub fn connections(&self) -> CoreResult<Value> {
        let v = read(
            &self.environment.config.join("cortex/connections.json"),
            true,
        )?;
        parse_connections(v.as_ref()).map_err(CoreError::Refused)
    }
    fn cache(&self, directory: &str, id: &str) -> CoreResult<Value> {
        if !matches(r"^cn_[a-f0-9]{12}$", id) {
            return Err(CoreError::Refused("invalid_connection_id"));
        }
        Ok(read(
            &self
                .environment
                .state
                .join("cortex")
                .join(directory)
                .join(format!("{id}.json")),
            true,
        )?
        .unwrap_or(json!({})))
    }
    /// Cache-only. Codex rollout scanning is deliberately a separate producer.
    pub fn usage_cache(&self, id: &str, engine: &str, now: f64) -> CoreResult<Value> {
        Ok(usage_windows(engine, &self.cache("quota", id)?, now))
    }
    pub fn context(&self, id: &str, model: Option<&str>) -> CoreResult<Option<u64>> {
        Ok(context_window(&self.cache("context_windows", id)?, model))
    }
}
