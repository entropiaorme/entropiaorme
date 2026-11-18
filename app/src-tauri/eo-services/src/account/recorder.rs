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
//! - **A strongbox run** (outside a tracked session) is read when it
//!   begins, every minute while it is open, and once more when the player
//!   says it is done; then reading stops. A run left open closes after 30
//!   minutes, and when the view that opened it goes away (see [`run`]).
//! - **While connected and nothing else is being read**, what happens
//!   depends on the [`ReadingMode`] the connection was built with. Built
//!   with [`ReadingMode::Always`] (the installed app), it reads the account
//!   in full every five minutes, and a minute after any reading that
//!   differs from the one stored before it, until two in a row agree: the
//!   inventory is followed between sessions, and the site keeps the
//!   session. Built with [`ReadingMode::OnDemand`] (a development build),
//!   it only asks the site for the PED Card balance whenever five minutes
//!   pass with no request to it, so the site does not end the session for
//!   being idle. A tracked session or a strongbox run takes over from
//!   either the moment it starts, and hands back when it is done. A refusal
//!   ends the connection as expired, the way a refused reading does, and is
//!   logged with how long the session lasted.
//!
//! Every reading stored is then examined for what it proposes (see
//! [`super::proposals`]), and the readings a previous run left unexamined
//! are examined once the task starts. A failed examination is logged and
//! never fails the reading. Then what each step did to the stock is
//! recorded (see [`super::follow`]), whenever no strongbox run is open
//! (the run's own flow records what it reads); a failure there is logged
//! and tried again after the next reading.
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

use super::adoption::SessionStore;
use super::amount::Pec;
use super::costs::{self, SessionCosts};
use super::follow::{FollowFailure, FollowPass};
use super::minutes::{self, SessionMinutes};
use super::placement::{self, Placement, Placements};
use super::proposals::{self, Decided, Examined, Proposal, ProposalId, ProposalKind};
use super::reading::{Holding, HoldingKey, Reading, SiteResponse};
use super::sessions::{self, MeasureRead, MeasuredPage, ToSeal};
use super::site::{SessionSecret, SharedSite, SiteFailure, SiteOutcome};
use super::store::{self, Purpose, ReadingHeader, ReadingId};
use super::AccountError;
use crate::analytics::AnalyticsService;
use crate::bus_events::BusEvent;
use crate::chatlog_span::ChatLogSource;
use crate::clock::Clock;
use crate::config_service::ConfigService;
use crate::db::Db;
use crate::event_bus::{EventBus, Registration, Topic};
use crate::game_data_store::GameDataStore;
use crate::time::naive_to_epoch;

mod run;

use run::{Run, Waiter};
pub use run::{RunReading, RUN_OPEN_FOR, RUN_READ_EVERY, RUN_WAIT};

/// How often a running session is read.
pub const READ_EVERY: Duration = Duration::from_secs(60);
/// How often a running session is read once the site keeps failing.
pub const READ_EVERY_WHILE_FAILING: Duration = Duration::from_secs(5 * 60);
/// The failures in a row after which readings fall back to
/// [`READ_EVERY_WHILE_FAILING`].
pub const FAILURES_BEFORE_BACKING_OFF: u32 = 3;
/// The least time between the starts of two full readings.
pub const READING_FLOOR: Duration = Duration::from_secs(55);
/// How long the site may go without a request from a connection before
/// it is asked for the balance, so it keeps the session.
pub const KEEP_ALIVE_EVERY: Duration = Duration::from_secs(5 * 60);
/// How many passes in a row may fail on the same step before it is passed
/// over with nothing recorded (see [`super::follow`]).
pub const FOLLOW_FAILURES_BEFORE_PASSING: u32 = 3;
/// How often the account is read while signed in and nothing else is
/// being read, once two readings in a row agree (see [`ReadingMode`]).
pub const IDLE_READ_EVERY: Duration = Duration::from_secs(5 * 60);
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

/// Whether the account is read while signed in and nothing else asks for
/// a reading. Decided by the build that runs the connection, never by a
/// setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadingMode {
    /// Only a tracked session or a strongbox run is read; in between, the
    /// balance alone keeps the session. How a development build reads, so
    /// that one running beside the installed app neither reads the account
    /// twice nor records it into its own database.
    OnDemand,
    /// Read in full every [`IDLE_READ_EVERY`] while nothing else is read,
    /// and every [`READ_EVERY`] while the readings keep changing.
    Always,
}

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
    /// a reading being stored or examined, a measure being sealed, or the
    /// task's own start.
    pub working: bool,
    /// How confirming the stored session stands; only while unconfirmed.
    pub confirming: Option<Confirming>,
    /// Whether the sign-in window the app opened is open.
    pub signing_in: bool,
    /// Whether any reading has ever been stored, connected or not.
    pub has_readings: bool,
    /// When the readings that keep failing began to fail, while connected:
    /// the first of the failures in a row. Cleared by the next reading
    /// that succeeds.
    pub failing_since: Option<f64>,
    /// How many readings in a row have failed while connected.
    pub failures_in_a_row: u32,
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
    /// The game's chat log, read after the fact to show clean the gap a
    /// session's readings leave at an end; without it no gap is billed.
    pub chat_log: Option<ChatLogSource>,
    pub changed: Option<ChangedSink>,
    /// Whether the account is read while nothing else asks for a reading.
    pub mode: ReadingMode,
    /// Where a session the player signed in with is kept for the next
    /// launch: written once the site accepts it.
    pub store: Arc<dyn SessionStore>,
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

/// What recording an arrival as stock of unknown origin came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArrivalRecording {
    /// Recorded, with the record's own id; the proposal is linked to it.
    Recorded { record_id: String },
    /// Recorded, but the proposal could not be linked to it: it stays
    /// claimed, shown as possibly recorded already.
    Unlinked { record_id: String },
    /// No open arrival proposal has that id.
    NotOpen,
    /// It cannot be claimed: why, in words for the player.
    Refused(&'static str),
    /// The stock write was refused; the claim is released.
    WriteRefused,
}

