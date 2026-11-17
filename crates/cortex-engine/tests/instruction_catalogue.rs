//! The orchestrator's worker-routing catalogue lists exactly the plannable
//! roles of the advertised plan schema.
use serde_json::Value;
use std::{collections::BTreeSet, path::Path};

fn find_role_enum(value: &Value) -> Option<Vec<String>> {
    match value {
        Value::Object(map) => {
            if let Some(roles) = map.get("role").and_then(|r| r["enum"].as_array()) {
                if roles.iter().any(|r| r == "backend-dev") {
                    return Some(roles.iter().filter_map(|r| r.as_str().map(str::to_owned)).collect());
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
    let plannable: BTreeSet<String> = find_role_enum(&schema["tools"]).expect("plan node role enum").into_iter().collect();
    let routing = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/orchestrator/references/worker-routing.md"),
    )
    .unwrap();
    let catalogue: BTreeSet<String> = regex::Regex::new(r"(?m)^- `([^`]+)`:")
        .unwrap()
        .captures_iter(&routing)
        .map(|c| c[1].to_owned())
        .collect();
    assert_eq!(catalogue, plannable);
    // The reserved roles (reviewer, build verification, consultant) are never plannable.
    for reserved in ["code-reviewer", "build-verification", "senior-consultant"] {
        assert!(!plannable.contains(reserved), "{reserved}");
    }
}
