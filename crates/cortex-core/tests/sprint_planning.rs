//! A read-only sprint planning task on the production SQLite writer: bound at
//! Start, plans only read-only specialists and records its plan through the runtime.
mod support;
use cortex_core::{
    lifecycle::{plan, sprint_planning},
    workspace::{self, NativeExecutionPolicy},
    OpenMode, Store,
};
use serde_json::{json, Value};
use support::Project;

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
            "payload":{"description":"Sprint planning (read-only). Plan the sprint.","checkout":{"kind":"main"}}
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

fn start(policy: Option<NativeExecutionPolicy>) -> (Project, Store, String, Value) {
    let p = Project::new();
    let s = p.open();
    let start = draft(&s, &p);
    let receipt =
        workspace::start_native_input_with_policy(&s, "p", "owner", &start, None, None, policy)
            .unwrap();
    (p, s, receipt["task"].as_str().unwrap().into(), start)
}

fn plan_with(role: &str) -> Value {
    json!({"tasks":[{"id":"study","role":role,"objective":"Study the project",
        "dependencies":[],"scope":["src"]}],"verification":[["cargo","test"]]})
}

#[test]
fn sprint_planning_is_bound_at_start_and_bound_to_its_replay() {
    let (p, s, task, cmd) = start(Some(NativeExecutionPolicy::SprintPlanning));
    assert!(s.read(|db| sprint_planning::enabled(db, &task)).unwrap());
    assert!(workspace::replay_native_start_with_policy(
        &s,
        "p",
        "owner",
        &cmd,
        Some(NativeExecutionPolicy::SprintPlanning)
    )
    .unwrap()
    .is_some());
    // The same Start without the policy is a different request, never a silent unbinding.
    assert_eq!(
        workspace::replay_native_start(&s, "p", "owner", &cmd)
            .unwrap_err()
            .code(),
        "payload_conflict"
    );
    drop(s);
    // The binding is an event: an immutable open reads it without any migration.
    let s = Store::open(p.key(), OpenMode::InspectImmutable).unwrap();
    assert!(s.read(|db| sprint_planning::enabled(db, &task)).unwrap());
}

#[test]
fn a_planning_task_plans_only_read_only_specialists() {
    let (_p, s, task, _) = start(Some(NativeExecutionPolicy::SprintPlanning));
    let refused = s
        .write(|db| {
            plan::accept_tx(
                db,
                &task,
                "coordinator",
                "writer",
                &plan_with("backend-dev"),
            )
        })
        .unwrap_err();
    assert_eq!(refused.code(), "read_only_task_writer_refused");
    s.write(|db| plan::accept_tx(db, &task, "coordinator", "study", &plan_with("explorer")))
        .unwrap();
    // An ordinary task keeps its writing roles.
    let (_p, s, task, _) = start(None);
    assert!(!s.read(|db| sprint_planning::enabled(db, &task)).unwrap());
    s.write(|db| {
        plan::accept_tx(
            db,
            &task,
            "coordinator",
            "writer",
            &plan_with("backend-dev"),
        )
    })
    .unwrap();
}

#[test]
fn the_plan_is_recorded_only_for_a_planning_task_and_replays_exactly() {
    let tasks = json!([{"key":"t1","title":"First","description":"Do it","dependsOn":[]}]);
    let args = json!({"tasks":tasks,"request_key":"plan-1"});
    let (_p, s, task, _) = start(None);
    assert_eq!(
        s.write(|db| sprint_planning::submit_tx(db, &task, "c", "plan-1", &args, &tasks))
            .unwrap_err()
            .code(),
        "sprint_plan_not_expected"
    );
    let (_p, s, task, _) = start(Some(NativeExecutionPolicy::SprintPlanning));
    let receipt = s
        .write(|db| sprint_planning::submit_tx(db, &task, "c", "plan-1", &args, &tasks))
        .unwrap();
    assert_eq!(
        receipt,
        json!({"recorded":true,"tasks":1,"review":"scheduled"})
    );
    // An exact retry returns the same receipt and records nothing more.
    assert_eq!(
        s.write(|db| sprint_planning::submit_tx(db, &task, "c", "plan-1", &args, &tasks))
            .unwrap(),
        receipt
    );
    let other = json!({"tasks":tasks,"request_key":"plan-1","x":1});
    assert_eq!(
        s.write(|db| sprint_planning::submit_tx(db, &task, "c", "plan-1", &other, &tasks))
            .unwrap_err()
            .code(),
        "payload_conflict"
    );
    let (_, saved) = s
        .read(|db| sprint_planning::latest(db, &task))
        .unwrap()
        .unwrap();
    assert_eq!(saved, tasks);
    // While its review runs, no other plan is taken: the coordinator submits at its own turn.
    let revised = json!([{"key":"t1","title":"First","description":"Do it better","dependsOn":[]}]);
    let args = json!({"tasks":revised,"request_key":"plan-2"});
    assert_eq!(
        s.write(|db| sprint_planning::submit_tx(db, &task, "c", "plan-2", &args, &revised))
            .unwrap_err()
            .code(),
        "sprint_plan_not_expected"
    );
    // Neither while a study worker runs.
    let (_p, s, task, _) = start(Some(NativeExecutionPolicy::SprintPlanning));
    s.write(|db| plan::accept_tx(db, &task, "coordinator", "study", &plan_with("explorer")))
        .unwrap();
    let args = json!({"tasks":revised,"request_key":"plan-3"});
    assert_eq!(
        s.write(|db| sprint_planning::submit_tx(db, &task, "c", "plan-3", &args, &revised))
            .unwrap_err()
            .code(),
        "sprint_plan_not_expected"
    );
}

