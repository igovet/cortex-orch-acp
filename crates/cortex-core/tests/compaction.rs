mod support;
use cortex_core::{lifecycle::TaskMachine, store::CompactionBudget, Store};
use support::*;

fn budget() -> CompactionBudget {
    CompactionBudget {
        batch_rows: 7,
        max_batches: 1000,
        max_duration: std::time::Duration::from_secs(60),
        pause: std::time::Duration::ZERO,
        max_load_per_cpu: 0.0,
    }
}
fn activity(
    db: &rusqlite::Connection,
    task: &str,
    key: &str,
    tool_kind: Option<&str>,
    status: &str,
    reference: &str,
) {
    let mut payload = serde_json::json!({"kind":"worker_activity","status":status,"activity_ref":reference,"attempt_id":"xa"});
    if let Some(k) = tool_kind {
        payload["tool_kind"] = k.into();
    }
    db.execute("INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at,consumed_by) VALUES (?,'progress','a',?,?,'runtime','now','runtime-controller:1')", (task, key, payload.to_string())).unwrap();
}
/// A closed task with hidden chunks, intermediate and terminal tool states, evidence rows and a tail.
fn closed(s: &Store, task: &str, updated: &str, state: &str) {
    TaskMachine::new(s).create(task, "goal", "owner").unwrap();
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state=?, updated_at=? WHERE task_id=?", (state, updated, task))?;
        for i in 0..30 {
            activity(db, task, &format!("thought-{i}"), Some("agent_thought_chunk"), "in_progress", &format!("display:{i}"));
            activity(db, task, &format!("usage-{i}"), Some("usage_update"), "in_progress", &format!("usage:{i}"));
        }
        for i in 0..10 {
            let r = format!("tool:{i}");
            activity(db, task, &format!("pend-{i}"), None, "pending", &r);
            activity(db, task, &format!("prog-{i}"), Some("execute"), "in_progress", &r);
            activity(db, task, &format!("done-{i}"), Some("execute"), "completed", &r);
        }
        // An unfinished tool call keeps its only state; an error row never supersedes the call.
        activity(db, task, "lonely", Some("execute"), "in_progress", "tool:lonely");
        activity(db, task, "err-a", Some("execute"), "in_progress", "tool:err");
        activity(db, task, "err-b", Some("execute"), "failed", "tool-error:err");
        for (kind, key) in [("worker_result", "result"), ("worker_question", "question"), ("user_message", "um")] {
            db.execute("INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at) VALUES (?,?,'a',?,'{}','runtime','now')", (task, kind, key))?;
        }
        db.execute("INSERT INTO task_events(task_id,event_type,assignment_ref,dedup_key,payload_json,origin,created_at) VALUES (?,'progress','a','cfg','{\"kind\":\"worker_configuration_confirmed\"}','runtime','now')", [task])?;
        // The newest events are always kept, even hidden chunks.
        for i in 0..200 {
            activity(db, task, &format!("tail-{i}"), Some("agent_thought_chunk"), "in_progress", &format!("display:t{i}"));
        }
        Ok(())
    })
    .unwrap();
}
fn keys(s: &Store, task: &str) -> Vec<String> {
    s.read(|db| {
        Ok(db
            .prepare("SELECT dedup_key FROM task_events WHERE task_id=? ORDER BY id")?
            .query_map([task], |r| r.get(0))?
            .collect::<Result<_, _>>()?)
    })
    .unwrap()
}
const OLD: &str = "2020-01-01T00:00:00.000000+00:00";

#[test]
fn removes_only_superseded_and_hidden_rows_of_old_closed_tasks_and_keeps_evidence() {
    let p = Project::new();
    let s = p.open();
    closed(&s, "old-done", OLD, "COMPLETED");
    closed(&s, "old-cancelled", OLD, "CANCELLED");
    closed(&s, "recent", &cortex_core::now(), "COMPLETED");
    closed(&s, "open", OLD, "WAITING_USER");
    let before_recent = keys(&s, "recent");
    let before_open = keys(&s, "open");
    let report = s.compact_closed_task_activity(7, budget()).unwrap();
    assert!(!report.more && report.skipped.is_none());
    // 60 hidden + 10 pending + 10 in_progress superseded by a completed state, per old task.
    assert_eq!(report.rows_deleted, 2 * 80);
    assert_eq!(report.tasks_visited, 2);
    assert_eq!(
        keys(&s, "recent"),
        before_recent,
        "a recent closed task is untouched"
    );
    assert_eq!(keys(&s, "open"), before_open, "an open task is untouched");
    for task in ["old-done", "old-cancelled"] {
        let left = keys(&s, task);
        for kept in [
            "result",
            "question",
            "um",
            "cfg",
            "lonely",
            "err-a",
            "err-b",
            "done-0",
            "done-9",
            "tail-0",
            "tail-199",
            "activity_compaction",
        ] {
            assert!(left.iter().any(|k| k == kept), "{task} lost {kept}");
        }
        for gone in ["thought-0", "usage-29", "pend-3", "prog-9"] {
            assert!(!left.iter().any(|k| k == gone), "{task} kept {gone}");
        }
        let marker: String = s
            .read(|db| Ok(db.query_row("SELECT payload_json FROM task_events WHERE task_id=? AND dedup_key='activity_compaction'", [task], |r| r.get(0))?))
            .unwrap();
        let marker: serde_json::Value = serde_json::from_str(&marker).unwrap();
        assert_eq!(marker["kind"], "activity_compacted");
        assert_eq!(marker["rows_removed"], 80);
    }
    // A second pass has nothing to do and does not rescan.
    let again = s.compact_closed_task_activity(7, budget()).unwrap();
    assert_eq!((again.rows_deleted, again.tasks_visited), (0, 0));
}

