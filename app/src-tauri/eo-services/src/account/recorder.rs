//! The account connection: one task owns the session the player signed in
//! with and every reading taken with it; everything else talks to that task
//! through an [`AccountRecorder`] handle.
//!
//! The connection is [`Disconnected`](ConnectionState::Disconnected);
//! [`Unconfirmed`](ConnectionState::Unconfirmed) (a stored session was
//! handed over and the site has not answered for it yet);
//! [`Connected`](ConnectionState::Connected), the only state that holds the
//! session; or [`Expired`](ConnectionState::Expired) (the site refused the
//! session; nothing is held). Nothing reads the site unless it is
//! connected, and only while something needs a reading:
//!
//! - **Connecting** checks the session with one balance read, at most one
//!   check every 10 seconds. Accepted, that balance and one item read make
//!   the connect reading; refused, nothing changes (an unconfirmed stored
//!   session becomes expired). The app starts disconnected: a session is
//!   held in memory only, for the run.
//! - **A tracked session** is read the moment it starts (its baseline) and
//!   every minute while it runs; every fifth minute after three failures
//!   in a row, until one succeeds.
//! - **When it stops**, a closing reading follows 20 seconds later, then
//!   one a minute until a reading comes back identical to the one before
//!   it, at most three in all; then the session's measure is sealed. A new
//!   session starting meanwhile seals the old one with what it has.
//!
//! Every full reading passes through one gate, which keeps any two at
//! least 55 seconds apart: a reading asked for inside that floor is
//! answered by the latest stored reading (taken at its own time), and a
//! closing reading waits for the floor to pass. At most one request is in
//! flight at a time, and a refused session ends the connection quietly as
//! expired.
//!
//! Every request carries what it was started for: the connection it used
//! (each connect, disconnect and expiry starts a new one), the session it
//! reads, and the session its reading is tagged with. A result that comes
//! back for a connection that has ended, or for a session no longer being
//! read, is dropped and stores nothing. A connect carries the epoch it was
//! started in; a disconnect moves the epoch on, so a connect already on its
//! way cannot undo it.
//!
//! The bus delivers session starts and stops on the tracker's thread; the
//! subscriptions only forward them into the task and return. Waiting is on
//! tokio time and timestamps come from the injected clock: a reading is
//! timed when its last request completes.

use std::collections::{BTreeMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::sync::{mpsc, oneshot, watch};
use tokio::task::{JoinError, JoinHandle};
use tokio::time::Instant;

use super::amount::Pec;
use super::reading::{Holding, HoldingKey, Reading, SiteResponse};
use super::sessions::{self, MeasureRead, MeasuredPage, ToSeal};
use super::site::{SessionSecret, SharedSite, SiteFailure, SiteOutcome};
use super::store::{self, Purpose, ReadingHeader};
use super::AccountError;
use crate::bus_events::BusEvent;
use crate::clock::Clock;
use crate::config_service::ConfigService;
use crate::db::Db;
use crate::event_bus::{EventBus, Registration, Topic};
use crate::game_data_store::GameDataStore;
use crate::time::naive_to_epoch;

/// How often a running session is read.
pub const READ_EVERY: Duration = Duration::from_secs(60);
/// How often a running session is read once the site keeps failing.
pub const READ_EVERY_WHILE_FAILING: Duration = Duration::from_secs(5 * 60);
/// The failures in a row after which readings fall back to
/// [`READ_EVERY_WHILE_FAILING`].
pub const FAILURES_BEFORE_BACKING_OFF: u32 = 3;
/// The least time between the starts of two full readings.
pub const READING_FLOOR: Duration = Duration::from_secs(55);
/// The least time between the starts of two checks of a session.
pub const CHECK_FLOOR: Duration = Duration::from_secs(10);
/// How long after a session stops its first closing reading is taken.
pub const CLOSING_AFTER: Duration = Duration::from_secs(20);
/// The most closing readings taken before a session is sealed.
pub const CLOSING_READINGS: u32 = 3;
/// How long a change waits for the changes that follow it, so a burst is
/// announced once.
pub const CHANGE_SETTLES: Duration = Duration::from_millis(250);
/// The longest [`AccountRecorder::shutdown`] waits for the task to end.
pub const SHUTDOWN_WAIT: Duration = Duration::from_millis(1500);

/// Announces that the connection or its readings changed.
pub type ChangedSink = Arc<dyn Fn() + Send + Sync>;

/// The connection's state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionState {
    Disconnected,
    /// A stored session was handed over and the site has not yet answered
    /// for it (it could not be reached). Nothing is read.
    Unconfirmed,
    Connected,
    /// The site refused a session during this run. The player may sign
    /// in again; nothing is read meanwhile.
    Expired,
}

/// How confirming a stored session stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirming {
    /// No attempt has failed to reach the site yet.
    Checking,
    /// The last attempt could not reach the site.
    SiteUnreachable,
}

/// What the connection shows: nothing secret.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionStatus {
    pub state: ConnectionState,
    /// When the current connection was made.
    pub connected_at: Option<f64>,
    /// When the latest stored reading was taken.
    pub last_reading_at: Option<f64>,
    /// The tracked session being read now (running, or waiting for its
    /// closing readings).
    pub reading_session: Option<String>,
    /// The session being tracked, connected or not.
    pub tracked_session: Option<String>,
    /// Whether the player has connected and not disconnected since.
    pub has_connected: bool,
    /// How many readings this connection task has stored.
    pub readings_stored: u64,
    /// Whether the connection has work in flight or waiting: a request,
    /// a reading being stored, a measure being sealed.
    pub working: bool,
    /// How confirming the stored session stands; only while unconfirmed.
    pub confirming: Option<Confirming>,
    /// Whether the sign-in window the app opened is open.
    pub signing_in: bool,
    /// Whether any reading has ever been stored, connected or not.
    pub has_readings: bool,
}

/// What a connect came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectOutcome {
    /// The site accepted the session.
    Connected,
    /// The site refused the session; nothing changed, except that an
    /// unconfirmed stored session is now expired.
    NotAccepted,
    /// The site could not be asked; nothing changed.
    Unreachable(SiteFailure),
    /// A later connect, a disconnect, or the app closing overtook it.
    Interrupted,
}

