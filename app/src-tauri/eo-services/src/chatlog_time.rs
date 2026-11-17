//! The chat-log time base: what a chat-log timestamp actually names,
//! and how it becomes an instant.
//!
//! The game stamps its chat log in the game server's own zone. Every
//! player reads the same digits for the same event whatever their own
//! machine is set to, so a reading is **not** a host wall-clock
//! reading and must not resolve through [`crate::time::resolve_local`].
//! Resolving one that way puts every logged event out by the
//! difference between the two zones, which is invisible until
//! something compares a logged instant against one the app itself
//! stamped: a British machine in summer against a UTC server reads
//! every kill and every loot clump an hour into the past, while the
//! session, interval, and quest-run boundaries beside them are stamped
//! from the host clock and are correct.
//!
//! Neither zone is knowable from configuration that stays true. The
//! host's offset moves with travel and with its own daylight saving;
//! the server's moves with the server's. What the conversion needs is
//! the server's offset from UTC, and that is directly observable: the
//! watcher tails from end-of-file on a short poll, so every line it
//! reads was appended moments earlier, and the gap between a reading
//! taken as UTC and the instant it was read is that offset plus the
//! small read lag. Real zone offsets are whole quarter-hours, so
//! snapping the gap to the nearest quarter-hour recovers the offset
//! exactly and discards the lag.
//!
//! Deriving against UTC rather than against the host zone is
//! deliberate: the host's zone has no part in what a server-stamped
//! reading means, and leaving it out of the conversion also leaves out
//! the host's own daylight-saving gaps and ambiguities, which would
//! otherwise smear an hour across a reading that was never in doubt.
//!
//! What the live tail settled is remembered across the app's runs
//! ([`OffsetMemory`]): when the offset settles again after a restart at
//! the value remembered, it is taken to have held since the remembered
//! offset did, so an instant from before the restart is still placed by
//! it. A different value has held only since it settled.
//!
//! [`ChatLogReading`] is a newtype for one reason: it is the only
//! thing [`crate::time::parse_timestamp_str`] hands back, and it
//! offers no route to an instant except [`ChatLogClock::resolve`]. A
//! later reader cannot reach for `resolve_local` out of habit and
//! quietly reintroduce the skew.

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, NaiveDateTime, TimeDelta, Utc};

/// Real UTC offsets are whole quarter-hours (+05:30, +05:45, +12:45
/// all exist), so a derived offset that is not one is not a zone.
const QUARTER_HOUR_SECONDS: i64 = 15 * 60;

/// The widest real offset from UTC in either direction (+14:00 for
/// Kiritimati, -12:00 for Baker Island).
const MAX_OFFSET_SECONDS: i64 = 14 * 3600;

/// How far a sample may sit from the quarter-hour it snaps to before
/// it stops being evidence. A live tail lands within a second or two;
/// anything further out was not read live (a suspended host, a stalled
/// share, a writer that paused mid-flush) and says nothing true about
/// the server's zone.
const MAX_READ_LAG_SECONDS: i64 = 120;

/// Consecutive agreeing samples needed to move an offset that has
/// already settled. One sample settles an unset clock, because running
/// knowingly wrong is worse than acting on good early evidence; moving
/// a settled one makes a daylight-saving step prove itself rather than
/// following a single odd reading.
const CONFIRMATIONS_TO_MOVE: u32 = 3;

/// One timestamp as the chat log wrote it: the server's wall-clock
/// digits, carrying no zone of their own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ChatLogReading(NaiveDateTime);

impl ChatLogReading {
    pub(crate) fn new(reading: NaiveDateTime) -> Self {
        Self(reading)
    }

    /// The digits the log showed, for rendering and for the payload
    /// strings that carry a reading onward verbatim. This is not a
    /// route to an instant: resolving it against any zone but the
    /// server's is exactly the mistake the newtype exists to prevent.
    pub fn as_naive(self) -> NaiveDateTime {
        self.0
    }
}

