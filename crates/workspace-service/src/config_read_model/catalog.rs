//! Complete cached catalog/selector display production, not execution authorization.
//! Stored connection states only: explicit refresh owns revalidation/persistence.
//! No executable discovery, credential/keyring access, engine launch or fallback.
use super::{defaults::latest_snapshot, ConfigurationReader, SettingsRead, ENGINES};
use cortex_core::{CoreError, CoreResult};
use serde_json::{json, Value};
use std::{collections::{BTreeMap, BTreeSet}, path::Path};

fn rows(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten()
}
fn contains(v: &Value, text: &str) -> bool {
    rows(v).any(|v| v == text)
}
fn tokens(v: &Value, engine: &str, model: &str) -> Value {
    json!(rows(v)
        .filter(|v| v.as_str().is_some_and(|s| s == model
            || s == format!("{engine}/{model}")
            || s == format!("{engine}/*")))
        .collect::<Vec<_>>())
}
fn selectable(e: &Value, r: &Value) -> bool {
    r["state"] == "configured"
        && r["auth_mode"] != "gateway"
        && (e["evidence"].get("authorization_required").is_none()
            || e["evidence"].get("configured").is_some())
}
fn observed(e: &Value) -> bool {
    !e["evidence"]
        .as_object()
        .unwrap()
        .values()
        .any(|v| v["source"] == "seed")
}
fn display_name(model: &str) -> String {
    let re = regex::Regex::new(
        r"^(?:claude-)?([a-z]+)-(\d+(?:-\d{1,2})*)(?:-\d{8})?(?:\[([a-z0-9]+)\])?$",
    )
    .unwrap();
    let Some(c) = re.captures(model) else {
        return model.into();
    };
    let family = &c[1];
    let mut name = format!(
        "{}{} {}",
        family[..1].to_uppercase(),
        &family[1..],
        c[2].replace('-', ".")
    );
    if let Some(context) = c.get(3) {
        name += &format!(" ({})", context.as_str().to_uppercase());
    }
    name
}
fn presented(entries: &[Value]) -> Vec<Value> {
    let mut out = vec![];
    for e in entries {
        let mut e = e.clone();
        let model = e["route"]["model"].as_str().unwrap();
        // A pure alias entry (the Claude adapter's `default`) names no concrete model: never offered,
        // even from a catalogue saved before aliases were dropped.
        if crate::native_routes::is_alias_model(model) {
            continue;
        }
        if e["model_name"].is_null() || e["model_name"] == model {
            e["model_name"] = json!(display_name(model));
        }
        out.push(e);
    }
    out
}
/// Owner-facing reason for one connection's unreadable stored catalog.
pub(super) fn problem_message(code: &str) -> &'static str {
    match code {
        "invalid_catalog_snapshot" => {
            "The saved model list for this connection is unreadable. Refresh its models."
        }
        "ambiguous_catalog_snapshot" => {
            "Several saved model lists for this connection have the same time. Refresh its models."
        }
        "unsafe_storage" => "The saved model list for this connection is not private to this user.",
        "native_route_catalog_too_large" => "This connection has too many saved model lists.",
        _ => "The saved model list for this connection could not be read.",
    }
}
fn policy(rules: &Value, engine: &str, model: &str) -> (Value, Vec<&'static str>) {
    let config = &rules["models"][engine];
    let p = &config["policy"][model];
    let tier = &config["tiers"][model];
    let tiers: Vec<_> = ["flagship", "deep", "standard", "light"]
        .into_iter()
        .filter(|t| tier == *t || contains(tier, t))
        .collect();
    let mut attrs = json!({"effort_floor":null,"max_effort":null,"effort_ladder":[],
        "also_standard_for":[],"preferred_for":[],"preferred_when":null,"request_names":[],"attribute_sources":{}});
    for key in [
        "effort_floor",
        "max_effort",
        "effort_ladder",
        "also_standard_for",
        "preferred_for",
        "preferred_when",
        "request_names",
    ] {
        if !p[key].is_null() {
            attrs[key] = p[key].clone();
            attrs["attribute_sources"][key] = json!("configured");
        }
    }
    for key in ["also_standard_for", "preferred_for"] {
        let mut kinds = BTreeSet::new();
        for k in rows(&attrs[key]).filter_map(Value::as_str) {
            if k == "implementation" {
                kinds.extend([
                    "implementation_backend",
                    "implementation_frontend",
                    "implementation_frontend_design",
                ]);
            } else {
                kinds.insert(k);
            }
        }
        attrs[key] = json!(kinds);
    }
    if !p["tier_efforts"].is_null() {
        attrs["tier_efforts"] = p["tier_efforts"].clone();
        attrs["attribute_sources"]["tier_efforts"] = json!("configured");
    }
    (attrs, tiers)
}
impl ConfigurationReader {
    /// Complete catalog surface plus raw selectable entry observations. As in Python
    /// `read_route_catalog`, one connection's refused snapshot becomes its own
    /// `catalog_problems` row and contributes no routes; other connections and the
    /// connection itself stay listed. Nothing is rewritten or removed.
    /// Snapshot absence is a genuine empty observation, never a seed.
    /// The project pair is trusted (logical root, channel routing path), as for routing.
    pub fn catalog_selector(&self, project: Option<(&Path, &Path)>) -> CoreResult<Value> {
        let (registry, _) = self.registry_snapshot()?;
        let routing = self.routing_management(project)?;
        let valid = routing["effective_valid"] == true;
        let empty = json!({});
        let rules = if valid {
            &routing["effective_rules"]
        } else {
            &empty
        };
        let source_rules: Vec<_> = rows(&routing["sources"])
            .filter(|s| s["valid"] == true)
            .collect();
        let mut records: Vec<_> = registry["connections"]
            .as_object()
            .unwrap()
            .values()
            .collect();
        records.sort_by_key(|r| (r["engine"].as_str(), r["name"].as_str(), r["id"].as_str()));
        let mut pairs = vec![];
        let mut entries = vec![];
        let mut problems = vec![];
        let mut times = json!({});
        for r in &records {
            let id = r["id"].as_str().unwrap();
            let read = || -> CoreResult<Option<Value>> {
                let Some(doc) = latest_snapshot(&self.environment, id)? else {
                    return Ok(None);
                };
                if !doc["snapshot_id"]
                    .as_str()
                    .is_some_and(|s| super::matches(r"^cs_[a-f0-9]{12}$", s))
                    || !doc["created_at"].as_str().is_some_and(|s| !s.is_empty())
                {
                    return Err(CoreError::Refused("invalid_catalog_snapshot"));
                }
                // Never allow a stale file for another engine to acquire this record's identity.
                if doc["entries"].as_array().unwrap().iter().any(|e| {
                    e["route"]["engine"] != r["engine"]
                        || e["catalog_snapshot_id"] != doc["snapshot_id"]
                }) {
                    return Err(CoreError::Refused("invalid_catalog_snapshot"));
                }
                Ok(Some(doc))
            };
            let doc = match read() {
                Ok(Some(doc)) => doc,
                Ok(None) => continue,
                Err(CoreError::Refused(code)) => {
                    problems.push(
                        json!({"connection_id":id,"code":code,"message":problem_message(code)}),
                    );
                    continue;
                }
                Err(e) => return Err(e),
            };
            let raw = doc["entries"].as_array().unwrap();
            times[id] = doc["created_at"].clone();
            for e in raw.iter().filter(|e| selectable(e, r)) {
                entries.push(json!({"route_id":e["route"]["id"],"connection_id":id,
                    "connection_name":r["name"],"model":e["route"]["model"],
                    "efforts": rows(&e["route"]["supported_parameters"])
                        .filter(|p| p[0] == "thought_level" && p[1].as_str().is_some_and(crate::native_routes::is_concrete_effort))
                        .map(|p| p[1].clone()).collect::<Vec<_>>()}));
            }
            pairs.extend(presented(raw).into_iter().map(|e| (e, *r)));
        }
        let providers = &rules["providers"];
        // Ids earlier snapshots gave each model: a saved task may still hold one.
        let mut retained = BTreeMap::new();
        for (_, r) in &pairs {
            let id = r["id"].as_str().unwrap().to_owned();
            retained
                .entry(id.clone())
                .or_insert_with(|| crate::native_routes::retained_route_ids(&self.environment.state, &id));
        }
        // A current route is never an earlier id of another current route of the same model.
        let current: BTreeSet<String> = pairs
            .iter()
            .filter_map(|(e, _)| e["route"]["id"].as_str().map(str::to_owned))
            .collect();
        let mut routes = vec![];
        for (e, r) in &pairs {
            let engine = e["route"]["engine"].as_str().unwrap();
            let model = e["route"]["model"].as_str().unwrap();
            let ready = selectable(e, r);
            let deny = tokens(&rules["deny"], engine, model);
            let enabled = contains(&rules["enabled"][engine], model);
            let disabled = !deny.as_array().unwrap().is_empty() || !enabled;
            let seed = !observed(e);
            let provider = providers.is_null() || contains(providers, engine);
            let candidate =
                ready && !r["launch_command"].is_null() && !seed && !disabled && provider;
            let (attrs, tiers) = policy(rules, engine, model);
            let selected = contains(&rules["models"][engine]["auto"], model);
            let automatic = if !valid {
                "routing_invalid"
            } else if !ready {
                "connection_not_ready"
            } else if r["launch_command"].is_null() {
                "no_launch_command"
            } else if disabled {
                "disabled"
            } else if seed {
                "not_observed"
            } else if !provider {
                "engine_excluded_from_automatic_routing"
            } else if !selected {
                "not_selected"
            } else if tiers.is_empty() {
                "policy_missing"
            } else {
                "eligible"
            };
            let mut matches = json!({});
            for s in &source_rules {
                matches[s["source"].as_str().unwrap()] = json!({
                    "deny":tokens(&s["rules"]["deny"],engine,model),
                    "auto_allow":tokens(&s["rules"]["auto_allow"],engine,model)});
            }
            let previous: Vec<_> = retained[r["id"].as_str().unwrap()]
                .get(model)
                .into_iter()
                .flatten()
                .filter(|id| !current.contains(id.as_str()))
                .collect();
            let mut route = json!({"route_id":e["route"]["id"],"engine":engine,"model":model,
                "model_name":e["model_name"],"connection_id":r["id"],"connection_name":r["name"],
                "connection_state":r["state"],"qualified":r["qualified"],"observed":!seed,
                "tier":if candidate { tiers.first().copied() } else { None },
                "tiers":if candidate { tiers.clone() } else { vec![] },
                "tier_source":if candidate && !tiers.is_empty() { Some("configured") } else { None },
                "configured_tier":rules["models"][engine]["tiers"][model],
                "selection":"listed","auto_selected":selected,"policy":attrs,
                "preferred_for":attrs["preferred_for"],"preferred_when":attrs["preferred_when"],
                "preferred_inactive":(!rows(&attrs["preferred_for"]).collect::<Vec<_>>().is_empty() || !attrs["preferred_when"].is_null()) && automatic != "eligible",
                "selectable":ready && !disabled,"in_selector":ready && !disabled,"disabled":disabled,
                "deny_tokens":deny,"enabled":enabled,"engine_disabled":contains(&rules["deny"], &format!("{engine}/*")),
                "default_auto_excluded":null,"auto_allow_tokens":tokens(&rules["auto_allow"],engine,model),
                "automatic":automatic,"superseded_by":null,"matches":matches});
            // Only a route that replaced older ids names them; older readers see no new field.
            if !previous.is_empty() {
                route["previous_route_ids"] = json!(previous);
            }
            routes.push(route);
        }
        let mut engines = vec![];
        for engine in ENGINES {
            let offered: BTreeSet<_> = pairs
                .iter()
                .filter(|(_, r)| r["engine"] == engine)
                .map(|(e, _)| e["route"]["model"].as_str().unwrap())
                .collect();
            let selected = rules["models"][engine]
                .get("auto")
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or(json!([]));
            let enabled = rules["enabled"]
                .get(engine)
                .filter(|v| !v.is_null())
                .cloned()
                .unwrap_or(json!([]));
            let mut matches = json!({});
            for s in &source_rules {
                matches[s["source"].as_str().unwrap()] = json!({
                    "disabled":contains(&s["rules"]["deny"],&format!("{engine}/*")),
                    "automatic":s["rules"]["providers"].is_null() || contains(&s["rules"]["providers"],engine)});
            }
            engines.push(json!({"engine":engine,"optional":engine=="opencode","selection":"listed",
                "selected_models":selected,"unmatched_selected":rows(&selected).filter(|m| !offered.contains(m.as_str().unwrap())).collect::<Vec<_>>(),
                "enabled_models":enabled,"enabled_default":false,
                "unmatched_enabled":rows(&enabled).filter(|m| !offered.contains(m.as_str().unwrap())).collect::<Vec<_>>(),
                "connections":records.iter().filter(|r| r["engine"] == engine).map(|r| json!({
                    "id":r["id"],"name":r["name"],"state":r["state"],"qualified":r["qualified"],
                    "catalog_observed_at":times[r["id"].as_str().unwrap()]})).collect::<Vec<_>>(),
                "disabled":contains(&rules["deny"],&format!("{engine}/*")),
                "automatic":providers.is_null() || contains(providers,engine),"default_auto_excluded":false,
                "route_count":pairs.iter().filter(|(_,r)| r["engine"] == engine).count(),"matches":matches}));
        }
        Ok(
            json!({"routes":{"surface":"route_catalog","project_root":project.map(|(root,_)|root),
            "precedence":routing["precedence"],"effective_source":routing["effective_source"],
            "effective_valid":valid,"providers":providers,"engines":engines,"routes":routes,
            "catalog_problems":problems,"not_shown":["recorded_unavailability","quota"]},"entries":entries}),
        )
    }
    /// Replaces all catalog/selector and management seams with native producers.
    /// Workspace and setup discovery remain supplied; machine dispatch is unchanged.
    pub fn settings_with_catalog_readers(
        &self,
        observations: &Value,
        now: f64,
        project: Option<(&Path, &Path)>,
    ) -> CoreResult<SettingsRead> {
        let mut o = observations.clone();
        if !o.is_object() {
            return Err(CoreError::Refused("invalid_settings_observation"));
        }
        // Retained machine config.read requests the user catalog without a project override.
        let catalog = self.catalog_selector(None)?;
        o["routes"] = catalog["routes"].clone();
        o["entries"] = catalog["entries"].clone();
        self.settings_with_management_readers(&o, now, project)
    }
}

#[cfg(test)]
mod alias_tests {
    use super::*;
    /// A catalogue saved before aliases were dropped may still list `default`: it is never presented.
    #[test]
    fn a_saved_catalogue_listing_the_default_alias_does_not_present_it() {
        let entry = |model: &str, description: Value| json!({"route":{"model":model},"model_name":model,"model_description":description});
        let shown = presented(&[
            entry("default", json!("claude-opus-5")),
            entry("Default", json!(null)),
            entry("claude-sonnet-5", json!(null)),
        ]);
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0]["route"]["model"], "claude-sonnet-5");
    }
}
