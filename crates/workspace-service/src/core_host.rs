//! Reusable Core projections served to owner reads, one reader per logical project/channel.
use crate::{actor, failure, local::core_failure as err, project_guard, Service};
use cortex_core::{
    events::StreamFrame, projections::ProjectionReader, store::CommitSignal, OpenMode, ProjectKey,
    Store,
};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    io,
    path::PathBuf,
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::{
    sync::{broadcast, watch, Mutex as AsyncMutex},
    task::JoinHandle,
    time::{sleep_until, Instant},
};

mod wal_watch;

#[cfg(feature = "test-observations")]
fn broadcast_identity(
    event: &Value,
) -> Option<(
    cortex_core::test_observations::Batch,
    Vec<cortex_core::test_observations::Change>,
)> {
    use cortex_core::test_observations::{Batch, Change};
    let batch = &event["batch"];
    Some((
        Batch {
            project: event["projectId"].as_str()?.into(),
            generation: batch["generation"].as_str()?.into(),
            from: batch["fromSequence"].as_str()?.parse().ok()?,
            through: batch["throughSequence"].as_str()?.parse().ok()?,
        },
        batch["changes"]
            .as_array()?
            .iter()
            .map(|change| {
                Some(Change {
                    kind: change["kind"].as_str()?.into(),
                    id: change["id"].as_str()?.into(),
                    version: change["newVersion"].as_str()?.parse().ok()?,
                })
            })
            .collect::<Option<_>>()?,
    ))
}

struct Entry {
    project: String,
    reader: ProjectionReader,
    subscription: cortex_core::events::Subscription,
}
type ReaderSlot = Arc<AsyncMutex<Option<Entry>>>;

#[derive(Default)]
pub(crate) struct Registry {
    readers: Mutex<Readers>,
}

#[derive(Default)]
struct Readers {
    closed: bool,
    registrations: BTreeMap<String, Arc<ProjectRegistration>>,
    slots: BTreeMap<ProjectKey, Slot>,
}

pub(crate) struct ProjectRegistration(watch::Sender<bool>);

impl Default for ProjectRegistration {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}

impl ProjectRegistration {
    pub(crate) fn stopped(&self) -> watch::Receiver<bool> {
        self.0.subscribe()
    }
}

#[derive(Default)]
struct Slot {
    // The poller owns the reusable reader. Retiring a slot under the registry
    // lock cannot close SQLite or drop a blocked read's access guard there.
    entry: Weak<AsyncMutex<Option<Entry>>>,
    running: Option<Running>,
}

impl Slot {
    fn entry(&mut self) -> ReaderSlot {
        if let Some(entry) = self.entry.upgrade() {
            return entry;
        }
        if let Some(running) = self.running.take() {
            running.stop();
        }
        let entry = Arc::default();
        self.entry = Arc::downgrade(&entry);
        entry
    }
}

struct Running {
    project: String,
    stop: watch::Sender<bool>,
    task: JoinHandle<()>,
}

impl Running {
    fn stop(self) -> JoinHandle<()> {
        self.stop.send_replace(true);
        self.task
    }
}

impl Readers {
    fn check(&self, project: &str, registration: &Arc<ProjectRegistration>) -> io::Result<()> {
        if self.closed {
            return Err(failure("service_stopping"));
        }
        if !self
            .registrations
            .get(project)
            .is_some_and(|current| Arc::ptr_eq(current, registration))
        {
            return Err(failure("project_inactive"));
        }
        Ok(())
    }
}

const FALLBACK: Duration = Duration::from_millis(250);
const COALESCE: Duration = Duration::from_millis(25);
const WAL_FINAL_RECHECK: Duration = Duration::from_millis(50);

// A writer can advance the outbox between poll_model and its read transaction.
fn consistent_read<T>(
    mut read: impl FnMut() -> cortex_core::CoreResult<T>,
) -> cortex_core::CoreResult<T> {
    for attempt in 0..3 {
        match read() {
            Err(error) if error.code() == "snapshot_expired" && attempt < 2 => continue,
            result => return result,
        }
    }
    unreachable!()
}
fn key(root: &std::path::Path) -> io::Result<ProjectKey> {
    crate::native_runtime::key(root).map_err(err)
}
impl Registry {
    // Call under Service::state so removal invalidates even a queued initial query.
    fn registration(&self, project: &str) -> io::Result<Arc<ProjectRegistration>> {
        let mut readers = self.readers.lock().unwrap();
        if readers.closed {
            return Err(failure("service_stopping"));
        }
        Ok(readers
            .registrations
            .entry(project.into())
            .or_default()
            .clone())
    }

