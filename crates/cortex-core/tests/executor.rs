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

/// Group commit: write bodies queued together share one transaction, each in its own
/// savepoint. A failing body is rolled back alone; no reply (success or failure) is sent
/// before the shared COMMIT; and reads on the reader pool never wait for that transaction.
#[tokio::test]
async fn grouped_writes_fail_alone_reply_after_commit_and_never_block_reads() {
    use std::time::{Duration, Instant};
    let p = Project::new();
    let executor = StoreExecutor::open(p.key()).unwrap();
    executor
        .write(|db| {
            db.execute_batch("CREATE TABLE group_rows(value TEXT NOT NULL)")?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel::<()>();
    let blocker = executor
        .publication(move |_| {
            entered.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(())
        })
        .unwrap();
    observed.await.unwrap();
    let insert = |value: &'static str| {
        executor
            .write(move |db| {
                db.execute("INSERT INTO group_rows VALUES (?)", [value])?;
                Ok(())
            })
            .unwrap()
    };
    let mut one = insert("one");
    let mut failing = executor
        .write(|db| -> cortex_core::CoreResult<()> {
            db.execute("INSERT INTO group_rows VALUES ('rolled back')", [])?;
            Err(cortex_core::CoreError::Refused("fixture_failure"))
        })
        .unwrap();
    let (inside, inside_seen) = std::sync::mpsc::channel::<()>();
    let (finish, finish_gate) = std::sync::mpsc::channel::<()>();
    let mut slow = executor
        .write(move |db| {
            db.execute("INSERT INTO group_rows VALUES ('slow')", [])?;
            inside.send(()).unwrap();
            finish_gate.recv_timeout(Duration::from_secs(10)).unwrap();
            Ok(())
        })
        .unwrap();
    let after = insert("after");
    release.send(()).unwrap();
    blocker.await.unwrap().unwrap();
    inside_seen.recv_timeout(Duration::from_secs(10)).unwrap();
    // The group is open: 'one' and the failing body already ran in it, yet neither has
    // a reply, and a reader sees nothing of the uncommitted transaction, at once.
    let started = Instant::now();
    let visible: i64 = executor
        .read(|db| Ok(db.query_row("SELECT count(*) FROM group_rows", [], |r| r.get(0))?))
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(visible, 0);
    assert!(started.elapsed() < Duration::from_millis(500), "read waited {:?}", started.elapsed());
    let read_store = executor
        .read_store(|store| {
            store.write(|db| {
                db.execute("INSERT INTO group_rows VALUES ('reader')", [])?;
                Ok(())
            })
        })
        .unwrap()
        .await
        .unwrap();
    assert_eq!(read_store.unwrap_err().code(), "storage_read_lane");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(one.try_recv().is_err(), "a reply came before the commit");
    assert!(failing.try_recv().is_err(), "a failure reply came before the commit");
    assert!(slow.try_recv().is_err());
    finish.send(()).unwrap();
    one.await.unwrap().unwrap();
    assert_eq!(failing.await.unwrap().unwrap_err().code(), "fixture_failure");
    slow.await.unwrap().unwrap();
    after.await.unwrap().unwrap();
    let rows = executor
        .read(|db| {
            let rows = db
                .prepare("SELECT value FROM group_rows ORDER BY rowid")?
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rows, ["one", "slow", "after"]);
}

/// Display/progress rows committed on their own use `synchronous=NORMAL`; every other
/// write, and deferred rows committed together with one, uses FULL, and the connection
/// is back at FULL after a relaxed commit.
#[tokio::test]
async fn deferred_rows_alone_commit_relaxed_and_everything_else_full() {
    let p = Project::new();
    let executor = StoreExecutor::open(p.key()).unwrap();
    let level = |db: &rusqlite::Transaction<'_>| -> cortex_core::CoreResult<i64> {
        Ok(db.query_row("PRAGMA synchronous", [], |r| r.get(0))?)
    };
    executor
        .write(|db| {
            db.execute_batch("CREATE TABLE durability(kind TEXT, level INTEGER)")?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let signal = p.key().commit_signal();
    executor
        .write_deferred(move |db| {
            db.execute("INSERT INTO durability VALUES ('deferred alone', ?)", [level(db)?])?;
            Ok(())
        })
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(2), signal.notified())
        .await
        .unwrap();
    let (entered, observed) = tokio::sync::oneshot::channel();
    let (release, gate) = std::sync::mpsc::channel();
    let blocker = executor
        .publication(move |_| {
            entered.send(()).unwrap();
            gate.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
            Ok(())
        })
        .unwrap();
    observed.await.unwrap();
    executor
        .write_deferred(move |db| {
            db.execute("INSERT INTO durability VALUES ('deferred before a write', ?)", [level(db)?])?;
            Ok(())
        })
        .unwrap();
    let write = executor
        .write(move |db| {
            db.execute("INSERT INTO durability VALUES ('write', ?)", [level(db)?])?;
            Ok(())
        })
        .unwrap();
    release.send(()).unwrap();
    blocker.await.unwrap().unwrap();
    write.await.unwrap().unwrap();
    executor
        .publication(move |store| {
            store.write(|db| {
                db.execute("INSERT INTO durability VALUES ('publication', ?)", [level(db)?])?;
                Ok(())
            })
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let rows = executor
        .read(|db| {
            let rows = db
                .prepare("SELECT kind, level FROM durability ORDER BY rowid")?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let rows: Vec<(&str, i64)> = rows.iter().map(|(k, l)| (k.as_str(), *l)).collect();
    // 1 = NORMAL, 2 = FULL.
    assert_eq!(
        rows,
        [
            ("deferred alone", 1),
            ("deferred before a write", 2),
            ("write", 2),
            ("publication", 2)
        ]
    );
}

/// Synthetic write/read latency figures for the writer lane. Not a check: run with
/// `--ignored --nocapture`. `CORTEX_BENCH_ROOT` names a disposable project whose store
/// (a copy, never an owner store) is used instead of a fresh empty one.
#[tokio::test]
#[ignore]
async fn bench_writer_lane() {
    use std::time::{Duration, Instant};
    let fresh = Project::new();
    let key = match std::env::var_os("CORTEX_BENCH_ROOT").filter(|r| !r.is_empty()) {
        Some(root) => cortex_core::ProjectKey::new(root, cortex_core::StoreChannel::Dev).unwrap(),
        None => fresh.key(),
    };
    let executor = std::sync::Arc::new(StoreExecutor::open(key).unwrap());
    executor
        .write(|db| {
            db.execute_batch(
                "CREATE TABLE IF NOT EXISTS bench_rows(id INTEGER PRIMARY KEY, task TEXT, payload TEXT)",
            )?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let payload = "x".repeat(300);
    // Concurrent producers: up to 120 jobs in flight, as many tool calls and
    // activity rows arriving at once.
    let rows = 3000;
    let started = Instant::now();
    let mut flight: std::collections::VecDeque<tokio::sync::oneshot::Receiver<cortex_core::CoreResult<()>>> =
        std::collections::VecDeque::new();
    for n in 0..rows {
        if flight.len() >= 120 {
            let rx = flight.pop_front().unwrap();
            rx.await.unwrap().unwrap();
        }
        let payload = payload.clone();
        flight.push_back(
            executor
                .write(move |db| {
                    db.execute(
                        "INSERT INTO bench_rows(task,payload) VALUES (?,?)",
                        (format!("t{}", n % 7), payload),
                    )?;
                    Ok(())
                })
                .unwrap(),
        );
    }
    for rx in flight {
        rx.await.unwrap().unwrap();
    }
    let concurrent = started.elapsed();
    // One awaited write at a time: the latency of a single commit.
    let sequential_rows = 200;
    let started = Instant::now();
    for _ in 0..sequential_rows {
        let payload = payload.clone();
        executor
            .write(move |db| {
                db.execute("INSERT INTO bench_rows(task,payload) VALUES ('s',?)", [payload])?;
                Ok(())
            })
            .unwrap()
            .await
            .unwrap()
            .unwrap();
    }
    let sequential = started.elapsed();
    // Display-only rows committed later in batches, alone (no ordinary job after them).
    let started = Instant::now();
    for _ in 0..rows {
        let payload = payload.clone();
        executor
            .write_deferred(move |db| {
                db.execute("INSERT INTO bench_rows(task,payload) VALUES ('d',?)", [payload])?;
                Ok(())
            })
            .unwrap();
    }
    let deferred_submit = started.elapsed();
    loop {
        let done: i64 = executor
            .read(|db| Ok(db.query_row("SELECT count(*) FROM bench_rows WHERE task='d'", [], |r| r.get(0))?))
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        if done as usize >= rows {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let deferred = started.elapsed();
    // A read while 100 writes are queued: on the reader pool, and queued on the writer.
    let queue = |n: usize| -> Vec<_> {
        (0..n)
            .map(|_| {
                let payload = payload.clone();
                executor
                    .write(move |db| {
                        db.execute("INSERT INTO bench_rows(task,payload) VALUES ('q',?)", [payload])?;
                        Ok(())
                    })
                    .unwrap()
            })
            .collect()
    };
    let pending = queue(100);
    let started = Instant::now();
    executor
        .read(|db| Ok(db.query_row("SELECT count(*) FROM bench_rows", [], |r| r.get::<_, i64>(0))?))
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let reader = started.elapsed();
    for rx in pending {
        rx.await.unwrap().unwrap();
    }
    let pending = queue(100);
    let started = Instant::now();
    executor
        .publication(|store| store.read(|db| Ok(db.query_row("SELECT count(*) FROM bench_rows", [], |r| r.get::<_, i64>(0))?)))
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    let behind_writer = started.elapsed();
    for rx in pending {
        rx.await.unwrap().unwrap();
    }
    let commits = |relaxed: bool| {
        let payload = payload.clone();
        executor
            .publication(move |store| {
                let started = Instant::now();
                for _ in 0..200 {
                    let body = |db: &rusqlite::Transaction<'_>| -> cortex_core::CoreResult<()> {
                        db.execute("INSERT INTO bench_rows(task,payload) VALUES ('c',?)", [&payload])?;
                        Ok(())
                    };
                    if relaxed {
                        store.write_relaxed(body)?
                    } else {
                        store.write(body)?
                    }
                }
                Ok(started.elapsed())
            })
            .unwrap()
    };
    let full_commits = commits(false).await.unwrap().unwrap();
    let relaxed_commits = commits(true).await.unwrap().unwrap();
    let per = |d: Duration, n: usize| d.as_secs_f64() * 1e6 / n as f64;
    println!(
        "bench single-row commits: FULL {:.1} us, NORMAL {:.1} us",
        per(full_commits, 200),
        per(relaxed_commits, 200)
    );
    executor
        .write(|db| {
            db.execute_batch("DROP TABLE bench_rows")?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    println!(
        "bench concurrent: {rows} rows in {concurrent:?} = {:.1} us/row",
        per(concurrent, rows)
    );
    println!(
        "bench sequential: {sequential_rows} rows in {sequential:?} = {:.1} us/row",
        per(sequential, sequential_rows)
    );
    println!(
        "bench deferred: submit {deferred_submit:?}, committed {rows} rows in {deferred:?}"
    );
    println!("bench read behind 100 queued writes: reader {reader:?}, writer-queued {behind_writer:?}");
}
