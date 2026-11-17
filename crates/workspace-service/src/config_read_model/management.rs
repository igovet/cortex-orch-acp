//! Read-only profile snapshots and management composition. No activation or persistence.
use super::{
    defaults::{defaults_document, empty_ceiling, DEFAULT_KEYS},
    profiles::{self, SECTIONS},
    ConfigurationReader, SettingsRead,
};
use crate::native_routes::read_bytes;
use cortex_core::{CoreError, CoreResult};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

pub(super) fn revision(bytes: Option<&[u8]>) -> String {
    bytes
        .map(|b| format!("sha256:{:x}", Sha256::digest(b)))
        .unwrap_or("absent".into())
}
pub(super) fn error(code: &str, message: &str, field: Option<&str>) -> Value {
    json!({"category":"validation","code":code,"message":message,"field":field,"retry_eligible":false})
}
pub(super) fn failure(e: Value) -> Value {
    json!({"ok":false,"error":e})
}
pub(super) fn json_object(
    bytes: Option<&[u8]>,
    name: &str,
    code: &str,
    absent: Value,
) -> Result<Value, Value> {
    let Some(bytes) = bytes else {
        return Ok(absent);
    };
    let v: Value = serde_json::from_slice(bytes).map_err(|_| {
        error(
            code,
            &format!("{name} is not valid UTF-8 JSON; repair it by hand. Nothing was changed."),
            None,
        )
    })?;
    if !v.is_object() {
        return Err(error(
            code,
            &format!("{name} must contain a JSON object; repair it by hand. Nothing was changed."),
            None,
        ));
    }
    Ok(v)
}
fn invalid_capture() -> Value {
    error(
        "invalid_configuration",
        "The current section cannot be represented as a profile; repair it first.",
        None,
    )
}
pub(crate) fn capture(section: &str, bytes: Option<&[u8]>) -> Result<Value, Value> {
    let v = match section {
        "user_defaults" => {
            let (d, c, _) = defaults_document(bytes)?;
            let mut captured = json!({});
            for k in DEFAULT_KEYS {
                if !d[k].is_null()
                    && (k != "parameters" || d[k].as_object().is_none_or(|m| !m.is_empty()))
                {
                    captured[k] = d[k].clone();
                }
            }
            json!({"defaults":captured,"ceiling":c})
        }
        "routing" => {
            let d = json_object(
                bytes,
                "routing.json",
                "invalid_routing_overrides",
                json!({}),
            )?;
            let mut rules = json!({});
            for k in [
                "model_policy_version",
                "pin",
                "deny",
                "enabled",
                "auto_allow",
                "tiers",
                "providers",
                "engine_affinity",
                "models",
                "quota_windows",
            ] {
                if let Some(v) = d.get(k) {
                    rules[k] = v.clone();
                }
            }
            if !crate::machine::project_routing::valid_profile(&d) {
                return Err(error(
                    "invalid_routing_overrides",
                    "The user routing.json is invalid; repair it first.",
                    None,
                ));
            }
            rules
        }
        "mcp_servers" => {
            let d = json_object(
                bytes,
                "mcp-servers.json",
                "invalid_mcp_registry",
                json!({"version":1,"servers":{}}),
            )?;
            let key = if d.get("servers").is_some() {
                "servers"
            } else {
                "mcpServers"
            };
            let servers = d[key].as_object().ok_or_else(|| {
                error(
                    "invalid_mcp_registry",
                    "mcp-servers.json must hold a 'servers' object; repair it by hand.",
                    None,
                )
            })?;
            let mut captured = json!({});
            for (name, spec) in servers {
                let m = spec.as_object().ok_or_else(|| {
                    error(
                        "invalid_mcp_registry",
                        "mcp-servers.json has a server entry that is not an object.",
                        None,
                    )
                })?;
                let mut policy = json!({"enabled":spec["enabled"] != false});
                for k in [
                    "allowed_tools",
                    "read_only_tools",
                    "coordinator_tools",
                    "always_load_tools",
                    "read_paths",
                    "write_paths",
                    "network",
                    "runtime_dir_env",
                    "roles",
                    "engines",
                ] {
                    if let Some(v) = m.get(k) {
                        policy[k] = v.clone();
                    }
                }
                captured[name] = policy;
            }
            captured
        }
        _ => {
            let parsed = if bytes.is_some_and(|b| !b.is_empty()) {
                Some(json_object(
                    bytes,
                    "connections.json",
                    "invalid_registry",
                    json!({}),
                )?)
            } else {
                None
            };
            let registry = super::parse_connections(parsed.as_ref()).map_err(|_| {
                error(
                    "invalid_registry",
                    "The connection registry is invalid; repair it first.",
                    None,
                )
            })?;
            let mut captured = json!({});
            for (id, r) in registry["connections"].as_object().unwrap() {
                captured[id] = json!({"max_concurrent_sessions":r["max_concurrent_sessions"]});
            }
            captured
        }
    };
    profiles::normalize(section, &v).map_err(|_| invalid_capture())
}
pub(crate) struct Snapshot {
    pub(crate) revision: Value,
    pub(crate) capture: Result<Value, Value>,
}
fn snapshot(reader: &ConfigurationReader, routing: Option<&[u8]>) -> Vec<Snapshot> {
    SECTIONS
        .iter()
        .zip([
            "routing.json",
            "defaults.json",
            "mcp-servers.json",
            "connections.json",
        ])
        .map(|(section, file)| {
            match if *section == "routing" && routing.is_some() {
                Ok(routing.map(<[u8]>::to_vec))
            } else {
                read_bytes(&reader.environment.config.join("cortex").join(file), true)
            } {
                Ok(bytes) => Snapshot {
                    revision: json!(revision(bytes.as_deref())),
                    capture: capture(section, bytes.as_deref()),
                },
                Err(_) => Snapshot {
                    revision: Value::Null,
                    capture: Err(error(
                        "unsafe_storage",
                        "The current section is not a safe readable owner file.",
                        None,
                    )),
                },
            }
        })
        .collect()
}
fn compare(section: &str, saved: &Value, current: &Value) -> (Vec<String>, Vec<String>) {
    let mut keys = vec![];
    let mut notes = vec![];
    match section {
        "user_defaults" => {
            for (group, members) in [
                (
                    "route",
                    &[
                        "coordinator",
                        "route_id",
                        "route_connection_id",
                        "route_model",
                    ][..],
                ),
                ("parameters", &["parameters"][..]),
                ("language", &["language"][..]),
            ] {
                if members
                    .iter()
                    .any(|k| saved["defaults"][*k] != current["defaults"][*k])
                {
                    keys.push(group.into());
                }
            }
            if saved["ceiling"] != current["ceiling"] {
                keys.push("ceiling".into());
            }
        }
        "connections" => {
            for (id, v) in saved.as_object().unwrap() {
                if current.get(id).is_none() {
                    notes.push(format!("connection_missing:{id}"));
                } else if current[id] != *v {
                    keys.push(id.clone());
                }
            }
        }
        _ => {
            let names: BTreeSet<_> = saved
                .as_object()
                .unwrap()
                .keys()
                .chain(current.as_object().unwrap().keys())
                .collect();
            for name in names {
                if saved[name] != current[name]
                    && (section != "mcp_servers"
                        || saved.get(name).is_some()
                        || current[name]["enabled"] == true)
                {
                    keys.push(name.clone());
                }
            }
        }
    }
    (keys, notes)
}
// Match Python's sorted compact JSON, including exponent spelling for preserved floats.
pub(crate) fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}:{}", json!(k), canonical(v)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        Value::Number(n) if n.is_f64() => {
            let raw = n.to_string();
            let (sign, raw) = raw
                .strip_prefix('-')
                .map(|s| ("-", s))
                .unwrap_or(("", raw.as_str()));
            let (base, power) = raw
                .split_once('e')
                .or_else(|| raw.split_once('E'))
                .map(|(b, p)| (b, p.parse::<i32>().unwrap()))
                .unwrap_or((raw, 0));
            let point = base.find('.').unwrap_or(base.len()) as i32;
            let all = base.replace('.', "");
            let leading = all.bytes().take_while(|b| *b == b'0').count();
            let digits = all[leading..].trim_end_matches('0');
            if digits.is_empty() {
                return format!("{sign}0.0");
            }
            let exponent = power + point - leading as i32 - 1;
            if !(-4..16).contains(&exponent) {
                let tail = if digits.len() > 1 {
                    format!(".{}", &digits[1..])
                } else {
                    String::new()
                };
                format!("{sign}{}{tail}e{exponent:+03}", &digits[..1])
            } else if exponent < 0 {
                format!("{sign}0.{}{digits}", "0".repeat((-exponent - 1) as usize))
            } else {
                let n = exponent as usize + 1;
                if digits.len() <= n {
                    format!("{sign}{digits}{}.0", "0".repeat(n - digits.len()))
                } else {
                    format!("{sign}{}.{}", &digits[..n], &digits[n..])
                }
            }
        }
        _ => v.to_string(),
    }
}
fn summary(entry: &Value) -> Value {
    let settings = &entry["settings"];
    let rules = &settings["routing"];
    let mut selected = json!({});
    if let Some(models) = rules["models"].as_object() {
        for (engine, config) in models {
            if let Some(a) = config["auto"].as_array() {
                selected[engine] = json!(a.len());
            }
        }
    }
    let denied: Vec<_> = super::ENGINES
        .iter()
        .filter(|engine| {
            rules["deny"]
                .as_array()
                .is_some_and(|a| a.contains(&json!(format!("{engine}/*"))))
        })
        .collect();
    let enabled = settings["mcp_servers"]
        .as_object()
        .into_iter()
        .flat_map(|m| m.values())
        .filter(|v| v["enabled"] == true)
        .count();
    json!({"providers":rules["providers"],"denied_engines":denied,"selected_models":selected,
        "default_model":settings["user_defaults"]["defaults"]["route_model"],"language":settings["user_defaults"]["defaults"]["language"],
        "enabled_mcp_servers":enabled,"ceiling_set":settings.get("user_defaults").is_some() && settings["user_defaults"]["ceiling"]!=empty_ceiling()})
}
impl ConfigurationReader {
    fn profile_shadow(&self, project: Option<(&Path, &Path)>) -> CoreResult<Value> {
        let mut paths = Vec::new();
        if let Some((root, path)) = project {
            if !root.is_absolute()
                || root
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(CoreError::Refused("invalid_project_root"));
            }
            paths.push(("project", path));
        }
        if let Some(path) = &self.environment.routing_override {
            paths.push(("env", path.as_path()));
        }
        for (label, path) in paths {
            if !path.is_absolute()
                || path
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                return Err(CoreError::Refused("unsafe_storage"));
            }
            for parent in path.ancestors() {
                match std::fs::symlink_metadata(parent) {
                    Ok(m) if m.file_type().is_symlink() => {
                        return Err(CoreError::Refused("unsafe_storage"))
                    }
                    Err(e) if e.kind() != std::io::ErrorKind::NotFound => {
                        return Err(CoreError::Refused("unsafe_storage"))
                    }
                    _ => (),
                }
            }
            if path.is_file() {
                return Ok(json!(label));
            }
        }
        Ok(Value::Null)
    }
    /// Snapshot/status/differences are advisory; no profile activation, lock or write.
    pub fn profiles_management_observation(
        &self,
        project: Option<(&Path, &Path)>,
    ) -> CoreResult<Value> {
        self.profiles_with_routing(project, None)
    }
    fn profiles_with_routing(
        &self,
        project: Option<(&Path, &Path)>,
        routing: Option<&[u8]>,
    ) -> CoreResult<Value> {
        let path = self.environment.config.join("cortex/routing-profiles.json");
        let bytes = read_bytes(&path, true)?;
        let input = match json_object(
            bytes.as_deref(),
            "routing-profiles.json",
            "invalid_profiles",
            Value::Null,
        ) {
            Ok(v) => v,
            Err(e) => return Ok(failure(e)),
        };
        let parsed = match super::parse_profiles((!input.is_null()).then_some(&input)) {
            Ok(v) => v,
            Err(code) => {
                return Ok(failure(error(
                    code,
                    if code == "unsupported_profiles_format" {
                        "routing-profiles.json format must be 2 (or 1, which is migrated on the first change)."
                    } else {
                        "Routing profiles contain invalid metadata or settings; repair them first."
                    },
                    (code == "unsupported_profiles_format").then_some("format"),
                )))
            }
        };
        let current = snapshot(self, routing);
        let shadow = self.profile_shadow(project)?;
        Ok(profile_view(
            &path,
            bytes.as_deref(),
            &parsed,
            &current,
            shadow,
        ))
    }
}