    pub(crate) fn remove(&self, project: &str) -> Vec<JoinHandle<()>> {
        let mut readers = self.readers.lock().unwrap();
        if let Some(registration) = readers.registrations.remove(project) {
            registration.0.send_replace(true);
        }
        let mut stopped = Vec::new();
        readers.slots.retain(|_, slot| {
            if slot.running.as_ref().is_some_and(|r| r.project == project) {
                stopped.push(slot.running.take().unwrap().stop());
                false
            } else {
                true
            }
        });
        stopped
    }

    pub(crate) async fn shutdown(&self) {
        let stopped = {
            let mut readers = self.readers.lock().unwrap();
            readers.closed = true;
            for (_, registration) in std::mem::take(&mut readers.registrations) {
                registration.0.send_replace(true);
            }
            std::mem::take(&mut readers.slots)
                .into_values()
                .filter_map(|slot| slot.running.map(Running::stop))
                .collect::<Vec<_>>()
        };
        for task in stopped {
            let _ = task.await;
        }
    }

    fn projected<T>(
        &self,
        root: PathBuf,
        project: String,
        registration: Arc<ProjectRegistration>,
        events: broadcast::Sender<Value>,
        mut read: impl FnMut(&ProjectionReader) -> cortex_core::CoreResult<T>,
    ) -> io::Result<T> {
        // Conversation pages and task facts answer the owner's screen: read at the session's
        // disk priority, not the level the service unit gives task processes (`io_priority`).
        let _disk = cortex_core::io_priority::raised();
        project_guard::check(&root)?;
        let key = key(&root)?;
        let slot = {
            let mut readers = self.readers.lock().unwrap();
            readers.check(&project, &registration)?;
            readers.slots.entry(key.clone()).or_default().entry()
        };
        let result = (|| {
            let mut entry = slot.blocking_lock();
            self.readers.lock().unwrap().check(&project, &registration)?;
            if let Some(current) = entry.as_ref() {
                if current.project != project {
                    return Err(failure("project_context_conflict"));
                }
                let reply = consistent_read(|| read(&current.reader)).map_err(err)?;
                self.readers.lock().unwrap().check(&project, &registration)?;
                return Ok(reply);
            }

            // The owner waits for this read: queue behind an exclusive migration, never `busy`.
            let store = Store::open_waiting(key.clone(), OpenMode::LiveReadOnly).map_err(err)?;
            let wal_directory = store
                .key()
                .home_directory()
                .filter(|home| home == &store.key().directory());
            let reader = ProjectionReader::new(store, project.clone(), actor()).map_err(err)?;
            let reply = consistent_read(|| read(&reader)).map_err(err)?;
            let (generation, high) = reader.stream_position().map_err(err)?;
            let subscription = reader.subscribe(&generation, high).map_err(err)?;
            let mut readers = self.readers.lock().unwrap();
            readers.check(&project, &registration)?;
            // An alias may have owned this slot before a read failure invalidated its entry.
            // Removal can evict that slot while the replacement snapshot is opening.
            if !readers
                .slots
                .get(&key)
                .is_some_and(|current| std::ptr::eq(current.entry.as_ptr(), Arc::as_ptr(&slot)))
            {
                return Err(failure("snapshot_expired"));
            }
            // If the logical store at this path changed, retire the old poller before
            // installing the new generation. Stop never has to acquire a reader lock.
            readers.slots.retain(|old_key, old| {
                if old_key != &key && old.running.as_ref().is_some_and(|r| r.project == project) {
                    old.running.take().unwrap().stop();
                    false
                } else {
                    true
                }
            });
            let running = &mut readers.slots.get_mut(&key).unwrap().running;
            // A new snapshot can replace a stale reader at the same logical key,
            // including a formerly checkout-local store that has moved home.
            if let Some(previous) = running.take() {
                previous.stop();
            }
            *running = Some(Poller {
                root,
                signal: key.commit_signal(),
                key: key.clone(),
                project: project.clone(),
                entry: slot.clone(),
                events,
                wal_directory,
            }
            .start());
            *entry = Some(Entry {
                project,
                reader,
                subscription,
            });
            Ok(reply)
        })();
        // A failed initial read leaves no subscriber. Last concurrent request prunes the slot.
        drop(slot);
        let mut readers = self.readers.lock().unwrap();
        if readers
            .slots
            .get(&key)
            .is_some_and(|s| s.running.is_none() && s.entry.strong_count() == 0)
        {
            readers.slots.remove(&key);
        }
        result
    }
}

