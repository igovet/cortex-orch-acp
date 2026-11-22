//! The orchestrator's worker-routing catalogue lists exactly the plannable
//! roles of the advertised plan schema.
use serde_json::Value;
use std::{collections::BTreeSet, path::Path};

fn find_role_enum(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::Object(map) => {
            if let Some(roles) = map.get("role").and_then(|r| r["enum"].as_array()) {
                if roles.iter().any(|r| r == "backend-dev") {
                    return Some(
                        roles
                            .iter()
                            .filter_map(|r| r.as_str().map(str::to_owned))
                            .collect(),
                    );
                }
            }
            map.values().find_map(find_role_enum)
        }
        Value::Array(items) => items.iter().find_map(find_role_enum),
        _ => None,
    }
}

#[test]
fn worker_routing_catalogue_equals_the_plannable_role_enum() {
    let schema: Value = serde_json::from_str(include_str!("../schema/tools.json")).unwrap();
    // The plan's node roles; a review finding's proposed fix role is a subset.
    let plan = schema["tools"]
        .as_object()
        .unwrap()
        .values()
        .find(|t| t["name"] == "plan_submit")
        .expect("plan_submit");
    let plannable: BTreeSet<String> = find_role_enum(plan)
        .expect("plan node role enum")
        .into_iter()
        .collect();
    let routing = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../skills/orchestrator/references/worker-routing.md"),
    )
    .unwrap();
    let catalogue: BTreeSet<String> = regex::Regex::new(r"(?m)^- `([^`]+)`:")
        .unwrap()
        .captures_iter(&routing)
        .map(|c| c[1].to_owned())
        .collect();
    assert_eq!(catalogue, plannable);
    // Every fix role a review finding may propose is a plannable writing role.
    for tool in schema["tools"].as_object().unwrap().values() {
        let proposed =
            &tool["inputSchema"]["properties"]["findings"]["items"]["properties"]["role"]["enum"];
        for role in proposed.as_array().into_iter().flatten() {
            let role = role.as_str().unwrap();
            assert!(plannable.contains(role), "{role}");
            assert!(
                ![
                    "architect",
                    "explorer",
                    "planner",
                    "security-auditor",
                    "ux-designer",
                    "accessibility-auditor",
                    "database-architect",
                    "performance-engineer"
                ]
                .contains(&role),
                "{role}"
            );
        }
    }
    // The reserved roles (reviewer, build verification, consultant) are never plannable.
    for reserved in ["code-reviewer", "build-verification", "senior-consultant"] {
        assert!(!plannable.contains(reserved), "{reserved}");
    }
}

/// What a catalogue describes is what it serves: the consultant's tools name no
/// command, git, access or escalation tool, read-only and review catalogues do not
/// serve access_request, and create_draft offers no template it always refuses.
#[test]
fn catalogues_describe_only_what_they_serve() {
    let schema: Value = serde_json::from_str(include_str!("../schema/tools.json")).unwrap();
    let read_only = [
        "worker-architect",
        "worker-ux-designer",
        "worker-code-reviewer",
        "worker-senior-consultant",
        "worker-explorer",
        "worker-planner",
        "worker-security-auditor",
        "worker-accessibility-auditor",
        "worker-database-architect",
        "worker-performance-engineer",
        "worker-build-verification",
    ];
    for (key, ids) in schema["catalogues"].as_object().unwrap() {
        let tools: Vec<&Value> = ids
            .as_array()
            .unwrap()
            .iter()
            .map(|id| &schema["tools"][id.as_str().unwrap()])
            .collect();
        let served: BTreeSet<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        let parts: Vec<&str> = key.split('|').collect();
        if parts[0] == "worker" && (parts[2] == "review" || read_only.contains(&parts[1])) {
            assert!(
                !served.contains("access_request"),
                "{key} serves access_request"
            );
        }
        // The consultant is dispatched only at the execution stage.
        if parts[1] == "worker-senior-consultant" && parts[2] == "execution" {
            for tool in &tools {
                let text = tool.to_string();
                for name in [
                    "command_run",
                    "outside_command",
                    "check_preflight",
                    "project_git",
                    "access_request",
                    "escalation_request",
                ] {
                    assert!(
                        served.contains(name) || !text.contains(name),
                        "{key}: {} describes unserved {name}",
                        tool["name"]
                    );
                }
            }
        }
        for tool in &tools {
            if tool["name"] == "create_draft" {
                let templates = &tool["inputSchema"]["properties"]["template"]["enum"];
                assert!(
                    !templates
                        .as_array()
                        .unwrap()
                        .iter()
                        .any(|t| t == "pipeline"),
                    "{key}"
                );
            }
            assert!(
                tool["inputSchema"]["properties"]
                    .get("redact_values")
                    .is_none(),
                "{key}: {}",
                tool["name"]
            );
            // Execution-stage results have no verdict field, and their description says so.
            if tool["name"] == "result_submit" && parts.get(2) == Some(&"execution") {
                assert!(
                    tool["inputSchema"]["properties"].get("verdict").is_none(),
                    "{key}"
                );
                assert!(
                    !tool["description"]
                        .as_str()
                        .unwrap()
                        .contains("Reviewers must include verdict"),
                    "{key}"
                );
            }
        }
    }
}
