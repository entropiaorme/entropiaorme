//! A session's measured costs: the buckets and lines a measure gives, the
//! window they are read over, and the projection stored beside the seal.

use super::amount::{Amount, MICRO_PER_PEC};
use super::costs::{
    heal_session_costs, session_costs, stored_costs, ArmourPart, Basis, Charge, CostBucket,
    CostLine, LineKind, LineReason, SessionCosts, Status, TopUp, Unmeasured, COSTS_VERSION,
};
use super::reading::Container;
use super::roles::{CostCategory, KnownRoles};
use super::sessions::{cache_sealed, measure_to_seal};
use super::store::{record_reading, Purpose};
use super::test_support::reading;
use crate::db::migrated_in_memory;

const C: Container = Container::Carried;
const PED: i64 = 1_000_000;
const PEC: i64 = MICRO_PER_PEC;

fn on(parent: &str) -> Container {
    Container::Attached {
        parent: parent.to_string(),
    }
}

fn session_row(conn: &rusqlite::Connection, id: &str, started: f64, ended: Option<f64>) {
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
         VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![id, started, ended, i64::from(ended.is_none())],
    )
    .unwrap();
}

/// The player's weapon with its amplifier, extender and implant, a first
/// aid pack, and two armour sets: one unlimited, one limited.
fn the_player_s_gear(conn: &rusqlite::Connection) {
    conn.execute(
        "INSERT INTO equipment_library (name, item_type, properties_json) VALUES \
         ('Opalo', 'weapon', '{\"weapon_entity\": {\"name\": \"Opalo\"}, \
           \"amp_entity\": {\"name\": \"Odd Amp (L)\"}, \
           \"absorber_entity\": {\"name\": \"ArMatrix Extender P10 (L)\"}, \
           \"implant_entity\": {\"name\": \"NeoPsion 85-B Mindforce Implant (L)\"}}'), \
         ('Frontier First Aid Pack, Adjusted', 'healing', '{}')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO protection_sets (kind, name, economy_kind, markup_percent, created_at) \
         VALUES ('armour', 'Frontier', 'unlimited', NULL, 1), \
                ('armour', 'Pegasus', 'limited', 120, 1)",
        [],
    )
    .unwrap();
}

/// The session the hunt was tracked over: its readings are a minute apart
/// from 990, the first taken before it started and the eleventh at its
/// stop.
const STARTED: f64 = 1_000.0;
const STOPPED: f64 = 1_590.0;

/// One value per reading, for twelve readings.
type Series = [i64; 12];

fn falling_by(start: i64, per_step: i64) -> Series {
    std::array::from_fn(|k| start - per_step * k as i64)
}

/// `start`, becoming each value given from the reading at its position.
fn from(start: i64, changes: &[(usize, i64)]) -> Series {
    std::array::from_fn(|k| {
        changes
            .iter()
            .rev()
            .find(|(at, _)| *at <= k)
            .map_or(start, |(_, value)| *value)
    })
}

/// A hunt read whole, a minute at a time: ammo burned, with Shrapnel
/// turned into ammo half way; the weapon, its amplifier, extender, a
/// sight and the implant wearing; the first aid pack wearing; unlimited
/// armour and its plate falling 1.13 in all and three repairs restoring
/// exactly that; a teleport with its Mind Essence; the portable repair and
/// TT units wearing; a hoverpod wearing; a material traded away; and, when
/// `limited`, a limited helmet losing 0.33. A twelfth reading, a second
/// closing one after the stop, catches up on a last PEC of the weapon's
/// wear that the site had not shown yet.
fn hunt(limited: bool) -> Vec<super::Reading> {
    let ammo_main: Series = falling_by(1_000_000, 10_000);
    let opalo = falling_by(5_000, 1);
    let amp = falling_by(2_000, 2);
    let extender = from(1_000, &[(1, 999), (2, 998), (3, 997)]);
    let sight = from(300, &[(8, 299)]);
    let implant = from(1_420, &[(5, 1_419), (9, 1_418)]);
    let fap = from(3_000, &[(2, 2_997), (7, 2_994)]);
    let harness: Series = [
        4_000, 3_980, 4_000, 3_970, 3_950, 4_000, 3_990, 3_990, 3_990, 3_990, 4_000, 3_995,
    ];
    let plate: Series = [
        2_500, 2_490, 2_490, 2_480, 2_480, 2_500, 2_490, 2_487, 2_487, 2_487, 2_500, 2_500,
    ];
    let helmet: Series = [
        1_000, 1_000, 1_000, 989, 989, 989, 978, 978, 967, 967, 967, 960,
    ];
    let chip = from(8_665, &[(4, 8_663)]);
    let repair_unit = from(500, &[(3, 490)]);
    let tt_unit = from(300, &[(8, 292)]);
    let hoverpod = from(1_200, &[(5, 1_195)]);
    // The PED Card pays each repair, exactly what it restored.
    let balance = from(100_000, &[(2, 99_980), (5, 99_910), (10, 99_887)]);
    (0..12)
        .map(|k| {
            // The last reading changes nothing but the weapon's wear.
            let j = k.min(10);
            let mut rows: Vec<(&str, Container, i64, i64)> = vec![
                ("Opalo", C, 1, opalo[k]),
                ("Odd Amp (L)", on("Opalo"), 1, amp[j]),
                ("ArMatrix Extender P10 (L)", on("Opalo"), 1, extender[j]),
                ("Odd Sight", on("Opalo"), 1, sight[j]),
                ("NeoPsion 85-B Mindforce Implant (L)", C, 1, implant[j]),
                ("Universal Ammo", C, ammo_main[j], ammo_main[j] / 100),
                ("Frontier First Aid Pack, Adjusted", C, 1, fap[j]),
                ("Frontier Harness, Adjusted (M)", C, 1, harness[j]),
                (
                    "Armor Plating Mk. 6A",
                    on("Frontier Harness, Adjusted (M)"),
                    1,
                    plate[j],
                ),
                ("Teleportation Chip 1", C, 1, chip[j]),
                ("Portable Repair Unit (L)", C, 1, repair_unit[j]),
                ("Portable TT Unit (L)", C, 1, tt_unit[j]),
                ("Exploration Hoverpod MkI (L)", C, 1, hoverpod[j]),
            ];
            if k < 7 {
                rows.push(("Shrapnel", C, 20_000, 200));
            } else {
                rows.push(("Universal Ammo", C, 20_200, 202));
            }
            if k < 4 {
                rows.push(("Mind Essence", C, 2_000, 20));
            } else {
                rows.push(("Mind Essence", C, 1_100, 11));
            }
            if k < 6 {
                rows.push(("Animal Oil Residue", C, 5_000, 50));
            }
            if limited {
                rows.push(("Pegasus Helmet (M,L)", C, 1, helmet[j]));
            }
            reading(990.0 + 60.0 * k as f64, balance[j], &rows)
        })
        .collect()
}

/// The hunt stored as the connection stores it: the reading before the
/// session untagged, the rest tagged with it.
fn store_the_hunt(conn: &mut rusqlite::Connection, id: &str, limited: bool) {
    the_player_s_gear(conn);
    session_row(conn, id, STARTED, Some(STOPPED));
    for (index, reading) in hunt(limited).iter().enumerate() {
        let tag = (index >= 1).then_some(id);
        record_reading(conn, reading, Purpose::Session, tag).unwrap();
    }
}

fn costs_of(conn: &mut rusqlite::Connection, id: &str) -> SessionCosts {
    session_costs(conn, id, None, 5_000.0, None)
        .unwrap()
        .expect("a known session")
}

fn basis(costs: &SessionCosts, bucket: CostBucket) -> Basis {
    costs.bucket(bucket).expect("every bucket").basis
}

fn measured(costs: &SessionCosts, bucket: CostBucket) -> i64 {
    match basis(costs, bucket) {
        Basis::Measured(amount) => amount.micro_ped(),
        other => panic!("{bucket:?} not measured whole: {other:?}"),
    }
}

fn line<'a>(costs: &'a SessionCosts, item: &str) -> &'a CostLine {
    costs
        .lines
        .iter()
        .find(|line| line.item == item)
        .unwrap_or_else(|| panic!("a line for {item}: {:?}", costs.lines))
}

fn rows(conn: &rusqlite::Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
        row.get(0)
    })
    .unwrap()
}

fn stored_rows(conn: &rusqlite::Connection) -> (i64, i64, i64) {
    (
        rows(conn, "account_session_costs"),
        rows(conn, "account_session_cost_buckets"),
        rows(conn, "account_session_cost_lines"),
    )
}

