//! Owner-stop idleness, runtime-created node routing, review independence, a
//! reviewer's failed-check finding and runtime-composed synthesis, on real
//! format-12 SQLite transactions.
mod support;
use cortex_core::{
    lifecycle::{completion::*, domain::*, plan, reconciliation, review, *},
    ownership::OwnerEpoch,
    ports::CommandObservation,
    CoreResult, Store, TaskState,
};
use rusqlite::Transaction;
use serde_json::{json, Value};
use support::*;

fn evidence() -> EvidenceSnapshot {
    EvidenceSnapshot {
        project_digest: Some("b".repeat(64)),
        head: Some("head".into()),
        ..Default::default()
    }
}
fn state(s: &Store) -> String {
    s.read(|db| {
        Ok(db.query_row(
            "SELECT state FROM runtime_tasks WHERE task_id='t'",
            [],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}
fn bind(db: &Transaction<'_>, node: &str, a: &str, attempt: &str, channel: &str) -> CoreResult<()> {
    db.execute("UPDATE runtime_nodes SET status='running',assignment_ref=? WHERE task_id='t' AND node_id=?", (a, node))?;
    db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,channel_id,engine_session_ref,owner_epoch,created_at,updated_at)
        VALUES (?,'t',?,'worker-backend-dev','ready','ready',?,?,1,'now','now')", (a, attempt, channel, format!("es-{attempt}")))?;
    db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,attempt_id,owner_epoch,issued_at)
        VALUES (?,'fixture','t','thread','worker',?,1,'now')", (channel, attempt))?;
    Ok(())
}
fn publish(db: &Transaction<'_>, a: &str, report: &str) -> CoreResult<()> {
    db.execute(
        "INSERT INTO reports VALUES (?,'t','report',?,'now')",
        (report, format!("{report}.md")),
    )?;
    db.execute("INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at)
        VALUES ('t','worker_result',?,?,?,'runtime','now')", (a, format!("result:{a}"), json!({"report_ref":report}).to_string()))?;
    Ok(())
}
fn routed(db: &Transaction<'_>, node: &str, a: &str, model: &str, warning: bool) -> CoreResult<()> {
    let mut decision = json!({});
    if warning {
        decision["independence"] = json!({"status":"warning","warning":"same model"});
    }
    event_tx(
        db,
        "t",
        "NATIVE_ROUTE_SELECTED",
        json!({"assignment_ref":a,"node":node,"revision":1,"route_engine":"codex","model":model,"decision":decision}),
    )?;
    Ok(())
}

#[test]
fn failed_non_review_specialist_handoff_keeps_exact_blocker_for_coordinator() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='t'",
            [],
        )?;
        db.execute(
            "INSERT INTO runtime_nodes VALUES ('t',1,'build','worker-backend-dev',?,'[]','[\"src\"]','execution','failed','a1',?)",
            (
                "Implement the backend change",
                json!({
                    "report_ref":"report-build-1",
                    "summary":"The provider refused the required deployment permission.",
                    "blocker":"owner approval is required for deployment"
                })
                .to_string(),
            ),
        )?;
        assert!(return_to_coordinator_tx(db, "t", "worker_failed")?);
        let raw: String = db.query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id='t' AND type='RETURNED_TO_COORDINATOR' ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )?;
        let payload: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(payload["reason"], "worker_failed");
        assert_eq!(payload["handoff"]["role"], "worker-backend-dev");
        assert_eq!(payload["handoff"]["report_ref"], "report-build-1");
        assert_eq!(
            payload["handoff"]["blocker"],
            "owner approval is required for deployment"
        );
        Ok(())
    })
    .unwrap();
}

