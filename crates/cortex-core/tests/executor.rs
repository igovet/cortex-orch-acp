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
