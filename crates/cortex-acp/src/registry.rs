//! The owner connection registry exactly as Python `connections.registry`
//! reads and writes it (`_parse_registry_bytes`, `_record_from_dict`,
//! `ConnectionRecord.__post_init__`, `RegistryState.to_json_bytes`; the oracle).
//!
//! It accepts every shape Python accepts, including Python-only JSON in the
//! opaque `override_history` (floats, `NaN`, integers beyond 64 bits) and
//! `schema_version` values equal to 1 in Python (`true`, `1.0`), and refuses
//! everything Python refuses. Records keep Python's dict (file) order for
//! iteration and are re-serialized in Python's normalized sorted compact form.
use crate::{
    adapters::python_json,
    pyjson::{self, Py},
};
use cortex_core::{CoreError, CoreResult};
use serde_json::{json, Value};

pub(crate) const ENGINES: [&str; 3] = ["codex-acp", "claude-agent-acp", "opencode"];
const ROOT_KEYS: [&str; 4] = [
    "schema_version",
    "connections",
    "override_history",
    "auto_connect_disabled",
];
const RECORD_KEYS: [&str; 14] = [
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
const HISTORY_KEYS: [&str; 8] = [
    "fingerprint",
    "previous_fingerprint",
    "engine",
    "provider",
    "endpoint",
    "launch_command",
    "approved_at",
    "approved_by_user",
];
const STATES: [&str; 4] = [
    "configured",
    "authorization_required",
    "unavailable",
    "invalid",
];

fn invalid() -> CoreError {
    CoreError::Refused("invalid_registry")
}

/// Validated registry contents.
pub(crate) struct Registry {
    /// Normalized records (Python `_record_to_dict`) in Python dict order.
    connections: Vec<(String, Value)>,
    /// The opaque `override_history` object, kept with Python's JSON values.
    history: Py,
    disabled: Vec<String>,
    /// False when owner strings hold a lone surrogate: Python can read but
    /// never write such a registry (`UnicodeEncodeError`).
    encodable: bool,
}

fn keys(entries: &[(String, Py)]) -> impl Iterator<Item = &str> {
    entries.iter().map(|(k, _)| k.as_str())
}
fn nonempty(value: Option<&Py>) -> Option<&str> {
    value.and_then(Py::as_str).filter(|s| !s.is_empty())
}
/// Python `_KEYRING_IDENTIFIER.match`: `$` also matches before one final newline.
fn keyring_identifier(text: &str) -> bool {
    let body = text.strip_suffix('\n').unwrap_or(text);
    !text.contains('/')
        && !text.contains("..")
        && regex::Regex::new(r"\A[A-Za-z0-9][A-Za-z0-9._-]{0,127}\z")
            .unwrap()
            .is_match(body)
}
/// Python `deserialize_credential_ref` plus each reference's `__post_init__`.
fn credential_ref(value: &Py) -> Option<Value> {
    let Py::Object(entries) = value else {
        return None;
    };
    let exact = |expected: &[&str]| {
        entries.len() == expected.len() && keys(entries).all(|k| expected.contains(&k))
    };
    let absolute = |key: &str| nonempty(value.get(key)).filter(|p| p.starts_with('/'));
    match value.get("kind").and_then(Py::as_str) {
        Some("engine_native_dir") => {
            let path = absolute("path")?;
            if keys(entries).any(|k| !["kind", "path", "owned"].contains(&k)) {
                return None;
            }
            let owned = match value.get("owned") {
                None => true,
                Some(Py::Bool(owned)) => *owned,
                Some(_) => return None,
            };
            Some(json!({"kind": "engine_native_dir", "path": path, "owned": owned}))
        }
        Some("secret_file") => {
            let path = absolute("path")?;
            exact(&["kind", "path"]).then(|| json!({"kind": "secret_file", "path": path}))
        }
        Some("os_keyring") => {
            let service = nonempty(value.get("service"))?;
            let account = nonempty(value.get("account"))?;
            (exact(&["kind", "service", "account"])
                && keyring_identifier(service)
                && keyring_identifier(account))
            .then(|| json!({"kind": "os_keyring", "service": service, "account": account}))
        }
        _ => None,
    }
}
fn optional_string(value: &Py) -> Option<Value> {
    match value {
        Py::Null => Some(Value::Null),
        Py::Str(s) if !s.is_empty() => Some(json!(s)),
        _ => None,
    }
}
/// Python `_record_from_dict` and `ConnectionRecord.__post_init__`.
fn record(value: &Py) -> Option<Value> {
    let Py::Object(entries) = value else {
        return None;
    };
    if RECORD_KEYS
        .iter()
        .any(|k| !keys(entries).any(|present| present == *k))
        || keys(entries).any(|k| !RECORD_KEYS.contains(&k) && k != "max_concurrent_sessions")
    {
        return None;
    }
    let field = |key: &str| value.get(key).unwrap();
    let limit = match value.get("max_concurrent_sessions") {
        None => 4,
        Some(Py::Int(text)) => text.parse::<u8>().ok().filter(|n| (1..=64).contains(n))?,
        Some(_) => return None,
    };
    let auth = field("auth_mode")
        .as_str()
        .filter(|a| ["native_account", "api_key", "gateway"].contains(a))?;
    let state = field("state").as_str().filter(|s| STATES.contains(s))?;
    let launch = match field("launch_command") {
        Py::Null => Value::Null,
        Py::Array(parts) => Value::Array(
            parts
                .iter()
                .map(|p| nonempty(Some(p)).map(|s| json!(s)))
                .collect::<Option<_>>()?,
        ),
        _ => return None,
    };
    let id = field("id").as_str().filter(|id| {
        id.strip_prefix("cn_").is_some_and(|s| {
            s.len() == 12 && s.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
        })
    })?;
    let [name, provider, fingerprint, created, updated] = [
        "name",
        "provider",
        "approved_fingerprint",
        "created_at",
        "updated_at",
    ]
    .map(|k| nonempty(Some(field(k))));
    let engine = field("engine").as_str().filter(|e| ENGINES.contains(e))?;
    // Python `validate_auth_selection` over the support matrix.
    let key_kind = field("key_kind");
    let supported = match (engine, auth) {
        ("opencode", "native_account") => true,
        ("opencode", _) => false,
        (_, "api_key") => {
            let kinds: &[&str] = if engine == "codex-acp" {
                &["openai_api_key"]
            } else {
                &["anthropic_api_key", "claude_oauth_token"]
            };
            key_kind.as_str().is_some_and(|k| kinds.contains(&k))
        }
        _ => matches!(key_kind, Py::Null),
    };
    let qualified = match field("qualified") {
        Py::Bool(qualified) => *qualified,
        _ => return None,
    };
    if !supported {
        return None;
    }
    Some(json!({
        "id": id, "name": name?, "engine": engine, "provider": provider?,
        "auth_mode": auth, "key_kind": optional_string(key_kind)?,
        "endpoint": optional_string(field("endpoint"))?, "launch_command": launch,
        "credential_ref": credential_ref(field("credential_ref"))?,
        "approved_fingerprint": fingerprint?, "state": state, "qualified": qualified,
        "created_at": created?, "updated_at": updated?, "max_concurrent_sessions": limit,
    }))
}

impl Registry {
    /// Python `_parse_registry_bytes`; absent or empty bytes are an empty registry.
    pub(crate) fn parse(bytes: Option<&[u8]>) -> CoreResult<Self> {
        let mut registry = Self {
            connections: vec![],
            history: Py::Object(vec![]),
            disabled: vec![],
            encodable: true,
        };
        let Some(bytes) = bytes.filter(|b| !b.is_empty()) else {
            return Ok(registry);
        };
        let document = pyjson::loads(bytes).ok_or_else(invalid)?;
        registry.encodable = document.encodable;
        let mut root = document.value;
        let Py::Object(entries) = &mut root else {
            return Err(invalid());
        };
        if keys(entries).any(|k| !ROOT_KEYS.contains(&k)) {
            return Err(invalid());
        }
        let mut take = |key: &str| {
            entries
                .iter_mut()
                .find(|(k, _)| k == key)
                .map(|(_, v)| std::mem::replace(v, Py::Null))
        };
        if !take("schema_version").is_some_and(|v| v.equals_one()) {
            return Err(invalid());
        }
        if let Some(records) = take("connections") {
            let Py::Object(records) = &records else {
                return Err(invalid());
            };
            for (id, value) in records {
                let record =
                    record(value).ok_or(CoreError::Refused("invalid_connection_record"))?;
                registry.connections.push((id.clone(), record));
            }
        }
        if registry
            .connections
            .iter()
            .any(|(id, record)| record["id"] != *id)
        {
            return Err(invalid());
        }
        if let Some(history) = take("override_history") {
            let Py::Object(lists) = &history else {
                return Err(invalid());
            };
            for (_, list) in lists {
                let Py::Array(items) = list else {
                    return Err(invalid());
                };
                for item in items {
                    let valid = match item {
                        Py::Object(fields) => {
                            fields.len() == HISTORY_KEYS.len()
                                && keys(fields).all(|k| HISTORY_KEYS.contains(&k))
                        }
                        _ => false,
                    };
                    if !valid {
                        return Err(CoreError::Refused("invalid_override_history"));
                    }
                }
            }
            registry.history = history;
        }
        if let Some(disabled) = take("auto_connect_disabled") {
            let Py::Array(items) = &disabled else {
                return Err(invalid());
            };
            for item in items {
                match item.as_str() {
                    Some(engine)
                        if ENGINES.contains(&engine)
                            && !registry.disabled.iter().any(|e| e == engine) =>
                    {
                        registry.disabled.push(engine.to_owned())
                    }
                    _ => return Err(invalid()),
                }
            }
            registry.disabled.sort();
        }
        Ok(registry)
    }
    pub(crate) fn ids(&self) -> Vec<String> {
        self.connections.iter().map(|(id, _)| id.clone()).collect()
    }
    pub(crate) fn get(&self, id: &str) -> Option<&Value> {
        self.connections
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, r)| r)
    }
    pub(crate) fn get_mut(&mut self, id: &str) -> Option<&mut Value> {
        self.connections
            .iter_mut()
            .find(|(k, _)| k == id)
            .map(|(_, r)| r)
    }
    pub(crate) fn insert(&mut self, id: String, record: Value) {
        match self.get_mut(&id) {
            Some(existing) => *existing = record,
            None => self.connections.push((id, record)),
        }
    }
    /// Python `next(r for r in state.connections.values() if r.engine == engine)`.
    pub(crate) fn first_of(&self, engine: &str) -> Option<String> {
        self.connections
            .iter()
            .find(|(_, r)| r["engine"] == engine)
            .map(|(id, _)| id.clone())
    }
    pub(crate) fn opted_out(&self, engine: &str) -> bool {
        self.disabled.iter().any(|e| e == engine)
    }
    /// Python `RegistryState.to_json_bytes`. Unencodable owner data refuses
    /// the write like Python's `UnicodeEncodeError` (not a portable refusal).
    pub(crate) fn to_json(&self) -> CoreResult<String> {
        if !self.encodable {
            return Err(CoreError::Io(std::io::Error::other(
                "registry_not_utf8_encodable",
            )));
        }
        let mut out = String::from("{");
        if !self.disabled.is_empty() {
            out.push_str("\"auto_connect_disabled\":");
            python_json(&json!(self.disabled), &mut out)?;
            out.push(',');
        }
        out.push_str("\"connections\":{");
        let mut sorted: Vec<&(String, Value)> = self.connections.iter().collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        for (index, (id, record)) in sorted.into_iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            out.push_str(&pyjson::quote(id));
            out.push(':');
            python_json(record, &mut out)?;
        }
        out.push_str("},\"override_history\":");
        pyjson::dumps(&self.history, &mut out);
        out.push_str(",\"schema_version\":1}");
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECORD: &str = r#"{"id":"cn_0123456789ab","name":"codex","engine":"codex-acp","provider":"openai","auth_mode":"api_key","key_kind":"openai_api_key","endpoint":null,"launch_command":["/n","/e"],"credential_ref":{"kind":"secret_file","path":"/s"},"approved_fingerprint":"f","state":"configured","qualified":true,"created_at":"t","updated_at":"t"}"#;

    fn with(record: &str, rest: &str) -> String {
        format!(r#"{{"schema_version":1,"connections":{{"cn_0123456789ab":{record}}}{rest}}}"#)
    }

    #[test]
    fn python_only_values_survive_a_rewrite() {
        let history = r#","override_history":{"cn_0123456789ab":[{"fingerprint":1e16,"previous_fingerprint":NaN,"engine":-0.0,"provider":123456789012345678901234567890,"endpoint":null,"launch_command":[1.5,2],"approved_at":"x","approved_by_user":true}]}"#;
        let text = with(RECORD, history).replace("\"schema_version\":1", "\"schema_version\":true");
        let registry = Registry::parse(Some(text.as_bytes())).unwrap();
        let out = registry.to_json().unwrap();
        assert!(out.ends_with(r#""override_history":{"cn_0123456789ab":[{"approved_at":"x","approved_by_user":true,"endpoint":null,"engine":-0.0,"fingerprint":1e+16,"launch_command":[1.5,2],"previous_fingerprint":NaN,"provider":123456789012345678901234567890}]},"schema_version":1}"#), "{out}");
        assert!(out.contains(r#""credential_ref":{"kind":"secret_file","path":"/s"}"#));
        assert!(out.contains(r#""max_concurrent_sessions":4"#));
        assert!(Registry::parse(Some(with(RECORD, "").replace(":1,", ":1.0,").as_bytes())).is_ok());
    }

    #[test]
    fn keyring_identifiers_follow_python_match() {
        assert!(keyring_identifier("acct\n"));
        assert!(keyring_identifier("a.b_c-1"));
        for bad in ["acct\n\n", ".acct", "a/b", "a..b", "", "\nacct"] {
            assert!(!keyring_identifier(bad), "{bad:?}");
        }
    }

    #[test]
    fn invalid_registries_still_refuse() {
        for (from, to) in [
            ("\"schema_version\":1", "\"schema_version\":2"),
            ("\"schema_version\":1", "\"schema_version\":\"1\""),
            ("\"cn_0123456789ab\":{", "\"cn_0123456789ac\":{"),
            ("\"qualified\":true", "\"qualified\":1"),
            (
                "\"updated_at\":\"t\"",
                "\"updated_at\":\"t\",\"max_concurrent_sessions\":4.0",
            ),
            (
                "\"updated_at\":\"t\"",
                "\"updated_at\":\"t\",\"max_concurrent_sessions\":true",
            ),
            (
                "\"updated_at\":\"t\"",
                "\"updated_at\":\"t\",\"max_concurrent_sessions\":65",
            ),
            ("\"updated_at\":\"t\"", "\"updated_at\":\"t\",\"extra\":1"),
            ("\"key_kind\":\"openai_api_key\"", "\"key_kind\":null"),
            ("\"name\":\"codex\"", "\"name\":\"\""),
            ("[\"/n\",\"/e\"]", "[\"/n\",\"\"]"),
            ("\"path\":\"/s\"", "\"path\":\"s\""),
            ("\"engine\":\"codex-acp\"", "\"engine\":[\"codex-acp\"]"),
        ] {
            let text = with(RECORD, "").replace(from, to);
            assert!(Registry::parse(Some(text.as_bytes())).is_err(), "{to}");
        }
        for rest in [
            r#","override_history":{"x":[{}]}"#,
            r#","override_history":{"x":{}}"#,
            r#","auto_connect_disabled":["codex-acp","codex-acp"]"#,
            r#","auto_connect_disabled":["other"]"#,
            r#","extra":1"#,
        ] {
            assert!(
                Registry::parse(Some(with(RECORD, rest).as_bytes())).is_err(),
                "{rest}"
            );
        }
    }

    #[test]
    fn lone_surrogates_read_but_never_write() {
        let text = with(&RECORD.replace("\"codex\"", "\"\\ud800\""), "");
        let registry = Registry::parse(Some(text.as_bytes())).unwrap();
        assert_eq!(registry.ids(), ["cn_0123456789ab"]);
        assert!(matches!(registry.to_json(), Err(CoreError::Io(_))));
    }
}
