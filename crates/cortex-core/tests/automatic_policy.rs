//! Policy admission and lifecycle on the production SQLite writer and read model.
mod support;
use cortex_core::{
    lifecycle::{self, automatic, completion, domain, plan, review, MAX_VERIFICATION_REPAIRS},
    ownership::OwnerEpoch,
    ports::CommandObservation,
    projections::ProjectionReader,
    workspace::{self, NativeExecutionPolicy},
    OpenMode, Store, TaskState,
};
use serde_json::{json, Value};
use support::Project;
use workspace_protocol::core::{CoreQuery, Target};

fn draft(s: &Store, p: &Project) -> Value {
    s.write(|db| {
        db.execute(
            "INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)",
            [p.0.to_str()],
        )?;
        Ok(())
    })
    .unwrap();
    let draft = workspace::create_draft(
        s,
        "p",
        "owner",
        &json!({
            "protocol":{"major":1,"minor":8},"requestId":"draft","projectId":"p","actorId":"owner",
            "operation":"tasks.create","target":"p","expectedVersion":"0","ownerEpoch":null,
            "payload":{"description":"Implement the change","checkout":{"kind":"main"}}
        }),
    )
    .unwrap()
    .receipt;
    json!({
        "protocol":{"major":1,"minor":8},"requestId":"start","projectId":"p","actorId":"owner",
        "operation":"tasks.start","target":draft["target"],"expectedVersion":draft["entityVersion"],
        "ownerEpoch":null,"payload":{}
    })
}

fn start(enabled: bool) -> (Project, Store, String, Value) {
    let p = Project::new();
    let s = p.open();
    let start = draft(&s, &p);
    let receipt = workspace::start_native_input_with_policy(
        &s,
        "p",
        "owner",
        &start,
        None,
        None,
        enabled.then_some(NativeExecutionPolicy::AutomaticContinuation),
    )
    .unwrap();
    (p, s, receipt["task"].as_str().unwrap().into(), start)
}

fn test_plan() -> Value {
    json!({"tasks":[{"id":"work","role":"backend-dev","objective":"Correct the implementation",
        "dependencies":[],"scope":["src"]}],"verification":[["cargo","test"]]})
}

fn event_count(s: &Store, task: &str, kind: &str) -> i64 {
    s.read(|db| {
        Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id=? AND type=?",
            (task, kind),
            |r| r.get(0),
        )?)
    })
    .unwrap()
}

fn review_case(s: &Store, task: &str, verdict: &str) {
    let rev: i64 = s.write(|db| {
        let revision = domain::task_tx(db, task)?.1 + 1;
        db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=?,plan_json=? WHERE task_id=?",
            (revision, test_plan().to_string(), task))?;
        let result = json!({"verdict":verdict, "report_ref":format!("review-{revision}"),
            "project_digest":"a".repeat(64),"findings":[{"node":"work","fix":"Correct the boundary"}]});
        db.execute("INSERT INTO runtime_nodes VALUES (?,?,'review','worker-code-reviewer','Review','[]','[\"src\"]','review','completed',?,?)",
            (task, revision, format!("review-{revision}"), result.to_string()))?;
        lifecycle::event_tx(db, task, "REVIEW_STARTED", json!({"revision":revision}))?;
        lifecycle::event_tx(db, task, "WORKER_COMPLETED", json!({"result":result}))?;
        review::advance_tx(db, task)?;
        Ok(revision)
    }).unwrap();
    assert!(rev > 0);
}

