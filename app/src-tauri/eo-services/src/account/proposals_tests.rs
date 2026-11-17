//! What the readings propose: the recorded hunt's two openings, listings
//! matched or not, the proceeds a sale may carry, examining once, and
//! whether a proposal is still pending as its record moves.

use super::amount::{Amount, Pec};
use super::composite::Settlement;
use std::cell::RefCell;

use super::proposals::{
    claim, commit, detect, dismiss, examine, link, pending, prepare, release, step_outcomes,
    Connections, Decided, Detail, Doubt, ProposalId, ProposalKind, ProposedAmmo, Which,
};
use super::reading::{Container, Reading};
use super::roles::KnownRoles;
use super::store::{record_reading, Purpose, ReadingId};
use super::test_support::{reading, recorded_hunt};
use super::AccountError;
use crate::db::migrated_in_memory;

/// One connection standing for the database's readers and its writer, so
/// the tests run the app's own examination pass.
struct On<'a>(RefCell<&'a mut rusqlite::Connection>);

impl Connections for On<'_> {
    fn read<T: Send + 'static>(
        &self,
        job: impl FnOnce(&rusqlite::Connection) -> Result<T, AccountError> + Send + 'static,
    ) -> Result<T, AccountError> {
        job(&self.0.borrow())
    }

    fn write<T: Send + 'static>(
        &self,
        job: impl FnOnce(&mut rusqlite::Connection) -> Result<T, AccountError> + Send + 'static,
    ) -> Result<T, AccountError> {
        job(&mut self.0.borrow_mut())
    }
}

/// Examine one stored reading through the pass; how many it added.
fn examine_reading(
    conn: &mut rusqlite::Connection,
    reading: ReadingId,
    _roles: &KnownRoles,
    now: f64,
) -> Result<usize, AccountError> {
    Ok(
        examine(&On(RefCell::new(conn)), None, Which::Reading(reading), now)?
            .map_or(0, |examined| examined.added),
    )
}

/// What examining every waiting reading came to.
#[derive(Debug, PartialEq, Eq)]
struct Pass {
    examined: usize,
    passed: usize,
    added: usize,
}

/// Examine every waiting reading through the pass, oldest first.
fn examine_all(conn: &mut rusqlite::Connection) -> Result<Pass, AccountError> {
    let on = On(RefCell::new(conn));
    let mut pass = Pass {
        examined: 0,
        passed: 0,
        added: 0,
    };
    while let Some(examined) = examine(&on, None, Which::Oldest, 1.0)? {
        pass.examined += 1;
        pass.passed += usize::from(examined.passed);
        pass.added += examined.added;
    }
    Ok(pass)
}

const C: Container = Container::Carried;
const A: Container = Container::Auction;
const PED: i64 = 1_000_000;

fn roles() -> KnownRoles {
    KnownRoles::default()
}

fn count(conn: &rusqlite::Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).expect("count")
}

/// Store readings in order, untagged, as a run of readings would be.
fn stored(conn: &mut rusqlite::Connection, readings: &[Reading]) -> Vec<ReadingId> {
    readings
        .iter()
        .map(|reading| {
            record_reading(conn, reading, Purpose::Run, None)
                .expect("stored")
                .id
        })
        .collect()
}

/// A listing on auction since before the readings began.
fn listing(conn: &rusqlite::Connection, id: &str, item: &str, quantity: i64, tt: f64) {
    listed(conn, id, item, quantity, tt, "1970-01-01", Some(-1.0));
}

fn listed(
    conn: &rusqlite::Connection,
    id: &str,
    item: &str,
    quantity: i64,
    tt: f64,
    day: &str,
    instant: Option<f64>,
) {
    conn.execute(
        "INSERT INTO auction_listings (id, item_name, quantity, tt_value, starting_bid, \
             listed_at, listed_instant, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, 1.0, ?5, ?6, 1, 1)",
        rusqlite::params![id, item, quantity, tt, day, instant],
    )
    .expect("listing");
}

#[test]
fn the_opening_before_the_session_is_five_boxes_with_its_fireworks_only_possibly_its() {
    let readings = recorded_hunt();
    let outcomes = step_outcomes(&readings[1], &readings[2], &roles()).unwrap();
    let opening = outcomes.opening.expect("an opening");
    assert_eq!(opening.boxes.len(), 1);
    assert_eq!(opening.boxes[0].name, "Halloween Strongbox");
    assert_eq!(opening.boxes[0].count, 5);
    assert_eq!(opening.keys_used, 5);
    assert_eq!(opening.keys.micro_ped(), 50 * PED);
    assert_eq!(
        opening.ammo,
        Some(ProposedAmmo {
            units: 499_500,
            amount: Amount::exact(49_950_000)
        })
    );
    assert!(opening.contents.is_empty());
    assert_eq!(opening.possible_contents.len(), 1);
    assert_eq!(opening.possible_contents[0].key.name, "Halloween Fireworks");
    assert_eq!(opening.possible_contents[0].amount.micro_ped(), 0);
    assert_eq!(opening.settlement, Settlement::Settled);
    assert_eq!(opening.doubt, None);
    // Four rows of exactly ten PED, and one of 9.95 beside the fireworks.
    assert_eq!(opening.paid_ammo_alone, Some(4));
    assert!(outcomes.departures.is_empty());
}

#[test]
fn the_hunt_s_opening_is_seven_boxes_and_one_item() {
    let readings = recorded_hunt();
    let outcomes = step_outcomes(&readings[8], &readings[9], &roles()).unwrap();
    let opening = outcomes.opening.expect("an opening");
    assert_eq!(opening.boxes[0].count, 7);
    assert_eq!(opening.keys.micro_ped(), 70 * PED);
    assert_eq!(
        opening.ammo.map(|ammo| ammo.amount.micro_ped()),
        Some(60 * PED)
    );
    assert_eq!(opening.contents.len(), 1);
    assert_eq!(opening.contents[0].amount.micro_ped(), 10 * PED);
    assert_eq!(opening.paid_ammo_alone, Some(6));
    // No other step of the hunt holds an opening.
    let openings = readings
        .windows(2)
        .filter(|pair| {
            step_outcomes(&pair[0], &pair[1], &roles())
                .unwrap()
                .opening
                .is_some()
        })
        .count();
    assert_eq!(openings, 2);
}