/// The persisted record that the player has connected: set on a
/// successful connect, cleared on disconnect.
pub trait ConnectionFlag: Send + Sync + 'static {
    fn is_set(&self) -> bool;
    /// Persist the flag. May block on a file write.
    fn set(&self, connected: bool) -> std::io::Result<()>;
}

/// The flag in the settings store.
pub struct SettingsFlag(pub Arc<Mutex<ConfigService>>);

/// The settings key the flag is stored under.
pub const CONNECTED_SETTING: &str = "account_connected";

impl ConnectionFlag for SettingsFlag {
    fn is_set(&self) -> bool {
        self.0
            .lock()
            .map(|service| service.get().account_connected)
            .unwrap_or(false)
    }

    fn set(&self, connected: bool) -> std::io::Result<()> {
        let mut updates = serde_json::Map::new();
        updates.insert(
            CONNECTED_SETTING.to_string(),
            serde_json::Value::Bool(connected),
        );
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("the settings writer is poisoned"))?
            .update(&updates)
            .map(|_| ())
    }
}

/// The flag held in memory only, for tests.
#[cfg(any(test, feature = "test-support"))]
#[derive(Default)]
pub struct MemoryFlag(std::sync::atomic::AtomicBool);

#[cfg(any(test, feature = "test-support"))]
impl MemoryFlag {
    pub fn new(set: bool) -> MemoryFlag {
        MemoryFlag(std::sync::atomic::AtomicBool::new(set))
    }
}

#[cfg(any(test, feature = "test-support"))]
impl ConnectionFlag for MemoryFlag {
    fn is_set(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }

    fn set(&self, connected: bool) -> std::io::Result<()> {
        self.0.store(connected, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

/// What the connection task is built from.
pub struct RecorderParts {
    pub db: Db,
    pub clock: Arc<dyn Clock>,
    pub site: SharedSite,
    pub flag: Arc<dyn ConnectionFlag>,
    pub game_data: Option<Arc<GameDataStore>>,
    pub changed: Option<ChangedSink>,
}

/// Where a session handed to the connection comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectSource {
    /// The player signed in, in the sign-in window.
    SignIn,
    /// The store a session is kept in between runs.
    Stored,
}

/// The epoch a connect was started in; a disconnect moves it on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectEpoch(u64);

/// The handle to the account connection.
pub struct AccountRecorder {
    sender: mpsc::UnboundedSender<Msg>,
    status: watch::Receiver<ConnectionStatus>,
    epoch: Arc<AtomicU64>,
    db: Db,
    clock: Arc<dyn Clock>,
    game_data: Option<Arc<GameDataStore>>,
}

enum Msg {
    SessionStarted(String),
    SessionStopped(String),
    Connect(
        ConnectEpoch,
        ConnectSource,
        SessionSecret,
        oneshot::Sender<ConnectOutcome>,
    ),
    Confirming,
    SigningIn(bool),
    Disconnect(oneshot::Sender<()>),
    Shutdown(oneshot::Sender<()>),
}

impl AccountRecorder {
    /// Start the connection task on `runtime`, subscribed to the session
    /// starts and stops `bus` carries. It starts disconnected, whether or
    /// not the player connected before: nothing holds a session yet.
    pub fn start(bus: &Arc<EventBus>, parts: RecorderParts, runtime: &Handle) -> Arc<Self> {
        let (sender, inbox) = mpsc::unbounded_channel();
        let has_connected = parts.flag.is_set();
        let connection = Connection::Disconnected;
        let initial = ConnectionStatus {
            state: connection.state(),
            connected_at: None,
            last_reading_at: None,
            reading_session: None,
            tracked_session: None,
            has_connected,
            readings_stored: 0,
            working: false,
            confirming: None,
            signing_in: false,
            has_readings: false,
        };
        let (status, status_rx) = watch::channel(initial);
        let epoch = Arc::new(AtomicU64::new(0));
        let subscriptions = subscribe(bus, &sender);
        let actor = Actor {
            bus: bus.clone(),
            subscriptions,
            db: parts.db.clone(),
            clock: parts.clock.clone(),
            site: parts.site,
            flag: parts.flag,
            game_data: parts.game_data.clone(),
            changed: parts.changed,
            status,
            epoch: epoch.clone(),
            connection,
            generation: 0,
            has_connected,
            tracking: Tracking::Idle,
            baseline_for: None,
            work: None,
            next_attempt: None,
            attempt_due: None,
            failures: 0,
            last_full_start: None,
            last_check_start: None,
            confirming: Confirming::Checking,
            signing_in: false,
            latest: None,
            last_reading_at: None,
            pending_seals: VecDeque::new(),
            queued_connect: None,
            flush_at: None,
            readings_stored: 0,
        };
        runtime.spawn(actor.run(inbox));
        Arc::new(AccountRecorder {
            sender,
            status: status_rx,
            epoch,
            db: parts.db,
            clock: parts.clock,
            game_data: parts.game_data,
        })
    }

    /// The connection as it stands.
    pub fn status(&self) -> ConnectionStatus {
        self.status.borrow().clone()
    }

    /// A receiver that sees every change of [`AccountRecorder::status`].
    pub fn watch_status(&self) -> watch::Receiver<ConnectionStatus> {
        self.status.clone()
    }

    /// The epoch a connect started now belongs to.
    pub fn epoch(&self) -> ConnectEpoch {
        ConnectEpoch(self.epoch.load(Ordering::SeqCst))
    }

    /// Connect with the session the player signed in to, or replace the
    /// one held. `epoch` is the one the attempt began in: a connect from
    /// before a disconnect is dropped. The session is checked with one
    /// balance read first.
    pub async fn connect(&self, epoch: ConnectEpoch, session: SessionSecret) -> ConnectOutcome {
        self.connect_from(epoch, ConnectSource::SignIn, session)
            .await
    }

    /// Hand over the session kept in the store. Only an unconfirmed
    /// connection takes it: one the player has meanwhile connected (or
    /// disconnected) is left as it is.
    pub async fn connect_stored(
        &self,
        epoch: ConnectEpoch,
        session: SessionSecret,
    ) -> ConnectOutcome {
        self.connect_from(epoch, ConnectSource::Stored, session)
            .await
    }

    async fn connect_from(
        &self,
        epoch: ConnectEpoch,
        source: ConnectSource,
        session: SessionSecret,
    ) -> ConnectOutcome {
        let (reply, answer) = oneshot::channel();
        if self
            .sender
            .send(Msg::Connect(epoch, source, session, reply))
            .is_err()
        {
            return ConnectOutcome::Interrupted;
        }
        answer.await.unwrap_or(ConnectOutcome::Interrupted)
    }

    /// A stored session is about to be handed over: a disconnected
    /// connection is unconfirmed until the site answers for it.
    pub fn confirming_stored_session(&self) {
        let _ = self.sender.send(Msg::Confirming);
    }

    /// The sign-in window the app opened is open, or has gone (by any
    /// route). Shown in the status, and announced as a change.
    pub fn set_signing_in(&self, open: bool) {
        let _ = self.sender.send(Msg::SigningIn(open));
    }

    /// Drop the session and stop reading. Moves the epoch on first, so no
    /// connect begun before it can undo it.
    pub async fn disconnect(&self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        let (reply, answer) = oneshot::channel();
        if self.sender.send(Msg::Disconnect(reply)).is_ok() {
            let _ = answer.await;
        }
    }

    /// End the connection task, waiting at most [`SHUTDOWN_WAIT`]: a
    /// request in flight is abandoned, never waited for.
    pub async fn shutdown(&self) {
        let (reply, answer) = oneshot::channel();
        if self.sender.send(Msg::Shutdown(reply)).is_ok() {
            let _ = tokio::time::timeout(SHUTDOWN_WAIT, answer).await;
        }
    }

    /// One tracked session's measure: the sealed one cached, or computed
    /// now on a reader (and, when the session has ended and is no longer
    /// being read, sealed and cached in a short write of its own).
    pub async fn session_measure(&self, session_id: String) -> Result<MeasureRead, AccountError> {
        let being_read = self.status().reading_session;
        let game_data = self.game_data.clone();
        let now = naive_to_epoch(self.clock.now());
        let (read, to_seal) = joined(
            on_reader(&self.db, move |conn| {
                let roles = super::roles::KnownRoles::read(conn, game_data.as_deref())?;
                sessions::session_measure(conn, &session_id, &roles, now, being_read.as_deref())
            })
            .await,
        )?;
        if let Some(seal) = to_seal {
            self.cache(vec![seal], now).await?;
        }
        Ok(read)
    }

    /// One page of the sessions with readings, newest first, computed on a
    /// reader; a page whose measures were all cached writes nothing.
    pub async fn measured_sessions(
        &self,
        seek: Option<(f64, String)>,
        limit: Option<i64>,
    ) -> Result<MeasuredPage, AccountError> {
        let being_read = self.status().reading_session;
        let game_data = self.game_data.clone();
        let now = naive_to_epoch(self.clock.now());
        let (page, to_seal) = joined(
            on_reader(&self.db, move |conn| {
                sessions::measured_sessions(
                    conn,
                    seek.as_ref(),
                    limit,
                    game_data.as_deref(),
                    now,
                    being_read.as_deref(),
                )
            })
            .await,
        )?;
        if !to_seal.is_empty() {
            self.cache(to_seal, now).await?;
        }
        Ok(page)
    }

    async fn cache(&self, sealed: Vec<ToSeal>, now: f64) -> Result<(), AccountError> {
        joined(
            on_writer(&self.db, move |conn| {
                sessions::cache_sealed(conn, &sealed, now)
            })
            .await,
        )
        .map(|_| ())
    }
}

/// Run a job on the database's writer, waited for on the blocking pool
/// rather than awaited on a runtime worker. The connection's database work
/// is short and rare; waiting for it there also keeps tokio's paused clock
/// still while it runs, which is what lets the minute cadence be tested
/// to the second.
fn on_writer<T: Send + 'static>(
    db: &Db,
    job: impl FnOnce(&mut rusqlite::Connection) -> Result<T, AccountError> + Send + 'static,
) -> JoinHandle<Result<T, AccountError>> {
    let db = db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_writer_blocking(move |conn| Ok(job(conn)))
            .map_err(AccountError::from)
            .and_then(|done| done)
    })
}

