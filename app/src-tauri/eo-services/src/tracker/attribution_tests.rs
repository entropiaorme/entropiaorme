//! Weapon attribution through the tracker: missed switches, decisions on a
//! standing mismatch, unpriced shots, and the conservation of cost between
//! memory and the database.

use std::sync::{Arc, Mutex};

use super::tests::{carried, carried_cost, hit, rig, Rig};
use super::{
    CarriedWeaponProfile, EquipmentLibrary, EquipmentProfile, HarvestGuardrailTools, HuntTracker,
    MismatchDecision, Providers, WeaponDecisionError,
};
use crate::bus_events::{
    ActiveHarvestToolChangedPayload, BusEvent, CombatPayload, HotbarIntentPayload, HotbarItemKind,
    LootGroupPayload, LootTag,
};
use crate::clock::Clock;
use crate::ped::Ped;
use crate::time::naive_to_epoch;

/// The carried weapons, swappable mid-session like a settings edit.
struct Carried(Mutex<Vec<CarriedWeaponProfile>>);

impl EquipmentLibrary for Carried {
    fn weapon_profile(&self, _tool_name: &str) -> EquipmentProfile {
        None
    }

    fn cost_per_shot(&self, _tool_name: &str) -> f64 {
        0.0
    }

    fn carried_weapons(&self) -> Vec<CarriedWeaponProfile> {
        self.0.lock().unwrap().clone()
    }

    fn resolve_harvest_guardrail(&self) -> Option<HarvestGuardrailTools> {
        None
    }
}

/// Pistol 5-10, Cannon 20-40, Rifle 8-16 (overlapping the pistol).
fn arsenal() -> Vec<CarriedWeaponProfile> {
    vec![
        carried(1, "Pistol", 10.0, 0.05),
        carried(2, "Cannon", 40.0, 0.2),
        carried(3, "Rifle", 16.0, 0.1),
    ]
}

fn pistol() -> f64 {
    carried_cost(10.0, 0.05)
}

fn cannon() -> f64 {
    carried_cost(40.0, 0.2)
}

fn tracker_with(rig: &Rig, weapons: Vec<CarriedWeaponProfile>) -> Arc<HuntTracker> {
    rig.tracker(Providers {
        equipment: Arc::new(Carried(Mutex::new(weapons))),
        ..Providers::default()
    })
}

fn now(rig: &Rig) -> f64 {
    naive_to_epoch(rig.clock.now())
}

fn press(rig: &Rig, weapon: &str) {
    rig.bus
        .publish(&BusEvent::HotbarIntent(Box::new(HotbarIntentPayload {
            session_id: None,
            slot: "1".into(),
            occurred_at: now(rig),
            equipment_id: 1,
            item_name: weapon.into(),
            item_kind: HotbarItemKind::Weapon,
            cost_per_use_ped: 0.0,
            reload_seconds: 0.0,
            healing_profile: None,
            lifesteal_percent: None,
            consumable_profile: None,
        })));
}

fn jam(rig: &Rig) {
    rig.bus.publish(&BusEvent::Combat(CombatPayload::TargetJam {
        timestamp: "2026-01-01T00:00:01".into(),
    }));
}

fn loot(rig: &Rig) {
    rig.bus.publish(&BusEvent::LootGroup(LootGroupPayload {
        kind: LootTag,
        source_id: None,
        timestamp: Some("2026-01-01T00:00:02".into()),
        items: vec![],
        total_ped: 0.0,
    }));
}

/// The accumulator's phases as (tool, shots, per-shot cost).
fn pending(rig: &Rig, tracker: &HuntTracker) -> Vec<(String, i64, f64)> {
    rig.probe(tracker, |actor| {
        actor
            .session
            .active()
            .unwrap()
            .accumulator
            .tool_stats
            .iter()
            .map(|(_, stats)| {
                (
                    stats.tool_name.clone(),
                    stats.shots_fired,
                    stats.cost_per_shot.value(),
                )
            })
            .collect()
    })
}

