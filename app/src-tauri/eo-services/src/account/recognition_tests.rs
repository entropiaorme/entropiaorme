//! The one recogniser over a step: each pattern alone and together, a step
//! that closes more than one way, what is never a candidate, a purchase,
//! and a sale to another player.

use super::amount::{Amount, Pec};
use super::diff::{diff, Step, StepSet};
use super::reading::{Container, Reading};
use super::recognition::{
    bought_and_opened, purchase, recognise_step, sale_to_player, BoughtAndOpened, ItemOut, Looted,
    Recognised, SaleToPlayer, ShrapnelConverted, KEYS_PER_BOX, NANOCUBE,
};
use super::roles::Stacking;
use super::test_support::reading;

const C: Container = Container::Carried;
const A: Container = Container::Auction;

fn stack(_: &str) -> Stacking {
    Stacking::Stack
}

fn step(before: &Reading, after: &Reading) -> StepSet {
    diff(before, after, &stack).expect("a step")
}

fn recognised(before: &Reading, after: &Reading) -> Recognised {
    recognise_step(&step(before, after), before, after).expect("read")
}

fn item(name: &str, units: i64, pec: i64) -> ItemOut {
    ItemOut {
        name: name.to_string(),
        units,
        amount: Amount::whole_pec(Pec(pec)).unwrap(),
    }
}

/// Shrapnel down 100 000 and a new ammo row of the 101 000 the Trade
/// Terminal pays for it, beside the ammo already held.
fn converting() -> (Reading, Reading) {
    (
        reading(
            0.0,
            5_000,
            &[
                ("Shrapnel", C, 100_000, 1_000),
                ("Universal Ammo", C, 50_000, 500),
            ],
        ),
        reading(
            60.0,
            5_000,
            &[
                ("Universal Ammo", C, 50_000, 500),
                ("Universal Ammo", C, 101_000, 1_010),
            ],
        ),
    )
}

#[test]
fn a_step_with_change_no_pattern_explains_is_not_read_whole() {
    let (before, mut after) = converting();
    // Something arrives that no conversion, recycling or sale explains.
    after = reading(
        after.taken_at(),
        5_000,
        &[
            ("Universal Ammo", C, 50_000, 500),
            ("Universal Ammo", C, 101_000, 1_010),
            ("Animal Hide", C, 40, 400),
        ],
    );
    let read = recognised(&before, &after);
    assert!(
        read.shrapnel.is_some(),
        "the conversion is still recognised"
    );
    assert!(!read.whole);
}

#[test]
fn a_shrapnel_conversion_alone_is_read_as_one() {
    let (before, after) = converting();
    assert_eq!(
        recognised(&before, &after),
        Recognised {
            shrapnel: Some(ShrapnelConverted {
                shrapnel_units: 100_000,
                ammo_units: 101_000,
            }),
            shrapnel_presumed: None,
            recycled: vec![],
            sold: vec![],
            whole: true,
        }
    );
}

#[test]
fn shrapnel_leaving_without_the_terminal_s_ammo_row_is_no_conversion() {
    let before = reading(0.0, 5_000, &[("Shrapnel", C, 100_000, 1_000)]);
    let after = reading(60.0, 5_000, &[("Universal Ammo", C, 100_000, 1_000)]);
    assert_eq!(recognised(&before, &after).shrapnel, None);
}

#[test]
fn shrapnel_whose_ammo_was_stacked_onto_a_row_is_a_conversion_read_whole() {
    let before = reading(
        0.0,
        5_000,
        &[
            ("Shrapnel", C, 100_000, 1_000),
            ("Universal Ammo", C, 50_000, 500),
        ],
    );
    let after = reading(60.0, 5_000, &[("Universal Ammo", C, 151_000, 1_510)]);
    assert_eq!(
        recognised(&before, &after),
        Recognised {
            shrapnel: Some(ShrapnelConverted {
                shrapnel_units: 100_000,
                ammo_units: 101_000,
            }),
            shrapnel_presumed: None,
            recycled: vec![],
            sold: vec![],
            whole: true,
        }
    );
    // A row grown by any other amount is no conversion.
    let fired = reading(60.0, 5_000, &[("Universal Ammo", C, 150_999, 1_509)]);
    assert_eq!(recognised(&before, &fired).shrapnel, None);
}

#[test]
fn shrapnel_leaving_as_ammo_arrives_short_of_what_it_pays_is_presumed_converted() {
    let before = reading(
        0.0,
        5_000,
        &[
            ("Shrapnel", C, 100_000, 1_000),
            ("Universal Ammo", C, 50_000, 500),
        ],
    );
    // Converted in two goes, each rounded down: a unit short of 101 000.
    let after = reading(60.0, 5_000, &[("Universal Ammo", C, 150_999, 1_509)]);
    let read = recognised(&before, &after);
    assert_eq!(read.shrapnel, None);
    assert_eq!(read.shrapnel_presumed, Some(100_000));
    assert!(!read.whole, "nothing closed exactly");
    // With no ammo arriving, nothing is presumed.
    let gone = reading(60.0, 5_000, &[("Universal Ammo", C, 50_000, 500)]);
    assert_eq!(recognised(&before, &gone).shrapnel_presumed, None);
    // A sale to the Trade Terminal takes it first, ammo arriving or not.
    let sold = reading(60.0, 6_000, &[("Universal Ammo", C, 60_000, 600)]);
    let read = recognised(&before, &sold);
    assert_eq!(read.sold.len(), 1);
    assert_eq!(
        (read.sold[0].name.as_str(), read.sold[0].units),
        ("Shrapnel", 100_000)
    );
    assert_eq!(read.shrapnel_presumed, None);
}

