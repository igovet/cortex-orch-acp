//! Pure activation candidates. Definitions stay in their installed backing files.
use super::*;
pub(super) fn build(
    index: usize,
    saved: &O,
    items: &[Section],
    folder: &Path,
    now: &str,
    notes: &mut Vec<String>,
) -> Result<Option<Vec<u8>>> {
    let settings = value(saved);
    let item = &items[index];
    let current = item.observation.capture.as_ref().ok();
    let unchanged = current.is_some_and(|c| {
        if index == 3 {
            settings
                .as_object()
                .unwrap()
                .iter()
                .all(|(id, v)| c.get(id).is_none_or(|x| x == v))
        } else if index == 2 {
            c.as_object()
                .unwrap()
                .iter()
                .all(|(n, v)| settings.get(n).is_some() || v["enabled"] == false)
                && settings
                    .as_object()
                    .unwrap()
                    .iter()
                    .all(|(n, v)| c.get(n) == Some(v))
        } else {
            *c == settings
        }
    });
    // The routing loader's named refusals (`require_model_policy`).
    if index == 0 && settings["model_policy_version"].is_null() {
        return Err(refuse("model_policy_migration_required","Explicitly migrate routing.json to model_policy_version=2 with exact enabled/auto identities and per-engine tiers.")
            .with(json!({"field":"model_policy_version"})));
    }
    if index == 0 && settings["model_policy_version"] != 2 {
        return Err(refuse(
            "model_policy_version_unsupported",
            "Only model_policy_version=2 is supported.",
        )
        .with(json!({"field":"model_policy_version"})));
    }
    if unchanged {
        if index == 3 {
            for id in settings.as_object().unwrap().keys() {
                if current.unwrap().get(id).is_none() {
                    notes.push(format!("connection_missing:{id}"));
                }
            }
        }
        return Ok(None);
    }
    // A backing file the loader rejects keeps that loader's own named refusal.
    if index == 1 {
        if let Err(e) = model::defaults_document(item.bytes.as_deref()) {
            return Err(refuse(
                match e["code"].as_str() {
                    Some("invalid_user_defaults") => "invalid_user_defaults",
                    Some("invalid_ceiling_snapshot") => "invalid_ceiling_snapshot",
                    _ => "invalid_settings",
                },
                e["message"]
                    .as_str()
                    .unwrap_or(
                        "defaults.json cannot be read; repair it by hand. Nothing was changed.",
                    )
                    .to_owned(),
            ));
        }
    }
    let file = FILES[index];
    let raw = item
        .bytes
        .as_deref()
        .map(serde_json::from_slice::<O>)
        .transpose()
        .map_err(|_| {
            refuse(
                "invalid_settings",
                format!("{file} is not valid UTF-8 JSON that Cortex can rewrite exactly. Repair it by hand; nothing was changed."),
            )
        })?
        .unwrap_or(O::Object(vec![]));
    if !raw.is_object() {
        return Err(refuse(
            "invalid_settings",
            format!("{file} must hold a JSON object; repair it by hand. Nothing was changed."),
        ));
    }
    let body = match index {
        0 => {
            let O::Object(fields) = saved else {
                unreachable!()
            };
            let O::Object(previous) = raw else {
                unreachable!()
            };
            let mut target = O::Object(
                fields
                    .iter()
                    .filter(|(k, _)| KEYS.contains(&k.as_str()))
                    .cloned()
                    .collect(),
            );
            for (k, v) in previous
                .into_iter()
                .filter(|(k, _)| !KEYS.contains(&k.as_str()))
            {
                set(&mut target, &k, v);
            }
            for (k, v) in fields.iter().filter(|(k, _)| !KEYS.contains(&k.as_str())) {
                set(&mut target, k, v.clone());
            }
            pretty(&target)
        }
        1 => {
            let (old, ceiling, _) =
                model::defaults_document(item.bytes.as_deref()).map_err(|_| {
                    refuse(
                        "config_action_unavailable",
                        "Unqualified defaults document.",
                    )
                })?;
            let mut merged = old.as_object().unwrap().clone();
            merged.retain(|k, _| !model::DEFAULT_KEYS.contains(&k.as_str()));
            merged.extend(settings["defaults"].as_object().unwrap().clone());
            if let Some(id) = merged.get("route_connection_id").and_then(Value::as_str) {
                let registry = crate::machine::config_action::registry_json::RegistryJson::parse(
                    items[3].bytes.as_deref(),
                )?;
                let state = crate::machine::config_action::registry::normalize(&registry.value)?;
                if state["connections"].get(id).is_none() {
                    return Err(refuse("profile_reference_missing", "The profile's default route names a connection that is not registered here; nothing was written.")
                        .with(json!({"field":"settings.user_defaults.defaults.route_connection_id"})));
                }
            }
            if ceiling != settings["ceiling"] {
                notes.push("owner_ceiling_changed".into());
            }
            let mut payload = value(&raw);
            let m = payload.as_object_mut().unwrap();
            m.remove("defaults");
            m.remove("ceiling");
            m.insert("defaults".into(), Value::Object(merged));
            m.insert("ceiling".into(), settings["ceiling"].clone());
            model::python_json(&payload).into_bytes()
        }
        2 => {
            let mut document = if item.bytes.is_none() {
                object(vec![("version", scalar(1)), ("servers", O::Object(vec![]))])
            } else {
                raw
            };
            let dv = value(&document);
            if dv.get("version").is_some_and(|v| *v != 1) {
                return Err(refuse(
                    "unsupported_mcp_registry_version",
                    "mcp-servers.json 'version' must be 1.",
                )
                .with(json!({"field":"version"})));
            }
            let key = if dv.get("servers").is_some() {
                "servers"
            } else {
                "mcpServers"
            };
            let O::Object(servers) = get(&document, key) else {
                return Err(refuse(
                    "config_action_unavailable",
                    "Unqualified MCP document.",
                ));
            };
            let O::Object(policies) = saved else {
                unreachable!()
            };
            for (n, _) in policies {
                if !servers.iter().any(|(id, _)| id == n) {
                    return Err(refuse("profile_mcp_server_missing", "The profile names an MCP server that is not configured here; add it again or save the profile. Nothing was written.")
                        .with(json!({"field":format!("settings.mcp_servers.{n}")})));
                }
            }
            let mut output = vec![];
            for (n, spec) in servers {
                let mut spec = spec.clone();
                if spec.is_object() {
                    if policies.iter().any(|(id, _)| id == n) {
                        for k in model::mcp::KEYS
                            .iter()
                            .filter(|k| !["command", "args", "env"].contains(k))
                        {
                            remove(&mut spec, k);
                        }
                        let O::Object(policy) = get(saved, n) else {
                            unreachable!()
                        };
                        for (k, v) in policy {
                            set(&mut spec, k, v.clone());
                        }
                    } else {
                        set(&mut spec, "enabled", scalar(false));
                    }
                }
                output.push((n.clone(), spec));
            }
            if dv.get("version").is_none() {
                set(&mut document, "version", scalar(1));
            }
            set(&mut document, key, O::Object(output));
            let d = value(&document);
            for (n, spec) in d[key].as_object().unwrap() {
                if let Some(e) = model::mcp::problem(n, spec, &folder.join("mcp-servers.json")) {
                    return Err(refuse(
                        "config_action_unavailable",
                        e["message"].as_str().unwrap(),
                    ));
                }
            }
            pretty(&document)
        }
        3 => {
            let input = crate::machine::config_action::registry_json::RegistryJson::parse(
                item.bytes.as_deref(),
            )?;
            let mut state = crate::machine::config_action::registry::normalize(&input.value)?;
            for (id, limit) in settings.as_object().unwrap() {
                if state["connections"].get(id).is_none() {
                    notes.push(format!("connection_missing:{id}"));
                } else if state["connections"][id]["max_concurrent_sessions"]
                    != limit["max_concurrent_sessions"]
                {
                    state["connections"][id]["max_concurrent_sessions"] =
                        limit["max_concurrent_sessions"].clone();
                    state["connections"][id]["updated_at"] = json!(now);
                }
            }
            input.bytes(&state)
        }
        _ => unreachable!(),
    };
    Ok(Some(body))
}
