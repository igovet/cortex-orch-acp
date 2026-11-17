//! Native machine user-defaults section.
//! Replies and both file candidates are budgeted before settings writes.
//! No provider switch or replay is safe after the first replacement.
use super::{validation::*, *};
use crate::config_read_model::{defaults_document, parse_connections, DEFAULT_KEYS};
#[path = "user_defaults_management.rs"]
mod management;
pub use management::update as update_management;
pub(in crate::machine) use management::{ceiling_patch, default_patch, Problems};

#[cfg(test)]
#[path = "user_defaults_tests.rs"]
mod tests;

fn patch(value: &Value) -> Result<Value> {
    if !value.is_object() {
        return Err(object_error());
    }
    let mut patch = json!({"coordinator":null,"route_id":null,"route_connection_id":null,
        "route_model":null,"language":if value["language"] == "auto" {Value::Null} else {value["language"].clone()}});
    let choice = &value["coordinator"];
    let route = &value["routeDefault"];
    if truth(choice) {
        if !choice.is_object() {
            return Err(object_error());
        }
        let mut parameters = dictionary(&choice["parameters"])?;
        if truth(&choice["effort"]) {
            parameters["thought_level"] = choice["effort"].clone();
        }
        if choice.get("engine").is_none() || choice.get("model").is_none() {
            return Err(refuse("machine_tool_failed", "KeyError"));
        }
        patch["coordinator"] =
            json!({"engine":choice["engine"],"model":choice["model"],"parameters":parameters});
    } else if truth(route) {
        if !route.is_object() {
            return Err(refuse("machine_tool_failed", "TypeError"));
        }
        for (key, wire) in [
            ("route_id", "routeId"),
            ("route_connection_id", "connectionId"),
            ("route_model", "model"),
        ] {
            patch[key] = route
                .get(wire)
                .cloned()
                .ok_or_else(|| refuse("machine_tool_failed", "KeyError"))?;
        }
        patch["parameters"] = if truth(&route["parameters"]) {
            route["parameters"].clone()
        } else {
            Value::Null
        };
    }
    Ok(patch)
}
fn dictionary(value: &Value) -> Result<Value> {
    if !truth(value) {
        return Ok(json!({}));
    }
    if value.is_object() {
        return Ok(value.clone());
    }
    let Some(items) = value.as_array() else {
        return Err(refuse(
            "machine_tool_failed",
            if value.is_string() {
                "ValueError"
            } else {
                "TypeError"
            },
        ));
    };
    let mut out = json!({});
    for item in items {
        let Some(pair) = item.as_array() else {
            return Err(refuse(
                "machine_tool_failed",
                if item.is_string() {
                    "ValueError"
                } else {
                    "TypeError"
                },
            ));
        };
        if pair.len() != 2 {
            return Err(refuse("machine_tool_failed", "ValueError"));
        }
        // A JSON mapping can represent only string keys. Non-string Python dict
        // keys cannot satisfy portable-coordinator validation either.
        let Some(key) = pair[0].as_str() else {
            return Err(refuse(
                "native_config_unavailable",
                "Parameter dictionary keys must be strings; nothing was changed.",
            ));
        };
        out[key] = pair[1].clone();
    }
    Ok(out)
}
fn validate(patch: &Value, errors: &mut Errors) {
    // Machine helper constructs this insertion order; keep aggregation identical.
    for key in [
        "coordinator",
        "route_id",
        "route_connection_id",
        "route_model",
        "language",
        "parameters",
    ] {
        let v = &patch[key];
        if v.is_null() {
            continue;
        }
        let field = format!("defaults.{key}");
        match key {
            "coordinator" => {
                if !v.is_object()
                    || v.as_object().unwrap().len() != 3
                    || !v["engine"]
                        .as_str()
                        .is_some_and(|s| routing::ENGINES.contains(&s))
                    || !v["model"].as_str().is_some_and(|s| !s.trim().is_empty())
                    || !v["parameters"].as_object().is_some_and(|m| {
                        m.iter().all(|(k, v)| {
                            !k.is_empty() && v.as_str().is_some_and(|s| !s.is_empty())
                        })
                    })
                {
                    errors.add(
                        field,
                        "Use exact engine/model and a parameter string mapping.",
                    );
                }
            }
            "route_id"
                if !v.as_str().is_some_and(|s| {
                    crate::config_read_model::matches(r"^xr_[a-f0-9]{12,64}$", s)
                }) =>
            {
                errors.add(field, "must be an observed route id (xr_<hex>)")
            }
            "route_connection_id"
                if !v.as_str().is_some_and(|s| {
                    crate::config_read_model::matches(r"^cn_[a-f0-9]{12}$", s)
                }) =>
            {
                errors.add(field, "must be a connection id (cn_<12 hex>)")
            }
            "route_model" if !crate::config_read_model::clean(v, 256) => {
                errors.add(field, "must be the route's exact model id")
            }
            "language" if v != "en" && v != "ru" => {
                errors.add(field, "must be null (auto) or one of en, ru")
            }
            "parameters" => {
                if let Some(m) = v.as_object() {
                    for (name, item) in m {
                        if !crate::config_read_model::matches(r"^[a-z][a-z0-9_]{0,63}$", name)
                            || !crate::config_read_model::clean(item, 128)
                        {
                            errors.add(format!("{field}.{name}"),
                                "parameter names are lowercase identifiers and values non-empty strings");
                        }
                    }
                } else {
                    errors.add(field, "must be an object of parameter name -> value");
                }
            }
            _ => (),
        }
    }
}
/// Python's sorted UTF-8 JSON, with its number representation and optional spaces.
fn encode(value: &Value, spaced: bool) -> Vec<u8> {
    let pretty = routing::view::Ordered::from_value(value).pretty();
    let mut out = Vec::new();
    let (mut quoted, mut escape) = (false, false);
    for b in pretty {
        if quoted {
            out.push(b);
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                quoted = false;
            }
        } else if b == b'"' {
            quoted = true;
            out.push(b);
        } else if !b.is_ascii_whitespace() {
            out.push(b);
            if spaced && (b == b':' || b == b',') {
                out.push(b' ');
            }
        }
    }
    out
}
/// Never silently round preserved Python-sized integers into floats.
pub(in crate::machine) fn exact_numbers(bytes: &[u8]) -> Result<()> {
    let (mut i, mut quoted, mut escape) = (0, false, false);
    while i < bytes.len() {
        let b = bytes[i];
        if quoted {
            if escape {
                escape = false;
            } else if b == b'\\' {
                escape = true;
            } else if b == b'"' {
                quoted = false;
            }
            i += 1;
        } else if b == b'"' {
            quoted = true;
            i += 1;
        } else if b == b'-' || b.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || b".eE+-".contains(&bytes[i])) {
                i += 1;
            }
            let n = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
            if !n.contains(['.', 'e', 'E'])
                && n.parse::<i64>().is_err()
                && n.parse::<u64>().is_err()
            {
                return Err(refuse(
                    "native_config_unavailable",
                    "Preserved integers exceed the native exact range; nothing was changed.",
                ));
            }
        } else {
            i += 1;
        }
    }
    Ok(())
}
fn parsed(bytes: Option<&[u8]>) -> Result<(Value, Value)> {
    let (defaults, _, _) = defaults_document(bytes).map_err(|e| {
        refuse(
            match e["code"].as_str() {
                Some("invalid_ceiling_snapshot") => "invalid_ceiling_snapshot",
                _ => "invalid_user_defaults",
            },
            e["message"].as_str().unwrap_or("Invalid user defaults."),
        )
    })?;
    if let Some(bytes) = bytes {
        exact_numbers(bytes)?;
    }
    let document = bytes
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|_| object_error())?
        .unwrap_or(json!({}));
    Ok((defaults, document))
}
pub(super) fn ignore(folder: &Path) -> Result<()> {
    let path = folder.join(".gitignore");
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(&path)
    {
        Ok(mut f) => {
            let result = (|| {
                f.write_all(b"*\n")?;
                f.sync_all()?;
                Ok(())
            })();
            if result.is_err() {
                let _ = fs::remove_file(path);
            }
            result
        }
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if reader::read_plain(&path, true)?.as_deref() != Some(b"*\n") {
                return Err(refuse("unsafe_storage",format!("{} already exists with unexpected content; refusing to overwrite it (it must contain exactly '*').",path.display())));
            }
            Ok(())
        }
        Err(e) => Err(e.into()),
    }
}
fn workspace(config: &crate::Config, value: &Value) -> Result<(Option<Vec<u8>>, Vec<u8>)> {
    let path = config.state_dir.join("defaults.json");
    safe_target(&path)?;
    let before = reader::read_plain(&path, true)?;
    if let Some(b) = &before {
        exact_numbers(b)?;
    }
    let mut doc: Value = before
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|_| {
            refuse(
                "state_unreadable",
                "Saved Workspace state could not be read.",
            )
        })?
        .unwrap_or(json!({}));
    if !doc.is_object() {
        return Err(refuse("machine_tool_failed", "TypeError"));
    }
    for (key, default) in [("mode", "guarded"), ("checkout", "main")] {
        doc[key] = value.get(key).cloned().unwrap_or(json!(default));
    }
    Ok((before, encode(&doc, true)))
}
pub(crate) fn save(config: &crate::Config, params: &Value) -> Result<Value> {
    save_with(config, params, replace)
}
fn save_with(
    config: &crate::Config,
    params: &Value,
    mut persist: impl FnMut(&Path, &[u8]) -> Result<()>,
) -> Result<Value> {
    if !params.is_object() {
        return Err(object_error());
    }
    let p = patch(&params["value"])?;
    let c = if truth(&params["value"]["ceiling"]) {
        params["value"]["ceiling"].clone()
    } else {
        json!({})
    };
    let c = ceiling::document(&c)?;
    let revision = expected(params, "user_defaults")?;
    let mut errors = Errors::default();
    validate(&p, &mut errors);
    let c = ceiling::collect(&c, &mut errors);
    errors.finish()?;
    let folder = reader::owner_folder(config)?;
    let _owner = owner_lock(&folder)?;
    ignore(&folder)?;
    let path = folder.join("defaults.json");
    let before = reader::read_plain(&path, true)?;
    check("user_defaults", &revision, &token(before.as_deref()))?;
    let (mut defaults, mut doc) = parsed(before.as_deref())?;
    for (key, value) in p.as_object().unwrap() {
        if value.is_null() {
            defaults.as_object_mut().unwrap().remove(key);
        } else {
            defaults[key] = value.clone();
        }
    }
    if !defaults["coordinator"].is_null()
        && ["route_id", "route_connection_id", "route_model"]
            .iter()
            .any(|k| !defaults[k].is_null())
    {
        let mut e = Errors::default();
        e.add(
            "defaults.coordinator",
            "remove the bound route before setting a portable coordinator",
        );
        e.finish()?;
    }
    let present = ["route_id", "route_connection_id", "route_model"]
        .iter()
        .filter(|k| !defaults[**k].is_null())
        .count();
    let mut errors = Errors::default();
    if present != 0 && present != 3 {
        errors.add(
            "defaults",
            "route_id, route_connection_id and route_model are set or removed together",
        );
    } else if present == 3 {
        let bytes = reader::read_plain(&folder.join("connections.json"), true)?;
        let input = bytes
            .as_deref()
            .map(serde_json::from_slice::<Value>)
            .transpose()
            .map_err(|_| {
                refuse(
                    "invalid_registry",
                    "The connection registry is invalid; nothing was written.",
                )
            })?;
        let registry = parse_connections(input.as_ref()).map_err(|code| {
            refuse(
                code,
                "The connection registry is invalid; nothing was written.",
            )
        })?;
        if registry["connections"]
            .get(defaults["route_connection_id"].as_str().unwrap())
            .is_none()
        {
            errors.add(
                "defaults.route_connection_id",
                "names no registered connection",
            );
        }
    }
    errors.finish()?;
    doc["defaults"] = defaults.clone();
    doc["ceiling"] = c.clone();
    let body = encode(&doc, false);
    let preserved: Vec<_> = defaults
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| !DEFAULT_KEYS.contains(&k.as_str()))
        .collect();
    // update_user_defaults does not add read_user_defaults' coordinator_resolution.
    let reply = json!({"surface":"user_defaults","path":path,"revision":token(Some(&body)),
        "exists":true,"defaults":defaults,"editable_keys":DEFAULT_KEYS,"preserved_keys":preserved,
        "languages":["en","ru"],"ceiling":c,"ceiling_set":true,"changed":Some(body.as_slice()) != before.as_deref()});
    crate::local::private_tree(&config.state_dir)?;
    let _workspace = cortex_core::store::locks::AccessLock::acquire(
        &config.state_dir.join("machine.lock"),
        true,
        true,
        std::time::Duration::from_secs(25),
    )
    .map_err(|_| {
        refuse(
            "machine_busy",
            "Workspace defaults lock is busy or unsafe; nothing was written.",
        )
    })?;
    let (workspace_before, workspace_body) = workspace(config, &params["value"])?;
    if [
        body.len(),
        workspace_body.len(),
        serde_json::to_vec(&reply).unwrap().len(),
    ]
    .iter()
    .any(|n| *n > FRAME_BUDGET)
    {
        return Err(refuse(
            "machine_reply_too_large",
            "Settings exceed the native write limit.",
        ));
    }
    check(
        "user_defaults",
        &token(before.as_deref()),
        &token(reader::read_plain(&path, true)?.as_deref()),
    )?;
    check(
        "workspace_defaults",
        &token(workspace_before.as_deref()),
        &token(reader::read_plain(&config.state_dir.join("defaults.json"), true)?.as_deref()),
    )?;
    persist(&path, &body)?;
    // From here a failure can leave the two independent files partially committed.
    // Never misrepresent it as an ordinary refusal or delegate/retry the operation.
    persist(&config.state_dir.join("defaults.json"), &workspace_body).map_err(|_| unknown())?;
    Ok(reply)
}
