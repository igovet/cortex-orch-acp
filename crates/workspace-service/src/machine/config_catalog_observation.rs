//! Strict ACP configOptions interpretation and Python-compatible discovery snapshots.
use super::*;
use sha2::{Digest, Sha256};
#[path = "config_catalog_unicode.rs"]
pub(super) mod unicode;

pub(super) struct Observation {
    options: Vec<Value>,
    pub(super) models: Vec<Value>,
    pairs: Vec<(String, String)>,
    levels: BTreeMap<String, Vec<String>>,
}
impl Observation {
    /// Whether the select option `id` offers `value`.
    pub(super) fn offers(&self, id: &str, value: &str) -> bool {
        self.options.iter().any(|o| {
            o["id"] == id
                && o["type"] == "select"
                && o["options"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|c| c["value"] == value))
        })
    }
    pub(super) fn enrich(&mut self, levels: BTreeMap<String, Vec<String>>) {
        self.levels.extend(levels);
    }
}
fn invalid() -> String {
    "invalid_observation:invalid_observation".into()
}
fn nonempty(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.is_empty())
}
fn choices(v: &Value) -> std::result::Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for choice in v.as_array().ok_or_else(invalid)? {
        if choice.get("value").is_some() {
            if !nonempty(&choice["value"]) || !nonempty(&choice["name"]) {
                return Err(invalid());
            }
            out.push(choice.clone());
        } else if choice.get("group").is_some() {
            for nested in choice["options"].as_array().ok_or_else(invalid)? {
                if !nonempty(&nested["value"]) || !nonempty(&nested["name"]) {
                    return Err(invalid());
                }
                out.push(nested.clone());
            }
        } else {
            return Err(invalid());
        }
    }
    Ok(out)
}
pub(super) fn parse(raw: &Value) -> std::result::Result<Observation, String> {
    let mut ids = BTreeSet::new();
    let mut options = Vec::new();
    let mut models = None;
    let mut pairs = Vec::new();
    let mut levels = Vec::new();
    for item in raw.as_array().ok_or_else(invalid)? {
        if !item.is_object()
            || !nonempty(&item["id"])
            || !nonempty(&item["name"])
            || !ids.insert(item["id"].as_str().unwrap())
            || !item["category"].is_null() && !item["category"].is_string()
        {
            return Err(invalid());
        }
        let mut option = item.clone();
        if item["type"] == "select" {
            let choices = choices(&item["options"])?;
            if !nonempty(&item["currentValue"])
                || !choices.iter().any(|c| c["value"] == item["currentValue"])
            {
                return Err(invalid());
            }
            if item["category"] == "model" && models.is_none() {
                models = Some(choices.clone());
            }
            for c in &choices {
                let value = c["value"].as_str().unwrap().to_owned();
                match item["category"].as_str() {
                    Some("mode" | "model_config") => {
                        pairs.push((item["category"].as_str().unwrap().into(), value))
                    }
                    Some("thought_level") => levels.push(value),
                    _ => {}
                }
            }
            option["options"] = json!(choices);
        } else if item["type"] == "boolean" && item["currentValue"].is_boolean() {
            pairs.push((
                item["id"].as_str().unwrap().into(),
                item["currentValue"].to_string(),
            ));
        } else {
            return Err(invalid());
        }
        options.push(option);
    }
    let models = models.unwrap_or_default();
    let levels = models
        .iter()
        .map(|m| (m["value"].as_str().unwrap().into(), levels.clone()))
        .collect();
    Ok(Observation {
        options,
        models,
        pairs,
        levels,
    })
}
pub(super) async fn model_efforts(
    peer: &mut Peer,
    result: &Value,
    raw: Value,
) -> std::result::Result<Observation, String> {
    let mut observation = parse(&raw).map_err(|_| "engine_unreachable:PortableError".to_owned())?;
    let Some(option) = observation
        .options
        .iter()
        .find(|o| o["category"] == "model" && o["type"] == "select")
        .cloned()
    else {
        return Ok(observation);
    };
    let mut levels = BTreeMap::new();
    // Each failure names what the engine did, not a Python exception class: an engine that
    // never confirms the model it was switched to (seen with OpenCode) reads as such.
    let selected: std::result::Result<(),String> = async {
        for model in &observation.models {
            let reply = peer.call("session/set_config_option",json!({"sessionId":result["sessionId"],"configId":option["id"],"value":model["value"]})).await?;
            if reply.get("configOptions").is_none() { return Err("engine_unreachable:model_switch_unanswered".into()); }
            let selected = parse(&reply["configOptions"]).map_err(|_| "engine_unreachable:PortableError".to_owned())?;
            let chosen = selected.options.iter().find(|o| o["category"] == "model")
                .ok_or_else(|| "engine_unreachable:model_option_missing".to_owned())?;
            if chosen["currentValue"] != model["value"] { return Err("engine_unreachable:model_switch_unconfirmed".into()); }
            let id = model["value"].as_str().unwrap();
            levels.insert(id.into(), selected.levels.get(id).cloned().unwrap_or_default());
        }
        Ok(())
    }.await;
    // Restore the initial model even after a failed selection, as the Python finally does.
    peer.call("session/set_config_option",json!({"sessionId":result["sessionId"],"configId":option["id"],"value":option["currentValue"]})).await?;
    selected?;
    observation.levels = levels;
    Ok(observation)
}
fn component(s: &str) -> Result<String> {
    unicode::component(s).ok_or_else(|| {
        refuse(
            "invalid_catalog:invalid_field",
            "The observed model has no displayable label.",
        )
    })
}
pub(super) fn snapshot(r: &Value, observation: Observation) -> Result<Value> {
    if observation.models.is_empty() {
        return Err(refuse(
            "no_model_options_advertised",
            "The engine advertised no model choices.",
        ));
    }
    let now = cortex_core::now();
    let id = format!("cs_{}", random_id().map_err(|_| unknown())?);
    let mut hash = Sha256::new();
    for key in ["id", "approved_fingerprint", "state"] {
        hash.update(r[key].as_str().unwrap().as_bytes());
    }
    let registry = format!("{:x}", hash.finalize());
    let provenance = json!({"source":"discovery","adapter_version":"unknown","observed_at":now,"evidence_ref":null});
    let mut evidence = json!({"catalog_obtained":provenance});
    if matches!(
        r["state"].as_str(),
        Some("configured" | "authorization_required")
    ) {
        evidence[r["state"].as_str().unwrap()] = provenance.clone();
    }
    let mut entries = Vec::new();
    for model in &observation.models {
        let value = model["value"].as_str().unwrap();
        let mut pairs = observation.pairs.clone();
        pairs.extend(
            observation
                .levels
                .get(value)
                .into_iter()
                .flatten()
                .map(|v| ("thought_level".into(), v.clone())),
        );
        // SDK-unknown Claude fallback has its own qualification; keep it explicit.
        if r["engine"] == "claude-agent-acp"
            && !pairs.iter().any(|(k, _)| k == "thought_level")
            && model["name"] == model["value"]
        {
            return Err(unavailable("Claude SDK-unknown model effort fallback is not qualified; no snapshot was published."));
        }
        // Python ExecutionRoute keeps repeated pairs (Claude's several
        // model_config on/off selects) in its sorted, stored identity.
        pairs.sort();
        let route_id = crate::native_routes::catalog_route_id(
            &r["engine"],
            &r["id"],
            &json!(value),
            pairs.clone(),
        );
        let body = json!({"engine":r["engine"],"connection_id":r["id"],"model":value,"supported_parameters":pairs});
        let label = format!(
            "{} · {} · {}",
            r["engine"].as_str().unwrap(),
            component(r["name"].as_str().unwrap())?,
            component(value)?
        );
        let mut route = body;
        route["id"] = json!(route_id);
        entries.push(json!({"route":route,"connection_id":r["id"],"label":label,"disambiguated_label":label,
            "evidence":evidence,"catalog_snapshot_id":id,"model_name":model["name"],
            "model_description":model["description"].as_str().filter(|s| !s.is_empty()),"context_window_tokens":null}));
    }
    let mut counts = BTreeMap::<String, usize>::new();
    for e in &entries {
        *counts
            .entry(e["label"].as_str().unwrap().into())
            .or_default() += 1;
    }
    for e in &mut entries {
        let label = e["label"].as_str().unwrap();
        if counts[label] > 1 {
            e["disambiguated_label"] = json!(format!(
                "{} (#{})",
                label,
                &e["route"]["id"].as_str().unwrap()[3..11]
            ));
        }
    }
    Ok(
        json!({"snapshot_id":id,"created_at":now,"connection_registry_fingerprint":registry,
        "setup_action":null,"unavailable_reason":null,"entries":entries}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    /// claude-agent-acp 0.97.0 and 0.107.0 without boolean client support: `fast` and
    /// `ultracode` are both `model_config` on/off selects.
    #[test]
    fn repeated_claude_model_config_pairs_keep_the_oracle_route_identity() {
        let select = |id: &str, category: &str, current: &str, values: &[&str]| {
            json!({"id":id,"name":id,"category":category,"type":"select","currentValue":current,
                "options":values.iter().map(|v| json!({"value":v,"name":v})).collect::<Vec<_>>()})
        };
        let raw = json!([
            select(
                "mode",
                "mode",
                "default",
                &[
                    "default",
                    "acceptEdits",
                    "plan",
                    "auto",
                    "bypassPermissions"
                ]
            ),
            select("model", "model", "claude-sonnet-5", &["claude-sonnet-5"]),
            select(
                "effort",
                "thought_level",
                "medium",
                &["low", "medium", "high"]
            ),
            select("fast", "model_config", "off", &["on", "off"]),
            select("ultracode", "model_config", "off", &["on", "off"]),
        ]);
        let mut observation = parse(&raw).unwrap();
        observation.models[0]["name"] = json!("Claude Sonnet 5");
        let record = json!({"id":"cn_000000000002","approved_fingerprint":"fixture","state":"configured",
            "engine":"claude-agent-acp","name":"Claude"});
        let doc = snapshot(&record, observation).unwrap();
        let route = &doc["entries"][0]["route"];
        // ExecutionRoute(...).id from the Python oracle for the same observation.
        assert_eq!(route["id"], "xr_b067806096f03b639ac4fcf2");
        assert_eq!(
            route["supported_parameters"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|p| p[0] == "model_config")
                .count(),
            4
        );
    }
}
