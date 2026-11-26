//! The generic loop guard and the per-fingerprint strategy ladder, on the production
//! SQLite writer. Event payloads copy the shapes recorded by real stores (verification
//! failures, review dead ends, owner pauses, restarts, sandbox receipts).
mod support;
use cortex_core::{
    lifecycle::{self, automatic, domain, loop_guard, plan},
    workspace::{self, NativeExecutionPolicy},
    Store, TaskState,
};
use rusqlite::Transaction;
use serde_json::{json, Value};
use support::Project;

fn start() -> (Project, Store, String) {
    let p = Project::new();
    let s = p.open();
    s.write(|db| {
        db.execute(
            "INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)",
            [p.0.to_str()],
        )?;
        Ok(())
    })
    .unwrap();
    let draft = workspace::create_draft(
        &s,
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
    let start = json!({
        "protocol":{"major":1,"minor":8},"requestId":"start","projectId":"p","actorId":"owner",
        "operation":"tasks.start","target":draft["target"],"expectedVersion":draft["entityVersion"],
        "ownerEpoch":null,"payload":{}
    });
    let receipt = workspace::start_native_input_with_policy(
        &s,
        "p",
        "owner",
        &start,
        None,
        None,
        Some(NativeExecutionPolicy::AutomaticContinuation),
    )
    .unwrap();
    let task = receipt["task"].as_str().unwrap().to_owned();
    (p, s, task)
}

fn test_plan() -> Value {
    json!({"tasks":[{"id":"work","role":"backend-dev","objective":"Correct the implementation",
        "dependencies":[],"scope":["src"]}],"verification":[["scripts/cortex-xtask.sh","test-fast"],["bash","-n","scripts/qualify-dev-deb.sh"]]})
}

fn count(s: &Store, task: &str, kind: &str) -> i64 {
    s.read(|db| {
        Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id=? AND type=?",
            (task, kind),
            |r| r.get(0),
        )?)
    })
    .unwrap()
}

fn latest(s: &Store, task: &str, kind: &str) -> Value {
    s.read(|db| {
        let raw: String = db.query_row(
            "SELECT payload_json FROM runtime_events WHERE task_id=? AND type=? ORDER BY id DESC LIMIT 1",
            (task, kind),
            |r| r.get(0),
        )?;
        Ok(serde_json::from_str(&raw)?)
    })
    .unwrap()
}

/// The real failed-verification shape (cortex store, `VERIFICATION_FAILED`), with a
/// checkout digest that changes on every attempt.
fn verification_failed(db: &Transaction<'_>, task: &str, attempt: i64) {
    let revision = domain::task_tx(db, task).unwrap().1;
    lifecycle::event_tx(
        db,
        task,
        "VERIFICATION_FAILED",
        json!({"checkout_digest":format!("{attempt:064}"),"checks":[
            {"argv":["scripts/cortex-xtask.sh","test-fast"],"complete":true,"exit_code":1,"failure":null,
             "owner_epoch":1,"receipt_ref":format!("native-command:{attempt:064x}"),"revision":revision,"signal":null},
            {"argv":["bash","-n","scripts/qualify-dev-deb.sh"],"complete":true,"exit_code":0,"failure":null,
             "owner_epoch":1,"receipt_ref":"native-command:d774bf42166b984776eb4","revision":revision,"signal":null}]}),
    )
    .unwrap();
}

/// Resume the owed continuation and accept a fresh plan: what every cycle passes
/// through (`AUTOMATIC_RESUMED`, `PLAN_ACCEPTED`, a new revision).
fn resume_and_plan(db: &Transaction<'_>, task: &str, n: i64) -> Value {
    let owed = automatic::owed(db, task).unwrap().unwrap();
    assert!(owed.waiting.is_empty(), "continuation {n} waits");
    automatic::resume_tx(db, task, owed.event).unwrap().unwrap();
    plan::accept_tx(db, task, "coordinator", &format!("plan-{n}"), &support::with_goal(&test_plan())).unwrap();
    owed.evidence
}

/// Walk the same verification failure over every rung (each cycle resumes and accepts
/// a fresh plan), then stop once more: the loop question. Returns its id and text.
fn exhaust(s: &Store, task: &str) -> (String, String) {
    let mut strategies = Vec::new();
    for n in 1..=4 {
        s.write(|db| {
            verification_failed(db, task, n);
            domain::stop_tx(db, task, "verification_failed")?;
            let evidence = resume_and_plan(db, task, n);
            strategies.push(evidence["strategy"].as_str().unwrap().to_owned());
            Ok(())
        })
        .unwrap();
    }
    // The window survived AUTOMATIC_RESUMED, PLAN_ACCEPTED and new revisions.
    assert_eq!(
        strategies,
        ["repair", "escalate_route", "consult", "reassign"]
    );
    s.write(|db| {
        verification_failed(db, task, 5);
        assert_eq!(
            domain::stop_tx(db, task, "verification_failed")?,
            TaskState::WaitingUser
        );
        // The task waits without any model call.
        assert!(automatic::owed(db, task)?.is_none());
        Ok(db.query_row(
            "SELECT id,text FROM pending_questions WHERE task_id=? AND status='open'",
            [task],
            |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
        )?)
    })
    .unwrap()
}