pub(crate) fn profile_view(
    path: &Path,
    bytes: Option<&[u8]>,
    parsed: &Value,
    current: &[Snapshot],
    shadow: Value,
) -> Value {
    let doc = &parsed["document"];
    let mut problems = vec![];
    let mut revisions = json!({});
    for (section, item) in SECTIONS.iter().zip(current) {
        revisions[*section] = item.revision.clone();
        if let Err(e) = &item.capture {
            problems.push(json!({"section":section,"code":e["code"],"message":e["message"]}));
        }
    }
    let mut modified = vec![];
    let mut differences = vec![];
    let mut notes = vec![];
    let mut unavailable = false;
    if let Some(active) = doc["active"].as_str() {
        for section in doc["profiles"][active]["sections"].as_array().unwrap() {
            let section = section.as_str().unwrap();
            let i = SECTIONS.iter().position(|s| *s == section).unwrap();
            match &current[i].capture {
                Err(_) => unavailable = true,
                Ok(c) => {
                    let (keys, n) =
                        compare(section, &doc["profiles"][active]["settings"][section], c);
                    notes.extend(n);
                    if !keys.is_empty() {
                        modified.push(section);
                    }
                    differences
                        .extend(keys.into_iter().map(|k| json!({"section":section,"key":k})));
                }
            }
        }
    }
    let state = if !doc["pending"].is_null() {
        "interrupted"
    } else if doc["active"].is_null() {
        "none"
    } else if unavailable {
        "unavailable"
    } else if !modified.is_empty() {
        "modified"
    } else {
        "in_sync"
    };
    differences.truncate(200);
    let mut entries = vec![];
    let mut matches = vec![];
    for (name, entry) in doc["profiles"].as_object().unwrap() {
        if entry["sections"].as_array().unwrap().iter().all(|section| {
            let s = section.as_str().unwrap();
            let i = SECTIONS.iter().position(|k| *k == s).unwrap();
            current[i]
                .capture
                .as_ref()
                .is_ok_and(|c| compare(s, &entry["settings"][s], c).0.is_empty())
        }) {
            matches.push(name);
        }
        entries.push(json!({"name":name,"description":entry["description"],"revision":revision(Some(canonical(entry).as_bytes())),
                "sections":entry["sections"],"settings":entry["settings"],"created_at":entry["created_at"],"updated_at":entry["updated_at"],"summary":summary(entry)}));
    }
    json!({"ok":true,"result":{"surface":"routing_profiles","path":path,"revision":revision(bytes),"format":2,
            "migrated_from":parsed["migrated_from"],"active":doc["active"],"state":state,"modified_sections":modified,
            "differences":differences,"pending":doc["pending"],"problems":problems,"applied":doc["applied"],
            "current_revisions":revisions,"profiles":entries,"user_routing_revision":revisions["routing"],"shadowed_by":shadow,
            "matches_profile":matches,"notes":notes}})
}
impl ConfigurationReader {
    /// Best-effort post-save selection status, computed before persistence against
    /// candidate routing bytes. A damaged/newer profile never prevents a save.
    pub(crate) fn selection_with_routing(&self, routing: &[u8]) -> Value {
        let view = match self.profiles_with_routing(None, Some(routing)) {
            Ok(v) if v["ok"] == true => v["result"].clone(),
            Ok(v) => return json!({"state":"unavailable","code":v["error"]["code"]}),
            Err(e) => return json!({"state":"unavailable","code":e.code()}),
        };
        let name = if view["active"].is_null() {
            &view["pending"]["profile"]
        } else {
            &view["active"]
        };
        if name.is_null() {
            Value::Null
        } else {
            json!({"name":name,"state":view["state"]})
        }
    }
    /// All five management file producers composed. Catalog/selector, workspace and
    /// setup are still trusted supplied observations; this does not change dispatch.
    pub fn settings_with_management_readers(
        &self,
        observations: &Value,
        now: f64,
        project: Option<(&Path, &Path)>,
    ) -> CoreResult<SettingsRead> {
        if !observations["surfaces"].is_object() {
            return Err(CoreError::Refused("invalid_settings_observation"));
        }
        let mut o = observations.clone();
        o["surfaces"]["user_defaults"] = self.user_defaults_management_observation()?;
        o["surfaces"]["routing_profiles"] = self.profiles_management_observation(project)?;
        self.settings_with_routing_readers(&o, now, project)
    }
}
