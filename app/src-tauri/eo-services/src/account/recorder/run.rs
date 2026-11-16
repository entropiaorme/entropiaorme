//! A strongbox run read from the account, and every stored reading
//! examined for what it proposes.
//!
//! Outside a tracked session nothing reads the account, so the strongbox
//! view asks for readings of its own. When a run begins it needs a reading
//! from before the boxes are opened: the latest stored reading when it was
//! taken inside the floor, otherwise one taken at once. While the run is
//! open the account is read every minute, so no step grows long. When the
//! player says the run is done, one more reading is taken as soon as the
//! floor allows, and reading for the run stops. A run left open closes
//! after [`RUN_OPEN_FOR`]; one whose view goes away is closed by it.
//!
//! A run's readings go through the same gate as every other reading and
//! are never taken inside the floor. While a tracked session is being read
//! its own readings serve the run instead. A request is answered only once
//! its reading is stored and examined, so the proposals it led to can be
//! read at once.

use std::time::Duration;

use tokio::sync::oneshot;
use tokio::task::JoinError;
use tokio::time::Instant;

use super::{Actor, Began, ReadingFor, Tracking, Work, READING_FLOOR};
use crate::account::proposals::{self, Examined, Which};
use crate::account::store::ReadingId;
use crate::account::AccountError;

/// How often an open run is read.
pub const RUN_READ_EVERY: Duration = Duration::from_secs(60);
/// How long a run stays open without being done.
pub const RUN_OPEN_FOR: Duration = Duration::from_secs(30 * 60);
/// The longest a run request waits for its reading: the floor, and a
/// reading that is slow to come back.
pub const RUN_WAIT: Duration = Duration::from_secs(150);

/// What a reading asked for by a strongbox run came to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RunReading {
    /// The reading, stored and examined.
    Read { reading: ReadingId, taken_at: f64 },
    /// No connection to read with.
    NotConnected,
    /// The site no longer accepts the session.
    SignedOut,
    /// The site could not be read, or the reading not stored.
    Unreadable,
    /// The run was closed, or the app is closing, before it came.
    Interrupted,
}

/// An open run.
pub(super) struct Run {
    /// When it closes on its own, unless it is done first.
    closes_at: Instant,
    /// Done: one more reading, then reading stops.
    closing: bool,
}

/// A run request waiting for its reading.
pub(super) struct Waiter {
    /// When it was asked.
    since: Instant,
    /// The run's last reading: only a reading started after it was asked
    /// answers it. Any reading answers the start of a run.
    last: bool,
    reply: oneshot::Sender<RunReading>,
}

impl Actor {
    /// Why a run cannot be read now, if it cannot.
    fn unready(&self) -> Option<RunReading> {
        match self.connection {
            super::Connection::Connected { .. } => None,
            super::Connection::Expired => Some(RunReading::SignedOut),
            super::Connection::Disconnected | super::Connection::Unconfirmed => {
                Some(RunReading::NotConnected)
            }
        }
    }

    pub(super) fn run_closes(&self) -> Option<Instant> {
        self.run
            .as_ref()
            .filter(|run| !run.closing)
            .map(|run| run.closes_at)
    }

    pub(super) fn run_begin(&mut self, reply: oneshot::Sender<RunReading>) {
        if let Some(outcome) = self.unready() {
            let _ = reply.send(outcome);
            return;
        }
        let now = Instant::now();
        let closing = self.run.as_ref().is_some_and(|run| run.closing);
        self.run = Some(Run {
            closes_at: now + RUN_OPEN_FOR,
            closing,
        });
        // The latest reading, taken inside the floor, is from before the
        // boxes are opened.
        let fresh_age = READING_FLOOR.as_secs_f64();
        let fresh = match (self.latest_id, self.last_reading_at) {
            (Some(reading), Some(taken_at))
                if (0.0..fresh_age).contains(&(self.now_epoch() - taken_at)) =>
            {
                Some((reading, taken_at))
            }
            _ => None,
        };
        match fresh {
            Some((reading, taken_at)) => {
                let _ = reply.send(RunReading::Read { reading, taken_at });
                if self.run_next.is_none() && !self.run_due {
                    self.run_next = Some(
                        self.last_full_start
                            .map_or(now, |start| start + RUN_READ_EVERY)
                            .max(now),
                    );
                }
            }
            None => {
                self.waiters.push(Waiter {
                    since: now,
                    last: false,
                    reply,
                });
                self.run_next = None;
                self.run_due = true;
            }
        }
    }

    pub(super) fn run_done(&mut self, reply: oneshot::Sender<RunReading>) {
        if let Some(outcome) = self.unready() {
            let _ = reply.send(outcome);
            return;
        }
        let now = Instant::now();
        match &mut self.run {
            Some(run) => run.closing = true,
            None => {
                self.run = Some(Run {
                    closes_at: now + RUN_OPEN_FOR,
                    closing: true,
                })
            }
        }
        self.waiters.push(Waiter {
            since: now,
            last: true,
            reply,
        });
        self.run_next = None;
        self.run_due = true;
    }

