//! Explicit owner workflow descriptions; ordinary task prose never activates these.
use super::native::*;
use serde_json::{json, Value};

pub fn describe(params: &Value) -> Result<Value> {
    let action = params["action"].as_str().unwrap_or("");
    let scope = params.get("scope").and_then(Value::as_str).unwrap_or("");
    if !["harvest", "harvest-refresh", "skill-management"].contains(&action)
        || params.get("scope").is_some_and(|s| !s.is_string())
        || scope.chars().count() > 30000
    {
        return Err(refuse(
            "invalid_workflow",
            "Choose a supported workflow and a scope up to 30000 characters.",
        ));
    }
    let scope = scope.trim();
    if action == "skill-management" {
        if scope.is_empty() {
            return Err(refuse(
                "description_required",
                "Describe the skill to create, revise or import.",
            ));
        }
        return Ok(
            json!({"description":format!("{scope}\n\nThe user explicitly invoked /skill-management. Read the coordinator-only shared skill skill-management using read_skill before planning. Treat the supplied request as the complete skill-management request. This workflow is active only because the user invoked the registered command; ordinary prose does not activate it.")}),
        );
    }
    let refresh = action == "harvest-refresh";
    let goal = if scope.is_empty() {
        if refresh {
            "Update the project's existing source-backed documentation"
        } else {
            "Create complete source-backed documentation for the project"
        }
    } else {
        scope
    };
    let suffix = if scope.is_empty() {
        String::new()
    } else {
        format!(" ({scope})")
    };
    let instruction = if refresh {
        format!("The user explicitly requested a documentation refresh{suffix}. Read `knowledge-harvest-scope` with `read_skill` and update the project's source-backed documentation for this task; this is an explicit request, not an inference.")
    } else {
        format!("The user explicitly requested a knowledge harvest{suffix}. Read `knowledge-harvest-scope` with `read_skill` and carry it out for this task; this is the explicit request that skill requires, not an inference from anything said earlier.")
    };
    Ok(json!({"description":format!("{goal}\n\n{instruction}")}))
}
