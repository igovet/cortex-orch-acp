use serde_json::json;
use workspace_protocol::core::*;
use workspace_protocol::events::*;
#[test]
fn canonical_decimals_reject_numbers_signs_leading_zero_overflow() {
    for value in [
        json!(1),
        json!("-1"),
        json!("01"),
        json!("+1"),
        json!("9223372036854775808"),
        json!("١"),
        json!(""),
    ] {
        assert!(serde_json::from_value::<Version>(value).is_err());
    }
    for text in ["0", "1", "9223372036854775807"] {
        let v: Version = serde_json::from_value(json!(text)).unwrap();
        assert_eq!(serde_json::to_value(v).unwrap(), json!(text));
    }
}
#[test]
fn native_envelope_exact_shape_unknown_fields_and_protocol_gate() {
    let value = json!({"protocol":{"major":2,"mapping":2},"requestId":"k","projectId":"p",
        "expectedVersion":"1","expectedOwnerEpoch":"2","operation":"tasks.start","payload":{"id":"t"}});
    let request: RequestEnvelope<CoreCommand> = serde_json::from_value(value.clone()).unwrap();
    request.validate().unwrap();
    assert_eq!(serde_json::to_value(&request).unwrap(), value);
    let mut old = value.clone();
    old["protocol"]["major"] = json!(1);
    let old: RequestEnvelope<CoreCommand> = serde_json::from_value(old).unwrap();
    assert_eq!(old.validate(), Err("unsupported_protocol"));
    let mut bad = value.clone();
    bad["actor"] = json!("owner");
    assert!(serde_json::from_value::<RequestEnvelope<CoreCommand>>(bad).is_err());
    let mut bad = value.clone();
    bad["payload"]["shell"] = json!("not-a-command");
    assert!(serde_json::from_value::<RequestEnvelope<CoreCommand>>(bad).is_err());
    let mut bad = value;
    bad["operation"] = json!("arbitrary.sql");
    assert!(serde_json::from_value::<RequestEnvelope<CoreCommand>>(bad).is_err());
}
#[test]
fn duplicate_request_and_payload_keys_are_not_silently_overwritten() {
    for text in [
        r#"{"protocol":{"major":2,"mapping":2},"requestId":"a","requestId":"b","operation":"tasks.start","payload":{"id":"t"}}"#,
        r#"{"protocol":{"major":2,"mapping":2},"requestId":"a","operation":"tasks.start","payload":{"id":"t","id":"other"}}"#,
        r#"{"protocol":{"major":2,"mapping":2},"requestId":"a","operation":"tasks.start","operation":"tasks.start","payload":{"id":"t"}}"#,
    ] {
        assert!(
            serde_json::from_str::<RequestEnvelope<CoreCommand>>(text).is_err(),
            "{text}"
        );
    }
}
#[test]
fn deltas_use_entity_versions_and_reject_conflicting_upserts() {
    let mut b = DeltaBatch {
        generation: "g".into(),
        stream_key: StreamKey::Project {
            project_id: "p".into(),
        },
        from_sequence: Version::new(1).unwrap(),
        through_sequence: Version::new(2).unwrap(),
        changes: vec![EntityChange {
            kind: EntityKind::Task,
            id: "t".into(),
            base_version: Version::new(0).unwrap(),
            new_version: Version::new(1).unwrap(),
            operation: DeltaOperation::Upsert(PublicEntity::Task(TaskSummary {
                id: "t".into(),
                version: Version::new(1).unwrap(),
                state: TaskState::Planning,
            })),
        }],
    };
    b.validate().unwrap();
    b.changes[0].id = "wrong".into();
    assert_eq!(b.validate(), Err("conflicting_delta"));
    b.changes[0].id = "t".into();
    b.changes[0].base_version = Version::new(1).unwrap();
    assert_eq!(b.validate(), Err("invalid_delta_version"));
}
#[test]
fn decoder_enforces_text_frame_utf8_and_page_bounds_before_dispatch() {
    let mut v = json!({"protocol":{"major":2,"mapping":2},"requestId":"k","operation":"tasks.create",
        "payload":{"taskId":"t","description":"界".repeat(32000)}});
    let bytes = serde_json::to_vec(&v).unwrap();
    decode_native_command(&bytes).unwrap();
    v["payload"]["description"] = json!("界".repeat(32001));
    assert_eq!(
        decode_native_command(&serde_json::to_vec(&v).unwrap()).unwrap_err(),
        "text_too_large"
    );
    assert_eq!(
        decode_native_command(&vec![b' '; 8388609]).unwrap_err(),
        "frame_too_large"
    );
    assert_eq!(
        decode_native_command(&[0xff]).unwrap_err(),
        "invalid_request"
    );
    let query = json!({"protocol":{"major":2,"mapping":2},"requestId":"k","operation":"tasks.page",
        "payload":{"target":"p","cursor":null,"limit":51}});
    assert_eq!(
        decode_native_query(&serde_json::to_vec(&query).unwrap()).unwrap_err(),
        "invalid_page_limit"
    );
}
/// A conversation read from its end (`history.earlier`) decodes with the same bounded page as `history.page`.
#[test]
fn a_backward_history_read_decodes_with_the_history_page_bounds() {
    let query = |limit: u64, extra: Option<(&str, serde_json::Value)>| {
        let mut v = json!({"protocol":{"major":2,"mapping":2},"requestId":"k","projectId":"p","operation":"history.earlier",
            "payload":{"target":"t","cursor":null,"limit":limit}});
        if let Some((k, value)) = extra {
            v["payload"][k] = value;
        }
        decode_native_query(&serde_json::to_vec(&v).unwrap())
    };
    assert!(matches!(
        query(100, None).unwrap().body,
        workspace_protocol::core::CoreQuery::HistoryEarlier(_)
    ));
    assert_eq!(query(101, None).unwrap_err(), "invalid_page_limit");
    assert_eq!(query(0, None).unwrap_err(), "invalid_page_limit");
    assert_eq!(
        query(100, Some(("direction", json!("backward")))).unwrap_err(),
        "invalid_request"
    );
}
