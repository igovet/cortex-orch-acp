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
#[tokio::test]
async fn full_writer_admission_timeout_never_runs_the_unaccepted_job() {
    use std::time::{Duration, Instant};

    let p = Project::new();
    let executor = StoreExecutor::open(p.key()).unwrap();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let blocker = executor
        .publication(move |_| {
            entered.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(6)).unwrap();
            Ok(())
        })
        .unwrap();
    observed.await.unwrap();
    let queued: Vec<_> = (0..128)
        .map(|_| executor.publication(|_| Ok(())).unwrap())
        .collect();

    let started = Instant::now();
    let ticket = executor.submitted();
    let result = tokio::time::timeout(
        Duration::from_secs(4),
        executor.publication_wait(|store| {
            store.write(|db| {
                db.execute("CREATE TABLE unexpected_admission (value TEXT)", [])?;
                Ok(())
            })
        }),
    )
    .await
    .expect("admission must be bounded");
    assert_eq!(result.unwrap_err().code(), "busy");
    assert!(started.elapsed() >= Duration::from_secs(2));
    assert_eq!(executor.submitted(), ticket, "refusals are not enqueued");

    release.send(()).unwrap();
    blocker.await.unwrap().unwrap();
    for done in queued {
        done.await.unwrap().unwrap();
    }
    assert!(!executor
        .publication_wait(|store| {
            store.read(|db| {
                Ok(db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='unexpected_admission')",
                    [],
                    |r| r.get::<_, bool>(0),
                )?)
            })
        })
        .await
        .unwrap());
}

#[tokio::test]
async fn owner_reserve_is_bounded_and_keeps_ordinary_and_deferred_order() {
    use std::{future::{poll_fn, Future}, task::Poll, time::Duration};

    let p = Project::new();
    let executor = StoreExecutor::open(p.key()).unwrap();
    executor.write(|db| {
        db.execute_batch("CREATE TABLE reserve_order(value INTEGER)")?;
        Ok(())
    }).unwrap().await.unwrap().unwrap();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let blocker = executor.publication(move |_| {
        entered.send(()).unwrap();
        gate.recv_timeout(Duration::from_secs(6)).unwrap();
        Ok(())
    }).unwrap();
    observed.await.unwrap();
    let ordinary: Vec<_> = (0..128).map(|i| executor.write(move |db| {
        db.execute("INSERT INTO reserve_order VALUES (?)", [i])?;
        Ok(())
    }).unwrap()).collect();
    assert_eq!(executor.publication(|_| Ok(())).unwrap_err().code(), "busy");
    executor.write_deferred(|db| {
        db.execute("INSERT INTO reserve_order VALUES (128)", [])?;
        Ok(())
    }).unwrap();

    let mut admitted = Vec::new();
    for i in 129..137 {
        let mut request = Box::pin(executor.owner_publication(move |store| store.write(|db| {
            db.execute("INSERT INTO reserve_order VALUES (?)", [i])?;
            Ok(())
        })));
        poll_fn(|cx| {
            // The writer is blocked, so a pending result means the request was
            // enqueued; a full reserve would return immediately.
            assert!(request.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        }).await;
        admitted.push(request);
    }
    let ticket = executor.submitted();
    let rejected = executor.owner_publication(|_| -> cortex_core::CoreResult<()> {
        panic!("a refused owner request must never run")
    }).await.unwrap_err();
    assert_eq!(rejected.code(), "owner_queue_busy");
    assert_eq!(executor.submitted(), ticket);

    release.send(()).unwrap();
    blocker.await.unwrap().unwrap();
    for job in ordinary {
        job.await.unwrap().unwrap();
    }
    for request in admitted {
        request.await.unwrap();
    }
    executor.owner_publication(|store| store.write(|db| {
        db.execute("INSERT INTO reserve_order VALUES (137)", [])?;
        Ok(())
    })).await.unwrap();
    let order = executor.read(|db| {
        let mut query = db.prepare("SELECT value FROM reserve_order ORDER BY rowid")?;
        let rows = query.query_map([], |r| r.get::<_, i64>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }).unwrap().await.unwrap().unwrap();
    assert_eq!(order, (0..138).collect::<Vec<_>>());
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
/// The service unit starts the process at the lowest best-effort disk level for the task
/// processes it runs; the store's own connection lanes still read and commit at the
/// session default, so a busy disk cannot starve the owner's reads behind any process.
#[cfg(target_os = "linux")]
#[test]
fn store_lanes_run_at_the_session_disk_priority_under_a_lowered_service() {
    use cortex_core::io_priority;
    std::thread::spawn(|| {
        let lowest = (2 << 13) | 7;
        let lowered = unsafe { libc::syscall(libc::SYS_ioprio_set, 1, 0, lowest) } == 0;
        assert!(lowered);
        assert_eq!(io_priority::current(), Some(lowest));
        let p = Project::new();
        let executor = StoreExecutor::open(p.key()).unwrap();
        let level = |rx: tokio::sync::oneshot::Receiver<cortex_core::CoreResult<Option<i32>>>| {
            rx.blocking_recv().unwrap().unwrap()
        };
        for _ in 0..4 {
            let read = level(executor.read(|_| Ok(io_priority::current())).unwrap());
            assert!(!io_priority::below_default(read.unwrap()), "reader lane at {read:?}");
        }
        let written = level(executor.write(|_| Ok(io_priority::current())).unwrap());
        assert!(!io_priority::below_default(written.unwrap()), "writer lane at {written:?}");
        assert_eq!(io_priority::current(), Some(lowest), "the opening thread keeps its level");
        drop(executor);
    })
    .join()
    .unwrap();
}

/// An owner control write (a task's route change) must commit promptly while a
/// running command's monitor re-authorizes through the writer lane every 50 ms
/// and the command itself runs for a long time: no write lock or queue slot is
/// held across command execution.
#[tokio::test]
async fn owner_write_commits_within_a_bound_while_a_command_monitor_is_active() {
    use std::time::{Duration, Instant};

    let p = Project::new();
    let executor = std::sync::Arc::new(StoreExecutor::open(p.key()).unwrap());
    executor
        .write(|db| {
            db.execute("CREATE TABLE IF NOT EXISTS probe(v TEXT)", [])?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    // The monitor of a long-running command: one authority write per 50 ms.
    let monitor = {
        let (ex, stop) = (executor.clone(), stop.clone());
        tokio::spawn(async move {
            while !stop.load(std::sync::atomic::Ordering::SeqCst) {
                ex.write(|db| {
                    db.query_row("SELECT count(*) FROM tasks", [], |r| r.get::<_, i64>(0))?;
                    Ok(())
                })
                .unwrap()
                .await
                .unwrap()
                .unwrap();
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    };
    // The command itself: minutes in production, 1.5 s here, never inside the writer.
    let command = tokio::spawn(async { tokio::time::sleep(Duration::from_millis(1500)).await });
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut slowest = Duration::ZERO;
    for n in 0..5 {
        let started = Instant::now();
        executor
            .owner_publication(move |store| {
                store.write(|db| {
                    db.execute("INSERT INTO probe VALUES (?)", [n.to_string()])?;
                    Ok(())
                })
            })
            .await
            .unwrap();
        slowest = slowest.max(started.elapsed());
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    assert!(!command.is_finished(), "the command was still running");
    assert!(slowest < Duration::from_millis(100), "owner write waited {slowest:?}");
    stop.store(true, std::sync::atomic::Ordering::SeqCst);
    monitor.await.unwrap();
    command.await.unwrap();
}