#[test]
fn an_unsettled_opening_carries_why_and_no_ammo() {
    let before = reading(
        0.0,
        0,
        &[
            ("Halloween Strongbox", C, 23, 0),
            ("Strongbox Key", C, 24, 24_000),
        ],
    );
    let after = reading(
        60.0,
        0,
        &[
            ("Halloween Strongbox", C, 17, 0),
            ("Strongbox Key", C, 17, 17_000),
            ("Skeleton Pants Red (F)", C, 1, 1_000),
        ],
    );
    let opening = step_outcomes(&before, &after, &roles())
        .unwrap()
        .opening
        .expect("still an opening");
    assert_eq!(opening.settlement, Settlement::Unsettled);
    assert_eq!(opening.doubt, Some(Doubt::CountsDiffer));
    assert_eq!(opening.ammo, None);
    assert_eq!(opening.paid_ammo_alone, None);
    assert!(opening.contents.is_empty());
    assert_eq!(opening.possible_contents.len(), 1);

    let no_keys = step_outcomes(
        &reading(0.0, 0, &[("Halloween Strongbox", C, 3, 0)]),
        &reading(60.0, 0, &[("Halloween Strongbox", C, 1, 0)]),
        &roles(),
    )
    .unwrap()
    .opening
    .unwrap();
    assert_eq!(no_keys.doubt, Some(Doubt::NoKeys));
}

