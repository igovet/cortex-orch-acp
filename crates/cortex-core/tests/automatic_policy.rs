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
    assert_eq!(event_count(&store, &task, "AUTOMATIC_CONTINUATION"), 1);
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
    assert_eq!(status["lastContinuationReason"], "review_incomplete");
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
fn automatic_policy_changes_blocked_review_and_verification_stops_only_when_bound() {
    for enabled in [false, true] {
        let (_p, s, task, _) = start(enabled);
        review_case(&s, &task, "BLOCK");
        let status = s.read(|db| automatic::projection(db, &task)).unwrap();
        if enabled {
            assert_eq!(status.unwrap()["lastContinuationReason"], "review_blocked");
            let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
            assert_eq!(owed.evidence["reports"][0]["reportId"], "review-1");
            assert_eq!(event_count(&s, &task, "ASK_USER_REQUIRED"), 0);
        } else {
            assert!(status.is_none());
            assert_eq!(event_count(&s, &task, "ASK_USER_REQUIRED"), 1);
            s.write(|db| domain::stop_tx(db, &task, "review_blocked"))
                .unwrap();
            assert_eq!(support::count(&s, "pending_questions"), 1);
        }

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
        if enabled {
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
            assert_eq!(
                s.write(|db| plan::accept_tx(db, &task, "coordinator", "skip", &replacement))
                    .unwrap_err()
                    .code(),
                "automatic_required_check_missing"
            );
            s.write(|db| plan::accept_tx(db, &task, "coordinator", "repair", &test_plan()))
                .unwrap();
        } else {
            assert_eq!(event_count(&s, &task, "ASK_USER_REQUIRED"), 1);
            assert_eq!(event_count(&s, &task, "AUTOMATIC_CONTINUATION"), 0);
        }
    }
}

#[test]
fn automatic_policy_consults_once_and_holds_at_hard_cap() {
    let (_p, s, task, _) = start(true);
    for n in 1..=8 {
        s.write(|db| {
            domain::stop_tx(db, &task, "coordinator_no_progress_limit")?;
            domain::stop_tx(db, &task, "duplicate_stop")?;
            let owed = automatic::owed(db, &task)?.unwrap();
            assert_eq!(owed.evidence["count"], n);
            automatic::resume_tx(db, &task, owed.event)?.unwrap();
            assert!(automatic::resume_tx(db, &task, owed.event)?.is_none());
            plan::accept_tx(db, &task, "coordinator", &format!("plan-{n}"), &test_plan())?;
            Ok(())
        })
        .unwrap();
        assert_eq!(
            event_count(&s, &task, "CONSULTATION_REQUIRED"),
            i64::from(n >= 4)
        );
    }
    s.write(|db| domain::stop_tx(db, &task, "coordinator_no_progress_limit"))
        .unwrap();
    let v = s
        .read(|db| {
            assert!(automatic::owed(db, &task)?.is_none());
            automatic::projection(db, &task)
        })
        .unwrap()
        .unwrap();
    assert_eq!(v["continuationCount"], 8);
    assert_eq!(v["pause"]["kind"], "continuation_cap");
    assert_eq!(support::count(&s, "pending_questions"), 0);

    let owner = OwnerEpoch {
        owner: "owner".into(),
        epoch: 1,
    };
    s.write(|db| {
        let version = domain::pause_version_tx(db, &task)?;
        domain::owner_pause_tx(db, &task, &owner, "resume-cap", true, version)?;
        assert!(automatic::held(db, &task)?);
        domain::native_continue_tx(db, &task)?;
        assert!(!automatic::held(db, &task)?);
        plan::accept_tx(db, &task, "coordinator", "after-cap", &test_plan())?;
        domain::stop_tx(db, &task, "coordinator_no_progress_limit")?;
        let owed = automatic::owed(db, &task)?.unwrap();
        assert_eq!(owed.evidence["count"], 9);
        Ok(())
    })
    .unwrap();
    assert_eq!(event_count(&s, &task, "AUTOMATIC_CAP_RENEWED"), 1);
    assert_eq!(event_count(&s, &task, "CONSULTATION_REQUIRED"), 1);
}

