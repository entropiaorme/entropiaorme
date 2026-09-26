use chrono::NaiveDateTime;
use serde_json::Value;

use crate::bus_events::BusEvent;
use crate::chatlog_time::ChatLogClock;
use crate::event_bus::Topic;
use crate::harvest_yield::{HarvestYieldSource, HarvestYieldTier};

use super::actor::TrackerActor;
use super::time::{epoch_to_naive, parse_bus_timestamp, python_total_seconds};
use super::weapons::DamageEnhancerState;
use super::*;
use crate::bus_events::{
    ActiveHealToolChangedPayload, ActiveToolChangedPayload, EnhancerBreakPayload, EnhancerBreakTag,
    HotbarIntentPayload, HotbarItemKind, LootGroupPayload, LootItem, LootTag, TickFlushedPayload,
};
use crate::bus_events::{CombatPayload, GlobalPayload};
use crate::clock::MockClock;
use crate::cost_engine::cost_per_shot_from_props;
use crate::healing_profile::{HealingMode, HealingProfile};
use crate::ped::Ped;
use crate::time::{epoch_to_parts, naive_isoformat, naive_to_epoch, to_iso_utc};
use serde_json::json;
use std::sync::Mutex as StdMutex;

type CostScript = Arc<dyn Fn(&str) -> f64 + Send + Sync>;
type ProfileScript = Arc<dyn Fn(&str) -> EquipmentProfile + Send + Sync>;
type ManualMobScript = Arc<dyn Fn() -> Option<(String, String)> + Send + Sync>;

/// Closure-scripted equipment library for tests.
#[derive(Default)]
struct ScriptedEquipment {
    cost: Option<CostScript>,
    profile: Option<ProfileScript>,
    carried: Vec<CarriedWeaponProfile>,
    harvest_guardrail: Option<HarvestGuardrailTools>,
    looters: Option<crate::expected_hunting::HuntingLooterLevels>,
}

impl EquipmentLibrary for ScriptedEquipment {
    fn weapon_profile(&self, tool_name: &str) -> EquipmentProfile {
        self.profile.as_ref().and_then(|lookup| lookup(tool_name))
    }

    fn cost_per_shot(&self, tool_name: &str) -> f64 {
        self.cost
            .as_ref()
            .map(|lookup| lookup(tool_name))
            .unwrap_or(0.0)
    }

    fn carried_weapons(&self) -> Vec<CarriedWeaponProfile> {
        self.carried.clone()
    }

    fn resolve_harvest_guardrail(&self) -> Option<HarvestGuardrailTools> {
        self.harvest_guardrail.clone()
    }

    fn hunting_looter_levels(&self) -> crate::expected_hunting::HuntingLooterLevels {
        self.looters
            .unwrap_or(crate::expected_hunting::HuntingLooterLevels {
                animal: 0.0,
                mutant: 0.0,
                robot: 0.0,
            })
    }
}

/// Closure-scripted session-capture config for tests; unset fields
/// fall back to the inert defaults.
#[derive(Default)]
struct ScriptedConfig {
    session_name: Option<String>,
    session_definition_id: Option<i64>,
    skill_boost_percent: Option<i64>,
    manual_mob: Option<ManualMobScript>,
    blacklist: Vec<String>,
}

impl TrackingConfig for ScriptedConfig {
    fn session_name(&self) -> String {
        self.session_name.clone().unwrap_or_default()
    }

    fn session_definition_id(&self) -> Option<i64> {
        self.session_definition_id
    }

    fn declared_skill_boost_percent(&self) -> Option<i64> {
        self.skill_boost_percent
    }

    fn manual_mob(&self) -> Option<(String, String)> {
        self.manual_mob.as_ref().and_then(|f| f())
    }

    fn loot_filter_blacklist(&self) -> Vec<String> {
        self.blacklist.clone()
    }
}

pub(super) struct Rig {
    _dir: tempfile::TempDir,
    pub(super) runtime: tokio::runtime::Runtime,
    pub(super) bus: Arc<EventBus>,
    pub(super) clock: Arc<MockClock>,
    pub(super) db: Db,
}

pub(super) fn rig() -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let db = runtime
        .block_on(Db::open(&dir.path().join("entropia_orme.db")))
        .unwrap();
    Rig {
        _dir: dir,
        runtime,
        bus: Arc::new(EventBus::new()),
        clock: Arc::new(MockClock::new(None, 0.0)),
        db,
    }
}

impl Rig {
    pub(super) fn tracker(&self, providers: Providers) -> Arc<HuntTracker> {
        self.tracker_on_chatlog_clock(providers, ChatLogClock::host_local())
    }

    /// A tracker resolving chat-log readings against a known server
    /// offset, for the cases that turn on a logged instant landing
    /// where the app's own stamps can see it.
    fn tracker_on_chatlog_clock(
        &self,
        providers: Providers,
        chatlog_clock: ChatLogClock,
    ) -> Arc<HuntTracker> {
        self.runtime
            .block_on(HuntTracker::new(
                self.bus.clone(),
                self.db.clone(),
                self.clock.clone(),
                chatlog_clock,
                providers,
            ))
            .unwrap()
    }

    /// Drive one tracker command (or any future) to completion on the
    /// rig's runtime.
    pub(super) fn wait<F: std::future::Future>(&self, future: F) -> F::Output {
        self.runtime.block_on(future)
    }

    /// Structural probe against the actor's owned state.
    pub(super) fn probe<R, F>(&self, tracker: &HuntTracker, probe: F) -> R
    where
        R: Send + 'static,
        F: FnOnce(&mut super::actor::TrackerActor) -> R + Send + 'static,
    {
        self.runtime.block_on(tracker.inspect(probe))
    }

    fn capture(&self) -> Arc<StdMutex<Vec<(Topic, Value)>>> {
        let captured = Arc::new(StdMutex::new(Vec::new()));
        let sink = captured.clone();
        self.bus.add_tap(move |event| {
            sink.lock()
                .unwrap()
                .push((event.topic(), event.payload_value()));
        });
        captured
    }

    pub(super) fn scalar_f64(&self, sql: &'static str, binds: &[&str]) -> f64 {
        let binds: Vec<String> = binds.iter().map(|bind| bind.to_string()).collect();
        self.wait(self.db.with_reader(move |conn| {
            Ok(
                conn.query_row(sql, rusqlite::params_from_iter(binds.iter()), |row| {
                    row.get::<_, f64>(0)
                })?,
            )
        }))
        .unwrap()
    }

    pub(super) fn scalar_i64(&self, sql: &'static str, binds: &[&str]) -> i64 {
        let binds: Vec<String> = binds.iter().map(|bind| bind.to_string()).collect();
        self.wait(self.db.with_reader(move |conn| {
            Ok(
                conn.query_row(sql, rusqlite::params_from_iter(binds.iter()), |row| {
                    row.get::<_, i64>(0)
                })?,
            )
        }))
        .unwrap()
    }

    pub(super) fn scalar_string(&self, sql: &'static str, binds: &[&str]) -> String {
        let binds: Vec<String> = binds.iter().map(|bind| bind.to_string()).collect();
        self.wait(self.db.with_reader(move |conn| {
            Ok(
                conn.query_row(sql, rusqlite::params_from_iter(binds.iter()), |row| {
                    row.get::<_, String>(0)
                })?,
            )
        }))
        .unwrap()
    }

    fn execute(&self, sql: &'static str) {
        self.wait(self.db.with_writer(move |conn| {
            conn.execute(sql, [])?;
            Ok(())
        }))
        .unwrap();
    }
}

pub(super) fn naive(text: &str) -> NaiveDateTime {
    NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S").unwrap()
}

pub(super) fn healer_intent(
    equipment_id: i64,
    name: &str,
    cost: f64,
    reload: f64,
    occurred_at: f64,
    profile: HealingProfile,
) -> BusEvent {
    BusEvent::HotbarIntent(Box::new(HotbarIntentPayload {
        session_id: None,
        slot: "8".into(),
        occurred_at,
        equipment_id,
        item_name: name.into(),
        item_kind: HotbarItemKind::Healing,
        cost_per_use_ped: cost,
        reload_seconds: reload,
        healing_profile: Some(profile),
        lifesteal_percent: None,
        consumable_profile: None,
    }))
}

pub(super) fn weapon_intent(occurred_at: f64, lifesteal_percent: Option<f64>) -> BusEvent {
    BusEvent::HotbarIntent(Box::new(HotbarIntentPayload {
        session_id: None,
        slot: "1".into(),
        occurred_at,
        equipment_id: 2,
        item_name: "Rifle".into(),
        item_kind: HotbarItemKind::Weapon,
        cost_per_use_ped: 0.05,
        reload_seconds: 0.0,
        healing_profile: None,
        lifesteal_percent,
        consumable_profile: None,
    }))
}

/// A carried weapon whose stored props give it a regular band of half to
/// all of `damage`, and a per-shot cost derived from `decay` (PEC).
pub(super) fn carried(id: i64, name: &str, damage: f64, decay: f64) -> CarriedWeaponProfile {
    let props = json!({
        "weapon_entity": {
            "name": name,
            "damage": {"impact": damage},
            "economy": {"decay": decay, "ammo_burn": 0}
        }
    })
    .as_object()
    .unwrap()
    .clone();
    CarriedWeaponProfile {
        weapon: CarriedWeapon::from_props(id, name.to_string(), &Value::Object(props.clone())),
        props,
    }
}

/// The per-shot PED cost a `carried` weapon books.
pub(super) fn carried_cost(damage: f64, decay: f64) -> f64 {
    let profile = carried(0, "x", damage, decay);
    cost_per_shot_from_props(&Value::Object(profile.props), Some(0))["totalCostPerUse"]
        .as_f64()
        .unwrap()
        / 100.0
}

pub(super) fn hit(amount: f64) -> BusEvent {
    BusEvent::Combat(CombatPayload::DamageDealt {
        amount,
        timestamp: "2026-01-01T00:00:01".into(),
    })
}

fn updated_events(captured: &StdMutex<Vec<(Topic, Value)>>) -> Vec<Value> {
    captured
        .lock()
        .unwrap()
        .iter()
        .filter(|(topic, _)| *topic == Topic::TrackingSessionUpdated)
        .map(|(_, data)| data.clone())
        .collect()
}

#[test]
fn session_lifecycle_round_trip() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            cost: Some(Arc::new(|name| if name == "Rifle" { 0.05 } else { 0.0 })),
            ..Default::default()
        }),
        ..Providers::default()
    });
    let captured = rig.capture();

    assert!(!tracker.is_tracking());
    assert!(rig.wait(tracker.stop_session()).unwrap().is_none());
    assert!(!rig.bus.has_subscribers(Topic::Combat));

    let session = rig.wait(tracker.start_session()).unwrap();
    assert!(tracker.is_tracking());
    assert!(rig.bus.has_subscribers(Topic::Combat));
    let start_ts = naive_to_epoch(naive("2026-01-01T00:00:00"));
    assert_eq!(
        rig.scalar_f64(
            "SELECT started_at FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        start_ts
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT is_active FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        1
    );

    // Accumulate one kill with both shrapnel kinds, plus dangling
    // shots after it.
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Rifle".into(),
            source: None,
        }));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 30.0,
            timestamp: "2026-01-01T00:00:01".into(),
        }));
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:02".into()),
        items: vec![
            LootItem {
                item_name: "Animal Hide".into(),
                quantity: 1,
                value_ped: 4.5,
                is_enhancer_shrapnel: false,
            },
            LootItem {
                item_name: "Shrapnel".into(),
                quantity: 50,
                value_ped: 0.5,
                is_enhancer_shrapnel: false,
            },
            LootItem {
                item_name: "Shrapnel".into(),
                quantity: 10,
                value_ped: 0.1,
                is_enhancer_shrapnel: true,
            },
        ],
        total_ped: 5.1,
    }));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 7.5,
            timestamp: "2026-01-01T00:00:03".into(),
        }));

    // A skill gain qualifies the session for a summary.
    {
        let session_id = session.id.clone();
        rig.wait(rig.db.with_writer(move |conn| {
            conn.execute(
                "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
                     VALUES (?, 1.0, 'Rifle', 1.0, 0.5)",
                rusqlite::params![session_id],
            )?;
            Ok(())
        }))
        .unwrap();
    }

    rig.clock.advance(10.0).unwrap();
    let stopped = rig.wait(tracker.stop_session()).unwrap().unwrap();
    assert_eq!(stopped.id, session.id);
    assert_eq!(stopped.kills.len(), 1);
    assert_eq!(stopped.dangling_cost, Ped(0.05));
    assert!(!tracker.is_tracking());
    assert!(!rig.bus.has_subscribers(Topic::Combat));

    let end_ts = naive_to_epoch(naive("2026-01-01T00:00:10"));
    assert_eq!(
        rig.scalar_f64(
            "SELECT ended_at FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        end_ts
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT is_active FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        0
    );
    assert_eq!(
        rig.scalar_f64(
            "SELECT dangling_cost FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        0.05
    );

    // Enhancer-break Shrapnel remains an immediate rebate. Ordinary
    // Shrapnel stays in stock until the player deliberately converts it.
    assert_eq!(
        rig.scalar_f64(
            "SELECT amount FROM ledger_entries WHERE tag = 'enhancer' \
                 AND description = 'Enhancer Shrapnel Rebate'",
            &[],
        ),
        0.1
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM ledger_entries WHERE tag = 'convert'",
            &[],
        ),
        0
    );
    assert_eq!(
        rig.scalar_f64(
            "SELECT SUM(value_ped) FROM kill_loot_items \
             WHERE item_name = 'Shrapnel' AND is_enhancer_shrapnel = 0 \
               AND deactivated_at IS NULL",
            &[],
        ),
        0.5
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM session_summaries WHERE session_id = ?",
            &[&session.id],
        ),
        1
    );

    // Producer events after the stop reach nothing.
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:11".into()),
        items: vec![],
        total_ped: 0.0,
    }));
    assert_eq!(rig.scalar_i64("SELECT COUNT(*) FROM kills", &[]), 1);

    // The lifecycle's domain events: started, the hotbar weapon-switch
    // re-hydrate nudge (emitted directly, stamped at the switch's
    // instant), then stopped.
    let updated = updated_events(&captured);
    assert_eq!(updated.len(), 3);
    assert_eq!(updated[0]["payload"]["reason"], "started");
    assert_eq!(updated[0]["payload"]["status"], "active");
    assert_eq!(updated[0]["occurred_at"], to_iso_utc(start_ts));
    assert_eq!(updated[1]["payload"]["reason"], "updated");
    assert_eq!(updated[1]["payload"]["status"], "active");
    assert_eq!(updated[1]["occurred_at"], to_iso_utc(start_ts));
    assert_eq!(updated[2]["payload"]["reason"], "stopped");
    assert_eq!(updated[2]["payload"]["status"], "idle");
    assert_eq!(updated[2]["occurred_at"], to_iso_utc(end_ts));
}

#[test]
fn start_while_tracking_stops_the_prior_session() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let captured = rig.capture();

    let first = rig.wait(tracker.start_session()).unwrap();
    rig.clock.advance(5.0).unwrap();
    let second = rig.wait(tracker.start_session()).unwrap();
    assert_ne!(first.id, second.id);

    assert_eq!(
        rig.scalar_i64(
            "SELECT is_active FROM tracking_sessions WHERE id = ?",
            &[&first.id],
        ),
        0
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT is_active FROM tracking_sessions WHERE id = ?",
            &[&second.id],
        ),
        1
    );

    // The second start's event order: the prior session's stop
    // lands before the new session's start.
    let topics: Vec<Topic> = captured
        .lock()
        .unwrap()
        .iter()
        .map(|(topic, _)| *topic)
        .collect();
    assert_eq!(
        topics,
        vec![
            Topic::SessionStarted,
            Topic::TrackingSessionUpdated,
            Topic::SessionStopped,
            Topic::TrackingSessionUpdated,
            Topic::SessionStarted,
            Topic::TrackingSessionUpdated,
        ]
    );
}

#[test]
fn recovery_closes_crash_orphaned_sessions() {
    let rig = rig();
    rig.execute(
        "INSERT INTO tracking_sessions (id, started_at, is_active, mob_tracking_mode) \
             VALUES ('orphan', 1000.0, 1, 'mob')",
    );
    rig.execute(
        "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
             timestamp, shots_fired, damage_dealt, damage_taken, critical_hits, \
             cost_ped, enhancer_cost, loot_total_ped, is_global, is_hof) \
             VALUES ('k1', 'orphan', 'Atrox', '', '', 1500.0, 3, 30.0, 0.0, 0, \
             0.15, 0.0, 80.0, 0, 0)",
    );
    rig.execute(
        "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, \
             is_enhancer_shrapnel) VALUES ('k1', 'Shrapnel', 500, 50.0, 0)",
    );
    rig.execute(
        "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, \
             is_enhancer_shrapnel) VALUES ('k1', 'Shrapnel', 300, 30.0, 1)",
    );
    rig.execute(
        "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
             critical_hits, cost_per_shot) VALUES ('k1', 'Rifle', 3, 30.0, 0, 0.05)",
    );
    rig.execute(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
             VALUES ('orphan', 1100.0, 'Rifle', 1.0, 0.5)",
    );
    rig.execute(
        "INSERT INTO healing_activations (\
             id, session_id, tool_name, intent_at, observed_at, chat_timestamp, cost_ped, \
             profile_json, provenance\
         ) VALUES (\
             'ha1', 'orphan', 'FAP', 1600.0, 1600.0, '2026-01-01 00:10:00', 0.04, \
             '{}', 'direct'\
         )",
    );
    rig.execute(
        "INSERT INTO healing_outputs (\
             id, session_id, activation_id, observed_at, chat_timestamp, amount, \
             classification, reason\
         ) VALUES (\
             'ho1', 'orphan', 'ha1', 1600.0, '2026-01-01 00:10:00', 50.0, \
             'direct', 'intent_confirmed'\
         )",
    );
    rig.execute("UPDATE tracking_sessions SET heal_cost = 0.04 WHERE id = 'orphan'");

    let _tracker = rig.tracker(Providers::default());

    assert_eq!(
        rig.scalar_i64(
            "SELECT is_active FROM tracking_sessions WHERE id = 'orphan'",
            &[],
        ),
        0
    );
    assert_eq!(
        rig.scalar_f64(
            "SELECT ended_at FROM tracking_sessions WHERE id = 'orphan'",
            &[],
        ),
        1600.0
    );
    assert_eq!(
        rig.scalar_f64(
            "SELECT heal_cost FROM tracking_sessions WHERE id = 'orphan'",
            &[],
        ),
        0.04
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM ledger_entries WHERE tag = 'convert'",
            &[],
        ),
        0
    );
    assert_eq!(
        rig.scalar_f64(
            "SELECT amount FROM ledger_entries WHERE tag = 'enhancer'",
            &[],
        ),
        30.0
    );
    let expected_date = naive_isoformat(epoch_to_naive(1600.0));
    let date: String = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT date FROM ledger_entries WHERE tag = 'enhancer'",
                [],
                |row| row.get::<_, String>(0),
            )?)
        }))
        .unwrap();
    assert_eq!(date, expected_date);
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM session_summaries WHERE session_id = 'orphan'",
            &[],
        ),
        1
    );
}

#[test]
fn stopping_a_session_relands_its_days_rollups() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            cost: Some(Arc::new(|name| if name == "Rifle" { 0.05 } else { 0.0 })),
            ..Default::default()
        }),
        ..Providers::default()
    });
    let session = rig.wait(tracker.start_session()).unwrap();
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Rifle".into(),
            source: None,
        }));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 30.0,
            timestamp: "2026-01-01T00:00:01".into(),
        }));
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:02".into()),
        items: vec![LootItem {
            item_name: "Animal Hide".into(),
            quantity: 1,
            value_ped: 4.5,
            is_enhancer_shrapnel: false,
        }],
        total_ped: 4.5,
    }));
    // A dangling shot after the kill: its cost persists only at stop.
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 7.5,
            timestamp: "2026-01-01T00:00:03".into(),
        }));

    // Two days later the session's day is behind the heal watermark,
    // rolled up WITHOUT the still-unpersisted dangling cost.
    rig.clock.advance(2.0 * 86_400.0).unwrap();
    let now = naive_to_epoch(rig.clock.now());
    rig.runtime.block_on(async {
        rig.db
            .with_writer(move |conn| crate::daily_rollup::heal_rollups(conn, now))
            .await
            .unwrap();
    });
    let start_day = crate::daily_rollup::epoch_day(naive_to_epoch(naive("2026-01-01T00:00:00")));
    let pre_stop: Option<f64> = {
        let start_day = start_day.clone();
        rig.wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT dangling_cost FROM daily_rollups WHERE day = ?",
                rusqlite::params![start_day],
                |row| row.get::<_, Option<f64>>(0),
            )?)
        }))
        .unwrap()
    };
    assert_eq!(pre_stop, Some(0.0), "pre-stop: the column default");

    // The stop transaction persists the dangling cost and relands
    // the session's days in the same commit.
    let stopped = rig.wait(tracker.stop_session()).unwrap().unwrap();
    assert_eq!(stopped.id, session.id);
    let post_stop: Option<f64> = {
        let start_day = start_day.clone();
        rig.wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT dangling_cost FROM daily_rollups WHERE day = ?",
                rusqlite::params![start_day],
                |row| row.get::<_, Option<f64>>(0),
            )?)
        }))
        .unwrap()
    };
    assert_eq!(post_stop, Some(0.05), "the stop hook relanded the day");
    // The stop day itself (today) stays raw.
    let today = crate::daily_rollup::epoch_day(now);
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM daily_rollups WHERE day >= ?",
            &[&today],
        ),
        0
    );
}