#[test]
fn a_hunt_read_whole_costs_what_each_item_lost_in_its_bucket() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", true);
    let costs = costs_of(&mut conn, "hunt");
    assert_eq!(costs.status, Status::Measured);

    // Ammo burned, and the weapon, its amplifier, extender, sight and the
    // implant wearing, each its own line, the weapon's last PEC read from
    // the second closing reading. Shrapnel turned into ammo is no cost.
    let ammo = line(&costs, "Universal Ammo");
    assert_eq!(
        (ammo.bucket, ammo.category, ammo.kind),
        (CostBucket::Weapon, CostCategory::Ammo, LineKind::AmmoBurned)
    );
    assert_eq!(ammo.units, Some(100_000));
    assert_eq!(ammo.amount().unwrap().micro_ped(), 10 * PED);
    for (item, category, lost) in [
        ("Opalo", CostCategory::Weapon, 11),
        ("Odd Amp (L)", CostCategory::Amplifier, 20),
        ("ArMatrix Extender P10 (L)", CostCategory::Attachment, 3),
        ("Odd Sight", CostCategory::Attachment, 1),
        (
            "NeoPsion 85-B Mindforce Implant (L)",
            CostCategory::MindforceImplant,
            2,
        ),
    ] {
        let line = line(&costs, item);
        assert_eq!(
            (line.bucket, line.category, line.kind, line.units),
            (CostBucket::Weapon, category, LineKind::Decay, None),
            "{item}"
        );
        assert_eq!(line.amount().unwrap().micro_ped(), lost * PEC, "{item}");
    }
    assert!(costs.lines.iter().all(|line| line.item != "Shrapnel"));
    assert_eq!(measured(&costs, CostBucket::Weapon), 10 * PED + 37 * PEC);

    // The first aid pack's wear is healing.
    let fap = line(&costs, "Frontier First Aid Pack, Adjusted");
    assert_eq!(
        (fap.bucket, fap.category),
        (CostBucket::Healing, CostCategory::HealingTool)
    );
    assert_eq!(measured(&costs, CostBucket::Healing), 6 * PEC);

    // The unlimited armour and its plate fell 1.13 in all, and three
    // repairs restoring 1.13 take nothing off it. The limited helmet is
    // listed with the 0.33 it lost, unpriced, so the armour bucket waits
    // for its set.
    let harness = line(&costs, "Frontier Harness, Adjusted (M)");
    let plate = line(&costs, "Armor Plating Mk. 6A");
    assert_eq!(
        (harness.category, harness.kind),
        (CostCategory::UnlimitedArmour, LineKind::Decay)
    );
    assert_eq!(
        (plate.category, plate.kind),
        (CostCategory::UnlimitedPlate, LineKind::Decay)
    );
    assert_eq!(
        harness.amount().unwrap().micro_ped() + plate.amount().unwrap().micro_ped(),
        113 * PEC
    );
    let helmet = line(&costs, "Pegasus Helmet (M,L)");
    assert_eq!(
        (helmet.bucket, helmet.category, helmet.kind),
        (
            CostBucket::Armour,
            CostCategory::LimitedArmour,
            LineKind::LimitedDecay
        )
    );
    assert_eq!(helmet.amount().unwrap().micro_ped(), 33 * PEC);
    assert_eq!(
        basis(&costs, CostBucket::Armour),
        Basis::Unmeasured(Unmeasured::LimitedSetUnconfirmed)
    );
    // The unlimited part stays measured; the helmet's loss waits unpriced.
    assert_eq!(
        costs.armour,
        [
            ArmourPart::Unlimited {
                lost: Amount::WholePec {
                    micro_ped: 113 * PEC
                }
            },
            ArmourPart::Unconfirmed {
                lost: Amount::WholePec {
                    micro_ped: 33 * PEC
                }
            },
        ]
    );

    // The teleport: the chip's wear and the 900 Mind Essence it burned,
    // 0.11 in all, as travel; the portable units 0.18 as field logistics;
    // the hoverpod, which nothing names, unclassified. None is Cycled.
    let chip = line(&costs, "Teleportation Chip 1");
    let essence = line(&costs, "Mind Essence");
    assert_eq!(
        (chip.bucket, chip.category),
        (CostBucket::Travel, CostCategory::TeleportChip)
    );
    assert_eq!(
        (
            essence.bucket,
            essence.category,
            essence.kind,
            essence.units
        ),
        (
            CostBucket::Travel,
            CostCategory::MindEssence,
            LineKind::Consumed,
            Some(900)
        )
    );
    assert_eq!(measured(&costs, CostBucket::Travel), 11 * PEC);
    assert_eq!(measured(&costs, CostBucket::FieldLogistics), 18 * PEC);
    assert_eq!(measured(&costs, CostBucket::Unclassified), 5 * PEC);
    for bucket in [
        CostBucket::Travel,
        CostBucket::FieldLogistics,
        CostBucket::Unclassified,
    ] {
        assert!(!bucket.is_cycled(), "{bucket:?}");
    }

    // The material traded away is no cost anywhere: it is listed, charged
    // nothing, for the player to place. The enhancer is the chat log's;
    // nothing harvested or dosed is a measured nothing.
    let oil = line(&costs, "Animal Oil Residue");
    assert_eq!(
        (oil.bucket, oil.category, oil.kind, oil.units, oil.charge),
        (
            CostBucket::Unclassified,
            CostCategory::Unclassified,
            LineKind::Consumed,
            Some(5_000),
            Charge::Nothing(LineReason::NotPlaced)
        )
    );
    assert_eq!(
        basis(&costs, CostBucket::Enhancer),
        Basis::Unmeasured(Unmeasured::TrackedOnly)
    );
    assert_eq!(measured(&costs, CostBucket::Harvesting), 0);
    assert_eq!(measured(&costs, CostBucket::Consumables), 0);
}

#[test]
fn unlimited_armour_alone_is_measured_at_what_it_fell() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", false);
    let costs = costs_of(&mut conn, "hunt");
    assert_eq!(measured(&costs, CostBucket::Armour), 113 * PEC);
}

#[test]
fn the_lines_come_in_a_stable_order_and_every_bucket_once() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", true);
    let costs = costs_of(&mut conn, "hunt");
    let order: Vec<CostBucket> = costs.buckets.iter().map(|cost| cost.bucket).collect();
    assert_eq!(order, CostBucket::ALL.to_vec());
    let keys: Vec<_> = costs
        .lines
        .iter()
        .map(|line| (line.bucket, line.kind, line.item.clone()))
        .collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted);
    let weapon: Vec<&str> = costs
        .lines
        .iter()
        .filter(|line| line.bucket == CostBucket::Weapon)
        .map(|line| line.item.as_str())
        .collect();
    assert_eq!(
        weapon,
        [
            "Universal Ammo",
            "ArMatrix Extender P10 (L)",
            "NeoPsion 85-B Mindforce Implant (L)",
            "Odd Amp (L)",
            "Odd Sight",
            "Opalo",
        ]
    );
}

#[test]
fn the_seal_stores_the_costs_in_the_same_write() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", true);
    let seal = measure_to_seal(&conn, "hunt", None, None, 2_000.0)
        .unwrap()
        .expect("an ended hunt with readings");
    assert_eq!(cache_sealed(&mut conn, &[seal], 2_000.0).unwrap(), 1);
    let roles = KnownRoles::read(&conn, None).unwrap();
    let stored = stored_costs(&conn, "hunt", roles.fingerprint())
        .unwrap()
        .expect("stored at seal");
    assert_eq!(stored.status, Status::Measured);
    assert_eq!(measured(&stored, CostBucket::Weapon), 10 * PED + 37 * PEC);
    let header: (i64, i64, String, Option<i64>, Option<i64>, f64) = conn
        .query_row(
            "SELECT costs_version, measure_version, status, first_reading_id, \
                    last_reading_id, measure_computed_at FROM account_session_costs \
             WHERE session_id = 'hunt'",
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
        )
        .unwrap();
    // The window runs from the reading before the start through the
    // closing readings: the twelfth stored is the last.
    assert_eq!(
        header,
        (
            COSTS_VERSION,
            super::measure::MEASURE_VERSION,
            "measured".to_string(),
            Some(1),
            Some(12),
            2_000.0
        )
    );
    // A read serves what the seal stored.
    assert_eq!(costs_of(&mut conn, "hunt"), stored);
}

#[test]
fn a_material_leaving_mid_session_costs_nothing_and_a_consumable_does() {
    let mut conn = migrated_in_memory().unwrap();
    session_row(&conn, "trade", 0.0, Some(120.0));
    conn.execute(
        "INSERT INTO equipment_library (name, item_type, properties_json) \
         VALUES ('Odd Pill', 'consumable', \
                 '{\"dose\": {\"markup_percent\": 100.0, \"track_cost\": true}}')",
        [],
    )
    .unwrap();
    let readings = [
        reading(
            0.0,
            1_000,
            &[
                ("Animal Oil Residue", C, 4_000, 40),
                ("Odd Pill", C, 10, 50),
            ],
        ),
        reading(60.0, 1_000, &[("Odd Pill", C, 10, 50)]),
        reading(120.0, 1_000, &[("Odd Pill", C, 8, 40)]),
    ];
    for reading in &readings {
        record_reading(&mut conn, reading, Purpose::Session, Some("trade")).unwrap();
    }
    let costs = costs_of(&mut conn, "trade");
    assert_eq!(costs.status, Status::Measured);
    assert_eq!(costs.lines.len(), 2, "{:?}", costs.lines);
    // The oil that left is listed for the player to place, and costs
    // nothing until they place it as used up.
    assert_eq!(
        line(&costs, "Animal Oil Residue").charge,
        Charge::Nothing(LineReason::NotPlaced)
    );
    let pill = line(&costs, "Odd Pill");
    assert_eq!(
        (pill.bucket, pill.category, pill.kind, pill.units),
        (
            CostBucket::Consumables,
            CostCategory::Consumable,
            LineKind::Consumed,
            Some(2)
        )
    );
    assert_eq!(measured(&costs, CostBucket::Consumables), 10 * PEC);
    assert_eq!(measured(&costs, CostBucket::Unclassified), 0);
}

