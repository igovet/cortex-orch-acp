//! Native machine MCP save. Definitions are data: never discover, launch or call tools.
use super::{routing::view::Ordered, validation::*, *};
use crate::config_read_model::mcp;
use std::collections::{BTreeMap, BTreeSet};

fn view(path: &Path, bytes: Option<&[u8]>) -> Result<Value> {
    let v = mcp::observation(path, bytes)
        .map_err(|e| refuse(e.code(), "The MCP registry could not be read."))?;
    if v["ok"] == true {
        Ok(v["result"].clone())
    } else {
        Err(refuse(
            "invalid_mcp_registry",
            v["error"]["message"].as_str().unwrap(),
        ))
    }
}
fn members(node: &mut Ordered) -> &mut Vec<(String, Ordered)> {
    match node {
        Ordered::Object(m) => m,
        _ => unreachable!(),
    }
}
fn set(node: &mut Ordered, key: &str, value: Ordered) {
    let m = members(node);
    if let Some((_, v)) = m.iter_mut().find(|(k, _)| k == key) {
        *v = value;
    } else {
        m.push((key.into(), value));
    }
}
fn value(node: &Ordered) -> Value {
    serde_json::to_value(node).unwrap()
}
fn old_env(spec: &Value) -> BTreeMap<String, String> {
    if let Some(m) = spec["env"].as_object() {
        m.iter()
            .filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.into())))
            .collect()
    } else {
        spec["env"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| Some((v["name"].as_str()?.into(), v["value"].as_str()?.into())))
            .collect()
    }
}
fn array<'a>(v: &'a Value) -> Result<&'a [Value]> {
    if !truth(v) {
        return Ok(&[]);
    }
    v.as_array()
        .map(Vec::as_slice)
        .ok_or_else(|| refuse("machine_tool_failed", "TypeError"))
}
fn env_items(v: &Value) -> Result<&[Value]> {
    if truth(v) && (v.is_string() || v.is_object()) {
        return Err(object_error());
    }
    array(v)
}
/// The entry's own `containment` after an edit. An editor that does not send the
/// field keeps the stored value; a value equal to the registry default (absent:
/// contained) is not repeated on the entry, so the entry keeps inheriting it.
fn containment(server: &Value, old: &Value, default: &Value) -> Option<Value> {
    let Some(wanted) = server.get(mcp::CONTAINMENT).filter(|v| !v.is_null()) else {
        return old.get(mcp::CONTAINMENT).cloned();
    };
    let inherited = default.as_str().unwrap_or("contained");
    (wanted != inherited).then(|| wanted.clone())
}
fn entry(
    server: &Value,
    old: &Value,
    default: &Value,
    field: &str,
    errors: &mut Errors,
) -> Result<Ordered> {
    if !server.is_object() {
        return Err(object_error());
    }
    let mut spec = json!({});
    spec["command"] = server["command"].clone();
    for (key, wire) in [
        ("args", "args"),
        ("allowed_tools", "allowedTools"),
        ("read_only_tools", "readOnlyTools"),
        ("read_paths", "readPaths"),
        ("write_paths", "writePaths"),
        ("runtime_dir_env", "runtimeDirEnv"),
    ] {
        spec[key] = if truth(&server[wire]) {
            server[wire].clone()
        } else {
            json!([])
        };
    }
    for key in ["enabled", "network"] {
        spec[key] = json!(truth(&server[key]));
    }
    for (key, wire) in [
        ("coordinator_tools", "coordinatorTools"),
        ("always_load_tools", "alwaysLoadTools"),
        ("roles", "roles"),
        ("engines", "engines"),
    ] {
        if !server[wire].is_null() {
            spec[key] = server[wire].clone();
        }
    }
    let previous = old_env(old);
    let mut env = Vec::new();
    let mut seen = BTreeSet::new();
    for item in env_items(&server["env"])? {
        if !item.is_object() {
            return Err(object_error());
        }
        if item["action"] == "remove" {
            continue;
        }
        let name = item
            .get("name")
            .ok_or_else(|| refuse("machine_tool_failed", "KeyError"))?;
        let Some(name) = name.as_str() else {
            errors.add(
                format!("{field}.env"),
                "variable names are unique identifiers",
            );
            continue;
        };
        if !crate::config_read_model::matches(r"^[A-Za-z_][A-Za-z0-9_]{0,127}$", name)
            || seen.contains(name)
        {
            errors.add(
                format!("{field}.env"),
                "variable names are unique identifiers",
            );
            continue;
        }
        let v = if item["action"] == "keep" && truth(&item["stored"]) {
            match previous.get(name) {
                Some(v) => json!(v),
                None => {
                    errors.add(format!("{field}.env.{name}"), "has no stored value to keep");
                    continue;
                }
            }
        } else {
            item.get("value").cloned().unwrap_or(json!(""))
        };
        if !v.as_str().is_some_and(|s| !s.contains('\0')) {
            errors.add(
                format!("{field}.env.{name}"),
                "must be a string or {\"keep\": true}",
            );
            continue;
        }
        seen.insert(name.to_owned());
        env.push((name.into(), Ordered::from_value(&v)));
    }
    let mut ordered = Ordered::Object(
        mcp::KEYS
            .iter()
            .filter_map(|k| {
                spec.get(*k)
                    .map(|v| (k.to_string(), Ordered::from_value(v)))
            })
            .collect(),
    );
    if !env.is_empty() {
        set(&mut ordered, "env", Ordered::Object(env));
    }
    if let Some(c) = containment(server, old, default) {
        set(&mut ordered, mcp::CONTAINMENT, Ordered::from_value(&c));
    }
    Ok(ordered)
}