struct Poller {
    root: PathBuf,
    key: ProjectKey,
    project: String,
    entry: ReaderSlot,
    signal: Arc<CommitSignal>,
    events: broadcast::Sender<Value>,
    wal_directory: Option<PathBuf>,
}

#[derive(Clone, Copy)]
struct WalRecheck {
    at: Instant,
    next_delay: Option<Duration>,
}

impl Poller {
    fn start(self) -> Running {
        self.start_with(poll)
    }

    fn start_with<F>(self, read_once: F) -> Running
    where
        F: Fn(&std::path::Path, &ProjectKey, &str, &mut Option<Entry>) -> io::Result<Vec<Value>>
            + Clone
            + Send
            + 'static,
    {
        let (stop, stopped) = watch::channel(false);
        let project = self.project.clone();
        let task = tokio::spawn(self.run(stopped, read_once));
        Running {
            project,
            stop,
            task,
        }
    }

    async fn run<F>(self, mut stop: watch::Receiver<bool>, read_once: F)
    where
        F: Fn(&std::path::Path, &ProjectKey, &str, &mut Option<Entry>) -> io::Result<Vec<Value>>
            + Clone
            + Send
            + 'static,
    {
        // The first poll catches commits between the initial snapshot and registration.
        let mut timing = None;
        let mut wal = None;
        let mut recheck: Option<WalRecheck> = None;
        loop {
            if wal.is_none() {
                wal = self
                    .wal_directory
                    .as_deref()
                    .and_then(|directory| wal_watch::Watch::new(directory).ok());
            }
            let next_recheck = if let Some((ready, fallback)) = timing {
                // Leave the single permit untouched during cooldown. A burst through the
                // current read and the cooldown thus asks for one trailing poll, not a queue.
                tokio::select! {
                    biased;
                    _ = stop.changed() => return,
                    _ = sleep_until(ready) => (),
                }
                tokio::select! {
                    biased;
                    _ = stop.changed() => return,
                    _ = self.signal.notified() => None,
                    _ = sleep_until(fallback) => None,
                    result = wal_watch::wait(wal.as_ref()) => {
                        if result.is_err() {
                            wal = None;
                        }
                        result.is_ok().then_some((COALESCE, Some(WAL_FINAL_RECHECK)))
                    },
                    // A WAL write can notify before SQLite makes the commit
                    // visible. Only an empty WAL poll can schedule a recheck.
                    _ = sleep_until(recheck.map_or(ready, |check| check.at)), if recheck.is_some() => {
                        recheck.and_then(|check| check.next_delay).map(|delay| (delay, None))
                    },
                }
            } else {
                None
            };
            recheck = None;
            let mut entry = tokio::select! {
                biased;
                _ = stop.changed() => return,
                entry = self.entry.clone().lock_owned() => entry,
            };
            let started = Instant::now();
            let root = self.root.clone();
            let key = self.key.clone();
            let project = self.project.clone();
            let cancelled = stop.clone();
            let read_once = read_once.clone();
            let mut read = tokio::task::spawn_blocking(move || {
                // A running blocking call cannot be aborted. It owns only this slot
                // and cannot publish, retain the service, or keep the wake registered.
                let events = if *cancelled.borrow() {
                    Vec::new()
                } else {
                    read_once(&root, &key, &project, &mut entry).unwrap_or_default()
                };
                (entry, events)
            });
            let result = tokio::select! {
                biased;
                _ = stop.changed() => {
                    read.abort();
                    return;
                }
                result = &mut read => result,
            };
            if let Ok((_entry, events)) = result {
                if events.is_empty() {
                    recheck = next_recheck.map(|(delay, next_delay)| WalRecheck {
                        at: started + delay,
                        next_delay,
                    });
                }
                // Keep the reader guard until its drained frames are delivered, and
                // serialize the final send with removal's stop. Neither lock spans an await.
                let stopped = stop.borrow();
                if *stopped {
                    return;
                }
                for event in events {
                    broadcast(&self.events, event);
                }
            }
            timing = Some((started + COALESCE, Instant::now() + FALLBACK));
        }
    }
}

