mod support;
use cortex_core::{
    lifecycle::*,
    ownership,
    AssignmentState::{NeedsRevision, Reported, Running},
    CoreError,
    TaskState::*,
};
use serde_json::json;
use support::*;
#[test]
fn graph_preserves_escape_terminal_pause_and_non_linear_edges() {
    let states = [
        Created,
        Planning,
        AnswerReady,
        PlanReady,
        Executing,
        Reviewing,
        Verifying,
        Reconciling,
        WaitingUser,
        Recovering,
        Blocked,
        Paused,
        Failed,
        Cancelled,
        Completed,
    ];
    for source in states {
        for target in states {
            if source.terminal() {
                assert!(!allowed(source, target));
            }
            if source == Paused {
                assert_eq!(allowed(source, target), target == Cancelled);
            }
            if !source.terminal() && source != Paused {
                for emergency in [Blocked, Failed, Cancelled, Recovering] {
                    assert!(allowed(source, emergency));
                }
            }
        }
    }
    for (source, target) in [
        (Planning, AnswerReady),
        (Planning, Verifying),
        (Reviewing, Executing),
        (Verifying, Planning),
        (Verifying, Reviewing),
        (Reconciling, Reviewing),
        (WaitingUser, Reviewing),
    ] {
        assert!(allowed(source, target));
    }
    assert!(!allowed(Created, Executing));
    assert!(!allowed(Executing, Completed));
}
#[test]
fn transaction_idempotency_conflict_rollback_and_owner_fencing() {
    let p = Project::new();
    let s = p.open();
    let m = TaskMachine::new(&s);
    let owner = m.create("t", "goal", "owner").unwrap();
    let result = m
        .transition("t", &owner, "k", Planning, None, &FoundationGuards)
        .unwrap();
    let events = count(&s, "runtime_events");
    assert_eq!(
        result,
        m.transition("t", &owner, "k", Planning, None, &FoundationGuards)
            .unwrap()
    );
    assert_eq!(events, count(&s, "runtime_events"));
    assert_eq!(
        m.transition("t", &owner, "k", AnswerReady, None, &FoundationGuards)
            .unwrap_err()
            .code(),
        "payload_conflict"
    );
    let new = s
        .write(|db| ownership::acquire_tx(db, "t", "replacement", Some(owner.epoch)))
        .unwrap();
    assert_eq!(
        m.transition("t", &owner, "later", AnswerReady, None, &FoundationGuards)
            .unwrap_err()
            .code(),
        "stale_owner"
    );
    m.transition("t", &new, "answer", AnswerReady, None, &FoundationGuards)
        .unwrap();
    // The foundation must never certify completion with a boolean/default guard.
    assert_eq!(
        m.transition("t", &new, "finish", Completed, None, &FoundationGuards)
            .unwrap_err()
            .code(),
        "completion_domain_unavailable"
    );
}
#[test]
fn event_append_failure_rolls_back_state_and_request() {
    let p = Project::new();
    let s = p.open();
    let m = TaskMachine::new(&s);
    let owner = m.create("t", "g", "o").unwrap();
    s.write(|db|{db.execute_batch("CREATE TRIGGER fail_event BEFORE INSERT ON runtime_events BEGIN SELECT RAISE(ABORT,'injected'); END;")?;Ok(())}).unwrap();
    assert!(m
        .transition("t", &owner, "k", Planning, None, &FoundationGuards)
        .is_err());
    assert_eq!(count(&s, "runtime_requests"), 0);
    s.read(|db| {
        assert_eq!(
            db.query_row("SELECT state FROM runtime_tasks", [], |r| r
                .get::<_, String>(0))?,
            "CREATED"
        );
        Ok(())
    })
    .unwrap();
}
#[test]
fn review_waiver_is_durable_current_revision_not_a_caller_flag() {
    let p = Project::new();
    let s = p.open();
    let m = TaskMachine::new(&s);
    let owner = m.create("t", "g", "o").unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=3", [])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        m.transition("t", &owner, "a", Verifying, None, &FoundationGuards)
            .unwrap_err()
            .code(),
        "invalid_state_transition"
    );
    s.write(|db| {
        event_tx(db, "t", "REVIEW_NOT_REQUIRED", json!({"revision":2}))?;
        Ok(())
    })
    .unwrap();
    assert!(m
        .transition("t", &owner, "b", Verifying, None, &FoundationGuards)
        .is_err());
    s.write(|db| {
        event_tx(db, "t", "REVIEW_NOT_REQUIRED", json!({"revision":3}))?;
        Ok(())
    })
    .unwrap();
    m.transition("t", &owner, "c", Verifying, None, &FoundationGuards)
        .unwrap();
}
#[test]
fn attempt_graph_mirrors_assignment_status_and_requires_failure_reason() {
    let p = Project::new();
    let s = p.open();
    let m = TaskMachine::new(&s);
    let owner = m.create("t", "g", "o").unwrap();
    s.write(|db|{
        db.execute("INSERT INTO work_assignments VALUES ('a','t','n','{}',NULL,'planned','structured','k','now')",[])?;
        db.execute("INSERT INTO work_assignment_attempts VALUES ('xa','t','a',1,NULL,NULL,'{}','planned',NULL,'k','now')",[])?;Ok(())
    }).unwrap();
    m.transition_attempt("t", &owner, "run", "xa", Running, None)
        .unwrap();
    assert!(m
        .transition_attempt("t", &owner, "bad", "xa", NeedsRevision, None)
        .is_err());
    m.transition_attempt("t", &owner, "report", "xa", Reported, None)
        .unwrap();
    assert!(!attempt_allowed(Reported, Running));
    m.transition_attempt(
        "t",
        &owner,
        "revise",
        "xa",
        NeedsRevision,
        Some("review_changes"),
    )
    .unwrap();
    s.read(|db| {
        assert_eq!(
            db.query_row("SELECT status FROM work_assignments", [], |r| r
                .get::<_, String>(0))?,
            "needs_revision"
        );
        Ok(())
    })
    .unwrap();
    assert!(matches!(
        m.transition_attempt("t", &owner, "again", "xa", Running, None),
        Err(CoreError::Refused(_))
    ));
}
#[test]
fn concurrent_same_key_commits_once() {
    let p = Project::new();
    let s = std::sync::Arc::new(p.open());
    let owner = TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    let mut threads = Vec::new();
    for _ in 0..8 {
        let s = s.clone();
        let owner = owner.clone();
        threads.push(std::thread::spawn(move || {
            TaskMachine::new(&s)
                .transition("t", &owner, "same", Planning, None, &FoundationGuards)
                .unwrap()
        }));
    }
    for thread in threads {
        assert_eq!(thread.join().unwrap()["state"], "PLANNING");
    }
    assert_eq!(count(&s, "runtime_requests"), 1);
    // Creation + immutable routing-file snapshot + exactly one transition.
    assert_eq!(count(&s, "runtime_events"), 3);
}