/// A one-node plan whose node carries explicit routing fields, already executed.
fn executed() -> (Project, Store, EvidenceSnapshot, OwnerEpoch) {
    let p = Project::new();
    let s = p.open();
    let owner = TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    let mut e = evidence();
    e.artifacts.insert("r".into(), "f".repeat(64));
    let plan = json!({"tasks":[{"id":"n","role":"backend-dev","objective":"Work","dependencies":[],"scope":["src"],
        "complexity":"high","work_kind":"debugging","provider":"codex-acp","model":"gpt-x","model_reason":"hard"}],
        "verification":[["cargo","test"]]});
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLAN_READY',revision=1,plan_json=? WHERE task_id='t'", [plan.to_string()])?;
        execution_baseline_tx(db, "t", &EvidenceSnapshot { project_digest: Some("a".repeat(64)), ignored_digest: Some("i".repeat(64)), ..Default::default() })?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)", [])?;
        bind(db, "n", "a", "xa", "c")?;
        publish(db, "a", "r")?;
        record_result_tx(db, "t", "a", &json!({"report_ref":"r","summary":"done","checks":[]}), &e)?;
        routed(db, "n", "a", "gpt-x", false)?;
        add_review_tx(db, "t", &e)
    })
    .unwrap();
    (p, s, e, owner)
}
fn submit_review(s: &Store, e: &mut EvidenceSnapshot, result: Value) -> CoreResult<()> {
    e.artifacts.insert("review".into(), "e".repeat(64));
    let e: &EvidenceSnapshot = e;
    s.write(|db| {
        bind(db, "_review_1", "ar", "xr", "cr")?;
        start_review_tx(db, "t", "ar", e)?;
        publish(db, "ar", "review")?;
        record_result_tx(db, "t", "ar", &result, e)
    })
}
fn plan_json(s: &Store) -> Value {
    s.read(|db| {
        Ok(db.query_row(
            "SELECT plan_json FROM runtime_tasks WHERE task_id='t'",
            [],
            |r| r.get::<_, String>(0),
        )?)
    })
    .map(|p| serde_json::from_str(&p).unwrap())
    .unwrap()
}

#[test]
fn review_and_fix_nodes_are_routed_with_stored_fields_not_defaults() {
    let (_p, s, mut e, _o) = executed();
    assert_eq!(
        plan::node_routing(&plan_json(&s), "_review_1"),
        json!({"work_kind":"review","complexity":"high"})
    );
    submit_review(
        &s,
        &mut e,
        json!({"report_ref":"review","summary":"Needs work","checks":[],"verdict":"REQUEST_CHANGES",
        "findings":[{"node":"n","cross_cutting":false,"files":["src/a.rs"],"fix":"Handle empty"}]}),
    )
    .unwrap();
    s.write(|db| review::advance_tx(db, "t")).unwrap();
    assert_eq!(
        plan::node_routing(&plan_json(&s), "_fix_1_n"),
        json!({"complexity":"high","work_kind":"debugging","provider":"codex-acp","model":"gpt-x","model_reason":"hard"})
    );
    assert_eq!(state(&s), "EXECUTING");
}

#[test]
fn request_changes_citing_a_failed_check_starts_a_fix_cycle() {
    let (_p, s, mut e, _o) = executed();
    submit_review(&s, &mut e, json!({"report_ref":"review","summary":"The test fails","verdict":"REQUEST_CHANGES",
        "checks":[{"name":"tests","outcome":"failed","tool_call_ref":"native-command:1"}],
        "findings":[{"node":"n","cross_cutting":false,"files":["src/a.rs"],"fix":"Make the test pass"}]})).unwrap();
    let status: String = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT status FROM runtime_nodes WHERE node_id='_review_1'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(status, "completed");
    s.write(|db| review::advance_tx(db, "t")).unwrap();
    assert_eq!(state(&s), "EXECUTING");
    assert!(s
        .read(|db| Ok(db.query_row(
            "SELECT count(*) FROM runtime_nodes WHERE node_id='_fix_1_n'",
            [],
            |r| r.get::<_, i64>(0)
        )? == 1))
        .unwrap());
}

