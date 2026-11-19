//! A listing the readings saw the player make: one item or stack moving
//! into the auction in a step, the PED Card falling by the listing fee, and
//! nothing else in the step that could have moved the Card (the predicate
//! the proposals read for a sale's proceeds, [`super::proposals::card_move`]).
//!
//! The fee is the Card's fall, exactly. The starting bid the readings never
//! show: the fee model run backwards
//! ([`crate::auction_fee::starting_bids_for_fee`]) gives every starting bid
//! whose fee is the one paid, which is a range. Where the TT the readings
//! show rests on values the site truncated, the true TT may lie up to a PEC
//! a row either side of it, so the range runs from the lowest bid at the
//! shown TT less that (never below nothing) to the highest at the shown TT
//! plus it. The buyout and the duration are not known at all.
//!
//! Nothing is found for more than one thing listed in the step, for a fee
//! no starting bid gives under the model, or for a fee at the model's cap,
//! which every bid past a point gives. Whether the step is one a listing is
//! recorded from, and whether the app holds the item, are the pass's to
//! judge ([`super::follow`]).

use super::amount::{Pec, Precision, MICRO_PER_PEC, MICRO_PER_PED};
use super::diff::{MoveKind, Step, StepSet};
use super::proposals::card_move;
use super::reading::{HoldingKey, Reading};

/// A listing seen made in a step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListingMade {
    /// The item, as the account site names it, and how many were listed.
    pub name: String,
    pub units: i64,
    /// The fee: what the Card fell by.
    pub fee: Pec,
    /// The lowest and the highest starting bid the fee reads back as.
    pub bids: (Pec, Pec),
}

