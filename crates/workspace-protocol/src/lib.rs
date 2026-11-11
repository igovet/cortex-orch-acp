//! Validated Workspace serde newtypes, negotiation and bounded local frames.
//! Structural validation does not authorize or dispatch an operation.
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::sync::OnceLock;

pub mod channel;
pub mod directories;
pub mod terminal;
pub mod voice;
mod generated;
pub use generated::*;

fn schema() -> &'static Value {
    static SCHEMA: OnceLock<Value> = OnceLock::new();
    SCHEMA.get_or_init(|| {
        serde_json::from_str(include_str!(
            "../../../src/cortex_runtime/workspace_contract/schema.json"
        ))
        .expect("checked canonical schema")
    })
}
fn definition(name: &str) -> Result<&'static Value, String> {
    schema()["$defs"]
        .get(name)
        .ok_or_else(|| "unknown_type".into())
}
fn error(path: &str, code: &str) -> String {
    format!("{path}:{code}")
}
fn pattern_matches(pattern: &str, value: &str) -> bool {
    let decimal = !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit());
    match pattern {
        "^(0|[1-9][0-9]*)$" => decimal && (value == "0" || !value.starts_with('0')),
        "^[1-9][0-9]*$" => decimal && !value.starts_with('0'),
        "^[0-9a-f]{64}$" => {
            value.len() == 64
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        _ => false,
    }
}
fn check(spec: &Value, value: &Value, path: &str, depth: usize) -> Result<(), String> {
    if depth > 64 {
        return Err("nesting_limit".into());
    }
    if let Some(reference) = spec["$ref"].as_str() {
        return check(
            definition(reference.rsplit('/').next().unwrap())?,
            value,
            path,
            depth + 1,
        );
    }
    for union in ["oneOf", "anyOf"] {
        if let Some(branches) = spec[union].as_array() {
            let matches = branches
                .iter()
                .filter(|branch| check(branch, value, path, depth + 1).is_ok())
                .count();
            return if matches > 0 && (union != "oneOf" || matches == 1) {
                Ok(())
            } else {
                Err(error(path, "union"))
            };
        }
    }
    if let Some(constant) = spec.get("const") {
        let equal = if constant.is_number() && value.is_number() {
            constant.as_f64() == value.as_f64()
        } else {
            constant == value
        };
        if !equal {
            return Err(error(path, "const"));
        }
    }
    if let Some(choices) = spec["enum"].as_array() {
        if !choices.contains(value) {
            return Err(error(path, "enum"));
        }
    }
    match spec["type"].as_str() {
        Some("object") => {
            let object = value.as_object().ok_or_else(|| error(path, "type"))?;
            if let Some(required) = spec["required"].as_array() {
                if required
                    .iter()
                    .any(|key| !object.contains_key(key.as_str().unwrap()))
                {
                    return Err(error(path, "required"));
                }
            }
            for (key, item) in object {
                if let Some(property) = spec["properties"].get(key) {
                    check(property, item, &format!("{path}.{key}"), depth + 1)?;
                } else if spec["additionalProperties"] == false {
                    return Err(error(path, "unknown_field"));
                }
            }
        }
        Some("array") => {
            let array = value.as_array().ok_or_else(|| error(path, "type"))?;
            if array.len() < spec["minItems"].as_u64().unwrap_or(0) as usize
                || array.len() > spec["maxItems"].as_u64().unwrap_or(u64::MAX) as usize
            {
                return Err(error(path, "items"));
            }
            if spec["uniqueItems"] == true {
                let unique: BTreeSet<_> = array.iter().map(Value::to_string).collect();
                if unique.len() != array.len() {
                    return Err(error(path, "unique"));
                }
            }
            for item in array {
                check(&spec["items"], item, &format!("{path}[]"), depth + 1)?;
            }
        }
        Some("string") => {
            let text = value.as_str().ok_or_else(|| error(path, "type"))?;
            let length = text.chars().count() as u64;
            if length < spec["minLength"].as_u64().unwrap_or(0)
                || length > spec["maxLength"].as_u64().unwrap_or(u64::MAX)
            {
                return Err(error(path, "length"));
            }
            if let Some(pattern) = spec["pattern"].as_str() {
                if !pattern_matches(pattern, text) {
                    return Err(error(path, "pattern"));
                }
            }
        }
        Some("integer") => {
            let number = value.as_f64().ok_or_else(|| error(path, "type"))?;
            if !number.is_finite() || number.fract() != 0.0 {
                return Err(error(path, "type"));
            }
            if number < spec["minimum"].as_f64().unwrap_or(f64::NEG_INFINITY)
                || number > spec["maximum"].as_f64().unwrap_or(f64::INFINITY)
            {
                return Err(error(path, "range"));
            }
        }
        Some("null") if !value.is_null() => return Err(error(path, "type")),
        Some("boolean") if !value.is_boolean() => return Err(error(path, "type")),
        _ => {}
    }
    Ok(())
}
fn json_depth(value: &Value, depth: usize) -> Result<(), String> {
    if depth > 64 {
        return Err("nesting_limit".into());
    }
    match value {
        Value::Object(object) => {
            for item in object.values() {
                json_depth(item, depth + 1)?;
            }
        }
        Value::Array(array) => {
            for item in array {
                json_depth(item, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}
/// Validate against the canonical subset; unknown response fields are retained.
pub fn validate(type_name: &str, value: &Value) -> Result<(), String> {
    let spec = definition(type_name)?;
    json_depth(value, 0)?;
    let size = serde_json::to_vec(value).map_err(|_| "invalid_json")?.len();
    if size > FRAME_BYTES {
        return Err("frame_too_large".into());
    }
    if spec["x-page"] == true && size > PAGE_CONTENT_BYTES {
        return Err("page_too_large".into());
    }
    check(spec, value, "$", 0)
}
fn normalize(spec: &Value, value: &Value, warnings: &mut Vec<String>, path: &str) -> Value {
    if let Some(reference) = spec["$ref"].as_str() {
        return normalize(
            definition(reference.rsplit('/').next().unwrap()).unwrap(),
            value,
            warnings,
            path,
        );
    }
    for union in ["oneOf", "anyOf"] {
        if let Some(branches) = spec[union].as_array() {
            if let Some(branch) = branches
                .iter()
                .find(|branch| check(branch, value, path, 0).is_ok())
            {
                return normalize(branch, value, warnings, path);
            }
        }
    }
    if let Some(known) = spec["x-known-values"].as_array() {
        if !known.contains(value) {
            warnings.push(error(path, "unknown_enum"));
            return json!("unknown");
        }
    }
    if spec["type"] == "object" {
        return Value::Object(
            value
                .as_object()
                .unwrap()
                .iter()
                .map(|(key, item)| {
                    let decoded = match spec["properties"].get(key) {
                        Some(property) => {
                            normalize(property, item, warnings, &format!("{path}.{key}"))
                        }
                        None => item.clone(),
                    };
                    (key.clone(), decoded)
                })
                .collect(),
        );
    }
    if spec["type"] == "array" {
        return Value::Array(
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|item| normalize(&spec["items"], item, warnings, &format!("{path}[]")))
                .collect(),
        );
    }
    value.clone()
}
/// Clients must use this decoder for responses, not infer success from raw strings.
pub fn decode_response(type_name: &str, value: &Value) -> Result<(Value, Vec<String>), String> {
    validate(type_name, value)?;
    let mut warnings = Vec::new();
    let decoded = normalize(definition(type_name)?, value, &mut warnings, "$");
    Ok((decoded, warnings))
}
pub fn capabilities(
    build_identity: &str,
    generation: &str,
    operations: &[&str],
) -> Result<Value, String> {
    let supported = schema()["x-operations"].as_array().unwrap();
    if operations.iter().any(|op| !supported.contains(&json!(op))) {
        return Err("unsupported_operation".into());
    }
    let operations: BTreeSet<_> = operations.iter().copied().collect();
    let value = json!({"protocol":{"major":WIRE_MAJOR,"minor":WIRE_MINOR},
        "buildIdentity":build_identity,"generation":generation,"schemaFingerprint":SCHEMA_FINGERPRINT,
        "mappingVersion":schema()["x-mapping-version"],"operations":operations,
        "limits":schema()["x-limits"],"acpCapabilities":{}});
    validate("Capabilities", &value)?;
    Ok(value)
}
pub fn negotiate(local: &Value, remote: &Value) -> Result<Value, String> {
    validate("Capabilities", local)?;
    validate("Capabilities", remote)?;
    if local["protocol"]["major"].as_f64() != remote["protocol"]["major"].as_f64() {
        return Err("version_mismatch".into());
    }
    let operations: BTreeSet<_> = local["operations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|op| remote["operations"].as_array().unwrap().contains(op))
        .cloned()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    let mut limits = serde_json::Map::new();
    for (key, number) in local["limits"].as_object().unwrap() {
        if let Some(peer) = remote["limits"][key].as_f64() {
            limits.insert(
                key.clone(),
                json!((number.as_f64().unwrap() as u64).min(peer as u64)),
            );
        }
    }
    Ok(json!({"protocol":{"major":local["protocol"]["major"],
        "minor":(local["protocol"]["minor"].as_f64().unwrap() as u64).min(remote["protocol"]["minor"].as_f64().unwrap() as u64)},
        "operations":operations, "limits":limits,
        "mappingVersion":if local["mappingVersion"] == remote["mappingVersion"] {local["mappingVersion"].clone()} else {Value::Null}}))
}
pub fn encode_frame(value: &Value) -> Result<Vec<u8>, String> {
    json_depth(value, 0)?;
    let data = serde_json::to_vec(value).map_err(|_| "invalid_json")?;
    if data.is_empty() || data.len() > FRAME_BYTES {
        return Err("frame_too_large".into());
    }
    let mut frame = (data.len() as u32).to_be_bytes().to_vec();
    frame.extend(data);
    Ok(frame)
}
pub fn decode_frame(frame: &[u8]) -> Result<Value, String> {
    if frame.len() < 4 {
        return Err("incomplete_frame".into());
    }
    let size = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
    if size == 0 || size > FRAME_BYTES {
        return Err("frame_too_large".into());
    }
    if size != frame.len() - 4 {
        return Err("incomplete_frame".into());
    }
    let value = serde_json::from_slice(&frame[4..]).map_err(|_| "invalid_json")?;
    json_depth(&value, 0)?;
    Ok(value)
}
