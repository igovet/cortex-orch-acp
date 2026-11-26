use super::*;
use view::Ordered;
/// The request's own key order for each mapping the validator walks (Python
/// iterates dicts in insertion order; serde maps are sorted). `None` keeps
/// the sorted order of desktop-built documents.
#[derive(Clone, Copy)]
struct Order<'a>(Option<&'a Ordered>);
impl<'a> Order<'a> {
    fn child(self, key: &str) -> Order<'a> {
        Order(match self.0 {
            Some(Ordered::Object(m)) => m.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        })
    }
    fn entries<'v>(self, map: &'v serde_json::Map<String, Value>) -> Vec<(&'v String, &'v Value)> {
        match self.0 {
            Some(Ordered::Object(m)) => {
                let mut seen = std::collections::BTreeSet::new();
                m.iter()
                    .filter_map(|(k, _)| map.get_key_value(k))
                    .filter(|(k, _)| seen.insert(*k))
                    .collect()
            }
            _ => map.iter().collect(),
        }
    }
}
const ATTRS: [&str; 8] = [
    "effort_floor",
    "max_effort",
    "effort_ladder",
    "tier_efforts",
    "also_standard_for",
    "preferred_for",
    "preferred_when",
    "request_names",
];
fn effort(v: &Value) -> Option<usize> {
    v.as_str()
        .and_then(|s| EFFORTS[..5].iter().position(|x| *x == s))
}
fn engine(v: &Value) -> bool {
    v.as_str().is_some_and(|s| ENGINES.contains(&s))
}
fn tiers(v: &Value) -> bool {
    v.as_str().is_some_and(|s| TIERS.contains(&s))
        || v.as_array().is_some_and(|a| {
            !a.is_empty() && unique(v, |v| v.as_str().is_some_and(|s| TIERS.contains(&s)))
        })
}
fn check_model(e: &mut Errors, field: &str, v: &Value, eng: &str) -> bool {
    if !model(v) {
        e.coded(
            field,
            "invalid_model",
            "must be an exact model id without whitespace (at most 256 characters)",
        );
        false
    } else if eng == "opencode" && !exact(v) {
        e.coded(
            field,
            "model_not_exact",
            format!(
                "{} is not an exact {eng} model id",
                repr(v.as_str().unwrap())
            ),
        );
        false
    } else {
        true
    }
}
fn policy(e: &mut Errors, where_: &str, p: &Value, o: Order) {
    let attrs = ATTRS.join(", ");
    let efforts = EFFORTS[..5].join(", ");
    let Some(m) = p.as_object() else {
        e.coded(
            where_,
            "invalid_type",
            format!("must be an object of {attrs}"),
        );
        return;
    };
    // UI conversion inserts these fields in ATTRS order, irrespective of JSON order.
    for (key, _) in o.entries(m) {
        if !ATTRS.contains(&key.as_str()) {
            e.coded(
                format!("{where_}.{key}"),
                "unsupported_setting",
                format!("use one of {attrs}"),
            );
        }
    }
    for k in ["effort_floor", "max_effort"] {
        if !p[k].is_null() && effort(&p[k]).is_none() {
            e.coded(
                format!("{where_}.{k}"),
                "invalid_effort",
                format!("must be null or one of {efforts}"),
            );
        }
    }
    if let (Some(a), Some(b)) = (effort(&p["effort_floor"]), effort(&p["max_effort"])) {
        if a > b {
            e.add(
                format!("{where_}.effort_floor"),
                "effort_floor must not be above max_effort",
            );
        }
    }
    if !p["effort_ladder"].is_null()
        && !p["effort_ladder"].as_array().is_some_and(|a| {
            (1..=3).contains(&a.len())
                && a.iter().all(|v| effort(v).is_some())
                && a.windows(2).all(|w| effort(&w[0]) < effort(&w[1]))
        })
    {
        e.add(
            format!("{where_}.effort_ladder"),
            format!("must be null or 1-3 strictly ascending efforts from {efforts}"),
        );
    }
    for k in ["also_standard_for", "preferred_for"] {
        if !p[k].is_null() && !unique(&p[k], |v| v.as_str().is_some_and(|s| KINDS.contains(&s))) {
            e.add(
                format!("{where_}.{k}"),
                format!(
                    "must be null or unique work kinds from {}",
                    KINDS.join(", ")
                ),
            );
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
        e.add(
            format!("{where_}.preferred_when"),
            "must be null or non-blank text of at most 500 characters without control characters",
        );
    }
    if !p["request_names"].is_null() {
        let names = &p["request_names"];
        if !names
            .as_array()
            .is_some_and(|a| (1..=16).contains(&a.len()) && unique(names, |_| true))
        {
            e.add(
                format!("{where_}.request_names"),
                "must be null or 1-16 unique names",
            );
        } else {
            for (i, n) in names.as_array().unwrap().iter().enumerate() {
                let field = format!("{where_}.request_names[{i}]");
                if !n.as_str().is_some_and(|s| {
                    crate::config_read_model::matches(r"^[\p{L}\p{N}_-]{2,32}$", s)
                        && super::super::super::project_routing::casefold_stable(s)
                }) {
                    e.add(
                        field,
                        "must be a lowercase word or stem of 2-32 letters, digits, '_' or '-'",
                    );
                } else if crate::native_routes::reserved_policy_word(n.as_str().unwrap()) {
                    e.coded(field,"reserved_word",format!("{} is an ordinary request, work, means or negation word and cannot name a model",repr(n.as_str().unwrap())));
                }
            }
        }
    }
}
fn models(e: &mut Errors, v: &Value, o: Order) {
    let Some(m) = v.as_object() else {
        e.coded(
            "rules.models",
            "invalid_type",
            "must map an engine id to {auto, tiers, policy}",
        );
        return;
    };
    for (eng, c) in o.entries(m) {
        let oc = o.child(eng);
        let base = format!("rules.models.{eng}");
        if !ENGINES.contains(&eng.as_str()) {
            e.coded(
                base,
                "unknown_engine",
                format!("engine must be one of {}", ENGINES.join(", ")),
            );
            continue;
        }
        let Some(cm) = c.as_object() else {
            e.coded(
                base,
                "invalid_type",
                "must be an object with optional auto, tiers and policy",
            );
            continue;
        };
        for (k, _) in oc.entries(cm) {
            if !["auto", "tiers", "policy"].contains(&k.as_str()) {
                e.coded(
                    format!("{base}.{k}"),
                    "unsupported_setting",
                    "use one of auto, tiers, policy",
                );
            }
        }
        if let Some(a) = cm.get("auto") {
            if !a.as_array().is_some_and(|a| a.len() <= 1000) {
                e.coded(
                    format!("{base}.auto"),
                    "invalid_type",
                    "must be an array of at most 1000 exact model ids",
                );
            } else {
                let mut seen = std::collections::BTreeSet::new();
                for (i, n) in a.as_array().unwrap().iter().enumerate() {
                    let field = format!("{base}.auto[{i}]");
                    if check_model(e, &field, n, eng) && !seen.insert(n.to_string()) {
                        e.coded(
                            field,
                            "duplicate_model",
                            format!("{} is listed twice", repr(n.as_str().unwrap())),
                        );
                    }
                }
            }
        }
        if let Some(t) = cm.get("tiers") {
            if let Some(t) = t.as_object() {
                for (n, t) in oc.child("tiers").entries(t) {
                    let field = format!("{base}.tiers.{n}");
                    if !check_model(e, &field, &json!(n), eng) {
                        continue;
                    }
                    if t.is_array() {
                        if !tiers(t) {
                            e.add(
                                field,
                                format!(
                                    "a list of tiers must be non-empty and unique, drawn from {}",
                                    TIERS.join(", ")
                                ),
                            );
                        }
                    } else if !tiers(t) {
                        e.add(
                            field,
                            format!("tier must be one of {} or a list of them", TIERS.join(", ")),
                        );
                    }
                }
            } else {
                e.coded(
                    format!("{base}.tiers"),
                    "invalid_type",
                    "must map an exact model id to a tier or a list of tiers",
                );
            }
        }
        if let Some(p) = cm.get("policy") {
            let Some(p) = p.as_object() else {
                e.coded(
                    format!("{base}.policy"),
                    "invalid_type",
                    "must map an exact model id to its policy attributes",
                );
                continue;
            };
            for (n, p) in oc.child("policy").entries(p) {
                let where_ = format!("{base}.policy.{n}");
                if !check_model(e, &where_, &json!(n), eng) {
                    continue;
                }
                policy(e, &where_, p, oc.child("policy").child(n));
                let placed = &c["tiers"][n.as_str()];
                let placed: Vec<&str> = TIERS
                    .into_iter()
                    .filter(|t| {
                        placed.as_str() == Some(*t)
                            || placed.as_array().is_some_and(|a| a.contains(&json!(t)))
                    })
                    .collect();
                if !crate::native_routes::tier_efforts_valid(&p["tier_efforts"], &placed) {
                    e.add(
                        format!("{where_}.tier_efforts"),
                        format!("must be null or map tiers this model is placed in to {{min, max}} efforts from {}, min not above max", EFFORTS[..5].join(", ")),
                    );
                }
                if c["auto"].as_array().is_some_and(|a| !a.contains(&json!(n))) {
                    for k in ["preferred_for", "preferred_when"] {
                        if truth(&p[k]) {
                            e.coded(
                                format!("{where_}.{k}"),
                                "preferred_not_selected",
                                "a preferred model must be selected in this engine's auto list",
                            );
                        }
                    }
                }
            }
        }
    }
}
fn quota(v: &Value, o: Order) -> Option<(String, String)> {
    let fail = |f: String, m: &str| Some((format!("rules.{f}"), m.to_owned()));
    let Some(m) = v.as_object() else {
        return fail(
            "quota_windows".into(),
            "must map engines to window rule lists",
        );
    };
    for (eng, rules) in o.entries(m) {
        let base = format!("quota_windows.{eng}");
        if !ENGINES.contains(&eng.as_str()) || !rules.as_array().is_some_and(|a| a.len() <= 1000) {
            return fail(base, "needs a supported engine and at most 1000 rules");
        }
        let mut seen = std::collections::BTreeSet::new();
        for (i, r) in rules.as_array().unwrap().iter().enumerate() {
            let f = format!("{base}[{i}]");
            if !r.as_object().is_some_and(|m| {
                m.len() == 2 && m.contains_key("window") && m.contains_key("models")
            }) {
                return fail(f, "needs exactly window and models");
            }
            if !r["window"].as_str().is_some_and(|s| {
                !s.is_empty()
                    && s.chars().count() <= 256
                    && !s.chars().any(|c| c < ' ' || c == '\u{7f}')
            }) {
                return fail(
                    f,
                    "window must be nonempty text without controls, at most 256 characters",
                );
            }
            if !seen.insert(r["window"].to_string()) {
                return fail(
                    f,
                    "ambiguous quota window: each exact label may appear only once per engine",
                );
            }
            if !r["models"].is_null()
                && !(r["models"].as_array().is_some_and(|a| a.len() <= 1000)
                    && unique(&r["models"], |v| {
                        model(v) && (eng != "opencode" || exact(v))
                    }))
            {
                return fail(f,"models must be null (shared) or unique exact model ids (OpenCode: provider/model)");
            }
        }
    }
    None
}
pub(super) fn rules(r: &Value) -> Result<()> {
    checked(r, false, Order(None))
}
/// The management API's `validate_routing_rules`: also refuses unknown keys,
/// and reports problems in the request's own key order.
pub(super) fn management_rules(r: &Value, ordered: &Ordered) -> Result<()> {
    checked(r, true, Order(Some(ordered)))
}
fn policy_error(code: &'static str, message: impl Into<String>) -> Refused {
    refuse(code, message).with(json!({"field":"model_policy_version"}))
}
fn checked(r: &Value, unknown: bool, o: Order) -> Result<()> {
    let mut e = Errors::default();
    let engines = ENGINES.join(", ");
    let Some(m) = r.as_object() else {
        e.coded(
            "rules",
            "invalid_type",
            format!("must be an object with keys from {}", KEYS.join(", ")),
        );
        return e.finish();
    };
    if unknown {
        // serde's map is key-sorted: Python's `sorted(set(rules) - set(KEYS))`.
        for key in m.keys().filter(|k| !KEYS.contains(&k.as_str())) {
            e.coded(
                format!("rules.{key}"),
                "unsupported_setting",
                format!("use one of {}", KEYS.join(", ")),
            );
        }
    }
    if !r["pin"].is_null() {
        if let Some(p) = r["pin"].as_object() {
            for (role, v) in o.child("pin").entries(p) {
                let f = format!("rules.pin.{role}");
                if !crate::config_read_model::matches(
                    r"^(\*|[A-Za-z0-9][A-Za-z0-9._-]{0,127})$",
                    role,
                ) {
                    e.coded(&f, "invalid_role", "must be a worker role name or *");
                }
                if !model(v) {
                    e.coded(
                        f,
                        "invalid_pin",
                        "must be engine/model or engine/model:effort",
                    );
                    continue;
                }
                let value = v.as_str().unwrap();
                let (route, effort) = value
                    .split_once(':')
                    .map(|(a, b)| (a, Some(b)))
                    .unwrap_or((value, None));
                let (eng, n) = route.split_once('/').unwrap_or((route, ""));
                if !ENGINES.contains(&eng) || n.is_empty() {
                    e.coded(
                        &f,
                        "invalid_pin",
                        format!("must be engine/model[:effort] with engine one of {engines}"),
                    );
                } else if eng == "opencode" && !exact(&json!(n)) {
                    e.coded(&f,"opencode_model_not_exact","an OpenCode pin names the exact provider/model id, e.g. opencode/provider/model");
                }
                if effort.is_some_and(|s| !EFFORTS.contains(&s)) {
                    e.coded(
                        &f,
                        "invalid_effort",
                        format!("effort must be one of {}", EFFORTS.join(", ")),
                    );
                }
            }
        } else {
            e.coded(
                "rules.pin",
                "invalid_type",
                "must map a role name or * to engine/model[:effort]",
            );
        }
    }
    for k in ["deny", "auto_allow"] {
        if r[k].is_null() {
            continue;
        }
        if !unique(&r[k], model) {
            e.coded(
                format!("rules.{k}"),
                "invalid_type",
                "must be an array of unique model tokens without spaces",
            );
            continue;
        }
        for v in r[k].as_array().unwrap() {
            let s = v.as_str().unwrap();
            let pair = s.split_once('/');
            if s.contains('*') {
                if pair.is_some_and(|(a, b)| ENGINES.contains(&a) && b == "*") {
                    continue;
                }
                let msg = if k == "deny" {
                    "the only wildcard is <engine>/* in deny, which disables that whole engine"
                } else {
                    "only exact models or engine/* selectors are supported"
                };
                e.coded(
                    format!("rules.{k}"),
                    "unsupported_wildcard",
                    format!("{}: {msg}", repr(s)),
                );
            } else if let Some((prefix, rest)) = pair {
                if !ENGINES.contains(&prefix) || rest.is_empty() {
                    e.coded(format!("rules.{k}"),"ambiguous_engine_prefix",format!("{}: the part before '/' must be an engine id ({engines}); write an OpenCode model with its engine, e.g. opencode/{s}",repr(s)));
                } else if prefix == "opencode" && k == "auto_allow" && !exact(&json!(rest)) {
                    e.coded(format!("rules.{k}"),"opencode_model_not_exact",format!("{}: auto_allow lifts the OpenCode exclusion only for an exact opencode/<provider>/<model> route",repr(s)));
                }
            }
        }
    }
    if let Some(t) = m.get("tiers") {
        if let Some(t) = t.as_object() {
            for (n, t) in o.child("tiers").entries(t) {
                if !model(&json!(n)) || !t.as_str().is_some_and(|s| TIERS.contains(&s)) {
                    e.add(
                        format!("rules.tiers.{n}"),
                        format!("tier must be one of {}", TIERS.join(", ")),
                    );
                }
            }
        } else {
            e.coded(
                "rules.tiers",
                "invalid_type",
                "must map a model id to a tier",
            );
        }
    }
    if m.contains_key("providers")
        && !(r["providers"].as_array().is_some_and(|a| !a.is_empty())
            && unique(&r["providers"], engine))
    {
        e.add(
            "rules.providers",
            format!("must be a non-empty array of unique engines from {engines}"),
        );
    }
    if let Some(a) = m.get("engine_affinity") {
        if let Some(a) = a.as_object() {
            for (k, v) in o.child("engine_affinity").entries(a) {
                if k != "*" && !KINDS.contains(&k.as_str()) {
                    e.add(
                        format!("rules.engine_affinity.{k}"),
                        format!("work kind must be one of {}, *", KINDS.join(", ")),
                    );
                } else if !v.is_null() && !engine(v) {
                    e.add(
                        format!("rules.engine_affinity.{k}"),
                        format!("must be null or one of {engines}"),
                    );
                }
            }
        } else {
            e.coded(
                "rules.engine_affinity",
                "invalid_type",
                "must map a work kind or * to an engine or null",
            );
        }
    }
    if let Some(a) = m.get("enabled") {
        if let Some(a) = a.as_object() {
            for (eng, v) in o.child("enabled").entries(a) {
                let f = format!("rules.enabled.{eng}");
                if !ENGINES.contains(&eng.as_str()) {
                    e.add(f, format!("engine must be one of {engines}"));
                } else if !unique(v, model) {
                    e.coded(
                        f,
                        "invalid_type",
                        "must be an array of unique exact model ids without spaces",
                    );
                } else if eng == "opencode" && !v.as_array().unwrap().iter().all(exact) {
                    e.coded(
                        f,
                        "opencode_model_not_exact",
                        "an OpenCode model is its exact provider/model id, e.g. provider/model",
                    );
                }
            }
        } else {
            e.coded(
                "rules.enabled",
                "invalid_type",
                "must map an engine id to an array of exact model ids",
            );
        }
    }
    if let Some(v) = m.get("models") {
        models(&mut e, v, o.child("models"));
    }
    if let Some(v) = m.get("quota_windows") {
        if let Some((f, m)) = quota(v, o.child("quota_windows")) {
            e.coded(f, "invalid_quota_windows", m);
        }
    }
    if !crate::native_routes::coordinator_models_valid(&r[crate::native_routes::COORDINATOR_MODELS])
    {
        e.coded(
            "rules.coordinator_models",
            "invalid_coordinator_models",
            format!("must be null or 1-64 unique {{engine, model, tiers}} entries: engine one of {engines}, an exact model id, tiers mapping one or more of {} to {{min, max}} efforts from {}, min not above max", TIERS.join(", "), EFFORTS[..5].join(", ")),
        );
    }
    e.finish()?;
    // Loader-only errors retain their named refusals, not aggregated validation.
    if r["model_policy_version"].is_null() {
        return Err(policy_error("model_policy_migration_required","Explicitly migrate routing.json to model_policy_version=2 with exact enabled/auto identities and per-engine tiers."));
    }
    if r["model_policy_version"].as_i64() != Some(2) {
        return Err(policy_error(
            "model_policy_version_unsupported",
            "Only model_policy_version=2 is supported.",
        ));
    }
    if truth(&r["tiers"]) {
        let mut e = Errors::default();
        e.coded(
            "rules",
            "invalid_routing_overrides",
            "legacy top-level tiers require explicit per-engine migration",
        );
        return e.finish();
    }
    let mut labels = std::collections::BTreeMap::new();
    if let Some(models) = r["models"].as_object() {
        let o = o.child("models");
        for (eng, c) in o.entries(models) {
            for n in c["auto"].as_array().into_iter().flatten() {
                if !tiers(&c["tiers"][n.as_str().unwrap()]) {
                    return Err(policy_error(
                        "policy_missing",
                        format!(
                            "models.{eng}.tiers.{} needs nonempty explicit tiers.",
                            n.as_str().unwrap()
                        ),
                    ));
                }
            }
            if let Some(ps) = c["policy"].as_object() {
                for (n, p) in o.child(eng).child("policy").entries(ps) {
                    for label in p["request_names"].as_array().into_iter().flatten() {
                        let identity = format!("{eng}/{n}");
                        if labels
                            .insert(label.to_string(), identity.clone())
                            .is_some_and(|s| s != identity)
                        {
                            return Err(policy_error(
                                "model_request_label_ambiguous",
                                format!(
                                    "Request label {} names more than one exact identity.",
                                    repr(label.as_str().unwrap())
                                ),
                            ));
                        }
                    }
                }
            }
        }
    }
    Ok(())
}
