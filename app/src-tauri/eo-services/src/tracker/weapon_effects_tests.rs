//! Damage-over-time effects through the tracker: the captured
//! Electrocution rotation, one opener per activation, restarts and new
//! sessions, context stamps, expiry, and overlapping casts.
//!
//! A restart is simulated the way it happens: the old process's bus goes
//! quiet, and a fresh bus and tracker open the same database, whose
//! construction recovers the orphaned session.

use std::sync::Arc;

use serde_json::{json, Value};

use super::tests::{carried, carried_cost, rig, Rig};
use super::{
    ActivityKey, ActivityRef, CarriedWeapon, CarriedWeaponProfile, EquipmentLibrary,
    EquipmentProfile, HarvestGuardrailTools, HuntTracker, Providers,
};
use crate::bus_events::{
    BusEvent, CombatPayload, HotbarIntentPayload, HotbarItemKind, LootGroupPayload, LootItem,
    LootTag,
};
use crate::chatlog_time::ChatLogClock;
use crate::clock::Clock;
use crate::event_bus::EventBus;
use crate::time::naive_to_epoch;
use crate::weapon_effect::EFFECT_PROFILE_KEY;

struct Carried(Vec<CarriedWeaponProfile>);

impl EquipmentLibrary for Carried {
    fn weapon_profile(&self, _tool_name: &str) -> EquipmentProfile {
        None
    }

    fn cost_per_shot(&self, _tool_name: &str) -> f64 {
        0.0
    }

    fn carried_weapons(&self) -> Vec<CarriedWeaponProfile> {
        self.0.clone()
    }

    fn resolve_harvest_guardrail(&self) -> Option<HarvestGuardrailTools> {
        None
    }
}

/// The primary chip of the capture, its amplifier folded into one figure
/// (a 95.7-191.4 band), at its recorded 0.366602 PED a shot.
const MAYHEM: (&str, f64, f64) = ("Mayhem", 191.4, 36.6602);
/// The Electrocution chip at its recorded 4.8732 PED a cast.
const ELECTROCUTION: (&str, f64, f64) = ("Electrocution", 2000.0, 487.32);

/// The Electrocution chip's declared effect, fitted to the capture: a
/// 100-160 opener, then ticks of 35-75 for 25 seconds.
fn electrocution_effect() -> Value {
    json!({
        "mode": "compound",
        "hit_min": 100.0,
        "hit_max": 160.0,
        "duration_seconds": 25.0,
        "tick_min": 35.0,
        "tick_max": 75.0,
        "tick_seconds": 1.15,
    })
}

fn with_effect(mut profile: CarriedWeaponProfile, effect: Value) -> CarriedWeaponProfile {
    profile.props.insert(EFFECT_PROFILE_KEY.to_string(), effect);
    profile.weapon = CarriedWeapon::from_props(
        profile.weapon.equipment_id,
        profile.weapon.name.clone(),
        &Value::Object(profile.props.clone()),
    );
    profile
}

fn loadout() -> Vec<CarriedWeaponProfile> {
    vec![
        carried(1, MAYHEM.0, MAYHEM.1, MAYHEM.2),
        with_effect(
            carried(2, ELECTROCUTION.0, ELECTROCUTION.1, ELECTROCUTION.2),
            electrocution_effect(),
        ),
    ]
}

fn mayhem() -> f64 {
    carried_cost(MAYHEM.1, MAYHEM.2)
}

fn electrocution() -> f64 {
    carried_cost(ELECTROCUTION.1, ELECTROCUTION.2)
}

/// One process lifetime over the rig's database and clock.
struct Process {
    bus: Arc<EventBus>,
    tracker: Arc<HuntTracker>,
}

fn boot(rig: &Rig) -> Process {
    boot_with(rig, loadout())
}

fn boot_with(rig: &Rig, weapons: Vec<CarriedWeaponProfile>) -> Process {
    let bus = Arc::new(EventBus::new());
    let tracker = rig
        .wait(HuntTracker::new(
            bus.clone(),
            rig.db.clone(),
            rig.clock.clone(),
            ChatLogClock::host_local(),
            Providers {
                equipment: Arc::new(Carried(weapons)),
                ..Providers::default()
            },
        ))
        .unwrap();
    Process { bus, tracker }
}

