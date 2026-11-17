//! User defaults and portable-coordinator display binding, without dispatch authority.
use super::{management::*, profiles, ConfigurationReader, ENGINES};
use crate::native_routes::{read_bytes, RouteEnvironment};
use cortex_core::{CoreError, CoreResult};
use serde_json::{json, Value};
use std::{fs, os::unix::fs::MetadataExt, path::Path};

pub(crate) const DEFAULT_KEYS: [&str; 6] = [
    "coordinator",
    "route_id",
    "route_connection_id",
    "route_model",
    "parameters",
    "language",
];
pub(super) fn empty_ceiling() -> Value {
    profiles::ceiling(&json!({})).unwrap()
}
pub(crate) fn defaults_document(bytes: Option<&[u8]>) -> Result<(Value, Value, bool), Value> {
    let doc = json_object(bytes, "defaults.json", "invalid_user_defaults", json!({}))?;
    let defaults = doc.get("defaults").cloned().unwrap_or(json!({}));
    if !defaults.is_object() {
        return Err(error(
            "invalid_user_defaults",
            "defaults.json 'defaults' must be an object; repair it by hand.",
            None,
        ));
    }
    let ceiling = if let Some(c) = doc.get("ceiling") {
        // Stored snapshots require all serialized dimensions, unlike editable patches.
        let empty = empty_ceiling();
        if !c.is_object()
            || empty
                .as_object()
                .unwrap()
                .keys()
                .any(|k| c.get(k).is_none())
            || empty["fallback_policy"]
                .as_object()
                .unwrap()
                .keys()
                .any(|k| c["fallback_policy"].get(k).is_none())
        {
            return Err(error(
                "invalid_ceiling_snapshot",
                "a stored ceiling snapshot has an unexpected shape.",
                None,
            ));
        }
        // Unknown snapshot metadata is ignored by the retained deserializer.
        let mut known = empty;
        for k in known
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
        {
            known[&k] = c[&k].clone();
        }
        let fallback = known["fallback_policy"].clone();
        known["fallback_policy"] = empty_ceiling()["fallback_policy"].clone();
        for k in known["fallback_policy"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>()
        {
            known["fallback_policy"][&k] = fallback[&k].clone();
        }
        // Stored set dimensions are deduplicated, not treated as editable patches.
        for k in [
            "allowed_routes",
            "allowed_connections",
            "allowed_data_recipients",
        ] {
            dedup(&mut known[k]);
        }
        for k in ["allowed_target_routes", "allowed_failure_classes"] {
            dedup(&mut known["fallback_policy"][k]);
        }
        profiles::ceiling(&known).map_err(|_| {
            error(
                "invalid_ceiling_snapshot",
                "a stored ceiling snapshot has an unexpected shape.",
                None,
            )
        })?
    } else {
        empty_ceiling()
    };
    Ok((defaults, ceiling, doc.get("ceiling").is_some()))
}
fn dedup(v: &mut Value) {
    if let Some(a) = v.as_array_mut() {
        a.sort_by_key(Value::to_string);
        a.dedup();
    }
}
fn portable(defaults: &Value) -> Result<Value, Value> {
    let mut v = defaults["coordinator"].clone();
    let valid = |v: &Value| {
        v.is_object()
            && v.as_object().unwrap().len() == 3
            && v["engine"].as_str().is_some_and(|s| ENGINES.contains(&s))
            && v["model"].as_str().is_some_and(|s| !s.trim().is_empty())
            && v["parameters"].as_object().is_some_and(|m| {
                m.iter()
                    .all(|(k, v)| !k.is_empty() && v.as_str().is_some_and(|s| !s.is_empty()))
            })
    };
    if !valid(&v) {
        return Err(error(
            "invalid_portable_coordinator",
            "Use exact engine/model and a parameter string mapping.",
            Some("defaults.coordinator"),
        ));
    }
    if let Some(p) = defaults.get("parameters") {
        v["parameters"] = p.clone();
    }
    if !valid(&v) {
        return Err(error(
            "invalid_portable_coordinator",
            "Use exact engine/model and a parameter string mapping.",
            Some("defaults.coordinator"),
        ));
    }
    Ok(v)
}
fn private_directory(path: &Path) -> CoreResult<bool> {
    // Reject links in all existing ancestors before enumeration.
    for p in path.ancestors() {
        match fs::symlink_metadata(p) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            _ => return Err(CoreError::Refused("unsafe_storage")),
        }
    }
    match fs::symlink_metadata(path) {
        Ok(m) if m.uid() == unsafe { libc::getuid() } && m.mode() & 0o077 == 0 => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        _ => Err(CoreError::Refused("unsafe_storage")),
    }
}
pub(super) fn latest_snapshot(
    environment: &RouteEnvironment,
    id: &str,
) -> CoreResult<Option<Value>> {
    let base = environment.state.join("cortex/catalog");
    for p in [
        environment.state.join("cortex"),
        base.clone(),
        base.join(id),
    ] {
        if !private_directory(&p)? {
            return Ok(None);
        }
    }
    let mut files = fs::read_dir(base.join(id))?
        .take(4097)
        .collect::<std::io::Result<Vec<_>>>()?;
    if files.len() > 4096 {
        return Err(CoreError::Refused("native_route_catalog_too_large"));
    }
    files.retain(|f| {
        f.file_name()
            .to_str()
            .is_some_and(|s| s.starts_with("cs_") && s.ends_with(".json"))
    });
    let mut dated = Vec::with_capacity(files.len());
    for f in files {
        let m = fs::symlink_metadata(f.path())?;
        if !m.is_file() || m.file_type().is_symlink() {
            return Err(CoreError::Refused("invalid_catalog_snapshot"));
        }
        dated.push((m.modified()?, f));
    }
    dated.sort_by_key(|(time, _)| std::cmp::Reverse(*time));
    // Equal mtimes have no authoritative newest identity; never guess from readdir order.
    // Only the newest identity matters; equal times among older history are harmless.
    if dated.len() > 1 && dated[0].0 == dated[1].0 {
        return Err(CoreError::Refused("ambiguous_catalog_snapshot"));
    }
    for (_, f) in dated {
        if !f
            .file_name()
            .to_str()
            .is_some_and(|s| super::matches(r"^cs_[a-f0-9]{12}\.json$", s))
        {
            return Err(CoreError::Refused("invalid_catalog_snapshot"));
        }
        let bytes = read_bytes(&f.path(), true)?;
        let Some(bytes) = bytes else {
            continue;
        };
        let doc: Value = serde_json::from_slice(&bytes)
            .map_err(|_| CoreError::Refused("invalid_catalog_snapshot"))?;
        if doc["snapshot_id"].as_str() != f.path().file_stem().and_then(|s| s.to_str()) {
            return Err(CoreError::Refused("invalid_catalog_snapshot"));
        }
        let entries = doc["entries"]
            .as_array()
            .ok_or(CoreError::Refused("invalid_catalog_snapshot"))?;
        if entries.is_empty()
            && !doc["unavailable_reason"].is_null()
            && doc["unavailable_reason"] != "no_models_observed"
        {
            continue;
        }
        for e in entries {
            let r = &e["route"];
            let pairs: Vec<(String, String)> =
                serde_json::from_value(r["supported_parameters"].clone())
                    .map_err(|_| CoreError::Refused("invalid_catalog_snapshot"))?;
            if !r["engine"].as_str().is_some_and(|s| ENGINES.contains(&s))
                || !r["model"].as_str().is_some_and(|s| !s.is_empty())
                || !crate::machine::valid_cached_entry(e, id)
                || e["connection_id"] != id
                || r["connection_id"] != id
                || !e["catalog_snapshot_id"].is_string()
                || crate::native_routes::catalog_route_id(
                    &r["engine"],
                    &r["connection_id"],
                    &r["model"],
                    pairs,
                ) != r["id"]
            {
                return Err(CoreError::Refused("invalid_catalog_snapshot"));
            }
        }
        return Ok(Some(doc));
    }
    Ok(None)
}
fn latest(environment: &RouteEnvironment, id: &str) -> CoreResult<Vec<Value>> {
    Ok(latest_snapshot(environment, id)?
        .map(|v| v["entries"].as_array().unwrap().clone())
        .unwrap_or_default())
}
impl ConfigurationReader {
    fn coordinator_resolution(&self, defaults: &Value) -> CoreResult<Value> {
        let resolve = || -> CoreResult<Result<Value, Value>> {
            let v = match portable(defaults) {
                Ok(v) => v,
                Err(e) => return Ok(Err(e)),
            };
            let (registry, _) = self.registry_snapshot()?;
            // No project policy: owner coordinator selection ignores automatic providers.
            let routing = self.routing_management(None)?;
            if routing["effective_valid"] != true {
                let err = routing["sources"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|s| s["source"] == routing["effective_source"])
                    .map(|s| s["error"].clone())
                    .unwrap_or(Value::Null);
                return Ok(Err(if err.is_object() {
                    err
                } else {
                    error("model_policy_migration_required", "Explicitly migrate routing.json to model_policy_version=2 with exact enabled/auto identities and per-engine tiers.", Some("model_policy_version"))
                }));
            }
            let rules = &routing["effective_rules"];
            let mut matches = vec![];
            for (id, record) in registry["connections"].as_object().unwrap() {
                // Python catches the snapshot's PortableError: the binding is
                // unavailable with that reason, never a refused defaults read.
                let entries = match latest(&self.environment, id) {
                    Ok(entries) => entries,
                    Err(CoreError::Refused(code)) => {
                        return Ok(Err(error(
                            code,
                            super::catalog::problem_message(code),
                            Some("defaults.coordinator"),
                        )))
                    }
                    Err(e) => return Err(e),
                };
                if record["state"] != "configured"
                    || record["auth_mode"] == "gateway"
                    || record["launch_command"].is_null()
                {
                    continue;
                }
                for entry in entries {
                    let r = &entry["route"];
                    let evidence = &entry["evidence"];
                    if r["engine"] != v["engine"]
                        || r["model"] != v["model"]
                        || evidence["catalog_obtained"]["source"] != "discovery"
                        || (!evidence["authorization_required"].is_null()
                            && evidence["configured"].is_null())
                        || !rules["enabled"][r["engine"].as_str().unwrap()]
                            .as_array()
                            .is_some_and(|a| a.contains(&r["model"]))
                        || rules["deny"].as_array().is_some_and(|a| {
                            a.contains(&r["model"])
                                || a.contains(&json!(format!(
                                    "{}/{}",
                                    r["engine"].as_str().unwrap(),
                                    r["model"].as_str().unwrap()
                                )))
                                || a.contains(&json!(format!(
                                    "{}/*",
                                    r["engine"].as_str().unwrap()
                                )))
                        })
                    {
                        continue;
                    }
                    matches.push(entry);
                }
            }
            let problem = match matches.len() {
                0 => Some((
                    "portable_coordinator_unavailable",
                    "The configured coordinator is not currently observed/selectable.",
                )),
                1 => None,
                _ => Some((
                    "portable_coordinator_ambiguous",
                    "Multiple current connections/routes match; explicitly select one.",
                )),
            };
            if let Some((code, msg)) = problem {
                return Ok(Err(error(code, msg, Some("defaults.coordinator"))));
            }
            let entry = &matches[0];
            if v["parameters"].as_object().unwrap().iter().any(|(k, v)| {
                !entry["route"]["supported_parameters"]
                    .as_array()
                    .unwrap()
                    .contains(&json!([k, v]))
            }) {
                return Ok(Err(error(
                    "unsupported_dependent_setting",
                    "The configured coordinator parameters are not all observed on this route.",
                    Some("defaults.coordinator"),
                )));
            }
            Ok(Ok(
                json!({"state":"bound","route_id":entry["route"]["id"],"connection_id":entry["connection_id"]}),
            ))
        };
        match resolve()? {
            Ok(v) => Ok(v),
            Err(e) => Ok(json!({"state":"unavailable","error":e})),
        }
    }
    /// Complete defaults management envelope; unsafe storage refuses, never repairs.
    pub fn user_defaults_management_observation(&self) -> CoreResult<Value> {
        let path = self.environment.config.join("cortex/defaults.json");
        let bytes = read_bytes(&path, true)?;
        let (defaults, ceiling, ceiling_set) = match defaults_document(bytes.as_deref()) {
            Ok(v) => v,
            Err(e) => return Ok(failure(e)),
        };
        let preserved: Vec<_> = defaults
            .as_object()
            .unwrap()
            .keys()
            .filter(|k| !DEFAULT_KEYS.contains(&k.as_str()))
            .collect();
        let mut view = json!({"surface":"user_defaults","path":path,"revision":revision(bytes.as_deref()),"exists":bytes.is_some(),
            "defaults":defaults,"ceiling":ceiling,"ceiling_set":ceiling_set,"editable_keys":DEFAULT_KEYS,"preserved_keys":preserved,"languages":["en","ru"]});
        if !view["defaults"]["coordinator"].is_null() {
            view["coordinator_resolution"] = self.coordinator_resolution(&view["defaults"])?;
        }
        Ok(json!({"ok":true,"result":view}))
    }
}
