//! Sprint impact checks: an owner change of a sprint member with unstarted drafts
//! left requires a verdict before the task may finish.
mod support;
use cortex_core::{
    lifecycle::{self, completion::{complete_tx, EvidenceSnapshot}, review, sprint_impact, TaskMachine},
    Store,
};
use serde_json::json;
use support::Project;

fn setup() -> (Project, Store) {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute("INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)", [p.0.to_str()])?;
        for id in ["d1", "d2"] {
            db.execute(
                "INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES (?,'p','Draft','now')",
                [id],
            )?;
        }
        db.execute("UPDATE runtime_tasks SET state='RECONCILING',synthesis='Done' WHERE task_id='t'", [])?;
        Ok(())
    })
    .unwrap();
    (p, s)
}

fn checks(s: &Store) -> i64 {
    s.read(|db| {
        Ok(db.query_row(
            "SELECT count(*) FROM runtime_events WHERE task_id='t' AND type='SPRINT_IMPACT_CHECK_REQUIRED'",
            [],
            |r| r.get(0),
        )?)
    })
    .unwrap()
}

fn complete(s: &Store) -> String {
    s.write(|db| Ok(complete_tx(db, "t", &EvidenceSnapshot::default()).err().map(|e| e.code().to_owned())))
        .unwrap()
        .unwrap_or_default()
}

#[test]
fn only_owner_triggers_with_unstarted_members_require_a_check() {
    let (_p, s) = setup();
    // Not bound to a sprint: nothing.
    s.write(|db| lifecycle::event_tx(db, "t", "USER_ANSWER_RECEIVED", json!({})).map(|_| ())).unwrap();
    assert_eq!(checks(&s), 0);
    s.write(|db| sprint_impact::bind_tx(db, "t", "sp", &["d1".into(), "d2".into()])).unwrap();
    // A worker scope request and a non-owner reopen are not triggers.
    s.write(|db| {
        lifecycle::event_tx(db, "t", "SCOPE_REQUEST_RECORDED", json!({}))?;
        lifecycle::event_tx(db, "t", "TASK_REOPENED", json!({"origin":"verification"}))?;
        Ok(())
    })
    .unwrap();
    assert_eq!(checks(&s), 0);
    let trigger = s
        .write(|db| lifecycle::event_tx(db, "t", "STEERING_DELIVERED", json!({"request":"m1"})))
        .unwrap();
    assert_eq!(checks(&s), 1);
    assert_eq!(s.read(|db| sprint_impact::pending(db, "t")).unwrap(), Some(trigger));
    // Every recorded member started: no check.
    s.write(|db| {
        db.execute("UPDATE workspace_tasks SET execution_id='x'", [])?;
        lifecycle::event_tx(db, "t", "TASK_REOPENED", json!({"origin":"owner_rework"}))?;
        Ok(())
    })
    .unwrap();
    assert_eq!(checks(&s), 1);
}

#[test]
fn completion_waits_for_a_verdict_and_unchanged_clears_it() {
    let (_p, s) = setup();
    s.write(|db| sprint_impact::bind_tx(db, "t", "sp", &["d1".into()])).unwrap();
    let trigger = s
        .write(|db| lifecycle::event_tx(db, "t", "WORKER_USER_ANSWER_RECEIVED", json!({})))
        .unwrap();
    assert_eq!(complete(&s), "sprint_impact_pending");
    s.write(|db| sprint_impact::record_pending_tx(db, "t")).unwrap();
    assert!(s.read(|db| sprint_impact::prompt(db, "t")).unwrap().contains(&format!("trigger {trigger}")));
    // A stale trigger is refused; the newest one records the verdict.
    let args = |key: &str, t: i64| json!({"request_key":key,"trigger":t,"verdict":"unchanged","reason":"Not affected"});
    let stale = s
        .write(|db| sprint_impact::submit_tx(db, "t", "c", "k0", &args("k0", trigger - 1), None))
        .unwrap_err();
    assert_eq!(stale.code(), "sprint_impact_trigger_stale");
    let reply = s
        .write(|db| sprint_impact::submit_tx(db, "t", "c", "k1", &args("k1", trigger), None))
        .unwrap();
    assert_eq!(reply["verdict"], "unchanged");
    assert_eq!(s.read(|db| sprint_impact::pending(db, "t")).unwrap(), None);
    assert_ne!(complete(&s), "sprint_impact_pending");
    assert_eq!(s.read(|db| sprint_impact::feed_line(db, "t")).unwrap().unwrap()["outcome"], "unchanged");
}