fn answer(s: &Store, task: &str, question: &str, text: &str) -> automatic::Owed {
    s.write(|db| {
        assert!(lifecycle::waits::answer_tx(
            db, task, question, text, "text", None
        )?);
        Ok(automatic::owed(db, task)?.unwrap())
    })
    .unwrap()
}

#[test]
fn the_same_stop_walks_every_rung_once_then_the_runtime_asks_the_owner_once() {
    let (_p, s, task) = start();
    // The owner works in Russian.
    s.write(|db| {
        lifecycle::event_tx(
            db,
            &task,
            "NATIVE_TASK_SETTINGS",
            json!({"settings":{"language":"ru"}}),
        )?;
        Ok(())
    })
    .unwrap();
    let (question, text) = exhaust(&s, &task);
    assert!(text.starts_with("Задача «Implement the change»"), "{text}");
    assert!(text.contains("проверка результата не прошла"), "{text}");
    assert!(text.contains("scripts/cortex-xtask.sh"), "{text}");
    assert!(text.contains("1 раз"), "{text}");
    let asked = latest(&s, &task, "USER_QUESTION_REQUESTED");
    assert_eq!(
        asked["option_ids"],
        json!([
            "retry_now",
            "other_approach",
            "accept_unverified",
            "retry_later"
        ])
    );
    assert_eq!(asked["options"], json!([
        "Повторить сейчас: начать новый круг всех стратегий",
                "Другой подход: опишите в ответе, что сделать иначе",
                "Принять без проверки: отметить как непроверенное и завершить задачу",
                "Повторить позже: автоматически через 30 минут",
    ]));
    assert!(asked["runtime_stop"].is_null());
    let broken = latest(&s, &task, "LOOP_BROKEN");
    assert_eq!(broken["action"], "ask_owner");
    assert_eq!(broken["count"], 5);
    assert_eq!(
        broken["rungs_used"],
        json!(["repair", "escalate_route", "consult", "reassign"])
    );
    assert_eq!(count(&s, &task, "AUTOMATIC_PAUSED"), 0);
    // Retry now: a new round; after it is used up the question names attempt 2.
    let owed = answer(
        &s,
        &task,
        &question,
        "Повторить сейчас: начать новый круг всех стратегий",
    );
    assert_eq!(owed.evidence["owner_choice"], "retry_now");
    assert!(owed.waiting.is_empty());
    s.write(|db| {
        automatic::resume_tx(db, &task, owed.event)?.unwrap();
        plan::accept_tx(db, &task, "coordinator", "after-answer", &support::with_goal(&test_plan()))?;
        Ok(())
    })
    .unwrap();
    let mut strategies = Vec::new();
    for n in 10..13 {
        s.write(|db| {
            verification_failed(db, &task, n);
            domain::stop_tx(db, &task, "verification_failed")?;
            strategies.push(resume_and_plan(db, &task, n)["strategy"].clone());
            Ok(())
        })
        .unwrap();
    }
    assert_eq!(strategies, ["escalate_route", "consult", "reassign"]);
    let (second, text) = s
        .write(|db| {
            verification_failed(db, &task, 13);
            assert_eq!(
                domain::stop_tx(db, &task, "verification_failed")?,
                TaskState::WaitingUser
            );
            Ok(db.query_row(
                "SELECT id,text FROM pending_questions WHERE task_id=? AND status='open'",
                [&task],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )?)
        })
        .unwrap();
    assert_ne!(second, question);
    assert!(text.starts_with("Попытка 2."), "{text}");
    assert_eq!(support::count(&s, "pending_questions"), 2);
}

#[test]
fn another_approach_reaches_the_next_plan_as_the_owners_direction() {
    let (_p, s, task) = start();
    let (question, text) = exhaust(&s, &task);
    assert!(text.starts_with("Task \"Implement the change\""), "{text}");
    // An option with the owner's own words, and a free-text answer, both direct the plan.
    let owed = answer(
        &s,
        &task,
        &question,
        "Another approach: describe in your answer what to do differently\n\nSplit the slow test suite first",
    );
    assert_eq!(owed.evidence["owner_choice"], "other_approach");
    assert_eq!(
        owed.evidence["owner_direction"],
        "Split the slow test suite first"
    );
    let evidence = s
        .write(|db| automatic::resume_tx(db, &task, owed.event))
        .unwrap()
        .unwrap();
    assert!(evidence["instruction"]
        .as_str()
        .unwrap()
        .contains("Split the slow test suite first"));
    assert_eq!(
        s.write(|db| Ok(domain::task_tx(db, &task)?.0)).unwrap(),
        TaskState::Planning
    );
}