#[test]
fn a_stacked_conversion_and_a_recycling_in_one_step_are_both_read() {
    // Part of the Shrapnel held is converted onto the ammo row, and an
    // item is recycled, between two readings.
    let before = reading(
        0.0,
        5_000,
        &[
            ("Shrapnel", C, 31_731_376, 317_313),
            ("Universal Ammo", C, 4_703_156, 47_031),
            ("Skeleton Pants Red (M)", C, 1, 1_000),
            (NANOCUBE, C, 133_561, 133_561),
        ],
    );
    let after = reading(
        60.0,
        5_000,
        &[
            ("Shrapnel", C, 30_000_000, 300_000),
            ("Universal Ammo", C, 6_451_845, 64_518),
            (NANOCUBE, C, 134_561, 134_561),
        ],
    );
    let read = recognised(&before, &after);
    assert_eq!(
        read.shrapnel,
        Some(ShrapnelConverted {
            shrapnel_units: 1_731_376,
            ammo_units: 1_748_689,
        })
    );
    assert_eq!(
        read.recycled,
        vec![item("Skeleton Pants Red (M)", 1, 1_000)]
    );
    assert!(read.whole);
}

#[test]
fn a_recycling_alone_is_read_as_one() {
    let before = reading(0.0, 5_000, &[("Animal Oil Residue", C, 1_000, 1_000)]);
    let after = reading(60.0, 5_000, &[(NANOCUBE, C, 1_000, 1_000)]);
    let found = recognised(&before, &after);
    assert_eq!(
        found.recycled,
        vec![item("Animal Oil Residue", 1_000, 1_000)]
    );
    assert!(found.sold.is_empty());
}

#[test]
fn nanocubes_short_of_what_left_by_more_than_a_pec_a_row_are_no_recycling() {
    let before = reading(0.0, 5_000, &[("Animal Oil Residue", C, 1_000, 1_000)]);
    let after = reading(60.0, 5_000, &[(NANOCUBE, C, 900, 900)]);
    assert!(recognised(&before, &after).recycled.is_empty());
}

#[test]
fn a_sale_to_the_trade_terminal_alone_is_read_as_one() {
    let before = reading(0.0, 5_000, &[("Animal Oil Residue", C, 1_000, 1_000)]);
    let after = reading(60.0, 6_000, &[]);
    let found = recognised(&before, &after);
    assert_eq!(found.sold, vec![item("Animal Oil Residue", 1_000, 1_000)]);
    assert!(found.recycled.is_empty());
}

#[test]
fn a_sale_reads_only_what_the_card_s_rise_pays_and_leaves_the_rest() {
    // A healing pack used up beside the sale is not sold.
    let before = reading(
        0.0,
        5_000,
        &[
            ("Animal Oil Residue", C, 1_000, 1_000),
            ("Vivo T1", C, 10, 37),
        ],
    );
    let after = reading(60.0, 6_000, &[("Vivo T1", C, 4, 15)]);
    assert_eq!(
        recognised(&before, &after).sold,
        vec![item("Animal Oil Residue", 1_000, 1_000)]
    );
}

#[test]
fn every_pattern_in_one_step_is_read_together() {
    let before = reading(
        0.0,
        5_000,
        &[
            ("Shrapnel", C, 100_000, 1_000),
            ("Universal Ammo", C, 50_000, 500),
            ("Animal Oil Residue", C, 1_000, 1_000),
            ("Iron Stone", C, 100, 1_300),
        ],
    );
    let after = reading(
        60.0,
        6_000,
        &[
            ("Universal Ammo", C, 50_000, 500),
            ("Universal Ammo", C, 101_000, 1_010),
            (NANOCUBE, C, 1_300, 1_300),
        ],
    );
    let found = recognised(&before, &after);
    assert_eq!(
        found.shrapnel,
        Some(ShrapnelConverted {
            shrapnel_units: 100_000,
            ammo_units: 101_000,
        })
    );
    assert_eq!(found.recycled, vec![item("Iron Stone", 100, 1_300)]);
    assert_eq!(found.sold, vec![item("Animal Oil Residue", 1_000, 1_000)]);
}

#[test]
fn a_step_that_closes_more_than_one_way_reads_only_what_every_way_agrees_on() {
    // Two items of the same TT: either may be the one sold and the other
    // the one recycled. Only the third, sold in every way, is read.
    let before = reading(
        0.0,
        5_000,
        &[
            ("Lysterium Stone", C, 50, 500),
            ("Iron Stone", C, 40, 500),
            ("Zinc Stone", C, 30, 300),
        ],
    );
    let after = reading(60.0, 5_800, &[(NANOCUBE, C, 500, 500)]);
    let found = recognised(&before, &after);
    assert_eq!(found.sold, vec![item("Zinc Stone", 30, 300)]);
    assert!(found.recycled.is_empty());
}

