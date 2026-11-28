//! Selected behavioral ports of the named Python tests. Fixtures use real
//! format-12 SQLite/outbox transactions with fake engine/effect observations;
//! they are not production execution or installed-client qualification.
mod support;
use cortex_core::{
    lifecycle::{completion::*, domain::*, runtime::*, waits::*, *},
    ownership::{self, OwnerEpoch},
    ports::{CommandObservation, ProcessObservation},
    store::executor::StoreExecutor,
    CoreResult, Store, TaskState,
};
use rusqlite::Transaction;
use serde_json::{json, Value};
use support::*;
fn evidence(c: char) -> EvidenceSnapshot {
    EvidenceSnapshot {
        project_digest: Some(c.to_string().repeat(64)),
        head: Some("head".into()),
        ignored_digest: Some("1".repeat(64)),
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

#[test]
fn deep_plan_risk_does_not_request_a_consultant() {
    let project = Project::new();
    let store = project.open();
    for (task, count, complexity, scope) in [
        ("sensitive", 1, "normal", "src/storage/migration.rs"),
        ("high", 1, "high", "src/button.rs"),
        ("large", 6, "normal", "src/button.rs"),
    ] {
        let nodes: Vec<_> = (0..count).map(|i| json!({
            "id":format!("work{i}"),"role":"backend-dev","objective":"Proceed",
            "complexity":complexity,"dependencies":[],"scope":[scope]
        })).collect();
        let plan = support::with_goal(&json!({"tasks":nodes,"verification":[["cargo","test"]]}));
        TaskMachine::new(&store).create(task, "Update project", "owner").unwrap();
        store.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?", [task])?;
            plan::accept_routed_tx(db, task, "coordinator", "plan", &plan, &json!([]))?;
            let event: Value = serde_json::from_str(&db.query_row(
                "SELECT payload_json FROM runtime_events WHERE task_id=? AND type='PLAN_ACCEPTED'",
                [task], |r| r.get::<_, String>(0),
            )?)?;
            assert_eq!(event["strategy"]["mode"], "deep");
            assert!(event["strategy"]["consultation"].is_null());
            let nodes = event["plan"]["tasks"].as_array().unwrap();
            assert_eq!(nodes.len(), count);
            assert!(nodes.iter().all(|node| node["id"] != "_consultation"));
            Ok(())
        }).unwrap();
    }
}