/// A minute of armour wearing and a pill used (one whose doses book, at
/// TT), with the PED Card moving by `card` PEC across it and nothing in
/// the minute claiming the movement.
fn worn_beside_the_card(card: i64) -> SessionCosts {
    let mut conn = migrated_in_memory().unwrap();
    the_player_s_gear(&conn);
    conn.execute(
        "INSERT INTO equipment_library (name, item_type, properties_json) \
         VALUES ('Odd Pill', 'consumable', \
                 '{\"dose\": {\"markup_percent\": 100.0, \"track_cost\": true}}')",
        [],
    )
    .unwrap();
    session_row(&conn, "s", 0.0, Some(120.0));
    let readings = [
        reading(
            0.0,
            1_000,
            &[
                ("Frontier Harness, Adjusted (M)", C, 1, 4_000),
                ("Odd Pill", C, 10, 50),
            ],
        ),
        reading(
            60.0,
            1_000 + card,
            &[
                ("Frontier Harness, Adjusted (M)", C, 1, 3_995),
                ("Odd Pill", C, 8, 40),
            ],
        ),
        reading(
            120.0,
            1_000 + card,
            &[
                ("Frontier Harness, Adjusted (M)", C, 1, 3_995),
                ("Odd Pill", C, 8, 40),
            ],
        ),
    ];
    for reading in &readings {
        record_reading(&mut conn, reading, Purpose::Session, Some("s")).unwrap();
    }
    costs_of(&mut conn, "s")
}

#[test]
fn a_card_movement_no_use_claims_changes_no_cost() {
    // The Card falls by 3 PED that neither the armour nor the pill explains.
    let fell = worn_beside_the_card(-300);
    assert_eq!(fell.status, Status::Measured);
    // The armour's decay and the pill's use are billed as in any minute.
    assert_eq!(measured(&fell, CostBucket::Armour), 5 * PEC);
    assert_eq!(measured(&fell, CostBucket::Consumables), 10 * PEC);
    // And every figure is what the same minute with the Card still gives.
    let still = worn_beside_the_card(0);
    assert_eq!(fell.buckets, still.buckets);
    assert_eq!(fell.lines, still.lines);
    assert_eq!(fell.armour, still.armour);
}

#[test]
fn a_repair_is_settlement_and_its_card_cost_no_cost() {
    let mut conn = migrated_in_memory().unwrap();
    the_player_s_gear(&conn);
    session_row(&conn, "s", 0.0, Some(120.0));
    // The weapon wears 3 PEC, then a repair restores 7 PEC and the Card
    // pays exactly that.
    let readings = [
        reading(0.0, 1_000, &[("Opalo", C, 1, 500)]),
        reading(60.0, 1_000, &[("Opalo", C, 1, 497)]),
        reading(120.0, 993, &[("Opalo", C, 1, 504)]),
    ];
    for reading in &readings {
        record_reading(&mut conn, reading, Purpose::Session, Some("s")).unwrap();
    }
    let costs = costs_of(&mut conn, "s");
    assert_eq!(costs.status, Status::Measured);
    // The repair's cost is read beside it in the measure, as a check...
    let measure = match super::sessions::session_measure(
        &conn,
        "s",
        &KnownRoles::read(&conn, None).unwrap(),
        None,
        5_000.0,
        None,
    )
    .unwrap()
    .0
    {
        super::sessions::MeasureRead::Measured { measure, .. } => measure,
        _ => panic!("a measured session"),
    };
    assert_eq!(
        measure.repairs[0].repair.cost,
        super::composite::RepairCost::Paid {
            pec: super::amount::Pec(7)
        }
    );
    // ...and no cost carries it: the weapon is what it lost, and nothing
    // else is billed.
    assert_eq!(measured(&costs, CostBucket::Weapon), 3 * PEC);
    let billed: i64 = costs
        .buckets
        .iter()
        .filter_map(|cost| match cost.basis {
            Basis::Measured(amount) | Basis::Mixed(amount) => Some(amount.micro_ped()),
            Basis::Unmeasured(_) => None,
        })
        .sum();
    assert_eq!(billed, 3 * PEC);
}

/// The weapon and the first aid pack wearing a PEC a minute while Universal
/// Ammo is fired, with a top-up bought in the second minute: 10 000 fired,
/// then 5 000 more than fired bought, then 15 000 fired. Overall the ammo
/// falls.
fn topped_up_hunt(conn: &mut rusqlite::Connection, id: &str) {
    session_row(conn, id, 0.0, Some(180.0));
    let readings = [
        reading(
            0.0,
            1_000,
            &[
                ("Opalo", C, 1, 500),
                ("Universal Ammo", C, 100_000, 1_000),
                ("Frontier First Aid Pack, Adjusted", C, 1, 3000),
            ],
        ),
        reading(
            60.0,
            1_000,
            &[
                ("Opalo", C, 1, 499),
                ("Universal Ammo", C, 90_000, 900),
                ("Frontier First Aid Pack, Adjusted", C, 1, 2999),
            ],
        ),
        reading(
            120.0,
            800,
            &[
                ("Opalo", C, 1, 498),
                ("Universal Ammo", C, 95_000, 950),
                ("Frontier First Aid Pack, Adjusted", C, 1, 2998),
            ],
        ),
        reading(
            180.0,
            800,
            &[
                ("Opalo", C, 1, 497),
                ("Universal Ammo", C, 80_000, 800),
                ("Frontier First Aid Pack, Adjusted", C, 1, 2997),
            ],
        ),
    ];
    for reading in &readings {
        record_reading(conn, reading, Purpose::Session, Some(id)).unwrap();
    }
}

#[test]
fn ammo_topped_up_mid_session_leaves_the_weapon_measured_but_for_that_minute() {
    let mut conn = migrated_in_memory().unwrap();
    the_player_s_gear(&conn);
    topped_up_hunt(&mut conn, "s");
    let costs = costs_of(&mut conn, "s");
    assert_eq!(costs.status, Status::Measured);
    // The first and last minutes' 25 000 units at TT, and the weapon's
    // wear in those minutes: the top-up minute is the tracked figure's
    // whole, its wear with it, since a tracked shot's price holds that.
    let Basis::Mixed(weapon) = basis(&costs, CostBucket::Weapon) else {
        panic!("not mixed: {:?}", basis(&costs, CostBucket::Weapon));
    };
    assert_eq!(weapon.micro_ped(), 250 * PEC + 2 * PEC);
    assert_eq!(line(&costs, "Universal Ammo").units, Some(25_000));
    assert_eq!(
        line(&costs, "Opalo").amount().map(Amount::micro_ped),
        Some(2 * PEC)
    );
    // Wear outside the weapon in the top-up minute is measured as in any.
    assert_eq!(measured(&costs, CostBucket::Healing), 3 * PEC);
    assert_eq!(
        costs.top_ups,
        [TopUp {
            from_ms: 60_000,
            to_ms: 120_000
        }]
    );
    // Stored with its minute, and served as stored.
    assert_eq!(rows(&conn, "account_session_cost_top_ups"), 1);
    let roles = KnownRoles::read(&conn, None).unwrap();
    assert_eq!(
        stored_costs(&conn, "s", roles.fingerprint()).unwrap(),
        Some(costs)
    );
}

