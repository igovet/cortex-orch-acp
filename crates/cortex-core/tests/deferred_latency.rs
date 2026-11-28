#![cfg(feature = "test-observations")]
mod support;

use cortex_core::{
    store::{append_public_tx, executor::StoreExecutor},
    test_observations::{Observation, Observer},
};
use std::{
    sync::mpsc,
    time::{Duration, Instant},
};
use support::Project;

#[test]
#[ignore = "timing diagnostic on a disposable local filesystem"]
fn deferred_batch_latency() {
    let count = 256;
    for blocked in [false, true] {
        let project = Project::new();
        let root = project.0.to_str().unwrap().to_owned();
        let executor = StoreExecutor::open(project.key()).unwrap();
        executor
            .write(move |tx| {
                tx.execute(
                    "INSERT INTO workspace_projects VALUES ('p',?,'owner','now',1)",
                    [root],
                )?;
                tx.execute_batch("CREATE TABLE fragments (id INTEGER PRIMARY KEY, body BLOB)")?;
                Ok(())
            })
            .unwrap()
            .blocking_recv()
            .unwrap()
            .unwrap();
        let wal = project.key().directory().join("cortex.sqlite3-wal");
        let initial_wal = wal.metadata().unwrap().len();
        let observer = Observer::start(project.0.clone());
        let (started, entered) = mpsc::channel();
        let blocker = blocked.then(|| {
            let job = executor
                .publication(move |_| {
                    started.send(()).unwrap();
                    std::thread::sleep(Duration::from_millis(180));
                    Ok(())
                })
                .unwrap();
            entered.recv_timeout(Duration::from_secs(2)).unwrap();
            job
        });
        let (send, receive) = mpsc::channel();
        let mut submitted = Vec::new();
        for id in 1..=count {
            let start = Instant::now();
            submitted.push(start);
            let send = send.clone();
            executor
                .write_deferred(move |tx| {
                    let queued = start.elapsed();
                    tx.execute("INSERT INTO fragments VALUES (?,zeroblob(1024))", [id])?;
                    append_public_tx(tx, "p", "fixture", "task", id)?;
                    send.send((id, queued)).unwrap();
                    Ok(())
                })
                .unwrap();
            std::thread::sleep(Duration::from_millis(4));
        }
        if let Some(blocker) = blocker {
            blocker.blocking_recv().unwrap().unwrap();
        }
        let mut queued = Vec::new();
        for id in 1..=count {
            let (observed, age) = receive.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!(observed, id);
            queued.push(age);
        }
        // The body reports before COMMIT; wait for the final committed row before
        // taking the WAL size and observations without forcing an early batch flush.
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let high_watermark = executor
                .read(|db| {
                    Ok(db.query_row(
                        "SELECT COALESCE((SELECT high_watermark FROM workspace_outbox_state WHERE project_id='p'),0)",
                        [],
                        |row| row.get::<_, i64>(0),
                    )?)
                })
                .unwrap()
                .blocking_recv()
                .unwrap()
                .unwrap();
            if high_watermark == count {
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        // This no-op writer barrier is submitted only after visibility of the last
        // committed row, so its reply cannot have made any measured batch commit.
        executor
            .publication(|_| Ok(()))
            .unwrap()
            .blocking_recv()
            .unwrap()
            .unwrap();
        let (observations, overflow) = observer.finish();
        assert!(!overflow);
        let mut committed = Vec::new();
        let transactions = observations.len();
        for observation in observations {
            let Observation::Commit {
                project,
                from,
                through,
                returned,
            } = observation
            else {
                panic!("unexpected observation");
            };
            assert_eq!(project, "p");
            assert_eq!(from, committed.len() as i64 + 1);
            for id in from..=through {
                committed.push(
                    returned
                        .checked_duration_since(submitted[id as usize - 1])
                        .unwrap(),
                );
            }
        }
        assert_eq!(committed.len(), count as usize);
        let first = committed[0].as_secs_f64() * 1_000.0;
        let growth = wal.metadata().unwrap().len() - initial_wal;
        let percentile = |values: &mut [Duration], rank: usize| {
            values.sort_unstable();
            values[(values.len() * rank).div_ceil(100) - 1].as_secs_f64() * 1_000.0
        };
        println!(
            "[measured] blocked={blocked} samples={count} transactions={transactions} wal_growth_bytes={growth} first_commit_ms={first:.3} submit_to_commit_ms={:.3}/{:.3} submit_to_body_ms={:.3}/{:.3}",
            percentile(&mut committed, 50),
            percentile(&mut committed, 95),
            percentile(&mut queued, 50),
            percentile(&mut queued, 95),
        );
    }
}
