//! What a session cost: the one place its cost buckets are derived, and
//! where a bucket the account readings measured takes the place of the
//! tracked figure.
//!
//! The tracked buckets come from the chat log: the weapon's shots on each
//! kill, the enhancer breaks on each kill and those after the last one
//! (`crate::enhancer_breaks`), each harvesting swing's decay, and the
//! session's armour, healing, consumed doses and dangling spend (shots
//! after the last kill). [`tracked`] is the only derivation of them; every
//! reader that sums a session's buckets goes through it.
//!
//! Where the readings measured a bucket, the measured amount is the
//! bucket's cost, and the tracked events only share it out below the
//! session: each kill's weapon cost, each break, each swing is weighed by
//! its bucket's scale (the measured amount over the tracked one), so the
//! parts of a session add up to its measured bucket. The weapon bucket
//! measures every shot fired, so its scale carries the dangling spend as
//! well as the kills. A measured bucket with no tracked events to carry it
//! sits whole at the session's grain. Armour, healing and consumed doses
//! are session-grain buckets already: the measured amount replaces the
//! session's figure.
//!
//! Armour is billed in parts: unlimited armour at what it lost, and each
//! limited set whose pieces the player confirmed at its pieces' loss and
//! markup. When a limited piece no confirmed set holds fell, the limited
//! part is not measured while the rest is: the session's armour is then
//! the measured parts plus what the player's recordings of the limited
//! sets the readings did not price spread over the session, and reads as
//! mixed. A recording that reached a measured part is never added to it:
//! the measured figure stands, and the old share is not spread elsewhere.
//!
//! A session whose Universal Ammo was topped up mid-session (it arrived
//! with no Shrapnel conversion and no strongbox opening behind it) has a
//! weapon bucket the readings measure but for the ammo fired in the steps
//! it arrived in. Its weapon is then mixed: the measured amount, and the
//! tracked spend of the kills logged inside those steps, together shared
//! out over the kills and the dangling spend like a measured weapon. The
//! readings leave those steps whole to the tracked figure, the weapon's
//! wear in them included, since a tracked shot's price holds it. A
//! kill's shots are timed by the kill itself, so a kill fought across the
//! edge of such a step counts whole on the side it ended. Shots after the
//! last kill carry no time at all: when a top-up step reaches past the last
//! kill and the session has such shots, the weapon keeps its tracked figure
//! whole.
//!
//! A session the readings measured across a gap (its own readings stop
//! before it ends, or start after it began, and the chat log shows the gap
//! clean) resolves like one measured whole, and says how long the gap was.
//!
//! What a session cost beyond performing its activity is its overhead:
//! travel, field logistics, and what the app cannot place yet, as the
//! readings measured them. It has no tracked counterpart: a session whose
//! overhead was not measured has none, and says nothing of it. Overhead is
//! never part of Cycled ([`BucketCosts`]); it is kept beside it
//! ([`Overhead`]) for the readers that show Costs and Net, and shared below
//! the session by what each part cycled
//! (`crate::analytics::realised`).
//!
//! A bucket keeps its tracked figure, and says why, when the readings did
//! not measure it. A measured bucket is the cost even at nothing: an item
//! whose wear the app could not place is charged where the player places
//! it, never twice. A session with no current measured costs (still
//! running, read in part, never read, or read by rules since replaced)
//! resolves to its tracked figures throughout, exactly as if no
//! measurement existed, and so does one whose stored costs cannot be read
//! (logged, so one bad row never stops the rest serving).
//!
//! A session still running has no measured costs yet, but the account
//! side publishes what its readings measured up to the latest of them
//! ([`measured_so_far`]). Its live figures ([`live`]) take that, and the
//! tracked cost of the events after that reading, so they do not step when
//! the sealed costs replace them at the session's close. Between its stop
//! and its seal the session keeps the same blend, from its stored events
//! after that reading, and the seal hands over. The site shows play some
//! seconds late (about 20 in the player's own walk): a reading can miss the
//! last moments before it, which neither side then counts, so the blend
//! can run low by up to about 20 seconds of spend around each reading. The
//! seal settles it.
//!
//! The measured costs are read from the account module's stored rows by
//! plain SQL, here and nowhere else on the accounting surface (the
//! `measured-isolation` guard). What they resolve to is kept per session
//! (`session_cost_resolutions`), so readers that aggregate many sessions
//! in SQL weigh tracked events through [`scaled`] and its siblings without
//! re-deriving anything. The resolution is rebuilt whenever the session's
//! summary is written, and [`heal`] catches a measurement written, rebuilt
//! or dropped since.

use std::collections::HashMap;

use rusqlite::OptionalExtension as _;

use crate::db::DbError;

/// The version of the resolution's rules. A stored resolution of another
/// version is rebuilt by [`heal`].
// Bumped to 4 when a session's measured overhead came to be kept on its
// resolution, by category. Bumped to 5 when a weapon whose ammo was topped
// up came to be kept mixed, with its parts.
pub const RESOLUTION_VERSION: i64 = 5;

/// The version of the stored measured costs this resolver reads; any other
/// is stale until the account side rebuilds it, and resolves to tracked.
// Bumped to 4 with the account side's costs, when the player's placements
// came to decide where an item's use costs and a limited piece's line came
// to name the set that priced it. Bumped to 5 with them when the PED Card
// stopped holding a bucket back and a weapon whose ammo was topped up came
// to be stored mixed, with its top-up steps. Bumped to 6 with them when a
// limited piece came to be billed at its own markup, or at its TT.
pub const MEASURED_COSTS_VERSION: i64 = 6;

/// The version of the measure those costs must have been derived from.
// Bumped to 6 with the measure, when the PED Card stopped deciding what a
// step used up and top-up steps came to be left out of the ammo's split.
pub const MEASURED_MEASURE_VERSION: i64 = 6;

const RESOLUTIONS: &str = "session_cost_resolutions";
const RESOLUTION_BUCKETS: &str = "session_cost_resolution_buckets";
const RESOLUTION_ARMOUR: &str = "session_cost_resolution_armour";
const RESOLUTION_WEAPON: &str = "session_cost_resolution_weapon";

/// A bucket of Cycled that the readings can measure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Bucket {
    /// The shots fired: on kills, and dangling after the last one.
    Weapon,
    Enhancer,
    Harvesting,
    Healing,
    Armour,
    Consumables,
}

impl Bucket {
    pub const ALL: [Bucket; 6] = [
        Bucket::Weapon,
        Bucket::Enhancer,
        Bucket::Harvesting,
        Bucket::Healing,
        Bucket::Armour,
        Bucket::Consumables,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Bucket::Weapon => "weapon",
            Bucket::Enhancer => "enhancer",
            Bucket::Harvesting => "harvesting",
            Bucket::Healing => "healing",
            Bucket::Armour => "armour",
            Bucket::Consumables => "consumables",
        }
    }

    pub fn parse(value: &str) -> Option<Bucket> {
        Bucket::ALL
            .into_iter()
            .find(|bucket| bucket.as_str() == value)
    }

    /// Whether the chat log tracks the bucket event by event (so a measured
    /// amount is shared out over those events), rather than per session.
    pub fn tracked_by_event(self) -> bool {
        matches!(self, Bucket::Weapon | Bucket::Enhancer | Bucket::Harvesting)
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// A category of a session's overhead: what it cost beyond performing its
/// activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum OverheadCategory {
    /// Teleport chips, Mind Essence, vehicles and their fuel.
    Travel,
    /// Portable repair and Trade Terminal units, and their like.
    FieldLogistics,
    /// What the readings measured as used up that the app cannot place.
    Unclassified,
}

impl OverheadCategory {
    pub const ALL: [OverheadCategory; 3] = [
        OverheadCategory::Travel,
        OverheadCategory::FieldLogistics,
        OverheadCategory::Unclassified,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            OverheadCategory::Travel => "travel",
            OverheadCategory::FieldLogistics => "fieldLogistics",
            OverheadCategory::Unclassified => "unclassified",
        }
    }

    pub fn parse(value: &str) -> Option<OverheadCategory> {
        OverheadCategory::ALL
            .into_iter()
            .find(|category| category.as_str() == value)
    }

    /// The resolution's column keeping the category's measured amount.
    fn column(self) -> &'static str {
        match self {
            OverheadCategory::Travel => "overhead_travel_ped",
            OverheadCategory::FieldLogistics => "overhead_field_logistics_ped",
            OverheadCategory::Unclassified => "overhead_unclassified_ped",
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// A session's overhead in PED, by category: what the readings measured,
/// and nothing where they did not.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Overhead {
    pub travel: f64,
    pub field_logistics: f64,
    pub unclassified: f64,
}

impl Overhead {
    /// The overhead of one category.
    pub fn of(&self, category: OverheadCategory) -> f64 {
        match category {
            OverheadCategory::Travel => self.travel,
            OverheadCategory::FieldLogistics => self.field_logistics,
            OverheadCategory::Unclassified => self.unclassified,
        }
    }

    /// Every category, summed in one fixed order.
    pub fn total(&self) -> f64 {
        self.travel + self.field_logistics + self.unclassified
    }

    fn from_amounts(amounts: [Option<f64>; 3]) -> Overhead {
        let [travel, field_logistics, unclassified] = amounts.map(|amount| amount.unwrap_or(0.0));
        Overhead {
            travel,
            field_logistics,
            unclassified,
        }
    }
}

/// A bucket as the stored measured costs name it: one of Cycled's, or one
/// of the overhead categories, which no Cycled figure includes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StoredBucket {
    Cycled(Bucket),
    Overhead(OverheadCategory),
}

impl StoredBucket {
    pub fn parse(value: &str) -> Option<StoredBucket> {
        OverheadCategory::parse(value)
            .map(StoredBucket::Overhead)
            .or_else(|| Bucket::parse(value).map(StoredBucket::Cycled))
    }
}

/// Why a bucket keeps its tracked figure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TrackedReason {
    /// The session has no current measured costs: still running, never
    /// read, or read by rules since replaced.
    NoMeasuredCosts,
    /// The readings do not read one of the session's ends.
    PartialCoverage,
    /// Two consecutive readings are too far apart.
    Gap,
    /// How much of the ammo's change was burned is not known.
    AmmoUnsettled,
    /// Universal Ammo was topped up mid-session: the ammo fired in the
    /// steps it arrived in is the tracked figure's. Beside a measured part
    /// (a mixed weapon) or, when the tracked spend of those steps cannot be
    /// told apart, for the whole bucket.
    AmmoToppedUp,
    /// A limited piece lost TT, and pricing it needs its set confirmed.
    LimitedSetUnconfirmed,
    /// The readings never measure this bucket.
    TrackedOnly,
    /// An end is unread across a gap, and the chat log shows play outside
    /// the session in it.
    PlayInGap,
    /// An end is unread across a gap the chat log does not cover.
    ChatLogMissesGap,
    /// An end is unread across a gap the chat log could not be checked
    /// over.
    ChatLogUnchecked,
    /// An end is unread across a gap whose readings hold a repair, or a
    /// cost of a kind the session's own play never had.
    GapUnaccounted,
}

