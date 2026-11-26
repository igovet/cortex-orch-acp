//! Owner MCP management display, never an executable capability or launcher.
use super::{matches, ordered::Ordered, ConfigurationReader, ENGINES};
use crate::native_routes::read_bytes;
use cortex_core::{CoreError, CoreResult};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, path::Path};

pub(crate) const KEYS: [&str; 14] = [
    "command",
    "args",
    "env",
    "enabled",
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
];
/// Per-server and registry-default containment. Not a profile policy key: like
/// the command it describes the owner's trust in this machine's executable, so
/// it stays outside `KEYS` (profile capture/activation and the frozen editable list).
pub(crate) const CONTAINMENT: &str = "containment";
pub(crate) fn containment_valid(v: &Value) -> bool {
    matches!(v.as_str(), Some("contained" | "none"))
}
/// True when the entry, or else the registry default, opts out of containment.
pub(crate) fn uncontained(spec: &Value, default: &Value) -> bool {
    spec.get(CONTAINMENT).unwrap_or(default) == "none"
}
fn string(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.contains('\0'))
}
fn nonempty(v: &Value) -> bool {
    string(v) && v.as_str().is_some_and(|s| !s.is_empty())
}
fn subset(value: &Value, options: &Value) -> bool {
    value.as_array().is_some_and(|a| {
        a.iter()
            .all(|v| options.as_array().is_some_and(|o| o.contains(v)))
    })
}
fn unique_choices(value: &Value, options: &[&str]) -> bool {
    value.as_array().is_some_and(|a| {
        !a.is_empty()
            && a.iter()
                .all(|v| v.as_str().is_some_and(|s| options.contains(&s)))
            && a.iter()
                .map(Value::to_string)
                .collect::<BTreeSet<_>>()
                .len()
                == a.len()
    })
}
fn env_problem(value: &Value) -> Option<&'static str> {
    if value.is_null() {
        return None;
    }
    let pairs: Vec<(&str, &Value)> = if let Some(o) = value.as_object() {
        o.iter().map(|(k, v)| (k.as_str(), v)).collect()
    } else if let Some(a) = value.as_array() {
        if a.iter()
            .any(|v| !v.is_object() || !v["name"].is_string() || !v["value"].is_string())
        {
            return Some("env must contain name/value strings.");
        }
        a.iter()
            .map(|v| (v["name"].as_str().unwrap(), &v["value"]))
            .collect()
    } else {
        return Some("env must be an object or array.");
    };
    if pairs
        .iter()
        .any(|(k, v)| k.is_empty() || k.contains('\0') || !string(v))
    {
        return Some("env contains an invalid variable.");
    }
    None
}
pub(crate) fn problem(name: &str, spec: &Value, path: &Path) -> Option<Value> {
    let p = path.display();
    if name == "cortex" {
        return Some(
            json!({"code":"invalid_mcp_registry","message":"'cortex' is reserved for Cortex's own server"}),
        );
    }
    if !spec.is_object() {
        return Some(
            json!({"code":"invalid_mcp_registry","message":"a server entry must be an object"}),
        );
    }
    if name.is_empty() {
        return Some(
            json!({"code":"invalid_mcp_registry","message":format!("{p} contains an invalid server entry.")}),
        );
    }
    // As in Python management, validate even a disabled entry as if enabled.
    let empty = json!([]);
    let get = |key: &str| spec.get(key).unwrap_or(&empty);
    let message = if !nonempty(&spec["command"]) {
        Some("command must be a non-empty string.")
    } else if !get("args").as_array().is_some_and(|a| a.iter().all(string)) {
        Some("args must be a string array.")
    } else if !spec["roles"].is_null()
        && !unique_choices(&spec["roles"], &["coordinator", "worker"])
    {
        Some("roles must be a non-empty array of unique coordinator/worker roles.")
    } else if !spec["engines"].is_null() && !unique_choices(&spec["engines"], &ENGINES) {
        Some("engines must be a non-empty array of unique codex-acp/claude-agent-acp/opencode engines.")
    } else {
        env_problem(&spec["env"])
    };
    if let Some(message) = message {
        return Some(
            json!({"code":"invalid_mcp_registry","message":format!("{p}: {name}.{message}")}),
        );
    }
    // The executable loader requires explicit tools, but does not normalize or
    // de-duplicate tool names here. Keep its exact management verdict.
    if !spec["allowed_tools"].as_array().is_some_and(|a| {
        !a.is_empty() && a.iter().all(|v| v.as_str().is_some_and(|s| !s.is_empty()))
    }) {
        return Some(json!({"code":"external_mcp_explicit_tool_grants_required",
            "message":format!("{p}: {name}.allowed_tools must name the exact permitted tools.")}));
    }
    let mut message = None;
    for key in ["read_paths", "write_paths"] {
        if !get(key).as_array().is_some_and(|a| {
            a.iter()
                .all(|v| v.as_str().is_some_and(|s| Path::new(s).is_absolute()))
        }) {
            message = Some(format!("{key} must contain absolute paths."));
            break;
        }
    }
    if message.is_none() && spec.get("network").is_some_and(|v| !v.is_boolean()) {
        message = Some("network must be boolean.".into());
    }
    if message.is_none() && spec.get(CONTAINMENT).is_some_and(|v| !containment_valid(v)) {
        message = Some("containment must be \"contained\" or \"none\".".into());
    }
    if message.is_none() && !subset(get("read_only_tools"), &spec["allowed_tools"]) {
        message = Some("read_only_tools must be a subset of allowed_tools.".into());
    }
    if message.is_none()
        && !get("runtime_dir_env").as_array().is_some_and(|a| {
            a.iter().all(|v| {
                v.as_str().is_some_and(|s| {
                    matches(r"^[A-Z_][A-Z0-9_]*$", s)
                        && !["PATH", "HOME", "TMPDIR", "XDG_RUNTIME_DIR"].contains(&s)
                })
            }) && a
                .iter()
                .map(Value::to_string)
                .collect::<BTreeSet<_>>()
                .len()
                == a.len()
        })
    {
        message = Some("runtime_dir_env must be unique environment variable names.".into());
    }
    if message.is_none()
        && spec
            .get("coordinator_tools")
            .is_some_and(|v| !subset(v, get("read_only_tools")))
    {
        message = Some("coordinator_tools must be a subset of read_only_tools.".into());
    }
    if message.is_none()
        && spec
            .get("always_load_tools")
            .is_some_and(|v| !subset(v, &spec["allowed_tools"]))
    {
        message = Some("always_load_tools must be a subset of allowed_tools.".into());
    }
    message.map(|m| json!({"code":"invalid_mcp_registry","message":format!("{p}: {name}.{m}")}))
}
fn redacted_env(node: &Ordered) -> Value {
    if node.value.is_object() {
        json!(node
            .members
            .iter()
            .map(|(k, v)| json!({"name":k,
            "value_set":v.value.as_str().is_some_and(|s| !s.is_empty())}))
            .collect::<Vec<_>>())
    } else if let Some(a) = node.value.as_array() {
        json!(a
            .iter()
            .filter(|v| v.is_object())
            .map(|v| json!({"name":v["name"],
            "value_set":v["value"].as_str().is_some_and(|s| !s.is_empty())}))
            .collect::<Vec<_>>())
    } else {
        Value::Null
    }
}
impl ConfigurationReader {
    /// Byte-exact revision, insertion-ordered entries and env presence only.
    /// No executable, root creation, repair, registry rewrite or engine credential IO.
    pub fn mcp_management(&self) -> CoreResult<Value> {
        let observation = self.mcp_management_observation()?;
        if observation["ok"] != true {
            return Err(CoreError::Refused("invalid_mcp_registry"));
        }
        Ok(observation["result"].clone())
    }
    /// Management envelope for composition. Malformed documents retain the safe
    /// Python surface error; unsafe storage still refuses the entire read.
    pub fn mcp_management_observation(&self) -> CoreResult<Value> {
        let path = self.environment.config.join("cortex/mcp-servers.json");
        let bytes = read_bytes(&path, true)?;
        observation(&path, bytes.as_deref())
    }
}
/// Pure candidate projection shared by the staged writer and passive reader.
pub(crate) fn observation(path: &Path, bytes: Option<&[u8]>) -> CoreResult<Value> {
    let failure = |message: &str| {
        Ok(json!({"ok":false,"error":{
            "category":"validation","code":"invalid_mcp_registry","message":message,
            "field":null,"retry_eligible":false}}))
    };
    let document: Ordered =
        match serde_json::from_slice(bytes.unwrap_or(b"{\"version\":1,\"servers\":{}}")) {
            Ok(document) => document,
            Err(_) => return failure(
                "mcp-servers.json is not valid UTF-8 JSON; repair it by hand. Nothing was changed.",
            ),
        };
    if !document.value.is_object() {
        return failure(
            "mcp-servers.json must contain a JSON object; repair it by hand. Nothing was changed.",
        );
    }
    let key = if document.member("servers").is_some() {
        "servers"
    } else {
        "mcpServers"
    };
    let Some(entries) = document.member(key).filter(|n| n.value.is_object()) else {
        return failure("mcp-servers.json must hold a 'servers' object; repair it by hand.");
    };
    let default = document.member(CONTAINMENT).map(|n| n.value.clone());
    if default.as_ref().is_some_and(|v| !containment_valid(v)) {
        return failure(
            "mcp-servers.json 'containment' must be \"contained\" or \"none\"; repair it by hand.",
        );
    }
    let mut servers = vec![];
    for (name, node) in &entries.members {
        let spec = &node.value;
        let problem = problem(name, spec, &path);
        let mut config = Value::Null;
        let mut unsupported = vec![];
        if spec.is_object() {
            config = json!({});
            for (k, v) in &node.members {
                if KEYS.contains(&k.as_str()) || k == CONTAINMENT {
                    config[k] = if k == "env" {
                        redacted_env(v)
                    } else {
                        v.value.clone()
                    };
                } else {
                    unsupported.push(k.clone());
                }
            }
        }
        unsupported.sort();
        servers.push(json!({"name":name,"enabled":if spec.is_object(){json!(spec["enabled"] != false)}else{Value::Null},
                "valid":problem.is_none(),"problem":problem,"config":config,"unsupported_keys":unsupported}));
    }
    let token = bytes
        .map(|b| format!("sha256:{:x}", Sha256::digest(b)))
        .unwrap_or("absent".into());
    let mut view = json!({"ok":true,"result":{"surface":"mcp_servers","path":path,"revision":token,"exists":bytes.is_some(),
        "root_key":key,"version":document.value["version"],"servers":servers,"editable_keys":KEYS,"engines":ENGINES}});
    // The registry default appears only when the owner set one.
    if let Some(default) = default {
        view["result"][CONTAINMENT] = default;
    }
    Ok(view)
}

/// A registry observation as the editor's server rows (env as presence only),
/// shared by the machine and the project-scoped editors.
pub(crate) fn editor_rows(result: &Value) -> Value {
    super::settings::mcp(result)
}