#[test]
fn automatic_policy_replan_limit_and_genuine_work_question_are_independent() {
    let (p, s, task, _) = start(true);
    s.write(|db| {
        let question = json!({"question":"Which input format should be supported?",
            "options":["JSON","YAML"]});
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

    let (_p, s, task, _) = start(true);
    s.write(|db| {
        for i in 0..3 {
            domain::propose_replan_tx(
                db,
                &task,
                "coordinator",
                &format!("replan-{i}"),
                "replan_request",
                &json!({"reason":"Revise the plan"}),
            )?;
            domain::settle_recovery_tx(db, &task, TaskState::Planning)?;
        }
        let result = domain::propose_replan_tx(
            db,
            &task,
            "coordinator",
            "limit",
            "replan_request",
            &json!({"reason":"Further repair"}),
        )?;
        assert_eq!(result["accepted"], false);
        let owed = automatic::owed(db, &task)?.unwrap();
        assert_eq!(owed.evidence["reason"], "replan_limit");
        automatic::resume_tx(db, &task, owed.event)?.unwrap();
        let result = domain::propose_replan_tx(
            db,
            &task,
            "coordinator",
            "fresh",
            "replan_request",
            &json!({"reason":"Further repair after recovery"}),
        )?;
        assert_eq!(result["accepted"], true);
        Ok(())
    })
    .unwrap();
    assert_eq!(support::count(&s, "pending_questions"), 0);
}

#[test]
fn automatic_policy_never_replays_an_unresolved_effect() {
    let (_p, s, task, _) = start(true);
    s.write(|db| {
        db.execute(
            "INSERT INTO runtime_requests VALUES (?,'command','channel','command_run','-',?)",
            (&task, json!({"complete":false,"settled":false}).to_string()),
        )?;
        domain::stop_tx(db, &task, "worker_failed")?;
        Ok(())
    })
    .unwrap();
    let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
    let reason = s
        .write(|db| automatic::resume_tx(db, &task, owed.event))
        .unwrap_err()
        .code();
    assert_eq!(reason, "native_outside_outcome_unknown");
    s.write(|db| {
        assert!(automatic::pause_unsettled_tx(db, &task, reason)?);
        assert!(automatic::owed(db, &task)?.is_none());
        assert_eq!(
            automatic::projection(db, &task)?.unwrap()["pause"]["kind"],
            "outcome_unknown"
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(event_count(&s, &task, "AUTOMATIC_RESUMED"), 0);
    assert_eq!(support::count(&s, "pending_questions"), 0);
}

#[test]
fn a_settled_exit_continues_only_a_task_that_the_exit_alone_held() {
    // Paused only for an unconfirmed exit: continued once the host settled it.
    let (_p, s, task, _) = start(true);
    s.write(|db| {
        domain::stop_tx(db, &task, "native_shutdown_unconfirmed")?;
        assert!(automatic::owed(db, &task)?.is_none());
        assert!(automatic::exit_settled_tx(db, &task)?);
        Ok(())
    })
    .unwrap();
    let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
    assert_eq!(owed.evidence["reason"], "native_process_exit_settled");
    // Paused for an effect whose outcome is unknown: never continued by an exit.
    let (_p, s, task, _) = start(true);
    s.write(|db| {
        db.execute(
            "INSERT INTO runtime_requests VALUES (?,'command','channel','command_run','-',?)",
            (&task, json!({"complete":false,"settled":false}).to_string()),
        )?;
        domain::stop_tx(db, &task, "native_shutdown_unconfirmed")?;
        assert!(!automatic::exit_settled_tx(db, &task)?);
        assert!(automatic::pause_unsettled_tx(
            db,
            &task,
            "native_outside_outcome_unknown"
        )?);
        assert!(!automatic::exit_settled_tx(db, &task)?);
        Ok(())
    })
    .unwrap();
    assert_eq!(event_count(&s, &task, "AUTOMATIC_CONTINUATION"), 0);
    // A process still unsettled, or a task without the policy: nothing changes.
    let (_p, s, task, _) = start(true);
    s.write(|db| {
        db.execute("INSERT INTO capacity_reservations VALUES ('r','c',?,NULL,'a','worker','ambiguous','0','0',NULL)", [&task])?;
        domain::stop_tx(db, &task, "native_shutdown_unconfirmed")?;
        assert!(!automatic::exit_settled_tx(db, &task)?);
        Ok(())
    })
    .unwrap();
    let (_p, s, task, _) = start(false);
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='RECOVERING',blocker=NULL WHERE task_id=?",
            [&task],
        )?;
        assert!(!automatic::exit_settled_tx(db, &task)?);
        Ok(())
    })
    .unwrap();
    assert_eq!(event_count(&s, &task, "AUTOMATIC_CONTINUATION"), 0);
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
/// once and escalate by run mode (Manual: back to the coordinator first, which then
/// asks the owner; Automatic: the coordinator continuation, then the consultant),
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
            if enabled {
                let status = s
                    .read(|db| automatic::projection(db, &task))
                    .unwrap()
                    .unwrap();
                assert_eq!(status["lastContinuationReason"], "review_no_progress");
                assert_eq!(support::count(&s, "pending_questions"), 0);
            } else {
                // Manual: the coordinator gets it first; a repeat stop asks the owner.
                assert_eq!(event_count(&s, &task, "ASK_USER_REQUIRED"), 1);
                assert_eq!(support::count(&s, "pending_questions"), 0);
                s.write(|db| domain::stop_tx(db, &task, "review_no_progress"))
                    .unwrap();
                assert_eq!(support::count(&s, "pending_questions"), 1);
            }
        }
    }
}
