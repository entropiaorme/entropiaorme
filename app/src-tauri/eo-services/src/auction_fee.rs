//! Auction listing-fee economics measured from the game's sale window.
//!
//! The fee depends on gross markup, not item identity, quantity, buyout, or
//! duration. A fee-efficient packet keeps the listing fee within the user's
//! chosen share of expected gross markup, before the separately charged sale
//! fee.

const CURVE_COEFFICIENT: f64 = 0.000_670_02;
const BASE_FEE_PED: f64 = 0.50;
const PEC_PER_PED: f64 = 100.0;
// The curve approaches 75.124... PED from below, so 75.12 PED is the
// greatest fee the game's whole-PEC display can emit.
const MAX_DISPLAYED_FEE_PEC: u64 = 7_512;
pub const DEFAULT_MAX_FEE_SHARE_PCT: f64 = 10.0;

/// The game's displayed listing fee for a non-negative gross markup amount.
/// The quote truncates, rather than rounds, to whole PEC.
pub fn listing_fee_ped(gross_markup_ped: f64) -> f64 {
    if !gross_markup_ped.is_finite() || gross_markup_ped <= 0.0 {
        return BASE_FEE_PED;
    }
    let raw = BASE_FEE_PED + 0.05 / (CURVE_COEFFICIENT + gross_markup_ped.recip());
    (raw * PEC_PER_PED + f64::EPSILON).floor() / PEC_PER_PED
}

/// Smallest TT packet, rounded upward to a whole PEC, whose expected direct
/// market markup amortises the listing fee to the requested share or less.
///
/// Markup is the auction's percentage convention (`105` means five per cent
/// over TT). No recommendation is possible without a finite positive premium.
pub fn recommended_packet_tt(markup_pct: f64, max_fee_share_pct: f64) -> Option<f64> {
    if !markup_pct.is_finite() || markup_pct <= 100.0 {
        return None;
    }
    let premium = markup_pct / 100.0 - 1.0;
    let required_markup = minimum_efficient_gross_markup_ped(max_fee_share_pct)?;
    // Remove only floating-point dust at an exact PEC boundary. This does not
    // turn a genuinely fractional PEC downwards.
    Some((((required_markup / premium) * PEC_PER_PED) - 1e-9).ceil() / PEC_PER_PED)
}

/// The fee the game shows for a listing at `bid_pec` of an item of
/// `tt_ped` TT, in whole PEC.
fn fee_pec_at(bid_pec: i64, tt_ped: f64) -> i64 {
    (listing_fee_ped(bid_pec as f64 / PEC_PER_PED - tt_ped) * PEC_PER_PED).round() as i64
}

/// Far past any bid whose fee is below the cap, in PEC: the fee reaches the
/// cap near 24 million PED of markup.
const BID_SEARCH_SPAN_PEC: i64 = 1_000_000_000_000;

/// The starting bids, in whole PEC, at which the game shows a listing fee
/// of exactly `fee_pec` for an item of `tt_ped` TT: the fee model run
/// backwards. The fee never falls as the bid rises, so the bids that give
/// one fee form a range, returned as its lowest and highest. The auction
/// takes no bid below the item's TT, so no range starts below it (rounded
/// up to the PEC).
///
/// `None` for a fee no bid gives (below the base fee, past the cap, or one
/// the whole-PEC display skips), and for a fee at the cap, which every bid
/// past a point gives, so no range holds it.
pub fn starting_bids_for_fee(fee_pec: i64, tt_ped: f64) -> Option<(i64, i64)> {
    let cap = i64::try_from(MAX_DISPLAYED_FEE_PEC).ok()?;
    let base = (BASE_FEE_PED * PEC_PER_PED).round() as i64;
    if !tt_ped.is_finite() || tt_ped < 0.0 || fee_pec < base || fee_pec >= cap {
        return None;
    }
    let lowest = (tt_ped * PEC_PER_PED - 1e-9).ceil() as i64;
    // The first bid whose fee reaches `target`: the fee at the far end of
    // the search is the cap, which every target here is no more than.
    let first_reaching = |target: i64| {
        let (mut low, mut high) = (lowest, lowest + BID_SEARCH_SPAN_PEC);
        while low < high {
            let mid = low + (high - low) / 2;
            if fee_pec_at(mid, tt_ped) >= target {
                high = mid;
            } else {
                low = mid + 1;
            }
        }
        low
    };
    let from = first_reaching(fee_pec);
    if fee_pec_at(from, tt_ped) != fee_pec {
        return None;
    }
    Some((from, first_reaching(fee_pec + 1) - 1))
}