#[test]
fn recovery_relands_the_orphans_days_and_backdated_ledger_keys() {
    let rig = rig();
    let start_epoch = naive_to_epoch(naive("2025-12-30T10:00:00"));
    let kill_epoch = naive_to_epoch(naive("2025-12-30T11:00:00"));
    rig.wait(rig.db.with_writer(move |conn| {
        conn.execute(
            "INSERT INTO tracking_sessions (id, started_at, is_active, mob_tracking_mode) \
                 VALUES ('orphan', ?, 1, 'mob')",
            rusqlite::params![start_epoch],
        )?;
        conn.execute(
            "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
                 timestamp, shots_fired, damage_dealt, damage_taken, critical_hits, \
                 cost_ped, enhancer_cost, loot_total_ped, is_global, is_hof) \
                 VALUES ('k1', 'orphan', 'Atrox', '', '', ?, 3, 30.0, 0.0, 0, \
                 0.15, 0.0, 80.0, 0, 0)",
            rusqlite::params![kill_epoch],
        )?;
        Ok(())
    }))
    .unwrap();
    rig.execute(
        "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, \
             is_enhancer_shrapnel) VALUES ('k1', 'Shrapnel', 500, 50.0, 0)",
    );
    rig.execute(
        "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, \
             is_enhancer_shrapnel) VALUES ('k1', 'Shrapnel', 300, 30.0, 1)",
    );
    rig.execute(
        "INSERT INTO kill_tool_stats (kill_id, tool_name, shots_fired, damage_dealt, \
             critical_hits, cost_per_shot) VALUES ('k1', 'Rifle', 3, 30.0, 0, 0.05)",
    );

    // Heal first (clock: 2026-01-01), so the orphan's day sits at or
    // below the watermark when recovery closes it.
    let now = naive_to_epoch(rig.clock.now());
    rig.runtime.block_on(async {
        rig.db
            .with_writer(move |conn| crate::daily_rollup::heal_rollups(conn, now))
            .await
            .unwrap();
    });

    let _tracker = rig.tracker(Providers::default());

    // Recovery relanded the kill day's families.
    let kill_day = crate::daily_rollup::epoch_day(kill_epoch);
    let loot: Option<f64> = {
        let kill_day = kill_day.clone();
        rig.wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT loot_tt FROM daily_rollups WHERE day = ?",
                rusqlite::params![kill_day],
                |row| row.get::<_, Option<f64>>(0),
            )?)
        }))
        .unwrap()
    };
    assert_eq!(loot, Some(80.0));

    // The still-automatic enhancer rebate relands at the crashed session's
    // end, while ordinary Shrapnel remains unrealised.
    let ledger_key = naive_isoformat(epoch_to_naive(kill_epoch));
    let (kind, amount): (String, f64) = {
        let ledger_key = ledger_key.clone();
        rig.wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT entry_type, amount FROM daily_ledger_rollups WHERE day = ? AND tag = 'enhancer'",
                rusqlite::params![ledger_key],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?)),
            )?)
        }))
        .unwrap()
    };
    assert_eq!((kind.as_str(), amount), ("markup", 30.0));
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM daily_ledger_rollups WHERE day = ? AND tag = 'convert'",
            &[&ledger_key],
        ),
        0
    );
}

#[test]
fn a_failed_stop_rolls_back_every_stop_write() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();
    // A kill with enhancer-break Shrapnel and a skill gain, so the stop
    // sequence writes the session close, a ledger gain, and a summary.
    rig.execute(
        "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
             timestamp, shots_fired, damage_dealt, damage_taken, critical_hits, \
             cost_ped, enhancer_cost, loot_total_ped, is_global, is_hof) \
             VALUES ('k1', (SELECT id FROM tracking_sessions WHERE is_active = 1), \
             'Atrox', '', '', 1500.0, 3, 30.0, 0.0, 0, 0.15, 0.0, 80.0, 0, 0)",
    );
    rig.execute(
        "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped, \
             is_enhancer_shrapnel) VALUES ('k1', 'Shrapnel', 500, 50.0, 1)",
    );
    rig.execute(
        "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
             VALUES ((SELECT id FROM tracking_sessions WHERE is_active = 1), 1100.0, \
             'Rifle', 1.0, 0.5)",
    );
    // Force the final statement of the stop sequence to fail.
    rig.execute("DROP TABLE session_summaries");

    assert!(rig.wait(tracker.stop_session()).is_err());

    // The whole stop transaction rolled back: the session is still
    // active with no end stamp, and no ledger gain landed.
    assert_eq!(
        rig.scalar_i64(
            "SELECT is_active FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        1
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM tracking_sessions WHERE id = ? AND ended_at IS NOT NULL",
            &[&session.id],
        ),
        0
    );
    assert_eq!(
        rig.scalar_i64("SELECT COUNT(*) FROM ledger_entries", &[]),
        0
    );
}

#[test]
fn loot_creates_and_persists_kills_with_filtering() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            cost: Some(Arc::new(|name| if name == "Rifle" { 0.05 } else { 0.0 })),
            ..Default::default()
        }),
        ..Providers::default()
    });
    let session = rig.wait(tracker.start_session()).unwrap();
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Rifle".into(),
            source: None,
        }));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 30.0,
            timestamp: "2026-01-01T00:00:01".into(),
        }));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::CriticalHit {
            amount: 10.0,
            timestamp: "2026-01-01T00:00:01".into(),
        }));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::TargetDodge {
            timestamp: "2026-01-01T00:00:01".into(),
        }));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageReceived {
            amount: 5.0,
            timestamp: "2026-01-01T00:00:01".into(),
        }));

    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:02".into()),
        items: vec![
            LootItem {
                item_name: "Animal Hide".into(),
                quantity: 1,
                value_ped: 4.5,
                is_enhancer_shrapnel: false,
            },
            LootItem {
                item_name: "Universal Ammo".into(),
                quantity: 20,
                value_ped: 0.2,
                is_enhancer_shrapnel: false,
            },
            LootItem {
                item_name: "Shrapnel".into(),
                quantity: 10,
                value_ped: 0.1,
                is_enhancer_shrapnel: true,
            },
        ],
        total_ped: 4.8,
    }));

    let kill_id: String = {
        let session_id = session.id.clone();
        rig.wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT id FROM kills WHERE session_id = ?",
                rusqlite::params![session_id],
                |row| row.get::<_, String>(0),
            )?)
        }))
        .unwrap()
    };
    assert_eq!(
        rig.scalar_i64("SELECT shots_fired FROM kills WHERE id = ?", &[&kill_id]),
        3
    );
    assert_eq!(
        rig.scalar_f64("SELECT damage_dealt FROM kills WHERE id = ?", &[&kill_id]),
        40.0
    );
    assert_eq!(
        rig.scalar_f64("SELECT damage_taken FROM kills WHERE id = ?", &[&kill_id]),
        5.0
    );
    assert_eq!(
        rig.scalar_i64("SELECT critical_hits FROM kills WHERE id = ?", &[&kill_id],),
        1
    );
    assert_eq!(
        rig.scalar_f64("SELECT cost_ped FROM kills WHERE id = ?", &[&kill_id]),
        0.05 * 3.0
    );
    // The blacklisted ammo never lands; the enhancer shrapnel
    // lands as an item but stays out of the loot total.
    assert_eq!(
        rig.scalar_f64("SELECT loot_total_ped FROM kills WHERE id = ?", &[&kill_id],),
        4.5
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM kill_loot_items WHERE kill_id = ?",
            &[&kill_id],
        ),
        2
    );
    let mob: String = {
        let kill_id = kill_id.clone();
        rig.wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT mob_name FROM kills WHERE id = ?",
                rusqlite::params![kill_id],
                |row| row.get::<_, String>(0),
            )?)
        }))
        .unwrap()
    };
    assert_eq!(mob, "Unknown");
    assert_eq!(
        rig.scalar_i64(
            "SELECT shots_fired FROM kill_tool_stats WHERE kill_id = ? \
                 AND tool_name = 'Rifle'",
            &[&kill_id],
        ),
        3
    );
    assert_eq!(
        rig.scalar_f64("SELECT timestamp FROM kills WHERE id = ?", &[&kill_id],),
        naive_to_epoch(naive("2026-01-01T00:00:02"))
    );

    // The accumulator reset: an immediate second group carries
    // zero shots.
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:04".into()),
        items: vec![LootItem {
            item_name: "Mud".into(),
            quantity: 1,
            value_ped: 0.03,
            is_enhancer_shrapnel: false,
        }],
        total_ped: 0.03,
    }));
    assert_eq!(
        rig.scalar_i64(
            "SELECT shots_fired FROM kills WHERE session_id = ? AND id != ?",
            &[&session.id, &kill_id],
        ),
        0
    );
}

/// The chat log is stamped in the game server's zone, not the host's,
/// so a reading must land at the instant it actually names. The
/// session, interval, and quest-run boundaries beside a kill are
/// stamped from the host clock, and every feature that compares the
/// two (a manual quest hand-in offering the clump it just saw, an
/// armour window, a rollup boundary) is only correct while both are
/// the same clock.
#[test]
fn a_server_stamped_loot_reading_lands_at_the_instant_it_names() {
    let rig = rig();
    // A server two hours ahead of UTC, which is what every player's
    // log says whatever their own machine is set to.
    let tracker =
        rig.tracker_on_chatlog_clock(Providers::default(), ChatLogClock::pinned(2 * 3600));
    rig.wait(tracker.start_session()).unwrap();
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: Some("server-stamped".into()),
        timestamp: Some("2026-01-01T02:00:02".into()),
        items: vec![LootItem {
            item_name: "Animal Hide".into(),
            quantity: 1,
            value_ped: 4.5,
            is_enhancer_shrapnel: false,
        }],
        total_ped: 4.5,
    }));
    assert_eq!(
        rig.scalar_f64(
            "SELECT timestamp FROM kills WHERE loot_source_id = ?",
            &["server-stamped"],
        ),
        super::time::instant_to_epoch(naive("2026-01-01T00:00:02").and_utc()),
        "02:00:02 on a server two hours ahead of UTC is 00:00:02 UTC"
    );
}

#[test]
fn exact_loot_source_reclassification_updates_the_live_aggregate() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: Some("quest-clump-1".into()),
        timestamp: Some("2026-01-01T00:00:02".into()),
        items: vec![
            LootItem {
                item_name: "Blazar Fragment".into(),
                quantity: 238,
                value_ped: 1.23,
                is_enhancer_shrapnel: false,
            },
            LootItem {
                item_name: "Animal Oil".into(),
                quantity: 1,
                value_ped: 0.77,
                is_enhancer_shrapnel: false,
            },
        ],
        total_ped: 2.0,
    }));

    let before = rig.wait(tracker.snapshot()).unwrap().active.unwrap();
    assert_eq!(before.returns, 2.0);
    rig.wait(rig.db.with_writer(|conn| {
        conn.execute(
            "UPDATE kill_loot_items SET deactivated_at = 1 \
             WHERE item_name = 'Blazar Fragment'",
            [],
        )?;
        conn.execute(
            "UPDATE kills SET loot_total_ped = 0.77 WHERE loot_source_id = 'quest-clump-1'",
            [],
        )?;
        Ok(())
    }))
    .unwrap();
    assert!(rig
        .wait(tracker.reconcile_loot_source("quest-clump-1"))
        .unwrap());
    let after = rig.wait(tracker.snapshot()).unwrap().active.unwrap();
    assert_eq!(after.returns, 0.77);
    assert!(!rig
        .wait(tracker.reconcile_loot_source("quest-clump-1"))
        .unwrap());
    rig.wait(rig.db.with_writer(|conn| {
        conn.execute(
            "UPDATE kill_loot_items SET deactivated_at = NULL \
             WHERE item_name = 'Blazar Fragment'",
            [],
        )?;
        conn.execute(
            "UPDATE kills SET loot_total_ped = 2.0 WHERE loot_source_id = 'quest-clump-1'",
            [],
        )?;
        Ok(())
    }))
    .unwrap();
    assert!(rig
        .wait(tracker.reconcile_loot_source("quest-clump-1"))
        .unwrap());
    assert_eq!(
        rig.wait(tracker.snapshot())
            .unwrap()
            .active
            .unwrap()
            .returns,
        2.0
    );
    assert_eq!(
        rig.scalar_f64(
            "SELECT loot_total_ped FROM kills WHERE loot_source_id = ?",
            &["quest-clump-1"],
        ),
        2.0,
        "the quest transaction, not actor reconciliation, owns persistence"
    );
}

#[test]
fn loot_dedup_inside_the_window_only() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();

    let group = |ts: &str| {
        BusEvent::LootGroup(LootGroupPayload {
            kind: LootTag,
            source_id: None,
            timestamp: Some(ts.into()),
            items: vec![LootItem {
                item_name: "Animal Hide".into(),
                quantity: 1,
                value_ped: 1.0,
                is_enhancer_shrapnel: false,
            }],
            total_ped: 1.0,
        })
    };
    rig.bus.publish(&group("2026-01-01T00:00:02"));
    // Identical fingerprint inside the strict 2s window: dropped.
    rig.bus.publish(&group("2026-01-01T00:00:03"));
    assert_eq!(rig.scalar_i64("SELECT COUNT(*) FROM kills", &[]), 1);
    // Exactly the window: recorded (the comparison is strict).
    rig.bus.publish(&group("2026-01-01T00:00:04"));
    assert_eq!(rig.scalar_i64("SELECT COUNT(*) FROM kills", &[]), 2);
    // A different fingerprint inside the window: recorded.
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:05".into()),
        items: vec![LootItem {
            item_name: "Mud".into(),
            quantity: 1,
            value_ped: 1.0,
            is_enhancer_shrapnel: false,
        }],
        total_ped: 1.0,
    }));
    assert_eq!(rig.scalar_i64("SELECT COUNT(*) FROM kills", &[]), 3);
}

#[test]
fn snapshot_aggregates_and_rounds_the_readout() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            cost: Some(Arc::new(|name| if name == "Rifle" { 0.05 } else { 0.0 })),
            ..Default::default()
        }),
        player_name: "Hero".to_string(),
        ..Providers::default()
    });

    let idle = rig.wait(tracker.snapshot()).unwrap();
    assert!(idle.active.is_none());
    assert_eq!(idle.current_tool, None);

    let session = rig.wait(tracker.start_session()).unwrap();
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Rifle".into(),
            source: None,
        }));
    rig.bus.publish(&BusEvent::ActiveHealToolChanged(
        ActiveHealToolChangedPayload {
            tool_name: "FAP".into(),
            cost_per_use_ped: 0.02,
            reload_seconds: 2.5,
            source: None,
        },
    ));
    rig.bus.publish(&healer_intent(
        7,
        "FAP",
        0.02,
        2.5,
        naive_to_epoch(naive("2026-01-01T00:00:00")),
        HealingProfile {
            direct_min: Some(10.0),
            direct_max: Some(14.0),
            ..HealingProfile::default()
        },
    ));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 30.0,
            timestamp: "2026-01-01T00:00:01".into(),
        }));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::CriticalHit {
            amount: 10.0,
            timestamp: "2026-01-01T00:00:01".into(),
        }));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::TargetDodge {
            timestamp: "2026-01-01T00:00:01".into(),
        }));
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:02".into()),
        items: vec![LootItem {
            item_name: "Animal Hide".into(),
            quantity: 1,
            value_ped: 5.0,
            is_enhancer_shrapnel: false,
        }],
        total_ped: 5.0,
    }));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 20.0,
            timestamp: "2026-01-01T00:00:03".into(),
        }));
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:04".into()),
        items: vec![LootItem {
            item_name: "Mud".into(),
            quantity: 1,
            value_ped: 0.03,
            is_enhancer_shrapnel: false,
        }],
        total_ped: 0.03,
    }));
    // In-flight accumulator damage after the latest kill.
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 7.5,
            timestamp: "2026-01-01T00:00:05".into(),
        }));
    // Two counted heals (the second exactly at the reload bound).
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 12.0,
        timestamp: "2026-01-01T00:00:05".into(),
    }));
    rig.clock.advance(2.5).unwrap();
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 12.0,
        timestamp: "2026-01-01T00:00:07.500000".into(),
    }));
    // A global correlated to the latest kill.
    rig.bus
        .publish(&BusEvent::Global(GlobalPayload::GlobalKill {
            timestamp: "2026-01-01T00:00:05".into(),
            player: "hero".into(),
            creature: "Atrox".into(),
            value: 12.0,
        }));
    {
        let session_id = session.id.clone();
        rig.wait(rig.db.with_writer(move |conn| {
            conn.execute(
                "INSERT INTO skill_gains (session_id, timestamp, skill_name, amount, ped_value) \
                     VALUES (?, 1.0, 'Rifle', 1.0, 1.0), (?, 2.0, 'Rifle', 1.0, 0.25)",
                rusqlite::params![session_id.clone(), session_id],
            )?;
            Ok(())
        }))
        .unwrap();
    }

    // The reload exercise above already advanced the session by 2.5 seconds.
    rig.clock.advance(57.5).unwrap();
    let readout = rig.wait(tracker.snapshot()).unwrap();
    assert_eq!(readout.current_tool.as_deref(), Some("FAP"));
    let active = readout.active.unwrap();
    assert_eq!(active.session_id, session.id);
    assert_eq!(active.started_at, "2026-01-01T00:00:00");
    assert_eq!(active.kill_count, 2);
    assert_eq!(active.elapsed, 60);
    assert_eq!(active.cost, 0.29);
    assert_eq!(active.returns, 5.03);
    assert_eq!(active.pes, 1.25);
    assert_eq!(active.net, 4.74);
    assert_eq!(active.return_rate, 17.3448);
    assert_eq!(active.damage_dealt_total, 60.0);
    assert_eq!(active.weapon_damage_dealt, 67.5);
    assert_eq!(active.weapon_cost, 0.25);
    assert_eq!(active.shots_fired_total, 4);
    assert_eq!(active.critical_hits_total, 1);
    assert_eq!(active.max_damage, 40.0);
    assert_eq!(active.globals_count, 1);
    assert_eq!(active.hofs_count, 0);
    assert_eq!(active.latest_kill_loot, Some(0.03));
    assert_eq!(active.multiplier_last, Some(0.6));
    assert_eq!(active.multiplier_avg, Some(16.9667));
    assert_eq!(active.multiplier_max, Some(33.3333));
    assert_eq!(active.multiplier_history, vec![33.3333, 0.6]);
    assert_eq!(active.cumulative_net_history, vec![4.82, 4.79]);
    assert_eq!(active.current_mob, None);
    // Undeclared: the session is an instance of the protected default
    // and reads with its name.
    assert_eq!(active.session_name.as_deref(), Some("Default Tracking"));
    assert_eq!(active.skill_boost_percent, None);
    assert_eq!(active.notable_event_rows.len(), 1);
    let row = &active.notable_event_rows[0];
    assert_eq!(row.0, "global_kill");
    assert_eq!(row.1, "Atrox");
    assert_eq!(row.2, 12.0);
    assert_eq!(row.3, Some(naive_to_epoch(naive("2026-01-01T00:00:05"))));
    assert!(active.warnings.is_empty());

    // The session heal cost reached the session row on stop.
    rig.wait(tracker.stop_session()).unwrap();
    assert_eq!(
        rig.scalar_f64(
            "SELECT heal_cost FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        0.04
    );
}

