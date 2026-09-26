//! Consumable doses through the tracker: booking once, re-dosing, expiry at
//! the absolute expiry, removal and exact restore, restart, the hotbar press
//! and a heal's on-use buff as starts, and re-pricing the weapons and the
//! held healer when a dose moves the reload speed in effect.

use std::sync::Arc;

use serde_json::{json, Value};

use super::tests::{healer_intent, naive, rig, Rig};
use super::{
    CarriedWeapon, CarriedWeaponProfile, DoseStart, EquipmentLibrary, EquipmentProfile,
    HarvestGuardrailTools, HuntTracker, Providers,
};
use crate::attack_rate::with_attack_rate;
use crate::bus_events::{BusEvent, CombatPayload, HotbarIntentPayload, HotbarItemKind};
use crate::chatlog_time::ChatLogClock;
use crate::clock::Clock;
use crate::consumables::{ConsumableProfile, DoseBoard, DoseEffect, DoseSource, OnUseEffect};
use crate::event_bus::{EventBus, Topic};
use crate::healing_profile::HealingProfile;
use crate::time::naive_to_epoch;

/// The Adrenaline Boost stimulant: +10% reload speed for an hour, a 3 PED
/// dose bought at 150%.
fn adrenaline(track_cost: bool) -> DoseStart {
    DoseStart {
        equipment_id: 40,
        item_name: "Nanobots - Adrenaline Boost".into(),
        profile: ConsumableProfile {
            duration_seconds: 3600.0,
            effects: vec![DoseEffect::new(
                "Reload Speed Increased",
                Some(10.0),
                Some("%"),
            )],
            tt_value_ped: 3.0,
            markup_percent: 150.0,
            track_cost,
        },
    }
}

/// A short, strong dose for the expiry and pricing cases: +20% for a minute.
fn rush() -> DoseStart {
    DoseStart {
        equipment_id: 41,
        item_name: "Rush Pill".into(),
        profile: ConsumableProfile {
            duration_seconds: 60.0,
            effects: vec![DoseEffect::new(
                "Reload Speed Increased",
                Some(20.0),
                Some("%"),
            )],
            tt_value_ped: 1.0,
            markup_percent: 100.0,
            track_cost: true,
        },
    }
}

/// A weapon at 90 attacks a minute: a +20% dose asks for 108, which the
/// server holds at 100, so each attack costs 1.08 times as much.
struct Rifle {
    board: DoseBoard,
}

impl Rifle {
    fn props(&self) -> Value {
        let base = json!({
            "weapon_entity": {
                "name": "Rifle",
                "damage": {"impact": 40.0},
                "economy": {"decay": 10.0, "ammo_burn": 0},
                "uses_per_minute": 90.0
            }
        });
        let speed = self.board.reload_speed_percent(&[]);
        with_attack_rate(&base, None, speed)
    }
}

impl EquipmentLibrary for Rifle {
    fn weapon_profile(&self, _tool_name: &str) -> EquipmentProfile {
        None
    }

    fn cost_per_shot(&self, _tool_name: &str) -> f64 {
        0.0
    }

    fn carried_weapons(&self) -> Vec<CarriedWeaponProfile> {
        let props = self.props();
        vec![CarriedWeaponProfile {
            weapon: CarriedWeapon::from_props(1, "Rifle".into(), &props),
            props: props.as_object().unwrap().clone(),
        }]
    }

    fn resolve_harvest_guardrail(&self) -> Option<HarvestGuardrailTools> {
        None
    }
}

fn board(rig: &Rig) -> DoseBoard {
    DoseBoard::new(rig.clock.clone())
}

fn tracker_with(rig: &Rig, board: &DoseBoard) -> Arc<HuntTracker> {
    rig.tracker(Providers {
        equipment: Arc::new(Rifle {
            board: board.clone(),
        }),
        doses: board.clone(),
        ..Providers::default()
    })
}

fn now(rig: &Rig) -> f64 {
    naive_to_epoch(rig.clock.now())
}

fn session_cost(rig: &Rig, session_id: &str) -> f64 {
    rig.scalar_f64(
        "SELECT consumable_cost FROM tracking_sessions WHERE id = ?",
        &[session_id],
    )
}

fn open_consumable_intervals(rig: &Rig) -> i64 {
    rig.scalar_i64(
        "SELECT COUNT(*) FROM session_intervals WHERE kind = 'consumable' AND ended_at IS NULL",
        &[],
    )
}

