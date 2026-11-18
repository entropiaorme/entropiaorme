//! Weighted stock allocation and confirmed-sale arithmetic.
//!
//! Two questions live here, both pure. When a quantity leaves holdings,
//! which source activities produced it? And when a listing sells, how much
//! markup did it actually realise, and how much of that describes the
//! activity rather than the player's market execution?
//!
//! Fungible stock has no identity: 100 boards in inventory are not the
//! particular boards any one swing yielded. Asking the player to pick source
//! units, or imposing FIFO or LIFO, would invent a fact the game does not
//! record. The composition of what is held is the honest answer, so a sale
//! consumes every source in proportion to what that source still has open.
//!
//! A source is one provenance bucket: a harvest yield tier for Tree Cutting,
//! or Hunting with an optional observed mob species. One item can hold both
//! at once (Nanocubes recycled from boards beside Nanocubes recycled from
//! hides), and a sale of it draws on each in proportion, crediting each
//! activity its own share. The observed target is descriptive evidence, not
//! a prerequisite for knowing that gameplay produced the stock.
//!
//! Allocation is at (provenance, context, tool) granularity, not per source
//! event. Hunting keeps its session definition beside species so the same
//! immutable sale can be projected through either honest comparison axis.
//! Per-event granularity would fan a sale of a few hundred units into hundreds
//! of rows nothing reads, and the movement schema carries a nullable
//! `source_event_id` for it if that ever changes.
//!
//! Stock a conversion produced carries more than a quantity: the markup paid
//! for bought inputs, any markup declared on the player's own inputs, and the
//! root item it descends from. Those facts are per unit, so an outflow consumes
//! them in the same proportion as the stock itself. [`split_realised`] then
//! says who a realised figure belongs to when some inputs were declared to be
//! worth a fixed amount and others were left to share whatever is left.

use crate::harvest_yield::HarvestYieldTier;

/// Sub-PED tolerance for treating a residual as closed. Quantities and TT
/// both round to hundredths at the display edge; anything under this is
/// float noise, not a real position.
const EPSILON: f64 = 1e-9;

/// Which activity a unit of stock traces back to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StockProvenance<'a> {
    /// Tree Cutting: the effective yield tier whose swings produced it.
    Harvest(HarvestYieldTier),
    /// Hunting: the activity is known even when the observed mob species is
    /// not. Species is an optional downstream analytical dimension only.
    Hunt(Option<&'a str>),
    /// A confirmed quest-item reward. The immutable acquisition, durable run,
    /// quest, and exact effort context travel together through every stock
    /// transformation and outflow.
    Quest {
        reward_item_id: i64,
        quest_run_id: i64,
        quest_id: i64,
        activity_context_id: Option<i64>,
    },
}

/// What a parcel of converted stock carries beyond its quantity and TT.
///
/// On a [`SourcePosition`] the figures are per unit; on a [`SourceAllocation`]
/// they are the total for that share. Stock that no conversion touched has the
/// default, all absent.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DerivedFacts<'a> {
    /// The conversion input this stock descends from, which keeps one
    /// conversion's terms from blending into another's.
    pub source_line_id: Option<i64>,
    /// The item this stock traces back to before any conversion.
    pub origin_item: Option<&'a str>,
    /// Markup paid for bought inputs that the eventual sale must recover
    /// before anything counts as gained.
    pub paid_markup: f64,
    /// The share of a declared worth this stock is carrying. Absent means the
    /// source takes part in whatever is left after declared worths are met.
    pub declared_markup: Option<f64>,
    /// How many of `origin_item` this stock accounts for.
    pub origin_quantity: Option<f64>,
    /// Set when these facts describe a whole asset rather than fungible stock.
    pub inventory_item_id: Option<&'a str>,
}

impl DerivedFacts<'_> {
    fn scaled(self, units: f64) -> Self {
        Self {
            paid_markup: self.paid_markup * units,
            declared_markup: self.declared_markup.map(|value| value * units),
            origin_quantity: self.origin_quantity.map(|value| value * units),
            ..self
        }
    }

    /// The same facts signed for the outflow that consumes them.
    #[must_use]
    pub fn outflow(self) -> Self {
        self.scaled(-1.0)
    }
}

