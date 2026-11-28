mod support;
use cortex_core::{
    events::StreamFrame, lifecycle::TaskMachine, projections::ProjectionReader, OpenMode, Store,
};
use serde_json::{json, Value};
use support::*;
use workspace_protocol::{
    core::{CoreQuery, Page, Target},
    read::QueryReply,
};
fn setup() -> (Project, Store) {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s)
        .create("runtime", "goal", "owner")
        .unwrap();
    s.write(|db| {
        db.execute("INSERT INTO workspace_projects VALUES ('p',?,'owner','2026-11-11T00:00:00Z',1)",[p.0.to_str()])?;
        db.execute("INSERT INTO acp_host_sessions VALUES ('session',?,'runtime','{}','{}','[]','2026-11-11T00:00:00Z',NULL)",[p.0.to_str()])?;
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,execution_id,created_at) VALUES ('task','p','Description',1,'execution','2026-11-11T00:00:00Z')",[])?;
        db.execute("INSERT INTO workspace_executions VALUES ('execution','task','runtime','session',1,1,'2026-11-11T00:00:00Z')",[])?;
        Ok(())
    }).unwrap();
    (p, s)
}
#[test]
fn board_batches_more_than_a_sprint_and_keeps_one_committed_watermark() {
    let (p, s) = setup();
    s.write(|db| {
        for i in 0..68 {
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES (?,'p','Saved task','now')",[format!("task-{i}")])?;
        }
        let payload=json!({"sessionUpdate":"usage_update"}).to_string();
        let mut insert=db.prepare("INSERT INTO host_updates VALUES ('session',?,?,'now')")?;
        for i in 0..100000 {insert.execute((i,&payload))?;}
        Ok(())
    }).unwrap();
    let r = reader(&p);
    let before = count(&s, "host_updates");
    let result = r
        .query(CoreQuery::TasksBoard(Target { id: "p".into() }))
        .unwrap();
    let QueryReply::Board(page) = result.data else {
        panic!("not a board")
    };
    assert_eq!(page.items.len(), 69);
    assert!(page.next_cursor.is_none());
    assert_eq!(page.high_watermark, result.high_watermark);
    assert_eq!(count(&s, "host_updates"), before);
    assert_eq!(
        r.query(CoreQuery::TasksBoard(Target {
            id: "another".into()
        }))
        .unwrap_err()
        .code(),
        "invalid_project"
    );
}
fn reader(p: &Project) -> ProjectionReader {
    ProjectionReader::new(
        Store::open(p.key(), OpenMode::LiveReadOnly).unwrap(),
        "p".into(),
        "owner".into(),
    )
    .unwrap()
}
fn get(r: &ProjectionReader) -> workspace_protocol::read::TaskProjection {
    match r
        .query(CoreQuery::TaskGet(Target { id: "task".into() }))
        .unwrap()
        .data
    {
        QueryReply::Task(t) => t,
        _ => panic!(),
    }
}
fn page(cursor: Option<String>) -> CoreQuery {
    CoreQuery::TasksPage(Page {
        target: "p".into(),
        cursor,
        limit: 1,
    })
}
#[test]
fn live_reader_is_query_only_reuses_generation_and_observes_committed_wal() {
    let (p, s) = setup();
    let r = reader(&p);
    assert_eq!(get(&r).runtime_state.as_deref(), Some("CREATED"));
    let initial = r.query(page(None)).unwrap();
    let sub = r
        .subscribe(&initial.generation, initial.high_watermark)
        .unwrap();
    s.write(|db| {db.execute("UPDATE runtime_tasks SET state='RECOVERING'",[])?;db.execute("INSERT INTO task_orchestration(task_id,state,updated_at,wait_reason) VALUES ('runtime','RECOVERING','now','turn_interrupted')",[])?;Ok(())}).unwrap();
    r.poll().unwrap();
    let Some(StreamFrame::Delta(batch)) = sub.next() else {
        panic!("no delta");
    };
    batch.validate().unwrap();
    assert_eq!(batch.from_sequence.get(), initial.high_watermark.get() + 1);
    let t = get(&r);
    assert_eq!(t.phase, "recovery");
    assert_eq!(t.wait_reason.as_deref(), Some("turn_interrupted"));
    let readonly = Store::open(p.key(), OpenMode::LiveReadOnly).unwrap();
    assert_eq!(
        readonly
            .write(|db| {
                db.execute("DELETE FROM workspace_tasks", [])?;
                Ok(())
            })
            .unwrap_err()
            .code(),
        "inspection_read_only"
    );
    assert_eq!(r.query(page(None)).unwrap().generation, initial.generation);
    assert_eq!(count(&s, "workspace_tasks"), 1);
}
/// D7: native hosts never write task_orchestration, so every native RECOVERING
/// task projected as unconfirmed and the desktop disabled Stop and Close.
#[test]
fn native_recovery_states_its_reason_only_with_settled_or_recorded_evidence() {
    let (p, s) = setup();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='RECOVERING'", [])?;
        Ok(())
    })
    .unwrap();
    // A Python-owned recovery keeps its own wait reason contract: unconfirmed.
    assert_eq!(get(&reader(&p)).wait_reason, None);
    s.write(|db| {
        cortex_core::lifecycle::event_tx(
            db,
            "runtime",
            "NATIVE_TASK_ADOPTED",
            json!({"execution_host":"rust"}),
        )?;
        Ok(())
    })
    .unwrap();
    let t = get(&reader(&p));
    assert_eq!(t.phase, "recovery");
    assert_eq!(t.execution_state, "recovering");
    assert_eq!(t.wait_reason.as_deref(), Some("turn_interrupted"));
    // A reservation without a confirmed release is not an ended turn.
    s.write(|db| {
        db.execute("INSERT INTO capacity_reservations VALUES ('runtime','fake','runtime',NULL,'runtime','worker','reserved','fixture','fixture',NULL)",[])?;
        Ok(())
    })
    .unwrap();
    let t = get(&reader(&p));
    assert_eq!(t.wait_reason, None);
    // It names which case holds: a session still being stopped decides by itself.
    assert!(
        t.warnings.iter().any(|w| w == "process_exit_pending"),
        "{:?}",
        t.warnings
    );
    // The host recorded the exit as unconfirmed: only the owner can end it now. The
    // reservation change republishes the task, so no screen keeps an older answer.
    let r = reader(&p);
    let initial = r.query(page(None)).unwrap();
    let sub = r
        .subscribe(&initial.generation, initial.high_watermark)
        .unwrap();
    s.write(|db| {
        db.execute("UPDATE capacity_reservations SET state='ambiguous',release_observation='termination_unconfirmed'", [])?;
        Ok(())
    })
    .unwrap();
    r.poll().unwrap();
    assert!(
        matches!(sub.next(), Some(StreamFrame::Delta(_))),
        "a reservation change republishes the task"
    );
    let t = get(&r);
    assert_eq!(t.wait_reason, None);
    assert!(
        t.warnings.iter().any(|w| w == "process_exit_unconfirmed"),
        "{:?}",
        t.warnings
    );
    assert!(!t.warnings.iter().any(|w| w == "process_exit_pending"));
    // Once the exit is confirmed, the same task is an ended turn again, with no hold.
    s.write(|db| {
        db.execute("UPDATE capacity_reservations SET state='released',release_observation='process_exited'", [])?;
        Ok(())
    })
    .unwrap();
    let t = get(&reader(&p));
    assert_eq!(t.wait_reason.as_deref(), Some("turn_interrupted"));
    assert!(!t.warnings.iter().any(|w| w.starts_with("process_exit_")));
    s.write(|db| {
        db.execute(
            "UPDATE capacity_reservations SET state='reserved',release_observation=NULL",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    // A recorded native recovery reason is always stated to the owner.
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET blocker='native_session_outcome_unknown'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        get(&reader(&p)).wait_reason.as_deref(),
        Some("native_session_outcome_unknown")
    );
}
/// A replan that cancelled a live worker waits for that worker's session to stop. The
/// hold stays `process_exit_pending`; it is named for the owner with the worker's role,
/// and ends once the cancellation is settled.
#[test]
fn a_replan_waiting_for_a_cancelled_worker_names_that_worker() {
    let (p, s) = setup();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='RECOVERING'", [])?;
        cortex_core::lifecycle::event_tx(db, "runtime", "NATIVE_TASK_ADOPTED", json!({"execution_host":"rust"}))?;
        db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,role_id,step,status,owner_epoch,created_at,updated_at)
            VALUES ('a1','runtime','worker-qa-engineer','engine','ready',1,'now','now')", [])?;
        cortex_core::lifecycle::event_tx(db, "runtime", "LIVE_ASSIGNMENTS_CANCELLED", json!({"assignment_refs":["a1"],"reason":"replanning"}))?;
        Ok(())
    })
    .unwrap();
    let t = get(&reader(&p));
    assert_eq!(t.phase, "recovery");
    assert!(
        t.warnings.iter().any(|w| w == "process_exit_pending"),
        "{:?}",
        t.warnings
    );
    assert!(
        t.warnings.iter().any(|w| w == "cancelled_worker_stopping"),
        "{:?}",
        t.warnings
    );
    assert_eq!(
        t.wait_reason.as_deref(),
        Some("cancelled_worker_stopping:qa-engineer")
    );
    // Once the cancellation is settled the same live row is not named as a cancelled worker.
    s.write(|db| {
        let through: i64 = db.query_row(
            "SELECT max(id) FROM runtime_events WHERE type='LIVE_ASSIGNMENTS_CANCELLED'",
            [],
            |r| r.get(0),
        )?;
        cortex_core::lifecycle::event_tx(
            db,
            "runtime",
            "LIVE_CANCELLATIONS_SETTLED",
            json!({"assignment_refs":["a1"],"through":through}),
        )?;
        Ok(())
    })
    .unwrap();
    let t = get(&reader(&p));
    assert!(
        !t.warnings.iter().any(|w| w == "cancelled_worker_stopping"),
        "{:?}",
        t.warnings
    );
    assert_eq!(t.wait_reason, None);
}
/// The automatic continuation projection says what its attempts went to, and names an
/// item the reviewer reported unreachable in two dead-end reviews.
#[test]
fn automatic_attempts_name_what_they_were_spent_on_and_a_repeated_review_dead_end() {
    let (p, s) = setup();
    let dead_end = |item: &str| json!({"blocked":[{"blocker":"tool_unavailable","item":item,"reason":"lines over the read limit"}],"verdict":"INCOMPLETE"});
    s.write(|db| {
        cortex_core::lifecycle::event_tx(
            db,
            "runtime",
            "AUTOMATIC_POLICY_BOUND",
            json!({"policy":"automatic_continuation"}),
        )?;
        cortex_core::lifecycle::event_tx(
            db,
            "runtime",
            "REVIEW_DEAD_END_ROUTED",
            dead_end("README.md (paged completely)"),
        )?;
        for reason in [
            "review_incomplete",
            "review_incomplete",
            "native_session_lost",
        ] {
            cortex_core::lifecycle::event_tx(
                db,
                "runtime",
                "AUTOMATIC_CONTINUATION",
                json!({"reason":reason}),
            )?;
        }
        Ok(())
    })
    .unwrap();
    let t = get(&reader(&p));
    let a = t.automatic_continuation.expect("automatic task");
    assert_eq!(
        a["spentOn"],
        json!([{"reason":"review_incomplete","count":2},{"reason":"native_session_lost","count":1}])
    );
    assert!(
        a.get("reviewDeadEnd").is_none(),
        "one dead end is not a repeat: {a}"
    );
    s.write(|db| {
        cortex_core::lifecycle::event_tx(
            db,
            "runtime",
            "REVIEW_DEAD_END_ROUTED",
            dead_end("README.md: every line over 2000 characters"),
        )?;
        Ok(())
    })
    .unwrap();
    let a = get(&reader(&p)).automatic_continuation.unwrap();
    assert_eq!(a["reviewDeadEnd"], json!({"item":"README.md","count":2}));
    // A newer dead end on another item: the repeated one may have been fixed.
    s.write(|db| {
        cortex_core::lifecycle::event_tx(
            db,
            "runtime",
            "REVIEW_DEAD_END_ROUTED",
            dead_end("CHANGELOG.md"),
        )?;
        Ok(())
    })
    .unwrap();
    assert!(get(&reader(&p))
        .automatic_continuation
        .unwrap()
        .get("reviewDeadEnd")
        .is_none());
}
#[test]
fn scoped_keyset_pages_expire_on_change_and_wrong_project_never_leaks() {
    let (p, s) = setup();
    s.write(|db| {db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES ('other','p','Other','now')",[])?;Ok(())}).unwrap();
    let r = reader(&p);
    let first = r.query(page(None)).unwrap();
    let QueryReply::Tasks(first) = first.data else {
        panic!()
    };
    let next = first.next_cursor.unwrap();
    assert!(r.query(page(Some(next.clone()))).is_ok());
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='WAITING_USER'", [])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        r.query(page(Some(next))).unwrap_err().code(),
        "snapshot_expired"
    );
    assert!(r
        .query(CoreQuery::TasksPage(Page {
            target: "foreign".into(),
            cursor: None,
            limit: 1
        }))
        .is_err());
    assert!(ProjectionReader::new(
        Store::open(p.key(), OpenMode::LiveReadOnly).unwrap(),
        "p".into(),
        "foreign".into()
    )
    .is_err());
    assert!(r
        .query(CoreQuery::TaskGet(Target {
            id: "foreign".into()
        }))
        .is_err());
}
#[test]
fn contradictory_owner_is_unknown_and_never_asserts_quiescence() {
    let (p, s) = setup();
    let r = reader(&p);
    s.write(|db| {
        db.execute("UPDATE workspace_executions SET owner_epoch=99", [])?;
        Ok(())
    })
    .unwrap();
    let t = get(&r);
    assert_eq!(t.phase, "unknown");
    assert_eq!(t.execution_state, "unknown");
    assert!(t.warnings.contains(&"contradictory_evidence".into()));
}
#[test]
fn filtered_history_pages_bind_session_and_never_expose_read_bodies_or_thoughts() {
    let (p, s) = setup();
    let updates = vec![
        json!({"sessionUpdate":"agent_thought_chunk","content":{"text":"PRIVATE THOUGHT"}}),
        json!({"sessionUpdate":"tool_call","toolCallId":"read","title":"project_read","kind":"read","rawInput":{"path":"README.md"},"rawOutput":"PRIVATE READ"}),
        json!({"sessionUpdate":"tool_call","toolCallId":"report","title":"draft_write","kind":"edit","rawInput":{"content":"REPORT BODY"},"rawOutput":"REPORT RESULT","content":[{"type":"diff","newText":"PRIVATE DIFF"}]}),
        json!({"sessionUpdate":"tool_call","toolCallId":"cmd","title":"Run","kind":"execute","rawInput":{"argv":["git","status"]}}),
        json!({"sessionUpdate":"tool_call_update","toolCallId":"cmd","status":"completed","rawInput":{},"rawOutput":"Selected command output"}),
    ];
    s.write(|db| {
        for (seq, u) in updates.iter().enumerate() {
            db.execute(
                "INSERT INTO host_updates VALUES ('session',?,?,'2026-11-11T00:00:00Z')",
                rusqlite::params![seq as i64, u.to_string()],
            )?;
        }
        Ok(())
    })
    .unwrap();
    let r = reader(&p);
    let mut cursor = None;
    let mut rows = Vec::new();
    loop {
        let reply = r
            .query(CoreQuery::History(Page {
                target: "task".into(),
                cursor,
                limit: 2,
            }))
            .unwrap();
        let QueryReply::History(p) = reply.data else {
            panic!()
        };
        rows.extend(p.items);
        cursor = p.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    let text = serde_json::to_string(&rows).unwrap();
    assert!(!text.contains("PRIVATE"));
    assert!(text.contains("README.md"));
    // A non-file call carries its real input and result; a file read keeps its link only.
    assert!(text.contains("REPORT BODY") && text.contains("REPORT RESULT"));
    assert!(text.contains("git status"));
    assert!(text.contains("Selected command output"));
    assert_eq!(rows.len(), 4);
    assert!(r
        .query(CoreQuery::History(Page {
            target: "foreign".into(),
            cursor: None,
            limit: 2
        }))
        .is_err());
}
#[test]
fn native_read_preserves_a_valid_long_description_and_reports_corrupt_history_as_unavailable() {
    let (p, s) = setup();
    let description = "a".repeat(31000);
    s.write(|db| {
        db.execute(
            "UPDATE workspace_tasks SET description=? WHERE task_id='task'",
            [&description],
        )?;
        db.execute(
            "INSERT INTO host_updates VALUES ('session',0,'invalid JSON','2026-11-11T00:00:00Z')",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let r = reader(&p);
    assert_eq!(get(&r).description, description);
    let QueryReply::History(page) = r
        .query(CoreQuery::History(Page {
            target: "task".into(),
            cursor: None,
            limit: 100,
        }))
        .unwrap()
        .data
    else {
        panic!()
    };
    assert_eq!(page.items.len(), 1);
    assert!(page.items[0].text.is_none());
    assert_eq!(page.items[0].availability, "unavailable");
}
/// The frozen output of the retired Python `task_projection` for a task whose
/// runtime task was interrupted (`RECOVERING`, `turn_interrupted`).
#[test]
fn native_task_projection_matches_frozen_oracle_for_interrupted_state() {
    let (p, s) = setup();
    s.write(|db|{db.execute("UPDATE runtime_tasks SET state='RECOVERING'",[])?;db.execute("INSERT INTO task_orchestration(task_id,state,updated_at,wait_reason) VALUES ('runtime','RECOVERING','now','turn_interrupted')",[])?;Ok(())}).unwrap();
    let r = reader(&p);
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/task-projection-interrupted.json")).unwrap();
    let mut actual = serde_json::to_value(get(&r)).unwrap();
    // The additive wire 1.13 binding is checked independently of the frozen oracle.
    assert_eq!(
        actual.as_object_mut().unwrap().remove("draftBinding"),
        Some(serde_json::json!({
            "ownerEpoch": "1", "runtimeTaskId": "runtime", "sessionId": "session"
        }))
    );
    // The confirmation time is the store's wall clock; everything else is frozen.
    assert!(actual["lastConfirmedAt"].is_string());
    assert_eq!(expected["lastConfirmedAt"], "<timestamp>");
    actual["lastConfirmedAt"] = expected["lastConfirmedAt"].clone();
    assert_eq!(actual, expected);
}
#[test]
fn a_task_the_owner_closed_reads_as_resolved_even_without_its_finish_receipt() {
    // Close task commits the cancellation at admission; an earlier host could then refuse
    // its finish (unconfirmed engine exit), so no OWNER_TASK_RESOLVED was ever written.
    let (p, s) = setup();
    let r = reader(&p);
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='CANCELLED',blocker='owner_resolved_without_certification' WHERE task_id='runtime'", [])?;
        Ok(())
    })
    .unwrap();
    let t = get(&r);
    assert_eq!(t.phase, "stopped");
    assert!(t
        .warnings
        .contains(&"owner_resolved_without_certification".into()));
    // A run the runtime cancelled for another reason is not an owner resolution.
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET blocker='task_stopped' WHERE task_id='runtime'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(!get(&r)
        .warnings
        .contains(&"owner_resolved_without_certification".into()));
}
/// Live run 2026-11-17: Finish chosen on the runtime's stop question cancelled the task
/// (`user_force_finished`), but the lists kept it in Needs you as a cancelled run. It is
/// the owner's own close, read from the blocker or, for any store, the recorded event.
#[test]
fn finish_chosen_on_the_stop_question_reads_as_the_owner_closing_the_task() {
    let (p, s) = setup();
    let r = reader(&p);
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='CANCELLED',blocker='user_force_finished' WHERE task_id='runtime'", [])?;
        Ok(())
    })
    .unwrap();
    let t = get(&r);
    assert_eq!(t.phase, "stopped");
    assert!(t
        .warnings
        .contains(&"owner_resolved_without_certification".into()));
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET blocker='task_stopped' WHERE task_id='runtime'", [])?;
        db.execute("INSERT INTO runtime_events(task_id,type,actor,payload_json,created_at) VALUES ('runtime','USER_FORCE_FINISHED','runtime','{}','now')", [])?;
        Ok(())
    })
    .unwrap();
    assert!(get(&r)
        .warnings
        .contains(&"owner_resolved_without_certification".into()));
}

/// A project with no task yet still answers its board: an empty page at a committed watermark. A client must treat that as
/// a real read of an empty project (it is not a placeholder), so Inbox and Home can say "nothing" instead of loading forever.
#[test]
fn a_project_with_no_task_answers_an_empty_board_read() {
    let p = Project::new();
    let s = p.open();
    s.write(|db| {
        db.execute("INSERT INTO workspace_projects VALUES ('p',?,'owner','2026-11-11T00:00:00Z',1)", [p.0.to_str()])?;
        Ok(())
    })
    .unwrap();
    let result = reader(&p)
        .query(CoreQuery::TasksBoard(Target { id: "p".into() }))
        .unwrap();
    let QueryReply::Board(page) = result.data else {
        panic!("not a board")
    };
    assert!(page.items.is_empty());
    assert!(page.next_cursor.is_none());
    assert_eq!(page.high_watermark, result.high_watermark);
}
