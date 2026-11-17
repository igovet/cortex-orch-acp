//! Metadata-only compatibility validation for project editor reads.
//! Never opens a credential reference, catalog snapshot, engine or keyring.
use super::native::*;
use serde_json::Value;
fn err(code: &'static str, message: &str) -> Refused {
    refuse(code, message)
}
fn text<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v[k].as_str().filter(|s| !s.is_empty()).ok_or_else(|| {
        refuse(
            "invalid_field",
            format!("'{k}' must be a non-empty string."),
        )
    })
}
fn keys(v: &Value, required: &[&str], optional: &[&str]) -> bool {
    v.as_object().is_some_and(|m| {
        required.iter().all(|k| m.contains_key(*k))
            && m.keys()
                .all(|k| required.contains(&k.as_str()) || optional.contains(&k.as_str()))
    })
}
fn credential(v: &Value) -> Result<()> {
    let invalid = || {
        err("invalid_credential_ref","'credential_ref.kind' must be one of ('engine_native_dir', 'secret_file', 'os_keyring') with exactly the fields that kind requires; unknown shapes are rejected rather than guessed at.")
    };
    match v["kind"].as_str() {
        Some("engine_native_dir" | "secret_file") => {
            let optional = if v["kind"] == "engine_native_dir" {
                &["owned"][..]
            } else {
                &[][..]
            };
            if !keys(v, &["kind", "path"], optional)
                || !v["path"].as_str().is_some_and(|s| !s.is_empty())
                || v.get("owned").is_some_and(|v| !v.is_boolean())
            {
                return Err(invalid());
            }
            if !text(v, "path")?.starts_with('/') {
                return Err(err("invalid_field", "'path' must be an absolute path."));
            }
        }
        Some("os_keyring") => {
            if !keys(v, &["kind", "service", "account"], &[])
                || !v["service"].as_str().is_some_and(|s| !s.is_empty())
                || !v["account"].as_str().is_some_and(|s| !s.is_empty())
            {
                return Err(invalid());
            }
            for k in ["service", "account"] {
                let s = text(v, k)?;
                if s.len() > 128
                    || s.contains("..")
                    || !s.as_bytes()[0].is_ascii_alphanumeric()
                    || !s
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"._-".contains(&c))
                {
                    return Err(refuse("invalid_field",format!("'{k}' must be a plain identifier (letters/digits/'.'/'_'/'-' only, no path separators).")));
                }
            }
        }
        _ => return Err(invalid()),
    }
    Ok(())
}
fn record(v: &Value) -> Result<()> {
    if !keys(
        v,
        &[
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
        ],
        &["max_concurrent_sessions"],
    ) {
        return Err(err(
            "invalid_connection_record",
            "A connection record has an unexpected or missing field set.",
        ));
    }
    if v.get("max_concurrent_sessions")
        .is_some_and(|v| !v.is_i64() && !v.is_u64())
    {
        return Err(err(
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
        return Err(err(
            "invalid_connection_record",
            "auth_mode/state must be known enum values.",
        ));
    }
    if !v["launch_command"].is_null()
        && !v["launch_command"]
            .as_array()
            .is_some_and(|a| a.iter().all(Value::is_string))
    {
        return Err(err(
            "invalid_connection_record",
            "launch_command must be a list of strings or null.",
        ));
    }
    credential(&v["credential_ref"])?;
    if !v["id"].as_str().is_some_and(|s| {
        s.strip_prefix("cn_").is_some_and(|s| {
            s.len() == 12
                && s.bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
    }) {
        return Err(err("invalid_identifier","Use an identifier beginning with 'cn_' followed by 12 lowercase hexadecimal characters, exactly as issued; never invent or truncate one."));
    }
    for k in [
        "name",
        "provider",
        "approved_fingerprint",
        "created_at",
        "updated_at",
    ] {
        text(v, k)?;
    }
    let engine = v["engine"].as_str().unwrap_or("");
    let auth = v["auth_mode"].as_str().unwrap();
    let supported = match (engine, auth) {
        ("codex-acp" | "claude-agent-acp", "native_account" | "gateway")
        | ("opencode", "native_account") => v["key_kind"].is_null(),
        ("codex-acp", "api_key") => v["key_kind"] == "openai_api_key",
        ("claude-agent-acp", "api_key") => {
            v["key_kind"] == "anthropic_api_key" || v["key_kind"] == "claude_oauth_token"
        }
        _ => false,
    };
    if !supported {
        return Err(err("unsupported_auth_mode","Use one of the documented (engine, auth_mode[, key_kind]) combinations in connections.model.SUPPORT_MATRIX; native account, API key, and gateway modes are distinct and only enabled where the engine documents or statically exposes them."));
    }
    if !v["endpoint"].is_null() {
        text(v, "endpoint")?;
    }
    if v["launch_command"]
        .as_array()
        .is_some_and(|a| a.iter().any(|v| v == ""))
    {
        return Err(err(
            "invalid_field",
            "'launch_command' must be a tuple of non-empty strings, or None.",
        ));
    }
    if !v["qualified"].is_boolean() {
        return Err(err(
            "invalid_field",
            "'qualified' must be an explicit bool.",
        ));
    }
    if v.get("max_concurrent_sessions")
        .is_some_and(|v| !v.as_u64().is_some_and(|n| (1..=64).contains(&n)))
    {
        return Err(err(
            "invalid_field",
            "'max_concurrent_sessions' must be an integer from 1 through 64.",
        ));
    }
    Ok(())
}
pub(super) fn validate(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() {
        return Ok(());
    }
    let v: Value = serde_json::from_slice(bytes).map_err(|_| {
        err(
            "invalid_registry",
            "connections.json is not valid UTF-8 JSON.",
        )
    })?;
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
        return Err(err("invalid_registry","connections.json must only contain keys from ['auto_connect_disabled', 'connections', 'override_history', 'schema_version']."));
    }
    if v["schema_version"] != 1 {
        return Err(err(
            "invalid_registry",
            "connections.json schema_version must be 1.",
        ));
    }
    if let Some(cs) = v.get("connections") {
        let Some(cs) = cs.as_object() else {
            return Err(err(
                "invalid_registry",
                "connections.json 'connections' must be an object.",
            ));
        };
        for r in cs.values() {
            record(r)?;
        }
        if cs.iter().any(|(k, r)| r["id"] != *k) {
            return Err(err(
                "invalid_registry",
                "connection record id must match its registry key.",
            ));
        }
    }
    if let Some(h) = v.get("override_history") {
        let Some(h) = h.as_object() else {
            return Err(err(
                "invalid_registry",
                "connections.json 'override_history' must be an object.",
            ));
        };
        for entries in h.values() {
            let Some(entries) = entries.as_array() else {
                return Err(err(
                    "invalid_registry",
                    "override_history entries must be a list.",
                ));
            };
            if entries.iter().any(|v| {
                !keys(
                    v,
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
                )
            }) {
                return Err(err(
                    "invalid_override_history",
                    "An override-history entry has an unexpected or missing field set.",
                ));
            }
        }
    }
    if v.get("auto_connect_disabled").is_some_and(|v| {
        !v.as_array().is_some_and(|a| {
            a.iter().all(|v| {
                v.as_str()
                    .is_some_and(|s| super::project_routing::ENGINES.contains(&s))
            }) && a
                .iter()
                .map(Value::to_string)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == a.len()
        })
    }) {
        return Err(err(
            "invalid_registry",
            "auto_connect_disabled must list distinct supported engines.",
        ));
    }
    Ok(())
}