fn setup() -> (Project, Store, OwnerEpoch) {
    setup_with_baseline(&evidence('a'))
}
fn setup_with_baseline(baseline: &EvidenceSnapshot) -> (Project, Store, OwnerEpoch) {
    let p = Project::new();
    let s = p.open();
    let o = TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db|{
        db.execute("UPDATE runtime_tasks SET state='PLAN_READY',revision=1,plan_json=? WHERE task_id='t'",
            [json!({"verification":[["true"]]}).to_string()])?;
        execution_baseline_tx(db,"t",baseline)?;
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'n','worker-backend-dev','Work','[]','[\"src\"]','execution','running','a',NULL)",[])?;
        bind(db,"n","a","xa","c")?;
        Ok(())
    }).unwrap();
    (p, s, o)
}
fn bind(db: &Transaction<'_>, node: &str, a: &str, attempt: &str, channel: &str) -> CoreResult<()> {
    db.execute("UPDATE runtime_nodes SET status='running',assignment_ref=? WHERE task_id='t' AND node_id=?",(a,node))?;
    db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,channel_id,engine_session_ref,owner_epoch,created_at,updated_at)
        VALUES (?,'t',?,'worker-backend-dev','ready','ready',?,?,1,'now','now')",
        (a,attempt,channel,format!("es-{attempt}")))?;
    db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,attempt_id,owner_epoch,issued_at)
        VALUES (?,'fixture','t','thread','worker',?,1,'now')",(channel,attempt))?;
    Ok(())
}
fn publish(db: &Transaction<'_>, a: &str, report: &str) -> CoreResult<()> {
    db.execute(
        "INSERT INTO reports VALUES (?,'t','report',?,'now')",
        (report, format!("{report}.md")),
    )?;
    db.execute("INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at)
        VALUES ('t','worker_result',?,?,?,'runtime','now')",(a,format!("result:{a}"),json!({"report_ref":report}).to_string()))?;
    Ok(())
}

#[test]
fn completed_consultation_report_survives_a_plan_without_consultation() {
    use cortex_core::store::documents::{Publication, Publisher};

    let project = Project::new();
    let store = project.open();
    TaskMachine::new(&store).create("t", "goal", "owner").unwrap();
    let plan = support::with_goal(&json!({
        "tasks":[{"id":"work","role":"backend-dev","objective":"Update the file",
            "dependencies":[],"scope":["src"]}],"verification":[["cargo","test"]]
    }));
    let proposal = json!({"question":"Which approach should we choose?",
        "reason":"The project records do not distinguish the two approaches"});
    let baseline = evidence('a');
    store.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'", [])?;
        propose_replan_tx(db, "t", "coordinator", "first", "escalation_request", &proposal)?;
        settle_recovery_tx(db, "t", TaskState::Planning)?;
        plan::accept_tx(db, "t", "coordinator", "first-plan", &plan)?;
        execution_baseline_tx(db, "t", &baseline)?;
        bind(db, "_consultation", "advice", "advice-attempt", "advice-channel")?;
        Ok(())
    }).unwrap();

    let publisher = Publisher::new(&store);
    let draft = publisher.create_draft("t", "advice-channel", "general", "advice-draft").unwrap();
    publisher.write_draft("t", "advice-channel", &draft.draft_id,
        &format!("Cortex draft ID: `{}`\n\nIndependent advice", draft.draft_id)).unwrap();
    let report = publisher.publish("t", "advice-channel", &draft.draft_id, "advice-report",
        Publication { title:"Advice",summary:"Use the second approach",author:"worker",
            source_revision:0,artifacts:json!([]) }).unwrap();
    let mut observed = baseline;
    observed.artifacts.insert(report.report_id.clone(), report.sha256.clone());
    store.write(|db| {
        // This is the assignment publication marker retained by the result gateway.
        db.execute("INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at)
            VALUES ('t','worker_result','advice','advice-publication',?,'runtime','now')",
            [json!({"report_ref":report.report_id}).to_string()])?;
        record_result_tx(db, "t", "advice", &json!({"report_ref":report.report_id,"summary":"Advice","checks":[]}), &observed)?;
        propose_replan_tx(db, "t", "coordinator", "ordinary", "replan_request", &json!({"reason":"Use the advice"}))?;
        settle_recovery_tx(db, "t", TaskState::Planning)?;
        plan::accept_tx(db, "t", "coordinator", "second-plan", &plan)?;
        Ok(())
    }).unwrap();

    store.read(|db| {
        let snapshot = cortex_core::recovery_context::snapshot(db, "t")?;
        let extra = cortex_core::assignment_brief::extras(db, "t", "work", "worker-backend-dev", "execution", "en")?;
        for value in [snapshot, extra] {
            assert_eq!(value["previous_consultation"]["report_id"], report.report_id);
            assert_eq!(value["previous_consultation"]["sha256"], report.sha256);
            assert_eq!(value["previous_consultation"]["plan_revision"], 1);
        }
        Ok(())
    }).unwrap();

    let new_basis = json!({"question":proposal["question"],"reason":proposal["reason"],
        "report_ids":[report.report_id]});
    store.write(|db| {
        propose_replan_tx(db, "t", "coordinator", "new", "escalation_request", &new_basis)?;
        settle_recovery_tx(db, "t", TaskState::Planning)?;
        plan::accept_tx(db, "t", "coordinator", "third-plan", &plan)?;
        let raw:String = db.query_row("SELECT payload_json FROM runtime_events WHERE task_id='t'
            AND type='CONSULTATION_REQUIRED' ORDER BY id DESC LIMIT 1", [], |row| row.get(0))?;
        let packet: Value = serde_json::from_str(&raw)?;
        assert_eq!(packet["dependency_reports"][0]["report_id"], report.report_id);
        assert_eq!(packet["dependency_reports"][0]["sha256"], report.sha256);
        Ok(())
    }).unwrap();
}
fn finish_execution(s: &Store, e: &mut EvidenceSnapshot) {
    e.artifacts.insert("r".into(), "f".repeat(64));
    s.write(|db| {
        publish(db, "a", "r")?;
        record_result_tx(
            db,
            "t",
            "a",
            &json!({"report_ref":"r","summary":"done","checks":[]}),
            e,
        )
    })
    .unwrap();
}
fn verified(s: &Store, o: &OwnerEpoch, e: &EvidenceSnapshot) {
    s.write(|db| {
        retain_check_tx(
            db,
            "t",
            o,
            "check",
            &["true".into()],
            &CommandObservation {
                complete: true,
                exit_code: Some(0),
                receipt_ref: "fake-executor:1".into(),
            },
            e,
        )?;
        assert!(record_verification_tx(db, "t", &["check".into()], e)?);
        db.execute(
            "UPDATE runtime_tasks SET synthesis='Done' WHERE task_id='t'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
}
// Mirrors tests/test_runtime_task_machine.py::test_planned_work_that_changed_nothing_completes_without_review.
#[test]
fn no_change_waiver_verification_then_runtime_completion() {
    let (_p, s, o) = setup();
    let mut e = evidence('a');
    finish_execution(&s, &mut e);
    s.write(|db| add_review_tx(db, "t", &e)).unwrap();
    assert_eq!(state(&s), "VERIFYING");
    verified(&s, &o, &e);
    e.workers
        .insert("xa".into(), ProcessObservation::ConfirmedExited);
    s.write(|db| complete_tx(db, "t", &e)).unwrap();
    assert_eq!(state(&s), "COMPLETED");
}
// Mirrors tests/test_runtime_task_machine.py::test_a_model_cannot_skip_review_of_changed_work.
#[test]
fn changed_work_requires_independent_review_before_verification() {
    let (_p, s, o) = setup();
    let mut e = evidence('b');
    finish_execution(&s, &mut e);
    s.write(|db| add_review_tx(db, "t", &e)).unwrap();
    assert_eq!(state(&s), "REVIEWING");
    assert!(s
        .write(|db| retain_check_tx(
            db,
            "t",
            &o,
            "early",
            &["true".into()],
            &CommandObservation {
                complete: true,
                exit_code: Some(0),
                receipt_ref: "early".into()
            },
            &e
        ))
        .is_err());
    e.artifacts.insert("review".into(), "e".repeat(64));
    s.write(|db| {
        bind(db, "_review_1", "ar", "xr", "cr")?;
        start_review_tx(db, "t", "ar", &e)?;
        publish(db, "ar", "review")?;
        record_result_tx(
            db,
            "t",
            "ar",
            &json!({"report_ref":"review","summary":"Approved","checks":[],"verdict":"APPROVE"}),
            &e,
        )?;
        // The controller selects VERIFYING only after the bound approval.
        db.execute(
            "UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id='t'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    verified(&s, &o, &e);
    e.workers
        .insert("xa".into(), ProcessObservation::ConfirmedExited);
    e.workers
        .insert("xr".into(), ProcessObservation::ConfirmedExited);
    s.write(|db| complete_tx(db, "t", &e)).unwrap();
    assert_eq!(state(&s), "COMPLETED");
}
// Mirrors tests/test_runtime_task_machine.py::test_an_unobservable_project_always_requires_review.
#[test]
fn unavailable_project_is_never_a_no_change_waiver() {
    let (_p, s, _) = setup();
    let mut e = evidence('a');
    finish_execution(&s, &mut e);
    e.project_digest = None;
    s.write(|db| add_review_tx(db, "t", &e)).unwrap();
    assert_eq!(state(&s), "REVIEWING");
}
// Mirrors tests/test_runtime_task_machine.py::test_a_fix_cycle_without_new_changes_is_still_reviewed.
#[test]
fn fix_cycles_cannot_waive_review_and_review_limit_stops() {
    let (_p, s, _) = setup();
    let mut e = evidence('a');
    finish_execution(&s, &mut e);
    s.write(|db|{
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'old-review','worker-code-reviewer','review','[]','[\"**\"]','review','completed',NULL,'{}')",[])?;
        db.execute("UPDATE runtime_tasks SET review_cycle=3",[])?;
        add_review_tx(db,"t",&e)
    }).unwrap();
    assert_eq!(state(&s), "RECOVERING");
    assert_eq!(
        s.read(|db| Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE type='REVIEW_NOT_REQUIRED'",
            [],
            |r| r.get::<_, i64>(0)
        )?))
        .unwrap(),
        0
    );
}
// Mirrors tests/test_runtime_task_machine.py::test_an_earlier_revisions_unreviewed_change_still_requires_review.
#[test]
fn replan_cannot_replace_first_execution_baseline() {
    let (_p, s, _) = setup();
    let mut e = evidence('b');
    finish_execution(&s, &mut e);
    s.write(|db| {
        event_tx(
            db,
            "t",
            "EXECUTION_BASELINE",
            json!({"revision":1,"kind":"observed","revision_digest":"b".repeat(64),"head":"head"}),
        )?;
        add_review_tx(db, "t", &e)
    })
    .unwrap();
    assert_eq!(state(&s), "REVIEWING");
}
// Mirrors tests/test_runtime_task_machine.py::test_cannot_skip_review_or_complete_with_chat.
#[test]
fn permissive_guard_cannot_complete_or_replace_runtime_certification() {
    struct Allow;
    impl TransitionGuards for Allow {
        fn before_transition(
            &self,
            _: &Transaction<'_>,
            _: &str,
            _: TaskState,
            _: TaskState,
        ) -> CoreResult<()> {
            Ok(())
        }
    }
    let p = Project::new();
    let s = p.open();
    let m = TaskMachine::new(&s);
    let o = m.create("t", "goal", "o").unwrap();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='ANSWER_READY',synthesis='chat'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        m.transition("t", &o, "bypass", TaskState::Completed, None, &Allow)
            .unwrap_err()
            .code(),
        "runtime_completion_required"
    );
    assert_eq!(state(&s), "ANSWER_READY");
}
// Mirrors tests/test_runtime_task_machine.py::test_direct_answer_is_runtime_completed_and_cannot_bypass_a_plan.
#[test]
fn informational_answer_requires_exact_receipt_and_no_plan() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "o").unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PLANNING'", [])?;
        submit_answer_tx(db, "t", "An answer")
    })
    .unwrap();
    s.write(|db| complete_tx(db, "t", &EvidenceSnapshot::default()))
        .unwrap();
    assert_eq!(state(&s), "COMPLETED");
    let (_p, s, _) = setup();
    assert_eq!(
        s.write(|db| submit_answer_tx(db, "t", "Skip work"))
            .unwrap_err()
            .code(),
        "direct_answer_not_allowed"
    );
}
// Mirrors tests/test_runtime_task_machine.py::test_a_failed_verification_goes_back_to_the_coordinator_to_repair.
#[test]
fn failed_verification_returns_to_planning_and_does_not_count_as_pass() {
    let (_p, s, o) = setup();
    let mut e = evidence('a');
    finish_execution(&s, &mut e);
    s.write(|db| {
        add_review_tx(db, "t", &e)?;
        retain_check_tx(
            db,
            "t",
            &o,
            "fail",
            &["true".into()],
            &CommandObservation {
                complete: true,
                exit_code: Some(1),
                receipt_ref: "failed-check".into(),
            },
            &e,
        )?;
        assert!(!record_verification_tx(db, "t", &["fail".into()], &e)?);
        Ok(())
    })
    .unwrap();
    assert_eq!(state(&s), "PLANNING");
}
// Mirrors tests/test_runtime_task_machine.py::test_cannot_skip_review_or_complete_with_chat;\n// expands the artifact/liveness/latest-verification gates of state.py::_work_invariants.
#[test]
fn final_gate_rejects_artifact_digest_liveness_and_latest_failure() {
    let (_p, s, o) = setup();
    let mut e = evidence('a');
    finish_execution(&s, &mut e);
    s.write(|db| add_review_tx(db, "t", &e)).unwrap();
    verified(&s, &o, &e);
    assert_eq!(
        s.write(|db| complete_tx(db, "t", &e)).unwrap_err().code(),
        "workers_still_live_or_unknown"
    );
    e.workers
        .insert("xa".into(), ProcessObservation::ConfirmedExited);
    e.artifacts.insert("r".into(), "d".repeat(64));
    assert_eq!(
        s.write(|db| complete_tx(db, "t", &e)).unwrap_err().code(),
        "completion_artifact_changed_or_missing"
    );
    e.artifacts.insert("r".into(), "f".repeat(64));
    e.project_digest = Some("b".repeat(64));
    assert_eq!(
        s.write(|db| complete_tx(db, "t", &e)).unwrap_err().code(),
        "completion_project_revision_changed"
    );
    e.project_digest = Some("a".repeat(64));
    s.write(|db| {
        event_tx(db, "t", "VERIFICATION_FAILED", json!({"revision":1}))?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.write(|db| complete_tx(db, "t", &e)).unwrap_err().code(),
        "completion_verification_missing"
    );
}
// Mirrors tests/test_reviewer_verification_dedupe.py::test_reused_source_cannot_verify_two_checks.
#[test]
fn duplicate_or_consumed_verification_receipts_never_pass() {
    let (_p, s, o) = setup();
    let mut e = evidence('a');
    finish_execution(&s, &mut e);
    s.write(|db| {
        add_review_tx(db, "t", &e)?;
        retain_check_tx(
            db,
            "t",
            &o,
            "one",
            &["true".into()],
            &CommandObservation {
                complete: true,
                exit_code: Some(0),
                receipt_ref: "same-receipt".into(),
            },
            &e,
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.write(|db| record_verification_tx(db, "t", &["one".into(), "one".into()], &e))
            .unwrap_err()
            .code(),
        "verification_receipt_replayed"
    );
    s.write(|db| {
        assert!(record_verification_tx(db, "t", &["one".into()], &e)?);
        db.execute("UPDATE runtime_tasks SET state='VERIFYING'", [])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.write(|db| record_verification_tx(db, "t", &["one".into()], &e))
            .unwrap_err()
            .code(),
        "verification_receipt_replayed"
    );
}
fn ask(s: &Store) -> Value {
    s.write(|db| {
        request_tx_bound(
            db,
            "t",
            "c",
            "ask",
            &[QuestionForm {
                question: "Choose".into(),
                options: vec!["One".into(), "Two".into()],
            }],
        )
    })
    .unwrap()
}
// Mirrors tests/test_worker_wait_continuation.py::test_answer_and_single_continuation_claim_commit_atomically.
#[test]
fn answer_and_one_claim_are_atomic_and_assignment_local() {
    let (_p, s, _) = setup();
    let q = ask(&s);
    s.write(|db| {
        assert!(check_mutation_tx(db, "t", "a").is_err());
        assert!(check_mutation_tx(db, "t", "sibling").is_ok());
        assert!(answer_tx(
            db,
            "t",
            q["question_id"].as_str().unwrap(),
            "One",
            "text",
            Some("incoming")
        )?);
        assert!(!answer_tx(
            db,
            "t",
            q["question_id"].as_str().unwrap(),
            "Two",
            "text",
            None
        )?);
        let claim = claim_tx(db, "t", "a", "es-xa")?.unwrap();
        assert_eq!(claim["answer"], "One");
        assert!(claim_tx(db, "t", "a", "es-xa")?.is_none());
        assert!(finish_tx(
            db,
            "t",
            claim["wait"]["wait_id"].as_str().unwrap(),
            claim["wait"]["delivery_id"].as_str().unwrap(),
            None
        )?);
        Ok(())
    })
    .unwrap();
    assert_eq!(state(&s), "EXECUTING");
}
// Mirrors tests/test_worker_wait_continuation.py::test_wrong_session_or_ambiguous_started_delivery_is_never_replayed.
#[test]
fn wrong_session_or_recovered_delivery_is_orphaned_never_replayed() {
    for wrong_session in [true, false] {
        let (_p, s, _) = setup();
        let q = ask(&s);
        s.write(|db| {
            answer_tx(
                db,
                "t",
                q["question_id"].as_str().unwrap(),
                "One",
                "elicitation",
                None,
            )?;
            let claim = claim_tx(db, "t", "a", if wrong_session { "wrong" } else { "es-xa" })?;
            assert_eq!(claim.is_none(), wrong_session);
            recover_deliveries_tx(db, "t")?;
            assert!(claim_tx(db, "t", "a", "es-xa")?.is_none());
            assert_eq!(projection_tx(db, "t")?[0]["status"], "orphaned");
            Ok(())
        })
        .unwrap();
    }
}
// Mirrors tests/test_worker_wait_continuation.py::test_late_answers_cannot_revive_stopped_or_replaced_workers.
#[test]
fn individual_stop_cancels_wait_without_releasing_capacity_or_sibling() {
    let (_p, s, _) = setup();
    let q = ask(&s);
    s.write(|db| {
        stop_assignment_tx(db, "t", "a", "stop")?;
        assert!(!answer_tx(
            db,
            "t",
            q["question_id"].as_str().unwrap(),
            "Late",
            "text",
            None
        )?);
        assert!(claim_tx(db, "t", "a", "es-xa")?.is_none());
        Ok(())
    })
    .unwrap();
    assert_eq!(state(&s), "EXECUTING");
}
// Mirrors tests/test_workspace_pause.py::test_pause_resume_survives_store_restart_and_preserves_question
// and ::test_paused_worker_decision_cannot_be_claimed_answered_or_cancelled_by_polling.
#[test]
fn pause_restart_resume_preserves_decision_but_fences_old_session() {
    let (p, s, o) = setup();
    let q = ask(&s);
    s.write(|db| owner_pause_tx(db, "t", &o, "pause", false, 0))
        .unwrap();
    assert_eq!(state(&s), "PAUSED");
    s.write(|db| {
        interrupt_tx(db, "t")?;
        assert!(!answer_tx(
            db,
            "t",
            q["question_id"].as_str().unwrap(),
            "One",
            "text",
            None
        )?);
        assert!(claim_tx(db, "t", "a", "es-xa")?.is_none());
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = p.open();
    let v = s.write(|db| pause_version_tx(db, "t")).unwrap();
    assert_eq!(
        s.write(|db| owner_pause_tx(db, "t", &o, "stale", true, v - 1))
            .unwrap_err()
            .code(),
        "stale_version"
    );
    s.write(|db| owner_pause_tx(db, "t", &o, "resume", true, v))
        .unwrap();
    s.write(|db| {
        assert!(answer_tx(
            db,
            "t",
            q["question_id"].as_str().unwrap(),
            "One",
            "text",
            None
        )?);
        assert!(claim_tx(db, "t", "a", "es-xa")?.is_none());
        assert_eq!(projection_tx(db, "t")?[0]["status"], "ready");
        Ok(())
    })
    .unwrap();
    assert_eq!(state(&s), "EXECUTING");
}
// The no-stop policy replaces the runtime stop question: a runtime stop is a
// continuation on the strategy ladder, never an owner question.
#[test]
fn a_runtime_stop_continues_instead_of_asking_the_owner() {
    let (_p, s, _) = setup();
    s.write(|db| {
        assert_eq!(
            stop_tx(db, "t", "coordinator_prompt_failed")?,
            TaskState::Recovering
        );
        assert!(cortex_core::lifecycle::automatic::owed(db, "t")?.is_some());
        Ok(())
    })
    .unwrap();
    assert_eq!(support::count(&s, "pending_questions"), 0);
    assert_eq!(state(&s), "RECOVERING");
}
// Mirrors tests/test_recovery_cancellation_targets.py::test_recovery_cancels_and_terminates_only_the_assignments_that_were_live.
#[test]
fn recovery_captures_live_only_and_waits_for_confirmed_absence() {
    let (_p, s, _) = setup();
    s.write(|db|{
        db.execute("INSERT INTO runtime_nodes VALUES ('t',1,'finished','worker-backend-dev','done','[]','[\"docs\"]','execution','completed','done','{}')",[])?;
        interrupt_tx(db,"t")?;
        assert_eq!(recovery_cancellations_tx(db,"t")?.1,["a"]);Ok(())
    }).unwrap();
    assert_eq!(
        s.write(|db| settle_recovery_tx(db, "t", TaskState::Planning))
            .unwrap_err()
            .code(),
        "lost_worker_termination_unconfirmed"
    );
    s.write(|db|{
        db.execute("INSERT INTO capacity_reservations VALUES ('cap','connection','t','a','xa','worker','released','now','now','process_exited')",[])?;
        settle_recovery_tx(db,"t",TaskState::Planning)?;
        assert!(recovery_cancellations_tx(db,"t")?.1.is_empty());Ok(())
    }).unwrap();
    assert_eq!(state(&s), "PLANNING");
}
// Mirrors tests/test_runtime_task_machine.py::test_new_plans_alone_cannot_renew_returns_forever.
#[test]
fn replans_and_coordinator_returns_are_durable_and_user_bounded() {
    let (_p, s, _) = setup();
    for i in 0..7 {
        let accepted = s
            .write(|db| {
                event_tx(db, "t", "PLAN_ACCEPTED", json!({"revision":i}))?;
                db.execute("UPDATE runtime_tasks SET state='EXECUTING'", [])?;
                return_to_coordinator_tx(db, "t", "worker_failed")
            })
            .unwrap();
        assert_eq!(accepted, i < 6);
    }
    s.write(|db| {
        event_tx(db, "t", "WORKER_USER_ANSWER_RECEIVED", json!({}))?;
        assert!(return_to_coordinator_tx(db, "t", "worker_failed")?);
        Ok(())
    })
    .unwrap();
    for i in 0..3 {
        s.write(|db| {
            propose_replan_tx(
                db,
                "t",
                "runtime",
                &format!("replan-{i}"),
                "replan_request",
                &json!({"reason":"different decomposition"}),
            )
        })
        .unwrap();
    }
    // Ordinary replan requests remain replan requests across revisions.
    assert_eq!(
        s.write(|db| propose_replan_tx(
            db,
            "t",
            "runtime",
            "exhausted",
            "replan_request",
            &json!({})
        ))
        .unwrap()["decision"],
        "replan"
    );
}
// Mirrors tests/test_runtime_task_machine.py::test_plan_atomic_and_idempotent,\n// additionally exercising stale task owner fencing through the writer executor.
#[test]
fn executor_commits_request_state_outbox_once_and_rejects_stale_owner() {
    let p = Project::new();
    let s = p.open();
    let o = TaskMachine::new(&s).create("t", "goal", "o").unwrap();
    drop(s);
    let executor = StoreExecutor::open(p.key()).unwrap();
    let submit_cancel = |owner: OwnerEpoch| {
        submit(
            &executor,
            RuntimeRequest {
                task: "t".into(),
                owner,
                key: "cancel".into(),
                action: RuntimeAction::Cancel {
                    reason: "owner".into(),
                },
                evidence: EvidenceSnapshot::default(),
            },
        )
        .unwrap()
        .blocking_recv()
        .unwrap()
    };
    let first = submit_cancel(o.clone()).unwrap();
    assert_eq!(first, submit_cancel(o.clone()).unwrap());
    executor
        .write(move |db| {
            ownership::acquire_tx(db, "t", "replacement", Some(o.epoch))?;
            Ok(())
        })
        .unwrap()
        .blocking_recv()
        .unwrap()
        .unwrap();
    assert_eq!(
        submit_cancel(OwnerEpoch {
            owner: "o".into(),
            epoch: 1
        })
        .unwrap_err()
        .code(),
        "stale_owner"
    );
    assert_eq!(
        executor
            .read(|db| Ok(
                db.query_row("SELECT count(*) FROM runtime_requests", [], |r| r
                    .get::<_, i64>(0))?
            ))
            .unwrap()
            .blocking_recv()
            .unwrap()
            .unwrap(),
        1
    );
}
// Mirrors tests/test_runtime_task_machine.py::test_plan_atomic_and_idempotent (rollback invariant).
#[test]
fn event_failure_rolls_back_stop_and_questions() {
    let (_p, s, _) = setup();
    let before = count(&s, "runtime_events");
    s.write(|db|{db.execute_batch("CREATE TRIGGER fail_stop BEFORE INSERT ON runtime_events BEGIN SELECT RAISE(ABORT,'fault'); END;")?;Ok(())}).unwrap();
    assert!(s
        .write(|db| stop_tx(db, "t", "coordinator_failed"))
        .is_err());
    assert_eq!(state(&s), "EXECUTING");
    assert_eq!(count(&s, "pending_questions"), 0);
    assert_eq!(count(&s, "runtime_events"), before);
}
// Mirrors tests/test_workspace_pause_service.py::test_resume_uses_the_version_shown_after_stop
// and tests/test_workspace_outbox.py transactional invalidation behavior.
#[test]
fn bound_pause_version_outbox_and_receipt_share_one_commit() {
    let (_p, s, o) = setup();
    s.write(|db|{
        db.execute("INSERT INTO workspace_projects VALUES ('p','/fixture','owner','now',1)",[])?;
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES ('task','p','description','now')",[])?;
        db.execute("INSERT INTO workspace_executions VALUES ('execution','task','t','session',1,1,'now')",[])?;
        Ok(())
    }).unwrap();
    let old = s.write(|db| pause_version_tx(db, "t")).unwrap();
    s.write(|db| owner_pause_tx(db, "t", &o, "pause", false, old))
        .unwrap();
    let version = s.write(|db| pause_version_tx(db, "t")).unwrap();
    assert!(version > old);
    let outbox = count(&s, "workspace_outbox");
    assert!(outbox > 0);
    s.write(|db| owner_pause_tx(db, "t", &o, "pause", false, old))
        .unwrap();
    assert_eq!(count(&s, "workspace_outbox"), outbox);
    assert_eq!(
        s.write(|db| owner_pause_tx(db, "t", &o, "stale", true, old))
            .unwrap_err()
            .code(),
        "stale_version"
    );
    let before_requests = count(&s, "runtime_requests");
    let before_events = count(&s, "runtime_events");
    s.write(|db|{db.execute_batch("CREATE TEMP TRIGGER reject_outbox BEFORE INSERT ON workspace_outbox BEGIN SELECT RAISE(ABORT,'outbox fault'); END;")?;Ok(())}).unwrap();
    assert!(s
        .write(|db| owner_pause_tx(db, "t", &o, "resume", true, version))
        .is_err());
    assert_eq!(state(&s), "PAUSED");
    assert_eq!(count(&s, "workspace_outbox"), outbox);
    assert_eq!(count(&s, "runtime_requests"), before_requests);
    assert_eq!(count(&s, "runtime_events"), before_events);
}
// Mirrors tests/test_review_revision_rebind.py::test_a_changed_project_rebinds_the_review_and_a_new_submission_succeeds.
#[test]
fn review_rebinding_refusal_is_durable_and_new_key_can_succeed() {
    let (_p, s, o) = setup();
    let mut e = evidence('b');
    finish_execution(&s, &mut e);
    s.write(|db| {
        add_review_tx(db, "t", &e)?;
        bind(db, "_review_1", "ar", "xr", "cr")?;
        start_review_tx(db, "t", "ar", &e)
    })
    .unwrap();
    e.project_digest = Some("c".repeat(64));
    e.artifacts.insert("review".into(), "d".repeat(64));
    s.write(|db| publish(db, "ar", "review")).unwrap();
    let request = |key: &str, e: EvidenceSnapshot| RuntimeRequest {
        task: "t".into(),
        owner: o.clone(),
        key: key.into(),
        action: RuntimeAction::RecordResult {
            assignment: "ar".into(),
            result: json!({"report_ref":"review","summary":"Approved","checks":[],"verdict":"APPROVE"}),
        },
        evidence: e,
    };
    // Snapshot is constructed afresh, not cloned out of any model payload.
    let mut changed = evidence('c');
    changed.artifacts.insert("review".into(), "d".repeat(64));
    let r = s
        .write(|db| execute_tx(db, &request("old", changed)))
        .unwrap();
    assert_eq!(r["error"], "review_project_revision_changed");
    let r = s
        .write(|db| execute_tx(db, &request("old", evidence('c'))))
        .unwrap();
    assert_eq!(r["error"], "review_project_revision_changed");
    s.write(|db| execute_tx(db, &request("new", e))).unwrap();
    s.write(|db| cortex_core::lifecycle::review::advance_tx(db, "t"))
        .unwrap();
    assert_eq!(state(&s), "VERIFYING");
}
// Mirrors tests/test_review_revision_rebind.py::test_a_project_that_keeps_changing_is_refused_after_the_bound.
#[test]
fn changing_review_has_three_rebinds_not_an_infinite_loop() {
    let (_p, s, _) = setup();
    let mut e = evidence('b');
    finish_execution(&s, &mut e);
    s.write(|db| {
        add_review_tx(db, "t", &e)?;
        bind(db, "_review_1", "ar", "xr", "cr")?;
        start_review_tx(db, "t", "ar", &e)
    })
    .unwrap();
    for (i, c) in ['c', 'd', 'e', 'f'].into_iter().enumerate() {
        let refusal = s
            .write(|db| {
                cortex_core::lifecycle::review::preflight_result_tx(
                    db,
                    "t",
                    "ar",
                    &format!("key-{i}"),
                    &evidence(c),
                )
            })
            .unwrap()
            .unwrap();
        assert_eq!(
            refusal["error"],
            if i < 3 {
                "review_project_revision_changed"
            } else {
                "review_project_revision_unstable"
            }
        );
    }
    assert_eq!(
        s.read(|db| Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE type='REVIEW_STARTED'",
            [],
            |r| r.get::<_, i64>(0)
        )?))
        .unwrap(),
        4
    );
}
/// Owner store 2026-11-19: after three rebinds a review was refused
/// `review_project_revision_unstable` six times and the task went to recovery,
/// because no binding was recorded past the budget and every later submission was
/// compared with the stale one. Once two submissions see the same project, the
/// review is re-bound and the next submission on that state is accepted.
#[test]
fn an_unstable_review_rebinds_once_the_project_is_stable_again() {
    let (_p, s, _) = setup();
    let mut e = evidence('b');
    finish_execution(&s, &mut e);
    s.write(|db| {
        add_review_tx(db, "t", &e)?;
        bind(db, "_review_1", "ar", "xr", "cr")?;
        start_review_tx(db, "t", "ar", &e)
    })
    .unwrap();
    let submit = |key: &str, c: char| {
        s.write(|db| {
            cortex_core::lifecycle::review::preflight_result_tx(db, "t", "ar", key, &evidence(c))
        })
        .unwrap()
        .map(|r| r["error"].as_str().unwrap().to_owned())
    };
    for (i, c) in ['c', 'd', 'e'].into_iter().enumerate() {
        assert_eq!(
            submit(&format!("k{i}"), c).as_deref(),
            Some("review_project_revision_changed")
        );
    }
    // Still changing: unstable, and the stale binding stays.
    assert_eq!(
        submit("k3", 'f').as_deref(),
        Some("review_project_revision_unstable")
    );
    assert_eq!(
        submit("k4", 'g').as_deref(),
        Some("review_project_revision_unstable")
    );
    // A key refused as unstable never records a verdict.
    assert_eq!(
        submit("k4", 'g').as_deref(),
        Some("review_resubmit_required")
    );
    // Unchanged since the previous refusal: re-bound to that state.
    assert_eq!(
        submit("k5", 'g').as_deref(),
        Some("review_project_revision_changed")
    );
    let latest: Value = serde_json::from_str(
        &s.read(|db| Ok(db.query_row(
            "SELECT payload_json FROM runtime_events WHERE type='REVIEW_STARTED' ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get::<_, String>(0)
        )?))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(latest["revision_digest"], "g".repeat(64));
    assert_eq!(latest["after_unstable"], true);
    // The next submission on the same state passes the preflight.
    assert_eq!(submit("k6", 'g'), None);
}
// Mirrors tests/test_targeted_review_fixes.py::test_a_local_finding_creates_a_single_fix_with_only_its_findings.
#[test]
fn targeted_review_fix_retains_effective_scope_and_only_local_findings() {
    let (_p, s, _) = setup();
    let mut e = evidence('b');
    finish_execution(&s, &mut e);
    s.write(|db|{
        event_tx(db,"t","ACCESS_GRANTED",json!({"assignment_ref":"a","added_paths":["extra"]}))?;
        add_review_tx(db,"t",&e)?;
        db.execute("UPDATE runtime_nodes SET status='completed',result_json=? WHERE node_id='_review_1'",
            [json!({"verdict":"REQUEST_CHANGES","summary":"shared summary","report_ref":"review","findings":[
                {"node":"n","cross_cutting":false,"files":["extra/file.rs"],"fix":"Check the empty value"}]}).to_string()])?;
        cortex_core::lifecycle::review::advance_tx(db,"t")?;
        let (scope,objective):(String,String)=db.query_row("SELECT scope_json,objective FROM runtime_nodes WHERE node_id='_fix_1_n'",[],|r|Ok((r.get(0)?,r.get(1)?)))?;
        assert!(scope.contains("extra"));assert!(objective.contains("Check the empty value"));assert!(!objective.contains("shared summary"));
        Ok(())
    }).unwrap();
    assert_eq!(state(&s), "EXECUTING");
}
// Mirrors tests/test_worker_wait_continuation.py::test_complete_batch_resumes_same_worker_session_without_blocking_sibling.
#[test]
fn complete_question_batch_readies_one_continuation_without_blocking_siblings() {
    let (_p, s, _) = setup();
    let q = s
        .write(|db| {
            request_tx_bound(
                db,
                "t",
                "c",
                "batch",
                &[
                    QuestionForm {
                        question: "First?".into(),
                        options: vec!["A".into(), "B".into()],
                    },
                    QuestionForm {
                        question: "Second?".into(),
                        options: vec!["C".into(), "D".into()],
                    },
                ],
            )
        })
        .unwrap();
    s.write(|db| {
        answer_tx(
            db,
            "t",
            q["questions"][0]["question_id"].as_str().unwrap(),
            "A",
            "text",
            None,
        )?;
        assert_eq!(projection_tx(db, "t")?[0]["status"], "open");
        assert!(check_mutation_tx(db, "t", "sibling").is_ok());
        answer_tx(
            db,
            "t",
            q["questions"][1]["question_id"].as_str().unwrap(),
            "D",
            "elicitation",
            None,
        )?;
        let claim = claim_tx(db, "t", "a", "es-xa")?.unwrap();
        assert_eq!(claim["answer"].as_array().unwrap().len(), 2);
        assert!(claim_tx(db, "t", "a", "es-xa")?.is_none());
        Ok(())
    })
    .unwrap();
}
// Mirrors tests/test_recovery_cancellation_targets.py::test_a_stop_recorded_before_captures_existed_is_still_cancelled.
#[test]
fn recovery_backfills_legacy_cancelled_nodes_without_repeating_settled_ones() {
    let (_p, s, _) = setup();
    s.write(|db|{
        db.execute("UPDATE runtime_nodes SET status='cancelled'",[])?;
        db.execute("UPDATE runtime_tasks SET state='RECOVERING'",[])?;
        assert_eq!(recovery_cancellations_tx(db,"t")?.1,["a"]);
        assert_eq!(recovery_cancellations_tx(db,"t")?.1,["a"]);
        db.execute("INSERT INTO capacity_reservations VALUES ('cap','connection','t','a','xa','worker','released','now','now','process_exited')",[])?;
        settle_recovery_tx(db,"t",TaskState::Planning)?;
        assert!(recovery_cancellations_tx(db,"t")?.1.is_empty());Ok(())
    }).unwrap();
}
// Mirrors tests/test_lifecycle_integrity.py::test_every_path_that_cancels_a_worker_also_cancels_its_open_question.
#[test]
fn assignment_stop_closes_the_explicit_work_graph_attempt_not_the_engine_id() {
    let (_p, s, _) = setup();
    ask(&s);
    s.write(|db|{
        db.execute("INSERT INTO work_assignments VALUES ('work','t','n','{}',NULL,'running','structured','work-key','now')",[])?;
        db.execute("INSERT INTO work_assignment_attempts VALUES ('work-attempt','t','work',1,NULL,NULL,'{}','running',NULL,'attempt-key','now')",[])?;
        db.execute("INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at)
            VALUES ('t','delegation_requested','a','delegation',?,'runtime','now')",
            [json!({"work_attempt_ref":"work-attempt"}).to_string()])?;
        stop_assignment_tx(db,"t","a","owner_stop")?;
        assert_eq!(db.query_row("SELECT status FROM work_assignment_attempts WHERE attempt_id='work-attempt'",[],|r|r.get::<_,String>(0))?,"failed");
        assert_eq!(projection_tx(db,"t")?[0]["status"],"cancelled");Ok(())
    }).unwrap();
}
// Mirrors tests/test_runtime_task_machine.py::test_unrelated_same_name_pass_does_not_resolve_synthesis_gap.
#[test]
fn completion_never_infers_success_from_an_unverified_worker_check() {
    let (_p, s, o) = setup();
    let mut e = evidence('a');
    finish_execution(&s, &mut e);
    s.write(|db| add_review_tx(db, "t", &e)).unwrap();
    verified(&s, &o, &e);
    e.workers
        .insert("xa".into(), ProcessObservation::ConfirmedExited);
    for outcome in ["passed", "not_run", "unverified"] {
        s.write(|db| {
            db.execute(
                "UPDATE runtime_nodes SET result_json=? WHERE assignment_ref='a'",
                [
                    json!({"report_ref":"r","report_digest":"f".repeat(64),"checks":[
                        {"name":"missing receipt","outcome":outcome}
                    ]})
                    .to_string(),
                ],
            )?;
            Ok(())
        })
        .unwrap();
        let code = s.write(|db| complete_tx(db, "t", &e)).unwrap_err().code();
        assert_eq!(
            code,
            if outcome == "passed" {
                "completion_worker_check_unverified"
            } else {
                "completion_unresolved_worker_check"
            }
        );
    }
}

// Real store transactions, fake effect observations as in the existing completion
// fixtures. Completion rederives every link instead of trusting stored markers.
#[test]
fn resolved_check_history_is_rederived_at_completion() {
    use cortex_core::lifecycle::worker_checks;
    for kind in ["closed", "withdrawn", "retracted", "superseded"] {
        for invalid in ["valid", "forged_link", "lost_output", "later_failure"] {
            let (_p, s, o) = setup();
            let mut e = evidence('a');
            let bad_source = "import re; re.search('(', 'subject')";
            let good_source = "import re; re.search('subject', 'subject')";
            let output = "File \"<string>\"\nre.error: unbalanced pattern\n";
            let mut checks = vec![json!({"name":"check","outcome":"failed","tool_call_ref":"bad"})];
            if kind != "withdrawn" {
                checks.push(json!({"name":"check","outcome":"passed","tool_call_ref":"good"}));
            }
            match kind {
                "withdrawn" => checks[0]["probe_withdrawn"] = json!("expected exploratory error"),
                "retracted" => {
                    checks[0]["invocation_error"] =
                        json!({"corrected_by":"good","reason":"my regex typo"})
                }
                "superseded" => {
                    checks[0]["command_correction"] =
                        json!({"corrected_by":"good","reason":"my regex typo"})
                }
                _ => (),
            }
            s.write(|db| {
                for (reference,code,source) in [("bad",1,bad_source),("good",0,if kind=="closed" {bad_source} else {good_source})] {
                    let body=if code==1 {output} else {""};
                    let receipt=json!({"tool_call_ref":reference,"argv":["python3","-B","-c",source],
                        "cwd":"/fixture","profile":"development","complete":true,"exit_code":code,
                        "failure":null,"revision_digest":"a".repeat(64),"large_output_id":reference,
                        "output":body,"byte_count":body.len()});
                    db.execute("INSERT INTO runtime_requests VALUES ('t',?,'c','command_run','bound',?)",(reference,receipt.to_string()))?;
                    db.execute("INSERT INTO large_outputs VALUES (?,'t','c','terminal',?,'fixture',?,0,?,NULL,NULL,'now')",
                        (reference,"b".repeat(64),body.len() as i64,code))?;
                    e.artifacts.insert(reference.into(),"b".repeat(64));
                }
                Ok(())
            }).unwrap();
            let normalized = s
                .read(|db| {
                    let mut runs = worker_checks::load_runs(db, "t", "c")?;
                    worker_checks::observe_outputs(db, "t", &mut runs, &e.artifacts)?;
                    worker_checks::evaluate(&checks, &runs, e.project_digest.as_deref())
                })
                .unwrap();
            e.artifacts.insert("r".into(), "f".repeat(64));
            s.write(|db| {
                publish(db, "a", "r")?;
                record_result_tx(
                    db,
                    "t",
                    "a",
                    &json!({"report_ref":"r","summary":"done","checks":normalized}),
                    &e,
                )?;
                assert_eq!(
                    db.query_row(
                        "SELECT status FROM runtime_nodes WHERE assignment_ref='a'",
                        [],
                        |r| r.get::<_, String>(0)
                    )?,
                    "completed"
                );
                add_review_tx(db, "t", &e)?;
                Ok(())
            })
            .unwrap();
            verified(&s, &o, &e);
            e.workers
                .insert("xa".into(), ProcessObservation::ConfirmedExited);
            s.write(|db|{
                if invalid=="forged_link" {
                    let field=match kind {"closed"=>"closed_by","superseded"=>"superseded_by",_=>"retracted_by"};
                    db.execute("UPDATE runtime_nodes SET result_json=json_set(result_json,?, 'forged') WHERE assignment_ref='a'",
                        [format!("$.checks[0].{field}")])?;
                }
                if invalid=="later_failure" {
                    let raw:String=db.query_row("SELECT result_json FROM runtime_requests WHERE request_key='bad'",[],|r|r.get(0))?;
                    let mut receipt:Value=serde_json::from_str(&raw)?;
                    receipt["tool_call_ref"]=json!("late");
                    db.execute("INSERT INTO runtime_requests VALUES ('t','late','c','command_run','bound',?)",[receipt.to_string()])?;
                }
                Ok(())
            }).unwrap();
            if invalid == "lost_output" {
                e.artifacts.remove("bad");
            }
            let result = s.write(|db| complete_tx(db, "t", &e));
            // A withdrawn independent probe is not undone by another run; a
            // corrected invocation/exact rerun must still be current.
            let expected =
                invalid == "valid" || (invalid == "later_failure" && kind == "withdrawn");
            assert_eq!(result.is_ok(), expected, "{kind}/{invalid}: {result:?}");
        }
    }
}
// A worker's `checks[].command` is descriptive: the cited receipt is the run. A verified
// pass whose description differs from the receipt's argv (an abbreviated inline script,
// as a live reviewer sent) completes after passing runtime verification; the receipt
// gates still apply, so a trivial receipt keeps completion refused whatever it says.
#[test]
fn descriptive_check_command_never_blocks_a_verified_pass_at_completion() {
    use cortex_core::lifecycle::worker_checks;
    let script = "import sys; sys.exit(0 if open('README.md').read() else 1)";
    for (argv, command, completes) in [
        (
            json!(["python3", "-I", "-c", script]),
            json!(["python3", "-I", "-c", "..."]),
            true,
        ),
        (
            json!(["python3", "-I", "-c", script]),
            json!(["python3", "-I", "-c", script]),
            true,
        ),
        (json!(["python3", "-I", "-c", script]), Value::Null, true),
        (
            json!(["cat", "README.md"]),
            json!(["python3", "-I", "-c", script]),
            false,
        ),
    ] {
        let (_p, s, o) = setup();
        let mut e = evidence('a');
        s.write(|db| {
            let receipt = json!({"tool_call_ref":"good","argv":argv,"cwd":"/fixture","profile":"development",
                "complete":true,"exit_code":0,"failure":null,"revision_digest":"a".repeat(64),
                "large_output_id":"good","output":"","byte_count":0});
            db.execute("INSERT INTO runtime_requests VALUES ('t','good','c','command_run','bound',?)",[receipt.to_string()])?;
            db.execute("INSERT INTO large_outputs VALUES ('good','t','c','terminal',?,'fixture',0,0,0,NULL,NULL,'now')",
                ["b".repeat(64)])?;
            Ok(())
        })
        .unwrap();
        e.artifacts.insert("good".into(), "b".repeat(64));
        let mut check = json!({"name":"readme check","outcome":"passed","tool_call_ref":"good"});
        if !command.is_null() {
            check["command"] = command.clone();
        }
        let normalized = s
            .read(|db| {
                let mut runs = worker_checks::load_runs(db, "t", "c")?;
                worker_checks::observe_outputs(db, "t", &mut runs, &e.artifacts)?;
                worker_checks::evaluate(&[check.clone()], &runs, e.project_digest.as_deref())
            })
            .unwrap();
        e.artifacts.insert("r".into(), "f".repeat(64));
        s.write(|db| {
            publish(db, "a", "r")?;
            record_result_tx(
                db,
                "t",
                "a",
                &json!({"report_ref":"r","summary":"done","checks":normalized}),
                &e,
            )?;
            add_review_tx(db, "t", &e)?;
            Ok(())
        })
        .unwrap();
        verified(&s, &o, &e);
        e.workers
            .insert("xa".into(), ProcessObservation::ConfirmedExited);
        let result = s.write(|db| complete_tx(db, "t", &e));
        if completes {
            assert!(result.is_ok(), "{command}: {result:?}");
            assert_eq!(state(&s), "COMPLETED");
        } else {
            assert_eq!(
                result.unwrap_err().code(),
                "completion_unresolved_worker_check"
            );
        }
    }
}
// Ignored-file changes (or an unobservable ignored set) never take the no-change waiver.
#[test]
fn ignored_file_changes_or_unobserved_ignored_files_refuse_the_waiver() {
    for (before, after) in [
        (Some("1"), Some("2")),
        (Some("1"), None),
        (None, Some("1")),
        (None, None),
    ] {
        let mut baseline = evidence('a');
        baseline.ignored_digest = before.map(|s| s.repeat(64));
        let (_p, s, _) = setup_with_baseline(&baseline);
        let mut e = evidence('a');
        e.ignored_digest = after.map(|s| s.repeat(64));
        finish_execution(&s, &mut e);
        s.write(|db| add_review_tx(db, "t", &e)).unwrap();
        assert_eq!(state(&s), "REVIEWING", "{before:?} -> {after:?}");
        s.read(|db| {
            let waived: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id='t' AND type='REVIEW_NOT_REQUIRED')",
                [],
                |r| r.get(0),
            )?;
            assert!(!waived, "{before:?} -> {after:?}");
            Ok(())
        })
        .unwrap();
    }
}
// The 128-step progress budget is persisted: reopening the store continues the count, and
// only an owner action or a driver stop starts a fresh budget.
#[test]
fn progress_budget_survives_restart_and_resets_on_owner_action() {
    use cortex_core::lifecycle::progress::spend_tx;
    let (p, s, _) = setup();
    for _ in 0..127 {
        s.write(|db| spend_tx(db, "t", 128)).unwrap();
    }
    drop(s);
    let s = p.open();
    s.write(|db| spend_tx(db, "t", 128)).unwrap();
    let error = s.write(|db| spend_tx(db, "t", 128)).unwrap_err();
    assert_eq!(error.code(), "native_progress_limit");
    for event in ["NATIVE_DRIVER_STOPPED", "USER_STEERING", "TURN_INTERRUPTED"] {
        s.write(|db| event_tx(db, "t", event, json!({}))).unwrap();
        s.write(|db| spend_tx(db, "t", 1)).unwrap();
        assert_eq!(
            s.write(|db| spend_tx(db, "t", 1)).unwrap_err().code(),
            "native_progress_limit"
        );
    }
}

// Metadata-only or incomplete observations (restored mtimes) never take either waiver.
#[test]
fn metadata_only_or_incomplete_observations_refuse_the_waiver() {
    for (metadata_only, incomplete) in [(true, false), (false, true)] {
        let (_p, s, _) = setup();
        let mut e = evidence('a');
        e.metadata_only = metadata_only;
        e.incomplete = incomplete;
        finish_execution(&s, &mut e);
        s.write(|db| add_review_tx(db, "t", &e)).unwrap();
        assert_eq!(state(&s), "REVIEWING", "{metadata_only}/{incomplete}");
    }
}
// Fix specialization is based on the affected files and retained author.
#[test]
fn new_review_fixes_use_general_with_domain_competencies() {
    for (original, finding, expected) in [
        ("worker-qa-engineer",
            json!({"files":["SECURITY.md","docs/a.md"],"fix":"Update docs"}),
            json!(["documentation"])),
        ("worker-qa-engineer",
            json!({"files":["crates/a/src/lib.rs"],"fix":"Fix code"}),
            json!(["backend"])),
        ("worker-qa-engineer",
            json!({"files":["packages/ui/src/view.tsx","crates/a/src/lib.rs"],"fix":"Fix both"}),
            json!(["backend","frontend"])),
        ("worker-data-engineer",
            json!({"files":["crates/a/tests/flow.rs","ui/a.test.tsx"],"fix":"Repair tests"}),
            json!(["testing"])),
        ("worker-data-engineer",
            json!({"files":["src/data.rs"],"fix":"Repair conversion","role":"devops-engineer"}),
            json!(["data"])),
        ("worker-qa-engineer",
            json!({"files":["scripts/build.sh"],"fix":"Fix the build script","role":"general"}),
            json!(["operations"])),
        ("worker-general",
            json!({"files":["packages/ui/src/view.tsx"],"fix":"Fix product UI"}),
            json!(["frontend"])),
    ] {
        let (_p, s, _) = setup();
        let mut e = evidence('b');
        finish_execution(&s, &mut e);
        s.write(|db| {
            db.execute("UPDATE runtime_nodes SET role=? WHERE node_id='n'", [original])?;
            if original == "worker-general" {
                cortex_core::lifecycle::plan::record_node_routing_tx(db, "t", "n",
                    &json!({"competencies":["testing"],"work_kind":"testing"}))?;
            }
            add_review_tx(db, "t", &e)?;
            let mut finding = finding.clone();
            finding["node"] = json!("n");
            finding["cross_cutting"] = json!(true);
            db.execute("UPDATE runtime_nodes SET status='completed',result_json=? WHERE node_id='_review_1'",
                [json!({"verdict":"REQUEST_CHANGES","summary":"s","report_ref":"review","findings":[finding]}).to_string()])?;
            cortex_core::lifecycle::review::advance_tx(db, "t")?;
            let own: String = db.query_row("SELECT role FROM runtime_nodes WHERE node_id='n'", [], |r| r.get(0))?;
            let fixed: String = db.query_row("SELECT role FROM runtime_nodes WHERE node_id='_fix_1_n'", [], |r| r.get(0))?;
            let plan: String = db.query_row("SELECT plan_json FROM runtime_tasks WHERE task_id='t'", [], |r| r.get(0))?;
            assert_eq!(own, original, "retained author identity");
            assert_eq!(fixed, "worker-general", "{finding}");
            let routing = cortex_core::lifecycle::plan::node_routing(&serde_json::from_str(&plan)?, "_fix_1_n");
            assert_eq!(routing["competencies"], expected, "{finding}");
            Ok(())
        }).unwrap();
    }
}

/// Owner store 2026-11-21: the owner answered "close as is" ten times while each
/// closing review re-raised the same declined reads as an owner decision. An owner
/// decision already routed and answered becomes a disclosed limit instead of
/// another question; a new one is still routed to the owner.
#[test]
fn an_answered_owner_decision_is_disclosed_not_asked_again() {
    for answered in [false, true] {
        let (_p, s, _) = setup();
        let mut e = evidence('b');
        finish_execution(&s, &mut e);
        let (state, disclosed) = s
            .write(|db| {
                let blocked = json!([{"item":"art/wide.json","blocker":"owner_decision","reason":"the owner declined the read"}]);
                event_tx(db, "t", "REVIEW_DEAD_END_ROUTED", json!({"review":"_review_0","revision":1,"blocked":blocked}))?;
                if answered {
                    event_tx(db, "t", "USER_ANSWER_RECEIVED", json!({"question_id":"pq_1"}))?;
                }
                add_review_tx(db, "t", &e)?;
                db.execute("UPDATE runtime_nodes SET status='completed',result_json=? WHERE node_id='_review_1'",
                    [json!({"verdict":"INCOMPLETE","summary":"s","report_ref":"review","findings":[],
                        "not_covered":[{"item":"art/wide.json","blocker":"owner_decision","reason":"the owner declined the read"}]}).to_string()])?;
                cortex_core::lifecycle::review::advance_tx(db, "t")?;
                let disclosed: i64 = db.query_row("SELECT count(*) FROM runtime_events WHERE type='REVIEW_LIMIT_DISCLOSED'", [], |r| r.get(0))?;
                Ok((cortex_core::lifecycle::domain::task_tx(db, "t")?.0, disclosed))
            })
            .unwrap();
        if answered {
            assert_eq!((state, disclosed), (TaskState::Verifying, 1));
        } else {
            assert_ne!(state, TaskState::Verifying);
            assert_eq!(disclosed, 0);
        }
    }
}

/// An owner's answer to a question is an owner message: an `allow-write: <path>` in it
/// lifts the owner's own `deny-write` of that path like the Remove control; an answer
/// naming another path lifts nothing. Rule files carry no default protection at all.
#[test]
fn an_allow_write_answer_lifts_that_owner_denial() {
    use cortex_core::lifecycle::{event_tx, steering};
    for (answer, lifted) in [
        ("One, and allow-write: src/free.rs", false),
        ("allow-write: src/secret.rs — разрешить правку", true),
    ] {
        let (_p, s, _) = setup();
        assert!(s.read(|db| steering::rule_file_protections(db, "t")).unwrap().is_empty());
        s.write(|db| {
            event_tx(db, "t", "USER_STEERING", json!({"text":"deny-write: src/secret.rs","request":"m1","deny_write":["src/secret.rs"],"delivery":"boundary"}))?;
            Ok(())
        })
        .unwrap();
        let denied = |s: &Store| -> bool {
            s.read(|db| steering::write_denials(db, "t")).unwrap().iter().any(|p| p == "src/secret.rs")
        };
        assert!(denied(&s));
        let q = ask(&s);
        s.write(|db| {
            assert!(answer_tx(db, "t", q["question_id"].as_str().unwrap(), answer, "text", None)?);
            Ok(())
        })
        .unwrap();
        assert_eq!(denied(&s), !lifted, "{answer}");
    }
}

/// Business store t_4797cc4c: the plan narrowed the goal to "preparation only, no
/// push", the review approved that stage, verification ran and the task completed
/// with the push undone. An approval that reports a goal outcome undelivered now
/// returns to the coordinator without any verification; only a full approval with
/// every outcome delivered is verified, exactly once, and completes.
#[test]
fn an_approved_stage_that_misses_the_goal_is_never_verified() {
    let outcomes = json!([{"id":"prepare","statement":"The changes are prepared"},
        {"id":"push","statement":"The changes are pushed to GitHub","deferred":"After preparation"}]);
    for delivered in [false, true] {
        let (_p, s, o) = setup();
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET plan_json=? WHERE task_id='t'",
                [json!({"verification":[["cargo","test","-p","x"]],"outcomes":outcomes,
                    "tasks":[{"id":"n","role":"worker-backend-dev","objective":"Work","dependencies":[],"scope":["src"],"delivers":["prepare"]}]}).to_string()])?;
            Ok(())
        })
        .unwrap();
        let mut e = evidence('b');
        finish_execution(&s, &mut e);
        s.write(|db| add_review_tx(db, "t", &e)).unwrap();
        e.artifacts.insert("review".into(), "e".repeat(64));
        let coverage = json!([{"id":"prepare","delivered":true},{"id":"push","delivered":delivered}]);
        s.write(|db| {
            bind(db, "_review_1", "ar", "xr", "cr")?;
            start_review_tx(db, "t", "ar", &e)?;
            publish(db, "ar", "review")?;
            // An approval must answer every goal outcome.
            assert_eq!(
                record_result_tx(db, "t", "ar",
                    &json!({"report_ref":"review","summary":"Approved","checks":[],"verdict":"APPROVE",
                        "goal_coverage":[{"id":"prepare","delivered":true}]}), &e)
                .unwrap_err()
                .code(),
                "review_goal_coverage_required"
            );
            record_result_tx(db, "t", "ar",
                &json!({"report_ref":"review","summary":"Approved","checks":[],"verdict":"APPROVE","goal_coverage":coverage}), &e)?;
            cortex_core::lifecycle::review::advance_tx(db, "t")?;
            Ok(())
        })
        .unwrap();
        let verifications = |s: &Store| -> i64 {
            s.read(|db| Ok(db.query_row(
                "SELECT count(*) FROM runtime_events WHERE task_id='t' AND type LIKE 'VERIFICATION_%'", [], |r| r.get(0))?))
            .unwrap()
        };
        if !delivered {
            assert_eq!(state(&s), "RECOVERING");
            assert_eq!(verifications(&s), 0);
            let (reason, undelivered): (String, String) = s
                .read(|db| Ok((
                    db.query_row("SELECT json_extract(payload_json,'$.reason') FROM runtime_events WHERE type='RETURNED_TO_COORDINATOR'", [], |r| r.get(0))?,
                    db.query_row("SELECT json_extract(payload_json,'$.undelivered') FROM runtime_events WHERE type='REVIEW_GOAL_NOT_MET'", [], |r| r.get(0))?,
                )))
                .unwrap();
            assert_eq!(reason, "goal_not_met");
            assert!(undelivered.contains("push"), "{undelivered}");
            // The next plan keeps the outcomes and the verification that never ran.
            s.write(|db| {
                settle_recovery_tx(db, "t", TaskState::Planning)?;
                db.execute("UPDATE capacity_reservations SET state='released'", [])?;
                let delivered_now = json!([{"id":"prepare","statement":"The changes are prepared"},{"id":"push","statement":"The changes are pushed to GitHub"}]);
                let next = json!({"verification":[["cargo","test"]],"outcomes":delivered_now,
                    "tasks":[{"id":"push","role":"devops-engineer","objective":"Push","dependencies":[],"scope":["."],"delivers":["prepare","push"],"competencies":["operations"]}]});
                // A plan that leaves the owed command out is accepted: the runtime appends it.
                let accepted = plan::accept_tx(db, "t", "c", "next", &next)?;
                assert_eq!(accepted["verification_kept_by_runtime"], json!([{"argv":["cargo","test","-p","x"],"reason":"goal_not_met"}]));
                let stored: String = db.query_row("SELECT plan_json FROM runtime_tasks WHERE task_id='t'", [], |r| r.get(0))?;
                let stored: serde_json::Value = serde_json::from_str(&stored)?;
                assert_eq!(stored["verification"], json!([["cargo","test"],["cargo","test","-p","x"]]));
                Ok(())
            })
            .unwrap();
            continue;
        }
        assert_eq!(state(&s), "VERIFYING");
        s.write(|db| {
            retain_check_tx(db, "t", &o, "check", &["cargo".into(), "test".into(), "-p".into(), "x".into()],
                &CommandObservation { complete: true, exit_code: Some(0), receipt_ref: "fake-executor:1".into() }, &e)?;
            assert!(record_verification_tx(db, "t", &["check".into()], &e)?);
            reconciliation::submit_synthesis_tx(db, "t", &json!({"text":"Done","outcomes":[
                {"id":"prepare","status":"done","evidence":"r"},{"id":"push","status":"done","evidence":"review"}]}))?;
            Ok(())
        })
        .unwrap();
        assert_eq!(verifications(&s), 1);
        e.workers.insert("xa".into(), ProcessObservation::ConfirmedExited);
        e.workers.insert("xr".into(), ProcessObservation::ConfirmedExited);
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET synthesis='Unbound text' WHERE task_id='t'", [])?;
            assert_eq!(complete_tx(db, "t", &e).unwrap_err().code(), "completion_goal_not_met");
            db.execute("UPDATE runtime_tasks SET synthesis='Done' WHERE task_id='t'", [])?;
            Ok(())
        }).unwrap();
        s.write(|db| complete_tx(db, "t", &e)).unwrap();
        assert_eq!(state(&s), "COMPLETED");
    }
}

