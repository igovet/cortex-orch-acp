#[path = "support/hermetic.rs"]
mod hermetic;
use serde_json::{json, Value};
use workspace_service::config_read_model::settings_view;

#[test]
fn settings_envelopes_match_the_oracle_projection() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/settings-view-python.json")).unwrap();
    let cases = fixture["cases"].as_array().unwrap();
    assert_eq!(cases.len(), 56);
    for case in cases {
        let original = case["input"].clone();
        assert_eq!(
            settings_view(&original).unwrap(),
            case["expected"],
            "{}",
            case["name"]
        );
        assert_eq!(original, case["input"]);
    }
}
#[test]
fn settings_capture_provenance_is_current() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/settings-view-python.json")).unwrap();
    assert_eq!(fixture["evidence"]["real_engine_calls"], 0);
}
#[test]
fn invalid_composition_roots_refuse_instead_of_inventing_empty_settings() {
    for input in [
        json!(null),
        json!({}),
        json!({"surfaces":[],"workspace":{},"entries":[],"usage_records":[]}),
        json!({"surfaces":{},"workspace":{},"entries":[],"usage_records":[]}),
    ] {
        assert_eq!(settings_view(&input), Err("invalid_settings_observation"));
    }
}
/// A server shows `containment: "none"` only when it effectively runs without
/// the sandbox (its own value, else the registry default); contained rows are unchanged.
#[test]
fn uncontained_servers_are_labelled_from_their_own_value_or_the_registry_default() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/settings-view-python.json")).unwrap();
    let mut input = fixture["cases"][0]["input"].clone();
    let server = |name: &str, containment: Option<&str>| {
        let mut config = json!({"command":"/opt/tool","allowed_tools":["read"]});
        if let Some(c) = containment {
            config["containment"] = json!(c);
        }
        json!({"name":name,"enabled":true,"valid":true,"problem":null,"config":config,"unsupported_keys":[]})
    };
    let servers = json!([
        server("own", Some("none")),
        server("inherits", None),
        server("kept", Some("contained"))
    ]);
    let labels = |input: &Value| {
        settings_view(input).unwrap()["config"]["surfaces"]["mcp_servers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r.get("containment").cloned())
            .collect::<Vec<_>>()
    };
    input["surfaces"]["mcp_servers"]["result"]["servers"] = servers.clone();
    assert_eq!(labels(&input), [Some(json!("none")), None, None]);
    input["surfaces"]["mcp_servers"]["result"]["containment"] = json!("none");
    assert_eq!(
        labels(&input),
        [Some(json!("none")), Some(json!("none")), None]
    );
}