impl TrackedReason {
    /// The reasons a resolution stores (every one but the session-wide
    /// absence, which stores nothing).
    pub const STORED: [TrackedReason; 10] = [
        TrackedReason::PartialCoverage,
        TrackedReason::Gap,
        TrackedReason::AmmoUnsettled,
        TrackedReason::AmmoToppedUp,
        TrackedReason::LimitedSetUnconfirmed,
        TrackedReason::TrackedOnly,
        TrackedReason::PlayInGap,
        TrackedReason::ChatLogMissesGap,
        TrackedReason::ChatLogUnchecked,
        TrackedReason::GapUnaccounted,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            TrackedReason::NoMeasuredCosts => "noMeasuredCosts",
            TrackedReason::PartialCoverage => "partialCoverage",
            TrackedReason::Gap => "gap",
            TrackedReason::AmmoUnsettled => "ammoUnsettled",
            TrackedReason::AmmoToppedUp => "ammoToppedUp",
            TrackedReason::LimitedSetUnconfirmed => "limitedSetUnconfirmed",
            TrackedReason::TrackedOnly => "trackedOnly",
            TrackedReason::PlayInGap => "playInGap",
            TrackedReason::ChatLogMissesGap => "chatLogMissesGap",
            TrackedReason::ChatLogUnchecked => "chatLogUnchecked",
            TrackedReason::GapUnaccounted => "gapUnaccounted",
        }
    }

    pub fn parse(value: &str) -> Option<TrackedReason> {
        TrackedReason::STORED
            .into_iter()
            .find(|reason| reason.as_str() == value)
    }
}

/// What one bucket's figure rests on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Basis {
    /// The readings measured it: this many PED.
    Measured(f64),
    /// The chat log's figure stands, for this reason.
    Tracked(TrackedReason),
    /// Part measured (`measured` PED), part tracked (`tracked` PED), for
    /// `reason`. Armour: the readings measured the unlimited armour and
    /// every confirmed limited set, and the limited sets they could not
    /// price carry what the player's recordings spread over the session.
    /// The weapon: the readings measured everything but the ammo fired in
    /// the steps it was topped up in, which carry the tracked spend of the
    /// kills logged inside them.
    Mixed {
        measured: f64,
        tracked: f64,
        reason: TrackedReason,
    },
}

/// What a figure summed from buckets, or from sessions, rests on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CostBasis {
    Measured,
    Tracked,
    /// Part measured, part tracked.
    Mixed,
}

impl CostBasis {
    /// The basis of a sum of two figures.
    pub fn and(self, other: CostBasis) -> CostBasis {
        if self == other {
            self
        } else {
            CostBasis::Mixed
        }
    }

    pub fn of(basis: Basis) -> CostBasis {
        match basis {
            Basis::Measured(_) => CostBasis::Measured,
            Basis::Tracked(_) => CostBasis::Tracked,
            Basis::Mixed { .. } => CostBasis::Mixed,
        }
    }
}

/// A session's cost buckets in PED.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BucketCosts {
    pub weapon: f64,
    pub enhancer: f64,
    pub armour: f64,
    pub heal: f64,
    pub consumable: f64,
    /// Shots after the last kill: weapon spend no kill carries.
    pub dangling: f64,
    pub harvest: f64,
}

impl BucketCosts {
    /// The session's Cycled: every bucket, summed in one fixed order.
    pub fn cycled(&self) -> f64 {
        self.weapon
            + self.enhancer
            + self.armour
            + self.heal
            + self.consumable
            + self.dangling
            + self.harvest
    }

    /// The tracked amount a bucket's measurement answers: the weapon's
    /// counts its dangling spend too.
    pub fn of(&self, bucket: Bucket) -> f64 {
        match bucket {
            Bucket::Weapon => self.weapon + self.dangling,
            Bucket::Enhancer => self.enhancer,
            Bucket::Harvesting => self.harvest,
            Bucket::Healing => self.heal,
            Bucket::Armour => self.armour,
            Bucket::Consumables => self.consumable,
        }
    }
}

/// One bucket as resolved and kept.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Resolved {
    basis: Basis,
    /// What each tracked event's cost is multiplied by; `None` where the
    /// events carry nothing of a measured amount (it sits at the session's
    /// grain) or the bucket is tracked.
    scale: Option<f64>,
}

/// A session the readings measured across a gap the chat log showed clean.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcrossGap {
    /// How long the readings were missing, in whole seconds.
    pub gap_seconds: i64,
}

/// A session's costs: tracked, resolved, and what each bucket rests on;
/// and its overhead, beside Cycled.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionCosts {
    pub tracked: BucketCosts,
    pub resolved: BucketCosts,
    buckets: [Resolved; 6],
    /// Set when the measured buckets were measured across a gap.
    pub across_gap: Option<AcrossGap>,
    /// What the session cost beyond performing its activity: in its Costs
    /// and its Net, never in Cycled.
    pub overhead: Overhead,
}

impl SessionCosts {
    fn tracked_only(tracked: BucketCosts) -> SessionCosts {
        SessionCosts {
            tracked,
            resolved: tracked,
            buckets: [Resolved {
                basis: Basis::Tracked(TrackedReason::NoMeasuredCosts),
                scale: None,
            }; 6],
            across_gap: None,
            overhead: Overhead::default(),
        }
    }

    fn with(tracked: BucketCosts, kept: KeptResolution) -> SessionCosts {
        let (buckets, across_gap, overhead) = kept;
        let mut resolved = tracked;
        for bucket in Bucket::ALL {
            let entry = buckets[bucket.index()];
            let measured = match entry.basis {
                Basis::Measured(measured) => measured,
                Basis::Mixed {
                    measured, tracked, ..
                } if bucket == Bucket::Armour => {
                    resolved.armour = measured + tracked;
                    continue;
                }
                // The weapon's parts are shared out as one measured figure.
                Basis::Mixed {
                    measured, tracked, ..
                } if bucket == Bucket::Weapon => measured + tracked,
                Basis::Mixed { .. } | Basis::Tracked(_) => continue,
            };
            match (bucket, entry.scale) {
                (Bucket::Weapon, Some(scale)) => {
                    resolved.weapon = tracked.weapon * scale;
                    resolved.dangling = tracked.dangling * scale;
                }
                (Bucket::Weapon, None) => {
                    resolved.weapon = measured;
                    resolved.dangling = 0.0;
                }
                (Bucket::Enhancer, Some(scale)) => resolved.enhancer = tracked.enhancer * scale,
                (Bucket::Enhancer, None) => resolved.enhancer = measured,
                (Bucket::Harvesting, Some(scale)) => resolved.harvest = tracked.harvest * scale,
                (Bucket::Harvesting, None) => resolved.harvest = measured,
                (Bucket::Healing, _) => resolved.heal = measured,
                (Bucket::Armour, _) => resolved.armour = measured,
                (Bucket::Consumables, _) => resolved.consumable = measured,
            }
        }
        SessionCosts {
            tracked,
            resolved,
            buckets,
            across_gap,
            overhead,
        }
    }

    /// The session's Costs: its Cycled and its overhead.
    pub fn costs(&self) -> f64 {
        self.resolved.cycled() + self.overhead.total()
    }

    /// What the bucket's figure rests on.
    pub fn basis(&self, bucket: Bucket) -> Basis {
        self.buckets[bucket.index()].basis
    }

    /// The bucket's figure's basis, as a sum would combine it.
    pub fn cost_basis(&self, bucket: Bucket) -> CostBasis {
        CostBasis::of(self.basis(bucket))
    }

    /// The gap the session's measured figures were measured across, when
    /// any of its Cycled rests on them: a session whose Cycled is tracked
    /// throughout says nothing of a gap it shows no figure from.
    pub fn shown_across_gap(&self) -> Option<AcrossGap> {
        self.across_gap
            .filter(|_| self.cycled_basis() != CostBasis::Tracked)
    }

    /// What the session's Cycled rests on: the bases of the buckets that
    /// hold anything (of every bucket, when none does).
    pub fn cycled_basis(&self) -> CostBasis {
        let holding: Vec<Bucket> = Bucket::ALL
            .into_iter()
            .filter(|bucket| self.resolved.of(*bucket) != 0.0)
            .collect();
        let buckets = if holding.is_empty() {
            Bucket::ALL.to_vec()
        } else {
            holding
        };
        buckets
            .into_iter()
            .map(|bucket| self.cost_basis(bucket))
            .reduce(CostBasis::and)
            .unwrap_or(CostBasis::Tracked)
    }

    /// What a tracked event of the bucket weighs below the session: its
    /// tracked cost times this.
    pub fn scale(&self, bucket: Bucket) -> f64 {
        self.buckets[bucket.index()].scale.unwrap_or(1.0)
    }
}

// ── The tracked buckets ─────────────────────────────────────────────

/// The tracked weapon spend on the kills of the session `session` (an SQL
/// expression for its id): every phase's shots at their price.
fn kill_weapon_sql(session: &str) -> String {
    // every-kill: spend (a claimed arrival's shots are real spend).
    format!(
        "(SELECT COALESCE(SUM(COALESCE(ts.cost_per_shot, 0) * COALESCE(ts.shots_fired, 0)), 0) \
          FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id \
          WHERE k.session_id = {session})"
    )
}

/// The tracked enhancer breaks of the session `session`: those its kills
/// carry, and those after its last kill, which the session carries.
fn enhancer_sql(session: &str) -> String {
    // every-kill: spend (every break is spend).
    format!(
        "((SELECT COALESCE(SUM(enhancer_cost), 0) FROM kills WHERE session_id = {session}) \
          + COALESCE({}, 0))",
        session_breaks_sql(session)
    )
}

/// The cost of the priced enhancer breaks no kill of the session `session`
/// carries, or NULL when it has none that cost anything.
fn session_breaks_sql(session: &str) -> String {
    format!(
        "(SELECT SUM(cost_ped) FROM enhancer_breaks \
          WHERE session_id = {session} AND kill_id IS NULL AND cost_ped != 0)"
    )
}

/// The tracked harvesting decay of the session `session`.
fn harvest_sql(session: &str) -> String {
    format!("(SELECT COALESCE(SUM(cost_ped), 0) FROM harvest_events WHERE session_id = {session})")
}

/// The session-grain tracked columns of `tracking_sessions`, in the order
/// [`tracked`] reads them: armour, healing, dangling spend, consumed doses.
const SESSION_COLUMNS: [&str; 4] = [
    "armour_cost",
    "heal_cost",
    "dangling_cost",
    "consumable_cost",
];