/// One stored kill's phases as (tool, shots, per-shot cost), in write order.
fn stored_phases(rig: &Rig, kill_id: &str) -> Vec<(String, i64, f64)> {
    let kill_id = kill_id.to_string();
    rig.wait(rig.db.with_reader(move |conn| {
        let mut stmt = conn.prepare(
            "SELECT tool_name, shots_fired, cost_per_shot FROM kill_tool_stats \
             WHERE kill_id = ? ORDER BY id",
        )?;
        let rows = stmt.query_map([kill_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }))
    .unwrap()
}

fn kill_ids(rig: &Rig) -> Vec<String> {
    rig.wait(rig.db.with_reader(|conn| {
        let mut stmt = conn.prepare("SELECT id FROM kills ORDER BY timestamp, rowid")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }))
    .unwrap()
}

fn close(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

#[test]
fn a_missed_switch_is_recorded_to_the_evidence_weapon_and_raises_the_cue() {
    let rig = rig();
    let tracker = tracker_with(&rig, arsenal());
    let session = rig.wait(tracker.start_session()).unwrap();
    press(&rig, "Pistol");
    rig.bus.publish(&hit(7.0));
    rig.clock.advance(2.0).unwrap();
    rig.bus.publish(&hit(30.0));

    let readout = rig.wait(tracker.snapshot()).unwrap();
    assert_eq!(readout.current_tool.as_deref(), Some("Pistol"));
    let active = readout.active.unwrap();
    let cue = active.weapon_guardrail_mismatch.expect("the cue stands");
    assert_eq!(cue.hotbar_tool, "Pistol");
    assert_eq!(cue.recording_tool, "Cannon");
    assert_eq!(cue.shots, 1);
    assert_eq!(active.unpriced_shots, 0);
    assert_eq!(
        pending(&rig, &tracker),
        vec![
            ("Pistol".to_string(), 1, pistol()),
            ("Cannon".to_string(), 1, cannon()),
        ],
        "the evidence shot costs what the cannon costs, not the hotbar's pistol"
    );

    loot(&rig);
    let kill = kill_ids(&rig).pop().unwrap();
    let row: (String, String, String, String, f64, String) = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT kill_id, attribution, hotbar_tool, tool_name, cost_per_shot, \
                        candidates_json FROM weapon_shot_evidence",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )?)
        }))
        .unwrap();
    assert_eq!(row.0, kill);
    assert_eq!(row.1, "evidence");
    assert_eq!(row.2, "Pistol");
    assert_eq!(row.3, "Cannon");
    assert!(close(row.4, cannon()));
    let candidates: serde_json::Value = serde_json::from_str(&row.5).unwrap();
    assert_eq!(
        candidates,
        serde_json::json!([
            {"equipmentId": 1, "name": "Pistol", "fits": false},
            {"equipmentId": 2, "name": "Cannon", "fits": true},
            {"equipmentId": 3, "name": "Rifle", "fits": false},
        ])
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM weapon_shot_evidence WHERE session_id = ?",
            &[&session.id],
        ),
        1,
        "the agreeing pistol shot keeps no row"
    );
}

