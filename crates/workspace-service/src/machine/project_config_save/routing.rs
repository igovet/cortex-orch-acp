use super::{validation::*, *};
#[path = "routing_validate.rs"]
mod validate;
pub(super) fn validate_rules(document: &Value) -> Result<()> {
    validate::rules(document)
}
/// `validate_routing_rules` of the management API, unknown keys included.
pub(super) fn validate_management_rules(rules: &Value, ordered: &view::Ordered) -> Result<()> {
    validate::management_rules(rules, ordered)
}
#[path = "routing_view.rs"]
pub(super) mod view;
pub(super) const KEYS: [&str; 10] = [
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
];
pub(super) const ENGINES: [&str; 3] = ["codex-acp", "claude-agent-acp", "opencode"];
pub(super) const TIERS: [&str; 4] = ["flagship", "deep", "standard", "light"];
pub(super) const EFFORTS: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];
pub(super) const KINDS: [&str; 13] = [
    "implementation_backend",
    "implementation_frontend",
    "implementation_frontend_design",
    "operations",
    "debugging",
    "testing",
    "investigation",
    "review",
    "design",
    "writing",
    "planning",
    "analysis",
    "implementation",
];
// The helper builds this order, then management filters it into KEYS order.
pub(super) fn document(v: &Value, previous: &Value) -> Result<Value> {
    if !v.is_object() {
        return Err(object_error());
    }
    let mut d = json!({"model_policy_version":v.get("version").cloned().unwrap_or(json!(2))});
    if let Some(t) = previous.get("tiers") {
        d["tiers"] = t.clone();
    }
    for (k, w) in [
        ("deny", "deny"),
        ("auto_allow", "autoAllow"),
        ("engine_affinity", "affinity"),
    ] {
        if truth(&v[w]) {
            d[k] = v[w].clone();
        }
    }
    if truth(&v["providers"]) {
        let Some(a) = v["providers"].as_array() else {
            return Err(refuse("machine_tool_failed", "TypeError"));
        };
        if a.iter()
            .map(Value::to_string)
            .collect::<std::collections::BTreeSet<_>>()
            != ENGINES.iter().map(|e| json!(e).to_string()).collect()
        {
            d["providers"] = v["providers"].clone();
        }
    }
    if truth(&v["pins"]) {
        let Some(pins) = v["pins"].as_object() else {
            return Err(object_error());
        };
        let mut p = json!({});
        for (role, pin) in pins {
            let (Some(e), Some(m)) = (pin["engine"].as_str(), pin["model"].as_str()) else {
                return Err(refuse("machine_tool_failed", "TypeError"));
            };
            let mut route = format!("{e}/{m}");
            if truth(&pin["effort"]) {
                let Some(s) = pin["effort"].as_str() else {
                    return Err(refuse("machine_tool_failed", "TypeError"));
                };
                route.push(':');
                route.push_str(s);
            }
            p[role] = json!(route);
        }
        d["pin"] = p;
    }
    let mut enabled = json!({});
    let mut models = json!({});
    let mut quota = json!({});
    if truth(&v["engines"]) {
        let Some(engines) = v["engines"].as_object() else {
            return Err(object_error());
        };
        for (engine, c) in engines {
            if !c.is_object() {
                return Err(object_error());
            }
            enabled[engine] = if truth(&c["enabled"]) {
                c["enabled"].clone()
            } else {
                json!([])
            };
            let mut entry = json!({});
            if truth(&c["auto"]) {
                entry["auto"] = c["auto"].clone();
            }
            if truth(&c["tiers"]) {
                let Some(t) = c["tiers"].as_object() else {
                    return Err(object_error());
                };
                let mut tiers = json!({});
                for (m, ts) in t {
                    if !truth(ts) {
                        continue;
                    }
                    tiers[m] = match ts {
                        Value::Array(a) if a.len() == 1 => a[0].clone(),
                        Value::Array(_) | Value::String(_) => ts.clone(),
                        _ => return Err(refuse("machine_tool_failed", "TypeError")),
                    };
                }
                entry["tiers"] = tiers;
            }
            if truth(&c["policy"]) {
                let Some(policies) = c["policy"].as_object() else {
                    return Err(object_error());
                };
                let mut policies_out = json!({});
                for (m, p) in policies {
                    if !p.is_object() {
                        return Err(object_error());
                    }
                    let mut written = json!({});
                    for (k, w) in [
                        ("effort_floor", "effortFloor"),
                        ("max_effort", "maxEffort"),
                        ("effort_ladder", "effortLadder"),
                        ("also_standard_for", "alsoStandardFor"),
                        ("preferred_for", "preferredFor"),
                        ("preferred_when", "preferredWhen"),
                        ("request_names", "requestNames"),
                    ] {
                        if !p[w].is_null() {
                            written[k] = p[w].clone();
                        }
                    }
                    if truth(&written) {
                        policies_out[m] = written;
                    }
                }
                if truth(&policies_out) {
                    entry["policy"] = policies_out;
                }
            }
            if truth(&entry) {
                models[engine] = entry;
            }
            if truth(&c["quota"]) {
                quota[engine] = c["quota"].clone();
            }
        }
    }
    d["enabled"] = enabled;
    if truth(&models) {
        d["models"] = models;
    }
    if truth(&quota) {
        d["quota_windows"] = quota;
    }
    Ok(d)
}
/// The existing file's object, validated as the management writer reads it.
fn previous_document(path: &Path, previous: Option<&[u8]>) -> Result<Option<view::Ordered>> {
    // Never round a Python-sized integer in preserved metadata into an f64.
    if let Some(bytes) = previous {
        let mut i = 0;
        let mut quoted = false;
        let mut escape = false;
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
                continue;
            }
            if b == b'"' {
                quoted = true;
                i += 1;
                continue;
            }
            if b == b'-' || b.is_ascii_digit() {
                let start = i;
                i += 1;
                while i < bytes.len() && (bytes[i].is_ascii_digit() || b".eE+-".contains(&bytes[i]))
                {
                    i += 1;
                }
                let n = std::str::from_utf8(&bytes[start..i]).unwrap_or("");
                if !n.contains(['.', 'e', 'E'])
                    && n.parse::<i64>().is_err()
                    && n.parse::<u64>().is_err()
                {
                    return Err(refuse("invalid_routing_overrides","Preserved routing integers exceed the native exact range; nothing was changed."));
                }
            } else {
                i += 1;
            }
        }
    }
    // Preserve source object order, including unknown nested metadata.
    let old: Option<view::Ordered> =
        previous
            .map(serde_json::from_slice)
            .transpose()
            .map_err(|_| {
                refuse(
                    "invalid_routing_overrides",
                    format!(
                        "{} is not valid UTF-8 JSON; repair it by hand. Nothing was changed.",
                        path.display()
                    ),
                )
            })?;
    if old.as_ref().is_some_and(|v| !v.is_object()) {
        return Err(refuse(
            "invalid_routing_overrides",
            format!(
                "{} must contain a JSON object; repair it by hand. Nothing was changed.",
                path.display()
            ),
        ));
    }
    Ok(old)
}
/// `_pretty({**document, **preserved})`: the supported keys in their order
/// with the request's own nested order, then the file's other keys.
pub(super) fn pretty_ordered(
    path: &Path,
    mut fields: Vec<(String, view::Ordered)>,
    previous: Option<&[u8]>,
) -> Result<Vec<u8>> {
    if let Some(view::Ordered::Object(old)) = previous_document(path, previous)? {
        fields.extend(old.into_iter().filter(|(k, _)| !KEYS.contains(&k.as_str())));
    }
    let mut bytes = view::Ordered::Object(fields).pretty();
    bytes.push(b'\n');
    Ok(bytes)
}
pub(super) fn pretty(path: &Path, d: &Value, previous: Option<&[u8]>) -> Result<Vec<u8>> {
    let old = previous_document(path, previous)?;
    let mut fields = Vec::new();
    for k in KEYS {
        if let Some(v) = d.get(k) {
            let mut ordered = view::Ordered::from_value(v);
            if k == "models" {
                let mut engines = vec![];
                for (engine, c) in v.as_object().unwrap() {
                    let mut fields = vec![];
                    for key in ["auto", "tiers", "policy"] {
                        if let Some(p) = c.get(key) {
                            let value = if key == "policy" {
                                view::Ordered::Object(
                                    p.as_object()
                                        .unwrap()
                                        .iter()
                                        .map(|(model, p)| {
                                            let members = [
                                                "effort_floor",
                                                "max_effort",
                                                "effort_ladder",
                                                "also_standard_for",
                                                "preferred_for",
                                                "preferred_when",
                                                "request_names",
                                            ]
                                            .iter()
                                            .filter_map(|key| {
                                                p.get(*key).map(|v| {
                                                    (key.to_string(), view::Ordered::from_value(v))
                                                })
                                            })
                                            .collect();
                                            (model.clone(), view::Ordered::Object(members))
                                        })
                                        .collect(),
                                )
                            } else {
                                view::Ordered::from_value(p)
                            };
                            fields.push((key.into(), value));
                        }
                    }
                    engines.push((engine.clone(), view::Ordered::Object(fields)));
                }
                ordered = view::Ordered::Object(engines);
            }
            fields.push((k.into(), ordered));
        }
    }
    if let Some(view::Ordered::Object(old)) = old {
        for (k, v) in old {
            if !KEYS.contains(&k.as_str()) {
                fields.push((k, v));
            }
        }
    }
    let mut bytes = view::Ordered::Object(fields).pretty();
    bytes.push(b'\n');
    Ok(bytes)
}
pub(super) fn save(
    root: &Path,
    config: &crate::Config,
    params: &Value,
    current: &Value,
) -> Result<Value> {
    let expected = super::expected(params, "routing")?;
    let d = if params["value"].is_null() {
        Value::Null
    } else {
        document(&params["value"], &current["value"]["routing"])?
    };
    // Existing legacy tiers, not the displayed legacyTiers list, survive edits.
    let path = context::key(root)
        .map_err(|_| {
            refuse(
                "project_context_conflict",
                "The project store refused the request.",
            )
        })?
        .directory()
        .join("routing.json");
    let before = reader::read_plain(&path, false)?;
    let previous = before
        .as_ref()
        .and_then(|b| serde_json::from_slice::<Value>(b).ok())
        .filter(super::super::project_routing::valid);
    let mut d = d;
    if let Some(t) = previous.as_ref().and_then(|r| r.get("tiers")) {
        if d.is_object() {
            d["tiers"] = t.clone();
        }
    }
    validate::rules(&d)?;
    let folder = reader::owner_folder(config)?;
    let _lock = super::owner_lock(&folder)?;
    let data = reader::read_plain(&path, false)?;
    check("routing (project)", &expected, &token(data.as_deref()))?;
    let body = pretty(&path, &d, data.as_deref())?;
    // Build the complete response BEFORE effects. A failed catalog/display observation
    // cannot turn a committed write into a safe-to-replay failure.
    let mut reply = view::routing(root, config, Some(&body))?;
    reply["changed"] = json!(Some(body.as_slice()) != data.as_deref());
    reply["target"] = json!("project");
    reply["written_revision"] = json!(token(Some(&body)));
    reply["shadowed_by"] = Value::Null;
    reply["profile"] = Value::Null;
    if serde_json::to_vec(&reply).unwrap().len() > FRAME_BUDGET {
        return Err(refuse(
            "machine_reply_too_large",
            "Project settings exceed the native reply limit.",
        ));
    }
    replace(&path, &body)?;
    Ok(reply)
}