#[test]
fn accept_unverified_records_a_disclosed_limit_never_a_pass() {
    let (_p, s, task) = start();
    let (question, _) = exhaust(&s, &task);
    let owed = answer(
        &s,
        &task,
        &question,
        "Accept unverified: record this as not verified and finish the task",
    );
    assert_eq!(owed.evidence["owner_choice"], "accept_unverified");
    s.write(|db| {
        automatic::resume_tx(db, &task, owed.event)?.unwrap();
        // An Automatic repair plan may leave the accepted command out now.
        assert_eq!(
            automatic::owner_accepted_commands(db, &task)?,
            vec![json!(["scripts/cortex-xtask.sh", "test-fast"])]
        );
        // The limit is a gap of the final result (Not verified), never a pass.
        let gaps = lifecycle::reconciliation::gaps(db, &task)?;
        assert!(
            gaps.values()
                .any(|g| g.starts_with("Accepted without verification")),
            "{gaps:?}"
        );
        Ok(())
    })
    .unwrap();
    let broken = latest(&s, &task, "LOOP_BROKEN");
    assert_eq!(broken["action"], "accept_unverified");
    assert_eq!(broken["limits"][0]["outcome"], "unverified");
    assert_eq!(broken["limits"][0]["source"], "runtime");
    assert_eq!(count(&s, &task, "VERIFICATION_PASSED"), 0);
}