/// A stack of 1000 Shrapnel on auction, worth 10 PED, and the Card.
fn on_auction(
    at: f64,
    balance: i64,
    auction: bool,
    extra: &[(&'static str, Container, i64, i64)],
) -> Reading {
    let mut rows: Vec<(&str, Container, i64, i64)> = vec![("Universal Ammo", C, 50_000, 500)];
    if auction {
        rows.push(("Shrapnel", A, 100_000, 1_000));
    }
    rows.extend_from_slice(extra);
    reading(at, balance, &rows)
}

#[test]
fn a_sale_carries_the_card_s_rise_only_when_nothing_else_could_move_it_in_a_short_step() {
    let sold = step_outcomes(
        &on_auction(0.0, 10_000, true, &[]),
        &on_auction(60.0, 11_250, false, &[]),
        &roles(),
    )
    .unwrap();
    assert_eq!(sold.departures.len(), 1);
    let departure = &sold.departures[0];
    assert_eq!(departure.name, "Shrapnel");
    assert_eq!(departure.units, 100_000);
    assert_eq!(departure.returned_to, None);
    assert_eq!(departure.proceeds, Some(Pec(1_250)));

    // Readings more than three minutes apart: no price.
    let long = step_outcomes(
        &on_auction(0.0, 10_000, true, &[]),
        &on_auction(181.0, 11_250, false, &[]),
        &roles(),
    )
    .unwrap();
    assert_eq!(long.departures[0].proceeds, None);

    // Ammo burned beside it might have been sold to the Trade Terminal.
    let busy = step_outcomes(
        &on_auction(0.0, 10_000, true, &[]),
        &reading(60.0, 11_250, &[("Universal Ammo", C, 40_000, 400)]),
        &roles(),
    )
    .unwrap();
    assert_eq!(busy.departures[0].proceeds, None);

    // The Card did not rise.
    let still = step_outcomes(
        &on_auction(0.0, 10_000, true, &[]),
        &on_auction(60.0, 10_000, false, &[]),
        &roles(),
    )
    .unwrap();
    assert_eq!(still.departures[0].proceeds, None);

    // Wear beside it never moves the Card.
    let worn = step_outcomes(
        &on_auction(0.0, 10_000, true, &[("Opalo", C, 1, 500)]),
        &on_auction(60.0, 11_250, false, &[("Opalo", C, 1, 495)]),
        &roles(),
    )
    .unwrap();
    assert_eq!(worn.departures[0].proceeds, Some(Pec(1_250)));
}

#[test]
fn a_stack_moved_out_of_the_auction_is_a_return() {
    let returned = step_outcomes(
        &on_auction(0.0, 10_000, true, &[]),
        &on_auction(60.0, 10_000, false, &[("Shrapnel", C, 100_000, 1_000)]),
        &roles(),
    )
    .unwrap();
    assert_eq!(returned.departures.len(), 1);
    assert_eq!(returned.departures[0].returned_to, Some(Container::Carried));
    assert_eq!(returned.departures[0].proceeds, None);
}

#[test]
fn examining_stores_each_proposal_once_and_marks_the_reading() {
    let mut conn = migrated_in_memory().unwrap();
    let readings = recorded_hunt();
    let ids = stored(&mut conn, &readings);
    let pass = examine_all(&mut conn).unwrap();
    assert_eq!(
        pass,
        Pass {
            examined: 15,
            passed: 0,
            added: 2
        }
    );
    assert_eq!(
        examine_all(&mut conn).unwrap(),
        Pass {
            examined: 0,
            passed: 0,
            added: 0
        }
    );
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM account_readings_examined"),
        15
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM account_proposals"), 2);

    // Again, examined or not: nothing is added twice.
    assert_eq!(
        examine_reading(&mut conn, ids[9], &roles(), 2.0).unwrap(),
        0
    );
    assert_eq!(detect(&mut conn, ids[9], &roles(), 2.0).unwrap(), 0);
    assert_eq!(detect(&mut conn, ids[2], &roles(), 2.0).unwrap(), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM account_proposals"), 2);

    let proposals = pending(&conn, Some(ProposalKind::Opening)).unwrap();
    assert_eq!(proposals.len(), 2);
    // The latest seen first, with the window it happened in.
    assert_eq!(proposals[0].reading, ids[9]);
    assert_eq!(proposals[0].prev_reading, ids[8]);
    assert_eq!(proposals[0].between.from, readings[8].taken_at());
    assert_eq!(proposals[0].between.to, readings[9].taken_at());
    assert_eq!(proposals[0].session_id, None);
    let Detail::Opening(opening) = &proposals[0].detail else {
        panic!("an opening");
    };
    assert_eq!(opening.boxes[0].count, 7);
    assert!(pending(&conn, Some(ProposalKind::ListingSold))
        .unwrap()
        .is_empty());
}

/// Two readings a minute apart, the Shrapnel listing sold between them.
fn sold_step(conn: &mut rusqlite::Connection) -> ReadingId {
    let ids = stored(
        conn,
        &[
            on_auction(0.0, 10_000, true, &[]),
            on_auction(60.0, 11_250, false, &[]),
        ],
    );
    ids[1]
}

#[test]
fn a_listing_is_proposed_only_when_exactly_one_listing_on_auction_matches() {
    let mut conn = migrated_in_memory().unwrap();
    listing(&conn, "match", "shrapnel ", 100_000, 10.0);
    listing(&conn, "other-quantity", "Shrapnel", 90_000, 9.0);
    listing(&conn, "other-tt", "Shrapnel", 100_000, 20.0);
    let reading = sold_step(&mut conn);
    examine_reading(&mut conn, reading, &roles(), 1.0).unwrap();
    let proposals = pending(&conn, None).unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].kind, ProposalKind::ListingSold);
    let Detail::Listing(sold) = &proposals[0].detail else {
        panic!("a listing");
    };
    assert_eq!(sold.listing_id, "match");
    assert_eq!(sold.proceeds, Some(Pec(1_250)));

    // A second listing of the same: ambiguous, so nothing.
    let mut conn = migrated_in_memory().unwrap();
    listing(&conn, "a", "Shrapnel", 100_000, 10.0);
    listing(&conn, "b", "Shrapnel", 100_000, 0.0);
    let reading = sold_step(&mut conn);
    examine_reading(&mut conn, reading, &roles(), 1.0).unwrap();
    assert!(pending(&conn, None).unwrap().is_empty());

    // A listing already resolved is not on auction.
    let mut conn = migrated_in_memory().unwrap();
    listing(&conn, "done", "Shrapnel", 100_000, 10.0);
    conn.execute("UPDATE auction_listings SET status = 'sold'", [])
        .unwrap();
    let reading = sold_step(&mut conn);
    examine_reading(&mut conn, reading, &roles(), 1.0).unwrap();
    assert!(pending(&conn, None).unwrap().is_empty());
}

#[test]
fn a_listing_s_proposal_follows_its_listing() {
    let mut conn = migrated_in_memory().unwrap();
    listing(&conn, "match", "Shrapnel", 100_000, 10.0);
    let reading = sold_step(&mut conn);
    examine_reading(&mut conn, reading, &roles(), 1.0).unwrap();
    let id = pending(&conn, None).unwrap()[0].id;

    // The wrong outcome is refused; the listing still on auction.
    assert_eq!(
        link(&mut conn, &[id], "match", 2.0).unwrap(),
        Decided::Refused("that record does not stand as the outcome the proposal is of")
    );
    conn.execute(
        "UPDATE auction_listings SET status = 'sold', final_price = 12.5 WHERE id = 'match'",
        [],
    )
    .unwrap();
    // Resolved by hand: settled, whether or not it is marked.
    assert!(pending(&conn, None).unwrap().is_empty());
    assert_eq!(link(&mut conn, &[id], "match", 2.0).unwrap(), Decided::Done);
    assert_eq!(
        link(&mut conn, &[id], "match", 3.0).unwrap(),
        Decided::Done,
        "marking again changes nothing"
    );
    assert_eq!(
        dismiss(&mut conn, id, 3.0).unwrap(),
        Decided::Refused("that proposal is recorded")
    );
    // The sale reverted: pending again.
    conn.execute(
        "UPDATE auction_listings SET status = 'pending', final_price = NULL WHERE id = 'match'",
        [],
    )
    .unwrap();
    assert_eq!(pending(&conn, None).unwrap().len(), 1);
    assert_eq!(dismiss(&mut conn, id, 4.0).unwrap(), Decided::Done);
    assert!(pending(&conn, None).unwrap().is_empty());
    assert_eq!(dismiss(&mut conn, id, 5.0).unwrap(), Decided::Done);
    assert_eq!(
        link(&mut conn, &[ProposalId(99)], "x", 5.0).unwrap(),
        Decided::NotFound
    );
}

#[test]
fn a_step_inside_a_session_names_it_and_the_step_before_its_baseline_does_not() {
    let mut conn = migrated_in_memory().unwrap();
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES ('hunt', 1, 0)",
        [],
    )
    .unwrap();
    let readings = recorded_hunt();
    record_reading(&mut conn, &readings[1], Purpose::Connect, None).unwrap();
    let baseline = record_reading(&mut conn, &readings[2], Purpose::Baseline, Some("hunt"))
        .unwrap()
        .id;
    for reading in &readings[3..10] {
        record_reading(&mut conn, reading, Purpose::Session, Some("hunt")).unwrap();
    }
    examine_all(&mut conn).unwrap();
    let proposals = pending(&conn, None).unwrap();
    assert_eq!(proposals.len(), 2);
    assert_eq!(proposals[0].session_id.as_deref(), Some("hunt"));
    assert_eq!(proposals[1].reading, baseline);
    assert_eq!(proposals[1].session_id, None);
    // Deleting the session lets go of it.
    super::sessions::release_session(&conn, "hunt").unwrap();
    assert!(pending(&conn, None)
        .unwrap()
        .iter()
        .all(|proposal| proposal.session_id.is_none()));
}

/// Two readings a minute apart from `base`, the Shrapnel listing sold
/// between them.
fn sold_step_at(conn: &mut rusqlite::Connection, base: f64) -> ReadingId {
    let ids = stored(
        conn,
        &[
            on_auction(base, 10_000, true, &[]),
            on_auction(base + 60.0, 11_250, false, &[]),
        ],
    );
    ids[1]
}