/// The listing the step `set` shows made, if it shows exactly one and its
/// fee reads back as a range of starting bids. Pure.
pub fn listing_made(set: &StepSet, before: &Reading) -> Option<ListingMade> {
    let (index, name, units, amount, from) =
        set.steps
            .iter()
            .enumerate()
            .find_map(|(index, step)| match step {
                Step::Moved {
                    name,
                    units,
                    amount,
                    from,
                    to,
                } if MoveKind::of(from, to) == MoveKind::Listing => {
                    Some((index, name, *units, *amount, from))
                }
                _ => None,
            })?;
    // Anything else listed, or anything else that could move the Card,
    // leaves its fall unread.
    let fee = card_move(set, index)?.checked_neg()?;
    if fee <= 0 || units <= 0 {
        return None;
    }
    // A truncated value may be off by up to a PEC a row, either way.
    let hidden = match amount.precision() {
        Precision::Exact => 0,
        Precision::WholePec => {
            let rows = before
                .holding(&HoldingKey {
                    name: name.clone(),
                    container: from.clone(),
                })
                .map_or(1, |holding| holding.row_count().max(1));
            i64::try_from(rows)
                .ok()?
                .checked_mul(MICRO_PER_PEC)?
                .checked_sub(1)?
        }
    };
    let tt = |micro: i64| micro as f64 / MICRO_PER_PED as f64;
    let shown = amount.micro_ped();
    let (low, _) =
        crate::auction_fee::starting_bids_for_fee(fee, tt(shown.saturating_sub(hidden).max(0)))?;
    let (_, high) = crate::auction_fee::starting_bids_for_fee(fee, tt(shown.checked_add(hidden)?))?;
    Some(ListingMade {
        name: name.clone(),
        units,
        fee: Pec(fee),
        bids: (Pec(low), Pec(high)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::proposals::step_between;
    use crate::account::reading::Container;
    use crate::account::roles::KnownRoles;
    use crate::account::test_support::reading;

    const C: Container = Container::Carried;
    const A: Container = Container::Auction;

    type Rows<'a> = &'a [(&'a str, Container, i64, i64)];

    /// What a minute's step from `before` to `after` shows made, the Card
    /// at 50.00 PED before and at `card` PEC after.
    fn made(before: Rows, after: Rows, card: i64) -> Option<ListingMade> {
        let (before, after) = (reading(0.0, 5_000, before), reading(60.0, card, after));
        let set = step_between(&before, &after, &KnownRoles::default()).unwrap();
        listing_made(&set, &before)
    }

    #[test]
    fn a_stack_moved_into_the_auction_is_a_listing_whose_fee_is_the_card_s_fall() {
        // 5.00 PED as the row shows it, up to a PEC either side in truth:
        // 5.21 to 5.40 at the shown TT, from 5.20 and to 5.41 at what it may
        // be.
        assert_eq!(
            made(
                &[("Animal Hide", C, 10, 500)],
                &[("Animal Hide", A, 10, 500)],
                4_949
            ),
            Some(ListingMade {
                name: "Animal Hide".to_string(),
                units: 10,
                fee: Pec(51),
                bids: (Pec(520), Pec(541)),
            })
        );
        // Part of a stack, beside what stays.
        let part = made(
            &[("Animal Hide", C, 30, 1_500)],
            &[("Animal Hide", C, 20, 1_000), ("Animal Hide", A, 10, 500)],
            4_949,
        )
        .expect("a listing");
        assert_eq!((part.units, part.fee), (10, Pec(51)));
        assert_eq!(part.bids.0, Pec(520));
    }

    #[test]
    fn a_tt_the_site_truncated_widens_the_bids_both_ways() {
        // 30.00 PED as shown, 29.9901 to 30.0099 in truth: a PEC either side
        // of what the shown TT alone gives.
        let item =
            made(&[("Opalo", C, 1, 3_000)], &[("Opalo", A, 1, 3_000)], 4_949).expect("a listing");
        assert_eq!(
            crate::auction_fee::starting_bids_for_fee(51, 30.0),
            Some((3_021, 3_040))
        );
        assert_eq!(item.bids, (Pec(3_020), Pec(3_041)));
        // A true TT of 29.9901 under a shown 30.00: a starting bid of 30.20
        // pays the 0.51 seen, below the low end the shown TT alone gives.
        let fee = crate::auction_fee::listing_fee_ped(30.20 - 29.9901);
        assert!((fee - 0.51).abs() < 1e-9);
        assert!(item.bids.0 <= Pec(3_020));
    }

    #[test]
    fn a_tt_of_nothing_shown_never_reads_below_nothing() {
        let item = made(&[("Opalo", C, 1, 0)], &[("Opalo", A, 1, 0)], 4_950).expect("a listing");
        assert_eq!(item.bids.0, Pec(0));
    }

    #[test]
    fn wear_and_moves_between_the_player_s_own_places_leave_the_fee_read() {
        let found = made(
            &[
                ("Animal Hide", C, 10, 500),
                (
                    "Iron Stone",
                    Container::Storage {
                        planet: "Calypso".to_string(),
                    },
                    5,
                    130,
                ),
            ],
            &[("Animal Hide", A, 10, 500), ("Iron Stone", C, 5, 130)],
            4_949,
        );
        assert_eq!(found.map(|made| made.fee), Some(Pec(51)));
    }

    #[test]
    fn nothing_is_read_when_more_than_one_thing_is_listed_or_the_card_moved_otherwise() {
        let hides_and_stones: Rows = &[("Animal Hide", C, 10, 500), ("Iron Stone", C, 5, 130)];
        // Two things listed: whose fee is which is not known.
        assert_eq!(
            made(
                hides_and_stones,
                &[("Animal Hide", A, 10, 500), ("Iron Stone", A, 5, 130)],
                4_898
            ),
            None
        );
        // Something arrived beside it: it may have moved the Card.
        assert_eq!(
            made(
                &[("Animal Hide", C, 10, 500)],
                &[("Animal Hide", A, 10, 500), ("Garcen Grease", C, 1, 100)],
                4_949
            ),
            None
        );
        // The Card rose, or did not move.
        for card in [5_051, 5_000] {
            assert_eq!(
                made(
                    &[("Animal Hide", C, 10, 500)],
                    &[("Animal Hide", A, 10, 500)],
                    card
                ),
                None,
                "Card at {card}"
            );
        }
    }

    #[test]
    fn a_fee_no_bid_gives_or_one_at_the_cap_is_no_listing_read() {
        let hides: Rows = &[("Animal Hide", C, 10, 500)];
        let listed: Rows = &[("Animal Hide", A, 10, 500)];
        assert_eq!(made(hides, listed, 5_000 - 30), None, "below the base fee");
        let at_cap = reading(60.0, 10_000 - 7_512, listed);
        let before = reading(0.0, 10_000, hides);
        let set = step_between(&before, &at_cap, &KnownRoles::default()).unwrap();
        assert_eq!(listing_made(&set, &before), None, "at the cap");
        // A PEC under the cap still reads.
        let under = reading(60.0, 10_000 - 7_511, listed);
        let set = step_between(&before, &under, &KnownRoles::default()).unwrap();
        assert!(listing_made(&set, &before).is_some());
    }
}