fn press(rig: &Rig, process: &Process, weapon: &str) {
    process
        .bus
        .publish(&BusEvent::HotbarIntent(HotbarIntentPayload {
            session_id: None,
            slot: "1".into(),
            occurred_at: naive_to_epoch(rig.clock.now()),
            equipment_id: 0,
            item_name: weapon.into(),
            item_kind: HotbarItemKind::Weapon,
            cost_per_use_ped: 0.0,
            reload_seconds: 0.0,
            healing_profile: None,
            lifesteal_percent: None,
        }));
}

fn hit(process: &Process, amount: f64) {
    process
        .bus
        .publish(&BusEvent::Combat(CombatPayload::DamageDealt {
            amount,
            timestamp: "2026-07-30T17:25:53".into(),
        }));
}

fn jam(process: &Process) {
    process
        .bus
        .publish(&BusEvent::Combat(CombatPayload::TargetJam {
            timestamp: "2026-07-30T17:25:53".into(),
        }));
}

fn loot(process: &Process) {
    process.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-07-30T17:26:45".into()),
        items: vec![LootItem {
            item_name: "Shrapnel".into(),
            quantity: 136263,
            value_ped: 13.62,
            is_enhancer_shrapnel: false,
        }],
        total_ped: 13.62,
    }));
}

fn window_count(rig: &Rig, process: &Process) -> usize {
    rig.probe(&process.tracker, |actor| {
        actor
            .session
            .active()
            .map(|active| active.weapons.attribution.effect_windows().len())
            .unwrap_or(0)
    })
}