#[test]
fn a_traded_known_material_is_no_placement_prompt_and_not_in_other() {
    let mut conn = migrated_in_memory().unwrap();
    let snapshot = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../entropia-orme/resources/snapshot");
    let game_data =
        crate::game_data_store::GameDataStore::new(&snapshot).expect("the bundled game data");
    session_row(&conn, "s", 0.0, Some(120.0));
    // Enhancers the game data knows, traded away; and oil nothing names.
    let readings = [
        reading(
            0.0,
            1_000,
            &[
                ("T1 Weapon Damage Enhancer", C, 10, 1_000),
                ("Animal Oil Residue", C, 4_000, 40),
            ],
        ),
        reading(60.0, 1_000, &[("T1 Weapon Damage Enhancer", C, 8, 800)]),
        reading(120.0, 1_000, &[("T1 Weapon Damage Enhancer", C, 8, 800)]),
    ];
    for reading in &readings {
        record_reading(&mut conn, reading, Purpose::Session, Some("s")).unwrap();
    }
    let costs = session_costs(&mut conn, "s", Some(&game_data), 5_000.0, None)
        .unwrap()
        .expect("a known session");
    assert_eq!(costs.status, Status::Measured);
    // Only the oil is offered for placement.
    let lines: Vec<(&str, Charge)> = costs
        .lines
        .iter()
        .map(|line| (line.item.as_str(), line.charge))
        .collect();
    assert_eq!(
        lines,
        [("Animal Oil Residue", Charge::Nothing(LineReason::NotPlaced))]
    );
    // The enhancers left for the reconciliation, never as used up.
    let roles = KnownRoles::read(&conn, Some(&game_data)).unwrap();
    let super::sessions::MeasureRead::Measured { measure, .. } =
        super::sessions::session_measure(&conn, "s", &roles, None, 5_000.0, None)
            .unwrap()
            .0
    else {
        panic!("a measured session");
    };
    let names = |items: &[super::measure::ItemUnits]| -> Vec<String> {
        items.iter().map(|item| item.key.name.clone()).collect()
    };
    assert_eq!(names(&measure.other_consumption), ["Animal Oil Residue"]);
    assert_eq!(
        names(&measure.unexplained_departures),
        ["T1 Weapon Damage Enhancer"]
    );
    let totals = super::sessions::MeasureTotals::of(&measure).unwrap();
    assert_eq!(totals.other_consumption.micro_ped(), 40 * PEC);
}

#[test]
fn a_session_with_no_top_up_keeps_no_top_up_steps() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", false);
    let costs = costs_of(&mut conn, "hunt");
    // The hunt's Shrapnel turned into ammo is no top-up.
    assert!(matches!(
        basis(&costs, CostBucket::Weapon),
        Basis::Measured(_)
    ));
    assert!(costs.top_ups.is_empty());
    assert_eq!(rows(&conn, "account_session_cost_top_ups"), 0);
}

/// Readings of the weapon wearing a PEC a step from `value`, tagged with
/// `tag`, at the given times, each with any further wear given beside it.
fn wear(
    conn: &mut rusqlite::Connection,
    tag: Option<&str>,
    purpose: Purpose,
    value: &mut i64,
    times: &[(f64, i64)],
) {
    for (at, extra) in times {
        *value -= 1 + extra;
        let reading = reading(*at, 1_000, &[("Opalo", C, 1, *value)]);
        record_reading(conn, &reading, purpose, tag).unwrap();
    }
}

fn opalo_lost(costs: &SessionCosts) -> i64 {
    costs
        .lines
        .iter()
        .filter(|line| line.item == "Opalo")
        .map(|line| line.amount().unwrap().micro_ped())
        .sum()
}

#[test]
fn a_session_started_seconds_after_another_stopped_shares_no_change_with_it() {
    let mut conn = migrated_in_memory().unwrap();
    // The first stopped at 1300 and the second started at 1305, inside the
    // reading floor: no reading came between them, and the first reading
    // of the second (at 1350) holds 50 PEC of wear from either side.
    session_row(&conn, "first", 1_000.0, Some(1_300.0));
    session_row(&conn, "second", 1_305.0, Some(1_500.0));
    let mut value = 5_000;
    wear(
        &mut conn,
        Some("first"),
        Purpose::Baseline,
        &mut value,
        &[(1_000.5, 0)],
    );
    wear(
        &mut conn,
        Some("first"),
        Purpose::Session,
        &mut value,
        &[
            (1_060.0, 0),
            (1_120.0, 0),
            (1_180.0, 0),
            (1_240.0, 0),
            (1_290.0, 0),
        ],
    );
    wear(
        &mut conn,
        Some("second"),
        Purpose::Session,
        &mut value,
        &[(1_350.0, 49), (1_410.0, 0), (1_470.0, 0), (1_530.0, 0)],
    );
    let first = costs_of(&mut conn, "first");
    let second = costs_of(&mut conn, "second");
    assert_eq!(opalo_lost(&first), 5 * PEC);
    assert_eq!(opalo_lost(&second), 3 * PEC);
    // Neither has a reading between the two to read the end or the start
    // it shares: both say so, rather than claim the other's play.
    assert_eq!(
        first.status,
        Status::Unmeasured(Unmeasured::PartialCoverage)
    );
    assert_eq!(
        second.status,
        Status::Unmeasured(Unmeasured::PartialCoverage)
    );
    assert_eq!(
        basis(&first, CostBucket::Weapon),
        Basis::Unmeasured(Unmeasured::PartialCoverage)
    );
}

#[test]
fn play_after_a_session_s_last_closing_reading_is_charged_to_neither_session() {
    let mut conn = migrated_in_memory().unwrap();
    session_row(&conn, "first", 1_000.0, Some(1_300.0));
    session_row(&conn, "second", 2_000.0, Some(2_050.0));
    let mut value = 5_000;
    wear(
        &mut conn,
        Some("first"),
        Purpose::Baseline,
        &mut value,
        &[(1_000.5, 0)],
    );
    // Two closing readings: the 20 PEC of wear the second catches up on is
    // the first session's; the 30 more before the second session's
    // baseline is neither's.
    wear(
        &mut conn,
        Some("first"),
        Purpose::Session,
        &mut value,
        &[
            (1_060.0, 0),
            (1_120.0, 0),
            (1_180.0, 0),
            (1_240.0, 0),
            (1_310.0, 0),
            (1_370.0, 19),
        ],
    );
    wear(
        &mut conn,
        Some("second"),
        Purpose::Baseline,
        &mut value,
        &[(2_000.5, 29)],
    );
    wear(
        &mut conn,
        Some("second"),
        Purpose::Session,
        &mut value,
        &[(2_060.0, 0)],
    );
    let first = costs_of(&mut conn, "first");
    let second = costs_of(&mut conn, "second");
    assert_eq!(first.status, Status::Measured);
    assert_eq!(second.status, Status::Measured);
    assert_eq!(opalo_lost(&first), 25 * PEC);
    assert_eq!(opalo_lost(&second), PEC);
}

