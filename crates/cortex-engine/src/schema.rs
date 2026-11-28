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
        let mut plan = tools.iter().find(|t| t["name"] == "plan_submit")
            .and_then(|t| t["inputSchema"]["properties"].get("plan"))
            .cloned().ok_or(Error::Forbidden)?;
        tools.retain(|t| active_steering_read(t["name"].as_str().unwrap_or_default()));
        let source = json!({"type":"string","minLength":1,"maxLength":128,
            "description":"Exact owner message request shown in this steering prompt."});
        let key = json!({"type":"string","minLength":1,"maxLength":128,
            "description":"Stable action key; reuse only for an exact retry of this owner message's action."});
        plan["description"] = json!("Full editable plan from the steering snapshot. Add fresh task ids, revise pending tasks and their dependencies, or omit pending tasks to cancel them. Keep every locked task and the verification commands identical, and carry all accepted outcomes. Remove or replace dependencies on cancelled tasks. New tasks use automatic routing.");
        let node = &mut plan["properties"]["tasks"]["items"];
        node.as_object_mut().ok_or(Error::Forbidden)?.remove("anyOf");
        node["required"].as_array_mut().ok_or(Error::Forbidden)?.retain(|field| field != "competencies");
        node["properties"]["competencies"]["description"] = json!("New or edited nodes must explicitly select competencies and explain an empty selection. Retain an untouched node's existing selection and rationale verbatim, including absence from a legacy snapshot.");
        let role = &mut node["properties"]["role"];
        *role = json!({"type":"string","minLength":1,"maxLength":80,
            "description":"Use explorer or general for new nodes or role changes. An existing node may keep its exact role from the steering snapshot."});
        tools.push(json!({"name":"steer_plan",
            "description":"Propose changes to never-claimed work in the executing plan. The runtime checks the full graph and scope policy and returns added, changed and cancelled node ids. Independent review and verification still run through the ordinary lifecycle.",
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
    if role == "coordinator" && stage == Some("owner_decision") {
        let mut question = tools.iter().find(|t| t["name"] == "user_question_request")
            .cloned().ok_or(Error::Forbidden)?;
        let original = &question["inputSchema"]["properties"];
        question["inputSchema"] = json!({
            "type":"object","additionalProperties":false,
            "required":["question","options","recommended","request_key"],
            "properties":{"question":original["question"],"options":original["options"],
                "recommended":original["recommended"],"request_key":original["request_key"]}
        });
        question["inputSchema"]["properties"]["question"]["description"] =
            json!("One short question about this blocked node in the resolved task language.");
        question["inputSchema"]["properties"]["options"]["description"] =
            json!("Meaningful choices, with labels and any details in the resolved task language.");
        question["inputSchema"]["properties"]["recommended"]["description"] =
            json!("One recommended choice, by its 1-based position in options.");
        question["description"] = json!("Ask one short, plain question about this turn's blocked node, with meaningful options and one recommendation. Write the question and option labels/details in the task language from the prompt. Custom free text stays available. The runtime binds the question to the exact failed assignment without pausing unrelated work. End the turn after the call commits.");
        tools.retain(|t| active_steering_read(t["name"].as_str().unwrap_or_default()));
        for t in &mut tools {
            if t["name"] == "list_sources" {
                t["inputSchema"]["properties"]["answered_after"] = json!({"type":"integer","minimum":0,
                    "description":"List only answered non-permission questions, eight at most per page. Start with zero, then pass next_answered_after until null. Both source refs are returned."});
            } else if t["name"] == "read_source" {
                t["inputSchema"]["properties"]["source_ref"]["description"] = json!("Exact listed source ref. For prior decision evidence page <question_id>:question and <question_id>:answer with direct calls, not read_batch. Follow next_offset until null for both.");
                t["inputSchema"]["properties"]["offset"]["maximum"] = json!(i64::MAX);
            }
        }
        // Keyed by content digest like every tool; found by its advertised name.
        let retry = snapshot()["tools"].as_object().and_then(|tools| {
            tools.values().find(|t| t["name"] == "owner_decision_retry")
        }).cloned().ok_or(Error::Forbidden)?;
        tools.extend([question, retry]);
    }
    tools.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok(tools)
}
pub fn active_steering_read(name: &str) -> bool {
    matches!(name, "tools_discover" | "read_batch" | "read_capsule" | "read_report"
        | "list_reports" | "read_skill" | "list_sources" | "read_source" | "project_read")
}

#[cfg(test)]
#[test]
fn fresh_plans_only_advertise_the_two_execution_bases() {
    let tools = catalogue("coordinator", None, None).unwrap();
    let plan = tools.iter().find(|t| t["name"] == "plan_submit").unwrap();
    let role = &plan["inputSchema"]["properties"]["plan"]["properties"]["tasks"]["items"]["properties"]["role"];
    assert_eq!(role["enum"], json!(cortex_core::lifecycle::plan::PLANNABLE_ROLES));
    for base in ["explorer", "general"] {
        assert!(validate(role, &json!(base)).is_ok());
    }
    for old in ["data-engineer", "code-reviewer", "build-verification", "senior-consultant"] {
        assert!(validate(role, &json!(old)).is_err());
    }
}

#[cfg(test)]
#[test]
fn consultation_proposals_require_a_question_and_reason() {
    let coordinator = catalogue("coordinator", None, None).unwrap();
    let worker = catalogue("worker", Some("worker-general"), Some("execution")).unwrap();
    for tools in [coordinator, worker] {
        let tool = tools.iter().find(|tool| tool["name"] == "escalation_request").unwrap();
        let schema = &tool["inputSchema"];
        let question = "Can the retry distinguish a pending receipt from a committed write?";
        let reason = "The answer selects retry or reconciliation. The receipts observed so far lack a terminal state.";
        assert!(validate(schema, &json!({"request_key":"proposal","reason":reason})).is_err());
        assert!(validate(schema, &json!({"request_key":"proposal","question":question})).is_err());
        assert!(validate(schema, &json!({"request_key":"proposal","reason":reason,"question":question})).is_ok());
    }
}

#[cfg(test)]
#[test]
fn plan_advertises_an_explicit_competency_choice() {
    let tools = catalogue("coordinator", None, None).unwrap();
    let plan = tools.iter().find(|t| t["name"] == "plan_submit").unwrap();
    let node = &plan["inputSchema"]["properties"]["plan"]["properties"]["tasks"]["items"];
    let mut input = json!({"id":"study","role":"explorer","objective":"Compare manifest fields",
        "dependencies":[],"scope":["manifest"],"model":"auto"});
    assert!(validate(node, &input).is_err());
    input["competencies"] = json!([]);
    assert!(validate(node, &input).is_err());
    input["base_sufficiency_reason"] = json!(" \t ");
    assert!(validate(node, &input).is_err());
    input["base_sufficiency_reason"] = json!("The checked-in manifest provides both fields for a direct comparison.");
    assert!(validate(node, &input).is_ok());
    input.as_object_mut().unwrap().remove("base_sufficiency_reason");
    input["competencies"] = json!(["database"]);
    assert!(validate(node, &input).is_ok());
}

#[cfg(test)]
#[test]
fn owner_decision_tools_are_only_offered_on_the_scoped_coordinator_channel() {
    let retry = |tools: &[Value]| tools.iter().any(|t| t["name"] == "owner_decision_retry");
    assert!(!retry(&catalogue("coordinator", None, None).unwrap()));
    assert!(!retry(&catalogue("coordinator", None, Some("active_steering")).unwrap()));
    assert!(!retry(&catalogue("worker", Some("worker-backend-dev"), Some("execution")).unwrap()));
    let tools = catalogue("coordinator", None, Some("owner_decision")).unwrap();
    assert!(retry(&tools));
    for name in ["list_sources","read_source"] {
        assert!(tools.iter().any(|tool| tool["name"] == name));
    }
    assert!(!tools.iter().any(|t| t["name"] == "plan_submit" || t["name"] == "project_edit"));
    let ask = tools.iter().find(|t| t["name"] == "user_question_request").unwrap();
    let schema = &ask["inputSchema"];
    assert!(validate(schema, &json!({"question":"Keep it?", "options":["Yes","No"],
        "recommended":1,"request_key":"ask"})).is_ok());
    assert!(validate(schema, &json!({"question":"Keep it?", "options":["Yes"],
        "recommended":1,"request_key":"ask"})).is_err());
    assert!(validate(schema, &json!({"question":"Keep it?","request_key":"ask"})).is_err());
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
