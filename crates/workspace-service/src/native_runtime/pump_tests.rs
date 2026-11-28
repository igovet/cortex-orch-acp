use super::*;
use std::time::Duration;

async fn poll_pending(mut wait: std::pin::Pin<&mut impl std::future::Future>) {
    std::future::poll_fn(|cx| {
        assert!(
            wait.as_mut().poll(cx).is_pending(),
            "the held step did not wait"
        );
        std::task::Poll::Ready(())
    })
    .await;
}

#[tokio::test]
async fn driver_reconciliation_leaves_a_queued_control_lock_on_shutdown() {
    let root = std::env::temp_dir().join(format!("stop-controls-{}", crate::identity().unwrap()));
    std::fs::create_dir(&root).unwrap();
    let path = root.clone();
    let mut runtime = tokio::task::spawn_blocking(move || {
        Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
    })
    .await
    .unwrap()
    .unwrap();
    Arc::get_mut(&mut runtime.host).unwrap().workspace_state = Some(root.clone());
    {
        // A job may be waiting for a prior control when shutdown takes over.
        let _controls = runtime.controls.lock().await;
        let mut reconcile = Box::pin(runtime.reconcile_after_merge_driver("t"));
        poll_pending(reconcile.as_mut()).await;
        runtime.stop.send_replace(true);
        tokio::time::timeout(Duration::from_millis(250), reconcile).await
            .expect("reconciliation still needs the control lock after stop");
    }
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}