#[test]
fn shots_before_the_first_press_are_never_repriced_by_it() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            cost: Some(Arc::new(|name| if name == "Pistol" { 0.02 } else { 0.0 })),
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();

    // No weapon is carried or declared yet: these shots stay unpriced.
    rig.bus.publish(&hit(9.0));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::CriticalHit {
            amount: 4.0,
            timestamp: "2026-01-01T00:00:01".into(),
        }));
    // The press starts a regime; it may not reach back.
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Pistol".into(),
            source: None,
        }));
    rig.bus.publish(&hit(6.0));
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:03".into()),
        items: vec![],
        total_ped: 0.0,
    }));

    let rows: Vec<(String, i64, f64, i64, f64)> = rig
        .wait(rig.db.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT tool_name, shots_fired, damage_dealt, critical_hits, cost_per_shot \
                     FROM kill_tool_stats ORDER BY id",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, f64>(4)?,
                ))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        }))
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("Unknown".to_string(), 2, 13.0, 1, 0.0),
            ("Pistol".to_string(), 1, 6.0, 0, 0.02),
        ]
    );
}

#[test]
fn a_shot_before_the_first_press_stays_unpriced_and_outside_expected_evidence() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            cost: Some(Arc::new(|_| 0.02)),
            profile: Some(Arc::new(|name| {
                (name == "MyGun").then(|| {
                    json!({
                        "weapon_catalog_id": "weapon-42",
                        "weapon_markup": 100.0,
                        "weapon_entity": {
                            "name": "MyGun",
                            "economy": {
                                "decay": 1.0,
                                "ammo_burn": 100.0,
                                "efficiency": 84.5
                            }
                        },
                        "amp_entity": null
                    })
                    .as_object()
                    .unwrap()
                    .clone()
                })
            })),
            looters: Some(crate::expected_hunting::HuntingLooterLevels {
                animal: 60.0,
                mutant: 60.0,
                robot: 60.0,
            }),
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();

    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 9.0,
            timestamp: "2026-01-01T00:00:01".into(),
        }));
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "MyGun".into(),
            source: None,
        }));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 6.0,
            timestamp: "2026-01-01T00:00:02".into(),
        }));

    // The unpriced shot books no raw TT, so the model covers everything
    // that was priced; the unpriced shot is disclosed on its own.
    let active = rig.wait(tracker.snapshot()).unwrap().active.unwrap();
    assert_eq!(active.expected_return_coverage, Some(1.0));
    assert_eq!(active.unpriced_shots, 1);
    rig.probe(&tracker, |actor| {
        let phases = &actor.session.active().unwrap().accumulator.tool_stats;
        assert_eq!(phases.len(), 2);
        assert_eq!(phases[0].1.tool_name, "Unknown");
        assert_eq!(phases[0].1.shots_fired, 1);
        assert_eq!(phases[0].1.cost_per_shot, Ped::ZERO);
        assert!(phases[0].1.expected_economics.is_none());
        assert_eq!(phases[1].1.tool_name, "MyGun");
        assert_eq!(phases[1].1.shots_fired, 1);
        assert!(phases[1].1.expected_economics.is_some());
    });
}

#[test]
fn phased_tool_stats_split_on_cost_change() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();

    rig.probe(&tracker, |actor| {
        let accumulator = &mut actor.session.active_mut().unwrap().accumulator;
        TrackerActor::tool_stats_for_phase(
            &mut accumulator.tool_stats,
            "Rifle",
            Ped(0.05),
            None,
        )
        .shots_fired += 1;
        // Within the tolerance: the same phase.
        TrackerActor::tool_stats_for_phase(
            &mut accumulator.tool_stats,
            "Rifle",
            Ped(0.05 + 1e-12),
            None,
        )
        .shots_fired += 1;
        // A real cost change: a second phase keyed `Rifle#2`.
        TrackerActor::tool_stats_for_phase(
            &mut accumulator.tool_stats,
            "Rifle",
            Ped(0.04),
            None,
        )
        .shots_fired += 1;
        // A third: `Rifle#3`; a different tool keeps its bare key.
        TrackerActor::tool_stats_for_phase(
            &mut accumulator.tool_stats,
            "Rifle",
            Ped(0.03),
            None,
        )
        .shots_fired += 1;
        TrackerActor::tool_stats_for_phase(
            &mut accumulator.tool_stats,
            "Pistol",
            Ped(0.02),
            None,
        )
        .shots_fired += 1;
        // A cost difference of exactly the tolerance opens a phase:
        // the comparison is strict (2e-9 - 1e-9 is exactly 1e-9).
        TrackerActor::tool_stats_for_phase(
            &mut accumulator.tool_stats,
            "Laser",
            Ped(1e-9),
            None,
        )
        .shots_fired += 1;
        TrackerActor::tool_stats_for_phase(
            &mut accumulator.tool_stats,
            "Laser",
            Ped(2e-9),
            None,
        )
        .shots_fired += 1;

        let keys: Vec<(String, String, i64)> = accumulator
            .tool_stats
            .iter()
            .map(|(key, stats)| (key.clone(), stats.tool_name.clone(), stats.shots_fired))
            .collect();
        assert_eq!(
            keys,
            vec![
                ("Rifle".to_string(), "Rifle".to_string(), 2),
                ("Rifle#2".to_string(), "Rifle".to_string(), 1),
                ("Rifle#3".to_string(), "Rifle".to_string(), 1),
                ("Pistol".to_string(), "Pistol".to_string(), 1),
                ("Laser".to_string(), "Laser".to_string(), 1),
                ("Laser#2".to_string(), "Laser".to_string(), 1),
            ]
        );
    });
}

#[test]
fn healing_cost_requires_compatible_intent_and_respects_cooldown() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();

    // Chat output by itself is evidence, never a paid use.
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:01".into(),
    }));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:09".into(),
    }));
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.heal_cost, Ped::ZERO);
        assert_eq!(active.healing.unattributed_output_count, 2);
    });

    let now = naive_to_epoch(naive("2026-01-01T00:00:00"));
    rig.bus.publish(&healer_intent(
        7,
        "FAP",
        0.03,
        5.0,
        now,
        HealingProfile {
            direct_min: Some(8.0),
            direct_max: Some(12.0),
            ..HealingProfile::default()
        },
    ));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:20".into(),
    }));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:24".into(),
    }));
    rig.clock.advance(5.0).unwrap();
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:25".into(),
    }));
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.heal_cost, Ped(0.06));
        assert_eq!(active.healing.activation_count, 2);
    });
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM healing_activations WHERE session_id = ?",
            &[&session.id],
        ),
        2
    );
    assert_eq!(
        rig.scalar_f64(
            "SELECT heal_cost FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        0.06
    );
}

#[test]
fn a_heal_read_just_inside_its_reload_still_bills_and_an_early_one_does_not() {
    // Chat lines are read in polls and written in bursts, so a FAP used at
    // its full rate can reach the tracker a little inside its 2.5 s reload.
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();
    rig.bus.publish(&healer_intent(
        7,
        "FAP",
        0.03,
        2.5,
        naive_to_epoch(naive("2026-01-01T00:00:00")),
        HealingProfile {
            direct_min: Some(8.0),
            direct_max: Some(12.0),
            ..HealingProfile::default()
        },
    ));
    let heal = |at: &str| {
        BusEvent::Combat(CombatPayload::SelfHeal {
            amount: 10.0,
            timestamp: at.into(),
        })
    };
    rig.bus.publish(&heal("2026-01-01T00:00:20"));
    // Read 0.25 s early: within the allowance, a genuine use.
    rig.clock.advance(2.25).unwrap();
    rig.bus.publish(&heal("2026-01-01T00:00:22"));
    // Read 1 s after that one: far inside the reload, not a paid use.
    rig.clock.advance(1.0).unwrap();
    rig.bus.publish(&heal("2026-01-01T00:00:23"));

    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM healing_activations WHERE session_id = ?",
            &[&session.id],
        ),
        2
    );
}

#[test]
fn effective_reload_accepts_back_to_back_heals_within_the_base_interval() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();
    let effective_reload = crate::passive_effects::effective_reload_seconds(
        2.5,
        &[crate::passive_effects::PassiveEffectSource {
            id: "ares-perfect".into(),
            name: "Ares Ring, Perfected".into(),
            enabled: true,
            effects: vec![crate::passive_effects::PassiveEffect {
                kind: crate::passive_effects::PassiveEffectKind::ReloadSpeed,
                magnitude_percent: 14.0,
            }],
        }],
    );
    rig.bus.publish(&healer_intent(
        7,
        "FAP",
        0.03,
        effective_reload,
        naive_to_epoch(naive("2026-01-01T00:00:00")),
        HealingProfile {
            direct_min: Some(8.0),
            direct_max: Some(12.0),
            base_reload_seconds: Some(2.5),
            reload_speed_percent: Some(14.0),
            effective_reload_seconds: Some(effective_reload),
            ..HealingProfile::default()
        },
    ));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:20".into(),
    }));
    rig.clock.advance(2.206).unwrap();
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:22.206".into(),
    }));

    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM healing_activations WHERE session_id = ?",
            &[&session.id],
        ),
        2
    );
    assert_eq!(
        rig.scalar_f64(
            "SELECT heal_cost FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        0.06
    );
}

#[test]
fn an_unprofiled_healer_press_invalidates_the_previous_activation_candidate() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();
    let now = naive_to_epoch(naive("2026-01-01T00:00:00"));
    rig.bus.publish(&healer_intent(
        7,
        "FAP",
        0.03,
        5.0,
        now,
        HealingProfile {
            direct_min: Some(8.0),
            direct_max: Some(12.0),
            ..HealingProfile::default()
        },
    ));
    rig.bus
        .publish(&BusEvent::HotbarIntent(Box::new(HotbarIntentPayload {
            session_id: None,
            slot: "9".into(),
            occurred_at: now + 0.1,
            equipment_id: 9,
            item_name: "Unprofiled healer".into(),
            item_kind: HotbarItemKind::Healing,
            cost_per_use_ped: 0.05,
            reload_seconds: 3.0,
            healing_profile: None,
            lifesteal_percent: None,
            consumable_profile: None,
        })));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:01".into(),
    }));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.heal_cost, Ped::ZERO);
        assert_eq!(active.healing.activation_count, 0);
        assert_eq!(active.healing.unattributed_output_count, 1);
    });
}

#[test]
fn stale_session_hotbar_intents_cannot_mutate_equipment_state() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();
    let stale_session_id = format!("previous-{}", session.id);
    let now = naive_to_epoch(naive("2026-01-01T00:00:00"));

    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Current rifle".into(),
            source: None,
        }));
    rig.bus.publish(&BusEvent::ActiveHealToolChanged(
        ActiveHealToolChangedPayload {
            tool_name: "Current FAP".into(),
            cost_per_use_ped: 0.02,
            reload_seconds: 2.5,
            source: None,
        },
    ));

    for (slot, equipment_id, item_name, item_kind) in [
        ("1", 11, "Stale rifle", HotbarItemKind::Weapon),
        ("8", 12, "Stale FAP", HotbarItemKind::Healing),
        ("9", 13, "Stale harvester", HotbarItemKind::Harvesting),
    ] {
        rig.bus
            .publish(&BusEvent::HotbarIntent(Box::new(HotbarIntentPayload {
                session_id: Some(stale_session_id.clone()),
                slot: slot.into(),
                occurred_at: now,
                equipment_id,
                item_name: item_name.into(),
                item_kind,
                cost_per_use_ped: 0.5,
                reload_seconds: 3.0,
                healing_profile: Some(HealingProfile {
                    direct_min: Some(20.0),
                    direct_max: Some(30.0),
                    ..HealingProfile::default()
                }),
                lifesteal_percent: Some(5.0),
                consumable_profile: None,
            })));
    }

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.weapons.attribution.declared(), Some("Current rifle"));
        assert_eq!(active.healing.weapon_lifesteal_percent, None);
        assert_eq!(
            actor.held_item.as_ref().map(|(name, _)| name.as_str()),
            Some("Current FAP")
        );
        assert!(actor.harvest_tool.is_none());
        assert_eq!(
            actor.held_item.as_ref().map(|(_, kind)| *kind),
            Some(HotbarItemKind::Healing)
        );
    });
}

#[test]
fn rapid_healer_activation_survives_the_switch_back_to_a_lifesteal_weapon() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();
    let now = naive_to_epoch(naive("2026-01-01T00:00:00"));
    let profile = HealingProfile {
        direct_min: Some(28.0),
        direct_max: Some(32.0),
        ..HealingProfile::default()
    };

    rig.bus.publish(&healer_intent(
        8,
        "Restoration chip",
        0.04,
        10.0,
        now,
        profile,
    ));
    rig.clock.advance(0.5).unwrap();
    rig.bus.publish(&weapon_intent(now + 0.5, Some(2.0)));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 30.0,
        timestamp: "2026-01-01T00:00:00".into(),
    }));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.heal_cost, Ped(0.04));
        assert_eq!(active.healing.activation_count, 1);
    });
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM healing_activations WHERE session_id = ?",
            &[&session.id],
        ),
        1
    );
}

#[test]
fn damage_correlated_lifesteal_is_persisted_as_passive_and_never_costed() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();
    let now = naive_to_epoch(naive("2026-01-01T00:00:00"));
    rig.bus.publish(&weapon_intent(now, Some(2.0)));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 100.0,
            timestamp: "2026-01-01T00:00:00".into(),
        }));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 2.0,
        timestamp: "2026-01-01T00:00:00".into(),
    }));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.heal_cost, Ped::ZERO);
        assert_eq!(active.healing.passive_output_count, 1);
    });
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM healing_outputs WHERE session_id = ? \
             AND classification = 'passive' AND activation_id IS NULL",
            &[&session.id],
        ),
        1
    );
}

#[test]
fn compound_healing_costs_once_and_suppresses_ticks_even_with_another_healer_ready() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();
    let now = naive_to_epoch(naive("2026-01-01T00:00:00"));
    rig.bus.publish(&healer_intent(
        8,
        "Restoration chip",
        0.04,
        10.0,
        now,
        HealingProfile {
            mode: HealingMode::Compound,
            direct_min: Some(28.0),
            direct_max: Some(32.0),
            effect_duration_seconds: Some(20.0),
            tick_min: Some(9.0),
            tick_max: Some(11.0),
            tick_seconds: Some(2.0),
            ..HealingProfile::default()
        },
    ));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 30.0,
        timestamp: "2026-01-01T00:00:00".into(),
    }));
    rig.clock.advance(1.0).unwrap();
    rig.bus.publish(&healer_intent(
        9,
        "FAP",
        0.03,
        3.0,
        now + 1.0,
        HealingProfile {
            direct_min: Some(60.0),
            direct_max: Some(100.0),
            ..HealingProfile::default()
        },
    ));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:01".into(),
    }));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 80.0,
        timestamp: "2026-01-01T00:00:01".into(),
    }));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.heal_cost, Ped(0.07));
        assert_eq!(active.healing.activation_count, 2);
        assert_eq!(active.healing.effect_output_count, 1);
    });
}

#[test]
fn a_fresh_healer_edge_wins_an_overlap_then_the_effect_wins_after_the_edge_ages() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();
    let now = naive_to_epoch(naive("2026-01-01T00:00:00"));
    rig.bus.publish(&healer_intent(
        8,
        "Restoration chip",
        0.04,
        10.0,
        now,
        HealingProfile {
            mode: HealingMode::Compound,
            direct_min: Some(28.0),
            direct_max: Some(32.0),
            effect_duration_seconds: Some(20.0),
            tick_min: Some(60.0),
            tick_max: Some(100.0),
            tick_seconds: Some(2.0),
            ..HealingProfile::default()
        },
    ));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 30.0,
        timestamp: "2026-01-01T00:00:00".into(),
    }));
    rig.clock.advance(1.0).unwrap();
    rig.bus.publish(&healer_intent(
        9,
        "FAP",
        0.03,
        3.0,
        now + 1.0,
        HealingProfile {
            direct_min: Some(60.0),
            direct_max: Some(100.0),
            ..HealingProfile::default()
        },
    ));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 80.0,
        timestamp: "2026-01-01T00:00:01".into(),
    }));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.heal_cost, Ped(0.07));
        assert_eq!(active.healing.activation_count, 2);
        assert_eq!(active.healing.effect_output_count, 0);
    });

    rig.clock.advance(3.0).unwrap();
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 80.0,
        timestamp: "2026-01-01T00:00:04".into(),
    }));
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.heal_cost, Ped(0.07));
        assert_eq!(active.healing.activation_count, 2);
        assert_eq!(active.healing.effect_output_count, 1);
    });
}

#[test]
fn overlapping_effects_suppress_cost_without_inventing_one_source_window() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();
    let now = naive_to_epoch(naive("2026-01-01T00:00:00"));
    let profile = |direct_min, direct_max| HealingProfile {
        mode: HealingMode::Compound,
        direct_min: Some(direct_min),
        direct_max: Some(direct_max),
        effect_duration_seconds: Some(20.0),
        tick_min: Some(9.0),
        tick_max: Some(11.0),
        tick_seconds: Some(2.0),
        ..HealingProfile::default()
    };

    rig.bus.publish(&healer_intent(
        8,
        "Restoration A",
        0.04,
        1.0,
        now,
        profile(28.0, 32.0),
    ));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 30.0,
        timestamp: "2026-01-01T00:00:00".into(),
    }));
    rig.clock.advance(1.0).unwrap();
    rig.bus.publish(&healer_intent(
        9,
        "Restoration B",
        0.05,
        1.0,
        now + 1.0,
        profile(38.0, 42.0),
    ));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 40.0,
        timestamp: "2026-01-01T00:00:01".into(),
    }));
    rig.clock.advance(2.0).unwrap();
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:03".into(),
    }));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.heal_cost, Ped(0.09));
        assert_eq!(active.healing.activation_count, 2);
        assert_eq!(active.healing.effect_output_count, 1);
    });
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM healing_outputs WHERE session_id = ? \
             AND classification = 'effect' AND activation_id IS NULL \
             AND effect_window_id IS NULL",
            &[&session.id],
        ),
        1
    );
}

#[test]
fn delayed_hotbar_resolution_reconciles_an_earlier_uncosted_output() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();
    let now = naive_to_epoch(naive("2026-01-01T00:00:00"));
    rig.clock.advance(0.5).unwrap();
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 30.0,
        timestamp: "2026-01-01T00:00:00".into(),
    }));
    rig.bus.publish(&healer_intent(
        8,
        "Restoration chip",
        0.04,
        10.0,
        now,
        HealingProfile {
            direct_min: Some(28.0),
            direct_max: Some(32.0),
            ..HealingProfile::default()
        },
    ));

    rig.probe(&tracker, |actor| {
        assert_eq!(actor.session.active().unwrap().heal_cost, Ped(0.04));
    });
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM healing_outputs WHERE session_id = ? \
             AND classification = 'direct' AND activation_id IS NOT NULL",
            &[&session.id],
        ),
        1
    );
}

#[test]
fn pure_over_time_healing_costs_on_its_first_tick_and_not_on_later_ticks() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();
    let now = naive_to_epoch(naive("2026-01-01T00:00:00"));
    rig.bus.publish(&healer_intent(
        8,
        "Regeneration chip",
        0.04,
        10.0,
        now,
        HealingProfile {
            mode: HealingMode::OverTime,
            direct_min: None,
            direct_max: None,
            effect_duration_seconds: Some(20.0),
            tick_min: Some(9.0),
            tick_max: Some(11.0),
            tick_seconds: Some(2.0),
            ..HealingProfile::default()
        },
    ));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:00".into(),
    }));
    rig.clock.advance(2.0).unwrap();
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:02".into(),
    }));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.heal_cost, Ped(0.04));
        assert_eq!(active.healing.activation_count, 1);
        assert_eq!(active.healing.effect_output_count, 2);
    });
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM healing_activations WHERE session_id = ?",
            &[&session.id],
        ),
        1
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM healing_effect_windows WHERE session_id = ?",
            &[&session.id],
        ),
        1
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM healing_outputs WHERE session_id = ? \
             AND classification = 'effect' AND activation_id IS NOT NULL",
            &[&session.id],
        ),
        2
    );
}

#[test]
fn delayed_pure_over_time_intent_reconciles_its_first_tick_as_one_activation() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();
    let now = naive_to_epoch(naive("2026-01-01T00:00:00"));
    rig.clock.advance(0.5).unwrap();
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:00".into(),
    }));
    rig.bus.publish(&healer_intent(
        8,
        "Regeneration chip",
        0.04,
        10.0,
        now,
        HealingProfile {
            mode: HealingMode::OverTime,
            direct_min: None,
            direct_max: None,
            effect_duration_seconds: Some(20.0),
            tick_min: Some(9.0),
            tick_max: Some(11.0),
            tick_seconds: Some(2.0),
            ..HealingProfile::default()
        },
    ));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.heal_cost, Ped(0.04));
        assert_eq!(active.healing.activation_count, 1);
        assert_eq!(active.healing.effect_output_count, 1);
    });
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM healing_outputs WHERE session_id = ? \
             AND classification = 'effect' AND activation_id IS NOT NULL",
            &[&session.id],
        ),
        1
    );
}

