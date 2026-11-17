use super::{integer, matches, ModelResult, ENGINES};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
pub fn route_fingerprint(
    engine: &str,
    provider: &str,
    endpoint: Option<&str>,
    launch: Option<&[String]>,
) -> String {
    format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&json!({
                "engine":engine,"provider":provider,"endpoint":endpoint,"launch_command":launch
            }))
            .unwrap()
        )
    )
}
fn keys(v: &Value, allowed: &[&str]) -> bool {
    v.as_object()
        .is_some_and(|m| m.keys().all(|k| allowed.contains(&k.as_str())))
}
fn nonempty(v: &Value) -> bool {
    v.as_str().is_some_and(|s| !s.is_empty())
}
fn reference(v: &Value) -> bool {
    match v["kind"].as_str() {
        Some("engine_native_dir") => {
            keys(v, &["kind", "path", "owned"])
                && v["path"].as_str().is_some_and(|s| s.starts_with('/'))
                && (v.get("owned").is_none() || v["owned"].is_boolean())
        }
        Some("secret_file") => {
            keys(v, &["kind", "path"])
                && v.as_object().unwrap().len() == 2
                && v["path"].as_str().is_some_and(|s| s.starts_with('/'))
        }
        Some("os_keyring") => {
            keys(v, &["kind", "service", "account"])
                && v.as_object().unwrap().len() == 3
                && ["service", "account"].iter().all(|k| {
                    v[k].as_str().is_some_and(|s| {
                        matches(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$", s) && !s.contains("..")
                    })
                })
        }
        _ => false,
    }
}
/// Normalized registry snapshot, without revalidation, canonical-reference authorization,
/// credential IO, keyring access or any state write. Unknown fields fail closed.
pub fn parse_connections(input: Option<&Value>) -> ModelResult<Value> {
    let empty = json!({"schema_version":1,"connections":{},"override_history":{}});
    let v = input.unwrap_or(&empty);
    if !keys(
        v,
        &[
            "schema_version",
            "connections",
            "override_history",
            "auto_connect_disabled",
        ],
    ) || v["schema_version"] != 1
    {
        return Err("invalid_registry");
    }
    let records = match v.get("connections") {
        None => serde_json::Map::new(),
        Some(v) => v.as_object().cloned().ok_or("invalid_registry")?,
    };
    let mut normalized = json!({});
    let fields = [
        "id",
        "name",
        "engine",
        "provider",
        "auth_mode",
        "key_kind",
        "endpoint",
        "launch_command",
        "credential_ref",
        "approved_fingerprint",
        "state",
        "qualified",
        "created_at",
        "updated_at",
    ];
    for (id, record) in &records {
        let Some(m) = record.as_object() else {
            return Err("invalid_connection_record");
        };
        if fields.iter().any(|k| !m.contains_key(*k))
            || m.keys()
                .any(|k| !fields.contains(&k.as_str()) && k != "max_concurrent_sessions")
        {
            return Err("invalid_connection_record");
        }
        if !matches(r"^cn_[a-f0-9]{12}$", id)
            || record["id"] != *id
            || [
                "name",
                "provider",
                "approved_fingerprint",
                "created_at",
                "updated_at",
            ]
            .iter()
            .any(|k| !nonempty(&record[k]))
            || !record["engine"]
                .as_str()
                .is_some_and(|e| ENGINES.contains(&e))
            || !record["state"].as_str().is_some_and(|s| {
                [
                    "configured",
                    "authorization_required",
                    "unavailable",
                    "invalid",
                ]
                .contains(&s)
            })
            || !record["qualified"].is_boolean()
            || (!record["endpoint"].is_null() && !nonempty(&record["endpoint"]))
            || (!record["launch_command"].is_null()
                && !record["launch_command"]
                    .as_array()
                    .is_some_and(|a| a.iter().all(nonempty)))
            || !reference(&record["credential_ref"])
        {
            return Err("invalid_connection_record");
        }
        let auth = record["auth_mode"].as_str();
        if !match auth {
            Some("native_account") => record["key_kind"].is_null(),
            Some("gateway") => record["engine"] != "opencode" && record["key_kind"].is_null(),
            Some("api_key") => match record["engine"].as_str() {
                Some("codex-acp") => record["key_kind"] == "openai_api_key",
                Some("claude-agent-acp") => {
                    record["key_kind"] == "anthropic_api_key"
                        || record["key_kind"] == "claude_oauth_token"
                }
                _ => false,
            },
            _ => false,
        } {
            return Err("invalid_connection_record");
        }
        let mut r = record.clone();
        if r.get("max_concurrent_sessions").is_none() {
            r["max_concurrent_sessions"] = json!(4);
        }
        if !integer(&r["max_concurrent_sessions"], 64) || r["max_concurrent_sessions"] == 0 {
            return Err("invalid_connection_record");
        }
        if r["credential_ref"]["kind"] == "engine_native_dir"
            && r["credential_ref"].get("owned").is_none()
        {
            r["credential_ref"]["owned"] = json!(true);
        }
        normalized[id] = r;
    }
    let history = v.get("override_history").cloned().unwrap_or(json!({}));
    if !history.as_object().is_some_and(|h| {
        h.values().all(|entries| {
            entries.as_array().is_some_and(|a| {
                a.iter().all(|e| {
                    keys(
                        e,
                        &[
                            "fingerprint",
                            "previous_fingerprint",
                            "engine",
                            "provider",
                            "endpoint",
                            "launch_command",
                            "approved_at",
                            "approved_by_user",
                        ],
                    ) && e.as_object().unwrap().len() == 8
                })
            })
        })
    }) {
        return Err("invalid_registry");
    }
    let disabled = v.get("auto_connect_disabled").cloned().unwrap_or(json!([]));
    if !disabled.as_array().is_some_and(|a| {
        a.iter()
            .all(|v| v.as_str().is_some_and(|s| ENGINES.contains(&s)))
            && a.iter()
                .map(|v| v.as_str().unwrap())
                .collect::<BTreeSet<_>>()
                .len()
                == a.len()
    }) {
        return Err("invalid_registry");
    }
    let mut out = json!({"schema_version":1,"connections":normalized,"override_history":history});
    if !disabled.as_array().unwrap().is_empty() {
        let mut a = disabled.as_array().unwrap().clone();
        a.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        out["auto_connect_disabled"] = json!(a);
    }
    Ok(out)
}
