//! The synchronous connection core behind [`Db`](super::Db): one writer
//! thread owning the write connection, and a small pool of reader
//! threads each owning a read connection against the WAL.
//!
//! The topology re-expresses the seam's established doctrine (one
//! dedicated writer serialising every mutation; concurrent readers so a
//! live write stream never stalls dashboard reads) in synchronous
//! terms: exclusive access to a connection IS the thread that owns it,
//! so there is no pool checkout, no lock order, and no async executor
//! between a caller and SQLite. Callers submit closures; the owning
//! thread runs them to completion in arrival order and replies over a
//! oneshot channel.
//!
//! Panic semantics match `tokio::task::spawn_blocking`: a closure that
//! panics propagates its payload to the awaiting caller (the worker
//! thread itself survives and serves the next job). A caller that
//! drops mid-flight simply never observes its result; the job still
//! runs to completion on the worker, so a submitted write is never
//! half-abandoned.

use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rusqlite::Connection;

use super::DbError;

/// The number of reader threads (and connections). SQLite in WAL mode
/// serves many concurrent readers against one writer; a handful is
/// ample for a desktop app's dashboard, and keeps the page-cache
/// footprint bounded.
const READER_POOL_SIZE: usize = 4;

/// The page cache each connection may grow to, in KiB (the leading `-`
/// is SQLite's "kibibytes, not pages" sign): 64 MB, for a database
/// heading past a gigabyte. Pages are demand-allocated up to the
/// limit, so the resident cost tracks real working set, not the
/// ceiling.
const CACHE_SIZE_KIB: i64 = -64000;

/// A unit of work for a connection-owning thread. The outer `Option` is
/// the closure's one-shot consumption slot; the payload result travels
/// back over the caller's channel inside the closure itself.
type Job = Box<dyn FnOnce(&mut Connection) + Send>;

/// One connection-owning worker thread: jobs arrive over the channel
/// and run serially on the owned connection.
fn run_worker(mut connection: Connection, jobs: Arc<Mutex<mpsc::Receiver<Job>>>) {
    loop {
        // Take the receiver lock only to dequeue; the job itself runs
        // with the lock released so sibling readers dequeue freely.
        let job = {
            let receiver = jobs.lock().expect("job receiver lock");
            receiver.recv()
        };
        match job {
            Ok(job) => job(&mut connection),
            // All senders dropped: the core is closing.
            Err(mpsc::RecvError) => break,
        }
    }
}

/// Open one connection with the seam's session configuration: WAL
/// journal, NORMAL synchronous, a five-second busy timeout, foreign
/// keys off (matching the established pragma surface, where
/// `REFERENCES` clauses are declarative), and the 64 MB page cache.
pub(super) fn open_configured(path: &Path) -> Result<Connection, DbError> {
    let connection = Connection::open(path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    // `journal_mode` answers a row; `query_row` consumes it. The rest
    // are silent settings.
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "NORMAL")?;
    connection.pragma_update(None, "foreign_keys", false)?;
    connection.pragma_update(None, "cache_size", CACHE_SIZE_KIB)?;
    Ok(connection)
}

/// Apply a read-table denylist to one configured reader connection. The
/// production path supplies no denylist; facade integration tests use this
/// seam to prove settled requests never prepare protected raw-fact reads.
fn apply_reader_denylist(
    connection: &mut Connection,
    denied_tables: Option<Arc<HashSet<String>>>,
) -> Result<(), DbError> {
    let Some(denied_tables) = denied_tables else {
        return Ok(());
    };
    connection.authorizer(Some(
        move |context: rusqlite::hooks::AuthContext<'_>| match context.action {
            rusqlite::hooks::AuthAction::Read { table_name, .. }
                if denied_tables.contains(table_name) =>
            {
                rusqlite::hooks::Authorization::Deny
            }
            _ => rusqlite::hooks::Authorization::Allow,
        },
    ))?;
    Ok(())
}

/// The handle over the writer thread and the reader-thread pool.
/// Cloning shares the running threads (a clone is a handle, never a
/// second core); the last handle's drop closes the job channels and
/// joins the threads.
#[derive(Clone)]
pub(super) struct SyncCore {
    inner: Arc<CoreInner>,
}

struct CoreInner {
    /// The writer's job queue: exactly one thread consumes it, so every
    /// submitted write runs serially on the one write connection.
    writer: mpsc::Sender<Job>,
    /// The readers' shared job queue: whichever reader thread is free
    /// dequeues next.
    readers: mpsc::Sender<Job>,
    /// Joined on drop, after the senders close, so a dropped core
    /// leaves no thread holding a connection (Windows cannot delete a
    /// database file that an open connection still maps).
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl std::fmt::Debug for SyncCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SyncCore").finish_non_exhaustive()
    }
}

impl Drop for CoreInner {
    fn drop(&mut self) {
        // The threads exit when their job channel disconnects, and they
        // must be joined so a dropped core leaves no thread holding an
        // open connection. The senders are closed first by swapping in
        // senders whose receivers are already gone.
        let (dead_writer, _) = mpsc::channel();
        let (dead_readers, _) = mpsc::channel();
        drop(std::mem::replace(&mut self.writer, dead_writer));
        drop(std::mem::replace(&mut self.readers, dead_readers));
        let threads = std::mem::take(&mut *self.threads.lock().expect("thread handles"));
        let current = std::thread::current().id();
        for handle in threads {
            // A job closure can own the last handle, putting this drop on a
            // worker thread; joining that thread from itself would deadlock,
            // so it detaches instead (it exits on its own once the closed
            // channel drains).
            if handle.thread().id() == current {
                continue;
            }
            let _ = handle.join();
        }
    }
}

