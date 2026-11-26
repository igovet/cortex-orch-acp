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
/// A caller's reply, given the outcome of the shared commit of its group.
type Reply = Box<dyn FnOnce(CoreResult<()>) + Send + 'static>;
/// One [`StoreExecutor::write`] body. Run inside a group transaction (or given the
/// refusal that stopped the group), it returns whether its changes stand and the reply
/// to send once the group's COMMIT outcome is known.
type TxJob =
    Box<dyn FnOnce(CoreResult<&rusqlite::Transaction<'_>>) -> (bool, Reply) + Send + 'static>;
/// What a writer job needs: the whole store (publication-style jobs that manage their own
/// transactions, run alone) or one transaction body (grouped with its neighbours).
enum Work {
    Store(Job),
    Tx(TxJob),
}
/// At most this many queued write bodies share one transaction and one fsync.
pub const GROUP_LIMIT: usize = 64;
/// A group stops taking queued bodies once it has run this long, so the transaction
/// (and the writer lock it holds) stays short.
pub const GROUP_BUDGET: Duration = Duration::from_millis(2);
/// How long an idle shared writer keeps the scheduler lease.
const IDLE_RELEASE: Duration = Duration::from_millis(1000);
/// How long opening a shared writer waits for another process's idle release.
const OPEN_WAIT: Duration = Duration::from_millis(5000);
const QUEUE_LIMIT: usize = 128;
/// Extra slots on the same FIFO, never available to background producers.
const OWNER_QUEUE_LIMIT: usize = 8;
/// Ordinary waiters (such as the sprint scheduler) do not use the reserve.
const QUEUE_WAIT: Duration = Duration::from_secs(2);
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
    space: Arc<tokio::sync::Semaphore>,
    owner_space: Arc<tokio::sync::Semaphore>,
    /// The writer lane's deferred display writes; `None` on read lanes.
    deferred: Option<Arc<Deferred>>,
}
/// What a lane receives: an ordinary job with its submission ticket, or a nudge to
/// re-examine the deferred writes (a first deferral, or a full deferral batch).
enum Item {
    Job(u64, Work, tokio::sync::OwnedSemaphorePermit),
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
    wake_queued: bool,
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
/// Close the savepoint around one body: keep its changes, or roll back only them. An
/// error means the shared transaction itself is gone (SQLite rolled it back).
fn end_savepoint(tx: &rusqlite::Transaction<'_>, kept: bool) -> CoreResult<()> {
    Ok(tx.execute_batch(if kept {
        "RELEASE cortex_group"
    } else {
        "ROLLBACK TO cortex_group; RELEASE cortex_group"
    })?)
}
/// Run deferred display writes inside the current transaction, each in its own savepoint.
fn run_deferred(tx: &rusqlite::Transaction<'_>, bodies: Vec<DeferredBody>) -> CoreResult<()> {
    for body in bodies {
        tx.execute_batch("SAVEPOINT cortex_group")?;
        let kept = body(tx).is_ok();
        end_savepoint(tx, kept)?;
    }
    Ok(())
}
/// One transaction for a batch of deferred writes alone. Each runs inside its own
/// savepoint: one that fails is rolled back alone and the rest still commit. Nothing
/// here is authority or lifecycle state, so the commit uses `synchronous=NORMAL`: it
/// does not wait for an fsync, and the next FULL commit makes it durable with its own.
fn commit_deferred(store: CoreResult<&Store>, bodies: Vec<DeferredBody>) {
    let Ok(store) = store else { return };
    let _ = store.write_relaxed(|tx| run_deferred(tx, bodies));
}
/// Group commit: `first` and the write bodies queued right behind it (at most
/// [`GROUP_LIMIT`], for at most [`GROUP_BUDGET`]) run in ONE `BEGIN IMMEDIATE` … `COMMIT`
/// with `synchronous=FULL`, each inside its own savepoint, so a failing body is rolled
/// back alone. Deferred writes submitted before a body run before it, in the same
/// transaction. Every reply is sent only after the shared COMMIT, in submission order:
/// a caller never sees success for a write that is not durable. A queued item that is
/// not a write body (a publication-style job or a wake) ends the group and is returned.
fn group(
    store: &Store,
    recv: &std::sync::mpsc::Receiver<Item>,
    deferred: Option<&Arc<Deferred>>,
    ticket: u64,
    first: TxJob,
) -> Option<Item> {
    let started = Instant::now();
    let mut first = Some((ticket, first));
    let mut next = None;
    let mut replies: Vec<Reply> = Vec::new();
    let committed = store.write(|tx| {
        loop {
            let (ticket, job) = match first.take() {
                Some(job) => job,
                None => {
                    if replies.len() >= GROUP_LIMIT || started.elapsed() >= GROUP_BUDGET {
                        break;
                    }
                    match recv.try_recv() {
                        Ok(Item::Job(ticket, Work::Tx(job), slot)) => {
                            drop(slot);
                            (ticket, job)
                        }
                        Ok(other) => {
                            next = Some(other);
                            break;
                        }
                        Err(_) => break,
                    }
                }
            };
            // Never reorder: what was deferred before this body commits first.
            if let Some(deferred) = deferred {
                if let Err(error) = run_deferred(tx, deferred.take(Some(ticket))) {
                    replies.push(job(Err(CoreError::Refused(error.code()))).1);
                    return Err(error);
                }
            }
            if let Err(error) = tx.execute_batch("SAVEPOINT cortex_group") {
                let error = CoreError::from(error);
                replies.push(job(Err(CoreError::Refused(error.code()))).1);
                return Err(error);
            }
            let (kept, reply) = job(Ok(tx));
            replies.push(reply);
            end_savepoint(tx, kept)?;
        }
        Ok(())
    });
    match committed {
        // The transaction never began: only the first body was taken, and it never ran.
        Err(error) if replies.is_empty() => {
            if let Some((_, job)) = first {
                job(Err(error)).1(Ok(()));
            }
        }
        Err(error) => {
            // Nothing in the group committed. A body that failed on its own keeps its error.
            for reply in replies {
                reply(Err(CoreError::Refused(error.code())));
            }
        }
        Ok(()) => {
            for reply in replies {
                reply(Ok(()));
            }
        }
    }
    next
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
        // A coalesced Wake occupies at most one slot, separate from both quotas.
        let reserve = if owner { OWNER_QUEUE_LIMIT } else { 0 };
        let (send, recv) = sync_channel::<Item>(QUEUE_LIMIT + reserve + usize::from(owner));
        let (ready, wait) = sync_channel(1);
        let deferred = owner.then(|| Arc::new(Deferred::default()));
        let pending = deferred.clone();
        let space = Arc::new(tokio::sync::Semaphore::new(QUEUE_LIMIT));
        let owner_space = Arc::new(tokio::sync::Semaphore::new(reserve));
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                // Store connections are the runtime's own bookkeeping, never a task process:
                // they read and commit at the session's default disk priority, not the
                // lowest level the service unit gives the work it starts (`io_priority`).
                crate::io_priority::raise_thread();
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
                    // A read lane queues behind an exclusive migration instead of refusing `busy`.
                    let store = if owner {
                        Store::open(key.clone(), mode)?
                    } else {
                        // One writer: a reader connection never commits domain data.
                        Store::open_waiting(key.clone(), mode)?.read_lane()
                    };
                    Ok((store, lease))
                };
                match setup() {
                    Ok((store, mut lease)) => {
                        if ready.send(Ok(())).is_err() {
                            return;
                        }
                        let retake = |lease: &mut Option<Fences>| -> CoreResult<()> {
                            if shared && owner && lease.is_none() {
                                // A live process never releases the lease with an
                                // unfinished task, so an unfinished task found here was
                                // left by a process that is gone. Keep the lease and ask
                                // the runtime to recover it (never two schedulers).
                                let taken = fences_waiting(&key)?;
                                if !idle(&store) {
                                    orphaned.store(true, Ordering::SeqCst);
                                }
                                *lease = Some(taken)
                            }
                            Ok(())
                        };
                        let run = |job: Job, lease: &mut Option<Fences>| match retake(lease) {
                            Ok(()) => job(Ok(&store)),
                            Err(e) => job(Err(e)),
                        };
                        let mut carried: Option<Item> = None;
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
                            let received = match (carried.take(), wait) {
                                (Some(item), _) => Ok(item),
                                (None, Some(wait)) => recv.recv_timeout(wait),
                                (None, None) => {
                                    recv.recv().map_err(|_| RecvTimeoutError::Disconnected)
                                }
                            };
                            let (ticket, work) = match received {
                                Ok(Item::Job(ticket, work, slot)) => {
                                    drop(slot);
                                    (ticket, work)
                                }
                                Ok(Item::Wake) => {
                                    if let Some(deferred) = &pending {
                                        deferred.lock().wake_queued = false;
                                    }
                                    continue;
                                }
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
                            match work {
                                Work::Store(job) => {
                                    // Never reorder: what was deferred before this job
                                    // commits first.
                                    flush(Some(ticket), &mut lease);
                                    run(job, &mut lease);
                                }
                                Work::Tx(job) => match retake(&mut lease) {
                                    Err(e) => job(Err(e)).1(Ok(())),
                                    Ok(()) => {
                                        carried =
                                            group(&store, &recv, pending.as_ref(), ticket, job)
                                    }
                                },
                            }
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
                owner_space,
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
    fn submit(&self, job: Work, owner: bool) -> CoreResult<()> {
        let space = if owner { &self.owner_space } else { &self.space };
        let slot = space.clone().try_acquire_owned().map_err(|_| {
            CoreError::Refused(if owner { "owner_queue_busy" } else { "busy" })
        })?;
        self.enqueue(job, slot)
    }

    /// Only admitted jobs get tickets. Both classes retain the same FIFO.
    fn enqueue(&self, job: Work, slot: tokio::sync::OwnedSemaphorePermit) -> CoreResult<()> {
        let sender = self
            .sender
            .as_ref()
            .ok_or(CoreError::Refused("storage_executor_closed"))?;
        // Ticketing and enqueue stay under the same lock as deferred writes.
        let mut state = self.deferred.as_ref().map(|d| d.lock());
        let ticket = state.as_ref().map_or(0, |s| s.ticket + 1);
        sender.try_send(Item::Job(ticket, job, slot)).map_err(|e| match e {
            std::sync::mpsc::TrySendError::Full(_) => CoreError::Refused("busy"),
            std::sync::mpsc::TrySendError::Disconnected(_) => CoreError::Refused("storage_executor_closed"),
        })?;
        if let Some(state) = state.as_mut() {
            state.ticket = ticket;
        }
        Ok(())
    }

    async fn submit_wait(&self, job: Work) -> CoreResult<()> {
        let slot = tokio::time::timeout(QUEUE_WAIT, self.space.clone().acquire_owned())
            .await
            .map_err(|_| CoreError::Refused("busy"))?
            .map_err(|_| CoreError::Refused("storage_executor_closed"))?;
        self.enqueue(job, slot)
    }
    fn defer(&self, body: DeferredBody) -> CoreResult<u64> {
        let (Some(sender), Some(deferred)) = (self.sender.as_ref(), self.deferred.as_ref()) else {
            return refuse("storage_executor_closed");
        };
        let mut state = deferred.lock();
        state.ticket += 1;
        let ticket = state.ticket;
        state.pending.push((ticket, body));
        let first = state.since.is_none();
        if first {
            state.since = Some(Instant::now());
        }
        if (first || state.pending.len() == DEFER_LIMIT) && !state.wake_queued {
            match sender.try_send(Item::Wake) {
                Ok(()) => state.wake_queued = true,
                // A full queue keeps the writer busy and re-examining its batch.
                Err(std::sync::mpsc::TrySendError::Full(_)) => {}
                Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    return refuse("storage_executor_closed");
                }
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
        self.writer.submit(Work::Store(Box::new(move |store| {
            let _ = send.send(store.and_then(body));
        })), false)?;
        Ok(recv)
    }

    /// Reserve space for an owner mutation on the existing ordered writer. If
    /// all reserved slots are in use, refuse before enqueue. Once enqueued,
    /// return the real result rather than treating a slow commit as a refusal.
    pub async fn owner_publication<T: Send + 'static>(
        &self,
        body: impl FnOnce(&Store) -> CoreResult<T> + Send + 'static,
    ) -> CoreResult<T> {
        let (send, recv) = tokio::sync::oneshot::channel();
        self.writer.submit(Work::Store(Box::new(move |store| {
            let _ = send.send(store.and_then(body));
        })), true)?;
        recv.await
            .map_err(|_| CoreError::Refused("storage_executor_closed"))?
    }

    /// Wait briefly for ordinary writer queue space before accepting a job.
    /// A full-queue timeout happens before enqueue; after admission, return only
    /// the actual operation result. Accepted jobs keep the ordinary FIFO order.
    pub async fn publication_wait<T: Send + 'static>(
        &self,
        body: impl FnOnce(&Store) -> CoreResult<T> + Send + 'static,
    ) -> CoreResult<T> {
        let (send, recv) = tokio::sync::oneshot::channel();
        self.writer
            .submit_wait(Work::Store(Box::new(move |store| {
                let _ = send.send(store.and_then(body));
            })))
            .await?;
        recv.await
            .map_err(|_| CoreError::Refused("storage_executor_closed"))?
    }

    /// One transaction body on the writer lane, committed with `synchronous=FULL`. It
    /// may share its transaction with the bodies queued beside it (group commit, see
    /// [`GROUP_LIMIT`]): it runs in its own savepoint, so its failure rolls back only
    /// it, and its reply arrives only after the shared COMMIT.
    pub fn write<T: Send + 'static>(
        &self,
        body: impl FnOnce(&rusqlite::Transaction<'_>) -> CoreResult<T> + Send + 'static,
    ) -> CoreResult<tokio::sync::oneshot::Receiver<CoreResult<T>>> {
        let (send, recv) = tokio::sync::oneshot::channel();
        // Grouped with the write bodies queued beside it (see [`group`]); the reply
        // waits for the shared COMMIT.
        self.writer.submit(Work::Tx(Box::new(move |tx| -> (bool, Reply) {
            match tx.and_then(body) {
                Ok(value) => (true, Box::new(move |committed: CoreResult<()>| {
                    let _ = send.send(committed.map(|()| value));
                })),
                Err(error) => (false, Box::new(move |_| {
                    let _ = send.send(Err(error));
                })),
            }
        })), false)?;
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
    /// A batch committed on its own uses `synchronous=NORMAL` (see [`Store::write_relaxed`]);
    /// one committed in a group with an ordinary write uses that group's FULL commit.
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
        self.readers[index].submit(Work::Store(Box::new(move |store| {
            let _ = send.send(store.and_then(|store| store.read(body)));
        })), false)?;
        Ok(recv)
    }
    /// Run a read-only domain operation on the reader pool: a WAL snapshot on a reader
    /// connection, never queued behind the writer lane or its fsync. The store it gets
    /// refuses every write with `storage_read_lane`; an operation that needs one (a
    /// crash reconciliation, say) is then repeated whole through [`Self::publication`].
    pub fn read_store<T: Send + 'static>(
        &self,
        body: impl FnOnce(&Store) -> CoreResult<T> + Send + 'static,
    ) -> CoreResult<tokio::sync::oneshot::Receiver<CoreResult<T>>> {
        let (send, recv) = tokio::sync::oneshot::channel();
        let index = self.next.fetch_add(1, Ordering::Relaxed) % self.readers.len();
        self.readers[index].submit(Work::Store(Box::new(move |store| {
            let _ = send.send(store.and_then(body));
        })), false)?;
        Ok(recv)
    }
}