fn proposed(conn: &rusqlite::Connection) -> Vec<String> {
    pending(conn, None)
        .unwrap()
        .into_iter()
        .filter_map(|proposal| match proposal.detail {
            Detail::Listing(listing) => Some(listing.listing_id),
            Detail::Opening(_) | Detail::Purchase(_) | Detail::Arrival(_) => None,
        })
        .collect()
}

#[test]
fn a_listing_s_tt_agrees_within_a_pec_a_row_and_no_further() {
    for (tt, matches) in [(10.0, true), (10.01, true), (9.99, true), (10.02, false)] {
        let mut conn = migrated_in_memory().unwrap();
        listing(&conn, "match", "Shrapnel", 100_000, tt);
        let reading = sold_step(&mut conn);
        examine_reading(&mut conn, reading, &roles(), 1.0).unwrap();
        assert_eq!(proposed(&conn).len(), usize::from(matches), "TT {tt}");
    }
}

#[test]
fn a_listing_made_after_the_step_began_is_not_matched() {
    const DAY: f64 = 86_400.0;
    let base = 10.0 * DAY;
    let cases: [(&str, Option<f64>, bool); 5] = [
        ("1970-01-11", Some(base - 1.0), true),
        ("1970-01-11", Some(base), true),
        ("1970-01-11", Some(base + 30.0), false),
        ("1970-01-05", None, true),
        ("1970-01-20", None, false),
    ];
    for (day, instant, matches) in cases {
        let mut conn = migrated_in_memory().unwrap();
        listed(&conn, "match", "Shrapnel", 100_000, 10.0, day, instant);
        let reading = sold_step_at(&mut conn, base);
        examine_reading(&mut conn, reading, &roles(), 1.0).unwrap();
        assert_eq!(
            proposed(&conn).len(),
            usize::from(matches),
            "{day} {instant:?}"
        );
    }
}

#[test]
fn a_listing_leaving_while_the_same_item_is_looted_proposes_nothing() {
    let outcomes = step_outcomes(
        &on_auction(0.0, 10_000, true, &[]),
        &on_auction(60.0, 10_000, false, &[("Shrapnel", C, 100_500, 1_005)]),
        &roles(),
    )
    .unwrap();
    assert!(outcomes.departures.is_empty(), "{:?}", outcomes.departures);
    // Sold beside loot of the same item: no sale proposed either.
    let outcomes = step_outcomes(
        &on_auction(0.0, 10_000, true, &[("Shrapnel", C, 500, 5)]),
        &on_auction(60.0, 11_250, false, &[("Shrapnel", C, 900, 9)]),
        &roles(),
    )
    .unwrap();
    assert!(outcomes.departures.is_empty(), "{:?}", outcomes.departures);
}

#[test]
fn a_reading_whose_step_cannot_be_read_is_passed_and_the_rest_examined() {
    let mut conn = migrated_in_memory().unwrap();
    let readings = recorded_hunt();
    record_reading(&mut conn, &readings[0], Purpose::Run, None).unwrap();
    let bad = record_reading(&mut conn, &readings[1], Purpose::Connect, None)
        .unwrap()
        .id;
    // A reading that no longer decodes: totals its rows do not add up to.
    conn.execute(
        "UPDATE account_reading_holdings SET quantity = quantity + 1 \
         WHERE reading_id = ?1 AND row_count > 0",
        [bad.0],
    )
    .unwrap();
    // A fresh keyframe after it, then the hunt's opening of seven.
    record_reading(&mut conn, &readings[8], Purpose::Connect, None).unwrap();
    record_reading(&mut conn, &readings[9], Purpose::Run, None).unwrap();
    assert_eq!(
        examine_all(&mut conn).unwrap(),
        Pass {
            examined: 4,
            passed: 2,
            added: 1
        }
    );
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM account_readings_examined"),
        4
    );
    assert_eq!(pending(&conn, None).unwrap().len(), 1);
}

/// A standing opening of `count` boxes recorded at `created_at`.
fn recorded_opening(conn: &rusqlite::Connection, id: &str, count: f64, created_at: f64) {
    conn.execute(
        "INSERT INTO stock_conversions (id, source_item, target_item, quantity, tt_value, \
             converted_at, created_at) VALUES (?1, 'Halloween Strongbox', 'Mixed', 1, 0, \
             '2026-11-14', ?2)",
        rusqlite::params![format!("c-{id}"), created_at],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO strongbox_openings (id, box_item, box_count, intake, \
             opening_conversion_id, opened_at, created_at) \
         VALUES (?1, 'Halloween Strongbox', ?2, 'export', ?3, '2026-11-14', ?4)",
        rusqlite::params![id, count, format!("c-{id}"), created_at],
    )
    .unwrap();
}

/// Openings of `counts` boxes in consecutive steps a minute apart, from
/// a reading at `base`, examined.
fn openings_from(conn: &mut rusqlite::Connection, base: f64, counts: &[i64]) {
    let mut held = 40;
    let mut readings = vec![reading(
        base,
        0,
        &[
            ("Halloween Strongbox", C, held, 0),
            ("Strongbox Key", C, held, held * 1_000),
        ],
    )];
    for (index, opened) in counts.iter().enumerate() {
        held -= opened;
        let mut rows: Vec<(&str, Container, i64, i64)> = vec![
            ("Halloween Strongbox", C, held, 0),
            ("Strongbox Key", C, held, held * 1_000),
        ];
        // Each opening's ammo, a row a box, in addition to every earlier one.
        let paid: i64 = counts[..=index].iter().sum();
        for _ in 0..paid {
            rows.push(("Universal Ammo", C, 100_000, 1_000));
        }
        readings.push(reading(base + 60.0 * (index + 1) as f64, 0, &rows));
    }
    stored(conn, &readings);
    examine_all(conn).unwrap();
}

