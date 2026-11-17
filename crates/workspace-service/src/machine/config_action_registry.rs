//! Action-specific legacy registry validation and normalization.
use super::*;
use crate::config_read_model::matches;

fn fail(code: &'static str, message: &str) -> Refused {
    refuse(code, message)
}
fn field(v: &Value, key: &str) -> Result<()> {
    if !v[key].as_str().is_some_and(|s| !s.is_empty()) {
        return Err(refuse(
            "invalid_field",
            format!("'{key}' must be a non-empty string."),
        ));
    }
    Ok(())
}
fn shape(v: &Value, required: &[&str], optional: &[&str]) -> bool {
    v.as_object().is_some_and(|m| {
        required.iter().all(|k| m.contains_key(*k))
            && m.keys()
                .all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str()))
    })
}
fn credential(v: &mut Value) -> Result<()> {
    let invalid = || {
        fail("invalid_credential_ref", "'credential_ref.kind' must be one of ('engine_native_dir', 'secret_file', 'os_keyring') with exactly the fields that kind requires; unknown shapes are rejected rather than guessed at.")
    };
    match v["kind"].as_str() {
        Some("engine_native_dir" | "secret_file") => {
            let native = v["kind"] == "engine_native_dir";
            if !shape(v, &["kind", "path"], if native { &["owned"] } else { &[] })
                || !v["path"].as_str().is_some_and(|s| !s.is_empty())
                || native && v.get("owned").is_some_and(|v| !v.is_boolean())
            {
                return Err(invalid());
            }
            if !v["path"].as_str().unwrap().starts_with('/') {
                return Err(fail("invalid_field", "'path' must be an absolute path."));
            }
            if native && v.get("owned").is_none() {
                v["owned"] = json!(true);
            }
        }
        Some("os_keyring") => {
            if !shape(v, &["kind", "service", "account"], &[])
                || ["service", "account"]
                    .iter()
                    .any(|k| !v[k].as_str().is_some_and(|s| !s.is_empty()))
            {
                return Err(invalid());
            }
            for k in ["service", "account"] {
                let s = v[k].as_str().unwrap();
                if !matches(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$", s) || s.contains("..") {
                    return Err(refuse("invalid_field", format!("'{k}' must be a plain identifier (letters/digits/'.'/'_'/'-' only, no path separators).")));
                }
            }
        }
        _ => return Err(invalid()),
    }
    Ok(())
}
fn record(v: &mut Value) -> Result<()> {
    let required = [
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
    if !shape(v, &required, &["max_concurrent_sessions"]) {
        return Err(fail(
            "invalid_connection_record",
            "A connection record has an unexpected or missing field set.",
        ));
    }
    if v.get("max_concurrent_sessions")
        .is_some_and(|n| !n.is_i64() && !n.is_u64())
    {
        return Err(fail(
            "invalid_connection_record",
            "max_concurrent_sessions must be an integer.",
        ));
    }
    if !v["auth_mode"]
        .as_str()
        .is_some_and(|s| ["native_account", "api_key", "gateway"].contains(&s))
        || !v["state"].as_str().is_some_and(|s| {
            [
                "configured",
                "authorization_required",
                "unavailable",
                "invalid",
            ]
            .contains(&s)
        })
    {
        return Err(fail(
            "invalid_connection_record",
            "auth_mode/state must be known enum values.",
        ));
    }
    if !v["launch_command"].is_null()
        && !v["launch_command"]
            .as_array()
            .is_some_and(|a| a.iter().all(Value::is_string))
    {
        return Err(fail(
            "invalid_connection_record",
            "launch_command must be a list of strings or null.",
        ));
    }
    credential(&mut v["credential_ref"])?;
    if !v["id"]
        .as_str()
        .is_some_and(|s| matches(r"^cn_[a-f0-9]{12}$", s))
    {
        return Err(fail("invalid_identifier", "Use an identifier beginning with 'cn_' followed by 12 lowercase hexadecimal characters, exactly as issued; never invent or truncate one."));
    }
    for k in [
        "name",
        "provider",
        "approved_fingerprint",
        "created_at",
        "updated_at",
    ] {
        field(v, k)?;
    }
    let supported = match v["engine"].as_str() {
        Some("codex-acp") => match v["auth_mode"].as_str() {
            Some("api_key") => v["key_kind"] == "openai_api_key",
            _ => v["key_kind"].is_null(),
        },
        Some("claude-agent-acp") => match v["auth_mode"].as_str() {
            Some("api_key") => {
                v["key_kind"] == "anthropic_api_key" || v["key_kind"] == "claude_oauth_token"
            }
            _ => v["key_kind"].is_null(),
        },
        Some("opencode") => v["auth_mode"] == "native_account" && v["key_kind"].is_null(),
        _ => false,
    };
    if !supported {
        return Err(fail("unsupported_auth_mode", "Use one of the documented (engine, auth_mode[, key_kind]) combinations in connections.model.SUPPORT_MATRIX; native account, API key, and gateway modes are distinct and only enabled where the engine documents or statically exposes them."));
    }
    if !v["endpoint"].is_null() {
        field(v, "endpoint")?;
    }
    if v["launch_command"]
        .as_array()
        .is_some_and(|a| a.iter().any(|p| p == ""))
    {
        return Err(fail(
            "invalid_field",
            "'launch_command' must be a tuple of non-empty strings, or None.",
        ));
    }
    if !v["qualified"].is_boolean() {
        return Err(fail(
            "invalid_field",
            "'qualified' must be an explicit bool.",
        ));
    }
    if v.get("max_concurrent_sessions").is_none() {
        v["max_concurrent_sessions"] = json!(4);
    }
    if !v["max_concurrent_sessions"]
        .as_u64()
        .is_some_and(|n| (1..=64).contains(&n))
    {
        return Err(fail(
            "invalid_field",
            "'max_concurrent_sessions' must be an integer from 1 through 64.",
        ));
    }
    Ok(())
}
pub(in crate::machine) fn normalize(v: &Value) -> Result<Value> {
    if !v.as_object().is_some_and(|m| {
        m.keys().all(|k| {
            [
                "schema_version",
                "connections",
                "override_history",
                "auto_connect_disabled",
            ]
            .contains(&k.as_str())
        })
    }) {
        return Err(fail("invalid_registry", "connections.json must only contain keys from ['auto_connect_disabled', 'connections', 'override_history', 'schema_version']."));
    }
    if v["schema_version"] != 1
        && v["schema_version"] != true
        && v["schema_version"].as_f64() != Some(1.0)
    {
        return Err(fail(
            "invalid_registry",
            "connections.json schema_version must be 1.",
        ));
    }
    let mut connections = v.get("connections").cloned().unwrap_or(json!({}));
    let Some(records) = connections.as_object_mut() else {
        return Err(fail(
            "invalid_registry",
            "connections.json 'connections' must be an object.",
        ));
    };
    for r in records.values_mut() {
        record(r)?;
    }
    for (id, r) in records {
        if r["id"] != *id {
            return Err(fail(
                "invalid_registry",
                "connection record id must match its registry key.",
            ));
        }
    }
    let history = v.get("override_history").cloned().unwrap_or(json!({}));
    let Some(entries) = history.as_object() else {
        return Err(fail(
            "invalid_registry",
            "connections.json 'override_history' must be an object.",
        ));
    };
    for entries in entries.values() {
        let Some(entries) = entries.as_array() else {
            return Err(fail(
                "invalid_registry",
                "override_history entries must be a list.",
            ));
        };
        for entry in entries {
            if !shape(
                entry,
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
                &[],
            ) {
                return Err(fail(
                    "invalid_override_history",
                    "An override-history entry has an unexpected or missing field set.",
                ));
            }
        }
    }
    let disabled = v.get("auto_connect_disabled").cloned().unwrap_or(json!([]));
    if !disabled.as_array().is_some_and(|a| {
        a.iter().all(|v| {
            v.as_str()
                .is_some_and(|s| ["codex-acp", "claude-agent-acp", "opencode"].contains(&s))
        }) && a
            .iter()
            .map(Value::to_string)
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            == a.len()
    }) {
        return Err(fail(
            "invalid_registry",
            "auto_connect_disabled must list distinct supported engines.",
        ));
    }
    let mut result =
        json!({"schema_version":1,"connections":connections,"override_history":history});
    if let Some(a) = disabled.as_array().filter(|a| !a.is_empty()) {
        let mut a = a.clone();
        a.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
        result["auto_connect_disabled"] = json!(a);
    }
    Ok(result)
}