#[test]
fn a_card_rise_no_set_of_what_left_explains_is_no_sale_and_leaves_a_recycling_standing() {
    let before = reading(
        0.0,
        5_000,
        &[
            ("Animal Oil Residue", C, 1_000, 1_000),
            ("Iron Stone", C, 100, 1_300),
        ],
    );
    // The Card rose by an amount nothing that left adds up to.
    let after = reading(
        60.0,
        5_777,
        &[
            (NANOCUBE, C, 1_300, 1_300),
            ("Animal Oil Residue", C, 1_000, 1_000),
        ],
    );
    let found = recognised(&before, &after);
    assert!(found.sold.is_empty());
    assert_eq!(found.recycled, vec![item("Iron Stone", 100, 1_300)]);
}

#[test]
fn what_leaves_the_auction_is_never_sold_or_recycled_here() {
    let before = reading(0.0, 5_000, &[("Animal Oil Residue", A, 1_000, 1_000)]);
    let after = reading(60.0, 6_000, &[]);
    assert_eq!(recognised(&before, &after), Recognised::default());
}

#[test]
fn a_step_holding_an_opening_records_no_sale_or_recycling() {
    let before = reading(
        0.0,
        5_000,
        &[
            ("Strongbox Key", C, 5, 5_000),
            ("Halloween Strongbox", C, 5, 0),
            ("Animal Oil Residue", C, 1_000, 1_000),
            ("Universal Ammo", C, 10_000, 100),
        ],
    );
    let after = reading(
        60.0,
        6_000,
        &[
            ("Universal Ammo", C, 10_000, 100),
            ("Universal Ammo", C, 500_000, 5_000),
        ],
    );
    let found = recognised(&before, &after);
    assert!(found.sold.is_empty());
    assert!(found.recycled.is_empty());
}

#[test]
fn a_purchase_is_one_item_arriving_for_more_than_its_tt() {
    let before = reading(0.0, 5_000, &[]);
    let after = reading(60.0, 3_800, &[("Garcen Grease", C, 10, 200)]);
    let bought = purchase(&step(&before, &after), &Looted::new())
        .unwrap()
        .expect("a purchase");
    assert_eq!(bought.key.name, "Garcen Grease");
    assert_eq!(bought.units, 10);
    assert_eq!(bought.paid, Pec(1_200));
    assert_eq!(bought.markup.micro_ped(), 10_000_000);
}

#[test]
fn an_arrival_paid_at_its_tt_or_beside_anything_else_is_no_purchase() {
    let before = reading(0.0, 5_000, &[("Iron Stone", C, 10, 130)]);
    // Bought at the Trade Terminal: the Card fell by its TT alone.
    let at_tt = reading(
        60.0,
        4_800,
        &[("Garcen Grease", C, 10, 200), ("Iron Stone", C, 10, 130)],
    );
    assert_eq!(
        purchase(&step(&before, &at_tt), &Looted::new()).unwrap(),
        None
    );
    // Two items arrived: what each cost is not known.
    let two = reading(
        60.0,
        3_000,
        &[
            ("Garcen Grease", C, 10, 200),
            ("Iron Stone", C, 10, 130),
            ("Zinc Stone", C, 5, 50),
        ],
    );
    assert_eq!(
        purchase(&step(&before, &two), &Looted::new()).unwrap(),
        None
    );
    // Something left beside it.
    let beside = reading(60.0, 3_000, &[("Garcen Grease", C, 10, 200)]);
    assert_eq!(
        purchase(&step(&before, &beside), &Looted::new()).unwrap(),
        None
    );
    // Universal Ammo is never bought at auction.
    let ammo = reading(
        60.0,
        3_000,
        &[
            ("Iron Stone", C, 10, 130),
            ("Universal Ammo", C, 10_000, 100),
        ],
    );
    assert_eq!(
        purchase(&step(&before, &ammo), &Looted::new()).unwrap(),
        None
    );
}

/// Armour, one piece to a row, wears in place; everything else stacks.
fn armour_singly(name: &str) -> Stacking {
    if name.ends_with("(M)") {
        Stacking::Single
    } else {
        Stacking::Stack
    }
}

/// Loot on active lines, `entries` of `(item, units)`.
fn looted(entries: &[(&str, i64)]) -> Looted {
    let lines: std::collections::BTreeMap<String, i64> = entries
        .iter()
        .map(|(name, units)| ((*name).to_string(), *units))
        .collect();
    Looted {
        active: lines.clone(),
        every: lines,
    }
}

/// Loot on deactivated lines alone.
fn deactivated(entries: &[(&str, i64)]) -> Looted {
    Looted {
        active: Default::default(),
        every: looted(entries).every,
    }
}

const ARMOUR: &str = "Hyperion Harness, Adjusted (M)";

#[test]
fn wear_beside_a_purchase_is_set_aside() {
    let before = reading(0.0, 5_000, &[(ARMOUR, C, 1, 13_690)]);
    let after = reading(
        60.0,
        3_800,
        &[(ARMOUR, C, 1, 13_687), ("Garcen Grease", C, 10, 200)],
    );
    let set = diff(&before, &after, &armour_singly).expect("a step");
    assert!(set
        .steps
        .iter()
        .any(|step| matches!(step, Step::Decayed { .. })));
    let bought = purchase(&set, &Looted::new())
        .unwrap()
        .expect("a purchase beside wear");
    assert_eq!(bought.key.name, "Garcen Grease");
    assert_eq!(bought.paid, Pec(1_200));
}