/// Which base a reading resolves against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Base {
    /// A reading is a host wall-clock reading. What every non-live
    /// surface wants: recorded lines replayed from a committed
    /// scenario are not a live tail, so no sample taken from them
    /// could say anything true about a server's zone, and pinning
    /// them to the host clock keeps a replay's output a property of
    /// the scenario rather than of the machine's calendar.
    HostLocal,
    /// A reading resolves against the server offset the live tail has
    /// derived, and against the host clock until the first sample
    /// lands (a few hundred milliseconds after the first logged line).
    Observed,
    /// A reading resolves against a known offset; nothing observed
    /// moves it.
    Pinned,
}

struct Candidate {
    offset_seconds: i64,
    agreements: u32,
}

/// A settled offset as remembered across the app's runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RememberedOffset {
    /// Seconds the server's wall clock runs ahead of UTC.
    pub offset_seconds: i64,
    /// Since when (unix seconds) it is known to have held.
    pub held_since: i64,
}

/// Where a settled offset is remembered between the app's runs. Called on
/// the live tail's own thread, and only when the offset settles or moves.
pub trait OffsetMemory: Send + Sync {
    /// The offset remembered from an earlier run, if any.
    fn recall(&self) -> Option<RememberedOffset>;
    /// Remember the offset held now, read live at `confirmed_at` (unix
    /// seconds).
    fn remember(&self, offset: RememberedOffset, confirmed_at: i64);
}

/// The offset remembered in the app's database
/// (`chat_log_time_base`, one row).
pub struct StoredOffsetMemory {
    db: crate::db::Db,
}

impl StoredOffsetMemory {
    pub fn new(db: crate::db::Db) -> Self {
        StoredOffsetMemory { db }
    }
}

impl OffsetMemory for StoredOffsetMemory {
    fn recall(&self) -> Option<RememberedOffset> {
        use rusqlite::OptionalExtension as _;
        let recalled = self.db.with_reader_blocking(|conn| {
            Ok(conn
                .query_row(
                    "SELECT offset_seconds, held_since FROM chat_log_time_base \
                     WHERE singleton = 1",
                    [],
                    |row| {
                        Ok(RememberedOffset {
                            offset_seconds: row.get(0)?,
                            held_since: row.get(1)?,
                        })
                    },
                )
                .optional()?)
        });
        match recalled {
            Ok(recalled) => recalled,
            Err(error) => {
                tracing::warn!(target: "eo::chatlog", %error, "the remembered chat-log time base could not be read");
                None
            }
        }
    }

    fn remember(&self, offset: RememberedOffset, confirmed_at: i64) {
        let written = self.db.with_writer_blocking(move |conn| {
            conn.execute(
                "INSERT INTO chat_log_time_base \
                     (singleton, offset_seconds, held_since, confirmed_at) \
                 VALUES (1, ?1, ?2, ?3) \
                 ON CONFLICT(singleton) DO UPDATE SET \
                     offset_seconds = excluded.offset_seconds, \
                     held_since = excluded.held_since, \
                     confirmed_at = excluded.confirmed_at",
                rusqlite::params![offset.offset_seconds, offset.held_since, confirmed_at],
            )?;
            Ok(())
        });
        if let Err(error) = written {
            tracing::warn!(target: "eo::chatlog", %error, "the chat-log time base could not be remembered");
        }
    }
}

struct Inner {
    base: Base,
    /// Seconds the server's wall clock runs ahead of UTC. Read on
    /// every resolve, so an atomic rather than a lock.
    offset_seconds: AtomicI64,
    /// Since when (unix seconds, read on the host's clock) the offset has
    /// been the one held now: the read that settled it or last moved it.
    /// The least instant for a pinned offset, which has always held.
    held_since: AtomicI64,
    settled: AtomicBool,
    candidate: Mutex<Option<Candidate>>,
    /// Where the settled offset is remembered between runs, if anywhere.
    memory: Option<Arc<dyn OffsetMemory>>,
}

/// The shared handle every chat-log consumer resolves through. Cloning
/// shares one derived offset, so the watcher's observations reach the
/// trackers that resolve the payloads it publishes.
#[derive(Clone)]
pub struct ChatLogClock {
    inner: Arc<Inner>,
}

