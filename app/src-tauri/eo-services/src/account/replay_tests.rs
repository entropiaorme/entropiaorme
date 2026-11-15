//! The recorded hunt: fifteen readings a minute apart across one tracked
//! session, replayed through the diff, the patterns, and the measure. Every
//! figure here was checked by hand against the raw recording.

use std::collections::BTreeMap;
use std::path::Path;

use super::amount::{Amount, Pec, Precision};
use super::composite::{recognise, OpenedBoxes, RepairCost, Settlement};
use super::diff::diff;
use super::measure::{
    measure_session, AmmoSplit, CoverageLevel, Edge, GainMark, NotableEvent, SessionWindow,
};
use super::reading::{Container, HoldingKey};
use super::roles::{KnownRoles, Role, RoleBook, Stacking};
use super::test_support::recorded_hunt;
use crate::game_data_store::GameDataStore;

/// The player's own equipment and protection, as the hunt was played.
fn roles() -> KnownRoles {
    let snapshot =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../entropia-orme/resources/snapshot");
    let game_data = GameDataStore::new(&snapshot).expect("the bundled game data loads");
    let library = vec![
        (
            "Exceptional Mayhem Electric Attack Chip 15, Perfected (L)".to_string(),
            "weapon".to_string(),
            r#"{"weapon_entity": {"name": "Exceptional Mayhem Electric Attack Chip 15, Perfected (L)"},
                "amp_entity": {"name": "Mayhem MF-Amplifier Delta (L)"},
                "implant_entity": {"name": "NeoPsion 85-B Mindforce Implant (L)"}}"#
                .to_string(),
        ),
        (
            "Frontier First Aid Pack, Adjusted".to_string(),
            "healing".to_string(),
            "{}".to_string(),
        ),
    ];
    let sets = vec![("armour".to_string(), "Pegasus".to_string())];
    KnownRoles::from_parts(&library, &sets, Some(&game_data))
}

/// The session ran from just after the reading at 1794754637 to just
/// before the final reading.
const SESSION: SessionWindow = SessionWindow {
    started_at: 1_794_754_640.0,
    ended_at: Some(1_794_755_350.0),
};

const PED: i64 = 1_000_000;
const PEC: i64 = 10_000;

fn carried(name: &str) -> HoldingKey {
    HoldingKey {
        name: name.to_string(),
        container: Container::Carried,
    }
}

#[test]
fn the_recording_reads_whole() {
    let readings = recorded_hunt();
    assert_eq!(readings.len(), 15);
    assert_eq!(readings[2].taken_at(), 1_794_754_637.0);
    assert_eq!(readings[0].balance(), Pec(25_000));
    assert_eq!(readings[14].balance(), Pec(24_952));
    let hub = HoldingKey {
        name: "Common Dung".into(),
        container: Container::Storage {
            planet: "The Hub".into(),
        },
    };
    assert!(readings[0].holding(&hub).is_some());
}

#[test]
fn the_opening_before_the_session_is_five_boxes_settled_within_the_truncation() {
    let readings = recorded_hunt();
    let book = RoleBook::resolve(&roles(), &readings);
    let set = diff(&readings[1], &readings[2], &|name| book.stacking(name)).unwrap();
    assert_eq!(set.span.from, 1_794_754_577.0);
    let opening = recognise(&set, &readings[1], &readings[2], &[])
        .unwrap()
        .opening
        .expect("an opening");
    assert_eq!(
        opening.boxes,
        vec![OpenedBoxes {
            name: "Halloween Strongbox".into(),
            count: 5
        }]
    );
    assert_eq!(opening.keys_used, 5);
    assert_eq!(opening.keys, Amount::whole_pec(Pec(5_000)).unwrap());
    assert_eq!(opening.ammo_units, 499_500);
    assert_eq!(opening.ammo_rows, 5);
    assert_eq!(opening.ammo_from_rows, Amount::exact(49_950_000));
    assert_eq!(opening.ammo_from_keys.micro_ped(), 50 * PED);
    // The fireworks have no TT: the balance closes with them and without
    // them, so they are only possibly the opening's.
    assert!(opening.contents.is_empty());
    assert_eq!(opening.possible_contents.len(), 1);
    assert_eq!(
        opening.possible_contents[0].key,
        carried("Halloween Fireworks")
    );
    assert_eq!(opening.possible_contents[0].amount.micro_ped(), 0);
    assert_eq!(opening.settlement, Settlement::Settled);
}