#[test]
fn partial_coverage_and_a_gap_leave_every_bucket_unmeasured_with_the_reason() {
    let mut conn = migrated_in_memory().unwrap();
    // Read from part-way through: nothing reads the start.
    session_row(&conn, "late", 0.0, Some(300.0));
    let mut value = 5_000;
    wear(
        &mut conn,
        Some("late"),
        Purpose::Session,
        &mut value,
        &[(100.0, 0), (160.0, 0), (220.0, 0), (310.0, 0)],
    );
    let late = costs_of(&mut conn, "late");
    assert_eq!(late.status, Status::Unmeasured(Unmeasured::PartialCoverage));
    for cost in &late.buckets {
        let expected = if cost.bucket == CostBucket::Enhancer {
            Unmeasured::TrackedOnly
        } else {
            Unmeasured::PartialCoverage
        };
        assert_eq!(cost.basis, Basis::Unmeasured(expected), "{:?}", cost.bucket);
    }
    // Both ends read, but the readings stop for four minutes on the way.
    session_row(&conn, "gapped", 1_000.0, Some(1_400.0));
    wear(
        &mut conn,
        Some("gapped"),
        Purpose::Baseline,
        &mut value,
        &[(1_000.5, 0)],
    );
    wear(
        &mut conn,
        Some("gapped"),
        Purpose::Session,
        &mut value,
        &[(1_060.0, 0), (1_300.0, 0), (1_400.0, 0)],
    );
    let gapped = costs_of(&mut conn, "gapped");
    assert_eq!(gapped.status, Status::Unmeasured(Unmeasured::Gap));
    assert_eq!(
        basis(&gapped, CostBucket::Weapon),
        Basis::Unmeasured(Unmeasured::Gap)
    );
    // Partial sessions are stored too, with their reason and no amount.
    let stored: (String, String) = conn
        .query_row(
            "SELECT status, reason FROM account_session_costs WHERE session_id = 'gapped'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(stored, ("unmeasured".to_string(), "gap".to_string()));
    let amounts: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM account_session_cost_buckets \
             WHERE session_id = 'gapped' AND amount_micro_ped IS NOT NULL",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(amounts, 0);
}

#[test]
fn an_unsettled_ammo_split_leaves_only_the_weapon_unmeasured() {
    let mut conn = migrated_in_memory().unwrap();
    conn.execute(
        "INSERT INTO equipment_library (name, item_type, properties_json) \
         VALUES ('Odd FAP', 'healing', '{}')",
        [],
    )
    .unwrap();
    session_row(&conn, "s", 0.0, Some(120.0));
    // Shrapnel left with no ammo row for its conversion: the ammo it
    // became stacked onto the row being fired, so what was burned is not
    // known.
    let readings = [
        reading(
            0.0,
            1_000,
            &[
                ("Universal Ammo", C, 100_000, 1_000),
                ("Shrapnel", C, 10_000, 100),
                ("Odd FAP", C, 1, 500),
            ],
        ),
        reading(
            60.0,
            1_000,
            &[
                ("Universal Ammo", C, 105_100, 1_051),
                ("Odd FAP", C, 1, 498),
            ],
        ),
        reading(
            120.0,
            1_000,
            &[("Universal Ammo", C, 95_100, 951), ("Odd FAP", C, 1, 497)],
        ),
    ];
    for reading in &readings {
        record_reading(&mut conn, reading, Purpose::Session, Some("s")).unwrap();
    }
    let costs = costs_of(&mut conn, "s");
    assert_eq!(costs.status, Status::Measured);
    assert_eq!(
        basis(&costs, CostBucket::Weapon),
        Basis::Unmeasured(Unmeasured::AmmoUnsettled)
    );
    assert!(costs.top_ups.is_empty(), "a conversion is no top-up");
    assert!(costs
        .lines
        .iter()
        .all(|line| line.kind != LineKind::AmmoBurned));
    assert_eq!(measured(&costs, CostBucket::Healing), 3 * PEC);
}

#[test]
fn a_session_with_no_readings_or_still_running_stores_nothing() {
    let mut conn = migrated_in_memory().unwrap();
    assert!(session_costs(&mut conn, "absent", None, 1.0, None)
        .unwrap()
        .is_none());
    session_row(&conn, "quiet", 0.0, Some(100.0));
    let quiet = costs_of(&mut conn, "quiet");
    assert_eq!(quiet.status, Status::Unmeasured(Unmeasured::NoReadings));
    assert!(quiet.lines.is_empty());
    session_row(&conn, "live", 1_000.0, None);
    let mut value = 5_000;
    wear(
        &mut conn,
        Some("live"),
        Purpose::Baseline,
        &mut value,
        &[(1_000.5, 0)],
    );
    wear(
        &mut conn,
        Some("live"),
        Purpose::Session,
        &mut value,
        &[(1_060.0, 0)],
    );
    let live = costs_of(&mut conn, "live");
    assert_eq!(live.status, Status::Unmeasured(Unmeasured::StillRunning));
    assert_eq!(
        basis(&live, CostBucket::Weapon),
        Basis::Unmeasured(Unmeasured::StillRunning)
    );
    assert_eq!(opalo_lost(&live), PEC, "its lines so far");
    // A session that has stopped but is still being read to its close is
    // running too.
    conn.execute(
        "UPDATE tracking_sessions SET ended_at = 1100, is_active = 0 WHERE id = 'live'",
        [],
    )
    .unwrap();
    let closing = session_costs(&mut conn, "live", None, 1_200.0, Some("live"))
        .unwrap()
        .unwrap();
    assert_eq!(closing.status, Status::Unmeasured(Unmeasured::StillRunning));
    assert_eq!(stored_rows(&conn), (0, 0, 0));
    assert_eq!(rows(&conn, "account_session_measures"), 0);
}

#[test]
fn a_stale_projection_is_rebuilt_on_the_next_read_and_a_current_one_is_served() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", false);
    let fresh = costs_of(&mut conn, "hunt");
    let weapon = measured(&fresh, CostBucket::Weapon);
    let tamper = |conn: &rusqlite::Connection| {
        conn.execute(
            "UPDATE account_session_cost_buckets SET amount_micro_ped = 1 \
             WHERE session_id = 'hunt' AND bucket = 'weapon'",
            [],
        )
        .unwrap();
    };
    // A current projection is what a read serves.
    tamper(&conn);
    assert_eq!(
        measured(&costs_of(&mut conn, "hunt"), CostBucket::Weapon),
        1
    );
    // One of other rules (a later build's) is rebuilt.
    conn.execute(
        "UPDATE account_session_costs SET costs_version = ?1",
        [COSTS_VERSION + 1],
    )
    .unwrap();
    assert_eq!(costs_of(&mut conn, "hunt"), fresh);
    let version: i64 = conn
        .query_row(
            "SELECT costs_version FROM account_session_costs",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version, COSTS_VERSION);
    // One older than its measure is rebuilt.
    tamper(&conn);
    conn.execute(
        "UPDATE account_session_measures SET computed_at = computed_at + 1",
        [],
    )
    .unwrap();
    assert_eq!(
        measured(&costs_of(&mut conn, "hunt"), CostBucket::Weapon),
        weapon
    );
    // One derived from a measure of older rules is rebuilt with the
    // measure, which is sealed again.
    tamper(&conn);
    conn.execute(
        "UPDATE account_session_measures SET measure_version = 4",
        [],
    )
    .unwrap();
    assert_eq!(
        measured(&costs_of(&mut conn, "hunt"), CostBucket::Weapon),
        weapon
    );
    let sealed: (i64, Option<f64>) = conn
        .query_row(
            "SELECT measure_version, sealed_at FROM account_session_measures",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(sealed.0, super::measure::MEASURE_VERSION);
    assert!(sealed.1.is_some());
}

#[test]
fn recomputing_a_projection_changes_nothing() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", true);
    let first = costs_of(&mut conn, "hunt");
    let snapshot = |conn: &rusqlite::Connection| -> Vec<String> {
        let mut stmt = conn
            .prepare(
                "SELECT position || '|' || bucket || '|' || category || '|' || kind || '|' || \
                        item_name || '|' || COALESCE(units, '') || '|' || \
                        COALESCE(amount_micro_ped, '') || '|' || COALESCE(precision, '') || \
                        '|' || COALESCE(markup_hundredths, '') || '|' || COALESCE(reason, '') || \
                        '|' || COALESCE(set_id, '') \
                 FROM account_session_cost_lines ORDER BY position",
            )
            .unwrap();
        let mut lines: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT bucket || '|' || basis || '|' || COALESCE(reason, '') || '|' || \
                        COALESCE(amount_micro_ped, '') || '|' || COALESCE(precision, '') \
                 FROM account_session_cost_buckets ORDER BY bucket",
            )
            .unwrap();
        lines.extend(
            stmt.query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<rusqlite::Result<Vec<_>>>()
                .unwrap(),
        );
        lines
    };
    let before = snapshot(&conn);
    conn.execute(
        "UPDATE account_session_costs SET costs_version = ?1",
        [COSTS_VERSION + 1],
    )
    .unwrap();
    let again = costs_of(&mut conn, "hunt");
    assert_eq!(again, first);
    assert_eq!(snapshot(&conn), before);
    assert_eq!(stored_rows(&conn).0, 1);
}

#[test]
fn a_measure_written_again_takes_the_costs_stored_from_it_away() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", false);
    costs_of(&mut conn, "hunt");
    assert_eq!(stored_rows(&conn).0, 1);
    let window = super::measure::SessionWindow {
        started_at: STARTED,
        ended_at: Some(STOPPED),
    };
    super::store::refresh_session_measure(
        &conn,
        "hunt",
        &window,
        &super::roles::KnownRoles::default(),
        None,
        6_000.0,
    )
    .unwrap();
    assert_eq!(stored_rows(&conn), (0, 0, 0));
}

#[tokio::test]
async fn deleting_a_session_leaves_no_projection_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Db::open(&dir.path().join("costs.db"))
        .await
        .unwrap();
    db.with_writer(|conn| {
        store_the_hunt(conn, "hunt", true);
        session_costs(conn, "hunt", None, 5_000.0, None).unwrap();
        assert_eq!(stored_rows(conn).0, 1);
        Ok(())
    })
    .await
    .unwrap();
    crate::session_delete::delete_session_impl(&db, "hunt")
        .await
        .unwrap();
    let left = db.with_reader(|conn| Ok(stored_rows(conn))).await.unwrap();
    assert_eq!(left, (0, 0, 0));
}

#[test]
fn a_seal_by_older_rules_is_replaced_whatever_it_counted() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", false);
    let seal = measure_to_seal(&conn, "hunt", None, None, 2_000.0)
        .unwrap()
        .expect("an ended hunt with readings");
    assert_eq!(cache_sealed(&mut conn, &[seal], 2_000.0).unwrap(), 1);
    // A seal by older rules that counted more readings than these do.
    conn.execute(
        "UPDATE account_session_measures SET measure_version = ?1, reading_count = 99",
        [super::measure::MEASURE_VERSION - 1],
    )
    .unwrap();
    let costs = costs_of(&mut conn, "hunt");
    assert_eq!(measured(&costs, CostBucket::Armour), 113 * PEC);
    let sealed: (i64, i64) = conn
        .query_row(
            "SELECT measure_version, reading_count FROM account_session_measures",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(sealed, (super::measure::MEASURE_VERSION, 12));
    let roles = KnownRoles::read(&conn, None).unwrap();
    assert_eq!(
        stored_costs(&conn, "hunt", roles.fingerprint()).unwrap(),
        Some(costs)
    );
}

