//! Static ports of the legacy routing-consent matrices. No Python or live engines.
use super::{
    consent::{normalize_effort, plain},
    grammar as g, RouteEnvironment, RouteResolver,
};
use cortex_core::routing::{Evidence, Instruction, UserInput};
use serde_json::{json, Value};

#[test]
fn legacy_consent_case_matrix() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../tests/fixtures/routing_consent.json")).unwrap();
    let mut failures = Vec::new();
    for c in fixture["cases"].as_array().unwrap() {
        let names: Vec<g::Name> = fixture["profiles"][c["profile"].as_u64().unwrap() as usize]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| g::Name {
                label: n["label"].as_str().unwrap().into(),
                aliases: n["aliases"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().into())
                    .collect(),
                bare: n["bare"] == true,
            })
            .collect();
        let hint = c["hint"].as_str().unwrap();
        let role = c["role"].as_str().unwrap();
        let mut rules = json!({"models":{},"enabled":{}});
        for n in &names {
            let (e, m) = n.label.split_once('/').unwrap();
            if rules["models"][e].is_null() {
                rules["models"][e] = json!({"tiers":{},"policy":{}});
                rules["enabled"][e] = json!([]);
            }
            rules["models"][e]["tiers"][m] = json!("standard");
            rules["models"][e]["policy"][m] = json!({"request_names":n.aliases});
            rules["enabled"][e].as_array_mut().unwrap().push(json!(m));
        }
        let resolver = RouteResolver {
            environment: RouteEnvironment {
                home: "/".into(),
                config: "/".into(),
                state: "/".into(),
                routing_override: None,
            },
            rules,
            defaults: json!({}),
            candidates: vec![],
        };
        let text = plain(c["text"].as_str().unwrap());
        let normalized = normalize_effort(&text, &names);
        let all = g::mentions(&normalized, &names);
        let matches: Vec<_> = all
            .iter()
            .filter(|m| names[m.model].label == hint)
            .collect();
        let mode = c["mode"].as_str().unwrap_or("consent");
        let mut actual = Value::Null;
        if mode == "grammar" || mode == "assignment" || mode == "refusal" {
            for m in matches {
                let yes = match mode {
                    "grammar" => g::requested(&normalized, m, &names, c["exact"] == true),
                    "assignment" => g::assignment(&normalized, m).is_some(),
                    _ => g::refused(&normalized, m, true),
                };
                if yes {
                    actual = json!(m.text(&normalized));
                    break;
                }
            }
            let ok = if c.get("expected_bool").is_some() {
                actual.is_string() == (c["expected_bool"] == true)
            } else {
                actual == c["expected"]
            };
            if !ok {
                failures.push(format!(
                    "{}: expected {}, got {}",
                    c["source"],
                    c["expected_bool"]
                        .as_bool()
                        .map(|b| json!(b))
                        .unwrap_or_else(|| c["expected"].clone()),
                    actual
                ));
            }
            continue;
        }
        let mut evidence = Evidence::default();
        if let Some(instruction) = c["instruction"].as_str() {
            evidence.instructions.push(Instruction {
                path: c["instruction_path"].as_str().unwrap_or("AGENTS.md").into(),
                text: instruction.into(),
                changed: c["changed"] == true,
            });
        }
        evidence.inputs.push(UserInput {
            text: c["text"].as_str().unwrap().into(),
            source_ref: "goal".into(),
            question: c["question"].as_str().map(str::to_string),
            selected_option: c["selected"] == true,
            option_with_comment: None,
        });
        if let Some(steps) = c["steps"].as_array() {
            for (i, s) in steps.iter().enumerate() {
                evidence.inputs.push(UserInput {
                    text: s.as_str().unwrap().into(),
                    source_ref: format!("step:{i}"),
                    question: None,
                    selected_option: false,
                    option_with_comment: None,
                });
            }
        }
        let (grant, change, revocation) = resolver.consent(role, "review", hint, &evidence);
        if let Some(v) = &grant {
            actual = if c.get("expected_route").is_some() {
                v["family"].clone()
            } else {
                v["matched"].clone()
            };
        }
        let expected = c.get("expected_route").unwrap_or(&c["expected"]);
        let ok = if c["expected_fold"] == true {
            actual.as_str().map(str::to_lowercase) == expected.as_str().map(str::to_lowercase)
        } else {
            actual == *expected
        };
        if !ok {
            failures.push(format!(
                "{}: expected {}, got {} ({})",
                c["source"], expected, actual, c["text"]
            ));
        }
        if c.get("expected_effort").is_some()
            && grant.as_ref().map(|g| &g["effort"]).unwrap_or(&Value::Null) != &c["expected_effort"]
        {
            failures.push(format!(
                "{}: effort expected {}, got {:?}",
                c["source"], c["expected_effort"], grant
            ));
        }
        if c.get("expected_change").is_some() && change.as_ref() != c.get("expected_change") {
            failures.push(format!(
                "{}: change expected {}, got {:?}",
                c["source"], c["expected_change"], change
            ));
        }
        if c.get("expected_revocation").is_some()
            && revocation.is_some() != (c["expected_revocation"] == true)
        {
            failures.push(format!(
                "{}: unexpected revocation {:?}",
                c["source"], revocation
            ));
        }
        if c["expected_ambiguous"] == true
            && !grant
                .as_ref()
                .is_some_and(|g| g["effort_ambiguous"] == true)
        {
            failures.push(format!("{}: missing effort ambiguity", c["source"]));
        }
        if c["expected_revoked"] == true && revocation.is_none() {
            failures.push(format!("{}: missing revocation", c["source"]));
        }
        if c.get("expected_line").is_some()
            && grant.as_ref().map(|g| &g["line"]) != c.get("expected_line")
        {
            failures.push(format!("{}: wrong project line", c["source"]));
        }
        if c.get("expected_purpose").is_some() {
            let (purpose, _, _) = resolver.consent(role, "review", "auto", &evidence);
            if purpose
                .as_ref()
                .map(|g| &g["family"])
                .unwrap_or(&Value::Null)
                != &c["expected_purpose"]
            {
                failures.push(format!(
                    "{}: purpose expected {}, got {:?}",
                    c["source"], c["expected_purpose"], purpose
                ));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} matrix failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert_eq!(fixture["cases"].as_array().unwrap().len(), 898);
}