    /// Close the run, answering every request still waiting with `outcome`.
    pub(super) fn close_run(&mut self, outcome: RunReading) {
        self.run = None;
        self.run_next = None;
        self.run_due = false;
        for waiter in self.waiters.drain(..) {
            let _ = waiter.reply.send(outcome);
        }
    }

    /// A run reading fell due: take it through the gate, or wait for the
    /// floor. While a tracked session is being read, its own readings
    /// serve the run.
    pub(super) fn run_attempt(&mut self) {
        // A request whose caller stopped waiting is gone. A run whose last
        // reading nobody waits for any more is over.
        self.waiters.retain(|waiter| !waiter.reply.is_closed());
        let Some(run) = &self.run else {
            return;
        };
        let closing = run.closing;
        if closing && !self.waiters.iter().any(|waiter| waiter.last) {
            self.close_run(RunReading::Interrupted);
            return;
        }
        if !self.is_connected() {
            return;
        }
        let now = Instant::now();
        if !matches!(self.tracking, Tracking::Idle) {
            self.run_next = Some(now + RUN_READ_EVERY);
            return;
        }
        self.run_next = match self.begin_reading(ReadingFor::Run, None) {
            Began::Floor(end) => Some(end),
            Began::Started if closing => None,
            Began::Started => Some(now + RUN_READ_EVERY),
            Began::NotConnected => None,
        };
    }

    /// Answer the requests a reading whose request started at `started`
    /// answers. The run's last reading answered closes it, and so does any
    /// answer that is not a reading.
    pub(super) fn answer_waiters(&mut self, started: Instant, outcome: RunReading) {
        let mut kept = Vec::new();
        let mut answered = false;
        let mut last_answered = false;
        for waiter in self.waiters.drain(..) {
            if !waiter.last || started >= waiter.since {
                answered = true;
                last_answered |= waiter.last;
                let _ = waiter.reply.send(outcome);
            } else {
                kept.push(waiter);
            }
        }
        self.waiters = kept;
        // A run whose reading failed ends with it: its consumer has been
        // told, and nothing reads on for a run no one is following.
        let failed = answered && !matches!(outcome, RunReading::Read { .. });
        if failed || (last_answered && self.run.as_ref().is_some_and(|run| run.closing)) {
            self.run = None;
            self.run_next = None;
            self.run_due = false;
        }
    }

    /// Examine a reading just stored, through the one examination pass. It
    /// is already stored: a failure here is logged and never fails it.
    pub(super) fn start_examine(&mut self, reading: ReadingId, taken_at: f64, started: Instant) {
        let db = self.db.clone();
        let game_data = self.game_data.clone();
        let now = self.now_epoch();
        let task = tokio::task::spawn_blocking(move || {
            proposals::examine(&db, game_data, Which::Reading(reading), now)
        });
        self.work = Some(Work::Examine {
            reading,
            taken_at,
            started,
            task,
        });
    }

    pub(super) fn examined(
        &mut self,
        reading: ReadingId,
        taken_at: f64,
        started: Instant,
        examined: Result<Result<Option<Examined>, AccountError>, JoinError>,
    ) {
        let readable = match examined {
            Ok(Ok(Some(examined))) => {
                if examined.added > 0 {
                    self.mark_changed();
                }
                !examined.passed
            }
            Ok(Ok(None)) => true,
            Ok(Err(error)) => {
                // Left unexamined: the next start examines it again.
                tracing::error!(target: "eo::account", %error, "a reading could not be examined");
                false
            }
            Err(error) => {
                tracing::error!(target: "eo::account", %error, "examining a reading stopped");
                false
            }
        };
        // A run is never told nothing was opened when its step could not
        // be read: the reading stands, and the run hears it was unreadable.
        self.answer_waiters(
            started,
            if readable {
                RunReading::Read { reading, taken_at }
            } else {
                RunReading::Unreadable
            },
        );
    }

    /// Examine the oldest reading a previous run left unexamined, through
    /// the same pass: one reading per piece of work, so a reading falling
    /// due waits for at most one examination.
    pub(super) fn start_catch_up(&mut self) {
        let db = self.db.clone();
        let game_data = self.game_data.clone();
        let now = self.now_epoch();
        let task = tokio::task::spawn_blocking(move || {
            proposals::examine(&db, game_data, Which::Oldest, now)
        });
        self.work = Some(Work::CatchUp { task });
    }

    pub(super) fn caught_up(
        &mut self,
        pass: Result<Result<Option<Examined>, AccountError>, JoinError>,
    ) {
        match pass {
            Ok(Ok(Some(examined))) => {
                if examined.added > 0 {
                    self.mark_changed();
                }
                // The next one, once whatever else is due has gone.
                self.catch_up = true;
            }
            Ok(Ok(None)) => {}
            Ok(Err(error)) => {
                // Left unexamined, for the next start.
                tracing::error!(target: "eo::account", %error, "the readings left unexamined could not be examined");
            }
            Err(error) => {
                tracing::error!(target: "eo::account", %error, "examining the readings left unexamined stopped");
            }
        }
    }
}