fn live_reload(rig: &Rig, tracker: &HuntTracker) -> f64 {
    rig.probe(tracker, |actor| {
        actor
            .session
            .active()
            .map(|active| active.reload_speed_percent)
            .unwrap_or(0.0)
    })
}

#[test]
fn a_dose_books_its_cost_once_in_the_running_session_and_stands_as_an_interval() {
    let rig = rig();
    let board = board(&rig);
    let tracker = tracker_with(&rig, &board);
    let session = rig.wait(tracker.start_session()).unwrap();

    let dose = rig.wait(tracker.start_dose(adrenaline(true))).unwrap();

    assert_eq!(dose.source, DoseSource::Manual);
    assert_eq!(dose.session_id.as_deref(), Some(session.id.as_str()));
    assert!((dose.cost_ped - 4.5).abs() < 1e-12);
    assert!((session_cost(&rig, &session.id) - 4.5).abs() < 1e-12);
    assert_eq!(open_consumable_intervals(&rig), 1);
    assert_eq!(dose.expires_at - dose.started_at, 3600.0);
    assert_eq!(board.consumed_reload_at(now(&rig)), vec![10.0]);
    assert_eq!(live_reload(&rig, &tracker), 10.0);

    // The live readout and the stopped session carry the same cost.
    rig.wait(tracker.stop_session()).unwrap();
    assert!((session_cost(&rig, &session.id) - 4.5).abs() < 1e-12);
    assert_eq!(open_consumable_intervals(&rig), 0);
}

#[test]
fn an_untracked_item_or_a_dose_outside_a_session_books_nothing_and_still_counts() {
    let rig = rig();
    let board = board(&rig);
    let tracker = tracker_with(&rig, &board);

    // Idle: the dose is recorded with no session and no cost.
    let idle = rig.wait(tracker.start_dose(adrenaline(true))).unwrap();
    assert_eq!(idle.session_id, None);
    assert_eq!(idle.cost_ped, 0.0);
    assert!(idle.cost_tracked);
    assert_eq!(board.consumed_reload_at(now(&rig)), vec![10.0]);

    // A session starting while it runs prices under it and carries its
    // context.
    let session = rig.wait(tracker.start_session()).unwrap();
    assert_eq!(live_reload(&rig, &tracker), 10.0);
    assert_eq!(open_consumable_intervals(&rig), 1);
    assert_eq!(session_cost(&rig, &session.id), 0.0);

    // Tracking off: recorded, counted, never booked.
    let untracked = rig.wait(tracker.start_dose(rush_untracked())).unwrap();
    assert_eq!(untracked.cost_ped, 0.0);
    assert_eq!(session_cost(&rig, &session.id), 0.0);
    assert_eq!(live_reload(&rig, &tracker), 20.0);
}

fn rush_untracked() -> DoseStart {
    let mut start = rush();
    start.profile.track_cost = false;
    start
}

#[test]
fn a_re_dose_ends_the_running_dose_where_it_starts_and_never_stacks() {
    let rig = rig();
    let board = board(&rig);
    let tracker = tracker_with(&rig, &board);
    let session = rig.wait(tracker.start_session()).unwrap();

    let first = rig.wait(tracker.start_dose(adrenaline(true))).unwrap();
    rig.clock.advance(600.0).unwrap();
    let second = rig.wait(tracker.start_dose(adrenaline(true))).unwrap();

    assert_eq!(
        second.supersedes_dose_id.as_deref(),
        Some(first.id.as_str())
    );
    assert_eq!(
        rig.scalar_f64(
            "SELECT superseded_at FROM consumable_doses WHERE id = ?",
            &[&first.id]
        ),
        second.started_at
    );
    // Both doses were taken, so both are paid; their effect never doubles.
    assert!((session_cost(&rig, &session.id) - 9.0).abs() < 1e-12);
    assert_eq!(board.consumed_reload_at(now(&rig)), vec![10.0]);
    assert_eq!(open_consumable_intervals(&rig), 1);
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM session_intervals WHERE kind = 'consumable'",
            &[]
        ),
        2
    );
}