#[test]
fn globals_correlate_within_the_window() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        player_name: "  Hero  ".to_string(),
        ..Providers::default()
    });
    let session = rig.wait(tracker.start_session()).unwrap();

    let loot = |ts: &str, value: f64| {
        BusEvent::LootGroup(LootGroupPayload {
            kind: LootTag,
            source_id: None,
            timestamp: Some(ts.into()),
            items: vec![LootItem {
                item_name: "Animal Hide".into(),
                quantity: 1,
                value_ped: value,
                is_enhancer_shrapnel: false,
            }],
            total_ped: value,
        })
    };
    rig.bus.publish(&loot("2026-01-01T00:00:02", 1.0));
    // The wrong player never lands.
    rig.bus
        .publish(&BusEvent::Global(GlobalPayload::GlobalKill {
            timestamp: "2026-01-01T00:00:03".into(),
            player: "Villain".into(),
            creature: "Atrox".into(),
            value: 8.0,
        }));
    assert_eq!(
        rig.scalar_i64("SELECT COUNT(*) FROM notable_events", &[]),
        0
    );
    // Case-insensitive match (the configured name is stripped at
    // construction); a HoF inside the window tags the kill.
    rig.bus.publish(&BusEvent::Global(GlobalPayload::HofKill {
        timestamp: "2026-01-01T00:00:04".into(),
        player: "HERO".into(),
        creature: "Atrox".into(),
        value: 120.0,
    }));
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM kills WHERE is_global = 1 AND is_hof = 1",
            &[],
        ),
        1
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM notable_events WHERE kill_id IS NOT NULL",
            &[],
        ),
        1
    );

    // A stale global (past the 5s window) records the notable
    // event with no kill correlation.
    rig.bus.publish(&loot("2026-01-01T00:00:10", 2.0));
    rig.bus
        .publish(&BusEvent::Global(GlobalPayload::GlobalKill {
            timestamp: "2026-01-01T00:00:16".into(),
            player: "Hero".into(),
            creature: "Rare Thing".into(),
            value: 50.0,
        }));
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM notable_events WHERE kill_id IS NULL \
                 AND mob_or_item = 'Rare Thing'",
            &[],
        ),
        1
    );
    assert_eq!(
        rig.scalar_i64("SELECT COUNT(*) FROM kills WHERE is_global = 1", &[]),
        1
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM notable_events WHERE session_id = ?",
            &[&session.id],
        ),
        2
    );

    // An empty configured player name disables correlation.
    let unnamed = rig.tracker(Providers::default());
    rig.wait(unnamed.start_session()).unwrap();
    rig.bus
        .publish(&BusEvent::Global(GlobalPayload::GlobalKill {
            timestamp: "2026-01-01T00:00:20".into(),
            player: "".into(),
            creature: "Atrox".into(),
            value: 1.0,
        }));
    assert_eq!(
        rig.scalar_i64("SELECT COUNT(*) FROM notable_events", &[]),
        2
    );
}

#[test]
fn enhancer_breaks_filter_and_deplete_stacks() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            profile: Some(Arc::new(|name| {
                (name == "Rifle").then(|| {
                    let profile = json!({
                        "damage_enhancers": 2,
                        "weapon_entity": {"name": "Rifle Prime"},
                    });
                    profile.as_object().unwrap().clone()
                })
            })),
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Rifle".into(),
            source: None,
        }));
    rig.probe(&tracker, |actor| {
        let weapons = &actor.session.active().unwrap().weapons;
        assert_eq!(weapons.active_key.as_deref(), Some("Rifle Prime"));
        assert_eq!(
            weapons.enhancer_states["Rifle Prime"].stacks,
            vec![100, 100]
        );
    });

    // A non-damage enhancer never applies; a damage break naming
    // a different item never applies.
    rig.bus
        .publish(&BusEvent::EnhancerBreak(EnhancerBreakPayload {
            kind: EnhancerBreakTag,
            timestamp: "2026-01-01T00:00:01".into(),
            enhancer_name: "Accuracy Enhancer 5".into(),
            item_name: "Rifle Prime".into(),
            remaining: 150,
            shrapnel_ped: 0.0,
        }));
    rig.bus
        .publish(&BusEvent::EnhancerBreak(EnhancerBreakPayload {
            kind: EnhancerBreakTag,
            timestamp: "2026-01-01T00:00:01".into(),
            enhancer_name: "Damage Enhancer 5".into(),
            item_name: "Sword".into(),
            remaining: 150,
            shrapnel_ped: 0.0,
        }));
    rig.probe(&tracker, |actor| {
        assert_eq!(
            actor.session.active().unwrap().weapons.enhancer_states["Rifle Prime"].stacks,
            vec![100, 100]
        );
    });

    // A matching break with a remaining count redistributes,
    // front-loading the remainder. The match admits the observed
    // hotbar spelling and lowercased-alphanumeric containment.
    rig.bus
        .publish(&BusEvent::EnhancerBreak(EnhancerBreakPayload {
            kind: EnhancerBreakTag,
            timestamp: "2026-01-01T00:00:01".into(),
            enhancer_name: "Damage Enhancer 5".into(),
            item_name: "rifle-prime".into(),
            remaining: 151,
            shrapnel_ped: 0.0,
        }));
    rig.probe(&tracker, |actor| {
        assert_eq!(
            actor.session.active().unwrap().weapons.enhancer_states["Rifle Prime"].stacks,
            vec![76, 75]
        );
    });
    rig.bus
        .publish(&BusEvent::EnhancerBreak(EnhancerBreakPayload {
            kind: EnhancerBreakTag,
            timestamp: "2026-01-01T00:00:01".into(),
            enhancer_name: "damage enh".into(),
            item_name: "Rifle".into(),
            remaining: 150,
            shrapnel_ped: 0.0,
        }));
    rig.probe(&tracker, |actor| {
        assert_eq!(
            actor.session.active().unwrap().weapons.enhancer_states["Rifle Prime"].stacks,
            vec![75, 75]
        );
    });
}

#[test]
fn damage_enhancer_state_arithmetic() {
    let props = Arc::new(json!({"damage_enhancers": 3.7}));
    let mut state = DamageEnhancerState::from_props("Rifle", props);
    assert_eq!(state.stacks, vec![100, 100, 100], "int() truncates");
    assert_eq!(state.active_slots(), 3);

    state.set_total(7);
    assert_eq!(state.stacks, vec![3, 2, 2], "the remainder front-loads");
    state.set_total(-5);
    assert_eq!(state.stacks, vec![0, 0, 0], "totals clamp at zero");

    state.set_total(2);
    assert_eq!(state.stacks, vec![1, 1, 0]);
    assert_eq!(state.active_slots(), 2);
    // A break with no remaining decrements the last positive slot
    // and reports the depletion.
    assert!(state.apply_break(None));
    assert_eq!(state.stacks, vec![1, 0, 0]);
    assert!(
        state.apply_break(Some(3)),
        "redistribution re-activating slots reports the change"
    );
    assert_eq!(state.stacks, vec![1, 1, 1]);

    let mut slotless = DamageEnhancerState::from_props("Bare", Arc::new(json!({})));
    assert_eq!(slotless.stacks, Vec::<i64>::new());
    assert!(!slotless.apply_break(Some(50)), "no slots, no change");

    let negative =
        DamageEnhancerState::from_props("Neg", Arc::new(json!({"damage_enhancers": -2})));
    assert_eq!(negative.stacks, Vec::<i64>::new());
}

#[test]
fn carried_weapons_attribute_shots_without_the_hotbar_and_heals_stay_uncosted() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            carried: vec![
                carried(1, "Pistol", 10.0, 0.05),
                carried(2, "Cannon", 40.0, 0.2),
            ],
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();
    let pistol = carried_cost(10.0, 0.05);
    let cannon = carried_cost(40.0, 0.2);

    // Only the pistol's band (5-10) explains 7.
    rig.bus.publish(&hit(7.0));
    // Nothing explains 0.5 or 90: recorded, never priced.
    rig.bus.publish(&hit(0.5));
    rig.bus.publish(&hit(90.0));
    // A critical 25 fits the pistol's critical reach (up to 30) and the
    // cannon's regular band (20-40): two sources, so no guess.
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::CriticalHit {
            amount: 25.0,
            timestamp: "2026-01-01T00:00:02".into(),
        }));
    // Only the cannon explains 35; a countered shot inherits it.
    rig.bus.publish(&hit(35.0));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::TargetJam {
        timestamp: "2026-01-01T00:00:02".into(),
    }));
    rig.probe(&tracker, move |actor| {
        let active = actor.session.active().unwrap();
        let stats: Vec<(String, i64, f64)> = active
            .accumulator
            .tool_stats
            .iter()
            .map(|(key, stats)| (key.clone(), stats.shots_fired, stats.cost_per_shot.value()))
            .collect();
        assert_eq!(
            stats,
            vec![
                ("Pistol".to_string(), 1, pistol),
                ("Unknown".to_string(), 3, 0.0),
                ("Cannon".to_string(), 2, cannon),
            ]
        );
        assert!(
            active.warnings.is_empty(),
            "unpriced shots are disclosed, not warned"
        );
        assert!(
            active.weapons.attribution.mismatch().is_none(),
            "nothing declared, nothing to disagree with"
        );
        assert_eq!(active.weapons.attribution.recording(), Some("Cannon"));
    });
    let active = rig.wait(tracker.snapshot()).unwrap().active.unwrap();
    assert_eq!(active.unpriced_shots, 3);
    assert!(active.weapon_guardrail_mismatch.is_none());
    let readout = rig.wait(tracker.snapshot()).unwrap();
    assert_eq!(readout.current_tool.as_deref(), Some("Cannon"));

    // Healing never bills from damage-range inference: chat outputs stay
    // zero-cost without a hotbar activation intent.
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 15.0,
        timestamp: "2026-01-01T00:00:03".into(),
    }));
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.heal_cost, Ped::ZERO);
        assert_eq!(active.healing.activation_count, 0);
    });
}

/// A session opens its interval context before anything can be recorded
/// into it, and the opening boost becomes a modifier interval inside
/// that context. The context is minted even with nothing declared: an
/// event stamped with the empty context is a different, useful fact
/// from an event that predates the interval model.
#[test]
fn a_session_opens_a_context_and_its_declared_modifier() {
    let rig = rig();
    let boosted = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            skill_boost_percent: Some(50),
            ..Default::default()
        }),
        ..Providers::default()
    });
    let session = rig.wait(boosted.start_session()).unwrap();

    rig.probe(&boosted, |actor| {
        let active = actor.session.active().unwrap();
        assert!(
            active.intervals.context_id().is_some(),
            "opening context minted"
        );
        assert_eq!(active.intervals.modifier_magnitude(), Some(50.0));
    });

    // The interval is a real row, open, owned by this session.
    let row: (String, Option<f64>, Option<f64>) = rig
        .wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT kind, magnitude, ended_at FROM session_intervals WHERE session_id = ?",
                rusqlite::params![session.id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<f64>>(1)?,
                        row.get::<_, Option<f64>>(2)?,
                    ))
                },
            )?)
        }))
        .unwrap();
    assert_eq!(row, ("modifier".to_string(), Some(50.0), None));
}

/// A session STARTED under a declared zero opens the same real modifier
/// interval a mid-session declaration would, so the baseline holds from
/// the first event rather than only from the first re-declaration. The
/// session row's own scalar stays null (0019 constrains it to `> 0 OR
/// NULL`), which is precisely why the interval layer is the source of
/// truth and the readout reads from there.
#[test]
fn a_session_started_under_a_declared_zero_opens_its_baseline() {
    let rig = rig();
    let unboosted = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            skill_boost_percent: Some(0),
            ..Default::default()
        }),
        ..Providers::default()
    });
    let session = rig.wait(unboosted.start_session()).unwrap();

    rig.probe(&unboosted, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(
            active.intervals.modifier_magnitude(),
            Some(0.0),
            "the declared baseline is in force from the session's first moment"
        );
        assert_eq!(
            active.facets.skill_boost_percent, None,
            "the row mirror cannot hold a zero, and does not pretend to"
        );
    });

    let row: (String, Option<f64>) = rig
        .wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT kind, magnitude FROM session_intervals WHERE session_id = ?",
                rusqlite::params![session.id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<f64>>(1)?)),
            )?)
        }))
        .unwrap();
    assert_eq!(row, ("modifier".to_string(), Some(0.0)));
}

/// The two activity declarations, as the tests spell them.
fn quest_activity(quest_id: i64, name: &str) -> ActivityRef {
    ActivityRef::Quest {
        quest_id,
        name: name.to_string(),
    }
}

fn segment_activity(name: &str) -> ActivityRef {
    ActivityRef::Segment {
        name: name.to_string(),
    }
}

/// The standing set's names, in declaration order.
fn names(standing: &[ActiveActivity]) -> Vec<&str> {
    standing
        .iter()
        .map(|activity| activity.name.as_str())
        .collect()
}

/// Co-activating quests STACKS: one hunt can genuinely advance two at
/// once, and finishing one must leave the other running. This is the
/// case a per-axis column on the event row could not express, and the
/// reason attribution is one context naming a set rather than one
/// column each.
#[test]
fn co_activating_quests_stacks_and_closes_one_at_a_time() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();

    let standing = rig
        .wait(tracker.activate_activity(quest_activity(11, "Daily: Carabok"), false))
        .unwrap();
    assert_eq!(names(&standing), vec!["Daily: Carabok"]);
    let standing = rig
        .wait(tracker.activate_activity(quest_activity(22, "Daily: Monura"), true))
        .unwrap();
    assert_eq!(
        names(&standing),
        vec!["Daily: Carabok", "Daily: Monura"],
        "declaration order, both standing"
    );
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert!(active
            .intervals
            .open_of_ref(IntervalKind::Quest, 11)
            .is_some());
        assert!(active
            .intervals
            .open_of_ref(IntervalKind::Quest, 22)
            .is_some());
    });

    // One finishes; the other keeps running.
    let standing = rig
        .wait(tracker.deactivate_activity(ActivityKey::Quest(11)))
        .unwrap();
    assert_eq!(names(&standing), vec!["Daily: Monura"]);
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert!(
            active
                .intervals
                .open_of_ref(IntervalKind::Quest, 11)
                .is_none(),
            "the completed quest's stretch closed"
        );
        assert!(
            active
                .intervals
                .open_of_ref(IntervalKind::Quest, 22)
                .is_some(),
            "the sibling daily is untouched"
        );
    });

    let rows: Vec<(i64, Option<f64>, Option<String>)> = rig
        .wait(rig.db.with_reader(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT ref_id, ended_at, label FROM session_intervals \
                 WHERE session_id = ? AND kind = 'quest' ORDER BY ref_id",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![session.id], |row| {
                    Ok((row.get(0)?, row.get(1)?, row.get(2)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        }))
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, 11);
    assert!(rows[0].1.is_some(), "the closed stretch carries an end");
    assert_eq!(rows[0].2.as_deref(), Some("Daily: Carabok"));
    assert_eq!(rows[1].0, 22);
    assert!(rows[1].1.is_none(), "the running stretch has no end yet");
}

/// Re-declaring a standing quest must not grow a second stretch (the
/// record would double-count the same quest's time): the additive
/// re-declaration is a no-op, and the exclusive one seals only the
/// others, never splitting the target's own continuous stretch.
#[test]
fn redeclaring_a_standing_quest_never_splits_its_stretch() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();

    rig.wait(tracker.activate_activity(quest_activity(7, "Daily: Repeat"), false))
        .unwrap();
    rig.wait(tracker.activate_activity(quest_activity(7, "Daily: Repeat"), true))
        .unwrap();
    rig.wait(tracker.activate_activity(quest_activity(7, "Daily: Repeat"), false))
        .unwrap();

    let count: i64 = rig
        .wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM session_intervals WHERE session_id = ? AND kind = 'quest'",
                rusqlite::params![session.id],
                |row| row.get(0),
            )?)
        }))
        .unwrap();
    assert_eq!(count, 1, "one stretch, not two");
}

/// The default declaration is the one-tap switch: declaring the next
/// daily seals the standing quest stretch in the same motion. An already
/// standing sibling of an exclusive re-declaration closes too, while the
/// target's own stretch survives unsplit.
#[test]
fn the_switch_moves_between_quests_in_one_motion() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();

    rig.wait(tracker.activate_activity(quest_activity(11, "Daily: Carabok"), false))
        .unwrap();
    let standing = rig
        .wait(tracker.activate_activity(quest_activity(22, "Daily: Monura"), false))
        .unwrap();
    assert_eq!(
        names(&standing),
        vec!["Daily: Monura"],
        "the switch closed the first"
    );

    let rows: Vec<(i64, Option<f64>)> = rig
        .wait(rig.db.with_reader(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT ref_id, ended_at FROM session_intervals \
                 WHERE session_id = ? AND kind = 'quest' ORDER BY id",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![session.id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        }))
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].0, 11);
    assert!(rows[0].1.is_some(), "the switched-away stretch closed");
    assert_eq!(rows[1].0, 22);
    assert!(rows[1].1.is_none(), "the switched-to stretch runs");
}

/// One control offers both kinds, so a tap seals whatever was standing
/// whichever kind it is: switching to a quest ends the open segment, and
/// switching to a segment ends the quest stretch. The primitive still
/// supports overlap (the next test), but the tap does not.
#[test]
fn the_switch_seals_the_standing_activity_of_either_kind() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();

    rig.wait(tracker.activate_activity(segment_activity("Rotation 1"), false))
        .unwrap();
    let standing = rig
        .wait(tracker.activate_activity(quest_activity(11, "Daily: Carabok"), false))
        .unwrap();
    assert_eq!(
        names(&standing),
        vec!["Daily: Carabok"],
        "the segment was sealed by the switch onto a quest"
    );

    let standing = rig
        .wait(tracker.activate_activity(segment_activity("Rotation 2"), false))
        .unwrap();
    assert_eq!(
        names(&standing),
        vec!["Rotation 2"],
        "and the quest stretch by the switch back onto a segment"
    );
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert!(active
            .intervals
            .open_of_ref(IntervalKind::Quest, 11)
            .is_none());
    });
}

/// Co-activation is the deliberate gesture that keeps a segment running
/// inside a quest stretch: an event recorded while both hold sits inside
/// BOTH, which the context expresses natively.
#[test]
fn co_activation_keeps_a_segment_running_inside_a_quest() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();

    rig.wait(tracker.activate_activity(quest_activity(11, "Daily: Carabok"), false))
        .unwrap();
    let standing = rig
        .wait(tracker.activate_activity(segment_activity("Rotation 1"), true))
        .unwrap();
    assert_eq!(names(&standing), vec!["Daily: Carabok", "Rotation 1"]);

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(
            active
                .intervals
                .open_of_kind(IntervalKind::Segment)
                .and_then(|interval| interval.label.as_deref()),
            Some("Rotation 1")
        );
        assert!(active
            .intervals
            .open_of_ref(IntervalKind::Quest, 11)
            .is_some());
    });
}

/// A quest stretch and a boost overlap, and an event recorded while both
/// hold sits inside BOTH. That is the whole reason attribution is a
/// context naming a set. The boost is not an activity, so the Activities
/// switch never touches it.
#[test]
fn a_context_can_name_a_quest_and_a_modifier_at_once() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();

    rig.wait(tracker.set_skill_boost(Some(50))).unwrap();
    rig.wait(tracker.activate_activity(quest_activity(3, "Daily: Overlap"), false))
        .unwrap();

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.intervals.modifier_magnitude(), Some(50.0));
        assert!(active
            .intervals
            .open_of_ref(IntervalKind::Quest, 3)
            .is_some());
    });
}

/// Stopping the session ends every stretch still open, so no interval
/// outlives the session that owns it.
#[test]
fn stopping_the_session_closes_a_standing_stretch() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();
    rig.wait(tracker.activate_activity(quest_activity(5, "Daily: Unfinished"), false))
        .unwrap();
    rig.wait(tracker.stop_session()).unwrap();

    let open: i64 = rig
        .wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM session_intervals \
                 WHERE session_id = ? AND ended_at IS NULL",
                rusqlite::params![session.id],
                |row| row.get(0),
            )?)
        }))
        .unwrap();
    assert_eq!(open, 0);
}

/// With no session running there is nothing to declare into. The signal
/// is refused rather than inventing a session or writing an orphan row.
#[test]
fn declaring_outside_a_session_records_nothing() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());

    assert!(rig
        .wait(tracker.activate_activity(quest_activity(1, "Daily: Idle"), false))
        .is_err());

    let count: i64 = rig
        .wait(rig.db.with_reader(move |conn| {
            Ok(
                conn.query_row("SELECT COUNT(*) FROM session_intervals", [], |row| {
                    row.get(0)
                })?,
            )
        }))
        .unwrap();
    assert_eq!(count, 0);
}