// A turn whose state read fails still signals the batch (siblings must not keep running).
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_state_read_after_a_turn_still_signals_the_batch() {
    let root = std::env::temp_dir().join(format!("batch-{}", crate::identity().unwrap()));
    std::fs::create_dir(&root).unwrap();
    let path = root.clone();
    let runtime = Arc::new(
        tokio::task::spawn_blocking(move || {
            Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
        })
        .await
        .unwrap()
        .unwrap(),
    );
    let (batch, sibling) = watch::channel(false);
    let stale = std::sync::atomic::AtomicBool::new(false);
    let outcome = runtime
        .settle_batch_turn("no-such-task", Ok(()), &batch, &stale)
        .await;
    assert!(outcome.is_err());
    assert!(
        *sibling.borrow(),
        "siblings must be signalled on every exit path"
    );
    // A failed turn signals the batch too.
    let (batch, sibling) = watch::channel(false);
    let _ = runtime
        .settle_batch_turn("no-such-task", Err(CoreError::Refused("x")), &batch, &stale)
        .await;
    assert!(*sibling.borrow());
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}
// Installing a pump for a task that already has one cancels it and waits for it to
// finish; the new driver is never visible while the old one may still be running.
#[tokio::test(flavor = "multi_thread")]
async fn installing_a_pump_cancels_and_awaits_the_prior_driver() {
    let root = std::env::temp_dir().join(format!("pump-{}", crate::identity().unwrap()));
    std::fs::create_dir(&root).unwrap();
    let path = root.clone();
    let runtime = Arc::new(
        tokio::task::spawn_blocking(move || {
            Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
        })
        .await
        .unwrap()
        .unwrap(),
    );
    let (cancel1, mut cancelled1) = watch::channel(false);
    let (done1, finished1) = watch::channel(false);
    runtime.install_pump("t".into(), cancel1, finished1).await;
    let (cancel2, _cancelled2) = watch::channel(false);
    let (_done2, finished2) = watch::channel(false);
    let r = runtime.clone();
    let second =
        tokio::spawn(async move { r.install_pump("t".into(), cancel2, finished2).await });
    cancelled1.changed().await.unwrap();
    assert!(*cancelled1.borrow());
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !second.is_finished(),
        "replacement must wait for the prior driver"
    );
    done1.send_replace(true);
    second.await.unwrap();
    assert_eq!(runtime.pumps.lock().await.len(), 1);
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}
// A driver job that ended without signalling (its sender dropped, e.g. a panic)
// no longer runs: settling the task must not fail every later owner control.
#[tokio::test(flavor = "multi_thread")]
async fn a_driver_gone_without_signalling_is_settled() {
    let root = std::env::temp_dir().join(format!("pump-gone-{}", crate::identity().unwrap()));
    std::fs::create_dir(&root).unwrap();
    let path = root.clone();
    let runtime = Arc::new(
        tokio::task::spawn_blocking(move || {
            Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
        })
        .await
        .unwrap()
        .unwrap(),
    );
    let (cancel, mut cancelled) = watch::channel(false);
    let (done, finished) = watch::channel(false);
    runtime.install_pump("t".into(), cancel, finished).await;
    let job = tokio::spawn(async move {
        let _ = cancelled.changed().await;
        drop(done);
    });
    runtime.settle_task("t").await.unwrap();
    job.await.unwrap();
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}
// A ready step whose file another unfinished task's worker holds waits for it (one
// recorded wait), instead of stopping the task as `native_continuation_unavailable`;
// it claims again once the holder's task is cancelled.
#[tokio::test]
async fn a_step_held_by_another_tasks_writer_waits_until_the_holding_ends() {
    let root = std::env::temp_dir().join(format!("held-{}", crate::identity().unwrap()));
    std::fs::create_dir(&root).unwrap();
    let path = root.clone();
    let runtime = Arc::new(
        tokio::task::spawn_blocking(move || {
            Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
        })
        .await
        .unwrap()
        .unwrap(),
    );
    let owners = runtime.host.executor.publication(|store| {
        let machine = cortex_core::lifecycle::TaskMachine::new(store);
        let waiting = machine.create("waiting", "Reset test", "owner")?;
        machine.create("holder", "formatAll", "owner")?;
        store.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='EXECUTING',revision=1", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('holder',1,'formatAll','worker-backend-dev','o','[]','[\"test/counter.test.js\"]','execution','running','ha',NULL)", [])?;
            db.execute("INSERT INTO delegation_provisioning VALUES ('ha','holder',NULL,'hx','worker-backend-dev','ready','ambiguous',NULL,'hc',NULL,NULL,NULL,NULL,NULL,1,'0','0')", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('waiting',1,'reset','worker-backend-dev','o','[]','[\"test\"]','execution','pending',NULL,NULL)", [])?;
            Ok(())
        })?;
        Ok(waiting)
    }).unwrap().await.unwrap().unwrap();
    let (_cancel, cancelled) = watch::channel(false);
    let mut wait = Box::pin(runtime.wait_for_write_holding("waiting", &owners, &cancelled));
    poll_pending(wait.as_mut()).await;
    let count = |runtime: Arc<Runtime>| async move {
        runtime.host.executor.read(|db| Ok(db.query_row(
            "SELECT count(*)||':'||coalesce(max(json_extract(payload_json,'$.holder_task')),'') FROM runtime_events WHERE type='NATIVE_WRITE_CONFLICT_WAITING' AND task_id='waiting'",
            [], |r| r.get::<_, String>(0))?)).unwrap().await.unwrap().unwrap()
    };
    // Observe the real SQLite event rather than guessing how long the executor takes.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while count(runtime.clone()).await != "1:holder" {
        assert!(std::time::Instant::now() < deadline, "holder event not recorded");
    }
    runtime
        .host
        .executor
        .write(|db| {
            db.execute(
                "UPDATE runtime_tasks SET state='CANCELLED' WHERE task_id='holder'",
                [],
            )?;
            Ok(())
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    // The first poll wrote the event; poll its completed result now to register
    // the real retry timer, then advance it without sleeping on the wall clock.
    tokio::time::pause();
    poll_pending(wait.as_mut()).await;
    tokio::time::advance(Duration::from_millis(1001)).await;
    tokio::time::resume();
    assert!(tokio::time::timeout(Duration::from_secs(5), wait)
        .await
        .unwrap()
        .unwrap());
    assert_eq!(count(runtime.clone()).await, "1:holder");
    // Nothing held: no wait at all.
    let (_cancel, cancelled) = watch::channel(false);
    let owner = runtime
        .host
        .executor
        .read(|db| {
            Ok(db.query_row(
                "SELECT owner_ref,epoch FROM task_owners WHERE task_id='waiting'",
                [],
                |r| {
                    Ok(OwnerEpoch {
                        owner: r.get(0)?,
                        epoch: r.get(1)?,
                    })
                },
            )?)
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert!(!runtime
        .wait_for_write_holding("waiting", &owner, &cancelled)
        .await
        .unwrap());
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn owner_file_lease_wait_is_visible_and_rechecks_release_and_expiry() {
    use cortex_core::store::owner_file_leases as leases;
    let root = std::env::temp_dir().join(format!("lease-wait-{}", crate::identity().unwrap()));
    std::fs::create_dir(&root).unwrap();
    let path = root.clone();
    let runtime = Arc::new(tokio::task::spawn_blocking(move || {
        Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
    }).await.unwrap().unwrap());
    for (task, expire) in [("release", false), ("expire", true)] {
        let (owner, epoch, checkout, file, token) = runtime.host.executor.publication(move |store| {
            let checkout = leases::Checkout::identify(store.key().root())?;
            let file = leases::RelativePath::new("file.txt")?;
            let epoch = leases::EditorEpoch::new();
            let editor = leases::Editor { epoch: &epoch, session: "fixture-owner" };
            let owner = cortex_core::lifecycle::TaskMachine::new(store).create(task, "Lease wait", "owner")?;
            let token = store.write(|db| {
                leases::activate_editor_tx(db, &epoch)?;
                let token = leases::acquire_tx(db, editor, &checkout, &file)?;
                db.execute("UPDATE runtime_tasks SET state='EXECUTING', revision=1 WHERE task_id=?", [task])?;
                db.execute("INSERT INTO runtime_nodes VALUES (?,1,'write','worker-backend-dev','Write','[]','[\"file.txt\"]','execution','pending',NULL,NULL)", [task])?;
                assert!(cortex_core::scheduler::claim_ready_tx(db, task, &owner, "fixture",
                    &cortex_core::scheduler::policy::NativePolicy::default())?.is_empty());
                Ok(token)
            })?;
            Ok((owner, epoch, checkout, file, token))
        }).unwrap().await.unwrap().unwrap();
        let (_cancel, cancelled) = watch::channel(false);
        let mut wait = Box::pin(runtime.wait_for_write_holding(task, &owner, &cancelled));
        poll_pending(wait.as_mut()).await;
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let observed = runtime.host.executor.read(move |db| Ok(db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM runtime_events WHERE task_id=? AND type='NATIVE_WRITE_CONFLICT_WAITING'
                     AND json_extract(payload_json,'$.reason')='owner_file_lease_held'
                     AND json_extract(payload_json,'$.paths[0]')='file.txt')", [task], |r| r.get::<_, bool>(0))?
                )).unwrap().await.unwrap().unwrap();
                if observed { break; }
            }
        }).await.expect("owner lease wait was not visible");
        runtime.host.executor.write(move |db| {
            if expire {
                db.execute("UPDATE code_owner_file_leases SET expires_at=0", [])?;
            } else {
                let editor = leases::Editor { epoch: &epoch, session: "fixture-owner" };
                leases::release_tx(db, editor, &checkout, &file, token.expose())?;
            }
            Ok(())
        }).unwrap().await.unwrap().unwrap();
        // The SQL write above completed after the initial conflict write. Register
        // the waiter before sending a release wake or advancing the expiry poll.
        if expire {
            tokio::time::pause();
        }
        poll_pending(wait.as_mut()).await;
        if expire {
            tokio::time::advance(Duration::from_millis(1001)).await;
            tokio::time::resume();
        } else {
            runtime.wake.notify_waiters();
        }
        assert!(tokio::time::timeout(Duration::from_secs(5), wait)
            .await
            .unwrap()
            .unwrap());
        runtime.host.executor.write(move |db| {
            let claimed = cortex_core::scheduler::claim_ready_tx(db, task, &owner, "fixture",
                &cortex_core::scheduler::policy::NativePolicy::default())?;
            assert_eq!(claimed.len(), 1);
            db.execute("UPDATE runtime_tasks SET state='CANCELLED' WHERE task_id=?", [task])?;
            db.execute("UPDATE runtime_nodes SET status='completed' WHERE task_id=?", [task])?;
            Ok(())
        }).unwrap().await.unwrap().unwrap();
    }
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}
// A blocking review while the driver advances is a continuation on the strategy
// ladder (no-stop policy): the runtime asks the owner nothing, the driver ends
// without recording the step as one that could not continue, and the continuation
// cites the review report. A later driver error never records a second stop.
#[tokio::test(flavor = "multi_thread")]
async fn a_blocking_review_is_a_continuation_and_the_driver_ends() {
    let root = std::env::temp_dir().join(format!("blocked-{}", crate::identity().unwrap()));
    std::fs::create_dir(&root).unwrap();
    // The review is bound to an observed project revision.
    assert!(std::process::Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .current_dir(&root)
        .status()
        .unwrap()
        .success());
    let path = root.clone();
    let runtime = Arc::new(
        tokio::task::spawn_blocking(move || {
            Runtime::open(&path, "project", Arc::new(UnqualifiedRoutes))
        })
        .await
        .unwrap()
        .unwrap(),
    );
    let owner = runtime.host.executor.publication(|store| {
        let owner = cortex_core::lifecycle::TaskMachine::new(store).create("t", "Serve HTTPS", "owner")?;
        store.write(|db| {
            db.execute("UPDATE runtime_tasks SET state='REVIEWING',revision=4,review_cycle=1", [])?;
            db.execute("INSERT INTO runtime_nodes VALUES ('t',4,'trace','worker-security-auditor','Trace the proxy path','[]','[\"traefik/**\"]','execution','completed','a1','{}')", [])?;
            let review = json!({"verdict":"BLOCK","summary":"The request chain was not verified.","report_ref":"r_2413b9f526b9","checks":[],
                "findings":[{"node":"trace","files":["traefik/tls.yml"],"cross_cutting":false,"fix":"Forward the visitor address"}]});
            db.execute("INSERT INTO runtime_nodes VALUES ('t',4,'_review_1','worker-code-reviewer','Review','[\"trace\"]','[\"**\"]','review','completed','rv',?)",
                [review.to_string()])?;
            Ok(())
        })?;
        Ok(owner)
    }).unwrap().await.unwrap().unwrap();
    let (_cancel, cancelled) = watch::channel(false);
    tokio::time::timeout(
        Duration::from_secs(20),
        runtime.progress("t", &owner, cancelled),
    )
    .await
    .expect("the driver kept running after the continuation")
    .unwrap();
    let facts = runtime
        .host
        .executor
        .read(|db| {
            let stopped: i64 = db.query_row(
                "SELECT count(*) FROM runtime_events WHERE type='NATIVE_DRIVER_STOPPED'",
                [],
                |r| r.get(0),
            )?;
            let questions: i64 = db.query_row(
                "SELECT count(*) FROM pending_questions WHERE task_id='t'",
                [],
                |r| r.get(0),
            )?;
            let (state, blocker): (String, String) = db.query_row(
                "SELECT state,blocker FROM runtime_tasks WHERE task_id='t'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let owed = cortex_core::lifecycle::automatic::owed(db, "t")?.map(|o| o.evidence);
            Ok((stopped, questions, state, blocker, owed))
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        facts.0, 0,
        "the driver recorded a stop after the continuation"
    );
    assert_eq!(facts.1, 0, "the owner was asked");
    assert_eq!(
        (facts.2.as_str(), facts.3.as_str()),
        ("RECOVERING", "automatic_continuation_pending")
    );
    let evidence = facts.4.expect("a continuation is owed");
    assert_eq!(evidence["reason"], "review_blocked");
    assert_eq!(evidence["strategy"], "repair");
    assert!(evidence["reports"]
        .as_array()
        .unwrap()
        .iter()
        .any(|r| r["reportId"] == "r_2413b9f526b9"));
    // A driver error arriving now records no second continuation.
    runtime
        .stop_after_error(
            "t",
            &owner,
            CoreError::Refused("native_continuation_unavailable"),
            watch::channel(false).1,
        )
        .await;
    let count = runtime
        .host
        .executor
        .read(|db| {
            Ok(db.query_row(
                "SELECT count(*) FROM runtime_events WHERE type='AUTOMATIC_CONTINUATION'",
                [],
                |r| r.get::<_, i64>(0),
            )?)
        })
        .unwrap()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(count, 1);
    runtime.shutdown().await;
    drop(runtime);
    std::fs::remove_dir_all(root).unwrap();
}