pub fn minimum_efficient_gross_markup_ped(max_fee_share_pct: f64) -> Option<f64> {
    if !max_fee_share_pct.is_finite() || max_fee_share_pct <= 0.0 || max_fee_share_pct > 100.0 {
        return None;
    }
    let fee_share = max_fee_share_pct / 100.0;

    // Truncating the displayed fee creates a small sawtooth at every fee-PEC
    // boundary, so efficiency is not a monotonic predicate and cannot be
    // binary-searched. Instead, consider every possible displayed fee. For a
    // fee F, ceil(F / share) is the earliest markup PEC that could qualify;
    // if the actual fee there is no greater than F, it is efficient. Any true
    // global minimum must appear among these finite candidates.
    for displayed_fee_pec in 50..=MAX_DISPLAYED_FEE_PEC {
        let candidate = ((displayed_fee_pec as f64 / fee_share) - 1e-9).ceil();
        if !candidate.is_finite() || candidate > u64::MAX as f64 {
            return None;
        }
        let candidate_pec = candidate.max(1.0) as u64;
        let gross_markup = candidate_pec as f64 / PEC_PER_PED;
        if listing_fee_ped(gross_markup) <= fee_share * gross_markup {
            return Some(gross_markup);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measured_curve_points_match_the_game_to_the_pec() {
        for (markup, fee) in [
            (0.0, 0.50),
            (1.0, 0.54),
            (100.0, 5.18),
            (900.0, 28.57),
            (9_900.0, 65.34),
            (1_111_118.96, 75.02),
        ] {
            assert_eq!(listing_fee_ped(markup), fee, "gross markup {markup}");
        }
    }

    #[test]
    fn finite_markup_extremes_reach_the_displayed_fee_cap() {
        assert_eq!(listing_fee_ped(f64::MAX), 75.12);
    }

    #[test]
    fn fee_efficiency_crosses_at_nine_ped_eighty_markup() {
        assert!(listing_fee_ped(9.79) > 9.79 * 0.10);
        assert_eq!(listing_fee_ped(9.80), 0.98);
        assert!(listing_fee_ped(9.80) <= 9.80 * 0.10);
        assert!(listing_fee_ped(125.99) > 125.99 * 0.05);
        assert_eq!(listing_fee_ped(126.00), 6.30);
        assert!(listing_fee_ped(126.00) <= 126.00 * 0.05);
        assert_eq!(minimum_efficient_gross_markup_ped(5.0), Some(126.0));
        assert_eq!(minimum_efficient_gross_markup_ped(10.0), Some(9.8));
        assert_eq!(minimum_efficient_gross_markup_ped(15.0), Some(4.94));
    }

    #[test]
    fn a_fee_reads_back_as_the_whole_range_of_bids_that_give_it() {
        // Five PED of TT: the base fee holds to twenty PEC of markup.
        assert_eq!(starting_bids_for_fee(50, 5.0), Some((500, 520)));
        assert_eq!(starting_bids_for_fee(51, 5.0), Some((521, 540)));
        for fee in [50, 51, 98, 518, 2_857, 6_534, 7_511] {
            let (low, high) = starting_bids_for_fee(fee, 12.345).expect("a fee the model gives");
            assert!(low <= high, "fee {fee}");
            for bid in [low, (low + high) / 2, high] {
                assert_eq!(fee_pec_at(bid, 12.345), fee, "fee {fee}, bid {bid}");
            }
            assert_eq!(
                fee_pec_at(high + 1, 12.345),
                fee + 1,
                "fee {fee}: the next PEC is past it"
            );
            if fee > 50 {
                assert_eq!(
                    fee_pec_at(low - 1, 12.345),
                    fee - 1,
                    "fee {fee}: the PEC before is short of it"
                );
            }
        }
    }

    #[test]
    fn no_range_starts_below_the_item_s_tt() {
        // 12.345 PED of TT: the lowest bid is 12.35.
        assert_eq!(
            starting_bids_for_fee(50, 12.345).map(|(low, _)| low),
            Some(1_235)
        );
        assert_eq!(starting_bids_for_fee(50, 0.0).map(|(low, _)| low), Some(0));
    }

    #[test]
    fn a_fee_no_bid_gives_or_one_at_the_cap_reads_back_as_nothing() {
        assert_eq!(starting_bids_for_fee(49, 5.0), None, "below the base fee");
        assert_eq!(starting_bids_for_fee(0, 5.0), None);
        assert_eq!(
            starting_bids_for_fee(7_512, 5.0),
            None,
            "the cap holds no range"
        );
        assert_eq!(starting_bids_for_fee(7_600, 5.0), None, "past the cap");
        assert_eq!(starting_bids_for_fee(50, f64::NAN), None);
        assert_eq!(starting_bids_for_fee(50, -1.0), None);
    }

    proptest::proptest! {
        #[test]
        fn every_bid_lies_in_the_range_its_own_fee_reads_back_as(
            tt_pec in 0i64..=100_000_000,
            markup_pec in 0i64..1_000_000_000,
        ) {
            let tt = tt_pec as f64 / PEC_PER_PED;
            let bid = tt_pec + markup_pec;
            let fee = fee_pec_at(bid, tt);
            if fee < 7_512 {
                let (low, high) = starting_bids_for_fee(fee, tt).expect("a fee the model gives");
                proptest::prop_assert!(low <= bid && bid <= high);
            } else {
                proptest::prop_assert_eq!(starting_bids_for_fee(fee, tt), None);
            }
        }
    }

    #[test]
    fn packet_recommendations_scale_with_the_market_premium() {
        assert_eq!(recommended_packet_tt(105.0, 10.0), Some(196.0));
        assert_eq!(recommended_packet_tt(110.0, 10.0), Some(98.0));
        assert_eq!(recommended_packet_tt(120.0, 10.0), Some(49.0));
        assert_eq!(recommended_packet_tt(200.0, 10.0), Some(9.80));
        assert_eq!(recommended_packet_tt(100.0, 10.0), None);
        assert_eq!(recommended_packet_tt(f64::NAN, 10.0), None);
        assert_eq!(recommended_packet_tt(110.0, 0.0), None);
    }
}