#[test]
fn expiry_ends_a_dose_at_its_absolute_expiry_and_reprices() {
    let rig = rig();
    let board = board(&rig);
    let tracker = tracker_with(&rig, &board);
    rig.wait(tracker.start_session()).unwrap();
    let captured = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    {
        let captured = captured.clone();
        rig.bus.add_tap(move |event| {
            if event.topic() == Topic::ConsumablesUpdated {
                *captured.lock().unwrap() += 1;
            }
        });
    }

    let dose = rig.wait(tracker.start_dose(rush())).unwrap();
    assert_eq!(live_reload(&rig, &tracker), 20.0);
    let announced_at_start = *captured.lock().unwrap();

    // Well past the expiry before anything wakes the tracker: the interval
    // still ends at the expiry itself.
    rig.clock.advance(95.0).unwrap();
    rig.wait(tracker.wake_doses());

    assert_eq!(
        rig.scalar_f64(
            "SELECT ended_at FROM session_intervals WHERE kind = 'consumable'",
            &[]
        ),
        dose.expires_at
    );
    assert_eq!(live_reload(&rig, &tracker), 0.0);
    assert!(board.consumed_reload_at(now(&rig)).is_empty());
    assert_eq!(*captured.lock().unwrap(), announced_at_start + 1);

    // A second sweep finds nothing left to end.
    rig.wait(tracker.wake_doses());
    assert_eq!(*captured.lock().unwrap(), announced_at_start + 1);
}

#[test]
fn removing_a_dose_takes_back_its_cost_and_effect_and_restore_gives_them_back() {
    let rig = rig();
    let board = board(&rig);
    let tracker = tracker_with(&rig, &board);
    let session = rig.wait(tracker.start_session()).unwrap();
    let first = rig.wait(tracker.start_dose(adrenaline(true))).unwrap();
    rig.clock.advance(60.0).unwrap();
    let misclick = rig.wait(tracker.start_dose(adrenaline(true))).unwrap();
    assert!((session_cost(&rig, &session.id) - 9.0).abs() < 1e-12);

    let removed = rig.wait(tracker.remove_dose(&misclick.id)).unwrap();
    assert!(removed.removed_at.is_some());
    // The misclick's cost comes back off, and the dose it ended runs again
    // to its own expiry.
    assert!((session_cost(&rig, &session.id) - 4.5).abs() < 1e-12);
    let running = board.doses();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].id, first.id);
    assert_eq!(running[0].expires_at, first.expires_at);
    assert_eq!(open_consumable_intervals(&rig), 1);

    // Removing it again changes nothing.
    rig.wait(tracker.remove_dose(&misclick.id)).unwrap();
    assert!((session_cost(&rig, &session.id) - 4.5).abs() < 1e-12);

    // Restoring is exact: the cost, the early end of the first dose, and
    // one running dose.
    let restored = rig.wait(tracker.restore_dose(&misclick.id)).unwrap();
    assert_eq!(restored.removed_at, None);
    assert!((session_cost(&rig, &session.id) - 9.0).abs() < 1e-12);
    assert_eq!(
        rig.scalar_f64(
            "SELECT superseded_at FROM consumable_doses WHERE id = ?",
            &[&first.id]
        ),
        misclick.started_at
    );
    let running = board.doses();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].id, misclick.id);
    assert_eq!(open_consumable_intervals(&rig), 1);
}

#[test]
fn a_restore_is_refused_while_a_later_dose_of_the_item_runs() {
    let rig = rig();
    let board = board(&rig);
    let tracker = tracker_with(&rig, &board);
    rig.wait(tracker.start_session()).unwrap();
    let first = rig.wait(tracker.start_dose(adrenaline(true))).unwrap();
    rig.wait(tracker.remove_dose(&first.id)).unwrap();
    rig.clock.advance(30.0).unwrap();
    rig.wait(tracker.start_dose(adrenaline(true))).unwrap();

    let refused = rig.wait(tracker.restore_dose(&first.id));
    assert!(matches!(refused, Err(super::DoseError::Refused(_))));
}