#[test]
fn confirming_reprices_the_regime_and_declares_the_evidence_weapon() {
    let rig = rig();
    let tracker = tracker_with(&rig, arsenal());
    let session = rig.wait(tracker.start_session()).unwrap();
    press(&rig, "Pistol");
    rig.bus.publish(&hit(6.0)); // only the pistol: proof it was in hand
    jam(&rig); // inherits the pistol, for now
    loot(&rig);
    jam(&rig);
    rig.clock.advance(2.0).unwrap();
    rig.bus.publish(&hit(30.0)); // only the cannon: the missed switch

    assert!(rig
        .wait(tracker.decide_weapon_mismatch(MismatchDecision::Confirm))
        .unwrap());

    let kill = kill_ids(&rig).pop().unwrap();
    assert_eq!(
        stored_phases(&rig, &kill),
        vec![
            ("Pistol".to_string(), 1, pistol()),
            ("Cannon".to_string(), 1, cannon()),
        ],
        "the settled jam moved to the cannon; the proven pistol hit did not"
    );
    assert!(close(
        rig.scalar_f64("SELECT cost_ped FROM kills WHERE id = ?", &[&kill]),
        pistol() + cannon()
    ));
    assert_eq!(
        pending(&rig, &tracker),
        vec![("Cannon".to_string(), 2, cannon())]
    );
    let (decision, repriced, delta): (String, i64, f64) = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT decision, repriced_shots, cost_delta_ped FROM weapon_attribution_reviews",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )?)
        }))
        .unwrap();
    assert_eq!(decision, "confirmed");
    assert_eq!(repriced, 2);
    assert!(close(delta, 2.0 * (cannon() - pistol())));

    let readout = rig.wait(tracker.snapshot()).unwrap();
    assert_eq!(readout.current_tool.as_deref(), Some("Cannon"));
    assert!(readout.active.unwrap().weapon_guardrail_mismatch.is_none());
    // The cannon is declared now: its hits simply agree.
    rig.bus.publish(&hit(30.0));
    assert!(rig
        .wait(tracker.snapshot())
        .unwrap()
        .active
        .unwrap()
        .weapon_guardrail_mismatch
        .is_none());

    rig.wait(tracker.stop_session()).unwrap();
    assert_eq!(
        rig.scalar_i64(
            "SELECT weapon_shots_evidenced FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        3,
        "the evidence shot and the two confirmed jams"
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT weapon_shots_agreed FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        2
    );
}

#[test]
fn keeping_reprices_the_evidence_back_and_marks_its_rows() {
    let rig = rig();
    let tracker = tracker_with(&rig, arsenal());
    let session = rig.wait(tracker.start_session()).unwrap();
    press(&rig, "Pistol");
    rig.clock.advance(2.0).unwrap();
    rig.bus.publish(&hit(30.0));
    loot(&rig);
    rig.bus.publish(&hit(31.0));

    assert!(rig
        .wait(tracker.decide_weapon_mismatch(MismatchDecision::Keep))
        .unwrap());

    let kill = kill_ids(&rig).pop().unwrap();
    assert_eq!(
        stored_phases(&rig, &kill),
        vec![("Pistol".to_string(), 1, pistol())]
    );
    assert!(close(
        rig.scalar_f64("SELECT cost_ped FROM kills WHERE id = ?", &[&kill]),
        pistol()
    ));
    assert_eq!(
        pending(&rig, &tracker),
        vec![("Pistol".to_string(), 1, pistol())]
    );
    // The settled row moved with its shot and names the decision.
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM weapon_shot_evidence e \
             JOIN weapon_attribution_reviews r ON r.id = e.review_id \
             WHERE e.tool_name = 'Pistol' AND r.decision = 'kept' AND e.session_id = ?",
            &[&session.id],
        ),
        1
    );

    // The cannon's band no longer overrides the pistol this regime.
    rig.bus.publish(&hit(30.0));
    let active = rig.wait(tracker.snapshot()).unwrap().active.unwrap();
    assert!(active.weapon_guardrail_mismatch.is_none());
    assert_eq!(
        pending(&rig, &tracker),
        vec![("Pistol".to_string(), 2, pistol())]
    );

    // The pending row lands with the stop, already repriced.
    rig.wait(tracker.stop_session()).unwrap();
    assert_eq!(
        rig.scalar_i64(
            "SELECT COUNT(*) FROM weapon_shot_evidence \
             WHERE kill_id IS NULL AND tool_name = 'Pistol' AND review_id IS NOT NULL \
               AND session_id = ?",
            &[&session.id],
        ),
        1
    );
    assert!(close(
        rig.scalar_f64(
            "SELECT dangling_cost FROM tracking_sessions WHERE id = ?",
            &[&session.id]
        ),
        2.0 * pistol()
    ));
}