#[test]
fn placing_an_item_after_the_seal_moves_its_cost_where_it_belongs() {
    let mut conn = migrated_in_memory().unwrap();
    session_row(&conn, "s", 0.0, Some(120.0));
    for (at, value) in [(0.0, 1_000), (60.0, 990), (120.0, 985)] {
        let reading = reading(at, 1_000, &[("Odd Harness (M)", C, 1, value)]);
        record_reading(&mut conn, &reading, Purpose::Session, Some("s")).unwrap();
    }
    let before = costs_of(&mut conn, "s");
    let harness = line(&before, "Odd Harness (M)");
    assert_eq!(
        (harness.bucket, harness.category),
        (CostBucket::Unclassified, CostCategory::Unclassified)
    );
    assert_eq!(measured(&before, CostBucket::Unclassified), 15 * PEC);
    // The player names the set the harness belongs to.
    conn.execute(
        "INSERT INTO protection_sets (kind, name, economy_kind, markup_percent, created_at) \
         VALUES ('armour', 'Odd', 'unlimited', NULL, 1)",
        [],
    )
    .unwrap();
    let roles = KnownRoles::read(&conn, None).unwrap();
    assert_eq!(stored_costs(&conn, "s", roles.fingerprint()).unwrap(), None);
    assert_eq!(
        super::costs::sessions_with_stale_costs(&conn, roles.fingerprint()).unwrap(),
        ["s"]
    );
    let after = costs_of(&mut conn, "s");
    let harness = line(&after, "Odd Harness (M)");
    assert_eq!(
        (harness.bucket, harness.category),
        (CostBucket::Armour, CostCategory::UnlimitedArmour)
    );
    assert_eq!(measured(&after, CostBucket::Armour), 15 * PEC);
    assert_eq!(measured(&after, CostBucket::Unclassified), 0);
    assert!(
        super::costs::sessions_with_stale_costs(&conn, roles.fingerprint())
            .unwrap()
            .is_empty()
    );
}

/// A session over which a hoverpod nothing names wore 0.15, stored.
fn a_hoverpod_wearing(conn: &mut rusqlite::Connection) {
    session_row(conn, "s", 0.0, Some(120.0));
    for (at, value) in [(0.0, 1_000), (60.0, 990), (120.0, 985)] {
        let reading = reading(at, 1_000, &[("Exploration Hoverpod MkI (L)", C, 1, value)]);
        record_reading(conn, &reading, Purpose::Session, Some("s")).unwrap();
    }
}

fn place(conn: &rusqlite::Connection, item: &str, category: CostCategory) {
    let placement = super::placement::Placement::new(item, category).expect("placeable");
    super::placement::place(conn, &placement, 50.0).unwrap();
}

#[test]
fn an_item_the_app_cannot_place_is_placed_once_for_every_session() {
    const HOVERPOD: &str = "Exploration Hoverpod MkI (L)";
    let mut conn = migrated_in_memory().unwrap();
    a_hoverpod_wearing(&mut conn);
    let unplaced = costs_of(&mut conn, "s");
    let hoverpod = line(&unplaced, HOVERPOD);
    assert_eq!(
        (hoverpod.bucket, hoverpod.category),
        (CostBucket::Unclassified, CostCategory::Unclassified)
    );
    assert_eq!(measured(&unplaced, CostBucket::Unclassified), 15 * PEC);
    // It awaits a placement, with the sessions it wore in and what it lost.
    let awaiting = super::placement::awaiting(&conn).unwrap();
    assert_eq!(awaiting.len(), 1);
    assert_eq!(awaiting[0].item, HOVERPOD);
    assert_eq!(awaiting[0].sessions, 1);
    assert_eq!(awaiting[0].lost.map(Amount::micro_ped), Some(15 * PEC));
    assert_eq!(awaiting[0].units, None);

    // Placed as travel, its wear is travel, and unclassified holds nothing.
    place(&conn, HOVERPOD, CostCategory::Travel);
    let roles = KnownRoles::read(&conn, None).unwrap();
    assert_eq!(
        super::costs::sessions_with_stale_costs(&conn, roles.fingerprint()).unwrap(),
        ["s"],
        "a placement leaves the stored costs stale"
    );
    let travel = costs_of(&mut conn, "s");
    let hoverpod = line(&travel, HOVERPOD);
    assert_eq!(
        (hoverpod.bucket, hoverpod.category),
        (CostBucket::Travel, CostCategory::Travel)
    );
    assert_eq!(measured(&travel, CostBucket::Travel), 15 * PEC);
    assert_eq!(measured(&travel, CostBucket::Unclassified), 0);
    assert!(super::placement::awaiting(&conn).unwrap().is_empty());
    let placed = super::placement::read_all(&conn).unwrap();
    assert_eq!(placed.len(), 1);
    assert_eq!(
        (placed[0].item.as_str(), placed[0].category),
        (HOVERPOD, CostCategory::Travel)
    );

    // Placed as a healing tool, it moves into Cycled's healing.
    place(&conn, HOVERPOD, CostCategory::HealingTool);
    let healing = costs_of(&mut conn, "s");
    assert_eq!(line(&healing, HOVERPOD).bucket, CostBucket::Healing);
    assert_eq!(measured(&healing, CostBucket::Healing), 15 * PEC);
    assert_eq!(measured(&healing, CostBucket::Travel), 0);

    // As no cost, it is in no bucket and no line.
    place(&conn, HOVERPOD, CostCategory::NotACost);
    let none = costs_of(&mut conn, "s");
    assert!(none.lines.iter().all(|line| line.item != HOVERPOD));
    for bucket in [
        CostBucket::Travel,
        CostBucket::Healing,
        CostBucket::Unclassified,
    ] {
        assert_eq!(measured(&none, bucket), 0, "{bucket:?}");
    }

    // Cleared, it is unclassified again, and awaits a placement.
    assert!(super::placement::clear(&conn, HOVERPOD).unwrap());
    let cleared = costs_of(&mut conn, "s");
    assert_eq!(cleared, unplaced);
    assert_eq!(super::placement::awaiting(&conn).unwrap().len(), 1);
    assert!(!super::placement::clear(&conn, HOVERPOD).unwrap());
}

#[test]
fn a_placement_outranks_the_library_and_the_game_data() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", false);
    // The library calls the first aid pack a healing tool; the player
    // says it is field logistics.
    place(
        &conn,
        "Frontier First Aid Pack, Adjusted",
        CostCategory::FieldLogistics,
    );
    let costs = costs_of(&mut conn, "hunt");
    let fap = line(&costs, "Frontier First Aid Pack, Adjusted");
    assert_eq!(
        (fap.bucket, fap.category),
        (CostBucket::FieldLogistics, CostCategory::FieldLogistics)
    );
    assert_eq!(measured(&costs, CostBucket::Healing), 0);
}

#[test]
fn only_a_named_item_in_a_placeable_category_is_a_placement() {
    use super::placement::Placement;
    assert!(Placement::new("  ", CostCategory::Travel).is_none());
    for category in [
        CostCategory::Ammo,
        CostCategory::Enhancer,
        CostCategory::LimitedArmour,
        CostCategory::LimitedPlate,
        CostCategory::TeleportChip,
        CostCategory::MindEssence,
        CostCategory::Unclassified,
    ] {
        assert!(Placement::new("Odd", category).is_none(), "{category:?}");
    }
    let placed = Placement::new(" Odd Pod ", CostCategory::Travel).unwrap();
    assert_eq!(placed.item(), "Odd Pod");
    // The table refuses what the type refuses.
    let conn = migrated_in_memory().unwrap();
    for (item, category) in [("Odd", "ammo"), (" Odd", "travel"), ("", "travel")] {
        assert!(
            conn.execute(
                "INSERT INTO cost_item_placements (item_name, category, placed_at) \
                 VALUES (?1, ?2, 1.0)",
                [item, category],
            )
            .is_err(),
            "{item:?} {category}"
        );
    }
    // Every category a placement may hold reads back as one.
    for category in CostCategory::PLACEABLE {
        conn.execute(
            "INSERT INTO cost_item_placements (item_name, category, placed_at) \
             VALUES (?1, ?2, 1.0)",
            [category.as_str(), category.as_str()],
        )
        .unwrap();
    }
    assert_eq!(
        super::placement::read_all(&conn).unwrap().len(),
        CostCategory::PLACEABLE.len()
    );
}

#[test]
fn a_limited_piece_s_line_names_the_set_that_priced_it() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", true);
    let unconfirmed = costs_of(&mut conn, "hunt");
    assert_eq!(line(&unconfirmed, "Pegasus Helmet (M,L)").set_id, None);
    confirm_pieces(&conn, "Pegasus", &["Pegasus Helmet (M,L)"]);
    let confirmed = costs_of(&mut conn, "hunt");
    assert_eq!(
        line(&confirmed, "Pegasus Helmet (M,L)").set_id,
        Some(set_id(&conn, "Pegasus"))
    );
    // Kept and read back with it.
    let roles = KnownRoles::read(&conn, None).unwrap();
    assert_eq!(
        stored_costs(&conn, "hunt", roles.fingerprint()).unwrap(),
        Some(confirmed)
    );
}