/// The one stored kill's phases as (tool, shots, per-shot cost), its cost,
/// its shot count, and its damage.
fn the_kill(rig: &Rig) -> (Vec<(String, i64, f64)>, f64, i64, f64) {
    rig.wait(rig.db.with_reader(|conn| {
        let (id, cost, shots, damage): (String, f64, i64, f64) = conn.query_row(
            "SELECT id, cost_ped, shots_fired, damage_dealt FROM kills",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;
        let mut stmt = conn.prepare(
            "SELECT tool_name, shots_fired, cost_per_shot FROM kill_tool_stats \
             WHERE kill_id = ? ORDER BY id",
        )?;
        let phases = stmt
            .query_map([id], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((phases, cost, shots, damage))
    }))
    .unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// One offensive chat-log line of the capture.
#[derive(Clone, Copy)]
enum Line {
    Hit(f64),
    Jam,
    Miss,
}

/// The captured rotation (2026-07-30): each offensive line with the whole
/// second it printed at, counted from the Electrocution opener.
const ROTATION: &[(u32, Line)] = &[
    (0, Line::Hit(129.2)),
    (0, Line::Hit(0.8)),
    (1, Line::Hit(53.0)),
    (2, Line::Hit(57.5)),
    (3, Line::Hit(90.0)),
    (3, Line::Hit(55.3)),
    (4, Line::Hit(147.7)),
    (4, Line::Hit(64.3)),
    (5, Line::Hit(144.8)),
    (6, Line::Hit(63.6)),
    (6, Line::Hit(162.5)),
    (7, Line::Hit(56.0)),
    (8, Line::Miss),
    (8, Line::Hit(55.3)),
    (9, Line::Hit(93.5)),
    (9, Line::Hit(58.3)),
    (10, Line::Hit(118.1)),
    (11, Line::Hit(70.4)),
    (11, Line::Hit(135.5)),
    (12, Line::Hit(68.1)),
    (12, Line::Hit(111.1)),
    (13, Line::Hit(126.7)),
    (13, Line::Hit(62.8)),
    (14, Line::Hit(157.3)),
    (14, Line::Hit(52.2)),
    (15, Line::Hit(171.3)),
    (15, Line::Hit(58.3)),
    (16, Line::Hit(138.3)),
    (17, Line::Hit(56.0)),
    (18, Line::Jam),
    (18, Line::Hit(68.1)),
    (19, Line::Hit(167.1)),
    (19, Line::Hit(62.1)),
    (20, Line::Hit(48.4)),
    (22, Line::Hit(51.5)),
    (23, Line::Hit(62.8)),
    (24, Line::Hit(37.8)),
    (25, Line::Hit(128.0)),
    (26, Line::Jam),
    (28, Line::Hit(89.8)),
    (29, Line::Hit(148.4)),
    (30, Line::Hit(86.9)),
    (31, Line::Hit(95.3)),
    (32, Line::Hit(99.4)),
    (34, Line::Hit(120.4)),
    (35, Line::Hit(146.2)),
    (36, Line::Hit(151.8)),
    (37, Line::Jam),
    (38, Line::Hit(142.2)),
    (39, Line::Hit(116.3)),
    (41, Line::Hit(154.0)),
    (42, Line::Hit(106.4)),
    (43, Line::Hit(151.9)),
    (44, Line::Jam),
    (45, Line::Hit(97.7)),
    (47, Line::Hit(168.2)),
    (48, Line::Hit(117.2)),
    (49, Line::Hit(119.2)),
    (50, Line::Hit(167.7)),
    (52, Line::Hit(103.2)),
];

#[test]
fn the_captured_rotation_bills_the_opener_once_and_the_primary_normally() {
    let rig = rig();
    let process = boot(&rig);
    let session = rig.wait(process.tracker.start_session()).unwrap();
    press(&rig, &process, ELECTROCUTION.0);
    let mut clock_at = 0.0;
    for (index, (at, observation)) in ROTATION.iter().enumerate() {
        // The switch to the primary lands between the opener's first tick
        // and its second.
        if index == 3 {
            rig.clock.advance(1.5 - clock_at).unwrap();
            clock_at = 1.5;
            press(&rig, &process, MAYHEM.0);
        }
        let target = f64::from(*at);
        if target > clock_at {
            rig.clock.advance(target - clock_at).unwrap();
            clock_at = target;
        }
        match observation {
            Line::Hit(amount) => hit(&process, *amount),
            Line::Jam => jam(&process),
            Line::Miss => process
                .bus
                .publish(&BusEvent::Combat(CombatPayload::TargetMiss {
                    timestamp: "2026-07-30T17:26:01".into(),
                })),
        }
    }
    loot(&process);

    let (phases, cost, shots, damage) = the_kill(&rig);
    assert_eq!(
        phases,
        vec![
            (ELECTROCUTION.0.to_string(), 1, electrocution()),
            (MAYHEM.0.to_string(), 38, mayhem()),
        ],
        "one opener, and every genuine primary activation, the miss included"
    );
    assert_eq!(shots, 39, "ticks are not shots");
    // 4.8732 + 38 x 0.366602: the recorded 35.1493 carried 21 ticks as
    // shots (and missed the miss).
    assert!(close(cost, 18.804076), "{cost}");
    assert!(
        close(damage, 5565.9),
        "every damage line still counts: {damage}"
    );
    let ticks = rig.scalar_i64(
        "SELECT COUNT(*) FROM weapon_shot_evidence \
         WHERE attribution = 'effect_tick' AND effect_window_id IS NOT NULL",
        &[],
    );
    assert_eq!(ticks, 21);
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM weapon_shot_evidence WHERE attribution <> 'effect_tick'",
            &[],
        ),
        0,
        "nothing unresolved, nothing overriding the hotbar"
    );
    let window: (String, f64, f64, f64, f64) = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT session_id, hit_amount, cost_per_shot, expires_at - started_at, tick_max \
                 FROM weapon_effect_windows",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?)
        }))
        .unwrap();
    assert_eq!(window.0, session.id);
    assert!(close(window.1, 129.2));
    assert!(close(window.2, electrocution()));
    assert!(close(window.3, 25.0));
    assert!(close(window.4, 75.0));
    let readout = rig.wait(process.tracker.snapshot()).unwrap();
    assert!(
        readout.active.unwrap().weapon_guardrail_mismatch.is_none(),
        "ticks never raise a false switch"
    );
}

#[test]
fn staying_on_the_effect_weapon_bills_one_opener_per_cast() {
    let rig = rig();
    let process = boot(&rig);
    rig.wait(process.tracker.start_session()).unwrap();
    press(&rig, &process, ELECTROCUTION.0);
    hit(&process, 130.0);
    for _ in 0..10 {
        rig.clock.advance(1.0).unwrap();
        hit(&process, 50.0);
    }
    // A second cast after the first effect has run out.
    rig.clock.advance(20.0).unwrap();
    hit(&process, 120.0);
    rig.clock.advance(1.0).unwrap();
    hit(&process, 60.0);
    loot(&process);
    let (phases, cost, shots, _) = the_kill(&rig);
    assert_eq!(
        phases,
        vec![(ELECTROCUTION.0.to_string(), 2, electrocution())]
    );
    assert_eq!(shots, 2);
    assert!(close(cost, 2.0 * electrocution()));
    assert_eq!(
        rig.scalar_i64("SELECT COUNT(*) FROM weapon_effect_windows", &[]),
        2
    );
}

