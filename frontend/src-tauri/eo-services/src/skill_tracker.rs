//! The skill gain tracker, ported from
//! the original Python implementation: records chat-log skill events
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

use serde_json::Value;
use sqlx::sqlite::SqlitePool;
use sqlx::Row;
use tokio::runtime::Handle;

use crate::character_calc::ATTRIBUTE_SKILLS;
use crate::clock::Clock;
use crate::event_bus::{EventBus, Registration, Topic};
use crate::tracker::{naive_to_epoch, parse_bus_timestamp};
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
    pool: SqlitePool,
    runtime: Handle,
    clock: Arc<dyn Clock>,
    state: Mutex<SkillState>,
    /// Held for the lifetime of the tracker: the subscriptions are
    /// permanent, exactly as the original subscribes once in its
    /// constructor and never unsubscribes.
    _subscriptions: Mutex<Vec<(Topic, Registration)>>,
}

impl SkillTracker {
    pub fn new(
        bus: &Arc<EventBus>,
        pool: SqlitePool,
        runtime: Handle,
        clock: Arc<dyn Clock>,
    ) -> Arc<Self> {
        let tracker = Arc::new(Self {
            pool,
            runtime,
            clock,
            state: Mutex::new(SkillState::default()),
            _subscriptions: Mutex::new(Vec::new()),
        });
        type Handler = fn(&SkillTracker, &Value);
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

    /// Bridge a database future from either calling context (the
    /// hunt tracker's dual shape).
    fn block_on<F: std::future::Future>(&self, future: F) -> F::Output {
        if Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| self.runtime.block_on(future))
        } else {
            self.runtime.block_on(future)
        }
    }

    /// Register a pending codex claim: suppress the next matching
    /// skill gain within the timeout window.
    pub fn suppress_next(&self, skill_name: &str, timeout_seconds: f64) {
        let expiry = naive_to_epoch(self.clock.now()) + timeout_seconds;
        self.lock_state()
            .suppressed_claims
            .insert(skill_name.to_string(), expiry);
    }

    fn on_session_start(&self, data: &Value) {
        let mut state = self.lock_state();
        state.active = true;
        state.session_id = data
            .get("session_id")
            .and_then(Value::as_str)
            .map(str::to_string);
        state.session_skills.clear();
        state.session_skill_tt.clear();
        // A suppression armed in a prior session must not carry into
        // this one.
        state.suppressed_claims.clear();
    }

    fn on_session_stop(&self, _data: &Value) {
        let mut state = self.lock_state();
        state.active = false;
        state.session_id = None;
        // Drop any still-armed codex suppression so it cannot bleed
        // into the next session.
        state.suppressed_claims.clear();
    }

    fn on_skill_gain(&self, data: &Value) {
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
            // The original indexes these keys (a missing one raises,
            // contained by the bus); the watcher always supplies them.
            let Some(skill_name) = data
                .get("skill_name")
                .and_then(Value::as_str)
                .map(str::to_string)
            else {
                return;
            };
            let Some(amount) = data.get("amount").and_then(Value::as_f64) else {
                return;
            };
            // Bus timestamps are the watcher's isoformat strings; the
            // original's float passthrough is kept for numeric stamps.
            let ts_epoch = match parse_bus_timestamp(data.get("timestamp")) {
                Some(instant) => naive_to_epoch(instant),
                None => match data.get("timestamp").and_then(Value::as_f64) {
                    Some(numeric) => numeric,
                    None => return,
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

        let old_level = self.block_on(async {
            sqlx::query(
                "SELECT level FROM skill_calibrations WHERE skill_name = ? \
                 ORDER BY scanned_at DESC LIMIT 1",
            )
            .bind(&skill_name)
            .fetch_optional(&self.pool)
            .await
            .ok()
            .flatten()
            .and_then(|row| row.try_get::<f64, _>(0).ok())
        });

        let is_attribute = ATTRIBUTE_SKILLS.contains(&skill_name.as_str());
        let mut ped_value: Option<f64> = None;
        if let Some(old_level) = old_level {
            let new_level = old_level + amount;
            // Only regular skills price through the curve: no
            // attribute curve exists yet, exactly as the original.
            if !is_attribute {
                ped_value = Some(tt_value_of_gain(old_level, new_level));
            }
            let result = self.block_on(async {
                sqlx::query(
                    "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                     VALUES (?, ?, 'chatlog', ?)",
                )
                .bind(&skill_name)
                .bind(new_level)
                .bind(ts_epoch)
                .execute(&self.pool)
                .await
            });
            // The original's raise aborts the rest of the handler
            // (contained by the bus): no gain row, no totals.
            if result.is_err() {
                return;
            }
        }

        let result = self.block_on(async {
            sqlx::query(
                "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(&session_id)
            .bind(ts_epoch)
            .bind(&skill_name)
            .bind(amount)
            .bind(ped_value)
            .execute(&self.pool)
            .await
        });
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
    use crate::clock::MockClock;
    use crate::db::{decoded_f64, Db};
    use serde_json::json;

    struct Rig {
        _dir: tempfile::TempDir,
        runtime: tokio::runtime::Runtime,
        bus: Arc<EventBus>,
        clock: Arc<MockClock>,
        pool: SqlitePool,
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
        let pool = db.pool().clone();
        let bus = Arc::new(EventBus::new());
        let clock = Arc::new(MockClock::new(None, 0.0));
        let tracker =
            SkillTracker::new(&bus, pool.clone(), runtime.handle().clone(), clock.clone());
        Rig {
            _dir: dir,
            runtime,
            bus,
            clock,
            pool,
            tracker,
        }
    }

    impl Rig {
        fn start_session(&self) {
            self.bus
                .publish(Topic::SessionStarted, &json!({"session_id": "s1"}));
        }

        fn gain(&self, name: &str, amount: f64, ts: &str) {
            self.bus.publish(
                Topic::SkillGain,
                &json!({"type": "skill_gain", "skill_name": name, "amount": amount,
                        "timestamp": ts}),
            );
        }

        fn gains_count(&self) -> i64 {
            self.runtime.block_on(async {
                sqlx::query("SELECT COUNT(*) FROM skill_gains")
                    .fetch_one(&self.pool)
                    .await
                    .unwrap()
                    .try_get(0)
                    .unwrap()
            })
        }

        fn calibrations(&self, name: &str) -> Vec<(f64, String)> {
            self.runtime.block_on(async {
                sqlx::query(
                    "SELECT level, source FROM skill_calibrations WHERE skill_name = ? \
                     ORDER BY id",
                )
                .bind(name)
                .fetch_all(&self.pool)
                .await
                .unwrap()
                .iter()
                .map(|row| (decoded_f64(row, 0), row.try_get(1).unwrap()))
                .collect()
            })
        }

        fn seed_calibration(&self, name: &str, level: f64, scanned_at: f64) {
            self.runtime.block_on(async {
                sqlx::query(
                    "INSERT INTO skill_calibrations (skill_name, level, source, scanned_at) \
                     VALUES (?, ?, 'scan', ?)",
                )
                .bind(name)
                .bind(level)
                .bind(scanned_at)
                .execute(&self.pool)
                .await
                .unwrap();
            });
        }

        fn last_gain(&self) -> (String, f64, Option<f64>, f64) {
            self.runtime.block_on(async {
                let row = sqlx::query(
                    "SELECT skill_name, amount, ped_value, timestamp FROM skill_gains \
                     ORDER BY id DESC LIMIT 1",
                )
                .fetch_one(&self.pool)
                .await
                .unwrap();
                (
                    row.try_get(0).unwrap(),
                    decoded_f64(&row, 1),
                    row.try_get(2).unwrap(),
                    decoded_f64(&row, 3),
                )
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
            .publish(Topic::SessionStopped, &json!({"session_id": "s1"}));
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
            .publish(Topic::SessionStopped, &json!({"session_id": "s1"}));
        rig.start_session();
        rig.gain("Anatomy", 1.0, "2026-01-01T00:00:06");
        assert_eq!(rig.gains_count(), 6, "a stop drops armed suppressions");
    }

    #[test]
    fn numeric_timestamps_pass_straight_through() {
        let rig = rig();
        rig.start_session();
        rig.bus.publish(
            Topic::SkillGain,
            &json!({"type": "skill_gain", "skill_name": "Rifle", "amount": 0.1,
                    "timestamp": 1735680000.5}),
        );
        let (_, _, _, ts) = rig.last_gain();
        assert_eq!(ts, 1735680000.5);
    }
}