#[test]
fn an_arrival_the_chat_log_looted_is_set_aside_and_one_it_only_partly_explains_is_not() {
    let before = reading(0.0, 5_000, &[("Shrapnel", C, 10_000, 100)]);
    let after = reading(
        60.0,
        3_800,
        &[("Shrapnel", C, 11_700, 117), ("Garcen Grease", C, 10, 200)],
    );
    let set = step(&before, &after);
    // Unexplained, two items arrived: there is no one price.
    assert_eq!(purchase(&set, &Looted::new()).unwrap(), None);
    // The chat log looted every unit that arrived, or more.
    for units in [1_700, 2_000] {
        let bought = purchase(&set, &looted(&[("Shrapnel", units)]))
            .unwrap()
            .expect("the loot set aside");
        assert_eq!(bought.key.name, "Garcen Grease");
        assert_eq!(bought.units, 10);
        assert_eq!(bought.amount.micro_ped(), 2_000_000);
        assert_eq!(bought.paid, Pec(1_200));
    }
    // Only part of it: it stays, and the step has no one price.
    assert_eq!(
        purchase(&set, &looted(&[("Shrapnel", 1_699)])).unwrap(),
        None
    );
}

#[test]
fn an_item_the_chat_log_looted_any_of_is_never_the_purchase() {
    let before = reading(0.0, 5_000, &[]);
    let after = reading(60.0, 3_800, &[("Garcen Grease", C, 10, 200)]);
    let set = step(&before, &after);
    assert!(purchase(&set, &Looted::new()).unwrap().is_some());
    assert!(purchase(&set, &looted(&[("Garcen Grease", 0)]))
        .unwrap()
        .is_some());
    // Some of it looted: what was bought is not known.
    assert_eq!(
        purchase(&set, &looted(&[("Garcen Grease", 4)])).unwrap(),
        None
    );
    // All of it looted: nothing is left to have been bought.
    assert_eq!(
        purchase(&set, &looted(&[("Garcen Grease", 10)])).unwrap(),
        None
    );
}

#[test]
fn deactivated_loot_explains_no_arrival_and_still_marks_its_item_never_bought() {
    let before = reading(0.0, 5_000, &[("Shrapnel", C, 10_000, 100)]);
    let after = reading(
        60.0,
        3_800,
        &[("Shrapnel", C, 11_700, 117), ("Garcen Grease", C, 10, 200)],
    );
    let set = step(&before, &after);
    assert!(purchase(&set, &looted(&[("Shrapnel", 1_700)]))
        .unwrap()
        .is_some());
    // Lines the player deactivated explain nothing.
    assert_eq!(
        purchase(&set, &deactivated(&[("Shrapnel", 1_700)])).unwrap(),
        None
    );
    // And any line of the item itself, deactivated or not, keeps it from
    // being the purchase.
    let mut both = looted(&[("Shrapnel", 1_700)]);
    both.every.insert("Garcen Grease".to_string(), 1);
    assert_eq!(purchase(&set, &both).unwrap(), None);
}

#[test]
fn a_purchase_is_beyond_its_tt_by_more_than_a_pec_for_every_row() {
    let before = reading(0.0, 5_000, &[]);
    let one = |balance: i64| reading(60.0, balance, &[("Garcen Grease", C, 10, 200)]);
    // A PEC a row is what the site may hide: no markup is known.
    assert_eq!(
        purchase(&step(&before, &one(4_799)), &Looted::new()).unwrap(),
        None
    );
    assert!(purchase(&step(&before, &one(4_798)), &Looted::new())
        .unwrap()
        .is_some());
    // Three rows, three PEC: every row counts, across rows.
    let three = |balance: i64| {
        reading(
            60.0,
            balance,
            &[
                ("Garcen Grease", C, 5, 100),
                ("Garcen Grease", C, 3, 60),
                ("Garcen Grease", C, 2, 40),
            ],
        )
    };
    assert_eq!(
        purchase(&step(&before, &three(4_797)), &Looted::new()).unwrap(),
        None
    );
    assert!(purchase(&step(&before, &three(4_796)), &Looted::new())
        .unwrap()
        .is_some());
}

#[test]
fn an_arrival_on_the_auction_is_a_listing_and_never_a_purchase() {
    let before = reading(0.0, 5_000, &[]);
    let after = reading(60.0, 3_800, &[("Garcen Grease", A, 10, 200)]);
    assert_eq!(
        purchase(&step(&before, &after), &Looted::new()).unwrap(),
        None
    );
}

#[test]
fn several_rows_and_places_of_one_item_are_one_purchase() {
    let storage = Container::Storage {
        planet: "Calypso".to_string(),
    };
    let before = reading(0.0, 70_000, &[("Unreal Strongbox", C, 496, 0)]);
    let after = reading(
        60.0,
        9_200,
        &[
            ("Unreal Strongbox", C, 5, 0),
            ("Unreal Strongbox", C, 71, 0),
            ("Unreal Strongbox", C, 496, 0),
            ("Unreal Strongbox", storage, 4, 0),
        ],
    );
    let bought = purchase(&step(&before, &after), &Looted::new())
        .unwrap()
        .expect("one purchase");
    assert_eq!(bought.key.name, "Unreal Strongbox");
    assert_eq!(bought.key.container, C, "where the most of it arrived");
    assert_eq!(bought.units, 80);
    assert_eq!(bought.amount.micro_ped(), 0);
    assert_eq!(bought.paid, Pec(60_800));
    assert_eq!(bought.markup.micro_ped(), 608_000_000);
}