fn open_counts(conn: &rusqlite::Connection) -> Vec<i64> {
    pending(conn, Some(ProposalKind::Opening))
        .unwrap()
        .iter()
        .map(|proposal| match &proposal.detail {
            Detail::Opening(opening) => opening.boxes[0].count,
            Detail::Listing(_) | Detail::Purchase(_) | Detail::Arrival(_) => 0,
        })
        .collect()
}

#[test]
fn an_opening_recorded_another_way_accounts_for_one_group_of_proposals_only() {
    // One pasted opening does not hide two of the same count: the older
    // stays.
    let mut conn = migrated_in_memory().unwrap();
    openings_from(&mut conn, 0.0, &[7]);
    openings_from(&mut conn, 1_000.0, &[7]);
    assert_eq!(open_counts(&conn), vec![7, 7]);
    recorded_opening(&conn, "pasted", 7.0, 2_000.0);
    let left = pending(&conn, None).unwrap();
    assert_eq!(left.len(), 1);
    assert!(left[0].between.from < 100.0, "the older one stays");

    // A pasted opening of seven accounts for a three and a four seen in
    // consecutive steps.
    let mut conn = migrated_in_memory().unwrap();
    openings_from(&mut conn, 0.0, &[3, 4]);
    assert_eq!(open_counts(&conn), vec![4, 3]);
    recorded_opening(&conn, "pasted", 7.0, 500.0);
    assert!(open_counts(&conn).is_empty());

    // A different count, or an opening recorded before the step began,
    // accounts for nothing.
    let mut conn = migrated_in_memory().unwrap();
    openings_from(&mut conn, 1_000.0, &[7]);
    recorded_opening(&conn, "other", 6.0, 2_000.0);
    recorded_opening(&conn, "earlier", 7.0, 500.0);
    assert_eq!(open_counts(&conn), vec![7]);
}

#[test]
fn a_claim_marks_proposals_possibly_recorded_until_released_or_linked() {
    let mut conn = migrated_in_memory().unwrap();
    openings_from(&mut conn, 0.0, &[3, 4]);
    let found: Vec<ProposalId> = pending(&conn, None)
        .unwrap()
        .iter()
        .map(|proposal| proposal.id)
        .collect();
    assert_eq!(claim(&mut conn, &found, &[], 2.0).unwrap(), Decided::Done);
    assert!(pending(&conn, None).unwrap().iter().all(|p| p.claimed));
    // Claimed and not released: not claimed again, whole or in part.
    assert_eq!(
        claim(&mut conn, &found[..1], &[], 3.0).unwrap(),
        Decided::Refused("that proposal may already be recorded; check your openings first")
    );
    assert_eq!(release(&mut conn, &found).unwrap(), Decided::Done);
    assert!(pending(&conn, None).unwrap().iter().all(|p| !p.claimed));
    assert_eq!(claim(&mut conn, &found, &[], 4.0).unwrap(), Decided::Done);

    // Linked as one: both leave, and their claim with them.
    recorded_opening(&conn, "seven", 7.0, 500.0);
    assert_eq!(
        link(&mut conn, &found, "seven", 5.0).unwrap(),
        Decided::Done
    );
    assert!(pending(&conn, None).unwrap().is_empty());
    let claimed = count(
        &conn,
        "SELECT COUNT(*) FROM account_proposals WHERE claimed_at IS NOT NULL",
    );
    assert_eq!(claimed, 0);
    // Taken back: both open again, not claimed.
    conn.execute(
        "UPDATE strongbox_openings SET undone_at = '2026-11-15' WHERE id = 'seven'",
        [],
    )
    .unwrap();
    assert_eq!(pending(&conn, None).unwrap().len(), 2);
    // A set with one proposal no longer open is refused whole.
    dismiss(&mut conn, found[0], 6.0).unwrap();
    assert_eq!(
        claim(&mut conn, &found, &[], 7.0).unwrap(),
        Decided::Refused("that proposal is no longer open")
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM account_proposals WHERE claimed_at IS NOT NULL"
        ),
        0
    );
    assert_eq!(
        claim(&mut conn, &[ProposalId(99)], &[], 7.0).unwrap(),
        Decided::NotFound
    );
}

#[test]
fn a_database_error_leaves_the_reading_to_be_examined_again() {
    let mut conn = migrated_in_memory().unwrap();
    listing(&conn, "match", "Shrapnel", 100_000, 10.0);
    let reading = sold_step(&mut conn);
    // Matching the departure needs the listings, and the query fails.
    conn.execute_batch("ALTER TABLE auction_listings RENAME TO listings_away")
        .unwrap();
    let error = examine_reading(&mut conn, reading, &roles(), 1.0).unwrap_err();
    assert!(matches!(error, AccountError::Db(_)), "{error:?}");
    let examined = count(
        &conn,
        "SELECT COUNT(*) FROM account_readings_examined WHERE reading_id = 2",
    );
    assert_eq!(examined, 0, "left to be examined again");
    conn.execute_batch("ALTER TABLE listings_away RENAME TO auction_listings")
        .unwrap();
    assert_eq!(
        examine_reading(&mut conn, reading, &roles(), 1.0).unwrap(),
        1
    );
}

#[test]
fn a_session_let_go_before_the_write_leaves_the_proposal_without_one() {
    let mut conn = migrated_in_memory().unwrap();
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, is_active) VALUES ('hunt', 1, 0)",
        [],
    )
    .unwrap();
    let readings = recorded_hunt();
    record_reading(&mut conn, &readings[8], Purpose::Session, Some("hunt")).unwrap();
    let seven = record_reading(&mut conn, &readings[9], Purpose::Session, Some("hunt"))
        .unwrap()
        .id;
    let examination = prepare(&conn, seven, &roles()).unwrap().unwrap();
    // The session is deleted between working the step out and storing it.
    super::sessions::release_session(&conn, "hunt").unwrap();
    conn.execute("DELETE FROM tracking_sessions WHERE id = 'hunt'", [])
        .unwrap();
    assert_eq!(commit(&mut conn, &examination, 1.0).unwrap(), 1);
    let found = pending(&conn, None).unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].session_id, None);
}