#[test]
fn a_dose_outlives_a_restart_by_its_persisted_expiry() {
    let rig = rig();
    let board_one = board(&rig);
    let tracker = tracker_with(&rig, &board_one);
    rig.wait(tracker.start_session()).unwrap();
    let dose = rig.wait(tracker.start_dose(adrenaline(true))).unwrap();
    drop(tracker);

    // A fresh process: a new bus and board over the same database.
    rig.clock.advance(1200.0).unwrap();
    let board_two = board(&rig);
    let restarted = rig
        .wait(HuntTracker::new(
            Arc::new(EventBus::new()),
            rig.db.clone(),
            rig.clock.clone(),
            ChatLogClock::host_local(),
            Providers {
                equipment: Arc::new(Rifle {
                    board: board_two.clone(),
                }),
                doses: board_two.clone(),
                ..Providers::default()
            },
        ))
        .unwrap();
    let running = board_two.doses();
    assert_eq!(running.len(), 1);
    assert_eq!(running[0].id, dose.id);
    assert_eq!(running[0].expires_at, dose.expires_at);
    rig.wait(restarted.start_session()).unwrap();
    assert_eq!(live_reload(&rig, &restarted), 10.0);
}

#[test]
fn a_consumable_hotbar_press_is_the_dose() {
    let rig = rig();
    let board = board(&rig);
    let tracker = tracker_with(&rig, &board);
    let session = rig.wait(tracker.start_session()).unwrap();
    let start = adrenaline(true);

    rig.bus
        .publish(&BusEvent::HotbarIntent(Box::new(HotbarIntentPayload {
            session_id: None,
            slot: "5".into(),
            occurred_at: now(&rig),
            equipment_id: start.equipment_id,
            item_name: start.item_name.clone(),
            item_kind: HotbarItemKind::Consumable,
            cost_per_use_ped: 0.0,
            reload_seconds: 0.0,
            healing_profile: None,
            lifesteal_percent: None,
            consumable_profile: Some(start.profile),
        })));

    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM consumable_doses WHERE source = 'hotbar' AND session_id = ?",
            &[&session.id]
        ),
        1
    );
    assert!((session_cost(&rig, &session.id) - 4.5).abs() < 1e-12);
    assert_eq!(live_reload(&rig, &tracker), 10.0);
}

#[test]
fn a_dose_reprices_the_next_shot_under_the_attack_rate_limit() {
    let rig = rig();
    let board = board(&rig);
    let tracker = tracker_with(&rig, &board);
    rig.wait(tracker.start_session()).unwrap();
    let before = rig.probe(&tracker, |actor| {
        let active = actor.session.active_mut().unwrap();
        crate::attack_rate::factor_from_props(&active.weapons.carried_profiles["Rifle"])
    });
    assert_eq!(before, 1.0);

    rig.wait(tracker.start_dose(rush())).unwrap();
    let during = rig.probe(&tracker, |actor| {
        let active = actor.session.active_mut().unwrap();
        crate::attack_rate::factor_from_props(&active.weapons.carried_profiles["Rifle"])
    });
    assert!((during - 1.08).abs() < 1e-12);

    rig.clock.advance(61.0).unwrap();
    rig.wait(tracker.wake_doses());
    let after = rig.probe(&tracker, |actor| {
        let active = actor.session.active_mut().unwrap();
        crate::attack_rate::factor_from_props(&active.weapons.carried_profiles["Rifle"])
    });
    assert_eq!(after, 1.0);
}

#[test]
fn a_dose_shortens_the_held_healers_reload_at_once() {
    let rig = rig();
    let board = board(&rig);
    let tracker = tracker_with(&rig, &board);
    let session = rig.wait(tracker.start_session()).unwrap();
    let start = naive_to_epoch(naive("2026-01-01T00:00:00"));
    rig.bus.publish(&healer_intent(
        7,
        "FAP",
        0.03,
        2.5,
        start,
        HealingProfile {
            direct_min: Some(8.0),
            direct_max: Some(12.0),
            base_reload_seconds: Some(2.5),
            reload_speed_percent: Some(0.0),
            effective_reload_seconds: Some(2.5),
            ..HealingProfile::default()
        },
    ));
    rig.wait(tracker.start_dose(rush())).unwrap();
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:20".into(),
    }));
    // 2.1 s later: inside the catalogue reload less the read allowance
    // (2.2 s), outside the dosed one (2.5 / 1.2 less 0.3).
    rig.clock.advance(2.1).unwrap();
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 10.0,
        timestamp: "2026-01-01T00:00:22".into(),
    }));

    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM healing_activations WHERE session_id = ?",
            &[&session.id]
        ),
        2
    );
    assert_eq!(
        rig.scalar_f64(
            "SELECT MAX(reload_speed_percent) FROM healing_activations WHERE session_id = ?",
            &[&session.id]
        ),
        20.0
    );
}

