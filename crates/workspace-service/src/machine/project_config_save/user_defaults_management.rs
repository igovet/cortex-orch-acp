//! Management-shaped defaults writes. No Workspace editor conversion or state write.
use super::*;
use crate::native_routes::RouteEnvironment;

type Answer = std::result::Result<Value, Value>;
fn error(code: &str, message: impl Into<String>, field: Option<&str>) -> Value {
    json!({"category":"validation","code":code,"message":message.into(),
        "field":field,"retry_eligible":false})
}
fn native(e: Refused) -> Value {
    error(e.code, e.message, None)
}
#[derive(Default)]
pub(in crate::machine) struct Problems(Vec<Value>);
impl Problems {
    pub(in crate::machine) fn add(
        &mut self,
        field: impl Into<String>,
        code: &str,
        message: impl Into<String>,
    ) {
        self.0
            .push(json!({"field":field.into(),"code":code,"message":message.into()}));
    }
    pub(in crate::machine) fn finish(self) -> std::result::Result<(), Value> {
        if self.0.is_empty() {
            return Ok(());
        }
        let shown = self
            .0
            .iter()
            .take(5)
            .map(|e| {
                format!(
                    "{}: {}",
                    e["field"].as_str().unwrap(),
                    e["message"].as_str().unwrap()
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        let more = if self.0.len() > 5 {
            format!(" (and {} more)", self.0.len() - 5)
        } else {
            String::new()
        };
        let mut e = error(
            "invalid_configuration",
            format!("Correct the invalid setting(s); nothing was written. {shown}{more}"),
            self.0[0]["field"].as_str(),
        );
        e["errors"] = json!(self.0);
        Err(e)
    }
}
fn conflict(expected: &str, current: &str) -> Value {
    let mut e = error("revision_conflict", format!("The user_defaults configuration changed after it was read (expected revision {expected}, current {current}); nothing was written. Re-read it and apply the change again."), Some("expected_revision"));
    e["current_revision"] = json!(current);
    e
}
/// `_validate_default_patch`: the patch, or every problem in `e`.
pub(in crate::machine) fn default_patch(raw: &Value, e: &mut Problems) -> Value {
    if raw.is_null() {
        return json!({});
    }
    let Some(m) = raw.as_object() else {
        e.add(
            "defaults",
            "invalid_type",
            "must be an object of setting -> value (null removes it)",
        );
        return json!({});
    };
    for (k, v) in m {
        let field = format!("defaults.{k}");
        if !DEFAULT_KEYS.contains(&k.as_str()) {
            e.add(
                field,
                "unsupported_setting",
                format!(
                    "is not an editable default; use {}",
                    DEFAULT_KEYS.join(", ")
                ),
            );
            continue;
        }
        if v.is_null() {
            continue;
        }
        let mut errors = Errors::default();
        // Reuse the runtime-shaped validators, not the desktop patch builder.
        validate(&json!({k:v}), &mut errors);
        if errors.finish().is_err() {
            // Keep management codes/details; the desktop validator flattens its errors.
            match k.as_str() {
                "coordinator" => e.add(
                    field,
                    "invalid_portable_coordinator",
                    "Use exact engine/model and a parameter string mapping.",
                ),
                "route_id" => e.add(
                    field,
                    "invalid_value",
                    "must be an observed route id (xr_<hex>)",
                ),
                "route_connection_id" => e.add(
                    field,
                    "invalid_value",
                    "must be a connection id (cn_<12 hex>)",
                ),
                "route_model" => {
                    e.add(field, "invalid_value", "must be the route's exact model id")
                }
                "language" => e.add(
                    field,
                    "invalid_value",
                    "must be null (auto) or one of en, ru",
                ),
                "parameters" => {
                    if let Some(p) = v.as_object() {
                        for (name, item) in p {
                            if !crate::config_read_model::matches(r"^[a-z][a-z0-9_]{0,63}$", name)
                                || !crate::config_read_model::clean(item, 128)
                            {
                                e.add(format!("{field}.{name}"),"invalid_value","parameter names are lowercase identifiers and values non-empty strings");
                            }
                        }
                    } else {
                        e.add(
                            field,
                            "invalid_type",
                            "must be an object of parameter name -> value",
                        );
                    }
                }
                _ => unreachable!(),
            }
        }
    }
    raw.clone()
}
const CEILING_KEYS: &[&str] = &[
    "allowed_routes",
    "allowed_connections",
    "allowed_data_recipients",
    "permission_level",
    "limits",
    "spend_limit",
    "spend_limit_enforceable",
    "fallback_policy",
];
const LIMIT_KEYS: &[&str] = &["attempts", "calls", "tokens", "concurrency"];
const FALLBACK_KEYS: &[&str] = &[
    "allowed_target_routes",
    "allowed_failure_classes",
    "budget",
    "data_recipient_consent",
];
fn string_set(raw: &Value, field: &str, e: &mut Problems) -> Value {
    if raw.is_null() {
        return Value::Null;
    }
    if !unique(raw, |v| crate::config_read_model::clean(v, 256)) {
        e.add(
            field,
            "invalid_type",
            "must be null (unrestricted) or an array of unique non-empty strings",
        );
        return Value::Null;
    }
    let mut items = raw.as_array().unwrap().clone();
    items.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    json!(items)
}
/// `parse_ceiling`: the ceiling's stored shape, or every problem in `e`.
pub(in crate::machine) fn ceiling_patch(raw: &Value, e: &mut Problems) -> Value {
    let mut out = ceiling::empty();
    let Some(m) = raw.as_object() else {
        e.add("ceiling", "invalid_type", "must be an object");
        return out;
    };
    for k in m.keys().filter(|k| !CEILING_KEYS.contains(&k.as_str())) {
        e.add(
            format!("ceiling.{k}"),
            "unsupported_setting",
            format!("is not a ceiling setting; use {}", CEILING_KEYS.join(", ")),
        );
    }
    for k in [
        "allowed_routes",
        "allowed_connections",
        "allowed_data_recipients",
    ] {
        out[k] = string_set(&raw[k], &format!("ceiling.{k}"), e);
    }
    if !raw["permission_level"].is_null() {
        if !["READ_ONLY", "STANDARD", "ELEVATED"]
            .iter()
            .any(|s| raw["permission_level"] == *s)
        {
            e.add(
                "ceiling.permission_level",
                "invalid_value",
                "must be null or one of READ_ONLY, STANDARD, ELEVATED",
            );
        } else {
            out["permission_level"] = raw["permission_level"].clone();
        }
    }
    if let Some(limits) = raw.get("limits") {
        if let Some(limits) = limits.as_object() {
            for (k, v) in limits {
                if !LIMIT_KEYS.contains(&k.as_str()) {
                    e.add(
                        format!("ceiling.limits.{k}"),
                        "unsupported_setting",
                        format!("use one of {}", LIMIT_KEYS.join(", ")),
                    );
                } else if v.as_u64().is_none() {
                    e.add(
                        format!("ceiling.limits.{k}"),
                        "invalid_value",
                        "must be a non-negative integer",
                    );
                } else {
                    out["limits"][k] = v.clone();
                }
            }
        } else {
            e.add(
                "ceiling.limits",
                "invalid_type",
                "must be an object of named non-negative integers",
            );
        }
    }
    out["spend_limit"] = raw["spend_limit"].clone();
    if !raw["spend_limit"].is_null() && raw["spend_limit"].as_u64().is_none() {
        e.add(
            "ceiling.spend_limit",
            "invalid_value",
            "must be null or a non-negative integer",
        );
    }
    let enforceable = raw
        .get("spend_limit_enforceable")
        .cloned()
        .unwrap_or(json!(false));
    if !enforceable.is_boolean() {
        e.add(
            "ceiling.spend_limit_enforceable",
            "invalid_type",
            "must be true or false",
        );
    } else if !raw["spend_limit"].is_null() && enforceable != true {
        e.add("ceiling.spend_limit","unenforceable_spend_limit","may only be set with spend_limit_enforceable: true (unknown cost is never treated as zero)");
    }
    out["spend_limit_enforceable"] = enforceable;
    if let Some(f) = raw.get("fallback_policy") {
        if let Some(m) = f.as_object() {
            for k in m.keys().filter(|k| !FALLBACK_KEYS.contains(&k.as_str())) {
                e.add(
                    format!("ceiling.fallback_policy.{k}"),
                    "unsupported_setting",
                    format!("use one of {}", FALLBACK_KEYS.join(", ")),
                );
            }
            for k in ["allowed_target_routes", "allowed_failure_classes"] {
                out["fallback_policy"][k] =
                    string_set(&f[k], &format!("ceiling.fallback_policy.{k}"), e);
            }
            let denied = out["fallback_policy"]["allowed_failure_classes"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .filter(|s| {
                    [
                        "authorization_loss",
                        "cancellation",
                        "provider_restriction",
                        "refusal",
                    ]
                    .contains(s)
                })
                .collect::<Vec<_>>();
            if !denied.is_empty() {
                e.add(
                    "ceiling.fallback_policy.allowed_failure_classes",
                    "fallback_denylisted_failure_class",
                    format!("may never name {}", denied.join(", ")),
                );
            }
            if !f["budget"].is_null() && f["budget"].as_u64().is_none() {
                e.add(
                    "ceiling.fallback_policy.budget",
                    "invalid_value",
                    "must be null or a non-negative integer",
                );
            } else {
                out["fallback_policy"]["budget"] = f["budget"].clone();
            }
            let consent = f
                .get("data_recipient_consent")
                .cloned()
                .unwrap_or(json!(false));
            if !consent.is_boolean() {
                e.add(
                    "ceiling.fallback_policy.data_recipient_consent",
                    "invalid_type",
                    "must be true or false",
                );
            } else {
                out["fallback_policy"]["data_recipient_consent"] = consent;
            }
        } else {
            e.add(
                "ceiling.fallback_policy",
                "invalid_type",
                "must be an object",
            );
        }
    }
    out
}

/// Trusted host roots and management input; never writes Workspace defaults.
/// Errors retain management fields/current_revision/validation details.
pub fn update(
    environment: &RouteEnvironment,
    expected: &Value,
    defaults: &Value,
    ceiling: Option<&Value>,
) -> Answer {
    let expected=expected.as_str().filter(|s| *s=="absent" ||
        (s.len()==71 && s.starts_with("sha256:") && s[7..].bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))))
        .ok_or_else(|| error("invalid_expected_revision","expected_revision must be the exact revision a read returned ('absent' or 'sha256:<64 hex>').",Some("expected_revision")))?;
    let mut problems = Problems::default();
    let patch = default_patch(defaults, &mut problems);
    let new_ceiling = ceiling.map(|v| {
        if v.is_null() {
            ceiling::empty()
        } else {
            ceiling_patch(v, &mut problems)
        }
    });
    problems.finish()?;
    if [&environment.config, &environment.state].iter().any(|path| {
        path.components().any(|part| matches!(part, std::path::Component::Normal(s) if s == ".cortex" || s == ".cortex-dev"))
    }) {
        return Err(error("connection_storage_under_project",
            "Connection storage must be user-scoped, never inside a project's Cortex store.", None));
    }
    environment.validate().map_err(|e| {
        error(
            e.code(),
            "The host configuration environment is unsafe.",
            None,
        )
    })?;
    let folder = environment.config.join("cortex");
    // The existing private-tree/lock implementation rejects links and unsafe parents.
    let _lock = owner_lock(&folder).map_err(native)?;
    ignore(&folder).map_err(native)?;
    let path = folder.join("defaults.json");
    let before = reader::read_plain(&path, true).map_err(native)?;
    let revision = token(before.as_deref());
    if revision != expected {
        return Err(conflict(expected, &revision));
    }
    let (mut merged, mut document) = parsed(before.as_deref()).map_err(native)?;
    for (k, v) in patch.as_object().unwrap() {
        if v.is_null() {
            merged.as_object_mut().unwrap().remove(k);
        } else {
            merged[k] = v.clone();
        }
    }
    let route_keys = ["route_id", "route_connection_id", "route_model"];
    let mut problems = Problems::default();
    if !merged["coordinator"].is_null() && route_keys.iter().any(|k| !merged[*k].is_null()) {
        problems.add(
            "defaults.coordinator",
            "route_conflict",
            "remove the bound route before setting a portable coordinator",
        );
        problems.finish()?;
        unreachable!();
    }
    if route_keys.iter().any(|k| patch.get(*k).is_some()) {
        let present = route_keys.iter().filter(|k| !merged[**k].is_null()).count();
        if present != 0 && present != 3 {
            problems.add(
                "defaults",
                "route_incomplete",
                "route_id, route_connection_id and route_model are set or removed together",
            );
        } else if present == 3 {
            let bytes =
                reader::read_plain(&folder.join("connections.json"), true).map_err(native)?;
            let input = bytes
                .as_deref()
                .map(serde_json::from_slice::<Value>)
                .transpose()
                .map_err(|_| {
                    error(
                        "invalid_registry",
                        "The connection registry is invalid; nothing was written.",
                        None,
                    )
                })?;
            let registry = parse_connections(input.as_ref()).map_err(|code| {
                error(
                    code,
                    "The connection registry is invalid; nothing was written.",
                    None,
                )
            })?;
            if merged["route_connection_id"]
                .as_str()
                .is_none_or(|id| registry["connections"].get(id).is_none())
            {
                problems.add(
                    "defaults.route_connection_id",
                    "unknown_connection",
                    "names no registered connection",
                );
            }
        }
    }
    problems.finish()?;
    let current_ceiling = defaults_document(before.as_deref())?.1;
    let target = new_ceiling.unwrap_or(current_ceiling);
    document["defaults"] = merged.clone();
    document["ceiling"] = target.clone();
    let body = encode(&document, false);
    let preserved = merged
        .as_object()
        .unwrap()
        .keys()
        .filter(|k| !DEFAULT_KEYS.contains(&k.as_str()))
        .collect::<Vec<_>>();
    let reply = json!({"surface":"user_defaults","path":path,"revision":token(Some(&body)),
        "exists":true,"defaults":merged,"editable_keys":DEFAULT_KEYS,"preserved_keys":preserved,
        "languages":["en","ru"],"ceiling":target,"ceiling_set":true,"changed":Some(body.as_slice())!=before.as_deref()});
    if body.len() > FRAME_BUDGET || serde_json::to_vec(&reply).unwrap().len() > FRAME_BUDGET {
        return Err(error(
            "machine_reply_too_large",
            "Settings exceed the native write limit.",
            None,
        ));
    }
    let current = token(reader::read_plain(&path, true).map_err(native)?.as_deref());
    if current != revision {
        return Err(conflict(expected, &current));
    }
    replace(&path, &body).map_err(native)?;
    Ok(reply)
}
