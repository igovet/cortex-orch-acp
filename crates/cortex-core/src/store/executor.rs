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
/// Time an interactive operation can wait for admission, not for SQLite or commit.
const OWNER_QUEUE_WAIT: Duration = Duration::from_secs(2);
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
    sender: Option<SyncSender<Item>>,
    thread: Option<std::thread::JoinHandle<()>>,
    space: Arc<tokio::sync::Notify>,
    /// The writer lane's deferred display writes; `None` on read lanes.
    deferred: Option<Arc<Deferred>>,
}
/// What a lane receives: an ordinary job with its submission ticket, or a nudge to
/// re-examine the deferred writes (a first deferral, or a full deferral batch).
enum Item {
    Job(u64, Job),
    Wake,
}
/// A display-only write committed later, together with others (see
/// [`StoreExecutor::write_deferred`]).
type DeferredBody = Box<dyn FnOnce(&rusqlite::Transaction<'_>) -> CoreResult<()> + Send + 'static>;
/// How long a deferred write may wait for companions before it commits.
pub const DEFER_WINDOW: Duration = Duration::from_millis(250);
/// A batch this large commits at once.
pub const DEFER_LIMIT: usize = 64;
#[derive(Default)]
struct Deferred {
    state: std::sync::Mutex<DeferredState>,
}
#[derive(Default)]
struct DeferredState {
    /// One counter orders ordinary jobs and deferred writes alike.
    ticket: u64,
    pending: Vec<(u64, DeferredBody)>,
    since: Option<Instant>,
}
impl Deferred {
    fn lock(&self) -> std::sync::MutexGuard<'_, DeferredState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Deferred writes submitted before `ticket`, or all of them.
    fn take(&self, before: Option<u64>) -> Vec<DeferredBody> {
        let mut state = self.lock();
        let split = match before {
            Some(ticket) => state.pending.partition_point(|(t, _)| *t < ticket),
            None => state.pending.len(),
        };
        let taken: Vec<_> = state.pending.drain(..split).map(|(_, b)| b).collect();
        if state.pending.is_empty() {
            state.since = None;
        } else if !taken.is_empty() {
            // The remaining writes were deferred after the ones taken; restart their window.
            state.since = Some(Instant::now());
        }
        taken
    }
    /// Time until the oldest pending write is due, when there is one.
    fn due_in(&self) -> Option<Duration> {
        let state = self.lock();
        let since = state.since?;
        Some(if state.pending.len() >= DEFER_LIMIT {
            Duration::ZERO
        } else {
            DEFER_WINDOW.saturating_sub(since.elapsed())
        })
    }
}
/// One transaction for a batch of deferred writes. Each runs inside its own
/// savepoint: one that fails is rolled back alone and the rest still commit.
fn commit_deferred(store: CoreResult<&Store>, bodies: Vec<DeferredBody>) {
    let Ok(store) = store else { return };
    let _ = store.write(|tx| {
        for body in bodies {
            tx.execute_batch("SAVEPOINT cortex_deferred")?;
            match body(tx) {
                Ok(()) => tx.execute_batch("RELEASE cortex_deferred")?,
                Err(_) => {
                    tx.execute_batch("ROLLBACK TO cortex_deferred; RELEASE cortex_deferred")?
                }
            }
        }
        Ok(())
    });
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
        let (send, recv) = sync_channel::<Item>(128);
        let (ready, wait) = sync_channel(1);
        let deferred = owner.then(|| Arc::new(Deferred::default()));
        let pending = deferred.clone();
        let space = Arc::new(tokio::sync::Notify::new());
        let dequeued = space.clone();
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
                        let run = |job: Job, lease: &mut Option<Fences>| {
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
                                        *lease = Some(taken)
                                    }
                                    Err(e) => {
                                        job(Err(e));
                                        return;
                                    }
                                }
                            }
                            job(Ok(&store));
                        };
                        // Deferred writes submitted before `before` (all when `None`)
                        // commit first, as one transaction through the same lease path.
                        let flush = |before: Option<u64>, lease: &mut Option<Fences>| {
                            let Some(deferred) = &pending else { return };
                            let bodies = deferred.take(before);
                            if !bodies.is_empty() {
                                run(Box::new(move |store| commit_deferred(store, bodies)), lease);
                            }
                        };
                        loop {
                            let due = pending.as_ref().and_then(|d| d.due_in());
                            let wait = match (due, shared) {
                                (Some(due), true) => Some(due.min(IDLE_RELEASE)),
                                (Some(due), false) => Some(due),
                                (None, true) => Some(IDLE_RELEASE),
                                (None, false) => None,
                            };
                            let received = match wait {
                                Some(wait) => recv.recv_timeout(wait),
                                None => recv.recv().map_err(|_| RecvTimeoutError::Disconnected),
                            };
                            if owner && received.is_ok() {
                                dequeued.notify_waiters();
                            }
                            let (ticket, job) = match received {
                                Ok(Item::Job(ticket, job)) => (ticket, job),
                                Ok(Item::Wake) => continue,
                                Err(RecvTimeoutError::Timeout) => {
                                    if due.is_some() {
                                        if pending.as_ref().and_then(|d| d.due_in())
                                            == Some(Duration::ZERO)
                                        {
                                            flush(None, &mut lease);
                                        }
                                        continue;
                                    }
                                    // Release only with every task finished: the
                                    // next job retakes the fences or is refused.
                                    if lease.is_some() && idle(&store) {
                                        lease = None;
                                    }
                                    continue;
                                }
                                Err(RecvTimeoutError::Disconnected) => {
                                    // Shutdown commits every write still deferred.
                                    flush(None, &mut lease);
                                    break;
                                }
                            };
                            // Never reorder: what was deferred before this job commits first.
                            flush(Some(ticket), &mut lease);
                            run(job, &mut lease);
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
                space,
                deferred,
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
        if self.try_submit(job)?.is_some() {
            return refuse("busy");
        }
        Ok(())
    }

    /// On a full queue, return the untouched job. Only admitted jobs get tickets.
    fn try_submit(&self, job: Job) -> CoreResult<Option<Job>> {
        use std::sync::mpsc::TrySendError;
        let sender = self
            .sender
            .as_ref()
            .ok_or(CoreError::Refused("storage_executor_closed"))?;
        // Ticketing and enqueue stay under the same lock as deferred writes.
        let mut state = self.deferred.as_ref().map(|d| d.lock());
        let ticket = state.as_ref().map_or(0, |s| s.ticket + 1);
        match sender.try_send(Item::Job(ticket, job)) {
            Ok(()) => {
                if let Some(state) = state.as_mut() {
                    state.ticket = ticket;
                }
                Ok(None)
            }
            Err(TrySendError::Full(Item::Job(_, job))) => Ok(Some(job)),
            Err(TrySendError::Full(Item::Wake)) => unreachable!("submitted a job"),
            Err(TrySendError::Disconnected(_)) => refuse("storage_executor_closed"),
        }
    }

    async fn submit_wait(&self, mut job: Job) -> CoreResult<()> {
        tokio::time::timeout(OWNER_QUEUE_WAIT, async {
            loop {
                let space = self.space.notified();
                tokio::pin!(space);
                // Observe a dequeue even if it races the failed admission attempt.
                space.as_mut().enable();
                match self.try_submit(job)? {
                    None => return Ok(()),
                    Some(unsent) => job = unsent,
                }
                space.await;
            }
        })
        .await
        .map_err(|_| CoreError::Refused("busy"))?
    }
    fn defer(&self, body: DeferredBody) -> CoreResult<u64> {
        let (Some(sender), Some(deferred)) = (self.sender.as_ref(), self.deferred.as_ref()) else {
            return refuse("storage_executor_closed");
        };
        let (wake, ticket) = {
            let mut state = deferred.lock();
            state.ticket += 1;
            let ticket = state.ticket;
            state.pending.push((ticket, body));
            let first = state.since.is_none();
            if first {
                state.since = Some(Instant::now());
            }
            (first || state.pending.len() == DEFER_LIMIT, ticket)
        };
        if wake {
            // A full queue already keeps the writer busy and re-examining its batch.
            if let Err(std::sync::mpsc::TrySendError::Disconnected(_)) = sender.try_send(Item::Wake)
            {
                return refuse("storage_executor_closed");
            }
        }
        Ok(ticket)
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
    /// Wait briefly for writer queue space before accepting an interactive job.
    /// A full-queue timeout happens before enqueue; after admission, return only
    /// the actual operation result. Accepted jobs keep the ordinary FIFO order.
    pub async fn publication_wait<T: Send + 'static>(
        &self,
        body: impl FnOnce(&Store) -> CoreResult<T> + Send + 'static,
    ) -> CoreResult<T> {
        let (send, recv) = tokio::sync::oneshot::channel();
        self.writer
            .submit_wait(Box::new(move |store| {
                let _ = send.send(store.and_then(body));
            }))
            .await?;
        recv.await
            .map_err(|_| CoreError::Refused("storage_executor_closed"))?
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
    /// Queue a display-only write on the writer lane without waiting for it. Writes
    /// deferred together commit in ONE transaction (each in its own savepoint, so a
    /// failing one is rolled back alone): after at most [`DEFER_WINDOW`], at once when
    /// [`DEFER_LIMIT`] are pending, before any ordinary writer job submitted after them
    /// (so every later write, a turn's settle event included, sees them committed and
    /// ordered first), and when the executor shuts down. The outcome is never reported.
    /// A process killed before the commit loses the pending writes, at most one window's
    /// worth: never use this for authority, lifecycle or any state another write relies on.
    /// Returns the write's ticket (see [`Self::submitted`]).
    pub fn write_deferred(
        &self,
        body: impl FnOnce(&rusqlite::Transaction<'_>) -> CoreResult<()> + Send + 'static,
    ) -> CoreResult<u64> {
        self.writer.defer(Box::new(body))
    }
    /// The ticket of the newest job or deferred write submitted to the writer lane. A
    /// deferred write whose ticket is still the newest has nothing ordered after it yet.
    pub fn submitted(&self) -> u64 {
        self.writer.deferred.as_ref().map_or(0, |d| d.lock().ticket)
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