#[test]
fn a_heal_with_an_on_use_buff_opens_a_dose_the_player_cannot_remove() {
    let rig = rig();
    let board = board(&rig);
    let tracker = tracker_with(&rig, &board);
    let session = rig.wait(tracker.start_session()).unwrap();
    rig.bus.publish(&healer_intent(
        9,
        "Eir Mk 1",
        0.05,
        2.0,
        now(&rig),
        HealingProfile {
            direct_min: Some(37.5),
            direct_max: Some(50.0),
            base_reload_seconds: Some(2.0),
            on_use: Some(OnUseEffect {
                duration_seconds: 8.0,
                effects: vec![DoseEffect::new(
                    "Reload Speed Increased",
                    Some(10.0),
                    Some("%"),
                )],
            }),
            ..HealingProfile::default()
        },
    ));
    rig.bus.publish(&BusEvent::Combat(CombatPayload::SelfHeal {
        amount: 45.0,
        timestamp: "2026-01-01T00:00:20".into(),
    }));

    let activation: String = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(conn.query_row("SELECT id FROM healing_activations", [], |row| row.get(0))?)
        }))
        .unwrap();
    let dose = board.doses()[0].clone();
    assert_eq!(dose.source, DoseSource::OnUse);
    assert_eq!(dose.expires_at - dose.started_at, 8.0);
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM consumable_doses WHERE healing_activation_id = ?",
            &[&activation]
        ),
        1
    );
    // The heal carries the cost; the buff books none and opens no interval.
    assert_eq!(session_cost(&rig, &session.id), 0.0);
    assert_eq!(open_consumable_intervals(&rig), 0);
    assert_eq!(live_reload(&rig, &tracker), 10.0);
    assert!(matches!(
        rig.wait(tracker.remove_dose(&dose.id)),
        Err(super::DoseError::Refused(_))
    ));
}

mod properties {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum Step {
        Start(bool),
        Remove(usize),
        Restore(usize),
        Wait(u16),
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            any::<bool>().prop_map(Step::Start),
            (0usize..8).prop_map(Step::Remove),
            (0usize..8).prop_map(Step::Restore),
            (1u16..120).prop_map(Step::Wait),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(16))]

        /// However doses of two items are started, removed, restored, and
        /// left to expire, the session's dose cost is exactly what its
        /// standing doses booked, the live total agrees with the row, and
        /// at most one dose of an item runs at a time.
        #[test]
        fn a_sessions_dose_cost_is_exactly_its_standing_doses(steps in proptest::collection::vec(step(), 1..24)) {
            let rig = rig();
            let board = board(&rig);
            let tracker = tracker_with(&rig, &board);
            let session = rig.wait(tracker.start_session()).unwrap();
            let mut ids: Vec<String> = Vec::new();
            for step in steps {
                match step {
                    Step::Start(first) => {
                        let start = if first { adrenaline(true) } else { rush() };
                        ids.push(rig.wait(tracker.start_dose(start)).unwrap().id);
                    }
                    Step::Remove(index) if !ids.is_empty() => {
                        let _ = rig.wait(tracker.remove_dose(&ids[index % ids.len()]));
                    }
                    Step::Restore(index) if !ids.is_empty() => {
                        let _ = rig.wait(tracker.restore_dose(&ids[index % ids.len()]));
                    }
                    Step::Wait(seconds) => {
                        rig.clock.advance(f64::from(seconds)).unwrap();
                        rig.wait(tracker.wake_doses());
                    }
                    _ => {}
                }
                let booked = rig.scalar_f64(
                    "SELECT COALESCE(SUM(cost_ped), 0) FROM consumable_doses \
                     WHERE session_id = ? AND removed_at IS NULL",
                    &[&session.id],
                );
                let row = session_cost(&rig, &session.id);
                prop_assert!((booked - row).abs() < 1e-9);
                let live = rig.probe(&tracker, |actor| {
                    actor.session.active().unwrap().consumable_cost.value()
                });
                prop_assert!((live - row).abs() < 1e-9);
                let at = now(&rig);
                let running = board.in_effect_at(at);
                for item in [40, 41] {
                    prop_assert!(
                        running.iter().filter(|dose| dose.equipment_id == Some(item)).count() <= 1
                    );
                }
            }
        }
    }
}
