//! Validation for the project routing editor, not dispatch authorization.
//! Invalid documents retain their byte revision but display the legacy empty view.
use serde_json::{json, Value};
use std::collections::BTreeSet;
pub(super) const ENGINES: [&str; 3] = ["claude-agent-acp", "codex-acp", "opencode"];
const TIERS: [&str; 4] = ["flagship", "deep", "standard", "light"];
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const KINDS: [&str; 13] = [
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
fn model(v: &Value) -> bool {
    v.as_str().is_some_and(|s| {
        (1..=256).contains(&s.chars().count())
            && !s
                .chars()
                .any(|c| c.is_whitespace() || c < ' ' || c == '\u{7f}')
    })
}
fn list(v: &Value, unique: bool, valid: impl Fn(&Value) -> bool) -> bool {
    v.as_array().is_some_and(|a| {
        a.iter().all(valid)
            && (!unique
                || a.iter()
                    .map(Value::to_string)
                    .collect::<BTreeSet<_>>()
                    .len()
                    == a.len())
    })
}
fn tiers(v: &Value) -> bool {
    v.as_str().is_some_and(|s| TIERS.contains(&s))
        || v.as_array().is_some_and(|a| {
            !a.is_empty() && list(v, true, |v| v.as_str().is_some_and(|s| TIERS.contains(&s)))
        })
}
fn effort(v: &Value) -> Option<usize> {
    v.as_str()
        .and_then(|s| EFFORTS.iter().position(|e| *e == s))
}
// Python's casefold-fixed-point test differs from lowercase for these Unicode
// characters. Cherokee uppercase is itself casefold-stable; its lowercase is not.
pub(super) fn casefold_stable(s: &str) -> bool {
    s.chars().all(|c| {
        let n = c as u32;
        (0x13a0..=0x13f5).contains(&n)
            || (c.to_lowercase().eq(std::iter::once(c))
                && !matches!(n,
                0xb5 | 0xdf | 0x149 | 0x17f | 0x1f0 | 0x345 | 0x390 | 0x3b0 |
                0x3c2 | 0x3d0..=0x3d1 | 0x3d5..=0x3d6 | 0x3f0..=0x3f1 |
                0x3f5 | 0x587 | 0x13f8..=0x13fd | 0x1c80..=0x1c88 |
                0x1e96..=0x1e9b | 0x1f50 | 0x1f52 | 0x1f54 | 0x1f56 |
                0x1f80..=0x1f87 | 0x1f90..=0x1f97 | 0x1fa0..=0x1fa7 |
                0x1fb2..=0x1fb4 | 0x1fb6..=0x1fb7 | 0x1fbe | 0x1fc2..=0x1fc4 |
                0x1fc6..=0x1fc7 | 0x1fd2..=0x1fd3 | 0x1fd6..=0x1fd7 |
                0x1fe2..=0x1fe4 | 0x1fe6..=0x1fe7 | 0x1ff2..=0x1ff4 |
                0x1ff6..=0x1ff7 | 0xab70..=0xabbf | 0xfb00..=0xfb06 | 0xfb13..=0xfb17))
    })
}
fn policy(p: &Value) -> bool {
    let Some(m) = p.as_object() else { return false };
    if m.keys().any(|k| {
        ![
            "effort_floor",
            "max_effort",
            "effort_ladder",
            "also_standard_for",
            "preferred_for",
            "preferred_when",
            "request_names",
        ]
        .contains(&k.as_str())
    }) {
        return false;
    }
    for key in ["effort_floor", "max_effort"] {
        if !p[key].is_null() && effort(&p[key]).is_none() {
            return false;
        }
    }
    if let (Some(a), Some(b)) = (effort(&p["effort_floor"]), effort(&p["max_effort"])) {
        if a > b {
            return false;
        }
    }
    if !p["effort_ladder"].is_null()
        && !p["effort_ladder"].as_array().is_some_and(|a| {
            (1..=3).contains(&a.len())
                && a.iter().all(|v| effort(v).is_some())
                && a.windows(2).all(|w| effort(&w[0]) < effort(&w[1]))
        })
    {
        return false;
    }
    for key in ["also_standard_for", "preferred_for"] {
        if !p[key].is_null()
            && !list(&p[key], true, |v| {
                v.as_str().is_some_and(|s| KINDS.contains(&s))
            })
        {
            return false;
        }
    }
    if !p["preferred_when"].is_null()
        && !p["preferred_when"].as_str().is_some_and(|s| {
            !s.trim().is_empty()
                && s.chars().count() <= 500
                && !s
                    .chars()
                    .any(|c| (c < ' ' && c != '\n' && c != '\t') || c == '\u{7f}')
        })
    {
        return false;
    }
    if !p["request_names"].is_null()
        && !p["request_names"].as_array().is_some_and(|a| {
            (1..=16).contains(&a.len())
                && list(&p["request_names"], true, |v| {
                    v.as_str().is_some_and(|s| {
                        (2..=32).contains(&s.chars().count())
                            && {
                                static NAME: std::sync::OnceLock<regex::Regex> =
                                    std::sync::OnceLock::new();
                                NAME.get_or_init(|| {
                                    regex::Regex::new(r"^[\p{L}\p{N}_-]{2,32}$").unwrap()
                                })
                                .is_match(s)
                            }
                            && casefold_stable(s)
                            && !crate::native_routes::reserved_policy_word(s)
                    })
                })
        })
    {
        return false;
    }
    true
}
pub(super) fn valid(r: &Value) -> bool {
    valid_inner(r, true, true)
}
pub(crate) fn valid_profile(r: &Value) -> bool {
    valid_inner(r, false, true) && crate::config_read_model::profile_routing_syntax(r)
}
/// Structural validation before named v2 policy diagnostics, without profile syntax.
pub(crate) fn valid_routing_structure(r: &Value) -> bool {
    valid_inner(r, true, false)
}
fn valid_inner(r: &Value, editor: bool, final_policy: bool) -> bool {
    let Some(m) = r.as_object() else { return false };
    let policy_v2 = editor || r["model_policy_version"].as_f64() == Some(2.0);
    if editor && r["model_policy_version"].as_i64() != Some(2) {
        return false;
    }
    if let Some(v) = m.get("pin") {
        if !v.as_object().is_some_and(|m| {
            m.values()
                .all(|v| v.as_str().is_some_and(|s| s.contains('/')))
        }) {
            return false;
        }
    }
    for key in ["deny", "auto_allow"] {
        if let Some(v) = m.get(key) {
            if !list(v, false, |v| v.as_str().is_some_and(|s| !s.is_empty())) {
                return false;
            }
        }
    }
    if m.get("tiers").is_some_and(|v| {
        !v.as_object().is_some_and(|m| {
            if policy_v2 {
                m.is_empty()
            } else {
                m.values()
                    .all(|v| v.as_str().is_some_and(|s| TIERS.contains(&s)))
            }
        })
    }) {
        return false;
    }
    if let Some(v) = m.get("providers") {
        if !v.as_array().is_some_and(|a| !a.is_empty())
            || !list(v, false, |v| {
                v.as_str().is_some_and(|s| ENGINES.contains(&s))
            })
        {
            return false;
        }
    }
    if let Some(v) = m.get("engine_affinity") {
        if !v.as_object().is_some_and(|a| {
            a.iter().all(|(k, v)| {
                (k == "*" || KINDS.contains(&k.as_str()))
                    && (v.is_null() || v.as_str().is_some_and(|s| ENGINES.contains(&s)))
            })
        }) {
            return false;
        }
    }
    if let Some(v) = m.get("enabled") {
        if !v.as_object().is_some_and(|a| {
            a.iter()
                .all(|(k, v)| ENGINES.contains(&k.as_str()) && list(v, true, model))
        }) {
            return false;
        }
    }
    if let Some(v) = m.get("quota_windows") {
        let Some(engines) = v.as_object() else {
            return false;
        };
        for (engine, rules) in engines {
            if !ENGINES.contains(&engine.as_str()) {
                return false;
            }
            let Some(rules) = rules.as_array().filter(|a| a.len() <= 1000) else {
                return false;
            };
            let mut seen = BTreeSet::new();
            for rule in rules {
                if !rule.as_object().is_some_and(|m| {
                    m.len() == 2 && m.contains_key("window") && m.contains_key("models")
                }) || !rule["window"].as_str().is_some_and(|s| {
                    !s.is_empty()
                        && s.chars().count() <= 256
                        && !s.chars().any(|c| c < ' ' || c == '\u{7f}')
                }) || !seen.insert(rule["window"].to_string())
                {
                    return false;
                }
                if !rule["models"].is_null()
                    && (!rule["models"].as_array().is_some_and(|a| a.len() <= 1000)
                        || !list(&rule["models"], true, |v| {
                            model(v)
                                && (engine != "opencode"
                                    || v.as_str().is_some_and(|s| {
                                        s.contains('/') && !s.starts_with('/') && !s.ends_with('/')
                                    }))
                        }))
                {
                    return false;
                }
            }
        }
    }
    if let Some(v) = m.get("models") {
        let Some(engines) = v.as_object() else {
            return false;
        };
        let mut labels = BTreeSet::new();
        for (engine, config) in engines {
            let Some(c) = config.as_object() else {
                return false;
            };
            if !ENGINES.contains(&engine.as_str())
                || c.keys()
                    .any(|k| !["auto", "tiers", "policy"].contains(&k.as_str()))
            {
                return false;
            }
            if let Some(auto) = c.get("auto") {
                if !auto.as_array().is_some_and(|a| a.len() <= 1000) || !list(auto, true, model) {
                    return false;
                }
                for model in auto.as_array().unwrap() {
                    if policy_v2
                        && final_policy
                        && !tiers(&config["tiers"][model.as_str().unwrap()])
                    {
                        return false;
                    }
                }
            }
            if let Some(v) = c.get("tiers") {
                if !v
                    .as_object()
                    .is_some_and(|m| m.iter().all(|(k, v)| model(&json!(k)) && tiers(v)))
                {
                    return false;
                }
            }
            if let Some(v) = c.get("policy") {
                let Some(policies) = v.as_object() else {
                    return false;
                };
                for (name, p) in policies {
                    if !model(&json!(name)) || !policy(p) {
                        return false;
                    }
                    if let Some(auto) = c.get("auto") {
                        if !auto.as_array().unwrap().contains(&json!(name))
                            && (super::workers::truthy(&p["preferred_for"])
                                || super::workers::truthy(&p["preferred_when"]))
                        {
                            return false;
                        }
                    }
                    for label in p["request_names"].as_array().into_iter().flatten() {
                        if policy_v2 && final_policy && !labels.insert(label.to_string()) {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}
pub(super) fn view(rules: &Value) -> Value {
    let mut engines = json!({});
    for engine in ENGINES {
        let c = &rules["models"][engine];
        let mut policies = json!({});
        for (name, p) in c["policy"].as_object().into_iter().flatten() {
            policies[name] = json!({"effortFloor":p["effort_floor"],"maxEffort":p["max_effort"],
                "effortLadder":p["effort_ladder"],"alsoStandardFor":p["also_standard_for"],
                "preferredFor":p["preferred_for"],"preferredWhen":p["preferred_when"],"requestNames":p["request_names"]});
        }
        let mut ts = json!({});
        for (name, t) in c["tiers"].as_object().into_iter().flatten() {
            ts[name] = if t.is_array() { t.clone() } else { json!([t]) };
        }
        engines[engine] = json!({"enabled":or(&rules["enabled"][engine],json!([])),
            "auto":or(&c["auto"],json!([])),"tiers":ts,"policy":policies,
            "quota":or(&rules["quota_windows"][engine],json!([]))});
    }
    let mut pins = json!({});
    for (role, v) in rules["pin"].as_object().into_iter().flatten() {
        let (route, effort) = v
            .as_str()
            .unwrap()
            .split_once(':')
            .unwrap_or((v.as_str().unwrap(), ""));
        let (engine, model) = route.split_once('/').unwrap_or((route, ""));
        pins[role] = json!({"engine":engine,"model":model,"effort":if effort.is_empty(){Value::Null}else{json!(effort)}});
    }
    json!({"version":2,"deny":or(&rules["deny"],json!([])),"providers":or(&rules["providers"],json!(ENGINES)),
        "engines":engines,"pins":pins,"affinity":or(&rules["engine_affinity"],json!({})),
        "autoAllow":or(&rules["auto_allow"],json!([])),"legacyTiers":[]})
}
fn or(v: &Value, default: Value) -> Value {
    if super::workers::truthy(v) {
        v.clone()
    } else {
        default
    }
}
