//! Advertised contracts are data, not profile prose. Snapshot of the Python oracle.
//! Sources: src/cortex_runtime/channeled_tools.py:25-27,
//! delegation/mcp_tools.py:755-797 and tool_read_api.py:66-96.
use crate::{Error, Result};
use serde_json::Value;
use std::sync::OnceLock;
fn snapshot() -> &'static Value {
    static DATA: OnceLock<Value> = OnceLock::new();
    DATA.get_or_init(|| {
        serde_json::from_str(include_str!("../schema/tools.json")).expect("embedded schema")
    })
}
/// Unknown roles fail closed. Non-review stages share execution contracts.
pub fn catalogue(role: &str, profile: Option<&str>, stage: Option<&str>) -> Result<Vec<Value>> {
    let key = if role == "coordinator" {
        "coordinator||".to_owned()
    } else if role == "worker" {
        let profile = profile.ok_or(Error::Forbidden)?;
        format!(
            "worker|{}|{}",
            profile,
            if stage == Some("review") {
                "review"
            } else {
                "execution"
            }
        )
    } else {
        return Err(Error::Forbidden);
    };
    let ids = snapshot()["catalogues"][&key]
        .as_array()
        .ok_or(Error::Forbidden)?;
    let mut tools = ids
        .iter()
        .map(|id| snapshot()["tools"][id.as_str().unwrap()].clone())
        .collect::<Vec<_>>();
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(tools)
}
pub fn validate(schema: &Value, v: &Value) -> Result<()> {
    fn valid(s: &Value, v: &Value, depth: usize) -> bool {
        if depth > 64 {
            return false;
        }
        if let Some(t) = s.get("type") {
            let matches_type = |t: &Value| match t.as_str() {
                Some("object") => v.is_object(),
                Some("array") => v.is_array(),
                Some("string") => v.is_string(),
                Some("integer") => v.as_i64().is_some() || v.as_u64().is_some(),
                Some("number") => v.is_number(),
                Some("boolean") => v.is_boolean(),
                Some("null") => v.is_null(),
                _ => false,
            };
            let ok = t
                .as_array()
                .map_or_else(|| matches_type(t), |types| types.iter().any(matches_type));
            if !ok {
                return false;
            }
        }
        if let Some(a) = s["enum"].as_array() {
            if !a.contains(v) {
                return false;
            }
        }
        if let Some(a) = s["anyOf"].as_array() {
            if !a.iter().any(|s| valid(s, v, depth + 1)) {
                return false;
            }
        }
        if let Some(n) = s.get("not") {
            if valid(n, v, depth + 1) {
                return false;
            }
        }
        if let Some(i) = s.get("if") {
            let branch = if valid(i, v, depth + 1) {
                s.get("then")
            } else {
                s.get("else")
            };
            if branch.is_some_and(|s| !valid(s, v, depth + 1)) {
                return false;
            }
        }
        if let Some(text) = v.as_str() {
            let len = text.chars().count() as u64;
            if s["minLength"].as_u64().is_some_and(|n| len < n)
                || s["maxLength"].as_u64().is_some_and(|n| len > n)
            {
                return false;
            }
            if let Some(pattern) = s["pattern"].as_str() {
                if !regex::Regex::new(pattern).is_ok_and(|r| r.is_match(text)) {
                    return false;
                }
            }
        }
        if let Some(n) = v.as_f64() {
            if s["minimum"].as_f64().is_some_and(|min| n < min)
                || s["maximum"].as_f64().is_some_and(|max| n > max)
            {
                return false;
            }
        }
        if let Some(obj) = v.as_object() {
            if s["required"].as_array().is_some_and(|r| {
                r.iter()
                    .any(|k| !obj.contains_key(k.as_str().unwrap_or("")))
            }) {
                return false;
            }
            for (k, v) in obj {
                if let Some(prop) = s["properties"].get(k) {
                    if !valid(prop, v, depth + 1) {
                        return false;
                    }
                } else if s["additionalProperties"] == false {
                    return false;
                }
            }
        }
        if let Some(a) = v.as_array() {
            let n = a.len() as u64;
            if s["minItems"].as_u64().is_some_and(|m| n < m)
                || s["maxItems"].as_u64().is_some_and(|m| n > m)
            {
                return false;
            }
            for (i, v) in a.iter().enumerate() {
                if s["uniqueItems"] == true && a[..i].contains(v) {
                    return false;
                }
                if let Some(item) = s.get("items") {
                    if !valid(item, v, depth + 1) {
                        return false;
                    }
                }
            }
        }
        true
    }
    if valid(schema, v, 0) {
        Ok(())
    } else {
        Err(Error::Invalid)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn nullable_output_types_still_check_their_other_constraints() {
        let schema = json!({"type":["string","null"],"pattern":"^r_[0-9a-f]{12}$"});
        assert!(validate(&schema, &Value::Null).is_ok());
        assert!(validate(&schema, &json!("r_0123456789ab")).is_ok());
        assert!(validate(&schema, &json!("bad")).is_err());
        assert!(validate(&schema, &json!(false)).is_err());
        assert!(validate(&json!({"type":[]}), &Value::Null).is_err());
    }
}