#[test]
fn an_opening_recorded_after_a_claim_accounts_for_the_claimed_proposal_first() {
    let mut conn = migrated_in_memory().unwrap();
    // P, five at 10:00, and P', five at 10:05.
    openings_from(&mut conn, 36_000.0, &[5]);
    openings_from(&mut conn, 36_300.0, &[5]);
    let found = pending(&conn, None).unwrap();
    assert_eq!(found.len(), 2);
    let (later, earlier) = (found[0].id, found[1].id);
    // P is claimed, recorded as O, and the link never made.
    assert_eq!(
        claim(&mut conn, &[earlier], &[], 36_400.0).unwrap(),
        Decided::Done
    );
    recorded_opening(&conn, "o", 5.0, 36_500.0);
    let left = pending(&conn, None).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].id, later, "P' stays");
    assert!(!left[0].claimed, "an ordinary pending proposal");
}

#[test]
fn a_claim_left_unlinked_is_taken_over_only_when_named() {
    let mut conn = migrated_in_memory().unwrap();
    openings_from(&mut conn, 0.0, &[3, 4]);
    let found: Vec<ProposalId> = pending(&conn, None)
        .unwrap()
        .iter()
        .map(|proposal| proposal.id)
        .collect();
    assert_eq!(
        claim(&mut conn, &found[..1], &[], 2.0).unwrap(),
        Decided::Done
    );
    // The whole set, the claimed one named to be taken over: one
    // transaction, all of it claimed.
    assert_eq!(
        claim(&mut conn, &found, &found[..1], 3.0).unwrap(),
        Decided::Done
    );
    assert!(pending(&conn, None).unwrap().iter().all(|p| p.claimed));
    // A claimed proposal not named is still refused, and nothing changes.
    release(&mut conn, &found[1..]).unwrap();
    assert_eq!(
        claim(&mut conn, &found, &found[1..], 4.0).unwrap(),
        Decided::Refused("that proposal may already be recorded; check your openings first")
    );
    let claimed_at: f64 = conn
        .query_row(
            "SELECT claimed_at FROM account_proposals WHERE id = ?1",
            [found[0].0],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(claimed_at, 3.0);
}

/// A fast run opened in steps a minute apart: from 80 boxes and 80 keys,
/// each step's boxes paying ten PED each, `ammo` of them as a row of
/// Universal Ammo and the rest as an item of their own. Readings at
/// `base`, `base + 60` and so on, with one more after the last step that
/// changes nothing.
fn fast_run(base: f64, steps: &[(i64, i64)]) -> Vec<Reading> {
    let mut held = 80;
    let mut names: Vec<String> = Vec::new();
    let mut ammo_rows = 0;
    let at = |index: usize| base + 60.0 * index as f64;
    let build = |at: f64, held: i64, ammo_rows: i64, names: &[String]| {
        let mut rows: Vec<(&str, Container, i64, i64)> = vec![
            ("Halloween Strongbox", C, held, 0),
            ("Strongbox Key", C, held, held * 1_000),
            ("Universal Ammo", C, 50_000, 500),
        ];
        for _ in 0..ammo_rows {
            rows.push(("Universal Ammo", C, 100_000, 1_000));
        }
        for name in names {
            rows.push((name.as_str(), C, 1, 1_000));
        }
        reading(at, 25_000, &rows)
    };
    let mut readings = vec![build(at(0), held, ammo_rows, &names)];
    for (index, (ammo, items)) in steps.iter().enumerate() {
        held -= ammo + items;
        ammo_rows += ammo;
        for item in 0..*items {
            names.push(format!("Prize {index}-{item} (F)"));
        }
        readings.push(build(at(index + 1), held, ammo_rows, &names));
    }
    let last = readings.len();
    readings.push(build(at(last), held, ammo_rows, &names));
    readings
}

/// Store readings in order, untagged, as `purpose` would.
fn stored_as(conn: &mut rusqlite::Connection, readings: &[Reading], purpose: Purpose) {
    for reading in readings {
        record_reading(conn, reading, purpose, None).expect("stored");
    }
}

/// Three steps, each with more arrivals than its weighing can take apart.
const FAST: [(i64, i64); 3] = [(5, 17), (4, 18), (3, 17)];

#[test]
fn a_fast_run_seen_while_idle_is_one_settled_opening_with_every_item_certain() {
    let readings = fast_run(0.0, &FAST);
    // Step by step, each is too much to weigh: unsettled, nothing certain.
    for pair in readings[..4].windows(2) {
        let opening = step_outcomes(&pair[0], &pair[1], &roles())
            .unwrap()
            .opening
            .expect("an opening");
        assert_eq!(opening.settlement, Settlement::Unsettled);
        assert!(opening.contents.is_empty());
    }
    let mut conn = migrated_in_memory().unwrap();
    stored_as(&mut conn, &readings, Purpose::Ambient);
    examine_all(&mut conn).unwrap();
    let proposals = pending(&conn, Some(ProposalKind::Opening)).unwrap();
    assert_eq!(proposals.len(), 1, "one opening, not three in doubt");
    let proposal = &proposals[0];
    assert_eq!(proposal.prev_reading, ReadingId(1));
    assert_eq!(proposal.reading, ReadingId(4));
    assert_eq!(proposal.between.from, 0.0);
    assert_eq!(proposal.between.to, 180.0);
    let Detail::Opening(opening) = &proposal.detail else {
        panic!("an opening");
    };
    assert_eq!(opening.boxes[0].count, 64);
    assert_eq!(opening.keys_used, 64);
    assert_eq!(opening.keys.micro_ped(), 640 * PED);
    assert_eq!(opening.settlement, Settlement::Settled);
    assert_eq!(opening.doubt, None);
    assert_eq!(
        opening.ammo,
        Some(ProposedAmmo {
            units: 1_200_000,
            amount: Amount::exact(120 * PED)
        })
    );
    assert_eq!(opening.contents.len(), 52, "every item is certain");
    assert!(opening.possible_contents.is_empty());
    assert_eq!(opening.paid_ammo_alone, Some(12));
    // The steps' own proposals, and the opening read whole over the first
    // two, are still stored, and no longer offered.
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM account_proposals WHERE kind = 'opening'"
        ),
        5
    );
    // Recording it settles the run: none of the steps is offered again.
    let id = proposal.id;
    assert_eq!(claim(&mut conn, &[id], &[], 500.0).unwrap(), Decided::Done);
    recorded_opening(&conn, "whole", 64.0, 500.0);
    assert_eq!(
        link(&mut conn, &[id], "whole", 501.0).unwrap(),
        Decided::Done
    );
    assert!(pending(&conn, None).unwrap().is_empty());
}