/// One source's still-open position for a canonical item.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourcePosition<'a> {
    /// `None` for stock whose provenance is genuinely unknown (migrated
    /// overlay rows, opening balances). Never a guess.
    pub provenance: Option<StockProvenance<'a>>,
    /// Hunting's user-designated context. `None` for harvesting, genuinely
    /// unassigned hunting, and movements recorded before this dimension.
    pub session_definition_id: Option<i64>,
    /// The exact declared activity context that produced hunted stock.
    /// `None` for harvesting, unstamped hunting, and legacy positions.
    pub activity_context_id: Option<i64>,
    /// The tool that produced it. `None` when the swing recorded no tool,
    /// when the movement predates tool capture, or for hunted stock (a
    /// kill's loot is not a single tool's produce).
    pub tool_name: Option<&'a str>,
    pub quantity: f64,
    /// Per unit, so any share of the position takes its proportion.
    pub derived: DerivedFacts<'a>,
}

/// One source's share of an outflow.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceAllocation<'a> {
    pub provenance: Option<StockProvenance<'a>>,
    pub session_definition_id: Option<i64>,
    pub activity_context_id: Option<i64>,
    pub tool_name: Option<&'a str>,
    pub quantity: f64,
    pub tt_value: f64,
    pub derived: DerivedFacts<'a>,
}

/// How a requested outflow divides across open positions.
#[derive(Debug, Clone, PartialEq)]
pub struct AllocationPlan<'a> {
    pub allocations: Vec<SourceAllocation<'a>>,
    /// Quantity covered by open positions carrying a known provenance.
    pub attributed_qty: f64,
    pub attributed_tt: f64,
    /// Quantity beyond tracked stock, plus any consumed from explicitly
    /// unattributed positions. Real value with no activity claim on it.
    pub unattributed_qty: f64,
    pub unattributed_tt: f64,
    /// The part of `unattributed_qty` that no open position covered at all,
    /// as opposed to the part drawn from positions of unknown provenance.
    ///
    /// The two are the same to the activity (neither may be claimed), but not
    /// to the stock ledger: an unattributed draw leaves a position that was
    /// there, while this leaves a position the app never recorded. Booking it
    /// as an outflow alone would drive the item's holdings negative, so the
    /// caller records the acquisition it implies before recording the outflow.
    pub excess_qty: f64,
    pub excess_tt: f64,
    /// Paid markup the outflow consumes, which any sale of it must recover.
    pub paid_markup: f64,
}

/// Split `quantity` of an item across its open source positions, weighted by
/// how much each source still holds.
///
/// A request larger than the total open position is not an error: the player
/// may hold stock from before tracking began, or from another source. The
/// excess is carried explicitly as unattributed rather than being spread over
/// the sources that happen to be tracked, which would credit them with output
/// they never produced.
///
/// `unit_tt` is the item's TT per unit, which Entropia fixes per item, so
/// quantity share and TT share are the same ratio.
pub fn allocate<'a>(
    positions: &[SourcePosition<'a>],
    quantity: f64,
    unit_tt: f64,
) -> AllocationPlan<'a> {
    let open: Vec<SourcePosition<'a>> = positions
        .iter()
        .copied()
        .filter(|position| position.quantity > EPSILON)
        .collect();
    let available: f64 = open.iter().map(|position| position.quantity).sum();

    let attributable = quantity.min(available).max(0.0);
    let excess = (quantity - attributable).max(0.0);

    let mut allocations = Vec::with_capacity(open.len());
    let mut assigned = 0.0_f64;
    for (index, position) in open.iter().enumerate() {
        // The last open position absorbs the residual so the parts sum to the
        // whole exactly, rather than drifting by a float ulp per source.
        let share = if index + 1 == open.len() {
            attributable - assigned
        } else {
            attributable * (position.quantity / available)
        };
        if share <= EPSILON {
            continue;
        }
        assigned += share;
        allocations.push(SourceAllocation {
            provenance: position.provenance,
            session_definition_id: position.session_definition_id,
            activity_context_id: position.activity_context_id,
            tool_name: position.tool_name,
            quantity: share,
            tt_value: share * unit_tt,
            derived: position.derived.scaled(share),
        });
    }

    // A position with no known provenance is consumed like any other (the
    // player really is holding it), but what it funds cannot be claimed by an
    // activity, so it lands on the unattributed side of the split.
    let attributed_qty: f64 = allocations
        .iter()
        .filter(|allocation| allocation.provenance.is_some())
        .map(|allocation| allocation.quantity)
        .sum();
    let unknown_qty: f64 = allocations
        .iter()
        .filter(|allocation| allocation.provenance.is_none())
        .map(|allocation| allocation.quantity)
        .sum();

    if excess > EPSILON {
        allocations.push(SourceAllocation {
            provenance: None,
            session_definition_id: None,
            activity_context_id: None,
            tool_name: None,
            quantity: excess,
            tt_value: excess * unit_tt,
            derived: DerivedFacts::default(),
        });
    }

    let unattributed_qty = unknown_qty + excess;
    let paid_markup = allocations
        .iter()
        .map(|allocation| allocation.derived.paid_markup)
        .sum();
    AllocationPlan {
        allocations,
        attributed_qty,
        attributed_tt: attributed_qty * unit_tt,
        unattributed_qty,
        unattributed_tt: unattributed_qty * unit_tt,
        excess_qty: excess,
        excess_tt: excess * unit_tt,
        paid_markup,
    }
}