#[test]
fn a_jammed_or_missed_cast_is_a_shot_that_starts_no_effect() {
    let rig = rig();
    let process = boot(&rig);
    rig.wait(process.tracker.start_session()).unwrap();
    press(&rig, &process, ELECTROCUTION.0);
    jam(&process);
    process
        .bus
        .publish(&BusEvent::Combat(CombatPayload::TargetMiss {
            timestamp: "2026-07-30T17:25:53".into(),
        }));
    assert_eq!(window_count(&rig, &process), 0);
    loot(&process);
    let (phases, _, shots, _) = the_kill(&rig);
    assert_eq!(
        phases,
        vec![(ELECTROCUTION.0.to_string(), 2, electrocution())]
    );
    assert_eq!(shots, 2);
    assert_eq!(
        rig.scalar_i64("SELECT COUNT(*) FROM weapon_effect_windows", &[]),
        0
    );
}

#[test]
fn an_effect_outlives_a_restart_and_its_cost_stays_where_it_was_paid() {
    let rig = rig();
    let first = boot(&rig);
    let crashed = rig.wait(first.tracker.start_session()).unwrap();
    press(&rig, &first, ELECTROCUTION.0);
    hit(&first, 130.0);
    rig.clock.advance(1.0).unwrap();
    hit(&first, 50.0);
    loot(&first);

    // The process dies mid-effect; the next one recovers the orphan and
    // starts a new session while the effect still ticks in the game.
    rig.clock.advance(3.0).unwrap();
    let second = boot(&rig);
    let resumed = rig.wait(second.tracker.start_session()).unwrap();
    assert_eq!(window_count(&rig, &second), 1);
    press(&rig, &second, MAYHEM.0);
    rig.clock.advance(1.0).unwrap();
    hit(&second, 55.0);
    hit(&second, 150.0);
    rig.wait(second.tracker.stop_session()).unwrap();

    let tick: (String, Option<String>) = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT e.session_id, w.session_id FROM weapon_shot_evidence e \
                 JOIN weapon_effect_windows w ON w.id = e.effect_window_id \
                 ORDER BY e.observed_at DESC LIMIT 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        }))
        .unwrap();
    assert_eq!(tick.0, resumed.id, "the tick lands in the new session");
    assert_eq!(
        tick.1.as_deref(),
        Some(crashed.id.as_str()),
        "and keeps the provenance of the cast that paid for it"
    );
    assert!(close(
        rig.scalar_f64(
            "SELECT dangling_cost FROM tracking_sessions WHERE id = ?",
            &[&resumed.id],
        ),
        mayhem()
    ));
    assert!(close(
        rig.scalar_f64(
            "SELECT SUM(cost_ped) FROM kills WHERE session_id = ?",
            &[&crashed.id],
        ),
        electrocution()
    ));

    // Once the effect has run out, a new session starts without it.
    rig.clock.advance(30.0).unwrap();
    rig.wait(second.tracker.start_session()).unwrap();
    assert_eq!(window_count(&rig, &second), 0);
}

#[test]
fn keeping_the_hotbar_weapon_takes_back_an_effect_its_evidence_opened() {
    let rig = rig();
    // A pistol whose band no cast or tick shares, and the chip.
    let weapons = || {
        vec![
            carried(1, "Pistol", 10.0, 5.0),
            with_effect(
                carried(2, ELECTROCUTION.0, ELECTROCUTION.1, ELECTROCUTION.2),
                electrocution_effect(),
            ),
        ]
    };
    let process = boot_with(&rig, weapons());
    rig.wait(process.tracker.start_session()).unwrap();
    press(&rig, &process, "Pistol");
    rig.clock.advance(1.0).unwrap();
    // Only the chip explains it: a missed switch, and its effect opens.
    hit(&process, 130.0);
    assert_eq!(window_count(&rig, &process), 1);
    assert!(rig
        .wait(
            process
                .tracker
                .decide_weapon_mismatch(super::MismatchDecision::Keep)
        )
        .unwrap());
    assert_eq!(window_count(&rig, &process), 0, "the cast is taken back");
    let (withdrawn, by_review): (Option<f64>, Option<String>) = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT withdrawn_at, withdrawn_by_review_id FROM weapon_effect_windows",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        }))
        .unwrap();
    assert!(withdrawn.is_some());
    assert_eq!(
        by_review,
        Some(rig.scalar_string("SELECT id FROM weapon_attribution_reviews", &[]))
    );
    // A tick-shaped hit is no tick of it now.
    rig.clock.advance(1.0).unwrap();
    hit(&process, 55.0);
    assert_eq!(
        rig.probe(&process.tracker, |actor| {
            actor
                .session
                .active()
                .unwrap()
                .weapons
                .attribution
                .counts()
                .effect_ticks
        }),
        0
    );
    // Nor after a restart: a taken-back window is never read back.
    let second = boot_with(&rig, weapons());
    rig.wait(second.tracker.start_session()).unwrap();
    assert_eq!(window_count(&rig, &second), 0);
}

