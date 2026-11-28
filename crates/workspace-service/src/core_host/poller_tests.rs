use super::*;
use std::{
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc as thread_channel,
    },
};
use tokio::{
    sync::mpsc,
    time::{advance, timeout},
};
use workspace_protocol::core::{CoreQuery, Page};

const DEADLINE: Duration = Duration::from_secs(5);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "workspace-poller-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&root).unwrap();
        project_guard::reserve(&root).unwrap();
        Self(root)
    }

    fn start_reader(&self, project: &str, events: broadcast::Sender<Value>) -> (Store, Poller) {
        let key = key(&self.0).unwrap();
        let store = Store::open(key.clone(), OpenMode::Initialize).unwrap();
        store
            .write(|db| {
                db.execute(
                    "INSERT INTO workspace_projects VALUES (?, ?, ?, 'now', 1)",
                    (project, key.root().to_str().unwrap(), actor()),
                )?;
                db.execute(
                    "INSERT INTO workspace_tasks(task_id, project_id, description, created_at)
                     VALUES ('task', ?, 'initial', 'now')",
                    [project],
                )?;
                Ok(())
            })
            .unwrap();
        let reader = ProjectionReader::new(
            Store::open(key.clone(), OpenMode::LiveReadOnly).unwrap(),
            project.to_owned(),
            actor(),
        )
        .unwrap();
        let snapshot = reader
            .query(CoreQuery::TasksPage(Page {
                target: project.to_owned(),
                cursor: None,
                limit: 1,
            }))
            .unwrap();
        let subscription = reader
            .subscribe(&snapshot.generation, snapshot.high_watermark)
            .unwrap();
        let entry = Arc::new(AsyncMutex::new(Some(Entry {
            project: project.to_owned(),
            reader,
            subscription,
        })));
        (
            store,
            Poller {
                root: self.0.clone(),
                signal: key.commit_signal(),
                key,
                project: project.to_owned(),
                entry,
                events,
                wal_directory: None,
            },
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn commit(store: &Store, project: &str, version: i64) {
    store
        .write(|db| {
            db.execute("UPDATE workspace_tasks SET version = ? WHERE task_id = 'task'", [version])?;
            cortex_core::store::append_public_tx(db, project, "task", "task", version)?;
            Ok(())
        })
        .unwrap();
}

// Keep virtual time still while giving the poll task and blocking worker CPU time.
// Use a real deadline for missing evidence so a regression cannot hang a paused test.
async fn at_this_tick<T>(mut take: impl FnMut() -> Option<T>) -> T {
    let until = std::time::Instant::now() + DEADLINE;
    loop {
        if let Some(value) = take() {
            return value;
        }
        assert!(std::time::Instant::now() < until, "missing poll or event");
        tokio::task::yield_now().await;
    }
}

fn delta(event: &Value, project: &str, from: &str, through: &str, version: &str) {
    assert_eq!(event["type"], "native_delta", "{event}");
    assert_eq!(event["projectId"], project);
    assert_eq!(event["batch"]["fromSequence"], from);
    assert_eq!(event["batch"]["throughSequence"], through);
    let changes = event["batch"]["changes"].as_array().unwrap();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0]["newVersion"], version);
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn a_directory_watch_survives_wal_replacement_and_reports_its_own_loss() {
    let fixture = Fixture::new();
    let directory = fixture.0.join("store");
    fs::create_dir(&directory).unwrap();
    let watch = wal_watch::Watch::new(&directory).unwrap();
    let wal = directory.join("cortex.sqlite3-wal");
    fs::write(&wal, b"first WAL").unwrap();
    timeout(DEADLINE, wal_watch::wait(Some(&watch)))
        .await
        .unwrap()
        .unwrap();

    let database = directory.join("cortex.sqlite3");
    fs::write(&database, b"original database").unwrap();
    timeout(DEADLINE, wal_watch::wait(Some(&watch)))
        .await
        .unwrap()
        .unwrap();
    fs::remove_file(&wal).unwrap();
    fs::write(&wal, b"replacement WAL").unwrap();
    timeout(DEADLINE, wal_watch::wait(Some(&watch)))
        .await
        .unwrap()
        .unwrap();
    fs::rename(&database, directory.join("old-database")).unwrap();
    fs::write(&database, b"replacement database").unwrap();
    timeout(DEADLINE, wal_watch::wait(Some(&watch)))
        .await
        .unwrap()
        .unwrap();

    fs::rename(&directory, fixture.0.join("retired-store")).unwrap();
    assert_eq!(
        timeout(DEADLINE, wal_watch::wait(Some(&watch)))
            .await
            .unwrap()
            .unwrap_err()
            .kind(),
        io::ErrorKind::NotFound
    );
    assert!(wal_watch::Watch::new(&directory).is_err());
    fs::create_dir(&directory).unwrap();
    let replacement = wal_watch::Watch::new(&directory).unwrap();
    fs::write(&wal, b"new directory").unwrap();
    timeout(DEADLINE, wal_watch::wait(Some(&replacement)))
        .await
        .unwrap()
        .unwrap();
}

// SQLite's own connection writes the portable outbox without a Core wake.
fn raw_update(database: &std::path::Path, project: &str, checkpoints: usize) {
    let mut db = rusqlite::Connection::open(database).unwrap();
    db.busy_timeout(DEADLINE).unwrap();
    if checkpoints > 0 {
        db.execute_batch("CREATE TABLE checkpoint_probe(id INTEGER)")
            .unwrap();
    }
    for _ in 0..checkpoints {
        db.execute("INSERT INTO checkpoint_probe VALUES (1)", [])
            .unwrap();
        db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    }
    let tx = db.transaction().unwrap();
    tx.execute(
        "UPDATE workspace_tasks SET version=2 WHERE task_id='task'",
        [],
    )
    .unwrap();
    cortex_core::store::append_public_tx(&tx, project, "task", "task", 2).unwrap();
    tx.commit().unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn fallback_delivers_writes_after_the_watch_is_lost() {
    let fixture = Fixture::new();
    let (events, mut received) = broadcast::channel(16);
    let (_store, mut reader) = fixture.start_reader("fallback", events);
    let database = reader.key.database();
    let directory = fixture.0.join("observed");
    fs::create_dir(&directory).unwrap();
    reader.wal_directory = Some(directory.clone());
    let (calls, mut reads) = mpsc::unbounded_channel();
    let running = reader.start_with(move |root, key, project, entry| {
        let result = poll(root, key, project, entry);
        calls.send(()).unwrap();
        result
    });
    timeout(DEADLINE, reads.recv()).await.unwrap().unwrap();
    fs::remove_dir(&directory).unwrap();
    timeout(DEADLINE, reads.recv()).await.unwrap().unwrap();
    tokio::task::spawn_blocking(move || raw_update(&database, "fallback", 0))
        .await
        .unwrap();
    let event = timeout(DEADLINE, received.recv()).await.unwrap().unwrap();
    delta(&event, "fallback", "1", "1", "2");
    running.stop().await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test(start_paused = true)]
async fn a_wal_hint_before_commit_rechecks_once_at_cooldown() {
    check_early_wal_hint(false).await;
}

#[cfg(target_os = "linux")]
#[tokio::test(start_paused = true)]
async fn a_slow_commit_is_visible_at_the_final_wal_recheck() {
    check_early_wal_hint(true).await;
}

#[cfg(target_os = "linux")]
async fn check_early_wal_hint(slow_commit: bool) {
    let fixture = Fixture::new();
    let (events, mut received) = broadcast::channel(16);
    let (_store, mut reader) = fixture.start_reader("early", events);
    let database = reader.key.database();
    let observed = fixture.0.join("observed");
    fs::create_dir(&observed).unwrap();
    reader.wal_directory = Some(observed.clone());
    let (calls, mut reads) = mpsc::unbounded_channel();
    let running = reader.start_with(move |root, key, project, entry| {
        let result = poll(root, key, project, entry);
        calls.send(()).unwrap();
        result
    });
    at_this_tick(|| reads.try_recv().ok()).await;

    // A hint from the watched directory arrives without a visible commit.
    fs::write(observed.join("cortex.sqlite3-wal"), b"early wake").unwrap();
    advance(COALESCE).await;
    at_this_tick(|| reads.try_recv().ok()).await;
    assert!(received.try_recv().is_err());

    if slow_commit {
        advance(COALESCE).await;
        at_this_tick(|| reads.try_recv().ok()).await;
        assert!(received.try_recv().is_err());
    }

    // The other connection emits no local signal and its WAL is outside
    // the test watch. A bounded recheck observes the durable write.
    raw_update(&database, "early", 0);
    advance(if slow_commit { WAL_FINAL_RECHECK } else { COALESCE }).await;
    let event = at_this_tick(|| received.try_recv().ok()).await;
    delta(&event, "early", "1", "1", "2");
    at_this_tick(|| reads.try_recv().ok()).await;
    advance(COALESCE * 2).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(reads.try_recv().is_err());
    running.stop().await.unwrap();
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn checkpoint_bursts_preserve_one_committed_delta() {
    let fixture = Fixture::new();
    let (events, mut received) = broadcast::channel(16);
    let (_store, mut reader) = fixture.start_reader("checkpoint", events);
    let database = reader.key.database();
    reader.wal_directory = Some(reader.key.directory());
    let (calls, mut reads) = mpsc::unbounded_channel();
    let running = reader.start_with(move |root, key, project, entry| {
        calls.send(()).unwrap();
        poll(root, key, project, entry)
    });
    timeout(DEADLINE, reads.recv()).await.unwrap().unwrap();
    tokio::task::spawn_blocking(move || raw_update(&database, "checkpoint", 80))
        .await
        .unwrap();
    let event = timeout(DEADLINE, received.recv()).await.unwrap().unwrap();
    delta(&event, "checkpoint", "1", "1", "2");
    running.stop().await.unwrap();
    assert!(reads.try_recv().is_ok());
    assert!(received.try_recv().is_err());
}

#[tokio::test(start_paused = true)]
async fn a_commit_during_a_blocked_poll_keeps_one_trailing_read_and_does_not_block_other_projects() {
    let slow = Fixture::new();
    let fast = Fixture::new();
    let (events, mut received) = broadcast::channel(16);
    let (slow_store, slow) = slow.start_reader("slow", events.clone());
    let (fast_store, fast) = fast.start_reader("fast", events);
    let (slow_calls, mut slow_reads) = mpsc::unbounded_channel();
    let (fast_calls, mut fast_reads) = mpsc::unbounded_channel();
    let (release, gate) = thread_channel::channel();
    let gate = Arc::new(Mutex::new(Some(gate)));

    commit(&slow_store, "slow", 2);
    let slow = slow.start_with(move |root, key, project, entry| {
        let result = poll(root, key, project, entry);
        slow_calls.send(()).unwrap();
        if let Some(gate) = gate.lock().unwrap().take() {
            gate.recv_timeout(DEADLINE).unwrap();
        }
        result
    });
    let fast = fast.start_with(move |root, key, project, entry| {
        let result = poll(root, key, project, entry);
        fast_calls.send(()).unwrap();
        result
    });
    at_this_tick(|| slow_reads.try_recv().ok()).await;
    at_this_tick(|| fast_reads.try_recv().ok()).await;
    for version in 3..=8 {
        commit(&slow_store, "slow", version);
    }
    commit(&fast_store, "fast", 2);
    assert!(received.try_recv().is_err());

    // Well before the 250 ms fallback, fast can finish while slow is still blocked.
    advance(COALESCE).await;
    let first = at_this_tick(|| received.try_recv().ok()).await;
    delta(&first, "fast", "1", "1", "2");

    release.send(()).unwrap();
    let leading = at_this_tick(|| received.try_recv().ok()).await;
    let trailing = at_this_tick(|| received.try_recv().ok()).await;
    delta(&leading, "slow", "1", "1", "2");
    delta(&trailing, "slow", "2", "7", "8");
    assert_eq!(leading["batch"]["generation"], trailing["batch"]["generation"]);
    slow_reads.try_recv().unwrap(); // one trailing poll
    fast_reads.try_recv().unwrap(); // one signaled poll

    // Progress the clock past cooldown but not fallback: no duplicate, resync, or idle scan.
    advance(COALESCE * 2).await;
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
    assert!(slow_reads.try_recv().is_err());
    assert!(fast_reads.try_recv().is_err());
    assert!(received.try_recv().is_err());

    // A leading write after idle needs no timer advancement.
    commit(&fast_store, "fast", 3);
    let next = at_this_tick(|| received.try_recv().ok()).await;
    delta(&next, "fast", "2", "2", "3");
    slow.stop().await.unwrap();
    fast.stop().await.unwrap();
}

#[tokio::test]
async fn a_blocking_read_cannot_keep_a_removed_poller_alive_or_send_its_drained_frames() {
    let fixture = Fixture::new();
    let (events, mut received) = broadcast::channel(16);
    let (store, reader) = fixture.start_reader("blocked", events.clone());
    let registry = Registry::default();
    let registration = registry.registration("blocked").unwrap();
    let key = reader.key.clone();
    let signal = Arc::downgrade(&reader.signal);
    let slot = reader.entry.clone();
    let old_slot = Arc::downgrade(&slot);
    let (entered, in_read) = tokio::sync::oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered)));
    let (release, gate) = thread_channel::channel();
    let gate = Arc::new(Mutex::new(gate));

    commit(&store, "blocked", 2);
    let running = reader.start_with(move |root, key, project, entry| {
        let result = poll(root, key, project, entry);
        assert_eq!(result.as_ref().unwrap().len(), 1);
        entered.lock().unwrap().take().unwrap().send(()).unwrap();
        gate.lock().unwrap().recv_timeout(DEADLINE).unwrap();
        result
    });
    registry.readers.lock().unwrap().slots.insert(
        key.clone(),
        Slot {
            entry: Arc::downgrade(&slot),
            running: Some(running),
        },
    );
    drop(slot);
    timeout(DEADLINE, in_read).await.unwrap().unwrap();
    for stopped in registry.remove("blocked") {
        timeout(DEADLINE / 2, stopped).await.unwrap().unwrap();
    }
    assert!(signal.upgrade().is_none(), "cancelled poll retains wake");
    assert!(registry
        .readers
        .lock()
        .unwrap()
        .check("blocked", &registration)
        .is_err());
    assert!(old_slot.upgrade().is_some(), "read should still be blocked");
    assert!(received.try_recv().is_err());

    // A new registration can use a fresh slot without waiting on the blocked one.
    let new_registration = registry.registration("blocked").unwrap();
    assert!(!Arc::ptr_eq(&registration, &new_registration));
    assert!(registry.readers.lock().unwrap().slots.is_empty());
    let new_entry = Arc::new(AsyncMutex::new(None));
    let new_slot = Arc::downgrade(&new_entry);
    let fresh = Poller {
        root: fixture.0.clone(),
        signal: key.commit_signal(),
        key: key.clone(),
        project: "blocked".into(),
        entry: new_entry.clone(),
        events,
        wal_directory: None,
    };
    let new_signal = Arc::downgrade(&fresh.signal);
    let (entered, in_read) = tokio::sync::oneshot::channel();
    let entered = Arc::new(Mutex::new(Some(entered)));
    let (release_new, gate) = thread_channel::channel();
    let gate = Arc::new(Mutex::new(gate));
    let running = fresh.start_with(move |_, _, _, _| {
        entered.lock().unwrap().take().unwrap().send(()).unwrap();
        gate.lock().unwrap().recv_timeout(DEADLINE).unwrap();
        Ok(Vec::new())
    });
    registry.readers.lock().unwrap().slots.insert(
        key,
        Slot {
            entry: Arc::downgrade(&new_entry),
            running: Some(running),
        },
    );
    drop(new_entry);
    timeout(DEADLINE, in_read).await.unwrap().unwrap();
    timeout(DEADLINE / 2, registry.shutdown()).await.unwrap();
    assert!(new_signal.upgrade().is_none());
    assert!(registry.registration("blocked").is_err());

    release.send(()).unwrap();
    release_new.send(()).unwrap();
    at_this_tick(|| {
        (old_slot.upgrade().is_none() && new_slot.upgrade().is_none()).then_some(())
    })
    .await;
    assert!(received.try_recv().is_err());
}