impl SyncCore {
    /// Stand the core up over an already-opened, already-migrated write
    /// connection (the caller migrates first so no reader can observe a
    /// pre-migration database), opening the reader connections here.
    pub(super) fn start(
        path: &Path,
        write_connection: Connection,
        denied_reader_tables: Option<Arc<HashSet<String>>>,
    ) -> Result<SyncCore, DbError> {
        let (writer_tx, writer_rx) = mpsc::channel::<Job>();
        let (reader_tx, reader_rx) = mpsc::channel::<Job>();
        let reader_rx = Arc::new(Mutex::new(reader_rx));
        let writer_rx = Arc::new(Mutex::new(writer_rx));

        let mut threads = Vec::with_capacity(1 + READER_POOL_SIZE);
        threads.push(
            std::thread::Builder::new()
                .name("db-writer".into())
                .spawn(move || run_worker(write_connection, writer_rx))
                .expect("spawn the database writer thread"),
        );
        for index in 0..READER_POOL_SIZE {
            let mut connection = open_configured(path)?;
            apply_reader_denylist(&mut connection, denied_reader_tables.clone())?;
            let queue = reader_rx.clone();
            threads.push(
                std::thread::Builder::new()
                    .name(format!("db-reader-{index}"))
                    .spawn(move || run_worker(connection, queue))
                    .expect("spawn a database reader thread"),
            );
        }
        Ok(SyncCore {
            inner: Arc::new(CoreInner {
                writer: writer_tx,
                readers: reader_tx,
                threads: Mutex::new(threads),
            }),
        })
    }

    /// Run `job` on the writer thread and await its result. Writes
    /// submitted concurrently run serially, in submission order.
    pub(super) async fn write<T, F>(&self, job: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    {
        Self::submit(&self.inner.writer, job).await
    }

    /// Run `job` on whichever reader thread is free and await its result.
    pub(super) async fn read<T, F>(&self, job: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    {
        Self::submit(&self.inner.readers, job).await
    }

    /// The blocking counterpart of [`SyncCore::write`], for plain
    /// producer threads (never call it on an async runtime's worker).
    pub(super) fn write_blocking<T, F>(&self, job: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    {
        Self::submit_blocking(&self.inner.writer, job)
    }

    /// The blocking counterpart of [`SyncCore::read`], for plain
    /// producer threads (never call it on an async runtime's worker).
    pub(super) fn read_blocking<T, F>(&self, job: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    {
        Self::submit_blocking(&self.inner.readers, job)
    }

    async fn submit<T, F>(queue: &mpsc::Sender<Job>, job: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        Self::enqueue(queue, job, reply_tx)?;
        match reply_rx.await {
            Ok(Ok(value)) => value,
            // The job panicked: propagate the payload, exactly as
            // `spawn_blocking` re-raises a panicking task on its awaiter.
            Ok(Err(payload)) => std::panic::resume_unwind(payload),
            // The worker dropped the reply without answering: the core is
            // shutting down underneath the caller.
            Err(_) => Err(DbError::CoreClosed),
        }
    }

    fn submit_blocking<T, F>(queue: &mpsc::Sender<Job>, job: F) -> Result<T, DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    {
        let (reply_tx, reply_rx) = tokio::sync::oneshot::channel();
        Self::enqueue(queue, job, reply_tx)?;
        match reply_rx.blocking_recv() {
            Ok(Ok(value)) => value,
            Ok(Err(payload)) => std::panic::resume_unwind(payload),
            Err(_) => Err(DbError::CoreClosed),
        }
    }

    /// Wrap `job` so its result (or panic payload) travels back over
    /// `reply`, and queue it. A send failure means every worker exited:
    /// the core is closed.
    fn enqueue<T, F>(
        queue: &mpsc::Sender<Job>,
        job: F,
        reply: tokio::sync::oneshot::Sender<Result<Result<T, DbError>, PanicPayload>>,
    ) -> Result<(), DbError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
    {
        let wrapped: Job = Box::new(move |connection| {
            // Time the job and emit an observe-only DB-latency event on a stable
            // target the telemetry metrics layer aggregates into the database-
            // latency histogram. The payload is the elapsed wall time only: the
            // job is an opaque closure at this layer, so no SQL text or bound
            // value can reach the event (a tighter PII boundary than the driver's
            // own per-query events carried). At TRACE, so it stays below the
            // default console/file threshold while the metrics layer sees it
            // through its own target filter. Emitted BEFORE the reply is sent, so
            // the sample is recorded by the time an awaiting caller is released.
            let started = Instant::now();
            let outcome = catch_unwind(AssertUnwindSafe(|| job(connection)));
            tracing::trace!(
                target: "eo::db::query",
                elapsed_secs = started.elapsed().as_secs_f64(),
                "db job complete"
            );
            // A caller that stopped listening is not an error; the job has
            // already run to completion either way.
            let _ = reply.send(outcome);
        });
        queue.send(wrapped).map_err(|_| DbError::CoreClosed)
    }
}

/// A caught panic payload in transit back to the awaiting caller.
type PanicPayload = Box<dyn std::any::Any + Send>;