fn state(s: &Store, task: &str) -> String {
    s.read(|db| {
        Ok(db.query_row(
            "SELECT state FROM runtime_tasks WHERE task_id=?",
            [task],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}
/// A reviewer's completed result for the pending plan review, as the result path records it.
fn reviewed(s: &Store, task: &str, node: &str, result: Value) {
    s.write(|db| {
        db.execute(
            "UPDATE runtime_nodes SET status='completed',result_json=? WHERE task_id=? AND node_id=?",
            (result.to_string(), task, node),
        )?;
        cortex_core::lifecycle::event_tx(
            db,
            task,
            "WORKER_COMPLETED",
            json!({"node":node,"result":{"report_ref":result["report_ref"]}}),
        )?;
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_submitted_plan_is_independently_reviewed_and_verified_by_the_runtime() {
    use cortex_core::lifecycle::review;
    let (_p, s, task, _) = start(Some(NativeExecutionPolicy::SprintPlanning));
    let first = json!([{"key":"t1","title":"First","description":"Do it","dependsOn":[]}]);
    let args = json!({"tasks":first,"request_key":"p1"});
    let receipt = s
        .write(|db| sprint_planning::submit_tx(db, &task, "c", "p1", &args, &first))
        .unwrap();
    assert_eq!(receipt["review"], "scheduled");
    assert_eq!(state(&s, &task), "REVIEWING");
    let objective: String = s
        .read(|db| Ok(db.query_row("SELECT objective FROM runtime_nodes WHERE task_id=? AND node_id='_plan_review_1' AND stage='review' AND role='worker-code-reviewer'", [&task], |r| r.get(0))?))
        .unwrap();
    assert!(
        objective.contains("Independently review the submitted sprint plan")
            && objective.contains("\"Do it\"")
    );
    // Findings return the plan to its coordinator.
    let digest = "a".repeat(64);
    reviewed(
        &s,
        &task,
        "_plan_review_1",
        json!({"verdict":"REQUEST_CHANGES","report_ref":"r_aaaaaaaaaaaa",
        "project_digest":digest,"findings":[{"node":"t1","files":[],"fix":"Name the acceptance criteria"}]}),
    );
    s.write(|db| review::advance_tx(db, &task)).unwrap();
    assert_eq!(state(&s, &task), "RECOVERING");
    let returned: i64 = s.read(|db| Ok(db.query_row("SELECT count(*) FROM runtime_events WHERE task_id=? AND type IN ('SPRINT_PLAN_CHANGES_REQUESTED','RETURNED_TO_COORDINATOR')", [&task], |r| r.get(0))?)).unwrap();
    assert_eq!(returned, 2);
    // The relaunched coordinator revises; an unchanged plan is never reviewed again.
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='PLANNING' WHERE task_id=?",
            [&task],
        )?;
        Ok(())
    })
    .unwrap();
    let same = json!({"tasks":first,"request_key":"p2"});
    assert_eq!(
        s.write(|db| sprint_planning::submit_tx(db, &task, "c", "p2", &same, &first))
            .unwrap_err()
            .code(),
        "sprint_plan_unchanged"
    );
    let revised = json!([{"key":"t1","title":"First","description":"Do it; acceptance: it works","dependsOn":[]}]);
    let args = json!({"tasks":revised,"request_key":"p3"});
    s.write(|db| sprint_planning::submit_tx(db, &task, "c", "p3", &args, &revised))
        .unwrap();
    assert_eq!(state(&s, &task), "REVIEWING");
    // The same project files are expected: only the plan changed, so the round proceeds.
    reviewed(
        &s,
        &task,
        "_plan_review_2",
        json!({"verdict":"APPROVE","report_ref":"r_bbbbbbbbbbbb",
        "project_digest":digest,"findings":[]}),
    );
    s.write(|db| review::advance_tx(db, &task)).unwrap();
    assert_eq!(state(&s, &task), "VERIFYING");
    // The runtime verifies the reviewed plan itself: no model-declared command.
    assert!(s.write(|db| sprint_planning::verify_tx(db, &task)).unwrap());
    assert_eq!(state(&s, &task), "RECONCILING");
    let (kind, payload): (String, String) = s.read(|db| Ok(db.query_row("SELECT type,payload_json FROM runtime_events WHERE task_id=? AND type LIKE 'VERIFICATION_%' ORDER BY id DESC LIMIT 1", [&task], |r| Ok((r.get(0)?, r.get(1)?)))?)).unwrap();
    assert_eq!(kind, "VERIFICATION_PASSED");
    let payload: Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["runtime"], "sprint_plan");
    assert_eq!(payload["revision_digest"], digest.as_str());
    let (latest, _) = s
        .read(|db| sprint_planning::latest(db, &task))
        .unwrap()
        .unwrap();
    assert_eq!(payload["plan_event"], latest);
}

#[test]
fn a_planning_study_names_no_verification_and_other_plans_still_must() {
    let study = json!({"tasks":[{"id":"study","role":"explorer","objective":"Study the project",
        "dependencies":[],"scope":["src"]}],"verification":[]});
    let (_p, s, task, _) = start(Some(NativeExecutionPolicy::SprintPlanning));
    let accepted = s
        .write(|db| plan::accept_tx(db, &task, "coordinator", "study", &study))
        .unwrap();
    assert!(accepted.is_object(), "{accepted}");
    let (_p, s, task, _) = start(None);
    assert_eq!(
        s.write(|db| plan::accept_tx(db, &task, "coordinator", "study", &study))
            .unwrap_err()
            .code(),
        "verification_required"
    );
}

/// Production stores: the first review-cycle stop of a planning task goes to its
/// coordinator (`ASK_USER_REQUIRED` with the blocker, no blocked state), which asks
/// the owner. The owner's "continue" did not renew the budget, so every later round
/// stopped again after one review. An answer after that stop now grants a fresh
/// budget at the next submission; a resubmission with no answer does not.
#[test]
fn a_review_cycle_continuation_renews_the_plan_budget() {
    use cortex_core::lifecycle::{automatic, review};
    for resumed in [true, false] {
        let (_p, s, task, _) = start(Some(NativeExecutionPolicy::SprintPlanning));
        let first = json!([{"key":"t1","title":"First","description":"Do it","dependsOn":[]}]);
        let args = json!({"tasks":first,"request_key":"p1"});
        s.write(|db| sprint_planning::submit_tx(db, &task, "c", "p1", &args, &first))
            .unwrap();
        // The budget is used up: the third review asks for changes again.
        s.write(|db| {
            db.execute(
                "UPDATE runtime_tasks SET review_cycle=3 WHERE task_id=?",
                [&task],
            )?;
            Ok(())
        })
        .unwrap();
        reviewed(
            &s,
            &task,
            "_plan_review_1",
            json!({"verdict":"REQUEST_CHANGES","report_ref":"r_aaaaaaaaaaaa",
            "project_digest":"a".repeat(64),"findings":[{"node":"t1","files":[],"fix":"Name the acceptance criteria"}]}),
        );
        s.write(|db| review::advance_tx(db, &task)).unwrap();
        assert_eq!(state(&s, &task), "RECOVERING");
        // No-stop policy: a continuation, never a question for the owner.
        let owed = s.read(|db| automatic::owed(db, &task)).unwrap().unwrap();
        assert_eq!(owed.evidence["reason"], "review_cycle_limit");
        let asked: i64 = s
            .read(|db| {
                Ok(db.query_row(
                    "SELECT count(*) FROM runtime_events WHERE task_id=? AND type IN ('ASK_USER_REQUIRED','USER_QUESTION_REQUESTED')",
                    [&task],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(asked, 0);
        s.write(|db| {
            if resumed {
                automatic::resume_tx(db, &task, owed.event)?.unwrap();
            } else {
                db.execute(
                    "UPDATE runtime_tasks SET state='PLANNING',blocker=NULL WHERE task_id=?",
                    [&task],
                )?;
            }
            Ok(())
        })
        .unwrap();
        let revised = json!([{"key":"t1","title":"First","description":"Do it; acceptance: it works","dependsOn":[]}]);
        let args = json!({"tasks":revised,"request_key":"p2"});
        let receipt = s
            .write(|db| sprint_planning::submit_tx(db, &task, "c", "p2", &args, &revised))
            .unwrap();
        assert_eq!(receipt["review"], "scheduled");
        let cycle: i64 = s
            .read(|db| {
                Ok(db.query_row(
                    "SELECT review_cycle FROM runtime_tasks WHERE task_id=?",
                    [&task],
                    |r| r.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(cycle, if resumed { 1 } else { 4 }, "resumed: {resumed}");
    }
}