/// [`on_writer`] for a read.
fn on_reader<T: Send + 'static>(
    db: &Db,
    job: impl FnOnce(&mut rusqlite::Connection) -> Result<T, AccountError> + Send + 'static,
) -> JoinHandle<Result<T, AccountError>> {
    let db = db.clone();
    tokio::task::spawn_blocking(move || {
        db.with_reader_blocking(move |conn| Ok(job(conn)))
            .map_err(AccountError::from)
            .and_then(|done| done)
    })
}

/// A database job's result, a job that stopped short being an error.
fn joined<T>(result: Result<Result<T, AccountError>, JoinError>) -> Result<T, AccountError> {
    result.unwrap_or(Err(AccountError::Stored("the database work stopped")))
}

/// Forward session starts and stops into the task. The bus calls these on
/// the publisher's thread: they enqueue and return, never waiting.
fn subscribe(
    bus: &Arc<EventBus>,
    sender: &mpsc::UnboundedSender<Msg>,
) -> Vec<(Topic, Registration)> {
    let started = sender.clone();
    let stopped = sender.clone();
    vec![
        (
            Topic::SessionStarted,
            bus.subscribe(Topic::SessionStarted, move |event| {
                if let BusEvent::SessionStarted(payload) = event {
                    let _ = started.send(Msg::SessionStarted(payload.session_id.clone()));
                }
            }),
        ),
        (
            Topic::SessionStopped,
            bus.subscribe(Topic::SessionStopped, move |event| {
                if let BusEvent::SessionStopped(payload) = event {
                    let _ = stopped.send(Msg::SessionStopped(payload.session_id.clone()));
                }
            }),
        ),
    ]
}

enum Connection {
    Disconnected,
    Unconfirmed,
    Connected {
        session: Arc<SessionSecret>,
        connected_at: f64,
    },
    Expired,
}

impl Connection {
    fn state(&self) -> ConnectionState {
        match self {
            Connection::Disconnected => ConnectionState::Disconnected,
            Connection::Unconfirmed => ConnectionState::Unconfirmed,
            Connection::Connected { .. } => ConnectionState::Connected,
            Connection::Expired => ConnectionState::Expired,
        }
    }
}

