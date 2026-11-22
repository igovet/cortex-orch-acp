mod support;
use cortex_core::{
    lifecycle::{
        self, completion::*, domain::*, reconciliation::*, skill_proposals, waits, TaskMachine,
    },
    ownership::OwnerEpoch,
    ports::CommandObservation,
    Store,
};
use serde_json::{json, Value};
use support::*;
fn setup() -> (Project, Store, OwnerEpoch, EvidenceSnapshot) {
    let p = Project::new();
    let s = p.open();
    let o = TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    let e = EvidenceSnapshot {
        project_digest: Some("a".repeat(64)),
        ignored_digest: Some("1".repeat(64)),
        ..Default::default()
    };
    s.write(|db|{
        db.execute("UPDATE runtime_tasks SET state='PLAN_READY',revision=1,plan_json=? WHERE task_id='t'",
            [json!({"verification":[["true"]],"tasks":[]}).to_string()])?;
        execution_baseline_tx(db,"t",&e)?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'work','worker-backend-dev','Work','[]','[\"src\"]','execution','completed','a',?)",
            [json!({"report_ref":"r","report_digest":"f".repeat(64),"checks":[]}).to_string()])?;
        db.execute("INSERT INTO reports VALUES ('r','t','report','r.md','now')",[])?;
        add_review_tx(db,"t",&e)?;
        Ok(())
    }).unwrap();
    (p, s, o, e)
}
fn verify(s: &Store, o: &OwnerEpoch, e: &EvidenceSnapshot, key: &str, code: i32) {
    s.write(|db| {
        let raw: String = db.query_row(
            "SELECT plan_json FROM runtime_tasks WHERE task_id='t'",
            [],
            |r| r.get(0),
        )?;
        let plan: Value = serde_json::from_str(&raw)?;
        let argv = plan["verification"][0]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        retain_check_tx(
            db,
            "t",
            o,
            key,
            &argv,
            &CommandObservation {
                complete: true,
                exit_code: Some(code),
                receipt_ref: key.into(),
            },
            e,
        )?;
        record_verification_tx(db, "t", &[key.into()], e)?;
        Ok(())
    })
    .unwrap();
}
#[test]
fn synthesis_disclosures_conflicts_and_certified_completion() {
    let (_p, s, o, mut e) = setup();
    e.artifacts.insert("r".into(), "f".repeat(64));
    assert_eq!(
        s.write(|db| submit_synthesis_tx(db, "t", &json!({"text":"Too soon"})))
            .unwrap_err()
            .code(),
        "synthesis_not_expected"
    );
    verify(&s, &o, &e, "pass", 0);
    s.write(|db|{db.execute("UPDATE runtime_nodes SET result_json=? WHERE task_id='t'",
        [json!({"report_ref":"r","report_digest":"f".repeat(64),"checks":[{"name":"Live check","outcome":"not_run","blocker":"No owner approval"}]}).to_string()])?;Ok(())}).unwrap();
    assert_eq!(
        s.write(|db| submit_synthesis_tx(db, "t", &json!({"text":"Done"})))
            .unwrap_err()
            .code(),
        "synthesis_gaps_undisclosed"
    );
    let args = json!({"text":"Done","not_verified":[{"item":"Live check","description":"Live behavior","blocker":"Paid call not authorized"}]});
    s.write(|db| {
        submit_synthesis_tx(db, "t", &args)?;
        submit_synthesis_tx(db, "t", &args)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.write(|db| submit_synthesis_tx(
            db,
            "t",
            &json!({"text":"Different","not_verified":args["not_verified"]})
        ))
        .unwrap_err()
        .code(),
        "synthesis_conflict"
    );
    let mut drift = EvidenceSnapshot {
        project_digest: Some("b".repeat(64)),
        artifacts: e.artifacts.clone(),
        ..Default::default()
    };
    assert_eq!(
        s.write(|db| complete_tx(db, "t", &drift))
            .unwrap_err()
            .code(),
        "completion_project_revision_changed"
    );
    drift.artifacts.clear();
    assert_eq!(
        s.write(|db| complete_tx(db, "t", &drift))
            .unwrap_err()
            .code(),
        "completion_artifact_changed_or_missing"
    );
    s.write(|db| complete_tx(db, "t", &e)).unwrap();
}
/// A verification receipt kept only `complete` and `exit_code`, so a timed-out or
/// signalled command reached the coordinator and the owner as a bare "no exit
/// code". The retained cause of the same run now stays with the check.
#[test]
fn a_failed_verification_keeps_the_retained_cause_of_its_run() {
    let (_p, s, o, e) = setup();
    s.write(|db| {
        lifecycle::request_tx(db, "t", "native-verifier-output", "out", "native_command_output",
            &json!({"tool_call_ref":"slow"}),
            || Ok(json!({"tool_call_ref":"slow","observation":{"complete":false,"exit_code":null,"failure":"timeout","signal":9}})))?;
        retain_check_tx(db, "t", &o, "slow", &["true".to_owned()],
            &CommandObservation { complete: false, exit_code: None, receipt_ref: "slow".into() }, &e)?;
        record_verification_tx(db, "t", &["slow".into()], &e)?;
        Ok(())
    })
    .unwrap();
    let failed: Value = serde_json::from_str(
        &s.read(|db| {
            Ok(db.query_row(
                "SELECT payload_json FROM runtime_events WHERE type='VERIFICATION_FAILED'",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap(),
    )
    .unwrap();
    assert_eq!(failed["checks"][0]["failure"], "timeout");
    assert_eq!(failed["checks"][0]["signal"], 9);
    assert_eq!(failed["checks"][0]["exit_code"], Value::Null);
}
#[test]
fn runtime_exact_command_closes_gap_but_unrelated_or_failed_runs_do_not() {
    let (_p, s, o, e) = setup();
    s.write(|db|{db.execute("UPDATE runtime_nodes SET result_json=? WHERE task_id='t'",
        [json!({"checks":[{"name":"same","outcome":"unverified","command":["true"]},{"name":"other","outcome":"not_run","command":["false"]}]}).to_string()])?;Ok(())}).unwrap();
    verify(&s, &o, &e, "pass", 0);
    let open = s.read(|db| gaps(db, "t")).unwrap();
    assert!(!open.contains_key("same"));
    assert!(open.contains_key("other"));
    s.write(|db| {
        lifecycle::event_tx(
            db,
            "t",
            "VERIFICATION_FAILED",
            json!({"revision":1,"checks":[{"argv":["true"],"complete":true,"exit_code":0}]}),
        )?;
        Ok(())
    })
    .unwrap();
    assert!(s.read(|db| gaps(db, "t")).unwrap().contains_key("same"));
}
#[test]
fn localized_single_disclosure_and_heading_refusal() {
    let (_p, s, _, _) = setup();
    let gaps = json!([{"item":"a","description":"Проверка","blocker":"Нет устройства"},{"item":"b","blocker":"Нет устройства"}]);
    s.write(|db| {
        lifecycle::event_tx(
            db,
            "t",
            "NATIVE_TASK_SETTINGS",
            json!({"settings":{"language":"ru"}}),
        )?;
        Ok(())
    })
    .unwrap();
    let body = s.read(|db| disclosed(db, "t", "Готово", &gaps)).unwrap();
    assert_eq!(body.matches("## Не проверено").count(), 1);
    assert_eq!(body.matches("Нет устройства").count(), 1);
    for text in [
        "Done\n## Not verified\nextra",
        "Done\n## **Что не проверено**",
        "Done\n## Не проверено",
        "Done\nUnverified checks\n---",
    ] {
        assert_eq!(
            s.read(|db| disclosed(db, "t", text, &gaps))
                .unwrap_err()
                .code(),
            "synthesis_disclosure_in_text"
        );
    }
    assert!(s
        .read(|db| disclosed(db, "t", "Example\n```md\n## Not verified\n```", &gaps))
        .is_ok());
    assert_eq!(
        s.read(|db| disclosed(db, "t", "Done", &json!([{"item":"a","blocker":" "}])))
            .unwrap_err()
            .code(),
        "synthesis_blocker_required"
    );
}
#[test]
fn verification_repair_is_idempotent_preserves_review_and_dropped_failures() {
    let (_p, s, o, e) = setup();
    verify(&s, &o, &e, "failed", 1);
    let args = json!({"verification":[["true"]],"request_key":"repair"});
    let receipt = s
        .write(|db| submit_verification_tx(db, "t", "coordinator", "repair", &args))
        .unwrap();
    assert_eq!(receipt["state"], "VERIFYING");
    assert_eq!(
        s.write(|db| submit_verification_tx(db, "t", "coordinator", "repair", &args))
            .unwrap(),
        receipt
    );
    assert_eq!(
        s.write(|db| submit_verification_tx(
            db,
            "t",
            "coordinator",
            "repair",
            &json!({"verification":[["false"]]})
        ))
        .unwrap_err()
        .code(),
        "payload_conflict"
    );
    verify(&s, &o, &e, "fixed", 0);
    assert_eq!(
        s.write(|db| submit_verification_tx(db, "t", "coordinator", "late", &args))
            .unwrap_err()
            .code(),
        "verification_repair_not_expected"
    );
}
#[test]
fn reconciling_question_answer_returns_to_planning_and_cannot_skip_certification() {
    let (p, s, o, mut e) = setup();
    verify(&s, &o, &e, "pass", 0);
    let q = s
        .write(|db| {
            skill_proposals::request(
                db,
                &p.0,
                "t",
                "coordinator",
                "q",
                &json!({"question":"Continue?","options":["Explain","Change"],"request_key":"q"}),
            )
        })
        .unwrap();
    assert!(s
        .write(|db| submit_synthesis_tx(db, "t", &json!({"text":"Too soon"})))
        .is_err());
    s.write(|db| {
        assert!(waits::answer_tx(
            db,
            "t",
            q["question_id"].as_str().unwrap(),
            "Explain",
            "text",
            Some("client")
        )?);
        Ok(())
    })
    .unwrap();
    s.write(|db| {
        lifecycle::reconciliation::submit_answer_tx(
            db,
            "t",
            "coordinator",
            "answer",
            &json!({"text":"Explained","request_key":"answer"}),
        )
    })
    .unwrap();
    assert_eq!(
        s.write(|db| complete_tx(db, "t", &e)).unwrap_err().code(),
        "completion_artifact_changed_or_missing"
    );
    e.artifacts.insert("r".into(), "f".repeat(64));
    s.write(|db| complete_tx(db, "t", &e)).unwrap();
}
#[test]
fn released_answer_with_changed_project_fails_without_fabricated_certification() {
    let (p, s, o, e) = setup();
    verify(&s, &o, &e, "pass", 0);
    let q = s
        .write(|db| {
            skill_proposals::request(
                db,
                &p.0,
                "t",
                "coordinator",
                "q",
                &json!({"question":"Explain?","options":["Yes","Change"],"request_key":"q"}),
            )
        })
        .unwrap();
    s.write(|db| {
        waits::answer_tx(
            db,
            "t",
            q["question_id"].as_str().unwrap(),
            "Yes",
            "text",
            Some("client"),
        )?;
        Ok(())
    })
    .unwrap();
    s.write(|db| {
        lifecycle::reconciliation::submit_answer_tx(
            db,
            "t",
            "coordinator",
            "answer",
            &json!({"text":"Explained","request_key":"answer"}),
        )
    })
    .unwrap();
    let changed = EvidenceSnapshot {
        project_digest: Some("b".repeat(64)),
        artifacts: std::collections::BTreeMap::from([("r".into(), "f".repeat(64))]),
        ..Default::default()
    };
    s.write(|db| close_answer_tx(db, "t", &changed)).unwrap();
    let (state, blocker): (String, String) = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT state,blocker FROM runtime_tasks WHERE task_id='t'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap();
    assert_eq!(state, "FAILED");
    assert_eq!(
        blocker,
        "answer_not_certified:completion_project_revision_changed"
    );
}
#[test]
fn shortening_verification_retains_removed_failure() {
    let (_p, s, _, _) = setup();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='PLANNING',plan_json=? WHERE task_id='t'",
            [json!({"verification":[["false"],["true"]]}).to_string()],
        )?;
        lifecycle::event_tx(
            db,
            "t",
            "VERIFICATION_FAILED",
            json!({"revision":1,"checks":[{"argv":["false"],"exit_code":1,"complete":true}]}),
        )?;
        submit_verification_tx(
            db,
            "t",
            "coordinator",
            "shorten",
            &json!({"verification":[["true"]],"request_key":"shorten"}),
        )?;
        assert_eq!(dropped_failures_tx(db, "t")?, vec![json!(["false"])]);
        Ok(())
    })
    .unwrap();
}
fn failed_cargo_test(s: &Store) {
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='PLANNING',plan_json=? WHERE task_id='t'",
            [json!({"verification":[["cargo","test"]],"tasks":[]}).to_string()],
        )?;
        lifecycle::event_tx(
            db,
            "t",
            "VERIFICATION_FAILED",
            json!({"revision":1,"checks":[{"argv":["cargo","test"],"exit_code":101,"complete":true}]}),
        )?;
        Ok(())
    })
    .unwrap();
}
#[test]
fn equal_length_trivial_replacement_of_a_failed_check_is_refused() {
    let (_p, s, _, _) = setup();
    failed_cargo_test(&s);
    for argv in [
        vec!["true"],
        vec![":"],
        vec!["echo", "ok"],
        vec!["sh", "-c", "exit 0"],
    ] {
        let err = s
            .write(|db| {
                submit_verification_tx(
                    db,
                    "t",
                    "coordinator",
                    "swap",
                    &json!({"verification":[argv],"request_key":"swap"}),
                )
            })
            .unwrap_err();
        assert_eq!(err.code(), "verification_trivial_replacement", "{argv:?}");
    }
    let (state, revised): (String, i64) = s
        .read(|db| {
            Ok((
                db.query_row(
                    "SELECT state FROM runtime_tasks WHERE task_id='t'",
                    [],
                    |r| r.get(0),
                )?,
                db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='VERIFICATION_REVISED'",
                    [],
                    |r| r.get(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!((state.as_str(), revised), ("PLANNING", 0));
}

#[test]
fn failed_verification_survives_multiple_plan_revisions_until_exact_pass() {
    let (_p, s, _owner, _e) = setup();
    let argv = json!(["cargo", "test", "-p", "fixture"]);
    let plan_with = json!({
        "tasks":[{"id":"repair","role":"backend-dev","objective":"Repair the failing check","dependencies":[],"scope":["src"]}],
        "verification":[argv.clone()]
    });
    let plan_without = json!({
        "tasks":[{"id":"repair-again","role":"backend-dev","objective":"Continue the repair","dependencies":[],"scope":["src"]}],
        "verification":[["cargo","test","-p","fixture-unit"]]
    });

    // The first revision records a real failed command.
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLANNING',revision=1", [])?;
        lifecycle::event_tx(
            db,
            "t",
            "VERIFICATION_FAILED",
            json!({
                "revision":1,
                "checks":[{"argv":argv,"exit_code":101,"complete":true,"receipt_ref":"failed-r1"}]
            }),
        )?;
        Ok(())
    })
    .unwrap();

    // A repair plan that keeps the exact command is accepted.
    s.write(|db| lifecycle::plan::accept_tx(db, "t", "coordinator", "repair-r1", &plan_with))
        .unwrap();

    // Another replan before verification cannot make the previous revision's failure disappear.
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let err = s
        .write(|db| lifecycle::plan::accept_tx(db, "t", "coordinator", "drop-r2", &plan_without))
        .unwrap_err();
    assert_eq!(err.code(), "verification_failed_check_missing");

    // The exact command later passes, allowing a subsequent plan to remove it.
    s.write(|db| {
        let revision: i64 = db.query_row(
            "SELECT revision FROM runtime_tasks WHERE task_id='t'",
            [],
            |r| r.get(0),
        )?;
        lifecycle::event_tx(
            db,
            "t",
            "VERIFICATION_PASSED",
            json!({
                "revision":revision,
                "checks":[{"argv":argv,"exit_code":0,"complete":true,"receipt_ref":"passed-r2"}]
            }),
        )?;
        db.execute("UPDATE runtime_tasks SET state='PLANNING'", [])?;
        Ok(())
    })
    .unwrap();
    s.write(|db| {
        lifecycle::plan::accept_tx(db, "t", "coordinator", "drop-after-pass", &plan_without)
    })
    .unwrap();
}
#[test]
fn a_legacy_failed_run_keeps_commands_without_receipts() {
    let (_project, store, _, _) = setup();
    let plan = json!({"tasks":[{"id":"repair","role":"backend-dev","objective":"Repair","dependencies":[],"scope":["src"]}],
        "verification":[["cargo","check"],["cargo","test"]]});
    store.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'", [])?;
        let accepted = lifecycle::plan::accept_tx(db,"t","coordinator","initial",&plan)?;
        let revision = &accepted["revision"];
        lifecycle::event_tx(db,"t","VERIFICATION_FAILED",json!({"revision":revision,"revision_digest":"d","review_event":7,
            "checks":[{"argv":["cargo","check"],"exit_code":0,"complete":true,"revision":revision,"revision_digest":"d","review_event":7}]}))?;
        db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'", [])?;
        Ok(())
    }).unwrap();
    let mut shortened = plan.clone();
    shortened["verification"] = json!([["cargo", "check"]]);
    let error = store
        .write(|db| lifecycle::plan::accept_tx(db, "t", "coordinator", "drop", &shortened))
        .unwrap_err();
    assert_eq!(error.code(), "verification_failed_check_missing");
    store
        .write(|db| lifecycle::plan::accept_tx(db, "t", "coordinator", "keep", &plan))
        .unwrap();
}

#[test]
fn equal_length_nontrivial_replacement_records_the_dropped_failure() {
    let (_p, s, _, _) = setup();
    failed_cargo_test(&s);
    s.write(|db| {
        submit_verification_tx(
            db,
            "t",
            "coordinator",
            "swap",
            &json!({"verification":[["cargo","check"]],"request_key":"swap"}),
        )?;
        assert_eq!(
            dropped_failures_tx(db, "t")?,
            vec![json!(["cargo", "test"])]
        );
        Ok(())
    })
    .unwrap();
}
/// The trivial-command detector is a denylist and can be bypassed. The policy does not
/// rest on it: whatever replaces a failed check is an open obligation, and completion
/// stays refused until the owner answers or the exact failed command passes.
#[test]
fn any_replacement_of_a_failed_check_needs_the_owner_to_complete() {
    for argv in [
        vec!["sh", "-o", "pipefail", "-c", "true"],
        vec!["bash", "-O", "extglob", "-c", "true"],
        vec!["ruby", "-e", "1"],
        vec!["cmp", "/dev/null", "/dev/null"],
        vec!["expr", "1"],
        vec!["perl", "-e", "1"],
        vec!["xargs", "true"],
        vec!["busybox", "true"],
        vec!["cargo", "check"],
    ] {
        let (_p, s, o, mut e) = setup();
        failed_cargo_test(&s);
        s.write(|db| {
            db.execute("UPDATE runtime_nodes SET status='completed'", [])?;
            submit_verification_tx(
                db,
                "t",
                "coordinator",
                "swap",
                &json!({"verification":[argv.clone()],"request_key":"swap"}),
            )
            .map(|_| ())
            .or_else(|e| {
                if e.code() == "verification_trivial_replacement" {
                    Ok(())
                } else {
                    Err(e)
                }
            })?;
            Ok(())
        })
        .unwrap();
        let refused = s
            .read(|db| {
                Ok(db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='VERIFICATION_REVISED'",
                    [],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .unwrap()
            == 0;
        if refused {
            continue; // the extra trivial-command signal caught it
        }
        // Accepted: the failed command is recorded as dropped ...
        assert_eq!(
            s.write(|db| dropped_failures_tx(db, "t")).unwrap(),
            vec![json!(["cargo", "test"])],
            "{argv:?}"
        );
        // ... so a passing run of the replacement cannot complete the task.
        e.artifacts.insert("r".into(), "f".repeat(64));
        s.write(|db| {
            retain_check_tx(
                db,
                "t",
                &o,
                "run",
                &argv.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                &CommandObservation {
                    complete: true,
                    exit_code: Some(0),
                    receipt_ref: "run".into(),
                },
                &e,
            )?;
            record_verification_tx(db, "t", &["run".into()], &e)?;
            db.execute(
                "UPDATE runtime_tasks SET synthesis='Done' WHERE task_id='t'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            s.write(|db| complete_tx(db, "t", &e)).unwrap_err().code(),
            "completion_dropped_failed_check",
            "{argv:?}"
        );
    }
}
#[test]
fn replanning_budget_scope_policy_and_escalation_injection() {
    let (_p, s, o, e) = setup();
    verify(&s, &o, &e, "pass", 0);
    let plan = json!({"tasks":[{"id":"next","role":"backend-dev","objective":"Continue","dependencies":[],"scope":["src"]}],"verification":[["cargo","test"]]});
    for n in 0..3 {
        let op = if n == 0 {
            "escalation_request"
        } else {
            "replan_request"
        };
        let proposal = json!({"reason":"Need independent diagnosis","report_ids":["r_missing"],"request_key":format!("r{n}")});
        s.write(|db| {
            let receipt =
                propose_replan_tx(db, "t", "coordinator", &format!("r{n}"), op, &proposal)?;
            assert_eq!(
                propose_replan_tx(db, "t", "coordinator", &format!("r{n}"), op, &proposal)?,
                receipt
            );
            settle_recovery_tx(db, "t", cortex_core::TaskState::Planning)?;
            lifecycle::plan::accept_tx(db, "t", "coordinator", &format!("p{n}"), &plan)?;
            if n == 0 {
                // plan_json and PLAN_ACCEPTED carry the inserted node and its edges, so
                // the plan agrees with the runtime node list.
                let stored: Value = serde_json::from_str(&db.query_row(
                    "SELECT plan_json FROM runtime_tasks", [], |r| r.get::<_, String>(0))?).unwrap();
                let accepted: Value = serde_json::from_str(&db.query_row(
                    "SELECT payload_json FROM runtime_events WHERE type='PLAN_ACCEPTED' ORDER BY id DESC LIMIT 1",
                    [], |r| r.get::<_, String>(0))?).unwrap();
                assert_eq!(stored["tasks"], accepted["plan"]["tasks"]);
                let tasks = stored["tasks"].as_array().unwrap();
                assert_eq!(tasks[0]["id"], "_consultation");
                assert_eq!(tasks[1]["dependencies"], json!(["_consultation"]));
                assert_eq!(
                    db.query_row("SELECT count(*) FROM runtime_nodes WHERE revision=(SELECT revision FROM runtime_tasks)", [], |r| r.get::<_, i64>(0))?,
                    tasks.len() as i64
                );
            }
            db.execute(
                "UPDATE runtime_nodes SET status='completed' WHERE task_id='t'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    }
    // No replan limit: the fourth request changes approach to a senior consultation.
    assert_eq!(
        s.write(|db| propose_replan_tx(
            db,
            "t",
            "coordinator",
            "r4",
            "scope_request",
            &json!({"scope":["src"]})
        ))
        .unwrap()["decision"],
        "senior_consultation"
    );
    s.write(|db| {
        lifecycle::event_tx(
            db,
            "t",
            "USER_STEERING",
            json!({"deny_write":["protected"]}),
        )?;
        Ok(())
    })
    .unwrap();
    for scope in [".git/config", "../escape", "a\\b", "protected/file"] {
        assert!(s
            .write(|db| propose_replan_tx(
                db,
                "t",
                "coordinator",
                "invalid",
                "scope_request",
                &json!({"scope":[scope]})
            ))
            .is_err());
    }
    s.write(|db| {
        propose_replan_tx(
            db,
            "t",
            "coordinator",
            "allowed",
            "scope_request",
            &json!({"scope":["src"]}),
        )
    })
    .unwrap();
    assert_eq!(
        s.read(|db| Ok(db.query_row(
            "SELECT count(*) FROM runtime_nodes WHERE node_id='_consultation'",
            [],
            |r| r.get::<_, i64>(0)
        )?))
        .unwrap(),
        1
    );
    let packet = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT payload_json FROM runtime_events WHERE type='CONSULTATION_REQUIRED'",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap();
    assert!(packet.contains("unavailable"));
}