#[test]
fn a_run_seen_by_a_strongbox_run_s_own_readings_keeps_its_steps() {
    // The strongbox run's own readings keep today's step by step reading.
    let mut conn = migrated_in_memory().unwrap();
    stored_as(&mut conn, &fast_run(0.0, &FAST), Purpose::Run);
    examine_all(&mut conn).unwrap();
    assert_eq!(open_counts(&conn), vec![20, 22, 22]);
}

#[test]
fn a_span_read_whole_joins_only_steps_still_undecided_and_only_when_it_balances() {
    // A step dismissed before the next arrives is not joined: the later
    // steps are read whole from where it ends.
    let readings = fast_run(0.0, &FAST);
    let mut conn = migrated_in_memory().unwrap();
    stored_as(&mut conn, &readings[..2], Purpose::Ambient);
    examine_all(&mut conn).unwrap();
    let first = pending(&conn, None).unwrap()[0].id;
    assert_eq!(dismiss(&mut conn, first, 100.0).unwrap(), Decided::Done);
    stored_as(&mut conn, &readings[2..], Purpose::Ambient);
    examine_all(&mut conn).unwrap();
    let proposals = pending(&conn, Some(ProposalKind::Opening)).unwrap();
    assert_eq!(proposals.len(), 1);
    assert_eq!(proposals[0].prev_reading, ReadingId(2));
    assert_eq!(open_counts(&conn), vec![42]);

    // A span that does not balance (a stack arriving that no box paid)
    // leaves every step proposed on its own.
    let mut readings = fast_run(0.0, &FAST);
    let extra = |reading: &Reading| {
        let mut holdings = reading.holdings().clone();
        holdings.insert(
            super::reading::HoldingKey {
                name: "Shrapnel".into(),
                container: C,
            },
            super::reading::Holding::new(vec![super::reading::Row {
                quantity: 500_000,
                value: Pec(5_000),
            }])
            .unwrap(),
        );
        Reading::new(reading.taken_at(), reading.balance(), holdings).unwrap()
    };
    for reading in &mut readings[2..] {
        *reading = extra(reading);
    }
    let mut conn = migrated_in_memory().unwrap();
    stored_as(&mut conn, &readings, Purpose::Ambient);
    examine_all(&mut conn).unwrap();
    assert_eq!(open_counts(&conn), vec![20, 22, 22]);
}

#[test]
fn a_part_of_an_opening_read_whole_recorded_by_hand_turns_it_back_into_its_steps() {
    // Steps of 22, 22 and 20 boxes, read whole as 64.
    let run = |conn: &mut rusqlite::Connection| {
        stored_as(conn, &fast_run(0.0, &FAST), Purpose::Ambient);
        examine_all(conn).unwrap();
        assert_eq!(open_counts(conn), vec![64]);
    };
    // The middle 22 recorded by hand: the 64 is never offered again, and
    // the steps left are.
    let mut conn = migrated_in_memory().unwrap();
    run(&mut conn);
    recorded_opening(&conn, "middle", 22.0, 500.0);
    assert_eq!(open_counts(&conn), vec![20, 22]);
    // The last two steps, 20 and 22, recorded as one: the first is left.
    let mut conn = migrated_in_memory().unwrap();
    run(&mut conn);
    recorded_opening(&conn, "later", 42.0, 500.0);
    assert_eq!(open_counts(&conn), vec![22]);
    // The whole 64 recorded by hand accounts for the opening read whole.
    let mut conn = migrated_in_memory().unwrap();
    run(&mut conn);
    recorded_opening(&conn, "all", 64.0, 500.0);
    assert!(open_counts(&conn).is_empty());
    // A count no step or run of steps adds up to leaves the steps offered,
    // none of them twice.
    let mut conn = migrated_in_memory().unwrap();
    run(&mut conn);
    recorded_opening(&conn, "odd", 5.0, 500.0);
    assert_eq!(open_counts(&conn), vec![20, 22, 22]);
}

#[test]
fn an_opening_is_read_whole_across_sixty_steps_at_most() {
    // One box a minute for 61 minutes, each paying a row of ammo.
    let steps = 61;
    let mut held = 70;
    let mut readings = Vec::new();
    for index in 0..=steps {
        let mut rows: Vec<(&str, Container, i64, i64)> = vec![
            ("Halloween Strongbox", C, held, 0),
            ("Strongbox Key", C, held, held * 1_000),
            ("Universal Ammo", C, 50_000, 500),
        ];
        for _ in 0..index {
            rows.push(("Universal Ammo", C, 100_000, 1_000));
        }
        readings.push(reading(60.0 * f64::from(index), 25_000, &rows));
        held -= 1;
    }
    let mut conn = migrated_in_memory().unwrap();
    stored_as(&mut conn, &readings, Purpose::Ambient);
    examine_all(&mut conn).unwrap();
    // The first sixty steps read whole; the last on its own.
    assert_eq!(open_counts(&conn), vec![1, 60]);
    let proposals = pending(&conn, Some(ProposalKind::Opening)).unwrap();
    assert_eq!(proposals[1].prev_reading, ReadingId(1));
    assert_eq!(proposals[1].reading, ReadingId(61));
}

// ── A purchase at auction ───────────────────────────────────────────