#[test]
fn a_stack_nothing_names_that_left_awaits_a_placement_and_is_billed_once_placed() {
    let mut conn = migrated_in_memory().unwrap();
    session_row(&conn, "s", 0.0, Some(120.0));
    for (at, units) in [(0.0, 1_000), (60.0, 900), (120.0, 700)] {
        let reading = reading(at, 1_000, &[("Odd Fuel", C, units, units / 100)]);
        record_reading(&mut conn, &reading, Purpose::Session, Some("s")).unwrap();
    }
    let unplaced = costs_of(&mut conn, "s");
    let fuel = line(&unplaced, "Odd Fuel");
    assert_eq!(
        (fuel.bucket, fuel.kind, fuel.units, fuel.charge),
        (
            CostBucket::Unclassified,
            LineKind::Consumed,
            Some(300),
            Charge::Nothing(LineReason::NotPlaced)
        )
    );
    // Listed, billed nothing.
    assert_eq!(measured(&unplaced, CostBucket::Unclassified), 0);
    let awaiting = super::placement::awaiting(&conn).unwrap();
    assert_eq!(awaiting.len(), 1);
    assert_eq!(
        (
            awaiting[0].item.as_str(),
            awaiting[0].units,
            awaiting[0].lost
        ),
        ("Odd Fuel", Some(300), None)
    );
    // Placed as travel, its units used up are travel, at TT.
    place(&conn, "Odd Fuel", CostCategory::Travel);
    let travel = costs_of(&mut conn, "s");
    let fuel = line(&travel, "Odd Fuel");
    assert_eq!(
        (fuel.bucket, fuel.category, fuel.units),
        (CostBucket::Travel, CostCategory::Travel, Some(300))
    );
    assert_eq!(measured(&travel, CostBucket::Travel), 3 * PEC);
    assert!(super::placement::awaiting(&conn).unwrap().is_empty());
}

#[test]
fn a_traded_loot_material_is_no_placement_prompt_and_no_other_consumption() {
    let mut conn = migrated_in_memory().unwrap();
    session_row(&conn, "s", 0.0, Some(120.0));
    // A hide the player looted before, traded away mid-session, beside a
    // stack nothing names that was used up.
    conn.execute(
        "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped) \
         VALUES ('earlier', 'Animal Hide', 50, 0.5)",
        [],
    )
    .unwrap();
    let readings = [
        reading(
            0.0,
            1_000,
            &[("Animal Hide", C, 50, 50), ("Odd Fuel", C, 1_000, 10)],
        ),
        reading(60.0, 1_000, &[("Odd Fuel", C, 900, 9)]),
        reading(120.0, 1_000, &[("Odd Fuel", C, 700, 7)]),
    ];
    for reading in &readings {
        record_reading(&mut conn, reading, Purpose::Session, Some("s")).unwrap();
    }
    let costs = costs_of(&mut conn, "s");
    assert!(
        costs.lines.iter().all(|line| line.item != "Animal Hide"),
        "{:?}",
        costs.lines
    );
    assert_eq!(
        line(&costs, "Odd Fuel").charge,
        Charge::Nothing(LineReason::NotPlaced)
    );
    let awaiting = super::placement::awaiting(&conn).unwrap();
    assert_eq!(
        awaiting
            .iter()
            .map(|item| item.item.as_str())
            .collect::<Vec<_>>(),
        ["Odd Fuel"]
    );
    // The measure leaves the hide out of what was used up: a departure,
    // not "Other".
    let super::store::CachedMeasure::Current { measure, .. } =
        super::store::cached_measure(&conn, "s").unwrap()
    else {
        panic!("a sealed measure");
    };
    assert!(measure
        .other_consumption
        .iter()
        .all(|item| item.key.name != "Animal Hide"));
    assert!(measure
        .unexplained_departures
        .iter()
        .any(|item| item.key.name == "Animal Hide"));
    let totals = super::sessions::MeasureTotals::of(&measure).unwrap();
    assert_eq!(totals.other_consumption.micro_ped(), 3 * PEC);

    // What the app holds as stock is a material too, and knowing it is part
    // of the roles the costs were placed by.
    let before = KnownRoles::read(&conn, None).unwrap();
    conn.execute(
        "INSERT INTO stock_movements (item_name, movement_kind, source_kind, quantity, \
         tt_value, occurred_at, created_at) \
         VALUES ('Odd Fuel', 'opening_balance', 'unattributed', 5, 0.05, '2026-01-01', 1.0)",
        [],
    )
    .unwrap();
    let after = KnownRoles::read(&conn, None).unwrap();
    assert_ne!(after.fingerprint(), before.fingerprint());
    let costs = costs_of(&mut conn, "s");
    assert!(costs.lines.iter().all(|line| line.item != "Odd Fuel"));
    assert!(super::placement::awaiting(&conn).unwrap().is_empty());
}