/// Segments stay sequential under co-activation: the gesture that lets a
/// quest and a segment overlap must not let two segments overlap, since
/// a player-drawn slice is a cut of the run rather than a state. Names
/// are kept verbatim, trimmed.
#[test]
fn a_segment_declaration_seals_the_standing_segment_even_co_activated() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();

    rig.wait(tracker.activate_activity(segment_activity("  Boss: Kreltin  "), false))
        .unwrap();
    rig.wait(tracker.activate_activity(segment_activity("Boss: Feffoid"), true))
        .unwrap();

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        let open = active
            .intervals
            .open_of_kind(IntervalKind::Segment)
            .expect("one segment open");
        assert_eq!(open.label.as_deref(), Some("Boss: Feffoid"));
    });

    let rows: Vec<(Option<String>, Option<f64>)> = rig
        .wait(rig.db.with_reader(move |conn| {
            let mut stmt = conn.prepare(
                "SELECT label, ended_at FROM session_intervals \
                 WHERE session_id = ? AND kind = 'segment' ORDER BY id",
            )?;
            let rows = stmt
                .query_map(rusqlite::params![session.id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        }))
        .unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].0.as_deref(),
        Some("Boss: Kreltin"),
        "the name is kept verbatim, trimmed"
    );
    assert!(
        rows[0].1.is_some(),
        "the first segment closed on the second's declaration"
    );
    assert_eq!(rows[1].0.as_deref(), Some("Boss: Feffoid"));
    assert!(rows[1].1.is_none(), "the standing segment has no end yet");
}

/// A segment is ended by name, because a name is all a player-drawn
/// slice has. Ending one that is not standing is a no-op: every
/// Activities verb is idempotent over the standing set, so a stale
/// control cannot fail the user.
#[test]
fn ending_a_segment_matches_it_by_name_and_is_idempotent() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();

    rig.wait(tracker.activate_activity(segment_activity("Rotation 1"), false))
        .unwrap();
    let standing = rig
        .wait(tracker.deactivate_activity(ActivityKey::Segment("Nothing here".into())))
        .unwrap();
    assert_eq!(
        names(&standing),
        vec!["Rotation 1"],
        "a name nothing is standing under changes nothing"
    );

    let standing = rig
        .wait(tracker.deactivate_activity(ActivityKey::Segment("rotation 1".into())))
        .unwrap();
    assert!(
        standing.is_empty(),
        "matched case-insensitively, as the control renders it"
    );
    let standing = rig
        .wait(tracker.deactivate_activity(ActivityKey::Segment("Rotation 1".into())))
        .unwrap();
    assert!(standing.is_empty(), "and ending it again is a no-op");
}

/// Stopping the session ends the open segment like every other
/// interval, and the Activities verbs refuse an idle tracker.
#[test]
fn segments_are_session_scoped() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());

    assert!(rig
        .wait(tracker.activate_activity(segment_activity("Idle"), false))
        .is_err());
    assert!(rig
        .wait(tracker.deactivate_activity(ActivityKey::Segment("Idle".into())))
        .is_err());

    let session = rig.wait(tracker.start_session()).unwrap();
    rig.wait(tracker.activate_activity(segment_activity("Rotation 1"), false))
        .unwrap();
    rig.wait(tracker.stop_session()).unwrap();

    let open: i64 = rig
        .wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT COUNT(*) FROM session_intervals \
                 WHERE session_id = ? AND ended_at IS NULL",
                rusqlite::params![session.id],
                |row| row.get(0),
            )?)
        }))
        .unwrap();
    assert_eq!(open, 0);
}

/// The three states the modifier declaration must keep apart. Only a
/// declared zero can serve as the unboosted baseline an effect is
/// measured against, so it must not collapse into "not declared".
#[test]
fn a_declared_zero_boost_is_distinct_from_no_declaration() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();

    // Nothing declared: no modifier interval at all.
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.intervals.modifier_magnitude(), None);
    });

    // Declared unboosted: a real interval carrying zero.
    rig.wait(tracker.set_skill_boost(Some(0))).unwrap();
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.intervals.modifier_magnitude(), Some(0.0));
    });

    // Withdrawn: back to claiming nothing.
    rig.wait(tracker.set_skill_boost(None)).unwrap();
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.intervals.modifier_magnitude(), None);
    });
}

#[test]
fn session_facet_and_declared_mob_rules() {
    let rig = rig();

    // No session: the declaration command refuses.
    let tracker = rig.tracker(Providers::default());
    assert_eq!(
        rig.wait(tracker.set_declared_mob("Atrox", "Atrox", "Young")),
        Err(TrackerCommandError::NoActiveSession)
    );
    assert_eq!(
        rig.wait(tracker.set_skill_boost(Some(25))),
        Err(TrackerCommandError::NoActiveSession)
    );

    // The facets snapshot from the config at session start: the name is
    // trimmed, and a non-positive boost reads as no boost at all.
    let facets = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            session_name: Some("  Team Hunt \u{1c}".to_string()),
            skill_boost_percent: Some(50),
            manual_mob: Some(Arc::new(|| {
                Some(("Atrox".to_string(), "Young".to_string()))
            })),
            ..Default::default()
        }),
        ..Providers::default()
    });
    let session = rig.wait(facets.start_session()).unwrap();
    rig.probe(&facets, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.facets.name.as_deref(), Some("Team Hunt"));
        assert_eq!(active.facets.skill_boost_percent, Some(50));
        // The declared mob is independent of the name: both are in force.
        assert_eq!(active.stamped_mob_name(), Some("Young Atrox"));
    });

    // Both facets persist onto the session row at start.
    let row: (Option<String>, Option<i64>) = {
        let session_id = session.id.clone();
        rig.wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT session_name, skill_boost_percent FROM tracking_sessions WHERE id = ?",
                rusqlite::params![session_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                    ))
                },
            )?)
        }))
        .unwrap()
    };
    assert_eq!(row, (Some("Team Hunt".to_string()), Some(50)));

    // A kill stamps the declared mob AND records where the stamp came
    // from, so a later detected stamp reads apart from a declared one.
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:02".into()),
        items: vec![],
        total_ped: 0.0,
    }));
    let stamped: (String, String, String, Option<String>) = {
        let session_id = session.id.clone();
        rig.wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT mob_name, mob_species, mob_maturity, mob_stamp_source \
                 FROM kills WHERE session_id = ?",
                rusqlite::params![session_id],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )?)
        }))
        .unwrap()
    };
    assert_eq!(
        stamped,
        (
            "Young Atrox".to_string(),
            "Atrox".to_string(),
            "Young".to_string(),
            Some("declared".to_string())
        )
    );

    // The two live facets move mid-session independently: the mob
    // declaration (kill-grain) and the boost (skill-gain-grain). The
    // name is session-grain and no command moves it, so it stands.
    rig.wait(facets.set_declared_mob("Old Atrox", "Atrox", "Old"))
        .unwrap();
    rig.wait(facets.set_skill_boost(Some(25))).unwrap();
    rig.probe(&facets, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.stamped_mob_name(), Some("Old Atrox"));
        assert_eq!(active.facets.skill_boost_percent, Some(25));
        // The name snapshotted at start and no command moves it.
        assert_eq!(active.facets.name.as_deref(), Some("Team Hunt"));
    });

    // A boost that runs out clears to "no boost" rather than zero.
    rig.wait(facets.set_skill_boost(None)).unwrap();
    rig.probe(&facets, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.facets.skill_boost_percent, None);
    });

    // Releasing clears only the mob; the other facets stand.
    assert_eq!(
        rig.wait(facets.release_declared_mob()).as_deref(),
        Some("Old Atrox")
    );
    assert_eq!(rig.wait(facets.release_declared_mob()), None);
    rig.probe(&facets, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.stamped_mob_name(), None);
        assert_eq!(active.facets.name.as_deref(), Some("Team Hunt"));
    });

    // With no declaration in force a kill stamps "Unknown" and carries no
    // stamp source, rather than inventing provenance.
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:09".into()),
        items: vec![],
        total_ped: 0.0,
    }));
    let undeclared: (String, Option<String>) = {
        let session_id = session.id.clone();
        rig.wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT mob_name, mob_stamp_source FROM kills \
                 WHERE session_id = ? ORDER BY timestamp DESC LIMIT 1",
                rusqlite::params![session_id],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
            )?)
        }))
        .unwrap()
    };
    assert_eq!(undeclared, ("Unknown".to_string(), None));

    // The name never moves for the session's life; the boost column
    // follows the latest declaration (a withdrawal above cleared it),
    // so the record never claims a boost the player withdrew.
    let opened: (Option<String>, Option<i64>) = {
        let session_id = session.id.clone();
        rig.wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT session_name, skill_boost_percent FROM tracking_sessions WHERE id = ?",
                rusqlite::params![session_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                    ))
                },
            )?)
        }))
        .unwrap()
    };
    assert_eq!(opened, (Some("Team Hunt".to_string()), None));
    rig.wait(facets.stop_session()).unwrap();

    // An undeclared session still guesses nothing about the boost or
    // the mob: absent is recorded as NULL. The name is the exception,
    // and not a guess: every session is an instance of a definition, so
    // an undeclared one is an instance of the protected default and
    // carries its name.
    let bare = rig.tracker(Providers::default());
    let bare_session = rig.wait(bare.start_session()).unwrap();
    rig.probe(&bare, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.facets.name.as_deref(), Some("Default Tracking"));
        assert_eq!(active.facets.skill_boost_percent, None);
        assert_eq!(active.stamped_mob_name(), None);
    });
    let bare_row: (Option<String>, Option<i64>) = {
        let session_id = bare_session.id.clone();
        rig.wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT session_name, skill_boost_percent FROM tracking_sessions WHERE id = ?",
                rusqlite::params![session_id],
                |row| {
                    Ok((
                        row.get::<_, Option<String>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                    ))
                },
            )?)
        }))
        .unwrap()
    };
    assert_eq!(bare_row, (Some("Default Tracking".to_string()), None));
    rig.wait(bare.stop_session()).unwrap();

    // A maturity-less declaration displays the bare species.
    let bare_mob = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            manual_mob: Some(Arc::new(|| Some(("Atrox".to_string(), String::new())))),
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(bare_mob.start_session()).unwrap();
    rig.probe(&bare_mob, |actor| {
        assert_eq!(
            actor.session.active().unwrap().stamped_mob_name(),
            Some("Atrox")
        );
    });
    rig.wait(bare_mob.stop_session()).unwrap();
}

/// A mid-session boost re-declaration re-lands on the session row, so
/// the record keeps the latest declaration (including a withdrawal)
/// rather than whatever the session opened under.
#[test]
fn a_boost_redeclaration_lands_on_the_session_row() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let session = rig.wait(tracker.start_session()).unwrap();

    let row_boost = |rig: &Rig, id: String| -> Option<i64> {
        rig.wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT skill_boost_percent FROM tracking_sessions WHERE id = ?",
                rusqlite::params![id],
                |row| row.get(0),
            )?)
        }))
        .unwrap()
    };

    rig.wait(tracker.set_skill_boost(Some(50))).unwrap();
    assert_eq!(row_boost(&rig, session.id.clone()), Some(50));

    // Withdrawn mid-session: the row must stop claiming it.
    rig.wait(tracker.set_skill_boost(None)).unwrap();
    assert_eq!(row_boost(&rig, session.id.clone()), None);

    // The stop re-lands the in-memory facet like the name, so a
    // contained mid-session write failure cannot strand the record.
    rig.wait(tracker.set_skill_boost(Some(25))).unwrap();
    rig.wait(tracker.stop_session()).unwrap();
    assert_eq!(row_boost(&rig, session.id.clone()), Some(25));
}

#[test]
fn reload_config_transitions_manual_mob_and_heal_state() {
    let rig = rig();
    let scripted_mob: Arc<StdMutex<Option<(String, String)>>> = Arc::new(StdMutex::new(Some((
        "Atrox".to_string(),
        "Young".to_string(),
    ))));
    let provider_view = scripted_mob.clone();
    let tracker = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            manual_mob: Some(Arc::new(move || provider_view.lock().unwrap().clone())),
            ..Default::default()
        }),
        ..Providers::default()
    });

    // Idle reload only refreshes the loot filter.
    rig.wait(tracker.reload_config());
    assert!(!tracker.is_tracking());

    rig.wait(tracker.start_session()).unwrap();
    rig.bus.publish(&BusEvent::ActiveHealToolChanged(
        ActiveHealToolChangedPayload {
            tool_name: "FAP".into(),
            cost_per_use_ped: 0.03,
            reload_seconds: 5.0,
            source: None,
        },
    ));
    rig.probe(&tracker, |actor| {
        assert_eq!(
            actor.session.active().unwrap().stamped_mob_name(),
            Some("Young Atrox")
        );
    });

    // The provider switching mobs re-stamps; switching to None clears a
    // manual stamp.
    *scripted_mob.lock().unwrap() = Some(("Feffoid".to_string(), String::new()));
    rig.wait(tracker.reload_config());
    rig.probe(&tracker, |actor| {
        assert_eq!(
            actor.session.active().unwrap().stamped_mob_name(),
            Some("Feffoid")
        );
    });
    *scripted_mob.lock().unwrap() = None;
    rig.wait(tracker.reload_config());
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.stamped_mob_name(), None);
        assert!(active.declared_mob.is_none());
    });
}

#[test]
fn tick_flushed_coalesces_dirty_mutations() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let captured = rig.capture();
    let session = rig.wait(tracker.start_session()).unwrap();

    // A clean tick wakes nothing.
    rig.bus.publish(&BusEvent::TickFlushed(TickFlushedPayload {
        timestamp: Some("2026-01-01T00:00:01".into()),
    }));
    assert_eq!(updated_events(&captured).len(), 1, "only the start event");

    // A deflection is durable protection evidence, but does not move
    // the live tracking readout and therefore keeps the next tick quiet.
    rig.bus.publish(&BusEvent::Combat(CombatPayload::Deflect {
        timestamp: "2026-01-01T00:00:01".into(),
    }));
    rig.bus.publish(&BusEvent::TickFlushed(TickFlushedPayload {
        timestamp: Some("2026-01-01T00:00:01".into()),
    }));
    assert_eq!(
        updated_events(&captured).len(),
        1,
        "deflection is not a tracking mutation"
    );
    let evidence = rig
        .wait(rig.db.with_reader(|conn| {
            // id-order: insertion (the event this test just recorded).
            Ok(conn.query_row(
                "SELECT session_id, context_id, protection_interval_id, damage, deflected \
                 FROM protection_defence_events ORDER BY id DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<f64>>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                },
            )?)
        }))
        .unwrap();
    assert_eq!(evidence.0, session.id);
    assert!(
        evidence.1.is_some(),
        "the active session context is retained"
    );
    assert_eq!((evidence.2, evidence.3, evidence.4), (None, None, 1));

    // A mutating event then a tick: one update stamped with the
    // tick's own instant.
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 5.0,
            timestamp: "2026-01-01T00:00:02".into(),
        }));
    rig.bus.publish(&BusEvent::TickFlushed(TickFlushedPayload {
        timestamp: Some("2026-01-01T00:00:02".into()),
    }));
    let events = updated_events(&captured);
    assert_eq!(events.len(), 2);
    assert_eq!(
        events[1],
        json!({
            "type": "tracking.session.updated",
            "event_version": 1,
            "occurred_at": to_iso_utc(naive_to_epoch(naive("2026-01-01T00:00:02"))),
            "payload": {"sessionId": session.id, "status": "active", "reason": "updated"},
        })
    );

    // The dirty flag resets: the next tick is silent again.
    rig.bus.publish(&BusEvent::TickFlushed(TickFlushedPayload {
        timestamp: Some("2026-01-01T00:00:03".into()),
    }));
    assert_eq!(updated_events(&captured).len(), 2);

    // An epoch-numeric tick stamp passes straight through the
    // float() leg; an absent one falls back to the injected clock.
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 5.0,
            timestamp: "2026-01-01T00:00:04".into(),
        }));
    rig.bus.publish(&BusEvent::TickFlushed(TickFlushedPayload {
        timestamp: Some("1735680000.0".into()),
    }));
    let events = updated_events(&captured);
    assert_eq!(events[2]["occurred_at"], "2024-12-31T21:20:00+00:00");

    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 5.0,
            timestamp: "2026-01-01T00:00:05".into(),
        }));
    rig.bus.publish(&BusEvent::TickFlushed(TickFlushedPayload {
        timestamp: None,
    }));
    let events = updated_events(&captured);
    assert_eq!(
        events[3]["occurred_at"],
        to_iso_utc(naive_to_epoch(naive("2026-01-01T00:00:00"))),
        "the frozen mock clock stamps the fallback"
    );

    // An unparseable timestamp drops the event (the original's
    // float() raise, contained) with the dirty flag consumed; a
    // numeric string passes through float() instead.
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 5.0,
            timestamp: "2026-01-01T00:00:06".into(),
        }));
    rig.bus.publish(&BusEvent::TickFlushed(TickFlushedPayload {
        timestamp: Some("garbage".into()),
    }));
    assert_eq!(updated_events(&captured).len(), 4);
    rig.bus.publish(&BusEvent::TickFlushed(TickFlushedPayload {
        timestamp: Some("2026-01-01T00:00:07".into()),
    }));
    assert_eq!(
        updated_events(&captured).len(),
        4,
        "the dropped event consumed the dirty flag"
    );
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 5.0,
            timestamp: "2026-01-01T00:00:08".into(),
        }));
    rig.bus.publish(&BusEvent::TickFlushed(TickFlushedPayload {
        timestamp: Some("1735680000.5".into()),
    }));
    let events = updated_events(&captured);
    assert_eq!(events[4]["occurred_at"], "2024-12-31T21:20:00.500000+00:00");
}

#[test]
fn defensive_evidence_failure_surfaces_accounting_degradation() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let captured = rig.capture();
    rig.wait(tracker.start_session()).unwrap();
    rig.execute("DROP TABLE protection_defence_events");

    rig.bus.publish(&BusEvent::Combat(CombatPayload::Deflect {
        timestamp: "2026-01-01T00:00:01".into(),
    }));
    rig.bus.publish(&BusEvent::TickFlushed(TickFlushedPayload {
        timestamp: Some("2026-01-01T00:00:01".into()),
    }));

    let warnings = rig.probe(&tracker, |actor| {
        actor.session.active().unwrap().warnings.clone()
    });
    assert_eq!(
        warnings,
        vec!["Protection accounting degraded: defensive evidence could not be saved"]
    );
    assert_eq!(
        updated_events(&captured).len(),
        2,
        "the warning invalidates the live tracking readout"
    );
}

#[test]
fn tool_change_emits_a_direct_overlay_nudge() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let captured = rig.capture();
    let _session = rig.wait(tracker.start_session()).unwrap();
    assert_eq!(updated_events(&captured).len(), 1, "only the start event");

    // A hotbar weapon-switch emits one re-hydrate nudge immediately,
    // WITHOUT waiting for a chat-log tick: the coalesced tick only
    // flushes on combat, so the overlay must be nudged directly or it
    // stays stale until the first attack.
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Rifle".into(),
            source: None,
        }));
    let events = updated_events(&captured);
    assert_eq!(events.len(), 2, "the weapon-switch nudged immediately");
    assert_eq!(events[1]["payload"]["reason"], "updated");
    assert_eq!(events[1]["payload"]["status"], "active");

    // Re-equipping the same weapon changes nothing: no nudge.
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Rifle".into(),
            source: None,
        }));
    assert_eq!(
        updated_events(&captured).len(),
        2,
        "an unchanged tool re-equip emits nothing"
    );

    // A heal-tool equip nudges on the same direct path.
    rig.bus.publish(&BusEvent::ActiveHealToolChanged(
        ActiveHealToolChangedPayload {
            tool_name: "FAP-5".into(),
            cost_per_use_ped: 0.5,
            reload_seconds: 2.5,
            source: None,
        },
    ));
    assert_eq!(
        updated_events(&captured).len(),
        3,
        "the heal-tool equip nudged immediately"
    );
}

#[test]
fn session_event_wire_shape_matches_the_python_model_dump() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    let captured = rig.capture();
    let session = rig.wait(tracker.start_session()).unwrap();

    let events = updated_events(&captured);
    let start_ts = naive_to_epoch(naive("2026-01-01T00:00:00"));
    assert_eq!(
        events[0],
        json!({
            "type": "tracking.session.updated",
            "event_version": 1,
            "occurred_at": to_iso_utc(start_ts),
            "payload": {"sessionId": session.id, "status": "active", "reason": "started"},
        })
    );
    let captured_topics: Vec<Topic> = captured
        .lock()
        .unwrap()
        .iter()
        .map(|(topic, _)| *topic)
        .collect();
    assert!(captured_topics.contains(&Topic::TrackingSessionUpdated));
}