/// A session's tracked buckets, from the chat log's records; zero for a
/// session that does not exist.
pub fn tracked(conn: &rusqlite::Connection, session_id: &str) -> rusqlite::Result<BucketCosts> {
    let scalar = |sql: String| -> rusqlite::Result<f64> {
        conn.prepare_cached(&format!("SELECT {sql}"))?
            .query_row([session_id], |row| row.get::<_, f64>(0))
    };
    let columns = SESSION_COLUMNS.map(|column| format!("COALESCE({column}, 0)"));
    let (armour, heal, dangling, consumable) = conn
        .prepare_cached(&format!(
            "SELECT {} FROM tracking_sessions WHERE id = ?1",
            columns.join(", ")
        ))?
        .query_row([session_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .optional()?
        .unwrap_or((0.0, 0.0, 0.0, 0.0));
    Ok(BucketCosts {
        weapon: scalar(kill_weapon_sql("?1"))?,
        enhancer: scalar(enhancer_sql("?1"))?,
        armour,
        heal,
        consumable,
        dangling,
        harvest: scalar(harvest_sql("?1"))?,
    })
}

/// The tracked records of each bucket's events, as SQL tables with a
/// `session_id` column: the one map from a bucket to what the chat log
/// recorded of it. Enhancer breaks ride the weapon's kills, and its own
/// table, so they have one too.
const EVENT_TABLES: [(Bucket, &str); 6] = [
    // every-kill: spend (any row's shots are the session's weapon play, a
    // claimed reward's arrival included).
    (Bucket::Weapon, "kills"),
    (Bucket::Enhancer, "enhancer_breaks"),
    (Bucket::Harvesting, "harvest_events"),
    (Bucket::Healing, "healing_outputs"),
    (Bucket::Armour, "protection_defence_events"),
    (Bucket::Consumables, "consumable_doses"),
];

/// The buckets a session's tracked play touched: those with a tracked
/// figure above zero, or with any tracked event (a shot at an unpriced
/// weapon still is the weapon's play). Empty for a session that does not
/// exist.
pub fn played(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> rusqlite::Result<std::collections::BTreeSet<Bucket>> {
    let tracked = tracked(conn, session_id)?;
    let mut played = std::collections::BTreeSet::new();
    for (bucket, table) in EVENT_TABLES {
        let any_event: bool = conn
            .prepare_cached(&format!(
                "SELECT EXISTS (SELECT 1 FROM {table} WHERE session_id = ?1)"
            ))?
            .query_row([session_id], |row| row.get(0))?;
        if tracked.of(bucket) > 0.0 || any_event {
            played.insert(bucket);
        }
    }
    Ok(played)
}

// ── The kept resolution ─────────────────────────────────────────────

/// A session's kept resolution: its buckets, the gap they were measured
/// across, if any, and its overhead.
type KeptResolution = ([Resolved; 6], Option<AcrossGap>, Overhead);

fn read_resolution(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> rusqlite::Result<Option<KeptResolution>> {
    // A resolution kept by other rules, or no longer in step with the
    // stored costs, is not read: the session reads tracked until healed.
    type Header = (Option<i64>, [Option<f64>; 3]);
    let current: Option<Header> = conn
        .prepare_cached(&format!(
            "SELECT r.across_gap_seconds, r.overhead_travel_ped, \
                    r.overhead_field_logistics_ped, r.overhead_unclassified_ped \
             FROM {RESOLUTIONS} r \
             WHERE r.session_id = ?1 AND r.resolution_version = ?2 \
               AND (EXISTS (SELECT 1 FROM account_session_costs c \
                            WHERE c.session_id = r.session_id AND c.costs_version = ?3 \
                              AND c.measure_version = ?4 \
                              AND c.computed_at = r.projection_computed_at) \
                    OR EXISTS (SELECT 1 FROM {LIVE} l \
                               WHERE l.session_id = r.session_id \
                                 AND l.computed_at = r.projection_computed_at AND {}))",
            published_sql(true)
        ))?
        .query_row(
            rusqlite::params![
                session_id,
                RESOLUTION_VERSION,
                MEASURED_COSTS_VERSION,
                MEASURED_MEASURE_VERSION
            ],
            |row| Ok((row.get(0)?, [row.get(1)?, row.get(2)?, row.get(3)?])),
        )
        .optional()?;
    let Some((across_gap, overhead)) = current else {
        return Ok(None);
    };
    let across_gap = across_gap.map(|gap_seconds| AcrossGap { gap_seconds });
    let overhead = match overhead
        .into_iter()
        .map(|amount| {
            amount
                .map(|amount| in_range(amount, "overhead"))
                .transpose()
        })
        .collect::<Result<Vec<_>, String>>()
    {
        Ok(amounts) => Overhead::from_amounts([amounts[0], amounts[1], amounts[2]]),
        Err(problem) => {
            unreadable(session_id, &problem);
            return Ok(None);
        }
    };
    type Row = (String, String, Option<String>, Option<f64>, Option<f64>);
    let rows: Vec<Row> = {
        let mut stmt = conn.prepare_cached(&format!(
            "SELECT bucket, basis, reason, measured_ped, scale FROM {RESOLUTION_BUCKETS} \
             WHERE session_id = ?1"
        ))?;
        let rows = stmt.query_map([session_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let mut buckets = [Resolved {
        basis: Basis::Tracked(TrackedReason::NoMeasuredCosts),
        scale: None,
    }; 6];
    for (bucket, basis, reason, measured, scale) in rows {
        let decoded = Bucket::parse(&bucket)
            .ok_or_else(|| unknown("bucket", &bucket))
            .and_then(|bucket| {
                let basis = match (basis.as_str(), reason, measured) {
                    ("measured", None, Some(measured)) => {
                        Basis::Measured(in_range(measured, "measured amount")?)
                    }
                    ("tracked", Some(reason), None) => Basis::Tracked(
                        TrackedReason::parse(&reason).ok_or_else(|| unknown("reason", &reason))?,
                    ),
                    _ => return Err(unknown("basis", &basis)),
                };
                let scale = scale.map(|scale| in_range(scale, "scale")).transpose()?;
                Ok((bucket, Resolved { basis, scale }))
            });
        match decoded {
            Ok((bucket, resolved)) => buckets[bucket.index()] = resolved,
            Err(problem) => {
                unreadable(session_id, &problem);
                return Ok(None);
            }
        }
    }
    let mixed: Option<(f64, f64)> = conn
        .prepare_cached(&format!(
            "SELECT measured_ped, tracked_ped FROM {RESOLUTION_ARMOUR} WHERE session_id = ?1"
        ))?
        .query_row([session_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .optional()?;
    if let Some((measured, tracked)) = mixed {
        let armour = &mut buckets[Bucket::Armour.index()];
        let decoded = match armour.basis {
            Basis::Tracked(TrackedReason::LimitedSetUnconfirmed) => {
                in_range(measured, "measured armour").and_then(|measured| {
                    Ok(Basis::Mixed {
                        measured,
                        tracked: in_range(tracked, "recorded armour")?,
                        reason: TrackedReason::LimitedSetUnconfirmed,
                    })
                })
            }
            _ => Err("mixed armour on an armour bucket that is not mixed".to_string()),
        };
        match decoded {
            Ok(basis) => armour.basis = basis,
            Err(problem) => {
                unreadable(session_id, &problem);
                return Ok(None);
            }
        }
    }
    let weapon_parts: Option<(f64, f64, Option<f64>)> = conn
        .prepare_cached(&format!(
            "SELECT measured_ped, tracked_ped, scale FROM {RESOLUTION_WEAPON} \
             WHERE session_id = ?1"
        ))?
        .query_row([session_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .optional()?;
    if let Some((measured, tracked, scale)) = weapon_parts {
        let weapon = &mut buckets[Bucket::Weapon.index()];
        let decoded = match weapon.basis {
            Basis::Tracked(TrackedReason::AmmoToppedUp) => {
                topped_up_weapon(measured, tracked, scale)
            }
            _ => Err("mixed weapon on a weapon bucket that is not mixed".to_string()),
        };
        match decoded {
            Ok(resolved) => *weapon = resolved,
            Err(problem) => {
                unreadable(session_id, &problem);
                return Ok(None);
            }
        }
    }
    Ok(Some((buckets, across_gap, overhead)))
}

/// A weapon kept mixed for its top-ups, from its stored parts; `Err` names
/// a part out of range.
fn topped_up_weapon(measured: f64, tracked: f64, scale: Option<f64>) -> Result<Resolved, String> {
    Ok(Resolved {
        basis: Basis::Mixed {
            measured: in_range(measured, "measured weapon")?,
            tracked: in_range(tracked, "tracked weapon")?,
            reason: TrackedReason::AmmoToppedUp,
        },
        scale: scale.map(|scale| in_range(scale, "scale")).transpose()?,
    })
}

/// A stored value outside the closed set this module knows.
fn unknown(what: &str, value: &str) -> String {
    format!("unknown session cost {what}: {value:?}")
}

/// A stored amount this module can weigh with: finite and not negative.
fn in_range(value: f64, what: &str) -> Result<f64, String> {
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(format!("session cost {what} out of range: {value}"))
    }
}

/// A session's stored costs that cannot be read: it reads tracked, and
/// the problem is logged, never read as zero.
fn unreadable(session_id: &str, problem: &str) {
    tracing::warn!(
        target: "eo::session_costs",
        session_id,
        problem,
        "a session's measured costs cannot be read; it keeps its tracked costs"
    );
}

/// One session's costs, resolved.
pub fn resolve(conn: &rusqlite::Connection, session_id: &str) -> rusqlite::Result<SessionCosts> {
    let tracked = tracked(conn, session_id)?;
    Ok(match read_resolution(conn, session_id)? {
        Some(kept) => SessionCosts::with(tracked, kept),
        None => SessionCosts::tracked_only(tracked),
    })
}

/// Several sessions' costs, resolved, by id.
pub fn resolve_many<S: AsRef<str>>(
    conn: &rusqlite::Connection,
    session_ids: &[S],
) -> rusqlite::Result<HashMap<String, SessionCosts>> {
    let mut out = HashMap::with_capacity(session_ids.len());
    for id in session_ids {
        let id = id.as_ref();
        out.insert(id.to_string(), resolve(conn, id)?);
    }
    Ok(out)
}

// ── Keeping the resolution current ──────────────────────────────────

/// Rebuild one session's resolution from its current measured costs and
/// its tracked buckets, or drop it when it has none. Run inside the write
/// that changed either; [`crate::session_summary::write_session_summary`]
/// runs it before computing the summary.
pub fn refresh(conn: &rusqlite::Connection, session_id: &str) -> rusqlite::Result<()> {
    forget(conn, session_id)?;
    // A session measured across a gap keeps how long the gap was: the
    // readings' missing time at each end it was read across.
    let header: Option<(f64, Option<i64>)> = conn
        .prepare_cached(
            "SELECT c.computed_at, \
                    CASE WHEN c.status = 'acrossGap' THEN \
                      (SELECT COALESCE(g.opening_gap_seconds, 0) \
                              + COALESCE(g.closing_gap_seconds, 0) \
                       FROM account_session_cost_gaps g WHERE g.session_id = c.session_id) \
                    END \
             FROM account_session_costs c \
             WHERE c.session_id = ?1 AND c.costs_version = ?2 AND c.measure_version = ?3",
        )?
        .query_row(
            rusqlite::params![session_id, MEASURED_COSTS_VERSION, MEASURED_MEASURE_VERSION],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((computed_at, across_gap_seconds)) = header else {
        // Ended and waiting for its seal: what its readings measured while
        // it ran, and its tracked play since.
        refresh_pending(conn, session_id)?;
        return Ok(());
    };
    let stored: Vec<(String, String, Option<String>, Option<i64>)> = {
        let mut stmt = conn.prepare_cached(
            "SELECT bucket, basis, reason, amount_micro_ped \
             FROM account_session_cost_buckets WHERE session_id = ?1",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let tracked = tracked(conn, session_id)?;
    let decoded = match decode_stored(stored, &tracked) {
        Ok((resolved, overhead)) => match mixed_armour(conn, session_id, resolved)? {
            Ok(resolved) => mixed_weapon(conn, session_id, resolved, &tracked)?
                .map(|resolved| (resolved, overhead)),
            Err(problem) => Err(problem),
        },
        Err(problem) => Err(problem),
    };
    // The header is kept even when the stored costs cannot be read, so the
    // session reads tracked (with no overhead) and is not taken up again
    // until they change.
    let overhead = decoded
        .as_ref()
        .map_or([None; 3], |(_, overhead)| *overhead);
    conn.execute(
        &format!(
            "INSERT INTO {RESOLUTIONS} \
                 (session_id, resolution_version, projection_computed_at, across_gap_seconds, \
                  overhead_travel_ped, overhead_field_logistics_ped, overhead_unclassified_ped) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"
        ),
        rusqlite::params![
            session_id,
            RESOLUTION_VERSION,
            computed_at,
            across_gap_seconds,
            overhead[OverheadCategory::Travel.index()],
            overhead[OverheadCategory::FieldLogistics.index()],
            overhead[OverheadCategory::Unclassified.index()],
        ],
    )?;
    let resolved = match decoded {
        Ok((resolved, _)) => resolved,
        Err(problem) => {
            unreadable(session_id, &problem);
            return Ok(());
        }
    };
    let mut insert = conn.prepare_cached(&format!(
        "INSERT INTO {RESOLUTION_BUCKETS} \
             (session_id, bucket, basis, reason, measured_ped, scale) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
    ))?;
    for (bucket, resolved) in resolved {
        // A mixed bucket is kept as tracked for its reason, its parts
        // beside it: the armour's in its table, the weapon's (with the
        // scale that shares them out) in its own.
        let (basis, reason, measured, scale) = match resolved.basis {
            Basis::Measured(measured) => ("measured", None, Some(measured), resolved.scale),
            Basis::Tracked(reason) => ("tracked", Some(reason.as_str()), None, None),
            Basis::Mixed { reason, .. } => ("tracked", Some(reason.as_str()), None, None),
        };
        insert.execute(rusqlite::params![
            session_id,
            bucket.as_str(),
            basis,
            reason,
            measured,
            scale,
        ])?;
        if let Basis::Mixed {
            measured, tracked, ..
        } = resolved.basis
        {
            let sql = match bucket {
                Bucket::Weapon => format!(
                    "INSERT INTO {RESOLUTION_WEAPON} \
                         (session_id, measured_ped, tracked_ped, scale) \
                     VALUES (?1, ?2, ?3, ?4)"
                ),
                _ => format!(
                    "INSERT INTO {RESOLUTION_ARMOUR} (session_id, measured_ped, tracked_ped) \
                     VALUES (?1, ?2, ?3)"
                ),
            };
            let mut statement = conn.prepare_cached(&sql)?;
            match bucket {
                Bucket::Weapon => statement.execute(rusqlite::params![
                    session_id,
                    measured,
                    tracked,
                    resolved.scale
                ])?,
                _ => statement.execute(rusqlite::params![session_id, measured, tracked])?,
            };
        }
    }
    Ok(())
}

/// The armour of a session whose limited part waits for a set to be
/// confirmed, made mixed when its unlimited part was measured: the measured
/// parts, and what the player's recordings of every limited set the
/// readings did not price spread over the session. Every other bucket
/// passes through. `Err` names a stored part this module cannot read.
fn mixed_armour(
    conn: &rusqlite::Connection,
    session_id: &str,
    mut resolved: Vec<(Bucket, Resolved)>,
) -> rusqlite::Result<Result<Vec<(Bucket, Resolved)>, String>> {
    let Some(armour) = resolved.iter_mut().find(|(bucket, resolved)| {
        *bucket == Bucket::Armour
            && resolved.basis == Basis::Tracked(TrackedReason::LimitedSetUnconfirmed)
    }) else {
        return Ok(Ok(resolved));
    };
    let parts: Vec<(String, Option<i64>)> = {
        let mut stmt = conn.prepare_cached(
            "SELECT part, amount_micro_ped FROM account_session_cost_armour_parts \
             WHERE session_id = ?1",
        )?;
        let rows = stmt.query_map([session_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    if !parts.iter().any(|(part, _)| part == "unlimited") {
        return Ok(Ok(resolved));
    }
    let mut measured_micro: i64 = 0;
    for (part, amount) in &parts {
        match (part.as_str(), amount) {
            ("unlimited" | "limitedSet", Some(amount)) => measured_micro += amount,
            ("unconfirmed", None) => {}
            _ => return Ok(Err(unknown("armour part", part))),
        }
    }
    let measured = match in_range(measured_micro as f64 / 1_000_000.0, "measured armour") {
        Ok(measured) => measured,
        Err(problem) => return Ok(Err(problem)),
    };
    let tracked: f64 = conn
        .prepare_cached(
            "SELECT COALESCE(SUM(a.cost_ped), 0) FROM protection_cost_allocations a \
             JOIN protection_cost_windows w ON w.id = a.window_id \
             WHERE a.session_id = ?1 AND w.kind = 'limited_decay' \
               AND w.superseded_at IS NULL \
               AND w.set_id NOT IN (SELECT p.set_id FROM account_session_cost_armour_parts p \
                                    WHERE p.session_id = ?1 AND p.part = 'limitedSet')",
        )?
        .query_row([session_id], |row| row.get(0))?;
    armour.1 = Resolved {
        basis: Basis::Mixed {
            measured,
            tracked: tracked.max(0.0),
            reason: TrackedReason::LimitedSetUnconfirmed,
        },
        scale: None,
    };
    Ok(Ok(resolved))
}

/// The weapon of a session whose ammo was topped up, from what the readings
/// measured of it (the stored mixed amount, which [`decode_stored`] left as
/// a mixed weapon with nothing tracked yet): the tracked spend of the kills
/// logged inside the top-up steps beside it, the two shared out over the
/// kills and the dangling spend as a measured weapon is; or, when a top-up
/// step reaches past the last kill and the session has shots after it,
/// whose time no record keeps, the tracked figure whole. Every other bucket
/// passes through. `Err` names a stored value this module cannot read.
fn mixed_weapon(
    conn: &rusqlite::Connection,
    session_id: &str,
    mut resolved: Vec<(Bucket, Resolved)>,
    tracked: &BucketCosts,
) -> rusqlite::Result<Result<Vec<(Bucket, Resolved)>, String>> {
    let Some(weapon) = resolved.iter_mut().find(|(bucket, resolved)| {
        *bucket == Bucket::Weapon && matches!(resolved.basis, Basis::Mixed { .. })
    }) else {
        return Ok(Ok(resolved));
    };
    let Basis::Mixed { measured, .. } = weapon.1.basis else {
        return Ok(Ok(resolved));
    };
    let steps: Vec<(i64, i64)> = {
        let mut stmt = conn.prepare_cached(
            "SELECT from_ms, to_ms FROM account_session_cost_top_ups \
             WHERE session_id = ?1 ORDER BY from_ms",
        )?;
        let rows = stmt.query_map([session_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    if steps.is_empty() {
        return Ok(Err("a mixed weapon with no top-up step".to_string()));
    }
    let Some(in_steps) = tracked_weapon_within(conn, session_id, &steps, tracked.dangling)? else {
        weapon.1 = Resolved {
            basis: Basis::Tracked(TrackedReason::AmmoToppedUp),
            scale: None,
        };
        return Ok(Ok(resolved));
    };
    let total = measured + in_steps;
    let carried = tracked.of(Bucket::Weapon);
    weapon.1 = Resolved {
        basis: Basis::Mixed {
            measured,
            tracked: in_steps,
            reason: TrackedReason::AmmoToppedUp,
        },
        scale: (carried > 0.0).then(|| total / carried),
    };
    Ok(Ok(resolved))
}

/// The tracked weapon spend of the session's kills logged inside the
/// steps (`(from, to]`, whole milliseconds), as [`tracked`] prices every
/// kill; `None` when a step reaches past the last kill (or the session has
/// none) while the session has shots after its last kill (`dangling` PED):
/// those carry no time, so what of them a step holds is not known.
fn tracked_weapon_within(
    conn: &rusqlite::Connection,
    session_id: &str,
    steps: &[(i64, i64)],
    dangling: f64,
) -> rusqlite::Result<Option<f64>> {
    // every-kill: spend (the last row bounds the shots no kill carries).
    let last_kill: Option<f64> = conn
        .prepare_cached("SELECT MAX(timestamp) FROM kills WHERE session_id = ?1")?
        .query_row([session_id], |row| row.get(0))?;
    let seconds = |ms: i64| ms as f64 / 1_000.0;
    let past_the_last_kill = steps
        .iter()
        .any(|&(_, to)| last_kill.is_none_or(|last| seconds(to) > last));
    if past_the_last_kill && dangling != 0.0 {
        return Ok(None);
    }
    let mut spend = 0.0;
    for &(from, to) in steps {
        // every-kill: spend (any row's shots are the session's weapon play).
        spend += conn
            .prepare_cached(
                "SELECT COALESCE(SUM(COALESCE(ts.cost_per_shot, 0) \
                                     * COALESCE(ts.shots_fired, 0)), 0) \
                 FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id \
                 WHERE k.session_id = ?1 AND k.timestamp > ?2 AND k.timestamp <= ?3",
            )?
            .query_row(
                rusqlite::params![session_id, seconds(from), seconds(to)],
                |row| row.get::<_, f64>(0),
            )?;
    }
    Ok(Some(spend))
}

/// What a session's stored costs decode to: its Cycled buckets resolved,
/// and its overhead by category (the measured amount, or `None`).
type Decoded = (Vec<(Bucket, Resolved)>, [Option<f64>; 3]);

/// The stored measured buckets resolved against the tracked ones, and the
/// overhead the readings measured, which no Cycled figure includes. An
/// overhead category left unmeasured, for whatever reason, has none.
/// `Err` names the first value this module cannot read.
fn decode_stored(
    stored: Vec<(String, String, Option<String>, Option<i64>)>,
    tracked: &BucketCosts,
) -> Result<Decoded, String> {
    let mut out = Vec::with_capacity(stored.len());
    let mut overhead = [None; 3];
    for (bucket, basis, reason, amount) in stored {
        let bucket = match StoredBucket::parse(&bucket) {
            Some(StoredBucket::Cycled(bucket)) => bucket,
            Some(StoredBucket::Overhead(category)) => {
                overhead[category.index()] = match (basis.as_str(), reason, amount) {
                    ("measured", None, Some(micro_ped)) => Some(in_range(
                        micro_ped as f64 / 1_000_000.0,
                        "measured overhead",
                    )?),
                    ("unmeasured", Some(_), None) => None,
                    _ => return Err(unknown("basis", &basis)),
                };
                continue;
            }
            None => return Err(unknown("bucket", &bucket)),
        };
        let resolved = match (basis.as_str(), reason, amount) {
            ("measured", None, Some(micro_ped)) => {
                let measured = in_range(micro_ped as f64 / 1_000_000.0, "measured amount")?;
                resolve_measured(bucket, measured, tracked)
            }
            // Measured but for the steps its ammo was topped up in: the
            // tracked spend of those is added by [`mixed_weapon`].
            ("mixed", None, Some(micro_ped)) if bucket == Bucket::Weapon => Resolved {
                basis: Basis::Mixed {
                    measured: in_range(micro_ped as f64 / 1_000_000.0, "measured amount")?,
                    tracked: 0.0,
                    reason: TrackedReason::AmmoToppedUp,
                },
                scale: None,
            },
            ("unmeasured", Some(reason), None) => Resolved {
                basis: Basis::Tracked(
                    TrackedReason::parse(&reason).ok_or_else(|| unknown("reason", &reason))?,
                ),
                scale: None,
            },
            _ => return Err(unknown("basis", &basis)),
        };
        out.push((bucket, resolved));
    }
    Ok((out, overhead))
}

/// One measured bucket against its tracked amount: the measured amount is
/// the cost, shared over the bucket's tracked events where there are any.
fn resolve_measured(bucket: Bucket, measured: f64, tracked: &BucketCosts) -> Resolved {
    let tracked = tracked.of(bucket);
    Resolved {
        basis: Basis::Measured(measured),
        scale: (bucket.tracked_by_event() && tracked > 0.0).then(|| measured / tracked),
    }
}

/// Drop a session's resolution: before it is rebuilt, or with the session.
pub fn forget(conn: &rusqlite::Connection, session_id: &str) -> rusqlite::Result<()> {
    for table in [
        RESOLUTION_WEAPON,
        RESOLUTION_ARMOUR,
        RESOLUTION_BUCKETS,
        RESOLUTIONS,
    ] {
        conn.prepare_cached(&format!("DELETE FROM {table} WHERE session_id = ?1"))?
            .execute([session_id])?;
    }
    Ok(())
}

/// Bring every figure derived from a session's costs up to date with them:
/// its resolution and summary, its days, and its rollup cells, all or
/// none (one savepoint). Run inside the write that moved its costs,
/// tracked or measured.
pub fn refresh_derived(conn: &rusqlite::Connection, session_id: &str) -> Result<(), DbError> {
    conn.execute_batch("SAVEPOINT session_figures")?;
    let refreshed = (|| -> Result<(), DbError> {
        crate::session_summary::write_session_summary(conn, session_id)?;
        crate::daily_rollup::refresh_session_days(conn, session_id)?;
        crate::session_rollup::recompute_session(conn, session_id)?;
        Ok(())
    })();
    match refreshed {
        Ok(()) => conn.execute_batch("RELEASE session_figures")?,
        Err(error) => {
            if let Err(undo) =
                conn.execute_batch("ROLLBACK TO session_figures; RELEASE session_figures")
            {
                tracing::warn!(
                    target: "eo::session_costs",
                    %undo,
                    "a session's figures could not be rolled back after a failed refresh"
                );
            }
            return Err(error);
        }
    }
    Ok(())
}

/// The sessions whose kept resolution no longer matches their measured
/// costs (or, for an ended session waiting for its seal, what was
/// published of it while it ran): written, rebuilt or dropped since, or
/// kept by other rules.
pub fn out_of_step(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare_cached(&format!(
        "SELECT r.session_id FROM {RESOLUTIONS} r \
         WHERE r.resolution_version != ?1 \
            OR (NOT EXISTS (SELECT 1 FROM account_session_costs c \
                            WHERE c.session_id = r.session_id AND c.costs_version = ?2 \
                              AND c.measure_version = ?3 \
                              AND c.computed_at = r.projection_computed_at) \
                AND NOT EXISTS (SELECT 1 FROM {LIVE} l \
                                WHERE l.session_id = r.session_id \
                                  AND l.computed_at = r.projection_computed_at AND {pending})) \
         UNION \
         SELECT c.session_id FROM account_session_costs c \
         WHERE c.costs_version = ?2 AND c.measure_version = ?3 \
           AND NOT EXISTS (SELECT 1 FROM {RESOLUTIONS} r WHERE r.session_id = c.session_id) \
         UNION \
         SELECT l.session_id FROM {LIVE} l WHERE {pending} \
           AND NOT EXISTS (SELECT 1 FROM {RESOLUTIONS} r \
                           WHERE r.session_id = l.session_id \
                             AND r.projection_computed_at = l.computed_at)",
        pending = published_sql(true)
    ))?;
    let rows = stmt.query_map(
        rusqlite::params![
            RESOLUTION_VERSION,
            MEASURED_COSTS_VERSION,
            MEASURED_MEASURE_VERSION
        ],
        |row| row.get::<_, String>(0),
    )?;
    rows.collect()
}

/// Refresh every session [`out_of_step`], and everything derived from its
/// costs. Idempotent, and cheap once nothing is out of step; every cache
/// heal runs it first.
pub fn heal(conn: &rusqlite::Connection) -> Result<(), DbError> {
    for session_id in out_of_step(conn)? {
        refresh_derived(conn, &session_id)?;
        // A session the measured costs name but the tracker no longer
        // holds keeps no resolution.
        let exists: bool = conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM tracking_sessions WHERE id = ?1)",
            [&session_id],
            |row| row.get(0),
        )?;
        if !exists {
            forget(conn, &session_id)?;
        }
    }
    Ok(())
}

// ── SQL for readers that aggregate many sessions ────────────────────

/// The scale a bucket's tracked events weigh by, for the session named by
/// `session` (an SQL expression): a mixed weapon's from its parts, and 1
/// where the bucket is tracked. The integer 1 keeps a tracked figure
/// exactly as it was, type included.
pub fn scale_sql(bucket: Bucket, session: &str) -> String {
    let mixed = match bucket {
        Bucket::Weapon => {
            format!("(SELECT scale FROM {RESOLUTION_WEAPON} WHERE session_id = {session}), ")
        }
        _ => String::new(),
    };
    format!(
        "COALESCE((SELECT scale FROM {RESOLUTION_BUCKETS} \
                   WHERE session_id = {session} AND bucket = '{}'), {mixed}1)",
        bucket.as_str()
    )
}

/// A tracked event's cost (`expr`) as its bucket resolves it.
pub fn scaled(expr: &str, bucket: Bucket, session: &str) -> String {
    format!("({expr}) * {}", scale_sql(bucket, session))
}

/// A kill row's cycled spend (its weapon cost and its enhancer break) as
/// resolved, for the kill table aliased `kill`.
pub fn kill_cycled_sql(kill: &str) -> String {
    let session = format!("{kill}.session_id");
    format!(
        "({} + {})",
        scaled(&format!("{kill}.cost_ped"), Bucket::Weapon, &session),
        scaled(&format!("{kill}.enhancer_cost"), Bucket::Enhancer, &session),
    )
}

/// A session-grain column of `tracking_sessions` aliased `session` as
/// resolved: armour, healing and consumed doses take the measured amount;
/// the dangling spend weighs by the weapon's scale.
pub fn session_column_sql(column: SessionColumn, session: &str) -> String {
    let id = format!("{session}.id");
    let measured = |bucket: Bucket, column: &str| {
        format!(
            "COALESCE((SELECT measured_ped FROM {RESOLUTION_BUCKETS} \
                       WHERE session_id = {id} AND bucket = '{}'), {session}.{column})",
            bucket.as_str()
        )
    };
    match column {
        SessionColumn::Armour => format!(
            "COALESCE((SELECT measured_ped FROM {RESOLUTION_BUCKETS} \
                       WHERE session_id = {id} AND bucket = 'armour'), \
                      (SELECT measured_ped + tracked_ped FROM {RESOLUTION_ARMOUR} \
                       WHERE session_id = {id}), \
                      {session}.armour_cost)"
        ),
        SessionColumn::Heal => measured(Bucket::Healing, "heal_cost"),
        SessionColumn::Consumable => measured(Bucket::Consumables, "consumable_cost"),
        SessionColumn::Dangling => scaled(&format!("{session}.dangling_cost"), Bucket::Weapon, &id),
    }
}

/// The session-grain cost columns of `tracking_sessions`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionColumn {
    Armour,
    Heal,
    Consumable,
    Dangling,
}

/// Whether a session of `tracking_sessions` aliased `owner` cost
/// anything: its tracked buckets, derived as [`tracked`] derives them,
/// summing above zero, a bucket measured above zero, or measured overhead
/// above zero. The summaries' heal finds the sessions it owes a summary
/// with it.
pub fn spent_anything_sql(owner: &str) -> String {
    let id = format!("{owner}.id");
    let columns = SESSION_COLUMNS.map(|column| format!("COALESCE({owner}.{column}, 0)"));
    format!(
        "({} + {} + {} + {} > 0 \
          OR EXISTS (SELECT 1 FROM {RESOLUTION_BUCKETS} b \
                     WHERE b.session_id = {id} AND b.basis = 'measured' \
                       AND b.measured_ped > 0) \
          OR EXISTS (SELECT 1 FROM {RESOLUTION_ARMOUR} m \
                     WHERE m.session_id = {id} AND m.measured_ped + m.tracked_ped > 0) \
          OR EXISTS (SELECT 1 FROM {RESOLUTION_WEAPON} w \
                     WHERE w.session_id = {id} AND w.measured_ped + w.tracked_ped > 0) \
          OR {} > 0)",
        kill_weapon_sql(&id),
        enhancer_sql(&id),
        harvest_sql(&id),
        columns.join(" + "),
        overhead_total_sql(&id),
    )
}

/// The part of an event bucket that sits at the grain of the session
/// aliased `session` (measured, or a mixed weapon, with no tracked event to
/// carry it, and for the enhancer bucket the breaks after the last kill, as
/// resolved), or
/// NULL: a sum over sessions stays NULL unless one holds such a part.
pub fn session_grain_sql(bucket: Bucket, session: &str) -> String {
    let id = format!("{session}.id");
    let measured = format!(
        "(SELECT measured_ped FROM {RESOLUTION_BUCKETS} \
          WHERE session_id = {id} AND bucket = '{}' AND basis = 'measured' \
            AND scale IS NULL AND measured_ped != 0)",
        bucket.as_str()
    );
    match bucket {
        Bucket::Enhancer => {
            let breaks = scaled(&session_breaks_sql(&id), Bucket::Enhancer, &id);
            // Either part alone, both summed, or NULL when neither is held.
            format!("COALESCE({measured} + {breaks}, {measured}, {breaks})")
        }
        // A mixed weapon no tracked shot carries sits whole at the grain.
        Bucket::Weapon => format!(
            "COALESCE({measured}, \
                      (SELECT measured_ped + tracked_ped FROM {RESOLUTION_WEAPON} \
                       WHERE session_id = {id} AND scale IS NULL \
                         AND measured_ped + tracked_ped != 0))"
        ),
        _ => measured,
    }
}

/// A family's sum with the session-grain part of its bucket added: a part
/// that is absent leaves the sum exactly as it was, NULL included.
pub fn with_session_grain(sum: Option<f64>, grain: Option<f64>) -> Option<f64> {
    match (sum, grain) {
        (sum, None) => sum,
        (None, Some(grain)) => Some(grain),
        (Some(sum), Some(grain)) => Some(sum + grain),
    }
}

/// What one session aliased `session` costs at its own grain, as resolved,
/// in order: armour, healing, dangling spend, consumed doses, then the
/// session-grain parts of the weapon, enhancer and harvesting buckets
/// ([`session_grain_sql`]).
fn session_parts(session: &str) -> [String; 7] {
    [
        session_column_sql(SessionColumn::Armour, session),
        session_column_sql(SessionColumn::Heal, session),
        session_column_sql(SessionColumn::Dangling, session),
        session_column_sql(SessionColumn::Consumable, session),
        session_grain_sql(Bucket::Weapon, session),
        session_grain_sql(Bucket::Enhancer, session),
        session_grain_sql(Bucket::Harvesting, session),
    ]
}

/// The sums over a group of `tracking_sessions` aliased `session` of each
/// of [`session_parts`], in its order; each NULL over no contributing rows.
pub fn session_sums_sql(session: &str) -> String {
    session_parts(session)
        .iter()
        .map(|part| format!("SUM({part})"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// Everything a group of sessions aliased `session` cost at their own
/// grain, added up (zero over none).
pub fn session_total_sql(session: &str) -> String {
    session_parts(session)
        .iter()
        .map(|part| format!("COALESCE(SUM({part}), 0)"))
        .collect::<Vec<_>>()
        .join(" + ")
}

/// The sums over a group of `tracking_sessions` aliased `session` of each
/// session's overhead, by category in the order of [`OverheadCategory::ALL`];
/// each NULL unless some session of the group has that category measured.
pub fn overhead_sums_sql(session: &str) -> String {
    OverheadCategory::ALL
        .iter()
        .map(|category| {
            format!(
                "SUM((SELECT r.{} FROM {RESOLUTIONS} r WHERE r.session_id = {session}.id))",
                category.column()
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The overhead of the session named by `session` (an SQL expression for
/// its id), every category added up: zero where none was measured.
pub fn overhead_total_sql(session: &str) -> String {
    let columns = OverheadCategory::ALL
        .map(|category| format!("COALESCE(r.{}, 0)", category.column()))
        .join(" + ");
    format!("COALESCE((SELECT {columns} FROM {RESOLUTIONS} r WHERE r.session_id = {session}), 0)")
}

/// Why an overhead category the readings did not measure is withheld.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverheadReason {
    /// The session was measured across a gap, and the category changed in
    /// the gap: it may be the player's after the session, so it is not
    /// billed to it.
    InGap,
    /// Any of the reasons a Cycled bucket keeps its tracked figure for.
    Tracked(TrackedReason),
}

impl OverheadReason {
    fn parse(value: &str) -> Option<OverheadReason> {
        match value {
            "overheadInGap" => Some(OverheadReason::InGap),
            other => TrackedReason::parse(other).map(OverheadReason::Tracked),
        }
    }
}

/// Why each overhead category of a session is withheld, by
/// [`OverheadCategory::ALL`]: `None` for a category measured, and for every
/// category of a session with no current measured costs (no overhead, and
/// nothing to say of it). A reason this module cannot read is logged and
/// reads as none.
pub fn overhead_reasons(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> rusqlite::Result<[Option<OverheadReason>; 3]> {
    let mut reasons = [None; 3];
    let rows: Vec<(String, String)> = {
        let mut stmt = conn.prepare_cached(
            "SELECT b.bucket, b.reason FROM account_session_cost_buckets b \
             JOIN account_session_costs c ON c.session_id = b.session_id \
             WHERE b.session_id = ?1 AND b.basis = 'unmeasured' AND c.costs_version = ?2 \
               AND c.measure_version = ?3",
        )?;
        let rows = stmt.query_map(
            rusqlite::params![session_id, MEASURED_COSTS_VERSION, MEASURED_MEASURE_VERSION],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (bucket, reason) in rows {
        let Some(category) = OverheadCategory::parse(&bucket) else {
            continue;
        };
        match OverheadReason::parse(&reason) {
            Some(reason) => reasons[category.index()] = Some(reason),
            None => unreadable(session_id, &unknown("overhead reason", &reason)),
        }
    }
    Ok(reasons)
}

// ── The measured lines, for a session's own detail ──────────────────

/// How exactly a measured amount is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasuredPrecision {
    Exact,
    /// It rests on values the account site truncated to the whole PEC.
    WholePec,
}

impl MeasuredPrecision {
    fn parse(value: &str) -> Option<MeasuredPrecision> {
        match value {
            "exact" => Some(MeasuredPrecision::Exact),
            "wholePec" => Some(MeasuredPrecision::WholePec),
            _ => None,
        }
    }
}

/// What a measured line measures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeasuredLineKind {
    /// Ammo burned, in units at TT.
    AmmoBurned,
    /// An item's TT falls, summed.
    Decay,
    /// A limited piece's TT falls, summed: what it lost, which its set's
    /// markup prices.
    LimitedDecay,
    /// A consumable's units used up.
    Consumed,
}

impl MeasuredLineKind {
    fn parse(value: &str) -> Option<MeasuredLineKind> {
        match value {
            "ammoBurned" => Some(MeasuredLineKind::AmmoBurned),
            "decay" => Some(MeasuredLineKind::Decay),
            "limitedDecay" => Some(MeasuredLineKind::LimitedDecay),
            "consumed" => Some(MeasuredLineKind::Consumed),
            _ => None,
        }
    }
}

/// Why a measured line is no cost of the session's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unpriced {
    /// The player accounts for the item through the ledger, where its
    /// purchase is recorded.
    BoughtThroughLedger,
    /// A limited piece no confirmed set holds: pricing it needs its set's
    /// pieces confirmed.
    LimitedSetUnconfirmed,
    /// Units of a stack nothing names left: no cost until the player places
    /// it as used up.
    NotPlaced,
}

/// How a measured line is priced.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LinePricing {
    /// At TT. For units used up of an item bought at markup, the markup is
    /// not included.
    AtTt,
    /// At the acquisition markup the player recorded, or a limited set's
    /// markup, in percent of TT.
    AtMarkup(f64),
    /// Not a cost here, for this reason.
    Unpriced(Unpriced),
}

/// One item's measured use within a bucket.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasuredLine {
    pub bucket: StoredBucket,
    pub kind: MeasuredLineKind,
    pub item: String,
    /// The units, for a line that counts them.
    pub units: Option<i64>,
    /// What the line measured in PED (for a limited piece, the TT it lost;
    /// for units at markup, their cost), with its precision; `None` for a
    /// line charged nothing.
    pub amount: Option<(f64, MeasuredPrecision)>,
    pub pricing: LinePricing,
}

/// One part of a session's measured armour.
#[derive(Debug, Clone, PartialEq)]
pub enum MeasuredArmourPart {
    /// Unlimited armour and plates, billed at the TT they lost.
    Unlimited { lost: (f64, MeasuredPrecision) },
    /// A confirmed limited set's pieces, billed at the TT they lost times
    /// the set's markup.
    Set {
        set_id: i64,
        name: Option<String>,
        markup_percent: f64,
        lost: (f64, MeasuredPrecision),
        cost: (f64, MeasuredPrecision),
    },
    /// What limited pieces no confirmed set holds lost: no price yet.
    Unconfirmed { lost: (f64, MeasuredPrecision) },
    /// One limited piece: the TT it lost, billed at the markup the player
    /// set for it (in percent), or at that TT when none was set.
    Piece {
        item: String,
        markup_percent: Option<f64>,
        lost: (f64, MeasuredPrecision),
        cost: (f64, MeasuredPrecision),
    },
}

/// A session's measured costs item by item, as the readings measured them.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MeasuredDetail {
    /// In the stored order: by bucket, kind and item.
    pub lines: Vec<MeasuredLine>,
    /// Unlimited first, then each limited piece by name.
    pub armour: Vec<MeasuredArmourPart>,
}

/// A stored amount in PED with its precision; `Err` names a value this
/// module cannot read.
fn stored_amount(
    micro_ped: i64,
    precision: &str,
    what: &str,
) -> Result<(f64, MeasuredPrecision), String> {
    let precision =
        MeasuredPrecision::parse(precision).ok_or_else(|| unknown("precision", precision))?;
    Ok((in_range(micro_ped as f64 / 1_000_000.0, what)?, precision))
}

/// A session's measured lines and armour parts, when its stored costs are
/// current and measured it (whole, or across a gap); `None` otherwise, and
/// for stored costs this module cannot read (logged).
pub fn measured_detail(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> rusqlite::Result<Option<MeasuredDetail>> {
    let current: bool = conn
        .prepare_cached(
            "SELECT EXISTS (SELECT 1 FROM account_session_costs \
                            WHERE session_id = ?1 AND costs_version = ?2 \
                              AND measure_version = ?3 \
                              AND status IN ('measured', 'acrossGap'))",
        )?
        .query_row(
            rusqlite::params![session_id, MEASURED_COSTS_VERSION, MEASURED_MEASURE_VERSION],
            |row| row.get(0),
        )?;
    if !current {
        return Ok(None);
    }
    type RawLine = (
        String,
        String,
        String,
        Option<i64>,
        Option<i64>,
        Option<String>,
        Option<i64>,
        Option<String>,
        Option<i64>,
        bool,
        Option<i64>,
    );
    let raw: Vec<RawLine> = {
        let mut stmt = conn.prepare_cached(
            "SELECT l.bucket, l.kind, l.item_name, l.units, l.amount_micro_ped, l.precision, \
                    l.markup_hundredths, l.reason, l.set_id, q.item_name IS NOT NULL, \
                    q.markup_hundredths \
             FROM account_session_cost_lines l \
             LEFT JOIN account_session_cost_armour_pieces q \
                    ON q.session_id = l.session_id AND q.item_name = l.item_name \
                   AND l.kind = 'limitedDecay' \
             WHERE l.session_id = ?1 ORDER BY l.position",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
                row.get(9)?,
                row.get(10)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let decode_line = |(
        bucket,
        kind,
        item,
        units,
        amount,
        precision,
        markup,
        reason,
        set_id,
        priced_as_piece,
        piece_markup,
    ): RawLine|
     -> Result<MeasuredLine, String> {
        let bucket = StoredBucket::parse(&bucket).ok_or_else(|| unknown("bucket", &bucket))?;
        let kind = MeasuredLineKind::parse(&kind).ok_or_else(|| unknown("line kind", &kind))?;
        let amount = match (amount, precision) {
            (Some(amount), Some(precision)) => Some(stored_amount(amount, &precision, "line")?),
            (None, None) => None,
            _ => return Err("a line's amount without its precision".to_string()),
        };
        let pricing = match (kind, markup, reason.as_deref(), set_id) {
            (_, None, Some("boughtThroughLedger"), None) => {
                LinePricing::Unpriced(Unpriced::BoughtThroughLedger)
            }
            (MeasuredLineKind::Consumed, None, Some("notPlaced"), None) => {
                LinePricing::Unpriced(Unpriced::NotPlaced)
            }
            (_, Some(markup), None, None) => LinePricing::AtMarkup(markup as f64 / 100.0),
            // A limited piece is priced at the markup set for it, or at
            // the TT it lost when none was.
            (MeasuredLineKind::LimitedDecay, None, None, None) if priced_as_piece => {
                match piece_markup {
                    Some(markup) => LinePricing::AtMarkup(markup as f64 / 100.0),
                    None => LinePricing::AtTt,
                }
            }
            (MeasuredLineKind::LimitedDecay, None, None, _) => {
                return Err("a limited piece's line has no piece".to_string())
            }
            (_, None, None, None) => LinePricing::AtTt,
            (_, _, Some(reason), _) => return Err(unknown("line reason", reason)),
            _ => return Err("a line's pricing is not one this module knows".to_string()),
        };
        Ok(MeasuredLine {
            bucket,
            kind,
            item,
            units,
            amount,
            pricing,
        })
    };
    let mut lines = Vec::with_capacity(raw.len());
    for row in raw {
        match decode_line(row) {
            Ok(line) => lines.push(line),
            Err(problem) => {
                unreadable(session_id, &problem);
                return Ok(None);
            }
        }
    }
    // Unlimited armour first, then each limited piece by name.
    let unlimited: Vec<(String, i64, String)> = {
        let mut stmt = conn.prepare_cached(
            "SELECT part, lost_micro_ped, lost_precision \
             FROM account_session_cost_armour_parts WHERE session_id = ?1",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    type RawPiece = (String, i64, String, Option<i64>, i64, String);
    let pieces: Vec<RawPiece> = {
        let mut stmt = conn.prepare_cached(
            "SELECT item_name, lost_micro_ped, lost_precision, markup_hundredths, \
                    amount_micro_ped, precision \
             FROM account_session_cost_armour_pieces WHERE session_id = ?1 ORDER BY item_name",
        )?;
        let rows = stmt.query_map([session_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    let decode = || -> Result<Vec<MeasuredArmourPart>, String> {
        let mut armour = Vec::with_capacity(unlimited.len() + pieces.len());
        for (part, lost, precision) in &unlimited {
            if part != "unlimited" {
                return Err(unknown("armour part", part));
            }
            armour.push(MeasuredArmourPart::Unlimited {
                lost: stored_amount(*lost, precision, "armour lost")?,
            });
        }
        for (item, lost, lost_precision, markup, cost, precision) in &pieces {
            armour.push(MeasuredArmourPart::Piece {
                item: item.clone(),
                markup_percent: markup.map(|markup| markup as f64 / 100.0),
                lost: stored_amount(*lost, lost_precision, "piece lost")?,
                cost: stored_amount(*cost, precision, "piece cost")?,
            });
        }
        Ok(armour)
    };
    let armour = match decode() {
        Ok(armour) => armour,
        Err(problem) => {
            unreadable(session_id, &problem);
            return Ok(None);
        }
    };
    Ok(Some(MeasuredDetail { lines, armour }))
}

// ── Armour measured from the readings, for the protection recordings ──

/// A protection stream whose share of a session the readings can measure:
/// the pooled unlimited armour, or one limited set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmourStream {
    Unlimited,
    Set(i64),
}

/// The sessions, by current measured costs, holding an armour part that
/// `part` (an SQL condition over the part `p`) names: an SQL subquery,
/// uncorrelated, so it is read once however many rows ask.
fn sessions_with_part(part: &str) -> String {
    format!(
        "SELECT p.session_id FROM account_session_cost_armour_parts p \
         JOIN account_session_costs c ON c.session_id = p.session_id \
         WHERE c.costs_version = {MEASURED_COSTS_VERSION} \
           AND c.measure_version = {MEASURED_MEASURE_VERSION} AND {part}"
    )
}

/// Whether the readings measured the `stream` part of the armour of the
/// session named by `session` (an SQL expression for its id), by the
/// current measured costs: an SQL condition. A recording of the stream
/// never reaches such a session, since its armour is already billed.
///
/// A limited set's part is measured when the set has a part of its own, or
/// when the session was measured whole with no limited piece left unpriced:
/// then nothing of any set that has no part wore unmeasured, so a reading
/// of it has nothing there to cover.
pub fn armour_measured_sql(stream: ArmourStream, session: &str) -> String {
    let unlimited = sessions_with_part("p.part = 'unlimited'");
    match stream {
        ArmourStream::Unlimited => format!("({session} IN ({unlimited}))"),
        ArmourStream::Set(set_id) => format!(
            "({session} IN ({}) OR ({session} IN ({unlimited}) AND {session} NOT IN ({})))",
            sessions_with_part(&format!("p.part = 'limitedSet' AND p.set_id = {set_id}")),
            sessions_with_part("p.part = 'unconfirmed'"),
        ),
    }
}

/// Whether a limited piece no confirmed set held lost TT in the session
/// named by `session`, so its limited armour waits to be priced: an SQL
/// condition.
pub fn armour_unpriced_sql(session: &str) -> String {
    format!(
        "({session} IN ({}))",
        sessions_with_part("p.part = 'unconfirmed'")
    )
}

/// The TT, in micro-PED, the `stream` part of the armour of the session
/// named by `session` lost as the readings measured it, by current measured
/// costs: an SQL expression, NULL where the session was not measured whole.
/// A limited set's part is what its confirmed pieces lost, each billed on
/// its own (nothing when none of them wore).
pub fn armour_lost_sql(stream: ArmourStream, session: &str) -> String {
    match stream {
        ArmourStream::Unlimited => format!(
            "(SELECT p.lost_micro_ped FROM account_session_cost_armour_parts p \
              JOIN account_session_costs c ON c.session_id = p.session_id \
              WHERE p.session_id = {session} AND c.costs_version = {MEASURED_COSTS_VERSION} \
                AND c.measure_version = {MEASURED_MEASURE_VERSION} AND p.part = 'unlimited')"
        ),
        ArmourStream::Set(set_id) => format!(
            "(SELECT COALESCE((SELECT SUM(q.lost_micro_ped) \
                                FROM account_session_cost_armour_pieces q \
                                WHERE q.session_id = p.session_id AND {}), 0) \
              FROM account_session_cost_armour_parts p \
              JOIN account_session_costs c ON c.session_id = p.session_id \
              WHERE p.session_id = {session} AND c.costs_version = {MEASURED_COSTS_VERSION} \
                AND c.measure_version = {MEASURED_MEASURE_VERSION} AND p.part = 'unlimited')",
            set_piece(set_id),
        ),
    }
}

/// Whether the piece `q` is one confirmed for the set `set_id`: an SQL
/// condition.
fn set_piece(set_id: i64) -> String {
    format!(
        "q.item_name IN (SELECT s.item_name FROM protection_set_pieces s \
                         WHERE s.set_id = {set_id})"
    )
}

/// Whether the readings have billed a limited set: one of its confirmed
/// pieces lost TT in a session measured by current costs. Its markup
/// priced that bill, so it may no longer change.
pub fn set_billed_from_readings(
    conn: &rusqlite::Connection,
    set_id: i64,
) -> rusqlite::Result<bool> {
    conn.query_row(
        &format!(
            "SELECT EXISTS (SELECT 1 FROM account_session_cost_armour_pieces q \
             JOIN account_session_costs c ON c.session_id = q.session_id \
             WHERE c.costs_version = {MEASURED_COSTS_VERSION} \
               AND c.measure_version = {MEASURED_MEASURE_VERSION} \
               AND q.lost_micro_ped > 0 AND {})",
            set_piece(set_id)
        ),
        [],
        |row| row.get(0),
    )
}

// ── A running session: measured so far, tracked since ───────────────

/// What the account side publishes of a running session's costs so far.
const LIVE: &str = "account_live_session_costs";

/// The published column of each Cycled bucket, in the order of
/// [`Bucket::ALL`]: the enhancer, which the readings never measure, has
/// none.
const LIVE_CYCLED: [Option<&str>; 6] = [
    Some("weapon_micro_ped"),
    None,
    Some("harvesting_micro_ped"),
    Some("healing_micro_ped"),
    Some("armour_micro_ped"),
    Some("consumables_micro_ped"),
];

/// The published column of each overhead category, in the order of
/// [`OverheadCategory::ALL`].
const LIVE_OVERHEAD: [&str; 3] = [
    "travel_micro_ped",
    "field_logistics_micro_ped",
    "unclassified_micro_ped",
];

/// What the account readings measured of a session still running, up to
/// its latest reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasuredSoFar {
    /// When that reading was taken (unix seconds): the measured figures run
    /// to here, and the tracked events after it carry on from here.
    pub through: f64,
    /// Each Cycled bucket's measured amount up to then, in PED, in the
    /// order of [`Bucket::ALL`]; `None` where the readings could not settle
    /// it, so the bucket stays tracked.
    pub cycled: [Option<f64>; 6],
    /// The overhead measured up to then; a category not settled has none.
    pub overhead: Overhead,
}

impl MeasuredSoFar {
    /// What the readings measured of a bucket up to [`Self::through`].
    pub fn bucket(&self, bucket: Bucket) -> Option<f64> {
        self.cycled[bucket.index()]
    }
}

/// What the readings measured of the session `session_id` so far, as the
/// account side last published it by the rules this resolver reads: while
/// it runs, and once it has ended until its costs are sealed (the seal
/// drops the row). `None` for a session with nothing published, one whose
/// sealed costs are stored, or one whose published row cannot be read
/// (logged).
pub fn measured_so_far(
    conn: &rusqlite::Connection,
    session_id: &str,
) -> rusqlite::Result<Option<MeasuredSoFar>> {
    Ok(read_so_far(conn, session_id, false)?.map(|(so_far, _)| so_far))
}

/// SQL true while the published row `l` stands for its session: by these
/// rules, and with no sealed costs stored for the session by them; with
/// `ended`, only once the session has ended (its seal pending).
fn published_sql(ended: bool) -> String {
    let ended = if ended { " AND s.is_active = 0" } else { "" };
    format!(
        "l.costs_version = {MEASURED_COSTS_VERSION} \
         AND l.measure_version = {MEASURED_MEASURE_VERSION} \
         AND EXISTS (SELECT 1 FROM tracking_sessions s \
                     WHERE s.id = l.session_id{ended}) \
         AND NOT EXISTS (SELECT 1 FROM account_session_costs c \
                         WHERE c.session_id = l.session_id \
                           AND c.costs_version = {MEASURED_COSTS_VERSION} \
                           AND c.measure_version = {MEASURED_MEASURE_VERSION})"
    )
}

/// [`measured_so_far`], with when the row was computed; with `ended`, only
/// for a session that has ended and waits for its seal.
fn read_so_far(
    conn: &rusqlite::Connection,
    session_id: &str,
    ended: bool,
) -> rusqlite::Result<Option<(MeasuredSoFar, f64)>> {
    let columns: Vec<&str> = LIVE_CYCLED
        .iter()
        .flatten()
        .chain(LIVE_OVERHEAD.iter())
        .copied()
        .collect();
    let row: Option<(f64, f64, Vec<Option<i64>>)> = conn
        .prepare_cached(&format!(
            "SELECT l.measured_through, l.computed_at, {} FROM {LIVE} l \
             WHERE l.session_id = ?1 AND {}",
            columns
                .iter()
                .map(|column| format!("l.{column}"))
                .collect::<Vec<_>>()
                .join(", "),
            published_sql(ended),
        ))?
        .query_row([session_id], |row| {
            let mut amounts = Vec::with_capacity(columns.len());
            for index in 0..columns.len() {
                amounts.push(row.get::<_, Option<i64>>(index + 2)?);
            }
            Ok((row.get(0)?, row.get(1)?, amounts))
        })
        .optional()?;
    let Some((through, computed_at, amounts)) = row else {
        return Ok(None);
    };
    match decode_so_far(through, &amounts) {
        Ok(so_far) => Ok(Some((so_far, computed_at))),
        Err(problem) => {
            unreadable(session_id, &problem);
            Ok(None)
        }
    }
}

/// Of an ended session's tracked buckets, what its stored events timed
/// after `through` cost: its kills ended after it (a kill timed whole by
/// its end, as no shot's time is stored), the shots after its last kill
/// (the end of its play, so after any reading taken while it ran), its
/// paid heals observed after it with their standing corrections, its doses
/// started after it and not removed, and its harvesting swings after it.
/// Armour has no event of play to time, and the enhancer stays tracked
/// whole.
fn tracked_after_stored(
    conn: &rusqlite::Connection,
    session_id: &str,
    through: f64,
    tracked: &BucketCosts,
) -> rusqlite::Result<BucketCosts> {
    let scalar = |sql: &str| -> rusqlite::Result<f64> {
        conn.prepare_cached(sql)?
            .query_row(rusqlite::params![session_id, through], |row| row.get(0))
    };
    // every-kill: spend (the shots of the kills ended after the reading).
    let weapon = scalar(
        "SELECT COALESCE(SUM(COALESCE(ts.cost_per_shot, 0) * COALESCE(ts.shots_fired, 0)), 0) \
         FROM kill_tool_stats ts JOIN kills k ON k.id = ts.kill_id \
         WHERE k.session_id = ?1 AND k.timestamp > ?2",
    )?;
    let heal = scalar(
        "SELECT COALESCE(SUM(a.cost_ped), 0) \
                + COALESCE((SELECT SUM(c.cost_delta_ped) FROM healing_corrections c \
                            JOIN healing_activations h ON h.id = c.activation_id \
                            WHERE c.session_id = ?1 AND c.undone_at IS NULL \
                              AND h.observed_at > ?2), 0) \
         FROM healing_activations a WHERE a.session_id = ?1 AND a.observed_at > ?2",
    )?;
    let consumable = scalar(
        "SELECT COALESCE(SUM(cost_ped), 0) FROM consumable_doses \
         WHERE session_id = ?1 AND removed_at IS NULL AND started_at > ?2",
    )?;
    let harvest = scalar(
        "SELECT COALESCE(SUM(cost_ped), 0) FROM harvest_events \
         WHERE session_id = ?1 AND timestamp > ?2",
    )?;
    Ok(BucketCosts {
        weapon,
        heal,
        consumable,
        harvest,
        dangling: tracked.dangling,
        ..BucketCosts::default()
    })
}

/// Keep the resolution of an ended session whose seal is pending: what its
/// readings measured up to their last reading while it ran, and the
/// tracked cost of its stored events after that reading, bucket by bucket
/// as [`live`] blends them, so its figures do not fall back to tracked
/// between its stop and its seal. Only the blended buckets are kept (as
/// measured, shared over their events); the rest read tracked. `Ok(false)`
/// when the session has nothing pending. The seal drops the published row
/// and refreshes this resolution from the sealed costs.
fn refresh_pending(conn: &rusqlite::Connection, session_id: &str) -> rusqlite::Result<bool> {
    let Some((so_far, computed_at)) = read_so_far(conn, session_id, true)? else {
        return Ok(false);
    };
    let tracked = tracked(conn, session_id)?;
    let after = tracked_after_stored(conn, session_id, so_far.through, &tracked)?;
    let live = live(Some(&so_far), &tracked, &after);
    conn.execute(
        &format!(
            "INSERT INTO {RESOLUTIONS} \
                 (session_id, resolution_version, projection_computed_at, across_gap_seconds, \
                  overhead_travel_ped, overhead_field_logistics_ped, overhead_unclassified_ped) \
             VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6)"
        ),
        rusqlite::params![
            session_id,
            RESOLUTION_VERSION,
            computed_at,
            live.overhead.travel,
            live.overhead.field_logistics,
            live.overhead.unclassified,
        ],
    )?;
    let mut insert = conn.prepare_cached(&format!(
        "INSERT INTO {RESOLUTION_BUCKETS} \
             (session_id, bucket, basis, reason, measured_ped, scale) \
         VALUES (?1, ?2, 'measured', NULL, ?3, ?4)"
    ))?;
    for bucket in Bucket::ALL
        .into_iter()
        .filter(|bucket| live.is_measured(*bucket))
    {
        let resolved = resolve_measured(bucket, live.cycled.of(bucket), &tracked);
        let Basis::Measured(measured) = resolved.basis else {
            continue;
        };
        insert.execute(rusqlite::params![
            session_id,
            bucket.as_str(),
            measured,
            resolved.scale
        ])?;
    }
    Ok(true)
}

/// A published row's amounts, Cycled's (but the enhancer) then the
/// overhead's, as [`MeasuredSoFar`]. `Err` names the first value this
/// module cannot weigh with.
fn decode_so_far(through: f64, amounts: &[Option<i64>]) -> Result<MeasuredSoFar, String> {
    if !through.is_finite() {
        return Err(format!("session cost measured so far at {through}"));
    }
    let mut decoded = amounts.iter().map(|amount| {
        amount
            .map(|micro_ped| in_range(micro_ped as f64 / 1_000_000.0, "measured so far"))
            .transpose()
    });
    let mut next = || decoded.next().unwrap_or(Ok(None));
    let mut cycled = [None; 6];
    for (index, column) in LIVE_CYCLED.iter().enumerate() {
        if column.is_some() {
            cycled[index] = next()?;
        }
    }
    let mut overhead = [None; 3];
    for slot in &mut overhead {
        *slot = next()?;
    }
    Ok(MeasuredSoFar {
        through,
        cycled,
        overhead: Overhead::from_amounts(overhead),
    })
}

/// A running session's live costs: per bucket, what the readings measured
/// up to their latest reading and the tracked cost of the events after it;
/// tracked where nothing was measured.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveCosts {
    /// The tracked buckets the figures were built from.
    pub tracked: BucketCosts,
    /// Cycled, bucket by bucket.
    pub cycled: BucketCosts,
    /// The overhead measured up to the latest reading: nothing tracks it.
    pub overhead: Overhead,
    /// When the measured part runs to, while any of the figures rests on
    /// it.
    pub measured_through: Option<f64>,
    measured: [bool; 6],
}

/// How a bucket's tracked lines carry its live figure below the session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LiveShare {
    /// The bucket is tracked: each line stands as it is.
    Tracked,
    /// Each line's tracked cost times this.
    Scaled(f64),
    /// No tracked line carries it: the whole amount sits at the bucket.
    Whole(f64),
}

impl LiveCosts {
    /// Whether the bucket's live figure rests on the readings.
    pub fn is_measured(&self, bucket: Bucket) -> bool {
        self.measured[bucket.index()]
    }

    /// The session's Costs so far: its Cycled and its overhead.
    pub fn costs(&self) -> f64 {
        self.cycled.cycled() + self.overhead.total()
    }

    /// How the bucket's tracked lines carry its live figure. Armour's
    /// tracked lines are what the player's recordings spread over the
    /// session, never events of its play: a measured armour figure sits
    /// whole at the bucket.
    pub fn share(&self, bucket: Bucket) -> LiveShare {
        if !self.is_measured(bucket) {
            return LiveShare::Tracked;
        }
        let live = self.cycled.of(bucket);
        let tracked = self.tracked.of(bucket);
        if bucket != Bucket::Armour && tracked > 0.0 {
            LiveShare::Scaled(live / tracked)
        } else {
            LiveShare::Whole(live)
        }
    }
}

/// A running session's live costs, from what the readings measured of it
/// so far (`so_far`, none before its first reading after the baseline),
/// its tracked buckets now (`tracked`), and, of those, the cost of the
/// events timed after the reading `so_far` runs to (`after`; a kill counts
/// whole on the side it ended, so a fight still running is after it).
///
/// A bucket the readings measured is that amount plus the tracked cost of
/// its events after the reading; armour, whose tracked figure holds no
/// event of play, and the overhead, which nothing tracks, are what the
/// readings measured alone. A bucket the readings could not settle, the
/// enhancer always among them, keeps its tracked figure. With nothing
/// measured, every figure is the tracked one.
pub fn live(
    so_far: Option<&MeasuredSoFar>,
    tracked: &BucketCosts,
    after: &BucketCosts,
) -> LiveCosts {
    let mut costs = LiveCosts {
        tracked: *tracked,
        cycled: *tracked,
        overhead: Overhead::default(),
        measured_through: None,
        measured: [false; 6],
    };
    let Some(so_far) = so_far else {
        return costs;
    };
    for bucket in Bucket::ALL {
        let Some(measured) = so_far.bucket(bucket) else {
            continue;
        };
        costs.measured[bucket.index()] = true;
        let since = match bucket {
            Bucket::Armour => 0.0,
            _ => after.of(bucket),
        };
        let figure = measured + since;
        match bucket {
            Bucket::Weapon => {
                costs.cycled.weapon = figure;
                costs.cycled.dangling = 0.0;
            }
            Bucket::Enhancer => costs.cycled.enhancer = figure,
            Bucket::Harvesting => costs.cycled.harvest = figure,
            Bucket::Healing => costs.cycled.heal = figure,
            Bucket::Armour => costs.cycled.armour = figure,
            Bucket::Consumables => costs.cycled.consumable = figure,
        }
    }
    costs.overhead = so_far.overhead;
    let any_measured =
        costs.measured.iter().any(|measured| *measured) || so_far.overhead.total() != 0.0;
    costs.measured_through = any_measured.then_some(so_far.through);
    costs
}