#[test]
fn stays_incremental_within_the_batch_budget_and_finishes_later() {
    let p = Project::new();
    let s = p.open();
    closed(&s, "old", OLD, "FAILED");
    let mut small = budget();
    small.max_batches = 2;
    let first = s.compact_closed_task_activity(7, small).unwrap();
    assert_eq!(first.rows_deleted, 14);
    assert!(first.more);
    let rest = s.compact_closed_task_activity(7, budget()).unwrap();
    assert_eq!(first.rows_deleted + rest.rows_deleted, 80);
    assert!(!rest.more);
}

#[test]
fn a_task_with_a_live_process_or_running_node_is_never_compacted() {
    let p = Project::new();
    let s = p.open();
    closed(&s, "old", OLD, "COMPLETED");
    s.write(|db| {
        db.execute("INSERT INTO capacity_reservations VALUES ('r','c','old',NULL,'xa','worker','reserved','now','now',NULL)", [])?;
        Ok(())
    })
    .unwrap();
    let before = keys(&s, "old");
    let report = s.compact_closed_task_activity(7, budget()).unwrap();
    assert_eq!(report.rows_deleted, 0);
    assert_eq!(keys(&s, "old"), before);
}

#[test]
fn invalid_age_is_refused_and_load_skips_the_pass() {
    let p = Project::new();
    let s = p.open();
    assert!(s.compact_closed_task_activity(0, budget()).is_err());
    closed(&s, "old", OLD, "COMPLETED");
    let mut loaded = budget();
    loaded.max_load_per_cpu = 1e-9;
    let report = s.compact_closed_task_activity(7, loaded).unwrap();
    assert_eq!((report.skipped, report.rows_deleted), (Some("load"), 0));
}

#[test]
fn compaction_does_not_publish_workspace_invalidations() {
    let p = Project::new();
    let s = p.open();
    closed(&s, "old", OLD, "COMPLETED");
    s.write(|db| {
        db.execute("INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)", [p.0.to_str()])?;
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,execution_id,created_at) VALUES ('wt','p','d',1,'ex','now')", [])?;
        db.execute("INSERT INTO workspace_executions VALUES ('ex','wt','old','sess',1,1,'now')", [])?;
        Ok(())
    })
    .unwrap();
    // Reopen so the connection-local hooks see the adapter tables, as the service does.
    drop(s);
    let s = p.open();
    let report = s.compact_closed_task_activity(7, budget()).unwrap();
    assert_eq!(report.rows_deleted, 80);
    assert_eq!(count(&s, "workspace_outbox"), 0);
    let version: i64 = s
        .read(|db| Ok(db.query_row("SELECT version FROM workspace_tasks", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(version, 1);
}

/// Manual measurement on a private copy: `CORTEX_COMPACT_PROJECT=<dir containing .cortex/>`.
#[test]
#[ignore]
fn measure_on_private_copy() {
    let dir = std::env::var("CORTEX_COMPACT_PROJECT").expect("CORTEX_COMPACT_PROJECT");
    let key = cortex_core::ProjectKey::new(
        std::path::Path::new(&dir),
        cortex_core::StoreChannel::Release,
    )
    .unwrap();
    let s = Store::open_for_workspace_retention(key).unwrap();
    let started = std::time::Instant::now();
    let mut total = (0u64, 0u64);
    for pass in 1.. {
        let r = s
            .compact_closed_task_activity(
                7,
                CompactionBudget {
                    max_duration: std::time::Duration::from_secs(60),
                    max_load_per_cpu: 0.0,
                    ..Default::default()
                },
            )
            .unwrap();
        total = (total.0 + r.rows_deleted, total.1 + r.payload_bytes_deleted);
        println!("pass {pass}: {r:?} elapsed={:?}", started.elapsed());
        if !r.more {
            break;
        }
    }
    println!("total rows={} payload_bytes={}", total.0, total.1);
}
