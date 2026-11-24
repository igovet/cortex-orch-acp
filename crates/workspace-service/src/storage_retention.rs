//! Workspace service owner for storage maintenance. It never starts or resumes
//! execution. Only runtimes already opened by a local project activation qualify.
use crate::{machine::retention, project_path, Service};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{atomic::Ordering, Arc},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{sync::watch, time::Instant};

const INTERVAL: Duration = Duration::from_secs(60 * 60);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, LaunchSpec};
    use serde_json::{json, Value};
    use std::{
        fs,
        future::{poll_fn, Future},
        pin::pin,
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::atomic::AtomicU64,
    };

    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn task_file(config: &Config, project: &str) -> PathBuf {
        config
            .state_dir
            .join("product")
            .join(format!("{project}.json"))
    }

    fn record(path: &Path, done_at: u64) {
        fs::write(
            path,
            json!({
                "tasks":{"closed":{"status":"done","doneAt":done_at,"work":"unmerged"}},
                "messages":[],"comments":{},"sprints":[]
            })
            .to_string(),
        )
        .unwrap();
    }

    fn archived(path: &Path) -> Value {
        serde_json::from_slice::<Value>(&fs::read(path).unwrap()).unwrap()["tasks"]["closed"]
            ["archive"]
            .clone()
    }

    async fn wait_for_invalidation(events: &mut tokio::sync::broadcast::Receiver<Value>, id: &str) {
        loop {
            let event = events.recv().await.unwrap();
            if event["type"] == "invalidated" && event["projectId"] == id {
                return;
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn service_storage_retention_visits_on_activation_and_hourly_until_shutdown() {
        // Keep Tokio from auto-advancing while the service waits for real Git I/O.
        // The blocking task also bounds the test if an expected event never arrives.
        let (done, pending) = std::sync::mpsc::channel::<()>();
        let mut watchdog =
            tokio::task::spawn_blocking(move || pending.recv_timeout(Duration::from_secs(60)));
        let checkpoint = std::cell::Cell::new("setup");
        tokio::select! {
            result = &mut watchdog => panic!("storage retention test did not finish at {}: {result:?}", checkpoint.get()),
            () = exercise_storage_retention(&checkpoint) => {}
        }
        drop(done);
        assert_eq!(
            watchdog.await.unwrap(),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        );
    }

    async fn next_deadline(
        parked: &mut watch::Receiver<Option<Instant>>,
        after: Option<Instant>,
    ) -> Instant {
        parked.wait_for(|deadline| *deadline > after).await.unwrap().unwrap()
    }

    async fn advance_through(due: Instant) {
        // Tokio rounds fractional-millisecond deadlines up to its next timer tick.
        tokio::time::advance(due - Instant::now() + Duration::from_millis(1)).await;
    }

    async fn exercise_storage_retention(checkpoint: &std::cell::Cell<&'static str>) {
        const WEEK: u64 = 7 * 86_400_000;
        const START: u64 = 10 * WEEK;
        const HOUR: u64 = 3_600_000;
        let base =
            std::env::temp_dir().join(format!("cortex-storage-retention-{}", std::process::id()));
        fs::create_dir(&base).unwrap();
        let fixture = Fixture(base);
        fs::set_permissions(&fixture.0, fs::Permissions::from_mode(0o700)).unwrap();
        let projects = fixture.0.join("projects");
        let alpha = projects.join("alpha");
        let beta = projects.join("beta");
        fs::create_dir_all(&alpha).unwrap();
        fs::create_dir_all(&beta).unwrap();
        let config = Config {
            runtime_dir: fixture.0.join("runtime"),
            state_dir: fixture.0.join("state"),
            allowed_root: projects,
            launch: LaunchSpec::bare(),
            request_timeout: Duration::from_secs(5),
            shutdown_timeout: Duration::from_secs(1),
            event_capacity: 64,
        };
        let service = Service::bind_native(
            config.clone(),
            Arc::new(crate::native_runtime::UnqualifiedRoutes),
        )
        .await
        .unwrap();
        let a = service.register(&alpha, "Alpha").await.unwrap()["projectId"]
            .as_str()
            .unwrap()
            .to_owned();
        let b = service.register(&beta, "Beta").await.unwrap()["projectId"]
            .as_str()
            .unwrap()
            .to_owned();
        let first = task_file(&config, &a);
        let second = task_file(&config, &b);
        fs::create_dir_all(first.parent().unwrap()).unwrap();
        let wall = Arc::new(AtomicU64::new(START));
        let clock = wall.clone();
        record(&first, START - WEEK);
        record(&second, START - WEEK);
        // Exercise an hourly deadline between Tokio timer ticks.
        tokio::time::advance(Duration::from_micros(500)).await;
        let mut events = service.subscribe();
        // Drive the production retention loop without launching the unrelated sprint
        // scheduler, whose store reads can legitimately defer an exclusive purge.
        let (stop_retention, stop) = watch::channel(());
        let (parked, mut schedule) = watch::channel(None);
        let retaining = tokio::spawn(service.clone().run_storage_retention(
            stop,
            move || UNIX_EPOCH + Duration::from_millis(clock.load(Ordering::Relaxed)),
            move |due| {
                let parked = parked.clone();
                async move {
                    let mut sleep = pin!(tokio::time::sleep_until(due));
                    let mut announced = false;
                    poll_fn(|cx| {
                        let result = sleep.as_mut().poll(cx);
                        if result.is_pending() && !announced {
                            let _ = parked.send_replace(Some(due));
                            announced = true;
                        }
                        result
                    })
                    .await
                }
            },
        ));
        let activated_at = Instant::now();
        checkpoint.set("alpha activation");
        service.activate(&a).await.unwrap();
        wait_for_invalidation(&mut events, &a).await;
        assert_eq!(archived(&first)["at"], START);
        assert!(archived(&second).is_null());

        checkpoint.set("beta activation");
        service.activate(&b).await.unwrap();
        wait_for_invalidation(&mut events, &b).await;
        assert_eq!(archived(&second)["at"], START);
        record(&second, START - WEEK);
        let store = cortex_core::Store::open_for_workspace_retention(
            crate::native_runtime::key(&beta).unwrap(),
        )
        .unwrap();
        store.write(|db| {
            db.execute("INSERT INTO workspace_tasks(task_id,project_id,description,execution_id,created_at) VALUES ('expired',?,'Finished','execution','now')", [&b])?;
            db.execute("INSERT INTO tasks VALUES ('t_00000000000000000000000000000001',?,'now')", [beta.to_str()])?;
            db.execute("INSERT INTO runtime_tasks(task_id,state,goal,created_at,updated_at) VALUES ('t_00000000000000000000000000000001','COMPLETED','Fixture','now','now')", [])?;
            db.execute("INSERT INTO task_owners VALUES ('t_00000000000000000000000000000001','owner',1,'now')", [])?;
            db.execute("INSERT INTO acp_host_sessions VALUES ('display',?,'t_00000000000000000000000000000001','{\"name\":\"cortex-native\"}','{}','[]','now',NULL)", [beta.to_str()])?;
            db.execute("INSERT INTO workspace_executions VALUES ('execution','expired','t_00000000000000000000000000000001','display',1,1,'now')", [])?;
            Ok(())
        }).unwrap();
        let mut product: Value = serde_json::from_slice(&fs::read(&second).unwrap()).unwrap();
        product["tasks"]["expired"] = json!({
            "status":"done","doneAt":1,"work":"merged",
            "archive":{"at":WEEK,"previousStatus":"done"}
        });
        fs::write(&second, product.to_string()).unwrap();
        let saved_settings = config.state_dir.join("machine.json");
        fs::write(&saved_settings, br#"{"automaticArchivePurge":false}"#).unwrap();
        // An unreadable first project does not stop the other project's hourly visit.
        fs::write(&first, b"{broken").unwrap();
        let first_due = next_deadline(&mut schedule, None).await;
        assert_eq!(first_due, activated_at + INTERVAL);
        for _ in 0..3 {
            schedule.borrow_and_update();
            service.retention_wake.notify_one();
            schedule.changed().await.unwrap();
            assert_eq!(*schedule.borrow_and_update(), Some(first_due));
        }
        tokio::time::advance(first_due - Instant::now() - Duration::from_secs(60)).await;
        assert!(
            archived(&second).is_null(),
            "duplicate wakes do not create another pass"
        );
        checkpoint.set("first hourly visit");
        wall.store(START + HOUR, Ordering::Relaxed);
        advance_through(first_due).await;
        let second_due = next_deadline(&mut schedule, Some(first_due)).await;
        assert_eq!(second_due, Instant::now() + INTERVAL);
        assert_eq!(archived(&second)["at"], START + HOUR);
        assert!(
            serde_json::from_slice::<Value>(&fs::read(&second).unwrap()).unwrap()["tasks"]
                ["expired"]["purged"]
                .is_null(),
            "machine setting disables background purge"
        );
        wait_for_invalidation(&mut events, &b).await;
        fs::write(&saved_settings, b"{}").unwrap();
        checkpoint.set("second hourly visit");
        wall.store(START + 2 * HOUR, Ordering::Relaxed);
        advance_through(second_due).await;
        let third_due = next_deadline(&mut schedule, Some(second_due)).await;
        assert_eq!(third_due, Instant::now() + INTERVAL);
        let seen: Value = serde_json::from_slice(&fs::read(&second).unwrap()).unwrap();
        assert_eq!(
            seen["tasks"]["expired"]["purged"], true,
            "eligible archive was not purged"
        );
        assert_eq!(
            store
                .read(|db| Ok(db.query_row(
                    "SELECT count(*) FROM workspace_tasks WHERE task_id='expired'",
                    [],
                    |r| r.get::<_, i64>(0)
                )?))
                .unwrap(),
            0
        );

        wait_for_invalidation(&mut events, &b).await;
        checkpoint.set("shutdown");
        service.stop().await.unwrap();
        // serve_with_retention_clock closes this channel and joins the loop on shutdown.
        drop(stop_retention);
        retaining.await.unwrap();
        record(&second, WEEK);
        tokio::time::advance(INTERVAL).await;
        assert!(archived(&second).is_null());
    }
}

struct Visit {
    due: Instant,
    cursor: Option<String>,
}

impl Service {
    pub(super) async fn storage_retention(
        self: Arc<Self>,
        stop: watch::Receiver<()>,
        wall_clock: impl Fn() -> SystemTime,
    ) {
        self.run_storage_retention(stop, wall_clock, tokio::time::sleep_until)
            .await;
    }

    async fn run_storage_retention<F: std::future::Future<Output = ()>>(
        self: Arc<Self>,
        mut stop: watch::Receiver<()>,
        wall_clock: impl Fn() -> SystemTime,
        sleep_until: impl Fn(Instant) -> F,
    ) {
        let mut visits = BTreeMap::<String, Visit>::new();
        loop {
            if self.stopping.load(Ordering::Acquire) || stop.has_changed().is_err() {
                return;
            }
            let active = {
                let state = self.state.lock().await;
                state
                    .projects
                    .iter()
                    .filter(|project| state.native.contains_key(&project.project_id))
                    .map(|project| (project.project_id.clone(), project.canonical_root.clone()))
                    .collect::<Vec<_>>()
            };
            let ids: BTreeSet<_> = active.iter().map(|(id, _)| id).collect();
            visits.retain(|id, _| ids.contains(id));
            let now = Instant::now();
            let at = wall_clock();
            let at_ms = at
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|elapsed| u64::try_from(elapsed.as_millis()).ok());
            let mut visited = false;
            for (id, root) in active {
                if self.stopping.load(Ordering::Acquire) || stop.has_changed().is_err() {
                    return;
                }
                let visit = visits.entry(id.clone()).or_insert(Visit {
                    due: now,
                    cursor: None,
                });
                if visit.due > now {
                    continue;
                }
                // Missed ticks coalesce. No new work is spawned per timer tick.
                visit.due = now + INTERVAL;
                visited = true;
                let Some(at_ms) = at_ms else { continue };
                if !project_path(&self.config.allowed_root, &root)
                    .is_ok_and(|canonical| canonical == root)
                {
                    continue;
                }
                let stopping =
                    || self.stopping.load(Ordering::Acquire) || stop.has_changed().is_err();
                if matches!(
                    retention::scheduled_project(
                        &self.config,
                        &root,
                        &id,
                        at_ms,
                        &mut visit.cursor,
                        stopping
                    )
                    .await,
                    Ok(true)
                ) {
                    let _ = self
                        .events
                        .send(serde_json::json!({"type":"invalidated","projectId":id}));
                }
            }
            if visited && !self.stopping.load(Ordering::Acquire) && stop.has_changed().is_ok() {
                retention::scheduled_attachments(&self.config.state_dir, at);
            }
            let deadline = visits.values().map(|v| v.due).min();
            tokio::select! {
                _ = stop.changed() => return,
                _ = self.retention_wake.notified() => {},
                _ = async {
                    match deadline {
                        Some(due) => sleep_until(due).await,
                        None => std::future::pending().await,
                    }
                } => {},
            }
        }
    }
}