#[test]
fn an_approval_or_explicit_failure_citing_a_failed_check_still_fails_the_review() {
    for result in [
        json!({"report_ref":"review","summary":"ok","verdict":"APPROVE","checks":[{"name":"tests","outcome":"failed","tool_call_ref":"x"}]}),
        json!({"report_ref":"review","summary":"broken","verdict":"REQUEST_CHANGES","assignment_outcome":"failed","checks":[],
            "blocker":{"kind":"tool_unavailable","detail":"The project's test runner is not installed on this host."}}),
    ] {
        let (_p, s, mut e, _o) = executed();
        submit_review(&s, &mut e, result).unwrap();
        let status: String = s
            .read(|db| {
                Ok(db.query_row(
                    "SELECT status FROM runtime_nodes WHERE node_id='_review_1'",
                    [],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(status, "failed");
    }
}

fn review_on(model: &str, warning: bool) -> (Project, Store, CoreResult<()>) {
    let (p, s, mut e, _o) = executed();
    s.write(|db| {
        bind(db, "_review_1", "ar", "xr", "cr")?;
        routed(db, "_review_1", "ar", model, warning)
    })
    .unwrap();
    e.artifacts.insert("review".into(), "e".repeat(64));
    let result = s.write(|db| {
        start_review_tx(db, "t", "ar", &e)?;
        publish(db, "ar", "review")?;
        record_result_tx(
            db,
            "t",
            "ar",
            &json!({"report_ref":"review","summary":"ok","checks":[],"verdict":"APPROVE"}),
            &e,
        )
    });
    (p, s, result)
}

#[test]
fn reviewer_on_the_reviewed_model_is_refused_unless_routing_found_no_other() {
    // Same model, no recorded warning: refused.
    let (_p, _s, result) = review_on("gpt-x", false);
    assert_eq!(result.unwrap_err().code(), "review_not_independent");
    // Same model with the explicit independence warning: proceeds and is visible.
    let (_p, s, result) = review_on("gpt-x", true);
    result.unwrap();
    let (stored, warnings): (String, i64) = s
        .read(|db| {
            Ok((
                db.query_row(
                    "SELECT result_json FROM runtime_nodes WHERE node_id='_review_1'",
                    [],
                    |r| r.get(0),
                )?,
                db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='REVIEW_INDEPENDENCE_WARNING'",
                    [],
                    |r| r.get(0),
                )?,
            ))
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&stored).unwrap()["review_independence"]["status"],
        "warning"
    );
    assert_eq!(warnings, 1);
    // A different model is recorded as independent.
    let (_p, s, result) = review_on("claude-y", false);
    result.unwrap();
    let stored: String = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT result_json FROM runtime_nodes WHERE node_id='_review_1'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&stored).unwrap()["review_independence"]["status"],
        "independent"
    );
}

fn plan_with_stopped_node() -> (Project, Store) {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1,plan_json='{}' WHERE task_id='t'", [])?;
        for (id, deps, status, a) in [("stopped", "[]", "running", "as"), ("sibling", "[]", "running", "ab"), ("after", "[\"stopped\"]", "pending", "")] {
            db.execute("INSERT INTO runtime_nodes VALUES ('t',1,?,'worker-backend-dev','Work',?,'[\"src\"]','execution',?,?,NULL)",
                (id, deps, status, if a.is_empty() { None } else { Some(a) }))?;
        }
        stop_assignment_tx(db, "t", "as", "owner stopped it")
    })
    .unwrap();
    (p, s)
}