#[test]
fn a_tick_stamps_the_context_it_lands_in() {
    let rig = rig();
    let process = boot(&rig);
    rig.wait(process.tracker.start_session()).unwrap();
    press(&rig, &process, ELECTROCUTION.0);
    hit(&process, 130.0);
    rig.wait(process.tracker.activate_activity(
        ActivityRef::Segment {
            name: "Boss".into(),
        },
        false,
    ))
    .unwrap();
    let boss = rig.probe(&process.tracker, |actor| {
        actor.session.active().unwrap().intervals.context_id()
    });
    rig.clock.advance(1.0).unwrap();
    hit(&process, 50.0);
    rig.wait(
        process
            .tracker
            .deactivate_activity(ActivityKey::Segment("Boss".into())),
    )
    .unwrap();
    loot(&process);
    let (window_context, tick_context): (Option<i64>, Option<i64>) = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT w.context_id, e.context_id FROM weapon_shot_evidence e \
                 JOIN weapon_effect_windows w ON w.id = e.effect_window_id",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        }))
        .unwrap();
    assert_ne!(window_context, tick_context);
    assert_eq!(tick_context, boss);
}

#[test]
fn overlapping_casts_leave_their_shared_ticks_unclaimed_and_free() {
    let rig = rig();
    let process = boot(&rig);
    rig.wait(process.tracker.start_session()).unwrap();
    press(&rig, &process, ELECTROCUTION.0);
    hit(&process, 130.0);
    rig.clock.advance(5.0).unwrap();
    hit(&process, 140.0);
    rig.clock.advance(1.0).unwrap();
    hit(&process, 50.0);
    loot(&process);
    let (phases, _, _, _) = the_kill(&rig);
    assert_eq!(
        phases,
        vec![(ELECTROCUTION.0.to_string(), 2, electrocution())]
    );
    let (window, candidates): (Option<String>, String) = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT effect_window_id, effect_candidates_json FROM weapon_shot_evidence",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        }))
        .unwrap();
    assert_eq!(window, None, "no cast is chosen by the order it opened in");
    let candidates: Vec<Value> = serde_json::from_str(&candidates).unwrap();
    assert_eq!(candidates.len(), 2);
}

mod properties {
    use std::collections::BTreeMap;

    use proptest::prelude::*;

    use super::*;

    /// One step of a generated rotation.
    #[derive(Debug, Clone)]
    enum Op {
        /// Press the primary (false) or the Electrocution chip (true).
        Press(bool),
        /// A hit the chip's opener range holds (and the primary's band too).
        Cast,
        /// A hit only the chip's tick range holds.
        Tick,
        /// A hit only the primary's band holds.
        Primary,
        /// A sliver of damage short of every band.
        Sliver,
        Jam,
        Advance(f64),
        Loot,
        /// The process dies; the next one starts a fresh session.
        Restart,
    }

    fn op() -> impl Strategy<Value = Op> {
        prop_oneof![
            2 => any::<bool>().prop_map(Op::Press),
            2 => Just(Op::Cast),
            4 => Just(Op::Tick),
            3 => Just(Op::Primary),
            1 => Just(Op::Sliver),
            1 => Just(Op::Jam),
            4 => prop::sample::select(vec![0.5, 1.0, 2.0, 5.0, 12.0, 30.0]).prop_map(Op::Advance),
            1 => Just(Op::Loot),
            1 => Just(Op::Restart),
        ]
    }

    /// What a played rotation left in the database.
    #[derive(Debug, PartialEq)]
    struct Outcome {
        /// Every session's weapon cost: its kills' and its dangling cost.
        cost: f64,
        /// Shots per weapon across the stored kills.
        shots: BTreeMap<String, i64>,
    }