/// The handle to the account connection.
pub struct AccountRecorder {
    sender: mpsc::UnboundedSender<Msg>,
    status: watch::Receiver<ConnectionStatus>,
    epoch: Arc<AtomicU64>,
    db: Db,
    clock: Arc<dyn Clock>,
    game_data: Option<Arc<GameDataStore>>,
    chat_log: Option<ChatLogSource>,
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
    /// A strongbox run begins, or is done: answered once its reading is in.
    RunBegin(oneshot::Sender<RunReading>),
    RunDone(oneshot::Sender<RunReading>),
    /// The run's view went away: reading for it stops.
    RunCancel,
    /// A proposal was decided: announce it.
    ProposalsChanged,
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
            // Its start is ahead of it: reading the last state, and
            // examining what a previous run left unexamined.
            working: true,
            confirming: None,
            signing_in: false,
            has_readings: false,
            failing_since: None,
            failures_in_a_row: 0,
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
            chat_log: parts.chat_log.clone(),
            changed: parts.changed,
            mode: parts.mode,
            store: parts.store,
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
            last_contact: None,
            last_answer: None,
            keep_alive_due: false,
            idle_due: false,
            idle_failures: 0,
            failing_since: None,
            failures_in_a_row: 0,
            connect_reading_at: None,
            connect_reading_due: false,
            connected_since: None,
            floor_from_idle: false,
            confirming: Confirming::Checking,
            signing_in: false,
            latest: None,
            latest_changed: false,
            latest_tagged: false,
            last_reading_at: None,
            pending_seals: VecDeque::new(),
            queued_connect: None,
            flush_at: None,
            readings_stored: 0,
            latest_id: None,
            catch_up: true,
            run: None,
            run_next: None,
            run_due: false,
            waiters: Vec::new(),
            heal_due: false,
            live_due: None,
            // Steps a previous run left are followed once the task starts.
            follow_due: true,
            follow_failing: None,
            follow_pass_over: None,
        };
        runtime.spawn(actor.run(inbox));
        Arc::new(AccountRecorder {
            sender,
            status: status_rx,
            epoch,
            db: parts.db,
            clock: parts.clock,
            game_data: parts.game_data,
            chat_log: parts.chat_log,
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
        let chat_log = self.chat_log.clone();
        let now = naive_to_epoch(self.clock.now());
        let (read, to_seal) = joined(
            on_reader(&self.db, move |conn| {
                let roles = super::roles::KnownRoles::read(conn, game_data.as_deref())?;
                sessions::session_measure(
                    conn,
                    &session_id,
                    &roles,
                    chat_log.as_ref(),
                    now,
                    being_read.as_deref(),
                )
            })
            .await,
        )?;
        if let Some(seal) = to_seal {
            self.cache(vec![seal], now).await?;
        }
        Ok(read)
    }

    /// One tracked session's costs as its readings measure them: the stored
    /// ones when current, or computed now on a reader, and what that leaves
    /// to store (a seal, or costs derived again) written in a short guarded
    /// write of its own. `None` for a session that does not exist.
    pub async fn session_costs(
        &self,
        session_id: String,
    ) -> Result<Option<SessionCosts>, AccountError> {
        let being_read = self.status().reading_session;
        let game_data = self.game_data.clone();
        let chat_log = self.chat_log.clone();
        let now = naive_to_epoch(self.clock.now());
        let (costs, write) = joined(
            on_reader(&self.db, move |conn| {
                let roles = super::roles::KnownRoles::read(conn, game_data.as_deref())?;
                costs::read_session_costs(
                    conn,
                    &session_id,
                    &roles,
                    chat_log.as_ref(),
                    now,
                    being_read.as_deref(),
                )
            })
            .await,
        )?;
        if let Some(write) = write {
            joined(on_writer(&self.db, move |conn| costs::write_costs(conn, write, now)).await)?;
        }
        Ok(costs)
    }

    /// Rebuild every session's costs that a change to what places their
    /// items (the library, the protection sets, a set's pieces) left stale,
    /// and every figure derived from them, leaving alone the session the
    /// connection is still reading. A failure is logged, never raised: the
    /// next read of a session rebuilds it anyway.
    pub async fn heal_costs(&self) {
        let being_read = self.status().reading_session;
        let db = self.db.clone();
        let game_data = self.game_data.clone();
        let chat_log = self.chat_log.clone();
        let now = naive_to_epoch(self.clock.now());
        let _ = tokio::task::spawn_blocking(move || {
            heal_costs(&db, game_data, chat_log, now, being_read)
        })
        .await;
    }

    /// Read a strongbox run whole, from the reading before its boxes to
    /// its latest (see [`proposals::read_run_whole`]), and announce any
    /// proposal it stored.
    pub async fn run_whole(
        &self,
        from: super::store::ReadingId,
        to: super::store::ReadingId,
        box_item: String,
    ) -> Result<proposals::RunWhole, AccountError> {
        let game_data = self.game_data.clone();
        let now = naive_to_epoch(self.clock.now());
        let read = joined(
            on_writer(&self.db, move |conn| {
                let roles = super::roles::KnownRoles::read(conn, game_data.as_deref())?;
                proposals::read_run_whole(conn, from, to, &box_item, &roles, now)
            })
            .await,
        )?;
        if read.read_whole {
            let _ = self.sender.send(Msg::ProposalsChanged);
        }
        Ok(read)
    }

    /// The limited armour and plates the player holds, each with the
    /// markup it is billed at, and the items that may be declared one.
    pub async fn armour_pieces(&self) -> Result<super::armour::ArmourPieces, AccountError> {
        let game_data = self.game_data.clone();
        joined(
            on_reader(&self.db, move |conn| {
                let roles = super::roles::KnownRoles::read(conn, game_data.as_deref())?;
                super::armour::pieces(conn, &roles)
            })
            .await,
        )
    }

    /// Set the markup a limited piece is billed at from now on (`None`:
    /// at TT), then rebuild every session's costs it left stale, as a
    /// placement does. The caller has checked the markup.
    pub async fn set_piece_markup(
        &self,
        item: String,
        markup_percent: Option<f64>,
    ) -> Result<Vec<String>, AccountError> {
        let now = naive_to_epoch(self.clock.now());
        joined(
            on_writer(&self.db, move |conn| {
                super::armour::set_markup(conn, &item, markup_percent, now)
            })
            .await,
        )?;
        self.rebuild_after_placement().await
    }

    /// The items the measured sessions could not place, and the player's
    /// placements.
    pub async fn cost_placements(&self) -> Result<Placements, AccountError> {
        joined(on_reader(&self.db, |conn| placement::placements(conn)).await)
    }

    /// Place an item once for every session, then rebuild every session's
    /// costs it left stale and the figures derived from them. Answers with
    /// the sessions whose costs are still to be rebuilt (none, once the
    /// rebuild has caught every one).
    pub async fn place_cost_item(&self, placed: Placement) -> Result<Vec<String>, AccountError> {
        let now = naive_to_epoch(self.clock.now());
        joined(on_writer(&self.db, move |conn| placement::place(conn, &placed, now)).await)?;
        self.rebuild_after_placement().await
    }

    /// Clear an item's placement, so what the app knows places it again,
    /// then rebuild as [`Self::place_cost_item`] does.
    pub async fn clear_cost_placement(&self, item: String) -> Result<Vec<String>, AccountError> {
        joined(on_writer(&self.db, move |conn| placement::clear(conn, &item)).await)?;
        self.rebuild_after_placement().await
    }

    /// Rebuild the costs a placement left stale, and name the sessions
    /// whose costs are still not current: those the rebuild could not
    /// write (one changed meanwhile, or a failure, which is logged).
    async fn rebuild_after_placement(&self) -> Result<Vec<String>, AccountError> {
        self.heal_costs().await;
        let game_data = self.game_data.clone();
        joined(
            on_reader(&self.db, move |conn| {
                let roles = super::roles::KnownRoles::read(conn, game_data.as_deref())?;
                costs::sessions_with_stale_costs(conn, roles.fingerprint())
            })
            .await,
        )
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
        let chat_log = self.chat_log.clone();
        let now = naive_to_epoch(self.clock.now());
        let (page, to_seal) = joined(
            on_reader(&self.db, move |conn| {
                sessions::measured_sessions(
                    conn,
                    seek.as_ref(),
                    limit,
                    game_data.as_deref(),
                    chat_log.as_ref(),
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

    /// Every pending proposal, of one kind or all, the latest seen first.
    pub async fn proposals(
        &self,
        kind: Option<ProposalKind>,
    ) -> Result<Vec<Proposal>, AccountError> {
        joined(on_reader(&self.db, move |conn| proposals::pending(conn, kind)).await)
    }

    /// Decide proposals on the writer, and announce a decision made.
    async fn decide(
        &self,
        job: impl FnOnce(&mut rusqlite::Connection, f64) -> Result<Decided, AccountError>
            + Send
            + 'static,
    ) -> Result<Decided, AccountError> {
        let now = naive_to_epoch(self.clock.now());
        let decided = joined(on_writer(&self.db, move |conn| job(conn, now)).await)?;
        if decided == Decided::Done {
            let _ = self.sender.send(Msg::ProposalsChanged);
        }
        Ok(decided)
    }

    /// Claim proposals before the opening they show is recorded; `retake`
    /// names those whose earlier claim, never linked, is taken over.
    pub async fn claim_proposals(
        &self,
        ids: Vec<ProposalId>,
        retake: Vec<ProposalId>,
    ) -> Result<Decided, AccountError> {
        self.decide(move |conn, now| proposals::claim(conn, &ids, &retake, now))
            .await
    }

    /// Release proposals claimed for a recording that did not happen.
    pub async fn release_proposals(&self, ids: Vec<ProposalId>) -> Result<Decided, AccountError> {
        self.decide(move |conn, _| proposals::release(conn, &ids))
            .await
    }

    /// Link proposals to the record they became, as one.
    pub async fn link_proposals(
        &self,
        ids: Vec<ProposalId>,
        record: String,
    ) -> Result<Decided, AccountError> {
        self.decide(move |conn, now| proposals::link(conn, &ids, &record, now))
            .await
    }

    /// Dismiss a proposal, and announce it.
    pub async fn dismiss_proposal(&self, id: ProposalId) -> Result<Decided, AccountError> {
        self.decide(move |conn, now| proposals::dismiss(conn, id, now))
            .await
    }

    /// What is new to the player in what the account did on its own (see
    /// [`super::activity`]). A read.
    pub async fn activity_marks(&self) -> Result<super::activity::ActivityMarks, AccountError> {
        joined(on_reader(&self.db, |conn| super::activity::marks(conn)).await)
    }

    /// The player has seen what the account did up to `through` (unix
    /// seconds); announced when the watermark moved, so every view of the
    /// marks follows.
    pub async fn mark_activity_seen(&self, through: f64) -> Result<(), AccountError> {
        let now = naive_to_epoch(self.clock.now());
        let moved = joined(
            on_writer(&self.db, move |conn| {
                super::activity::mark_seen(conn, through, now)
            })
            .await,
        )?;
        if moved {
            let _ = self.sender.send(Msg::ProposalsChanged);
        }
        Ok(())
    }

    /// One tracked session minute by minute (see [`super::minutes`]), read
    /// on a reader. The arrivals it offers that are not proposed yet are
    /// stored as proposals, and the pending ones whose flag is gone
    /// withdrawn, in a short write of their own, announced, and read back,
    /// so each carries its handle. `None` for a session that does not
    /// exist.
    pub async fn session_minutes(
        &self,
        session_id: String,
    ) -> Result<Option<SessionMinutes>, AccountError> {
        let game_data = self.game_data.clone();
        let id = session_id.clone();
        let read = joined(
            on_reader(&self.db, move |conn| {
                let roles = super::roles::KnownRoles::read(conn, game_data.as_deref())?;
                minutes::read_minutes(conn, &id, &roles)
            })
            .await,
        )?;
        let Some(read) = read else {
            return Ok(None);
        };
        let seen = read.to_propose();
        let withdraw = read.withdraw.clone();
        if seen.is_empty() && withdraw.is_empty() {
            return Ok(Some(read));
        }
        let now = naive_to_epoch(self.clock.now());
        let writing = session_id.clone();
        let changed = joined(
            on_writer(&self.db, move |conn| {
                proposals::sync_arrivals(conn, &writing, &seen, &withdraw, now)
            })
            .await,
        )?;
        if changed > 0 {
            let _ = self.sender.send(Msg::ProposalsChanged);
        }
        let stored = joined(
            on_reader(&self.db, move |conn| {
                proposals::session_arrivals(conn, &session_id)
            })
            .await,
        )?;
        Ok(Some(read.with_stored(&stored)))
    }

    /// Offer a dismissed arrival again, and announce it.
    pub async fn restore_arrival(&self, id: ProposalId) -> Result<Decided, AccountError> {
        self.decide(move |conn, _| proposals::restore_arrival(conn, id))
            .await
    }

    /// Record an arrival proposal as stock of unknown origin: claimed (an
    /// earlier claim never linked only when `retake`), written through the
    /// analytics service as unattributed stock, then linked to that record.
    /// A write that fails releases the claim; a link that fails leaves the
    /// claim, shown as possibly recorded already, and its record standing,
    /// which a retake links rather than writing a second.
    pub async fn record_arrival_unknown_origin(
        &self,
        id: ProposalId,
        retake: bool,
    ) -> Result<ArrivalRecording, AccountError> {
        let now = naive_to_epoch(self.clock.now());
        let claimed = joined(
            on_writer(&self.db, move |conn| {
                let Some(arrival) = proposals::open_arrival(conn, id)? else {
                    return Ok(Err(ArrivalRecording::NotOpen));
                };
                let retaking: Vec<ProposalId> = if retake { vec![id] } else { Vec::new() };
                match proposals::claim(conn, &[id], &retaking, now)? {
                    Decided::Done => Ok(Ok(arrival)),
                    Decided::NotFound => Ok(Err(ArrivalRecording::NotOpen)),
                    Decided::Refused(reason) => Ok(Err(ArrivalRecording::Refused(reason))),
                }
            })
            .await,
        )?;
        let (arrival, seen_to) = match claimed {
            Ok(claimed) => claimed,
            Err(outcome) => return Ok(outcome),
        };
        let analytics = AnalyticsService::new(self.db.clone(), self.clock.clone());
        let written = analytics
            .record_unknown_origin(
                &arrival.key.name,
                arrival.units as f64,
                arrival.amount.micro_ped() as f64 / super::amount::MICRO_PER_PED as f64,
                seen_to,
                Some(id.0),
            )
            .await;
        // A record from this arrival that still stands (recorded, never
        // linked) is handed back, never written twice.
        let record_id = match written {
            Ok(recorded) => recorded.record_id,
            Err(error) => {
                let _ = self.release_proposals(vec![id]).await;
                tracing::warn!(target: "eo::account", %error, proposal = id.0, "an arrival could not be recorded as stock of unknown origin");
                return Ok(ArrivalRecording::WriteRefused);
            }
        };
        let record = record_id.clone();
        let linked = self
            .decide(move |conn, now| proposals::link(conn, &[id], &record, now))
            .await?;
        Ok(match linked {
            Decided::Done => ArrivalRecording::Recorded { record_id },
            // The stock stands; the arrival stays claimed, shown as possibly
            // recorded, until the player takes it over.
            Decided::NotFound | Decided::Refused(_) => ArrivalRecording::Unlinked { record_id },
        })
    }

    /// Take back stock of unknown origin recorded from an arrival (by the
    /// record's own id): its stock goes, and the arrival is open to record
    /// again. `false` when no such record stands. Announced.
    pub async fn undo_arrival_unknown_origin(
        &self,
        record_id: String,
    ) -> Result<bool, crate::analytics::AnalyticsError> {
        let analytics = AnalyticsService::new(self.db.clone(), self.clock.clone());
        let undone = analytics.undo_unknown_origin(&record_id).await?
            == crate::analytics::UnknownOriginUndo::Undone;
        if undone {
            let _ = self.sender.send(Msg::ProposalsChanged);
        }
        Ok(undone)
    }

    /// Plan a stock reconciliation from the latest stored reading, kept as
    /// the draft being reviewed (see [`super::reconcile`]).
    pub async fn reconciliation_from_reading(
        &self,
    ) -> Result<crate::analytics::ReconciliationDraft, super::reconcile::PlanError> {
        let analytics = AnalyticsService::new(self.db.clone(), self.clock.clone());
        super::reconcile::plan_from_reading(&self.db, &analytics, self.game_data.clone()).await
    }

    /// Plan a stock reconciliation from an inventory pasted from the
    /// account site, observed now, kept as the draft being reviewed. Needs
    /// no connection.
    pub async fn reconciliation_from_paste(
        &self,
        text: String,
    ) -> Result<crate::analytics::ReconciliationDraft, super::reconcile::PlanError> {
        let analytics = AnalyticsService::new(self.db.clone(), self.clock.clone());
        let now = naive_to_epoch(self.clock.now());
        super::reconcile::plan_from_paste(&self.db, &analytics, self.game_data.clone(), &text, now)
            .await
    }

    /// A strongbox run begins: answered with a reading from before the
    /// boxes are opened, once it is stored and examined (the latest one,
    /// when it was taken inside the floor), waiting at most [`RUN_WAIT`].
    /// Reading goes on every minute until the run is done or closed.
    pub async fn run_begin(&self) -> RunReading {
        self.run_request(Msg::RunBegin).await
    }

    /// A strongbox run is done: answered with a reading started after
    /// this was asked, once it is stored and examined, waiting at most
    /// [`RUN_WAIT`]. Then reading for the run stops.
    pub async fn run_done(&self) -> RunReading {
        self.run_request(Msg::RunDone).await
    }

    /// Close the run without another reading: its view went away.
    pub fn run_cancel(&self) {
        let _ = self.sender.send(Msg::RunCancel);
    }

    async fn run_request(&self, message: fn(oneshot::Sender<RunReading>) -> Msg) -> RunReading {
        let (reply, answer) = oneshot::channel();
        if self.sender.send(message(reply)).is_err() {
            return RunReading::Interrupted;
        }
        match tokio::time::timeout(RUN_WAIT, answer).await {
            Ok(Ok(reading)) => reading,
            Ok(Err(_)) => RunReading::Interrupted,
            Err(_) => {
                // Given up on: the run ends with its consumer.
                let _ = self.sender.send(Msg::RunCancel);
                RunReading::Unreadable
            }
        }
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

/// Rebuild every stale cost projection (see [`costs::heal_session_costs`]),
/// logging a failure: it never fails the work it follows. Blocking.
fn heal_costs(
    db: &Db,
    game_data: Option<Arc<GameDataStore>>,
    chat_log: Option<ChatLogSource>,
    now: f64,
    being_read: Option<String>,
) -> usize {
    let written = match costs::heal_session_costs(db, game_data.clone(), chat_log, now, being_read)
    {
        Ok(written) => written,
        Err(error) => {
            tracing::error!(target: "eo::account", %error, "stale session costs could not be rebuilt");
            0
        }
    };
    // What the steps between sessions bill, priced again by the roles as
    // they now stand, a bounded batch a write.
    loop {
        let game_data = game_data.clone();
        let healed = db
            .with_writer_blocking(move |conn| {
                Ok(super::roles::KnownRoles::read(conn, game_data.as_deref())
                    .and_then(|roles| super::between::heal(conn, &roles, now)))
            })
            .map_err(AccountError::from)
            .and_then(|healed| healed);
        match healed {
            Ok(healed) if healed >= super::between::STEPS_PER_HEAL => continue,
            Ok(_) => break,
            Err(error) => {
                tracing::error!(target: "eo::account", %error, "the overhead between sessions could not be priced again");
                break;
            }
        }
    }
    written
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
    /// For a strongbox run, outside a tracked session.
    Run,
    /// While signed in and nothing else is read.
    Idle,
}

impl ReadingFor {
    fn session(&self) -> Option<&str> {
        match self {
            ReadingFor::Connect | ReadingFor::Run | ReadingFor::Idle => None,
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
            ReadingFor::Run => Purpose::Run,
            ReadingFor::Idle => Purpose::Ambient,
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
    /// When the request started.
    started: Instant,
    /// The floor as it stood before the request began, restored when an
    /// idle reading is abandoned for a session's baseline.
    floor_before: FloorStart,
}

/// Where the reading floor runs from: when the latest full reading began,
/// and whether that was a reading while idle (whose floor a session's
/// baseline does not wait for).
#[derive(Debug, Clone, Copy)]
struct FloorStart {
    at: Option<Instant>,
    idle: bool,
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
    /// Stale session costs rebuilt: how many were written.
    Heal { task: JoinHandle<usize> },
    /// A running session's costs so far published: whether anything was
    /// written.
    Publish {
        session: String,
        task: JoinHandle<Result<bool, AccountError>>,
    },
    /// A reading just stored, examined for what it proposes.
    Examine {
        reading: ReadingId,
        taken_at: f64,
        /// When its request started.
        started: Instant,
        /// Taken while idle: a strongbox run waiting is told only of a
        /// reading it can use, and otherwise takes its own.
        idle: bool,
        task: JoinHandle<Result<Option<Examined>, AccountError>>,
    },
    /// Readings a previous run left unexamined.
    CatchUp {
        task: JoinHandle<Result<Option<Examined>, AccountError>>,
    },
    /// What the steps waiting did to the stock, recorded.
    Follow {
        task: JoinHandle<Result<FollowPass, FollowFailure>>,
    },
    /// A balance asked for only so the site keeps the session.
    KeepAlive {
        /// The connection it was asked for.
        generation: u64,
        task: JoinHandle<SiteOutcome<Pec>>,
    },
}

enum Done {
    Validated(Result<SiteOutcome<Pec>, JoinError>),
    Fetched(Result<SiteOutcome<Reading>, JoinError>),
    Stored(Result<Result<Option<(ReadingHeader, Reading)>, AccountError>, JoinError>),
    Sealed(Result<Result<bool, AccountError>, JoinError>),
    Healed(Result<usize, JoinError>),
    Published(Result<Result<bool, AccountError>, JoinError>),
    Examined(Result<Result<Option<Examined>, AccountError>, JoinError>),
    CaughtUp(Result<Result<Option<Examined>, AccountError>, JoinError>),
    Followed(Result<Result<FollowPass, FollowFailure>, JoinError>),
    KeptAlive(Result<SiteOutcome<Pec>, JoinError>),
}

async fn finish(work: &mut Option<Work>) -> Done {
    match work {
        None => std::future::pending().await,
        Some(Work::Validate { task, .. }) => Done::Validated(task.await),
        Some(Work::Fetch { task, .. }) => Done::Fetched(task.await),
        Some(Work::Store { task, .. }) => Done::Stored(task.await),
        Some(Work::Seal { task, .. }) => Done::Sealed(task.await),
        Some(Work::Heal { task }) => Done::Healed(task.await),
        Some(Work::Publish { task, .. }) => Done::Published(task.await),
        Some(Work::Examine { task, .. }) => Done::Examined(task.await),
        Some(Work::CatchUp { task }) => Done::CaughtUp(task.await),
        Some(Work::Follow { task }) => Done::Followed(task.await),
        Some(Work::KeepAlive { task, .. }) => Done::KeptAlive(task.await),
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
    RunDue,
    RunExpired,
    KeepAliveDue,
    IdleReadingDue,
    ConnectReadingDue,
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
    chat_log: Option<ChatLogSource>,
    changed: Option<ChangedSink>,
    mode: ReadingMode,
    store: Arc<dyn SessionStore>,
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
    /// When the latest request of any kind was made to the site: the
    /// keep-alive runs from it.
    last_contact: Option<Instant>,
    /// When the site last answered for the session.
    last_answer: Option<Instant>,
    /// The keep-alive fell due and waits for the work in flight.
    keep_alive_due: bool,
    /// A reading while idle fell due and waits for the work in flight.
    idle_due: bool,
    /// Failed readings while idle in a row.
    idle_failures: u32,
    /// When the failures in a row of any reading began, while connected.
    failing_since: Option<f64>,
    /// Failed readings of any kind in a row, while connected.
    failures_in_a_row: u32,
    /// A connect made inside the floor takes its reading once the floor
    /// passes, at this instant.
    connect_reading_at: Option<Instant>,
    /// The connect's reading fell due and waits for the work in flight.
    connect_reading_due: bool,
    /// When the current connection was made: a full reading begun since
    /// is the connect's reading.
    connected_since: Option<Instant>,
    /// Whether the latest full reading began while idle: a session's
    /// baseline is never held back by its floor.
    floor_from_idle: bool,
    /// How confirming the stored session stands, while unconfirmed.
    confirming: Confirming,
    signing_in: bool,
    /// The latest stored reading.
    latest: Option<Reading>,
    /// Whether the latest stored reading differed from the one stored
    /// before it.
    latest_changed: bool,
    /// Whether the latest stored reading is tagged with a session.
    latest_tagged: bool,
    last_reading_at: Option<f64>,
    pending_seals: VecDeque<String>,
    queued_connect: Option<PendingConnect>,
    flush_at: Option<Instant>,
    readings_stored: u64,
    /// The latest stored reading's id.
    latest_id: Option<ReadingId>,
    /// Readings a previous run left unexamined are still to be examined.
    catch_up: bool,
    /// A reading was stored outside any session: it may be the one a
    /// sealed session's gap ends at, so stale costs are rebuilt.
    heal_due: bool,
    /// A reading of the running session was stored: what it has cost so
    /// far is published for the live figures.
    live_due: Option<String>,
    /// Steps may wait to have what they did to the stock recorded.
    follow_due: bool,
    /// The step the last passes failed on, and how many in a row.
    follow_failing: Option<(ReadingId, u32)>,
    /// A step failed [`FOLLOW_FAILURES_BEFORE_PASSING`] passes in a row:
    /// the next pass marks it with nothing recorded.
    follow_pass_over: Option<ReadingId>,
    /// The strongbox run being read, if one is open.
    run: Option<Run>,
    /// When the run's next reading falls due.
    run_next: Option<Instant>,
    /// A run reading fell due and waits for the work in flight.
    run_due: bool,
    /// Run requests waiting for their reading.
    waiters: Vec<Waiter>,
}

impl Actor {
    async fn run(mut self, mut inbox: mpsc::UnboundedReceiver<Msg>) {
        self.seed().await;
        self.advance();
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
            let run_at = if self.run_due { None } else { self.run_next };
            let run_closes = self.run_closes();
            let keep_alive_at = self.keep_alive_at();
            let idle_at = if self.idle_due {
                None
            } else {
                self.idle_reading_at()
            };
            let connect_at = if self.connect_reading_due {
                None
            } else {
                self.connect_reading_at
            };
            // Messages first: a disconnect that arrives with a result is
            // applied before the result is looked at.
            let event = tokio::select! {
                biased;
                message = inbox.recv() => Event::Message(message),
                done = finish(&mut self.work) => Event::Done(done),
                () = at(attempt_at) => Event::Attempt,
                () = at(check_at) => Event::CheckFloorPassed,
                () = at(flush_at) => Event::Flush,
                () = at(run_at) => Event::RunDue,
                () = at(run_closes) => Event::RunExpired,
                () = at(keep_alive_at) => Event::KeepAliveDue,
                () = at(idle_at) => Event::IdleReadingDue,
                () = at(connect_at) => Event::ConnectReadingDue,
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
                Event::RunDue => {
                    self.run_next = None;
                    self.run_due = true;
                }
                Event::RunExpired => self.close_run(RunReading::Interrupted),
                Event::KeepAliveDue => self.keep_alive_due = true,
                Event::IdleReadingDue => self.idle_due = true,
                Event::ConnectReadingDue => {
                    self.connect_reading_at = None;
                    self.connect_reading_due = true;
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
                self.floor_from_idle = stored.header.purpose == Purpose::Ambient;
            }
            self.last_reading_at = Some(taken_at);
            self.latest_id = Some(stored.header.id);
            self.latest_tagged = stored.header.session_id.is_some();
            self.latest = Some(stored.reading);
        }
        if let Some(session) = active {
            self.tracking = Tracking::Running(session);
        }
        // Costs a change made while the app was closed (a new build's
        // rules, an item placed differently) left stale are rebuilt now.
        let db = self.db.clone();
        let game_data = self.game_data.clone();
        let chat_log = self.chat_log.clone();
        let now = self.now_epoch();
        let _ =
            tokio::task::spawn_blocking(move || heal_costs(&db, game_data, chat_log, now, None))
                .await;
    }

    fn now_epoch(&self) -> f64 {
        naive_to_epoch(self.clock.now())
    }

    /// The session the connection is reading now, if any.
    fn reading_session(&self) -> Option<String> {
        self.is_connected()
            .then(|| self.tracking.session().map(str::to_string))
            .flatten()
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
            reading_session: self.reading_session(),
            tracked_session: match &self.tracking {
                Tracking::Running(session) => Some(session.clone()),
                Tracking::Idle | Tracking::Grace { .. } => None,
            },
            has_connected: self.has_connected,
            readings_stored: self.readings_stored,
            working: self.work.is_some()
                || self.queued_connect.is_some()
                || !self.pending_seals.is_empty()
                || self.live_due.is_some()
                || self.attempt_due.is_some()
                || self.catch_up,
            confirming: matches!(self.connection, Connection::Unconfirmed)
                .then_some(self.confirming),
            signing_in: self.signing_in,
            has_readings: self.latest.is_some(),
            failing_since: self.is_connected().then_some(self.failing_since).flatten(),
            failures_in_a_row: if self.is_connected() {
                self.failures_in_a_row
            } else {
                0
            },
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
            Msg::RunBegin(reply) => self.run_begin(reply),
            Msg::RunDone(reply) => self.run_done(reply),
            Msg::RunCancel => self.close_run(RunReading::Interrupted),
            Msg::ProposalsChanged => self.mark_changed(),
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
        // A reading while idle still on its way is abandoned (its answer is
        // dropped when it comes), and its floor with it: the session's
        // baseline is taken at once.
        let abandoned = match &self.work {
            Some(Work::Fetch { ticket, task }) if ticket.purpose == ReadingFor::Idle => {
                task.abort();
                Some(ticket.floor_before)
            }
            _ => None,
        };
        if let Some(floor) = abandoned {
            self.last_full_start = floor.at;
            self.floor_from_idle = floor.idle;
        }
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
            Some(Work::KeepAlive { task, .. }) => task.abort(),
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
        self.close_run(match to {
            Connection::Expired => RunReading::SignedOut,
            _ => RunReading::NotConnected,
        });
        self.generation += 1;
        self.connection = to;
        self.next_attempt = None;
        self.attempt_due = None;
        self.keep_alive_due = false;
        self.idle_due = false;
        self.idle_failures = 0;
        self.failing_since = None;
        self.failures_in_a_row = 0;
        self.connect_reading_at = None;
        self.connect_reading_due = false;
        self.connected_since = None;
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
        // A session's baseline is never answered by a reading taken while
        // idle: it would stand untagged for the session's start.
        let past_idle_floor = matches!(purpose, ReadingFor::Baseline(_)) && self.floor_from_idle;
        if let Some(end) = self
            .floor_end()
            .filter(|end| now < *end && !past_idle_floor)
        {
            return Began::Floor(end);
        }
        let Connection::Connected { session, .. } = &self.connection else {
            return Began::NotConnected;
        };
        debug_assert!(self.work.is_none(), "one request at a time");
        let floor_before = FloorStart {
            at: self.last_full_start,
            idle: self.floor_from_idle,
        };
        self.last_full_start = Some(now);
        self.floor_from_idle = purpose == ReadingFor::Idle;
        self.last_contact = Some(now);
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
                started: now,
                floor_before,
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
                self.last_contact = Some(now);
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
            let healing_with = self.game_data.clone();
            let chat_log = self.chat_log.clone();
            let sealing_with = self.chat_log.clone();
            let now = self.now_epoch();
            let being_read = self.reading_session();
            let sealing = session.clone();
            let task = tokio::task::spawn_blocking(move || {
                // Computed on a reader; only the result is written.
                let seal = db
                    .with_reader_blocking(move |conn| {
                        Ok(sessions::measure_to_seal(
                            conn,
                            &sealing,
                            game_data.as_deref(),
                            sealing_with.as_ref(),
                            now,
                        ))
                    })
                    .map_err(AccountError::from)
                    .and_then(|seal| seal)?;
                let written = match seal {
                    Some(seal) => db
                        .with_writer_blocking(move |conn| {
                            Ok(sessions::cache_sealed(conn, &[seal], now))
                        })
                        .map_err(AccountError::from)
                        .and_then(|cached| cached)?,
                    None => 0,
                };
                // Then any other session's costs a change has left stale.
                heal_costs(&db, healing_with, chat_log, now, being_read);
                Ok(written > 0)
            });
            self.work = Some(Work::Seal { session, task });
            return;
        }
        // What the running session has cost so far, computed on a reader
        // and published before the next reading is asked for.
        if let Some(session) = self.live_due.take() {
            let db = self.db.clone();
            let game_data = self.game_data.clone();
            let now = self.now_epoch();
            let publishing = session.clone();
            let task = tokio::task::spawn_blocking(move || {
                super::live::publish_so_far(&db, game_data, publishing, now)
            });
            self.work = Some(Work::Publish { session, task });
            return;
        }
        if let Some(due) = self.attempt_due.take() {
            self.attempt(due);
            if self.work.is_some() {
                return;
            }
        }
        if self.run_due {
            self.run_due = false;
            self.run_attempt();
            if self.work.is_some() {
                return;
            }
        }
        if self.connect_reading_due {
            self.connect_reading_due = false;
            self.connect_reading();
            if self.work.is_some() {
                return;
            }
        }
        if self.idle_due {
            self.idle_due = false;
            self.idle_reading();
            if self.work.is_some() {
                return;
            }
        }
        if self.keep_alive_due {
            self.keep_alive_due = false;
            self.keep_alive();
            if self.work.is_some() {
                return;
            }
        }
        // Costs a reading outside any session may have let a sealed
        // session's gap be weighed, rebuilt whenever nothing else is due.
        if self.heal_due {
            self.heal_due = false;
            let db = self.db.clone();
            let game_data = self.game_data.clone();
            let chat_log = self.chat_log.clone();
            let now = self.now_epoch();
            let being_read = self.reading_session();
            let task = tokio::task::spawn_blocking(move || {
                heal_costs(&db, game_data, chat_log, now, being_read)
            });
            self.work = Some(Work::Heal { task });
            return;
        }
        // What the steps waiting did to the stock, whenever nothing else is
        // due and no strongbox run is reading: the run's flow records what
        // its readings show.
        if self.follow_due && self.run.is_none() {
            self.follow_due = false;
            let db = self.db.clone();
            let analytics = AnalyticsService::new(self.db.clone(), self.clock.clone());
            let game_data = self.game_data.clone();
            let now = self.now_epoch();
            let pass_over = self.follow_pass_over;
            let task = tokio::task::spawn_blocking(move || {
                super::follow::follow(&db, &analytics, game_data, now, pass_over)
            });
            self.work = Some(Work::Follow { task });
            return;
        }
        // Readings a previous run left unexamined, a batch at a time,
        // whenever nothing else is due.
        if self.catch_up {
            self.catch_up = false;
            self.start_catch_up();
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
            (Work::Publish { session, .. }, Done::Published(published)) => match published {
                Ok(Ok(true)) => self.mark_changed(),
                Ok(Ok(false)) => {}
                Ok(Err(error)) => {
                    tracing::warn!(target: "eo::account", %error, session_id = %session, "a running session's costs so far could not be published");
                }
                Err(error) => {
                    tracing::warn!(target: "eo::account", %error, "publishing a running session's costs so far stopped");
                }
            },
            (Work::Heal { .. }, Done::Healed(healed)) => match healed {
                Ok(0) => {}
                Ok(_) => self.mark_changed(),
                Err(error) => {
                    tracing::error!(target: "eo::account", %error, "rebuilding stale session costs stopped");
                }
            },
            (
                Work::Examine {
                    reading,
                    taken_at,
                    started,
                    idle,
                    ..
                },
                Done::Examined(examined),
            ) => self.examined(reading, taken_at, started, idle, examined),
            (Work::CatchUp { .. }, Done::CaughtUp(pass)) => self.caught_up(pass),
            (Work::Follow { .. }, Done::Followed(pass)) => match pass {
                Ok(Ok(pass)) => {
                    if pass.recorded > 0 {
                        self.mark_changed();
                    }
                    self.follow_failing = None;
                    self.follow_pass_over = None;
                    // The rest, once whatever else is due has gone.
                    self.follow_due |= pass.more;
                }
                Ok(Err(failure)) => {
                    // Left for the pass after the next reading, unless the
                    // same step has failed too often: then it is passed
                    // over, so it never holds the steps after it.
                    tracing::error!(target: "eo::account", error = %failure.error, "what the readings did to the stock could not be recorded");
                    if let Some(reading) = failure.reading {
                        let failures = match self.follow_failing {
                            Some((failing, count)) if failing == reading => count + 1,
                            _ => 1,
                        };
                        if failures >= FOLLOW_FAILURES_BEFORE_PASSING {
                            self.follow_failing = None;
                            self.follow_pass_over = Some(reading);
                            self.follow_due = true;
                        } else {
                            self.follow_failing = Some((reading, failures));
                        }
                    }
                }
                Err(error) => {
                    tracing::error!(target: "eo::account", %error, "recording what the readings did to the stock stopped");
                }
            },
            (Work::KeepAlive { generation, .. }, Done::KeptAlive(answer)) => {
                self.kept_alive(generation, answer)
            }
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
        self.idle_failures = 0;
        self.failing_since = None;
        self.failures_in_a_row = 0;
        self.connect_reading_at = None;
        self.connect_reading_due = false;
        self.connected_since = Some(started);
        self.last_answer = Some(Instant::now());
        self.connection = Connection::Connected {
            session: session.clone(),
            connected_at: self.now_epoch(),
        };
        // A session the player signed in with is kept for the next launch.
        if source == ConnectSource::SignIn {
            let store = self.store.clone();
            let keeping = session.clone();
            tokio::task::spawn_blocking(move || store.save(&keeping));
        }
        // The check's balance is the connect reading's: one item read
        // completes it. Inside the floor of a reading begun before (one
        // the site may have refused, under an earlier sign-in), the
        // connect reading is taken in full the moment the floor passes:
        // a sign-in always gets its reading.
        let floor_stands = self.floor_end().filter(|end| started < *end);
        if let Some(end) = floor_stands {
            self.connect_reading_at = Some(end);
        } else {
            let floor_before = FloorStart {
                at: self.last_full_start,
                idle: self.floor_from_idle,
            };
            self.last_full_start = Some(started);
            self.floor_from_idle = false;
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
                    started,
                    floor_before,
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
        // A reading while idle is kept only while no tracked session has
        // taken over (the session abandoned it).
        let idle = ticket.purpose == ReadingFor::Idle;
        let for_this = for_this && (!idle || matches!(self.tracking, Tracking::Idle));
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
                if current {
                    self.unreadable(&ticket);
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
                self.last_answer = Some(Instant::now());
                self.failing_since = None;
                self.failures_in_a_row = 0;
                if idle || ticket.purpose == ReadingFor::Connect {
                    self.idle_failures = 0;
                }
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
            SiteOutcome::Unauthorised => self.signed_out_by_site("a reading"),
            SiteOutcome::Failed(failure) => {
                tracing::warn!(target: "eo::account", ?failure, "an account reading failed; the next falls due as usual");
                self.unreadable(&ticket);
                self.failures_in_a_row += 1;
                if self.failing_since.is_none() {
                    self.failing_since = Some(self.now_epoch());
                }
                self.mark_changed();
                if idle || ticket.purpose == ReadingFor::Connect {
                    // Read again a minute on, never pushed out to the idle
                    // interval by the failure; every fifth minute once
                    // failures run on.
                    self.idle_failures += 1;
                }
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
                // A reading outside any session may end a sealed session's
                // gap. One taken while idle that changes nothing after
                // another outside any session cannot.
                let ends_nothing =
                    header.purpose == Purpose::Ambient && identical && !self.latest_tagged;
                if header.session_id.is_none() && !ends_nothing {
                    self.heal_due = true;
                }
                // A reading of the session still running moves what it has
                // cost so far.
                if let (Some(session), Tracking::Running(running)) =
                    (header.session_id.as_deref(), &self.tracking)
                {
                    if session == running {
                        self.live_due = Some(running.clone());
                    }
                }
                self.last_reading_at = Some(header.taken_at);
                self.latest_id = Some(header.id);
                self.latest_changed = !identical;
                self.latest_tagged = header.session_id.is_some();
                self.latest = Some(reading);
                self.readings_stored += 1;
                self.follow_due = true;
                self.mark_changed();
                if closing {
                    self.closing_done(Some(identical));
                }
                self.start_examine(
                    header.id,
                    header.taken_at,
                    ticket.started,
                    ticket.purpose == ReadingFor::Idle,
                );
            }
            Ok(Err(error)) => {
                tracing::error!(target: "eo::account", %error, "an account reading could not be stored");
                self.unreadable(&ticket);
                if closing {
                    self.closing_done(None);
                }
            }
            Err(error) => {
                tracing::error!(target: "eo::account", %error, "storing an account reading stopped");
                self.unreadable(&ticket);
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

    /// A reading came to nothing: the strongbox run waiting for it is told,
    /// unless it was taken while idle, when the run takes its own instead.
    fn unreadable(&mut self, ticket: &Ticket) {
        if ticket.purpose != ReadingFor::Idle {
            self.answer_waiters(ticket.started, RunReading::Unreadable);
        }
    }

    /// Whether the account is read in full now that nothing else is read:
    /// built to read always, connected, no session being read or closed,
    /// and no strongbox run open.
    fn reads_while_idle(&self) -> bool {
        self.mode == ReadingMode::Always
            && self.is_connected()
            && matches!(self.tracking, Tracking::Idle)
            && self.run.is_none()
    }

    /// When the next reading while idle falls due: a minute after the
    /// latest full reading began when that reading changed something,
    /// five minutes after it otherwise, and never inside the floor.
    fn idle_reading_at(&self) -> Option<Instant> {
        if !self.reads_while_idle() {
            return None;
        }
        let every = if self.idle_failures >= FAILURES_BEFORE_BACKING_OFF {
            READ_EVERY_WHILE_FAILING.max(IDLE_READ_EVERY)
        } else if self.latest_changed || self.idle_failures > 0 {
            READ_EVERY
        } else {
            IDLE_READ_EVERY
        };
        let due = self
            .last_full_start
            .map_or_else(Instant::now, |start| start + every);
        Some(due.max(self.floor_end().unwrap_or(due)))
    }

    /// Read the account in full while idle, unless something read it since
    /// the reading fell due (the work it waited for), or a session or run
    /// has taken over.
    fn idle_reading(&mut self) {
        if self
            .idle_reading_at()
            .is_some_and(|due| due <= Instant::now())
        {
            let _ = self.begin_reading(ReadingFor::Idle, None);
        }
    }

    /// The reading a connect made inside the floor is owed, once the floor
    /// has passed: unless a full reading has begun since the connect (it
    /// is the connect's), or a tracked session or a strongbox run reads
    /// the account itself.
    fn connect_reading(&mut self) {
        if !self.is_connected() || !matches!(self.tracking, Tracking::Idle) || self.run.is_some() {
            return;
        }
        let read_since = match (self.last_full_start, self.connected_since) {
            (Some(start), Some(since)) => start >= since,
            _ => false,
        };
        if read_since {
            return;
        }
        if let Began::Floor(end) = self.begin_reading(ReadingFor::Connect, None) {
            self.connect_reading_at = Some(end);
        }
    }

    /// When the keep-alive falls due: five minutes after the latest
    /// request to the site, while connected, not already due, and the
    /// account is not being read while idle (those readings keep the
    /// session themselves).
    fn keep_alive_at(&self) -> Option<Instant> {
        if !self.is_connected() || self.keep_alive_due || self.reads_while_idle() {
            return None;
        }
        self.last_contact.map(|contact| contact + KEEP_ALIVE_EVERY)
    }

    /// Ask for the balance, unless a request has been made since the
    /// keep-alive fell due (the work it waited for was one), or the
    /// account is now read while idle.
    fn keep_alive(&mut self) {
        if self.reads_while_idle() {
            return;
        }
        let Connection::Connected { session, .. } = &self.connection else {
            return;
        };
        let now = Instant::now();
        if self
            .last_contact
            .is_some_and(|contact| now < contact + KEEP_ALIVE_EVERY)
        {
            return;
        }
        let site = self.site.clone();
        let session = session.clone();
        self.last_contact = Some(now);
        let task = tokio::spawn(async move { site.read_balance(&session).await });
        self.work = Some(Work::KeepAlive {
            generation: self.generation,
            task,
        });
    }

    fn kept_alive(&mut self, generation: u64, answer: Result<SiteOutcome<Pec>, JoinError>) {
        if generation != self.generation || !self.is_connected() {
            return;
        }
        match answer {
            Ok(SiteOutcome::Read(_)) => {
                self.last_answer = Some(Instant::now());
                tracing::debug!(target: "eo::account", "the account session was kept alive");
            }
            Ok(SiteOutcome::Unauthorised) => self.signed_out_by_site("a keep-alive"),
            Ok(SiteOutcome::Failed(failure)) => {
                tracing::warn!(target: "eo::account", ?failure, "a keep-alive failed; the next falls due as usual");
            }
            Err(_) => {}
        }
    }

    /// The site refused the session: the connection ends as expired. How
    /// long the session lasted, and how long since the site last answered
    /// for it, are logged: they say how long the site keeps a session.
    fn signed_out_by_site(&mut self, refused: &'static str) {
        let connected_minutes = match &self.connection {
            Connection::Connected { connected_at, .. } => {
                ((self.now_epoch() - connected_at) / 60.0).round()
            }
            _ => 0.0,
        };
        let since_answer_minutes = self
            .last_answer
            .map(|answer| (answer.elapsed().as_secs_f64() / 60.0).round());
        tracing::info!(
            target: "eo::account",
            refused,
            connected_minutes,
            ?since_answer_minutes,
            "the account site no longer accepts the session"
        );
        self.end_connection(Connection::Expired);
    }

    fn stop(&mut self) {
        if let Some(connect) = self.queued_connect.take() {
            let _ = connect.reply.send(ConnectOutcome::Interrupted);
        }
        self.close_run(RunReading::Interrupted);
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