impl ChatLogClock {
    fn with_base(
        base: Base,
        offset_seconds: i64,
        settled: bool,
        memory: Option<Arc<dyn OffsetMemory>>,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                base,
                offset_seconds: AtomicI64::new(offset_seconds),
                held_since: AtomicI64::new(i64::MIN),
                settled: AtomicBool::new(settled),
                candidate: Mutex::new(None),
                memory,
            }),
        }
    }

    /// The live tail's clock: host-local until its first accepted
    /// sample, then the derived server offset.
    pub fn observed() -> Self {
        Self::with_base(Base::Observed, 0, false, None)
    }

    /// [`ChatLogClock::observed`], remembering what it settles in
    /// `memory`, and taking an offset that settles at the value remembered
    /// to have held since the remembered one did.
    pub fn observed_remembering(memory: Arc<dyn OffsetMemory>) -> Self {
        Self::with_base(Base::Observed, 0, false, Some(memory))
    }

    /// The historical base, for recorded lines and for any surface
    /// where "live" has no meaning.
    pub fn host_local() -> Self {
        Self::with_base(Base::HostLocal, 0, false, None)
    }

    /// A clock pinned to a known server offset from UTC, in seconds.
    /// A test seam for the cases that turn on a specific offset: the
    /// app derives its offset by observation and exposes no setting
    /// for one.
    pub fn pinned(offset_seconds: i64) -> Self {
        Self::with_base(Base::Pinned, offset_seconds, true, None)
    }

    /// The derived server offset from UTC in seconds, once one has
    /// settled. `None` while a reading still resolves host-local.
    pub fn server_offset_seconds(&self) -> Option<i64> {
        (self.inner.base != Base::HostLocal && self.inner.settled.load(Ordering::Acquire))
            .then(|| self.inner.offset_seconds.load(Ordering::Acquire))
    }

    /// Since when (unix seconds) the offset held now is known to have
    /// held: the live read that settled it or last moved it, or, when it
    /// settled at the value an earlier run remembered, since that one had
    /// held; `None` while no offset has settled. An instant placed in time before it may have
    /// been logged under another offset (a daylight-saving step on the
    /// server, or one the app was not running to see).
    pub fn offset_held_since(&self) -> Option<i64> {
        self.server_offset_seconds()
            .map(|_| self.inner.held_since.load(Ordering::Acquire))
    }

    /// Feed one live reading and the instant it was read. Samples that
    /// do not look like a live read of a real zone are discarded, so a
    /// bad one costs nothing and never unsettles a good offset.
    pub fn observe(&self, reading: ChatLogReading, read_at: DateTime<Utc>) {
        if self.inner.base != Base::Observed {
            return;
        }
        let gap = reading
            .0
            .and_utc()
            .signed_duration_since(read_at)
            .num_seconds();
        let snapped = snap_to_quarter_hour(gap);
        if snapped.abs() > MAX_OFFSET_SECONDS || (gap - snapped).abs() > MAX_READ_LAG_SECONDS {
            return;
        }

        let mut candidate = self
            .inner
            .candidate
            .lock()
            .expect("chat-log offset candidate");
        if !self.inner.settled.load(Ordering::Acquire) {
            // Settled at the value an earlier run remembered: taken to have
            // held since that one had.
            let recalled = self
                .inner
                .memory
                .as_ref()
                .and_then(|memory| memory.recall());
            let held_since = match recalled {
                Some(kept) if kept.offset_seconds == snapped => {
                    kept.held_since.min(read_at.timestamp())
                }
                _ => read_at.timestamp(),
            };
            self.inner.offset_seconds.store(snapped, Ordering::Release);
            self.inner.held_since.store(held_since, Ordering::Release);
            self.inner.settled.store(true, Ordering::Release);
            *candidate = None;
            self.remember(snapped, held_since, read_at);
            return;
        }
        if snapped == self.inner.offset_seconds.load(Ordering::Acquire) {
            *candidate = None;
            return;
        }
        match candidate.as_mut() {
            Some(pending) if pending.offset_seconds == snapped => {
                pending.agreements += 1;
                if pending.agreements >= CONFIRMATIONS_TO_MOVE {
                    self.inner
                        .held_since
                        .store(read_at.timestamp(), Ordering::Release);
                    self.inner.offset_seconds.store(snapped, Ordering::Release);
                    *candidate = None;
                    self.remember(snapped, read_at.timestamp(), read_at);
                }
            }
            _ => {
                *candidate = Some(Candidate {
                    offset_seconds: snapped,
                    agreements: 1,
                })
            }
        }
    }

    fn remember(&self, offset_seconds: i64, held_since: i64, read_at: DateTime<Utc>) {
        if let Some(memory) = &self.inner.memory {
            memory.remember(
                RememberedOffset {
                    offset_seconds,
                    held_since,
                },
                read_at.timestamp(),
            );
        }
    }

    /// The instant a reading names.
    pub fn resolve(&self, reading: ChatLogReading) -> DateTime<Utc> {
        match self.server_offset_seconds() {
            Some(offset) => reading.0.and_utc() - TimeDelta::seconds(offset),
            None => crate::time::resolve_local(reading.0),
        }
    }

    /// The reading's instant as the epoch-seconds float the database
    /// stores.
    pub fn resolve_epoch(&self, reading: ChatLogReading) -> f64 {
        crate::time::instant_to_epoch(self.resolve(reading))
    }
}