type Rows = Vec<(&'static str, Container, i64, i64)>;

#[test]
fn anything_unexplained_beside_a_purchase_refuses_it() {
    let armour = (ARMOUR, C, 1, 13_690);
    let iron = ("Iron Stone", C, 10, 130);
    let ammo = ("Universal Ammo", C, 1_000, 10);
    let keys = ("Strongbox Key", C, 2, 200);
    let boxes = ("Unreal Strongbox", C, 2, 0);
    let bought = ("Garcen Grease", C, 10, 200);
    let held: Rows = vec![
        armour.clone(),
        iron.clone(),
        ammo.clone(),
        keys.clone(),
        boxes.clone(),
    ];
    let before = reading(0.0, 5_000, &held);
    let read = |after: &Rows| {
        let after = reading(60.0, 3_800, after);
        purchase(
            &diff(&before, &after, &armour_singly).expect("a step"),
            &Looted::new(),
        )
        .unwrap()
    };
    // With everything else held as it was, it is read.
    let mut alone = held.clone();
    alone.push(bought.clone());
    assert!(read(&alone).is_some());
    let cases: [(&str, Rows); 6] = [
        (
            "a repair",
            vec![
                (ARMOUR, C, 1, 13_700),
                iron.clone(),
                ammo.clone(),
                keys.clone(),
                boxes.clone(),
                bought.clone(),
            ],
        ),
        (
            "a listing",
            vec![
                armour.clone(),
                ("Iron Stone", A, 10, 130),
                ammo.clone(),
                keys.clone(),
                boxes.clone(),
                bought.clone(),
            ],
        ),
        (
            "Universal Ammo spent",
            vec![
                armour.clone(),
                iron.clone(),
                ("Universal Ammo", C, 900, 9),
                keys.clone(),
                boxes.clone(),
                bought.clone(),
            ],
        ),
        (
            "a stack used up",
            vec![
                armour.clone(),
                ammo.clone(),
                keys.clone(),
                boxes.clone(),
                bought.clone(),
            ],
        ),
        (
            "another arrival",
            vec![
                armour.clone(),
                iron.clone(),
                ammo.clone(),
                keys.clone(),
                boxes.clone(),
                bought.clone(),
                ("Zinc Stone", C, 5, 50),
            ],
        ),
        (
            "a strongbox opening",
            vec![armour.clone(), iron.clone(), ammo.clone(), bought.clone()],
        ),
    ];
    for (beside, after) in cases {
        assert_eq!(read(&after), None, "{beside} beside a purchase");
    }
}

/// `count` items, each named by its index, one row each, worth `value`
/// PEC for the index it is given.
fn bag(count: usize, value: impl Fn(usize) -> i64) -> Vec<(String, i64)> {
    (0..count)
        .map(|index| (format!("Stone {index:02}"), value(index)))
        .collect()
}

fn rows(items: &[(String, i64)]) -> Vec<(&str, Container, i64, i64)> {
    items
        .iter()
        .map(|(name, value)| (name.as_str(), C, 1, *value))
        .collect()
}

#[test]
fn a_trade_terminal_emptied_of_a_whole_bag_is_read_as_sold() {
    // Twenty-five items of 10 PED, and one worth a PEC that the slack
    // cannot tell sold from kept.
    let mut items = bag(25, |_| 1_000);
    items.push(("Tiny Stone".to_string(), 1));
    let before = reading(0.0, 5_000, &rows(&items));
    let after = reading(60.0, 5_000 + 25_001, &[]);
    let found = recognised(&before, &after);
    assert_eq!(found.sold.len(), 25);
    assert!(found
        .sold
        .iter()
        .all(|item| item.name.starts_with("Stone ")));
    assert!(found.recycled.is_empty());
}

#[test]
fn a_bag_the_card_s_rise_does_not_close_whole_is_weighed_set_by_set() {
    // Two items, the Card paying for one: the whole bag does not close, so
    // the sets are weighed.
    let before = reading(
        0.0,
        5_000,
        &[("Lysterium Stone", C, 50, 500), ("Iron Stone", C, 40, 300)],
    );
    let after = reading(60.0, 5_500, &[]);
    assert_eq!(
        recognised(&before, &after).sold,
        vec![item("Lysterium Stone", 50, 500)]
    );
}

/// Items worth 1, 2, 4, ... PED, so no two sets of them add up within the
/// tolerance of each other: the Card pays for the first, the Nanocubes for
/// the second.
fn weighed(count: usize) -> Recognised {
    let items = bag(count, |index| 100 << index);
    let before = reading(0.0, 5_000, &rows(&items));
    let after = reading(60.0, 5_100, &[(NANOCUBE, C, 200, 200)]);
    recognised(&before, &after)
}

#[test]
fn the_weighing_s_cap_takes_sixteen_items_and_refuses_seventeen() {
    assert_eq!(super::composite::MAX_BALANCE_EVALUATIONS, 100_000);
    let sixteen = weighed(16);
    assert_eq!(sixteen.sold, vec![item("Stone 00", 1, 100)]);
    assert_eq!(sixteen.recycled, vec![item("Stone 01", 1, 200)]);
    let seventeen = weighed(17);
    assert!(seventeen.sold.is_empty());
    assert!(seventeen.recycled.is_empty());
}

#[test]
fn more_items_than_a_mask_holds_are_refused_without_weighing() {
    let items = bag(40, |_| 100);
    let before = reading(0.0, 5_000, &rows(&items));
    let after = reading(60.0, 5_100, &[(NANOCUBE, C, 100, 100)]);
    assert_eq!(recognised(&before, &after), Recognised::default());
}

// ── A sale to another player ────────────────────────────────────────

fn sale(before: &Reading, after: &Reading, looted: &Looted) -> Option<SaleToPlayer> {
    let set = diff(before, after, &armour_singly).expect("a step");
    sale_to_player(&set, before, after, looted).unwrap()
}

#[test]
fn a_sale_to_a_player_is_one_item_leaving_for_more_than_its_tt_beside_wear() {
    let before = reading(
        0.0,
        5_000,
        &[("Garcen Grease", C, 10, 200), (ARMOUR, C, 1, 13_690)],
    );
    let after = reading(60.0, 6_200, &[(ARMOUR, C, 1, 13_687)]);
    let sold = sale(&before, &after, &Looted::new()).expect("a sale");
    assert_eq!(sold.key.name, "Garcen Grease");
    assert_eq!(sold.key.container, C);
    assert_eq!(sold.units, 10);
    assert_eq!(sold.amount.micro_ped(), 2_000_000);
    assert_eq!(sold.received, Pec(1_200));
    assert_eq!(sold.markup.micro_ped(), 10_000_000);
}

#[test]
fn a_rise_of_its_tt_is_the_trade_terminal_s_sale_and_never_a_sale_to_a_player() {
    let before = reading(0.0, 5_000, &[("Garcen Grease", C, 10, 200)]);
    let at = |balance: i64| reading(60.0, balance, &[]);
    for balance in [5_200, 5_201] {
        assert_eq!(sale(&before, &at(balance), &Looted::new()), None);
        assert_eq!(
            recognised(&before, &at(balance)).sold,
            vec![item("Garcen Grease", 10, 200)],
            "the Trade Terminal's"
        );
    }
    // A PEC beyond what the row may hide: no longer the Trade Terminal's.
    assert!(recognised(&before, &at(5_202)).sold.is_empty());
    assert_eq!(
        sale(&before, &at(5_202), &Looted::new()).map(|sold| sold.received),
        Some(Pec(202))
    );
}

#[test]
fn several_rows_and_places_of_one_item_leaving_are_one_sale() {
    let storage = Container::Storage {
        planet: "Calypso".to_string(),
    };
    let before = reading(
        0.0,
        5_000,
        &[
            ("Garcen Grease", C, 10, 200),
            ("Garcen Grease", C, 5, 100),
            ("Garcen Grease", storage, 2, 40),
        ],
    );
    let after = reading(60.0, 9_000, &[]);
    let sold = sale(&before, &after, &Looted::new()).expect("one sale");
    assert_eq!(sold.units, 17);
    assert_eq!(sold.key.container, C, "where the most of it left");
    assert_eq!(sold.received, Pec(4_000));
}

#[test]
fn keys_leaving_alone_may_be_sold_and_boxes_leaving_are_an_opening_s() {
    let before = reading(
        0.0,
        5_000,
        &[("Strongbox Key", C, 2, 200), ("Unreal Strongbox", C, 2, 0)],
    );
    let keys_gone = reading(60.0, 6_000, &[("Unreal Strongbox", C, 2, 0)]);
    assert_eq!(
        sale(&before, &keys_gone, &Looted::new()).map(|sold| sold.key.name),
        Some("Strongbox Key".to_string())
    );
    let boxes_gone = reading(60.0, 6_000, &[("Strongbox Key", C, 2, 200)]);
    assert_eq!(sale(&before, &boxes_gone, &Looted::new()), None);
}

#[test]
fn loot_beside_a_sale_is_set_aside_only_when_the_chat_log_explains_it_all() {
    let before = reading(
        0.0,
        5_000,
        &[("Garcen Grease", C, 10, 200), ("Shrapnel", C, 10_000, 100)],
    );
    let after = reading(60.0, 6_200, &[("Shrapnel", C, 11_700, 117)]);
    assert_eq!(sale(&before, &after, &Looted::new()), None);
    assert!(sale(&before, &after, &looted(&[("Shrapnel", 1_700)])).is_some());
    assert_eq!(sale(&before, &after, &looted(&[("Shrapnel", 1_699)])), None);
    // Loot of the item sold lands on the stack it left from, hiding how
    // much left.
    assert_eq!(
        sale(
            &before,
            &after,
            &looted(&[("Shrapnel", 1_700), ("Garcen Grease", 1)])
        ),
        None
    );
}

#[test]
fn anything_unexplained_beside_a_sale_refuses_it() {
    let armour = (ARMOUR, C, 1, 13_690);
    let iron = ("Iron Stone", C, 10, 130);
    let ammo = ("Universal Ammo", C, 1_000, 10);
    let grease = ("Garcen Grease", C, 10, 200);
    let listed = ("Lysterium Stone", A, 5, 50);
    let held: Rows = vec![
        armour.clone(),
        iron.clone(),
        ammo.clone(),
        grease.clone(),
        listed.clone(),
    ];
    let before = reading(0.0, 5_000, &held);
    let read = |balance: i64, after: &Rows| {
        let after = reading(60.0, balance, after);
        sale(&before, &after, &Looted::new())
    };
    // The grease gone, everything else as it was: it is read.
    let without = |name: &str| -> Rows {
        held.iter()
            .filter(|(held, ..)| *held != name)
            .cloned()
            .collect()
    };
    let sold = without("Garcen Grease");
    assert!(read(6_200, &sold).is_some());
    let with = |extra: (&'static str, Container, i64, i64)| -> Rows {
        let mut rows = sold.clone();
        rows.retain(|(name, container, ..)| !(*name == extra.0 && *container == extra.1));
        rows.push(extra);
        rows
    };
    let cases: [(&str, i64, Rows); 9] = [
        ("an arrival", 6_200, with(("Zinc Stone", C, 5, 50))),
        (
            "a second item leaving",
            6_200,
            without("Garcen Grease")
                .into_iter()
                .filter(|(name, ..)| *name != "Iron Stone")
                .collect(),
        ),
        ("a repair", 6_200, with((ARMOUR, C, 1, 13_700))),
        (
            "Universal Ammo spent",
            6_200,
            with(("Universal Ammo", C, 900, 9)),
        ),
        ("a listing", 6_200, {
            let mut rows = with(("Iron Stone", A, 10, 130));
            rows.retain(|row| *row != iron);
            rows
        }),
        ("a listing's outcome", 6_200, {
            let mut rows = sold.clone();
            rows.retain(|row| *row != listed);
            rows
        }),
        ("the Card falling", 4_000, sold.clone()),
        ("the Card still", 5_000, sold.clone()),
        ("Universal Ammo alone leaving", 6_200, {
            let mut rows = held.clone();
            rows.retain(|row| *row != ammo);
            rows
        }),
    ];
    for (beside, balance, after) in cases {
        assert_eq!(read(balance, &after), None, "{beside} beside a sale");
    }
    // What leaves the auction alone is a listing's outcome, never this.
    let mut auction_alone = held.clone();
    auction_alone.retain(|row| *row != listed);
    assert_eq!(read(6_200, &auction_alone), None);
}

// ── Boxes bought and opened in one step ─────────────────────────────

const BOX: &str = "Unreal Strongbox";
const KEY: &str = "Strongbox Key";

/// 572 boxes, 68 keys and a row of ammo held, as the step began.
fn boxes_held() -> Rows {
    vec![
        (BOX, C, 572, 0),
        (KEY, C, 68, 68_000),
        ("Universal Ammo", C, 6_167_301, 61_673),
        (ARMOUR, C, 1, 13_690),
    ]
}

/// What the step ends with: `boxes` boxes, `keys` keys, six new ammo rows
/// of a key's TT each, and `extra` beside them.
fn boxes_after(boxes: i64, keys: i64, extra: &[(&'static str, Container, i64, i64)]) -> Rows {
    let mut rows: Rows = vec![
        (BOX, C, boxes, 0),
        (KEY, C, keys, keys * 1_000),
        ("Universal Ammo", C, 6_167_301, 61_673),
        (ARMOUR, C, 1, 13_690),
    ];
    rows.extend(std::iter::repeat_n(
        ("Universal Ammo", C, 100_000, 1_000),
        6,
    ));
    rows.extend(extra.iter().cloned());
    rows
}

fn both(before: &Rows, balance: i64, after: &Rows, looted: &Looted) -> Option<BoughtAndOpened> {
    let before = reading(0.0, 217_872, before);
    let after = reading(200.0, balance, after);
    let set = diff(&before, &after, &armour_singly).expect("a step");
    bought_and_opened(&set, &before, &after, &armour_singly, looted).unwrap()
}

#[test]
fn boxes_bought_and_opened_are_the_boxes_opened_beyond_those_that_left_at_the_card_s_fall() {
    assert_eq!(KEYS_PER_BOX, 1);
    let read = both(
        &boxes_held(),
        186_672,
        &boxes_after(605, 62, &[]),
        &Looted::new(),
    )
    .expect("bought and opened");
    assert_eq!(read.opened, 6);
    assert_eq!(read.purchase.key.name, BOX);
    assert_eq!(read.purchase.units, 39);
    assert_eq!(read.purchase.amount.micro_ped(), 0);
    assert_eq!(read.purchase.paid, Pec(31_200));
    assert_eq!(read.purchase.markup.micro_ped(), 312_000_000);
    // Had the purchase come first: the boxes beside those held, the Card
    // already down by their cost.
    let boxes = read
        .before
        .holding(&super::reading::HoldingKey {
            name: BOX.to_string(),
            container: C,
        })
        .expect("the boxes");
    assert_eq!(boxes.quantity(), 611);
    assert_eq!(read.before.balance(), Pec(186_672));
    assert_eq!(read.before.taken_at(), 0.0);
}

#[test]
fn boxes_bought_cost_more_than_the_pec_a_row_the_site_may_hide() {
    let before = reading(0.0, 217_872, &boxes_held());
    let at = |balance: i64| reading(200.0, balance, &boxes_after(605, 62, &[]));
    let rows = diff(&before, &at(217_000), &armour_singly)
        .expect("a step")
        .steps
        .iter()
        .find_map(|step| match step {
            Step::Gained {
                key, rows_touched, ..
            } if key.name == BOX => Some((*rows_touched).max(1) as i64),
            _ => None,
        })
        .expect("the boxes rose");
    let read = |fall: i64| {
        let after = at(217_872 - fall);
        let set = diff(&before, &after, &armour_singly).expect("a step");
        bought_and_opened(&set, &before, &after, &armour_singly, &Looted::new()).unwrap()
    };
    assert_eq!(read(rows), None);
    assert_eq!(
        read(rows + 1).map(|both| both.purchase.paid),
        Some(Pec(rows + 1))
    );
}

#[test]
fn boxes_that_fell_by_fewer_than_the_keys_open_were_bought_too() {
    let read = both(
        &boxes_held(),
        201_872,
        &boxes_after(570, 62, &[]),
        &Looted::new(),
    )
    .expect("bought and opened");
    assert_eq!(read.opened, 6);
    assert_eq!(read.purchase.units, 4);
}

#[test]
fn an_ordinary_opening_or_anything_beside_it_is_never_read_as_boxes_bought() {
    let held = boxes_held();
    let none = |balance: i64, after: Rows| both(&held, balance, &after, &Looted::new());
    // Wear is set aside, and so is a move between the player's own places.
    let mut worn = boxes_after(605, 62, &[]);
    worn.retain(|row| row.0 != ARMOUR);
    worn.push((ARMOUR, C, 1, 13_680));
    assert!(none(186_672, worn.clone()).is_some());
    let mut iron_held = held.clone();
    iron_held.push(("Iron Stone", C, 10, 130));
    let mut iron_moved = boxes_after(605, 62, &[]);
    iron_moved.push((
        "Iron Stone",
        Container::Storage {
            planet: "Calypso".to_string(),
        },
        10,
        130,
    ));
    assert!(both(&iron_held, 186_672, &iron_moved, &Looted::new()).is_some());
    let cases: [(&str, i64, Rows); 14] = [
        (
            "an arrival the opening does not explain",
            186_672,
            boxes_after(605, 62, &[("Animal Oil Residue", C, 100, 100)]),
        ),
        (
            "more Universal Ammo than the opening paid",
            186_672,
            boxes_after(605, 62, &[("Universal Ammo", C, 500_000, 5_000)]),
        ),
        (
            "as many boxes bought as opened: no box changed",
            186_672,
            boxes_after(572, 62, &[]),
        ),
        ("keys arriving", 186_672, {
            let mut rows = boxes_after(605, 68, &[]);
            rows.retain(|row| row.0 != KEY);
            rows.push((KEY, C, 62, 62_000));
            rows.push((
                KEY,
                Container::Storage {
                    planet: "Calypso".to_string(),
                },
                8,
                8_000,
            ));
            rows
        }),
        (
            "the box in two places",
            186_672,
            boxes_after(
                605,
                62,
                &[(
                    BOX,
                    Container::Storage {
                        planet: "Calypso".to_string(),
                    },
                    3,
                    0,
                )],
            ),
        ),
        (
            "as many boxes left as keys used",
            217_000,
            boxes_after(566, 62, &[]),
        ),
        (
            "more boxes left than keys used",
            217_000,
            boxes_after(565, 62, &[]),
        ),
        ("the Card still", 217_872, boxes_after(605, 62, &[])),
        ("the Card rising", 220_000, boxes_after(605, 62, &[])),
        (
            "a second box changing",
            186_672,
            boxes_after(605, 62, &[("Halloween Strongbox", C, 3, 0)]),
        ),
        ("no keys used", 186_672, boxes_after(605, 68, &[])),
        ("a repair", 186_672, {
            let mut rows = boxes_after(605, 62, &[]);
            rows.retain(|row| row.0 != ARMOUR);
            rows.push((ARMOUR, C, 1, 13_700));
            rows
        }),
        ("Universal Ammo used", 186_672, {
            let mut rows = boxes_after(605, 62, &[]);
            rows.retain(|row| row.2 != 6_167_301);
            rows.push(("Universal Ammo", C, 5_000_000, 50_000));
            rows
        }),
        ("a listing", 186_672, {
            let mut rows = boxes_after(605, 62, &[]);
            rows.retain(|row| row.0 != ARMOUR);
            rows.push((ARMOUR, A, 1, 13_690));
            rows
        }),
    ];
    for (beside, balance, after) in cases {
        assert_eq!(none(balance, after), None, "{beside}");
    }
    // Loot the chat log explains is set aside; only its active lines do.
    let residue = boxes_after(605, 62, &[("Animal Oil Residue", C, 100, 100)]);
    assert!(both(
        &held,
        186_672,
        &residue,
        &looted(&[("Animal Oil Residue", 100)])
    )
    .is_some());
    assert_eq!(
        both(
            &held,
            186_672,
            &residue,
            &deactivated(&[("Animal Oil Residue", 100)])
        ),
        None
    );
    // The box looted around the step.
    assert_eq!(
        both(
            &held,
            186_672,
            &boxes_after(605, 62, &[]),
            &looted(&[(BOX, 1)])
        ),
        None,
        "a box looted"
    );
}