/// The tracked session the connection is reading for.
enum Tracking {
    Idle,
    Running(String),
    /// Stopped, and waiting for its closing readings.
    Grace {
        session: String,
        attempts: u32,
        /// When it stopped.
        stopped: Instant,
    },
}

impl Tracking {
    fn session(&self) -> Option<&str> {
        match self {
            Tracking::Idle => None,
            Tracking::Running(session) | Tracking::Grace { session, .. } => Some(session),
        }
    }
}

/// What a full reading is taken for.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ReadingFor {
    Connect,
    /// The first reading of a session, taken because it started.
    Baseline(String),
    Session(String),
    Closing(String),
}

impl ReadingFor {
    fn session(&self) -> Option<&str> {
        match self {
            ReadingFor::Connect => None,
            ReadingFor::Baseline(session)
            | ReadingFor::Session(session)
            | ReadingFor::Closing(session) => Some(session),
        }
    }

    fn purpose(&self) -> Purpose {
        match self {
            ReadingFor::Connect => Purpose::Connect,
            ReadingFor::Baseline(_) => Purpose::Baseline,
            ReadingFor::Session(_) | ReadingFor::Closing(_) => Purpose::Session,
        }
    }
}

/// What a request was started for: its result is used only while the
/// connection and the session it reads still hold, and its reading is
/// tagged with the session decided when it started.
#[derive(Debug, Clone)]
struct Ticket {
    /// The connection the request used.
    generation: u64,
    purpose: ReadingFor,
    /// The session the reading is tagged with.
    tag: Option<String>,
    /// When the attempt fell due, for a scheduled one.
    due: Option<Instant>,
}

/// A connect waiting for its check.
struct PendingConnect {
    source: ConnectSource,
    session: Arc<SessionSecret>,
    reply: oneshot::Sender<ConnectOutcome>,
}

/// The one piece of work in flight.
enum Work {
    Validate {
        connect: PendingConnect,
        /// The session being tracked when the check began: the connect
        /// reading is tagged with it.
        tag: Option<String>,
        started: Instant,
        task: JoinHandle<SiteOutcome<Pec>>,
    },
    Fetch {
        ticket: Ticket,
        task: JoinHandle<SiteOutcome<Reading>>,
    },
    Store {
        ticket: Ticket,
        /// Whether the reading is identical to the one stored before it.
        identical: bool,
        task: JoinHandle<Result<Option<(ReadingHeader, Reading)>, AccountError>>,
    },
    Seal {
        session: String,
        task: JoinHandle<Result<bool, AccountError>>,
    },
}

enum Done {
    Validated(Result<SiteOutcome<Pec>, JoinError>),
    Fetched(Result<SiteOutcome<Reading>, JoinError>),
    Stored(Result<Result<Option<(ReadingHeader, Reading)>, AccountError>, JoinError>),
    Sealed(Result<Result<bool, AccountError>, JoinError>),
}

async fn finish(work: &mut Option<Work>) -> Done {
    match work {
        None => std::future::pending().await,
        Some(Work::Validate { task, .. }) => Done::Validated(task.await),
        Some(Work::Fetch { task, .. }) => Done::Fetched(task.await),
        Some(Work::Store { task, .. }) => Done::Stored(task.await),
        Some(Work::Seal { task, .. }) => Done::Sealed(task.await),
    }
}