impl Default for ChatLogClock {
    fn default() -> Self {
        Self::host_local()
    }
}

fn snap_to_quarter_hour(seconds: i64) -> i64 {
    let quarters = (seconds as f64 / QUARTER_HOUR_SECONDS as f64).round() as i64;
    quarters * QUARTER_HOUR_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reading(raw: &str) -> ChatLogReading {
        ChatLogReading::new(
            NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S").expect("reading"),
        )
    }

    fn instant(raw: &str) -> DateTime<Utc> {
        NaiveDateTime::parse_from_str(raw, "%Y-%m-%dT%H:%M:%S")
            .expect("instant")
            .and_utc()
    }

    #[test]
    fn a_utc_server_read_live_derives_a_zero_offset() {
        let clock = ChatLogClock::observed();
        // The log says 09:35:26 and we read it as it lands. The
        // server is UTC, whatever the host machine is set to.
        clock.observe(
            reading("2026-08-22T09:35:26"),
            instant("2026-08-22T09:35:26"),
        );
        assert_eq!(clock.server_offset_seconds(), Some(0));
        assert_eq!(
            clock.resolve(reading("2026-08-22T09:35:26")),
            instant("2026-08-22T09:35:26")
        );
    }

    #[test]
    fn a_whole_hour_server_offset_resolves_back_to_the_true_instant() {
        let clock = ChatLogClock::observed();
        // A server two hours ahead of UTC: its 11:35 is 09:35 UTC.
        clock.observe(
            reading("2026-08-22T11:35:26"),
            instant("2026-08-22T09:35:25"),
        );
        assert_eq!(clock.server_offset_seconds(), Some(2 * 3600));
        assert_eq!(
            clock.resolve(reading("2026-08-22T11:35:26")),
            instant("2026-08-22T09:35:26")
        );
    }

    #[test]
    fn a_quarter_hour_server_offset_survives_snapping() {
        let clock = ChatLogClock::observed();
        clock.observe(
            reading("2026-08-22T15:05:26"),
            instant("2026-08-22T09:35:25"),
        );
        assert_eq!(clock.server_offset_seconds(), Some(5 * 3600 + 30 * 60));
    }

    #[test]
    fn the_read_lag_is_discarded_not_carried_into_the_offset() {
        let clock = ChatLogClock::observed();
        // Read a minute and a half late: still the same zone.
        clock.observe(
            reading("2026-08-22T09:35:26"),
            instant("2026-08-22T09:36:56"),
        );
        assert_eq!(clock.server_offset_seconds(), Some(0));
    }

    #[test]
    fn a_sample_that_is_not_a_live_read_is_discarded() {
        let clock = ChatLogClock::observed();
        // Mid-bucket: not a plausible zone plus a small lag.
        clock.observe(
            reading("2026-08-22T09:28:00"),
            instant("2026-08-22T09:35:26"),
        );
        assert_eq!(clock.server_offset_seconds(), None);
        // With nothing derived, a reading still resolves the way it
        // always did rather than silently becoming UTC.
        assert_eq!(
            clock.resolve(reading("2026-08-22T09:35:26")),
            crate::time::resolve_local(reading("2026-08-22T09:35:26").as_naive())
        );
    }

    #[test]
    fn an_absurd_gap_cannot_settle_an_offset() {
        let clock = ChatLogClock::observed();
        clock.observe(
            reading("2027-08-22T09:35:26"),
            instant("2026-08-22T09:35:26"),
        );
        assert_eq!(clock.server_offset_seconds(), None);
    }

    #[test]
    fn one_odd_sample_does_not_move_a_settled_offset() {
        let clock = ChatLogClock::observed();
        clock.observe(
            reading("2026-08-22T09:35:26"),
            instant("2026-08-22T09:35:26"),
        );
        clock.observe(
            reading("2026-08-22T10:35:26"),
            instant("2026-08-22T09:35:26"),
        );
        assert_eq!(clock.server_offset_seconds(), Some(0));
    }

    #[test]
    fn a_daylight_saving_step_moves_the_offset_once_it_is_confirmed() {
        let clock = ChatLogClock::observed();
        clock.observe(
            reading("2026-10-25T01:00:00"),
            instant("2026-10-25T01:00:00"),
        );
        assert_eq!(clock.server_offset_seconds(), Some(0));
        for minute in 0..CONFIRMATIONS_TO_MOVE {
            let read_at = instant("2026-10-25T02:00:00") + TimeDelta::minutes(minute.into());
            clock.observe(
                ChatLogReading::new((read_at + TimeDelta::hours(1)).naive_utc()),
                read_at,
            );
        }
        assert_eq!(clock.server_offset_seconds(), Some(3600));
    }

    #[test]
    fn an_interrupted_run_of_samples_restarts_the_confirmation() {
        let clock = ChatLogClock::observed();
        clock.observe(
            reading("2026-08-22T09:00:00"),
            instant("2026-08-22T09:00:00"),
        );
        clock.observe(
            reading("2026-08-22T10:00:01"),
            instant("2026-08-22T09:00:01"),
        );
        clock.observe(
            reading("2026-08-22T11:00:02"),
            instant("2026-08-22T09:00:02"),
        );
        clock.observe(
            reading("2026-08-22T10:00:03"),
            instant("2026-08-22T09:00:03"),
        );
        assert_eq!(clock.server_offset_seconds(), Some(0));
    }

    #[test]
    fn a_host_local_clock_never_observes() {
        let clock = ChatLogClock::host_local();
        clock.observe(
            reading("2026-08-22T11:35:26"),
            instant("2026-08-22T09:35:26"),
        );
        assert_eq!(clock.server_offset_seconds(), None);
        assert_eq!(
            clock.resolve(reading("2026-08-22T09:35:26")),
            crate::time::resolve_local(reading("2026-08-22T09:35:26").as_naive())
        );
    }

    #[test]
    fn a_pinned_clock_ignores_what_the_tail_sees() {
        let clock = ChatLogClock::pinned(3600);
        clock.observe(
            reading("2026-08-22T09:35:26"),
            instant("2026-08-22T09:35:26"),
        );
        assert_eq!(clock.server_offset_seconds(), Some(3600));
        assert_eq!(
            clock.resolve(reading("2026-08-22T10:35:26")),
            instant("2026-08-22T09:35:26")
        );
    }

    #[test]
    fn an_offset_is_known_to_hold_from_the_read_that_settled_or_moved_it() {
        let clock = ChatLogClock::observed();
        assert_eq!(clock.offset_held_since(), None);
        let settled_at = instant("2026-08-22T09:35:26");
        clock.observe(reading("2026-08-22T09:35:26"), settled_at);
        assert_eq!(clock.offset_held_since(), Some(settled_at.timestamp()));
        // An agreeing sample later changes nothing.
        clock.observe(
            reading("2026-08-22T10:00:00"),
            instant("2026-08-22T10:00:00"),
        );
        assert_eq!(clock.offset_held_since(), Some(settled_at.timestamp()));
        // A step the samples confirm moves it, from the confirming read.
        for minute in 1..=3 {
            clock.observe(
                reading(&format!("2026-08-22T12:0{minute}:00")),
                instant(&format!("2026-08-22T11:0{minute}:00")),
            );
        }
        assert_eq!(clock.server_offset_seconds(), Some(3_600));
        assert_eq!(
            clock.offset_held_since(),
            Some(instant("2026-08-22T11:03:00").timestamp())
        );
        assert_eq!(ChatLogClock::pinned(0).offset_held_since(), Some(i64::MIN));
        assert_eq!(ChatLogClock::host_local().offset_held_since(), None);
    }

    /// A memory held in the test, remembering what it was told.
    #[derive(Default)]
    struct Remembered(Mutex<Option<(RememberedOffset, i64)>>);

    impl OffsetMemory for Remembered {
        fn recall(&self) -> Option<RememberedOffset> {
            self.0.lock().unwrap().map(|(offset, _)| offset)
        }
        fn remember(&self, offset: RememberedOffset, confirmed_at: i64) {
            *self.0.lock().unwrap() = Some((offset, confirmed_at));
        }
    }

    fn remembering(kept: Option<RememberedOffset>) -> (ChatLogClock, Arc<Remembered>) {
        let memory = Arc::new(Remembered(Mutex::new(kept.map(|offset| (offset, 0)))));
        (ChatLogClock::observed_remembering(memory.clone()), memory)
    }

    /// After a restart, an offset that settles at the value remembered has
    /// held since the remembered one did; a different one, or one with
    /// nothing remembered, only since it settled. What settles is
    /// remembered for the next run.
    #[test]
    fn a_remembered_offset_carries_across_a_restart_only_at_the_same_value() {
        let settled_at = instant("2026-08-22T09:35:26");
        let earlier = instant("2026-08-21T18:00:00").timestamp();
        let kept = RememberedOffset {
            offset_seconds: 0,
            held_since: earlier,
        };

        let (clock, memory) = remembering(Some(kept));
        clock.observe(reading("2026-08-22T09:35:26"), settled_at);
        assert_eq!(clock.offset_held_since(), Some(earlier));
        assert_eq!(
            *memory.0.lock().unwrap(),
            Some((kept, settled_at.timestamp()))
        );

        // A different offset: an hour ahead of what was remembered.
        let (clock, memory) = remembering(Some(RememberedOffset {
            offset_seconds: 3_600,
            held_since: earlier,
        }));
        clock.observe(reading("2026-08-22T09:35:26"), settled_at);
        assert_eq!(clock.offset_held_since(), Some(settled_at.timestamp()));
        assert_eq!(
            memory.0.lock().unwrap().map(|(offset, _)| offset),
            Some(RememberedOffset {
                offset_seconds: 0,
                held_since: settled_at.timestamp(),
            })
        );

        // A first run: nothing remembered.
        let (clock, memory) = remembering(None);
        clock.observe(reading("2026-08-22T09:35:26"), settled_at);
        assert_eq!(clock.offset_held_since(), Some(settled_at.timestamp()));
        assert!(memory.0.lock().unwrap().is_some());
    }

    /// The database remembers the offset between runs.
    #[tokio::test]
    async fn the_database_remembers_the_offset() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(&dir.path().join("clock.db"))
            .await
            .unwrap();
        let memory = StoredOffsetMemory::new(db);
        let kept = RememberedOffset {
            offset_seconds: 7_200,
            held_since: 1_000,
        };
        let recalled = tokio::task::spawn_blocking(move || {
            let before = memory.recall();
            memory.remember(kept, 2_000);
            memory.remember(kept, 3_000);
            (before, memory.recall())
        })
        .await
        .unwrap();
        assert_eq!(recalled, (None, Some(kept)));
    }
}