pub(crate) fn save(config: &crate::Config, params: &Value) -> Result<Value> {
    save_with(config, params, replace)
}
pub(super) fn save_with(
    config: &crate::Config,
    params: &Value,
    persist: impl FnOnce(&Path, &[u8]) -> Result<()>,
) -> Result<Value> {
    if !params.is_object() {
        return Err(object_error());
    }
    let folder = reader::owner_folder(config)?;
    let path = folder.join("mcp-servers.json");
    let before = reader::read_plain(&path, true)?;
    let current = view(&path, before.as_deref())?;
    let wanted = array(&params["value"])?;
    // Match the machine editor's implicit removes, then ordered upserts.
    let mut names = BTreeSet::new();
    for server in wanted {
        let name = server.get("name").ok_or_else(|| {
            refuse(
                "machine_tool_failed",
                if server.is_object() {
                    "KeyError"
                } else {
                    "TypeError"
                },
            )
        })?;
        if name.is_array() || name.is_object() {
            return Err(refuse("machine_tool_failed", "TypeError"));
        }
        names.insert(name.to_string());
    }
    let removed: BTreeSet<_> = current["servers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| !names.contains(&v["name"].to_string()))
        .map(|v| v["name"].as_str().unwrap().to_owned())
        .collect();
    if removed.is_empty() && wanted.is_empty() {
        return Ok(json!({"changed":false}));
    }
    // mcp_operations constructs env patches before update admission. Preserve
    // those malformed-editor errors without creating lock/ignore files first.
    for server in wanted {
        for item in env_items(&server["env"])? {
            if !item.is_object() {
                return Err(object_error());
            }
            if item["action"] != "remove" && item.get("name").is_none() {
                return Err(refuse("machine_tool_failed", "KeyError"));
            }
        }
    }
    let mut params = params.clone();
    if truth(&params["tokens"]) && !params["tokens"].is_object() {
        return Err(object_error());
    }
    if !truth(&params["tokens"]["mcp_servers"]) {
        params["tokens"] = json!({"mcp_servers":token(before.as_deref())});
    }
    let expected = expected(&params, "mcp_servers")?;
    let _lock = owner_lock(&folder)?;
    user_defaults::ignore(&folder)?;
    let data = reader::read_plain(&path, true)?;
    check("mcp_servers", &expected, &token(data.as_deref()))?;
    let current = view(&path, data.as_deref())?;
    if let Some(b) = &data {
        user_defaults::exact_numbers(b)?;
    }
    let mut doc: Ordered =
        serde_json::from_slice(data.as_deref().unwrap_or(b"{\"version\":1,\"servers\":{}}"))
            .map_err(|_| object_error())?;
    let mut raw = value(&doc);
    if raw
        .get("version")
        .is_some_and(|v| v != &json!(1) && v != &json!(1.0) && v != &json!(true))
    {
        return Err(refuse(
            "unsupported_mcp_registry_version",
            "mcp-servers.json 'version' must be 1.",
        ));
    }
    let key = current["root_key"].as_str().unwrap();
    let default = raw[mcp::CONTAINMENT].clone();
    let servers = members(&mut doc)
        .iter_mut()
        .find(|(k, _)| k == key)
        .unwrap();
    let mut errors = Errors::default();
    for (index, name) in removed.iter().enumerate() {
        let m = members(&mut servers.1);
        if !m.iter().any(|(k, _)| k == name) {
            errors.add(
                format!("operations[{index}].name"),
                "names no configured server",
            );
        }
        m.retain(|(k, _)| k != name);
    }
    for (i, server) in wanted.iter().enumerate() {
        let field = format!("operations[{}]", i + removed.len());
        let name = &server["name"];
        let Some(name) = name.as_str().filter(|s| {
            *s != "cortex"
                && crate::config_read_model::matches(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$", s)
        }) else {
            errors.add(
                format!("{field}.name"),
                "must be a short name of letters, digits, '.', '_' or '-' other than 'cortex'",
            );
            continue;
        };
        let old = value(&servers.1);
        let new = entry(
            server,
            &old[name],
            &default,
            &format!("{field}.server"),
            &mut errors,
        )?;
        if let Some(p) = mcp::problem(name, &value(&new), &path) {
            errors.add(format!("{field}.server"), p["message"].as_str().unwrap());
            continue;
        }
        set(&mut servers.1, name, new);
    }
    errors.finish()?;
    // The loader validates all entries, including retained invalid enabled entries.
    for (name, spec) in members(&mut servers.1).iter() {
        let spec = value(spec);
        let problem = if name.is_empty() || name == "cortex" || !spec.is_object() {
            Some(format!(
                "{} contains an invalid server entry.",
                path.display()
            ))
        } else if spec["enabled"] == false {
            None
        } else {
            mcp::problem(name, &spec, &path).map(|p| p["message"].as_str().unwrap().to_owned())
        };
        if let Some(p) = problem {
            let mut e = Errors::default();
            e.add("servers", p);
            e.finish()?;
        }
    }
    raw = value(&doc);
    if raw.get("version").is_none() {
        set(&mut doc, "version", Ordered::Scalar(json!(1)));
    }
    let mut body = doc.pretty();
    body.push(b'\n');
    let mut reply = view(&path, Some(&body))?;
    reply["changed"] = json!(Some(body.as_slice()) != data.as_deref());
    if body.len() > FRAME_BUDGET || serde_json::to_vec(&reply).unwrap().len() > FRAME_BUDGET {
        return Err(refuse(
            "machine_reply_too_large",
            "Settings exceed the native write limit.",
        ));
    }
    safe_target(&path)?;
    check(
        "mcp_servers",
        &token(data.as_deref()),
        &token(reader::read_plain(&path, true)?.as_deref()),
    )?;
    persist(&path, &body)?;
    Ok(reply)
}
