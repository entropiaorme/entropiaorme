//! The skill gain tracker: records chat-log skill events
//! during tracking sessions into `skill_gains`, with the gain's TT
//! value computed off the calibrated level, and keeps the calibration
//! current between full scans by appending an incremental point per
//! recorded gain.
//!
//! Subscriptions are permanent (taken at construction, unlike the
//! hunt tracker's per-session set) and gate on the active flag the
//! session lifecycle events flip. A pending codex claim suppresses
//! the next matching gain so the in-game skill-up the claim produces
//! is not double-counted alongside the ledger entry the claim already
//! recorded; the suppression is consumed on sight and honoured only
//! inside its expiry window.
//!
//! The original keeps this state lock-free (its mutations are
//! bus-thread-only in practice); the port owns it under one
//! poison-tolerant mutex, with the database statements issued after
//! the decision is captured. The original's logging is omitted.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::bus_events::BusEvent;
use crate::character_calc::ATTRIBUTE_SKILLS;
use crate::chatlog_time::ChatLogClock;
use crate::clock::Clock;
use crate::db::Db;
use crate::event_bus::{EventBus, Registration, Topic};
use crate::time::{naive_to_epoch, parse_timestamp_str};
use crate::tt_value_curve::tt_value_of_gain;

/// Seconds a registered codex-claim suppression stays armed.
pub const SUPPRESS_TIMEOUT_SECONDS: f64 = 30.0;

#[derive(Default)]
struct SkillState {
    active: bool,
    session_id: Option<String>,
    /// Per-session totals: name -> level amount.
    session_skills: BTreeMap<String, f64>,
    /// Per-session totals: name -> TT PED.
    session_skill_tt: BTreeMap<String, f64>,
    /// Pending codex-claim suppressions: name -> expiry epoch.
    suppressed_claims: BTreeMap<String, f64>,
}

pub struct SkillTracker {
    db: Db,
    clock: Arc<dyn Clock>,
    /// The base chat-log readings resolve against; see
    /// [`crate::chatlog_time`]. Shared with the watcher publishing them.
    chatlog_clock: ChatLogClock,
    state: Mutex<SkillState>,
    /// Held for the lifetime of the tracker: the subscriptions are
    /// permanent, exactly as the original subscribes once in its
    /// constructor and never unsubscribes.
    _subscriptions: Mutex<Vec<(Topic, Registration)>>,
}

impl SkillTracker {
    pub fn new(
        bus: &Arc<EventBus>,
        db: Db,
        clock: Arc<dyn Clock>,
        chatlog_clock: ChatLogClock,
    ) -> Arc<Self> {
        let tracker = Arc::new(Self {
            db,
            clock,
            chatlog_clock,
            state: Mutex::new(SkillState::default()),
            _subscriptions: Mutex::new(Vec::new()),
        });
        type Handler = fn(&SkillTracker, &BusEvent);
        let pairs: [(Topic, Handler); 3] = [
            (Topic::SkillGain, Self::on_skill_gain),
            (Topic::SessionStarted, Self::on_session_start),
            (Topic::SessionStopped, Self::on_session_stop),
        ];
        let mut subscriptions = Vec::new();
        for (topic, handler) in pairs {
            let subscriber = tracker.clone();
            let registration = bus.subscribe(topic, move |data| handler(&subscriber, data));
            subscriptions.push((topic, registration));
        }
        *tracker._subscriptions.lock().expect("subscriptions") = subscriptions;
        tracker
    }

    /// The state guard, tolerating poison for the same reason the
    /// hunt tracker does: a contained panic must not brick the
    /// service.
    fn lock_state(&self) -> std::sync::MutexGuard<'_, SkillState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Register a pending codex claim: suppress the next matching
    /// skill gain within the timeout window.
    pub fn suppress_next(&self, skill_name: &str, timeout_seconds: f64) {
        let expiry = naive_to_epoch(self.clock.now()) + timeout_seconds;
        self.lock_state()
            .suppressed_claims
            .insert(skill_name.to_string(), expiry);
    }