#[test]
fn owner_stopped_work_is_idle_only_when_nothing_else_can_run() {
    let (_p, s) = plan_with_stopped_node();
    let idle = |s: &Store| {
        s.write(|db| owner_stopped_work_is_idle_tx(db, "t"))
            .unwrap()
    };
    // A running sibling keeps the plan going.
    assert!(!idle(&s));
    s.write(|db| {
        Ok(db.execute(
            "UPDATE runtime_nodes SET status='completed' WHERE node_id='sibling'",
            [],
        )?)
    })
    .unwrap();
    // The dependent of the stopped node never becomes runnable.
    assert!(idle(&s));
    // An independent pending node still can run.
    s.write(|db| Ok(db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'free','worker-backend-dev','Work','[\"sibling\"]','[\"src\"]','execution','pending',NULL,NULL)", [])?)).unwrap();
    assert!(!idle(&s));
}

#[test]
fn a_cancellation_made_by_a_return_is_not_an_owner_stop_when_the_coordinator_resumes_the_revision()
{
    let (_p, s) = plan_with_stopped_node();
    s.write(|db| {
        Ok(db.execute(
            "UPDATE runtime_nodes SET status='completed' WHERE node_id='sibling'",
            [],
        )?)
    })
    .unwrap();
    assert!(s
        .write(|db| owner_stopped_work_is_idle_tx(db, "t"))
        .unwrap());
    // The runtime returns to the coordinator for the owner stop, which cancels what is left.
    s.write(|db| {
        assert!(return_to_coordinator_tx(
            db,
            "t",
            "worker_stopped_by_owner"
        )?);
        // The coordinator settles back into the same revision without replanning.
        settle_recovery_tx(db, "t", TaskState::Planning)?;
        db.execute(
            "UPDATE runtime_tasks SET state='EXECUTING' WHERE task_id='t'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    // The node cancelled by that return, and the old stop, are consumed: no repeated return.
    assert!(!s.write(|db| owner_stop_unhandled_tx(db, "t")).unwrap());
    assert!(!s
        .write(|db| owner_stopped_work_is_idle_tx(db, "t"))
        .unwrap());
    // A runtime-only cancellation alone never counts as an owner stop.
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1,plan_json='{}' WHERE task_id='t'", [])?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','cancelled','a',NULL)", [])?;
        Ok(())
    })
    .unwrap();
    assert!(!s
        .write(|db| owner_stopped_work_is_idle_tx(db, "t"))
        .unwrap());
}

#[test]
fn a_stopped_workers_open_question_is_closed_visibly_and_none_survives_the_return() {
    let (_p, s) = plan_with_stopped_node();
    // The sibling asks the owner a question, then the owner stops that worker too.
    s.write(|db| {
        db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,channel_id,engine_session_ref,owner_epoch,created_at,updated_at)
            VALUES ('ab','t','xb','worker-backend-dev','ready','ready','cb','es-xb',1,'now','now')", [])?;
        db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,attempt_id,owner_epoch,issued_at)
            VALUES ('cb','fixture','t','thread','worker','xb',1,'now')", [])?;
        cortex_core::lifecycle::waits::request_tx_bound(db, "t", "cb", "q", &[cortex_core::lifecycle::waits::QuestionForm {
            question: "Which way?".into(), options: vec!["A".into(), "B".into()] }])?;
        Ok(())
    })
    .unwrap();
    let open = |s: &Store| {
        s.read(|db| {
            Ok(db.query_row(
                "SELECT count(*) FROM pending_questions WHERE status='open'",
                [],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap()
    };
    assert_eq!(open(&s), 1);
    s.write(|db| stop_assignment_tx(db, "t", "ab", "owner stopped it"))
        .unwrap();
    assert_eq!(open(&s), 0);
    let status: String = s
        .read(|db| Ok(db.query_row("SELECT status FROM pending_questions", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(status, "cancelled");
    s.write(|db| Ok(return_to_coordinator_tx(db, "t", "worker_stopped_by_owner").map(|_| ())?))
        .unwrap();
    assert_eq!(open(&s), 0);
}

#[test]
fn the_owner_stop_blocker_is_a_work_stop_the_coordinator_is_asked_about() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1,plan_json='{}' WHERE task_id='t'", [])?;
        assert_eq!(stop_tx(db, "t", "worker_stopped_by_owner")?, TaskState::Recovering);
        Ok(())
    }).unwrap();
    let asked: String = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT payload_json FROM runtime_events WHERE type='ASK_USER_REQUIRED'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert!(asked.contains("worker_stopped_by_owner"));
}

/// Executed, approved by another model, and verified: RECONCILING with a baseline manifest.
fn reconciling(
    summary: &str,
    current: &[(&str, &str)],
) -> (Project, Store, std::collections::BTreeMap<String, String>) {
    let (p, s, mut e, o) = executed();
    s.write(|db| {
        db.execute("UPDATE runtime_nodes SET result_json=json_set(result_json,'$.summary',?) WHERE node_id='n'", [summary])?;
        let mut before = std::collections::BTreeMap::new();
        before.insert("src/a.rs".to_owned(), "1".repeat(64));
        before.insert("README.md".to_owned(), "2".repeat(64));
        reconciliation::record_baseline_manifest_tx(db, "t", &"a".repeat(64), &before)
    })
    .unwrap();
    s.write(|db| {
        bind(db, "_review_1", "ar", "xr", "cr")?;
        routed(db, "_review_1", "ar", "claude-y", false)
    })
    .unwrap();
    e.artifacts.insert("review".into(), "e".repeat(64));
    s.write(|db| {
        start_review_tx(db, "t", "ar", &e)?;
        publish(db, "ar", "review")?;
        record_result_tx(
            db,
            "t",
            "ar",
            &json!({"report_ref":"review","summary":"ok","checks":[],"verdict":"APPROVE"}),
            &e,
        )?;
        review::advance_tx(db, "t")?;
        let obs = CommandObservation {
            complete: true,
            exit_code: Some(0),
            receipt_ref: "fake:1".into(),
        };
        retain_check_tx(
            db,
            "t",
            &o,
            "check",
            &["cargo".into(), "test".into()],
            &obs,
            &e,
        )?;
        assert!(record_verification_tx(db, "t", &["check".into()], &e)?);
        Ok(())
    })
    .unwrap();
    assert_eq!(state(&s), "RECONCILING");
    let current = current
        .iter()
        .map(|(k, v)| (k.to_string(), v.repeat(64)))
        .collect();
    (p, s, current)
}

#[test]
fn runtime_synthesis_quotes_execution_summaries_lists_changed_files_and_records_its_event() {
    let (_p, s, current) = reconciling(
        "Fixed   the *parser*\n`bug`",
        &[("src/a.rs", "9"), ("README.md", "2"), ("src/new.rs", "3")],
    );
    let text = s
        .write(|db| {
            reconciliation::compose_runtime_synthesis_tx(
                db,
                "t",
                Some(&current),
                Some(&"i".repeat(64)),
            )
        })
        .unwrap()
        .unwrap();
    // Review prose is not quoted; whitespace is flattened and Markdown escaped.
    assert!(
        text.starts_with("The work is done.\n\nWork summaries"),
        "{text}"
    );
    assert!(
        text.contains("- Fixed the \\*parser\\* \\`bug\\`"),
        "{text}"
    );
    assert!(!text.contains("\nok\n"));
    // README.md is unchanged from the baseline; the other two files changed.
    assert!(
        text.contains("- `src/a.rs`")
            && text.contains("- `src/new.rs`")
            && !text.contains("README.md"),
        "{text}"
    );
    let (composed, submitted, saved): (i64, i64, Option<String>) = s
        .read(|db| {
            Ok((
                db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='SYNTHESIS_RUNTIME_COMPOSED'",
                    [],
                    |r| r.get(0),
                )?,
                db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='SYNTHESIS_SUBMITTED'",
                    [],
                    |r| r.get(0),
                )?,
                db.query_row("SELECT synthesis FROM runtime_tasks", [], |r| r.get(0))?,
            ))
        })
        .unwrap();
    assert_eq!(
        (composed, submitted, saved.as_deref()),
        (1, 0, Some(text.as_str()))
    );
    // Already composed text is never composed again.
    assert!(s
        .write(|db| reconciliation::compose_runtime_synthesis_tx(
            db,
            "t",
            Some(&current),
            Some(&"i".repeat(64))
        ))
        .unwrap()
        .is_none());
}

#[test]
fn runtime_synthesis_labels_follow_the_chat_language() {
    let (_p, s, current) = reconciling("Готово", &[("src/a.rs", "9")]);
    s.write(|db| {
        Ok(db.execute(
            "INSERT INTO task_language(task_id,language) VALUES ('t','ru')",
            [],
        )?)
    })
    .unwrap();
    let text = s
        .write(|db| {
            reconciliation::compose_runtime_synthesis_tx(
                db,
                "t",
                Some(&current),
                Some(&"i".repeat(64)),
            )
        })
        .unwrap()
        .unwrap();
    assert!(
        text.starts_with("Работа выполнена.") && text.contains("Изменённые файлы:"),
        "{text}"
    );
}

#[test]
fn runtime_synthesis_falls_back_to_the_coordinator_when_a_bound_or_baseline_is_missing() {
    let compose = |s: &Store, current: &std::collections::BTreeMap<String, String>| {
        s.write(|db| {
            reconciliation::compose_runtime_synthesis_tx(
                db,
                "t",
                Some(current),
                Some(&"i".repeat(64)),
            )
        })
        .unwrap()
    };
    let (_p, s, c) = reconciling(&"word ".repeat(400), &[("src/a.rs", "9")]);
    assert!(compose(&s, &c).is_none());
    let (_p, s, c) = reconciling("  ", &[("src/a.rs", "9")]);
    assert!(compose(&s, &c).is_none());
    let (_p, s, c) = reconciling("Done\u{202e}gnp.exe", &[("src/a.rs", "9")]);
    assert!(compose(&s, &c).is_none());
    for name in [
        "we`ird.rs",
        "line\u{2028}break.rs",
        "bell\u{7}.rs",
        "zero\u{200b}width.rs",
    ] {
        let (_p, s, c) = reconciling("Done", &[("src/a.rs", "9"), (name, "4")]);
        assert!(compose(&s, &c).is_none(), "{name:?}");
    }
    let many: Vec<(String, String)> = (0..2000)
        .map(|i| {
            (
                format!("src/generated_file_with_a_long_name_{i:05}.rs"),
                "5".into(),
            )
        })
        .collect();
    let many: Vec<(&str, &str)> = many.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let (_p, s, c) = reconciling("Done", &many);
    assert!(compose(&s, &c).is_none());
    // No trustworthy manifest (incomplete or metadata-only observation): the coordinator.
    let (_p, s, _c) = reconciling("Done", &[("src/a.rs", "9")]);
    assert!(s
        .write(|db| reconciliation::compose_runtime_synthesis_tx(
            db,
            "t",
            None,
            Some(&"i".repeat(64))
        ))
        .unwrap()
        .is_none());
    // Ignored files changed (or could not be observed): they are not deliverable, so
    // the deliverable list stays complete and they are not listed.
    for ignored in [Some("j".repeat(64)), None] {
        let (_p, s, c) = reconciling("Done", &[("src/a.rs", "9")]);
        let text = s
            .write(|db| {
                reconciliation::compose_runtime_synthesis_tx(db, "t", Some(&c), ignored.as_deref())
            })
            .unwrap()
            .expect("the deliverable change set is complete");
        assert!(text.contains("- `src/a.rs`"), "{text}");
    }
    let (_p, s, c) = reconciling("Done", &[("src/a.rs", "9")]);
    s.write(|db| Ok(db.execute("DELETE FROM attempt_revisions", [])?))
        .unwrap();
    assert!(compose(&s, &c).is_none());
    let (_p, s, c) = reconciling("Done", &[("src/a.rs", "9")]);
    s.write(|db| Ok(db.execute("UPDATE attempt_revisions SET dirty_digest='x'", [])?))
        .unwrap();
    assert!(compose(&s, &c).is_none());
}

#[test]
fn a_waived_review_or_a_repair_keeps_the_coordinator() {
    for event in [
        "REVIEW_NOT_REQUIRED",
        "VERIFICATION_FAILED",
        "REVIEW_CHANGES_REQUESTED",
        "REPLAN_REQUEST_ACCEPTED",
        "RETURNED_TO_COORDINATOR",
    ] {
        let (_p, s, c) = reconciling("Done", &[("src/a.rs", "9")]);
        s.write(|db| Ok(event_tx(db, "t", event, json!({}))?))
            .unwrap();
        assert!(
            s.write(|db| reconciliation::compose_runtime_synthesis_tx(
                db,
                "t",
                Some(&c),
                Some(&"i".repeat(64))
            ))
            .unwrap()
            .is_none(),
            "{event}"
        );
    }
}

#[test]
fn escalation_is_scoped_to_the_plan_revision_that_dispatched_the_node() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    let previous = |node: &str, revision: i64| {
        s.read(|db| cortex_core::routing::previous_dispatch_engine(db, "t", node, revision))
            .unwrap()
    };
    s.write(|db| {
        event_tx(
            db,
            "t",
            "NATIVE_ROUTE_SELECTED",
            json!({"node":"work","revision":1,"engine":"codex-acp"}),
        )?;
        event_tx(
            db,
            "t",
            "NATIVE_ROUTE_SELECTED",
            json!({"node":"legacy","engine":"codex-acp"}),
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(previous("work", 1).as_deref(), Some("codex-acp"));
    assert_eq!(previous("work", 2), None);
    assert_eq!(previous("_review_1", 2), None);
    assert_eq!(previous("other", 1), None);
    assert_eq!(previous("legacy", 1), None);
}

#[test]
fn unverifiable_independence_is_a_visible_warning_and_the_attempt_guard_is_hard() {
    // No route recorded for the reviewer (for example a task in flight across an upgrade).
    let (_p, s, mut e, _o) = executed();
    submit_review(
        &s,
        &mut e,
        json!({"report_ref":"review","summary":"ok","checks":[],"verdict":"APPROVE"}),
    )
    .unwrap();
    let (stored, events): (String, String) = s.read(|db| Ok((
        db.query_row("SELECT result_json FROM runtime_nodes WHERE node_id='_review_1'", [], |r| r.get(0))?,
        db.query_row("SELECT payload_json FROM runtime_events WHERE type='REVIEW_INDEPENDENCE_WARNING'", [], |r| r.get(0))?))).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&stored).unwrap()["review_independence"]["status"],
        "unverified"
    );
    assert!(events.contains("unverified"));
    // A reviewer that is the very attempt that executed work is refused regardless of routes.
    let (_p, s, mut e, _o) = executed();
    e.artifacts.insert("review".into(), "e".repeat(64));
    let err = s
        .write(|db| {
            bind(db, "_review_1", "ar", "xa", "cr")?;
            start_review_tx(db, "t", "ar", &e)?;
            publish(db, "ar", "review")?;
            record_result_tx(
                db,
                "t",
                "ar",
                &json!({"report_ref":"review","summary":"ok","checks":[],"verdict":"APPROVE"}),
                &e,
            )
        })
        .unwrap_err();
    assert_eq!(err.code(), "review_not_independent");
}

/// A started review whose host packet lists `changed`, its channel bound to the
/// assignment so its reads count.
fn review_with_packet(changed: Value) -> (Project, Store, EvidenceSnapshot) {
    let (p, s, mut e, _o) = executed();
    e.artifacts.insert("review".into(), "e".repeat(64));
    s.write(|db| {
        bind(db, "_review_1", "ar", "xr", "cr")?;
        db.execute(
            "UPDATE trusted_channels SET assignment_id='ar' WHERE channel_id='cr'",
            [],
        )?;
        start_review_tx(db, "t", "ar", &e)?;
        publish(db, "ar", "review")?;
        event_tx(db, "t", "REVIEW_PACKET", json!({"assignment_ref":"ar","node":"_review_1",
            "packet":{"changed_files":changed,"read_scope":["**"],"read_scope_covers_change":true,"outside_read_scope":[]}}))?;
        Ok(())
    })
    .unwrap();
    (p, s, e)
}
fn read_completely(s: &Store, path: &str, sha256: &str) {
    use sha2::Digest;
    let key = format!("file-read:{:x}", sha2::Sha256::digest(path.as_bytes()));
    s.write(|db| {
        Ok(db.execute(
            "INSERT INTO runtime_requests VALUES ('t',?,'cr','native_file_read','fixture',?)",
            (key, json!({"complete":true,"sha256":sha256}).to_string()),
        )?)
    })
    .unwrap();
}
fn approve(s: &Store, e: &EvidenceSnapshot) -> CoreResult<()> {
    s.write(|db| {
        record_result_tx(
            db,
            "t",
            "ar",
            &json!({"report_ref":"review","summary":"ok","checks":[],"verdict":"APPROVE"}),
            e,
        )
    })
}
fn approved_checks(s: &Store) -> Value {
    s.read(|db| {
        Ok(db.query_row(
            "SELECT json_extract(result_json,'$.checks') FROM runtime_nodes WHERE node_id='_review_1'",
            [],
            |r| r.get::<_, String>(0),
        )?)
    })
    .map(|c| serde_json::from_str(&c).unwrap())
    .unwrap()
}

/// A change set the host could not enumerate never makes APPROVE impossible: the
/// listed files must still be read completely, and the limit is recorded with the
/// approval as an unverified check the final result must disclose.
#[test]
fn an_unenumerable_change_set_allows_approval_with_a_recorded_limitation() {
    let hash = "c".repeat(64);
    let (_p, s, e) = review_with_packet(
        json!({"files":[{"path":"src/a.rs","change":"written","by":["n"],
        "current_sha256":hash}],"total":1,"complete":false,"source":"recorded writes only"}),
    );
    let missing = s
        .read(|db| review_coverage_missing_tx(db, "t", "ar", &json!({"verdict":"APPROVE"})))
        .unwrap();
    assert_eq!(
        missing,
        vec![json!({"path":"src/a.rs","reason":"current complete read is missing or stale"})]
    );
    assert_eq!(
        approve(&s, &e).unwrap_err().code(),
        "review_file_coverage_required"
    );
    read_completely(&s, "src/a.rs", &hash);
    approve(&s, &e).unwrap();
    let checks = approved_checks(&s);
    assert_eq!(checks.as_array().unwrap().len(), 1, "{checks}");
    assert_eq!(checks[0]["outcome"], "unverified");
    assert_eq!(
        checks[0]["blocker"],
        "Changes made by commands outside the recorded writes could not be listed; inspect the folder."
    );
    let (limited, gaps) = s
        .read(|db| {
            Ok((
                db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE type='REVIEW_CHANGE_SET_LIMITED'",
                    [],
                    |r| r.get::<_, i64>(0),
                )?,
                reconciliation::gaps(db, "t")?,
            ))
        })
        .unwrap();
    assert_eq!(limited, 1);
    assert!(
        gaps.contains_key("changes outside the recorded writes"),
        "{gaps:?}"
    );
}