/// What a confirmed sale realised, split into what the ledger records and
/// what the activity may claim.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SaleOutcome {
    /// Sale proceeds above the whole listing's TT and any markup paid to
    /// produce it, and exactly what the ledger records as the gain.
    ///
    /// The TT itself is never income at sale time. For tracked stock it was
    /// already booked as loot when it dropped; for untracked stock the player
    /// still demonstrably held the value, so selling it converts a position
    /// rather than creating one. Netting off only the *tracked* TT would book
    /// the untracked units' principal as profit.
    pub gross_markup: f64,
    pub total_fees: f64,
    /// Gross markup after both auction fees.
    pub net_markup: f64,
    /// The fraction of the listing that tracked stock covered.
    pub attributed_share: f64,
    /// Net realised markup the activity may claim, to be divided across its
    /// contributing sources. The remainder of `net_markup` is the untracked
    /// share: real money, but with no activity able to claim it.
    pub activity_net_markup: f64,
}

/// Resolve a confirmed sale.
///
/// `tt_value` is the whole listing's TT and `attributed_quantity` the part of
/// its quantity covered by tracked stock. `paid_markup` is what bought inputs
/// cost above TT when the stock was produced by a conversion, and is zero for
/// everything else. This remains defined for zero-TT vouchers. Fees are what the game actually charged: the
/// starting-bid fee taken at listing, and the additional fee taken at the
/// point of sale when an item sells above its starting bid.
///
/// Fees reduce the activity's realised markup because they are the direct
/// cost of capturing it, and the point-of-sale fee scales with how far above
/// TT the output sold. A fee on a listing that *expired* is a different
/// animal and never reaches here: the stock came back, so it describes market
/// execution, not the gameplay.
pub fn resolve_sale(
    quantity: f64,
    attributed_quantity: f64,
    tt_value: f64,
    paid_markup: f64,
    final_price: f64,
    listing_fee: f64,
    sale_fee: f64,
) -> SaleOutcome {
    let gross_markup = final_price - tt_value - paid_markup;
    let total_fees = listing_fee + sale_fee;
    let net_markup = gross_markup - total_fees;
    let attributed_share = if quantity.abs() > EPSILON {
        (attributed_quantity / quantity).clamp(0.0, 1.0)
    } else {
        0.0
    };
    SaleOutcome {
        gross_markup,
        total_fees,
        net_markup,
        attributed_share,
        activity_net_markup: net_markup * attributed_share,
    }
}

/// One source's standing in a realised figure.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CreditLine {
    /// Its share among the sources that were left to take what remains.
    pub weight: f64,
    /// The amount it was declared to be worth, or `None` to take a share of
    /// whatever is left once declared worths are met.
    pub worth: Option<f64>,
}