#[test]
fn a_review_with_open_items_is_refused_and_named_dead_ends_go_to_automatic_recovery() {
    let (_project, store, task, _) = start(true);
    store.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=1,plan_json=? WHERE task_id=?",
            (test_plan().to_string(), &task))?;
        db.execute("INSERT INTO runtime_nodes VALUES (?,1,'review','worker-code-reviewer','Review','[]','[]','review','running','first',NULL)",
            [&task])?;
        // A part the reviewer can still cover is not a result: it is refused with
        // exactly that part, and the assignment stays the reviewer's.
        let mut result = json!({"verdict":"INCOMPLETE","report_ref":"first","checks":[],
            "not_covered":[{"item":"README.md","reason":"ran out of time"},
                {"item":"docs/install.md","blocker":"missing_access","reason":"the install host is not reachable from the review"}]});
        assert_eq!(completion::outstanding_tx(db, &task, "first", "review", &result)?,
            Some(("review_coverage_unfinished", vec!["README.md".to_owned()])));
        // An invented category is no dead end either.
        result["not_covered"][0]["blocker"] = json!("no_time");
        assert_eq!(completion::outstanding_tx(db, &task, "first", "review", &result)?.unwrap().0,
            "review_coverage_unfinished");
        result["not_covered"][0]["blocker"] = json!("other_role_work");
        assert_eq!(completion::outstanding_tx(db, &task, "first", "review", &result)?, None);
        // A BLOCK names its confirmed problems; a failed assignment names its dead end.
        let block = json!({"verdict":"BLOCK","report_ref":"first","checks":[],"findings":[]});
        assert_eq!(completion::outstanding_tx(db, &task, "first", "review", &block)?.unwrap().0,
            "review_block_findings_required");
        let mut failed = json!({"assignment_outcome":"failed","report_ref":"w","checks":[]});
        assert_eq!(completion::outstanding_tx(db, &task, "w", "execution", &failed)?.unwrap().0,
            "assignment_blocker_required");
        failed["blocker"] = json!({"kind":"owner_decision","detail":"  "});
        assert!(completion::outstanding_tx(db, &task, "w", "execution", &failed)?.is_some());
        failed["blocker"] = json!({"kind":"owner_decision","detail":"Which API version to keep is the owner's choice."});
        assert_eq!(completion::outstanding_tx(db, &task, "w", "execution", &failed)?, None);
        // The finished review with only named dead ends goes the Automatic way: the
        // coordinator first, never a second reviewer for the same bytes.
        result["project_digest"] = json!("a".repeat(64));
        db.execute("UPDATE runtime_nodes SET status='completed',result_json=? WHERE task_id=? AND node_id='review'",
            (result.to_string(), &task))?;
        review::advance_tx(db, &task)
    }).unwrap();
    assert_eq!(event_count(&store, &task, "REVIEW_RERUN"), 0);
    assert_eq!(event_count(&store, &task, "REVIEW_DEAD_END_ROUTED"), 1);
    // The named dead end is missing access: it waits for the owner's action (no rung used).
    assert_eq!(event_count(&store, &task, "AUTOMATIC_CONTINUATION"), 0);
    assert_eq!(event_count(&store, &task, "OWNER_ACTION_REQUIRED"), 1);
    assert_eq!(support::count(&store, "pending_questions"), 0);
    let reviews: i64 = store
        .read(|db| {
            Ok(db.query_row(
                "SELECT count(*) FROM runtime_nodes WHERE task_id=? AND stage='review'",
                [&task],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(reviews, 1);
    let status = store
        .read(|db| automatic::projection(db, &task))
        .unwrap()
        .unwrap();
    assert_eq!(status["pause"]["kind"], "obstacle");
    assert_eq!(status["pause"]["reason"], "review_incomplete");
}

#[test]
fn automatic_policy_is_host_admitted_and_bound_to_start_replay() {
    let (p, s, task, cmd) = start(true);
    let policy = Some(NativeExecutionPolicy::AutomaticContinuation);
    assert!(s.read(|db| automatic::enabled(db, &task)).unwrap());
    assert_eq!(event_count(&s, &task, "AUTOMATIC_POLICY_BOUND"), 1);
    assert!(
        workspace::replay_native_start_with_policy(&s, "p", "owner", &cmd, policy)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        workspace::replay_native_start(&s, "p", "owner", &cmd)
            .unwrap_err()
            .code(),
        "payload_conflict"
    );

    let mut model = test_plan();
    model["executionPolicy"] = json!("manual");
    assert!(s
        .write(|db| plan::accept_tx(db, &task, "coordinator", "fake-policy", &model))
        .is_err());
    assert_eq!(event_count(&s, &task, "AUTOMATIC_POLICY_BOUND"), 1);
    drop(s);
    // There is no migration or write-on-read for the event-backed field.
    let s = Store::open(p.key(), OpenMode::InspectImmutable).unwrap();
    assert!(s.read(|db| automatic::enabled(db, &task)).unwrap());

    let p = Project::new();
    let s = p.open();
    let mut cmd = draft(&s, &p);
    cmd["payload"]["policy"] = json!("automatic_continuation");
    assert_eq!(
        workspace::start_native(&s, "p", "owner", &cmd)
            .unwrap_err()
            .code(),
        "invalid_workspace_request"
    );
    assert_eq!(support::count(&s, "runtime_tasks"), 0);
}

#[test]
fn blocked_review_and_failed_verification_continue_every_task_without_asking() {
    for enabled in [false, true] {
        let (_p, s, task, _) = start(enabled);
        review_case(&s, &task, "BLOCK");
        let status = s.read(|db| automatic::projection(db, &task)).unwrap();
        assert_eq!(status.unwrap()["lastContinuationReason"], "review_blocked");
        let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
        assert_eq!(owed.evidence["reports"][0]["reportId"], "review-1");
        assert_eq!(owed.evidence["strategy"], "repair");
        assert_eq!(event_count(&s, &task, "ASK_USER_REQUIRED"), 0);
        assert_eq!(support::count(&s, "pending_questions"), 0);

        let (_p, s, task, _) = start(enabled);
        let snapshot = completion::EvidenceSnapshot {
            project_digest: Some("a".repeat(64)),
            ..Default::default()
        };
        let owner = OwnerEpoch {
            owner: "owner".into(),
            epoch: 1,
        };
        for i in 0..=MAX_VERIFICATION_REPAIRS {
            review_case(&s, &task, "APPROVE");
            s.write(|db| {
                let receipt = format!("check-{i}");
                completion::retain_check_tx(
                    db,
                    &task,
                    &owner,
                    &receipt,
                    &["cargo".into(), "test".into()],
                    &CommandObservation {
                        complete: true,
                        exit_code: Some(1),
                        receipt_ref: receipt.clone(),
                    },
                    &snapshot,
                )?;
                assert!(!completion::record_verification_tx(
                    db,
                    &task,
                    &[receipt],
                    &snapshot
                )?);
                Ok(())
            })
            .unwrap();
        }
        assert_eq!(event_count(&s, &task, "ASK_USER_REQUIRED"), 0);
        assert_eq!(support::count(&s, "pending_questions"), 0);
        let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
        assert_eq!(owed.evidence["reason"], "verification_failed");
        assert_eq!(owed.evidence["events"][0]["type"], "VERIFICATION_FAILED");
        s.write(|db| automatic::resume_tx(db, &task, owed.event))
            .unwrap()
            .unwrap();
        let mut replacement = test_plan();
        replacement["verification"] = json!([["cargo", "check"]]);
        let dropped = s.write(|db| plan::accept_tx(db, &task, "coordinator", "skip", &replacement));
        // A failed check is never dropped from the repair plan.
        assert_eq!(
            dropped.unwrap_err().code(),
            if enabled {
                "automatic_required_check_missing"
            } else {
                "verification_failed_check_missing"
            }
        );
        s.write(|db| plan::accept_tx(db, &task, "coordinator", "repair", &test_plan()))
            .unwrap();
        // The resumed rung gets its own direct repairs again before the next change of
        // approach: one more failed check returns to planning, not to a continuation.
        review_case(&s, &task, "APPROVE");
        s.write(|db| {
            completion::retain_check_tx(
                db,
                &task,
                &owner,
                "after-resume",
                &["cargo".into(), "test".into()],
                &CommandObservation {
                    complete: true,
                    exit_code: Some(1),
                    receipt_ref: "after-resume".into(),
                },
                &snapshot,
            )?;
            completion::record_verification_tx(db, &task, &["after-resume".into()], &snapshot)?;
            assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Planning);
            Ok(())
        })
        .unwrap();
        assert_eq!(event_count(&s, &task, "AUTOMATIC_CONTINUATION"), 1);
    }
}

/// No continuation cap: every stop continues on the strategy ladder (repair, escalated
/// routes, the senior consultant, another role) without pausing for the owner. Once
/// the same stop used every rung and cannot be disclosed, the runtime asks the owner
/// one question itself (`LOOP_BROKEN` action `ask_owner`, no model call); an answer
/// to try again starts a new round. The consultant joins the plan on each `consult`
/// rung.
#[test]
fn the_strategy_ladder_continues_without_a_cap_and_consults_on_its_rung() {
    for enabled in [false, true] {
        let (_p, s, task, _) = start(enabled);
        for n in 1..=4i64 {
            s.write(|db| {
                domain::stop_tx(db, &task, "coordinator_no_progress_limit")?;
                domain::stop_tx(db, &task, "duplicate_stop")?;
                let owed = automatic::owed(db, &task)?.unwrap();
                assert!(!owed.external);
                assert!(owed.waiting.is_empty());
                assert_eq!(owed.evidence["count"], n);
                assert_eq!(owed.evidence["strategy"], automatic::strategy_for(n));
                assert_eq!(
                    owed.evidence["switch_coordinator"],
                    automatic::strategy_for(n) == "reassign"
                );
                assert_eq!(automatic::escalated(db, &task)?, n >= 2);
                automatic::resume_tx(db, &task, owed.event)?.unwrap();
                assert!(automatic::resume_tx(db, &task, owed.event)?.is_none());
                plan::accept_tx(db, &task, "coordinator", &format!("plan-{n}"), &test_plan())?;
                Ok(())
            })
            .unwrap();
            assert_eq!(
                event_count(&s, &task, "CONSULTATION_REQUIRED"),
                (n + 1) / 4,
                "continuation {n}"
            );
        }
        // Every rung was used for this same stop: one runtime question, no model call.
        let question = s
            .write(|db| {
                assert_eq!(
                    domain::stop_tx(db, &task, "coordinator_no_progress_limit")?,
                    TaskState::WaitingUser
                );
                assert!(automatic::owed(db, &task)?.is_none());
                Ok(db.query_row(
                    "SELECT id FROM pending_questions WHERE task_id=? AND status='open'",
                    [&task],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .unwrap();
        assert_eq!(event_count(&s, &task, "LOOP_BROKEN"), 1);
        assert_eq!(support::count(&s, "pending_questions"), 1);
        s.write(|db| {
            assert!(lifecycle::waits::answer_tx(
                db,
                &task,
                &question,
                "Retry now: start a new round of every strategy",
                "text",
                None
            )?);
            let owed = automatic::owed(db, &task)?.unwrap();
            assert_eq!(owed.evidence["owner_choice"], "retry_now");
            automatic::resume_tx(db, &task, owed.event)?.unwrap();
            plan::accept_tx(db, &task, "coordinator", "plan-5", &test_plan())?;
            // The resumed attempt was the new round's `repair`: the next stop escalates.
            domain::stop_tx(db, &task, "coordinator_no_progress_limit")?;
            assert_eq!(
                automatic::owed(db, &task)?.unwrap().evidence["strategy"],
                "escalate_route"
            );
            Ok(())
        })
        .unwrap();
        let v = s
            .read(|db| automatic::projection(db, &task))
            .unwrap()
            .unwrap();
        assert_eq!(v["continuationCount"], 6);
        assert!(v["pause"].is_null());
        assert_eq!(event_count(&s, &task, "AUTOMATIC_PAUSED"), 0);
    }
    assert_eq!(
        automatic::STRATEGIES,
        ["repair", "escalate_route", "consult", "reassign"]
    );
}

/// A degenerate model turn never runs on the same model again: its continuation is at
/// least an escalated rung and moves the coordinator to another model.
#[test]
fn a_degenerate_turn_continues_on_another_model_instead_of_pausing() {
    let (_p, s, task, _) = start(true);
    s.write(|db| {
        domain::stop_tx(db, &task, "native_turn_degenerate")?;
        let owed = automatic::owed(db, &task)?.unwrap();
        assert_eq!(owed.evidence["strategy"], "escalate_route");
        assert_eq!(owed.evidence["switch_coordinator"], true);
        assert!(automatic::escalated(db, &task)?);
        automatic::resume_tx(db, &task, owed.event)?.unwrap();
        plan::accept_tx(db, &task, "coordinator", "after-degenerate", &test_plan())?;
        domain::stop_tx(db, &task, "native_turn_degenerate")?;
        let owed = automatic::owed(db, &task)?.unwrap();
        assert_eq!(owed.evidence["count"], 2);
        assert_eq!(owed.evidence["switch_coordinator"], true);
        Ok(())
    })
    .unwrap();
    assert_eq!(event_count(&s, &task, "AUTOMATIC_PAUSED"), 0);
    assert_eq!(support::count(&s, "pending_questions"), 0);
}

/// A pause an earlier version recorded for the owner is resumed by the runtime itself.
#[test]
fn a_retired_owner_pause_is_owed_and_resumes_on_an_escalated_rung() {
    for kind in ["continuation_cap", "owner_blocker", "outcome_unknown"] {
        let (_p, s, task, _) = start(true);
        s.write(|db| {
            lifecycle::event_tx(
                db,
                &task,
                "AUTOMATIC_PAUSED",
                json!({"pause_kind":kind,"reason":"native_turn_degenerate","revision":0}),
            )?;
            db.execute(
                "UPDATE runtime_tasks SET state='RECOVERING',blocker='automatic_continuation_paused' WHERE task_id=?",
                [&task],
            )?;
            let owed = automatic::owed(db, &task)?.unwrap();
            assert!(!owed.external, "{kind}");
            assert_eq!(owed.evidence["strategy"], "escalate_route");
            assert!(automatic::resume_tx(db, &task, owed.event)?.is_some());
            assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Planning);
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn a_real_work_question_is_kept_and_replans_change_approach_without_a_limit() {
    let (p, s, task, _) = start(true);
    s.write(|db| {
        let question = json!({"question":"Which input format should be supported?",
            "recommended":1,"options":["JSON","YAML"]});
        lifecycle::skill_proposals::request(db, &p.0, &task, "coordinator", "work", &question)?;
        assert_eq!(
            domain::stop_tx(db, &task, "coordinator_no_progress_limit")?,
            TaskState::WaitingUser
        );
        assert!(automatic::owed(db, &task)?.is_none());
        Ok(())
    })
    .unwrap();
    assert_eq!(support::count(&s, "pending_questions"), 1);
    assert_eq!(event_count(&s, &task, "USER_QUESTION_REQUESTED"), 1);
    assert_eq!(event_count(&s, &task, "AUTOMATIC_CONTINUATION"), 0);

    for enabled in [false, true] {
        let (_p, s, task, _) = start(enabled);
        s.write(|db| {
            for i in 0..3 {
                let result = domain::propose_replan_tx(
                    db,
                    &task,
                    "coordinator",
                    &format!("replan-{i}"),
                    "replan_request",
                    &json!({"reason":"Revise the plan"}),
                )?;
                assert_eq!(result["decision"], "replan");
                domain::settle_recovery_tx(db, &task, TaskState::Planning)?;
            }
            // The fourth replan is accepted, as a senior consultation.
            let result = domain::propose_replan_tx(
                db,
                &task,
                "coordinator",
                "fourth",
                "replan_request",
                &json!({"reason":"Further repair"}),
            )?;
            assert_eq!(result["accepted"], true);
            assert_eq!(result["decision"], "senior_consultation");
            assert_eq!(result["strategy"], "consult");
            domain::settle_recovery_tx(db, &task, TaskState::Planning)?;
            let accepted = plan::accept_tx(db, &task, "coordinator", "consulted", &test_plan())?;
            assert_eq!(accepted["state"], "PLAN_READY");
            Ok(())
        })
        .unwrap();
        assert_eq!(event_count(&s, &task, "CONSULTATION_REQUIRED"), 1);
        assert_eq!(event_count(&s, &task, "AUTOMATIC_CONTINUATION"), 0);
        assert_eq!(support::count(&s, "pending_questions"), 0);
    }
}

/// An effect whose outcome is unknown never parks the task for the owner. A write of
/// a session that is gone is settled for re-verification and recorded in the
/// coordinator's context; a command that may still run waits for the host's process
/// evidence (the runtime's own wait). Nothing is replayed.
#[test]
fn an_unknown_effect_continues_with_recorded_uncertainty_and_no_owner_hold() {
    for enabled in [false, true] {
        let (_p, s, task, _) = start(enabled);
        s.write(|db| {
            db.execute(
                "INSERT INTO runtime_requests VALUES (?,'write','channel','project_write','-',?)",
                (
                    &task,
                    json!({"complete":false,"path":"src/lib.rs"}).to_string(),
                ),
            )?;
            domain::stop_tx(db, &task, "lost_worker_action_outcome_unknown")?;
            Ok(())
        })
        .unwrap();
        let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
        assert_eq!(owed.evidence["uncertain"], true);
        assert_eq!(owed.evidence["unknown_effects"][0]["path"], "src/lib.rs");
        let evidence = s
            .write(|db| automatic::resume_tx(db, &task, owed.event))
            .unwrap()
            .unwrap();
        assert_eq!(evidence["uncertainty"][0]["operation"], "project_write");
        assert_eq!(event_count(&s, &task, "RUNTIME_UNCERTAINTY_RECORDED"), 1);
        let receipt: Value = s
            .read(|db| {
                Ok(serde_json::from_str(&db.query_row(
                    "SELECT result_json FROM runtime_requests WHERE task_id=? AND request_key='write'",
                    [&task],
                    |r| r.get::<_, String>(0),
                )?)?)
            })
            .unwrap();
        assert_eq!(receipt["settled"], true);
        assert_eq!(receipt["outcome"], "unknown");
        assert_eq!(receipt["settled_by"], "runtime_reverify");
        assert_eq!(event_count(&s, &task, "AUTOMATIC_PAUSED"), 0);
        assert_eq!(support::count(&s, "pending_questions"), 0);
        assert_eq!(
            s.read(|db| Ok(db.query_row(
                "SELECT state FROM runtime_tasks WHERE task_id=?",
                [&task],
                |r| r.get::<_, String>(0)
            )?))
            .unwrap(),
            "PLANNING"
        );

        // A command receipt that may still run: the continuation stays owed while the
        // host settles it; it is never paused for the owner.
        let (_p, s, task, _) = start(enabled);
        s.write(|db| {
            db.execute(
                "INSERT INTO runtime_requests VALUES (?,'command','channel','command_run','-',?)",
                (&task, json!({"complete":false,"settled":false}).to_string()),
            )?;
            assert!(automatic::continue_unsettled_tx(
                db,
                &task,
                "native_outside_outcome_unknown"
            )?);
            Ok(())
        })
        .unwrap();
        let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
        assert_eq!(
            s.write(|db| automatic::resume_tx(db, &task, owed.event))
                .unwrap_err()
                .code(),
            "native_command_outcome_pending"
        );
        assert!(s.read(|db| automatic::owed(db, &task)).unwrap().is_some());
        s.write(|db| {
            db.execute(
                "UPDATE runtime_requests SET result_json=? WHERE task_id=? AND request_key='command'",
                (json!({"settled":true,"outcome":"unknown","exit_code":1}).to_string(), &task),
            )?;
            assert!(automatic::resume_tx(db, &task, owed.event)?.is_some());
            Ok(())
        })
        .unwrap();
        assert_eq!(event_count(&s, &task, "AUTOMATIC_PAUSED"), 0);
        assert_eq!(event_count(&s, &task, "AUTOMATIC_RESUMED"), 1);
    }
}

#[test]
fn a_settled_exit_continues_a_task_that_the_exit_held() {
    // Held only for an unconfirmed exit: continued once the host settled it.
    for enabled in [false, true] {
        let (_p, s, task, _) = start(enabled);
        s.write(|db| {
            db.execute(
                "UPDATE runtime_tasks SET state='RECOVERING',blocker='native_shutdown_unconfirmed' WHERE task_id=?",
                [&task],
            )?;
            assert!(automatic::owed(db, &task)?.is_none());
            assert!(automatic::exit_settled_tx(db, &task)?);
            Ok(())
        })
        .unwrap();
        let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
        assert_eq!(owed.evidence["reason"], "native_process_exit_settled");
    }
    // A process still unsettled: nothing changes until the host settles it.
    let (_p, s, task, _) = start(true);
    s.write(|db| {
        db.execute("INSERT INTO capacity_reservations VALUES ('r','c',?,NULL,'a','worker','ambiguous','0','0',NULL)", [&task])?;
        db.execute(
            "UPDATE runtime_tasks SET state='RECOVERING',blocker='native_shutdown_unconfirmed' WHERE task_id=?",
            [&task],
        )?;
        assert!(!automatic::exit_settled_tx(db, &task)?);
        Ok(())
    })
    .unwrap();
    assert_eq!(event_count(&s, &task, "AUTOMATIC_CONTINUATION"), 0);
    // A stop for an unconfirmed exit is itself a continuation that waits for the
    // process evidence, never an owner pause.
    let (_p, s, task, _) = start(true);
    s.write(|db| {
        db.execute("INSERT INTO capacity_reservations VALUES ('r','c',?,NULL,'a','worker','ambiguous','0','0',NULL)", [&task])?;
        domain::stop_tx(db, &task, "native_shutdown_unconfirmed")?;
        let owed = automatic::owed(db, &task)?.unwrap();
        assert_eq!(owed.evidence["uncertain"], true);
        assert_eq!(
            automatic::resume_tx(db, &task, owed.event).unwrap_err().code(),
            "lost_worker_termination_unconfirmed"
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(event_count(&s, &task, "AUTOMATIC_PAUSED"), 0);
}

#[test]
fn automatic_policy_durable_external_hold_projects_and_resumes_exactly_once() {
    let (p, s, task, start) = start(true);
    assert_eq!(
        s.write(|db| domain::stop_tx(db, &task, "engine_quota_exhausted"))
            .unwrap(),
        TaskState::Recovering
    );
    let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
    assert!(owed.external);
    drop(s);
    let s = p.open();
    let reader = ProjectionReader::new(
        Store::open(p.key(), OpenMode::LiveReadOnly).unwrap(),
        "p".into(),
        "owner".into(),
    )
    .unwrap();
    let task_id = start["target"].as_str().unwrap().to_owned();
    let reply = reader
        .query(CoreQuery::TaskGet(Target {
            id: task_id.clone(),
        }))
        .unwrap();
    let workspace_protocol::read::QueryReply::Task(row) = reply.data else {
        panic!("task missing")
    };
    assert_eq!(row.phase, "paused");
    let wire = serde_json::to_value(&row).unwrap();
    workspace_protocol::validate("Task", &wire).unwrap();
    assert_eq!(
        wire["automaticContinuation"]["pause"]["reason"],
        "engine_quota_exhausted"
    );
    s.write(|db| {
        assert!(automatic::resume_tx(db, &task, owed.event + 1)?.is_none());
        assert!(automatic::resume_tx(db, &task, owed.event)?.is_some());
        assert!(automatic::resume_tx(db, &task, owed.event)?.is_none());
        assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Planning);
        Ok(())
    })
    .unwrap();
    assert_eq!(event_count(&s, &task, "AUTOMATIC_RESUMED"), 1);
    assert_eq!(event_count(&s, &task, "AUTOMATIC_CONTINUATION"), 0);
    assert_eq!(support::count(&s, "pending_questions"), 0);
}

/// A fix and review round continues only on real progress. Two consecutive
/// reviews of one revision with the same findings, or with unchanged files, stop
/// once as a continuation on the next rung of the strategy ladder (every task),
/// instead of a third round. Changed files with different findings continue.
#[test]
fn identical_findings_or_unchanged_files_stop_the_fix_review_ping_pong() {
    let finding =
        |fix: &str| json!([{"node":"work","files":["src/lib.rs"],"cross_cutting":false,"fix":fix}]);
    for (case, second_digest, second_fix) in [
        ("repeated_findings", "b", "Handle  the EMPTY input."),
        ("unchanged_files", "a", "Also validate the length."),
        ("progress", "b", "Also validate the length."),
    ] {
        for enabled in [false, true] {
            let (_p, s, task, _) = start(enabled);
            s.write(|db| {
                db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=1,review_cycle=1,plan_json=? WHERE task_id=?",
                    (test_plan().to_string(), &task))?;
                db.execute("INSERT INTO runtime_nodes VALUES (?,1,'work','worker-backend-dev','Work','[]','[\"src\"]','execution','completed','w1','{}')", [&task])?;
                let first = json!({"verdict":"REQUEST_CHANGES","report_ref":"r1","project_digest":"a".repeat(64),
                    "findings":finding("Handle the empty input.")});
                db.execute("INSERT INTO runtime_nodes VALUES (?,1,'_review_1','worker-code-reviewer','Review','[\"work\"]','[\"**\"]','review','completed','rv1',?)",
                    (&task, first.to_string()))?;
                review::advance_tx(db, &task)?;
                assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Executing);
                // The fix ran; the second review of the same revision comes back.
                db.execute("UPDATE runtime_nodes SET status='completed',result_json='{}' WHERE task_id=? AND node_id='_fix_1_work'", [&task])?;
                db.execute("UPDATE runtime_tasks SET state='REVIEWING',review_cycle=2 WHERE task_id=?", [&task])?;
                let second = json!({"verdict":"REQUEST_CHANGES","report_ref":"r2","project_digest":second_digest.repeat(64),
                    "findings":finding(second_fix)});
                db.execute("INSERT INTO runtime_nodes VALUES (?,1,'_review_2','worker-code-reviewer','Review','[\"_fix_1_work\"]','[\"**\"]','review','completed','rv2',?)",
                    (&task, second.to_string()))?;
                review::advance_tx(db, &task)
            })
            .unwrap();
            let fixes: i64 = s.read(|db| Ok(db.query_row(
                "SELECT count(*) FROM runtime_nodes WHERE task_id=? AND node_id='_fix_2_work'", [&task], |r| r.get(0))?)).unwrap();
            let reason: Option<String> = s.read(|db| Ok(db.query_row(
                "SELECT json_extract(payload_json,'$.reason') FROM runtime_events WHERE task_id=? AND type='REVIEW_NO_PROGRESS'",
                [&task], |r| r.get(0)).ok())).unwrap();
            if case == "progress" {
                assert_eq!((fixes, reason), (1, None), "{case} {enabled}");
                continue;
            }
            assert_eq!(fixes, 0, "{case} {enabled}");
            assert_eq!(reason.as_deref(), Some(case), "{enabled}");
            // Every task (manual or Automatic) continues with a changed approach; the
            // owner is asked nothing.
            let status = s
                .read(|db| automatic::projection(db, &task))
                .unwrap()
                .unwrap();
            assert_eq!(status["lastContinuationReason"], "review_no_progress");
            assert_eq!(support::count(&s, "pending_questions"), 0);
            assert_eq!(event_count(&s, &task, "ASK_USER_REQUIRED"), 0);
            assert!(s.read(|db| automatic::owed(db, &task)).unwrap().is_some());
        }
    }
}

/// A reviewer that names the same item it cannot reach with its tools in two dead
/// ends (alongside other dead ends) moves the continuation to the `reassign` rung;
/// it never pauses the task for the owner.
#[test]
fn a_repeated_unreachable_review_item_takes_the_reassign_rung() {
    let (_p, s, task, _) = start(true);
    let dead_end = |item: &str| {
        s.write(|db| {
            db.execute(
                "UPDATE runtime_tasks SET state='PLANNING',blocker=NULL WHERE task_id=?",
                [&task],
            )?;
            lifecycle::event_tx(
                db,
                &task,
                "REVIEW_DEAD_END_ROUTED",
                json!({"verdict":"INCOMPLETE","blocked":[
                    {"item":item,"blocker":"tool_unavailable","reason":"cannot read it"},
                    {"item":"docs/a.md","blocker":"owner_decision","reason":"owner"}]}),
            )?;
            domain::stop_tx(db, &task, "review_incomplete")?;
            Ok(())
        })
        .unwrap()
    };
    dead_end("assets/font.css (embedded base64)");
    assert_eq!(event_count(&s, &task, "AUTOMATIC_CONTINUATION"), 1);
    // The same item, described differently: another role takes it.
    dead_end("assets/font.css: lines too long");
    assert_eq!(event_count(&s, &task, "AUTOMATIC_CONTINUATION"), 2);
    assert_eq!(event_count(&s, &task, "AUTOMATIC_PAUSED"), 0);
    let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
    assert_eq!(owed.evidence["strategy"], "reassign");
    assert_eq!(owed.evidence["review_dead_end"]["item"], "assets/font.css");
    assert_eq!(owed.evidence["review_dead_end"]["count"], 2);
}

/// A review whose only gaps are items its tools cannot reach, with nothing to repair,
/// records them as disclosed unverified limits and continues to verification.
#[test]
fn a_tool_unavailable_review_dead_end_becomes_a_disclosed_limit() {
    for enabled in [false, true] {
        let (_p, s, task, _) = start(enabled);
        s.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=1,plan_json=? WHERE task_id=?",
                (test_plan().to_string(), &task))?;
            let result = json!({"verdict":"INCOMPLETE","report_ref":"r","checks":[],
                "project_digest":"a".repeat(64),
                "not_covered":[{"item":"assets/font.woff2","blocker":"tool_unavailable","reason":"binary file"}]});
            db.execute("INSERT INTO runtime_nodes VALUES (?,1,'review','worker-code-reviewer','Review','[]','[]','review','completed','r',?)",
                (&task, result.to_string()))?;
            review::advance_tx(db, &task)?;
            assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Verifying);
            Ok(())
        })
        .unwrap();
        assert_eq!(event_count(&s, &task, "REVIEW_LIMIT_DISCLOSED"), 1);
        assert_eq!(event_count(&s, &task, "AUTOMATIC_CONTINUATION"), 0);
        assert_eq!(support::count(&s, "pending_questions"), 0);
        let result: Value = s
            .read(|db| {
                Ok(serde_json::from_str(&db.query_row(
                    "SELECT result_json FROM runtime_nodes WHERE task_id=? AND node_id='review'",
                    [&task],
                    |r| r.get::<_, String>(0),
                )?)?)
            })
            .unwrap();
        assert_eq!(result["review_limit_disclosed"], true);
        assert_eq!(result["checks"][0]["outcome"], "unverified");
        assert!(result["checks"][0]["name"]
            .as_str()
            .unwrap()
            .contains("assets/font.woff2"));
    }
}

/// Owner 2026-11-21: once the ladder escalated, every later worker of a long task
/// ran on the senior models, and an infrastructure retry (a lost session) escalated
/// too. Only the work that fell short escalates now.
#[test]
fn only_work_that_fell_short_routes_escalated() {
    let (_p, s, task, _) = start(true);
    let esc = |node: &str, revision: i64| {
        s.read(|db| automatic::node_escalated(db, &task, node, revision))
            .unwrap()
    };
    let event = |kind: &str, payload: Value| {
        s.write(|db| lifecycle::event_tx(db, &task, kind, payload))
            .unwrap();
    };
    // A first dispatch and an infrastructure retry are not escalated.
    event(
        "ASSIGNMENT_CREATED",
        json!({"assignment_ref":"r1","node":"qa","revision":3}),
    );
    event(
        "ASSIGNMENT_RETRIED",
        json!({"node":"qa","failure_class":"worker_lost","previous_assignment":"r1","revision":3}),
    );
    assert!(!esc("qa", 3));
    // A failure that names another role's work is not the model's.
    event(
        "ASSIGNMENT_CREATED",
        json!({"assignment_ref":"r2","node":"qa","revision":3}),
    );
    event(
        "WORKER_FAILED",
        json!({"assignment_ref":"r2","result":{"assignment_outcome":"failed","blocker":{"kind":"other_role_work","detail":"d"}}}),
    );
    assert!(!esc("qa", 3));
    // Its own failure escalates the node and its first review fix, nothing else.
    event(
        "ASSIGNMENT_CREATED",
        json!({"assignment_ref":"r3","node":"api","revision":3}),
    );
    event(
        "WORKER_FAILED",
        json!({"assignment_ref":"r3","result":{"assignment_outcome":"failed","blocker":{"kind":"tool_unavailable","detail":"d"}}}),
    );
    assert!(esc("api", 3));
    assert!(esc("_fix_1_api", 3));
    assert!(!esc("docs", 3));
    assert!(!esc("_fix_1_docs", 3));
    // A second review cycle's fix escalates.
    assert!(esc("_fix_2_docs", 3));
    // A quality retry (no infrastructure class) escalates its node in its revision.
    event(
        "ASSIGNMENT_RETRIED",
        json!({"node":"ui","failure_class":"indeterminate","revision":3}),
    );
    assert!(esc("ui", 3));
    assert!(!esc("ui", 4));
    // An escalating rung escalates only the next plan revision.
    event(
        "AUTOMATIC_CONTINUATION",
        json!({"strategy":"escalate_route","count":1}),
    );
    event("EXECUTION_BASELINE", json!({"revision":4}));
    assert!(esc("docs", 4));
    event("EXECUTION_BASELINE", json!({"revision":5}));
    assert!(!esc("docs", 5));
}