/// One item arriving as the Card falls by 10 PED beyond its 2 PED of TT.
fn bought() -> Vec<Reading> {
    vec![
        reading(0.0, 5_000, &[]),
        reading(60.0, 3_800, &[("Garcen Grease", C, 10, 200)]),
    ]
}

fn purchases(conn: &rusqlite::Connection) -> Vec<super::recognition::Purchase> {
    pending(conn, Some(ProposalKind::Purchase))
        .unwrap()
        .into_iter()
        .filter_map(|proposal| match proposal.detail {
            Detail::Purchase(purchase) => Some(purchase),
            Detail::Opening(_) | Detail::Listing(_) | Detail::Arrival(_) => None,
        })
        .collect()
}

#[test]
fn a_purchase_outside_every_session_is_proposed_with_the_markup_paid() {
    let mut conn = migrated_in_memory().unwrap();
    stored(&mut conn, &bought());
    examine_all(&mut conn).unwrap();
    let found = purchases(&conn);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].key.name, "Garcen Grease");
    assert_eq!(found[0].units, 10);
    assert_eq!(found[0].paid, Pec(1_200));
    assert_eq!(found[0].markup.micro_ped(), 10 * PED);
}

#[test]
fn a_purchase_is_never_proposed_in_a_step_a_tracked_session_overlaps() {
    let mut conn = migrated_in_memory().unwrap();
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
         VALUES ('live', 30.0, NULL, 1)",
        [],
    )
    .unwrap();
    stored(&mut conn, &bought());
    examine_all(&mut conn).unwrap();
    assert!(purchases(&conn).is_empty());
}

/// A step whose readings were taken for a session is the session's, even
/// where the session's recorded span does not reach it.
#[test]
fn a_purchase_is_never_proposed_in_a_session_s_own_step() {
    let mut conn = migrated_in_memory().unwrap();
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
         VALUES ('live', 1000.0, NULL, 1)",
        [],
    )
    .unwrap();
    let readings = bought();
    record_reading(&mut conn, &readings[0], Purpose::Session, Some("live")).unwrap();
    record_reading(&mut conn, &readings[1], Purpose::Session, Some("live")).unwrap();
    examine_all(&mut conn).unwrap();
    assert!(purchases(&conn).is_empty());
}

#[test]
fn an_item_a_session_looted_inside_the_step_is_never_a_purchase() {
    let mut conn = migrated_in_memory().unwrap();
    conn.execute(
        "INSERT INTO tracking_sessions (id, started_at, ended_at, is_active) \
         VALUES ('earlier', -500.0, -400.0, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO kills (id, session_id, mob_name, mob_species, mob_maturity, timestamp, \
         shots_fired, damage_dealt, cost_ped, loot_total_ped, is_global, is_hof) \
         VALUES ('k', 'earlier', 'Atrox', 'Atrox', '', 30.0, 1, 1.0, 0.1, 2.0, 0, 0)",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO kill_loot_items (kill_id, item_name, quantity, value_ped) \
         VALUES ('k', 'Garcen Grease', 10, 2.0)",
        [],
    )
    .unwrap();
    stored(&mut conn, &bought());
    examine_all(&mut conn).unwrap();
    assert!(purchases(&conn).is_empty());
}

/// An asset held under `id`, with one purchase of `markup` PED above its
/// TT (or one a conversion made); the purchase's id.
fn asset(conn: &rusqlite::Connection, id: &str, markup: f64, made: bool) -> i64 {
    conn.execute(
        "INSERT OR IGNORE INTO inventory_items \
             (id, name, tt_value, markup_paid, acquired_at, quantity) \
         VALUES (?1, 'Garcen Grease', 2.0, ?2, '1970-01-01', 10)",
        rusqlite::params![id, markup],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO asset_purchases \
             (position_id, quantity, tt_value, markup_paid, acquired_at, conversion_id, \
              created_at) \
         VALUES (?1, 10, 2.0, ?2, '1970-01-01', ?3, 1.0)",
        rusqlite::params![id, markup, made.then_some("a-conversion")],
    )
    .unwrap();
    conn.last_insert_rowid()
}

#[test]
fn a_purchase_is_settled_by_the_asset_purchase_it_became_and_opens_again_if_that_goes() {
    let mut conn = migrated_in_memory().unwrap();
    stored(&mut conn, &bought());
    examine_all(&mut conn).unwrap();
    let id = pending(&conn, Some(ProposalKind::Purchase)).unwrap()[0].id;
    let older = asset(&conn, "older", 1.0, false);
    let made = asset(&conn, "grease", 0.0, true);
    let purchase = asset(&conn, "grease", 10.0, false);
    let refused = Decided::Refused("that record does not stand as the outcome the proposal is of");
    // Not an asset, nor a purchase a conversion made.
    assert_eq!(link(&mut conn, &[id], "grease", 2.0).unwrap(), refused);
    assert_eq!(
        link(&mut conn, &[id], &made.to_string(), 2.0).unwrap(),
        refused
    );
    // A claim left unlinked speaks of purchases.
    assert_eq!(claim(&mut conn, &[id], &[], 2.0).unwrap(), Decided::Done);
    assert_eq!(
        claim(&mut conn, &[id], &[], 2.0).unwrap(),
        Decided::Refused("that purchase may already be recorded; check your assets first")
    );
    assert_eq!(
        link(&mut conn, &[id], &purchase.to_string(), 2.0).unwrap(),
        Decided::Done
    );
    assert!(purchases(&conn).is_empty());
    assert_eq!(
        dismiss(&mut conn, id, 3.0).unwrap(),
        Decided::Refused("that proposal is recorded")
    );
    // Another asset going leaves it settled.
    conn.execute("DELETE FROM asset_purchases WHERE id = ?1", [older])
        .unwrap();
    conn.execute("DELETE FROM inventory_items WHERE id = 'older'", [])
        .unwrap();
    assert!(purchases(&conn).is_empty());
    // The purchase taken back opens it again.
    conn.execute("DELETE FROM asset_purchases WHERE id = ?1", [purchase])
        .unwrap();
    assert_eq!(purchases(&conn).len(), 1);
}
