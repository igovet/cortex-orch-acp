// Default for no-model fixtures. Tests of the choice contract bypass this helper.
#[allow(dead_code)]
pub fn fill_plan_choices(plan: &mut serde_json::Value) {
    for node in plan["tasks"].as_array_mut().into_iter().flatten() {
        if node.get("competencies").is_none() {
            node["competencies"] = serde_json::json!([]);
            node["base_sufficiency_reason"] = serde_json::json!(
                "This bounded fixture uses direct project reads or edits and checks the resulting file."
            );
        }
    }
}