#[test]
fn a_packet_with_more_files_than_listed_records_the_unlisted_count() {
    let hash = "c".repeat(64);
    let (_p, s, e) = review_with_packet(
        json!({"files":[{"path":"src/a.rs","change":"modified","by":[],
        "current_sha256":hash}],"total":4,"complete":true,
        "more":{"count":3,"by_folder":[{"folder":"gen","files":3}]}}),
    );
    read_completely(&s, "src/a.rs", &hash);
    approve(&s, &e).unwrap();
    let checks = approved_checks(&s);
    assert_eq!(checks.as_array().unwrap().len(), 1, "{checks}");
    assert_eq!(checks[0]["name"], "Changed files beyond the review list");
    assert!(checks[0]["blocker"]
        .as_str()
        .unwrap()
        .starts_with("3 changed files"));
}

#[test]
fn a_complete_packet_approval_records_no_limitation() {
    let hash = "c".repeat(64);
    let (_p, s, e) = review_with_packet(
        json!({"files":[{"path":"src/a.rs","change":"modified","by":[],
        "current_sha256":hash}],"total":1,"complete":true}),
    );
    read_completely(&s, "src/a.rs", &hash);
    approve(&s, &e).unwrap();
    assert_eq!(approved_checks(&s), json!([]));
}