#[test]
fn a_decision_needs_a_session_and_a_standing_mismatch() {
    let rig = rig();
    let tracker = tracker_with(&rig, arsenal());
    assert_eq!(
        rig.wait(tracker.decide_weapon_mismatch(MismatchDecision::Confirm)),
        Err(WeaponDecisionError::NoActiveSession)
    );
    rig.wait(tracker.start_session()).unwrap();
    press(&rig, "Pistol");
    rig.bus.publish(&hit(7.0));
    for decision in [MismatchDecision::Confirm, MismatchDecision::Keep] {
        assert_eq!(
            rig.wait(tracker.decide_weapon_mismatch(decision)),
            Ok(false)
        );
    }
    assert_eq!(
        rig.scalar_i64("SELECT COUNT(*) FROM weapon_attribution_reviews", &[]),
        0
    );
}

#[test]
fn a_decision_that_cannot_be_saved_changes_nothing() {
    let rig = rig();
    let tracker = tracker_with(&rig, arsenal());
    rig.wait(tracker.start_session()).unwrap();
    press(&rig, "Pistol");
    rig.bus.publish(&hit(6.0));
    jam(&rig);
    loot(&rig);
    rig.clock.advance(2.0).unwrap();
    rig.bus.publish(&hit(30.0));
    let kill = kill_ids(&rig).pop().unwrap();
    let phases_before = stored_phases(&rig, &kill);
    let pending_before = pending(&rig, &tracker);

    rig.wait(rig.db.with_writer(|conn| {
        conn.execute("DROP TABLE weapon_attribution_reviews", [])?;
        Ok(())
    }))
    .unwrap();
    assert_eq!(
        rig.wait(tracker.decide_weapon_mismatch(MismatchDecision::Confirm)),
        Err(WeaponDecisionError::NotSaved)
    );

    assert_eq!(stored_phases(&rig, &kill), phases_before);
    assert_eq!(pending(&rig, &tracker), pending_before);
    let readout = rig.wait(tracker.snapshot()).unwrap();
    assert_eq!(readout.current_tool.as_deref(), Some("Pistol"));
    let cue = readout.active.unwrap().weapon_guardrail_mismatch;
    assert_eq!(
        cue.map(|cue| cue.recording_tool),
        Some("Cannon".to_string())
    );
    rig.probe(&tracker, |actor| {
        let active = actor.session.active().unwrap();
        assert_eq!(active.weapons.attribution.declared(), Some("Pistol"));
        let kill = active.session.kills.last().unwrap();
        assert_eq!(
            kill.tool_stats.len(),
            1,
            "the settled jam stayed the pistol's"
        );
        assert_eq!(kill.tool_stats[0].1.shots_fired, 2);
    });
}

#[test]
fn unpriced_shots_settle_with_the_dangling_cost_and_the_session_keeps_its_tallies() {
    let rig = rig();
    let tracker = tracker_with(&rig, arsenal());
    let session = rig.wait(tracker.start_session()).unwrap();
    press(&rig, "Pistol");
    rig.bus.publish(&hit(7.0));
    // Beyond every carried weapon's reach, critical or not.
    rig.bus.publish(&hit(200.0));
    let active = rig.wait(tracker.snapshot()).unwrap().active.unwrap();
    assert_eq!(active.unpriced_shots, 1);
    assert!(active.weapon_guardrail_mismatch.is_none());

    let stopped = rig.wait(tracker.stop_session()).unwrap().unwrap();
    assert_eq!(stopped.dangling_cost, Ped(pistol()));
    let (kill_id, attribution, tool, cost, amount, hotbar): (
        Option<String>,
        String,
        Option<String>,
        f64,
        f64,
        String,
    ) = rig
        .wait(rig.db.with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT kill_id, attribution, tool_name, cost_per_shot, amount, hotbar_tool \
                 FROM weapon_shot_evidence",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )?)
        }))
        .unwrap();
    assert_eq!(kill_id, None);
    assert_eq!(attribution, "unresolved");
    assert_eq!(tool, None);
    assert_eq!(cost, 0.0);
    assert_eq!(amount, 200.0);
    assert_eq!(hotbar, "Pistol");
    assert_eq!(
        rig.scalar_i64(
            "SELECT weapon_shots_agreed FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        1
    );
    assert_eq!(
        rig.scalar_i64(
            "SELECT weapon_shots_evidenced FROM tracking_sessions WHERE id = ?",
            &[&session.id],
        ),
        0
    );
}

