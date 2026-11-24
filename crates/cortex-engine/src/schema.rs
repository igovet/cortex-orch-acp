//! Advertised contracts are data, not profile prose. Snapshot of the Python oracle.
//! Sources: src/cortex_runtime/channeled_tools.py:25-27,
//! delegation/mcp_tools.py:755-797 and tool_read_api.py:66-96.
use crate::{Error, Result};
use serde_json::{json, Value};
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
    if role == "coordinator" && stage == Some("active_steering") {
        let plan = tools.iter().find(|t| t["name"] == "plan_submit")
            .and_then(|t| t["inputSchema"]["properties"].get("plan"))
            .cloned().ok_or(Error::Forbidden)?;
        tools.retain(|t| active_steering_read(t["name"].as_str().unwrap_or_default()));
        let source = json!({"type":"string","minLength":1,"maxLength":128,
            "description":"Exact owner message request shown in this steering prompt."});
        let key = json!({"type":"string","minLength":1,"maxLength":128,
            "description":"Stable action key; reuse only for an exact retry of this owner message's action."});
        tools.push(json!({"name":"steer_plan",
            "description":"Propose a replacement of unclaimed work in the executing plan. Every already claimed or completed node and the verification commands must remain identical. The runtime validates the graph and owner limits; it never restarts a worker for this operation.",
            "inputSchema":{"type":"object","additionalProperties":false,
                "required":["source_request","request_key","expected_plan","plan"],
                "properties":{"source_request":source,"request_key":key,
                    "expected_plan":{"type":"string","minLength":64,"maxLength":64,
                        "description":"Current plan digest from the steering snapshot or the last amendment receipt."},
                    "plan":plan}}}));
        tools.push(json!({"name":"steer_worker",
            "description":"Queue instructions on the exact live execution attempt, without granting scope or changing its assignment. The receipt confirms admission, not delivery; a completed attempt is refused and a later attempt never inherits the instruction.",
            "inputSchema":{"type":"object","additionalProperties":false,
                "required":["source_request","request_key","attempt","text"],
                "properties":{"source_request":source,"request_key":key,
                    "attempt":{"type":"string","minLength":1,"maxLength":128,
                        "description":"Exact live attempt from the steering snapshot."},
                    "text":{"type":"string","minLength":1,"maxLength":4000,
                        "description":"English instruction within the current assignment's authority."}}}}));
    }
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(tools)
}
pub fn active_steering_read(name: &str) -> bool {
    matches!(name, "tools_discover" | "read_batch" | "read_capsule" | "read_report"
        | "list_reports" | "read_skill" | "list_sources" | "read_source" | "project_read")
}

