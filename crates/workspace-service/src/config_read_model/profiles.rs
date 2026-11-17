use super::{clean, integer, matches, ModelResult, ENGINES};
use serde_json::{json, Value};
use std::collections::BTreeSet;
pub(super) const SECTIONS: [&str; 4] = ["routing", "user_defaults", "mcp_servers", "connections"];
fn object(v: &Value) -> ModelResult<&serde_json::Map<String, Value>> {
    v.as_object().ok_or("invalid_profiles")
}
fn keys(v: &Value, allowed: &[&str]) -> bool {
    v.as_object()
        .is_some_and(|m| m.keys().all(|k| allowed.contains(&k.as_str())))
}
fn description(v: &Value) -> bool {
    v.as_str()
        .is_some_and(|s| s.chars().count() <= 256 && !s.chars().any(|c| c < ' ' || c == '\u{7f}'))
}
fn strings(v: &Value) -> bool {
    v.as_array().is_some_and(|a| {
        a.iter().all(|v| clean(v, 256))
            && a.iter()
                .map(|v| v.as_str().unwrap())
                .collect::<BTreeSet<_>>()
                .len()
                == a.len()
    })
}
pub(super) fn ceiling(v: &Value) -> ModelResult<Value> {
    let allowed = [
        "allowed_routes",
        "allowed_connections",
        "allowed_data_recipients",
        "permission_level",
        "limits",
        "spend_limit",
        "spend_limit_enforceable",
        "fallback_policy",
    ];
    if !keys(v, &allowed) {
        return Err("invalid_profiles");
    }
    let mut out = json!({"allowed_routes":null,"allowed_connections":null,"allowed_data_recipients":null,
        "permission_level":null,"limits":{},"spend_limit":null,"spend_limit_enforceable":false,
        "fallback_policy":{"allowed_target_routes":null,"allowed_failure_classes":null,"budget":null,"data_recipient_consent":false}});
    for (k, val) in object(v)? {
        out[k] = val.clone();
    }
    for k in [
        "allowed_routes",
        "allowed_connections",
        "allowed_data_recipients",
    ] {
        if !out[k].is_null() {
            if !strings(&out[k]) {
                return Err("invalid_profiles");
            }
            let mut a = out[k].as_array().unwrap().clone();
            a.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
            out[k] = json!(a);
        }
    }
    if !out["permission_level"].is_null()
        && !out["permission_level"]
            .as_str()
            .is_some_and(|s| ["READ_ONLY", "STANDARD", "ELEVATED"].contains(&s))
    {
        return Err("invalid_profiles");
    }
    if !keys(
        &out["limits"],
        &["attempts", "calls", "tokens", "concurrency"],
    ) || !object(&out["limits"])?
        .values()
        .all(|v| integer(v, u64::MAX))
    {
        return Err("invalid_profiles");
    }
    if !out["spend_limit_enforceable"].is_boolean()
        || (!out["spend_limit"].is_null()
            && (!integer(&out["spend_limit"], u64::MAX) || out["spend_limit_enforceable"] != true))
    {
        return Err("invalid_profiles");
    }
    let f = out["fallback_policy"].clone();
    if !keys(
        &f,
        &[
            "allowed_target_routes",
            "allowed_failure_classes",
            "budget",
            "data_recipient_consent",
        ],
    ) {
        return Err("invalid_profiles");
    }
    let mut full = json!({"allowed_target_routes":null,"allowed_failure_classes":null,"budget":null,"data_recipient_consent":false});
    for (k, v) in object(&f)? {
        full[k] = v.clone();
    }
    for k in ["allowed_target_routes", "allowed_failure_classes"] {
        if !full[k].is_null() {
            if !strings(&full[k]) {
                return Err("invalid_profiles");
            }
            let mut a = full[k].as_array().unwrap().clone();
            a.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
            full[k] = json!(a);
        }
    }
    if full["allowed_failure_classes"].as_array().is_some_and(|a| {
        a.iter().any(|v| {
            v.as_str().is_some_and(|s| {
                [
                    "cancellation",
                    "refusal",
                    "authorization_loss",
                    "provider_restriction",
                ]
                .contains(&s)
            })
        })
    }) || (!full["budget"].is_null() && !integer(&full["budget"], u64::MAX))
        || !full["data_recipient_consent"].is_boolean()
    {
        return Err("invalid_profiles");
    }
    out["fallback_policy"] = full;
    Ok(out)
}
fn defaults(v: &Value) -> bool {
    if !keys(
        v,
        &[
            "coordinator",
            "route_id",
            "route_connection_id",
            "route_model",
            "parameters",
            "language",
        ],
    ) {
        return false;
    }
    let m = v.as_object().unwrap();
    let bound = ["route_id", "route_connection_id", "route_model"]
        .iter()
        .filter(|k| m.contains_key(**k))
        .count();
    if bound != 0 && (bound != 3 || m.contains_key("coordinator")) {
        return false;
    }
    m.iter().all(|(k, v)| {
        !v.is_null()
            && match k.as_str() {
                "coordinator" => {
                    keys(v, &["engine", "model", "parameters"])
                        && v.as_object().unwrap().len() == 3
                        && v["engine"].as_str().is_some_and(|s| ENGINES.contains(&s))
                        && v["model"].as_str().is_some_and(|s| !s.trim().is_empty())
                        && v["parameters"].as_object().is_some_and(|m| {
                            m.iter().all(|(k, v)| {
                                !k.is_empty() && v.as_str().is_some_and(|s| !s.is_empty())
                            })
                        })
                }
                "route_id" => v
                    .as_str()
                    .is_some_and(|s| matches(r"^xr_[a-f0-9]{12,64}$", s)),
                "route_connection_id" => {
                    v.as_str().is_some_and(|s| matches(r"^cn_[a-f0-9]{12}$", s))
                }
                "route_model" => clean(v, 256),
                "language" => v == "ru" || v == "en",
                "parameters" => v.as_object().is_some_and(|m| {
                    m.iter()
                        .all(|(k, v)| matches(r"^[a-z][a-z0-9_]{0,63}$", k) && clean(v, 128))
                }),
                _ => false,
            }
    })
}
pub(super) fn normalize(section: &str, v: &Value) -> ModelResult<Value> {
    match section {
        "routing" => {
            object(v)?;
            if !crate::machine::project_routing::valid_profile(v) {
                return Err("invalid_profiles");
            }
            Ok(v.clone())
        }
        "user_defaults" => {
            if !keys(v, &["defaults", "ceiling"])
                || object(v)?.len() != 2
                || !defaults(&v["defaults"])
            {
                return Err("invalid_profiles");
            }
            Ok(json!({"defaults":v["defaults"],"ceiling":ceiling(&v["ceiling"])?}))
        }
        "mcp_servers" => {
            for (name, policy) in object(v)? {
                if name == "cortex"
                    || !matches(r"^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,63}$", name)
                    || !keys(
                        policy,
                        &[
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
                        ],
                    )
                    || !policy["enabled"].is_boolean()
                {
                    return Err("invalid_profiles");
                }
            }
            Ok(v.clone())
        }
        "connections" => {
            for (id, limit) in object(v)? {
                if !matches(r"^cn_[a-f0-9]{12}$", id)
                    || !keys(limit, &["max_concurrent_sessions"])
                    || object(limit)?.len() != 1
                    || !integer(&limit["max_concurrent_sessions"], 64)
                    || limit["max_concurrent_sessions"] == 0
                {
                    return Err("invalid_profiles");
                }
            }
            Ok(v.clone())
        }
        _ => Err("invalid_profiles"),
    }
}
/// Pure format-1/2 normalization; never materializes a profile or changes backing files.
/// The library returns coarse refusal codes, not the management wire error envelope.
pub fn parse_profiles(value: Option<&Value>) -> ModelResult<Value> {
    let Some(input) = value else {
        return Ok(
            json!({"document":{"format":2,"active":null,"applied":null,"pending":null,"profiles":{}},"migrated_from":null}),
        );
    };
    let m = object(input)?;
    let version = input["format"]
        .as_u64()
        .filter(|n| *n == 1 || *n == 2)
        .ok_or("unsupported_profiles_format")?;
    let profiles = object(&input["profiles"])?;
    if profiles.len() > 32
        || profiles
            .iter()
            .any(|(k, v)| !matches(r"^[a-z0-9][a-z0-9-]{0,39}$", k) || !v.is_object())
        || (!input["active"].is_null()
            && !input["active"]
                .as_str()
                .is_some_and(|s| profiles.contains_key(s)))
        || (!input["applied"].is_null() && !input["applied"].is_object())
    {
        return Err("invalid_profiles");
    }
    let mut doc = if version == 1 {
        json!({"format":2,"profiles":{},"active":input["active"],"applied":null,"pending":null})
    } else {
        Value::Object(m.clone())
    };
    doc["profiles"] = json!({});
    for (name, entry) in profiles {
        let mut e = entry.clone();
        if version == 1 {
            if !entry["rules"].is_object() || !description(&entry["description"]) {
                return Err("invalid_profiles");
            }
            e = json!({"description":entry["description"],"sections":["routing"],"settings":{"routing":entry["rules"]}});
            for key in ["created_at", "updated_at"] {
                if entry[key].is_string() {
                    e[key] = entry[key].clone();
                }
            }
        }
        if !description(&e["description"])
            || !keys(
                &e,
                &[
                    "description",
                    "created_at",
                    "updated_at",
                    "sections",
                    "settings",
                ],
            )
            || ["created_at", "updated_at"]
                .iter()
                .any(|k| !e[k].is_null() && !e[k].is_string())
        {
            return Err("invalid_profiles");
        }
        let sections = e["sections"].as_array().ok_or("invalid_profiles")?;
        if sections.is_empty()
            || sections
                .iter()
                .any(|s| !s.as_str().is_some_and(|s| SECTIONS.contains(&s)))
            || sections
                .iter()
                .map(|s| s.as_str().unwrap())
                .collect::<BTreeSet<_>>()
                .len()
                != sections.len()
            || object(&e["settings"])?.len() != sections.len()
            || object(&e["settings"])?
                .keys()
                .any(|k| !sections.contains(&json!(k)))
        {
            return Err("invalid_profiles");
        }
        let ordered: Vec<_> = SECTIONS
            .iter()
            .filter(|s| sections.contains(&json!(s)))
            .collect();
        let mut settings = json!({});
        for s in &ordered {
            settings[*s] = normalize(s, &e["settings"][*s])?;
        }
        e["sections"] = json!(ordered);
        e["settings"] = settings;
        doc["profiles"][name] = e;
    }
    if version == 1 {
        let applied = &input["applied"];
        if applied["profile"]
            .as_str()
            .is_some_and(|p| profiles.contains_key(p))
        {
            doc["applied"] = json!({"profile":applied["profile"],"at":applied["at"],"revisions":{"routing":applied["routing_revision"]}});
        }
    } else {
        let pending = &input["pending"];
        if !pending.is_null()
            && (!pending.is_object()
                || pending["operation"] != "activate"
                || !pending["profile"]
                    .as_str()
                    .is_some_and(|p| profiles.contains_key(p))
                || !pending["at"].is_string())
        {
            return Err("invalid_profiles");
        }
        for k in ["active", "applied", "pending"] {
            if doc.get(k).is_none() {
                doc[k] = Value::Null;
            }
        }
    }
    Ok(json!({"document":doc,"migrated_from":if version==1 {json!(1)} else {Value::Null}}))
}