#[test]
fn retry_later_waits_without_model_calls_then_continues() {
    let (_p, s, task) = start();
    let (question, _) = exhaust(&s, &task);
    let owed = answer(&s, &task, &question, "4");
    assert_eq!(owed.evidence["owner_choice"], "retry_later");
    assert_eq!(owed.waiting.len(), 1);
    s.write(|db| {
        assert!(automatic::resume_tx(db, &task, owed.event)?.is_none());
        // Thirty minutes later (the answer's time moved back).
        db.execute(
            "UPDATE pending_questions SET answered_at='2020-01-01T00:00:00+00:00' WHERE id=?",
            [&question],
        )?;
        let owed = automatic::owed(db, &task)?.unwrap();
        assert!(owed.waiting.is_empty());
        automatic::resume_tx(db, &task, owed.event)?.unwrap();
        assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Planning);
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_different_failure_and_the_owners_steering_start_the_ladder_again() {
    let (_p, s, task) = start();
    s.write(|db| {
        verification_failed(db, &task, 1);
        domain::stop_tx(db, &task, "verification_failed")?;
        assert_eq!(resume_and_plan(db, &task, 1)["strategy"], "repair");
        // Another failure has its own fingerprint and its own first rung.
        domain::stop_tx(db, &task, "coordinator_no_progress_limit")?;
        assert_eq!(resume_and_plan(db, &task, 2)["strategy"], "repair");
        verification_failed(db, &task, 3);
        domain::stop_tx(db, &task, "verification_failed")?;
        assert_eq!(resume_and_plan(db, &task, 3)["strategy"], "escalate_route");
        lifecycle::event_tx(
            db,
            &task,
            "USER_STEERING",
            json!({"text":"Use the other API"}),
        )?;
        verification_failed(db, &task, 4);
        domain::stop_tx(db, &task, "verification_failed")?;
        assert_eq!(resume_and_plan(db, &task, 4)["strategy"], "repair");
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_rung_whose_routes_would_not_change_is_skipped() {
    let (_p, s, task) = start();
    s.write(|db| {
        verification_failed(db, &task, 1);
        domain::stop_tx(db, &task, "verification_failed")?;
        assert_eq!(resume_and_plan(db, &task, 1)["strategy"], "repair");
        // Every worker of the failed revision already ran escalated (real
        // `NATIVE_ROUTE_SELECTED` decision shape): escalating again changes nothing.
        let revision = domain::task_tx(db, &task)?.1;
        lifecycle::event_tx(
            db,
            &task,
            "NATIVE_ROUTE_SELECTED",
            json!({"assignment_ref":"a1","node":"work","revision":revision,"engine":"claude-agent-acp",
                "decision":{"basis":"rank","candidates":[{"route":"claude-agent-acp/fable"},{"route":"codex-acp/gpt-6.2-astra"}],
                    "chosen":"claude-agent-acp/fable","escalated":true,"role":"backend-dev"}}),
        )?;
        verification_failed(db, &task, 2);
        domain::stop_tx(db, &task, "verification_failed")?;
        assert_eq!(resume_and_plan(db, &task, 2)["strategy"], "consult");
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&s, &task, "AUTOMATIC_CONSULTATION_REQUESTED"), 1);
}

#[test]
fn a_review_dead_end_that_repeats_across_resumes_takes_reassign() {
    let (_p, s, task) = start();
    let dead_end = |item: &str| {
        s.write(|db| {
            let revision = domain::task_tx(db, &task)?.1;
            lifecycle::event_tx(
                db,
                &task,
                "REVIEW_DEAD_END_ROUTED",
                json!({"review":"_review_1","revision":revision,"report_ref":"r","verdict":"INCOMPLETE","blocked":[
                    {"item":item,"blocker":"tool_unavailable","reason":"cannot read it"},
                    {"item":"src/api.rs","blocker":"other_role_work","reason":"the backend role owns it"}]}),
            )?;
            domain::stop_tx(db, &task, "review_incomplete")?;
            let owed = automatic::owed(db, &task)?.unwrap();
            let strategy = owed.evidence["strategy"].clone();
            resume_and_plan(db, &task, revision);
            Ok(strategy)
        })
        .unwrap()
    };
    assert_eq!(dead_end("assets/font.css (embedded base64)"), "repair");
    // AUTOMATIC_RESUMED in between no longer hides the repeat (defect: the dead end
    // count reset at every resume and `reassign` never fired).
    assert_eq!(dead_end("assets/font.css: lines too long"), "reassign");
    assert_eq!(count(&s, &task, "AUTOMATIC_PAUSED"), 0);
}

#[test]
fn owner_pauses_and_restarts_take_no_rung_and_resume_on_the_current_rung() {
    let (_p, s, task) = start();
    s.write(|db| {
        verification_failed(db, &task, 1);
        domain::stop_tx(db, &task, "verification_failed")?;
        resume_and_plan(db, &task, 1);
        verification_failed(db, &task, 2);
        domain::stop_tx(db, &task, "verification_failed")?;
        assert_eq!(resume_and_plan(db, &task, 2)["strategy"], "escalate_route");
        // Youtube 35448-35456: the owner paused during verification; the command's
        // outcome became unknown and the resumed task was stopped for it.
        lifecycle::event_tx(
            db,
            &task,
            "OWNER_TASK_PAUSED",
            json!({"blocker":null,"previous":"VERIFYING","questionIds":[],"resumeState":"VERIFYING","waitIds":[]}),
        )?;
        lifecycle::event_tx(
            db,
            &task,
            "NATIVE_RECOVERY_REQUIRED",
            json!({"previous":"PAUSED","reason":"native_verification_outcome_unknown","replayed":false}),
        )?;
        domain::stop_tx(db, &task, "native_verification_outcome_unknown")?;
        let owed = automatic::owed(db, &task)?.unwrap();
        assert_eq!(owed.evidence["class"], "infrastructure");
        assert_eq!(owed.evidence["infrastructure"], "owner_paused");
        assert_eq!(owed.evidence["strategy"], "escalate_route");
        assert_eq!(owed.evidence["count"], 2);
        assert_eq!(owed.evidence["switch_coordinator"], false);
        resume_and_plan(db, &task, 3);
        // A service restart (cortex 288531-288538): same rung, no new one.
        lifecycle::event_tx(
            db,
            &task,
            "NATIVE_RECOVERY_REQUIRED",
            json!({"code":"storage_unavailable","reason":"native_recovery_failed","replayed":false}),
        )?;
        domain::stop_tx(db, &task, "native_session_outcome_unknown")?;
        let owed = automatic::owed(db, &task)?.unwrap();
        assert_eq!(owed.evidence["infrastructure"], "runtime_restart");
        assert_eq!(owed.evidence["strategy"], "escalate_route");
        resume_and_plan(db, &task, 4);
        // The owner stopped a worker: no rung either.
        domain::stop_tx(db, &task, "worker_stopped_by_owner")?;
        assert_eq!(
            automatic::owed(db, &task)?.unwrap().evidence["infrastructure"],
            "owner_paused"
        );
        resume_and_plan(db, &task, 5);
        // The ladder then continues where it was.
        verification_failed(db, &task, 6);
        domain::stop_tx(db, &task, "verification_failed")?;
        assert_eq!(resume_and_plan(db, &task, 6)["strategy"], "consult");
        Ok(())
    })
    .unwrap();
    let rungs: i64 = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT count(*) FROM runtime_events WHERE task_id=? AND type='AUTOMATIC_CONTINUATION'
                   AND json_extract(payload_json,'$.class')='technical'",
                [&task],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(rungs, 3);
    let projection = s
        .read(|db| automatic::projection(db, &task))
        .unwrap()
        .unwrap();
    assert_eq!(projection["continuationCount"], 3);
}

#[test]
fn the_same_interruption_repeating_without_progress_is_waited_out_by_the_runtime() {
    let (_p, s, task) = start();
    s.write(|db| {
        for n in 1..=2 {
            domain::stop_tx(db, &task, "native_service_interrupted")?;
            resume_and_plan(db, &task, n);
        }
        domain::stop_tx(db, &task, "native_service_interrupted")?;
        let owed = automatic::owed(db, &task)?.unwrap();
        assert_eq!(owed.evidence["pause_kind"], "infrastructure");
        assert_eq!(owed.waiting.len(), 1);
        assert!(automatic::resume_tx(db, &task, owed.event)?.is_none());
        // Once the wait elapsed the runtime resumes it itself, on the same rung.
        db.execute(
            "UPDATE runtime_events SET payload_json=json_set(payload_json,'$.not_before',0) WHERE id=?",
            [owed.event],
        )?;
        let owed = automatic::owed(db, &task)?.unwrap();
        assert!(owed.waiting.is_empty());
        assert_eq!(owed.evidence["strategy"], "repair");
        assert!(automatic::resume_tx(db, &task, owed.event)?.is_some());
        assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Planning);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        latest(&s, &task, "LOOP_BROKEN")["action"],
        "infrastructure_wait"
    );
    assert_eq!(count(&s, &task, "AUTOMATIC_CONSULTATION_REQUESTED"), 0);
}

fn finish_interruption_wait(db: &Transaction<'_>, task: &str) -> cortex_core::CoreResult<Value> {
    let owed = automatic::owed(db, task)?.unwrap();
    if !owed.waiting.is_empty() {
        assert!(automatic::resume_tx(db, task, owed.event)?.is_none());
        db.execute(
            "UPDATE runtime_events SET payload_json=json_set(payload_json,'$.not_before',0) WHERE id=?",
            [owed.event],
        )?;
    }
    Ok(automatic::resume_tx(db, task, owed.event)?.unwrap())
}

#[test]
fn real_progress_resets_the_coordinator_interruption_series_but_plan_acceptance_does_not() {
    let (_p, s, task) = start();
    let reason = "native_turn_outcome_unknown";
    s.write(|db| {
        for n in 1..=7 {
            assert_eq!(domain::stop_tx(db, &task, reason)?, TaskState::Recovering);
            let evidence = finish_interruption_wait(db, &task)?;
            assert_eq!(evidence["loop"]["count"], n);
            if n >= 3 {
                assert_eq!(evidence["pause_kind"], "infrastructure");
            }
            plan::accept_tx(db, &task, "coordinator", &format!("plan-{n}"), &support::with_goal(&test_plan()))?;
        }
        lifecycle::event_tx(
            db,
            &task,
            "TASK_PROGRESS",
            json!({"kind":"node_completed","node":"work"}),
        )?;
        for n in 1..=3 {
            assert_eq!(domain::stop_tx(db, &task, reason)?, TaskState::Recovering);
            let evidence = finish_interruption_wait(db, &task)?;
            assert_eq!(evidence["loop"]["count"], n);
            assert_eq!(evidence["loop"]["breaks"], 0);
            if n == 3 {
                assert_eq!(evidence["backoff_seconds"], 300);
            } else {
                assert!(evidence["pause_kind"].is_null());
            }
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&s, &task, "USER_QUESTION_REQUESTED"), 0);
}

#[test]
fn the_owner_sees_the_coordinator_reason_and_can_delay_or_give_new_direction() {
    for (language, answer_text, choice) in [
        ("ru", "3", "retry_later"),
        (
            "en",
            "Use the existing evidence and investigate the connection.",
            "other_approach",
        ),
    ] {
        let (_p, s, task) = start();
        let reason = "native_turn_outcome_unknown";
        let question = s
            .write(|db| {
                lifecycle::event_tx(
                    db,
                    &task,
                    "NATIVE_TASK_SETTINGS",
                    json!({"settings":{"language":language}}),
                )?;
                for _ in 0..7 {
                    assert_eq!(domain::stop_tx(db, &task, reason)?, TaskState::Recovering);
                    finish_interruption_wait(db, &task)?;
                }
                assert_eq!(domain::stop_tx(db, &task, reason)?, TaskState::WaitingUser);
                Ok(db.query_row(
                    "SELECT id FROM pending_questions WHERE task_id=? AND status='open'",
                    [&task],
                    |r| r.get::<_, String>(0),
                )?)
            })
            .unwrap();
        let card = workspace::control::decisions(
            &s,
            "p",
            "owner",
            &json!({"operation":"decisions.get","target":format!("wd_{question}")}),
        )
        .unwrap();
        assert_eq!(card["status"], "pending");
        assert_eq!(card["allowedOperations"], json!(["decisions.answer"]));
        let prompt = card["prompt"].as_str().unwrap();
        let labels: Vec<_> = card["options"].as_array().unwrap().iter().map(|o| o["label"].as_str().unwrap()).collect();
        if language == "ru" {
            assert!(prompt.contains("ход координатора был прерван"));
            assert_eq!(labels, [
                "Повторить сейчас: продолжить с текущим подходом",
                "Другой подход: опишите в ответе, что сделать иначе",
                "Повторить позже: автоматически через 30 минут",
            ]);
        } else {
            assert!(prompt.contains("the coordinator's turn was interrupted"));
            assert_eq!(labels, [
                "Retry now: continue on the current approach",
                "Another approach: describe in your answer what to do differently",
                "Retry later: try again automatically in 30 minutes",
            ]);
        }

        let owed = answer(&s, &task, &question, answer_text);
        assert_eq!(owed.evidence["owner_choice"], choice);
        if choice == "retry_later" {
            assert_eq!(owed.waiting.len(), 1);
            assert!(s
                .write(|db| automatic::resume_tx(db, &task, owed.event))
                .unwrap()
                .is_none());
        } else {
            assert!(owed.waiting.is_empty());
            assert_eq!(owed.evidence["owner_direction"], answer_text);
        }
        assert!(s
            .read(|db| automatic::owner_accepted_commands(db, &task))
            .unwrap()
            .is_empty());
    }
}

#[test]
fn repeated_worker_interruptions_keep_cooling_down_past_the_coordinator_boundary() {
    let (_p, s, task) = start();
    s.write(|db| {
        for n in 1..=9 {
            assert_eq!(
                domain::stop_tx(db, &task, "lost_worker_action_outcome_unknown")?,
                TaskState::Recovering
            );
            let evidence = finish_interruption_wait(db, &task)?;
            assert_eq!(evidence["loop"]["count"], n);
            if n >= 8 {
                assert_eq!(evidence["pause_kind"], "infrastructure");
                assert_eq!(evidence["backoff_seconds"], 3600);
            }
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&s, &task, "USER_QUESTION_REQUESTED"), 0);
}

#[test]
fn a_sandbox_that_could_not_be_prepared_is_not_the_workers_failure() {
    let (_p, s, task) = start();
    s.write(|db| {
        plan::accept_tx(db, &task, "coordinator", "plan", &support::with_goal(&test_plan()))?;
        let revision = domain::task_tx(db, &task)?.1;
        lifecycle::event_tx(
            db,
            &task,
            "ASSIGNMENT_CREATED",
            json!({"assignment_ref":"679c77b7","node":"work","revision":revision}),
        )?;
        // Case_Unsealed 25199/25217: the receipt records the sandbox cause; the worker
        // names its tools unavailable.
        lifecycle::event_tx(
            db,
            &task,
            "NATIVE_COMMAND_FINISHED",
            json!({"receipt":{"argv":["python3","-B","approved-recut/source/audit.py"],
                "observation":{"cause":"sandbox_unavailable:git_context_unavailable","complete":false,
                    "exit_code":null,"failure":"command_not_started","launched":false,"outcome":"not_started"}}}),
        )?;
        lifecycle::event_tx(
            db,
            &task,
            "WORKER_FAILED",
            json!({"assignment_ref":"679c77b7","attempt_id":"xa_1","result":{"assignment_outcome":"failed",
                "blocker":{"kind":"tool_unavailable","detail":"After a complete candidate render, the command sandbox repeatedly failed to start both the corrected sample render and full decode/audit."}}}),
        )?;
        assert!(!automatic::node_escalated(db, &task, "work", revision)?);
        // Youtube 21005: the refusal code in the blocker is enough.
        lifecycle::event_tx(
            db,
            &task,
            "ASSIGNMENT_CREATED",
            json!({"assignment_ref":"12a0b253","node":"snap","revision":revision}),
        )?;
        lifecycle::event_tx(
            db,
            &task,
            "WORKER_FAILED",
            json!({"assignment_ref":"12a0b253","result":{"assignment_outcome":"failed",
                "blocker":{"kind":"tool_unavailable","detail":"The command broker refused mkdir with native_command_sandbox_unavailable; no change was made."}}}),
        )?;
        assert!(!automatic::node_escalated(db, &task, "snap", revision)?);
        // A retry of an infrastructure class from the contract does not escalate.
        for class in loop_guard::INFRASTRUCTURE_CLASSES {
            lifecycle::event_tx(
                db,
                &task,
                "ASSIGNMENT_RETRIED",
                json!({"node":"ui","failure_class":class,"revision":revision}),
            )?;
        }
        assert!(!automatic::node_escalated(db, &task, "ui", revision)?);
        // The stop takes no rung.
        domain::stop_tx(db, &task, "worker_failed")?;
        let owed = automatic::owed(db, &task)?.unwrap();
        assert_eq!(owed.evidence["infrastructure"], "sandbox_unavailable");
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.read(|db| automatic::projection(db, &task))
            .unwrap()
            .unwrap()["continuationCount"],
        0
    );
}

/// A review of `revision` that ended INCOMPLETE on dead ends, as `review::advance_tx`
/// leaves it before its stop.
fn incomplete_review(db: &Transaction<'_>, task: &str, revision: i64, blocked: Value) {
    db.execute(
        "UPDATE runtime_tasks SET state='REVIEWING',revision=?,plan_json=? WHERE task_id=?",
        (revision, test_plan().to_string(), task),
    )
    .unwrap();
    let not_covered = blocked.clone();
    let result = json!({"verdict":"INCOMPLETE","report_ref":format!("r{revision}"),"checks":[],
        "project_digest":"a".repeat(64),"not_covered":not_covered});
    db.execute(
        "INSERT INTO runtime_nodes VALUES (?,?,'_review_1','worker-code-reviewer','Review','[]','[]','review','completed',?,?)",
        (task, revision, format!("rv{revision}"), result.to_string()),
    )
    .unwrap();
    lifecycle::event_tx(
        db,
        task,
        "REVIEW_DEAD_END_ROUTED",
        json!({"review":"_review_1","revision":revision,"report_ref":format!("r{revision}"),
            "verdict":"INCOMPLETE","blocked":blocked}),
    )
    .unwrap();
}

#[test]
fn a_review_dead_end_on_every_rung_asks_the_owner_and_only_their_close_as_is_reaches_verification() {
    let (_p, s, task) = start();
    let blocked = json!([
        {"blocker":"other_role_work","item":"channels/data/generated-index.json (66 KB generated)","reason":"generated by the build role"},
        {"blocker":"tool_unavailable","item":"assets/cover.png","reason":"image"}]);
    let question: String = s
        .write(|db| {
            for n in 1..=4i64 {
                incomplete_review(db, &task, 10 + n, blocked.clone());
                domain::stop_tx(db, &task, "review_incomplete")?;
                resume_and_plan(db, &task, n);
            }
            incomplete_review(db, &task, 20, blocked.clone());
            // Every strategy met the same dead end: the owner decides, the review is
            // not disclosed by the runtime and verification does not run.
            assert_eq!(
                domain::stop_tx(db, &task, "review_incomplete")?,
                TaskState::WaitingUser
            );
            Ok(db.query_row(
                "SELECT id FROM pending_questions WHERE task_id=? AND status='open'",
                [&task],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(latest(&s, &task, "LOOP_BROKEN")["action"], "ask_owner");
    assert_eq!(count(&s, &task, "REVIEW_LIMIT_DISCLOSED"), 0);
    assert_eq!(count(&s, &task, "VERIFICATION_PASSED") + count(&s, &task, "VERIFICATION_FAILED"), 0);
    // The owner closes the unreachable parts as is.
    let owed = answer(
        &s,
        &task,
        &question,
        "Accept unverified: record this as not verified and finish the task",
    );
    assert_eq!(owed.evidence["owner_choice"], "accept_unverified");
    s.write(|db| {
        automatic::resume_tx(db, &task, owed.event)?.unwrap();
        plan::accept_tx(db, &task, "coordinator", "after-close", &support::with_goal(&test_plan()))?;
        incomplete_review(db, &task, 30, blocked.clone());
        lifecycle::review::advance_tx(db, &task)?;
        assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Verifying);
        Ok(())
    })
    .unwrap();
    let disclosed = latest(&s, &task, "REVIEW_LIMIT_DISCLOSED");
    assert_eq!(disclosed["owner_authorized"], true);
    assert_eq!(disclosed["limits"].as_array().unwrap().len(), 2);
    let result: Value = s
        .read(|db| {
            Ok(serde_json::from_str(&db.query_row(
                "SELECT result_json FROM runtime_nodes WHERE task_id=? AND revision=30 AND node_id='_review_1'",
                [&task],
                |r| r.get::<_, String>(0),
            )?)?)
        })
        .unwrap();
    assert_eq!(result["review_limit_disclosed"], true);
    assert_eq!(result["review_limit_owner_authorized"], true);
    assert_eq!(result["checks"].as_array().unwrap().len(), 2);
}

/// Insert the executing node `work` of `revision` beside its review.
fn work_node(db: &Transaction<'_>, task: &str, revision: i64) {
    db.execute(
        "INSERT INTO runtime_nodes VALUES (?,?,'work','worker-backend-dev','Correct the implementation','[]','[\"src\"]','execution','completed',?,?)",
        (task, revision, format!("w{revision}"),
            json!({"report_ref":format!("w{revision}"),"checks":[],"objective_status":"complete"}).to_string()),
    )
    .unwrap();
}

#[test]
fn an_incomplete_review_with_a_cross_cutting_finding_is_fixed_never_verified() {
    let (_p, s, task) = start();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='REVIEWING',revision=5,plan_json=?,review_cycle=1 WHERE task_id=?",
            (support::with_goal(&test_plan()).to_string(), &task),
        )?;
        work_node(db, &task, 5);
        let result = json!({"verdict":"INCOMPLETE","report_ref":"r5","checks":[],"project_digest":"a".repeat(64),
            "not_covered":[{"item":"assets/cover.png","blocker":"tool_unavailable","reason":"image"}],
            "findings":[{"node":"work","files":["src/lib.rs","docs/api.md"],"fix":"Keep the interface and its documentation in step","cross_cutting":true}]});
        db.execute(
            "INSERT INTO runtime_nodes VALUES (?,5,'_review_1','worker-code-reviewer','Review','[\"work\"]','[]','review','completed','rv5',?)",
            (&task, result.to_string()),
        )?;
        lifecycle::review::advance_tx(db, &task)?;
        assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Executing);
        Ok(())
    })
    .unwrap();
    let requested = latest(&s, &task, "REVIEW_CHANGES_REQUESTED");
    assert_eq!(requested["fallback"], "cross_cutting");
    assert_eq!(requested["fixes"], json!(["_fix_1_work"]));
    assert_eq!(count(&s, &task, "REVIEW_DEAD_END_ROUTED"), 0);
    assert_eq!(count(&s, &task, "REVIEW_LIMIT_DISCLOSED"), 0);
}

#[test]
fn unreachable_review_parts_get_one_narrow_follow_up_then_the_owner_decides() {
    let (_p, s, task) = start();
    let unreachable = json!([{"item":"assets/cover.png","blocker":"tool_unavailable","reason":"image"}]);
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='REVIEWING',revision=5,plan_json=?,review_cycle=1 WHERE task_id=?",
            (support::with_goal(&test_plan()).to_string(), &task),
        )?;
        work_node(db, &task, 5);
        let result = json!({"verdict":"INCOMPLETE","report_ref":"r5","checks":[],"project_digest":"a".repeat(64),
            "not_covered":unreachable});
        db.execute(
            "INSERT INTO runtime_nodes VALUES (?,5,'_review_1','worker-code-reviewer','Review','[\"work\"]','[]','review','completed','rv5',?)",
            (&task, result.to_string()),
        )?;
        lifecycle::review::advance_tx(db, &task)?;
        // A follow-up review narrowed to the unreachable part, still reviewing.
        assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Reviewing);
        let (objective, status): (String, String) = db.query_row(
            "SELECT objective,status FROM runtime_nodes WHERE task_id=? AND revision=5 AND node_id='_review_2'",
            [&task],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        assert_eq!(status, "pending");
        assert!(objective.contains("assets/cover.png") && objective.contains("different approach"), "{objective}");
        // The follow-up meets the same dead end: the owner decides, not the runtime.
        db.execute(
            "UPDATE runtime_nodes SET status='completed',assignment_ref='rv5b',result_json=? WHERE task_id=? AND node_id='_review_2'",
            (json!({"verdict":"INCOMPLETE","report_ref":"r5b","checks":[],"project_digest":"a".repeat(64),
                "not_covered":unreachable}).to_string(), &task),
        )?;
        lifecycle::review::advance_tx(db, &task)?;
        assert_ne!(domain::task_tx(db, &task)?.0, TaskState::Verifying);
        Ok(())
    })
    .unwrap();
    let rerun = latest(&s, &task, "REVIEW_RERUN");
    assert_eq!((rerun["review"].as_str(), rerun["rerun"].as_str()), (Some("_review_1"), Some("_review_2")));
    let routed = latest(&s, &task, "REVIEW_DEAD_END_ROUTED");
    assert_eq!(routed["blocked"][0]["blocker"], "owner_decision");
    assert_eq!(latest(&s, &task, "AUTOMATIC_CONTINUATION")["strategy"], "ask_owner");
    assert_eq!(count(&s, &task, "REVIEW_LIMIT_DISCLOSED"), 0);
}

#[test]
fn replans_count_since_the_owners_input_not_since_the_last_resume() {
    let (_p, s, task) = start();
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
            // A continuation resumed in between (every cycle passes through one).
            lifecycle::event_tx(db, &task, "AUTOMATIC_RESUMED", json!({"stop_event":0}))?;
        }
        let result = domain::propose_replan_tx(
            db,
            &task,
            "coordinator",
            "fourth",
            "replan_request",
            &json!({"reason":"Further repair"}),
        )?;
        assert_eq!(result["decision"], "senior_consultation");
        Ok(())
    })
    .unwrap();
}

/// A review that failed only because another actor kept changing the checkout waits
/// on the runtime's timer and is then run again directly: no rung, no coordinator.
#[test]
fn a_review_ended_by_checkout_drift_is_rerun_without_a_coordinator_turn() {
    let (_p, s, task) = start();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='REVIEWING',revision=3,plan_json=? WHERE task_id=?",
            (test_plan().to_string(), &task),
        )?;
        db.execute(
            "INSERT INTO runtime_nodes VALUES (?,3,'_review_1','worker-code-reviewer','Review','[]','[]','review','failed','rv3',NULL)",
            [&task],
        )?;
        lifecycle::event_tx(db, &task, "REVIEW_STARTED", json!({"assignment_ref":"rv3","revision":3}))?;
        // cortex t_bd86114cf610 254951-255263: refusals for a changed revision while
        // another process wrote outside the task's files.
        for _ in 0..2 {
            lifecycle::event_tx(
                db,
                &task,
                "WORKER_RESULT_REFUSED",
                json!({"assignment_ref":"rv3","attempt":"xa_r","channel_id":"ch_r","code":"review_project_revision_changed"}),
            )?;
        }
        domain::stop_tx(db, &task, "review_blocked")?;
        let owed = automatic::owed(db, &task)?.unwrap();
        assert_eq!(owed.evidence["pause_kind"], "obstacle");
        assert_eq!(owed.evidence["resume_state"], "EXECUTING");
        assert_eq!(owed.evidence["resume_review"], "_review_1");
        assert!(!owed.waiting.is_empty());
        let id = owed.waiting[0].as_str().unwrap().to_owned();
        assert!(lifecycle::obstacles::record_recheck_tx(db, &task, &id, false, i64::MAX / 2, "")?);
        let owed = automatic::owed(db, &task)?.unwrap();
        assert!(owed.waiting.is_empty());
        let evidence = automatic::resume_tx(db, &task, owed.event)?.unwrap();
        assert_eq!(evidence["resume"], "execution");
        assert_eq!(domain::task_tx(db, &task)?.0, TaskState::Reviewing);
        let status: String = db.query_row(
            "SELECT status FROM runtime_nodes WHERE task_id=? AND revision=3 AND node_id='_review_1'",
            [&task],
            |r| r.get(0),
        )?;
        assert_eq!(status, "pending");
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&s, &task, "AUTOMATIC_CONTINUATION"), 0);
    assert_eq!(count(&s, &task, "LOOP_OBSERVED"), 0);
}
