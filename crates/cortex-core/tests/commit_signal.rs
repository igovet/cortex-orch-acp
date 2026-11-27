mod support;

use cortex_core::{
    CoreError, CoreResult, OpenMode, Store, StoreChannel,
    store::{CommitSignal, executor::StoreExecutor},
};
use rusqlite::Transaction;
use std::{
    future::Future,
    pin::pin,
    sync::mpsc,
    task::{Context, Poll, Waker},
    time::Duration,
};
use support::Project;

fn hint(signal: &CommitSignal) -> Poll<()> {
    pin!(signal.notified()).poll(&mut Context::from_waker(Waker::noop()))
}

fn insert(tx: &Transaction<'_>, id: i64) -> CoreResult<()> {
    tx.execute("INSERT INTO signal_rows VALUES (?)", [id])?;
    Ok(())
}

fn fixture(project: &Project) -> Store {
    let store = project.open();
    store
        .write(|tx| {
            tx.execute_batch(
                "CREATE TABLE signal_rows (id INTEGER PRIMARY KEY);
                 CREATE TABLE signal_child (
                     id INTEGER REFERENCES signal_rows(id) DEFERRABLE INITIALLY DEFERRED
                 );",
            )?;
            Ok(())
        })
        .unwrap();
    store
}

fn purge_fixture(project: &Project, with_runtime: bool) -> Store {
    let store = project.open();
    store
        .write(|tx| {
            let root = project.0.to_str();
            tx.execute(
                "INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)",
                [root],
            )?;
            tx.execute(
                "INSERT INTO workspace_tasks(task_id,project_id,description,created_at)
                 VALUES ('draft','p','Fixture','now')",
                [],
            )?;
            if with_runtime {
                tx.execute("INSERT INTO tasks VALUES ('runtime',?,'now')", [root])?;
                tx.execute(
                    "INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at)
                     VALUES ('runtime','COMPLETED','Fixture','now','now')",
                    [],
                )?;
                tx.execute(
                    "INSERT INTO workspace_executions VALUES
                     ('execution','draft','runtime','session',1,1,'now')",
                    [],
                )?;
                tx.execute(
                    "UPDATE workspace_tasks SET execution_id='execution' WHERE task_id='draft'",
                    [],
                )?;
            }
            Ok(())
        })
        .unwrap();
    store
}

#[test]
fn successful_purge_leaves_one_wake_and_a_refused_purge_leaves_none() {
    for with_runtime in [false, true] {
        let project = Project::new();
        let store = purge_fixture(&project, with_runtime);
        let signal = project.key().commit_signal();

        assert!(hint(&signal).is_pending());
        assert_eq!(
            store.purge_workspace_task("p", "draft", false).unwrap()["deleted"],
            true
        );
        assert!(hint(&signal).is_ready());
        assert!(hint(&signal).is_pending(), "purge leaves just one permit");
        assert_eq!(support::count(&store, "workspace_tasks"), 0);
        assert_eq!(support::count(&store, "pending_deletions"), 0);

        assert_eq!(
            store
                .purge_workspace_task("p", "draft", false)
                .unwrap_err()
                .code(),
            "not_found"
        );
        assert!(hint(&signal).is_pending());
    }
}

#[test]
fn purge_signals_a_committed_intent_even_if_the_final_commit_fails() {
    let project = Project::new();
    let store = purge_fixture(&project, true);
    store
        .write(|tx| {
            tx.execute_batch(
                "CREATE TABLE hold_task (
                     task_id TEXT REFERENCES workspace_tasks(task_id) DEFERRABLE INITIALLY DEFERRED
                 );
                 INSERT INTO hold_task VALUES ('draft');",
            )?;
            Ok(())
        })
        .unwrap();
    let signal = project.key().commit_signal();
    let purge_with_deferred_constraint = || {
        let error = store.purge_workspace_task("p", "draft", false).unwrap_err();
        assert!(
            matches!(
                error,
                CoreError::Sql(rusqlite::Error::SqliteFailure(e, _))
                    if e.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_FOREIGNKEY
            ),
            "the deferred foreign key must refuse the final commit"
        );
    };

    assert!(hint(&signal).is_pending());
    purge_with_deferred_constraint();
    assert!(hint(&signal).is_ready(), "the new deletion intent committed");
    assert_eq!(support::count(&store, "pending_deletions"), 1);
    assert_eq!(support::count(&store, "workspace_tasks"), 1);
    assert!(hint(&signal).is_pending());

    purge_with_deferred_constraint();
    assert!(
        hint(&signal).is_pending(),
        "the intent already existed and deletion rolled back"
    );

    store
        .write(|tx| {
            tx.execute("DELETE FROM hold_task", [])?;
            Ok(())
        })
        .unwrap();
    assert!(hint(&signal).is_ready(), "consume the fixture write's permit");
    assert_eq!(
        store.purge_workspace_task("p", "draft", false).unwrap()["deleted"],
        true
    );
    assert!(
        hint(&signal).is_ready(),
        "the final deletion independently wakes the reader"
    );
    assert!(hint(&signal).is_pending());
    assert_eq!(support::count(&store, "pending_deletions"), 0);
}