/// Divide a realised figure across its sources.
///
/// Declared worths are met first, because the player has said what those
/// sources are worth and the rest is what the remaining sources earned. A
/// figure that falls short pays the declared sources pro rata and leaves the
/// others nothing, and a loss lands on the undeclared sources, which are the
/// ones the outcome was riding on. With no undeclared source, whatever is left
/// over goes to the declared ones in proportion to their worth.
///
/// The parts always sum to `net` exactly.
#[must_use]
pub fn split_realised(net: f64, lines: &[CreditLine]) -> Vec<f64> {
    let mut parts = vec![0.0; lines.len()];
    if lines.is_empty() {
        return parts;
    }
    let declared_total: f64 = lines
        .iter()
        .filter_map(|line| line.worth)
        .map(|worth| worth.max(0.0))
        .sum();
    let to_declared = if net > 0.0 {
        net.min(declared_total)
    } else {
        0.0
    };
    if declared_total > EPSILON {
        for (part, line) in parts.iter_mut().zip(lines) {
            if let Some(worth) = line.worth {
                *part = to_declared * (worth.max(0.0) / declared_total);
            }
        }
    }

    let remainder = net - to_declared;
    let blank_weight: f64 = lines
        .iter()
        .filter(|line| line.worth.is_none())
        .map(|line| line.weight)
        .sum();
    // Who takes the remainder, weighted: the undeclared sources, or failing
    // those the declared ones by worth, or failing that all of them by weight.
    let takes: Vec<f64> = if blank_weight > EPSILON {
        lines
            .iter()
            .map(|line| {
                if line.worth.is_none() {
                    line.weight
                } else {
                    0.0
                }
            })
            .collect()
    } else if declared_total > EPSILON {
        lines
            .iter()
            .map(|line| line.worth.unwrap_or(0.0).max(0.0))
            .collect()
    } else {
        lines.iter().map(|line| line.weight).collect()
    };
    let total_take: f64 = takes.iter().sum();
    if total_take > EPSILON {
        for (part, take) in parts.iter_mut().zip(&takes) {
            *part += remainder * (take / total_take);
        }
    } else if let Some(first) = parts.first_mut() {
        // Every weight is zero: nothing distinguishes the sources, but the
        // figure still has to land somewhere for the total to hold.
        *first += remainder;
    }

    // Float division can leave an ulp behind; the largest part absorbs it so
    // the sources reconcile with the ledger exactly.
    let residual = net - parts.iter().sum::<f64>();
    if residual != 0.0 {
        if let Some(largest) = parts.iter_mut().max_by(|a, b| {
            a.abs()
                .partial_cmp(&b.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        }) {
            *largest += residual;
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn tier(tier: HarvestYieldTier, quantity: f64) -> SourcePosition<'static> {
        SourcePosition {
            provenance: Some(StockProvenance::Harvest(tier)),
            session_definition_id: None,
            activity_context_id: None,
            tool_name: None,
            quantity,
            derived: DerivedFacts::default(),
        }
    }

    fn species(name: &str, quantity: f64) -> SourcePosition<'_> {
        SourcePosition {
            provenance: Some(StockProvenance::Hunt(Some(name))),
            session_definition_id: None,
            activity_context_id: None,
            tool_name: None,
            quantity,
            derived: DerivedFacts::default(),
        }
    }

    fn unclassified_hunt(quantity: f64) -> SourcePosition<'static> {
        SourcePosition {
            provenance: Some(StockProvenance::Hunt(None)),
            session_definition_id: None,
            activity_context_id: None,
            tool_name: None,
            quantity,
            derived: DerivedFacts::default(),
        }
    }

    fn tier_with_tool(tier: HarvestYieldTier, tool: &str, quantity: f64) -> SourcePosition<'_> {
        SourcePosition {
            provenance: Some(StockProvenance::Harvest(tier)),
            session_definition_id: None,
            activity_context_id: None,
            tool_name: Some(tool),
            quantity,
            derived: DerivedFacts::default(),
        }
    }

    fn total_quantity(plan: &AllocationPlan) -> f64 {
        plan.allocations
            .iter()
            .map(|allocation| allocation.quantity)
            .sum()
    }

    /// The whole requested quantity is always accounted for, whether or not
    /// tracked stock covers it.
    #[test]
    fn allocation_conserves_the_requested_quantity() {
        let positions = [
            tier(HarvestYieldTier::Short, 60.0),
            tier(HarvestYieldTier::Long, 40.0),
        ];
        for requested in [1.0, 33.0, 100.0, 250.0] {
            let plan = allocate(&positions, requested, 0.03);
            assert!(
                (total_quantity(&plan) - requested).abs() < 1e-6,
                "requested {requested} did not conserve"
            );
            assert!(
                (plan.attributed_qty + plan.unattributed_qty - requested).abs() < 1e-6,
                "requested {requested} split does not sum back"
            );
        }
    }

    /// Sources are consumed in proportion to what each still holds.
    #[test]
    fn allocation_is_weighted_by_open_position() {
        let positions = [
            tier(HarvestYieldTier::Short, 75.0),
            tier(HarvestYieldTier::Long, 25.0),
        ];
        let plan = allocate(&positions, 40.0, 0.01);

        let short = plan
            .allocations
            .iter()
            .find(|allocation| {
                allocation.provenance == Some(StockProvenance::Harvest(HarvestYieldTier::Short))
            })
            .expect("short tier allocated");
        let long = plan
            .allocations
            .iter()
            .find(|allocation| {
                allocation.provenance == Some(StockProvenance::Harvest(HarvestYieldTier::Long))
            })
            .expect("long tier allocated");

        assert!((short.quantity - 30.0).abs() < 1e-9);
        assert!((long.quantity - 10.0).abs() < 1e-9);
        assert!((short.tt_value - 0.30).abs() < 1e-9);
        assert!((plan.unattributed_qty).abs() < 1e-9);
    }

    /// Hunted stock attributes exactly like harvested stock: a species
    /// position is a first-class source, not a lesser copy of a tier.
    #[test]
    fn species_positions_attribute_like_tiers() {
        let positions = [species("Carabok", 80.0), species("Berycled", 20.0)];
        let plan = allocate(&positions, 50.0, 0.02);

        let carabok = plan
            .allocations
            .iter()
            .find(|allocation| {
                allocation.provenance == Some(StockProvenance::Hunt(Some("Carabok")))
            })
            .expect("carabok allocated");
        let berycled = plan
            .allocations
            .iter()
            .find(|allocation| {
                allocation.provenance == Some(StockProvenance::Hunt(Some("Berycled")))
            })
            .expect("berycled allocated");

        assert!((carabok.quantity - 40.0).abs() < 1e-9);
        assert!((berycled.quantity - 10.0).abs() < 1e-9);
        assert!((plan.attributed_qty - 50.0).abs() < 1e-9);
        assert!((plan.unattributed_qty).abs() < 1e-9);
    }

    /// Session definition is an orthogonal provenance dimension: stock from
    /// two routines that hunted the same species stays separately claimable.
    #[test]
    fn definitions_inside_one_species_are_allocated_separately() {
        let positions = [
            SourcePosition {
                provenance: Some(StockProvenance::Hunt(Some("Atrox"))),
                session_definition_id: Some(7),
                activity_context_id: Some(31),
                tool_name: None,
                quantity: 75.0,
                derived: DerivedFacts::default(),
            },
            SourcePosition {
                provenance: Some(StockProvenance::Hunt(Some("Atrox"))),
                session_definition_id: Some(9),
                activity_context_id: Some(32),
                tool_name: None,
                quantity: 25.0,
                derived: DerivedFacts::default(),
            },
        ];
        let plan = allocate(&positions, 40.0, 0.01);

        let first = plan
            .allocations
            .iter()
            .find(|allocation| allocation.session_definition_id == Some(7))
            .expect("first definition allocated");
        let second = plan
            .allocations
            .iter()
            .find(|allocation| allocation.session_definition_id == Some(9))
            .expect("second definition allocated");
        assert!((first.quantity - 30.0).abs() < 1e-9);
        assert!((second.quantity - 10.0).abs() < 1e-9);
        assert_eq!(first.provenance, Some(StockProvenance::Hunt(Some("Atrox"))));
        assert_eq!(first.activity_context_id, Some(31));
        assert_eq!(
            second.provenance,
            Some(StockProvenance::Hunt(Some("Atrox")))
        );
        assert_eq!(second.activity_context_id, Some(32));
    }

    /// Missing target evidence never turns known Hunting loot into external
    /// stock: activity attribution remains complete while the optional
    /// species dimension stays empty.
    #[test]
    fn unclassified_hunting_stock_remains_attributable() {
        let mut position = unclassified_hunt(1.0);
        position.session_definition_id = Some(7);
        let plan = allocate(&[position], 1.0, 1.0);

        assert_eq!(plan.attributed_qty, 1.0);
        assert_eq!(plan.attributed_tt, 1.0);
        assert_eq!(plan.unattributed_qty, 0.0);
        assert_eq!(
            plan.allocations[0].provenance,
            Some(StockProvenance::Hunt(None))
        );
        assert_eq!(plan.allocations[0].session_definition_id, Some(7));
    }

    /// An item holding both harvest and hunt provenance (Nanocubes recycled
    /// from boards beside Nanocubes recycled from hides) draws on each in
    /// proportion, so a joint pile credits each activity its own share.
    #[test]
    fn mixed_provenance_draws_on_both_activities() {
        let positions = [
            tier(HarvestYieldTier::Long, 300.0),
            species("Carabok", 100.0),
        ];
        let plan = allocate(&positions, 200.0, 0.01);

        let harvest_share: f64 = plan
            .allocations
            .iter()
            .filter(|allocation| matches!(allocation.provenance, Some(StockProvenance::Harvest(_))))
            .map(|allocation| allocation.quantity)
            .sum();
        let hunt_share: f64 = plan
            .allocations
            .iter()
            .filter(|allocation| matches!(allocation.provenance, Some(StockProvenance::Hunt(_))))
            .map(|allocation| allocation.quantity)
            .sum();

        assert!((harvest_share - 150.0).abs() < 1e-9);
        assert!((hunt_share - 50.0).abs() < 1e-9);
        assert!((plan.attributed_qty - 200.0).abs() < 1e-9);
    }

    /// Selling more than is held keeps the tracked part attributed and names
    /// the rest unattributed, instead of inflating the tracked sources.
    #[test]
    fn excess_beyond_stock_is_explicitly_unattributed() {
        let positions = [tier(HarvestYieldTier::Huge, 10.0)];
        let plan = allocate(&positions, 25.0, 0.06);

        assert!((plan.attributed_qty - 10.0).abs() < 1e-9);
        assert!((plan.unattributed_qty - 15.0).abs() < 1e-9);
        assert!((plan.attributed_tt - 0.60).abs() < 1e-9);
        assert!((plan.unattributed_tt - 0.90).abs() < 1e-9);
        assert_eq!(
            plan.allocations
                .iter()
                .filter(|allocation| allocation.provenance.is_none())
                .count(),
            1,
            "the excess is one explicit unattributed row"
        );
    }

    /// Stock drawn from an unattributed position is unattributed but not
    /// excess: the position existed. Only quantity no position covered is
    /// excess, and the caller needs the two apart to keep holdings off
    /// negative.
    #[test]
    fn excess_is_only_the_part_no_position_covered() {
        let positions = [
            tier(HarvestYieldTier::Short, 10.0),
            SourcePosition {
                provenance: None,
                session_definition_id: None,
                activity_context_id: None,
                tool_name: None,
                quantity: 5.0,
                derived: DerivedFacts::default(),
            },
        ];
        let plan = allocate(&positions, 20.0, 0.03);

        assert!((plan.attributed_qty - 10.0).abs() < 1e-9);
        // 5 drawn from the unattributed position, 5 covered by nothing at all.
        assert!((plan.unattributed_qty - 10.0).abs() < 1e-9);
        assert!((plan.excess_qty - 5.0).abs() < 1e-9);
        assert!((plan.excess_tt - 0.15).abs() < 1e-9);

        // A sale within tracked stock has no excess to make good.
        let within = allocate(&positions, 12.0, 0.03);
        assert!(within.excess_qty.abs() < 1e-9);
        assert!(
            within.unattributed_qty > 0.0,
            "still draws on the unattributed position"
        );
    }

    /// With nothing tracked at all, the whole sale is unattributed rather
    /// than silently credited somewhere.
    #[test]
    fn selling_with_no_tracked_stock_attributes_nothing() {
        let plan = allocate(&[], 12.0, 0.03);
        assert!((plan.attributed_qty).abs() < 1e-9);
        assert!((plan.unattributed_qty - 12.0).abs() < 1e-9);
    }

    /// Stock migrated without provenance is consumed, but funds no activity.
    #[test]
    fn unattributed_positions_consume_but_do_not_attribute() {
        let positions = [
            tier(HarvestYieldTier::Short, 50.0),
            SourcePosition {
                provenance: None,
                session_definition_id: None,
                activity_context_id: None,
                tool_name: None,
                quantity: 50.0,
                derived: DerivedFacts::default(),
            },
        ];
        let plan = allocate(&positions, 100.0, 0.01);

        assert!((plan.attributed_qty - 50.0).abs() < 1e-9);
        assert!((plan.unattributed_qty - 50.0).abs() < 1e-9);
        assert!((total_quantity(&plan) - 100.0).abs() < 1e-9);
    }

    /// Two tools working the same tier are separate positions, so a sale
    /// draws on each in proportion to what it still holds rather than
    /// crediting the tier and leaving the tools uncredited.
    #[test]
    fn tools_inside_one_tier_are_allocated_separately() {
        let positions = [
            tier_with_tool(HarvestYieldTier::Long, "PH-3", 30.0),
            tier_with_tool(HarvestYieldTier::Long, "PH-4", 10.0),
        ];
        let plan = allocate(&positions, 20.0, 0.03);

        assert_eq!(plan.allocations.len(), 2);
        let ph3 = plan
            .allocations
            .iter()
            .find(|a| a.tool_name == Some("PH-3"))
            .expect("PH-3 allocated");
        let ph4 = plan
            .allocations
            .iter()
            .find(|a| a.tool_name == Some("PH-4"))
            .expect("PH-4 allocated");
        assert!((ph3.quantity - 15.0).abs() < 1e-9);
        assert!((ph4.quantity - 5.0).abs() < 1e-9);
        // Both still belong to the tier that owns them.
        assert_eq!(
            ph3.provenance,
            Some(StockProvenance::Harvest(HarvestYieldTier::Long))
        );
        assert_eq!(
            ph4.provenance,
            Some(StockProvenance::Harvest(HarvestYieldTier::Long))
        );
        assert!((plan.attributed_qty - 20.0).abs() < 1e-9);
    }

    /// Exhausted sources do not produce zero-quantity allocation rows.
    #[test]
    fn closed_positions_are_skipped() {
        let positions = [
            tier(HarvestYieldTier::Short, 0.0),
            tier(HarvestYieldTier::Long, 20.0),
        ];
        let plan = allocate(&positions, 5.0, 0.03);
        assert_eq!(plan.allocations.len(), 1);
        assert_eq!(
            plan.allocations[0].provenance,
            Some(StockProvenance::Harvest(HarvestYieldTier::Long))
        );
    }

    /// A fully tracked sale gives the activity the whole net markup, and the
    /// ledger only the money that was not already booked as loot TT.
    #[test]
    fn fully_tracked_sale_attributes_all_net_markup() {
        let outcome = resolve_sale(300.0, 300.0, 3.00, 0.0, 5.00, 0.50, 0.20);

        assert!((outcome.gross_markup - 2.00).abs() < 1e-9);
        assert!((outcome.net_markup - 1.30).abs() < 1e-9);
        assert!((outcome.attributed_share - 1.0).abs() < 1e-9);
        assert!((outcome.activity_net_markup - 1.30).abs() < 1e-9);
        // The 3.00 TT counted as loot when it dropped; only the uplift is new.
        assert!((outcome.gross_markup - 2.00).abs() < 1e-9);
    }

    /// A partly tracked sale gives the activity only its share, and the
    /// remainder reconciles exactly against the global figure.
    #[test]
    fn partly_tracked_sale_reconciles_against_the_ledger() {
        let outcome = resolve_sale(450.0, 300.0, 4.50, 0.0, 7.50, 0.50, 0.20);

        assert!((outcome.attributed_share - 2.0 / 3.0).abs() < 1e-9);
        assert!((outcome.net_markup - 2.30).abs() < 1e-9);
        assert!((outcome.activity_net_markup - 2.30 * 2.0 / 3.0).abs() < 1e-9);
        // The ledger records markup only. The untracked units' 1.50 of TT was
        // value the player already held; selling it converts a position and
        // must not read as profit.
        assert!((outcome.gross_markup - 3.00).abs() < 1e-9);

        // The activity claims its share; the remainder is untracked markup,
        // real money that no activity may claim.
        let global_only = outcome.net_markup - outcome.activity_net_markup;
        assert!((global_only - 2.30 / 3.0).abs() < 1e-9);
    }

    /// Selling below TT is a real loss and must stay negative, not clamp.
    #[test]
    fn a_sale_under_tt_realises_a_negative_markup() {
        let outcome = resolve_sale(300.0, 300.0, 3.00, 0.0, 2.50, 0.50, 0.0);
        assert!((outcome.gross_markup + 0.50).abs() < 1e-9);
        assert!((outcome.net_markup + 1.00).abs() < 1e-9);
        assert!(outcome.activity_net_markup < 0.0);
    }

    /// Nothing tracked means the activity claims nothing, however large the
    /// sale, while the ledger still records the full proceeds.
    #[test]
    fn untracked_sale_claims_nothing_for_the_activity() {
        let outcome = resolve_sale(400.0, 0.0, 4.00, 0.0, 9.00, 0.50, 0.30);
        assert!((outcome.attributed_share).abs() < 1e-9);
        assert!((outcome.activity_net_markup).abs() < 1e-9);
        // Markup only, even with nothing tracked: the 4.00 of TT sold was
        // still value the player held.
        assert!((outcome.gross_markup - 5.00).abs() < 1e-9);
    }

    /// A zero-TT reward still has quantity, so all of its realised markup can
    /// return to the activity that earned it.
    #[test]
    fn zero_tt_listing_attributes_by_quantity() {
        let outcome = resolve_sale(10.0, 10.0, 0.0, 0.0, 1.00, 0.50, 0.0);
        assert!(outcome.attributed_share.is_finite());
        assert!((outcome.attributed_share - 1.0).abs() < 1e-9);
        assert!((outcome.activity_net_markup - 0.50).abs() < 1e-9);
    }

    /// Stock a conversion produced gives up its paid markup and declared worth
    /// in proportion, however it is sliced.
    #[test]
    fn an_outflow_carries_its_share_of_a_lot() {
        let positions = [SourcePosition {
            provenance: Some(StockProvenance::Hunt(None)),
            session_definition_id: Some(5),
            activity_context_id: None,
            tool_name: None,
            quantity: 4.0,
            derived: DerivedFacts {
                source_line_id: Some(9),
                origin_item: Some("Voucher"),
                paid_markup: 2.5,
                declared_markup: Some(1.25),
                origin_quantity: Some(20.0),
                inventory_item_id: None,
            },
        }];
        let plan = allocate(&positions, 2.0, 50.0);
        assert!((plan.paid_markup - 5.0).abs() < 1e-9);
        let derived = plan.allocations[0].derived;
        assert!((derived.paid_markup - 5.0).abs() < 1e-9);
        assert!((derived.declared_markup.unwrap() - 2.5).abs() < 1e-9);
        assert!((derived.origin_quantity.unwrap() - 40.0).abs() < 1e-9);
        assert_eq!(derived.origin_item, Some("Voucher"));
        assert!((derived.outflow().paid_markup + 5.0).abs() < 1e-9);
    }

    /// Markup paid to produce the stock is recovered before anything counts
    /// as gained: a 50 TT harness carrying 10 of paid markup, sold at 220.
    #[test]
    fn paid_markup_comes_off_the_gain_before_fees() {
        let outcome = resolve_sale(1.0, 1.0, 50.0, 10.0, 220.0, 0.0, 8.0);
        assert!((outcome.gross_markup - 160.0).abs() < 1e-9);
        assert!((outcome.net_markup - 152.0).abs() < 1e-9);
        assert!((outcome.activity_net_markup - 152.0).abs() < 1e-9);
    }

    fn blank(weight: f64) -> CreditLine {
        CreditLine {
            weight,
            worth: None,
        }
    }

    fn worth(amount: f64) -> CreditLine {
        CreditLine {
            weight: 1.0,
            worth: Some(amount),
        }
    }

    /// Declared inputs get their figure; whoever was left blank takes the rest.
    #[test]
    fn declared_worth_is_met_first_and_the_rest_goes_to_blank_lines() {
        let parts = split_realised(162.0, &[blank(1.0), worth(5.0), worth(5.0)]);
        assert!((parts[0] - 152.0).abs() < 1e-9);
        assert!((parts[1] - 5.0).abs() < 1e-9);
        assert!((parts[2] - 5.0).abs() < 1e-9);
    }

    /// A sale that cannot cover the declared worths pays them pro rata and
    /// leaves the blank source nothing.
    #[test]
    fn a_short_sale_pays_declared_lines_pro_rata() {
        let parts = split_realised(4.0, &[blank(1.0), worth(5.0), worth(15.0)]);
        assert!(parts[0].abs() < 1e-9);
        assert!((parts[1] - 1.0).abs() < 1e-9);
        assert!((parts[2] - 3.0).abs() < 1e-9);
    }

    /// A loss is never charged against a declared worth while a blank source
    /// exists to carry it.
    #[test]
    fn a_loss_lands_on_blank_lines() {
        let parts = split_realised(-6.0, &[blank(1.0), worth(5.0)]);
        assert!((parts[0] + 6.0).abs() < 1e-9);
        assert!(parts[1].abs() < 1e-9);
    }

    #[test]
    fn with_no_blank_line_the_surplus_follows_declared_worth() {
        let parts = split_realised(40.0, &[worth(5.0), worth(15.0)]);
        assert!((parts[0] - 10.0).abs() < 1e-9);
        assert!((parts[1] - 30.0).abs() < 1e-9);
    }

    #[test]
    fn blank_lines_share_by_weight() {
        let parts = split_realised(9.0, &[blank(2.0), blank(1.0)]);
        assert!((parts[0] - 6.0).abs() < 1e-9);
        assert!((parts[1] - 3.0).abs() < 1e-9);
    }

    proptest! {
        /// The sources always add back to the figure being divided, so a
        /// conversion can never create or lose value in the split.
        #[test]
        fn a_split_always_reconciles(
            net in -5_000.0f64..5_000.0,
            lines in prop::collection::vec(
                (0.0f64..10.0, prop::option::of(0.0f64..500.0)),
                1..8,
            ),
        ) {
            let lines: Vec<CreditLine> = lines
                .into_iter()
                .map(|(weight, worth)| CreditLine { weight, worth })
                .collect();
            let parts = split_realised(net, &lines);
            prop_assert!((parts.iter().sum::<f64>() - net).abs() < 1e-6);
        }

        /// While any blank source exists, a declared line never receives more
        /// than it was declared to be worth, and never carries a loss.
        #[test]
        fn declared_lines_stay_within_their_worth(
            net in -5_000.0f64..5_000.0,
            declared in prop::collection::vec(0.0f64..500.0, 1..6),
        ) {
            let mut lines = vec![blank(1.0)];
            lines.extend(declared.iter().map(|amount| worth(*amount)));
            let parts = split_realised(net, &lines);
            for (part, amount) in parts[1..].iter().zip(&declared) {
                prop_assert!(*part >= -1e-6);
                prop_assert!(*part <= amount + 1e-6);
            }
        }
    }
}