#[test]
fn helper_pins() {
    assert_eq!(to_iso_utc(1735680000.0), "2024-12-31T21:20:00+00:00");
    assert_eq!(to_iso_utc(1735680000.5), "2024-12-31T21:20:00.500000+00:00");

    let whole = naive("2026-01-01T00:00:05");
    assert_eq!(naive_isoformat(whole), "2026-01-01T00:00:05");
    let fractional =
        NaiveDateTime::parse_from_str("2026-01-01T00:00:05.250000", "%Y-%m-%dT%H:%M:%S%.f")
            .unwrap();
    assert_eq!(naive_isoformat(fractional), "2026-01-01T00:00:05.250000");

    assert_eq!(
        parse_bus_timestamp(Some(&json!("2026-01-01T00:00:05"))),
        Some(whole)
    );
    assert_eq!(
        parse_bus_timestamp(Some(&json!("2026-01-01T00:00:05.5"))),
        NaiveDateTime::parse_from_str("2026-01-01T00:00:05.5", "%Y-%m-%dT%H:%M:%S%.f").ok()
    );
    assert_eq!(parse_bus_timestamp(Some(&json!("garbage"))), None);
    assert_eq!(parse_bus_timestamp(Some(&json!(12.5))), None);
    assert_eq!(parse_bus_timestamp(None), None);

    let delta = naive("2026-01-01T00:00:05") - naive("2026-01-01T00:00:02");
    assert_eq!(python_total_seconds(delta), 3.0);
    let negative = naive("2026-01-01T00:00:02") - naive("2026-01-01T00:00:05");
    assert_eq!(python_total_seconds(negative), -3.0);

    // The naive epoch round-trip holds in the host zone.
    let instant = naive("2026-06-15T12:30:45");
    assert_eq!(epoch_to_naive(naive_to_epoch(instant)), instant);

    // The instant basis composes to the same bytes: resolving a local
    // reading and rendering it back is the identity for representable
    // wall-clock times, and its epoch matches the naive path exactly.
    let resolved = super::time::resolve_local(instant);
    assert_eq!(
        super::time::local_isoformat(resolved),
        naive_isoformat(instant)
    );
    assert_eq!(
        super::time::instant_to_epoch(resolved),
        naive_to_epoch(instant)
    );
    assert_eq!(
        super::time::epoch_to_instant(naive_to_epoch(instant)),
        resolved
    );
}
#[test]
fn snapshot_prices_enhancer_cost_and_skips_costless_multipliers() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();

    let loot = |ts: &str, name: &str, value: f64| {
        BusEvent::LootGroup(LootGroupPayload {
            kind: LootTag,
            source_id: None,
            timestamp: Some(ts.into()),
            items: vec![LootItem {
                item_name: name.into(),
                quantity: 1,
                value_ped: value,
                is_enhancer_shrapnel: false,
            }],
            total_ped: value,
        })
    };
    rig.bus.publish(&loot("2026-01-01T00:00:02", "Hide", 2.0));
    let readout = rig.wait(tracker.snapshot()).unwrap();
    let active = readout.active.unwrap();
    // A costless kill: no rate, no multipliers (a >= admission
    // would divide by zero into infinities).
    assert_eq!(active.cost, 0.0);
    assert_eq!(active.return_rate, 0.0);
    assert_eq!(active.multiplier_last, None);
    assert_eq!(active.multiplier_avg, None);
    assert_eq!(active.multiplier_max, None);
    assert!(active.multiplier_history.is_empty());

    // Enhancer cost flows from the accumulator into the kill and
    // the live readout arithmetic.
    rig.probe(&tracker, |actor| {
        actor
            .session
            .active_mut()
            .unwrap()
            .accumulator
            .enhancer_cost = Ped(0.25);
    });
    rig.bus.publish(&loot("2026-01-01T00:00:05", "Mud", 1.0));
    rig.probe(&tracker, |actor| {
        actor
            .session
            .active_mut()
            .unwrap()
            .accumulator
            .enhancer_cost = Ped(0.5);
    });
    let active = rig.wait(tracker.snapshot()).unwrap().active.unwrap();
    assert_eq!(active.cost, 0.75);
    assert_eq!(active.returns, 3.0);
    assert_eq!(active.net, 2.25);
    assert_eq!(active.return_rate, 4.0);
    assert_eq!(active.cumulative_net_history, vec![2.0, 2.75]);

    // The unresolved enhancer cost is the dangling remainder.
    let stopped = rig.wait(tracker.stop_session()).unwrap().unwrap();
    assert_eq!(stopped.dangling_cost, Ped(0.5));
}

#[test]
fn a_carried_weapon_prices_from_its_stored_props_before_the_library_lookup() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            carried: vec![
                carried(1, "Pistol", 10.0, 0.05),
                carried(2, "Cannon", 40.0, 0.2),
            ],
            // The name lookup would price everything at 0.9.
            cost: Some(Arc::new(|_| 0.9)),
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();
    rig.bus.publish(&hit(7.0));
    rig.bus.publish(&hit(35.0));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::TargetJam {
        timestamp: "2026-01-01T00:00:02".into(),
    }));
    let pistol = carried_cost(10.0, 0.05);
    let cannon = carried_cost(40.0, 0.2);
    rig.probe(&tracker, move |actor| {
        let stats: Vec<(String, f64, i64)> = actor
            .session
            .active()
            .unwrap()
            .accumulator
            .tool_stats
            .iter()
            .map(|(key, stats)| (key.clone(), stats.cost_per_shot.value(), stats.shots_fired))
            .collect();
        assert_eq!(
            stats,
            vec![
                ("Pistol".to_string(), pistol, 1),
                ("Cannon".to_string(), cannon, 2),
            ]
        );
    });
}

#[test]
fn an_unpriced_shot_never_borrows_a_library_cost() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            // Any name lookup would answer 0.7, including "Unknown".
            cost: Some(Arc::new(|_| 0.7)),
            ..Default::default()
        }),
        ..Providers::default()
    });
    let session = rig.wait(tracker.start_session()).unwrap();
    rig.bus.publish(&hit(9.0));
    rig.bus.publish(&hit(6.0));
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:02".into()),
        items: vec![],
        total_ped: 0.0,
    }));
    let kill_id: String = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(conn.query_row("SELECT id FROM kills", [], |row| row.get::<_, String>(0))?)
        }))
        .unwrap();
    assert_eq!(
        rig.scalar_f64("SELECT cost_ped FROM kills WHERE id = ?", &[&kill_id]),
        0.0
    );
    assert_eq!(
        rig.scalar_f64(
            "SELECT cost_per_shot FROM kill_tool_stats WHERE kill_id = ? \
                 AND tool_name = 'Unknown'",
            &[&kill_id],
        ),
        0.0
    );
    // Each unpriced shot is kept for review, settled into its kill.
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM weapon_shot_evidence WHERE session_id = ? \
                 AND attribution = 'unresolved' AND tool_name IS NULL AND kill_id IS NOT NULL",
            &[&session.id],
        ),
        2
    );
}

#[test]
fn a_kill_persists_immutable_efficiency_and_session_looter_evidence() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            cost: Some(Arc::new(|_| 0.02)),
            profile: Some(Arc::new(|name| {
                (name == "MyGun").then(|| {
                    json!({
                        "weapon_catalog_id": "weapon-42",
                        "weapon_markup": 100.0,
                        "weapon_entity": {
                            "name": "MyGun",
                            "economy": {
                                "decay": 1.0,
                                "ammo_burn": 100.0,
                                "efficiency": 84.5
                            }
                        },
                        "amp_entity": null
                    })
                    .as_object()
                    .unwrap()
                    .clone()
                })
            })),
            looters: Some(crate::expected_hunting::HuntingLooterLevels {
                animal: 30.0,
                mutant: 60.0,
                robot: 90.0,
            }),
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "MyGun".into(),
            source: None,
        }));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 9.0,
            timestamp: "2026-01-01T00:00:01".into(),
        }));
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:02".into()),
        items: vec![],
        total_ped: 0.0,
    }));

    let active = rig.wait(tracker.snapshot()).unwrap().active.unwrap();
    assert_eq!(active.expected_tt_rate, Some(0.96115));
    assert_eq!(active.expected_return_coverage, Some(1.0));
    assert_eq!(
        active.expected_return_model.as_deref(),
        Some("community_v1")
    );

    let encoded: String = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT expected_economics_json FROM kill_tool_stats",
                [],
                |row| row.get(0),
            )?)
        }))
        .unwrap();
    let evidence: crate::expected_hunting::OffensiveLoadoutEvidence =
        serde_json::from_str(&encoded).unwrap();
    assert_eq!(evidence.looters.three_looter_mean(), 60.0);
    assert_eq!(evidence.components.len(), 1);
    assert_eq!(
        evidence.components[0].catalog_id.as_deref(),
        Some("weapon-42")
    );
    assert_eq!(evidence.components[0].efficiency_pct, Some(84.5));
    assert_eq!(evidence.components[0].raw_tt_per_use, 0.02);
}

#[test]
fn an_unresolved_offensive_phase_narrows_live_expected_return_coverage() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            cost: Some(Arc::new(|_| 0.02)),
            profile: Some(Arc::new(|name| {
                (name == "KnownGun").then(|| {
                    json!({
                        "weapon_catalog_id": "known",
                        "weapon_entity": {
                            "name": "KnownGun",
                            "economy": {
                                "decay": 1.0,
                                "ammo_burn": 100.0,
                                "efficiency": 80.0
                            }
                        },
                        "amp_entity": null
                    })
                    .as_object()
                    .unwrap()
                    .clone()
                })
            })),
            looters: Some(crate::expected_hunting::HuntingLooterLevels {
                animal: 50.0,
                mutant: 50.0,
                robot: 50.0,
            }),
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();
    for (tool, timestamp) in [
        ("KnownGun", "2026-01-01T00:00:01"),
        ("UnresolvedGun", "2026-01-01T00:00:02"),
    ] {
        rig.bus
            .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
                tool_name: tool.into(),
                source: None,
            }));
        rig.bus
            .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
                amount: 9.0,
                timestamp: timestamp.into(),
            }));
    }

    let active = rig.wait(tracker.snapshot()).unwrap().active.unwrap();
    assert_eq!(active.expected_tt_rate, Some(0.951));
    assert_eq!(active.expected_return_coverage, Some(0.5));
}

#[test]
fn a_costless_declared_tool_keys_its_own_entry_and_leaves_earlier_shots_unpriced() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 9.0,
            timestamp: "2026-01-01T00:00:01".into(),
        }));
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Stick".into(),
            source: None,
        }));
    rig.bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount: 6.0,
            timestamp: "2026-01-01T00:00:02".into(),
        }));
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:03".into()),
        items: vec![],
        total_ped: 0.0,
    }));
    let rows: Vec<(String, i64, f64)> = rig
        .wait(rig.db.with_reader(|conn| {
            let mut stmt =
                conn.prepare("SELECT tool_name, shots_fired, damage_dealt FROM kill_tool_stats")?;
            let rows = stmt.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, f64>(2)?,
                ))
            })?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        }))
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("Unknown".to_string(), 1, 9.0),
            ("Stick".to_string(), 1, 6.0)
        ]
    );
}

#[test]
fn break_matching_admits_every_containment_direction() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            profile: Some(Arc::new(|name| {
                (name == "MyGun").then(|| {
                    json!({"damage_enhancers": 1, "weapon_entity": {"name": "Blast Master"}})
                        .as_object()
                        .unwrap()
                        .clone()
                })
            })),
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "MyGun".into(),
            source: None,
        }));

    let break_event = |item: &str, remaining: i64| {
        BusEvent::EnhancerBreak(EnhancerBreakPayload {
            kind: EnhancerBreakTag,
            timestamp: "2026-01-01T00:00:01".into(),
            enhancer_name: "Damage Enhancer 5".into(),
            item_name: item.into(),
            remaining,
            shrapnel_ped: 0.0,
        })
    };
    let stacks = |tracker: &HuntTracker| {
        rig.probe(tracker, |actor| {
            actor.session.active().unwrap().weapons.enhancer_states["Blast Master"]
                .stacks
                .clone()
        })
    };
    // The canonical name contains the item; the item contains the
    // canonical name; the observed hotbar name contains the item;
    // the item contains the observed name. Each direction matches.
    rig.bus.publish(&break_event("Blast", 99));
    assert_eq!(stacks(&tracker), vec![99]);
    rig.bus.publish(&break_event("Blast Master Deluxe", 98));
    assert_eq!(stacks(&tracker), vec![98]);
    rig.bus.publish(&break_event("Gun", 97));
    assert_eq!(stacks(&tracker), vec![97]);
    rig.bus.publish(&break_event("MyGun Deluxe", 96));
    assert_eq!(stacks(&tracker), vec![96]);
    // No containment in any direction: ignored.
    rig.bus.publish(&break_event("Sword", 90));
    assert_eq!(stacks(&tracker), vec![96]);

    // Stopping the session drops the whole ActiveSession, weapon
    // runtime included: the clear is structural under the typestate.
    rig.wait(tracker.stop_session()).unwrap();
    rig.probe(&tracker, |actor| {
        assert!(actor.session.active().is_none());
    });
}

#[test]
fn recovery_zero_timestamp_kills_fall_back_to_the_start() {
    let rig = rig();
    rig.execute(
        "INSERT INTO tracking_sessions (id, started_at, is_active, mob_tracking_mode) \
             VALUES ('orphan2', 2000.0, 1, 'mob')",
    );
    rig.execute(
        "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, \
             timestamp, shots_fired, damage_dealt, damage_taken, critical_hits, \
             cost_ped, enhancer_cost, loot_total_ped, is_global, is_hof) \
             VALUES ('kz', 'orphan2', 'Atrox', '', '', 0.0, 1, 1.0, 0.0, 0, \
             0.1, 0.0, 1.0, 0, 0)",
    );
    let _tracker = rig.tracker(Providers::default());
    assert_eq!(
        rig.scalar_f64(
            "SELECT ended_at FROM tracking_sessions WHERE id = 'orphan2'",
            &[],
        ),
        2000.0,
        "a zero kill timestamp is falsy there, not a real maximum"
    );
}

#[test]
fn reload_config_resyncs_the_declared_mob_from_the_live_config() {
    // The declare and release commands write the config first, so a
    // reload (a settings-page edit landing mid-session) must bring the
    // in-memory declaration back in step with it rather than keep a
    // stale one.
    let rig = rig();
    let tracker = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            session_name: Some("Team".to_string()),
            manual_mob: Some(Arc::new(|| {
                Some(("Atrox".to_string(), "Young".to_string()))
            })),
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();
    rig.wait(tracker.reload_config());
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.stamped_mob_name(), Some("Young Atrox"));
        // The name facet is snapshotted at start, so a reload leaves it be.
        assert_eq!(active.facets.name.as_deref(), Some("Team"));
    });
}

#[test]
fn a_blank_configured_name_takes_the_resolved_definition_name() {
    // "Not declared" must stay distinguishable from "declared as empty":
    // a whitespace-only configured name is no name at all, and no
    // configured mob is no declaration, never a guessed default. The
    // boost's declared zero is a real declaration that the row mirror
    // simply cannot hold; the interval layer carries it (covered above).
    let rig = rig();
    let blank = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            session_name: Some("   ".to_string()),
            skill_boost_percent: Some(0),
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(blank.start_session()).unwrap();
    rig.probe(&blank, |actor| {
        let active = actor.session.active().unwrap();
        // A blank declaration is no longer nameless history: with
        // nothing configured, the resolved definition names the session.
        assert_eq!(active.facets.name.as_deref(), Some("Default Tracking"));
        assert_eq!(active.facets.skill_boost_percent, None);
        assert_eq!(active.stamped_mob_name(), None);
        assert!(active.declared_mob.is_none());
    });
    rig.wait(blank.stop_session()).unwrap();
}

#[test]
fn the_blacklist_provider_refreshes_at_session_start() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            blacklist: vec!["Mud".to_string()],
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:02".into()),
        items: vec![
            LootItem {
                item_name: "Mud".into(),
                quantity: 1,
                value_ped: 1.0,
                is_enhancer_shrapnel: false,
            },
            LootItem {
                item_name: "Hide".into(),
                quantity: 1,
                value_ped: 2.0,
                is_enhancer_shrapnel: false,
            },
        ],
        total_ped: 3.0,
    }));
    assert_eq!(
        rig.scalar_f64("SELECT loot_total_ped FROM kills", &[]),
        2.0,
        "the provider's blacklist drops Mud"
    );
    assert_eq!(
        rig.scalar_i64("SELECT COUNT(*) FROM kill_loot_items", &[]),
        1
    );
}

#[test]
fn command_error_messages_are_stable() {
    assert_eq!(
        TrackerCommandError::NoActiveSession.to_string(),
        "No active session"
    );
}

#[test]
fn enhancer_state_prices_through_the_cost_engine() {
    let props: Arc<Value> = Arc::new(json!({
        "weapon_entity": {"economy": {"decay": 0.05, "ammo_burn": 200}},
        "damage_enhancers": 2,
    }));
    let mut state = DamageEnhancerState::from_props("Rifle", props.clone());
    let priced = |slots: i64| {
        cost_per_shot_from_props(&props, Some(slots))["totalCostPerUse"]
            .as_f64()
            .unwrap()
            / 100.0
    };
    let two_slots = priced(2);
    assert!(two_slots > 0.0);
    assert_eq!(state.current_cost().value(), two_slots);
    assert_eq!(
        state.current_cost().value(),
        two_slots,
        "the cached read agrees"
    );
    state.set_total(1);
    assert_eq!(
        state.current_cost().value(),
        priced(1),
        "a stack change reprices at the new active count"
    );
}

#[test]
fn epoch_helpers_carry_and_keep_fractions() {
    assert_eq!(epoch_to_parts(5.0), (5, 0));
    assert_eq!(epoch_to_parts(2.25), (2, 250_000));
    assert_eq!(
        epoch_to_parts(1.999_999_9),
        (2, 0),
        "microsecond round-up carries into the seconds"
    );
    assert_eq!(
        epoch_to_parts(-0.25),
        (-1, 750_000),
        "negative fractions borrow a second"
    );

    let base = naive("2026-06-15T12:30:45");
    let fractional =
        NaiveDateTime::parse_from_str("2026-06-15T12:30:45.250000", "%Y-%m-%dT%H:%M:%S%.f")
            .unwrap();
    let delta = naive_to_epoch(fractional) - naive_to_epoch(base);
    assert!((delta - 0.25).abs() < 1e-9);
    assert_eq!(epoch_to_naive(naive_to_epoch(fractional)), fractional);
}
#[test]
fn a_carried_weapon_priced_at_zero_falls_back_to_the_library_cost() {
    let rig = rig();
    let mut free = carried(1, "Pistol", 10.0, 0.0);
    free.props = json!({"weapon_entity": {"damage": {"impact": 10.0},
                                          "economy": {"decay": 0, "ammo_burn": 0}}})
    .as_object()
    .unwrap()
    .clone();
    let tracker = rig.tracker(Providers {
        equipment: Arc::new(ScriptedEquipment {
            carried: vec![free],
            cost: Some(Arc::new(|_| 0.3)),
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();
    rig.bus.publish(&hit(7.0));
    rig.probe(&tracker, |actor| {
        let (key, stats) = &actor.session.active().unwrap().accumulator.tool_stats[0];
        assert_eq!(key, "Pistol");
        assert_eq!(stats.cost_per_shot, Ped(0.3));
    });
}

#[test]
fn a_global_at_the_exact_window_bound_is_not_correlated() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        player_name: "Hero".to_string(),
        ..Providers::default()
    });
    let session = rig.wait(tracker.start_session()).unwrap();
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:20".into()),
        items: vec![LootItem {
            item_name: "Hide".into(),
            quantity: 1,
            value_ped: 1.0,
            is_enhancer_shrapnel: false,
        }],
        total_ped: 1.0,
    }));
    rig.bus
        .publish(&BusEvent::Global(GlobalPayload::GlobalKill {
            timestamp: "2026-01-01T00:00:25".into(),
            player: "Hero".into(),
            creature: "Atrox".into(),
            value: 9.0,
        }));
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM kills WHERE session_id = ? AND is_global = 1",
            &[&session.id],
        ),
        0,
        "the five-second window is strict"
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM notable_events WHERE session_id = ? \
                 AND kill_id IS NULL",
            &[&session.id],
        ),
        1
    );
}

