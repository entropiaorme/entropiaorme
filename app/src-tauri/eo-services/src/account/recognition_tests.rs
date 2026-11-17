//! The one recogniser over a step: each pattern alone and together, a step
//! that closes more than one way, what is never a candidate, and the
//! purchase at auction.

use super::amount::{Amount, Pec};
use super::diff::{diff, StepSet};
use super::reading::{Container, Reading};
use super::recognition::{
    purchase, recognise_step, ItemOut, Recognised, ShrapnelConverted, NANOCUBE,
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
fn a_shrapnel_conversion_alone_is_read_as_one() {
    let (before, after) = converting();
    assert_eq!(
        recognised(&before, &after),
        Recognised {
            shrapnel: Some(ShrapnelConverted {
                shrapnel_units: 100_000,
                ammo_units: 101_000,
            }),
            recycled: vec![],
            sold: vec![],
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
    let bought = purchase(&step(&before, &after))
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
    assert_eq!(purchase(&step(&before, &at_tt)).unwrap(), None);
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
    assert_eq!(purchase(&step(&before, &two)).unwrap(), None);
    // Something left beside it.
    let beside = reading(60.0, 3_000, &[("Garcen Grease", C, 10, 200)]);
    assert_eq!(purchase(&step(&before, &beside)).unwrap(), None);
    // Universal Ammo is never bought at auction.
    let ammo = reading(
        60.0,
        3_000,
        &[
            ("Iron Stone", C, 10, 130),
            ("Universal Ammo", C, 10_000, 100),
        ],
    );
    assert_eq!(purchase(&step(&before, &ammo)).unwrap(), None);
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