fn revise(s: &Store, key: &str, trigger: i64) -> serde_json::Value {
    let checked = sprint_impact::Checked {
        revision_id: format!("sr_{key}"),
        sprint_id: "sp".into(),
        base_graph_revision: "g".into(),
        changes: json!([{"op":"update","member_id":"d1","expected_version":"1","description":"New"}]),
        affected: vec!["d1".into()],
        cancel: vec![],
        started_affected: vec![],
        context: json!([]),
        review_projection: json!([]),
    };
    let args = json!({"request_key":key,"trigger":trigger,"verdict":"revise","reason":"Changed"});
    s.write(|db| sprint_impact::submit_tx(db, "t", "c", key, &args, Some(checked))).unwrap()
}

fn finish_review(s: &Store, verdict: &str) {
    s.write(|db| {
        db.execute(
            "UPDATE runtime_nodes SET status='completed',result_json=? WHERE task_id='t' AND status='pending'",
            [json!({"verdict":verdict,"findings":[{"fix":"x"}],"report_ref":"r"}).to_string()],
        )?;
        review::advance_tx(db, "t")
    })
    .unwrap();
}

fn state(s: &Store) -> String {
    s.read(|db| Ok(db.query_row("SELECT state FROM runtime_tasks WHERE task_id='t'", [], |r| r.get(0))?))
        .unwrap()
}

#[test]
fn a_revision_is_reviewed_and_resolved_only_once_settled() {
    let (_p, s) = setup();
    s.write(|db| sprint_impact::bind_tx(db, "t", "sp", &["d1".into()])).unwrap();
    let trigger = s
        .write(|db| lifecycle::event_tx(db, "t", "USER_ANSWER_RECEIVED", json!({})))
        .unwrap();
    assert_eq!(revise(&s, "a", trigger)["review"], "scheduled");
    assert_eq!(state(&s), "REVIEWING");
    let held = s.read(|db| sprint_impact::revisions(db, "t")).unwrap();
    assert_eq!(held[0]["held"], json!(["d1"]));
    // Findings return it to the coordinator's state; the check stays open.
    finish_review(&s, "REQUEST_CHANGES");
    assert_eq!(state(&s), "RECONCILING");
    assert_eq!(s.read(|db| sprint_impact::revisions(db, "t")).unwrap()[0]["status"], "changes_requested");
    assert!(s.read(|db| sprint_impact::prompt(db, "t")).unwrap().contains("requested changes"));
    // A new revision supersedes it; approval still waits for the service.
    revise(&s, "b", trigger);
    finish_review(&s, "APPROVE");
    let all = s.read(|db| sprint_impact::revisions(db, "t")).unwrap();
    assert_eq!((all[0]["status"].as_str(), all[1]["status"].as_str()), (Some("superseded"), Some("approved")));
    assert_eq!(complete(&s), "sprint_impact_pending");
    s.write(|db| {
        sprint_impact::settle_tx(db, "t", "sr_b", sprint_impact::Settlement::Applied {
            actor: "runtime", payload: json!({"updated":["d1"],"added":[]}),
        })
    })
    .unwrap();
    assert_eq!(s.read(|db| sprint_impact::pending(db, "t")).unwrap(), None);
    let line = s.read(|db| sprint_impact::feed_line(db, "t")).unwrap().unwrap();
    assert_eq!((line["outcome"].as_str(), line["updated"].clone()), (Some("applied"), json!(["d1"])));
    assert_eq!(s.read(|db| sprint_impact::revisions(db, "t")).unwrap()[1]["held"], json!([]));
}

#[test]
fn a_revision_review_never_binds_the_tasks_own_completion() {
    let (_p, s) = setup();
    s.write(|db| sprint_impact::bind_tx(db, "t", "sp", &["d1".into()])).unwrap();
    let trigger = s.write(|db| lifecycle::event_tx(db, "t", "USER_ANSWER_RECEIVED", json!({}))).unwrap();
    revise(&s, "a", trigger);
    finish_review(&s, "APPROVE");
    s.write(|db| sprint_impact::settle_tx(db, "t", "sr_a", sprint_impact::Settlement::AwaitingOwner { reason: "owner_decision" }))
        .unwrap();
    // The revision's approval is not the task's review: completion still has its own gates.
    let code = complete(&s);
    assert!(!code.is_empty() && code != "sprint_impact_pending", "{code}");
}
