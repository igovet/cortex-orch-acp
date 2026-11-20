mod support;
use cortex_core::store::executor::StoreExecutor;
use support::*;
#[test]
fn bounded_executor_owns_project_reuses_connections_and_queries_committed_wal() {
    let p = Project::new();
    let executor = StoreExecutor::open(p.key()).unwrap();
    assert!(StoreExecutor::open(p.key()).is_err());
    let id = executor
        .write(|db| {
            db.execute("CREATE TEMP TABLE writer_identity(value TEXT)", [])?;
            db.execute("INSERT INTO writer_identity VALUES ('same-connection')", [])?;
            db.execute(
                "INSERT INTO tasks VALUES ('t',?,'now')",
                [std::env::temp_dir().to_str()],
            )?;
            Ok(())
        })
        .unwrap();
    id.blocking_recv().unwrap().unwrap();
    assert_eq!(
        executor
            .write(
                |db| Ok(db.query_row("SELECT value FROM writer_identity", [], |r| r
                    .get::<_, String>(0))?)
            )
            .unwrap()
            .blocking_recv()
            .unwrap()
            .unwrap(),
        "same-connection"
    );
    for _ in 0..8 {
        assert_eq!(
            executor
                .read(|db| Ok(
                    db.query_row("SELECT count(*) FROM tasks", [], |r| r.get::<_, i64>(0))?
                ))
                .unwrap()
                .blocking_recv()
                .unwrap()
                .unwrap(),
            1
        );
    }
    drop(executor);
}
/// Deferred display writes: never reported, committed before any ordinary job
/// submitted after them, each in its own savepoint (a failing one is rolled back
/// alone), and on shutdown.
#[test]
fn deferred_writes_roll_back_alone_precede_later_jobs_and_commit_on_shutdown() {
    let p = Project::new();
    let executor = StoreExecutor::open(p.key()).unwrap();
    executor
        .write(|db| {
            db.execute("CREATE TABLE deferred_rows(value TEXT NOT NULL)", [])?;
            Ok(())
        })
        .unwrap()
        .blocking_recv()
        .unwrap()
        .unwrap();
    let first = executor
        .write_deferred(|db| {
            db.execute("INSERT INTO deferred_rows VALUES ('one')", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(executor.submitted(), first);
    executor
        .write_deferred(|db| {
            db.execute("INSERT INTO deferred_rows VALUES ('rolled back')", [])?;
            Err(cortex_core::CoreError::Refused("fixture_failure"))
        })
        .unwrap();
    executor
        .write_deferred(|db| {
            db.execute("INSERT INTO deferred_rows VALUES ('two')", [])?;
            Ok(())
        })
        .unwrap();
    assert!(executor.submitted() > first);
    let seen = executor
        .write(|db| {
            let rows = db
                .prepare("SELECT value FROM deferred_rows ORDER BY rowid")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .unwrap()
        .blocking_recv()
        .unwrap()
        .unwrap();
    assert_eq!(seen, ["one", "two"]);
    executor
        .write_deferred(|db| {
            db.execute("INSERT INTO deferred_rows VALUES ('at shutdown')", [])?;
            Ok(())
        })
        .unwrap();
    drop(executor);
    let store = p.open();
    let count: i64 = store
        .read(|db| Ok(db.query_row("SELECT count(*) FROM deferred_rows", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(count, 3);
}