async fn at(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

enum Event {
    Message(Option<Msg>),
    Done(Done),
    Attempt,
    CheckFloorPassed,
    Flush,
}

/// Whether a reading was started through the gate.
enum Began {
    Started,
    /// Inside the floor: the latest stored reading stands, and the floor
    /// passes at this instant.
    Floor(Instant),
    NotConnected,
}

/// Whether two readings hold the same: the PED Card and every holding,
/// whenever each was taken.
fn same_contents(a: &Reading, b: &Reading) -> bool {
    a.balance() == b.balance() && a.holdings() == b.holdings()
}

/// The item read of a full reading, beside a balance already read: timed
/// when the items answer.
async fn read_items_beside(
    site: SharedSite,
    session: Arc<SessionSecret>,
    clock: Arc<dyn Clock>,
    balance: Pec,
) -> SiteOutcome<Reading> {
    let holdings: BTreeMap<HoldingKey, Holding> = match site.read_items(&session).await {
        SiteOutcome::Read(holdings) => holdings,
        SiteOutcome::Unauthorised => return SiteOutcome::Unauthorised,
        SiteOutcome::Failed(failure) => return SiteOutcome::Failed(failure),
    };
    let taken_at = naive_to_epoch(clock.now());
    match Reading::new(taken_at, balance, holdings) {
        Ok(reading) => SiteOutcome::Read(reading),
        Err(_) => SiteOutcome::Failed(SiteFailure::Malformed(SiteResponse::Items)),
    }
}

/// One full reading: the balance, then the items.
async fn read_whole(
    site: SharedSite,
    session: Arc<SessionSecret>,
    clock: Arc<dyn Clock>,
) -> SiteOutcome<Reading> {
    match site.read_balance(&session).await {
        SiteOutcome::Read(balance) => read_items_beside(site, session, clock, balance).await,
        SiteOutcome::Unauthorised => SiteOutcome::Unauthorised,
        SiteOutcome::Failed(failure) => SiteOutcome::Failed(failure),
    }
}

struct Actor {
    bus: Arc<EventBus>,
    subscriptions: Vec<(Topic, Registration)>,
    db: Db,
    clock: Arc<dyn Clock>,
    site: SharedSite,
    flag: Arc<dyn ConnectionFlag>,
    game_data: Option<Arc<GameDataStore>>,
    changed: Option<ChangedSink>,
    status: watch::Sender<ConnectionStatus>,
    /// Moved on by every disconnect (in the handle, before the message).
    epoch: Arc<AtomicU64>,
    connection: Connection,
    /// Counts connections: every connect, disconnect and expiry moves it.
    generation: u64,
    has_connected: bool,
    tracking: Tracking,
    /// The session whose baseline is still to be taken.
    baseline_for: Option<String>,
    work: Option<Work>,
    /// When the next scheduled reading falls due.
    next_attempt: Option<Instant>,
    /// A scheduled reading that fell due (at this instant) and waits for
    /// the work in flight.
    attempt_due: Option<Instant>,
    /// Failed readings of the running session in a row.
    failures: u32,
    /// When the latest full reading was started: the floor runs from it.
    last_full_start: Option<Instant>,
    /// When the latest check of a session was started.
    last_check_start: Option<Instant>,
    /// How confirming the stored session stands, while unconfirmed.
    confirming: Confirming,
    signing_in: bool,
    /// The latest stored reading.
    latest: Option<Reading>,
    last_reading_at: Option<f64>,
    pending_seals: VecDeque<String>,
    queued_connect: Option<PendingConnect>,
    flush_at: Option<Instant>,
    readings_stored: u64,
}

impl Actor {
    async fn run(mut self, mut inbox: mpsc::UnboundedReceiver<Msg>) {
        self.seed().await;
        self.publish_status();
        loop {
            let attempt_at = if self.attempt_due.is_some() {
                None
            } else {
                self.next_attempt
            };
            let check_at = self
                .queued_connect
                .as_ref()
                .and(self.check_floor_end())
                .filter(|end| *end > Instant::now());
            let flush_at = self.flush_at;
            // Messages first: a disconnect that arrives with a result is
            // applied before the result is looked at.
            let event = tokio::select! {
                biased;
                message = inbox.recv() => Event::Message(message),
                done = finish(&mut self.work) => Event::Done(done),
                () = at(attempt_at) => Event::Attempt,
                () = at(check_at) => Event::CheckFloorPassed,
                () = at(flush_at) => Event::Flush,
            };
            match event {
                Event::Message(None) => break,
                Event::Message(Some(Msg::Shutdown(reply))) => {
                    self.stop();
                    let _ = reply.send(());
                    return;
                }
                Event::Message(Some(message)) => self.on_message(message).await,
                Event::Done(done) => self.on_done(done).await,
                Event::Attempt => self.attempt_due = self.next_attempt.take(),
                Event::CheckFloorPassed => {}
                Event::Flush => {
                    self.flush_at = None;
                    if let Some(changed) = &self.changed {
                        changed();
                    }
                }
            }
            self.advance();
            self.publish_status();
        }
        self.stop();
    }

    /// The state the last run left: the latest stored reading (and the
    /// floor it still holds), and a session still marked running.
    async fn seed(&mut self) {
        let seeded = joined(
            on_reader(&self.db, |conn| {
                let latest = match store::last_stored(conn)? {
                    Some(header) => store::load_reading(conn, header.id)?,
                    None => None,
                };
                Ok((latest, sessions::active_session(conn)?))
            })
            .await,
        );
        let (latest, active) = match seeded {
            Ok(seeded) => seeded,
            Err(error) => {
                tracing::error!(target: "eo::account", %error, "the account connection could not read its last state");
                return;
            }
        };
        if let Some(stored) = latest {
            let taken_at = stored.reading.taken_at();
            let age = self.now_epoch() - taken_at;
            if (0.0..READING_FLOOR.as_secs_f64()).contains(&age) {
                self.last_full_start = Instant::now().checked_sub(Duration::from_secs_f64(age));
            }
            self.last_reading_at = Some(taken_at);
            self.latest = Some(stored.reading);
        }
        if let Some(session) = active {
            self.tracking = Tracking::Running(session);
        }
    }

    fn now_epoch(&self) -> f64 {
        naive_to_epoch(self.clock.now())
    }

    fn is_connected(&self) -> bool {
        matches!(self.connection, Connection::Connected { .. })
    }

    fn mark_changed(&mut self) {
        if self.flush_at.is_none() {
            self.flush_at = Some(Instant::now() + CHANGE_SETTLES);
        }
    }

    fn publish_status(&self) {
        let connected_at = match &self.connection {
            Connection::Connected { connected_at, .. } => Some(*connected_at),
            _ => None,
        };
        let status = ConnectionStatus {
            state: self.connection.state(),
            connected_at,
            last_reading_at: self.last_reading_at,
            reading_session: self
                .is_connected()
                .then(|| self.tracking.session().map(str::to_string))
                .flatten(),
            tracked_session: match &self.tracking {
                Tracking::Running(session) => Some(session.clone()),
                Tracking::Idle | Tracking::Grace { .. } => None,
            },
            has_connected: self.has_connected,
            readings_stored: self.readings_stored,
            working: self.work.is_some()
                || self.queued_connect.is_some()
                || !self.pending_seals.is_empty()
                || self.attempt_due.is_some(),
            confirming: matches!(self.connection, Connection::Unconfirmed)
                .then_some(self.confirming),
            signing_in: self.signing_in,
            has_readings: self.latest.is_some(),
        };
        self.status.send_if_modified(|current| {
            if *current == status {
                false
            } else {
                *current = status;
                true
            }
        });
    }

    async fn on_message(&mut self, message: Msg) {
        match message {
            Msg::SessionStarted(session) => self.session_started(session),
            Msg::SessionStopped(session) => self.session_stopped(session),
            Msg::Connect(epoch, source, session, reply) => {
                self.connect(epoch, source, session, reply)
            }
            Msg::Confirming => {
                if matches!(self.connection, Connection::Disconnected) && self.has_connected {
                    self.connection = Connection::Unconfirmed;
                    self.confirming = Confirming::Checking;
                    self.mark_changed();
                }
            }
            Msg::SigningIn(open) => {
                if self.signing_in != open {
                    self.signing_in = open;
                    self.mark_changed();
                }
            }
            Msg::Disconnect(reply) => {
                self.disconnect().await;
                let _ = reply.send(());
            }
            // Handled by the loop itself.
            Msg::Shutdown(_) => {}
        }
    }

    fn session_started(&mut self, session: String) {
        match &self.tracking {
            Tracking::Running(running) if *running == session => return,
            // The one before it ends with what it has.
            Tracking::Running(earlier)
            | Tracking::Grace {
                session: earlier, ..
            } => {
                self.pending_seals.push_back(earlier.clone());
            }
            Tracking::Idle => {}
        }
        self.tracking = Tracking::Running(session.clone());
        self.failures = 0;
        self.next_attempt = None;
        // The baseline, at once. Only a connection that was there when the
        // session started takes one.
        if self.is_connected() {
            self.baseline_for = Some(session);
            self.attempt_due = Some(Instant::now());
        } else {
            self.baseline_for = None;
            self.attempt_due = None;
        }
        self.mark_changed();
    }

    fn session_stopped(&mut self, session: String) {
        if !matches!(&self.tracking, Tracking::Running(running) if *running == session) {
            return;
        }
        self.attempt_due = None;
        self.baseline_for = None;
        let stopped = Instant::now();
        if self.is_connected() {
            self.next_attempt = Some(self.closing_from(stopped));
            self.tracking = Tracking::Grace {
                session,
                attempts: 0,
                stopped,
            };
        } else if self.connecting() {
            // A connect is being checked: the session waits for its
            // answer, and is read to its close exactly as it would be had
            // the answer come first.
            self.next_attempt = None;
            self.tracking = Tracking::Grace {
                session,
                attempts: 0,
                stopped,
            };
        } else {
            self.next_attempt = None;
            self.pending_seals.push_back(session);
            self.tracking = Tracking::Idle;
        }
        self.mark_changed();
    }

    /// When the first closing reading of a session stopped at `stopped`
    /// falls due: 20 seconds on, and never inside the floor.
    fn closing_from(&self, stopped: Instant) -> Instant {
        let closing = stopped + CLOSING_AFTER;
        closing.max(self.floor_end().unwrap_or(closing))
    }

    /// Whether a connect is being checked or waits to be.
    fn connecting(&self) -> bool {
        self.queued_connect.is_some() || matches!(self.work, Some(Work::Validate { .. }))
    }

    /// A connect came to nothing. A session that stopped while it was
    /// being checked, with nothing connected to read it, is sealed with
    /// what it has.
    fn connect_failed(&mut self) {
        if self.is_connected() || self.connecting() {
            return;
        }
        if let Tracking::Grace { session, .. } = &self.tracking {
            self.pending_seals.push_back(session.clone());
            self.tracking = Tracking::Idle;
            self.next_attempt = None;
            self.mark_changed();
        }
    }

    /// Queue a connect behind the work in flight. A connect begun before
    /// the latest disconnect is dropped; a newer sign-in overtakes an
    /// earlier one still being checked; a reading in flight with the
    /// session held is let finish first (it is the one request allowed in
    /// flight).
    fn connect(
        &mut self,
        epoch: ConnectEpoch,
        source: ConnectSource,
        session: SessionSecret,
        reply: oneshot::Sender<ConnectOutcome>,
    ) {
        let stale = epoch.0 != self.epoch.load(Ordering::SeqCst);
        // The stored session is only ever the unconfirmed one's.
        let unwanted =
            source == ConnectSource::Stored && !matches!(self.connection, Connection::Unconfirmed);
        if stale || unwanted {
            let _ = reply.send(ConnectOutcome::Interrupted);
            return;
        }
        if let Some(earlier) = self.queued_connect.take() {
            let _ = earlier.reply.send(ConnectOutcome::Interrupted);
        }
        if matches!(self.work, Some(Work::Validate { .. })) {
            self.abandon_request();
        }
        self.queued_connect = Some(PendingConnect {
            source,
            session: Arc::new(session),
            reply,
        });
    }

    /// Abandon a request in flight. A check is dropped outright; a reading
    /// is aborted but stays in hand, so an answer that had already come
    /// back is still met by the identity check and dropped there.
    /// Storing and sealing are left to finish.
    fn abandon_request(&mut self) {
        match self.work.take() {
            Some(Work::Validate { connect, task, .. }) => {
                task.abort();
                let _ = connect.reply.send(ConnectOutcome::Interrupted);
            }
            Some(Work::Fetch { ticket, task }) => {
                task.abort();
                self.work = Some(Work::Fetch { ticket, task });
            }
            other => self.work = other,
        }
    }

    async fn disconnect(&mut self) {
        if let Some(earlier) = self.queued_connect.take() {
            let _ = earlier.reply.send(ConnectOutcome::Interrupted);
        }
        self.abandon_request();
        self.end_connection(Connection::Disconnected);
        if self.has_connected {
            self.has_connected = false;
            self.write_flag(false).await;
        }
    }

    /// End the connection as `to`: the session is dropped, nothing more is
    /// read, and a session waiting for its closing readings is sealed with
    /// what it has.
    fn end_connection(&mut self, to: Connection) {
        self.generation += 1;
        self.connection = to;
        self.next_attempt = None;
        self.attempt_due = None;
        self.baseline_for = None;
        self.failures = 0;
        if let Tracking::Grace { session, .. } = &self.tracking {
            self.pending_seals.push_back(session.clone());
            self.tracking = Tracking::Idle;
        }
        self.mark_changed();
    }

    async fn write_flag(&self, connected: bool) {
        let flag = self.flag.clone();
        let written = tokio::task::spawn_blocking(move || flag.set(connected)).await;
        if !matches!(written, Ok(Ok(()))) {
            tracing::error!(
                target: "eo::account",
                connected,
                "the account connection could not record whether the player is connected"
            );
        }
    }

    fn floor_end(&self) -> Option<Instant> {
        self.last_full_start.map(|start| start + READING_FLOOR)
    }

    fn check_floor_end(&self) -> Option<Instant> {
        self.last_check_start.map(|start| start + CHECK_FLOOR)
    }

    /// The one place a scheduled full reading starts: refused while not
    /// connected, and inside the floor answered by the latest stored
    /// reading. (A connect reading passes the same floor where its check
    /// starts.)
    fn begin_reading(&mut self, purpose: ReadingFor, due: Option<Instant>) -> Began {
        let now = Instant::now();
        if let Some(end) = self.floor_end().filter(|end| now < *end) {
            return Began::Floor(end);
        }
        let Connection::Connected { session, .. } = &self.connection else {
            return Began::NotConnected;
        };
        debug_assert!(self.work.is_none(), "one request at a time");
        self.last_full_start = Some(now);
        let task = tokio::spawn(read_whole(
            self.site.clone(),
            session.clone(),
            self.clock.clone(),
        ));
        let tag = purpose.session().map(str::to_string);
        self.work = Some(Work::Fetch {
            ticket: Ticket {
                generation: self.generation,
                purpose,
                tag,
                due,
            },
            task,
        });
        Began::Started
    }

    /// Start the next piece of work, if none is in flight.
    fn advance(&mut self) {
        if self.work.is_some() {
            return;
        }
        if self.queued_connect.is_some() {
            let now = Instant::now();
            if self.check_floor_end().is_some_and(|end| now < end) {
                // Waits for the check floor; the loop wakes for it.
            } else if let Some(connect) = self.queued_connect.take() {
                self.last_check_start = Some(now);
                let site = self.site.clone();
                let checking = connect.session.clone();
                let task = tokio::spawn(async move { site.read_balance(&checking).await });
                self.work = Some(Work::Validate {
                    connect,
                    tag: self.tracking.session().map(str::to_string),
                    started: now,
                    task,
                });
                return;
            }
        }
        if let Some(session) = self.pending_seals.pop_front() {
            let db = self.db.clone();
            let game_data = self.game_data.clone();
            let now = self.now_epoch();
            let sealing = session.clone();
            let task = tokio::task::spawn_blocking(move || {
                // Computed on a reader; only the result is written.
                let seal = db
                    .with_reader_blocking(move |conn| {
                        Ok(sessions::measure_to_seal(
                            conn,
                            &sealing,
                            game_data.as_deref(),
                            now,
                        ))
                    })
                    .map_err(AccountError::from)
                    .and_then(|seal| seal)?;
                let Some(seal) = seal else {
                    return Ok(false);
                };
                let written = db
                    .with_writer_blocking(move |conn| {
                        Ok(sessions::cache_sealed(conn, &[seal], now))
                    })
                    .map_err(AccountError::from)
                    .and_then(|cached| cached)?;
                Ok(written > 0)
            });
            self.work = Some(Work::Seal { session, task });
            return;
        }
        if let Some(due) = self.attempt_due.take() {
            self.attempt(due);
        }
    }

    /// The interval to the next minute's reading.
    fn read_every(&self) -> Duration {
        if self.failures >= FAILURES_BEFORE_BACKING_OFF {
            READ_EVERY_WHILE_FAILING
        } else {
            READ_EVERY
        }
    }

    /// A scheduled reading that fell due at `due`: the session's baseline
    /// or minute, or a closing reading. The next one is due an interval
    /// after this one was, however long this one waited for the work
    /// before it.
    fn attempt(&mut self, due: Instant) {
        if !self.is_connected() {
            return;
        }
        match &self.tracking {
            Tracking::Idle => {}
            Tracking::Running(session) => {
                let purpose = if self.baseline_for.as_deref() == Some(session.as_str()) {
                    ReadingFor::Baseline(session.clone())
                } else {
                    ReadingFor::Session(session.clone())
                };
                self.baseline_for = None;
                self.next_attempt = Some(due + self.read_every());
                // Inside the floor the latest stored reading stands for
                // this minute.
                let _ = self.begin_reading(purpose, Some(due));
            }
            Tracking::Grace { session, .. } => {
                let purpose = ReadingFor::Closing(session.clone());
                // The next closing reading, unless this one ends the wait.
                self.next_attempt = Some(match self.begin_reading(purpose, Some(due)) {
                    Began::Floor(end) => end,
                    Began::Started | Began::NotConnected => due + READ_EVERY,
                });
            }
        }
    }

    async fn on_done(&mut self, done: Done) {
        let Some(work) = self.work.take() else {
            return;
        };
        match (work, done) {
            (
                Work::Validate {
                    connect,
                    tag,
                    started,
                    ..
                },
                Done::Validated(answer),
            ) => {
                self.validated(connect, tag, started, answer).await;
            }
            (Work::Fetch { ticket, .. }, Done::Fetched(answer)) => self.fetched(ticket, answer),
            (
                Work::Store {
                    ticket, identical, ..
                },
                Done::Stored(stored),
            ) => self.stored(ticket, identical, stored),
            (Work::Seal { session, .. }, Done::Sealed(sealed)) => match sealed {
                Ok(Ok(true)) => self.mark_changed(),
                Ok(Ok(false)) => {}
                Ok(Err(error)) => {
                    tracing::error!(target: "eo::account", %error, session_id = %session, "a session's measure could not be sealed");
                }
                Err(error) => {
                    tracing::error!(target: "eo::account", %error, "sealing a session's measure stopped");
                }
            },
            // `finish` answers for the work it was given.
            (work, _) => self.work = Some(work),
        }
    }

    async fn validated(
        &mut self,
        connect: PendingConnect,
        tag: Option<String>,
        started: Instant,
        answer: Result<SiteOutcome<Pec>, JoinError>,
    ) {
        let PendingConnect {
            source,
            session,
            reply,
        } = connect;
        let balance = match answer {
            Ok(SiteOutcome::Read(balance)) => balance,
            Ok(SiteOutcome::Unauthorised) => {
                // An unconfirmed stored session the site refuses is over;
                // a refused sign-in leaves everything as it was.
                if source == ConnectSource::Stored
                    && matches!(self.connection, Connection::Unconfirmed)
                {
                    self.connection = Connection::Expired;
                    self.mark_changed();
                }
                self.connect_failed();
                let _ = reply.send(ConnectOutcome::NotAccepted);
                return;
            }
            Ok(SiteOutcome::Failed(failure)) => {
                if source == ConnectSource::Stored
                    && matches!(self.connection, Connection::Unconfirmed)
                    && self.confirming != Confirming::SiteUnreachable
                {
                    self.confirming = Confirming::SiteUnreachable;
                    self.mark_changed();
                }
                self.connect_failed();
                let _ = reply.send(ConnectOutcome::Unreachable(failure));
                return;
            }
            Err(_) => {
                self.connect_failed();
                let _ = reply.send(ConnectOutcome::Interrupted);
                return;
            }
        };
        self.generation += 1;
        self.failures = 0;
        self.connection = Connection::Connected {
            session: session.clone(),
            connected_at: self.now_epoch(),
        };
        // The check's balance is the connect reading's: one item read
        // completes it, unless a reading started inside the floor stands.
        if self.floor_end().is_none_or(|end| started >= end) {
            self.last_full_start = Some(started);
            let task = tokio::spawn(read_items_beside(
                self.site.clone(),
                session,
                self.clock.clone(),
                balance,
            ));
            self.work = Some(Work::Fetch {
                ticket: Ticket {
                    generation: self.generation,
                    purpose: ReadingFor::Connect,
                    tag,
                    due: None,
                },
                task,
            });
        }
        // A session already running is read from now on; its first
        // reading is the connect reading. One that stopped while the check
        // was out is read to its close.
        match &self.tracking {
            Tracking::Running(_) => {
                self.next_attempt = None;
                self.attempt_due = Some(Instant::now());
            }
            Tracking::Grace { stopped, .. }
                if self.next_attempt.is_none() && self.attempt_due.is_none() =>
            {
                self.next_attempt = Some(self.closing_from(*stopped));
            }
            Tracking::Grace { .. } | Tracking::Idle => {}
        }
        self.mark_changed();
        if !self.has_connected {
            self.has_connected = true;
            self.write_flag(true).await;
        }
        tracing::info!(target: "eo::account", "connected to the account site");
        let _ = reply.send(ConnectOutcome::Connected);
    }

    fn fetched(&mut self, ticket: Ticket, answer: Result<SiteOutcome<Reading>, JoinError>) {
        let current = ticket.generation == self.generation && self.is_connected();
        // A reading for a session still being read, or stopped and not
        // yet sealed, is kept; one for a session already sealed is not.
        let for_this = match ticket.purpose.session() {
            None => true,
            Some(session) => {
                self.tracking.session() == Some(session)
                    || self.pending_seals.iter().any(|pending| pending == session)
            }
        };
        let closing = matches!(ticket.purpose, ReadingFor::Closing(_));
        let minute = matches!(
            ticket.purpose,
            ReadingFor::Baseline(_) | ReadingFor::Session(_)
        );
        let outcome = match answer {
            Ok(outcome) => outcome,
            Err(_) => {
                if closing && current && for_this {
                    self.closing_done(None);
                }
                return;
            }
        };
        if !(current && for_this) {
            tracing::debug!(target: "eo::account", "a reading for an ended connection or session was dropped");
            return;
        }
        match outcome {
            SiteOutcome::Read(reading) => {
                if minute {
                    // A success ends any backing off: the next minute is
                    // a minute after this one fell due.
                    if self.failures >= FAILURES_BEFORE_BACKING_OFF {
                        if let (Some(due), Tracking::Running(_)) = (ticket.due, &self.tracking) {
                            self.next_attempt = Some(due + READ_EVERY);
                        }
                    }
                    self.failures = 0;
                }
                let identical = self
                    .latest
                    .as_ref()
                    .is_some_and(|latest| same_contents(latest, &reading));
                let purpose = ticket.purpose.purpose();
                let tag = ticket.tag.clone();
                let task = on_writer(&self.db, move |conn| {
                    store_reading(conn, reading, purpose, tag)
                });
                self.work = Some(Work::Store {
                    ticket,
                    identical,
                    task,
                });
            }
            SiteOutcome::Unauthorised => {
                tracing::info!(target: "eo::account", "the account site no longer accepts the session");
                self.end_connection(Connection::Expired);
            }
            SiteOutcome::Failed(failure) => {
                tracing::warn!(target: "eo::account", ?failure, "an account reading failed; the next falls due as usual");
                if closing {
                    self.closing_done(None);
                }
                if minute {
                    self.failures += 1;
                    if self.failures >= FAILURES_BEFORE_BACKING_OFF {
                        if let (Some(due), Tracking::Running(_)) = (ticket.due, &self.tracking) {
                            self.next_attempt = Some(due + READ_EVERY_WHILE_FAILING);
                        }
                    }
                }
            }
        }
    }

    fn stored(
        &mut self,
        ticket: Ticket,
        identical: bool,
        stored: Result<Result<Option<(ReadingHeader, Reading)>, AccountError>, JoinError>,
    ) {
        let closing = matches!(ticket.purpose, ReadingFor::Closing(_));
        match stored {
            Ok(Ok(None)) => {
                tracing::debug!(target: "eo::account", "a reading for a sealed or deleted session was not kept");
                if closing {
                    self.closing_done(None);
                }
            }
            Ok(Ok(Some((header, reading)))) => {
                self.last_reading_at = Some(header.taken_at);
                self.latest = Some(reading);
                self.readings_stored += 1;
                self.mark_changed();
                if closing {
                    self.closing_done(Some(identical));
                }
            }
            Ok(Err(error)) => {
                tracing::error!(target: "eo::account", %error, "an account reading could not be stored");
                if closing {
                    self.closing_done(None);
                }
            }
            Err(error) => {
                tracing::error!(target: "eo::account", %error, "storing an account reading stopped");
                if closing {
                    self.closing_done(None);
                }
            }
        }
    }

    /// One closing reading done: identical to the one before it
    /// (`Some(true)`), different, or failed (`None`). Unless it ends the
    /// wait, the next closing reading stays due as `attempt` set it.
    fn closing_done(&mut self, identical: Option<bool>) {
        let Tracking::Grace {
            session, attempts, ..
        } = &mut self.tracking
        else {
            return;
        };
        *attempts += 1;
        if identical == Some(true) || *attempts >= CLOSING_READINGS {
            self.pending_seals.push_back(session.clone());
            self.tracking = Tracking::Idle;
            self.next_attempt = None;
            self.attempt_due = None;
            self.mark_changed();
        }
    }

    fn stop(&mut self) {
        if let Some(connect) = self.queued_connect.take() {
            let _ = connect.reply.send(ConnectOutcome::Interrupted);
        }
        self.abandon_request();
        self.connection = Connection::Disconnected;
        for (topic, registration) in self.subscriptions.drain(..) {
            self.bus.unsubscribe(topic, registration);
        }
    }
}

/// Store a reading, tagged with its session only while that session
/// exists (one deleted meanwhile is not named).
fn store_reading(
    conn: &mut rusqlite::Connection,
    reading: Reading,
    purpose: Purpose,
    session: Option<String>,
) -> Result<Option<(ReadingHeader, Reading)>, AccountError> {
    let tag = match session {
        Some(session) => {
            let gone = sessions::tracked_session(conn, &session)?.is_none();
            let sealed = sessions::is_sealed(conn, &session)?;
            match purpose {
                // A reading taken for a session already sealed or deleted
                // has nothing left to measure: it is not kept.
                Purpose::Baseline | Purpose::Session if gone || sealed => return Ok(None),
                // A connect reading stands on its own.
                _ if gone || sealed => None,
                _ => Some(session),
            }
        }
        None => None,
    };
    let header = store::record_reading(conn, &reading, purpose, tag.as_deref())?;
    Ok(Some((header, reading)))
}