#[test]
fn the_hunt_measures_as_a_human_read_it() {
    let readings = recorded_hunt();
    let outcome = measure_session(&SESSION, &readings, &roles()).unwrap();
    let measure = outcome.measure;
    assert_eq!(outcome.used, 2..15);

    assert_eq!(measure.coverage.level, CoverageLevel::Complete);
    assert_eq!(measure.coverage.opening, Edge::Read);
    assert_eq!(measure.coverage.closing, Edge::Read);
    assert_eq!(measure.coverage.reading_count, 13);
    assert_eq!(measure.coverage.gap_count, 0);

    // Ammo: 660288 units burned, exactly 66.0288 PED, beside the 600000
    // units an opening of seven paid back.
    assert_eq!(measure.ammo.len(), 1);
    let ammo = &measure.ammo[0];
    assert_eq!(ammo.name, "Universal Ammo");
    assert_eq!(ammo.net_units, -60_288);
    assert_eq!(
        ammo.split,
        AmmoSplit::Settled {
            burned_units: 660_288,
            burned: Amount::exact(66_028_800),
            strongbox_units: 600_000,
            strongbox: Amount::exact(60 * PED),
            converted_units: 0,
            converted: Amount::NONE,
        }
    );

    let openings: Vec<_> = measure
        .events
        .iter()
        .filter_map(|event| match event {
            NotableEvent::StrongboxOpening { opening, .. } => Some(opening),
            _ => None,
        })
        .collect();
    assert_eq!(openings.len(), 1);
    let opening = openings[0];
    assert_eq!(opening.keys_used, 7);
    assert_eq!(opening.keys.micro_ped(), 70 * PED);
    assert_eq!(opening.contents.len(), 1);
    assert_eq!(opening.contents[0].key, carried("Skeleton Pants Red (F)"));
    assert_eq!(opening.contents[0].amount.micro_ped(), 10 * PED);
    assert_eq!(opening.ammo_from_rows, Amount::exact(60 * PED));
    assert_eq!(opening.ammo_from_keys.micro_ped(), 60 * PED);
    assert_eq!(opening.settlement, Settlement::Settled);
    // Besides the opening, only the Mayhem Token arriving as an item: no
    // move, no sale or purchase, and no unexplained Card movement.
    assert_eq!(measure.events.len(), 2, "{:#?}", measure.events);
    assert!(matches!(
        &measure.events[0],
        NotableEvent::Appeared { key, units: 1, .. } if key.name == "Mayhem Token"
    ));

    // What arrived: the figure tracked loot compares with.
    let gained: Vec<(&str, Stacking, GainMark, i64, Amount)> = measure
        .gained
        .iter()
        .map(|item| {
            (
                item.key.name.as_str(),
                item.stacking,
                item.mark,
                item.units,
                item.amount,
            )
        })
        .collect();
    let arrived = GainMark::Arrived;
    assert_eq!(
        gained,
        vec![
            (
                "Mayhem Token",
                Stacking::Single,
                arrived,
                1,
                Amount::whole_pec(Pec(0)).unwrap()
            ),
            (
                "Output Amplifier Component",
                Stacking::Stack,
                arrived,
                4,
                Amount::whole_pec(Pec(160)).unwrap()
            ),
            (
                "Shrapnel",
                Stacking::Stack,
                arrived,
                363_184,
                Amount::exact(36_318_400)
            ),
        ]
    );
    assert_eq!(measure.gained_possibly_strongbox.micro_ped(), 0);
    assert_eq!(measure.gained_total.micro_ped(), 36_318_400 + 1_600_000);

    let consumed: Vec<(&str, i64, Amount)> = measure
        .other_consumption
        .iter()
        .map(|item| (item.key.name.as_str(), item.units, item.amount))
        .collect();
    assert_eq!(consumed, vec![("Mind Essence", 900, Amount::exact(90_000))]);

    // Wear per item, never netted against the repair.
    let decay: BTreeMap<(String, Container), (Role, i64)> = measure
        .decay
        .iter()
        .map(|item| {
            assert_eq!(item.decay.precision(), Precision::WholePec);
            (
                (item.key.name.clone(), item.key.container.clone()),
                (item.role, item.decay.micro_ped() / PEC),
            )
        })
        .collect();
    let worn = |name: &str| decay[&(name.to_string(), Container::Carried)];
    let plate = |plate: &str, parent: &str| {
        decay[&(
            plate.to_string(),
            Container::Attached {
                parent: parent.to_string(),
            },
        )]
    };
    assert_eq!(
        worn("Frontier First Aid Pack, Adjusted"),
        (Role::Healing, 30)
    );
    let paneleon = [
        ("Arm Guards", 7),
        ("Chest Guard", 11),
        ("Foot Guards", 10),
        ("Head Guard", 3),
        ("Thigh Guards", 5),
    ];
    let mut paneleon_plates = 0;
    for (piece, pec) in paneleon {
        let name = format!("Paneleon Spec Imperium {piece} (M)");
        assert_eq!(worn(&name), (Role::Armour, pec), "{name}");
        let (role, plate_pec) = plate("Armor Plating Mk. 6A", &name);
        assert_eq!(role, Role::Plating);
        paneleon_plates += plate_pec;
    }
    assert_eq!(paneleon_plates, 12);
    let mut pegasus = 0;
    let mut pegasus_plates = 0;
    for piece in [
        "Arm Guards",
        "Foot Guards",
        "Gloves",
        "Helmet",
        "Shin Guards",
        "Thigh Guards",
    ] {
        let name = format!("Pegasus {piece} (M,L)");
        let (role, pec) = worn(&name);
        assert_eq!(role, Role::Armour, "{name}");
        pegasus += pec;
        let (role, plate_pec) = plate("Armor Plating Mk. 6B", &name);
        assert_eq!(role, Role::Plating);
        pegasus_plates += plate_pec;
    }
    assert_eq!(pegasus, 32);
    assert_eq!(pegasus_plates, 9);
    let chip = "Exceptional Mayhem Electric Attack Chip 15, Perfected (L)";
    assert_eq!(worn(chip), (Role::Weapon, 1));
    assert_eq!(
        plate("Mayhem MF-Amplifier Delta (L)", chip),
        (Role::Amplifier, 3)
    );
    assert_eq!(worn("Teleportation Chip 1"), (Role::OtherEquipment, 2));
    assert_eq!(
        worn("NeoPsion 85-B Mindforce Implant (L)"),
        (Role::OtherEquipment, 1)
    );
    assert_eq!(decay.len(), 27, "no other item wore");

    // One repair: sixteen items restored 57 PEC, and the Card's 48 PEC fall
    // is its exact cost.
    assert_eq!(measure.repairs.len(), 1);
    let repair = &measure.repairs[0];
    assert_eq!(repair.between.from, 1_794_755_123.0);
    assert_eq!(repair.repair.items.len(), 16);
    assert_eq!(repair.repair.restored, Amount::whole_pec(Pec(57)).unwrap());
    assert_eq!(repair.repair.cost, RepairCost::Paid { pec: Pec(48) });
}

