//! Catalogue and instruction boundaries for canonical and retained profiles.
use serde_json::Value;
use std::{collections::BTreeSet, path::Path};

#[test]
fn sprint_planning_questions_are_grounded_in_project_rules_and_evidence() {
    let tools = cortex_engine::schema::catalogue("coordinator", None, None).unwrap();
    for name in ["user_question_request", "sprint_plan_submit"] {
        let description = tools.iter().find(|tool| tool["name"] == name).unwrap()["description"]
            .as_str().unwrap();
        assert!(description.contains("applicable project rules"));
        assert!(description.contains("implementation"));
        assert!(description.contains("mandatory independent review"));
        assert!(description.contains("required profiles"));
    }
}

#[test]
fn fresh_execution_and_review_fixes_only_offer_canonical_roles() {
    let schema: Value = serde_json::from_str(include_str!("../schema/tools.json")).unwrap();
    let tools = schema["tools"].as_object().unwrap();
    let plan = tools.values().find(|t| t["name"] == "plan_submit").unwrap();
    let roles = &plan["inputSchema"]["properties"]["plan"]["properties"]["tasks"]["items"]["properties"]["role"]["enum"];
    assert_eq!(roles, &serde_json::json!(["explorer", "general"]));
    let mut fix_selectors = 0;
    for tool in tools.values() {
        let role = &tool["inputSchema"]["properties"]["findings"]["items"]["properties"]["role"]["enum"];
        if !role.is_null() {
            assert_eq!(role, &serde_json::json!(["general"]));
            fix_selectors += 1;
        }
    }
    assert!(fix_selectors > 0, "review findings expose the repair role");
    let routing = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../skills/orchestrator/references/worker-routing.md"),
    ).unwrap();
    assert!(routing.contains("live plan schema and routing menu"));
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

#[test]
fn project_write_offers_binary_content_as_base64() {
    let schema: Value = serde_json::from_str(include_str!("../schema/tools.json")).unwrap();
    let write = schema["tools"]
        .as_object()
        .unwrap()
        .values()
        .find(|t| t["name"] == "project_write")
        .expect("project_write");
    let description = write["description"].as_str().unwrap();
    assert!(description.contains("base64"), "{description}");
    assert_eq!(write["inputSchema"]["properties"]["encoding"]["enum"], serde_json::json!(["utf8","base64"]));
}
