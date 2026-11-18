//! Bounded blocking SQLite lanes. Construct/drop on a service blocking thread;
//! awaiting returned oneshot receivers never runs SQLite on an async reactor.
use super::{OpenMode, ProjectKey, Store};
use crate::{
    error::refuse,
    ownership::{ProjectOwner, SchedulerOwner},
    CoreError, CoreResult,
};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{sync_channel, RecvTimeoutError, SyncSender},
        Arc,
    },
    time::{Duration, Instant},
};
/// The store, or the refusal that stopped this job from running (a shared
/// writer that could not retake the scheduler lease).
type Job = Box<dyn FnOnce(CoreResult<&Store>) + Send + 'static>;
/// How long an idle shared writer keeps the scheduler lease.
const IDLE_RELEASE: Duration = Duration::from_millis(1000);
/// How long opening a shared writer waits for another process's idle release.
const OPEN_WAIT: Duration = Duration::from_millis(5000);
type Fences = (SchedulerOwner, ProjectOwner);
fn fences(key: &ProjectKey) -> CoreResult<Fences> {
    let scheduler = SchedulerOwner::acquire(key)?;
    // A peer releasing both locks can briefly hold only the project lock: that is
    // the same "another owner is busy" state, retried like the scheduler lock.
    let project = ProjectOwner::acquire(key).map_err(|e| match e.code() {
        "busy" => CoreError::Refused("scheduler_owner_busy"),
        _ => e,
    })?;
    Ok((scheduler, project))
}
/// Take both fences, waiting up to `OPEN_WAIT` for another process's idle release.
fn fences_waiting(key: &ProjectKey) -> CoreResult<Fences> {
    let started = Instant::now();
    loop {
        match fences(key) {
            Err(e) if e.code() == "scheduler_owner_busy" && started.elapsed() < OPEN_WAIT => {
                std::thread::sleep(Duration::from_millis(50))
            }
            other => return other,
        }
    }
}
/// True when no task is unfinished, so no scheduler state depends on the lease.
fn idle(store: &Store) -> bool {
    store
        .read(|db| {
            Ok(db.query_row(
                "SELECT NOT EXISTS(SELECT 1 FROM runtime_tasks WHERE state NOT IN ('COMPLETED','CANCELLED','FAILED'))",
                [],
                |r| r.get::<_, bool>(0),
            )?)
        })
        .unwrap_or(false)
}
struct Lane {
    sender: Option<SyncSender<Job>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Lane {
    fn start(
        key: ProjectKey,
        mode: OpenMode,
        owner: bool,
        shared: bool,
        orphaned: Arc<AtomicBool>,
        name: &str,
    ) -> CoreResult<Self> {
        let (send, recv) = sync_channel::<Job>(128);
        let (ready, wait) = sync_channel(1);
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let setup = || -> CoreResult<_> {
                    // Acquire the owner fence BEFORE any writable initialization.
                    let lease = if owner {
                        super::locks::private_directory(&key.directory(), true)?;
                        // A shared writer waits for another process's idle release;
                        // an exclusive one refuses at once.
                        Some(if shared {
                            fences_waiting(&key)?
                        } else {
                            fences(&key)?
                        })
                    } else {
                        None
                    };
                    let store = Store::open(key.clone(), mode)?;
                    Ok((store, lease))
                };
                match setup() {
                    Ok((store, mut lease)) => {
                        if ready.send(Ok(())).is_err() {
                            return;
                        }
                        loop {
                            let job = if shared {
                                match recv.recv_timeout(IDLE_RELEASE) {
                                    Ok(job) => job,
                                    Err(RecvTimeoutError::Timeout) => {
                                        // Release only with every task finished: the
                                        // next job retakes the fences or is refused.
                                        if lease.is_some() && idle(&store) {
                                            lease = None;
                                        }
                                        continue;
                                    }
                                    Err(RecvTimeoutError::Disconnected) => break,
                                }
                            } else {
                                match recv.recv() {
                                    Ok(job) => job,
                                    Err(_) => break,
                                }
                            };
                            if shared && owner && lease.is_none() {
                                match fences_waiting(&key) {
                                    // A live process never releases the lease with an
                                    // unfinished task, so an unfinished task found here was
                                    // left by a process that is gone. Keep the lease and ask
                                    // the runtime to recover it (never two schedulers).
                                    Ok(taken) => {
                                        if !idle(&store) {
                                            orphaned.store(true, Ordering::SeqCst);
                                        }
                                        lease = Some(taken)
                                    }
                                    Err(e) => {
                                        job(Err(e));
                                        continue;
                                    }
                                }
                            }
                            job(Ok(&store));
                        }
                    }
                    Err(error) => {
                        let _ = ready.send(Err(error));
                    }
                }
            })?;
        match wait.recv() {
            Ok(Ok(())) => Ok(Self {
                sender: Some(send),
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            }
            Err(_) => {
                let _ = thread.join();
                refuse("storage_executor_unavailable")
            }
        }
    }
    fn submit(&self, job: Job) -> CoreResult<()> {
        self.sender
            .as_ref()
            .ok_or(CoreError::Refused("storage_executor_closed"))?
            .try_send(job)
            .map_err(|e| match e {
                std::sync::mpsc::TrySendError::Full(_) => CoreError::Refused("busy"),
                std::sync::mpsc::TrySendError::Disconnected(_) => {
                    CoreError::Refused("storage_executor_closed")
                }
            })
    }
}
impl Drop for Lane {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
/// One owned writer and four process-lifetime read connections per logical project.
/// Shutdown joins lanes, and must be run outside a reactor just like construction.
pub struct StoreExecutor {
    readers: Vec<Lane>,
    writer: Lane,
    next: AtomicUsize,
    orphaned: Arc<AtomicBool>,
}
impl StoreExecutor {
    pub fn open(key: ProjectKey) -> CoreResult<Self> {
        Self::open_with(key, false)
    }
    /// Like `open`, but for one of several same-project processes (ACP agents):
    /// opening waits briefly for another process's idle release, and this
    /// writer hands the scheduler lease over once every task is finished. A
    /// later job retakes the lease, or is refused with `scheduler_owner_busy`
    /// while another process holds it. Never two schedulers for one task.
    pub fn open_shared(key: ProjectKey) -> CoreResult<Self> {
        Self::open_with(key, true)
    }
    /// True once since the last call when this shared writer retook the lease and found
    /// an unfinished task left by a process that is gone: the runtime must recover it.
    pub fn take_orphaned(&self) -> bool {
        self.orphaned.swap(false, Ordering::SeqCst)
    }
    fn open_with(key: ProjectKey, shared: bool) -> CoreResult<Self> {
        let orphaned = Arc::new(AtomicBool::new(false));
        let writer = Lane::start(
            key.clone(),
            OpenMode::Initialize,
            true,
            shared,
            orphaned.clone(),
            "cortex-sqlite-writer",
        )?;
        let mut readers = Vec::new();
        for n in 0..4 {
            readers.push(Lane::start(
                key.clone(),
                OpenMode::RuntimeValidate,
                false,
                false,
                Arc::new(AtomicBool::new(false)),
                &format!("cortex-sqlite-read-{n}"),
            )?);
        }
        Ok(Self {
            writer,
            readers,
            next: AtomicUsize::new(0),
            orphaned,
        })
    }
    /// Run a file-backed domain operation on the owned writer lane. Publication
    /// manages its own transactions and crash intents, so it cannot be nested in
    /// write(). No other host mutation interleaves with this operation.
    pub fn publication<T: Send + 'static>(
        &self,
        body: impl FnOnce(&Store) -> CoreResult<T> + Send + 'static,
    ) -> CoreResult<tokio::sync::oneshot::Receiver<CoreResult<T>>> {
        let (send, recv) = tokio::sync::oneshot::channel();
        self.writer.submit(Box::new(move |store| {
            let _ = send.send(store.and_then(body));
        }))?;
        Ok(recv)
    }
    pub fn write<T: Send + 'static>(
        &self,
        body: impl FnOnce(&rusqlite::Transaction<'_>) -> CoreResult<T> + Send + 'static,
    ) -> CoreResult<tokio::sync::oneshot::Receiver<CoreResult<T>>> {
        let (send, recv) = tokio::sync::oneshot::channel();
        self.writer.submit(Box::new(move |store| {
            let _ = send.send(store.and_then(|store| store.write(body)));
        }))?;
        Ok(recv)
    }
    pub fn read<T: Send + 'static>(
        &self,
        body: impl FnOnce(&rusqlite::Connection) -> CoreResult<T> + Send + 'static,
    ) -> CoreResult<tokio::sync::oneshot::Receiver<CoreResult<T>>> {
        let (send, recv) = tokio::sync::oneshot::channel();
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        self.readers[index].submit(Box::new(move |store| {
            let _ = send.send(store.and_then(|store| store.read(body)));
        }))?;
        Ok(recv)
    }
}