#[test]
fn the_measure_survives_its_cache_unchanged() {
    let readings = recorded_hunt();
    let measure = measure_session(&SESSION, &readings, &roles())
        .unwrap()
        .measure;
    let json = serde_json::to_string(&measure).expect("serialises");
    let back: super::measure::SessionMeasure = serde_json::from_str(&json).expect("decodes");
    assert_eq!(back, measure);
}

#[test]
fn without_a_reading_close_to_either_end_the_measure_is_partial() {
    let readings = recorded_hunt();
    // No reading before the start at all.
    let late = &readings[3..];
    let outcome = measure_session(&SESSION, late, &roles()).unwrap();
    assert_eq!(outcome.measure.coverage.level, CoverageLevel::Partial);
    assert_eq!(outcome.measure.coverage.opening, Edge::Unread);
    assert_eq!(outcome.used, 0..12);

    // A reading before the start, but more than 90 seconds before it: not
    // used, so nothing from before the session is counted.
    let mut thinned = recorded_hunt();
    thinned.drain(1..3);
    let outcome = measure_session(&SESSION, &thinned, &roles()).unwrap();
    assert_eq!(outcome.measure.coverage.opening, Edge::Unread);
    assert_eq!(
        outcome.used,
        1..13,
        "the reading taken 124 seconds before the start is not used"
    );
    assert!(outcome.measure.events.iter().all(|event| match event {
        NotableEvent::StrongboxOpening { opening, .. } => opening.keys_used == 7,
        _ => true,
    }));

    // Still running: no closing reading.
    let running = SessionWindow {
        started_at: SESSION.started_at,
        ended_at: None,
    };
    let outcome = measure_session(&running, &readings, &roles()).unwrap();
    assert_eq!(outcome.measure.coverage.closing, Edge::Unread);
    assert_eq!(outcome.measure.coverage.level, CoverageLevel::Partial);
    assert_eq!(outcome.used, 2..15);

    // The first reading after the end taken more than 90 seconds after it.
    let mut gapped = recorded_hunt();
    gapped.remove(13);
    let early_end = SessionWindow {
        started_at: SESSION.started_at,
        ended_at: Some(1_794_755_250.0),
    };
    let outcome = measure_session(&early_end, &gapped, &roles()).unwrap();
    assert_eq!(outcome.measure.coverage.closing, Edge::Unread);
    assert_eq!(outcome.used, 2..13);
}