#[test]
fn reload_clears_the_declaration_once_the_config_drops_it() {
    // Releasing writes the config first, so the reload that follows must
    // drop the in-memory declaration rather than keep stamping a mob the
    // user has let go.
    let rig = rig();
    let declared = Arc::new(StdMutex::new(Some((
        "Atrox".to_string(),
        "Young".to_string(),
    ))));
    let provider_view = declared.clone();
    let tracker = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            manual_mob: Some(Arc::new(move || provider_view.lock().unwrap().clone())),
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();
    rig.probe(&tracker, |actor| {
        assert_eq!(
            actor.session.active().unwrap().stamped_mob_name(),
            Some("Young Atrox")
        );
    });

    *declared.lock().unwrap() = None;
    rig.wait(tracker.reload_config());
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.stamped_mob_name(), None);
        assert!(active.declared_mob.is_none());
    });
}

#[test]
fn prime_demo_activates_a_demo_session_and_stamps_its_mob() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    assert!(!tracker.is_tracking(), "idle before priming");

    let session = crate::tracking_models::TrackingSession {
        id: "demo".to_string(),
        start_time: chrono::DateTime::from_timestamp(1_000, 0).unwrap(),
        end_time: None,
        kills: Vec::new(),
        harvests: Vec::new(),
        dangling_cost: Ped::ZERO,
    };
    rig.wait(tracker.prime_demo(
        session,
        Some(super::mob::DeclaredMob::from_parts(
            "Atrox".to_string(),
            String::new(),
        )),
        SessionFacets::default(),
    ));

    // The demo session is live without ever running start_session.
    assert!(tracker.is_tracking(), "prime_demo activates the session");
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("a demo session is active");
        assert_eq!(active.stamped_mob_name(), Some("Atrox"));
    });
}

#[test]
fn a_weapon_press_leaves_the_shots_already_accumulated_alone() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    // A demo session gives an active session without the bus wiring; the
    // handler is exercised directly on the actor thread.
    let session = crate::tracking_models::TrackingSession {
        id: "demo".to_string(),
        start_time: chrono::DateTime::from_timestamp(1_000, 0).unwrap(),
        end_time: None,
        kills: Vec::new(),
        harvests: Vec::new(),
        dangling_cost: Ped::ZERO,
    };
    rig.wait(tracker.prime_demo(session, None, SessionFacets::default()));

    rig.probe(&tracker, |actor| {
        let unknown = crate::tracking_models::ToolStats {
            tool_name: "Unknown".to_string(),
            shots_fired: 5,
            damage_dealt: 12.0,
            critical_hits: 1,
            cost_per_shot: Ped::ZERO,
            expected_economics: None,
        };
        actor.session.active_mut().unwrap().accumulator.tool_stats =
            vec![("Unknown".to_string(), unknown.clone())];
        actor.on_weapon_press("Rifle", 1_001.0);

        let active = actor.session.active().unwrap();
        assert_eq!(
            active.accumulator.tool_stats,
            vec![("Unknown".to_string(), unknown)],
            "no evidence reaches back across the press"
        );
        assert_eq!(active.weapons.attribution.declared(), Some("Rifle"));
        // An empty name is no press at all.
        actor.on_weapon_press("", 1_002.0);
        assert_eq!(
            actor
                .session
                .active()
                .unwrap()
                .weapons
                .attribution
                .declared(),
            Some("Rifle")
        );
    });
}

#[test]
fn harvest_tool_equip_prices_wood_swings_and_fails() {
    use crate::bus_events::{ActiveHarvestToolChangedPayload, HarvestFailPayload, HarvestFailTag};

    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();

    rig.bus.publish(&BusEvent::ActiveHarvestToolChanged(
        ActiveHarvestToolChangedPayload {
            tool_name: "Terratech PH-3".into(),
            cost_per_use_ped: 0.1,
            source: Some("hotbar:4".into()),
        },
    ));
    // A wood group is a swing, priced at the equipped tool's per-use cost.
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:02".into()),
        items: vec![
            LootItem {
                item_name: "Short Moonleaf Board".into(),
                quantity: 9,
                value_ped: 0.09,
                is_enhancer_shrapnel: false,
            },
            LootItem {
                item_name: "Wood Shavings".into(),
                quantity: 8,
                value_ped: 0.008,
                is_enhancer_shrapnel: false,
            },
        ],
        total_ped: 0.098,
    }));
    // The explicit failed swing costs the same decay.
    rig.bus.publish(&BusEvent::HarvestFail(HarvestFailPayload {
        kind: HarvestFailTag,
        timestamp: "2026-01-01T00:00:04".into(),
    }));
    // A non-wood group still lands on the kill path.
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:06".into()),
        items: vec![LootItem {
            item_name: "Animal Hide".into(),
            quantity: 1,
            value_ped: 1.0,
            is_enhancer_shrapnel: false,
        }],
        total_ped: 1.0,
    }));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        let harvests = &active.session.harvests;
        assert_eq!(harvests.len(), 2, "one success + one fail");
        assert!(harvests[0].success);
        assert_eq!(harvests[0].tool_name.as_deref(), Some("Terratech PH-3"));
        assert_eq!(harvests[0].cost_ped, Ped(0.1));
        assert_eq!(harvests[0].loot_total_ped, Ped(0.098));
        assert_eq!(harvests[0].loot_items.len(), 2);
        assert!(!harvests[1].success);
        assert_eq!(harvests[1].cost_ped, Ped(0.1));
        assert!(harvests[1].loot_items.is_empty());
        assert_eq!(active.session.kills.len(), 1, "the hide group is a kill");
        assert!(
            active.warnings.is_empty(),
            "no no-tool warning when the tool is equipped"
        );
    });

    // Both swings persisted with their loot rows.
    let (events, items): (i64, i64) = rig
        .wait(rig.db.with_reader(|conn| {
            Ok((
                conn.query_row("SELECT COUNT(*) FROM harvest_events", [], |row| row.get(0))?,
                conn.query_row("SELECT COUNT(*) FROM harvest_loot_items", [], |row| {
                    row.get(0)
                })?,
            ))
        }))
        .unwrap();
    assert_eq!(events, 2);
    assert_eq!(items, 2);
}

#[test]
fn wood_loot_with_no_tool_records_zero_cost_and_warns_once() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();

    for (ts, quantity) in [("2026-01-01T00:00:02", 9), ("2026-01-01T00:00:05", 7)] {
        rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
            kind: LootTag,
            source_id: None,
            timestamp: Some(ts.into()),
            items: vec![LootItem {
                item_name: "Short Moonleaf Board".into(),
                quantity,
                value_ped: 0.01 * quantity as f64,
                is_enhancer_shrapnel: false,
            }],
            total_ped: 0.01 * quantity as f64,
        }));
    }

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        assert_eq!(active.session.harvests.len(), 2);
        for harvest in &active.session.harvests {
            assert_eq!(harvest.tool_name, None);
            assert_eq!(harvest.cost_ped, Ped::ZERO, "never guess a cost");
        }
        assert_eq!(active.session.kills.len(), 0, "no phantom kill from wood");
        assert_eq!(
            active.warnings,
            vec!["Harvesting detected: no harvesting tool equipped via hotbar".to_string()],
            "the no-tool warning is one-shot"
        );
    });
}

/// The scripted guardrail used across the guardrail tests: the
/// PH-1/PH-3/PH-4 intent the tree-cutting loadout implies.
fn guardrail_providers() -> Providers {
    Providers {
        equipment: Arc::new(ScriptedEquipment {
            harvest_guardrail: Some(HarvestGuardrailTools {
                short: Some(GuardrailTool {
                    name: "Terratech PH-1 (L)".into(),
                    cost_per_use_ped: 0.02,
                }),
                long: Some(GuardrailTool {
                    name: "Terratech PH-3".into(),
                    cost_per_use_ped: 0.1,
                }),
                huge: Some(GuardrailTool {
                    name: "Terratech PH-4 (L)".into(),
                    cost_per_use_ped: 0.875,
                }),
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn equip_harvest_tool(rig: &Rig, name: &str, cost: f64) {
    use crate::bus_events::ActiveHarvestToolChangedPayload;
    rig.bus.publish(&BusEvent::ActiveHarvestToolChanged(
        ActiveHarvestToolChangedPayload {
            tool_name: name.into(),
            cost_per_use_ped: cost,
            source: Some("hotbar:4".into()),
        },
    ));
}

fn wood_group(ts: &str, board: Option<&str>) -> BusEvent {
    let mut items = vec![LootItem {
        item_name: "Wood Shavings".into(),
        quantity: 8,
        value_ped: 0.008,
        is_enhancer_shrapnel: false,
    }];
    if let Some(name) = board {
        items.push(LootItem {
            item_name: name.into(),
            quantity: 2,
            value_ped: 0.02,
            is_enhancer_shrapnel: false,
        });
    }
    let total_ped = items.iter().map(|item| item.value_ped).sum();
    BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some(ts.into()),
        items,
        total_ped,
    })
}

#[test]
fn tree_size_classification_reads_the_board_prefix() {
    use super::harvest::tree_size_for_group;
    use crate::harvest_yield::yield_tier_for_board;

    assert_eq!(
        yield_tier_for_board("Short Moonleaf Board"),
        Some(TreeSize::Short)
    );
    assert_eq!(yield_tier_for_board("Moonleaf Board"), Some(TreeSize::Long));
    assert_eq!(
        yield_tier_for_board("Long Kaisenbrandt Board"),
        Some(TreeSize::Huge)
    );
    // No space after the prefix word: a species name, not a size.
    assert_eq!(yield_tier_for_board("Longleaf Board"), Some(TreeSize::Long));
    assert_eq!(yield_tier_for_board("Wood Shavings"), None);
    assert_eq!(yield_tier_for_board("Shrapnel"), None);

    let group = [
        LootItem {
            item_name: "Wood Shavings".into(),
            quantity: 3,
            value_ped: 0.003,
            is_enhancer_shrapnel: false,
        },
        LootItem {
            item_name: "Long Moonleaf Board".into(),
            quantity: 1,
            value_ped: 0.05,
            is_enhancer_shrapnel: false,
        },
    ];
    assert_eq!(tree_size_for_group(&group), Some(TreeSize::Huge));
    assert_eq!(tree_size_for_group(&group[..1]), None);
}

#[test]
fn guardrail_off_ph3_huge_run_attributes_all_four_swings_to_huge() {
    use crate::bus_events::{HarvestFailPayload, HarvestFailTag};

    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();
    equip_harvest_tool(&rig, "Terratech PH-3", 0.1);

    for timestamp in [
        "2026-01-01T00:00:02",
        "2026-01-01T00:00:04",
        "2026-01-01T00:00:06",
    ] {
        rig.bus.publish(&BusEvent::HarvestFail(HarvestFailPayload {
            kind: HarvestFailTag,
            timestamp: timestamp.into(),
        }));
    }
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:08",
        Some("Long Moonleaf Board"),
    ));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        assert_eq!(active.session.harvests.len(), 4);
        for (index, harvest) in active.session.harvests.iter().enumerate() {
            assert_eq!(harvest.tool_name.as_deref(), Some("Terratech PH-3"));
            assert_eq!(harvest.cost_ped, Ped(0.1));
            assert_eq!(harvest.yield_tier, HarvestYieldTier::Huge);
            assert_eq!(
                harvest.yield_tier_source,
                Some(if index == 3 {
                    HarvestYieldSource::Board
                } else {
                    HarvestYieldSource::Inferred
                })
            );
        }
    });

    let rows: Vec<(String, Option<String>, f64)> = rig
        .wait(rig.db.with_reader(|conn| {
            let mut stmt = conn.prepare(
                "SELECT yield_tier, yield_tier_source, cost_ped \
                 FROM harvest_events ORDER BY timestamp",
            )?;
            let mapped = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;
            Ok(mapped.collect::<rusqlite::Result<Vec<_>>>()?)
        }))
        .unwrap();
    assert_eq!(
        rows,
        vec![
            ("huge".into(), Some("inferred".into()), 0.1),
            ("huge".into(), Some("inferred".into()), 0.1),
            ("huge".into(), Some("inferred".into()), 0.1),
            ("huge".into(), Some("board".into()), 0.1),
        ]
    );
}

#[test]
fn conflicting_direct_evidence_leaves_the_between_swing_unclassified() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();
    equip_harvest_tool(&rig, "Terratech PH-3", 0.1);

    rig.bus
        .publish(&wood_group("2026-01-01T00:00:02", Some("Moonleaf Board")));
    rig.bus.publish(&wood_group("2026-01-01T00:00:04", None));
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:06",
        Some("Long Moonleaf Board"),
    ));
    rig.bus.publish(&wood_group("2026-01-01T00:00:08", None));

    rig.probe(&tracker, |actor| {
        let harvests = &actor
            .session
            .active()
            .expect("session is active")
            .session
            .harvests;
        assert_eq!(harvests[0].yield_tier, HarvestYieldTier::Long);
        assert_eq!(
            harvests[0].yield_tier_source,
            Some(HarvestYieldSource::Board)
        );
        assert_eq!(harvests[1].yield_tier, HarvestYieldTier::Unknown);
        assert_eq!(harvests[1].yield_tier_source, None);
        assert_eq!(harvests[2].yield_tier, HarvestYieldTier::Huge);
        assert_eq!(
            harvests[2].yield_tier_source,
            Some(HarvestYieldSource::Board)
        );
        assert_eq!(harvests[3].yield_tier, HarvestYieldTier::Huge);
        assert_eq!(
            harvests[3].yield_tier_source,
            Some(HarvestYieldSource::Inferred)
        );
    });
}

#[test]
fn yield_inference_stops_at_hotkey_and_time_boundaries() {
    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();
    equip_harvest_tool(&rig, "Terratech PH-3", 0.1);
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:02",
        Some("Long Moonleaf Board"),
    ));

    // Re-pressing the same harvesting tool starts a new action regime.
    equip_harvest_tool(&rig, "Terratech PH-3", 0.1);
    rig.bus.publish(&wood_group("2026-01-01T00:00:04", None));
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        assert_eq!(
            active.session.harvests[1].yield_tier,
            HarvestYieldTier::Unknown
        );
    });
    // Direct evidence inside that regime may classify it retroactively.
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:06",
        Some("Long Moonleaf Board"),
    ));

    // A weapon press also closes the harvesting evidence regime, before
    // the next harvesting-tool press restores the harvesting hand.
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Sollomate Opalo".into(),
            source: Some("hotbar:1".into()),
        }));
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        assert_eq!(active.harvest_press_floor, 3);
        assert_eq!(
            actor.held_item.as_ref().map(|(_, kind)| *kind),
            Some(HotbarItemKind::Weapon)
        );
    });
    equip_harvest_tool(&rig, "Terratech PH-3", 0.1);
    rig.bus.publish(&wood_group("2026-01-01T00:00:08", None));
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        assert_eq!(
            active.session.harvests[3].yield_tier,
            HarvestYieldTier::Unknown
        );
    });
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:10",
        Some("Long Moonleaf Board"),
    ));
    // A later boardless swing outside 30 seconds stays unknown.
    rig.bus.publish(&wood_group("2026-01-01T00:00:42", None));

    rig.probe(&tracker, |actor| {
        let harvests = &actor
            .session
            .active()
            .expect("session is active")
            .session
            .harvests;
        assert_eq!(harvests[0].yield_tier, HarvestYieldTier::Huge);
        assert_eq!(harvests[1].yield_tier, HarvestYieldTier::Huge);
        assert_eq!(
            harvests[1].yield_tier_source,
            Some(HarvestYieldSource::Inferred)
        );
        assert_eq!(harvests[2].yield_tier, HarvestYieldTier::Huge);
        assert_eq!(harvests[3].yield_tier, HarvestYieldTier::Huge);
        assert_eq!(
            harvests[3].yield_tier_source,
            Some(HarvestYieldSource::Inferred)
        );
        assert_eq!(harvests[4].yield_tier, HarvestYieldTier::Huge);
        assert_eq!(harvests[5].yield_tier, HarvestYieldTier::Unknown);
        assert_eq!(harvests[5].yield_tier_source, None);
    });
}

#[test]
fn guardrail_attributes_a_mismatched_swing_to_the_intended_tool() {
    let rig = rig();
    let tracker = rig.tracker(guardrail_providers());
    rig.wait(tracker.start_session()).unwrap();

    // The hotbar believes the huge-tree tool; the evidence says short.
    equip_harvest_tool(&rig, "Terratech PH-4 (L)", 0.875);
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:02",
        Some("Short Moonleaf Board"),
    ));
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:04",
        Some("Short Moonleaf Board"),
    ));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        assert_eq!(active.session.harvests.len(), 2);
        for harvest in &active.session.harvests {
            assert_eq!(
                harvest.tool_name.as_deref(),
                Some("Terratech PH-1 (L)"),
                "the intended tool wins over the hotbar belief"
            );
            assert_eq!(harvest.cost_ped, Ped(0.02));
        }
        let mismatch = active
            .guardrail_mismatch
            .as_ref()
            .expect("the disagreement stands");
        assert_eq!(mismatch.expected_tool, "Terratech PH-1 (L)");
        assert_eq!(
            mismatch.observed_tool.as_deref(),
            Some("Terratech PH-4 (L)")
        );
        assert_eq!(mismatch.tree_size, TreeSize::Short);
        assert_eq!(active.warnings.len(), 1, "the warning is one-shot");
        assert!(active.warnings[0].starts_with("Harvest guardrail:"));
    });
}

#[test]
fn guardrail_agreement_and_hotbar_presses_clear_the_mismatch() {
    let rig = rig();
    let tracker = rig.tracker(guardrail_providers());
    rig.wait(tracker.start_session()).unwrap();

    equip_harvest_tool(&rig, "Terratech PH-4 (L)", 0.875);
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:02",
        Some("Short Moonleaf Board"),
    ));
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        assert!(active.guardrail_mismatch.is_some());
    });

    // A fresh harvest-tool press re-syncs the belief and clears the cue.
    equip_harvest_tool(&rig, "Terratech PH-1 (L)", 0.02);
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        assert!(active.guardrail_mismatch.is_none());
    });

    // Agreeing evidence keeps it clear; disagreeing evidence re-arms it,
    // and a weapon press clears it again.
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:06",
        Some("Short Moonleaf Board"),
    ));
    rig.bus
        .publish(&wood_group("2026-01-01T00:00:08", Some("Moonleaf Board")));
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        let mismatch = active.guardrail_mismatch.as_ref().expect("re-armed");
        assert_eq!(mismatch.expected_tool, "Terratech PH-3");
        assert_eq!(mismatch.tree_size, TreeSize::Long);
    });
    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Sollomate Opalo".into(),
            source: Some("hotbar:1".into()),
        }));
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        assert!(
            active.guardrail_mismatch.is_none(),
            "a weapon press also re-syncs the belief"
        );
    });
}

#[test]
fn guardrail_falls_back_to_the_hotbar_belief_without_board_evidence() {
    use crate::bus_events::{HarvestFailPayload, HarvestFailTag};

    let rig = rig();
    let tracker = rig.tracker(guardrail_providers());
    rig.wait(tracker.start_session()).unwrap();

    equip_harvest_tool(&rig, "Terratech PH-4 (L)", 0.875);
    // A failed swing and a shavings-only success carry no evidence.
    rig.bus.publish(&BusEvent::HarvestFail(HarvestFailPayload {
        kind: HarvestFailTag,
        timestamp: "2026-01-01T00:00:02".into(),
    }));
    rig.bus.publish(&wood_group("2026-01-01T00:00:04", None));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        assert_eq!(active.session.harvests.len(), 2);
        for harvest in &active.session.harvests {
            assert_eq!(harvest.tool_name.as_deref(), Some("Terratech PH-4 (L)"));
            assert_eq!(harvest.cost_ped, Ped(0.875));
        }
        assert!(active.guardrail_mismatch.is_none());
        assert!(active.warnings.is_empty());
    });
}

#[test]
fn guardrail_with_no_tool_equipped_stamps_the_intended_tool() {
    let rig = rig();
    let tracker = rig.tracker(guardrail_providers());
    rig.wait(tracker.start_session()).unwrap();

    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:02",
        Some("Short Moonleaf Board"),
    ));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        let harvest = &active.session.harvests[0];
        assert_eq!(harvest.tool_name.as_deref(), Some("Terratech PH-1 (L)"));
        assert_eq!(harvest.cost_ped, Ped(0.02));
        let mismatch = active.guardrail_mismatch.as_ref().expect("flagged");
        assert_eq!(mismatch.observed_tool, None);
        assert_eq!(active.warnings.len(), 1);
        assert!(
            active.warnings[0].starts_with("Harvest guardrail:"),
            "the guardrail warning replaces the no-tool warning"
        );
    });
}