pub fn validate(schema: &Value, v: &Value) -> Result<()> {
    fn valid(
        s: &Value,
        v: &Value,
        depth: usize,
        path: &mut String,
    ) -> std::result::Result<(), &'static str> {
        if depth > 64 {
            return Err("depth_limit");
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
                return Err("wrong_type");
            }
        }
        if let Some(a) = s["enum"].as_array() {
            if !a.contains(v) {
                return Err("invalid_enum");
            }
        }
        if let Some(a) = s["anyOf"].as_array() {
            if !a
                .iter()
                .any(|s| valid(s, v, depth + 1, &mut path.clone()).is_ok())
            {
                return Err("no_matching_alternative");
            }
        }
        if let Some(n) = s.get("not") {
            if valid(n, v, depth + 1, &mut path.clone()).is_ok() {
                return Err("excluded_value");
            }
        }
        if let Some(i) = s.get("if") {
            let branch = if valid(i, v, depth + 1, &mut path.clone()).is_ok() {
                s.get("then")
            } else {
                s.get("else")
            };
            if let Some(branch) = branch {
                valid(branch, v, depth + 1, path)?;
            }
        }
        if let Some(text) = v.as_str() {
            let len = text.chars().count() as u64;
            if s["minLength"].as_u64().is_some_and(|n| len < n)
                || s["maxLength"].as_u64().is_some_and(|n| len > n)
            {
                return Err("string_length");
            }
            if let Some(pattern) = s["pattern"].as_str() {
                if !regex::Regex::new(pattern).is_ok_and(|r| r.is_match(text)) {
                    return Err("invalid_pattern");
                }
            }
        }
        if let Some(n) = v.as_f64() {
            if s["minimum"].as_f64().is_some_and(|min| n < min)
                || s["maximum"].as_f64().is_some_and(|max| n > max)
            {
                return Err("number_out_of_bounds");
            }
        }
        if let Some(obj) = v.as_object() {
            if let Some(missing) = s["required"].as_array().and_then(|keys| {
                keys.iter()
                    .filter_map(Value::as_str)
                    .find(|k| !obj.contains_key(*k))
            }) {
                path.push_str(&format!(".{missing}"));
                return Err("missing_required_field");
            }
            for (k, v) in obj {
                if let Some(prop) = s["properties"].get(k) {
                    let end = path.len();
                    path.push_str(&format!(".{k}"));
                    valid(prop, v, depth + 1, path)?;
                    path.truncate(end);
                } else if s["additionalProperties"] == false {
                    // Bound caller-controlled names and quote ambiguous path segments.
                    let mut chars = k.chars();
                    let name: String = chars.by_ref().take(128).collect();
                    let truncated = chars.next().is_some();
                    if !truncated
                        && !name.is_empty()
                        && name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                    {
                        path.push('.');
                        path.push_str(&name);
                    } else {
                        path.push_str(&format!("[{}", json!(name)));
                        if truncated {
                            path.push_str("...");
                        }
                        path.push(']');
                    }
                    return Err("unexpected_property");
                }
            }
        }
        if let Some(a) = v.as_array() {
            let n = a.len() as u64;
            if s["minItems"].as_u64().is_some_and(|m| n < m)
                || s["maxItems"].as_u64().is_some_and(|m| n > m)
            {
                return Err("array_length");
            }
            for (i, v) in a.iter().enumerate() {
                if s["uniqueItems"] == true && a[..i].contains(v) {
                    return Err("duplicate_item");
                }
                if let Some(item) = s.get("items") {
                    let end = path.len();
                    path.push_str(&format!("[{i}]"));
                    valid(item, v, depth + 1, path)?;
                    path.truncate(end);
                }
            }
        }
        Ok(())
    }
    let mut path = String::from("arguments");
    valid(schema, v, 0, &mut path).map_err(|reason| Error::InvalidField {
        field: path,
        reason,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn schema_examples_and_nested_edit_failures_match_the_advertised_contract() {
        for tool in snapshot()["tools"].as_object().unwrap().values() {
            if let Some(examples) = tool["inputSchema"]["examples"].as_array() {
                for args in examples {
                    validate(&tool["inputSchema"], args).unwrap();
                }
            }
        }
        let schema = snapshot()["tools"]
            .as_object()
            .unwrap()
            .values()
            .find(|t| t["name"] == "project_edit")
            .unwrap()["inputSchema"]
            .clone();
        let args = json!({"path":"fixture","edits":[{"old_string":"x","new_string":""},{"old_string":"y"}]});
        assert!(
            matches!(validate(&schema,&args),Err(Error::InvalidField{field,reason:"missing_required_field"}) if field=="arguments.edits[1].new_string")
        );
    }
    #[test]
    fn question_option_details_accept_legacy_and_detailed_choices() {
        for (role, profile, name) in [
            ("coordinator", None, "user_question_request"),
            ("worker", Some("worker-backend-dev"), "ask_question"),
        ] {
            let tool = catalogue(role, profile, Some("execution"))
                .unwrap()
                .into_iter()
                .find(|t| t["name"] == name)
                .unwrap();
            for batch in [false, true] {
                let args = |option: Value| {
                    let q = json!({"question":"Use this route?", "options":[option, "No"], "recommended":2});
                    if batch {
                        json!({"request_key":"q", "questions":[q, {"question":"Continue?", "options":["Yes","No"], "recommended":1}]})
                    } else {
                        let mut q = q;
                        q["request_key"] = json!("q");
                        q
                    }
                };
                let schema = &tool["inputSchema"];
                assert!(validate(schema, &args(json!("Yes"))).is_ok());
                let mut question = args(json!("Yes"));
                let member = if batch { &mut question["questions"][0] } else { &mut question };
                member.as_object_mut().unwrap().remove("recommended");
                assert!(validate(schema, &question).is_err());
                for invalid in [json!(0), json!(9), json!("1"), json!([1, 2])] {
                    let mut question = args(json!("Yes"));
                    let member = if batch { &mut question["questions"][0] } else { &mut question };
                    member["recommended"] = invalid;
                    assert!(validate(schema, &question).is_err());
                }
                assert!(validate(schema, &args(json!({"label":"Yes"}))).is_ok());
                assert!(validate(schema, &args(json!({"label":"Yes", "detail":"я".repeat(1000)}))).is_ok());
                for invalid in [
                    json!("A".repeat(1001)),
                    json!({"label":"A".repeat(1001)}),
                    json!({"label":"Yes","detail":"я".repeat(1001)}),
                    json!({"label":"Yes","detail":""}),
                    json!({"label":"Yes","detail":null}),
                    json!({"detail":"Missing label"}),
                    json!({"label":"Yes","extra":"no"}),
                ] {
                    assert!(validate(schema, &args(invalid.clone())).is_err(), "{name}: {invalid}");
                }
            }
        }
    }
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