/// A worker whose objective is only partly done names what remains and may submit
/// that only at a dead end; the node then does not count as done.
#[test]
fn a_partly_done_objective_needs_a_dead_end_and_is_not_a_success() {
    let (_p, s, _) = setup();
    let mut e = evidence('b');
    e.artifacts.insert("r".into(), "f".repeat(64));
    s.write(|db| {
        publish(db, "a", "r")?;
        let partial = json!({"report_ref":"r","summary":"half","checks":[],"objective_status":"partial","remaining":["Push the branch"]});
        assert_eq!(
            record_result_tx(db, "t", "a", &partial, &e).unwrap_err().code(),
            "objective_partial_blocker_required"
        );
        let mut empty = partial.clone();
        empty["remaining"] = json!([]);
        empty["assignment_outcome"] = json!("failed");
        empty["blocker"] = json!({"kind":"missing_access","detail":"No push credentials"});
        assert_eq!(
            record_result_tx(db, "t", "a", &empty, &e).unwrap_err().code(),
            "objective_partial_blocker_required"
        );
        let mut blocked = partial.clone();
        blocked["assignment_outcome"] = json!("failed");
        blocked["blocker"] = json!({"kind":"missing_access","detail":"No push credentials"});
        record_result_tx(db, "t", "a", &blocked, &e)?;
        let status: String = db.query_row("SELECT status FROM runtime_nodes WHERE node_id='n'", [], |r| r.get(0))?;
        assert_eq!(status, "failed");
        Ok(())
    })
    .unwrap();
}