#[test]
fn a_standing_mismatch_prices_evidence_less_swings_by_the_expected_tool() {
    use crate::bus_events::{HarvestFailPayload, HarvestFailTag};

    let rig = rig();
    let tracker = rig.tracker(guardrail_providers());
    rig.wait(tracker.start_session()).unwrap();

    equip_harvest_tool(&rig, "Terratech PH-4 (L)", 0.875);
    // Board evidence arms the mismatch (short tree, PH-1 expected).
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:02",
        Some("Short Moonleaf Board"),
    ));
    // While it stands, a fail and a shavings-only swing inherit PH-1.
    rig.bus.publish(&BusEvent::HarvestFail(HarvestFailPayload {
        kind: HarvestFailTag,
        timestamp: "2026-01-01T00:00:04".into(),
    }));
    rig.bus.publish(&wood_group("2026-01-01T00:00:06", None));
    // A hotbar press clears the mismatch; a later fail follows the
    // fresh belief again.
    equip_harvest_tool(&rig, "Terratech PH-4 (L)", 0.875);
    rig.bus.publish(&BusEvent::HarvestFail(HarvestFailPayload {
        kind: HarvestFailTag,
        timestamp: "2026-01-01T00:00:08".into(),
    }));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        let harvests = &active.session.harvests;
        assert_eq!(harvests.len(), 4);
        for harvest in &harvests[1..3] {
            assert_eq!(
                harvest.tool_name.as_deref(),
                Some("Terratech PH-1 (L)"),
                "evidence-less swings inherit the standing mismatch's tool"
            );
            assert_eq!(harvest.cost_ped, Ped(0.02));
        }
        assert_eq!(
            harvests[3].tool_name.as_deref(),
            Some("Terratech PH-4 (L)"),
            "after the clearing press the belief stands again"
        );
        assert_eq!(harvests[3].cost_ped, Ped(0.875));
    });
}

#[test]
fn mismatch_setting_evidence_restamps_the_preceding_evidence_less_run() {
    use crate::bus_events::{HarvestFailPayload, HarvestFailTag};

    let rig = rig();
    let tracker = rig.tracker(guardrail_providers());
    rig.wait(tracker.start_session()).unwrap();

    equip_harvest_tool(&rig, "Terratech PH-3", 0.1);
    // An agreeing long-tree run whose swings the belief was right
    // about: its trailing fail must never be rewritten.
    rig.bus
        .publish(&wood_group("2026-01-01T00:00:02", Some("Moonleaf Board")));
    rig.bus.publish(&BusEvent::HarvestFail(HarvestFailPayload {
        kind: HarvestFailTag,
        timestamp: "2026-01-01T00:00:05".into(),
    }));
    // The desynced short-tree run, past the chain window: a fail and a
    // shavings-only swing before the first board drops, all still
    // believed PH-3.
    rig.bus.publish(&BusEvent::HarvestFail(HarvestFailPayload {
        kind: HarvestFailTag,
        timestamp: "2026-01-01T00:00:50".into(),
    }));
    rig.bus.publish(&wood_group("2026-01-01T00:00:52", None));
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:54",
        Some("Short Moonleaf Board"),
    ));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        let harvests = &active.session.harvests;
        assert_eq!(harvests.len(), 5);
        assert_eq!(
            harvests[1].tool_name.as_deref(),
            Some("Terratech PH-3"),
            "the fail beyond the chain window keeps its stamp"
        );
        assert_eq!(harvests[1].cost_ped, Ped(0.1));
        for harvest in &harvests[2..5] {
            assert_eq!(
                harvest.tool_name.as_deref(),
                Some("Terratech PH-1 (L)"),
                "the contiguous run before the evidence is re-stamped"
            );
            assert_eq!(harvest.cost_ped, Ped(0.02));
        }
    });

    // The re-stamp reached the persisted rows too.
    let rows: Vec<(Option<String>, f64)> = rig
        .wait(rig.db.with_reader(|conn| {
            let mut stmt =
                conn.prepare("SELECT tool_name, cost_ped FROM harvest_events ORDER BY timestamp")?;
            let mapped = stmt.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
            Ok(mapped.collect::<rusqlite::Result<Vec<_>>>()?)
        }))
        .unwrap();
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[1], (Some("Terratech PH-3".into()), 0.1));
    for row in &rows[2..5] {
        assert_eq!(row, &(Some("Terratech PH-1 (L)".into()), 0.02));
    }
}

#[test]
fn an_unconfigured_tree_size_stays_outside_the_guardrail_remit() {
    // Only the short size carries an intent; long-tree evidence is
    // outside the guardrail's remit and must neither inherit a
    // standing short-tree mismatch nor trigger the retro pass.
    let providers = Providers {
        equipment: Arc::new(ScriptedEquipment {
            harvest_guardrail: Some(HarvestGuardrailTools {
                short: Some(GuardrailTool {
                    name: "Terratech PH-1 (L)".into(),
                    cost_per_use_ped: 0.02,
                }),
                long: None,
                huge: None,
            }),
            ..Default::default()
        }),
        ..Default::default()
    };
    let rig = rig();
    let tracker = rig.tracker(providers);
    rig.wait(tracker.start_session()).unwrap();

    equip_harvest_tool(&rig, "Terratech PH-4 (L)", 0.875);
    // Short-tree evidence arms the mismatch (expected PH-1).
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:02",
        Some("Short Moonleaf Board"),
    ));
    // An evidence-less swing inherits the standing mismatch's tool.
    rig.bus.publish(&wood_group("2026-01-01T00:00:04", None));
    // Long-tree evidence: unconfigured size, so the belief stands and
    // nothing before it is re-stamped.
    rig.bus
        .publish(&wood_group("2026-01-01T00:00:06", Some("Moonleaf Board")));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        let harvests = &active.session.harvests;
        assert_eq!(harvests.len(), 3);
        assert_eq!(harvests[0].tool_name.as_deref(), Some("Terratech PH-1 (L)"));
        assert_eq!(
            harvests[1].tool_name.as_deref(),
            Some("Terratech PH-1 (L)"),
            "the evidence-less swing inherited the standing mismatch"
        );
        assert_eq!(
            harvests[2].tool_name.as_deref(),
            Some("Terratech PH-4 (L)"),
            "the unconfigured size follows the belief, not the mismatch"
        );
        assert_eq!(harvests[2].cost_ped, Ped(0.875));
        assert!(
            active.guardrail_mismatch.is_some(),
            "the short-tree mismatch stays standing; the long swing proves nothing about it"
        );
    });
}

#[test]
fn the_retro_pass_never_reaches_back_past_a_hotbar_press() {
    use crate::bus_events::{HarvestFailPayload, HarvestFailTag};

    let rig = rig();
    let tracker = rig.tracker(guardrail_providers());
    rig.wait(tracker.start_session()).unwrap();

    // A fail stamped under the PH-3 belief, then a press (belief
    // re-syncs to PH-4), then evidence contradicting the NEW belief.
    // The press is a boundary: the fail's stamp belongs to the earlier
    // belief regime and stays, even inside the chain window.
    equip_harvest_tool(&rig, "Terratech PH-3", 0.1);
    rig.bus.publish(&BusEvent::HarvestFail(HarvestFailPayload {
        kind: HarvestFailTag,
        timestamp: "2026-01-01T00:00:02".into(),
    }));
    equip_harvest_tool(&rig, "Terratech PH-4 (L)", 0.875);
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:05",
        Some("Short Moonleaf Board"),
    ));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        let harvests = &active.session.harvests;
        assert_eq!(harvests.len(), 2);
        assert_eq!(
            harvests[0].tool_name.as_deref(),
            Some("Terratech PH-3"),
            "the pre-press fail keeps its stamp"
        );
        assert_eq!(harvests[0].cost_ped, Ped(0.1));
        assert_eq!(harvests[1].tool_name.as_deref(), Some("Terratech PH-1 (L)"));
        assert!(active.guardrail_mismatch.is_some());
    });
}

#[test]
fn agreeing_evidence_never_restamps_preceding_swings() {
    use crate::bus_events::{HarvestFailPayload, HarvestFailTag};

    let rig = rig();
    let tracker = rig.tracker(guardrail_providers());
    rig.wait(tracker.start_session()).unwrap();

    // A genuine long-tree fail on PH-3, then a legitimate move to a
    // short tree with a proper hotbar press: the agreeing short board
    // clears nothing and rewrites nothing.
    equip_harvest_tool(&rig, "Terratech PH-3", 0.1);
    rig.bus.publish(&BusEvent::HarvestFail(HarvestFailPayload {
        kind: HarvestFailTag,
        timestamp: "2026-01-01T00:00:02".into(),
    }));
    equip_harvest_tool(&rig, "Terratech PH-1 (L)", 0.02);
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:05",
        Some("Short Moonleaf Board"),
    ));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        let harvests = &active.session.harvests;
        assert_eq!(harvests.len(), 2);
        assert_eq!(harvests[0].tool_name.as_deref(), Some("Terratech PH-3"));
        assert_eq!(harvests[0].cost_ped, Ped(0.1));
        assert!(active.guardrail_mismatch.is_none());
    });
}

#[test]
fn the_snapshot_carries_the_guardrail_mismatch_view() {
    let rig = rig();
    let tracker = rig.tracker(guardrail_providers());
    rig.wait(tracker.start_session()).unwrap();

    equip_harvest_tool(&rig, "Terratech PH-4 (L)", 0.875);
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:02",
        Some("Short Moonleaf Board"),
    ));

    let readout = rig.wait(tracker.snapshot()).unwrap();
    let active = readout.active.expect("session is active");
    let mismatch = active
        .harvest_guardrail_mismatch
        .expect("the view carries the disagreement");
    assert_eq!(mismatch.expected_tool, "Terratech PH-1 (L)");
    assert_eq!(
        mismatch.observed_tool.as_deref(),
        Some("Terratech PH-4 (L)")
    );
    assert_eq!(mismatch.tree_size, "short");

    // Without a guardrail the view stays empty on the same evidence.
    let plain = rig.tracker(Providers::default());
    rig.wait(plain.start_session()).unwrap();
    rig.bus.publish(&wood_group(
        "2026-01-01T00:00:12",
        Some("Short Moonleaf Board"),
    ));
    let readout = rig.wait(plain.snapshot()).unwrap();
    assert!(readout
        .active
        .expect("session is active")
        .harvest_guardrail_mismatch
        .is_none());
}

#[test]
fn the_snapshot_current_tool_follows_the_hand_between_weapon_healer_and_harvest() {
    use crate::bus_events::ActiveHarvestToolChangedPayload;

    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();

    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Rifle".into(),
            source: Some("hotbar:1".into()),
        }));
    let (tool, kind, _) = rig.wait(tracker.aggregate());
    assert_eq!(tool.as_deref(), Some("Rifle"));
    assert_eq!(kind, Some(HotbarItemKind::Weapon));

    rig.bus.publish(&BusEvent::ActiveHealToolChanged(
        ActiveHealToolChangedPayload {
            tool_name: "Restoration Chip 10".into(),
            cost_per_use_ped: 0.04,
            reload_seconds: 2.5,
            source: Some("hotbar:8".into()),
        },
    ));
    let (tool, kind, _) = rig.wait(tracker.aggregate());
    assert_eq!(tool.as_deref(), Some("Restoration Chip 10"));
    assert_eq!(kind, Some(HotbarItemKind::Healing));

    rig.bus.publish(&BusEvent::ActiveHarvestToolChanged(
        ActiveHarvestToolChangedPayload {
            tool_name: "Terratech PH-3".into(),
            cost_per_use_ped: 0.1,
            source: Some("hotbar:4".into()),
        },
    ));
    let (tool, kind, _) = rig.wait(tracker.aggregate());
    assert_eq!(
        tool.as_deref(),
        Some("Terratech PH-3"),
        "a harvest equip takes the displayed hand item"
    );
    assert_eq!(kind, Some(HotbarItemKind::Harvesting));

    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Rifle".into(),
            source: Some("hotbar:1".into()),
        }));
    let (tool, kind, _) = rig.wait(tracker.aggregate());
    assert_eq!(
        tool.as_deref(),
        Some("Rifle"),
        "a weapon equip takes the hand back"
    );
    assert_eq!(kind, Some(HotbarItemKind::Weapon));
}

#[test]
fn a_blacklisted_wood_group_still_routes_to_harvest_not_a_kill() {
    let rig = rig();
    let tracker = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            blacklist: vec![
                "Wood Shavings".to_string(),
                "Short Moonleaf Board".to_string(),
            ],
            ..Default::default()
        }),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();

    // Every item filtered: the swing still happened (classification
    // reads the raw group), only the recorded loot is trimmed.
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:02".into()),
        items: vec![
            LootItem {
                item_name: "Short Moonleaf Board".into(),
                quantity: 9,
                value_ped: 0.09,
                is_enhancer_shrapnel: false,
            },
            LootItem {
                item_name: "Wood Shavings".into(),
                quantity: 8,
                value_ped: 0.008,
                is_enhancer_shrapnel: false,
            },
        ],
        total_ped: 0.098,
    }));

    rig.probe(&tracker, |actor| {
        let active = actor.session.active().expect("session is active");
        assert_eq!(active.session.kills.len(), 0, "no phantom kill");
        assert_eq!(active.session.harvests.len(), 1);
        assert!(active.session.harvests[0].loot_items.is_empty());
        assert_eq!(active.session.harvests[0].loot_total_ped, Ped::ZERO);
        assert_eq!(
            active.session.harvests[0].yield_tier,
            crate::harvest_yield::HarvestYieldTier::Short
        );
        assert_eq!(
            active.session.harvests[0].yield_tier_source,
            Some(crate::harvest_yield::HarvestYieldSource::Board)
        );
    });
}

#[test]
fn the_cumulative_net_history_includes_harvest_swings() {
    use crate::bus_events::ActiveHarvestToolChangedPayload;

    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();

    rig.bus.publish(&BusEvent::ActiveHarvestToolChanged(
        ActiveHarvestToolChangedPayload {
            tool_name: "Terratech PH-1 (L)".into(),
            cost_per_use_ped: 0.02,
            source: Some("hotbar:4".into()),
        },
    ));
    for (ts, value) in [("2026-01-01T00:00:02", 0.1), ("2026-01-01T00:00:05", 0.06)] {
        rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
            kind: LootTag,
            source_id: None,
            timestamp: Some(ts.into()),
            items: vec![LootItem {
                item_name: "Short Moonleaf Board".into(),
                quantity: 1,
                value_ped: value,
                is_enhancer_shrapnel: false,
            }],
            total_ped: value,
        }));
    }

    let (_, _, aggregate) = rig.wait(tracker.aggregate());
    let aggregate = aggregate.expect("active aggregate");
    // Two swings: +0.08, then +0.04 -> running 0.08, 0.12; the curve's
    // endpoint reconciles with the displayed Net (returns - cost).
    assert_eq!(aggregate.cumulative_net, vec![0.08, 0.12]);
    assert_eq!(
        (aggregate.returns - aggregate.cost)
            .round_half_even(2)
            .value(),
        0.12
    );
}

#[test]
fn a_weapon_equip_clears_the_harvest_hand() {
    use crate::bus_events::ActiveHarvestToolChangedPayload;

    let rig = rig();
    let tracker = rig.tracker(Providers::default());
    rig.wait(tracker.start_session()).unwrap();

    rig.bus.publish(&BusEvent::ActiveHarvestToolChanged(
        ActiveHarvestToolChangedPayload {
            tool_name: "Terratech PH-1 (L)".into(),
            cost_per_use_ped: 0.02,
            source: Some("hotbar:4".into()),
        },
    ));
    rig.probe(&tracker, |actor| {
        assert_eq!(
            actor.held_item.as_ref().map(|(_, kind)| *kind),
            Some(HotbarItemKind::Harvesting)
        )
    });

    rig.bus
        .publish(&BusEvent::ActiveToolChanged(ActiveToolChangedPayload {
            tool_name: "Rifle".into(),
            source: Some("hotbar:1".into()),
        }));
    rig.probe(&tracker, |actor| {
        assert_eq!(
            actor.held_item.as_ref().map(|(_, kind)| *kind),
            Some(HotbarItemKind::Weapon)
        )
    });
}

#[test]
fn a_selected_definition_stamps_the_session_row_at_start() {
    let rig = rig();
    rig.execute("INSERT INTO session_definitions (id, name) VALUES (7, 'ARIS Dailies')");
    let tracker = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            session_name: Some("ARIS Dailies".into()),
            session_definition_id: Some(7),
            ..Default::default()
        }),
        ..Providers::default()
    });

    let session = rig.wait(tracker.start_session()).unwrap();
    assert_eq!(
        rig.scalar_i64(
            "SELECT definition_id FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        7
    );

    // The stamped reference rides the live readout for the session's life.
    let readout = rig.wait(tracker.snapshot()).unwrap();
    let active = readout.active.unwrap();
    assert_eq!(active.definition_id, Some(7));
    assert_eq!(active.session_name.as_deref(), Some("ARIS Dailies"));
}

/// Nothing about protection is declared during play: a session records
/// its hits with their context and no protection identity, even under a
/// definition authored when per-segment declaration existed, and stamps
/// the retired per-segment flag off.
#[test]
fn hits_carry_their_context_and_no_declared_protection() {
    let rig = rig();
    rig.execute(
        "INSERT INTO session_definitions \
         (id, name, track_protection_costs, track_protection_by_segment) \
         VALUES (7, 'Authored by segment', 1, 1)",
    );
    let tracker = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            session_definition_id: Some(7),
            ..Default::default()
        }),
        ..Providers::default()
    });

    let session = rig.wait(tracker.start_session()).unwrap();
    assert_eq!(
        rig.scalar_i64(
            "SELECT track_protection_by_segment FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        0
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM session_intervals WHERE session_id = ? AND kind = 'protection'",
            &[&session.id],
        ),
        0,
        "no protection interval opens"
    );
    rig.bus.publish(&BusEvent::Combat(CombatPayload::Deflect {
        timestamp: "2026-01-01T00:00:01".into(),
    }));
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM protection_defence_events \
             WHERE session_id = ? AND context_id IS NOT NULL \
               AND protection_interval_id IS NULL",
            &[&session.id],
        ),
        1
    );
}

#[test]
fn an_armour_cost_opt_out_stamps_policy_and_records_no_defence_evidence() {
    let rig = rig();
    rig.execute(
        "INSERT INTO session_definitions \
         (id, name, track_protection_costs) \
         VALUES (7, 'Offensive costs only', 0)",
    );
    let tracker = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            session_definition_id: Some(7),
            ..Default::default()
        }),
        ..Providers::default()
    });

    let session = rig.wait(tracker.start_session()).unwrap();
    let active = rig.wait(tracker.snapshot()).unwrap().active.unwrap();
    assert!(!active.track_protection_costs);
    assert_eq!(
        rig.scalar_i64(
            "SELECT track_protection_costs FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        0
    );

    rig.bus.publish(&BusEvent::Combat(CombatPayload::Deflect {
        timestamp: "2026-01-01T00:00:01".into(),
    }));
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM protection_defence_events WHERE session_id = ?",
            &[&session.id],
        ),
        0
    );
}

#[test]
fn a_stale_definition_selection_falls_through_to_the_default_and_keeps_the_name() {
    let rig = rig();
    // A definition selected and then archived while idle: the inactive
    // id must not stamp, and rather than recording an instance of
    // nothing the session becomes one of the protected default. The
    // name facet remains an honest declaration of its own.
    rig.execute(
        "INSERT INTO session_definitions (id, name, is_active) VALUES (7, 'ARIS Dailies', 0)",
    );
    let default_id: i64 = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT id FROM session_definitions WHERE is_protected = 1 AND is_active = 1",
                [],
                |row| row.get(0),
            )?)
        }))
        .unwrap();
    let tracker = rig.tracker(Providers {
        config: Arc::new(ScriptedConfig {
            session_name: Some("ARIS Dailies".into()),
            session_definition_id: Some(7),
            ..Default::default()
        }),
        ..Providers::default()
    });

    let session = rig.wait(tracker.start_session()).unwrap();
    let stamped: Option<i64> = {
        let id = session.id.clone();
        rig.wait(rig.db.with_reader(move |conn| {
            Ok(conn.query_row(
                "SELECT definition_id FROM tracking_sessions WHERE id = ?",
                rusqlite::params![id],
                |row| row.get(0),
            )?)
        }))
        .unwrap()
    };
    assert_eq!(stamped, Some(default_id));

    let readout = rig.wait(tracker.snapshot()).unwrap();
    let active = readout.active.unwrap();
    assert_eq!(active.definition_id, Some(default_id));
    assert_eq!(active.session_name.as_deref(), Some("ARIS Dailies"));
}
