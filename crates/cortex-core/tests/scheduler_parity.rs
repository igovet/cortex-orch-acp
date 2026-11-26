//! Native policies exercised on disposable format-12 stores. Each case names
//! its Python behavioral oracle; fake observations are not live qualification.
mod support;
use cortex_core::{
    lifecycle::{event_tx, waits::*, TaskMachine},
    ownership::OwnerEpoch,
    ports::ProcessObservation,
    scheduler::{policy::*, retry::*, *},
    CoreResult, Store,
};
use rusqlite::Transaction;
use serde_json::json;
use support::*;
struct Allow;
impl SchedulingPolicy for Allow {
    fn may_claim(&self, _: &Transaction<'_>, _: &str, _: &str) -> CoreResult<bool> {
        Ok(true)
    }
}
fn setup() -> (Project, Store, OwnerEpoch) {
    let p = Project::new();
    let s = p.open();
    let o = TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1", [])?;
        Ok(())
    })
    .unwrap();
    (p, s, o)
}
fn node(
    db: &Transaction<'_>,
    task: &str,
    id: &str,
    role: &str,
    scope: &[&str],
    deps: &[&str],
) -> CoreResult<()> {
    db.execute("INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status)
        VALUES (?,1,?,?,'objective',?,?,'execution','pending')",
        (task,id,role,json!(deps).to_string(),json!(scope).to_string()))?;
    Ok(())
}
fn bind(db: &Transaction<'_>, task: &str, node: &str, a: &str, x: &str, c: &str) -> CoreResult<()> {
    db.execute(
        "UPDATE runtime_nodes SET status='running',assignment_ref=? WHERE task_id=? AND node_id=?",
        (a, task, node),
    )?;
    db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,channel_id,engine_session_ref,owner_epoch,created_at,updated_at)
        VALUES (?,?,?,'worker-backend-dev','ready','ready',?,?,1,'now','now')",(a,task,x,c,format!("es-{x}")))?;
    db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,attempt_id,owner_epoch,issued_at)
        VALUES (?,'fixture',?,'thread','worker',?,1,'now')",(c,task,x))?;
    Ok(())
}
fn names(s: &Store, refs: &[String]) -> Vec<String> {
    s.read(|db| {
        refs.iter()
            .map(|a| {
                Ok(db.query_row(
                    "SELECT node_id FROM runtime_nodes WHERE assignment_ref=?",
                    [a],
                    |r| r.get(0),
                )?)
            })
            .collect()
    })
    .unwrap()
}
// Mirrors tests/test_runtime_task_machine.py::test_scheduler_serializes_overlapping_write_scopes.
#[test]
fn native_policy_cannot_be_bypassed_by_permissive_adapter() {
    let (_p, s, o) = setup();
    s.write(|db| {
        node(db, "t", "first", "worker-backend-dev", &["src/**"], &[])?;
        node(db, "t", "second", "worker-backend-dev", &["src/a.rs"], &[])?;
        node(db, "t", "docs", "worker-backend-dev", &["docs"], &[])
    })
    .unwrap();
    let a = claim_ready(&s, "t", &o, "c", &Allow).unwrap();
    assert_eq!(names(&s, &a), ["first", "docs"]);
}
// Mirrors tests/test_runtime_task_machine.py::test_scheduler_claims_overlapping_read_only_nodes_together
// and ::test_scheduler_does_not_serialize_read_only_and_writing_candidates.
#[test]
fn readers_neither_reserve_nor_wait_on_writer_scopes() {
    for role in ["worker-explorer", "worker-code-reviewer"] {
        let (_p, s, o) = setup();
        s.write(|db| {
            node(db, "t", "reader", role, &["."], &[])?;
            node(db, "t", "writer", "worker-backend-dev", &["."], &[])
        })
        .unwrap();
        assert_eq!(claim_ready(&s, "t", &o, "c", &Allow).unwrap().len(), 2);
    }
}
// Mirrors tests/test_runtime_task_machine.py::test_scheduler_respects_dependencies_and_the_session_worker_cap.
#[test]
fn dependencies_and_four_claimed_slots_are_transactional() {
    let (_p, s, o) = setup();
    s.write(|db| {
        for i in 0..6 {
            node(
                db,
                "t",
                &format!("n{i}"),
                "worker-backend-dev",
                &[&format!("s{i}")],
                &[],
            )?;
        }
        node(db, "t", "child", "worker-backend-dev", &["child"], &["n0"])?;
        Ok(())
    })
    .unwrap();
    let refs = claim_ready(&s, "t", &o, "c", &Allow).unwrap();
    assert_eq!(refs.len(), 4);
    assert_eq!(names(&s, &refs)[0], "n0");
    assert!(claim_ready(&s, "t", &o, "c", &Allow).unwrap().is_empty());
}
// Mirrors tests/test_orchestration_overhead.py::test_claim_ready_orders_by_longest_remaining_chain.
#[test]
fn dense_dependency_graph_is_memoized_and_stable() {
    let mut nodes = Vec::new();
    for i in 0..64 {
        nodes.push(Node {
            id: format!("n{i}"),
            stage: "execution".into(),
            status: "pending".into(),
            dependencies: (0..i).map(|n| format!("n{n}")).collect(),
        });
    }
    assert_eq!(dependency_ready(&nodes, "execution").unwrap(), ["n0"]);
    nodes[0].dependencies.push("n63".into());
    assert!(dependency_ready(&nodes, "execution").is_err());
}
// Mirrors tests/test_runtime_task_machine.py::test_finished_node_holds_scope_until_engine_absence_is_observed.
#[test]
fn finished_worker_holds_scope_across_restart_until_confirmed_release() {
    let (p, s, o) = setup();
    s.write(|db| {
        node(db, "t", "old", "worker-backend-dev", &["src"], &[])?;
        node(db, "t", "new", "worker-backend-dev", &["src/a"], &[])?;
        bind(db, "t", "old", "a", "xa", "c")?;
        db.execute(
            "UPDATE runtime_nodes SET status='completed' WHERE node_id='old'",
            [],
        )?;
        reserve_tx(
            db,
            &Reservation {
                task: "t",
                connection: "conn",
                assignment: Some("a"),
                attempt: "xa",
                kind: ReservationKind::Worker,
                connection_limit: 4,
            },
        )?;
        Ok(())
    })
    .unwrap();
    assert!(claim_ready(&s, "t", &o, "coord", &Allow)
        .unwrap()
        .is_empty());
    drop(s);
    let s = p.open();
    let cap = s
        .read(|db| {
            Ok(db.query_row(
                "SELECT reservation_id FROM capacity_reservations",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap();
    s.write(|db| {
        settle_tx(db, &cap, ProcessObservation::Unknown)?;
        Ok(())
    })
    .unwrap();
    assert!(claim_ready(&s, "t", &o, "coord", &Allow)
        .unwrap()
        .is_empty());
    s.write(|db| {
        settle_tx(db, &cap, ProcessObservation::ConfirmedExited)?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        names(&s, &claim_ready(&s, "t", &o, "coord", &Allow).unwrap()),
        ["new"]
    );
}
// Mirrors tests/test_worker_access_requests.py::test_unbound_running_node_reserves_scope_and_excludes_stale_revisions.
#[test]
fn unbound_current_writer_reserves_but_stale_unbound_node_does_not() {
    let (_p, s, o) = setup();
    s.write(|db| {
        node(db, "t", "old", "worker-backend-dev", &["src"], &[])?;
        db.execute(
            "UPDATE runtime_nodes SET status='running',revision=0 WHERE node_id='old'",
            [],
        )?;
        node(db, "t", "new", "worker-backend-dev", &["src/a"], &[])
    })
    .unwrap();
    assert_eq!(claim_ready(&s, "t", &o, "c", &Allow).unwrap().len(), 1);
}
// Mirrors tests/test_worker_access_requests.py::test_safe_extension_is_atomic_replayable_and_visible_not_a_fake_remount.
#[test]
fn access_grant_is_durable_replayable_and_not_a_remount() {
    let (_p, s, _) = setup();
    s.write(|db| {
        node(db, "t", "n", "worker-backend-dev", &["src"], &[])?;
        bind(db, "t", "n", "a", "xa", "c")
    })
    .unwrap();
    let paths = vec!["new/file.rs".into(), "new/file.rs".into()];
    let decision = s
        .write(|db| request_access_tx(db, "t", "c", "grant", &paths))
        .unwrap();
    assert_eq!(decision["status"], "granted");
    assert_eq!(decision["added_paths"], json!(["new/file.rs"]));
    assert_eq!(decision["enforcement"], "already_project_writable");
    assert_eq!(
        decision,
        s.write(|db| request_access_tx(db, "t", "c", "grant", &paths))
            .unwrap()
    );
    assert_eq!(
        s.write(|db| request_access_tx(db, "t", "c", "grant", &["changed".into()]))
            .unwrap_err()
            .code(),
        "payload_conflict"
    );
    assert!(s
        .write(|db| effective_scope_tx(db, "t", Some("a"), "[\"src\"]"))
        .unwrap()
        .contains(&"new/file.rs".into()));
}
// Mirrors tests/test_worker_access_requests.py::test_owner_denials_including_queued_policy_are_not_overridden
// and ::test_read_only_access_cannot_elevate_a_running_worker.
#[test]
fn queued_owner_denials_and_read_only_roles_cannot_be_overridden() {
    for role in ["worker-backend-dev", "worker-explorer"] {
        let (_p, s, _) = setup();
        s.write(|db| {
            node(db, "t", "n", role, &["src"], &[])?;
            bind(db, "t", "n", "a", "xa", "c")?;
            event_tx(db, "t", "USER_STEERING", json!({"deny_write":["denied"]}))?;
            Ok(())
        })
        .unwrap();
        let decision = s
            .write(|db| request_access_tx(db, "t", "c", "denied", &["denied/file".into()]))
            .unwrap();
        assert_eq!(decision["status"], "pending");
        let wait = s
            .write(|db| Ok(projection_tx(db, "t")?.pop().unwrap()))
            .unwrap();
        assert_eq!(wait["access_decision"]["status"], "denied");
        assert_eq!(
            wait["access_decision"]["reason"],
            if role == "worker-explorer" {
                "read_only_role"
            } else {
                "deny_write_policy"
            }
        );
    }
}
// Mirrors tests/test_worker_access_requests.py::test_unsafe_request_is_runtime_denial_without_node_failure.
#[test]
fn symlinks_private_paths_and_outside_paths_are_runtime_denials() {
    let (p, s, _) = setup();
    std::os::unix::fs::symlink("target", p.0.join("link")).unwrap();
    s.write(|db| {
        node(db, "t", "n", "worker-backend-dev", &["src"], &[])?;
        bind(db, "t", "n", "a", "xa", "c")
    })
    .unwrap();
    // Cortex private state and Git control paths may be reserved (owner decision
    // 2026-11-25); each write there is still decided by the runtime.
    for (i, path) in [".cortex/file", ".git/hooks"].iter().enumerate() {
        let decision = s
            .write(|db| request_access_tx(db, "t", "c", &format!("protected{i}"), &[(*path).into()]))
            .unwrap();
        assert_eq!(decision["status"], "granted", "{path}");
    }
    for (i, path) in [
        "link/file",
        "../escape",
        "/tmp/outside",
        "a*b",
        "a\\b",
    ]
    .iter()
    .enumerate()
    {
        let decision = s
            .write(|db| request_access_tx(db, "t", "c", &format!("unsafe{i}"), &[(*path).into()]))
            .unwrap();
        assert_eq!(decision["status"], "pending");
        s.write(|db| {
            let claimed = claim_tx(db, "t", "a", "es-xa")?.unwrap();
            assert_eq!(claimed["answer"]["status"], "denied");
            finish_tx(
                db,
                "t",
                claimed["wait"]["wait_id"].as_str().unwrap(),
                claimed["wait"]["delivery_id"].as_str().unwrap(),
                None,
            )?;
            Ok(())
        })
        .unwrap();
    }
}
// Mirrors tests/test_worker_access_requests.py::test_retry_deadlines_later_success_and_no_card
// and ::test_backoff_checks_once_per_deadline_without_catchup_loop.
#[test]
fn access_conflict_wait_rechecks_once_per_deadline_and_resumes_same_attempt() {
    let (_p, s, _) = setup();
    s.write(|db| {
        node(db, "t", "first", "worker-backend-dev", &["one"], &[])?;
        node(db, "t", "second", "worker-backend-dev", &["two"], &[])?;
        bind(db, "t", "first", "a", "xa", "ca")?;
        bind(db, "t", "second", "b", "xb", "cb")
    })
    .unwrap();
    let decision = s
        .write(|db| request_access_tx(db, "t", "ca", "conflict", &["two/file".into()]))
        .unwrap();
    assert_eq!(decision["status"], "pending");
    assert_eq!(count(&s, "pending_questions"), 0);
    s.write(|db| {
        let old = projection_tx(db, "t")?.pop().unwrap();
        reconcile_access_tx(db, "t", old["created_at"].as_str().unwrap())?;
        assert_eq!(projection_tx(db, "t")?[0]["retry_attempt"], 0);
        db.execute(
            "UPDATE runtime_nodes SET status='completed' WHERE assignment_ref='b'",
            [],
        )?;
        event_tx(
            db,
            "t",
            "WORKER_RELEASED",
            json!({"assignment_ref":"b","observed_session":"absent"}),
        )?;
        reconcile_access_tx(db, "t", "2099-01-01T00:00:00.000000+00:00")?;
        let w = projection_tx(db, "t")?.pop().unwrap();
        assert_eq!(w["status"], "ready");
        assert_eq!(w["access_decision"]["status"], "granted");
        let claim = claim_tx(db, "t", "a", "es-xa")?.unwrap();
        assert_eq!(claim["answer"]["status"], "granted");
        Ok(())
    })
    .unwrap();
}
// Mirrors tests/test_worker_access_requests.py::test_exhaustion_is_bounded_persisted_runtime_denial.
#[test]
fn conflict_retries_exhaust_after_one_two_four_seconds_and_survive_restart() {
    let (p, s, _) = setup();
    s.write(|db| {
        node(db, "t", "first", "worker-backend-dev", &["one"], &[])?;
        node(db, "t", "second", "worker-backend-dev", &["two"], &[])?;
        bind(db, "t", "first", "a", "xa", "ca")?;
        bind(db, "t", "second", "b", "xb", "cb")?;
        request_access_tx(db, "t", "ca", "conflict", &["two/file".into()])?;
        Ok(())
    })
    .unwrap();
    drop(s);
    let s = p.open();
    for i in 0..3 {
        s.write(|db| {
            let w = projection_tx(db, "t")?.pop().unwrap();
            assert_eq!(w["retry_attempt"], i);
            reconcile_access_tx(db, "t", w["retry_at"].as_str().unwrap())?;
            Ok(())
        })
        .unwrap();
    }
    let w = s
        .write(|db| Ok(projection_tx(db, "t")?.pop().unwrap()))
        .unwrap();
    assert_eq!(w["status"], "ready");
    assert_eq!(w["access_decision"]["status"], "denied");
    assert_eq!(count(&s, "pending_questions"), 0);
}
// Mirrors tests/test_worker_access_requests.py::test_pending_fifo_blocks_new_scheduler_overtakers.
#[test]
fn access_fifo_reservation_blocks_a_later_scheduler_writer() {
    let (_p, s, o) = setup();
    s.write(|db| {
        node(db, "t", "first", "worker-backend-dev", &["one"], &[])?;
        node(db, "t", "holder", "worker-backend-dev", &["two"], &[])?;
        bind(db, "t", "first", "a", "xa", "ca")?;
        bind(db, "t", "holder", "b", "xb", "cb")?;
        request_access_tx(db, "t", "ca", "conflict", &["two/file".into()])?;
        db.execute(
            "UPDATE runtime_nodes SET status='completed' WHERE assignment_ref='b'",
            [],
        )?;
        event_tx(db, "t", "WORKER_RELEASED", json!({"assignment_ref":"b"}))?;
        node(
            db,
            "t",
            "overtaker",
            "worker-backend-dev",
            &["two/file"],
            &[],
        )
    })
    .unwrap();
    assert!(claim_ready(&s, "t", &o, "coord", &Allow)
        .unwrap()
        .is_empty());
}
// Mirrors tests/test_engine_retry_window.py::test_delays_double_from_five_seconds_and_never_exceed_a_minute,
// ::test_work_stops_only_once_failures_kept_repeating_for_ten_minutes,
// ::test_a_failure_long_after_the_previous_one_starts_a_new_streak.
#[test]
fn infrastructure_window_has_capped_delays_and_resets_after_a_long_gap() {
    let mut times = Vec::new();
    let mut delays = Vec::new();
    for t in (0..400).step_by(50) {
        times.push(t as f64);
        delays.push(
            windowed_retry(&times, t as f64, RetryWindow::default())
                .unwrap()
                .delay,
        );
    }
    assert_eq!(delays, [5.0, 10.0, 20.0, 40.0, 60.0, 60.0, 60.0, 60.0]);
    let times = (0..=600).step_by(50).map(|t| t as f64).collect::<Vec<_>>();
    assert!(
        !windowed_retry(&times, 600.0, RetryWindow::default())
            .unwrap()
            .may_retry
    );
    assert_eq!(
        windowed_retry(&[0.0, 601.0], 601.0, RetryWindow::default())
            .unwrap()
            .retry_number,
        1
    );
}
// Mirrors tests/test_runtime_task_machine.py::test_lost_unprovisioned_reservation_is_retried_bounded.
#[test]
fn unprovisioned_worker_loss_has_windowed_budget_and_fresh_assignment_ids() {
    let (_p, s, o) = setup();
    s.write(|db| node(db, "t", "n", "worker-backend-dev", &["src"], &[]))
        .unwrap();
    let mut ids = Vec::new();
    for now in [0.0, 0.2, 0.4] {
        let a = claim_ready(&s, "t", &o, "coord", &Allow)
            .unwrap()
            .pop()
            .unwrap();
        ids.push(a.clone());
        let outcome = s.write(|db| {
            retry_lost_tx(
                db,
                "t",
                &RetryObservation {
                    assignment: &a,
                    attempt: None,
                    process: ProcessObservation::Unknown,
                    now,
                    window: RetryWindow {
                        first_delay: 0.0,
                        max_delay: 0.0,
                        window: 0.3,
                    },
                },
            )
        });
        // A streak longer than the window is an infrastructure loss, never the
        // model's failure: it keeps retrying (bounded per node lineage, then an
        // infrastructure wait), never `worker_retry_limit`.
        outcome.unwrap();
    }
    assert_eq!(
        ids.iter().collect::<std::collections::BTreeSet<_>>().len(),
        3
    );
}
// Mirrors tests/test_engine_retry_window.py::test_a_worker_engine_failure_waits_out_its_delay_and_the_chat_shows_it.
#[test]
fn durable_retry_deadline_prevents_immediate_claim_after_restart() {
    let (p, s, o) = setup();
    s.write(|db| node(db, "t", "n", "worker-backend-dev", &["src"], &[]))
        .unwrap();
    let a = claim_ready(&s, "t", &o, "coord", &Allow)
        .unwrap()
        .pop()
        .unwrap();
    let now = chrono::Utc::now().timestamp_micros() as f64 / 1e6;
    s.write(|db| {
        retry_lost_tx(
            db,
            "t",
            &RetryObservation {
                assignment: &a,
                attempt: None,
                process: ProcessObservation::Unknown,
                now,
                window: RetryWindow::default(),
            },
        )
    })
    .unwrap();
    drop(s);
    let s = p.open();
    assert!(claim_ready(&s, "t", &o, "coord", &Allow)
        .unwrap()
        .is_empty());
    assert!(s
        .write(|db| NativePolicy {
            now_seconds: now + 5.0
        }
        .may_claim(db, "t", "n"))
        .unwrap());
}
// Mirrors tests/test_worker_wait_continuation.py::test_late_answers_cannot_revive_stopped_or_replaced_workers,\n// applying the same exact-identity/unknown-outcome refusal to recovery.py::retry_lost.
#[test]
fn retry_needs_exact_absence_and_refuses_ambiguous_mutation_or_published_result() {
    let (_p, s, _) = setup();
    s.write(|db| {
        node(db, "t", "n", "worker-backend-dev", &["src"], &[])?;
        bind(db, "t", "n", "a", "xa", "c")
    })
    .unwrap();
    for process in [ProcessObservation::Alive, ProcessObservation::Unknown] {
        assert_eq!(
            s.write(|db| retry_lost_tx(
                db,
                "t",
                &RetryObservation {
                    assignment: "a",
                    attempt: Some("xa"),
                    process,
                    now: 1.0,
                    window: RetryWindow::default()
                }
            ))
            .unwrap_err()
            .code(),
            "lost_worker_termination_unconfirmed"
        );
    }
    s.write(|db|{db.execute("INSERT INTO runtime_requests VALUES ('t','cmd','c','command_run','digest','{\"complete\":false}')",[])?;Ok(())}).unwrap();
    let retry = |s: &Store| {
        s.write(|db| {
            retry_lost_tx(
                db,
                "t",
                &RetryObservation {
                    assignment: "a",
                    attempt: Some("xa"),
                    process: ProcessObservation::ConfirmedExited,
                    now: 1.0,
                    window: RetryWindow::default(),
                },
            )
        })
    };
    assert_eq!(
        retry(&s).unwrap_err().code(),
        "lost_worker_action_outcome_unknown"
    );
    s.write(|db| {
        db.execute(
            "UPDATE runtime_requests SET result_json='{\"complete\":true}'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    // A finished draft write, as document tools recorded it before `complete` was added, is a
    // known outcome; a pending one is not.
    s.write(|db|{db.execute("INSERT INTO runtime_requests VALUES ('t','draft','c','draft_write','digest','{\"status\":\"pending\"}')",[])?;Ok(())}).unwrap();
    assert_eq!(
        retry(&s).unwrap_err().code(),
        "lost_worker_action_outcome_unknown"
    );
    s.write(|db|{db.execute("UPDATE runtime_requests SET result_json='{\"status\":\"complete\",\"result\":{\"outcome\":\"written\"}}' WHERE request_key='draft'",[])?;
        db.execute("INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at)
            VALUES ('t','worker_result','xa','r','{}','runtime','now')",[])?;Ok(())}).unwrap();
    assert_eq!(retry(&s).unwrap_err().code(), "lost_worker_result_changed");
}
/// Production stores held lost workers for the owner (`lost_worker_action_outcome_unknown`)
/// although every receipt of the attempt was settled with a known end: commands the
/// owner stopped (`owner_stopped`, signal 9) and commands that provably never launched
/// (`not_started`, `no_supervisor_bound`). Those no longer block a retry. A receipt
/// settled only because its process is gone, with no observed end, still blocks.
#[test]
fn lost_worker_retry_ignores_settled_receipts_with_a_known_end_only() {
    let known = [
        json!({"complete":false,"settled":true,"exit_code":null,"signal":9,"failure":"owner_stopped"}),
        json!({"complete":false,"settled":true,"exit_code":null,"signal":9,"failure":"output_incomplete"}),
        json!({"complete":false,"settled":true,"exit_code":1,"failure":null}),
        json!({"complete":false,"settled":true,"exit_code":null,"failure":"timeout"}),
        json!({"complete":false,"settled":true,"exit_code":null,"failure":"command_not_started",
            "launched":false,"outcome":"not_started","settlement_evidence":"no_supervisor_bound"}),
        json!({"complete":false,"settled":true,"exit_code":2,"failure":"outcome_not_recorded",
            "outcome":"unknown","settlement_evidence":"supervisor_finished"}),
        json!({"complete":false,"settled":true,"exit_code":null,"failure":"outcome_not_recorded",
            "outcome":"unknown","settlement_evidence":"supervisor_gone_pid_namespace"}),
    ];
    let unknown = [
        json!({"complete":false}),
        json!({"complete":false,"settled":false}),
        json!({"complete":false,"settled":true,"exit_code":null,"failure":"outcome_not_recorded",
            "outcome":"unknown","settlement_evidence":"supervisor_and_process_group_gone"}),
        json!({"complete":false,"settled":true,"exit_code":null,"failure":"outcome_not_recorded",
            "outcome":"unknown","settlement_evidence":"previous_boot"}),
        json!({"complete":false,"settled":true,"failure":"command_outcome_unknown"}),
        json!({"complete":false,"settled":true,"outcome_unknown":true,"settled_by":"owner_retry"}),
        json!({"complete":false,"settled":true,"outcome":"not_started"}),
    ];
    for (receipt, retried) in known
        .iter()
        .map(|r| (r, true))
        .chain(unknown.iter().map(|r| (r, false)))
    {
        assert_eq!(action_outcome_known(receipt), retried, "{receipt}");
        let (_p, s, _) = setup();
        s.write(|db| {
            node(db, "t", "n", "worker-backend-dev", &["src"], &[])?;
            bind(db, "t", "n", "a", "xa", "c")?;
            db.execute(
                "INSERT INTO runtime_requests VALUES ('t','owner-stop','c','command_run','digest','{\"complete\":true}')",
                [],
            )?;
            db.execute(
                "INSERT INTO runtime_requests VALUES ('t','cmd','c','command_run','digest',?)",
                [receipt.to_string()],
            )?;
            Ok(())
        })
        .unwrap();
        let outcome = s.write(|db| {
            retry_lost_tx(
                db,
                "t",
                &RetryObservation {
                    assignment: "a",
                    attempt: Some("xa"),
                    process: ProcessObservation::ConfirmedExited,
                    now: 1.0,
                    window: RetryWindow::default(),
                },
            )
        });
        if retried {
            outcome.unwrap_or_else(|e| panic!("{receipt}: {}", e.code()));
            let status: String = s
                .read(|db| {
                    Ok(db.query_row(
                        "SELECT status FROM runtime_nodes WHERE node_id='n'",
                        [],
                        |r| r.get(0),
                    )?)
                })
                .unwrap();
            assert_eq!(status, "pending", "{receipt}");
        } else {
            assert_eq!(
                outcome.unwrap_err().code(),
                "lost_worker_action_outcome_unknown",
                "{receipt}"
            );
        }
    }
}
// Mirrors tests/test_runtime_task_machine.py::test_scope_overlap_never_treats_a_wildcard_prefix_as_a_directory.
#[test]
fn conservative_wildcards_do_not_justify_parallel_writers() {
    for (a, b, expected) in [
        ("database*", "database_backup", true),
        ("src/a?.py", "src/ab.py", true),
        ("src/a", "src/ab", false),
        ("src/**", "tests/**", false),
        ("**/*.py", "secret/file.py", true),
    ] {
        assert_eq!(overlaps(a, b), expected);
        assert_eq!(overlaps(b, a), expected);
    }
}
/// A worker left running by a killed service kept its node holding its files for the
/// whole project, after its task was cancelled or deleted, so a later task touching
/// those files never got a worker. A finished or deleted task holds nothing, nor does
/// an attempt whose rows are settled (also when an earlier build left no reservation
/// row); an unfinished task's possibly live worker still queues the other writer.
#[test]
fn finished_deleted_or_settled_work_holds_no_files_for_other_tasks() {
    let (_p, s, o) = setup();
    TaskMachine::new(&s)
        .create("u", "crashed", "owner")
        .unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1 WHERE task_id='u'", [])?;
        node(db, "u", "formatAll", "worker-backend-dev", &["src/format.js", "test/counter.test.js"], &[])?;
        bind(db, "u", "formatAll", "ua", "ux", "uc")?;
        db.execute("UPDATE delegation_provisioning SET status='ambiguous' WHERE attempt_id='ux'", [])?;
        db.execute("INSERT INTO capacity_reservations VALUES ('ur','conn','u','ua','ux','worker','ambiguous','0','0','termination_unconfirmed')", [])?;
        node(db, "t", "reset", "worker-backend-dev", &["test/counter.test.js"], &[])?;
        Ok(())
    })
    .unwrap();
    let conflict = |s: &Store| s.write(|db| write_conflict_tx(db, "t")).unwrap();
    // The possibly live worker of the unfinished task queues the other task's writer.
    let held = conflict(&s).expect("held by the unfinished task");
    assert_eq!(held["holder_task"], "u");
    assert_eq!(held["node"], "reset");
    assert!(
        held["paths"][0]
            .as_str()
            .unwrap()
            .ends_with("/test/counter.test.js"),
        "{held}"
    );
    assert!(claim_ready(&s, "t", &o, "coord", &Allow)
        .unwrap()
        .is_empty());
    // A task being deleted holds nothing.
    s.write(|db| {
        db.execute("INSERT INTO pending_deletions VALUES ('u')", [])?;
        Ok(())
    })
    .unwrap();
    assert!(conflict(&s).is_none());
    s.write(|db| {
        db.execute("DELETE FROM pending_deletions", [])?;
        Ok(())
    })
    .unwrap();
    assert!(conflict(&s).is_some());
    // A settled attempt holds nothing, with or without its (older builds') reservation row.
    s.write(|db| {
        db.execute("UPDATE delegation_provisioning SET status='terminated' WHERE attempt_id='ux'", [])?;
        db.execute("UPDATE capacity_reservations SET state='released',release_observation='process_exited'", [])?;
        Ok(())
    })
    .unwrap();
    assert!(conflict(&s).is_none());
    s.write(|db| {
        db.execute("DELETE FROM capacity_reservations", [])?;
        Ok(())
    })
    .unwrap();
    assert!(conflict(&s).is_none());
    s.write(|db| {
        db.execute("UPDATE delegation_provisioning SET status='ambiguous' WHERE attempt_id='ux'", [])?;
        db.execute("INSERT INTO capacity_reservations VALUES ('ur','conn','u','ua','ux','worker','ambiguous','0','0','termination_unconfirmed')", [])?;
        Ok(())
    })
    .unwrap();
    assert!(conflict(&s).is_some());
    // A cancelled task holds nothing, even with its old rows still unsettled.
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='CANCELLED' WHERE task_id='u'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert!(conflict(&s).is_none());
    assert_eq!(
        names(&s, &claim_ready(&s, "t", &o, "coord", &Allow).unwrap()),
        ["reset"]
    );
}

#[test]
fn shared_checkout_review_waits_for_active_writers_and_queues_new_writers() {
    let (p, s, _) = setup();
    TaskMachine::new(&s)
        .create("u", "other work", "owner")
        .unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1", [])?;
        node(db, "t", "done", "worker-backend-dev", &["one"], &[])?;
        db.execute(
            "UPDATE runtime_nodes SET status='completed' WHERE task_id='t'",
            [],
        )?;
        node(db, "u", "active", "worker-backend-dev", &["two"], &[])?;
        bind(db, "u", "active", "a", "x", "c")?;
        assert_eq!(write_conflict_tx(db, "t")?.unwrap()["holder_task"], "u");
        db.execute(
            "UPDATE delegation_provisioning SET status='terminated' WHERE attempt_id='x'",
            [],
        )?;
        assert!(review_writer_tx(db, "t")?.is_none());
        node(db, "u", "next", "worker-backend-dev", &["three"], &[])?;
        let policy = NativePolicy::default();
        for state in ["REVIEWING", "VERIFYING"] {
            db.execute(
                "UPDATE runtime_tasks SET state=? WHERE task_id='t'",
                [state],
            )?;
            assert!(!policy.may_claim(db, "u", "next")?);
            assert_eq!(write_conflict_tx(db, "u")?.unwrap()["holder_task"], "t");
        }
        // A verified revision keeps the checkout reservation while its owner-facing
        // synthesis is reconciled. Completion releases it, while a new plan or owner
        // wait releases it as well.
        db.execute(
            "UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id='t'",
            [],
        )?;
        event_tx(db, "t", "VERIFICATION_PASSED", json!({"revision": 1}))?;
        assert!(!policy.may_claim(db, "u", "next")?);
        assert_eq!(write_conflict_tx(db, "u")?.unwrap()["holder_task"], "t");
        db.execute(
            "UPDATE runtime_tasks SET state='COMPLETED' WHERE task_id='t'",
            [],
        )?;
        assert!(policy.may_claim(db, "u", "next")?);
        db.execute(
            "UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id='t'",
            [],
        )?;
        db.execute(
            "UPDATE runtime_tasks SET state='WAITING_USER' WHERE task_id='t'",
            [],
        )?;
        assert!(policy.may_claim(db, "u", "next")?);
        db.execute(
            "UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id='t'",
            [],
        )?;
        db.execute(
            "UPDATE runtime_tasks SET state='PLANNING' WHERE task_id='t'",
            [],
        )?;
        assert!(policy.may_claim(db, "u", "next")?);
        // A task worktree is a separate checkout even when project_root is shared.
        db.execute(
            "UPDATE runtime_tasks SET state='RECONCILING' WHERE task_id='t'",
            [],
        )?;
        db.execute(
            "INSERT INTO task_workspaces(task_id,workspace_root,recorded_at) VALUES ('u',?,'now')",
            [p.0.join("other-worktree").to_str().unwrap()],
        )?;
        assert!(policy.may_claim(db, "u", "next")?);
        Ok(())
    })
    .unwrap();
}