/// A superseded plan review (an earlier revision of the plan) never decides
/// completion: only the newest plan review's checks count.
#[test]
fn only_the_newest_plan_review_decides_worker_check_completion() {
    for newest_fails in [false, true] {
        let (_p, s, o) = setup();
        let mut e = evidence('a');
        finish_execution(&s, &mut e);
        let failing = json!({"name":"Read-only source search","outcome":"failed"});
        s.write(|db| {
            // An older plan review (superseded by a revision) with a failed check,
            // then the newest one: approved, bound to the checked-out project.
            for (node, checks) in [
                ("_plan_review_1", json!([failing])),
                ("_plan_review_2", if newest_fails { json!([failing]) } else { json!([]) }),
            ] {
                db.execute(
                    "INSERT INTO runtime_nodes VALUES ('t',1,?,'worker-code-reviewer','Review','[]','[]','review','completed',NULL,?)",
                    (node, json!({"verdict":"APPROVE","report_ref":"r","report_digest":"f".repeat(64),
                        "project_digest":"a".repeat(64),"checks":checks}).to_string()),
                )?;
            }
            cortex_core::lifecycle::event_tx(db, "t", "WORKER_COMPLETED",
                json!({"node":"_plan_review_2","result":{"report_ref":"r"}}))?;
            db.execute("UPDATE runtime_tasks SET state='VERIFYING' WHERE task_id='t'", [])?;
            Ok(())
        })
        .unwrap();
        verified(&s, &o, &e);
        e.workers
            .insert("xa".into(), ProcessObservation::ConfirmedExited);
        let result = s.write(|db| complete_tx(db, "t", &e));
        if newest_fails {
            assert_eq!(result.unwrap_err().code(), "completion_unresolved_worker_check");
        } else {
            assert!(result.is_ok(), "{result:?}");
            assert_eq!(state(&s), "COMPLETED");
        }
    }
}