#[test]
fn the_previous_weapons_shot_after_a_switch_is_no_mismatch() {
    let rig = rig();
    let tracker = tracker_with(&rig, arsenal());
    rig.wait(tracker.start_session()).unwrap();
    press(&rig, "Cannon");
    rig.clock.advance(5.0).unwrap();
    press(&rig, "Pistol");
    rig.clock.advance(0.5).unwrap();
    rig.bus.publish(&hit(30.0));
    let active = rig.wait(tracker.snapshot()).unwrap().active.unwrap();
    assert!(active.weapon_guardrail_mismatch.is_none());
    assert_eq!(
        pending(&rig, &tracker),
        vec![("Cannon".to_string(), 1, cannon())],
        "the cannon's shot landing after the switch is the cannon's"
    );
    rig.clock.advance(2.0).unwrap();
    rig.bus.publish(&hit(30.0));
    let active = rig.wait(tracker.snapshot()).unwrap().active.unwrap();
    assert!(active.weapon_guardrail_mismatch.is_some());
}

#[test]
fn a_harvesting_press_resyncs_the_weapon_regime() {
    let rig = rig();
    let tracker = tracker_with(&rig, arsenal());
    rig.wait(tracker.start_session()).unwrap();
    press(&rig, "Pistol");
    rig.clock.advance(2.0).unwrap();
    rig.bus.publish(&hit(30.0));
    assert!(rig
        .wait(tracker.snapshot())
        .unwrap()
        .active
        .unwrap()
        .weapon_guardrail_mismatch
        .is_some());
    rig.bus.publish(&BusEvent::ActiveHarvestToolChanged(
        ActiveHarvestToolChangedPayload {
            tool_name: "Terratech PH-1 (L)".into(),
            cost_per_use_ped: 0.02,
            source: Some("hotbar:4".into()),
        },
    ));
    assert!(rig
        .wait(tracker.snapshot())
        .unwrap()
        .active
        .unwrap()
        .weapon_guardrail_mismatch
        .is_none());
    assert_eq!(
        rig.wait(tracker.decide_weapon_mismatch(MismatchDecision::Keep)),
        Ok(false)
    );
}

#[test]
fn a_mid_session_reload_adopts_the_new_carried_set_and_keeps_the_declared_weapon() {
    let rig = rig();
    let equipment = Arc::new(Carried(Mutex::new(vec![carried(1, "Pistol", 10.0, 0.05)])));
    let tracker = rig.tracker(Providers {
        equipment: equipment.clone(),
        ..Providers::default()
    });
    rig.wait(tracker.start_session()).unwrap();
    press(&rig, "Pistol");
    rig.clock.advance(2.0).unwrap();
    // Nothing carried explains 30 yet.
    rig.bus.publish(&hit(30.0));
    assert_eq!(
        rig.wait(tracker.snapshot())
            .unwrap()
            .active
            .unwrap()
            .unpriced_shots,
        1
    );
    *equipment.0.lock().unwrap() = arsenal();
    rig.wait(tracker.reload_config());
    rig.bus.publish(&hit(30.0));
    let active = rig.wait(tracker.snapshot()).unwrap().active.unwrap();
    assert_eq!(
        active
            .weapon_guardrail_mismatch
            .map(|cue| (cue.hotbar_tool, cue.recording_tool)),
        Some(("Pistol".to_string(), "Cannon".to_string()))
    );
}

mod conservation {
    use super::*;
    use proptest::prelude::*;