fn poll(
    root: &std::path::Path,
    expected: &ProjectKey,
    project: &str,
    entry: &mut Option<Entry>,
) -> io::Result<Vec<Value>> {
    let Some(current) = entry.as_ref().filter(|entry| entry.project == project) else {
        return Ok(Vec::new());
    };
    // A stalled poll holds this project's reader, which the next page read waits for.
    let _disk = cortex_core::io_priority::raised();
    project_guard::check(root)?;
    if &key(root)? != expected {
        return Ok(Vec::new());
    }
    if let Err(error) = current.reader.poll() {
        // Drop the stale generation; the next snapshot opens a fresh read provider.
        *entry = None;
        return Ok(vec![
            json!({"type":"native_resync","projectId":project,"reason":error.code()}),
        ]);
    }
    Ok(ProjectionReader::drain(&current.subscription)
        .into_iter()
        .map(|frame| match frame {
            StreamFrame::Delta(batch) => {
                let heads = current.reader.history_heads(&batch.generation, batch.through_sequence);
                let mut event = json!({"type":"native_delta","projectId":project,"batch":batch});
                if !heads.is_empty() {
                    event["historyHeads"] = json!(heads);
                }
                event
            }
            StreamFrame::Resync(r) => {
                json!({"type":"native_resync","projectId":project,"reason":r.reason})
            }
        })
        .collect())
}

fn broadcast(events: &broadcast::Sender<Value>, event: Value) {
    #[cfg(feature = "test-observations")]
    let trace = (event["type"] == "native_delta" && cortex_core::test_observations::observing(None))
        .then(|| broadcast_identity(&event));
    let _sent = events.send(event);
    #[cfg(feature = "test-observations")]
    if let Some(trace) = trace {
        use cortex_core::test_observations::{record, Observation};
        let sent = std::time::Instant::now();
        record(match (trace, _sent) {
            (Some((batch, changes)), Ok(receivers)) => {
                Observation::Broadcast { batch, changes, sent, receivers }
            }
            _ => Observation::Missing,
        });
    }
}

#[cfg(test)]
mod poller_tests;

#[cfg(test)]
mod startup_reads {
    use super::*;
    #[test]
    fn retries_snapshot_races_but_never_hides_persistent_or_policy_failures() {
        let mut calls = 0;
        let result = consistent_read(|| {
            calls += 1;
            if calls < 3 {
                Err(cortex_core::CoreError::Refused("snapshot_expired"))
            } else {
                Ok(31)
            }
        });
        assert_eq!(result.unwrap(), 31);
        assert_eq!(calls, 3);
        calls = 0;
        let failure = consistent_read::<()>(|| {
            calls += 1;
            Err(cortex_core::CoreError::Refused("snapshot_expired"))
        });
        assert_eq!(failure.unwrap_err().code(), "snapshot_expired");
        assert_eq!(calls, 3);
        calls = 0;
        let failure = consistent_read::<()>(|| {
            calls += 1;
            Err(cortex_core::CoreError::Refused("project_inactive"))
        });
        assert_eq!(failure.unwrap_err().code(), "project_inactive");
        assert_eq!(calls, 1);
    }
}
impl Service {
    pub(crate) async fn projection_registration(
        &self,
        project: &str,
    ) -> io::Result<(PathBuf, Arc<ProjectRegistration>)> {
        let state = self.state.lock().await;
        let entry = state
            .projects
            .iter()
            .find(|p| p.project_id == project)
            .ok_or_else(|| failure("project_not_found"))?;
        if entry.activation.is_none() {
            return Err(failure("project_inactive"));
        }
        Ok((entry.canonical_root.clone(), self.core.registration(project)?))
    }

    pub(crate) async fn follow_history_read(
        &self,
        root: PathBuf,
        registration: Arc<ProjectRegistration>,
        request: workspace_protocol::read::HistoryFollowRequest,
        previous_decision: Option<String>,
    ) -> io::Result<cortex_core::projections::HistoryFollowRead> {
        let project = request.project_id.clone();
        let host = self.core.clone();
        let events = self.events.clone();
        tokio::task::spawn_blocking(move || {
            host.projected(root, project, registration, events, |reader| {
                reader.follow_history(&request, previous_decision.as_deref())
            })
        })
        .await
        .map_err(|_| failure("storage_unavailable"))?
    }

    pub async fn native_query(&self, params: Value) -> io::Result<Value> {
        let request = workspace_protocol::core::decode_native_query(
            &serde_json::to_vec(&params).map_err(|_| failure("invalid_request"))?,
        )
        .map_err(failure)?;
        let project = request
            .project_id
            .clone()
            .ok_or_else(|| failure("invalid_project"))?;
        let (root, registration) = self.projection_registration(&project).await?;
        let host = self.core.clone();
        let events = self.events.clone();
        let reply = tokio::task::spawn_blocking(move || {
            host.projected(root, project, registration, events, |reader| {
                reader.query(request.body.clone())
            })
        })
            .await
            .map_err(|_| failure("storage_unavailable"))??;
        serde_json::to_value(reply).map_err(|_| failure("invalid_reply"))
    }
}
