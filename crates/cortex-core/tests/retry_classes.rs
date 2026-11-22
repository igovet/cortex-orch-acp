//! Why a worker attempt ended decides its retry: infrastructure never escalates,
//! never takes a rung and never stops on `worker_retry_limit`; a turn the model
//! ended after refused results is its own quality. Earlier-revision failed checks
//! are superseded by the same intent or disclosed after one continuation. Shapes
//! are copied from owner stores (Youtube_Channels t_f2c20f98e1da, cortex
//! t_5f5112ca685d, Case_Unsealed t_476724b85809); no store is read here.
mod support;
use cortex_core::{
    lifecycle::{automatic, event_tx, repair_checks, TaskMachine},
    ports::ProcessObservation,
    scheduler::retry::*,
    CoreResult, Store,
};
use rusqlite::Transaction;
use serde_json::{json, Value};
use support::*;

fn setup(revision: i64) -> (Project, Store) {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s).create("t", "goal", "owner").unwrap();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET state='REVIEWING',revision=?",
            [revision],
        )?;
        Ok(())
    })
    .unwrap();
    (p, s)
}
/// A running node of `revision` bound to a provisioned, now terminated attempt.
fn running(
    db: &Transaction<'_>,
    revision: i64,
    node: &str,
    assignment: &str,
    attempt: &str,
    channel: &str,
) -> CoreResult<()> {
    db.execute("INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status,assignment_ref)
        VALUES ('t',?,?,'worker-code-reviewer','review','[]','[\".\"]','review','running',?)",(revision,node,assignment))?;
    db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,channel_id,engine_session_ref,owner_epoch,created_at,updated_at)
        VALUES (?,'t',?,'worker-code-reviewer','ready','terminated',?,?,1,'now','now')",(assignment,attempt,channel,format!("es-{attempt}")))?;
    db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,assignment_id,attempt_id,owner_epoch,issued_at)
        VALUES (?,'fixture','t','thread','worker',?,?,1,'now')",(channel,assignment,attempt))?;
    event_tx(
        db,
        "t",
        "ASSIGNMENT_CREATED",
        json!({"assignment_ref":assignment,"node":node,"revision":revision}),
    )?;
    Ok(())
}
fn retry(s: &Store, assignment: &str, attempt: &str, now: f64) -> CoreResult<()> {
    s.write(|db| {
        retry_lost_tx(
            db,
            "t",
            &RetryObservation {
                assignment,
                attempt: Some(attempt),
                process: ProcessObservation::ConfirmedExited,
                now,
                window: RetryWindow::default(),
            },
        )
    })
}
fn events(s: &Store, kind: &str) -> Vec<Value> {
    s.read(|db| {
        Ok(db
            .prepare(
                "SELECT payload_json FROM runtime_events WHERE task_id='t' AND type=? ORDER BY id",
            )?
            .query_map([kind], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?)
    })
    .unwrap()
    .iter()
    .map(|raw| serde_json::from_str(raw).unwrap())
    .collect()
}

/// Youtube t_f2c20 24210-24214: a `_review_1` reviewer was refused twice with
/// `review_file_coverage_required`, ended its turn (`end_turn`) and the retry was
/// classed `worker_lost` (24214) — an infrastructure retry for the model's own
/// failure to submit. It is `result_refused` now: a quality retry that escalates.
#[test]
fn a_turn_ended_after_refused_results_is_a_quality_retry() {
    let (_p, s) = setup(2);
    let a = "542ea56a3a11fe8e600017059a2630fa66e18744ade274d1ff7cd7499f15fa97";
    let x = "xa_fd437a75582043149d2e15d9a553214e";
    s.write(|db| {
        running(db, 2, "_review_1", a, x, "ch_365610c7fff8")?;
        for _ in 0..2 {
            event_tx(db,"t","WORKER_RESULT_REFUSED",json!({"assignment_ref":a,"attempt":x,"channel_id":"ch_365610c7fff8",
                "code":"review_file_coverage_required","missing":["channels/life-is-broken/README.md (current complete read is missing or stale)"]}))?;
        }
        event_tx(db,"t","NATIVE_TURN_SETTLED",json!({"channel_id":"ch_365610c7fff8","stop_reason":"end_turn","writer":false}))?;
        assert!(record_attempt_ending_tx(db,"t",a,x,"turn_ended","result_refused",
            &json!({"cause":"turn_ended","refused_results":2,"last_refusal":"review_file_coverage_required"}))?);
        // Recorded once per attempt.
        assert!(!record_attempt_ending_tx(db,"t",a,x,"turn_ended","result_refused",&json!({}))?);
        Ok(())
    })
    .unwrap();
    retry(&s, a, x, 1795214029.1236641).unwrap();
    let retried = events(&s, "ASSIGNMENT_RETRIED").pop().unwrap();
    assert_eq!(retried["failure_class"], "result_refused");
    assert!(retried["failed_at"].is_null(), "{retried}");
    assert_eq!(
        retried["ending"]["last_refusal"],
        "review_file_coverage_required"
    );
    assert_eq!(events(&s, "ATTEMPT_ENDED").len(), 1);
    // A quality retry escalates the next dispatch of the node.
    assert!(s
        .read(|db| automatic::node_escalated(db, "t", "_review_1", 2))
        .unwrap());
}

/// Youtube t_f2c20 24224-24228: the runtime cancelled an `execute` permission under
/// the read-only rule and the reviewer's process vanished without ending its turn.
/// That is infrastructure (`worker_lost`): retried on the runtime's backoff, never
/// escalated, with the observed end kept on the retry.
#[test]
fn a_vanished_process_is_an_infrastructure_retry_with_its_end_recorded() {
    let (_p, s) = setup(2);
    let a = "a2f4badfdb49546779365b67c067bf45dd321b0f9466191e373c57f0995f589e";
    let x = "xa_b02583faf8514a8db8947b1140c114c1";
    s.write(|db| {
        running(db, 2, "_review_1", a, x, "ch_e143d2901979")?;
        event_tx(db,"t","NATIVE_PERMISSION_DECIDED",json!({"channel_id":"ch_e143d2901979","decider":"runtime",
            "outcome":{"optionId":"cancel","outcome":"selected"},"rule":"read_only","turn_generation":4}))?;
        record_attempt_ending_tx(db,"t",a,x,"process_exited","worker_lost",
            &json!({"cause":"process_exited","exit":"signal:9","exited_unprompted":true,"transport_close":"eof",
                "runtime_cancel":"read_only_permission","stderr_tail":""}))?;
        Ok(())
    })
    .unwrap();
    retry(&s, a, x, 1795214120.05599).unwrap();
    let retried = events(&s, "ASSIGNMENT_RETRIED").pop().unwrap();
    assert_eq!(retried["failure_class"], "worker_lost");
    assert_eq!(retried["ending"]["runtime_cancel"], "read_only_permission");
    assert_eq!(retried["ending"]["exit"], "signal:9");
    assert!(retried["retry_not_before"].as_f64().unwrap() > 1795214120.05599);
    assert!(!s
        .read(|db| automatic::node_escalated(db, "t", "_review_1", 2))
        .unwrap());
}

/// Youtube t_f2c20 24465-24577: `_review_2` lost its session four times within ten
/// minutes, then the windowed retry refused (`worker_retry_limit`, 24577) and the
/// task took a ladder rung. Losses are now bounded per node lineage across plan
/// revisions (6), and exhaustion is an infrastructure wait: never a refusal.
#[test]
fn infrastructure_exhaustion_is_a_wait_never_a_retry_limit() {
    let (_p, s) = setup(3);
    // Four losses of `_review_2` in revision 2 and two of its fix review in revision 3.
    s.write(|db| {
        for (i, (node, revision, at)) in [
            ("_review_2", 2, 1795214807.3826146),
            ("_review_2", 2, 1795214949.1407216),
            ("_review_2", 2, 1795215043.3237944),
            ("_review_2", 2, 1795215177.9177341),
            ("_fix_1__review_2", 3, 1795216000.0),
            ("_fix_1__review_2", 3, 1795216100.0),
        ]
        .into_iter()
        .enumerate()
        {
            event_tx(db,"t","ASSIGNMENT_RETRIED",json!({"failed_at":at,"failure_class":"worker_lost","infrastructure_retry":i+1,
                "node":node,"observed_session":"absent","previous_assignment":format!("lost-{i}"),"retry":i+1,
                "retry_not_before":at+5.0,"revision":revision}))?;
        }
        // An owner pause and a restart are not losses of the node.
        event_tx(db,"t","ASSIGNMENT_RETRIED",json!({"failed_at":1795216150.0,"failure_class":"owner_paused","node":"_review_2","revision":3}))?;
        event_tx(db,"t","ASSIGNMENT_RETRIED",json!({"failed_at":1795216160.0,"failure_class":"runtime_restart","node":"_review_2","revision":3}))?;
        running(db, 3, "_review_2", "a7", "x7", "c7")?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.read(|db| lineage_infrastructure_retries(db, "t", "_fix_2__review_2"))
            .unwrap(),
        6
    );
    let now = 1795216200.0;
    retry(&s, "a7", "x7", now).unwrap();
    let retried = events(&s, "ASSIGNMENT_RETRIED").pop().unwrap();
    assert_eq!(retried["infrastructure_wait"], true, "{retried}");
    assert_eq!(
        retried["retry_not_before"],
        json!(now + INFRASTRUCTURE_WAIT_FIRST)
    );
    let broken = events(&s, "LOOP_BROKEN");
    assert_eq!(broken.len(), 1);
    assert_eq!(broken[0]["action"], "infrastructure_wait");
    assert_eq!(broken[0]["kind"], "worker_lost");
    // The next loss waits longer and records no second LOOP_BROKEN.
    s.write(|db| {
        db.execute(
            "UPDATE runtime_nodes SET node_id='_review_2_old' WHERE node_id='_review_2'",
            [],
        )?;
        running(db, 3, "_review_2", "a8", "x8", "c8")
    })
    .unwrap();
    retry(&s, "a8", "x8", now + 400.0).unwrap();
    let retried = events(&s, "ASSIGNMENT_RETRIED").pop().unwrap();
    assert_eq!(
        retried["retry_not_before"],
        json!(now + 400.0 + 2.0 * INFRASTRUCTURE_WAIT_FIRST)
    );
    assert_eq!(events(&s, "LOOP_BROKEN").len(), 1);
    assert_eq!(infrastructure_wait(10), INFRASTRUCTURE_WAIT_MAX);
}

/// A streak of losses longer than the retry window keeps retrying on the longest
/// ordinary delay (Case_Unsealed t_476724b85809: 11 lost sessions, each window
/// refusal became a rung).
#[test]
fn a_long_streak_inside_the_lineage_budget_keeps_retrying() {
    let (_p, s) = setup(1);
    s.write(|db| {
        for (i, at) in [0.0, 300.0, 590.0].into_iter().enumerate() {
            event_tx(db,"t","ASSIGNMENT_RETRIED",json!({"failed_at":at,"failure_class":"worker_lost","node":"n","revision":1,"retry":i+1}))?;
        }
        running(db, 1, "n", "a", "x", "c")
    })
    .unwrap();
    retry(&s, "a", "x", 700.0).unwrap();
    let retried = events(&s, "ASSIGNMENT_RETRIED").pop().unwrap();
    assert_eq!(retried["retry_not_before"], json!(760.0));
    assert!(retried["infrastructure_wait"].is_null());
}

/// The owner's pause and a runtime restart retry the node at once, uncounted; only a
/// restart's runtime-settled effects (`settled_by: runtime_reverify`) are carried to
/// the fresh attempt for re-verification. The same receipt after an ordinary loss
/// still blocks the retry.
#[test]
fn a_restart_retries_at_once_and_carries_effects_to_reverify() {
    for (class, retried) in [("runtime_restart", true), ("worker_lost", false)] {
        let (_p, s) = setup(1);
        s.write(|db| {
            running(db, 1, "n", "a", "x", "c")?;
            db.execute("INSERT INTO runtime_requests VALUES ('t','w','c','project_write','digest',?)",
                [json!({"path":"src/lib.rs","settled":true,"outcome":"unknown","settled_by":"runtime_reverify"}).to_string()])?;
            record_attempt_ending_tx(db,"t","a","x","service_restarted",class,&json!({"cause":"service_restarted"}))?;
            Ok(())
        })
        .unwrap();
        let outcome = retry(&s, "a", "x", 50.0);
        if retried {
            outcome.unwrap();
            let r = events(&s, "ASSIGNMENT_RETRIED").pop().unwrap();
            assert_eq!(r["retry_not_before"], json!(50.0));
            assert_eq!(r["reverify"][0]["path"], "src/lib.rs");
            assert_eq!(
                s.read(|db| lineage_infrastructure_retries(db, "t", "n"))
                    .unwrap(),
                0
            );
        } else {
            assert_eq!(
                outcome.unwrap_err().code(),
                "lost_worker_action_outcome_unknown"
            );
        }
    }
}

#[test]
fn classes_match_the_shared_contract() {
    for class in [
        "engine_failure",
        "transport_failure",
        "worker_lost",
        "authorization_loss",
        "capacity_unavailable",
        "owner_paused",
        "runtime_restart",
        "sandbox_unavailable",
        "verifier_environment",
    ] {
        assert!(infrastructure_class(class), "{class}");
    }
    for class in ["result_refused", "degenerate_turn", "indeterminate", ""] {
        assert!(!infrastructure_class(class), "{class}");
    }
    assert_eq!(node_base("_fix_2__review_2"), "_review_2");
    assert_eq!(node_base("_fix_x_y"), "_fix_x_y");
}

// ---- prior failed checks -------------------------------------------------------

fn bind_node(
    db: &Transaction<'_>,
    revision: i64,
    node: &str,
    assignment: &str,
    status: &str,
    checks: Value,
) -> CoreResult<()> {
    let channel = format!("ch-{assignment}");
    let attempt = format!("xa-{assignment}");
    db.execute("INSERT INTO runtime_nodes(task_id,revision,node_id,role,objective,dependencies_json,scope_json,stage,status,assignment_ref,result_json)
        VALUES ('t',?,?,'worker-qa-engineer','work','[]','[\"channels\"]','execution',?,?,?)",
        (revision,node,status,assignment,json!({"checks":checks}).to_string()))?;
    db.execute("INSERT INTO delegation_provisioning(idempotency_key,task_id,attempt_id,role_id,step,status,channel_id,engine_session_ref,owner_epoch,created_at,updated_at)
        VALUES (?,'t',?,'worker-qa-engineer','ready','terminated',?,?,'1','now','now')",(assignment,&attempt,&channel,format!("engine-{attempt}")))?;
    db.execute("INSERT INTO trusted_channels(channel_id,secret_hash,task_id,principal_thread_id,role,assignment_id,attempt_id,owner_epoch,issued_at)
        VALUES (?,'fixture','t','thread','worker',?,?,1,'now')",(&channel,assignment,&attempt))?;
    event_tx(
        db,
        "t",
        "ASSIGNMENT_CREATED",
        json!({"assignment_ref":assignment,"node":node,"revision":revision}),
    )?;
    Ok(())
}
fn command(
    db: &Transaction<'_>,
    assignment: &str,
    reference: &str,
    argv: Value,
    profile: &str,
    code: i64,
) -> CoreResult<()> {
    db.execute(
        "INSERT INTO runtime_requests VALUES ('t',?,?,'command_run','bound',?)",
        (
            reference,
            format!("ch-{assignment}"),
            json!({"tool_call_ref":reference,"complete":true,"exit_code":code,"failure":Value::Null,
                "argv":argv,"cwd":"/mnt/Big_Storage/Web_Projects/Youtube_Channels","profile":profile,
                "revision_digest":"a".repeat(64)})
            .to_string(),
        ),
    )?;
    Ok(())
}
/// Youtube t_f2c20 node `confirm` (revision 3): a worker's own `python3 -c` probe
/// "no tracked changes since review" (`native-command:da5f1dfde6e993f5ee71b6b8`)
/// failed because the checkout held other files; it blocked completion through 22
/// returns to the coordinator and 9 continuations (23889-36662).
fn probe_fixture(db: &Transaction<'_>) -> CoreResult<()> {
    db.execute("UPDATE runtime_tasks SET revision=5", [])?;
    let probe = json!(["python3","-B","-I","-c","import subprocess, sys\nout = subprocess.run(['git', 'status', '--porcelain', '--', 'channels/life-is-broken'], capture_output=True, text=True).stdout.splitlines()\nsys.exit(1 if out else 0)\n"]);
    bind_node(
        db,
        3,
        "confirm",
        "confirm-3",
        "failed",
        json!([
        {"name":"entry gate","outcome":"passed","evidence_strength":"verified","tool_call_ref":"native-command:25c35e9a2025fddb0bdb0229"},
        {"name":"no tracked changes since review","outcome":"failed","evidence_strength":"verified","tool_call_ref":"native-command:da5f1dfde6e993f5ee71b6b8"}]),
    )?;
    command(
        db,
        "confirm-3",
        "native-command:da5f1dfde6e993f5ee71b6b8",
        probe,
        "read_only",
        1,
    )
}
fn verification() -> Value {
    json!({"revision_digest":"a".repeat(64),"checks":[]})
}

#[test]
fn a_later_pass_under_the_same_name_supersedes_an_earlier_revisions_failed_check() {
    let (_p, s) = setup(5);
    s.write(|db| {
        probe_fixture(db)?;
        bind_node(db,5,"recheck","recheck-5","completed",json!([{"name":"no tracked changes since review","outcome":"passed",
            "evidence_strength":"verified","tool_call_ref":"rerun","revision_digest":"a".repeat(64)}]))?;
        command(db, "recheck-5", "rerun", json!(["git","status","--porcelain","--","channels/life-is-broken/episodes"]), "read_only", 0)?;
        repair_checks::require_prior_worker_repairs(db, "t", 5, &verification())
    })
    .unwrap();
    assert!(events(&s, "PRIOR_WORKER_CHECK_DISCLOSED").is_empty());
}

/// The same argv in the same directory passing on another profile is the same intent.
#[test]
fn the_same_argv_and_directory_on_another_profile_supersedes_it() {
    let (_p, s) = setup(5);
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET revision=5", [])?;
        bind_node(db,3,"build","build-3","failed",json!([{"name":"cargo tests","outcome":"failed","evidence_strength":"verified","tool_call_ref":"fail"}]))?;
        command(db, "build-3", "fail", json!(["cargo","test"]), "read_only", 101)?;
        bind_node(db,5,"repair","repair-5","completed",json!([{"name":"workspace tests","outcome":"passed",
            "evidence_strength":"verified","tool_call_ref":"pass","revision_digest":"a".repeat(64)}]))?;
        command(db, "repair-5", "pass", json!(["cargo","test"]), "development", 0)?;
        repair_checks::require_prior_worker_repairs(db, "t", 5, &verification())
    })
    .unwrap();
}

/// Without a superseding pass the coordinator is sent back once; after that one
/// continuation the failure is disclosed as an `unverified` runtime limit and
/// completion proceeds. It is never recorded as passed.
#[test]
fn an_unrepaired_prior_failure_is_disclosed_at_once_for_a_probe_and_after_one_continuation_otherwise(
) {
    for (probe, kind) in [(true, "probe"), (false, "project_check")] {
        let (_p, s) = setup(5);
        s.write(|db| {
            if probe {
                probe_fixture(db)?;
            } else {
                db.execute("UPDATE runtime_tasks SET revision=5", [])?;
                bind_node(db,3,"build","build-3","failed",json!([{"name":"cargo tests","outcome":"failed","evidence_strength":"verified","tool_call_ref":"fail"}]))?;
                command(db, "build-3", "fail", json!(["cargo","test","--workspace"]), "development", 101)?;
            }
            Ok(())
        })
        .unwrap();
        let first =
            s.write(|db| repair_checks::require_prior_worker_repairs(db, "t", 5, &verification()));
        if probe {
            // A worker's own probe from an earlier failed revision is disclosed at once.
            first.unwrap();
        } else {
            assert_eq!(
                first.unwrap_err().code(),
                "completion_unresolved_prior_worker_check"
            );
        }
        if !probe {
            s.write(|db| {
                event_tx(
                    db,
                    "t",
                    "RETURNED_TO_COORDINATOR",
                    json!({"reason":"unresolved_prior_worker_check"}),
                )?;
                repair_checks::require_prior_worker_repairs(db, "t", 5, &verification())
            })
            .unwrap();
        }
        let disclosed = events(&s, "PRIOR_WORKER_CHECK_DISCLOSED");
        assert_eq!(disclosed.len(), 1);
        let limit = &disclosed[0]["limits"][0];
        assert_eq!(limit["outcome"], "unverified");
        assert_eq!(limit["source"], "runtime");
        assert_eq!(limit["kind"], kind);
        assert_eq!(limit["revision"], 3);
        assert!(limit["blocker"]
            .as_str()
            .unwrap()
            .contains("never as passed"));
    }
}
