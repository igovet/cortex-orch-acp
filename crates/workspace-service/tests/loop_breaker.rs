//! How a worker attempt's end is classified, and which models a node lineage is
//! routed away from, on event shapes copied from owner stores (Youtube_Channels
//! t_f2c20f98e1da 24199-24577, Case_Unsealed t_476724b85809). No store is read here
//! and no engine runs: the in-attempt refusal limit and the restart resume have
//! their unit tests next to the code (`native_results`, `native_recovery`).
use cortex_core::{
    lifecycle::{event_tx, TaskMachine},
    OpenMode, ProjectKey, Store, StoreChannel,
};
use serde_json::{json, Value};
use std::os::unix::fs::DirBuilderExt;
use workspace_service::native_runtime::{
    lost_models, stderr_tail, worker_ending_class, worker_ending_facts, ENDING_STDERR_CHARS,
};

struct Project(std::path::PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "loop-breaker-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&path)
            .unwrap();
        Self(path)
    }
    fn store(&self) -> Store {
        let store = Store::open(
            ProjectKey::new(&self.0, StoreChannel::Dev).unwrap(),
            OpenMode::Initialize,
        )
        .unwrap();
        TaskMachine::new(&store)
            .create("t", "goal", "owner")
            .unwrap();
        store
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn facts(store: &Store, channel: &str, attempt: &str) -> Value {
    store
        .read(|db| worker_ending_facts(db, "t", channel, attempt))
        .unwrap()
}

/// 24210-24213: two `review_file_coverage_required` refusals, then `end_turn`.
#[test]
fn a_turn_ended_after_refusals_is_result_refused() {
    let p = Project::new();
    let s = p.store();
    let (x, ch) = ("xa_fd437a75582043149d2e15d9a553214e", "ch_365610c7fff8");
    s.write(|db| {
        event_tx(db,"t","NATIVE_ATTEMPT_DELIVERY",json!({"assignment_ref":"542ea56a","attempt":x}))?;
        for _ in 0..2 {
            event_tx(db,"t","WORKER_RESULT_REFUSED",json!({"assignment_ref":"542ea56a","attempt":x,"channel_id":ch,
                "code":"review_file_coverage_required","missing":["channels/life-is-broken/README.md (current complete read is missing or stale)"]}))?;
        }
        event_tx(db,"t","NATIVE_TURN_SETTLED",json!({"channel_id":ch,"stop_reason":"end_turn","writer":false}))?;
        Ok(())
    })
    .unwrap();
    let f = facts(&s, ch, x);
    assert_eq!(f["refused_results"], 2);
    assert_eq!(f["last_refusal"], "review_file_coverage_required");
    assert_eq!(worker_ending_class("turn_ended", &f), "result_refused");
    // The same refusals with a process that vanished first are not the model's.
    assert_eq!(worker_ending_class("process_exited", &f), "worker_lost");
}

/// 24224-24228: the runtime cancelled an `execute` permission under the read-only
/// rule, then the reviewer's process exited without ending its turn.
#[test]
fn a_process_that_vanished_after_a_runtime_cancel_is_worker_lost() {
    let p = Project::new();
    let s = p.store();
    let (x, ch) = ("xa_b02583faf8514a8db8947b1140c114c1", "ch_e143d2901979");
    s.write(|db| {
        event_tx(db,"t","NATIVE_ATTEMPT_DELIVERY",json!({"assignment_ref":"a2f4badf","attempt":x}))?;
        event_tx(db,"t","NATIVE_PERMISSION_REQUESTED",json!({"channel_id":ch,"kind":"execute","request_id":0,"tool":null,"turn_generation":4}))?;
        event_tx(db,"t","NATIVE_PERMISSION_DECIDED",json!({"channel_id":ch,"decider":"runtime",
            "outcome":{"optionId":"cancel","outcome":"selected"},"rule":"read_only","turn_generation":4}))?;
        Ok(())
    })
    .unwrap();
    let f = facts(&s, ch, x);
    assert_eq!(f["runtime_cancel"], "read_only_permission");
    assert_eq!(f["refused_results"], 0);
    assert_eq!(worker_ending_class("process_exited", &f), "worker_lost");
    // A model that ends its turn without submitting anything is its own failure.
    assert_eq!(worker_ending_class("turn_ended", &f), "indeterminate");
}

#[test]
fn owner_restart_sandbox_drift_and_degenerate_endings_are_classified() {
    let p = Project::new();
    let s = p.store();
    s.write(|db| {
        event_tx(db,"t","NATIVE_ATTEMPT_DELIVERY",json!({"assignment_ref":"a","attempt":"x"}))?;
        event_tx(db,"t","OWNER_TASK_PAUSED",json!({}))?;
        // Case_Unsealed 25199-25214: the command sandbox could not be prepared.
        db.execute("INSERT INTO runtime_requests VALUES ('t','cmd','c','command_run','digest',?)",
            [json!({"complete":false,"settled":true,"failure":"native_command_sandbox_unavailable","exit_code":null}).to_string()])?;
        event_tx(db,"t","WORKER_RESULT_REFUSED",json!({"attempt":"x","channel_id":"c","code":"review_project_revision_unstable"}))?;
        Ok(())
    })
    .unwrap();
    let f = facts(&s, "c", "x");
    assert_eq!(f["owner_paused"], true);
    assert_eq!(f["sandbox_unavailable"], true);
    // Checkout drift by another writer is never counted as the worker's refusal.
    assert_eq!(
        (f["refused_results"].as_i64(), f["drift_refusals"].as_i64()),
        (Some(0), Some(1))
    );
    assert_eq!(
        worker_ending_class("native_task_interrupted", &f),
        "owner_paused"
    );
    assert_eq!(worker_ending_class("owner_stopped", &f), "owner_paused");
    assert_eq!(
        worker_ending_class("process_exited", &f),
        "sandbox_unavailable"
    );
    assert_eq!(
        worker_ending_class("native_service_stopped", &f),
        "runtime_restart"
    );
    assert_eq!(
        worker_ending_class("native_turn_degenerate", &f),
        "degenerate_turn"
    );
    assert_eq!(
        worker_ending_class("native_turn_outcome_unknown", &f),
        "transport_failure"
    );
    let mut drift_only = f.clone();
    drift_only["sandbox_unavailable"] = json!(false);
    assert_eq!(
        worker_ending_class("turn_ended", &drift_only),
        "worker_lost"
    );
    let none = json!({"refused_results":0,"drift_refusals":0});
    assert_eq!(
        worker_ending_class("native_task_interrupted", &none),
        "runtime_restart"
    );
    assert_eq!(
        worker_ending_class("some_engine_error", &none),
        "engine_failure"
    );
}

/// Youtube t_f2c20: `_review_2` was lost four times on `gpt-6.2-sol`; its next
/// dispatch moves to another model. A model lost once, an owner pause or a restart
/// is not a reason to move.
#[test]
fn a_model_that_lost_the_lineage_twice_is_avoided() {
    let p = Project::new();
    let s = p.store();
    s.write(|db| {
        for (i, (node, model, class)) in [
            ("_review_2", "gpt-6.2-sol", "worker_lost"),
            ("_review_2", "gpt-6.1-terra", "worker_lost"),
            ("_fix_1__review_2", "gpt-6.2-sol", "transport_failure"),
            ("_review_2", "gpt-6.1-terra", "owner_paused"),
            ("_review_2", "gpt-6.1-terra", "runtime_restart"),
            ("_review_1", "sonnet", "worker_lost"),
            ("_review_1", "sonnet", "worker_lost"),
        ]
        .into_iter()
        .enumerate()
        {
            let a = format!("lost-{i}");
            event_tx(
                db,
                "t",
                "NATIVE_ROUTE_SELECTED",
                json!({"assignment_ref":a,"node":node,"route_engine":"codex-acp","model":model}),
            )?;
            event_tx(
                db,
                "t",
                "ASSIGNMENT_RETRIED",
                json!({"node":node,"revision":2,"previous_assignment":a,
                "failure_class":class,"failed_at":1795214807.0 + i as f64}),
            )?;
        }
        Ok(())
    })
    .unwrap();
    let lost = s.read(|db| lost_models(db, "t", "_review_2")).unwrap();
    assert_eq!(
        lost,
        vec![json!({"engine":"codex-acp","model":"gpt-6.2-sol","losses":2})]
    );
}

#[test]
fn the_stderr_tail_is_bounded() {
    let long = "x".repeat(5000) + "\nlast line\n";
    let tail = stderr_tail(&long);
    assert!(tail.chars().count() <= ENDING_STDERR_CHARS);
    assert!(tail.ends_with("last line"));
}
