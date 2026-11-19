mod support;
use cortex_core::{lifecycle::TaskMachine, Store};
use serde_json::json;
use support::*;
fn bound() -> (Project, Store) {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s)
        .create("runtime", "goal", "owner")
        .unwrap();
    s.write(|db|{
        db.execute("INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)",[p.0.to_str()])?;
        db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,version,execution_id,created_at) VALUES ('task','p','description',1,'execution','now')",[])?;
        db.execute("INSERT INTO workspace_executions VALUES ('execution','task','runtime','session',1,1,'now')",[])?;
        Ok(())
    }).unwrap();
    (p, s)
}
#[test]
fn hooks_commit_private_change_versions_and_metadata_together() {
    let (_p, s) = bound();
    s.write(|db| {
        db.execute(
            "UPDATE runtime_tasks SET goal='private-never-broadcast'",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    assert_eq!(count(&s, "workspace_outbox"), 2);
    s.read(|db|{
        assert_eq!(db.query_row("SELECT version FROM workspace_tasks",[],|r|r.get::<_,i64>(0))?,2);
        assert_eq!(db.query_row("SELECT high_watermark FROM workspace_outbox_state",[],|r|r.get::<_,i64>(0))?,2);
        let digest:String=db.query_row("SELECT payload_digest FROM workspace_outbox WHERE kind='task'",[],|r|r.get(0))?;
        use sha2::{Digest,Sha256};
        let expected=json!({"projectId":"p","entityId":"task","kind":"task","entityVersion":"2","type":"invalidated","schemaVersion":"1.0","mappingVersion":"1"});
        assert_eq!(digest,format!("{:x}",Sha256::digest(serde_json::to_vec(&expected)?)));
        Ok(())
    }).unwrap();
}
#[test]
fn outbox_append_failure_aborts_original_write_versions_and_counter() {
    let (_p, s) = bound();
    s.write(|db|{db.execute_batch("CREATE TRIGGER inject_failure BEFORE INSERT ON workspace_outbox BEGIN SELECT RAISE(ABORT,'injected'); END;")?;Ok(())}).unwrap();
    assert!(s
        .write(|db| {
            db.execute("UPDATE runtime_tasks SET goal='lost'", [])?;
            Ok(())
        })
        .is_err());
    assert_eq!(count(&s, "workspace_outbox"), 0);
    assert_eq!(count(&s, "workspace_outbox_state"), 0);
    s.read(|db| {
        assert_eq!(
            db.query_row("SELECT goal FROM runtime_tasks", [], |r| r
                .get::<_, String>(0))?,
            "goal"
        );
        assert_eq!(
            db.query_row("SELECT version FROM workspace_tasks", [], |r| r
                .get::<_, i64>(0))?,
            1
        );
        Ok(())
    })
    .unwrap();
}
#[test]
fn unbound_writes_do_not_publish_and_connection_hooks_are_not_persistent() {
    let p = Project::new();
    let s = p.open();
    TaskMachine::new(&s)
        .create("unbound", "goal", "owner")
        .unwrap();
    assert_eq!(count(&s, "workspace_outbox"), 0);
    s.read(|db|{
        assert_eq!(db.query_row("SELECT count(*) FROM sqlite_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%'",[],|r|r.get::<_,i64>(0))?,0);
        assert_eq!(db.query_row("SELECT count(*) FROM sqlite_temp_master WHERE type='trigger' AND name LIKE 'workspace_outbox_%'",[],|r|r.get::<_,i64>(0))?,48);
        Ok(())
    }).unwrap();
}
#[test]
fn concurrent_connections_keep_transactional_outbox_sequences_unique() {
    let (p, s) = bound();
    let stores: Vec<_> = (0..4)
        .map(|_| Store::open(p.key(), cortex_core::OpenMode::RuntimeValidate).unwrap())
        .collect();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    let threads: Vec<_> = stores
        .into_iter()
        .map(|store| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..20 {
                    store
                        .write(|db| {
                            db.execute("UPDATE runtime_tasks SET goal='concurrent'", [])?;
                            Ok(())
                        })
                        .unwrap();
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    s.read(|db| {
        assert_eq!(
            db.query_row("SELECT count(*) FROM workspace_outbox", [], |r| r
                .get::<_, i64>(0))?,
            160
        );
        assert_eq!(
            db.query_row(
                "SELECT count(DISTINCT sequence) FROM workspace_outbox",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            160
        );
        assert_eq!(
            db.query_row(
                "SELECT high_watermark FROM workspace_outbox_state",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            160
        );
        assert_eq!(
            db.query_row("SELECT version FROM workspace_tasks", [], |r| r
                .get::<_, i64>(0))?,
            81
        );
        Ok(())
    })
    .unwrap();
}
#[test]
fn metadata_retention_keeps_counter_and_never_reuses_sequences() {
    let (_p, s) = bound();
    s.write(|db| {
        for _ in 0..5002 {
            db.execute("UPDATE runtime_tasks SET goal='updated'", [])?;
        }
        Ok(())
    })
    .unwrap();
    s.read(|db| {
        assert_eq!(
            db.query_row("SELECT count(*) FROM workspace_outbox", [], |r| r
                .get::<_, i64>(0))?,
            10_000
        );
        assert_eq!(
            db.query_row(
                "SELECT high_watermark FROM workspace_outbox_state",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            10_004
        );
        assert_eq!(
            db.query_row(
                "SELECT retained_floor FROM workspace_outbox_state",
                [],
                |r| r.get::<_, i64>(0)
            )?,
            4
        );
        Ok(())
    })
    .unwrap();
}