    fn on_session_start(&self, event: &BusEvent) {
        let BusEvent::SessionStarted(payload) = event else {
            return;
        };
        let mut state = self.lock_state();
        state.active = true;
        state.session_id = Some(payload.session_id.clone());
        state.session_skills.clear();
        state.session_skill_tt.clear();
        // A suppression armed in a prior session must not carry into
        // this one.
        state.suppressed_claims.clear();
    }

    fn on_session_stop(&self, _event: &BusEvent) {
        let mut state = self.lock_state();
        state.active = false;
        state.session_id = None;
        // Drop any still-armed codex suppression so it cannot bleed
        // into the next session.
        state.suppressed_claims.clear();
    }

    fn on_skill_gain(&self, event: &BusEvent) {
        let BusEvent::SkillGain(payload) = event else {
            return;
        };
        // Capture the decision under the guard; the database
        // statements run after release, then the totals land.
        let (session_id, skill_name, amount, ts_epoch) = {
            let mut state = self.lock_state();
            if !state.active {
                return;
            }
            let Some(session_id) = state.session_id.clone() else {
                return;
            };
            let skill_name = payload.skill_name.clone();
            let amount = payload.amount;
            // Bus timestamps are the watcher's isoformat strings; the
            // original's float passthrough is kept for numeric stamps.
            let ts_epoch = match parse_timestamp_str(&payload.timestamp) {
                Some(reading) => self.chatlog_clock.resolve_epoch(reading),
                None => match payload.timestamp.trim().parse::<f64>() {
                    Ok(numeric) => numeric,
                    Err(_) => return,
                },
            };

            // The suppression is consumed on sight; only an unexpired
            // one actually swallows the gain.
            if let Some(expiry) = state.suppressed_claims.remove(&skill_name) {
                if naive_to_epoch(self.clock.now()) < expiry {
                    return;
                }
            }
            (session_id, skill_name, amount, ts_epoch)
        };

        let old_level = {
            let skill_name = skill_name.clone();
            self.db
                .with_reader_blocking(move |conn| {
                    use rusqlite::OptionalExtension as _;
                    Ok(conn
                        .query_row(
                            "SELECT level FROM skill_calibrations WHERE skill_name = ? \
                             ORDER BY scanned_at DESC LIMIT 1",
                            rusqlite::params![skill_name],
                            |row| row.get::<_, f64>(0),
                        )
                        .optional()?)
                })
                .ok()
                .flatten()
        };

        let is_attribute = ATTRIBUTE_SKILLS.contains(&skill_name.as_str());
        let mut ped_value: Option<f64> = None;
        if let Some(old_level) = old_level {
            let new_level = old_level + amount;
            // Only regular skills price through the curve: no
            // attribute curve exists yet, exactly as the original.
            if !is_attribute {
                ped_value = Some(tt_value_of_gain(old_level, new_level));
            }
            let result = {
                let skill_name = skill_name.clone();
                self.db.with_writer_blocking(move |conn| {
                    conn.execute(
                        "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                         VALUES (?, ?, 'chatlog', ?)",
                        rusqlite::params![skill_name, new_level, ts_epoch],
                    )?;
                    Ok(())
                })
            };
            // The original's raise aborts the rest of the handler
            // (contained by the bus): no gain row, no totals.
            if result.is_err() {
                return;
            }
        }

        let result = {
            let skill_name = skill_name.clone();
            self.db.with_writer_blocking(move |conn| {
                // The session's current context, read inside the same
                // write as the insert. Contexts are append-only per
                // session, so the newest IS the one in force: exact,
                // and never a timestamp comparison (gain timestamps
                // carry in-game server time, interval bounds wall-clock).
                // id-order: insertion (contexts append in process order).
                use rusqlite::OptionalExtension as _;
                let context_id: Option<i64> = conn
                    .query_row(
                        "SELECT id FROM session_contexts WHERE session_id = ? \
                         ORDER BY id DESC LIMIT 1",
                        rusqlite::params![session_id],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?;
                conn.execute(
                    "INSERT INTO skill_gains \
                     (session_id, timestamp, skill_name, amount, ped_value, context_id) \
                     VALUES (?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        session_id, ts_epoch, skill_name, amount, ped_value, context_id
                    ],
                )?;
                Ok(())
            })
        };
        if result.is_err() {
            return;
        }

        let mut state = self.lock_state();
        *state
            .session_skills
            .entry(skill_name.clone())
            .or_insert(0.0) += amount;
        if let Some(ped_value) = ped_value {
            *state.session_skill_tt.entry(skill_name).or_insert(0.0) += ped_value;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::bus_events::{SessionLifecyclePayload, SkillGainPayload, SkillGainTag};
    use crate::clock::MockClock;
    use crate::db::Db;

    struct Rig {
        _dir: tempfile::TempDir,
        runtime: tokio::runtime::Runtime,
        bus: Arc<EventBus>,
        clock: Arc<MockClock>,
        db: Db,
        tracker: Arc<SkillTracker>,
    }

    fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let db = runtime
            .block_on(Db::open(&dir.path().join("entropia_orme.db")))
            .unwrap();
        let bus = Arc::new(EventBus::new());
        let clock = Arc::new(MockClock::new(None, 0.0));
        let tracker =
            SkillTracker::new(&bus, db.clone(), clock.clone(), ChatLogClock::host_local());
        Rig {
            _dir: dir,
            runtime,
            bus,
            clock,
            db,
            tracker,
        }
    }

    impl Rig {
        fn start_session(&self) {
            self.bus
                .publish(&BusEvent::SessionStarted(SessionLifecyclePayload {
                    session_id: "s1".into(),
                }));
        }

        fn gain(&self, name: &str, amount: f64, ts: &str) {
            self.bus.publish(&BusEvent::SkillGain(SkillGainPayload {
                kind: SkillGainTag,
                timestamp: ts.into(),
                amount,
                skill_name: name.into(),
            }));
        }

        /// Mint a context row for the session, as the tracker's segment
        /// engine does when a declaration changes.
        fn mint_context(&self) -> i64 {
            self.runtime.block_on(async {
                self.db
                    .with_writer(|conn| {
                        conn.execute(
                            "INSERT INTO tracking_sessions (id, started_at, is_active) \
                             VALUES ('s1', 1000.0, 1) ON CONFLICT(id) DO NOTHING",
                            [],
                        )?;
                        conn.execute(
                            "INSERT INTO session_contexts (session_id, created_at) \
                             VALUES ('s1', 1000.0)",
                            [],
                        )?;
                        Ok(conn.last_insert_rowid())
                    })
                    .await
                    .unwrap()
            })
        }

        /// Every gain's context stamp, oldest first.
        fn context_stamps(&self) -> Vec<Option<i64>> {
            self.runtime.block_on(async {
                self.db
                    .with_reader(|conn| {
                        let mut stmt =
                            conn.prepare("SELECT context_id FROM skill_gains ORDER BY id")?;
                        let rows = stmt.query_map([], |row| row.get::<_, Option<i64>>(0))?;
                        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
                    })
                    .await
                    .unwrap()
            })
        }

        fn gains_count(&self) -> i64 {
            self.runtime.block_on(async {
                self.db
                    .with_reader(|conn| {
                        Ok(
                            conn.query_row("SELECT COUNT(*) FROM skill_gains", [], |row| {
                                row.get::<_, i64>(0)
                            })?,
                        )
                    })
                    .await
                    .unwrap()
            })
        }

        fn calibrations(&self, name: &str) -> Vec<(f64, String)> {
            let name = name.to_string();
            self.runtime.block_on(async {
                self.db
                    .with_reader(move |conn| {
                        let mut stmt = conn.prepare(
                            "SELECT level, source FROM skill_calibrations WHERE skill_name = ? \
                             ORDER BY id",
                        )?;
                        let rows = stmt.query_map(rusqlite::params![name], |row| {
                            Ok((row.get::<_, f64>(0)?, row.get::<_, String>(1)?))
                        })?;
                        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
                    })
                    .await
                    .unwrap()
            })
        }

        fn seed_calibration(&self, name: &str, level: f64, scanned_at: f64) {
            let name = name.to_string();
            self.runtime.block_on(async {
                self.db
                    .with_writer(move |conn| {
                        conn.execute(
                            "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                             VALUES (?, ?, 'scan', ?)",
                            rusqlite::params![name, level, scanned_at],
                        )?;
                        Ok(())
                    })
                    .await
                    .unwrap();
            });
        }

        fn last_gain(&self) -> (String, f64, Option<f64>, f64) {
            self.runtime.block_on(async {
                self.db
                    .with_reader(|conn| {
                        // id-order: insertion (the gain this test just wrote).
                        Ok(conn.query_row(
                            "SELECT skill_name, amount, ped_value, timestamp FROM skill_gains \
                             ORDER BY id DESC LIMIT 1",
                            [],
                            |row| {
                                Ok((
                                    row.get::<_, String>(0)?,
                                    row.get::<_, f64>(1)?,
                                    row.get::<_, Option<f64>>(2)?,
                                    row.get::<_, f64>(3)?,
                                ))
                            },
                        )?)
                    })
                    .await
                    .unwrap()
            })
        }
    }

    #[test]
    fn gains_record_only_inside_an_active_session() {
        let rig = rig();
        rig.gain("Rifle", 0.5, "2026-01-01T00:00:01");
        assert_eq!(rig.gains_count(), 0, "idle gains never record");

        rig.start_session();
        rig.gain("Rifle", 0.5, "2026-01-01T00:00:02");
        assert_eq!(rig.gains_count(), 1);
        let (name, amount, ped, ts) = rig.last_gain();
        assert_eq!(name, "Rifle");
        assert_eq!(amount, 0.5);
        assert_eq!(ped, None, "no calibration: no TT value");
        assert_eq!(
            ts,
            naive_to_epoch(
                chrono::NaiveDateTime::parse_from_str("2026-01-01T00:00:02", "%Y-%m-%dT%H:%M:%S")
                    .unwrap()
            )
        );
        assert!(
            rig.calibrations("Rifle").is_empty(),
            "an uncalibrated skill never gains a calibration point"
        );

        rig.bus
            .publish(&BusEvent::SessionStopped(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));
        rig.gain("Rifle", 0.5, "2026-01-01T00:00:03");
        assert_eq!(rig.gains_count(), 1, "stopped gains never record");
    }

    #[test]
    fn calibrated_gains_price_and_chain_the_calibration() {
        let rig = rig();
        rig.seed_calibration("Rifle", 100.0, 50.0);
        rig.start_session();

        rig.gain("Rifle", 0.5, "2026-01-01T00:00:01");
        let (_, _, ped, _) = rig.last_gain();
        assert_eq!(ped, Some(tt_value_of_gain(100.0, 100.5)));
        assert_eq!(
            rig.calibrations("Rifle"),
            vec![(100.0, "scan".to_string()), (100.5, "chatlog".to_string())]
        );

        // The next gain chains off the newest calibration point.
        rig.gain("Rifle", 0.25, "2026-01-01T00:00:05");
        let (_, _, ped, _) = rig.last_gain();
        assert_eq!(ped, Some(tt_value_of_gain(100.5, 100.75)));
        assert_eq!(rig.calibrations("Rifle").len(), 3);

        // Totals accumulate in memory.
        let state = rig.tracker.lock_state();
        assert_eq!(state.session_skills["Rifle"], 0.75);
        assert_eq!(
            state.session_skill_tt["Rifle"],
            tt_value_of_gain(100.0, 100.5) + tt_value_of_gain(100.5, 100.75)
        );
    }

    #[test]
    fn attribute_gains_calibrate_without_pricing() {
        let rig = rig();
        rig.seed_calibration("Agility", 40.0, 50.0);
        rig.start_session();
        rig.gain("Agility", 0.25, "2026-01-01T00:00:01");
        let (_, _, ped, _) = rig.last_gain();
        assert_eq!(ped, None, "no attribute curve exists yet");
        assert_eq!(
            rig.calibrations("Agility"),
            vec![(40.0, "scan".to_string()), (40.25, "chatlog".to_string())]
        );
    }

    #[test]
    fn codex_suppression_swallows_once_within_its_window() {
        let rig = rig();
        rig.start_session();

        rig.tracker
            .suppress_next("Anatomy", SUPPRESS_TIMEOUT_SECONDS);
        rig.gain("Anatomy", 1.0, "2026-01-01T00:00:01");
        assert_eq!(rig.gains_count(), 0, "the claimed gain is swallowed");
        rig.gain("Anatomy", 1.0, "2026-01-01T00:00:02");
        assert_eq!(rig.gains_count(), 1, "the suppression was consumed");

        // Other skills pass through an armed suppression untouched.
        rig.tracker
            .suppress_next("Anatomy", SUPPRESS_TIMEOUT_SECONDS);
        rig.gain("Rifle", 1.0, "2026-01-01T00:00:03");
        assert_eq!(rig.gains_count(), 2);

        // The expiry compare is strict: a gain at exactly the
        // expiry instant processes.
        rig.tracker.suppress_next("Wounding", 5.0);
        rig.clock.advance(5.0).unwrap();
        rig.gain("Wounding", 1.0, "2026-01-01T00:00:03");
        assert_eq!(rig.gains_count(), 3, "the boundary instant is expired");

        // An expired suppression is consumed AND the gain processes.
        rig.clock.advance(26.0).unwrap();
        rig.gain("Anatomy", 1.0, "2026-01-01T00:00:04");
        assert_eq!(rig.gains_count(), 4, "expired suppression falls through");

        // Session boundaries clear armed suppressions both ways.
        rig.tracker
            .suppress_next("Anatomy", SUPPRESS_TIMEOUT_SECONDS);
        rig.start_session();
        rig.gain("Anatomy", 1.0, "2026-01-01T00:00:05");
        assert_eq!(rig.gains_count(), 5, "a restart drops prior suppressions");
        rig.tracker
            .suppress_next("Anatomy", SUPPRESS_TIMEOUT_SECONDS);
        rig.bus
            .publish(&BusEvent::SessionStopped(SessionLifecyclePayload {
                session_id: "s1".into(),
            }));
        rig.start_session();
        rig.gain("Anatomy", 1.0, "2026-01-01T00:00:06");
        assert_eq!(rig.gains_count(), 6, "a stop drops armed suppressions");
    }

    #[test]
    fn numeric_timestamps_pass_straight_through() {
        let rig = rig();
        rig.start_session();
        rig.bus.publish(&BusEvent::SkillGain(SkillGainPayload {
            kind: SkillGainTag,
            timestamp: "1735680000.5".into(),
            amount: 0.1,
            skill_name: "Rifle".into(),
        }));
        let (_, _, _, ts) = rig.last_gain();
        assert_eq!(ts, 1735680000.5);
    }

    /// The context stamp is a "from now on" declaration: each gain
    /// records what was in force when it was written, and a later
    /// change never reaches back to the rows already stamped. This is
    /// the whole reason events are stamped at insert rather than
    /// bucketed into intervals by timestamp afterwards.
    #[test]
    fn context_stamps_are_forward_only() {
        let rig = rig();
        rig.start_session();

        // No context minted yet: this gain predates the interval model.
        rig.gain("Bioregenesis", 0.1, "2026-01-01T00:00:01");

        let first = rig.mint_context();
        rig.gain("Bioregenesis", 0.1, "2026-01-01T00:00:02");
        rig.gain("Anatomy", 0.1, "2026-01-01T00:00:03");

        // A pill runs out: a fresh context, and the two rows already
        // written keep the one they were recorded under.
        let second = rig.mint_context();
        rig.gain("Anatomy", 0.1, "2026-01-01T00:00:04");

        assert_eq!(
            rig.context_stamps(),
            vec![None, Some(first), Some(first), Some(second)],
            "each gain keeps the context it was recorded under"
        );
    }
}