    #[derive(Debug, Clone)]
    enum Step {
        Hit(f64, bool),
        Jam,
        Press(usize),
        Loot,
        Wait,
        Confirm,
        Keep,
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            6 => (prop::sample::select(vec![3.0, 6.0, 9.0, 13.0, 18.0, 25.0, 30.0, 45.0, 200.0]), any::<bool>())
                .prop_map(|(amount, critical)| Step::Hit(amount, critical)),
            2 => Just(Step::Jam),
            1 => (0usize..3).prop_map(Step::Press),
            2 => Just(Step::Loot),
            2 => Just(Step::Wait),
            1 => Just(Step::Confirm),
            1 => Just(Step::Keep),
        ]
    }

    const NAMES: [&str; 3] = ["Pistol", "Cannon", "Rifle"];

    /// A kill as memory holds it: its id, weapon cost, and sorted phases.
    type KillState = (String, f64, Vec<(String, i64, f64)>);

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(12))]

        /// Whatever the player presses and decides, every shot is counted
        /// once, memory and the database hold the same phases and costs for
        /// every settled kill, every kill's cost is its phases' sum, and the
        /// shots left unpriced are exactly the stored unresolved rows no
        /// decision priced.
        #[test]
        fn decisions_conserve_shots_and_cost_between_memory_and_the_database(
            steps in proptest::collection::vec(step(), 1..40),
        ) {
            let rig = rig();
            let tracker = tracker_with(&rig, arsenal());
            let session = rig.wait(tracker.start_session()).unwrap();
            let mut shots = 0i64;
            for step in steps {
                match step {
                    Step::Hit(amount, critical) => {
                        let event = if critical {
                            BusEvent::Combat(CombatPayload::CriticalHit {
                                amount,
                                timestamp: "2026-01-01T00:00:01".into(),
                            })
                        } else {
                            hit(amount)
                        };
                        rig.bus.publish(&event);
                        shots += 1;
                    }
                    Step::Jam => {
                        jam(&rig);
                        shots += 1;
                    }
                    Step::Press(index) => press(&rig, NAMES[index]),
                    Step::Loot => loot(&rig),
                    Step::Wait => rig.clock.advance(2.0).unwrap(),
                    Step::Confirm => {
                        rig.wait(tracker.decide_weapon_mismatch(MismatchDecision::Confirm)).unwrap();
                    }
                    Step::Keep => {
                        rig.wait(tracker.decide_weapon_mismatch(MismatchDecision::Keep)).unwrap();
                    }
                }
            }

            let (memory_kills, pending_shots, pending_unpriced_rows, unpriced) =
                rig.probe(&tracker, |actor| {
                    let active = actor.session.active().unwrap();
                    let kills: Vec<KillState> = active
                        .session
                        .kills
                        .iter()
                        .map(|kill| {
                            let mut phases: Vec<(String, i64, f64)> = kill
                                .tool_stats
                                .iter()
                                .map(|(_, s)| (s.tool_name.clone(), s.shots_fired, s.cost_per_shot.value()))
                                .collect();
                            phases.sort_by(|a, b| a.partial_cmp(b).unwrap());
                            (kill.id.clone(), kill.cost_ped.value(), phases)
                        })
                        .collect();
                    let pending: i64 = active
                        .accumulator
                        .tool_stats
                        .iter()
                        .map(|(_, s)| s.shots_fired)
                        .sum();
                    let pending_unpriced = active
                        .accumulator
                        .evidence
                        .iter()
                        .filter(|row| row.tool_name.is_none())
                        .count() as i64;
                    (kills, pending, pending_unpriced, active.weapons.attribution.counts().unresolved)
                });

            let mut settled = 0i64;
            for (kill_id, cost, phases) in &memory_kills {
                let mut stored = stored_phases(&rig, kill_id);
                stored.sort_by(|a, b| a.partial_cmp(b).unwrap());
                prop_assert_eq!(&stored, phases);
                let stored_cost = rig.scalar_f64("SELECT cost_ped FROM kills WHERE id = ?", &[kill_id]);
                prop_assert!(close(stored_cost, *cost));
                let phase_sum: f64 = phases.iter().map(|(_, n, c)| *n as f64 * c).sum();
                prop_assert!(close(phase_sum, *cost));
                settled += phases.iter().map(|(_, n, _)| n).sum::<i64>();
            }
            prop_assert_eq!(settled + pending_shots, shots);

            let settled_unpriced = rig.scalar_i64(
                "SELECT COUNT(*) FROM weapon_shot_evidence \
                 WHERE session_id = ? AND attribution = 'unresolved' AND tool_name IS NULL",
                &[&session.id],
            );
            prop_assert_eq!(settled_unpriced + pending_unpriced_rows, unpriced);
        }
    }
}