#[test]
fn permits_coalesce_and_a_commit_during_polling_is_retained() {
    let project = Project::new();
    let store = fixture(&project);
    let signal = project.key().commit_signal();

    assert!(hint(&signal).is_pending());
    store.write(|tx| insert(tx, 1)).unwrap();
    assert!(hint(&signal).is_ready());

    // Simulate the poller reading after its first wake. It has no outstanding
    // Notified future while commits happen on this separate writable connection.
    let independent = Store::open(project.key(), OpenMode::RuntimeValidate).unwrap();
    store
        .read(|db| {
            let count = |db: &rusqlite::Connection| {
                db.query_row("SELECT count(*) FROM signal_rows", [], |r| r.get::<_, i64>(0))
            };
            assert_eq!(count(db)?, 1);
            for id in 2..=4 {
                independent.write(|tx| insert(tx, id))?;
            }
            assert_eq!(count(db)?, 1, "the original poll still holds its snapshot");
            Ok(())
        })
        .unwrap();

    assert!(hint(&signal).is_ready(), "a trailing poll is still owed");
    assert!(hint(&signal).is_pending(), "three commits leave just one permit");
    assert_eq!(support::count(&store, "signal_rows"), 4);
}

#[test]
fn full_and_relaxed_writes_signal_only_on_successful_row_commits() {
    let project = Project::new();
    let store = fixture(&project);
    let signal = project.key().commit_signal();
    let other_project = Project::new();
    let other_signal = other_project.key().commit_signal();
    let release_signal = cortex_core::ProjectKey::new(&project.0, StoreChannel::Release)
        .unwrap()
        .commit_signal();

    let _: CoreResult<()> = store.write_relaxed(|tx| {
        insert(tx, 1)?;
        Err(CoreError::Refused("test_rollback"))
    });
    assert!(hint(&signal).is_pending());
    assert_eq!(support::count(&store, "signal_rows"), 0);

    store.write_relaxed(|tx| insert(tx, 1)).unwrap();
    assert!(hint(&signal).is_ready());
    assert!(hint(&other_signal).is_pending());
    assert!(hint(&release_signal).is_pending());
    assert_eq!(support::count(&store, "signal_rows"), 1);
    assert!(hint(&signal).is_pending());

    assert!(
        store
            .write(|tx| {
                tx.execute("INSERT INTO signal_child VALUES (99)", [])?;
                Ok(())
            })
            .is_err(),
        "the body succeeded, but deferred FK validation refused COMMIT"
    );
    assert!(hint(&signal).is_pending());
    assert_eq!(support::count(&store, "signal_child"), 0);

    store
        .write(|tx| {
            assert_eq!(tx.execute("DELETE FROM signal_rows WHERE id = 99", [])?, 0);
            Ok(())
        })
        .unwrap();
    assert!(hint(&signal).is_pending());
}

#[test]
fn writer_signals_after_commit_not_after_body_or_failed_group() {
    let project = Project::new();
    drop(fixture(&project));
    let executor = StoreExecutor::open(project.key()).unwrap();
    let signal = project.key().commit_signal();

    let (entered, enter) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let pending = executor
        .write(move |tx| {
            insert(tx, 1)?;
            entered.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(5)).unwrap();
            Ok(())
        })
        .unwrap();
    enter.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(hint(&signal).is_pending());
    release.send(()).unwrap();
    pending.blocking_recv().unwrap().unwrap();
    assert!(hint(&signal).is_ready());

    assert_eq!(
        executor
            .write(|tx| {
                insert(tx, 2)?;
                Err::<(), _>(CoreError::Refused("test_savepoint"))
            })
            .unwrap()
            .blocking_recv()
            .unwrap()
            .unwrap_err()
            .code(),
        "test_savepoint"
    );
    assert!(hint(&signal).is_pending(), "savepoint-only rollback");
    assert!(
        executor
            .write(|tx| {
                tx.execute("INSERT INTO signal_child VALUES (99)", [])?;
                Ok(())
            })
            .unwrap()
            .blocking_recv()
            .unwrap()
            .is_err()
    );
    assert!(hint(&signal).is_pending(), "whole group failed to commit");
    assert!(hint(&signal).is_pending());
    let store = Store::open(project.key(), OpenMode::LiveReadOnly).unwrap();
    assert_eq!(support::count(&store, "signal_rows"), 1);
    assert_eq!(support::count(&store, "signal_child"), 0);
}

#[test]
fn deferred_and_store_jobs_signal_their_commits_not_the_job_result() {
    let project = Project::new();
    drop(fixture(&project));
    let executor = StoreExecutor::open(project.key()).unwrap();
    let signal = project.key().commit_signal();

    for succeeds in [false, true] {
        executor
            .write_deferred(move |tx| {
                insert(tx, 1)?;
                if succeeds {
                    Ok(())
                } else {
                    Err(CoreError::Refused("test_savepoint"))
                }
            })
            .unwrap();
        // A writer-lane read needs no hint, but first flushes the deferred batch.
        assert_eq!(
            executor
                .publication(|store| Ok(support::count(store, "signal_rows")))
                .unwrap()
                .blocking_recv()
                .unwrap()
                .unwrap(),
            i64::from(succeeds)
        );
        assert_eq!(hint(&signal).is_ready(), succeeds);
        assert!(hint(&signal).is_pending());
    }

    let running_read = executor
        .read_store(|store| Ok(support::count(store, "signal_rows")))
        .unwrap();
    assert_eq!(running_read.blocking_recv().unwrap().unwrap(), 1);
    assert!(hint(&signal).is_pending(), "reader-lane Work::Store");

    let error = executor
        .publication(|store| {
            store.write(|tx| insert(tx, 2))?;
            Err::<(), _>(CoreError::Refused("test_error_after_commit"))
        })
        .unwrap()
        .blocking_recv()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code(), "test_error_after_commit");
    assert!(hint(&signal).is_ready(), "an earlier commit stands despite the job error");
    assert!(hint(&signal).is_pending());
}
