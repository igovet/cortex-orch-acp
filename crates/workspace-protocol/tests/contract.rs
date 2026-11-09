use serde_json::{json, Value};
use workspace_protocol::*;

#[test]
fn shared_vectors() {
    let vectors: Vec<Value> = serde_json::from_str(include_str!(
        "../../../packages/workspace-contract/fixtures.json"
    ))
    .unwrap();
    for vector in vectors {
        let result = validate(vector["type"].as_str().unwrap(), &vector["value"]);
        assert_eq!(
            result.is_ok(),
            vector["valid"].as_bool().unwrap(),
            "{}",
            vector["name"]
        );
        let typed = decode_typed(vector["type"].as_str().unwrap(), vector["value"].clone());
        assert_eq!(
            typed.is_ok(),
            vector["valid"].as_bool().unwrap(),
            "serde {}",
            vector["name"]
        );
    }
}

#[test]
fn version_and_intersection() {
    let local = capabilities("test-build", "7", &["capabilities.get", "tasks.get"]).unwrap();
    let mut remote = capabilities("peer-build", "8", &["tasks.get", "events.page"]).unwrap();
    remote["protocol"]["major"] = json!(1.0);
    remote["protocol"]["minor"] = json!(99.0);
    assert_eq!(
        negotiate(&local, &remote).unwrap()["operations"],
        json!(["tasks.get"])
    );
    remote["protocol"]["major"] = json!(2);
    assert_eq!(negotiate(&local, &remote).unwrap_err(), "version_mismatch");
}

#[test]
fn unknown_is_not_success_and_response_is_additive() {
    let receipt = json!({"requestId":"req", "outcome":"future_success", "target":"task",
        "entityVersion":"9007199254740993", "recordedAt":"2026-11-05T00:00:00Z", "extra":true});
    let (decoded, warnings) = decode_response("Receipt", &receipt).unwrap();
    assert_eq!(decoded["outcome"], "unknown");
    assert_eq!(warnings, vec!["$.outcome:unknown_enum"]);
    assert_eq!(decoded["extra"], true);
    let typed: Receipt = serde_json::from_value(receipt).unwrap();
    assert_eq!(typed.as_value()["outcome"], "unknown");
    assert_eq!(typed.warnings(), &["$.outcome:unknown_enum"]);
}

#[test]
fn json_schema_numbers_and_unicode_boundaries() {
    assert!(validate("Protocol", &json!({"major":1.0,"minor":0})).is_ok());
    assert!(validate("Protocol", &json!({"major":1.5,"minor":0})).is_err());
    assert!(validate("Protocol", &json!({"major":true,"minor":0})).is_err());
    let vectors: Vec<Value> = serde_json::from_str(include_str!(
        "../../../packages/workspace-contract/fixtures.json"
    ))
    .unwrap();
    let mut command = vectors.iter().find(|v| v["name"] == "create").unwrap()["value"].clone();
    command["payload"]["description"] = json!("😀".repeat(32000));
    assert!(validate("Command", &command).is_ok());
    command["payload"]["description"] = json!("😀".repeat(32001));
    assert!(validate("Command", &command).is_err());
    let page = json!({"items":[],"nextCursor":null,"highWatermark":"0","future":"é".repeat(33000)});
    assert_eq!(validate("TaskPage", &page).unwrap_err(), "page_too_large");
}

#[test]
fn framed_json_and_limits() {
    let value = json!({"text":"line\n😀"});
    assert_eq!(decode_frame(&encode_frame(&value).unwrap()).unwrap(), value);
    for bytes in [
        vec![],
        vec![0, 0, 0, 1],
        vec![0, 16, 0, 1],
        vec![0, 0, 0, 1, 255],
    ] {
        assert!(decode_frame(&bytes).is_err());
    }
    assert!(encode_frame(&json!({"text":"x".repeat(1048576)})).is_err());
    let mut nested = Value::Null;
    for _ in 0..65 {
        nested = json!([nested]);
    }
    assert_eq!(encode_frame(&nested).unwrap_err(), "nesting_limit");
}

#[test]
fn typed_command_deserialization_is_strict() {
    let vectors: Vec<Value> = serde_json::from_str(include_str!(
        "../../../packages/workspace-contract/fixtures.json"
    ))
    .unwrap();
    let mut command = vectors
        .iter()
        .find(|v| v["type"] == "Command" && v["valid"] == true)
        .unwrap()["value"]
        .clone();
    assert!(serde_json::from_value::<Command>(command.clone()).is_ok());
    command["extra"] = json!(true);
    assert!(serde_json::from_value::<Command>(command).is_err());
}