#[test]
fn a_gap_between_readings_is_counted_and_never_filled() {
    let mut readings = recorded_hunt();
    // Lose four readings from the middle of the hunt: one interval of five
    // minutes remains, measured as one step.
    readings.drain(6..10);
    let outcome = measure_session(&SESSION, &readings, &roles()).unwrap();
    let coverage = outcome.measure.coverage;
    assert_eq!(coverage.level, CoverageLevel::Partial);
    assert_eq!(coverage.gap_count, 1);
    assert_eq!(coverage.reading_count, 9);
    // What changed across the gap is still measured, and the ammo totals
    // hold.
    let ammo = &outcome.measure.ammo[0];
    assert!(matches!(
        ammo.split,
        AmmoSplit::Settled {
            burned_units: 660_288,
            strongbox_units: 600_000,
            ..
        }
    ));
    // The opening now shares its step with five minutes of loot. Its keys'
    // balance claims the trousers only: the shrapnel and the component stay
    // loot, and the token, which has no TT to place it, stays gained marked
    // as possibly the opening's. Nothing that arrived is lost.
    let opening = outcome
        .measure
        .events
        .iter()
        .find_map(|event| match event {
            NotableEvent::StrongboxOpening { opening, .. } => Some(opening),
            _ => None,
        })
        .expect("the opening is still reported");
    assert_eq!(opening.settlement, Settlement::Settled);
    assert_eq!(opening.contents.len(), 1);
    assert_eq!(opening.contents[0].key, carried("Skeleton Pants Red (F)"));
    let gained: Vec<(&str, GainMark, i64)> = outcome
        .measure
        .gained
        .iter()
        .map(|gain| (gain.key.name.as_str(), gain.mark, gain.units))
        .collect();
    assert_eq!(
        gained,
        vec![
            ("Mayhem Token", GainMark::PossiblyStrongbox, 1),
            ("Output Amplifier Component", GainMark::Arrived, 4),
            ("Shrapnel", GainMark::Arrived, 363_184),
        ]
    );
    assert_eq!(outcome.measure.gained_possibly_strongbox.micro_ped(), 0);
}