#[test]
fn a_placement_that_moves_nothing_rebuilds_nothing() {
    let mut conn = migrated_in_memory().unwrap();
    a_hoverpod_wearing(&mut conn);
    let before = costs_of(&mut conn, "s");
    let computed_at = |conn: &rusqlite::Connection| -> (f64, String) {
        conn.query_row(
            "SELECT computed_at, roles_fingerprint FROM account_session_costs \
             WHERE session_id = 's'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    };
    let (stored_at, stored_roles) = computed_at(&conn);
    // An item no session saw: every projection is stale by its digest, and
    // the rebuild finds the same costs, so it moves only the digest.
    place(&conn, "Never Seen", CostCategory::Travel);
    let roles = KnownRoles::read(&conn, None).unwrap();
    assert_eq!(
        super::costs::sessions_with_stale_costs(&conn, roles.fingerprint()).unwrap(),
        ["s"]
    );
    let after = session_costs(&mut conn, "s", None, 9_000.0, None)
        .unwrap()
        .unwrap();
    assert_eq!(
        SessionCosts {
            roles_fingerprint: before.roles_fingerprint.clone(),
            ..after
        },
        before
    );
    let (kept_at, kept_roles) = computed_at(&conn);
    assert_eq!(kept_at, stored_at, "nothing derived from it is rebuilt");
    assert_ne!(kept_roles, stored_roles);
    assert_eq!(kept_roles, roles.fingerprint());
    assert!(
        super::costs::sessions_with_stale_costs(&conn, roles.fingerprint())
            .unwrap()
            .is_empty()
    );
    // One that moves an item rebuilds the costs.
    place(&conn, "Exploration Hoverpod MkI (L)", CostCategory::Travel);
    session_costs(&mut conn, "s", None, 9_500.0, None).unwrap();
    assert_eq!(computed_at(&conn).0, 9_500.0);
}

#[test]
fn a_consumable_used_up_follows_the_player_s_choice_for_it() {
    let mut conn = migrated_in_memory().unwrap();
    session_row(&conn, "s", 0.0, Some(120.0));
    // One pill whose doses book at the 150% it was bought at; one stim the
    // player accounts for through the ledger; and, in the game data only,
    // a stim and a healing pack the library does not hold.
    conn.execute(
        "INSERT INTO equipment_library (name, item_type, properties_json) VALUES \
         ('Booked Pill', 'consumable', \
          '{\"dose\": {\"markup_percent\": 150.0, \"track_cost\": true}}'), \
         ('Ledger Stim', 'consumable', \
          '{\"dose\": {\"markup_percent\": 200.0, \"track_cost\": false}}')",
        [],
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("stimulants.json"),
        r#"[{"id": 1, "name": "Odd Stim"}]"#,
    )
    .unwrap();
    std::fs::write(
        dir.path().join("medical_tools.json"),
        r#"[{"id": 2, "name": "Odd Pack"}]"#,
    )
    .unwrap();
    let store = crate::game_data_store::GameDataStore::new(dir.path()).unwrap();
    let readings = [
        reading(
            0.0,
            1_000,
            &[
                ("Booked Pill", C, 10, 50),
                ("Ledger Stim", C, 10, 100),
                ("Odd Stim", C, 5, 50),
                ("Odd Pack", C, 20, 20),
            ],
        ),
        reading(
            60.0,
            1_000,
            &[
                ("Booked Pill", C, 9, 45),
                ("Ledger Stim", C, 8, 80),
                ("Odd Stim", C, 5, 50),
                ("Odd Pack", C, 19, 19),
            ],
        ),
        reading(
            120.0,
            1_000,
            &[
                ("Booked Pill", C, 8, 40),
                ("Ledger Stim", C, 7, 70),
                ("Odd Stim", C, 4, 40),
                ("Odd Pack", C, 18, 18),
            ],
        ),
    ];
    for reading in &readings {
        record_reading(&mut conn, reading, Purpose::Session, Some("s")).unwrap();
    }
    let costs = session_costs(&mut conn, "s", Some(&store), 5_000.0, None)
        .unwrap()
        .unwrap();
    assert_eq!(costs.status, Status::Measured);
    let booked = line(&costs, "Booked Pill");
    assert_eq!(booked.units, Some(2));
    assert_eq!(
        booked.charge,
        Charge::AtMarkup {
            amount: Amount::WholePec {
                micro_ped: 15 * PEC
            },
            markup_hundredths: 15_000,
        }
    );
    let ledger = line(&costs, "Ledger Stim");
    assert_eq!(
        (ledger.units, ledger.charge),
        (Some(3), Charge::Nothing(LineReason::BoughtThroughLedger))
    );
    let unknown = line(&costs, "Odd Stim");
    assert_eq!(unknown.category, CostCategory::Consumable);
    assert_eq!(unknown.charge.amount().unwrap().micro_ped(), 10 * PEC);
    assert!(matches!(unknown.charge, Charge::AtTt(_)));
    let pack = line(&costs, "Odd Pack");
    assert_eq!(
        (pack.bucket, pack.category, pack.units),
        (
            CostBucket::Healing,
            CostCategory::HealingConsumable,
            Some(2)
        )
    );
    assert!(matches!(pack.charge, Charge::AtTt(_)));
    // The ledger's stim is in no figure: 15 PEC at markup and 10 at TT.
    assert_eq!(measured(&costs, CostBucket::Consumables), 25 * PEC);
    assert_eq!(measured(&costs, CostBucket::Healing), 2 * PEC);
    // Stored and read back as they were charged.
    let roles = KnownRoles::read(&conn, Some(&store)).unwrap();
    assert_eq!(
        stored_costs(&conn, "s", roles.fingerprint()).unwrap(),
        Some(costs)
    );
}

#[test]
fn the_rules_versions_are_pinned_and_the_resolver_reads_the_same_ones() {
    // A change of the measure's or the costs' rules bumps its version:
    // the bump is what rebuilds everything the earlier rules stored.
    assert_eq!(super::measure::MEASURE_VERSION, 6);
    assert_eq!(COSTS_VERSION, 5);
    // The resolver names its own copies; one left behind would read every
    // stored projection as stale.
    assert_eq!(
        crate::session_costs::MEASURED_MEASURE_VERSION,
        super::measure::MEASURE_VERSION
    );
    assert_eq!(crate::session_costs::MEASURED_COSTS_VERSION, COSTS_VERSION);
}

#[tokio::test]
async fn the_sweep_rebuilds_every_stale_projection_and_what_derives_from_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Db::open(&dir.path().join("costs.db"))
        .await
        .unwrap();
    db.with_writer(|conn| {
        store_the_hunt(conn, "hunt", false);
        session_costs(conn, "hunt", None, 5_000.0, None).unwrap();
        // Stored by other rules, and the figures derived from it gone.
        conn.execute(
            "UPDATE account_session_costs SET costs_version = ?1",
            [COSTS_VERSION + 1],
        )?;
        conn.execute("DELETE FROM session_cost_resolutions", [])?;
        Ok(())
    })
    .await
    .unwrap();
    let sweep = |db: crate::db::Db| async move {
        tokio::task::spawn_blocking(move || heal_session_costs(&db, None, None, 6_000.0, None))
            .await
            .unwrap()
            .unwrap()
    };
    assert_eq!(sweep(db.clone()).await, 1);
    let (version, resolutions): (i64, i64) = db
        .with_reader(|conn| {
            Ok(conn.query_row(
                "SELECT (SELECT costs_version FROM account_session_costs), \
                        (SELECT COUNT(*) FROM session_cost_resolutions)",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?)
        })
        .await
        .unwrap();
    assert_eq!((version, resolutions), (COSTS_VERSION, 1));
    assert_eq!(sweep(db).await, 0, "nothing is left stale");
}

/// Name `items` as the pieces of the limited set `set`, as the player
/// confirms them.
fn confirm_pieces(conn: &rusqlite::Connection, set: &str, items: &[&str]) {
    for item in items {
        conn.execute(
            "INSERT INTO protection_set_pieces (set_id, item_name, confirmed_at) \
             SELECT id, ?2, 3.0 FROM protection_sets WHERE name = ?1",
            rusqlite::params![set, item],
        )
        .unwrap();
    }
}

fn set_id(conn: &rusqlite::Connection, name: &str) -> i64 {
    conn.query_row(
        "SELECT id FROM protection_sets WHERE name = ?1",
        [name],
        |row| row.get(0),
    )
    .unwrap()
}

#[test]
fn a_confirmed_set_s_pieces_bill_the_tt_they_lost_at_its_markup() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", true);
    conn.execute(
        "UPDATE protection_sets SET markup_percent = 140 WHERE name = 'Pegasus'",
        [],
    )
    .unwrap();
    confirm_pieces(&conn, "Pegasus", &["Pegasus Helmet (M,L)"]);
    let costs = costs_of(&mut conn, "hunt");

    // The helmet lost 0.33 TT; at 140% that is 0.462 of armour, beside the
    // unlimited armour's 1.13.
    let pegasus = set_id(&conn, "Pegasus");
    assert_eq!(
        costs.armour,
        [
            ArmourPart::Unlimited {
                lost: Amount::WholePec {
                    micro_ped: 113 * PEC
                }
            },
            ArmourPart::Set {
                set_id: pegasus,
                lost: Amount::WholePec {
                    micro_ped: 33 * PEC
                },
                markup_hundredths: 14_000,
                cost: Amount::WholePec { micro_ped: 462_000 },
            },
        ]
    );
    assert_eq!(measured(&costs, CostBucket::Armour), 113 * PEC + 462_000);
    // The helmet's own line still says what it lost, not what it cost.
    assert_eq!(
        line(&costs, "Pegasus Helmet (M,L)")
            .amount()
            .unwrap()
            .micro_ped(),
        33 * PEC
    );

    // Stored and read back part for part.
    let roles = KnownRoles::read(&conn, None).unwrap();
    assert_eq!(
        stored_costs(&conn, "hunt", roles.fingerprint()).unwrap(),
        Some(costs)
    );
}

#[test]
fn a_falling_limited_piece_in_no_confirmed_set_leaves_only_the_limited_part_unmeasured() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", true);
    // A set is confirmed, but the helmet that fell is not one of its pieces.
    conn.execute(
        "INSERT INTO protection_sets (kind, name, economy_kind, markup_percent, created_at) \
         VALUES ('plates', '5B plates', 'limited', 130, 1)",
        [],
    )
    .unwrap();
    confirm_pieces(&conn, "5B plates", &["Armor Plating Mk. 5B (L)"]);
    let costs = costs_of(&mut conn, "hunt");
    assert_eq!(
        basis(&costs, CostBucket::Armour),
        Basis::Unmeasured(Unmeasured::LimitedSetUnconfirmed)
    );
    let plates = set_id(&conn, "5B plates");
    // The set whose pieces did not fall is measured at nothing in it.
    assert_eq!(
        costs.armour,
        [
            ArmourPart::Unlimited {
                lost: Amount::WholePec {
                    micro_ped: 113 * PEC
                }
            },
            ArmourPart::Set {
                set_id: plates,
                lost: Amount::NONE,
                markup_hundredths: 13_000,
                cost: Amount::NONE,
            },
            ArmourPart::Unconfirmed {
                lost: Amount::WholePec {
                    micro_ped: 33 * PEC
                }
            },
        ]
    );
}

#[test]
fn a_session_measured_in_part_keeps_no_armour_parts() {
    let mut conn = migrated_in_memory().unwrap();
    the_player_s_gear(&conn);
    session_row(&conn, "s", 1_000.0, Some(1_300.0));
    // Read only after the start: its opening edge is unread.
    for (k, value) in [990, 980].into_iter().enumerate() {
        let reading = reading(
            1_100.0 + 60.0 * k as f64,
            1_000,
            &[("Frontier Harness, Adjusted (M)", C, 1, value)],
        );
        record_reading(&mut conn, &reading, Purpose::Session, Some("s")).unwrap();
    }
    let costs = costs_of(&mut conn, "s");
    assert!(matches!(costs.status, Status::Unmeasured(_)));
    assert!(costs.armour.is_empty());
}

#[test]
fn confirming_pieces_rebuilds_a_sealed_session_s_armour() {
    let mut conn = migrated_in_memory().unwrap();
    store_the_hunt(&mut conn, "hunt", true);
    conn.execute(
        "UPDATE protection_sets SET markup_percent = 140 WHERE name = 'Pegasus'",
        [],
    )
    .unwrap();
    let before = costs_of(&mut conn, "hunt");
    assert_eq!(
        basis(&before, CostBucket::Armour),
        Basis::Unmeasured(Unmeasured::LimitedSetUnconfirmed)
    );
    confirm_pieces(&conn, "Pegasus", &["Pegasus Helmet (M,L)"]);
    // The pieces are part of what places the items: the stored costs are
    // stale until rebuilt, and the rebuild bills the helmet at the set's
    // markup.
    let roles = KnownRoles::read(&conn, None).unwrap();
    assert_eq!(
        stored_costs(&conn, "hunt", roles.fingerprint()).unwrap(),
        None
    );
    assert_eq!(
        super::costs::sessions_with_stale_costs(&conn, roles.fingerprint()).unwrap(),
        ["hunt"]
    );
    let after = costs_of(&mut conn, "hunt");
    assert_eq!(measured(&after, CostBucket::Armour), 113 * PEC + 462_000);
    // And the session's resolved armour follows in the same write.
    let resolved = crate::session_costs::resolve(&conn, "hunt").unwrap();
    assert!((resolved.resolved.armour - 1.592).abs() < 1e-9);
    assert_eq!(
        resolved.cost_basis(crate::session_costs::Bucket::Armour),
        crate::session_costs::CostBasis::Measured
    );
}
