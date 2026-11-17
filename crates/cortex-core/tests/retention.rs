mod support;
use cortex_core::{
    store::{documents::Publisher, locks::AccessLock},
    OpenMode, Store,
};
use std::{fs, os::unix::fs::symlink, time::Duration};
use support::{count, Project};

fn setup(runtime: bool) -> (Project, Store) {
    let p = Project::new();
    let s = p.open();
    s.write(|db|{
        db.execute("INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)",[p.0.to_str()])?;
        for task in ["draft","other"] {
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,created_at) VALUES (?,'p','Fixture','now')",[task])?;
            db.execute("INSERT INTO workspace_task_options VALUES (?,'{}')",[task])?;
        }
        if runtime {
            db.execute("INSERT INTO tasks VALUES ('runtime',?,'now')",[p.0.to_str()])?;
            db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('runtime','COMPLETED','Fixture','now','now')",[])?;
            db.execute("INSERT INTO workspace_executions VALUES ('execution','draft','runtime','session',1,1,'now')",[])?;
            db.execute("UPDATE workspace_tasks SET execution_id='execution' WHERE task_id='draft'",[])?;
        }
        Ok(())
    }).unwrap();
    (p, s)
}
#[test]
fn draft_and_settled_runtime_purge_are_exact_and_emit_public_removal() {
    for runtime in [false, true] {
        let (_p, s) = setup(runtime);
        let published = if runtime {
            let pubr = Publisher::new(&s);
            Some(
                pubr.create_draft("runtime", "owner", "implementation", "create")
                    .unwrap(),
            )
        } else {
            None
        };
        let result = s.purge_workspace_task("p", "draft", true).unwrap();
        assert_eq!(result["deleted"], true);
        assert!(matches!(
            result["vacuum_status"].as_str(),
            Some("completed" | "not_needed")
        ));
        assert_eq!(count(&s, "workspace_tasks"), 1);
        assert_eq!(count(&s, "workspace_task_options"), 1);
        assert_eq!(count(&s, "runtime_tasks"), 0);
        assert_eq!(count(&s, "tasks"), 0);
        assert_eq!(count(&s, "pending_deletions"), 0);
        if let Some(d) = published {
            assert!(!d.draft_path.exists());
        }
        s.read(|db| {
            let n: i64 = db.query_row(
                "SELECT count(*) FROM workspace_outbox WHERE entity_id='draft' AND kind='task'",
                [],
                |r| r.get(0),
            )?;
            assert!(n > 0);
            assert!(db
                .prepare("PRAGMA foreign_key_check")?
                .query([])?
                .next()?
                .is_none());
            Ok(())
        })
        .unwrap();
        assert_eq!(
            s.purge_workspace_task("p", "draft", true)
                .unwrap_err()
                .code(),
            "not_found"
        );
    }
}
#[test]
fn retention_never_removes_active_foreign_or_symlink_sources_and_replays_only_its_intent() {
    let (_p, s) = setup(true);
    assert_eq!(
        s.purge_workspace_task("foreign", "draft", false)
            .unwrap_err()
            .code(),
        "not_found"
    );
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='PAUSED'", [])?;
        Ok(())
    })
    .unwrap();
    assert_eq!(
        s.purge_workspace_task("p", "draft", false)
            .unwrap_err()
            .code(),
        "active_execution"
    );
    assert_eq!(count(&s, "pending_deletions"), 0);
    s.write(|db| {
        db.execute("UPDATE runtime_tasks SET state='COMPLETED'", [])?;
        Ok(())
    })
    .unwrap();
    let d = Publisher::new(&s)
        .create_draft("runtime", "owner", "general", "create")
        .unwrap();
    let outside = s.key().root().join("owner.txt");
    fs::write(&outside, "keep").unwrap();
    fs::remove_file(&d.draft_path).unwrap();
    symlink(&outside, &d.draft_path).unwrap();
    assert!(s.purge_workspace_task("p", "draft", false).is_err());
    assert_eq!(count(&s, "pending_deletions"), 1);
    assert_eq!(fs::read_to_string(&outside).unwrap(), "keep");
    assert_eq!(count(&s, "runtime_tasks"), 1);
    fs::remove_file(&d.draft_path).unwrap();
    assert_eq!(
        s.purge_workspace_task("p", "draft", false).unwrap()["deleted"],
        true
    );
    assert_eq!(fs::read_to_string(&outside).unwrap(), "keep");
}
#[test]
fn exclusive_access_and_unknown_starts_refuse_without_initializing_or_mutating() {
    let (p, s) = setup(false);
    let reader = Store::open(p.key(), OpenMode::LiveReadOnly).unwrap();
    assert_eq!(
        reader
            .purge_workspace_task("p", "draft", false)
            .unwrap_err()
            .code(),
        "inspection_read_only"
    );
    let lock = AccessLock::acquire(
        &s.key().directory().join(".access.lock"),
        false,
        false,
        Duration::ZERO,
    )
    .unwrap();
    assert_eq!(
        s.purge_workspace_task("p", "draft", false)
            .unwrap_err()
            .code(),
        "busy"
    );
    drop(lock);
    s.write(|db|{
        db.execute("INSERT INTO workspace_starts VALUES ('e','draft','p','owner','r','origin','event','unknown',1,'now')",[])?;
        Ok(())
    }).unwrap();
    assert_eq!(
        s.purge_workspace_task("p", "draft", false)
            .unwrap_err()
            .code(),
        "active_execution"
    );
    assert_eq!(count(&s, "workspace_tasks"), 2);
    assert_eq!(count(&s, "pending_deletions"), 0);
}
#[test]
fn adapter_reclamation_reports_busy_without_disguising_committed_deletion() {
    let (_p, s) = setup(false);
    let observer = rusqlite::Connection::open(s.key().database()).unwrap();
    observer
        .execute_batch("BEGIN; SELECT count(*) FROM workspace_tasks;")
        .unwrap();
    let result = s.purge_workspace_task("p", "draft", true).unwrap();
    assert_eq!(result["deleted"], true);
    assert_eq!(result["vacuum_status"], "failed");
    assert_eq!(
        result["vacuum_error"],
        "space reclamation failed; deletion remains committed"
    );
    assert_eq!(count(&s, "workspace_tasks"), 1);
    assert_eq!(count(&s, "workspace_task_options"), 1);
    assert_eq!(
        s.purge_workspace_task("p", "draft", true)
            .unwrap_err()
            .code(),
        "not_found"
    );
    observer.execute_batch("ROLLBACK").unwrap();
}