    /// Play the rotation; with `extra_ticks`, three more tick lines land
    /// right after every cast the chip in hand paid for, inside its effect.
    fn play(ops: &[Op], extra_ticks: bool) -> Outcome {
        let rig = rig();
        let mut process = boot(&rig);
        rig.wait(process.tracker.start_session()).unwrap();
        let mut chip_in_hand = false;
        for op in ops {
            match op {
                Op::Press(chip) => {
                    press(
                        &rig,
                        &process,
                        if *chip { ELECTROCUTION.0 } else { MAYHEM.0 },
                    );
                    chip_in_hand = *chip;
                }
                Op::Cast => {
                    hit(&process, 130.0);
                    if extra_ticks && chip_in_hand {
                        for _ in 0..3 {
                            hit(&process, 55.0);
                        }
                    }
                }
                Op::Tick => hit(&process, 55.0),
                Op::Primary => hit(&process, 180.0),
                Op::Sliver => hit(&process, 1.0),
                Op::Jam => jam(&process),
                Op::Advance(seconds) => rig.clock.advance(*seconds).unwrap(),
                Op::Loot => loot(&process),
                Op::Restart => {
                    process = boot(&rig);
                    rig.wait(process.tracker.start_session()).unwrap();
                    chip_in_hand = false;
                }
            }
        }
        rig.wait(process.tracker.stop_session()).unwrap();
        assert_invariants(&rig);
        rig.wait(rig.db.with_reader(|conn| {
            let cost: f64 = conn.query_row(
                "SELECT COALESCE((SELECT SUM(cost_ped) FROM kills), 0) + \
                        COALESCE((SELECT SUM(dangling_cost) FROM tracking_sessions), 0)",
                [],
                |row| row.get(0),
            )?;
            let mut stmt = conn.prepare(
                "SELECT tool_name, SUM(shots_fired) FROM kill_tool_stats GROUP BY tool_name",
            )?;
            let shots = stmt
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<BTreeMap<String, i64>>>()?;
            Ok(Outcome { cost, shots })
        }))
        .unwrap()
    }

    /// What must hold of any played rotation: a tick costs nothing and is
    /// priced to no weapon; a tick a window claims lies inside that window;
    /// a kill's shots are exactly its phases' shots; every window was opened
    /// by the chip, for its declared duration.
    fn assert_invariants(rig: &Rig) {
        rig.wait(rig.db.with_reader(|conn| {
            let bad_ticks: i64 = conn.query_row(
                "SELECT COUNT(*) FROM weapon_shot_evidence \
                 WHERE attribution = 'effect_tick' AND (cost_per_shot <> 0 OR tool_name IS NOT NULL)",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(bad_ticks, 0, "a tick carried a price");
            let outside: i64 = conn.query_row(
                "SELECT COUNT(*) FROM weapon_shot_evidence e \
                 JOIN weapon_effect_windows w ON w.id = e.effect_window_id \
                 WHERE e.observed_at < w.started_at - 0.05 OR e.observed_at > w.expires_at + 1.25",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(outside, 0, "a tick outside the window that claims it");
            let mismatched: i64 = conn.query_row(
                "SELECT COUNT(*) FROM kills k WHERE k.shots_fired <> \
                     (SELECT COALESCE(SUM(t.shots_fired), 0) FROM kill_tool_stats t \
                      WHERE t.kill_id = k.id)",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(mismatched, 0, "a kill's shots differ from its phases'");
            let foreign: i64 = conn.query_row(
                "SELECT COUNT(*) FROM weapon_effect_windows \
                 WHERE tool_name <> ?1 OR abs(expires_at - started_at - 25.0) > 1e-9",
                [ELECTROCUTION.0],
                |row| row.get(0),
            )?;
            assert_eq!(foreign, 0, "a window the chip's declared effect did not open");
            Ok(())
        }))
        .unwrap();
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]

        /// Whatever the rotation, restarts, and loot: more ticks inside an
        /// open effect never add a shot or a cost. The cast is billed once
        /// however many ticks it produces.
        #[test]
        fn ticks_inside_an_open_effect_never_bill(ops in prop::collection::vec(op(), 1..40)) {
            let plain = play(&ops, false);
            let ticking = play(&ops, true);
            prop_assert_eq!(&plain.shots, &ticking.shots);
            prop_assert!(
                close(plain.cost, ticking.cost),
                "{} against {}",
                plain.cost,
                ticking.cost
            );
        }
    }
}
